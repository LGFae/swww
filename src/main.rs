use clap::Parser;
use std::{
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

mod cli;
mod communication;
mod daemon;
use cli::Swww;
use communication::Answer;

fn main() -> Result<(), String> {
    let mut swww = Swww::parse();
    if let Swww::Init {
        no_daemon,
        img,
        color,
    } = &swww
    {
        if get_socket(1, 0).is_err() {
            spawn_daemon(*no_daemon)?;
            if *no_daemon {
                return Ok(());
            }
            if img.is_some() || color.is_some() {
                eprintln!(
                    "WARNING: options -i and -c for swww init are deprecated!Use
    swww init && swww img <path/to/img>
or
    swww init && swww clear <color>
instead."
                );
            }
        } else {
            return Err("There seems to already be another instance running...".to_string());
        }
    }

    let socket = get_socket(5, 100)?;
    swww.send(&socket)?;
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
        "/tmp/swww".to_string()
    };
    let runtime_dir = Path::new(&runtime_dir);
    runtime_dir.join("swww.socket")
}
