use std::env;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use rustix::fd::OwnedFd;
use rustix::io::Errno;
use rustix::net;
use rustix::net::RecvFlags;

use super::ErrnoExt;
use super::IpcError;
use super::IpcErrorKind;
use super::Mmap;

pub struct SocketMsg {
    pub(super) code: u8,
    pub(super) shm: Option<Mmap>,
}

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

    fn socket_file() -> String {
        let runtime = env::var("XDG_RUNTIME_DIR");
        let display = env::var("WAYLAND_DISPLAY");

        let runtime = runtime.as_deref().unwrap_or("/tmp/swww");
        let display = display.as_deref().unwrap_or("wayland-0");

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

pub fn read_socket(stream: &OwnedFd) -> Result<SocketMsg, String> {
    let mut buf = [0u8; 16];
    let mut ancillary_buf = [0u8; rustix::cmsg_space!(ScmRights(1))];

    let mut control = net::RecvAncillaryBuffer::new(&mut ancillary_buf);

    let mut tries = 0;
    loop {
        let iov = rustix::io::IoSliceMut::new(&mut buf);
        match net::recvmsg(stream, &mut [iov], &mut control, RecvFlags::WAITALL) {
            Ok(_) => break,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock && tries < 5 {
                    std::thread::sleep(Duration::from_millis(1));
                } else {
                    return Err(format!("failed to read serialized length: {e}"));
                }
            }
        }
        tries += 1;
    }

    let code = u64::from_ne_bytes(buf[0..8].try_into().unwrap()) as u8;
    let len = u64::from_ne_bytes(buf[8..16].try_into().unwrap()) as usize;

    let shm = if len == 0 {
        None
    } else {
        let shm_file = match control.drain().next().unwrap() {
            net::RecvAncillaryMessage::ScmRights(mut iter) => iter.next().unwrap(),
            _ => panic!("malformed ancillary message"),
        };
        Some(Mmap::from_fd(shm_file, len))
    };
    Ok(SocketMsg { code, shm })
}

pub(super) fn send_socket_msg(
    stream: &OwnedFd,
    socket_msg: &mut [u8; 16],
    mmap: Option<&Mmap>,
) -> rustix::io::Result<bool> {
    let mut ancillary_buf = [0u8; rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = net::SendAncillaryBuffer::new(&mut ancillary_buf);

    let msg_buf;
    if let Some(mmap) = mmap.as_ref() {
        socket_msg[8..].copy_from_slice(&(mmap.len() as u64).to_ne_bytes());
        msg_buf = [mmap.fd()];
        let msg = net::SendAncillaryMessage::ScmRights(&msg_buf);
        ancillary.push(msg);
    }

    let iov = rustix::io::IoSlice::new(&socket_msg[..]);
    net::sendmsg(stream, &[iov], &mut ancillary, net::SendFlags::empty())
        .map(|written| written == socket_msg.len())
}

#[must_use]
pub fn get_socket_path() -> PathBuf {
    IpcSocket::<Client>::path().into()
}

/// We make sure the Stream is always set to blocking mode
///
/// * `tries` -  how many times to attempt the connection
/// * `interval` - how long to wait between attempts, in milliseconds
pub fn connect_to_socket(_: &PathBuf, _: u8, _: u64) -> Result<OwnedFd, String> {
    IpcSocket::connect()
        .map(|socket| socket.fd)
        .map_err(|err| err.to_string())
}
