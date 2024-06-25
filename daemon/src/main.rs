//! All expects in this program must be carefully chosen on purpose. The idea is that if any of
//! them fail there is no point in continuing. All of the initialization code, for example, is full
//! of `expects`, **on purpose**, because we **want** to unwind and exit when they happen

use std::error::Error;
use std::fs;
use std::io;
use std::io::IsTerminal;
use std::io::Write;
use std::path::Path;
use std::thread;
use std::time::Instant;

use cli::Cli;
use daemon::Daemon;

use log::debug;
use log::error;
use log::info;
use log::warn;
use log::Level;
use log::LevelFilter;

use common::ipc::IpcSocket;
use common::ipc::Server;
use log::SetLoggerError;

mod animations;
mod cli;
mod daemon;
mod wallpaper;
#[allow(dead_code)]
mod wayland;

fn main() -> Result<(), Box<dyn Error>> {
    // first, get the command line arguments and make the logger
    let cli = Cli::new();
    Logger::init(cli.quiet)?;
    Daemon::handle_signals();

    // initialize the wayland connection, getting all the necessary globals
    let initializer = wayland::globals::init(cli.format);

    let listener = IpcSocket::server()?;

    // use the initializer to create the Daemon, then drop it to free up the memory
    let mut daemon = Daemon::new(initializer, !cli.no_cache)?;

    if let Ok(true) = sd_notify::booted() {
        if let Err(e) = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]) {
            error!("Error sending status update to systemd: {e}");
        }
    }

    daemon.main_loop(listener)?;

    info!("Goodbye!");
    Ok(())
}

struct Logger {
    filter: LevelFilter,
    start: Instant,
    term: bool,
}

impl Logger {
    fn init(quiet: bool) -> Result<(), SetLoggerError> {
        let filter = if quiet {
            LevelFilter::Error
        } else {
            LevelFilter::Debug
        };

        let logger = Self {
            filter,
            start: Instant::now(),
            term: io::stderr().is_terminal(),
        };

        log::set_max_level(filter);
        log::set_boxed_logger(Box::new(logger))
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.filter
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let time = self.start.elapsed().as_millis();

        let level = if self.term {
            match record.level() {
                Level::Error => "\x1b[31m[ERROR]\x1b[0m",
                Level::Warn => "\x1b[33m[WARN]\x1b[0m ",
                Level::Info => "\x1b[32m[INFO]\x1b[0m ",
                Level::Debug | Level::Trace => "\x1b[36m[DEBUG]\x1b[0m",
            }
        } else {
            match record.level() {
                Level::Error => "[ERROR]",
                Level::Warn => "[WARN] ",
                Level::Info => "[INFO] ",
                Level::Debug | Level::Trace => "[DEBUG]",
            }
        };

        let thread = thread::current();
        let thread = thread.name().unwrap_or("???");
        let msg = record.args();

        let _ = io::stderr()
            .lock()
            .write_fmt(format_args!("{time:>8}ms {level} ({thread}) {msg}\n"));
    }

    fn flush(&self) {
        //no op (we do not buffer anything)
    }
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
