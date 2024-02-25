use utils::ipc::BgImg;

use std::{
    num::NonZeroI32,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex, RwLock,
    },
};

use smithay_client_toolkit::{
    output::OutputInfo,
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, LayerSurface},
        WaylandSurface,
    },
    shm::Shm,
};

use wayland_client::{protocol::wl_surface::WlSurface, QueueHandle};

use crate::{bump_pool::BumpPool, Daemon};

#[derive(Debug)]
struct AnimationState {
    id: AtomicUsize,
    transition_finished: Arc<AtomicBool>,
}

#[derive(Debug)]
pub(super) struct AnimationToken {
    id: usize,
    transition_done: Arc<AtomicBool>,
}

impl AnimationToken {
    pub(super) fn is_transition_done(&self) -> bool {
        self.transition_done.load(Ordering::Acquire)
    }

    pub(super) fn set_transition_done(&self, wallpaper: &Wallpaper) {
        if wallpaper.has_animation_id(self) {
            self.transition_done.store(true, Ordering::Release);
        }
    }
}

struct FrameCallbackHandler {
    cvar: Condvar,
    /// This time doesn't really mean anything. We don't really use it for frame timing, but we
    /// store it for the sake of signaling when the compositor emitted the last frame callback
    time: Mutex<Option<u32>>,
}

/// Owns all the necessary information for drawing.
#[derive(Debug)]
struct WallpaperInner {
    width: NonZeroI32,
    height: NonZeroI32,
    scale_factor: NonZeroI32,

    pool: BumpPool,
    img: BgImg,
}

pub(super) struct Wallpaper {
    output_id: u32,
    inner: RwLock<WallpaperInner>,
    layer_surface: LayerSurface,

    animation_state: AnimationState,
    pub configured: AtomicBool,
    qh: QueueHandle<Daemon>,
    frame_callback_handler: FrameCallbackHandler,
}

impl Wallpaper {
    pub(crate) fn new(
        output_info: OutputInfo,
        layer_surface: LayerSurface,
        shm: &Shm,
        qh: &QueueHandle<Daemon>,
    ) -> Self {
        let (width, height): (NonZeroI32, NonZeroI32) = if let Some(size) = output_info.logical_size
        {
            if size.0 == 0 || size.1 == 0 {
                (256.try_into().unwrap(), 256.try_into().unwrap())
            } else {
                (size.0.try_into().unwrap(), size.1.try_into().unwrap())
            }
        } else {
            (256.try_into().unwrap(), 256.try_into().unwrap())
        };

        let scale_factor = NonZeroI32::new(output_info.scale_factor).unwrap();

        let frame_callback_handler = FrameCallbackHandler {
            cvar: Condvar::new(),
            time: Mutex::new(Some(0)), // we do not have to wait for the first frame
        };

        // Configure the layer surface
        layer_surface.set_anchor(Anchor::all());
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_margin(0, 0, 0, 0);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.set_size(width.get() as u32, height.get() as u32);
        layer_surface
            .set_buffer_scale(scale_factor.get() as u32)
            .unwrap();
        // commit so that the compositor send the initial configuration
        layer_surface.commit();
        layer_surface
            .wl_surface()
            .frame(qh, layer_surface.wl_surface().clone());

        let w = width.get() * scale_factor.get();
        let h = height.get() * scale_factor.get();
        let pool = BumpPool::new(w, h, shm, qh);

        Self {
            output_id: output_info.id,
            layer_surface,
            inner: RwLock::new(WallpaperInner {
                width,
                height,
                scale_factor,
                img: BgImg::Color([0, 0, 0]),
                pool,
            }),
            animation_state: AnimationState {
                id: AtomicUsize::new(0),
                transition_finished: Arc::new(AtomicBool::new(false)),
            },
            configured: AtomicBool::new(false),
            qh: qh.clone(),
            frame_callback_handler,
        }
    }

    #[inline]
    pub(super) fn has_id(&self, id: u32) -> bool {
        self.output_id == id
    }

    #[inline]
    pub(super) fn has_animation_id(&self, token: &AnimationToken) -> bool {
        self.animation_state
            .id
            .load(std::sync::atomic::Ordering::Acquire)
            == token.id
    }

    #[inline]
    pub(super) fn has_surface(&self, surface: &WlSurface) -> bool {
        self.layer_surface.wl_surface() == surface
    }

    pub(super) fn get_dimensions(&self) -> (u32, u32) {
        let inner = self.inner.read().unwrap();
        let width = inner.width.get() as u32;
        let height = inner.height.get() as u32;
        let scale_factor = inner.scale_factor.get() as u32;
        (width * scale_factor, height * scale_factor)
    }

    pub(super) fn canvas_change<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut [u8]) -> T,
    {
        let mut inner = self.inner.write().unwrap();
        f(inner.pool.get_drawable(&self.qh))
    }

    #[inline]
    pub(super) fn get_img_info(&self) -> BgImg {
        self.inner.read().unwrap().img.clone()
    }

    #[inline]
    pub(super) fn create_animation_token(&self) -> AnimationToken {
        let id = self.animation_state.id.load(Ordering::Acquire);
        AnimationToken {
            id,
            transition_done: Arc::clone(&self.animation_state.transition_finished),
        }
    }

    #[inline]
    pub(super) fn frame_callback_completed(&self, time: u32) {
        *self.frame_callback_handler.time.lock().unwrap() = Some(time);
        self.frame_callback_handler.cvar.notify_all();
    }

    /// Stops all animations with the current id, by increasing that id
    #[inline]
    pub(super) fn stop_animations(&self) {
        self.animation_state.id.fetch_add(1, Ordering::AcqRel);
        self.animation_state
            .transition_finished
            .store(false, Ordering::Release);
    }

    pub(super) fn clear(&self, mut color: [u8; 3]) {
        let pixel_format = super::pixel_format();

        if pixel_format.must_swap_r_and_b_channels() {
            color.swap(0, 2);
        }

        self.canvas_change(|canvas| {
            for pixel in canvas.chunks_exact_mut(pixel_format.channels().into()) {
                pixel[0..3].copy_from_slice(&color);
            }
        })
    }

    pub(super) fn set_img_info(&self, img_info: BgImg) {
        log::debug!("output {} - drawing: {}", self.output_id, img_info);
        self.inner.write().unwrap().img = img_info;
    }

    pub(super) fn draw(&self) {
        {
            let mut time = self.frame_callback_handler.time.lock().unwrap();
            while time.is_none() {
                log::debug!("waiting for condvar");
                time = self.frame_callback_handler.cvar.wait(time).unwrap();
            }
            *time = None;
        }
        let inner = self.inner.read().unwrap();
        if let Some(buf) = inner.pool.get_commitable_buffer() {
            let width = inner.width.get() * inner.scale_factor.get();
            let height = inner.height.get() * inner.scale_factor.get();
            let surface = self.layer_surface.wl_surface();
            surface.attach(Some(buf), 0, 0);
            drop(inner);
            surface.damage_buffer(0, 0, width, height);
            surface.commit();
            surface.frame(&self.qh, surface.clone());
        } else {
            drop(inner);
            // commit and send another frame request, since we consumed the previous one
            let surface = self.layer_surface.wl_surface();
            surface.commit();
            surface.frame(&self.qh, surface.clone());
        }
    }

    pub(super) fn resize(
        &self,
        width: Option<NonZeroI32>,
        height: Option<NonZeroI32>,
        scale_factor: Option<NonZeroI32>,
    ) {
        if let Some(s) = scale_factor {
            self.layer_surface.set_buffer_scale(s.get() as u32).unwrap();
        }
        let mut inner = self.inner.write().unwrap();
        let width = width.unwrap_or(inner.width);
        let height = height.unwrap_or(inner.height);
        let scale_factor = scale_factor.unwrap_or(inner.scale_factor);
        if (width, height, scale_factor) == (inner.width, inner.height, inner.scale_factor) {
            return;
        }
        self.stop_animations();

        inner.width = width;
        inner.height = height;
        inner.scale_factor = scale_factor;
        inner.img = BgImg::Color([0, 0, 0]);

        let w = width.get() * scale_factor.get();
        let h = height.get() * scale_factor.get();
        inner.pool.resize(w, h, &self.qh);
        drop(inner);

        *self.frame_callback_handler.time.lock().unwrap() = Some(0);
        self.layer_surface
            .set_size(width.get() as u32, height.get() as u32);
        self.layer_surface.commit();
        self.layer_surface
            .wl_surface()
            .frame(&self.qh, self.layer_surface.wl_surface().clone());
        self.configured.store(false, Ordering::Release);
    }
}
