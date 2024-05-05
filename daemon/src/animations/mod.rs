use log::error;

use std::{
    sync::Arc,
    thread::{self, Scope},
};

use utils::{
    compression::Decompressor,
    ipc::{self, Animation, Answer, BgImg, ImgRecv},
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
        transition: &'b ipc::Transition,
        img: &'b [u8],
        path: &'b str,
        dim: (u32, u32),
        wallpapers: &'b mut Vec<Arc<Wallpaper>>,
    ) where
        'a: 'b,
    {
        thread::Builder::new()
            .name("transition".to_string()) //Name our threads  for better log messages
            .stack_size(STACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn_scoped(scope, move || {
                if wallpapers.is_empty() {
                    return;
                }
                for w in wallpapers.iter_mut() {
                    w.set_img_info(BgImg::Img(path.to_string()));
                }

                let expect = wallpapers[0].get_dimensions();
                if dim != expect {
                    wallpapers.clear();
                    error!("image has wrong dimensions! Expect {expect:?}, actual {dim:?}");
                    return;
                }

                Transition::new(wallpapers, dim, transition).execute(img);
            })
            .unwrap(); // builder only fails if name contains null bytes
    }

    pub(super) fn transition(
        &mut self,
        transition: ipc::Transition,
        imgs: Box<[ImgRecv]>,
        animations: Option<Box<[Animation]>>,
        mut wallpapers: Vec<Vec<Arc<Wallpaper>>>,
    ) -> Answer {
        let barrier = self.anim_barrier.clone();
        thread::Builder::new()
            .stack_size(1 << 15)
            .name("animation spawner".to_string())
            .spawn(move || {
                thread::scope(|s| {
                    for (ImgRecv { img, path, dim, .. }, wallpapers) in
                        imgs.iter().zip(wallpapers.iter_mut())
                    {
                        Self::spawn_transition_thread(
                            s,
                            &transition,
                            img.bytes(),
                            path.str(),
                            *dim,
                            wallpapers,
                        );
                    }
                });
                drop(imgs);
                #[allow(clippy::drop_non_drop)]
                drop(transition);
                if let Some(animations) = animations {
                    thread::scope(|s| {
                        for (animation, wallpapers) in animations.iter().zip(wallpapers) {
                            let barrier = barrier.clone();
                            Self::spawn_animation_thread(s, animation, wallpapers, barrier);
                        }
                    });
                }
            })
            .unwrap(); // builder only fails if name contains null bytes
        Answer::Ok
    }

    fn spawn_animation_thread<'a, 'b>(
        scope: &'a Scope<'b, '_>,
        animation: &'b Animation,
        mut wallpapers: Vec<Arc<Wallpaper>>,
        barrier: ArcAnimBarrier,
    ) where
        'a: 'b,
    {
        thread::Builder::new()
            .name("animation".to_string()) //Name our threads  for better log messages
            .stack_size(STACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn_scoped(scope, move || {
                /* We only need to animate if we have > 1 frame */
                if animation.animation.len() <= 1 || wallpapers.is_empty() {
                    return;
                }
                log::debug!("Starting animation");

                let mut tokens: Vec<AnimationToken> = wallpapers
                    .iter()
                    .map(|w| w.create_animation_token())
                    .collect();

                let mut now = std::time::Instant::now();

                let mut decompressor = Decompressor::new();
                for (frame, duration) in animation.animation.iter().cycle() {
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
                            decompressor.decompress(frame, canvas, crate::pixel_format())
                        });

                        if let Err(e) = result {
                            error!("failed to unpack frame: {e}");
                            wallpapers.swap_remove(i);
                            tokens.swap_remove(i);
                            continue;
                        }

                        i += 1;
                    }

                    if wallpapers.is_empty() {
                        return;
                    }

                    for wallpaper in &wallpapers {
                        wallpaper.draw();
                    }
                    let timeout = duration.saturating_sub(now.elapsed());
                    spin_sleep::sleep(timeout);
                    crate::flush_wayland();

                    now = std::time::Instant::now();
                }
            })
            .unwrap(); // builder only fails if name contains null bytes
    }
}
