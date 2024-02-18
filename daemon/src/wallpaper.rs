use utils::ipc::BgImg;

use std::{
    num::NonZeroI32,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
    },
};

use smithay_client_toolkit::{
    output::OutputInfo,
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, LayerSurface},
        WaylandSurface,
    },
    shm,
};

use wayland_client::protocol::{wl_buffer::WlBuffer, wl_shm, wl_surface::WlSurface};

/// The memory pool wallpapers use
pub type ShmPool = shm::multi::MultiPool<(WlSurface, u32)>;
/// The memory pool, multithreaded
pub type MtShmPool = Arc<Mutex<ShmPool>>;

#[derive(Debug)]
struct AnimationState {
    id: AtomicUsize,
    transition_finished: Arc<AtomicBool>,
}

#[derive(Debug)]
pub struct AnimationToken {
    id: usize,
    transition_done: Arc<AtomicBool>,
}

impl AnimationToken {
    pub fn is_transition_done(&self) -> bool {
        self.transition_done.load(Ordering::Acquire)
    }

    pub fn set_transition_done(&self, wallpaper: &Wallpaper) {
        if wallpaper.has_animation_id(self) {
            self.transition_done.store(true, Ordering::Release);
        }
    }
}

/// Owns all the necessary information for drawing.
#[derive(Debug)]
struct WallpaperInner {
    width: NonZeroI32,
    height: NonZeroI32,
    scale_factor: NonZeroI32,

    img: BgImg,
}

pub struct Wallpaper {
    output_id: u32,
    inner: RwLock<WallpaperInner>,
    layer_surface: LayerSurface,

    animation_state: AnimationState,
    pool: MtShmPool,
    pub configured: AtomicBool,
}

impl Wallpaper {
    pub fn new(output_info: OutputInfo, layer_surface: LayerSurface, pool: MtShmPool) -> Self {
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

        Self {
            output_id: output_info.id,
            layer_surface,
            pool,
            inner: RwLock::new(WallpaperInner {
                width,
                height,
                scale_factor,
                img: BgImg::Color([0, 0, 0]),
            }),
            animation_state: AnimationState {
                id: AtomicUsize::new(0),
                transition_finished: Arc::new(AtomicBool::new(false)),
            },
            configured: AtomicBool::new(false),
        }
    }

    #[inline]
    pub fn has_id(&self, id: u32) -> bool {
        self.output_id == id
    }

    #[inline]
    pub fn has_animation_id(&self, token: &AnimationToken) -> bool {
        self.animation_state
            .id
            .load(std::sync::atomic::Ordering::Acquire)
            == token.id
    }

    #[inline]
    pub fn has_surface(&self, surface: &WlSurface) -> bool {
        self.layer_surface.wl_surface() == surface
    }

    pub fn get_dimensions(&self) -> (u32, u32) {
        let inner = self.lock_inner();
        let width = inner.width.get() as u32;
        let height = inner.height.get() as u32;
        let scale_factor = inner.scale_factor.get() as u32;
        (width * scale_factor, height * scale_factor)
    }

    #[inline]
    fn lock(&self) -> (RwLockReadGuard<WallpaperInner>, MutexGuard<ShmPool>) {
        (self.lock_inner(), self.pool.lock().unwrap())
    }

    #[inline]
    fn lock_mut(&self) -> (RwLockWriteGuard<WallpaperInner>, MutexGuard<ShmPool>) {
        (self.lock_inner_mut(), self.pool.lock().unwrap())
    }

    #[inline]
    fn lock_inner(&self) -> RwLockReadGuard<WallpaperInner> {
        self.inner.read().unwrap()
    }

    #[inline]
    fn lock_inner_mut(&self) -> RwLockWriteGuard<WallpaperInner> {
        self.inner.write().unwrap()
    }

    pub fn canvas_change<F, T>(&self, f: F) -> (T, WlBuffer)
    where
        F: FnOnce(&mut [u8]) -> T,
    {
        let (inner, mut pool) = self.lock();
        let width = inner.width.get() * inner.scale_factor.get();
        let stride = width * 4;
        let height = inner.height.get() * inner.scale_factor.get();
        drop(inner);
        let mut frame = 0u32;
        loop {
            match pool.create_buffer(
                width,
                stride,
                height,
                &(self.layer_surface.wl_surface().clone(), frame),
                wl_shm::Format::Xrgb8888,
            ) {
                Ok((_offset, buffer, canvas)) => return (f(canvas), buffer.clone()),
                Err(e) => match e {
                    smithay_client_toolkit::shm::multi::PoolError::InUse => frame += 1,
                    smithay_client_toolkit::shm::multi::PoolError::Overlap => {
                        pool.remove(&(self.layer_surface.wl_surface().clone(), frame));
                    }
                    smithay_client_toolkit::shm::multi::PoolError::NotFound => unreachable!(),
                },
            }
        }
    }

    #[inline]
    pub fn get_img_info(&self) -> BgImg {
        self.lock_inner().img.clone()
    }

    #[inline]
    pub fn create_animation_token(&self) -> AnimationToken {
        let id = self.animation_state.id.load(Ordering::Acquire);
        AnimationToken {
            id,
            transition_done: Arc::clone(&self.animation_state.transition_finished),
        }
    }

    /// Stops all animations with the current id, by increasing that id
    #[inline]
    pub fn stop_animations(&self) {
        self.animation_state.id.fetch_add(1, Ordering::AcqRel);
        self.animation_state
            .transition_finished
            .store(false, Ordering::Release);
    }

    pub fn clear(&self, color: [u8; 3]) -> WlBuffer {
        self.canvas_change(|canvas| {
            for pixel in canvas.chunks_exact_mut(4) {
                pixel[2] = color[0];
                pixel[1] = color[1];
                pixel[0] = color[2];
            }
        })
        .1
    }

    pub fn set_img_info(&self, img_info: BgImg) {
        log::debug!("output {} - drawing: {}", self.output_id, img_info);
        self.lock_inner_mut().img = img_info;
    }

    pub fn draw(&self, buf: &WlBuffer) {
        let inner = self.lock_inner();
        let width = inner.width.get() * inner.scale_factor.get();
        let height = inner.height.get() * inner.scale_factor.get();
        drop(inner);

        let surface = self.layer_surface.wl_surface();
        surface.attach(Some(buf), 0, 0);
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
        let (mut inner, mut pool) = self.lock_mut();
        let width = width.unwrap_or(inner.width);
        let height = height.unwrap_or(inner.height);
        let scale_factor = scale_factor.unwrap_or(inner.scale_factor);
        if (width, height, scale_factor) == (inner.width, inner.height, inner.scale_factor) {
            return;
        }
        self.stop_animations();

        // remove all buffers with the previous size
        let mut frame = 0u32;
        while pool
            .remove(&(self.layer_surface.wl_surface().clone(), frame))
            .is_some()
        {
            frame += 1;
        }
        drop(pool);

        inner.width = width;
        inner.height = height;
        inner.scale_factor = scale_factor;

        self.layer_surface
            .set_size(inner.width.get() as u32, inner.height.get() as u32);
        inner.img = BgImg::Color([0, 0, 0]);
        drop(inner);
        self.layer_surface.commit();
        self.configured.store(false, Ordering::Release);
    }
}
