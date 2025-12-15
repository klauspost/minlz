//! Safe memory access helpers equivalent to Go's unsafe operations

use crate::error::{Error, Result};

/// Load a single byte from slice at index
#[inline(always)]
pub fn load8(src: &[u8], index: usize) -> Result<u8> {
    src.get(index).copied().ok_or(Error::Corrupt)
}

/// Load a 16-bit little-endian value from slice at index
#[inline(always)]
pub fn load16(src: &[u8], index: usize) -> Result<u16> {
    if index + 2 > src.len() {
        return Err(Error::Corrupt);
    }
    Ok(u16::from_le_bytes([src[index], src[index + 1]]))
}

/// Load a 32-bit little-endian value from slice at index
#[inline(always)]
pub fn load32(src: &[u8], index: usize) -> Result<u32> {
    if index + 4 > src.len() {
        return Err(Error::Corrupt);
    }
    Ok(u32::from_le_bytes([
        src[index],
        src[index + 1],
        src[index + 2],
        src[index + 3],
    ]))
}

/// Load a 64-bit little-endian value from slice at index
#[inline(always)]
pub fn load64(src: &[u8], index: usize) -> Result<u64> {
    if index + 8 > src.len() {
        return Err(Error::Corrupt);
    }
    Ok(u64::from_le_bytes([
        src[index],
        src[index + 1],
        src[index + 2],
        src[index + 3],
        src[index + 4],
        src[index + 5],
        src[index + 6],
        src[index + 7],
    ]))
}

/// Store an 8-bit value into slice at index
#[inline(always)]
pub fn store8(dst: &mut [u8], index: usize, value: u8) -> Result<()> {
    if index >= dst.len() {
        return Err(Error::Corrupt);
    }
    dst[index] = value;
    Ok(())
}

/// Store a 16-bit little-endian value into slice at index
#[inline(always)]
pub fn store16(dst: &mut [u8], index: usize, value: u16) -> Result<()> {
    if index + 2 > dst.len() {
        return Err(Error::Corrupt);
    }
    let bytes = value.to_le_bytes();
    dst[index] = bytes[0];
    dst[index + 1] = bytes[1];
    Ok(())
}

/// Store a 32-bit little-endian value into slice at index
#[inline(always)]
pub fn store32(dst: &mut [u8], index: usize, value: u32) -> Result<()> {
    if index + 4 > dst.len() {
        return Err(Error::Corrupt);
    }
    let bytes = value.to_le_bytes();
    dst[index..index + 4].copy_from_slice(&bytes);
    Ok(())
}

