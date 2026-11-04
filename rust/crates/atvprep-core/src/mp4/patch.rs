//! File-level moov patching: alternate groups, handler types, tags, Atmos and the layout moves
//! (port of the file half of `atvprep/mp4patch.py`).
//!
//! Every writing operation builds the complete new moov in memory first; the first byte reaches the
//! file only after that succeeded. Layout moves go through a temp file and an atomic rename.
//!
//! * moov is the last box: written in place, the file truncated; nothing follows it, so no media
//!   moves and no chunk offset changes.
//! * the new moov fits: written in place, the rest filled with a `free` box.
//! * it grew with the moov in front of the media: every chunk offset moves by the growth and the
//!   file is rewritten through a temp file.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::atmos::patch_moov_for_atmos;
use super::boxes::{find_box, read_box_header};
use super::copy::{COPY_CHUNK, copy_all, copy_n, via_temp};
use super::error::{Mp4Error, Result};
use super::offsets::{add_to_chunk_offsets, moov_for_front};
pub use super::tags::{PatchSummary, SubInfo, TagCheck};
use super::tags::{build_patched_moov, chapter_track_ids_in, check_moov, summarize_moov};

/// `(bytes done, bytes total)` while a file is copied.
pub type Progress<'a> = Option<&'a mut dyn FnMut(u64, u64)>;

/// `tkhd` `alternate_group` offset from the start of the box: version 0 and version 1.
const TKHD_ALTERNATE_GROUP_V0: u64 = 42;
const TKHD_ALTERNATE_GROUP_V1: u64 = 54;

/// Handler type to `alternate_group`: audio 1, subtitles 2, everything else 0.
fn alternate_group(handler: &[u8]) -> i16 {
    match handler {
        b"soun" => 1,
        b"text" | b"sbtl" | b"subt" | b"clcp" => 2,
        _ => 0,
    }
}

/// The moov of an open file and the boxes around it.
#[derive(Debug, Clone, Copy)]
struct Layout {
    file_size: u64,
    moov_pos: u64,
    moov_size: u64,
    mdat_pos: Option<u64>,
}

impl Layout {
    /// Moov in front of the media (or no media at all): chunk offsets shift when it grows.
    fn moov_first(&self) -> bool {
        self.mdat_pos.is_none_or(|mdat| self.moov_pos < mdat)
    }

    fn moov_is_last(&self) -> bool {
        self.moov_pos + self.moov_size == self.file_size
    }
}

/// Where the moov is; `None` when the file has none. The size field must be a plain 32-bit size.
fn locate(file: &mut File) -> Result<Option<Layout>> {
    let file_size = file.metadata()?.len();
    let Some(moov_pos) = find_box(file, 0, file_size, b"moov")? else {
        return Ok(None);
    };
    let mdat_pos = find_box(file, 0, file_size, b"mdat")?;
    file.seek(SeekFrom::Start(moov_pos))?;
    let mut raw = [0_u8; 4];
    file.read_exact(&mut raw)?;
    let moov_size = match u32::from_be_bytes(raw) {
        0 => return Err(Mp4Error::Unsupported("a moov with size 0")),
        1 => return Err(Mp4Error::Unsupported("a moov with a 64-bit size")),
        n if n < 8 => return Err(Mp4Error::Malformed("moov smaller than its header")),
        n => u64::from(n),
    };
    if moov_pos + moov_size > file_size {
        return Err(Mp4Error::Malformed("moov extends past the end of the file"));
    }
    Ok(Some(Layout {
        file_size,
        moov_pos,
        moov_size,
        mdat_pos,
    }))
}

fn read_moov(file: &mut File, layout: &Layout) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(layout.moov_pos))?;
    let mut moov = vec![
        0_u8;
        usize::try_from(layout.moov_size)
            .map_err(|_| Mp4Error::Unsupported("moov too large"))?
    ];
    file.read_exact(&mut moov)?;
    Ok(moov)
}

/// Whether the moov precedes the first mdat (faststart layout); false when either is missing.
///
/// # Errors
///
/// I/O errors.
pub fn is_faststart(path: &Path) -> Result<bool> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let moov = find_box(&mut file, 0, size, b"moov")?;
    let mdat = find_box(&mut file, 0, size, b"mdat")?;
    Ok(matches!((moov, mdat), (Some(m), Some(d)) if m < d))
}

/// Track IDs of the `QuickTime` chapter tracks: those another track names in `tref/chap`.
///
/// A chapter track carries the `text` handler and is no subtitle; `AVFoundation` reads chapters only
/// from it, so it must keep that handler and stay out of the subtitle handling.
///
/// # Errors
///
/// I/O errors, or a moov in a form the patcher does not handle.
pub fn chapter_track_ids(path: &Path) -> Result<BTreeSet<u32>> {
    let mut file = File::open(path)?;
    let Some(layout) = locate(&mut file)? else {
        return Ok(BTreeSet::new());
    };
    Ok(chapter_track_ids_in(&read_moov(&mut file, &layout)?))
}

/// Would the tag patch change the tags of `path`? Never writes.
///
/// # Errors
///
/// I/O errors, or a moov that cannot be rebuilt.
pub fn check_accessibility(path: &Path, sub_infos: &[SubInfo]) -> Result<TagCheck> {
    let mut file = File::open(path)?;
    let Some(layout) = locate(&mut file)? else {
        return Ok(TagCheck {
            current: PatchSummary::default(),
            wanted: PatchSummary::default(),
            needs_patch: false,
        });
    };
    check_moov(&read_moov(&mut file, &layout)?, sub_infos)
}

fn read_at(cursor: &mut Cursor<&[u8]>, position: u64, count: usize) -> Result<Vec<u8>> {
    cursor.seek(SeekFrom::Start(position))?;
    let mut bytes = Vec::with_capacity(count);
    cursor.by_ref().take(count as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// `track_ID` of the `trak` in `[inner, end)`, read the way the Python seek-based code reads it.
fn track_id_at(cursor: &mut Cursor<&[u8]>, inner: u64, end: u64) -> Result<Option<u32>> {
    let Some(tkhd) = find_box(cursor, inner, end, b"tkhd")? else {
        return Ok(None);
    };
    let version = read_at(cursor, tkhd + 8, 1)?;
    let skip = if version == [1] { 20 } else { 12 };
    let raw = read_at(cursor, tkhd + 8 + skip, 4)?;
    Ok(<[u8; 4]>::try_from(raw.as_slice())
        .ok()
        .map(u32::from_be_bytes))
}

/// The in-place byte patches (offset in the moov, new bytes) for one `trak`.
fn patch_trak(
    cursor: &mut Cursor<&[u8]>,
    inner: u64,
    end: u64,
    chapters: &BTreeSet<u32>,
    patches: &mut Vec<(u64, Vec<u8>)>,
) -> Result<()> {
    if !chapters.is_empty()
        && track_id_at(cursor, inner, end)?.is_some_and(|id| chapters.contains(&id))
    {
        return Ok(()); // a chapter track keeps its 'text' handler and stays out of the subtitle group
    }
    let Some(mdia) = find_box(cursor, inner, end, b"mdia")? else {
        return Ok(());
    };
    cursor.seek(SeekFrom::Start(mdia))?;
    let Some((mdia_size, _)) = read_box_header(cursor)? else {
        return Ok(());
    };
    let Some(hdlr) = find_box(cursor, mdia + 8, mdia.saturating_add(mdia_size), b"hdlr")? else {
        return Ok(());
    };
    let mut handler = read_at(cursor, hdlr + 16, 4)?;
    // ffmpeg writes 'text' for mov_text; AVFoundation requires 'sbtl'.
    if handler == b"text" {
        patches.push((hdlr + 16, b"sbtl".to_vec()));
        handler = b"sbtl".to_vec();
    }
    let Some(tkhd) = find_box(cursor, inner, end, b"tkhd")? else {
        return Ok(());
    };
    let version = read_at(cursor, tkhd + 8, 1)?;
    let Some(&version) = version.first() else {
        return Err(Mp4Error::Malformed("tkhd cut off"));
    };
    let offset = tkhd
        + if version == 0 {
            TKHD_ALTERNATE_GROUP_V0
        } else {
            TKHD_ALTERNATE_GROUP_V1
        };
    patches.push((offset, alternate_group(&handler).to_be_bytes().to_vec()));
    Ok(())
}

/// A copy of the patch-in-memory plan for [`Patcher::set_alternate_groups`].
fn alternate_group_patches(moov: &[u8], chapters: &BTreeSet<u32>) -> Result<Vec<(u64, Vec<u8>)>> {
    let mut cursor = Cursor::new(moov);
    let end = moov.len() as u64;
    let mut patches = Vec::new();
    let mut at = 8_u64;
    while at + 8 <= end {
        cursor.seek(SeekFrom::Start(at))?;
        let Some((size, kind)) = read_box_header(&mut cursor)? else {
            break;
        };
        if size < 8 {
            break;
        }
        if &kind == b"trak" {
            patch_trak(
                &mut cursor,
                at + 8,
                at.saturating_add(size).min(end),
                chapters,
                &mut patches,
            )?;
        }
        at = at.saturating_add(size);
    }
    // A patch must stay inside the moov it was planned from.
    if patches
        .iter()
        .any(|(offset, bytes)| offset + bytes.len() as u64 > end)
    {
        return Err(Mp4Error::Malformed("a patch would fall outside the moov"));
    }
    Ok(patches)
}

/// Settings of the writing operations; the defaults are what production uses.
#[derive(Debug, Clone)]
pub struct Patcher {
    /// Bytes per read and write while a file is copied.
    pub chunk: usize,
}

impl Default for Patcher {
    fn default() -> Self {
        Self { chunk: COPY_CHUNK }
    }
}

/// Counts bytes and tells the progress callback `(done, total)` after every chunk.
struct Ticker<'a> {
    done: u64,
    total: u64,
    progress: Progress<'a>,
}

impl Ticker<'_> {
    fn tick(&mut self, count: u64) {
        self.done += count;
        if let Some(progress) = self.progress.as_deref_mut() {
            progress(self.done, self.total);
        }
    }
}

impl Patcher {
    /// Patch `alternate_group` in every `tkhd` and `text` to `sbtl` in every subtitle `hdlr`, in
    /// place. A chapter track is left as it is. Sizes never change.
    ///
    /// # Errors
    ///
    /// I/O errors, or a moov the patcher cannot follow (nothing is written then).
    pub fn set_alternate_groups(&self, path: &Path) -> Result<()> {
        let chapters = chapter_track_ids(path)?;
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let Some(layout) = locate(&mut file)? else {
            return Ok(());
        };
        let moov = read_moov(&mut file, &layout)?;
        for (offset, bytes) in alternate_group_patches(&moov, &chapters)? {
            file.seek(SeekFrom::Start(layout.moov_pos + offset))?;
            file.write_all(&bytes)?;
        }
        Ok(())
    }

    /// Write Apple's `elng` tags, `chtx` atoms, forced flags and track references.
    ///
    /// `sub_infos` holds `(language, is_sdh, is_forced)` per active subtitle track in file order.
    /// If the moov grows, every chunk offset moves by the growth and the file is rewritten through
    /// a temp file (offsets only move when the moov precedes the media).
    ///
    /// # Errors
    ///
    /// I/O errors, or a moov that cannot be rebuilt (the file is left as it was).
    pub fn set_subtitle_accessibility(
        &self,
        path: &Path,
        sub_infos: &[SubInfo],
        progress: Progress<'_>,
    ) -> Result<PatchSummary> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let Some(layout) = locate(&mut file)? else {
            return Ok(PatchSummary::default());
        };
        let new_moov = build_patched_moov(&read_moov(&mut file, &layout)?, sub_infos)?;
        let summary = summarize_moov(&new_moov);
        self.replace_moov(path, &mut file, &layout, &new_moov, progress)?;
        Ok(summary)
    }

    /// Everything `patch` does to the moov, written once: alternate groups and handler types in
    /// place, then the Apple tags and the Atmos `dec3` extension built on the same in-memory moov
    /// and committed in a single write. With `faststart`, a moov that trails the media is written
    /// in front of it in the same single copy.
    ///
    /// Returns the summary of the written moov and whether an Atmos `dec3` was patched.
    ///
    /// # Errors
    ///
    /// As [`Patcher::set_subtitle_accessibility`].
    pub fn apply_moov_patches(
        &self,
        path: &Path,
        sub_infos: &[SubInfo],
        progress: Progress<'_>,
        faststart: bool,
    ) -> Result<(PatchSummary, bool)> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let Some(layout) = locate(&mut file)? else {
            return Ok((PatchSummary::default(), false));
        };
        // The alternate groups and handler types are applied to the moov in memory: the moov is
        // written back whole below, so the file ends up the same, and a later failure leaves no
        // half-patched file behind.
        let mut moov = read_moov(&mut file, &layout)?;
        let chapters = chapter_track_ids_in(&moov);
        for (offset, bytes) in alternate_group_patches(&moov, &chapters)? {
            let at = usize::try_from(offset).map_err(|_| Mp4Error::Malformed("patch offset"))?;
            moov[at..at + bytes.len()].copy_from_slice(&bytes);
        }
        let tagged = build_patched_moov(&moov, sub_infos)?;
        let summary = summarize_moov(&tagged);
        let new_moov = patch_moov_for_atmos(&tagged)?;
        match layout.mdat_pos {
            Some(mdat) if faststart && layout.moov_pos > mdat => {
                self.move_moov_to_front(path, &file, &layout, mdat, &new_moov, progress)?;
            }
            _ => self.replace_moov(path, &mut file, &layout, &new_moov, progress)?,
        }
        Ok((summary, new_moov != tagged))
    }

    /// Add the JOC extension to the `dec3` boxes of E-AC-3 Atmos tracks. Returns whether anything
    /// was patched.
    ///
    /// # Errors
    ///
    /// As [`Patcher::set_subtitle_accessibility`].
    pub fn patch_atmos_dec3(&self, path: &Path) -> Result<bool> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let Some(layout) = locate(&mut file)? else {
            return Ok(false);
        };
        let moov = read_moov(&mut file, &layout)?;
        let new_moov = patch_moov_for_atmos(&moov)?;
        if new_moov == moov {
            return Ok(false);
        }
        self.replace_moov(path, &mut file, &layout, &new_moov, None)?;
        Ok(true)
    }

    /// See [`check_accessibility`].
    ///
    /// # Errors
    ///
    /// As [`check_accessibility`].
    pub fn check_accessibility(&self, path: &Path, sub_infos: &[SubInfo]) -> Result<TagCheck> {
        check_accessibility(path, sub_infos)
    }

    /// Put `new_moov` where the old moov was (`file` is open for update).
    fn replace_moov(
        &self,
        path: &Path,
        file: &mut File,
        layout: &Layout,
        new_moov: &[u8],
        progress: Progress<'_>,
    ) -> Result<()> {
        let new_size = new_moov.len() as u64;
        if layout.moov_is_last() {
            if new_size == layout.moov_size {
                file.seek(SeekFrom::Start(layout.moov_pos))?;
                file.write_all(new_moov)?;
                return Ok(());
            }
            // A moov at the end that grows or shrinks would be rewritten and truncated in place, and
            // an interruption in between leaves a file that does not parse; go through a temp copy.
            return self.rewrite_with_new_moov(path, file, layout, new_moov, progress);
        }
        if new_size <= layout.moov_size {
            let mut moov = new_moov.to_vec();
            let mut pad = layout.moov_size - new_size;
            if (1..8).contains(&pad) {
                // No valid free box fits in 1..7 bytes: keep the old size field and zero-pad inside the moov.
                let old_size = u32::try_from(layout.moov_size)
                    .map_err(|_| Mp4Error::Unsupported("moov too large"))?;
                moov = [
                    &old_size.to_be_bytes()[..],
                    &new_moov[4..],
                    &vec![0_u8; usize::try_from(pad).unwrap_or(0)],
                ]
                .concat();
                pad = 0;
            }
            file.seek(SeekFrom::Start(layout.moov_pos))?;
            file.write_all(&moov)?;
            if pad >= 8 {
                let size =
                    u32::try_from(pad).map_err(|_| Mp4Error::Unsupported("free box too large"))?;
                file.write_all(&size.to_be_bytes())?;
                file.write_all(b"free")?;
                file.write_all(&vec![
                    0_u8;
                    usize::try_from(pad - 8).map_err(|_| {
                        Mp4Error::Unsupported("free box too large")
                    })?
                ])?;
            }
            return Ok(());
        }
        // The moov grew: rewrite the file. Chunk offsets move only when the media shifts right.
        let delta = new_size - layout.moov_size;
        let mut moov = new_moov.to_vec();
        if layout.moov_first() {
            add_to_chunk_offsets(&mut moov, delta)?;
        }
        self.rewrite_with_new_moov(path, file, layout, &moov, progress)
    }

    fn rewrite_with_new_moov(
        &self,
        path: &Path,
        file: &File,
        layout: &Layout,
        new_moov: &[u8],
        progress: Progress<'_>,
    ) -> Result<()> {
        let total = layout.file_size - layout.moov_size + new_moov.len() as u64;
        let mut ticker = Ticker {
            done: 0,
            total,
            progress,
        };
        via_temp(path, |tmp| {
            // The handle the layout was measured on, not a fresh open of the name: a file replaced
            // since `locate` must not have the old offsets applied to it.
            let mut source = file.try_clone()?;
            source.seek(SeekFrom::Start(0))?;
            let mut sink = File::create(tmp)?;
            // Everything before the moov (ftyp, free, ...), the new moov, then the rest (mdat, ...).
            copy_n(
                &mut source,
                &mut sink,
                layout.moov_pos,
                self.chunk,
                &mut |n| ticker.tick(n),
            )?;
            sink.write_all(new_moov)?;
            ticker.tick(new_moov.len() as u64);
            source.seek(SeekFrom::Start(layout.moov_pos + layout.moov_size))?;
            copy_all(&mut source, &mut sink, self.chunk, &mut |n| ticker.tick(n))?;
            sink.flush()?;
            Ok(())
        })
    }

    /// Rewrite the file with `new_moov` in front of the first mdat and the old moov removed: one
    /// sequential copy; only the offsets in the moov change.
    fn move_moov_to_front(
        &self,
        path: &Path,
        file: &File,
        layout: &Layout,
        mdat_pos: u64,
        new_moov: &[u8],
        progress: Progress<'_>,
    ) -> Result<()> {
        let front = moov_for_front(new_moov)?;
        let total = layout.file_size - layout.moov_size + front.len() as u64;
        let mut ticker = Ticker {
            done: 0,
            total,
            progress,
        };
        via_temp(path, |tmp| {
            // The handle the layout was measured on, not a fresh open of the name: a file replaced
            // since `locate` must not have the old offsets applied to it.
            let mut source = file.try_clone()?;
            source.seek(SeekFrom::Start(0))?;
            let mut sink = File::create(tmp)?;
            copy_n(&mut source, &mut sink, mdat_pos, self.chunk, &mut |n| {
                ticker.tick(n);
            })?; // ftyp and whatever precedes the media
            sink.write_all(&front)?;
            ticker.tick(front.len() as u64);
            copy_n(
                &mut source,
                &mut sink,
                layout.moov_pos - mdat_pos,
                self.chunk,
                &mut |n| ticker.tick(n),
            )?; // the media
            source.seek(SeekFrom::Start(layout.moov_pos + layout.moov_size))?;
            copy_all(&mut source, &mut sink, self.chunk, &mut |n| ticker.tick(n))?; // anything that followed the old moov
            sink.flush()?;
            Ok(())
        })
    }
}

/// [`Patcher::set_alternate_groups`] with the default settings.
///
/// # Errors
///
/// As [`Patcher::set_alternate_groups`].
pub fn set_mp4_alternate_groups(path: &Path) -> Result<()> {
    Patcher::default().set_alternate_groups(path)
}

/// [`Patcher::set_subtitle_accessibility`] with the default settings.
///
/// # Errors
///
/// As [`Patcher::set_subtitle_accessibility`].
pub fn set_subtitle_accessibility(
    path: &Path,
    sub_infos: &[SubInfo],
    progress: Progress<'_>,
) -> Result<PatchSummary> {
    Patcher::default().set_subtitle_accessibility(path, sub_infos, progress)
}

/// [`Patcher::apply_moov_patches`] with the default settings.
///
/// # Errors
///
/// As [`Patcher::apply_moov_patches`].
pub fn apply_moov_patches(
    path: &Path,
    sub_infos: &[SubInfo],
    progress: Progress<'_>,
    faststart: bool,
) -> Result<(PatchSummary, bool)> {
    Patcher::default().apply_moov_patches(path, sub_infos, progress, faststart)
}

/// [`Patcher::patch_atmos_dec3`] with the default settings.
///
/// # Errors
///
/// As [`Patcher::patch_atmos_dec3`].
pub fn patch_atmos_dec3(path: &Path) -> Result<bool> {
    Patcher::default().patch_atmos_dec3(path)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use atvprep_testkit::{fixture_path, load_golden, sha256_hex};
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;

    #[derive(Deserialize)]
    struct Input {
        file: String,
        op: String,
        sub_infos: Vec<SubInfo>,
    }

    #[derive(Deserialize)]
    struct Expected {
        returns: Value,
        size: u64,
        sha: String,
        listing: Vec<String>,
        progress: Vec<(u64, u64)>,
        changed: bool,
    }

    fn summary_json(s: &PatchSummary) -> Value {
        json!({"elng": s.elng, "fall": s.fall, "folw": s.folw, "forc": s.forc, "forced_flags": s.forced_flags})
    }

    /// Run one named operation on `path`; returns what Python's oracle returns for it.
    fn run(
        patcher: &Patcher,
        op: &str,
        path: &Path,
        infos: &[SubInfo],
        progress: &mut Vec<(u64, u64)>,
    ) -> Result<Value> {
        let mut record = |done: u64, total: u64| progress.push((done, total));
        Ok(match op {
            "alt" => {
                patcher.set_alternate_groups(path)?;
                Value::Null
            }
            "acc" => {
                summary_json(&patcher.set_subtitle_accessibility(path, infos, Some(&mut record))?)
            }
            "apply" | "apply_front" => {
                let (summary, atmos) = patcher.apply_moov_patches(
                    path,
                    infos,
                    Some(&mut record),
                    op == "apply_front",
                )?;
                json!({"summary": summary_json(&summary), "atmos": atmos})
            }
            "atmos" => json!(patcher.patch_atmos_dec3(path)?),
            "check" => {
                let check = patcher.check_accessibility(path, infos)?;
                json!({"current": summary_json(&check.current), "wanted": summary_json(&check.wanted), "needs_patch": check.needs_patch})
            }
            "faststart" => json!(is_faststart(path)?),
            "chapters" => json!(chapter_track_ids(path)?),
            other => panic!("unknown op {other}"),
        })
    }

    fn copy_fixture(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::copy(fixture_path(name), &path).unwrap();
        (dir, path)
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn operations_match_the_python_oracle() {
        let cases = load_golden::<Input, Expected>("mp4_ops");
        assert!(cases.len() > 150);
        let patcher = Patcher { chunk: 4096 };
        for case in cases {
            let name = &case.name;
            let original = std::fs::read(fixture_path(&case.input.file)).unwrap();
            let (dir, path) = copy_fixture(&case.input.file);
            let mut progress = Vec::new();
            let returns = run(
                &patcher,
                &case.input.op,
                &path,
                &case.input.sub_infos,
                &mut progress,
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"));
            let after = std::fs::read(&path).unwrap();
            assert_eq!(returns, case.expected.returns, "{name} returns");
            assert_eq!(after.len() as u64, case.expected.size, "{name} size");
            assert_eq!(sha256_hex(&after), case.expected.sha, "{name} bytes");
            assert_eq!(after != original, case.expected.changed, "{name} changed");
            assert_eq!(
                listing(dir.path()),
                case.expected.listing,
                "{name} directory"
            );
            assert_eq!(progress, case.expected.progress, "{name} progress");
        }
    }

    #[test]
    fn a_pad_of_three_bytes_is_absorbed_into_the_moov() {
        let (_dir, path) = copy_fixture("syn_pad_3.mp4");
        let before = std::fs::read(&path).unwrap();
        Patcher::default()
            .set_subtitle_accessibility(&path, &[], None)
            .unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(after.len(), before.len(), "the file keeps its size");
        let moov = after.windows(4).position(|w| w == b"moov").unwrap() - 4;
        let size = u32::from_be_bytes(after[moov..moov + 4].try_into().unwrap()) as usize;
        assert_eq!(size, 723, "the moov size field is unchanged");
        assert_eq!(
            &after[moov + size - 3..moov + size],
            [0, 0, 0],
            "three zero bytes close the moov"
        );
    }

    fn assert_untouched_on_error(name: &str, op: &str) -> bool {
        let before = std::fs::read(fixture_path(name)).unwrap();
        let (dir, path) = copy_fixture(name);
        let result = run(&Patcher::default(), op, &path, &[], &mut Vec::new());
        if result.is_err() {
            assert_eq!(
                std::fs::read(&path).unwrap(),
                before,
                "{name} {op}: bytes changed by a failed call"
            );
            assert_eq!(
                listing(dir.path()),
                [name.to_owned()],
                "{name} {op}: stray files"
            );
        }
        result.is_err()
    }

    #[test]
    fn malformed_files_are_errors_and_stay_byte_identical() {
        for name in ["syn_moov64.mp4", "syn_truncated_moov.mp4"] {
            for op in ["acc", "apply", "apply_front", "atmos"] {
                assert!(
                    assert_untouched_on_error(name, op),
                    "{name} {op} must be an error"
                );
            }
        }
        for (name, op) in [
            ("syn_tiny_size_box.mp4", "acc"),
            ("syn_huge_entry_count.mp4", "apply_front"),
        ] {
            assert!(
                assert_untouched_on_error(name, op),
                "{name} {op} must be an error"
            );
        }
        for name in [
            "syn_moov64.mp4",
            "syn_truncated_moov.mp4",
            "syn_truncated_file.mp4",
            "syn_tiny_size_box.mp4",
            "syn_huge_entry_count.mp4",
            "syn_not_mp4.mp4",
        ] {
            for op in [
                "alt",
                "acc",
                "apply",
                "apply_front",
                "atmos",
                "check",
                "faststart",
                "chapters",
            ] {
                assert_untouched_on_error(name, op); // never a panic, never a half-written file
            }
        }
    }

    #[test]
    fn a_file_without_a_moov_is_left_alone() {
        let (_dir, path) = copy_fixture("syn_not_mp4.mp4");
        let before = std::fs::read(&path).unwrap();
        assert_eq!(
            Patcher::default()
                .set_subtitle_accessibility(&path, &[], None)
                .unwrap(),
            PatchSummary::default()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!is_faststart(&path).unwrap());
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        assert!(matches!(
            is_faststart(Path::new("/definitely/not/here.mp4")),
            Err(Mp4Error::Io(_))
        ));
    }
}
