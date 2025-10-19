use log::error;
use waybackend::{Waybackend, objman::ObjectManager};

use std::time::{Duration, Instant};

use common::{
    compression::Decompressor,
    ipc::{self, BgImg, ImgReq, PixelFormat},
    mmap::MmappedBytes,
};

use crate::{WaylandObject, wallpaper::Wallpaper};

mod transitions;
use transitions::Effect;

pub struct Animator {
    /// Output names this Animator is responsible for
    pub group: Vec<u32>,
    now: Instant,
    animator: AnimatorKind,
}

enum AnimatorKind {
    Transition(Transition),
    Animation(Animation),
}

impl Animator {
    pub fn new(
        wallpapers: &mut [Wallpaper],
        group: Vec<u32>,
        transition: &ipc::Transition,
        pixel_format: PixelFormat,
        img_req: ImgReq,
        animation: Option<ipc::Animation>,
    ) -> Option<Self> {
        let ImgReq { img, path, dim, .. } = img_req;

        let expect = group.first().map(|i| {
            wallpapers
                .iter()
                .find(|w| w.has_output_name(*i))
                .unwrap()
                .get_dimensions()
        })?;

        if dim != expect {
            error!("image has wrong dimensions! Expect {expect:?}, actual {dim:?}");
            return None;
        }

        for w in wallpapers
            .iter_mut()
            .filter(|w| group.contains(&w.output_name))
        {
            w.set_animating(true);
            w.set_img_info(BgImg::Img(path.str().to_string()));
        }

        let fps = Duration::from_nanos(1_000_000_000 / transition.fps as u64);
        let effect = Effect::new(transition, pixel_format, dim);
        Some(Self {
            group,
            now: Instant::now(),
            animator: AnimatorKind::Transition(Transition {
                effect,
                fps,
                img,
                animation,
                over: false,
            }),
        })
    }

    pub fn time_to_draw(&self) -> std::time::Duration {
        match &self.animator {
            AnimatorKind::Transition(transition) => transition.time_to_draw(&self.now),
            AnimatorKind::Animation(animation) => animation.time_to_draw(&self.now),
        }
    }

    pub fn updt_time(&mut self) {
        self.now = Instant::now();
    }

    pub fn frame(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        wallpapers: &mut [Wallpaper],
        pixel_format: PixelFormat,
    ) -> bool {
        let Self {
            group, animator, ..
        } = self;
        match animator {
            AnimatorKind::Transition(transition) => {
                let wallpapers = wallpapers
                    .iter_mut()
                    .filter(|w| group.contains(&w.output_name));
                if !transition.frame(backend, objman, wallpapers, pixel_format) {
                    return false;
                }
                // Note: it needs to have more than a single frame, otherwise there is no point in
                // animating it
                if let Some(animation) = transition.animation.take()
                    && animation.animation.len() > 1
                {
                    *animator = AnimatorKind::Animation(Animation {
                        animation,
                        decompressor: Decompressor::new(),
                        i: 0,
                    });
                    return false;
                }
                true
            }
            AnimatorKind::Animation(animation) => {
                animation.frame(backend, objman, group, wallpapers, pixel_format);
                false
            }
        }
    }
}

struct Transition {
    fps: Duration,
    effect: Effect,
    img: MmappedBytes,
    animation: Option<ipc::Animation>,
    over: bool,
}

impl Transition {
    fn time_to_draw(&self, now: &Instant) -> std::time::Duration {
        self.fps.saturating_sub(now.elapsed())
    }

    fn frame<'a, W: Iterator<Item = &'a mut Wallpaper>>(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        wallpapers: W,
        pixel_format: PixelFormat,
    ) -> bool {
        let Self {
            effect, img, over, ..
        } = self;
        if !*over {
            *over = effect.execute(backend, objman, pixel_format, wallpapers, img.bytes());
            false
        } else {
            true
        }
    }
}

struct Animation {
    animation: ipc::Animation,
    decompressor: Decompressor,
    i: usize,
}

impl Animation {
    fn time_to_draw(&self, now: &Instant) -> std::time::Duration {
        self.animation.animation[self.i % self.animation.animation.len()]
            .1
            .saturating_sub(now.elapsed())
    }

    fn frame(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        group: &mut Vec<u32>,
        wallpapers: &mut [Wallpaper],
        pixel_format: PixelFormat,
    ) {
        let Self {
            animation,
            decompressor,
            i,
            ..
        } = self;

        let frame = &animation.animation[*i % animation.animation.len()].0;

        if *i < animation.animation.len() {
            for w in wallpapers {
                if let Some(j) = group.iter().position(|g| w.has_output_name(*g)) {
                    let result = w.canvas_change(backend, objman, pixel_format, |canvas| {
                        decompressor.decompress(frame, canvas, pixel_format)
                    });
                    if let Err(e) = result {
                        error!("failed to unpack frame: {e}");
                        group.remove(j);
                    }
                }
            }
        } else {
            // if we already went through one loop, we can use the unsafe version, because
            // everything was already validated
            for w in wallpapers {
                if group.contains(&w.output_name) {
                    // SAFETY: we have already validated every frame and removed the ones that have
                    // errors in the previous loops. The only ones left should be those that can be
                    // decompressed correctly
                    w.canvas_change(backend, objman, pixel_format, |canvas| unsafe {
                        decompressor.decompress_unchecked(frame, canvas, pixel_format)
                    });
                }
            }
        }

        *i += 1;
    }
}
