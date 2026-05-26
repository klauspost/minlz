//! Block format constants, tag values, and varint codec.
//!
//! Reference: SPEC.md §1–§2 in the upstream Go repository.

/// Maximum uncompressed block size (8 MiB).  Mirrors Go's `MaxBlockSize`.
pub const MAX_BLOCK_SIZE: usize = 8 << 20;

/// Tag values for the lower 2 bits of a tag byte (SPEC §2 table).
pub(super) const TAG_LITERAL: u8 = 0b00;
pub(super) const TAG_REPEAT: u8 = 1 << 2;
pub(super) const TAG_COPY1: u8 = 0b01;
pub(super) const TAG_COPY2: u8 = 0b10;
pub(super) const TAG_COPY3: u8 = 0b11 | 0b100;
pub(super) const TAG_COPY2_FUSED: u8 = 0b11;

/// Copy offset bounds (true offsets after adding the per-encoding bias).
pub(super) const MAX_COPY1_OFFSET: usize = 1024;
pub(super) const MIN_COPY2_OFFSET: usize = 64;
pub(super) const MAX_COPY2_OFFSET: usize = MIN_COPY2_OFFSET + 65535;
pub(super) const MIN_COPY3_OFFSET: usize = 65536;
pub(super) const MAX_COPY3_OFFSET: usize = (2 << 20) + 65535;

/// Maximum fused-literal counts for each encoding.
pub(super) const COPY_LIT_BITS: u32 = 2;
pub(super) const MAX_COPY2_LITS: usize = 1 << COPY_LIT_BITS;
pub(super) const MAX_COPY3_LITS: usize = (1 << COPY_LIT_BITS) - 1;

/// Maximum length encodable in a fused Copy2 (raw value, the tag stores
/// `length - 4` in 3 bits ⇒ 4..=11).
pub(super) const COPY2_LIT_MAX_LEN: usize = 7 + 4;

/// Encoder loop margin: the hot path keeps at least this many extra bytes
/// after `s` so unaligned 8-byte loads/stores never run past `src`.
pub(super) const INPUT_MARGIN: usize = 8;

/// Minimum input size that the L1/L2 encoders will compress; smaller inputs
/// are emitted uncompressed (Go `minNonLiteralBlockSize`).
pub(super) const MIN_NON_LITERAL_BLOCK_SIZE: usize = 16;

/// Returns the maximum encoded length for a `src` of length `n`, or `None`
/// if `n` exceeds [`MAX_BLOCK_SIZE`].
///
/// Mirrors Go's `MaxEncodedLen`:
/// - `n == 0`  → `1` (the `\x00` empty-block marker).
/// - `0 < n ≤ MAX_BLOCK_SIZE` → `n + 2` (worst case: header + literal body).
/// - `n > MAX_BLOCK_SIZE`     → `None` (Go returns `-1`).
#[inline]
pub fn max_encoded_len(n: usize) -> Option<usize> {
    if n > MAX_BLOCK_SIZE {
        None
    } else if n == 0 {
        Some(1)
    } else {
        Some(n + 2)
    }
}

/// Append the unsigned LEB128 (Go-style `binary.PutUvarint`) of `v` to `dst`.
pub(super) fn put_uvarint(dst: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        dst.push((v as u8) | 0x80);
        v >>= 7;
    }
    dst.push(v as u8);
}

/// Decode an unsigned LEB128 from the head of `src`.
///
/// Returns `Some((value, bytes_read))` on success, `None` if the buffer is
/// too short or the value overflows 64 bits.  Matches Go's `binary.Uvarint`
/// rejection set: `n <= 0` becomes `None`.
pub(super) fn get_uvarint(src: &[u8]) -> Option<(u64, usize)> {
    const MAX_VARINT_LEN_64: usize = 10;
    let mut x: u64 = 0;
    let mut s: u32 = 0;
    for (i, &b) in src.iter().enumerate() {
        if i == MAX_VARINT_LEN_64 {
            return None;
        }
        if b < 0x80 {
            if i == MAX_VARINT_LEN_64 - 1 && b > 1 {
                return None;
            }
            return Some((x | ((b as u64) << s), i + 1));
        }
        x |= ((b & 0x7f) as u64) << s;
        s += 7;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_encoded_len_matches_go() {
        assert_eq!(max_encoded_len(0), Some(1));
        assert_eq!(max_encoded_len(1), Some(3));
        assert_eq!(max_encoded_len(MAX_BLOCK_SIZE), Some(MAX_BLOCK_SIZE + 2));
        assert_eq!(max_encoded_len(MAX_BLOCK_SIZE + 1), None);
    }

    #[test]
    fn varint_roundtrip() {
        for &v in &[
            0u64,
            1,
            127,
            128,
            255,
            16_383,
            16_384,
            1 << 20,
            u32::MAX as u64,
            u64::MAX,
        ] {
            let mut buf = Vec::new();
            put_uvarint(&mut buf, v);
            let (decoded, n) = get_uvarint(&buf).expect("decode");
            assert_eq!(decoded, v, "value {v} round-trip failed");
            assert_eq!(n, buf.len(), "n != buf.len() for v={v}");
        }
    }

    #[test]
    fn varint_rejects_truncated() {
        assert_eq!(get_uvarint(&[]), None);
        assert_eq!(get_uvarint(&[0xff]), None);
        // 10 continuation bytes followed by 0 — overflows (>= 11 bytes).
        let bad = [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00,
        ];
        assert_eq!(get_uvarint(&bad), None);
    }

    #[test]
    fn varint_rejects_64bit_overflow() {
        // Final byte > 1 in 10th position overflows.
        let bad = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02];
        assert_eq!(get_uvarint(&bad), None);
    }
}
