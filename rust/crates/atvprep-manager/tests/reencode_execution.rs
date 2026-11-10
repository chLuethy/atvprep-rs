//! ADR 0007: an approved item re-encodes, every other item only copies. The worker starts the media
//! CLI with a fake executable that records its arguments, so the `--encode` level is
//! observed without needing ffmpeg here (the real run is covered in `atvprep/tests/cli.rs`).
use atvprep_manager::{
    approval::{ApprovalItem, ApprovalVerifier},
    clock::{Clock, SystemClock, format_micros},
    db::{Conn, Db},
    jobs::{Jobs, NewJob},
    leases::MediaExecutorLease,
    library::LibraryRoots,
    media::Media,
    repo::Ctx,
    schema,
    snapshots::InputSnapshots,
    state_machine::ItemStatus,
    worker::execution::{
        ExecutionConfig, REENCODE_NOT_RUN, REENCODE_TIMEOUT, execute_claimed, prepare_timeout,
    },
    worker::process::PrepareMode,
};
use serde_json::json;
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt, time::Duration};

struct World {
    dir: tempfile::TempDir,
    conn: Conn,
    media: Vec<i64>,
    job: i64,
    fence: i64,
}

/// One manual, signed job over `copy.mp4` and `encode.mp4`. `encode.mp4` is approved to re-encode
/// when `reencode`; `delete_original` is recorded on every item and signed with it.
fn world(reencode: bool, delete_original: bool) -> World {
    let dir = tempfile::tempdir().unwrap();
    let root_path = dir.path().canonicalize().unwrap();
    for name in ["copy.mp4", "encode.mp4"] {
        std::fs::write(root_path.join(name), name.as_bytes()).unwrap();
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
    for name in ["copy.mp4", "encode.mp4"] {
        let observed =
            atvprep_manager::worker::scanner::observe(&root_path, name, 0, clock.now_micros())
                .unwrap();
        media.push(Media(ctx).create(root.id, name, &observed.meta).unwrap().id);
    }
    ctx.conn
        .execute(
            "UPDATE media_files SET compliance_state='warning', needs_reencode=(id=?1)",
            [media[1]],
        )
        .unwrap();
    ctx.conn
        .execute(
            "INSERT INTO auth_sessions(id,session_token_hash,csrf_secret_hash,expires_at,last_seen_at) VALUES(1,'t','c',?1,?2)",
            rusqlite::params![format_micros(clock.now_micros() + 600_000_000), clock.now()],
        )
        .unwrap();
    let mut new = NewJob::new("fix", &media);
    new.trigger = "manual";
    new.settings_snapshot = json!({"pipeline":"prepare","policy":"copy-only-v2","delete_original":delete_original,
        "manual_approval":true,"approved_by":1,"approved_at":clock.now_micros()/1_000_000});
    if reencode {
        new.reencode_media.insert(media[1]);
    }
    let job = Jobs(ctx).create(&new).unwrap();
    let verifier = ApprovalVerifier::new(b"secret").unwrap();
    for mut item in Jobs(ctx).items(job.id).unwrap() {
        let recorded = InputSnapshots(ctx).get_for_item(item.id).unwrap().unwrap();
        let identity = [
            recorded["file_device"].clone(),
            recorded["file_inode"].clone(),
            recorded["file_ctime_ns"].clone(),
            recorded["sidecar_fingerprint"].clone(),
        ];
        let mac = verifier.sign(
            &ApprovalItem {
                job_id: job.id,
                media_id: item.media_file_id,
                action: "fix",
                identity: &identity,
                reencode: item.settings_snapshot["reencode"] == json!(true),
                delete_original,
            },
            1,
            clock.now_micros() / 1_000_000,
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
    World {
        dir,
        conn,
        media,
        job: job.id,
        fence,
    }
}

/// A stand-in for `atvprep`: records its arguments and prints `output`.
fn fake_cli(dir: &std::path::Path, output: &str) -> ExecutionConfig {
    let script = dir.join("fake-atvprep");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGS_FILE\"\nprintf '%s\\n' '{output}'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    ExecutionConfig {
        executable: script,
        environment: BTreeMap::from([(
            "ARGS_FILE".to_owned(),
            dir.join("args.txt").to_string_lossy().into_owned(),
        )]),
        timeout: Duration::from_secs(20),
        grace: Duration::from_millis(200),
        lease_seconds: 600,
        shutdown: None,
        approvals: ApprovalVerifier::new(b"secret").map(std::sync::Arc::new),
        session_idle_seconds: 3600,
    }
}

/// Claim and run the item of `media`, returning the arguments the media CLI was started with.
fn run(
    world: &World,
    media: i64,
    config: &ExecutionConfig,
) -> (Vec<String>, ItemStatus, Option<String>) {
    let clock = SystemClock;
    let ctx = Ctx {
        conn: world.conn.raw(),
        clock: &clock,
    };
    let item = loop {
        let item = Jobs(ctx)
            .claim_media_item(world.job, "worker", world.fence, 600, 1)
            .unwrap()
            .unwrap();
        if item.media_file_id == media {
            break item;
        }
        // Not the one asked for: end it so that the next claim moves on.
        ctx.conn
            .execute(
                "UPDATE job_items SET status='cancelled' WHERE id=?1",
                [item.id],
            )
            .unwrap();
    };
    let _ = std::fs::remove_file(world.dir.path().join("args.txt"));
    execute_claimed(ctx, item.id, "worker", world.fence, config).unwrap();
    let ended = Jobs(ctx).get_item(item.id).unwrap().unwrap();
    let arguments = std::fs::read_to_string(world.dir.path().join("args.txt"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    (arguments, ended.status, ended.error_code)
}

fn has(arguments: &[String], flag: &str) -> bool {
    arguments.iter().any(|a| a == flag)
}

fn encode_level(arguments: &[String]) -> &str {
    let at = arguments
        .iter()
        .position(|a| a == "--encode")
        .unwrap_or_else(|| panic!("no --encode in {arguments:?}"));
    &arguments[at + 1]
}

/// What the real CLI prints for a file it refuses (see `atvprep/tests/encode_allowance.rs`).
const REFUSED: &str = "ATVPREP-OUTCOME encode_not_allowed";

#[test]
fn an_approved_re_encode_with_delete_original_may_encode_everything() {
    let world = world(true, true);
    let config = fake_cli(world.dir.path(), "ATVPREP-OUTCOME reencoded");
    let (arguments, status, code) = run(&world, world.media[1], &config);
    assert_eq!(encode_level(&arguments), "all", "{arguments:?}");
    assert!(has(&arguments, "--delete-original"), "{arguments:?}");
    assert_eq!(status, ItemStatus::Succeeded);
    assert_eq!(code, None);
}

#[test]
fn an_item_not_approved_to_re_encode_only_copies_in_a_mixed_job() {
    let world = world(true, true);
    let config = fake_cli(world.dir.path(), "ATVPREP-OUTCOME patched");
    // The first item in the job is the copy-only file; it must not inherit the other's mode.
    let (arguments, _, _) = run(&world, world.media[0], &config);
    assert_eq!(encode_level(&arguments), "copy", "{arguments:?}");
    assert!(!has(&arguments, "--delete-original"), "{arguments:?}");
}

#[test]
fn a_re_encode_without_delete_original_ends_as_needed_but_not_run() {
    let world = world(true, false);
    let config = fake_cli(world.dir.path(), REFUSED);
    let (arguments, status, code) = run(&world, world.media[1], &config);
    assert_eq!(encode_level(&arguments), "audio", "{arguments:?}");
    assert!(!has(&arguments, "--delete-original"), "{arguments:?}");
    assert_eq!(status, ItemStatus::Succeeded);
    assert_eq!(code.as_deref(), Some(REENCODE_NOT_RUN));
    let clock = SystemClock;
    let ctx = Ctx {
        conn: world.conn.raw(),
        clock: &clock,
    };
    let detail: String = ctx
        .conn
        .query_row(
            "SELECT error_detail FROM job_items WHERE error_code=?1",
            [REENCODE_NOT_RUN],
            |row| row.get(0),
        )
        .unwrap();
    assert!(detail.contains("delete_original"), "{detail}");
}

#[test]
fn a_file_that_was_not_approved_to_encode_fails_as_it_did_in_0_5_7() {
    let world = world(false, false);
    let config = fake_cli(world.dir.path(), REFUSED);
    let (arguments, status, code) = run(&world, world.media[1], &config);
    assert_eq!(encode_level(&arguments), "copy", "{arguments:?}");
    assert_eq!(status, ItemStatus::Failed);
    assert_eq!(code.as_deref(), Some("verification_failed"));
}

#[test]
fn only_a_re_encode_item_gets_the_four_hour_limit() {
    let configured = Duration::from_secs(600);
    assert_eq!(REENCODE_TIMEOUT, Duration::from_hours(4));
    assert_eq!(
        prepare_timeout(configured, PrepareMode::default()),
        configured
    );
    assert_eq!(
        prepare_timeout(
            configured,
            PrepareMode {
                reencode: false,
                delete_original: true
            }
        ),
        configured
    );
    assert_eq!(
        prepare_timeout(
            configured,
            PrepareMode {
                reencode: true,
                delete_original: false
            }
        ),
        REENCODE_TIMEOUT
    );
    // A longer configured limit is never shortened.
    let long = Duration::from_hours(6);
    assert_eq!(
        prepare_timeout(
            long,
            PrepareMode {
                reencode: true,
                delete_original: true
            }
        ),
        long
    );
}

#[test]
fn a_job_queued_under_copy_only_v1_is_refused() {
    let world = world(false, false);
    let clock = SystemClock;
    let ctx = Ctx {
        conn: world.conn.raw(),
        clock: &clock,
    };
    ctx.conn
        .execute(
            "UPDATE job_items SET settings_snapshot_json=json_set(settings_snapshot_json,'$.policy','copy-only-v1')",
            [],
        )
        .unwrap();
    let config = fake_cli(world.dir.path(), "x");
    let item = Jobs(ctx)
        .claim_media_item(world.job, "worker", world.fence, 600, 1)
        .unwrap()
        .unwrap();
    assert!(execute_claimed(ctx, item.id, "worker", world.fence, &config).is_err());
    assert!(
        !world.dir.path().join("args.txt").exists(),
        "the media CLI was never started"
    );
}
