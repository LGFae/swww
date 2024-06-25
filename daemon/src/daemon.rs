use std::error::Error;
use std::fs;
use std::mem;
use std::num::NonZeroU32;
use std::path::Path;
use std::ptr;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;

use common::ipc::Answer;
use common::ipc::BgInfo;
use common::ipc::ImageReq;
use common::ipc::IpcSocket;
use common::ipc::RequestRecv;
use common::ipc::RequestSend;
use common::ipc::Server;
use common::mmap::MmappedStr;

use log::debug;
use log::error;
use log::info;
use log::warn;

use rustix::event::poll;
use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::io::Errno;
use rustix::net::accept;

use crate::animations::Animator;
use crate::wallpaper;
use crate::wallpaper::Wallpaper;
use crate::wayland::globals;
use crate::wayland::globals::Initializer;
use crate::wayland::wire;
use crate::wayland::ObjectId;

mod wayland;

// We need this because this might be set by signals, so we can't keep it in the self
static EXIT: AtomicBool = AtomicBool::new(false);

pub struct Daemon {
    pub wallpapers: Vec<Arc<Wallpaper>>,
    pub animator: Animator,
    pub cache: bool,
    pub fractional_scale_manager: Option<(ObjectId, NonZeroU32)>,
}

impl Daemon {
    pub fn new(initializer: Initializer, cache: bool) -> Result<Self, Box<dyn Error>> {
        Self::init_rt_files()?;

        info!("Selected wl_shm format: {:?}", globals::pixel_format());
        let fractional_scale_manager = initializer.fractional_scale().copied();

        let mut daemon = Self {
            wallpapers: Vec::new(),
            animator: Animator::new(),
            cache,
            fractional_scale_manager,
        };

        for name in initializer.output_names().iter().copied() {
            daemon.new_output(name);
        }

        Ok(daemon)
    }

    fn init_rt_files() -> Result<(), Box<dyn Error>> {
        let addr = IpcSocket::<Server>::path();
        let path = Path::new(addr);
        if path.exists() {
            if Daemon::socket_occupied() {
                Err("There is an swww-daemon instance already running on this socket!")?;
            } else {
                warn!("socket file '{addr}' was not deleted when the previous daemon exited",);
                if let Err(err) = fs::remove_file(addr) {
                    Err(format!("failed to delete previous socket: {err}"))?;
                }
            }
        } else {
            let Some(parent) = path.parent() else {
                Err("couldn't find a valid runtime directory")?
            };
            if !parent.exists() {
                if let Err(err) = fs::create_dir(parent) {
                    Err(format!("failed to create runtime dir: {err}"))?
                };
            }
        }
        debug!("Created socket in {addr}");
        Ok(())
    }

    fn wallpapers_info(&self) -> Box<[BgInfo]> {
        self.wallpapers
            .iter()
            .map(|wallpaper| wallpaper.get_bg_info())
            .collect()
    }

    fn find_wallpapers_by_names(&self, names: &[MmappedStr]) -> Vec<Arc<Wallpaper>> {
        self.wallpapers
            .iter()
            .filter_map(|wallpaper| {
                if names.is_empty() || names.iter().any(|n| wallpaper.has_name(n.str())) {
                    return Some(Arc::clone(wallpaper));
                }
                None
            })
            .collect()
    }

    pub fn main_loop(&mut self, socket: IpcSocket<Server>) -> Result<(), Errno> {
        let wayland_fd = globals::wayland_fd();
        let mut fds = [
            PollFd::new(&wayland_fd, PollFlags::IN),
            PollFd::new(socket.as_fd(), PollFlags::IN),
        ];

        // main loop
        while !EXIT.load(Ordering::Acquire) {
            if let Err(e) = poll(&mut fds, -1) {
                match e {
                    Errno::INTR => continue,
                    _ => return Err(e),
                }
            }

            if !fds[0].revents().is_empty() {
                match wire::WireMsg::recv() {
                    Ok((msg, payload)) => self.wayland_handler(msg, payload),
                    Err(Errno::INTR) => continue,
                    Err(err) => return Err(err),
                };
            }

            if !fds[1].revents().is_empty() {
                match accept(socket.as_fd()) {
                    // TODO: abstract away explicit socket creation
                    Ok(stream) => self.request_handler(IpcSocket::new(stream)),
                    Err(Errno::INTR | Errno::WOULDBLOCK) => continue,
                    Err(err) => return Err(err),
                }
            }
        }

        Ok(())
    }

    fn request_handler(&mut self, socket: IpcSocket<Server>) {
        let bytes = match socket.recv() {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("FATAL: cannot read socket: {e}. Exiting...");
                Self::exit();
                return;
            }
        };
        let request = RequestRecv::receive(bytes);
        let answer = match request {
            RequestRecv::Clear(clear) => {
                let wallpapers = self.find_wallpapers_by_names(&clear.outputs);
                thread::Builder::new()
                    .stack_size(1 << 15)
                    .name("clear".to_string())
                    .spawn(move || {
                        wallpaper::stop_animations(&wallpapers);
                        for wallpaper in &wallpapers {
                            wallpaper.set_img_info(common::ipc::BgImg::Color(clear.color));
                            wallpaper.clear(clear.color);
                        }
                        wallpaper::attach_buffers_and_damange_surfaces(&wallpapers);
                        wallpaper::commit_wallpapers(&wallpapers);
                    })
                    .expect("builder only failed if the name contains null bytes");
                Answer::Ok
            }
            RequestRecv::Ping => Answer::Ping(
                self.wallpapers
                    .iter()
                    .all(|w| w.configured.load(std::sync::atomic::Ordering::Acquire)),
            ),
            RequestRecv::Kill => {
                Self::exit();
                Answer::Ok
            }
            RequestRecv::Query => Answer::Info(self.wallpapers_info()),
            RequestRecv::Img(ImageReq {
                transition,
                imgs,
                outputs,
                animations,
            }) => {
                let mut used_wallpapers = Vec::new();
                for names in outputs.iter() {
                    let wallpapers = self.find_wallpapers_by_names(names);
                    wallpaper::stop_animations(&wallpapers);
                    used_wallpapers.push(wallpapers);
                }
                self.animator
                    .transition(transition, imgs, animations, used_wallpapers)
            }
        };
        if let Err(e) = answer.send(&socket) {
            error!("error sending answer to client: {e}");
        }
    }

    fn socket_occupied() -> bool {
        let Ok(socket) = IpcSocket::connect() else {
            return false;
        };

        let Answer::Ping(_) = RequestSend::Ping
            .send(&socket)
            .and_then(|()| socket.recv().map_err(|err| err.to_string()))
            .map(Answer::receive)
            .unwrap_or_else(|err| panic!("{err}"))
        else {
            unreachable!("Daemon did not return Answer::Ping, IPC is broken")
        };
        true
    }

    pub fn exit() {
        EXIT.store(true, Ordering::Release);
    }

    extern "C" fn handler(_: libc::c_int) {
        Self::exit();
    }

    pub fn handle_signals() {
        // C data structure, expected to be zeroed out.
        let mut sigaction: libc::sigaction = unsafe { mem::zeroed() };
        unsafe { libc::sigemptyset(ptr::addr_of_mut!(sigaction.sa_mask)) };

        // Is this necessary
        #[cfg(not(target_os = "aix"))]
        {
            sigaction.sa_sigaction = Self::handler as usize;
        }
        #[cfg(target_os = "aix")]
        {
            sigaction.sa_union.__su_sigaction = Self::handler;
        }

        for signal in [libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGHUP] {
            let ret = unsafe { libc::sigaction(signal, ptr::addr_of!(sigaction), ptr::null_mut()) };
            if ret != 0 {
                error!("Failed to install signal handler!")
            }
        }
        debug!("Finished setting up signal handlers")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        wallpaper::stop_animations(&self.wallpapers);

        // wait for the animation threads to finish.
        while !self.wallpapers.is_empty() {
            // When all animations finish, Arc's strong count will be exactly 1
            self.wallpapers
                .retain(|wallpaper| Arc::strong_count(wallpaper) > 1);

            // set all frame callbacks as completed, otherwise the animation threads might deadlock on
            // the conditional variable
            for wallpaper in &self.wallpapers {
                wallpaper.frame_callback_completed();
            }

            // yield to waste less cpu
            thread::yield_now();
        }

        let addr = IpcSocket::<Server>::path();
        match std::fs::remove_file(Path::new(addr)) {
            Err(err) => error!("Failed to remove socket at {addr}: {err}"),
            Ok(()) => info!("Removed socket at {addr}"),
        };
    }
}
