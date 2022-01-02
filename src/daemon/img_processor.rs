use image::{self, imageops::FilterType, GenericImageView};
use image::{AnimationDecoder, ImageFormat};
use log::{debug, info};

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
    sender: Sender<(Vec<String>, Vec<u8>)>,
    receiver: Channel<(Vec<String>, (u32, u32), FilterType, PathBuf)>,
) {
    let mut event_loop = calloop::EventLoop::<LoopSignal>::try_new().unwrap();
    let event_handle = event_loop.handle();
    event_handle
        .insert_source(receiver, |event, _, loop_signal| match event {
            calloop::channel::Event::Msg(msg) => {
                let sender = sender.clone();
                thread::spawn(move || handle_msg(sender, msg.0, msg.1, msg.2, msg.3));
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
    sender: Sender<(Vec<String>, Vec<u8>)>,
    outputs: Vec<String>,
    dimensions: (u32, u32),
    filter: FilterType,
    path: PathBuf,
) {
    let (width, height) = dimensions;

    //We check if we can open and read the image before sending it, so these should never fail
    let img = image::io::Reader::open(&path)
        .expect("Failed to open image, though this should be impossible...");
    let img_bytes;
    match img.format() {
        Some(ImageFormat::Gif) => {
            let gif = image::codecs::gif::GifDecoder::new(img.into_inner())
                .expect("Failed to read gif. This should be impossible");
            let mut frames = gif.into_frames().collect_frames().unwrap();
            if frames.len() > 1 {
                return;
            } else {
                img_bytes = img_try_open_and_resize(
                    image::DynamicImage::ImageRgba8(frames.pop().unwrap().into_buffer()),
                    width,
                    height,
                    filter,
                );
            }
        }
        None => unreachable!("Unsupported format. This also should be impossible..."),
        _ => img_bytes = img_try_open_and_resize(img.decode().unwrap(), width, height, filter),
    };

    debug!(
        "Sending message back from processor. {:?}, {}x{}",
        path, width, height
    );
    sender.send((outputs, img_bytes)).unwrap();
}

fn img_try_open_and_resize(
    img: image::DynamicImage,
    width: u32,
    height: u32,
    filter: FilterType,
) -> Vec<u8> {
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
