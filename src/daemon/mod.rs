use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use simplelog::{ColorChoice, LevelFilter, TermLogger, TerminalMode};

use smithay_client_toolkit::{
    environment::Environment,
    output::{with_output_info, OutputInfo},
    reexports::{
        calloop::{
            self, channel,
            signals::{self, Signal},
        },
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
    fmt, fs,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    rc::Rc,
};

use crate::cli::{Clear, Fswww, Img};
use crate::Answer;

mod processor;
mod wayland;

use processor::comp_decomp::Packed;

#[derive(PartialEq, Copy, Clone)]
enum RenderEvent {
    Configure { width: u32, height: u32 },
    Closed,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum BgImg {
    Color([u8; 3]),
    Img(PathBuf),
}

impl fmt::Display for BgImg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BgImg::Color(color) => write!(f, "color: {}{}{}", color[0], color[1], color[2]),
            BgImg::Img(p) => write!(
                f,
                "image: {:#?}",
                p.file_name().unwrap_or_else(|| std::ffi::OsStr::new("?"))
            ),
        }
    }
}

pub struct Bg {
    output_name: String,
    surface: wl_surface::WlSurface,
    layer_surface: Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    next_render_event: Rc<Cell<Option<RenderEvent>>>,
    pool: MemPool,
    dimensions: (u32, u32),
    img: BgImg,
}

impl Bg {
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
            img: BgImg::Color([0, 0, 0]),
        })
    }

    /// Handles any events that have occurred since the last call, redrawing if needed.
    /// Returns whether the surface was configured or not.
    /// If it was, returns whether or not it should be dropped
    fn handle_events(&mut self) -> Option<bool> {
        match self.next_render_event.take() {
            Some(RenderEvent::Closed) => Some(true),
            Some(RenderEvent::Configure { width, height }) => {
                if self.dimensions != (width, height) {
                    self.dimensions = (width, height);
                    self.pool
                        .resize(width as usize * height as usize * 4)
                        .unwrap();
                    self.clear([0, 0, 0]);
                    debug!("Configured output: {}", self.output_name);
                    Some(false)
                } else {
                    debug!(
                        "Output {} is already configured correctly.",
                        self.output_name
                    );
                    None
                }
            }
            None => None,
        }
    }

    ///'color' argument is in rbg. We copy it correctly to brgx inside the function
    fn clear(&mut self, color: [u8; 3]) {
        self.img = BgImg::Color(color);
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
        debug!("Clearing output: {}", self.output_name);
        self.surface.attach(Some(&buffer), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();
    }

    fn draw(&mut self, img: &Packed) {
        //It's possible to receive one extra img from the processor before it shuts down the
        //animation. With this test we stop that (there might be a better way of doing this)
        if let BgImg::Img(_) = self.img {
            let stride = 4 * self.dimensions.0 as i32;
            let width = self.dimensions.0 as i32;
            let height = self.dimensions.1 as i32;

            debug!(
                "Current state of mempoll for output {}:{:?}",
                self.output_name, self.pool
            );
            let buffer = self
                .pool
                .buffer(0, width, height, stride, wl_shm::Format::Xrgb8888);
            let canvas = self.pool.mmap();
            img.unpack(canvas);
            debug!("Decompressed img.");

            self.surface.attach(Some(&buffer), 0, 0);
            self.surface.damage_buffer(0, 0, width, height);
            self.surface.commit();
        }
    }

    ///This method is what makes necessary that we use the mempoll, instead of the "easier"
    ///automempoll
    fn get_current_img(&mut self) -> &[u8] {
        self.pool.mmap()
    }
}

impl Drop for Bg {
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
    main_loop(bgs, queue, &display, listener);
    let socket_addr = get_socket_addr();
    if let Err(e) = fs::remove_file(&socket_addr) {
        error!(
            "Failed to remove socket at {:?} after closing unexpectedly: {}",
            socket_addr, e
        );
    } else {
        info!("Removed socket at {:?}", socket_addr);
    }

    info!("Goodbye!");
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
            LevelFilter::Info,
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
    bgs: &Rc<RefCell<Vec<Bg>>>,
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
        if let Some(bg) = Bg::new(&output, info.name.clone(), surface, layer_shell, pool) {
            (*bgs.borrow_mut()).push(bg);
        }
    }
}

fn make_socket() -> UnixListener {
    let socket_addr = get_socket_addr();
    UnixListener::bind(socket_addr).expect("Couldn't bind socket")
}

fn get_socket_addr() -> PathBuf {
    let runtime_dir = if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        dir
    } else {
        "/tmp/fswww".to_string()
    };

    let runtime_dir = Path::new(&runtime_dir);

    if !runtime_dir.exists() {
        fs::create_dir(runtime_dir).expect("Failed to create runtime dir...");
    }

    runtime_dir.join("fswww.socket")
}

///bgs and display can't be moved into here because it causes a segfault
fn main_loop(
    bgs: Rc<RefCell<Vec<Bg>>>,
    queue: EventQueue,
    display: &Display,
    listener: UnixListener,
) {
    //We use 1 because we can't send a new frame without being absolutely sure that all previous
    //have already been displayed
    let (frame_sender, frame_receiver) = calloop::channel::sync_channel(1);
    let processor = Rc::new(RefCell::new(processor::Processor::new(frame_sender)));
    let mut event_loop = calloop::EventLoop::<calloop::LoopSignal>::try_new().unwrap();
    let event_handle = event_loop.handle();

    //I don't think the signal handling failing here is enough for us to panic.
    if let Ok(signals) = signals::Signals::new(&[Signal::SIGINT, Signal::SIGQUIT, Signal::SIGTERM])
    {
        event_handle
            .insert_source(signals, |_, _, loop_signal| loop_signal.stop())
            .unwrap();
    } else {
        error!("failed to register signals to stop program!");
    }

    event_handle
        .insert_source(frame_receiver, |evt, _, loop_signal| match evt {
            channel::Event::Msg(msg) => handle_recv_img(&mut bgs.borrow_mut(), &msg),
            channel::Event::Closed => loop_signal.stop(),
        })
        .unwrap();

    listener.set_nonblocking(true).unwrap();
    event_handle
        .insert_source(
            calloop::generic::Generic::new(listener, calloop::Interest::READ, calloop::Mode::Level),
            |_, listener, loop_signal| {
                let mut processor = processor.borrow_mut();
                match listener.accept() {
                    Ok((stream, _)) => {
                        match recv_socket_msg(bgs.borrow_mut(), stream, loop_signal, &mut processor)
                        {
                            Err(e) => error!("Failed to receive socket message: {}", e),
                            Ok(()) => {
                                //We must flush here because if multiple requests are sent at once the loop
                                //might never be idle, and so the callback in the run function bellow
                                //wouldn't be called (afaik)
                                if let Err(e) = display.flush() {
                                    error!("Couldn't flush display: {}", e);
                                }
                            }
                        }
                    }
                    Err(e) => error!("Failed to accept connection: {}", e),
                }
                Ok(calloop::PostAction::Continue)
            },
        )
        .unwrap();

    WaylandSource::new(queue)
        .quick_insert(event_handle)
        .unwrap();

    //IMPORTANT: For here on out, any failures must NOT result in a panic. We need to exit cleanly.
    //It is specially important to delete the socket file, since that will cause an attempt to
    //launch a new instance of the daemon to fail
    info!("Initialization succeeded! Starting main loop...");
    let mut loop_signal = event_loop.get_signal();
    if let Err(e) = event_loop.run(None, &mut loop_signal, |_| {
        {
            let mut bgs = bgs.borrow_mut();
            let mut i = 0;
            while i != bgs.len() {
                if let Some(should_remove) = bgs[i].handle_events() {
                    let mut processor = processor.borrow_mut();
                    processor.stop_animations(&[bgs[i].output_name.clone()]);
                    if should_remove {
                        bgs.remove(i);
                    } else {
                        i += 1;
                    }
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
    info!("Finished running event loop.");
}

fn recv_socket_msg(
    mut bgs: RefMut<Vec<Bg>>,
    mut stream: UnixStream,
    loop_signal: &calloop::LoopSignal,
    processor: &mut processor::Processor,
) -> Result<(), String> {
    let request = Fswww::receive(&mut stream);
    let answer = match request {
        Ok(Fswww::Clear(clear)) => clear_outputs(&mut bgs, clear, processor),
        Ok(Fswww::Kill) => {
            loop_signal.stop();
            Answer::Ok
        }
        Ok(Fswww::Img(img)) => processor.process(&mut bgs, img),
        Ok(Fswww::Init { img, color, .. }) => {
            if let Some(img) = img {
                let request = Img {
                    path: img,
                    outputs: "".to_string(),
                    filter: crate::cli::Filter::Lanczos3,
                    transition_step: 255,
                };
                processor.process(&mut bgs, request)
            } else {
                if let Some(color) = color {
                    bgs.iter_mut().for_each(|bg| bg.clear(color));
                }
                Answer::Ok
            }
        }
        Ok(Fswww::Query) => Answer::Info {
            out_dim_img: bgs
                .iter()
                .map(|bg| (bg.output_name.clone(), bg.dimensions, bg.img.clone()))
                .collect(),
        },
        Err(e) => Answer::Err { msg: e },
    };
    answer.send(&stream)
}

fn handle_recv_img(bgs: &mut RefMut<Vec<Bg>>, msg: &(Vec<String>, Packed)) {
    let (outputs, img) = msg;
    if outputs.is_empty() {
        warn!("Received empty list of outputs from processor, which should be impossible");
    }
    bgs.iter_mut()
        .filter(|bg| outputs.contains(&bg.output_name))
        .for_each(|bg| bg.draw(img));
}

fn clear_outputs(
    bgs: &mut RefMut<Vec<Bg>>,
    clear: Clear,
    processor: &mut processor::Processor,
) -> Answer {
    let bgs_to_change: Vec<&mut Bg> = if clear.outputs.is_empty() {
        bgs.iter_mut().collect()
    } else {
        bgs.iter_mut()
            .filter(|bg| clear.outputs.split(',').any(|o| o == bg.output_name))
            .collect()
    };

    if bgs_to_change.is_empty() {
        return Answer::Err {
            msg: "None of the specified outputs exist!".to_string(),
        };
    }

    for bg in bgs_to_change {
        processor.stop_animations(&[bg.output_name.clone()]);
        bg.clear(clear.color);
    }

    Answer::Ok
}
