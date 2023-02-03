use serde::{Deserialize, Serialize};
use std::{
    fmt,
    io::{BufReader, BufWriter},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::comp_decomp::BitPack;

#[derive(PartialEq, Clone, Serialize, Deserialize,Debug)]
pub enum Position {
    Pixel(f32,f32),
    Percent(f32,f32),
}

impl Position {
    pub fn to_pixel(&self, dim: (u32, u32)) -> (f32, f32) {
        match self {
            Position::Pixel(x, y) => (*x as f32, *y as f32),
            Position::Percent(x, y) => (dim.0 as f32 * x, dim.1 as f32 * y),
        }
    }

    pub fn to_percent(&self, dim: (u32, u32)) -> (f32, f32) {
        match self {
            Position::Pixel(x, y) => (x / dim.0 as f32, y / dim.1 as f32),
            Position::Percent(x, y) => (*x as f32, *y as f32),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Serialize, Deserialize)]
pub enum BgImg {
    Color([u8; 3]),
    Img(PathBuf),
}

impl fmt::Display for BgImg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BgImg::Color(color) => write!(f, "color: {}{}{}", color[0], color[1], color[2]),
            BgImg::Img(p) => write!(
                f,
                "image: {:#?}",
                p.file_name().unwrap_or_else(|| std::ffi::OsStr::new("?"))
            ),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BgInfo {
    pub name: String,
    pub dim: (u32, u32),
    pub scale_factor: i32,
    pub img: BgImg,
}

impl BgInfo {
    #[must_use]
    pub fn real_dim(&self) -> (u32, u32) {
        (
            self.dim.0 * self.scale_factor as u32,
            self.dim.1 * self.scale_factor as u32,
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

#[derive(Serialize, Deserialize)]
pub struct Clear {
    pub color: [u8; 3],
    pub outputs: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Animation {
    pub animation: Box<[(BitPack, Duration)]>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum TransitionType {
    Simple,
    Outer,
    Wipe,
    Grow,
    Wave,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Transition {
    pub transition_type: TransitionType,
    pub duration: f32,
    pub step: u8,
    pub fps: u8,
    pub angle: f64,
    pub pos: Position,
    pub bezier: (f32, f32, f32, f32),
    pub wave: (f32, f32),
}

#[derive(Serialize, Deserialize)]
pub struct Img {
    pub path: PathBuf,
    pub img: Vec<u8>,
}

pub type AnimationRequest = Vec<(Animation, Vec<String>)>;
pub type ImageRequest = (Transition, Vec<(Img, Vec<String>)>);

#[derive(Serialize, Deserialize)]
pub enum Request {
    Animation(AnimationRequest),
    Clear(Clear),
    Init,
    Kill,
    Query,
    Img(ImageRequest),
}

impl Request {
    pub fn send(&mut self, stream: &UnixStream) -> Result<(), String> {
        let writer = BufWriter::new(stream);
        match bincode::serialize_into(writer, self) {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("Failed to serialize request: {e}")),
        }
    }

    pub fn receive(stream: &mut UnixStream) -> Result<Self, String> {
        let reader = BufReader::new(stream);
        match bincode::deserialize_from(reader) {
            Ok(i) => Ok(i),
            Err(e) => Err(format!("Failed to deserialize request: {e}")),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum Answer {
    Ok,
    Err(String),
    Info(Vec<BgInfo>),
}

impl Answer {
    pub fn send(&self, stream: &UnixStream) -> Result<(), String> {
        match bincode::serialize_into(stream, self) {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("Failed to send answer: {e}")),
        }
    }

    pub fn receive(stream: UnixStream) -> Result<Self, String> {
        #[cfg(debug_assertions)]
        let timeout = Duration::from_secs(30); //Some operations take a while to respond in debug mode
        #[cfg(not(debug_assertions))]
        let timeout = Duration::from_secs(5);

        if let Err(e) = stream.set_read_timeout(Some(timeout)) {
            return Err(format!("Failed to set read timeout: {e}"));
        };

        match bincode::deserialize_from(stream) {
            Ok(i) => Ok(i),
            Err(e) => Err(format!("Failed to receive answer: {e}")),
        }
    }
}

#[must_use]
pub fn get_socket_path() -> PathBuf {
    let runtime_dir = if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        dir
    } else {
        "/tmp/swww".to_string()
    };
    let runtime_dir = Path::new(&runtime_dir);
    runtime_dir.join("swww.socket")
}
