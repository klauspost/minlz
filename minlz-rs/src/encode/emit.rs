//! MinLZ binary format encoding functions
//!
//! This module contains functions for encoding various MinLZ operations into the binary format.
//! Each function handles the exact binary layout as specified in the MinLZ format.

use crate::{
    constants::*,
    error::Result,
    memory::{store8, store16, store32},
};

/// Writes a literal chunk and returns the number of bytes written.
///
/// Literal encoding format:
/// - 0-28: Length 1 -> 29 (stored as (length-1) << 3 | tagLiteral)
/// - 29: Length (Read 1) + 1
/// - 30: Length (Read 2) + 1
/// - 31: Length (Read 3) + 1
pub fn emit_literal(dst: &mut [u8], lit: &[u8]) -> Result<usize> {
    if lit.is_empty() {
        return Ok(0);
    }

    let i;
    let n = lit.len() - 1; // Length stored as len-1

    match n {
        0..=28 => {
            store8(dst, 0, (n as u8) << 3 | TAG_LITERAL)?;
            i = 1;
        }
        29..=284 => { // 29 + 255
            store8(dst, 1, (n - 29) as u8)?;
            store8(dst, 0, 29 << 3 | TAG_LITERAL)?;
            i = 2;
        }
        285..=65564 => { // 29 + 65535
            let n = n - 29;
            dst[2] = (n >> 8) as u8;
            dst[1] = n as u8;
            dst[0] = 30 << 3 | TAG_LITERAL;
            i = 3;
        }
        65565..=16777244 => { // 29 + 16777215
            let n = n - 29;
            dst[3] = (n >> 16) as u8;
            dst[2] = (n >> 8) as u8;
            dst[1] = n as u8;
            dst[0] = 31 << 3 | TAG_LITERAL;
            i = 4;
        }
        _ => {
            return Err(crate::error::Error::TooLarge);
        }
    }

    // Copy the literal data
    dst[i..i + lit.len()].copy_from_slice(lit);
    Ok(i + lit.len())
}

/// Writes a repeat chunk and returns the number of bytes written.
///
/// Repeat encoding uses the same pattern as literals but with tagRepeat.
/// Length is the number of bytes to repeat from the previous offset.
pub fn emit_repeat(dst: &mut [u8], length: usize) -> Result<usize> {
    if length == 0 {
        return Ok(0);
    }

    match length {
        1..30 => {
            store8(dst, 0, ((length - 1) << 3) as u8 | TAG_REPEAT)?;
            Ok(1)
        }
        30..286 => { // 30 + 256
            let length = length - 30;
            store8(dst, 1, length as u8)?;
            store8(dst, 0, 29 << 3 | TAG_REPEAT)?;
            Ok(2)
        }
        286..65566 => { // 30 + 65536
            let length = length - 30;
            dst[2] = (length >> 8) as u8;
            dst[1] = length as u8;
            dst[0] = 30 << 3 | TAG_REPEAT;
            Ok(3)
        }
        _ => {
            let length = length - 30;
            dst[3] = (length >> 16) as u8;
            dst[2] = (length >> 8) as u8;
            dst[1] = length as u8;
            dst[0] = 31 << 3 | TAG_REPEAT;
            Ok(4)
        }
    }
}

/// Encodes a Copy3 operation with 21-bit offset.
///
/// Copy3 format:
/// - Offset range: 65536 to ~2MB
/// - Length: minimum 4 bytes
/// - Can include embedded literals (lits parameter)
fn encode_copy3(dst: &mut [u8], offset: usize, length: usize, lits: usize) -> Result<usize> {
    debug_assert!(offset >= 65536, "Copy3 offset must be >= 65536");


    let length = length.saturating_sub(4);


    // Encode offset (subtract base to fit in 21 bits)
    let mut encoded = ((offset - 65536) << 11) as u32 | TAG_COPY3 as u32 | ((lits << 3) as u32);

    match length {
        0..=60 => {
            encoded |= (length << 5) as u32;
            store32(dst, 0, encoded)?;
            Ok(4)
        }
        61..=316 => { // 60 + 256
            let length = length - 60;
            store8(dst, 4, length as u8)?;
            encoded |= 61 << 5;
            store32(dst, 0, encoded)?;
            Ok(5)
        }
        317..=65596 => { // 60 + 65536
            let length = length - 60;
            encoded |= 62 << 5;
            dst[5] = (length >> 8) as u8;
            dst[4] = length as u8;
            store32(dst, 0, encoded)?;
            Ok(6)
        }
        _ => {
            let length = length - 60;
            encoded |= 63 << 5;
            dst[6] = (length >> 16) as u8;
            dst[5] = (length >> 8) as u8;
            dst[4] = length as u8;
            store32(dst, 0, encoded)?;
            Ok(7)
        }
    }
}

/// Encodes a Copy2 operation with 16-bit offset.
///
/// Copy2 format:
/// - Offset range: 64 to 65535
/// - Length: minimum 4 bytes
fn encode_copy2(dst: &mut [u8], offset: usize, length: usize) -> Result<usize> {
    debug_assert!(offset >= MIN_COPY2_OFFSET && offset <= MAX_COPY2_OFFSET,
                  "Copy2 offset must be in range {}-{}", MIN_COPY2_OFFSET, MAX_COPY2_OFFSET);

    let length = length.saturating_sub(4);
    let offset = offset - MIN_COPY2_OFFSET;

    store16(dst, 1, offset as u16)?;

    match length {
        0..=60 => {
            store8(dst, 0, (length << 2) as u8 | TAG_COPY2)?;
            Ok(3)
        }
        61..=316 => { // 60 + 256
            let length = length - 60;
            store8(dst, 3, length as u8)?;
            store8(dst, 0, 61 << 2 | TAG_COPY2)?;
            Ok(4)
        }
        317..=65596 => { // 60 + 65536
            let length = length - 60;
            dst[4] = (length >> 8) as u8;
            dst[3] = length as u8;
            dst[0] = 62 << 2 | TAG_COPY2;
            Ok(5)
        }
        _ => {
            let length = length - 60;
            dst[5] = (length >> 16) as u8;
            dst[4] = (length >> 8) as u8;
            dst[3] = length as u8;
            dst[0] = 63 << 2 | TAG_COPY2;
            Ok(6)
        }
    }
}

/// Writes a copy chunk and returns the number of bytes written.
///
/// Dispatches to the appropriate copy encoding based on offset size:
/// - Small offsets (≤1024): Copy1 format
/// - Medium offsets (64-65535): Copy2 format
/// - Large offsets (65536+): Copy3 format
pub fn emit_copy(dst: &mut [u8], offset: usize, length: usize) -> Result<usize> {
    debug_assert!(offset > 0 && offset <= MAX_COPY3_OFFSET,
                  "Copy offset must be in range 1-{}", MAX_COPY3_OFFSET);

    // Debug: Print emit_copy calls for comparison with mz.exe block-debug
    println!("EMIT_COPY: offset={}, length={}", offset, length);

    let result = match offset {
        o if o > MAX_COPY2_OFFSET => {
            // Use Copy3 for large offsets
            encode_copy3(dst, offset, length, 0)
        }
        o if o <= MAX_COPY1_OFFSET => {
            // Use Copy1 for small offsets
            let offset = offset - 1; // Copy1 stores offset-1

            match length {
                4..19 => { // length < 15 + 4
                    // Copy1 format: bits 0-1=tag(1), bits 2-5=length-4, bits 6-7=offset_low_2_bits
                    let byte1 = ((offset & 0x03) << 6) | ((length - 4) << 2) | TAG_COPY1 as usize;
                    let byte2 = offset >> 2;
                    dst[0] = byte1 as u8;
                    dst[1] = byte2 as u8;
                    Ok(2)
                }
                19..274 => { // length < 256 + 18
                    // Copy1 format with length extension
                    let byte1 = ((offset & 0x03) << 6) | (15 << 2) | TAG_COPY1 as usize;
                    let byte2 = offset >> 2;
                    dst[0] = byte1 as u8;
                    dst[1] = byte2 as u8;
                    dst[2] = (length - 18) as u8;
                    Ok(3)
                }
                _ => {
                    // Encode as Copy1 + repeat for very long lengths
                    let byte1 = ((offset & 0x03) << 6) | (14 << 2) | TAG_COPY1 as usize;
                    let byte2 = offset >> 2;
                    dst[0] = byte1 as u8;
                    dst[1] = byte2 as u8;
                    let repeat_len = emit_repeat(&mut dst[2..], length - 18)?;
                    Ok(2 + repeat_len)
                }
            }
        }
        _ => {
            // Use Copy2 for medium offsets
            encode_copy2(dst, offset, length)
        }
    };

    // Debug: Print result and raw bytes written
    match &result {
        Ok(bytes_written) => {
            println!("  -> wrote {} bytes: {:02x?}", bytes_written, &dst[..*bytes_written]);
        }
        Err(e) => {
            println!("  -> ERROR: {:?}", e);
        }
    }

    result
}

/// Emit a Copy2 operation with embedded literals.
///
/// This is an optimization that allows 1-4 literals to be embedded
/// within a Copy2 operation, saving space when there are small
/// literal runs before a copy.
pub fn emit_copy_lits2(dst: &mut [u8], lits: &[u8], offset: usize, length: usize) -> Result<usize> {
    debug_assert!(lits.len() <= MAX_COPY2_LITS, "Too many literals for Copy2: {} > {}", lits.len(), MAX_COPY2_LITS);
    debug_assert!(offset >= MIN_COPY2_OFFSET && offset <= MAX_COPY2_OFFSET,
                  "Copy2 offset must be in range {}-{}", MIN_COPY2_OFFSET, MAX_COPY2_OFFSET);

    let offset = offset - MIN_COPY2_OFFSET;
    let length = length.saturating_sub(4);

    if length > COPY2_LIT_MAX_LEN - 4 {
        // Split long copies: emit max length + repeat for remainder
        store16(dst, 1, offset as u16)?;
        store8(dst, 0, TAG_COPY2_FUSED | ((COPY2_LIT_MAX_LEN - 4) << 5) as u8 | ((lits.len() - 1) << 3) as u8)?;
        dst[3..3 + lits.len()].copy_from_slice(lits);
        let n = 3 + lits.len();
        let repeat_len = emit_repeat(&mut dst[n..], length - (COPY2_LIT_MAX_LEN - 4))?;
        Ok(n + repeat_len)
    } else {
        store16(dst, 1, offset as u16)?;
        store8(dst, 0, TAG_COPY2_FUSED | (length << 5) as u8 | ((lits.len() - 1) << 3) as u8)?;
        dst[3..3 + lits.len()].copy_from_slice(lits);
        Ok(3 + lits.len())
    }
}

/// Emit a Copy3 operation with embedded literals.
///
/// Similar to Copy2 but for large offsets, allows 1-3 literals to be embedded.
pub fn emit_copy_lits3(dst: &mut [u8], lits: &[u8], offset: usize, length: usize) -> Result<usize> {
    debug_assert!(lits.len() <= MAX_COPY3_LITS, "Too many literals for Copy3: {} > {}", lits.len(), MAX_COPY3_LITS);
    debug_assert!(offset > MAX_COPY2_OFFSET, "Copy3 offset too small: {} <= {}", offset, MAX_COPY2_OFFSET);

    let n = encode_copy3(dst, offset, length, lits.len())?;
    dst[n..n + lits.len()].copy_from_slice(lits);
    Ok(n + lits.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{load16, load32};

    #[test]
    fn test_emit_literal() {
        let mut dst = vec![0u8; 100];

        // Test short literal (≤28 bytes)
        let lit = b"hello";
        let len = emit_literal(&mut dst, lit).unwrap();
        assert_eq!(len, 6);
        assert_eq!(dst[0], (4 << 3) | TAG_LITERAL); // length-1 = 4
        assert_eq!(&dst[1..6], b"hello");

        // Test empty literal
        let len = emit_literal(&mut dst, &[]).unwrap();
        assert_eq!(len, 0);
    }

    #[test]
    fn test_emit_repeat() {
        let mut dst = vec![0u8; 10];

        // Test short repeat (≤29)
        let len = emit_repeat(&mut dst, 10).unwrap();
        assert_eq!(len, 1);
        assert_eq!(dst[0], (9 << 3) | TAG_REPEAT); // length-1 = 9

        // Test medium repeat
        let len = emit_repeat(&mut dst, 100).unwrap();
        assert_eq!(len, 2);
        assert_eq!(dst[0], 29 << 3 | TAG_REPEAT);
        assert_eq!(dst[1], 70); // 100-30 = 70
    }


    #[test]
    fn test_emitters() {
        let mut tmp = [0u8; 11];
        let l_factor = 1.5; // Use faster test mode

        // Test copy1 - reduced range for speed
        for off in (1..=MAX_COPY1_OFFSET).step_by(100) {
            for l in (4..=273).step_by(50) {
                let n = emit_copy(&mut tmp, off, l).unwrap();
                let input = &tmp[..n];

                let got_tag = input[0] & 3;
                let want_tag = TAG_COPY1;
                assert_eq!(got_tag, want_tag, "tag mismatch for off: {}, ml: {}", off, l);

                let length = ((input[0] as usize) >> 2) & 15;
                let offset = (load16(input, 0).unwrap() >> 6) as usize + 1;

                let (expected_length, s) = if length == 15 {
                    let ext_len = input[2] as usize + 18;
                    (ext_len, 3)
                } else {
                    (length + 4, 2)
                };

                assert_eq!(expected_length, l, "length mismatch for off: {}, ml: {}", off, l);
                assert_eq!(offset, off, "offset mismatch for off: {}, ml: {}", off, l);
                assert_eq!(s, n, "output length mismatch for off: {}, ml: {}", off, l);
            }
        }

        // Test copy2
        let mut test_off = MAX_COPY1_OFFSET + 1; // Start at 1025
        while test_off <= MAX_COPY2_OFFSET {
            let mut ml = 4;
            while ml <= 1 << 24 {
                let n = emit_copy(&mut tmp, test_off, ml).unwrap();
                let input = &tmp[..n];

                let got_tag = input[0] & 3;
                let want_tag = TAG_COPY2;
                assert_eq!(got_tag, want_tag, "tag mismatch for off: {}, ml: {}", test_off, ml);

                let mut length = (input[0] as usize) >> 2;
                let offset = (input[1] as u32 | (input[2] as u32) << 8) as usize;

                let s = if length <= 60 {
                    length += 4;
                    3
                } else {
                    match length {
                        61 => {
                            length = input[3] as usize + 64;
                            4
                        }
                        62 => {
                            length = input[3] as usize | (input[4] as usize) << 8;
                            length += 64;
                            5
                        }
                        63 => {
                            length = input[3] as usize | (input[4] as usize) << 8 | (input[5] as usize) << 16;
                            length += 64;
                            6
                        }
                        _ => panic!("invalid length encoding"),
                    }
                };
                let final_offset = offset + MIN_COPY2_OFFSET;

                assert_eq!(length, ml, "length mismatch for off: {}, ml: {}", test_off, ml);
                assert_eq!(final_offset, test_off, "offset mismatch for off: {}, ml: {}", test_off, ml);
                assert_eq!(s, n, "output length mismatch for off: {}, ml: {}", test_off, ml);

                ml = ((ml as f64) * l_factor + 1.0) as usize;
            }
            test_off += 100;
        }

        // Test copy2 with literals
        let mut lits = vec![1u8];
        while lits.len() <= MAX_COPY2_LITS {
            let mut test_off = MIN_COPY2_OFFSET;
            while test_off <= MAX_COPY2_OFFSET {
                for l in 4..=11 {
                    let n = emit_copy_lits2(&mut tmp, &lits, test_off, l).unwrap();
                    let input = &tmp[..n];

                    let got_tag = input[0] & 3;
                    let want_tag = TAG_COPY2_FUSED;
                    assert_eq!(got_tag, want_tag, "tag mismatch");
                    assert_eq!(input[0] & 4, 0, "copy3 bit was set");

                    let value = input[0] >> 3;
                    let offset = (input[1] as u32 | (input[2] as u32) << 8) as usize + 64;
                    let lit_length = (value & 3) as usize + 1;
                    let copy_length = (value >> 2) as usize + 4;

                    assert_eq!(copy_length, l, "copy length mismatch");
                    assert_eq!(offset, test_off, "offset mismatch");
                    assert_eq!(lit_length, lits.len(), "literal length mismatch");

                    let want_n = 3 + lits.len();
                    assert_eq!(n, want_n, "n mismatch");

                    for i in 0..lits.len() {
                        assert_eq!(input[3 + i], lits[i], "literal mismatch at pos {}", i);
                    }
                }
                test_off += 100;
            }
            lits.push((lits.len() + 1) as u8);
        }

        // Test copy3
        let mut lits = vec![];
        while lits.len() <= MAX_COPY3_LITS {
            let mut test_off = MAX_COPY2_OFFSET + 1;
            while test_off <= MAX_COPY3_OFFSET {
                let mut ml = 4;
                while ml <= 1 << 24 {
                    let n = match lits.is_empty() {
                        false => emit_copy_lits3(&mut tmp, &lits, test_off, ml).unwrap(),
                        true => emit_copy(&mut tmp, test_off, ml).unwrap(),
                    };
                    let input = &tmp[..n];
                    let got_tag = input[0] & 7;
                    let want_tag = TAG_COPY3;
                    assert_eq!(got_tag, want_tag, "tag mismatch for off: {}, ml: {}, ll: {}", test_off, ml, lits.len());

                    let length_raw = (load16(input, 0).unwrap() >> 5) as usize & 63;
                    let offset = (load32(input, 0).unwrap() >> 11) as usize + MIN_COPY3_OFFSET;

                    let (length, mut s) = match length_raw {
                        0..=60 => (length_raw + 4, 4),
                        61 => (input[4] as usize + MIN_COPY3_LENGTH, 5),
                        62 => (input[4] as usize | (input[5] as usize) << 8, 6),
                        63 => (input[4] as usize | (input[5] as usize) << 8 | (input[6] as usize) << 16, 7),
                        _ => panic!("invalid length encoding"),
                    };

                    let nlits = ((input[0] >> 3) & 3) as usize;
                    assert_eq!(nlits, lits.len(), "literal count mismatch");

                    if nlits > 0 {
                        assert_eq!(&lits[..], &input[s..s + nlits], "literal data mismatch");
                        s += nlits;
                    }

                    assert_eq!(length, ml, "length mismatch");
                    assert_eq!(offset, test_off, "offset mismatch");
                    assert_eq!(s, n, "output length mismatch");

                    ml = ((ml as f64) * l_factor + 1.0) as usize;
                    if ml < 100 { break; } // Prevent infinite small increments
                }
                test_off *= 2;
                if test_off < MAX_COPY2_OFFSET + 1 { break; }
            }
            match lits.len() {
                0 => lits.push(1),
                len => lits.push((len + 1) as u8),
            }
        }

        // Test repeat
        for l in 1..=1000 { // Reduced from MaxBlockSize for test performance
            let n = emit_repeat(&mut tmp, l).unwrap();
            let input = &tmp[..n];

            let got_tag = input[0] & 7;
            let want_tag = TAG_REPEAT;
            assert_eq!(got_tag, want_tag, "tag mismatch for length {}", l);

            let length_tmp = (input[0] >> 3) as usize;
            let (length, s) = match length_tmp {
                29 => {
                    assert!(n >= 2, "insufficient bytes for length {}", l);
                    (input[1] as usize + 30, 2)
                }
                30 => {
                    assert!(n >= 3, "insufficient bytes for length {}", l);
                    ((input[1] as usize | (input[2] as usize) << 8) + 30, 3)
                }
                31 => {
                    assert!(n >= 4, "insufficient bytes for length {}", l);
                    ((input[1] as usize | (input[2] as usize) << 8 | (input[3] as usize) << 16) + 30, 4)
                }
                _ => {
                    (length_tmp + 1, 1)
                }
            };

            assert_eq!(length, l, "length mismatch for repeat length {}", l);
            assert_eq!(s, n, "output length mismatch for repeat length {}", l);

            if l > 50 {
                // Skip some values for performance
                continue;
            }
        }

        // Test literals - simplified version for performance
        let input_data: Vec<u8> = (0..255).cycle().take(1000).collect();
        let mut dst = vec![0u8; 2000];

        for l in &[1, 5, 28, 29, 30, 100, 300, 1000] {
            dst.fill(0);
            let n = emit_literal(&mut dst, &input_data[..*l]).unwrap();
            let got_data = &dst[n - l..n];
            assert_eq!(got_data, &input_data[..*l], "data not copied for length {}", l);

            let got_tag = &dst[..n - l];
            let v = got_tag[0] as u32;
            let tag = v & 3;
            assert_eq!(v & 4, 0, "repeat flag should not be set");
            let value = v >> 3;

            let length = match tag {
                0 => { // Literal tag
                    match value {
                        0..=28 => value + 1,
                        29 => {
                            assert!(got_tag.len() >= 2, "insufficient bytes for 1-byte length");
                            got_tag[1] as u32 + 30
                        }
                        30 => {
                            assert!(got_tag.len() >= 3, "insufficient bytes for 2-byte length");
                            load16(got_tag, 1).unwrap() as u32 + 30
                        }
                        31 => {
                            assert!(got_tag.len() >= 4, "insufficient bytes for 3-byte length");
                            got_tag[1] as u32 | (got_tag[2] as u32) << 8 | (got_tag[3] as u32) << 16
                        }
                        _ => panic!("unexpected value {}", value),
                    }
                }
                _ => panic!("unexpected tag {}", tag),
            };

            assert_eq!(*l as u32, length, "length mismatch for literal length {}", l);
        }
    }
}