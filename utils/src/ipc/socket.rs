use std::{path::PathBuf, time::Duration};

use rustix::{
    fd::OwnedFd,
    net::{self, RecvFlags},
};

use super::Mmap;

pub struct SocketMsg {
    pub(super) code: u8,
    pub(super) shm: Option<Mmap>,
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
    let runtime_dir = if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        dir
    } else {
        let uid = rustix::process::getuid();
        format!("/run/user/{}", uid.as_raw())
    };

    let mut socket_path = PathBuf::from(runtime_dir);

    if let Ok(wayland_socket) = std::env::var("WAYLAND_DISPLAY") {
        let mut i = 0;
        // if WAYLAND_DISPLAY is a full path, use only its final component
        for (j, ch) in wayland_socket.bytes().enumerate().rev() {
            if ch == b'/' {
                i = j + 1;
                break;
            }
        }
        socket_path.push(format!("{}.sock", &wayland_socket[i..]));
    } else {
        eprintln!("WARNING: WAYLAND_DISPLAY variable not set. Defaulting to wayland-0");
        socket_path.push("wayland-0.sock");
    }

    socket_path
}

/// We make sure the Stream is always set to blocking mode
///
/// * `tries` -  how many times to attempt the connection
/// * `interval` - how long to wait between attempts, in milliseconds
pub fn connect_to_socket(addr: &PathBuf, tries: u8, interval: u64) -> Result<OwnedFd, String> {
    let socket = rustix::net::socket_with(
        rustix::net::AddressFamily::UNIX,
        rustix::net::SocketType::STREAM,
        rustix::net::SocketFlags::CLOEXEC,
        None,
    )
    .expect("failed to create socket file descriptor");
    let addr = net::SocketAddrUnix::new(addr).unwrap();
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
                if let Err(e) = net::sockopt::set_socket_timeout(
                    &socket,
                    net::sockopt::Timeout::Recv,
                    Some(timeout),
                ) {
                    return Err(format!("failed to set read timeout for socket: {e}"));
                }

                return Ok(socket);
            }
            Err(e) => error = Some(e),
        }
        std::thread::sleep(Duration::from_millis(interval));
    }
    let error = error.unwrap();
    if error.kind() == std::io::ErrorKind::NotFound {
        return Err("Socket file not found. Are you sure swww-daemon is running?".to_string());
    }

    Err(format!("Failed to connect to socket: {error}"))
}
