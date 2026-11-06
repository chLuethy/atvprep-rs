//! The job state machine (ported from `tests/manager/db/test_transitions.py` and the cancel and
//! stale-input cases of `test_job_repository.py`), plus the Python oracle for its tables.

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
use atvprep_manager::snapshots::InputSnapshots;
use atvprep_manager::state_machine::{
    ItemStatus, JobStateMachine, JobStatus, aggregate_parent_status, item_allowed, parent_allowed,
};
use atvprep_testkit::load_golden;
use serde_json::{Value, json};

const T0: i64 = 1_700_000_000_000_000;

struct Fixture {
    db: Db,
    conn: Conn,
    clock: FixedClock,
}

impl Fixture {
    fn new() -> Self {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        migrate(&mut conn).unwrap();
        Self {
            db,
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

    fn machine(&self) -> JobStateMachine<'_> {
        JobStateMachine(self.ctx())
    }

    fn media(&self, names: &[&str]) -> Vec<i64> {
        let roots = LibraryRoots(self.ctx());
        let root = roots
            .get(1)
            .unwrap()
            .map_or_else(|| roots.create("library", "/library").unwrap().id, |r| r.id);
        (self.next_inode()..)
            .zip(names)
            .map(|(inode, name)| {
                let meta = MediaMeta {
                    kind: "video".into(),
                    suffix: ".mp4".into(),
                    size: 1,
                    mtime_ns: 1,
                    fingerprint: (*name).into(),
                    scanned_device: Some(1),
                    scanned_inode: Some(inode),
                    scanned_ctime_ns: Some(1),
                    ..MediaMeta::default()
                };
                Media(self.ctx()).create(root, name, &meta).unwrap().id
            })
            .collect()
    }

    /// A job of `count` files and its first item.
    fn job(&self, action: &str, count: usize) -> (i64, i64) {
        let names: Vec<String> = (0..count).map(|i| format!("{i}.mp4")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let media = self.media(&refs);
        let job = Jobs(self.ctx())
            .create(&NewJob::new(action, &media))
            .unwrap();
        (job.id, Jobs(self.ctx()).items(job.id).unwrap()[0].id)
    }

    fn next_inode(&self) -> i64 {
        self.conn
            .raw()
            .query_row(
                "SELECT COALESCE(MAX(scanned_inode), 0) + 1 FROM media_files",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn sql(&self, sql: &str) {
        self.conn.raw().execute_batch(sql).unwrap();
    }
}

#[test]
fn the_tables_and_the_aggregation_match_the_python_oracle() {
    let cases = load_golden::<Value, Value>("state_machine");
    assert_eq!(cases.len(), 1002);
    let statuses = |kind: &str| -> Vec<String> {
        cases[0].expected[kind]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    };
    for (kind, allowed) in [
        ("item", item_allowed as fn(&str, &str) -> bool),
        ("parent", parent_allowed),
    ] {
        let table = cases[0].expected[kind].as_object().unwrap();
        let universe: Vec<String> = statuses("item")
            .into_iter()
            .chain(statuses("parent"))
            .collect();
        for from in &universe {
            let wanted: Vec<&str> = table
                .get(from)
                .map(|v| {
                    v.as_array()
                        .unwrap()
                        .iter()
                        .map(|s| s.as_str().unwrap())
                        .collect()
                })
                .unwrap_or_default();
            for to in &universe {
                assert_eq!(
                    allowed(from, to),
                    wanted.contains(&to.as_str()),
                    "{kind} {from} -> {to}"
                );
            }
        }
    }
    for case in &cases[1..] {
        let statuses: Vec<&str> = case.input["statuses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        let got = match aggregate_parent_status(&statuses) {
            Ok(status) => json!({"ok": status}),
            Err(error) => json!({"error": error.to_string()}),
        };
        assert_eq!(got, case.expected, "{}", case.name);
    }
}

#[test]
fn the_typed_statuses_follow_the_python_oracle_tables() {
    let cases = load_golden::<Value, Value>("state_machine");
    let wanted = |kind: &str, from: &str, to: &str| {
        cases[0].expected[kind]
            .get(from)
            .is_some_and(|v| v.as_array().unwrap().iter().any(|s| s == to))
    };
    for from in ItemStatus::ALL {
        for to in ItemStatus::ALL {
            assert_eq!(
                from.can_become(*to),
                wanted("item", from.as_str(), to.as_str()),
                "item {from} -> {to}"
            );
        }
    }
    for from in JobStatus::ALL {
        for to in JobStatus::ALL {
            assert_eq!(
                from.can_become(*to),
                wanted("parent", from.as_str(), to.as_str()),
                "job {from} -> {to}"
            );
        }
    }
}

#[test]
fn the_typed_statuses_round_trip_their_strings_and_know_terminal_states() {
    let item_terminal = [
        "succeeded",
        "failed",
        "stale_input",
        "cancelled",
        "interrupted",
    ];
    for status in ItemStatus::ALL {
        assert_eq!(status.as_str().parse::<ItemStatus>().unwrap(), *status);
        assert_eq!(
            status.is_terminal(),
            item_terminal.contains(&status.as_str()),
            "item {status}"
        );
    }
    let job_terminal = ["succeeded", "partial_success", "failed", "cancelled"];
    for status in JobStatus::ALL {
        assert_eq!(status.as_str().parse::<JobStatus>().unwrap(), *status);
        assert_eq!(
            status.is_terminal(),
            job_terminal.contains(&status.as_str()),
            "job {status}"
        );
    }
}

#[test]
fn the_typed_statuses_read_and_write_the_same_strings_in_sqlite() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t(item TEXT, job TEXT)")
        .unwrap();
    for (item, job) in ItemStatus::ALL.iter().zip(JobStatus::ALL.iter().cycle()) {
        conn.execute(
            "INSERT INTO t VALUES (?1, ?2)",
            rusqlite::params![item, job],
        )
        .unwrap();
    }
    let (raw_item, raw_job): (String, String) = conn
        .query_row("SELECT item, job FROM t LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((raw_item.as_str(), raw_job.as_str()), ("queued", "queued"));
    let item: ItemStatus = conn
        .query_row("SELECT item FROM t LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(item, ItemStatus::Queued);
    conn.execute("UPDATE t SET job='bogus'", []).unwrap();
    let error = conn
        .query_row("SELECT job FROM t LIMIT 1", [], |r| {
            r.get::<_, JobStatus>(0)
        })
        .unwrap_err();
    assert!(error.to_string().contains("bogus"), "{error}");
}

#[test]
fn an_unknown_status_string_is_an_error_naming_the_value() {
    let error = "bogus".parse::<ItemStatus>().unwrap_err();
    assert!(matches!(error, Error::Invalid(_)));
    assert!(error.to_string().contains("bogus"), "{error}");
    let error = "claimed".parse::<JobStatus>().unwrap_err();
    assert!(error.to_string().contains("claimed"), "{error}");
}

#[test]
fn an_item_transition_bumps_the_version_starts_the_job_and_emits_an_event() {
    let f = Fixture::new();
    let (job, item) = f.job("verify", 1);
    let result = f
        .machine()
        .transition_item(item, "claimed", 0, Some(7), None)
        .unwrap();
    assert_eq!(
        (
            result.status.as_str(),
            result.status_version,
            result.changed
        ),
        ("claimed", 1, true)
    );
    assert_eq!(Jobs(f.ctx()).get(job).unwrap().unwrap().status, "running");
    let events = Events(f.ctx()).list(job, None, 100).unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.topic, "job.item.transition");
    assert_eq!(
        last.payload,
        json!({"from": "queued", "to": "claimed", "from_version": 0, "to_version": 1})
    );
    assert_eq!(
        (last.item_id, last.entity_type.as_str()),
        (Some(item), "item")
    );
}

#[test]
fn a_stale_writer_is_rejected_without_a_second_update() {
    let f = Fixture::new();
    let (job, item) = f.job("verify", 1);
    f.machine()
        .transition_item(item, "claimed", 0, Some(7), None)
        .unwrap();
    let error = f
        .machine()
        .transition_item(item, "running", 0, Some(7), None)
        .unwrap_err();
    assert!(
        matches!(&error, Error::Conflict(m) if m.starts_with("stale job.item.transition writer: expected version 0, current 1")),
        "{error}"
    );
    assert_eq!(Jobs(f.ctx()).items(job).unwrap()[0].status_version, 1);
}

#[test]
fn a_worker_transition_needs_the_current_fencing_token_even_on_replay() {
    let f = Fixture::new();
    let (_, item) = f.job("verify", 1);
    f.machine()
        .transition_item(item, "claimed", 0, Some(7), None)
        .unwrap();
    let wrong = f
        .machine()
        .transition_item(item, "claimed", 1, Some(999), None)
        .unwrap_err();
    assert_eq!(wrong.to_string(), "stale item fencing token");
    let missing = f
        .machine()
        .transition_item(item, "running", 1, None, None)
        .unwrap_err();
    assert_eq!(
        missing.to_string(),
        "worker item transition requires a fencing token"
    );
}

#[test]
fn the_executor_lease_guards_worker_transitions_once_it_is_in_use() {
    let f = Fixture::new();
    let (_, item) = f.job("fix", 1);
    f.sql(&format!(
        "UPDATE media_executor_leases SET owner_id='w', fencing_token=7, lease_expires_at='{}'",
        format_micros(T0 + 30_000_000)
    ));
    f.machine()
        .transition_item(item, "claimed", 0, Some(7), None)
        .unwrap();
    // Another holder took the lease over: the old token no longer works.
    f.sql("UPDATE media_executor_leases SET fencing_token=8");
    let lost = f
        .machine()
        .transition_item(item, "running", 1, Some(7), None)
        .unwrap_err();
    assert_eq!(
        lost.to_string(),
        "global media executor lease no longer held by this worker"
    );
    f.sql("UPDATE media_executor_leases SET fencing_token=7");
    f.machine()
        .transition_item(item, "running", 1, Some(7), None)
        .unwrap();
    // An expired lease fails closed.
    f.sql(&format!(
        "UPDATE media_executor_leases SET lease_expires_at='{}'",
        format_micros(T0 - 1)
    ));
    assert!(matches!(
        f.machine()
            .transition_item(item, "succeeded", 2, Some(7), None),
        Err(Error::Conflict(_))
    ));
    // A lease written with a time zone suffix (as a differently written heartbeat would) still counts.
    f.sql(&format!(
        "UPDATE media_executor_leases SET lease_expires_at='{}+00:00'",
        format_micros(T0 + 30_000_000).replace(' ', "T")
    ));
    f.machine()
        .transition_item(item, "succeeded", 2, Some(7), None)
        .unwrap();
}

#[test]
fn an_item_transition_keeps_a_cancel_requested_parent() {
    let f = Fixture::new();
    let (job, item) = f.job("verify", 1);
    f.machine()
        .transition_item(item, "claimed", 0, Some(7), None)
        .unwrap();
    let parent = Jobs(f.ctx()).get(job).unwrap().unwrap();
    f.machine()
        .transition_job(job, "cancel_requested", parent.status_version, None)
        .unwrap();
    f.machine()
        .transition_item(item, "running", 1, Some(7), None)
        .unwrap();
    assert_eq!(
        Jobs(f.ctx()).get(job).unwrap().unwrap().status,
        "cancel_requested"
    );
}

#[test]
fn an_illegal_transition_is_rejected() {
    let f = Fixture::new();
    let (_, item) = f.job("verify", 1);
    let error = f
        .machine()
        .transition_item(item, "succeeded", 0, None, None)
        .unwrap_err();
    assert!(
        matches!(&error, Error::Illegal(m) if m == "cannot transition queued to succeeded"),
        "{error}"
    );
    assert!(matches!(
        f.machine().transition_item(9999, "claimed", 0, None, None),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn a_terminal_parent_cannot_regress_and_a_repeat_is_stable() {
    let f = Fixture::new();
    let (job, _) = f.job("verify", 1);
    let machine = f.machine();
    let first = machine.transition_job(job, "running", 0, None).unwrap();
    let second = machine
        .transition_job(job, "cancel_requested", first.status_version, None)
        .unwrap();
    let cancelled = machine
        .transition_job(job, "cancelled", second.status_version, None)
        .unwrap();
    let repeated = machine
        .transition_job(job, "cancelled", cancelled.status_version, None)
        .unwrap();
    assert_eq!(
        (
            repeated.status.as_str(),
            repeated.status_version,
            repeated.changed
        ),
        ("cancelled", cancelled.status_version, false)
    );
    let error = machine
        .transition_job(job, "running", cancelled.status_version, None)
        .unwrap_err();
    assert!(matches!(error, Error::Illegal(_)));
}

#[test]
fn start_and_finish_times_are_recorded_once_for_the_item_and_its_job() {
    let f = Fixture::new();
    let (job, item) = f.job("verify", 1);
    let jobs = Jobs(f.ctx());
    f.machine()
        .transition_item(item, "claimed", 0, Some(7), None)
        .unwrap();
    let parent = jobs.get(job).unwrap().unwrap();
    assert!(parent.started_at.is_some() && parent.finished_at.is_none());
    assert!(jobs.items(job).unwrap()[0].started_at.is_none());
    f.machine()
        .transition_item(item, "running", 1, Some(7), None)
        .unwrap();
    let running = jobs.items(job).unwrap().remove(0);
    assert!(running.started_at.is_some() && running.finished_at.is_none());
    f.machine()
        .transition_item(item, "succeeded", 2, Some(7), None)
        .unwrap();
    let done = jobs.items(job).unwrap().remove(0);
    assert!(done.finished_at.is_some());
    assert_eq!(done.started_at, running.started_at);
    let finished = jobs.get(job).unwrap().unwrap();
    assert_eq!(finished.status, "succeeded");
    assert!(finished.finished_at.is_some() && finished.started_at.is_some());
    assert_eq!(
        (
            finished.total_count,
            finished.succeeded_count,
            finished.failed_count
        ),
        (1, 1, 0)
    );
}

#[test]
fn the_parent_counts_follow_a_mixed_result() {
    let f = Fixture::new();
    let (job, first) = f.job("verify", 1);
    // A second item for the same job, from the same fixture's second file.
    let extra = f.media(&["second.mp4"]);
    f.conn.raw().execute(
        "INSERT INTO job_items(job_id, media_file_id, action, position) VALUES (?1, ?2, 'verify', 1)",
        [job, extra[0]],
    ).unwrap();
    let second = f.conn.raw().last_insert_rowid();
    let machine = f.machine();
    machine
        .transition_item(first, "claimed", 0, Some(1), None)
        .unwrap();
    machine
        .transition_item(first, "running", 1, Some(1), None)
        .unwrap();
    machine
        .transition_item(first, "succeeded", 2, Some(1), None)
        .unwrap();
    assert_eq!(Jobs(f.ctx()).get(job).unwrap().unwrap().status, "running");
    machine
        .transition_item(second, "stale_input", 0, None, None)
        .unwrap();
    let parent = Jobs(f.ctx()).get(job).unwrap().unwrap();
    assert_eq!(parent.status, "partial_success");
    assert_eq!(
        (
            parent.succeeded_count,
            parent.failed_count,
            parent.cancelled_count
        ),
        (1, 1, 0)
    );
}

#[test]
fn an_item_marked_stale_fails_closed_with_the_reasons() {
    let f = Fixture::new();
    let (job, item) = f.job("verify", 1);
    let snapshots = InputSnapshots(f.ctx());
    assert!(snapshots.check_item_not_stale(item).unwrap().is_empty());
    f.sql("UPDATE media_files SET size=999");
    let reasons = snapshots.check_item_not_stale(item).unwrap();
    assert!(reasons.contains(&"file.size".to_owned()));
    let result = f.machine().mark_item_stale(item, &reasons).unwrap();
    assert_eq!(
        (result.status.as_str(), result.changed),
        ("stale_input", true)
    );
    assert_eq!(Jobs(f.ctx()).items(job).unwrap()[0].status, "stale_input");
    let events = Events(f.ctx()).list(job, None, 100).unwrap();
    let stale: Vec<_> = events
        .iter()
        .filter(|e| e.topic == "job.item.transition" && e.payload["to"] == "stale_input")
        .collect();
    assert_eq!(stale.last().unwrap().payload["reasons"], json!(reasons));
}

#[test]
fn cancelling_queued_automatic_jobs_leaves_manual_work_alone() {
    let f = Fixture::new();
    let media = f.media(&["auto-fix.mp4", "manual-verify.mp4"]);
    let jobs = Jobs(f.ctx());
    let mut auto = NewJob::new("fix", &media[..1]);
    auto.trigger = "automatic";
    let automatic = jobs.create(&auto).unwrap();
    let mut by_hand = NewJob::new("verify", &media[1..]);
    by_hand.trigger = "manual";
    let manual = jobs.create(&by_hand).unwrap();
    assert_eq!(f.machine().cancel_queued_automatic_jobs().unwrap(), 1);
    assert_eq!(jobs.get(automatic.id).unwrap().unwrap().status, "cancelled");
    assert_eq!(jobs.items(automatic.id).unwrap()[0].status, "cancelled");
    assert_eq!(jobs.get(manual.id).unwrap().unwrap().status, "queued");
    assert_eq!(jobs.items(manual.id).unwrap()[0].status, "queued");
    assert_eq!(f.machine().cancel_queued_automatic_jobs().unwrap(), 0);
}

#[test]
fn items_left_queued_under_terminal_automatic_jobs_are_settled() {
    let f = Fixture::new();
    let media = f.media(&["stranded.mp4", "failed.mp4", "live.mp4"]);
    let jobs = Jobs(f.ctx());
    let stranded = jobs.create(&NewJob::new("fix", &media[..1])).unwrap();
    let failed = jobs.create(&NewJob::new("fix", &media[1..2])).unwrap();
    let mut live = NewJob::new("fix", &media[2..]);
    live.trigger = "manual";
    let live = jobs.create(&live).unwrap();
    f.sql(&format!(
        "UPDATE jobs SET status='cancelled' WHERE id={}",
        stranded.id
    ));
    f.sql(&format!(
        "UPDATE jobs SET status='failed' WHERE id={}",
        failed.id
    ));
    f.machine().cancel_queued_automatic_jobs().unwrap();
    assert_eq!(jobs.items(stranded.id).unwrap()[0].status, "cancelled");
    assert_eq!(jobs.items(failed.id).unwrap()[0].status, "cancelled");
    assert_eq!(jobs.get(stranded.id).unwrap().unwrap().status, "cancelled");
    assert_eq!(jobs.get(failed.id).unwrap().unwrap().status, "failed");
    assert_eq!(jobs.items(live.id).unwrap()[0].status, "queued");
}

#[test]
fn failed_claim_readback_rolls_back_the_claim_and_attempt() {
    let f = Fixture::new();
    let (job, item) = f.job("verify", 1);
    f.sql("CREATE TRIGGER unreadable_claim AFTER UPDATE OF status ON job_items WHEN NEW.status='claimed' BEGIN UPDATE job_items SET attempt='unreadable' WHERE id=NEW.id; END;");
    assert!(
        Jobs(f.ctx())
            .claim_item(job, "worker", 7, 30, &ClaimLimits::default())
            .is_err()
    );
    let fresh = Jobs(f.ctx()).get_item(item).unwrap().unwrap();
    assert_eq!(fresh.status, "queued");
    assert_eq!(fresh.attempt, 0);
    assert!(f.conn.raw().is_autocommit());
}

#[test]
fn failed_transition_event_rolls_back_item_parent_and_all_events() {
    let f = Fixture::new();
    let (job, item) = f.job("verify", 1);
    let before: i64 = f
        .conn
        .raw()
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    f.sql("CREATE TRIGGER refuse_item_event BEFORE INSERT ON events WHEN NEW.topic='job.item.transition' BEGIN SELECT RAISE(ABORT,'forced item event failure'); END;");
    assert!(
        f.machine()
            .transition_item(item, "claimed", 0, Some(7), None)
            .is_err()
    );
    assert_eq!(
        Jobs(f.ctx()).get_item(item).unwrap().unwrap().status,
        "queued"
    );
    assert_eq!(Jobs(f.ctx()).get(job).unwrap().unwrap().status, "queued");
    let after: i64 = f
        .conn
        .raw()
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after, before);
    assert!(f.conn.raw().is_autocommit());
}

#[test]
fn claim_and_transition_retry_real_shared_cache_write_locks() {
    let f = Fixture::new();
    let (job, item) = f.job("verify", 1);
    let block = || {
        let conn = f.db.conn().unwrap();
        conn.raw()
            .execute_batch("BEGIN IMMEDIATE; UPDATE job_items SET position=position;")
            .unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(40));
            conn.raw().execute_batch("ROLLBACK").unwrap();
        })
    };
    let release = block();
    let claimed = Jobs(f.ctx())
        .claim_item(job, "worker", 7, 30, &ClaimLimits::default())
        .unwrap()
        .unwrap();
    release.join().unwrap();
    assert_eq!(claimed.id, item);
    assert_eq!(claimed.attempt, 1);
    let release = block();
    f.machine()
        .transition_item(item, "running", claimed.status_version, Some(7), None)
        .unwrap();
    release.join().unwrap();
    assert_eq!(
        Jobs(f.ctx()).get_item(item).unwrap().unwrap().status,
        "running"
    );
}
