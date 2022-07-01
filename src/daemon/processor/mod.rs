use fast_image_resize::FilterType;
use image::{self, codecs::gif::GifDecoder, ImageFormat};
use log::debug;

use smithay_client_toolkit::reexports::calloop::channel::SyncSender;

use std::{
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use crate::Answer;

mod animations;
pub mod comp_decomp;

use animations::{GifProcessor, Transition};
use comp_decomp::ReadiedPack;

///The default thread stack size of 2MiB is way too overkill for our purposes
const TSTACK_SIZE: usize = 1 << 18; //256KiB

///Note: since this entire struct will be going to a new thread, it has to own all of its values.
///This means even though, in the case of multiple outputs with different dimensions, they would
///all have the same path, filter, step and fps, we still need to store all those values multiple
///times, because we would simply have to clone them when moving them into the thread anyway
pub struct ProcessorRequest {
    pub outputs: Vec<String>,
    pub dimensions: (u32, u32),
    pub old_img: Box<[u8]>,
    pub path: PathBuf,
    pub filter: FilterType,
    pub step: u8,
    pub fps: Duration,
}

impl ProcessorRequest {
    fn split(self) -> (Vec<String>, Transition, Option<GifProcessor>) {
        let transition = Transition::new(self.old_img, self.step, self.fps);
        let img = image::io::Reader::open(&self.path);
        let animation = {
            if let Ok(img) = img {
                if img.format() == Some(ImageFormat::Gif) {
                    Some(GifProcessor::new(
                        GifDecoder::new(img.into_inner()).unwrap(),
                        self.dimensions,
                        self.filter,
                    ))
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
    frame_sender: SyncSender<(Vec<String>, ReadiedPack)>,
    anim_stoppers: Vec<mpsc::SyncSender<Vec<String>>>,
}

impl Processor {
    pub fn new(frame_sender: SyncSender<(Vec<String>, ReadiedPack)>) -> Self {
        Self {
            anim_stoppers: Vec::new(),
            frame_sender,
        }
    }

    pub fn process(&mut self, requests: Vec<ProcessorRequest>) -> Answer {
        for request in requests {
            let img = match image::open(&request.path) {
                Ok(i) => i.into_rgba8(),
                Err(e) => {
                    return Answer::Err(format!(
                        "failed to open image '{:#?}': {}",
                        &request.path, e
                    ))
                }
            };
            self.stop_animations(&request.outputs);

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

    fn transition(&mut self, request: ProcessorRequest, new_img: Box<[u8]>) {
        let sender = self.frame_sender.clone();
        let (stopper, stop_recv) = mpsc::sync_channel(1);
        self.anim_stoppers.push(stopper);
        if let Err(e) = thread::Builder::new()
            .name("animator".to_string()) //Name our threads  for better log messages
            .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn(move || {
                let (mut out, mut transition, gif) = request.split();
                if transition.default(&new_img, &mut out, &sender, &stop_recv) {
                    drop(transition);
                    if let Some(gif) = gif {
                        animation(gif, new_img, out, sender, stop_recv);
                    }
                }
            })
        {
            log::error!("failed to spawn 'animator' thread: {}", e);
        };
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

fn animation(
    gif: GifProcessor,
    new_img: Box<[u8]>,
    mut outputs: Vec<String>,
    sender: SyncSender<(Vec<String>, ReadiedPack)>,
    stopper: mpsc::Receiver<Vec<String>>,
) {
    let img_len = new_img.len();
    let mut cached_frames = Vec::new();
    let mut now = Instant::now();
    {
        let (fr_send, fr_recv) = mpsc::channel();
        let handle = match thread::Builder::new()
            .name("gif_processor".to_string()) //Name our threads  for better log messages
            .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn(move || gif.process(new_img, fr_send))
        {
            Ok(h) => h,
            Err(e) => {
                log::error!("failed to spawn 'gif_processor' thread: {}", e);
                return;
            }
        };

        while let Ok((fr, dur)) = fr_recv.recv() {
            let frame = fr.ready(img_len);
            let timeout = dur.saturating_sub(now.elapsed());
            if send_frame(frame, &mut outputs, timeout, &sender, &stopper) {
                drop(fr_recv);
                let _ = handle.join();
                return;
            };
            now = Instant::now();
            cached_frames.push((fr, dur));
        }
        let _ = handle.join();
    }
    let cached_frames = cached_frames.into_boxed_slice();

    if cached_frames.len() > 1 {
        loop {
            for (fr, dur) in cached_frames.iter() {
                let frame = fr.ready(img_len);
                let timeout = dur.saturating_sub(now.elapsed());
                if send_frame(frame, &mut outputs, timeout, &sender, &stopper) {
                    return;
                };
                now = Instant::now();
            }
        }
    }
}

fn img_resize(img: image::RgbaImage, dimensions: (u32, u32), filter: FilterType) -> Box<[u8]> {
    let (width, height) = dimensions;
    debug!("Output dimensions: {:?}", (width, height));
    debug!("Image dimensions:  {:?}", img.dimensions());
    let mut resized_img = if img.dimensions() != (width, height) {
        debug!("Image dimensions are different from output's. Resizing...");
        let mut src = fast_image_resize::Image::from_vec_u8(
            std::num::NonZeroU32::new(img.dimensions().0).unwrap(),
            std::num::NonZeroU32::new(img.dimensions().1).unwrap(),
            img.into_raw(),
            fast_image_resize::PixelType::U8x4,
        )
        .unwrap();
        let alpha_mul_div = fast_image_resize::MulDiv::default();
        alpha_mul_div
            .multiply_alpha_inplace(&mut src.view_mut())
            .unwrap();
        let mut dst = fast_image_resize::Image::new(
            std::num::NonZeroU32::new(width).unwrap(),
            std::num::NonZeroU32::new(height).unwrap(),
            fast_image_resize::PixelType::U8x4,
        );
        let mut dst_view = dst.view_mut();

        let mut resizer =
            fast_image_resize::Resizer::new(fast_image_resize::ResizeAlg::Convolution(filter));
        resizer.resize(&src.view(), &mut dst_view).unwrap();

        alpha_mul_div.divide_alpha_inplace(&mut dst_view).unwrap();
        image::RgbaImage::from_vec(width, height, dst.into_vec()).unwrap()
    } else {
        img
    };

    // The ARGB is 'little endian', so here we must  put the order
    // of bytes 'in reverse', so it needs to be BGRA.
    for pixel in resized_img.pixels_mut() {
        pixel.0.swap(0, 2);
    }

    resized_img.into_raw().into_boxed_slice()
}

///Returns whether the calling function should exit or not
fn send_frame(
    frame: ReadiedPack,
    outputs: &mut Vec<String>,
    timeout: Duration,
    sender: &SyncSender<(Vec<String>, ReadiedPack)>,
    stop_recv: &mpsc::Receiver<Vec<String>>,
) -> bool {
    match stop_recv.recv_timeout(timeout) {
        Ok(to_remove) => {
            outputs.retain(|o| !to_remove.contains(o));
            if outputs.is_empty() || to_remove.is_empty() {
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
