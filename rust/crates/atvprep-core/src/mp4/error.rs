//! The error of every `mp4` function.

use std::fmt;
use std::io;

/// Why an MP4 could not be read or patched.
#[derive(Debug)]
pub enum Mp4Error {
    /// The bytes are not a box structure the patcher can follow.
    Malformed(&'static str),
    /// A valid but unsupported shape (for example a 64-bit `moov` header in a write path).
    Unsupported(&'static str),
    Io(io::Error),
}

impl fmt::Display for Mp4Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(what) => write!(f, "malformed MP4: {what}"),
            Self::Unsupported(what) => write!(f, "unsupported MP4: {what}"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Mp4Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Mp4Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub type Result<T> = std::result::Result<T, Mp4Error>;
