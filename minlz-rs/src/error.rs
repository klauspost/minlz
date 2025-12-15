//! Error types for MinLZ operations

use std::fmt;

/// Result type for MinLZ operations
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur during compression/decompression
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Input data is corrupted or invalid
    Corrupt,

    /// Decoded block would be too large
    TooLarge,

    /// Input format is not supported
    Unsupported,

    /// Invalid compression level
    InvalidLevel,

    /// CRC checksum mismatch (for streams)
    CrcMismatch,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Corrupt => write!(f, "minlz: corrupt input"),
            Error::TooLarge => write!(f, "minlz: decoded block is too large"),
            Error::Unsupported => write!(f, "minlz: unsupported input"),
            Error::InvalidLevel => write!(f, "minlz: invalid compression level"),
            Error::CrcMismatch => write!(f, "minlz: corrupt input, crc mismatch"),
        }
    }
}

impl std::error::Error for Error {}