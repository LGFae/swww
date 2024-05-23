use std::sync::atomic::{AtomicBool, Ordering};

use utils::ipc::Mmap;

use super::{globals, ObjectId};

#[derive(Debug)]
pub struct ReleaseFlag(AtomicBool);

impl ReleaseFlag {
    fn is_released(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn set_released(&self) {
        self.0.store(true, Ordering::Release)
    }

    fn unset_released(&self) {
        self.0.store(false, Ordering::Release)
    }
}

#[derive(Debug)]
struct Buffer {
    object_id: ObjectId,
    released: ReleaseFlag,
}

impl Buffer {
    fn new(
        pool_id: ObjectId,
        offset: i32,
        width: i32,
        height: i32,
        stride: i32,
        format: u32,
    ) -> Self {
        let released = ReleaseFlag(AtomicBool::new(true));

        let object_id = globals::object_create(super::WlDynObj::Buffer);
        super::interfaces::wl_shm_pool::req::create_buffer(
            pool_id, object_id, offset, width, height, stride, format,
        )
        .expect("WlShmPool failed to create buffer");
        Self {
            object_id,
            released,
        }
    }

    fn destroy(self) {
        if let Err(e) = super::interfaces::wl_buffer::req::destroy(self.object_id) {
            log::error!("failed to destroy wl_buffer: {e:?}");
        }
    }
}

#[derive(Debug)]
/// A pool implementation that only gives buffers of a fixed size, creating new ones if none of
/// them are freed. It also takes care of copying the previous buffer's content over to the new one
/// for us.
///
/// Current implementation will automatically unmap the underlying shared memory when we aren't
/// animating and all created buffers have been released
pub(crate) struct BumpPool {
    pool_id: ObjectId,
    mmap: Mmap,
    buffers: Vec<Buffer>,
    width: i32,
    height: i32,
    last_used_buffer: usize,
}

impl BumpPool {
    /// We assume `width` and `height` have already been multiplied by their scale factor
    pub(crate) fn new(width: i32, height: i32) -> Self {
        let len =
            width as usize * height as usize * super::globals::pixel_format().channels() as usize;
        let mmap = Mmap::create(len);
        let pool_id = globals::object_create(super::WlDynObj::ShmPool);
        super::interfaces::wl_shm::req::create_pool(pool_id, &mmap.fd(), len as i32)
            .expect("failed to create WlShmPool object");
        let buffers = Vec::with_capacity(2);

        Self {
            pool_id,
            mmap,
            buffers,
            width,
            height,
            last_used_buffer: 0,
        }
    }

    /// Releases a buffer, if we have it
    ///
    /// This will unmap the underlying shared memory if we aren't animating and all buffers have
    /// been released
    pub(crate) fn set_buffer_release_flag(
        &mut self,
        buffer_id: ObjectId,
        is_animating: bool,
    ) -> bool {
        if let Some(b) = self.buffers.iter().find(|b| b.object_id == buffer_id) {
            b.released.set_released();
            if !is_animating && self.buffers.iter().all(|b| b.released.is_released()) {
                for buffer in self.buffers.drain(..) {
                    buffer.destroy();
                }
                self.mmap.unmap();
            }
            true
        } else {
            false
        }
    }

    fn buffer_len(&self) -> usize {
        self.width as usize
            * self.height as usize
            * super::globals::pixel_format().channels() as usize
    }

    fn buffer_offset(&self, buffer_index: usize) -> usize {
        self.buffer_len() * buffer_index
    }

    fn occupied_bytes(&self) -> usize {
        self.buffer_offset(self.buffers.len())
    }

    /// resizes the pool and creates a new WlBuffer at the next free offset
    fn grow(&mut self) {
        let len = self.buffer_len();
        let new_len = self.occupied_bytes() + len;

        // we unmap the shared memory file descriptor when animations are done, so here we must
        // ensure the bytes are actually mmaped
        self.mmap.ensure_mapped();

        if new_len > self.mmap.len() {
            if new_len > i32::MAX as usize {
                panic!("Buffers have grown too big. We cannot allocate any more.")
            }
            self.mmap.remap(new_len);
            super::interfaces::wl_shm_pool::req::resize(self.pool_id, new_len as i32).unwrap();
        }

        let new_buffer_index = self.buffers.len();
        self.buffers.push(Buffer::new(
            self.pool_id,
            self.buffer_offset(new_buffer_index) as i32,
            self.width,
            self.height,
            self.width * super::globals::pixel_format().channels() as i32,
            super::globals::wl_shm_format(),
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

        if self.last_used_buffer != i {
            let last_offset = self.buffer_offset(self.last_used_buffer);
            self.mmap
                .slice_mut()
                .copy_within(last_offset..last_offset + len, offset);
            self.last_used_buffer = i;
        }

        &mut self.mmap.slice_mut()[offset..offset + len]
    }

    /// gets the last buffer we've drawn to
    pub(crate) fn get_commitable_buffer(&self) -> ObjectId {
        self.buffers[self.last_used_buffer].object_id
    }

    /// We assume `width` and `height` have already been multiplied by their scale factor
    pub(crate) fn resize(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
        self.last_used_buffer = 0;
        for buffer in self.buffers.drain(..) {
            buffer.destroy();
        }
    }
}

impl Drop for BumpPool {
    fn drop(&mut self) {
        for buffer in self.buffers.drain(..) {
            buffer.destroy();
        }
        if let Err(e) = super::interfaces::wl_shm_pool::req::destroy(self.pool_id) {
            log::error!("failed to destroy wl_shm_pool: {e}");
        }
    }
}
