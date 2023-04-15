use clap::Parser;
use fast_image_resize::{FilterType, PixelType, Resizer};
use image::{codecs::gif::GifDecoder, AnimationDecoder, RgbaImage};
use std::{
    fs::File,
    io::{stdin, BufReader, Read},
    num::NonZeroU32,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use utils::{
    communication::{self, get_socket_path, AnimationRequest, Answer, Coord, Position, Request},
    comp_decomp::BitPack,
};

mod cli;
use cli::Swww;

fn main() -> Result<(), String> {
    let swww = Swww::parse();
    if let Swww::Init { no_daemon } = &swww {
        match is_daemon_running() {
            Ok(false) => {
                let socket_path = get_socket_path();
                if socket_path.exists() {
                    eprintln!(
                        "WARNING: socket file {} was not deleted when the previous daemon exited",
                        socket_path.to_string_lossy()
                    );
                    if let Err(e) = std::fs::remove_file(socket_path) {
                        return Err(format!("failed to delete previous socket: {e}"));
                    }
                }
            }
            Ok(true) => {
                return Err("There seems to already be another instance running...".to_string())
            }
            Err(e) => {
                eprintln!("WARNING: failed to read '/proc' directory to determine whether the daemon is running: {e}
                          Falling back to trying to checking if the socket file exists...");
                let socket_path = get_socket_path();
                if socket_path.exists() {
                    return Err(format!(
                        "Found socket at {}. There seems to be an instance already running...",
                        socket_path.to_string_lossy()
                    ));
                }
            }
        }
        spawn_daemon(*no_daemon)?;
        if *no_daemon {
            return Ok(());
        }
    }

    let request = make_request(&swww)?;
    let socket = connect_to_socket(5, 100)?;
    request.send(&socket)?;
    match Answer::receive(socket)? {
        Answer::Err(msg) => return Err(msg),
        Answer::Info(info) => info.into_iter().for_each(|i| println!("{i}")),
        Answer::Ok => {
            if let Swww::Kill = swww {
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
    }
    Ok(())
}

fn make_request(args: &Swww) -> Result<Request, String> {
    match args {
        Swww::Clear(c) => Ok(Request::Clear(communication::Clear {
            color: c.color,
            outputs: split_cmdline_outputs(&c.outputs),
        })),
        Swww::Img(img) => {
            let requested_outputs = split_cmdline_outputs(&img.outputs);
            let (dims, outputs) = get_dimensions_and_outputs(requested_outputs)?;
            let (img_raw, is_gif) = read_img(&img.path)?;
            if is_gif {
                match std::thread::scope(|s| {
                    let animations = s.spawn(|| make_animation_request(img, &dims, &outputs));
                    let img_request = make_img_request(img, img_raw, &dims, &outputs)?;
                    let animations = match animations.join() {
                        Ok(a) => a,
                        Err(e) => Err(format!("{e:?}")),
                    };
                    let socket = connect_to_socket(5, 100)?;
                    Request::Img(img_request).send(&socket)?;
                    Answer::receive(socket)?;
                    animations
                }) {
                    Ok(animations) => Ok(Request::Animation(animations)),
                    Err(e) => Err(format!("failed to create animated request: {e}")),
                }
            } else {
                Ok(Request::Img(make_img_request(
                    img, img_raw, &dims, &outputs,
                )?))
            }
        }
        Swww::Init { .. } => Ok(Request::Init),
        Swww::Kill => Ok(Request::Kill),
        Swww::Query => Ok(Request::Query),
    }
}

fn split_cmdline_outputs(outputs: &str) -> Vec<String> {
    outputs
        .split(',')
        .map(|s| s.to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

fn read_img(path: &Path) -> Result<(RgbaImage, bool), String> {
    if let Some("-") = path.to_str() {
        let mut reader = BufReader::new(stdin());
        let mut buffer = Vec::new();
        if let Err(e) = reader.read_to_end(&mut buffer) {
            return Err(format!("failed to read stdin: {e}"));
        }

        return match image::load_from_memory(&buffer) {
            Ok(img) => Ok((img.into_rgba8(), false)),
            Err(e) => return Err(format!("failed load image from memory: {e}")),
        };
    }

    let imgbuf = match image::io::Reader::open(path) {
        Ok(img) => img,
        Err(e) => return Err(format!("failed to open image: {e}")),
    };

    let imgbuf = match imgbuf.with_guessed_format() {
        Ok(img) => img,
        Err(e) => return Err(format!("failed to detect the image's format: {e}")),
    };

    let is_gif = imgbuf.format() == Some(image::ImageFormat::Gif);
    match imgbuf.decode() {
        Ok(img) => Ok((img.into_rgba8(), is_gif)),
        Err(e) => Err(format!("failed to decode image: {e}")),
    }
}

fn make_img_request(
    img: &cli::Img,
    img_raw: image::RgbaImage,
    dims: &[(u32, u32)],
    outputs: &[Vec<String>],
) -> Result<communication::ImageRequest, String> {
    let transition = make_transition(img);
    let mut unique_requests = Vec::with_capacity(dims.len());
    for (dim, outputs) in dims.iter().zip(outputs) {
        unique_requests.push((
            communication::Img {
                img: if img.no_resize {
                    img_pad(img_raw.clone(), *dim, &img.fill_color)?
                } else {
                    img_resize(img_raw.clone(), *dim, make_filter(&img.filter))?
                },
                path: match img.path.canonicalize() {
                    Ok(p) => p,
                    Err(e) => {
                        if let Some("-") = img.path.to_str() {
                            PathBuf::from("STDIN")
                        } else {
                            return Err(format!("failed no canonicalize image path: {e}"));
                        }
                    }
                },
            },
            outputs.to_owned(),
        ));
    }

    Ok((transition, unique_requests))
}

#[allow(clippy::type_complexity)]
fn get_dimensions_and_outputs(
    requested_outputs: Vec<String>,
) -> Result<(Vec<(u32, u32)>, Vec<Vec<String>>), String> {
    let mut outputs: Vec<Vec<String>> = Vec::new();
    let mut dims: Vec<(u32, u32)> = Vec::new();
    let mut imgs: Vec<communication::BgImg> = Vec::new();

    let socket = connect_to_socket(5, 100)?;
    Request::Query.send(&socket)?;
    let answer = Answer::receive(socket)?;
    match answer {
        Answer::Info(infos) => {
            for info in infos {
                if !requested_outputs.is_empty() && !requested_outputs.contains(&info.name) {
                    continue;
                }
                let mut should_add = true;
                let real_dim = (
                    info.dim.0 * info.scale_factor as u32,
                    info.dim.1 * info.scale_factor as u32,
                );
                for (i, (dim, img)) in dims.iter().zip(&imgs).enumerate() {
                    if real_dim == *dim && info.img == *img {
                        outputs[i].push(info.name.clone());
                        should_add = false;
                        break;
                    }
                }

                if should_add {
                    outputs.push(vec![info.name]);
                    dims.push(real_dim);
                    imgs.push(info.img);
                }
            }
            if outputs.is_empty() {
                Err("none of the requested outputs are valid".to_owned())
            } else {
                Ok((dims, outputs))
            }
        }
        Answer::Err(e) => Err(format!("failed to query swww-daemon: {e}")),
        _ => unreachable!(),
    }
}

fn make_animation_request(
    img: &cli::Img,
    dims: &[(u32, u32)],
    outputs: &[Vec<String>],
) -> Result<AnimationRequest, String> {
    let filter = make_filter(&img.filter);
    let mut animations = Vec::with_capacity(dims.len());
    for (dim, outputs) in dims.iter().zip(outputs) {
        let imgbuf = match image::io::Reader::open(&img.path) {
            Ok(img) => img.into_inner(),
            Err(e) => return Err(format!("error opening image during animation: {e}")),
        };
        let gif = match GifDecoder::new(imgbuf) {
            Ok(gif) => gif,
            Err(e) => return Err(format!("failed to decode gif during animation: {e}")),
        };
        animations.push((
            communication::Animation {
                animation: compress_frames(gif, *dim, filter, img.no_resize, &img.fill_color)?
                    .into_boxed_slice(),
                sync: img.sync,
            },
            outputs.to_owned(),
        ));
    }
    Ok(animations)
}

fn compress_frames(
    gif: GifDecoder<BufReader<File>>,
    dim: (u32, u32),
    filter: FilterType,
    no_resize: bool,
    color: &[u8; 3],
) -> Result<Vec<(BitPack, Duration)>, String> {
    let mut compressed_frames = Vec::new();
    let mut frames = gif.into_frames();

    // The first frame should always exist
    let first = frames.next().unwrap().unwrap();
    let first_duration = first.delay().numer_denom_ms();
    let first_duration = Duration::from_millis((first_duration.0 / first_duration.1).into());
    let first_img = if no_resize {
        img_pad(first.into_buffer(), dim, color)?
    } else {
        img_resize(first.into_buffer(), dim, filter)?
    };

    let mut canvas = first_img.clone();
    while let Some(Ok(frame)) = frames.next() {
        let (dur_num, dur_div) = frame.delay().numer_denom_ms();
        let duration = Duration::from_millis((dur_num / dur_div).into());

        let img = if no_resize {
            img_pad(frame.into_buffer(), dim, color)?
        } else {
            img_resize(frame.into_buffer(), dim, filter)?
        };

        compressed_frames.push((BitPack::pack(&mut canvas, &img)?, duration));
    }
    //Add the first frame we got earlier:
    compressed_frames.push((BitPack::pack(&mut canvas, &first_img)?, first_duration));

    Ok(compressed_frames)
}

fn make_filter(filter: &cli::Filter) -> fast_image_resize::FilterType {
    match filter {
        cli::Filter::Nearest => fast_image_resize::FilterType::Box,
        cli::Filter::Bilinear => fast_image_resize::FilterType::Bilinear,
        cli::Filter::CatmullRom => fast_image_resize::FilterType::CatmullRom,
        cli::Filter::Mitchell => fast_image_resize::FilterType::Mitchell,
        cli::Filter::Lanczos3 => fast_image_resize::FilterType::Lanczos3,
    }
}

fn img_pad(
    mut img: image::RgbaImage,
    dimensions: (u32, u32),
    color: &[u8; 3],
) -> Result<Vec<u8>, String> {
    let (padded_w, padded_h) = dimensions;
    let (padded_w, padded_h) = (padded_w as usize, padded_h as usize);
    let mut padded = Vec::with_capacity(padded_w * padded_w * 4);

    let img = image::imageops::crop(&mut img, 0, 0, dimensions.0, dimensions.1).to_image();
    let (img_w, img_h) = img.dimensions();
    let (img_w, img_h) = (img_w as usize, img_h as usize);
    let raw_img = img.into_vec();

    for _ in 0..(((padded_h - img_h) / 2) * padded_w) {
        padded.push(color[2]);
        padded.push(color[1]);
        padded.push(color[0]);
        padded.push(255);
    }

    for row in 0..img_h {
        for _ in 0..(padded_w - img_w) / 2 {
            padded.push(color[2]);
            padded.push(color[1]);
            padded.push(color[0]);
            padded.push(255);
        }

        for pixel in raw_img[(row * img_w * 4)..((row + 1) * img_w * 4)].chunks_exact(4) {
            padded.push(pixel[2]);
            padded.push(pixel[1]);
            padded.push(pixel[0]);
            padded.push(pixel[3]);
        }
        for _ in 0..(padded_w - img_w) / 2 {
            padded.push(color[2]);
            padded.push(color[1]);
            padded.push(color[0]);
            padded.push(255);
        }
    }

    while padded.len() < (padded_h * padded_w * 4) {
        padded.push(color[2]);
        padded.push(color[1]);
        padded.push(color[0]);
        padded.push(255);
    }

    Ok(padded)
}

fn img_resize(
    img: image::RgbaImage,
    dimensions: (u32, u32),
    filter: FilterType,
) -> Result<Vec<u8>, String> {
    let (width, height) = dimensions;
    let (img_w, img_h) = img.dimensions();
    let mut resized_img = if (img_w, img_h) != (width, height) {
        let mut src = match fast_image_resize::Image::from_vec_u8(
            // We unwrap below because we know the images's dimensions should never be 0
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

        // We unwrap below because we know the outputs's dimensions should never be 0
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

        dst.into_vec()
    } else {
        img.into_vec()
    };

    // The ARGB is 'little endian', so here we must  put the order
    // of bytes 'in reverse', so it needs to be BGRA.
    for pixel in resized_img.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    Ok(resized_img)
}

fn make_transition(img: &cli::Img) -> communication::Transition {
    let mut angle = img.transition_angle;

    let x = match img.transition_pos.x {
        cli::CliCoord::Percent(x) => {
            if !(0.0..=1.0).contains(&x) {
                println!(
                    "Warning: x value not in range [0,1] position might be set outside screen: {x}"
                );
            }
            Coord::Percent(x)
        }
        cli::CliCoord::Pixel(x) => Coord::Pixel(x),
    };

    let y = match img.transition_pos.y {
        cli::CliCoord::Percent(y) => {
            if !(0.0..=1.0).contains(&y) {
                println!(
                    "Warning: y value not in range [0,1] position might be set outside screen: {y}"
                );
            }
            Coord::Percent(y)
        }
        cli::CliCoord::Pixel(y) => Coord::Pixel(y),
    };

    let mut pos = Position::new(x, y);

    let transition_type = match img.transition_type {
        cli::TransitionType::Simple => communication::TransitionType::Simple,
        cli::TransitionType::Wipe => communication::TransitionType::Wipe,
        cli::TransitionType::Outer => communication::TransitionType::Outer,
        cli::TransitionType::Grow => communication::TransitionType::Grow,
        cli::TransitionType::Wave => communication::TransitionType::Wave,
        cli::TransitionType::Right => {
            angle = 0.0;
            communication::TransitionType::Wipe
        }
        cli::TransitionType::Top => {
            angle = 90.0;
            communication::TransitionType::Wipe
        }
        cli::TransitionType::Left => {
            angle = 180.0;
            communication::TransitionType::Wipe
        }
        cli::TransitionType::Bottom => {
            angle = 270.0;
            communication::TransitionType::Wipe
        }
        cli::TransitionType::Center => {
            pos = Position::new(Coord::Percent(0.5), Coord::Percent(0.5));
            communication::TransitionType::Grow
        }
        cli::TransitionType::Any => {
            pos = Position::new(
                Coord::Percent(rand::random::<f32>()),
                Coord::Percent(rand::random::<f32>()),
            );
            if rand::random::<u8>() % 2 == 0 {
                communication::TransitionType::Grow
            } else {
                communication::TransitionType::Outer
            }
        }
        cli::TransitionType::Random => {
            pos = Position::new(
                Coord::Percent(rand::random::<f32>()),
                Coord::Percent(rand::random::<f32>()),
            );
            angle = rand::random();
            match rand::random::<u8>() % 4 {
                0 => communication::TransitionType::Simple,
                1 => communication::TransitionType::Wipe,
                2 => communication::TransitionType::Outer,
                3 => communication::TransitionType::Grow,
                _ => unreachable!(),
            }
        }
    };

    communication::Transition {
        duration: img.transition_duration,
        step: img.transition_step,
        fps: img.transition_fps,
        bezier: img.transition_bezier,
        angle,
        pos,
        transition_type,
        wave: img.transition_wave,
    }
}

fn spawn_daemon(no_daemon: bool) -> Result<(), String> {
    let cmd = "swww-daemon";
    if no_daemon {
        match std::process::Command::new(cmd).status() {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("error spawning swww-daemon: {e}")),
        }
    } else {
        match std::process::Command::new(cmd)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("error spawning swww-daemon: {e}")),
        }
    }
}

/// We make sure the Stream is always set to blocking mode
///
/// * `tries` -  how make times to attempt the connection
/// * `interval` - how long to wait between attempts, in milliseconds
fn connect_to_socket(tries: u8, interval: u64) -> Result<UnixStream, String> {
    //Make sure we try at least once
    let tries = if tries == 0 { 1 } else { tries };
    let path = get_socket_path();
    let mut error = None;
    for _ in 0..tries {
        match UnixStream::connect(&path) {
            Ok(socket) => {
                if let Err(e) = socket.set_nonblocking(false) {
                    return Err(format!("Failed to set blocking connection: {e}"));
                }
                return Ok(socket);
            }
            Err(e) => error = Some(e),
        }
        std::thread::sleep(Duration::from_millis(interval));
    }
    let error = error.unwrap();
    if error.kind() == std::io::ErrorKind::NotFound {
        return Err("Socket file not found. Are you sure swww-daemon is running?".to_string());
    }

    Err(format!("Failed to connect to socket: {error}"))
}

fn is_daemon_running() -> Result<bool, String> {
    let proc = PathBuf::from("/proc");

    let entries = match proc.read_dir() {
        Ok(e) => e,
        Err(e) => return Err(e.to_string()),
    };

    for entry in entries.flatten() {
        let dirname = entry.file_name();
        if let Ok(pid) = dirname.to_string_lossy().parse::<u32>() {
            if std::process::id() == pid {
                continue;
            }
            let mut entry_path = entry.path();
            entry_path.push("cmdline");
            if let Ok(cmd) = std::fs::read_to_string(entry_path) {
                let mut args = cmd.split(&[' ', '\0']);
                if let Some(arg0) = args.next() {
                    if arg0.ends_with("swww-daemon") {
                        return Ok(true);
                    }
                }
            }
        }
    }

    Ok(false)
}
