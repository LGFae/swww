use fast_image_resize::{FilterType, PixelType, Resizer};
use image::{
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
    AnimationDecoder, DynamicImage, Frames, ImageFormat, RgbImage,
};
use std::{
    io::{stdin, Cursor, Read},
    num::NonZeroU32,
    path::Path,
    time::Duration,
};

use utils::{
    compression::{BitPack, Compressor},
    ipc::{self, ArchivedPixelFormat, Coord, Position},
};

use crate::cli::ResizeStrategy;

use super::cli;

#[derive(Clone)]
pub struct ImgBuf {
    bytes: Box<[u8]>,
    format: ImageFormat,
    is_animated: bool,
}

impl ImgBuf {
    /// Create a new ImgBuf from a given path. Use - for Stdin
    pub fn new(path: &Path) -> Result<Self, String> {
        let bytes = if let Some("-") = path.to_str() {
            let mut bytes = Vec::new();
            stdin()
                .read_to_end(&mut bytes)
                .map_err(|e| format!("failed to read standard input: {e}"))?;
            bytes.into_boxed_slice()
        } else {
            std::fs::read(path)
                .map_err(|e| format!("failed to read file: {e}"))?
                .into_boxed_slice()
        };

        let reader = image::io::Reader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|e| format!("failed to detect the image's format: {e}"))?;

        let format = reader.format();
        let is_animated = match format {
            Some(ImageFormat::Gif) => true,
            Some(ImageFormat::WebP) => {
                // Note: unwrapping is safe because we already opened the file once before this
                WebPDecoder::new(Cursor::new(&bytes))
                    .map_err(|e| format!("failed to decode Webp Image: {e}"))?
                    .has_animation()
            }
            Some(ImageFormat::Png) => PngDecoder::new(Cursor::new(&bytes))
                .map_err(|e| format!("failed to decode Png Image: {e}"))?
                .is_apng()
                .map_err(|e| format!("failed to detect if Png is animated: {e}"))?,
            None => return Err("Unknown image format".to_string()),
            _ => false,
        };

        Ok(Self {
            format: format.unwrap(), // this is ok because we return err earlier if it is None
            bytes,
            is_animated,
        })
    }

    #[inline]
    pub fn is_animated(&self) -> bool {
        self.is_animated
    }

    /// Decode the ImgBuf into am RgbImage
    pub fn decode(&self) -> Result<RgbImage, String> {
        let mut reader = image::io::Reader::new(Cursor::new(&self.bytes));
        reader.set_format(self.format);
        Ok(reader
            .decode()
            .map_err(|e| format!("failed to decode image: {e}"))?
            .into_rgb8())
    }

    /// Convert this ImgBuf into Frames
    pub fn as_frames(&self) -> Result<Frames, String> {
        match self.format {
            ImageFormat::Gif => Ok(GifDecoder::new(Cursor::new(&self.bytes))
                .map_err(|e| format!("failed to decode gif during animation: {e}"))?
                .into_frames()),
            ImageFormat::WebP => Ok(WebPDecoder::new(Cursor::new(&self.bytes))
                .map_err(|e| format!("failed to decode webp during animation: {e}"))?
                .into_frames()),
            ImageFormat::Png => Ok(PngDecoder::new(Cursor::new(&self.bytes))
                .map_err(|e| format!("failed to decode png during animation: {e}"))?
                .apng()
                .unwrap() // we detected this earlier
                .into_frames()),
            _ => Err(format!(
                "requested format has no decoder: {:#?}",
                self.format
            )),
        }
    }
}

#[inline]
pub fn frame_to_rgb(frame: image::Frame) -> RgbImage {
    DynamicImage::ImageRgba8(frame.into_buffer()).into_rgb8()
}

pub fn compress_frames(
    mut frames: Frames,
    dim: (u32, u32),
    format: ArchivedPixelFormat,
    filter: FilterType,
    resize: ResizeStrategy,
    color: &[u8; 3],
) -> Result<Vec<(BitPack, Duration)>, String> {
    let mut compressor = Compressor::new();
    let mut compressed_frames = Vec::new();

    // The first frame should always exist
    let first = frames.next().unwrap().unwrap();
    let first_duration = first.delay().numer_denom_ms();
    let mut first_duration = Duration::from_millis((first_duration.0 / first_duration.1).into());
    let first_img = match resize {
        ResizeStrategy::No => img_pad(frame_to_rgb(first), dim, format, color)?,
        ResizeStrategy::Crop => img_resize_crop(frame_to_rgb(first), dim, format, filter)?,
        ResizeStrategy::Fit => img_resize_fit(frame_to_rgb(first), dim, format, filter, color)?,
    };

    let mut canvas: Option<Vec<u8>> = None;
    while let Some(Ok(frame)) = frames.next() {
        let (dur_num, dur_div) = frame.delay().numer_denom_ms();
        let duration = Duration::from_millis((dur_num / dur_div).into());

        let img = match resize {
            ResizeStrategy::No => img_pad(frame_to_rgb(frame), dim, format, color)?,
            ResizeStrategy::Crop => img_resize_crop(frame_to_rgb(frame), dim, format, filter)?,
            ResizeStrategy::Fit => img_resize_fit(frame_to_rgb(frame), dim, format, filter, color)?,
        };

        if let Some(canvas) = canvas.as_ref() {
            match compressor.compress(canvas, &img, format) {
                Some(bytes) => compressed_frames.push((bytes, duration)),
                None => match compressed_frames.last_mut() {
                    Some(last) => last.1 += duration,
                    None => first_duration += duration,
                },
            }
        } else {
            match compressor.compress(&first_img, &img, format) {
                Some(bytes) => compressed_frames.push((bytes, duration)),
                None => first_duration += duration,
            }
        }
        canvas = Some(img);
    }

    //Add the first frame we got earlier:
    if let Some(canvas) = canvas.as_ref() {
        match compressor.compress(canvas, &first_img, format) {
            Some(bytes) => compressed_frames.push((bytes, first_duration)),
            None => match compressed_frames.last_mut() {
                Some(last) => last.1 += first_duration,
                None => first_duration += first_duration,
            },
        }
    }

    Ok(compressed_frames)
}

pub fn make_filter(filter: &cli::Filter) -> fast_image_resize::FilterType {
    match filter {
        cli::Filter::Nearest => fast_image_resize::FilterType::Box,
        cli::Filter::Bilinear => fast_image_resize::FilterType::Bilinear,
        cli::Filter::CatmullRom => fast_image_resize::FilterType::CatmullRom,
        cli::Filter::Mitchell => fast_image_resize::FilterType::Mitchell,
        cli::Filter::Lanczos3 => fast_image_resize::FilterType::Lanczos3,
    }
}

pub fn img_pad(
    mut img: RgbImage,
    dimensions: (u32, u32),
    format: ArchivedPixelFormat,
    color: &[u8; 3],
) -> Result<Vec<u8>, String> {
    let mut color = color.to_owned();
    if format.must_swap_r_and_b_channels() {
        color.swap(0, 2);
    }
    let (padded_w, padded_h) = dimensions;
    let (padded_w, padded_h) = (padded_w as usize, padded_h as usize);
    let mut padded = Vec::with_capacity(padded_h * padded_w * 3);

    let img = {
        if img.width() > dimensions.0 || img.height() > dimensions.1 {
            let left = (img.width() - dimensions.0) / 2;
            let top = (img.height() - dimensions.1) / 2;
            image::imageops::crop(&mut img, left, top, dimensions.0, dimensions.1).to_image()
        } else {
            image::imageops::crop(&mut img, 0, 0, dimensions.0, dimensions.1).to_image()
        }
    };
    let (img_w, img_h) = img.dimensions();
    let (img_w, img_h) = (img_w as usize, img_h as usize);
    let raw_img = img.into_vec();

    for _ in 0..(((padded_h - img_h) / 2) * padded_w) {
        padded.extend(color);
    }

    // Calculate left and right border widths. `u32::div` rounds toward 0, so, if `img_w` is odd,
    // add an extra pixel to the right border to ensure the row is the correct width.
    let left_border_w = (padded_w - img_w) / 2;
    let right_border_w = left_border_w + (img_w % 2);

    for row in 0..img_h {
        for _ in 0..left_border_w {
            padded.extend(color);
        }

        for pixel in raw_img[(row * img_w * 3)..((row + 1) * img_w * 3)].chunks_exact(3) {
            if format.must_swap_r_and_b_channels() {
                padded.extend(pixel.iter().rev());
            } else {
                padded.extend(pixel);
            }
        }
        for _ in 0..right_border_w {
            padded.extend(color);
        }
    }

    while padded.len() < (padded_h * padded_w * 3) {
        padded.extend(color);
    }

    Ok(padded)
}

/// Convert an RGB &[u8] to BRG in-place by swapping bytes
#[inline]
fn rgb_to_brg(rgb: &mut [u8]) {
    for pixel in rgb.chunks_exact_mut(3) {
        pixel.swap(0, 2);
    }
}

/// Resize an image to fit within the given dimensions, covering as much space as possible without
/// cropping.
pub fn img_resize_fit(
    img: RgbImage,
    dimensions: (u32, u32),
    format: ArchivedPixelFormat,
    filter: FilterType,
    padding_color: &[u8; 3],
) -> Result<Vec<u8>, String> {
    let (width, height) = dimensions;
    let (img_w, img_h) = img.dimensions();
    if (img_w, img_h) != (width, height) {
        // if our image is already scaled to fit, skip resizing it and just pad it directly
        if img_w == width || img_h == height {
            return img_pad(img, dimensions, format, padding_color);
        }

        let ratio = width as f32 / height as f32;
        let img_r = img_w as f32 / img_h as f32;

        let (trg_w, trg_h) = if ratio > img_r {
            let scale = height as f32 / img_h as f32;
            ((img_w as f32 * scale) as u32, height)
        } else {
            let scale = width as f32 / img_w as f32;
            (width, (img_h as f32 * scale) as u32)
        };

        let src = match fast_image_resize::Image::from_vec_u8(
            // We unwrap below because we know the images's dimensions should never be 0
            NonZeroU32::new(img_w).unwrap(),
            NonZeroU32::new(img_h).unwrap(),
            img.into_raw(),
            PixelType::U8x3,
        ) {
            Ok(i) => i,
            Err(e) => return Err(e.to_string()),
        };

        // We unwrap below because we know the outputs's dimensions should never be 0
        let new_w = NonZeroU32::new(trg_w).unwrap();
        let new_h = NonZeroU32::new(trg_h).unwrap();

        let mut dst = fast_image_resize::Image::new(new_w, new_h, PixelType::U8x3);
        let mut dst_view = dst.view_mut();

        let mut resizer = Resizer::new(fast_image_resize::ResizeAlg::Convolution(filter));
        if let Err(e) = resizer.resize(&src.view(), &mut dst_view) {
            return Err(e.to_string());
        }

        img_pad(
            image::RgbImage::from_raw(trg_w, trg_h, dst.into_vec()).unwrap(),
            dimensions,
            format,
            padding_color,
        )
    } else {
        let mut res = img.into_vec();
        if format.must_swap_r_and_b_channels() {
            rgb_to_brg(&mut res);
        }
        Ok(res)
    }
}

pub fn img_resize_crop(
    img: RgbImage,
    dimensions: (u32, u32),
    format: ArchivedPixelFormat,
    filter: FilterType,
) -> Result<Vec<u8>, String> {
    let (width, height) = dimensions;
    let (img_w, img_h) = img.dimensions();
    let mut resized_img = if (img_w, img_h) != (width, height) {
        let src = match fast_image_resize::Image::from_vec_u8(
            // We unwrap below because we know the images's dimensions should never be 0
            NonZeroU32::new(img_w).unwrap(),
            NonZeroU32::new(img_h).unwrap(),
            img.into_raw(),
            PixelType::U8x3,
        ) {
            Ok(i) => i,
            Err(e) => return Err(e.to_string()),
        };

        // We unwrap below because we know the outputs's dimensions should never be 0
        let new_w = NonZeroU32::new(width).unwrap();
        let new_h = NonZeroU32::new(height).unwrap();
        let mut src_view = src.view();
        src_view.set_crop_box_to_fit_dst_size(new_w, new_h, Some((0.5, 0.5)));

        let mut dst = fast_image_resize::Image::new(new_w, new_h, PixelType::U8x3);
        let mut dst_view = dst.view_mut();

        let mut resizer = Resizer::new(fast_image_resize::ResizeAlg::Convolution(filter));
        if let Err(e) = resizer.resize(&src_view, &mut dst_view) {
            return Err(e.to_string());
        }

        dst.into_vec()
    } else {
        img.into_vec()
    };

    if format.must_swap_r_and_b_channels() {
        rgb_to_brg(&mut resized_img);
    }

    Ok(resized_img)
}

pub fn make_transition(img: &cli::Img) -> ipc::Transition {
    let mut angle = img.transition_angle;
    let mut step = img.transition_step;

    let x = match img.transition_pos.x {
        cli::CliCoord::Percent(x) => {
            if !(0.0..=1.0).contains(&x) {
                println!(
                    "Warning: x value not in range [0,1] position might be set outside screen: {x}"
                );
            }
            Coord::Percent(x)
        }
        cli::CliCoord::Pixel(x) => Coord::Pixel(x),
    };

    let y = match img.transition_pos.y {
        cli::CliCoord::Percent(y) => {
            if !(0.0..=1.0).contains(&y) {
                println!(
                    "Warning: y value not in range [0,1] position might be set outside screen: {y}"
                );
            }
            Coord::Percent(y)
        }
        cli::CliCoord::Pixel(y) => Coord::Pixel(y),
    };

    let mut pos = Position::new(x, y);

    let transition_type = match img.transition_type {
        cli::TransitionType::None => {
            step = u8::MAX;
            ipc::TransitionType::Simple
        }
        cli::TransitionType::Simple => ipc::TransitionType::Simple,
        cli::TransitionType::Fade => ipc::TransitionType::Fade,
        cli::TransitionType::Wipe => ipc::TransitionType::Wipe,
        cli::TransitionType::Outer => ipc::TransitionType::Outer,
        cli::TransitionType::Grow => ipc::TransitionType::Grow,
        cli::TransitionType::Wave => ipc::TransitionType::Wave,
        cli::TransitionType::Right => {
            angle = 0.0;
            ipc::TransitionType::Wipe
        }
        cli::TransitionType::Top => {
            angle = 90.0;
            ipc::TransitionType::Wipe
        }
        cli::TransitionType::Left => {
            angle = 180.0;
            ipc::TransitionType::Wipe
        }
        cli::TransitionType::Bottom => {
            angle = 270.0;
            ipc::TransitionType::Wipe
        }
        cli::TransitionType::Center => {
            pos = Position::new(Coord::Percent(0.5), Coord::Percent(0.5));
            ipc::TransitionType::Grow
        }
        cli::TransitionType::Any => {
            pos = Position::new(
                Coord::Percent(rand::random::<f32>()),
                Coord::Percent(rand::random::<f32>()),
            );
            if rand::random::<u8>() % 2 == 0 {
                ipc::TransitionType::Grow
            } else {
                ipc::TransitionType::Outer
            }
        }
        cli::TransitionType::Random => {
            pos = Position::new(
                Coord::Percent(rand::random::<f32>()),
                Coord::Percent(rand::random::<f32>()),
            );
            angle = rand::random();
            match rand::random::<u8>() % 4 {
                0 => ipc::TransitionType::Simple,
                1 => ipc::TransitionType::Wipe,
                2 => ipc::TransitionType::Outer,
                3 => ipc::TransitionType::Grow,
                _ => unreachable!(),
            }
        }
    };

    ipc::Transition {
        duration: img.transition_duration,
        step,
        fps: img.transition_fps,
        bezier: img.transition_bezier,
        angle,
        pos,
        transition_type,
        wave: img.transition_wave,
        invert_y: img.invert_y,
    }
}
