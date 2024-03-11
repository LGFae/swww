//! # Compression Strategy
//!
//! We only compress RBG images, 8 bytes per channel; I don't think anyone will use
//! transparency for a background (nor if it even makes sense)
//!
//! For what's left, we store only the difference from the last frame to this one.
//! We do that as follows:
//! * First, we count how many pixels didn't change. We store that value as a u8.
//!   Every time the u8 hits the max (i.e. 255, or 0xFF), we push in onto the vector
//!   and restart the counting.
//! * Once we find a pixel that has changed, we count, starting from that one, how many
//!   changed, the same way we counted above (i.e. store as u8, every time it hits the
//!   max push and restart the counting)
//! * Then, we store all the new bytes.
//! * Start from the top until we are done with the image
//!
//! The default implementation lies in this file. Architecture-specific implementations
//! that make use of specialized instructions lie in other submodules.

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(super) mod sse2;

/// # Safety
///
/// s1.len() must be equal to s2.len()
#[inline(always)]
unsafe fn count_equals(s1: &[u8], s2: &[u8], mut i: usize) -> usize {
    let mut equals = 0;
    while i + 7 < s1.len() {
        // SAFETY: we exit the while loop when there are less than 8 bytes left we read
        let a: u64 = unsafe { s1.as_ptr().add(i).cast::<u64>().read_unaligned() };
        let b: u64 = unsafe { s2.as_ptr().add(i).cast::<u64>().read_unaligned() };
        let cmp = a ^ b;
        if cmp != 0 {
            equals += cmp.trailing_zeros() as usize / 24;
            return equals;
        }
        equals += 2;
        i += 6;
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
#[inline(always)]
unsafe fn count_different(s1: &[u8], s2: &[u8], mut i: usize) -> usize {
    let mut different = 0;
    while i + 2 < s1.len() {
        // SAFETY: we exit the while loop when there are less than 3 bytes left we read
        let a = unsafe { s1.get_unchecked(i..i + 3) };
        let b = unsafe { s2.get_unchecked(i..i + 3) };
        if a == b {
            break;
        }
        different += 1;
        i += 3;
    }
    different
}

/// This calculates the difference between the current(cur) frame and the next(goal)
///
/// # Safety
///
/// cur.len() must be equal to goal.len()
#[inline(always)]
pub(super) unsafe fn pack_bytes(cur: &[u8], goal: &[u8], v: &mut Vec<u8>) {
    // use the most efficient implementation available:
    #[cfg(not(test))] // when testing, we want to use the specific implementation
    {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        if super::cpu::features::sse2() {
            return unsafe { sse2::pack_bytes(cur, goal, v) };
        }
    }

    let mut i = 0;
    while i < cur.len() {
        // SAFETY: count_equals demands the same invariants as the current function
        let equals = unsafe { count_equals(cur, goal, i) };
        i += equals * 3;

        if i >= cur.len() {
            break;
        }

        let start = i;
        // SAFETY: count_different demands the same invariants as the current function
        let diffs = unsafe { count_different(cur, goal, i) };
        i += diffs * 3;

        let j = v.len() + equals / 255;
        v.resize(1 + j + diffs / 255, 255);
        v[j] = (equals % 255) as u8;
        v.push((diffs % 255) as u8);

        v.extend_from_slice(unsafe { goal.get_unchecked(start..i) });
        i += 3;
    }

    if !v.is_empty() {
        // add two extra bytes to prevent access out of bounds later during decompression
        v.push(0);
        v.push(0);
    }
}

#[cfg(test)]
mod tests {
    // note the full compression -> decompression roundtrip is tested in super

    use super::*;
    #[test]
    fn count_equal_test() {
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
        let a = [0u8; 102];
        assert_eq!(unsafe { count_different(&a, &a, 0) }, 0,);
        for i in [10, 20, 30, 40, 50, 60, 70, 80, 90, 102] {
            let mut b = a;
            for x in &mut b[..i] {
                *x = 1;
            }
            assert_eq!(unsafe { count_different(&a, &b, 0) }, (i + 2) / 3, "i: {i}");
        }
    }
}
