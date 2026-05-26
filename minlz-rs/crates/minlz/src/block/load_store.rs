//! Unaligned little-endian loads/stores.
//!
//! Direct ports of `unsafe_enabled.go`'s `loadN` / `storeN`.  On supported
//! little-endian targets these compile to a single unaligned `MOVQ` /
//! `LDR` (no byte-swap).  On big-endian targets `to_le()` / `from_le_bytes`
//! insert a `BSWAP`, which is correct but slower (BE is supported but not
//! perf-tuned per the project rules).

/// Load a `u16` at offset `i` in `b` with no bounds check.
///
/// # Safety
/// `i + 2 <= b.len()` must hold.
#[inline(always)]
#[allow(dead_code)] // used by L2/L3 in future stages
pub(super) unsafe fn load16(b: &[u8], i: usize) -> u16 {
    // SAFETY: caller guarantees `i + 2 <= b.len()`; `read_unaligned` does
    // not require alignment, and `b.as_ptr().add(i)` stays within the slice.
    unsafe { core::ptr::read_unaligned(b.as_ptr().add(i) as *const u16).to_le() }
}

/// Load a `u32` at offset `i` in `b` with no bounds check.
///
/// # Safety
/// `i + 4 <= b.len()` must hold.
#[inline(always)]
#[allow(dead_code)] // used by L2/L3 in future stages
pub(super) unsafe fn load32(b: &[u8], i: usize) -> u32 {
    // SAFETY: see `load16`.
    unsafe { core::ptr::read_unaligned(b.as_ptr().add(i) as *const u32).to_le() }
}

/// Load a `u64` at offset `i` in `b` with no bounds check.
///
/// # Safety
/// `i + 8 <= b.len()` must hold.
#[inline(always)]
pub(super) unsafe fn load64(b: &[u8], i: usize) -> u64 {
    // SAFETY: see `load16`.
    unsafe { core::ptr::read_unaligned(b.as_ptr().add(i) as *const u64).to_le() }
}

/// Store `v` at offset `i` in `b` with no bounds check.
///
/// # Safety
/// `i < b.len()` must hold.
#[inline(always)]
pub(super) unsafe fn store8(b: &mut [u8], i: usize, v: u8) {
    // SAFETY: caller guarantees `i < b.len()`.
    unsafe { *b.as_mut_ptr().add(i) = v };
}

/// Store a `u16` (LE) at offset `i` in `b` with no bounds check.
///
/// # Safety
/// `i + 2 <= b.len()` must hold.
#[inline(always)]
pub(super) unsafe fn store16(b: &mut [u8], i: usize, v: u16) {
    // SAFETY: see `store8`.
    unsafe {
        core::ptr::write_unaligned(b.as_mut_ptr().add(i) as *mut u16, v.to_le());
    }
}

/// Store a `u32` (LE) at offset `i` in `b` with no bounds check.
///
/// # Safety
/// `i + 4 <= b.len()` must hold.
#[inline(always)]
pub(super) unsafe fn store32(b: &mut [u8], i: usize, v: u32) {
    // SAFETY: see `store8`.
    unsafe {
        core::ptr::write_unaligned(b.as_mut_ptr().add(i) as *mut u32, v.to_le());
    }
}
