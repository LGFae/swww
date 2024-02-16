#[inline]
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn unpack_bytes(buf: &mut [u8], diff: &[u8]) {
    use std::arch::x86_64 as intr;

    let len = diff.len();
    let buf = buf.as_mut_ptr();
    let diff = diff.as_ptr();
    let mask = intr::_mm_set_epi8(-1, 11, 10, 9, -1, 8, 7, 6, -1, 5, 4, 3, -1, 2, 1, 0);

    let mut diff_idx = 0;
    let mut pix_idx = 0;
    while diff_idx + 1 < len {
        while diff.add(diff_idx).read() == u8::MAX {
            pix_idx += u8::MAX as usize;
            diff_idx += 1;
        }
        pix_idx += diff.add(diff_idx).read() as usize;
        diff_idx += 1;

        let mut to_cpy = 0;
        while diff.add(diff_idx).read() == u8::MAX {
            to_cpy += u8::MAX as usize;
            diff_idx += 1;
        }
        to_cpy += diff.add(diff_idx).read() as usize;
        diff_idx += 1;

        while to_cpy > 4 {
            let d = intr::_mm_loadu_si128(diff.add(diff_idx).cast());
            let to_store = intr::_mm_shuffle_epi8(d, mask);
            intr::_mm_storeu_si128(buf.add(pix_idx * 4).cast(), to_store);

            diff_idx += 12;
            pix_idx += 4;
            to_cpy -= 4;
        }
        for _ in 0..to_cpy {
            std::ptr::copy_nonoverlapping(diff.add(diff_idx), buf.add(pix_idx * 4), 4);
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
        let compressed = pack_bytes(&frame1, &frame2);

        let mut buf = buf_from(&frame1);
        unsafe { unpack_bytes(&mut buf, &compressed) }
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
            compressed.push(pack_bytes(original.last().unwrap(), &original[0]));
            for i in 1..20 {
                compressed.push(pack_bytes(&original[i - 1], &original[i]));
            }

            let mut buf = buf_from(original.last().unwrap());
            for i in 0..20 {
                unsafe { unpack_bytes(&mut buf, &compressed[i]) }
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
            compressed.push(pack_bytes(original.last().unwrap(), &original[0]));
            for i in 1..20 {
                compressed.push(pack_bytes(&original[i - 1], &original[i]));
            }

            let mut buf = buf_from(original.last().unwrap());
            for i in 0..20 {
                unsafe { unpack_bytes(&mut buf, &compressed[i]) }
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
