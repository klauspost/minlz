//! L3 / Smallest encoder.
//!
//! Port of `encode_l3.go:encodeBlockBest`.  Multi-candidate search with
//! encoded-size scoring.  Two hash tables (20-bit long, 18-bit short),
//! each slot storing two candidates (current + previous) packed into a
//! `u64`.  Dictionary support is intentionally omitted per the project
//! rules — there is no `dict` parameter and no dictionary lookup paths.

use std::cell::RefCell;

use super::emit::{
    emit_copy, emit_copy_lits2, emit_copy_lits3, emit_copy_size, emit_literal, emit_literal_size_n,
    emit_repeat, emit_repeat_size, encode_copy2,
};
use super::format::{
    COPY2_LIT_MAX_LEN, MAX_COPY1_OFFSET, MAX_COPY2_LITS, MAX_COPY2_OFFSET, MAX_COPY3_LITS,
    MAX_COPY3_OFFSET, MIN_NON_LITERAL_BLOCK_SIZE,
};
use super::hash::{hash4, hash8};
use super::load_store::{load32, load64};

const INPUT_MARGIN_L3: usize = 8 + 2;
const L_TABLE_BITS: u32 = 20;
const L_TABLE_SIZE: usize = 1 << L_TABLE_BITS;
const S_TABLE_BITS: u32 = 18;
const S_TABLE_SIZE: usize = 1 << S_TABLE_BITS;
const MAX_SKIP: usize = 64;

thread_local! {
    static L_TABLE: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static S_TABLE: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

fn with_tables<R>(f: impl FnOnce(&mut [u64], &mut [u64]) -> R) -> R {
    L_TABLE.with(|lcell| {
        S_TABLE.with(|scell| {
            let mut lt = lcell.borrow_mut();
            let mut st = scell.borrow_mut();
            if lt.len() < L_TABLE_SIZE {
                lt.resize(L_TABLE_SIZE, 0);
            } else {
                lt.iter_mut().for_each(|x| *x = 0);
            }
            if st.len() < S_TABLE_SIZE {
                st.resize(S_TABLE_SIZE, 0);
            } else {
                st.iter_mut().for_each(|x| *x = 0);
            }
            f(&mut lt[..L_TABLE_SIZE], &mut st[..S_TABLE_SIZE])
        })
    })
}

#[inline(always)]
fn get_cur(x: u64) -> usize {
    (x & 0xffff_ffff) as usize
}

#[inline(always)]
fn get_prev(x: u64) -> usize {
    (x >> 32) as usize
}

#[inline(always)]
fn pack_entry(new_cur: usize, prev_entry: u64) -> u64 {
    // New "current" goes in the low 32 bits, the previous "current" gets
    // shifted into the high 32 bits.  Matches Go: `uint64(s) | cand << 32`.
    (new_cur as u64) | (prev_entry << 32)
}

#[derive(Clone, Copy, Default)]
struct Match {
    offset: usize,
    s: usize,
    length: usize,
    score: i64,
    rep: bool,
    nextrep: bool,
}

/// Entry point.
pub(super) fn encode_block(dst: &mut [u8], src: &[u8]) -> usize {
    if src.len() < MIN_NON_LITERAL_BLOCK_SIZE {
        return 0;
    }
    with_tables(|l_table, s_table| inner(dst, src, l_table, s_table))
}

fn inner(dst: &mut [u8], src: &[u8], l_table: &mut [u64], s_table: &mut [u64]) -> usize {
    let s_limit = src.len() - INPUT_MARGIN_L3;
    let dst_limit = src.len() - 5;

    let mut next_emit: usize = 0;
    let mut s: usize = 1;
    let mut repeat: usize = 1;
    // SAFETY: src.len() ≥ MIN_NON_LITERAL_BLOCK_SIZE = 16.
    let mut cv = unsafe { load64(src, s) };
    let mut d: usize = 0;

    'outer: loop {
        let mut best = Match::default();
        loop {
            // Next position to skip to if we find no match.
            let step = (s - next_emit) >> 8;
            let next_s = if step + 1 > MAX_SKIP {
                s + MAX_SKIP
            } else {
                s + step + 1
            };
            if next_s > s_limit {
                break 'outer;
            }

            let hash_l = hash8(cv, L_TABLE_BITS) as usize;
            let hash_s = hash4(cv, S_TABLE_BITS) as usize;
            let cand_l_entry = l_table[hash_l];
            let cand_s_entry = s_table[hash_s];

            // Search at primary candidates.
            if s > 0 {
                best = best_of(
                    best,
                    match_at(
                        src,
                        get_cur(cand_l_entry),
                        s,
                        cv as u32,
                        &best,
                        next_emit,
                        s_limit,
                    ),
                );
                best = best_of(
                    best,
                    match_at(
                        src,
                        get_prev(cand_l_entry),
                        s,
                        cv as u32,
                        &best,
                        next_emit,
                        s_limit,
                    ),
                );
                best = best_of(
                    best,
                    match_at(
                        src,
                        get_cur(cand_s_entry),
                        s,
                        cv as u32,
                        &best,
                        next_emit,
                        s_limit,
                    ),
                );
                best = best_of(
                    best,
                    match_at(
                        src,
                        get_prev(cand_s_entry),
                        s,
                        cv as u32,
                        &best,
                        next_emit,
                        s_limit,
                    ),
                );
            }

            // Repeat-style matches at s and s+1.
            if repeat > 0 && s >= repeat {
                best = best_of(
                    best,
                    match_at_repeat(src, s - repeat, s, cv as u32, &best, next_emit, s_limit),
                );
                if s + 1 >= repeat && s < s_limit {
                    best = best_of(
                        best,
                        match_at_repeat(
                            src,
                            s + 1 - repeat,
                            s + 1,
                            (cv >> 8) as u32,
                            &best,
                            next_emit,
                            s_limit,
                        ),
                    );
                }
            }

            if best.length > 0 {
                // Look at s+1, s+2 candidates.
                let s_fwd1 = s + 1;
                if s_fwd1 + 8 <= src.len() {
                    let hash_s1 = hash4(cv >> 8, S_TABLE_BITS) as usize;
                    let next_short = s_table[hash_s1];
                    // SAFETY: s_fwd1 + 8 ≤ src.len() (checked above).
                    let cv1 = unsafe { load64(src, s_fwd1) };
                    let hash_l1 = hash8(cv1, L_TABLE_BITS) as usize;
                    let next_long = l_table[hash_l1];
                    best = best_of(
                        best,
                        match_at(
                            src,
                            get_cur(next_short),
                            s_fwd1,
                            cv1 as u32,
                            &best,
                            next_emit,
                            s_limit,
                        ),
                    );
                    best = best_of(
                        best,
                        match_at(
                            src,
                            get_prev(next_short),
                            s_fwd1,
                            cv1 as u32,
                            &best,
                            next_emit,
                            s_limit,
                        ),
                    );
                    best = best_of(
                        best,
                        match_at(
                            src,
                            get_cur(next_long),
                            s_fwd1,
                            cv1 as u32,
                            &best,
                            next_emit,
                            s_limit,
                        ),
                    );
                    best = best_of(
                        best,
                        match_at(
                            src,
                            get_prev(next_long),
                            s_fwd1,
                            cv1 as u32,
                            &best,
                            next_emit,
                            s_limit,
                        ),
                    );

                    let s_fwd2 = s + 2;
                    if s_fwd2 + 8 <= src.len() {
                        // SAFETY: s_fwd2 + 8 ≤ src.len().
                        let cv2 = unsafe { load64(src, s_fwd2) };
                        let hash_l2 = hash8(cv2, L_TABLE_BITS) as usize;
                        let next_long2 = l_table[hash_l2];
                        if repeat > 0 && s_fwd2 >= repeat {
                            best = best_of(
                                best,
                                match_at_repeat(
                                    src,
                                    s_fwd2 - repeat,
                                    s_fwd2,
                                    cv2 as u32,
                                    &best,
                                    next_emit,
                                    s_limit,
                                ),
                            );
                        }
                        let hash_s2 = hash4(cv2, S_TABLE_BITS) as usize;
                        let next_short2 = s_table[hash_s2];
                        best = best_of(
                            best,
                            match_at(
                                src,
                                get_cur(next_short2),
                                s_fwd2,
                                cv2 as u32,
                                &best,
                                next_emit,
                                s_limit,
                            ),
                        );
                        best = best_of(
                            best,
                            match_at(
                                src,
                                get_prev(next_short2),
                                s_fwd2,
                                cv2 as u32,
                                &best,
                                next_emit,
                                s_limit,
                            ),
                        );
                        best = best_of(
                            best,
                            match_at(
                                src,
                                get_cur(next_long2),
                                s_fwd2,
                                cv2 as u32,
                                &best,
                                next_emit,
                                s_limit,
                            ),
                        );
                        best = best_of(
                            best,
                            match_at(
                                src,
                                get_prev(next_long2),
                                s_fwd2,
                                cv2 as u32,
                                &best,
                                next_emit,
                                s_limit,
                            ),
                        );
                    }
                }

                // Search for a match starting near the end of the current
                // best match — gives small extra savings on very repetitive
                // inputs.  See Go's "skipBeginning / skipEnd" block.
                const SKIP_BEGIN: usize = 2;
                const SKIP_END: usize = 1;
                let s_at = best.s + best.length - SKIP_END;
                if s_at < s_limit && best.length > SKIP_BEGIN + SKIP_END {
                    let s_back = best.s + SKIP_BEGIN - SKIP_END;
                    let back_l = best.length - SKIP_BEGIN;
                    if s_back + 8 <= src.len() && s_at + 8 <= src.len() {
                        // SAFETY: indices checked above.
                        let cv_back = unsafe { load64(src, s_back) };
                        let cv_at = unsafe { load64(src, s_at) };
                        let h_l = hash8(cv_at, L_TABLE_BITS) as usize;
                        let next = l_table[h_l];
                        if let Some(check_at) = get_cur(next).checked_sub(back_l) {
                            if check_at > 0 {
                                best = best_of(
                                    best,
                                    match_at(
                                        src,
                                        check_at,
                                        s_back,
                                        cv_back as u32,
                                        &best,
                                        next_emit,
                                        s_limit,
                                    ),
                                );
                            }
                        }
                        if let Some(check_at) = get_prev(next).checked_sub(back_l) {
                            if check_at > 0 {
                                best = best_of(
                                    best,
                                    match_at(
                                        src,
                                        check_at,
                                        s_back,
                                        cv_back as u32,
                                        &best,
                                        next_emit,
                                        s_limit,
                                    ),
                                );
                            }
                        }
                        let h_s = hash4(cv_at, S_TABLE_BITS) as usize;
                        let next_s_tbl = s_table[h_s];
                        if let Some(check_at) = get_cur(next_s_tbl).checked_sub(back_l) {
                            if check_at > 0 {
                                best = best_of(
                                    best,
                                    match_at(
                                        src,
                                        check_at,
                                        s_back,
                                        cv_back as u32,
                                        &best,
                                        next_emit,
                                        s_limit,
                                    ),
                                );
                            }
                        }
                        if let Some(check_at) = get_prev(next_s_tbl).checked_sub(back_l) {
                            if check_at > 0 {
                                best = best_of(
                                    best,
                                    match_at(
                                        src,
                                        check_at,
                                        s_back,
                                        cv_back as u32,
                                        &best,
                                        next_emit,
                                        s_limit,
                                    ),
                                );
                            }
                        }
                    }
                }
            }

            // Update tables — keep s as new "current", push previous current
            // to "previous" slot.
            l_table[hash_l] = pack_entry(s, cand_l_entry);
            s_table[hash_s] = pack_entry(s, cand_s_entry);

            if best.length > 0 {
                break;
            }
            // SAFETY: next_s ≤ s_limit ⇒ next_s + INPUT_MARGIN_L3 ≤ src.len().
            cv = unsafe { load64(src, next_s) };
            s = next_s;
        }

        let start_idx = s + 1;
        s = best.s;

        if d + (s - next_emit) > dst_limit {
            return 0;
        }
        let base = s;
        let offset = s - best.offset;
        let mut s_after = s + best.length;

        // Bail if match doesn't justify its encoding cost.
        if !best.rep
            && best.length <= 4
            && (offset > 65535
                || (offset > MAX_COPY1_OFFSET
                    && offset <= MAX_COPY2_OFFSET
                    && base - next_emit > MAX_COPY2_LITS))
        {
            s = start_idx + 1;
            if s >= s_limit {
                break 'outer;
            }
            // SAFETY: s ≤ s_limit ⇒ s + 8 ≤ src.len().
            cv = unsafe { load64(src, s) };
            continue;
        }

        if best.rep {
            d += emit_literal(&mut dst[d..], &src[next_emit..base]);
            d += emit_repeat(&mut dst[d..], best.length);
        } else {
            let lits = &src[next_emit..base];
            if !lits.is_empty() {
                if offset <= MAX_COPY2_OFFSET {
                    let must_split_emit = lits.len() > MAX_COPY2_LITS
                        || offset < 64
                        || (offset <= MAX_COPY1_OFFSET && best.length > COPY2_LIT_MAX_LEN);
                    if must_split_emit {
                        d += emit_literal(&mut dst[d..], lits);
                        if best.length > 18 && best.length <= 64 && offset >= 64 {
                            d += encode_copy2(&mut dst[d..], offset, best.length);
                        } else {
                            d += emit_copy(&mut dst[d..], offset, best.length);
                        }
                    } else if best.length > 11 {
                        // Emit the maximum-fused-Copy2 and let the next
                        // iteration find the residual match.
                        d += emit_copy_lits2(&mut dst[d..], lits, offset, 11);
                        s_after = best.s + 11;
                    } else {
                        d += emit_copy_lits2(&mut dst[d..], lits, offset, best.length);
                    }
                } else if lits.len() > MAX_COPY3_LITS {
                    d += emit_literal(&mut dst[d..], lits);
                    d += emit_copy(&mut dst[d..], offset, best.length);
                } else {
                    d += emit_copy_lits3(&mut dst[d..], lits, offset, best.length);
                }
            } else if best.length > 18
                && best.length <= 64
                && (64..=MAX_COPY2_OFFSET).contains(&offset)
            {
                d += encode_copy2(&mut dst[d..], offset, best.length);
            } else {
                d += emit_copy(&mut dst[d..], offset, best.length);
            }
        }
        repeat = offset;

        s = s_after;
        next_emit = s;
        if s >= s_limit {
            break 'outer;
        }
        if d > dst_limit {
            return 0;
        }

        // Fill tables for the bytes consumed by the match.
        let mut i = start_idx;
        while i < s {
            // SAFETY: i < s ≤ s_limit ⇒ i + 8 ≤ src.len().
            let cv0 = unsafe { load64(src, i) };
            let long0 = hash8(cv0, L_TABLE_BITS) as usize;
            let short0 = hash4(cv0, S_TABLE_BITS) as usize;
            l_table[long0] = pack_entry(i, l_table[long0]);
            s_table[short0] = pack_entry(i, s_table[short0]);
            i += 1;
        }
        // SAFETY: s ≤ s_limit ⇒ s + 8 ≤ src.len().
        cv = unsafe { load64(src, s) };
    }

    // emitRemainder
    if next_emit < src.len() {
        let lit_len = src.len() - next_emit;
        if d + lit_len + emit_literal_size_n(lit_len) > dst_limit {
            return 0;
        }
        d += emit_literal(&mut dst[d..], &src[next_emit..]);
    }
    d
}

#[inline]
fn score(m: &Match, next_emit: usize) -> i64 {
    let ll = (m.s - next_emit) as i64;
    let mut score =
        (m.length as i64) - (emit_literal_size_n(m.s - next_emit) as i64) - (m.s as i64);
    let offset = (m.s as i64) - (m.offset as i64);
    if m.rep {
        return score - (emit_repeat_size(m.length) as i64);
    }
    if ll > 0 && offset > 1024 {
        // Fused-copy bonus: 1-4 lits free for Copy2, 0-3 free for Copy3.
        let fused_copy2 =
            ll <= MAX_COPY2_LITS as i64 && offset < 65536 + 63 && m.length <= COPY2_LIT_MAX_LEN;
        let fused_copy3 = ll <= MAX_COPY3_LITS as i64;
        if fused_copy2 || fused_copy3 {
            score += 1;
        }
    }
    if offset <= 0 {
        return i64::MIN;
    }
    score - (emit_copy_size(offset as usize, m.length) as i64)
}

#[inline]
fn match_at(
    src: &[u8],
    offset: usize,
    s: usize,
    first: u32,
    best: &Match,
    next_emit: usize,
    s_limit: usize,
) -> Match {
    // Filter out non-matches and same-offset retests, matching Go's guards.
    if (best.length != 0 && best.s.wrapping_sub(best.offset) == s.wrapping_sub(offset))
        || s.saturating_sub(offset) >= MAX_COPY3_OFFSET
        || s <= offset
        || offset == 0
        || offset + 4 > src.len()
    {
        return Match {
            offset,
            s,
            ..Match::default()
        };
    }
    // SAFETY: offset + 4 ≤ src.len() (checked).
    if unsafe { load32(src, offset) } != first {
        return Match {
            offset,
            s,
            ..Match::default()
        };
    }

    let mut m = Match {
        offset,
        s,
        length: 4 + offset,
        rep: false,
        ..Match::default()
    };
    let mut sx = s + 4;

    while sx < src.len() {
        if src.len() - sx < 8 {
            if src[sx] == src[m.length] {
                m.length += 1;
                sx += 1;
                continue;
            }
            break;
        }
        // SAFETY: sx + 8 ≤ src.len() (just checked).  m.length is in
        // [offset+4, offset+(s-offset)] ⊆ [0, src.len()) — same condition
        // we just verified for the src side.
        let diff = unsafe { load64(src, sx) ^ load64(src, m.length) };
        if diff != 0 {
            m.length += (diff.trailing_zeros() as usize) >> 3;
            break;
        }
        sx += 8;
        m.length += 8;
    }

    // Extend backwards.
    while m.s > next_emit && m.offset > 0 && src[m.offset - 1] == src[m.s - 1] {
        m.s -= 1;
        m.offset -= 1;
        m.length += 1;
    }
    m.length -= offset;

    m.score = score(&m, next_emit);
    if m.score <= -(m.s as i64) {
        m.length = 0;
    }
    if m.s + m.length < s_limit {
        const CHECKOFF: usize = 1;
        let a = m.s + m.length + CHECKOFF;
        let b = m.offset + m.length + CHECKOFF;
        if a + 4 <= src.len() && b + 4 <= src.len() {
            // SAFETY: both ranges checked.
            m.nextrep = unsafe { load32(src, a) == load32(src, b) };
        }
    }
    m
}

#[inline]
fn match_at_repeat(
    src: &[u8],
    offset: usize,
    s: usize,
    first: u32,
    best: &Match,
    next_emit: usize,
    s_limit: usize,
) -> Match {
    if best.rep || offset == 0 || s <= offset || offset + 3 > src.len() {
        return Match {
            offset,
            s,
            ..Match::default()
        };
    }
    const CHECKBYTES: usize = 3;
    let mask: u32 = (1 << (8 * CHECKBYTES)) - 1;
    // SAFETY: offset + 4 > src.len() is rejected above; here offset + 4 ≤ src.len()
    // is not necessarily true (we only checked + 3).  load32 reads 4 bytes,
    // so guard one more time.
    if offset + 4 > src.len() {
        return Match {
            offset,
            s,
            ..Match::default()
        };
    }
    if (unsafe { load32(src, offset) } & mask) != (first & mask) {
        return Match {
            offset,
            s,
            ..Match::default()
        };
    }

    let mut m = Match {
        offset,
        s,
        length: CHECKBYTES + offset,
        rep: true,
        ..Match::default()
    };
    let mut sx = s + CHECKBYTES;
    while sx < src.len() {
        if src.len() - sx < 8 {
            if src[sx] == src[m.length] {
                m.length += 1;
                sx += 1;
                continue;
            }
            break;
        }
        // SAFETY: sx + 8 ≤ src.len(); m.length + 8 ≤ src.len() (same range).
        let diff = unsafe { load64(src, sx) ^ load64(src, m.length) };
        if diff != 0 {
            m.length += (diff.trailing_zeros() as usize) >> 3;
            break;
        }
        sx += 8;
        m.length += 8;
    }
    while m.s > next_emit && m.offset > 0 && src[m.offset - 1] == src[m.s - 1] {
        m.s -= 1;
        m.offset -= 1;
        m.length += 1;
    }
    m.length -= offset;
    if m.s + m.length < s_limit {
        const CHECKOFF: usize = 1;
        let a = m.s + m.length + CHECKOFF;
        let b = m.offset + m.length + CHECKOFF;
        if a + 4 <= src.len() && b + 4 <= src.len() {
            // SAFETY: both ranges checked.
            m.nextrep = unsafe { load32(src, a) == load32(src, b) };
        }
    }
    m.score = score(&m, next_emit);
    m
}

#[inline]
fn best_of(a: Match, b: Match) -> Match {
    if b.length == 0 {
        return a;
    }
    if a.length == 0 {
        return b;
    }
    if a.score > b.score {
        return a;
    }
    if b.score > a.score {
        return b;
    }
    // Tiebreaker: prefer earlier start, then a repeat candidate, then
    // smaller offset.
    if a.s != b.s {
        return if a.s < b.s { a } else { b };
    }
    if a.nextrep != b.nextrep {
        return if a.nextrep { a } else { b };
    }
    if a.offset > b.offset {
        a
    } else {
        b
    }
}
