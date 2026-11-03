//! MP4 box handling: the byte-level half of `mp4patch` (port of `atvprep/mp4patch.py`).

pub mod atmos;
pub mod boxes;
pub mod copy;
pub mod error;
pub mod offsets;
pub mod patch;
pub mod tags;

pub use error::{Mp4Error, Result};
