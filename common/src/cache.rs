//! Implements basic cache functionality.
//!
//! The idea is:
//!   1. the client registers the last image sent for each output in a file
//!   2. the daemon spawns a client that reloads that image when an output is created

use std::{
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use crate::ipc::Animation;
use crate::ipc::PixelFormat;
use crate::mmap::Mmap;

pub(crate) fn store(output_name: &str, img_path: &str) -> io::Result<()> {
    let mut filepath = cache_dir()?;
    filepath.push(output_name);
    File::create(filepath)?.write_all(img_path.as_bytes())
}

pub(crate) fn store_animation_frames(
    animation: &[u8],
    path: &Path,
    dimensions: (u32, u32),
    pixel_format: PixelFormat,
) -> io::Result<()> {
    let filename = animation_filename(path, dimensions, pixel_format);
    let mut filepath = cache_dir()?;
    filepath.push(&filename);

    if !filepath.is_file() {
        File::create(filepath)?.write_all(animation)
    } else {
        Ok(())
    }
}

pub fn load_animation_frames(
    path: &Path,
    dimensions: (u32, u32),
    pixel_format: PixelFormat,
) -> io::Result<Option<Animation>> {
    let filename = animation_filename(path, dimensions, pixel_format);
    let cache_dir = cache_dir()?;
    let mut filepath = cache_dir.clone();
    filepath.push(filename);

    let read_dir = cache_dir.read_dir()?;

    for entry in read_dir.into_iter().flatten() {
        if entry.path() == filepath {
            let fd = File::open(&filepath)?.into();
            let len = rustix::fs::seek(&fd, rustix::fs::SeekFrom::End(0))?;
            let mmap = Mmap::from_fd(fd, len as usize);

            match std::panic::catch_unwind(|| Animation::deserialize(&mmap, mmap.slice())) {
                Ok((frames, _)) => return Ok(Some(frames)),
                Err(e) => eprintln!("Error loading animation frames: {e:?}"),
            }
        }
    }
    Ok(None)
}

pub fn get_previous_image_path(output_name: &str) -> io::Result<String> {
    let mut filepath = cache_dir()?;
    clean_previous_verions(&filepath);

    filepath.push(output_name);
    if !filepath.is_file() {
        return Ok("".to_string());
    }

    let mut buf = Vec::with_capacity(64);
    File::open(filepath)?.read_to_end(&mut buf)?;

    String::from_utf8(buf).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("failed to decode bytes: {e}"),
        )
    })
}

pub fn load(output_name: &str) -> io::Result<()> {
    let img_path = get_previous_image_path(output_name)?;
    if img_path.is_empty() {
        return Ok(());
    }

    if let Ok(mut child) = std::process::Command::new("pidof").arg("swww").spawn() {
        if let Ok(status) = child.wait() {
            if status.success() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "there is already another swww process running".to_string(),
                ));
            }
        }
    }

    std::process::Command::new("swww")
        .arg("img")
        .args([
            &format!("--outputs={output_name}"),
            "--transition-type=none",
            &img_path,
        ])
        .spawn()?
        .wait()?;
    Ok(())
}

pub fn clean() -> io::Result<()> {
    std::fs::remove_dir_all(cache_dir()?)
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

fn create_dir(p: &Path) -> io::Result<()> {
    if !p.is_dir() {
        std::fs::create_dir(p)
    } else {
        Ok(())
    }
}

fn cache_dir() -> io::Result<PathBuf> {
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
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "failed to read both $XDG_CACHE_HOME and $HOME environment variables".to_string(),
        ))
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
