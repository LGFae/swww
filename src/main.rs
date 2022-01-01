use fork;
use image;
use nix::{
    libc,
    sys::signal::{self, SigHandler, Signal},
    unistd::{self, Pid},
};
use std::{
    convert::TryFrom,
    fs,
    path::{Path, PathBuf},
};
use structopt::StructOpt;

mod daemon;

const PID_FILE: &str = "/tmp/fswww/pid";

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
    ///Initialize the daemon. Exits if there is already a daemon running
    Init {
        ///Don't fork the daemon. This will keep it running in the current
        ///terminal, so you can track its log, for example
        #[structopt(long)]
        no_daemon: bool,
    },

    ///Kills the daemon
    Kill,

    /// Send an img for the daemon to display
    Img {
        /// Path to the image to display
        #[structopt(parse(from_os_str))]
        file: PathBuf,

        /// Comma separated list of outputs to display the image at. If it isn't set, the image is
        /// displayed on all outputs
        #[structopt(short, long)]
        outputs: Option<String>,

        ///Filter to use when scaling images. Available options are:
        ///
        ///        Nearest
        ///        Triangle
        ///        CatmullRom
        ///        Gaussian
        ///        Lanczos3
        ///
        ///These are offered by the image crate (https://crates.io/crates/image).
        ///'Nearest' is what I recommend for pixel art stuff, and ONLY for pixel
        ///art stuff. It is also the fastest filter.
        ///For non pixel art stuff, I would usually recommend one of the last three,
        ///though some experimentation will be necessary to see which one you like
        ///best. Also note they are all slower than Nearest. For some examples, see
        ///https://docs.rs/image/0.23.14/image/imageops/enum.FilterType.html.
        #[structopt(short, long, default_value = "Lanczos3", verbatim_doc_comment)]
        filter: Filter,
    },
}

fn main() -> Result<(), String> {
    let opts = Fswww::from_args();
    match opts {
        Fswww::Init { no_daemon } => {
            if get_daemon_pid().is_err() {
                if no_daemon {
                    daemon::main(None);
                    return Ok(());
                }
                let this_pid = std::process::id() as i32;
                match fork::fork() {
                    Ok(fork::Fork::Child) => {
                        if let Ok(fork::Fork::Child) = fork::daemon(false, false) {
                            daemon::main(Some(this_pid));
                        } else {
                            return Err("Couldn't daemonize forked process!".to_string());
                        }
                    }
                    Ok(fork::Fork::Parent(pid)) => {
                        println!("Daemon pid = {}", pid);
                    }
                    Err(_) => {
                        return Err("Coulnd't fork process!".to_string());
                    }
                }
            } else {
                return Err("There seems to already be another instance running...".to_string());
            }
        }
        Fswww::Kill => kill()?,
        Fswww::Img {
            file,
            outputs,
            filter,
        } => send_img(file, outputs.unwrap_or("".to_string()))?,
    }

    wait_for_response()
}

fn send_img(path: PathBuf, outputs: String) -> Result<(), String> {
    if let Err(e) = image::open(&path) {
        return Err(format!("Cannot open img {:?}: {}", path, e));
    }
    let pid = get_daemon_pid()?;

    let abs_path = match path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return Err(format!("Failed to find absolute path: {}", e));
        }
    };
    let img_path_str = abs_path.to_str().unwrap();
    let msg = format!("{}\n{}\n{}\n", std::process::id(), outputs, img_path_str);
    fs::write("/tmp/fswww/in", msg)
        .expect("Couldn't write to /tmp/fswww/in. Did you delete the file?");

    signal::kill(Pid::from_raw(pid as i32), signal::SIGUSR1).expect("Failed to send signal.");
    Ok(())
}

extern "C" fn handle_sigusr(signal: libc::c_int) {
    let signal = Signal::try_from(signal).unwrap();
    if signal == Signal::SIGUSR1 {
        println!("Success!");
    } else if signal == Signal::SIGUSR2 {
        eprintln!("FAILED...");
    }
}

fn wait_for_response() -> Result<(), String> {
    let handler = SigHandler::Handler(handle_sigusr);
    unsafe {
        signal::signal(signal::SIGUSR1, handler)
            .expect("Couldn't register signal handler for usr1");
        signal::signal(signal::SIGUSR2, handler)
            .expect("Couldn't register signal handler for usr2");
    }
    let time_slept = unistd::sleep(10);
    if time_slept == 0 {
        return Err("Timeout waiting for daemon!".to_string());
    }
    Ok(())
}

fn kill() -> Result<(), String> {
    let pid = get_daemon_pid()?;

    let msg = format!("{}\n", std::process::id());
    fs::write("/tmp/fswww/in", msg)
        .expect("Couldn't write to /tmp/fswww/in. Did you delete the file?");

    signal::kill(Pid::from_raw(pid as i32), signal::SIGUSR2)
        .expect("Failed to send signal to kill daemon...");

    Ok(())
}

fn get_daemon_pid() -> Result<u32, String> {
    let pid_file_path = Path::new(PID_FILE);
    if !pid_file_path.exists() {
        return Err(format!(
            "pid file {} doesn't exist. Are you sure the daemon is running?",
            PID_FILE
        ));
    }
    let pid = fs::read_to_string(pid_file_path).expect("Failed to read pid file");

    //if the daemon exits unexpectably, the pid file will exist, but the pid in the file will no
    //longer be valid, and we might send the signal to the wrong process! So we check for that.
    let proc_file = "/proc/".to_owned() + &pid + "/cmdline";
    let program = fs::read_to_string(&proc_file)
        .expect(&("Couldn't read ".to_owned() + &proc_file + " to check if pid is correct")); //TODO: BETTER MESSAGE IF PROBLEM IS MISSING FILE
    println!("{}", program);

    //NOTE: since all calls to fswww (except --help) demand a subcommand, this will always have at
    //least two elements
    let mut args = program.split('\0');
    if !args.next().unwrap().ends_with("fswww") {
        return Err(format!(
            "Pid in {} refers a different program than the fswww daemon. It was probably terminated abnormaly and is no longer running.",
            PID_FILE

               ));
    }
    if args.next().unwrap() != "init" {
        return Err(format!(
            "Pid in {} refers a different instance of fswww than the daemon. The daemon was probably terminated abnormaly and is no longer running.",
            PID_FILE));
    }

    Ok(pid.parse().unwrap())
}
