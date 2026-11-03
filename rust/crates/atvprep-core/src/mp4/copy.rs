//! Chunked file copying and the temporary-file protocol of the moov rewrites.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use super::error::{Mp4Error, Result};

/// Few large reads instead of many small ones.
pub const COPY_CHUNK: usize = 16 << 20;

/// Bytes in one directory entry on the filesystems the library lives on.
const NAME_MAX: usize = 255;

/// Read until `buffer` is full or the source ends; returns the bytes read.
fn read_fill<R: Read>(source: &mut R, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match source.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// Copy `count` bytes (fewer when the source ends) in chunks of at most `chunk` bytes.
///
/// # Errors
///
/// Read or write errors.
pub fn copy_n<R: Read, W: Write>(
    source: &mut R,
    sink: &mut W,
    count: u64,
    chunk: usize,
    on_chunk: &mut dyn FnMut(u64),
) -> std::io::Result<()> {
    let mut left = count;
    let size = usize::try_from(left).map_or(chunk, |n| n.min(chunk));
    let mut buffer = vec![0_u8; if left > 0 { size } else { 0 }];
    while left > 0 {
        let want = usize::try_from(left).map_or(buffer.len(), |n| n.min(buffer.len()));
        let got = read_fill(source, &mut buffer[..want])?;
        if got == 0 {
            break;
        }
        sink.write_all(&buffer[..got])?;
        left -= got as u64;
        on_chunk(got as u64);
    }
    Ok(())
}

/// Copy everything that is left in `source` in chunks of `chunk` bytes.
///
/// # Errors
///
/// Read or write errors.
pub fn copy_all<R: Read, W: Write>(
    source: &mut R,
    sink: &mut W,
    chunk: usize,
    on_chunk: &mut dyn FnMut(u64),
) -> std::io::Result<()> {
    let mut buffer = vec![0_u8; chunk];
    loop {
        let got = read_fill(source, &mut buffer)?;
        if got == 0 {
            return Ok(());
        }
        sink.write_all(&buffer[..got])?;
        on_chunk(got as u64);
    }
}

fn token() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    hasher.write_u32(COUNTER.fetch_add(1, Ordering::Relaxed));
    format!("{:08x}", hasher.finish() & 0xFFFF_FFFF)
}

/// `<name>.tmp_moov.<pid>-<8 hex>` next to `path`: unique per call.
///
/// Two runs on the same file (the hook and a manager job) never share a working file. The name
/// keeps `.tmp` so the manager scanner skips it; a long name is shortened to fit one entry.
#[must_use]
pub fn moov_temp_path(path: &Path) -> PathBuf {
    let unique = format!(".tmp_moov.{}-{}", std::process::id(), token());
    let room = NAME_MAX.saturating_sub(unique.len());
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut end = name.len().min(room);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    path.with_file_name(format!("{}{unique}", &name[..end]))
}

/// Run `build` against a fresh temp file next to `path`, then rename it over `path`.
///
/// On any error the temp file is removed and `path` is untouched.
///
/// # Errors
///
/// The error of `build`, or of the rename.
pub fn via_temp(path: &Path, build: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let tmp = moov_temp_path(path);
    let result = build(&tmp).and_then(|()| std::fs::rename(&tmp, path).map_err(Mp4Error::from));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn copy_n_reports_every_chunk_and_stops_at_the_requested_count() {
        let source = vec![7_u8; 10_000];
        let mut sink = Vec::new();
        let mut ticks = Vec::new();
        copy_n(
            &mut Cursor::new(&source),
            &mut sink,
            9_000,
            4096,
            &mut |n| ticks.push(n),
        )
        .unwrap();
        assert_eq!(sink.len(), 9_000);
        assert_eq!(ticks, [4096, 4096, 808]);
    }

    #[test]
    fn copy_n_stops_quietly_at_the_end_of_a_short_source() {
        let mut sink = Vec::new();
        copy_n(
            &mut Cursor::new(vec![1_u8; 10]),
            &mut sink,
            100,
            4096,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(sink.len(), 10);
    }

    #[test]
    fn copy_all_reads_to_the_end_in_chunks() {
        let mut sink = Vec::new();
        let mut ticks = Vec::new();
        copy_all(
            &mut Cursor::new(vec![3_u8; 9_000]),
            &mut sink,
            4096,
            &mut |n| ticks.push(n),
        )
        .unwrap();
        assert_eq!((sink.len(), ticks), (9_000, vec![4096, 4096, 808]));
    }

    #[test]
    fn temp_names_are_unique_keep_dot_tmp_and_fit_255_bytes() {
        let dir = std::path::Path::new("/media");
        let first = moov_temp_path(&dir.join("Film.mp4"));
        let second = moov_temp_path(&dir.join("Film.mp4"));
        assert_ne!(first, second);
        let name = first.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("Film.mp4.tmp_moov."), "{name}");
        let pid = std::process::id();
        let rest = name
            .strip_prefix(&format!("Film.mp4.tmp_moov.{pid}-"))
            .unwrap();
        assert_eq!(rest.len(), 8);
        assert!(rest.chars().all(|c| c.is_ascii_hexdigit()));

        let long = format!("{}.mp4", "ä".repeat(125)); // 250 bytes
        let shortened = moov_temp_path(&dir.join(&long));
        let shortened = shortened.file_name().unwrap().to_str().unwrap();
        assert!(shortened.len() <= 255, "{}", shortened.len());
        assert!(shortened.contains(".tmp_moov."));
    }

    #[test]
    fn a_failing_rewrite_removes_its_temp_file_and_keeps_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Film.mp4");
        std::fs::write(&target, b"original").unwrap();
        let result = via_temp(&target, |tmp| {
            std::fs::write(tmp, b"half written")?;
            Err(Mp4Error::Io(std::io::Error::other("disk full")))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["Film.mp4"]);
    }

    #[test]
    fn a_successful_rewrite_replaces_the_original_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Film.mp4");
        std::fs::write(&target, b"original").unwrap();
        via_temp(&target, |tmp| Ok(std::fs::write(tmp, b"new content")?)).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new content");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
