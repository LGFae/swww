use smithay_client_toolkit::{
    default_environment,
    environment::SimpleGlobal,
    new_default_environment,
    output::{with_output_info, OutputInfo},
    reexports::{
        calloop,
        client::protocol::{wl_output, wl_shm, wl_surface},
        client::{Attached, Main},
        protocols::wlr::unstable::layer_shell::v1::client::{
            zwlr_layer_shell_v1, zwlr_layer_surface_v1,
        },
    },
    shm::AutoMemPool,
    WaylandSource,
};

use std::{
    cell::{Cell, RefCell},
    path::Path,
    rc::Rc,
};

use log::{debug, error, info, warn};

use image;

default_environment!(Env,
    fields = [
        layer_shell: SimpleGlobal<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    ],
    singles = [
        zwlr_layer_shell_v1::ZwlrLayerShellV1 => layer_shell
    ],
);

#[derive(PartialEq, Copy, Clone)]
enum RenderEvent {
    Configure { width: u32, height: u32 },
    Closed,
}

struct Background {
    surface: wl_surface::WlSurface,
    layer_surface: Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    next_render_event: Rc<Cell<Option<RenderEvent>>>,
    pool: AutoMemPool,
    dimensions: (u32, u32),
    img: Vec<u8>,
}

impl Background {
    fn new(
        output: &wl_output::WlOutput,
        surface: wl_surface::WlSurface,
        layer_shell: &Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        pool: AutoMemPool,
        img_path: &Path,
    ) -> Option<Self> {
        let img = image::open(img_path).unwrap();
        let mut img = img.to_rgba8().to_vec();

        // The ARGB is 'little endian', so here we must do a slight adaptation
        // Specifically, we must put the order of bytes 'in reverse', so it needs to be
        // BGRA, which we achieve by swaping the R and B on our original vector
        for pixel in img.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        info!("Img is ready!");

        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            Some(output),
            zwlr_layer_shell_v1::Layer::Background,
            "fswww".to_owned(),
        );

        layer_surface.set_anchor(zwlr_layer_surface_v1::Anchor::all());
        layer_surface.set_exclusive_zone(-1);

        let next_render_event = Rc::new(Cell::new(None::<RenderEvent>));
        let next_render_event_handle = Rc::clone(&next_render_event);
        layer_surface.quick_assign(move |layer_surface, event, _| {
            match (event, next_render_event_handle.get()) {
                (zwlr_layer_surface_v1::Event::Closed, _) => {
                    next_render_event_handle.set(Some(RenderEvent::Closed));
                }
                (
                    zwlr_layer_surface_v1::Event::Configure {
                        serial,
                        width,
                        height,
                    },
                    next,
                ) if next != Some(RenderEvent::Closed) => {
                    layer_surface.ack_configure(serial);
                    next_render_event_handle.set(Some(RenderEvent::Configure { width, height }));
                }
                (_, _) => {}
            }
        });

        // Commit so that the server will send a configure event
        surface.commit();

        Some(Self {
            surface,
            layer_surface,
            next_render_event,
            pool,
            img,
            dimensions: (0, 0),
        })
    }

    /// Handles any events that have occurred since the last call, redrawing if needed.
    /// Returns true if the surface should be dropped.
    fn handle_events(&mut self) -> bool {
        match self.next_render_event.take() {
            Some(RenderEvent::Closed) => true,
            Some(RenderEvent::Configure { width, height }) => {
                self.dimensions = (width, height);
                self.draw();
                false
            }
            None => false,
        }
    }

    fn draw(&mut self) {
        let stride = 4 * self.dimensions.0 as i32;
        let width = self.dimensions.0 as i32;
        let height = self.dimensions.1 as i32;

        // Note: unwrap() is only used here in the interest of simplicity of the example.
        // A "real" application should handle the case where both pools are still in use by the
        // compositor.
        let (canvas, buffer) = self
            .pool
            .buffer(width, height, stride, wl_shm::Format::Argb8888)
            .unwrap();

        canvas.copy_from_slice(self.img.as_slice());
        info!("Copied bytes to canvas.");

        // Attach the buffer to the surface and mark the entire surface as damaged
        self.surface.attach(Some(&buffer), 0, 0);
        self.surface
            .damage_buffer(0, 0, width as i32, height as i32);

        // Finally, commit the surface
        self.surface.commit();
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        self.layer_surface.destroy();
        self.surface.destroy();
    }
}

fn main() {
    //If using a debug build, we generaly want to see all the logging
    #[cfg(debug_assertions)]
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Debug)
        .init();

    //If using a release build, we let the user decide
    #[cfg(not(debug_assertions))]
    env_logger::init();

    info!("Starting...");
    let img_path = std::env::args().last().unwrap();
    let (env, display, queue) =
        new_default_environment!(Env, fields = [layer_shell: SimpleGlobal::new(),])
            .expect("Initial roundtrip failed!");

    let surfaces = Rc::new(RefCell::new(Vec::new()));

    let layer_shell = env.require_global::<zwlr_layer_shell_v1::ZwlrLayerShellV1>();

    let env_handle = env.clone();
    let surfaces_handle = Rc::clone(&surfaces);
    let output_handler = move |output: wl_output::WlOutput, info: &OutputInfo| {
        if info.obsolete {
            // an output has been removed, release it
            surfaces_handle.borrow_mut().retain(|(i, _)| *i != info.id);
            output.release();
        } else {
            // an output has been created, construct a surface for it
            let surface = env_handle.create_surface().detach();
            let pool = env_handle
                .create_auto_pool()
                .expect("Failed to create a memory pool!");

            if let Some(s) = Background::new(
                &output,
                surface,
                &layer_shell.clone(),
                pool,
                Path::new(&img_path),
            ) {
                (*surfaces_handle.borrow_mut()).push((info.id, s));
            }
        }
    };

    // Process currently existing outputs
    for output in env.get_all_outputs() {
        if let Some(info) = with_output_info(&output, Clone::clone) {
            output_handler(output, &info);
        }
    }

    // Setup a listener for changes
    // The listener will live for as long as we keep this handle alive
    let _listner_handle =
        env.listen_for_outputs(move |output, info, _| output_handler(output, info));

    let mut event_loop = calloop::EventLoop::<()>::try_new().unwrap();

    WaylandSource::new(queue)
        .quick_insert(event_loop.handle())
        .unwrap();

    loop {
        // This is ugly, let's hope that some version of drain_filter() gets stabilized soon
        // https://github.com/rust-lang/rust/issues/43244
        {
            let mut surfaces = surfaces.borrow_mut();
            let mut i = 0;
            while i != surfaces.len() {
                if surfaces[i].1.handle_events() {
                    surfaces.remove(i);
                } else {
                    i += 1;
                }
            }
        }

        display.flush().unwrap();
        event_loop.dispatch(None, &mut ()).unwrap();
    }
}
