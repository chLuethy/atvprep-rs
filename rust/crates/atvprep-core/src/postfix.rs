//! Post-mux container rewrites that bring ffmpeg output into strict `AVPlayer` compliance (port of
//! `atvprep/postfix.py`). Every operation is size-neutral, so the sample tables stay valid.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde_json::Value;

use crate::ffprobe::{Ffprobe, ProbeError};
use crate::mp4::atmos::parse_dec3;
use crate::mp4::error::Mp4Error;
use crate::mp4::patch::chapter_track_ids;
use crate::track_policy::{Media, enabled_track_ids, roles_from_probe};

const BRAND_LEN: usize = 4;
/// `tkhd` flag bit 0: the track is enabled.
const TKHD_FLAG_ENABLED: u32 = 0x00_0001;
/// Extra payload bytes before the child atoms: `stsd` is a full box with an entry count, `ec-3` an
/// audio sample entry.
const STSD_EXTRA: u64 = 8;
const EC3_EXTRA: u64 = 28;

/// Why a rewrite was refused or failed.
#[derive(Debug)]
pub enum PostfixError {
    /// The file or an argument is not what the rewrite needs (Python `ValueError`).
    Value(String),
    /// A layout the size-neutral rewrites do not handle (Python `NotImplementedError`).
    NotImplemented(String),
    Probe(ProbeError),
    Mp4(Mp4Error),
    Io(std::io::Error),
}

impl fmt::Display for PostfixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(message) | Self::NotImplemented(message) => f.write_str(message),
            Self::Probe(error) => write!(f, "{error}"),
            Self::Mp4(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PostfixError {}

impl From<std::io::Error> for PostfixError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<Mp4Error> for PostfixError {
    fn from(error: Mp4Error) -> Self {
        Self::Mp4(error)
    }
}

impl From<ProbeError> for PostfixError {
    fn from(error: ProbeError) -> Self {
        Self::Probe(error)
    }
}

pub type Result<T> = std::result::Result<T, PostfixError>;

fn value<T>(message: String) -> Result<T> {
    Err(PostfixError::Value(message))
}

/// The `ftyp` atom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ftyp {
    /// Four ASCII characters.
    pub major_brand: String,
    pub minor_version: u32,
    /// Each four ASCII characters.
    pub compatible_brands: Vec<String>,
}

impl Ftyp {
    /// The atom's body (after the 8-byte size and type header).
    ///
    /// # Errors
    ///
    /// [`PostfixError::Value`] for a brand that is not four ASCII characters.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let brand = |what: &str, text: &str| -> Result<Vec<u8>> {
            if text.chars().count() != BRAND_LEN {
                return value(format!("{what} must be 4 chars: '{text}'"));
            }
            if !text.is_ascii() {
                return value(format!("{what} must be ASCII: '{text}'"));
            }
            Ok(text.as_bytes().to_vec())
        };
        let mut payload = brand("major_brand", &self.major_brand)?;
        payload.extend_from_slice(&self.minor_version.to_be_bytes());
        for compatible in &self.compatible_brands {
            payload.extend_from_slice(&brand("brand", compatible)?);
        }
        Ok(payload)
    }
}

/// Read the `ftyp` atom from the start of an MP4 file.
///
/// # Errors
///
/// [`PostfixError::Value`] when the file does not begin with a well-formed `ftyp`.
pub fn read_ftyp(path: &Path) -> Result<Ftyp> {
    let mut file = File::open(path)?;
    let mut header = [0_u8; 8];
    let got = read_up_to(&mut file, &mut header)?;
    if got < 8 {
        return value(format!("{}: too small to contain ftyp", path.display()));
    }
    let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    if &header[4..8] != b"ftyp" {
        return value(format!("{}: does not begin with ftyp atom", path.display()));
    }
    if !(16..=1024).contains(&size) {
        return value(format!("{}: implausible ftyp size {size}", path.display()));
    }
    let mut body = vec![0_u8; size as usize - 8];
    let got = read_up_to(&mut file, &mut body)?;
    body.truncate(got);
    if body.len() < 8 {
        return value(format!("{}: ftyp body is truncated", path.display()));
    }
    let ascii = |bytes: &[u8]| -> Result<String> {
        if bytes.is_ascii() {
            Ok(String::from_utf8_lossy(bytes).into_owned())
        } else {
            value(format!("{}: ftyp brand is not ASCII", path.display()))
        }
    };
    let major_brand = ascii(&body[0..4])?;
    let minor_version = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
    let brand_bytes = &body[8..];
    if !brand_bytes.len().is_multiple_of(BRAND_LEN) {
        return value(format!(
            "{}: ftyp brand list length {} not multiple of 4",
            path.display(),
            brand_bytes.len()
        ));
    }
    let compatible_brands = brand_bytes
        .chunks(BRAND_LEN)
        .map(ascii)
        .collect::<Result<Vec<_>>>()?;
    Ok(Ftyp {
        major_brand,
        minor_version,
        compatible_brands,
    })
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

/// Replace the `ftyp` atom with `new_ftyp`, preserving its byte size; the file changes in place.
///
/// # Errors
///
/// [`PostfixError::Value`] when the number of brands differs from the file's.
pub fn rewrite_ftyp_size_neutral(path: &Path, new_ftyp: &Ftyp) -> Result<()> {
    let original = read_ftyp(path)?;
    let original_body_size = 4 + 4 + BRAND_LEN * original.compatible_brands.len();
    let new_body = new_ftyp.encode()?;
    if new_body.len() != original_body_size {
        return value(format!(
            "size-neutral rewrite requires same number of brands (original={}, new={})",
            original.compatible_brands.len(),
            new_ftyp.compatible_brands.len()
        ));
    }
    // The existing 8-byte header (size and type) stays, so nothing downstream shifts.
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    file.seek(SeekFrom::Start(8))?;
    file.write_all(&new_body)?;
    Ok(())
}

/// Remove `brand_to_remove` from the compatible brands, padding with a duplicate brand so the atom
/// size stays the same. Returns whether the file was modified.
///
/// `pad_with` defaults to the first brand that is not the one being removed.
///
/// # Errors
///
/// [`PostfixError::Value`] for malformed brands or when there is no other brand to pad with.
pub fn strip_brand_size_neutral(
    path: &Path,
    brand_to_remove: &str,
    pad_with: Option<&str>,
) -> Result<bool> {
    if brand_to_remove.chars().count() != BRAND_LEN {
        return value(format!(
            "brand_to_remove must be 4 chars: '{brand_to_remove}'"
        ));
    }
    let original = read_ftyp(path)?;
    if !original
        .compatible_brands
        .iter()
        .any(|b| b == brand_to_remove)
    {
        return Ok(false);
    }
    let pad = if let Some(pad) = pad_with {
        pad.to_owned()
    } else if let Some(other) = original
        .compatible_brands
        .iter()
        .find(|b| *b != brand_to_remove)
    {
        other.clone()
    } else if original.major_brand != brand_to_remove {
        original.major_brand.clone()
    } else {
        return value(format!(
            "{}: cannot strip '{brand_to_remove}' \u{2014} no other brand available to pad with",
            path.display()
        ));
    };
    if pad.chars().count() != BRAND_LEN {
        return value(format!("pad_with must be 4 chars: '{pad}'"));
    }
    if pad == brand_to_remove {
        return value(format!(
            "pad_with ('{pad}') must differ from brand_to_remove"
        ));
    }
    let compatible_brands = original
        .compatible_brands
        .iter()
        .map(|b| {
            if b == brand_to_remove {
                pad.clone()
            } else {
                b.clone()
            }
        })
        .collect();
    rewrite_ftyp_size_neutral(
        path,
        &Ftyp {
            major_brand: original.major_brand,
            minor_version: original.minor_version,
            compatible_brands,
        },
    )?;
    Ok(true)
}

/// Strip `dby1` from the compatible brands (the Atmos-silence mitigation). Returns whether the file
/// was modified.
///
/// # Errors
///
/// As [`strip_brand_size_neutral`].
pub fn normalize_ftyp_for_no_atmos(path: &Path) -> Result<bool> {
    strip_brand_size_neutral(path, "dby1", None)
}

#[derive(Debug, Clone, Copy)]
struct Atom {
    offset: u64,
    size: u64,
    kind: [u8; 4],
    payload_offset: u64,
    payload_size: u64,
}

fn kind_text(kind: [u8; 4]) -> String {
    String::from_utf8_lossy(&kind).into_owned()
}

/// The atom header at `offset`; `None` at end of file. Size 0 (atom to end of file) is refused.
fn read_atom_header(file: &mut File, offset: u64) -> Result<Option<Atom>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut header = [0_u8; 8];
    if read_up_to(file, &mut header)? < 8 {
        return Ok(None);
    }
    let kind = [header[4], header[5], header[6], header[7]];
    if !kind.is_ascii() {
        return value(format!("non-ASCII atom type at offset {offset}: {kind:?}"));
    }
    let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let (total, header_size) = match size32 {
        1 => {
            let mut extended = [0_u8; 8];
            if read_up_to(file, &mut extended)? < 8 {
                return value(format!(
                    "truncated 64-bit size for {} at {offset}",
                    kind_text(kind)
                ));
            }
            (u64::from_be_bytes(extended), 16)
        }
        0 => {
            return value(format!(
                "{} at {offset}: size=0 (atom-to-EOF) not supported",
                kind_text(kind)
            ));
        }
        n => (u64::from(n), 8),
    };
    if total < header_size {
        return value(format!(
            "{} at {offset}: size {total} < header {header_size}",
            kind_text(kind)
        ));
    }
    Ok(Some(Atom {
        offset,
        size: total,
        kind,
        payload_offset: offset + header_size,
        payload_size: total - header_size,
    }))
}

/// The direct children of `parent`, starting `extra` bytes into its payload.
fn children_after(file: &mut File, parent: &Atom, extra: u64) -> Result<Vec<Atom>> {
    let mut cursor = parent.payload_offset + extra;
    let end = parent.payload_offset + parent.payload_size;
    let mut found = Vec::new();
    while cursor < end {
        let Some(atom) = read_atom_header(file, cursor)? else {
            break;
        };
        cursor += atom.size;
        found.push(atom);
    }
    Ok(found)
}

fn children(file: &mut File, parent: &Atom) -> Result<Vec<Atom>> {
    children_after(file, parent, 0)
}

/// The first child of that type; later children are not read (a malformed one after it is never seen).
fn find_child(file: &mut File, parent: &Atom, kind: [u8; 4]) -> Result<Option<Atom>> {
    let mut cursor = parent.payload_offset;
    let end = parent.payload_offset + parent.payload_size;
    while cursor < end {
        let Some(atom) = read_atom_header(file, cursor)? else {
            return Ok(None);
        };
        if atom.kind == kind {
            return Ok(Some(atom));
        }
        cursor += atom.size;
    }
    Ok(None)
}

/// The moov atom; it must precede any mdat (faststart), which the size-neutral rewrites need.
fn find_moov(path: &Path) -> Result<Atom> {
    let mut file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let mut saw_mdat = false;
    let mut cursor = 0;
    while cursor < file_size {
        let Some(atom) = read_atom_header(&mut file, cursor)? else {
            break;
        };
        if &atom.kind == b"mdat" {
            saw_mdat = true;
        } else if &atom.kind == b"moov" {
            if saw_mdat {
                return Err(PostfixError::NotImplemented(format!(
                    "{}: moov follows mdat (non-faststart MP4); size-neutral postfix requires faststart layout",
                    path.display()
                )));
            }
            return Ok(atom);
        }
        cursor += atom.size;
    }
    value(format!("{}: no moov atom found", path.display()))
}

fn handler_of(file: &mut File, hdlr: &Atom) -> Result<Option<[u8; 4]>> {
    file.seek(SeekFrom::Start(hdlr.payload_offset + 8))?;
    let mut bytes = [0_u8; 4];
    if read_up_to(file, &mut bytes)? < 4 || !bytes.is_ascii() {
        return Ok(None);
    }
    Ok(Some(bytes))
}

/// `(dec3 offset, dec3 payload size)` of every `dec3` on the audio path, in document order.
fn walk_audio_dec3_atoms(path: &Path) -> Result<Vec<(u64, u64)>> {
    let moov = find_moov(path)?;
    let mut file = File::open(path)?;
    let mut locations = Vec::new();
    for trak in children(&mut file, &moov)?
        .into_iter()
        .filter(|a| &a.kind == b"trak")
    {
        let Some(mdia) = find_child(&mut file, &trak, *b"mdia")? else {
            continue;
        };
        let Some(hdlr) = find_child(&mut file, &mdia, *b"hdlr")? else {
            continue;
        };
        if handler_of(&mut file, &hdlr)? != Some(*b"soun") {
            continue;
        }
        let Some(minf) = find_child(&mut file, &mdia, *b"minf")? else {
            continue;
        };
        let Some(stbl) = find_child(&mut file, &minf, *b"stbl")? else {
            continue;
        };
        let Some(stsd) = find_child(&mut file, &stbl, *b"stsd")? else {
            continue;
        };
        for entry in children_after(&mut file, &stsd, STSD_EXTRA)?
            .into_iter()
            .filter(|a| &a.kind == b"ec-3")
        {
            for sub in children_after(&mut file, &entry, EC3_EXTRA)? {
                if &sub.kind == b"dec3" {
                    locations.push((sub.offset, sub.payload_size));
                }
            }
        }
    }
    Ok(locations)
}

/// Clear `ec3_ext_flag` on every E-AC-3 track's `dec3` box (a legacy artifact of older runs that
/// signalled Atmos on plain Dolby Digital Plus). The trailing complexity byte becomes dead padding,
/// so the atom size and the sample tables stay valid. Returns the number of boxes modified.
///
/// # Errors
///
/// [`PostfixError`] for a layout the rewrite cannot follow; nothing is written then.
pub fn normalize_dec3_for_no_atmos(path: &Path) -> Result<u32> {
    let locations = walk_audio_dec3_atoms(path)?;
    if locations.is_empty() {
        return Ok(0);
    }
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut patches = Vec::new();
    for (dec3_offset, payload_size) in locations {
        file.seek(SeekFrom::Start(dec3_offset + 8))?;
        let mut payload = vec![
            0_u8;
            usize::try_from(payload_size)
                .map_err(|_| PostfixError::Value("dec3 too large".to_owned()))?
        ];
        let got = read_up_to(&mut file, &mut payload)?;
        payload.truncate(got);
        let (_bsids, ext_byte) = parse_dec3(&payload);
        if ext_byte == 0 || ext_byte > payload.len() {
            continue;
        }
        let flag_byte = ext_byte - 1;
        if payload[flag_byte] & 0x01 == 0 {
            continue; // flag already clear
        }
        patches.push((
            dec3_offset + 8 + flag_byte as u64,
            payload[flag_byte] & 0xFE,
        ));
    }
    for &(offset, byte) in &patches {
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&[byte])?;
    }
    Ok(u32::try_from(patches.len()).unwrap_or(u32::MAX))
}

#[derive(Debug, Clone)]
struct TrackLayout {
    hdlr: Atom,
    handler_type: [u8; 4],
    tkhd_flags: u32,
    flags_offset: u64,
    track_id: u32,
}

/// One layout per `trak` that has a `tkhd`, `mdia` and `hdlr`, in document order.
fn read_tracks(path: &Path) -> Result<Vec<TrackLayout>> {
    let moov = find_moov(path)?;
    let mut file = File::open(path)?;
    let mut tracks = Vec::new();
    for trak in children(&mut file, &moov)?
        .into_iter()
        .filter(|a| &a.kind == b"trak")
    {
        let tkhd = find_child(&mut file, &trak, *b"tkhd")?;
        let mdia = find_child(&mut file, &trak, *b"mdia")?;
        let (Some(tkhd), Some(mdia)) = (tkhd, mdia) else {
            continue;
        };
        let Some(hdlr) = find_child(&mut file, &mdia, *b"hdlr")? else {
            continue;
        };
        file.seek(SeekFrom::Start(tkhd.payload_offset))?;
        let mut head = [0_u8; 4];
        if read_up_to(&mut file, &mut head)? < 4 {
            continue;
        }
        let tkhd_flags = u32::from_be_bytes([0, head[1], head[2], head[3]]);
        file.seek(SeekFrom::Start(
            tkhd.payload_offset + if head[0] == 1 { 20 } else { 12 },
        ))?;
        let mut id = [0_u8; 4];
        if read_up_to(&mut file, &mut id)? < 4 {
            continue;
        }
        let Some(handler_type) = handler_of(&mut file, &hdlr)? else {
            continue;
        };
        tracks.push(TrackLayout {
            hdlr,
            handler_type,
            tkhd_flags,
            flags_offset: tkhd.payload_offset + 1,
            track_id: u32::from_be_bytes(id),
        });
    }
    Ok(tracks)
}

fn classify(handler: [u8; 4]) -> Option<Media> {
    match &handler {
        b"vide" => Some(Media::Video),
        b"soun" => Some(Media::Audio),
        b"sbtl" | b"text" | b"subt" => Some(Media::Subtitle),
        _ => None,
    }
}

/// Set the `tkhd` ENABLED flag on the tracks the Apple selection policy picks.
///
/// Every video, audio and subtitle `trak` is enabled iff its track id is in the policy's set; a
/// `trak` without a role (a chapter `text` track) gets ENABLED cleared, unless the probe has no
/// role at all for its media type, in which case that type is left untouched. The other flag bits
/// are preserved. Returns the number of tracks whose flags were modified.
///
/// # Errors
///
/// [`PostfixError`] for a layout the rewrite cannot follow; nothing is written then.
pub fn set_enabled_flags_by_rule(path: &Path, probe: &Value) -> Result<u32> {
    let roles = roles_from_probe(probe);
    let enabled = enabled_track_ids(&roles);
    let probed: BTreeSet<_> = roles.iter().map(|r| r.media as u8).collect();
    let mut patches = Vec::new();
    for track in read_tracks(path)? {
        // A media type the probe reports no stream for (ffprobe may call a real trak "data") is
        // left alone rather than disabled wholesale.
        let Some(media) = classify(track.handler_type) else {
            continue;
        };
        if !probed.contains(&(media as u8)) {
            continue;
        }
        let flags = if enabled.contains(&track.track_id) {
            track.tkhd_flags | TKHD_FLAG_ENABLED
        } else {
            track.tkhd_flags & !TKHD_FLAG_ENABLED
        };
        if flags != track.tkhd_flags {
            patches.push((track.flags_offset, flags));
        }
    }
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    for &(offset, flags) in &patches {
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&flags.to_be_bytes()[1..])?;
    }
    Ok(u32::try_from(patches.len()).unwrap_or(u32::MAX))
}

/// Rewrite every `hdlr` handler type `text` to `sbtl` in place, except a chapter track (one
/// another track names in `tref/chap`): `AVFoundation` reads chapters only from a `text` track.
/// Returns the number of handlers rewritten.
///
/// # Errors
///
/// [`PostfixError`] for a layout the rewrite cannot follow; nothing is written then.
pub fn set_subtitle_handler_types_to_sbtl(path: &Path) -> Result<u32> {
    let tracks = read_tracks(path)?;
    let chapters = chapter_track_ids(path)?;
    let targets: Vec<&TrackLayout> = tracks
        .iter()
        .filter(|t| &t.handler_type == b"text" && !chapters.contains(&t.track_id))
        .collect();
    if targets.is_empty() {
        return Ok(0);
    }
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    for track in &targets {
        file.seek(SeekFrom::Start(track.hdlr.payload_offset + 8))?;
        file.write_all(b"sbtl")?;
    }
    Ok(u32::try_from(targets.len()).unwrap_or(u32::MAX))
}

/// What [`bring_into_compliance`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    pub ftyp_modified: u32,
    pub dec3_modified: u32,
    pub tkhd_modified: u32,
    pub hdlr_modified: u32,
}

/// Run every size-neutral compliance rewrite against `path`.
///
/// Without Atmos: strip `dby1` from the brands and clear a false `ec3_ext_flag`. Always: set the
/// ENABLED flags by the selection policy (probing `path` with `ffprobe` unless `probe` is given)
/// and turn `text` handlers into `sbtl`.
///
/// # Errors
///
/// [`PostfixError`]. The layout is validated before the first write, so a file that cannot be
/// followed is left untouched.
pub fn bring_into_compliance(
    path: &Path,
    has_atmos: bool,
    probe: Option<&Value>,
    ffprobe: &Ffprobe,
) -> Result<Counters> {
    find_moov(path)?; // refuse an unsupported layout before anything is written
    let (mut ftyp_modified, mut dec3_modified) = (0, 0);
    if !has_atmos {
        ftyp_modified = u32::from(normalize_ftyp_for_no_atmos(path)?);
        dec3_modified = normalize_dec3_for_no_atmos(path)?;
    }
    let probed;
    let probe = if let Some(probe) = probe {
        probe
    } else {
        probed = ffprobe.probe_json(path)?;
        &probed
    };
    let tkhd_modified = set_enabled_flags_by_rule(path, probe)?;
    let hdlr_modified = set_subtitle_handler_types_to_sbtl(path)?;
    Ok(Counters {
        ftyp_modified,
        dec3_modified,
        tkhd_modified,
        hdlr_modified,
    })
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
        args: Value,
    }

    #[derive(Deserialize)]
    struct Expected {
        result: Value,
        sha: String,
        size: u64,
        changed: bool,
    }

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    }

    fn run(op: &str, args: &Value, path: &Path) -> std::result::Result<Value, PostfixError> {
        Ok(match op {
            "read_ftyp" => {
                let f = read_ftyp(path)?;
                json!({"major": f.major_brand, "minor": f.minor_version, "brands": f.compatible_brands})
            }
            "strip" => json!(strip_brand_size_neutral(
                path,
                args["brand"].as_str().unwrap(),
                args["pad_with"].as_str()
            )?),
            "normalize_ftyp" => json!(normalize_ftyp_for_no_atmos(path)?),
            "normalize_dec3" => json!(normalize_dec3_for_no_atmos(path)?),
            "enabled_flags" => json!(set_enabled_flags_by_rule(path, &args["probe"])?),
            "sbtl" => json!(set_subtitle_handler_types_to_sbtl(path)?),
            "bring" => {
                let c = bring_into_compliance(
                    path,
                    args["has_atmos"].as_bool().unwrap(),
                    Some(&args["probe"]),
                    &Ffprobe::new(),
                )?;
                json!({"ftyp_modified": c.ftyp_modified, "dec3_modified": c.dec3_modified, "tkhd_modified": c.tkhd_modified, "hdlr_modified": c.hdlr_modified})
            }
            "rewrite" => {
                let ftyp = Ftyp {
                    major_brand: args["major"].as_str().unwrap().to_owned(),
                    minor_version: u32::try_from(args["minor"].as_u64().unwrap()).unwrap(),
                    compatible_brands: strings(&args["brands"]),
                };
                rewrite_ftyp_size_neutral(path, &ftyp)?;
                Value::Null
            }
            other => panic!("unknown op {other}"),
        })
    }

    #[test]
    fn postfix_matches_the_python_oracle() {
        let cases = load_golden::<Input, Expected>("postfix");
        assert!(cases.len() > 200);
        for case in cases {
            let name = &case.name;
            let original = std::fs::read(fixture_path(&case.input.file)).unwrap();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(&case.input.file);
            std::fs::write(&path, &original).unwrap();
            let result = run(&case.input.op, &case.input.args, &path);
            let after = std::fs::read(&path).unwrap();
            match (
                &result,
                case.expected.result.get("raises").and_then(Value::as_str),
            ) {
                (Ok(value), None) => {
                    assert_eq!(value, &case.expected.result["ok"], "{name} result");
                    assert_eq!(sha256_hex(&after), case.expected.sha, "{name} bytes");
                    assert_eq!(after.len() as u64, case.expected.size, "{name} size");
                    assert_eq!(after != original, case.expected.changed, "{name} changed");
                }
                (Err(PostfixError::NotImplemented(_)), Some("NotImplementedError"))
                | (
                    Err(PostfixError::Value(_)),
                    Some("ValueError" | "UnicodeDecodeError" | "error"),
                ) => {
                    assert_eq!(after, original, "{name}: a failed call changed the file");
                }
                (other, expected) => panic!("{name}: got {other:?}, Python raised {expected:?}"),
            }
        }
    }

    #[test]
    fn strip_brand_pads_with_the_first_other_brand_and_keeps_the_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dby1.mp4");
        std::fs::copy(fixture_path("dby1.mp4"), &path).unwrap();
        let before = read_ftyp(&path).unwrap();
        assert!(before.compatible_brands.contains(&"dby1".to_owned()));
        assert!(normalize_ftyp_for_no_atmos(&path).unwrap());
        let after = read_ftyp(&path).unwrap();
        assert_eq!(
            after.compatible_brands.len(),
            before.compatible_brands.len()
        );
        assert!(!after.compatible_brands.contains(&"dby1".to_owned()));
        assert_eq!(
            after.compatible_brands.last().map(String::as_str),
            Some("isom")
        );
        assert!(
            !normalize_ftyp_for_no_atmos(&path).unwrap(),
            "a second run changes nothing"
        );
    }

    #[test]
    fn a_moov_after_mdat_is_not_implemented_with_the_python_text() {
        let error = normalize_dec3_for_no_atmos(&fixture_path("plain_moov_last.mp4")).unwrap_err();
        assert!(
            matches!(&error, PostfixError::NotImplemented(m) if m.ends_with(
            ": moov follows mdat (non-faststart MP4); size-neutral postfix requires faststart layout")),
            "{error}"
        );
    }

    #[test]
    fn an_atom_with_size_zero_or_below_its_header_is_a_value_error() {
        let error =
            normalize_dec3_for_no_atmos(&fixture_path("syn_zero_size_box.mp4")).unwrap_err();
        assert!(
            matches!(&error, PostfixError::Value(m) if m.contains("size=0 (atom-to-EOF) not supported")),
            "{error}"
        );
        assert!(read_ftyp(&fixture_path("syn_not_mp4.mp4")).is_err());
    }

    #[test]
    fn a_chapter_track_keeps_its_text_handler_and_enabled_flags_keep_other_bits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("syn_chapters.mp4");
        std::fs::copy(fixture_path("syn_chapters.mp4"), &path).unwrap();
        // Track 4 (sbtl) and track 5 (the chapter track, handler text): only a real text subtitle is rewritten.
        assert_eq!(set_subtitle_handler_types_to_sbtl(&path).unwrap(), 0);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes.windows(4).any(|w| w == b"text"),
            "the chapter handler is still text"
        );
    }
}
