//! All expects in this program must be carefully chosen on purpose. The idea is that if any of
//! them fail there is no point in continuing. All of the initialization code, for example, is full
//! of `expects`, **on purpose**, because we **want** to unwind and exit when they happen

mod animations;
pub mod bump_pool;
mod cli;
mod wallpaper;
use log::{debug, error, info, warn, LevelFilter};
use rustix::{
    event::{poll, PollFd, PollFlags},
    path::Arg,
};
use simplelog::{ColorChoice, TermLogger, TerminalMode, ThreadLogMode};
use wallpaper::Wallpaper;
use wayland_protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::WpFractionalScaleV1,
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

use std::{
    fs,
    num::NonZeroI32,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
};

use wayland_client::{
    backend::WeakBackend,
    globals::{registry_queue_init, GlobalList, GlobalListContents},
    protocol::{
        wl_callback::WlCallback,
        wl_compositor::WlCompositor,
        wl_output,
        wl_region::WlRegion,
        wl_registry::WlRegistry,
        wl_shm::{self, WlShm},
        wl_surface::{self, WlSurface},
    },
    Connection, Dispatch, Proxy, QueueHandle,
};

use utils::ipc::{
    connect_to_socket, get_socket_path, read_socket, AnimationRequest, Answer, BgInfo,
    ImageRequest, PixelFormat, Request, Scale,
};

use animations::Animator;

pub type LayerShell =
    wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
pub type LayerSurface =
    wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;

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

static PIXEL_FORMAT: OnceLock<PixelFormat> = OnceLock::new();
static BACKEND: OnceLock<WeakBackend> = OnceLock::new();

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
pub fn flush_wayland() {
    debug_assert!(BACKEND.get().is_some());
    BACKEND.get().unwrap().upgrade().unwrap().flush().unwrap();
}

#[inline]
pub fn pixel_format() -> PixelFormat {
    debug_assert!(PIXEL_FORMAT.get().is_some());
    *PIXEL_FORMAT.get().unwrap_or(&PixelFormat::Xrgb)
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
    setup_signals();

    let conn = Connection::connect_to_env().expect("failed to connect to the wayland server");
    BACKEND.set(conn.backend().downgrade()).unwrap();
    // Enumerate the list of globals to get the protocols the server implements.
    let (globals, mut event_queue) =
        registry_queue_init(&conn).expect("failed to initialize the event queue");
    let qh = event_queue.handle();

    let mut daemon = Daemon::new(&globals, &qh, cli.no_cache);
    // roundtrip to get the shm formats before setting up the outputs
    event_queue.roundtrip(&mut daemon).unwrap();

    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" {
            if global.version >= 2 {
                let output = globals
                    .registry()
                    .bind(global.name, global.version, &qh, ());
                daemon.new_output(&qh, output, global.name);
            } else {
                error!("wl_output must be at least version 2 for swww-daemon!")
            }
        }
    }

    if let Ok(true) = sd_notify::booted() {
        if let Err(e) = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]) {
            error!("Error sending status update to systemd: {}", e.to_string());
        }
    }
    info!("Initialization succeeded! Starting main loop...");
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
                PollFd::new(&connection_fd, PollFlags::IN),
                PollFd::new(&listener.0, PollFlags::IN),
            ];

            match poll(&mut fds, -1) {
                Ok(_) => (),
                Err(e) => match e {
                    rustix::io::Errno::INTR => (),
                    _ => panic!("failed to poll file descriptors: {e}"),
                },
            };

            [fds[0].revents(), fds[1].revents()]
        };

        if !events[0].is_empty() {
            match read_guard.read() {
                Ok(_) => {
                    event_queue
                        .dispatch_pending(&mut daemon)
                        .expect("failed to dispatch events");
                }
                Err(e) => match e {
                    wayland_client::backend::WaylandError::Io(io) => match io.kind() {
                        std::io::ErrorKind::WouldBlock => {
                            warn!("failed to read wayland events because it would block")
                        }
                        _ => panic!("Io error when reading wayland events: {io}"),
                    },
                    wayland_client::backend::WaylandError::Protocol(e) => {
                        panic!("{e}")
                    }
                },
            }
        } else {
            drop(read_guard);
        }

        if !events[1].is_empty() {
            match listener.0.accept() {
                Ok((stream, _adr)) => daemon.recv_socket_msg(stream),
                Err(e) => match e.kind() {
                    std::io::ErrorKind::WouldBlock => (),
                    _ => return Err(format!("failed to accept incoming connection: {e}")),
                },
            }
        }
    }

    info!("Goodbye!");
    Ok(())
}

fn setup_signals() {
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

    for signal in [libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGHUP] {
        let ret =
            unsafe { libc::sigaction(signal, std::ptr::addr_of!(sigaction), std::ptr::null_mut()) };
        if ret != 0 {
            error!("Failed to install signal handler!")
        }
    }
}

/// This is a wrapper that makes sure to delete the socket when it is dropped
struct SocketWrapper(UnixListener);
impl SocketWrapper {
    fn new() -> Result<Self, String> {
        let socket_addr = get_socket_path();

        if socket_addr.exists() {
            if is_daemon_running(&socket_addr)? {
                return Err(
                    "There is an swww-daemon instance already running on this socket!".to_string(),
                );
            } else {
                warn!(
                    "socket file {} was not deleted when the previous daemon exited",
                    socket_addr.to_string_lossy()
                );
                if let Err(e) = std::fs::remove_file(&socket_addr) {
                    return Err(format!("failed to delete previous socket: {e}"));
                }
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
    shm: WlShm,
    pixel_format: PixelFormat,
    shm_format: wl_shm::Format,
    viewporter: WpViewporter,
    fractional_scale_manager: Option<WpFractionalScaleManagerV1>,

    // swww stuff
    wallpapers: Vec<Arc<Wallpaper>>,
    animator: Animator,
    use_cache: bool,
}

impl Daemon {
    fn new(globals: &GlobalList, qh: &QueueHandle<Self>, no_cache: bool) -> Self {
        // The compositor (not to be confused with the server which is commonly called the compositor) allows
        // configuring surfaces to be presented.
        let compositor: WlCompositor = globals
            .bind(qh, 1..=6, ())
            .expect("wl_compositor is not available");

        let layer_shell: LayerShell = globals
            .bind(qh, 1..=4, ())
            .expect("layer shell is not available");

        let shm: WlShm = globals
            .bind(qh, 1..=1, ())
            .expect("wl_shm is not available");

        let pixel_format = PixelFormat::Xrgb;
        let shm_format = wl_shm::Format::Xrgb8888;

        let viewporter = globals
            .bind(qh, 1..=1, ())
            .expect("viewported not available");
        let fractional_scale_manager = globals.bind(qh, 1..=1, ()).ok();

        Self {
            layer_shell,
            compositor,
            shm,
            pixel_format,
            shm_format,
            viewporter,
            fractional_scale_manager,

            wallpapers: Vec::new(),
            animator: Animator::new(),
            use_cache: !no_cache,
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
            Request::Animation(AnimationRequest {
                animations,
                outputs,
            }) => {
                let mut wallpapers = Vec::new();
                for names in outputs.iter() {
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
            Request::Img(ImageRequest {
                transition,
                imgs,
                outputs,
            }) => {
                let mut used_wallpapers = Vec::new();
                for names in outputs.iter() {
                    let mut wallpapers = self.find_wallpapers_by_names(names);
                    for wallpaper in wallpapers.iter_mut() {
                        wallpaper.stop_animations();
                    }
                    used_wallpapers.push(wallpapers);
                }
                self.animator.transition(transition, imgs, used_wallpapers)
            }
        };
        if let Err(e) = answer.send(&stream) {
            error!("error sending answer to client: {e}");
        }
    }

    fn wallpapers_info(&self) -> Box<[BgInfo]> {
        self.wallpapers
            .iter()
            .map(|wallpaper| wallpaper.get_bg_info())
            .collect()
    }

    fn find_wallpapers_by_names(&self, names: &[String]) -> Vec<Arc<Wallpaper>> {
        self.wallpapers
            .iter()
            .filter_map(|wallpaper| {
                if names.is_empty() || names.iter().any(|n| wallpaper.has_name(n)) {
                    return Some(Arc::clone(wallpaper));
                }
                None
            })
            .collect()
    }

    fn new_output(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput, name: u32) {
        if PIXEL_FORMAT.get().is_none() {
            assert!(PIXEL_FORMAT.set(self.pixel_format).is_ok());
            log::info!("Selected wl_shm format: {:?}", self.shm_format);
        }

        let surface = self.compositor.create_surface(qh, ());

        // Wayland clients are expected to render the cursor on their input region.
        // By setting the input region to an empty region, the compositor renders the
        // default cursor. Without this, an empty desktop won't render a cursor.
        let region = self.compositor.create_region(qh, ());
        surface.set_input_region(Some(&region));

        let layer_surface = self.layer_shell.get_layer_surface(
            &surface,
            Some(&output),
            wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Background,
            "swww-daemon".to_string(),
            qh,
            (),
        );

        let wp_viewport = self.viewporter.get_viewport(&surface, qh, ());
        let wp_fractional = self
            .fractional_scale_manager
            .as_ref()
            .map(|f| f.get_fractional_scale(&surface, qh, surface.clone()));

        debug!("New output: {}", output.id());
        self.wallpapers.push(Arc::new(Wallpaper::new(
            output,
            name,
            surface,
            wp_viewport,
            wp_fractional,
            layer_surface,
            &self.shm,
            qh,
        )));
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Daemon {
    fn event(
        state: &mut Self,
        proxy: &wl_output::WlOutput,
        event: <wl_output::WlOutput as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        for wallpaper in state.wallpapers.iter_mut() {
            if wallpaper.has_output(proxy) {
                match event {
                    wl_output::Event::Geometry {
                        x, y, transform, ..
                    } => {
                        debug!("output {} position: {x},{y}", proxy.id());
                        match transform {
                            wayland_client::WEnum::Value(v) => match v {
                                wl_output::Transform::_90
                                | wl_output::Transform::_270
                                | wl_output::Transform::Flipped90
                                | wl_output::Transform::Flipped270 => wallpaper.set_vertical(),
                                wl_output::Transform::Normal
                                | wl_output::Transform::_180
                                | wl_output::Transform::Flipped
                                | wl_output::Transform::Flipped180 => wallpaper.set_horizontal(),
                                e => warn!("unprocessed transform: {e:?}"),
                            },
                            wayland_client::WEnum::Unknown(u) => {
                                error!("received unknown transform from compositor: {u}")
                            }
                        }
                    }
                    wl_output::Event::Mode {
                        flags: _flags,
                        width,
                        height,
                        ..
                    } => wallpaper.set_dimensions(width, height),
                    wl_output::Event::Done => wallpaper.commit_surface_changes(state.use_cache),
                    wl_output::Event::Scale { factor } => match NonZeroI32::new(factor) {
                        Some(factor) => wallpaper.set_scale(Scale::Whole(factor)),
                        None => error!("received scale factor of 0 from compositor"),
                    },
                    wl_output::Event::Name { name } => wallpaper.set_name(name),
                    wl_output::Event::Description { description } => {
                        wallpaper.set_desc(description)
                    }
                    e => error!("unrecognized WlOutput event: {e:?}"),
                }
                return;
            }
        }
        warn!("received event for non-existing output")
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
        error!("WlCompositor has no events");
    }
}

impl Dispatch<WlSurface, ()> for Daemon {
    fn event(
        state: &mut Self,
        proxy: &WlSurface,
        event: <WlSurface as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::Enter { output } => debug!("Output {}: Surface Enter", output.id()),
            wl_surface::Event::Leave { output } => debug!("Output {}: Surface Leave", output.id()),
            wl_surface::Event::PreferredBufferScale { factor } => {
                for wallpaper in state.wallpapers.iter_mut() {
                    if wallpaper.has_surface(proxy) {
                        match NonZeroI32::new(factor) {
                            Some(factor) => {
                                wallpaper.set_scale(Scale::Whole(factor));
                                wallpaper.commit_surface_changes(state.use_cache);
                            }
                            None => error!("received scale factor of 0 from compositor"),
                        }
                        return;
                    }
                }
                warn!("received new scale factor for non-existing surface")
            }
            wl_surface::Event::PreferredBufferTransform { .. } => {
                warn!("Received transform. We currently ignore those")
            }
            e => error!("unrecognized WlSurface event: {e:?}"),
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
        error!("WlRegion has no events")
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
            e => warn!("unhandled WlShm event: {e:?}"),
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
            e => error!("unrecognized WlCallback event: {e:?}"),
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Daemon {
    fn event(
        state: &mut Self,
        proxy: &WlRegistry,
        event: <WlRegistry as wayland_client::Proxy>::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wayland_client::protocol::wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                if interface.as_str() == "wl_output" {
                    if version < 2 {
                        error!("your compositor must support at least version 2 of wl_output");
                    } else {
                        let output = proxy.bind(name, version, qh, ());
                        state.new_output(qh, output, name)
                    }
                }
            }

            wayland_client::protocol::wl_registry::Event::GlobalRemove { name } => {
                state.wallpapers.retain(|w| !w.has_output_name(name));

                debug!("Destroyed output with id: {name}");
            }
            e => error!("unrecognized WlRegistry event: {e:?}"),
        }
    }
}

impl Dispatch<WpViewporter, ()> for Daemon {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewporter,
        _event: <WpViewporter as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        error!("WpViewporter has no events");
    }
}

impl Dispatch<WpViewport, ()> for Daemon {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewport,
        _event: <WpViewport as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        error!("WpViewport has no events");
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for Daemon {
    fn event(
        _state: &mut Self,
        _proxy: &WpFractionalScaleManagerV1,
        _event: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        error!("WpFractionalScaleManagerV1 has no events");
    }
}

impl Dispatch<WpFractionalScaleV1, WlSurface> for Daemon {
    fn event(
        state: &mut Self,
        _proxy: &WpFractionalScaleV1,
        event: <WpFractionalScaleV1 as Proxy>::Event,
        data: &WlSurface,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1;
        match event {
            wp_fractional_scale_v1::Event::PreferredScale { scale } => {
                for wallpaper in state.wallpapers.iter_mut() {
                    if wallpaper.has_surface(data) {
                        match NonZeroI32::new(scale as i32) {
                            Some(factor) => {
                                wallpaper.set_scale(Scale::Fractional(factor));
                                wallpaper.commit_surface_changes(state.use_cache);
                            }
                            None => error!("received scale factor of 0 from compositor"),
                        }
                        return;
                    }
                }
                warn!("received new fractional scale factor for non-existing surface")
            }
            e => error!("unrecognized WpFractionalScaleV1 event: {e:?}"),
        }
    }
}

impl Dispatch<LayerShell, ()> for Daemon {
    fn event(
        _state: &mut Self,
        _proxy: &LayerShell,
        _event: <LayerShell as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        error!("LayerShell has no events");
    }
}

impl Dispatch<LayerSurface, ()> for Daemon {
    fn event(
        state: &mut Self,
        proxy: &LayerSurface,
        event: <LayerSurface as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Event;
        match event {
            Event::Configure { serial, .. } => {
                for w in &mut state.wallpapers {
                    if w.has_layer_surface(proxy) {
                        proxy.ack_configure(serial);
                        return;
                    }
                }
            }
            Event::Closed => {
                state.wallpapers.retain(|w| !w.has_layer_surface(proxy));
            }
            e => error!("unrecognized LayerSurface event: {e:?}"),
        }
    }
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

pub fn is_daemon_running(addr: &PathBuf) -> Result<bool, String> {
    let sock = match connect_to_socket(addr, 5, 100) {
        Ok(s) => s,
        // likely a connection refused; either way, this is a reliable signal there's no surviving
        // daemon.
        Err(_) => return Ok(false),
    };

    Request::Ping.send(&sock)?;
    let answer = Answer::receive(&read_socket(&sock)?);
    match answer {
        Answer::Ping(_) => Ok(true),
        _ => Err("Daemon did not return Answer::Ping, as expected".to_string()),
    }
}
