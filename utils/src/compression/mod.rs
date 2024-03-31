//! # Compression utilities
//!
//! Our compression strategy is documented in `comp/mod.rs`

use comp::pack_bytes;
use decomp::{unpack_bytes_3channels, unpack_bytes_4channels};
use std::ffi::{c_char, c_int};

use bitcode::{Decode, Encode};

use crate::ipc::PixelFormat;
mod comp;
mod cpu;
mod decomp;

/// extracted from lz4.h
const LZ4_MAX_INPUT_SIZE: usize = 0x7E000000;

extern "C" {
    /// # Safety
    ///
    /// This is guaranteed to succeed if `dst_cap >= LZ4_compressBound`.
    fn LZ4_compress_HC(
        src: *const c_char,
        dst: *mut c_char,
        src_len: c_int,
        dst_cap: c_int,
        comp_level: c_int,
    ) -> c_int;

    /// # Safety
    ///
    /// Fails when src is malformed, or dst_cap is insufficient.
    fn LZ4_decompress_safe(
        src: *const c_char,
        dst: *mut c_char,
        compressed_size: c_int,
        dst_cap: c_int,
    ) -> c_int;

    /// # Safety
    ///
    /// Only works for input_size <= LZ4_MAX_INPUT_SIZE.
    fn LZ4_compressBound(input_size: c_int) -> c_int;
}

/// This struct represents the cached difference between the previous frame and the next
#[derive(Encode, Decode)]
pub struct BitPack {
    inner: Box<[u8]>,
    /// This field will ensure we won't ever try to unpack the images on a buffer of the wrong size,
    /// which ultimately is what allows us to use unsafe in the unpack_bytes function
    expected_buf_size: usize,

    compressed_size: i32,
}

/// Struct responsible for compressing our data. We use it to cache vector extensions that might
/// speed up compression
#[derive(Default)]
pub struct Compressor {
    buf: Vec<u8>,
}

impl Compressor {
    #[inline]
    pub fn new() -> Self {
        cpu::init();
        Self { buf: Vec::new() }
    }

    /// Compresses a frame of animation by getting the difference between the previous and the
    /// current frame, and then running lz4
    ///
    /// # Returns:
    ///   * None if the two frames are identical
    ///   * Some(bytes) if compression yielded something
    ///
    /// # Panics:
    ///   * `prev.len() != cur.len()`
    ///   * the len of the diff buffer is larger than 0x7E000000. In practice, this can only
    ///   happen for 64k monitors and beyond
    #[inline]
    pub fn compress(
        &mut self,
        prev: &[u8],
        cur: &[u8],
        pixel_format: PixelFormat,
    ) -> Option<BitPack> {
        assert_eq!(
            prev.len(),
            cur.len(),
            "swww cannot currently deal with animations whose frames have different sizes!"
        );

        self.buf.clear();
        // SAFETY: the above assertion ensures prev.len() and cur.len() are equal, as needed
        unsafe { pack_bytes(prev, cur, &mut self.buf) }

        if self.buf.is_empty() {
            return None;
        }

        // This should only be a problem with 64k monitors and beyond, (hopefully) far into the future
        assert!(
            self.buf.len() <= LZ4_MAX_INPUT_SIZE,
            "frame is too large! cannot compress with LZ4!"
        );

        // SAFETY: the above assertion ensures this will never fail
        let size = unsafe { LZ4_compressBound(self.buf.len() as c_int) } as usize;
        let mut v = vec![0; size];
        // SAFETY: we've ensured above that size >= LZ4_compressBound, so this should always work
        let n = unsafe {
            LZ4_compress_HC(
                self.buf.as_ptr().cast(),
                v.as_mut_ptr() as _,
                self.buf.len() as c_int,
                size as c_int,
                9,
            ) as usize
        };
        v.truncate(n);

        let expected_buf_size = if pixel_format.channels() == 3 {
            cur.len()
        } else {
            (cur.len() / 3) * 4
        };

        Some(BitPack {
            inner: v.into_boxed_slice(),
            expected_buf_size,
            compressed_size: self.buf.len() as i32,
        })
    }
}

pub struct Decompressor {
    /// this pointer stores an inner buffer we need to speed up decompression
    /// note we explicitly do not care about its length
    ptr: std::ptr::NonNull<u8>,
    cap: usize,
}

impl Drop for Decompressor {
    #[inline]
    fn drop(&mut self) {
        if self.cap > 0 {
            let layout = std::alloc::Layout::array::<u8>(self.cap).unwrap();
            unsafe { std::alloc::dealloc(self.ptr.as_ptr(), layout) }
        }
    }
}

impl Decompressor {
    #[allow(clippy::new_without_default)]
    #[inline]
    pub fn new() -> Self {
        cpu::init();
        Self {
            ptr: std::ptr::NonNull::dangling(),
            cap: 0,
        }
    }

    fn ensure_capacity(&mut self, goal: usize) {
        if self.cap >= goal {
            return;
        }

        let ptr = if self.cap == 0 {
            let layout = std::alloc::Layout::array::<u8>(goal).unwrap();
            let p = unsafe { std::alloc::alloc(layout) };
            match std::ptr::NonNull::new(p) {
                Some(p) => p,
                None => std::alloc::handle_alloc_error(layout),
            }
        } else {
            let old_layout = std::alloc::Layout::array::<u8>(self.cap).unwrap();
            let new_layout = std::alloc::Layout::array::<u8>(goal).unwrap();
            let p =
                unsafe { std::alloc::realloc(self.ptr.as_ptr(), old_layout, new_layout.size()) };
            match std::ptr::NonNull::new(p) {
                Some(p) => p,
                None => std::alloc::handle_alloc_error(new_layout),
            }
        };

        self.ptr = ptr;
        self.cap = goal;
    }

    ///returns whether unpacking was successful. Note it can only fail if `buf.len() !=
    ///expected_buf_size`
    #[inline]
    pub fn decompress(
        &mut self,
        bitpack: &BitPack,
        buf: &mut [u8],
        pixel_format: PixelFormat,
    ) -> Result<(), String> {
        if buf.len() != bitpack.expected_buf_size {
            return Err(format!(
                "buf has len {}, but expected len is {}",
                buf.len(),
                bitpack.expected_buf_size
            ));
        }

        self.ensure_capacity(bitpack.compressed_size as usize);

        // SAFETY: errors will never happen because BitPacked is *always* only produced
        // with correct lz4 compression, and ptr has the necessary capacity
        let size = unsafe {
            LZ4_decompress_safe(
                bitpack.inner.as_ptr() as _,
                self.ptr.as_ptr() as _,
                bitpack.inner.len() as c_int,
                bitpack.compressed_size as c_int,
            )
        };

        if size != bitpack.compressed_size {
            return Err("BitPack is malformed!".to_string());
        }

        // SAFETY: the call to self.ensure_capacity guarantees the pointer has the necessary size
        // to hold all the data
        let v = unsafe {
            std::slice::from_raw_parts_mut(self.ptr.as_ptr(), bitpack.compressed_size as usize)
        };

        if pixel_format.can_copy_directly_onto_wl_buffer() {
            unpack_bytes_3channels(buf, v);
        } else {
            unpack_bytes_4channels(buf, v);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::prelude::random;

    const FORMATS: [PixelFormat; 2] = [PixelFormat::Xrgb, PixelFormat::Rgb];

    fn buf_from(slice: &[u8], original_channels: usize) -> Vec<u8> {
        if original_channels == 3 {
            return slice.to_vec();
        }
        let mut v = Vec::new();
        for pix in slice.chunks_exact(3) {
            v.extend_from_slice(pix);
            v.push(255);
        }
        v
    }

    #[test]
    //Use this when annoying problems show up
    fn small() {
        for format in FORMATS {
            let frame1 = [1, 2, 3, 4, 5, 6];
            let frame2 = [1, 2, 3, 6, 5, 4];
            let compressed = Compressor::new()
                .compress(&frame1, &frame2, format)
                .unwrap();

            let mut buf = buf_from(&frame1, format.channels().into());
            Decompressor::new()
                .decompress(&compressed, &mut buf, format)
                .unwrap();
            for i in 0..2 {
                for j in 0..3 {
                    assert_eq!(
                        frame2[i * 3 + j],
                        buf[i * format.channels() as usize + j],
                        "\nframe2: {frame2:?}, buf: {buf:?}\n"
                    );
                }
            }
        }
    }

    #[test]
    fn total_random() {
        for format in FORMATS.into_iter() {
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
                let mut compressor = Compressor::new();
                let mut decompressor = Decompressor::new();
                compressed.push(
                    compressor
                        .compress(original.last().unwrap(), &original[0], format)
                        .unwrap(),
                );
                for i in 1..20 {
                    compressed.push(
                        compressor
                            .compress(&original[i - 1], &original[i], format)
                            .unwrap(),
                    );
                }

                let mut buf = buf_from(original.last().unwrap(), format.channels().into());
                for i in 0..20 {
                    decompressor
                        .decompress(&compressed[i], &mut buf, format)
                        .unwrap();
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
                        l += !format.can_copy_directly_onto_wl_buffer() as usize;
                    }
                }
            }
        }
    }

    #[test]
    fn full() {
        for format in FORMATS {
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

                let mut compressor = Compressor::new();
                let mut decompressor = Decompressor::new();
                let mut compressed = Vec::with_capacity(20);
                compressed.push(
                    compressor
                        .compress(original.last().unwrap(), &original[0], format)
                        .unwrap(),
                );
                for i in 1..20 {
                    compressed.push(
                        compressor
                            .compress(&original[i - 1], &original[i], format)
                            .unwrap(),
                    );
                }

                let mut buf = buf_from(original.last().unwrap(), format.channels().into());
                for i in 0..20 {
                    decompressor
                        .decompress(&compressed[i], &mut buf, format)
                        .unwrap();
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
                        l += !format.can_copy_directly_onto_wl_buffer() as usize;
                    }
                }
            }
        }
    }
}
