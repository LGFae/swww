use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use utils::comp_decomp::{BitPack, ReadiedPack};

fn generate_data() -> (Box<[u8]>, Box<[u8]>) {
    let v1 = vec![120; 1920 * 1080 * 4];
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

pub fn compression_and_decompression(c: &mut Criterion) {
    let (prev, cur) = generate_data();

    let mut comp = c.benchmark_group("compression");
    comp.bench_function("Full", |b| {
        b.iter_batched(
            || prev.clone(),
            |mut prev| {
                black_box(BitPack::pack(&mut prev, &cur).ok());
            },
            BatchSize::SmallInput,
        )
    });
    comp.bench_function("Partial", |b| {
        b.iter_batched(
            || prev.clone(),
            |mut prev| {
                black_box(ReadiedPack::new(&mut prev, &cur, |_, _, _| {}));
            },
            BatchSize::SmallInput,
        )
    });
    comp.finish();

    let mut decomp = c.benchmark_group("decompression");
    let mut p = prev.clone();
    let bitpack = BitPack::pack(&mut p, &cur).unwrap();
    let ready = bitpack.ready(prev.len());

    decomp.bench_function("Full", |b| {
        b.iter_batched(
            || prev.clone(),
            |mut prev| {
                black_box(bitpack.ready(prev.len()).unpack(&mut prev));
            },
            BatchSize::SmallInput,
        )
    });
    decomp.bench_function("Partial", |b| {
        b.iter_batched(
            || prev.clone(),
            |mut prev| {
                black_box(ready.unpack(&mut prev));
            },
            BatchSize::SmallInput,
        )
    });

    decomp.finish();
}

criterion_group!(compression, compression_and_decompression);
criterion_main!(compression);
