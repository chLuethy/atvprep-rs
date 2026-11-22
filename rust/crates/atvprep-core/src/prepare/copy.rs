//! Copy a file in blocks and report the bytes, so the progress bar moves while a file is copied.

use std::io::{self, Read, Write};
use std::path::Path;

/// Bytes per block: large enough for throughput, small enough for a steady bar.
pub const BLOCK: usize = 4 * 1024 * 1024;

/// Copy `source` to `dest` (created or truncated) in blocks of `block` bytes; `progress` gets
/// `(done, total)` after every block. The permissions follow the source, like `std::fs::copy`.
///
/// # Errors
///
/// Any I/O error of reading, writing or the permissions.
pub fn copy_with_progress(
    source: &Path,
    dest: &Path,
    block: usize,
    mut progress: impl FnMut(u64, u64),
) -> io::Result<u64> {
    let mut reader = std::fs::File::open(source)?;
    let metadata = reader.metadata()?;
    let total = metadata.len();
    let mut writer = std::fs::File::create(dest)?;
    let mut buffer = vec![0_u8; block.max(1)];
    let mut done = 0_u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        done += read as u64;
        progress(done, total.max(done));
    }
    writer.flush()?;
    std::fs::set_permissions(dest, metadata.permissions())?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_is_byte_identical_and_reports_every_block() {
        let dir = tempfile::tempdir().unwrap();
        let (source, dest) = (dir.path().join("a.bin"), dir.path().join("b.bin"));
        let content: Vec<u8> = (0..1000_u32).map(|n| (n % 251) as u8).collect();
        std::fs::write(&source, &content).unwrap();

        let mut seen = Vec::new();
        let copied = copy_with_progress(&source, &dest, 300, |d, t| seen.push((d, t))).unwrap();

        assert_eq!(copied, 1000);
        assert_eq!(std::fs::read(&dest).unwrap(), content);
        assert_eq!(seen, [(300, 1000), (600, 1000), (900, 1000), (1000, 1000)]);
    }

    #[test]
    fn an_empty_file_is_copied_without_a_report() {
        let dir = tempfile::tempdir().unwrap();
        let (source, dest) = (dir.path().join("a.bin"), dir.path().join("b.bin"));
        std::fs::write(&source, b"").unwrap();
        let copied = copy_with_progress(&source, &dest, 8, |_, _| panic!("no block")).unwrap();
        assert_eq!(copied, 0);
        assert!(dest.exists());
    }
}
