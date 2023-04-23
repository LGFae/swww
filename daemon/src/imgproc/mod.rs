use log::{error, info};
use smithay_client_toolkit::shm::slot::SlotPool;

use std::{sync::Arc, sync::Mutex, thread};

use utils::communication::{Animation, Answer, BgImg, Img};

use crate::wallpaper::Wallpaper;

mod animations;
mod sync_barrier;

///The default thread stack size of 2MiB is way too overkill for our purposes
const TSTACK_SIZE: usize = 1 << 17; //128KiB

pub struct Imgproc {
    sync_barrier: Arc<sync_barrier::SyncBarrier>,
}

impl Imgproc {
    pub fn new() -> Self {
        Self {
            sync_barrier: Arc::new(sync_barrier::SyncBarrier::new(0)),
        }
    }

    pub fn set_output_count(&mut self, outputs_count: u8) {
        self.sync_barrier.set_goal(outputs_count);
    }

    pub fn transition(
        &mut self,
        pool: &Arc<Mutex<SlotPool>>,
        transition: utils::communication::Transition,
        requests: Vec<(Img, Vec<Arc<Wallpaper>>)>,
    ) -> Answer {
        let mut answer = Answer::Ok;
        for (Img { img, path }, mut wallpapers) in requests {
            let pool = Arc::clone(pool);
            let transition = transition.clone();

            if let Err(e) = thread::Builder::new()
                .name("transition".to_string()) //Name our threads  for better log messages
                .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
                .spawn(move || {
                    if wallpapers.is_empty() {
                        return;
                    }
                    for w in wallpapers.iter_mut() {
                        w.set_img_info(BgImg::Img(path.clone()));
                    }
                    let dimensions = wallpapers[0].get_dimensions();

                    if img.len() == dimensions.0 as usize * dimensions.1 as usize * 4 {
                        animations::Transition::new(wallpapers, dimensions, transition, pool)
                            .execute(&img);
                    } else {
                        error!(
                            "image is of wrong size! Image len: {}, expected size: {}",
                            img.len(),
                            dimensions.0 as usize * dimensions.1 as usize * 4
                        );
                    }
                })
            {
                answer = Answer::Err(format!("failed to spawn transition thread: {e}"));
                error!("failed to spawn 'transition' thread: {}", e);
            };
        }
        answer
    }

    pub fn animate(
        &mut self,
        pool: &Arc<Mutex<SlotPool>>,
        animation: utils::communication::Animation,
        mut wallpapers: Vec<Arc<Wallpaper>>,
    ) -> Answer {
        let mut answer = Answer::Ok;

        let pool = Arc::clone(pool);
        let barrier = Arc::clone(&self.sync_barrier);
        if let Err(e) = thread::Builder::new()
            .name("animation".to_string()) //Name our threads  for better log messages
            .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn(move || {
                /* We only need to animate if we have > 1 frame */
                if animation.animation.len() == 1 || wallpapers.is_empty() {
                    return;
                }

                for wallpaper in &wallpapers {
                    wallpaper.wait_for_animation();
                    wallpaper.begin_animation();
                }

                let mut now = std::time::Instant::now();

                for (frame, duration) in animation.animation.iter().cycle() {
                    if animation.sync {
                        barrier.inc_and_wait(*duration);
                    }

                    for wallpaper in wallpapers.iter_mut() {
                        let mut pool = wallpaper.lock_pool_to_get_canvas(&pool);
                        let canvas = wallpaper.get_canvas(&mut pool);
                        frame.ready(canvas.len()).unpack(canvas);
                        wallpaper.draw(&mut pool);
                    }

                    wallpapers.retain(|w| {
                        if w.animation_should_stop() {
                            w.end_animation();
                            false
                        } else {
                            true
                        }
                    });
                    let timeout = duration.saturating_sub(now.elapsed());
                    thread::sleep(timeout);
                    nix::sys::signal::kill(nix::unistd::Pid::this(), nix::sys::signal::SIGUSR1)
                        .unwrap();
                    now = std::time::Instant::now();
                }
            })
        {
            answer = Answer::Err(format!("failed to spawn animation thread: {e}"));
            error!("failed to spawn 'animation' thread: {e}");
        };

        answer
    }

    pub fn import_cached_img(
        &mut self,
        pool: &Arc<Mutex<SlotPool>>,
        wallpaper: &Arc<Wallpaper>,
        output_name: &str,
        output_size: usize,
    ) {
        if let Some((Img { img, path }, anim)) = get_cached_bg(output_name) {
            if output_size != img.len() {
                info!(
                    "{output_name} monitor's buffer size ({output_size}) is different than cache's image ({})",
                    img.len(),
                );
                return;
            }

            let pool = Arc::clone(pool);
            let wallpaper = Arc::clone(wallpaper);
            if let Err(e) = thread::Builder::new()
                .name("cache importing".to_string()) //Name our threads  for better log messages
                .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
                .spawn(move || {
                    if wallpaper.is_loading_cache() {
                        return;
                    }
                    wallpaper.start_cache_load();
                    wallpaper.set_img(&pool, &img, BgImg::Img(path));
                    wallpaper.end_cache_load();
                    if let Some(anim) = anim {
                        if anim.animation.len() <= 1 {
                            return;
                        }
                        wallpaper.begin_animation();
                        let mut now = std::time::Instant::now();
                        for (frame, duration) in anim.animation.iter().cycle() {
                            {
                                let mut pool = wallpaper.lock_pool_to_get_canvas(&pool);
                                let canvas = wallpaper.get_canvas(&mut pool);
                                frame.ready(canvas.len()).unpack(canvas);
                                wallpaper.draw(&mut pool);
                            }

                            if wallpaper.animation_should_stop() {
                                wallpaper.end_animation();
                                return;
                            }
                            let timeout = duration.saturating_sub(now.elapsed());
                            thread::sleep(timeout);
                            nix::sys::signal::kill(
                                nix::unistd::Pid::this(),
                                nix::sys::signal::SIGUSR1,
                            )
                            .unwrap();
                            now = std::time::Instant::now();
                        }
                    }
                })
            {
                error!("failed to spawn 'cache importing' thread: {}", e);
            }
        } else {
            info!("did not find cached image for monitor '{}'", output_name);
        }
    }
}

fn get_cached_bg(output: &str) -> Option<(Img, Option<Animation>)> {
    let cache_path = match utils::communication::get_cache_path() {
        Ok(mut path) => {
            path.push(output);
            path
        }
        Err(e) => {
            error!("failed to get bgs cache's path: {e}");
            return None;
        }
    };

    let cache_file = match std::fs::File::open(cache_path) {
        Ok(file) => file,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                error!("failed to open bgs cache's file: {e}");
            }
            return None;
        }
    };

    let mut reader = std::io::BufReader::new(cache_file);
    match Img::try_from(&mut reader) {
        Ok(img) => match Animation::try_from(&mut reader) {
            Ok(anim) => Some((img, Some(anim))),
            Err(_) => Some((img, None)),
        },
        Err(e) => {
            error!("failed to read bgs cache's file: {e}");
            None
        }
    }
}
