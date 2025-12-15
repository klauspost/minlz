//! Constants and tag definitions from the MinLZ specification

/// Compression level constants
pub const LEVEL_FASTEST: i32 = 1;
pub const LEVEL_BALANCED: i32 = 2;
pub const LEVEL_SMALLEST: i32 = 3;

/// Maximum block size (8 MiB)
pub const MAX_BLOCK_SIZE: usize = 8 << 20;

/// Tag values for different chunk types
pub const TAG_LITERAL: u8 = 0x00;
pub const TAG_REPEAT: u8 = 0x00 | (1 << 2);
pub const TAG_COPY1: u8 = 0x01;
pub const TAG_COPY2: u8 = 0x02;
pub const TAG_COPY3: u8 = 0x03;
pub const TAG_COPY2_FUSED: u8 = 0x03;

/// Copy operation limits and offsets

// Copy1 limits
pub const MAX_COPY1_OFFSET: usize = 1024;

// Copy2 limits
pub const MIN_COPY2_OFFSET: usize = 64;
pub const MAX_COPY2_OFFSET: usize = MIN_COPY2_OFFSET + 65535; // 65599
pub const COPY2_LIT_MAX_LEN: usize = 7 + 4;
pub const MAX_COPY2_LITS: usize = 1 << 2; // 4
pub const MIN_COPY2_LENGTH: usize = 64;

// Copy3 limits
pub const MAX_COPY3_LITS: usize = (1 << 2) - 1; // 3
pub const MIN_COPY3_OFFSET: usize = 65536;
pub const MAX_COPY3_OFFSET: usize = (2 << 20) + 65535; // ~2MiB
pub const MIN_COPY3_LENGTH: usize = 64;

/// Encoding parameters
pub const INPUT_MARGIN: usize = 8;
pub const MIN_NON_LITERAL_BLOCK_SIZE: usize = 16;

/// Length encoding thresholds
pub const LITERAL_LENGTH_SMALL_MAX: u8 = 28;
pub const LITERAL_LENGTH_1_BYTE: u8 = 29;
pub const LITERAL_LENGTH_2_BYTE: u8 = 30;
pub const LITERAL_LENGTH_3_BYTE: u8 = 31;

/// Copy1 length encoding
pub const COPY1_LENGTH_EXTENDED: u8 = 15;
pub const COPY1_BASE_LENGTH: usize = 4;
pub const COPY1_EXTENDED_BASE: usize = 18;

/// Copy2 length encoding thresholds
pub const COPY2_LENGTH_MAX_DIRECT: u8 = 60;
pub const COPY2_LENGTH_1_BYTE: u8 = 61;
pub const COPY2_LENGTH_2_BYTE: u8 = 62;
pub const COPY2_LENGTH_3_BYTE: u8 = 63;
pub const COPY2_BASE_LENGTH: usize = 4;
pub const COPY2_EXTENDED_BASE: usize = 64;

/// Copy3 length encoding (same as Copy2)
pub const COPY3_LENGTH_MAX_DIRECT: u8 = 60;
pub const COPY3_LENGTH_1_BYTE: u8 = 61;
pub const COPY3_LENGTH_2_BYTE: u8 = 62;
pub const COPY3_LENGTH_3_BYTE: u8 = 63;
pub const COPY3_BASE_LENGTH: usize = 4;
pub const COPY3_EXTENDED_BASE: usize = 64;