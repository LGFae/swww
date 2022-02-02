use std::{io::Error, path::Path};

use structopt::clap::App;

include!("src/cli.rs");

const COMPLETION_DIR: &str = "completions";
const APP: &str = "fswww";
const FILE: &str = "_fswww";

fn main() -> Result<(), Error> {
    let outdir = completion_dir()?;
    let mut app = Fswww::clap();

    bash_completion(&mut app, &outdir)?;
    zsh_completion(&mut app, &outdir)?;
    fish_completion(&mut app, &outdir)?;

    println!(
        "cargo:warning=completion file is generated: {}",
        COMPLETION_DIR
    );

    Ok(())
}

fn bash_completion(app: &mut App, dir: &Path) -> std::io::Result<()> {
    let dir = dir.join("bash");
    if !dir.is_dir() {
        std::fs::create_dir(&dir)?;
    }
    let file = dir.join(FILE);
    let mut file = std::fs::File::create(file)?;

    app.gen_completions_to(APP, structopt::clap::Shell::Bash, &mut file);

    Ok(())
}

fn zsh_completion(app: &mut App, dir: &Path) -> std::io::Result<()> {
    let dir = dir.join("zsh");
    if !dir.is_dir() {
        std::fs::create_dir(&dir)?;
    }
    let file = dir.join(FILE);
    let mut file = std::fs::File::create(file)?;

    app.gen_completions_to(APP, structopt::clap::Shell::Zsh, &mut file);

    Ok(())
}

fn fish_completion(app: &mut App, dir: &Path) -> std::io::Result<()> {
    let dir = dir.join("fish");
    if !dir.is_dir() {
        std::fs::create_dir(&dir)?;
    }
    let file = dir.join(FILE);
    let mut file = std::fs::File::create(file)?;

    app.gen_completions_to(APP, structopt::clap::Shell::Fish, &mut file);

    Ok(())
}

fn completion_dir() -> std::io::Result<PathBuf> {
    let path = PathBuf::from(COMPLETION_DIR);
    if !path.is_dir() {
        std::fs::create_dir(&path)?;
    }
    Ok(path)
}
