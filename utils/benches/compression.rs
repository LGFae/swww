use criterion::{black_box, criterion_group, criterion_main, Criterion};
use utils::compression::{Compressor, Decompressor};

fn generate_data() -> (Box<[u8]>, Box<[u8]>) {
    let v1 = vec![120; 1920 * 1080 * 3];
    let mut v2 = v1.clone();

    const REGIONS: usize = 2000;
    let diff_bytes: usize = v2.len() / (REGIONS + 1);
    // Make different regions
    for i in 0..REGIONS {
        // With 100 different bytes total
        for j in 0..10 {
            v2[i * diff_bytes + j] = 100;
        }
        for j in 10..30 {
            v2[i * diff_bytes + j] = 200;
        }
        for j in 30..60 {
            v2[i * diff_bytes + j] = 20;
        }
        for j in 60..100 {
            v2[i * diff_bytes + j] = 30;
        }
    }

    (v1.into_boxed_slice(), v2.into_boxed_slice())
}

fn buf_from(slice: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    for pix in slice.chunks_exact(3) {
        v.extend_from_slice(pix);
        v.push(255);
    }
    v
}

pub fn compression_and_decompression(c: &mut Criterion) {
    let (prev, cur) = generate_data();

    let mut compressor = Compressor::new();
    let mut comp = c.benchmark_group("compression");
    comp.bench_function("Full", |b| {
        b.iter(|| {
            black_box(
                compressor
                    .compress(&prev, &cur, utils::ipc::PixelFormat::Xrgb)
                    .is_some(),
            )
        })
    });
    comp.finish();

    let mut decomp = c.benchmark_group("decompression 4 channels");
    let bitpack = compressor
        .compress(&prev, &cur, utils::ipc::PixelFormat::Xrgb)
        .unwrap();
    let mut canvas = buf_from(&prev);

    let mut decompressor = Decompressor::new();
    decomp.bench_function("Full", |b| {
        b.iter(|| {
            black_box(decompressor.decompress(&bitpack, &mut canvas, utils::ipc::PixelFormat::Xrgb))
        })
    });

    decomp.finish();
}

criterion_group!(compression, compression_and_decompression);
criterion_main!(compression);
