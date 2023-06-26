use utils::ipc::BgImg;

use std::{
    num::NonZeroI32,
    sync::{Arc, Condvar, Mutex, MutexGuard},
};

use smithay_client_toolkit::{
    output::OutputInfo,
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, LayerSurface},
        WaylandSurface,
    },
    shm::slot::{Slot, SlotPool},
};

use wayland_client::protocol::{wl_shm, wl_surface::WlSurface};

#[derive(Debug)]
pub enum AnimationState {
    Animating,
    ShouldStop,
    Idle,
}

/// Owns all the necessary information for drawing.
struct WallpaperInner {
    width: NonZeroI32,
    height: NonZeroI32,
    scale_factor: NonZeroI32,

    slot: Slot,
    img: BgImg,

    animation_state: AnimationState,
}

pub struct Wallpaper {
    output_id: u32,
    inner: Mutex<WallpaperInner>,
    layer_surface: LayerSurface,
    condvar: Condvar,
    pool: Arc<Mutex<SlotPool>>,
}

impl Wallpaper {
    pub fn new(
        output_info: OutputInfo,
        layer_surface: LayerSurface,
        pool: Arc<Mutex<SlotPool>>,
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
        let slot = pool
            .lock()
            .unwrap()
            .new_slot(
                width.get() as usize
                    * height.get() as usize
                    * scale_factor.get() as usize
                    * scale_factor.get() as usize
                    * 4,
            )
            .expect("failed to create slot in pool");

        // Configure the layer surface
        layer_surface.set_anchor(Anchor::all());
        layer_surface.set_margin(0, 0, 0, 0);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.set_size(width.get() as u32, height.get() as u32);
        layer_surface
            .set_buffer_scale(scale_factor.get() as u32)
            .unwrap();
        // commit so that the compositor send the initial configuration
        layer_surface.commit();

        Self {
            output_id: output_info.id,
            layer_surface,
            pool,
            inner: Mutex::new(WallpaperInner {
                width,
                height,
                scale_factor,
                slot,
                img: BgImg::Color([0, 0, 0]),
                animation_state: AnimationState::Idle,
            }),
            condvar: Condvar::new(),
        }
    }

    pub fn has_id(&self, id: u32) -> bool {
        self.output_id == id
    }

    pub fn has_surface(&self, surface: &WlSurface) -> bool {
        self.layer_surface.wl_surface() == surface
    }

    pub fn get_dimensions(&self) -> (u32, u32) {
        let (inner, _) = self.lock();
        let width = inner.width.get() as u32;
        let height = inner.height.get() as u32;
        let scale_factor = inner.scale_factor.get() as u32;
        (width * scale_factor, height * scale_factor)
    }

    #[inline]
    fn lock(&self) -> (MutexGuard<WallpaperInner>, MutexGuard<SlotPool>) {
        (self.inner.lock().unwrap(), self.pool.lock().unwrap())
    }

    pub fn canvas_change<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut [u8]) -> T,
    {
        loop {
            let (inner, mut pool) = self.lock();
            if let Some(canvas) = inner.slot.canvas(&mut pool) {
                log::debug!("got canvas! - output {}", self.output_id);
                return f(canvas);
            }
            log::debug!("failed to get canvas - output {}", self.output_id);
            // sleep to mitigate busy waiting
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    pub fn get_img_info(&self) -> BgImg {
        self.lock().0.img.clone()
    }

    pub fn begin_animation(&self) {
        let mut lock = self.lock().0;
        log::debug!("beginning animation for output: {}", self.output_id);
        while !matches!(lock.animation_state, AnimationState::Idle) {
            lock = self.condvar.wait(lock).unwrap();
        }
        lock.animation_state = AnimationState::Animating;
    }

    pub fn set_end_animation_flag(&self) {
        let mut lock = self.lock().0;
        if !matches!(lock.animation_state, AnimationState::Idle) {
            log::debug!("setting end animation flags for output: {}", self.output_id,);
            lock.animation_state = AnimationState::ShouldStop;
        }
    }

    pub fn animation_should_stop(&self) -> bool {
        let lock = self.lock().0;
        matches!(lock.animation_state, AnimationState::ShouldStop)
    }

    pub fn end_animation(&self) {
        let mut lock = self.lock().0;
        log::debug!("ending animation for output: {}", self.output_id);
        lock.animation_state = AnimationState::Idle;
        self.condvar.notify_all();
    }

    pub fn wait_for_animation(&self) {
        let mut lock = self.lock().0;
        log::debug!("wait for output {} animation to finish...", self.output_id);
        while !matches!(lock.animation_state, AnimationState::Idle) {
            lock = self.condvar.wait(lock).unwrap();
        }
        log::debug!("output {} animation to finished!", self.output_id);
    }

    pub fn clear(&self, color: [u8; 3]) {
        self.canvas_change(|canvas| {
            for pixel in canvas.chunks_exact_mut(4) {
                pixel[2] = color[0];
                pixel[1] = color[1];
                pixel[0] = color[2];
            }
        });
    }

    pub fn set_img_info(&self, img_info: BgImg) {
        log::debug!("output {} - drawing: {}", self.output_id, img_info);
        self.lock().0.img = img_info;
    }

    pub fn notify_condvar(&self) {
        self.condvar.notify_all()
    }

    pub fn draw(&self) {
        let (inner, mut pool) = self.lock();

        let width = inner.width.get() * inner.scale_factor.get();
        let height = inner.height.get() * inner.scale_factor.get();
        let stride = width * 4;

        let buf = pool
            .create_buffer_in(&inner.slot, width, height, stride, wl_shm::Format::Xrgb8888)
            .unwrap();
        drop(inner);
        let surface = self.layer_surface.wl_surface();
        buf.attach_to(surface).unwrap();
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
    }

    pub fn resize(
        &self,
        width: Option<NonZeroI32>,
        height: Option<NonZeroI32>,
        scale_factor: Option<NonZeroI32>,
    ) {
        if let Some(s) = scale_factor {
            self.layer_surface.set_buffer_scale(s.get() as u32).unwrap();
        }
        let (mut inner, mut pool) = self.lock();
        let width = width.unwrap_or(inner.width);
        let height = height.unwrap_or(inner.height);
        let scale_factor = scale_factor.unwrap_or(inner.scale_factor);
        if (width, height, scale_factor) == (inner.width, inner.height, inner.scale_factor) {
            return;
        }

        if matches!(inner.animation_state, AnimationState::Animating) {
            inner.animation_state = AnimationState::ShouldStop;
        }
        inner.width = width;
        inner.height = height;
        inner.scale_factor = scale_factor;
        inner.slot = pool
            .new_slot(
                inner.width.get() as usize
                    * inner.height.get() as usize
                    * inner.scale_factor.get() as usize
                    * inner.scale_factor.get() as usize
                    * 4,
            )
            .expect("failed to create slot");
        self.layer_surface
            .set_size(inner.width.get() as u32, inner.height.get() as u32);
        inner.img = BgImg::Color([0, 0, 0]);
        self.layer_surface.commit();
    }
}
