use image::gif::GifDecoder;
use image::io::Reader;
use image::{self, imageops::FilterType, GenericImageView};
use image::{AnimationDecoder, ImageFormat};
use log::{debug, error, info};

use smithay_client_toolkit::reexports::calloop::channel::Sender;

use std::cell::RefMut;
use std::io::BufReader;
use std::time::{Duration, Instant};
use std::{path::Path, sync::mpsc, thread};

use crate::cli::Img;

use super::Background;

pub type ProcessorResult = Result<Vec<(Vec<String>, Vec<u8>)>, String>;

pub mod comp_decomp;

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
    pub fn process(&mut self, bgs: &mut RefMut<Vec<Background>>, request: &Img) -> ProcessorResult {
        let outputs = get_real_outputs(bgs, &request.outputs);
        if outputs.is_empty() {
            error!("None of the outputs sent were valid.");
            return Err("None of the outputs sent are valid.".to_string());
        }
        let filter = request.filter.get_image_filter();
        let path = &request.path;

        let mut results = Vec::new();
        for group in get_outputs_groups(bgs, outputs) {
            let bg = bgs
                .iter_mut()
                .find(|bg| bg.output_name == group[0])
                .unwrap();

            let (width, height) = bg.dimensions;
            let old_img = bg.get_current_img();

            self.stop_animations(&group);
            //We check if we can open and read the image before sending it, so these should never fail
            let img_buf = image::io::Reader::open(path)
                .expect("Failed to open image, though this should be impossible...");
            let format = img_buf.format();
            let img = img_buf
                .decode()
                .expect("Img decoding failed, though this should be impossible...");
            let img_resized = img_resize(img, width, height, filter);

            let mut transition = None;
            if old_img.is_some() && !request.no_transition {
                let old_img = old_img.unwrap();
                info!("There's an old image here! Beginning transition...");
                results.push((
                    group.clone(),
                    self.transition(old_img, &img_resized, &group),
                ));
                transition = Some(self.on_going_animations.last().unwrap().clone());
            } else {
                results.push((group.clone(), img_resized.clone()));
            };

            //TODO: Also do apng
            if format == Some(ImageFormat::Gif) {
                self.process_gif(path, img_resized, width, height, group, filter, transition);
            }
        }

        debug!("Finished image processing!");
        Ok(results)
    }

    pub fn stop_animations(&mut self, to_stop: &[String]) {
        let mut i = 0;
        while i < self.on_going_animations.len() {
            if self.on_going_animations[i].send(to_stop.to_vec()).is_err() {
                self.on_going_animations.remove(i);
            } else {
                i += 1;
            }
        }
    }

    fn transition(&mut self, old_img: Vec<u8>, new_img: &[u8], outputs: &[String]) -> Vec<u8> {
        let sender = self.frame_sender.clone();
        let (stop_sender, stop_receiver) = mpsc::channel();
        self.on_going_animations.push(stop_sender);

        let mut done = true;
        let mut transition_img = Vec::with_capacity(new_img.len());
        let mut i = 0;
        for old_pixel in old_img.chunks_exact(4) {
            for j in 0..3 {
                let k = i + j;
                let distance = if old_pixel[j] > new_img[k] {
                    old_pixel[j] - new_img[k]
                } else {
                    new_img[k] - old_pixel[j]
                };
                if distance < 20 {
                    transition_img.push(new_img[k]);
                } else if old_pixel[j] > new_img[k] {
                    done = false;
                    transition_img.push(old_pixel[j] - 20);
                } else {
                    done = false;
                    transition_img.push(old_pixel[j] + 20);
                }
            }
            transition_img.push(old_pixel[3]);
            i += 4;
        }

        if !done {
            let img = transition_img.clone();
            let new_img = new_img.to_vec();
            let outputs = outputs.to_vec();
            thread::spawn(move || {
                complete_transition(img, new_img, outputs, sender, stop_receiver)
            });
        }

        transition_img
    }

    fn process_gif(
        &mut self,
        gif_path: &Path,
        first_frame: Vec<u8>,
        width: u32,
        height: u32,
        outputs: Vec<String>,
        filter: FilterType,
        transition: Option<mpsc::Sender<Vec<String>>>,
    ) {
        let sender = self.frame_sender.clone();
        let (stop_sender, stop_receiver) = mpsc::channel();
        self.on_going_animations.push(stop_sender);

        let gif_buf = image::io::Reader::open(gif_path).unwrap();

        thread::spawn(move || {
            if let Some(transition) = transition {
                while transition.send(vec![]).is_ok() {
                    thread::sleep(Duration::from_millis(30));
                }
            }
            animate(
                gif_buf,
                first_frame,
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

///Verifies that all outputs exist
///Also puts in all outpus if an empty string was offered
fn get_real_outputs(bgs: &mut RefMut<Vec<Background>>, outputs: &str) -> Vec<String> {
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
fn get_outputs_groups(
    bgs: &mut RefMut<Vec<Background>>,
    mut outputs: Vec<String>,
) -> Vec<Vec<String>> {
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

fn complete_transition(
    mut old_img: Vec<u8>,
    goal: Vec<u8>,
    mut outputs: Vec<String>,
    sender: Sender<(Vec<String>, Vec<u8>)>,
    receiver: mpsc::Receiver<Vec<String>>,
) {
    let mut done = true;
    let mut now = Instant::now();
    let duration = Duration::from_millis(30); //A little less than 30 fps
    loop {
        let mut transition_img = Vec::with_capacity(goal.len());
        let mut i = 0;
        for old_pixel in old_img.chunks_exact(4) {
            for j in 0..3 {
                let k = i + j;
                let distance = if old_pixel[j] > goal[k] {
                    old_pixel[j] - goal[k]
                } else {
                    goal[k] - old_pixel[j]
                };
                if distance < 20 {
                    transition_img.push(goal[k]);
                } else if old_pixel[j] > goal[k] {
                    done = false;
                    transition_img.push(old_pixel[j] - 20);
                } else {
                    done = false;
                    transition_img.push(old_pixel[j] + 20);
                }
            }
            transition_img.push(old_pixel[3]);
            i += 4;
        }

        let mut compressed_img = comp_decomp::mixed_comp(&old_img, &transition_img);
        comp_decomp::mixed_decomp(&mut old_img, &compressed_img);
        compressed_img.shrink_to_fit();

        match receiver.recv_timeout(duration.saturating_sub(now.elapsed())) {
            Ok(out_to_remove) => {
                outputs.retain(|o| !out_to_remove.contains(o));
                if outputs.is_empty() {
                    return;
                }
                thread::sleep(duration.saturating_sub(now.elapsed()));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                debug!("Receiver disconnected! Stopping animation...");
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        };
        sender
            .send((outputs.clone(), compressed_img))
            .unwrap_or_else(|_| return);
        now = Instant::now();
        if done {
            break;
        } else {
            done = true;
        }
    }
    info!("Transition has finished!");
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

    //The first frame should always exist
    let duration_first_frame = frames.next().unwrap().unwrap().delay().numer_denom_ms();
    let duration_first_frame =
        Duration::from_millis((duration_first_frame.0 / duration_first_frame.1).into());

    let mut cached_frames = Vec::new();

    let mut canvas = first_frame.clone();
    let mut now = Instant::now();
    while let Some(frame) = frames.next() {
        let frame = frame.unwrap();
        let (dur_num, dur_div) = frame.delay().numer_denom_ms();
        let duration = Duration::from_millis((dur_num / dur_div).into());
        let img = img_resize(
            image::DynamicImage::ImageRgba8(frame.into_buffer()),
            width,
            height,
            filter,
        );
        let mut compressed_frame = comp_decomp::mixed_comp(&canvas, &img);
        comp_decomp::mixed_decomp(&mut canvas, &compressed_frame);
        compressed_frame.shrink_to_fit();

        cached_frames.push((compressed_frame.clone(), duration));

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
            .send((outputs.clone(), compressed_frame))
            .unwrap_or_else(|_| return);
        now = Instant::now();
    }
    //Add the first frame we got earlier:
    cached_frames.push((
        comp_decomp::mixed_comp(&canvas, &first_frame),
        duration_first_frame,
    ));
    if cached_frames.len() == 1 {
        return; //This means we only had a static image anyway
    } else {
        cached_frames.shrink_to_fit();
        let duration = cached_frames.last().unwrap().1;
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
            .send((outputs.clone(), cached_frames.last().unwrap().0.clone()))
            .unwrap_or_else(|_| return);
        now = Instant::now();

        loop_animation(&cached_frames, outputs, sender, receiver, now);
    }
}

fn loop_animation(
    cached_frames: &[(Vec<u8>, Duration)],
    mut outputs: Vec<String>,
    sender: Sender<(Vec<String>, Vec<u8>)>,
    receiver: mpsc::Receiver<Vec<String>>,
    mut now: Instant,
) {
    info!("Finished caching the frames!");
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
