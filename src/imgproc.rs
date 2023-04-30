use fast_image_resize::{FilterType, PixelType, Resizer};
use image::{codecs::gif::GifDecoder, AnimationDecoder, DynamicImage, RgbImage};
use std::{
    fs::File,
    io::{stdin, BufReader, Read},
    num::NonZeroU32,
    path::Path,
    time::Duration,
};

use utils::{
    comp_decomp::BitPack,
    ipc::{self, Coord, Position},
};

use crate::cli::ResizeStrategy;

use super::cli;

pub fn read_img(path: &Path) -> Result<(RgbImage, bool), String> {
    if let Some("-") = path.to_str() {
        let mut reader = BufReader::new(stdin());
        let mut buffer = Vec::new();
        if let Err(e) = reader.read_to_end(&mut buffer) {
            return Err(format!("failed to read stdin: {e}"));
        }

        return match image::load_from_memory(&buffer) {
            Ok(img) => Ok((img.into_rgb8(), false)),
            Err(e) => return Err(format!("failed load image from memory: {e}")),
        };
    }

    let imgbuf = match image::io::Reader::open(path) {
        Ok(img) => img,
        Err(e) => return Err(format!("failed to open image: {e}")),
    };

    let imgbuf = match imgbuf.with_guessed_format() {
        Ok(img) => img,
        Err(e) => return Err(format!("failed to detect the image's format: {e}")),
    };

    let is_gif = imgbuf.format() == Some(image::ImageFormat::Gif);
    match imgbuf.decode() {
        Ok(img) => Ok((img.into_rgb8(), is_gif)),
        Err(e) => Err(format!("failed to decode image: {e}")),
    }
}

#[inline]
pub fn frame_to_rgb(frame: image::Frame) -> RgbImage {
    DynamicImage::ImageRgba8(frame.into_buffer()).into_rgb8()
}

/// This is a fairly complicated function. It compresses the animation frames in a pipeline:
///
/// ```
///       READ_FRAME (SEQUENTIAL)
///              |     
///      RESIZE_IMAGE (PARALLEL)
///              |     
///   COLLECT_RESIZED_IMAGES (SEQUENTIAL)
///              |
///   BIT_PACK_THE_RESIZED_IMAGES (PARALLEL)
///              |
///   COLLECT_PACKED_FRAMES (SEQUENTIAL)
/// ```
///
/// While this has improved performance, ultimately our biggest bottleneck is actually the very
/// first step, `READ_FRAME`. This means that significant speed gains can only be had through
/// improvements in the `image` crate itself.
pub fn compress_frames(
    gif: GifDecoder<BufReader<File>>,
    dim: (u32, u32),
    filter: FilterType,
    resize: ResizeStrategy,
    color: &[u8; 3],
) -> Result<Vec<(BitPack, Duration)>, String> {
    let mut rgb_frames = Vec::new();
    std::thread::scope(|s| {
        let (fr_snd, fr_recv) = std::sync::mpsc::channel();
        // This will contain all of the final 3 stages
        let compressor = s.spawn(move || {
            let (comp_snd, comp_recv) = std::sync::mpsc::channel();
            let mut durs = Vec::new();
            let mut tags = Vec::new();
            // COLLECT_RESIZED_IMAGES STAGE
            while let Ok((img, dur, tag)) = fr_recv.recv() {
                let len = tags.len();
                let mut i = 0;
                while i <= len {
                    if i == len || tag < tags[i] {
                        rgb_frames.insert(i, img);
                        durs.insert(i, dur);
                        tags.insert(i, tag);
                        break;
                    }
                    i += 1;
                }
                // BIT_PACK_THE_RESIZED_IMAGES STAGE
                if i > 0 && tags[i - 1] == tag - 1 {
                    let tag = tags[i - 1];
                    let dur = durs[i - 1];
                    let comp_snd = comp_snd.clone();
                    let prev_cur: &[Vec<u8>] = &rgb_frames[i - 1..i + 1];
                    let prev_cur: &[Vec<u8>] = unsafe { std::mem::transmute(prev_cur) };
                    s.spawn(move || {
                        comp_snd
                            .send((BitPack::pack(&prev_cur[0], &prev_cur[1]).unwrap(), dur, tag))
                            .unwrap();
                    });
                }
                if i < len && tags[i + 1] == tag + 1 {
                    let comp_snd = comp_snd.clone();
                    let prev_cur: &[Vec<u8>] = &rgb_frames[i..i + 2];
                    let prev_cur: &[Vec<u8>] = unsafe { std::mem::transmute(prev_cur) };
                    s.spawn(move || {
                        comp_snd
                            .send((BitPack::pack(&prev_cur[0], &prev_cur[1]).unwrap(), dur, tag))
                            .unwrap();
                    });
                }
            }
            // Send the last frame to loop around
            let prev: &Vec<u8> = unsafe { std::mem::transmute(rgb_frames.last().unwrap()) };
            let cur: &Vec<u8> = unsafe { std::mem::transmute(rgb_frames.first().unwrap()) };
            let tag = *tags.last().unwrap();
            let dur = *durs.last().unwrap();
            s.spawn(move || {
                comp_snd
                    .send((BitPack::pack(prev, cur).unwrap(), dur, tag))
                    .unwrap();
            });

            // COLLECT_PACKED_FRAMES STAGE
            let mut packs = Vec::with_capacity(rgb_frames.len());
            let mut tags = Vec::with_capacity(rgb_frames.len());
            while let Ok((pack, dur, tag)) = comp_recv.recv() {
                let len = tags.len();
                for i in 0..len + 1 {
                    if i == len || tag < tags[i] {
                        packs.insert(i, (pack, dur));
                        tags.insert(i, tag);
                        break;
                    }
                }
            }
            // return the collected packs
            packs
        });

        // READ_FRAME STAGE
        let mut frames = gif.into_frames().enumerate();
        while let Some((i, Ok(frame))) = frames.next() {
            let fr_snd = fr_snd.clone();
            // RESIZE_IMAGE STAGE
            s.spawn(move || {
                let (dur_num, dur_div) = frame.delay().numer_denom_ms();
                let duration = Duration::from_millis((dur_num / dur_div).into());
                let img = match resize {
                    ResizeStrategy::No => img_pad(frame_to_rgb(frame), dim, color)?,
                    ResizeStrategy::Crop => img_resize_crop(frame_to_rgb(frame), dim, filter)?,
                    ResizeStrategy::Fit => img_resize_fit(frame_to_rgb(frame), dim, filter, color)?,
                };
                fr_snd.send((img, duration, i)).unwrap();
                Ok::<(), String>(())
            });
        }
        drop(fr_snd);

        match compressor.join() {
            Ok(compressed_frames) => Ok(compressed_frames),
            Err(e) => Err(format!("error compressing animation frames: {e:?}")),
        }
    })
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
    let mut padded = Vec::with_capacity(padded_w * padded_w * 3);

    let img = image::imageops::crop(&mut img, 0, 0, dimensions.0, dimensions.1).to_image();
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

/// Convert an ARGB &[u8] to BRGA in-place by swapping bytes
#[inline]
fn argb_to_brga(argb: &mut [u8]) {
    for pixel in argb.chunks_exact_mut(4) {
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

        let (trg_w, trg_h) = if width.abs_diff(img_w) > height.abs_diff(img_h) {
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
        argb_to_brga(&mut res);
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
    argb_to_brga(&mut resized_img);

    Ok(resized_img)
}

pub fn make_transition(img: &cli::Img) -> ipc::Transition {
    let mut angle = img.transition_angle;

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
        step: img.transition_step,
        fps: img.transition_fps,
        bezier: img.transition_bezier,
        angle,
        pos,
        transition_type,
        wave: img.transition_wave,
        invert_y: img.invert_y,
    }
}
