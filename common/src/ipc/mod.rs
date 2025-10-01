use std::path::PathBuf;

use transmit::RawMsg;

mod error;
mod socket;
mod transmit;
mod types;

use crate::cache;
use crate::mmap::Mmap;
pub use error::*;
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
    pub fn push(
        &mut self,
        img: ImgSend,
        namespace: String,
        filter: String,
        outputs: &[String],
        animation: Option<Animation>,
    ) {
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
            if let Err(e) = super::cache::CacheEntry::new(&namespace, &filter, path).store(output) {
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
    Pause,
    Kill,
}

pub enum RequestRecv {
    Ping,
    Query,
    Clear(ClearReq),
    Img(ImageReq),
    Pause,
    Kill,
}

impl RequestSend {
    pub fn send(self, stream: &IpcSocket<Client>) -> Result<(), String> {
        match stream.send(self.into()) {
            Ok(true) => Ok(()),
            Ok(false) => Err("failed to send full length of message in socket!".to_string()),
            Err(e) => Err(format!("failed to write serialized request: {e}")),
        }
    }
}

impl RequestRecv {
    #[must_use]
    #[inline]
    pub fn receive(msg: RawMsg) -> Self {
        msg.into()
    }
}

pub enum Answer {
    Ok,
    Ping(bool),
    Info(Box<[BgInfo]>),
}

impl Answer {
    pub fn send(self, stream: &IpcSocket<Server>) -> Result<(), String> {
        match stream.send(self.into()) {
            Ok(true) => Ok(()),
            Ok(false) => Err("failed to send full length of message in socket!".to_string()),
            Err(e) => Err(format!("failed to write serialized request: {e}")),
        }
    }

    #[must_use]
    #[inline]
    pub fn receive(msg: RawMsg) -> Self {
        msg.into()
    }
}
