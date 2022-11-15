//! # Compression Strategy
//!
//! For every pixel, we drop the alpha part; I don't think anyone will use transparency for a
//! background (nor if it even makes sense)
//!
//! For what's left, we store only the difference from the last frame to this one. We do that as
//! follows:
//! * First, we count how many pixels didn't change. We store that value as a u8.
//!   Everytime the u8 hits the max (i.e. 255, or 0xFF), we push in onto the vector
//!   and restart the counting.
//! * Once we find a pixel that has changed, we count, starting from that one, how many changed,
//!   the same way we counted above (i.e. store as u8, everytime it hits the max push and restart
//!   the counting)
//! * Then, we store all the new bytes.
//! * Start from the top until we are done with the image
//!

use lzzzz::lz4f;

lazy_static::lazy_static! {
    static ref COMPRESSION_PREFERENCES: lz4f::Preferences = lz4f::PreferencesBuilder::new()
            .block_size(lz4f::BlockSize::Max256KB)
            .compression_level(9)
            .build();
}

/// This calculates the difference between the current(cur) frame and the next(goal).
/// The closure you pass is run at every difference. It dictates the update logic of the current
/// frame. With that, you can control whether all different pixels changed are updated, or only the
/// ones at a certain position. It is meant to be used primarily when writting transitions
fn pack_bytes<F>(cur: &mut [u8], goal: &[u8], mut f: F) -> Box<[u8]>
where
    F: FnMut(&mut [u8; 4], &[u8; 4], usize),
{
    let mut v = Vec::with_capacity((goal.len() * 5) / 8);

    let mut iter = zip_eq(pixels_mut(cur), pixels(goal)).enumerate();
    let mut to_add = Vec::with_capacity(333); // 100 pixels
    while let Some((mut i, (mut cur, mut goal))) = iter.next() {
        let mut equals = 0;
        while cur == goal {
            equals += 1;
            match iter.next() {
                None => return v.into_boxed_slice(),
                Some((j, (c, g))) => {
                    i = j;
                    cur = c;
                    goal = g;
                }
            }
        }

        let mut diffs = 0;
        while cur != goal {
            f(cur, goal, i);
            to_add.extend_from_slice(&cur[0..3]);
            diffs += 1;
            match iter.next() {
                None => break,
                Some((j, (c, g))) => {
                    i = j;
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
pub struct BitPack {
    inner: Box<[u8]>,
}

impl BitPack {
    /// Compresses a frame of animation by getting the difference between the previous and the
    /// current frame.
    /// IMPORTANT: this will change `prev` into `cur`, that's why it needs to be 'mut'
    pub fn pack(prev: &mut [u8], cur: &[u8]) -> Self {
        let bit_pack = pack_bytes(prev, cur, |old, new, _| *old = *new);
        let mut v = Vec::with_capacity(bit_pack.len() / 2);
        lzzzz::lz4f::compress_to_vec(&bit_pack, &mut v, &COMPRESSION_PREFERENCES).unwrap();
        BitPack {
            inner: v.into_boxed_slice(),
        }
    }

    /// Produces a `ReadiedPack`, which can be sent through a channel to be unpacked later
    pub fn ready(&self, expected_buf_size: usize) -> ReadiedPack {
        let mut v = Vec::with_capacity(self.inner.len() * 3);
        lz4f::decompress_to_vec(&self.inner, &mut v).unwrap();
        ReadiedPack {
            inner: v.into_boxed_slice(),
            expected_buf_size,
        }
    }
}

/// This is what we send through the channel to be drawn
pub struct ReadiedPack {
    inner: Box<[u8]>,
    /// This field will ensure we won't ever try to unpack the images on a buffer of the wrong size,
    /// which ultimately is what allows us to use unsafe in the unpack_bytes function
    expected_buf_size: usize,
}

impl ReadiedPack {
    /// This should only be used in the transitions. For caching the animation frames, use the
    /// Bitpack struct
    ///
    /// The `f` runs at every different pixel found, iterating through the three colors BGR. Its
    /// parameters are:
    ///
    /// * First -> old img byte, that has to change to the new one according to the transition logic
    /// * Second -> new img byte. This stays constant
    /// * Third -> the pixel's position in the image. This can be used to make more complex
    ///   transition logic
    pub fn new<F>(cur: &mut [u8], goal: &[u8], f: F) -> Self
    where
        F: FnMut(&mut [u8; 4], &[u8; 4], usize),
    {
        let bit_pack = pack_bytes(cur, goal, f);
        ReadiedPack {
            inner: bit_pack,
            expected_buf_size: cur.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn unpack(&self, buf: &mut [u8]) {
        if buf.len() == self.expected_buf_size {
            unpack_bytes(buf, &self.inner);
        }
    }
}

// Utility functions. Largely copied from the Itertools and Bytemuck crates

/// An iterator which iterates two other iterators simultaneously
/// Copy pasted from the Iterator crate, and adapted for our purposes
#[must_use = "iterator adaptors are lazy and do nothing unless consumed"]
struct ZipEq<'a, I> {
    a: std::slice::IterMut<'a, I>,
    b: std::slice::Iter<'a, I>,
}

fn zip_eq<'a, I>(i: &'a mut [I], j: &'a [I]) -> ZipEq<'a, I> {
    if i.len() != j.len() {
        unreachable!(
            "Iterators of zip_eq have different sizes: {}, {}",
            i.len(),
            j.len()
        );
    }
    ZipEq {
        a: i.iter_mut(),
        b: j.iter(),
    }
}

impl<'a, I> Iterator for ZipEq<'a, I> {
    type Item = (&'a mut I, &'a I);

    fn next(&mut self) -> Option<Self::Item> {
        match (self.a.next(), self.b.next()) {
            (None, None) => None,
            (Some(a), Some(b)) => Some((a, b)),
            _ => unsafe { std::hint::unreachable_unchecked() },
        }
    }
}

// The functions bellow were copy pasted and adapted from the bytemuck crate:

#[inline]
fn pixels(img: &[u8]) -> &[[u8; 4]] {
    if img.len() % 4 != 0 {
        unreachable!("Calling pixels with a wrongly formated image");
    }
    unsafe { core::slice::from_raw_parts(img.as_ptr().cast::<[u8; 4]>(), img.len() / 4) }
}

#[inline]
fn pixels_mut(img: &mut [u8]) -> &mut [[u8; 4]] {
    if img.len() % 4 != 0 {
        unreachable!("Calling pixels_mut with a wrongly formated image");
    }
    unsafe { core::slice::from_raw_parts_mut(img.as_ptr() as *mut [u8; 4], img.len() / 4) }
}

#[cfg(test)]
mod tests {
    use super::BitPack;
    use rand::prelude::random;

    #[test]
    //Use this when annoying problems show up
    fn should_compress_and_decompress_to_same_info_small() {
        let frame1 = [1, 2, 3, 4, 5, 6, 7, 8];
        let frame2 = [1, 2, 3, 4, 8, 7, 6, 5];
        let compressed = BitPack::pack(&mut frame1.clone(), &frame2);

        let mut buf = frame1;
        let readied = compressed.ready(8);
        readied.unpack(&mut buf);
        for i in 0..2 {
            for j in 0..3 {
                assert_eq!(
                    frame2[i * 4 + j],
                    buf[i * 4 + j],
                    "\nframe2: {:?}, buf: {:?}\n",
                    frame2,
                    buf
                );
            }
        }
    }

    #[test]
    fn should_compress_and_decompress_to_same_info() {
        for _ in 0..10 {
            let mut original = Vec::with_capacity(20);
            for _ in 0..20 {
                let mut v = Vec::with_capacity(4000);
                for _ in 0..4000 {
                    v.push(random::<u8>());
                }
                original.push(v);
            }

            let mut compressed = Vec::with_capacity(20);
            compressed.push(BitPack::pack(
                &mut original.last().unwrap().clone(),
                &original[0],
            ));
            for i in 1..20 {
                compressed.push(BitPack::pack(&mut original[i - 1].clone(), &original[i]));
            }

            let mut buf = original.last().unwrap().clone();
            for i in 0..20 {
                let readied = compressed[i].ready(4000);
                readied.unpack(&mut buf);
                let mut j = 0;
                while j < 4000 {
                    for k in 0..3 {
                        assert_eq!(buf[j + k], original[i][j + k], "Failed at index: {}", j + k);
                    }
                    j += 4;
                }
            }
        }
    }

    #[test]
    fn should_compress_and_decompress_to_same_info_with_equal_data() {
        for _ in 0..10 {
            let mut original = Vec::with_capacity(20);
            for _ in 0..20 {
                let mut v = Vec::with_capacity(4000);
                for _ in 0..3000 {
                    v.push(random::<u8>());
                }
                for i in 0..1000 {
                    v.push((i % 255) as u8);
                }
                original.push(v);
            }

            let mut compressed = Vec::with_capacity(20);
            compressed.push(BitPack::pack(
                &mut original.last().unwrap().clone(),
                &original[0],
            ));
            for i in 1..20 {
                compressed.push(BitPack::pack(&mut original[i - 1].clone(), &original[i]));
            }

            let mut buf = original.last().unwrap().clone();
            for i in 0..20 {
                let readied = compressed[i].ready(4000);
                readied.unpack(&mut buf);
                let mut j = 0;
                while j < 4000 {
                    for k in 0..3 {
                        assert_eq!(buf[j + k], original[i][j + k], "Failed at index: {}", j + k);
                    }
                    j += 4;
                }
            }
        }
    }
}
