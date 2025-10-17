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

const CACHE_DIRNAME: &str = env!("CARGO_PKG_VERSION");

pub struct CacheEntry<'a> {
    pub namespace: &'a str,
    pub resize: &'a str,
    pub filter: &'a str,
    pub img_path: &'a str,
}

impl<'a> CacheEntry<'a> {
    pub(crate) fn new(
        namespace: &'a str,
        resize: &'a str,
        filter: &'a str,
        img_path: &'a str,
    ) -> Self {
        Self {
            namespace,
            resize,
            filter,
            img_path,
        }
    }

    fn parse_file<'b>(output_name: &str, data: &'b [u8]) -> io::Result<Vec<CacheEntry<'b>>> {
        use std::io::Error;

        let mut v = Vec::new();
        let mut strings = data.split(|ch| *ch == 0);
        while let Some(namespace) = strings.next() {
            let resize = strings.next().ok_or_else(|| {
                Error::other(format!(
                    "cache file for output {output_name} is in the wrong format (no resize)"
                ))
            })?;
            let filter = strings.next().ok_or_else(|| {
                Error::other(format!(
                    "cache file for output {output_name} is in the wrong format (no filter)"
                ))
            })?;
            let img_path = strings.next().ok_or_else(|| {
                Error::other(format!(
                    "cache file for output {output_name} is in the wrong format (no image path)"
                ))
            })?;

            let err = format!("cache file for output {output_name} is not valid utf8");
            let namespace = str::from_utf8(namespace).map_err(|_| Error::other(err.clone()))?;
            let resize = str::from_utf8(resize).map_err(|_| Error::other(err.clone()))?;
            let filter = str::from_utf8(filter).map_err(|_| Error::other(err.clone()))?;
            let img_path = str::from_utf8(img_path).map_err(|_| Error::other(err))?;

            v.push(CacheEntry {
                namespace,
                resize,
                filter,
                img_path,
            })
        }

        Ok(v)
    }

    pub(crate) fn store(self, output_name: &str) -> io::Result<()> {
        use std::io::Seek;

        let mut filepath = cache_dir()?;
        filepath.push(output_name);

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .truncate(false)
            .open(filepath)?;

        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        let mut entries = Self::parse_file(output_name, &data).unwrap_or_else(|_| Vec::new());

        if let Some(entry) = entries
            .iter_mut()
            .find(|elem| elem.namespace == self.namespace)
        {
            entry.resize = self.resize;
            entry.filter = self.filter;
            entry.img_path = self.img_path;
        } else {
            entries.push(self);
        }

        file.seek(std::io::SeekFrom::Start(0))?;
        for entry in entries {
            let CacheEntry {
                namespace,
                resize,
                filter,
                img_path,
            } = entry;
            file.write_all(format!("{namespace}\0{resize}\0{filter}\0{img_path}").as_bytes())?;
        }

        let len = file.stream_position().unwrap_or(0);
        file.set_len(len)?;
        Ok(())
    }
}

pub(crate) fn store_animation_frames(
    animation: &[u8],
    path: &Path,
    dimensions: (u32, u32),
    resize: &str,
    pixel_format: PixelFormat,
) -> io::Result<()> {
    let filename = animation_filename(path, dimensions, resize, pixel_format);
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
    resize: &str,
    pixel_format: PixelFormat,
) -> io::Result<Option<Animation>> {
    let filename = animation_filename(path, dimensions, resize, pixel_format);
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

pub fn read_cache_file(output_name: &str) -> io::Result<Vec<u8>> {
    clean_previous_versions();

    let mut filepath = cache_dir()?;

    filepath.push(output_name);
    std::fs::read(filepath)
}

pub fn get_previous_image_cache<'a>(
    output_name: &str,
    namespace: &str,
    cache_data: &'a [u8],
) -> io::Result<Option<CacheEntry<'a>>> {
    let entries = CacheEntry::parse_file(output_name, cache_data)?;

    Ok(entries
        .into_iter()
        .find(|entry| entry.namespace == namespace))
}

pub fn load(output_name: &str, namespace: &str) -> io::Result<()> {
    let cache_data = read_cache_file(output_name)?;

    let cache = match get_previous_image_cache(output_name, namespace, &cache_data)? {
        Some(cache) => cache,
        None => return Ok(()),
    };

    if let Ok(mut child) = std::process::Command::new("pidof").arg("swww").spawn()
        && let Ok(status) = child.wait()
        && status.success()
    {
        return Err(std::io::Error::other(
            "there is already another swww process running",
        ));
    }

    std::process::Command::new("swww")
        .arg("img")
        .args([
            "--outputs",
            output_name,
            "--resize",
            cache.resize,
            "--filter",
            cache.filter,
            // namespace needs a format because the empty namespace is valid, so we need to use the
            // `=` format
            &format!("--namespace={namespace}"),
            "--transition-type=none",
            cache.img_path,
        ])
        .spawn()?
        .wait()?;
    Ok(())
}

pub fn clean() -> io::Result<()> {
    clean_previous_versions();
    std::fs::remove_dir_all(cache_dir()?)
}

fn clean_previous_versions() {
    let user_cache = match user_cache_dir() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("WARNING: failed to get user cache dir {e}");
            return;
        }
    };

    let mut read_dir = match std::fs::read_dir(&user_cache) {
        Ok(read_dir) => read_dir,
        Err(_) => {
            eprintln!("WARNING: failed to read cache dir {user_cache:?} entries");
            return;
        }
    };

    while let Some(Ok(entry)) = read_dir.next() {
        let entryname = entry.file_name();
        if entryname == CACHE_DIRNAME {
            continue;
        }

        if entry.path().is_dir() {
            if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                eprintln!("failed to remove old cache directory {entryname:?}: {e}");
            }
        } else if let Err(e) = std::fs::remove_file(entry.path()) {
            eprintln!("failed to remove old cache directory {entryname:?}: {e}");
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

fn user_cache_dir() -> io::Result<PathBuf> {
    if let Ok(path) = std::env::var("XDG_CACHE_HOME") {
        let mut path: PathBuf = path.into();
        path.push("swww");
        Ok(path)
    } else if let Ok(path) = std::env::var("HOME") {
        let mut path: PathBuf = path.into();
        path.push(".cache");
        path.push("swww");
        Ok(path)
    } else {
        Err(std::io::Error::other(
            "failed to read both $XDG_CACHE_HOME and $HOME environment variables",
        ))
    }
}

fn cache_dir() -> io::Result<PathBuf> {
    let mut path = user_cache_dir()?;
    path.push(CACHE_DIRNAME);
    create_dir(&path)?;
    Ok(path)
}

#[must_use]
fn animation_filename(
    path: &Path,
    dimensions: (u32, u32),
    resize: &str,
    pixel_format: PixelFormat,
) -> PathBuf {
    format!(
        "{}__{}x{}_{}_{:?}",
        path.to_string_lossy().replace('/', "_"),
        dimensions.0,
        dimensions.1,
        resize,
        pixel_format,
    )
    .into()
}
