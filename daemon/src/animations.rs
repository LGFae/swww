use log::error;
use smallvec::SmallVec;
use waybackend::{Waybackend, objman::ObjectManager};

use rustix::time::{ClockId, Timespec, clock_gettime};

use common::{
    compression::Decompressor,
    ipc::{self, BgImg, ImgReq, Nanos, PixelFormat},
    mmap::MmappedBytes,
};

use crate::{WaylandObject, wallpaper::WallpaperCell};

mod transitions;
use transitions::Effect;

pub struct Animator {
    pub wallpapers: SmallVec<[WallpaperCell; 2]>,
    now: Timespec,
    animator: AnimatorKind,
}

enum AnimatorKind {
    Transition(Transition),
    Animation(Animation),
}

impl Animator {
    pub fn new(
        mut wallpapers: SmallVec<[WallpaperCell; 2]>,
        transition: &ipc::Transition,
        img_req: ImgReq,
        animation: Option<ipc::Animation>,
    ) -> Option<Self> {
        let ImgReq { img, path, dim, .. } = img_req;
        if wallpapers.is_empty() {
            return None;
        }
        for w in &mut wallpapers {
            w.borrow_mut().set_img_info(BgImg::Img(path.str().into()));
        }

        let expect = wallpapers[0].borrow().get_dimensions();
        if dim != expect {
            error!("image has wrong dimensions! Expect {expect:?}, actual {dim:?}");
            return None;
        }
        let effect = Some(Effect::new(transition, dim));
        Some(Self {
            wallpapers,
            now: clock_gettime(ClockId::Monotonic),
            animator: AnimatorKind::Transition(Transition {
                effect,
                fps_nanos: Nanos::from_nanos(1_000_000_000 / transition.fps as u64),
                img,
                animation,
            }),
        })
    }

    pub fn time_to_draw(&self) -> Timespec {
        match &self.animator {
            AnimatorKind::Transition(transition) => transition.time_to_draw(&self.now),
            AnimatorKind::Animation(animation) => animation.time_to_draw(&self.now),
        }
    }

    pub fn updt_time(&mut self) {
        self.now = clock_gettime(ClockId::Monotonic);
    }

    pub fn frame(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
    ) -> bool {
        let Self {
            wallpapers,
            animator,
            ..
        } = self;
        match animator {
            AnimatorKind::Transition(transition) => {
                if !transition.frame(backend, objman, wallpapers.as_mut_slice(), pixel_format) {
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
                animation.frame(backend, objman, wallpapers, pixel_format);
                false
            }
        }
    }
}

struct Transition {
    fps_nanos: Nanos,
    effect: Option<Effect>,
    img: MmappedBytes,
    animation: Option<ipc::Animation>,
}

impl Transition {
    fn time_to_draw(&self, start: &Timespec) -> Timespec {
        let now = clock_gettime(ClockId::Monotonic);
        let elapsed = now - *start;
        timespec_saturating_sub(self.fps_nanos.into_timespec(), elapsed)
    }

    fn frame(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        wallpapers: &mut [WallpaperCell],
        pixel_format: PixelFormat,
    ) -> bool {
        let Self { effect, img, .. } = self;
        match effect.as_mut() {
            Some(e) => {
                let over = e.execute(backend, objman, pixel_format, wallpapers, img.bytes());
                if over {
                    *effect = None;
                }
                false
            }
            None => true,
        }
    }
}

struct Animation {
    animation: ipc::Animation,
    decompressor: Decompressor,
    i: usize,
}

impl Animation {
    fn time_to_draw(&self, start: &Timespec) -> Timespec {
        let now = clock_gettime(ClockId::Monotonic);
        let elapsed = now - *start;
        timespec_saturating_sub(
            self.animation.animation[self.i % self.animation.animation.len()]
                .1
                .into_timespec(),
            elapsed,
        )
    }

    fn frame(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        wallpapers: &mut SmallVec<[WallpaperCell; 2]>,
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
            wallpapers.retain(|w| {
                let mut borrow = w.borrow_mut();
                let result = borrow.canvas_change(backend, objman, pixel_format, |canvas| {
                    decompressor.decompress(frame, canvas, pixel_format)
                });
                match result {
                    Ok(()) => true,
                    Err(e) => {
                        error!("failed to unpack frame: {e}");
                        false
                    }
                }
            });
        } else {
            // if we already went through one loop, we can use the unsafe version, because
            // everything was already validated
            for w in wallpapers {
                let mut borrow = w.borrow_mut();
                // SAFETY: we have already validated every frame and removed the ones that have
                // errors in the previous loops. The only ones left should be those that can be
                // decompressed correctly
                borrow.canvas_change(backend, objman, pixel_format, |canvas| unsafe {
                    decompressor.decompress_unchecked(frame, canvas, pixel_format);
                });
            }
        }

        *i += 1;
    }
}

/// inspired by the std Duration implementation
fn timespec_saturating_sub(a: Timespec, b: Timespec) -> Timespec {
    let mut res = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };

    if a.tv_sec >= b.tv_sec {
        let mut secs = a.tv_sec - b.tv_sec;
        let nanos = if a.tv_nsec >= b.tv_nsec {
            a.tv_nsec - b.tv_nsec
        } else if secs > 0 {
            secs -= 1;
            a.tv_nsec + 1_000_000_000 - b.tv_nsec
        } else {
            return res;
        };
        res.tv_sec = secs;
        res.tv_nsec = nanos;
    }

    res
}
