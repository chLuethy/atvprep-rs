//! Library, media, scan, sidecar and inspection repositories (ported from
//! `tests/manager/db/test_media_repository.py`).

use std::path::PathBuf;

use atvprep_manager::Error;
use atvprep_manager::clock::{Clock, FixedClock};
use atvprep_manager::db::{Conn, Db};
use atvprep_manager::inspections::{Inspections, NewInspection};
use atvprep_manager::library::LibraryRoots;
use atvprep_manager::media::Media;
use atvprep_manager::models::MediaMeta;
use atvprep_manager::repo::Ctx;
use atvprep_manager::scans::Scans;
use atvprep_manager::schema::migrate;
use atvprep_manager::sidecars::Sidecars;
use atvprep_manager::state_machine::ScanStatus;
use serde_json::{Value, json};

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

    fn root(&self) -> i64 {
        LibraryRoots(self.ctx())
            .create("library", "/library")
            .unwrap()
            .id
    }

    /// A scheduled scan claimed by `worker` under a scheduler lease with fencing token 1.
    fn scheduled_scan(&self, root: i64) -> i64 {
        let expires = atvprep_manager::clock::format_micros(T0 + 60_000_000);
        self.conn
            .raw()
            .execute(
                "INSERT OR REPLACE INTO scheduler_leases(id, owner_id, lease_expires_at, fencing_token)
                 VALUES (1, 'worker', ?1, 1)",
                [expires],
            )
            .unwrap();
        self.conn
            .raw()
            .execute(
                "INSERT INTO scan_runs(root_id, generation, trigger, status, claimed_by, fencing_token)
                 VALUES (?1, 1, 'scheduled', 'running', 'worker', 1)",
                [root],
            )
            .unwrap();
        self.conn.raw().last_insert_rowid()
    }
}

fn meta(fingerprint: &str) -> MediaMeta {
    MediaMeta {
        kind: "video".into(),
        suffix: ".mp4".into(),
        size: 1,
        mtime_ns: 1,
        fingerprint: fingerprint.into(),
        scanned_device: Some(7),
        scanned_inode: Some(8),
        scanned_ctime_ns: Some(9),
        ..MediaMeta::default()
    }
}

fn inspection<'a>(tool: &'a str, fingerprint: &'a str, result: &'a Value) -> NewInspection<'a> {
    NewInspection {
        source_fingerprint: fingerprint,
        input_fingerprint: fingerprint,
        tool_name: tool,
        tool_version: "1",
        result,
    }
}

#[test]
fn relative_path_is_unique_per_root_but_not_globally() {
    let f = Fixture::new();
    let roots = LibraryRoots(f.ctx());
    let (a, b) = (
        roots.create("a", "/a").unwrap(),
        roots.create("b", "/b").unwrap(),
    );
    let media = Media(f.ctx());
    media.create(a.id, "movie.mp4", &meta("1")).unwrap();
    media.create(b.id, "movie.mp4", &meta("1")).unwrap();
    let again = media.create(a.id, "movie.mp4", &meta("1"));
    assert!(matches!(again, Err(Error::Constraint(_))), "{again:?}");
}

#[test]
fn media_persists_scanner_file_identity_and_rejects_incomplete_identity() {
    let f = Fixture::new();
    let root = f.root();
    let media = Media(f.ctx());
    let saved = media
        .upsert(root, "movie.mp4", &meta("1:2"), None)
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            saved.scanned_device,
            saved.scanned_inode,
            saved.scanned_ctime_ns
        ),
        (Some(7), Some(8), Some(9))
    );
    let partial = [
        (None, None, None),
        (Some(7), None, None),
        (Some(7), Some(8), None),
        (Some(7), None, Some(9)),
        (Some(-1), Some(8), Some(9)),
    ];
    for (device, inode, ctime) in partial {
        let bad = MediaMeta {
            scanned_device: device,
            scanned_inode: inode,
            scanned_ctime_ns: ctime,
            ..meta("x")
        };
        for result in [
            media.create(root, "other.mp4", &bad).map(|_| ()),
            media.upsert(root, "other.mp4", &bad, None).map(|_| ()),
        ] {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("scan identity"), "{error}");
        }
    }
}

#[test]
fn a_completed_scan_marks_unseen_media_missing_but_an_aborted_one_does_not() {
    let f = Fixture::new();
    let root = f.root();
    let (media, scans) = (Media(f.ctx()), Scans(f.ctx()));
    let old = media
        .upsert(root, "old.mp4", &meta("old"), None)
        .unwrap()
        .unwrap();
    let scan = scans.start(root, 1, "scheduled").unwrap();
    scans
        .record_seen(scan.id, root, "new.mp4", &meta("new"))
        .unwrap();
    scans.finish(scan.id, ScanStatus::Succeeded, true).unwrap();
    assert_eq!(
        media.get(old.id).unwrap().unwrap().discovery_state,
        "missing"
    );
    let present = media
        .upsert(root, "present.mp4", &meta("p"), None)
        .unwrap()
        .unwrap();
    let second = scans.start(root, 2, "scheduled").unwrap();
    scans
        .finish(second.id, ScanStatus::Interrupted, true)
        .unwrap();
    assert_eq!(
        media.get(old.id).unwrap().unwrap().discovery_state,
        "missing"
    );
    assert_eq!(
        media.get(present.id).unwrap().unwrap().discovery_state,
        "present"
    );
}

#[test]
fn scan_statuses_are_normalised_and_invalid_ones_rejected() {
    let f = Fixture::new();
    let root = f.root();
    let scans = Scans(f.ctx());
    let started = scans.start(root, 1, "manual").unwrap();
    assert_eq!(
        (started.status.as_str(), started.trigger.as_str()),
        ("running", "manual")
    );
    assert_eq!(
        (
            started.attempt,
            started.status_version,
            started.fencing_token
        ),
        (0, 0, 0)
    );
    assert_eq!(
        scans
            .finish(started.id, ScanStatus::Succeeded, true)
            .unwrap()
            .status,
        ScanStatus::Succeeded
    );
    let second = scans.start(root, 2, "scheduled").unwrap();
    assert_eq!(
        scans
            .finish(second.id, ScanStatus::Interrupted, true)
            .unwrap()
            .status,
        ScanStatus::Interrupted
    );
    let third = scans.start(root, 3, "scheduled").unwrap();
    let error = scans
        .finish(third.id, ScanStatus::Running, true)
        .unwrap_err();
    assert_eq!(error.to_string(), "invalid scan status");
    let unknown = "completed-ish".parse::<ScanStatus>().unwrap_err();
    assert_eq!(unknown.to_string(), "unknown scan status: completed-ish");
    assert_eq!(
        scans.start(root, 4, "bogus").unwrap_err().to_string(),
        "invalid scan trigger"
    );
}

#[test]
fn manual_scan_requests_queue_the_next_generation_and_share_one_queued_scan() {
    let f = Fixture::new();
    let root = f.root();
    let scans = Scans(f.ctx());
    scans.start(root, 1, "scheduled").unwrap();
    let requested = scans.request_manual(root).unwrap();
    assert_eq!(
        (requested.generation, requested.status.as_str()),
        (2, "queued")
    );
    assert_eq!(
        (requested.trigger.as_str(), requested.claimed_by),
        ("manual", None)
    );
    assert_eq!(scans.request_manual(root).unwrap().id, requested.id);
    f.conn
        .raw()
        .execute(
            "UPDATE scan_runs SET status='running' WHERE id=?1",
            [requested.id],
        )
        .unwrap();
    let next = scans.request_manual(root).unwrap();
    assert_ne!(next.id, requested.id);
    assert_eq!(next.generation, 3);
    assert_eq!(
        scans.request_manual(999).unwrap_err().to_string(),
        "unknown root id: 999"
    );
}

#[test]
fn an_older_scan_cannot_hide_media_seen_by_a_newer_scan() {
    let f = Fixture::new();
    let root = f.root();
    let scans = Scans(f.ctx());
    let older = scans.start(root, 1, "scheduled").unwrap();
    let newer = scans.start(root, 2, "scheduled").unwrap();
    let seen = scans
        .record_seen(newer.id, root, "newer.mp4", &meta("newer"))
        .unwrap()
        .unwrap();
    scans.finish(older.id, ScanStatus::Succeeded, true).unwrap();
    assert_eq!(
        Media(f.ctx())
            .get(seen.id)
            .unwrap()
            .unwrap()
            .discovery_state,
        "present"
    );
    scans.finish(newer.id, ScanStatus::Succeeded, true).unwrap();
    assert_eq!(
        Media(f.ctx())
            .get(seen.id)
            .unwrap()
            .unwrap()
            .discovery_state,
        "present"
    );
}

#[test]
fn a_stale_scan_cannot_overwrite_media_sidecar_or_inspection() {
    let f = Fixture::new();
    let root = f.root();
    let scans = Scans(f.ctx());
    let older = scans.start(root, 1, "scheduled").unwrap();
    let newer = scans.start(root, 2, "scheduled").unwrap();
    let current = MediaMeta {
        size: 2,
        ..meta("new")
    };
    let current = scans
        .record_seen(newer.id, root, "movie.mp4", &current)
        .unwrap()
        .unwrap();
    let (sidecars, inspections) = (Sidecars(f.ctx()), Inspections(f.ctx()));
    sidecars
        .upsert(current.id, "movie.en.srt", "new-side", 0, 0, Some(newer.id))
        .unwrap()
        .unwrap();
    let probe = json!({"generation": 2});
    inspections
        .record(
            current.id,
            &inspection("ffprobe", "new", &probe),
            Some(newer.id),
        )
        .unwrap()
        .unwrap();

    assert!(
        scans
            .record_seen(older.id, root, "movie.mp4", &meta("old"))
            .unwrap()
            .is_none()
    );
    assert!(
        sidecars
            .upsert(current.id, "movie.en.srt", "old-side", 0, 0, Some(older.id))
            .unwrap()
            .is_none()
    );
    let stale = json!({"generation": 1});
    assert!(
        inspections
            .record(
                current.id,
                &inspection("ffprobe", "old", &stale),
                Some(older.id)
            )
            .unwrap()
            .is_none()
    );
    let loaded = Media(f.ctx()).get(current.id).unwrap().unwrap();
    assert_eq!(
        (loaded.fingerprint.as_str(), loaded.last_seen_scan_id),
        ("new", Some(newer.id))
    );
    assert_eq!(
        sidecars.list_for_media(current.id).unwrap()[0].fingerprint,
        "new-side"
    );
}

#[test]
fn a_scheduled_scan_that_lost_its_lease_persists_nothing() {
    assert_lost_scan_fenced("scheduled");
}

#[test]
fn a_claimed_manual_scan_that_lost_its_lease_persists_nothing() {
    assert_lost_scan_fenced("manual");
}

fn assert_lost_scan_fenced(trigger: &str) {
    let f = Fixture::new();
    let root = f.root();
    let scan = f.scheduled_scan(root);
    f.conn
        .raw()
        .execute(
            "UPDATE scan_runs SET trigger=?1 WHERE id=?2",
            rusqlite::params![trigger, scan],
        )
        .unwrap();
    let media = Media(f.ctx())
        .upsert(root, "movie.mp4", &meta("current"), None)
        .unwrap()
        .unwrap();
    let scans = Scans(f.ctx());
    // While the lease is held the scan may write.
    assert!(
        scans
            .record_seen(scan, root, "movie.mp4", &meta("fresh"))
            .unwrap()
            .is_some()
    );

    // The lease is taken over (new owner, new fencing token): everything is refused.
    f.conn
        .raw()
        .execute(
            "UPDATE scheduler_leases SET owner_id='other', fencing_token=2",
            [],
        )
        .unwrap();
    assert!(
        scans
            .record_seen(scan, root, "movie.mp4", &meta("stale"))
            .unwrap()
            .is_none()
    );
    assert!(
        Sidecars(f.ctx())
            .upsert(media.id, "movie.en.srt", "stale", 0, 0, Some(scan))
            .unwrap()
            .is_none()
    );
    let result = json!({});
    assert!(
        Inspections(f.ctx())
            .record(
                media.id,
                &inspection("ffprobe", "stale", &result),
                Some(scan)
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(
        scans
            .finish(scan, ScanStatus::Succeeded, true)
            .unwrap()
            .status,
        ScanStatus::Running
    );
    assert_eq!(
        Media(f.ctx()).get(media.id).unwrap().unwrap().fingerprint,
        "fresh"
    );
}

#[test]
fn an_expired_scheduler_lease_rejects_scan_persistence_without_a_takeover() {
    let f = Fixture::new();
    let root = f.root();
    let scan = f.scheduled_scan(root);
    let expired = atvprep_manager::clock::format_micros(T0 - 1_000_000);
    f.conn
        .raw()
        .execute("UPDATE scheduler_leases SET lease_expires_at=?1", [expired])
        .unwrap();
    let scans = Scans(f.ctx());
    assert!(!scans.is_authoritative(scan, root).unwrap());
    assert!(
        scans
            .record_seen(scan, root, "movie.mp4", &meta("x"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        scans
            .finish(scan, ScanStatus::Succeeded, true)
            .unwrap()
            .status,
        ScanStatus::Running
    );
}

#[test]
fn record_seen_validates_the_scan_and_root() {
    let f = Fixture::new();
    let root = f.root();
    let other = LibraryRoots(f.ctx()).create("other", "/other").unwrap().id;
    let scans = Scans(f.ctx());
    let scan = scans.start(root, 1, "scheduled").unwrap();
    assert_eq!(
        scans
            .record_seen(999, root, "a.mp4", &meta("1"))
            .unwrap_err()
            .to_string(),
        "unknown scan id: 999"
    );
    let mismatch = scans
        .record_seen(scan.id, other, "a.mp4", &meta("1"))
        .unwrap_err();
    assert_eq!(
        mismatch.to_string(),
        format!("scan {} does not belong to root {other}", scan.id)
    );
}

#[test]
fn sidecars_inspections_and_the_conversion_link_persist_provenance() {
    let f = Fixture::new();
    let root = f.root();
    let media = Media(f.ctx());
    let source = media
        .upsert(root, "source.mp4", &meta("source-v1"), None)
        .unwrap()
        .unwrap();
    let output = media
        .upsert(root, "output.mp4", &meta("output-v1"), None)
        .unwrap()
        .unwrap();
    media
        .set_conversion(source.id, output.id, "source-v1")
        .unwrap();
    assert_eq!(
        media
            .set_conversion(999, output.id, "x")
            .unwrap_err()
            .to_string(),
        "unknown media id: 999"
    );
    let sidecars = Sidecars(f.ctx());
    sidecars
        .upsert(source.id, "source.en.srt", "side-v1", 0, 0, None)
        .unwrap();
    sidecars
        .upsert(source.id, "source.en.srt", "side-v2", 5, 6, None)
        .unwrap();
    let ok = json!({"ok": true});
    let made = NewInspection {
        source_fingerprint: "source-v1",
        input_fingerprint: "input-v1",
        tool_name: "ffprobe",
        tool_version: "7.0",
        result: &ok,
    };
    let recorded = Inspections(f.ctx())
        .record(source.id, &made, None)
        .unwrap()
        .unwrap();
    let loaded = Inspections(f.ctx()).latest(source.id).unwrap().unwrap();
    assert_eq!(
        (loaded.id, loaded.source_fingerprint.as_str()),
        (recorded.id, "source-v1")
    );
    assert_eq!(
        (loaded.input_fingerprint.as_str(), loaded.tool_name.as_str()),
        ("input-v1", "ffprobe")
    );
    assert_eq!(loaded.result, ok);
    let listed = sidecars.list_for_media(source.id).unwrap();
    assert_eq!(
        (listed.len(), listed[0].fingerprint.as_str(), listed[0].size),
        (1, "side-v2", 5)
    );
    let converted = media.get(source.id).unwrap().unwrap();
    assert_eq!(converted.conversion_output_media_id, Some(output.id));
    assert_eq!(
        converted.last_converted_input_fingerprint.as_deref(),
        Some("source-v1")
    );
}

fn compliance(f: &Fixture, id: i64) -> (String, Vec<String>) {
    let m = Media(f.ctx()).get(id).unwrap().unwrap();
    (m.compliance_state, m.compliance_reasons)
}

fn blocker() -> Value {
    json!({"passed": false, "violations": [
        {"rule": "no-fragments", "severity": "blocker", "message": "fragmented MP4"}]})
}

#[test]
fn compliance_is_projected_conservatively_and_a_verify_verdict_survives_ffprobe() {
    let f = Fixture::new();
    let root = f.root();
    let media = Media(f.ctx())
        .create(root, "movie.mp4", &meta("fp-1"))
        .unwrap();
    let inspections = Inspections(f.ctx());
    let streams = json!({"streams": []});
    inspections
        .record(media.id, &inspection("ffprobe", "fp-1", &streams), None)
        .unwrap();
    assert_eq!(
        compliance(&f, media.id),
        ("unknown".to_owned(), vec![
            "Current ffprobe inspection is available, but AVPlayer compliance requires a verify report.".to_owned()
        ])
    );
    let clean = json!({"passed": true, "violations": []});
    inspections
        .record(media.id, &inspection("verify", "fp-1", &clean), None)
        .unwrap();
    assert_eq!(compliance(&f, media.id).0, "clean");
    // A later verify blocker, then an ffprobe run: the verdict for the same bytes stays.
    let verdict = blocker();
    inspections
        .record(media.id, &inspection("verify", "fp-1", &verdict), None)
        .unwrap();
    inspections
        .record(media.id, &inspection("ffprobe", "fp-1", &streams), None)
        .unwrap();
    assert_eq!(
        compliance(&f, media.id),
        (
            "blocker".to_owned(),
            vec!["[blocker] no-fragments: fragmented MP4".to_owned()]
        )
    );
    f.conn
        .raw()
        .execute(
            "UPDATE media_files SET compliance_state='unknown', compliance_reasons='[\"stale\"]'",
            [],
        )
        .unwrap();
    inspections.backfill_compliance().unwrap();
    assert_eq!(compliance(&f, media.id).0, "blocker");
}

fn needs_reencode(f: &Fixture, id: i64) -> bool {
    Media(f.ctx()).get(id).unwrap().unwrap().needs_reencode
}

#[test]
fn the_re_encode_need_is_stored_with_the_check_result_independent_of_the_state() {
    let f = Fixture::new();
    let root = f.root();
    let media = Media(f.ctx())
        .create(root, "old.mkv", &meta("fp-1"))
        .unwrap();
    let inspections = Inspections(f.ctx());
    assert!(!needs_reencode(&f, media.id));
    // A pure media analysis: state unknown, but the need is known.
    let vc1 = json!({"streams": [{"codec_type": "video", "codec_name": "vc1"}]});
    inspections
        .record(media.id, &inspection("ffprobe", "fp-1", &vc1), None)
        .unwrap();
    assert_eq!(compliance(&f, media.id).0, "unknown");
    assert!(needs_reencode(&f, media.id));
    // A verify report: state plus need, whatever the state.
    let report = json!({"violations": [
        {"rule": "video-reencode", "severity": "warning", "message": "video vc1 needs a re-encode"}]});
    inspections
        .record(media.id, &inspection("verify", "fp-1", &report), None)
        .unwrap();
    assert_eq!(compliance(&f, media.id).0, "warning");
    assert!(needs_reencode(&f, media.id));
    // The need goes away with a clean report, and backfill reproduces it.
    let clean = json!({"violations": []});
    inspections
        .record(media.id, &inspection("verify", "fp-1", &clean), None)
        .unwrap();
    assert!(!needs_reencode(&f, media.id));
    f.conn
        .raw()
        .execute("UPDATE media_files SET needs_reencode=1", [])
        .unwrap();
    inspections.backfill_compliance().unwrap();
    assert!(!needs_reencode(&f, media.id));
}

#[test]
fn a_verify_verdict_for_older_bytes_is_not_carried_over_to_a_changed_file() {
    let f = Fixture::new();
    let root = f.root();
    let media = Media(f.ctx())
        .create(root, "movie.mp4", &meta("fp-old"))
        .unwrap();
    let inspections = Inspections(f.ctx());
    let clean = json!({"passed": true, "violations": []});
    inspections
        .record(media.id, &inspection("verify", "fp-old", &clean), None)
        .unwrap();
    assert_eq!(compliance(&f, media.id).0, "clean");
    f.conn
        .raw()
        .execute("UPDATE media_files SET fingerprint='fp-new'", [])
        .unwrap();
    let streams = json!({"streams": []});
    inspections
        .record(media.id, &inspection("ffprobe", "fp-new", &streams), None)
        .unwrap();
    assert_eq!(compliance(&f, media.id).0, "unknown");
}

#[test]
fn backfill_uses_the_latest_inspection_and_explains_missing_data() {
    let f = Fixture::new();
    let root = f.root();
    let media = Media(f.ctx())
        .create(root, "verified.mp4", &meta("current"))
        .unwrap();
    let bare = Media(f.ctx())
        .create(root, "uninspected.mp4", &meta("uninspected"))
        .unwrap();
    let inspections = Inspections(f.ctx());
    let stale =
        json!({"violations": [{"rule": "fast-start", "severity": "warning", "message": "stale"}]});
    inspections
        .record(media.id, &inspection("verify", "old", &stale), None)
        .unwrap();
    let clean = json!({"violations": []});
    inspections
        .record(media.id, &inspection("verify", "current", &clean), None)
        .unwrap();
    f.conn
        .raw()
        .execute(
            "UPDATE media_files SET compliance_state='unknown', compliance_reasons='[]'",
            [],
        )
        .unwrap();
    assert_eq!(inspections.backfill_compliance().unwrap(), 2);
    assert_eq!(
        compliance(&f, media.id),
        (
            "clean".to_owned(),
            vec!["No compliance violations reported by verify.".to_owned()]
        )
    );
    assert_eq!(
        compliance(&f, bare.id),
        (
            "unknown".to_owned(),
            vec!["No inspection is available for this media file.".to_owned()]
        )
    );
}

#[test]
fn configured_libraries_are_registered_once_and_existing_ones_are_kept() {
    let f = Fixture::new();
    let roots = LibraryRoots(f.ctx());
    let both = [
        PathBuf::from("/mnt/media/Film"),
        PathBuf::from("/mnt/media/TV"),
    ];
    assert_eq!(
        (
            roots.ensure_configured(&both).unwrap(),
            roots.ensure_configured(&both).unwrap()
        ),
        (2, 0)
    );
    let names: Vec<String> = f
        .conn
        .raw()
        .prepare("SELECT name FROM library_roots ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(names, ["Film", "TV"]);
    f.conn
        .raw()
        .execute("UPDATE library_roots SET enabled=0 WHERE name='Film'", [])
        .unwrap();
    let more = [
        PathBuf::from("/mnt/media/Film"),
        PathBuf::from("/srv/other/Film"),
        PathBuf::from("/"),
    ];
    assert_eq!(roots.ensure_configured(&more).unwrap(), 2);
    let film = roots.get(1).unwrap().unwrap();
    assert!(!film.enabled);
    let by_path = |p: &str| -> String {
        f.conn
            .raw()
            .query_row(
                "SELECT name FROM library_roots WHERE canonical_path=?1",
                [p],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(
        (by_path("/srv/other/Film").as_str(), by_path("/").as_str()),
        ("Film (2)", "/")
    );
}

#[test]
fn a_dismissed_missing_file_is_shown_again_when_a_scan_finds_it() {
    let f = Fixture::new();
    let root = f.root();
    let (media, scans) = (Media(f.ctx()), Scans(f.ctx()));
    let gone = media
        .upsert(root, "gone.mp4", &meta("g"), None)
        .unwrap()
        .unwrap();
    let first = scans.start(root, 1, "scheduled").unwrap();
    scans.finish(first.id, ScanStatus::Succeeded, true).unwrap();
    assert_eq!(
        (
            media.dismiss_missing().unwrap(),
            media.dismiss_missing().unwrap()
        ),
        (1, 0)
    );
    let hidden = media.get(gone.id).unwrap().unwrap();
    assert!(hidden.discovery_state == "missing" && hidden.dismissed_at.is_some());

    let second = scans.start(root, 2, "scheduled").unwrap();
    scans
        .record_seen(second.id, root, "gone.mp4", &meta("g"))
        .unwrap();
    scans
        .finish(second.id, ScanStatus::Succeeded, true)
        .unwrap();
    assert_eq!(
        media.get(gone.id).unwrap().unwrap().discovery_state,
        "present"
    );
    let third = scans.start(root, 3, "scheduled").unwrap();
    scans.finish(third.id, ScanStatus::Succeeded, true).unwrap();
    let again = media.get(gone.id).unwrap().unwrap();
    assert!(again.discovery_state == "missing" && again.dismissed_at.is_none());
    let _ = f.clock.now();
}
