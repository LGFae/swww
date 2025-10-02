use std::io::Error;

use clap::{CommandFactory, value_parser};
use clap_complete::{Shell, generate_to};

include!("src/cli.rs");

const COMPLETION_DIR: &str = "../completions";
const APP_NAME: &str = "swww";

fn main() -> Result<(), Error> {
    let outdir = completion_dir()?;
    let mut app = Swww::command();

    // we must change the value parser for the img subcommand argument to a PathBuf so that the
    // generator creates the correct autocompletion that suggests filepaths to our users
    for cmd in app.get_subcommands_mut() {
        if cmd.get_name() == "img" {
            *cmd = cmd
                .clone()
                .mut_arg("image", |arg| arg.value_parser(value_parser!(PathBuf)));
            break;
        }
    }

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
