//! Variable-length integer encoding/decoding compatible with Go's binary.Uvarint

use crate::error::{Error, Result};

/// Maximum number of bytes in a varint-encoded value
const MAX_VARINT_LEN: usize = 10;

/// Encode an unsigned 64-bit integer as a varint and append to a Vec
pub fn encode_uvarint_vec(dst: &mut Vec<u8>, mut value: u64) -> Result<usize> {
    let start_len = dst.len();

    while value >= 0x80 {
        dst.push((value as u8) | 0x80);
        value >>= 7;
    }
    dst.push(value as u8);

    Ok(dst.len() - start_len)
}

/// Encode an unsigned 64-bit integer as a varint and return bytes written
pub fn encode_uvarint(dst: &mut [u8], mut value: u64) -> Result<usize> {
    let mut i = 0;
    while value >= 0x80 {
        if i >= dst.len() {
            return Err(Error::Corrupt);
        }
        dst[i] = (value as u8) | 0x80;
        value >>= 7;
        i += 1;
    }
    if i >= dst.len() {
        return Err(Error::Corrupt);
    }
    dst[i] = value as u8;
    Ok(i + 1)
}

/// Decode a varint from the beginning of src
/// Returns (value, bytes_consumed) or an error
pub fn decode_uvarint(src: &[u8]) -> Result<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0;

    for (i, &byte) in src.iter().enumerate() {
        if i >= MAX_VARINT_LEN {
            return Err(Error::Corrupt);
        }

        if shift >= 64 {
            return Err(Error::Corrupt);
        }

        let byte_value = (byte & 0x7F) as u64;

        // Check for overflow
        if shift == 63 && byte_value > 1 {
            return Err(Error::Corrupt);
        }

        value |= byte_value << shift;

        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }

        shift += 7;
    }

    Err(Error::Corrupt)
}

/// Calculate the number of bytes needed to encode a varint value
#[allow(dead_code)]
pub fn varint_len(mut value: u64) -> usize {
    if value == 0 {
        return 1;
    }

    let mut len = 0;
    while value > 0 {
        len += 1;
        value >>= 7;
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_vec() {
        let mut buf = Vec::new();
        let len = encode_uvarint_vec(&mut buf, 300).unwrap();
        assert_eq!(len, 2);
        assert_eq!(buf, vec![0xAC, 0x02]); // 300 in varint format
    }

    #[test]
    fn test_varint_roundtrip() {
        let test_values = [
            0,
            1,
            127,
            128,
            255,
            256,
            16383,
            16384,
            u64::MAX - 1,
            u64::MAX,
        ];

        for &value in &test_values {
            let mut buf = vec![0u8; MAX_VARINT_LEN];
            let encoded_len = encode_uvarint(&mut buf, value).unwrap();
            let (decoded_value, decoded_len) = decode_uvarint(&buf).unwrap();

            assert_eq!(value, decoded_value);
            assert_eq!(encoded_len, decoded_len);
            assert_eq!(encoded_len, varint_len(value));
        }
    }

    #[test]
    fn test_varint_overflow() {
        // Test buffer too small
        let mut small_buf = [0u8; 1];
        assert!(encode_uvarint(&mut small_buf, 128).is_err());

        // Test malformed varint (all bytes have continuation bit)
        let bad_varint = [0x80; MAX_VARINT_LEN + 1];
        assert!(decode_uvarint(&bad_varint).is_err());
    }
}
