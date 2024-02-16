//! This modules contains all the decompression functions, including the specialized ones using
//! architecture-dependent instructions

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(super) mod ssse3;

#[inline(always)]
pub(super) fn unpack_bytes(buf: &mut [u8], diff: &[u8]) {
    // use the most efficient implementation available:
    #[cfg(not(test))] // when testing, we want to use the specific implementation
    {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        if super::cpu::features::ssse3() {
            return unsafe { ssse3::unpack_bytes(buf, diff) };
        }
    }

    // The very final byte is just padding to let us read 4 bytes at once without going out of
    // bounds
    let len = diff.len() - 1;
    let buf_ptr = buf.as_mut_ptr();
    let diff_ptr = diff.as_ptr();

    let mut diff_idx = 0;
    let mut pix_idx = 0;
    while diff_idx + 1 < len {
        while unsafe { diff_ptr.add(diff_idx).read() } == u8::MAX {
            pix_idx += u8::MAX as usize;
            diff_idx += 1;
        }
        pix_idx += unsafe { diff_ptr.add(diff_idx).read() } as usize;
        diff_idx += 1;

        let mut to_cpy = 0;
        while unsafe { diff_ptr.add(diff_idx).read() } == u8::MAX {
            to_cpy += u8::MAX as usize;
            diff_idx += 1;
        }
        to_cpy += unsafe { diff_ptr.add(diff_idx).read() } as usize;
        diff_idx += 1;

        for _ in 0..to_cpy {
            debug_assert!(
                diff_idx + 3 < diff.len(),
                "diff_idx + 3: {}, diff.len(): {}",
                diff_idx + 3,
                diff.len()
            );
            unsafe {
                std::ptr::copy_nonoverlapping(diff_ptr.add(diff_idx), buf_ptr.add(pix_idx * 4), 4)
            }
            diff_idx += 3;
            pix_idx += 1;
        }
        pix_idx += 1;
    }
}
