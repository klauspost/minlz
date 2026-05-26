//! L2 / Balanced encoder.
//!
//! Ports `encode_l2.go:encodeBlockBetterGo` (for inputs > 64 KiB) and
//! `encode_l2.go:encodeBlockBetterGo64K` (for inputs ≤ 64 KiB).  Uses a
//! long (8-byte hash) table and a short (4-byte hash) table, with the
//! large long table allocated per-thread via [`thread_local!`] to avoid
//! a 512 KiB allocation per encode call (Go's `sync.Pool` analog).

use std::cell::RefCell;

use super::emit::{emit_copy, emit_copy_lits2, emit_copy_lits3, emit_literal, emit_repeat};
use super::format::{
    INPUT_MARGIN, MAX_COPY2_LITS, MAX_COPY2_OFFSET, MAX_COPY3_LITS, MAX_COPY3_OFFSET,
    MIN_NON_LITERAL_BLOCK_SIZE,
};
use super::hash::{hash4, hash6, hash7};
use super::load_store::{load32, load64};

/// Entry point: dispatches on `src.len()`.  Returns the number of bytes
/// written into `dst` or `0` if the block is incompressible.
pub(super) fn encode_block(dst: &mut [u8], src: &[u8]) -> usize {
    if src.len() < MIN_NON_LITERAL_BLOCK_SIZE {
        return 0;
    }
    if src.len() <= 64 << 10 {
        encode_block_64k(dst, src)
    } else {
        encode_block_big(dst, src)
    }
}

const L_TABLE_BITS_BIG: u32 = 17;
const L_TABLE_SIZE_BIG: usize = 1 << L_TABLE_BITS_BIG;
const S_TABLE_BITS_BIG: u32 = 14;
const S_TABLE_SIZE_BIG: usize = 1 << S_TABLE_BITS_BIG;
const L_TABLE_BITS_64K: u32 = 15;
const L_TABLE_SIZE_64K: usize = 1 << L_TABLE_BITS_64K;
const S_TABLE_BITS_64K: u32 = 12;
const S_TABLE_SIZE_64K: usize = 1 << S_TABLE_BITS_64K;

thread_local! {
    static L_TABLE_BIG: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static L_TABLE_64K: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
}

/// Lend the per-thread big long-table to `f`, lazily allocating on first
/// use and zero-clearing on every borrow (mirrors Go's `encLPool` reset).
fn with_l_table_big<R>(f: impl FnOnce(&mut [u32]) -> R) -> R {
    L_TABLE_BIG.with(|cell| {
        let mut t = cell.borrow_mut();
        if t.len() < L_TABLE_SIZE_BIG {
            t.resize(L_TABLE_SIZE_BIG, 0);
        } else {
            t.iter_mut().for_each(|x| *x = 0);
        }
        f(&mut t[..L_TABLE_SIZE_BIG])
    })
}

fn with_l_table_64k<R>(f: impl FnOnce(&mut [u16]) -> R) -> R {
    L_TABLE_64K.with(|cell| {
        let mut t = cell.borrow_mut();
        if t.len() < L_TABLE_SIZE_64K {
            t.resize(L_TABLE_SIZE_64K, 0);
        } else {
            t.iter_mut().for_each(|x| *x = 0);
        }
        f(&mut t[..L_TABLE_SIZE_64K])
    })
}

/// L2 encoder for inputs > 64 KiB.  Port of `encodeBlockBetterGo`.
fn encode_block_big(dst: &mut [u8], src: &[u8]) -> usize {
    let s_limit = src.len() - INPUT_MARGIN;
    let dst_limit = src.len() - (src.len() >> 5) - 6;

    with_l_table_big(|l_table| {
        let mut s_table = [0u32; S_TABLE_SIZE_BIG];
        encode_better_inner_big(dst, src, s_limit, dst_limit, l_table, &mut s_table)
    })
}

fn encode_better_inner_big(
    dst: &mut [u8],
    src: &[u8],
    s_limit: usize,
    dst_limit: usize,
    l_table: &mut [u32],
    s_table: &mut [u32; S_TABLE_SIZE_BIG],
) -> usize {
    const S_TABLE_BITS: u32 = S_TABLE_BITS_BIG;
    let mut next_emit: usize = 0;
    let mut s: usize = 1;
    // SAFETY: src.len() ≥ MIN_NON_LITERAL_BLOCK_SIZE = 16 ⇒ s + 8 ≤ src.len().
    let mut cv = unsafe { load64(src, s) };
    let mut repeat: usize = 1;
    let mut d: usize = 0;

    'outer: loop {
        let candidate_l;
        let mut next_s: usize;
        loop {
            next_s = s + ((s - next_emit) >> 7) + 1;
            if next_s > s_limit {
                break 'outer;
            }
            let min_src_pos = (s as isize) - (MAX_COPY3_OFFSET as isize) + 1;
            let hash_l = hash7(cv, L_TABLE_BITS_BIG) as usize;
            let hash_s = hash4(cv, S_TABLE_BITS) as usize;
            let cand_l = l_table[hash_l] as usize;
            let cand_s = s_table[hash_s] as usize;
            l_table[hash_l] = s as u32;
            s_table[hash_s] = s as u32;

            // SAFETY: cand_l/cand_s are previously-recorded s values (< current s)
            // or 0; in either case + 8 ≤ s + 8 ≤ s_limit + INPUT_MARGIN = src.len().
            let val_long = unsafe { load64(src, cand_l) };
            let val_short = unsafe { load64(src, cand_s) };

            // 8-byte long match — best case.
            if (cand_l as isize) > min_src_pos && cv == val_long {
                candidate_l = cand_l;
                break;
            }

            // Repeat check at offset +1: matches bytes [s+1..s+5] vs
            // [s-repeat+1..s-repeat+5].
            let repeat_mask: u64 = 0x0000_00ff_ffff_ff00;
            if repeat > 0 && s >= repeat {
                // SAFETY: s + 8 ≤ src.len() (s ≤ s_limit); s - repeat is valid since s ≥ repeat.
                let prev_v = unsafe { load64(src, s - repeat) };
                if cv & repeat_mask == prev_v & repeat_mask {
                    let mut base = s + 1;
                    let mut i = base - repeat;
                    while base > next_emit && i > 0 && src[i - 1] == src[base - 1] {
                        i -= 1;
                        base -= 1;
                    }
                    if d + (base - next_emit) > dst_limit {
                        return 0;
                    }
                    d += emit_literal(&mut dst[d..], &src[next_emit..base]);

                    let mut cand = s - repeat + 4 + 1;
                    s += 4 + 1;
                    while s < src.len() {
                        if src.len() - s < 8 {
                            if src[s] == src[cand] {
                                s += 1;
                                cand += 1;
                                continue;
                            }
                            break;
                        }
                        // SAFETY: s + 8 ≤ src.len(), cand + 8 ≤ src.len() (cand < s).
                        let diff = unsafe { load64(src, s) ^ load64(src, cand) };
                        if diff != 0 {
                            s += (diff.trailing_zeros() as usize) >> 3;
                            break;
                        }
                        s += 8;
                        cand += 8;
                    }
                    d += emit_repeat(&mut dst[d..], s - base);
                    next_emit = s;
                    if s >= s_limit {
                        break 'outer;
                    }
                    // Index in-between positions.
                    let mut idx0 = base + 1;
                    let mut idx1 = s.saturating_sub(2);
                    while idx0 < idx1 {
                        // SAFETY: idx0 < idx1 < s ≤ src.len() - 8; idx0 + 8 ≤ src.len().
                        let cv0 = unsafe { load64(src, idx0) };
                        let cv1 = unsafe { load64(src, idx1) };
                        l_table[hash7(cv0, L_TABLE_BITS_BIG) as usize] = idx0 as u32;
                        s_table[hash4(cv0 >> 8, S_TABLE_BITS) as usize] = (idx0 + 1) as u32;
                        l_table[hash7(cv1, L_TABLE_BITS_BIG) as usize] = idx1 as u32;
                        s_table[hash4(cv1 >> 8, S_TABLE_BITS) as usize] = (idx1 + 1) as u32;
                        idx0 += 2;
                        idx1 = idx1.saturating_sub(2);
                    }
                    // SAFETY: s ≤ s_limit ⇒ s + 8 ≤ src.len().
                    cv = unsafe { load64(src, s) };
                    continue;
                }
            }

            // 4-byte long match.
            if (cand_l as isize) >= min_src_pos && (cv as u32) == (val_long as u32) {
                candidate_l = cand_l;
                break;
            }

            // 4-byte short match, then try long at s+1.
            if (cand_s as isize) >= min_src_pos && (cv as u32) == (val_short as u32) {
                let hash_l2 = hash7(cv >> 8, L_TABLE_BITS_BIG) as usize;
                let cand_l2 = l_table[hash_l2] as usize;
                l_table[hash_l2] = (s + 1) as u32;
                // SAFETY: cand_l2 was a recorded s-value < current s; + 4 ≤ src.len().
                if (cand_l2 as isize) > min_src_pos
                    && ((cv >> 8) as u32) == unsafe { load32(src, cand_l2) }
                {
                    candidate_l = cand_l2;
                    s += 1;
                    break;
                }
                candidate_l = cand_s;
                break;
            }

            // SAFETY: next_s ≤ s_limit ⇒ next_s + 8 ≤ src.len().
            cv = unsafe { load64(src, next_s) };
            s = next_s;
        }

        // Extend backwards.
        let mut candidate_pos = candidate_l;
        while candidate_pos > 0 && s > next_emit && src[candidate_pos - 1] == src[s - 1] {
            candidate_pos -= 1;
            s -= 1;
        }

        if d + (s - next_emit) > dst_limit {
            return 0;
        }

        let base = s;
        let offset = base - candidate_pos;

        // Extend forwards.
        let mut cand = candidate_pos + 4;
        s += 4;
        while s < src.len() {
            if src.len() - s < 8 {
                if src[s] == src[cand] {
                    s += 1;
                    cand += 1;
                    continue;
                }
                break;
            }
            // SAFETY: s + 8 ≤ src.len(), cand + 8 ≤ src.len() (cand < s).
            let diff = unsafe { load64(src, s) ^ load64(src, cand) };
            if diff != 0 {
                s += (diff.trailing_zeros() as usize) >> 3;
                break;
            }
            s += 8;
            cand += 8;
        }

        // Bail if 3-byte-offset Copy3 with length ≤ 4 — won't compress.
        if offset > 65535 && s - base <= 4 && repeat != offset {
            s = next_s + 1;
            if s >= s_limit {
                break 'outer;
            }
            // SAFETY: s ≤ s_limit ⇒ s + 8 ≤ src.len().
            cv = unsafe { load64(src, s) };
            continue;
        }

        let lits = &src[next_emit..base];
        let match_len = s - base;
        if !lits.is_empty() {
            if offset <= MAX_COPY2_OFFSET {
                if lits.len() > MAX_COPY2_LITS || offset < 64 {
                    d += emit_literal(&mut dst[d..], lits);
                    d += emit_copy(&mut dst[d..], offset, match_len);
                } else {
                    d += emit_copy_lits2(&mut dst[d..], lits, offset, match_len);
                }
            } else if lits.len() > MAX_COPY3_LITS {
                d += emit_literal(&mut dst[d..], lits);
                d += emit_copy(&mut dst[d..], offset, match_len);
            } else {
                d += emit_copy_lits3(&mut dst[d..], lits, offset, match_len);
            }
        } else {
            d += emit_copy(&mut dst[d..], offset, match_len);
        }
        repeat = offset;

        next_emit = s;
        if s >= s_limit {
            break 'outer;
        }
        if d > dst_limit {
            return 0;
        }

        // Index short & long for the bytes we just consumed.
        let mut idx0 = base + 1;
        let mut idx1 = s - 2;
        // SAFETY: idx0 + 8 ≤ s + 8 ≤ src.len(); same for idx1.
        let cv0 = unsafe { load64(src, idx0) };
        let cv1 = unsafe { load64(src, idx1) };
        l_table[hash7(cv0, L_TABLE_BITS_BIG) as usize] = idx0 as u32;
        s_table[hash4(cv0 >> 8, S_TABLE_BITS) as usize] = (idx0 + 1) as u32;
        l_table[hash7(cv1, L_TABLE_BITS_BIG) as usize] = idx1 as u32;
        s_table[hash4(cv1 >> 8, S_TABLE_BITS) as usize] = (idx1 + 1) as u32;
        idx0 += 1;
        idx1 = idx1.saturating_sub(1);

        // SAFETY: s ≤ s_limit ⇒ s + 8 ≤ src.len().
        cv = unsafe { load64(src, s) };

        // Sparse long-only indexing for the middle of the range.
        let mut idx2 = (idx0 + idx1 + 1) >> 1;
        while idx2 < idx1 {
            // SAFETY: idx0/idx2 < idx1 < s ≤ src.len() - 8.
            l_table[hash7(unsafe { load64(src, idx0) }, L_TABLE_BITS_BIG) as usize] = idx0 as u32;
            l_table[hash7(unsafe { load64(src, idx2) }, L_TABLE_BITS_BIG) as usize] = idx2 as u32;
            idx0 += 2;
            idx2 += 2;
        }
    }

    if next_emit < src.len() {
        if d + src.len() - next_emit > dst_limit {
            return 0;
        }
        d += emit_literal(&mut dst[d..], &src[next_emit..]);
    }
    d
}

/// L2 encoder for inputs ≤ 64 KiB.  Port of `encodeBlockBetterGo64K`.
fn encode_block_64k(dst: &mut [u8], src: &[u8]) -> usize {
    let s_limit = src.len() - INPUT_MARGIN;
    let dst_limit = src.len() - (src.len() >> 5) - 6;

    with_l_table_64k(|l_table| {
        let mut s_table = [0u16; S_TABLE_SIZE_64K];
        encode_better_inner_64k(dst, src, s_limit, dst_limit, l_table, &mut s_table)
    })
}

fn encode_better_inner_64k(
    dst: &mut [u8],
    src: &[u8],
    s_limit: usize,
    dst_limit: usize,
    l_table: &mut [u16],
    s_table: &mut [u16; S_TABLE_SIZE_64K],
) -> usize {
    const S_TABLE_BITS: u32 = S_TABLE_BITS_64K;
    let mut next_emit: usize = 0;
    let mut s: usize = 1;
    // SAFETY: src.len() ≥ MIN_NON_LITERAL_BLOCK_SIZE = 16.
    let mut cv = unsafe { load64(src, s) };
    let mut repeat: usize = 1;
    let mut d: usize = 0;

    'outer: loop {
        let candidate_l;
        let mut next_s: usize;
        loop {
            next_s = s + ((s - next_emit) >> 7) + 1;
            if next_s > s_limit {
                break 'outer;
            }
            let hash_l = hash6(cv, L_TABLE_BITS_64K) as usize;
            let hash_s = hash4(cv, S_TABLE_BITS) as usize;
            let cand_l = l_table[hash_l] as usize;
            let cand_s = s_table[hash_s] as usize;
            l_table[hash_l] = s as u16;
            s_table[hash_s] = s as u16;

            // SAFETY: cand_l / cand_s are previously-recorded s-values (< current s)
            // or 0; cand + 8 ≤ s + 8 ≤ src.len().
            let val_long = unsafe { load64(src, cand_l) };
            let val_short = unsafe { load64(src, cand_s) };

            if cv == val_long {
                candidate_l = cand_l;
                break;
            }

            // Repeat check at +1.
            let repeat_mask: u64 = 0x0000_00ff_ffff_ff00;
            if repeat > 0 && s >= repeat {
                // SAFETY: s ≤ s_limit ⇒ s + 8 ≤ src.len().
                let prev_v = unsafe { load64(src, s - repeat) };
                if cv & repeat_mask == prev_v & repeat_mask {
                    let mut base = s + 1;
                    let mut i = base - repeat;
                    while base > next_emit && i > 0 && src[i - 1] == src[base - 1] {
                        i -= 1;
                        base -= 1;
                    }
                    if d + (base - next_emit) > dst_limit {
                        return 0;
                    }
                    d += emit_literal(&mut dst[d..], &src[next_emit..base]);

                    let mut cand = s - repeat + 4 + 1;
                    s += 4 + 1;
                    while s < src.len() {
                        if src.len() - s < 8 {
                            if src[s] == src[cand] {
                                s += 1;
                                cand += 1;
                                continue;
                            }
                            break;
                        }
                        let diff = unsafe { load64(src, s) ^ load64(src, cand) };
                        if diff != 0 {
                            s += (diff.trailing_zeros() as usize) >> 3;
                            break;
                        }
                        s += 8;
                        cand += 8;
                    }
                    d += emit_repeat(&mut dst[d..], s - base);
                    next_emit = s;
                    if s >= s_limit {
                        break 'outer;
                    }
                    let mut idx0 = base + 1;
                    let mut idx1 = s.saturating_sub(2);
                    while idx0 < idx1 {
                        let cv0 = unsafe { load64(src, idx0) };
                        let cv1 = unsafe { load64(src, idx1) };
                        l_table[hash6(cv0, L_TABLE_BITS_64K) as usize] = idx0 as u16;
                        s_table[hash4(cv0 >> 8, S_TABLE_BITS) as usize] = (idx0 + 1) as u16;
                        l_table[hash6(cv1, L_TABLE_BITS_64K) as usize] = idx1 as u16;
                        s_table[hash4(cv1 >> 8, S_TABLE_BITS) as usize] = (idx1 + 1) as u16;
                        idx0 += 2;
                        idx1 = idx1.saturating_sub(2);
                    }
                    cv = unsafe { load64(src, s) };
                    continue;
                }
            }

            if (cv as u32) == (val_long as u32) {
                candidate_l = cand_l;
                break;
            }

            if (cv as u32) == (val_short as u32) {
                let hash_l2 = hash6(cv >> 8, L_TABLE_BITS_64K) as usize;
                let cand_l2 = l_table[hash_l2] as usize;
                l_table[hash_l2] = (s + 1) as u16;
                if ((cv >> 8) as u32) == unsafe { load32(src, cand_l2) } {
                    candidate_l = cand_l2;
                    s += 1;
                    break;
                }
                candidate_l = cand_s;
                break;
            }

            cv = unsafe { load64(src, next_s) };
            s = next_s;
        }

        let mut candidate_pos = candidate_l;
        while candidate_pos > 0 && s > next_emit && src[candidate_pos - 1] == src[s - 1] {
            candidate_pos -= 1;
            s -= 1;
        }
        if d + (s - next_emit) > dst_limit {
            return 0;
        }
        let base = s;
        let offset = base - candidate_pos;
        let mut cand = candidate_pos + 4;
        s += 4;
        while s < src.len() {
            if src.len() - s < 8 {
                if src[s] == src[cand] {
                    s += 1;
                    cand += 1;
                    continue;
                }
                break;
            }
            let diff = unsafe { load64(src, s) ^ load64(src, cand) };
            if diff != 0 {
                s += (diff.trailing_zeros() as usize) >> 3;
                break;
            }
            s += 8;
            cand += 8;
        }

        let lits = &src[next_emit..base];
        let match_len = s - base;
        if !lits.is_empty() {
            if lits.len() > MAX_COPY2_LITS || offset < 64 {
                d += emit_literal(&mut dst[d..], lits);
                d += emit_copy(&mut dst[d..], offset, match_len);
            } else {
                d += emit_copy_lits2(&mut dst[d..], lits, offset, match_len);
            }
        } else {
            d += emit_copy(&mut dst[d..], offset, match_len);
        }
        repeat = offset;
        next_emit = s;
        if s >= s_limit {
            break 'outer;
        }
        if d > dst_limit {
            return 0;
        }

        let mut idx0 = base + 1;
        let mut idx1 = s - 2;
        let cv0 = unsafe { load64(src, idx0) };
        let cv1 = unsafe { load64(src, idx1) };
        l_table[hash6(cv0, L_TABLE_BITS_64K) as usize] = idx0 as u16;
        s_table[hash4(cv0 >> 8, S_TABLE_BITS) as usize] = (idx0 + 1) as u16;
        l_table[hash6(cv1, L_TABLE_BITS_64K) as usize] = idx1 as u16;
        s_table[hash4(cv1 >> 8, S_TABLE_BITS) as usize] = (idx1 + 1) as u16;
        idx0 += 1;
        idx1 = idx1.saturating_sub(1);

        cv = unsafe { load64(src, s) };

        let mut idx2 = (idx0 + idx1 + 1) >> 1;
        while idx2 < idx1 {
            l_table[hash6(unsafe { load64(src, idx0) }, L_TABLE_BITS_64K) as usize] = idx0 as u16;
            l_table[hash6(unsafe { load64(src, idx2) }, L_TABLE_BITS_64K) as usize] = idx2 as u16;
            idx0 += 2;
            idx2 += 2;
        }
    }

    if next_emit < src.len() {
        if d + src.len() - next_emit > dst_limit {
            return 0;
        }
        d += emit_literal(&mut dst[d..], &src[next_emit..]);
    }
    d
}
