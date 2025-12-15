# MinLZ Rust Benchmarks

This document describes the comprehensive benchmark suite for the MinLZ Rust implementation, designed for direct performance comparison with the Go implementation.

## Overview

The benchmark suite mirrors the structure of the Go `benchmarks_test.go` to enable direct performance comparison. It includes:

- **Test Data Compatibility**: Uses identical test files from Snappy's canonical dataset
- **Compression Level Coverage**: All three levels (Fastest, Balanced, Smallest)
- **Comprehensive Metrics**: Throughput, compression ratios, and performance statistics
- **Both Single and Parallel Testing**: Mirrors Go's single-threaded and parallel benchmark variants

## Benchmark Categories

### 1. File-Based Benchmarks

Tests compression/decompression on standardized test files:

| Label | File | Description | Size Limit |
|-------|------|-------------|------------|
| html | html | HTML content | None |
| urls | urls.10K | URL list | None |
| jpg | fireworks.jpeg | JPEG image | None |
| jpg_200b | fireworks.jpeg | JPEG image | 200 bytes |
| pdf | paper-100k.pdf | PDF document | None |
| html4 | html_x_4 | HTML content (4x) | None |
| txt1-4 | alice29.txt, asyoulik.txt, etc. | Text files | Various |
| pb | geo.protodata | Protocol buffer data | None |
| gaviota | kppkn.gtb | Chess tablebase | None |

### 2. Random Data Benchmarks

Tests on randomly generated data:
- **1MB Random Data**: Tests at all compression levels
- **8MB Random Data**: Tests maximum block size performance

### 3. Twain Series Benchmarks

Size-series tests using Mark Twain text:
- **Sizes**: 10, 100, 1K, 10K, 100K, 1M bytes
- **Both Encode and Decode**: Performance across different data sizes

## Running Benchmarks

### Prerequisites

```bash
# Install Rust if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Update dependencies (first run only)
cargo update
```

### Basic Usage

```bash
# Run all benchmarks (Warning: Takes a long time!)
cargo bench --bench comparison_benchmarks

# Run specific benchmark groups
cargo bench --bench comparison_benchmarks benches_single
cargo bench --bench comparison_benchmarks benches_parallel

# Run specific compression level tests
cargo bench --bench comparison_benchmarks -- "level-1"
cargo bench --bench comparison_benchmarks -- "level-2"
cargo bench --bench comparison_benchmarks -- "level-3"

# Run specific file type tests
cargo bench --bench comparison_benchmarks -- "encode_html"
cargo bench --bench comparison_benchmarks -- "decode_txt1"

# Run random data tests only
cargo bench --bench comparison_benchmarks -- "encode_random"

# Run Twain series tests only
cargo bench --bench comparison_benchmarks -- "twain"
```

### Quick Testing (Smaller Samples)

For faster testing during development:

```bash
# Reduce sample size and time
CRITERION_SAMPLE_SIZE=10 cargo bench --bench comparison_benchmarks -- "encode_random"
```

### Output Location

- **Results**: `target/criterion/`
- **HTML Reports**: `target/criterion/*/report/index.html`
- **Raw Data**: `target/criterion/*/base/`

## Test Data Management

### Automatic Downloads

The benchmark suite automatically downloads test files from:
`https://raw.githubusercontent.com/google/snappy/master/testdata/`

Downloaded files are cached in:
- `testdata/bench/` (preferred)
- `target/testdata/bench/` (fallback)

### Manual Test Data Setup

If automatic downloads fail, manually place test files in `testdata/bench/`:

```bash
mkdir -p testdata/bench
cd testdata/bench

# Download key test files
curl -O https://raw.githubusercontent.com/google/snappy/master/testdata/html
curl -O https://raw.githubusercontent.com/google/snappy/master/testdata/urls.10K
curl -O https://raw.githubusercontent.com/google/snappy/master/testdata/fireworks.jpeg
curl -O https://raw.githubusercontent.com/google/snappy/master/testdata/alice29.txt
# ... etc for other files
```

### Twain Data

For Twain series benchmarks, ensure `testdata/Mark.Twain-Tom.Sawyer.txt` exists.
This file is included in the Go project's testdata directory.

## Interpreting Results

### Throughput Metrics

```
encode_html/level-1/102400  time:   [45.2 µs 45.8 µs 46.4 µs]
                            thrpt: [2.1 GiB/s 2.1 GiB/s 2.2 GiB/s]
```

- **time**: Compression time per operation
- **thrpt**: Throughput in bytes/second

### Compression Ratios

The benchmarks automatically calculate and display compression ratios:

```
  html/level-1: 65.2% compression ratio
  html/level-2: 62.8% compression ratio
  html/level-3: 61.4% compression ratio
```

Lower percentages indicate better compression.

### Statistical Analysis

Criterion provides:
- **Mean and confidence intervals**
- **Regression analysis**
- **Performance comparison with previous runs**
- **Outlier detection and analysis**

## Comparison with Go Implementation

### Running Go Benchmarks

In the Go project directory:

```bash
# Run equivalent Go benchmarks
go test -bench=BenchmarkEncode -benchtime=10s
go test -bench=BenchmarkDecode -benchtime=10s
go test -bench=BenchmarkTwainEncode -benchtime=10s
go test -bench=BenchmarkTwainDecode -benchtime=10s
go test -bench=BenchmarkRandomEncode -benchtime=10s
```

### Direct Performance Comparison

Both benchmark suites use:
- **Identical test data** (same URLs, same files)
- **Same size limits** and data transformations
- **Equivalent compression levels** (1=Fastest, 2=Balanced, 3=Smallest)
- **Same random data seeds** for reproducible results

### Comparison Methodology

1. **Normalize for hardware differences**:
   ```bash
   # Go single-threaded
   go test -bench=BenchmarkEncodeBlockSingle -cpu=1

   # Rust single-threaded equivalent
   cargo bench --bench comparison_benchmarks benches_single
   ```

2. **Compare compression ratios**:
   - Both implementations should achieve similar compression ratios
   - Minor differences are acceptable due to implementation details

3. **Throughput analysis**:
   - Compare MB/s measurements
   - Account for Go's assembly optimizations vs Rust's pure implementation

## Performance Tuning

### CPU Target Features

For maximum performance, build with native CPU features:

```bash
RUSTFLAGS="-C target-cpu=native" cargo bench --bench comparison_benchmarks
```

### Release Profile Optimization

The benchmark uses the `bench` profile which is optimized for performance:

```toml
[profile.bench]
inherits = "release"
lto = true
codegen-units = 1
```

### Memory Allocation

- Uses `Vec::with_capacity()` for known sizes
- Pre-allocates buffers to minimize allocation overhead
- Pools hash tables (Level 3) to reduce allocation costs

## Troubleshooting

### Download Issues

If test file downloads fail:

```bash
# Check network connectivity
curl -I https://raw.githubusercontent.com/google/snappy/master/testdata/html

# Use manual download
mkdir -p testdata/bench
cd testdata/bench && wget https://raw.githubusercontent.com/google/snappy/master/testdata/html
```

### Missing Twain File

```bash
# Copy from Go project
cp ../../../testdata/Mark.Twain-Tom.Sawyer.txt testdata/

# Or create symlink
ln -s ../../../testdata/Mark.Twain-Tom.Sawyer.txt testdata/
```

### Performance Issues

If benchmarks run too slowly:

1. **Reduce sample size**: Use `CRITERION_SAMPLE_SIZE=5`
2. **Skip large files**: Comment out large test files in `test_files.rs`
3. **Run specific tests**: Use pattern matching to run subset

### Compilation Warnings

The many warnings about unused code are expected - they come from the Level 3 encoder foundation which is not fully implemented yet.

## Benchmark Architecture

### File Structure

```
benches/
├── comparison_benchmarks.rs    # Main benchmark suite
└── utils/
    ├── mod.rs                 # Utilities and helpers
    └── test_files.rs          # Test file definitions
```

### Key Components

1. **Test File Management**: Download, cache, and size limit application
2. **Data Generation**: Random data and Twain text expansion
3. **Benchmark Organization**: Grouped by operation type and test data
4. **Metrics Collection**: Throughput, compression ratios, and timing

### Extension Points

To add new benchmarks:

1. **Add test files**: Update `TEST_FILES` in `test_files.rs`
2. **Add benchmark functions**: Follow existing patterns in `comparison_benchmarks.rs`
3. **Add to criterion groups**: Update `criterion_group!` and `criterion_main!`

## Future Enhancements

- **Cross-platform comparison**: Automated CI comparison between Go and Rust
- **Historical tracking**: Performance regression detection
- **Memory profiling**: Integration with memory profilers
- **Assembly optimization**: Compare with assembly-optimized Go variants