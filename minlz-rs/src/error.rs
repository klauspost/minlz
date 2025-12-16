//! Error types for MinLZ operations

use std::fmt;

/// Result type for MinLZ operations
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur during compression/decompression
#[derive(Debug)]
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

    /// Invalid input parameter or data
    InvalidInput(String),

    /// I/O error
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Corrupt => write!(f, "minlz: corrupt input"),
            Error::TooLarge => write!(f, "minlz: decoded block is too large"),
            Error::Unsupported => write!(f, "minlz: unsupported input"),
            Error::InvalidLevel => write!(f, "minlz: invalid compression level"),
            Error::CrcMismatch => write!(f, "minlz: corrupt input, crc mismatch"),
            Error::InvalidInput(msg) => write!(f, "minlz: invalid input: {}", msg),
            Error::Io(err) => write!(f, "minlz: I/O error: {}", err),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}