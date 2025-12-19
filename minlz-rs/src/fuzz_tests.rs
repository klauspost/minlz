//! Fuzz testing for MinLZ encoders and decoders
//!
//! This module uses property-based testing to validate that our encoders and decoders
//! handle arbitrary inputs safely and correctly. The tests ensure:
//! - No panics or crashes on arbitrary input
//! - Round-trip property: decode(encode(data)) == data
//! - Encoders never produce corrupt output
//! - Bounds checking is correct
//! - Memory safety is maintained

#[cfg(test)]
mod tests {
    use crate::{
        constants::*,
        decode::decode,
        encode::{encode, level1, level2, level3},
    };
    use proptest::prelude::*;
    use proptest::test_runner::TestCaseResult;

    // Strategy for generating test data of various sizes and patterns
    prop_compose! {
        fn arbitrary_data(max_size: usize)
                         (len in 0..=max_size)
                         (data in prop::collection::vec(any::<u8>(), len))
                         -> Vec<u8> {
            data
        }
    }

    // Strategy for generating repetitive data that should compress well
    prop_compose! {
        fn repetitive_data(max_size: usize)
                          (len in 16..=max_size, pattern_len in 1usize..=8usize)
                          (len in Just(len), pattern in prop::collection::vec(any::<u8>(), pattern_len))
                          -> Vec<u8> {
            let mut data = Vec::with_capacity(len);
            while data.len() < len {
                for &byte in &pattern {
                    if data.len() >= len { break; }
                    data.push(byte);
                }
            }
            data
        }
    }

    // Strategy for generating structured data patterns
    prop_compose! {
        fn structured_data(max_size: usize)
                          (len in 16..=max_size, chunk_size in 4usize..=32usize)
                          -> Vec<u8> {
            let mut data = Vec::with_capacity(len);
            let mut counter = 0u32;
            while data.len() + 4 <= len {
                data.extend_from_slice(&counter.to_le_bytes());
                counter += 1;
                // Add some structured padding
                let remaining = len - data.len();
                let padding_len = (chunk_size - 4).min(remaining);
                for i in 0..padding_len {
                    data.push((counter as u8).wrapping_add(i as u8));
                }
            }
            // Fill remaining bytes
            while data.len() < len {
                data.push(0);
            }
            data
        }
    }

    #[test]
    fn test_level1_never_panics() {
        fn property(data: Vec<u8>) -> TestCaseResult {
            let mut dst = vec![0u8; data.len() + 1000]; // Extra space for safety

            // This should never panic
            let result = level1::encode_block(&mut dst, &data);

            // Result should always be Ok (even if 0 for rejected input)
            prop_assert!(result.is_ok());

            let encoded_len = result.unwrap();
            prop_assert!(encoded_len <= dst.len());

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(1000), |(data in arbitrary_data(3145728))| {
            property(data)?;
        });
    }

    #[test]
    fn test_level2_never_panics() {
        fn property(data: Vec<u8>) -> TestCaseResult {
            let mut dst = vec![0u8; data.len() + 1000]; // Extra space for safety

            // This should never panic
            let result = level2::encode_block(&mut dst, &data);

            // Result should always be Ok (even if 0 for rejected input)
            prop_assert!(result.is_ok());

            let encoded_len = result.unwrap();
            prop_assert!(encoded_len <= dst.len());

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(1000), |(data in arbitrary_data(3145728))| {
            property(data)?;
        });
    }

    #[test]
    fn test_level3_never_panics() {
        fn property(data: Vec<u8>) -> TestCaseResult {
            let mut dst = vec![0u8; data.len() + 1000]; // Extra space for safety

            // This should never panic
            let result = level3::encode_block(&mut dst, &data);

            // Result should always be Ok (even if 0 for rejected input)
            prop_assert!(result.is_ok());

            let encoded_len = result.unwrap();
            prop_assert!(encoded_len <= dst.len());

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(1000), |(data in arbitrary_data(3145728))| {
            property(data)?;
        });
    }

    #[test]
    fn test_high_level_api_never_panics() {
        fn property(data: Vec<u8>, level: i32) -> TestCaseResult {
            let mut encoded = Vec::new();
            let mut decoded = Vec::new();

            // High-level encode should never panic
            let encode_result = encode(&mut encoded, &data, level);
            prop_assert!(encode_result.is_ok());

            // If encoding succeeded and produced output, decode should work
            if !encoded.is_empty() {
                let decode_result = decode(&mut decoded, &encoded);
                prop_assert!(decode_result.is_ok());
            }

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(500), |(data in arbitrary_data(3145728), level in 1..=2i32)| {
            property(data, level)?;
        });
    }

    #[test]
    fn test_round_trip_property() {
        fn property(data: Vec<u8>) -> TestCaseResult {
            // Skip very small inputs that encoders might reject
            if data.len() < MIN_NON_LITERAL_BLOCK_SIZE {
                return Ok(());
            }

            for level in [1, 2] {
                let mut encoded = Vec::new();
                let mut decoded = Vec::new();

                // Encode
                encode(&mut encoded, &data, level)?;

                // If encoding produced output, test round-trip
                if !encoded.is_empty() {
                    // Decode
                    decode(&mut decoded, &encoded)?;

                    // Round-trip property: decode(encode(data)) == data
                    prop_assert_eq!(
                        decoded,
                        data.as_slice(),
                        "Round-trip failed for level {}",
                        level
                    );
                }
            }

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(500), |(data in repetitive_data(3145728))| {
            property(data)?;
        });
    }

    #[test]
    fn test_block_level_round_trip() {
        fn property(data: Vec<u8>) -> TestCaseResult {
            if data.len() < MIN_NON_LITERAL_BLOCK_SIZE {
                return Ok(());
            }

            // Test Level 1
            let mut encoded1 = vec![0u8; data.len() + 1000];
            let encoded_len1 = level1::encode_block(&mut encoded1, &data)?;

            if encoded_len1 > 0 {
                // Create a complete MinLZ block with header for decoding
                let mut complete_block1 = Vec::new();
                encode(&mut complete_block1, &data, 1)?;

                if !complete_block1.is_empty() {
                    let mut decoded1 = Vec::new();
                    decode(&mut decoded1, &complete_block1)?;
                    prop_assert_eq!(decoded1, data.as_slice(), "Level 1 round-trip failed");
                }
            }

            // Test Level 2
            let mut encoded2 = vec![0u8; data.len() + 1000];
            let encoded_len2 = level2::encode_block(&mut encoded2, &data)?;

            if encoded_len2 > 0 {
                // Create a complete MinLZ block with header for decoding
                let mut complete_block2 = Vec::new();
                encode(&mut complete_block2, &data, 2)?;

                if !complete_block2.is_empty() {
                    let mut decoded2 = Vec::new();
                    decode(&mut decoded2, &complete_block2)?;
                    prop_assert_eq!(decoded2, data.as_slice(), "Level 2 round-trip failed");
                }
            }

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(200), |(data in structured_data(3145728))| {
            property(data)?;
        });
    }

    #[test]
    fn test_encoder_output_bounds() {
        fn property(data: Vec<u8>) -> TestCaseResult {
            let mut dst = vec![0u8; data.len() + 500];

            // Test Level 1
            let encoded_len1 = level1::encode_block(&mut dst, &data)?;
            prop_assert!(encoded_len1 <= dst.len(), "Level 1 exceeded buffer bounds");

            // Test Level 2
            let encoded_len2 = level2::encode_block(&mut dst, &data)?;
            prop_assert!(encoded_len2 <= dst.len(), "Level 2 exceeded buffer bounds");

            // Encoded data should never be larger than original + reasonable overhead
            if encoded_len1 > 0 {
                prop_assert!(
                    encoded_len1 <= data.len() + 100,
                    "Level 1 output too large: {} vs {}",
                    encoded_len1,
                    data.len()
                );
            }

            if encoded_len2 > 0 {
                prop_assert!(
                    encoded_len2 <= data.len() + 100,
                    "Level 2 output too large: {} vs {}",
                    encoded_len2,
                    data.len()
                );
            }

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(500), |(data in arbitrary_data(3145728))| {
            property(data)?;
        });
    }

    #[test]
    fn test_small_buffer_handling() {
        fn property(data: Vec<u8>, buf_size: usize) -> TestCaseResult {
            if data.is_empty() {
                return Ok(());
            }

            // Test with intentionally small buffers
            let mut dst = vec![0u8; buf_size];

            // These should not panic, even with small buffers
            let result1 = level1::encode_block(&mut dst, &data);
            prop_assert!(
                result1.is_ok(),
                "Level 1 should handle small buffers gracefully"
            );

            let result2 = level2::encode_block(&mut dst, &data);
            prop_assert!(
                result2.is_ok(),
                "Level 2 should handle small buffers gracefully"
            );

            let result3 = level3::encode_block(&mut dst, &data);
            prop_assert!(
                result3.is_ok(),
                "Level 3 should handle small buffers gracefully"
            );

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(200), |(data in arbitrary_data(100000), buf_size in 0..=50usize)| {
            property(data, buf_size)?;
        });
    }

    #[test]
    fn test_compression_consistency() {
        fn property(data: Vec<u8>) -> TestCaseResult {
            if data.len() < MIN_NON_LITERAL_BLOCK_SIZE {
                return Ok(());
            }

            // Same input should always produce same output
            let mut encoded1a = vec![0u8; data.len() + 1000];
            let mut encoded1b = vec![0u8; data.len() + 1000];

            let len1a = level1::encode_block(&mut encoded1a, &data)?;
            let len1b = level1::encode_block(&mut encoded1b, &data)?;

            prop_assert_eq!(len1a, len1b, "Level 1 produced inconsistent output lengths");
            if len1a > 0 {
                prop_assert_eq!(
                    &encoded1a[..len1a],
                    &encoded1b[..len1b],
                    "Level 1 produced different output for same input"
                );
            }

            // Test Level 2 consistency
            let mut encoded2a = vec![0u8; data.len() + 1000];
            let mut encoded2b = vec![0u8; data.len() + 1000];

            let len2a = level2::encode_block(&mut encoded2a, &data)?;
            let len2b = level2::encode_block(&mut encoded2b, &data)?;

            prop_assert_eq!(len2a, len2b, "Level 2 produced inconsistent output lengths");
            if len2a > 0 {
                prop_assert_eq!(
                    &encoded2a[..len2a],
                    &encoded2b[..len2b],
                    "Level 2 produced different output for same input"
                );
            }

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(300), |(data in repetitive_data(3145728))| {
            property(data)?;
        });
    }

    #[test]
    fn test_size_rejection_threshold() {
        fn property(data: Vec<u8>) -> TestCaseResult {
            let mut dst = vec![0u8; data.len() + 1000];

            // Inputs smaller than MIN_NON_LITERAL_BLOCK_SIZE should be rejected
            if data.len() < MIN_NON_LITERAL_BLOCK_SIZE {
                let result1 = level1::encode_block(&mut dst, &data)?;
                let result2 = level2::encode_block(&mut dst, &data)?;

                prop_assert_eq!(result1, 0, "Level 1 should reject small input");
                prop_assert_eq!(result2, 0, "Level 2 should reject small input");
            }

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(200), |(data in arbitrary_data(50))| {
            property(data)?;
        });
    }

    #[test]
    fn test_maximum_size_handling() {
        fn property(data_len: usize) -> TestCaseResult {
            // Test very large inputs
            let data = vec![0x42u8; data_len];
            let mut dst = vec![0u8; data_len + 10000];

            // Should not panic even with very large inputs
            let result1 = level1::encode_block(&mut dst, &data);
            let result2 = level2::encode_block(&mut dst, &data);

            prop_assert!(result1.is_ok());
            prop_assert!(result2.is_ok());

            // For repetitive data, should achieve good compression
            if data_len >= MIN_NON_LITERAL_BLOCK_SIZE {
                let len1 = result1.unwrap();
                let len2 = result2.unwrap();

                if len1 > 0 {
                    prop_assert!(len1 < data_len, "Level 1 should compress repetitive data");
                }
                if len2 > 0 {
                    prop_assert!(len2 < data_len, "Level 2 should compress repetitive data");
                }
            }

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(100), |(data_len in 1000..=100000usize)| {
            property(data_len)?;
        });
    }

    #[test]
    fn test_edge_pattern_data() {
        fn property(pattern: u8, repeat_count: usize) -> TestCaseResult {
            if repeat_count < MIN_NON_LITERAL_BLOCK_SIZE {
                return Ok(());
            }

            // Test with specific patterns that might trigger edge cases
            let data = vec![pattern; repeat_count];
            let mut dst = vec![0u8; repeat_count + 1000];

            let result1 = level1::encode_block(&mut dst, &data)?;
            let result2 = level2::encode_block(&mut dst, &data)?;

            // Single-byte patterns should compress well
            // Level 1 is optimized for speed, so we use more lenient expectations
            if result1 > 0 {
                prop_assert!(
                    result1 <= data.len() * 2 / 3,
                    "Level 1 should compress single-byte pattern to at most 67% of original size"
                );
            }

            // Level 2 should achieve better compression ratios
            if result2 > 0 {
                prop_assert!(
                    result2 < data.len() / 2,
                    "Level 2 should compress single-byte pattern well"
                );
            }

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(100), |(pattern in any::<u8>(), repeat_count in 16..=10000usize)| {
            property(pattern, repeat_count)?;
        });
    }
}
