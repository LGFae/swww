use rkyv::{Archive, Deserialize, Serialize};
use std::{
    fmt,
    io::{BufReader, BufWriter, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{cache, comp_decomp::BitPack};

#[derive(PartialEq, Archive, Serialize)]
#[archive_attr(derive(Clone))]
pub enum Coord {
    Pixel(f32),
    Percent(f32),
}

#[derive(PartialEq, Archive, Serialize)]
#[archive_attr(derive(Clone))]
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

impl ArchivedPosition {
    #[must_use]
    pub fn to_pixel(&self, dim: (u32, u32), invert_y: bool) -> (f32, f32) {
        let x = match self.x {
            ArchivedCoord::Pixel(x) => x,
            ArchivedCoord::Percent(x) => x * dim.0 as f32,
        };

        let y = match self.y {
            ArchivedCoord::Pixel(y) => {
                if invert_y {
                    dim.1 as f32 - y
                } else {
                    y
                }
            }
            ArchivedCoord::Percent(y) => {
                if invert_y {
                    (1.0 - y) * dim.1 as f32
                } else {
                    y * dim.1 as f32
                }
            }
        };

        (x, y)
    }
}

#[derive(PartialEq, Clone, Archive, Serialize, Deserialize)]
#[archive_attr(derive(PartialEq))]
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

impl ArchivedBgImg {
    /// Deserialized the archived bg img
    #[must_use]
    pub fn de(&self) -> BgImg {
        self.deserialize(&mut rkyv::Infallible).unwrap()
    }
}

impl fmt::Display for ArchivedBgImg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArchivedBgImg::Color(color) => {
                write!(f, "color: {:02X}{:02X}{:02X}", color[0], color[1], color[2])
            }
            ArchivedBgImg::Img(p) => write!(f, "image: {p}",),
        }
    }
}

#[derive(Clone, Archive, Serialize)]
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

impl fmt::Display for ArchivedBgInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}x{}, scale: {}, currently displaying: {}",
            self.name, self.dim.0, self.dim.1, self.scale_factor, self.img
        )
    }
}

#[derive(Archive, Serialize)]
#[archive_attr(derive(Clone))]
pub enum TransitionType {
    Simple,
    Fade,
    Outer,
    Wipe,
    Grow,
    Wave,
}

#[derive(Archive, Serialize)]
#[archive_attr(derive(Clone))]
pub struct Transition {
    pub transition_type: TransitionType,
    pub duration: f32,
    pub step: u8,
    pub fps: u8,
    pub angle: f64,
    pub pos: Position,
    pub bezier: (f32, f32, f32, f32),
    pub wave: (f32, f32),
    pub invert_y: bool,
}

#[derive(Archive, Serialize)]
pub struct Clear {
    pub color: [u8; 3],
    pub outputs: Box<[String]>,
}

#[derive(Archive, Serialize)]
pub struct Img {
    pub path: String,
    pub img: Box<[u8]>,
}

#[derive(Archive, Serialize, Deserialize)]
pub struct Animation {
    pub animation: Box<[(BitPack, Duration)]>,
    pub path: String,
    pub dimensions: (u32, u32),
}

pub type AnimationRequest = Box<[(Animation, Box<[String]>)]>;
pub type ImageRequest = (Transition, Box<[(Img, Box<[String]>)]>);

#[derive(Archive, Serialize)]
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
        let bytes = match rkyv::to_bytes::<_, 1024>(self) {
            Ok(bytes) => bytes,
            Err(e) => return Err(format!("Failed to serialize request: {e}")),
        };

        std::thread::scope(|s| {
            if let Self::Animation(animations) = self {
                s.spawn(|| {
                    for (animation, _) in animations.iter() {
                        if let Err(e) = cache::store_animation_frames(animation) {
                            eprintln!("Error storing cache for {}: {e}", animation.path);
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
                if let Self::Img((_, imgs)) = self {
                    for (Img { path, .. }, outputs) in imgs.iter() {
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
    pub fn receive(bytes: &[u8]) -> &ArchivedRequest {
        unsafe { rkyv::archived_root::<Self>(bytes) }
    }
}

#[derive(Archive, Serialize)]
pub enum Answer {
    Ok,
    Err(String),
    Info(Box<[BgInfo]>),
    Init(bool),
}

impl Answer {
    pub fn send(&self, stream: &UnixStream) -> Result<(), String> {
        let bytes = match rkyv::to_bytes::<_, 256>(self) {
            Ok(bytes) => bytes,
            Err(e) => return Err(format!("Failed to serialize answer: {e}")),
        };
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
    pub fn receive(bytes: &[u8]) -> &ArchivedAnswer {
        unsafe { rkyv::archived_root::<Self>(bytes) }
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
    let runtime_dir = Path::new(&runtime_dir);
    runtime_dir.join("swww.socket")
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
