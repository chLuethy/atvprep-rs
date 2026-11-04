//! A minimal atom walker for the faststart and fragmentation checks (port of the atom helpers of
//! `atvprep/compliance.py`). Only atom headers are read.

use std::fmt::{self, Write as _};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// A malformed atom header (Python raised `ValueError` with this text) or an I/O failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomError(pub String);

impl fmt::Display for AtomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AtomError {}

impl From<std::io::Error> for AtomError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

/// Which of the two comes first at the top level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Top {
    Moov,
    Mdat,
}

/// `repr(bytes)` as Python prints it: `b'moov'`, `b"a'\xffb"`.
#[must_use]
pub fn bytes_repr(bytes: &[u8]) -> String {
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = format!("b{quote}");
    for &byte in bytes {
        match byte {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b if char::from(b) == quote => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(char::from(byte)),
            _ => {
                let _ = write!(out, "\\x{byte:02x}");
            }
        }
    }
    out.push(quote);
    out
}

fn read_up_to(file: &mut File, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// `(total size, type, header size)` of the atom at `offset`; `None` at end of file and for an atom
/// that runs to the end of the file (size 0), which ends the walk.
fn read_atom_header(file: &mut File, offset: u64) -> Result<Option<(u64, String, u64)>, AtomError> {
    file.seek(SeekFrom::Start(offset))?;
    let mut header = [0_u8; 8];
    if read_up_to(file, &mut header)? < 8 {
        return Ok(None);
    }
    let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let kind_bytes = [header[4], header[5], header[6], header[7]];
    if !kind_bytes.is_ascii() {
        return Err(AtomError(format!(
            "non-ASCII atom type at offset {offset}: {}",
            bytes_repr(&kind_bytes)
        )));
    }
    let kind = String::from_utf8_lossy(&kind_bytes).into_owned();
    let (total, header_size) = match size32 {
        1 => {
            let mut extended = [0_u8; 8];
            if read_up_to(file, &mut extended)? < 8 {
                return Err(AtomError(format!(
                    "truncated 64-bit size for {kind} at {offset}"
                )));
            }
            (u64::from_be_bytes(extended), 16)
        }
        0 => return Ok(None),
        n => (u64::from(n), 8),
    };
    if total < header_size {
        return Err(AtomError(format!(
            "{kind} at {offset}: size {total} < header {header_size}"
        )));
    }
    if offset.checked_add(total).is_none() {
        return Err(AtomError(format!(
            "{kind} at {offset}: size {total} overflows the file offset"
        )));
    }
    Ok(Some((total, kind, header_size)))
}

/// Whichever of `moov` and `mdat` appears first at the top level; `None` if neither is found.
///
/// # Errors
///
/// [`AtomError`] for a malformed header or an I/O failure.
pub fn first_moov_vs_mdat(path: &Path) -> Result<Option<Top>, AtomError> {
    let mut file = File::open(path)?;
    let file_size = std::fs::metadata(path)?.len();
    let mut cursor = 0;
    while cursor < file_size {
        let Some((total, kind, _)) = read_atom_header(&mut file, cursor)? else {
            return Ok(None);
        };
        match kind.as_str() {
            "moov" => return Ok(Some(Top::Moov)),
            "mdat" => return Ok(Some(Top::Mdat)),
            _ => {}
        }
        cursor += total;
    }
    Ok(None)
}

/// Whether the first `moov` atom has a `mvex` child; `false` without a `moov`.
///
/// # Errors
///
/// [`AtomError`] for a malformed header or an I/O failure.
pub fn is_fragmented(path: &Path) -> Result<bool, AtomError> {
    let mut file = File::open(path)?;
    let file_size = std::fs::metadata(path)?.len();
    let mut cursor = 0;
    let mut moov = None;
    while cursor < file_size {
        let Some((total, kind, header_size)) = read_atom_header(&mut file, cursor)? else {
            break;
        };
        if kind == "moov" {
            moov = Some((cursor, total, header_size));
            break;
        }
        cursor += total;
    }
    let Some((start, size, header_size)) = moov else {
        return Ok(false);
    };
    let (mut child, end) = (start + header_size, start + size);
    while child < end {
        let Some((total, kind, _)) = read_atom_header(&mut file, child)? else {
            break;
        };
        if kind == "mvex" {
            return Ok(true);
        }
        child += total;
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use atvprep_testkit::{fixture_path, hex_decode, load_golden};
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;

    #[derive(Deserialize)]
    struct Input {
        file: Option<String>,
        hex: Option<String>,
    }

    fn first_json(result: &Result<Option<Top>, AtomError>) -> Value {
        match result {
            Ok(top) => {
                json!({"ok": top.map(|t| match t { Top::Moov => "moov", Top::Mdat => "mdat" })})
            }
            Err(error) => json!({"raises": "ValueError", "message": error.0}),
        }
    }

    fn fragmented_json(result: &Result<bool, AtomError>) -> Value {
        match result {
            Ok(value) => json!({"ok": value}),
            Err(error) => json!({"raises": "ValueError", "message": error.0}),
        }
    }

    #[test]
    fn atom_walks_match_the_python_oracle() {
        let cases = load_golden::<Input, Value>("compliance_atoms");
        assert!(cases.len() > 40);
        let dir = tempfile::tempdir().unwrap();
        for case in cases {
            let path = match (&case.input.file, &case.input.hex) {
                (Some(file), _) => fixture_path(file),
                (None, Some(hex)) => {
                    let path = dir.path().join("x.mp4");
                    std::fs::write(&path, hex_decode(hex)).unwrap();
                    path
                }
                _ => panic!("{}: no input", case.name),
            };
            let first = first_json(&first_moov_vs_mdat(&path));
            let fragmented = fragmented_json(&is_fragmented(&path));
            let name = &case.name;
            assert_eq!(first, case.expected["first"], "{name} first");
            assert_eq!(fragmented, case.expected["fragmented"], "{name} fragmented");
        }
    }

    #[test]
    fn a_size_of_zero_ends_the_walk_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.mp4");
        let mut data = vec![0, 0, 0, 0];
        data.extend_from_slice(b"mdat");
        data.extend_from_slice(&[0; 8]);
        std::fs::write(&path, data).unwrap();
        assert_eq!(first_moov_vs_mdat(&path).unwrap(), None);
        assert!(!is_fragmented(&path).unwrap());
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        assert!(first_moov_vs_mdat(std::path::Path::new("/definitely/not/here.mp4")).is_err());
        assert!(is_fragmented(std::path::Path::new("/definitely/not/here.mp4")).is_err());
    }

    #[test]
    fn bytes_are_shown_the_way_python_prints_them() {
        assert_eq!(bytes_repr(b"moov"), "b'moov'");
        assert_eq!(bytes_repr(b"a'\xffb"), "b\"a'\\xffb\"");
        assert_eq!(bytes_repr(b"'\"\xff\t"), "b'\\'\"\\xff\\t'");
        assert_eq!(bytes_repr(b"\\\n\r\0"), "b'\\\\\\n\\r\\x00'");
    }

    #[test]
    fn a_box_whose_64_bit_size_wraps_the_offset_is_rejected_not_looped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wrap.mp4");
        let mut bytes = vec![0, 0, 0, 16, b'f', b't', b'y', b'p', 0, 0, 0, 0, 0, 0, 0, 0];
        bytes.extend([0, 0, 0, 1, b'f', b'r', b'e', b'e']);
        bytes.extend((u64::MAX - 15).to_be_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(first_moov_vs_mdat(&path).is_err());
        assert!(is_fragmented(&path).is_err());
    }
}
