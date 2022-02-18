use image::{
    self, codecs::gif::GifDecoder, imageops::FilterType, io::Reader, AnimationDecoder,
    GenericImageView, ImageFormat,
};
use log::{debug, info};

use smithay_client_toolkit::reexports::calloop::channel::SyncSender;

use std::{
    io::BufReader,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use crate::Answer;
pub mod comp_decomp;
use comp_decomp::Packed;

pub struct ProcessorRequest {
    pub outputs: Vec<String>,
    pub dimensions: (u32, u32),
    pub old_img: Vec<u8>,
    pub path: PathBuf,
    pub filter: FilterType,
    pub step: u8,
}

pub struct Processor {
    frame_sender: SyncSender<(Vec<String>, Packed)>,
    anim_stoppers: Vec<mpsc::Sender<Vec<String>>>,
}

impl Processor {
    pub fn new(frame_sender: SyncSender<(Vec<String>, Packed)>) -> Self {
        Self {
            anim_stoppers: Vec::new(),
            frame_sender,
        }
    }

    pub fn process(&mut self, requests: Vec<ProcessorRequest>) -> Answer {
        for request in requests {
            self.stop_animations(&request.outputs);
            //Note these can't be moved outside the loop without creating some memory overhead
            let img_buf = match image::io::Reader::open(&request.path) {
                Ok(i) => i,
                Err(e) => {
                    return Answer::Err {
                        msg: format!("failed to open image '{:#?}': {}", &request.path, e),
                    }
                }
            };
            let format = img_buf.format();
            let img = match img_buf.decode() {
                Ok(i) => i,
                Err(e) => {
                    return Answer::Err {
                        msg: format!("failed to decode image '{:#?}': {}", &request.path, e),
                    }
                }
            };

            let new_img = img_resize(img, request.dimensions, request.filter);
            self.transition(request, new_img, format);
        }
        debug!("Finished image processing!");
        Answer::Ok
    }

    pub fn stop_animations(&mut self, to_stop: &[String]) {
        self.anim_stoppers
            .retain(|a| a.send(to_stop.to_vec()).is_ok());
    }

    //TODO: if two images will have the same animation, but have differen current images,
    //this will make the animations independent from each other, which isn't really necessary
    fn transition(&mut self, req: ProcessorRequest, new_img: Vec<u8>, format: Option<ImageFormat>) {
        let ProcessorRequest {
            mut outputs,
            dimensions,
            old_img,
            path,
            filter,
            step,
        } = req;
        let sender = self.frame_sender.clone();
        let (stopper, stop_recv) = mpsc::channel();
        self.anim_stoppers.push(stopper);
        thread::spawn(move || {
            if !complete_transition(old_img, &new_img, step, &mut outputs, &sender, &stop_recv) {
                return;
            }
            if format == Some(ImageFormat::Gif) {
                let gif = image::io::Reader::open(path).unwrap();
                animate(gif, new_img, outputs, dimensions, filter, sender, stop_recv);
            }
        });
    }
}

impl Drop for Processor {
    //We need to make sure pending animators exited
    fn drop(&mut self) {
        while !self.anim_stoppers.is_empty() {
            self.stop_animations(&Vec::new());
        }
    }
}

///Returns whether the transition completed or was interrupted
fn complete_transition(
    mut old_img: Vec<u8>,
    goal: &[u8],
    step: u8,
    outputs: &mut Vec<String>,
    sender: &SyncSender<(Vec<String>, Packed)>,
    stop_recv: &mpsc::Receiver<Vec<String>>,
) -> bool {
    let mut done = true;
    let mut now = Instant::now();
    let duration = Duration::from_millis(34); //A little less than 30 fps
    let mut transition_img: Vec<u8> = Vec::with_capacity(goal.len());
    loop {
        for (old, new) in old_img.chunks_exact(4).zip(goal.chunks_exact(4)) {
            for (old_color, new_color) in old.iter().zip(new.iter()).take(3) {
                let distance = if old_color > new_color {
                    old_color - new_color
                } else {
                    new_color - old_color
                };
                if distance < step {
                    transition_img.push(*new_color);
                } else if old_color > new_color {
                    done = false;
                    transition_img.push(old_color - step);
                } else {
                    done = false;
                    transition_img.push(old_color + step);
                }
            }
            transition_img.push(255);
        }

        let compressed_img = Packed::pack(&old_img, &transition_img);
        if send_frame(
            compressed_img,
            outputs,
            duration.saturating_sub(now.elapsed()),
            sender,
            stop_recv,
        ) {
            debug!("Transition was interrupted!");
            return false;
        }

        if done {
            break;
        } else {
            old_img.clear();
            old_img.append(&mut transition_img);
            now = Instant::now();
            done = true;
        }
    }
    debug!("Transition has finished.");
    true
}

fn animate(
    gif: Reader<BufReader<std::fs::File>>,
    first_frame: Vec<u8>,
    mut outputs: Vec<String>,
    dimensions: (u32, u32),
    filter: FilterType,
    sender: SyncSender<(Vec<String>, Packed)>,
    stop_recv: mpsc::Receiver<Vec<String>>,
) {
    let mut frames = GifDecoder::new(gif.into_inner())
        .expect("Couldn't decode gif, though this should be impossible...")
        .into_frames();

    //The first frame should always exist
    let duration_first_frame = frames.next().unwrap().unwrap().delay().numer_denom_ms();
    let duration_first_frame =
        Duration::from_millis((duration_first_frame.0 / duration_first_frame.1).into());

    let mut cached_frames = Vec::new();

    let mut canvas = first_frame.clone();
    let mut now = Instant::now();
    while let Some(Ok(frame)) = frames.next() {
        let (dur_num, dur_div) = frame.delay().numer_denom_ms();
        let duration = Duration::from_millis((dur_num / dur_div).into());
        let img = img_resize(
            image::DynamicImage::ImageRgba8(frame.into_buffer()),
            dimensions,
            filter,
        );

        let compressed_frame = Packed::pack(&canvas, &img);
        canvas = img;
        cached_frames.push((compressed_frame.clone(), duration));

        if send_frame(
            compressed_frame,
            &mut outputs,
            duration.saturating_sub(now.elapsed()),
            &sender,
            &stop_recv,
        ) {
            return;
        };

        now = Instant::now();
    }
    //Add the first frame we got earlier:
    let first_frame_comp = Packed::pack(&canvas, &first_frame);
    cached_frames.insert(0, (first_frame_comp, duration_first_frame));
    if cached_frames.len() > 1 {
        drop(first_frame);
        drop(canvas);
        drop(frames);
        cached_frames.shrink_to_fit();
        loop_animation(&cached_frames, outputs, sender, stop_recv, now);
    }
}

fn loop_animation(
    cached_frames: &[(Packed, Duration)],
    mut outputs: Vec<String>,
    sender: SyncSender<(Vec<String>, Packed)>,
    stop_recv: mpsc::Receiver<Vec<String>>,
    mut now: Instant,
) {
    info!("Finished caching the frames!");
    loop {
        for (cached_img, duration) in cached_frames {
            if send_frame(
                cached_img.clone(),
                &mut outputs,
                duration.saturating_sub(now.elapsed()),
                &sender,
                &stop_recv,
            ) {
                return;
            };
            now = Instant::now();
        }
    }
}

fn img_resize(img: image::DynamicImage, dimensions: (u32, u32), filter: FilterType) -> Vec<u8> {
    let (width, height) = dimensions;
    let img_dimensions = img.dimensions();
    debug!("Output dimensions: width: {} height: {}", width, height);
    debug!(
        "Image dimensions:  width: {} height: {}",
        img_dimensions.0, img_dimensions.1
    );
    let resized_img = if img_dimensions != (width, height) {
        debug!("Image dimensions are different from output's. Resizing...");
        img.resize_to_fill(width, height, filter)
    } else {
        debug!("Image dimensions are identical to output's. Skipped resize!!");
        img
    };

    // The ARGB is 'little endian', so here we must  put the order
    // of bytes 'in reverse', so it needs to be BGRA.
    let mut result = resized_img.into_rgba8().into_raw();
    for pixel in result.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    result.shrink_to_fit();
    result
}

///Returns whether the calling function should exit or not
fn send_frame(
    frame: Packed,
    outputs: &mut Vec<String>,
    timeout: Duration,
    sender: &SyncSender<(Vec<String>, Packed)>,
    stop_recv: &mpsc::Receiver<Vec<String>>,
) -> bool {
    match stop_recv.recv_timeout(timeout) {
        Ok(to_remove) => {
            outputs.retain(|o| !to_remove.contains(o));
            if outputs.is_empty() {
                return true;
            }
            match sender.send((outputs.clone(), frame)) {
                Ok(()) => false,
                Err(_) => true,
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => match sender.send((outputs.clone(), frame)) {
            Ok(()) => false,
            Err(_) => true,
        },
        Err(mpsc::RecvTimeoutError::Disconnected) => true,
    }
}
