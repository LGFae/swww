use image::gif::GifDecoder;
use image::io::Reader;
use image::{self, imageops::FilterType, GenericImageView};
use image::{AnimationDecoder, ImageFormat};
use log::{debug, info};

use smithay_client_toolkit::reexports::calloop::channel::Sender;

use std::io::BufReader;
use std::time::{Duration, Instant};
use std::{path::Path, sync::mpsc, thread};

pub type ProcessingResult = (Vec<String>, Vec<u8>);

pub struct Processor {
    frame_sender: Sender<(Vec<String>, Vec<u8>)>,
    on_going_animations: Vec<mpsc::Sender<Vec<String>>>,
}

impl Processor {
    pub fn new(frame_sender: Sender<(Vec<String>, Vec<u8>)>) -> Self {
        Self {
            on_going_animations: Vec::new(),
            frame_sender,
        }
    }
    pub fn process(
        &mut self,
        msg: (Vec<String>, (u32, u32), FilterType, &Path),
    ) -> ProcessingResult {
        let outputs = msg.0;
        let (width, height) = msg.1;
        let filter = msg.2;
        let path = msg.3;

        let mut i = 0;
        while i < self.on_going_animations.len() {
            if self.on_going_animations[i].send(outputs.clone()).is_err() {
                self.on_going_animations.remove(i);
            } else {
                i += 1;
            }
        }
        //We check if we can open and read the image before sending it, so these should never fail
        let img_buf = image::io::Reader::open(&path)
            .expect("Failed to open image, though this should be impossible...");
        let format = img_buf.format();
        let img = img_buf
            .decode()
            .expect("Img decoding failed, though this should be impossible...");
        let img = img_resize(img, width, height, filter);

        //TODO: Also do apng and maybe try to find a way that doesn't clone stuff like this
        if format == Some(ImageFormat::Gif) {
            self.process_gif(path, &img, width, height, outputs.clone(), filter);
        }

        debug!("Finished image processing!");
        (outputs, img)
    }

    fn process_gif(
        &mut self,
        gif_path: &Path,
        first_frame: &[u8],
        width: u32,
        height: u32,
        outputs: Vec<String>,
        filter: FilterType,
    ) {
        let sender = self.frame_sender.clone();
        let (stop_sender, stop_receiver) = mpsc::channel();
        self.on_going_animations.push(stop_sender);

        let gif_buf = image::io::Reader::open(gif_path).unwrap();
        let mut compressed = deflate::compress_to_vec(first_frame, 6);
        compressed.shrink_to_fit();

        thread::spawn(move || {
            animate(
                gif_buf,
                compressed,
                outputs,
                width,
                height,
                filter,
                sender,
                stop_receiver,
            )
        });
    }
}

fn animate(
    gif: Reader<BufReader<std::fs::File>>,
    first_frame: Vec<u8>,
    mut outputs: Vec<String>,
    width: u32,
    height: u32,
    filter: FilterType,
    sender: Sender<(Vec<String>, Vec<u8>)>,
    receiver: mpsc::Receiver<Vec<String>>,
) {
    let mut frames = GifDecoder::new(gif.into_inner())
        .expect("Couldn't decode gif, though this should be impossible...")
        .into_frames();

    let mut cached_frames = Vec::new();

    //The first frame should always exist, the second might not
    let mut frame = frames.next().unwrap();
    if let Some(f) = frames.next() {
        let (dur_num, dur_div) = frame.unwrap().delay().numer_denom_ms();
        let duration = Duration::from_millis((dur_num / dur_div).into());
        cached_frames.push((first_frame, duration));
        frame = f;
    } else {
        return; //This means we had only a static image anyway
    }

    let mut now = Instant::now();
    loop {
        {
            let frame = frame.unwrap();
            let (dur_num, dur_div) = frame.delay().numer_denom_ms();
            let duration = Duration::from_millis((dur_num / dur_div).into());

            let img_buf = image::DynamicImage::ImageRgba8(frame.into_buffer());
            let img = img_resize(img_buf, width, height, filter);
            let mut compressed = deflate::compress_to_vec(&img, 6);
            compressed.shrink_to_fit();

            cached_frames.push((compressed.clone(), duration));

            match receiver.recv_timeout(duration.saturating_sub(now.elapsed())) {
                Ok(out_to_remove) => {
                    outputs.retain(|o| !out_to_remove.contains(o));
                    if outputs.is_empty() {
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    debug!("Receiver disconnected! Stopping animation...");
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            };
            sender
                .send((outputs.clone(), compressed))
                .unwrap_or_else(|_| return);
        }
        if let Some(f) = frames.next() {
            frame = f;
            now = Instant::now();
        } else {
            break; // We've seen and cached all the frames
        }
    }
    cached_frames.shrink_to_fit();
    loop_animation(&cached_frames, outputs, sender, receiver);
}

fn loop_animation(
    cached_frames: &[(Vec<u8>, Duration)],
    mut outputs: Vec<String>,
    sender: Sender<(Vec<String>, Vec<u8>)>,
    receiver: mpsc::Receiver<Vec<String>>,
) {
    info!("Finished caching the frames!");
    let mut now = Instant::now();
    loop {
        for (cached_img, duration) in cached_frames {
            match receiver.recv_timeout(duration.saturating_sub(now.elapsed())) {
                Ok(out_to_remove) => {
                    outputs.retain(|o| !out_to_remove.contains(o));
                    if outputs.is_empty() {
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            };
            sender
                .send((outputs.clone(), cached_img.clone()))
                .unwrap_or_else(|_| return);
            now = Instant::now();
        }
    }
}

fn img_resize(img: image::DynamicImage, width: u32, height: u32, filter: FilterType) -> Vec<u8> {
    let img_dimensions = img.dimensions();
    debug!("Output dimensions: width: {} height: {}", width, height);
    debug!(
        "Image dimensions:  width: {} height: {}",
        img_dimensions.0, img_dimensions.1
    );
    let resized_img = if img_dimensions != (width, height) {
        info!("Image dimensions are different from output's. Resizing...");
        img.resize_to_fill(width, height, filter)
    } else {
        info!("Image dimensions are identical to output's. Skipped resize!!");
        img
    };

    // The ARGB is 'little endian', so here we must  put the order
    // of bytes 'in reverse', so it needs to be BGRA.
    resized_img.into_bgra8().into_raw()
}
