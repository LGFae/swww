//! All expects in this program must be carefully chosen on purpose. The idea is that if any of
//! them fail there is no point in continuing. All of the initialization code, for example, is full
//! of `expects`, **on purpose**, because we **want** to unwind and exit when they happen

mod imgproc;
mod wallpaper;
use log::{debug, error, info, LevelFilter};
use nix::{
    poll::{poll, PollFd, PollFlags},
    sys::signal::{self, SigHandler, Signal},
};
use simplelog::{ColorChoice, TermLogger, TerminalMode, ThreadLogMode};
use wallpaper::{OutputId, Wallpaper};

use std::{
    fs,
    num::NonZeroI32,
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::{UnixListener, UnixStream},
    },
    sync::{Arc, Mutex, MutexGuard, RwLock},
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
    shm::{slot::SlotPool, Shm, ShmHandler},
};

use wayland_client::{
    globals::{registry_queue_init, GlobalList},
    protocol::{wl_output, wl_surface},
    Connection, QueueHandle,
};

use utils::communication::{get_socket_path, Answer, BgImg, BgInfo, Request};

use imgproc::Imgproc;

// We need this because this might be set by signals, so we can't keep it in the daemon
static EXIT: RwLock<bool> = RwLock::new(false);

fn exit_daemon() {
    let mut lock = EXIT.write().expect("failed to lock EXIT for writing");
    *lock = true;
}

fn should_daemon_exit() -> bool {
    *EXIT.read().expect("failed to read EXIT")
}

extern "C" fn signal_handler(s: i32) {
    // SIGUSR1 simply signals us to stop polling, since we need to draw something
    if let Ok(Signal::SIGUSR1) = Signal::try_from(s) {
        return;
    }
    exit_daemon();
}

type DaemonResult<T> = Result<T, String>;
fn main() -> DaemonResult<()> {
    make_logger();
    let listener = SocketWrapper::new()?;

    let handler = SigHandler::Handler(signal_handler);
    for signal in [
        Signal::SIGINT,
        Signal::SIGQUIT,
        Signal::SIGTERM,
        Signal::SIGUSR1,
    ] {
        unsafe { signal::signal(signal, handler).expect("Failed to install signal handler") };
    }

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
    let mut poll_handler = PollHandler::new(&listener);
    while !should_daemon_exit() {
        // Process wayland events
        event_queue
            .flush()
            .expect("failed to flush the event queue");
        event_queue
            .dispatch_pending(&mut daemon)
            .expect("failed to dispatch events");
        let read_guard = event_queue
            .prepare_read()
            .expect("failed to prepare the event queue's read");

        poll_handler.block(read_guard.connection_fd().as_raw_fd());

        if poll_handler.has_event(PollHandler::WAYLAND_FD) {
            read_guard.read().expect("failed to read the event queue");
            event_queue
                .dispatch_pending(&mut daemon)
                .expect("failed to dispatch events");
        }

        if poll_handler.has_event(PollHandler::SOCKET_FD) {
            match listener.0.accept() {
                Ok((stream, _addr)) => daemon.recv_socket_msg(stream),
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

struct PollHandler {
    fds: [PollFd; 2],
}

impl PollHandler {
    const SOCKET_FD: usize = 0;
    const WAYLAND_FD: usize = 1;

    pub fn new(listener: &SocketWrapper) -> Self {
        Self {
            fds: [
                PollFd::new(listener.0.as_raw_fd(), PollFlags::POLLIN),
                PollFd::new(0, PollFlags::POLLIN),
            ],
        }
    }

    pub fn block(&mut self, wayland_fd: RawFd) {
        self.fds[Self::WAYLAND_FD] =
            PollFd::new(wayland_fd, PollFlags::POLLIN | PollFlags::POLLRDBAND);
        match poll(&mut self.fds, -1) {
            Ok(_) => (),
            Err(e) => match e {
                nix::errno::Errno::EINTR => (),
                _ => panic!("failed to poll file descriptors: {e}"),
            },
        };
    }

    pub fn has_event(&self, fd_index: usize) -> bool {
        if let Some(flags) = self.fds[fd_index].revents() {
            !flags.is_empty()
        } else {
            false
        }
    }
}

struct Daemon {
    // Wayland stuff
    layer_shell: LayerShell,
    compositor_state: CompositorState,
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: Arc<Mutex<SlotPool>>,

    // swww stuff
    wallpapers: Arc<Mutex<Vec<Wallpaper>>>,
    imgproc: Imgproc,
}

impl Daemon {
    fn new(globals: &GlobalList, qh: &QueueHandle<Self>) -> Self {
        // The compositor (not to be confused with the server which is commonly called the compositor) allows
        // configuring surfaces to be presented.
        let compositor_state =
            CompositorState::bind(globals, qh).expect("wl_compositor is not available");

        let layer_shell = LayerShell::bind(globals, qh).expect("layer shell is not available");

        let shm = Shm::bind(globals, qh).expect("wl_shm is not available");
        let pool = SlotPool::new(256 * 256 * 4, &shm).expect("failed to create SlotPool");

        Self {
            // Outputs may be hotplugged at runtime, therefore we need to setup a registry state to
            // listen for Outputs.
            registry_state: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            compositor_state,
            shm,
            pool: Arc::new(Mutex::new(pool)),
            layer_shell,

            wallpapers: Arc::new(Mutex::new(Vec::new())),
            imgproc: Imgproc::new(),
        }
    }

    fn recv_socket_msg(&mut self, stream: UnixStream) {
        let request = Request::receive(&stream);
        let answer = match request {
            Ok(request) => match request {
                Request::Animation(animations) => {
                    let mut result = Answer::Ok;
                    for animation in animations {
                        let ids = self.find_wallpapers_id_by_names(animation.1);
                        result =
                            self.imgproc
                                .animate(&self.pool, animation.0, ids, &self.wallpapers);
                    }
                    result
                }
                Request::Clear(clear) => {
                    let ids = self.find_wallpapers_id_by_names(clear.outputs);
                    self.clear_by_id(ids, clear.color);
                    Answer::Ok
                }
                Request::Init => Answer::Ok,
                Request::Kill => {
                    exit_daemon();
                    Answer::Ok
                }
                Request::Query => Answer::Info(self.wallpapers_info()),
                Request::Img((transition, imgs)) => {
                    let mut requests = Vec::new();
                    for img in imgs {
                        let ids = self.find_wallpapers_id_by_names(img.1);
                        requests.push((img.0, ids));
                    }
                    self.imgproc
                        .transition(&self.pool, transition, requests, &self.wallpapers);
                    Answer::Ok
                }
            },
            Err(e) => Answer::Err(e),
        };
        if let Err(e) = answer.send(&stream) {
            error!("error sending answer to client: {e}");
        }
    }

    fn wallpapers_info(&self) -> Vec<BgInfo> {
        let (_pool, wallpapers) = lock_pool_and_wallpapers(&self.pool, &self.wallpapers);
        self.output_state
            .outputs()
            .filter_map(|output| {
                if let Some(info) = self.output_state.info(&output) {
                    if let Some(wallpaper) = wallpapers.iter().find(|w| w.output_id.0 == info.id) {
                        return Some(BgInfo {
                            name: info.name.unwrap_or("?".to_string()),
                            dim: info
                                .logical_size
                                .map(|(width, height)| (width as u32, height as u32))
                                .unwrap_or((0, 0)),
                            scale_factor: info.scale_factor,
                            img: wallpaper.img.clone(),
                        });
                    }
                }
                None
            })
            .collect()
    }

    fn find_wallpapers_id_by_names(&self, names: Vec<String>) -> Vec<OutputId> {
        self.output_state
            .outputs()
            .filter_map(|output| {
                if let Some(info) = self.output_state.info(&output) {
                    if let Some(name) = info.name {
                        if names.is_empty() || names.contains(&name) {
                            return Some(OutputId(info.id));
                        }
                    }
                }
                None
            })
            .collect()
    }

    fn clear_by_id(&mut self, ids: Vec<OutputId>, color: [u8; 3]) {
        let (mut pool, mut wallpapers) = lock_pool_and_wallpapers(&self.pool, &self.wallpapers);
        for wallpaper in wallpapers.iter_mut() {
            if ids.contains(&wallpaper.output_id) {
                wallpaper.img = BgImg::Color(color);
                wallpaper.clear(&mut pool, color);
                wallpaper.draw(&mut pool);
            }
        }
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
        let (mut pool, mut wallpapers) = lock_pool_and_wallpapers(&self.pool, &self.wallpapers);
        for wallpaper in wallpapers.iter_mut() {
            if wallpaper.layer_surface.wl_surface() == surface {
                wallpaper.resize(
                    &mut pool,
                    wallpaper.width,
                    wallpaper.height,
                    NonZeroI32::new(new_factor).unwrap(),
                );
                //wallpaper.draw(&mut pool);
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
        let (mut pool, mut wallpapers) = lock_pool_and_wallpapers(&self.pool, &self.wallpapers);
        for wallpaper in wallpapers.iter_mut() {
            if wallpaper.layer_surface.wl_surface() == surface {
                wallpaper.draw(&mut pool);
                return;
            }
        }
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

            {
                let (mut pool, mut wallpapers) =
                    lock_pool_and_wallpapers(&self.pool, &self.wallpapers);
                wallpapers.push(Wallpaper::new(output_info, layer_surface, &mut pool));
            }
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
                let (mut pool, mut wallpapers) =
                    lock_pool_and_wallpapers(&self.pool, &self.wallpapers);
                for wallpaper in wallpapers.iter_mut() {
                    if wallpaper.output_id.0 == output_info.id {
                        let (width, height) = (
                            NonZeroI32::new(output_size.0).unwrap(),
                            NonZeroI32::new(output_size.1).unwrap(),
                        );
                        let scale_factor = NonZeroI32::new(output_info.scale_factor).unwrap();
                        if (width, height, scale_factor)
                            != (wallpaper.width, wallpaper.height, wallpaper.scale_factor)
                        {
                            wallpaper.resize(&mut pool, width, height, scale_factor);
                        }
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
            let (mut _pools, mut wallpapers) =
                lock_pool_and_wallpapers(&self.pool, &self.wallpapers);
            wallpapers.retain(|w| w.output_id.0 != output_info.id);
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
        let (mut _pools, mut wallpapers) = lock_pool_and_wallpapers(&self.pool, &self.wallpapers);
        wallpapers.retain(|w| w.layer_surface != *layer)
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        _configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // After configuring, we try to import the cache
        let (pool, mut wallpapers) = lock_pool_and_wallpapers(&self.pool, &self.wallpapers);
        for wallpaper in wallpapers.iter_mut() {
            if wallpaper.layer_surface == *layer {
                if let Some(output_info) = self.output_state.outputs().find_map(|o| {
                    if let Some(info) = self.output_state.info(&o) {
                        if info.id == wallpaper.output_id.0 {
                            return Some(info);
                        }
                    }
                    None
                }) {
                    let id = OutputId(output_info.id);
                    let output_name = output_info.name.clone().unwrap_or("?".to_string());
                    let logical_size = output_info
                        .logical_size
                        .map(|(width, height)| (width as usize, height as usize))
                        .unwrap_or((0, 0));

                    drop(pool);
                    drop(wallpapers);
                    self.imgproc.import_cached_img(
                        &self.pool,
                        &self.wallpapers,
                        id,
                        &output_name,
                        logical_size.0 * logical_size.1 * 4,
                    );
                }
                return;
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
        .set_thread_level(LevelFilter::Error) //let me see where the processing is happening
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

pub fn lock_pool_and_wallpapers<'a>(
    pool: &'a Arc<Mutex<SlotPool>>,
    wallpapers: &'a Arc<Mutex<Vec<Wallpaper>>>,
) -> (MutexGuard<'a, SlotPool>, MutexGuard<'a, Vec<Wallpaper>>) {
    use std::sync::TryLockError;
    loop {
        match (pool.try_lock(), wallpapers.try_lock()) {
            (Ok(pool), Ok(wallpapers)) => return (pool, wallpapers),
            (Err(TryLockError::WouldBlock), Ok(_))
            | (Ok(_), Err(TryLockError::WouldBlock))
            | (Err(TryLockError::WouldBlock), Err(TryLockError::WouldBlock)) => {
                std::thread::yield_now()
            }
            _ => panic!("failed to lock"),
        }
    }
}
