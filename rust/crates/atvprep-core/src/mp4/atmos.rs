//! Atmos (JOC) signalling in the `dec3` box (port of the Atmos half of `atvprep/mp4patch.py`).
//!
//! For E-AC-3 streams with `bsid == 16` (Dolby Atmos) `AVFoundation` requires `ec3_ext_flag` (the
//! least significant bit of the last padding byte) to be 1, followed by one byte of
//! `complexity_index_type_a`. ffmpeg 6 and older copy E-AC-3 from MKV without that extension.

use super::boxes::{iter_boxes, rebuild_container};
use super::error::Result;
use super::tags::trak_handler;

/// The standard Atmos `complexity_index_type_a` value.
const JOC_COMPLEXITY_INDEX: u8 = 0x10;

struct Bits<'a> {
    data: &'a [u8],
    position: usize,
}

impl Bits<'_> {
    /// `count` bits as a number; 0 (without advancing) when fewer remain.
    fn read(&mut self, count: usize) -> u32 {
        if self.position + count > self.data.len() * 8 {
            return 0;
        }
        let mut value = 0_u32;
        for bit in self.position..self.position + count {
            let byte = self.data[bit / 8];
            value = (value << 1) | u32::from((byte >> (7 - bit % 8)) & 1);
        }
        self.position += count;
        value
    }
}

/// Parse an `EC3SpecificBox` payload: `(bsids, ext_byte_offset)`.
///
/// `ext_byte_offset` is the index just past the byte that holds `ec3_ext_flag` as its least
/// significant bit, or 0 when the payload is byte-aligned (no slot for the flag).
#[must_use]
pub fn parse_dec3(payload: &[u8]) -> (Vec<u8>, usize) {
    if payload.len() < 2 {
        return (Vec::new(), 0);
    }
    let mut bits = Bits {
        data: payload,
        position: 0,
    };
    bits.read(13); // data_rate
    let substreams = bits.read(3) + 1;
    let mut bsids = Vec::new();
    for _ in 0..substreams {
        bits.read(2); // fscod
        bsids.push(u8::try_from(bits.read(5)).unwrap_or(0)); // bsid
        bits.read(1 + 1 + 3 + 3 + 1 + 5); // reserved, asvc, bsmod, acmod, lfeon, reserved
        let dependent = bits.read(3);
        bits.read(if dependent > 0 { 9 } else { 1 }); // chan_loc or reserved
    }
    if bits.position.is_multiple_of(8) {
        return (bsids, 0);
    }
    (bsids, bits.position.div_ceil(8))
}

/// `dec3` with the JOC extension added when it is an Atmos stream without it.
#[must_use]
pub fn patch_dec3_box(dec3: &[u8]) -> Vec<u8> {
    let payload = dec3.get(8..).unwrap_or_default();
    let (bsids, ext_byte) = parse_dec3(payload);
    if !bsids.contains(&16) || ext_byte == 0 || ext_byte > payload.len() {
        return dec3.to_vec(); // not Atmos, or an unexpected layout: leave untouched
    }
    let flag_byte = ext_byte - 1;
    if payload[flag_byte] & 0x01 != 0 {
        return dec3.to_vec(); // extension already present
    }
    let mut patched = payload.to_vec();
    patched[flag_byte] |= 0x01;
    patched.push(JOC_COMPLEXITY_INDEX);
    let mut out = u32::try_from(8 + patched.len())
        .unwrap_or(u32::MAX)
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(b"dec3");
    out.extend_from_slice(&patched);
    out
}

fn patch_ec3(ec3: &[u8]) -> Result<Vec<u8>> {
    // ec-3 sample entry: 8-byte header + 6 reserved + 2 data ref + 20 audio sample entry = 36
    rebuild_container(ec3, 36, &|kind, chunk| {
        Ok((kind == b"dec3").then(|| patch_dec3_box(chunk)))
    })
}

fn patch_stsd(stsd: &[u8]) -> Result<Vec<u8>> {
    // stsd full box: 8-byte header + version/flags + entry_count = 16
    rebuild_container(stsd, 16, &|kind, chunk| {
        if kind == b"ec-3" {
            patch_ec3(chunk).map(Some)
        } else {
            Ok(None)
        }
    })
}

fn patch_inside(
    parent: &[u8],
    child: [u8; 4],
    next: fn(&[u8]) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    rebuild_container(parent, 8, &|kind, chunk| {
        if *kind == child {
            next(chunk).map(Some)
        } else {
            Ok(None)
        }
    })
}

fn patch_stbl(stbl: &[u8]) -> Result<Vec<u8>> {
    rebuild_container(stbl, 8, &|kind, chunk| {
        if kind == b"stsd" {
            patch_stsd(chunk).map(Some)
        } else {
            Ok(None)
        }
    })
}

fn patch_minf(minf: &[u8]) -> Result<Vec<u8>> {
    patch_inside(minf, *b"stbl", patch_stbl)
}

fn patch_mdia(mdia: &[u8]) -> Result<Vec<u8>> {
    patch_inside(mdia, *b"minf", patch_minf)
}

fn patch_trak(trak: &[u8]) -> Result<Option<Vec<u8>>> {
    if trak_handler(trak) != Some(*b"soun") {
        return Ok(None);
    }
    patch_inside(trak, *b"mdia", patch_mdia).map(Some)
}

/// A new moov whose Atmos `dec3` boxes carry the JOC extension.
///
/// # Errors
///
/// [`Mp4Error`] for a box smaller than its header, or a rebuilt box beyond 4 GiB.
pub fn patch_moov_for_atmos(moov: &[u8]) -> Result<Vec<u8>> {
    rebuild_container(moov, 8, &|kind, chunk| {
        if kind == b"trak" {
            patch_trak(chunk)
        } else {
            Ok(None)
        }
    })
}

/// Whether any `dec3` below `moov` would be patched (cheap pre-check for callers).
#[must_use]
pub fn has_audio_track(moov: &[u8]) -> bool {
    iter_boxes(moov, 8, moov.len())
        .iter()
        .any(|b| &b.kind == b"trak" && trak_handler(&moov[b.start..b.end]) == Some(*b"soun"))
}

#[cfg(test)]
mod tests {
    use atvprep_testkit::{hex_decode, hex_encode, load_golden, sha256_hex};
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::super::tags::tests_support::moov_of;
    use super::*;

    #[derive(Deserialize)]
    struct Input {
        kind: String,
        #[serde(default)]
        hex: String,
        #[serde(default)]
        file: String,
    }

    fn case_hex(name: &str) -> Vec<u8> {
        let case = load_golden::<Input, Value>("mp4_atmos")
            .into_iter()
            .find(|c| c.name == name)
            .unwrap();
        hex_decode(&case.input.hex)
    }

    #[test]
    fn dec3_handling_matches_the_python_oracle() {
        let cases = load_golden::<Input, Value>("mp4_atmos");
        assert!(cases.len() >= 35);
        for case in cases {
            let input = &case.input;
            let actual = match input.kind.as_str() {
                "parse" => {
                    let (bsids, ext) = parse_dec3(&hex_decode(&input.hex));
                    json!({"bsids": bsids, "ext": ext})
                }
                "patch" => json!(hex_encode(&patch_dec3_box(&hex_decode(&input.hex)))),
                _ => {
                    let moov = moov_of(&input.file);
                    let patched = patch_moov_for_atmos(&moov).unwrap();
                    json!({"sha": sha256_hex(&patched), "len": patched.len(), "changed": patched != moov})
                }
            };
            assert_eq!(actual, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn patch_dec3_sets_the_flag_and_appends_the_complexity_byte() {
        let before = case_hex("patch plain ddp bsid 16 flag clear");
        let after = patch_dec3_box(&before);
        assert_eq!(after.len(), before.len() + 1);
        assert_eq!(after.last(), Some(&0x10));
        assert_eq!(
            patch_dec3_box(&after),
            after,
            "an already patched dec3 stays as it is"
        );
    }

    #[test]
    fn a_non_atmos_or_unparsable_dec3_is_returned_unchanged() {
        for name in [
            "patch non atmos bsid 6",
            "patch byte aligned no slot",
            "patch two zero bytes",
            "patch empty",
        ] {
            let before = case_hex(name);
            assert_eq!(patch_dec3_box(&before), before, "{name}");
        }
    }

    #[test]
    fn a_container_child_smaller_than_its_header_is_an_error() {
        let mut data = vec![0, 0, 0, 16, b'x', b'x', b'x', b'x'];
        data.extend_from_slice(&[0, 0, 0, 4, b'f', b'r', b'e', b'e']);
        assert!(rebuild_container(&data, 8, &|_, _| Ok(None)).is_err());
    }
}
