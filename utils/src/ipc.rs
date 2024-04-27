use bitcode::{Decode, Encode};
use std::{
    fmt,
    io::{BufReader, BufWriter, Read, Write},
    num::{NonZeroI32, NonZeroU8},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

use crate::{cache, compression::BitPack};

#[derive(Clone, PartialEq, Decode, Encode)]
pub enum Coord {
    Pixel(f32),
    Percent(f32),
}

#[derive(Clone, PartialEq, Decode, Encode)]
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

#[derive(Debug, PartialEq, Clone, Encode, Decode)]
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

#[derive(Clone, Copy, Debug, Encode, Decode, PartialEq)]
pub enum PixelFormat {
    /// No swap, can copy directly onto WlBuffer
    Bgr,
    /// Swap R and B channels at client, can copy directly onto WlBuffer
    Rgb,
    /// No swap, must extend pixel with an extra byte when copying
    Xbgr,
    /// Swap R and B channels at client, must extend pixel with an extra byte when copying
    Xrgb,
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

#[derive(Clone, Copy, Debug, Decode, Encode, PartialEq)]
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

#[derive(Clone, Decode, Encode)]
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

#[derive(Clone, Copy, Decode, Encode)]
pub enum TransitionType {
    None,
    Simple,
    Fade,
    Outer,
    Wipe,
    Grow,
    Wave,
}

#[derive(Decode, Encode)]
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

#[derive(Decode, Encode)]
pub struct Clear {
    pub color: [u8; 3],
    pub outputs: Box<[String]>,
}

#[derive(Decode, Encode)]
pub struct Img {
    pub path: String,
    pub img: Box<[u8]>,
}

#[derive(Decode, Encode)]
pub struct Animation {
    pub animation: Box<[(BitPack, Duration)]>,
    pub path: String,
    pub dimensions: (u32, u32),
    pub pixel_format: PixelFormat,
}

#[derive(Decode, Encode)]
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

#[derive(Decode, Encode)]
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

#[derive(Decode, Encode)]
pub enum Request {
    Animation(AnimationRequest),
    Clear(Clear),
    Ping,
    Kill,
    Query,
    Img(ImageRequest),
}

impl Request {
    pub fn send(&self, stream: &UnixStream) -> Result<(), String> {
        let bytes = bitcode::encode(self);
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
            let mut writer = BufWriter::new(stream);
            if let Err(e) = writer.write_all(&bytes.len().to_ne_bytes()) {
                return Err(format!("failed to write serialized request's length: {e}"));
            }
            if let Err(e) = writer.write_all(&bytes) {
                Err(format!("failed to write serialized request: {e}"))
            } else {
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
            }
        })
    }

    #[must_use]
    #[inline]
    pub fn receive(bytes: &[u8]) -> Self {
        bitcode::decode(bytes).expect("failed to decode request")
    }
}

#[derive(Decode, Encode)]
pub enum Answer {
    Ok,
    Err(String),
    Info(Box<[BgInfo]>),
    Ping(bool),
}

impl Answer {
    pub fn send(&self, stream: &UnixStream) -> Result<(), String> {
        let bytes = bitcode::encode(self);
        let mut writer = BufWriter::new(stream);
        if let Err(e) = writer.write_all(&bytes.len().to_ne_bytes()) {
            return Err(format!("failed to write serialized answer's length: {e}"));
        }
        if let Err(e) = writer.write_all(&bytes) {
            Err(format!("Failed to write serialized answer: {e}"))
        } else {
            Ok(())
        }
    }

    #[must_use]
    #[inline]
    pub fn receive(bytes: &[u8]) -> Self {
        bitcode::decode(bytes).expect("failed to decode answer")
    }
}

pub fn read_socket(stream: &UnixStream) -> Result<Vec<u8>, String> {
    let mut reader = BufReader::new(stream);
    let mut buf = vec![0; 8];

    let mut tries = 0;
    loop {
        match reader.read_exact(&mut buf[0..std::mem::size_of::<usize>()]) {
            Ok(()) => break,
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
    let len = usize::from_ne_bytes(buf[0..std::mem::size_of::<usize>()].try_into().unwrap());
    buf.clear();
    buf.resize(len, 0);

    if let Err(e) = reader.read_exact(&mut buf) {
        return Err(format!("Failed to read request: {e}"));
    }
    Ok(buf)
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
pub fn connect_to_socket(addr: &PathBuf, tries: u8, interval: u64) -> Result<UnixStream, String> {
    //Make sure we try at least once
    let tries = if tries == 0 { 1 } else { tries };
    let mut error = None;
    for _ in 0..tries {
        match UnixStream::connect(addr) {
            Ok(socket) => {
                if let Err(e) = socket.set_nonblocking(false) {
                    return Err(format!("Failed to set blocking connection: {e}"));
                }
                #[cfg(debug_assertions)]
                let timeout = Duration::from_secs(30); //Some operations take a while to respond in debug mode
                #[cfg(not(debug_assertions))]
                let timeout = Duration::from_secs(5);

                if let Err(e) = socket.set_read_timeout(Some(timeout)) {
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
