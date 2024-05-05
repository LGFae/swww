use std::{
    fmt,
    num::{NonZeroI32, NonZeroU8},
    path::PathBuf,
    ptr::NonNull,
    time::Duration,
};

use rustix::{
    fd::{AsFd, BorrowedFd, OwnedFd},
    io::Errno,
    mm::{mmap, munmap, MapFlags, ProtFlags},
    net::{self, RecvFlags},
    shm::{Mode, ShmOFlags},
};

use crate::{cache, compression::BitPack};

#[derive(Clone, PartialEq)]
pub enum Coord {
    Pixel(f32),
    Percent(f32),
}

#[derive(Clone, PartialEq)]
pub struct Position {
    pub x: Coord,
    pub y: Coord,
}

impl Position {
    #[must_use]
    pub fn new(x: Coord, y: Coord) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub fn to_pixel(&self, dim: (u32, u32), invert_y: bool) -> (f32, f32) {
        let x = match self.x {
            Coord::Pixel(x) => x,
            Coord::Percent(x) => x * dim.0 as f32,
        };

        let y = match self.y {
            Coord::Pixel(y) => {
                if invert_y {
                    y
                } else {
                    dim.1 as f32 - y
                }
            }
            Coord::Percent(y) => {
                if invert_y {
                    y * dim.1 as f32
                } else {
                    (1.0 - y) * dim.1 as f32
                }
            }
        };

        (x, y)
    }

    #[must_use]
    pub fn to_percent(&self, dim: (u32, u32)) -> (f32, f32) {
        let x = match self.x {
            Coord::Pixel(x) => x / dim.0 as f32,
            Coord::Percent(x) => x,
        };

        let y = match self.y {
            Coord::Pixel(y) => y / dim.1 as f32,
            Coord::Percent(y) => y,
        };

        (x, y)
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum BgImg {
    Color([u8; 3]),
    Img(String),
}

impl fmt::Display for BgImg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BgImg::Color(color) => {
                write!(f, "color: {:02X}{:02X}{:02X}", color[0], color[1], color[2])
            }
            BgImg::Img(p) => write!(f, "image: {p}",),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum PixelFormat {
    /// No swap, can copy directly onto WlBuffer
    Bgr = 0,
    /// Swap R and B channels at client, can copy directly onto WlBuffer
    Rgb = 1,
    /// No swap, must extend pixel with an extra byte when copying
    Xbgr = 2,
    /// Swap R and B channels at client, must extend pixel with an extra byte when copying
    Xrgb = 3,
}

impl PixelFormat {
    #[inline]
    #[must_use]
    pub const fn channels(&self) -> u8 {
        match self {
            Self::Rgb => 3,
            Self::Bgr => 3,
            Self::Xbgr => 4,
            Self::Xrgb => 4,
        }
    }

    #[inline]
    #[must_use]
    pub const fn must_swap_r_and_b_channels(&self) -> bool {
        match self {
            Self::Bgr => false,
            Self::Rgb => true,
            Self::Xbgr => false,
            Self::Xrgb => true,
        }
    }

    #[inline]
    #[must_use]
    pub const fn can_copy_directly_onto_wl_buffer(&self) -> bool {
        match self {
            Self::Bgr => true,
            Self::Rgb => true,
            Self::Xbgr => false,
            Self::Xrgb => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scale {
    Whole(NonZeroI32),
    Fractional(NonZeroI32),
}

impl Scale {
    #[inline]
    #[must_use]
    pub fn mul_dim(&self, width: i32, height: i32) -> (i32, i32) {
        match self {
            Scale::Whole(i) => (width * i.get(), height * i.get()),
            Scale::Fractional(f) => {
                let scale = f.get() as f64 / 120.0;
                let width = (width as f64 * scale).round() as i32;
                let height = (height as f64 * scale).round() as i32;
                (width, height)
            }
        }
    }

    #[inline]
    #[must_use]
    pub fn div_dim(&self, width: i32, height: i32) -> (i32, i32) {
        match self {
            Scale::Whole(i) => (width / i.get(), height / i.get()),
            Scale::Fractional(f) => {
                let scale = 120.0 / f.get() as f64;
                let width = (width as f64 * scale).round() as i32;
                let height = (height as f64 * scale).round() as i32;
                (width, height)
            }
        }
    }
}

impl fmt::Display for Scale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Scale::Whole(i) => i.get() as f32,
                Scale::Fractional(f) => f.get() as f32 / 120.0,
            }
        )
    }
}

#[derive(Clone)]
pub struct BgInfo {
    pub name: String,
    pub dim: (u32, u32),
    pub scale_factor: Scale,
    pub img: BgImg,
    pub pixel_format: PixelFormat,
}

impl BgInfo {
    #[inline]
    #[must_use]
    pub fn real_dim(&self) -> (u32, u32) {
        let dim = self
            .scale_factor
            .mul_dim(self.dim.0 as i32, self.dim.1 as i32);
        (dim.0 as u32, dim.1 as u32)
    }

    fn serialize(&self, buf: &mut Vec<u8>) {
        let Self {
            name,
            dim,
            scale_factor,
            img,
            pixel_format,
        } = self;

        serialize_bytes(name.as_bytes(), buf);
        buf.extend(dim.0.to_ne_bytes());
        buf.extend(dim.1.to_ne_bytes());

        match scale_factor {
            Scale::Whole(value) => {
                buf.push(0);
                buf.extend(value.get().to_ne_bytes());
            }
            Scale::Fractional(value) => {
                buf.push(1);
                buf.extend(value.get().to_ne_bytes());
            }
        }

        match img {
            BgImg::Color(color) => {
                buf.push(0);
                buf.extend(color);
            }
            BgImg::Img(path) => {
                buf.push(1);
                serialize_bytes(path.as_bytes(), buf);
            }
        }

        buf.push(*pixel_format as u8);
    }

    fn deserialize(bytes: &[u8]) -> (Self, usize) {
        let name = deserialize_string(bytes);
        let mut i = name.len() + 4;

        assert!(bytes.len() > i + 17);

        let dim = (
            u32::from_ne_bytes(bytes[i..i + 4].try_into().unwrap()),
            u32::from_ne_bytes(bytes[i + 4..i + 8].try_into().unwrap()),
        );
        i += 8;

        let scale_factor = if bytes[i] == 0 {
            Scale::Whole(
                i32::from_ne_bytes(bytes[i + 1..i + 5].try_into().unwrap())
                    .try_into()
                    .unwrap(),
            )
        } else {
            Scale::Fractional(
                i32::from_ne_bytes(bytes[i + 1..i + 5].try_into().unwrap())
                    .try_into()
                    .unwrap(),
            )
        };
        i += 5;

        let img = if bytes[i] == 0 {
            i += 4;
            BgImg::Color([bytes[i - 3], bytes[i - 2], bytes[i - 1]])
        } else {
            i += 1;
            let path = deserialize_string(&bytes[i..]);
            i += 4 + path.len();
            BgImg::Img(path)
        };

        let pixel_format = match bytes[i] {
            0 => PixelFormat::Bgr,
            1 => PixelFormat::Rgb,
            2 => PixelFormat::Xbgr,
            _ => PixelFormat::Xrgb,
        };
        i += 1;

        (
            Self {
                name,
                dim,
                scale_factor,
                img,
                pixel_format,
            },
            i,
        )
    }
}

impl fmt::Display for BgInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}x{}, scale: {}, currently displaying: {}",
            self.name, self.dim.0, self.dim.1, self.scale_factor, self.img
        )
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
pub enum TransitionType {
    Simple = 0,
    Fade = 1,
    Outer = 2,
    Wipe = 3,
    Grow = 4,
    Wave = 5,
    None = 6,
}

pub struct Transition {
    pub transition_type: TransitionType,
    pub duration: f32,
    pub step: NonZeroU8,
    pub fps: u16,
    pub angle: f64,
    pub pos: Position,
    pub bezier: (f32, f32, f32, f32),
    pub wave: (f32, f32),
    pub invert_y: bool,
}

impl Transition {
    fn serialize(&self, buf: &mut Vec<u8>) {
        let Self {
            transition_type,
            duration,
            step,
            fps,
            angle,
            pos,
            bezier,
            wave,
            invert_y,
        } = self;

        buf.push(*transition_type as u8);
        buf.extend(duration.to_ne_bytes());
        buf.push(step.get());
        buf.extend(fps.to_ne_bytes());
        buf.extend(angle.to_ne_bytes());
        match pos.x {
            Coord::Pixel(f) => {
                buf.push(0);
                buf.extend(f.to_ne_bytes());
            }
            Coord::Percent(f) => {
                buf.push(1);
                buf.extend(f.to_ne_bytes());
            }
        }
        match pos.y {
            Coord::Pixel(f) => {
                buf.push(0);
                buf.extend(f.to_ne_bytes());
            }
            Coord::Percent(f) => {
                buf.push(1);
                buf.extend(f.to_ne_bytes());
            }
        }
        buf.extend(bezier.0.to_ne_bytes());
        buf.extend(bezier.1.to_ne_bytes());
        buf.extend(bezier.2.to_ne_bytes());
        buf.extend(bezier.3.to_ne_bytes());
        buf.extend(wave.0.to_ne_bytes());
        buf.extend(wave.1.to_ne_bytes());
        buf.push(*invert_y as u8);
    }

    fn deserialize(bytes: &[u8]) -> Self {
        assert!(bytes.len() > 50);
        let transition_type = match bytes[0] {
            0 => TransitionType::Simple,
            1 => TransitionType::Fade,
            2 => TransitionType::Outer,
            3 => TransitionType::Wipe,
            4 => TransitionType::Grow,
            5 => TransitionType::Wave,
            _ => TransitionType::None,
        };
        let duration = f32::from_ne_bytes(bytes[1..5].try_into().unwrap());
        let step = NonZeroU8::new(bytes[5]).expect("received step of 0");
        let fps = u16::from_ne_bytes(bytes[6..8].try_into().unwrap());
        let angle = f64::from_ne_bytes(bytes[8..16].try_into().unwrap());
        let pos = {
            let x = if bytes[16] == 0 {
                Coord::Pixel(f32::from_ne_bytes(bytes[17..21].try_into().unwrap()))
            } else {
                Coord::Percent(f32::from_ne_bytes(bytes[17..21].try_into().unwrap()))
            };
            let y = if bytes[21] == 0 {
                Coord::Pixel(f32::from_ne_bytes(bytes[22..26].try_into().unwrap()))
            } else {
                Coord::Percent(f32::from_ne_bytes(bytes[22..26].try_into().unwrap()))
            };
            Position { x, y }
        };

        let bezier = (
            f32::from_ne_bytes(bytes[26..30].try_into().unwrap()),
            f32::from_ne_bytes(bytes[30..34].try_into().unwrap()),
            f32::from_ne_bytes(bytes[34..38].try_into().unwrap()),
            f32::from_ne_bytes(bytes[38..42].try_into().unwrap()),
        );

        let wave = (
            f32::from_ne_bytes(bytes[42..46].try_into().unwrap()),
            f32::from_ne_bytes(bytes[46..50].try_into().unwrap()),
        );

        let invert_y = bytes[50] != 0;

        Self {
            transition_type,
            duration,
            step,
            fps,
            angle,
            pos,
            bezier,
            wave,
            invert_y,
        }
    }
}

pub struct Clear {
    pub color: [u8; 3],
    pub outputs: Box<[String]>,
}

pub struct ClearRecv {
    pub color: [u8; 3],
    pub outputs: Box<[MmappedStr]>,
}

pub struct Img {
    pub path: String,
    pub dim: (u32, u32),
    pub format: PixelFormat,
    pub img: Box<[u8]>,
}

pub struct ImgRecv {
    pub path: MmappedStr,
    pub dim: (u32, u32),
    pub format: PixelFormat,
    pub img: MmappedBytes,
}

pub struct Animation {
    pub animation: Box<[(BitPack, Duration)]>,
}

impl Animation {
    pub(crate) fn serialize(&self, buf: &mut Vec<u8>) {
        let Self { animation } = self;

        buf.extend((animation.len() as u32).to_ne_bytes());
        for (bitpack, duration) in animation.iter() {
            bitpack.serialize(buf);
            buf.extend(duration.as_secs_f64().to_ne_bytes())
        }
    }

    pub(crate) fn deserialize(mmap: &Mmap, bytes: &[u8]) -> (Self, usize) {
        let mut i = 0;
        let animation_len = u32::from_ne_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        i += 4;
        let mut animation = Vec::with_capacity(animation_len);
        for _ in 0..animation_len {
            let (anim, offset) = BitPack::deserialize(mmap, &bytes[i..]);
            i += offset;
            let duration =
                Duration::from_secs_f64(f64::from_ne_bytes(bytes[i..i + 8].try_into().unwrap()));
            i += 8;
            animation.push((anim, duration));
        }

        (
            Self {
                animation: animation.into(),
            },
            i,
        )
    }
}

pub struct ImageRequest {
    pub transition: Transition,
    pub imgs: Box<[Img]>,
    pub outputs: Box<[Box<[String]>]>,
    pub animations: Option<Box<[Animation]>>,
}

pub struct ImageRecv {
    pub transition: Transition,
    pub imgs: Box<[ImgRecv]>,
    pub outputs: Box<[Box<[MmappedStr]>]>,
    pub animations: Option<Box<[Animation]>>,
}

pub struct ImageRequestBuilder {
    transition: Transition,
    imgs: Vec<Img>,
    outputs: Vec<Box<[String]>>,
    animations: Vec<Animation>,
}

impl ImageRequestBuilder {
    #[inline]
    pub fn new(transition: Transition) -> Self {
        Self {
            transition,
            imgs: Vec::new(),
            outputs: Vec::new(),
            animations: Vec::new(),
        }
    }

    #[inline]
    pub fn push(&mut self, img: Img, outputs: Box<[String]>, animation: Option<Animation>) {
        self.imgs.push(img);
        self.outputs.push(outputs);
        if let Some(animation) = animation {
            self.animations.push(animation);
        }
    }

    #[inline]
    pub fn build(self) -> ImageRequest {
        let animations = if self.animations.is_empty() {
            None
        } else {
            assert_eq!(self.animations.len(), self.imgs.len());
            Some(self.animations.into_boxed_slice())
        };
        ImageRequest {
            transition: self.transition,
            imgs: self.imgs.into_boxed_slice(),
            outputs: self.outputs.into_boxed_slice(),
            animations,
        }
    }
}

pub enum Request {
    Ping,
    Query,
    Clear(Clear),
    Img(ImageRequest),
    Kill,
}

pub enum RequestRecv {
    Ping,
    Query,
    Clear(ClearRecv),
    Img(ImageRecv),
    Kill,
}

impl Request {
    pub fn send(&self, stream: &OwnedFd) -> Result<(), String> {
        let mut socket_msg = [0u8; 16];
        socket_msg[0..8].copy_from_slice(&match self {
            Request::Ping => 0u64.to_ne_bytes(),
            Request::Query => 1u64.to_ne_bytes(),
            Request::Clear(_) => 2u64.to_ne_bytes(),
            Request::Img(_) => 3u64.to_ne_bytes(),
            Request::Kill => 4u64.to_ne_bytes(),
        });
        let bytes: Vec<u8> = match self {
            Self::Clear(clear) => {
                let mut buf = Vec::with_capacity(64);
                buf.push(clear.outputs.len() as u8); // we assume someone does not have more than
                                                     // 255 monitors. Seems reasonable
                for output in clear.outputs.iter() {
                    serialize_bytes(output.as_bytes(), &mut buf);
                }
                buf.extend(clear.color);
                buf
            }
            Self::Img(img) => {
                let ImageRequest {
                    transition,
                    imgs,
                    outputs,
                    animations,
                } = img;

                let mut buf = Vec::with_capacity(imgs[0].img.len() + 1024);
                transition.serialize(&mut buf);
                buf.push(imgs.len() as u8); // we assume someone does not have more than 255
                                            // monitors

                for i in 0..outputs.len() {
                    let Img {
                        path,
                        img,
                        dim: dims,
                        format,
                    } = &imgs[i];
                    serialize_bytes(path.as_bytes(), &mut buf);
                    serialize_bytes(img, &mut buf);
                    buf.extend(dims.0.to_ne_bytes());
                    buf.extend(dims.1.to_ne_bytes());
                    buf.push(*format as u8);

                    let output = &outputs[i];
                    buf.push(output.len() as u8);
                    for output in output.iter() {
                        serialize_bytes(output.as_bytes(), &mut buf);
                    }

                    if let Some(animation) = animations {
                        buf.push(1);
                        animation[i].serialize(&mut buf);
                    } else {
                        buf.push(0);
                    }
                }

                buf
            }
            _ => vec![],
        };

        match send_socket_msg(stream, &mut socket_msg, &bytes) {
            Ok(true) => (),
            Ok(false) => return Err("failed to send full length of message in socket!".to_string()),
            Err(e) => return Err(format!("failed to write serialized request: {e}")),
        }

        if let Self::Img(ImageRequest {
            imgs,
            outputs,
            animations,
            ..
        }) = self
        {
            for i in 0..outputs.len() {
                let Img {
                    path,
                    dim: dims,
                    format,
                    ..
                } = &imgs[i];
                for output in outputs[i].iter() {
                    if let Err(e) = super::cache::store(output, path) {
                        eprintln!("ERROR: failed to store cache: {e}");
                    }
                }
                if let Some(animations) = animations.as_ref() {
                    for animation in animations.iter() {
                        // only store the cache if we aren't reading from stdin
                        if path != "-" {
                            let p = PathBuf::from(&path);
                            if let Err(e) =
                                cache::store_animation_frames(animation, &p, *dims, *format)
                            {
                                eprintln!("Error storing cache for {}: {e}", path);
                            }
                        }
                    }
                }
            }
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
                Self::Clear(ClearRecv {
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
                    let path = MmappedStr::new(&mmap, &bytes[i..]);
                    i += 4 + path.str().len();

                    let img = MmappedBytes::new(&mmap, &bytes[i..]);
                    i += 4 + img.bytes().len();

                    let dims = (
                        u32::from_ne_bytes(bytes[i..i + 4].try_into().unwrap()),
                        u32::from_ne_bytes(bytes[i + 4..i + 8].try_into().unwrap()),
                    );
                    i += 8;

                    let format = match bytes[i] {
                        0 => PixelFormat::Bgr,
                        1 => PixelFormat::Rgb,
                        2 => PixelFormat::Xbgr,
                        _ => PixelFormat::Xrgb,
                    };
                    i += 1;

                    imgs.push(ImgRecv {
                        path,
                        img,
                        dim: dims,
                        format,
                    });

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
                        i += 1;
                        let (animation, offset) = Animation::deserialize(&mmap, &bytes[i..]);
                        i += offset;
                        animations.push(animation);
                    } else {
                        i += 1;
                    }
                }

                Self::Img(ImageRecv {
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
    Err(String),
}

impl Answer {
    pub fn send(&self, stream: &OwnedFd) -> Result<(), String> {
        let mut socket_msg = [0u8; 16];
        socket_msg[0..8].copy_from_slice(&match self {
            Self::Ok => 0u64.to_ne_bytes(),
            Self::Ping(true) => 1u64.to_ne_bytes(),
            Self::Ping(false) => 2u64.to_ne_bytes(),
            Self::Info(_) => 3u64.to_ne_bytes(),
            Self::Err(_) => 4u64.to_ne_bytes(),
        });

        let bytes = match self {
            Self::Info(infos) => {
                let mut buf = Vec::with_capacity(1024);

                buf.push(infos.len() as u8);
                for info in infos.iter() {
                    info.serialize(&mut buf);
                }

                buf
            }
            Self::Err(s) => {
                let mut buf = Vec::with_capacity(128);
                serialize_bytes(s.as_bytes(), &mut buf);
                buf
            }
            _ => vec![],
        };

        match send_socket_msg(stream, &mut socket_msg, &bytes) {
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
            4 => {
                let mmap = socket_msg.shm.unwrap();
                let bytes = mmap.slice();
                Self::Err(deserialize_string(bytes))
            }
            _ => panic!("Received malformed answer from daemon"),
        }
    }
}

fn send_socket_msg(
    stream: &OwnedFd,
    socket_msg: &mut [u8; 16],
    bytes: &[u8],
) -> rustix::io::Result<bool> {
    let mut ancillary_buf = [0u8; rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = net::SendAncillaryBuffer::new(&mut ancillary_buf);

    let mmap = if !bytes.is_empty() {
        socket_msg[8..].copy_from_slice(&(bytes.len() as u64).to_ne_bytes());
        let mut mmap = Mmap::create(bytes.len());
        mmap.slice_mut().copy_from_slice(bytes);
        Some(mmap)
    } else {
        None
    };

    let msg_buf;
    if let Some(mmap) = mmap.as_ref() {
        msg_buf = [mmap.fd.as_fd()];
        let msg = net::SendAncillaryMessage::ScmRights(&msg_buf);
        ancillary.push(msg);
    }

    let iov = rustix::io::IoSlice::new(&socket_msg[..]);
    net::sendmsg(stream, &[iov], &mut ancillary, net::SendFlags::empty())
        .map(|written| written == socket_msg.len())
}

pub struct SocketMsg {
    code: u8,
    shm: Option<Mmap>,
}

pub fn read_socket(stream: &OwnedFd) -> Result<SocketMsg, String> {
    let mut buf = [0u8; 16];
    let mut ancillary_buf = [0u8; rustix::cmsg_space!(ScmRights(1))];

    let mut control = net::RecvAncillaryBuffer::new(&mut ancillary_buf);

    let mut tries = 0;
    loop {
        let iov = rustix::io::IoSliceMut::new(&mut buf);
        match net::recvmsg(stream, &mut [iov], &mut control, RecvFlags::WAITALL) {
            Ok(_) => break,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock && tries < 5 {
                    std::thread::sleep(Duration::from_millis(1));
                } else {
                    return Err(format!("failed to read serialized length: {e}"));
                }
            }
        }
        tries += 1;
    }

    let code = u64::from_ne_bytes(buf[0..8].try_into().unwrap()) as u8;
    let len = u64::from_ne_bytes(buf[8..16].try_into().unwrap()) as usize;

    let shm = if len == 0 {
        None
    } else {
        let shm_file = match control.drain().next().unwrap() {
            net::RecvAncillaryMessage::ScmRights(mut iter) => iter.next().unwrap(),
            _ => panic!("malformed ancillary message"),
        };
        Some(Mmap::from_fd(shm_file, len))
    };
    Ok(SocketMsg { code, shm })
}

#[must_use]
pub fn get_socket_path() -> PathBuf {
    let runtime_dir = if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        dir
    } else {
        "/tmp/swww".to_string()
    };

    let mut socket_path = PathBuf::new();
    socket_path.push(runtime_dir);

    let mut socket_name = String::new();
    socket_name.push_str("swww-");
    if let Ok(socket) = std::env::var("WAYLAND_DISPLAY") {
        socket_name.push_str(socket.as_str());
    } else {
        socket_name.push_str("wayland-0")
    }
    socket_name.push_str(".socket");

    socket_path.push(socket_name);

    socket_path
}

/// We make sure the Stream is always set to blocking mode
///
/// * `tries` -  how many times to attempt the connection
/// * `interval` - how long to wait between attempts, in milliseconds
pub fn connect_to_socket(addr: &PathBuf, tries: u8, interval: u64) -> Result<OwnedFd, String> {
    let socket = rustix::net::socket_with(
        rustix::net::AddressFamily::UNIX,
        rustix::net::SocketType::STREAM,
        rustix::net::SocketFlags::CLOEXEC,
        None,
    )
    .expect("failed to create socket file descriptor");
    let addr = net::SocketAddrUnix::new(addr).unwrap();
    //Make sure we try at least once
    let tries = if tries == 0 { 1 } else { tries };
    let mut error = None;
    for _ in 0..tries {
        match net::connect_unix(&socket, &addr) {
            Ok(()) => {
                #[cfg(debug_assertions)]
                let timeout = Duration::from_secs(30); //Some operations take a while to respond in debug mode
                #[cfg(not(debug_assertions))]
                let timeout = Duration::from_secs(5);
                if let Err(e) = net::sockopt::set_socket_timeout(
                    &socket,
                    net::sockopt::Timeout::Recv,
                    Some(timeout),
                ) {
                    return Err(format!("failed to set read timeout for socket: {e}"));
                }

                return Ok(socket);
            }
            Err(e) => error = Some(e),
        }
        std::thread::sleep(Duration::from_millis(interval));
    }
    let error = error.unwrap();
    if error.kind() == std::io::ErrorKind::NotFound {
        return Err("Socket file not found. Are you sure swww-daemon is running?".to_string());
    }

    Err(format!("Failed to connect to socket: {error}"))
}

pub struct MmappedBytes {
    base_ptr: NonNull<std::ffi::c_void>,
    ptr: NonNull<std::ffi::c_void>,
    len: usize,
}

impl MmappedBytes {
    const PROT: ProtFlags = ProtFlags::READ;
    const FLAGS: MapFlags = MapFlags::SHARED;

    pub(crate) fn new(map: &Mmap, bytes: &[u8]) -> Self {
        let len = u32::from_ne_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let offset = 4 + bytes.as_ptr() as usize - map.ptr.as_ptr() as usize;
        let page_size = rustix::param::page_size();
        let page_offset = offset - offset % page_size;

        let base_ptr = unsafe {
            let ptr = mmap(
                std::ptr::null_mut(),
                len + (offset - page_offset),
                Self::PROT,
                Self::FLAGS,
                &map.fd,
                page_offset as u64,
            )
            .unwrap();
            // SAFETY: the function above will never return a null pointer if it succeeds
            // POSIX says that the implementation will never select an address at 0
            NonNull::new_unchecked(ptr)
        };
        let ptr =
            unsafe { NonNull::new_unchecked(base_ptr.as_ptr().byte_add(offset - page_offset)) };

        Self { base_ptr, ptr, len }
    }

    pub(crate) fn new_with_len(map: &Mmap, bytes: &[u8], len: usize) -> Self {
        let offset = bytes.as_ptr() as usize - map.ptr.as_ptr() as usize;
        let page_size = rustix::param::page_size();
        let page_offset = offset - offset % page_size;

        let base_ptr = unsafe {
            let ptr = mmap(
                std::ptr::null_mut(),
                len + (offset - page_offset),
                Self::PROT,
                Self::FLAGS,
                &map.fd,
                page_offset as u64,
            )
            .unwrap();
            // SAFETY: the function above will never return a null pointer if it succeeds
            // POSIX says that the implementation will never select an address at 0
            NonNull::new_unchecked(ptr)
        };
        let ptr =
            unsafe { NonNull::new_unchecked(base_ptr.as_ptr().byte_add(offset - page_offset)) };

        Self { base_ptr, ptr, len }
    }

    #[inline]
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr().cast(), self.len) }
    }
}

impl Drop for MmappedBytes {
    #[inline]
    fn drop(&mut self) {
        let len = self.len + self.ptr.as_ptr() as usize - self.base_ptr.as_ptr() as usize;
        if let Err(e) = unsafe { munmap(self.base_ptr.as_ptr(), len) } {
            eprintln!("ERROR WHEN UNMAPPING MEMORY: {e}");
        }
    }
}

unsafe impl Send for MmappedBytes {}
unsafe impl Sync for MmappedBytes {}

pub struct MmappedStr {
    base_ptr: NonNull<std::ffi::c_void>,
    ptr: NonNull<std::ffi::c_void>,
    len: usize,
}

impl MmappedStr {
    const PROT: ProtFlags = ProtFlags::READ;
    const FLAGS: MapFlags = MapFlags::SHARED;

    fn new(map: &Mmap, bytes: &[u8]) -> Self {
        let len = u32::from_ne_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let offset = 4 + bytes.as_ptr() as usize - map.ptr.as_ptr() as usize;
        let page_size = rustix::param::page_size();
        let page_offset = offset - offset % page_size;

        let base_ptr = unsafe {
            let ptr = mmap(
                std::ptr::null_mut(),
                len + (offset - page_offset),
                Self::PROT,
                Self::FLAGS,
                &map.fd,
                page_offset as u64,
            )
            .unwrap();
            // SAFETY: the function above will never return a null pointer if it succeeds
            // POSIX says that the implementation will never select an address at 0
            NonNull::new_unchecked(ptr)
        };
        let ptr =
            unsafe { NonNull::new_unchecked(base_ptr.as_ptr().byte_add(offset - page_offset)) };

        // try to parse, panicking if we fail
        let s = unsafe { std::slice::from_raw_parts(ptr.as_ptr().cast(), len) };
        let _s = std::str::from_utf8(s).expect("received a non utf8 string from socket");

        Self { base_ptr, ptr, len }
    }

    #[inline]
    #[must_use]
    pub fn str(&self) -> &str {
        let s = unsafe { std::slice::from_raw_parts(self.ptr.as_ptr().cast(), self.len) };
        unsafe { std::str::from_utf8_unchecked(s) }
    }
}

unsafe impl Send for MmappedStr {}
unsafe impl Sync for MmappedStr {}

impl Drop for MmappedStr {
    #[inline]
    fn drop(&mut self) {
        let len = self.len + self.ptr.as_ptr() as usize - self.base_ptr.as_ptr() as usize;
        if let Err(e) = unsafe { munmap(self.base_ptr.as_ptr(), len) } {
            eprintln!("ERROR WHEN UNMAPPING MEMORY: {e}");
        }
    }
}

#[derive(Debug)]
pub struct Mmap {
    fd: OwnedFd,
    ptr: NonNull<std::ffi::c_void>,
    len: usize,
}

impl Mmap {
    const PROT: ProtFlags = ProtFlags::WRITE.union(ProtFlags::READ);
    const FLAGS: MapFlags = MapFlags::SHARED;

    #[inline]
    #[must_use]
    pub fn create(len: usize) -> Self {
        let fd = create_shm_fd().unwrap();
        rustix::io::retry_on_intr(|| rustix::fs::ftruncate(&fd, len as u64)).unwrap();

        let ptr = unsafe {
            let ptr = mmap(std::ptr::null_mut(), len, Self::PROT, Self::FLAGS, &fd, 0).unwrap();
            // SAFETY: the function above will never return a null pointer if it succeeds
            // POSIX says that the implementation will never select an address at 0
            NonNull::new_unchecked(ptr)
        };
        Self { fd, ptr, len }
    }

    #[inline]
    pub fn remap(&mut self, new_len: usize) {
        rustix::io::retry_on_intr(|| rustix::fs::ftruncate(&self.fd, new_len as u64)).unwrap();

        #[cfg(target_os = "linux")]
        {
            let result = unsafe {
                rustix::mm::mremap(
                    self.ptr.as_ptr(),
                    self.len,
                    new_len,
                    rustix::mm::MremapFlags::MAYMOVE,
                )
            };

            if let Ok(ptr) = result {
                // SAFETY: the mremap above will never return a null pointer if it succeeds
                let ptr = unsafe { NonNull::new_unchecked(ptr) };
                self.ptr = ptr;
                self.len = new_len;
                return;
            }
        }

        if let Err(e) = unsafe { munmap(self.ptr.as_ptr(), self.len) } {
            eprintln!("ERROR WHEN UNMAPPING MEMORY: {e}");
        }

        self.len = new_len;
        self.ptr = unsafe {
            let ptr = mmap(
                std::ptr::null_mut(),
                self.len,
                Self::PROT,
                Self::FLAGS,
                &self.fd,
                0,
            )
            .unwrap();
            // SAFETY: the function above will never return a null pointer if it succeeds
            // POSIX says that the implementation will never select an address at 0
            NonNull::new_unchecked(ptr)
        };
    }

    #[must_use]
    pub(crate) fn from_fd(fd: OwnedFd, len: usize) -> Self {
        let ptr = unsafe {
            let ptr = mmap(std::ptr::null_mut(), len, Self::PROT, Self::FLAGS, &fd, 0).unwrap();
            // SAFETY: the function above will never return a null pointer if it succeeds
            // POSIX says that the implementation will never select an address at 0
            NonNull::new_unchecked(ptr)
        };
        Self { fd, ptr, len }
    }

    #[inline]
    #[must_use]
    pub fn slice_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr().cast(), self.len) }
    }

    #[inline]
    #[must_use]
    pub fn slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr().cast(), self.len) }
    }

    #[inline]
    #[must_use]
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    #[must_use]
    pub fn fd(&self) -> BorrowedFd {
        self.fd.as_fd()
    }
}

impl Drop for Mmap {
    #[inline]
    fn drop(&mut self) {
        if let Err(e) = unsafe { munmap(self.ptr.as_ptr(), self.len) } {
            eprintln!("ERROR WHEN UNMAPPING MEMORY: {e}");
        }
    }
}

fn create_shm_fd() -> std::io::Result<OwnedFd> {
    #[cfg(target_os = "linux")]
    {
        match create_memfd() {
            Ok(fd) => return Ok(fd),
            // Not supported, use fallback.
            Err(Errno::NOSYS) => (),
            Err(err) => return Err(err.into()),
        };
    }

    let time = std::time::SystemTime::now();
    let mut mem_file_handle = format!(
        "/swww-ipc-{}",
        time.duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );

    let flags = ShmOFlags::CREATE | ShmOFlags::EXCL | ShmOFlags::RDWR;
    let mode = Mode::RUSR | Mode::WUSR;
    loop {
        match rustix::shm::shm_open(mem_file_handle.as_str(), flags, mode) {
            Ok(fd) => match rustix::shm::shm_unlink(mem_file_handle.as_str()) {
                Ok(_) => return Ok(fd),

                Err(errno) => {
                    return Err(errno.into());
                }
            },
            Err(Errno::EXIST) => {
                // Change the handle if we happen to be duplicate.
                let time = std::time::SystemTime::now();

                mem_file_handle = format!(
                    "/swww-ipc-{}",
                    time.duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .subsec_nanos()
                );

                continue;
            }
            Err(Errno::INTR) => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

#[cfg(target_os = "linux")]
fn create_memfd() -> rustix::io::Result<OwnedFd> {
    use rustix::fs::{MemfdFlags, SealFlags};
    use std::ffi::CStr;

    let name = CStr::from_bytes_with_nul(b"swww-ipc\0").unwrap();
    let flags = MemfdFlags::ALLOW_SEALING | MemfdFlags::CLOEXEC;

    loop {
        match rustix::fs::memfd_create(name, flags) {
            Ok(fd) => {
                // We only need to seal for the purposes of optimization, ignore the errors.
                let _ = rustix::fs::fcntl_add_seals(&fd, SealFlags::SHRINK | SealFlags::SEAL);
                return Ok(fd);
            }
            Err(Errno::INTR) => continue,
            Err(err) => return Err(err),
        }
    }
}

fn serialize_bytes(bytes: &[u8], buf: &mut Vec<u8>) {
    buf.extend((bytes.len() as u32).to_ne_bytes());
    buf.extend(bytes);
}

fn deserialize_string(bytes: &[u8]) -> String {
    let size = u32::from_ne_bytes(bytes[0..4].try_into().unwrap()) as usize;
    std::str::from_utf8(&bytes[4..4 + size])
        .expect("received a non utf8 string from socket")
        .to_string()
}
