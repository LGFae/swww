use clap::Parser;
use hex::{self, FromHex};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

mod cli;
mod communication;
mod daemon;
use cli::{Clear, Filter, Fswww, Img};

fn main() -> Result<(), String> {
    let fswww = Fswww::parse();
    match &fswww {
        Fswww::Clear(clear) => send_clear(clear)?,
        Fswww::Init { no_daemon } => {
            if get_socket(1, 0).is_err() {
                spawn_daemon(*no_daemon)?;
            } else {
                return Err("There seems to already be another instance running...".to_string());
            }
            if *no_daemon {
                return Ok(());
            } else {
                send_request("__INIT__")?;
            }
        }
        Fswww::Kill => send_request("__KILL__")?,
        Fswww::Img(img) => send_img(img)?,
        Fswww::Query => send_request("__QUERY__")?,
    }

    wait_for_response()?;
    if let Fswww::Kill = fswww {
        let socket_path = get_socket_path();
        for _ in 0..10 {
            if !socket_path.exists() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        return Err(format!(
            "Could not confirm socket deletion at: {:?}",
            socket_path
        ));
    } else {
        Ok(())
    }
}

impl Filter {
    pub fn get_image_filter(&self) -> image::imageops::FilterType {
        match self {
            Self::Nearest => image::imageops::FilterType::Nearest,
            Self::Triangle => image::imageops::FilterType::Triangle,
            Self::CatmullRom => image::imageops::FilterType::CatmullRom,
            Self::Gaussian => image::imageops::FilterType::Gaussian,
            Self::Lanczos3 => image::imageops::FilterType::Lanczos3,
        }
    }
}

impl std::str::FromStr for Fswww {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut lines = s.lines();
        match lines.next() {
            Some(cmd) => match cmd {
                "__CLEAR__" => {
                    let color = lines.next();
                    let outputs = lines.next();

                    if color.is_none() || outputs.is_none() {
                        return Err("badly formatted clear request".to_string());
                    }

                    let color = <[u8; 3]>::from_hex(color.unwrap());
                    if let Err(e) = color {
                        return Err(format!("badly formatted clear request: {}", e));
                    }
                    let color = color.unwrap();

                    Ok(Self::Clear(Clear {
                        outputs: outputs.unwrap().to_string(),
                        color,
                    }))
                }
                "__INIT__" => Ok(Self::Init { no_daemon: false }),
                "__KILL__" => Ok(Self::Kill),
                "__QUERY__" => Ok(Self::Query),
                "__IMG__" => {
                    let file = lines.next();
                    let outputs = lines.next();
                    let filter = lines.next();
                    let transition_step = lines.next();

                    if filter.is_none()
                        || outputs.is_none()
                        || file.is_none()
                        || transition_step.is_none()
                    {
                        return Err("badly formatted img request".to_string());
                    }

                    Ok(Self::Img(Img {
                        path: PathBuf::from_str(file.unwrap()).unwrap(),
                        outputs: outputs.unwrap().to_string(),
                        filter: Filter::from_str(filter.unwrap())?,
                        transition_step: transition_step.unwrap().parse().unwrap(),
                    }))
                }
                _ => Err(format!("unrecognized command: {}", cmd)),
            },
            None => Err("empty request!".to_string()),
        }
    }
}

fn spawn_daemon(no_daemon: bool) -> Result<(), String> {
    if no_daemon {
        daemon::main();
    }
    match fork::fork() {
        Ok(fork::Fork::Child) => match fork::daemon(false, false) {
            Ok(fork::Fork::Child) => {
                daemon::main();
                Ok(())
            }
            Ok(fork::Fork::Parent(_)) => Ok(()),
            Err(_) => Err("Couldn't daemonize process!".to_string()),
        },
        Ok(fork::Fork::Parent(_)) => Ok(()),
        Err(_) => Err("Couldn't fork process!".to_string()),
    }
}

fn send_clear(clear: &Clear) -> Result<(), String> {
    let msg = format!(
        "__CLEAR__\n{}\n{}\n",
        hex::encode(clear.color),
        clear.outputs
    );
    send_request(&msg)
}

///This tests if the img exsits and can be openned before sending it
fn send_img(img: &Img) -> Result<(), String> {
    if let Err(e) = image::open(&img.path) {
        return Err(format!("Cannot open img {:?}: {}", img.path, e));
    }
    let abs_path = match img.path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return Err(format!("Failed to find absolute path: {}", e));
        }
    };
    let img_path_str = abs_path.to_str().unwrap();
    let msg = format!(
        "__IMG__\n{}\n{}\n{}\n{}\n",
        img_path_str, img.outputs, img.filter, img.transition_step
    );
    send_request(&msg)
}

fn send_request(request: &str) -> Result<(), String> {
    let mut socket = get_socket(5, 100)?;
    let timeout = Duration::from_millis(500);
    if let Err(e) = socket.set_write_timeout(Some(timeout)) {
        return Err(format!("Failed to set write timeout: {}", e));
    };

    match socket.write_all(request.as_bytes()) {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// We make sure the Stream is always set to blocking mode
///
/// * `tries` -  how make times to attempt the connection
/// * `interval` - how long to wait between attempts, in milliseconds
fn get_socket(tries: u8, interval: u64) -> Result<UnixStream, String> {
    //Make sure we try at least once
    let tries = if tries == 0 { 1 } else { tries };
    let path = get_socket_path();
    let mut error = String::new();
    for _ in 0..tries {
        match UnixStream::connect(&path) {
            Ok(socket) => {
                if let Err(e) = socket.set_nonblocking(false) {
                    return Err(format!("Failed to set blocking connection: {}", e));
                }
                return Ok(socket);
            }
            Err(e) => error = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(interval));
    }
    Err("Failed to connect to socket: ".to_string() + &error)
}

fn get_socket_path() -> PathBuf {
    let runtime_dir = if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        dir
    } else {
        "/tmp/fswww".to_string()
    };
    let runtime_dir = Path::new(&runtime_dir);
    runtime_dir.join("fswww.socket")
}

///Timeouts in 10 seconds in release and in 20 in debug
fn wait_for_response() -> Result<(), String> {
    let mut socket = get_socket(5, 100)?;
    let mut buf = String::with_capacity(100);

    #[cfg(debug_assertions)]
    let tries = 40; //Some operations take a while to respond in debug mode
    #[cfg(not(debug_assertions))]
    let tries = 20;

    let timeout = Duration::from_millis(500);
    if let Err(e) = socket.set_read_timeout(Some(timeout)) {
        return Err(format!("Failed to set read timeout: {}", e));
    };
    for _ in 0..tries {
        match socket.read_to_string(&mut buf) {
            Ok(_) => {
                if let Some(answer) = buf.strip_prefix("Ok\n") {
                    print!("{}", answer);
                    return Ok(());
                } else if let Some(answer) = buf.strip_prefix("Err\n") {
                    return Err(format!("daemon sent back: {}", answer));
                } else {
                    return Err(format!("daemon returned a badly formatted answer: {}", buf));
                }
            }
            Err(e) => {
                //If the error is a timeout we just try again
                if let std::io::ErrorKind::TimedOut = e.kind() {
                    return Err(e.to_string());
                }
            }
        }
    }
    Err("daemon response wasn't sent.".to_string())
}
