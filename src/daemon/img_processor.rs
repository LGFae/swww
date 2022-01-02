use image::{self, imageops::FilterType, GenericImageView};
use image::{AnimationDecoder, ImageFormat};
use log::{debug, info};

use smithay_client_toolkit::reexports::calloop::channel::Channel;
use smithay_client_toolkit::reexports::calloop::{self, channel::Sender, LoopSignal};

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

///Waits for a msg from the daemon event_loop
pub fn processor_loop(
    sender: Sender<(Vec<String>, Vec<u8>)>,
    receiver: Channel<(Vec<String>, (u32, u32), FilterType, PathBuf)>,
) {
    let mut event_loop =
        calloop::EventLoop::<(LoopSignal, Vec<mpsc::Sender<Vec<String>>>)>::try_new().unwrap();
    let event_handle = event_loop.handle();
    event_handle
        .insert_source(
            receiver,
            |event, _, (loop_signal, anim_senders)| match event {
                calloop::channel::Event::Msg(msg) => {
                    let mut i = 0;
                    while i != anim_senders.len() {
                        if anim_senders[i].send(msg.0.clone()).is_err() {
                            anim_senders.remove(i);
                        } else {
                            i += 1;
                        }
                    }
                    let sender = sender.clone();
                    if let Some(anim_sender) = handle_msg(sender, msg.0, msg.1, msg.2, &msg.3) {
                        anim_senders.push(anim_sender);
                    }
                }
                calloop::channel::Event::Closed => loop_signal.stop(),
            },
        )
        .unwrap();
    let loop_signal = event_loop.get_signal();
    let anim_senders = Vec::new();
    event_loop
        .run(None, &mut (loop_signal, anim_senders), |_| {})
        .expect("img_processor event_loop failed!");
}

fn handle_msg(
    sender: Sender<(Vec<String>, Vec<u8>)>,
    outputs: Vec<String>,
    dimensions: (u32, u32),
    filter: FilterType,
    path: &Path,
) -> Option<mpsc::Sender<Vec<String>>> {
    let (width, height) = dimensions;

    //We check if we can open and read the image before sending it, so these should never fail
    let img_buff = image::io::Reader::open(&path)
        .expect("Failed to open image, though this should be impossible...");
    let img;
    match img_buff.format() {
        Some(ImageFormat::Gif) => {
            let gif = image::codecs::gif::GifDecoder::new(img_buff.into_inner())
                .expect("Failed to read gif. This should be impossible");
            let mut frames = gif.into_frames().collect_frames().unwrap();
            if frames.len() > 1 {
                let (anim_sender, anim_recv) = mpsc::channel();
                thread::spawn(move || {
                    animate(frames, width, height, outputs, filter, sender, anim_recv)
                });
                return Some(anim_sender);
            } else {
                img = image::DynamicImage::ImageRgba8(frames.pop().unwrap().into_buffer());
            }
        }
        None => unreachable!("Unsupported format. This also should be impossible..."),
        _ => {
            img = img_buff
                .decode()
                .expect("Img decoding failed, though this should be impossible...")
        }
    };
    thread::spawn(move || {
        let img_bytes = img_resize(img, width, height, filter);
        debug!(
            "Sending message back from processor for outputs {:?}",
            outputs
        );
        sender.send((outputs, img_bytes)).unwrap();
    });

    return None;
}

fn animate(
    frames: Vec<image::Frame>,
    width: u32,
    height: u32,
    mut outputs: Vec<String>,
    filter: FilterType,
    frame_sender: Sender<(Vec<String>, Vec<u8>)>,
    anim_recv: mpsc::Receiver<Vec<String>>,
) {
    let mut cached_frames = Vec::with_capacity(frames.len());
    //first loop
    for frame in frames.into_iter() {
        let (dur_num, dur_div) = frame.delay().numer_denom_ms();
        let duration = (dur_num / dur_div).into();
        let img = img_resize(
            image::DynamicImage::ImageRgba8(frame.into_buffer()),
            width,
            height,
            filter,
        );

        cached_frames.push((img.clone(), duration));

        match anim_recv.recv_timeout(std::time::Duration::from_millis(duration)) {
            Ok(out_to_remove) => {
                outputs.retain(|o| !out_to_remove.contains(o));
                if outputs.is_empty() {
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        };
        frame_sender
            .send((outputs.clone(), img))
            .unwrap_or_else(|_| return);
    }
    //loop forever with the cached results:
    loop {
        for frame in &cached_frames {
            let frame_copy = frame.0.clone();
            match anim_recv.recv_timeout(std::time::Duration::from_millis(frame.1)) {
                Ok(out_to_remove) => {
                    outputs.retain(|o| !out_to_remove.contains(o));
                    if outputs.is_empty() {
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            };
            frame_sender
                .send((outputs.clone(), frame_copy))
                .unwrap_or_else(|_| return);
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
