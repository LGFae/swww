//! This module creates the Answer struct to send back stuff from the daemon, and also implements
//! some helper functions to make communication more streamlined
use crate::{
    cli::{Filter, Swww},
    daemon::BgInfo,
};
use serde::{Deserialize, Serialize};
use std::{
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Serialize, Deserialize)]
pub enum Answer {
    Ok,
    Err(String),
    Info(Vec<BgInfo>),
}

impl Answer {
    pub fn send(&self, stream: &UnixStream) -> Result<(), String> {
        match bincode::serialize_into(stream, self) {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("Failed to send answer: {}", e)),
        }
    }

    pub fn receive(stream: UnixStream) -> Result<Self, String> {
        #[cfg(debug_assertions)]
        let timeout = Duration::from_secs(60); //Some operations take a while to respond in debug mode
        #[cfg(not(debug_assertions))]
        let timeout = Duration::from_secs(10);

        if let Err(e) = stream.set_read_timeout(Some(timeout)) {
            return Err(format!("Failed to set read timeout: {}", e));
        };

        match bincode::deserialize_from(stream) {
            Ok(i) => Ok(i),
            Err(e) => Err(format!("Failed to receive answer: {}", e)),
        }
    }
}

impl Swww {
    pub fn send(&mut self, stream: &UnixStream) -> Result<(), String> {
        if let Swww::Img(img) = self {
            img.path = match img.path.canonicalize() {
                Ok(p) => p,
                Err(e) => return Err(format!("Couldn't get absolute path: {}", e)),
            };
            if img.transition_step == 0 {
                eprintln!("WARNING: a transition_step of 0 is invalid! Using 1 instead...");
                img.transition_step = 1;
            }
            if img.transition_fps == 0 {
                eprintln!("WARNING: a transition_fps of 0 is invalid! Using 1 instead...");
                img.transition_fps = 1;
            }
        }
        match bincode::serialize_into(stream, self) {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("Failed to serialize request: {}", e)),
        }
    }

    pub fn receive(stream: &mut UnixStream) -> Result<Self, String> {
        match bincode::deserialize_from(stream) {
            Ok(i) => Ok(i),
            Err(e) => Err(format!("Failed to deserialize request: {}", e)),
        }
    }
}

impl Filter {
    ///Simply gets the equivalent filter from imageops
    pub fn get_image_filter(&self) -> fast_image_resize::FilterType {
        match self {
            Self::Nearest => fast_image_resize::FilterType::Box,
            Self::Bilinear => fast_image_resize::FilterType::Bilinear,
            Self::CatmullRom => fast_image_resize::FilterType::CatmullRom,
            Self::Mitchell => fast_image_resize::FilterType::Mitchell,
            Self::Lanczos3 => fast_image_resize::FilterType::Lanczos3,
        }
    }
}

pub fn get_socket_path() -> PathBuf {
    let runtime_dir = if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        dir
    } else {
        "/tmp/swww".to_string()
    };
    let runtime_dir = Path::new(&runtime_dir);
    runtime_dir.join("swww.socket")
}
