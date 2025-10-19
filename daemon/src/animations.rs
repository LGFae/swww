use log::error;
use waybackend::{Waybackend, objman::ObjectManager};

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use common::{
    compression::Decompressor,
    ipc::{self, Animation, BgImg, ImgReq, PixelFormat},
    mmap::MmappedBytes,
};

use crate::{WaylandObject, wallpaper::Wallpaper};

mod transitions;
use transitions::Effect;

pub struct TransitionAnimator {
    pub wallpapers: Vec<Rc<RefCell<Wallpaper>>>,
    fps: Duration,
    effect: Effect,
    img: MmappedBytes,
    animation: Option<Animation>,
    now: Instant,
    over: bool,
}

impl TransitionAnimator {
    pub fn new(
        mut wallpapers: Vec<Rc<RefCell<Wallpaper>>>,
        transition: &ipc::Transition,
        pixel_format: PixelFormat,
        img_req: ImgReq,
        animation: Option<Animation>,
    ) -> Option<Self> {
        let ImgReq { img, path, dim, .. } = img_req;
        if wallpapers.is_empty() {
            return None;
        }
        for w in wallpapers.iter_mut() {
            w.borrow_mut()
                .set_img_info(BgImg::Img(path.str().to_string()));
        }

        let expect = wallpapers[0].borrow().get_dimensions();
        if dim != expect {
            error!("image has wrong dimensions! Expect {expect:?}, actual {dim:?}");
            return None;
        }
        let fps = Duration::from_nanos(1_000_000_000 / transition.fps as u64);
        let effect = Effect::new(transition, pixel_format, dim);
        Some(Self {
            wallpapers,
            effect,
            fps,
            img,
            animation,
            now: Instant::now(),
            over: false,
        })
    }

    pub fn time_to_draw(&self) -> std::time::Duration {
        self.fps.saturating_sub(self.now.elapsed())
    }

    pub fn updt_time(&mut self) {
        self.now = Instant::now();
    }

    pub fn frame(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
    ) -> bool {
        let Self {
            wallpapers,
            effect,
            img,
            over,
            ..
        } = self;
        if !*over {
            *over = effect.execute(backend, objman, pixel_format, wallpapers, img.bytes());
            false
        } else {
            true
        }
    }

    pub fn into_image_animator(self) -> Option<ImageAnimator> {
        let Self {
            wallpapers,
            animation,
            ..
        } = self;

        if let Some(animation) = animation {
            // it needs to have more than a single frame, otherwise there is no point in animating
            // it
            if animation.animation.len() > 1 {
                return Some(ImageAnimator {
                    now: Instant::now(),
                    wallpapers,
                    animation,
                    decompressor: Decompressor::new(),
                    i: 0,
                });
            }
        }
        None
    }
}

pub struct ImageAnimator {
    now: Instant,
    pub wallpapers: Vec<Rc<RefCell<Wallpaper>>>,
    animation: Animation,
    decompressor: Decompressor,
    i: usize,
}

impl ImageAnimator {
    pub fn time_to_draw(&self) -> std::time::Duration {
        self.animation.animation[self.i % self.animation.animation.len()]
            .1
            .saturating_sub(self.now.elapsed())
    }

    pub fn updt_time(&mut self) {
        self.now = Instant::now();
    }

    pub fn frame(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
    ) {
        let Self {
            wallpapers,
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
                    decompressor.decompress_unchecked(frame, canvas, pixel_format)
                });
            }
        }

        *i += 1;
    }
}
