use std::ptr::NonNull;

use rustix::fd::AsFd;
use rustix::fd::BorrowedFd;
use rustix::fd::OwnedFd;
use rustix::io::Errno;
use rustix::mm::mmap;
use rustix::mm::munmap;
use rustix::mm::MapFlags;
use rustix::mm::ProtFlags;
use rustix::shm::Mode;
use rustix::shm::ShmOFlags;

#[derive(Debug)]
pub struct Mmap {
    fd: OwnedFd,
    ptr: NonNull<std::ffi::c_void>,
    len: usize,
    mmaped: bool,
}

impl Mmap {
    const PROT: ProtFlags = ProtFlags::WRITE.union(ProtFlags::READ);
    const FLAGS: MapFlags = MapFlags::SHARED;

    #[inline]
    #[must_use]
    pub fn create(len: usize) -> Self {
        let fd = create_shm_fd().unwrap();
        rustix::io::retry_on_intr(|| rustix::fs::ftruncate(&fd, len as u64)).unwrap();

        let ptr = unsafe {
            let ptr = mmap(std::ptr::null_mut(), len, Self::PROT, Self::FLAGS, &fd, 0).unwrap();
            // SAFETY: the function above will never return a null pointer if it succeeds
            // POSIX says that the implementation will never select an address at 0
            NonNull::new_unchecked(ptr)
        };
        Self {
            fd,
            ptr,
            len,
            mmaped: true,
        }
    }

    #[inline]
    /// Unmaps without destroying the file descriptor
    ///
    /// This is only ever used in the daemon, when animations finish, in order to free up memory
    pub fn unmap(&mut self) {
        if let Err(e) = unsafe { munmap(self.ptr.as_ptr(), self.len) } {
            eprintln!("ERROR WHEN UNMAPPING MEMORY: {e}");
        } else {
            self.mmaped = false;
        }
    }

    #[inline]
    /// Ensures that the underlying file descriptor is mapped
    ///
    /// Because `unmap`, above, is only used in the daemon, this is also only used there
    pub fn ensure_mapped(&mut self) {
        if !self.mmaped {
            self.mmaped = true;
            self.ptr = unsafe {
                let ptr = mmap(
                    std::ptr::null_mut(),
                    self.len,
                    Self::PROT,
                    Self::FLAGS,
                    &self.fd,
                    0,
                )
                .unwrap();
                // SAFETY: the function above will never return a null pointer if it succeeds
                // POSIX says that the implementation will never select an address at 0
                NonNull::new_unchecked(ptr)
            };
        }
    }

    #[inline]
    pub fn remap(&mut self, new_len: usize) {
        rustix::io::retry_on_intr(|| rustix::fs::ftruncate(&self.fd, new_len as u64)).unwrap();

        #[cfg(target_os = "linux")]
        {
            let result = unsafe {
                rustix::mm::mremap(
                    self.ptr.as_ptr(),
                    self.len,
                    new_len,
                    rustix::mm::MremapFlags::MAYMOVE,
                )
            };

            if let Ok(ptr) = result {
                // SAFETY: the mremap above will never return a null pointer if it succeeds
                let ptr = unsafe { NonNull::new_unchecked(ptr) };
                self.ptr = ptr;
                self.len = new_len;
                return;
            }
        }

        self.unmap();

        self.len = new_len;
        self.ptr = unsafe {
            let ptr = mmap(
                std::ptr::null_mut(),
                self.len,
                Self::PROT,
                Self::FLAGS,
                &self.fd,
                0,
            )
            .unwrap();
            // SAFETY: the function above will never return a null pointer if it succeeds
            // POSIX says that the implementation will never select an address at 0
            NonNull::new_unchecked(ptr)
        };
    }

    #[must_use]
    pub(crate) fn from_fd(fd: OwnedFd, len: usize) -> Self {
        let ptr = unsafe {
            let ptr = mmap(
                std::ptr::null_mut(),
                len,
                ProtFlags::READ,
                Self::FLAGS,
                &fd,
                0,
            )
            .unwrap();
            // SAFETY: the function above will never return a null pointer if it succeeds
            // POSIX says that the implementation will never select an address at 0
            NonNull::new_unchecked(ptr)
        };
        Self {
            fd,
            ptr,
            len,
            mmaped: true,
        }
    }

    #[inline]
    #[must_use]
    pub fn slice_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr().cast(), self.len) }
    }

    #[inline]
    #[must_use]
    pub fn slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr().cast(), self.len) }
    }

    #[inline]
    #[must_use]
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    #[must_use]
    pub fn fd(&self) -> BorrowedFd {
        self.fd.as_fd()
    }
}

impl Drop for Mmap {
    #[inline]
    fn drop(&mut self) {
        if self.mmaped {
            self.unmap();
        }
    }
}

fn create_shm_fd() -> std::io::Result<OwnedFd> {
    #[cfg(target_os = "linux")]
    {
        match create_memfd() {
            Ok(fd) => return Ok(fd),
            // Not supported, use fallback.
            Err(Errno::NOSYS) => (),
            Err(err) => return Err(err.into()),
        };
    }

    let time = std::time::SystemTime::now();
    let mut mem_file_handle = format!(
        "/swww-ipc-{}",
        time.duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );

    let flags = ShmOFlags::CREATE | ShmOFlags::EXCL | ShmOFlags::RDWR;
    let mode = Mode::RUSR | Mode::WUSR;
    loop {
        match rustix::shm::shm_open(mem_file_handle.as_str(), flags, mode) {
            Ok(fd) => match rustix::shm::shm_unlink(mem_file_handle.as_str()) {
                Ok(_) => return Ok(fd),

                Err(errno) => {
                    return Err(errno.into());
                }
            },
            Err(Errno::EXIST) => {
                // Change the handle if we happen to be duplicate.
                let time = std::time::SystemTime::now();

                mem_file_handle = format!(
                    "/swww-ipc-{}",
                    time.duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .subsec_nanos()
                );

                continue;
            }
            Err(Errno::INTR) => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

#[cfg(target_os = "linux")]
fn create_memfd() -> rustix::io::Result<OwnedFd> {
    use rustix::fs::{MemfdFlags, SealFlags};
    use std::ffi::CStr;

    let name = CStr::from_bytes_with_nul(b"swww-ipc\0").unwrap();
    let flags = MemfdFlags::ALLOW_SEALING | MemfdFlags::CLOEXEC;

    loop {
        match rustix::fs::memfd_create(name, flags) {
            Ok(fd) => {
                // We only need to seal for the purposes of optimization, ignore the errors.
                let _ = rustix::fs::fcntl_add_seals(&fd, SealFlags::SHRINK | SealFlags::SEAL);
                return Ok(fd);
            }
            Err(Errno::INTR) => continue,
            Err(err) => return Err(err),
        }
    }
}

pub struct Mmapped<const UTF8: bool> {
    base_ptr: NonNull<std::ffi::c_void>,
    ptr: NonNull<std::ffi::c_void>,
    len: usize,
}

pub type MmappedBytes = Mmapped<false>;
pub type MmappedStr = Mmapped<true>;

impl<const UTF8: bool> Mmapped<UTF8> {
    const PROT: ProtFlags = ProtFlags::READ;
    const FLAGS: MapFlags = MapFlags::SHARED;

    pub(crate) fn new(map: &Mmap, bytes: &[u8]) -> Self {
        let len = u32::from_ne_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let bytes = &bytes[4..];
        Self::new_with_len(map, bytes, len)
    }

    pub(crate) fn new_with_len(map: &Mmap, bytes: &[u8], len: usize) -> Self {
        let offset = bytes.as_ptr() as usize - map.ptr.as_ptr() as usize;
        let page_size = rustix::param::page_size();
        let page_offset = offset - offset % page_size;

        let base_ptr = unsafe {
            let ptr = mmap(
                std::ptr::null_mut(),
                len + (offset - page_offset),
                Self::PROT,
                Self::FLAGS,
                &map.fd,
                page_offset as u64,
            )
            .unwrap();
            // SAFETY: the function above will never return a null pointer if it succeeds
            // POSIX says that the implementation will never select an address at 0
            NonNull::new_unchecked(ptr)
        };
        let ptr =
            unsafe { NonNull::new_unchecked(base_ptr.as_ptr().byte_add(offset - page_offset)) };

        if UTF8 {
            // try to parse, panicking if we fail
            let s = unsafe { std::slice::from_raw_parts(ptr.as_ptr().cast(), len) };
            let _s = std::str::from_utf8(s).expect("received a non utf8 string from socket");
        }

        Self { base_ptr, ptr, len }
    }

    #[inline]
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr().cast(), self.len) }
    }
}

impl MmappedStr {
    #[inline]
    #[must_use]
    pub fn str(&self) -> &str {
        let s = unsafe { std::slice::from_raw_parts(self.ptr.as_ptr().cast(), self.len) };
        unsafe { std::str::from_utf8_unchecked(s) }
    }
}

impl<const UTF8: bool> Drop for Mmapped<UTF8> {
    #[inline]
    fn drop(&mut self) {
        let len = self.len + self.ptr.as_ptr() as usize - self.base_ptr.as_ptr() as usize;
        if let Err(e) = unsafe { munmap(self.base_ptr.as_ptr(), len) } {
            eprintln!("ERROR WHEN UNMAPPING MEMORY: {e}");
        }
    }
}

unsafe impl<const UTF8: bool> Send for Mmapped<UTF8> {}
unsafe impl<const UTF8: bool> Sync for Mmapped<UTF8> {}
