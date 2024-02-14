//! # Compression Strategy
//!
//! For every pixel, we drop the alpha part; I don't think anyone will use transparency for a
//! background (nor if it even makes sense)
//!
//! For what's left, we store only the difference from the last frame to this one. We do that as
//! follows:
//! * First, we count how many pixels didn't change. We store that value as a u8.
//!   Every time the u8 hits the max (i.e. 255, or 0xFF), we push in onto the vector
//!   and restart the counting.
//! * Once we find a pixel that has changed, we count, starting from that one, how many changed,
//!   the same way we counted above (i.e. store as u8, every time it hits the max push and restart
//!   the counting)
//! * Then, we store all the new bytes.
//! * Start from the top until we are done with the image
//!

use lzzzz::lz4f;
use rkyv::{Archive, Deserialize, Serialize};

#[inline(always)]
fn count_equals(s1: &[u8], s2: &[u8], mut i: usize) -> usize {
    let mut equals = 0;
    while i + 7 < s1.len() {
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

#[inline(always)]
fn count_different(s1: &[u8], s2: &[u8], mut i: usize) -> usize {
    let mut different = 0;
    while i + 2 < s1.len() {
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
pub fn pack_bytes(cur: &[u8], goal: &[u8]) -> Box<[u8]> {
    let mut v = Vec::with_capacity((goal.len() * 5) / 8);

    let mut i = 0;
    while i < cur.len() {
        let equals = count_equals(cur, goal, i);
        i += equals * 3;

        if i >= cur.len() {
            return v.into_boxed_slice();
        }

        let start = i;
        let diffs = count_different(cur, goal, i);
        i += diffs * 3;

        let j = v.len() + equals / 255;
        v.resize(1 + j + diffs / 255, 255);
        v[j] = (equals % 255) as u8;
        v.push((diffs % 255) as u8);

        v.extend_from_slice(unsafe { goal.get_unchecked(start..i) });
        i += 3;
    }
    v.push(0);
    v.into_boxed_slice()
}

fn unpack_bytes(buf: &mut [u8], diff: &[u8]) {
    let len = diff.len();
    let buf = buf.as_mut_ptr();
    let diff = diff.as_ptr();

    let mut diff_idx = 0;
    let mut pix_idx = 0;
    while diff_idx + 1 < len {
        while unsafe { diff.add(diff_idx).read() } == u8::MAX {
            pix_idx += u8::MAX as usize;
            diff_idx += 1;
        }
        pix_idx += unsafe { diff.add(diff_idx).read() } as usize;
        diff_idx += 1;

        let mut to_cpy = 0;
        while unsafe { diff.add(diff_idx).read() } == u8::MAX {
            to_cpy += u8::MAX as usize;
            diff_idx += 1;
        }
        to_cpy += unsafe { diff.add(diff_idx).read() } as usize;
        diff_idx += 1;

        for _ in 0..to_cpy {
            unsafe { std::ptr::copy_nonoverlapping(diff.add(diff_idx), buf.add(pix_idx * 4), 4) }
            diff_idx += 3;
            pix_idx += 1;
        }
        pix_idx += 1;
    }
}

/// Struct responsible for compressing our data. We use it to cache vector extensions that might
/// speed up compression, as well as our lz4 compression configuration preferences
#[derive(Default)]
pub struct Compressor {
    preferences: lz4f::Preferences,
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    ssse3_support: bool,
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            preferences: lz4f::PreferencesBuilder::new()
                .block_size(lz4f::BlockSize::Max256KB)
                .compression_level(9)
                .build(),

            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            ssse3_support: is_x86_feature_detected!("ssse3"),
        }
    }

    /// Compresses a frame of animation by getting the difference between the previous and the
    /// current frame, and then running lz4
    pub fn compress(&self, prev: &[u8], cur: &[u8]) -> Result<BitPack, String> {
        let bit_pack = pack_bytes(prev, cur);

        if bit_pack.is_empty() {
            return Ok(BitPack {
                inner: Box::new([]),
                expected_buf_size: (cur.len() / 3) * 4,
            });
        }

        let mut v = Vec::with_capacity(bit_pack.len() / 2);
        lz4f::compress_to_vec(&bit_pack, &mut v, &self.preferences).map_err(|e| e.to_string())?;
        Ok(BitPack {
            inner: v.into_boxed_slice(),
            expected_buf_size: (cur.len() / 3) * 4,
        })
    }
}

/// This struct represents the cached difference between the previous frame and the next
#[derive(Archive, Serialize, Deserialize)]
pub struct BitPack {
    inner: Box<[u8]>,
    /// This field will ensure we won't ever try to unpack the images on a buffer of the wrong size,
    /// which ultimately is what allows us to use unsafe in the unpack_bytes function
    expected_buf_size: usize,
}

impl BitPack {
    ///return whether unpacking was successful. Note it can only fail if `buf.len() !=
    ///expected_buf_size`
    #[must_use]
    pub fn unpack(&self, buf: &mut [u8]) -> bool {
        if buf.len() == self.expected_buf_size {
            if !self.inner.is_empty() {
                let mut v = Vec::with_capacity(self.inner.len() * 3);
                // Note: panics will never happen because BitPacked is *always* only produced
                // with correct lz4 compression
                lz4f::decompress_to_vec(&self.inner, &mut v).unwrap();
                unpack_bytes(buf, &v);
            }
            true
        } else {
            false
        }
    }
}

impl ArchivedBitPack {
    ///return whether unpacking was successful. Note it can only fail if `buf.len() !=
    ///expected_buf_size`
    ///This function is identical to its NonArchived counterpart
    #[must_use]
    pub fn unpack(&self, buf: &mut [u8]) -> bool {
        let expected_len = self
            .expected_buf_size
            .deserialize(&mut rkyv::Infallible)
            .unwrap();
        if buf.len() == expected_len {
            if !self.inner.is_empty() {
                let mut v = Vec::with_capacity(self.inner.len() * 3);
                // Note: panics will never happen because BitPacked is *always* only produced
                // with correct lz4 compression
                lz4f::decompress_to_vec(&self.inner, &mut v).unwrap();
                unpack_bytes(buf, &v);
            }
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    //Use this when annoying problems show up
    fn should_compress_and_decompress_to_same_info_small() {
        let frame1 = [1, 2, 3, 4, 5, 6];
        let frame2 = [1, 2, 3, 6, 5, 4];
        let compressed = Compressor::new().compress(&frame1, &frame2).unwrap();

        let mut buf = buf_from(&frame1);
        assert!(compressed.unpack(&mut buf));
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
    fn should_compress_and_decompress_to_same_info() {
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
            let compressor = Compressor::new();
            compressed.push(
                compressor
                    .compress(original.last().unwrap(), &original[0])
                    .unwrap(),
            );
            for i in 1..20 {
                compressed.push(compressor.compress(&original[i - 1], &original[i]).unwrap());
            }

            let mut buf = buf_from(original.last().unwrap());
            for i in 0..20 {
                assert!(compressed[i].unpack(&mut buf));
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
    fn should_compress_and_decompress_to_same_info_with_equal_data() {
        for _ in 0..10 {
            let mut original = Vec::with_capacity(20);
            for _ in 0..20 {
                let mut v = Vec::with_capacity(3000);
                for _ in 0..2000 {
                    v.push(random::<u8>());
                }
                for i in 0..1000 {
                    v.push((i % 255) as u8);
                }
                original.push(v);
            }

            let compressor = Compressor::new();
            let mut compressed = Vec::with_capacity(20);
            compressed.push(
                compressor
                    .compress(original.last().unwrap(), &original[0])
                    .unwrap(),
            );
            for i in 1..20 {
                compressed.push(compressor.compress(&original[i - 1], &original[i]).unwrap());
            }

            let mut buf = buf_from(original.last().unwrap());
            for i in 0..20 {
                assert!(compressed[i].unpack(&mut buf));
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
