use super::super::DecompressionError;

#[inline]
#[target_feature(enable = "ssse3")]
pub(crate) unsafe fn unpack_bytes_4channels(
    buf: &mut [u8],
    diff: &[u8],
) -> Result<(), DecompressionError> {
    #[cfg(target_arch = "x86")]
    use core::arch::x86 as intr;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64 as intr;

    let mut dst = buf.as_mut_ptr();
    let mut src = diff.as_ptr();
    let last_src = src.add(diff.len() - 2);
    let last_dst = dst.add(buf.len());
    let mask = intr::_mm_set_epi8(-1, 11, 10, 9, -1, 8, 7, 6, -1, 5, 4, 3, -1, 2, 1, 0);
    let alphas = intr::_mm_set_epi8(-1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0);
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

        super::verify_copy::<4>(src, last_src, dst, last_dst, to_cpy)?;

        while to_cpy > 4 {
            let d = intr::_mm_loadu_si128(src.cast());
            let shuffled = intr::_mm_shuffle_epi8(d, mask);
            let to_store = intr::_mm_or_si128(shuffled, alphas);
            intr::_mm_storeu_si128(dst.cast(), to_store);

            src = src.add(12);
            dst = dst.add(16);
            to_cpy -= 4;
        }
        for _ in 0..to_cpy {
            core::ptr::copy_nonoverlapping(src, dst, 4);
            dst.add(3).write(0xFF);
            src = src.add(3);
            dst = dst.add(4);
        }
        dst = dst.add(4);
    }

    Ok(())
}

#[inline]
#[target_feature(enable = "ssse3")]
pub(crate) unsafe fn unpack_unsafe_bytes_4channels(buf: &mut [u8], diff: &[u8]) {
    #[cfg(target_arch = "x86")]
    use core::arch::x86 as intr;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64 as intr;

    let mut dst = buf.as_mut_ptr();
    let mut src = diff.as_ptr();
    let last_src = src.add(diff.len() - 2);
    let mask = intr::_mm_set_epi8(-1, 11, 10, 9, -1, 8, 7, 6, -1, 5, 4, 3, -1, 2, 1, 0);
    let alphas = intr::_mm_set_epi8(-1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0);
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

        while to_cpy > 4 {
            let d = intr::_mm_loadu_si128(src.cast());
            let shuffled = intr::_mm_shuffle_epi8(d, mask);
            let to_store = intr::_mm_or_si128(shuffled, alphas);
            intr::_mm_storeu_si128(dst.cast(), to_store);

            src = src.add(12);
            dst = dst.add(16);
            to_cpy -= 4;
        }
        for _ in 0..to_cpy {
            core::ptr::copy_nonoverlapping(src, dst, 4);
            dst.add(3).write(0xFF);
            src = src.add(3);
            dst = dst.add(4);
        }
        dst = dst.add(4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::comp::pack_bytes;

    fn buf_from(slice: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        for pix in slice.chunks_exact(3) {
            v.extend_from_slice(pix);
            v.push(255);
        }
        v
    }

    #[test]
    fn small() {
        if !is_x86_feature_detected!("ssse3") {
            return;
        }
        let frame1 = [1, 2, 3, 4, 5, 6];
        let frame2 = [1, 2, 3, 6, 5, 4];
        let mut compressed = Vec::new();
        unsafe { pack_bytes(&frame1, &frame2, &mut compressed) }
        compressed.extend([0, 0]);

        let mut buf = buf_from(&frame1);
        unsafe { unpack_bytes_4channels(&mut buf, &compressed).unwrap() }
        for i in 0..2 {
            for j in 0..3 {
                assert_eq!(
                    frame2[i * 3 + j],
                    buf[i * 4 + j],
                    "\nframe2: {frame2:?}, buf: {buf:?}\n"
                );
            }
        }
    }

    #[test]
    fn total_random() {
        if !is_x86_feature_detected!("ssse3") {
            return;
        }
        for _ in 0..10 {
            let mut original = Vec::with_capacity(20);
            for _ in 0..20 {
                let mut v = Vec::with_capacity(3000);
                for _ in 0..3000 {
                    v.push(fastrand::u8(..));
                }
                original.push(v);
            }

            let mut compressed = Vec::with_capacity(20);
            let mut buf = Vec::new();
            unsafe { pack_bytes(original.last().unwrap(), &original[0], &mut buf) }
            buf.extend([0, 0]);
            compressed.push(buf.clone().into_boxed_slice());
            for i in 1..20 {
                buf.clear();
                unsafe { pack_bytes(&original[i - 1], &original[i], &mut buf) }
                buf.extend([0, 0]);
                compressed.push(buf.clone().into_boxed_slice());
            }

            let mut buf = buf_from(original.last().unwrap());
            for i in 0..20 {
                unsafe { unpack_bytes_4channels(&mut buf, &compressed[i]).unwrap() }
                let mut j = 0;
                let mut l = 0;
                while j < 3000 {
                    for k in 0..3 {
                        assert_eq!(
                            buf[j + l + k],
                            original[i][j + k],
                            "Failed at index: {}",
                            j + k
                        );
                    }
                    j += 3;
                    l += 1;
                }
            }
        }
    }

    #[test]
    fn full() {
        if !is_x86_feature_detected!("ssse3") {
            return;
        }
        for _ in 0..10 {
            let mut original = Vec::with_capacity(20);
            for _ in 0..20 {
                let mut v = Vec::with_capacity(3000);
                for _ in 0..750 {
                    v.push(fastrand::u8(..));
                }
                for i in 0..750 {
                    v.push((i % 255) as u8);
                }
                for _ in 0..750 {
                    v.push(fastrand::u8(..));
                }
                for i in 0..750 {
                    v.push((i % 255) as u8);
                }
                original.push(v);
            }

            let mut compressed = Vec::with_capacity(20);
            let mut buf = Vec::new();
            unsafe { pack_bytes(original.last().unwrap(), &original[0], &mut buf) }
            buf.extend([0, 0]);
            compressed.push(buf.clone().into_boxed_slice());
            for i in 1..20 {
                buf.clear();
                unsafe { pack_bytes(&original[i - 1], &original[i], &mut buf) }
                buf.extend([0, 0]);
                compressed.push(buf.clone().into_boxed_slice());
            }

            let mut buf = buf_from(original.last().unwrap());
            for i in 0..20 {
                unsafe { unpack_bytes_4channels(&mut buf, &compressed[i]).unwrap() }
                let mut j = 0;
                let mut l = 0;
                while j < 3000 {
                    for k in 0..3 {
                        assert_eq!(
                            buf[j + l + k],
                            original[i][j + k],
                            "Failed at index: {}",
                            j + k
                        );
                    }
                    j += 3;
                    l += 1;
                }
            }
        }
    }
}
