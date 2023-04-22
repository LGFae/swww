use log::{error, info};
use smithay_client_toolkit::shm::slot::SlotPool;

use std::{sync::Arc, sync::Mutex, thread, time::Duration};

use utils::communication::{Animation, Answer, BgImg, Img};

use crate::{
    lock_pool_and_wallpapers,
    wallpaper::{OutputId, Wallpaper},
};

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
        requests: Vec<(Img, Vec<OutputId>)>,
        wallpapers: &Arc<Mutex<Vec<Wallpaper>>>,
    ) -> Answer {
        let mut answer = Answer::Ok;
        for (Img { img, path }, outputs) in requests {
            let wallpapers = Arc::clone(wallpapers);
            let pool = Arc::clone(pool);
            let transition = transition.clone();

            if let Err(e) = thread::Builder::new()
                .name("transition".to_string()) //Name our threads  for better log messages
                .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
                .spawn(move || {
                    let mut dimensions = None;
                    {
                        let (_pool, mut wallpapers) = lock_pool_and_wallpapers(&pool, &wallpapers);
                        for wallpaper in wallpapers
                            .iter_mut()
                            .filter(|w| outputs.contains(&w.output_id))
                        {
                            wallpaper.chown();
                            wallpaper.img = BgImg::Img(path.clone());
                            wallpaper.in_transition = true;
                            if dimensions.is_none() {
                                dimensions = Some((
                                    wallpaper.width.get() as u32
                                        * wallpaper.scale_factor.get() as u32,
                                    wallpaper.height.get() as u32
                                        * wallpaper.scale_factor.get() as u32,
                                ));
                            }
                        }
                    }

                    if let Some(dimensions) = dimensions {
                        if img.len() == dimensions.0 as usize * dimensions.1 as usize * 4 {
                            animations::Transition::new(&wallpapers, dimensions, transition, &pool)
                                .execute(&img);
                        } else {
                            error!(
                                "image is of wrong size! Image len: {}, expected size: {}",
                                img.len(),
                                dimensions.0 as usize * dimensions.1 as usize * 4
                            );
                        }
                    }

                    {
                        let (_pool, mut wallpapers) = lock_pool_and_wallpapers(&pool, &wallpapers);
                        for wallpaper in wallpapers.iter_mut().filter(|w| {
                            outputs.contains(&w.output_id) || w.is_owned_by(thread::current().id())
                        }) {
                            wallpaper.in_transition = false;
                        }
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
        outputs: Vec<OutputId>,
        wallpapers: &Arc<Mutex<Vec<Wallpaper>>>,
    ) -> Answer {
        let mut answer = Answer::Ok;

        let wallpapers = Arc::clone(wallpapers);
        let pool = Arc::clone(pool);
        let barrier = Arc::clone(&self.sync_barrier);
        if let Err(e) = thread::Builder::new()
            .name("animation".to_string()) //Name our threads  for better log messages
            .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn(move || {
                /* We only need to animate if we have > 1 frame */
                if animation.animation.len() == 1 || outputs.is_empty() {
                    return;
                }

                loop {
                    {
                        let (_pool, wallpapers) = lock_pool_and_wallpapers(&pool, &wallpapers);
                        if wallpapers
                            .iter()
                            .filter(|w| outputs.contains(&w.output_id))
                            .all(|w| !w.in_transition)
                        {
                            break;
                        }
                    }
                    std::thread::sleep(Duration::from_micros(100));
                }
                {
                    let (_pool, mut wallpapers) = lock_pool_and_wallpapers(&pool, &wallpapers);
                    for wallpaper in wallpapers
                        .iter_mut()
                        .filter(|w| outputs.contains(&w.output_id))
                    {
                        wallpaper.chown();
                    }
                }
                let mut now = std::time::Instant::now();

                for (frame, duration) in animation.animation.iter().cycle() {
                    if animation.sync {
                        barrier.inc_and_wait(*duration);
                    }

                    let mut done = true;
                    {
                        let (mut pool, mut wallpapers) =
                            lock_pool_and_wallpapers(&pool, &wallpapers);
                        for wallpaper in wallpapers.iter_mut().filter(|w| {
                            outputs.contains(&w.output_id)
                                && w.is_owned_by(std::thread::current().id())
                        }) {
                            done = false;
                            let mut i = 0;
                            while wallpaper.slot.has_active_buffers() && i < 100 {
                                i += 1;
                                std::thread::sleep(Duration::from_micros(100));
                            }
                            if let Some(canvas) = wallpaper.slot.canvas(&mut pool) {
                                frame.ready(canvas.len()).unpack(canvas);
                                wallpaper.draw(&mut pool);
                            }
                        }
                    }
                    if done {
                        return;
                    }

                    let timeout = duration.saturating_sub(now.elapsed());
                    std::thread::sleep(timeout);
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
        wallpapers: &Arc<Mutex<Vec<Wallpaper>>>,
        output: OutputId,
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
            let wallpapers = Arc::clone(wallpapers);
            if let Err(e) = thread::Builder::new()
                .name("cache importing".to_string()) //Name our threads  for better log messages
                .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
                .spawn(move || {
                    {
                        let (mut pool, mut wallpapers) =
                            lock_pool_and_wallpapers(&pool, &wallpapers);
                        if let Some(w) = wallpapers.iter_mut().find(|w| w.output_id == output) {
                            let mut i = 0;
                            while w.slot.has_active_buffers() && i < 100 {
                                i += 1;
                                std::thread::sleep(Duration::from_micros(100));
                            }
                            w.img = BgImg::Img(path);
                            w.set_img(&mut pool, &img);
                            w.draw(&mut pool);
                        }
                    }

                    if let Some(anim) = anim {
                        if anim.animation.len() == 1 {
                            return;
                        }
                        {
                            let (_p, mut wallpapers) = lock_pool_and_wallpapers(&pool, &wallpapers);
                            if let Some(w) = wallpapers.iter_mut().find(|w| w.output_id == output) {
                                w.chown();
                            } else {
                                return;
                            }
                        }
                        let thread_id = thread::current().id();
                        let mut now = std::time::Instant::now();
                        for (frame, duration) in anim.animation.iter().cycle() {
                            {
                                let (mut pool, mut wallpapers) =
                                    lock_pool_and_wallpapers(&pool, &wallpapers);
                                if let Some(w) = wallpapers
                                    .iter_mut()
                                    .find(|w| w.output_id == output && w.is_owned_by(thread_id))
                                {
                                    let mut i = 0;
                                    while w.slot.has_active_buffers() && i < 100 {
                                        i += 1;
                                        std::thread::sleep(Duration::from_micros(100));
                                    }
                                    if let Some(canvas) = w.slot.canvas(&mut pool) {
                                        frame.ready(canvas.len()).unpack(canvas);
                                        w.draw(&mut pool);
                                    }
                                } else {
                                    return;
                                }
                            }

                            let timeout = duration.saturating_sub(now.elapsed());
                            std::thread::sleep(timeout);
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
