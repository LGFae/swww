use image::{
    self, codecs::gif::GifDecoder, imageops::FilterType, io::Reader, AnimationDecoder,
    GenericImageView, ImageFormat,
};
use log::{debug, error, info};

use smithay_client_toolkit::reexports::calloop::channel::SyncSender;

use std::{
    cell::RefMut,
    io::BufReader,
    sync::{Arc, RwLock},
    thread,
    time::{Duration, Instant},
};

use super::{Bg, BgImg};
use crate::cli::Img;
use crate::Answer;
pub mod comp_decomp;
use comp_decomp::Packed;

pub struct ProcessingGroups {
    outputs: Vec<String>,
    dimensions: (u32, u32),
    old_img: Vec<u8>,
}

impl ProcessingGroups {
    ///Returns one group per output with same dimensions and current image
    pub fn make(bgs: &mut RefMut<Vec<Bg>>, outputs: &[String]) -> Vec<Self> {
        let mut groups: Vec<(ProcessingGroups, BgImg)> = Vec::with_capacity(outputs.len());
        bgs.iter_mut()
            .filter(|bg| outputs.contains(&bg.output_name))
            .for_each(|bg| {
                if let Some(i) = groups
                    .iter()
                    .position(|g| bg.dimensions == g.0.dimensions && bg.img == g.1)
                {
                    groups[i].0.outputs.push(bg.output_name.clone());
                } else {
                    groups.push((
                        ProcessingGroups {
                            outputs: vec![bg.output_name.clone()],
                            dimensions: bg.dimensions,
                            old_img: bg.get_current_img().to_vec(),
                        },
                        bg.img.clone(),
                    ));
                }
            });
        groups.into_iter().map(|g| g.0).collect()
    }
}

struct Animator {
    outputs: Arc<RwLock<Vec<String>>>,
    thread_handle: thread::JoinHandle<()>,
}

pub struct Processor {
    frame_sender: SyncSender<(Vec<String>, Packed)>,
    animations: Vec<Animator>,
}

impl Processor {
    pub fn new(frame_sender: SyncSender<(Vec<String>, Packed)>) -> Self {
        Self {
            animations: Vec::new(),
            frame_sender,
        }
    }

    pub fn process(&mut self, groups: Vec<ProcessingGroups>, request: Img) -> Answer {
        for ProcessingGroups {
            outputs,
            dimensions,
            old_img,
        } in groups
        {
            self.stop_animations(&outputs);
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

            let new_img = img_resize(img, dimensions, request.filter.get_image_filter());

            self.transition(&request, old_img, new_img, dimensions, outputs, format);
        }
        debug!("Finished image processing!");
        Answer::Ok
    }

    pub fn stop_animations(&mut self, to_stop: &[String]) {
        let mut to_remove = Vec::with_capacity(self.animations.len());
        for (i, animator) in self.animations.iter().enumerate() {
            //NOTE: this blocks the calloop! We might want to change this with something with a timeout
            if let Ok(mut outputs) = animator.outputs.write() {
                outputs.retain(|o| !to_stop.contains(o));
                if outputs.is_empty() {
                    to_remove.push(i);
                }
            }
        }
        //If we remove one 'i', the next indexes will become i - 1. If we remove two 'i's, the next
        //indexes will become i - 2. So on and so forth.
        for (offset, i) in to_remove.iter().enumerate() {
            let animator = self.animations.remove(i - offset);
            if let Err(e) = animator.thread_handle.join() {
                error!("Animation thread panicked: {:?}", e);
            };
        }
    }

    //TODO: if two images will have the same animation, but have differen current images,
    //this will make the animations independent from each other, which isn't really necessary
    fn transition(
        &mut self,
        request: &Img,
        old_img: Vec<u8>,
        new_img: Vec<u8>,
        dimensions: (u32, u32),
        outputs: Vec<String>,
        format: Option<ImageFormat>,
    ) {
        let filter = request.filter.get_image_filter();
        let path = request.path.clone();
        let step = request.transition_step;
        let sender = self.frame_sender.clone();
        let out_arc = Arc::new(RwLock::new(outputs));
        let mut out_clone = out_arc.clone();
        self.animations.push(Animator {
            outputs: out_arc,
            thread_handle: thread::spawn(move || {
                let mut ani = format == Some(ImageFormat::Gif);
                ani &= complete_transition(old_img, &new_img, step, &mut out_clone, &sender);
                debug!("Transition has finished!");

                if ani {
                    let gif = image::io::Reader::open(path).unwrap();
                    animate(gif, new_img, &mut out_clone, dimensions, filter, sender);
                }
            }),
        })
    }
}

impl Drop for Processor {
    //We need to make sure to kill all pending animators
    fn drop(&mut self) {
        for animator in &self.animations {
            if let Ok(mut outputs) = animator.outputs.write() {
                outputs.clear();
            }
        }
        while !self.animations.is_empty() {
            let anim = self.animations.pop().unwrap();
            if let Err(e) = anim.thread_handle.join() {
                error!("Animation thread panicked on drop: {:?}", e);
            };
        }
    }
}


///Returns whether the transition completed or was interrupted
fn complete_transition(
    mut old_img: Vec<u8>,
    goal: &[u8],
    step: u8,
    outputs: &mut Arc<RwLock<Vec<String>>>,
    sender: &SyncSender<(Vec<String>, Packed)>,
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
        if send_frame(compressed_img, outputs, &duration, &mut now, sender) {
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
    true
}

fn animate(
    gif: Reader<BufReader<std::fs::File>>,
    first_frame: Vec<u8>,
    outputs: &mut Arc<RwLock<Vec<String>>>,
    dimensions: (u32, u32),
    filter: FilterType,
    sender: SyncSender<(Vec<String>, Packed)>,
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

        if send_frame(compressed_frame, outputs, &duration, &mut now, &sender) {
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
        loop_animation(&cached_frames, outputs, sender, now);
    }
}

fn loop_animation(
    cached_frames: &[(Packed, Duration)],
    outputs: &mut Arc<RwLock<Vec<String>>>,
    sender: SyncSender<(Vec<String>, Packed)>,
    mut now: Instant,
) {
    info!("Finished caching the frames!");
    loop {
        for (cached_img, duration) in cached_frames {
            if send_frame(cached_img.clone(), outputs, duration, &mut now, &sender) {
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
    mut frame: Packed,
    outputs: &mut Arc<RwLock<Vec<String>>>,
    timeout: &Duration,
    now: &mut Instant,
    sender: &SyncSender<(Vec<String>, Packed)>,
) -> bool {
    loop {
        thread::sleep(timeout.saturating_sub(now.elapsed())); //TODO: better timeout?
        match outputs.read() {
            Ok(outputs) => {
                //This means a new image will be displayed instead, and this animation must end
                if outputs.is_empty() {
                    return true;
                }
                match sender.try_send((outputs.clone(), frame)) {
                    Ok(()) => return false,
                    //we try again in this case
                    Err(std::sync::mpsc::TrySendError::Full(e)) => {
                        *now = Instant::now();
                        frame = e.1
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return true,
                }
            }
            Err(e) => {
                error!("Error when sending frame from processor: {}", e);
                return true;
            }
        }
    }
}
