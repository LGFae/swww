use std::path::PathBuf;

use rustix::fd::OwnedFd;

mod error;
mod mmap;
mod socket;
mod types;

use crate::cache;
pub use error::*;
pub use mmap::*;
pub use socket::*;
pub use types::*;

pub struct ImageRequestBuilder {
    memory: Mmap,
    len: usize,
    img_count: u8,
    img_count_index: usize,
}

impl ImageRequestBuilder {
    #[inline]
    pub fn new(transition: Transition) -> Self {
        let memory = Mmap::create(1 << (20 + 3)); // start with 8 MB
        let len = 0;
        let mut builder = Self {
            memory,
            len,
            img_count: 0,
            img_count_index: 0,
        };
        transition.serialize(&mut builder);
        builder.img_count_index = builder.len;
        builder.len += 1;
        assert_eq!(builder.len, 52);
        builder
    }

    fn push_byte(&mut self, byte: u8) {
        if self.len >= self.memory.len() {
            self.grow();
        }
        self.memory.slice_mut()[self.len] = byte;
        self.len += 1;
    }

    pub(crate) fn extend(&mut self, bytes: &[u8]) {
        if self.len + bytes.len() >= self.memory.len() {
            self.memory.remap(self.memory.len() + bytes.len() * 2);
        }
        self.memory.slice_mut()[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len()
    }

    fn grow(&mut self) {
        self.memory.remap((self.memory.len() * 3) / 2);
    }

    #[inline]
    pub fn push(&mut self, img: ImgSend, outputs: &[String], animation: Option<Animation>) {
        self.img_count += 1;

        let ImgSend {
            path,
            img,
            dim: dims,
            format,
        } = &img;
        self.serialize_bytes(path.as_bytes());
        self.serialize_bytes(img);
        self.extend(&dims.0.to_ne_bytes());
        self.extend(&dims.1.to_ne_bytes());
        self.push_byte(*format as u8);

        self.push_byte(outputs.len() as u8);
        for output in outputs.iter() {
            self.serialize_bytes(output.as_bytes());
        }

        let animation_start = self.len + 1;
        if let Some(animation) = animation.as_ref() {
            self.push_byte(1);
            animation.serialize(self);
        } else {
            self.push_byte(0);
        }

        // cache the request
        for output in outputs.iter() {
            if let Err(e) = super::cache::store(output, path) {
                eprintln!("ERROR: failed to store cache: {e}");
            }
        }

        if animation.is_some() && path != "-" {
            let p = PathBuf::from(&path);
            if let Err(e) = cache::store_animation_frames(
                &self.memory.slice()[animation_start..],
                &p,
                *dims,
                *format,
            ) {
                eprintln!("Error storing cache for {}: {e}", path);
            }
        }
    }

    #[inline]
    pub fn build(mut self) -> Mmap {
        self.memory.slice_mut()[self.img_count_index] = self.img_count;
        self.memory
    }

    fn serialize_bytes(&mut self, bytes: &[u8]) {
        self.extend(&(bytes.len() as u32).to_ne_bytes());
        self.extend(bytes);
    }
}

pub enum RequestSend {
    Ping,
    Query,
    Clear(Mmap),
    Img(Mmap),
    Kill,
}

pub enum RequestRecv {
    Ping,
    Query,
    Clear(ClearReq),
    Img(ImageReq),
    Kill,
}

impl RequestSend {
    pub fn send(&self, stream: &OwnedFd) -> Result<(), String> {
        let mut socket_msg = [0u8; 16];
        socket_msg[0..8].copy_from_slice(&match self {
            Self::Ping => 0u64.to_ne_bytes(),
            Self::Query => 1u64.to_ne_bytes(),
            Self::Clear(_) => 2u64.to_ne_bytes(),
            Self::Img(_) => 3u64.to_ne_bytes(),
            Self::Kill => 4u64.to_ne_bytes(),
        });

        let mmap = match self {
            Self::Clear(clear) => Some(clear),
            Self::Img(img) => Some(img),
            _ => None,
        };

        match send_socket_msg(stream, &mut socket_msg, mmap) {
            Ok(true) => (),
            Ok(false) => return Err("failed to send full length of message in socket!".to_string()),
            Err(e) => return Err(format!("failed to write serialized request: {e}")),
        }

        Ok(())
    }
}

impl RequestRecv {
    #[must_use]
    #[inline]
    pub fn receive(socket_msg: SocketMsg) -> Self {
        let ret = match socket_msg.code {
            0 => Self::Ping,
            1 => Self::Query,
            2 => {
                let mmap = socket_msg.shm.unwrap();
                let bytes = mmap.slice();
                let len = bytes[0] as usize;
                let mut outputs = Vec::with_capacity(len);
                let mut i = 1;
                for _ in 0..len {
                    let output = MmappedStr::new(&mmap, &bytes[i..]);
                    i += 4 + output.str().len();
                    outputs.push(output);
                }
                let color = [bytes[i], bytes[i + 1], bytes[i + 2]];
                Self::Clear(ClearReq {
                    color,
                    outputs: outputs.into(),
                })
            }
            3 => {
                let mmap = socket_msg.shm.unwrap();
                let bytes = mmap.slice();
                let transition = Transition::deserialize(&bytes[0..]);
                let len = bytes[51] as usize;

                let mut imgs = Vec::with_capacity(len);
                let mut outputs = Vec::with_capacity(len);
                let mut animations = Vec::with_capacity(len);

                let mut i = 52;
                for _ in 0..len {
                    let (img, offset) = ImgReq::deserialize(&mmap, &bytes[i..]);
                    i += offset;
                    imgs.push(img);

                    let n_outputs = bytes[i] as usize;
                    i += 1;
                    let mut out = Vec::with_capacity(n_outputs);
                    for _ in 0..n_outputs {
                        let output = MmappedStr::new(&mmap, &bytes[i..]);
                        i += 4 + output.str().len();
                        out.push(output);
                    }
                    outputs.push(out.into());

                    if bytes[i] == 1 {
                        let (animation, offset) = Animation::deserialize(&mmap, &bytes[i + 1..]);
                        i += offset;
                        animations.push(animation);
                    }
                    i += 1;
                }

                Self::Img(ImageReq {
                    transition,
                    imgs: imgs.into(),
                    outputs: outputs.into(),
                    animations: if animations.is_empty() {
                        None
                    } else {
                        Some(animations.into())
                    },
                })
            }
            _ => Self::Kill,
        };
        ret
    }
}

pub enum Answer {
    Ok,
    Ping(bool),
    Info(Box<[BgInfo]>),
}

impl Answer {
    pub fn send(&self, stream: &OwnedFd) -> Result<(), String> {
        let mut socket_msg = [0u8; 16];
        socket_msg[0..8].copy_from_slice(&match self {
            Self::Ok => 0u64.to_ne_bytes(),
            Self::Ping(true) => 1u64.to_ne_bytes(),
            Self::Ping(false) => 2u64.to_ne_bytes(),
            Self::Info(_) => 3u64.to_ne_bytes(),
        });

        let mmap = match self {
            Self::Info(infos) => {
                let len = 1 + infos.iter().map(|i| i.serialized_size()).sum::<usize>();
                let mut mmap = Mmap::create(len);
                let bytes = mmap.slice_mut();

                bytes[0] = infos.len() as u8;
                let mut i = 1;

                for info in infos.iter() {
                    i += info.serialize(&mut bytes[i..]);
                }

                Some(mmap)
            }
            _ => None,
        };

        match send_socket_msg(stream, &mut socket_msg, mmap.as_ref()) {
            Ok(true) => Ok(()),
            Ok(false) => Err("failed to send full length of message in socket!".to_string()),
            Err(e) => Err(format!("failed to write serialized request: {e}")),
        }
    }

    #[must_use]
    #[inline]
    pub fn receive(socket_msg: SocketMsg) -> Self {
        match socket_msg.code {
            0 => Self::Ok,
            1 => Self::Ping(true),
            2 => Self::Ping(false),
            3 => {
                let mmap = socket_msg.shm.unwrap();
                let bytes = mmap.slice();
                let len = bytes[0] as usize;
                let mut bg_infos = Vec::with_capacity(len);

                let mut i = 1;
                for _ in 0..len {
                    let (info, offset) = BgInfo::deserialize(&bytes[i..]);
                    i += offset;
                    bg_infos.push(info);
                }

                Self::Info(bg_infos.into())
            }
            _ => panic!("Received malformed answer from daemon"),
        }
    }
}
