//! `stco`/`co64` chunk offsets: shifting them and moving the moov to the front (port of the offset
//! half of `atvprep/mp4patch.py`).
//!
//! Offsets are 64-bit throughout: a 32-bit `stco` whose offsets no longer fit becomes a `co64`, so
//! the moov can grow by the widening, and files beyond 4 GiB stay patchable.

use super::boxes::{rebuild_container, u32_at};
use super::error::{Mp4Error, Result};

/// Box types that can contain `stco`/`co64` boxes, directly or indirectly.
const CONTAINERS: [&[u8; 4]; 8] = [
    b"moov", b"trak", b"mdia", b"minf", b"stbl", b"dinf", b"edts", b"udta",
];

fn is_container(kind: [u8; 4]) -> bool {
    CONTAINERS.contains(&&kind)
}

/// The number of entries of a chunk offset box, checked against the bytes the box really has.
fn entry_count(chunk: &[u8], width: usize) -> Result<usize> {
    let count = u32_at(chunk, 12)? as usize;
    let needed = count.checked_mul(width).and_then(|n| n.checked_add(16));
    if needed.is_none_or(|n| n > chunk.len()) {
        return Err(Mp4Error::Malformed(
            "chunk offset count is larger than its box",
        ));
    }
    Ok(count)
}

/// Add `delta` to every `stco`/`co64` chunk offset in `moov` in place.
///
/// Either every offset moves or none does: the moov is left as it was on an error.
///
/// # Errors
///
/// [`Mp4Error::Unsupported`] when a 32-bit offset would no longer fit (use [`shift_chunk_offsets`],
/// which widens it); [`Mp4Error::Malformed`] for a box smaller than its header or an entry count
/// larger than its box.
pub fn add_to_chunk_offsets(moov: &mut [u8], delta: u64) -> Result<()> {
    let mut work = moov.to_vec();
    let end = work.len();
    scan(&mut work, 8, end, delta)?;
    moov.copy_from_slice(&work);
    Ok(())
}

fn scan(data: &mut [u8], start: usize, end: usize, delta: u64) -> Result<()> {
    let mut at = start;
    while at + 8 <= end {
        let mut size = u32_at(data, at)? as usize;
        if size == 0 {
            size = end - at;
        }
        if size < 8 {
            return Err(Mp4Error::Malformed("a box is smaller than its header"));
        }
        let box_end = at.saturating_add(size).min(end);
        let kind: [u8; 4] = data[at + 4..at + 8]
            .try_into()
            .map_err(|_| Mp4Error::Malformed("short header"))?;
        match &kind {
            b"stco" | b"co64" => {
                let wide = &kind == b"co64";
                let width = if wide { 8 } else { 4 };
                let count = entry_count(&data[at..box_end], width)?;
                for i in 0..count {
                    let pos = at + 16 + i * width;
                    if wide {
                        let old = u64::from_be_bytes(
                            data[pos..pos + 8]
                                .try_into()
                                .map_err(|_| Mp4Error::Malformed("short entry"))?,
                        );
                        let new = old
                            .checked_add(delta)
                            .ok_or(Mp4Error::Unsupported("chunk offset overflows 64 bits"))?;
                        data[pos..pos + 8].copy_from_slice(&new.to_be_bytes());
                    } else {
                        let old = u64::from(u32_at(data, pos)?);
                        let new = u32::try_from(old + delta).map_err(|_| {
                            Mp4Error::Unsupported("chunk offset no longer fits 32 bits")
                        })?;
                        data[pos..pos + 4].copy_from_slice(&new.to_be_bytes());
                    }
                }
            }
            kind if is_container(*kind) => scan(data, at + 8, box_end, delta)?,
            _ => {}
        }
        at += size;
    }
    Ok(())
}

/// `chunk` (a box) with every `stco`/`co64` offset below it moved by `delta`.
///
/// A 32-bit `stco` whose offsets no longer fit becomes a 64-bit `co64`, so the result can be
/// larger than `chunk`; the containers above it are resized to match.
///
/// # Errors
///
/// [`Mp4Error::Malformed`] for an entry count larger than its box or trailing bytes after the
/// entries; [`Mp4Error::Unsupported`] when an offset overflows 64 bits.
pub fn shift_chunk_offsets(chunk: &[u8], delta: u64) -> Result<Vec<u8>> {
    let kind: [u8; 4] = chunk
        .get(4..8)
        .and_then(|k| k.try_into().ok())
        .ok_or(Mp4Error::Malformed("box without a header"))?;
    match &kind {
        b"stco" => {
            let count = entry_count(chunk, 4)?;
            if chunk.len() != 16 + 4 * count {
                return Err(Mp4Error::Malformed("stco has bytes after its entries"));
            }
            let offsets = (0..count)
                .map(|i| Ok(u64::from(u32_at(chunk, 16 + 4 * i)?) + delta))
                .collect::<Result<Vec<u64>>>()?;
            if offsets.iter().any(|&o| o > u64::from(u32::MAX)) {
                let size = u32::try_from(16 + 8 * count)
                    .map_err(|_| Mp4Error::Unsupported("box larger than 4 GiB"))?;
                let mut out = size.to_be_bytes().to_vec();
                out.extend_from_slice(b"co64");
                out.extend_from_slice(&chunk[8..12]);
                out.extend_from_slice(&chunk[12..16]);
                for offset in &offsets {
                    out.extend_from_slice(&offset.to_be_bytes());
                }
                return Ok(out);
            }
            let mut out = chunk[..16].to_vec();
            for offset in offsets {
                out.extend_from_slice(
                    &u32::try_from(offset)
                        .map_err(|_| Mp4Error::Unsupported("offset"))?
                        .to_be_bytes(),
                );
            }
            Ok(out)
        }
        b"co64" => {
            let count = entry_count(chunk, 8)?;
            if chunk.len() != 16 + 8 * count {
                return Err(Mp4Error::Malformed("co64 has bytes after its entries"));
            }
            let mut out = chunk[..16].to_vec();
            for i in 0..count {
                let at = 16 + 8 * i;
                let old = u64::from_be_bytes(
                    chunk[at..at + 8]
                        .try_into()
                        .map_err(|_| Mp4Error::Malformed("short entry"))?,
                );
                let new = old
                    .checked_add(delta)
                    .ok_or(Mp4Error::Unsupported("chunk offset overflows 64 bits"))?;
                out.extend_from_slice(&new.to_be_bytes());
            }
            Ok(out)
        }
        kind if is_container(*kind) => rebuild_container(chunk, 8, &|child, bytes| {
            if matches!(child, b"stco" | b"co64") || is_container(*child) {
                shift_chunk_offsets(bytes, delta).map(Some)
            } else {
                Ok(None)
            }
        }),
        _ => Ok(chunk.to_vec()),
    }
}

/// `moov` as it must read once it sits in front of the media: every offset moves by its own size.
///
/// Widening an `stco` makes the moov bigger, which moves the offsets further, so the shift is
/// repeated until the moov's size and the shift agree.
///
/// # Errors
///
/// As [`shift_chunk_offsets`]; [`Mp4Error::Malformed`] when the size does not settle in 8 rounds.
pub fn moov_for_front(moov: &[u8]) -> Result<Vec<u8>> {
    let mut delta = moov.len() as u64;
    for _ in 0..8 {
        let shifted = shift_chunk_offsets(moov, delta)?;
        if shifted.len() as u64 == delta {
            return Ok(shifted);
        }
        delta = shifted.len() as u64;
    }
    Err(Mp4Error::Malformed(
        "chunk offsets did not settle while moving the moov to the front",
    ))
}

#[cfg(test)]
mod tests {
    use atvprep_testkit::{load_golden, sha256_hex};
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::super::tags::tests_support::moov_of;
    use super::*;

    #[derive(Deserialize)]
    struct Input {
        file: String,
        delta: Option<u64>,
    }

    fn outcome<E>(result: std::result::Result<Vec<u8>, E>) -> Value {
        match result {
            Ok(bytes) => json!({"sha": sha256_hex(&bytes), "len": bytes.len()}),
            Err(_) => json!({"raises": true}),
        }
    }

    #[test]
    fn offsets_match_the_python_oracle() {
        let cases = load_golden::<Input, Value>("mp4_offsets");
        assert!(cases.len() > 100);
        for case in cases {
            let moov = moov_of(&case.input.file);
            let name = &case.name;
            if let Some(delta) = case.input.delta {
                let mut added = moov.clone();
                let result = add_to_chunk_offsets(&mut added, delta).map(|()| added);
                assert_eq!(outcome(result), case.expected["add"], "{name} add");
                assert_eq!(
                    outcome(shift_chunk_offsets(&moov, delta)),
                    case.expected["shift"],
                    "{name} shift"
                );
            } else {
                assert_eq!(
                    outcome(moov_for_front(&moov)),
                    case.expected["front"],
                    "{name} front"
                );
            }
        }
    }

    #[test]
    fn stco_crossing_four_gib_becomes_co64_and_the_moov_grows() {
        let moov = moov_of("syn_stco_overflow.mp4");
        let shifted = shift_chunk_offsets(&moov, 1000).unwrap();
        assert_eq!(
            shifted.len(),
            moov.len() + 4,
            "one stco entry widened from 4 to 8 bytes"
        );
        assert!(shifted.windows(4).any(|w| w == b"co64"));
        let mut in_place = moov.clone();
        assert!(
            add_to_chunk_offsets(&mut in_place, 1000).is_err(),
            "the in-place add cannot widen"
        );
        assert_eq!(in_place, moov, "a failed add leaves the moov as it was");
    }

    #[test]
    fn moov_for_front_converges_and_its_size_equals_the_shift() {
        for name in [
            "plain_moov_last.mp4",
            "syn_stco_overflow.mp4",
            "syn_co64.mp4",
        ] {
            let moov = moov_of(name);
            let front = moov_for_front(&moov).unwrap();
            assert_eq!(
                shift_chunk_offsets(&moov, front.len() as u64).unwrap(),
                front,
                "{name}"
            );
        }
    }

    #[test]
    fn an_entry_count_larger_than_the_box_is_an_error_not_a_panic() {
        let moov = moov_of("syn_huge_entry_count.mp4");
        assert!(shift_chunk_offsets(&moov, 1).is_err());
        let mut copy = moov.clone();
        assert!(add_to_chunk_offsets(&mut copy, 1).is_err());
        assert_eq!(copy, moov);
        assert!(moov_for_front(&moov).is_err());
    }

    #[test]
    fn a_child_smaller_than_its_header_stops_nothing_silently() {
        let moov = moov_of("syn_tiny_size_box.mp4");
        let mut copy = moov.clone();
        assert!(add_to_chunk_offsets(&mut copy, 1).is_err());
        assert!(shift_chunk_offsets(&moov, 1).is_err());
    }
}
