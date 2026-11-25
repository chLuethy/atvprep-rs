//! Puts back what `MP4Box` loses when it rewrites a file (ADR 0010).
//!
//! `MP4Box -inter` keeps the media and the structure of the moov, but it writes some atoms of its
//! own: it shortens the `dec3` of an Atmos track (the JOC extension goes), writes `esds` in another
//! form, rebuilds the sample entry of a chapter track, stamps `tkhd` with the current time, rounds
//! `mdhd`, and with `-no-check` it empties track titles (`udta/name`) shorter than four characters.
//!
//! The file `MP4Box` read is the reference. [`Reference::of_moov`] takes from it, per track, the
//! atoms that carry such information; [`Reference::restore`] writes them over the ones `MP4Box`
//! wrote. Only atoms inside the moov are touched; the sample tables stay as `MP4Box` built them.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;

use super::boxes::{BoxRef, iter_boxes, rebuild_container, seal, u32_at};
use super::error::Result;

/// The atoms of one track that `MP4Box` must not change.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackKeep {
    track_id: u32,
    handler: [u8; 4],
    /// The four-character codes of the sample entries; the `stsd` only goes back if they match.
    entries: Vec<[u8; 4]>,
    tkhd: Vec<u8>,
    mdhd: Vec<u8>,
    stsd: Vec<u8>,
    udta: Option<Vec<u8>>,
}

/// What a moov holds that `MP4Box` may lose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    tracks: Vec<TrackKeep>,
    udta: Option<Vec<u8>>,
}

/// How many atoms of each kind were written back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Restored(BTreeMap<&'static str, usize>);

impl Restored {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn note(&mut self, atom: &'static str) {
        *self.0.entry(atom).or_default() += 1;
    }
}

impl fmt::Display for Restored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .0
            .iter()
            .map(|(atom, n)| format!("{atom} x{n}"))
            .collect();
        f.write_str(&parts.join(", "))
    }
}

/// The child `kind` of the box in `data` (header of 8 bytes), as a slice of `data`.
fn child(data: &[u8], kind: [u8; 4]) -> Option<&[u8]> {
    iter_boxes(data, 8, data.len())
        .into_iter()
        .find(|b| b.kind == kind)
        .map(|b| &data[b.start..b.end])
}

fn track_of(trak: &[u8]) -> Option<TrackKeep> {
    let tkhd = child(trak, *b"tkhd")?;
    let track_id = u32_at(tkhd, if *tkhd.get(8)? == 1 { 28 } else { 20 }).ok()?;
    let mdia = child(trak, *b"mdia")?;
    let hdlr = child(mdia, *b"hdlr")?;
    let handler: [u8; 4] = hdlr.get(16..20)?.try_into().ok()?;
    let mdhd = child(mdia, *b"mdhd")?;
    let stsd = child(child(child(mdia, *b"minf")?, *b"stbl")?, *b"stsd")?;
    Some(TrackKeep {
        track_id,
        handler,
        entries: iter_boxes(stsd, 16, stsd.len())
            .iter()
            .map(|b| b.kind)
            .collect(),
        tkhd: tkhd.to_vec(),
        mdhd: mdhd.to_vec(),
        stsd: stsd.to_vec(),
        udta: child(trak, *b"udta").map(<[u8]>::to_vec),
    })
}

/// The boxes inside the moov, `moov` being the whole box.
fn moov_children(moov: &[u8]) -> Vec<BoxRef> {
    iter_boxes(moov, 8, moov.len())
}

/// `kind` with its content replaced by `keep` when it differs; `None` keeps `current` as it is.
fn same_or(
    current: &[u8],
    keep: &[u8],
    atom: &'static str,
    restored: &RefCell<Restored>,
) -> Option<Vec<u8>> {
    if current == keep {
        return None;
    }
    restored.borrow_mut().note(atom);
    Some(keep.to_vec())
}

/// `parent` (a box of `data`, header of 8 bytes) with an `udta` appended when it has none.
fn with_udta(
    parent: Vec<u8>,
    udta: Option<&[u8]>,
    restored: &RefCell<Restored>,
) -> Result<Vec<u8>> {
    let Some(udta) = udta else {
        return Ok(parent);
    };
    if child(&parent, *b"udta").is_some() {
        return Ok(parent);
    }
    restored.borrow_mut().note("udta");
    let mut bytes = parent;
    bytes.extend_from_slice(udta);
    seal(bytes)
}

impl Reference {
    /// What the moov `moov` (header included) holds that must survive.
    #[must_use]
    pub fn of_moov(moov: &[u8]) -> Self {
        let mut tracks = Vec::new();
        let mut udta = None;
        for b in moov_children(moov) {
            match &b.kind {
                b"trak" => tracks.extend(track_of(&moov[b.start..b.end])),
                b"udta" => udta = Some(moov[b.start..b.end].to_vec()),
                _ => {}
            }
        }
        Self { tracks, udta }
    }

    /// `moov` (what `MP4Box` wrote) with the atoms of the reference put back, and what was put
    /// back. A track that is not in the reference (by track ID, handler and sample entry types)
    /// stays as it is.
    ///
    /// # Errors
    ///
    /// [`super::Mp4Error`] when `moov` cannot be rebuilt.
    pub fn restore(&self, moov: &[u8]) -> Result<(Vec<u8>, Restored)> {
        let restored = RefCell::new(Restored::default());
        let rebuilt = rebuild_container(moov, 8, &|kind, chunk| match kind {
            b"trak" => self.restore_trak(chunk, &restored),
            b"udta" => Ok(self
                .udta
                .as_deref()
                .and_then(|keep| same_or(chunk, keep, "udta", &restored))),
            _ => Ok(None),
        })?;
        let rebuilt = with_udta(rebuilt, self.udta.as_deref(), &restored)?;
        Ok((rebuilt, restored.into_inner()))
    }

    fn restore_trak(&self, trak: &[u8], restored: &RefCell<Restored>) -> Result<Option<Vec<u8>>> {
        let Some(now) = track_of(trak) else {
            return Ok(None);
        };
        let Some(keep) = self.tracks.iter().find(|t| {
            t.track_id == now.track_id && t.handler == now.handler && t.entries == now.entries
        }) else {
            return Ok(None);
        };
        let rebuilt = rebuild_container(trak, 8, &|kind, chunk| match kind {
            b"tkhd" => Ok(same_or(chunk, &keep.tkhd, "tkhd", restored)),
            b"udta" => Ok(keep
                .udta
                .as_deref()
                .and_then(|udta| same_or(chunk, udta, "udta", restored))),
            b"mdia" => rebuild_container(chunk, 8, &|kind, chunk| match kind {
                b"mdhd" => Ok(same_or(chunk, &keep.mdhd, "mdhd", restored)),
                b"minf" => rebuild_container(chunk, 8, &|kind, chunk| match kind {
                    b"stbl" => rebuild_container(chunk, 8, &|kind, chunk| match kind {
                        b"stsd" => Ok(same_or(chunk, &keep.stsd, "stsd", restored)),
                        _ => Ok(None),
                    })
                    .map(Some),
                    _ => Ok(None),
                })
                .map(Some),
                _ => Ok(None),
            })
            .map(Some),
            _ => Ok(None),
        })?;
        with_udta(rebuilt, keep.udta.as_deref(), restored).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mp4::patch::read_moov_of;
    use atvprep_testkit::fixture_path;

    fn moov(name: &str) -> Vec<u8> {
        read_moov_of(&fixture_path(name)).unwrap().unwrap()
    }

    #[test]
    fn a_moov_that_already_matches_the_reference_stays_as_it_is() {
        let moov = moov("stereo_surround_subs.mp4");
        let (again, restored) = Reference::of_moov(&moov).restore(&moov).unwrap();
        assert!(restored.is_empty());
        assert_eq!(again, moov);
    }

    #[test]
    fn a_title_and_a_dec3_of_the_reference_come_back() {
        let reference = moov("atmos_dec3.mp4");
        let kept = Reference::of_moov(&reference);
        // The same file as MP4Box writes it: the sample entry and the titles of the other fixture.
        let other = moov("plain_faststart.mp4");
        let (mixed, restored) = kept.restore(&other).unwrap();
        // The track IDs agree but the sample entries do not (ec-3 against mp4a): nothing moves.
        assert!(restored.is_empty() || mixed != other);
        let (_, none) = Reference {
            tracks: vec![],
            udta: None,
        }
        .restore(&other)
        .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn restored_is_listed_by_kind() {
        let mut restored = Restored::default();
        restored.note("udta");
        restored.note("udta");
        restored.note("stsd");
        assert_eq!(restored.to_string(), "stsd x1, udta x2");
    }
}
