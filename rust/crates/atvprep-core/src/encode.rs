//! Kodierfreigabe and Kodierbedarf: how far a run may encode, and how far a plan would (ADR 0008).

use crate::models::{AudioAction, Plan, VideoAction};

/// The Kodierfreigabe: the upper limit of what one run may encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum EncodeAllowance {
    /// Streams are only copied.
    Copy,
    /// Audio may be encoded; the video stays a copy.
    #[default]
    Audio,
    /// Audio and video may be encoded.
    All,
}

impl EncodeAllowance {
    /// The spellings of `--encode`, in rising order.
    pub const NAMES: [&'static str; 3] = ["copy", "audio", "all"];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Audio => "audio",
            Self::All => "all",
        }
    }

    /// The level named `text`, as spelled by `--encode`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "copy" => Some(Self::Copy),
            "audio" => Some(Self::Audio),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// Whether a plan with this need may run.
    #[must_use]
    pub fn permits(self, need: EncodeNeed) -> bool {
        self >= need.required()
    }
}

/// The Kodierbedarf of one plan. Video includes whatever the audio needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeNeed {
    None,
    Audio,
    Video,
}

impl EncodeNeed {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }

    /// What the planner is about to do: a video transcode is Video, a downmix or an audio
    /// transcode is Audio, anything else only copies.
    #[must_use]
    pub fn of_plan(plan: &Plan) -> Self {
        let video = plan.video_info.as_ref().map(|v| v.action);
        if video == Some(VideoAction::Transcode) {
            Self::Video
        } else if plan
            .audio_tracks
            .iter()
            .any(|t| t.action != AudioAction::Copy)
        {
            Self::Audio
        } else {
            Self::None
        }
    }

    /// The lowest allowance this need runs under.
    #[must_use]
    pub const fn required(self) -> EncodeAllowance {
        match self {
            Self::None => EncodeAllowance::Copy,
            Self::Audio => EncodeAllowance::Audio,
            Self::Video => EncodeAllowance::All,
        }
    }
}

/// „Kodierung nicht freigegeben“: the plan needs more than the run was allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeRefusal {
    pub need: EncodeNeed,
    pub allowance: EncodeAllowance,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_allowance_permits_exactly_the_needs_up_to_it() {
        use EncodeAllowance::{All, Audio, Copy};
        let table = [
            (Copy, [true, false, false]),
            (Audio, [true, true, false]),
            (All, [true, true, true]),
        ];
        for (allowance, want) in table {
            let got = [EncodeNeed::None, EncodeNeed::Audio, EncodeNeed::Video]
                .map(|need| allowance.permits(need));
            assert_eq!(got, want, "{allowance:?}");
        }
    }

    #[test]
    fn every_spelling_round_trips_and_nothing_else_parses() {
        for name in EncodeAllowance::NAMES {
            assert_eq!(EncodeAllowance::parse(name).unwrap().as_str(), name);
        }
        assert_eq!(EncodeAllowance::parse("video"), None);
    }
}
