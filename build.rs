use std::io::Error;

use clap::CommandFactory;
use clap_complete::{generate_to, Shell};

include!("src/cli.rs");

const COMPLETION_DIR: &str = "completions";
const APP_NAME: &str = "swww";

fn main() -> Result<(), Error> {
    let outdir = completion_dir()?;
    let mut app = Swww::command();

    let shells = [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::Elvish];
    for shell in shells {
        let comp_file = generate_to(shell, &mut app, APP_NAME, &outdir)?;
        println!("cargo:warning=generated shell completion file: {comp_file:?}");
    }
    Ok(())
}

fn completion_dir() -> std::io::Result<PathBuf> {
    let path = PathBuf::from(COMPLETION_DIR);
    if !path.is_dir() {
        std::fs::create_dir(&path)?;
    }
    Ok(path)
}
