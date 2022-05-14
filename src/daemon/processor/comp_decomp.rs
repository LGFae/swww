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

use itertools::Itertools;
use lzzzz::lz4f;

lazy_static::lazy_static! {
    static ref COMPRESSION_PREFERENCES: lz4f::Preferences = lz4f::PreferencesBuilder::new()
            .favor_dec_speed(lz4f::FavorDecSpeed::Enabled)
            .block_size(lz4f::BlockSize::Max256KB)
            .compression_level(8)
            .build();
}

pub fn pack_bytes(prev: &[u8], cur: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity((prev.len() * 3) / 4);

    let prev_chunks = bytemuck::cast_slice::<u8, [u8; 4]>(prev);
    let cur_chunks = bytemuck::cast_slice::<u8, [u8; 4]>(cur);
    let mut iter = prev_chunks.iter().zip_eq(cur_chunks);

    let mut next_byte;
    let mut to_add = Vec::with_capacity(333); // 100 pixels
    while let Some((mut prev, mut cur)) = iter.next() {
        next_byte = 0;
        while prev == cur {
            next_byte += 1;
            if next_byte == u8::MAX {
                v.push(next_byte);
                next_byte = 0;
            }
            match iter.next() {
                None => {
                    v.push(next_byte);
                    v.push(0);
                    return v;
                }
                Some((p, c)) => {
                    prev = p;
                    cur = c;
                }
            }
        }
        v.push(next_byte);

        next_byte = 0;
        while prev != cur {
            to_add.extend_from_slice(&cur[0..3]);
            next_byte += 1;
            if next_byte == u8::MAX {
                v.push(next_byte);
                next_byte = 0;
            }
            match iter.next() {
                None => {
                    v.push(next_byte);
                    v.extend(to_add);
                    return v;
                }
                Some((p, c)) => {
                    prev = p;
                    cur = c;
                }
            }
        }
        v.push(next_byte);
        v.append(&mut to_add);
    }
    v
}

pub fn unpack_bytes(buf: &mut [u8], diff: &[u8]) {
    let buf_chunks = bytemuck::cast_slice_mut::<u8, [u8; 4]>(buf);
    let mut diff_idx = 0;
    let mut pix_idx = 0;
    let mut to_cpy = 0;
    while diff_idx < diff.len() {
        while diff[diff_idx] == u8::MAX {
            pix_idx += u8::MAX as usize;
            diff_idx += 1;
        }
        pix_idx += diff[diff_idx] as usize;
        diff_idx += 1;

        while diff[diff_idx] == u8::MAX {
            to_cpy += u8::MAX as usize;
            diff_idx += 1;
        }
        to_cpy += diff[diff_idx] as usize;
        diff_idx += 1;

        while to_cpy != 0 {
            unsafe {
                buf_chunks
                    .get_unchecked_mut(pix_idx)
                    .get_unchecked_mut(0..3)
                    .clone_from_slice(diff.get_unchecked(diff_idx..diff_idx + 3));
            }
            diff_idx += 3;
            pix_idx += 1;
            to_cpy -= 1;
        }
        pix_idx += 1;
    }
}

#[derive(Clone)]
/// Wrapper struct for compression and decompression. This makes sure we operating on a Vec<u8> with
/// the correct properties, simply by virtue of the type checking.
pub struct Packed {
    inner: Vec<u8>,
    /// This field will ensure we won't ever try to unpack the images on a buffer of the wrong size,
    /// which ultimately is what allows us to use unsafe in the diff_byte_header_copy_onto function
    expected_buf_size: usize,
}

impl Packed {
    ///Compresses a frame of animation by getting the difference between the previous and the
    ///current frame
    pub fn pack(prev: &[u8], curr: &[u8]) -> Self {
        let bit_pack = pack_bytes(prev, curr);
        let mut v = Vec::with_capacity(bit_pack.len() / 2);
        lzzzz::lz4f::compress_to_vec(&bit_pack, &mut v, &COMPRESSION_PREFERENCES).unwrap();
        v.shrink_to_fit();
        Packed {
            inner: v,
            expected_buf_size: prev.len(),
        }
    }

    pub fn unpack(&self, buf: &mut [u8]) {
        if buf.len() == self.expected_buf_size {
            let mut v = Vec::with_capacity(self.inner.len() * 3);
            lz4f::decompress_to_vec(&self.inner, &mut v).unwrap();
            unpack_bytes(buf, &v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Packed;
    use rand::prelude::random;

    #[test]
    //Use this when annoying problems show up
    fn should_compress_and_decompress_to_same_info_small() {
        let frame1 = [1, 2, 3, 4, 5, 6, 7, 8];
        let frame2 = [1, 2, 3, 4, 8, 7, 6, 5];
        let compressed = Packed::pack(&frame1, &frame2);

        let mut buf = frame1;
        compressed.unpack(&mut buf);
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
            compressed.push(Packed::pack(original.last().unwrap(), &original[0]));
            for i in 1..20 {
                compressed.push(Packed::pack(&original[i - 1], &original[i]));
            }

            let mut buf = original.last().unwrap().clone();
            for i in 0..20 {
                compressed[i].unpack(&mut buf);
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
            compressed.push(Packed::pack(original.last().unwrap(), &original[0]));
            for i in 1..20 {
                compressed.push(Packed::pack(&original[i - 1], &original[i]));
            }

            let mut buf = original.last().unwrap().clone();
            for i in 0..20 {
                compressed[i].unpack(&mut buf);
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
