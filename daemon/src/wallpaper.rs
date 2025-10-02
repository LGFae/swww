use common::ipc::{BgImg, BgInfo, PixelFormat, Scale};
use log::{debug, error, warn};
use waybackend::{Waybackend, objman::ObjectManager, types::ObjectId};

use std::{cell::RefCell, num::NonZeroI32, rc::Rc};

use crate::{
    WaylandObject,
    wayland::{
        bump_pool::BumpPool, wl_compositor, wl_region, wl_surface, wp_fractional_scale_manager_v1,
        wp_fractional_scale_v1, wp_viewport, wp_viewporter, zwlr_layer_shell_v1,
        zwlr_layer_surface_v1,
    },
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

pub struct OutputInfo {
    pub name: Option<String>,
    pub desc: Option<String>,
    pub scale_factor: Scale,

    pub output: ObjectId,
    pub output_name: u32,
}

impl OutputInfo {
    pub fn new(
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        registry: ObjectId,
        output_name: u32,
    ) -> Self {
        let output = objman.create(WaylandObject::Output);
        crate::wayland::wl_registry::req::bind(
            backend,
            registry,
            output_name,
            output,
            "wl_output",
            4,
        )
        .unwrap();
        Self {
            name: None,
            desc: None,
            scale_factor: Scale::Output(NonZeroI32::new(1).unwrap()),
            output,
            output_name,
        }
    }
}

pub struct Wallpaper {
    name: Option<String>,
    desc: Option<String>,

    output: ObjectId,
    output_name: u32,
    wl_surface: ObjectId,
    wp_viewport: ObjectId,
    wp_fractional: Option<ObjectId>,
    layer_surface: ObjectId,

    width: NonZeroI32,
    height: NonZeroI32,
    scale_factor: Scale,

    ack_serial: u32,
    needs_ack: bool,

    pub configured: bool,
    dirty: bool,

    frame_callback_handler: FrameCallbackHandler,
    img: BgImg,
    pool: BumpPool,
}

impl Wallpaper {
    pub fn new(daemon: &mut crate::Daemon, output_info: OutputInfo) -> Self {
        let crate::Daemon {
            objman,
            backend,
            pixel_format,
            compositor,
            shm,
            viewporter,
            fractional_scale_manager,
            layer_shell,
            layer,
            ..
        } = daemon;

        let OutputInfo {
            name,
            desc,
            scale_factor,
            output,
            output_name,
        } = output_info;

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
            *layer,
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

        let frame_callback_handler = FrameCallbackHandler::new(backend, objman, wl_surface);
        // commit so that the compositor send the initial configuration
        wl_surface::req::commit(backend, wl_surface).unwrap();

        let pool = BumpPool::new(backend, objman, *shm, 256, 256, *pixel_format);

        debug!("New wallpaper at output: {output_name}");
        Self {
            name,
            desc,
            output,
            output_name,
            wl_surface,
            wp_viewport,
            wp_fractional,
            layer_surface,
            width: NonZeroI32::new(4).unwrap(),
            height: NonZeroI32::new(4).unwrap(),
            scale_factor,
            ack_serial: 0,
            needs_ack: false,
            configured: false,
            dirty: false,
            frame_callback_handler,
            img: BgImg::Color([0, 0, 0, 0]),
            pool,
        }
    }

    pub fn get_bg_info(&self, pixel_format: PixelFormat) -> BgInfo {
        BgInfo {
            name: self.name.clone().unwrap_or("?".to_string()),
            dim: (self.width.get() as u32, self.height.get() as u32),
            scale_factor: self.scale_factor,
            img: match &self.img {
                BgImg::Color(color) => {
                    let mut color = *color;
                    if pixel_format.must_swap_r_and_b_channels() {
                        color.swap(0, 2);
                    }
                    BgImg::Color(color)
                }
                BgImg::Img(img) => BgImg::Img(img.clone()),
            },
            pixel_format,
        }
    }

    pub fn set_name(&mut self, name: String) {
        debug!("Output {} name: {name}", self.output_name);
        self.name = Some(name);
    }

    pub fn set_desc(&mut self, desc: String) {
        debug!("Output {} description: {desc}", self.output_name);
        self.desc = Some(desc);
    }

    pub fn set_dimensions(&mut self, width: i32, height: i32) {
        let width = match NonZeroI32::new(width) {
            Some(width) => width,
            None => {
                error!("Cannot set wallpaper width to 0!");
                return;
            }
        };

        let height = match NonZeroI32::new(height) {
            Some(height) => height,
            None => {
                error!("Cannot set wallpaper height to 0!");
                return;
            }
        };

        if (self.width, self.height) != (width, height) {
            debug!(
                "Output {} new dimensions: {width}x{height}",
                self.output_name
            );

            self.width = width;
            self.height = height;
            self.dirty = true;
        }
    }

    pub fn set_ack_serial(&mut self, serial: u32) {
        self.ack_serial = serial;
        self.needs_ack = true;
    }

    pub fn set_scale(&mut self, scale: Scale) {
        if self.scale_factor == scale || scale.priority() < self.scale_factor.priority() {
            return;
        }

        debug!("Output {} new scale: {scale}", self.output_name);
        self.scale_factor = scale;
        self.dirty = true;
    }

    pub fn commit_surface_changes(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        namespace: &str,
        use_cache: bool,
    ) -> bool {
        if self.needs_ack {
            crate::wayland::zwlr_layer_surface_v1::req::ack_configure(
                backend,
                self.layer_surface,
                self.ack_serial,
            )
            .unwrap();
            self.needs_ack = false;
        }

        if !self.dirty {
            return false;
        }
        self.dirty = false;

        if (!self.configured && use_cache) || self.img.is_set() {
            let name = self.name.clone().unwrap_or("".to_string());
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

        let (width, height) = (self.width.get(), self.height.get());
        log::debug!(
            "Output {} new configuration: width: {width}, height: {height}, scale_factor: {}",
            self.output_name,
            self.scale_factor
        );

        wp_viewport::req::set_destination(backend, self.wp_viewport, width, height).unwrap();

        let (w, h) = self.scale_factor.mul_dim(width, height);
        self.pool.resize(backend, w, h);

        self.frame_callback_handler
            .request_frame_callback(backend, objman, self.wl_surface);

        wl_surface::req::commit(backend, self.wl_surface).unwrap();
        self.configured = true;
        true
    }

    pub fn has_name(&self, name: &str) -> bool {
        self.name.as_ref().is_some_and(|s| s.eq(name))
    }

    pub fn has_output(&self, output: ObjectId) -> bool {
        self.output == output
    }

    pub fn has_output_name(&self, name: u32) -> bool {
        self.output_name == name
    }

    pub fn has_layer_surface(&self, layer_surface: ObjectId) -> bool {
        self.layer_surface == layer_surface
    }

    pub fn try_set_buffer_release_flag(
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

    pub fn has_callback(&self, callback: ObjectId) -> bool {
        self.frame_callback_handler.callback == callback
    }

    pub fn has_fractional_scale(&self, fractional_scale: ObjectId) -> bool {
        self.wp_fractional.is_some_and(|f| f == fractional_scale)
    }

    pub fn get_dimensions(&self) -> (u32, u32) {
        let dim = self
            .scale_factor
            .mul_dim(self.width.get(), self.height.get());
        (dim.0 as u32, dim.1 as u32)
    }

    pub fn canvas_change<F, T>(
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

    pub fn frame_callback_completed(&mut self) {
        self.frame_callback_handler.done = true;
    }

    pub fn clear(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        color: [u8; 4],
    ) {
        let channels = pixel_format.channels() as usize;
        self.canvas_change(backend, objman, pixel_format, |canvas| {
            for pixel in canvas.chunks_exact_mut(channels) {
                pixel[0..channels].copy_from_slice(&color[0..channels]);
            }
        })
    }

    pub fn set_img_info(&mut self, img_info: BgImg) {
        debug!("output {:?} - drawing: {}", self.name, img_info);
        self.img = img_info;
    }

    pub fn destroy(&mut self, backend: &mut Waybackend) {
        // Careful not to panic here, since we call this on drop

        if let Err(e) = wp_viewport::req::destroy(backend, self.wp_viewport) {
            error!("error destroying wp_viewport: {e:?}");
        }

        if let Some(fractional) = self.wp_fractional
            && let Err(e) = wp_fractional_scale_v1::req::destroy(backend, fractional)
        {
            error!("error destroying wp_fractional_scale_v1: {e:?}");
        }

        if let Err(e) = zwlr_layer_surface_v1::req::destroy(backend, self.layer_surface) {
            error!("error destroying zwlr_layer_surface_v1: {e:?}");
        }

        self.pool.destroy(backend);

        debug!(
            "Destroyed output {} - {}",
            self.name.as_ref().unwrap_or(&"?".to_string()),
            self.desc.as_ref().unwrap_or(&"?".to_string())
        );
    }

    fn attach_buffer_and_damage_surface(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
    ) {
        let surface = self.wl_surface;
        let buf = self.pool.get_committable_buffer();
        let (width, height) = (self.pool.width(), self.pool.height());

        wl_surface::req::attach(backend, surface, Some(buf), 0, 0).unwrap();
        wl_surface::req::damage_buffer(backend, surface, 0, 0, width, height).unwrap();
        self.frame_callback_handler
            .request_frame_callback(backend, objman, surface);
    }
}

/// attaches all pending buffers and damages all surfaces with one single request
pub fn attach_buffers_and_damage_surfaces(
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
pub fn commit_wallpapers(backend: &mut Waybackend, wallpapers: &[Rc<RefCell<Wallpaper>>]) {
    for wallpaper in wallpapers {
        let wallpaper = wallpaper.borrow();
        wl_surface::req::commit(backend, wallpaper.wl_surface).unwrap();
    }
}
