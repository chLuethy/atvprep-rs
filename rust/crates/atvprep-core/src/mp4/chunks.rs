//! How long the chunks of a file are, read from the moov alone (ADR 0010).
//!
//! ffmpeg writes chunks of one or two samples, which Apple TV does not play smoothly; `MP4Box`
//! (`-inter 500`) writes chunks of about half a second. A track whose chunks are shorter than
//! [`MIN_MEAN_CHUNK_MS`] on average is a defect. Only audio and video count: a subtitle track is
//! thin by nature, and chunks that are too long are no defect. The media data is never read.

use std::path::Path;

use super::boxes::{BoxRef, find_path, iter_boxes, u32_at};
use super::error::Result;
use super::patch::read_moov_of;

/// The chunk duration `MP4Box` is asked for, in milliseconds.
pub const TARGET_CHUNK_MS: u32 = 500;

/// Id of the compliance rule that reports chunks that are too short.
pub const CHUNK_RULE: &str = "chunk-length";

/// Start of the plan reason a file with chunks that are too short gets.
pub const CHUNK_REASON: &str = "chunks too short for Apple TV";

/// Whether `reason` is the plan reason for chunks that are too short.
#[must_use]
pub fn is_chunk_reason(reason: &str) -> bool {
    reason.starts_with(CHUNK_REASON)
}

/// SQL `LIKE` pattern for a stored finding of [`CHUNK_RULE`], which is stored as
/// `[<severity>] <rule>: <message>`.
#[must_use]
pub fn chunk_finding_like() -> String {
    format!("[%] {CHUNK_RULE}: %")
}

/// A track whose mean chunk duration is below this, in milliseconds, has chunks that are too short.
pub const MIN_MEAN_CHUNK_MS: u64 = 250;

/// An audio or video track whose chunks are too short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortChunks {
    pub track_id: u32,
    /// The handler: `soun` or `vide`.
    pub handler: [u8; 4],
    pub chunks: u64,
    /// The mean chunk duration, in milliseconds (rounded down).
    pub mean_ms: u64,
}

impl ShortChunks {
    /// What a track of this kind is called to the operator.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        if matches!(&self.handler, b"vide") {
            "video"
        } else {
            "audio"
        }
    }
}

/// The audio and video tracks of `path` with too short chunks; empty for a file without a moov.
///
/// # Errors
///
/// I/O errors, or a moov in a form the patcher does not handle.
pub fn short_chunks(path: &Path) -> Result<Vec<ShortChunks>> {
    Ok(read_moov_of(path)?.map_or_else(Vec::new, |moov| short_chunks_in(&moov)))
}

/// [`short_chunks`] for a moov already in memory (header included).
#[must_use]
pub fn short_chunks_in(moov: &[u8]) -> Vec<ShortChunks> {
    iter_boxes(moov, 8, moov.len())
        .into_iter()
        .filter(|b| &b.kind == b"trak")
        .filter_map(|trak| short_in_track(moov, trak))
        .collect()
}

fn short_in_track(moov: &[u8], trak: BoxRef) -> Option<ShortChunks> {
    let inside = |path: &[&[u8; 4]]| find_path(moov, trak.start + 8, trak.end, path);
    let (hdlr, _) = inside(&[b"mdia", b"hdlr"])?;
    let handler = <[u8; 4]>::try_from(moov.get(hdlr + 16..hdlr + 20)?).ok()?;
    if &handler != b"soun" && &handler != b"vide" {
        return None;
    }
    let (mdhd, _) = inside(&[b"mdia", b"mdhd"])?;
    let (timescale, duration) = if *moov.get(mdhd + 8)? == 1 {
        let high = u64::from(u32_at(moov, mdhd + 32).ok()?);
        (
            u32_at(moov, mdhd + 28).ok()?,
            high << 32 | u64::from(u32_at(moov, mdhd + 36).ok()?),
        )
    } else {
        (
            u32_at(moov, mdhd + 20).ok()?,
            u64::from(u32_at(moov, mdhd + 24).ok()?),
        )
    };
    let (tkhd, _) = inside(&[b"tkhd"])?;
    let track_id = u32_at(
        moov,
        if *moov.get(tkhd + 8)? == 1 {
            tkhd + 28
        } else {
            tkhd + 20
        },
    )
    .ok()?;
    let offsets = inside(&[b"mdia", b"minf", b"stbl", b"stco"])
        .or_else(|| inside(&[b"mdia", b"minf", b"stbl", b"co64"]))?;
    let chunks = u64::from(u32_at(moov, offsets.0 + 12).ok()?);
    if chunks == 0 || duration == 0 || timescale == 0 {
        return None;
    }
    // mean < MIN_MEAN_CHUNK_MS  <=>  duration / timescale / chunks * 1000 < MIN_MEAN_CHUNK_MS
    let (ticks, limit) = (
        u128::from(duration) * 1000,
        u128::from(timescale) * u128::from(chunks),
    );
    (ticks < limit * u128::from(MIN_MEAN_CHUNK_MS)).then(|| ShortChunks {
        track_id,
        handler,
        chunks,
        mean_ms: u64::try_from(ticks / limit).unwrap_or(u64::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::super::boxes::build_box;
    use super::*;

    fn full(kind: [u8; 4], version: u8, body: &[u8]) -> Vec<u8> {
        let mut payload = vec![version, 0, 0, 0];
        payload.extend_from_slice(body);
        build_box(&kind, &payload)
    }

    fn be(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    /// A `trak` of `handler` with `chunks` chunks over `seconds` at timescale 1000.
    fn trak(id: u32, handler: [u8; 4], seconds: u32, chunks: u32) -> Vec<u8> {
        let tkhd = full(*b"tkhd", 0, &be(&[0, 0, id, 0, 0]));
        let mdhd = full(*b"mdhd", 0, &be(&[0, 0, 1000, seconds * 1000, 0]));
        let mut hdlr_body = be(&[0]);
        hdlr_body.extend_from_slice(&handler);
        hdlr_body.extend_from_slice(&[0; 13]);
        let hdlr = full(*b"hdlr", 0, &hdlr_body);
        let stco = full(
            *b"stco",
            0,
            &[be(&[chunks]), vec![0; chunks as usize * 4]].concat(),
        );
        let stbl = build_box(b"stbl", &stco);
        let minf = build_box(b"minf", &stbl);
        let mdia = build_box(b"mdia", &[mdhd, hdlr, minf].concat());
        build_box(b"trak", &[tkhd, mdia].concat())
    }

    fn moov(traks: &[Vec<u8>]) -> Vec<u8> {
        build_box(b"moov", &traks.concat())
    }

    #[test]
    fn chunks_under_a_quarter_second_on_average_are_short() {
        let found = short_chunks_in(&moov(&[
            trak(1, *b"vide", 20, 499),
            trak(2, *b"soun", 20, 42),
        ]));
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].track_id, found[0].kind(), found[0].chunks),
            (1, "video", 499)
        );
        assert_eq!(found[0].mean_ms, 40);
    }

    #[test]
    fn the_limit_itself_is_not_short() {
        assert!(short_chunks_in(&moov(&[trak(1, *b"soun", 20, 80)])).is_empty());
        assert_eq!(
            short_chunks_in(&moov(&[trak(1, *b"soun", 20, 81)])).len(),
            1
        );
    }

    #[test]
    fn long_chunks_are_no_defect() {
        assert!(short_chunks_in(&moov(&[trak(1, *b"vide", 600, 1)])).is_empty());
    }

    #[test]
    fn thin_tracks_such_as_subtitles_do_not_count() {
        let tracks = [*b"sbtl", *b"text", *b"tmcd"].map(|handler| trak(3, handler, 7200, 5000));
        assert!(short_chunks_in(&moov(&tracks)).is_empty());
    }

    #[test]
    fn a_track_without_chunks_or_duration_says_nothing() {
        assert!(
            short_chunks_in(&moov(&[trak(1, *b"vide", 20, 0), trak(2, *b"soun", 0, 5)])).is_empty()
        );
        assert!(short_chunks_in(&[]).is_empty());
    }
}
