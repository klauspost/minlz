//! MinLZ encoding implementation

pub mod emit;
pub mod hash;
pub mod level1;
pub mod level2;
pub mod level3;

#[cfg(test)]
mod emit_validation_tests;

use crate::{
    constants::*,
    error::{Error, Result},
    varint::encode_uvarint,
};

/// Encode data using MinLZ compression
pub fn encode(dst: &mut Vec<u8>, src: &[u8], level: i32) -> Result<()> {
    // Validate compression level
    match level {
        LEVEL_FASTEST | LEVEL_BALANCED | LEVEL_SMALLEST => {}
        _ => return Err(Error::InvalidLevel),
    }

    // Check if source is too large
    if src.len() > MAX_BLOCK_SIZE {
        return Err(Error::TooLarge);
    }

    // Calculate maximum encoded size and ensure dst has capacity
    let max_len = crate::max_encoded_len(src.len()).unwrap();
    dst.clear();
    dst.resize(max_len, 0);

    // Handle small blocks as uncompressed
    if src.len() < MIN_NON_LITERAL_BLOCK_SIZE {
        let encoded_len = encode_uncompressed(&mut dst[..], src);
        dst.truncate(encoded_len);
        return Ok(());
    }

    // MinLZ format starts with 0 byte
    dst[0] = 0;
    let mut pos = 1;

    // Encode the decompressed length as varint
    pos += encode_uvarint(&mut dst[pos..], src.len() as u64)?;

    // Encode using the specified level
    let compressed_size = match level {
        LEVEL_FASTEST => level1::encode_block(&mut dst[pos..], src)?,
        LEVEL_BALANCED => level2::encode_block(&mut dst[pos..], src)?,
        LEVEL_SMALLEST => level3::encode_block(&mut dst[pos..], src)?,
        _ => unreachable!(),
    };

    if compressed_size > 0 {
        // Compression succeeded
        dst.truncate(pos + compressed_size);

        // Check if compression was worthwhile
        if dst.len() >= src.len() {
            // Not worth compressing, use uncompressed format
            dst.clear();
            dst.resize(src.len() + 2, 0);
            let encoded_len = encode_uncompressed(&mut dst[..], src);
            dst.truncate(encoded_len);
        }
    } else {
        // Compression failed, use uncompressed format
        dst.clear();
        dst.resize(src.len() + 2, 0);
        let encoded_len = encode_uncompressed(&mut dst[..], src);
        dst.truncate(encoded_len);
    }

    Ok(())
}

/// Encode data as uncompressed MinLZ block
fn encode_uncompressed(dst: &mut [u8], src: &[u8]) -> usize {
    if src.is_empty() {
        dst[0] = 0;
        return 1;
    }

    dst[0] = 0; // MinLZ indicator
    dst[1] = 0; // Zero length indicates literals
    dst[2..2 + src.len()].copy_from_slice(src);
    2 + src.len()
}
