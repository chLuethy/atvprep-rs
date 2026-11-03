//! Bounds-checked box primitives over byte slices and over seekable files.
//!
//! Nothing here indexes a slice without checking, and every offset sum is checked: a hostile or
//! truncated file yields `None`/`Err`, never a panic or an endless loop.

use std::io::{Read, Seek, SeekFrom};

use super::error::{Mp4Error, Result};

/// A box found inside a byte slice: its type and its `[start, end)` range (header included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxRef {
    pub kind: [u8; 4],
    pub start: usize,
    pub end: usize,
}

/// A big-endian `u32` at `offset`.
///
/// # Errors
///
/// [`Mp4Error::Malformed`] when fewer than four bytes remain.
pub fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or(Mp4Error::Malformed("offset overflow"))?;
    let bytes = data
        .get(offset..end)
        .ok_or(Mp4Error::Malformed("read past the end of the data"))?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// The boxes in `data[start..end]`; stops at a malformed header. A size of 0 runs to `end`, a
/// size beyond `end` is clamped to it.
#[must_use]
pub fn iter_boxes(data: &[u8], start: usize, end: usize) -> Vec<BoxRef> {
    let end = end.min(data.len());
    let mut found = Vec::new();
    let mut at = start;
    while at
        .checked_add(8)
        .is_some_and(|header_end| header_end <= end)
    {
        let Ok(size) = u32_at(data, at) else {
            break;
        };
        let mut size = size as usize;
        if size == 0 {
            size = end - at;
        }
        if size < 8 {
            break;
        }
        let Some(kind) = data
            .get(at + 4..at + 8)
            .and_then(|k| <[u8; 4]>::try_from(k).ok())
        else {
            break;
        };
        found.push(BoxRef {
            kind,
            start: at,
            end: at.saturating_add(size).min(end),
        });
        let Some(next) = at.checked_add(size) else {
            break;
        };
        at = next;
    }
    found
}

/// Descend through nested boxes by type and return the `(start, end)` of the last one.
///
/// An empty path returns `(start, end)` unchanged.
#[must_use]
pub fn find_path(
    data: &[u8],
    start: usize,
    end: usize,
    path: &[&[u8; 4]],
) -> Option<(usize, usize)> {
    let (mut start, mut end) = (start, end);
    for (depth, kind) in path.iter().enumerate() {
        let found = iter_boxes(data, start, end)
            .into_iter()
            .find(|b| &b.kind == *kind)?;
        (start, end) = (found.start, found.end);
        if depth + 1 < path.len() {
            start = start.checked_add(8)?; // step into the container
        }
    }
    Some((start, end))
}

/// A box with the 32-bit size form.
///
/// # Panics
///
/// Panics when the payload does not fit a 32-bit box; callers build boxes from data that came out
/// of a 32-bit box.
#[must_use]
pub fn build_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let size = u32::try_from(payload.len() + 8).expect("box fits the 32-bit size form");
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// `bytes` with its first four bytes set to its own length.
///
/// # Errors
///
/// [`Mp4Error`] when `bytes` is shorter than a header or longer than a 32-bit box can describe.
pub fn seal(mut bytes: Vec<u8>) -> Result<Vec<u8>> {
    let size =
        u32::try_from(bytes.len()).map_err(|_| Mp4Error::Unsupported("box larger than 4 GiB"))?;
    bytes
        .get_mut(0..4)
        .ok_or(Mp4Error::Malformed("box without a header"))?
        .copy_from_slice(&size.to_be_bytes());
    Ok(bytes)
}

/// What to do with a child box while rebuilding a container: replacement bytes, or `None` to keep it.
pub type Transform<'a> = &'a dyn Fn(&[u8; 4], &[u8]) -> Result<Option<Vec<u8>>>;

/// A copy of `data` with child boxes replaced by `transform`; the box size field is updated.
///
/// `header_size` is the number of leading bytes kept before the children begin (8 for a plain
/// box, 16 for a full box such as `stsd`, 36 for an audio sample entry such as `ec-3`).
///
/// # Errors
///
/// [`Mp4Error::Malformed`] for a child smaller than its header (Python would silently cut the rest).
pub fn rebuild_container(
    data: &[u8],
    header_size: usize,
    transform: Transform<'_>,
) -> Result<Vec<u8>> {
    let mut result = data
        .get(..header_size)
        .ok_or(Mp4Error::Malformed("box shorter than its header"))?
        .to_vec();
    let mut at = header_size;
    while at + 8 <= data.len() {
        let mut size = u32_at(data, at)? as usize;
        if size == 0 {
            size = data.len() - at;
        }
        if size < 8 {
            return Err(Mp4Error::Malformed(
                "a child box is smaller than its header",
            ));
        }
        let chunk = &data[at..(at + size).min(data.len())];
        let kind: [u8; 4] = data[at + 4..at + 8]
            .try_into()
            .map_err(|_| Mp4Error::Malformed("short header"))?;
        match transform(&kind, chunk)? {
            Some(replacement) => result.extend_from_slice(&replacement),
            None => result.extend_from_slice(chunk),
        }
        at += size;
    }
    seal(result)
}

fn read_up_to<R: Read>(reader: &mut R, count: usize) -> Result<Vec<u8>> {
    let mut buffer = Vec::with_capacity(count);
    reader.take(count as u64).read_to_end(&mut buffer)?;
    Ok(buffer)
}

/// The `(size, type)` of the box at the current position; `None` at end of file or when the
/// 64-bit size is cut off. A size of 1 means the 64-bit form follows.
///
/// # Errors
///
/// I/O errors of the reader.
pub fn read_box_header<R: Read>(reader: &mut R) -> Result<Option<(u64, [u8; 4])>> {
    let header = read_up_to(reader, 8)?;
    if header.len() < 8 {
        return Ok(None);
    }
    let mut size = u64::from(u32::from_be_bytes([
        header[0], header[1], header[2], header[3],
    ]));
    let kind = [header[4], header[5], header[6], header[7]];
    if size == 1 {
        let extended = read_up_to(reader, 8)?;
        let Ok(bytes) = <[u8; 8]>::try_from(extended.as_slice()) else {
            return Ok(None);
        };
        size = u64::from_be_bytes(bytes);
    }
    Ok(Some((size, kind)))
}

/// The offset of the first box of type `target` in `[start, end)`.
///
/// # Errors
///
/// I/O errors of the reader.
pub fn find_box<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
    target: &[u8; 4],
) -> Result<Option<u64>> {
    let mut at = start;
    while at
        .checked_add(8)
        .is_some_and(|header_end| header_end <= end)
    {
        reader.seek(SeekFrom::Start(at))?;
        let Some((mut size, kind)) = read_box_header(reader)? else {
            break;
        };
        if size == 0 {
            size = end - at;
        }
        if size < 8 {
            break;
        }
        if &kind == target {
            return Ok(Some(at));
        }
        let Some(next) = at.checked_add(size) else {
            break;
        };
        at = next;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use atvprep_testkit::load_golden;
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;

    #[derive(Deserialize)]
    struct Input {
        op: String,
        hex: String,
        #[serde(default)]
        start: usize,
        #[serde(default)]
        end: usize,
        #[serde(default)]
        path: Vec<String>,
        #[serde(default)]
        target: String,
    }

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    /// A four-character type; anything else (Python compares bytes of other lengths, never equal)
    /// becomes a type no box has.
    fn kind(text: &str) -> [u8; 4] {
        text.as_bytes().try_into().unwrap_or([0xff; 4])
    }

    #[test]
    fn primitives_match_the_python_oracle() {
        let cases = load_golden::<Input, Value>("mp4_boxes");
        assert!(cases.len() > 250);
        for case in cases {
            let input = &case.input;
            let data = unhex(&input.hex);
            let actual = match input.op.as_str() {
                "iter" => json!(
                    iter_boxes(&data, input.start, input.end)
                        .iter()
                        .map(|b| json!([String::from_utf8_lossy(&b.kind), b.start, b.end]))
                        .collect::<Vec<_>>()
                ),
                "path" => {
                    let path: Vec<[u8; 4]> = input.path.iter().map(|p| kind(p)).collect();
                    let refs: Vec<&[u8; 4]> = path.iter().collect();
                    let found = find_path(&data, input.start, input.end, &refs);
                    if input.path.is_empty() {
                        assert_eq!(found, Some((input.start, input.end)), "{}", case.name);
                        continue;
                    }
                    json!(found.map(|(s, e)| [s, e]))
                }
                "find_box" => {
                    let mut cursor = Cursor::new(&data);
                    let found = find_box(
                        &mut cursor,
                        input.start as u64,
                        input.end as u64,
                        &kind(&input.target),
                    )
                    .unwrap();
                    json!(found)
                }
                "header" => {
                    let mut cursor = Cursor::new(&data);
                    json!(
                        read_box_header(&mut cursor)
                            .unwrap()
                            .map(|(size, k)| json!([size, String::from_utf8_lossy(&k)]))
                    )
                }
                other => panic!("unknown op {other}"),
            };
            assert_eq!(actual, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn a_size_beyond_the_buffer_is_clamped_like_python() {
        let mut data = 1000_u32.to_be_bytes().to_vec();
        data.extend_from_slice(b"free");
        data.extend_from_slice(&[0; 10]);
        let boxes = iter_boxes(&data, 0, data.len());
        assert_eq!((boxes[0].start, boxes[0].end), (0, data.len()));
    }

    #[test]
    fn a_header_below_8_bytes_stops_iteration() {
        let mut data = 4_u32.to_be_bytes().to_vec();
        data.extend_from_slice(b"free");
        data.extend_from_slice(&build_box(b"skip", &[]));
        assert!(iter_boxes(&data, 0, data.len()).is_empty());
    }

    #[test]
    fn a_huge_start_or_end_does_not_overflow_or_panic() {
        let data = build_box(b"free", &[1, 2, 3]);
        assert!(iter_boxes(&data, usize::MAX - 3, usize::MAX).is_empty());
        assert!(find_path(&data, usize::MAX - 3, usize::MAX, &[b"free"]).is_none());
        assert!(u32_at(&data, usize::MAX).is_err());
    }

    #[test]
    fn find_box_terminates_on_boxes_that_claim_to_start_past_the_end() {
        let mut data = u32::MAX.to_be_bytes().to_vec();
        data.extend_from_slice(b"free");
        data.extend_from_slice(&[0; 8]);
        let mut cursor = Cursor::new(&data);
        assert_eq!(
            find_box(&mut cursor, 0, data.len() as u64, b"moov").unwrap(),
            None
        );
    }
}
