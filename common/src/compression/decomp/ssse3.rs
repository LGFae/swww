#[inline]
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn unpack_bytes_4channels(buf: &mut [u8], diff: &[u8]) {
    #[cfg(target_arch = "x86")]
    use std::arch::x86 as intr;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64 as intr;

    let mut dst = buf.as_mut_ptr();
    let mut src = diff.as_ptr();
    let last = src.add(diff.len() - 3);
    let mask = intr::_mm_set_epi8(-1, 11, 10, 9, -1, 8, 7, 6, -1, 5, 4, 3, -1, 2, 1, 0);
    let alphas = intr::_mm_set_epi8(-1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0);
    loop {
        let skip = super::index_of_first_zero_byte_u64(src);
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

        assert!(
            src.add(to_cpy * 3 + 1) < last.add(3),
            "copying: {:?}, last: {last:?}",
            src.add(to_cpy * 3 + 1),
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::pack_bytes;

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

        let mut buf = buf_from(&frame1);
        unsafe { unpack_bytes_4channels(&mut buf, &compressed) }
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
            compressed.push(buf.clone().into_boxed_slice());
            for i in 1..20 {
                buf.clear();
                unsafe { pack_bytes(&original[i - 1], &original[i], &mut buf) }
                compressed.push(buf.clone().into_boxed_slice());
            }

            let mut buf = buf_from(original.last().unwrap());
            for i in 0..20 {
                unsafe { unpack_bytes_4channels(&mut buf, &compressed[i]) }
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
            compressed.push(buf.clone().into_boxed_slice());
            for i in 1..20 {
                buf.clear();
                unsafe { pack_bytes(&original[i - 1], &original[i], &mut buf) }
                compressed.push(buf.clone().into_boxed_slice());
            }

            let mut buf = buf_from(original.last().unwrap());
            for i in 0..20 {
                unsafe { unpack_bytes_4channels(&mut buf, &compressed[i]) }
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
