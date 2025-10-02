//! This modules contains all the decompression functions, including the specialized ones using
//! architecture-dependent instructions

use crate::compression::DecompressionError;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(super) mod avx512;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(super) mod ssse3;

/// # SAFETY:
///
/// diff must end with 2 zero bytes
#[inline(always)]
pub(crate) unsafe fn unpack_bytes_4channels(
    buf: &mut [u8],
    diff: &[u8],
) -> Result<(), DecompressionError> {
    let mut dst = buf.as_mut_ptr();
    let mut src = diff.as_ptr();
    let last_src = src.add(diff.len() - 2);
    let last_dst = dst.add(buf.len());
    while src < last_src {
        while src.read() == u8::MAX {
            dst = dst.add(u8::MAX as usize * 4);
            src = src.add(1);
        }
        dst = dst.add(src.read() as usize * 4);
        src = src.add(1);

        let mut to_cpy = 0;
        while src.read() == u8::MAX {
            to_cpy += u8::MAX as usize;
            src = src.add(1);
        }
        to_cpy += src.read() as usize;
        src = src.add(1);

        verify_copy::<4>(src, last_src, dst, last_dst, to_cpy)?;

        for _ in 0..to_cpy {
            std::ptr::copy_nonoverlapping(src, dst, 4);
            dst.add(3).write(0xFF);
            src = src.add(3);
            dst = dst.add(4);
        }
        dst = dst.add(4);
    }
    Ok(())
}

/// # SAFETY:
///
/// diff must be a previously validated bitpack
#[inline(always)]
pub(super) unsafe fn unpack_unsafe_bytes_4channels(buf: &mut [u8], diff: &[u8]) {
    let mut dst = buf.as_mut_ptr();
    let mut src = diff.as_ptr();
    let last_src = src.add(diff.len() - 2);
    while src < last_src {
        while src.read() == u8::MAX {
            dst = dst.add(u8::MAX as usize * 4);
            src = src.add(1);
        }
        dst = dst.add(src.read() as usize * 4);
        src = src.add(1);

        let mut to_cpy = 0;
        while src.read() == u8::MAX {
            to_cpy += u8::MAX as usize;
            src = src.add(1);
        }
        to_cpy += src.read() as usize;
        src = src.add(1);

        for _ in 0..to_cpy {
            std::ptr::copy_nonoverlapping(src, dst, 4);
            dst.add(3).write(0xFF);
            src = src.add(3);
            dst = dst.add(4);
        }
        dst = dst.add(4);
    }
}

/// # SAFETY:
///
/// diff must end with 2 zero bytes
#[inline(always)]
pub(super) unsafe fn unpack_bytes_3channels(
    buf: &mut [u8],
    diff: &[u8],
) -> Result<(), DecompressionError> {
    let mut dst = buf.as_mut_ptr();
    let mut src = diff.as_ptr();
    let last_src = src.add(diff.len() - 2);
    let last_dst = dst.add(buf.len());
    while src < last_src {
        while src.read() == u8::MAX {
            dst = dst.add(u8::MAX as usize * 3);
            src = src.add(1);
        }
        dst = dst.add(src.read() as usize * 3);
        src = src.add(1);

        let mut to_cpy = 0;
        while src.read() == u8::MAX {
            to_cpy += u8::MAX as usize;
            src = src.add(1);
        }
        to_cpy += src.read() as usize;
        src = src.add(1);

        verify_copy::<3>(src, last_src, dst, last_dst, to_cpy)?;

        std::ptr::copy_nonoverlapping(src, dst, to_cpy * 3);
        dst = dst.add((to_cpy + 1) * 3);
        src = src.add(to_cpy * 3);
    }

    Ok(())
}

/// # SAFETY:
///
/// diff must be a previously validated bitpack
#[inline(always)]
pub(super) unsafe fn unpack_unsafe_bytes_3channels(buf: &mut [u8], diff: &[u8]) {
    let mut dst = buf.as_mut_ptr();
    let mut src = diff.as_ptr();
    let last_src = src.add(diff.len() - 2);
    while src < last_src {
        while src.read() == u8::MAX {
            dst = dst.add(u8::MAX as usize * 3);
            src = src.add(1);
        }
        dst = dst.add(src.read() as usize * 3);
        src = src.add(1);

        let mut to_cpy = 0;
        while src.read() == u8::MAX {
            to_cpy += u8::MAX as usize;
            src = src.add(1);
        }
        to_cpy += src.read() as usize;
        src = src.add(1);

        std::ptr::copy_nonoverlapping(src, dst, to_cpy * 3);
        dst = dst.add((to_cpy + 1) * 3);
        src = src.add(to_cpy * 3);
    }
}

/// # SAFETY
///
/// This function is actually always safe because we do not read pointers, just compare their
/// addresses
#[inline(always)]
unsafe fn verify_copy<const CHANNELS: usize>(
    src: *const u8,
    last_src: *const u8,
    dst: *const u8,
    last_dst: *const u8,
    to_cpy: usize,
) -> Result<(), DecompressionError> {
    #[cold]
    const fn err() -> DecompressionError {
        DecompressionError::CopyInstructionIsTooLarge
    }

    if src.add(to_cpy * 3) <= last_src && dst.add(to_cpy * CHANNELS) <= last_dst {
        return Ok(());
    }

    Err(err())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn ub_unpack_bytes4_poorly_formed() {
        let mut bytes = vec![u8::MAX; 9];
        let diff = vec![u8::MAX; 18];
        unsafe { unpack_bytes_4channels(&mut bytes, &diff) }.unwrap();
    }

    #[test]
    #[should_panic]
    fn ub_unpack_bytes3_poorly_formed() {
        let mut bytes = vec![u8::MAX; 9];
        let diff = vec![u8::MAX; 18];
        unsafe { unpack_bytes_3channels(&mut bytes, &diff) }.unwrap();
    }

    #[test]
    #[should_panic]
    fn ub_unpack_bytes4_poorly_formed2() {
        let mut bytes = vec![u8::MAX; 9];
        let mut diff = vec![u8::MAX; 18];
        diff[8] = 0;
        diff[7] = 0;
        unsafe { unpack_bytes_4channels(&mut bytes, &diff) }.unwrap();
    }

    #[test]
    #[should_panic]
    fn ub_unpack_bytes3_poorly_formed2() {
        let mut bytes = vec![u8::MAX; 9];
        let mut diff = vec![u8::MAX; 18];
        diff[8] = 0;
        diff[7] = 0;
        unsafe { unpack_bytes_3channels(&mut bytes, &diff) }.unwrap();
    }

    #[test]
    #[should_panic]
    fn ub_unpack_bytes4_poorly_formed3() {
        let mut bytes = vec![u8::MAX; 9];
        let mut diff = vec![u8::MAX; 18];
        diff[8] = 0;
        diff[7] = 0;
        diff[2] = 0;
        unsafe { unpack_bytes_4channels(&mut bytes, &diff) }.unwrap();
    }

    #[test]
    #[should_panic]
    fn ub_unpack_bytes3_poorly_formed3() {
        let mut bytes = vec![u8::MAX; 9];
        let mut diff = vec![u8::MAX; 18];
        diff[8] = 0;
        diff[7] = 0;
        diff[2] = 0;
        unsafe { unpack_bytes_3channels(&mut bytes, &diff) }.unwrap();
    }
}
