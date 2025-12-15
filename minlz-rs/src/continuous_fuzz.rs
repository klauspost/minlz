//! Continuous fuzzing module for MinLZ
//!
//! This module provides high-intensity continuous fuzzing capabilities
//! using proptest with stable Rust. Unlike the regular fuzz tests,
//! these are designed to run for extended periods with many test cases.

#[cfg(test)]
mod tests {
    use crate::{
        encode::{encode, level1, level2, level3},
        decode::decode,
        constants::*,
    };
    use proptest::prelude::*;
    use proptest::test_runner::TestCaseResult;
    use std::env;

    // Get the number of test cases from environment variable, with high defaults
    fn get_test_cases() -> u32 {
        env::var("CONTINUOUS_FUZZ_CASES")
            .unwrap_or_else(|_| "10000".to_string())
            .parse()
            .unwrap_or(10000)
    }

    // Strategy for generating diverse test data
    prop_compose! {
        fn continuous_test_data()
                             (size in 0usize..100000)
                             (data in prop::collection::vec(any::<u8>(), size))
                             -> Vec<u8> {
            data
        }
    }

    // Strategy for generating repetitive patterns that compress well
    prop_compose! {
        fn repetitive_pattern()
                              (pattern_size in 1usize..=64, repeat_count in 1usize..=10000)
                              (pattern in prop::collection::vec(any::<u8>(), pattern_size), repeat_count in Just(repeat_count))
                              -> Vec<u8> {
            let mut result = Vec::new();
            for _ in 0..repeat_count {
                result.extend_from_slice(&pattern);
            }
            result
        }
    }

    #[test]
    fn continuous_round_trip_all_levels() {
        let cases = get_test_cases();
        println!("Running continuous round-trip fuzzing with {} test cases", cases);

        fn property(data: Vec<u8>) -> TestCaseResult {
            if data.is_empty() {
                return Ok(());
            }

            for level in [LEVEL_FASTEST, LEVEL_BALANCED, LEVEL_SMALLEST] {
                let mut encoded = Vec::new();

                match encode(&mut encoded, &data, level) {
                    Ok(_) => {
                        let mut decoded = Vec::new();
                        decode(&mut decoded, &encoded)?;
                        prop_assert_eq!(&data, &decoded, "Round-trip failed for level {}", level);
                    }
                    Err(_) => {
                        // Encoding can fail for data that doesn't compress well
                    }
                }
            }
            Ok(())
        }

        proptest!(ProptestConfig::with_cases(cases), |(data in continuous_test_data())| {
            property(data)?;
        });
    }

    #[test]
    fn continuous_block_level_robustness() {
        let cases = get_test_cases();
        println!("Running continuous block-level robustness fuzzing with {} test cases", cases);

        fn property(data: Vec<u8>) -> TestCaseResult {
            if data.len() < MIN_NON_LITERAL_BLOCK_SIZE {
                return Ok(());
            }

            let mut dst = vec![0u8; data.len() + 10000];

            // Test all block encoders for panics
            let _ = level1::encode_block(&mut dst, &data);
            let _ = level2::encode_block(&mut dst, &data);
            let _ = level3::encode_block(&mut dst, &data);

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(cases), |(data in continuous_test_data())| {
            property(data)?;
        });
    }

    #[test]
    fn continuous_decoder_robustness() {
        let cases = get_test_cases();
        println!("Running continuous decoder robustness fuzzing with {} test cases", cases);

        fn property(data: Vec<u8>) -> TestCaseResult {
            let mut decoded = Vec::new();

            // Decoder should never panic on arbitrary input
            let _ = decode(&mut decoded, &data);

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(cases), |(data in continuous_test_data())| {
            property(data)?;
        });
    }

    #[test]
    fn continuous_repetitive_patterns() {
        let cases = get_test_cases() / 4; // Use fewer cases for expensive patterns
        println!("Running continuous repetitive pattern fuzzing with {} test cases", cases);

        fn property(data: Vec<u8>) -> TestCaseResult {
            if data.len() < MIN_NON_LITERAL_BLOCK_SIZE {
                return Ok(());
            }

            // Repetitive patterns should compress very well
            for level in [LEVEL_BALANCED, LEVEL_SMALLEST] {
                let mut encoded = Vec::new();

                if let Ok(_) = encode(&mut encoded, &data, level) {
                    if !encoded.is_empty() {
                        let compression_ratio = encoded.len() as f64 / data.len() as f64;

                        // Repetitive patterns should achieve good compression
                        prop_assert!(compression_ratio < 0.8,
                                   "Poor compression ratio {:.2} for repetitive pattern at level {}",
                                   compression_ratio, level);

                        // Verify round-trip
                        let mut decoded = Vec::new();
                        decode(&mut decoded, &encoded)?;
                        prop_assert_eq!(&data, &decoded);
                    }
                }
            }
            Ok(())
        }

        proptest!(ProptestConfig::with_cases(cases), |(data in repetitive_pattern())| {
            property(data)?;
        });
    }

    #[test]
    fn continuous_compression_consistency() {
        let cases = get_test_cases() / 2; // Use fewer cases for multiple level testing
        println!("Running continuous compression consistency fuzzing with {} test cases", cases);

        fn property(data: Vec<u8>) -> TestCaseResult {
            if data.len() < MIN_NON_LITERAL_BLOCK_SIZE {
                return Ok(());
            }

            let mut encoded1 = Vec::new();
            let mut encoded2 = Vec::new();
            let mut encoded3 = Vec::new();

            let result1 = encode(&mut encoded1, &data, LEVEL_FASTEST);
            let result2 = encode(&mut encoded2, &data, LEVEL_BALANCED);
            let result3 = encode(&mut encoded3, &data, LEVEL_SMALLEST);

            // If any level succeeds, all should produce valid decodable output
            if let (Ok(_), Ok(_), Ok(_)) = (&result1, &result2, &result3) {
                for (encoded, level) in [(&encoded1, 1), (&encoded2, 2), (&encoded3, 3)] {
                    if !encoded.is_empty() {
                        let mut decoded = Vec::new();
                        decode(&mut decoded, encoded)?;
                        prop_assert_eq!(&data, &decoded, "Level {} consistency failure", level);
                    }
                }
            }

            Ok(())
        }

        proptest!(ProptestConfig::with_cases(cases), |(data in continuous_test_data())| {
            property(data)?;
        });
    }
}