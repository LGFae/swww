use image::{
    self, codecs::gif::GifDecoder, imageops::FilterType, AnimationDecoder, DynamicImage,
    ImageFormat,
};
use log::debug;

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

impl Transition {
    fn default(mut self, new_img: Vec<u8>, fr_sender: mpsc::Sender<(Packed, Duration)>) {
        let mut done = true;
        let mut transition_img: Vec<u8> = Vec::with_capacity(new_img.len());
        loop {
            for (old, new) in self.old_img.chunks_exact(4).zip(new_img.chunks_exact(4)) {
                for (old_color, new_color) in old.iter().zip(new.iter()).take(3) {
                    let distance = if old_color > new_color {
                        old_color - new_color
                    } else {
                        new_color - old_color
                    };
                    if distance < self.step {
                        transition_img.push(*new_color);
                    } else if old_color > new_color {
                        done = false;
                        transition_img.push(old_color - self.step);
                    } else {
                        done = false;
                        transition_img.push(old_color + self.step);
                    }
                }
                transition_img.push(255);
            }

            let compressed_img = Packed::pack(&self.old_img, &transition_img);
            if fr_sender.send((compressed_img, self.fps)).is_err() {
                debug!("Transition was interrupted!");
                return;
            }
            if done {
                debug!("Transition has finished.");
                return;
            }
            self.old_img.clear();
            self.old_img.append(&mut transition_img);
            done = true;
        }
    }
}

struct GifProcessor {
    gif: PathBuf,
    dimensions: (u32, u32),
    filter: FilterType,
}

impl GifProcessor {
    fn process(self, first_frame: Vec<u8>, fr_sender: mpsc::Sender<(Packed, Duration)>) {
        let gif_reader = image::io::Reader::open(self.gif).unwrap();
        let mut frames = GifDecoder::new(gif_reader.into_inner())
            .expect("Couldn't decode gif, though this should be impossible...")
            .into_frames();
        //The first frame should always exist
        let dur_first_frame = frames.next().unwrap().unwrap().delay().numer_denom_ms();
        let dur_first_frame = Duration::from_millis((dur_first_frame.0 / dur_first_frame.1).into());

        let mut canvas = first_frame.clone();
        while let Some(Ok(frame)) = frames.next() {
            let (dur_num, dur_div) = frame.delay().numer_denom_ms();
            let duration = Duration::from_millis((dur_num / dur_div).into());
            let img = img_resize(frame.into_buffer(), self.dimensions, self.filter);

            let compressed_frame = Packed::pack(&canvas, &img);
            canvas = img;

            if fr_sender.send((compressed_frame, duration)).is_err() {
                return;
            };
        }
        //Add the first frame we got earlier:
        let first_frame_comp = Packed::pack(&canvas, &first_frame);
        let _ = fr_sender.send((first_frame_comp, dur_first_frame));
    }
}

impl ProcessorRequest {
    fn split(self) -> (Vec<String>, Transition, Option<GifProcessor>) {
        let transition = Transition {
            old_img: self.old_img,
            step: self.step,
            fps: self.fps,
        };
        let img = image::io::Reader::open(&self.path);
        let animation = {
            if let Ok(img) = img {
                if img.format() == Some(ImageFormat::Gif) {
                    Some(GifProcessor {
                        gif: self.path,
                        dimensions: self.dimensions,
                        filter: self.filter,
                    })
                } else {
                    None
                }
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
            let img = match image::open(&request.path) {
                Ok(i) => i.into_rgba8(),
                Err(e) => {
                    return Answer::Err(format!(
                        "failed to open image '{:#?}': {}",
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

    fn transition(&mut self, request: ProcessorRequest, new_img: Vec<u8>) {
        let sender = self.frame_sender.clone();
        let (stopper, stop_recv) = mpsc::channel();
        self.anim_stoppers.push(stopper);
        thread::spawn(move || {
            let (mut out, transition, gif) = request.split();
            let tran_anim = |img| {
                animation(
                    |a| transition.default(img, a),
                    false,
                    &mut out,
                    &sender,
                    &stop_recv,
                )
            };
            if let Some(gif) = gif {
                let img_clone = new_img.clone();
                if tran_anim(img_clone){
                    return;
                };
                animation(
                    |a| gif.process(new_img, a),
                    true,
                    &mut out,
                    &sender,
                    &stop_recv,
                );
            } else {
                tran_anim(new_img);
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

/// returns whether or not it was interrupted
fn animation<F: FnOnce(mpsc::Sender<(Packed, Duration)>) + Send + 'static>(
    frame_generator: F,
    infinite: bool,
    outputs: &mut Vec<String>,
    sender: &SyncSender<(Vec<String>, Packed)>,
    stop_recv: &mpsc::Receiver<Vec<String>>,
) -> bool {
    let (fr_send, fr_recv) = mpsc::channel();
    thread::spawn(|| frame_generator(fr_send));
    let mut cached_frames = Vec::new();
    let mut now = Instant::now();
    while let Ok((frame, dur)) = fr_recv.recv() {
        let timeout = dur.saturating_sub(now.elapsed());
        if send_frame(frame.clone(), outputs, timeout, sender, stop_recv) {
            return true;
        };
        cached_frames.push((frame, dur));
        now = Instant::now();
    }

    if infinite && cached_frames.len() > 1 {
        loop {
            for (frame, dur) in &cached_frames {
                let timeout = dur.saturating_sub(now.elapsed());
                if send_frame(frame.to_owned(), outputs, timeout, sender, stop_recv) {
                    return true;
                };
                now = Instant::now();
            }
        }
    }

    false
}

fn img_resize(img: image::RgbaImage, dimensions: (u32, u32), filter: FilterType) -> Vec<u8> {
    let (width, height) = dimensions;
    debug!("Output dimensions: {:?}", (width, height));
    debug!("Image dimensions:  {:?}", img.dimensions());
    let mut resized_img = if img.dimensions() != (width, height) {
        debug!("Image dimensions are different from output's. Resizing...");
        DynamicImage::ImageRgba8(img)
            .resize_to_fill(width, height, filter)
            .into_rgba8()
            .into_raw()
    } else {
        img.into_raw()
    };

    // The ARGB is 'little endian', so here we must  put the order
    // of bytes 'in reverse', so it needs to be BGRA.
    for pixel in resized_img.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    resized_img
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
