//! The command line through `run`, with real tools and the committed fixtures.

use std::ffi::OsString;
use std::io::Cursor;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use atvprep::cli::reports_to_json;
use atvprep::{Environment, Io, run};
use atvprep_core::compliance::{Severity, Violation};
use atvprep_core::console::SharedBuffer;
use atvprep_core::verify::VerifyReport;
use atvprep_testkit::{fixture_path, load_golden};
use serde::Deserialize;

struct Outcome {
    code: i32,
    out: String,
    err: String,
}

fn invoke(args: &[&str], stdin: &str, terminal: bool) -> Outcome {
    let (out, err) = (SharedBuffer::default(), SharedBuffer::default());
    let io = Io {
        stdin: Box::new(Cursor::new(stdin.as_bytes().to_vec())),
        stdout: Box::new(out.clone()),
        stderr: Box::new(err.clone()),
        stdout_is_terminal: terminal,
        stdin_is_terminal: false,
    };
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let code = run(&args, &Environment::from_process(), io);
    Outcome {
        code,
        out: out.contents(),
        err: err.contents(),
    }
}

fn copy_fixture(dir: &Path, fixture: &str, name: &str) -> PathBuf {
    let target = dir.join(name);
    std::fs::copy(fixture_path(fixture), &target).unwrap();
    target
}

#[derive(Deserialize)]
struct Report {
    path: String,
    violations: Vec<(String, String, String, Option<u32>)>,
}

#[test]
fn verify_json_is_byte_identical_to_python() {
    let cases = load_golden::<Vec<Report>, String>("cli_verify_json");
    assert_eq!(cases.len(), 5);
    for case in cases {
        let reports: Vec<VerifyReport> = case
            .input
            .into_iter()
            .map(|r| VerifyReport {
                path: PathBuf::from(r.path),
                violations: r
                    .violations
                    .into_iter()
                    .map(|(rule, severity, message, track_id)| Violation {
                        rule,
                        severity: if severity == "blocker" {
                            Severity::Blocker
                        } else {
                            Severity::Warning
                        },
                        message,
                        track_id,
                    })
                    .collect(),
            })
            .collect();
        assert_eq!(reports_to_json(&reports), case.expected, "{}", case.name);
    }
}

#[test]
fn exit_codes_follow_the_policy() {
    let dir = tempfile::tempdir().unwrap();
    let clean = copy_fixture(dir.path(), "plain_faststart.mp4", "clean.mp4");
    let broken = copy_fixture(dir.path(), "syn_fragmented.mp4", "broken.mp4");
    let clean_arg = clean.to_str().unwrap();
    let broken_arg = broken.to_str().unwrap();
    let warned = copy_fixture(dir.path(), "plain_moov_last.mp4", "warned.mp4");
    let warned_arg = warned.to_str().unwrap();
    let table = [
        (vec!["verify", clean_arg], 0),
        (vec!["verify", broken_arg], 1),
        (vec!["verify", warned_arg], 0),
        (vec!["verify", "--fail-on", "warning", warned_arg], 1),
        (vec!["verify", "--fail-on", "warning", clean_arg], 0),
        (vec!["verify", "--scan", clean_arg], 0),
        (vec!["verify"], 2),
        (vec!["verify", "/definitely/not/here"], 2),
    ];
    for (args, want) in table {
        let got = invoke(&args, "", false);
        assert_eq!(got.code, want, "{args:?}\n{}{}", got.out, got.err);
    }
    let empty = tempfile::tempdir().unwrap();
    let got = invoke(&["verify", empty.path().to_str().unwrap()], "", false);
    assert_eq!(got.code, 0);
    assert!(got.out.starts_with("No media files found under:"));
}

#[test]
fn prepare_rejects_flag_combinations_with_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    let a = copy_fixture(dir.path(), "plain_faststart.mp4", "a.mp4");
    let b = copy_fixture(dir.path(), "plain_faststart.mp4", "b.mp4");
    let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());
    let cases: [Vec<&str>; 4] = [
        vec!["prepare", "--delete-original", "-o", "out", a],
        vec!["prepare", "-o", "one.mp4", a, b],
        vec!["prepare", "-o", "out", "--manager-leaf", "a.mp4", a],
        vec!["prepare", "--quarantine", "q", "--manager-leaf", "a.mp4", a],
    ];
    for args in cases {
        let got = invoke(&args, "", false);
        assert_eq!(got.code, 2, "{args:?}\n{}{}", got.out, got.err);
        assert!(got.out.starts_with("Error:"), "{}", got.out);
    }
    let unknown = invoke(&["prepare", "--no-such-flag", a], "", false);
    assert_eq!(unknown.code, 2);
    assert!(!unknown.err.is_empty());
}

#[test]
fn abbreviated_long_options_are_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let a = copy_fixture(dir.path(), "plain_faststart.mp4", "a.mp4");
    let got = invoke(
        &[
            "prepare",
            "--dry",
            "--verb",
            "--prefer",
            a.to_str().unwrap(),
        ],
        "",
        false,
    );
    assert_eq!(got.code, 0, "{}{}", got.out, got.err);
    let got = invoke(
        &["verify", "--js", "--fail", "warning", a.to_str().unwrap()],
        "",
        false,
    );
    assert!(
        got.out.contains("\"files_checked\": 1"),
        "{}{}",
        got.out,
        got.err
    );
}

#[test]
fn fix_and_convert_are_aliases_of_prepare() {
    let dir = tempfile::tempdir().unwrap();
    let a = copy_fixture(dir.path(), "plain_faststart.mp4", "a.mp4");
    let arg = a.to_str().unwrap();
    let outputs: Vec<String> = ["prepare", "fix", "convert"]
        .iter()
        .map(|name| {
            let got = invoke(&[name, "-n", arg], "", false);
            assert_eq!(got.code, 0, "{name}: {}{}", got.out, got.err);
            got.out
        })
        .collect();
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[1], outputs[2]);
}

#[test]
fn machine_lines_appear_only_when_piped() {
    let dir = tempfile::tempdir().unwrap();
    let a = copy_fixture(dir.path(), "plain_faststart.mp4", "a.mp4");
    let piped = invoke(&["prepare", "-y", a.to_str().unwrap()], "", false);
    assert!(
        piped.out.lines().any(|l| l.starts_with("ATVPREP-OUTCOME ")),
        "{}",
        piped.out
    );
    let shown = invoke(&["prepare", "-y", a.to_str().unwrap()], "", true);
    assert!(!shown.out.contains("ATVPREP-"), "{}", shown.out);
}

fn contract(dir: &Path, leaf: &str) -> (std::fs::File, Vec<String>) {
    let parent = std::fs::File::open(dir).unwrap();
    let meta = std::fs::metadata(dir.join(leaf)).unwrap();
    let args = vec![
        "prepare".to_owned(),
        "--manager-parent-fd".to_owned(),
        parent.as_raw_fd().to_string(),
        "--manager-leaf".to_owned(),
        leaf.to_owned(),
        "--manager-device".to_owned(),
        meta.dev().to_string(),
        "--manager-inode".to_owned(),
        meta.ino().to_string(),
        "--manager-ctime-ns".to_owned(),
        (i128::from(meta.ctime()) * 1_000_000_000 + i128::from(meta.ctime_nsec())).to_string(),
        "--copy-only".to_owned(),
        dir.join(leaf).to_string_lossy().into_owned(),
    ];
    (parent, args)
}

#[test]
fn a_manager_contract_runs_the_pinned_leaf() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture(dir.path(), "plain_faststart.mp4", "pinned.mp4");
    let (_parent, args) = contract(dir.path(), "pinned.mp4");
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let got = invoke(&refs, "", false);
    assert!(
        got.out.lines().any(|l| l.starts_with("ATVPREP-OUTCOME ")),
        "{}{}",
        got.out,
        got.err
    );
    assert_ne!(got.code, 2, "{}{}", got.out, got.err);
}

#[test]
fn worker_hands_a_pinned_directory_to_the_real_cli() {
    use atvprep_manager::worker::process::{StopReason, run_pinned_prepare};
    use std::{collections::BTreeMap, time::Duration};
    let directory = tempfile::tempdir().unwrap();
    copy_fixture(directory.path(), "plain_faststart.mp4", "pinned.mp4");
    let target =
        atvprep_core::fd_fix_contract::FdFixTarget::for_path(&directory.path().join("pinned.mp4"))
            .unwrap();
    let environment =
        BTreeMap::from([("PATH".to_owned(), std::env::var("PATH").unwrap_or_default())]);
    let result = run_pinned_prepare(
        Path::new(env!("CARGO_BIN_EXE_atvprep")),
        &target,
        &environment,
        Duration::from_secs(30),
        Duration::from_millis(100),
        || false,
    )
    .unwrap();
    assert_eq!(result.reason, StopReason::Exited);
    let output = String::from_utf8_lossy(&result.output);
    assert!(output.contains("ATVPREP-OUTCOME "), "{output}");
    assert_ne!(result.exit_code, Some(2), "{output}");
}

#[test]
#[allow(clippy::too_many_lines)] // One real-media fixture covers verify, start failure and probing.
fn worker_executes_a_claimed_verification_and_releases_its_permit() {
    use atvprep_manager::{
        clock::SystemClock,
        db::Db,
        jobs::{Jobs, NewJob},
        leases::MediaExecutorLease,
        library::LibraryRoots,
        media::Media,
        models::MediaMeta,
        repo::Ctx,
        schema,
        worker::execution::{ExecutionConfig, execute_claimed},
    };
    use std::{collections::BTreeMap, time::Duration};
    let directory = tempfile::tempdir().unwrap();
    copy_fixture(directory.path(), "plain_faststart.mp4", "verify.mp4");
    let root_path = directory.path().canonicalize().unwrap();
    let metadata = std::fs::metadata(root_path.join("verify.mp4")).unwrap();
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
    let media = Media(ctx)
        .create(
            root.id,
            "verify.mp4",
            &MediaMeta {
                kind: "video".to_owned(),
                suffix: ".mp4".to_owned(),
                size: i64::try_from(metadata.len()).unwrap(),
                mtime_ns: metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec(),
                fingerprint: "fixture".to_owned(),
                scanned_device: Some(i64::try_from(metadata.dev()).unwrap()),
                scanned_inode: Some(i64::try_from(metadata.ino()).unwrap()),
                scanned_ctime_ns: Some(metadata.ctime() * 1_000_000_000 + metadata.ctime_nsec()),
                ..MediaMeta::default()
            },
        )
        .unwrap();
    let job = Jobs(ctx)
        .create(&NewJob::new("verify", &[media.id]))
        .unwrap();
    let mut lease = MediaExecutorLease::new(ctx);
    assert!(lease.acquire("worker", 30).unwrap());
    let fence = lease.fencing_token.unwrap();
    let item = Jobs(ctx)
        .claim_media_item(job.id, "worker", fence, 30, 1)
        .unwrap()
        .unwrap();
    let config = ExecutionConfig {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_atvprep")),
        environment: BTreeMap::from([(
            "PATH".to_owned(),
            std::env::var("PATH").unwrap_or_default(),
        )]),
        timeout: Duration::from_secs(20),
        grace: Duration::from_millis(100),
        lease_seconds: 30,
        shutdown: None,
        approvals: None,
        session_idle_seconds: 3600,
    };
    let result = execute_claimed(ctx, item.id, "worker", fence, &config).unwrap();
    assert!(
        serde_json::from_slice::<serde_json::Value>(&result.output).is_ok(),
        "{}",
        String::from_utf8_lossy(&result.output)
    );
    let item = Jobs(ctx).get_item(item.id).unwrap().unwrap();
    assert!(matches!(item.status.as_str(), "succeeded" | "failed"));
    let permits: i64 = conn
        .raw()
        .query_row(
            "SELECT count(*) FROM deployment_permits WHERE released_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(permits, 0);
    let job = Jobs(ctx)
        .create(&NewJob::new("verify", &[media.id]))
        .unwrap();
    let item = Jobs(ctx)
        .claim_media_item(job.id, "worker", fence, 30, 1)
        .unwrap()
        .unwrap();
    let missing = ExecutionConfig {
        executable: root_path.join("missing-executable"),
        ..config
    };
    assert!(execute_claimed(ctx, item.id, "worker", fence, &missing).is_err());
    assert_eq!(
        Jobs(ctx).get_item(item.id).unwrap().unwrap().status,
        "failed"
    );
    let executable = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("ffprobe"))
        .find(|path| path.is_file())
        .expect("ffprobe is required for media integration tests")
        .canonicalize()
        .unwrap();
    let probe = ExecutionConfig {
        executable,
        ..missing
    };
    let job = Jobs(ctx)
        .create(&NewJob::new("inspect", &[media.id]))
        .unwrap();
    let item = Jobs(ctx)
        .claim_media_item(job.id, "worker", fence, 30, 1)
        .unwrap()
        .unwrap();
    let report = execute_claimed(ctx, item.id, "worker", fence, &probe).unwrap();
    assert_eq!(report.exit_code, Some(0));
    let inspection = atvprep_manager::inspections::Inspections(ctx)
        .latest(media.id)
        .unwrap()
        .unwrap();
    assert_eq!(inspection.tool_name, "ffprobe");
    assert!(inspection.result["streams"].is_array());
    assert!(lease.release().unwrap());
    let runtime = atvprep_manager::worker::runtime::RuntimeConfig {
        owner: "runtime".to_owned(),
        execution: ExecutionConfig {
            executable: PathBuf::from(env!("CARGO_BIN_EXE_atvprep")),
            ..probe.clone()
        },
        ffprobe_executable: probe.executable.clone(),
        scan: atvprep_manager::worker::scan_service::ScanOptions {
            follow_mounts: false,
            excluded: vec![],
            settle_seconds: 0,
            lease_seconds: 30,
            shutdown: None,
        },
        scan_interval_seconds: 3600,
        maintenance: atvprep_manager::worker::service::WorkerLimits::default(),
    };
    let runtime_clock: std::sync::Arc<dyn atvprep_manager::clock::Clock> =
        std::sync::Arc::new(SystemClock);
    let handle = atvprep_manager::worker::runtime::start(&db, &runtime_clock, &runtime).unwrap();
    let started = std::time::Instant::now();
    loop {
        let complete:bool=ctx.conn.query_row("SELECT EXISTS(SELECT 1 FROM scan_runs WHERE status='succeeded') AND EXISTS(SELECT 1 FROM inspections n JOIN media_files m ON n.media_file_id=m.id WHERE n.tool_name='ffprobe' AND n.input_fingerprint=m.fingerprint AND m.fingerprint!='fixture')",[],|row|row.get(0)).unwrap_or(false);
        if complete && handle.healthy() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "worker runtime did not finish scan and probe: scans={:?}, items={:?}, healthy={}",
            ctx.conn
                .query_row("SELECT group_concat(status) FROM scan_runs", [], |row| {
                    row.get::<_, Option<String>>(0)
                }),
            ctx.conn.query_row(
                "SELECT group_concat(status||':'||COALESCE(error_code,'')) FROM job_items",
                [],
                |row| row.get::<_, Option<String>>(0)
            ),
            handle.healthy()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let shutdown = std::time::Instant::now();
    drop(handle);
    assert!(shutdown.elapsed() < Duration::from_secs(3));
    std::fs::write(
        root_path.join("verify.en.srt"),
        "1\n00:00:00,000 --> 00:00:01,000\nPinned subtitle\n",
    )
    .unwrap();
    let observed = atvprep_manager::worker::scanner::observe(
        &root_path,
        "verify.mp4",
        0,
        ctx.clock.now_micros(),
    )
    .unwrap();
    Media(ctx)
        .upsert(root.id, "verify.mp4", &observed.meta, None)
        .unwrap();
    for sidecar in &observed.sidecars {
        atvprep_manager::sidecars::Sidecars(ctx)
            .upsert(
                media.id,
                &sidecar.relative_path,
                &atvprep_manager::worker::scanner::fingerprint(sidecar.size, sidecar.mtime_ns),
                sidecar.size,
                sidecar.mtime_ns,
                None,
            )
            .unwrap();
    }
    let auth = atvprep_api::auth::AuthService::with_csrf_ttl(ctx, 28_800, 3600, 900).unwrap();
    let session = auth.create().unwrap();
    let ids = [media.id];
    let mut prepare = NewJob::new("fix", &ids);
    prepare.trigger = "manual";
    prepare.settings_snapshot = serde_json::json!({"pipeline":"prepare","policy":"copy-only-v1","manual_approval":true,"approved_by":session.id,"approved_at":ctx.clock.now_micros()/1_000_000,"delete_original":false,"delete_original_confirmed":false});
    let job = Jobs(ctx).create(&prepare).unwrap();
    let queued = Jobs(ctx).items(job.id).unwrap().remove(0);
    let recorded = atvprep_manager::snapshots::InputSnapshots(ctx)
        .get_for_item(queued.id)
        .unwrap()
        .unwrap();
    let identity = [
        recorded["file_device"].clone(),
        recorded["file_inode"].clone(),
        recorded["file_ctime_ns"].clone(),
        recorded["sidecar_fingerprint"].clone(),
    ];
    let verifier = atvprep_manager::approval::ApprovalVerifier::new(b"approval-secret").unwrap();
    let mut signed = prepare.settings_snapshot.clone();
    signed["approval_mac"] = serde_json::json!(verifier.sign(
        &atvprep_manager::approval::ApprovalItem {
            job_id: job.id,
            media_id: media.id,
            action: "fix",
            identity: &identity
        },
        session.id,
        signed["approved_at"].as_i64().unwrap()
    ));
    ctx.conn
        .execute(
            "UPDATE job_items SET settings_snapshot_json=?1 WHERE id=?2",
            rusqlite::params![signed.to_string(), queued.id],
        )
        .unwrap();
    let mut lease = MediaExecutorLease::new(ctx);
    assert!(lease.acquire("prepare-worker", 30).unwrap());
    let fence = lease.fencing_token.unwrap();
    let item = Jobs(ctx)
        .claim_media_item(job.id, "prepare-worker", fence, 30, 1)
        .unwrap()
        .unwrap();
    let execution = ExecutionConfig {
        approvals: Some(std::sync::Arc::new(verifier)),
        ..runtime.execution.clone()
    };
    ctx.conn
        .execute(
            "UPDATE auth_sessions SET revoked_at=?1 WHERE id=?2",
            rusqlite::params![ctx.clock.now(), session.id],
        )
        .unwrap();
    assert!(execute_claimed(ctx, item.id, "prepare-worker", fence, &execution).is_err());
    assert!(
        Jobs(ctx)
            .get_item(item.id)
            .unwrap()
            .unwrap()
            .started_at
            .is_none()
    );
    ctx.conn
        .execute(
            "UPDATE auth_sessions SET revoked_at=NULL WHERE id=?1",
            [session.id],
        )
        .unwrap();
    let prepared = execute_claimed(ctx, item.id, "prepare-worker", fence, &execution).unwrap();
    assert_eq!(
        prepared.exit_code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&prepared.output)
    );
    assert_eq!(
        Jobs(ctx).get_item(item.id).unwrap().unwrap().status,
        "succeeded"
    );
    assert!(String::from_utf8_lossy(&prepared.output).contains("ATVPREP-OUTCOME "));
    let tail: String = ctx
        .conn
        .query_row(
            "SELECT log_tail FROM job_items WHERE id=?1",
            [item.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(tail.contains("ATVPREP-OUTCOME "));
    assert!(tail.len() <= 8192);
    assert!(
        ctx.conn
            .query_row(
                "SELECT count(*) FROM events WHERE item_id=?1 AND topic='job.log'",
                [item.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap()
            > 0
    );
    assert!(root_path.join("verify.mp4").is_file());
    let followup: i64 = ctx
        .conn
        .query_row(
            "SELECT id FROM jobs WHERE action='verify' AND id>?1 ORDER BY id LIMIT 1",
            [job.id],
            |row| row.get(0),
        )
        .unwrap();
    let verify = Jobs(ctx)
        .claim_media_item(followup, "prepare-worker", fence, 30, 1)
        .unwrap()
        .unwrap();
    let completed = execute_claimed(ctx, verify.id, "prepare-worker", fence, &execution).unwrap();
    assert_eq!(
        completed.exit_code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&completed.output)
    );
}

#[test]
fn an_incomplete_manager_contract_fails() {
    let dir = tempfile::tempdir().unwrap();
    let a = copy_fixture(dir.path(), "plain_faststart.mp4", "a.mp4");
    let got = invoke(
        &["prepare", "--manager-leaf", "a.mp4", a.to_str().unwrap()],
        "",
        false,
    );
    assert_eq!(got.code, 1);
    assert_eq!(got.out.trim(), "Error: incomplete manager fix contract");
}

#[test]
fn deploy_gate_requires_its_action_specific_arguments() {
    let cases = [
        vec!["deploy", "gate", "status"],
        vec!["deploy", "gate", "close", "--database", "manager.sqlite"],
        vec!["deploy", "gate", "open", "--database", "manager.sqlite"],
        vec![
            "deploy",
            "gate",
            "begin-migration",
            "--database",
            "manager.sqlite",
        ],
        vec![
            "deploy",
            "gate",
            "finish-migration",
            "--database",
            "manager.sqlite",
        ],
    ];
    for args in cases {
        let got = invoke(&args, "", false);
        assert_eq!(got.code, 2, "{args:?}\n{}{}", got.out, got.err);
        assert!(!got.err.is_empty(), "{args:?}");
    }
}

#[test]
fn deploy_gate_never_creates_a_missing_database() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("missing.sqlite");
    let got = invoke(
        &[
            "deploy",
            "gate",
            "status",
            "--database",
            database.to_str().unwrap(),
        ],
        "",
        false,
    );
    assert_eq!(got.code, 1, "{}{}", got.out, got.err);
    assert!(got.out.is_empty());
    assert!(
        got.err.contains("database file does not exist"),
        "{}",
        got.err
    );
    assert!(!database.exists());
}

#[test]
fn deploy_gate_is_exposed_in_help() {
    let got = invoke(&["deploy", "gate", "--help"], "", false);
    assert_eq!(got.code, 0, "{}{}", got.out, got.err);
    assert!(got.out.contains("begin-migration"), "{}", got.out);
    assert!(got.out.contains("--generation"), "{}", got.out);
}

#[test]
fn without_a_command_the_help_is_printed_and_the_exit_code_is_1() {
    let got = invoke(&[], "", false);
    assert_eq!(got.code, 1);
    assert!(got.out.contains("Usage:"), "{}", got.out);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_atvprep"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
}
