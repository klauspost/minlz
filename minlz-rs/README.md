# MinLZ Rust Implementation

This is the Rust implementation of MinLZ compression library, providing high-performance block compression and decompression compatible with the Go implementation.

## Features

- **Three compression levels**: Fastest (1), Balanced (2), Smallest (3)
- **Full format compatibility** with MinLZ Go implementation
- **Safe Rust implementation** with comprehensive error handling
- **Block-level compression** up to 8MB blocks
- **Comprehensive test suite** with round-trip validation and fuzz testing
- **Performance benchmarks** for comparison with Go implementation

## Quick Start

```rust
use minlz::{encode, decode, LEVEL_BALANCED};

let data = b"Hello, world! This is test data for compression.";
let mut encoded = Vec::new();
encode(&mut encoded, data, LEVEL_BALANCED)?;

let mut decoded = Vec::new();
decode(&mut decoded, &encoded)?;
assert_eq!(&decoded, data);
```

## Performance Benchmarks

This crate includes comprehensive benchmarks that mirror the Go implementation's test suite:

```bash
# Run all benchmarks
cargo bench --bench comparison_benchmarks

# Quick random data test
cargo bench --bench comparison_benchmarks -- "encode_random"

# See BENCHMARKS.md for detailed usage
```

See [BENCHMARKS.md](BENCHMARKS.md) for complete benchmarking documentation and performance comparison with the Go implementation.

## Development

### Testing and Benchmarking

For Windows users, convenient batch files are provided:

```bash
# Interactive benchmark runner with Level 1, 2, and 3 options
bench.cmd

# Interactive fuzz testing with continuous options
fuzz.cmd
```

Or use cargo directly:

```bash
# Run tests
cargo test

# Run all benchmarks (takes a long time)
cargo bench

# Run Level 3 specific benchmarks
cargo bench --bench comparison_benchmarks -- "level-3"

# Run fuzz tests (see FUZZING.md for continuous/advanced options)
cargo test fuzz_tests::

# Run high-intensity continuous fuzzing (stable Rust)
cargo test continuous_fuzz:: --release -- --nocapture

# Check compilation
cargo check --all-targets
```

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.