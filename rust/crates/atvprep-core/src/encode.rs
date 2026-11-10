//! Kodierfreigabe and Kodierbedarf: how far a run may encode, and how far a plan would (ADR 0008).

use crate::escalation::Level;
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

    /// The need stored under `text`; `None` for „unbekannt“.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "none" => Some(Self::None),
            "audio" => Some(Self::Audio),
            "video" => Some(Self::Video),
            _ => None,
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

/// The spelling of a stored need; `None` is „unbekannt“.
#[must_use]
pub const fn need_name(need: Option<EncodeNeed>) -> &'static str {
    match need {
        Some(need) => need.as_str(),
        None => UNKNOWN,
    }
}

/// „unbekannt“, for a stored need and a stored change alike.
pub const UNKNOWN: &str = "unknown";

/// The Änderungsart: how Prepare would change this file, as the operator sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    None,
    Patch,
    Remux,
    /// A new audio track or an audio transcode; the video stays a copy.
    Audio,
    Reencode,
}

impl ChangeKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Patch => "patch",
            Self::Remux => "remux",
            Self::Audio => "audio",
            Self::Reencode => "reencode",
        }
    }

    /// The change of a run at `level` whose plan needs `need`; encoding outranks the route.
    #[must_use]
    pub const fn of(level: Level, need: EncodeNeed) -> Self {
        match (need, level) {
            (EncodeNeed::Video, _) => Self::Reencode,
            (EncodeNeed::Audio, _) => Self::Audio,
            (EncodeNeed::None, Level::None) => Self::None,
            (EncodeNeed::None, Level::InPlace) => Self::Patch,
            (EncodeNeed::None, Level::Rebuild | Level::Reencode) => Self::Remux,
        }
    }
}

/// What `prepare` would do to one file, as the planner sees it right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedChange {
    pub need: EncodeNeed,
    pub change: ChangeKind,
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
    fn encoding_outranks_the_route_and_a_copy_keeps_it() {
        let table = [
            (Level::None, EncodeNeed::None, "none"),
            (Level::InPlace, EncodeNeed::None, "patch"),
            (Level::Rebuild, EncodeNeed::None, "remux"),
            (Level::Rebuild, EncodeNeed::Audio, "audio"),
            (Level::Reencode, EncodeNeed::Video, "reencode"),
            (Level::Rebuild, EncodeNeed::Video, "reencode"),
        ];
        for (level, need, want) in table {
            assert_eq!(
                ChangeKind::of(level, need).as_str(),
                want,
                "{level:?} {need:?}"
            );
        }
    }

    #[test]
    fn a_stored_need_round_trips_and_anything_else_is_unknown() {
        for need in [EncodeNeed::None, EncodeNeed::Audio, EncodeNeed::Video] {
            assert_eq!(EncodeNeed::parse(need_name(Some(need))), Some(need));
        }
        assert_eq!(need_name(None), "unknown");
        assert_eq!(EncodeNeed::parse("unknown"), None);
    }

    #[test]
    fn every_spelling_round_trips_and_nothing_else_parses() {
        for name in EncodeAllowance::NAMES {
            assert_eq!(EncodeAllowance::parse(name).unwrap().as_str(), name);
        }
        assert_eq!(EncodeAllowance::parse("video"), None);
    }
}
