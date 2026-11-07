//! A schedule's signature check refuses a queued changing job for good: the item ends at once,
//! so neither its claim nor the executor lease starves other changing jobs.
use atvprep_manager::{
    approval::{ApprovalVerifier, ScheduleParams},
    clock::SystemClock,
    db::{Conn, Db},
    jobs::{Jobs, NewJob},
    leases::MediaExecutorLease,
    library::LibraryRoots,
    media::Media,
    models::MediaMeta,
    repo::Ctx,
    schedules::run_schedule,
    schema,
    state_machine::ItemStatus,
    worker::execution::{ExecutionConfig, execute_claimed},
};
use std::{collections::BTreeMap, os::unix::fs::MetadataExt, path::PathBuf, time::Duration};

fn config() -> ExecutionConfig {
    ExecutionConfig {
        executable: PathBuf::from("/nonexistent/atvprep"),
        environment: BTreeMap::new(),
        timeout: Duration::from_secs(5),
        grace: Duration::from_millis(100),
        lease_seconds: 30,
        shutdown: None,
        approvals: ApprovalVerifier::new(b"secret").map(std::sync::Arc::new),
        session_idle_seconds: 3600,
    }
}

struct World {
    _dir: tempfile::TempDir,
    conn: Conn,
    media: i64,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("film.mp4"), b"not really a video").unwrap();
    let root_path = dir.path().canonicalize().unwrap();
    let metadata = std::fs::metadata(root_path.join("film.mp4")).unwrap();
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
            "film.mp4",
            &MediaMeta {
                kind: "video".to_owned(),
                suffix: ".mp4".to_owned(),
                size: i64::try_from(metadata.len()).unwrap(),
                mtime_ns: metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec(),
                fingerprint: "fp".to_owned(),
                scanned_device: Some(i64::try_from(metadata.dev()).unwrap()),
                scanned_inode: Some(i64::try_from(metadata.ino()).unwrap()),
                scanned_ctime_ns: Some(metadata.ctime() * 1_000_000_000 + metadata.ctime_nsec()),
                ..MediaMeta::default()
            },
        )
        .unwrap();
    conn.raw()
        .execute("UPDATE media_files SET compliance_state='warning'", [])
        .unwrap();
    conn.raw()
        .execute(
            "INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,max_files,enabled,armed_at) VALUES(1,'s','fix',1,127,'03:00',5,1,'2000-01-01')",
            [],
        )
        .unwrap();
    World {
        _dir: dir,
        conn,
        media: media.id,
    }
}

/// Sign the schedule as it is now, queue its job, then let `change` happen before the worker gets there.
fn refused_after(change: &str) {
    let world = world();
    let clock = SystemClock;
    let ctx = Ctx {
        conn: world.conn.raw(),
        clock: &clock,
    };
    let verifier = ApprovalVerifier::new(b"secret").unwrap();
    let params = ScheduleParams {
        schedule_id: 1,
        action: "fix",
        root_id: Some(1),
        compliance_state: None,
        max_files: 5,
        delete_original: false,
    };
    ctx.conn
        .execute(
            "UPDATE schedules SET approved_by=1, approved_at=1700000000, approval_mac=?1",
            [verifier.sign_schedule(&params, 1, 1_700_000_000)],
        )
        .unwrap();
    assert_eq!(run_schedule(ctx, 1).unwrap(), "Queued 1 file");
    ctx.conn.execute(change, []).unwrap();

    let mut lease = MediaExecutorLease::new(ctx);
    assert!(lease.acquire("worker", 30).unwrap());
    let fence = lease.fencing_token.unwrap();
    let job: i64 = ctx
        .conn
        .query_row("SELECT id FROM jobs ORDER BY id DESC LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let item = Jobs(ctx)
        .claim_media_item(job, "worker", fence, 30, 1)
        .unwrap()
        .unwrap();
    assert!(execute_claimed(ctx, item.id, "worker", fence, &config()).is_err());

    let ended = Jobs(ctx).get_item(item.id).unwrap().unwrap();
    assert_eq!(ended.status, ItemStatus::StaleInput, "{change}");
    // The refusal is not transient, so it must not keep the single heavy slot occupied.
    let other = Jobs(ctx)
        .create(&NewJob::new("fix", &[world.media]))
        .unwrap();
    assert!(
        Jobs(ctx)
            .claim_media_item(other.id, "worker", fence, 30, 1)
            .unwrap()
            .is_some(),
        "another changing job is starved after: {change}"
    );
    assert!(lease.release().unwrap());
}

#[test]
fn pausing_the_schedule_ends_its_queued_item() {
    refused_after("UPDATE schedules SET enabled=0 WHERE id=1");
}

#[test]
fn editing_max_files_ends_its_queued_item() {
    refused_after("UPDATE schedules SET max_files=6 WHERE id=1");
}

#[test]
fn deleting_the_schedule_ends_its_queued_item() {
    refused_after("DELETE FROM schedules WHERE id=1");
}
