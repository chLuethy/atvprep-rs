//! The media-executor lease and how it fences stale workers.

use atvprep_manager::Error;
use atvprep_manager::clock::{Clock, FixedClock};
use atvprep_manager::db::{Conn, Db};
use atvprep_manager::jobs::{ClaimLimits, Jobs, NewJob};
use atvprep_manager::leases::MediaExecutorLease;
use atvprep_manager::library::LibraryRoots;
use atvprep_manager::media::Media;
use atvprep_manager::models::MediaMeta;
use atvprep_manager::repo::Ctx;
use atvprep_manager::schema::migrate;
use atvprep_manager::state_machine::{ItemStatus, JobStateMachine};

const T0: i64 = 1_700_000_000_000_000;
const SECOND: i64 = 1_000_000;

fn setup() -> (Db, Conn) {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    migrate(&mut conn).unwrap();
    (db, conn)
}

fn at<'a>(conn: &'a Conn, clock: &'a FixedClock) -> Ctx<'a> {
    Ctx {
        conn: conn.raw(),
        clock,
    }
}

#[test]
fn only_one_worker_holds_the_lease_and_tokens_only_grow() {
    let (_db, conn) = setup();
    let clock = FixedClock(T0);
    let mut first = MediaExecutorLease::new(at(&conn, &clock));
    let mut second = MediaExecutorLease::new(at(&conn, &clock));
    assert!(first.acquire("a", 30).unwrap());
    assert_eq!(first.fencing_token, Some(1));
    assert!(!second.acquire("b", 30).unwrap());
    assert_eq!(second.fencing_token, None);

    // Once the lease has run out, the next worker takes over with a larger token.
    let later = FixedClock(T0 + 31 * SECOND);
    let mut takeover = MediaExecutorLease::new(at(&conn, &later));
    assert!(takeover.acquire("b", 30).unwrap());
    assert_eq!(takeover.fencing_token, Some(2));
    assert!(
        !MediaExecutorLease::new(at(&conn, &later))
            .heartbeat(first.fencing_token, 30)
            .unwrap()
    );
    assert!(
        !first.release().unwrap(),
        "a stale holder cannot release the new holder's lease"
    );
    assert!(takeover.release().unwrap());
    let mut third = MediaExecutorLease::new(at(&conn, &later));
    assert!(third.acquire("c", 30).unwrap());
    assert_eq!(third.fencing_token, Some(3));
}

#[test]
fn a_heartbeat_extends_a_live_lease_only() {
    let (_db, conn) = setup();
    let clock = FixedClock(T0);
    let mut lease = MediaExecutorLease::new(at(&conn, &clock));
    assert!(!lease.heartbeat(None, 30).unwrap(), "nothing acquired yet");
    assert!(lease.acquire("a", 30).unwrap());
    let halfway = FixedClock(T0 + 20 * SECOND);
    let renewing = MediaExecutorLease::new(at(&conn, &halfway));
    assert!(renewing.heartbeat(lease.fencing_token, 30).unwrap());
    // Renewed at +20 s for 30 s: still held at +45 s, so nobody else gets it.
    let near_end = FixedClock(T0 + 45 * SECOND);
    assert!(
        !MediaExecutorLease::new(at(&conn, &near_end))
            .acquire("b", 30)
            .unwrap()
    );
    // The wrong token never extends it; an expired lease cannot be revived by its old holder.
    assert!(!renewing.heartbeat(Some(99), 30).unwrap());
    let expired = FixedClock(T0 + 60 * SECOND);
    assert!(
        !MediaExecutorLease::new(at(&conn, &expired))
            .heartbeat(lease.fencing_token, 30)
            .unwrap()
    );
}

#[test]
fn released_leases_can_be_taken_at_once() {
    let (_db, conn) = setup();
    let clock = FixedClock(T0);
    let mut a = MediaExecutorLease::new(at(&conn, &clock));
    assert!(!a.release().unwrap());
    assert!(a.acquire("a", 3600).unwrap());
    assert!(a.release().unwrap());
    let mut b = MediaExecutorLease::new(at(&conn, &clock));
    assert!(b.acquire("b", 30).unwrap());
    assert_eq!(b.fencing_token, Some(2));
}

#[test]
fn two_workers_racing_for_the_lease_have_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("lease.db")).unwrap();
    migrate(&mut db.conn().unwrap()).unwrap();
    let wins: Vec<bool> = std::thread::scope(|scope| {
        let handles: Vec<_> = ["a", "b", "c", "d"]
            .into_iter()
            .map(|owner| {
                let db = db.clone();
                scope.spawn(move || {
                    let mut conn = db.conn().unwrap();
                    conn.transaction(|tx| {
                        let clock = FixedClock(T0);
                        MediaExecutorLease::new(Ctx {
                            conn: tx,
                            clock: &clock,
                        })
                        .acquire(owner, 30)
                    })
                    .unwrap()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(wins.iter().filter(|w| **w).count(), 1, "{wins:?}");
}

#[test]
fn a_worker_that_lost_the_lease_cannot_commit_late_results() {
    let (_db, conn) = setup();
    let clock = FixedClock(T0);
    let ctx = at(&conn, &clock);
    let root = LibraryRoots(ctx).create("library", "/library").unwrap().id;
    let meta = MediaMeta {
        kind: "video".into(),
        suffix: ".mp4".into(),
        size: 1,
        mtime_ns: 1,
        fingerprint: "f".into(),
        scanned_device: Some(1),
        scanned_inode: Some(1),
        scanned_ctime_ns: Some(1),
        ..MediaMeta::default()
    };
    let media = Media(ctx).create(root, "a.mp4", &meta).unwrap().id;
    let job = Jobs(ctx).create(&NewJob::new("fix", &[media])).unwrap();

    let mut old = MediaExecutorLease::new(ctx);
    assert!(old.acquire("old", 30).unwrap());
    let token = old.fencing_token.unwrap();
    let item = Jobs(ctx)
        .claim_media_item(job.id, "old", token, 30, 1)
        .unwrap()
        .unwrap();
    JobStateMachine(ctx)
        .transition_item(
            item.id,
            ItemStatus::Running,
            item.status_version,
            Some(token),
            None,
        )
        .unwrap();

    // The old worker stalls past its lease; a new worker takes over.
    let later = FixedClock(T0 + 40 * SECOND);
    let later_ctx = at(&conn, &later);
    let mut new = MediaExecutorLease::new(later_ctx);
    assert!(new.acquire("new", 30).unwrap());
    assert_eq!(later.now_micros(), T0 + 40 * SECOND);

    let machine = JobStateMachine(later_ctx);
    let late = machine.transition_item(
        item.id,
        ItemStatus::Succeeded,
        item.status_version + 1,
        Some(token),
        None,
    );
    assert!(matches!(late, Err(Error::Conflict(_))), "{late:?}");
    assert!(!Jobs(later_ctx).heartbeat_item(item.id, token, 30).unwrap());
    // Progress and log tail are fenced by token and status only, as in Python; the lease guards
    // heartbeats, claims and state changes.
    assert!(
        Jobs(later_ctx)
            .set_item_progress(item.id, token, 50)
            .unwrap()
    );
    assert!(
        Jobs(later_ctx)
            .claim_item(
                job.id,
                "old",
                token,
                30,
                &ClaimLimits {
                    global_limit_key: Some("max_ffmpeg_workers"),
                    global_limit_default: 1,
                    ..ClaimLimits::default()
                }
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(
        Jobs(later_ctx).items(job.id).unwrap()[0].status,
        ItemStatus::Running
    );
}
