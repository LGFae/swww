use std::num::NonZeroI32;
use std::num::NonZeroU8;
use std::time::Duration;

pub struct Vec2<T> {
    pub x: T,
    pub y: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

pub struct Image<'a> {
    pub dim: Vec2<u32>,
    pub format: PixelFormat,
    pub img: &'a [u8],
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TransitionType {
    Simple = 0,
    Fade = 1,
    Outer = 2,
    Wipe = 3,
    Grow = 4,
    Wave = 5,
    None = 6,
}

#[derive(Clone, PartialEq)]
pub enum Coord {
    Pixel(f32),
    Percent(f32),
}

pub struct Transition {
    pub transition_type: TransitionType,
    pub duration: f32,
    pub step: NonZeroU8,
    pub fps: u16,
    pub angle: f64,
    pub pos: Vec2<Coord>,
    pub bezier: (Vec2<f32>, Vec2<f32>),
    pub wave: Vec2<f32>,
    pub invert_y: bool,
}

pub struct BitPack<'a> {
    pub bytes: &'a [u8],
    pub expected_size: u32,
}

pub struct Animation<'a> {
    pub animation: Box<[(BitPack<'a>, Duration)]>,
}

pub struct ImageRequest<'a> {
    pub transition: Transition,
    pub imgs: Box<[Image<'a>]>,
    pub outputs: Box<[&'a str]>,
    pub animations: Option<Box<[Animation<'a>]>>,
}

pub struct ClearRequest<'a> {
    pub color: [u8; 3],
    pub outputs: Box<[&'a str]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scale {
    Whole(NonZeroI32),
    Fractional(NonZeroI32),
}

#[derive(Debug, PartialEq, Clone)]
pub enum ImageDescription {
    Color([u8; 3]),
    Img(String),
}

pub struct Info {
    pub name: String,
    pub dim: Vec2<u32>,
    pub scale: Scale,
    pub img: ImageDescription,
    pub format: PixelFormat,
}

pub enum Request<'a> {
    Ping,
    Query,
    Clear(ClearRequest<'a>),
    Img(ImageRequest<'a>),
    Kill,
}

// TODO: perhaps add error propagation
pub enum Response {
    Ok,
    Ping(bool),
    Info(Box<[Info]>),
}
