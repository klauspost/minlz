//! Level 3 (best) encoding implementation
//!
//! Direct port of Go's encodeBlockBest algorithm for maximum compression.
//! Uses sophisticated match finding with scoring system to find optimal matches.

use crate::{
    constants::*,
    error::Result,
    memory::{load64, load32},
    encode::{
        emit::{emit_literal, emit_repeat, emit_copy, emit_copy_lits2, emit_copy_lits3},
        hash::{hash4, hash8},
    },
};

/// Level 3 encoder - best compression using sophisticated match finding
pub fn encode_block(dst: &mut [u8], src: &[u8]) -> Result<usize> {
    if src.len() < MIN_NON_LITERAL_BLOCK_SIZE {
        return Ok(0);
    }

    encode_block_best(dst, src)
}

// Match structure
#[derive(Debug, Clone, Copy)]
struct Match {
    offset: usize,
    s: usize,
    length: usize,
    score: i32,
    rep: bool,
    nextrep: bool,
}

impl Default for Match {
    fn default() -> Self {
        Match {
            offset: 0,
            s: 0,
            length: 0,
            score: 0,
            rep: false,
            nextrep: false,
        }
    }
}

// Helper functions - exact ports of Go's size calculation functions
fn emit_literal_size_n(n: usize) -> usize {
    if n == 0 {
        0
    } else if n <= 29 {
        1
    } else if n < 29 + 256 {
        2
    } else if n < 29 + 65536 {
        3
    } else {
        4
    }
}

fn emit_repeat_size(length: usize) -> usize {
    if length <= 0 {
        0
    } else if length <= 29 {
        1
    } else {
        let length = length - 29;
        if length <= 256 {
            2
        } else if length <= 65536 {
            3
        } else {
            4
        }
    }
}

fn emit_copy2_size(length: usize) -> usize {
    let mut length = length - 4;

    if length <= 60 {
        // Length inside tag
        3
    } else {
        length -= 60;
        if length < 256 {
            // Length in 1 byte
            4
        } else if length < 65536 {
            // Length in 2 bytes
            5
        } else {
            // Length in 3 bytes
            6
        }
    }
}

fn emit_copy_size(offset: usize, length: usize) -> usize {
    if offset > 65536 + 63 {
        // 3 Byte offset + Variable length (base length 4)
        if length <= 64 {
            // Base length is included for free
            4
        } else {
            let extra_length = length - 64; // Extra length beyond the base
            4 + ((extra_length.ilog2() as usize + 7) / 8)
        }
    } else if offset <= 1024 {
        // Offset no more than 2 bytes
        if length <= 18 {
            // Emit up to 18 bytes with short offset
            2
        } else if length < 18 + 256 {
            3
        } else {
            // Worst case we have to emit a repeat for the rest
            2 + emit_repeat_size(length - 18)
        }
    } else {
        // 2 byte offset + Variable length (base length 4)
        emit_copy2_size(length)
    }
}

// Scoring function - port of Go's score function
fn score_match(m: &Match, next_emit: usize) -> i32 {
    let ll = m.s - next_emit;
    let mut score = m.length as i32 - emit_literal_size_n(ll) as i32 - m.s as i32;
    let offset = m.s - m.offset;

    if m.rep {
        return score - emit_repeat_size(m.length) as i32;
    }

    if ll > 0 && offset > 1024 {
        // Check for fused discount
        if ll <= MAX_COPY2_LITS && offset < 65536 + 63 && m.length <= 18 {
            score += 1;
        } else if ll <= MAX_COPY3_LITS {
            score += 1;
        }
    }

    score - emit_copy_size(offset, m.length) as i32
}

// Match finding function - port of Go's matchAt
fn match_at(src: &[u8], offset: usize, s: usize, first: u32, best: &Match, next_emit: usize, s_limit: usize) -> Match {
    if (best.length != 0 && best.s - best.offset == s - offset) ||
       s - offset >= MAX_COPY3_OFFSET || s <= offset {
        return Match { offset, s, ..Default::default() };
    }

    if load32(src, offset).unwrap_or(0) != first {
        return Match { offset, s, ..Default::default() };
    }

    let mut m = Match {
        offset,
        s,
        length: 4 + offset,
        rep: false,
        ..Default::default()
    };
    let mut s_pos = s + 4;

    // Forward extension
    while s_pos < src.len() {
        if src.len() - s_pos < 8 {
            if src[s_pos] == src[m.length] {
                m.length += 1;
                s_pos += 1;
                continue;
            }
            break;
        }

        if let (Ok(a), Ok(b)) = (load64(src, s_pos), load64(src, m.length)) {
            let diff = a ^ b;
            if diff != 0 {
                m.length += (diff.trailing_zeros() >> 3) as usize;
                break;
            }
            s_pos += 8;
            m.length += 8;
        } else {
            break;
        }
    }

    // Backward extension
    while m.s > next_emit && m.offset > 0 {
        if src[m.offset - 1] != src[m.s - 1] {
            break;
        }
        m.s -= 1;
        m.offset -= 1;
        m.length += 1;
    }

    m.length -= offset;
    m.score = score_match(&m, next_emit);

    if m.score <= -(m.s as i32) {
        m.length = 0;  // Eliminate if no savings
    }

    if m.s + m.length < s_limit {
        let a = m.s + m.length + 1;
        let b = m.offset + m.length + 1;
        if a < src.len() && b < src.len() {
            m.nextrep = load32(src, a).unwrap_or(0) == load32(src, b).unwrap_or(0);
        }
    }

    m
}

// Repeat match finding - port of Go's matchAtRepeat
fn match_at_repeat(src: &[u8], offset: usize, s: usize, first: u32, best: &Match, next_emit: usize, s_limit: usize) -> Match {
    if best.rep {
        return Match { offset, s, ..Default::default() };
    }

    const CHECK_BYTES: usize = 3;
    let mask = (1u32 << (8 * CHECK_BYTES)) - 1;
    if (load32(src, offset).unwrap_or(0) & mask) != (first & mask) {
        return Match { offset, s, ..Default::default() };
    }

    let mut m = Match {
        offset,
        s,
        length: CHECK_BYTES + offset,
        rep: true,
        ..Default::default()
    };
    let mut s_pos = s + CHECK_BYTES;

    // Forward extension
    while s_pos < src.len() {
        if src.len() - s_pos < 8 {
            if src[s_pos] == src[m.length] {
                m.length += 1;
                s_pos += 1;
                continue;
            }
            break;
        }

        if let (Ok(a), Ok(b)) = (load64(src, s_pos), load64(src, m.length)) {
            let diff = a ^ b;
            if diff != 0 {
                m.length += (diff.trailing_zeros() >> 3) as usize;
                break;
            }
            s_pos += 8;
            m.length += 8;
        } else {
            break;
        }
    }

    // Backward extension
    while m.s > next_emit && m.offset > 0 {
        if src[m.offset - 1] != src[m.s - 1] {
            break;
        }
        m.s -= 1;
        m.offset -= 1;
        m.length += 1;
    }

    m.length -= offset;

    if m.s + m.length < s_limit {
        let a = m.s + m.length + 1;
        let b = m.offset + m.length + 1;
        if a < src.len() && b < src.len() {
            m.nextrep = load32(src, a).unwrap_or(0) == load32(src, b).unwrap_or(0);
        }
    }

    m.score = score_match(&m, next_emit);
    m
}

// Best match selection - port of Go's bestOf
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

    // Pick whichever starts the earliest
    if a.s != b.s {
        if a.s < b.s {
            return a;
        }
        return b;
    }

    // If one is a good repeat candidate, pick it
    if a.nextrep != b.nextrep {
        if a.nextrep {
            return a;
        }
        return b;
    }

    // Pick the smallest distance offset
    if a.offset > b.offset {
        return a;
    }
    b
}

/// Main Level 3 encoding function - port of Go's encodeBlockBest
fn encode_block_best(dst: &mut [u8], src: &[u8]) -> Result<usize> {
    // Hash table configuration - matches Go's encodeBlockBest
    const L_TABLE_BITS: u8 = 20;  // Long hash matches
    const MAX_L_TABLE_SIZE: usize = 1 << L_TABLE_BITS;
    const S_TABLE_BITS: u8 = 18;  // Short hash matches
    const MAX_S_TABLE_SIZE: usize = 1 << S_TABLE_BITS;
    const INPUT_MARGIN: usize = 8 + 2;
    const MAX_SKIP: usize = 64;

    let s_limit = src.len().saturating_sub(INPUT_MARGIN);
    let dst_limit = src.len().saturating_sub(5); // Bail if we can't compress to at least this

    let mut l_table = vec![0u64; MAX_L_TABLE_SIZE];
    let mut s_table = vec![0u64; MAX_S_TABLE_SIZE];

    let mut d = 0;
    let mut next_emit = 0;
    let mut s = 1;  // Start looking for matches at s == 1
    let mut repeat = 1;
    let mut cv = load64(src, s)?;

    // Helper functions from Go implementation
    const LOWBIT_MASK: u64 = 0xffffffff;
    let get_cur = |x: u64| -> usize { (x & LOWBIT_MASK) as usize };
    let get_prev = |x: u64| -> usize { (x >> 32) as usize };

    'outer: loop {
        let mut best = Match::default();

        // Find best match - main search loop
        loop {
            let next_s = {
                let skip = (s - next_emit) >> 8;
                let next = if skip + 1 > MAX_SKIP {
                    s + MAX_SKIP
                } else {
                    s + skip + 1
                };
                next
            };

            if next_s > s_limit {
                break 'outer;
            }

            let hash_l = hash8(cv, L_TABLE_BITS) as usize;
            let hash_s = hash4(cv, S_TABLE_BITS) as usize;
            let candidate_l = l_table[hash_l];
            let candidate_s = s_table[hash_s];

            // Test candidates at current position
            if s > 0 {
                best = best_of(best, match_at(src, get_cur(candidate_l), s, cv as u32, &best, next_emit, s_limit));
                best = best_of(best, match_at(src, get_prev(candidate_l), s, cv as u32, &best, next_emit, s_limit));
                best = best_of(best, match_at(src, get_cur(candidate_s), s, cv as u32, &best, next_emit, s_limit));
                best = best_of(best, match_at(src, get_prev(candidate_s), s, cv as u32, &best, next_emit, s_limit));
            }

            // Test repeat matches
            if repeat <= s {
                best = best_of(best, match_at_repeat(src, s - repeat, s, cv as u32, &best, next_emit, s_limit));
                best = best_of(best, match_at_repeat(src, s - repeat + 1, s + 1, (cv >> 8) as u32, &best, next_emit, s_limit));
            }

            if best.length > 0 {
                // Look ahead for better matches (Go's lookahead logic)
                let s_fwd = s + 1;
                if s_fwd < src.len() - 8 {
                    let cv_fwd = load64(src, s_fwd)?;
                    let hash_s_fwd = hash4(cv >> 8, S_TABLE_BITS) as usize;
                    let hash_l_fwd = hash8(cv_fwd, L_TABLE_BITS) as usize;
                    let next_short = s_table[hash_s_fwd];
                    let next_long = l_table[hash_l_fwd];

                    best = best_of(best, match_at(src, get_cur(next_short), s_fwd, cv_fwd as u32, &best, next_emit, s_limit));
                    best = best_of(best, match_at(src, get_prev(next_short), s_fwd, cv_fwd as u32, &best, next_emit, s_limit));
                    best = best_of(best, match_at(src, get_cur(next_long), s_fwd, cv_fwd as u32, &best, next_emit, s_limit));
                    best = best_of(best, match_at(src, get_prev(next_long), s_fwd, cv_fwd as u32, &best, next_emit, s_limit));

                    // Look ahead +2
                    let s_fwd2 = s_fwd + 1;
                    if s_fwd2 < src.len() - 8 {
                        let cv_fwd2 = load64(src, s_fwd2)?;
                        let hash_l_fwd2 = hash8(cv_fwd2, L_TABLE_BITS) as usize;
                        let hash_s_fwd2 = hash4(cv_fwd2, S_TABLE_BITS) as usize;
                        let next_long2 = l_table[hash_l_fwd2];
                        let next_short2 = s_table[hash_s_fwd2];

                        if repeat <= s_fwd2 {
                            best = best_of(best, match_at_repeat(src, s_fwd2 - repeat, s_fwd2, cv_fwd2 as u32, &best, next_emit, s_limit));
                        }

                        best = best_of(best, match_at(src, get_cur(next_short2), s_fwd2, cv_fwd2 as u32, &best, next_emit, s_limit));
                        best = best_of(best, match_at(src, get_prev(next_short2), s_fwd2, cv_fwd2 as u32, &best, next_emit, s_limit));
                        best = best_of(best, match_at(src, get_cur(next_long2), s_fwd2, cv_fwd2 as u32, &best, next_emit, s_limit));
                        best = best_of(best, match_at(src, get_prev(next_long2), s_fwd2, cv_fwd2 as u32, &best, next_emit, s_limit));
                    }
                }
            }

            // Search for a match at best match end - critical optimization from Go
            // This searches for better matches near the end of current best match
            if best.length > 0 {
                const SKIP_BEGINNING: usize = 2;
                const SKIP_END: usize = 1;
                let s_at = best.s + best.length - SKIP_END;

                if s_at < s_limit {
                    let s_back = best.s + SKIP_BEGINNING - SKIP_END;
                    let back_l = best.length - SKIP_BEGINNING;

                    if s_back < src.len().saturating_sub(8) && back_l > 0 {
                        let cv_back = load64(src, s_back)?;

                        // Get candidates from hash table at match end position
                        if s_at < src.len().saturating_sub(8) {
                            let hash_l_at = hash8(load64(src, s_at)?, L_TABLE_BITS) as usize;
                            let next_l = l_table[hash_l_at];

                            // Test candidates with backward extension
                            let check_at_cur = get_cur(next_l).saturating_sub(back_l);
                            let check_at_prev = get_prev(next_l).saturating_sub(back_l);

                            if check_at_cur > 0 {
                                best = best_of(best, match_at(src, check_at_cur, s_back, cv_back as u32, &best, next_emit, s_limit));
                            }
                            if check_at_prev > 0 {
                                best = best_of(best, match_at(src, check_at_prev, s_back, cv_back as u32, &best, next_emit, s_limit));
                            }

                            // Test short hash candidates too
                            let hash_s_at = hash4(load64(src, s_at)?, S_TABLE_BITS) as usize;
                            let next_s = s_table[hash_s_at];

                            let check_at_cur_s = get_cur(next_s).saturating_sub(back_l);
                            let check_at_prev_s = get_prev(next_s).saturating_sub(back_l);

                            if check_at_cur_s > 0 {
                                best = best_of(best, match_at(src, check_at_cur_s, s_back, cv_back as u32, &best, next_emit, s_limit));
                            }
                            if check_at_prev_s > 0 {
                                best = best_of(best, match_at(src, check_at_prev_s, s_back, cv_back as u32, &best, next_emit, s_limit));
                            }
                        }
                    }
                }
            }

            // Update hash tables - port of Go's table update logic
            l_table[hash_l] = s as u64 | (candidate_l << 32);
            s_table[hash_s] = s as u64 | (candidate_s << 32);

            if best.length > 0 {
                break;
            }

            cv = load64(src, next_s)?;
            s = next_s;
        }

        let start_idx = s + 1;
        s = best.s;

        // Bail if we exceed the maximum size
        if d + (s - next_emit) > dst_limit {
            return Ok(0);
        }

        let base = s;
        let offset = s - best.offset;
        s += best.length;

        // Bail if the match is equal or worse to the encoding (Go logic)
        if !best.rep && best.length <= 4 {
            if offset > 65535 ||
               (offset > MAX_COPY1_OFFSET && offset <= MAX_COPY2_OFFSET && base - next_emit > MAX_COPY2_LITS) {
                s = start_idx + 1;
                if s >= s_limit {
                    break 'outer;
                }
                cv = load64(src, s)?;
                continue;
            }
        }

        // Emit the match - port of Go's emission logic
        if best.rep {
            d += emit_literal(&mut dst[d..], &src[next_emit..base])?;
            d += emit_repeat(&mut dst[d..], best.length)?;
        } else {
            let lits = &src[next_emit..base];
            if !lits.is_empty() {
                if offset <= MAX_COPY2_OFFSET {
                    if lits.len() > MAX_COPY2_LITS || offset < 64 ||
                       (offset <= 1024 && best.length > 18) {
                        d += emit_literal(&mut dst[d..], lits)?;
                        d += emit_copy(&mut dst[d..], offset, best.length)?;
                    } else {
                        if best.length > 11 {
                            d += emit_copy_lits2(&mut dst[d..], lits, offset, 11)?;
                            s = best.s + 11;
                        } else {
                            d += emit_copy_lits2(&mut dst[d..], lits, offset, best.length)?;
                        }
                    }
                } else {
                    if lits.len() > MAX_COPY3_LITS {
                        d += emit_literal(&mut dst[d..], lits)?;
                        d += emit_copy(&mut dst[d..], offset, best.length)?;
                    } else {
                        d += emit_copy_lits3(&mut dst[d..], lits, offset, best.length)?;
                    }
                }
            } else {
                d += emit_copy(&mut dst[d..], offset, best.length)?;
            }
        }

        repeat = offset;
        next_emit = s;

        if s >= s_limit {
            break;
        }

        if d > dst_limit {
            return Ok(0);
        }

        // Fill tables for positions between start_idx and s (Go's indexing)
        for i in start_idx..s {
            if i + 8 <= src.len() {
                let cv0 = load64(src, i)?;
                let long0 = hash8(cv0, L_TABLE_BITS) as usize;
                let short0 = hash4(cv0, S_TABLE_BITS) as usize;
                l_table[long0] = i as u64 | (l_table[long0] << 32);
                s_table[short0] = i as u64 | (s_table[short0] << 32);
            }
        }

        cv = load64(src, s)?;
    }

    // Emit any remaining literals
    if next_emit < src.len() {
        let lit_len = src.len() - next_emit;
        if d + lit_len + emit_literal_size_n(lit_len) > dst_limit {
            return Ok(0);
        }
        d += emit_literal(&mut dst[d..], &src[next_emit..])?;
    }

    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_block_empty() {
        let mut dst = vec![0u8; 100];
        let src = b"";
        let result = encode_block(&mut dst, src).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_encode_block_small() {
        let mut dst = vec![0u8; 100];
        let src = b"hello";
        let result = encode_block(&mut dst, src).unwrap();
        assert_eq!(result, 0); // Too small to compress
    }

    #[test]
    fn test_encode_block_compressible() {
        let mut dst = vec![0u8; 1000];
        let src = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"; // 40 'a's
        let result = encode_block(&mut dst, src).unwrap();
        assert!(result > 0);
        assert!(result < src.len()); // Should compress better than Level 1 and 2
    }
}