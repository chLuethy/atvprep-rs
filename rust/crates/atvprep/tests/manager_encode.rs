//! The Kodierfreigabe of a manual job, from the signed item to the file on disk (ADR 0008).
//!
//! Items run through the manager's own execution with the real `atvprep` binary and real ffmpeg
//! over short, generated files. A missing tool fails these tests; nothing is skipped.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use atvprep_core::encode::EncodeAllowance::{self, All, Audio, Copy};
use atvprep_core::encode::EncodeNeed;
use atvprep_manager::{
    Error,
    approval::{ApprovalItem, ApprovalVerifier, PREPARE_POLICY, ScheduleParams},
    clock::{Clock, SystemClock, format_micros},
    db::{Conn, Db},
    jobs::{Jobs, NewJob},
    leases::MediaExecutorLease,
    library::LibraryRoots,
    media::Media,
    repo::Ctx,
    schedules::run_schedule,
    schema,
    settings::{Settings, TargetProfile},
    snapshots::InputSnapshots,
    state_machine::ItemStatus,
    worker::execution::{
        ENCODE_NOT_ALLOWED, ExecutionConfig, REENCODE_TIMEOUT, execute_claimed, prepare_timeout,
    },
    worker::process::PrepareMode,
};
use atvprep_testkit::write_surround_only_mp4;
use serde_json::json;

const SECRET: &[u8] = b"approval-secret";

/// Panics naming the first media tool that cannot be started with this `PATH`.
fn require_media_tools(path: &str) {
    for tool in ["ffmpeg", "ffprobe", "mp4info", "mp4dump"] {
        Command::new(tool)
            .env("PATH", path)
            .arg("-version")
            .output()
            .unwrap_or_else(|error| panic!("{tool} must be on PATH: {error}"));
    }
}

fn search_path() -> String {
    std::env::var("PATH").expect("PATH is set")
}

/// The absolute path of `tool` on this `PATH`.
fn on_path(tool: &str) -> PathBuf {
    std::env::split_paths(&search_path())
        .map(|dir| dir.join(tool))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("{tool} must be on PATH"))
}

#[test]
#[should_panic(expected = "ffmpeg must be on PATH")]
fn a_missing_tool_fails_the_harness_loudly() {
    require_media_tools("/nonexistent");
}

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/prepare")
        .join(name)
}

/// H.264 that can be copied, EAC3 5.1 and no stereo track: the Gringa case.
fn surround_only(path: &Path) {
    write_surround_only_mp4(path);
}

/// High 10 video, which has to be re-encoded.
fn high10(path: &Path) {
    std::fs::copy(testdata("high10.mp4"), path).unwrap();
}

/// Already at the target: left alone unless a rebuild is forced.
fn at_target(path: &Path) {
    std::fs::copy(testdata("clean.mp4"), path).unwrap();
}

/// Only needs its moov moved: no encoding at all.
fn only_remux(path: &Path) {
    std::fs::copy(testdata("moovlast.mp4"), path).unwrap();
}

fn clean(path: &Path) {
    std::fs::copy(testdata("clean.mp4"), path).unwrap();
}

fn mkv_movie(path: &Path) {
    std::fs::copy(testdata("movie.mkv"), path).unwrap();
}

/// H.264 and FLAC in a Matroska file: the audio has to be transcoded, the video is copied.
fn flac_mkv(path: &Path) {
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-f", "lavfi", "-i", "testsrc2=s=160x90:r=24:d=0.5"])
        .args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo"])
        .args([
            "-t", "0.5", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "flac",
        ])
        .arg(path)
        .status()
        .expect("ffmpeg must be on PATH to generate test media");
    assert!(status.success(), "ffmpeg could not generate the FLAC file");
}

/// Another container whose streams can all be copied.
fn copyable_mkv(path: &Path) {
    std::fs::copy(testdata("movie.mkv"), path).unwrap();
}

type MakeFile = fn(&Path);

struct World {
    dir: tempfile::TempDir,
    conn: Conn,
    media: Vec<(String, i64)>,
    job: i64,
    fence: i64,
    config: ExecutionConfig,
}

/// One signed manual job over `files`, each with its own Kodierfreigabe.
fn world(files: &[(&str, MakeFile, EncodeAllowance)], delete_original: bool) -> World {
    world_with(files, delete_original, false)
}

/// Like [`world`], every item also carrying the signed decision to rebuild (`force`).
fn world_with(
    files: &[(&str, MakeFile, EncodeAllowance)],
    delete_original: bool,
    force: bool,
) -> World {
    world_profiled(files, delete_original, force, TargetProfile::default())
}

/// Like [`world_with`], under `profile` as the current Zielprofil, recorded and signed on the items.
#[allow(clippy::too_many_lines)] // One signed job, set up the way the API queues it.
fn world_profiled(
    files: &[(&str, MakeFile, EncodeAllowance)],
    delete_original: bool,
    force: bool,
    profile: TargetProfile,
) -> World {
    require_media_tools(&search_path());
    let dir = tempfile::tempdir().unwrap();
    let root_path = dir.path().canonicalize().unwrap();
    for (name, make, _) in files {
        make(&root_path.join(name));
    }
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    let clock = SystemClock;
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    let root = LibraryRoots(ctx)
        .create("Movies", root_path.to_str().unwrap())
        .unwrap();
    let mut media = Vec::new();
    for (name, _, _) in files {
        let observed =
            atvprep_manager::worker::scanner::observe(&root_path, name, 0, clock.now_micros())
                .unwrap();
        let id = Media(ctx).create(root.id, name, &observed.meta).unwrap().id;
        media.push(((*name).to_owned(), id));
    }
    ctx.conn
        .execute(
            "INSERT INTO auth_sessions(id,session_token_hash,csrf_secret_hash,expires_at,last_seen_at) VALUES(1,'t','c',?1,?2)",
            rusqlite::params![format_micros(clock.now_micros() + 600_000_000), clock.now()],
        )
        .unwrap();
    Settings(ctx)
        .set_many(
            &[
                ("target_crf", json!(profile.crf)),
                ("target_prefer_hevc10", json!(profile.prefer_hevc10)),
                ("target_keep_av1", json!(profile.keep_av1)),
                ("target_fix_dts", json!(profile.fix_dts)),
            ],
            "test",
            None,
        )
        .unwrap();
    let ids: Vec<i64> = media.iter().map(|(_, id)| *id).collect();
    let mut new = NewJob::new("fix", &ids);
    new.trigger = "manual";
    new.settings_snapshot = json!({"pipeline":"prepare","policy":PREPARE_POLICY,
        "delete_original":delete_original,"force":force,"manual_approval":true,"approved_by":1,
        "approved_at":clock.now_micros()/1_000_000,"profile":profile.to_json()});
    for ((_, id), (_, _, allowance)) in media.iter().zip(files) {
        new.encode_media.insert(*id, *allowance);
    }
    let job = Jobs(ctx).create(&new).unwrap();
    let verifier = ApprovalVerifier::new(SECRET).unwrap();
    for mut item in Jobs(ctx).items(job.id).unwrap() {
        let recorded = InputSnapshots(ctx).get_for_item(item.id).unwrap().unwrap();
        let identity = [
            recorded["file_device"].clone(),
            recorded["file_inode"].clone(),
            recorded["file_ctime_ns"].clone(),
            recorded["sidecar_fingerprint"].clone(),
        ];
        let encode = EncodeAllowance::parse(item.settings_snapshot["encode"].as_str().unwrap())
            .expect("every item records its allowance");
        let mac = verifier.sign(
            &ApprovalItem {
                job_id: job.id,
                media_id: item.media_file_id,
                action: "fix",
                identity: &identity,
                encode,
                delete_original,
                force,
                profile,
            },
            1,
            // The time the job records, not a second reading of the clock: a second boundary
            // between the two would sign another approval and the worker would refuse it.
            item.settings_snapshot["approved_at"].as_i64().unwrap(),
        );
        item.settings_snapshot["approval_mac"] = json!(mac);
        ctx.conn
            .execute(
                "UPDATE job_items SET settings_snapshot_json=?1 WHERE id=?2",
                rusqlite::params![item.settings_snapshot.to_string(), item.id],
            )
            .unwrap();
    }
    let mut lease = MediaExecutorLease::new(ctx);
    assert!(lease.acquire("worker", 600).unwrap());
    let fence = lease.fencing_token.unwrap();
    let config = ExecutionConfig {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_atvprep")),
        environment: BTreeMap::from([("PATH".to_owned(), search_path())]),
        timeout: Duration::from_secs(120),
        grace: Duration::from_secs(2),
        lease_seconds: 600,
        shutdown: None,
        approvals: ApprovalVerifier::new(SECRET).map(Arc::new),
        session_idle_seconds: 3600,
    };
    World {
        dir,
        conn,
        media,
        job: job.id,
        fence,
        config,
    }
}

struct Ended {
    status: ItemStatus,
    code: Option<String>,
    detail: Option<String>,
}

impl World {
    fn ctx<'a>(&'a self, clock: &'a SystemClock) -> Ctx<'a> {
        Ctx {
            conn: self.conn.raw(),
            clock,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Run „Prüfen“ on the first file with the real CLI and return what was stored about it.
    fn verified(&self) -> (Option<EncodeNeed>, String) {
        let clock = SystemClock;
        let ctx = self.ctx(&clock);
        let media = self.media[0].1;
        let ids = [media];
        let job = Jobs(ctx).create(&NewJob::new("verify", &ids)).unwrap();
        let (item, result) = self.run_item_of(job.id);
        result.unwrap();
        assert_eq!(self.ended(item).status, ItemStatus::Succeeded);
        let row = Media(ctx).get(media).unwrap().unwrap();
        (row.encode_need, row.planned_change)
    }

    /// Claim the next queued item and run it to its end.
    fn run_next(&self) -> (i64, Result<(), Error>) {
        self.run_item_of(self.job)
    }

    fn run_item_of(&self, job: i64) -> (i64, Result<(), Error>) {
        let clock = SystemClock;
        let ctx = self.ctx(&clock);
        let item = Jobs(ctx)
            .claim_media_item(job, "worker", self.fence, 600, 1)
            .unwrap()
            .expect("an item is queued");
        let result = execute_claimed(ctx, item.id, "worker", self.fence, &self.config).map(|_| ());
        (item.id, result)
    }

    fn ended(&self, item: i64) -> Ended {
        let clock = SystemClock;
        let item = Jobs(self.ctx(&clock)).get_item(item).unwrap().unwrap();
        Ended {
            status: item.status,
            code: item.error_code,
            detail: item.error_detail,
        }
    }
}

/// The codec names of the `kind` streams in `path`, in stream order.
fn codecs(path: &Path, kind: &str) -> Vec<String> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", kind])
        .args(["-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("ffprobe must be on PATH");
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        // ffprobe appends an empty csv column to a stream that has side data.
        .map(|name| name.trim_end_matches(',').to_owned())
        .collect()
}

fn run_single(make: MakeFile, allowance: EncodeAllowance) -> (World, Ended) {
    let world = world(&[("file.mp4", make, allowance)], false);
    let (item, result) = world.run_next();
    result.unwrap();
    let ended = world.ended(item);
    (world, ended)
}

/// Refused as asked: a success with the hint, the need and allowance named, and the file untouched.
fn assert_refused(make: MakeFile, allowance: EncodeAllowance, need: &str) {
    let world = world(&[("file.mp4", make, allowance)], false);
    let before = std::fs::read(world.path("file.mp4")).unwrap();
    let (item, result) = world.run_next();
    result.unwrap();
    let ended = world.ended(item);
    assert_eq!(ended.status, ItemStatus::Succeeded);
    assert_eq!(ended.code.as_deref(), Some(ENCODE_NOT_ALLOWED));
    let detail = ended.detail.unwrap();
    assert!(detail.contains(need), "{detail}");
    assert!(detail.contains(allowance.as_str()), "{detail}");
    assert_eq!(std::fs::read(world.path("file.mp4")).unwrap(), before);
    assert_eq!(world.names(), ["file.mp4"]);
}

fn assert_ran(ended: &Ended) {
    assert_eq!(ended.status, ItemStatus::Succeeded, "{:?}", ended.detail);
    assert_eq!(ended.code, None, "{:?}", ended.detail);
}

/// The stored need and change, and that the run behaves exactly as they say: the lowest allowance
/// that covers the need runs, the one below it leaves the file alone.
fn assert_need_matches_the_run(
    name: &str,
    make: MakeFile,
    need: EncodeNeed,
    change: &str,
    below: Option<EncodeAllowance>,
) {
    let world_at = |allowance| world(&[(name, make, allowance)], false);
    let required = need.required();
    let running = world_at(required);
    assert_eq!(
        running.verified(),
        (Some(need), change.to_owned()),
        "{name}"
    );
    let (item, result) = running.run_next();
    result.unwrap();
    assert_ran(&running.ended(item));
    if let Some(below) = below {
        let refused = world_at(below);
        assert_eq!(
            refused.verified(),
            (Some(need), change.to_owned()),
            "{name}"
        );
        let (item, result) = refused.run_next();
        result.unwrap();
        assert_eq!(
            refused.ended(item).code.as_deref(),
            Some(ENCODE_NOT_ALLOWED),
            "{name}"
        );
    }
}

#[test]
fn a_missing_stereo_track_is_stored_as_audio_and_runs_as_audio() {
    assert_need_matches_the_run(
        "file.mp4",
        surround_only,
        EncodeNeed::Audio,
        "audio",
        Some(Copy),
    );
}

#[test]
fn a_video_transcode_is_stored_as_video_and_runs_as_video() {
    assert_need_matches_the_run(
        "file.mp4",
        high10,
        EncodeNeed::Video,
        "reencode",
        Some(Audio),
    );
}

#[test]
fn a_remux_is_stored_as_no_need_and_runs_under_copy() {
    assert_need_matches_the_run("file.mp4", only_remux, EncodeNeed::None, "remux", None);
}

#[test]
fn an_audio_transcode_is_stored_as_audio_and_runs_as_audio() {
    assert_need_matches_the_run("file.mkv", flac_mkv, EncodeNeed::Audio, "audio", Some(Copy));
}

#[test]
fn a_check_of_another_container_only_feeds_the_need_and_not_its_state() {
    let world = world(&[("file.mkv", flac_mkv, Copy)], false);
    world.verified();
    let clock = SystemClock;
    let row = Media(world.ctx(&clock))
        .get(world.media[0].1)
        .unwrap()
        .unwrap();
    assert_eq!(row.compliance_state, "unknown");
    assert!(
        row.compliance_reasons
            .iter()
            .all(|r| !r.contains("rule crashed"))
    );
}

#[test]
fn a_clean_file_is_stored_with_no_need_and_no_change() {
    let world = world(&[("file.mp4", clean, Copy)], false);
    assert_eq!(
        world.verified(),
        (Some(EncodeNeed::None), "none".to_owned())
    );
}

#[test]
fn another_container_is_stored_as_the_planner_sees_it_and_runs_at_that_level() {
    let world = world(&[("file.mkv", mkv_movie, Copy)], false);
    let (need, change) = world.verified();
    let need = need.expect("the planner reads the container too");
    assert_ne!(change, "unknown");
    assert_eq!(need, EncodeNeed::None, "this fixture only copies");
    let (item, result) = world.run_next();
    result.unwrap();
    assert_ran(&world.ended(item));
    assert_eq!(world.ended(item).code, None);
}

/// A FLAC Matroska file, next to it the MP4 it was converted to, which still lacks a stereo track.
fn flac_mkv_beside_surround_mp4(path: &Path) {
    flac_mkv(path);
    surround_only(&path.with_extension("mp4"));
}

#[test]
fn a_container_with_an_mp4_next_to_it_is_stored_and_run_as_that_mp4() {
    // Prepare works on the MP4; the container alone would need an audio transcode, not a downmix.
    let world_at = |allowance| {
        world(
            &[("file.mkv", flac_mkv_beside_surround_mp4, allowance)],
            false,
        )
    };
    let running = world_at(Audio);
    assert_eq!(
        running.verified(),
        (Some(EncodeNeed::Audio), "audio".to_owned())
    );
    let container = std::fs::read(running.path("file.mkv")).unwrap();
    let (item, result) = running.run_next();
    result.unwrap();
    assert_ran(&running.ended(item));
    assert_eq!(running.names(), ["file.mkv", "file.mp4"]);
    assert_eq!(std::fs::read(running.path("file.mkv")).unwrap(), container);
    let audio = codecs(&running.path("file.mp4"), "a");
    assert!(audio.contains(&"aac".to_owned()), "{audio:?}");
    assert!(audio.contains(&"eac3".to_owned()), "{audio:?}");

    let refused = world_at(Copy);
    assert_eq!(
        refused.verified(),
        (Some(EncodeNeed::Audio), "audio".to_owned())
    );
    let sibling = std::fs::read(refused.path("file.mp4")).unwrap();
    let (item, result) = refused.run_next();
    result.unwrap();
    assert_eq!(
        refused.ended(item).code.as_deref(),
        Some(ENCODE_NOT_ALLOWED)
    );
    assert_eq!(std::fs::read(refused.path("file.mp4")).unwrap(), sibling);
}

#[test]
fn gringa_eac3_surround_without_stereo_gets_its_aac_track_with_audio() {
    let (world, ended) = run_single(surround_only, Audio);
    assert_ran(&ended);
    let file = world.path("file.mp4");
    assert!(codecs(&file, "a").contains(&"aac".to_owned()));
    assert!(codecs(&file, "a").contains(&"eac3".to_owned()));
    assert_eq!(codecs(&file, "v"), ["h264"]);
}

#[test]
fn copy_leaves_a_file_that_needs_audio_unchanged() {
    assert_refused(surround_only, Copy, "audio");
}

#[test]
fn copy_leaves_a_file_that_needs_video_unchanged() {
    assert_refused(high10, Copy, "video");
}

#[test]
fn audio_leaves_a_file_that_needs_video_unchanged() {
    assert_refused(high10, Audio, "video");
}

#[test]
fn all_encodes_audio_when_that_is_all_the_file_needs() {
    let (world, ended) = run_single(surround_only, All);
    assert_ran(&ended);
    let file = world.path("file.mp4");
    assert!(codecs(&file, "a").contains(&"aac".to_owned()));
    assert_eq!(codecs(&file, "v"), ["h264"]);
}

#[test]
fn all_re_encodes_the_video_of_an_mp4() {
    let (world, ended) = run_single(high10, All);
    assert_ran(&ended);
    assert_eq!(codecs(&world.path("file.mp4"), "v"), ["hevc"]);
    assert_eq!(world.names(), ["file.mp4"]);
}

#[test]
fn every_allowance_runs_a_file_that_needs_no_encoding() {
    for allowance in [Copy, Audio, All] {
        let (world, ended) = run_single(only_remux, allowance);
        assert_ran(&ended);
        assert_eq!(
            codecs(&world.path("file.mp4"), "v"),
            ["h264"],
            "{allowance:?}"
        );
    }
}

#[test]
fn a_file_at_the_target_is_left_alone_without_force() {
    let world = world(&[("file.mp4", at_target, Audio)], false);
    let before = std::fs::read(world.path("file.mp4")).unwrap();
    let (item, result) = world.run_next();
    result.unwrap();
    assert_ran(&world.ended(item));
    assert_eq!(std::fs::read(world.path("file.mp4")).unwrap(), before);
}

#[test]
fn force_rebuilds_a_file_at_the_target_and_only_copies_under_copy() {
    let world = world_with(&[("file.mp4", at_target, Copy)], false, true);
    let before = std::fs::read(world.path("file.mp4")).unwrap();
    let streams = codecs(&world.path("file.mp4"), "v");
    let (item, result) = world.run_next();
    result.unwrap();
    assert_ran(&world.ended(item));
    assert!(
        std::fs::read(world.path("file.mp4")).unwrap() != before,
        "the file was rebuilt"
    );
    assert_eq!(codecs(&world.path("file.mp4"), "v"), streams);
    assert_eq!(world.names(), ["file.mp4"]);
}

#[test]
fn force_stays_bound_to_the_allowance_and_a_file_needing_more_stays_unchanged() {
    for (make, allowance) in [
        (high10 as MakeFile, Audio),
        (high10 as MakeFile, Copy),
        (surround_only as MakeFile, Copy),
    ] {
        let world = world_with(&[("file.mp4", make, allowance)], false, true);
        let before = std::fs::read(world.path("file.mp4")).unwrap();
        let (item, result) = world.run_next();
        result.unwrap();
        let ended = world.ended(item);
        assert_eq!(ended.status, ItemStatus::Succeeded, "{allowance:?}");
        assert_eq!(
            ended.code.as_deref(),
            Some(ENCODE_NOT_ALLOWED),
            "{allowance:?}"
        );
        assert_eq!(std::fs::read(world.path("file.mp4")).unwrap(), before);
        assert_eq!(world.names(), ["file.mp4"]);
    }
}

#[test]
fn each_item_of_a_mixed_job_runs_at_its_own_signed_allowance() {
    let world = world(
        &[
            ("copy.mp4", high10, Copy),
            ("audio.mp4", surround_only, Audio),
            ("all.mp4", high10, All),
        ],
        false,
    );
    let copy_before = std::fs::read(world.path("copy.mp4")).unwrap();
    let mut outcome = BTreeMap::new();
    for (name, _) in &world.media {
        let (item, result) = world.run_next();
        result.unwrap();
        outcome.insert(name.clone(), world.ended(item).code);
    }
    assert_eq!(
        outcome["copy.mp4"].as_deref(),
        Some(ENCODE_NOT_ALLOWED),
        "the copy item stops at copy"
    );
    assert_eq!(std::fs::read(world.path("copy.mp4")).unwrap(), copy_before);
    assert_eq!(outcome["audio.mp4"], None);
    assert!(codecs(&world.path("audio.mp4"), "a").contains(&"aac".to_owned()));
    assert_eq!(outcome["all.mp4"], None);
    assert_eq!(codecs(&world.path("all.mp4"), "v"), ["hevc"]);
}

#[test]
fn a_changed_allowance_breaks_the_signature_and_nothing_runs() {
    let world = world(&[("file.mp4", high10, Copy)], false);
    let before = std::fs::read(world.path("file.mp4")).unwrap();
    world
        .conn
        .raw()
        .execute(
            "UPDATE job_items SET settings_snapshot_json=json_set(settings_snapshot_json,'$.encode','all')",
            [],
        )
        .unwrap();
    let (_, result) = world.run_next();
    assert!(result.is_err());
    assert_eq!(std::fs::read(world.path("file.mp4")).unwrap(), before);
}

#[test]
fn a_job_queued_under_an_older_policy_is_refused_for_good_and_says_to_queue_it_again() {
    let world = world(&[("file.mp4", high10, All)], false);
    let before = std::fs::read(world.path("file.mp4")).unwrap();
    world
        .conn
        .raw()
        .execute(
            "UPDATE job_items SET settings_snapshot_json=json_set(settings_snapshot_json,'$.policy','copy-only-v2')",
            [],
        )
        .unwrap();
    let (item, result) = world.run_next();
    assert!(result.is_err());
    let ended = world.ended(item);
    assert_eq!(ended.status, ItemStatus::StaleInput);
    // Word for word: the job detail translates exactly this text.
    assert_eq!(
        ended.detail.as_deref(),
        Some("This job predates the current policy. Queue the preparation again.")
    );
    assert_eq!(std::fs::read(world.path("file.mp4")).unwrap(), before);
}

/// The item ran, the new MP4 is there and the old container is not.
fn assert_container_replaced(world: &World, ended: &Ended) {
    assert_ran(ended);
    assert_eq!(world.names(), ["file.mp4"]);
    assert_eq!(codecs(&world.path("file.mp4"), "v"), ["h264"]);
}

#[test]
fn delete_original_removes_a_remuxed_container_at_every_allowance() {
    for allowance in [Copy, Audio, All] {
        let world = world(&[("file.mkv", copyable_mkv, allowance)], true);
        let (item, result) = world.run_next();
        result.unwrap();
        assert_container_replaced(&world, &world.ended(item));
    }
}

#[test]
fn a_removed_container_leaves_the_library_without_counting_as_missing() {
    let world = world(&[("file.mkv", copyable_mkv, Copy)], true);
    let (item, result) = world.run_next();
    result.unwrap();
    assert_container_replaced(&world, &world.ended(item));
    let clock = SystemClock;
    let ctx = world.ctx(&clock);
    let media = Media(ctx).get(world.media[0].1).unwrap().unwrap();
    assert_eq!(media.discovery_state, "missing");
    assert!(
        media.dismissed_at.is_some(),
        "not offered as a missing file"
    );
    let pending: i64 = ctx
        .conn
        .query_row(
            "SELECT count(*) FROM scan_runs WHERE status='queued'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 1, "the new MP4 is picked up by a scan");
}

#[test]
fn without_delete_original_the_container_stays_next_to_the_new_mp4() {
    let world = world(&[("file.mkv", copyable_mkv, Copy)], false);
    let before = std::fs::read(world.path("file.mkv")).unwrap();
    let (item, result) = world.run_next();
    result.unwrap();
    assert_ran(&world.ended(item));
    assert_eq!(world.names(), ["file.mkv", "file.mp4"]);
    assert_eq!(std::fs::read(world.path("file.mkv")).unwrap(), before);
}

#[test]
fn a_new_mp4_that_fails_verification_keeps_the_container() {
    let mut world = world(&[("file.mkv", copyable_mkv, Copy)], true);
    // Real ffmpeg does the work, then the output is swapped for one the check rejects.
    let tools = tempfile::tempdir().unwrap();
    let stand_in = tools.path().join("ffmpeg");
    std::fs::write(
        &stand_in,
        format!(
            "#!/bin/sh\nfor last; do :; done\n{ffmpeg} \"$@\" || exit $?\ncase \"$last\" in -|/dev/null) ;; *) cp {bad} \"$last\" ;; esac\n",
            ffmpeg = on_path("ffmpeg").display(),
            bad = testdata("high10.mp4").display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stand_in, std::fs::Permissions::from_mode(0o755)).unwrap();
    world.config.environment.insert(
        "PATH".to_owned(),
        format!("{}:{}", tools.path().display(), search_path()),
    );
    let before = std::fs::read(world.path("file.mkv")).unwrap();
    let (item, result) = world.run_next();
    result.unwrap();
    let ended = world.ended(item);
    assert_eq!(ended.status, ItemStatus::Failed, "{:?}", ended.detail);
    assert_eq!(ended.code.as_deref(), Some("verification_failed"));
    assert_eq!(world.names(), ["file.mkv"]);
    assert_eq!(std::fs::read(world.path("file.mkv")).unwrap(), before);
}

#[test]
fn a_container_with_its_converted_mp4_already_next_to_it_is_not_removed() {
    let world = world(&[("file.mkv", copyable_mkv, Copy)], true);
    only_remux(&world.path("file.mp4"));
    let before = std::fs::read(world.path("file.mkv")).unwrap();
    let (item, result) = world.run_next();
    result.unwrap();
    assert_ran(&world.ended(item));
    assert_eq!(world.names(), ["file.mkv", "file.mp4"]);
    assert_eq!(std::fs::read(world.path("file.mkv")).unwrap(), before);
}

#[test]
fn preparing_a_converted_mp4_leaves_its_kept_container_alone() {
    let world = world(&[("file.mp4", only_remux, Copy)], true);
    copyable_mkv(&world.path("file.mkv"));
    let before = std::fs::read(world.path("file.mkv")).unwrap();
    let (item, result) = world.run_next();
    result.unwrap();
    assert_ran(&world.ended(item));
    assert_eq!(world.names(), ["file.mkv", "file.mp4"]);
    assert_eq!(std::fs::read(world.path("file.mkv")).unwrap(), before);
}

#[test]
fn a_changed_delete_original_breaks_the_signature_and_nothing_runs() {
    let world = world(&[("file.mkv", copyable_mkv, Copy)], false);
    world
        .conn
        .raw()
        .execute(
            "UPDATE job_items SET settings_snapshot_json=json_set(settings_snapshot_json,'$.delete_original',json('true'))",
            [],
        )
        .unwrap();
    let (_, result) = world.run_next();
    assert!(result.is_err());
    assert_eq!(world.names(), ["file.mkv"]);
}

#[test]
fn only_all_with_a_video_or_unknown_need_gets_the_four_hour_limit() {
    let configured = Duration::from_secs(600);
    let limit = |encode, need, configured| {
        prepare_timeout(
            configured,
            PrepareMode {
                encode,
                delete_original: false,
                force: false,
                profile: TargetProfile::default(),
            },
            need,
        )
    };
    assert_eq!(REENCODE_TIMEOUT, Duration::from_hours(4));
    for need in [None, Some(EncodeNeed::Audio), Some(EncodeNeed::Video)] {
        assert_eq!(limit(Copy, need, configured), configured);
        assert_eq!(limit(Audio, need, configured), configured);
    }
    // „Audio und Video“ on a file that only needs audio stays at the shorter limit.
    assert_eq!(limit(All, Some(EncodeNeed::None), configured), configured);
    assert_eq!(limit(All, Some(EncodeNeed::Audio), configured), configured);
    assert_eq!(
        limit(All, Some(EncodeNeed::Video), configured),
        REENCODE_TIMEOUT
    );
    assert_eq!(limit(All, None, configured), REENCODE_TIMEOUT);
    // A longer configured limit is never shortened.
    let long = Duration::from_hours(6);
    assert_eq!(limit(All, Some(EncodeNeed::Video), long), long);
}

const KEEP_AV1: TargetProfile = TargetProfile {
    crf: 18,
    prefer_hevc10: false,
    keep_av1: true,
    fix_dts: false,
};

fn av1(path: &Path) {
    std::fs::copy(testdata("av1.mp4"), path).unwrap();
}

/// One file under `profile`, already checked with the real CLI: the stored need and the world.
fn checked_under(
    make: MakeFile,
    allowance: EncodeAllowance,
    profile: TargetProfile,
) -> (World, Option<EncodeNeed>) {
    let world = world_profiled(&[("file.mp4", make, allowance)], false, false, profile);
    let (need, _) = world.verified();
    (world, need)
}

#[test]
fn an_av1_file_needs_video_under_the_default_profile() {
    let (_, need) = checked_under(av1, Copy, TargetProfile::default());
    assert_eq!(need, Some(EncodeNeed::Video));
}

#[test]
fn with_keep_av1_a_fitting_av1_file_needs_nothing_and_stays_av1() {
    let (world, need) = checked_under(av1, Copy, KEEP_AV1);
    assert_eq!(need, Some(EncodeNeed::None));
    let (item, result) = world.run_next();
    result.unwrap();
    assert_ran(&world.ended(item));
    assert_eq!(codecs(&world.path("file.mp4"), "v"), ["av1"]);
}

/// Whether the stored check of the first file still warns that its video needs a re-encode.
fn warns_of_a_reencode(world: &World) -> bool {
    let clock = SystemClock;
    let row = Media(world.ctx(&clock))
        .get(world.media[0].1)
        .unwrap()
        .unwrap();
    row.compliance_reasons
        .iter()
        .any(|reason| reason.contains("video-reencode"))
}

#[test]
fn the_reencode_warning_follows_the_profile_like_the_need() {
    let (world, need) = checked_under(av1, Copy, TargetProfile::default());
    assert_eq!(need, Some(EncodeNeed::Video));
    assert!(warns_of_a_reencode(&world));
    let (world, need) = checked_under(av1, Copy, KEEP_AV1);
    assert_eq!(need, Some(EncodeNeed::None));
    assert!(!warns_of_a_reencode(&world));
    let hevc10 = TargetProfile {
        prefer_hevc10: true,
        ..TargetProfile::default()
    };
    let (world, need) = checked_under(clean, Copy, hevc10);
    assert_eq!(need, Some(EncodeNeed::Video));
    assert!(warns_of_a_reencode(&world));
}

#[test]
fn prefer_hevc10_turns_h264_into_a_video_need_and_the_run_follows_it() {
    let profile = TargetProfile {
        prefer_hevc10: true,
        ..TargetProfile::default()
    };
    let (world, need) = checked_under(clean, All, profile);
    assert_eq!(need, Some(EncodeNeed::Video));
    let (item, result) = world.run_next();
    result.unwrap();
    assert_ran(&world.ended(item));
    assert_eq!(codecs(&world.path("file.mp4"), "v"), ["hevc"]);
}

#[test]
fn the_crf_of_the_profile_reaches_the_encoder() {
    let size_at = |crf| {
        let profile = TargetProfile {
            crf,
            ..TargetProfile::default()
        };
        let world = world_profiled(&[("file.mp4", high10, All)], false, false, profile);
        let (item, result) = world.run_next();
        result.unwrap();
        assert_ran(&world.ended(item));
        std::fs::metadata(world.path("file.mp4")).unwrap().len()
    };
    assert!(size_at(10) > size_at(45), "a lower CRF keeps more data");
}

#[test]
fn a_profile_changed_after_queueing_refuses_the_item_for_good() {
    let world = world(&[("file.mp4", high10, All)], false);
    let before = std::fs::read(world.path("file.mp4")).unwrap();
    let clock = SystemClock;
    Settings(world.ctx(&clock))
        .set("target_keep_av1", &json!(true), "test", None)
        .unwrap();
    let (item, result) = world.run_next();
    assert!(result.is_err());
    let ended = world.ended(item);
    assert_eq!(ended.status, ItemStatus::StaleInput);
    assert_eq!(
        ended.detail.as_deref(),
        Some("The target profile changed after this job was queued. Queue the preparation again.")
    );
    assert_eq!(std::fs::read(world.path("file.mp4")).unwrap(), before);
}

#[test]
fn a_profile_rewritten_on_the_item_breaks_the_signature_even_if_it_matches_the_settings() {
    let world = world(&[("file.mp4", high10, All)], false);
    let before = std::fs::read(world.path("file.mp4")).unwrap();
    let clock = SystemClock;
    Settings(world.ctx(&clock))
        .set("target_crf", &json!(30), "test", None)
        .unwrap();
    world
        .conn
        .raw()
        .execute(
            "UPDATE job_items SET settings_snapshot_json=json_set(settings_snapshot_json,'$.profile.crf',30)",
            [],
        )
        .unwrap();
    let (_, result) = world.run_next();
    assert!(result.is_err());
    assert_eq!(std::fs::read(world.path("file.mp4")).unwrap(), before);
}

#[test]
fn an_item_without_a_profile_is_refused_for_good() {
    let world = world(&[("file.mp4", high10, All)], false);
    world
        .conn
        .raw()
        .execute(
            "UPDATE job_items SET settings_snapshot_json=json_remove(settings_snapshot_json,'$.profile')",
            [],
        )
        .unwrap();
    let (item, result) = world.run_next();
    assert!(result.is_err());
    assert_eq!(world.ended(item).status, ItemStatus::StaleInput);
}

/// A library of `files`, each checked with the real CLI, and an approved schedule that prepares
/// them under `encode`, signed as the API signs it when the operator turns it on. Nothing is queued
/// yet: [`World::run_schedule`] does that.
fn schedule_world(files: &[(&str, MakeFile)], encode: EncodeAllowance) -> World {
    require_media_tools(&search_path());
    let dir = tempfile::tempdir().unwrap();
    let root_path = dir.path().canonicalize().unwrap();
    for (name, make) in files {
        make(&root_path.join(name));
    }
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    let clock = SystemClock;
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    let root = LibraryRoots(ctx)
        .create("Movies", root_path.to_str().unwrap())
        .unwrap();
    let mut media = Vec::new();
    for (name, _) in files {
        let observed =
            atvprep_manager::worker::scanner::observe(&root_path, name, 0, clock.now_micros())
                .unwrap();
        let id = Media(ctx).create(root.id, name, &observed.meta).unwrap().id;
        media.push(((*name).to_owned(), id));
    }
    let mut lease = MediaExecutorLease::new(ctx);
    assert!(lease.acquire("worker", 600).unwrap());
    let fence = lease.fencing_token.unwrap();
    let config = ExecutionConfig {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_atvprep")),
        environment: BTreeMap::from([("PATH".to_owned(), search_path())]),
        timeout: Duration::from_secs(120),
        grace: Duration::from_secs(2),
        lease_seconds: 600,
        shutdown: None,
        approvals: ApprovalVerifier::new(SECRET).map(Arc::new),
        session_idle_seconds: 3600,
    };
    let world = World {
        dir,
        conn,
        media,
        job: 0,
        fence,
        config,
    };
    world.verify_all();
    let params = ScheduleParams {
        schedule_id: 1,
        action: "prepare",
        root_id: Some(root.id),
        compliance_state: None,
        max_files: 6,
        delete_original: false,
        encode,
        profile: TargetProfile::default(),
    };
    let mac = ApprovalVerifier::new(SECRET)
        .unwrap()
        .sign_schedule(&params, 1, 1_700_000_000);
    world
        .conn
        .raw()
        .execute(
            "INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,max_files,enabled,armed_at,encode,approved_by,approved_at,approval_mac)
             VALUES(1,'Nightly','prepare',?1,127,'03:00',6,1,'2000-01-01',?2,1,1700000000,?3)",
            rusqlite::params![root.id, encode.as_str(), mac],
        )
        .unwrap();
    world
}

impl World {
    /// Run „Prüfen“ on every file with the real CLI, so each stores its Kodierbedarf.
    fn verify_all(&self) {
        let clock = SystemClock;
        let ids: Vec<i64> = self.media.iter().map(|(_, id)| *id).collect();
        let job = Jobs(self.ctx(&clock))
            .create(&NewJob::new("verify", &ids))
            .unwrap();
        for _ in &ids {
            let (item, result) = self.run_item_of(job.id);
            result.unwrap();
            let ended = self.ended(item);
            let log: Option<String> = self
                .conn
                .raw()
                .query_row(
                    "SELECT log_tail FROM job_items WHERE id=?1",
                    [item],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                ended.status,
                ItemStatus::Succeeded,
                "{:?} {:?} {log:?}",
                ended.code,
                ended.detail
            );
        }
    }

    /// The stored Kodierbedarf of `name`.
    fn need_of(&self, name: &str) -> Option<EncodeNeed> {
        let clock = SystemClock;
        let id = self
            .media
            .iter()
            .find(|(known, _)| known == name)
            .unwrap()
            .1;
        Media(self.ctx(&clock))
            .get(id)
            .unwrap()
            .unwrap()
            .encode_need
    }

    /// Run the schedule once, as „Run now“ does, and return the names of the queued files.
    fn run_schedule(&mut self) -> Vec<String> {
        let clock = SystemClock;
        let message = run_schedule(self.ctx(&clock), 1).unwrap();
        assert!(message.starts_with("Queued "), "{message}");
        self.job = self
            .conn
            .raw()
            .query_row("SELECT max(id) FROM jobs", [], |row| row.get(0))
            .unwrap();
        Jobs(self.ctx(&clock))
            .items(self.job)
            .unwrap()
            .iter()
            .map(|item| {
                self.media
                    .iter()
                    .find(|(_, id)| *id == item.media_file_id)
                    .unwrap()
                    .0
                    .clone()
            })
            .collect()
    }
}

#[test]
fn a_schedule_with_audio_prepares_gringa_and_leaves_the_file_that_needs_video_out() {
    let mut world = schedule_world(
        &[("gringa.mp4", surround_only), ("high10.mp4", high10)],
        Audio,
    );
    assert_eq!(world.need_of("gringa.mp4"), Some(EncodeNeed::Audio));
    assert_eq!(world.need_of("high10.mp4"), Some(EncodeNeed::Video));
    let untouched = std::fs::read(world.path("high10.mp4")).unwrap();
    assert_eq!(world.run_schedule(), ["gringa.mp4"]);
    let (item, result) = world.run_next();
    result.unwrap();
    assert_ran(&world.ended(item));
    let file = world.path("gringa.mp4");
    assert!(codecs(&file, "a").contains(&"aac".to_owned()));
    assert_eq!(codecs(&file, "v"), ["h264"]);
    assert_eq!(std::fs::read(world.path("high10.mp4")).unwrap(), untouched);
}

#[test]
fn a_schedule_with_copy_takes_only_files_that_need_no_encoding() {
    let mut world = schedule_world(
        &[
            ("gringa.mp4", surround_only),
            ("high10.mp4", high10),
            ("moovlast.mp4", only_remux),
        ],
        Copy,
    );
    assert_eq!(world.need_of("moovlast.mp4"), Some(EncodeNeed::None));
    assert_eq!(world.run_schedule(), ["moovlast.mp4"]);
    let (item, result) = world.run_next();
    result.unwrap();
    assert_ran(&world.ended(item));
}

#[test]
fn a_schedule_with_audio_and_video_re_encodes_the_video() {
    let mut world = schedule_world(&[("high10.mp4", high10)], All);
    assert_eq!(world.run_schedule(), ["high10.mp4"]);
    let (item, result) = world.run_next();
    result.unwrap();
    assert_ran(&world.ended(item));
    let profile = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=profile", "-of", "csv=p=0"])
        .arg(world.path("high10.mp4"))
        .output()
        .unwrap();
    let profile = String::from_utf8_lossy(&profile.stdout).trim().to_owned();
    assert_ne!(profile, "High 10", "the video was re-encoded");
}

#[test]
fn a_schedule_item_is_refused_when_the_allowance_changes_after_queueing() {
    let mut world = schedule_world(&[("gringa.mp4", surround_only)], Audio);
    let before = std::fs::read(world.path("gringa.mp4")).unwrap();
    assert_eq!(world.run_schedule(), ["gringa.mp4"]);
    world
        .conn
        .raw()
        .execute("UPDATE schedules SET encode='all' WHERE id=1", [])
        .unwrap();
    let (item, result) = world.run_next();
    assert!(result.is_err());
    assert_eq!(world.ended(item).status, ItemStatus::StaleInput);
    assert_eq!(std::fs::read(world.path("gringa.mp4")).unwrap(), before);
}

#[test]
fn a_schedule_item_is_refused_when_the_target_profile_changes_after_queueing() {
    let mut world = schedule_world(&[("gringa.mp4", surround_only)], Audio);
    let before = std::fs::read(world.path("gringa.mp4")).unwrap();
    assert_eq!(world.run_schedule(), ["gringa.mp4"]);
    // Written straight into the table: the settings module would also pause the schedule.
    world
        .conn
        .raw()
        .execute(
            "INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('target_crf','24','int',1)",
            [],
        )
        .unwrap();
    let (item, result) = world.run_next();
    assert!(result.is_err());
    assert_eq!(world.ended(item).status, ItemStatus::StaleInput);
    assert_eq!(std::fs::read(world.path("gringa.mp4")).unwrap(), before);
}
