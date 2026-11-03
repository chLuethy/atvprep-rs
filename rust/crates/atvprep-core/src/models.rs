//! The plan and container-spec data model (port of `atvprep/models.py`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What happens to the video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoAction {
    Copy,
    Transcode,
}

/// What happens to an audio track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioAction {
    Copy,
    Downmix,
    TranscodeEac3,
}

/// What happens to a subtitle track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAction {
    MovText,
    Skip,
}

/// Where a subtitle track comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubSource {
    Internal,
    External,
}

/// Kind of a track in the output container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Video,
    Audio,
    Subtitle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoInfo {
    pub codec: String,
    /// Raw profile string from ffprobe, e.g. `High 10`.
    pub profile: String,
    pub width: u32,
    pub height: u32,
    pub hdr: bool,
    pub dolby_vision: bool,
    pub pix_fmt: String,
    pub action: VideoAction,
    /// e.g. `hvc1` for HEVC.
    pub tag: Option<String>,
    /// e.g. `smpte2084`, `arib-std-b67`.
    pub color_transfer: String,
    /// e.g. `bt2020`.
    pub color_primaries: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTrack {
    /// Original stream index in the input file.
    pub index: u32,
    pub codec: String,
    pub channels: u32,
    /// ISO 639-2.
    pub lang: String,
    pub is_atmos: bool,
    pub action: AudioAction,
    /// True if the action is not a plain copy.
    pub is_transcoded: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubTrack {
    /// ffmpeg input index (0 = main file, 1+ = sidecar).
    pub input_idx: u32,
    /// Stream index within that input.
    pub stream_idx: u32,
    /// Source codec, e.g. `subrip`.
    pub codec: String,
    /// ISO 639-2.
    pub lang: String,
    pub forced: bool,
    pub sdh: bool,
    pub source: SubSource,
    pub action: SubAction,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub video_info: Option<VideoInfo>,
    pub audio_tracks: Vec<AudioTrack>,
    pub sub_tracks: Vec<SubTrack>,
    pub ffmpeg_cmd: Vec<String>,
    pub warnings: Vec<String>,
    pub in_path: PathBuf,
    pub out_path: PathBuf,
    /// Input duration in seconds (from the ffprobe format section).
    pub duration_s: Option<f64>,
}

/// Expected per-track properties in the output container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackSpec {
    pub media_type: MediaType,
    /// ISO 639-2; `und` for video.
    pub language: String,
    /// Maps to `tkhd` flags bit 0 (ENABLED).
    pub is_enabled: bool,
    /// `sbtl` for subtitles; `None` lets ffmpeg/the muxer default.
    pub handler_type: Option<String>,
}

/// Declarative expected output container: `ftyp` brands and per-track flags.
///
/// Derived from the analyzer output, not from observed state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerSpec {
    pub major_brand: String,
    /// Includes `dby1` iff any output audio track is Atmos.
    pub compatible_brands: Vec<String>,
    /// In output order; index 0 is track id 1.
    pub tracks: Vec<TrackSpec>,
}

impl ContainerSpec {
    /// Build the spec from the analyzer output.
    #[must_use]
    pub fn from_plan_parts(
        video: Option<&VideoInfo>,
        audio: &[AudioTrack],
        subs: &[SubTrack],
    ) -> Self {
        let mut brands: Vec<String> = ["isom", "iso2", "avc1", "mp41"].map(str::to_owned).into();
        if audio.iter().any(|a| a.is_atmos) {
            brands.push("dby1".to_owned());
        }

        let mut tracks = Vec::new();
        if video.is_some() {
            tracks.push(TrackSpec {
                media_type: MediaType::Video,
                language: "und".to_owned(),
                is_enabled: true,
                handler_type: None,
            });
        }
        // Only the first audio track is enabled (0.2.8 behavior).
        for (i, a) in audio.iter().enumerate() {
            tracks.push(TrackSpec {
                media_type: MediaType::Audio,
                language: a.lang.clone(),
                is_enabled: i == 0,
                handler_type: None,
            });
        }
        // Only the first non-skipped subtitle is enabled.
        for (i, s) in subs
            .iter()
            .filter(|s| s.action != SubAction::Skip)
            .enumerate()
        {
            tracks.push(TrackSpec {
                media_type: MediaType::Subtitle,
                language: s.lang.clone(),
                is_enabled: i == 0,
                handler_type: Some("sbtl".to_owned()),
            });
        }

        Self {
            major_brand: "isom".to_owned(),
            compatible_brands: brands,
            tracks,
        }
    }
}

#[cfg(test)]
mod tests {
    use atvprep_testkit::load_golden;
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct Layout {
        video: Option<VideoInfo>,
        audio: Vec<AudioTrack>,
        subs: Vec<SubTrack>,
    }

    fn spec(layout: &Layout) -> ContainerSpec {
        ContainerSpec::from_plan_parts(layout.video.as_ref(), &layout.audio, &layout.subs)
    }

    #[test]
    fn from_plan_parts_matches_the_python_oracle() {
        for case in load_golden::<Layout, ContainerSpec>("container_spec") {
            assert_eq!(spec(&case.input), case.expected, "{}", case.name);
        }
    }

    fn named(name: &str) -> Layout {
        load_golden::<Layout, ContainerSpec>("container_spec")
            .into_iter()
            .find(|c| c.name == name)
            .unwrap()
            .input
    }

    #[test]
    fn only_the_first_audio_track_is_enabled() {
        let tracks = spec(&named("first of many subs")).tracks;
        let audio: Vec<bool> = tracks
            .iter()
            .filter(|t| t.media_type == MediaType::Audio)
            .map(|t| t.is_enabled)
            .collect();
        assert_eq!(audio, [true, false, false]);
    }

    #[test]
    fn dby1_only_with_atmos() {
        assert!(
            spec(&named("atmos adds dby1"))
                .compatible_brands
                .contains(&"dby1".to_owned())
        );
        assert!(
            !spec(&named("no video"))
                .compatible_brands
                .contains(&"dby1".to_owned())
        );
    }

    #[test]
    fn skipped_subtitles_get_no_track() {
        assert!(
            spec(&named("only skipped subs"))
                .tracks
                .iter()
                .all(|t| t.media_type != MediaType::Subtitle)
        );
    }
}
