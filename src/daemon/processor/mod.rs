use image::{
    self, codecs::gif::GifDecoder, imageops::FilterType, AnimationDecoder, Frames,
    GenericImageView, ImageFormat,
};
use log::{debug, info};

use smithay_client_toolkit::reexports::calloop::channel::SyncSender;

use std::{
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use crate::Answer;
pub mod comp_decomp;
use comp_decomp::Packed;

///Note: since this entire struct will be going to a new thread, it has to own all of its values.
///This means even though, in the case of multiple outputs with different dimensions, they would
///all have the same path, filter, step and fps, we still need to store all those values multiple
///times, because we would simply have to clone them when moving them into the thread anyway
pub struct ProcessorRequest {
    pub outputs: Vec<String>,
    pub dimensions: (u32, u32),
    pub old_img: Vec<u8>,
    pub path: PathBuf,
    pub filter: FilterType,
    pub step: u8,
    pub fps: Duration,
}

struct Transition {
    old_img: Vec<u8>,
    step: u8,
    fps: Duration,
}

struct Animation<'a> {
    frames: Frames<'a>,
    dimensions: (u32, u32),
    filter: FilterType,
}

impl ProcessorRequest {
    fn split<'a>(self) -> (Vec<String>, Transition, Option<Animation<'a>>) {
        let transition = Transition {
            old_img: self.old_img,
            step: self.step,
            fps: self.fps,
        };
        let animation = {
            let img = image::io::Reader::open(self.path).unwrap();
            if img.format() == Some(ImageFormat::Gif) {
                let frames = GifDecoder::new(img.into_inner())
                    .expect("Couldn't decode gif, though this should be impossible...")
                    .into_frames();
                Some(Animation {
                    frames,
                    dimensions: self.dimensions,
                    filter: self.filter,
                })
            } else {
                None
            }
        };
        (self.outputs, transition, animation)
    }
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
                    return Answer::Err(format!(
                        "failed to open image '{:#?}': {}",
                        &request.path, e
                    ))
                }
            };
            let img = match img_buf.decode() {
                Ok(i) => i,
                Err(e) => {
                    return Answer::Err(format!(
                        "failed to decode image '{:#?}': {}",
                        &request.path, e
                    ))
                }
            };

            let new_img = img_resize(img, request.dimensions, request.filter);
            self.transition(request, new_img);
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
    fn transition(&mut self, request: ProcessorRequest, new_img: Vec<u8>) {
        let sender = self.frame_sender.clone();
        let (stopper, stop_recv) = mpsc::channel();
        self.anim_stoppers.push(stopper);
        thread::spawn(move || {
            let (mut outputs, transition, animation) = request.split();
            if !complete_transition(transition, &new_img, &mut outputs, &sender, &stop_recv) {
                return;
            }
            if let Some(animation) = animation {
                if let Some((cached_frames, now)) =
                    animate(animation, new_img, &mut outputs, &sender, &stop_recv)
                {
                    loop_animation(cached_frames, outputs, sender, stop_recv, now);
                }
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
    transition: Transition,
    new_img: &[u8],
    outputs: &mut Vec<String>,
    sender: &SyncSender<(Vec<String>, Packed)>,
    stop_recv: &mpsc::Receiver<Vec<String>>,
) -> bool {
    let Transition {
        mut old_img,
        step,
        fps,
    } = transition;
    let mut done = true;
    let mut now = Instant::now();
    let mut transition_img: Vec<u8> = Vec::with_capacity(new_img.len());
    loop {
        for (old, new) in old_img.chunks_exact(4).zip(new_img.chunks_exact(4)) {
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
        let timeout = fps.saturating_sub(now.elapsed());
        if send_frame(compressed_img, outputs, timeout, sender, stop_recv) {
            debug!("Transition was interrupted!");
            return false;
        }
        if done {
            debug!("Transition has finished.");
            return true;
        }
        old_img.clear();
        old_img.append(&mut transition_img);
        now = Instant::now();
        done = true;
    }
}

fn animate(
    animation: Animation,
    first_frame: Vec<u8>,
    outputs: &mut Vec<String>,
    sender: &SyncSender<(Vec<String>, Packed)>,
    stop_recv: &mpsc::Receiver<Vec<String>>,
) -> Option<(Vec<(Packed, Duration)>, Instant)> {
    let Animation {
        mut frames,
        dimensions,
        filter,
    } = animation;
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

        let timeout = duration.saturating_sub(now.elapsed());
        if send_frame(compressed_frame, outputs, timeout, sender, stop_recv) {
            return None;
        };
        now = Instant::now();
    }
    //Add the first frame we got earlier:
    let first_frame_comp = Packed::pack(&canvas, &first_frame);
    cached_frames.insert(0, (first_frame_comp, duration_first_frame));
    if cached_frames.len() > 1 {
        cached_frames.shrink_to_fit();
        Some((cached_frames, now))
    } else {
        None
    }
}

fn loop_animation(
    cached_frames: Vec<(Packed, Duration)>,
    mut outputs: Vec<String>,
    sender: SyncSender<(Vec<String>, Packed)>,
    stop_recv: mpsc::Receiver<Vec<String>>,
    mut now: Instant,
) {
    info!("Finished caching the frames!");
    loop {
        for (cached_img, duration) in &cached_frames {
            let frame = cached_img.clone();
            let timeout = duration.saturating_sub(now.elapsed());
            if send_frame(frame, &mut outputs, timeout, &sender, &stop_recv) {
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
        }
        Err(mpsc::RecvTimeoutError::Timeout) => (),
        Err(mpsc::RecvTimeoutError::Disconnected) => return true,
    }
    match sender.send((outputs.clone(), frame)) {
        Ok(()) => false,
        Err(_) => true,
    }
}
