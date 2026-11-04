//! In-memory moov transforms: `elng`, `chtx`, forced flags and Apple track references (port of the
//! tag half of `atvprep/mp4patch.py`).
//!
//! `AVFoundation` identifies subtitle languages through `elng` (BCP-47 tags) inside `mdia`; SDH tracks
//! additionally need `chtx` accessibility atoms in `udta`. The same pass writes `elng` for audio and
//! video from their own `mdhd` language, `tref/fall` from every surround track to the first stereo
//! track of its language, and `tref/folw` / `tref/forc` plus the tx3g forced flags for forced
//! subtitles. Every function returns new bytes; nothing here touches a file.

use std::collections::{BTreeMap, BTreeSet};

use super::boxes::{BoxRef, build_box, find_path, iter_boxes, seal, u32_at};
use super::error::{Mp4Error, Result};
use crate::tracklinks::{AudioRef, SubRef, plan_links};

/// `(language, is_sdh, is_forced)` of one subtitle track, in file order.
pub type SubInfo = (String, bool, bool);

const SUBTITLE_HANDLERS: [&[u8; 4]; 4] = [b"text", b"sbtl", b"subt", b"clcp"];
const AV_HANDLERS: [&[u8; 4]; 2] = [b"vide", b"soun"];
/// tx3g displayFlags bits Apple sets on forced-only tracks: "forced present" | "all forced".
const TX3G_FORCED_FLAGS: u32 = 0xC000_0000;
const CHTX_TRANSCRIBES: &[u8] = b"public.accessibility.transcribes-spoken-dialog";
const CHTX_DESCRIBES: &[u8] = b"public.accessibility.describes-music-and-sound";
const FALL: [u8; 4] = *b"fall";
const FOLW: [u8; 4] = *b"folw";
const FORC: [u8; 4] = *b"forc";

/// ISO 639-2 (bibliographic and terminological) to BCP-47, from Apple iTunes M4V subtitle tracks.
pub const BCP47: &[(&str, &str)] = &[
    ("eng", "en-US"),
    ("deu", "de-DE"),
    ("ger", "de-DE"),
    ("fra", "fr-FR"),
    ("fre", "fr-FR"),
    ("spa", "es-419"),
    ("ita", "it-IT"),
    ("jpn", "ja"),
    ("kor", "ko"),
    ("zho", "cmn-Hans"),
    ("chi", "cmn-Hans"),
    ("por", "pt-BR"),
    ("ara", "ar"),
    ("bul", "bg"),
    ("ces", "cs"),
    ("cze", "cs"),
    ("dan", "da"),
    ("ell", "el"),
    ("gre", "el"),
    ("est", "et"),
    ("fin", "fi"),
    ("heb", "he"),
    ("hin", "hi"),
    ("hrv", "hr"),
    ("hun", "hu"),
    ("ind", "id"),
    ("isl", "is"),
    ("ice", "is"),
    ("lit", "lt"),
    ("lav", "lv"),
    ("msa", "ms"),
    ("may", "ms"),
    ("nld", "nl"),
    ("dut", "nl"),
    ("nor", "no"),
    ("pol", "pl"),
    ("ron", "ro"),
    ("rum", "ro"),
    ("rus", "ru"),
    ("slk", "sk"),
    ("slo", "sk"),
    ("slv", "sl"),
    ("srp", "sr-Latn"),
    ("swe", "sv"),
    ("tam", "ta"),
    ("tel", "te"),
    ("tha", "th"),
    ("tur", "tr"),
    ("ukr", "uk"),
    ("vie", "vi"),
    ("yue", "yue-Hant"),
];

/// What the patched file carries, counted from the written moov.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PatchSummary {
    /// Tracks with an extended language tag.
    pub elng: u32,
    /// Surround audio tracks with a stereo fallback.
    pub fall: u32,
    /// Audio tracks following a forced subtitle track.
    pub folw: u32,
    /// Full subtitle tracks pointing at their forced variant.
    pub forc: u32,
    /// Subtitle tracks marked forced-only in tx3g.
    pub forced_flags: u32,
}

/// Result of a read-only check: what the file carries, what it should, and whether they differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagCheck {
    pub current: PatchSummary,
    pub wanted: PatchSummary,
    pub needs_patch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackSignature {
    track_id: Option<u32>,
    elng: Option<Vec<u8>>,
    refs: BTreeMap<[u8; 4], Vec<u32>>,
    forced: bool,
    has_chtx: bool,
}

/// Everything the tag patch manages, per track.
pub type TagSignature = Vec<TrackSignature>;

fn bcp47_of(lang: &str) -> String {
    BCP47
        .iter()
        .find(|&&(k, _)| k == lang)
        .map_or(lang, |&(_, v)| v)
        .to_owned()
}

/// The BCP-47 tag Apple writes in `elng` for an ISO 639-2 code; `None` for undetermined.
#[must_use]
pub fn elng_tag(lang: &str) -> Option<String> {
    (!lang.is_empty() && lang != "und").then(|| bcp47_of(lang))
}

fn clamp(data: &[u8], start: usize, end: usize) -> &[u8] {
    let end = end.min(data.len());
    &data[start.min(end)..end]
}

/// Children of a box region; a child smaller than its header is an error, not a silent cut.
fn strict_children(data: &[u8], start: usize, end: usize) -> Result<Vec<BoxRef>> {
    let end = end.min(data.len());
    let boxes = iter_boxes(data, start, end);
    let reached = boxes.last().map_or(start, |b| b.end);
    if end.saturating_sub(reached) >= 8 {
        return Err(Mp4Error::Malformed(
            "a child box is smaller than its header",
        ));
    }
    Ok(boxes)
}

fn header_of(trak: &[u8]) -> Result<Vec<u8>> {
    trak.get(..8)
        .map(<[u8]>::to_vec)
        .ok_or(Mp4Error::Malformed("box without a header"))
}

/// The handler type of a `trak` (`mdia/hdlr`), if it has one.
pub(crate) fn trak_handler(trak: &[u8]) -> Option<[u8; 4]> {
    let mut offset = 8;
    while offset + 8 <= trak.len() {
        let size = u32_at(trak, offset).ok()? as usize;
        if size < 8 {
            break;
        }
        if &trak[offset + 4..offset + 8] == b"mdia" {
            let (mut inner, inner_end) = (offset + 8, offset + size);
            while inner + 8 <= inner_end && inner + 8 <= trak.len() {
                let inner_size = u32_at(trak, inner).ok()? as usize;
                if inner_size < 8 {
                    break;
                }
                if &trak[inner + 4..inner + 8] == b"hdlr" && inner + 20 <= trak.len() {
                    return trak[inner + 16..inner + 20].try_into().ok();
                }
                inner += inner_size;
            }
        }
        offset += size;
    }
    None
}

/// The ISO 639-2 code packed in the `mdhd` of a `trak`, or `None` when absent or undetermined.
fn trak_mdhd_language(trak: &[u8]) -> Option<String> {
    let mut offset = 8;
    while offset + 8 <= trak.len() {
        let size = u32_at(trak, offset).ok()? as usize;
        if size < 8 {
            break;
        }
        if &trak[offset + 4..offset + 8] == b"mdia" {
            let mut inner = offset + 8;
            let inner_end = (offset + size).min(trak.len());
            while inner + 8 <= inner_end {
                let inner_size = u32_at(trak, inner).ok()? as usize;
                if inner_size < 8 {
                    break;
                }
                if &trak[inner + 4..inner + 8] == b"mdhd" {
                    // version 0: language at body offset 20; version 1 (64-bit times): 32.
                    let version = *trak.get(inner + 8)?;
                    let at = inner + 8 + if version == 0 { 20 } else { 32 };
                    let packed = u16::from_be_bytes(trak.get(at..at + 2)?.try_into().ok()?);
                    let code: String = [10_u32, 5, 0]
                        .iter()
                        .map(|shift| {
                            char::from(u8::try_from((packed >> shift) & 0x1F).unwrap_or(0) + 0x60)
                        })
                        .collect();
                    return (code != "und" && code.chars().all(|c| c.is_ascii_lowercase()))
                        .then_some(code);
                }
                inner += inner_size;
            }
        }
        offset += size;
    }
    None
}

fn track_id(trak: &[u8]) -> Option<u32> {
    let (start, _) = find_path(trak, 8, trak.len(), &[b"tkhd"])?;
    let version = *trak.get(start + 8)?;
    u32_at(trak, start + 12 + if version == 0 { 8 } else { 16 }).ok()
}

/// `channelcount` of the first audio sample entry, 0 if it cannot be read.
fn audio_channels(trak: &[u8]) -> u32 {
    let Some(stsd) = find_path(trak, 8, trak.len(), &[b"mdia", b"minf", b"stbl", b"stsd"]) else {
        return 0;
    };
    let offset = stsd.0 + 16 + 24;
    if offset + 2 > stsd.1 {
        return 0;
    }
    trak.get(offset..offset + 2)
        .map_or(0, |b| u32::from(u16::from_be_bytes([b[0], b[1]])))
}

fn chapter_ids(traks: &[&[u8]]) -> BTreeSet<u32> {
    let mut ids = BTreeSet::new();
    for trak in traks {
        let Some(tref) = find_path(trak, 8, trak.len(), &[b"tref"]) else {
            continue;
        };
        for reference in iter_boxes(trak, tref.0 + 8, tref.1)
            .iter()
            .filter(|r| &r.kind == b"chap")
        {
            let count = (reference.end - reference.start - 8) / 4;
            ids.extend((0..count).filter_map(|i| u32_at(trak, reference.start + 8 + 4 * i).ok()));
        }
    }
    ids
}

fn traks_of(moov: &[u8]) -> Vec<&[u8]> {
    iter_boxes(moov, 8, moov.len())
        .iter()
        .filter(|b| &b.kind == b"trak")
        .map(|b| &moov[b.start..b.end])
        .collect()
}

/// Track IDs any `trak` of `moov` names in its `tref/chap`.
#[must_use]
pub fn chapter_track_ids_in(moov: &[u8]) -> BTreeSet<u32> {
    chapter_ids(&traks_of(moov))
}

fn build_elng(bcp47: &str) -> Vec<u8> {
    let mut body = vec![0, 0, 0, 0];
    body.extend_from_slice(bcp47.as_bytes());
    body.push(0);
    build_box(b"elng", &body)
}

/// A new `trak` with `elng` before `minf` in its `mdia`; an existing `elng` is replaced.
fn inject_elng_into_trak(trak: &[u8], bcp47: &str) -> Result<Vec<u8>> {
    let elng = build_elng(bcp47);
    let mut result = header_of(trak)?;
    for child in strict_children(trak, 8, trak.len())? {
        if &child.kind != b"mdia" {
            result.extend_from_slice(&trak[child.start..child.end]);
            continue;
        }
        let mut body = Vec::new();
        let mut written = false;
        for inner in strict_children(trak, child.start + 8, child.end)? {
            if &inner.kind == b"elng" {
                continue; // dropped here, re-inserted at the right position
            }
            if &inner.kind == b"minf" && !written {
                body.extend_from_slice(&elng);
                written = true;
            }
            body.extend_from_slice(&trak[inner.start..inner.end]);
        }
        if !written {
            body.extend_from_slice(&elng);
        }
        result.extend_from_slice(&build_box(b"mdia", &body));
    }
    seal(result)
}

fn build_chtx(characteristic: &[u8]) -> Vec<u8> {
    let mut body = vec![0, 0, 0, 0];
    body.extend_from_slice(characteristic);
    body.push(0);
    build_box(b"chtx", &body)
}

/// A new `trak` with the SDH `chtx` atoms in its `udta` (created at the end when missing).
fn inject_chtx_into_trak(trak: &[u8]) -> Result<Vec<u8>> {
    let mut atoms = build_chtx(CHTX_TRANSCRIBES);
    atoms.extend_from_slice(&build_chtx(CHTX_DESCRIBES));
    let mut result = header_of(trak)?;
    let mut found = false;
    for child in strict_children(trak, 8, trak.len())? {
        if &child.kind != b"udta" {
            result.extend_from_slice(&trak[child.start..child.end]);
            continue;
        }
        let mut body = Vec::new();
        for inner in strict_children(trak, child.start + 8, child.end)? {
            if &inner.kind != b"chtx" {
                body.extend_from_slice(&trak[inner.start..inner.end]);
            }
        }
        body.extend_from_slice(&atoms);
        result.extend_from_slice(&build_box(b"udta", &body));
        found = true;
    }
    if !found {
        result.extend_from_slice(&build_box(b"udta", &atoms));
    }
    seal(result)
}

/// The `trak` with the forced display flags set on its tx3g sample entry (size-neutral).
fn mark_tx3g_forced(trak: &[u8]) -> Vec<u8> {
    let Some(stsd) = find_path(trak, 8, trak.len(), &[b"mdia", b"minf", b"stbl", b"stsd"]) else {
        return trak.to_vec();
    };
    let entry = stsd.0 + 16;
    let flags_at = entry + 16;
    if clamp(trak, entry + 4, entry + 8) != b"tx3g" || flags_at + 4 > stsd.1 {
        return trak.to_vec();
    }
    let Ok(flags) = u32_at(trak, flags_at) else {
        return trak.to_vec();
    };
    let mut out = trak.to_vec();
    out[flags_at..flags_at + 4].copy_from_slice(&(flags | TX3G_FORCED_FLAGS).to_be_bytes());
    out
}

/// A new moov with `elng` (and for SDH `chtx`) injected into the tracks. A chapter track is no
/// subtitle: it takes no entry of `sub_infos` and gets no tags.
pub(crate) fn patch_moov_elng(moov: &[u8], sub_infos: &[SubInfo]) -> Result<Vec<u8>> {
    let chapters = chapter_track_ids_in(moov);
    let mut result = header_of(moov)?;
    let mut sub_idx = 0;
    for child in strict_children(moov, 8, moov.len())? {
        let mut bytes = moov[child.start..child.end].to_vec();
        if &child.kind == b"trak" {
            let handler = trak_handler(&bytes);
            let is_subtitle = handler.is_some_and(|h| SUBTITLE_HANDLERS.contains(&&h));
            let is_chapter = track_id(&bytes).is_some_and(|id| chapters.contains(&id));
            if is_subtitle && !is_chapter && sub_idx < sub_infos.len() {
                let (lang, sdh, forced) = &sub_infos[sub_idx];
                bytes = inject_elng_into_trak(&bytes, &bcp47_of(lang))?;
                if *forced {
                    bytes = mark_tx3g_forced(&bytes);
                }
                if *sdh {
                    bytes = inject_chtx_into_trak(&bytes)?;
                }
                sub_idx += 1;
            } else if handler.is_some_and(|h| AV_HANDLERS.contains(&&h)) {
                // Apple tags every audio/video track with elng too (en-US, de-DE, ...).
                if let Some(lang) = trak_mdhd_language(&bytes) {
                    bytes = inject_elng_into_trak(&bytes, &bcp47_of(&lang))?;
                }
            }
        }
        result.extend_from_slice(&bytes);
    }
    seal(result)
}

type Wanted = BTreeMap<u32, Vec<([u8; 4], u32)>>;

/// Plan the Apple track references (policy: [`plan_links`]) and the reference types this call owns.
///
/// `folw`/`forc` are only planned (and stale ones removed) when `sub_infos` describes every subtitle
/// track; otherwise it is unknown which one is forced and existing links stay.
fn reference_targets(traks: &[&[u8]], sub_infos: &[SubInfo]) -> (Wanted, BTreeSet<[u8; 4]>) {
    let chapters = chapter_ids(traks);
    let mut audio: Vec<(u32, AudioRef)> = Vec::new();
    let mut subs: Vec<(u32, Option<String>)> = Vec::new();
    for trak in traks {
        let Some(id) = track_id(trak) else {
            continue;
        };
        match trak_handler(trak) {
            Some(h) if &h == b"soun" => {
                audio.push((
                    id,
                    AudioRef {
                        lang: trak_mdhd_language(trak),
                        channels: audio_channels(trak),
                    },
                ));
            }
            Some(h) if SUBTITLE_HANDLERS.contains(&&h) && !chapters.contains(&id) => {
                subs.push((id, trak_mdhd_language(trak)));
            }
            _ => {}
        }
    }
    let forced_known = sub_infos.len() == subs.len();
    let sub_refs: Vec<SubRef> = if forced_known {
        subs.iter()
            .zip(sub_infos)
            .map(|((_, mdhd), (info_lang, _, forced))| SubRef {
                lang: Some(mdhd.clone().unwrap_or_else(|| info_lang.clone())),
                forced: *forced,
            })
            .collect()
    } else {
        subs.iter()
            .map(|(_, mdhd)| SubRef {
                lang: mdhd.clone(),
                forced: false,
            })
            .collect()
    };
    let audio_refs: Vec<AudioRef> = audio.iter().map(|(_, r)| r.clone()).collect();
    let links = plan_links(&audio_refs, &sub_refs);
    let (audio_ids, sub_ids): (Vec<u32>, Vec<u32>) = (
        audio.iter().map(|(i, _)| *i).collect(),
        subs.iter().map(|(i, _)| *i).collect(),
    );
    let mut wanted = Wanted::new();
    for (&src, &dst) in &links.fall {
        wanted
            .entry(audio_ids[src])
            .or_default()
            .push((FALL, audio_ids[dst]));
    }
    for (&src, &dst) in &links.folw {
        wanted
            .entry(audio_ids[src])
            .or_default()
            .push((FOLW, sub_ids[dst]));
    }
    for (&src, &dst) in &links.forc {
        wanted
            .entry(sub_ids[src])
            .or_default()
            .push((FORC, sub_ids[dst]));
    }
    let owned = if forced_known {
        BTreeSet::from([FALL, FOLW, FORC])
    } else {
        BTreeSet::from([FALL])
    };
    (wanted, owned)
}

/// The `trak` whose `tref` holds `wanted` for the `owned` reference types; other types stay.
fn with_refs(trak: &[u8], wanted: &[([u8; 4], u32)], owned: &BTreeSet<[u8; 4]>) -> Result<Vec<u8>> {
    let children = strict_children(trak, 8, trak.len())?;
    let had_tref = children.iter().any(|b| &b.kind == b"tref");
    let mut kept: Vec<Vec<u8>> = Vec::new();
    for child in children.iter().filter(|b| &b.kind == b"tref") {
        kept = iter_boxes(trak, child.start + 8, child.end)
            .iter()
            .filter(|r| !owned.contains(&r.kind))
            .map(|r| trak[r.start..r.end].to_vec())
            .collect();
    }
    kept.extend(
        wanted
            .iter()
            .map(|(kind, target)| build_box(kind, &target.to_be_bytes())),
    );
    let tref = if kept.is_empty() {
        Vec::new()
    } else {
        build_box(b"tref", &kept.concat())
    };

    let mut body = Vec::new();
    let mut placed = false;
    for (i, child) in children.iter().enumerate() {
        if &child.kind == b"tref" {
            body.extend_from_slice(&tref);
            placed = true;
            continue;
        }
        body.extend_from_slice(&trak[child.start..child.end]);
        // No tref yet: it goes right after edts, else right after tkhd.
        let follows_edts = children
            .get(i + 1)
            .is_some_and(|next| &next.kind == b"edts");
        if !placed
            && !had_tref
            && (&child.kind == b"edts" || (&child.kind == b"tkhd" && !follows_edts))
        {
            body.extend_from_slice(&tref);
            placed = true;
        }
    }
    Ok(build_box(b"trak", &body))
}

/// A new moov carrying Apple's fall/folw/forc track references.
pub(crate) fn patch_moov_links(moov: &[u8], sub_infos: &[SubInfo]) -> Result<Vec<u8>> {
    let boxes = iter_boxes(moov, 8, moov.len());
    let traks = traks_of(moov);
    let (wanted, owned) = reference_targets(&traks, sub_infos);
    let mut body = Vec::new();
    for child in boxes {
        let bytes = &moov[child.start..child.end];
        if &child.kind == b"trak" {
            let key = track_id(bytes).filter(|&id| id != 0);
            let mine = key
                .and_then(|id| wanted.get(&id))
                .map_or(&[][..], Vec::as_slice);
            body.extend_from_slice(&with_refs(bytes, mine, &owned)?);
        } else {
            body.extend_from_slice(bytes);
        }
    }
    seal(build_box(b"moov", &body))
}

/// Counts of `elng`, track references and forced tx3g flags in a moov.
#[must_use]
pub fn summarize_moov(moov: &[u8]) -> PatchSummary {
    let mut summary = PatchSummary::default();
    for trak in traks_of(moov) {
        if find_path(trak, 8, trak.len(), &[b"mdia", b"elng"]).is_some() {
            summary.elng += 1;
        }
        if let Some(tref) = find_path(trak, 8, trak.len(), &[b"tref"]) {
            for reference in iter_boxes(trak, tref.0 + 8, tref.1) {
                match reference.kind {
                    FALL => summary.fall += 1,
                    FOLW => summary.folw += 1,
                    FORC => summary.forc += 1,
                    _ => {}
                }
            }
        }
        if forced_flag(trak) == Some(true) {
            summary.forced_flags += 1;
        }
    }
    summary
}

/// Whether the tx3g entry of `trak` carries the forced flags; `None` without a tx3g entry.
fn forced_flag(trak: &[u8]) -> Option<bool> {
    let stsd = find_path(trak, 8, trak.len(), &[b"mdia", b"minf", b"stbl", b"stsd"])?;
    if clamp(trak, stsd.0 + 20, stsd.0 + 24) != b"tx3g" {
        return None;
    }
    let flags = u32_at(trak, stsd.0 + 32).ok()?;
    Some(flags & TX3G_FORCED_FLAGS == TX3G_FORCED_FLAGS)
}

/// Everything the tag patch manages, per track; compared to decide whether a patch would change anything.
#[must_use]
pub fn tag_signature(moov: &[u8]) -> TagSignature {
    traks_of(moov)
        .into_iter()
        .map(|trak| {
            let mut refs = BTreeMap::new();
            if let Some(tref) = find_path(trak, 8, trak.len(), &[b"tref"]) {
                for reference in iter_boxes(trak, tref.0 + 8, tref.1) {
                    if [FALL, FOLW, FORC].contains(&reference.kind) {
                        let count = (reference.end - reference.start - 8) / 4;
                        let targets = (0..count)
                            .filter_map(|i| u32_at(trak, reference.start + 8 + 4 * i).ok());
                        refs.insert(reference.kind, targets.collect());
                    }
                }
            }
            TrackSignature {
                track_id: track_id(trak),
                elng: find_path(trak, 8, trak.len(), &[b"mdia", b"elng"])
                    .map(|(s, e)| trak[s..e].to_vec()),
                refs,
                forced: forced_flag(trak) == Some(true),
                has_chtx: trak.windows(4).any(|w| w == b"chtx"),
            }
        })
        .collect()
}

/// A new moov with elng, chtx, forced flags and track references applied.
///
/// # Errors
///
/// [`Mp4Error`] when a box inside is smaller than its header or a rebuilt box would exceed 4 GiB.
pub fn build_patched_moov(moov: &[u8], sub_infos: &[SubInfo]) -> Result<Vec<u8>> {
    patch_moov_links(&patch_moov_elng(moov, sub_infos)?, sub_infos)
}

/// Would the tag patch change this moov? Never writes.
///
/// # Errors
///
/// As [`build_patched_moov`].
pub fn check_moov(moov: &[u8], sub_infos: &[SubInfo]) -> Result<TagCheck> {
    let patched = build_patched_moov(moov, sub_infos)?;
    Ok(TagCheck {
        current: summarize_moov(moov),
        wanted: summarize_moov(&patched),
        needs_patch: tag_signature(moov) != tag_signature(&patched),
    })
}

#[cfg(test)]
pub(crate) mod tests_support {
    use std::io::Cursor;

    use atvprep_testkit::fixture_path;

    use super::super::boxes::find_box;

    /// The moov box of a committed fixture.
    pub fn moov_of(name: &str) -> Vec<u8> {
        let data = std::fs::read(fixture_path(name)).unwrap();
        let end = u64::try_from(data.len()).unwrap();
        let at = usize::try_from(
            find_box(&mut Cursor::new(&data), 0, end, b"moov")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        data[at..at + size].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use atvprep_testkit::{load_golden, sha256_hex};
    use serde::Deserialize;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;

    use super::super::boxes::{find_path, iter_boxes};
    use super::*;

    use super::tests_support::moov_of;

    #[derive(Deserialize)]
    struct Input {
        kind: String,
        #[serde(default)]
        file: String,
        #[serde(default)]
        sub_infos: Vec<(String, bool, bool)>,
        #[serde(default)]
        lang: String,
    }

    fn summary_json(s: &PatchSummary) -> Value {
        json!({"elng": s.elng, "fall": s.fall, "folw": s.folw, "forc": s.forc, "forced_flags": s.forced_flags})
    }

    #[test]
    fn transforms_match_the_python_oracle() {
        let cases = load_golden::<Input, Value>("mp4_tags");
        assert!(cases.len() > 150);
        for case in cases {
            let input = &case.input;
            match input.kind.as_str() {
                "elng_tag" => {
                    assert_eq!(json!(elng_tag(&input.lang)), case.expected, "{}", case.name);
                }
                "table" => {
                    let mut pairs: Vec<[&str; 2]> = BCP47.iter().map(|&(k, v)| [k, v]).collect();
                    pairs.sort_unstable();
                    assert_eq!(json!(pairs), case.expected, "{}", case.name);
                }
                _ => {
                    let moov = moov_of(&input.file);
                    let infos = &input.sub_infos;
                    let patched = build_patched_moov(&moov, infos).unwrap();
                    let want = &case.expected;
                    let name = &case.name;
                    assert_eq!(
                        patched.len(),
                        usize::try_from(want["patched_len"].as_u64().unwrap()).unwrap(),
                        "{name} length"
                    );
                    assert_eq!(
                        json!(sha256_hex(&patched)),
                        want["patched_sha"],
                        "{name} patched"
                    );
                    assert_eq!(
                        json!(sha256_hex(&patch_moov_elng(&moov, infos).unwrap())),
                        want["elng_sha"],
                        "{name} elng"
                    );
                    assert_eq!(
                        json!(sha256_hex(&patch_moov_links(&moov, infos).unwrap())),
                        want["links_sha"],
                        "{name} links"
                    );
                    assert_eq!(
                        summary_json(&summarize_moov(&moov)),
                        want["summary_before"],
                        "{name} before"
                    );
                    assert_eq!(
                        summary_json(&summarize_moov(&patched)),
                        want["summary_after"],
                        "{name} after"
                    );
                    assert_eq!(
                        json!(check_moov(&moov, infos).unwrap().needs_patch),
                        want["needs_patch"],
                        "{name} needs_patch"
                    );
                    assert_eq!(
                        json!(chapter_track_ids_in(&moov)),
                        want["chapters"],
                        "{name} chapters"
                    );
                    let twice = build_patched_moov(&patched, infos).unwrap();
                    assert_eq!(
                        twice == patched,
                        want["idempotent"].as_bool().unwrap(),
                        "{name} idempotent"
                    );
                }
            }
        }
    }

    fn children(data: &[u8], start: usize, end: usize) -> Vec<String> {
        iter_boxes(data, start, end)
            .iter()
            .map(|b| String::from_utf8_lossy(&b.kind).into_owned())
            .collect()
    }

    fn traks(moov: &[u8]) -> Vec<Vec<u8>> {
        iter_boxes(moov, 8, moov.len())
            .iter()
            .filter(|b| &b.kind == b"trak")
            .map(|b| moov[b.start..b.end].to_vec())
            .collect()
    }

    #[test]
    fn elng_is_inserted_before_minf_and_an_old_one_is_replaced() {
        let moov = moov_of("syn_pad_3.mp4");
        let patched = build_patched_moov(&moov, &[]).unwrap();
        let audio = &traks(&patched)[1];
        let mdia = find_path(audio, 8, audio.len(), &[b"mdia"]).unwrap();
        let order = children(audio, mdia.0 + 8, mdia.1);
        assert_eq!(order, ["mdhd", "hdlr", "elng", "minf"]);
        assert_eq!(summarize_moov(&patched).elng, 1);
        assert_eq!(
            patched.len() + 3,
            moov.len(),
            "en-US-ab shrinks to en-US by exactly three bytes"
        );
    }

    #[test]
    fn chtx_atoms_are_written_for_sdh_tracks_only_and_not_doubled() {
        let moov = moov_of("stereo_surround_subs.mp4");
        let infos = vec![
            ("deu".to_owned(), false, false),
            ("deu".to_owned(), false, true),
            ("eng".to_owned(), false, false),
            ("eng".to_owned(), true, false),
        ];
        let once = build_patched_moov(&moov, &infos).unwrap();
        let twice = build_patched_moov(&once, &infos).unwrap();
        let count = |m: &[u8]| m.windows(4).filter(|w| *w == b"chtx").count();
        assert_eq!(count(&once), 2);
        assert_eq!(count(&twice), 2);
    }

    #[test]
    fn the_tref_goes_right_after_tkhd_or_after_edts() {
        let moov = moov_of("stereo_surround_subs.mp4");
        let infos = vec![
            ("deu".to_owned(), false, false),
            ("deu".to_owned(), false, true),
            ("eng".to_owned(), false, false),
            ("eng".to_owned(), true, false),
        ];
        for trak in traks(&build_patched_moov(&moov, &infos).unwrap()) {
            let order = children(&trak, 8, trak.len());
            if let Some(at) = order.iter().position(|k| k == "tref") {
                let before = &order[at - 1];
                assert!(before == "edts" || before == "tkhd", "{order:?}");
                if before == "tkhd" {
                    assert_ne!(
                        order.get(at + 1).map(String::as_str),
                        Some("edts"),
                        "{order:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn folw_and_forc_are_left_alone_when_sub_infos_do_not_cover_every_subtitle_track() {
        let moov = moov_of("stereo_surround_subs.mp4");
        let full = vec![
            ("deu".to_owned(), false, false),
            ("deu".to_owned(), false, true),
            ("eng".to_owned(), false, false),
            ("eng".to_owned(), true, false),
        ];
        let once = build_patched_moov(&moov, &full).unwrap();
        let partial = build_patched_moov(&once, &full[..2]).unwrap();
        let (a, b) = (summarize_moov(&once), summarize_moov(&partial));
        assert_eq!((a.folw, a.forc), (b.folw, b.forc));
    }

    #[test]
    fn a_chapter_track_gets_no_tags_and_takes_no_sub_info() {
        let moov = moov_of("syn_chapters.mp4");
        assert_eq!(chapter_track_ids_in(&moov), BTreeSet::from([5]));
        let patched = build_patched_moov(&moov, &[("eng".to_owned(), false, false)]).unwrap();
        let chapter = &traks(&patched)[3];
        assert!(find_path(chapter, 8, chapter.len(), &[b"mdia", b"elng"]).is_none());
        assert_eq!(summarize_moov(&patched).elng, 2);
    }

    #[test]
    fn a_child_box_smaller_than_its_header_is_an_error_not_silent_data_loss() {
        let moov = moov_of("syn_tiny_size_box.mp4");
        assert!(build_patched_moov(&moov, &[]).is_err());
    }
}
