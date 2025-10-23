use super::Wallpaper;
use crate::output_info::OutputInfo;

/// [Wallpaper] contains `clones` and `is_borrowed` fields to allow us to reimplement the semantics
/// of `Rc<RefCell<Wallpaper>>`. We do this because the normal implementation incurs 24 bytes of
/// overhead, but doing it all inline incurs 0 (because it will use padding space).
///
/// Note: this implementation is more stringent than the std Rc<RefCell<>> construct, because we
/// only allow exactly one borrow to be active at any time.
#[repr(transparent)]
#[derive(PartialEq)]
pub struct WallpaperCell(*mut Wallpaper);

#[repr(transparent)]
/// this needs to be a mutable reference because we need to overwrite the `is_borrowed` field on
/// drop
pub struct WallpaperBorrow<'a>(&'a mut Wallpaper);

#[repr(transparent)]
pub struct WallpaperBorrowMut<'a>(&'a mut Wallpaper);

impl WallpaperCell {
    const LAYOUT: std::alloc::Layout = std::alloc::Layout::new::<Wallpaper>();
    pub fn new(daemon: &mut crate::Daemon, output_info: OutputInfo) -> Self {
        let ptr = unsafe { std::alloc::alloc(Self::LAYOUT) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(Self::LAYOUT);
        }
        let inner = ptr.cast::<Wallpaper>();
        unsafe { inner.write(Wallpaper::new(daemon, output_info)) };
        Self(inner)
    }

    pub fn borrow(&self) -> WallpaperBorrow<'_> {
        // use pointers for this because we disobey Rust's borrowing rules
        // (we are mutating things behind an immutable reference)
        let is_borrowed_ptr = unsafe { &raw mut ((*self.0).is_borrowed) };
        let is_borrowed = unsafe { is_borrowed_ptr.read() };
        assert!(!is_borrowed);
        unsafe { is_borrowed_ptr.write(true) };
        WallpaperBorrow(unsafe { self.0.as_mut().unwrap_unchecked() })
    }

    pub fn borrow_mut(&self) -> WallpaperBorrowMut<'_> {
        // use pointers for this because we disobey Rust's borrowing rules
        // (we are mutating things behind an immutable reference)
        let is_borrowed_ptr = unsafe { &raw mut ((*self.0).is_borrowed) };
        let is_borrowed = unsafe { is_borrowed_ptr.read() };
        assert!(!is_borrowed);
        unsafe { is_borrowed_ptr.write(true) };
        WallpaperBorrowMut(unsafe { self.0.as_mut().unwrap_unchecked() })
    }

    pub fn clone(&self) -> Self {
        // use pointers for this because we disobey Rust's borrowing rules
        // (we are mutating things behind an immutable reference)
        let clones_ptr = unsafe { &raw mut ((*self.0).clones) };
        let clones = unsafe { clones_ptr.read() };
        unsafe { clones_ptr.write(clones + 1) };
        Self(self.0)
    }
}

impl<'a> std::ops::Deref for WallpaperBorrow<'a> {
    type Target = Wallpaper;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'a> std::ops::Deref for WallpaperBorrowMut<'a> {
    type Target = Wallpaper;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'a> std::ops::DerefMut for WallpaperBorrowMut<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
    }
}

impl<'a> Drop for WallpaperBorrow<'a> {
    fn drop(&mut self) {
        self.0.is_borrowed = false;
    }
}

impl<'a> Drop for WallpaperBorrowMut<'a> {
    fn drop(&mut self) {
        self.0.is_borrowed = false;
    }
}

impl Drop for WallpaperCell {
    fn drop(&mut self) {
        let clones_ptr = unsafe { &raw mut ((*self.0).clones) };
        let clones = unsafe { clones_ptr.read() };
        if clones > 0 {
            unsafe { clones_ptr.write(clones - 1) };
        } else {
            unsafe { std::alloc::dealloc(self.0.cast(), Self::LAYOUT) };
        }
    }
}
