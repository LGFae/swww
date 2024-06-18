use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::anyhow;
use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use rustix::fd::OwnedFd;
use rustix::net;
use rustix::net::RecvFlags;

use super::Mmap;

pub struct SocketMsg {
    pub code: u8,
    pub shm: Option<Mmap>,
}

pub struct Socket {
    fd: OwnedFd,
}

static PATH: OnceLock<String> = OnceLock::new();

impl Socket {
    fn socket_file() -> String {
        let runtime = env::var("XDG_RUNTIME_DIR");
        let display = env::var("WAYLAND_DISPLAY");

        let runtime = runtime.as_deref().unwrap_or("/tmp/swww");
        let display = display.as_deref().unwrap_or("wayland-0");

        format!("{runtime}/swww-{display}.socket")
    }

    #[must_use]
    pub fn path() -> &'static str {
        PATH.get_or_init(Self::socket_file)
    }

    #[must_use]
    pub fn as_fd(&self) -> &OwnedFd {
        &self.fd
    }

    pub fn connect() -> Result<Self> {
        Self::connect_configured(5, 100)
    }

    /// We make sure the Stream is always set to blocking mode
    ///
    /// * `tries` -  how many times to attempt the connection
    /// * `interval` - how long to wait between attempts, in milliseconds
    pub fn connect_configured(tries: u8, interval: u64) -> Result<Self> {
        let socket = net::socket_with(
            net::AddressFamily::UNIX,
            net::SocketType::STREAM,
            net::SocketFlags::CLOEXEC,
            None,
        )
        .context("failed to create socket file descriptor")?;

        let addr = net::SocketAddrUnix::new(Self::path()).expect("addr is correct");

        //Make sure we try at least once
        let tries = if tries == 0 { 1 } else { tries };
        let mut error = None;
        for _ in 0..tries {
            match net::connect_unix(&socket, &addr) {
                Ok(()) => {
                    #[cfg(debug_assertions)]
                    let timeout = Duration::from_secs(30); //Some operations take a while to respond in debug mode
                    #[cfg(not(debug_assertions))]
                    let timeout = Duration::from_secs(5);
                    return match net::sockopt::set_socket_timeout(
                        &socket,
                        net::sockopt::Timeout::Recv,
                        Some(timeout),
                    ) {
                        Ok(()) => Ok(Self { fd: socket }),
                        Err(e) => bail!("failed to set read timeout for socket: {e}"),
                    };
                }
                Err(e) => error = Some(e),
            }
            std::thread::sleep(Duration::from_millis(interval));
        }

        let error = error.expect("error must have ocurred");
        if error.kind() == std::io::ErrorKind::NotFound {
            bail!("Socket file not found. Are you sure swww-daemon is running?");
        }

        Err(anyhow!("Failed to connect to socket")).context(error)
    }

    pub fn read(&self) -> Result<SocketMsg> {
        let mut buf = [0u8; 16];
        let mut ancillary_buf = [0u8; rustix::cmsg_space!(ScmRights(1))];

        let mut control = net::RecvAncillaryBuffer::new(&mut ancillary_buf);

        let mut tries = 0;
        loop {
            let iov = rustix::io::IoSliceMut::new(&mut buf);
            match net::recvmsg(&self.fd, &mut [iov], &mut control, RecvFlags::WAITALL) {
                Ok(_) => break,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock && tries < 5 {
                        std::thread::sleep(Duration::from_millis(1));
                    } else {
                        bail!("failed to read serialized length: {e}");
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
}

impl From<OwnedFd> for Socket {
    fn from(value: OwnedFd) -> Self {
        Self { fd: value }
    }
}

pub(super) fn send_socket_msg(
    stream: &OwnedFd,
    socket_msg: &mut [u8; 16],
    mmap: Option<&Mmap>,
) -> rustix::io::Result<bool> {
    let mut ancillary_buf = [0u8; rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = net::SendAncillaryBuffer::new(&mut ancillary_buf);

    let msg_buf;
    if let Some(mmap) = mmap {
        socket_msg[8..].copy_from_slice(&(mmap.len() as u64).to_ne_bytes());
        msg_buf = [mmap.fd()];
        let msg = net::SendAncillaryMessage::ScmRights(&msg_buf);
        ancillary.push(msg);
    }

    let iov = rustix::io::IoSlice::new(&socket_msg[..]);
    net::sendmsg(stream, &[iov], &mut ancillary, net::SendFlags::empty())
        .map(|written| written == socket_msg.len())
}
