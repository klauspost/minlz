//! Hash helpers used by the L1/L2/L3 encoders.
//!
//! Ported from `encode_l1.go:hash6` and `encode_l2.go:hash4..hash8`.  Each
//! `hashN` takes a 64-bit value whose lowest `N` bytes are the bytes being
//! hashed and returns a value in `[0, 2^h)`.  `h` is expected to be a
//! constant ≤ 32 for `hash4` and ≤ 64 for the rest.

/// 4-byte hash (prime 2654435761).
#[inline(always)]
#[allow(dead_code)] // used by L2/L3 in future stages
pub(super) fn hash4(u: u64, h: u32) -> u32 {
    const PRIME4: u32 = 2_654_435_761;
    ((u as u32).wrapping_mul(PRIME4)) >> ((32 - h) & 31)
}

/// 5-byte hash (prime 889523592379).
#[inline(always)]
pub(super) fn hash5(u: u64, h: u32) -> u32 {
    const PRIME5: u64 = 889_523_592_379;
    (((u << (64 - 40)).wrapping_mul(PRIME5)) >> ((64 - h) & 63)) as u32
}

/// 6-byte hash (prime 227718039650203).
#[inline(always)]
pub(super) fn hash6(u: u64, h: u32) -> u32 {
    const PRIME6: u64 = 227_718_039_650_203;
    (((u << (64 - 48)).wrapping_mul(PRIME6)) >> ((64 - h) & 63)) as u32
}

/// 7-byte hash (prime 58295818150454627).
#[inline(always)]
#[allow(dead_code)] // used by L2/L3 in future stages
pub(super) fn hash7(u: u64, h: u32) -> u32 {
    const PRIME7: u64 = 58_295_818_150_454_627;
    (((u << (64 - 56)).wrapping_mul(PRIME7)) >> ((64 - h) & 63)) as u32
}

/// 8-byte hash (prime 0xcf1bbcdcb7a56463).
#[inline(always)]
#[allow(dead_code)] // used by L2/L3 in future stages
pub(super) fn hash8(u: u64, h: u32) -> u32 {
    const PRIME8: u64 = 0xcf1b_bcdc_b7a5_6463;
    ((u.wrapping_mul(PRIME8)) >> ((64 - h) & 63)) as u32
}
