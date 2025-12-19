//! Simple working benchmarks for MinLZ without external dependencies
//!
//! This benchmark suite provides basic performance testing without requiring
//! external test files or network downloads.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use minlz::{decode, encode, LEVEL_BALANCED, LEVEL_FASTEST, LEVEL_SMALLEST};
use std::time::Duration;

const KB: usize = 1024;
const MB: usize = 1024 * 1024;

/// Generate random data for testing
fn generate_random_data(size: usize, seed: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(size);
    let mut state = seed;

    for _ in 0..size {
        // Simple LCG for reproducible random data
        state = state.wrapping_mul(1103515245).wrapping_add(12345);
        data.push((state >> 8) as u8);
    }

    data
}

/// Generate repetitive data for testing
fn generate_repetitive_data(size: usize) -> Vec<u8> {
    let pattern = b"Hello, world! This is a test pattern for MinLZ compression. ";
    let mut data = Vec::with_capacity(size);

    while data.len() < size {
        let remaining = size - data.len();
        if remaining >= pattern.len() {
            data.extend_from_slice(pattern);
        } else {
            data.extend_from_slice(&pattern[..remaining]);
        }
    }

    data
}

/// Benchmark encoding random data at different compression levels
fn bench_encode_random(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_random");
    group.measurement_time(Duration::from_secs(5));

    let data_1mb = generate_random_data(MB, 12345);
    group.throughput(Throughput::Bytes(data_1mb.len() as u64));

    for (level, level_name) in [
        (LEVEL_FASTEST, "level-1"),
        (LEVEL_BALANCED, "level-2"),
        (LEVEL_SMALLEST, "level-3"),
    ] {
        group.bench_with_input(
            BenchmarkId::new(format!("1MB_{}", level_name), data_1mb.len()),
            &data_1mb,
            |b, data| {
                let mut output = Vec::new();
                b.iter(|| {
                    output.clear();
                    encode(&mut output, data, level).expect("encoding failed");
                });
            },
        );
    }

    group.finish();
}

/// Benchmark encoding repetitive data at different compression levels
fn bench_encode_repetitive(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_repetitive");
    group.measurement_time(Duration::from_secs(5));

    let data_1mb = generate_repetitive_data(MB);
    group.throughput(Throughput::Bytes(data_1mb.len() as u64));

    for (level, level_name) in [
        (LEVEL_FASTEST, "level-1"),
        (LEVEL_BALANCED, "level-2"),
        (LEVEL_SMALLEST, "level-3"),
    ] {
        group.bench_with_input(
            BenchmarkId::new(format!("1MB_{}", level_name), data_1mb.len()),
            &data_1mb,
            |b, data| {
                let mut output = Vec::new();
                b.iter(|| {
                    output.clear();
                    encode(&mut output, data, level).expect("encoding failed");
                });
            },
        );
    }

    group.finish();
}

/// Benchmark decoding at different sizes
fn bench_decode_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_sizes");
    group.measurement_time(Duration::from_secs(5));

    let sizes = [10 * KB, 100 * KB, MB];

    for &size in &sizes {
        let original = generate_repetitive_data(size);

        // Compress with Level 2 (balanced)
        let mut compressed = Vec::new();
        encode(&mut compressed, &original, LEVEL_BALANCED).expect("encoding failed");

        group.throughput(Throughput::Bytes(original.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("decode", size),
            &compressed,
            |b, compressed| {
                let mut output = Vec::new();
                b.iter(|| {
                    output.clear();
                    decode(&mut output, compressed).expect("decoding failed");
                });
            },
        );
    }

    group.finish();
}

/// Benchmark round-trip (encode + decode)
fn bench_round_trip(c: &mut Criterion) {
    let mut group = c.benchmark_group("round_trip");
    group.measurement_time(Duration::from_secs(5));

    let data_100k = generate_repetitive_data(100 * KB);
    group.throughput(Throughput::Bytes(data_100k.len() as u64));

    for (level, level_name) in [
        (LEVEL_FASTEST, "level-1"),
        (LEVEL_BALANCED, "level-2"),
        (LEVEL_SMALLEST, "level-3"),
    ] {
        group.bench_with_input(
            BenchmarkId::new(format!("100KB_{}", level_name), data_100k.len()),
            &data_100k,
            |b, data| {
                b.iter(|| {
                    let mut encoded = Vec::new();
                    encode(&mut encoded, data, level).expect("encoding failed");

                    let mut decoded = Vec::new();
                    decode(&mut decoded, &encoded).expect("decoding failed");

                    assert_eq!(decoded.len(), data.len());
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_encode_random,
    bench_encode_repetitive,
    bench_decode_sizes,
    bench_round_trip
);

criterion_main!(benches);
