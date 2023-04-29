use log::error;
use rkyv::{string::ArchivedString, vec::ArchivedVec, Deserialize};
use smithay_client_toolkit::shm::slot::SlotPool;

use std::{
    sync::Arc,
    sync::Mutex,
    thread::{self, Scope},
};

use utils::communication::{
    Answer, ArchivedAnimation, ArchivedImg, ArchivedRequest, ArchivedTransition, BgImg, Request,
};

use crate::wallpaper::Wallpaper;

mod sync_barrier;
mod transitions;
use transitions::Transition;

///The default thread stack size of 2MiB is way too overkill for our purposes
const STACK_SIZE: usize = 1 << 17; //128KiB

pub struct Animator {
    sync_barrier: Arc<sync_barrier::SyncBarrier>,
}

impl Animator {
    pub fn new() -> Self {
        Self {
            sync_barrier: Arc::new(sync_barrier::SyncBarrier::new(0)),
        }
    }

    pub fn set_output_count(&mut self, outputs_count: u8) {
        self.sync_barrier.set_goal(outputs_count);
    }

    fn spawn_transition_thread<'a, 'b>(
        scope: &'a Scope<'b, '_>,
        transition: &'b ArchivedTransition,
        img: &'b ArchivedVec<u8>,
        path: &'b ArchivedString,
        mut wallpapers: Vec<Arc<Wallpaper>>,
        pool: Arc<Mutex<SlotPool>>,
    ) where
        'a: 'b,
    {
        if let Err(e) = thread::Builder::new()
            .name("transition".to_string()) //Name our threads  for better log messages
            .stack_size(STACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn_scoped(scope, move || {
                if wallpapers.is_empty() {
                    return;
                }
                for w in wallpapers.iter_mut() {
                    w.set_img_info(BgImg::Img(path.to_string()));
                }
                let dimensions = wallpapers[0].get_dimensions();

                if img.len() == dimensions.0 as usize * dimensions.1 as usize * 3 {
                    Transition::new(wallpapers, dimensions, transition.clone(), pool).execute(img);
                } else {
                    error!(
                        "image is of wrong size! Image len: {}, expected size: {}",
                        img.len(),
                        dimensions.0 as usize * dimensions.1 as usize * 3
                    );
                }
            })
        {
            error!("failed to spawn 'transition' thread: {}", e);
        }
    }

    pub fn transition(
        &mut self,
        pool: &Arc<Mutex<SlotPool>>,
        bytes: Vec<u8>,
        wallpapers: Vec<Vec<Arc<Wallpaper>>>,
    ) -> Answer {
        let pool = Arc::clone(pool);
        match thread::Builder::new().stack_size(1 << 15).spawn(move || {
            if let ArchivedRequest::Img((transition, imgs)) = Request::receive(&bytes) {
                thread::scope(|s| {
                    for ((ArchivedImg { img, path }, _), wallpapers) in imgs.iter().zip(wallpapers)
                    {
                        let pool = Arc::clone(&pool);
                        Self::spawn_transition_thread(s, transition, img, path, wallpapers, pool);
                    }
                });
            }
        }) {
            Ok(_) => Answer::Ok,
            Err(e) => Answer::Err(e.to_string()),
        }
    }

    fn spawn_animation_thread<'a, 'b>(
        scope: &'a Scope<'b, '_>,
        animation: &'b ArchivedAnimation,
        mut wallpapers: Vec<Arc<Wallpaper>>,
        pool: Arc<Mutex<SlotPool>>,
        barrier: Arc<sync_barrier::SyncBarrier>,
    ) where
        'a: 'b,
    {
        if let Err(e) = thread::Builder::new()
            .name("animation".to_string()) //Name our threads  for better log messages
            .stack_size(STACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn_scoped(scope, move || {
                /* We only need to animate if we have > 1 frame */
                if animation.animation.len() == 1 {
                    return;
                }

                for wallpaper in &wallpapers {
                    wallpaper.wait_for_animation();
                    wallpaper.begin_animation();
                }

                let mut now = std::time::Instant::now();

                for (frame, duration) in animation.animation.iter().cycle() {
                    let duration = duration.deserialize(&mut rkyv::Infallible).unwrap();
                    if animation.sync {
                        barrier.inc_and_wait(duration);
                    }

                    wallpapers.retain(|wallpaper| {
                        if wallpaper.animation_should_stop() {
                            wallpaper.end_animation();
                            return false;
                        }
                        let mut pool = wallpaper.lock_pool_to_get_canvas(&pool);
                        let canvas = wallpaper.get_canvas(&mut pool);
                        if !frame.unpack(canvas) {
                            error!("failed to unpack frame, canvas has the wrong size");
                            return false;
                        }
                        wallpaper.draw(&mut pool);
                        true
                    });

                    if wallpapers.is_empty() {
                        return;
                    }

                    let timeout = duration.saturating_sub(now.elapsed());
                    thread::sleep(timeout);
                    nix::sys::signal::kill(nix::unistd::Pid::this(), nix::sys::signal::SIGUSR1)
                        .unwrap();
                    now = std::time::Instant::now();
                }
            })
        {
            error!("failed to spawn 'animation' thread: {}", e);
        }
    }

    pub fn animate(
        &mut self,
        pool: &Arc<Mutex<SlotPool>>,
        bytes: Vec<u8>,
        wallpapers: Vec<Vec<Arc<Wallpaper>>>,
    ) -> Answer {
        let pool = Arc::clone(pool);
        let barrier = Arc::clone(&self.sync_barrier);
        match thread::Builder::new().stack_size(1 << 15).spawn(move || {
            thread::scope(|s| {
                if let ArchivedRequest::Animation(animations) = Request::receive(&bytes) {
                    for ((animation, _), wallpapers) in animations.iter().zip(wallpapers) {
                        let pool = Arc::clone(&pool);
                        let barrier = Arc::clone(&barrier);
                        Self::spawn_animation_thread(s, animation, wallpapers, pool, barrier);
                    }
                }
            });
        }) {
            Ok(_) => Answer::Ok,
            Err(e) => Answer::Err(e.to_string()),
        }
    }
}
