//! Error type for the block codec.

use core::fmt;

/// Errors returned by the MinLZ block codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Input bytes are not a valid MinLZ block (or are truncated/malformed).
    ///
    /// Equivalent to Go's `ErrCorrupt`.
    Corrupt,
    /// Declared or input length exceeds [`crate::MAX_BLOCK_SIZE`].
    ///
    /// Equivalent to Go's `ErrTooLarge`.
    TooLarge,
    /// Compression level outside the supported range ([`crate::Level`]).
    ///
    /// Equivalent to Go's `ErrInvalidLevel`.
    InvalidLevel,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Error::Corrupt => "minlz: corrupt input",
            Error::TooLarge => "minlz: decoded block is too large",
            Error::InvalidLevel => "minlz: invalid compression level",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for Error {}
