//! MinLZ streaming writer implementation
//!
//! This module provides a streaming writer that compresses data into the MinLZ
//! stream format as defined in SPEC.md. The writer is designed to mirror the
//! functionality of the Go implementation while being idiomatic to Rust.

use crate::{encode, varint, Error, Result, LEVEL_BALANCED, LEVEL_FASTEST, LEVEL_SMALLEST};
use crate::stream::*;
use crate::parallel::CompressionPool;
use std::io::{self, Write};

/// Default block size for compression (2MB)
const DEFAULT_BLOCK_SIZE: usize = 2 << 20;

/// Default concurrency level (sequential processing)
const DEFAULT_CONCURRENCY: usize = 1;

/// A streaming MinLZ writer that compresses data into MinLZ stream format
///
/// The writer buffers input data and compresses it in blocks according to the
/// MinLZ specification. It supports various compression levels, block sizes,
/// and optional features like indexing and padding.
///
/// # Examples
///
/// ```rust
/// use minlz::Writer;
/// use std::io::Write;
///
/// let mut output = Vec::new();
/// let mut writer = Writer::new(&mut output)?;
///
/// writer.write_all(b"Hello, MinLZ world!")?;
/// writer.finish()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Writer<W: Write> {
    // Core state
    writer: Option<W>,
    input_buffer: Vec<u8>,

    // Configuration
    block_size: usize,
    level: i32,
    _concurrency: usize,
    flush_on_write: bool,
    _generate_index: bool,
    _append_index: bool,
    padding: Option<usize>,

    // Stream state
    wrote_stream_header: bool,
    closed: bool,

    // Statistics
    uncompressed_written: u64,
    compressed_written: u64,

    // Index tracking (TODO: implement Index)
    // index: Option<Index>,

    // Parallel compression
    compression_pool: Option<CompressionPool>,
    _pending_blocks: Vec<Vec<u8>>,

    // Buffer management
    _output_buffer_size: usize,
}

impl<W: Write> Writer<W> {
    /// Create a new MinLZ writer with default settings
    ///
    /// # Examples
    ///
    /// ```rust
    /// use minlz::Writer;
    ///
    /// let output = Vec::new();
    /// let writer = Writer::new(output)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(writer: W) -> Result<Self> {
        WriterBuilder::new(writer).build()
    }

    /// Create a writer builder for customizing options
    ///
    /// # Examples
    ///
    /// ```rust
    /// use minlz::{Writer, LEVEL_SMALLEST};
    ///
    /// let output = Vec::new();
    /// let writer = Writer::builder(output)
    ///     .compression_level(LEVEL_SMALLEST)
    ///     .block_size(1024 * 1024)
    ///     .build()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn builder(writer: W) -> WriterBuilder<W> {
        WriterBuilder::new(writer)
    }

    /// Write a buffer directly without buffering
    ///
    /// This is more efficient for large chunks of data as it bypasses
    /// the internal buffer and compresses data directly.
    pub fn encode_buffer(&mut self, data: &[u8]) -> Result<()> {
        if self.closed {
            return Err(Error::InvalidInput("Writer is closed".to_string()));
        }

        if data.is_empty() {
            return Ok(());
        }

        // Flush any buffered data first
        if !self.input_buffer.is_empty() {
            self.flush_internal()?;
        }

        // Write stream header if not already written
        if !self.wrote_stream_header {
            self.write_stream_header()?;
        }

        // Process data in block-sized chunks
        let mut remaining = data;
        while !remaining.is_empty() {
            let chunk_size = std::cmp::min(remaining.len(), self.block_size);
            let (chunk, rest) = remaining.split_at(chunk_size);

            self.write_compressed_block(chunk)?;
            remaining = rest;
        }

        Ok(())
    }

    /// Add a user-defined chunk to the stream
    ///
    /// The chunk ID must be in the valid user chunk range (0x80-0xbf for skippable,
    /// 0xc0-0xfd for non-skippable, or 0xfe for padding).
    pub fn add_user_chunk(&mut self, id: u8, data: &[u8]) -> Result<()> {
        if self.closed {
            return Err(Error::InvalidInput("Writer is closed".to_string()));
        }

        if !is_valid_user_chunk_id(id) {
            return Err(Error::InvalidInput(format!("Invalid chunk ID: {:#x}", id)));
        }

        if data.len() > MAX_USER_CHUNK_SIZE {
            return Err(Error::InvalidInput(format!(
                "User chunk too large: {} > {}",
                data.len(),
                MAX_USER_CHUNK_SIZE
            )));
        }

        // Flush any buffered data first
        if !self.input_buffer.is_empty() {
            self.flush_internal()?;
        }

        // Write stream header if not already written
        if !self.wrote_stream_header {
            self.write_stream_header()?;
        }

        // Write chunk header
        let mut header = [0u8; CHUNK_HEADER_SIZE];
        write_chunk_header(&mut header, id, data.len())?;

        if let Some(ref mut writer) = self.writer {
            writer.write_all(&header)?;
            writer.write_all(data)?;
            self.compressed_written += (CHUNK_HEADER_SIZE + data.len()) as u64;
        }

        Ok(())
    }

    /// Get statistics about bytes processed
    ///
    /// Returns (uncompressed_bytes, compressed_bytes)
    pub fn bytes_written(&self) -> (u64, u64) {
        (self.uncompressed_written, self.compressed_written)
    }

    /// Finish writing and return the underlying writer
    ///
    /// This flushes any remaining data, writes the EOF marker, and closes the stream.
    /// Returns the underlying writer and optionally an index if index generation was enabled.
    pub fn finish(mut self) -> Result<(W, Option<Vec<u8>>)> {
        if self.closed {
            return Err(Error::InvalidInput("Writer is already closed".to_string()));
        }

        // Flush any remaining data
        if !self.input_buffer.is_empty() {
            self.flush_internal()?;
        }

        // Wait for all parallel compression jobs to complete
        if let Some(ref mut pool) = self.compression_pool {
            let remaining_results = pool.finish()?;
            for (compressed_data, original_data) in remaining_results {
                self.write_compressed_data(compressed_data, &original_data)?;
            }
        }

        // Write EOF marker
        self.write_eof_marker()?;

        // TODO: Generate index if enabled
        let index = None;

        // Add padding if configured
        if let Some(padding_size) = self.padding {
            self.add_padding(padding_size as u64)?;
        }

        self.closed = true;
        let writer = self.writer.take().unwrap();
        Ok((writer, index))
    }

    // Internal methods

    fn write_stream_header(&mut self) -> Result<()> {
        if let Some(ref mut writer) = self.writer {
            let header = make_stream_header(self.block_size)?;
            writer.write_all(&header)?;
            self.compressed_written += header.len() as u64;
            self.wrote_stream_header = true;
        }
        Ok(())
    }

    fn write_compressed_block(&mut self, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }

        let writer = self.writer.as_mut().unwrap();

        // Calculate checksum of uncompressed data
        let checksum = crc32_minlz(data);

        // Prepare output buffer for compression - preallocate with actual size
        let mut compressed_data = vec![0u8; data.len() + OUTPUT_BUFFER_HEADER_SIZE];

        // Try to compress the data
        let compression_result = match self.level {
            LEVEL_FASTEST => encode::level1::encode_block(&mut compressed_data, data),
            LEVEL_BALANCED => encode::level2::encode_block(&mut compressed_data, data),
            LEVEL_SMALLEST => encode::level3::encode_block(&mut compressed_data, data),
            _ => return Err(Error::InvalidLevel),
        };

        // Prepare final data buffer
        let (chunk_type, final_data) = match compression_result {
            Ok(compressed_len) if compressed_len > 0 => {
                // Compression succeeded
                compressed_data.truncate(compressed_len);

                // Prepare the final buffer with varint length + compressed data
                let mut final_buffer = Vec::new();
                let _varint_len = varint::encode_uvarint_vec(&mut final_buffer, data.len() as u64)?;
                final_buffer.extend_from_slice(&compressed_data);

                (CHUNK_TYPE_MINLZ_COMPRESSED, final_buffer)
            }
            _ => {
                // Compression failed or wasn't beneficial, store uncompressed
                (CHUNK_TYPE_UNCOMPRESSED, data.to_vec())
            }
        };

        // Calculate chunk data length (checksum + data)
        let chunk_data_len = CHECKSUM_SIZE + final_data.len();

        // Write chunk header
        let mut header = [0u8; CHUNK_HEADER_SIZE];
        write_chunk_header(&mut header, chunk_type, chunk_data_len)?;
        writer.write_all(&header)?;

        // Write checksum (before data, matching Go format)
        let mut checksum_bytes = [0u8; CHECKSUM_SIZE];
        write_checksum(&mut checksum_bytes, checksum)?;
        writer.write_all(&checksum_bytes)?;

        // Write data
        writer.write_all(&final_data)?;

        // Update statistics
        self.uncompressed_written += data.len() as u64;
        self.compressed_written += (CHUNK_HEADER_SIZE + chunk_data_len) as u64;

        Ok(())
    }

    /// Write pre-compressed data as a chunk
    fn write_compressed_data(&mut self, compressed_data: Vec<u8>, original_data: &[u8]) -> Result<()> {
        if let Some(ref mut writer) = self.writer {
            // Calculate checksum for the original uncompressed data (MinLZ format requirement)
            let checksum = crc32_minlz(original_data);

            // Determine chunk type and prepare data based on compression result
            let (chunk_type, final_buffer) = if compressed_data.is_empty() {
                // Compression failed or wasn't beneficial - store as uncompressed
                (CHUNK_TYPE_UNCOMPRESSED, original_data.to_vec())
            } else {
                // Compression succeeded - prepare with varint length + compressed data
                let mut buffer = Vec::new();
                let _varint_len = crate::varint::encode_uvarint_vec(&mut buffer, original_data.len() as u64)?;
                buffer.extend_from_slice(&compressed_data);
                (CHUNK_TYPE_MINLZ_COMPRESSED, buffer)
            };

            // Calculate chunk data length (checksum + data)
            let chunk_data_len = CHECKSUM_SIZE + final_buffer.len();

            // Write chunk header
            let mut header = [0u8; CHUNK_HEADER_SIZE];
            write_chunk_header(&mut header, chunk_type, chunk_data_len)?;
            writer.write_all(&header)?;

            // Write checksum (before data, matching Go format)
            let mut checksum_bytes = [0u8; CHECKSUM_SIZE];
            write_checksum(&mut checksum_bytes, checksum)?;
            writer.write_all(&checksum_bytes)?;

            // Write data
            writer.write_all(&final_buffer)?;

            // Update statistics
            self.uncompressed_written += original_data.len() as u64;
            self.compressed_written += (CHUNK_HEADER_SIZE + chunk_data_len) as u64;

            if self.flush_on_write {
                writer.flush()?;
            }
        }
        Ok(())
    }

    fn write_eof_marker(&mut self) -> Result<()> {
        // Write stream header if not already written (important for empty streams)
        if !self.wrote_stream_header {
            self.write_stream_header()?;
        }

        if let Some(ref mut writer) = self.writer {
            // Prepare EOF chunk with total uncompressed size
            let mut eof_data = Vec::new();
            let _varint_len = varint::encode_uvarint_vec(&mut eof_data, self.uncompressed_written)?;

            // Write chunk header
            let mut header = [0u8; CHUNK_HEADER_SIZE];
            write_chunk_header(&mut header, CHUNK_TYPE_EOF, eof_data.len())?;
            writer.write_all(&header)?;

            // Write EOF data (varint-encoded total size)
            writer.write_all(&eof_data)?;

            self.compressed_written += (CHUNK_HEADER_SIZE + eof_data.len()) as u64;
        }
        Ok(())
    }

    fn add_padding(&mut self, want_multiple: u64) -> Result<()> {
        let current_size = self.compressed_written;
        let padding_size = crate::stream::calc_skippable_frame_size(current_size, want_multiple);

        if padding_size > 0 {
            // Write padding chunk header
            let data_size = padding_size - crate::stream::CHUNK_HEADER_SIZE as u64;
            let mut header = [0u8; 4];
            crate::stream::write_chunk_header(&mut header, crate::stream::CHUNK_TYPE_PADDING, data_size as usize)?;

            if let Some(writer) = &mut self.writer {
                writer.write_all(&header)?;

                // Fill with random data for security
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};

                let mut hasher = DefaultHasher::new();
                current_size.hash(&mut hasher);
                let mut seed = hasher.finish();

                for _ in 0..data_size {
                    // Simple PRNG for padding data
                    seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                    let byte = (seed >> 24) as u8;
                    writer.write_all(&[byte])?;
                }

                self.compressed_written += padding_size;
            }
        }

        Ok(())
    }

    fn flush_internal(&mut self) -> Result<()> {
        if self.input_buffer.is_empty() {
            return Ok(());
        }

        // Write stream header if not already written
        if !self.wrote_stream_header {
            self.write_stream_header()?;
        }

        // Handle compression based on concurrency mode
        if self.compression_pool.is_some() {
            // Parallel compression: submit block to pool
            let data = self.input_buffer.clone();
            self.compression_pool.as_mut().unwrap().compress_block(data, self.level)?;

            // Try to get completed results and write them
            let mut results = Vec::new();
            while let Some((compressed_data, original_data)) = self.compression_pool.as_mut().unwrap().get_result()? {
                results.push((compressed_data, original_data));
            }

            // Write all results after borrowing ends
            for (compressed_data, original_data) in results {
                self.write_compressed_data(compressed_data, &original_data)?;
            }
        } else {
            // Sequential compression: compress and write immediately
            self.write_compressed_block(&self.input_buffer.clone())?;
        }

        // Clear the buffer
        self.input_buffer.clear();

        Ok(())
    }
}

impl<W: Write> Write for Writer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Writer is closed"
            ));
        }

        if buf.is_empty() {
            return Ok(0);
        }

        if self.flush_on_write {
            // Direct write mode: encode each write immediately
            self.encode_buffer(buf)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            return Ok(buf.len());
        }

        let mut bytes_written = 0;
        let mut remaining = buf;

        // Process data that might fill or exceed the buffer
        while !remaining.is_empty() {
            let buffer_space = self.block_size - self.input_buffer.len();

            if remaining.len() >= buffer_space && !self.input_buffer.is_empty() {
                // Fill the buffer and flush
                let to_copy = std::cmp::min(remaining.len(), buffer_space);
                self.input_buffer.extend_from_slice(&remaining[..to_copy]);
                bytes_written += to_copy;
                remaining = &remaining[to_copy..];

                // Flush the full buffer
                self.flush_internal()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            } else if remaining.len() >= self.block_size && self.input_buffer.is_empty() {
                // Large write with empty buffer: write directly
                let chunk_size = (remaining.len() / self.block_size) * self.block_size;
                self.encode_buffer(&remaining[..chunk_size])
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                bytes_written += chunk_size;
                remaining = &remaining[chunk_size..];
            } else {
                // Buffer the remaining data
                self.input_buffer.extend_from_slice(remaining);
                bytes_written += remaining.len();
                break;
            }
        }

        Ok(bytes_written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_internal()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        if let Some(ref mut writer) = self.writer {
            writer.flush()?;
        }

        Ok(())
    }
}

/// Builder for configuring MinLZ writer options
pub struct WriterBuilder<W: Write> {
    writer: W,
    block_size: usize,
    level: i32,
    concurrency: usize,
    flush_on_write: bool,
    generate_index: bool,
    append_index: bool,
    padding: Option<usize>,
}

impl<W: Write> WriterBuilder<W> {
    /// Create a new writer builder with default settings
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            block_size: DEFAULT_BLOCK_SIZE,
            level: LEVEL_BALANCED,
            concurrency: DEFAULT_CONCURRENCY,
            flush_on_write: false,
            generate_index: true,
            append_index: false,
            padding: None,
        }
    }

    /// Set the compression level
    ///
    /// Valid levels are:
    /// - LEVEL_FASTEST (1): Optimized for speed
    /// - LEVEL_BALANCED (2): Balance between speed and compression
    /// - LEVEL_SMALLEST (3): Optimized for compression ratio
    pub fn compression_level(mut self, level: i32) -> Self {
        self.level = level;
        self
    }

    /// Set the block size for compression
    ///
    /// Must be between 4KB and 8MB and a power of 2.
    pub fn block_size(mut self, size: usize) -> Self {
        self.block_size = size;
        self
    }

    /// Set concurrency level (currently ignored, reserved for future use)
    pub fn concurrency(mut self, n: usize) -> Self {
        self.concurrency = n;
        self
    }

    /// Enable flushing on every write
    ///
    /// When enabled, each write call will immediately compress and output data.
    /// This is less efficient but provides immediate output.
    pub fn flush_on_write(mut self, enable: bool) -> Self {
        self.flush_on_write = enable;
        self
    }

    /// Enable or disable index generation
    pub fn generate_index(mut self, enable: bool) -> Self {
        self.generate_index = enable;
        self
    }

    /// Enable appending the index to the stream when closed
    pub fn append_index(mut self, enable: bool) -> Self {
        self.append_index = enable;
        self
    }

    /// Set padding to make output size a multiple of the specified value
    pub fn padding(mut self, size: usize) -> Self {
        self.padding = Some(size);
        self
    }

    /// Build the writer with the configured options
    pub fn build(self) -> Result<Writer<W>> {
        // Validate block size
        if self.block_size < MIN_BLOCK_SIZE || self.block_size > MAX_BLOCK_SIZE {
            return Err(Error::InvalidInput(format!(
                "Block size {} out of range [{}, {}]",
                self.block_size, MIN_BLOCK_SIZE, MAX_BLOCK_SIZE
            )));
        }

        if !self.block_size.is_power_of_two() {
            return Err(Error::InvalidInput(format!(
                "Block size {} must be a power of 2",
                self.block_size
            )));
        }

        // Validate compression level
        if self.level < LEVEL_FASTEST || self.level > LEVEL_SMALLEST {
            return Err(Error::InvalidLevel);
        }

        // Calculate output buffer size
        let output_buffer_size = OUTPUT_BUFFER_HEADER_SIZE +
            crate::max_encoded_len(self.block_size).unwrap_or(self.block_size + 2);

        // Initialize compression pool for parallel compression
        let compression_pool = if self.concurrency > 1 {
            Some(CompressionPool::new(self.concurrency)?)
        } else {
            None
        };

        Ok(Writer {
            writer: Some(self.writer),
            input_buffer: Vec::with_capacity(self.block_size),
            block_size: self.block_size,
            level: self.level,
            _concurrency: self.concurrency,
            flush_on_write: self.flush_on_write,
            _generate_index: self.generate_index,
            _append_index: self.append_index,
            padding: self.padding,
            wrote_stream_header: false,
            closed: false,
            uncompressed_written: 0,
            compressed_written: 0,
            compression_pool,
            _pending_blocks: Vec::new(),
            _output_buffer_size: output_buffer_size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn test_writer_new() {
        let output = Vec::new();
        let writer = Writer::new(output).unwrap();
        assert_eq!(writer.block_size, DEFAULT_BLOCK_SIZE);
        assert_eq!(writer.level, LEVEL_BALANCED);
    }

    #[test]
    fn test_writer_builder() {
        let output = Vec::new();
        let writer = Writer::builder(output)
            .compression_level(LEVEL_SMALLEST)
            .block_size(1024 * 1024)
            .flush_on_write(true)
            .build()
            .unwrap();

        assert_eq!(writer.level, LEVEL_SMALLEST);
        assert_eq!(writer.block_size, 1024 * 1024);
        assert!(writer.flush_on_write);
    }

    #[test]
    fn test_simple_write() {
        let output = Vec::new();
        let mut writer = Writer::new(output).unwrap();

        writer.write_all(b"Hello, MinLZ!").unwrap();
        let (output, _) = writer.finish().unwrap();

        // Should have stream header, compressed block, and EOF
        assert!(!output.is_empty());
        assert!(output.starts_with(MAGIC_CHUNK));
    }

    #[test]
    fn test_round_trip() {
        let test_data = b"Hello, MinLZ world! This is a test of the streaming writer.";

        let output = Vec::new();
        let mut writer = Writer::new(output).unwrap();
        writer.write_all(test_data).unwrap();
        let (compressed, _) = writer.finish().unwrap();

        // Try to decompress (this will need the reader implementation)
        // For now, just verify we have a valid stream format
        assert!(compressed.starts_with(MAGIC_CHUNK));
        assert!(compressed.len() > MAGIC_CHUNK.len());
    }

    #[test]
    fn test_encode_buffer() {
        let test_data = b"This is test data for encode_buffer method";

        let output = Vec::new();
        let mut writer = Writer::new(output).unwrap();
        writer.encode_buffer(test_data).unwrap();
        let (compressed, _) = writer.finish().unwrap();

        assert!(compressed.starts_with(MAGIC_CHUNK));
    }

    #[test]
    fn test_user_chunk() {
        let output = Vec::new();
        let mut writer = Writer::new(output).unwrap();

        // Add a skippable user chunk
        writer.add_user_chunk(0x80, b"metadata").unwrap();
        writer.write_all(b"Hello").unwrap();
        let (compressed, _) = writer.finish().unwrap();

        assert!(compressed.starts_with(MAGIC_CHUNK));
    }

    #[test]
    fn test_invalid_chunk_id() {
        let output = Vec::new();
        let mut writer = Writer::new(output).unwrap();

        // Try to add chunk with invalid ID
        let result = writer.add_user_chunk(0x01, b"invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_validation() {
        let output = Vec::new();

        // Test invalid block size
        let result = Writer::builder(output.clone())
            .block_size(1000) // Not power of 2
            .build();
        assert!(result.is_err());

        // Test block size too small
        let result = Writer::builder(output.clone())
            .block_size(1024) // Less than 4KB
            .build();
        assert!(result.is_err());

        // Test invalid compression level
        let result = Writer::builder(output)
            .compression_level(10) // Invalid level
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_bytes_written() {
        let test_data = b"Hello, world!";

        let output = Vec::new();
        let mut writer = Writer::new(output).unwrap();
        writer.write_all(test_data).unwrap();

        // Flush to process buffered data
        writer.flush().unwrap();

        let (uncompressed, compressed) = writer.bytes_written();
        assert_eq!(uncompressed, test_data.len() as u64);
        assert!(compressed > 0);
    }

    #[test]
    fn test_debug_crc() {
        let test_data = b"Hello, world! This is test data for parallel compression.\n";
        let crc = crate::stream::crc32_minlz(test_data);
        println!("Data: {:?}", std::str::from_utf8(test_data));
        println!("Length: {}", test_data.len());
        println!("CRC: {} (0x{:08x})", crc, crc);

        // Test Go CRC value (from hex dump: 8c 84 51 d4)
        let go_crc = u32::from_le_bytes([0x8c, 0x84, 0x51, 0xd4]);
        println!("Go CRC: {} (0x{:08x}) (from bytes 8c 84 51 d4)", go_crc, go_crc);
    }

    #[test]
    fn test_writer_parallel_compression() {
        let mut output = Vec::new();
        let mut writer = Writer::builder(&mut output)
            .compression_level(1)
            .concurrency(2) // Enable parallel compression
            .build()
            .unwrap();

        // Write multiple blocks to trigger parallel compression
        writer.write_all(b"First block of data for parallel compression").unwrap();
        writer.write_all(b"Second block of data for parallel compression").unwrap();
        writer.write_all(b"Third block of data for parallel compression").unwrap();

        let (final_output, _) = writer.finish().unwrap();

        // Verify the stream starts with proper header
        assert!(final_output.starts_with(MAGIC_CHUNK));
        assert!(final_output.len() > MAGIC_CHUNK.len());

        // Verify we can read it back with our Reader
        let mut reader = crate::Reader::new(&final_output[..]).unwrap();
        let mut decompressed = Vec::new();
        reader.read_to_end(&mut decompressed).unwrap();

        let expected = b"First block of data for parallel compressionSecond block of data for parallel compressionThird block of data for parallel compression";
        assert_eq!(&decompressed, expected);
    }

    #[test]
    fn test_writer_single_vs_parallel() {
        let test_data = b"This is test data for comparing single vs parallel compression performance";

        // Single-threaded compression
        let mut output1 = Vec::new();
        let mut writer1 = Writer::builder(&mut output1)
            .compression_level(2)
            .concurrency(1) // Single-threaded
            .build()
            .unwrap();
        writer1.write_all(test_data).unwrap();
        let (result1, _) = writer1.finish().unwrap();

        // Parallel compression
        let mut output2 = Vec::new();
        let mut writer2 = Writer::builder(&mut output2)
            .compression_level(2)
            .concurrency(4) // Multi-threaded
            .build()
            .unwrap();
        writer2.write_all(test_data).unwrap();
        let (result2, _) = writer2.finish().unwrap();

        // Both should produce valid MinLZ streams
        assert!(result1.starts_with(MAGIC_CHUNK));
        assert!(result2.starts_with(MAGIC_CHUNK));

        // Both should decompress to the same data
        let mut reader1 = crate::Reader::new(&result1[..]).unwrap();
        let mut decompressed1 = Vec::new();
        reader1.read_to_end(&mut decompressed1).unwrap();

        let mut reader2 = crate::Reader::new(&result2[..]).unwrap();
        let mut decompressed2 = Vec::new();
        reader2.read_to_end(&mut decompressed2).unwrap();

        assert_eq!(&decompressed1, test_data);
        assert_eq!(&decompressed2, test_data);
        assert_eq!(decompressed1, decompressed2);
    }

    #[test]
    fn test_writer_padding() {
        let input = b"Hello, world! This is test data for padding.";

        // Test without padding
        let mut compressed_no_padding = Vec::new();
        let mut writer_no_padding = Writer::builder(&mut compressed_no_padding)
            .compression_level(2)
            .build()
            .unwrap();

        writer_no_padding.write_all(input).unwrap();
        let (final_no_padding, _) = writer_no_padding.finish().unwrap();

        // Test with 64-byte alignment padding
        let mut compressed_with_padding = Vec::new();
        let mut writer_with_padding = Writer::builder(&mut compressed_with_padding)
            .compression_level(2)
            .padding(64)
            .build()
            .unwrap();

        writer_with_padding.write_all(input).unwrap();
        let (final_with_padding, _) = writer_with_padding.finish().unwrap();

        println!("No padding size: {}", final_no_padding.len());
        println!("With padding size: {}", final_with_padding.len());

        // Padded output should be larger and aligned to 64 bytes
        assert!(final_with_padding.len() > final_no_padding.len());
        assert_eq!(final_with_padding.len() % 64, 0);

        // Both should decompress to the same original data
        let mut reader_no_padding = crate::Reader::new(&final_no_padding[..]).unwrap();
        let mut decompressed_no_padding = Vec::new();
        reader_no_padding.read_to_end(&mut decompressed_no_padding).unwrap();
        assert_eq!(&decompressed_no_padding, input);

        let mut reader_with_padding = crate::Reader::new(&final_with_padding[..]).unwrap();
        let mut decompressed_with_padding = Vec::new();
        reader_with_padding.read_to_end(&mut decompressed_with_padding).unwrap();
        assert_eq!(&decompressed_with_padding, input);
    }
}