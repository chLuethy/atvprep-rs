//! Audio tracks whose packets duplicate an earlier track (port of `duplicate_audio_indices` and
//! `_identical_pairs` in `atvprep/probe.py`).
//!
//! Candidates share codec, channels, sample rate, language and bit rate. A candidate is confirmed
//! only when it has as many packets as the earlier stream AND every packet's MD5 matches, read over
//! the whole file: a shared silent intro must not make two different tracks look identical. All
//! candidate pairs are read in one ffprobe pass that stops as soon as every pair has differed. An
//! ffprobe failure means "no duplicate".

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::constants::normalize_lang;
use crate::ffprobe::Ffprobe;

type Pair = (i64, i64);

/// Pairs `(earlier, later)` of audio stream indices that share codec, channels, sample rate,
/// normalised language and bit rate.
#[must_use]
pub fn candidate_pairs(probe: &Value) -> BTreeSet<Pair> {
    let streams = probe
        .get("streams")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut groups: Vec<(String, Vec<i64>)> = Vec::new();
    for stream in streams
        .iter()
        .filter(|s| s.get("codec_type").and_then(Value::as_str) == Some("audio"))
    {
        let Some(index) = int_of(stream.get("index")) else {
            continue;
        };
        let language = stream
            .get("tags")
            .and_then(|t| t.get("language"))
            .and_then(Value::as_str);
        let key = Value::Array(vec![
            stream.get("codec_name").cloned().unwrap_or(Value::Null),
            stream.get("channels").cloned().unwrap_or(Value::Null),
            stream.get("sample_rate").cloned().unwrap_or(Value::Null),
            Value::from(normalize_lang(language)),
            stream.get("bit_rate").cloned().unwrap_or(Value::Null),
        ])
        .to_string();
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, indices)) => indices.push(index),
            None => groups.push((key, vec![index])),
        }
    }
    let mut pairs = BTreeSet::new();
    for (_, mut indices) in groups.into_iter().filter(|(_, i)| i.len() > 1) {
        indices.sort_unstable();
        for (n, &earlier) in indices.iter().enumerate() {
            pairs.extend(indices[n + 1..].iter().map(|&later| (earlier, later)));
        }
    }
    pairs
}

/// `int(x)` for a JSON integer or numeric string.
fn int_of(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// What the streaming comparison found, and whether the input ran to its end.
#[derive(Debug)]
pub struct Comparison {
    live: BTreeSet<Pair>,
    counts: BTreeMap<i64, usize>,
    compared: BTreeMap<Pair, usize>,
    /// True when the input ended by itself; false when reading stopped early or failed.
    pub exhausted: bool,
}

impl Comparison {
    /// The identical pairs, or none when the pass was cut short or ffprobe did not exit cleanly.
    #[must_use]
    pub fn finish(&self, exit_ok: bool) -> BTreeSet<Pair> {
        if !self.exhausted || !exit_ok {
            return BTreeSet::new();
        }
        self.live
            .iter()
            .copied()
            .filter(|&(first, second)| {
                let count = |i: i64| self.counts.get(&i).copied().unwrap_or(0);
                count(first) > 0
                    && count(first) == count(second)
                    && count(first) == self.compared[&(first, second)]
            })
            .collect()
    }
}

struct Streams {
    pending: BTreeMap<i64, VecDeque<String>>,
    count: BTreeMap<i64, usize>,
}

impl Streams {
    fn at(&self, index: i64, position: usize) -> Option<&String> {
        let skipped = self.count[&index] - self.pending[&index].len();
        self.pending[&index].get(position.checked_sub(skipped)?)
    }
}

/// Compare the packets of every pair in `pairs` as soon as both streams have the next one.
///
/// Every line is `stream_index,hash`. A pair drops out at its first difference, and reading stops
/// once none is left, so two different tracks cost seconds instead of a full read. Hashes both
/// streams of every remaining pair have passed are forgotten.
pub fn compare_streams(
    pairs: &BTreeSet<Pair>,
    lines: impl IntoIterator<Item = io::Result<String>>,
) -> Comparison {
    let mut alive = pairs.clone();
    let indices: BTreeSet<i64> = pairs.iter().flat_map(|&(a, b)| [a, b]).collect();
    let mut state = Streams {
        pending: indices.iter().map(|&i| (i, VecDeque::new())).collect(),
        count: indices.iter().map(|&i| (i, 0)).collect(),
    };
    let mut compared: BTreeMap<Pair, usize> = pairs.iter().map(|&p| (p, 0)).collect();
    let mut by_stream: BTreeMap<i64, Vec<Pair>> =
        indices.iter().map(|&i| (i, Vec::new())).collect();
    for &pair in pairs {
        by_stream.entry(pair.0).or_default().push(pair);
        by_stream.entry(pair.1).or_default().push(pair);
    }

    let mut exhausted = true;
    for line in lines {
        let Ok(line) = line else {
            exhausted = false;
            break;
        };
        let fields: Vec<&str> = line.trim().split(',').collect();
        if fields.len() < 2 {
            continue;
        }
        let Ok(index) = fields[0].trim().parse::<i64>() else {
            continue;
        };
        if !state.pending.contains_key(&index) {
            continue;
        }
        *state.count.entry(index).or_default() += 1;
        if !by_stream[&index].iter().any(|p| alive.contains(p)) {
            continue; // every pair of this stream already differed; only its count matters
        }
        state
            .pending
            .entry(index)
            .or_default()
            .push_back(fields[1].to_owned());
        for &pair in &by_stream[&index] {
            if !alive.contains(&pair) {
                continue;
            }
            let (first, second) = pair;
            let limit = state.count[&first].min(state.count[&second]);
            let mut done = compared[&pair];
            while done < limit && state.at(first, done) == state.at(second, done) {
                done += 1;
            }
            if done < limit {
                alive.remove(&pair);
            }
            compared.insert(pair, done);
        }
        let touched: BTreeSet<i64> = std::iter::once(index)
            .chain(by_stream[&index].iter().flat_map(|&(a, b)| [a, b]))
            .collect();
        for other in touched {
            let keep_from = by_stream[&other]
                .iter()
                .filter(|p| alive.contains(p))
                .map(|p| compared[p])
                .min()
                .unwrap_or(state.count[&other]);
            while let Some(queue) = state.pending.get_mut(&other) {
                if queue.is_empty() || state.count[&other] - queue.len() >= keep_from {
                    break;
                }
                queue.pop_front();
            }
        }
        if alive.is_empty() {
            exhausted = false;
            break;
        }
    }
    Comparison {
        live: alive,
        counts: state.count,
        compared,
        exhausted,
    }
}

impl Ffprobe {
    /// Stream indices of audio tracks whose packets duplicate an earlier audio track.
    ///
    /// Only groups of two or more candidates are read, all of them in one ffprobe pass. A failed
    /// or unreadable run means "no duplicate".
    #[must_use]
    pub fn duplicate_audio_indices(&self, path: &Path, probe: &Value) -> BTreeSet<i64> {
        let pairs = candidate_pairs(probe);
        if pairs.is_empty() {
            return BTreeSet::new(); // a lone candidate cannot be a duplicate; skip the spawn
        }
        let Ok(mut child) = Command::new(&self.program)
            .args([
                "-v",
                "error",
                "-select_streams",
                "a",
                "-show_data_hash",
                "MD5",
            ])
            .args([
                "-show_entries",
                "packet=stream_index,data_hash",
                "-of",
                "csv=p=0",
            ])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return BTreeSet::new();
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            return BTreeSet::new();
        };
        let comparison = compare_streams(&pairs, BufReader::new(stdout).lines());
        if !comparison.exhausted {
            let _ = child.kill();
        }
        let exit_ok = child.wait().is_ok_and(|status| status.success());
        comparison
            .finish(exit_ok)
            .into_iter()
            .map(|(_, later)| later)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::Path;

    use atvprep_testkit::load_golden;
    use serde::Deserialize;
    use serde_json::Value;

    use super::*;
    use crate::ffprobe::Ffprobe;

    #[derive(Deserialize)]
    struct Input {
        kind: String,
        #[serde(default)]
        pairs: Vec<(i64, i64)>,
        #[serde(default)]
        lines: Vec<String>,
        #[serde(default)]
        exit_code: i32,
        #[serde(default)]
        probe: Value,
    }

    #[derive(Deserialize)]
    struct Expected {
        #[serde(default)]
        identical: Vec<(i64, i64)>,
        #[serde(default)]
        consumed: usize,
        #[serde(default)]
        pairs: Vec<(i64, i64)>,
        #[serde(default)]
        indices: Vec<i64>,
    }

    #[test]
    fn comparison_and_candidates_match_the_python_oracle() {
        let cases = load_golden::<Input, Expected>("duplicate_audio");
        assert!(cases.len() >= 20);
        for case in cases {
            let name = &case.name;
            if case.input.kind == "lines" {
                let consumed = Cell::new(0_usize);
                let lines = case.input.lines.iter().map(|l| {
                    consumed.set(consumed.get() + 1);
                    Ok(l.clone())
                });
                let pairs: BTreeSet<(i64, i64)> = case.input.pairs.iter().copied().collect();
                let found = compare_streams(&pairs, lines).finish(case.input.exit_code == 0);
                let want: BTreeSet<(i64, i64)> = case.expected.identical.iter().copied().collect();
                assert_eq!(found, want, "{name}");
                assert_eq!(consumed.get(), case.expected.consumed, "{name} lines read");
            } else {
                let pairs = candidate_pairs(&case.input.probe);
                let want: BTreeSet<(i64, i64)> = case.expected.pairs.iter().copied().collect();
                assert_eq!(pairs, want, "{name}");
                let later: BTreeSet<i64> = pairs.iter().map(|&(_, later)| later).collect();
                assert_eq!(
                    later.into_iter().collect::<Vec<_>>(),
                    case.expected.indices,
                    "{name} indices"
                );
            }
        }
    }

    #[test]
    fn a_read_error_means_no_pair_is_identical() {
        let lines = vec![
            Ok("1,a".to_owned()),
            Err(std::io::Error::other("broken pipe")),
        ];
        let pairs = BTreeSet::from([(1, 2)]);
        assert!(compare_streams(&pairs, lines).finish(true).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_ffprobe_means_no_duplicate_and_a_missing_one_too() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("ffprobe");
        std::fs::write(&script, "#!/bin/sh\nprintf '1,a\\n2,a\\n'\nexit 1\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let probe = serde_json::json!({"streams": [
            {"index": 1, "codec_type": "audio", "codec_name": "ac3", "channels": 2},
            {"index": 2, "codec_type": "audio", "codec_name": "ac3", "channels": 2},
        ]});
        assert!(
            Ffprobe::with_program(&script)
                .duplicate_audio_indices(Path::new("x.mp4"), &probe)
                .is_empty()
        );
        assert!(
            Ffprobe::with_program("/definitely/not/ffprobe")
                .duplicate_audio_indices(Path::new("x"), &probe)
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn identical_tracks_read_from_a_real_process_are_reported() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("ffprobe");
        std::fs::write(&script, "#!/bin/sh\nprintf '1,a\\n2,a\\n1,b\\n2,b\\n'\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let probe = serde_json::json!({"streams": [
            {"index": 1, "codec_type": "audio", "codec_name": "ac3", "channels": 2},
            {"index": 2, "codec_type": "audio", "codec_name": "ac3", "channels": 2},
        ]});
        let found =
            Ffprobe::with_program(&script).duplicate_audio_indices(Path::new("x.mp4"), &probe);
        assert_eq!(found, BTreeSet::from([2]));
    }
}
