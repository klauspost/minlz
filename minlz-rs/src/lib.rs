//! # MinLZ Compression Library
//!
//! This implements the MinLZ specification v1.0 - a high-speed LZ77-style compressor
//! with a fixed size, byte-oriented encoding format.
//!
//! MinLZ provides three compression levels:
//! - Level 1 (Fastest): Optimized for speed
//! - Level 2 (Balanced): Balance between speed and compression ratio
//! - Level 3 (Best): Optimized for compression ratio
//!
//! The format is compatible with the reference Go implementation and provides
//! seamless fallback to Snappy when decoding non-MinLZ data.
//!
//! ## Example
//!
//! ```rust
//! use minlz::{encode, decode, LEVEL_FASTEST};
//!
//! let data = b"Hello, world! This is test data for compression.";
//! let mut encoded = Vec::new();
//! encode(&mut encoded, data, LEVEL_FASTEST)?;
//!
//! let mut decoded = Vec::new();
//! decode(&mut decoded, &encoded)?;
//! assert_eq!(&decoded, data);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod constants;
mod error;
mod memory;
mod varint;
mod decode;
mod encode;

#[cfg(test)]
mod tests;


#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod fuzz_tests;

#[cfg(test)]
mod continuous_fuzz;



pub use constants::*;
pub use error::{Error, Result};
pub use encode::encode;
pub use decode::decode;

/// Maximum size of a block that can be compressed/decompressed
pub const MAX_BLOCK_SIZE: usize = 8 << 20; // 8 MiB

/// Fastest compression level - optimized for speed
pub const LEVEL_FASTEST: i32 = 1;

/// Balanced compression level - balance between speed and ratio
pub const LEVEL_BALANCED: i32 = 2;

/// Best compression level - optimized for compression ratio
pub const LEVEL_SMALLEST: i32 = 3;

/// Returns the maximum encoded length for a given source length.
/// Returns None if the source is too large to encode.
pub fn max_encoded_len(src_len: usize) -> Option<usize> {
    if src_len > MAX_BLOCK_SIZE {
        return None;
    }
    if src_len == 0 {
        return Some(1);
    }
    // Maximum overhead is 2 bytes for the header
    Some(src_len + 2)
}

/// Returns the decoded length of a MinLZ block without decoding it.
pub fn decoded_len(src: &[u8]) -> Result<usize> {
    decode::decoded_len(src)
}

/// Checks if the data is a MinLZ block and returns the decoded size.
pub fn is_minlz(src: &[u8]) -> Result<(bool, usize)> {
    decode::is_minlz(src)
}