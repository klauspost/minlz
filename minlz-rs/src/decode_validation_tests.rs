//! Comprehensive unit tests for decoder helper functions
//! These tests validate individual decoder functions to catch bugs early

use crate::{
    constants::*,
    decode::{
        decode_copy1_safe, decode_copy2_safe, decode_copy3_safe, decode_fused_copy2_safe,
        decode_literal_header,
    },
    error::Result,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_literal_header_basic() -> Result<()> {
        // Test basic literal lengths (0-28)
        for length_bits in 0..=28 {
            let tag = length_bits << 3; // No repeat flag
            let src = vec![0x00]; // Dummy data
            let mut s = 0;

            let (length, repeat) = decode_literal_header(&src, &mut s, tag)?;

            assert_eq!(
                length,
                (length_bits + 1) as usize,
                "Basic literal length for bits {}",
                length_bits
            );
            assert_eq!(repeat, false, "No repeat flag for basic literal");
            assert_eq!(s, 0, "Source position unchanged for basic literal");
        }
        Ok(())
    }

    #[test]
    fn test_decode_literal_header_with_repeat() -> Result<()> {
        // Test repeat flag
        let tag = 0x04 | (10 << 3); // Repeat flag + length_bits=10
        let src = vec![0x00];
        let mut s = 0;

        let (length, repeat) = decode_literal_header(&src, &mut s, tag)?;

        assert_eq!(length, 11, "Length should be 10 + 1");
        assert_eq!(repeat, true, "Repeat flag should be set");
        Ok(())
    }

    #[test]
    fn test_decode_literal_header_1_byte_extension() -> Result<()> {
        // Test 1-byte extension (length_bits = 29)
        let tag = 29 << 3; // length_bits = 29
        let src = vec![100]; // Extension byte
        let mut s = 0;

        let (length, repeat) = decode_literal_header(&src, &mut s, tag)?;

        assert_eq!(length, 30 + 100, "1-byte extension: 30 + 100");
        assert_eq!(repeat, false, "No repeat flag");
        assert_eq!(s, 1, "Source advanced by 1 byte");
        Ok(())
    }

    #[test]
    fn test_decode_literal_header_2_byte_extension() -> Result<()> {
        // Test 2-byte extension (length_bits = 30)
        let tag = 30 << 3; // length_bits = 30
        let src = vec![0x34, 0x12]; // Little-endian 0x1234 = 4660
        let mut s = 0;

        let (length, repeat) = decode_literal_header(&src, &mut s, tag)?;

        assert_eq!(length, 30 + 4660, "2-byte extension: 30 + 4660");
        assert_eq!(repeat, false, "No repeat flag");
        assert_eq!(s, 2, "Source advanced by 2 bytes");
        Ok(())
    }

    #[test]
    fn test_decode_literal_header_3_byte_extension() -> Result<()> {
        // Test 3-byte extension (length_bits = 31)
        let tag = 31 << 3; // length_bits = 31
        let src = vec![0x04, 0x78, 0x8f]; // The exact bytes from our bug case
        let mut s = 0;

        let (length, repeat) = decode_literal_header(&src, &mut s, tag)?;

        // Calculate expected: 0x04 | (0x78 << 8) | (0x8f << 16) = 4 + 30720 + 9437184 = 9467908
        let expected_ext = 0x04 | (0x78 << 8) | (0x8f << 16);
        assert_eq!(length, 30 + expected_ext, "3-byte extension calculation");
        assert_eq!(repeat, false, "No repeat flag");
        assert_eq!(s, 3, "Source advanced by 3 bytes");
        Ok(())
    }

    #[test]
    fn test_decode_literal_header_3_byte_with_repeat() -> Result<()> {
        // Test 3-byte extension with repeat flag
        let tag = 0x04 | (31 << 3); // Repeat + length_bits = 31
        let src = vec![0x88, 0x8f, 0x01]; // From our original test case
        let mut s = 0;

        let (length, repeat) = decode_literal_header(&src, &mut s, tag)?;

        // This should give us the 102,310 bytes from our test case
        let expected_ext = 0x88 | (0x8f << 8) | (0x01 << 16);
        assert_eq!(
            length,
            30 + expected_ext,
            "Our test case: 30 + {}",
            expected_ext
        );
        assert_eq!(repeat, true, "Repeat flag should be set");
        assert_eq!(s, 3, "Source advanced by 3 bytes");

        // Verify this matches our expected 102,310
        assert_eq!(length, 102310, "Should match our test case length");
        Ok(())
    }

    #[test]
    fn test_decode_copy1_basic() -> Result<()> {
        // Test basic Copy1 operation
        let tag = 0x01 | (5 << 2) | (2 << 6); // tag=1, length_bits=5, offset_low=2
        let src = vec![10]; // offset_high=10
        let mut s = 0;

        let (offset, length) = decode_copy1_safe(&src, &mut s, tag)?;

        // offset = ((10 << 2) | 2) + 1 = 40 + 2 + 1 = 43
        let expected_offset = ((10 << 2) | 2) + 1;
        assert_eq!(offset, expected_offset, "Copy1 offset calculation");
        assert_eq!(length, COPY1_BASE_LENGTH + 5, "Copy1 length = 4 + 5 = 9");
        assert_eq!(s, 1, "Source advanced by 1 byte");
        Ok(())
    }

    #[test]
    fn test_decode_copy1_extended() -> Result<()> {
        // Test Copy1 with extended length
        let tag = 0x01 | (15 << 2) | (1 << 6); // tag=1, length_bits=15 (extended), offset_low=1
        let src = vec![20, 50]; // offset_high=20, extra_len=50
        let mut s = 0;

        let (offset, length) = decode_copy1_safe(&src, &mut s, tag)?;

        let expected_offset = ((20 << 2) | 1) + 1;
        assert_eq!(offset, expected_offset, "Copy1 extended offset");
        assert_eq!(
            length,
            COPY1_EXTENDED_BASE + 50,
            "Copy1 extended length = 18 + 50"
        );
        assert_eq!(s, 2, "Source advanced by 2 bytes for extended");
        Ok(())
    }

    #[test]
    fn test_decode_copy2_basic() -> Result<()> {
        // Test basic Copy2 operation
        let tag = 0x02 | (10 << 2); // tag=2, length_val=10
        let src = vec![0x34, 0x12]; // Little-endian offset 0x1234 = 4660
        let mut s = 0;

        let (offset, length) = decode_copy2_safe(&src, &mut s, tag)?;

        assert_eq!(offset, 4660 + MIN_COPY2_OFFSET, "Copy2 offset = 4660 + 64");
        assert_eq!(length, COPY2_BASE_LENGTH + 10, "Copy2 length = 4 + 10");
        assert_eq!(s, 2, "Source advanced by 2 bytes");
        Ok(())
    }

    #[test]
    fn test_decode_copy2_1_byte_extension() -> Result<()> {
        // Test Copy2 with 1-byte length extension
        let tag = 0x02 | (COPY2_LENGTH_1_BYTE << 2);
        let src = vec![0x78, 0x56, 100]; // offset + extension
        let mut s = 0;

        let (offset, length) = decode_copy2_safe(&src, &mut s, tag)?;

        assert_eq!(offset, 0x5678 + MIN_COPY2_OFFSET, "Copy2 1-byte ext offset");
        assert_eq!(length, COPY2_EXTENDED_BASE + 100, "Copy2 1-byte ext length");
        assert_eq!(s, 3, "Source advanced by 3 bytes");
        Ok(())
    }

    #[test]
    fn test_decode_copy2_2_byte_extension() -> Result<()> {
        // Test Copy2 with 2-byte length extension
        let tag = 0x02 | (COPY2_LENGTH_2_BYTE << 2);
        let src = vec![0x78, 0x56, 0x34, 0x12]; // offset + 2-byte extension
        let mut s = 0;

        let (offset, length) = decode_copy2_safe(&src, &mut s, tag)?;

        assert_eq!(offset, 0x5678 + MIN_COPY2_OFFSET, "Copy2 2-byte ext offset");
        assert_eq!(
            length,
            COPY2_EXTENDED_BASE + 0x1234,
            "Copy2 2-byte ext length"
        );
        assert_eq!(s, 4, "Source advanced by 4 bytes");
        Ok(())
    }

    #[test]
    fn test_decode_copy2_3_byte_extension() -> Result<()> {
        // Test Copy2 with 3-byte length extension
        let tag = 0x02 | (COPY2_LENGTH_3_BYTE << 2);
        let src = vec![0x78, 0x56, 0x04, 0x78, 0x8f]; // offset + 3-byte extension
        let mut s = 0;

        let (offset, length) = decode_copy2_safe(&src, &mut s, tag)?;

        assert_eq!(offset, 0x5678 + MIN_COPY2_OFFSET, "Copy2 3-byte ext offset");
        // 3-byte extension: 0x04 | (0x78 << 8) | (0x8f << 16) with 4th byte as 0
        let expected_ext = 0x04 | (0x78 << 8) | (0x8f << 16);
        assert_eq!(
            length,
            COPY2_EXTENDED_BASE + expected_ext,
            "Copy2 3-byte ext length"
        );
        assert_eq!(s, 5, "Source advanced by 5 bytes");
        Ok(())
    }

    #[test]
    fn test_decode_fused_copy2() -> Result<()> {
        // Test fused Copy2 operation
        let tag = 0x03 | (2 << 3) | (5 << 5); // tag=3, lit_len=2+1=3, copy_len=5+4=9
        let src = vec![0x78, 0x56]; // Little-endian offset
        let mut s = 0;

        let (offset, copy_length, lit_len) = decode_fused_copy2_safe(&src, &mut s, tag)?;

        assert_eq!(offset, 0x5678 + MIN_COPY2_OFFSET, "Fused Copy2 offset");
        assert_eq!(copy_length, 9, "Fused Copy2 copy length = 5 + 4");
        assert_eq!(lit_len, 3, "Fused Copy2 literal length = 2 + 1");
        assert_eq!(s, 2, "Source advanced by 2 bytes");
        Ok(())
    }

    #[test]
    fn test_decode_copy3_basic() -> Result<()> {
        // Test basic Copy3 operation (no length extension)
        let tag = 0x07 | (1 << 3) | (30 << 5); // tag=7, lit_len=1, length_val=30
                                               // Construct 4-byte value: tag + 3 offset bytes
                                               // We need 21-bit offset, so use a reasonable value
        let offset_21bit = 0x12345; // 21-bit offset
        let val = (tag as u32) | (30u32 << 5) | (offset_21bit << 11);
        let bytes = val.to_le_bytes();
        let src = vec![bytes[0], bytes[1], bytes[2], bytes[3]]; // Include tag byte at position 0
        let mut s = 1; // Position after tag

        let (offset, length, lit_len) = decode_copy3_safe(&src, &mut s, tag)?;

        assert_eq!(
            offset,
            offset_21bit as usize + MIN_COPY3_OFFSET,
            "Copy3 offset"
        );
        assert_eq!(
            length,
            COPY3_BASE_LENGTH + 30,
            "Copy3 basic length = 4 + 30"
        );
        assert_eq!(lit_len, 1, "Copy3 literal length");
        assert_eq!(s, 4, "Source advanced by 3 bytes");
        Ok(())
    }

    #[test]
    fn test_decode_copy3_1_byte_extension() -> Result<()> {
        // Test Copy3 with 1-byte length extension
        let tag = 0x07 | (2 << 3) | (COPY3_LENGTH_1_BYTE << 5);
        let offset_21bit = 0x1ABCD;
        let val = (tag as u32) | ((COPY3_LENGTH_1_BYTE as u32) << 5) | (offset_21bit << 11);
        let bytes = val.to_le_bytes();
        let src = vec![bytes[0], bytes[1], bytes[2], bytes[3], 200]; // Include tag + extension byte
        let mut s = 1;

        let (offset, length, lit_len) = decode_copy3_safe(&src, &mut s, tag)?;

        assert_eq!(
            offset,
            offset_21bit as usize + MIN_COPY3_OFFSET,
            "Copy3 1-byte ext offset"
        );
        assert_eq!(length, COPY3_EXTENDED_BASE + 200, "Copy3 1-byte ext length");
        assert_eq!(lit_len, 2, "Copy3 literal length");
        assert_eq!(s, 5, "Source advanced by 4 bytes");
        Ok(())
    }

    #[test]
    fn test_decode_copy3_2_byte_extension() -> Result<()> {
        // Test Copy3 with 2-byte length extension
        let tag = 0x07 | (0 << 3) | (COPY3_LENGTH_2_BYTE << 5);
        let offset_21bit = 0x15432;
        let val = (tag as u32) | ((COPY3_LENGTH_2_BYTE as u32) << 5) | (offset_21bit << 11);
        let bytes = val.to_le_bytes();
        let src = vec![bytes[0], bytes[1], bytes[2], bytes[3], 0x34, 0x12]; // Include tag + 2-byte extension
        let mut s = 1;

        let (offset, length, lit_len) = decode_copy3_safe(&src, &mut s, tag)?;

        assert_eq!(
            offset,
            offset_21bit as usize + MIN_COPY3_OFFSET,
            "Copy3 2-byte ext offset"
        );
        assert_eq!(
            length,
            COPY3_EXTENDED_BASE + 0x1234,
            "Copy3 2-byte ext length"
        );
        assert_eq!(lit_len, 0, "Copy3 literal length");
        assert_eq!(s, 6, "Source advanced by 5 bytes");
        Ok(())
    }

    #[test]
    fn test_decode_copy3_3_byte_extension() -> Result<()> {
        // Test Copy3 with 3-byte length extension
        let tag = 0x07 | (1 << 3) | (COPY3_LENGTH_3_BYTE << 5);
        let offset_21bit = 0x15432;
        let val = (tag as u32) | ((COPY3_LENGTH_3_BYTE as u32) << 5) | (offset_21bit << 11);
        let bytes = val.to_le_bytes();
        let src = vec![bytes[0], bytes[1], bytes[2], bytes[3], 0x78, 0x56, 0x34]; // Include tag + 3-byte extension
        let mut s = 1;

        let (offset, length, lit_len) = decode_copy3_safe(&src, &mut s, tag)?;

        assert_eq!(
            offset,
            offset_21bit as usize + MIN_COPY3_OFFSET,
            "Copy3 3-byte ext offset"
        );
        assert_eq!(
            length,
            COPY3_EXTENDED_BASE + 0x345678,
            "Copy3 3-byte ext length"
        );
        assert_eq!(lit_len, 1, "Copy3 literal length");
        assert_eq!(s, 7, "Source advanced by 6 bytes");
        Ok(())
    }

    #[test]
    fn test_decode_copy3_boundary_lengths() -> Result<()> {
        // Test boundary values for Copy3 length encoding
        let test_cases = vec![
            (0u8, COPY3_BASE_LENGTH),                   // Minimum: 4 + 0 = 4 bytes
            (60u8, COPY3_BASE_LENGTH),                  // Maximum direct: 4 + 60 = 64 bytes
            (COPY3_LENGTH_1_BYTE, COPY3_EXTENDED_BASE), // Start of 1-byte extension: 64 + 0 = 64
            (COPY3_LENGTH_2_BYTE, COPY3_EXTENDED_BASE), // Start of 2-byte extension: 64 + 0 = 64
            (COPY3_LENGTH_3_BYTE, COPY3_EXTENDED_BASE), // Start of 3-byte extension: 64 + 0 = 64
        ];

        for (length_val, expected_base) in test_cases {
            let tag = 0x07 | (length_val << 5);
            let offset_21bit = 0x10000; // Valid 21-bit offset
            let val = (tag as u32) | ((length_val as u32) << 5) | (offset_21bit << 11);
            let bytes = val.to_le_bytes();

            let mut src = vec![bytes[0], bytes[1], bytes[2], bytes[3]];

            // Add extension bytes for extension cases
            if length_val >= COPY3_LENGTH_1_BYTE {
                src.extend_from_slice(&[0, 0, 0]); // Extension bytes
            }

            let mut s = 1;
            let (offset, length, _) = decode_copy3_safe(&src, &mut s, tag)?;

            assert_eq!(
                offset,
                offset_21bit as usize + MIN_COPY3_OFFSET,
                "Offset for length_val {}",
                length_val
            );
            if length_val <= COPY3_LENGTH_MAX_DIRECT {
                assert_eq!(
                    length,
                    expected_base + length_val as usize,
                    "Direct length for length_val {}",
                    length_val
                );
            } else {
                assert_eq!(
                    length, expected_base,
                    "Extension base for length_val {}",
                    length_val
                );
            }
        }
        Ok(())
    }

    #[test]
    fn test_large_values_handling() -> Result<()> {
        // Test that decoder handles large but valid values correctly

        // Test maximum 3-byte literal extension
        let tag = 31 << 3;
        let src = vec![0xFF, 0xFF, 0xFF]; // Maximum 3-byte value
        let mut s = 0;
        let (length, _) = decode_literal_header(&src, &mut s, tag)?;
        assert_eq!(length, 30 + 0xFFFFFF, "Maximum 3-byte literal length");

        // Test maximum Copy3 offset (21 bits)
        let max_offset_21bit = (1 << 21) - 1; // 0x1FFFFF
        let tag = 0x07;
        let val = (tag as u32) | (max_offset_21bit << 11);
        let bytes = val.to_le_bytes();
        let src = vec![bytes[0], bytes[1], bytes[2], bytes[3]]; // Include tag
        let mut s = 1;
        let (offset, _, _) = decode_copy3_safe(&src, &mut s, tag)?;
        assert_eq!(
            offset,
            max_offset_21bit as usize + MIN_COPY3_OFFSET,
            "Maximum Copy3 offset"
        );

        Ok(())
    }
}
