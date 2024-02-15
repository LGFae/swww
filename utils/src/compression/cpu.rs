use std::sync::Once;

macro_rules! decl_feature {
    ($feature:ident, $function:ident) => {
        static mut $feature: bool = false;
        #[inline(always)]
        pub fn $function() -> bool {
            unsafe { $feature }
        }
    };
}

static ONCE_INIT: Once = Once::new();
pub(super) fn init() {
    ONCE_INIT.call_once(|| unsafe { features::init() });
}

#[cfg(target_arch = "x86_64")]
pub mod features {
    decl_feature!(SSE2, sse2);
    decl_feature!(SSSE3, ssse3);

    pub(super) unsafe fn init() {
        SSE2 = is_x86_feature_detected!("sse2");
        SSSE3 = is_x86_feature_detected!("ssse3");
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub mod features {

    /// do nothing for now
    pub(super) unsafe fn init() {}
}
