//! Level 1 (fastest) encoding implementation
//!
//! This module implements the fastest MinLZ compression level, optimized for speed
//! over compression ratio. It uses a simple hash table approach with 6-byte patterns.

use crate::{
    constants::*,
    encode::{emit::*, hash::*},
    error::Result,
    memory::*,
};

/// Encode a block using the fastest compression level
pub fn encode_block(dst: &mut [u8], src: &[u8]) -> Result<usize> {
    if src.len() < MIN_NON_LITERAL_BLOCK_SIZE {
        return Ok(0);
    }

    if src.len() <= 65536 {
        encode_block_64k(dst, src)
    } else {
        encode_block_large(dst, src)
    }
}

/// Encode blocks larger than 64K using a 15-bit hash table
fn encode_block_large(dst: &mut [u8], src: &[u8]) -> Result<usize> {
    const TABLE_BITS: u8 = 15;
    const MAX_TABLE_SIZE: usize = 1 << TABLE_BITS;
    const SKIP_LOG: usize = 6;
    const INPUT_MARGIN: usize = 8 + 2;

    let mut table = vec![0u32; MAX_TABLE_SIZE];

    // When to stop looking for matches - leave margin for safety
    let s_limit = src.len().saturating_sub(INPUT_MARGIN);

    // Bail if we can't compress to at least this ratio
    let dst_limit = src.len() - (src.len() >> 5) - 6;

    // Track where to emit literals from
    let mut next_emit = 0;

    // Start looking for matches at position 1
    let mut s = 1;
    let mut cv = unsafe { load64_unchecked(src, s) }; // Safe: s=1, src.len() >= INPUT_MARGIN

    // Track repeat offset for repeat detection
    let mut repeat = 1;
    let mut d = 0; // Current output position

    'outer: loop {
        let mut candidate;

        // Find a match
        loop {
            // Next position to check
            let next_s = s + ((s - next_emit) >> SKIP_LOG) + 4;
            if next_s > s_limit {
                // Emit remainder
                if next_emit < src.len() {
                    if d + src.len() - next_emit > dst_limit {
                        return Ok(0);
                    }
                    d += emit_literal(&mut dst[d..], &src[next_emit..])?;
                }
                return Ok(d);
            }

            let min_src_pos = s.saturating_sub(MAX_COPY3_OFFSET);

            // Hash current and next positions
            let hash0 = hash6(cv, TABLE_BITS);
            let hash1 = hash6(cv >> 8, TABLE_BITS);
            candidate = table[hash0 as usize] as usize;
            let candidate2 = table[hash1 as usize] as usize;
            table[hash0 as usize] = s as u32;
            table[hash1 as usize] = (s + 1) as u32;
            let hash2 = hash6(cv >> 16, TABLE_BITS);

            // Check repeat at offset 1
            const CHECK_REP: usize = 1;
            if (cv >> (CHECK_REP * 8)) as u32 == unsafe { load32_unchecked(src, s - repeat + CHECK_REP) } {
                let mut base = s + CHECK_REP;

                // Extend backwards
                while base > next_emit && s - repeat > 0 &&
                      src[s - repeat + CHECK_REP - (s + CHECK_REP - base) - 1] == src[base - 1] {
                    base -= 1;
                }

                // Check if we exceed size limit
                if d + (base - next_emit) > dst_limit {
                    return Ok(0);
                }

                // Emit literals before the repeat
                d += emit_literal(&mut dst[d..], &src[next_emit..base])?;

                // Extend forward
                let mut candidate_pos = s - repeat + 4 + CHECK_REP;
                s += 4 + CHECK_REP;

                while s <= s_limit {
                    let a = unsafe { load64_unchecked(src, s) };
                    let b = unsafe { load64_unchecked(src, candidate_pos) };
                    if a ^ b != 0 {
                        // Find exact mismatch position
                        let diff = a ^ b;
                        s += diff.trailing_zeros() as usize / 8;
                        break;
                    }
                    s += 8;
                    candidate_pos += 8;
                }

                d += emit_repeat(&mut dst[d..], s - base)?;
                next_emit = s;

                if s >= s_limit {
                    // Emit remainder
                    if next_emit < src.len() {
                        if d + src.len() - next_emit > dst_limit {
                            return Ok(0);
                        }
                        d += emit_literal(&mut dst[d..], &src[next_emit..])?;
                    }
                    return Ok(d);
                }

                cv = unsafe { load64_unchecked(src, s) };
                continue 'outer;
            }

            // Check candidate matches
            if candidate >= min_src_pos && (cv as u32) == unsafe { load32_unchecked(src, candidate) } {
                break;
            }

            candidate = table[hash2 as usize] as usize;
            if candidate2 >= min_src_pos && ((cv >> 8) as u32) == unsafe { load32_unchecked(src, candidate2) } {
                table[hash2 as usize] = (s + 2) as u32;
                candidate = candidate2;
                s += 1;
                break;
            }

            table[hash2 as usize] = (s + 2) as u32;
            if candidate >= min_src_pos && ((cv >> 16) as u32) == unsafe { load32_unchecked(src, candidate) } {
                s += 2;
                break;
            }

            cv = unsafe { load64_unchecked(src, next_s) };
            s = next_s;
        }

        // Extend backwards
        while candidate > 0 && s > next_emit && src[candidate - 1] == src[s - 1] {
            candidate -= 1;
            s -= 1;
        }

        // Found a 4-byte match, extend it forward
        let mut base = s;
        repeat = base - candidate;

        // Extend the match
        s += 4;
        candidate += 4;

        while s <= src.len() - 8 {
            let diff = unsafe { load64_unchecked(src, s) } ^ unsafe { load64_unchecked(src, candidate) };
            if diff != 0 {
                s += diff.trailing_zeros() as usize / 8;
                break;
            }
            s += 8;
            candidate += 8;
        }

        let length = s - base;


        // Emit literals and copy
        if next_emit != base {
            if base - next_emit > MAX_COPY3_LITS || repeat < MIN_COPY2_OFFSET {
                // Check size limit
                if d + (s - next_emit) > dst_limit {
                    return Ok(0);
                }
                d += emit_literal(&mut dst[d..], &src[next_emit..base])?;
                d += emit_copy(&mut dst[d..], repeat, length)?;
            } else if repeat <= MAX_COPY2_OFFSET {
                d += emit_copy_lits2(&mut dst[d..], &src[next_emit..base], repeat, length)?;
            } else {
                d += emit_copy_lits3(&mut dst[d..], &src[next_emit..base], repeat, length)?;
            }
        } else {
            d += emit_copy(&mut dst[d..], repeat, length)?;
        }

        // Update next_emit after emitting copy
        next_emit = s;

        // Look for immediate matches
        loop {
            if s >= s_limit {
                // Emit remainder
                if next_emit < src.len() {
                    if d + src.len() - next_emit > dst_limit {
                        return Ok(0);
                    }
                    d += emit_literal(&mut dst[d..], &src[next_emit..])?;
                }
                return Ok(d);
            }

            let x = unsafe { load64_unchecked(src, s - 2) };

            if d > dst_limit {
                return Ok(0);
            }

            // Check for immediate match
            let m2_hash = hash6(x, TABLE_BITS);
            let x = x >> 16;
            let curr_hash = hash6(x, TABLE_BITS);
            candidate = table[curr_hash as usize] as usize;
            table[m2_hash as usize] = (s - 2) as u32;
            table[curr_hash as usize] = s as u32;

            if s - candidate > MAX_COPY3_OFFSET || (x as u32) != unsafe { load32_unchecked(src, candidate) } {
                cv = unsafe { load64_unchecked(src, s + 1) };
                s += 1;
                break;
            }

            repeat = s - candidate;
            base = s;
            s += 4;
            candidate += 4;

            while s <= src.len() - 8 {
                let diff = load64(src, s)? ^ load64(src, candidate)?;
                if diff != 0 {
                    s += diff.trailing_zeros() as usize / 8;
                    break;
                }
                s += 8;
                candidate += 8;
            }

            d += emit_copy(&mut dst[d..], repeat, s - base)?;
            next_emit = s;
        }
    }
}

/// Encode blocks 64K or smaller using a 13-bit hash table (more cache-friendly)
fn encode_block_64k(dst: &mut [u8], src: &[u8]) -> Result<usize> {
    const TABLE_BITS: u8 = 13;
    const MAX_TABLE_SIZE: usize = 1 << TABLE_BITS;
    const SKIP_LOG: usize = 5;
    const INPUT_MARGIN: usize = 8 + 2;

    let mut table = vec![0u16; MAX_TABLE_SIZE];

    let s_limit = src.len().saturating_sub(INPUT_MARGIN);
    let dst_limit = src.len() - (src.len() >> 5) - 6;

    let mut next_emit = 0;
    let mut s = 1;
    let mut cv = unsafe { load64_unchecked(src, s) };
    let mut repeat = 1;
    let mut d = 0;

    'outer: loop {
        let mut candidate;

        // Find a match
        loop {
            let next_s = s + ((s - next_emit) >> SKIP_LOG) + 4;
            if next_s > s_limit {
                // Emit remainder
                if next_emit < src.len() {
                    if d + src.len() - next_emit > dst_limit {
                        return Ok(0);
                    }
                    d += emit_literal(&mut dst[d..], &src[next_emit..])?;
                }
                return Ok(d);
            }

            // Use hash5 for 64K variant (smaller patterns, better for small blocks)
            let hash0 = hash5(cv, TABLE_BITS);
            let hash1 = hash5(cv >> 8, TABLE_BITS);
            candidate = table[hash0 as usize] as usize;
            let candidate2 = table[hash1 as usize] as usize;
            table[hash0 as usize] = s as u16;
            table[hash1 as usize] = (s + 1) as u16;
            let hash2 = hash5(cv >> 16, TABLE_BITS);

            // Check repeat
            const CHECK_REP: usize = 1;
            if (cv >> (CHECK_REP * 8)) as u32 == unsafe { load32_unchecked(src, s - repeat + CHECK_REP) } {
                let mut base = s + CHECK_REP;

                // Extend backwards
                while base > next_emit && s - repeat > 0 &&
                      src[s - repeat + CHECK_REP - (s + CHECK_REP - base) - 1] == src[base - 1] {
                    base -= 1;
                }

                if d + (base - next_emit) > dst_limit {
                    return Ok(0);
                }

                d += emit_literal(&mut dst[d..], &src[next_emit..base])?;

                // Extend forward
                let mut candidate_pos = s - repeat + 4 + CHECK_REP;
                s += 4 + CHECK_REP;

                while s <= s_limit {
                    if load64(src, s)? ^ load64(src, candidate_pos)? != 0 {
                        let diff = load64(src, s)? ^ load64(src, candidate_pos)?;
                        s += diff.trailing_zeros() as usize / 8;
                        break;
                    }
                    s += 8;
                    candidate_pos += 8;
                }

                d += emit_repeat(&mut dst[d..], s - base)?;
                next_emit = s;

                if s >= s_limit {
                    // Emit remainder
                    if next_emit < src.len() {
                        if d + src.len() - next_emit > dst_limit {
                            return Ok(0);
                        }
                        d += emit_literal(&mut dst[d..], &src[next_emit..])?;
                    }
                    return Ok(d);
                }

                cv = unsafe { load64_unchecked(src, s) };
                continue 'outer;
            }

            // Check candidates (no bound check needed for 64K)
            if (cv as u32) == unsafe { load32_unchecked(src, candidate) } {
                break;
            }

            candidate = table[hash2 as usize] as usize;
            if ((cv >> 8) as u32) == unsafe { load32_unchecked(src, candidate2) } {
                table[hash2 as usize] = (s + 2) as u16;
                candidate = candidate2;
                s += 1;
                break;
            }

            table[hash2 as usize] = (s + 2) as u16;
            if ((cv >> 16) as u32) == unsafe { load32_unchecked(src, candidate) } {
                s += 2;
                break;
            }

            cv = unsafe { load64_unchecked(src, next_s) };
            s = next_s;
        }

        // Extend backwards
        while candidate > 0 && s > next_emit && src[candidate - 1] == src[s - 1] {
            candidate -= 1;
            s -= 1;
        }

        let mut base = s;
        repeat = base - candidate;

        // Extend forward
        s += 4;
        candidate += 4;

        while s <= src.len() - 8 {
            let diff = unsafe { load64_unchecked(src, s) } ^ unsafe { load64_unchecked(src, candidate) };
            if diff != 0 {
                s += diff.trailing_zeros() as usize / 8;
                break;
            }
            s += 8;
            candidate += 8;
        }

        let length = s - base;

        // Emit literals and copy (simpler logic for 64K)
        if next_emit != base {
            if base - next_emit > MAX_COPY2_LITS || repeat < MIN_COPY2_OFFSET {
                if d + (s - next_emit) > dst_limit {
                    return Ok(0);
                }
                d += emit_literal(&mut dst[d..], &src[next_emit..base])?;
                d += emit_copy(&mut dst[d..], repeat, length)?;
            } else {
                d += emit_copy_lits2(&mut dst[d..], &src[next_emit..base], repeat, length)?;
            }
        } else {
            d += emit_copy(&mut dst[d..], repeat, length)?;
        }

        // Update next_emit after emitting copy
        next_emit = s;

        // Look for immediate matches
        loop {
            if s >= s_limit {
                // Emit remainder
                if next_emit < src.len() {
                    if d + src.len() - next_emit > dst_limit {
                        return Ok(0);
                    }
                    d += emit_literal(&mut dst[d..], &src[next_emit..])?;
                }
                return Ok(d);
            }

            let x = unsafe { load64_unchecked(src, s - 2) };

            if d > dst_limit {
                return Ok(0);
            }

            let m2_hash = hash5(x, TABLE_BITS);
            let x = x >> 16;
            let curr_hash = hash5(x, TABLE_BITS);
            candidate = table[curr_hash as usize] as usize;
            table[m2_hash as usize] = (s - 2) as u16;
            table[curr_hash as usize] = s as u16;

            if (x as u32) != load32(src, candidate)? {
                cv = load64(src, s + 1)?;
                s += 1;
                break;
            }

            repeat = s - candidate;
            base = s;
            s += 4;
            candidate += 4;

            while s <= src.len() - 8 {
                let diff = load64(src, s)? ^ load64(src, candidate)?;
                if diff != 0 {
                    s += diff.trailing_zeros() as usize / 8;
                    break;
                }
                s += 8;
                candidate += 8;
            }

            d += emit_copy(&mut dst[d..], repeat, s - base)?;
            next_emit = s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_block_basic() {
        // Test with a pattern that Level 1 should definitely compress
        let src = b"AAAAAAAAAAAAAAAAAAAAAAAAAAAA"; // 28 A's - very compressible
        let mut dst = vec![0u8; 100];

        let result = encode_block(&mut dst, src).unwrap();

        // Should compress (repeated pattern)
        assert!(result > 0, "Level 1 should compress highly repetitive data");
        assert!(result < src.len(), "Compressed size should be smaller than original");
    }

    #[test]
    fn test_encode_block_realistic() {
        let src = b"Hello, world! Hello, world!";
        let mut dst = vec![0u8; 100];

        let result = encode_block(&mut dst, src).unwrap();

        // Level 1 might not compress this pattern efficiently - that's OK
        // The result should be either compressed (result > 0) or
        // indicate that compression wasn't worthwhile (result == 0)
        assert!(result == 0 || result < src.len(),
                "Level 1 should either compress or indicate compression isn't worthwhile");
    }

    #[test]
    fn test_encode_block_small() {
        let src = b"Hi";
        let mut dst = vec![0u8; 100];

        let result = encode_block(&mut dst, src).unwrap();

        // Too small to compress
        assert_eq!(result, 0);
    }

    #[test]
    fn test_encode_block_64k_vs_large() {
        let small_data = vec![b'A'; 1000];
        let large_data = vec![b'A'; 100000];

        let mut dst1 = vec![0u8; 2000];
        let mut dst2 = vec![0u8; 200000];

        let result1 = encode_block(&mut dst1, &small_data).unwrap();
        let result2 = encode_block(&mut dst2, &large_data).unwrap();

        // Both should compress repeated data well
        assert!(result1 > 0);
        assert!(result2 > 0);
        assert!(result1 < 100); // Very small output for repeated pattern
        assert!(result2 < 1000); // Still small for large repeated pattern
    }
}