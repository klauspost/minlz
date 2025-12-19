//! Comprehensive MinLZ benchmarks for performance comparison with Go implementation
//!
//! This benchmark suite mirrors the structure and test data of the Go benchmarks_test.go
//! to enable direct performance comparison between Rust and Go MinLZ implementations.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use minlz::{decode, encode, LEVEL_BALANCED, LEVEL_FASTEST, LEVEL_SMALLEST};
use std::time::Duration;

mod utils;
use utils::*;

const MB: usize = 1024 * 1024;
const MAX_BLOCK_SIZE: usize = 8 * MB;

/// Benchmark encoding for a specific test file and compression level
fn bench_encode_file(c: &mut Criterion, test_file: &TestFile, level: i32, level_name: &str) {
    let bench_data_dir = get_bench_data_dir();

    let data = match read_test_file(test_file, &bench_data_dir) {
        Ok(data) => data,
        Err(e) => {
            println!("Skipping {}: {}", test_file.label, e);
            return;
        }
    };

    let mut group = c.benchmark_group(format!("encode_{}", test_file.label));
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.measurement_time(Duration::from_secs(10));

    group.bench_with_input(
        BenchmarkId::new(level_name, data.len()),
        &data,
        |b, data| {
            let mut output = Vec::new();
            b.iter(|| {
                output.clear();
                encode(&mut output, data, level).expect("encoding failed");
            });
        },
    );

    // Report compression ratio
    let mut compressed = Vec::new();
    encode(&mut compressed, &data, level).expect("encoding failed for ratio calculation");
    let ratio = (compressed.len() as f64 / data.len() as f64) * 100.0;
    println!(
        "  {}/{}: {:.1}% compression ratio",
        test_file.label, level_name, ratio
    );

    group.finish();
}

/// Benchmark decoding for a specific test file and compression level
fn bench_decode_file(c: &mut Criterion, test_file: &TestFile, level: i32, level_name: &str) {
    let bench_data_dir = get_bench_data_dir();

    let data = match read_test_file(test_file, &bench_data_dir) {
        Ok(data) => data,
        Err(e) => {
            println!("Skipping decode {}: {}", test_file.label, e);
            return;
        }
    };

    // Pre-compress the data
    let mut compressed = Vec::new();
    if encode(&mut compressed, &data, level).is_err() {
        println!("Skipping decode {} (compression failed)", test_file.label);
        return;
    }

    let mut group = c.benchmark_group(format!("decode_{}", test_file.label));
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.measurement_time(Duration::from_secs(10));

    group.bench_with_input(
        BenchmarkId::new(level_name, data.len()),
        &compressed,
        |b, compressed| {
            let mut output = Vec::new();
            b.iter(|| {
                output.clear();
                decode(&mut output, compressed).expect("decoding failed");
            });
        },
    );

    group.finish();
}

/// Benchmark all test files for encoding at all compression levels
fn bench_encode_all_files(c: &mut Criterion) {
    for test_file in TEST_FILES {
        bench_encode_file(c, test_file, LEVEL_FASTEST, "level-1");
        bench_encode_file(c, test_file, LEVEL_BALANCED, "level-2");
        bench_encode_file(c, test_file, LEVEL_SMALLEST, "level-3");
    }
}

/// Benchmark all test files for decoding at all compression levels
fn bench_decode_all_files(c: &mut Criterion) {
    for test_file in TEST_FILES {
        bench_decode_file(c, test_file, LEVEL_FASTEST, "level-1");
        bench_decode_file(c, test_file, LEVEL_BALANCED, "level-2");
        bench_decode_file(c, test_file, LEVEL_SMALLEST, "level-3");
    }
}

/// Benchmark random data encoding (mirrors BenchmarkRandomEncodeBlock1MB and BenchmarkRandomEncodeBlock8MB)
fn bench_encode_random(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_random");

    // 1MB random data
    let data_1mb = generate_random_data(MB, 1);
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

    // 8MB random data
    let data_8mb = generate_random_data(MAX_BLOCK_SIZE, 1);
    group.throughput(Throughput::Bytes(data_8mb.len() as u64));

    for (level, level_name) in [
        (LEVEL_FASTEST, "level-1"),
        (LEVEL_BALANCED, "level-2"),
        (LEVEL_SMALLEST, "level-3"),
    ] {
        group.bench_with_input(
            BenchmarkId::new(format!("8MB_{}", level_name), data_8mb.len()),
            &data_8mb,
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

/// Benchmark Twain encode series (mirrors BenchmarkTwainEncode1e1 through BenchmarkTwainEncode1e6)
fn bench_twain_encode(c: &mut Criterion) {
    let twain_data = match read_twain_file() {
        Ok(data) => data,
        Err(e) => {
            println!("Skipping Twain benchmarks: {}", e);
            return;
        }
    };

    let mut group = c.benchmark_group("twain_encode");

    let sizes = [10, 100, 1000, 10000, 100000, 1000000];

    for &size in &sizes {
        let data = expand(&twain_data, size);
        group.throughput(Throughput::Bytes(data.len() as u64));

        for (level, level_name) in [
            (LEVEL_FASTEST, "level-1"),
            (LEVEL_BALANCED, "level-2"),
            (LEVEL_SMALLEST, "level-3"),
        ] {
            group.bench_with_input(
                BenchmarkId::new(format!("{}_{}", size, level_name), data.len()),
                &data,
                |b, data| {
                    let mut output = Vec::new();
                    b.iter(|| {
                        output.clear();
                        encode(&mut output, data, level).expect("encoding failed");
                    });
                },
            );
        }
    }

    group.finish();
}

/// Benchmark Twain decode series (mirrors BenchmarkTwainDecode1e1 through BenchmarkTwainDecode1e6)
fn bench_twain_decode(c: &mut Criterion) {
    let twain_data = match read_twain_file() {
        Ok(data) => data,
        Err(e) => {
            println!("Skipping Twain decode benchmarks: {}", e);
            return;
        }
    };

    let mut group = c.benchmark_group("twain_decode");

    let sizes = [10, 100, 1000, 10000, 100000, 1000000];

    for &size in &sizes {
        let data = expand(&twain_data, size);

        for (level, level_name) in [
            (LEVEL_FASTEST, "level-1"),
            (LEVEL_BALANCED, "level-2"),
            (LEVEL_SMALLEST, "level-3"),
        ] {
            let mut compressed = Vec::new();
            if encode(&mut compressed, &data, level).is_err() {
                continue;
            }

            group.throughput(Throughput::Bytes(data.len() as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("{}_{}", size, level_name), data.len()),
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
    }

    group.finish();
}

/// Single-threaded encode benchmarks
fn bench_encode_single(c: &mut Criterion) {
    bench_encode_all_files(c);
    bench_encode_random(c);
    bench_twain_encode(c);
}

/// Single-threaded decode benchmarks
fn bench_decode_single(c: &mut Criterion) {
    bench_decode_all_files(c);
    bench_twain_decode(c);
}

/// Parallel encoding benchmarks
fn bench_encode_parallel(c: &mut Criterion) {
    for test_file in TEST_FILES {
        let bench_data_dir = get_bench_data_dir();
        let data = match read_test_file(test_file, &bench_data_dir) {
            Ok(data) => data,
            Err(e) => {
                println!("Skipping parallel encode {}: {}", test_file.label, e);
                continue;
            }
        };

        let mut group = c.benchmark_group(format!("encode_parallel_{}", test_file.label));
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.measurement_time(Duration::from_secs(10));

        for (level, level_name) in [
            (LEVEL_FASTEST, "level-1"),
            (LEVEL_BALANCED, "level-2"),
            (LEVEL_SMALLEST, "level-3"),
        ] {
            group.bench_with_input(
                BenchmarkId::new(level_name, data.len()),
                &data,
                |b, data| {
                    b.iter_batched(
                        || Vec::new(),
                        |mut output| {
                            encode(&mut output, data, level).expect("encoding failed");
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
        }
        group.finish();
    }
}

/// Parallel decoding benchmarks
fn bench_decode_parallel(c: &mut Criterion) {
    for test_file in TEST_FILES {
        let bench_data_dir = get_bench_data_dir();
        let data = match read_test_file(test_file, &bench_data_dir) {
            Ok(data) => data,
            Err(e) => {
                println!("Skipping parallel decode {}: {}", test_file.label, e);
                continue;
            }
        };

        for (level, level_name) in [
            (LEVEL_FASTEST, "level-1"),
            (LEVEL_BALANCED, "level-2"),
            (LEVEL_SMALLEST, "level-3"),
        ] {
            let mut compressed = Vec::new();
            if encode(&mut compressed, &data, level).is_err() {
                continue;
            }

            let mut group = c.benchmark_group(format!(
                "decode_parallel_{}_{}",
                test_file.label, level_name
            ));
            group.throughput(Throughput::Bytes(data.len() as u64));
            group.measurement_time(Duration::from_secs(10));

            group.bench_with_input(
                BenchmarkId::new("parallel", data.len()),
                &compressed,
                |b, compressed| {
                    b.iter_batched(
                        || Vec::new(),
                        |mut output| {
                            decode(&mut output, compressed).expect("decoding failed");
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
            group.finish();
        }
    }
}

/// Configure Criterion for our benchmarks
fn configure_criterion() -> Criterion {
    Criterion::default()
        .sample_size(20)
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3))
        .with_plots()
}

criterion_group! {
    name = benches_single;
    config = configure_criterion();
    targets = bench_encode_single, bench_decode_single
}

criterion_group! {
    name = benches_parallel;
    config = configure_criterion();
    targets = bench_encode_parallel, bench_decode_parallel
}

criterion_main!(benches_single, benches_parallel);
