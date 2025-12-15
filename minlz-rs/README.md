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

```bash
# Run tests
cargo test

# Run benchmarks
cargo bench

# Check compilation
cargo check --all-targets
```

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.