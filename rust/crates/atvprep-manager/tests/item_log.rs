//! What Analysieren and Prüfen leave in an item's log, run against stand-in tools.

use atvprep_manager::{
    clock::SystemClock,
    db::{Conn, Db},
    jobs::{Jobs, NewJob},
    leases::MediaExecutorLease,
    library::LibraryRoots,
    media::Media,
    models::MediaMeta,
    repo::Ctx,
    schema,
    state_machine::ItemStatus,
    worker::execution::{ExecutionConfig, execute_claimed},
};
use std::{
    collections::BTreeMap,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    time::Duration,
};

/// A library with one file and a stand-in tool that runs `script` with the system shell.
struct Library {
    _directory: tempfile::TempDir,
    root: PathBuf,
    tool: PathBuf,
    _db: Db,
    conn: Conn,
}

impl Library {
    fn new(script: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("library")).unwrap();
        std::fs::write(root.join("library/film.mp4"), b"not really a film").unwrap();
        let tool = root.join("tool");
        atvprep_testkit::write_script(&tool, format!("#!/bin/sh\n{script}\n"));
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        Self {
            _directory: directory,
            root,
            tool,
            _db: db,
            conn,
        }
    }

    /// Run one item of `action` for the file and return its status and stored log.
    fn run(&self, action: &str) -> (ItemStatus, String) {
        let clock = SystemClock;
        let ctx = Ctx {
            conn: self.conn.raw(),
            clock: &clock,
        };
        let library = self.root.join("library");
        let root = LibraryRoots(ctx)
            .create("Movies", library.to_str().unwrap())
            .unwrap();
        let metadata = std::fs::metadata(library.join("film.mp4")).unwrap();
        let media = Media(ctx)
            .create(
                root.id,
                "film.mp4",
                &MediaMeta {
                    kind: "video".to_owned(),
                    suffix: ".mp4".to_owned(),
                    size: i64::try_from(metadata.len()).unwrap(),
                    mtime_ns: metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec(),
                    fingerprint: "fixture".to_owned(),
                    scanned_device: Some(i64::try_from(metadata.dev()).unwrap()),
                    scanned_inode: Some(i64::try_from(metadata.ino()).unwrap()),
                    scanned_ctime_ns: Some(
                        metadata.ctime() * 1_000_000_000 + metadata.ctime_nsec(),
                    ),
                    ..MediaMeta::default()
                },
            )
            .unwrap();
        let job = Jobs(ctx).create(&NewJob::new(action, &[media.id])).unwrap();
        let mut lease = MediaExecutorLease::new(ctx);
        assert!(lease.acquire("worker", 30).unwrap());
        let fence = lease.fencing_token.unwrap();
        let item = Jobs(ctx)
            .claim_media_item(job.id, "worker", fence, 30, 1)
            .unwrap()
            .unwrap();
        let config = ExecutionConfig {
            executable: self.tool.clone(),
            environment: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            timeout: Duration::from_secs(20),
            grace: Duration::from_millis(100),
            lease_seconds: 30,
            shutdown: None,
            approvals: None,
            session_idle_seconds: 3600,
        };
        let _ = execute_claimed(ctx, item.id, "worker", fence, &config);
        let item = Jobs(ctx).get_item(item.id).unwrap().unwrap();
        (item.status, item.log_tail.unwrap_or_default())
    }
}

/// The step lines of a log, without their `[HH:MM:SS] ` stamp.
fn steps(log: &str) -> Vec<&str> {
    log.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix('[')?;
            let (stamp, text) = rest.split_once("] ")?;
            (stamp.len() == 8 && stamp.as_bytes()[2] == b':').then_some(text)
        })
        .collect()
}

fn has(log: &str, wanted: &str) -> bool {
    steps(log).contains(&wanted)
}

fn starts(log: &str, wanted: &str) -> bool {
    steps(log).iter().any(|line| line.starts_with(wanted))
}

const PROBE: &str = r#"cat <<'EOF'
{"program_version":{"version":"7.1.1","configuration":"--prefix=/opt/private"},
 "streams":[
  {"index":0,"codec_type":"video","codec_name":"h264","disposition":{"default":1,"forced":0}},
  {"index":1,"codec_type":"audio","codec_name":"ac3","channels":6,"sample_rate":"44100","tags":{"language":"ger"},"disposition":{"default":1,"forced":0}},
  {"index":2,"codec_type":"subtitle","codec_name":"mov_text","tags":{"language":"eng"},"disposition":{"default":0,"forced":1}}
 ],
 "format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"5.0"}}
EOF"#;

#[test]
fn an_analysis_logs_start_tool_streams_exit_code_and_duration() {
    let library = Library::new(PROBE);
    let (status, log) = library.run("inspect");
    assert_eq!(status, ItemStatus::Succeeded, "{log}");
    let lines = steps(&log);
    assert_eq!(lines.first(), Some(&"Start: inspect"), "{log}");
    assert!(has(&log, "File: film.mp4"), "{log}");
    assert!(has(&log, "Tool: ffprobe 7.1.1"), "{log}");
    assert!(
        has(
            &log,
            "Command: tool -v error -show_program_version -show_format -show_streams -of json /dev/fd/0"
        ),
        "{log}"
    );
    assert!(has(&log, "Stream 0: video h264, default"), "{log}");
    assert!(
        has(
            &log,
            "Stream 1: audio ac3, 6 channels, 44100 Hz, language ger, default"
        ),
        "{log}"
    );
    assert!(
        has(&log, "Stream 2: subtitle mov_text, language eng, forced"),
        "{log}"
    );
    assert!(has(&log, "Exit code: 0"), "{log}");
    assert!(lines.last().unwrap().starts_with("Duration: "), "{log}");
    assert!(!log.contains("/opt/private"), "{log}");
}

#[test]
fn the_stored_analysis_report_leaves_the_tool_version_out() {
    let library = Library::new(PROBE);
    library.run("inspect");
    let clock = SystemClock;
    let ctx = Ctx {
        conn: library.conn.raw(),
        clock: &clock,
    };
    let media: i64 = ctx
        .conn
        .query_row("SELECT id FROM media_files", [], |row| row.get(0))
        .unwrap();
    let inspection = atvprep_manager::inspections::Inspections(ctx)
        .latest(media)
        .unwrap()
        .unwrap();
    assert!(inspection.result["streams"].is_array());
    assert!(inspection.result.get("program_version").is_none());
}

const VERIFY: &str = r#"cat <<'EOF'
{"files_checked":1,"files_passed":1,"files_failed":0,"total_blockers":0,"total_warnings":1,
 "reports":[{"path":"/dev/fd/0/film.mp4","passed":true,
  "violations":[{"rule":"audio-sample-rate","severity":"warning","message":"ac-3 track id=2 sample rate 44100Hz (expected 48000)","track_id":2}],
  "encode_need":"audio","planned_change":"audio"}]}
EOF"#;

#[test]
fn a_check_names_every_rule_with_its_result_and_the_outcome() {
    let library = Library::new(VERIFY);
    let (status, log) = library.run("verify");
    assert_eq!(status, ItemStatus::Succeeded, "{log}");
    assert_eq!(steps(&log).first(), Some(&"Start: verify"), "{log}");
    assert!(has(&log, "File: film.mp4"), "{log}");
    assert!(starts(&log, "Tool: atvprep "), "{log}");
    assert!(
        starts(&log, "Command: tool verify --json --crf 18 "),
        "{log}"
    );
    assert!(
        has(
            &log,
            "Target profile: crf 18, prefer HEVC 10-bit no, keep AV1 no, fix DTS no"
        ),
        "{log}"
    );
    for rule in [
        "ftyp-dby1-requires-atmos",
        "fast-start",
        "no-fragments",
        "apple-tags",
        "video-reencode",
    ] {
        assert!(has(&log, &format!("Rule {rule}: ok")), "{rule}: {log}");
    }
    assert!(
        has(
            &log,
            "Rule audio-sample-rate: warning (track 2): ac-3 track id=2 sample rate 44100Hz (expected 48000)"
        ),
        "{log}"
    );
    assert!(has(&log, "State: warning"), "{log}");
    assert!(has(&log, "Encode need: audio"), "{log}");
    assert!(has(&log, "Planned change: audio"), "{log}");
    assert!(has(&log, "Exit code: 0"), "{log}");
    assert!(
        steps(&log).last().unwrap().starts_with("Duration: "),
        "{log}"
    );
}

#[test]
fn a_failed_tool_leaves_its_output_and_the_cause_as_the_last_line() {
    let library = Library::new("echo 'probe exploded: token=private-value' >&2; exit 3");
    let (status, log) = library.run("inspect");
    assert_eq!(status, ItemStatus::Failed, "{log}");
    assert!(log.contains("probe exploded"), "{log}");
    assert!(has(&log, "Exit code: 3"), "{log}");
    assert_eq!(
        steps(&log).last(),
        Some(&"Failed: verification_failed: the tool exited with code 3"),
        "{log}"
    );
}

#[test]
fn an_unreadable_report_ends_the_log_with_its_cause() {
    let library = Library::new("echo 'not json'");
    let (status, log) = library.run("inspect");
    assert_eq!(status, ItemStatus::Failed, "{log}");
    assert_eq!(
        steps(&log).last(),
        Some(&"Failed: invalid: inspection returned an invalid report"),
        "{log}"
    );
}

#[test]
fn a_tool_that_cannot_start_ends_the_log_with_its_cause() {
    let library = Library::new("exit 0");
    std::fs::set_permissions(&library.tool, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (status, log) = library.run("verify");
    assert_eq!(status, ItemStatus::Failed, "{log}");
    assert_eq!(steps(&log).first(), Some(&"Start: verify"), "{log}");
    assert_eq!(
        steps(&log).last(),
        Some(&"Failed: invalid: executable is not executable"),
        "{log}"
    );
}
