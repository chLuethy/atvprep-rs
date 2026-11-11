//! Schedules that prepare files: queueing needs an approval, the Kodierfreigabe bounds which files
//! are taken, and missed runs are never caught up.
use atvprep_manager::{
    approval::PREPARE_POLICY,
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
    (3,1,'three.mkv','video','.mkv',1,1,'three','present','warning','idle','none');
    UPDATE media_files SET encode_need='none';").unwrap();
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
    let conn = seed(&db, "prepare", 0, false);
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
fn an_approved_schedule_queues_a_job_bound_to_its_parameters() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "prepare", 0, true);
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    // Preparing takes the files that need it: the warning mp4 and the mkv, not the clean mp4.
    assert_eq!(run_schedule(ctx, 1).unwrap(), "Queued 2 files");
    let queued = jobs(&conn);
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].0, "fix");
    let snapshot: serde_json::Value = serde_json::from_str(&queued[0].1).unwrap();
    assert_eq!(snapshot["schedule_id"], 1);
    assert_eq!(snapshot["pipeline"], "prepare");
    assert_eq!(snapshot["policy"], PREPARE_POLICY);
    assert_eq!(snapshot["profile"]["crf"], 18);
    assert_eq!(snapshot["schedule_params"]["max_files"], 5);
    assert_eq!(snapshot["schedule_params"]["delete_original"], false);
    assert_eq!(snapshot["schedule_params"]["action"], "prepare");
    assert_eq!(snapshot["schedule_params"]["encode"], "copy");
    assert_eq!(snapshot["schedule_params"]["profile"], snapshot["profile"]);
}

fn set_delete_original(conn: &Conn, value: bool) {
    conn.raw()
        .execute(
            "INSERT OR REPLACE INTO runtime_settings(key,value_json,value_type,revision) VALUES('delete_original',?1,'bool',1)",
            [value.to_string()],
        )
        .unwrap();
}

/// Four warning mp4s (ids 11 to 14) needing none, audio, video and an unknown encoding.
fn add_files_by_need(conn: &Conn) {
    conn.raw()
        .execute_batch(
            "UPDATE media_files SET compliance_state='clean', suffix='.mp4', relative_path='other' || id || '.mp4';
             INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action,encode_need) VALUES
             (11,1,'none.mp4','video','.mp4',1,1,'n','present','warning','idle','none','none'),
             (12,1,'audio.mp4','video','.mp4',1,1,'a','present','warning','idle','none','audio'),
             (13,1,'video.mp4','video','.mp4',1,1,'v','present','warning','idle','none','video'),
             (14,1,'unknown.mp4','video','.mp4',1,1,'u','present','warning','idle','none','unknown');",
        )
        .unwrap();
}

#[test]
fn a_schedule_takes_only_files_whose_need_lies_within_its_allowance() {
    for (encode, taken) in [
        ("copy", vec![11]),
        ("audio", vec![11, 12]),
        // Only the top level takes a file whose need is not known yet.
        ("all", vec![11, 12, 13, 14]),
    ] {
        for delete_original in [false, true] {
            let db = Db::memory().unwrap();
            let conn = seed(&db, "prepare", 0, true);
            add_files_by_need(&conn);
            set_delete_original(&conn, delete_original);
            conn.raw()
                .execute("UPDATE schedules SET encode=?1", [encode])
                .unwrap();
            let clock = FixedClock(NOW);
            let ctx = Ctx {
                conn: conn.raw(),
                clock: &clock,
            };
            run_schedule(ctx, 1).unwrap();
            let decisions = item_decisions(&conn);
            assert_eq!(
                decisions.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                taken,
                "{encode}, delete_original={delete_original}"
            );
            // Every item carries the schedule's own level, whatever the file needs.
            assert!(
                decisions.iter().all(|(_, level)| level == encode),
                "{encode}: {decisions:?}"
            );
        }
    }
}

#[test]
fn the_items_record_the_profile_the_schedule_runs_under() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "prepare", 0, true);
    conn.raw()
        .execute(
            "INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('target_crf','24','int',1)",
            [],
        )
        .unwrap();
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    run_schedule(ctx, 1).unwrap();
    let snapshot: serde_json::Value = serde_json::from_str(&jobs(&conn)[0].1).unwrap();
    assert_eq!(snapshot["schedule_params"]["profile"]["crf"], 24);
    let crf: Vec<serde_json::Value> = conn
        .raw()
        .prepare("SELECT settings_snapshot_json FROM job_items ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|text| {
            serde_json::from_str::<serde_json::Value>(&text.unwrap()).unwrap()["profile"]["crf"]
                .clone()
        })
        .collect();
    assert_eq!(crf, [serde_json::json!(24), serde_json::json!(24)]);
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
fn a_schedule_takes_a_clean_mp4_with_an_open_embed() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "prepare", 0, true);
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
fn a_schedule_leaves_a_clean_mp4_whose_sidecar_is_already_embedded() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "prepare", 0, true);
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
fn a_schedule_honours_max_files() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "prepare", 0, true);
    conn.raw()
        .execute("UPDATE schedules SET max_files=1", [])
        .unwrap();
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    assert_eq!(run_schedule(ctx, 1).unwrap(), "Queued 1 file");
    assert_eq!(jobs(&conn)[0].0, "fix");
}

#[test]
fn a_closed_gate_runs_no_schedule_that_changes_files() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "prepare", 0, true);
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
    {
        let action = "prepare";
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
