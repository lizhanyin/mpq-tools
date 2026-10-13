//! Error types for the MPQ library.

use std::fmt;

/// Result alias used across the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// All errors the MPQ library can produce.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Underlying I/O failure.
    Io(std::io::Error),
    /// No MPQ header could be located in the stream.
    NotAnArchive,
    /// The header declares a format version this library does not understand.
    UnsupportedVersion(u16),
    /// A table or header field is inconsistent / corrupt.
    Corrupt(&'static str),
    /// A table or header field is inconsistent / corrupt (dynamic message).
    InvalidData(String),
    /// The requested file was not found in the archive.
    FileNotFound(String),
    /// The file exists but is marked deleted.
    FileDeleted,
    /// The file uses a compression method that is not supported.
    UnsupportedCompression(u8),
    /// The file is encrypted and no usable key could be derived.
    UnknownFileKey,
    /// A data block failed MD5/CRC verification.
    ChecksumMismatch(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "i/o error: {e}"),
            Error::NotAnArchive => write!(f, "no MPQ archive header found in this file"),
            Error::UnsupportedVersion(v) => {
                write!(f, "unsupported MPQ format version field: {v}")
            }
            Error::Corrupt(what) => write!(f, "corrupt archive: {what}"),
            Error::InvalidData(msg) => write!(f, "invalid data: {msg}"),
            Error::FileNotFound(name) => write!(f, "file not found in archive: {name}"),
            Error::FileDeleted => write!(f, "file is marked as deleted"),
            Error::UnsupportedCompression(m) => {
                write!(f, "unsupported compression method 0x{m:02x}")
            }
            Error::UnknownFileKey => write!(f, "file is encrypted and the key could not be derived"),
            Error::ChecksumMismatch(msg) => write!(f, "checksum mismatch: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
