//! All expects in this program must be carefully chosen on purpose. The idea is that if any of
//! them fail there is no point in continuing. All of the initialization code, for example, is full
//! of `expects`, **on purpose**, because we **want** to unwind and exit when they happen

use std::error::Error;
use std::fs;
use std::io::IsTerminal;
use std::io::Write;
use std::mem;
use std::path::Path;
use std::ptr;

use daemon::Daemon;

use log::debug;
use log::error;
use log::info;
use log::warn;
use log::LevelFilter;

use common::ipc::IpcSocket;
use common::ipc::Server;

mod animations;
mod cli;
mod daemon;
mod wallpaper;
#[allow(dead_code)]
mod wayland;

fn main() -> Result<(), Box<dyn Error>> {
    // first, get the command line arguments and make the logger
    let cli = cli::Cli::new();
    make_logger(cli.quiet);
    Daemon::handle_signals();

    // initialize the wayland connection, getting all the necessary globals
    let initializer = wayland::globals::init(cli.format);

    // create the socket listener and setup the signal handlers
    // this will also return an error if there is an `swww-daemon` instance already exists
    // TODO: use `Daemon` constructor to do this
    let addr = IpcSocket::<Server>::path();
    let path = Path::new(addr);
    if path.exists() {
        if Daemon::socket_occupied() {
            Err("There is an swww-daemon instance already running on this socket!")?;
        } else {
            warn!("socket file '{addr}' was not deleted when the previous daemon exited",);
            if let Err(err) = fs::remove_file(addr) {
                Err(format!("failed to delete previous socket: {err}"))?;
            }
        }
    } else {
        let Some(parent) = path.parent() else {
            Err("couldn't find a valid runtime directory")?
        };
        if !parent.exists() {
            if let Err(err) = fs::create_dir(parent) {
                Err(format!("failed to create runtime dir: {err}"))?
            };
        }
    }

    let listener = IpcSocket::server()?;
    debug!("Created socket in {:?}", addr);

    // use the initializer to create the Daemon, then drop it to free up the memory
    let mut daemon = Daemon::new(initializer, !cli.no_cache);

    if let Ok(true) = sd_notify::booted() {
        if let Err(e) = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]) {
            error!("Error sending status update to systemd: {e}");
        }
    }

    daemon.main_loop(listener)?;

    info!("Goodbye!");
    Ok(())
}

fn setup_signals() {
    Daemon::handle_signals()
}

struct Logger {
    level_filter: LevelFilter,
    start: std::time::Instant,
    is_term: bool,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level_filter
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let time = self.start.elapsed().as_millis();

            let level = if self.is_term {
                match record.level() {
                    log::Level::Error => "\x1b[31m[ERROR]\x1b[0m",
                    log::Level::Warn => "\x1b[33m[WARN]\x1b[0m ",
                    log::Level::Info => "\x1b[32m[INFO]\x1b[0m ",
                    log::Level::Debug | log::Level::Trace => "\x1b[36m[DEBUG]\x1b[0m",
                }
            } else {
                match record.level() {
                    log::Level::Error => "[ERROR]",
                    log::Level::Warn => "[WARN] ",
                    log::Level::Info => "[INFO] ",
                    log::Level::Debug | log::Level::Trace => "[DEBUG]",
                }
            };

            let thread = std::thread::current();
            let thread_name = thread.name().unwrap_or("???");
            let msg = record.args();

            let _ = std::io::stderr()
                .lock()
                .write_fmt(format_args!("{time:>8}ms {level} ({thread_name}) {msg}\n"));
        }
    }

    fn flush(&self) {
        //no op (we do not buffer anything)
    }
}

fn make_logger(quiet: bool) {
    let level_filter = if quiet {
        LevelFilter::Error
    } else {
        LevelFilter::Debug
    };

    log::set_boxed_logger(Box::new(Logger {
        level_filter,
        start: std::time::Instant::now(),
        is_term: std::io::stderr().is_terminal(),
    }))
    .map(|()| log::set_max_level(level_filter))
    .unwrap();
}

/// copy-pasted from the `spin_sleep` crate on crates.io
///
/// This will sleep for an amount of time we can roughly expected the OS to still be precise enough
/// for frame timing (125 us, currently).
fn spin_sleep(duration: std::time::Duration) {
    const ACCURACY: std::time::Duration = std::time::Duration::new(0, 125_000);
    let start = std::time::Instant::now();
    if duration > ACCURACY {
        std::thread::sleep(duration - ACCURACY);
    }

    while start.elapsed() < duration {
        std::thread::yield_now();
    }
}
