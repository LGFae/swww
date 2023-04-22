use log::error;
use utils::communication::BgImg;

use std::{num::NonZeroI32, thread::ThreadId};

use smithay_client_toolkit::{
    output::OutputInfo,
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, LayerSurface},
        WaylandSurface,
    },
    shm::slot::{Slot, SlotPool},
};

use wayland_client::protocol::wl_shm;

#[derive(PartialEq)]
pub struct OutputId(pub u32);

/// Owns all the necessary information for drawing. In order to get the current image, use `buf_arc_clone`
pub struct Wallpaper {
    pub output_id: OutputId,
    pub width: NonZeroI32,
    pub height: NonZeroI32,
    pub scale_factor: NonZeroI32,

    pub slot: Slot,
    pub img: BgImg,

    pub layer_surface: LayerSurface,

    /// We change the owner in two instances: first, when sending things to the processor threads.
    /// Second, when we clear the wallpaper
    ///
    /// The processor threads use this to figure out when they should no longer draw to that
    /// wallpaper
    owner: ThreadId,
    pub in_transition: bool,
}

impl Wallpaper {
    pub fn new(output_info: OutputInfo, layer_surface: LayerSurface, pool: &mut SlotPool) -> Self {
        let (width, height) = if let Some(output_size) = output_info.logical_size {
            (
                NonZeroI32::new(output_size.0).unwrap(),
                NonZeroI32::new(output_size.1).unwrap(),
            )
        } else {
            (256.try_into().unwrap(), 256.try_into().unwrap())
        };

        let scale_factor = NonZeroI32::new(output_info.scale_factor).unwrap();
        let slot = pool
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
            output_id: OutputId(output_info.id),
            width,
            height,
            scale_factor,
            layer_surface,
            slot,
            img: BgImg::Color([0, 0, 0]),
            owner: std::thread::current().id(),
            in_transition: false,
        }
    }

    pub fn clear(&mut self, pool: &mut SlotPool, color: [u8; 3]) {
        match self.slot.canvas(pool) {
            Some(canvas) => {
                for pixel in canvas.chunks_exact_mut(4) {
                    pixel[2] = color[0];
                    pixel[1] = color[1];
                    pixel[0] = color[2];
                }
            }
            None => {
                error!("failed to get slot canvas");
                return;
            }
        }

        self.chown();
    }

    pub fn set_img(&mut self, pool: &mut SlotPool, img: &[u8]) {
        match self.slot.canvas(pool) {
            Some(canvas) => canvas.copy_from_slice(img),
            None => error!("failed to get slot canvas"),
        }
    }

    pub fn draw(&mut self, pool: &mut SlotPool) {
        log::debug!("drawing: {}", self.img);

        let width = self.width.get() * self.scale_factor.get();
        let height = self.height.get() * self.scale_factor.get();
        let stride = width * 4;

        let buf = pool
            .create_buffer_in(&self.slot, width, height, stride, wl_shm::Format::Argb8888)
            .unwrap();
        let surface = self.layer_surface.wl_surface();
        surface.damage_buffer(0, 0, width, height);
        buf.attach_to(surface).unwrap();
        self.layer_surface.commit();
    }

    pub fn resize(
        &mut self,
        pool: &mut SlotPool,
        width: NonZeroI32,
        height: NonZeroI32,
        scale_factor: NonZeroI32,
    ) {
        self.width = width;
        self.height = height;
        self.scale_factor = scale_factor;
        self.slot = pool
            .new_slot(
                width.get() as usize * height.get() as usize * scale_factor.get() as usize * 4,
            )
            .expect("failed to create slot");
        self.img = BgImg::Color([0, 0, 0]);
    }

    /// sets `in_transition` to false
    pub fn chown(&mut self) {
        self.owner = std::thread::current().id();
        self.in_transition = false;
    }

    pub fn is_owned_by(&self, thread_id: ThreadId) -> bool {
        self.owner == thread_id
    }
}
