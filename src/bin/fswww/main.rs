use fork;
use image;
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use structopt::StructOpt;

#[derive(Debug)]
enum Filter {
    Nearest,
    Triangle,
    CatmullRom,
    Gaussian,
    Lanczos3,
}

impl std::str::FromStr for Filter {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Nearest" => Ok(Self::Nearest),
            "Triangle" => Ok(Self::Triangle),
            "CatmullRom" => Ok(Self::CatmullRom),
            "Gaussian" => Ok(Self::Gaussian),
            "Lanczos3" => Ok(Self::Lanczos3),
            _ => Err("Non existing filter".to_string()),
        }
    }
}

impl std::fmt::Display for Filter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nearest => write!(f, "Nearest"),
            Self::Triangle => write!(f, "Triangle"),
            Self::CatmullRom => write!(f, "CatmullRom"),
            Self::Gaussian => write!(f, "Gaussian"),
            Self::Lanczos3 => write!(f, "Lanczos3"),
        }
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = "fswww")]
///The Final Solution to your Wayland Wallpaper Woes
///
///Change what your monitors display as a background by controlling the fswww daemon at runtime.
///Supports animated gifs and putting different stuff in different monitors. I also did my best to
///make it as resource efficient as possible.
enum Fswww {
    /// Send an image (or animated gif) for the daemon to display
    Img {
        /// Path to the image to display
        #[structopt(parse(from_os_str))]
        file: PathBuf,

        /// Comma separated list of outputs to display the image at. If it isn't set, the image is
        /// displayed on all outputs
        #[structopt(short, long)]
        outputs: Option<String>,

        ///Filter to use when scaling images (run fswww img --help to see options).
        ///
        ///Available options are:
        ///
        ///Nearest | Triangle | CatmullRom | Gaussian | Lanczos3
        ///
        ///These are offered by the image crate (https://crates.io/crates/image). 'Nearest' is
        ///what I recommend for pixel art stuff, and ONLY for pixel art stuff. It is also the
        ///fastest filter.
        ///
        ///For non pixel art stuff, I would usually recommend one of the last three, though some
        ///experimentation will be necessary to see which one you like best. Also note they are
        ///all slower than Nearest. For some examples, see
        ///https://docs.rs/image/0.23.14/image/imageops/enum.FilterType.html.
        #[structopt(short, long, default_value = "Lanczos3")]
        filter: Filter,
    },

    ///Initialize the daemon. Exits if there is already a daemon running.
    Init {
        ///Don't fork the daemon. This will keep it running in the current terminal.
        ///
        ///The only advantage of this would be seeing the logging real time. Even then, for release
        ///builds we only log warnings and errors, so you won't be seeing much.
        ///
        ///Also, fswww waits for a signal from the daemon to indicate it initalized successfully,
        ///and running something like <fswww init --no-daemon &>, though it will sent the process
        ///to the background, will fail to receive that message properly. Furthermore, in this
        ///case you would have 2 processes running in the background, not one: the original parent
        ///fswww and the child fswww-daemon.
        #[structopt(long)]
        no_daemon: bool,
    },

    ///Kills the daemon
    Kill,

    ///Asks the daemon to print output information (names and dimensions). You may use this to find
    ///out valid values for the <fswww-img --outputs> option. If you want more detailed information
    ///about your outputs, I would recommed trying wlr-randr.
    Query,
}

fn spawn_daemon(no_daemon: bool) -> Result<(), String> {
    let mut cmd = Command::new("fswww-daemon");
    let spawn_err =
        "Failed to initialize fswww-daemon. Are you sure it is installed (and in the PATH)?";
    if no_daemon {
        if cmd.output().is_err() {
            return Err(spawn_err.to_string());
        };
    }
    match fork::daemon(false, false) {
        Ok(fork::Fork::Child) => {
            cmd.output().expect(spawn_err);
            Ok(())
        }
        Ok(fork::Fork::Parent(_)) => Ok(()),
        Err(_) => Err("Couldn't daemonize forked process!".to_string()),
    }
}

fn main() -> Result<(), String> {
    let opts = Fswww::from_args();
    match opts {
        Fswww::Init { no_daemon } => {
            if get_socket().is_err() {
                spawn_daemon(no_daemon)?;
            } else {
                return Err("There seems to already be another instance running...".to_string());
            }
            if no_daemon {
                //in this case, when the daemon stops we are done
                return Ok(());
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
                return Ok(());
            }
        }
        Fswww::Img {
            file,
            outputs,
            filter,
        } => send_img(file, outputs.unwrap_or("".to_string()), filter)?,
        Fswww::Query => send_request("__QUERY__")?,
    }

    wait_for_response()
}

fn send_img(path: PathBuf, outputs: String, filter: Filter) -> Result<(), String> {
    if let Err(e) = image::open(&path) {
        return Err(format!("Cannot open img {:?}: {}", path, e));
    }
    let abs_path = match path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return Err(format!("Failed to find absolute path: {}", e));
        }
    };
    let img_path_str = abs_path.to_str().unwrap();
    let msg = format!("__IMG__\n{}\n{}\n{}\n", filter, outputs, img_path_str);
    send_request(&msg)?;

    Ok(())
}

fn kill() -> Result<(), String> {
    let mut socket = get_socket()?;
    match socket.write(b"__DIE__") {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn wait_for_response() -> Result<(), String> {
    let mut socket = get_socket()?;
    match socket.set_read_timeout(Some(Duration::from_secs(10))) {
        Ok(()) => {
            let mut buf = String::with_capacity(100);
            match socket.read_to_string(&mut buf) {
                Ok(_) => {
                    if buf.starts_with("Ok\n") {
                        if buf.len() > 3 {
                            print!("{}", &buf[3..]);
                        }
                        Ok(())
                    } else {
                        Err(format!("ERROR: daemon sent back: {}", buf))
                    }
                }
                Err(e) => Err(e.to_string()),
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

fn send_request(request: &str) -> Result<(), String> {
    let mut socket = get_socket()?;
    match socket.write(request.as_bytes()) {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn get_socket() -> Result<UnixStream, String> {
    let path = get_socket_path();

    match UnixStream::connect(&path) {
        Ok(socket) => Ok(socket),
        Err(e) => match e.kind() {
            //This could happen during initialization, in which case we just wait
            //a little bit and try again
            std::io::ErrorKind::NotFound => {
                std::thread::sleep(Duration::from_millis(100));
                match UnixStream::connect(&path) {
                    Ok(socket) => Ok(socket),
                    Err(e) => Err(e.to_string()),
                }
            }
            _ => Err(e.to_string()),
        },
    }
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
