//! Refuse any ffmpeg command that is not an exact, copy-only plan (port of `atvprep/copy_policy.py`).
//!
//! The manager only allows lossless fixes. This is an allowlist over the finished argv, parsed by
//! the arity of every option the planner emits, so a spelling or option that is not listed is
//! refused rather than guessed at. Rules:
//!
//! * inputs come first (`-i`); the first is the source, the others are sidecar subtitles;
//! * exactly one video map `0:v:0`; audio maps `0:a:K`; subtitle maps `N:M` for a declared input;
//! * every mapped output stream gets its own explicit codec option for its own index: `copy` for
//!   video and audio, `mov_text` for subtitles, so ffmpeg never picks an encoder;
//! * no filters, rates, formats, attachments, response files or extra outputs.
//!
//! Whether a subtitle map really points at a subtitle stream cannot be seen in the argv; the
//! caller checks that against ffprobe. Messages name the kind of violation, never the command line
//! or a path.

use std::collections::HashSet;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

/// The command would encode video or audio, or is not a plan this policy knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyOnlyViolation(pub String);

impl fmt::Display for CopyOnlyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CopyOnlyViolation {}

const BSF: &str = "hevc_mp4toannexb,extract_extradata";
const DISPOSITIONS: [&str; 4] = ["default", "0", "forced+default", "hearing_impaired"];
const FLAGS: [&str; 3] = ["-hide_banner", "-y", "-nostats"];

static CODEC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^-c:([vas])(?::([0-9]+))?$").expect("valid"));
static METADATA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^-metadata:s:([vas]):([0-9]+)$").expect("valid"));
static DISPOSITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^-disposition:([as]):([0-9]+)$").expect("valid"));
static MAP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9]+):(?:(v):0|(a):([0-9]+)|([0-9]+))$").expect("valid"));
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9]{4}$").expect("valid"));
static PROGRESS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/dev/fd/[0-9]+$").expect("valid"));

type Check = Result<(), CopyOnlyViolation>;
type Stream = (char, usize);

fn fail(why: &str) -> Check {
    Err(CopyOnlyViolation(why.to_owned()))
}

/// A stream number; one too large to be real never matches a mapped stream.
fn number(digits: &str) -> usize {
    digits.parse().unwrap_or(usize::MAX)
}

#[derive(Default)]
struct State {
    inputs: usize,
    outputs: Vec<Stream>,
    video: usize,
    audio: usize,
    subs: usize,
    coded: HashSet<Stream>,
    /// An `N:M` map still waiting for its `-c:s` option.
    pending: Option<Stream>,
    video_all: bool,
    first_map: bool,
}

impl State {
    fn map(&mut self, value: &str) -> Check {
        if self.pending.is_some() {
            return fail("a mapped stream does not name its codec");
        }
        let Some(m) = MAP.captures(value) else {
            return fail("a map form outside the allowlist");
        };
        if number(&m[1]) >= self.inputs {
            return fail("a map form outside the allowlist");
        }
        self.first_map = true;
        if m.get(2).is_some() {
            if &m[1] != "0" || self.video > 0 {
                return fail("only the first video stream of the source may be mapped");
            }
            self.outputs.push(('v', self.video));
            self.video += 1;
        } else if m.get(3).is_some() {
            if &m[1] != "0" {
                return fail("audio comes from the source only");
            }
            self.outputs.push(('a', self.audio));
            self.audio += 1;
        } else {
            let stream = ('s', self.subs);
            self.subs += 1;
            self.pending = Some(stream);
            self.outputs.push(stream);
        }
        Ok(())
    }

    fn codec(&mut self, kind: char, index: Option<usize>, value: &str) -> Check {
        if kind == 's' {
            let ok = self
                .pending
                .is_some_and(|p| index.is_some_and(|n| (kind, n) == p))
                && value == "mov_text";
            let Some(pending) = self.pending.filter(|_| ok) else {
                return fail("a subtitle codec other than mov_text, or for the wrong stream");
            };
            self.coded.insert(pending);
            self.pending = None;
            return Ok(());
        }
        if value != "copy" {
            return fail("a video or audio stream would be encoded");
        }
        match index {
            None if kind == 'v' => {
                self.video_all = true;
                self.coded.insert(('v', 0));
                Ok(())
            }
            Some(n) if self.outputs.contains(&(kind, n)) && !self.coded.contains(&(kind, n)) => {
                self.coded.insert((kind, n));
                Ok(())
            }
            _ => fail("a codec option for a stream that is not mapped"),
        }
    }

    fn option(&mut self, token: &str, value: &str) -> Check {
        match token {
            "-i" => {
                if self.first_map || value.is_empty() || value.starts_with('-') {
                    return fail("inputs must come first");
                }
                self.inputs += 1;
            }
            "-fflags" if value != "+genpts" => {
                return fail("an fflags value outside the allowlist");
            }
            "-map" => return self.map(value),
            "-tag:v" if !TAG.is_match(value) => return fail("a tag outside the allowlist"),
            "-bsf:v" if value != BSF => return fail("a bitstream filter outside the allowlist"),
            "-map_chapters" if value != "-1" => return fail("chapters may only be dropped"),
            "-avoid_negative_ts" if value != "make_zero" => {
                return fail("a timestamp mode outside the allowlist");
            }
            "-strict" if value != "unofficial" => {
                return fail("a strictness outside the allowlist");
            }
            "-progress" if !PROGRESS.is_match(value) => {
                return fail("progress must go to a descriptor");
            }
            "-fflags" | "-tag:v" | "-bsf:v" | "-map_chapters" | "-avoid_negative_ts"
            | "-strict" | "-progress" => {}
            _ => return self.pattern_option(token, value),
        }
        Ok(())
    }

    fn pattern_option(&mut self, token: &str, value: &str) -> Check {
        if let Some(c) = CODEC.captures(token) {
            let kind = c[1].chars().next().unwrap_or('?');
            return self.codec(kind, c.get(2).map(|n| number(n.as_str())), value);
        }
        if let Some(c) = METADATA.captures(token) {
            let stream = (c[1].chars().next().unwrap_or('?'), number(&c[2]));
            return if self.outputs.contains(&stream) && value.contains('=') {
                Ok(())
            } else {
                fail("metadata for a stream that is not mapped")
            };
        }
        if let Some(c) = DISPOSITION.captures(token) {
            let stream = (c[1].chars().next().unwrap_or('?'), number(&c[2]));
            return if self.outputs.contains(&stream) && DISPOSITIONS.contains(&value) {
                Ok(())
            } else {
                fail("a disposition outside the allowlist")
            };
        }
        fail("an option outside the allowlist")
    }
}

/// Refuse unless `cmd` is a copy-only plan from the allowlist.
///
/// # Errors
///
/// Returns a [`CopyOnlyViolation`] naming the kind of violation.
pub fn check_copy_only(cmd: &[String]) -> Check {
    let (Some(last_token), true) = (cmd.last(), cmd.len() >= 4) else {
        return fail("not a complete ffmpeg command");
    };
    if cmd[0] != "ffmpeg" || last_token.starts_with('-') || last_token.starts_with('@') {
        return fail("not a complete ffmpeg command");
    }
    let last = cmd.len() - 1; // the output path
    let mut state = State::default();
    let mut index = 1;
    while index < last {
        let token = cmd[index].as_str();
        if FLAGS.contains(&token) {
            index += 1;
            continue;
        }
        if index + 1 >= last {
            return fail("an option has no value");
        }
        state.option(token, &cmd[index + 1])?;
        index += 2;
    }
    if state.pending.is_some() {
        return fail("a mapped stream does not name its codec");
    }
    if state.inputs < 1
        || state.video != 1
        || (!state.video_all && !state.coded.contains(&('v', 0)))
    {
        return fail("the plan does not copy exactly one video stream");
    }
    if state.outputs.iter().any(|s| !state.coded.contains(s)) {
        return fail("a mapped stream does not say that it is copied");
    }
    Ok(())
}

/// Refuse unless `cmd` copies the video and does not filter it. Audio may be anything here.
///
/// The Kodierfreigabe „Audio kodieren“ is enforced on the plan first; this reads the finished argv
/// as a second, independent look, so an unknown way of asking for video work counts as a violation.
///
/// # Errors
///
/// Returns a [`CopyOnlyViolation`] naming the kind of violation.
pub fn check_video_copied(cmd: &[String]) -> Check {
    let Some((_output, arguments)) = cmd.split_last() else {
        return fail("not a complete ffmpeg command");
    };
    let mut copied = false;
    let mut tokens = arguments.iter().skip(1).map(String::as_str);
    while let Some(token) = tokens.next() {
        match token {
            "-i" => {
                tokens.next();
            }
            "-c:v" | "-c:v:0" => {
                if tokens.next() != Some("copy") {
                    return fail("the video stream would be encoded");
                }
                copied = true;
            }
            _ if token.starts_with("-c:v")
                || token.starts_with("-codec:v")
                || token.starts_with("-vcodec")
                || token.starts_with("-vf")
                || token.starts_with("-filter:v")
                || token.starts_with("-filter_complex")
                || token.starts_with("-lavfi")
                || matches!(token, "-c" | "-codec") =>
            {
                return fail("a video stream would be encoded or filtered");
            }
            _ => {}
        }
    }
    if copied {
        Ok(())
    } else {
        fail("the plan does not copy the video stream")
    }
}

#[cfg(test)]
mod tests {
    use atvprep_testkit::load_golden;

    use super::*;

    fn cmd(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_owned()).collect()
    }

    fn argv(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn video_check_accepts_a_copied_video_with_encoded_audio() {
        let plan = argv("ffmpeg -i in.mkv -map 0:v:0 -c:v copy -map 0:a:0 -c:a:0 aac out.mp4");
        assert_eq!(check_video_copied(&plan), Ok(()));
    }

    #[test]
    fn video_check_refuses_every_way_of_touching_the_video() {
        for line in [
            "ffmpeg -i in.mkv -map 0:v:0 -c:v libx265 out.mp4",
            "ffmpeg -i in.mkv -map 0:v:0 -c:v:0 libx265 out.mp4",
            "ffmpeg -i in.mkv -map 0:v:0 -c:v copy -vf scale=1:1 out.mp4",
            "ffmpeg -i in.mkv -map 0:v:0 -c:v copy -filter:v:0 scale=1:1 out.mp4",
            "ffmpeg -i in.mkv -map 0:v:0 -vcodec libx265 -c:v copy out.mp4",
            "ffmpeg -i in.mkv -map 0:v:0 -c copy out.mp4",
            "ffmpeg -i in.mkv -map 0:v:0 -c:a:0 aac out.mp4",
            "ffmpeg -filter_complex x -i in.mkv -c:v copy out.mp4",
        ] {
            assert!(check_video_copied(&argv(line)).is_err(), "{line}");
        }
        assert!(check_video_copied(&[]).is_err());
    }

    #[test]
    fn video_check_names_only_the_video() {
        let plan = argv("ffmpeg -i in.mkv -map 0:v:0 -c:v libx265 -c:a:0 aac out.mp4");
        assert_eq!(
            check_video_copied(&plan),
            Err(CopyOnlyViolation(
                "the video stream would be encoded".to_owned()
            ))
        );
    }

    #[test]
    fn verdicts_match_the_python_oracle() {
        let cases = load_golden::<Vec<String>, Option<String>>("copy_policy");
        assert!(cases.len() > 50);
        for case in cases {
            let actual = check_copy_only(&case.input).err().map(|e| e.0);
            assert_eq!(actual, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn a_minimal_copy_plan_passes() {
        let plan = cmd(&[
            "ffmpeg", "-i", "in.mkv", "-map", "0:v:0", "-c:v", "copy", "out.mp4",
        ]);
        assert_eq!(check_copy_only(&plan), Ok(()));
    }

    #[test]
    fn the_message_names_the_kind_not_the_command() {
        let plan = cmd(&[
            "ffmpeg",
            "-i",
            "in.mkv",
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-vf",
            "scale=/secret/path",
            "out.mp4",
        ]);
        let error = check_copy_only(&plan).unwrap_err();
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn incomplete_commands_are_refused() {
        for bad in [
            cmd(&["ffmpeg"]),
            cmd(&["ffmpeg", "-i", "in.mkv"]),
            cmd(&["ffmpeg", "-map", "0:v:0", "-c:v", "copy", "out.mp4"]),
        ] {
            assert!(check_copy_only(&bad).is_err(), "{bad:?}");
        }
    }
}
