//! The chunk stage: `MP4Box` rewrites the chunks of an ffmpeg output to about half a second
//! of media time (ADR 0010).
//!
//! ffmpeg's mov muxer writes chunks of one or two samples and cannot be set to a chunk duration;
//! Apple TV stutters on such files. `MP4Box -old-inter 500` interleaves with chunks of
//! 500 ms on every track. Plain `-inter` adds a drift correction between the tracks that, with a
//! sparse track (subtitles), cuts the chunks of all other tracks to a fraction of that duration.

use std::fmt;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::mp4::chunks::TARGET_CHUNK_MS;
use crate::mp4::patch;
use crate::mp4::restore::Reference;

/// Why the chunk stage did not deliver a file.
#[derive(Debug)]
pub enum ChunkError {
    /// `MP4Box` is not installed (or not on the search path).
    Missing,
    /// `MP4Box` could not be started or ended without writing a file.
    Failed(String),
}

impl fmt::Display for ChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str(
                "MP4Box not found: install GPAC (MP4Box) or update the image to rewrite the chunks",
            ),
            Self::Failed(why) => write!(f, "MP4Box failed: {why}"),
        }
    }
}

impl std::error::Error for ChunkError {}

/// The `MP4Box` program, found through `PATH` (or in an explicit list of directories).
#[derive(Debug, Clone, Default)]
pub struct Mp4Box {
    search_path: Option<Vec<PathBuf>>,
}

impl Mp4Box {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Look for `MP4Box` in these directories instead of `PATH` (a test seam).
    #[must_use]
    pub fn with_search_path(mut self, dirs: Vec<PathBuf>) -> Self {
        self.search_path = Some(dirs);
        self
    }

    /// The executable, if there is one.
    #[must_use]
    pub fn find(&self) -> Option<PathBuf> {
        let dirs: Vec<PathBuf> = self.search_path.clone().unwrap_or_else(|| {
            std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default()
        });
        dirs.into_iter()
            .map(|dir| dir.join("MP4Box"))
            .find(|candidate| {
                std::fs::metadata(candidate)
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
    }

    /// Writes `input` to `output` with chunks of [`TARGET_CHUNK_MS`]; `progress` receives 0..=1 as
    /// `MP4Box` reports it.
    ///
    /// `MP4Box` rejects an ffmpeg output whose track has a `name` box shorter than four bytes
    /// (a title such as "DEU"). Only then does it run again with `-no-check`, which reads the file but
    /// clears those titles. [`Rechunked::check`] says which happened.
    ///
    /// `MP4Box` drops or rewrites some atoms of the moov (titles, `dec3`, `esds`, `tkhd`, `mdhd`,
    /// sample entries). [`Rechunked::reference`] holds them as `input` had them; the moov patch that
    /// follows (`apply_moov_patches_restoring`) or [`patch::restore_from`] writes them back into
    /// `output`. `input` is read before `MP4Box` runs.
    ///
    /// # Errors
    ///
    /// [`ChunkError::Missing`] without `MP4Box`, [`ChunkError::Failed`] when it fails or writes nothing.
    pub fn rechunk(
        &self,
        input: &Path,
        output: &Path,
        progress: &mut dyn FnMut(f64),
    ) -> Result<Rechunked, ChunkError> {
        let program = self.find().ok_or(ChunkError::Missing)?;
        // Read before `MP4Box` runs: `input` may be what the caller replaces with `output` afterwards.
        let reference = patch::read_moov_of(input)
            .map_err(|e| ChunkError::Failed(format!("could not read the moov of the input: {e}")))?
            .map(|moov| Reference::of_moov(&moov));
        let check = match run_once(&program, input, output, Check::Strict, progress) {
            Err(ChunkError::Failed(why)) if why.contains("Invalid IsoMedia File") => {
                run_once(&program, input, output, Check::Skip, progress)?;
                Check::Skip
            }
            other => {
                other?;
                Check::Strict
            }
        };
        Ok(Rechunked { check, reference })
    }
}

/// How the chunk stage went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rechunked {
    /// How `MP4Box` read the file; with [`Check::Skip`] it cleared short track titles.
    pub check: Check,
    /// What the moov of the input held that `MP4Box` may have lost; `None` without a moov.
    pub reference: Option<Reference>,
}

/// Whether `MP4Box` validated the file it read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// The normal run: `MP4Box` rejects a file it finds invalid.
    Strict,
    /// `-no-check`: the file is read although it is invalid.
    Skip,
}

fn run_once(
    program: &Path,
    input: &Path,
    output: &Path,
    check: Check,
    progress: &mut dyn FnMut(f64),
) -> Result<(), ChunkError> {
    let mut command = Command::new(program);
    if check == Check::Skip {
        command.arg("-no-check");
    }
    let mut child = command
        .arg("-old-inter")
        .arg(TARGET_CHUNK_MS.to_string())
        .arg(input)
        .arg("-out")
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ChunkError::Failed(e.to_string()))?;
    let mut tail = String::new();
    if let Some(stderr) = child.stderr.take() {
        let mut stderr = io::BufReader::new(stderr);
        let mut line = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            match stderr.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
            // MP4Box redraws its progress bar with a carriage return.
            if byte[0] == b'\r' || byte[0] == b'\n' {
                let text = String::from_utf8_lossy(&line).into_owned();
                line.clear();
                if let Some(fraction) = parse_progress(&text) {
                    progress(fraction);
                } else if !text.trim().is_empty() {
                    tail = strip_colours(text.trim());
                }
            } else {
                line.push(byte[0]);
            }
        }
    }
    let status = child
        .wait()
        .map_err(|e| ChunkError::Failed(e.to_string()))?;
    let wrote = std::fs::metadata(output).is_ok_and(|m| m.is_file() && m.len() > 0);
    if status.success() && wrote {
        progress(1.0);
        return Ok(());
    }
    let _ = std::fs::remove_file(output);
    Err(ChunkError::Failed(if tail.is_empty() {
        format!("exit {}", status.code().map_or(-1, i64::from))
    } else {
        tail
    }))
}

/// `MP4Box` colours its messages with ANSI escapes even when stderr is a pipe.
fn strip_colours(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `ISO File Writing: |=====   | (42/100)` is 0.42.
fn parse_progress(line: &str) -> Option<f64> {
    let inner = line.trim_end().strip_suffix("/100)")?;
    let (_, digits) = inner.rsplit_once('(')?;
    digits
        .parse::<u32>()
        .ok()
        .map(|n| f64::from(n.min(100)) / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_read_from_the_bar() {
        assert_eq!(
            parse_progress("ISO File Writing: |==   | (42/100)"),
            Some(0.42)
        );
        assert_eq!(parse_progress("Importing foo"), None);
    }
}
