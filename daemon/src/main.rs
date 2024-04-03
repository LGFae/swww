//! All expects in this program must be carefully chosen on purpose. The idea is that if any of
//! them fail there is no point in continuing. All of the initialization code, for example, is full
//! of `expects`, **on purpose**, because we **want** to unwind and exit when they happen

mod animations;
pub mod raw_pool;
pub mod bump_pool;
mod cli;
mod wallpaper;
use log::{debug, error, info, warn, LevelFilter};
use rustix::event::{poll, PollFd, PollFlags};
use simplelog::{ColorChoice, TermLogger, TerminalMode, ThreadLogMode};
use wallpaper::Wallpaper;

use std::{
    fs,
    num::NonZeroI32,
    os::{
        fd::OwnedFd,
        unix::net::{UnixListener, UnixStream},
    },
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
};

use smithay_client_toolkit::{
    delegate_layer, delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        WaylandSurface,
    },
};

use wayland_client::{
    globals::{registry_queue_init, GlobalList},
    protocol::{
        wl_buffer::WlBuffer,
        wl_callback::WlCallback,
        wl_compositor::WlCompositor,
        wl_output,
        wl_region::WlRegion,
        wl_shm::{self, WlShm},
        wl_surface::{self, WlSurface},
    },
    Connection, Dispatch, QueueHandle,
};

use utils::ipc::{get_socket_path, Answer, BgInfo, PixelFormat, Request};

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

static POLL_WAKER: OnceLock<OwnedFd> = OnceLock::new();
static PIXEL_FORMAT: OnceLock<PixelFormat> = OnceLock::new();

#[inline]
pub fn wl_shm_format() -> wl_shm::Format {
    match pixel_format() {
        PixelFormat::Xrgb => wl_shm::Format::Xrgb8888,
        PixelFormat::Xbgr => wl_shm::Format::Xbgr8888,
        PixelFormat::Rgb => wl_shm::Format::Rgb888,
        PixelFormat::Bgr => wl_shm::Format::Bgr888,
    }
}

#[inline]
pub fn pixel_format() -> PixelFormat {
    debug_assert!(PIXEL_FORMAT.get().is_some());
    *PIXEL_FORMAT.get().unwrap_or(&PixelFormat::Xrgb)
}

#[inline]
pub fn wake_poll() {
    debug_assert!(POLL_WAKER.get().is_some());

    // SAFETY: POLL_WAKER is set up in setup_signals_and_pipe, which is called early in main
    // and panics if it fails. By the time anyone calls this function, POLL_WAKER will certainly
    // already have been initialized.

    if let Err(e) = rustix::io::write(
        unsafe { POLL_WAKER.get().unwrap_unchecked() },
        &1u64.to_ne_bytes(), // eventfd demands we write 8 bytes at once
    ) {
        error!("failed to write to eventfd file descriptor: {e}");
    }
}

extern "C" fn signal_handler(_s: libc::c_int) {
    exit_daemon();
}

fn main() -> Result<(), String> {
    let cli = cli::Cli::new();
    make_logger(cli.quiet);

    if let Some(format) = cli.format {
        PIXEL_FORMAT.set(format).unwrap();
        info!("Forced usage of wl_shm format: {:?}", wl_shm_format());
    }

    rayon::ThreadPoolBuilder::default()
        .thread_name(|i| format!("rayon thread {i}"))
        .stack_size(1 << 19) // 512KiB; we do not need a large stack
        .build_global()
        .expect("failed to configure rayon global thread pool");

    let listener = SocketWrapper::new()?;
    let wake = setup_signals_and_eventfd();

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
    let mut buf = [0; 8];
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
            let mut fds = [
                PollFd::new(&listener.0, PollFlags::IN),
                PollFd::new(&connection_fd, PollFlags::IN | PollFlags::RDBAND),
                PollFd::new(&wake, PollFlags::IN),
            ];

            match poll(&mut fds, -1) {
                Ok(_) => (),
                Err(e) => match e {
                    rustix::io::Errno::INTR => (),
                    _ => panic!("failed to poll file descriptors: {e}"),
                },
            };

            [fds[0].revents(), fds[1].revents(), fds[2].revents()]
        };

        if !events[1].is_empty() {
            read_guard.read().expect("failed to read the event queue");
            event_queue
                .dispatch_pending(&mut daemon)
                .expect("failed to dispatch events");
        }

        if !events[0].is_empty() {
            match listener.0.accept() {
                Ok((stream, _adr)) => daemon.recv_socket_msg(stream),
                Err(e) => match e.kind() {
                    std::io::ErrorKind::WouldBlock => (),
                    _ => return Err(format!("failed to accept incoming connection: {e}")),
                },
            }
        }

        if !events[2].is_empty() {
            if let Err(e) = rustix::io::read(&wake, &mut buf) {
                error!("error reading pipe file descriptor: {e}");
            }
        }
    }

    info!("Goodbye!");
    Ok(())
}

/// Returns the file descriptor we should install in the poll handler
fn setup_signals_and_eventfd() -> OwnedFd {
    // C data structure, expected to be zeroed out.
    let mut sigaction: libc::sigaction = unsafe { std::mem::zeroed() };
    unsafe { libc::sigemptyset(std::ptr::addr_of_mut!(sigaction.sa_mask)) };

    #[cfg(not(target_os = "aix"))]
    {
        sigaction.sa_sigaction = signal_handler as usize;
    }
    #[cfg(target_os = "aix")]
    {
        sigaction.sa_union.__su_sigaction = handler;
    }

    for signal in [libc::SIGINT, libc::SIGQUIT, libc::SIGTERM] {
        let ret =
            unsafe { libc::sigaction(signal, std::ptr::addr_of!(sigaction), std::ptr::null_mut()) };
        if ret != 0 {
            error!("Failed to install signal handler!")
        }
    }

    let fd: OwnedFd = rustix::event::eventfd(0, rustix::event::EventfdFlags::empty())
        .expect("failed to create event fd");
    POLL_WAKER
        .set(fd.try_clone().expect("failed to clone event fd"))
        .expect("failed to set POLL_WAKER");
    fd
}

/// This is a wrapper that makes sure to delete the socket when it is dropped
/// It also makes sure to set the listener to nonblocking mode
struct SocketWrapper(UnixListener);
impl SocketWrapper {
    fn new() -> Result<Self, String> {
        if is_daemon_running()? {
            return Err("There is an swww-daemon instance already running!".to_string());
        }

        let socket_addr = get_socket_path();
        if socket_addr.exists() {
            warn!(
                "socket file {} was not deleted when the previous daemon exited",
                socket_addr.to_string_lossy()
            );
            if let Err(e) = std::fs::remove_file(&socket_addr) {
                return Err(format!("failed to delete previous socket: {e}"));
            }
        }
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
    compositor: WlCompositor,
    registry_state: RegistryState,
    output_state: OutputState,
    shm: WlShm,
    pixel_format: PixelFormat,
    shm_format: wl_shm::Format,

    // swww stuff
    wallpapers: Vec<Arc<Wallpaper>>,
    animator: Animator,
}

impl Daemon {
    fn new(globals: &GlobalList, qh: &QueueHandle<Self>) -> Self {
        // The compositor (not to be confused with the server which is commonly called the compositor) allows
        // configuring surfaces to be presented.
        let compositor: WlCompositor = globals
            .bind(qh, 1..=6, ())
            .expect("wl_compositor is not available");

        let layer_shell = LayerShell::bind(globals, qh).expect("layer shell is not available");

        let shm: WlShm = globals
            .bind(qh, 1..=1, ())
            .expect("wl_shm is not available");

        let pixel_format = PixelFormat::Xrgb;
        let shm_format = wl_shm::Format::Xrgb8888;

        Self {
            layer_shell,
            // Outputs may be hotplugged at runtime, therefore we need to setup a registry state to
            // listen for Outputs.
            registry_state: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            compositor,
            shm,
            pixel_format,
            shm_format,

            wallpapers: Vec::new(),
            animator: Animator::new(),
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
            Request::Animation(animations) => {
                let mut wallpapers = Vec::new();
                for (_, names) in animations.iter() {
                    wallpapers.push(self.find_wallpapers_by_names(names));
                }
                self.animator.animate(animations, wallpapers)
            }
            Request::Clear(clear) => {
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
                            wallpaper.clear(color);
                            wallpaper.draw();
                        }
                        wake_poll();
                    }) {
                    Ok(_) => Answer::Ok,
                    Err(e) => Answer::Err(format!("failed to spawn `clear` thread: {e}")),
                }
            }
            Request::Ping => Answer::Ping(
                self.wallpapers
                    .iter()
                    .all(|w| w.configured.load(std::sync::atomic::Ordering::Acquire)),
            ),
            Request::Kill => {
                exit_daemon();
                Answer::Ok
            }
            Request::Query => Answer::Info(self.wallpapers_info()),
            Request::Img((transitions, imgs)) => {
                let mut used_wallpapers = Vec::new();
                for img in imgs.iter() {
                    let mut wallpapers = self.find_wallpapers_by_names(&img.1);
                    for wallpaper in wallpapers.iter_mut() {
                        wallpaper.stop_animations();
                    }
                    used_wallpapers.push(wallpapers);
                }
                self.animator.transition(transitions, imgs, used_wallpapers)
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
                            pixel_format: pixel_format(),
                        });
                    }
                }
                None
            })
            .collect()
    }

    fn find_wallpapers_by_names(&self, names: &[String]) -> Vec<Arc<Wallpaper>> {
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
        if PIXEL_FORMAT.get().is_none() {
            assert!(PIXEL_FORMAT.set(self.pixel_format).is_ok());
            log::info!("Selected wl_shm format: {:?}", self.shm_format);
        }
        if let Some(output_info) = self.output_state.info(&output) {
            let surface = self.compositor.create_surface(qh, ());

            // Wayland clients are expected to render the cursor on their input region.
            // By setting the input region to an empty region, the compositor renders the
            // default cursor. Without this, an empty desktop won't render a cursor.
            let region = self.compositor.create_region(qh, ());
            surface.set_input_region(Some(&region));

            let layer_surface = self.layer_shell.create_layer_surface(
                qh,
                surface,
                Layer::Background,
                Some("swww"),
                Some(&output),
            );

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

            debug!("New output: {output_info:?}");
            self.wallpapers.push(Arc::new(Wallpaper::new(
                output_info,
                layer_surface,
                &self.shm,
                qh,
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
                            Some(NonZeroI32::new(output_size.0).unwrap()),
                            Some(NonZeroI32::new(output_size.1).unwrap()),
                        );
                        let scale_factor = Some(NonZeroI32::new(output_info.scale_factor).unwrap());
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

impl Dispatch<WlBuffer, Arc<AtomicBool>> for Daemon {
    fn event(
        _state: &mut Self,
        _proxy: &WlBuffer,
        event: <WlBuffer as wayland_client::Proxy>::Event,
        data: &Arc<AtomicBool>,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wayland_client::protocol::wl_buffer::Event::Release => {
                data.store(true, Ordering::Release);
            }
            _ => log::error!("There should be no buffer events other than Release"),
        }
    }
}

impl Dispatch<WlCompositor, ()> for Daemon {
    fn event(
        _state: &mut Self,
        _proxy: &WlCompositor,
        _event: <WlCompositor as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!("WlCompositor has no events");
    }
}

impl Dispatch<WlSurface, ()> for Daemon {
    fn event(
        state: &mut Self,
        proxy: &WlSurface,
        event: <WlSurface as wayland_client::Proxy>::Event,
        _data: &(),
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::Enter { output } => state.new_output(conn, qh, output),
            wl_surface::Event::Leave { output } => state.output_destroyed(conn, qh, output),
            wl_surface::Event::PreferredBufferScale { factor } => {
                for wallpaper in state.wallpapers.iter_mut() {
                    if wallpaper.has_surface(proxy) {
                        wallpaper.resize(None, None, Some(NonZeroI32::new(factor).unwrap()));
                        return;
                    }
                }
                warn!("received new scale factor for non-existing surface")
            }
            wl_surface::Event::PreferredBufferTransform { .. } => {
                warn!("Received transform. We currently ignore those")
            }
            _ => error!("unrecognized WlSurface event!"),
        }
    }
}

impl Dispatch<WlRegion, ()> for Daemon {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegion,
        _event: <WlRegion as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!("WlRegion has no events")
    }
}

impl Dispatch<WlShm, ()> for Daemon {
    fn event(
        state: &mut Self,
        _proxy: &WlShm,
        event: <WlShm as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_shm::Event::Format { format: wenum } => match wenum {
                wayland_client::WEnum::Value(format) => {
                    if format == wl_shm::Format::Bgr888 {
                        state.shm_format = wl_shm::Format::Bgr888;
                        state.pixel_format = PixelFormat::Bgr;
                    } else if format == wl_shm::Format::Rgb888
                        && state.pixel_format != PixelFormat::Bgr
                    {
                        state.shm_format = wl_shm::Format::Rgb888;
                        state.pixel_format = PixelFormat::Rgb;
                    } else if format == wl_shm::Format::Xbgr8888
                        && state.pixel_format == PixelFormat::Xrgb
                    {
                        state.shm_format = wl_shm::Format::Xbgr8888;
                        state.pixel_format = PixelFormat::Xbgr;
                    }
                }
                wayland_client::WEnum::Unknown(v) => {
                    error!("Received unknown shm format number {v} from server")
                }
            },
            e => warn!("Unhandled WlShm event: {e:?}"),
        }
    }
}


impl Dispatch<WlCallback, WlSurface> for Daemon {
    fn event(
        state: &mut Self,
        _proxy: &WlCallback,
        event: <WlCallback as wayland_client::Proxy>::Event,
        data: &WlSurface,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wayland_client::protocol::wl_callback::Event::Done { callback_data } => {
                for wallpaper in state.wallpapers.iter_mut() {
                    if wallpaper.has_surface(data) {
                        wallpaper.frame_callback_completed(callback_data);
                        return;
                    }
                }
                warn!("received callback for non-existing surface!")
            }
            _ => error!("unrecognized WlCallback event!"),
        }
    }
}

delegate_output!(Daemon);
delegate_layer!(Daemon);
delegate_registry!(Daemon);

impl ProvidesRegistryState for Daemon {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

fn make_logger(quiet: bool) {
    let config = simplelog::ConfigBuilder::new()
        .set_thread_level(LevelFilter::Error) // let me see where the processing is happening
        .set_thread_mode(ThreadLogMode::Both)
        .build();

    TermLogger::init(
        if quiet {
            LevelFilter::Error
        } else {
            LevelFilter::Debug
        },
        config,
        TerminalMode::Stderr,
        ColorChoice::AlwaysAnsi,
    )
    .expect("Failed to initialize logger. Cancelling...");
}

fn is_daemon_running() -> Result<bool, String> {
    let proc = std::path::PathBuf::from("/proc");

    let entries = match proc.read_dir() {
        Ok(e) => e,
        Err(e) => return Err(e.to_string()),
    };

    for entry in entries.flatten() {
        let dirname = entry.file_name();
        if let Ok(pid) = dirname.to_string_lossy().parse::<u32>() {
            if std::process::id() == pid {
                continue;
            }
            let mut entry_path = entry.path();
            entry_path.push("cmdline");
            if let Ok(cmd) = std::fs::read_to_string(entry_path) {
                let mut args = cmd.split(&[' ', '\0']);
                if let Some(arg0) = args.next() {
                    if arg0.ends_with("swww-daemon") {
                        return Ok(true);
                    }
                }
            }
        }
    }

    Ok(false)
}
