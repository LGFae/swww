use fork;
mod daemon;

fn main() {
    if let Ok(fork::Fork::Child) = fork::daemon(true, true) {
        daemon::main();
    }
}
