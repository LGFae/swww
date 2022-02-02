use hex::{self, FromHex};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};
use structopt::StructOpt;

mod cli;
mod daemon;
use cli::{Clear, Filter, Fswww, Img};

fn main() -> Result<(), String> {
    let fswww = Fswww::from_args();
    if fswww.execute()? {
        wait_for_response()
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
    ///Returns whether we should wait for response or not
    pub fn execute(&self) -> Result<bool, String> {
        match self {
            Fswww::Clear(clear) => send_clear(&clear),
            //TODO: refactor this so that spawn_daemon already returns the correct result
            //(including sending the request)
            Fswww::Init { no_daemon } => {
                if get_socket().is_err() {
                    spawn_daemon(*no_daemon)?;
                } else {
                    return Err("There seems to already be another instance running...".to_string());
                }
                if *no_daemon {
                    Ok(false)
                } else {
                    send_request("__INIT__")
                }
            }
            Fswww::Kill => {
                kill()?;
                wait_for_response()?;
                let socket_path = get_socket_path();
                if let Err(e) = std::fs::remove_file(socket_path) {
                    return Err(format!("{}", e));
                } else {
                    println!("Stopped daemon and removed socket.");
                    return Ok(false);
                }
            }
            Fswww::Img(img) => send_img(&img),
            Fswww::Query => send_request("__QUERY__"),
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
    match fork::daemon(false, false) {
        Ok(fork::Fork::Child) => {
            daemon::main();
            Ok(())
        }
        Ok(fork::Fork::Parent(_)) => Ok(()),
        Err(_) => Err("Couldn't daemonize process!".to_string()),
    }
}

fn send_clear(clear: &Clear) -> Result<bool, String> {
    let msg = format!(
        "__CLEAR__\n{}\n{}\n",
        hex::encode(clear.color),
        clear.outputs
    );
    send_request(&msg)
}

///This tests if the img exsits and can be openned before sending it
fn send_img(img: &Img) -> Result<bool, String> {
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

fn kill() -> Result<(), String> {
    let mut socket = get_socket()?;
    match socket.write(b"__KILL__") {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn send_request(request: &str) -> Result<bool, String> {
    let mut socket = get_socket()?;
    let mut error = String::new();

    for _ in 0..5 {
        match socket.write_all(request.as_bytes()) {
            Ok(_) => return Ok(true),
            Err(e) => error = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("Failed to send request: ".to_string() + &error)
}

///Always sets connection to nonblocking. This is because the daemon will always listen to
///connections in a nonblocking fashion, so it makes sense to make this the standard for the whole
///program. The only difference is that we will have to make timeouts manually by trying to connect
///several times in a row, waiting some time between every attempt.
pub fn get_socket() -> Result<UnixStream, String> {
    let path = get_socket_path();
    let mut error = String::new();
    //We try to connect 5 fives, waiting 100 milis in between
    for _ in 0..5 {
        match UnixStream::connect(&path) {
            Ok(socket) => {
                if let Err(e) = socket.set_nonblocking(true) {
                    return Err(format!(
                        "Failed to set nonblocking connection: {}",
                        e.to_string()
                    ));
                }
                return Ok(socket);
            }
            Err(e) => error = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(100));
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

///Timeouts in 10 seconds
fn wait_for_response() -> Result<(), String> {
    let mut socket = get_socket()?;
    let mut buf = String::with_capacity(100);
    let mut error = String::new();

    #[cfg(debug_assertions)]
    let tries = 40; //Some operations take a while to respond in debug mode
    #[cfg(not(debug_assertions))]
    let tries = 20;

    for _ in 0..tries {
        match socket.read_to_string(&mut buf) {
            Ok(_) => {
                if buf.starts_with("Ok\n") {
                    if buf.len() > 3 {
                        print!("{}", &buf[3..]);
                    }
                    return Ok(());
                } else if buf.starts_with("Err\n") {
                    return Err(format!("daemon sent back: {}", &buf[4..]));
                } else {
                    return Err(format!("daemon returned a badly formatted answer: {}", buf));
                }
            }
            Err(e) => error = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("Error while waiting for response: ".to_string() + &error)
}
