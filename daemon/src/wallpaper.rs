use log::{debug, error, warn};
use utils::ipc::{BgImg, BgInfo, Scale};
use wayland_protocols::wp::{
    fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
    viewporter::client::wp_viewport::WpViewport,
};

use std::{
    num::NonZeroI32,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex, RwLock,
    },
};

use wayland_client::{
    protocol::{wl_output::WlOutput, wl_shm::WlShm, wl_surface::WlSurface},
    Proxy, QueueHandle,
};

use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};

use crate::{bump_pool::BumpPool, Daemon, LayerSurface};

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
    scale_factor: Scale,
    is_vertical: bool,
}

impl Default for WallpaperInner {
    fn default() -> Self {
        Self {
            name: None,
            desc: None,
            width: unsafe { NonZeroI32::new_unchecked(4) },
            height: unsafe { NonZeroI32::new_unchecked(4) },
            scale_factor: Scale::Whole(unsafe { NonZeroI32::new_unchecked(1) }),
            is_vertical: false,
        }
    }
}

pub(super) struct Wallpaper {
    output: WlOutput,
    output_name: u32,
    wl_surface: WlSurface,
    wp_viewport: WpViewport,
    #[allow(unused)]
    wp_fractional: Option<WpFractionalScaleV1>,
    layer_surface: LayerSurface,

    inner: RwLock<WallpaperInner>,
    inner_staging: Mutex<WallpaperInner>,

    animation_state: AnimationState,
    pub configured: AtomicBool,
    qh: QueueHandle<Daemon>,
    frame_callback_handler: FrameCallbackHandler,

    img: Mutex<BgImg>,
    pool: Mutex<BumpPool>,
}

impl Wallpaper {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        output: WlOutput,
        output_name: u32,
        wl_surface: WlSurface,
        wp_viewport: WpViewport,
        wp_fractional: Option<WpFractionalScaleV1>,
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
        //layer_surface.set_size(4, 4);
        wl_surface.set_buffer_scale(1);

        // commit so that the compositor send the initial configuration
        wl_surface.commit();
        wl_surface.frame(qh, wl_surface.clone());

        let pool = Mutex::new(BumpPool::new(256, 256, shm));

        Self {
            output,
            output_name,
            wl_surface,
            wp_viewport,
            wp_fractional,
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
            scale_factor: inner.scale_factor,
            img: self.img.lock().unwrap().clone(),
            pixel_format: crate::pixel_format(),
        }
    }

    #[inline]
    pub fn set_name(&self, name: String) {
        debug!("Output {} name: {name}", self.output.id());
        self.inner_staging.lock().unwrap().name = Some(name);
    }

    #[inline]
    pub fn set_desc(&self, desc: String) {
        debug!("Output {} description: {desc}", self.output.id());
        self.inner_staging.lock().unwrap().desc = Some(desc)
    }

    #[inline]
    pub fn set_dimensions(&self, width: i32, height: i32) {
        let mut lock = self.inner_staging.lock().unwrap();
        let (width, height) = lock.scale_factor.div_dim(width, height);

        match NonZeroI32::new(width) {
            Some(width) => lock.width = width,
            None => {
                error!(
                    "dividing width {width} by scale_factor {} results in width 0!",
                    lock.scale_factor
                )
            }
        }

        match NonZeroI32::new(height) {
            Some(height) => lock.height = height,
            None => {
                error!(
                    "dividing height {height} by scale_factor {} results in height 0!",
                    lock.scale_factor
                )
            }
        }

        if lock.is_vertical {
            if lock.width > lock.height {
                let t = lock.width;
                lock.width = lock.height;
                lock.height = t;
            }
        } else if lock.width < lock.height {
            let t = lock.width;
            lock.width = lock.height;
            lock.height = t;
        }
    }

    #[inline]
    pub fn set_vertical(&self) {
        let mut lock = self.inner_staging.lock().unwrap();
        lock.is_vertical = true;
    }

    #[inline]
    pub fn set_horizontal(&self) {
        let mut lock = self.inner_staging.lock().unwrap();
        lock.is_vertical = false;
    }

    #[inline]
    pub fn set_scale(&self, scale: Scale) {
        let mut lock = self.inner_staging.lock().unwrap();
        if matches!(lock.scale_factor, Scale::Fractional(_)) && matches!(scale, Scale::Whole(_)) {
            return;
        }

        let (old_width, old_height) = lock
            .scale_factor
            .mul_dim(lock.width.get(), lock.height.get());

        lock.scale_factor = scale;
        let (width, height) = lock.scale_factor.div_dim(old_width, old_height);
        match NonZeroI32::new(width) {
            Some(width) => lock.width = width,
            None => {
                error!(
                    "dividing width {width} by scale_factor {} results in width 0!",
                    lock.scale_factor
                )
            }
        }

        match NonZeroI32::new(height) {
            Some(height) => lock.height = height,
            None => {
                error!(
                    "dividing height {height} by scale_factor {} results in height 0!",
                    lock.scale_factor
                )
            }
        }
    }

    #[inline]
    pub fn commit_surface_changes(&self, use_cache: bool) {
        let mut inner = self.inner.write().unwrap();
        let staging = self.inner_staging.lock().unwrap();

        if inner.name != staging.name && use_cache {
            let name = staging.name.clone().unwrap_or("".to_string());
            if let Err(e) = std::thread::Builder::new()
                .name("cache loader".to_string())
                .stack_size(1 << 14)
                .spawn(move || {
                    if let Err(e) = utils::cache::load(&name) {
                        warn!("failed to load cache: {e}");
                    }
                })
            {
                warn!("failed to spawn `cache loader` thread: {e}");
            }
        }

        if staging.scale_factor != inner.scale_factor {
            match staging.scale_factor {
                Scale::Whole(i) => {
                    // unset destination
                    self.wp_viewport.set_destination(-1, -1);
                    self.wl_surface.set_buffer_scale(i.get());
                }
                Scale::Fractional(_) => {
                    self.wl_surface.set_buffer_scale(1);
                    self.wp_viewport
                        .set_destination(staging.width.get(), staging.height.get());
                }
            }
        }

        if (inner.width, inner.height) == (staging.width, staging.height) {
            inner.scale_factor = staging.scale_factor;
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
            .set_size(width.get() as u32, height.get() as u32);

        let (w, h) = scale_factor.mul_dim(width.get(), height.get());
        self.pool.lock().unwrap().resize(w, h);

        *self.frame_callback_handler.time.lock().unwrap() = Some(0);
        self.wl_surface.commit();
        self.wl_surface.frame(&self.qh, self.wl_surface.clone());
        self.configured
            .store(true, std::sync::atomic::Ordering::Release);
    }

    #[inline]
    pub(super) fn has_name(&self, name: &str) -> bool {
        match self.inner.read().unwrap().name.as_ref() {
            Some(n) => n == name,
            None => false,
        }
    }

    #[inline]
    pub(super) fn has_output(&self, output: &WlOutput) -> bool {
        self.output == *output
    }

    #[inline]
    pub(super) fn has_output_name(&self, name: u32) -> bool {
        self.output_name == name
    }

    #[inline]
    pub(super) fn has_animation_id(&self, token: &AnimationToken) -> bool {
        self.animation_state
            .id
            .load(std::sync::atomic::Ordering::Acquire)
            == token.id
    }

    #[inline]
    pub(super) fn has_surface(&self, wl_surface: &WlSurface) -> bool {
        self.wl_surface == *wl_surface
    }

    #[inline]
    pub(super) fn has_layer_surface(&self, layer_surface: &LayerSurface) -> bool {
        self.layer_surface == *layer_surface
    }

    pub(super) fn get_dimensions(&self) -> (u32, u32) {
        let inner = self.inner.read().unwrap();
        let dim = inner
            .scale_factor
            .mul_dim(inner.width.get(), inner.height.get());
        (dim.0 as u32, dim.1 as u32)
    }

    #[inline]
    pub(super) fn canvas_change<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut [u8]) -> T,
    {
        f(self.pool.lock().unwrap().get_drawable())
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
        debug!(
            "output {:?} - drawing: {}",
            self.inner.read().unwrap().name,
            img_info
        );
        *self.img.lock().unwrap() = img_info;
    }

    pub(super) fn draw(&self) {
        {
            let mut time = self.frame_callback_handler.time.lock().unwrap();
            while time.is_none() {
                debug!("waiting for condvar");
                time = self.frame_callback_handler.cvar.wait(time).unwrap();
            }
            *time = None;
        }
        let inner = self.inner.read().unwrap();
        if let Some(buf) = self.pool.lock().unwrap().get_commitable_buffer() {
            let (width, height) = inner
                .scale_factor
                .mul_dim(inner.width.get(), inner.height.get());
            let surface = &self.wl_surface;
            surface.attach(Some(buf), 0, 0);
            drop(inner);
            surface.damage_buffer(0, 0, width, height);
            surface.commit();
            surface.frame(&self.qh, surface.clone());
        } else {
            drop(inner);
            // commit and send another frame request, since we consumed the previous one
            let surface = &self.wl_surface;
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

unsafe impl Sync for Wallpaper {}
unsafe impl Send for Wallpaper {}
