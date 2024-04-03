use utils::ipc::{BgImg, BgInfo};

use std::{
    num::NonZeroI32,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex, RwLock,
    },
};

use smithay_client_toolkit::shell::{
    wlr_layer::{Anchor, KeyboardInteractivity, LayerSurface},
    WaylandSurface,
};

use wayland_client::{
    protocol::{wl_output::WlOutput, wl_shm::WlShm, wl_surface::WlSurface},
    QueueHandle,
};

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
#[derive(Clone, Debug)]
struct WallpaperInner {
    name: Option<String>,
    desc: Option<String>,
    width: NonZeroI32,
    height: NonZeroI32,
    scale_factor: NonZeroI32,
}

impl Default for WallpaperInner {
    fn default() -> Self {
        Self {
            name: None,
            desc: None,
            width: unsafe { NonZeroI32::new_unchecked(4) },
            height: unsafe { NonZeroI32::new_unchecked(4) },
            scale_factor: unsafe { NonZeroI32::new_unchecked(1) },
        }
    }
}

pub(super) struct Wallpaper {
    output: WlOutput,
    inner: RwLock<WallpaperInner>,
    inner_staging: Mutex<WallpaperInner>,

    layer_surface: LayerSurface,
    animation_state: AnimationState,
    pub configured: AtomicBool,
    qh: QueueHandle<Daemon>,
    frame_callback_handler: FrameCallbackHandler,

    img: Mutex<BgImg>,
    pool: Mutex<BumpPool>,
}

impl Wallpaper {
    pub(crate) fn new(
        output: WlOutput,
        layer_surface: LayerSurface,
        shm: &WlShm,
        qh: &QueueHandle<Daemon>,
    ) -> Self {
        let inner = RwLock::default();
        let inner_staging = Mutex::default();

        let frame_callback_handler = FrameCallbackHandler {
            cvar: Condvar::new(),
            time: Mutex::new(Some(0)), // we do not have to wait for the first frame
        };

        // Configure the layer surface
        layer_surface.set_anchor(Anchor::all());
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_margin(0, 0, 0, 0);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.set_size(4, 4);
        layer_surface.set_buffer_scale(1).unwrap();
        // commit so that the compositor send the initial configuration
        layer_surface.commit();
        layer_surface
            .wl_surface()
            .frame(qh, layer_surface.wl_surface().clone());

        let pool = Mutex::new(BumpPool::new(256, 256, shm, qh));

        Self {
            output,
            layer_surface,
            inner,
            inner_staging,
            animation_state: AnimationState {
                id: AtomicUsize::new(0),
                transition_finished: Arc::new(AtomicBool::new(false)),
            },
            configured: AtomicBool::new(false),
            qh: qh.clone(),
            frame_callback_handler,
            img: Mutex::new(BgImg::Color([0, 0, 0])),
            pool,
        }
    }

    #[inline]
    pub fn get_bg_info(&self) -> BgInfo {
        let inner = self.inner.read().unwrap();
        BgInfo {
            name: inner.name.clone().unwrap_or("?".to_string()),
            dim: (inner.width.get() as u32, inner.height.get() as u32),
            scale_factor: inner.scale_factor.get(),
            img: self.img.lock().unwrap().clone(),
            pixel_format: crate::pixel_format(),
        }
    }

    #[inline]
    pub fn set_name(&self, name: String) {
        self.inner_staging.lock().unwrap().name = Some(name);
    }

    #[inline]
    pub fn set_desc(&self, desc: String) {
        self.inner_staging.lock().unwrap().name = Some(desc)
    }

    #[inline]
    pub fn set_dimensions(&self, width: i32, height: i32) {
        let mut lock = self.inner_staging.lock().unwrap();

        if width <= 0 {
            log::error!("invalid width ({width}) for output: {:?}", self.output);
        } else {
            lock.width = unsafe { NonZeroI32::new_unchecked(width) };
        }

        if height <= 0 {
            log::error!("invalid height ({height}) for output: {:?}", self.output);
        } else {
            lock.height = unsafe { NonZeroI32::new_unchecked(height) };
        }
    }

    #[inline]
    pub fn set_scale(&self, scale: i32) {
        if scale <= 0 {
            log::error!("invalid scale ({scale}) for output: {:?}", self.output);
        } else {
            self.inner_staging.lock().unwrap().scale_factor =
                unsafe { NonZeroI32::new_unchecked(scale) }
        }
    }

    #[inline]
    pub fn commit_surface_changes(&self) {
        let mut inner = self.inner.write().unwrap();
        let staging = self.inner_staging.lock().unwrap();

        if (inner.width, inner.height, inner.scale_factor)
            == (staging.width, staging.height, staging.scale_factor)
        {
            // just the name and descriptions changed
            inner.name = staging.name.clone();
            inner.desc = staging.desc.clone();
            return;
        }
        //otherwise, everything changed
        *inner = staging.clone();

        let (width, height, scale_factor) = (staging.width, staging.height, staging.scale_factor);
        drop(inner);
        drop(staging);

        self.stop_animations();

        self.layer_surface
            .set_buffer_scale(scale_factor.get() as u32)
            .unwrap();

        *self.img.lock().unwrap() = BgImg::Color([0, 0, 0]);

        let w = width.get() * scale_factor.get();
        let h = height.get() * scale_factor.get();
        self.pool.lock().unwrap().resize(w, h, &self.qh);

        *self.frame_callback_handler.time.lock().unwrap() = Some(0);
        self.layer_surface
            .set_size(width.get() as u32, height.get() as u32);
        self.layer_surface.commit();
        self.layer_surface
            .wl_surface()
            .frame(&self.qh, self.layer_surface.wl_surface().clone());
        self.configured.store(false, Ordering::Release);
    }

    #[inline]
    pub(super) fn has_name(&self, name: &str) -> bool {
        match self.inner.read().unwrap().name.as_ref() {
            Some(n) => n == name,
            None => false,
        }
    }

    #[inline]
    pub(super) fn has_id(&self, id: u32) -> bool {
        wayland_client::Proxy::id(&self.output).protocol_id() == id
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

    #[inline]
    pub(super) fn canvas_change<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut [u8]) -> T,
    {
        f(self.pool.lock().unwrap().get_drawable(&self.qh))
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

    pub(super) fn clear(&self, color: [u8; 3]) {
        self.canvas_change(|canvas| {
            for pixel in canvas.chunks_exact_mut(crate::pixel_format().channels().into()) {
                pixel[0..3].copy_from_slice(&color);
            }
        })
    }

    pub(super) fn set_img_info(&self, img_info: BgImg) {
        log::debug!("output {:?} - drawing: {}", self.output, img_info);
        *self.img.lock().unwrap() = img_info;
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
        if let Some(buf) = self.pool.lock().unwrap().get_commitable_buffer() {
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
}

impl Drop for Wallpaper {
    fn drop(&mut self) {
        self.output.release()
    }
}
