use image::{self, imageops, GenericImageView};
use log::{debug, error, info, warn};
use nix::{sys::stat, unistd::mkfifo};

use smithay_client_toolkit::{
    default_environment,
    environment::SimpleGlobal,
    new_default_environment,
    output::{with_output_info, OutputInfo},
    reexports::{
        calloop::{
            self,
            signals::{Signal, Signals},
        },
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
    io::Read,
    rc::Rc,
};

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
    img_path: String,
    img: Option<Vec<u8>>,
}

impl Background {
    fn new(
        output: &wl_output::WlOutput,
        surface: wl_surface::WlSurface,
        layer_shell: &Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        pool: AutoMemPool,
        img_path: String,
    ) -> Option<Self> {
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
            img_path,
            img: None,
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
                if let Some(img) = self.img_try_open_and_resize() {
                    self.img = Some(img);
                    self.draw();
                }
                false
            }
            None => false,
        }
    }

    fn img_try_open_and_resize(&self) -> Option<Vec<u8>> {
        match image::open(&self.img_path) {
            Ok(img) => {
                let (width, height) = self.dimensions;
                if width == 0 || height == 0 {
                    warn!("Surface dimensions are set to 0. Can't resize image...");
                    return None;
                }

                let img_dimensions = img.dimensions();
                info!("Output dimensions: width: {} height: {}", width, height);
                info!(
                    "Image dimensions:  width: {} height: {}",
                    img_dimensions.0, img_dimensions.1
                );
                let resized_img = if img_dimensions != self.dimensions {
                    info!("Image dimensions are different from output's. Resizing...");
                    img.resize_to_fill(width, height, imageops::FilterType::Lanczos3)
                } else {
                    info!("Image dimensions are identical to output's. Skipped resize!!");
                    img
                };

                // The ARGB is 'little endian', so here we must  put the order
                // of bytes 'in reverse', so it needs to be BGRA.
                info!("Img is ready!");
                return Some(resized_img.into_bgra8().into_raw());
            }
            Err(e) => warn!("Couldn't open image: {}", e),
        }
        None
    }

    fn draw(&mut self) {
        if let Some(img) = &self.img {
            let stride = 4 * self.dimensions.0 as i32;
            let width = self.dimensions.0 as i32;
            let height = self.dimensions.1 as i32;

            match self
                .pool
                .buffer(width, height, stride, wl_shm::Format::Argb8888)
            {
                Ok((canvas, buffer)) => {
                    canvas.copy_from_slice(img.as_slice());
                    info!("Copied bytes to canvas.");

                    std::mem::drop(img);
                    self.img = None;

                    // Attach the buffer to the surface and mark the entire surface as damaged
                    self.surface.attach(Some(&buffer), 0, 0);
                    self.surface
                        .damage_buffer(0, 0, width as i32, height as i32);

                    // Finally, commit the surface
                    self.surface.commit();
                }
                Err(e) => warn!(
                    "Failed to create buffer from mempoll: {}. Image won't be drawn...",
                    e
                ),
            }
        }
    }

    fn update_img(&mut self, new_img: String) {
        self.img_path = new_img;
        if let Some(img) = self.img_try_open_and_resize() {
            self.img = Some(img);
            self.draw();
        }
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
    let signal = Signals::new(&[Signal::SIGUSR1]).unwrap();
    let mut curr_dir = std::env::current_dir().unwrap();
    curr_dir.push("fifo");
    let fifo_path = curr_dir.as_path();

    if !fifo_path.exists() {
        mkfifo(fifo_path, stat::Mode::S_IRWXU).expect("Failed to create fifo file");
    }

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
                img_path.clone(),
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

    let event_handle = event_loop.handle();
    event_handle
        .insert_source(signal, |_, _, _| {
            match std::fs::read_to_string(&fifo_path) {
                Ok(mut fifo_content) => {
                    fifo_content.pop();
                    let mut surfaces = surfaces.borrow_mut();
                    let mut i = 0;
                    while i != surfaces.len() {
                        surfaces[i].1.update_img(fifo_content.clone());
                        i += 1;
                    }
                }
                Err(e) => warn!("Error reading fifo file: {}", e),
            }
        })
        .unwrap();
    WaylandSource::new(queue)
        .quick_insert(event_handle)
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
