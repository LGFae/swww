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
use serde::{Deserialize, Serialize};

lazy_static::lazy_static! {
    static ref COMPRESSION_PREFERENCES: lz4f::Preferences = lz4f::PreferencesBuilder::new()
            .block_size(lz4f::BlockSize::Max256KB)
            .compression_level(9)
            .build();
}

/// This calculates the difference between the current(cur) frame and the next(goal).
/// The closure you pass is run at every difference. It dictates the update logic of the current
/// frame. With that, you can control whether all different pixels changed are updated, or only the
/// ones at a certain position. It is meant to be used primarily when writing transitions
fn pack_bytes(cur: &[u8], goal: &[u8]) -> Box<[u8]> {
    let mut v = Vec::with_capacity(goal.len());

    let mut iter = zip_eq(pixels(cur), pixels(goal));
    let mut to_add = Vec::with_capacity(333); // 100 pixels
    while let Some((mut cur, mut goal)) = iter.next() {
        let mut equals = 0;
        while cur == goal {
            equals += 1;
            match iter.next() {
                None => return v.into_boxed_slice(),
                Some((c, g)) => {
                    cur = c;
                    goal = g;
                }
            }
        }

        let mut diffs = 0;
        while cur != goal {
            to_add.extend_from_slice(goal);
            diffs += 1;
            match iter.next() {
                None => break,
                Some((c, g)) => {
                    cur = c;
                    goal = g;
                }
            }
        }
        let j = v.len() + equals / 255;
        v.resize(1 + v.len() + equals / 255 + diffs / 255, 255);
        v[j] = (equals % 255) as u8;
        v.push((diffs % 255) as u8);
        v.append(&mut to_add);
    }
    v.push(0);
    v.into_boxed_slice()
}

fn unpack_bytes(buf: &mut [u8], diff: &[u8]) {
    let buf_chunks = pixels_mut(buf);
    let mut diff_idx = 0;
    let mut pix_idx = 0;
    while diff_idx < diff.len() - 1 {
        while diff[diff_idx] == u8::MAX {
            pix_idx += u8::MAX as usize;
            diff_idx += 1;
        }
        pix_idx += diff[diff_idx] as usize;
        diff_idx += 1;

        let mut to_cpy = 0;
        while diff[diff_idx] == u8::MAX {
            to_cpy += u8::MAX as usize;
            diff_idx += 1;
        }
        to_cpy += diff[diff_idx] as usize;
        diff_idx += 1;

        for _ in 0..to_cpy {
            unsafe {
                buf_chunks
                    .get_unchecked_mut(pix_idx)
                    .clone_from_slice(diff.get_unchecked(diff_idx..diff_idx + 4));
            }
            diff_idx += 3;
            pix_idx += 1;
        }
        pix_idx += 1;
    }
}

/// This struct represents the cached difference between the previous frame and the next
#[derive(Serialize, Deserialize)]
pub struct BitPack {
    inner: Box<[u8]>,
    /// This field will ensure we won't ever try to unpack the images on a buffer of the wrong size,
    /// which ultimately is what allows us to use unsafe in the unpack_bytes function
    expected_buf_size: usize,
}

impl BitPack {
    /// Compresses a frame of animation by getting the difference between the previous and the
    /// current frame.
    /// IMPORTANT: this will change `prev` into `cur`, that's why it needs to be 'mut'
    pub fn pack(prev: &[u8], cur: &[u8]) -> Result<Self, String> {
        let bit_pack = pack_bytes(prev, cur);
        if bit_pack.is_empty() {
            return Ok(BitPack {
                inner: Box::new([]),
                expected_buf_size: (cur.len() / 3) * 4,
            });
        }

        let mut v = Vec::with_capacity(bit_pack.len() / 2);
        match lzzzz::lz4f::compress_to_vec(&bit_pack, &mut v, &COMPRESSION_PREFERENCES) {
            Ok(_) => Ok(BitPack {
                inner: v.into_boxed_slice(),
                expected_buf_size: (cur.len() / 3) * 4,
            }),
            Err(e) => Err(e.to_string()),
        }
    }

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

// Utility functions. Largely copied from the Itertools and Bytemuck crates

/// An iterator which iterates two other iterators simultaneously
/// Copy pasted from the Iterator crate, and adapted for our purposes
#[must_use = "iterator adaptors are lazy and do nothing unless consumed"]
struct ZipEq<'a, I> {
    a: std::slice::Iter<'a, I>,
    b: std::slice::Iter<'a, I>,
}

fn zip_eq<'a, I>(i: &'a [I], j: &'a [I]) -> ZipEq<'a, I> {
    if i.len() != j.len() {
        unreachable!(
            "Iterators of zip_eq have different sizes: {}, {}",
            i.len(),
            j.len()
        );
    }
    ZipEq {
        a: i.iter(),
        b: j.iter(),
    }
}

impl<'a, I> Iterator for ZipEq<'a, I> {
    type Item = (&'a I, &'a I);

    fn next(&mut self) -> Option<Self::Item> {
        match (self.a.next(), self.b.next()) {
            (None, None) => None,
            (Some(a), Some(b)) => Some((a, b)),
            _ => unsafe { std::hint::unreachable_unchecked() },
        }
    }
}

// The functions below were copy pasted and adapted from the bytemuck crate:

#[inline]
fn pixels(img: &[u8]) -> &[[u8; 3]] {
    if img.len() % 3 != 0 {
        unreachable!("Calling pixels with a wrongly formatted image");
    }
    unsafe { core::slice::from_raw_parts(img.as_ptr().cast::<[u8; 3]>(), img.len() / 3) }
}

#[inline]
fn pixels_mut(img: &mut [u8]) -> &mut [[u8; 4]] {
    if img.len() % 4 != 0 {
        unreachable!("Calling pixels_mut with a wrongly formatted image");
    }
    unsafe { core::slice::from_raw_parts_mut(img.as_ptr() as *mut [u8; 4], img.len() / 4) }
}

#[cfg(test)]
mod tests {
    use super::BitPack;
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
        let compressed = BitPack::pack(&frame1, &frame2).unwrap();

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
            compressed.push(BitPack::pack(original.last().unwrap(), &original[0]).unwrap());
            for i in 1..20 {
                compressed.push(BitPack::pack(&original[i - 1], &original[i]).unwrap());
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

            let mut compressed = Vec::with_capacity(20);
            compressed.push(BitPack::pack(original.last().unwrap(), &original[0]).unwrap());
            for i in 1..20 {
                compressed.push(BitPack::pack(&original[i - 1], &original[i]).unwrap());
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
