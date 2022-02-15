//! This module creates the Answer struct to send back stuff from the daemon, and also implements
//! some helper functions to make communication more streamlined
use crate::{
    cli::{Filter, Fswww},
    daemon,
};
use serde::{Deserialize, Serialize};
use std::{os::unix::net::UnixStream, time::Duration};

#[derive(Serialize, Deserialize)]
pub enum Answer {
    Ok,
    Err {
        msg: String,
    },
    Info {
        out_dim_img: Vec<(String, (u32, u32), daemon::BgImg)>,
    },
}

impl Answer {
    pub fn send(&self, stream: &UnixStream) -> Result<(), String> {
        match bincode::serialize_into(stream, self) {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("Failed to send answer: {}", e)),
        }
    }

    pub fn receive(stream: &mut UnixStream) -> Result<Self, String> {
        #[cfg(debug_assertions)]
        let timeout = Duration::from_secs(10); //Some operations take a while to respond in debug mode
        #[cfg(not(debug_assertions))]
        let timeout = Duration::from_secs(1);

        if let Err(e) = stream.set_read_timeout(Some(timeout)) {
            return Err(format!("Failed to set read timeout: {}", e));
        };

        match bincode::deserialize_from(stream) {
            Ok(i) => Ok(i),
            Err(e) => Err(format!("Failed to receive answer: {}", e)),
        }
    }
}

impl Fswww {
    pub fn send(&mut self, stream: &UnixStream) -> Result<(), String> {
        if let Fswww::Img(img) = self {
            img.path = match img.path.canonicalize() {
                Ok(p) => p,
                Err(e) => return Err(format!("Coulnd't get absolute path: {}", e)),
            };
            if img.transition_step == 0 {
                eprintln!("A transition_step of 0 is invalid! Defaulting to 20...");
                img.transition_step = 20;
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
    pub fn get_image_filter(&self) -> image::imageops::FilterType {
        match self {
            Self::Nearest => image::imageops::FilterType::Nearest,
            Self::Triangle => image::imageops::FilterType::Triangle,
            Self::CatmullRom => image::imageops::FilterType::CatmullRom,
            Self::Gaussian => image::imageops::FilterType::Gaussian,
            Self::Lanczos3 => image::imageops::FilterType::Lanczos3,
        }
    }
}
