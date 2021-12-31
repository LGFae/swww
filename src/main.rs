use fork;
use nix::{sys::signal, unistd::Pid};
use std::{fs, path, process::exit};
use structopt::StructOpt;
mod daemon;

const PID_FILE: &str = "/tmp/fswww/pid";

#[derive(Debug, StructOpt)]
#[structopt(
    name = "fswww",
    about = "The Final Solution to your Wayland Wallpaper Woes"
)]
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
    Img { path: String },
}

fn main() {
    let opts = Fswww::from_args();
    match opts {
        Fswww::Init { no_daemon } => {
            if !already_running() {
                if !no_daemon {
                    if let Ok(fork::Fork::Child) = fork::daemon(false, false) {
                        daemon::main();
                    } else {
                        eprintln!("Couldn't fork process!");
                        exit(1);
                    }
                } else {
                    daemon::main();
                }
            } else {
                eprintln!("There seems to already be another instance running...");
                exit(1);
            }
        }
        Fswww::Kill => kill(),
        Fswww::Img { path } => send_img(&path),
    }
}

fn send_img(path: &str) {
    let pid = get_daemon_pid(); //Do this first because we exit if we can't find it

    let path = path::Path::new(path);
    let abs_path = match path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            exit(1);
        }
    };
    let mut img_path_str = abs_path.to_str().unwrap().to_owned();
    img_path_str.push('\n');
    fs::write("/tmp/fswww/in", img_path_str)
        .expect("Couldn't write to /tmp/fswww/in. Did you delete the file?");

    signal::kill(Pid::from_raw(pid), signal::SIGUSR1).expect("Failed to send signal.");
}

fn kill() {
    let pid = get_daemon_pid();

    signal::kill(Pid::from_raw(pid), signal::SIGKILL).expect("Failed to kill daemon...");

    fs::remove_dir_all("/tmp/fswww").expect("Failed to remove /tmp/fswww directory.");

    println!("Successfully killed fswww daemon and removed /tmp/fswww directory!");
}

fn get_daemon_pid() -> i32 {
    let pid_file_path = path::Path::new(PID_FILE);
    if !pid_file_path.exists() {
        eprintln!(
            "pid file {} doesn't exist. Are you sure the daemon is running?",
            PID_FILE
        );
        exit(1);
    }
    fs::read_to_string(pid_file_path)
        .expect("Failed to read pid file")
        .parse()
        .unwrap()
}

fn already_running() -> bool {
    path::Path::new("/tmp/fswww").exists()
}
