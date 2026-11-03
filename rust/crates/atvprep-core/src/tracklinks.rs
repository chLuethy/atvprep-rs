//! Which Apple track references (`tref`) a file should carry (port of `atvprep/tracklinks.py`).
//!
//! * `fall`: a surround audio track falls back to the first stereo/mono track of its language,
//!   so `AVFoundation` lists the pair as a single audio option.
//! * `folw`: an audio track follows the forced subtitle track of its language.
//! * `forc`: a full subtitle track points at the forced-only track of its language.
//!
//! Tracks are addressed by their position within their own kind (audio #0, #1, …; subtitle #0, …).

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AudioRef {
    /// ISO 639-2; `None` when undetermined.
    pub lang: Option<String>,
    /// 0 when unknown.
    pub channels: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SubRef {
    pub lang: Option<String>,
    pub forced: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackLinks {
    /// audio -> audio
    pub fall: BTreeMap<usize, usize>,
    /// audio -> subtitle
    pub folw: BTreeMap<usize, usize>,
    /// subtitle -> subtitle
    pub forc: BTreeMap<usize, usize>,
}

impl TrackLinks {
    /// Table cells for an audio track, e.g. `["fall → A1", "folw → S0"]`.
    #[must_use]
    pub fn audio_labels(&self, index: usize) -> Vec<String> {
        let mut labels = Vec::new();
        if let Some(target) = self.fall.get(&index) {
            labels.push(format!("fall → A{target}"));
        }
        if let Some(target) = self.folw.get(&index) {
            labels.push(format!("folw → S{target}"));
        }
        labels
    }

    #[must_use]
    pub fn sub_labels(&self, index: usize) -> Vec<String> {
        self.forc
            .get(&index)
            .map(|target| format!("forc → S{target}"))
            .into_iter()
            .collect()
    }

    #[must_use]
    pub fn count(&self) -> BTreeMap<&'static str, usize> {
        BTreeMap::from([
            ("fall", self.fall.len()),
            ("folw", self.folw.len()),
            ("forc", self.forc.len()),
        ])
    }
}

/// The one policy behind both the binary patch and the CLI tables.
#[must_use]
pub fn plan_links(audio: &[AudioRef], subs: &[SubRef]) -> TrackLinks {
    let mut fall = BTreeMap::new();
    for (i, track) in audio.iter().enumerate() {
        if track.lang.is_none() || track.channels <= 2 {
            continue;
        }
        let sibling = audio
            .iter()
            .position(|other| other.lang == track.lang && (1..=2).contains(&other.channels));
        if let Some(j) = sibling {
            fall.insert(i, j);
        }
    }

    let mut forced: BTreeMap<&str, usize> = BTreeMap::new();
    for (j, sub) in subs.iter().enumerate() {
        if let (true, Some(lang)) = (sub.forced, sub.lang.as_deref()) {
            forced.entry(lang).or_insert(j);
        }
    }

    let target = |lang: &Option<String>| lang.as_deref().and_then(|l| forced.get(l).copied());
    let folw = audio
        .iter()
        .enumerate()
        .filter_map(|(i, t)| target(&t.lang).map(|f| (i, f)))
        .collect();
    let forc = subs
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.forced)
        .filter_map(|(j, s)| target(&s.lang).map(|f| (j, f)))
        .collect();
    TrackLinks { fall, folw, forc }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use atvprep_testkit::load_golden;
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct Input {
        audio: Vec<AudioRef>,
        subs: Vec<SubRef>,
    }

    #[derive(Deserialize)]
    struct Expected {
        fall: BTreeMap<usize, usize>,
        folw: BTreeMap<usize, usize>,
        forc: BTreeMap<usize, usize>,
        audio_labels: Vec<Vec<String>>,
        sub_labels: Vec<Vec<String>>,
        count: BTreeMap<String, usize>,
    }

    #[test]
    fn plan_links_matches_the_python_oracle() {
        for case in load_golden::<Input, Expected>("tracklinks") {
            let links = plan_links(&case.input.audio, &case.input.subs);
            let name = &case.name;
            assert_eq!(links.fall, case.expected.fall, "{name}");
            assert_eq!(links.folw, case.expected.folw, "{name}");
            assert_eq!(links.forc, case.expected.forc, "{name}");
            for (i, labels) in case.expected.audio_labels.iter().enumerate() {
                assert_eq!(&links.audio_labels(i), labels, "{name} audio {i}");
            }
            for (j, labels) in case.expected.sub_labels.iter().enumerate() {
                assert_eq!(&links.sub_labels(j), labels, "{name} sub {j}");
            }
            let count: BTreeMap<String, usize> = links
                .count()
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect();
            assert_eq!(count, case.expected.count, "{name}");
        }
    }

    fn a(lang: Option<&str>, channels: u32) -> AudioRef {
        AudioRef {
            lang: lang.map(str::to_owned),
            channels,
        }
    }

    fn s(lang: Option<&str>, forced: bool) -> SubRef {
        SubRef {
            lang: lang.map(str::to_owned),
            forced,
        }
    }

    #[test]
    fn surround_falls_back_to_the_first_stereo_track_of_its_language() {
        let audio = [
            a(Some("deu"), 6),
            a(Some("deu"), 2),
            a(Some("eng"), 6),
            a(Some("eng"), 2),
            a(Some("eng"), 1),
        ];
        assert_eq!(
            plan_links(&audio, &[]).fall,
            BTreeMap::from([(0, 1), (2, 3)])
        );
    }

    #[test]
    fn surround_without_a_stereo_sibling_or_a_language_gets_no_fall() {
        let audio = [a(Some("deu"), 6), a(Some("eng"), 2), a(None, 6), a(None, 2)];
        assert!(plan_links(&audio, &[]).fall.is_empty());
    }

    #[test]
    fn unknown_channel_count_is_neither_surround_nor_a_fallback_target() {
        assert!(
            plan_links(&[a(Some("deu"), 6), a(Some("deu"), 0)], &[])
                .fall
                .is_empty()
        );
    }

    #[test]
    fn audio_follows_the_forced_track_and_full_tracks_point_at_it() {
        let links = plan_links(
            &[a(Some("deu"), 6), a(Some("deu"), 2), a(Some("eng"), 2)],
            &[
                s(Some("deu"), true),
                s(Some("deu"), false),
                s(Some("eng"), false),
            ],
        );
        assert_eq!(links.folw, BTreeMap::from([(0, 0), (1, 0)]));
        assert_eq!(links.forc, BTreeMap::from([(1, 0)]));
    }

    #[test]
    fn first_forced_track_of_a_language_wins() {
        let links = plan_links(
            &[a(Some("deu"), 2)],
            &[
                s(Some("deu"), true),
                s(Some("deu"), true),
                s(Some("deu"), false),
            ],
        );
        assert_eq!(links.folw, BTreeMap::from([(0, 0)]));
        assert_eq!(links.forc, BTreeMap::from([(2, 0)]));
    }

    #[test]
    fn forced_only_language_links_audio_but_not_the_forced_track_itself() {
        let links = plan_links(&[a(Some("eng"), 2)], &[s(Some("eng"), true)]);
        assert_eq!(links.folw, BTreeMap::from([(0, 0)]));
        assert!(links.forc.is_empty());
    }

    #[test]
    fn no_links_without_a_forced_track() {
        let links = plan_links(&[a(Some("nld"), 2)], &[s(Some("nld"), false)]);
        assert!(links.fall.is_empty() && links.folw.is_empty() && links.forc.is_empty());
    }

    #[test]
    fn describe_lists_what_each_track_links_to() {
        let links = plan_links(
            &[a(Some("deu"), 6), a(Some("deu"), 2)],
            &[s(Some("deu"), true), s(Some("deu"), false)],
        );
        assert_eq!(links.audio_labels(0), ["fall → A1", "folw → S0"]);
        assert_eq!(links.audio_labels(1), ["folw → S0"]);
        assert_eq!(links.sub_labels(0), Vec::<String>::new());
        assert_eq!(links.sub_labels(1), ["forc → S0"]);
        assert_eq!(
            links.count(),
            BTreeMap::from([("fall", 1), ("folw", 2), ("forc", 1)])
        );
    }
}
