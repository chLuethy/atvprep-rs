//! Where `prepare` puts things: temp names, the output file, the sidecar list.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sidecars::find_sidecar_srts;

/// Leads every working file; `FdFixTarget` and the manager scanner rely on it.
pub const TEMP_PREFIX: &str = ".tmp_fixmeta_";
const NAME_MAX: usize = 255;
const MP4_LIKE: [&str; 3] = ["mp4", "m4v", "mov"];

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// `.tmp_fixmeta_<tag><stem>.<pid>-<8 hex>.mp4`: unique per call, prefix first, and short enough
/// for one directory entry (a long stem is cut at a character boundary).
#[must_use]
pub fn temp_name(stem: &str, tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let salt = nanos
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B1);
    let unique = format!(".{}-{salt:08x}.mp4", std::process::id());
    let room = NAME_MAX.saturating_sub(TEMP_PREFIX.len() + tag.len() + unique.len());
    let mut end = stem.len().min(room);
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{TEMP_PREFIX}{tag}{}{unique}", &stem[..end])
}

/// The PID inside a working-file name, or `None` when `name` is not exactly
/// `.tmp_fixmeta_<tag><stem>.<pid>-<8 lowercase hex>.mp4` (the shape `temp_name` makes).
#[must_use]
pub fn parse_temp_name(name: &str) -> Option<u32> {
    let rest = name.strip_prefix(TEMP_PREFIX)?.strip_suffix(".mp4")?;
    let (_, unique) = rest.rsplit_once('.')?;
    let (pid, salt) = unique.split_once('-')?;
    let hex = salt.len() == 8 && salt.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) || !hex {
        return None;
    }
    pid.parse().ok()
}

fn is_mp4_like(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| MP4_LIKE.iter().any(|m| e.eq_ignore_ascii_case(m)))
}

/// Whether `a` and `b` name the same file: the same resolved path, or one existing file.
#[must_use]
pub fn same_path(a: &Path, b: &Path) -> bool {
    if resolve(a) == resolve(b) {
        return true;
    }
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => same_file(&x, &y),
        _ => false,
    }
}

fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// `Path.resolve()`: walk the absolute path component by component, following symlinks that
/// exist and treating `..` of a missing directory lexically.
#[must_use]
pub fn resolve(path: &Path) -> PathBuf {
    use std::path::Component;
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            Component::Normal(name) => {
                out.push(name);
                if let Ok(real) = out.canonicalize() {
                    out = real;
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// The file `output` names, or `None` when the run needs no separate output.
///
/// A directory, or a suffix-less path that does not exist yet (a directory still to be created),
/// names `<dir>/<stem>.mp4`. `None` means `output` is `source` itself, or the sibling `<stem>.mp4`
/// of another container: that run takes the sibling route.
#[must_use]
pub fn output_path(source: &Path, output: Option<&Path>) -> Option<PathBuf> {
    let output = output?;
    let is_dir = output.is_dir() || (output.extension().is_none() && !output.exists());
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    let path = if is_dir {
        output.join(format!("{stem}.mp4"))
    } else {
        output.to_path_buf()
    };
    if same_path(&path, source) {
        return None;
    }
    if !is_mp4_like(source) && same_path(&path, &source.with_file_name(format!("{stem}.mp4"))) {
        return None;
    }
    Some(path)
}

/// The SRTs next to `source`, then those of `extra` whose file name is not among them yet.
#[must_use]
pub fn sidecars(source: &Path, extra: &[PathBuf]) -> Vec<PathBuf> {
    let beside = find_sidecar_srts(source).unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut merged = Vec::new();
    for path in beside.iter().chain(extra) {
        let name = path.file_name().map(std::ffi::OsStr::to_os_string);
        if let Some(name) = name
            && seen.insert(name)
        {
            merged.push(path.clone());
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use atvprep_testkit::load_golden;
    use serde::Deserialize;
    use serde_json::Value;

    use super::*;

    #[derive(Deserialize)]
    struct Input {
        kind: String,
        source: String,
        #[serde(default)]
        output: Option<String>,
        #[serde(default)]
        extra: Vec<String>,
    }

    fn populate(base: &Path) {
        for name in [
            "a.mp4", "b.mkv", "b.mp4", "c.mkv", "s.de.srt", "a.en.srt", "A.FR.SRT",
        ] {
            std::fs::write(base.join(name), "x").unwrap();
        }
        std::fs::create_dir(base.join("out")).unwrap();
        std::fs::create_dir(base.join("extra")).unwrap();
        std::fs::write(base.join("extra/a.nl.srt"), "x").unwrap();
        std::fs::write(base.join("extra/a.en.srt"), "y").unwrap();
        std::os::unix::fs::symlink(base.join("a.mp4"), base.join("link.mp4")).unwrap();
    }

    fn rel(base: &Path, path: &Path) -> String {
        path.strip_prefix(base)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn output_path_and_sidecars_match_the_python_oracle() {
        let cases = load_golden::<Input, Value>("prepare_helpers");
        assert_eq!(cases.len(), 21);
        for case in cases {
            let dir = tempfile::tempdir().unwrap();
            let base = dir.path().canonicalize().unwrap();
            populate(&base);
            let input = &case.input;
            let source = base.join(&input.source);
            let got = if input.kind == "output" {
                let output = input.output.as_deref().map(|o| base.join(o));
                let found = output_path(&source, output.as_deref());
                found.map_or(Value::Null, |p| Value::String(rel(&base, &p)))
            } else {
                let extra: Vec<PathBuf> = input.extra.iter().map(|e| base.join(e)).collect();
                Value::from(
                    sidecars(&source, &extra)
                        .iter()
                        .map(|p| rel(&base, p))
                        .collect::<Vec<_>>(),
                )
            };
            assert_eq!(got, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn temp_names_fit_255_bytes_lead_with_the_prefix_and_differ_per_call() {
        for stem in [
            "a",
            &"b".repeat(100),
            &"c".repeat(250),
            &"ä".repeat(200),
            &"😀".repeat(100),
        ] {
            let first = temp_name(stem, "enc.");
            let second = temp_name(stem, "enc.");
            assert!(first.len() <= 255, "{} bytes", first.len());
            assert!(first.starts_with(".tmp_fixmeta_enc."));
            assert_eq!(Path::new(&first).extension().unwrap(), "mp4");
            assert_ne!(first, second);
        }
        let short = temp_name("Movie", "");
        assert!(short.starts_with(".tmp_fixmeta_Movie."));
        assert!(short.contains(&format!(".{}-", std::process::id())));
    }

    #[test]
    fn working_file_names_are_recognized_exactly_and_yield_their_pid() {
        assert_eq!(
            parse_temp_name(".tmp_fixmeta_enc.Film (2020).4711-0a1b2c3d.mp4"),
            Some(4711)
        );
        assert_eq!(parse_temp_name(".tmp_fixmeta_.1-deadbeef.mp4"), Some(1));
        assert_eq!(
            parse_temp_name(&temp_name("Movie", "enc.")),
            Some(std::process::id())
        );
        for other in [
            "Film.TMP.mkv",
            "tmp_fixmeta_a.1-deadbeef.mp4",
            ".tmp_fixmeta_a.1-deadbeef.mkv",
            ".tmp_fixmeta_a.1-DEADBEEF.mp4",
            ".tmp_fixmeta_a.x-deadbeef.mp4",
            ".tmp_fixmeta_a.1-deadbee.mp4",
            ".tmp_fixmeta_a.99999999999-deadbeef.mp4",
            "Film.mp4.tmp_moov.1-abcdef",
        ] {
            assert_eq!(parse_temp_name(other), None, "{other}");
        }
    }

    #[test]
    fn a_missing_path_is_resolved_through_its_existing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        assert!(same_path(&base.join("x/../y"), &base.join("y")));
        assert!(!same_path(&base.join("y"), &base.join("z")));
    }
}
