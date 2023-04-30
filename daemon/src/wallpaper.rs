use utils::ipc::BgImg;

use std::{
    num::NonZeroI32,
    sync::{Condvar, Mutex, MutexGuard},
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

pub enum AnimationState {
    Animating,
    ShouldStop,
    Idle,
}

pub struct LockedPool<'a>(MutexGuard<'a, SlotPool>);

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
}

impl Wallpaper {
    pub fn new(
        output_info: OutputInfo,
        layer_surface: LayerSurface,
        pool: &Mutex<SlotPool>,
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
                width.get() as usize * height.get() as usize * scale_factor.get() as usize * 4,
            )
            .expect("failed to create slot in pool");

        // Configure the layer surface
        layer_surface.set_anchor(Anchor::all());
        layer_surface.set_margin(0, 0, 0, 0);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.set_size(
            width.get() as u32 * scale_factor.get() as u32,
            height.get() as u32 * scale_factor.get() as u32,
        );
        // commit so that the compositor send the initial configuration
        layer_surface.commit();

        Self {
            output_id: output_info.id,
            layer_surface,
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
        let lock = self.inner.lock().unwrap();
        let width = lock.width.get() as u32;
        let height = lock.height.get() as u32;
        let scale_factor = lock.scale_factor.get() as u32;
        (width * scale_factor, height * scale_factor)
    }

    pub fn lock_pool_to_get_canvas<'a>(&self, pool: &'a Mutex<SlotPool>) -> LockedPool<'a> {
        let mut lock = self.inner.lock().unwrap();
        while lock.slot.has_active_buffers() {
            lock = self.condvar.wait(lock).unwrap();
        }
        LockedPool(pool.lock().unwrap())
    }

    pub fn get_canvas<'a>(&'a self, pool: &'a mut LockedPool<'_>) -> &'a mut [u8] {
        let lock = self.inner.lock().unwrap();
        lock.slot.canvas(&mut pool.0).unwrap()
    }

    pub fn get_img_info(&self) -> BgImg {
        self.inner.lock().unwrap().img.clone()
    }

    pub fn begin_animation(&self) {
        let mut lock = self.inner.lock().unwrap();
        while !matches!(lock.animation_state, AnimationState::Idle) {
            lock = self.condvar.wait(lock).unwrap();
        }
        lock.animation_state = AnimationState::Animating;
    }

    pub fn set_end_animation_flag(&self) {
        let mut lock = self.inner.lock().unwrap();
        if !matches!(lock.animation_state, AnimationState::Idle) {
            lock.animation_state = AnimationState::ShouldStop;
        }
    }

    pub fn animation_should_stop(&self) -> bool {
        let lock = self.inner.lock().unwrap();
        matches!(lock.animation_state, AnimationState::ShouldStop)
    }

    pub fn end_animation(&self) {
        let mut lock = self.inner.lock().unwrap();
        lock.animation_state = AnimationState::Idle;
        self.condvar.notify_all();
    }

    pub fn wait_for_animation(&self) {
        let mut lock = self.inner.lock().unwrap();
        while !matches!(lock.animation_state, AnimationState::Idle) {
            lock = self.condvar.wait(lock).unwrap();
        }
    }

    pub fn clear(&self, pool: &Mutex<SlotPool>, color: [u8; 3]) {
        let mut pool = self.lock_pool_to_get_canvas(pool);
        for pixel in self.get_canvas(&mut pool).chunks_exact_mut(4) {
            pixel[2] = color[0];
            pixel[1] = color[1];
            pixel[0] = color[2];
        }
    }

    pub fn set_img_info(&self, img_info: BgImg) {
        self.inner.lock().unwrap().img = img_info;
    }

    pub fn notify_condvar(&self) {
        self.condvar.notify_all()
    }

    pub fn draw(&self, pool: &mut LockedPool<'_>) {
        let lock = self.inner.lock().unwrap();
        log::debug!("drawing: {}", lock.img);

        let width = lock.width.get() * lock.scale_factor.get();
        let height = lock.height.get() * lock.scale_factor.get();
        let stride = width * 4;

        let buf = pool
            .0
            .create_buffer_in(&lock.slot, width, height, stride, wl_shm::Format::Xrgb8888)
            .unwrap();
        drop(lock);
        let surface = self.layer_surface.wl_surface();
        buf.attach_to(surface).unwrap();
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
    }

    pub fn resize(
        &self,
        pool: &mut LockedPool<'_>,
        width: Option<NonZeroI32>,
        height: Option<NonZeroI32>,
        scale_factor: Option<NonZeroI32>,
    ) {
        let mut lock = self.inner.lock().unwrap();
        let width = width.unwrap_or(lock.width);
        let height = height.unwrap_or(lock.width);
        let scale_factor = scale_factor.unwrap_or(lock.scale_factor);
        if (width, height, scale_factor) == (lock.width, lock.height, lock.scale_factor) {
            return;
        }
        lock.animation_state = AnimationState::ShouldStop;
        lock.width = width;
        lock.height = height;
        lock.scale_factor = scale_factor;
        lock.slot = pool
            .0
            .new_slot(
                lock.width.get() as usize
                    * lock.height.get() as usize
                    * lock.scale_factor.get() as usize
                    * 4,
            )
            .expect("failed to create slot");
        self.layer_surface.set_size(
            lock.width.get() as u32 * lock.scale_factor.get() as u32,
            lock.height.get() as u32 * lock.scale_factor.get() as u32,
        );
        lock.img = BgImg::Color([0, 0, 0]);
        self.layer_surface.commit();
    }
}
