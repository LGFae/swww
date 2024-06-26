use std::mem::size_of;
use std::mem::size_of_val;
use std::num::NonZeroI32;
use std::num::NonZeroU8;
use std::time::Duration;

use super::types2::Animation;
use super::types2::BitPack;
use super::types2::ClearRequest;
use super::types2::Coord;
use super::types2::Image;
use super::types2::ImageDescription;
use super::types2::ImageRequest;
use super::types2::Info;
use super::types2::PixelFormat;
use super::types2::Scale;
use super::types2::Transition;
use super::types2::TransitionType;
use super::types2::Vec2;

/// Type for managing access in [`Serialize`] and [`Deserialize`]
pub struct Cursor<T> {
    buf: T,
    pos: usize,
}

impl<T> Cursor<T> {
    /// Create [`Self`] from provided buffer
    pub fn new(buf: T) -> Self {
        Self { buf, pos: 0 }
    }

    /// Extract buffer back, discarding all internal state.
    pub fn finish(self) -> T {
        self.buf
    }
}

impl<'a> Cursor<&'a mut [u8]> {
    fn write(&mut self, bytes: &[u8]) {
        let next = self.pos + bytes.len();
        self.buf[self.pos..next].copy_from_slice(bytes);
        self.pos = next;
    }

    fn write_tagged(&mut self, bytes: &[u8]) {
        bytes.tag().serialize(self);
        self.write(bytes);
    }
}

impl<'a> Cursor<&'a [u8]> {
    fn read(&mut self, count: usize) -> &'a [u8] {
        let read = &self.buf[self.pos..self.pos + count];
        self.pos += count;
        read
    }

    fn read_tagged(&mut self) -> &'a [u8] {
        let count = u32::deserialize(self);
        self.read(count as usize)
    }
}

trait Tagged<'a> {
    fn tag(&self) -> u32;
}

impl<T> Tagged<'_> for [T] {
    fn tag(&self) -> u32 {
        self.len() as u32
    }
}

impl Tagged<'_> for str {
    fn tag(&self) -> u32 {
        self.as_bytes().tag()
    }
}

/// Serializes data structure into byte slice.
pub trait Serialize {
    /// Write self into buffer.
    ///
    /// # Panics
    ///
    /// If [`size`](Serialize::size) is incorrectly implemented, buffer can be
    /// too small to fit all bytes.
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>);

    /// Calculate resulting size of serialized data structure.
    ///
    /// Resulting value will be used to allocate buffer, so incorrect implementation
    /// will either result in panic after out of bounds access or wasted space.
    fn size(&self) -> usize;

    /// Same as [`size`](Serialize::size), but with space for slice length (now [`u32`])
    #[doc(hidden)]
    fn size_tagged(&self) -> usize {
        size_of::<u32>() + self.size()
    }
}

/// Deserializes byte slice into desired data structure.
pub trait Deserialize<'a> {
    /// Read bytes from buffer and interpret as desired.
    ///
    /// # Panics
    ///
    /// If provided buffer is messed up (i.e. it isn't filled by corresponding
    /// [`Serialize`] implementation).
    ///
    /// # Safety
    ///
    /// This function isn't marked as unsafe, so in no circumstances can
    /// implementations count on provided buffer to be well formed. Panic instead.
    fn deserialize(buffer: &mut Cursor<&'a [u8]>) -> Self;
}

macro_rules! primitive {
    ($($type:ty),+ $(,)?) => {
        $(
        impl Serialize for $type {
            fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
                self.to_ne_bytes().serialize(buffer)
            }

            fn size(&self) -> usize {
                size_of::<$type>()
            }
        }

        impl Deserialize<'_> for $type {
            fn deserialize(buffer: &mut Cursor<&[u8]>) -> Self {
                let array = buffer.read(size_of::<$type>()).try_into().expect("slice is correctly sized");
                Self::from_ne_bytes(array)
            }
        }
        )+
    };
}

primitive!(u8, u16, u32, u64, u128, usize);
primitive!(i8, i16, i32, i64, i128, isize);
primitive!(f32, f64);

impl Serialize for [u8] {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        buffer.write(self);
    }

    fn size(&self) -> usize {
        size_of_val(self)
    }
}

impl Serialize for str {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        self.as_bytes().serialize(buffer)
    }

    fn size(&self) -> usize {
        size_of_val(self)
    }
}

impl<T: Serialize> Serialize for Vec2<T> {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        self.x.serialize(buffer);
        self.y.serialize(buffer);
    }

    fn size(&self) -> usize {
        2 * size_of::<T>()
    }
}

impl<T: Serialize> Serialize for Box<[T]> {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        self.tag().serialize(buffer);
        for item in self.iter() {
            item.serialize(buffer);
        }
    }

    fn size(&self) -> usize {
        self.tag().size() + self.iter().map(T::size).sum::<usize>()
    }
}

impl<'a, T: Deserialize<'a>> Deserialize<'a> for Box<[T]> {
    fn deserialize(buffer: &mut Cursor<&'a [u8]>) -> Self {
        (0..u32::deserialize(buffer))
            .map(|_| T::deserialize(buffer))
            .collect()
    }
}

impl<'a, T: Deserialize<'a>> Deserialize<'a> for Vec2<T> {
    fn deserialize(buffer: &mut Cursor<&'a [u8]>) -> Self {
        let x = T::deserialize(buffer);
        let y = T::deserialize(buffer);
        Self { x, y }
    }
}

impl Serialize for PixelFormat {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        (*self as u8).serialize(buffer)
    }

    fn size(&self) -> usize {
        size_of_val(self)
    }
}

impl Deserialize<'_> for PixelFormat {
    fn deserialize(buffer: &mut Cursor<&[u8]>) -> Self {
        match u8::deserialize(buffer) {
            num if num == Self::Bgr as u8 => Self::Bgr,
            num if num == Self::Rgb as u8 => Self::Rgb,
            num if num == Self::Xbgr as u8 => Self::Xbgr,
            num if num == Self::Xrgb as u8 => Self::Xrgb,
            _ => unreachable!("invalid discriminant"),
        }
    }
}

impl Serialize for Image<'_> {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        self.dim.serialize(buffer);
        self.format.serialize(buffer);
        buffer.write_tagged(self.img);
    }

    fn size(&self) -> usize {
        self.dim.size() + self.format.size() + self.img.size_tagged()
    }
}

impl<'a> Deserialize<'a> for Image<'a> {
    fn deserialize(buffer: &mut Cursor<&'a [u8]>) -> Self {
        let dim = Vec2::<u32>::deserialize(buffer);
        let format = PixelFormat::deserialize(buffer);
        let img = buffer.read_tagged();
        Self { dim, format, img }
    }
}

impl Serialize for TransitionType {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        (*self as u8).serialize(buffer)
    }

    fn size(&self) -> usize {
        size_of_val(self)
    }
}

impl Deserialize<'_> for TransitionType {
    fn deserialize(buffer: &mut Cursor<&[u8]>) -> Self {
        match u8::deserialize(buffer) {
            num if num == Self::Simple as u8 => Self::Simple,
            num if num == Self::Fade as u8 => Self::Fade,
            num if num == Self::Outer as u8 => Self::Outer,
            num if num == Self::Wipe as u8 => Self::Wipe,
            num if num == Self::Grow as u8 => Self::Grow,
            num if num == Self::Wave as u8 => Self::Wave,
            num if num == Self::None as u8 => Self::None,
            _ => unreachable!("invalid discriminant"),
        }
    }
}

impl Serialize for Coord {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        match self {
            Self::Pixel(pixel) => {
                0u8.serialize(buffer);
                pixel.serialize(buffer);
            }
            Self::Percent(percent) => {
                1u8.serialize(buffer);
                percent.serialize(buffer);
            }
        }
    }

    fn size(&self) -> usize {
        size_of_val(self)
    }
}

impl Deserialize<'_> for Coord {
    fn deserialize(buffer: &mut Cursor<&[u8]>) -> Self {
        match u8::deserialize(buffer) {
            0u8 => Self::Pixel(f32::deserialize(buffer)),
            1u8 => Self::Percent(f32::deserialize(buffer)),
            _ => unreachable!("invalid discriminant"),
        }
    }
}

impl Serialize for Transition {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        self.transition_type.serialize(buffer);
        self.duration.serialize(buffer);
        self.step.get().serialize(buffer);
        self.fps.serialize(buffer);
        self.angle.serialize(buffer);
        self.pos.serialize(buffer);
        self.bezier.0.serialize(buffer);
        self.bezier.1.serialize(buffer);
        self.wave.serialize(buffer);
        u8::from(self.invert_y).serialize(buffer);
    }

    fn size(&self) -> usize {
        self.transition_type.size()
            + self.duration.size()
            + self.step.get().size()
            + self.fps.size()
            + self.angle.size()
            + self.pos.size()
            + self.bezier.0.size()
            + self.bezier.1.size()
            + self.wave.size()
            + u8::from(self.invert_y).size()
    }
}

impl Deserialize<'_> for Transition {
    fn deserialize(buffer: &mut Cursor<&[u8]>) -> Self {
        let transition_type = TransitionType::deserialize(buffer);
        let duration = f32::deserialize(buffer);
        let step = NonZeroU8::new(u8::deserialize(buffer)).unwrap();
        let fps = u16::deserialize(buffer);
        let angle = f64::deserialize(buffer);
        let pos = Vec2::<Coord>::deserialize(buffer);
        let bezier = (
            Vec2::<f32>::deserialize(buffer),
            Vec2::<f32>::deserialize(buffer),
        );
        let wave = Vec2::<f32>::deserialize(buffer);
        let invert_y = match u8::deserialize(buffer) {
            0 => false,
            1 => true,
            _ => unreachable!("`bool` has only two valid values"),
        };
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

impl Serialize for BitPack<'_> {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        buffer.write_tagged(self.bytes);
        self.expected_size.serialize(buffer);
    }

    fn size(&self) -> usize {
        self.bytes.size_tagged() + self.expected_size.size()
    }
}

impl<'a> Deserialize<'a> for BitPack<'a> {
    fn deserialize(buffer: &mut Cursor<&'a [u8]>) -> Self {
        let bytes = buffer.read_tagged();
        let expected_size = u32::deserialize(buffer);
        Self {
            bytes,
            expected_size,
        }
    }
}

impl Serialize for Duration {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        self.as_secs_f64().serialize(buffer);
    }

    fn size(&self) -> usize {
        size_of::<f64>()
    }
}

impl Deserialize<'_> for Duration {
    fn deserialize(buffer: &mut Cursor<&[u8]>) -> Self {
        Duration::from_secs_f64(f64::deserialize(buffer))
    }
}

impl Serialize for Animation<'_> {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        self.animation.tag().serialize(buffer);
        for frame in self.animation.iter() {
            frame.0.serialize(buffer);
            frame.1.serialize(buffer);
        }
    }

    fn size(&self) -> usize {
        self.animation.tag().size()
            + self
                .animation
                .iter()
                .map(|frame| frame.0.size() + frame.1.size())
                .sum::<usize>()
    }
}

impl<'a> Deserialize<'a> for Animation<'a> {
    fn deserialize(buffer: &mut Cursor<&'a [u8]>) -> Self {
        let animation = (0..u32::deserialize(buffer))
            .map(|_| (BitPack::deserialize(buffer), Duration::deserialize(buffer)))
            .collect();
        Self { animation }
    }
}

impl Serialize for ImageRequest<'_> {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        self.transition.serialize(buffer);
        self.imgs.tag().serialize(buffer);
        for image in self.imgs.iter() {
            image.serialize(buffer);
        }
        self.outputs.tag().serialize(buffer);
        for &output in self.outputs.iter() {
            output.serialize(buffer);
        }
        let animations = self.animations.as_deref().unwrap_or_default();
        animations.tag().serialize(buffer);
        for animation in animations {
            animation.serialize(buffer);
        }
    }

    fn size(&self) -> usize {
        let animations = self.animations.as_deref().unwrap_or_default();
        self.transition.size()
            + self.imgs.tag().size()
            + self.imgs.iter().map(Image::size).sum::<usize>()
            + self.outputs.tag().size()
            + self.outputs.iter().copied().map(str::size).sum::<usize>()
            + animations.tag().size()
            + animations.iter().map(Animation::size).sum::<usize>()
    }
}

impl<'a> Deserialize<'a> for ImageRequest<'a> {
    fn deserialize(buffer: &mut Cursor<&'a [u8]>) -> Self {
        let transition = Transition::deserialize(buffer);
        let imgs = (0..u32::deserialize(buffer))
            .map(|_| Image::deserialize(buffer))
            .collect();
        let outputs = (0..u32::deserialize(buffer))
            .map(|_| buffer.read_tagged())
            .map(std::str::from_utf8)
            .map(|res| res.expect("serializer can write utf8 only"))
            .collect();
        let animations = match u32::deserialize(buffer) {
            0 => None,
            num => (0..num)
                .map(|_| Animation::deserialize(buffer))
                .collect::<Box<[_]>>()
                .into(),
        };
        Self {
            transition,
            imgs,
            outputs,
            animations,
        }
    }
}

impl Serialize for ClearRequest<'_> {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        self.color.as_slice().serialize(buffer);
        self.outputs.tag().serialize(buffer);
        for &output in self.outputs.iter() {
            output.serialize(buffer);
        }
    }

    fn size(&self) -> usize {
        self.color.size()
            + self.outputs.tag().size()
            + self
                .outputs
                .iter()
                .copied()
                .map(Serialize::size_tagged)
                .sum::<usize>()
    }
}

impl<'a> Deserialize<'a> for ClearRequest<'a> {
    fn deserialize(buffer: &mut Cursor<&'a [u8]>) -> Self {
        let color = buffer
            .read(3)
            .try_into()
            .expect("`[u8; 3]` can be created from three byte slice");
        let outputs = (0..u32::deserialize(buffer))
            .map(|_| buffer.read_tagged())
            .map(std::str::from_utf8)
            .map(|res| res.expect("serializer can write utf8 only"))
            .collect();
        Self { color, outputs }
    }
}

impl Serialize for Scale {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        match self {
            Self::Whole(whole) => {
                0u8.serialize(buffer);
                whole.get().serialize(buffer);
            }
            Self::Fractional(fraction) => {
                1u8.serialize(buffer);
                fraction.get().serialize(buffer);
            }
        }
    }

    fn size(&self) -> usize {
        size_of::<u8>() + size_of::<NonZeroI32>()
    }
}

impl<'a> Deserialize<'a> for Scale {
    fn deserialize(buffer: &mut Cursor<&'a [u8]>) -> Self {
        let tag = u8::deserialize(buffer);
        let value = NonZeroI32::new(i32::deserialize(buffer)).unwrap();
        match tag {
            0 => Self::Whole(value),
            1 => Self::Fractional(value),
            _ => unreachable!("invalid discriminant"),
        }
    }
}

impl Serialize for ImageDescription {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        match self {
            ImageDescription::Color(arr) => {
                0u8.serialize(buffer);
                arr.as_slice().serialize(buffer);
            }
            ImageDescription::Img(text) => {
                1u8.serialize(buffer);
                text.as_str().serialize(buffer);
            }
        }
    }

    fn size(&self) -> usize {
        size_of::<u8>()
            + match self {
                ImageDescription::Color(arr) => size_of_val(arr),
                ImageDescription::Img(text) => text.as_str().size_tagged(),
            }
    }
}

impl<'a> Deserialize<'a> for ImageDescription {
    fn deserialize(buffer: &mut Cursor<&'a [u8]>) -> Self {
        match u8::deserialize(buffer) {
            0 => Self::Color(
                buffer
                    .read(3)
                    .try_into()
                    .expect("`[u8; 3]` can be created from three byte slice"),
            ),
            1 => Self::Img(
                std::str::from_utf8(buffer.read_tagged())
                    .expect("valid utf8")
                    .into(),
            ),
            _ => unreachable!("invalid discriminant"),
        }
    }
}

impl Serialize for Info {
    fn serialize(&self, buffer: &mut Cursor<&mut [u8]>) {
        buffer.write_tagged(self.name.as_bytes());
        self.dim.serialize(buffer);
        self.scale.serialize(buffer);
        self.img.serialize(buffer);
        self.format.serialize(buffer);
    }

    fn size(&self) -> usize {
        self.name.as_str().size_tagged()
            + self.dim.size()
            + self.scale.size()
            + self.img.size()
            + self.format.size()
    }
}

impl<'a> Deserialize<'a> for Info {
    fn deserialize(buffer: &mut Cursor<&'a [u8]>) -> Self {
        let name = std::str::from_utf8(buffer.read_tagged())
            .expect("valid utf8")
            .into();
        let dim = Vec2::<u32>::deserialize(buffer);
        let scale = Scale::deserialize(buffer);
        let img = ImageDescription::deserialize(buffer);
        let format = PixelFormat::deserialize(buffer);
        Self {
            name,
            dim,
            scale,
            img,
            format,
        }
    }
}
