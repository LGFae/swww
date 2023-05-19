use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    io::{BufReader, BufWriter},
    path::PathBuf,
    time::Duration,
};

use utils::{communication::get_cache_path, comp_decomp::BitPack};

use log::{debug, error};

use crate::cli::Img;

const VERSION: u32 = 1;

#[derive(serde::Deserialize)]
pub struct CachedAnimation {
    version: u32,
    hash: u64,
    pub animation: Box<[(BitPack, Duration)]>,
}

// Only used for serialization.
//
// The difference to [`CachedAnimation`] is that this type does not own it's data
// so that it doesn't have to be cloned or moved.
#[derive(serde::Serialize)]
struct CachedAnimationRef<'a> {
    version: u32,
    hash: u64,
    animation: &'a [(BitPack, Duration)],
}

pub struct AnimationCache {
    cache_dir: PathBuf,
}

impl AnimationCache {
    pub fn init() -> Result<Self, String> {
        let cache_dir = get_cache_path()?.join("animations");
        if !cache_dir.is_dir() {
            if let Err(e) = std::fs::create_dir(&cache_dir) {
                return Err(format!(
                    "failed to create cache_path \"{}\": {e}",
                    cache_dir.display()
                ));
            }
        }
        Ok(Self { cache_dir })
    }

    fn hash(&self, img: &Img, dim: &(u32, u32)) -> (PathBuf, u64) {
        let mut hasher = DefaultHasher::new();
        img.path.hash(&mut hasher);
        dim.hash(&mut hasher);
        img.filter.hash(&mut hasher);
        img.no_resize.hash(&mut hasher);
        img.fill_color.hash(&mut hasher);

        let hash = hasher.finish();

        let name = format!("{:0x}.swww-cache", hash);
        let path = self.cache_dir.join(name);

        (path, hash)
    }

    pub fn load(&self, img: &Img, dim: &(u32, u32)) -> Result<Option<CachedAnimation>, String> {
        let (cache_path, hash) = self.hash(img, dim);
        if cache_path.is_file() {
            debug!("loading cached animation from {}", cache_path.display());
            let cache_file = std::fs::File::open(&cache_path).map_err(|e| {
                format!(
                    "cannot open image cache file from {}: {}",
                    cache_path.display(),
                    e
                )
            })?;
            match bincode::deserialize_from::<_, CachedAnimation>(BufReader::new(cache_file)) {
                Ok(cache) => {
                    debug!("loaded cached animation from {}", cache_path.display());
                    if cache.version != VERSION {
                        error!(
                            "invalid version. Expected {} but got {}",
                            VERSION, cache.version,
                        );
                        return Ok(None);
                    }
                    if cache.hash != hash {
                        error!(
                            "invalid hash in {}. Expected {} but got {}",
                            cache_path.display(), hash, cache.hash,
                        );
                        return Ok(None);
                    }
                    return Ok(Some(cache));
                }
                Err(e) => {
                    error!(
                        "failed to load image cache for {}: {}",
                        cache_path.display(),
                        e
                    );
                }
            }
        }
        debug!("no cached animation found for {}", cache_path.display());

        Ok(None)
    }

    pub fn save(
        &self,
        img: &crate::cli::Img,
        dim: &(u32, u32),
        animation: &[(BitPack, Duration)],
    ) -> Result<(), String> {
        let (cache_path, hash) = self.hash(img, dim);
        debug!("caching animation to {}", cache_path.display());
        let cache_file = std::fs::File::create(&cache_path).map_err(|e| {
            format!(
                "cannot open image cache file from {}: {}",
                cache_path.display(),
                e
            )
        })?;
        if let Err(e) = bincode::serialize_into(
            BufWriter::new(cache_file),
            &CachedAnimationRef {
                animation,
                version: VERSION,
                hash,
            },
        ) {
            error!(
                "failed to load image cache for {}: {}",
                cache_path.display(),
                e
            );
        }
        debug!("cached animation to {}", cache_path.display());

        Ok(())
    }
}
