use common::ipc::{BgImg, BgInfo, PixelFormat, Scale};
use log::{debug, error, warn};

use std::{cell::RefCell, num::NonZeroI32, rc::Rc, sync::atomic::AtomicBool};

use crate::wayland::{
    bump_pool::BumpPool,
    interfaces::{
        wl_output, wl_surface, wp_fractional_scale_v1, wp_viewport, zwlr_layer_surface_v1,
    },
    ObjectId, ObjectManager, WlDynObj,
};

struct FrameCallbackHandler {
    done: bool,
    callback: ObjectId,
}

impl FrameCallbackHandler {
    fn new(objman: &mut ObjectManager, surface: ObjectId) -> Self {
        let callback = objman.create(WlDynObj::Callback);
        wl_surface::req::frame(surface, callback).unwrap();
        FrameCallbackHandler {
            done: true, // we do not have to wait for the first frame
            callback,
        }
    }

    fn request_frame_callback(&mut self, objman: &mut ObjectManager, surface: ObjectId) {
        let callback = objman.create(WlDynObj::Callback);
        wl_surface::req::frame(surface, callback).unwrap();
        self.callback = callback;
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

    inner: WallpaperInner,
    inner_staging: WallpaperInner,

    pub configured: AtomicBool,

    frame_callback_handler: FrameCallbackHandler,
    img: BgImg,
    pool: BumpPool,
}

impl std::cmp::PartialEq for Wallpaper {
    fn eq(&self, other: &Self) -> bool {
        self.output_name == other.output_name
    }
}

impl Wallpaper {
    pub(crate) fn new(
        objman: &mut ObjectManager,
        pixel_format: PixelFormat,
        fractional_scale_manager: Option<ObjectId>,
        output_name: u32,
    ) -> Self {
        use crate::wayland::{self, interfaces::*};
        let output = objman.create(wayland::WlDynObj::Output);
        wl_registry::req::bind(output_name, output, "wl_output", 4).unwrap();

        let wl_surface = objman.create(wayland::WlDynObj::Surface);
        wl_compositor::req::create_surface(wl_surface).unwrap();

        let region = objman.create(wayland::WlDynObj::Region);
        wl_compositor::req::create_region(region).unwrap();

        wl_surface::req::set_input_region(wl_surface, Some(region)).unwrap();
        wl_region::req::destroy(region).unwrap();

        let layer_surface = objman.create(wayland::WlDynObj::LayerSurface);
        zwlr_layer_shell_v1::req::get_layer_surface(
            layer_surface,
            wl_surface,
            Some(output),
            zwlr_layer_shell_v1::layer::BACKGROUND,
            "swww-daemon",
        )
        .unwrap();

        let wp_viewport = objman.create(wayland::WlDynObj::Viewport);
        wp_viewporter::req::get_viewport(wp_viewport, wl_surface).unwrap();

        let wp_fractional = if let Some(fract_man) = fractional_scale_manager {
            let fractional = objman.create(wayland::WlDynObj::FractionalScale);
            wp_fractional_scale_manager_v1::req::get_fractional_scale(
                fract_man, fractional, wl_surface,
            )
            .unwrap();
            Some(fractional)
        } else {
            None
        };

        let inner = WallpaperInner::default();
        let inner_staging = WallpaperInner::default();

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

        let frame_callback_handler = FrameCallbackHandler::new(objman, wl_surface);
        // commit so that the compositor send the initial configuration
        wl_surface::req::commit(wl_surface).unwrap();

        let pool = BumpPool::new(256, 256, objman, pixel_format);

        debug!("New output: {output_name}");
        Self {
            output,
            output_name,
            wl_surface,
            wp_viewport,
            wp_fractional,
            layer_surface,
            inner,
            inner_staging,
            configured: AtomicBool::new(false),
            frame_callback_handler,
            img: BgImg::Color([0, 0, 0]),
            pool,
        }
    }

    pub fn get_bg_info(&self, pixel_format: PixelFormat) -> BgInfo {
        BgInfo {
            name: self.inner.name.clone().unwrap_or("?".to_string()),
            dim: (
                self.inner.width.get() as u32,
                self.inner.height.get() as u32,
            ),
            scale_factor: self.inner.scale_factor,
            img: self.img.clone(),
            pixel_format,
        }
    }

    pub fn set_name(&mut self, name: String) {
        debug!("Output {} name: {name}", self.output_name);
        self.inner_staging.name = Some(name);
    }

    pub fn set_desc(&mut self, desc: String) {
        debug!("Output {} description: {desc}", self.output_name);
        self.inner_staging.desc = Some(desc)
    }

    pub fn set_dimensions(&mut self, width: i32, height: i32) {
        let staging = &mut self.inner_staging;
        let (width, height) = staging.scale_factor.div_dim(width, height);

        match NonZeroI32::new(width) {
            Some(width) => staging.width = width,
            None => {
                error!(
                    "dividing width {width} by scale_factor {} results in width 0!",
                    staging.scale_factor
                )
            }
        }

        match NonZeroI32::new(height) {
            Some(height) => staging.height = height,
            None => {
                error!(
                    "dividing height {height} by scale_factor {} results in height 0!",
                    staging.scale_factor
                )
            }
        }
    }

    pub fn set_transform(&mut self, transform: u32) {
        self.inner_staging.transform = transform;
    }

    pub fn set_scale(&mut self, scale: Scale) {
        let staging = &mut self.inner_staging;
        if staging.scale_factor == scale {
            return;
        }

        let (old_width, old_height) = staging
            .scale_factor
            .mul_dim(staging.width.get(), staging.height.get());

        staging.scale_factor = scale;
        let (width, height) = staging.scale_factor.div_dim(old_width, old_height);
        match NonZeroI32::new(width) {
            Some(width) => staging.width = width,
            None => {
                error!(
                    "dividing width {width} by scale_factor {} results in width 0!",
                    staging.scale_factor
                )
            }
        }

        match NonZeroI32::new(height) {
            Some(height) => staging.height = height,
            None => {
                error!(
                    "dividing height {height} by scale_factor {} results in height 0!",
                    staging.scale_factor
                )
            }
        }
    }

    pub fn commit_surface_changes(&mut self, objman: &mut ObjectManager, use_cache: bool) -> bool {
        use wl_output::transform;
        let inner = &mut self.inner;
        let staging = &self.inner_staging;
        if inner.name != staging.name && use_cache {
            let name = staging.name.clone().unwrap_or("".to_string());
            std::thread::Builder::new()
                .name("cache loader".to_string())
                .stack_size(1 << 14)
                .spawn(move || {
                    if let Err(e) = common::cache::load(&name) {
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
            return false;
        }
        inner.width = width;
        inner.height = height;

        let scale_factor = staging.scale_factor;

        zwlr_layer_surface_v1::req::set_size(
            self.layer_surface,
            width.get() as u32,
            height.get() as u32,
        )
        .unwrap();

        let (w, h) = scale_factor.mul_dim(width.get(), height.get());
        self.pool.resize(w, h);

        self.frame_callback_handler
            .request_frame_callback(objman, self.wl_surface);
        wl_surface::req::commit(self.wl_surface).unwrap();
        self.configured
            .store(true, std::sync::atomic::Ordering::Release);
        true
    }

    pub(super) fn has_name(&self, name: &str) -> bool {
        match self.inner.name.as_ref() {
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

    pub(super) fn try_set_buffer_release_flag(
        &mut self,
        buffer: ObjectId,
        rc_strong_count: usize,
    ) -> bool {
        self.pool
            .set_buffer_release_flag(buffer, rc_strong_count != 1)
    }

    pub fn is_draw_ready(&self) -> bool {
        self.frame_callback_handler.done
    }

    pub(super) fn has_callback(&self, callback: ObjectId) -> bool {
        self.frame_callback_handler.callback == callback
    }

    pub(super) fn has_fractional_scale(&self, fractional_scale: ObjectId) -> bool {
        self.wp_fractional.is_some_and(|f| f == fractional_scale)
    }

    pub(super) fn get_dimensions(&self) -> (u32, u32) {
        let dim = self
            .inner
            .scale_factor
            .mul_dim(self.inner.width.get(), self.inner.height.get());
        (dim.0 as u32, dim.1 as u32)
    }

    pub(super) fn canvas_change<F, T>(
        &mut self,
        objman: &mut ObjectManager,
        pixel_format: PixelFormat,
        f: F,
    ) -> T
    where
        F: FnOnce(&mut [u8]) -> T,
    {
        f(self.pool.get_drawable(objman, pixel_format))
    }

    pub(super) fn frame_callback_completed(&mut self) {
        self.frame_callback_handler.done = true;
    }

    pub(super) fn clear(
        &mut self,
        objman: &mut ObjectManager,
        pixel_format: PixelFormat,
        color: [u8; 3],
    ) {
        self.canvas_change(objman, pixel_format, |canvas| {
            for pixel in canvas.chunks_exact_mut(pixel_format.channels().into()) {
                pixel[0..3].copy_from_slice(&color);
            }
        })
    }

    pub(super) fn set_img_info(&mut self, img_info: BgImg) {
        debug!("output {:?} - drawing: {}", self.inner.name, img_info);
        self.img = img_info;
    }
}

/// attaches all pending buffers and damages all surfaces with one single request
pub(crate) fn attach_buffers_and_damage_surfaces(
    objman: &mut ObjectManager,
    wallpapers: &[Rc<RefCell<Wallpaper>>],
) {
    #[rustfmt::skip]
    // Note this is little-endian specific
    const MSG: [u8; 56] = [
        0, 0, 0, 0,             // wl_surface object id (to be filled)
        1, 0,                   // attach opcode
        20, 0,                  // msg length
        0, 0, 0, 0,             // attach buffer id (to be filled)
        0, 0, 0, 0, 0, 0, 0, 0, // attach arguments
        0, 0, 0, 0,             // wl_surface object id (to be filled)
        9, 0,                   // damage opcode
        24, 0,                  // msg length
        0, 0, 0, 0, 0, 0, 0, 0, // damage first arguments
        0, 0, 0, 0, 0, 0, 0, 0, // damage second arguments (to be filled)
        0, 0, 0, 0,             // wl_surface object id (to be filled)
        3, 0,                   // frame opcode
        12, 0,                  // msg length
        0, 0, 0, 0,             // wl_callback object id (to be filled)
    ];
    let msg: Box<[u8]> = wallpapers
        .iter()
        .flat_map(|wallpaper| {
            let mut wallpaper = wallpaper.borrow_mut();
            let mut msg = MSG;

            let buf = wallpaper.pool.get_commitable_buffer();
            let inner = &wallpaper.inner;
            let (width, height) = inner
                .scale_factor
                .mul_dim(inner.width.get(), inner.height.get());

            // attach
            msg[0..4].copy_from_slice(&wallpaper.wl_surface.get().to_ne_bytes());
            msg[8..12].copy_from_slice(&buf.get().to_ne_bytes());

            //damage buffer
            msg[20..24].copy_from_slice(&wallpaper.wl_surface.get().to_ne_bytes());
            msg[36..40].copy_from_slice(&width.to_ne_bytes());
            msg[40..44].copy_from_slice(&height.to_ne_bytes());

            // frame callback
            let callback = objman.create(WlDynObj::Callback);
            wallpaper.frame_callback_handler.callback = callback;
            msg[44..48].copy_from_slice(&wallpaper.wl_surface.get().to_ne_bytes());
            msg[52..56].copy_from_slice(&callback.get().to_ne_bytes());
            msg
        })
        .collect();
    unsafe { crate::wayland::wire::send_unchecked(msg.as_ref(), &[]).unwrap() }
}

/// commits multiple wallpapers at once with a single message through the socket
pub(crate) fn commit_wallpapers(wallpapers: &[Rc<RefCell<Wallpaper>>]) {
    // Note this is little-endian specific
    #[rustfmt::skip]
    const MSG: [u8; 8] = [
        0, 0, 0, 0, // wl_surface object id (to be filled)
        6, 0,       // commit opcode
        8, 0,       // msg length
    ];
    let msg: Box<[u8]> = wallpapers
        .iter()
        .flat_map(|wallpaper| {
            let mut msg = MSG;
            msg[0..4].copy_from_slice(&wallpaper.borrow().wl_surface.get().to_ne_bytes());
            msg
        })
        .collect();
    unsafe { crate::wayland::wire::send_unchecked(msg.as_ref(), &[]).unwrap() }
}

impl Drop for Wallpaper {
    fn drop(&mut self) {
        // note we shouldn't panic in a drop implementation

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

        debug!(
            "Destroyed output {} - {}",
            self.inner.name.as_ref().unwrap_or(&"?".to_string()),
            self.inner.desc.as_ref().unwrap_or(&"?".to_string())
        );
    }
}

unsafe impl Sync for Wallpaper {}
unsafe impl Send for Wallpaper {}
