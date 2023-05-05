//! Implements basic cache functionality.
//!
//! The idea is:
//!   1. the client registers the last image sent for each output in a file
//!   2. the daemon spawns a client that reloads that image when an output is created

use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use rkyv::{Deserialize, Infallible};

use crate::ipc::Animation;

pub fn store(output_name: &str, img_path: &str) -> Result<(), String> {
    let mut filepath = cache_dir()?;
    filepath.push(output_name);
    let file = File::create(filepath).map_err(|e| e.to_string())?;

    let mut writer = BufWriter::new(file);
    writer
        .write_all(img_path.as_bytes())
        .map_err(|e| format!("failed to write cache: {e}"))
}

#[allow(clippy::borrowed_box)]
pub fn store_animation_frames(animation: &Animation) -> Result<(), String> {
    let filename = animation_filename(&PathBuf::from(&animation.path), animation.dimensions);
    let mut filepath = cache_dir()?;
    filepath.push(&filename);

    let bytes = match rkyv::to_bytes::<_, 1024>(animation) {
        Ok(bytes) => bytes,
        Err(e) => return Err(format!("Failed to serialize request: {e}")),
    };

    if !filepath.is_file() {
        let file = File::create(filepath).map_err(|e| e.to_string())?;

        let mut writer = BufWriter::new(file);
        writer
            .write_all(&bytes)
            .map_err(|e| format!("failed to write cache: {e}"))
    } else {
        Ok(())
    }
}

pub fn load_animation_frames(
    path: &Path,
    dimensions: (u32, u32),
) -> Result<Option<Animation>, String> {
    let filename = animation_filename(path, dimensions);
    let cache_dir = cache_dir()?;
    let mut filepath = cache_dir.clone();
    filepath.push(filename);

    let read_dir = cache_dir
        .read_dir()
        .map_err(|e| format!("failed to read cache directory ({cache_dir:?}): {e}"))?;

    for entry in read_dir.into_iter().flatten() {
        if entry.path() == filepath {
            let file = File::open(&filepath).map_err(|e| e.to_string())?;
            let mut buf_reader = BufReader::new(file);
            let mut buf = Vec::new();
            buf_reader
                .read_to_end(&mut buf)
                .map_err(|e| format!("failed to read file `{filepath:?}`: {e}"))?;

            let frames = unsafe { rkyv::archived_root::<Animation>(&buf) };
            let frames: Animation = frames.deserialize(&mut Infallible).unwrap();

            return Ok(Some(frames));
        }
    }
    Ok(None)
}

pub fn load(output_name: &str) -> Result<(), String> {
    let mut filepath = cache_dir()?;
    filepath.push(output_name);
    if !filepath.is_file() {
        return Ok(());
    }
    let file = std::fs::File::open(filepath).map_err(|e| format!("failed to open file: {e}"))?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::with_capacity(64);
    reader
        .read_to_end(&mut buf)
        .map_err(|e| format!("failed to read file: {e}"))?;

    let img_path = std::str::from_utf8(&buf).map_err(|e| format!("failed to decode bytes: {e}"))?;
    if buf.is_empty() {
        return Ok(());
    }

    if let Ok(mut child) = std::process::Command::new("pidof").arg("swww").spawn() {
        if let Ok(status) = child.wait() {
            if status.success() {
                return Err("there is already another swww process running".to_string());
            }
        }
    }

    match std::process::Command::new("swww")
        .arg("img")
        .args([
            &format!("--outputs={output_name}"),
            "--transition-type=simple",
            "--transition-step=255",
            img_path,
        ])
        .spawn()
    {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("failed to spawn child process: {e}")),
    }
}

fn create_dir(p: &Path) -> Result<(), String> {
    if !p.is_dir() {
        if let Err(e) = std::fs::create_dir(p) {
            return Err(format!("failed to create directory({p:#?}): {e}"));
        }
    }
    Ok(())
}

fn cache_dir() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("XDG_CACHE_HOME") {
        let mut path: PathBuf = path.into();
        path.push("swww");
        create_dir(&path)?;
        Ok(path)
    } else if let Ok(path) = std::env::var("HOME") {
        let mut path: PathBuf = path.into();
        path.push(".cache");
        path.push("swww");
        create_dir(&path)?;
        Ok(path)
    } else {
        Err("failed to read both $XDG_CACHE_HOME and $HOME environment variables".to_string())
    }
}

#[must_use]
fn animation_filename(path: &Path, dimensions: (u32, u32)) -> PathBuf {
    format!(
        "{}__{}x{}",
        path.to_string_lossy().replace('/', "__"),
        dimensions.0,
        dimensions.1
    )
    .into()
}
