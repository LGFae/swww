use clap::Parser;
use std::{
    io::Read,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

mod cli;
mod daemon;
use cli::{Filter, Fswww};

fn main() -> Result<(), String> {
    let mut fswww = Fswww::parse();
    if let Fswww::Init { no_daemon } = fswww {
        if get_socket(1, 0).is_err() {
            spawn_daemon(no_daemon)?;
            if no_daemon {
                return Ok(());
            }
        } else {
            return Err("There seems to already be another instance running...".to_string());
        }
    }

    let mut socket = get_socket(5, 100)?;
    fswww.send(&socket)?;
    wait_for_response(&mut socket)?;

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

impl Fswww {
    pub fn send(&mut self, stream: &UnixStream) -> Result<(), String> {
        if let Fswww::Img(img) = self {
            img.path = match img.path.canonicalize() {
                Ok(p) => p,
                Err(e) => return Err(format!("Coulnd't get absolute path: {}", e)),
            };
            if img.transition_step == 0 {
                eprintln!("A transition_step of 0 is invalid! Defaulting to 20...");
                img.transition_step = 20;
            }
        }
        match bincode::serialize_into(stream, self) {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("Failed to serialize request: {}", e)),
        }
    }

    pub fn receive(stream: &mut UnixStream) -> Result<Self, String> {
        match bincode::deserialize_from(stream) {
            Ok(i) => Ok(i),
            Err(e) => Err(format!("Failed to serialize request: {}", e)),
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
fn wait_for_response(socket: &mut UnixStream) -> Result<(), String> {
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
