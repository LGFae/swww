use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use smithay_client_toolkit::shm::{raw::RawPool, Shm};
use wayland_client::{protocol::wl_buffer::WlBuffer, QueueHandle};

use crate::Daemon;

#[derive(Debug)]
struct Buffer {
    inner: WlBuffer,
    released: Arc<AtomicBool>,
}

impl Buffer {
    fn new(inner: WlBuffer, released: Arc<AtomicBool>) -> Self {
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
    pool: RawPool,
    buffers: Vec<Buffer>,
    width: i32,
    height: i32,
    last_used_buffer: Option<usize>,
}

impl BumpPool {
    /// We assume `width` and `height` have already been multiplied by their scale factor
    pub(crate) fn new(width: i32, height: i32, shm: &Shm, qh: &QueueHandle<Daemon>) -> Self {
        let len = width as usize * height as usize * 4;
        let mut pool = RawPool::new(len, shm).expect("failed to create RawPool");
        let released = Arc::new(AtomicBool::new(true));
        let buffers = vec![Buffer::new(
            pool.create_buffer(
                0,
                width,
                height,
                width * 4,
                crate::wl_shm_format(),
                released.clone(),
                qh,
            ),
            released,
        )];

        Self {
            pool,
            buffers,
            width,
            height,
            last_used_buffer: None,
        }
    }

    #[inline]
    fn buffer_len(&self) -> usize {
        self.width as usize * self.height as usize * 4
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
    fn grow(&mut self, qh: &QueueHandle<Daemon>) {
        //TODO: CHECK IF WE HAVE SIZE
        let len = self.buffer_len();
        self.pool
            .resize(self.occupied_bytes() + len)
            .expect("failed to resize RawPool");
        let released = Arc::new(AtomicBool::new(true));
        let new_buffer_index = self.buffers.len();
        self.buffers.push(Buffer::new(
            self.pool.create_buffer(
                self.buffer_offset(new_buffer_index).try_into().unwrap(),
                self.width,
                self.height,
                self.width * 4,
                crate::wl_shm_format(),
                released.clone(),
                qh,
            ),
            released,
        ));
        log::info!(
            "BumpPool with: {} buffers. Size: {}Kb",
            self.buffers.len(),
            self.pool.len() / 1024
        );
    }

    /// Returns a drawable surface. If we can't find a free buffer, we request more memory
    ///
    /// This function automatically handles copying the previous buffer over onto the new one
    pub(crate) fn get_drawable(&mut self, qh: &QueueHandle<Daemon>) -> &mut [u8] {
        let (i, buf) = match self
            .buffers
            .iter()
            .enumerate()
            .find(|(_, b)| b.released.load(Ordering::Acquire))
        {
            Some((i, buf)) => (i, buf),
            None => {
                self.grow(qh);
                (self.buffers.len() - 1, self.buffers.last().unwrap())
            }
        };

        let len = self.buffer_len();
        let offset = self.buffer_offset(i);
        buf.released.store(false, Ordering::Release);

        if let Some(i) = self.last_used_buffer {
            let last_offset = self.buffer_offset(i);
            self.pool
                .mmap()
                .copy_within(last_offset..last_offset + len, offset);
        }
        self.last_used_buffer = Some(i);

        &mut self.pool.mmap()[offset..offset + len]
    }

    /// gets the last buffer we've drawn to
    ///
    /// This may return None if there was a resize request in-between the last call to get_drawable
    #[inline]
    pub(crate) fn get_commitable_buffer(&self) -> Option<&WlBuffer> {
        self.last_used_buffer.map(|i| &self.buffers[i].inner)
    }

    /// We assume `width` and `height` have already been multiplied by their scale factor
    pub(crate) fn resize(&mut self, width: i32, height: i32, qh: &QueueHandle<Daemon>) {
        self.width = width;
        self.height = height;
        self.last_used_buffer = None;
        self.buffers.clear();
        let released = Arc::new(AtomicBool::new(true));
        self.buffers.push(Buffer::new(
            self.pool.create_buffer(
                0,
                width,
                height,
                width * 4,
                crate::wl_shm_format(),
                released.clone(),
                qh,
            ),
            released,
        ));
    }
}
