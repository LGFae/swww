use fast_image_resize::{FilterType, PixelType, Resizer};
use image::{self, codecs::gif::GifDecoder, ImageFormat};
use log::debug;

use smithay_client_toolkit::reexports::calloop::channel::SyncSender;

use std::{
    num::NonZeroU32,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use crate::{
    cli::{Img, TransitionType},
    Answer,
};

mod animations;
pub mod comp_decomp;

use animations::{GifProcessor, Transition};
use comp_decomp::ReadiedPack;

use super::BgInfo;

///The default thread stack size of 2MiB is way too overkill for our purposes
const TSTACK_SIZE: usize = 1 << 18; //256KiB

///Note: since this entire struct will be going to a new thread, it has to own all of its values.
///This means even though, in the case of multiple outputs with different dimensions, they would
///all have the same path, filter, step and fps, we still need to store all those values multiple
///times, because we would simply have to clone them when moving them into the thread anyway
pub struct ProcessorRequest {
    outputs: Vec<String>,
    dimensions: (u32, u32),
    old_img: Box<[u8]>,
    path: PathBuf,
    transition_type: TransitionType,
    speed: u8,
    filter: FilterType,
    step: u8,
    fps: Duration,
    pos: String,
    bezier: String,
}

impl ProcessorRequest {
    pub fn new(info: &BgInfo, old_img: Box<[u8]>, img: &Img) -> Self {
        Self {
            outputs: vec![info.name.to_string()],
            dimensions: info.dim,
            old_img,
            path: img.path.clone(),
            transition_type: img.transition_type.clone(),
            speed: img.transition_speed,
            filter: img.filter.get_image_filter(),
            step: img.transition_step,
            fps: Duration::from_nanos(1_000_000_000 / img.transition_fps as u64),
            pos: img.transition_pos.clone(),
            // convert 1,1,1,1 to a tuple of 4 floats
            bezier: img.transition_bezier.clone(),
        }
    }

    pub fn add_output(&mut self, output: &str) {
        self.outputs.push(output.to_string());
    }

    pub fn dim(&self) -> (u32, u32) {
        self.dimensions
    }

    fn split(self) -> (Vec<String>, Transition, Option<GifProcessor>,String,String){
        let transition = Transition::new(
            self.old_img,
            self.dimensions,
            self.transition_type,
            self.speed,
            self.step,
            self.fps,
        );
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
        (self.outputs, transition, animation,self.pos,self.bezier)
    }
}

pub struct Processor {
    frame_sender: SyncSender<(Vec<String>, ReadiedPack)>,
    anim_stoppers: Vec<mpsc::Sender<Vec<String>>>,
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

            let new_img = match img_resize(img, request.dimensions, request.filter) {
                Ok(i) => i,
                Err(e) => return Answer::Err(e),
            };

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
        let (stopper, stop_recv) = mpsc::channel();
        self.anim_stoppers.push(stopper);
        let transition_type = request.transition_type.clone();
        if let Err(e) = thread::Builder::new()
            .name("animator".to_string()) //Name our threads  for better log messages
            .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn(move || {
                let (mut out, transition, gif,pos,bezier) = request.split();
                let (width,height) = (transition.dimensions.0 as f64, transition.dimensions.1 as f64);
                let mut parsed_pos = (0.0,0.0);

                // default ease-in-out
                let mut parsed_bezier:(f64,f64,f64,f64) = (0.42,0.0,0.58,1.0);
                match transition_type{
                    TransitionType::Wipe => {
                        let pos = pos.split(",").collect::<Vec<&str>>()[0].trim().replace("deg","");
                        match pos.as_str(){
                            "left" => parsed_pos = (0.0,0.0),
                            "right" => parsed_pos = (180.0,0.0),
                            "top" => parsed_pos = (90.0,0.0),
                            "bottom" => parsed_pos = (-90.0,0.0),
                            _ => {
                                match pos.parse::<f64>(){
                                    Ok(p) => parsed_pos = (p,0.0),
                                    Err(e) => {
                                        log::error!("failed to parse 'pos' flag using 0deg as fallback: {:?}", e);
                                    }
                                }
                                
                            }
                        }
                    },
                    _ => {
                        match pos.as_str() {
                            "center" => parsed_pos = (width/2.0,height/2.0),
                            "random" => parsed_pos = ((rand::random::<usize>() % width as usize) as f64,(rand::random::<usize>() % height as usize) as f64),
                            "left" => {parsed_pos = (0.0,height/2.0)},
                            "right" => parsed_pos = (width,height/2.0),
                            "top" => parsed_pos = (width/2.0,height),
                            "bottom" => parsed_pos = (width/2.0,0.0),
                            "bottom-left" => parsed_pos = (0.0,0.0),
                            "bottom-right" => parsed_pos = (width,0.0),
                            "top-left" => parsed_pos = (0.0,height),
                            "top-right" => parsed_pos = (width,height),
                            _ => {
                                let pos = pos.trim().split(",").collect::<Vec<&str>>();
                                if pos.len() == 2 {
                                    parsed_pos = (pos[0].parse::<f64>().unwrap(),pos[1].parse::<f64>().unwrap());
                                } else{
                                    log::error!("failed to parse 'pos' flag using '0,0' as fallback: {:?}",pos);
                                }
                            },
                        }
                    }
                }
                
                match bezier.as_str() {
                    "ease-in-out" => parsed_bezier = (0.42,0.0,0.58,1.0),
                    "ease-in" => parsed_bezier = (0.42,0.0,1.0,1.0),
                    "ease-out" => parsed_bezier = (0.0,0.0,0.58,1.0),
                    "linear" => parsed_bezier = (0.0,0.0,1.0,1.0),
                    _ => {
                        for (i,val) in  bezier.split(",").enumerate(){
                            let val = val.parse::<f64>().unwrap();
                            match i {
                                0 => parsed_bezier.0 = val,
                                1 => parsed_bezier.1 = val,
                                2 => parsed_bezier.2 = val,
                                3 => parsed_bezier.3 = val,
                                _ => (),
                            }
                        }
                    },
                }
                if transition.execute(&new_img, &mut out, &sender, &stop_recv, parsed_pos, parsed_bezier) {
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
    debug!("Starting animation");
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

fn img_resize(
    img: image::RgbaImage,
    dimensions: (u32, u32),
    filter: FilterType,
) -> Result<Box<[u8]>, String> {
    let (width, height) = dimensions;
    let (img_w, img_h) = img.dimensions();
    debug!("Output dimensions: {:?}", (width, height));
    debug!("Image dimensions:  {:?}", (img_w, img_h));
    let mut resized_img = if (img_w, img_h) != (width, height) {
        debug!("Image dimensions are different from output's. Resizing...");

        let mut src = match fast_image_resize::Image::from_vec_u8(
            // We unwrap bellow because we know the images's dimensions should never be 0
            NonZeroU32::new(img_w).unwrap(),
            NonZeroU32::new(img_h).unwrap(),
            img.into_raw(),
            PixelType::U8x4,
        ) {
            Ok(i) => i,
            Err(e) => return Err(e.to_string()),
        };

        let alpha_mul_div = fast_image_resize::MulDiv::default();
        if let Err(e) = alpha_mul_div.multiply_alpha_inplace(&mut src.view_mut()) {
            return Err(e.to_string());
        }

        // We unwrap bellow because we know the outputs's dimensions should never be 0
        let new_w = NonZeroU32::new(width).unwrap();
        let new_h = NonZeroU32::new(height).unwrap();
        let mut src_view = src.view();
        src_view.set_crop_box_to_fit_dst_size(new_w, new_h, Some((0.5, 0.5)));

        let mut dst = fast_image_resize::Image::new(new_w, new_h, PixelType::U8x4);
        let mut dst_view = dst.view_mut();

        let mut resizer = Resizer::new(fast_image_resize::ResizeAlg::Convolution(filter));
        if let Err(e) = resizer.resize(&src_view, &mut dst_view) {
            return Err(e.to_string());
        }

        if let Err(e) = alpha_mul_div.divide_alpha_inplace(&mut dst_view) {
            return Err(e.to_string());
        }

        image::RgbaImage::from_vec(width, height, dst.into_vec()).unwrap()
    } else {
        img
    };

    // The ARGB is 'little endian', so here we must  put the order
    // of bytes 'in reverse', so it needs to be BGRA.
    for pixel in resized_img.pixels_mut() {
        pixel.0.swap(0, 2);
    }

    Ok(resized_img.into_raw().into_boxed_slice())
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
                debug!("STOPPING");
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
