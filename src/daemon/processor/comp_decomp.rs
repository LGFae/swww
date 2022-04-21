//! # Compression Strategy
//!
//! For every pixel, we drop the alpha part; I don't think anyone will use transparency for a
//! background (nor if it even makes sense)
//!
//! For what's left, we store only the difference from the last frame to this one. We do that as
//! follows:
//! * We store a byte header, which indicate which pixels changed. For example, 1010 0000 would
//! mean that, from the current position, pixels 1 and 2 and 5 and 6 changed.
//! * After the header, we store the pixels we indicated.
//!
//! NOTE THAT EVERY BIT IN THE HEADER CORRESPONDS TO 2, NOT 1, PIXELS.
//!
//! Finally, after all that, we use Lz4 to compress the result.
//!
//! # Decompressing
//!
//! For decompression, we must do everything backwards:
//! * First we decompress with Lz4
//! * Then we replace in the frame the difference we stored before.
//!
//! Note that the frame itself has 4 byte pixels, so take that into account when copying the
//! difference.

use lzzzz::lz4f;

fn diff_byte_header(prev: &[u8], curr: &[u8]) -> Vec<u8> {
    let remainder = prev.len() - prev.len() % 64;
    let prev_chunks = bytemuck::cast_slice::<u8, [u8; 64]>(&prev[0..remainder]);
    let curr_chunks = bytemuck::cast_slice::<u8, [u8; 64]>(&curr[0..remainder]);
    let remainder = bytemuck::cast_slice::<u8, [u8; 8]>(&prev[remainder..])
        .iter()
        .zip(bytemuck::cast_slice::<u8, [u8; 8]>(&curr[remainder..]));

    let mut last_zero_header = 0;
    let mut vec = Vec::with_capacity(56 + (prev.len() * 49) / 64);
    let mut header_idx = 0;
    for (prev, curr) in prev_chunks.iter().zip(curr_chunks) {
        vec.push(0);
        for (k, (prev, curr)) in bytemuck::cast_slice::<u8, [u8; 8]>(prev)
            .iter()
            .zip(bytemuck::cast_slice::<u8, [u8; 8]>(curr))
            .enumerate()
        {
            if prev != curr {
                vec[header_idx] |= 0x80 >> k;
                vec.extend_from_slice(&curr[0..3]);
                vec.extend_from_slice(&curr[4..7]);
            }
        }

        if vec[header_idx] != 0 {
            last_zero_header = vec.len();
        }
        header_idx = vec.len();
    }
    vec.push(0);
    for (k, (prev, curr)) in remainder.enumerate() {
        if prev != curr {
            vec[header_idx] |= 0x80 >> k;
            vec.extend_from_slice(&curr[0..3]);
            vec.extend_from_slice(&curr[4..7]);
        }
    }
    //Remove the trailing 0 headers, if any:
    if vec[header_idx] == 0 {
        vec.truncate(last_zero_header);
    }

    vec
}

fn diff_byte_header_copy_onto(buf: &mut [u8], diff: &[u8]) {
    let mut byte_idx = 0;
    let mut pix_idx = 0;
    while byte_idx < diff.len() {
        let header = diff[byte_idx];
        byte_idx += 1;
        if header != 0 {
            for j in (0..8).rev() {
                if (header >> j) % 2 == 1 {
                    // This should always be safe, since we check if the buffer is big enough
                    // before unpacking (see the Packed struct)
                    // Going unsafe here makes decompression significantly faster in my
                    // (Horus645) computer
                    unsafe {
                        buf.get_unchecked_mut(pix_idx..pix_idx + 3)
                            .clone_from_slice(diff.get_unchecked(byte_idx..byte_idx + 3));
                        buf.get_unchecked_mut(pix_idx + 4..pix_idx + 7)
                            .clone_from_slice(diff.get_unchecked(byte_idx + 3..byte_idx + 6));
                    }
                    byte_idx += 6;
                }
                pix_idx += 8;
            }
        } else {
            pix_idx += 64;
        }
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
        let bit_pack = diff_byte_header(prev, curr);
        let mut v = Vec::with_capacity(bit_pack.len() / 2);
        let prefs = lz4f::PreferencesBuilder::new()
            .favor_dec_speed(lz4f::FavorDecSpeed::Enabled)
            .block_size(lz4f::BlockSize::Max256KB)
            .compression_level(8)
            .build();
        lzzzz::lz4f::compress_to_vec(&bit_pack, &mut v, &prefs).unwrap();
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
            diff_byte_header_copy_onto(buf, &v);
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
