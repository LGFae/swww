//! Implementation of the Wayland Wire Protocol
//!
//! There are some things that are specific for `swww-daemon` (for example, our ancillary buffer
//! for receiving socket messages is always empty, since none of the events we care about have file
//! descriptors), but I tried to actually make it fairly complete. This means types like `WlFixed`
//! exist even if they aren't used at all in the rest of the codebase.

use rustix::{
    fd::{AsRawFd, BorrowedFd, OwnedFd},
    io, net,
};
use std::num::NonZeroU32;

use super::{globals::wayland_fd, ObjectId};

#[derive(Debug, Clone)]
pub struct WaylandPayload(Box<[u32]>);

#[derive(Debug)]
pub struct WireMsg {
    sender_id: ObjectId,
    op: u16,
    fds: Box<[OwnedFd]>,
    cur: u16, // the message is at most 1 << 15 bytes long
}

#[derive(Debug, Clone)]
pub struct WlSlice<'a>(&'a [u8]);

#[derive(Debug, Clone)]
pub struct WlStr<'a>(&'a str);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WlFixed(i32);

#[derive(Debug, Clone)]
pub struct NewId<'a> {
    id: ObjectId,
    interface: &'a str,
    version: u32,
}

impl WaylandPayload {
    #[must_use]
    pub const fn get(&self) -> &[u32] {
        &self.0
    }
}

impl WireMsg {
    pub fn recv() -> rustix::io::Result<(Self, WaylandPayload)> {
        let fds = Vec::new();

        let mut header_buf = [0u32; 2];

        // we don't need these because no events we care about send file descriptors
        let mut ancillary_buf = [0; 0];
        let mut control = net::RecvAncillaryBuffer::new(i32_slice_to_u8_mut(&mut ancillary_buf));

        let iov = io::IoSliceMut::new(u32_slice_to_u8_mut(&mut header_buf));
        net::recvmsg(
            wayland_fd(),
            &mut [iov],
            &mut control,
            net::RecvFlags::empty(),
        )?;

        let sender_id = ObjectId(
            NonZeroU32::new(header_buf[0])
                .expect("received a message from compositor with a null sender id"),
        );
        let size = (header_buf[1] >> 16) as usize - 8;
        let op = (header_buf[1] & 0xFFFF) as u16;

        let mut payload = vec![0u32; size >> 2];

        if size > 0 {
            // this should not fail with INTR, because otherwise our socket's internal buffer will
            // be left in an inconsistent state (a message without a header)
            rustix::io::retry_on_intr(|| {
                let iov = io::IoSliceMut::new(u32_slice_to_u8_mut(&mut payload));
                net::recvmsg(
                    wayland_fd(),
                    &mut [iov],
                    &mut control,
                    net::RecvFlags::WAITALL,
                )
            })?;
        }

        Ok((
            Self {
                sender_id,
                op,
                fds: fds.into_boxed_slice(),
                cur: 0,
            },
            WaylandPayload(payload.into_boxed_slice()),
        ))
    }

    #[must_use]
    pub fn into_fds(self) -> Box<[OwnedFd]> {
        self.fds
    }

    #[must_use]
    pub const fn sender_id(&self) -> ObjectId {
        ObjectId(self.sender_id.0)
    }

    #[must_use]
    pub const fn op(&self) -> u16 {
        self.op
    }

    #[must_use]
    pub fn next_i32(&mut self, payload: &WaylandPayload) -> i32 {
        self.cur += 1;
        payload.get()[self.cur as usize - 1] as i32
    }

    #[must_use]
    pub fn next_u32(&mut self, payload: &WaylandPayload) -> u32 {
        self.cur += 1;
        payload.get()[self.cur as usize - 1]
    }

    #[must_use]
    pub fn next_fixed(&mut self, payload: &WaylandPayload) -> WlFixed {
        self.cur += 1;
        WlFixed::from(payload.get()[self.cur as usize - 1])
    }

    #[must_use]
    pub fn next_string<'a>(&mut self, payload: &'a WaylandPayload) -> &'a str {
        let len = payload.get()[self.cur as usize] as usize;

        unsafe {
            // remember to skip the length
            let ptr = payload.get().as_ptr().add(self.cur as usize + 1);

            // the len sent by the protocol includes the '0', but we do not need it
            let cast = std::slice::from_raw_parts(ptr.cast(), len - 1);
            self.cur += 1 + ((len + 3) >> 2) as u16;
            std::str::from_utf8(cast).expect("string received is not valid utf8")
        }
    }

    #[must_use]
    pub fn next_object(&mut self, payload: &WaylandPayload) -> Option<ObjectId> {
        self.cur += 1;
        NonZeroU32::new(payload.get()[self.cur as usize - 1]).map(ObjectId)
    }

    #[must_use]
    pub fn next_new_specified_id(&mut self, payload: &WaylandPayload) -> ObjectId {
        self.next_object(payload).unwrap()
    }

    #[must_use]
    pub fn next_new_unspecified_id<'a>(&mut self, payload: &'a WaylandPayload) -> NewId<'a> {
        let interface = self.next_string(payload);
        let version = self.next_u32(payload);
        let id = self.next_new_specified_id(payload);

        NewId {
            id,
            interface,
            version,
        }
    }

    #[must_use]
    pub fn next_array<'a>(&mut self, payload: &'a WaylandPayload) -> &'a [u8] {
        let len = payload.get()[self.cur as usize] as usize;

        unsafe {
            let ptr = payload.get().as_ptr().add(self.cur as usize + 1); // skip the length
            self.cur += 1 + ((len + 3) >> 2) as u16;
            std::slice::from_raw_parts(ptr.cast(), len)
        }
    }
}

pub struct WireMsgBuilder {
    msg: Vec<u32>,
    fds: Vec<i32>,
}

impl WireMsgBuilder {
    #[must_use]
    pub fn new(sender_id: ObjectId, op: u16) -> Self {
        let msg = vec![sender_id.get(), op as u32];
        Self {
            msg,
            fds: Vec::new(),
        }
    }

    pub fn add_i32(&mut self, i: i32) {
        self.msg.push(i as u32);
    }

    pub fn add_u32(&mut self, u: u32) {
        self.msg.push(u);
    }

    pub fn add_fixed(&mut self, fixed: WlFixed) {
        self.msg.push(fixed.0 as u32)
    }

    pub fn add_string(&mut self, s: &str) {
        WlStr(s).encode(&mut self.msg);
    }

    pub fn add_object(&mut self, object_id: Option<ObjectId>) {
        match object_id {
            Some(id) => self.msg.push(id.get()),
            None => self.msg.push(0),
        }
    }

    pub fn add_new_specified_id(&mut self, object_id: ObjectId) {
        self.msg.push(object_id.get());
    }

    pub fn add_new_unspecified_id(&mut self, object_id: ObjectId, interface: &str, version: u32) {
        self.add_string(interface);
        self.add_u32(version);
        self.add_new_specified_id(object_id);
    }

    pub fn add_array(&mut self, array: &[u8]) {
        WlSlice(array).encode(&mut self.msg);
    }

    pub fn add_fd<'a, 'b: 'a>(&'a mut self, fd: &'b impl AsRawFd) {
        self.fds.push(fd.as_raw_fd());
    }

    pub fn send(self) -> rustix::io::Result<()> {
        let Self { mut msg, fds } = self;
        let len = msg.len() << 2;
        // put the correct length in the upper part of the header's second word
        msg[1] |= (len as u32) << 16;

        let mut borrowed_fds = Vec::with_capacity(fds.len());
        for fd in fds {
            borrowed_fds.push(unsafe { BorrowedFd::borrow_raw(fd) });
        }
        unsafe { send_unchecked(u32_slice_to_u8(&msg), &borrowed_fds) }
    }
}

/// try to send a raw message through the wayland socket. We do no input validation whatsoever
pub unsafe fn send_unchecked(msg: &[u8], fds: &[BorrowedFd]) -> rustix::io::Result<()> {
    let iov = io::IoSlice::new(msg);
    let mut control_buf = [0u8; rustix::cmsg_space!(ScmRights(1))];
    let mut control = net::SendAncillaryBuffer::new(&mut control_buf);
    let msg = net::SendAncillaryMessage::ScmRights(fds);
    control.push(msg);
    net::sendmsg(wayland_fd(), &[iov], &mut control, net::SendFlags::NOSIGNAL).map(|_| ())
}

impl<'a> WlSlice<'a> {
    #[must_use]
    pub const fn get(&self) -> &[u8] {
        self.0
    }

    pub fn encode(&self, buf: &mut Vec<u32>) {
        let len = self.0.len().next_multiple_of(4);
        buf.push(len as u32);
        buf.reserve(len >> 2);
        unsafe {
            // dst is the next free position in the buffer
            let dst = buf.as_ptr().add(buf.len()) as *mut u8;

            // copy all the bytes
            // SAFETY: we've ensured the buf's pointer has the necessary size above
            std::ptr::copy_nonoverlapping(self.0.as_ptr(), dst, self.0.len());

            // 0 initialize the padding
            for i in self.0.len()..len {
                dst.add(i).write(0);
            }

            // set the len to the values we've just written
            buf.set_len(buf.len() + (len >> 2));
        }
    }
}

impl<'a, 'b> From<&'b [u8]> for WlSlice<'a>
where
    'b: 'a,
{
    #[must_use]
    fn from(bytes: &'b [u8]) -> Self {
        Self(bytes)
    }
}

impl<'a> WlStr<'a> {
    #[must_use]
    pub const fn get(&self) -> &str {
        self.0
    }

    pub fn encode(&self, buf: &mut Vec<u32>) {
        let bytes = self.0.as_bytes();
        // add one for the null terminator
        let len = (bytes.len() + 1).next_multiple_of(4);
        buf.push(len as u32);
        buf.reserve(len >> 2);
        unsafe {
            // dst is the next free position in the buffer
            let dst = buf.as_ptr().add(buf.len()) as *mut u8;

            // copy all the bytes
            // SAFETY: we've ensured the buf's pointer has the necessary size above
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());

            // 0 initialize the padding and the null terminator
            for i in bytes.len()..len {
                dst.add(i).write(0);
            }

            // set the len to the values we've just written
            buf.set_len(buf.len() + (len >> 2));
        }
    }
}

impl<'a, 'b> From<&'b str> for WlStr<'a>
where
    'b: 'a,
{
    #[must_use]
    fn from(s: &'b str) -> Self {
        Self(s)
    }
}

impl From<i32> for WlFixed {
    #[must_use]
    fn from(value: i32) -> Self {
        Self(value * 256)
    }
}

impl From<u32> for WlFixed {
    #[must_use]
    fn from(value: u32) -> Self {
        Self(value as i32 * 256)
    }
}

impl From<&WlFixed> for i32 {
    #[must_use]
    fn from(val: &WlFixed) -> Self {
        val.0 / 256
    }
}

impl From<f64> for WlFixed {
    #[must_use]
    fn from(value: f64) -> Self {
        let d = value + (3i64 << (51 - 8)) as f64;
        Self(d.to_bits() as i32)
    }
}

impl From<&WlFixed> for f64 {
    #[must_use]
    fn from(val: &WlFixed) -> Self {
        let i = ((1023i64 + 44i64) << 52) + (1i64 << 51) + val.0 as i64;
        let d = f64::from_bits(i as u64);
        d - (3i64 << 43) as f64
    }
}

impl<'a> NewId<'a> {
    #[must_use]
    pub const fn id(&self) -> &ObjectId {
        &self.id
    }

    #[must_use]
    pub const fn interface(&self) -> &str {
        self.interface
    }

    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }
}

const fn u32_slice_to_u8(src: &[u32]) -> &[u8] {
    let len = src.len() << 2;
    unsafe { std::slice::from_raw_parts(src.as_ptr() as *mut u8, len) }
}

fn u32_slice_to_u8_mut(src: &mut [u32]) -> &mut [u8] {
    let len = src.len() << 2;
    unsafe { std::slice::from_raw_parts_mut(src.as_mut_ptr() as *mut u8, len) }
}

fn i32_slice_to_u8_mut(src: &mut [i32]) -> &mut [u8] {
    let len = src.len() << 2;
    unsafe { std::slice::from_raw_parts_mut(src.as_mut_ptr() as *mut u8, len) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_creation() {
        assert_eq!(WlFixed::from(-1), WlFixed::from(0xFFFFFFFFu32));
    }

    #[test]
    fn slice_encoding() {
        let mut buf = Vec::new();

        let arr = [1u8, 2, 3, 4, 5, 6, 7];
        WlSlice::from(arr.as_ref()).encode(&mut buf);

        #[cfg(target_endian = "little")]
        let expected = vec![8, 0x04030201u32, 0x00070605];

        #[cfg(target_endian = "big")]
        let expected = vec![8, 0x01020304u32, 0x05060700];

        assert_eq!(buf, expected);
        buf.clear();

        let arr = [1u8, 2, 3, 4];
        WlSlice::from(arr.as_ref()).encode(&mut buf);

        #[cfg(target_endian = "little")]
        let expected = vec![4, 0x04030201u32];

        #[cfg(target_endian = "big")]
        let expected = vec![4, 0x01020304u32];

        assert_eq!(buf, expected);
    }

    #[test]
    fn str_encoding() {
        let mut buf = Vec::new();

        WlStr::from("hello world").encode(&mut buf);

        #[cfg(target_endian = "little")]
        let expected = vec![12, 0x6C6C6568u32, 0x6F77206F, 0x00646C72];

        #[cfg(target_endian = "big")]
        let expected = vec![12, 0x06865C6Cu32, 0x6F20776F, 0x726C6400];

        assert_eq!(buf, expected);
        buf.clear();

        WlStr::from("hell").encode(&mut buf);
        #[cfg(target_endian = "little")]
        let expected = vec![8, 0x6C6C6568u32, 0];

        #[cfg(target_endian = "big")]
        let expected = vec![8, 0x06865C6Cu32, 0];

        assert_eq!(buf, expected);
    }
}
