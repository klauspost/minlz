//! Level 2 (balanced) encoding implementation

use crate::{
    constants::*,
    error::Result,
    memory::{load64, load32},
    encode::{
        emit::{emit_literal, emit_repeat, emit_copy, emit_copy_lits2, emit_copy_lits3},
        hash::{hash4, hash6, hash7},
    },
};

/// Level 2 encoder - balanced compression using dual hash tables
/// Uses both long (7-byte) and short (4-byte) hash tables for better matches
pub fn encode_block(dst: &mut [u8], src: &[u8]) -> Result<usize> {
    if src.len() < MIN_NON_LITERAL_BLOCK_SIZE {
        return Ok(0);
    }

    if src.len() <= 64 << 10 {
        encode_block_64k(dst, src)
    } else {
        encode_block_large(dst, src)
    }
}

/// Level 2 encoder for inputs > 64K using 32-bit hash table entries
fn encode_block_large(dst: &mut [u8], src: &[u8]) -> Result<usize> {
    const L_TABLE_BITS: u8 = 17;
    const MAX_L_TABLE_SIZE: usize = 1 << L_TABLE_BITS;
    const S_TABLE_BITS: u8 = 14;
    const MAX_S_TABLE_SIZE: usize = 1 << S_TABLE_BITS;

    let s_limit = src.len().saturating_sub(INPUT_MARGIN);
    let dst_limit = src.len().saturating_sub(src.len() >> 5).saturating_sub(6);

    let mut l_table = vec![0u32; MAX_L_TABLE_SIZE];
    let mut s_table = vec![0u32; MAX_S_TABLE_SIZE];

    let mut d = 0;
    let mut next_emit = 0;
    let mut s = 1;
    let mut repeat = 1;

    let mut cv = load64(src, s)?;

    'outer: loop {
        let mut candidate_l;
        let mut next_s;

        // Inner hash table lookup loop
        loop {
            next_s = s + ((s - next_emit) >> 7) + 1;
            if next_s > s_limit {
                break 'outer;
            }

            let min_src_pos = s.saturating_sub(MAX_COPY3_OFFSET - 1);
            let hash_l = hash7(cv, L_TABLE_BITS) as usize;
            let hash_s = hash4(cv, S_TABLE_BITS) as usize;

            candidate_l = l_table[hash_l] as usize;
            let candidate_s = s_table[hash_s] as usize;

            l_table[hash_l] = s as u32;
            s_table[hash_s] = s as u32;

            if candidate_l + 8 <= src.len() {
                let val_long = load64(src, candidate_l)?;
                if candidate_l > min_src_pos && cv == val_long {
                    break;
                }
            }

            // Check repeat pattern
            if repeat > 1 && s >= repeat {
                const CHECK_REP: usize = 1;
                const WANT_REPEAT_BYTES: usize = 4;
                const REPEAT_MASK: u64 = ((1u64 << (WANT_REPEAT_BYTES * 8)) - 1) << (8 * CHECK_REP);

                if (cv & REPEAT_MASK) == (load64(src, s - repeat)? & REPEAT_MASK) {
                    let mut base = s + CHECK_REP;

                    // Extend backwards
                    while base > next_emit && base > repeat && src[base - 1] == src[base - repeat - 1] {
                        base -= 1;
                    }

                    if d + (base - next_emit) > dst_limit {
                        return Ok(0);
                    }

                    d += emit_literal(&mut dst[d..], &src[next_emit..base])?;

                    // Extend forward
                    let mut candidate = s - repeat + WANT_REPEAT_BYTES + CHECK_REP;
                    s += WANT_REPEAT_BYTES + CHECK_REP;

                    while s < src.len() {
                        if src.len() - s < 8 {
                            if src[s] == src[candidate] {
                                s += 1;
                                candidate += 1;
                                continue;
                            }
                            break;
                        }

                        let diff = load64(src, s)? ^ load64(src, candidate)?;
                        if diff != 0 {
                            s += (diff.trailing_zeros() >> 3) as usize;
                            break;
                        }
                        s += 8;
                        candidate += 8;
                    }

                    d += emit_repeat(&mut dst[d..], s - base)?;
                    next_emit = s;

                    if s >= s_limit {
                        break 'outer;
                    }

                    // Index positions in between
                    let mut index0 = base + 1;
                    let mut index1 = s.saturating_sub(2);

                    while index0 < index1 {
                        if index0 + 8 <= src.len() && index1 + 8 <= src.len() {
                            let cv0 = load64(src, index0)?;
                            let cv1 = load64(src, index1)?;

                            l_table[hash7(cv0, L_TABLE_BITS) as usize] = index0 as u32;
                            s_table[hash4(cv0 >> 8, S_TABLE_BITS) as usize] = (index0 + 1) as u32;

                            l_table[hash7(cv1, L_TABLE_BITS) as usize] = index1 as u32;
                            s_table[hash4(cv1 >> 8, S_TABLE_BITS) as usize] = (index1 + 1) as u32;
                        }
                        index0 += 2;
                        index1 = if index1 >= 2 { index1 - 2 } else { 0 };
                    }

                    cv = load64(src, s)?;
                    continue 'outer;
                }
            }

            // Check candidates
            if candidate_l >= min_src_pos && candidate_l + 4 <= src.len() {
                let val_long = load32(src, candidate_l)?;
                if (cv as u32) == val_long {
                    break;
                }
            }

            if candidate_s >= min_src_pos && candidate_s + 4 <= src.len() {
                let val_short = load32(src, candidate_s)?;
                if (cv as u32) == val_short {
                    // Try long candidate at s+1
                    let hash_l = hash7(cv >> 8, L_TABLE_BITS) as usize;
                    let candidate_l_next = l_table[hash_l] as usize;
                    l_table[hash_l] = (s + 1) as u32;

                    if candidate_l_next > min_src_pos && candidate_l_next + 4 <= src.len() {
                        let val_next = load32(src, candidate_l_next)?;
                        if ((cv >> 8) as u32) == val_next {
                            s += 1;
                            candidate_l = candidate_l_next;
                            break;
                        }
                    }

                    candidate_l = candidate_s;
                    break;
                }
            }

            cv = load64(src, next_s)?;
            s = next_s;
        }

        // Extend match backwards
        while candidate_l > 0 && s > next_emit && src[candidate_l - 1] == src[s - 1] {
            candidate_l -= 1;
            s -= 1;
        }

        if d + (s - next_emit) > dst_limit {
            return Ok(0);
        }

        let base = s;
        let offset = base - candidate_l;

        // Extend match forward
        s += 4;
        candidate_l += 4;

        while s < src.len() {
            if src.len() - s < 8 {
                if src[s] == src[candidate_l] {
                    s += 1;
                    candidate_l += 1;
                    continue;
                }
                break;
            }

            let diff = load64(src, s)? ^ load64(src, candidate_l)?;
            if diff != 0 {
                s += (diff.trailing_zeros() >> 3) as usize;
                break;
            }
            s += 8;
            candidate_l += 8;
        }

        // Check if match is worth encoding
        if offset > 65535 && s - base <= 4 && repeat != offset {
            s = next_s + 1;
            if s >= s_limit {
                break;
            }
            cv = load64(src, s)?;
            continue;
        }

        let lits = &src[next_emit..base];

        // Validate match parameters before emitting
        let match_length = s - base;
        if match_length < 4 || offset == 0 || offset > src.len() {
            // Invalid match, bail out
            return Ok(0);
        }

        if !lits.is_empty() {
            if offset <= MAX_COPY2_OFFSET {
                if lits.len() > MAX_COPY2_LITS || offset < 64 {
                    d += emit_literal(&mut dst[d..], lits)?;
                    let written = emit_copy(&mut dst[d..], offset, s - base)?;
                    d += written;
                } else {
                    let written = emit_copy_lits2(&mut dst[d..], lits, offset, s - base)?;
                    d += written;
                }
            } else {
                if lits.len() > MAX_COPY3_LITS {
                    d += emit_literal(&mut dst[d..], lits)?;
                    let written = emit_copy(&mut dst[d..], offset, s - base)?;
                    d += written;
                } else {
                    let written = emit_copy_lits3(&mut dst[d..], lits, offset, s - base)?;
                    d += written;
                }
            }
        } else {
            let written = emit_copy(&mut dst[d..], offset, s - base)?;
            d += written;
        }

        repeat = offset;
        next_emit = s;

        if s >= s_limit {
            break;
        }

        if d > dst_limit {
            return Ok(0);
        }


        // Index short & long entries
        let index0 = base + 1;
        let index1 = s.saturating_sub(2);

        if index0 + 8 <= src.len() && index1 + 8 <= src.len() {
            let cv0 = load64(src, index0)?;
            let cv1 = load64(src, index1)?;

            l_table[hash7(cv0, L_TABLE_BITS) as usize] = index0 as u32;
            s_table[hash4(cv0 >> 8, S_TABLE_BITS) as usize] = (index0 + 1) as u32;

            l_table[hash7(cv1, L_TABLE_BITS) as usize] = index1 as u32;
            s_table[hash4(cv1 >> 8, S_TABLE_BITS) as usize] = (index1 + 1) as u32;

            // Index sparsely in between
            let mut idx0 = index0 + 1;
            let idx1 = index1.saturating_sub(1);
            let mut index2 = (idx0 + idx1 + 1) >> 1;

            while index2 < idx1 && idx0 + 8 <= src.len() && index2 + 8 <= src.len() {
                l_table[hash7(load64(src, idx0)?, L_TABLE_BITS) as usize] = idx0 as u32;
                l_table[hash7(load64(src, index2)?, L_TABLE_BITS) as usize] = index2 as u32;
                idx0 += 2;
                index2 += 2;
            }
        }

        cv = load64(src, s)?;
    }

    if next_emit < src.len() {
        if d + (src.len() - next_emit) > dst_limit {
            return Ok(0);
        }
        d += emit_literal(&mut dst[d..], &src[next_emit..])?;
    }

    Ok(d)
}

/// Level 2 encoder for inputs <= 64K using 16-bit hash table entries
fn encode_block_64k(dst: &mut [u8], src: &[u8]) -> Result<usize> {
    const L_TABLE_BITS: u8 = 15;
    const MAX_L_TABLE_SIZE: usize = 1 << L_TABLE_BITS;
    const S_TABLE_BITS: u8 = 12;
    const MAX_S_TABLE_SIZE: usize = 1 << S_TABLE_BITS;

    let s_limit = src.len().saturating_sub(INPUT_MARGIN);
    let dst_limit = src.len().saturating_sub(src.len() >> 5).saturating_sub(6);

    let mut l_table = vec![0u16; MAX_L_TABLE_SIZE];
    let mut s_table = vec![0u16; MAX_S_TABLE_SIZE];

    let mut d = 0;
    let mut next_emit = 0;
    let mut s = 1;
    let mut repeat = 1;

    let mut cv = load64(src, s)?;

    'outer: loop {
        let mut candidate_l;
        let mut next_s;

        // Inner hash table lookup loop
        loop {
            next_s = s + ((s - next_emit) >> 7) + 1;
            if next_s > s_limit {
                break 'outer;
            }

            let hash_l = hash6(cv, L_TABLE_BITS) as usize;
            let hash_s = hash4(cv, S_TABLE_BITS) as usize;

            candidate_l = l_table[hash_l] as usize;
            let candidate_s = s_table[hash_s] as usize;

            l_table[hash_l] = s as u16;
            s_table[hash_s] = s as u16;

            if candidate_l + 8 <= src.len() {
                let val_long = load64(src, candidate_l)?;
                if cv == val_long {
                    break;
                }
            }

            // Check repeat pattern
            if repeat > 1 && s >= repeat {
                const CHECK_REP: usize = 1;
                const WANT_REPEAT_BYTES: usize = 4;
                const REPEAT_MASK: u64 = ((1u64 << (WANT_REPEAT_BYTES * 8)) - 1) << (8 * CHECK_REP);

                if (cv & REPEAT_MASK) == (load64(src, s - repeat)? & REPEAT_MASK) {
                    let mut base = s + CHECK_REP;

                    // Extend backwards
                    while base > next_emit && base > repeat && src[base - 1] == src[base - repeat - 1] {
                        base -= 1;
                    }

                    if d + (base - next_emit) > dst_limit {
                        return Ok(0);
                    }

                    d += emit_literal(&mut dst[d..], &src[next_emit..base])?;

                    // Extend forward
                    let mut candidate = s - repeat + WANT_REPEAT_BYTES + CHECK_REP;
                    s += WANT_REPEAT_BYTES + CHECK_REP;

                    while s < src.len() {
                        if src.len() - s < 8 {
                            if src[s] == src[candidate] {
                                s += 1;
                                candidate += 1;
                                continue;
                            }
                            break;
                        }

                        let diff = load64(src, s)? ^ load64(src, candidate)?;
                        if diff != 0 {
                            s += (diff.trailing_zeros() >> 3) as usize;
                            break;
                        }
                        s += 8;
                        candidate += 8;
                    }

                    d += emit_repeat(&mut dst[d..], s - base)?;
                    next_emit = s;

                    if s >= s_limit {
                        break 'outer;
                    }

                    // Index positions in between
                    let mut index0 = base + 1;
                    let mut index1 = s.saturating_sub(2);

                    while index0 < index1 {
                        if index0 + 8 <= src.len() && index1 + 8 <= src.len() {
                            let cv0 = load64(src, index0)?;
                            let cv1 = load64(src, index1)?;

                            l_table[hash6(cv0, L_TABLE_BITS) as usize] = index0 as u16;
                            s_table[hash4(cv0 >> 8, S_TABLE_BITS) as usize] = (index0 + 1) as u16;

                            l_table[hash6(cv1, L_TABLE_BITS) as usize] = index1 as u16;
                            s_table[hash4(cv1 >> 8, S_TABLE_BITS) as usize] = (index1 + 1) as u16;
                        }
                        index0 += 2;
                        index1 = if index1 >= 2 { index1 - 2 } else { 0 };
                    }

                    cv = load64(src, s)?;
                    continue 'outer;
                }
            }

            // Check candidates
            if candidate_l + 4 <= src.len() {
                let val_long = load32(src, candidate_l)?;
                if (cv as u32) == val_long {
                    break;
                }
            }

            if candidate_s + 4 <= src.len() {
                let val_short = load32(src, candidate_s)?;
                if (cv as u32) == val_short {
                    // Try long candidate at s+1
                    let hash_l = hash6(cv >> 8, L_TABLE_BITS) as usize;
                    let candidate_l_next = l_table[hash_l] as usize;
                    l_table[hash_l] = (s + 1) as u16;

                    if candidate_l_next + 4 <= src.len() {
                        let val_next = load32(src, candidate_l_next)?;
                        if ((cv >> 8) as u32) == val_next {
                            s += 1;
                            candidate_l = candidate_l_next;
                            break;
                        }
                    }

                    candidate_l = candidate_s;
                    break;
                }
            }

            cv = load64(src, next_s)?;
            s = next_s;
        }

        // Extend match backwards
        while candidate_l > 0 && s > next_emit && src[candidate_l - 1] == src[s - 1] {
            candidate_l -= 1;
            s -= 1;
        }

        if d + (s - next_emit) > dst_limit {
            return Ok(0);
        }

        let base = s;
        let offset = base - candidate_l;

        // Extend match forward
        s += 4;
        candidate_l += 4;

        while s < src.len() {
            if src.len() - s < 8 {
                if src[s] == src[candidate_l] {
                    s += 1;
                    candidate_l += 1;
                    continue;
                }
                break;
            }

            let diff = load64(src, s)? ^ load64(src, candidate_l)?;
            if diff != 0 {
                s += (diff.trailing_zeros() >> 3) as usize;
                break;
            }
            s += 8;
            candidate_l += 8;
        }

        let lits = &src[next_emit..base];

        // Validate match parameters before emitting
        let match_length = s - base;
        if match_length < 4 || offset == 0 || offset > src.len() {
            // Invalid match, bail out
            return Ok(0);
        }

        if !lits.is_empty() {
            // For 64K inputs, always prefer Copy2 over Copy1 for faster decode
            if lits.len() > MAX_COPY2_LITS || offset < 64 {
                let written1 = emit_literal(&mut dst[d..], lits)?;
                d += written1;
                let written2 = emit_copy(&mut dst[d..], offset, s - base)?;
                d += written2;
            } else {
                let written = emit_copy_lits2(&mut dst[d..], lits, offset, s - base)?;
                d += written;
            }
        } else {
            let written = emit_copy(&mut dst[d..], offset, s - base)?;
            d += written;
        }

        repeat = offset;
        next_emit = s;

        if s >= s_limit {
            break;
        }

        if d > dst_limit {
            return Ok(0);
        }


        // Index short & long entries
        let index0 = base + 1;
        let index1 = s.saturating_sub(2);

        if index0 + 8 <= src.len() && index1 + 8 <= src.len() {
            let cv0 = load64(src, index0)?;
            let cv1 = load64(src, index1)?;

            l_table[hash6(cv0, L_TABLE_BITS) as usize] = index0 as u16;
            s_table[hash4(cv0 >> 8, S_TABLE_BITS) as usize] = (index0 + 1) as u16;

            l_table[hash6(cv1, L_TABLE_BITS) as usize] = index1 as u16;
            s_table[hash4(cv1 >> 8, S_TABLE_BITS) as usize] = (index1 + 1) as u16;

            // Index sparsely in between
            let mut idx0 = index0 + 1;
            let idx1 = index1.saturating_sub(1);
            let mut index2 = (idx0 + idx1 + 1) >> 1;

            while index2 < idx1 && idx0 + 8 <= src.len() && index2 + 8 <= src.len() {
                l_table[hash6(load64(src, idx0)?, L_TABLE_BITS) as usize] = idx0 as u16;
                l_table[hash6(load64(src, index2)?, L_TABLE_BITS) as usize] = index2 as u16;
                idx0 += 2;
                index2 += 2;
            }
        }

        cv = load64(src, s)?;
    }

    if next_emit < src.len() {
        if d + (src.len() - next_emit) > dst_limit {
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
        let mut dst = vec![0u8; 200];
        let src = b"AAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let result = encode_block(&mut dst, src).unwrap();
        assert!(result > 0 && result < src.len());
    }

    #[test]
    fn test_encode_block_64k() {
        let mut dst = vec![0u8; 70000];
        let mut src = vec![0u8; 65536];

        // Create compressible pattern
        for i in 0..src.len() {
            src[i] = (i / 100) as u8;
        }

        let result = encode_block(&mut dst, &src).unwrap();
        assert!(result > 0 && result < src.len());
    }

    #[test]
    fn test_encode_block_large() {
        let mut dst = vec![0u8; 150000];
        let mut src = vec![0u8; 100000];

        // Create compressible pattern
        for i in 0..src.len() {
            src[i] = (i / 200) as u8;
        }

        let result = encode_block(&mut dst, &src).unwrap();
        assert!(result > 0 && result < src.len());
    }
}