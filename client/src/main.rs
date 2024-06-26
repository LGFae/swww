use std::error::Error;
use std::path::Path;
use std::thread;
use std::time::Duration;

use clap::Parser;
use cli::Img;
use common::cache;
use common::ipc;
use common::ipc::Answer;
use common::ipc::BgImg;
use common::ipc::Client;
use common::ipc::IpcSocket;
use common::ipc::PixelFormat;
use common::ipc::RequestSend;
use common::mmap::Mmap;

mod imgproc;
use imgproc::*;

mod cli;
use cli::CliImage;
use cli::ResizeStrategy;
use cli::Swww;

fn main() -> Result<(), Box<dyn Error>> {
    let swww = Swww::parse();

    if let Swww::ClearCache = &swww {
        cache::clean().map_err(|e| format!("failed to clean the cache: {e}"))?;
    }

    let socket = IpcSocket::connect()?;
    loop {
        RequestSend::Ping.send(&socket)?;
        let bytes = socket.recv()?;
        let answer = Answer::receive(bytes);
        if let Answer::Ping(configured) = answer {
            if configured {
                break;
            }
        } else {
            Err("Daemon did not return Answer::Ping, as expected")?;
        }
        thread::sleep(Duration::from_millis(1));
    }

    match swww {
        Swww::Clear(clear) => {
            let (format, _, _) = get_format_dims_and_outputs(&[], &socket)?;
            let mut color = clear.color;
            if format.must_swap_r_and_b_channels() {
                color.swap(0, 2);
            }
            let clear = ipc::ClearSend {
                color,
                outputs: split_cmdline_outputs(&clear.outputs)
                    .map(ToString::to_string)
                    .collect(),
            };
            RequestSend::Clear(clear.create_request()).send(&socket)?
        }
        Swww::Restore(restore) => {
            let requested_outputs: Box<_> = split_cmdline_outputs(&restore.outputs)
                .map(ToString::to_string)
                .collect();

            let (_, _, outputs) = get_format_dims_and_outputs(&requested_outputs, &socket)?;

            for output in outputs.iter().flatten().map(String::as_str) {
                let path = cache::get_previous_image_path(output)
                    .map_err(|err| format!("failed to get previous image path: {err}"))?;
                #[allow(deprecated)]
                let img = Img {
                    image: cli::parse_image(&path)?,
                    outputs: output.to_string(),
                    no_resize: false,
                    resize: ResizeStrategy::Crop,
                    fill_color: [0, 0, 0],
                    filter: cli::Filter::Lanczos3,
                    transition_type: cli::TransitionType::None,
                    transition_step: std::num::NonZeroU8::MAX,
                    transition_duration: 0.0,
                    transition_fps: 30,
                    transition_angle: 0.0,
                    transition_pos: cli::CliPosition {
                        x: cli::CliCoord::Pixel(0.0),
                        y: cli::CliCoord::Pixel(0.0),
                    },
                    invert_y: false,
                    transition_bezier: (0.0, 0.0, 0.0, 0.0),
                    transition_wave: (0.0, 0.0),
                };
                img_request(img, &socket)?.send(&socket)?;
            }
        }
        Swww::Img(img) => img_request(img, &socket)?.send(&socket)?,
        Swww::Kill => match RequestSend::Kill
            .send(&socket)
            .map(|()| socket.recv())?
            .map(Answer::receive)?
        {
            Answer::Ok => {
                #[cfg(debug_assertions)]
                let tries = 20;
                #[cfg(not(debug_assertions))]
                let tries = 10;
                let addr = IpcSocket::<Client>::path();
                let path = Path::new(addr);
                for _ in 0..tries {
                    if !path.exists() {
                        return Ok(());
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(format!("Could not confirm socket deletion at: {addr}"))?;
            }
            _ => unreachable!("invalid IPC response"),
        },
        Swww::Query => match RequestSend::Query
            .send(&socket)
            .map(|()| socket.recv())?
            .map(Answer::receive)?
        {
            Answer::Info(info) => info.iter().for_each(|info| println!("{info}")),
            _ => unreachable!("invalid IPC response"),
        },
        Swww::ClearCache => unreachable!("handled at the start of `main`"),
    };

    Ok(())
}

fn img_request(img: Img, socket: &IpcSocket<Client>) -> Result<RequestSend, Box<dyn Error>> {
    let requested_outputs: Box<_> = split_cmdline_outputs(&img.outputs)
        .map(ToString::to_string)
        .collect();
    let (format, dims, outputs) = get_format_dims_and_outputs(&requested_outputs, socket)?;
    // let imgbuf = ImgBuf::new(&img.path)?;

    let img_request = make_img_request(&img, &dims, format, &outputs)?;
    Ok(RequestSend::Img(img_request))
}

fn make_img_request(
    img: &cli::Img,
    dims: &[(u32, u32)],
    pixel_format: PixelFormat,
    outputs: &[Vec<String>],
) -> Result<Mmap, String> {
    let transition = make_transition(img);
    let mut img_req_builder = ipc::ImageRequestBuilder::new(transition);

    match &img.image {
        CliImage::Color(color) => {
            for (&dim, outputs) in dims.iter().zip(outputs) {
                img_req_builder.push(
                    ipc::ImgSend {
                        img: image::RgbImage::from_pixel(dim.0, dim.1, image::Rgb(*color))
                            .to_vec()
                            .into_boxed_slice(),
                        path: format!("0x{:02x}{:02x}{:02x}", color[0], color[1], color[2]),
                        dim,
                        format: pixel_format,
                    },
                    outputs,
                    None,
                );
            }
        }
        CliImage::Path(img_path) => {
            let imgbuf = ImgBuf::new(img_path)?;
            let img_raw = imgbuf.decode(pixel_format)?;

            for (&dim, outputs) in dims.iter().zip(outputs) {
                let path = match img_path.canonicalize() {
                    Ok(p) => p.to_string_lossy().to_string(),
                    Err(e) => {
                        if let Some("-") = img_path.to_str() {
                            "STDIN".to_string()
                        } else {
                            return Err(format!("failed no canonicalize image path: {e}"));
                        }
                    }
                };

                let animation = if !imgbuf.is_animated() {
                    None
                } else if img.resize == ResizeStrategy::Crop {
                    match cache::load_animation_frames(img_path, dim, pixel_format) {
                        Ok(Some(animation)) => Some(animation),
                        otherwise => {
                            if let Err(e) = otherwise {
                                eprintln!("Error loading cache for {:?}: {e}", img_path);
                            }

                            Some({
                                ipc::Animation {
                                    animation: compress_frames(
                                        imgbuf.as_frames()?,
                                        dim,
                                        pixel_format,
                                        make_filter(&img.filter),
                                        img.resize,
                                        &img.fill_color,
                                    )?
                                    .into_boxed_slice(),
                                }
                            })
                        }
                    }
                } else {
                    None
                };

                let img = match img.resize {
                    ResizeStrategy::No => img_pad(&img_raw, dim, &img.fill_color)?,
                    ResizeStrategy::Crop => {
                        img_resize_crop(&img_raw, dim, make_filter(&img.filter))?
                    }
                    ResizeStrategy::Fit => {
                        img_resize_fit(&img_raw, dim, make_filter(&img.filter), &img.fill_color)?
                    }
                };

                img_req_builder.push(
                    ipc::ImgSend {
                        img,
                        path,
                        dim,
                        format: pixel_format,
                    },
                    outputs,
                    animation,
                );
            }
        }
    }

    Ok(img_req_builder.build())
}

#[allow(clippy::type_complexity)]
fn get_format_dims_and_outputs(
    requested_outputs: &[String],
    socket: &IpcSocket<Client>,
) -> Result<(PixelFormat, Vec<(u32, u32)>, Vec<Vec<String>>), String> {
    let mut outputs: Vec<Vec<String>> = Vec::new();
    let mut dims: Vec<(u32, u32)> = Vec::new();
    let mut imgs: Vec<BgImg> = Vec::new();

    RequestSend::Query.send(socket)?;
    let bytes = socket.recv().map_err(|err| err.to_string())?;
    let answer = Answer::receive(bytes);
    let Answer::Info(infos) = answer else {
        unreachable!()
    };
    let mut format = PixelFormat::Xrgb;
    for info in infos.iter() {
        format = info.pixel_format;
        let info_img = &info.img;
        let name = info.name.to_string();
        if !requested_outputs.is_empty() && !requested_outputs.contains(&name) {
            continue;
        }
        let real_dim = info.real_dim();
        if let Some((_, output)) = dims
            .iter_mut()
            .zip(&imgs)
            .zip(&mut outputs)
            .find(|((dim, img), _)| real_dim == **dim && info_img == *img)
        {
            output.push(name);
        } else {
            outputs.push(vec![name]);
            dims.push(real_dim);
            imgs.push(info_img.clone());
        }
    }
    if outputs.is_empty() {
        Err("none of the requested outputs are valid".to_owned())
    } else {
        Ok((format, dims, outputs))
    }
}

fn split_cmdline_outputs(outputs: &str) -> impl Iterator<Item = &str> {
    outputs.split(',').filter(|s| !s.is_empty())
}
