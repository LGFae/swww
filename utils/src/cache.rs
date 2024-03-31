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

use crate::ipc::{Animation, PixelFormat};

pub fn store(output_name: &str, img_path: &str) -> Result<(), String> {
    let mut filepath = cache_dir()?;
    filepath.push(output_name);
    let file = File::create(filepath).map_err(|e| e.to_string())?;

    let mut writer = BufWriter::new(file);
    writer
        .write_all(img_path.as_bytes())
        .map_err(|e| format!("failed to write cache: {e}"))
}

pub fn store_animation_frames(animation: &Animation) -> Result<(), String> {
    let filename = animation_filename(
        &PathBuf::from(&animation.path),
        animation.dimensions,
        animation.pixel_format,
    );
    let mut filepath = cache_dir()?;
    filepath.push(&filename);

    let bytes = bitcode::encode(animation);

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
    pixel_format: PixelFormat,
) -> Result<Option<Animation>, String> {
    let filename = animation_filename(path, dimensions, pixel_format);
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

            let frames: Animation =
                bitcode::decode(&buf).expect("failed to decode cached animations");
            return Ok(Some(frames));
        }
    }
    Ok(None)
}

pub fn get_previous_image_path(output_name: &str) -> Result<String, String> {
    let mut filepath = cache_dir()?;
    clean_previous_verions(&filepath);

    filepath.push(output_name);
    if !filepath.is_file() {
        return Ok("".to_string());
    }
    let file = std::fs::File::open(filepath).map_err(|e| format!("failed to open file: {e}"))?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::with_capacity(64);
    reader
        .read_to_end(&mut buf)
        .map_err(|e| format!("failed to read file: {e}"))?;

    String::from_utf8(buf).map_err(|e| format!("failed to decode bytes: {e}"))
}

pub fn load(output_name: &str) -> Result<(), String> {
    let img_path = get_previous_image_path(output_name)?;
    if img_path.is_empty() {
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
            "--transition-type=none",
            &img_path,
        ])
        .spawn()
    {
        Ok(mut child) => match child.wait() {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("child process failed: {e}")),
        },
        Err(e) => Err(format!("failed to spawn child process: {e}")),
    }
}

pub fn clean() -> Result<(), String> {
    std::fs::remove_dir_all(cache_dir()?)
        .map_err(|e| format!("failed to remove cache directory: {e}"))
}

fn clean_previous_verions(cache_dir: &Path) {
    let mut read_dir = match std::fs::read_dir(cache_dir) {
        Ok(read_dir) => read_dir,
        Err(_) => {
            eprintln!("WARNING: failed to read cache dir {:?} entries", cache_dir);
            return;
        }
    };

    let current_version = env!("CARGO_PKG_VERSION");

    while let Some(Ok(entry)) = read_dir.next() {
        let filename = entry.file_name();
        let filename = match filename.to_str() {
            Some(filename) => filename,
            None => {
                eprintln!("WARNING: failed to read filename of {:?}", filename);
                continue;
            }
        };

        // only the images we've cached will have a _v token, indicating their version
        if let Some(i) = filename.rfind("_v") {
            if &filename[i + 2..] != current_version {
                if let Err(e) = std::fs::remove_file(entry.path()) {
                    eprintln!(
                        "WARNING: failed to remove cache file {} of old swww version {:?}",
                        filename, e
                    );
                }
            }
        }
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
fn animation_filename(path: &Path, dimensions: (u32, u32), pixel_format: PixelFormat) -> PathBuf {
    format!(
        "{}__{}x{}_{:?}_v{}",
        path.to_string_lossy().replace('/', "_"),
        dimensions.0,
        dimensions.1,
        pixel_format,
        env!("CARGO_PKG_VERSION"),
    )
    .into()
}
