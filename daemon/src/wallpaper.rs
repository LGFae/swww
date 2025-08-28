use common::ipc::{BgImg, BgInfo, PixelFormat, Scale};
use log::{debug, error, warn};
use waybackend::{objman::ObjectManager, types::ObjectId, Waybackend};

use std::{cell::RefCell, num::NonZeroI32, rc::Rc};

use crate::{
    wayland::{
        bump_pool::BumpPool,
        wl_compositor, wl_output, wl_region, wl_registry, wl_surface,
        wp_fractional_scale_manager_v1, wp_fractional_scale_v1, wp_viewport, wp_viewporter,
        zwlr_layer_shell_v1::{self, Layer},
        zwlr_layer_surface_v1,
    },
    WaylandObject,
};

struct FrameCallbackHandler {
    done: bool,
    callback: ObjectId,
}

impl FrameCallbackHandler {
    fn new(
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        surface: ObjectId,
    ) -> Self {
        let callback = objman.create(WaylandObject::Callback);
        wl_surface::req::frame(backend, surface, callback).unwrap();
        FrameCallbackHandler {
            done: true, // we do not have to wait for the first frame
            callback,
        }
    }

    fn request_frame_callback(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        surface: ObjectId,
    ) {
        let callback = objman.create(WaylandObject::Callback);
        wl_surface::req::frame(backend, surface, callback).unwrap();
        self.callback = callback;
    }
}

/// Owns all the necessary information for drawing.
#[derive(Clone, Debug)]
pub struct WallpaperInner {
    pub name: Option<String>,
    pub desc: Option<String>,
    width: NonZeroI32,
    height: NonZeroI32,
    scale_factor: Scale,
    transform: wl_output::Transform,
}

impl Default for WallpaperInner {
    fn default() -> Self {
        Self {
            name: None,
            desc: None,
            width: unsafe { NonZeroI32::new_unchecked(4) },
            height: unsafe { NonZeroI32::new_unchecked(4) },
            scale_factor: Scale::Output(unsafe { NonZeroI32::new_unchecked(1) }),
            transform: wl_output::Transform::normal,
        }
    }
}

pub(super) struct Wallpaper {
    output: ObjectId,
    output_name: u32,
    pub wl_surface: ObjectId,
    pub wp_viewport: ObjectId,
    pub wp_fractional: Option<ObjectId>,
    pub layer_surface: ObjectId,

    pub inner: WallpaperInner,
    inner_staging: WallpaperInner,

    configured: bool,
    pub dirty: bool,
    pub inited: bool,

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
    pub(crate) fn new(daemon: &mut crate::Daemon, layer: Layer, output_name: u32) -> Self {
        let crate::Daemon {
            objman,
            backend,
            pixel_format,
            registry,
            compositor,
            shm,
            viewporter,
            fractional_scale_manager,
            layer_shell,
            ..
        } = daemon;
        let output = objman.create(WaylandObject::Output);
        wl_registry::req::bind(backend, *registry, output_name, output, "wl_output", 4).unwrap();

        let wl_surface = objman.create(WaylandObject::Surface);
        wl_compositor::req::create_surface(backend, *compositor, wl_surface).unwrap();

        let region = objman.create(WaylandObject::Region);
        wl_compositor::req::create_region(backend, *compositor, region).unwrap();

        wl_surface::req::set_input_region(backend, wl_surface, Some(region)).unwrap();
        wl_region::req::destroy(backend, region).unwrap();

        let layer_surface = objman.create(WaylandObject::LayerSurface);
        zwlr_layer_shell_v1::req::get_layer_surface(
            backend,
            *layer_shell,
            layer_surface,
            wl_surface,
            Some(output),
            layer,
            &format!("swww-daemon{}", daemon.namespace),
        )
        .unwrap();

        let wp_viewport = objman.create(WaylandObject::Viewport);
        wp_viewporter::req::get_viewport(backend, *viewporter, wp_viewport, wl_surface).unwrap();

        let wp_fractional = if let Some(fract_man) = fractional_scale_manager {
            let fractional = objman.create(WaylandObject::FractionalScale);
            wp_fractional_scale_manager_v1::req::get_fractional_scale(
                backend, *fract_man, fractional, wl_surface,
            )
            .unwrap();
            Some(fractional)
        } else {
            None
        };

        let inner = WallpaperInner::default();
        let inner_staging = WallpaperInner::default();

        // Configure the layer surface
        zwlr_layer_surface_v1::req::set_anchor(
            backend,
            layer_surface,
            zwlr_layer_surface_v1::Anchor::TOP
                | zwlr_layer_surface_v1::Anchor::BOTTOM
                | zwlr_layer_surface_v1::Anchor::RIGHT
                | zwlr_layer_surface_v1::Anchor::LEFT,
        )
        .unwrap();
        zwlr_layer_surface_v1::req::set_exclusive_zone(backend, layer_surface, -1).unwrap();
        zwlr_layer_surface_v1::req::set_margin(backend, layer_surface, 0, 0, 0, 0).unwrap();
        zwlr_layer_surface_v1::req::set_keyboard_interactivity(
            backend,
            layer_surface,
            zwlr_layer_surface_v1::KeyboardInteractivity::None,
        )
        .unwrap();
        zwlr_layer_surface_v1::req::set_size(backend, layer_surface, 0, 0).unwrap();
        wl_surface::req::set_buffer_scale(backend, wl_surface, 1).unwrap();

        let frame_callback_handler = FrameCallbackHandler::new(backend, objman, wl_surface);
        // commit so that the compositor send the initial configuration
        wl_surface::req::commit(backend, wl_surface).unwrap();

        let pool = BumpPool::new(backend, objman, *shm, 256, 256, *pixel_format);

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
            configured: false,
            dirty: false,
            inited: false,
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
        self.dirty = true;
    }

    pub fn set_desc(&mut self, desc: String) {
        debug!("Output {} description: {desc}", self.output_name);
        self.inner_staging.desc = Some(desc);
        self.dirty = true;
    }

    pub fn set_dimensions(&mut self, width: i32, height: i32) {
        debug!(
            "Output {} new dimensions: {width}x{height}",
            self.output_name
        );
        let staging = &mut self.inner_staging;

        match NonZeroI32::new(width) {
            Some(width) => staging.width = width,
            None => error!("Cannot set wallpaper width to 0!"),
        }

        match NonZeroI32::new(height) {
            Some(height) => staging.height = height,
            None => error!("Cannot set wallpaper height to 0!"),
        }
        self.dirty = true;
    }

    pub fn set_transform(&mut self, transform: wl_output::Transform) {
        self.inner_staging.transform = transform;
        self.dirty = true;
    }

    pub fn set_scale(&mut self, scale: Scale) {
        debug!("Output {} new scale: {scale}", self.output_name);
        let staging = &mut self.inner_staging;
        if staging.scale_factor == scale || scale.priority() < staging.scale_factor.priority() {
            return;
        }

        staging.scale_factor = scale;
        self.dirty = true;
    }

    pub fn commit_surface_changes(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        namespace: &str,
        use_cache: bool,
    ) {
        use wl_output::Transform;
        let inner = &mut self.inner;
        let staging = &self.inner_staging;

        if (self.configured && !self.inited && use_cache) || self.img.is_set() {
            let name = staging.name.clone().unwrap_or("".to_string());
            let namespace = namespace.to_string();
            std::thread::Builder::new()
                .name("cache loader".to_string())
                .stack_size(1 << 14)
                .spawn(move || {
                    if let Err(e) = common::cache::load(&name, &namespace) {
                        warn!("failed to load cache: {e}");
                    }
                })
                .unwrap(); // builder only fails if `name` contains null bytes
        }

        let (width, height) = if matches!(
            staging.transform,
            Transform::_90 | Transform::_270 | Transform::flipped_90 | Transform::flipped_270
        ) {
            (staging.height, staging.width)
        } else {
            (staging.width, staging.height)
        };

        wp_viewport::req::set_destination(backend, self.wp_viewport, width.get(), height.get())
            .unwrap();

        inner.scale_factor = staging.scale_factor;
        inner.width = width;
        inner.height = height;
        inner.transform = staging.transform;
        inner.name.clone_from(&staging.name);
        inner.desc.clone_from(&staging.desc);

        log::debug!(
            "Output {} new configuration: width: {width}, height: {height}, scale_factor: {}",
            self.output_name,
            inner.scale_factor
        );

        let (w, h) = inner.scale_factor.mul_dim(width.get(), height.get());
        self.pool.resize(backend, w, h);

        self.frame_callback_handler
            .request_frame_callback(backend, objman, self.wl_surface);
        if !self.configured {
            self.clear(backend, objman, pixel_format, [0, 0, 0]);
            self.attach_buffer_and_damage_surface(backend, objman);
        } else {
            self.inited = true;
        }
        wl_surface::req::commit(backend, self.wl_surface).unwrap();
        self.configured = true;
        self.dirty = false;
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
        backend: &mut Waybackend,
        buffer: ObjectId,
        rc_strong_count: usize,
    ) -> bool {
        self.pool
            .set_buffer_release_flag(backend, buffer, rc_strong_count != 1)
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
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        f: F,
    ) -> T
    where
        F: FnOnce(&mut [u8]) -> T,
    {
        f(self.pool.get_drawable(backend, objman, pixel_format))
    }

    pub(super) fn frame_callback_completed(&mut self) {
        self.frame_callback_handler.done = true;
    }

    pub(super) fn clear(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        color: [u8; 3],
    ) {
        self.canvas_change(backend, objman, pixel_format, |canvas| {
            for pixel in canvas.chunks_exact_mut(pixel_format.channels().into()) {
                pixel[0..3].copy_from_slice(&color);
            }
        })
    }

    pub(super) fn set_img_info(&mut self, img_info: BgImg) {
        debug!("output {:?} - drawing: {}", self.inner.name, img_info);
        self.img = img_info;
    }

    pub(super) fn destroy(&mut self, backend: &mut Waybackend) {
        // carefull not to panic here, since we call this on drop

        if let Err(e) = wp_viewport::req::destroy(backend, self.wp_viewport) {
            error!("error destroying wp_viewport: {e:?}");
        }

        if let Some(fractional) = self.wp_fractional {
            if let Err(e) = wp_fractional_scale_v1::req::destroy(backend, fractional) {
                error!("error destroying wp_fractional_scale_v1: {e:?}");
            }
        }

        if let Err(e) = zwlr_layer_surface_v1::req::destroy(backend, self.layer_surface) {
            error!("error destroying zwlr_layer_surface_v1: {e:?}");
        }

        self.pool.destroy(backend);

        debug!(
            "Destroyed output {} - {}",
            self.inner.name.as_ref().unwrap_or(&"?".to_string()),
            self.inner.desc.as_ref().unwrap_or(&"?".to_string())
        );
    }

    fn attach_buffer_and_damage_surface(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
    ) {
        let surface = self.wl_surface;
        let buf = self.pool.get_commitable_buffer();
        let inner = &self.inner;
        let (width, height) = inner
            .scale_factor
            .mul_dim(inner.width.get(), inner.height.get());

        wl_surface::req::attach(backend, surface, Some(buf), 0, 0).unwrap();
        wl_surface::req::damage_buffer(backend, surface, 0, 0, width, height).unwrap();
        self.frame_callback_handler
            .request_frame_callback(backend, objman, surface);
    }
}

/// attaches all pending buffers and damages all surfaces with one single request
pub(crate) fn attach_buffers_and_damage_surfaces(
    backend: &mut Waybackend,
    objman: &mut ObjectManager<WaylandObject>,
    wallpapers: &[Rc<RefCell<Wallpaper>>],
) {
    for wallpaper in wallpapers {
        wallpaper
            .borrow_mut()
            .attach_buffer_and_damage_surface(backend, objman);
    }
}

/// commits multiple wallpapers at once with a single message through the socket
pub(crate) fn commit_wallpapers(backend: &mut Waybackend, wallpapers: &[Rc<RefCell<Wallpaper>>]) {
    for wallpaper in wallpapers {
        let wallpaper = wallpaper.borrow();
        wl_surface::req::commit(backend, wallpaper.wl_surface).unwrap();
    }
}

unsafe impl Sync for Wallpaper {}
unsafe impl Send for Wallpaper {}
