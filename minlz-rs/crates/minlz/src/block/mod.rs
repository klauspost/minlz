//! MinLZ block codec — public API.
//!
//! Block format reference: SPEC.md §1–§2 in the upstream Go repository.

mod decode;
mod emit;
mod encode_l1;
mod encode_l2;
mod encode_l3;
mod format;
mod hash;
mod load_store;
mod match_len;

pub use format::{max_encoded_len, MAX_BLOCK_SIZE};

use crate::Error;

/// Compression level for [`encode`].
///
/// The numeric values match the Go API (`LevelFastest = 1`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i8)]
pub enum Level {
    /// Single-pass with a 15-bit (or 13-bit for ≤64 KiB inputs) hash table.
    /// Maps to Go's `LevelFastest` / `encodeBlockGo`.
    Fastest = 1,
    /// Two-table encoder.  Maps to Go's `LevelBalanced` / `encodeBlockBetterGo`.
    Balanced = 2,
    /// Multi-candidate, size-scored encoder.  Maps to Go's
    /// `LevelSmallest` / `encodeBlockBest`.
    Smallest = 3,
}

impl Level {
    /// Convert from the Go integer level encoding.
    pub fn from_i32(v: i32) -> Result<Self, Error> {
        match v {
            1 => Ok(Level::Fastest),
            2 => Ok(Level::Balanced),
            3 => Ok(Level::Smallest),
            _ => Err(Error::InvalidLevel),
        }
    }
}

/// Encode `src` as a MinLZ block, *replacing* the contents of `dst`.
///
/// Equivalent to Go's `Encode(dst, src, level)`.  `dst` is cleared and then
/// filled with the encoded block.
pub fn encode(dst: &mut Vec<u8>, src: &[u8], level: Level) -> Result<(), Error> {
    dst.clear();
    append_encoded(dst, src, level)
}

/// Encode `src` as a MinLZ block, *appending* to `dst`.
///
/// Equivalent to Go's `AppendEncoded(dst, src, level)`.
pub fn append_encoded(dst: &mut Vec<u8>, src: &[u8], level: Level) -> Result<(), Error> {
    let max = max_encoded_len(src.len()).ok_or(Error::TooLarge)?;
    dst.reserve(max);
    let start = dst.len();
    // Worst case: a single literal block (header + raw bytes).
    if src.len() < format::MIN_NON_LITERAL_BLOCK_SIZE {
        encode_uncompressed(dst, src);
        return Ok(());
    }

    // Layout: [tag=0][varint(len)][compressed body].
    dst.push(0);
    format::put_uvarint(dst, src.len() as u64);
    let body_start = dst.len();
    // Reserve worst-case body length so the encoder can write via index.
    dst.resize(start + max, 0);

    let n = match level {
        Level::Fastest => encode_l1::encode_block(&mut dst[body_start..], src),
        Level::Balanced => encode_l2::encode_block(&mut dst[body_start..], src),
        Level::Smallest => encode_l3::encode_block(&mut dst[body_start..], src),
    };

    if n > 0 {
        dst.truncate(body_start + n);
        Ok(())
    } else {
        // Not compressible: fall back to an uncompressed block.
        dst.truncate(start);
        encode_uncompressed(dst, src);
        Ok(())
    }
}

/// Try to encode `src`; on success, append to `dst` and return `true`.
/// Returns `false` if the input is not compressible (parity with Go
/// `TryEncode`, which returns `nil` in that case).
pub fn try_encode(dst: &mut Vec<u8>, src: &[u8], level: Level) -> Result<bool, Error> {
    let max = max_encoded_len(src.len()).ok_or(Error::TooLarge)?;
    if src.len() < format::MIN_NON_LITERAL_BLOCK_SIZE {
        return Ok(false);
    }
    dst.reserve(max);
    let start = dst.len();
    dst.push(0);
    format::put_uvarint(dst, src.len() as u64);
    let body_start = dst.len();
    dst.resize(start + max, 0);

    let n = match level {
        Level::Fastest => encode_l1::encode_block(&mut dst[body_start..], src),
        Level::Balanced | Level::Smallest => 0,
    };

    // Go's TryEncode also rejects "compressed >= src.len()" (see encode.go).
    if n > 0 && (body_start - start) + n < src.len() {
        dst.truncate(body_start + n);
        Ok(true)
    } else {
        dst.truncate(start);
        Ok(false)
    }
}

/// Decode a MinLZ block into `dst`.
///
/// `dst` is cleared.  On success the decoded bytes are appended.
/// Equivalent to Go's `Decode(dst, src)` (without the Snappy/S2 fallback).
pub fn decode(dst: &mut Vec<u8>, src: &[u8]) -> Result<(), Error> {
    dst.clear();
    append_decoded(dst, src)
}

/// Append the decoded form of `src` to `dst`.
///
/// Equivalent to Go's `AppendDecoded(dst, src)`.
pub fn append_decoded(dst: &mut Vec<u8>, src: &[u8]) -> Result<(), Error> {
    let (literals, body, dlen) = parse_header(src)?;
    if literals {
        dst.extend_from_slice(body);
        return Ok(());
    }
    let start = dst.len();
    // The decoder uses LZ4-style 16-byte overshoot stores in its hot path;
    // give it `OVERSHOOT_PAD` bytes of writable padding past `dlen`.
    dst.reserve(dlen + decode::OVERSHOOT_PAD);
    // SAFETY: we just reserved enough room; `dst.as_mut_ptr().add(start)`
    // points to at least `dlen + OVERSHOOT_PAD` writable bytes.  The
    // decoder writes `dlen` valid bytes on success; the padding region's
    // contents are undefined and will not be exposed via the returned Vec.
    unsafe {
        let dst_ptr = dst.as_mut_ptr().add(start);
        decode::minlz_decode(dst_ptr, dlen, body)?;
        dst.set_len(start + dlen);
    }
    Ok(())
}

/// Returns the uncompressed length of a MinLZ block.
///
/// Equivalent to Go's `DecodedLen(src)`.
pub fn decoded_len(src: &[u8]) -> Result<usize, Error> {
    let (_, _, dlen) = parse_header(src)?;
    Ok(dlen)
}

/// Returns `true` if `src` looks like a MinLZ block (leading 0 byte).
///
/// Snappy/S2 fallback is out of scope, so a non-zero first byte returns
/// `Ok(false, _)` *only* if it's a well-formed legacy header — otherwise
/// the parser refuses.  In practice we just check the first byte; full
/// validation happens inside [`decode`].
///
/// On success returns `(is_minlz, uncompressed_size)`.
pub fn is_minlz(src: &[u8]) -> Result<(bool, usize), Error> {
    if src.is_empty() {
        return Err(Error::Corrupt);
    }
    if src[0] != 0 {
        // Snappy/S2 fallback is out of scope.
        return Ok((false, 0));
    }
    let (_, _, dlen) = parse_header(src)?;
    Ok((true, dlen))
}

/// Parse the block header.  Returns `(is_literal_only, body_slice, dlen)`.
fn parse_header(src: &[u8]) -> Result<(bool, &[u8], usize), Error> {
    if src.is_empty() {
        return Err(Error::Corrupt);
    }
    if src.len() == 1 {
        // Single byte: must be 0 (0-byte block).
        if src[0] == 0 {
            return Ok((true, &src[1..], 0));
        }
        return Err(Error::Corrupt);
    }
    if src[0] != 0 {
        // Snappy/S2 fallback path — refused here.
        return Err(Error::Corrupt);
    }
    let after_magic = &src[1..];
    let (v, n) = format::get_uvarint(after_magic).ok_or(Error::Corrupt)?;
    // Match Go decodedLen: anything that wouldn't fit in uint32 is corrupt …
    if v > 0xffff_ffff {
        return Err(Error::Corrupt);
    }
    // … and anything between uint32 and MaxBlockSize is "too large".
    if v > MAX_BLOCK_SIZE as u64 {
        return Err(Error::TooLarge);
    }
    let dlen = v as usize;
    let body = &after_magic[n..];
    // Order matches Go's isMinLZ: body-empty before the v==0 short-circuit.
    if body.is_empty() {
        return Err(Error::Corrupt);
    }
    if dlen == 0 {
        return Ok((true, body, body.len()));
    }
    if dlen < body.len() {
        // A compressed block may not be larger than the decompressed block.
        return Err(Error::Corrupt);
    }
    Ok((false, body, dlen))
}

fn encode_uncompressed(dst: &mut Vec<u8>, src: &[u8]) {
    if src.is_empty() {
        dst.push(0);
        return;
    }
    dst.push(0);
    dst.push(0);
    dst.extend_from_slice(src);
}

#[cfg(test)]
mod tests;
