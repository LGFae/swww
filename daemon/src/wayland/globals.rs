//! `swww-daemon` global variables
//!
//! There are a lot of `static mut`s in here. The strategy to make them safe is as follows:
//!
//! First, this module only exposes getter functions to the mutable statics, meaning they cannot be
//! mutated anywhere but in here.
//!
//! Second, the `pub init(..)` function only executes once. We ensure that using an atomic boolean.
//! This means we will only be mutating these variables inside that function, once.
//!
//! In order to be safe, then, all we have to do is make sure we call `init(..)` as early as
//! possible in the code, and everything will be fine. If we ever fail to that, we have a failsafe
//! with `debug_assert`s in the getter functions, so we would see it explode while debugging.

use rustix::{
    fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd},
    net::SocketAddrAny,
};

use common::ipc::PixelFormat;
use log::{debug, error};

use super::{ObjectId, ObjectManager};
use std::{num::NonZeroU32, path::PathBuf, sync::atomic::AtomicBool};

// all of these objects must always exist for `swww-daemon` to work correctly, so we turn them into
// global constants

pub const WL_DISPLAY: ObjectId = ObjectId(unsafe { NonZeroU32::new_unchecked(1) });
pub const WL_REGISTRY: ObjectId = ObjectId(unsafe { NonZeroU32::new_unchecked(2) });
pub const WL_COMPOSITOR: ObjectId = ObjectId(unsafe { NonZeroU32::new_unchecked(3) });
pub const WL_SHM: ObjectId = ObjectId(unsafe { NonZeroU32::new_unchecked(4) });
pub const WP_VIEWPORTER: ObjectId = ObjectId(unsafe { NonZeroU32::new_unchecked(5) });
pub const ZWLR_LAYER_SHELL_V1: ObjectId = ObjectId(unsafe { NonZeroU32::new_unchecked(6) });

/// wl_display and wl_registry will always be available, but these globals could theoretically be
/// absent. Nevertheless, they are required for `swww-daemon` to function, so we will need to bind
/// all of them.
const REQUIRED_GLOBALS: [&str; 4] = [
    "wl_compositor",
    "wl_shm",
    "wp_viewporter",
    "zwlr_layer_shell_v1",
];

/// Minimal version necessary for `REQUIRED_GLOBALS`
const VERSIONS: [u32; 4] = [4, 1, 1, 3];

/// This is an unsafe static mut that we only ever write to once, during the `init` function call.
/// Any other function in this program can only access this variable through the `wayland_fd`
/// function, which always creates an immutable reference, which should be safe.
static mut WAYLAND_FD: OwnedFd = unsafe { std::mem::zeroed() };

static INITIALIZED: AtomicBool = AtomicBool::new(false);

#[must_use]
pub fn wayland_fd() -> BorrowedFd<'static> {
    debug_assert!(INITIALIZED.load(std::sync::atomic::Ordering::Relaxed));
    let ptr = &raw const WAYLAND_FD;
    unsafe { &*ptr }.as_fd()
}

#[must_use]
pub fn wl_shm_format(pixel_format: PixelFormat) -> u32 {
    match pixel_format {
        PixelFormat::Xrgb => super::interfaces::wl_shm::format::XRGB8888,
        PixelFormat::Xbgr => super::interfaces::wl_shm::format::XBGR8888,
        PixelFormat::Rgb => super::interfaces::wl_shm::format::RGB888,
        PixelFormat::Bgr => super::interfaces::wl_shm::format::BGR888,
    }
}

/// Note that this function assumes the logger has already been set up
pub fn init(pixel_format: Option<PixelFormat>) -> InitState {
    if INITIALIZED.load(std::sync::atomic::Ordering::Relaxed) {
        panic!("trying to run initialization code twice");
    }

    unsafe {
        WAYLAND_FD = connect();
    }
    let mut initializer = Initializer::new(pixel_format);

    // the only globals that can break catastrophically are WAYLAND_FD and OBJECT_MANAGER, that we
    // have just initialized above. So this is safe
    INITIALIZED.store(true, std::sync::atomic::Ordering::SeqCst);

    // these functions already require for the wayland file descriptor and the object manager to
    // have been initialized, which we just did above
    super::interfaces::wl_display::req::get_registry().unwrap();
    super::interfaces::wl_display::req::sync(ObjectId::new(NonZeroU32::new(3).unwrap())).unwrap();

    const IDS: [ObjectId; 4] = [WL_COMPOSITOR, WL_SHM, WP_VIEWPORTER, ZWLR_LAYER_SHELL_V1];

    // this loop will process and store all advertised wayland globals, storing their global name
    // in the Initializer struct
    while !initializer.should_exit {
        let (msg, payload) = super::wire::WireMsg::recv().unwrap();
        if msg.sender_id().get() == 3 {
            super::interfaces::wl_callback::event(&mut initializer, msg, payload);
        } else if msg.sender_id() == WL_DISPLAY {
            super::interfaces::wl_display::event(&mut initializer, msg, payload);
        } else if msg.sender_id() == WL_REGISTRY {
            super::interfaces::wl_registry::event(&mut initializer, msg, payload);
        } else {
            panic!("Did not receive expected global events from registry")
        }
    }

    // if we failed to find some necessary global, panic
    if let Some((_, missing)) = initializer
        .global_names
        .iter()
        .zip(REQUIRED_GLOBALS)
        .find(|(name, _)| **name == 0)
    {
        panic!("Compositor does not implement required interface: {missing}");
    }

    // bind all the globals we need
    for (i, name) in initializer.global_names.into_iter().enumerate() {
        let id = IDS[i];
        let interface = REQUIRED_GLOBALS[i];
        let version = VERSIONS[i];
        super::interfaces::wl_registry::req::bind(name, id, interface, version).unwrap();
    }

    // bind fractional scale, if it is supported
    if let Some(fractional_scale_manager) = initializer.fractional_scale.as_ref() {
        super::interfaces::wl_registry::req::bind(
            fractional_scale_manager.name.get(),
            fractional_scale_manager.id,
            "wp_fractional_scale_manager_v1",
            1,
        )
        .unwrap();
    }

    let callback_id = initializer.callback_id();
    super::interfaces::wl_display::req::sync(callback_id).unwrap();
    initializer.should_exit = false;
    // this loop will go through all the advertised wl_shm format, selecting one for the
    // PIXEL_FORMAT global, if `--format <..>` wasn't passed as a command line argument
    while !initializer.should_exit {
        let (msg, payload) = super::wire::WireMsg::recv().unwrap();
        match msg.sender_id() {
            // in case there are errors
            WL_DISPLAY => super::interfaces::wl_display::event(&mut initializer, msg, payload),
            WL_REGISTRY => super::interfaces::wl_registry::event(&mut initializer, msg, payload),
            WL_SHM => super::interfaces::wl_shm::event(&mut initializer, msg, payload),
            other => {
                if other == callback_id {
                    super::interfaces::wl_callback::event(&mut initializer, msg, payload);
                } else {
                    error!("received unexpected event from compositor during initialization")
                }
            }
        }
    }

    initializer.into_init_state()
}

/// mostly copy-pasted from `wayland-client.rs`
fn connect() -> OwnedFd {
    if let Ok(txt) = std::env::var("WAYLAND_SOCKET") {
        // We should connect to the provided WAYLAND_SOCKET
        let fd = txt
            .parse::<i32>()
            .expect("invalid fd in WAYLAND_SOCKET env var");
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        let socket_addr =
            rustix::net::getsockname(&fd).expect("failed to get wayland socket address");
        if let SocketAddrAny::Unix(_) = socket_addr {
            fd
        } else {
            panic!("socket address {:?} is not a unix socket", socket_addr);
        }
    } else {
        let socket_name: PathBuf = std::env::var_os("WAYLAND_DISPLAY")
            .unwrap_or_else(|| {
                log::warn!("WAYLAND_DISPLAY is not set! Defaulting to wayland-0");
                std::ffi::OsString::from("wayland-0")
            })
            .into();

        let socket_path = if socket_name.is_absolute() {
            socket_name
        } else {
            let mut socket_path: PathBuf = std::env::var_os("XDG_RUNTIME_DIR")
                .unwrap_or_else(|| {
                    log::warn!("XDG_RUNTIME_DIR is not set! Defaulting to /run/user/UID");
                    let uid = rustix::process::getuid();
                    std::ffi::OsString::from(format!("/run/user/{}", uid.as_raw()))
                })
                .into();

            socket_path.push(socket_name);
            socket_path
        };

        match std::os::unix::net::UnixStream::connect(&socket_path) {
            Ok(stream) => stream.into(),
            Err(e) => panic!("failed to connect to wayland socket at {socket_path:?}: {e}"),
        }
    }
}

#[derive(Clone)]
pub struct FractionalScaleManager {
    id: ObjectId,
    name: NonZeroU32,
}

impl FractionalScaleManager {
    pub fn id(&self) -> ObjectId {
        self.id
    }
}

/// Helper struct to do all the initialization in this file
struct Initializer {
    objman: ObjectManager,
    pixel_format: PixelFormat,
    global_names: [u32; REQUIRED_GLOBALS.len()],
    output_names: Vec<u32>,
    fractional_scale: Option<FractionalScaleManager>,
    forced_shm_format: bool,
    should_exit: bool,
}

/// Helper struct to expose all of the initialized state
pub struct InitState {
    pub output_names: Vec<u32>,
    pub fractional_scale: Option<FractionalScaleManager>,
    pub objman: ObjectManager,
    pub pixel_format: PixelFormat,
}

impl Initializer {
    fn new(cli_format: Option<PixelFormat>) -> Self {
        Self {
            objman: ObjectManager::new(),
            global_names: [0; REQUIRED_GLOBALS.len()],
            output_names: Vec::new(),
            fractional_scale: None,
            forced_shm_format: cli_format.is_some(),
            should_exit: false,
            pixel_format: cli_format.unwrap_or(PixelFormat::Xrgb),
        }
    }

    fn callback_id(&self) -> ObjectId {
        if self.fractional_scale.is_some() {
            ObjectId(unsafe { NonZeroU32::new_unchecked(8) })
        } else {
            ObjectId(unsafe { NonZeroU32::new_unchecked(7) })
        }
    }

    fn into_init_state(self) -> InitState {
        debug!("Initialization Over");
        InitState {
            output_names: self.output_names,
            fractional_scale: self.fractional_scale,
            objman: self.objman,
            pixel_format: self.pixel_format,
        }
    }

    pub fn output_names(&self) -> &[u32] {
        &self.output_names
    }

    pub fn fractional_scale(&self) -> Option<&FractionalScaleManager> {
        self.fractional_scale.as_ref()
    }
}

impl super::interfaces::wl_display::HasObjman for Initializer {
    fn objman(&mut self) -> &mut ObjectManager {
        &mut self.objman
    }
}

impl super::interfaces::wl_display::EvHandler for Initializer {
    fn delete_id(&mut self, id: u32) {
        if id == 3 // initial callback for the roundtrip
            || self.fractional_scale.is_none() && id == 7
            || self.fractional_scale.is_some() && id == 8
        {
            self.should_exit = true;
        } else {
            panic!("ObjectId removed during initialization! This should be very rare, which is why we don't deal with it");
        }
    }
}

impl super::interfaces::wl_callback::EvHandler for Initializer {
    fn done(&mut self, sender_id: ObjectId, _callback_data: u32) {
        debug!(
            "Initialization: {} callback done",
            if sender_id.get() == 3 {
                "first"
            } else {
                "second"
            }
        );
    }
}

impl super::interfaces::wl_registry::EvHandler for Initializer {
    fn global(&mut self, name: u32, interface: &str, version: u32) {
        match interface {
            "wp_fractional_scale_manager_v1" => {
                self.fractional_scale = Some(FractionalScaleManager {
                    id: ObjectId(unsafe { NonZeroU32::new_unchecked(7) }),
                    name: name.try_into().unwrap(),
                });
                self.objman.set_fractional_scale_support(true);
            }
            "wl_output" => {
                if version < 4 {
                    error!("wl_output implementation must have at least version 4 for swww-daemon")
                } else {
                    self.output_names.push(name);
                }
            }
            _ => {
                for (i, global) in REQUIRED_GLOBALS.iter().enumerate() {
                    if *global == interface {
                        if version < VERSIONS[i] {
                            panic!(
                                "{interface} version must be at least {} for swww",
                                VERSIONS[i]
                            );
                        }
                        self.global_names[i] = name;
                        break;
                    }
                }
            }
        }
    }

    fn global_remove(&mut self, _name: u32) {
        panic!("Global removed during initialization! This should be very rare, which is why we don't deal with it");
    }
}

impl super::interfaces::wl_shm::EvHandler for Initializer {
    fn format(&mut self, format: u32) {
        match format {
            super::interfaces::wl_shm::format::XRGB8888 => {
                debug!("available shm format: Xrbg");
            }
            super::interfaces::wl_shm::format::XBGR8888 => {
                debug!("available shm format: Xbgr");
                if !self.forced_shm_format && self.pixel_format == PixelFormat::Xrgb {
                    self.pixel_format = PixelFormat::Xbgr;
                }
            }
            super::interfaces::wl_shm::format::RGB888 => {
                debug!("available shm format: Rbg");
                if !self.forced_shm_format && self.pixel_format != PixelFormat::Bgr {
                    self.pixel_format = PixelFormat::Rgb
                }
            }
            super::interfaces::wl_shm::format::BGR888 => {
                debug!("available shm format: Bgr");
                if !self.forced_shm_format {
                    self.pixel_format = PixelFormat::Bgr
                }
            }
            _ => (),
        }
    }
}
