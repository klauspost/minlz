//! MinLZ block codec — Rust port (stage A).
//!
//! This implements the MinLZ specification v1.0 — block format only.
//! See `SPEC.md` §1–§2 in the upstream Go repository for the wire format.
//!
//! Supported features: block-level encode at three compression levels
//! ([`Level::Fastest`], [`Level::Balanced`], [`Level::Smallest`]) and a safe
//! decoder.  Streaming, the stream index, search tables, dictionaries,
//! `SuperFast`, Snappy/S2 fallback, and LZ4 conversion are out of scope.
//!
//! # Endianness
//!
//! MinLZ's wire format is little-endian.  The tuned path targets LE hosts
//! (x86_64, aarch64, …) where unaligned 64-bit loads compile to a single
//! `MOVQ` / `LDR`.  Big-endian targets compile and pass tests, but pay
//! a per-load byte-swap and are not in the perf-target matrix.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

pub mod block;
mod error;

pub use block::{
    append_decoded, append_encoded, decode, decoded_len, encode, is_minlz, max_encoded_len,
    try_encode, Level, MAX_BLOCK_SIZE,
};
pub use error::Error;

/// A specialized [`Result`] type for MinLZ operations.
pub type Result<T> = core::result::Result<T, Error>;
