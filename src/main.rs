use clap::Parser;
use image::codecs::gif::GifDecoder;
use std::{os::unix::net::UnixStream, path::PathBuf, process::Stdio, time::Duration};

use utils::communication::{self, get_socket_path, AnimationRequest, Answer, Request};

mod imgproc;
use imgproc::*;

mod cli;
use cli::{ResizeStrategy, Swww};

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

fn make_img_request(
    img: &cli::Img,
    img_raw: image::RgbImage,
    dims: &[(u32, u32)],
    outputs: &[Vec<String>],
) -> Result<communication::ImageRequest, String> {
    let transition = make_transition(img);
    let mut unique_requests = Vec::with_capacity(dims.len());
    for (dim, outputs) in dims.iter().zip(outputs) {
        unique_requests.push((
            communication::Img {
                img: match img.resize {
                    ResizeStrategy::No => img_pad(img_raw.clone(), *dim, &img.fill_color)?,
                    ResizeStrategy::Crop => {
                        img_resize_crop(img_raw.clone(), *dim, make_filter(&img.filter))?
                    }
                    ResizeStrategy::Fit => img_resize_fit(
                        img_raw.clone(),
                        *dim,
                        make_filter(&img.filter),
                        &img.fill_color,
                    )?,
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
                animation: compress_frames(gif, *dim, filter, img.resize, &img.fill_color)?
                    .into_boxed_slice(),
                sync: img.sync,
            },
            outputs.to_owned(),
        ));
    }
    Ok(animations)
}

fn split_cmdline_outputs(outputs: &str) -> Vec<String> {
    outputs
        .split(',')
        .map(|s| s.to_owned())
        .filter(|s| !s.is_empty())
        .collect()
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
