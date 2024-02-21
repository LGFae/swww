//! All expects in this program must be carefully chosen on purpose. The idea is that if any of
//! them fail there is no point in continuing. All of the initialization code, for example, is full
//! of `expects`, **on purpose**, because we **want** to unwind and exit when they happen

mod animations;
mod wallpaper;
use log::{debug, error, info, warn, LevelFilter};
use nix::{
    poll::{poll, PollFd, PollFlags},
    sys::signal::{self, SigHandler, Signal},
};
use rkyv::{boxed::ArchivedBox, string::ArchivedString};
use simplelog::{ColorChoice, TermLogger, TerminalMode, ThreadLogMode};
use wallpaper::Wallpaper;

use std::{
    fs,
    num::NonZeroI32,
    os::{
        fd::{BorrowedFd, RawFd},
        unix::net::{UnixListener, UnixStream},
    },
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        WaylandSurface,
    },
    shm::{multi::MultiPool, Shm, ShmHandler},
};

use wayland_client::{
    globals::{registry_queue_init, GlobalList},
    protocol::{wl_output, wl_surface},
    Connection, QueueHandle,
};

use utils::ipc::{get_socket_path, Answer, ArchivedRequest, BgInfo, Request};

use animations::Animator;

// We need this because this might be set by signals, so we can't keep it in the daemon
static EXIT: AtomicBool = AtomicBool::new(false);

#[inline]
fn exit_daemon() {
    EXIT.store(true, Ordering::Release);
}

#[inline]
fn should_daemon_exit() -> bool {
    EXIT.load(Ordering::Acquire)
}

static POLL_WAKER: OnceLock<RawFd> = OnceLock::new();

pub fn wake_poll() {
    if let Err(e) = nix::unistd::write(*unsafe { POLL_WAKER.get().unwrap_unchecked() }, &[0]) {
        error!("failed to write to pipe file descriptor: {e}");
    }
}

extern "C" fn signal_handler(_s: i32) {
    exit_daemon();
}

fn main() -> Result<(), String> {
    rayon::ThreadPoolBuilder::default()
        .thread_name(|i| format!("rayon thread {i}"))
        .build_global()
        .expect("failed to configure rayon global thread pool");
    make_logger();
    let listener = SocketWrapper::new()?;
    let wake = setup_signals_and_pipe();

    let conn = Connection::connect_to_env().expect("failed to connect to the wayland server");
    // Enumerate the list of globals to get the protocols the server implements.
    let (globals, mut event_queue) =
        registry_queue_init(&conn).expect("failed to initialize the event queue");
    let qh = event_queue.handle();

    let mut daemon = Daemon::new(&globals, &qh);

    if let Ok(true) = sd_notify::booted() {
        if let Err(e) = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]) {
            error!("Error sending status update to systemd: {}", e.to_string());
        }
    }
    info!("Initialization succeeded! Starting main loop...");
    let mut buf = [0; 16];
    while !should_daemon_exit() {
        // Process wayland events
        event_queue
            .flush()
            .expect("failed to flush the event queue");
        let read_guard = event_queue
            .prepare_read()
            .expect("failed to prepare the event queue's read");

        let events = {
            let connection_fd = read_guard.connection_fd();
            let waker = unsafe { BorrowedFd::borrow_raw(wake) };
            let mut fds = [
                PollFd::new(&listener.0, PollFlags::POLLIN),
                PollFd::new(&connection_fd, PollFlags::POLLIN | PollFlags::POLLRDBAND),
                PollFd::new(&waker, PollFlags::POLLIN),
            ];

            match poll(&mut fds, -1) {
                Ok(_) => (),
                Err(e) => match e {
                    nix::errno::Errno::EINTR => (),
                    _ => panic!("failed to poll file descriptors: {e}"),
                },
            };

            [fds[0].revents(), fds[1].revents(), fds[2].revents()]
        };

        if let Some(flags) = events[1] {
            if !flags.is_empty() {
                read_guard.read().expect("failed to read the event queue");
                event_queue
                    .dispatch_pending(&mut daemon)
                    .expect("failed to dispatch events");
            }
        }

        if let Some(flags) = events[0] {
            if !flags.is_empty() {
                match listener.0.accept() {
                    Ok((stream, _adr)) => daemon.recv_socket_msg(stream),
                    Err(e) => match e.kind() {
                        std::io::ErrorKind::WouldBlock => (),
                        _ => return Err(format!("failed to accept incoming connection: {e}")),
                    },
                }
            }
        }

        if let Some(flags) = events[2] {
            if !flags.is_empty() {
                if let Err(e) = nix::unistd::read(wake, &mut buf) {
                    error!("error reading pipe file descriptor: {e}");
                }
            }
        }
    }

    if let Err(e) = nix::unistd::close(*POLL_WAKER.get().unwrap()) {
        error!("error closing write pipe file descriptor: {e}");
    }
    if let Err(e) = nix::unistd::close(wake) {
        error!("error closing read pipe file descriptor: {e}");
    }

    info!("Goodbye!");
    Ok(())
}

/// Returns the file descriptor we should install in the poll handler
fn setup_signals_and_pipe() -> RawFd {
    let handler = SigHandler::Handler(signal_handler);
    for signal in [Signal::SIGINT, Signal::SIGQUIT, Signal::SIGTERM] {
        unsafe { signal::signal(signal, handler).expect("failed to install signal handler") };
    }
    let (r, w) = nix::unistd::pipe().expect("failed to create pipe");
    let _ = POLL_WAKER.get_or_init(|| w);
    r
}

/// This is a wrapper that makes sure to delete the socket when it is dropped
/// It also makes sure to set the listener to nonblocking mode
struct SocketWrapper(UnixListener);
impl SocketWrapper {
    fn new() -> Result<Self, String> {
        let socket_addr = get_socket_path();
        let runtime_dir = match socket_addr.parent() {
            Some(path) => path,
            None => return Err("couldn't find a valid runtime directory".to_owned()),
        };

        if !runtime_dir.exists() {
            match fs::create_dir(runtime_dir) {
                Ok(()) => (),
                Err(e) => return Err(format!("failed to create runtime dir: {e}")),
            }
        }

        let listener = match UnixListener::bind(socket_addr.clone()) {
            Ok(address) => address,
            Err(e) => return Err(format!("couldn't bind socket: {e}")),
        };

        debug!(
            "Made socket in {:?} and initialized logger. Starting daemon...",
            listener.local_addr().unwrap() //this should always work if the socket connected correctly
        );

        if let Err(e) = listener.set_nonblocking(true) {
            let _ = fs::remove_file(&socket_addr);
            return Err(format!("failed to set socket to nonblocking mode: {e}"));
        }

        Ok(Self(listener))
    }
}

impl Drop for SocketWrapper {
    fn drop(&mut self) {
        let socket_addr = get_socket_path();
        if let Err(e) = fs::remove_file(&socket_addr) {
            error!("Failed to remove socket at {socket_addr:?}: {e}");
        }
        info!("Removed socket at {:?}", socket_addr);
    }
}

struct Daemon {
    // Wayland stuff
    layer_shell: LayerShell,
    compositor_state: CompositorState,
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: wallpaper::MtShmPool,

    // swww stuff
    wallpapers: Vec<Arc<Wallpaper>>,
    animator: Animator,
    initializing: bool,
}

impl Daemon {
    fn new(globals: &GlobalList, qh: &QueueHandle<Self>) -> Self {
        // The compositor (not to be confused with the server which is commonly called the compositor) allows
        // configuring surfaces to be presented.
        let compositor_state =
            CompositorState::bind(globals, qh).expect("wl_compositor is not available");

        let layer_shell = LayerShell::bind(globals, qh).expect("layer shell is not available");

        let shm = Shm::bind(globals, qh).expect("wl_shm is not available");
        let pool = MultiPool::new(&shm).expect("failed to create MultiPool");

        Self {
            // Outputs may be hotplugged at runtime, therefore we need to setup a registry state to
            // listen for Outputs.
            registry_state: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            compositor_state,
            shm,
            pool: Arc::new(Mutex::new(pool)),
            layer_shell,

            wallpapers: Vec::new(),
            animator: Animator::new(),
            initializing: true,
        }
    }

    fn recv_socket_msg(&mut self, stream: UnixStream) {
        let bytes = match utils::ipc::read_socket(&stream) {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("FATAL: cannot read socket: {e}. Exiting...");
                exit_daemon();
                return;
            }
        };
        let request = Request::receive(&bytes);
        let answer = match request {
            ArchivedRequest::Animation(animations) => {
                let mut wallpapers = Vec::new();
                for (_, names) in animations.iter() {
                    wallpapers.push(self.find_wallpapers_by_names(names));
                }
                self.animator.animate(bytes, wallpapers)
            }
            ArchivedRequest::Clear(clear) => {
                self.initializing = false;
                let wallpapers = self.find_wallpapers_by_names(&clear.outputs);
                let color = clear.color;
                match std::thread::Builder::new()
                    .stack_size(1 << 15)
                    .name("clear".to_string())
                    .spawn(move || {
                        for wallpaper in &wallpapers {
                            wallpaper.stop_animations();
                        }
                        for wallpaper in wallpapers {
                            wallpaper.set_img_info(utils::ipc::BgImg::Color(color));
                            let buffer = wallpaper.clear(color);
                            wallpaper.draw(&buffer);
                        }
                        wake_poll();
                    }) {
                    Ok(_) => Answer::Ok,
                    Err(e) => Answer::Err(format!("failed to spawn `clear` thread: {e}")),
                }
            }
            ArchivedRequest::Init => Answer::Init(
                self.wallpapers
                    .iter()
                    .all(|w| w.configured.load(std::sync::atomic::Ordering::Acquire)),
            ),
            ArchivedRequest::Kill => {
                exit_daemon();
                Answer::Ok
            }
            ArchivedRequest::Query => Answer::Info(self.wallpapers_info()),
            ArchivedRequest::Img((_, imgs)) => {
                self.initializing = false;
                let mut used_wallpapers = Vec::new();
                for img in imgs.iter() {
                    let mut wallpapers = self.find_wallpapers_by_names(&img.1);
                    for wallpaper in wallpapers.iter_mut() {
                        wallpaper.stop_animations();
                    }
                    used_wallpapers.push(wallpapers);
                }
                self.animator.transition(bytes, used_wallpapers);
                Answer::Ok
            }
        };
        if let Err(e) = answer.send(&stream) {
            error!("error sending answer to client: {e}");
        }
    }

    fn wallpapers_info(&self) -> Box<[BgInfo]> {
        self.output_state
            .outputs()
            .filter_map(|output| {
                if let Some(info) = self.output_state.info(&output) {
                    if let Some(wallpaper) = self.wallpapers.iter().find(|w| w.has_id(info.id)) {
                        return Some(BgInfo {
                            name: info.name.unwrap_or("?".to_string()),
                            dim: info
                                .logical_size
                                .map(|(width, height)| (width as u32, height as u32))
                                .unwrap_or((0, 0)),
                            scale_factor: info.scale_factor,
                            img: wallpaper.get_img_info(),
                        });
                    }
                }
                None
            })
            .collect()
    }

    fn find_wallpapers_by_names(
        &self,
        names: &ArchivedBox<[ArchivedString]>,
    ) -> Vec<Arc<Wallpaper>> {
        self.output_state
            .outputs()
            .filter_map(|output| {
                if let Some(info) = self.output_state.info(&output) {
                    if let Some(name) = info.name {
                        if names.is_empty() || names.iter().any(|n| n.as_str() == name) {
                            if let Some(wallpaper) =
                                self.wallpapers.iter().find(|w| w.has_id(info.id))
                            {
                                return Some(Arc::clone(wallpaper));
                            }
                        }
                    }
                }
                None
            })
            .collect()
    }
}

impl CompositorHandler for Daemon {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        for wallpaper in self.wallpapers.iter_mut() {
            if wallpaper.has_surface(surface) {
                wallpaper.resize(
                    None,
                    None,
                    Some(unsafe { NonZeroI32::new_unchecked(new_factor) }),
                );
                return;
            }
        }
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        for wallpaper in self.wallpapers.iter_mut() {
            if wallpaper.has_surface(surface) {
                wallpaper.draw(&wallpaper.canvas_change(|_| {}).1);
                return;
            }
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
        // do not do anything for now
    }
}

impl OutputHandler for Daemon {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let Some(output_info) = self.output_state.info(&output) {
            let surface = self.compositor_state.create_surface(qh);

            // Wayland clients are expected to render the cursor on their input region.
            // By setting the input region to an empty region, the compositor renders the
            // default cursor. Without this, an empty desktop won't render a cursor.
            if let Ok(region) = Region::new(&self.compositor_state) {
                surface.set_input_region(Some(region.wl_region()));
            }
            let layer_surface = self.layer_shell.create_layer_surface(
                qh,
                surface,
                Layer::Background,
                Some("swww"),
                Some(&output),
            );

            if !self.initializing {
                if let Some(name) = &output_info.name {
                    let name = name.to_owned();
                    if let Err(e) = std::thread::Builder::new()
                        .name("cache loader".to_string())
                        .stack_size(1 << 14)
                        .spawn(move || {
                            // Wait for a bit for the output to be properly configured and stuff
                            // this is obviously not ideal, but it solves the vast majority of problems
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            if let Err(e) = utils::cache::load(&name) {
                                warn!("failed to load cache: {e}");
                            }
                        })
                    {
                        warn!("failed to spawn `cache loader` thread: {e}");
                    }
                }
            }

            debug!("New output: {output_info:?}");
            self.wallpapers.push(Arc::new(Wallpaper::new(
                output_info,
                layer_surface,
                Arc::clone(&self.pool),
            )));
            debug!("Output count: {}", self.wallpapers.len());
        }
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let Some(output_info) = self.output_state.info(&output) {
            if let Some(output_size) = output_info.logical_size {
                if output_size.0 == 0 || output_size.1 == 0 {
                    error!(
                        "output dimensions cannot be '0'. Received: {:#?}",
                        output_size
                    );
                    return;
                }
                for wallpaper in self.wallpapers.iter_mut() {
                    if wallpaper.has_id(output_info.id) {
                        let (width, height) = (
                            Some(unsafe { NonZeroI32::new_unchecked(output_size.0) }),
                            Some(unsafe { NonZeroI32::new_unchecked(output_size.1) }),
                        );
                        let scale_factor =
                            Some(unsafe { NonZeroI32::new_unchecked(output_info.scale_factor) });
                        wallpaper.resize(width, height, scale_factor);
                        return;
                    }
                }
            }
        }
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let Some(output_info) = self.output_state.info(&output) {
            self.wallpapers.retain(|w| !w.has_id(output_info.id));
            debug!("Destroyed output: {output_info:?}");
        }
    }
}

impl ShmHandler for Daemon {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl LayerShellHandler for Daemon {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        self.wallpapers
            .retain(|w| !w.has_surface(layer.wl_surface()));
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        _configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // we only care about output configs, so we just set the wallpaper as having been
        // configured
        for w in &mut self.wallpapers {
            if w.has_surface(layer.wl_surface()) {
                w.configured
                    .store(true, std::sync::atomic::Ordering::Release);
                break;
            }
        }
    }
}

delegate_compositor!(Daemon);
delegate_output!(Daemon);
delegate_shm!(Daemon);

delegate_layer!(Daemon);

delegate_registry!(Daemon);

impl ProvidesRegistryState for Daemon {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

fn make_logger() {
    let config = simplelog::ConfigBuilder::new()
        .set_thread_level(LevelFilter::Error) // let me see where the processing is happening
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
