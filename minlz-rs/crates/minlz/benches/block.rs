//! Criterion benchmarks for the block codec.
//!
//! Mirrors Go's `BenchmarkTwainEncode1eN` / `BenchmarkTwainDecode1eN` shape
//! (`benchmarks_test.go:123` / `:108`).  Reads
//! `testdata/Mark.Twain-Tom.Sawyer.txt` from the upstream Go repo and runs
//! encode + decode at each level for several scaled-up corpus sizes.
//!
//! Run with:
//!
//! ```
//! cargo bench --bench block
//! cargo bench --bench block -- --quick      # rough estimate, ~5 s/case
//! cargo bench --bench block twain_decode_1e5/L1   # one case
//! ```
//!
//! HTML reports land in `target/criterion/`.
//!
//! The reference Go numbers (from `go test -tags=noasm -bench=BenchmarkTwain*`)
//! are listed in the README so a CI job can compare percentages.

#![allow(missing_docs)] // criterion_group! emits an undocumented fn

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use minlz::{decode, encode, Level};
use std::path::PathBuf;
use std::time::Duration;

/// Locate the Twain corpus.  Honours the `MINLZ_TESTDATA` env override so
/// the bench can be run from a copy of the repo that doesn't include
/// `testdata/`.
fn load_twain() -> Vec<u8> {
    let candidates: Vec<PathBuf> = {
        let mut v = Vec::new();
        if let Ok(d) = std::env::var("MINLZ_TESTDATA") {
            v.push(PathBuf::from(d).join("Mark.Twain-Tom.Sawyer.txt"));
        }
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        v.push(manifest.join("../../../testdata/Mark.Twain-Tom.Sawyer.txt"));
        v.push(manifest.join("../../testdata/Mark.Twain-Tom.Sawyer.txt"));
        v
    };
    for p in &candidates {
        if let Ok(b) = std::fs::read(p) {
            return b;
        }
    }
    panic!(
        "could not locate Mark.Twain-Tom.Sawyer.txt; tried: {candidates:?}.  \
         Set MINLZ_TESTDATA=<dir> to override."
    );
}

/// Port of Go's `expand(src, n)` (`minlz_test.go:1461`).  Tiles `src` to
/// fill an `n`-byte buffer, XOR-ing each copy with an incrementing counter
/// so adjacent runs don't compress trivially.
fn expand(src: &[u8], n: usize) -> Vec<u8> {
    let mut dst = vec![0u8; n];
    let mut cnt: u8 = 0;
    let mut start = 0;
    while start < n {
        let end = (start + src.len()).min(n);
        let copy_len = end - start;
        dst[start..end].copy_from_slice(&src[..copy_len]);
        for b in dst[start..end].iter_mut() {
            *b ^= cnt;
        }
        start += src.len();
        cnt = cnt.wrapping_add(1);
    }
    dst
}

fn level_label(l: Level) -> &'static str {
    match l {
        Level::Fastest => "L1",
        Level::Balanced => "L2",
        Level::Smallest => "L3",
    }
}

fn bench_encode(c: &mut Criterion) {
    let twain = load_twain();
    let sizes: &[(usize, &str)] = &[(100_000, "1e5"), (1_000_000, "1e6")];
    let levels = [Level::Fastest, Level::Balanced, Level::Smallest];

    let mut g = c.benchmark_group("twain_encode");
    g.measurement_time(Duration::from_secs(8));
    g.warm_up_time(Duration::from_secs(2));
    for (n, label) in sizes {
        let src = expand(&twain, *n);
        g.throughput(Throughput::Bytes(*n as u64));
        for &level in &levels {
            let id = BenchmarkId::new(*label, level_label(level));
            g.bench_with_input(id, &src, |b, src| {
                let mut buf = Vec::with_capacity(src.len() + 16);
                b.iter(|| {
                    encode(&mut buf, src, level).expect("encode");
                });
            });
        }
    }
    g.finish();
}

fn bench_decode(c: &mut Criterion) {
    let twain = load_twain();
    let sizes: &[(usize, &str)] = &[(100_000, "1e5"), (1_000_000, "1e6")];
    let levels = [Level::Fastest, Level::Balanced, Level::Smallest];

    let mut g = c.benchmark_group("twain_decode");
    g.measurement_time(Duration::from_secs(8));
    g.warm_up_time(Duration::from_secs(2));
    for (n, label) in sizes {
        let src = expand(&twain, *n);
        g.throughput(Throughput::Bytes(*n as u64));
        for &level in &levels {
            let mut enc = Vec::new();
            encode(&mut enc, &src, level).expect("encode for bench prep");
            let id = BenchmarkId::new(*label, level_label(level));
            g.bench_with_input(id, &enc, |b, enc| {
                let mut buf = Vec::with_capacity(src.len() + 16);
                b.iter(|| {
                    decode(&mut buf, enc).expect("decode");
                });
            });
        }
    }
    g.finish();
}

criterion_group!(benches, bench_encode, bench_decode);
criterion_main!(benches);
