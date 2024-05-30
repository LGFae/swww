//! This modules contains all the decompression functions, including the specialized ones using
//! architecture-dependent instructions

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(super) mod ssse3;

/// diff must be a slice produced by a BitPack
/// buf must have the EXACT expected size by the BitPack
#[inline(always)]
pub(super) fn unpack_bytes_4channels(buf: &mut [u8], diff: &[u8]) {
    assert!(
        diff[diff.len() - 1] | diff[diff.len() - 2] == 0,
        "Poorly formed BitPack"
    );
    // use the most efficient implementation available:
    #[cfg(not(test))] // when testing, we want to use the specific implementation
    {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        if super::cpu::features::ssse3() {
            return unsafe { ssse3::unpack_bytes_4channels(buf, diff) };
        }
    }

    // The final bytes are just padding to prevent us from going out of bounds
    let len = diff.len() - 3;
    let buf_ptr = buf.as_mut_ptr();
    let diff_ptr = diff.as_ptr();

    let mut diff_idx = 0;
    let mut pix_idx = 0;
    while diff_idx < len {
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

        assert!(
            diff_idx + to_cpy * 3 + 1 < diff.len(),
            "copying: {}, diff.len(): {}",
            diff_idx + to_cpy * 3 + 1,
            diff.len()
        );
        for _ in 0..to_cpy {
            unsafe {
                std::ptr::copy_nonoverlapping(diff_ptr.add(diff_idx), buf_ptr.add(pix_idx * 4), 4)
            }
            diff_idx += 3;
            pix_idx += 1;
        }
        pix_idx += 1;
    }
}

#[inline(always)]
pub(super) fn unpack_bytes_3channels(buf: &mut [u8], diff: &[u8]) {
    assert!(
        diff[diff.len() - 1] | diff[diff.len() - 2] == 0,
        "Poorly formed BitPack"
    );
    // The final bytes are just padding to prevent us from going out of bounds
    let len = diff.len() - 3;
    let buf_ptr = buf.as_mut_ptr();
    let diff_ptr = diff.as_ptr();

    let mut diff_idx = 0;
    let mut pix_idx = 0;
    while diff_idx < len {
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

        assert!(
            diff_idx + to_cpy * 3 <= diff.len(),
            "diff_idx: {diff_idx}, to_copy: {to_cpy}, diff.len(): {}",
            diff.len()
        );
        unsafe {
            std::ptr::copy_nonoverlapping(
                diff_ptr.add(diff_idx),
                buf_ptr.add(pix_idx * 3),
                to_cpy * 3,
            );
        }
        diff_idx += to_cpy * 3;
        pix_idx += to_cpy + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn ub_unpack_bytes4_poorly_formed() {
        let mut bytes = vec![u8::MAX; 9];
        let diff = vec![u8::MAX; 18];
        unpack_bytes_4channels(&mut bytes, &diff);
    }

    #[test]
    #[should_panic]
    fn ub_unpack_bytes3_poorly_formed() {
        let mut bytes = vec![u8::MAX; 9];
        let diff = vec![u8::MAX; 18];
        unpack_bytes_3channels(&mut bytes, &diff);
    }

    #[test]
    #[should_panic]
    fn ub_unpack_bytes4_poorly_formed2() {
        let mut bytes = vec![u8::MAX; 9];
        let mut diff = vec![u8::MAX; 18];
        diff[8] = 0;
        diff[7] = 0;
        unpack_bytes_4channels(&mut bytes, &diff);
    }

    #[test]
    #[should_panic]
    fn ub_unpack_bytes3_poorly_formed2() {
        let mut bytes = vec![u8::MAX; 9];
        let mut diff = vec![u8::MAX; 18];
        diff[8] = 0;
        diff[7] = 0;
        unpack_bytes_3channels(&mut bytes, &diff);
    }

    #[test]
    #[should_panic]
    fn ub_unpack_bytes4_poorly_formed3() {
        let mut bytes = vec![u8::MAX; 9];
        let mut diff = vec![u8::MAX; 18];
        diff[8] = 0;
        diff[7] = 0;
        diff[2] = 0;
        unpack_bytes_4channels(&mut bytes, &diff);
    }

    #[test]
    #[should_panic]
    fn ub_unpack_bytes3_poorly_formed3() {
        let mut bytes = vec![u8::MAX; 9];
        let mut diff = vec![u8::MAX; 18];
        diff[8] = 0;
        diff[7] = 0;
        diff[2] = 0;
        unpack_bytes_3channels(&mut bytes, &diff);
    }
}
