use std::env;
use std::marker::PhantomData;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
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
static SOCKET_PATH: OnceLock<PathBuf> = OnceLock::new();
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

    fn socket_file() -> PathBuf {
        let mut runtime = env::var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let mut p = PathBuf::from_iter(&["run", "user"]);
                let uid = rustix::process::getuid();
                p.push(format!("{}", uid.as_raw()));
                p
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
            format!("{}-swww-daemon", &wayland_socket[i..])
        } else {
            eprintln!("WARNING: WAYLAND_DISPLAY variable not set. Defaulting to wayland-0");
            "wayland-0-swww-daemon".to_string()
        };

        runtime.push(display);
        runtime
    }

    /// Retrieves path to socket file
    ///
    /// If you get errors with missing generics, you can shove any type as `T`, but
    /// [`Client`] or [`Server`] are recommended.
    #[must_use]
    pub fn path(namespace: &str) -> PathBuf {
        let mut p = SOCKET_PATH.get_or_init(Self::socket_file).clone();
        p.set_extension(format!("{namespace}.sock"));
        p
    }

    /// Retrieves all currently in-use namespaces
    pub fn all_namespaces() -> std::io::Result<Vec<String>> {
        let p = SOCKET_PATH.get_or_init(Self::socket_file).clone();
        let parent = match p.parent() {
            Some(parent) => parent,
            None => return Ok(Vec::new()),
        };

        let filename = match p.file_name() {
            Some(filename) => {
                let mut f = filename.to_os_string();
                // add a final '.' character, because the namespace is always preceded by a dot
                // character
                f.push(std::ffi::OsStr::from_bytes(b"."));
                f
            }
            None => {
                return Err(std::io::Error::other(
                    "socket path has invalid final component",
                ));
            }
        };

        let dir_entries = parent.read_dir()?;
        Ok(dir_entries
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                std::str::from_utf8(
                    entry
                        .file_name()
                        .as_encoded_bytes()
                        .strip_suffix(b".sock")?
                        .strip_prefix(filename.as_encoded_bytes())?,
                )
                .map(|e| e.to_string())
                .ok()
            })
            .collect())
    }

    #[must_use]
    pub fn as_fd(&self) -> &OwnedFd {
        &self.fd
    }
}

impl IpcSocket<Client> {
    /// Connects to already running `Daemon`, if there is one.
    pub fn connect(namespace: &str) -> Result<Self, IpcError> {
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

        let path = Self::path(namespace);
        let addr = net::SocketAddrUnix::new(&path).expect("addr is correct");

        // this will be overwritten, Rust just doesn't know it
        let mut error = Errno::INVAL;
        for _ in 0..tries {
            match net::connect(&socket, &addr) {
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
            IpcErrorKind::NoSocketFile(path)
        } else {
            IpcErrorKind::Connect
        };

        Err(error.context(kind))
    }
}

impl IpcSocket<Server> {
    /// Creates [`IpcSocket`] for use in server (i.e `Daemon`)
    pub fn server(namespace: &str) -> Result<Self, IpcError> {
        let addr = net::SocketAddrUnix::new(Self::path(namespace)).expect("addr is correct");
        let socket = net::socket_with(
            net::AddressFamily::UNIX,
            net::SocketType::STREAM,
            net::SocketFlags::CLOEXEC.union(rustix::net::SocketFlags::NONBLOCK),
            None,
        )
        .context(IpcErrorKind::Socket)?;
        net::bind(&socket, &addr).context(IpcErrorKind::Bind)?;
        net::listen(&socket, 0).context(IpcErrorKind::Listen)?;
        Ok(Self::new(socket))
    }
}
