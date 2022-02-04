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

use super::Bg;
use crate::cli::Img;
pub mod comp_decomp;

struct Animator {
    outputs: Arc<RwLock<Vec<String>>>,
    thread_handle: thread::JoinHandle<()>,
}

pub struct Processor {
    frame_sender: SyncSender<(Vec<String>, Vec<u8>)>,
    animations: Vec<Animator>,
}

impl Processor {
    pub fn new(frame_sender: SyncSender<(Vec<String>, Vec<u8>)>) -> Self {
        Self {
            animations: Vec::new(),
            frame_sender,
        }
    }

    pub fn process(&mut self, bgs: &mut RefMut<Vec<Bg>>, request: Img) -> Result<String, String> {
        let outputs = get_real_outputs(bgs, &request.outputs);
        if outputs.is_empty() {
            error!("None of the outputs sent were valid.");
            return Err("None of the outputs sent are valid.".to_string());
        }

        for group in get_outputs_groups(bgs, outputs) {
            self.stop_animations(&group);
            //We check if we can open and read the image before sending it, so these should never fail
            //Note these can't be moved outside the loop without creating some memory overhead
            let img_buf = image::io::Reader::open(&request.path)
                .expect("Failed to open image, though this should be impossible...");
            let format = img_buf.format();
            let img = img_buf
                .decode()
                .expect("Img decoding failed, though this should be impossible...");

            let bg = bgs
                .iter_mut()
                .find(|bg| bg.output_name == group[0])
                .unwrap();
            let dimensions = bg.dimensions;
            let old_img = bg.get_current_img();
            let img_resized = img_resize(img, dimensions, request.filter.get_image_filter());

            self.transition(&request, old_img, img_resized, dimensions, group, format);
        }
        debug!("Finished image processing!");
        Ok("".to_string())
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
        for i in to_remove {
            let animator = self.animations.swap_remove(i);
            if let Err(e) = animator.thread_handle.join() {
                error!("Animation thread panicked: {:?}", e);
            };
        }
    }

    fn transition(
        &mut self,
        request: &Img,
        old_img: &[u8],
        new_img: Vec<u8>,
        dimensions: (u32, u32),
        outputs: Vec<String>,
        format: Option<ImageFormat>,
    ) {
        let filter = request.filter.get_image_filter();
        let path = request.path.clone();
        let step = request.transition_step;
        let old_img = old_img.to_vec();
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

///Verifies that all outputs exist
///Also puts in all outpus if an empty string was offered
fn get_real_outputs(bgs: &mut RefMut<Vec<Bg>>, outputs: &str) -> Vec<String> {
    let mut real_outputs: Vec<String> = Vec::with_capacity(bgs.len());
    //An empty line means all outputs
    if outputs.is_empty() {
        for bg in bgs.iter() {
            real_outputs.push(bg.output_name.to_owned());
        }
    } else {
        for output in outputs.split(',') {
            let output = output.to_string();
            let mut exists = false;
            for bg in bgs.iter() {
                if output == bg.output_name {
                    exists = true;
                }
            }

            if !exists {
                error!("Output {} does not exist!", output);
            } else if !real_outputs.contains(&output) {
                real_outputs.push(output);
            }
        }
    }
    real_outputs
}

///Returns one result per output with same dimesions and image
fn get_outputs_groups(bgs: &mut RefMut<Vec<Bg>>, mut outputs: Vec<String>) -> Vec<Vec<String>> {
    let mut outputs_groups = Vec::new();

    while !outputs.is_empty() {
        let mut out_same_group = Vec::with_capacity(outputs.len());
        out_same_group.push(outputs.pop().unwrap());

        let dim;
        let old_img_path;
        {
            let bg = bgs
                .iter_mut()
                .find(|bg| bg.output_name == out_same_group[0])
                .unwrap();
            dim = bg.dimensions;
            old_img_path = bg.img.clone();
        }

        for bg in bgs.iter().filter(|bg| outputs.contains(&bg.output_name)) {
            if bg.dimensions == dim && bg.img == old_img_path {
                out_same_group.push(bg.output_name.clone());
            }
        }
        outputs.retain(|o| !out_same_group.contains(o));
        debug!(
            "Output group: {:?}, {:?}, {:?}",
            &out_same_group, dim, old_img_path
        );
        outputs_groups.push(out_same_group);
    }
    outputs_groups
}

///Returns whether the transition completed or was interrupted
fn complete_transition(
    mut old_img: Vec<u8>,
    goal: &[u8],
    step: u8,
    outputs: &mut Arc<RwLock<Vec<String>>>,
    sender: &SyncSender<(Vec<String>, Vec<u8>)>,
) -> bool {
    let mut done = true;
    let mut now = Instant::now();
    let duration = Duration::from_millis(34); //A little less than 30 fps
    let mut transition_img = Vec::with_capacity(goal.len());
    loop {
        let mut i = 0;
        for old_pixel in old_img.chunks_exact(4) {
            for (j, old_color) in old_pixel.iter().enumerate().take(3) {
                let k = i + j;
                let distance = if *old_color > goal[k] {
                    old_color - goal[k]
                } else {
                    goal[k] - old_color
                };
                if distance < step {
                    transition_img.push(goal[k]);
                } else if *old_color > goal[k] {
                    done = false;
                    transition_img.push(old_color - step);
                } else {
                    done = false;
                    transition_img.push(old_color + step);
                }
            }
            transition_img.push(old_pixel[3]);
            i += 4;
        }

        let mut compressed_img = comp_decomp::mixed_comp(&old_img, &transition_img);
        compressed_img.shrink_to_fit();
        comp_decomp::mixed_decomp(&mut old_img, &compressed_img);

        if send_frame(
            compressed_img,
            outputs,
            duration.saturating_sub(now.elapsed()),
            sender,
        ) {
            return false;
        }
        if done {
            break;
        } else {
            transition_img.clear();
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
    sender: SyncSender<(Vec<String>, Vec<u8>)>,
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
    for frame in frames.by_ref() {
        let frame = frame.unwrap();
        let (dur_num, dur_div) = frame.delay().numer_denom_ms();
        let duration = Duration::from_millis((dur_num / dur_div).into());
        let img = img_resize(
            image::DynamicImage::ImageRgba8(frame.into_buffer()),
            dimensions,
            filter,
        );
        let mut compressed_frame = comp_decomp::mixed_comp(&canvas, &img);
        compressed_frame.shrink_to_fit();
        canvas = img;

        cached_frames.push((compressed_frame.clone(), duration));

        if send_frame(
            compressed_frame,
            outputs,
            duration.saturating_sub(now.elapsed()),
            &sender,
        ) {
            return;
        };
        now = Instant::now();
    }
    //Add the first frame we got earlier:
    let mut first_frame_comp = comp_decomp::mixed_comp(&canvas, &first_frame);
    first_frame_comp.shrink_to_fit();
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
    cached_frames: &[(Vec<u8>, Duration)],
    outputs: &mut Arc<RwLock<Vec<String>>>,
    sender: SyncSender<(Vec<String>, Vec<u8>)>,
    mut now: Instant,
) {
    info!("Finished caching the frames!");
    loop {
        for (cached_img, duration) in cached_frames {
            if send_frame(
                cached_img.clone(),
                outputs,
                duration.saturating_sub(now.elapsed()),
                &sender,
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
    frame: Vec<u8>,
    outputs: &mut Arc<RwLock<Vec<String>>>,
    timeout: Duration,
    sender: &SyncSender<(Vec<String>, Vec<u8>)>,
) -> bool {
    thread::sleep(timeout); //TODO: better timeout?
    match outputs.read() {
        Ok(outputs) => {
            //This means a new image will be displayed instead, and this animation must end
            //This means the receiver died for some reason, and this animation must also end
            if outputs.is_empty() || sender.send((outputs.clone(), frame)).is_err() {
                return true;
            }
        }
        Err(e) => {
            error!("Error when sending frame from processor: {}", e);
            return true;
        }
    }
    false
}
