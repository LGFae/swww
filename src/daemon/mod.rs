use log::{debug, error, info, warn};
use simplelog::{ColorChoice, LevelFilter, TermLogger, TerminalMode};

use smithay_client_toolkit::{
    environment::Environment,
    output::{with_output_info, OutputInfo},
    reexports::{
        calloop::{self, channel},
        client::protocol::{wl_output, wl_shm, wl_surface},
        client::{Attached, Display, EventQueue, Main},
        protocols::wlr::unstable::layer_shell::v1::client::{
            zwlr_layer_shell_v1, zwlr_layer_surface_v1,
        },
    },
    shm::MemPool,
    WaylandSource,
};

use std::{
    cell::{Cell, RefCell, RefMut},
    fs,
    io::{Read, Write},
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    rc::Rc,
    str::FromStr,
};

use crate::cli::{Clear, Fswww};

mod processor;
mod wayland;

#[derive(PartialEq, Copy, Clone)]
enum RenderEvent {
    Configure { width: u32, height: u32 },
    Closed,
}

pub struct Background {
    output_name: String,
    surface: wl_surface::WlSurface,
    layer_surface: Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    next_render_event: Rc<Cell<Option<RenderEvent>>>,
    pool: MemPool,
    dimensions: (u32, u32),
    img: Option<PathBuf>,
}

impl Background {
    fn new(
        output: &wl_output::WlOutput,
        output_name: String,
        surface: wl_surface::WlSurface,
        layer_shell: &Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        pool: MemPool,
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
            output_name,
            dimensions: (0, 0),
            img: None,
        })
    }

    /// Handles any events that have occurred since the last call, redrawing if needed.
    /// Returns true if the surface should be dropped.
    fn handle_events(&mut self) -> bool {
        match self.next_render_event.take() {
            Some(RenderEvent::Closed) => true,
            Some(RenderEvent::Configure { width, height }) => {
                if self.dimensions != (width, height) {
                    self.dimensions = (width, height);
                    let width = width as usize;
                    let height = height as usize;
                    self.pool.resize(width * height * 4).unwrap();
                    info!("Configured output: {}", self.output_name);
                } else {
                    info!(
                        "Output {} already has correct dimensions.",
                        self.output_name
                    );
                }
                false
            }
            None => false,
        }
    }

    ///'color' argument is in rbg. We copy it correctly to brgx inside the function
    fn clear(&mut self, color: [u8; 3]) {
        let stride = 4 * self.dimensions.0 as i32;
        let width = self.dimensions.0 as i32;
        let height = self.dimensions.1 as i32;

        let buffer = self
            .pool
            .buffer(0, width, height, stride, wl_shm::Format::Xrgb8888);

        let canvas = self.pool.mmap();
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = color[2];
            pixel[1] = color[1];
            pixel[2] = color[0];
        }
        info!("Clearing output: {}", self.output_name);
        self.surface.attach(Some(&buffer), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();
    }

    fn draw(&mut self, img: &[u8]) {
        let stride = 4 * self.dimensions.0 as i32;
        let width = self.dimensions.0 as i32;
        let height = self.dimensions.1 as i32;

        info!(
            "Current state of mempoll for output {}:{:?}",
            self.output_name, self.pool
        );
        let buffer = self
            .pool
            .buffer(0, width, height, stride, wl_shm::Format::Xrgb8888);
        let canvas = self.pool.mmap();
        processor::comp_decomp::mixed_decomp(canvas, img);
        info!("Decompressed img.");

        self.surface.attach(Some(&buffer), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();
    }

    ///This method is what makes necessary that we use the mempoll, instead of the "easier"
    ///automempoll
    fn get_current_img(&mut self) -> Vec<u8> {
        self.pool.mmap().to_vec()
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        self.layer_surface.destroy();
        self.surface.destroy();
    }
}

pub fn main() {
    make_logger();

    let listener = make_socket();
    debug!(
        "Made socket in {:?} and initalized logger. Starting daemon...",
        listener.local_addr().unwrap()
    );

    let (env, display, queue) = wayland::make_wayland_environment();

    let bgs = Rc::new(RefCell::new(Vec::new()));

    let layer_shell = env.require_global::<zwlr_layer_shell_v1::ZwlrLayerShellV1>();

    let env_handle = env.clone();
    let bgs_handle = Rc::clone(&bgs);
    let output_handler = move |output: wl_output::WlOutput, info: &OutputInfo| {
        create_backgrounds(output, info, &env_handle, &bgs_handle, &layer_shell.clone())
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

    //NOTE: we can't move display into the function because it causes a segfault
    run_main_loop(bgs, queue, &display, listener);
    info!("Finished running event loop. Exiting...");
}

fn make_logger() {
    #[cfg(debug_assertions)]
    {
        let config = simplelog::ConfigBuilder::new()
            .set_thread_level(LevelFilter::Info) //let me see where the processing is happenning
            .set_time_format_str("%H:%M:%S%.f") //let me see those nanoseconds
            .build();
        TermLogger::init(
            LevelFilter::Debug,
            config,
            TerminalMode::Stderr,
            ColorChoice::AlwaysAnsi,
        )
        .expect("Failed to initialize logger. Cancelling...");
    }

    #[cfg(not(debug_assertions))]
    {
        TermLogger::init(
            LevelFilter::Warn,
            simplelog::Config::default(),
            TerminalMode::Stderr,
            ColorChoice::Auto,
        )
        .expect("Failed to initialize logger. Cancelling...");
    }
}

fn create_backgrounds(
    output: wl_output::WlOutput,
    info: &OutputInfo,
    env: &Environment<wayland::Env>,
    bgs: &Rc<RefCell<Vec<Background>>>,
    layer_shell: &Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
) {
    if info.obsolete {
        // an output has been removed, release it
        bgs.borrow_mut().retain(|bg| bg.output_name != info.name);
        output.release();
    } else {
        // an output has been created, construct a surface for it
        let surface = env.create_surface().detach();
        let pool = env
            .create_simple_pool(|_dispatch_data| {
                //do I need to do something here???
            })
            .expect("Failed to create a memory pool!");

        debug!("New background with output: {:?}", info);
        if let Some(bg) = Background::new(&output, info.name.clone(), surface, layer_shell, pool) {
            (*bgs.borrow_mut()).push(bg);
        }
    }
}

fn make_socket() -> UnixListener {
    let runtime_dir = if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        dir
    } else {
        "/tmp/fswww".to_string()
    };

    let runtime_dir = Path::new(&runtime_dir);

    if !runtime_dir.exists() {
        fs::create_dir(runtime_dir).expect("Failed to create runtime dir...");
    }

    let socket = runtime_dir.join("fswww.socket");

    UnixListener::bind(socket).expect("Couldn't bind socket")
}

///bgs and display can't be moved into here because it causes a segfault
fn run_main_loop(
    bgs: Rc<RefCell<Vec<Background>>>,
    queue: EventQueue,
    display: &Display,
    listener: UnixListener,
) {
    let (frame_sender, frame_receiver) = calloop::channel::channel();
    let processor = Rc::new(RefCell::new(processor::Processor::new(frame_sender)));
    let mut event_loop = calloop::EventLoop::<calloop::LoopSignal>::try_new().unwrap();
    let event_handle = event_loop.handle();

    event_handle
        .insert_source(frame_receiver, |evt, _, loop_signal| match evt {
            channel::Event::Msg(msg) => handle_recv_img(&mut bgs.borrow_mut(), &msg, None),
            channel::Event::Closed => loop_signal.stop(),
        })
        .unwrap();

    listener.set_nonblocking(true).unwrap();
    event_handle
        .insert_source(
            calloop::generic::Generic::new(listener, calloop::Interest::READ, calloop::Mode::Level),
            |_, listener, loop_signal| {
                let mut processor = processor.borrow_mut();
                recv_socket_msg(bgs.borrow_mut(), listener, loop_signal, &mut processor)
            },
        )
        .unwrap();

    WaylandSource::new(queue)
        .quick_insert(event_handle)
        .unwrap();

    //IMPORTANT: For here on out, any failures must NOT result in a panic. We need to exit cleanly.
    //If it's unrecoverable, we should also delete the socket. Note that on normal exit the cleanup
    //happens at the calling fswww instance (because we can't send back an answer after we've
    //removed the socket. So we can only assure the user the socket has been removed in the fswww
    //client).

    let mut loop_signal = event_loop.get_signal();
    if let Err(e) = event_loop.run(None, &mut loop_signal, |_| {
        {
            let mut bgs = bgs.borrow_mut();
            let mut i = 0;
            while i != bgs.len() {
                if bgs[i].handle_events() {
                    let mut processor = processor.borrow_mut();
                    processor.stop_animations(&[bgs[i].output_name.clone()]);
                    bgs.remove(i);
                } else {
                    i += 1;
                }
            }
        }
        if let Err(e) = display.flush() {
            error!("Couldn't flush display: {}", e);
        }
    }) {
        error!("Event loop closed unexpectedly: {}", e);
    }
}

fn recv_socket_msg(
    mut bgs: RefMut<Vec<Background>>,
    listener: &UnixListener,
    loop_signal: &calloop::LoopSignal,
    processor: &mut processor::Processor,
) -> Result<calloop::PostAction, std::io::Error> {
    match listener.accept() {
        Ok((mut socket, _)) => {
            let mut buf = String::with_capacity(100);
            if let Err(e) = socket.read_to_string(&mut buf) {
                error!("Failed to read socket: {}", e);
                return Err(e);
            };
            let request = Fswww::from_str(&buf);
            let mut answer = Ok(String::new());
            match request {
                Ok(Fswww::Clear(clear)) => answer = clear_outputs(&mut bgs, clear),
                Ok(Fswww::Kill) => loop_signal.stop(),
                Ok(Fswww::Img(img)) => match processor.process(&mut bgs, &img) {
                    Ok(results) => {
                        for result in results {
                            debug!("Received img as processing result");
                            handle_recv_img(&mut bgs, &result, Some(&img.path));
                        }
                    }
                    Err(e) => answer = Err(e),
                },
                Ok(Fswww::Init { .. }) => {
                    answer = clear_outputs(
                        &mut bgs,
                        Clear {
                            outputs: "".to_string(),
                            color: [0, 0, 0],
                        },
                    )
                }
                Ok(Fswww::Query) => answer = Ok(outputs_name_and_dim(&mut bgs)),
                Ok(Fswww::Stream(stream)) => match processor.start_stream(&mut bgs, &stream) {
                    Ok(results) => {
                        for result in results {
                            debug!("Received img as processing result");
                            handle_recv_img(&mut bgs, &result, Some(&stream.path));
                        }
                    }
                    Err(e) => answer = Err(e),
                },
                Err(e) => answer = Err(e),
            }
            send_answer(answer, &listener);
            Ok(calloop::PostAction::Continue)
        }
        Err(e) => Err(e),
    }
}

fn handle_recv_img(
    bgs: &mut RefMut<Vec<Background>>,
    msg: &(Vec<String>, Vec<u8>),
    img_path: Option<&Path>,
) {
    let (outputs, img) = msg;
    if outputs.is_empty() {
        warn!("Received empty list of outputs from processor, which should be impossible");
    }
    for bg in bgs.iter_mut() {
        if outputs.contains(&bg.output_name) {
            if let Some(path) = img_path {
                bg.img = Some(path.to_path_buf());
            }
            bg.draw(img);
        }
    }
}

fn outputs_name_and_dim(bgs: &mut RefMut<Vec<Background>>) -> String {
    let mut str = String::new();
    for bg in bgs.iter() {
        str += &format!(
            "{} - Dimensions: {}x{}\n",
            bg.output_name, bg.dimensions.0, bg.dimensions.1
        );
    }
    str
}

fn clear_outputs(bgs: &mut RefMut<Vec<Background>>, clear: Clear) -> Result<String, String> {
    let mut bgs_to_change = Vec::with_capacity(bgs.len());
    if clear.outputs.is_empty() {
        for bg in bgs.iter_mut() {
            bgs_to_change.push(bg);
        }
    } else {
        for bg in bgs.iter_mut() {
            if clear.outputs.contains(&bg.output_name) {
                bgs_to_change.push(bg);
            }
        }
    }
    if bgs_to_change.is_empty() {
        return Err("None of the specified outputs exist!".to_string());
    }

    for bg in bgs_to_change {
        bg.clear(clear.color);
    }

    Ok("".to_string())
}

fn send_answer(answer: Result<String, String>, listener: &UnixListener) {
    let mut socket;

    match listener.accept() {
        Ok((s, _)) => socket = s,
        Err(e) => {
            error!(
                "Failed to get socket stream while sending answer back: {}",
                e
            );
            return ();
        }
    }

    let send_result = match answer {
        Ok(msg) => socket.write_all(format!("Ok\n{}", msg).as_bytes()),
        Err(err) => socket.write_all(format!("Err\n{}", err).as_bytes()),
    };
    if let Err(e) = send_result {
        error!("Error sending answer back: {}", e);
    }
}
