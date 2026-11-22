//! A short database error while a worker runs must not cost it its lease (#132, #134).

use atvprep_manager::{
    clock::Clock,
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
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::{Duration, Instant},
};

const T0: i64 = 1_700_000_000_000_000;
const SECOND: i64 = 1_000_000;

/// A clock the test moves by hand, so that no test waits for a lease to run out.
#[derive(Clone)]
struct StepClock(Arc<AtomicI64>);

impl StepClock {
    fn new() -> Self {
        Self(Arc::new(AtomicI64::new(T0)))
    }

    fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds * SECOND, Ordering::SeqCst);
    }
}

impl Clock for StepClock {
    fn now_micros(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

const REPORT: &str = r#"{"program_version":{"version":"7.1.1"},"streams":[{"index":0,"codec_type":"video","codec_name":"h264","disposition":{"default":1,"forced":0}}],"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"5.0"}}"#;

/// A library with one file, a file database and a stand-in tool that announces its start, runs
/// for a while and then reports.
struct Library {
    _directory: tempfile::TempDir,
    root: PathBuf,
    db: Db,
    conn: Conn,
}

impl Library {
    fn new(run_seconds: u32) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("library")).unwrap();
        std::fs::write(root.join("library/film.mp4"), b"not really a film").unwrap();
        atvprep_testkit::write_script(
            &root.join("tool"),
            format!(
                "#!/bin/sh\ntouch \"$(dirname \"$0\")/started\"\nsleep {run_seconds}\ncat <<'EOF'\n{REPORT}\nEOF\n"
            ),
        );
        // A short busy timeout, so that a held lock shows up as an error within the test.
        let db = Db::open(&root.join("manager.db"))
            .unwrap()
            .with_busy_timeout(Duration::from_millis(100));
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        Self {
            _directory: directory,
            root,
            db,
            conn,
        }
    }

    /// Run one analysis item while `disturb` runs beside it once the tool has started.
    fn run(
        &self,
        clock: &StepClock,
        disturb: impl FnOnce(&Db, &StepClock) + Send,
    ) -> (ItemStatus, String) {
        let ctx = Ctx {
            conn: self.conn.raw(),
            clock,
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
        let job = Jobs(ctx)
            .create(&NewJob::new("inspect".parse().unwrap(), &[media.id]))
            .unwrap();
        let mut lease = MediaExecutorLease::new(ctx);
        assert!(lease.acquire("worker", 30).unwrap());
        let fence = lease.fencing_token.unwrap();
        let item = Jobs(ctx)
            .claim_media_item(job.id, "worker", fence, 30, 1)
            .unwrap()
            .unwrap();
        let config = ExecutionConfig {
            executable: self.root.join("tool"),
            environment: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            timeout: Duration::from_secs(20),
            grace: Duration::from_millis(100),
            lease_seconds: 30,
            shutdown: None,
            approvals: None,
            session_idle_seconds: 3600,
        };
        let started = self.root.join("started");
        let db = self.db.clone();
        let stepped = clock.clone();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                while !started.exists() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                disturb(&db, &stepped);
            });
            let _ = execute_claimed(ctx, item.id, "worker", fence, &config);
        });
        let item = Jobs(ctx).get_item(item.id).unwrap().unwrap();
        (item.status, item.log_tail.unwrap_or_default())
    }
}

#[test]
fn a_database_that_is_briefly_busy_does_not_cost_the_worker_its_lease() {
    let library = Library::new(4);
    let (status, log) = library.run(&StepClock::new(), |db, _| {
        // A second writer holds the database across at least one heartbeat.
        let other = db.conn().unwrap();
        other.raw().execute_batch("BEGIN IMMEDIATE").unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        other.raw().execute_batch("COMMIT").unwrap();
    });
    assert_eq!(status, ItemStatus::Succeeded, "{log}");
}

#[test]
fn an_overtaken_fencing_token_ends_the_run_at_once() {
    let library = Library::new(10);
    let began = Instant::now();
    let (status, log) = library.run(&StepClock::new(), |db, _| {
        let other = db.conn().unwrap();
        other
            .raw()
            .execute(
                "UPDATE media_executor_leases SET fencing_token=fencing_token+1",
                [],
            )
            .unwrap();
    });
    assert_ne!(status, ItemStatus::Succeeded, "{log}");
    assert!(
        began.elapsed() < Duration::from_secs(6),
        "stopped at once: {log}"
    );
    // Without the lease nothing may be written any more: the item is left to recovery.
    assert_eq!(status, ItemStatus::Running, "{log}");
}

#[test]
fn an_expired_lease_ends_the_run_at_once() {
    let library = Library::new(10);
    let began = Instant::now();
    let (status, log) = library.run(&StepClock::new(), |_, clock| clock.advance(31));
    assert_ne!(status, ItemStatus::Succeeded, "{log}");
    assert!(
        began.elapsed() < Duration::from_secs(6),
        "stopped at once: {log}"
    );
    // Without the lease nothing may be written any more: the item is left to recovery.
    assert_eq!(status, ItemStatus::Running, "{log}");
}

#[test]
fn database_errors_beyond_the_tolerance_end_the_run_as_failed_and_leave_the_file_alone() {
    let library = Library::new(10);
    let began = Instant::now();
    let (status, log) = library.run(&StepClock::new(), |db, clock| {
        let other = db.conn().unwrap();
        other.raw().execute_batch("BEGIN IMMEDIATE").unwrap();
        // 30 s lease minus 10 s: 19 s without a heartbeat is still tolerated, 21 s is not.
        clock.advance(19);
        std::thread::sleep(Duration::from_millis(1500));
        clock.advance(2);
        std::thread::sleep(Duration::from_millis(2500));
        other.raw().execute_batch("COMMIT").unwrap();
    });
    assert_eq!(status, ItemStatus::Failed, "{log}");
    assert!(
        began.elapsed() < Duration::from_secs(8),
        "stopped early: {log}"
    );
    assert_eq!(
        std::fs::read(library.root.join("library/film.mp4")).unwrap(),
        b"not really a film"
    );
}
