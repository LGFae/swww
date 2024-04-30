use std::{
    fmt,
    num::{NonZeroI32, NonZeroU8},
    path::PathBuf,
    time::Duration,
};

use rustix::{
    fd::{AsFd, OwnedFd},
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
                    dim.1 as f32 - y
                } else {
                    y
                }
            }
            Coord::Percent(y) => {
                if invert_y {
                    (1.0 - y) * dim.1 as f32
                } else {
                    y * dim.1 as f32
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

        buf.extend((name.len() as u32).to_ne_bytes());
        buf.extend(name.as_bytes());
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
                buf.extend((path.len() as u32).to_ne_bytes());
                buf.extend(path.as_bytes());
            }
        }

        buf.push(*pixel_format as u8);
    }

    fn deserialize(bytes: &[u8]) -> (Self, usize) {
        let mut i = 0;
        let name_size = unsafe { bytes.as_ptr().add(i).cast::<u32>().read_unaligned() } as usize;
        i += 4;
        let name = std::str::from_utf8(&bytes[i..i + name_size])
            .unwrap()
            .to_string();
        i += name_size;

        let dim = (
            unsafe { bytes.as_ptr().add(i).cast::<u32>().read_unaligned() },
            unsafe { bytes.as_ptr().add(i + 4).cast::<u32>().read_unaligned() },
        );
        i += 8;

        let scale_factor = if bytes[i] == 0 {
            Scale::Whole(
                unsafe { bytes.as_ptr().add(i + 1).cast::<i32>().read_unaligned() }
                    .try_into()
                    .unwrap(),
            )
        } else {
            Scale::Fractional(
                unsafe { bytes.as_ptr().add(i + 1).cast::<i32>().read_unaligned() }
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
            let path_size =
                unsafe { bytes.as_ptr().add(i).cast::<u32>().read_unaligned() } as usize;
            i += 4;
            let path = std::str::from_utf8(&bytes[i..i + path_size])
                .unwrap()
                .to_string();
            i += path_size;
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
        let duration = unsafe { bytes.as_ptr().add(1).cast::<f32>().read_unaligned() };
        let step = NonZeroU8::new(bytes[5]).expect("received step of 0");
        let fps = unsafe { bytes.as_ptr().add(6).cast::<u16>().read_unaligned() };
        let angle = unsafe { bytes.as_ptr().add(8).cast::<f64>().read_unaligned() };
        let pos = {
            let x = if bytes[16] == 0 {
                Coord::Pixel(unsafe { bytes.as_ptr().add(17).cast::<f32>().read_unaligned() })
            } else {
                Coord::Percent(unsafe { bytes.as_ptr().add(17).cast::<f32>().read_unaligned() })
            };
            let y = if bytes[21] == 0 {
                Coord::Pixel(unsafe { bytes.as_ptr().add(22).cast::<f32>().read_unaligned() })
            } else {
                Coord::Percent(unsafe { bytes.as_ptr().add(22).cast::<f32>().read_unaligned() })
            };
            Position { x, y }
        };

        let bezier = (
            unsafe { bytes.as_ptr().add(26).cast::<f32>().read_unaligned() },
            unsafe { bytes.as_ptr().add(30).cast::<f32>().read_unaligned() },
            unsafe { bytes.as_ptr().add(34).cast::<f32>().read_unaligned() },
            unsafe { bytes.as_ptr().add(38).cast::<f32>().read_unaligned() },
        );

        let wave = (
            unsafe { bytes.as_ptr().add(42).cast::<f32>().read_unaligned() },
            unsafe { bytes.as_ptr().add(46).cast::<f32>().read_unaligned() },
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

pub struct Img {
    pub path: String,
    pub img: Box<[u8]>,
}

pub struct Animation {
    pub animation: Box<[(BitPack, Duration)]>,
    pub path: String,
    pub dimensions: (u32, u32),
    pub pixel_format: PixelFormat,
}

impl Animation {
    pub(crate) fn serialize(&self, buf: &mut Vec<u8>) {
        let Self {
            animation,
            path,
            dimensions,
            pixel_format,
        } = self;

        buf.extend((animation.len() as u32).to_ne_bytes());
        for (bitpack, duration) in animation.iter() {
            bitpack.serialize(buf);
            buf.extend(duration.as_secs_f64().to_ne_bytes())
        }
        buf.extend((path.len() as u32).to_ne_bytes());
        buf.extend(path.as_bytes());

        buf.extend(dimensions.0.to_ne_bytes());
        buf.extend(dimensions.1.to_ne_bytes());
        buf.push(*pixel_format as u8);
    }

    pub(crate) fn deserialize(bytes: &[u8]) -> (Self, usize) {
        let mut i = 0;
        let animation_len =
            unsafe { bytes.as_ptr().add(i).cast::<u32>().read_unaligned() } as usize;
        i += 4;
        let mut animation = Vec::with_capacity(animation_len);
        for _ in 0..animation_len {
            let (anim, offset) = BitPack::deserialize(&bytes[i..]);
            i += offset;
            let duration = Duration::from_secs_f64(unsafe {
                bytes.as_ptr().add(i).cast::<f64>().read_unaligned()
            });
            i += 8;
            animation.push((anim, duration));
        }

        let path_size = unsafe { bytes.as_ptr().add(i).cast::<u32>().read_unaligned() } as usize;
        i += 4;
        let path = std::str::from_utf8(&bytes[i..i + path_size])
            .unwrap()
            .to_string();
        i += path_size;

        let dimensions = (
            unsafe { bytes.as_ptr().add(i).cast::<u32>().read_unaligned() },
            unsafe { bytes.as_ptr().add(i + 4).cast::<u32>().read_unaligned() },
        );
        i += 8;
        let pixel_format = match bytes[i] {
            0 => PixelFormat::Bgr,
            1 => PixelFormat::Rgb,
            2 => PixelFormat::Xbgr,
            _ => PixelFormat::Xrgb,
        };
        i += 1;

        (
            Self {
                animation: animation.into(),
                path,
                dimensions,
                pixel_format,
            },
            i,
        )
    }
}

pub struct AnimationRequest {
    pub animations: Box<[Animation]>,
    pub outputs: Box<[Box<[String]>]>,
}

pub struct AnimationRequestBuilder {
    animations: Vec<Animation>,
    outputs: Vec<Box<[String]>>,
}

impl AnimationRequestBuilder {
    #[inline]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            animations: Vec::new(),
            outputs: Vec::new(),
        }
    }

    #[inline]
    pub fn push(&mut self, animation: Animation, outputs: Box<[String]>) {
        self.animations.push(animation);
        self.outputs.push(outputs);
    }

    #[inline]
    pub fn build(self) -> AnimationRequest {
        AnimationRequest {
            animations: self.animations.into_boxed_slice(),
            outputs: self.outputs.into_boxed_slice(),
        }
    }
}

pub struct ImageRequest {
    pub transition: Transition,
    pub imgs: Box<[Img]>,
    pub outputs: Box<[Box<[String]>]>,
}

pub struct ImageRequestBuilder {
    pub transition: Transition,
    pub imgs: Vec<Img>,
    pub outputs: Vec<Box<[String]>>,
}

impl ImageRequestBuilder {
    #[inline]
    pub fn new(transition: Transition) -> Self {
        Self {
            transition,
            imgs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    #[inline]
    pub fn push(&mut self, img: Img, outputs: Box<[String]>) {
        self.imgs.push(img);
        self.outputs.push(outputs);
    }

    #[inline]
    pub fn build(self) -> ImageRequest {
        ImageRequest {
            transition: self.transition,
            imgs: self.imgs.into_boxed_slice(),
            outputs: self.outputs.into_boxed_slice(),
        }
    }
}

pub enum Request {
    Ping,
    Query,
    Clear(Clear),
    Img(ImageRequest),
    Animation(AnimationRequest),
    Kill,
}

impl Request {
    pub fn send(&self, stream: &OwnedFd) -> Result<(), String> {
        let now = std::time::Instant::now();
        let mut socket_msg = [0u8; 16];
        socket_msg[0..8].copy_from_slice(&match self {
            Request::Ping => 0u64.to_ne_bytes(),
            Request::Query => 1u64.to_ne_bytes(),
            Request::Clear(_) => 2u64.to_ne_bytes(),
            Request::Img(_) => 3u64.to_ne_bytes(),
            Request::Animation(_) => 4u64.to_ne_bytes(),
            Request::Kill => 5u64.to_ne_bytes(),
        });
        let bytes: Vec<u8> = match self {
            Self::Clear(clear) => {
                let mut buf = Vec::with_capacity(64);
                buf.push(clear.outputs.len() as u8); // we assume someone does not have more than
                                                     // 255 monitors. Seems reasonable
                for output in clear.outputs.iter() {
                    buf.extend((output.len() as u32).to_ne_bytes());
                    buf.extend(output.as_bytes());
                }
                buf.extend(clear.color);
                buf
            }
            Self::Img(img) => {
                let ImageRequest {
                    transition,
                    imgs,
                    outputs,
                } = img;

                let mut buf = Vec::with_capacity(imgs[0].img.len() + 1024);
                transition.serialize(&mut buf);
                buf.push(imgs.len() as u8); // we assume someone does not have more than 255

                for (img, output) in imgs.iter().zip(outputs.iter()) {
                    let Img { path, img } = img;
                    buf.extend((path.len() as u32).to_ne_bytes());
                    buf.extend(path.as_bytes());

                    buf.extend((img.len() as u32).to_ne_bytes());
                    buf.extend(img.iter());

                    buf.push(output.len() as u8);
                    for output in output.iter() {
                        buf.extend((output.len() as u32).to_ne_bytes());
                        buf.extend(output.as_bytes());
                    }
                }

                buf
            }
            Self::Animation(anim) => {
                let AnimationRequest {
                    animations,
                    outputs,
                } = anim;
                let mut buf = Vec::with_capacity(1 << 20);
                buf.push(animations.len() as u8);

                for (animation, output) in animations.iter().zip(outputs.iter()) {
                    animation.serialize(&mut buf);
                    buf.push(output.len() as u8);
                    for output in output.iter() {
                        buf.extend((output.len() as u32).to_ne_bytes());
                        buf.extend(output.as_bytes());
                    }
                }

                buf
            }
            _ => vec![],
        };
        println!(
            "Send encode time: {}us, size: {}",
            now.elapsed().as_micros(),
            bytes.len()
        );
        std::thread::scope(|s| {
            if let Self::Animation(AnimationRequest { animations, .. }) = self {
                s.spawn(|| {
                    for animation in animations.iter() {
                        // only store the cache if we aren't reading from stdin
                        if animation.path != "-" {
                            if let Err(e) = cache::store_animation_frames(animation) {
                                eprintln!("Error storing cache for {}: {e}", animation.path);
                            }
                        }
                    }
                });
            }

            let mut ancillary_buf = [0u8; 64];
            let mut ancillary = net::SendAncillaryBuffer::new(&mut ancillary_buf);

            let mmap = if !bytes.is_empty() {
                socket_msg[8..].copy_from_slice(&(bytes.len() as u64).to_ne_bytes());
                let shm_file = create_shm_fd().unwrap();
                let mut mmap = Mmap::new(shm_file, bytes.len());
                mmap.as_mut().copy_from_slice(&bytes);
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
            match net::sendmsg(stream, &[iov], &mut ancillary, net::SendFlags::empty()) {
                Ok(written) => {
                    if written != 16 {
                        return Err("failed to send full length of message in socket!".to_string());
                    }
                }
                Err(e) => return Err(format!("failed to write serialized request: {e}")),
            }

            if let Self::Img(ImageRequest { imgs, outputs, .. }) = self {
                for (Img { path, .. }, outputs) in imgs.iter().zip(outputs.iter()) {
                    for output in outputs.iter() {
                        if let Err(e) = super::cache::store(output, path) {
                            eprintln!("ERROR: failed to store cache: {e}");
                        }
                    }
                }
            }
            Ok(())
        })
    }

    #[must_use]
    #[inline]
    pub fn receive(bytes: &[u8]) -> Self {
        let now = std::time::Instant::now();
        let ret = match bytes[0] {
            0 => Self::Ping,
            1 => Self::Query,
            2 => {
                let len = bytes[1] as usize;
                let mut outputs = Vec::with_capacity(len);
                let mut i = 2;
                for _ in 0..len {
                    let str_size =
                        unsafe { bytes.as_ptr().add(i).cast::<u32>().read_unaligned() } as usize;
                    i += 4;
                    outputs.push(
                        std::str::from_utf8(&bytes[i..i + str_size])
                            .unwrap()
                            .to_string(),
                    );
                    i += str_size;
                }
                let color = [bytes[i], bytes[i + 1], bytes[i + 2]];
                Self::Clear(Clear {
                    color,
                    outputs: outputs.into(),
                })
            }
            3 => {
                let transition = Transition::deserialize(&bytes[1..]);
                let len = bytes[52] as usize;

                let mut imgs = Vec::with_capacity(len);
                let mut outputs = Vec::with_capacity(len);

                let mut i = 53;
                for _ in 0..len {
                    let path_size =
                        unsafe { bytes.as_ptr().add(i).cast::<u32>().read_unaligned() } as usize;
                    i += 4;
                    let path = std::str::from_utf8(&bytes[i..i + path_size])
                        .unwrap()
                        .to_string();
                    i += path_size;

                    let img_size =
                        unsafe { bytes.as_ptr().add(i).cast::<u32>().read_unaligned() } as usize;
                    i += 4;
                    let img = bytes[i..i + img_size].into();
                    i += img_size;

                    imgs.push(Img { path, img });

                    let n_outputs = bytes[i] as usize;
                    i += 1;
                    let mut out = Vec::with_capacity(n_outputs);
                    for _ in 0..n_outputs {
                        let str_size =
                            unsafe { bytes.as_ptr().add(i).cast::<u32>().read_unaligned() }
                                as usize;
                        i += 4;
                        out.push(
                            std::str::from_utf8(&bytes[i..i + str_size])
                                .unwrap()
                                .to_string(),
                        );
                        i += str_size;
                    }
                    outputs.push(out.into());
                }

                Self::Img(ImageRequest {
                    transition,
                    imgs: imgs.into(),
                    outputs: outputs.into(),
                })
            }
            4 => {
                let len = bytes[1] as usize;
                let mut animations = Vec::with_capacity(len);
                let mut outputs = Vec::with_capacity(len);

                let mut i = 2;
                for _ in 0..len {
                    let (animation, offset) = Animation::deserialize(&bytes[i..]);
                    i += offset;
                    animations.push(animation);
                    let n_outputs = bytes[i] as usize;
                    i += 1;
                    let mut out = Vec::with_capacity(n_outputs);
                    for _ in 0..n_outputs {
                        let str_size =
                            unsafe { bytes.as_ptr().add(i).cast::<u32>().read_unaligned() }
                                as usize;
                        i += 4;
                        out.push(
                            std::str::from_utf8(&bytes[i..i + str_size])
                                .unwrap()
                                .to_string(),
                        );
                        i += str_size;
                    }
                    outputs.push(out.into());
                }

                Self::Animation(AnimationRequest {
                    animations: animations.into(),
                    outputs: outputs.into(),
                })
            }
            _ => Self::Kill,
        };
        println!("Receive decode time: {}us", now.elapsed().as_micros());
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
                buf.extend((s.len() as u32).to_ne_bytes());
                buf.extend(s.as_bytes());
                buf
            }
            _ => vec![],
        };

        let mut ancillary_buf = [0u8; 64];
        let mut ancillary = net::SendAncillaryBuffer::new(&mut ancillary_buf);

        let mmap = if !bytes.is_empty() {
            socket_msg[8..].copy_from_slice(&(bytes.len() as u64).to_ne_bytes());
            let shm_file = create_shm_fd().unwrap();
            let mut mmap = Mmap::new(shm_file, bytes.len());
            mmap.as_mut().copy_from_slice(&bytes);
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
        match net::sendmsg(stream, &[iov], &mut ancillary, net::SendFlags::empty()) {
            Ok(written) => {
                if written != 16 {
                    return Err("failed to send full length of message in socket!".to_string());
                }
            }
            Err(e) => return Err(format!("failed to write serialized request: {e}")),
        }
        Ok(())
    }

    #[must_use]
    #[inline]
    pub fn receive(bytes: &[u8]) -> Self {
        match bytes[0] {
            0 => Self::Ok,
            1 => Self::Ping(true),
            2 => Self::Ping(false),
            3 => {
                let len = bytes[1] as usize;
                let mut bg_infos = Vec::with_capacity(len);

                let mut i = 2;
                for _ in 0..len {
                    let (info, offset) = BgInfo::deserialize(&bytes[i..]);
                    i += offset;
                    bg_infos.push(info);
                }

                Self::Info(bg_infos.into())
            }
            4 => {
                let err_size =
                    unsafe { bytes.as_ptr().add(1).cast::<u32>().read_unaligned() } as usize;
                let err = std::str::from_utf8(&bytes[5..5 + err_size])
                    .unwrap()
                    .to_string();
                Self::Err(err)
            }
            _ => panic!("Received malformed answer from daemon"),
        }
    }
}

pub fn read_socket(stream: &OwnedFd) -> Result<Vec<u8>, String> {
    let mut buf = [0; 16];
    let mut ancillary_buf = [0; 64];

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

    if len == 0 {
        Ok(vec![code])
    } else {
        let mut v = Vec::with_capacity(len + 1);
        v.push(code);
        let shm_file = match control.drain().next().unwrap() {
            net::RecvAncillaryMessage::ScmRights(mut iter) => iter.next().unwrap(),
            _ => panic!("malformed ancillary message"),
        };

        let mut mmap = Mmap::new(shm_file, len);
        v.extend_from_slice(mmap.as_mut());
        Ok(v)
    }
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

pub fn get_cache_path() -> Result<PathBuf, String> {
    let cache_path = match std::env::var("XDG_CACHE_HOME") {
        Ok(dir) => {
            let mut cache = PathBuf::from(dir);
            cache.push("swww");
            cache
        }
        Err(_) => match std::env::var("HOME") {
            Ok(dir) => {
                let mut cache = PathBuf::from(dir);
                cache.push(".cache/swww");
                cache
            }
            Err(_) => return Err("failed to read both XDG_CACHE_HOME and HOME env vars".to_owned()),
        },
    };

    if !cache_path.is_dir() {
        if let Err(e) = std::fs::create_dir(&cache_path) {
            return Err(format!(
                "failed to create cache_path \"{}\": {e}",
                cache_path.display()
            ));
        }
    }

    Ok(cache_path)
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

#[derive(Debug)]
struct Mmap {
    fd: OwnedFd,
    ptr: *mut std::ffi::c_void,
    len: usize,
}

impl Mmap {
    const PROT: ProtFlags = ProtFlags::WRITE.union(ProtFlags::READ);
    const FLAGS: MapFlags = MapFlags::SHARED;

    fn new(fd: OwnedFd, len: usize) -> Self {
        loop {
            match rustix::fs::ftruncate(&fd, len as u64) {
                Err(Errno::INTR) => continue,
                otherwise => break otherwise.unwrap(),
            }
        }

        let ptr =
            unsafe { mmap(std::ptr::null_mut(), len, Self::PROT, Self::FLAGS, &fd, 0).unwrap() };
        Self { fd, ptr, len }
    }

    fn as_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.cast(), self.len) }
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        if let Err(e) = unsafe { munmap(self.ptr, self.len) } {
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
                    "/swww-daemon-{}",
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

    let name = CStr::from_bytes_with_nul(b"swww-daemon\0").unwrap();
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
