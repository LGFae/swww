use std::{io::Read, time::Duration};
use structopt::StructOpt;

mod cli;
mod daemon;
use cli::Fswww;

fn main() -> Result<(), String> {
    let fswww = Fswww::from_args();
    if fswww.execute()? {
        wait_for_response()
    } else {
        Ok(())
    }
}

///Timeouts in 10 seconds
fn wait_for_response() -> Result<(), String> {
    let mut socket = cli::get_socket()?;
    let mut buf = String::with_capacity(100);
    let mut error = String::new();

    #[cfg(debug_assertions)]
    let tries = 40; //Some operations take a while to respond in debug mode
    #[cfg(not(debug_assertions))]
    let tries = 20;

    for _ in 0..tries {
        match socket.read_to_string(&mut buf) {
            Ok(_) => {
                if buf.starts_with("Ok\n") {
                    if buf.len() > 3 {
                        print!("{}", &buf[3..]);
                    }
                    return Ok(());
                } else if buf.starts_with("Err\n") {
                    return Err(format!("daemon sent back: {}", &buf[4..]));
                } else {
                    return Err(format!("daemon returned a badly formatted answer: {}", buf));
                }
            }
            Err(e) => error = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("Error while waiting for response: ".to_string() + &error)
}
