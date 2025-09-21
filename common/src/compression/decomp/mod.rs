//! This modules contains all the decompression functions, including the specialized ones using
//! architecture-dependent instructions

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(super) mod ssse3;

/// diff must be a slice produced by a BitPack
/// buf must have the EXACT expected size by the BitPack
#[inline(always)]
pub(super) fn unpack_bytes_4channels(buf: &mut [u8], diff: &[u8]) {
    // use the most efficient implementation available:
    #[cfg(not(debug_assertions))]
    {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        if super::cpu::features::ssse3() {
            return unsafe { ssse3::unpack_bytes_4channels(buf, diff) };
        }
    }

    let mut dst = buf.as_mut_ptr();
    let mut src = diff.as_ptr();
    unsafe {
        let last = src.add(diff.len() - 3);
        loop {
            let skip = index_of_first_zero_byte_u64(src);
            src = src.add(skip);
            dst = dst.add((u8::MAX as usize * skip + src.read() as usize) * 4);
            src = src.add(1);

            let mut to_cpy = 0;
            while src.read() == u8::MAX {
                to_cpy += u8::MAX as usize;
                src = src.add(1);
            }
            to_cpy += src.read() as usize;
            src = src.add(1);

            debug_assert!(
                src.add(to_cpy * 3 + 1) < last.add(3),
                "copying: {:?}, last: {last:?}",
                src.add(to_cpy * 3 + 1),
            );
            for _ in 0..to_cpy {
                std::ptr::copy_nonoverlapping(src, dst, 4);
                dst.add(3).write(0xFF);
                src = src.add(3);
                dst = dst.add(4);
            }
            dst = dst.add(4);
            if src == last {
                break;
            }
        }
    }
}

#[inline(always)]
pub(super) fn unpack_bytes_3channels(buf: &mut [u8], diff: &[u8]) {
    let mut dst = buf.as_mut_ptr();
    let mut src = diff.as_ptr();
    unsafe {
        let last = src.add(diff.len() - 3);
        loop {
            let skip = index_of_first_zero_byte_u64(src);
            src = src.add(skip);
            dst = dst.add((u8::MAX as usize * skip + src.read() as usize) * 3);
            src = src.add(1);

            let mut to_cpy = 0;
            while src.read() == u8::MAX {
                to_cpy += u8::MAX as usize;
                src = src.add(1);
            }
            to_cpy += src.read() as usize;
            src = src.add(1);

            debug_assert!(
                src.add(to_cpy * 3 + 1) < last.add(3),
                "copying: {:?}, last: {last:?}",
                src.add(to_cpy * 3 + 1),
            );
            std::ptr::copy_nonoverlapping(src, dst, to_cpy * 3);
            dst = dst.add((to_cpy + 1) * 3);
            src = src.add(to_cpy * 3);
            if src == last {
                break;
            }
        }
    }
}

unsafe fn index_of_first_zero_byte_u64(ptr: *const u8) -> usize {
    // I don't know if there is an architecture where usize > 8, but we need to prevent it anyway
    if const { std::mem::size_of::<usize>() > 8 } {
        let mut i = 0;
        let mut x = ptr.add(i).cast::<u64>().read_unaligned();
        while x == u64::MAX {
            i += 8;
            x = ptr.add(i).cast::<u64>().read_unaligned();
        }
        i += {
            #[cfg(target_endian = "little")]
            {
                x.trailing_ones() as usize / 8
            }
            #[cfg(target_endian = "big")]
            {
                x.leading_ones() as usize / 8
            }
        };
        i
    } else {
        let mut i = 0;
        let mut x = ptr.add(i).cast::<usize>().read_unaligned();
        while x == usize::MAX {
            i += std::mem::size_of::<usize>();
            x = ptr.add(i).cast::<usize>().read_unaligned();
        }
        i += {
            #[cfg(target_endian = "little")]
            {
                x.trailing_ones() as usize / 8
            }
            #[cfg(target_endian = "big")]
            {
                x.leading_ones() as usize / 8
            }
        };
        i
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
