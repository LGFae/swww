#[inline]
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn unpack_bytes_4channels(buf: &mut [u8], diff: &[u8]) {
    #[cfg(target_arch = "x86")]
    use std::arch::x86 as intr;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64 as intr;

    // The final bytes are just padding to prevent us from going out of bounds
    let len = diff.len() - 3;
    let buf_ptr = buf.as_mut_ptr();
    let diff_ptr = diff.as_ptr();
    let mask = intr::_mm_set_epi8(-1, 11, 10, 9, -1, 8, 7, 6, -1, 5, 4, 3, -1, 2, 1, 0);

    let mut diff_idx = 0;
    let mut pix_idx = 0;
    while diff_idx < len {
        while diff_ptr.add(diff_idx).read() == u8::MAX {
            pix_idx += u8::MAX as usize;
            diff_idx += 1;
        }
        pix_idx += diff_ptr.add(diff_idx).read() as usize;
        diff_idx += 1;

        let mut to_cpy = 0;
        while diff_ptr.add(diff_idx).read() == u8::MAX {
            to_cpy += u8::MAX as usize;
            diff_idx += 1;
        }
        to_cpy += diff_ptr.add(diff_idx).read() as usize;
        diff_idx += 1;

        assert!(
            diff_idx + to_cpy * 3 + 1 < diff.len(),
            "copying: {}, diff.len(): {}",
            diff_idx + to_cpy * 3 + 1,
            diff.len()
        );
        while to_cpy > 4 {
            let d = intr::_mm_loadu_si128(diff_ptr.add(diff_idx).cast());
            let to_store = intr::_mm_shuffle_epi8(d, mask);
            intr::_mm_storeu_si128(buf_ptr.add(pix_idx * 4).cast(), to_store);

            diff_idx += 12;
            pix_idx += 4;
            to_cpy -= 4;
        }
        for _ in 0..to_cpy {
            std::ptr::copy_nonoverlapping(diff_ptr.add(diff_idx), buf_ptr.add(pix_idx * 4), 4);
            diff_idx += 3;
            pix_idx += 1;
        }
        pix_idx += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::pack_bytes;
    use rand::prelude::random;

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
                    v.push(random::<u8>());
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
                    v.push(random::<u8>());
                }
                for i in 0..750 {
                    v.push((i % 255) as u8);
                }
                for _ in 0..750 {
                    v.push(random::<u8>());
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
