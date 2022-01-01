use image::{self, imageops, GenericImageView};
use log::{debug, error, info, warn};

use smithay_client_toolkit::reexports::calloop::channel::Channel;
use smithay_client_toolkit::reexports::calloop::{self, channel::Sender, LoopSignal};

use std::{
    path::{Path, PathBuf},
    thread,
};

///Waits for either sigusr1 or sigusr2 to be sent to this process,
///processes img accornding to request, and sends back the result
///If sigterm is found instead, ends the loop
pub fn processor_loop(
    sender: Sender<Option<(Vec<String>, Vec<u8>)>>,
    receiver: Channel<(Vec<String>, (u32, u32), PathBuf)>,
) {
    let mut event_loop = calloop::EventLoop::<LoopSignal>::try_new().unwrap();
    let event_handle = event_loop.handle();
    event_handle
        .insert_source(receiver, |event, _, loop_signal| match event {
            calloop::channel::Event::Msg(msg) => {
                let sender = sender.clone();
                thread::spawn(move || handle_msg(sender, msg.0, msg.1, msg.2));
            }
            calloop::channel::Event::Closed => loop_signal.stop(),
        })
        .unwrap();
    let mut loop_signal = event_loop.get_signal();
    event_loop
        .run(None, &mut loop_signal, |_| {})
        .expect("img_processor event_loop failed!");
}

fn handle_msg(
    sender: Sender<Option<(Vec<String>, Vec<u8>)>>,
    outputs: Vec<String>,
    dimensions: (u32, u32),
    img: PathBuf,
) {
    let (width, height) = dimensions;
    if let Some(img) = img_try_open_and_resize(&img, width, height) {
        info!("Img is ready!");
        sender.send(Some((outputs, img))).unwrap();
    } else {
        sender.send(None).unwrap();
    }
}

fn img_try_open_and_resize(img_path: &Path, width: u32, height: u32) -> Option<Vec<u8>> {
    match image::open(img_path) {
        Ok(img) => {
            if width == 0 || height == 0 {
                error!("Surface dimensions are set to 0. Can't resize image...");
                return None;
            }

            let img_dimensions = img.dimensions();
            debug!("Output dimensions: width: {} height: {}", width, height);
            debug!(
                "Image dimensions:  width: {} height: {}",
                img_dimensions.0, img_dimensions.1
            );
            let resized_img = if img_dimensions != (width, height) {
                info!("Image dimensions are different from output's. Resizing...");
                img.resize_to_fill(width, height, imageops::FilterType::Lanczos3)
            } else {
                info!("Image dimensions are identical to output's. Skipped resize!!");
                img
            };

            // The ARGB is 'little endian', so here we must  put the order
            // of bytes 'in reverse', so it needs to be BGRA.
            debug!(
                "Sending message back from processor: {:?}, {}x{}",
                img_path, width, height
            );
            Some(resized_img.into_bgra8().into_raw())
        }
        Err(e) => {
            error!("Couldn't open image: {}", e);
            None
        }
    }
}
