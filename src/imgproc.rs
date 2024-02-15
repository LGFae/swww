use fast_image_resize::{FilterType, PixelType, Resizer};
use image::{
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
    AnimationDecoder, DynamicImage, Frames, ImageFormat, RgbImage,
};
use std::{
    fs::File,
    io::Stdin,
    io::{stdin, BufReader, Read},
    num::NonZeroU32,
    path::Path,
    time::Duration,
};

use utils::{
    compression::{BitPack, Compressor},
    ipc::{self, Coord, Position},
};

use crate::cli::ResizeStrategy;

use super::cli;

enum ImgBufInner {
    Stdin(BufReader<Stdin>),
    File(image::io::Reader<BufReader<File>>),
}

impl ImgBufInner {
    /// Guess the format of the ImgBufInner
    #[inline]
    fn format(&self) -> Option<ImageFormat> {
        match &self {
            ImgBufInner::Stdin(_) => None, // not seekable
            ImgBufInner::File(reader) => reader.format(),
        }
    }
}

pub struct ImgBuf {
    inner: ImgBufInner,
    is_animated: bool,
}

impl ImgBuf {
    /// Create a new ImgBuf from a given path. Use - for Stdin
    pub fn new(path: &Path) -> Result<Self, String> {
        Ok(if let Some("-") = path.to_str() {
            let reader = BufReader::new(stdin());
            Self {
                inner: ImgBufInner::Stdin(reader),
                is_animated: false,
            }
        } else {
            let reader = image::io::Reader::open(path)
                .map_err(|e| format!("failed to open image: {e}"))?
                .with_guessed_format()
                .map_err(|e| format!("failed to detect the image's format: {e}"))?;

            let is_animated = {
                match reader.format() {
                    Some(ImageFormat::Gif) => true,
                    Some(ImageFormat::WebP) => {
                        // Note: unwrapping is safe because we already opened the file once before this
                        WebPDecoder::new(BufReader::new(File::open(path).unwrap()))
                            .map_err(|e| format!("failed to decode Webp Image: {e}"))?
                            .has_animation()
                    }
                    Some(ImageFormat::Png) => {
                        PngDecoder::new(BufReader::new(File::open(path).unwrap()))
                            .map_err(|e| format!("failed to decode Png Image: {e}"))?
                            .is_apng()
                    }

                    _ => false,
                }
            };

            Self {
                inner: ImgBufInner::File(reader),
                is_animated,
            }
        })
    }

    /// Guess the format of the ImgBuf
    fn format(&self) -> Option<ImageFormat> {
        self.inner.format()
    }

    #[inline]
    pub fn is_animated(&self) -> bool {
        self.is_animated
    }

    /// Decode the ImgBuf into am RgbImage
    pub fn decode(self) -> Result<RgbImage, String> {
        Ok(match self.inner {
            ImgBufInner::Stdin(mut reader) => {
                let mut buffer = Vec::new();
                reader
                    .read_to_end(&mut buffer)
                    .map_err(|e| format!("failed to read stdin: {e}"))?;

                image::load_from_memory(&buffer)
            }
            ImgBufInner::File(reader) => reader.decode(),
        }
        .map_err(|e| format!("failed to decode image: {e}"))?
        .into_rgb8())
    }

    /// Convert this ImgBuf into Frames
    pub fn into_frames<'a>(self) -> Result<Frames<'a>, String> {
        fn create_decoder<'a>(
            img_format: Option<ImageFormat>,
            reader: impl Read + 'a,
        ) -> Result<Frames<'a>, String> {
            match img_format {
                Some(ImageFormat::Gif) => Ok(GifDecoder::new(reader)
                    .map_err(|e| format!("failed to decode gif during animation: {e}"))?
                    .into_frames()),
                Some(ImageFormat::WebP) => Ok(WebPDecoder::new(reader)
                    .map_err(|e| format!("failed to decode webp during animation: {e}"))?
                    .into_frames()),
                Some(ImageFormat::Png) => Ok(PngDecoder::new(reader)
                    .map_err(|e| format!("failed to decode png during animation: {e}"))?
                    .apng()
                    .into_frames()),
                _ => Err(format!("requested format has no decoder: {img_format:#?}")),
            }
        }

        let img_format = self.format();
        match self.inner {
            ImgBufInner::Stdin(reader) => create_decoder(img_format, reader),
            ImgBufInner::File(reader) => create_decoder(img_format, reader.into_inner()),
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
    filter: FilterType,
    resize: ResizeStrategy,
    color: &[u8; 3],
) -> Result<Vec<(BitPack, Duration)>, String> {
    let compressor = Compressor::new();
    let mut compressed_frames = Vec::new();

    // The first frame should always exist
    let first = frames.next().unwrap().unwrap();
    let first_duration = first.delay().numer_denom_ms();
    let first_duration = Duration::from_millis((first_duration.0 / first_duration.1).into());
    let first_img = match resize {
        ResizeStrategy::No => img_pad(frame_to_rgb(first), dim, color)?,
        ResizeStrategy::Crop => img_resize_crop(frame_to_rgb(first), dim, filter)?,
        ResizeStrategy::Fit => img_resize_fit(frame_to_rgb(first), dim, filter, color)?,
    };

    let mut canvas: Option<Vec<u8>> = None;
    while let Some(Ok(frame)) = frames.next() {
        let (dur_num, dur_div) = frame.delay().numer_denom_ms();
        let duration = Duration::from_millis((dur_num / dur_div).into());

        let img = match resize {
            ResizeStrategy::No => img_pad(frame_to_rgb(frame), dim, color)?,
            ResizeStrategy::Crop => img_resize_crop(frame_to_rgb(frame), dim, filter)?,
            ResizeStrategy::Fit => img_resize_fit(frame_to_rgb(frame), dim, filter, color)?,
        };

        compressed_frames.push((
            compressor.compress(canvas.as_ref().unwrap_or(&first_img), &img)?,
            duration,
        ));
        canvas = Some(img);
    }
    //Add the first frame we got earlier:
    compressed_frames.push((
        compressor.compress(canvas.as_ref().unwrap_or(&first_img), &first_img)?,
        first_duration,
    ));
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
    color: &[u8; 3],
) -> Result<Vec<u8>, String> {
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
        padded.push(color[2]);
        padded.push(color[1]);
        padded.push(color[0]);
    }

    // Calculate left and right border widths. `u32::div` rounds toward 0, so, if `img_w` is odd,
    // add an extra pixel to the right border to ensure the row is the correct width.
    let left_border_w = (padded_w - img_w) / 2;
    let right_border_w = left_border_w + (img_w % 2);

    for row in 0..img_h {
        for _ in 0..left_border_w {
            padded.push(color[2]);
            padded.push(color[1]);
            padded.push(color[0]);
        }

        for pixel in raw_img[(row * img_w * 3)..((row + 1) * img_w * 3)].chunks_exact(3) {
            padded.push(pixel[2]);
            padded.push(pixel[1]);
            padded.push(pixel[0]);
        }
        for _ in 0..right_border_w {
            padded.push(color[2]);
            padded.push(color[1]);
            padded.push(color[0]);
        }
    }

    while padded.len() < (padded_h * padded_w * 3) {
        padded.push(color[2]);
        padded.push(color[1]);
        padded.push(color[0]);
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
    filter: FilterType,
    padding_color: &[u8; 3],
) -> Result<Vec<u8>, String> {
    let (width, height) = dimensions;
    let (img_w, img_h) = img.dimensions();
    if (img_w, img_h) != (width, height) {
        // if our image is already scaled to fit, skip resizing it and just pad it directly
        if img_w == width || img_h == height {
            return img_pad(img, dimensions, padding_color);
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
            padding_color,
        )
    } else {
        let mut res = img.into_vec();
        // The ARGB is 'little endian', so here we must  put the order
        // of bytes 'in reverse', so it needs to be BGRA.
        rgb_to_brg(&mut res);
        Ok(res)
    }
}

pub fn img_resize_crop(
    img: RgbImage,
    dimensions: (u32, u32),
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

    // The ARGB is 'little endian', so here we must  put the order
    // of bytes 'in reverse', so it needs to be BGRA.
    rgb_to_brg(&mut resized_img);

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
