use log::{debug, error, warn};
use utils::ipc::{BgImg, BgInfo, Scale};

use std::{
    num::NonZeroI32,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex, RwLock,
    },
};

use crate::wayland::{
    bump_pool::BumpPool,
    globals,
    interfaces::{
        wl_output, wl_surface, wp_fractional_scale_v1, wp_viewport, zwlr_layer_surface_v1,
    },
    ObjectId, WlDynObj,
};

#[derive(Debug)]
struct AnimationState {
    id: AtomicUsize,
}

#[derive(Debug)]
pub(super) struct AnimationToken {
    id: usize,
}

struct FrameCallbackHandler {
    cvar: Condvar,
    done: Mutex<bool>,
    callback: Mutex<ObjectId>,
}

impl FrameCallbackHandler {
    fn new(surface: ObjectId) -> Self {
        let callback = globals::object_create(WlDynObj::Callback);
        wl_surface::req::frame(surface, callback).unwrap();
        FrameCallbackHandler {
            cvar: Condvar::new(),
            done: Mutex::new(true), // we do not have to wait for the first frame
            callback: Mutex::new(callback),
        }
    }

    fn request_frame_callback(&self, surface: ObjectId) {
        let callback = globals::object_create(WlDynObj::Callback);
        wl_surface::req::frame(surface, callback).unwrap();
        *self.callback.lock().unwrap() = callback;
    }
}

/// Owns all the necessary information for drawing.
#[derive(Clone, Debug)]
struct WallpaperInner {
    name: Option<String>,
    desc: Option<String>,
    width: NonZeroI32,
    height: NonZeroI32,
    scale_factor: Scale,
    transform: u32,
}

impl Default for WallpaperInner {
    fn default() -> Self {
        Self {
            name: None,
            desc: None,
            width: unsafe { NonZeroI32::new_unchecked(4) },
            height: unsafe { NonZeroI32::new_unchecked(4) },
            scale_factor: Scale::Whole(unsafe { NonZeroI32::new_unchecked(1) }),
            transform: wl_output::transform::NORMAL,
        }
    }
}

pub(super) struct Wallpaper {
    output: ObjectId,
    output_name: u32,
    wl_surface: ObjectId,
    wp_viewport: ObjectId,
    #[allow(unused)]
    wp_fractional: Option<ObjectId>,
    layer_surface: ObjectId,

    inner: RwLock<WallpaperInner>,
    inner_staging: Mutex<WallpaperInner>,

    animation_state: AnimationState,
    pub configured: AtomicBool,

    frame_callback_handler: FrameCallbackHandler,
    img: Mutex<BgImg>,
    pool: Mutex<BumpPool>,
}

impl Wallpaper {
    pub(crate) fn new(
        output: ObjectId,
        output_name: u32,
        wl_surface: ObjectId,
        wp_viewport: ObjectId,
        wp_fractional: Option<ObjectId>,
        layer_surface: ObjectId,
    ) -> Self {
        let inner = RwLock::default();
        let inner_staging = Mutex::default();

        // Configure the layer surface
        zwlr_layer_surface_v1::req::set_anchor(layer_surface, 15).unwrap();
        zwlr_layer_surface_v1::req::set_exclusive_zone(layer_surface, -1).unwrap();
        zwlr_layer_surface_v1::req::set_margin(layer_surface, 0, 0, 0, 0).unwrap();
        zwlr_layer_surface_v1::req::set_keyboard_interactivity(
            layer_surface,
            zwlr_layer_surface_v1::keyboard_interactivity::NONE,
        )
        .unwrap();
        wl_surface::req::set_buffer_scale(wl_surface, 1).unwrap();

        let frame_callback_handler = FrameCallbackHandler::new(wl_surface);
        // commit so that the compositor send the initial configuration
        wl_surface::req::commit(wl_surface).unwrap();

        let pool = Mutex::new(BumpPool::new(256, 256));

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
            },
            configured: AtomicBool::new(false),
            frame_callback_handler,
            img: Mutex::new(BgImg::Color([0, 0, 0])),
            pool,
        }
    }

    pub fn get_bg_info(&self) -> BgInfo {
        let inner = self.inner.read().unwrap();
        BgInfo {
            name: inner.name.clone().unwrap_or("?".to_string()),
            dim: (inner.width.get() as u32, inner.height.get() as u32),
            scale_factor: inner.scale_factor,
            img: self.img.lock().unwrap().clone(),
            pixel_format: globals::pixel_format(),
        }
    }

    pub fn set_name(&self, name: String) {
        debug!("Output {} name: {name}", self.output_name);
        self.inner_staging.lock().unwrap().name = Some(name);
    }

    pub fn set_desc(&self, desc: String) {
        debug!("Output {} description: {desc}", self.output_name);
        self.inner_staging.lock().unwrap().desc = Some(desc)
    }

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
    }

    pub fn set_transform(&self, transform: u32) {
        self.inner_staging.lock().unwrap().transform = transform;
    }

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

    pub fn commit_surface_changes(&self, use_cache: bool) {
        use wl_output::transform;
        let mut inner = self.inner.write().unwrap();
        let staging = self.inner_staging.lock().unwrap();

        if inner.name != staging.name && use_cache {
            let name = staging.name.clone().unwrap_or("".to_string());
            std::thread::Builder::new()
                .name("cache loader".to_string())
                .stack_size(1 << 14)
                .spawn(move || {
                    if let Err(e) = utils::cache::load(&name) {
                        warn!("failed to load cache: {e}");
                    }
                })
                .unwrap(); // builder only fails if `name` contains null bytes
        }

        let (width, height) = if matches!(
            staging.transform,
            transform::_90 | transform::_270 | transform::FLIPPED_90 | transform::FLIPPED_270
        ) {
            (staging.height, staging.width)
        } else {
            (staging.width, staging.height)
        };

        if staging.scale_factor != inner.scale_factor || staging.transform != inner.transform {
            match staging.scale_factor {
                Scale::Whole(i) => {
                    // unset destination
                    wp_viewport::req::set_destination(self.wp_viewport, -1, -1).unwrap();
                    wl_surface::req::set_buffer_scale(self.wl_surface, i.get()).unwrap();
                }
                Scale::Fractional(_) => {
                    wl_surface::req::set_buffer_scale(self.wl_surface, 1).unwrap();
                    wp_viewport::req::set_destination(self.wp_viewport, width.get(), height.get())
                        .unwrap();
                }
            }
        }

        inner.scale_factor = staging.scale_factor;
        inner.transform = staging.transform;
        inner.name.clone_from(&staging.name);
        inner.desc.clone_from(&staging.desc);
        if (inner.width, inner.height) == (width, height) {
            return;
        }
        self.stop_animations();
        inner.width = width;
        inner.height = height;

        let scale_factor = staging.scale_factor;
        drop(inner);
        drop(staging);

        zwlr_layer_surface_v1::req::set_size(
            self.layer_surface,
            width.get() as u32,
            height.get() as u32,
        )
        .unwrap();

        let (w, h) = scale_factor.mul_dim(width.get(), height.get());
        self.pool.lock().unwrap().resize(w, h);

        self.frame_callback_handler
            .request_frame_callback(self.wl_surface);
        wl_surface::req::commit(self.wl_surface).unwrap();
        self.configured
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn has_name(&self, name: &str) -> bool {
        match self.inner.read().unwrap().name.as_ref() {
            Some(n) => n == name,
            None => false,
        }
    }

    pub(super) fn has_output(&self, output: ObjectId) -> bool {
        self.output == output
    }

    pub(super) fn has_output_name(&self, name: u32) -> bool {
        self.output_name == name
    }

    pub(super) fn has_surface(&self, wl_surface: ObjectId) -> bool {
        self.wl_surface == wl_surface
    }

    pub(super) fn has_layer_surface(&self, layer_surface: ObjectId) -> bool {
        self.layer_surface == layer_surface
    }

    pub(super) fn try_set_buffer_release_flag(&self, buffer: ObjectId) -> bool {
        let pool = self.pool.lock().unwrap();
        if let Some(release_flag) = pool.get_buffer_release_flag(buffer) {
            release_flag.set_released();
            true
        } else {
            false
        }
    }

    pub(super) fn has_callback(&self, callback: ObjectId) -> bool {
        *self.frame_callback_handler.callback.lock().unwrap() == callback
    }

    pub(super) fn has_fractional_scale(&self, fractional_scale: ObjectId) -> bool {
        self.wp_fractional.is_some_and(|f| f == fractional_scale)
    }

    pub(super) fn has_animation_id(&self, token: &AnimationToken) -> bool {
        self.animation_state
            .id
            .load(std::sync::atomic::Ordering::Acquire)
            == token.id
    }

    pub(super) fn get_dimensions(&self) -> (u32, u32) {
        let inner = self.inner.read().unwrap();
        let dim = inner
            .scale_factor
            .mul_dim(inner.width.get(), inner.height.get());
        (dim.0 as u32, dim.1 as u32)
    }

    pub(super) fn canvas_change<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut [u8]) -> T,
    {
        f(self.pool.lock().unwrap().get_drawable())
    }

    pub(super) fn create_animation_token(&self) -> AnimationToken {
        let id = self.animation_state.id.load(Ordering::Acquire);
        AnimationToken { id }
    }

    pub(super) fn frame_callback_completed(&self) {
        *self.frame_callback_handler.done.lock().unwrap() = true;
        self.frame_callback_handler.cvar.notify_all();
    }

    /// Stops all animations with the current id, by increasing that id
    pub(super) fn stop_animations(&self) {
        self.animation_state.id.fetch_add(1, Ordering::AcqRel);
    }

    pub(super) fn clear(&self, color: [u8; 3]) {
        self.canvas_change(|canvas| {
            for pixel in canvas.chunks_exact_mut(globals::pixel_format().channels().into()) {
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
            let mut done = self.frame_callback_handler.done.lock().unwrap();
            while !*done {
                debug!("waiting for condvar");
                done = self.frame_callback_handler.cvar.wait(done).unwrap();
            }
            *done = false;
        }
        let inner = self.inner.read().unwrap();
        if let Some(buf) = self.pool.lock().unwrap().get_commitable_buffer() {
            let (width, height) = inner
                .scale_factor
                .mul_dim(inner.width.get(), inner.height.get());
            wl_surface::req::attach(self.wl_surface, Some(buf), 0, 0).unwrap();
            drop(inner);
            wl_surface::req::damage_buffer(self.wl_surface, 0, 0, width, height).unwrap();
            self.frame_callback_handler
                .request_frame_callback(self.wl_surface);
        } else {
            drop(inner);
            // send another frame request, since we consumed the previous one
            self.frame_callback_handler
                .request_frame_callback(self.wl_surface);
        }
    }
}

/// commits multiple wallpapers at once with a single message through the socket
pub(crate) fn commit_wallpapers(wallpapers: &[Arc<Wallpaper>]) {
    let mut msg = Vec::with_capacity(wallpapers.len());
    for wallpaper in wallpapers {
        let object_id = wallpaper.wl_surface.get() as u64;
        msg.push(object_id | 0x0008000600000000);
    }
    let len = wallpapers.len() << 3;
    unsafe {
        let msg = std::slice::from_raw_parts(msg.as_ptr() as *mut u8, len);
        crate::wayland::wire::send_unchecked(msg, &[]).unwrap()
    }
}

impl Drop for Wallpaper {
    fn drop(&mut self) {
        // note we shouldn't panic in a drop implementation
        if let Err(e) = wl_surface::req::destroy(self.wl_surface) {
            error!("error destroying wl_surface: {e:?}");
        }
        if let Err(e) = wp_viewport::req::destroy(self.wp_viewport) {
            error!("error destroying wp_viewport: {e:?}");
        }
        if let Some(fractional) = self.wp_fractional {
            if let Err(e) = wp_fractional_scale_v1::req::destroy(fractional) {
                error!("error destroying wp_fractional_scale_v1: {e:?}");
            }
        }
        if let Err(e) = zwlr_layer_surface_v1::req::destroy(self.layer_surface) {
            error!("error destroying zwlr_layer_surface_v1: {e:?}");
        }

        if let Ok(read) = self.inner.read() {
            debug!(
                "Destroyed output {} - {}",
                read.name.as_ref().unwrap_or(&"?".to_string()),
                read.desc.as_ref().unwrap_or(&"?".to_string())
            );
        }
    }
}

unsafe impl Sync for Wallpaper {}
unsafe impl Send for Wallpaper {}
