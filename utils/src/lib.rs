use simplelog::{ColorChoice, LevelFilter, TermLogger, TerminalMode, ThreadLogMode};

pub mod communication;
pub mod comp_decomp;

pub fn make_logger() {
    let config = simplelog::ConfigBuilder::new()
        .set_thread_level(LevelFilter::Info) //let me see where the processing is happening
        .set_thread_mode(ThreadLogMode::Both)
        .build();

    TermLogger::init(
        LevelFilter::Debug,
        config,
        TerminalMode::Stderr,
        ColorChoice::AlwaysAnsi,
    )
    .expect("Failed to initialize logger. Cancelling...");
}
