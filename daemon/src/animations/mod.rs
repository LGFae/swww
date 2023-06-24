use log::error;
use rkyv::{boxed::ArchivedBox, string::ArchivedString, Deserialize};
use smithay_client_toolkit::shm::slot::SlotPool;

use std::{
    sync::Arc,
    sync::Mutex,
    thread::{self, Scope},
    time::Duration,
};

use utils::ipc::{
    Answer, ArchivedAnimation, ArchivedImg, ArchivedRequest, ArchivedTransition, BgImg, Request,
};

use crate::wallpaper::Wallpaper;

mod anim_barrier;
mod transitions;
use transitions::Transition;

use self::anim_barrier::ArcAnimBarrier;

///The default thread stack size of 2MiB is way too overkill for our purposes
const STACK_SIZE: usize = 1 << 17; //128KiB

pub struct Animator {
    anim_barrier: ArcAnimBarrier,
}

impl Animator {
    pub fn new() -> Self {
        Self {
            anim_barrier: ArcAnimBarrier::new(),
        }
    }

    fn spawn_transition_thread<'a, 'b>(
        scope: &'a Scope<'b, '_>,
        transition: &'b ArchivedTransition,
        img: &'b ArchivedBox<[u8]>,
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
        match thread::Builder::new()
            .stack_size(1 << 15)
            .name("animaiton spawner".to_string())
            .spawn(move || {
                if let ArchivedRequest::Img((transition, imgs)) = Request::receive(&bytes) {
                    thread::scope(|s| {
                        for ((ArchivedImg { img, path }, _), wallpapers) in
                            imgs.iter().zip(wallpapers)
                        {
                            let pool = Arc::clone(&pool);
                            Self::spawn_transition_thread(
                                s, transition, img, path, wallpapers, pool,
                            );
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
        barrier: ArcAnimBarrier,
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
                log::debug!("Starting animation");

                for wallpaper in &wallpapers {
                    wallpaper.wait_for_animation();
                    wallpaper.begin_animation();
                }

                let mut now = std::time::Instant::now();

                for (frame, duration) in animation.animation.iter().cycle() {
                    let duration: Duration = duration.deserialize(&mut rkyv::Infallible).unwrap();
                    barrier.wait(duration.div_f32(2.0));

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
        let barrier = self.anim_barrier.clone();
        match thread::Builder::new()
            .stack_size(1 << 15)
            .name("animaiton spawner".to_string())
            .spawn(move || {
                thread::scope(|s| {
                    if let ArchivedRequest::Animation(animations) = Request::receive(&bytes) {
                        for ((animation, _), wallpapers) in animations.iter().zip(wallpapers) {
                            let pool = Arc::clone(&pool);
                            let barrier = barrier.clone();
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
