use std::num::NonZeroI32;
use std::num::NonZeroU32;
use std::sync::Arc;

use super::Daemon;

use common::ipc::Scale;

use log::debug;
use log::error;
use log::warn;

use crate::wallpaper::Wallpaper;
use crate::wayland;
use crate::wayland::globals;
use crate::wayland::interfaces::*;
use crate::wayland::wire::WaylandPayload;
use crate::wayland::wire::WireMsg;
use crate::wayland::ObjectId;
use crate::wayland::WlDynObj;

impl Daemon {
    pub(super) fn new_output(&mut self, name: u32) {
        let output = globals::object_create(wayland::WlDynObj::Output);
        wl_registry::req::bind(name, output, "wl_output", 4).unwrap();

        let surface = globals::object_create(wayland::WlDynObj::Surface);
        wl_compositor::req::create_surface(surface).unwrap();

        let region = globals::object_create(wayland::WlDynObj::Region);
        wl_compositor::req::create_region(region).unwrap();

        wl_surface::req::set_input_region(surface, Some(region)).unwrap();
        wl_region::req::destroy(region).unwrap();

        let layer_surface = globals::object_create(wayland::WlDynObj::LayerSurface);
        zwlr_layer_shell_v1::req::get_layer_surface(
            layer_surface,
            surface,
            Some(output),
            zwlr_layer_shell_v1::layer::BACKGROUND,
            "swww-daemon",
        )
        .unwrap();

        let viewport = globals::object_create(wayland::WlDynObj::Viewport);
        wp_viewporter::req::get_viewport(viewport, surface).unwrap();

        let wp_fractional = if let Some((id, _)) = self.fractional_scale_manager.as_ref() {
            let fractional = globals::object_create(wayland::WlDynObj::FractionalScale);
            wp_fractional_scale_manager_v1::req::get_fractional_scale(*id, fractional, surface)
                .unwrap();
            Some(fractional)
        } else {
            None
        };

        debug!("New output: {name}");
        self.wallpapers.push(Arc::new(Wallpaper::new(
            output,
            name,
            surface,
            viewport,
            wp_fractional,
            layer_surface,
        )));
    }

    pub(super) fn wayland_handler(&mut self, msg: WireMsg, payload: WaylandPayload) {
        match msg.sender_id() {
            globals::WL_DISPLAY => wl_display::event(self, msg, payload),
            globals::WL_REGISTRY => wl_registry::event(self, msg, payload),
            globals::WL_COMPOSITOR => error!("wl_compositor has no events"),
            globals::WL_SHM => wl_shm::event(self, msg, payload),
            globals::WP_VIEWPORTER => error!("wp_viewporter has no events"),
            globals::ZWLR_LAYER_SHELL_V1 => error!("zwlr_layer_shell_v1 has no events"),
            other => match globals::object_type_get(other) {
                Some(obj) => self.wayland_dyn_handler(obj, msg, payload),
                None => error!("Received event for deleted object ({other:?})"),
            },
        }
    }

    fn wayland_dyn_handler(&mut self, obj: WlDynObj, msg: WireMsg, payload: WaylandPayload) {
        match obj {
            WlDynObj::Output => wl_output::event(self, msg, payload),
            WlDynObj::Surface => wl_surface::event(self, msg, payload),
            WlDynObj::Region => error!("wl_region has no events"),
            WlDynObj::LayerSurface => zwlr_layer_surface_v1::event(self, msg, payload),
            WlDynObj::Buffer => wl_buffer::event(self, msg, payload),
            WlDynObj::ShmPool => error!("wl_shm_pool has no events"),
            WlDynObj::Callback => wl_callback::event(self, msg, payload),
            WlDynObj::Viewport => error!("wp_viewport has no events"),
            WlDynObj::FractionalScale => wp_fractional_scale_v1::event(self, msg, payload),
        }
    }
}

impl wl_display::EvHandler for Daemon {
    fn delete_id(&mut self, id: u32) {
        if let Some(id) = NonZeroU32::new(id) {
            globals::object_remove(ObjectId::new(id));
        }
    }
}

impl wl_registry::EvHandler for Daemon {
    fn global(&mut self, name: u32, interface: &str, version: u32) {
        if interface == "wl_output" {
            if version < 4 {
                error!("your compositor must support at least version 4 of wl_output");
            } else {
                self.new_output(name);
            }
        }
    }

    fn global_remove(&mut self, name: u32) {
        self.wallpapers.retain(|w| !w.has_output_name(name));
    }
}

impl wl_shm::EvHandler for Daemon {
    fn format(&mut self, format: u32) {
        warn!(
            "received a wl_shm format after initialization: {format}. This shouldn't be possible"
        );
    }
}

impl wl_output::EvHandler for Daemon {
    fn geometry(
        &mut self,
        sender_id: ObjectId,
        _x: i32,
        _y: i32,
        _physical_width: i32,
        _physical_height: i32,
        _subpixel: i32,
        _make: &str,
        _model: &str,
        transform: i32,
    ) {
        for wallpaper in self.wallpapers.iter() {
            if wallpaper.has_output(sender_id) {
                if transform as u32 > wayland::interfaces::wl_output::transform::FLIPPED_270 {
                    error!("received invalid transform value from compositor: {transform}")
                } else {
                    wallpaper.set_transform(transform as u32);
                }
                break;
            }
        }
    }

    fn mode(&mut self, sender_id: ObjectId, _flags: u32, width: i32, height: i32, _refresh: i32) {
        for wallpaper in self.wallpapers.iter() {
            if wallpaper.has_output(sender_id) {
                wallpaper.set_dimensions(width, height);
                break;
            }
        }
    }

    fn done(&mut self, sender_id: ObjectId) {
        for wallpaper in self.wallpapers.iter() {
            if wallpaper.has_output(sender_id) {
                wallpaper.commit_surface_changes(self.cache);
                break;
            }
        }
    }

    fn scale(&mut self, sender_id: ObjectId, factor: i32) {
        for wallpaper in self.wallpapers.iter() {
            if wallpaper.has_output(sender_id) {
                match NonZeroI32::new(factor) {
                    Some(factor) => wallpaper.set_scale(Scale::Whole(factor)),
                    None => error!("received scale factor of 0 from compositor"),
                }
                break;
            }
        }
    }

    fn name(&mut self, sender_id: ObjectId, name: &str) {
        for wallpaper in self.wallpapers.iter() {
            if wallpaper.has_output(sender_id) {
                wallpaper.set_name(name.to_string());
                break;
            }
        }
    }

    fn description(&mut self, sender_id: ObjectId, description: &str) {
        for wallpaper in self.wallpapers.iter() {
            if wallpaper.has_output(sender_id) {
                wallpaper.set_desc(description.to_string());
                break;
            }
        }
    }
}

impl wl_surface::EvHandler for Daemon {
    fn enter(&mut self, _sender_id: ObjectId, output: ObjectId) {
        debug!("Output {}: Surface Enter", output.get());
    }

    fn leave(&mut self, _sender_id: ObjectId, output: ObjectId) {
        debug!("Output {}: Surface Leave", output.get());
    }

    fn preferred_buffer_scale(&mut self, sender_id: ObjectId, factor: i32) {
        for wallpaper in self.wallpapers.iter() {
            if wallpaper.has_surface(sender_id) {
                match NonZeroI32::new(factor) {
                    Some(factor) => wallpaper.set_scale(Scale::Whole(factor)),
                    None => error!("received scale factor of 0 from compositor"),
                }
                break;
            }
        }
    }

    fn preferred_buffer_transform(&mut self, _sender_id: ObjectId, _transform: u32) {
        warn!("Received PreferredBufferTransform. We currently ignore those")
    }
}

impl wl_buffer::EvHandler for Daemon {
    fn release(&mut self, sender_id: ObjectId) {
        for wallpaper in self.wallpapers.iter() {
            let strong_count = Arc::strong_count(wallpaper);
            if wallpaper.try_set_buffer_release_flag(sender_id, strong_count) {
                break;
            }
        }
    }
}

impl wl_callback::EvHandler for Daemon {
    fn done(&mut self, sender_id: ObjectId, _callback_data: u32) {
        for wallpaper in self.wallpapers.iter() {
            if wallpaper.has_callback(sender_id) {
                wallpaper.frame_callback_completed();
                break;
            }
        }
    }
}

impl zwlr_layer_surface_v1::EvHandler for Daemon {
    fn configure(&mut self, sender_id: ObjectId, serial: u32, _width: u32, _height: u32) {
        for wallpaper in self.wallpapers.iter() {
            if wallpaper.has_layer_surface(sender_id) {
                wayland::interfaces::zwlr_layer_surface_v1::req::ack_configure(sender_id, serial)
                    .unwrap();
                break;
            }
        }
    }

    fn closed(&mut self, sender_id: ObjectId) {
        self.wallpapers.retain(|w| !w.has_layer_surface(sender_id));
    }
}

impl wp_fractional_scale_v1::EvHandler for Daemon {
    fn preferred_scale(&mut self, sender_id: ObjectId, scale: u32) {
        for wallpaper in self.wallpapers.iter() {
            if wallpaper.has_fractional_scale(sender_id) {
                match NonZeroI32::new(scale as i32) {
                    Some(factor) => {
                        wallpaper.set_scale(Scale::Fractional(factor));
                        wallpaper.commit_surface_changes(self.cache);
                    }
                    None => error!("received scale factor of 0 from compositor"),
                }
                break;
            }
        }
    }
}
