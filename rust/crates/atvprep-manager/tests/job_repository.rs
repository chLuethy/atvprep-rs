//! Job repository (ported from `tests/manager/db/test_job_repository.py`). The tests that need
//! the state machine (cancelling automatic jobs, marking an item stale) live with Task 6.5.

use atvprep_manager::Error;
use atvprep_manager::clock::{FixedClock, format_micros};
use atvprep_manager::db::{Conn, Db};
use atvprep_manager::events::Events;
use atvprep_manager::jobs::{ClaimLimits, Jobs, NewJob};
use atvprep_manager::library::LibraryRoots;
use atvprep_manager::media::Media;
use atvprep_manager::models::MediaMeta;
use atvprep_manager::repo::Ctx;
use atvprep_manager::schema::migrate;
use atvprep_manager::settings::Settings;
use atvprep_manager::snapshots::InputSnapshots;
use atvprep_manager::state_machine::JobStatus;
use serde_json::json;

const T0: i64 = 1_700_000_000_000_000;

struct Fixture {
    _db: Db,
    conn: Conn,
    clock: FixedClock,
}

impl Fixture {
    fn new() -> Self {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        migrate(&mut conn).unwrap();
        Self {
            _db: db,
            conn,
            clock: FixedClock(T0),
        }
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            conn: self.conn.raw(),
            clock: &self.clock,
        }
    }

    fn media(&self, paths: &[&str]) -> Vec<i64> {
        let root = LibraryRoots(self.ctx())
            .create("library", "/library")
            .unwrap()
            .id;
        paths
            .iter()
            .map(|path| {
                let meta = MediaMeta {
                    kind: "video".into(),
                    suffix: ".mp4".into(),
                    size: 1,
                    mtime_ns: 1,
                    fingerprint: (*path).into(),
                    scanned_device: Some(1),
                    scanned_inode: Some(1),
                    scanned_ctime_ns: Some(1),
                    ..MediaMeta::default()
                };
                Media(self.ctx()).create(root, path, &meta).unwrap().id
            })
            .collect()
    }

    fn sql(&self, sql: &str) {
        self.conn.raw().execute_batch(sql).unwrap();
    }

    /// The media-executor lease held by one worker with this fencing token for a minute.
    fn lease(&self, token: i64) {
        let expires = format_micros(T0 + 60_000_000);
        self.sql(&format!(
            "UPDATE media_executor_leases SET owner_id='w', lease_expires_at='{expires}', fencing_token={token}"
        ));
    }
}

fn error_text<T: std::fmt::Debug>(result: atvprep_manager::Result<T>) -> String {
    result.unwrap_err().to_string()
}

#[test]
fn parent_and_items_persist_with_snapshots_and_a_created_event() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4", "b.mp4"]);
    let jobs = Jobs(f.ctx());
    let mut new = NewJob::new("fix", &media);
    new.trigger = "manual";
    new.settings_revision = 4;
    new.settings_snapshot = json!({"media_limit": 1});
    new.input_snapshots
        .insert(media[0], json!({"fingerprint": "a.mp4"}));
    let job = jobs.create(&new).unwrap();
    let loaded = jobs.get(job.id).unwrap().unwrap();
    assert_eq!(
        (
            loaded.action.as_str(),
            loaded.status.as_str(),
            loaded.settings_revision
        ),
        ("fix", "queued", 4)
    );
    assert_eq!(loaded.total_count, 2);
    let items = jobs.items(job.id).unwrap();
    assert_eq!(
        items.iter().map(|i| i.media_file_id).collect::<Vec<_>>(),
        media
    );
    assert_eq!(items[0].input_snapshot, json!({"fingerprint": "a.mp4"}));
    assert_eq!(items[0].settings_snapshot, json!({"media_limit": 1}));
    let events = Events(f.ctx()).list(job.id, None, 100).unwrap();
    assert_eq!(
        (events[0].topic.as_str(), events[0].entity_id),
        ("job.created", job.id)
    );
    assert_eq!(events[0].payload, json!({"action": "fix", "item_count": 2}));
}

#[test]
fn invalid_jobs_are_refused() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4"]);
    let jobs = Jobs(f.ctx());
    assert_eq!(
        error_text(jobs.create(&NewJob::new("fix", &[]))),
        "job must contain at least one media item"
    );
    assert_eq!(
        error_text(jobs.create(&NewJob::new("bogus", &media))),
        "invalid job action or trigger"
    );
    let mut bad = NewJob::new("fix", &media);
    bad.trigger = "bogus";
    assert_eq!(
        error_text(jobs.create(&bad)),
        "invalid job action or trigger"
    );
}

#[test]
fn mutating_items_are_unique_while_unfinished_and_a_retry_keeps_its_origin() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4"]);
    let jobs = Jobs(f.ctx());
    let first = jobs.create(&NewJob::new("convert", &media)).unwrap();
    assert_eq!(
        error_text(jobs.create(&NewJob::new("fix", &media))),
        "active job item already exists for media"
    );
    f.sql("UPDATE job_items SET status='failed'");
    let retry = jobs.retry(first.id, None).unwrap();
    assert_eq!(
        (retry.trigger.as_str(), retry.origin_job_id),
        ("retry", Some(first.id))
    );
    let origin = jobs.items(first.id).unwrap()[0].id;
    assert_eq!(
        jobs.items(retry.id).unwrap()[0].origin_item_id,
        Some(origin)
    );
    let events = Events(f.ctx()).list(retry.id, None, 100).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.topic == "job.retry_created"
                && e.payload == json!({"origin_job_id": first.id}))
    );
}

#[test]
fn inspect_items_can_share_media_and_the_schema_guards_hold() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4"]);
    let jobs = Jobs(f.ctx());
    let first = jobs.create(&NewJob::new("inspect", &media)).unwrap();
    let second = jobs.create(&NewJob::new("inspect", &media)).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(
        jobs.create(&NewJob::new("fix", &media)).unwrap().action,
        "fix"
    );
    let item = jobs.items(first.id).unwrap()[0].id;
    let invalid = f
        .conn
        .raw()
        .execute("UPDATE job_items SET action='invalid' WHERE id=?1", [item]);
    assert!(invalid.is_err());
    let event = Events(f.ctx()).list(first.id, None, 100).unwrap()[0].id;
    for sql in [
        "UPDATE events SET topic='changed' WHERE id=?1",
        "DELETE FROM events WHERE id=?1",
    ] {
        let error = f.conn.raw().execute(sql, [event]).unwrap_err();
        assert!(error.to_string().contains("append-only"), "{error}");
    }
}

#[test]
fn a_plain_claim_takes_the_first_queued_item_once_and_never_from_a_finished_parent() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4"]);
    let jobs = Jobs(f.ctx());
    let job = jobs.create(&NewJob::new("inspect", &media)).unwrap();
    let claimed = jobs
        .claim_item(job.id, "worker-a", 7, 30, &ClaimLimits::default())
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            claimed.claimed_by.as_deref(),
            claimed.executor_fencing_token
        ),
        (Some("worker-a"), Some(7))
    );
    assert_eq!(
        (
            claimed.status.as_str(),
            claimed.attempt,
            claimed.status_version
        ),
        ("claimed", 1, 1)
    );
    assert_eq!(
        claimed.lease_expires_at.as_deref(),
        Some(format_micros(T0 + 30_000_000).as_str())
    );
    assert!(
        jobs.claim_item(job.id, "worker-b", 8, 30, &ClaimLimits::default())
            .unwrap()
            .is_none()
    );

    let other = f.media_second();
    let second = jobs.create(&NewJob::new("inspect", &other)).unwrap();
    f.sql(&format!(
        "UPDATE jobs SET status='cancelled' WHERE id={}",
        second.id
    ));
    assert!(
        jobs.claim_item(second.id, "worker-a", 1, 30, &ClaimLimits::default())
            .unwrap()
            .is_none()
    );
}

impl Fixture {
    fn media_second(&self) -> Vec<i64> {
        let meta = MediaMeta {
            kind: "video".into(),
            suffix: ".mp4".into(),
            size: 1,
            mtime_ns: 1,
            fingerprint: "b".into(),
            scanned_device: Some(1),
            scanned_inode: Some(1),
            scanned_ctime_ns: Some(1),
            ..MediaMeta::default()
        };
        vec![Media(self.ctx()).create(1, "b.mp4", &meta).unwrap().id]
    }
}

#[test]
fn a_media_claim_needs_the_current_lease_and_respects_the_live_limit() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4", "b.mp4", "c.mp4"]);
    let jobs = Jobs(f.ctx());
    Settings(f.ctx())
        .set("max_ffmpeg_workers", &json!(1), "test", None)
        .unwrap();
    let job = jobs.create(&NewJob::new("fix", &media)).unwrap();

    // No lease yet, or the wrong token: nothing is claimed.
    assert!(
        jobs.claim_media_item(job.id, "w", 1, 30, 1)
            .unwrap()
            .is_none()
    );
    f.lease(5);
    assert!(
        jobs.claim_media_item(job.id, "w", 4, 30, 1)
            .unwrap()
            .is_none()
    );
    let first = jobs
        .claim_media_item(job.id, "w", 5, 30, 1)
        .unwrap()
        .unwrap();
    assert_eq!(first.media_file_id, media[0]);
    // The limit of one heavy item is reached.
    assert!(
        jobs.claim_media_item(job.id, "w", 5, 30, 1)
            .unwrap()
            .is_none()
    );
    Settings(f.ctx())
        .set("max_ffmpeg_workers", &json!(2), "test", None)
        .unwrap();
    assert_eq!(
        jobs.claim_media_item(job.id, "w", 5, 30, 1)
            .unwrap()
            .unwrap()
            .media_file_id,
        media[1]
    );
    // An expired lease claims nothing.
    f.sql(&format!(
        "UPDATE media_executor_leases SET lease_expires_at='{}'",
        format_micros(T0 - 1)
    ));
    Settings(f.ctx())
        .set("max_ffmpeg_workers", &json!(9), "test", None)
        .unwrap();
    assert!(
        jobs.claim_media_item(job.id, "w", 5, 30, 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn verifies_are_limited_by_the_probe_setting_among_verifies_only() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4", "b.mp4"]);
    let jobs = Jobs(f.ctx());
    f.lease(3);
    Settings(f.ctx())
        .set("max_probe_workers", &json!(1), "test", None)
        .unwrap();
    let heavy = jobs.create(&NewJob::new("fix", &media[..1])).unwrap();
    let verify = jobs.create(&NewJob::new("verify", &media)).unwrap();
    assert!(
        jobs.claim_media_item(heavy.id, "w", 3, 30, 1)
            .unwrap()
            .is_some()
    );
    // A running fix does not count against the verify limit.
    assert!(
        jobs.claim_media_item(verify.id, "w", 3, 30, 1)
            .unwrap()
            .is_some()
    );
    assert!(
        jobs.claim_media_item(verify.id, "w", 3, 30, 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn heartbeat_progress_and_log_belong_to_the_fenced_owner() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4"]);
    let jobs = Jobs(f.ctx());
    let job = jobs.create(&NewJob::new("fix", &media)).unwrap();
    f.lease(7);
    let item = jobs
        .claim_media_item(job.id, "w", 7, 30, 1)
        .unwrap()
        .unwrap();
    assert!(jobs.heartbeat_item(item.id, 7, 60).unwrap());
    assert!(!jobs.heartbeat_item(item.id, 8, 60).unwrap());
    let renewed = jobs.get_item(item.id).unwrap().unwrap();
    assert_eq!(
        renewed.lease_expires_at.as_deref(),
        Some(format_micros(T0 + 60_000_000).as_str())
    );

    // Progress and log only for a running item of the same owner, and progress never goes back.
    assert!(!jobs.set_item_progress(item.id, 7, 40).unwrap());
    f.sql("UPDATE job_items SET status='running'");
    assert!(jobs.set_item_progress(item.id, 7, 40).unwrap());
    assert!(!jobs.set_item_progress(item.id, 7, 25).unwrap());
    assert!(!jobs.set_item_progress(item.id, 8, 60).unwrap());
    assert!(jobs.set_item_progress(item.id, 7, 250).unwrap());
    assert_eq!(jobs.get_item(item.id).unwrap().unwrap().progress, Some(100));
    assert!(jobs.set_item_log_tail(item.id, 7, "frame=1").unwrap());
    assert!(!jobs.set_item_log_tail(item.id, 8, "x").unwrap());
    f.sql("UPDATE job_items SET status='succeeded'");
    assert!(!jobs.set_item_progress(item.id, 7, 99).unwrap());
    assert!(!jobs.set_item_log_tail(item.id, 7, "late").unwrap());
    assert!(!jobs.heartbeat_item(item.id, 7, 60).unwrap());
    // Losing the executor lease stops heartbeats of a still-running item.
    f.sql("UPDATE job_items SET status='running'");
    f.sql(&format!(
        "UPDATE media_executor_leases SET lease_expires_at='{}'",
        format_micros(T0 - 1)
    ));
    assert!(!jobs.heartbeat_item(item.id, 7, 60).unwrap());
}

#[test]
fn clear_finished_hides_only_the_requested_finished_statuses() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4", "b.mp4", "c.mp4"]);
    let jobs = Jobs(f.ctx());
    let ids: Vec<i64> = media
        .iter()
        .map(|m| jobs.create(&NewJob::new("verify", &[*m])).unwrap().id)
        .collect();
    f.sql(&format!(
        "UPDATE jobs SET status='failed' WHERE id={}",
        ids[0]
    ));
    f.sql(&format!(
        "UPDATE jobs SET status='succeeded' WHERE id={}",
        ids[1]
    ));
    assert_eq!(jobs.clear_finished(&[JobStatus::Failed]).unwrap(), 1);
    assert_eq!(jobs.clear_finished(&[JobStatus::Failed]).unwrap(), 0);
    let cleared = |id: i64| jobs.get(id).unwrap().unwrap().cleared_at.is_some();
    assert_eq!(
        (cleared(ids[0]), cleared(ids[1]), cleared(ids[2])),
        (true, false, false)
    );
    for status in [
        JobStatus::Queued,
        JobStatus::Running,
        JobStatus::CancelRequested,
    ] {
        assert!(
            matches!(jobs.clear_finished(&[status]), Err(Error::Invalid(_))),
            "{status}"
        );
    }
    assert!(jobs.clear_finished(&[]).is_err());
}

#[test]
fn a_retry_copies_only_the_retryable_items_of_a_mixed_result() {
    let f = Fixture::new();
    let media = f.media(&["succeeded.mp4", "failed.mp4", "cancelled.mp4"]);
    let jobs = Jobs(f.ctx());
    let job = jobs.create(&NewJob::new("verify", &media)).unwrap();
    f.sql("UPDATE job_items SET status='succeeded' WHERE position=0");
    f.sql("UPDATE job_items SET status='failed' WHERE position=1");
    f.sql("UPDATE job_items SET status='cancelled' WHERE position=2");
    let retry = jobs.retry(job.id, None).unwrap();
    assert_eq!(
        jobs.items(retry.id)
            .unwrap()
            .iter()
            .map(|i| i.media_file_id)
            .collect::<Vec<_>>(),
        [media[1]]
    );
    assert_eq!(retry.total_count, 1);
    // A job with an unfinished item, or with nothing to retry, is refused.
    f.sql(&format!(
        "UPDATE job_items SET status='running' WHERE job_id={} AND position=2",
        job.id
    ));
    assert_eq!(
        error_text(jobs.retry(job.id, None)),
        "job has no retryable failed items"
    );
    assert_eq!(error_text(jobs.retry(9999, None)), "unknown job id: 9999");
}

#[test]
fn a_prepare_retry_needs_the_same_files_and_drops_the_old_approval() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4"]);
    let jobs = Jobs(f.ctx());
    let mut new = NewJob::new("fix", &media);
    new.settings_snapshot = json!({"pipeline": "prepare", "approval_mac": "abc", "approved_by": 1});
    let job = jobs.create(&new).unwrap();
    f.sql("UPDATE job_items SET status='failed'");
    let retry = jobs.retry(job.id, Some(42)).unwrap();
    let snapshot = &retry.settings_snapshot;
    assert_eq!(
        (
            snapshot["approved_by"].clone(),
            snapshot.get("approval_mac")
        ),
        (json!(42), None)
    );
    assert_eq!(snapshot["approved_at"], json!(T0 / 1_000_000));

    f.sql("UPDATE job_items SET status='failed'");
    f.sql("UPDATE media_files SET size=999");
    assert_eq!(
        error_text(jobs.retry(job.id, None)),
        "a file changed since the job was queued; queue it again"
    );
}

#[test]
fn events_are_ordered_and_an_unknown_job_is_rejected() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4"]);
    let job = Jobs(f.ctx())
        .create(&NewJob::new("verify", &media))
        .unwrap();
    let events = Events(f.ctx());
    let event = events
        .append(
            job.id,
            "job.progress",
            "job",
            job.id,
            &json!({"done": 1}),
            None,
        )
        .unwrap();
    assert_eq!(event.payload, json!({"done": 1}));
    assert_eq!(event.expires_at, format_micros(T0 + 7 * 86_400 * 1_000_000));
    let listed = events.list(job.id, None, 100).unwrap();
    assert!(listed.windows(2).all(|w| w[0].id < w[1].id));
    assert_eq!(
        events.list(job.id, Some(listed[0].id), 100).unwrap().len(),
        listed.len() - 1
    );
    assert_eq!(events.list(job.id, None, 1).unwrap().len(), 1);
    assert!(
        error_text(events.append(99_999, "bad", "job", 99_999, &json!({}), None))
            .contains("unknown job")
    );
}

#[test]
fn the_input_snapshot_is_stored_once_and_detects_divergence() {
    let f = Fixture::new();
    Settings(f.ctx())
        .set("max_ffmpeg_workers", &json!(4), "test", None)
        .unwrap();
    let media = f.media(&["a.mp4"]);
    let jobs = Jobs(f.ctx());
    let job = jobs.create(&NewJob::new("verify", &media)).unwrap();
    let item = jobs.items(job.id).unwrap()[0].id;
    let snapshots = InputSnapshots(f.ctx());
    let stored = snapshots.get_for_item(item).unwrap().unwrap();
    assert_eq!(
        (
            stored["media_id"].clone(),
            stored["file_fingerprint"].clone()
        ),
        (json!(media[0]), json!("a.mp4"))
    );
    assert!(stored["settings_revision"].as_i64().unwrap() >= 1);
    // Recording again keeps the first snapshot.
    assert_eq!(
        snapshots.record(item, &json!({"other": true})).unwrap(),
        stored
    );
    assert!(snapshots.check_item_not_stale(item).unwrap().is_empty());

    f.sql("UPDATE media_files SET size=999, fingerprint='changed.mp4'");
    let reasons = snapshots.check_item_not_stale(item).unwrap();
    assert!(
        reasons.contains(&"file.size".to_owned())
            && reasons.contains(&"file.fingerprint".to_owned()),
        "{reasons:?}"
    );
    Settings(f.ctx())
        .set("max_ffmpeg_workers", &json!(8), "test", None)
        .unwrap();
    assert!(
        snapshots
            .check_item_not_stale(item)
            .unwrap()
            .contains(&"settings.revision".to_owned())
    );

    f.sql("DELETE FROM input_snapshots");
    assert_eq!(
        snapshots.check_item_not_stale(item).unwrap(),
        ["snapshot.missing"]
    );
}

#[test]
fn a_retry_takes_a_fresh_snapshot() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4"]);
    let jobs = Jobs(f.ctx());
    let first = jobs.create(&NewJob::new("convert", &media)).unwrap();
    f.sql("UPDATE job_items SET status='failed'");
    f.sql("UPDATE media_files SET size=555");
    let retry = jobs.retry(first.id, None).unwrap();
    let item = jobs.items(retry.id).unwrap()[0].id;
    let snapshot = InputSnapshots(f.ctx()).get_for_item(item).unwrap().unwrap();
    assert_eq!(snapshot["file_size"], json!(555));
}

#[test]
fn runtime_settings_of_the_control_plane_are_read_with_a_floor_of_one() {
    let f = Fixture::new();
    let jobs = Jobs(f.ctx());
    assert_eq!(jobs.read_runtime_int("media_limit", 3).unwrap(), 3);
    assert_eq!(jobs.read_runtime_int("media_limit", 0).unwrap(), 1);
    jobs.set_runtime_setting("media_limit", &json!(5), "int", 1)
        .unwrap();
    assert_eq!(jobs.read_runtime_int("media_limit", 3).unwrap(), 5);
    jobs.set_runtime_setting("media_limit", &json!("x"), "int", 2)
        .unwrap();
    assert_eq!(jobs.read_runtime_int("media_limit", 3).unwrap(), 3);
    jobs.set_runtime_setting("media_limit", &json!(-4), "int", 3)
        .unwrap();
    assert_eq!(jobs.read_runtime_int("media_limit", 3).unwrap(), 1);
}

#[test]
fn a_stored_status_that_is_not_known_is_an_error_naming_the_value() {
    let f = Fixture::new();
    let media = f.media(&["a.mp4"]);
    let jobs = Jobs(f.ctx());
    let job = jobs.create(&NewJob::new("fix", &media)).unwrap();
    let item = jobs.items(job.id).unwrap()[0].id;
    f.sql("PRAGMA ignore_check_constraints=ON");
    f.sql(&format!(
        "UPDATE job_items SET status='bogus_item' WHERE id={item};
         UPDATE jobs SET status='bogus_job' WHERE id={}",
        job.id
    ));

    assert!(error_text(jobs.get_item(item)).contains("bogus_item"));
    assert!(error_text(jobs.get(job.id)).contains("bogus_job"));
}
