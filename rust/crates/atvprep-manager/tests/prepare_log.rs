//! What Vorbereiten leaves in an item's log, run against a stand-in for the media CLI.

use atvprep_core::encode::EncodeAllowance;
use atvprep_manager::{
    approval::{ApprovalVerifier, ScheduleParams},
    clock::{Clock, SystemClock, ZonedClock},
    db::{Conn, Db},
    jobs::Jobs,
    leases::MediaExecutorLease,
    library::LibraryRoots,
    media::Media,
    models::MediaMeta,
    repo::Ctx,
    schedules::run_schedule,
    schema,
    settings::TargetProfile,
    state_machine::ItemStatus,
    worker::execution::{ExecutionConfig, execute_claimed},
};
use std::{
    collections::BTreeMap,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    time::Duration,
};

const SECRET: &[u8] = b"secret";

/// A library with one file, a signed Vorbereiten schedule over it and a stand-in CLI that runs
/// `script` with the system shell.
struct Library {
    _directory: tempfile::TempDir,
    tool: PathBuf,
    _db: Db,
    conn: Conn,
}

impl Library {
    fn new(script: &str, delete_original: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("library")).unwrap();
        let library = root.join("library");
        std::fs::write(library.join("film.mp4"), b"not really a film").unwrap();
        let tool = root.join("tool");
        std::fs::write(&tool, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = SystemClock;
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let library_root = LibraryRoots(ctx)
            .create("Movies", library.to_str().unwrap())
            .unwrap();
        let metadata = std::fs::metadata(library.join("film.mp4")).unwrap();
        Media(ctx)
            .create(
                library_root.id,
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
        ctx.conn
            .execute(
                "UPDATE media_files SET compliance_state='warning', encode_need='none'",
                [],
            )
            .unwrap();
        ctx.conn
            .execute(
                "INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,max_files,enabled,armed_at) VALUES(1,'s','prepare',?1,127,'03:00',5,1,'2000-01-01')",
                [library_root.id],
            )
            .unwrap();
        if delete_original {
            ctx.conn
                .execute(
                    "INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('delete_original','true','bool',1)",
                    [],
                )
                .unwrap();
        }
        let params = ScheduleParams {
            schedule_id: 1,
            action: "prepare",
            root_id: Some(library_root.id),
            compliance_state: None,
            max_files: 5,
            delete_original,
            encode: EncodeAllowance::Copy,
            profile: TargetProfile::default(),
        };
        let mac = ApprovalVerifier::new(SECRET)
            .unwrap()
            .sign_schedule(&params, 1, 1_700_000_000);
        ctx.conn
            .execute(
                "UPDATE schedules SET approved_by=1, approved_at=1700000000, approval_mac=?1",
                [mac],
            )
            .unwrap();
        assert_eq!(run_schedule(ctx, 1).unwrap(), "Queued 1 file");
        Self {
            _directory: directory,
            tool,
            _db: db,
            conn,
        }
    }

    /// Run the queued Vorbereiten item and return its status and stored log.
    fn run(&self) -> (ItemStatus, String) {
        self.run_on(&SystemClock)
    }

    /// [`Self::run`] with the time and the server timezone taken from `clock`.
    fn run_on(&self, clock: &dyn Clock) -> (ItemStatus, String) {
        let ctx = Ctx {
            conn: self.conn.raw(),
            clock,
        };
        let job: i64 = ctx
            .conn
            .query_row("SELECT id FROM jobs ORDER BY id DESC LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let mut lease = MediaExecutorLease::new(ctx);
        assert!(lease.acquire("worker", 30).unwrap());
        let fence = lease.fencing_token.unwrap();
        let item = Jobs(ctx)
            .claim_media_item(job, "worker", fence, 30, 1)
            .unwrap()
            .unwrap();
        let config = ExecutionConfig {
            executable: self.tool.clone(),
            environment: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            timeout: Duration::from_secs(20),
            grace: Duration::from_millis(100),
            lease_seconds: 30,
            shutdown: None,
            approvals: ApprovalVerifier::new(SECRET).map(std::sync::Arc::new),
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

/// Where `wanted` stands among the step lines.
fn position(log: &str, wanted: &str) -> usize {
    steps(log)
        .iter()
        .position(|line| *line == wanted)
        .unwrap_or_else(|| panic!("no step line {wanted:?} in:\n{log}"))
}

fn position_starting(log: &str, wanted: &str) -> usize {
    steps(log)
        .iter()
        .position(|line| line.starts_with(wanted))
        .unwrap_or_else(|| panic!("no step line starting {wanted:?} in:\n{log}"))
}

#[test]
fn a_preparation_logs_its_allowance_profile_and_delete_original_after_the_command() {
    let library = Library::new("echo 'ATVPREP-OUTCOME done'", false);
    let (status, log) = library.run();
    assert_eq!(status, ItemStatus::Succeeded, "{log}");
    let command = position_starting(&log, "Command: tool prepare ");
    let exit = position(&log, "Exit code: 0");
    for frame in [
        "Encode allowance: copy",
        "Target profile: crf 18, prefer HEVC 10-bit no, keep AV1 no, fix DTS no",
        "Delete original: no",
    ] {
        let at = position(&log, frame);
        assert!(command < at && at < exit, "{frame}: {log}");
    }
}

#[test]
fn a_preparation_logs_the_plan_of_every_stream_as_the_cli_reports_it() {
    // The CLI's machine lines come on stdout; stderr output may land inside one of them.
    let library = Library::new(
        "printf 'ATVPREP-PLAN stream 0 (video h264): copy\\nATVPREP-PLAN stream 1 (audio ac3): enc'
sleep 0.2; printf 'ffmpeg chatter\\n' >&2; sleep 0.2
printf 'ode eac3 at 48000 Hz\\nATVPREP-PLAN stream 2 (data bin_data): drop\\n'
echo 'ATVPREP-OUTCOME rebuilt'",
        false,
    );
    let (status, log) = library.run();
    assert_eq!(status, ItemStatus::Succeeded, "{log}");
    let command = position_starting(&log, "Command: tool prepare ");
    let exit = position(&log, "Exit code: 0");
    let mut last = command;
    for plan in [
        "Plan stream 0 (video h264): copy",
        "Plan stream 1 (audio ac3): encode eac3 at 48000 Hz",
        "Plan stream 2 (data bin_data): drop",
    ] {
        let at = position(&log, plan);
        assert!(last < at && at < exit, "{plan}: {log}");
        last = at;
    }
}

#[test]
fn a_preparation_logs_every_phase_with_its_start_and_duration() {
    // Only with the phase timing switched on does the CLI report its phases.
    let library = Library::new(
        r#"if [ "$ATVPREP_TRACE_PHASES" = 1 ]; then
  echo '{"phase":"ffmpeg","seconds":2.5,"bytes":10}' >&2
  echo '{"phase":"patch.moov","seconds":0.25}' >&2
  echo '{"phase":"verify","seconds":0.1}' >&2
fi
echo 'ATVPREP-OUTCOME rebuilt'"#,
        false,
    );
    let (status, log) = library.run();
    assert_eq!(status, ItemStatus::Succeeded, "{log}");
    let exit = position(&log, "Exit code: 0");
    let mut last = position_starting(&log, "Command: tool prepare ");
    for (name, took) in [
        ("ffmpeg", "2.50 s"),
        ("patch.moov", "0.25 s"),
        ("verify", "0.10 s"),
    ] {
        let at = position_starting(&log, &format!("Phase {name}: started "));
        let line = steps(&log)[at];
        assert!(line.ends_with(&format!(", took {took}")), "{line}");
        let started = line
            .strip_prefix(&format!("Phase {name}: started "))
            .unwrap()
            .split(',')
            .next()
            .unwrap();
        assert!(
            started.len() == 8 && started.as_bytes()[2] == b':' && started.as_bytes()[5] == b':',
            "{line}"
        );
        assert!(last < at && at < exit, "{name}: {log}");
        last = at;
    }
}

#[test]
fn step_lines_and_phase_starts_read_in_server_time() {
    let library = Library::new(
        r#"if [ "$ATVPREP_TRACE_PHASES" = 1 ]; then
  echo '{"phase":"ffmpeg","seconds":2.5}' >&2
fi
echo 'ATVPREP-OUTCOME rebuilt'"#,
        false,
    );
    // 2023-11-14 22:13:20 UTC is 2023-11-15 03:43:20 in Kolkata (UTC+05:30).
    let clock = ZonedClock {
        micros: 1_700_000_000_000_000,
        zone: chrono_tz::Asia::Kolkata,
    };
    let (status, log) = library.run_on(&clock);
    assert_eq!(status, ItemStatus::Succeeded, "{log}");
    let stamped: Vec<&str> = log.lines().filter(|line| line.starts_with('[')).collect();
    assert!(stamped.len() > 3, "{log}");
    for line in stamped {
        assert!(line.starts_with("[03:43:20] "), "{line}");
    }
    position(&log, "Phase ffmpeg: started 03:43:17, took 2.50 s");
}

#[test]
fn a_preparation_logs_the_check_after_it_and_the_removal_of_the_original() {
    let library = Library::new(
        "echo 'ATVPREP-VERIFY passed, 0 blockers, 1 warning'
echo 'ATVPREP-OUTCOME rebuilt'
echo 'ATVPREP-SOURCE removed'",
        true,
    );
    let (status, log) = library.run();
    assert_eq!(status, ItemStatus::Succeeded, "{log}");
    let frame = position(&log, "Delete original: yes");
    let checked = position(&log, "Check after prepare: passed, 0 blockers, 1 warning");
    let removed = position(&log, "Original: removed");
    let exit = position(&log, "Exit code: 0");
    assert!(
        frame < checked && checked < removed && removed < exit,
        "{log}"
    );
}

#[test]
fn a_kept_original_is_logged_with_the_reason() {
    let library = Library::new(
        "echo 'ATVPREP-VERIFY failed, 1 blocker, 0 warnings'
echo 'ATVPREP-OUTCOME failed'
echo 'ATVPREP-SOURCE kept verify failed'
exit 1",
        true,
    );
    let (_, log) = library.run();
    position(&log, "Check after prepare: failed, 1 blocker, 0 warnings");
    position(&log, "Original: kept (verify failed)");
}

#[test]
fn a_failed_preparation_ends_with_the_error_the_cli_reported() {
    let library = Library::new(
        "echo 'Error processing film.mp4: ffmpeg wrote no output for film.mp4'
echo 'ATVPREP-ERROR ffmpeg wrote no output for film.mp4'
echo 'ATVPREP-OUTCOME failed'
exit 1",
        false,
    );
    let (status, log) = library.run();
    assert_eq!(status, ItemStatus::Failed, "{log}");
    assert_eq!(
        steps(&log).last(),
        Some(&"Failed: verification_failed: ffmpeg wrote no output for film.mp4"),
        "{log}"
    );
}

#[test]
fn a_failed_preparation_without_a_reported_error_names_the_exit_code() {
    let library = Library::new("exit 2", false);
    let (status, log) = library.run();
    assert_eq!(status, ItemStatus::Failed, "{log}");
    assert_eq!(
        steps(&log).last(),
        Some(&"Failed: verification_failed: the tool exited with code 2"),
        "{log}"
    );
}

#[test]
fn long_tool_output_loses_its_middle_and_every_step_line_stays_whole() {
    let library = Library::new(
        "echo 'ATVPREP-PLAN stream 0 (video hevc): encode hevc'
i=0; while [ $i -lt 4000 ]; do echo \"frame=$i fps=24 q=28.0 size=1024kB time=00:00:01.00 bitrate=800kbits/s speed=1x\" >&2; i=$((i+1)); done
echo 'ATVPREP-VERIFY passed, 0 blockers, 0 warnings'
echo 'ATVPREP-OUTCOME reencoded'",
        false,
    );
    let (status, log) = library.run();
    assert_eq!(status, ItemStatus::Succeeded, "{log}");
    assert!(log.len() <= 64 * 1024, "{}", log.len());
    assert!(log.contains("bytes omitted"), "{log}");
    assert!(
        log.contains("frame=0 "),
        "the beginning of the output stays"
    );
    assert!(log.contains("frame=3999 "), "the end of the output stays");
    let lines = steps(&log);
    assert_eq!(lines.first(), Some(&"Start: prepare"), "{log}");
    for step in [
        "Encode allowance: copy",
        "Plan stream 0 (video hevc): encode hevc",
        "Check after prepare: passed, 0 blockers, 0 warnings",
        "Exit code: 0",
    ] {
        position(&log, step);
    }
    assert!(lines.last().unwrap().starts_with("Duration: "), "{log}");
}

#[test]
fn a_preparation_above_its_allowance_ends_with_the_refusal() {
    let library = Library::new(
        "echo 'ATVPREP-ENCODE need=audio allowance=copy'
echo 'ATVPREP-OUTCOME encode_not_allowed'",
        false,
    );
    let (status, log) = library.run();
    assert_eq!(status, ItemStatus::Succeeded, "{log}");
    assert_eq!(
        steps(&log).last(),
        Some(&"Encoding not allowed (need=audio allowance=copy): the file stays as it was."),
        "{log}"
    );
    assert!(
        position(&log, "Exit code: 0") < steps(&log).len() - 1,
        "{log}"
    );
}
