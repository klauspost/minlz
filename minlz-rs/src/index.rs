//! Index support for MinLZ streams
//!
//! This module implements stream indexing compatible with the Go implementation,
//! allowing for efficient seeking and random access within compressed streams.

use crate::{varint, Error, Result};

/// Index header magic bytes
pub const INDEX_HEADER: &[u8] = b"s2idx\x00";

/// Index trailer magic bytes
pub const _INDEX_TRAILER: &[u8] = b"\x00xdi2s";

/// Maximum number of entries in an index
pub const MAX_INDEX_ENTRIES: usize = 1 << 16;

/// Minimum uncompressed distance between index entries (1MB)
pub const MIN_INDEX_DISTANCE: u64 = 1 << 20;

/// Represents an offset pair for index entries
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffsetPair {
    /// Compressed stream offset
    pub compressed_offset: u64,
    /// Uncompressed data offset
    pub uncompressed_offset: u64,
}

/// Index for MinLZ streams enabling efficient seeking
#[derive(Debug, Clone)]
pub struct Index {
    /// Total uncompressed size of the stream
    pub total_uncompressed: u64,

    /// Total compressed size if known (-1 if unknown)
    pub total_compressed: i64,

    /// Offset pairs for seeking (compressed -> uncompressed positions)
    pub offsets: Vec<OffsetPair>,

    /// Estimated block size used for guessing uncompressed offsets
    estimated_block_size: u64,
}

impl Index {
    /// Create a new index with the given block size
    pub fn new(block_size: usize) -> Self {
        Self {
            total_uncompressed: 0,
            total_compressed: -1,
            offsets: Vec::new(),
            estimated_block_size: block_size as u64,
        }
    }

    /// Reset the index for reuse
    pub fn reset(&mut self, block_size: usize) {
        self.total_uncompressed = 0;
        self.total_compressed = -1;
        self.offsets.clear();
        self.estimated_block_size = block_size as u64;
    }

    /// Add a block to the index
    pub fn add_block(&mut self, compressed_offset: u64, uncompressed_offset: u64) -> Result<()> {
        // Only add entries at sufficient distance intervals to keep index manageable
        if !self.offsets.is_empty() {
            let last_uncompressed = self.offsets.last().unwrap().uncompressed_offset;
            if uncompressed_offset - last_uncompressed < MIN_INDEX_DISTANCE {
                return Ok(());
            }
        }

        // Prevent index from becoming too large
        if self.offsets.len() >= MAX_INDEX_ENTRIES {
            return Ok(());
        }

        self.offsets.push(OffsetPair {
            compressed_offset,
            uncompressed_offset,
        });

        Ok(())
    }

    /// Serialize the index to bytes
    pub fn serialize(&self, total_uncompressed: u64, total_compressed: Option<u64>) -> Vec<u8> {
        let mut buf = Vec::new();

        // Write header
        buf.extend_from_slice(INDEX_HEADER);

        // Total uncompressed size
        let _ = varint::encode_uvarint_vec(&mut buf, total_uncompressed);

        // Total compressed size (-1 if unknown)
        let compressed_size = total_compressed.map(|s| s as i64).unwrap_or(-1);
        let _ = varint::encode_uvarint_vec(&mut buf, zigzag_encode(compressed_size));

        // Estimated block size
        let _ = varint::encode_uvarint_vec(&mut buf, self.estimated_block_size);

        // Number of entries
        let _ = varint::encode_uvarint_vec(&mut buf, self.offsets.len() as u64);

        // Has uncompressed offsets flag
        buf.push(if self.offsets.is_empty() { 0 } else { 1 });

        if !self.offsets.is_empty() {
            // Write uncompressed offsets (delta-encoded)
            let mut last_uncompressed = 0u64;
            for pair in &self.offsets {
                let delta = pair.uncompressed_offset - last_uncompressed;
                let _ = varint::encode_uvarint_vec(&mut buf, delta);
                last_uncompressed = pair.uncompressed_offset;
            }

            // Write compressed offsets (delta-encoded)
            let mut last_compressed = 0u64;
            for pair in &self.offsets {
                let delta = pair.compressed_offset - last_compressed;
                let _ = varint::encode_uvarint_vec(&mut buf, delta);
                last_compressed = pair.compressed_offset;
            }
        }

        buf
    }

    /// Deserialize an index from bytes
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        if data.len() < INDEX_HEADER.len() {
            return Err(Error::Corrupt);
        }

        if &data[..INDEX_HEADER.len()] != INDEX_HEADER {
            return Err(Error::Corrupt);
        }

        let mut offset = INDEX_HEADER.len();

        // Read total uncompressed size
        let (total_uncompressed, len) = varint::decode_uvarint(&data[offset..])?;
        offset += len;

        // Read total compressed size
        let (compressed_encoded, len) = varint::decode_uvarint(&data[offset..])?;
        offset += len;
        let total_compressed = zigzag_decode(compressed_encoded);

        // Read estimated block size
        let (estimated_block_size, len) = varint::decode_uvarint(&data[offset..])?;
        offset += len;

        // Read number of entries
        let (entries_count, len) = varint::decode_uvarint(&data[offset..])?;
        offset += len;

        if entries_count > MAX_INDEX_ENTRIES as u64 {
            return Err(Error::Corrupt);
        }

        // Read has uncompressed offsets flag
        if offset >= data.len() {
            return Err(Error::Corrupt);
        }
        let has_uncompressed_offsets = data[offset] != 0;
        offset += 1;

        let mut offsets = Vec::new();

        if has_uncompressed_offsets && entries_count > 0 {
            let mut uncompressed_offsets = Vec::new();
            let mut compressed_offsets = Vec::new();

            // Read uncompressed offsets (delta-encoded)
            let mut current_uncompressed = 0u64;
            for _ in 0..entries_count {
                let (delta, len) = varint::decode_uvarint(&data[offset..])?;
                offset += len;
                current_uncompressed += delta;
                uncompressed_offsets.push(current_uncompressed);
            }

            // Read compressed offsets (delta-encoded)
            let mut current_compressed = 0u64;
            for _ in 0..entries_count {
                let (delta, len) = varint::decode_uvarint(&data[offset..])?;
                offset += len;
                current_compressed += delta;
                compressed_offsets.push(current_compressed);
            }

            // Combine into offset pairs
            for (uncompressed, compressed) in uncompressed_offsets.into_iter().zip(compressed_offsets) {
                offsets.push(OffsetPair {
                    compressed_offset: compressed,
                    uncompressed_offset: uncompressed,
                });
            }
        }

        Ok(Index {
            total_uncompressed,
            total_compressed,
            offsets,
            estimated_block_size,
        })
    }

    /// Find the best offset pair for seeking to the given uncompressed position
    pub fn find_offset(&self, uncompressed_pos: u64) -> Option<OffsetPair> {
        if self.offsets.is_empty() {
            return None;
        }

        // Binary search for the best entry
        match self.offsets.binary_search_by(|entry| {
            entry.uncompressed_offset.cmp(&uncompressed_pos)
        }) {
            Ok(idx) => Some(self.offsets[idx]),
            Err(idx) => {
                if idx == 0 {
                    // Position is before first entry
                    None
                } else {
                    // Use the previous entry (largest offset <= position)
                    Some(self.offsets[idx - 1])
                }
            }
        }
    }
}

/// Encode a signed integer using zigzag encoding
fn zigzag_encode(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

/// Decode a zigzag-encoded integer
fn zigzag_decode(encoded: u64) -> i64 {
    ((encoded >> 1) as i64) ^ (-((encoded & 1) as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_new() {
        let index = Index::new(1024 * 1024);
        assert_eq!(index.total_uncompressed, 0);
        assert_eq!(index.total_compressed, -1);
        assert_eq!(index.estimated_block_size, 1024 * 1024);
        assert!(index.offsets.is_empty());
    }

    #[test]
    fn test_index_add_block() {
        let mut index = Index::new(1024 * 1024);

        // Add first block
        index.add_block(100, 0).unwrap();
        assert_eq!(index.offsets.len(), 1);
        assert_eq!(index.offsets[0].compressed_offset, 100);
        assert_eq!(index.offsets[0].uncompressed_offset, 0);

        // Add block too close (should be ignored due to MIN_INDEX_DISTANCE)
        index.add_block(200, 1000).unwrap();
        assert_eq!(index.offsets.len(), 1);

        // Add block far enough away
        index.add_block(2000, MIN_INDEX_DISTANCE + 1).unwrap();
        assert_eq!(index.offsets.len(), 2);
    }

    #[test]
    fn test_index_serialization_empty() {
        let index = Index::new(1024 * 1024);
        let serialized = index.serialize(0, None);

        let deserialized = Index::deserialize(&serialized).unwrap();
        assert_eq!(deserialized.total_uncompressed, 0);
        assert_eq!(deserialized.total_compressed, -1);
        assert_eq!(deserialized.estimated_block_size, 1024 * 1024);
        assert!(deserialized.offsets.is_empty());
    }

    #[test]
    fn test_index_serialization_with_data() {
        let mut index = Index::new(1024 * 1024);
        index.add_block(100, 0).unwrap();
        index.add_block(2000, MIN_INDEX_DISTANCE + 1).unwrap();

        let serialized = index.serialize(MIN_INDEX_DISTANCE * 2, Some(3000));

        let deserialized = Index::deserialize(&serialized).unwrap();
        assert_eq!(deserialized.total_uncompressed, MIN_INDEX_DISTANCE * 2);
        assert_eq!(deserialized.total_compressed, 3000);
        assert_eq!(deserialized.offsets.len(), 2);
        assert_eq!(deserialized.offsets[0].compressed_offset, 100);
        assert_eq!(deserialized.offsets[1].compressed_offset, 2000);
    }

    #[test]
    fn test_find_offset() {
        let mut index = Index::new(1024);
        index.add_block(100, 0).unwrap();
        index.add_block(2000, MIN_INDEX_DISTANCE).unwrap();
        index.add_block(4000, MIN_INDEX_DISTANCE * 2).unwrap();

        // Exact match
        assert_eq!(index.find_offset(MIN_INDEX_DISTANCE).unwrap().compressed_offset, 2000);

        // Between entries (should return previous)
        assert_eq!(index.find_offset(MIN_INDEX_DISTANCE + 100).unwrap().compressed_offset, 2000);

        // At first entry position
        assert_eq!(index.find_offset(0).unwrap().compressed_offset, 100);

        // After last entry
        assert_eq!(index.find_offset(MIN_INDEX_DISTANCE * 3).unwrap().compressed_offset, 4000);
    }

    #[test]
    fn test_zigzag_encoding() {
        let test_values = [0i64, -1, 1, -2, 2, i64::MIN, i64::MAX];

        for &value in &test_values {
            let encoded = zigzag_encode(value);
            let decoded = zigzag_decode(encoded);
            assert_eq!(value, decoded, "Zigzag round-trip failed for {}", value);
        }
    }

    #[test]
    fn test_index_reset() {
        let mut index = Index::new(1024);
        index.add_block(100, 0).unwrap();
        index.total_uncompressed = 1000;
        index.total_compressed = 500;

        index.reset(2048);

        assert_eq!(index.total_uncompressed, 0);
        assert_eq!(index.total_compressed, -1);
        assert_eq!(index.estimated_block_size, 2048);
        assert!(index.offsets.is_empty());
    }

    #[test]
    fn test_max_index_entries() {
        let mut index = Index::new(1024);

        // Try to add more than maximum entries
        for i in 0..(MAX_INDEX_ENTRIES + 100) {
            let offset = (i as u64) * MIN_INDEX_DISTANCE;
            index.add_block(offset / 2, offset).unwrap();
        }

        // Should be capped at maximum
        assert!(index.offsets.len() <= MAX_INDEX_ENTRIES);
    }
}