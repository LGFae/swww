use hex::{self, FromHex};
use std::{
    io::Write,
    os::unix::{net::UnixStream, prelude::PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

use super::daemon;
use structopt::StructOpt;

#[derive(Debug)]
pub enum Filter {
    Nearest,
    Triangle,
    CatmullRom,
    Gaussian,
    Lanczos3,
}

impl std::str::FromStr for Filter {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
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

#[derive(Debug, StructOpt)]
#[structopt(name = "fswww")]
///The Final Solution to your Wayland Wallpaper Woes
///
///Change what your monitors display as a background by controlling the fswww daemon at runtime.
///Supports animated gifs and putting different stuff in different monitors. I also did my best to
///make it as resource efficient as possible.
pub enum Fswww {
    ///Fills the specified outputs with the given color (Defaults to filling all outputs with
    ///black).
    Clear(Clear),

    /// Send an image (or animated gif) for the daemon to display
    Img(Img),

    /// Initialize the daemon. Exits if there is already a daemon running.
    ///
    /// We check it by seeing if $XDG_RUNTIME_DIR/fswww.socket exists.
    Init {
        ///Don't fork the daemon. This will keep it running in the current terminal.
        ///
        ///The only advantage of this would be seeing the logging real time. Even then, for release
        ///builds we only log warnings and errors, so you won't be seeing much (ideally).
        #[structopt(long)]
        no_daemon: bool,
    },

    ///Kills the daemon
    Kill,

    ///Asks the daemon to print output information (names and dimensions). You may use this to find
    ///out valid values for the <fswww-img --outputs> option. If you want more detailed information
    ///about your outputs, I would recommed trying wlr-randr.
    Query,

    ///Display an arbitrary stream of bytes, printed by a separate program, as the wallpaper. The
    ///program will be initialized with the outputs width and height as arguments, in that order.
    Stream(Stream),
}

#[derive(Debug, StructOpt)]
pub struct Clear {
    /// Color to fill the screen with. Must be given in rrggbb format (note there is no prepended
    /// '#').
    #[structopt(parse(try_from_str = <[u8; 3]>::from_hex), default_value = "000000")]
    pub color: [u8; 3],

    /// Comma separated list of outputs to display the image at. If it isn't set, the image is
    /// displayed on all outputs
    #[structopt(short, long, default_value = "")]
    pub outputs: String,
    //TODO: Also transition!!
}

#[derive(Debug, StructOpt)]
pub struct Img {
    /// Path to the image to display
    #[structopt(parse(from_os_str))]
    pub path: PathBuf,

    /// Comma separated list of outputs to display the image at. If it isn't set, the image is
    /// displayed on all outputs
    #[structopt(short, long, default_value = "")]
    pub outputs: String,

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
    pub filter: Filter,

    ///By default, fswww will try to smooth transitions between images. If you'd rather the image
    ///be loaded immediately, you may activate this flag
    ///
    #[structopt(long)]
    pub no_transition: bool,
}

#[derive(Debug, StructOpt)]
pub struct Stream {
    /// Path to the program that will offer the stream to display
    #[structopt(parse(from_os_str))]
    pub path: PathBuf,

    /// Comma separated list of outputs to display the stream at. If it isn't set, the stream will
    /// be displayed on all outputs
    #[structopt(short, long, default_value = "")]
    pub outputs: String,

    /// Set this flag if the program will print the DIFFERENCE between one frame and the last, as
    /// opposed to simply printing the whole frame everytime. Run --help for extra details.
    ///
    /// The format of the byte vector is as follows:
    ///
    /// First we have a header byte, that will indicate which of the next pair of bytes must be
    /// redrawn. For example, assuming we are at the start of the vector, the byte
    ///
    /// 1010 0000
    ///
    /// would indicate that pixels in position 0, 1, 4 and 5 have changed.
    ///
    /// Following the header we have the bytes of the changed pixels. In the example above, we
    /// would first have the new bytes of pixel in position 0, followed by the new bytes in pixel
    /// of position 1, followed by those of pixel in position 4, and so on.
    ///
    /// After this is done we rinse and repeat for the next group of 16 pixels.
    #[structopt(short, long)]
    pub diff_mode: bool,
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
                super::wait_for_response()?;
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
            Fswww::Stream(stream) => send_stream(&stream),
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
                    let no_transition = lines.next();

                    if filter.is_none() || outputs.is_none() || file.is_none() {
                        return Err("badly formatted img request".to_string());
                    }

                    Ok(Self::Img(Img {
                        path: PathBuf::from_str(file.unwrap()).unwrap(),
                        outputs: outputs.unwrap().to_string(),
                        filter: Filter::from_str(filter.unwrap())?,
                        no_transition: no_transition.unwrap().parse().unwrap(),
                    }))
                }
                "__STREAM__" => {
                    let file = lines.next();
                    let outputs = lines.next();
                    let diff_mode = lines.next();

                    if diff_mode.is_none() || outputs.is_none() || file.is_none() {
                        return Err("badly formatted img request".to_string());
                    }

                    Ok(Self::Stream(Stream {
                        path: PathBuf::from_str(file.unwrap()).unwrap(),
                        outputs: outputs.unwrap().to_string(),
                        diff_mode: diff_mode.unwrap().parse().unwrap(),
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
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            send_request("__INIT__").unwrap();
        });
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
        img_path_str, img.outputs, img.filter, img.no_transition
    );
    send_request(&msg)
}

///Tests if file passed exists and is executable
fn send_stream(stream: &Stream) -> Result<bool, String> {
    let metadata = std::fs::metadata(&stream.path);
    if let Err(e) = metadata {
        return Err(format!(
            "Cannot read metadata from {:?}: {}",
            stream.path, e
        ));
    }
    let metadata = metadata.unwrap();
    if !metadata.is_file() {
        return Err(format!("{:?} is not a file!", stream.path));
    }
    let permissions = metadata.permissions();
    let is_exe = permissions.mode() & 0o111 != 0;
    if !is_exe {
        return Err(format!("File {:?} is not executable!", stream.path));
    }

    let abs_path = match stream.path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return Err(format!("Failed to find absolute path: {}", e));
        }
    };
    let stream_path_str = abs_path.to_str().unwrap();
    let msg = format!(
        "__STREAM__\n{}\n{}\n{}\n",
        stream_path_str, stream.outputs, stream.diff_mode
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
