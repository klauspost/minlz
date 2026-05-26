//! Tiny hand-rolled benchmark.  Run with `cargo run --release --example bench`.
//!
//! For real measurements use Criterion (stage A "DONE" criteria).

use std::env;
use std::fs;
use std::time::Instant;

use minlz::{decode, encode, Level};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 {
        let path = &args[1];
        let data = fs::read(path).expect("read file");
        // Match Go's `expand(data, n)` (minlz_test.go:1461) for direct
        // comparison with `BenchmarkTwain*1eN`.
        for &n in &[100_000usize, 1_000_000] {
            let expanded = expand(&data, n);
            run(&format!("file_{}", n), &expanded);
        }
        run("file_raw", &data);
        return;
    }
    run("english_x100", &pangram_x(100));
    run("english_x1000", &pangram_x(1000));
    run("english_x10000", &pangram_x(10_000));
    run("random_64k", &random_bytes(64 << 10));
    run("foobar_64k", &repeat_bytes(b"foobar ", 64 << 10));
    run("zeros_64k", &vec![0u8; 64 << 10]);
}

fn run(name: &str, src: &[u8]) {
    for level in [Level::Fastest, Level::Balanced, Level::Smallest] {
        run_at(name, src, level);
    }
}

fn run_at(name: &str, src: &[u8], level: Level) {
    let iters = pick_iters(src.len());
    let mut enc = Vec::with_capacity(src.len() + 16);
    let mut dec = Vec::with_capacity(src.len());

    // Warm-up.
    encode(&mut enc, src, level).unwrap();
    decode(&mut dec, &enc).unwrap();
    assert_eq!(dec, src);

    let t0 = Instant::now();
    for _ in 0..iters {
        encode(&mut enc, src, level).unwrap();
    }
    let enc_dt = t0.elapsed();

    let t1 = Instant::now();
    for _ in 0..iters {
        decode(&mut dec, &enc).unwrap();
    }
    let dec_dt = t1.elapsed();

    let mb_in = (src.len() as f64 * iters as f64) / (1024.0 * 1024.0);
    let enc_mbs = mb_in / enc_dt.as_secs_f64();
    let dec_mbs = mb_in / dec_dt.as_secs_f64();
    let ratio = enc.len() as f64 / src.len() as f64;
    let lvl_label = match level {
        Level::Fastest => "L1",
        Level::Balanced => "L2",
        Level::Smallest => "L3",
    };
    println!(
        "{name:18} {lvl_label}  src={:>9}B enc={:>9}B ratio={:5.3}  enc={:7.1} MB/s  dec={:7.1} MB/s  ({} iters)",
        src.len(),
        enc.len(),
        ratio,
        enc_mbs,
        dec_mbs,
        iters
    );
}

fn pick_iters(src_len: usize) -> usize {
    // Aim for ~256 MiB of work per measurement.
    ((256 << 20) / src_len.max(1)).max(8)
}

fn pangram_x(times: usize) -> Vec<u8> {
    let parts: &[&[u8]] = &[
        b"The quick brown fox jumps over the lazy dog. ",
        b"Pack my box with five dozen liquor jugs. ",
        b"How quickly daft jumping zebras vex. ",
        b"Sphinx of black quartz, judge my vow. ",
        b"The five boxing wizards jump quickly.",
    ];
    let pat_len: usize = parts.iter().map(|p| p.len()).sum();
    let mut out = Vec::with_capacity(pat_len * times);
    for _ in 0..times {
        for p in parts {
            out.extend_from_slice(p);
        }
    }
    out
}

fn repeat_bytes(pat: &[u8], n: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n);
    while v.len() < n {
        v.extend_from_slice(pat);
    }
    v.truncate(n);
    v
}

/// Port of Go's `expand(src, n)` from `minlz_test.go:1461` — repeats `src`
/// while XORing each copy with an incrementing counter so adjacent runs
/// don't compress trivially.
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

fn random_bytes(n: usize) -> Vec<u8> {
    // Reproducible xorshift.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(n);
    out
}
