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

// Fast unsafe versions for hot paths where bounds are guaranteed
// These should only be used in encoder inner loops where bounds have already been checked

/// Unsafe fast load of 64-bit value - no bounds checking
#[inline(always)]
pub unsafe fn load64_unchecked(src: &[u8], index: usize) -> u64 {
    u64::from_le_bytes([
        *src.get_unchecked(index),
        *src.get_unchecked(index + 1),
        *src.get_unchecked(index + 2),
        *src.get_unchecked(index + 3),
        *src.get_unchecked(index + 4),
        *src.get_unchecked(index + 5),
        *src.get_unchecked(index + 6),
        *src.get_unchecked(index + 7),
    ])
}

/// Unsafe fast load of 32-bit value - no bounds checking
#[inline(always)]
pub unsafe fn load32_unchecked(src: &[u8], index: usize) -> u32 {
    u32::from_le_bytes([
        *src.get_unchecked(index),
        *src.get_unchecked(index + 1),
        *src.get_unchecked(index + 2),
        *src.get_unchecked(index + 3),
    ])
}

/// Unsafe fast load of 16-bit value - no bounds checking
#[inline(always)]
#[allow(dead_code)]
pub unsafe fn load16_unchecked(src: &[u8], index: usize) -> u16 {
    u16::from_le_bytes([*src.get_unchecked(index), *src.get_unchecked(index + 1)])
}
