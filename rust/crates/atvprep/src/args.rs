//! The command line: the same commands, flags, defaults and abbreviations as the Python CLI.

use atvprep_core::encode::EncodeAllowance;
use clap::builder::PossibleValuesParser;
use clap::{Arg, ArgAction, Command, value_parser};

fn flag(name: &'static str, short: Option<char>, help: &'static str) -> Arg {
    let arg = Arg::new(name)
        .long(name)
        .action(ArgAction::SetTrue)
        .help(help);
    short.map_or(arg.clone(), |c| arg.short(c))
}

fn text(name: &'static str, meta: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).value_name(meta).help(help)
}

fn int128(name: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .hide(true)
        .value_parser(|s: &str| s.parse::<i128>().map_err(|e| e.to_string()))
}

fn crf() -> Arg {
    Arg::new("crf")
        .long("crf")
        .value_name("N")
        .default_value("18")
        .value_parser(value_parser!(u32))
        .help("CRF value for HEVC transcode (default: 18)")
}

const ENCODE: &str = "How far to encode: copy never encodes, audio allows a stereo downmix and an \
audio transcode, all also allows a video re-encode. A file that needs more is left unchanged and \
reported (default: audio)";
const HEVC10: &str = "Force HEVC 10-bit transcode even for SDR/1080p content";
const DTS: &str = "Normalize negative initial video DTS with -fflags +genpts / \
-avoid_negative_ts make_zero. Off by default; opt in only for sources where AVPlayer stumbles on \
negative-DTS edit lists.";
const QUARANTINE: &str = "Directory to move blocker-failing outputs into (with a .verify.json \
sidecar). When unset, blockers fail the file instead of quarantining.";

fn prepare() -> Command {
    Command::new("prepare")
        .infer_long_args(true)
        .visible_aliases(["fix", "convert"])
        .about("Bring video files to the Apple TV target, going no further than needed")
        .long_about(
            "Leave a file alone, patch it in place, rebuild it without a video encode, or \
re-encode it: whichever is the first to reach the Apple TV target. MP4/M4V/MOV files are replaced \
in place; other containers get a <stem>.mp4 next to them. 'fix' and 'convert' are aliases.",
        )
        .arg(
            Arg::new("inputs")
                .value_name("input")
                .num_args(1..)
                .required(true)
                .help("Video files or directories"),
        )
        .arg(
            text("output", "PATH", "Output directory or target .mp4 path (default: in place)")
                .short('o'),
        )
        .arg(flag("prefer-hevc10", None, HEVC10))
        .arg(crf())
        .arg(flag("keep-av1", None, "Copy AV1 video instead of transcoding (requires Apple TV 4K 3rd gen)"))
        .arg(flag("yes", Some('y'), "Skip confirmation prompt"))
        .arg(flag("dry-run", Some('n'), "Show what would change without writing anything"))
        .arg(flag("verbose", Some('v'), "Show ffmpeg command and post-processing steps"))
        .arg(flag("force", Some('f'), "Rebuild every file even if leaving it alone or patching it in place would do"))
        .arg(
            text("encode", "LEVEL", ENCODE)
                .default_value("audio")
                .value_parser(PossibleValuesParser::new(EncodeAllowance::NAMES)),
        )
        .arg(flag("fix-dts", None, DTS))
        .arg(text("quarantine", "DIR", QUARANTINE))
        .arg(flag("delete-original", None, "Delete another container's original once its new MP4 passed verification (an MP4 is never deleted)"))
        .arg(int128("manager-parent-fd"))
        .arg(Arg::new("manager-leaf").long("manager-leaf").hide(true))
        .arg(int128("manager-device"))
        .arg(int128("manager-inode"))
        .arg(int128("manager-ctime-ns"))
}

fn patch() -> Command {
    Command::new("patch")
        .infer_long_args(true)
        .about("Binary-patch MP4 metadata without ffmpeg")
        .arg(
            Arg::new("inputs")
                .value_name("input")
                .num_args(1..)
                .required(true)
                .help(".mp4 files or directories"),
        )
        .arg(flag(
            "verbose",
            Some('v'),
            "Show per-track patching details",
        ))
}

fn watch() -> Command {
    Command::new("watch")
        .infer_long_args(true)
        .about("Watch a directory and auto-convert new video files")
        .arg(
            Arg::new("directory")
                .value_name("dir")
                .required(true)
                .help("Directory to watch"),
        )
        .arg(
            text(
                "output",
                "DIR",
                "Output directory (default: same directory as input file)",
            )
            .short('o'),
        )
        .arg(
            text(
                "settle",
                "S",
                "Seconds a file must be stable before converting (default: 10)",
            )
            .default_value("10")
            .value_parser(value_parser!(f64)),
        )
        .arg(flag("prefer-hevc10", None, HEVC10))
        .arg(crf())
        .arg(flag("keep-av1", None, "Copy AV1 instead of transcoding"))
        .arg(flag(
            "verbose",
            Some('v'),
            "Show ffmpeg command and post-processing steps",
        ))
        .arg(flag("fix-dts", None, DTS))
        .arg(text("quarantine", "DIR", QUARANTINE))
}

fn deploy() -> Command {
    let gate = Command::new("gate")
        .about("Read or change the local deployment gate")
        .arg(
            Arg::new("action")
                .value_parser([
                    "status",
                    "close",
                    "open",
                    "begin-migration",
                    "finish-migration",
                    "permits",
                    "reap-permits",
                    "abandon-permit",
                    "history",
                ])
                .required(true),
        )
        .arg(text("database", "PATH", "Manager database file").required(true))
        .arg(text("reason", "TEXT", "Reason for close").required_if_eq("action", "close"))
        .arg(
            text("after", "N", "Last gate-history identifier")
                .value_parser(value_parser!(i64).range(0..))
                .default_value("0"),
        )
        .arg(
            text("permit-id", "N", "Permit to abandon")
                .value_parser(value_parser!(i64).range(1..))
                .required_if_eq("action", "abandon-permit"),
        )
        .arg(
            text(
                "evidence",
                "TEXT",
                "Operator evidence for abandoning an unknown holder",
            )
            .required_if_eq("action", "abandon-permit"),
        )
        .arg(
            text("limit", "N", "Maximum permits to list")
                .value_parser(value_parser!(i64).range(1..=1000))
                .default_value("100"),
        )
        .arg(
            text("generation", "N", "Generation read from gate status")
                .value_parser(value_parser!(i64))
                .required_if_eq_any([
                    ("action", "open"),
                    ("action", "begin-migration"),
                    ("action", "finish-migration"),
                ]),
        );
    Command::new("deploy")
        .about("Verified installation and deployment controls")
        .subcommand(gate)
        .subcommand(crate::deploy_release::command("install"))
        .subcommand(crate::deploy_release::command("update"))
        .subcommand(crate::deploy_release::command("rollback"))
        .subcommand(crate::deploy_window::command())
        .subcommand(crate::servarr::command())
}

fn verify() -> Command {
    Command::new("verify")
        .arg(int128("manager-parent-fd"))
        .arg(Arg::new("manager-leaf").long("manager-leaf").hide(true))
        .arg(int128("manager-device"))
        .arg(int128("manager-inode"))
        .arg(int128("manager-ctime-ns"))
        .infer_long_args(true)
        .about("Run AVPlayer-strict compliance rules against MP4 files")
        .arg(
            Arg::new("path")
                .value_name("path")
                .help("MP4 file or directory to audit"),
        )
        .arg(text(
            "scan",
            "DIR",
            "Alias for positional directory arg; recurses into the tree",
        ))
        .arg(flag(
            "json",
            None,
            "Emit a machine-readable JSON report on stdout",
        ))
        .arg(flag("prefer-hevc10", None, HEVC10))
        .arg(crf())
        .arg(flag("keep-av1", None, "Copy AV1 instead of transcoding"))
        .arg(flag("fix-dts", None, DTS))
        .arg(
            Arg::new("fail-on")
                .long("fail-on")
                .value_parser(["blocker", "warning"])
                .default_value("blocker")
                .help("Minimum severity that causes a non-zero exit (default: blocker)"),
        )
        .arg(flag(
            "verbose",
            Some('v'),
            "Also print a line for files that pass",
        ))
}

/// The whole command line. Long options may be abbreviated, as with Python's `argparse`.
#[must_use]
pub fn command() -> Command {
    Command::new("atvprep")
        .about("atvprep: converts video files to Apple TV optimized MP4")
        .version(atvprep_core::VERSION)
        .infer_long_args(true)
        .subcommand(prepare())
        .subcommand(patch())
        .subcommand(watch())
        .subcommand(verify())
        .subcommand(deploy())
        .subcommand(Command::new("version").about("Show the release version"))
}
