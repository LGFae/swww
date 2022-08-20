use clap::Parser;
use std::{os::unix::net::UnixStream, time::Duration};

mod cli;
mod communication;
mod daemon;
use cli::Swww;
use communication::Answer;

fn main() -> Result<(), String> {
    let mut swww = Swww::parse();
    if let Swww::Init { no_daemon } = &swww {
        if connect_to_socket(1, 0).is_err() {
            spawn_daemon(*no_daemon)?;
            if *no_daemon {
                return Ok(());
            }
        } else {
            return Err("There seems to already be another instance running...".to_string());
        }
    }

    let socket = connect_to_socket(5, 100)?;
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
                let socket_path = communication::get_socket_path();
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
fn connect_to_socket(tries: u8, interval: u64) -> Result<UnixStream, String> {
    //Make sure we try at least once
    let tries = if tries == 0 { 1 } else { tries };
    let path = communication::get_socket_path();
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
