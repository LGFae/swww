use std::env;
use std::marker::PhantomData;
use std::sync::OnceLock;
use std::time::Duration;

use rustix::fd::OwnedFd;
use rustix::io::Errno;
use rustix::net;

use super::ErrnoExt;
use super::IpcError;
use super::IpcErrorKind;

/// Represents client in IPC communication, via typestate pattern in [`IpcSocket`]
pub struct Client;
/// Represents server in IPC communication, via typestate pattern in [`IpcSocket`]
pub struct Server;

/// Typesafe handle for socket facilitating communication between [`Client`] and [`Server`]
pub struct IpcSocket<T> {
    fd: OwnedFd,
    phantom: PhantomData<T>,
}

impl<T> IpcSocket<T> {
    /// Creates new [`IpcSocket`] from provided [`OwnedFd`]
    ///
    /// TODO: remove external ability to construct [`Self`] from random file descriptors
    pub fn new(fd: OwnedFd) -> Self {
        Self {
            fd,
            phantom: PhantomData,
        }
    }

    pub fn to_fd(self) -> OwnedFd {
        self.fd
    }

    fn socket_file() -> String {
        let runtime = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
            let uid = rustix::process::getuid();
            format!("/run/user/{}", uid.as_raw())
        });

        let display = if let Ok(wayland_socket) = std::env::var("WAYLAND_DISPLAY") {
            let mut i = 0;
            // if WAYLAND_DISPLAY is a full path, use only its final component
            for (j, ch) in wayland_socket.bytes().enumerate().rev() {
                if ch == b'/' {
                    i = j + 1;
                    break;
                }
            }
            format!("{}.sock", &wayland_socket[i..])
        } else {
            eprintln!("WARNING: WAYLAND_DISPLAY variable not set. Defaulting to wayland-0");
            "wayland-0.sock".to_string()
        };

        format!("{runtime}/swww-{display}.socket")
    }

    /// Retreives path to socket file
    ///
    /// To treat this as filesystem path, wrap it in [`Path`].
    /// If you get errors with missing generics, you can shove any type as `T`, but
    /// [`Client`] or [`Server`] are recommended.
    ///
    /// [`Path`]: std::path::Path
    #[must_use]
    pub fn path() -> &'static str {
        static PATH: OnceLock<String> = OnceLock::new();
        PATH.get_or_init(Self::socket_file)
    }

    #[must_use]
    pub fn as_fd(&self) -> &OwnedFd {
        &self.fd
    }
}

impl IpcSocket<Client> {
    /// Connects to already running `Daemon`, if there is one.
    pub fn connect() -> Result<Self, IpcError> {
        // these were hardcoded everywhere, no point in passing them around
        let tries = 5;
        let interval = 100;

        let socket = net::socket_with(
            net::AddressFamily::UNIX,
            net::SocketType::STREAM,
            net::SocketFlags::CLOEXEC,
            None,
        )
        .context(IpcErrorKind::Socket)?;

        let addr = net::SocketAddrUnix::new(Self::path()).expect("addr is correct");

        // this will be overwriten, Rust just doesn't know it
        let mut error = Errno::INVAL;
        for _ in 0..tries {
            match net::connect_unix(&socket, &addr) {
                Ok(()) => {
                    #[cfg(debug_assertions)]
                    let timeout = Duration::from_secs(30); //Some operations take a while to respond in debug mode
                    #[cfg(not(debug_assertions))]
                    let timeout = Duration::from_secs(5);
                    return net::sockopt::set_socket_timeout(
                        &socket,
                        net::sockopt::Timeout::Recv,
                        Some(timeout),
                    )
                    .context(IpcErrorKind::SetTimeout)
                    .map(|()| Self::new(socket));
                }
                Err(e) => error = e,
            }
            std::thread::sleep(Duration::from_millis(interval));
        }

        let kind = if error.kind() == std::io::ErrorKind::NotFound {
            IpcErrorKind::NoSocketFile
        } else {
            IpcErrorKind::Connect
        };

        Err(error.context(kind))
    }
}

impl IpcSocket<Server> {
    /// Creates [`IpcSocket`] for use in server (i.e `Daemon`)
    pub fn server() -> Result<Self, IpcError> {
        let addr = net::SocketAddrUnix::new(Self::path()).expect("addr is correct");
        let socket = net::socket_with(
            net::AddressFamily::UNIX,
            net::SocketType::STREAM,
            net::SocketFlags::CLOEXEC.union(rustix::net::SocketFlags::NONBLOCK),
            None,
        )
        .context(IpcErrorKind::Socket)?;
        net::bind_unix(&socket, &addr).context(IpcErrorKind::Bind)?;
        net::listen(&socket, 0).context(IpcErrorKind::Listen)?;
        Ok(Self::new(socket))
    }
}
