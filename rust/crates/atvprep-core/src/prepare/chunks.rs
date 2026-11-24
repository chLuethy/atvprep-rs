//! Die Chunk-Stufe: `MP4Box` schreibt die Chunks einer ffmpeg-Ausgabe auf rund eine halbe Sekunde
//! Medienzeit um (ADR 0010).
//!
//! ffmpegs mov-Muxer bildet Chunks von ein bis zwei Samples und lässt sich nicht auf eine Chunk-Dauer
//! einstellen; Apple TV stockt bei solchen Dateien. `MP4Box -inter 500` verschränkt mit Chunks von
//! 500 ms.

use std::fmt;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Die Chunk-Dauer, um die `MP4Box` gebeten wird, in Millisekunden. Fest, nicht Teil des Zielprofils.
pub const CHUNK_MS: u32 = 500;

/// Warum die Chunk-Stufe keine Datei geliefert hat.
#[derive(Debug)]
pub enum ChunkError {
    /// `MP4Box` ist nicht installiert (oder nicht im Suchpfad).
    Missing,
    /// `MP4Box` liess sich nicht starten oder endete, ohne eine Datei zu schreiben.
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

/// Das Programm `MP4Box`, gefunden über `PATH` (oder in einer ausdrücklichen Verzeichnisliste).
#[derive(Debug, Clone, Default)]
pub struct Mp4Box {
    search_path: Option<Vec<PathBuf>>,
}

impl Mp4Box {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `MP4Box` in diesen Verzeichnissen statt in `PATH` suchen (eine Test-Naht).
    #[must_use]
    pub fn with_search_path(mut self, dirs: Vec<PathBuf>) -> Self {
        self.search_path = Some(dirs);
        self
    }

    /// Die ausführbare Datei, wenn es eine gibt.
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

    /// Schreibt `input` nach `output` mit Chunks von [`CHUNK_MS`]; `progress` bekommt 0..=1, wie
    /// `MP4Box` es meldet.
    ///
    /// `MP4Box` weist eine ffmpeg-Ausgabe zurück, deren Spur eine `name`-Box von weniger als vier Byte
    /// hat (ein Titel wie „DEU“). Nur dann läuft es erneut mit `-no-check`; das liest die Datei, löscht
    /// aber diese Titel. [`Rechunked::without_check`] sagt es.
    ///
    /// # Errors
    ///
    /// [`ChunkError::Missing`] ohne `MP4Box`, [`ChunkError::Failed`], wenn es scheitert oder nichts schreibt.
    pub fn rechunk(
        &self,
        input: &Path,
        output: &Path,
        progress: &mut dyn FnMut(f64),
    ) -> Result<Rechunked, ChunkError> {
        let program = self.find().ok_or(ChunkError::Missing)?;
        match run_once(&program, input, output, false, progress) {
            Err(ChunkError::Failed(why)) if why.contains("Invalid IsoMedia File") => {
                run_once(&program, input, output, true, progress)?;
                Ok(Rechunked {
                    without_check: true,
                })
            }
            other => other.map(|()| Rechunked {
                without_check: false,
            }),
        }
    }
}

/// Wie die Chunk-Stufe verlief.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rechunked {
    /// `MP4Box` konnte die Datei nur mit `-no-check` lesen; kurze Spurtitel sind dann gelöscht.
    pub without_check: bool,
}

fn run_once(
    program: &Path,
    input: &Path,
    output: &Path,
    without_check: bool,
    progress: &mut dyn FnMut(f64),
) -> Result<(), ChunkError> {
    let mut command = Command::new(program);
    if without_check {
        command.arg("-no-check");
    }
    let mut child = command
        .arg("-inter")
        .arg(CHUNK_MS.to_string())
        .arg(input)
        .arg("-out")
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ChunkError::Failed(e.to_string()))?;
    let mut tail = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let mut line = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            match stderr.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
            // MP4Box zeichnet den Fortschrittsbalken mit Wagenrücklauf neu.
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

/// `MP4Box` färbt seine Meldungen mit ANSI-Escapes, auch wenn stderr eine Pipe ist.
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
