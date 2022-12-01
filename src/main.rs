use clap::Parser;
use fast_image_resize::{FilterType, PixelType, Resizer};
use std::{
    num::NonZeroU32,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use utils::communication::{self, get_socket_path, Answer, Request};
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
                        return Err(format!("failed to delete previous socket: {}", e));
                    }
                }
            }
            Ok(true) => {
                return Err("There seems to already be another instance running...".to_string())
            }
            Err(e) => {
                eprintln!("WARNING: failed to read '/proc' directory to determine whether the daemon is running: {}
                          Falling back to trying to checking if the socket file exists...", e);
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

    if std::env::var("SWWW_TRANSITION_SPEED").is_ok() {
        eprintln!(
            "WARNING: the environment variable SWWW_TRANSITION_SPEED no longer does anything.\n\
            What used to be 'speed' is now controlled by the flags '--transition-bezier' and\n\
            '--transition-duration'. See swww img help for the full information.\n\
\n\
            This warning will go away in future versions of this program"
        );
    }

    let mut request = make_request(&swww);
    let socket = connect_to_socket(5, 100)?;
    request.send(&socket)?;
    match Answer::receive(socket)? {
        Answer::Err(msg) => return Err(msg),
        Answer::Info(info) => info.into_iter().for_each(|i| println!("{}", i)),
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
                    "Could not confirm socket deletion at: {:?}",
                    socket_path
                ));
            }
        }
    }
    Ok(())
}

fn make_request(args: &Swww) -> Request {
    match args {
        Swww::Clear(c) => Request::Clear(communication::Clear {
            color: c.color,
            outputs: c.outputs.split(' ').map(|s| s.to_string()).collect(),
        }),
        Swww::Img(img) => Request::Img(make_img_request(img)),
        Swww::Init { .. } => Request::Init,
        Swww::Kill => Request::Kill,
        Swww::Query => Request::Query,
    }
}

fn make_img_request(
    img: &cli::Img,
) -> (
    communication::Transition,
    Vec<(communication::Img, Vec<String>)>,
) {
    let img_raw = image::open(&img.path).unwrap().into_rgba8();
    let transition = communication::Transition {
        transition_type: convert_transition_type(&img.transition_type),
        duration: img.transition_duration,
        step: img.transition_step,
        fps: img.transition_fps,
        angle: img.transition_angle,
        pos: img.transition_pos,
        bezier: img.transition_bezier,
    };
    let socket = connect_to_socket(5, 100).unwrap();
    Request::Query.send(&socket).unwrap();
    let answer = Answer::receive(socket).unwrap();
    match answer {
        Answer::Info(infos) => {
            let mut outputs: Vec<Vec<String>> = Vec::with_capacity(infos.len());
            let mut dims: Vec<(u32, u32)> = Vec::with_capacity(infos.len());
            let mut imgs: Vec<communication::BgImg> = Vec::with_capacity(infos.len());

            for info in infos {
                let mut should_add = true;
                for (i, (dim, img)) in dims.iter().zip(&imgs).enumerate() {
                    if info.dim == *dim && info.img == *img {
                        outputs[i].push(info.name.clone());
                        should_add = false;
                        break;
                    }
                }

                if should_add {
                    outputs.push(vec![info.name]);
                    dims.push(info.dim);
                    imgs.push(info.img);
                }
            }

            let mut unique_requests = Vec::with_capacity(dims.len());
            for (dim, outputs) in dims.into_iter().zip(outputs) {
                unique_requests.push((
                    communication::Img {
                        img: img_resize(img_raw.clone(), dim, make_filter(&img.filter)).unwrap(),
                    },
                    outputs,
                ));
            }

            (transition, unique_requests)
        }
        _ => unreachable!(),
    }
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

fn img_resize(
    img: image::RgbaImage,
    dimensions: (u32, u32),
    filter: FilterType,
) -> Result<Vec<u8>, String> {
    let (width, height) = dimensions;
    let (img_w, img_h) = img.dimensions();
    let mut resized_img = if (img_w, img_h) != (width, height) {
        let mut src = match fast_image_resize::Image::from_vec_u8(
            // We unwrap bellow because we know the images's dimensions should never be 0
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

        // We unwrap bellow because we know the outputs's dimensions should never be 0
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

        image::RgbaImage::from_vec(width, height, dst.into_vec()).unwrap()
    } else {
        img
    };

    // The ARGB is 'little endian', so here we must  put the order
    // of bytes 'in reverse', so it needs to be BGRA.
    for pixel in resized_img.pixels_mut() {
        pixel.0.swap(0, 2);
    }

    Ok(resized_img.into_raw())
}

///Behold: the most stupid function ever
fn convert_transition_type(a: &cli::TransitionType) -> communication::TransitionType {
    match a {
        cli::TransitionType::Simple => communication::TransitionType::Simple,
        cli::TransitionType::Left => communication::TransitionType::Left,
        cli::TransitionType::Right => communication::TransitionType::Right,
        cli::TransitionType::Top => communication::TransitionType::Top,
        cli::TransitionType::Bottom => communication::TransitionType::Bottom,
        cli::TransitionType::Center => communication::TransitionType::Center,
        cli::TransitionType::Outer => communication::TransitionType::Outer,
        cli::TransitionType::Any => communication::TransitionType::Any,
        cli::TransitionType::Random => communication::TransitionType::Random,
        cli::TransitionType::Wipe => communication::TransitionType::Wipe,
        cli::TransitionType::Grow => communication::TransitionType::Grow,
    }
}

fn spawn_daemon(no_daemon: bool) -> Result<(), String> {
    let mut cmd = "./target/debug/swww-daemon";
    #[cfg(not(debug_assertions))] {
        cmd = "./target/release/swww-daemon";
    }
    if no_daemon {
        std::process::Command::new(cmd).status().unwrap();
        return Ok(());
    }
    match fork::fork() {
        Ok(fork::Fork::Child) => match fork::daemon(false, false) {
            Ok(fork::Fork::Child) => {
                std::process::Command::new(cmd).spawn().unwrap();
                Ok(())
            }
            Ok(fork::Fork::Parent(_)) => Ok(()),
            Err(_) => Err("Couldn't daemonize process!".to_string()),
        },
        Ok(fork::Fork::Parent(_)) => Ok(()),
        Err(_) => Err("Couldn't fork process!".to_string()),
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
                    return Err(format!("Failed to set blocking connection: {}", e));
                }
                return Ok(socket);
            }
            Err(e) => error = Some(e),
        }
        std::thread::sleep(Duration::from_millis(interval));
    }
    let error = error.unwrap();
    if error.kind() == std::io::ErrorKind::NotFound {
        return Err("Socket file not found. Are you sure the daemon is running?".to_string());
    }

    Err(format!("Failed to connect to socket: {}", error))
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
                    if arg0.ends_with("swww") {
                        if let Some("init") = args.next() {
                            return Ok(true);
                        }
                    }
                }
            }
        }
    }

    Ok(false)
}
