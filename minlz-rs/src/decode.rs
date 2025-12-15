//! MinLZ decoding implementation

use crate::{
    constants::*,
    error::{Error, Result},
    memory::*,
    varint::decode_uvarint,
};

/// Decode MinLZ compressed data into the destination buffer
pub fn decode(dst: &mut Vec<u8>, src: &[u8]) -> Result<()> {
    let (is_mlz, is_literals, block, decoded_len) = is_minlz_internal(src)?;

    if is_literals {
        dst.clear();
        dst.extend_from_slice(block);
        return Ok(());
    }

    if !is_mlz {
        // TODO: Add Snappy/S2 fallback support
        return Err(Error::Unsupported);
    }

    // Ensure dst has enough capacity
    dst.clear();
    dst.resize(decoded_len, 0);

    minlz_decode(&mut dst[..], block)?;
    Ok(())
}

/// Check if data is MinLZ format and return metadata
pub fn is_minlz(src: &[u8]) -> Result<(bool, usize)> {
    let (is_mlz, _, _, decoded_len) = is_minlz_internal(src)?;
    Ok((is_mlz, decoded_len))
}

/// Return the decoded length without decompressing
pub fn decoded_len(src: &[u8]) -> Result<usize> {
    let (_, _, _, decoded_len) = is_minlz_internal(src)?;
    Ok(decoded_len)
}

/// Internal function to parse MinLZ header and detect format
fn is_minlz_internal(src: &[u8]) -> Result<(bool, bool, &[u8], usize)> {
    if src.is_empty() {
        return Err(Error::Corrupt);
    }

    // Size 0 block
    if src.len() == 1 && src[0] == 0 {
        return Ok((true, true, &src[1..], 0));
    }

    // Check for MinLZ indicator byte
    if src[0] != 0 {
        // Not MinLZ - could be Snappy/S2
        let (decoded_len, _) = decoded_len_internal(src)?;
        return Ok((false, false, src, decoded_len));
    }

    let src = &src[1..]; // Skip the 0 byte
    let (decoded_len, header_len) = decoded_len_internal(src)?;

    if decoded_len > MAX_BLOCK_SIZE {
        return Err(Error::TooLarge);
    }

    let block = &src[header_len..];
    if block.is_empty() {
        return Err(Error::Corrupt);
    }

    if decoded_len == 0 {
        // Literals block - rest is uncompressed data
        return Ok((true, true, block, block.len()));
    }

    if decoded_len < block.len() {
        return Err(Error::Corrupt);
    }

    Ok((true, false, block, decoded_len))
}

/// Extract decoded length from varint header
fn decoded_len_internal(src: &[u8]) -> Result<(usize, usize)> {
    let (value, header_len) = decode_uvarint(src)?;

    if value > 0xffffffff {
        return Err(Error::Corrupt);
    }

    // Check for 32-bit overflow on 32-bit systems
    if cfg!(target_pointer_width = "32") && value > 0x7fffffff {
        return Err(Error::TooLarge);
    }

    Ok((value as usize, header_len))
}

/// Main MinLZ decoding function
fn minlz_decode(dst: &mut [u8], src: &[u8]) -> Result<()> {
    let mut d = 0; // destination position
    let mut s = 0; // source position
    let mut offset = 1; // last copy offset for repeats

    // Fast path - decode with margin for bounds checking
    while s + 11 < src.len() && d + 11 < dst.len() {
        let tag = load8(src, s)?;
        s += 1;

        match tag & 0x03 {
            TAG_LITERAL => {
                let (length, repeat) = decode_literal_header(src, &mut s, tag)?;
                if repeat {
                    copy_repeat(dst, &mut d, offset, length)?;
                } else {
                    copy_literals(dst, &mut d, src, &mut s, length)?;
                }
            }

            TAG_COPY1 => {
                let (new_offset, length) = decode_copy1(src, &mut s, tag)?;
                offset = new_offset;
                copy_match(dst, &mut d, offset, length)?;
            }

            TAG_COPY2 => {
                let (new_offset, length) = decode_copy2(src, &mut s, tag)?;
                offset = new_offset;
                copy_match(dst, &mut d, offset, length)?;
            }

            _ => {
                // TAG_COPY2_FUSED or TAG_COPY3
                if tag & 4 == 0 {
                    // Fused Copy2
                    let (new_offset, length, lit_len) = decode_fused_copy2(src, &mut s, tag)?;
                    if lit_len > 0 {
                        copy_literals(dst, &mut d, src, &mut s, lit_len)?;
                    }
                    offset = new_offset;
                    copy_match(dst, &mut d, offset, length)?;
                } else {
                    // Copy3
                    let (new_offset, length, lit_len) = decode_copy3(src, &mut s, tag)?;
                    if lit_len > 0 {
                        copy_literals(dst, &mut d, src, &mut s, lit_len)?;
                    }
                    offset = new_offset;
                    copy_match(dst, &mut d, offset, length)?;
                }
            }
        }
    }

    // Slow path with full bounds checking
    while s < src.len() {
        let tag = load8(src, s)?;
        s += 1;

        match tag & 0x03 {
            TAG_LITERAL => {
                let (length, repeat) = decode_literal_header(src, &mut s, tag)?;
                if repeat {
                    copy_repeat_safe(dst, &mut d, offset, length)?;
                } else {
                    copy_literals_safe(dst, &mut d, src, &mut s, length)?;
                }
            }

            TAG_COPY1 => {
                let (new_offset, length) = decode_copy1_safe(src, &mut s, tag)?;
                offset = new_offset;
                copy_match_safe(dst, &mut d, offset, length)?;
            }

            TAG_COPY2 => {
                let (new_offset, length) = decode_copy2_safe(src, &mut s, tag)?;
                offset = new_offset;
                copy_match_safe(dst, &mut d, offset, length)?;
            }

            _ => {
                // TAG_COPY2_FUSED or TAG_COPY3
                if tag & 4 == 0 {
                    // Fused Copy2
                    let (new_offset, length, lit_len) = decode_fused_copy2_safe(src, &mut s, tag)?;
                    if lit_len > 0 {
                        copy_literals_safe(dst, &mut d, src, &mut s, lit_len)?;
                    }
                    offset = new_offset;
                    copy_match_safe(dst, &mut d, offset, length)?;
                } else {
                    // Copy3
                    let (new_offset, length, lit_len) = decode_copy3_safe(src, &mut s, tag)?;
                    if lit_len > 0 {
                        copy_literals_safe(dst, &mut d, src, &mut s, lit_len)?;
                    }
                    offset = new_offset;
                    copy_match_safe(dst, &mut d, offset, length)?;
                }
            }
        }
    }

    if d != dst.len() {
        return Err(Error::Corrupt);
    }

    Ok(())
}

/// Decode literal length and repeat flag from tag
fn decode_literal_header(src: &[u8], s: &mut usize, tag: u8) -> Result<(usize, bool)> {
    let repeat = tag & 4 != 0;
    let x = tag >> 3;

    let length = match x {
        0..=LITERAL_LENGTH_SMALL_MAX => (x + 1) as usize,
        LITERAL_LENGTH_1_BYTE => {
            let len_byte = load8(src, *s)?;
            *s += 1;
            30 + len_byte as usize
        }
        LITERAL_LENGTH_2_BYTE => {
            let len = load16(src, *s)?;
            *s += 2;
            30 + len as usize
        }
        LITERAL_LENGTH_3_BYTE => {
            let len = load32(src, *s)? >> 8; // Only use 3 bytes
            *s += 3;
            30 + len as usize
        }
        _ => return Err(Error::Corrupt),
    };

    Ok((length, repeat))
}

// Implement remaining decode functions for Copy1, Copy2, Copy3...
// (This is getting quite long, so I'll implement the core structure first)

/// Safe copy for literals with bounds checking
#[inline(always)]
fn copy_literals(dst: &mut [u8], d: &mut usize, src: &[u8], s: &mut usize, length: usize) -> Result<()> {
    if *d + length > dst.len() || *s + length > src.len() {
        return Err(Error::Corrupt);
    }
    dst[*d..*d + length].copy_from_slice(&src[*s..*s + length]);
    *d += length;
    *s += length;
    Ok(())
}

/// Safe copy for literals (with bounds checking)
fn copy_literals_safe(dst: &mut [u8], d: &mut usize, src: &[u8], s: &mut usize, length: usize) -> Result<()> {
    if *d + length > dst.len() || *s + length > src.len() {
        return Err(Error::Corrupt);
    }
    copy_literals(dst, d, src, s, length)
}

/// Safe copy for matches with bounds checking
#[inline(always)]
fn copy_match(dst: &mut [u8], d: &mut usize, offset: usize, length: usize) -> Result<()> {
    if *d < offset {
        return Err(Error::Corrupt);
    }

    // Check destination bounds
    if *d + length > dst.len() {
        return Err(Error::Corrupt);
    }

    let src_start = *d - offset;
    if offset >= length {
        // No overlap - can use fast copy
        dst.copy_within(src_start..src_start + length, *d);
    } else {
        // Overlapping copy - byte by byte
        for i in 0..length {
            if *d + i >= dst.len() || src_start + i >= dst.len() {
                return Err(Error::Corrupt);
            }
            dst[*d + i] = dst[src_start + i];
        }
    }
    *d += length;
    Ok(())
}

/// Safe copy for matches (with bounds checking)
fn copy_match_safe(dst: &mut [u8], d: &mut usize, offset: usize, length: usize) -> Result<()> {
    if *d < offset || *d + length > dst.len() {
        return Err(Error::Corrupt);
    }
    copy_match(dst, d, offset, length)
}

/// Copy using last offset (repeat)
#[inline(always)]
fn copy_repeat(dst: &mut [u8], d: &mut usize, offset: usize, length: usize) -> Result<()> {
    copy_match(dst, d, offset, length)
}

/// Safe copy using last offset (repeat)
fn copy_repeat_safe(dst: &mut [u8], d: &mut usize, offset: usize, length: usize) -> Result<()> {
    copy_match_safe(dst, d, offset, length)
}

// Placeholder implementations for copy decoders - these need to be implemented
// following the SPEC.md format exactly

fn decode_copy1(src: &[u8], s: &mut usize, tag: u8) -> Result<(usize, usize)> {
    // Copy1 with 1-byte offset (tag 01)
    // Bits 2-5: Length (0-15), Bits 6-7: Offset LB (lower 2 bits)
    // Next byte: Offset UB (upper 8 bits)

    let length_val = (tag >> 2) & 15;
    let offset_lb = (tag >> 6) & 3;
    let offset_ub = load8(src, *s)?;
    *s += 1;

    let offset = (((offset_ub as usize) << 2) | (offset_lb as usize)) + 1;

    let length = if length_val == COPY1_LENGTH_EXTENDED {
        // Extended length with 1 byte
        let extra_len = load8(src, *s)?;
        *s += 1;
        COPY1_EXTENDED_BASE + extra_len as usize
    } else {
        COPY1_BASE_LENGTH + length_val as usize
    };

    Ok((offset, length))
}

fn decode_copy1_safe(src: &[u8], s: &mut usize, tag: u8) -> Result<(usize, usize)> {
    let length_val = (tag >> 2) & 15;
    let offset_lb = (tag >> 6) & 3;

    if *s >= src.len() {
        return Err(Error::Corrupt);
    }
    let offset_ub = src[*s];
    *s += 1;

    let offset = (((offset_ub as usize) << 2) | (offset_lb as usize)) + 1;

    let length = if length_val == COPY1_LENGTH_EXTENDED {
        if *s >= src.len() {
            return Err(Error::Corrupt);
        }
        let extra_len = src[*s];
        *s += 1;
        COPY1_EXTENDED_BASE + extra_len as usize
    } else {
        COPY1_BASE_LENGTH + length_val as usize
    };

    Ok((offset, length))
}

fn decode_copy2(src: &[u8], s: &mut usize, tag: u8) -> Result<(usize, usize)> {
    // Copy2 with 2-byte offset (tag 10)
    // Bits 2-7: Length (0-63)
    // Next 2 bytes: 16-bit little-endian offset + 64

    let length_val = tag >> 2;
    let offset = load16(src, *s)? as usize + MIN_COPY2_OFFSET;
    *s += 2;

    let length = match length_val {
        0..=COPY2_LENGTH_MAX_DIRECT => COPY2_BASE_LENGTH + length_val as usize,
        COPY2_LENGTH_1_BYTE => {
            let extra_len = load8(src, *s)?;
            *s += 1;
            COPY2_EXTENDED_BASE + extra_len as usize
        }
        COPY2_LENGTH_2_BYTE => {
            let extra_len = load16(src, *s)?;
            *s += 2;
            COPY2_EXTENDED_BASE + extra_len as usize
        }
        COPY2_LENGTH_3_BYTE => {
            let extra_len = load32(src, *s)? >> 8; // Only use 3 bytes
            *s += 3;
            COPY2_EXTENDED_BASE + extra_len as usize
        }
        _ => return Err(Error::Corrupt),
    };

    Ok((offset, length))
}

fn decode_copy2_safe(src: &[u8], s: &mut usize, tag: u8) -> Result<(usize, usize)> {
    let length_val = tag >> 2;

    if *s + 2 > src.len() {
        return Err(Error::Corrupt);
    }
    let offset = u16::from_le_bytes([src[*s], src[*s + 1]]) as usize + MIN_COPY2_OFFSET;
    *s += 2;

    let length = match length_val {
        0..=COPY2_LENGTH_MAX_DIRECT => COPY2_BASE_LENGTH + length_val as usize,
        COPY2_LENGTH_1_BYTE => {
            if *s >= src.len() {
                return Err(Error::Corrupt);
            }
            let extra_len = src[*s];
            *s += 1;
            COPY2_EXTENDED_BASE + extra_len as usize
        }
        COPY2_LENGTH_2_BYTE => {
            if *s + 2 > src.len() {
                return Err(Error::Corrupt);
            }
            let extra_len = u16::from_le_bytes([src[*s], src[*s + 1]]);
            *s += 2;
            COPY2_EXTENDED_BASE + extra_len as usize
        }
        COPY2_LENGTH_3_BYTE => {
            if *s + 3 > src.len() {
                return Err(Error::Corrupt);
            }
            let extra_len = u32::from_le_bytes([src[*s], src[*s + 1], src[*s + 2], 0]);
            *s += 3;
            COPY2_EXTENDED_BASE + extra_len as usize
        }
        _ => return Err(Error::Corrupt),
    };

    Ok((offset, length))
}

fn decode_fused_copy2(src: &[u8], s: &mut usize, tag: u8) -> Result<(usize, usize, usize)> {
    // Fused Copy2 (tag 11, bit 2 = 0)
    // Bits 3-4: Literal length + 1 [1->4]
    // Bits 5-7: Copy Length + 4 [4->11]
    // Next 2 bytes: 16 bit offset + 64 [64->65599]
    // Following: 1-4 Literals

    let lit_len = ((tag >> 3) & 3) as usize + 1; // 1-4 literals
    let copy_length = ((tag >> 5) & 7) as usize + 4; // 4-11 copy length
    let offset = load16(src, *s)? as usize + MIN_COPY2_OFFSET;
    *s += 2;

    Ok((offset, copy_length, lit_len))
}

fn decode_fused_copy2_safe(src: &[u8], s: &mut usize, tag: u8) -> Result<(usize, usize, usize)> {
    let lit_len = ((tag >> 3) & 3) as usize + 1;
    let copy_length = ((tag >> 5) & 7) as usize + 4;

    if *s + 2 > src.len() {
        return Err(Error::Corrupt);
    }
    let offset = u16::from_le_bytes([src[*s], src[*s + 1]]) as usize + MIN_COPY2_OFFSET;
    *s += 2;

    Ok((offset, copy_length, lit_len))
}

fn decode_copy3(src: &[u8], s: &mut usize, tag: u8) -> Result<(usize, usize, usize)> {
    // Copy3 (tag 11, bit 2 = 1)
    // Bits 3-4: Literal length [0->3]
    // Bits 5-10: Copy Length (6 bits)
    // Next 3 bytes: 21 bit offset + 65536 [64K->2MB]
    // Extended Length Bytes (if needed)
    // Literals (if any)

    let lit_len = ((tag >> 3) & 3) as usize; // 0-3 literals
    let val = load32(src, *s)?;
    *s += 4;

    let length_val = (val >> 5) & 63; // 6 bits for length
    let offset = (val >> 11) as usize + MIN_COPY3_OFFSET; // 21 bits for offset

    let length = if length_val <= COPY3_LENGTH_MAX_DIRECT as u32 {
        COPY3_BASE_LENGTH + length_val as usize
    } else if length_val == COPY3_LENGTH_1_BYTE as u32 {
        let extra_len = load8(src, *s)?;
        *s += 1;
        COPY3_EXTENDED_BASE + extra_len as usize
    } else if length_val == COPY3_LENGTH_2_BYTE as u32 {
        let extra_len = load16(src, *s)?;
        *s += 2;
        COPY3_EXTENDED_BASE + extra_len as usize
    } else if length_val == COPY3_LENGTH_3_BYTE as u32 {
        let extra_len = load32(src, *s)? >> 8; // Only use 3 bytes
        *s += 3;
        COPY3_EXTENDED_BASE + extra_len as usize
    } else {
        return Err(Error::Corrupt);
    };

    Ok((offset, length, lit_len))
}

fn decode_copy3_safe(src: &[u8], s: &mut usize, tag: u8) -> Result<(usize, usize, usize)> {
    let lit_len = ((tag >> 3) & 3) as usize;

    if *s + 4 > src.len() {
        return Err(Error::Corrupt);
    }
    let val = u32::from_le_bytes([src[*s], src[*s + 1], src[*s + 2], src[*s + 3]]);
    *s += 4;

    let length_val = (val >> 5) & 63;
    let offset = (val >> 11) as usize + MIN_COPY3_OFFSET;

    let length = if length_val <= COPY3_LENGTH_MAX_DIRECT as u32 {
        COPY3_BASE_LENGTH + length_val as usize
    } else if length_val == COPY3_LENGTH_1_BYTE as u32 {
        if *s >= src.len() {
            return Err(Error::Corrupt);
        }
        let extra_len = src[*s];
        *s += 1;
        COPY3_EXTENDED_BASE + extra_len as usize
    } else if length_val == COPY3_LENGTH_2_BYTE as u32 {
        if *s + 2 > src.len() {
            return Err(Error::Corrupt);
        }
        let extra_len = u16::from_le_bytes([src[*s], src[*s + 1]]);
        *s += 2;
        COPY3_EXTENDED_BASE + extra_len as usize
    } else if length_val == COPY3_LENGTH_3_BYTE as u32 {
        if *s + 3 > src.len() {
            return Err(Error::Corrupt);
        }
        let extra_len = u32::from_le_bytes([src[*s], src[*s + 1], src[*s + 2], 0]);
        *s += 3;
        COPY3_EXTENDED_BASE + extra_len as usize
    } else {
        return Err(Error::Corrupt);
    };

    Ok((offset, length, lit_len))
}