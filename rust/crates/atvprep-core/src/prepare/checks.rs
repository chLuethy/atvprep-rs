//! Checks `--copy-only` runs make against ffprobe: the argv alone cannot show them.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::copy_policy::CopyOnlyViolation;
use crate::ffprobe::{Ffprobe, ProbeError};
use crate::models::{Plan, SubAction};

/// The ffprobe JSON of one file; a seam for tests.
pub trait ProbeJson {
    /// # Errors
    ///
    /// The tool failed or printed something that is not JSON.
    fn probe_json(&self, path: &Path) -> Result<Value, ProbeError>;
}

impl ProbeJson for Ffprobe {
    fn probe_json(&self, path: &Path) -> Result<Value, ProbeError> {
        Self::probe_json(self, path)
    }
}

fn streams(probe: &Value) -> &[Value] {
    probe
        .get("streams")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn kind(stream: &Value) -> Option<&str> {
    stream.get("codec_type").and_then(Value::as_str)
}

fn codec(stream: &Value) -> Option<&str> {
    stream.get("codec_name").and_then(Value::as_str)
}

fn violation(message: &str) -> CopyOnlyViolation {
    CopyOnlyViolation(message.to_owned())
}

/// A regular file (no symlink) whose every stream ffprobe reads is a subtitle stream.
#[must_use]
pub fn is_subtitle_file(path: &Path, ffprobe: &dyn ProbeJson) -> bool {
    let plain = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file());
    if !plain {
        return false;
    }
    let Ok(probe) = ffprobe.probe_json(path) else {
        return false;
    };
    let all = streams(&probe);
    !all.is_empty() && all.iter().all(|s| kind(s) == Some("subtitle"))
}

/// The merged sidecars that really are subtitle files: a name ending in `.srt` proves nothing.
#[must_use]
pub fn subtitle_sidecars(
    source: &Path,
    extra: &[PathBuf],
    ffprobe: &dyn ProbeJson,
) -> Vec<PathBuf> {
    super::paths::sidecars(source, extra)
        .into_iter()
        .filter(|p| is_subtitle_file(p, ffprobe))
        .collect()
}

/// Every subtitle map must point at a subtitle stream.
///
/// # Errors
///
/// A map points at something else, or at a sidecar that is not there.
pub fn verify_subtitle_inputs(
    plan: &Plan,
    probe: &Value,
    sidecars: &[PathBuf],
    ffprobe: &dyn ProbeJson,
) -> Result<(), CopyOnlyViolation> {
    for track in plan
        .sub_tracks
        .iter()
        .filter(|t| t.action != SubAction::Skip)
    {
        let ok = if track.input_idx == 0 {
            streams(probe).iter().any(|s| {
                s.get("index").and_then(Value::as_u64) == Some(u64::from(track.stream_idx))
                    && kind(s) == Some("subtitle")
            })
        } else {
            usize::try_from(track.input_idx)
                .ok()
                .and_then(|i| sidecars.get(i - 1))
                .is_some_and(|p| is_subtitle_file(p, ffprobe))
        };
        if !ok {
            return Err(violation(
                "a subtitle map does not point at a subtitle stream",
            ));
        }
    }
    Ok(())
}

/// The finished file has the planned streams and the source's own video and audio codecs.
///
/// # Errors
///
/// The output cannot be read back, or differs from the plan.
pub fn verify_output_streams(
    tmp: &Path,
    plan: &Plan,
    probe: &Value,
    ffprobe: &dyn ProbeJson,
) -> Result<(), CopyOnlyViolation> {
    let out = ffprobe
        .probe_json(tmp)
        .map_err(|_| violation("the output could not be read back"))?;
    let (mut video, mut audio, mut subs) = (Vec::new(), Vec::new(), Vec::new());
    for stream in streams(&out) {
        match kind(stream) {
            Some("video") => video.push(stream),
            Some("audio") => audio.push(stream),
            Some("subtitle") => subs.push(stream),
            _ => return Err(violation("the output holds a stream that was not planned")),
        }
    }
    let planned_subs = plan
        .sub_tracks
        .iter()
        .filter(|t| t.action != SubAction::Skip)
        .count();
    if video.len() != 1 || audio.len() != plan.audio_tracks.len() || subs.len() != planned_subs {
        return Err(violation(
            "the output does not hold exactly the planned streams",
        ));
    }
    let source_video = streams(probe).iter().find(|s| kind(s) == Some("video"));
    if source_video.is_none_or(|v| codec(video[0]) != codec(v)) {
        return Err(violation("the output video is not the source video"));
    }
    for (track, stream) in plan.audio_tracks.iter().zip(&audio) {
        let original = streams(probe)
            .iter()
            .find(|s| s.get("index").and_then(Value::as_u64) == Some(u64::from(track.index)));
        let copied = track.action == crate::models::AudioAction::Copy
            && original.is_some_and(|o| codec(stream) == codec(o));
        if !copied {
            return Err(violation("an output audio stream is not the source audio"));
        }
    }
    if subs.iter().any(|s| codec(s) != Some("mov_text")) {
        return Err(violation("an output subtitle stream is not mov_text"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::models::{AudioAction, AudioTrack, SubSource, SubTrack};

    struct Fake(HashMap<PathBuf, Value>);

    impl ProbeJson for Fake {
        fn probe_json(&self, path: &Path) -> Result<Value, ProbeError> {
            self.0.get(path).cloned().ok_or(ProbeError::Failed(Some(1)))
        }
    }

    fn plan(audio: Vec<AudioTrack>, subs: Vec<SubTrack>) -> Plan {
        Plan {
            video_info: None,
            audio_tracks: audio,
            sub_tracks: subs,
            ffmpeg_cmd: Vec::new(),
            warnings: Vec::new(),
            in_path: PathBuf::from("in.mkv"),
            out_path: PathBuf::from("out.mp4"),
            duration_s: None,
        }
    }

    fn audio(index: u32) -> AudioTrack {
        AudioTrack {
            index,
            codec: "eac3".to_owned(),
            channels: 6,
            lang: "eng".to_owned(),
            is_atmos: false,
            action: AudioAction::Copy,
            is_transcoded: false,
            is_default: true,
        }
    }

    fn sub(input_idx: u32, stream_idx: u32, action: SubAction) -> SubTrack {
        SubTrack {
            input_idx,
            stream_idx,
            codec: "subrip".to_owned(),
            lang: "eng".to_owned(),
            forced: false,
            sdh: false,
            source: SubSource::Internal,
            action,
            is_default: false,
        }
    }

    fn source() -> Value {
        json!({"streams": [
            {"index": 0, "codec_type": "video", "codec_name": "hevc"},
            {"index": 1, "codec_type": "audio", "codec_name": "eac3"},
            {"index": 2, "codec_type": "subtitle", "codec_name": "subrip"},
        ]})
    }

    fn output(codecs: &[(&str, &str)]) -> Fake {
        let streams: Vec<Value> = codecs
            .iter()
            .map(|(k, c)| json!({"codec_type": k, "codec_name": c}))
            .collect();
        Fake(HashMap::from([(
            PathBuf::from("tmp.mp4"),
            json!({"streams": streams}),
        )]))
    }

    fn message(r: Result<(), CopyOnlyViolation>) -> String {
        r.expect_err("a violation").0
    }

    #[test]
    fn a_subtitle_file_is_a_plain_file_of_only_subtitle_streams() {
        let dir = tempfile::tempdir().unwrap();
        let mk = |n: &str| {
            let p = dir.path().join(n);
            std::fs::write(&p, "x").unwrap();
            p
        };
        let (srt, video, empty, link) = (
            mk("a.srt"),
            mk("v.srt"),
            mk("e.srt"),
            dir.path().join("l.srt"),
        );
        std::os::unix::fs::symlink(&srt, &link).unwrap();
        let sub = json!({"streams": [{"codec_type": "subtitle"}]});
        let mixed = json!({"streams": [{"codec_type": "subtitle"}, {"codec_type": "video"}]});
        let fake = Fake(HashMap::from([
            (srt.clone(), sub.clone()),
            (video.clone(), mixed),
            (empty.clone(), json!({"streams": []})),
            (link.clone(), sub),
        ]));
        assert!(is_subtitle_file(&srt, &fake));
        assert!(!is_subtitle_file(&video, &fake));
        assert!(!is_subtitle_file(&empty, &fake));
        assert!(!is_subtitle_file(&link, &fake));
        assert!(!is_subtitle_file(dir.path(), &fake));
        assert!(!is_subtitle_file(&dir.path().join("missing.srt"), &fake));
    }

    #[test]
    fn subtitle_maps_must_point_at_subtitle_streams() {
        let fake = Fake(HashMap::new());
        let ok = plan(
            vec![],
            vec![sub(0, 2, SubAction::MovText), sub(0, 9, SubAction::Skip)],
        );
        assert!(verify_subtitle_inputs(&ok, &source(), &[], &fake).is_ok());
        let wrong = plan(vec![], vec![sub(0, 1, SubAction::MovText)]);
        assert_eq!(
            message(verify_subtitle_inputs(&wrong, &source(), &[], &fake)),
            "a subtitle map does not point at a subtitle stream"
        );
        let missing = plan(vec![], vec![sub(1, 0, SubAction::MovText)]);
        assert!(verify_subtitle_inputs(&missing, &source(), &[], &fake).is_err());
    }

    #[test]
    fn the_output_must_hold_the_planned_streams_with_the_source_codecs() {
        let p = plan(vec![audio(1)], vec![sub(0, 2, SubAction::MovText)]);
        let good = output(&[
            ("video", "hevc"),
            ("audio", "eac3"),
            ("subtitle", "mov_text"),
        ]);
        assert!(verify_output_streams(Path::new("tmp.mp4"), &p, &source(), &good).is_ok());
        let cases = [
            (
                vec![("video", "hevc"), ("audio", "eac3")],
                "the output does not hold exactly the planned streams",
            ),
            (
                vec![("video", "hevc"), ("audio", "eac3"), ("data", "x")],
                "the output holds a stream that was not planned",
            ),
            (
                vec![
                    ("video", "h264"),
                    ("audio", "eac3"),
                    ("subtitle", "mov_text"),
                ],
                "the output video is not the source video",
            ),
            (
                vec![
                    ("video", "hevc"),
                    ("audio", "aac"),
                    ("subtitle", "mov_text"),
                ],
                "an output audio stream is not the source audio",
            ),
            (
                vec![("video", "hevc"), ("audio", "eac3"), ("subtitle", "subrip")],
                "an output subtitle stream is not mov_text",
            ),
        ];
        for (codecs, expected) in cases {
            let fake = output(&codecs);
            let got = message(verify_output_streams(
                Path::new("tmp.mp4"),
                &p,
                &source(),
                &fake,
            ));
            assert_eq!(got, expected);
        }
        let unreadable = Fake(HashMap::new());
        assert_eq!(
            message(verify_output_streams(
                Path::new("tmp.mp4"),
                &p,
                &source(),
                &unreadable
            )),
            "the output could not be read back"
        );
    }
}
