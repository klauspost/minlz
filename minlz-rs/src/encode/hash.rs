//! Hash functions for MinLZ compression
//!
//! These functions implement various hash algorithms used by different compression levels
//! to find matching patterns in the input data. Each uses a different prime number
//! and byte count for optimal distribution and performance characteristics.

/// Hash function for 4-byte patterns.
/// Used by Level 2 and Level 3 encoders for short pattern matching.
/// Prime: 2654435761 (FNV-like prime for good distribution)
#[inline(always)]
pub fn hash4(u: u64, h: u8) -> u32 {
    const PRIME_4BYTES: u32 = 2654435761;
    ((u as u32).wrapping_mul(PRIME_4BYTES)) >> ((32 - h) & 31)
}

/// Hash function for 5-byte patterns.
/// Used by Level 2 encoder for intermediate pattern lengths.
/// Prime: 889523592379 (chosen for good avalanche properties)
#[inline(always)]
pub fn hash5(u: u64, h: u8) -> u32 {
    const PRIME_5BYTES: u64 = 889523592379;
    (((u << (64 - 40)).wrapping_mul(PRIME_5BYTES)) >> ((64 - h) & 63)) as u32
}

/// Hash function for 6-byte patterns.
/// Used by Level 1 and Level 2 encoders - the primary hash for Level 1.
/// Prime: 227718039650203 (optimized for 6-byte pattern distribution)
#[inline(always)]
pub fn hash6(u: u64, h: u8) -> u32 {
    const PRIME_6BYTES: u64 = 227718039650203;
    (((u << (64 - 48)).wrapping_mul(PRIME_6BYTES)) >> ((64 - h) & 63)) as u32
}

/// Hash function for 7-byte patterns.
/// Used by Level 2 and Level 3 encoders for longer pattern matching.
/// Prime: 58295818150454627 (chosen for minimal collisions on text data)
#[inline(always)]
pub fn hash7(u: u64, h: u8) -> u32 {
    const PRIME_7BYTES: u64 = 58295818150454627;
    (((u << (64 - 56)).wrapping_mul(PRIME_7BYTES)) >> ((64 - h) & 63)) as u32
}

/// Hash function for 8-byte patterns (full u64).
/// Used by Level 3 encoder for maximum pattern length matching.
/// Prime: 0xcf1bbcdcb7a56463 (high-quality hash multiplication constant)
#[inline(always)]
pub fn hash8(u: u64, h: u8) -> u32 {
    const PRIME_8BYTES: u64 = 0xcf1bbcdcb7a56463;
    (u.wrapping_mul(PRIME_8BYTES) >> ((64 - h) & 63)) as u32
}

/// Convenience function to load a 64-bit value and hash it with hash4
#[inline(always)]
#[allow(dead_code)]
pub fn hash4_at(data: &[u8], pos: usize, h: u8) -> u32 {
    if pos + 8 <= data.len() {
        let val = crate::memory::load64(data, pos).unwrap_or(0);
        hash4(val, h)
    } else {
        // Handle end-of-buffer case by loading available bytes
        let mut val = 0u64;
        for i in 0..std::cmp::min(8, data.len() - pos) {
            val |= (data[pos + i] as u64) << (i * 8);
        }
        hash4(val, h)
    }
}

/// Convenience function to load a 64-bit value and hash it with hash5
#[inline(always)]
#[allow(dead_code)]
pub fn hash5_at(data: &[u8], pos: usize, h: u8) -> u32 {
    if pos + 8 <= data.len() {
        let val = crate::memory::load64(data, pos).unwrap_or(0);
        hash5(val, h)
    } else {
        let mut val = 0u64;
        for i in 0..std::cmp::min(8, data.len() - pos) {
            val |= (data[pos + i] as u64) << (i * 8);
        }
        hash5(val, h)
    }
}

/// Convenience function to load a 64-bit value and hash it with hash6
#[inline(always)]
#[allow(dead_code)]
pub fn hash6_at(data: &[u8], pos: usize, h: u8) -> u32 {
    if pos + 8 <= data.len() {
        let val = crate::memory::load64(data, pos).unwrap_or(0);
        hash6(val, h)
    } else {
        let mut val = 0u64;
        for i in 0..std::cmp::min(8, data.len() - pos) {
            val |= (data[pos + i] as u64) << (i * 8);
        }
        hash6(val, h)
    }
}

/// Convenience function to load a 64-bit value and hash it with hash7
#[inline(always)]
#[allow(dead_code)]
pub fn hash7_at(data: &[u8], pos: usize, h: u8) -> u32 {
    if pos + 8 <= data.len() {
        let val = crate::memory::load64(data, pos).unwrap_or(0);
        hash7(val, h)
    } else {
        let mut val = 0u64;
        for i in 0..std::cmp::min(8, data.len() - pos) {
            val |= (data[pos + i] as u64) << (i * 8);
        }
        hash7(val, h)
    }
}

/// Convenience function to load a 64-bit value and hash it with hash8
#[inline(always)]
#[allow(dead_code)]
pub fn hash8_at(data: &[u8], pos: usize, h: u8) -> u32 {
    if pos + 8 <= data.len() {
        let val = crate::memory::load64(data, pos).unwrap_or(0);
        hash8(val, h)
    } else {
        let mut val = 0u64;
        for i in 0..std::cmp::min(8, data.len() - pos) {
            val |= (data[pos + i] as u64) << (i * 8);
        }
        hash8(val, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_functions_basic() {
        let test_val = 0x0123456789abcdef;

        // Test that different hash functions produce different results
        let h4 = hash4(test_val, 16);
        let h5 = hash5(test_val, 16);
        let h6 = hash6(test_val, 16);
        let h7 = hash7(test_val, 16);
        let h8 = hash8(test_val, 16);

        // They should be different (very unlikely to collide on this test value)
        assert_ne!(h4, h5);
        assert_ne!(h5, h6);
        assert_ne!(h6, h7);
        assert_ne!(h7, h8);

        // Test that they fit in the specified bit range
        assert!(h4 < (1 << 16));
        assert!(h5 < (1 << 16));
        assert!(h6 < (1 << 16));
        assert!(h7 < (1 << 16));
        assert!(h8 < (1 << 16));
    }

    #[test]
    fn test_hash_at_functions() {
        let data = b"Hello, world! This is test data for hashing.";

        let h4 = hash4_at(data, 0, 12);
        let h5 = hash5_at(data, 0, 12);
        let h6 = hash6_at(data, 0, 12);
        let h7 = hash7_at(data, 0, 12);
        let h8 = hash8_at(data, 0, 12);

        // Results should fit in bit range
        assert!(h4 < (1 << 12));
        assert!(h5 < (1 << 12));
        assert!(h6 < (1 << 12));
        assert!(h7 < (1 << 12));
        assert!(h8 < (1 << 12));

        // Test boundary conditions
        let h_end = hash4_at(data, data.len() - 1, 12);
        assert!(h_end < (1 << 12));
    }

    #[test]
    fn test_hash_distribution() {
        // Test that consecutive positions produce different hashes (avalanche effect)
        let data = b"abcdefghijklmnopqrstuvwxyz0123456789";

        let h1 = hash6_at(data, 0, 15);
        let h2 = hash6_at(data, 1, 15);
        let h3 = hash6_at(data, 2, 15);

        // Consecutive positions should hash to different values
        assert_ne!(h1, h2);
        assert_ne!(h2, h3);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_hash_consistency() {
        let data = b"test pattern";
        let h = 13;

        // Same input should produce same hash
        assert_eq!(hash6_at(data, 0, h), hash6_at(data, 0, h));

        // Different hash bit counts should produce different results
        assert_ne!(hash6_at(data, 0, 10), hash6_at(data, 0, 15));
    }
}