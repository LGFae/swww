use serde::{Deserialize, Serialize};
use std::{
    fmt,
    fs::File,
    io::{BufReader, BufWriter},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::comp_decomp::BitPack;

#[derive(PartialEq, Clone, Serialize, Deserialize, Debug)]
pub enum Coord {
    Pixel(f32),
    Percent(f32),
}

#[derive(PartialEq, Clone, Serialize, Deserialize, Debug)]
pub struct Position {
    pub x: Coord,
    pub y: Coord,
}

impl Position {
    pub fn new(x: Coord, y: Coord) -> Self {
        Self { x, y }
    }

    pub fn to_pixel(&self, dim: (u32, u32)) -> (f32, f32) {
        let x = match self.x {
            Coord::Pixel(x) => x,
            Coord::Percent(x) => x * dim.0 as f32,
        };

        let y = match self.y {
            Coord::Pixel(y) => y,
            Coord::Percent(y) => y * dim.1 as f32,
        };

        (x, y)
    }

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
pub struct Clear {
    pub color: [u8; 3],
    pub outputs: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Img {
    pub path: PathBuf,
    pub img: Vec<u8>,
}

impl TryFrom<&mut BufReader<File>> for Img {
    type Error = String;
    fn try_from(file_reader: &mut BufReader<File>) -> Result<Self, Self::Error> {
        match bincode::deserialize_from(file_reader) {
            Ok(i) => Ok(i),
            Err(e) => Err(format!("Failed to deserialize request: {e}")),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Animation {
    pub animation: Box<[(BitPack, Duration)]>,
    pub sync: bool,
}

impl TryFrom<&mut BufReader<File>> for Animation {
    type Error = String;
    fn try_from(file_reader: &mut BufReader<File>) -> Result<Self, Self::Error> {
        match bincode::deserialize_from(file_reader) {
            Ok(i) => Ok(i),
            Err(e) => Err(format!("Failed to deserialize request: {e}")),
        }
    }
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
    pub fn send(&self, stream: &UnixStream) -> Result<(), String> {
        let writer = BufWriter::new(stream);
        std::thread::scope(|s| {
            let serializer = s.spawn(|| match bincode::serialize_into(writer, self) {
                Ok(()) => Ok(()),
                Err(e) => Err(format!("Failed to serialize request: {e}")),
            });

            match self {
                Request::Animation(animations) => match get_cache_path() {
                    Ok(cache_path) => {
                        s.spawn(move || Self::cache_animations(animations, cache_path));
                    }
                    Err(e) => eprintln!("failed to get cache path: {e}"),
                },
                Request::Img((_, images)) => match get_cache_path() {
                    Ok(cache_path) => {
                        s.spawn(move || Self::cache_images(images, cache_path));
                    }
                    Err(e) => eprintln!("failed to get cache path: {e}"),
                },
                _ => (),
            };

            match serializer.join() {
                Ok(result) => result,
                Err(e) => Err(format!("{e:?}")),
            }
        })
    }

    pub fn receive(stream: &UnixStream) -> Result<Self, String> {
        let reader = BufReader::new(stream);
        match bincode::deserialize_from(reader) {
            Ok(i) => Ok(i),
            Err(e) => Err(format!("Failed to deserialize request: {e}")),
        }
    }

    fn cache_images(images: &[(Img, Vec<String>)], mut cache_path: PathBuf) {
        for (img, outputs) in images {
            for output in outputs {
                cache_path.push(output);
                match File::create(&cache_path) {
                    Ok(file) => {
                        let writer = BufWriter::new(file);
                        if let Err(e) = bincode::serialize_into(writer, img) {
                            eprintln!(
                                "failed to serialize image into cache file '{cache_path:?}': {e}"
                            )
                        }
                    }
                    Err(e) => eprintln!("failed to create cache file '{cache_path:?}': {e}"),
                }
                cache_path.pop();
            }
        }
    }

    fn cache_animations(animations: &[(Animation, Vec<String>)], mut cache_path: PathBuf) {
        for (animation, outputs) in animations {
            for output in outputs {
                cache_path.push(output);
                match File::options().append(true).open(&cache_path) {
                    Ok(file) => {
                        let writer = BufWriter::new(file);
                        if let Err(e) = bincode::serialize_into(writer, animation) {
                            eprintln!("failed to serialize animation into cache file '{cache_path:?}': {e}")
                        }
                    }
                    Err(e) => {
                        eprintln!("failed to append animation to cache file '{cache_path:?}': {e}")
                    }
                }
                cache_path.pop();
            }
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

pub fn get_cache_path() -> Result<PathBuf, String> {
    let cache_dir = match std::env::var("XDG_CACHE_HOME") {
        Ok(dir) => dir + "/swww",
        Err(_) => match std::env::var("HOME") {
            Ok(dir) => dir + ".cache/swww",
            Err(_) => return Err("failed to read both XDG_CACHE_HOME and HOME env vars".to_owned()),
        },
    };

    let cache_path = PathBuf::from(&cache_dir);
    if !cache_path.exists() {
        if let Err(e) = std::fs::create_dir(&cache_path) {
            return Err(format!("failed to create cache_path: {e}"));
        }
    }

    Ok(cache_path)
}
