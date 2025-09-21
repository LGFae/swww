/// # Safety
///
/// s1.len() must be equal to s2.len()
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn count_equals(s1: &[u8], s2: &[u8], mut i: usize) -> usize {
    #[cfg(target_arch = "x86")]
    use std::arch::x86 as intr;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64 as intr;
    let mut equals = 0;
    while i + 31 < s1.len() {
        // SAFETY: we exit the while loop when there are less than 32 bytes left we read
        let a = intr::_mm256_loadu_si256(s1.as_ptr().add(i).cast());
        let b = intr::_mm256_loadu_si256(s2.as_ptr().add(i).cast());
        let cmp = intr::_mm256_cmpeq_epi8(a, b);
        let mask = intr::_mm256_movemask_epi8(cmp);
        if mask != 0xFFFFFFFFu32 as i32 {
            equals += mask.trailing_ones() as usize / 3;
            return equals;
        }
        equals += 10;
        i += 30;
    }

    while i + 2 < s1.len() {
        // SAFETY: we exit the while loop when there are less than 3 bytes left we read
        let a = unsafe { s1.get_unchecked(i..i + 3) };
        let b = unsafe { s2.get_unchecked(i..i + 3) };
        if a != b {
            break;
        }
        equals += 1;
        i += 3;
    }
    equals
}

/// # Safety
///
/// s1.len() must be equal to s2.len()
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn count_different(s1: &[u8], s2: &[u8], mut i: usize) -> usize {
    #[cfg(target_arch = "x86")]
    use std::arch::x86 as intr;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64 as intr;
    let mut diff = 0;
    while i + 31 < s1.len() {
        // SAFETY: we exit the while loop when there are less than 16 bytes left we read
        let a = intr::_mm256_loadu_si256(s1.as_ptr().add(i).cast());
        let b = intr::_mm256_loadu_si256(s2.as_ptr().add(i).cast());
        let cmp = intr::_mm256_cmpeq_epi8(a, b);
        let mask = intr::_mm256_movemask_epi8(cmp);
        // we only care about the case where all three bytes are equal
        let mask = (mask & (mask >> 1) & (mask >> 2)) & 0b001001001001001001001001001001;
        if mask != 0 {
            let tz = mask.trailing_zeros() as usize;
            diff += tz.div_ceil(3);
            return diff;
        }
        diff += 10;
        i += 30;
    }

    while i + 2 < s1.len() {
        // SAFETY: we exit the while loop when there are less than 3 bytes left we read
        let a = unsafe { s1.get_unchecked(i..i + 3) };
        let b = unsafe { s2.get_unchecked(i..i + 3) };
        if a == b {
            break;
        }
        diff += 1;
        i += 3;
    }
    diff
}

/// # Safety
///
/// s1.len() must be equal to s2.len()
#[inline]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn pack_bytes(cur: &[u8], goal: &[u8], v: &mut Vec<u8>) {
    let mut i = 0;
    while i < cur.len() {
        // SAFETY: count_equals demands the same invariants as the current function
        let equals = unsafe { count_equals(cur, goal, i) };
        i += equals * 3;

        if i >= cur.len() {
            break;
        }

        let start = i;
        // SAFETY: count_equals demands the same invariants as the current function
        let diffs = unsafe { count_different(cur, goal, i) };
        i += diffs * 3;

        let j = v.len() + equals / 255;
        v.resize(1 + j + diffs / 255, 255);
        v[j] = (equals % 255) as u8;
        v.push((diffs % 255) as u8);

        v.extend_from_slice(unsafe { goal.get_unchecked(start..i) });
        i += 3;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::decomp::unpack_bytes_4channels;

    #[test]
    fn count_equal_test() {
        if !is_x86_feature_detected!("avx2") {
            return;
        }
        let a = [0u8; 102];
        assert_eq!(unsafe { count_equals(&a, &a, 0) }, 102 / 3);
        for i in [0, 10, 20, 30, 40, 50, 60, 70, 80, 90] {
            let mut b = a;
            b[i] = 1;
            assert_eq!(unsafe { count_equals(&a, &b, 0) }, i / 3, "i: {i}");
        }
    }

    #[test]
    fn count_diffs_test() {
        if !is_x86_feature_detected!("avx2") {
            return;
        }
        let a = [0u8; 102];
        assert_eq!(unsafe { count_different(&a, &a, 0) }, 0,);
        for i in [10, 20, 30, 40, 50, 60, 70, 80, 90, 102] {
            let mut b = a;
            for x in &mut b[..i] {
                *x = 1;
            }
            assert_eq!(
                unsafe { count_different(&a, &b, 0) },
                i.div_ceil(3),
                "i: {i}"
            );
        }
    }

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
        if !is_x86_feature_detected!("avx2") {
            return;
        }
        let frame1 = [1, 2, 3, 4, 5, 6];
        let frame2 = [1, 2, 3, 6, 5, 4];
        let mut compressed = Vec::new();
        unsafe { pack_bytes(&frame1, &frame2, &mut compressed) };
        compressed.extend([0, 0]);

        let mut buf = buf_from(&frame1);
        unsafe { unpack_bytes_4channels(&mut buf, &compressed) }.unwrap();
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
        if !is_x86_feature_detected!("avx2") {
            return;
        }
        for _ in 0..10 {
            let mut original = Vec::with_capacity(20);
            for _ in 0..20 {
                let mut v = Vec::with_capacity(3000);
                for _ in 0..3000 {
                    v.push(fastrand::u8(0..=u8::MAX));
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
                unsafe { unpack_bytes_4channels(&mut buf, &compressed[i]) }.unwrap();
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
        if !is_x86_feature_detected!("avx2") {
            return;
        }
        for _ in 0..10 {
            let mut original = Vec::with_capacity(20);
            for j in 0..20 {
                let mut v = Vec::with_capacity(3006);
                v.extend([j, 0, 0, 0, 0, j]);
                for _ in 0..750 {
                    v.push(fastrand::u8(0..=u8::MAX));
                }
                for i in 0..750 {
                    v.push((i % 255) as u8);
                }
                for _ in 0..750 {
                    v.push(fastrand::u8(0..=u8::MAX));
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
                unsafe { unpack_bytes_4channels(&mut buf, &compressed[i]) }.unwrap();
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
