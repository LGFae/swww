use std::{
    os::unix::prelude::OwnedFd,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use utils::ipc::Mmap;
use wayland_client::{
    backend::ObjectData,
    protocol::{
        wl_buffer::WlBuffer,
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
    },
    Proxy, WEnum,
};

#[derive(Debug)]
struct ReleaseFlag(AtomicBool);

impl ReleaseFlag {
    fn is_released(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn set_released(&self) {
        self.0.store(true, Ordering::Release)
    }

    fn unset_released(&self) {
        self.0.store(false, Ordering::Release)
    }
}

impl ObjectData for ReleaseFlag {
    fn event(
        self: Arc<Self>,
        _: &wayland_client::backend::Backend,
        msg: wayland_client::backend::protocol::Message<wayland_client::backend::ObjectId, OwnedFd>,
    ) -> Option<Arc<(dyn ObjectData + 'static)>> {
        if msg.opcode == wayland_client::protocol::wl_buffer::Event::Release.opcode() {
            self.set_released();
        }

        None
    }

    fn destroyed(&self, _: wayland_client::backend::ObjectId) {}
}

#[derive(Debug)]
struct Buffer {
    inner: WlBuffer,
    released: Arc<ReleaseFlag>,
}

impl Buffer {
    fn new(
        pool: &WlShmPool,
        offset: i32,
        width: i32,
        height: i32,
        stride: i32,
        format: wl_shm::Format,
    ) -> Self {
        let released = Arc::new(ReleaseFlag(AtomicBool::new(true)));
        let inner = pool
            .send_constructor(
                wl_shm_pool::Request::CreateBuffer {
                    offset,
                    width,
                    height,
                    stride,
                    format: WEnum::Value(format),
                },
                released.clone(),
            )
            .expect("WlShmPool failed to create buffer");
        Self { inner, released }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        self.inner.destroy();
    }
}

#[derive(Debug)]
/// A pool implementation that only gives buffers of a fixed size, creating new ones if none of
/// them are freed. It also takes care of copying the previous buffer's content over to the new one
/// for us
pub(crate) struct BumpPool {
    pool: WlShmPool,
    mmap: Mmap,

    buffers: Vec<Buffer>,
    width: i32,
    height: i32,
    last_used_buffer: Option<usize>,
}

impl BumpPool {
    /// We assume `width` and `height` have already been multiplied by their scale factor
    pub(crate) fn new(width: i32, height: i32, shm: &WlShm) -> Self {
        let len = width as usize * height as usize * crate::pixel_format().channels() as usize;
        let (pool, mmap) = new_pool(len, shm);
        let buffers = vec![];

        Self {
            pool,
            mmap,
            buffers,
            width,
            height,
            last_used_buffer: None,
        }
    }

    #[inline]
    fn buffer_len(&self) -> usize {
        self.width as usize * self.height as usize * crate::pixel_format().channels() as usize
    }

    #[inline]
    fn buffer_offset(&self, buffer_index: usize) -> usize {
        self.buffer_len() * buffer_index
    }

    #[inline]
    fn occupied_bytes(&self) -> usize {
        self.buffer_offset(self.buffers.len())
    }

    /// resizes the pool and creates a new WlBuffer at the next free offset
    fn grow(&mut self) {
        //TODO: CHECK IF WE HAVE SIZE
        let len = self.buffer_len();

        let new_len = self.occupied_bytes() + len;
        if new_len > self.mmap.len() {
            self.mmap.remap(new_len);
            self.pool.resize(new_len as i32);
        }

        let new_buffer_index = self.buffers.len();
        self.buffers.push(Buffer::new(
            &self.pool,
            self.buffer_offset(new_buffer_index).try_into().unwrap(),
            self.width,
            self.height,
            self.width * crate::pixel_format().channels() as i32,
            crate::wl_shm_format(),
        ));

        log::info!(
            "BumpPool with: {} buffers. Size: {}Kb",
            self.buffers.len(),
            self.mmap.len() / 1024
        );
    }

    /// Returns a drawable surface. If we can't find a free buffer, we request more memory
    ///
    /// This function automatically handles copying the previous buffer over onto the new one
    pub(crate) fn get_drawable(&mut self) -> &mut [u8] {
        let (i, buf) = match self
            .buffers
            .iter()
            .enumerate()
            .find(|(_, b)| b.released.is_released())
        {
            Some((i, buf)) => (i, buf),
            None => {
                self.grow();
                (self.buffers.len() - 1, self.buffers.last().unwrap())
            }
        };

        let len = self.buffer_len();
        let offset = self.buffer_offset(i);
        buf.released.unset_released();

        if let Some(i) = self.last_used_buffer {
            let last_offset = self.buffer_offset(i);
            self.mmap
                .slice_mut()
                .copy_within(last_offset..last_offset + len, offset);
        }
        self.last_used_buffer = Some(i);

        &mut self.mmap.slice_mut()[offset..offset + len]
    }

    /// gets the last buffer we've drawn to
    ///
    /// This may return None if there was a resize request in-between the last call to get_drawable
    #[inline]
    pub(crate) fn get_commitable_buffer(&self) -> Option<&WlBuffer> {
        self.last_used_buffer.map(|i| &self.buffers[i].inner)
    }

    /// We assume `width` and `height` have already been multiplied by their scale factor
    #[inline]
    pub(crate) fn resize(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
        self.last_used_buffer = None;
        self.buffers.clear();
    }
}

impl Drop for BumpPool {
    fn drop(&mut self) {
        self.pool.destroy();
    }
}

fn new_pool(len: usize, shm: &WlShm) -> (WlShmPool, Mmap) {
    let mmap = Mmap::create(len);

    let pool = shm
        .send_constructor(
            wl_shm::Request::CreatePool {
                fd: mmap.fd(),
                size: len as i32,
            },
            Arc::new(ShmPoolData),
        )
        .expect("failed to create WlShmPool object");

    (pool, mmap)
}

#[derive(Debug)]
struct ShmPoolData;

impl ObjectData for ShmPoolData {
    fn event(
        self: Arc<Self>,
        _: &wayland_client::backend::Backend,
        _: wayland_client::backend::protocol::Message<wayland_client::backend::ObjectId, OwnedFd>,
    ) -> Option<Arc<(dyn ObjectData + 'static)>> {
        unreachable!("wl_shm_pool has no events")
    }

    fn destroyed(&self, _: wayland_client::backend::ObjectId) {}
}
