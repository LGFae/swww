use log::{debug, error, info};
use smithay_client_toolkit::shm::slot::SlotPool;

use std::{
    path::PathBuf,
    sync::Arc,
    sync::{mpsc, Mutex},
    thread,
    time::Duration,
};

use utils::{
    communication::{Animation, Answer, BgImg, BgInfo, Img},
    comp_decomp::ReadiedPack,
};

use crate::{
    lock_pool_and_wallpapers,
    wallpaper::{OutputId, Wallpaper},
};

mod animations;
mod sync_barrier;

///The default thread stack size of 2MiB is way too overkill for our purposes
const TSTACK_SIZE: usize = 1 << 17; //128KiB

pub type ImgWithDim = (Box<[u8]>, (u32, u32));

pub struct Processor {
    sync_barrier: Arc<sync_barrier::SyncBarrier>,
}

impl Processor {
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
                            animations::Transition::new(wallpapers, dimensions, transition, pool)
                                .execute(&img);
                        } else {
                            error!(
                                "image is of wrong size! Image len: {}, expected size: {}",
                                img.len(),
                                dimensions.0 as usize * dimensions.1 as usize * 4
                            );
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

    //    pub fn animate(
    //        &mut self,
    //        animation: utils::communication::Animation,
    //        mut outputs: Vec<String>,
    //        output_size: usize,
    //    ) -> Answer {
    //        let mut answer = Answer::Ok;
    //
    //        let sender = self.frame_sender.clone();
    //        let (stopper, stop_recv) = mpsc::channel();
    //        let on_going_transitions = Arc::clone(&self.on_going_transitions);
    //
    //        let barrier = Arc::clone(&self.sync_barrier);
    //        self.anim_stoppers.push(stopper);
    //        if let Err(e) = thread::Builder::new()
    //            .name("animation".to_string()) //Name our threads  for better log messages
    //            .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
    //            .spawn(move || {
    //                while on_going_transitions
    //                    .read()
    //                    .unwrap()
    //                    .iter()
    //                    .any(|output| outputs.contains(output))
    //                {
    //                    std::thread::yield_now();
    //                }
    //                let mut now = std::time::Instant::now();
    //                /* We only need to animate if we have > 1 frame */
    //                if animation.animation.len() == 1 {
    //                    return;
    //                }
    //                for (frame, duration) in animation.animation.iter().cycle() {
    //                    let frame = frame.ready(output_size);
    //
    //                    if animation.sync {
    //                        barrier.inc_and_wait_while(*duration, || match stop_recv.try_recv() {
    //                            Ok(to_remove) => {
    //                                outputs.retain(|o| !to_remove.contains(o));
    //                                outputs.is_empty() || to_remove.is_empty()
    //                            }
    //                            Err(mpsc::TryRecvError::Empty) => false,
    //                            Err(mpsc::TryRecvError::Disconnected) => true,
    //                        });
    //                        if outputs.is_empty() {
    //                            return;
    //                        }
    //                    }
    //
    //                    let timeout = duration.saturating_sub(now.elapsed());
    //                    if send_frame(frame, &mut outputs, timeout, &sender, &stop_recv) {
    //                        debug!("STOPPING");
    //                        return;
    //                    }
    //                    now = std::time::Instant::now();
    //                }
    //            })
    //        {
    //            answer = Answer::Err(format!("failed to spawn animation thread: {e}"));
    //            error!("failed to spawn 'animation' thread: {e}");
    //        };
    //
    //        answer
    //    }
    //
    //    #[must_use]
    //    pub fn import_cached_img(&mut self, info: BgInfo, old_img: &mut [u8]) -> Option<PathBuf> {
    //        if let Some((Img { img, path }, anim)) = get_cached_bg(&info.name) {
    //            let output_size = old_img.len();
    //            if output_size < img.len() {
    //                info!(
    //                    "{} monitor's buffer size ({output_size}) is smaller than cache's image ({})",
    //                    info.name,
    //                    img.len()
    //                );
    //                return None;
    //            }
    //            let pack = ReadiedPack::new(old_img, &img, |cur, goal, _| {
    //                *cur = *goal;
    //            });
    //
    //            let sender = self.frame_sender.clone();
    //            let (stopper, stop_recv) = mpsc::channel();
    //            self.anim_stoppers.push(stopper);
    //            if let Err(e) = thread::Builder::new()
    //                .name("cache importing".to_string()) //Name our threads  for better log messages
    //                .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
    //                .spawn(move || {
    //                    let mut outputs = vec![info.name];
    //                    send_frame(pack, &mut outputs, Duration::new(0, 0), &sender, &stop_recv);
    //                    if let Some(anim) = anim {
    //                        let mut now = std::time::Instant::now();
    //                        if anim.animation.len() == 1 {
    //                            return;
    //                        }
    //                        for (frame, duration) in anim.animation.iter().cycle() {
    //                            let frame = frame.ready(output_size);
    //                            let timeout = duration.saturating_sub(now.elapsed());
    //                            if send_frame(frame, &mut outputs, timeout, &sender, &stop_recv) {
    //                                return;
    //                            }
    //                            now = std::time::Instant::now();
    //                        }
    //                    }
    //                })
    //            {
    //                error!("failed to spawn 'cache importing' thread: {}", e);
    //                return None;
    //            }
    //
    //            return Some(path);
    //        }
    //        info!("failed to find cached image for monitor '{}'", info.name);
    //        None
    //    }
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
