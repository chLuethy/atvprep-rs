//! Sidecar subtitle files: language from the name, discovery and classification (port of the
//! sidecar half of `atvprep/analyzers.py`).

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::constants::{LANG_MAP, TEXT_SUB_CODECS, normalize_lang};
use crate::probe_helpers::{object, subtitle_is_forced, truthy};

/// Languages written out in a file name, as Bazarr and many subtitle sites do (`Film.German.srt`).
const LANGUAGE_NAMES: &[(&str, &str)] = &[
    ("german", "deu"),
    ("deutsch", "deu"),
    ("english", "eng"),
    ("french", "fra"),
    ("francais", "fra"),
    ("français", "fra"),
    ("italian", "ita"),
    ("italiano", "ita"),
    ("spanish", "spa"),
    ("espanol", "spa"),
    ("español", "spa"),
    ("portuguese", "por"),
    ("dutch", "nld"),
    ("japanese", "jpn"),
    ("korean", "kor"),
    ("chinese", "zho"),
    ("russian", "rus"),
    ("turkish", "tur"),
    ("arabic", "ara"),
    ("polish", "pol"),
    ("swedish", "swe"),
    ("norwegian", "nor"),
    ("danish", "dan"),
    ("hindi", "hin"),
];

/// Sidecar subtitle formats atvprep reads. The manager also notices `.ass`, `.ssa` and `.vtt` files;
/// a fix leaves those alone.
pub const EMBEDDABLE_SIDECAR_SUFFIXES: [&str; 1] = [".srt"];

fn lookup(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|&&(k, _)| k == key).map(|&(_, v)| v)
}

/// One piece of a file name as a language: a code (`de`, `ger`, `deu`), a region code (`pt-BR`,
/// `de_DE`) or a name.
fn language_from_token(token: &str) -> String {
    if let Some(code) = lookup(LANG_MAP, token).or_else(|| lookup(LANGUAGE_NAMES, token)) {
        return code.to_owned();
    }
    for separator in ['-', '_'] {
        if let Some((head, _)) = token.split_once(separator)
            && let Some(code) = lookup(LANG_MAP, head)
        {
            return code.to_owned();
        }
    }
    "und".to_owned()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Infer the language from a sidecar name, e.g. `Film.de.srt`, `Film.German.srt`, `Film.pt-BR.srt`.
#[must_use]
pub fn guess_lang_from_srt_name(video: &Path, srt: &Path) -> String {
    let base = video
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let name = file_name(srt);
    let Some(rest) = name.strip_prefix(&base) else {
        return "und".to_owned();
    };
    rest.to_lowercase()
        .split('.')
        .map(language_from_token)
        .find(|lang| lang != "und")
        .unwrap_or_else(|| "und".to_owned())
}

/// Infer the language code from a `.srt` path without a reference video path.
#[must_use]
pub fn lang_from_srt_path(srt: &Path) -> String {
    let name = file_name(srt).to_lowercase();
    LANG_MAP
        .iter()
        .find(|&&(key, _)| {
            name.contains(&format!(".{key}.srt")) || name.contains(&format!(".{key}.forced.srt"))
        })
        .map_or_else(|| "und".to_owned(), |&(_, code)| code.to_owned())
}

/// All `.srt` files in the video's directory whose name starts with the video stem, sorted by
/// lower-cased name.
///
/// Reads the directory instead of globbing so `[` and `]` in typical Radarr names such as
/// `Film [2020].mkv` stay literal.
///
/// # Errors
///
/// Returns the I/O error of reading the directory.
pub fn find_sidecar_srts(video: &Path) -> io::Result<Vec<PathBuf>> {
    let stem = video
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let parent = video.parent().filter(|p| !p.as_os_str().is_empty());
    let mut found = Vec::new();
    for entry in std::fs::read_dir(parent.unwrap_or_else(|| Path::new(".")))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = parent.map_or_else(|| PathBuf::from(&name), |p| p.join(&name));
        let is_srt = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("srt"));
        if is_srt && name.starts_with(&stem) && path.is_file() {
            found.push(path);
        }
    }
    found.sort_by_key(|p| file_name(p).to_lowercase());
    Ok(found)
}

/// The three kinds of subtitle track that can exist side by side for one language.
#[must_use]
pub const fn sub_kind(forced: bool, sdh: bool) -> &'static str {
    if forced {
        "forced"
    } else if sdh {
        "sdh"
    } else {
        "normal"
    }
}

/// Forced and SDH flags of a subtitle stream.
///
/// SDH comes from the `hearing_impaired` disposition or an SDH hint in the title. The disposition
/// matters for MP4: its muxer drops subtitle titles, so a prepared SDH track keeps only the flag
/// (#62). [`sub_kind`] lets forced win over SDH.
#[must_use]
pub fn sub_info_from_stream(stream: &Value) -> (bool, bool) {
    let title = object(stream, "tags")
        .and_then(|t| t.get("title"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let sdh = truthy(object(stream, "disposition").and_then(|d| d.get("hearing_impaired")))
        || ["sdh", " hi ", "hearing", "hörgeschädigt"]
            .iter()
            .any(|x| title.contains(x));
    (subtitle_is_forced(stream), sdh)
}

/// Forced and SDH flags from an SRT file name.
#[must_use]
pub fn sub_info_from_srt_name(srt: &Path) -> (bool, bool) {
    let name = file_name(srt).to_lowercase();
    (
        name.contains("forced"),
        ["sdh", ".hi.", "hearing"].iter().any(|x| name.contains(x)),
    )
}

/// Normal, forced or SDH, read from a sidecar's file name.
#[must_use]
pub fn sidecar_kind(name: &str) -> &'static str {
    let (forced, sdh) = sub_info_from_srt_name(Path::new(name));
    sub_kind(forced, sdh)
}

fn has_embeddable_suffix(name: &str) -> bool {
    let suffix = Path::new(name).extension().map_or_else(String::new, |e| {
        format!(".{}", e.to_string_lossy().to_lowercase())
    });
    EMBEDDABLE_SIDECAR_SUFFIXES.contains(&suffix.as_str())
}

/// What a fix or convert would do with each sidecar, as `(name, language, state)`.
///
/// Mirrors `analyze_subs` so the answer matches what the remux does. States: `pending` (wanted and
/// not inside the file yet), `embedded` (the file already has that language and kind),
/// `duplicate` (another `.srt` of the same language and kind is merged instead, the first by
/// lower-cased name wins), `ignored_language`, `unknown_language`, `unsupported` (not an `.srt`),
/// `unchecked` (no current ffprobe result to compare with, `streams` is `None`).
#[must_use]
pub fn classify_sidecars(
    video_name: &str,
    sidecar_names: &[String],
    streams: Option<&[Value]>,
) -> Vec<(String, String, String)> {
    let video = Path::new(video_name);
    let mut audio_langs = BTreeSet::new();
    let mut internal: BTreeSet<(String, &'static str)> = BTreeSet::new();
    for stream in streams.unwrap_or_default() {
        let language = object(stream, "tags")
            .and_then(|t| t.get("language"))
            .and_then(Value::as_str);
        let lang = normalize_lang(language);
        match stream.get("codec_type").and_then(Value::as_str) {
            Some("audio") if lang != "und" => {
                audio_langs.insert(lang);
            }
            Some("subtitle") => {
                let codec = stream
                    .get("codec_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase();
                if TEXT_SUB_CODECS.contains(&codec.as_str()) || codec == "mov_text" {
                    let (forced, sdh) = sub_info_from_stream(stream);
                    internal.insert((lang, sub_kind(forced, sdh)));
                }
            }
            _ => {}
        }
    }
    let mut result: Vec<(String, String, String)> = sidecar_names
        .iter()
        .map(|name| {
            let state = |lang: &str, state: &str| (name.clone(), lang.to_owned(), state.to_owned());
            if !has_embeddable_suffix(name) {
                return state("und", "unsupported");
            }
            let lang = guess_lang_from_srt_name(video, Path::new(name));
            if lang == "und" {
                state(&lang, "unknown_language")
            } else if streams.is_none() {
                state(&lang, "unchecked")
            } else if lang != "deu" && lang != "eng" && !audio_langs.contains(&lang) {
                state(&lang, "ignored_language")
            } else if internal.contains(&(lang.clone(), sidecar_kind(name))) {
                state(&lang, "embedded")
            } else {
                state(&lang, "pending")
            }
        })
        .collect();
    // The remux keeps only the first normal subtitle per language (forced and SDH are all kept).
    let mut winner: BTreeMap<String, String> = BTreeMap::new();
    for (name, lang, state) in &result {
        if state == "pending"
            && sidecar_kind(name) == "normal"
            && winner
                .get(lang)
                .is_none_or(|w| name.to_lowercase() < w.to_lowercase())
        {
            winner.insert(lang.clone(), name.clone());
        }
    }
    for (name, lang, state) in &mut result {
        if state == "pending" && sidecar_kind(name) == "normal" && winner.get(lang) != Some(name) {
            "duplicate".clone_into(state);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use atvprep_testkit::load_golden;
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;

    #[derive(Deserialize)]
    struct Input {
        #[serde(rename = "fn")]
        func: String,
        #[serde(default)]
        video: String,
        #[serde(default)]
        srt: String,
        #[serde(default)]
        stream: Value,
        #[serde(default)]
        names: Vec<String>,
        streams: Option<Vec<Value>>,
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        dirs: Vec<String>,
    }

    #[test]
    fn sidecar_logic_matches_the_python_oracle() {
        let cases = load_golden::<Input, Value>("sidecars");
        assert!(cases.len() > 100);
        let dir = tempfile::tempdir().unwrap();
        for case in cases {
            let i = &case.input;
            let actual = match i.func.as_str() {
                "guess" => Value::from(guess_lang_from_srt_name(
                    Path::new(&i.video),
                    Path::new(&i.srt),
                )),
                "path_lang" => Value::from(lang_from_srt_path(Path::new(&i.srt))),
                "kind" => {
                    let (forced, sdh) = sub_info_from_srt_name(Path::new(&i.srt));
                    json!({"info": [forced, sdh], "kind": sidecar_kind(&i.srt)})
                }
                "stream_info" => {
                    let (forced, sdh) = sub_info_from_stream(&i.stream);
                    json!([forced, sdh])
                }
                "classify" => json!(
                    classify_sidecars(&i.video, &i.names, i.streams.as_deref())
                        .into_iter()
                        .map(|(n, l, s)| json!([n, l, s]))
                        .collect::<Vec<_>>()
                ),
                "find" => {
                    for name in &i.files {
                        fs::write(dir.path().join(name), "x").unwrap();
                    }
                    for name in &i.dirs {
                        let _ = fs::create_dir(dir.path().join(name));
                    }
                    let found = find_sidecar_srts(&dir.path().join(&i.video)).unwrap();
                    json!(
                        found
                            .iter()
                            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                    )
                }
                other => panic!("unknown fn {other}"),
            };
            assert_eq!(actual, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn a_bare_video_name_searches_the_current_directory_without_a_prefix() {
        // Run from the crate directory: Cargo.toml is there, no .srt files are.
        assert_eq!(
            find_sidecar_srts(Path::new("Cargo.mkv")).unwrap(),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        assert!(find_sidecar_srts(Path::new("/definitely/not/here/Film.mkv")).is_err());
    }
}
