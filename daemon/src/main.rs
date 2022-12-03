use log::{debug, error, info, warn};
use simplelog::{ColorChoice, LevelFilter, TermLogger, TerminalMode, ThreadLogMode};

use smithay_client_toolkit::{
    environment::Environment,
    get_surface_scale_factor,
    output::{with_output_info, OutputInfo},
    reexports::{
        calloop::{
            self,
            channel::{self, Channel},
            signals::{self, Signal},
            LoopHandle, LoopSignal,
        },
        client::protocol::{wl_output, wl_shm, wl_surface},
        client::{protocol::wl_compositor, Attached, Display, EventQueue, Main},
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
    os::unix::net::{UnixListener, UnixStream},
    rc::Rc,
};

use utils::{
    communication::{get_socket_path, Answer, BgImg, BgInfo, Clear, Img, Request},
    comp_decomp::ReadiedPack,
};

mod processor;
mod wayland;

use processor::Processor;

#[derive(PartialEq, Copy, Clone)]
enum RenderEvent {
    Configure { width: u32, height: u32 },
    Closed,
}

struct Bg {
    info: BgInfo,
    surface: wl_surface::WlSurface,
    layer_surface: Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    next_render_event: Rc<Cell<Option<RenderEvent>>>,
    pool: MemPool,
}

impl Bg {
    fn new(
        output: &wl_output::WlOutput,
        output_name: String,
        surface: wl_surface::WlSurface,
        layer_shell: &Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        pool: MemPool,
    ) -> Self {
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            Some(output),
            zwlr_layer_shell_v1::Layer::Background,
            "swww".to_owned(),
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

        Self {
            surface,
            layer_surface,
            next_render_event,
            pool,
            info: BgInfo {
                name: output_name,
                dim: (0, 0),
                scale_factor: 1,
                img: BgImg::Color([0, 0, 0]),
            },
        }
    }

    /// Handles any events that have occurred since the last call, redrawing if needed.
    /// Returns whether the surface was configured or not.
    /// If it was, returns whether or not it should be dropped
    fn handle_events(&mut self) -> Option<bool> {
        match self.next_render_event.take() {
            Some(RenderEvent::Closed) => Some(true),
            Some(RenderEvent::Configure { width, height }) => {
                let scale_factor = get_surface_scale_factor(&self.surface);
                if self.info.dim != (width, height) || self.info.scale_factor != scale_factor {
                    self.surface.set_buffer_scale(scale_factor);
                    self.info.dim = (width, height);
                    self.info.scale_factor = scale_factor;
                    let width = width as usize * scale_factor as usize;
                    let height = height as usize * scale_factor as usize;
                    self.pool.resize(width * height * 4).unwrap();
                    // We must clear the outputs so that animations work due to the new underlying
                    // buffer needing to be the exact size of the monitor's.
                    self.clear([0, 0, 0]);
                    debug!("Configured {}", self.info);
                    Some(false)
                } else {
                    debug!("Output {} is already configured correctly", self.info.name);
                    None
                }
            }
            None => None,
        }
    }

    ///'color' argument is in rbg. We copy it correctly to brgx inside the function
    fn clear(&mut self, color: [u8; 3]) {
        self.info.img = BgImg::Color(color);
        let dim = self.info.real_dim();
        let stride = 4 * dim.0 as i32;
        let width = dim.0 as i32;
        let height = dim.1 as i32;

        let buffer = self
            .pool
            .buffer(0, width, height, stride, wl_shm::Format::Xrgb8888);

        let canvas = self.pool.mmap();
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = color[2];
            pixel[1] = color[1];
            pixel[2] = color[0];
        }
        debug!("Clearing output: {}", self.info.name);
        self.surface.attach(Some(&buffer), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();
    }

    fn draw(&mut self, img: &ReadiedPack) {
        let dim = self.info.real_dim();
        let stride = 4 * dim.0 as i32;
        let width = dim.0 as i32;
        let height = dim.1 as i32;

        let buffer = self
            .pool
            .buffer(0, width, height, stride, wl_shm::Format::Xrgb8888);
        let canvas = self.pool.mmap();
        if !img.unpack(canvas){
            error!("buf_len different from expected_buf_size");
        }
        debug!("Decompressed img.");

        self.surface.attach(Some(&buffer), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();
    }

    ///This method is what makes necessary that we use the mempoll, instead of the "easier"
    ///automempoll
    fn get_current_img(&mut self) -> &[u8] {
        let dim = self.info.real_dim();
        let size = dim.0 as usize * dim.1 as usize * 4;
        &self.pool.mmap()[0..size]
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
        create_backgrounds(
            &output,
            info,
            &env_handle,
            &bgs_handle,
            &layer_shell.clone(),
        );
    };
    // Process currently existing outputs
    for output in env.get_all_outputs() {
        if let Some(info) = with_output_info(&output, Clone::clone) {
            output_handler(output, &info);
        }
    }

    let _listner_handle =
        env.listen_for_outputs(move |output, info, _| output_handler(output, info));

    //NOTE: we can't move display into the function because it causes a segfault
    if let Err(e) = main_loop(&bgs, queue, &display, listener) {
        error!("{}", e);
    } else {
        info!("Finished running event loop.");
    }

    let socket_addr = get_socket_path();
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
    let config = simplelog::ConfigBuilder::new()
        .set_thread_level(LevelFilter::Info) //let me see where the processing is happenning
        .set_thread_mode(ThreadLogMode::Both)
        .build();

    TermLogger::init(
        LevelFilter::Debug,
        config,
        TerminalMode::Stderr,
        ColorChoice::AlwaysAnsi,
    )
    .expect("Failed to initialize logger. Cancelling...");
}

fn create_backgrounds(
    output: &wl_output::WlOutput,
    info: &OutputInfo,
    env: &Environment<wayland::Env>,
    bgs: &Rc<RefCell<Vec<Bg>>>,
    layer_shell: &Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
) {
    if info.obsolete {
        // an output has been removed, release it
        bgs.borrow_mut().retain(|bg| bg.info.name != info.name);
        output.release();
    } else {
        // an output has been created, construct a surface for it
        let surface = env.create_surface().detach();
        let pool = env
            .create_simple_pool(|_dispatch_data| {
                //do I need to do something here???
            })
            .expect("Failed to create a memory pool!");

        // Wayland clients are expected to render the cursor on their input region. By setting the
        // input region to an empty region, the compositor renders the default cursor. Without
        // this, and empty desktop won't render a cursor.
        let compositor = env.require_global::<wl_compositor::WlCompositor>();
        let empty_region = compositor.create_region();
        surface.set_input_region(Some(&empty_region));

        // From `wl_surface::set_opaque_region`:
        // > Setting the pending opaque region has copy semantics, and the
        // > wl_region object can be destroyed immediately.
        empty_region.destroy();

        debug!("New background with output: {:?}", info);
        let bg = Bg::new(output, info.name.clone(), surface, layer_shell, pool);
        bgs.borrow_mut().push(bg);
    }
}

fn make_socket() -> UnixListener {
    let socket_addr = get_socket_path();
    let runtime_dir = socket_addr
        .parent()
        .expect("couldn't find a valid runtime directory");

    if !runtime_dir.exists() {
        fs::create_dir(runtime_dir).expect("Failed to create runtime dir...");
    }

    UnixListener::bind(socket_addr).expect("Couldn't bind socket")
}

fn register_signals(handle: &LoopHandle<LoopSignal>) -> Result<(), String> {
    match signals::Signals::new(&[Signal::SIGINT, Signal::SIGQUIT, Signal::SIGTERM]) {
        Ok(signals) => {
            if let Err(e) = handle.insert_source(signals, |_, _, loop_signal| loop_signal.stop()) {
                Err(format!("failed to insert signals source: {}", e))
            } else {
                Ok(())
            }
        }
        Err(e) => Err(format!("failed to register signals to stop program: {}", e)),
    }
}

fn register_channel<'a>(
    handle: &LoopHandle<'a, LoopSignal>,
    bgs: &'a Rc<RefCell<Vec<Bg>>>,
    fr_recv: Channel<(Vec<String>, ReadiedPack)>,
) -> Result<(), String> {
    if let Err(e) = handle.insert_source(fr_recv, |evt, _, loop_signal| match evt {
        channel::Event::Msg(msg) => handle_recv_img(&mut bgs.borrow_mut(), &msg),
        channel::Event::Closed => loop_signal.stop(),
    }) {
        return Err(format! {"failed to register channel: {}", e});
    }
    Ok(())
}

fn register_socket<'a>(
    handle: &LoopHandle<'a, LoopSignal>,
    bgs: &'a Rc<RefCell<Vec<Bg>>>,
    display: &'a Display,
    processor: &'a Rc<RefCell<Processor>>,
    listener: UnixListener,
) -> Result<(), String> {
    listener.set_nonblocking(true).unwrap();
    if let Err(e) = handle.insert_source(
        calloop::generic::Generic::new(listener, calloop::Interest::READ, calloop::Mode::Level),
        |_, listener, loop_signal| {
            let mut processor = processor.borrow_mut();
            match listener.accept() {
                Ok((stream, _)) => {
                    match recv_socket_msg(bgs.borrow_mut(), stream, loop_signal, &mut processor) {
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
    ) {
        return Err(format! {"failed to register socket: {}", e});
    }
    Ok(())
}
///bgs and display can't be moved into here because it causes a segfault
fn main_loop(
    bgs: &Rc<RefCell<Vec<Bg>>>,
    queue: EventQueue,
    display: &Display,
    listener: UnixListener,
) -> Result<(), String> {
    //We use 1 because we can't send a new frame without being absolutely sure that all previous
    //have already been displayed. Using 0 causes the animation to stop.
    let (frame_sender, frame_receiver) = calloop::channel::sync_channel(1);
    let processor = Rc::new(RefCell::new(Processor::new(frame_sender)));
    let mut event_loop = match calloop::EventLoop::<calloop::LoopSignal>::try_new() {
        Ok(el) => el,
        Err(e) => return Err(e.to_string()),
    };
    let event_handle = event_loop.handle();

    register_signals(&event_handle)?;
    register_channel(&event_handle, bgs, frame_receiver)?;
    register_socket(&event_handle, bgs, display, &processor, listener)?;

    if let Err(e) = WaylandSource::new(queue).quick_insert(event_handle) {
        return Err(e.to_string());
    }

    info!("Initialization succeeded! Starting main loop...");
    let mut loop_signal = event_loop.get_signal();
    if let Err(e) = event_loop.run(None, &mut loop_signal, |_| {
        {
            let mut bgs = bgs.borrow_mut();
            let mut i = 0;
            while i != bgs.len() {
                if let Some(should_remove) = bgs[i].handle_events() {
                    let mut processor = processor.borrow_mut();
                    processor.stop_animations(&[bgs[i].info.name.clone()]);
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
        return Err(format!("Event loop closed unexpectedly: {}", e));
    }

    Ok(())
}

fn recv_socket_msg(
    mut bgs: RefMut<Vec<Bg>>,
    mut stream: UnixStream,
    loop_signal: &calloop::LoopSignal,
    proc: &mut Processor,
) -> Result<(), String> {
    let request = Request::receive(&mut stream);
    let answer = match request {
        Ok(Request::Animation(animations)) => {
            let mut result = Answer::Ok;
            for animation in &animations {
                for output in &animation.1 {
                    if !bgs.iter().any(|bg| &bg.info.name == output) {
                        result = Answer::Err(format!("Output {} doesn't exit", output));
                        break;
                    }
                }
            }
            if matches!(result, Answer::Err(_)) {
                result
            } else {
                for animation in animations {
                    let dim = bgs.iter().find(|bg| animation.1.contains(&bg.info.name)).unwrap().info.real_dim();
                    let size = dim.0 as usize * dim.1 as usize * 4;
                    if let Answer::Err(e) = proc.animate(animation.0, animation.1, size) {
                        result = Answer::Err(e);
                    }
                }
                result
            }
        },
        Ok(Request::Clear(clear)) => clear_outputs(&mut bgs, &clear, proc),
        Ok(Request::Kill) => {
            loop_signal.stop();
            Answer::Ok
        }
        Ok(Request::Img(img)) => {
            let old_imgs = get_old_imgs(&mut bgs, &img.1);
            if old_imgs.len() != img.1.len() {
                Answer::Err("Daemon received request for outputs that don't exist".to_string())
            } else {
                proc.transition(img.0, img.1, old_imgs)
            }
        }
        Ok(Request::Init { .. }) => Answer::Ok,
        Ok(Request::Query) => Answer::Info(bgs.iter().map(|bg| bg.info.clone()).collect()),
        Err(e) => Answer::Err(e),
    };
    answer.send(&stream)
}

fn get_old_imgs(
    bgs: &mut RefMut<Vec<Bg>>,
    imgs: &[(Img, Vec<String>)],
) -> Vec<(Box<[u8]>, (u32, u32))> {
    let mut v = Vec::with_capacity(imgs.len());

    for (_, outputs) in imgs {
        if let Some(bg) = bgs.iter_mut().find(|bg| bg.info.name == outputs[0]) {
            v.push((bg.get_current_img().into(), bg.info.real_dim()))
        }
    }

    v
}

fn handle_recv_img(bgs: &mut RefMut<Vec<Bg>>, msg: &(Vec<String>, ReadiedPack)) {
    let (outputs, img) = msg;
    if outputs.is_empty() {
        warn!("Received empty list of outputs from processor, which should be impossible");
    }
    bgs.iter_mut()
        .filter(|bg| outputs.contains(&bg.info.name))
        .for_each(|bg| bg.draw(img));
}

fn clear_outputs(bgs: &mut RefMut<Vec<Bg>>, clear: &Clear, proc: &mut Processor) -> Answer {
    proc.stop_animations(&clear.outputs);
    bgs.iter_mut()
        .filter(|bg| clear.outputs.contains(&bg.info.name))
        .for_each(|bg| bg.clear(clear.color));
    Answer::Ok
}
