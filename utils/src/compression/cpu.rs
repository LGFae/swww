//! This module detects cpu features once and caches the results in mutable statics
//!
//! The mutable statics are not public, and therefore they are innacessible from other modules.
//! The only thing this module exposes are functions that let us read them, and one function to
//! initialize them.
//!
//! At worst, if we forget to initialize them, they will be holding false by default, and we won't
//! be using the more optimized versions of the relevant functions.

use std::sync::Once;

/// This macro declares a a cpu feature as a static mut, automatically generating a getter
/// function for it. Using it is safe because no one can modify it outside this module
macro_rules! decl_feature {
    ($feature:ident, $function:ident) => {
        static mut $feature: bool = false;
        #[inline(always)]
        pub fn $function() -> bool {
            // SAFETY: we ensure this is false by default, and only changes ONCE, if someone calls
            // this module's init() function
            unsafe { $feature }
        }
    };
}

static ONCE_INIT: Once = Once::new();
pub(super) fn init() {
    // SAFETY: features::init will modify some static mut variables. It is safe because we are
    // wrapping them in a Once call
    ONCE_INIT.call_once(|| unsafe { features::init() });
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub mod features {
    decl_feature!(SSE2, sse2);
    decl_feature!(SSSE3, ssse3);

    /// # Safety
    ///
    /// This function modifies the static muts inside this module, so it mustn't be called while
    /// someone else is trying to read them.
    ///
    /// That said, at worst, what will happen is that they will default to false and we won't use
    /// the most optimized versions of some functions. The case where we will accidentally call a
    /// function with unsupported instructions will never happen.
    pub(super) unsafe fn init() {
        SSE2 = is_x86_feature_detected!("sse2");
        SSSE3 = is_x86_feature_detected!("ssse3");
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
pub mod features {

    /// UNIMPLEMENTED!!! This function must exist so that the init function in super compiles on
    /// any target
    pub(super) unsafe fn init() {}
}
