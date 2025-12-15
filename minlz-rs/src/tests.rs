//! Basic tests for the MinLZ implementation

#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn test_max_encoded_len() {
        assert_eq!(max_encoded_len(0), Some(1));
        assert_eq!(max_encoded_len(100), Some(102));
        assert_eq!(max_encoded_len(MAX_BLOCK_SIZE), Some(MAX_BLOCK_SIZE + 2));
        assert_eq!(max_encoded_len(MAX_BLOCK_SIZE + 1), None);
    }

    #[test]
    fn test_varint_roundtrip() {
        use crate::varint::{encode_uvarint, decode_uvarint};

        let test_values = [0, 1, 127, 128, 255, 256, 16383, 16384, u64::MAX];

        for &value in &test_values {
            let mut buf = vec![0u8; 10];
            let encoded_len = encode_uvarint(&mut buf, value).unwrap();
            let (decoded_value, decoded_len) = decode_uvarint(&buf).unwrap();

            assert_eq!(value, decoded_value);
            assert_eq!(encoded_len, decoded_len);
        }
    }

    #[test]
    fn test_memory_operations() {
        use crate::memory::*;

        let data = vec![0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];

        assert_eq!(load8(&data, 0).unwrap(), 0x12);
        assert_eq!(load16(&data, 0).unwrap(), 0x3412); // little-endian
        assert_eq!(load32(&data, 0).unwrap(), 0x78563412); // little-endian

        let mut output = vec![0u8; 8];
        store8(&mut output, 0, 0xAB).unwrap();
        store16(&mut output, 1, 0xCDEF).unwrap();

        assert_eq!(output[0], 0xAB);
        assert_eq!(output[1], 0xEF); // little-endian
        assert_eq!(output[2], 0xCD); // little-endian
    }

    #[test]
    fn test_uncompressed_encode() {
        let src = b"Hello, world!";
        let mut dst = Vec::new();

        encode(&mut dst, src, LEVEL_FASTEST).unwrap();

        // Should be encoded as uncompressed since it's too small
        assert_eq!(dst[0], 0); // MinLZ indicator
        assert_eq!(dst[1], 0); // Zero length indicates literals
        assert_eq!(&dst[2..], src);
    }

    #[test]
    fn test_compression_ratios_repetitive_patterns() {
        // Test compression ratios on deterministic repetitive patterns

        // Test 1: Array of zeros
        let zeros = vec![0u8; 1000];
        for level in [LEVEL_FASTEST, LEVEL_BALANCED, LEVEL_SMALLEST] {
            let mut encoded = Vec::new();
            encode(&mut encoded, &zeros, level).unwrap();
            let compression_ratio = encoded.len() as f64 / zeros.len() as f64;
            assert!(compression_ratio < 0.1,
                "Poor compression ratio {:.2} for {} zeros at level {}",
                compression_ratio, zeros.len(), level);
        }

        // Test 2: Repeating pattern "ABCD"
        let pattern = b"ABCD";
        let repeated = pattern.repeat(250); // 1000 bytes
        for level in [LEVEL_BALANCED, LEVEL_SMALLEST] {
            let mut encoded = Vec::new();
            encode(&mut encoded, &repeated, level).unwrap();
            let compression_ratio = encoded.len() as f64 / repeated.len() as f64;
            assert!(compression_ratio < 0.3,
                "Poor compression ratio {:.2} for ABCD pattern at level {}",
                compression_ratio, level);
        }

        // Test 3: Single byte repeated
        for &byte_value in &[0x00, 0xFF, 0x55] {
            let single_byte_pattern = vec![byte_value; 1000];
            for level in [LEVEL_BALANCED, LEVEL_SMALLEST] {
                let mut encoded = Vec::new();
                encode(&mut encoded, &single_byte_pattern, level).unwrap();
                let compression_ratio = encoded.len() as f64 / single_byte_pattern.len() as f64;
                assert!(compression_ratio < 0.1,
                    "Poor compression ratio {:.2} for byte 0x{:02X} pattern at level {}",
                    compression_ratio, byte_value, level);
            }
        }
    }

    #[test]
    fn test_compression_ratios_structured_data() {
        // Test compression on JSON-like structured data
        let json_like = br#"{"name":"test","value":123,"data":[1,2,3,4,5],"nested":{"key":"value"}}"#
            .repeat(50); // ~3500 bytes

        for level in [LEVEL_FASTEST, LEVEL_BALANCED, LEVEL_SMALLEST] {
            let mut encoded = Vec::new();
            if let Ok(_) = encode(&mut encoded, &json_like, level) {
                let compression_ratio = encoded.len() as f64 / json_like.len() as f64;
                // JSON-like data should compress reasonably well due to repetitive structure
                assert!(compression_ratio < 0.8,
                    "Poor compression ratio {:.2} for JSON-like data at level {}",
                    compression_ratio, level);
            }
        }
    }
}