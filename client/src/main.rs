use clap::Parser;
use std::time::Duration;

use common::{
    cache,
    ipc::{self, connect_to_socket, get_socket_path, read_socket, Answer, RequestSend},
};

mod imgproc;
use imgproc::*;

mod cli;
use cli::{CliImage, ResizeStrategy, Swww};

fn main() -> Result<(), String> {
    let swww = Swww::parse();

    if let Swww::ClearCache = &swww {
        return cache::clean().map_err(|e| format!("failed to clean the cache: {e}"));
    }

    loop {
        let socket = connect_to_socket(&get_socket_path(), 5, 100)?;
        RequestSend::Ping.send(&socket)?;
        let bytes = read_socket(&socket)?;
        let answer = Answer::receive(bytes);
        if let Answer::Ping(configured) = answer {
            if configured {
                break;
            }
        } else {
            return Err("Daemon did not return Answer::Ping, as expected".to_string());
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    process_swww_args(&swww)
}

fn process_swww_args(args: &Swww) -> Result<(), String> {
    let request = match make_request(args)? {
        Some(request) => request,
        None => return Ok(()),
    };
    let socket = connect_to_socket(&get_socket_path(), 5, 100)?;
    request.send(&socket)?;
    let bytes = read_socket(&socket)?;
    drop(socket);
    match Answer::receive(bytes) {
        Answer::Err(msg) => return Err(msg.to_string()),
        Answer::Info(info) => info.iter().for_each(|i| println!("{}", i)),
        Answer::Ok => {
            if let Swww::Kill = args {
                #[cfg(debug_assertions)]
                let tries = 20;
                #[cfg(not(debug_assertions))]
                let tries = 10;
                let socket_path = get_socket_path();
                for _ in 0..tries {
                    if !socket_path.exists() {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                return Err(format!(
                    "Could not confirm socket deletion at: {socket_path:?}"
                ));
            }
        }
        Answer::Ping(_) => {
            return Ok(());
        }
    }
    Ok(())
}

fn make_request(args: &Swww) -> Result<Option<RequestSend>, String> {
    match args {
        Swww::Clear(c) => {
            let (format, _, _) = get_format_dims_and_outputs(&[])?;
            let mut color = c.color;
            if format.must_swap_r_and_b_channels() {
                color.swap(0, 2);
            }
            let clear = ipc::ClearSend {
                color,
                outputs: split_cmdline_outputs(&c.outputs),
            };
            Ok(Some(RequestSend::Clear(clear.create_request())))
        }
        Swww::Restore(restore) => {
            let requested_outputs = split_cmdline_outputs(&restore.outputs);
            restore_from_cache(&requested_outputs)?;
            Ok(None)
        }
        Swww::ClearCache => unreachable!("there is no request for clear-cache"),
        Swww::Img(img) => {
            let requested_outputs = split_cmdline_outputs(&img.outputs);
            let (format, dims, outputs) = get_format_dims_and_outputs(&requested_outputs)?;
            // let imgbuf = ImgBuf::new(&img.path)?;

            let img_request = make_img_request(img, &dims, format, &outputs)?;

            Ok(Some(RequestSend::Img(img_request)))
        }
        Swww::Kill => Ok(Some(RequestSend::Kill)),
        Swww::Query => Ok(Some(RequestSend::Query)),
    }
}

fn make_img_request(
    img: &cli::Img,
    dims: &[(u32, u32)],
    pixel_format: ipc::PixelFormat,
    outputs: &[Vec<String>],
) -> Result<ipc::Mmap, String> {
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
) -> Result<(ipc::PixelFormat, Vec<(u32, u32)>, Vec<Vec<String>>), String> {
    let mut outputs: Vec<Vec<String>> = Vec::new();
    let mut dims: Vec<(u32, u32)> = Vec::new();
    let mut imgs: Vec<ipc::BgImg> = Vec::new();

    let socket = connect_to_socket(&get_socket_path(), 5, 100)?;
    RequestSend::Query.send(&socket)?;
    let bytes = read_socket(&socket)?;
    drop(socket);
    let answer = Answer::receive(bytes);
    match answer {
        Answer::Info(infos) => {
            let mut format = ipc::PixelFormat::Xrgb;
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
        Answer::Err(e) => Err(format!("daemon error when sending query: {e}")),
        _ => unreachable!(),
    }
}

fn split_cmdline_outputs(outputs: &str) -> Box<[String]> {
    outputs
        .split(',')
        .map(|s| s.to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

fn restore_from_cache(requested_outputs: &[String]) -> Result<(), String> {
    let (_, _, outputs) = get_format_dims_and_outputs(requested_outputs)?;

    for output in outputs.iter().flatten() {
        let img_path = common::cache::get_previous_image_path(output)
            .map_err(|e| format!("failed to get previous image path: {e}"))?;
        #[allow(deprecated)]
        if let Err(e) = process_swww_args(&Swww::Img(cli::Img {
            image: cli::parse_image(&img_path)?,
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
        })) {
            eprintln!("WARNING: failed to load cache for output {output}: {e}");
        }
    }

    Ok(())
}
