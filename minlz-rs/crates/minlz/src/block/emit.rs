//! Encode primitives: literal, repeat, copy1/2/3, fused copy2/3.
//!
//! Ported from `asm_none.go` (`emitLiteral`, `emitRepeat`, `emitCopy`,
//! `emitCopyLits2`, `emitCopyLits3`, `encodeCopy3`) and `encode.go`
//! (`encodeCopy2`).  These functions assume `dst` is large enough; the
//! caller pre-allocates `max_encoded_len(src.len())` bytes.

use super::format::{
    MAX_COPY1_OFFSET, MAX_COPY2_LITS, MAX_COPY2_OFFSET, MAX_COPY3_LITS, MIN_COPY2_OFFSET,
    MIN_COPY3_OFFSET, TAG_COPY1, TAG_COPY2, TAG_COPY2_FUSED, TAG_COPY3, TAG_LITERAL, TAG_REPEAT,
};
use super::load_store::{store16, store32, store8};

/// Write a literal chunk and return the number of bytes written.
///
/// Port of `asm_none.go:emitLiteral`.  `dst` must hold at least
/// `lit.len() + 4` bytes.
#[inline]
pub(super) fn emit_literal(dst: &mut [u8], lit: &[u8]) -> usize {
    if lit.is_empty() {
        return 0;
    }
    let n = lit.len() - 1;
    let header = match n {
        0..=28 => {
            // SAFETY: `dst.len() >= lit.len() + 4 >= 1`.
            unsafe { store8(dst, 0, ((n as u8) << 3) | TAG_LITERAL) };
            1
        }
        29..=283 => {
            // n - 29 fits in 1 byte.  29<<3|TAG_LITERAL == 0xe8.
            // SAFETY: `dst.len() >= 2`.
            unsafe {
                store8(dst, 1, (n - 29) as u8);
                store8(dst, 0, (29 << 3) | TAG_LITERAL);
            }
            2
        }
        284..=65564 => {
            let n2 = n - 29;
            // SAFETY: `dst.len() >= 3`.
            unsafe {
                store8(dst, 2, (n2 >> 8) as u8);
                store8(dst, 1, n2 as u8);
                store8(dst, 0, (30 << 3) | TAG_LITERAL);
            }
            3
        }
        65565..=16_777_244 => {
            let n2 = n - 29;
            // SAFETY: `dst.len() >= 4`.
            unsafe {
                store8(dst, 3, (n2 >> 16) as u8);
                store8(dst, 2, (n2 >> 8) as u8);
                store8(dst, 1, n2 as u8);
                store8(dst, 0, (31 << 3) | TAG_LITERAL);
            }
            4
        }
        _ => panic!("literal block too long: {}", lit.len()),
    };
    dst[header..header + lit.len()].copy_from_slice(lit);
    header + lit.len()
}

/// Write a repeat chunk and return the number of bytes written.
///
/// Port of `asm_none.go:emitRepeat`.
#[inline]
pub(super) fn emit_repeat(dst: &mut [u8], length: usize) -> usize {
    debug_assert!(length > 0, "emit_repeat length must be > 0");
    if length < 30 {
        // SAFETY: dst.len() >= 1.
        unsafe { store8(dst, 0, (((length - 1) as u8) << 3) | TAG_REPEAT) };
        return 1;
    }
    let length = length - 30;
    if length < 256 {
        // SAFETY: dst.len() >= 2.
        unsafe {
            store8(dst, 1, length as u8);
            store8(dst, 0, (29 << 3) | TAG_REPEAT);
        }
        return 2;
    }
    if length < 65536 {
        // SAFETY: dst.len() >= 3.
        unsafe {
            store8(dst, 2, (length >> 8) as u8);
            store8(dst, 1, length as u8);
            store8(dst, 0, (30 << 3) | TAG_REPEAT);
        }
        return 3;
    }
    // SAFETY: dst.len() >= 4.
    unsafe {
        store8(dst, 3, (length >> 16) as u8);
        store8(dst, 2, (length >> 8) as u8);
        store8(dst, 1, length as u8);
        store8(dst, 0, (31 << 3) | TAG_REPEAT);
    }
    4
}

/// Write a Copy2 chunk (no fused literals).
///
/// Port of `encode.go:encodeCopy2`.  Requires `offset ∈ [64, 65599]`,
/// `length >= 4`.
#[inline]
pub(super) fn encode_copy2(dst: &mut [u8], offset: usize, length: usize) -> usize {
    debug_assert!((MIN_COPY2_OFFSET..=MAX_COPY2_OFFSET).contains(&offset));
    debug_assert!(length >= 4);
    let length = length - 4;
    let offset_field = (offset - MIN_COPY2_OFFSET) as u16;
    // SAFETY: dst.len() >= 6.
    unsafe { store16(dst, 1, offset_field) };
    if length <= 60 {
        // SAFETY: see above.
        unsafe { store8(dst, 0, ((length as u8) << 2) | TAG_COPY2) };
        return 3;
    }
    let length = length - 60;
    if length < 256 {
        unsafe {
            store8(dst, 3, length as u8);
            store8(dst, 0, (61 << 2) | TAG_COPY2);
        }
        return 4;
    }
    if length < 65536 {
        unsafe {
            store8(dst, 4, (length >> 8) as u8);
            store8(dst, 3, length as u8);
            store8(dst, 0, (62 << 2) | TAG_COPY2);
        }
        return 5;
    }
    unsafe {
        store8(dst, 5, (length >> 16) as u8);
        store8(dst, 4, (length >> 8) as u8);
        store8(dst, 3, length as u8);
        store8(dst, 0, (63 << 2) | TAG_COPY2);
    }
    6
}

/// Encode a Copy3 chunk with `lits` fused literals (0..=3).
///
/// Port of `asm_none.go:encodeCopy3`.  Does *not* write the literal bytes;
/// the caller is responsible for appending them after the returned tag.
#[inline]
fn encode_copy3(dst: &mut [u8], offset: usize, length: usize, lits: usize) -> usize {
    debug_assert!(offset >= MIN_COPY3_OFFSET);
    debug_assert!(length >= 4);
    debug_assert!(lits <= MAX_COPY3_LITS);
    let length = length - 4;
    let mut encoded: u32 =
        ((offset - MIN_COPY3_OFFSET) as u32) << 11 | TAG_COPY3 as u32 | ((lits as u32) << 3);
    if length <= 60 {
        encoded |= (length as u32) << 5;
        // SAFETY: dst.len() >= 7.
        unsafe { store32(dst, 0, encoded) };
        return 4;
    }
    let length = length - 60;
    if length < 256 {
        encoded |= 61 << 5;
        unsafe {
            store32(dst, 0, encoded);
            store8(dst, 4, length as u8);
        }
        return 5;
    }
    if length < 65536 {
        encoded |= 62 << 5;
        unsafe {
            store32(dst, 0, encoded);
            store16(dst, 4, length as u16);
        }
        return 6;
    }
    encoded |= 63 << 5;
    unsafe {
        store32(dst, 0, encoded);
        store8(dst, 4, length as u8);
        store8(dst, 5, (length >> 8) as u8);
        store8(dst, 6, (length >> 16) as u8);
    }
    7
}

/// Write a copy chunk (Copy1/Copy2/Copy3) using the smallest encoding that
/// fits the given `(offset, length)`.
///
/// Port of `asm_none.go:emitCopy`.
#[inline]
pub(super) fn emit_copy(dst: &mut [u8], offset: usize, length: usize) -> usize {
    debug_assert!(offset > 0 && offset <= super::format::MAX_COPY3_OFFSET);
    debug_assert!(length >= 4);

    if offset > MAX_COPY2_OFFSET {
        return encode_copy3(dst, offset, length, 0);
    }
    if offset <= MAX_COPY1_OFFSET {
        let off = offset - 1;
        if length < 15 + 4 {
            let x = ((off as u16) << 6) | (((length - 4) as u16) << 2) | TAG_COPY1 as u16;
            // SAFETY: dst.len() >= 2.
            unsafe { store16(dst, 0, x) };
            return 2;
        }
        if length < 256 + 18 {
            let x = ((off as u16) << 6) | ((15u16) << 2) | TAG_COPY1 as u16;
            // SAFETY: dst.len() >= 3.
            unsafe {
                store16(dst, 0, x);
                store8(dst, 2, (length - 18) as u8);
            }
            return 3;
        }
        // Long copy: emit a maximum-length Copy1 (value 14 ⇒ length 18) and
        // chain the remainder via a Repeat.
        let x = ((off as u16) << 6) | ((14u16) << 2) | TAG_COPY1 as u16;
        // SAFETY: dst.len() >= 2.
        unsafe { store16(dst, 0, x) };
        return 2 + emit_repeat(&mut dst[2..], length - 18);
    }
    encode_copy2(dst, offset, length)
}

/// Emit a Copy2 chunk fused with 1..=4 literals.
///
/// `lits.len()` must be in `1..=MAX_COPY2_LITS`.  `offset` must lie in
/// `[MIN_COPY2_OFFSET, MAX_COPY2_OFFSET]`.  Long copies (length > 11) are
/// emitted as fused-Copy2 + Repeat.
#[inline]
pub(super) fn emit_copy_lits2(dst: &mut [u8], lits: &[u8], offset: usize, length: usize) -> usize {
    debug_assert!((MIN_COPY2_OFFSET..=MAX_COPY2_OFFSET).contains(&offset));
    debug_assert!(!lits.is_empty() && lits.len() <= MAX_COPY2_LITS);
    debug_assert!(length >= 4);

    let off_field = (offset - MIN_COPY2_OFFSET) as u16;
    let raw_len = length - 4;
    const COPY2_LIT_MAX_LEN_RAW: usize = super::format::COPY2_LIT_MAX_LEN - 4;

    if raw_len > COPY2_LIT_MAX_LEN_RAW {
        // Emit the maximum-length fused Copy2 and chain a Repeat.
        let tag = TAG_COPY2_FUSED
            | (((COPY2_LIT_MAX_LEN_RAW) as u8) << 5)
            | (((lits.len() - 1) as u8) << 3);
        // SAFETY: dst.len() >= 3 + lits.len() + repeat-overhead.
        unsafe {
            store16(dst, 1, off_field);
            store8(dst, 0, tag);
        }
        dst[3..3 + lits.len()].copy_from_slice(lits);
        let n = 3 + lits.len();
        return n + emit_repeat(&mut dst[n..], raw_len - COPY2_LIT_MAX_LEN_RAW);
    }

    let tag = TAG_COPY2_FUSED | ((raw_len as u8) << 5) | (((lits.len() - 1) as u8) << 3);
    // SAFETY: dst.len() >= 3 + lits.len().
    unsafe {
        store16(dst, 1, off_field);
        store8(dst, 0, tag);
    }
    dst[3..3 + lits.len()].copy_from_slice(lits);
    3 + lits.len()
}

/// Return the number of bytes [`emit_literal`] would use for a literal of
/// length `n` (the header only, not the payload).  Port of
/// `encode.go:emitLiteralSizeN`.
#[inline]
pub(super) fn emit_literal_size_n(n: usize) -> usize {
    if n == 0 {
        0
    } else if n <= 29 {
        1
    } else if n < 29 + (1 << 8) {
        2
    } else if n < 29 + (1 << 16) {
        3
    } else {
        4
    }
}

/// Return the encoded size of an [`emit_repeat`] call.  Port of
/// `encode_l3.go:emitRepeatSize`.
#[inline]
pub(super) fn emit_repeat_size(length: usize) -> usize {
    if length == 0 {
        return 0;
    }
    if length <= 29 {
        return 1;
    }
    let length = length - 29;
    if length <= 256 {
        return 2;
    }
    if length <= 65536 {
        return 3;
    }
    4
}

/// Return the encoded size of an [`encode_copy2`] call (no literals).
/// Port of `encode_l3.go:emitCopy2Size`.
#[inline]
pub(super) fn emit_copy2_size(length: usize) -> usize {
    let length = length - 4;
    if length <= 60 {
        return 3;
    }
    let length = length - 60;
    if length < 256 {
        return 4;
    }
    if length < 65536 {
        return 5;
    }
    6
}

/// Return the encoded size of an [`emit_copy`] call.  Port of
/// `encode_l3.go:emitCopySize`.
#[inline]
pub(super) fn emit_copy_size(offset: usize, length: usize) -> usize {
    if offset > 65536 + 63 {
        // Copy3: 4-byte head + 0..3 extra length bytes.
        if length <= 64 {
            return 4;
        }
        let extra = length - 64;
        4 + ((usize::BITS - extra.leading_zeros()) as usize).div_ceil(8)
    } else if offset <= MAX_COPY1_OFFSET {
        if length <= 18 {
            2
        } else if length < 18 + 256 {
            3
        } else {
            2 + emit_repeat_size(length - 18)
        }
    } else {
        emit_copy2_size(length)
    }
}

/// Emit a Copy3 chunk fused with 1..=3 literals.
///
/// `lits.len()` must be in `1..=MAX_COPY3_LITS`.
#[inline]
pub(super) fn emit_copy_lits3(dst: &mut [u8], lits: &[u8], offset: usize, length: usize) -> usize {
    debug_assert!(offset >= MIN_COPY3_OFFSET);
    debug_assert!(!lits.is_empty() && lits.len() <= MAX_COPY3_LITS);
    let n = encode_copy3(dst, offset, length, lits.len());
    dst[n..n + lits.len()].copy_from_slice(lits);
    n + lits.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ports `TestEmitLiteral` from `minlz_test.go:874`.
    #[test]
    fn emit_literal_matches_go() {
        let cases: &[(usize, &[u8])] = &[
            (1, b"\x00"),
            (2, b"\x08"),
            (27, b"\xd0"),
            (28, b"\xd8"),
            (29, b"\xe0"),
            (30, b"\xe8\x00"),
            (59, b"\xe8\x1d"),
            (60, b"\xe8\x1e"),
            (61, b"\xe8\x1f"),
            (62, b"\xe8\x20"),
            (254, b"\xe8\xe0"),
            (255, b"\xe8\xe1"),
            (256, b"\xe8\xe2"),
            (257, b"\xe8\xe3"),
            (65534, b"\xf0\xe0\xff"),
            (65535, b"\xf0\xe1\xff"),
            (65536, b"\xf0\xe2\xff"),
            (165536, b"\xf8\x82\x86\x02"),
        ];
        let mut dst = vec![0u8; super::super::format::MAX_BLOCK_SIZE + 16];
        let nines = vec![0x99u8; super::super::format::MAX_BLOCK_SIZE];
        for &(len, want_header) in cases {
            let lit = &nines[..len];
            let n = emit_literal(&mut dst, lit);
            assert!(
                dst[n - len..n].iter().all(|&b| b == 0x99),
                "length={len}: did not end with literal bytes"
            );
            let got_header = &dst[..n - len];
            assert_eq!(
                got_header, want_header,
                "length={len}: header mismatch\ngot  {got_header:?}\nwant {want_header:?}"
            );
        }
    }

    /// Ports `TestEmitCopy` from `minlz_test.go:916`.
    #[test]
    fn emit_copy_matches_go() {
        let cases: &[(usize, usize, &[u8])] = &[
            // 10-bit offsets (copy1).
            (8, 4, &[0xc1, 0x01]),
            (8, 11, &[0xdd, 0x01]),
            (8, 12, &[0xe1, 0x01]),
            (8, 13, &[0xe5, 0x01]),
            (8, 17, &[0xf5, 0x01]),
            (8, 18, &[0xf9, 0x01]),
            (8, 19, &[0xfd, 0x01, 0x01]),
            (8, 59, &[0xfd, 0x01, 0x29]),
            (8, 60, &[0xfd, 0x01, 0x2a]),
            (8, 61, &[0xfd, 0x01, 0x2b]),
            (8, 62, &[0xfd, 0x01, 0x2c]),
            (8, 63, &[0xfd, 0x01, 0x2d]),
            (8, 64, &[0xfd, 0x01, 0x2e]),
            (8, 65, &[0xfd, 0x01, 0x2f]),
            (8, 66, &[0xfd, 0x01, 0x30]),
            (8, 67, &[0xfd, 0x01, 0x31]),
            (8, 68, &[0xfd, 0x01, 0x32]),
            (8, 69, &[0xfd, 0x01, 0x33]),
            (8, 80, &[0xfd, 0x01, 0x3e]),
            (8, 800, &[0xf9, 0x01, 0xf4, 0xf0, 0x02]),
            (8, 800_000, &[0xf9, 0x01, 0xfc, 0xd0, 0x34, 0x0c]),
            (256, 4, &[0xc1, 0x3f]),
            (256, 11, &[0xdd, 0x3f]),
            (256, 12, &[0xe1, 0x3f]),
            (256, 13, &[0xe5, 0x3f]),
            (256, 18, &[0xf9, 0x3f]),
            (256, 19, &[0xfd, 0x3f, 0x01]),
            (256, 59, &[0xfd, 0x3f, 0x29]),
            (256, 60, &[0xfd, 0x3f, 0x2a]),
            (256, 61, &[0xfd, 0x3f, 0x2b]),
            (256, 62, &[0xfd, 0x3f, 0x2c]),
            (256, 63, &[0xfd, 0x3f, 0x2d]),
            (256, 64, &[0xfd, 0x3f, 0x2e]),
            (256, 65, &[0xfd, 0x3f, 0x2f]),
            (256, 66, &[0xfd, 0x3f, 0x30]),
            (256, 67, &[0xfd, 0x3f, 0x31]),
            (256, 68, &[0xfd, 0x3f, 0x32]),
            (256, 69, &[0xfd, 0x3f, 0x33]),
            (256, 80, &[0xfd, 0x3f, 0x3e]),
            (256, 800, &[0xf9, 0x3f, 0xf4, 0xf0, 0x02]),
            (256, 80_000, &[0xf9, 0x3f, 0xfc, 0x50, 0x38, 0x01]),
            // 16-bit offsets (copy2).
            (2048, 4, &[0x02, 0xc0, 0x07]),
            (2048, 11, &[0x1e, 0xc0, 0x07]),
            (2048, 12, &[0x22, 0xc0, 0x07]),
            (2048, 13, &[0x26, 0xc0, 0x07]),
            (2048, 59, &[0xde, 0xc0, 0x07]),
            (2048, 60, &[0xe2, 0xc0, 0x07]),
            (2048, 61, &[0xe6, 0xc0, 0x07]),
            (2048, 62, &[0xea, 0xc0, 0x07]),
            (2048, 63, &[0xee, 0xc0, 0x07]),
            (2048, 64, &[0xf2, 0xc0, 0x07]),
            (2048, 65, &[0xf6, 0xc0, 0x07, 0x01]),
            (2048, 66, &[0xf6, 0xc0, 0x07, 0x02]),
            (2048, 67, &[0xf6, 0xc0, 0x07, 0x03]),
            (2048, 68, &[0xf6, 0xc0, 0x07, 0x04]),
            (2048, 69, &[0xf6, 0xc0, 0x07, 0x05]),
            (2048, 80, &[0xf6, 0xc0, 0x07, 0x10]),
            (2048, 800, &[0xfa, 0xc0, 0x07, 0xe0, 0x02]),
            (2048, 80_000, &[0xfe, 0xc0, 0x07, 0x40, 0x38, 0x01]),
            // 22-bit offsets (copy3).
            (204_800, 4, &[0x07, 0x00, 0x00, 0x11]),
            (204_800, 28, &[0x07, 0x03, 0x00, 0x11]),
            (204_800, 32, &[0x87, 0x03, 0x00, 0x11]),
            (204_800, 33, &[0xa7, 0x03, 0x00, 0x11]),
            (204_800, 40, &[0x87, 0x04, 0x00, 0x11]),
            (204_800, 65, &[0xa7, 0x07, 0x00, 0x11, 0x01]),
            (204_800, 69, &[0xa7, 0x07, 0x00, 0x11, 0x05]),
            (204_800, 800, &[0xc7, 0x07, 0x00, 0x11, 0xe0, 0x02]),
            (204_800, 80_000, &[0xe7, 0x07, 0x00, 0x11, 0x40, 0x38, 0x01]),
        ];
        let mut dst = [0u8; 32];
        for &(offset, length, want) in cases {
            let n = emit_copy(&mut dst, offset, length);
            let got = &dst[..n];
            assert_eq!(
                got, want,
                "offset={offset} length={length}\ngot  {got:?}\nwant {want:?}"
            );
        }
    }
}
