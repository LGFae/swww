//! This modules contains all the specialized decompression functions, for each architecture,
//! using special SIMD functions and instructions

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(super) mod ssse3;
