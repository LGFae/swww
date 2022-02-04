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

use lz4_flex;

///Note: in its current form, this will panic when len of the arrays is not divisible by 8
fn diff_byte_header(prev: &[u8], curr: &[u8]) -> Vec<u8> {
    let mut last_zero_header = 0;
    let mut vec = Vec::with_capacity(56 + (prev.len() * 49) / 64);
    let mut to_add = Vec::with_capacity(8 * 6);
    let mut header = 0;
    let mut i = 0;
    let mut k = 0;
    for chunk in prev.chunks_exact(8) {
        if chunk[0..3] != curr[i..i + 3] || chunk[4..7] != curr[i + 4..i + 7] {
            to_add.extend_from_slice(&curr[i..i + 3]);
            to_add.extend_from_slice(&curr[i + 4..i + 7]);
            header |= 0x80 >> k;
        }

        i += 8;
        k += 1;
        if k == 8 {
            if header == 0 && last_zero_header == 0 {
                last_zero_header = vec.len();
            } else if header != 0 && last_zero_header != 0 {
                last_zero_header = 0;
            }
            vec.push(header);
            vec.extend_from_slice(&to_add);
            header = 0;
            to_add.clear();
            k = 0;
        }
    }
    if header > 0 {
        //Add whatever's left
        vec.push(header);
        vec.extend_from_slice(&to_add);
    } else if last_zero_header != 0 {
        //If there's nothing left, we should remove the trailing 0 headers:
        vec.truncate(last_zero_header);
    }
    vec.shrink_to_fit();
    vec
}

fn diff_byte_header_copy_onto(buf: &mut [u8], diff: &[u8]) {
    let mut byte_idx = 0;
    let mut pix_idx = 0;
    while byte_idx < diff.len() {
        let header = diff[byte_idx];
        byte_idx += 1;
        for j in (0..8).rev() {
            if (header >> j) % 2 == 1 {
                buf[pix_idx * 4..pix_idx * 4 + 3].clone_from_slice(&diff[byte_idx..byte_idx + 3]);
                buf[pix_idx * 4 + 4..pix_idx * 4 + 7]
                    .clone_from_slice(&diff[byte_idx + 3..byte_idx + 6]);
                byte_idx += 6;
            }
            pix_idx += 2;
        }
    }
}

///Compresses by first doing our custom bitpacking compression, and then using lz4
pub fn mixed_comp(prev: &[u8], curr: &[u8]) -> Vec<u8> {
    let bit_pack = diff_byte_header(prev, curr);
    lz4_flex::compress_prepend_size(&bit_pack)
}

///Decompresses by first undoing lz4, then undoing our custom bitpacking
pub fn mixed_decomp(buf: &mut [u8], diff: &[u8]) {
    let diff = lz4_flex::decompress_size_prepended(diff).unwrap();
    diff_byte_header_copy_onto(buf, &diff);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::prelude::*;

    #[test]
    fn should_make_byte_header() {
        let original = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let copy = original.clone();
        let different = vec![8, 7, 6, 5, 4, 3, 2, 1];

        let diff_copy = diff_byte_header(&original, &copy);
        assert_eq!(
            diff_copy.len(),
            0,
            "Since it's equal, it should have no bytes"
        );

        let diff_diff = diff_byte_header(&original, &different);
        assert_eq!(
            diff_diff.len(),
            7,
            "Since it's different, it should have 7 bytes. 1 for the header, 6 for the colors"
        );
        assert_eq!(
            diff_diff[0], 0x80,
            "Since it's different in the first position, header should be 1000 0000"
        );

        assert_eq!(
            diff_diff[1..4],
            different[0..3],
            "We should have stored the different bytes"
        );

        assert_eq!(
            diff_diff[4..7],
            different[4..7],
            "We should have stored the different bytes"
        );
    }
    #[test]
    //Use this when annoying problems show up
    fn should_compress_and_decompress_to_small() {
        let frame1 = [1, 2, 3, 4, 5, 6, 7, 8];
        let frame2 = [1, 2, 3, 4, 8, 7, 6, 5];
        let compreesed = diff_byte_header(&frame1, &frame2);
        assert_eq!(compreesed, [0x80, 1, 2, 3, 8, 7, 6]);

        let mut buf = frame1;
        diff_byte_header_copy_onto(&mut buf, &compreesed);
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
            compressed.push(diff_byte_header(original.last().unwrap(), &original[0]));
            for i in 1..20 {
                compressed.push(diff_byte_header(&original[i - 1], &original[i]));
            }

            let mut buf = original.last().unwrap().clone();
            for i in 0..20 {
                diff_byte_header_copy_onto(&mut buf, &compressed[i]);
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
