use log::error;
use rkyv::{boxed::ArchivedBox, string::ArchivedString, Deserialize};

use std::{
    sync::Arc,
    thread::{self, Scope},
    time::Duration,
};

use utils::{
    compression::Decompressor,
    ipc::{
        Answer, ArchivedAnimation, ArchivedImg, ArchivedRequest, ArchivedTransition, BgImg, Request,
    },
};

use crate::wallpaper::{AnimationToken, Wallpaper};

mod anim_barrier;
mod transitions;
use transitions::Transition;

use self::anim_barrier::ArcAnimBarrier;

///The default thread stack size of 2MiB is way too overkill for our purposes
const STACK_SIZE: usize = 1 << 17; //128KiB

pub(super) struct Animator {
    anim_barrier: ArcAnimBarrier,
}

impl Animator {
    pub(super) fn new() -> Self {
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

                if img.len()
                    == dimensions.0 as usize
                        * dimensions.1 as usize
                        * crate::pixel_format().channels() as usize
                {
                    Transition::new(wallpapers, dimensions, transition.clone()).execute(img);
                } else {
                    error!(
                        "image is of wrong size! Image len: {}, expected size: {}",
                        img.len(),
                        dimensions.0 as usize
                            * dimensions.1 as usize
                            * crate::pixel_format().channels() as usize
                    );
                }
            })
        {
            error!("failed to spawn 'transition' thread: {}", e);
        }
    }

    pub(super) fn transition(
        &mut self,
        bytes: Vec<u8>,
        wallpapers: Vec<Vec<Arc<Wallpaper>>>,
    ) -> Answer {
        match thread::Builder::new()
            .stack_size(1 << 15)
            .name("transition spawner".to_string())
            .spawn(move || {
                if let ArchivedRequest::Img((transition, imgs)) = Request::receive(&bytes) {
                    thread::scope(|s| {
                        for ((ArchivedImg { img, path }, _), wallpapers) in
                            imgs.iter().zip(wallpapers)
                        {
                            Self::spawn_transition_thread(s, transition, img, path, wallpapers);
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
        barrier: ArcAnimBarrier,
    ) where
        'a: 'b,
    {
        if let Err(e) = thread::Builder::new()
            .name("animation".to_string()) //Name our threads  for better log messages
            .stack_size(STACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn_scoped(scope, move || {
                /* We only need to animate if we have > 1 frame */
                if animation.animation.len() <= 1 {
                    return;
                }
                log::debug!("Starting animation");

                let mut tokens: Vec<AnimationToken> = wallpapers
                    .iter()
                    .map(|w| w.create_animation_token())
                    .collect();

                for (wallpaper, token) in wallpapers.iter().zip(&tokens) {
                    loop {
                        if !wallpaper.has_animation_id(token) || token.is_transition_done() {
                            break;
                        }
                        let duration: Duration = animation.animation[0]
                            .1
                            .deserialize(&mut rkyv::Infallible)
                            .unwrap();
                        std::thread::sleep(duration / 2);
                    }
                }

                let mut now = std::time::Instant::now();

                let mut decompressor = Decompressor::new();
                for (frame, duration) in animation.animation.iter().cycle() {
                    let duration: Duration = duration.deserialize(&mut rkyv::Infallible).unwrap();
                    barrier.wait(duration.div_f32(2.0));

                    let mut i = 0;
                    while i < wallpapers.len() {
                        let token = &tokens[i];
                        if !wallpapers[i].has_animation_id(token) {
                            wallpapers.swap_remove(i);
                            tokens.swap_remove(i);
                            continue;
                        }

                        let result = wallpapers[i].canvas_change(|canvas| {
                            decompressor.decompress_archived(frame, canvas, crate::pixel_format())
                        });

                        if let Err(e) = result {
                            error!("failed to unpack frame: {e}");
                            wallpapers.swap_remove(i);
                            tokens.swap_remove(i);
                            continue;
                        }

                        wallpapers[i].draw();
                        i += 1;
                    }

                    if wallpapers.is_empty() {
                        return;
                    }

                    let timeout = duration.saturating_sub(now.elapsed());
                    spin_sleep::sleep(timeout);
                    crate::wake_poll();
                    now = std::time::Instant::now();
                }
            })
        {
            error!("failed to spawn 'animation' thread: {}", e);
        }
    }

    pub(super) fn animate(
        &mut self,
        bytes: Vec<u8>,
        wallpapers: Vec<Vec<Arc<Wallpaper>>>,
    ) -> Answer {
        let barrier = self.anim_barrier.clone();
        match thread::Builder::new()
            .stack_size(1 << 15)
            .name("animation spawner".to_string())
            .spawn(move || {
                thread::scope(|s| {
                    if let ArchivedRequest::Animation(animations) = Request::receive(&bytes) {
                        for ((animation, _), wallpapers) in animations.iter().zip(wallpapers) {
                            let barrier = barrier.clone();
                            Self::spawn_animation_thread(s, animation, wallpapers, barrier);
                        }
                    }
                });
            }) {
            Ok(_) => Answer::Ok,
            Err(e) => Answer::Err(e.to_string()),
        }
    }
}
