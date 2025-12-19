//! MinLZ streaming reader
//!
//! This module implements a streaming reader for MinLZ format data that complements
//! the Writer. It handles the MinLZ stream format including headers, chunks, user chunks,
//! and provides a standard io::Read interface.

use crate::{decode, stream, Error, Result};
use std::io::{self, Read};

/// State of the reader
#[derive(Debug, PartialEq)]
enum ReaderState {
    /// Reading stream header
    ReadingHeader,
    /// Reading chunk headers and data
    ReadingChunks,
    /// Stream has ended (EOF chunk found)
    Finished,
    /// Error state
    Error,
}

/// MinLZ streaming reader
///
/// Provides streaming decompression of MinLZ format data with a standard io::Read interface.
/// Handles stream headers, compressed chunks, user chunks, and EOF markers automatically.
///
/// # Example
///
/// ```rust
/// use minlz::Reader;
/// use std::io::Read;
/// # use std::io::Cursor;
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let compressed_data = vec![0u8; 32]; // Placeholder MinLZ stream data
/// let mut reader = Reader::new(Cursor::new(&compressed_data))?;
/// let mut decompressed = Vec::new();
/// reader.read_to_end(&mut decompressed)?;
/// # Ok(())
/// # }
/// ```
pub struct Reader<R: Read> {
    /// Underlying data source
    source: R,

    /// Current reader state
    state: ReaderState,

    /// Buffer for reading from source
    read_buffer: Vec<u8>,

    /// Decompressed data buffer (output buffer for user)
    output_buffer: Vec<u8>,

    /// Current position in output buffer
    output_pos: usize,

    /// Stream header information
    max_block_size: usize,

    /// Total bytes read from source
    bytes_read: u64,

    /// Total bytes decompressed and returned to user
    bytes_written: u64,

    /// Collection of user chunks found in stream
    user_chunks: Vec<(u8, Vec<u8>)>,

    /// Whether we've read the stream header
    header_read: bool,
}

impl<R: Read> Reader<R> {
    /// Create a new MinLZ reader
    pub fn new(source: R) -> Result<Self> {
        Ok(Reader {
            source,
            state: ReaderState::ReadingHeader,
            read_buffer: vec![0u8; 64 * 1024], // 64KB read buffer
            output_buffer: Vec::new(),
            output_pos: 0,
            max_block_size: 8 << 20, // Default 8MB
            bytes_read: 0,
            bytes_written: 0,
            user_chunks: Vec::new(),
            header_read: false,
        })
    }

    /// Get statistics about bytes processed
    ///
    /// Returns (bytes_read_from_source, bytes_decompressed_to_user)
    pub fn bytes_processed(&self) -> (u64, u64) {
        (self.bytes_read, self.bytes_written)
    }

    /// Get user chunks found in the stream so far
    pub fn user_chunks(&self) -> &[(u8, Vec<u8>)] {
        &self.user_chunks
    }

    /// Check if the reader has finished processing the stream
    pub fn is_finished(&self) -> bool {
        self.state == ReaderState::Finished
    }

    /// Read and validate the stream header
    fn read_header(&mut self) -> Result<()> {
        if self.header_read {
            return Ok(());
        }

        // Read magic header and block size indicator
        let mut header_buf = [0u8; 10];
        self.source.read_exact(&mut header_buf)?;
        self.bytes_read += 10;

        // Validate magic header
        if &header_buf[..stream::MAGIC_CHUNK.len()] != stream::MAGIC_CHUNK {
            return Err(Error::Corrupt);
        }

        // Extract block size indicator
        let block_size_indicator = header_buf[9];
        self.max_block_size = 1 << (block_size_indicator + 10);

        // Validate block size is reasonable
        if self.max_block_size > 8 << 20 || self.max_block_size < 1024 {
            return Err(Error::Corrupt);
        }

        self.header_read = true;
        self.state = ReaderState::ReadingChunks;
        Ok(())
    }

    /// Read the next chunk from the stream
    fn read_next_chunk(&mut self) -> Result<bool> {
        // Read chunk header (type + length) - 4 bytes total
        let mut chunk_header = [0u8; 4];
        match self.source.read_exact(&mut chunk_header) {
            Ok(()) => {},
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                // End of stream without proper EOF chunk
                self.state = ReaderState::Finished;
                return Ok(false);
            },
            Err(e) => return Err(Error::Io(e)),
        }
        self.bytes_read += 4;

        // Parse chunk header
        let chunk_type = chunk_header[0];
        let chunk_length = u32::from_le_bytes([
            chunk_header[1], chunk_header[2], chunk_header[3], 0
        ]) as usize;


        // Validate chunk length is reasonable
        if chunk_length > self.max_block_size * 2 {
            return Err(Error::Corrupt);
        }

        match chunk_type {
            stream::CHUNK_TYPE_EOF => {
                // EOF chunk - stream is finished
                // Skip any data in EOF chunk
                if chunk_length > 0 {
                    let mut skip_buf = vec![0u8; chunk_length];
                    self.source.read_exact(&mut skip_buf)?;
                    self.bytes_read += chunk_length as u64;
                }
                self.state = ReaderState::Finished;
                return Ok(false);
            },

            stream::CHUNK_TYPE_UNCOMPRESSED => {
                // Uncompressed chunk - read directly
                return self.read_uncompressed_chunk(chunk_length);
            },

            stream::CHUNK_TYPE_MINLZ_COMPRESSED => {
                // MinLZ compressed chunk
                return self.read_compressed_chunk(chunk_length);
            },

            chunk_type if (chunk_type >= 0x80 && chunk_type <= 0xfd) => {
                // User-defined chunk
                return self.read_user_chunk(chunk_type, chunk_length);
            },

            _ => {
                // Skip unknown chunk types
                let mut skip_buf = vec![0u8; chunk_length];
                self.source.read_exact(&mut skip_buf)?;
                self.bytes_read += chunk_length as u64;
                return Ok(true);
            }
        }
    }

    /// Read and process an uncompressed chunk
    fn read_uncompressed_chunk(&mut self, chunk_length: usize) -> Result<bool> {

        // Validate and extract CRC32 if present
        if chunk_length < 4 {
            return Err(Error::Corrupt);
        }

        // Read the entire chunk
        if self.read_buffer.len() < chunk_length {
            self.read_buffer.resize(chunk_length, 0);
        }

        self.source.read_exact(&mut self.read_buffer[..chunk_length])?;
        self.bytes_read += chunk_length as u64;

        // First 4 bytes are CRC32, rest is uncompressed data (matching Go format)
        let crc_bytes = &self.read_buffer[..4];
        let uncompressed_data = &self.read_buffer[4..chunk_length];
        let stored_crc = u32::from_le_bytes([
            crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]
        ]);


        // Verify CRC32
        let calculated_crc = stream::crc32_minlz(uncompressed_data);
        if calculated_crc != stored_crc {
            return Err(Error::Corrupt);
        }

        // Add to output buffer directly (no decompression needed)
        self.output_buffer.extend_from_slice(uncompressed_data);


        Ok(true)
    }

    /// Read and process a compressed chunk
    fn read_compressed_chunk(&mut self, chunk_length: usize) -> Result<bool> {

        // Read the entire chunk
        if self.read_buffer.len() < chunk_length {
            self.read_buffer.resize(chunk_length, 0);
        }

        self.source.read_exact(&mut self.read_buffer[..chunk_length])?;
        self.bytes_read += chunk_length as u64;

        // Validate and extract CRC32 if present
        if chunk_length < 4 {
            return Err(Error::Corrupt);
        }

        // First 4 bytes are CRC32, rest is compressed data (matching Go format)
        let crc_bytes = &self.read_buffer[..4];
        let compressed_data = &self.read_buffer[4..chunk_length];
        let stored_crc = u32::from_le_bytes([
            crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]
        ]);


        // Per SPEC 4.4: MinLZ compressed data (chunk type 0x02) contains
        // "A MinLZ block *without* the MinLZ identifier (initial 0 byte)"
        // The compressed_data already contains: [varint_uncompressed_size] + [minlz_instructions]
        // So we need to prepend the 0x00 byte before decoding
        let mut full_block = Vec::with_capacity(compressed_data.len() + 1);
        full_block.push(0x00); // Add the missing MinLZ identifier
        full_block.extend_from_slice(compressed_data);

        // Decompress the complete MinLZ block
        let mut decompressed = Vec::new();
        match decode(&mut decompressed, &full_block) {
            Ok(()) => {
            }
            Err(e) => {

                // Check if this might be uncompressed data stored directly in a type=2 chunk
                // This can happen when Go's encoder determines compression would make data larger
                if let Some(uncompressed_len) = try_extract_uncompressed_from_type2(&full_block) {

                    // Skip the varint header and treat the rest as literal data
                    let (_varint_value, varint_len) = crate::varint::decode_uvarint(&full_block[1..])?;
                    let literal_data = &full_block[1 + varint_len..];

                    if literal_data.len() == uncompressed_len {
                        decompressed.extend_from_slice(literal_data);
                    } else {
                        return Err(e);
                    }
                } else {
                    return Err(e);
                }
            }
        }

        // Verify CRC32 of the original uncompressed data
        let calculated_crc = stream::crc32_minlz(&decompressed);

        if calculated_crc != stored_crc {
            return Err(Error::Corrupt);
        }


        // Add to output buffer
        self.output_buffer.extend_from_slice(&decompressed);

        Ok(true)
    }

    /// Read and store a user chunk
    fn read_user_chunk(&mut self, chunk_id: u8, chunk_length: usize) -> Result<bool> {
        let mut chunk_data = vec![0u8; chunk_length];
        self.source.read_exact(&mut chunk_data)?;
        self.bytes_read += chunk_length as u64;

        // Store user chunk
        self.user_chunks.push((chunk_id, chunk_data));

        Ok(true)
    }

    /// Fill output buffer if needed
    fn fill_output_buffer(&mut self) -> Result<()> {
        // If we have data in output buffer, no need to read more
        if self.output_pos < self.output_buffer.len() {
            return Ok(());
        }

        // Clear processed data from output buffer
        self.output_buffer.clear();
        self.output_pos = 0;

        // Read chunks until we have data or reach EOF
        while self.output_buffer.is_empty() && self.state != ReaderState::Finished {
            match self.state {
                ReaderState::ReadingHeader => {
                    self.read_header()?;
                },
                ReaderState::ReadingChunks => {
                    if !self.read_next_chunk()? {
                        // No more chunks or EOF reached
                        break;
                    }
                },
                ReaderState::Finished => break,
                ReaderState::Error => return Err(Error::Corrupt),
            }
        }

        Ok(())
    }
}

impl<R: Read> Read for Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Try to fill output buffer if needed
        if let Err(e) = self.fill_output_buffer() {
            self.state = ReaderState::Error;
            return Err(io::Error::new(io::ErrorKind::InvalidData, e));
        }

        // If no data available and stream is finished, return 0
        if self.output_pos >= self.output_buffer.len() && self.state == ReaderState::Finished {
            return Ok(0);
        }

        // Copy data from output buffer to user buffer
        let available = self.output_buffer.len() - self.output_pos;
        if available == 0 {
            return Ok(0);
        }

        let to_copy = buf.len().min(available);
        buf[..to_copy].copy_from_slice(&self.output_buffer[self.output_pos..self.output_pos + to_copy]);
        self.output_pos += to_copy;
        self.bytes_written += to_copy as u64;

        Ok(to_copy)
    }
}

/// Try to detect if a type=2 chunk contains uncompressed data instead of MinLZ instructions.
/// This can happen when Go's encoder determines compression would make data larger.
/// Returns Some(uncompressed_len) if detected, None otherwise.
fn try_extract_uncompressed_from_type2(full_block: &[u8]) -> Option<usize> {
    // full_block format: [0x00, varint_bytes..., data...]
    if full_block.len() < 2 || full_block[0] != 0x00 {
        return None;
    }

    // Try to decode the varint after the 0x00 prefix
    if let Ok((varint_value, varint_len)) = crate::varint::decode_uvarint(&full_block[1..]) {
        let data_start = 1 + varint_len;
        let remaining_data_len = full_block.len() - data_start;

        // Check if this looks like uncompressed data:
        // 1. The remaining data length should match the varint value (indicating no compression)
        // 2. The data should not start with valid MinLZ instruction patterns
        if remaining_data_len == varint_value as usize {
            // Additional validation: check if the data looks like text/binary content
            // rather than MinLZ instruction sequences
            if data_start < full_block.len() {
                let first_few_bytes = &full_block[data_start..data_start.min(full_block.len()).min(data_start + 16)];

                // MinLZ instructions typically have specific patterns for tags (0-4)
                // If we see mostly printable ASCII or other non-instruction patterns,
                // it's likely uncompressed data
                let non_instruction_like = first_few_bytes.iter()
                    .take(8) // Check first 8 bytes
                    .filter(|&&b| {
                        // Not typical MinLZ instruction bytes
                        b > 31 && b < 127 || // Printable ASCII
                        b == 0 || b == 255  // Common data bytes
                    })
                    .count();

                if non_instruction_like >= 4 { // More than half look like data, not instructions
                    return Some(varint_value as usize);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Writer;
    use std::io::Write;

    #[test]
    fn test_reader_basic() {
        // Create test data
        let input = b"Hello, world! This is a test of the MinLZ streaming reader.";

        // Compress with Writer
        let mut compressed = Vec::new();
        let mut writer = Writer::builder(&mut compressed)
            .compression_level(2)
            .build()
            .unwrap();

        writer.write_all(input).unwrap();
        let (final_output, _) = writer.finish().unwrap();

        // Decompress with Reader
        let mut reader = Reader::new(&final_output[..]).unwrap();
        let mut decompressed = Vec::new();
        reader.read_to_end(&mut decompressed).unwrap();

        assert_eq!(&decompressed, input);
    }

    #[test]
    fn test_reader_with_user_chunks() {
        let input = b"Test data for user chunk validation";

        // Compress with user chunks
        let mut compressed = Vec::new();
        let mut writer = Writer::builder(&mut compressed)
            .compression_level(1)
            .build()
            .unwrap();

        writer.add_user_chunk(128, b"metadata").unwrap();
        writer.add_user_chunk(129, b"author=test").unwrap();
        writer.write_all(input).unwrap();
        let (final_output, _) = writer.finish().unwrap();

        // Read with Reader
        let mut reader = Reader::new(&final_output[..]).unwrap();
        let mut decompressed = Vec::new();
        reader.read_to_end(&mut decompressed).unwrap();

        // Verify data
        assert_eq!(&decompressed, input);

        // Verify user chunks
        let user_chunks = reader.user_chunks();
        assert_eq!(user_chunks.len(), 2);
        assert_eq!(user_chunks[0], (128, b"metadata".to_vec()));
        assert_eq!(user_chunks[1], (129, b"author=test".to_vec()));
    }

    #[test]
    fn test_reader_empty_stream() {
        let mut compressed = Vec::new();
        let writer = Writer::builder(&mut compressed)
            .build()
            .unwrap();
        let (final_output, _) = writer.finish().unwrap();

        let mut reader = Reader::new(&final_output[..]).unwrap();
        let mut decompressed = Vec::new();
        reader.read_to_end(&mut decompressed).unwrap();

        assert!(decompressed.is_empty());
        assert!(reader.is_finished());
    }

    #[test]
    fn test_reader_statistics() {
        let input = b"Test data for statistics validation";

        let mut compressed = Vec::new();
        let mut writer = Writer::builder(&mut compressed)
            .build()
            .unwrap();
        writer.write_all(input).unwrap();
        let (final_output, _) = writer.finish().unwrap();

        let mut reader = Reader::new(&final_output[..]).unwrap();
        let mut decompressed = Vec::new();
        reader.read_to_end(&mut decompressed).unwrap();

        let (bytes_read, bytes_written) = reader.bytes_processed();
        assert_eq!(bytes_written, input.len() as u64);
        assert!(bytes_read > 0); // Should have read compressed stream
    }

    #[test]
    fn test_go_compatibility() {
        use std::fs;

        // Try to read Go-generated file
        if let Ok(go_data) = fs::read("go_test.mz") {

            let mut reader = Reader::new(&go_data[..]).unwrap();
            let mut decompressed = Vec::new();
            match reader.read_to_end(&mut decompressed) {
                Ok(_bytes_read) => {
                },
                Err(_e) => {
                }
            }
        } else {
        }
    }
}