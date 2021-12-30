use fork;
use structopt::StructOpt;
mod daemon;

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
}

fn main() {
    let opts = Fswww::from_args();
    match opts {
        Fswww::Init { no_daemon } => {
            if !no_daemon {
                if let Ok(fork::Fork::Child) = fork::daemon(false, false) {
                    daemon::main();
                } else {
                    eprintln!("Couldn't fork process!");
                }
            } else {
                daemon::main();
            }
        }
        Fswww::Kill => kill(),
    }
}

fn kill() {}
