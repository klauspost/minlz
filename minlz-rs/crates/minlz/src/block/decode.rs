//! Safe MinLZ block decoder.
//!
//! Port of `decode.go:minLZDecodeGo`.  Two-loop structure: the fast loop
//! runs while `s + 16 <= src.len()` and uses raw-pointer loads/stores
//! behind that invariant; the tail loop bounds-checks every byte read.
//!
//! The fast loop also uses LZ4-style 16-byte overshoot writes for short
//! copies — this requires the caller to provide `OVERSHOOT_PAD` bytes of
//! writable padding past `dlen` in the destination buffer.

use super::format::{MIN_COPY2_OFFSET, MIN_COPY3_OFFSET};
use crate::Error;
use core::ptr;

/// Bytes of writable padding past `dlen` that the caller must allocate.
pub(super) const OVERSHOOT_PAD: usize = 16;

/// Decode `src` into the buffer at `dst_ptr`.
///
/// On success the function writes exactly `dlen` valid bytes into
/// `dst_ptr[..dlen]`.  The caller must ensure that the buffer has at
/// least `dlen + OVERSHOOT_PAD` bytes of writable space (the extra padding
/// permits 16-byte overshoot stores in the hot path).
///
/// # Safety
/// - `dst_ptr` must point to at least `dlen + OVERSHOOT_PAD` writable
///   bytes within a single allocation.
/// - The bytes in `dst_ptr[..dlen]` may be uninitialised on entry; on
///   `Ok(())` they are fully written.  The padding region is undefined.
pub(super) unsafe fn minlz_decode(dst_ptr: *mut u8, dlen: usize, src: &[u8]) -> Result<(), Error> {
    let src_len = src.len();
    let src_ptr = src.as_ptr();

    let mut d: usize = 0;
    let mut s: usize = 0;
    let mut offset: usize = 1;

    // -------- Fast loop ----------------------------------------------------
    // Invariant on entry to each iteration: `s + 16 <= src_len`.
    // The tighter `+ 16` (vs Go's `+ 11`) lets us do 16-byte overshoot
    // reads from `src` for the literal fast path.  The last ≤15 bytes of
    // `src` go through the tail loop.
    if src_len >= 16 {
        let fast_limit = src_len - 16;
        while s <= fast_limit {
            // SAFETY: invariant `s + 16 <= src_len` gives us 16 bytes.
            let tag = unsafe { *src_ptr.add(s) };
            let length;
            match tag & 0x03 {
                0 => {
                    // tagLiteral / tagRepeat
                    let v = tag;
                    let x = v >> 3;
                    length = if x < 29 {
                        s += 1;
                        x as usize + 1
                    } else if x == 29 {
                        let n = unsafe { *src_ptr.add(s + 1) } as usize;
                        s += 2;
                        30 + n
                    } else if x == 30 {
                        let n = unsafe { read_u16_le(src_ptr.add(s + 1)) } as usize;
                        s += 3;
                        30 + n
                    } else {
                        let n = unsafe { read_u32_le(src_ptr.add(s)) >> 8 } as usize;
                        s += 4;
                        30 + n
                    };
                    if v & 4 == 0 {
                        // Literal: copy, advance, and skip the docopy below.
                        if length > dlen - d || length > src_len - s {
                            return Err(Error::Corrupt);
                        }
                        // SAFETY: bounds checked above.  We pad dst by
                        // `OVERSHOOT_PAD ≥ 16` bytes, so a 16-byte write
                        // at offset `d ≤ dlen - length` stays in-bounds
                        // because `d + 16 ≤ dlen + 16`.  For src, the
                        // 16-byte overshoot read needs `s + 16 ≤ src_len`,
                        // satisfied while inside the fast loop.
                        unsafe {
                            copy_short_literal(src_ptr.add(s), dst_ptr.add(d), length, s, src_len);
                        }
                        d += length;
                        s += length;
                        continue;
                    }
                    // Repeat (bit 2 set): fall through to docopy with the
                    // length computed above and the previous `offset`.
                }
                1 => {
                    // tagCopy1
                    let lo16 = unsafe { read_u16_le(src_ptr.add(s)) };
                    let len_field = ((lo16 >> 2) & 15) as usize;
                    offset = ((lo16 >> 6) as usize) + 1;
                    if len_field == 15 {
                        length = unsafe { *src_ptr.add(s + 2) } as usize + 18;
                        s += 3;
                    } else {
                        length = len_field + 4;
                        s += 2;
                    }
                }
                2 => {
                    // tagCopy2
                    let raw = (tag >> 2) as usize;
                    offset = unsafe { read_u16_le(src_ptr.add(s + 1)) } as usize;
                    length = if raw <= 60 {
                        s += 3;
                        raw + 4
                    } else if raw == 61 {
                        let n = unsafe { *src_ptr.add(s + 3) } as usize;
                        s += 4;
                        n + 64
                    } else if raw == 62 {
                        let n = unsafe { read_u16_le(src_ptr.add(s + 3)) } as usize;
                        s += 5;
                        n + 64
                    } else {
                        // raw == 63
                        let n = unsafe { read_u32_le(src_ptr.add(s + 2)) >> 8 } as usize;
                        s += 6;
                        n + 64
                    };
                    offset += MIN_COPY2_OFFSET;
                }
                _ => {
                    // 0b11 — Copy2-fused or Copy3
                    let val = unsafe { read_u32_le(src_ptr.add(s)) };
                    let is_copy3 = val & 4 != 0;
                    let mut lit_len = ((val >> 3) & 3) as usize;
                    if !is_copy3 {
                        length = 4 + (((val >> 5) & 7) as usize);
                        offset = ((val >> 8) & 0xffff) as usize + MIN_COPY2_OFFSET;
                        s += 3;
                        lit_len += 1; // fused Copy2 has 1..=4 literals
                    } else {
                        let length_tmp = (val >> 5) & 63;
                        offset = (val >> 11) as usize + MIN_COPY3_OFFSET;
                        length = if length_tmp <= 60 {
                            s += 4;
                            length_tmp as usize + 4
                        } else if length_tmp == 61 {
                            let n = unsafe { *src_ptr.add(s + 4) } as usize;
                            s += 5;
                            n + 64
                        } else if length_tmp == 62 {
                            let n = unsafe { read_u16_le(src_ptr.add(s + 4)) } as usize;
                            s += 6;
                            n + 64
                        } else {
                            let n = unsafe { read_u32_le(src_ptr.add(s + 3)) >> 8 } as usize;
                            s += 7;
                            n + 64
                        };
                    }
                    if lit_len > 0 {
                        if dlen - d < 4 {
                            return Err(Error::Corrupt);
                        }
                        // Fast-path: write 4 bytes; unused trailing bytes
                        // will be overwritten by the next op.
                        unsafe {
                            ptr::write_unaligned(
                                dst_ptr.add(d) as *mut u32,
                                read_u32_le(src_ptr.add(s)),
                            );
                        }
                        s += lit_len;
                        d += lit_len;
                    }
                }
            }
            // docopy: emit `length` bytes from `dst[d-offset..]`.
            if d < offset || length > dlen - d {
                return Err(Error::Corrupt);
            }
            // SAFETY: `d + length ≤ dlen`, `d ≥ offset`, both checked above.
            unsafe {
                if offset > length {
                    // Disjoint ranges — pure memcpy.  Use LZ4-style 16-byte
                    // overshoot when small and the offset gives us a clean
                    // 16-byte read.
                    if length <= 16 && offset >= 16 {
                        let v = ptr::read_unaligned(dst_ptr.add(d - offset) as *const u128);
                        ptr::write_unaligned(dst_ptr.add(d) as *mut u128, v);
                    } else {
                        ptr::copy_nonoverlapping(dst_ptr.add(d - offset), dst_ptr.add(d), length);
                    }
                } else {
                    forward_copy_ptr(dst_ptr, d, offset, length);
                }
            }
            d += length;
        }
    }

    // -------- Tail loop ----------------------------------------------------
    // Bounds-checked path for the last ≤15 source bytes.  We *cannot* form
    // a long-lived `&mut [u8]` over the destination here: the docopy path
    // below writes through `dst_ptr` directly (raw pointer), which under
    // Stacked Borrows invalidates any concurrently-live `&mut [u8]` tag.
    // Mixing the two access kinds is real UB — miri catches it.  So we
    // stick with raw pointers in this loop too; only `src` is borrow-safe.
    while s < src_len {
        let tag = src[s];
        let length;
        match tag & 0x03 {
            0 => {
                let v = tag;
                let x = v >> 3;
                length = if x < 29 {
                    s += 1;
                    x as usize + 1
                } else if x == 29 {
                    s += 2;
                    if s > src_len {
                        return Err(Error::Corrupt);
                    }
                    (src[s - 1] as usize) + 30
                } else if x == 30 {
                    s += 3;
                    if s > src_len {
                        return Err(Error::Corrupt);
                    }
                    ((src[s - 2] as usize) | ((src[s - 1] as usize) << 8)) + 30
                } else {
                    s += 4;
                    if s > src_len {
                        return Err(Error::Corrupt);
                    }
                    ((src[s - 3] as usize)
                        | ((src[s - 2] as usize) << 8)
                        | ((src[s - 1] as usize) << 16))
                        + 30
                };
                if v & 4 == 0 {
                    if length > dlen - d || length > src_len - s {
                        return Err(Error::Corrupt);
                    }
                    // SAFETY: bounds checked above; src and dst are
                    // disjoint allocations.
                    unsafe {
                        ptr::copy_nonoverlapping(src_ptr.add(s), dst_ptr.add(d), length);
                    }
                    d += length;
                    s += length;
                    continue;
                }
                // Repeat: fall through to docopy.
            }
            1 => {
                s += 2;
                if s > src_len {
                    return Err(Error::Corrupt);
                }
                let lo16 = ((src[s - 2] as u16) | ((src[s - 1] as u16) << 8)) as u32;
                let len_field = ((src[s - 2] >> 2) & 15) as usize;
                offset = ((lo16 >> 6) as usize) + 1;
                if len_field == 15 {
                    s += 1;
                    if s > src_len {
                        return Err(Error::Corrupt);
                    }
                    length = (src[s - 1] as usize) + 18;
                } else {
                    length = len_field + 4;
                }
            }
            2 => {
                s += 3;
                if s > src_len {
                    return Err(Error::Corrupt);
                }
                let raw = (src[s - 3] >> 2) as usize;
                offset = (src[s - 2] as usize) | ((src[s - 1] as usize) << 8);
                length = if raw <= 60 {
                    raw + 4
                } else if raw == 61 {
                    s += 1;
                    if s > src_len {
                        return Err(Error::Corrupt);
                    }
                    (src[s - 1] as usize) + 64
                } else if raw == 62 {
                    s += 2;
                    if s > src_len {
                        return Err(Error::Corrupt);
                    }
                    ((src[s - 2] as usize) | ((src[s - 1] as usize) << 8)) + 64
                } else {
                    s += 3;
                    if s > src_len {
                        return Err(Error::Corrupt);
                    }
                    ((src[s - 3] as usize)
                        | ((src[s - 2] as usize) << 8)
                        | ((src[s - 1] as usize) << 16))
                        + 64
                };
                offset += MIN_COPY2_OFFSET;
            }
            _ => {
                s += 4;
                if s > src_len {
                    return Err(Error::Corrupt);
                }
                let val = (src[s - 4] as u32)
                    | ((src[s - 3] as u32) << 8)
                    | ((src[s - 2] as u32) << 16)
                    | ((src[s - 1] as u32) << 24);
                let is_copy3 = val & 4 != 0;
                let mut lit_len = ((val >> 3) & 3) as usize;
                if !is_copy3 {
                    length = 4 + (((val >> 5) & 7) as usize);
                    offset = ((val >> 8) & 0xffff) as usize + MIN_COPY2_OFFSET;
                    s -= 1;
                    lit_len += 1;
                } else {
                    let length_tmp = (val >> 5) & 63;
                    offset = (val >> 11) as usize + MIN_COPY3_OFFSET;
                    length = if length_tmp <= 60 {
                        length_tmp as usize + 4
                    } else if length_tmp == 61 {
                        s += 1;
                        if s > src_len {
                            return Err(Error::Corrupt);
                        }
                        (src[s - 1] as usize) + 64
                    } else if length_tmp == 62 {
                        s += 2;
                        if s > src_len {
                            return Err(Error::Corrupt);
                        }
                        ((src[s - 2] as usize) | ((src[s - 1] as usize) << 8)) + 64
                    } else {
                        s += 3;
                        if s > src_len {
                            return Err(Error::Corrupt);
                        }
                        ((src[s - 3] as usize)
                            | ((src[s - 2] as usize) << 8)
                            | ((src[s - 1] as usize) << 16))
                            + 64
                    };
                }
                if lit_len > 0 {
                    if lit_len > dlen - d || s + lit_len > src_len {
                        return Err(Error::Corrupt);
                    }
                    // SAFETY: bounds checked above.
                    unsafe {
                        ptr::copy_nonoverlapping(src_ptr.add(s), dst_ptr.add(d), lit_len);
                    }
                    d += lit_len;
                    s += lit_len;
                }
            }
        }
        if offset == 0 || d < offset || length > dlen - d {
            return Err(Error::Corrupt);
        }
        unsafe {
            if offset > length {
                ptr::copy_nonoverlapping(dst_ptr.add(d - offset), dst_ptr.add(d), length);
            } else {
                forward_copy_ptr(dst_ptr, d, offset, length);
            }
        }
        d += length;
    }

    if d != dlen {
        return Err(Error::Corrupt);
    }
    Ok(())
}

/// Copy `length` (≤ 16) literal bytes from `src_ptr` to `dst_ptr`.
///
/// Uses a 16-byte overshoot move when the source has 16 readable bytes
/// remaining; otherwise falls back to a length-exact memcpy.  The caller
/// must guarantee 16 writable bytes at `dst_ptr` (which the
/// `OVERSHOOT_PAD = 16` padding contract provides).
#[inline(always)]
unsafe fn copy_short_literal(
    src_ptr: *const u8,
    dst_ptr: *mut u8,
    length: usize,
    s: usize,
    src_len: usize,
) {
    // SAFETY: caller guarantees 16 bytes writable at `dst_ptr`.
    // The fast loop's invariant `s + 16 ≤ src_len` is what gates the
    // overshoot read; we re-check here so a corrupt input's bogus length
    // doesn't read past `src_len`.
    if length <= 16 && s + 16 <= src_len {
        unsafe {
            let v = ptr::read_unaligned(src_ptr as *const u128);
            ptr::write_unaligned(dst_ptr as *mut u128, v);
        }
    } else {
        unsafe {
            ptr::copy_nonoverlapping(src_ptr, dst_ptr, length);
        }
    }
}

/// LZ77 forward copy: `dst[d+i] = dst[d-offset+i]` for `i ∈ [0, length)`.
///
/// Used when `offset <= length`, where the read range overlaps the write
/// range and earlier writes in the same copy may be read.
///
/// # Safety
/// - `d >= offset`
/// - `d + length <= allocated length of `dst_ptr`'s buffer`
#[inline(always)]
unsafe fn forward_copy_ptr(dst_ptr: *mut u8, d: usize, offset: usize, length: usize) {
    debug_assert!(offset > 0);
    debug_assert!(offset <= length);
    let mut i = 0;
    // SAFETY: caller guarantees `d + length` is in-bounds and `d >= offset`.
    while i < length {
        unsafe { *dst_ptr.add(d + i) = *dst_ptr.add(d - offset + i) };
        i += 1;
    }
}

#[inline(always)]
unsafe fn read_u16_le(p: *const u8) -> u16 {
    // SAFETY: caller guarantees 2 bytes readable at `p`.
    unsafe { ptr::read_unaligned(p as *const u16).to_le() }
}

#[inline(always)]
unsafe fn read_u32_le(p: *const u8) -> u32 {
    // SAFETY: caller guarantees 4 bytes readable at `p`.
    unsafe { ptr::read_unaligned(p as *const u32).to_le() }
}
