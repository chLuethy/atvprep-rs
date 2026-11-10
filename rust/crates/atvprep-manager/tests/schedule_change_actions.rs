//! Schedules that change files (fix, convert): queueing needs an approval, and missed runs are never caught up.
use atvprep_manager::{
    clock::{Clock, FixedClock},
    db::{Conn, Db},
    repo::Ctx,
    schedules::{run_due_schedules, run_schedule},
    schema,
};
use chrono::{DateTime, Duration, Local, Utc};

const NOW: i64 = 1_700_000_000_000_000;

/// One warning mkv and one clean mp4 in a library, plus a schedule of `action` whose slot was
/// `minutes_ago` minutes before NOW. `approved` stores a (placeholder) approval.
fn seed(db: &Db, action: &str, minutes_ago: i64, approved: bool) -> Conn {
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'test','/library');
    INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
    (1,1,'one.mp4','video','.mp4',1,1,'one','present','clean','idle','none'),
    (2,1,'two.mp4','video','.mp4',1,1,'two','present','warning','idle','none'),
    (3,1,'three.mkv','video','.mkv',1,1,'three','present','warning','idle','none');").unwrap();
    let time = (DateTime::<Utc>::from_timestamp_micros(NOW)
        .unwrap()
        .with_timezone(&Local)
        - Duration::minutes(minutes_ago))
    .format("%H:%M")
    .to_string();
    conn.raw().execute("INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,max_files,enabled,armed_at) VALUES(1,'t',?1,1,127,?2,5,1,'2000-01-01')",rusqlite::params![action,time]).unwrap();
    if approved {
        conn.raw()
            .execute(
                "UPDATE schedules SET approved_by=1, approved_at=1700000000, approval_mac=?1",
                ["a".repeat(64)],
            )
            .unwrap();
    }
    conn
}

fn jobs(conn: &Conn) -> Vec<(String, String)> {
    let mut statement = conn
        .raw()
        .prepare("SELECT action, settings_snapshot_json FROM jobs ORDER BY id")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn an_unapproved_schedule_queues_nothing_even_when_run_by_hand() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "fix", 0, false);
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    let message = run_schedule(ctx, 1).unwrap();
    assert!(message.starts_with("Not queued: "), "{message}");
    assert!(jobs(&conn).is_empty());
    assert_eq!(run_due_schedules(ctx).unwrap(), 1);
    assert!(jobs(&conn).is_empty());
}

#[test]
fn an_approved_fix_schedule_queues_a_job_bound_to_its_parameters() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "fix", 0, true);
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    // Repair takes the files that need it: the warning mp4 and the mkv, not the clean mp4.
    assert_eq!(run_schedule(ctx, 1).unwrap(), "Queued 2 files");
    let queued = jobs(&conn);
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].0, "fix");
    let snapshot: serde_json::Value = serde_json::from_str(&queued[0].1).unwrap();
    assert_eq!(snapshot["schedule_id"], 1);
    assert_eq!(snapshot["pipeline"], "prepare");
    assert_eq!(snapshot["policy"], "encode-v3");
    assert_eq!(snapshot["schedule_params"]["max_files"], 5);
    assert_eq!(snapshot["schedule_params"]["delete_original"], false);
    assert_eq!(snapshot["schedule_params"]["action"], "fix");
}

/// Mark file 3 (the mkv) as needing a re-encode; files 1 and 2 stay as they are.
fn mark_mkv_needing_reencode(conn: &Conn) {
    conn.raw()
        .execute("UPDATE media_files SET needs_reencode=1 WHERE id=3", [])
        .unwrap();
}

fn set_delete_original(conn: &Conn, value: bool) {
    conn.raw()
        .execute(
            "INSERT OR REPLACE INTO runtime_settings(key,value_json,value_type,revision) VALUES('delete_original',?1,'bool',1)",
            [value.to_string()],
        )
        .unwrap();
}

#[test]
fn a_schedule_without_the_re_encode_permission_leaves_out_files_that_need_one() {
    for delete_original in [false, true] {
        let db = Db::memory().unwrap();
        let conn = seed(&db, "fix", 0, true);
        mark_mkv_needing_reencode(&conn);
        set_delete_original(&conn, delete_original);
        let clock = FixedClock(NOW);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        // delete_original no longer decides: only the warning mp4 is taken.
        assert_eq!(
            run_schedule(ctx, 1).unwrap(),
            "Queued 1 file",
            "delete_original={delete_original}"
        );
        let snapshot: serde_json::Value = serde_json::from_str(&jobs(&conn)[0].1).unwrap();
        assert_eq!(snapshot["schedule_params"]["allow_reencode"], false);
    }
}

#[test]
fn a_schedule_with_the_re_encode_permission_takes_those_files_too() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "fix", 0, true);
    mark_mkv_needing_reencode(&conn);
    conn.raw()
        .execute("UPDATE schedules SET allow_reencode=1", [])
        .unwrap();
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    assert_eq!(run_schedule(ctx, 1).unwrap(), "Queued 2 files");
    let snapshot: serde_json::Value = serde_json::from_str(&jobs(&conn)[0].1).unwrap();
    assert_eq!(snapshot["schedule_params"]["allow_reencode"], true);
    // Only the file that needs it is queued to re-encode; the other one is merely repaired.
    assert_eq!(
        item_decisions(&conn),
        [
            (2, serde_json::json!("copy")),
            (3, serde_json::json!("all"))
        ]
    );
}

#[test]
fn a_schedule_with_the_permission_does_not_re_encode_a_file_that_does_not_need_it() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "fix", 0, true);
    conn.raw()
        .execute("UPDATE schedules SET allow_reencode=1", [])
        .unwrap();
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    assert_eq!(run_schedule(ctx, 1).unwrap(), "Queued 2 files");
    assert_eq!(
        item_decisions(&conn),
        [
            (2, serde_json::json!("copy")),
            (3, serde_json::json!("copy"))
        ]
    );
}

/// The Kodierfreigabe recorded on each queued item, by media id.
fn item_decisions(conn: &Conn) -> Vec<(i64, serde_json::Value)> {
    let mut statement = conn
        .raw()
        .prepare(
            "SELECT media_file_id, settings_snapshot_json FROM job_items ORDER BY media_file_id",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            let snapshot: serde_json::Value =
                serde_json::from_str(&row.get::<_, String>(1)?).unwrap();
            Ok((row.get(0)?, snapshot["encode"].clone()))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn a_fix_schedule_takes_a_clean_mp4_with_an_open_embed() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "fix", 0, true);
    // one.mp4 is clean but has a German .srt beside it that is not embedded yet.
    conn.raw()
        .execute_batch(
            "INSERT INTO sidecars(media_file_id,relative_path,fingerprint) VALUES(1,'one.de.srt','x');
             INSERT INTO inspections(media_file_id,source_fingerprint,input_fingerprint,tool_name,tool_version,result)
             VALUES(1,'s','one','ffprobe','1','{\"streams\":[]}');",
        )
        .unwrap();
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    assert_eq!(run_schedule(ctx, 1).unwrap(), "Queued 3 files");
}

#[test]
fn a_fix_schedule_leaves_a_clean_mp4_whose_sidecar_is_already_embedded() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "fix", 0, true);
    conn.raw()
        .execute_batch(
            "INSERT INTO sidecars(media_file_id,relative_path,fingerprint) VALUES(1,'one.de.srt','x');
             INSERT INTO inspections(media_file_id,source_fingerprint,input_fingerprint,tool_name,tool_version,result)
             VALUES(1,'s','one','ffprobe','1','{\"streams\":[{\"codec_type\":\"subtitle\",\"codec_name\":\"mov_text\",\"tags\":{\"language\":\"deu\"}}]}');",
        )
        .unwrap();
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    assert_eq!(run_schedule(ctx, 1).unwrap(), "Queued 2 files");
}

#[test]
fn a_convert_schedule_honours_max_files() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "convert", 0, true);
    conn.raw()
        .execute("UPDATE schedules SET max_files=1", [])
        .unwrap();
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    assert_eq!(run_schedule(ctx, 1).unwrap(), "Queued 1 file");
    assert_eq!(jobs(&conn)[0].0, "convert");
}

#[test]
fn a_closed_gate_runs_no_schedule_that_changes_files() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "fix", 0, true);
    conn.raw()
        .execute("UPDATE deployment_gate SET mode='maintenance'", [])
        .unwrap();
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    assert!(run_schedule(ctx, 1).is_err());
    assert_eq!(run_due_schedules(ctx).unwrap(), 0);
    assert!(jobs(&conn).is_empty());
}

#[test]
fn a_slot_on_time_runs_but_a_missed_one_is_never_caught_up() {
    for action in ["fix", "convert"] {
        let db = Db::memory().unwrap();
        let conn = seed(&db, action, 0, true);
        let clock = FixedClock(NOW);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        assert_eq!(run_due_schedules(ctx).unwrap(), 1, "{action} on time");
        assert_eq!(jobs(&conn).len(), 1);

        // The slot was 30 minutes ago: a verify would still catch up, a change must not.
        let db = Db::memory().unwrap();
        let conn = seed(&db, action, 30, true);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        assert_eq!(run_due_schedules(ctx).unwrap(), 0, "{action} missed");
        assert!(jobs(&conn).is_empty());
        // It is noted once as missed, with the two-minute window as the reason.
        assert_eq!(
            conn.raw()
                .query_row(
                    "SELECT group_concat(kind || ':' || result) FROM schedule_runs",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "missed:Missed: not started within 2 minutes of its slot"
        );
    }
    // Checking keeps its six-hour catch-up.
    let db = Db::memory().unwrap();
    let conn = seed(&db, "verify", 30, false);
    let clock = FixedClock(NOW);
    assert_eq!(
        run_due_schedules(Ctx {
            conn: conn.raw(),
            clock: &clock
        })
        .unwrap(),
        1
    );
    let _ = clock.now();
}
