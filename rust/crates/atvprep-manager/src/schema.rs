//! The base schema and its runner. `PRAGMA user_version` is the schema version.

use crate::db::Conn;
use crate::{Error, Result};

/// Migration stages in order; stage `n` takes a database from version `n - 1` to `n`.
/// To extend: add `000N_name.sql`, append it here and raise [`VERSION`].
const STAGES: [&str; 10] = [
    include_str!("migrations/0001_base.sql"),
    include_str!("migrations/0002_cron.sql"),
    include_str!("migrations/0003_scan_action.sql"),
    include_str!("migrations/0004_schedule_runs.sql"),
    include_str!("migrations/0005_schedule_approval.sql"),
    include_str!("migrations/0006_missed_slots.sql"),
    include_str!("migrations/0007_needs_reencode.sql"),
    include_str!("migrations/0008_scan_orphans.sql"),
    include_str!("migrations/0009_schedule_allow_reencode.sql"),
    include_str!("migrations/0010_encode_need.sql"),
];

/// The version this build creates and understands.
pub const VERSION: i64 = 10;

/// Every table of the current schema.
pub const TABLES: [&str; 23] = [
    "auth_sessions",
    "csrf_tokens",
    "deployment_events",
    "deployment_gate",
    "deployment_permits",
    "events",
    "idempotency_records",
    "input_snapshots",
    "inspections",
    "job_items",
    "jobs",
    "library_roots",
    "maintenance_leases",
    "media_executor_leases",
    "media_files",
    "runtime_settings",
    "scan_runs",
    "schedule_runs",
    "scheduler_leases",
    "schedules",
    "settings_audit",
    "settings_revision",
    "sidecars",
];

/// The tables a database at schema `version` holds: [`TABLES`] minus those added by later stages.
#[must_use]
pub fn tables_at(version: i64) -> Vec<&'static str> {
    TABLES
        .iter()
        .copied()
        .filter(|table| version >= 4 || *table != "schedule_runs")
        .collect()
}

fn version(conn: &Conn) -> Result<i64> {
    Ok(conn
        .raw()
        .query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

/// Create the schema on an empty database, upgrade an older one; do nothing on a current one.
///
/// # Errors
///
/// The database was written by a newer build, or the SQL fails.
pub fn migrate(conn: &mut Conn) -> Result<()> {
    let found = version(conn)?;
    if found > VERSION {
        return Err(newer(found));
    }
    if found == VERSION {
        return Ok(());
    }
    conn.transaction(|tx| {
        // Another process may have migrated between the check and this write lock.
        let current: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if current > VERSION {
            return Err(newer(current));
        }
        let done = usize::try_from(current).unwrap_or(0);
        for stage in &STAGES[done..] {
            tx.execute_batch(stage)?;
        }
        tx.execute_batch(&format!("PRAGMA user_version = {VERSION}"))?;
        Ok(())
    })
}

fn newer(found: i64) -> Error {
    Error::Invalid(format!(
        "database schema version {found} is newer than this build understands ({VERSION})"
    ))
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::*;
    use crate::db::Db;

    fn fresh() -> (Db, Conn) {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        migrate(&mut conn).unwrap();
        (db, conn)
    }

    fn exec(conn: &Conn, sql: &str) -> Result<usize> {
        Ok(conn.raw().execute(sql, [])?)
    }

    fn seed_media(conn: &Conn) {
        exec(
            conn,
            "INSERT INTO library_roots(name, canonical_path) VALUES ('r', '/r')",
        )
        .unwrap();
        exec(
            conn,
            "INSERT INTO media_files(root_id, relative_path, kind, suffix, size, mtime_ns, fingerprint,
               discovery_state, compliance_state, workflow_state, desired_action)
             VALUES (1, 'a.mp4', 'video', '.mp4', 1, 1, 'f', 'present', 'unknown', 'idle', 'none')",
        )
        .unwrap();
        exec(conn, "INSERT INTO jobs(action) VALUES ('fix')").unwrap();
    }

    #[test]
    fn migrate_creates_all_23_tables() {
        let (_db, conn) = fresh();
        let mut names: Vec<String> = conn
            .raw()
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        names.sort();
        assert_eq!(names, TABLES);
        assert_eq!(version(&conn).unwrap(), VERSION);
    }

    #[test]
    fn the_singletons_are_seeded() {
        let (_db, conn) = fresh();
        let gate: (String, String, i64) = conn
            .raw()
            .query_row(
                "SELECT mode, phase, generation FROM deployment_gate",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(gate, ("open".to_owned(), "steady".to_owned(), 0));
        for table in ["media_executor_leases", "settings_revision"] {
            let n: i64 = conn
                .raw()
                .query_row(
                    &format!("SELECT count(*) FROM {table} WHERE id=1"),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{table}");
        }
    }

    #[test]
    fn a_version_1_database_upgrades_and_keeps_its_schedules() {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        conn.raw().execute_batch(STAGES[0]).unwrap();
        conn.raw().execute_batch("PRAGMA user_version = 1").unwrap();
        exec(
            &conn,
            "INSERT INTO library_roots(name, canonical_path) VALUES ('r', '/r')",
        )
        .unwrap();
        exec(
            &conn,
            "INSERT INTO schedules(id, name, action, root_id, days_mask, time_of_day, max_files, enabled, armed_at, last_slot_at, last_run_at, last_job_id, last_result, created_at)
             VALUES (7, 'old', 'convert', 1, 5, '03:30', 40, 0, 'a', 'b', 'c', 9, 'done', 'd')",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), VERSION);
        let row: String = conn
            .raw()
            .query_row(
                "SELECT concat_ws('|', name, action, root_id, days_mask, time_of_day, max_files, enabled, armed_at, last_slot_at, last_run_at, last_job_id, last_result, created_at) FROM schedules WHERE id=7",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row, "old|convert|1|5|03:30|40|0|a|b|c|9|done|d");
        exec(
            &conn,
            "INSERT INTO schedules(name, action, days_mask, time_of_day) VALUES ('s', 'scan', 1, '01:00')",
        )
        .unwrap();
        assert!(matches!(
            exec(
                &conn,
                "INSERT INTO schedules(name, action, days_mask, time_of_day) VALUES ('s', 'bogus', 1, '01:00')"
            ),
            Err(Error::Constraint(_))
        ));
    }

    #[test]
    fn a_fresh_database_accepts_scan_schedules() {
        let (_db, conn) = fresh();
        exec(
            &conn,
            "INSERT INTO schedules(name, action, days_mask, time_of_day) VALUES ('s', 'scan', 1, '01:00')",
        )
        .unwrap();
    }

    #[test]
    fn migrate_is_idempotent() {
        let (_db, mut conn) = fresh();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), VERSION);
    }

    #[test]
    fn a_version_1_database_upgrades_and_keeps_every_weekday_combination_on_its_slots() {
        use crate::cron::CronExpr;
        use crate::schedules::{LocalDateTime, TimeOfDay, latest_slot};

        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        conn.raw().execute_batch(STAGES[0]).unwrap();
        conn.raw().execute_batch("PRAGMA user_version = 1").unwrap();
        for mask in 1..=127 {
            let time = ["00:00", "03:05", "12:30", "23:59"][mask % 4];
            exec(
                &conn,
                &format!(
                    "INSERT INTO schedules(id, name, action, days_mask, time_of_day)
                     VALUES ({mask}, 's', 'verify', {mask}, '{time}')"
                ),
            )
            .unwrap();
        }
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), VERSION);
        let rows: Vec<(u8, String, String)> = conn
            .raw()
            .prepare("SELECT days_mask, time_of_day, cron FROM schedules ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(rows.len(), 127);
        assert_eq!(rows[0], (1, "03:05".to_owned(), "5 3 * * 1".to_owned()));
        // Sunday only is cron weekday 0.
        assert_eq!(rows[63], (64, "00:00".to_owned(), "0 0 * * 0".to_owned()));
        // Same slots as before, over two weeks of every hour.
        let start = LocalDateTime::new(2026, 10, 19, 0, 0).unwrap();
        for (mask, time, cron) in rows {
            let cron = CronExpr::parse(&cron).unwrap();
            let time = TimeOfDay::parse(&time).unwrap();
            for step in 0..14 * 24 {
                let now = start
                    .add_days(step / 24)
                    .with_time(TimeOfDay::parse(&format!("{:02}:30", step % 24)).unwrap());
                assert_eq!(
                    cron.latest_slot(now),
                    latest_slot(now, mask, time),
                    "mask {mask} at {now:?}"
                );
            }
        }
    }

    fn database_at(version: usize) -> (Db, Conn) {
        let db = Db::memory().unwrap();
        let conn = db.conn().unwrap();
        for stage in &STAGES[..version] {
            conn.raw().execute_batch(stage).unwrap();
        }
        conn.raw()
            .execute_batch(&format!("PRAGMA user_version = {version}"))
            .unwrap();
        (db, conn)
    }

    #[test]
    fn a_version_2_database_upgrades_to_current_and_keeps_its_schedules() {
        let (_db, mut conn) = database_at(2);
        exec(
            &conn,
            "INSERT INTO schedules(id, name, action, days_mask, time_of_day, max_files, enabled, cron)
             VALUES (4, 'nightly', 'verify', 31, '02:15', 12, 1, '15 2 * * 1-5')",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), VERSION);
        let row: String = conn
            .raw()
            .query_row(
                "SELECT concat_ws('|', name, action, days_mask, time_of_day, max_files, enabled, cron) FROM schedules WHERE id=4",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row, "nightly|verify|31|02:15|12|1|15 2 * * 1-5");
        exec(
            &conn,
            "INSERT INTO schedules(name, action, days_mask, time_of_day) VALUES ('s', 'scan', 1, '01:00')",
        )
        .unwrap();
        conn.raw().prepare("SELECT 1 FROM schedule_runs").unwrap();
        conn.raw()
            .prepare("SELECT approval_mac FROM schedules")
            .unwrap();
    }

    #[test]
    fn a_version_3_database_upgrades_to_current_and_keeps_its_scan_schedules() {
        let (_db, mut conn) = database_at(3);
        exec(
            &conn,
            "INSERT INTO schedules(id, name, action, days_mask, time_of_day, cron, enabled)
             VALUES (6, 'scan all', 'scan', 127, '04:00', '0 4 * * *', 1)",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), VERSION);
        let row: String = conn
            .raw()
            .query_row(
                "SELECT concat_ws('|', name, action, cron, enabled, approval_mac) FROM schedules WHERE id=6",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row, "scan all|scan|0 4 * * *|1");
        exec(
            &conn,
            "INSERT INTO schedule_runs(schedule_id, started_at, trigger, result) VALUES (6, 'now', 'manual', 'done')",
        )
        .unwrap();
    }

    #[test]
    fn a_version_4_database_upgrades_to_5_and_keeps_its_schedule_runs() {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        for stage in &STAGES[..4] {
            conn.raw().execute_batch(stage).unwrap();
        }
        conn.raw().execute_batch("PRAGMA user_version = 4").unwrap();
        exec(
            &conn,
            "INSERT INTO schedules(id, name, action, days_mask, time_of_day) VALUES (3, 's', 'verify', 1, '01:00')",
        )
        .unwrap();
        exec(
            &conn,
            "INSERT INTO schedule_runs(id, schedule_id, slot, started_at, trigger, result, job_id) VALUES (11, 3, 'x', 'now', 'manual', 'done', 5)",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), VERSION);
        let run: String = conn
            .raw()
            .query_row(
                "SELECT concat_ws('|', schedule_id, slot, trigger, result, job_id) FROM schedule_runs WHERE id=11",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(run, "3|x|manual|done|5");
        let approval: (Option<i64>, Option<i64>, Option<String>) = conn
            .raw()
            .query_row(
                "SELECT approved_by, approved_at, approval_mac FROM schedules WHERE id=3",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(approval, (None, None, None));
    }

    #[test]
    fn a_version_5_database_upgrades_to_6_and_allows_one_missed_entry_per_slot() {
        let (_db, mut conn) = database_at(5);
        exec(
            &conn,
            "INSERT INTO schedules(id, name, action, days_mask, time_of_day) VALUES (3, 's', 'verify', 1, '01:00')",
        )
        .unwrap();
        exec(
            &conn,
            "INSERT INTO schedule_runs(id, schedule_id, slot, started_at, trigger, result) VALUES (11, 3, 'x', 'now', 'scheduled', 'Queued 1 file')",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), VERSION);
        let kind: String = conn
            .raw()
            .query_row("SELECT kind FROM schedule_runs WHERE id=11", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kind, "run", "existing entries are runs");
        let missed = |slot: &str| {
            exec(
                &conn,
                &format!(
                    "INSERT INTO schedule_runs(schedule_id, slot, started_at, trigger, result, kind) VALUES (3, '{slot}', 'now', 'scheduled', 'Missed', 'missed')"
                ),
            )
        };
        missed("x").unwrap();
        assert!(missed("x").is_err(), "a second missed entry for a slot");
        missed("y").unwrap();
        // A run of the same slot is not a missed entry.
        exec(
            &conn,
            "INSERT INTO schedule_runs(schedule_id, slot, started_at, trigger, result) VALUES (3, 'y', 'now', 'scheduled', 'Queued 1 file')",
        )
        .unwrap();
        assert!(
            exec(
                &conn,
                "INSERT INTO schedule_runs(schedule_id, slot, started_at, trigger, result, kind) VALUES (3, 'z', 'now', 'scheduled', 'x', 'bogus')"
            )
            .is_err()
        );
    }

    #[test]
    fn a_version_6_database_upgrades_to_7_and_derives_the_re_encode_need_from_old_reasons() {
        let (_db, mut conn) = database_at(6);
        exec(
            &conn,
            "INSERT INTO library_roots(name, canonical_path) VALUES ('r', '/r')",
        )
        .unwrap();
        let insert = |id: i64, state: &str, reasons: &str, discovery: &str| {
            conn.raw()
                .execute(
                    "INSERT INTO media_files(id, root_id, relative_path, kind, suffix, size, mtime_ns, fingerprint,
                       discovery_state, compliance_state, compliance_reasons, workflow_state, desired_action)
                     VALUES (?1, 1, ?2, 'video', '.mkv', 1, 1, 'f', ?4, ?3, ?5, 'idle', 'none')",
                    rusqlite::params![id, format!("f{id}.mkv"), state, discovery, reasons],
                )
                .unwrap();
        };
        insert(
            1,
            "warning",
            r#"["[warning] video-reencode: video vc1 needs a re-encode to HEVC"]"#,
            "present",
        );
        insert(
            2,
            "unknown",
            r#"["Current ffprobe inspection is available.","[warning] video-reencode: x"]"#,
            "present",
        );
        insert(
            3,
            "blocker",
            r#"["[blocker] no-fragments: fragmented MP4"]"#,
            "present",
        );
        insert(4, "clean", "[]", "present");
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), VERSION);
        let needs: Vec<String> = conn
            .raw()
            .prepare("SELECT encode_need FROM media_files ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(needs, ["video", "video", "unknown", "unknown"]);
        // A row written after the migration is unknown until it has been checked.
        exec(
            &conn,
            "INSERT INTO media_files(id, root_id, relative_path, kind, suffix, size, mtime_ns, fingerprint,
               discovery_state, compliance_state, workflow_state, desired_action)
             VALUES (9, 1, 'new.mp4', 'video', '.mp4', 1, 1, 'f', 'present', 'unknown', 'idle', 'none')",
        )
        .unwrap();
        let fresh: String = conn
            .raw()
            .query_row("SELECT encode_need FROM media_files WHERE id=9", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(fresh, "unknown");
        assert!(
            exec(
                &conn,
                "UPDATE media_files SET encode_need='maybe' WHERE id=9"
            )
            .is_err(),
            "only the known needs"
        );
        assert!(
            exec(&conn, "UPDATE media_files SET needs_reencode=1 WHERE id=9").is_err(),
            "the old flag is gone"
        );
    }

    #[test]
    fn a_version_9_database_keeps_what_was_marked_and_makes_the_rest_unknown() {
        let (_db, mut conn) = database_at(9);
        exec(
            &conn,
            "INSERT INTO library_roots(name, canonical_path) VALUES ('r', '/r')",
        )
        .unwrap();
        exec(
            &conn,
            "INSERT INTO media_files(id, root_id, relative_path, kind, suffix, size, mtime_ns, fingerprint,
               discovery_state, compliance_state, workflow_state, desired_action, needs_reencode)
             VALUES (1, 1, 'a.mp4', 'video', '.mp4', 1, 1, 'f', 'present', 'warning', 'idle', 'none', 1),
                    (2, 1, 'b.mp4', 'video', '.mp4', 1, 1, 'f', 'present', 'clean', 'idle', 'none', 0)",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let rows: Vec<(String, String)> = conn
            .raw()
            .prepare("SELECT encode_need, planned_change FROM media_files ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                ("video".to_owned(), "reencode".to_owned()),
                ("unknown".to_owned(), "unknown".to_owned())
            ]
        );
    }

    #[test]
    fn a_version_7_database_upgrades_to_8_and_adds_an_empty_orphan_report_to_its_scans() {
        let (_db, mut conn) = database_at(7);
        exec(
            &conn,
            "INSERT INTO library_roots(id, name, canonical_path) VALUES (1, 'r', '/r')",
        )
        .unwrap();
        exec(
            &conn,
            "INSERT INTO scan_runs(id, root_id, generation, status) VALUES (1, 1, 1, 'succeeded')",
        )
        .unwrap();
        assert!(
            exec(&conn, "SELECT orphans_json FROM scan_runs").is_err(),
            "the column is new in version 8"
        );
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), VERSION);
        let stored: Option<String> = conn
            .raw()
            .query_row("SELECT orphans_json FROM scan_runs WHERE id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(stored, None);
    }

    #[test]
    fn a_version_8_database_upgrades_to_9_and_pauses_switched_on_schedules_that_change_files() {
        let (_db, mut conn) = database_at(8);
        exec(
            &conn,
            "INSERT INTO schedules(id, name, action, days_mask, time_of_day, max_files, enabled, approved_by, approved_at, approval_mac) VALUES
               (1, 'repair', 'fix', 127, '03:00', 5, 1, 1, 1700000000, 'mac'),
               (2, 'convert', 'convert', 127, '03:00', 5, 1, 1, 1700000000, 'mac'),
               (3, 'repair paused', 'fix', 127, '03:00', 5, 0, NULL, NULL, NULL),
               (4, 'check', 'verify', 127, '03:00', 5, 1, NULL, NULL, NULL),
               (5, 'scan', 'scan', 127, '03:00', 5, 1, NULL, NULL, NULL)",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn).unwrap(), VERSION);
        let rows: Vec<(i64, bool, bool, Option<String>, bool)> = conn
            .raw()
            .prepare("SELECT id, enabled, allow_reencode, approval_mac, last_result IS NOT NULL FROM schedules ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (1, false, false, None, true),
                (2, false, false, None, true),
                (3, false, false, None, false),
                (4, true, false, None, false),
                (5, true, false, None, false),
            ]
        );
        let history: Vec<(i64, String, String)> = conn
            .raw()
            .prepare("SELECT schedule_id, trigger, result FROM schedule_runs ORDER BY schedule_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(
            history.len(),
            2,
            "only the two switched-on schedules get an entry"
        );
        assert!(
            history
                .iter()
                .all(|(_, trigger, result)| trigger == "scheduled"
                    && result.starts_with("Paused: ")
                    && result.contains("re-encoding"))
        );
        assert!(
            exec(&conn, "UPDATE schedules SET allow_reencode=2 WHERE id=1").is_err(),
            "only 0 or 1"
        );
    }

    #[test]
    fn a_fresh_database_runs_every_stage() {
        let (_db, conn) = fresh();
        conn.raw().prepare("SELECT cron FROM schedules").unwrap();
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        conn.raw()
            .execute_batch("PRAGMA user_version = 99")
            .unwrap();
        let error = migrate(&mut conn).unwrap_err();
        assert!(
            error.to_string().contains("newer than this build"),
            "{error}"
        );
    }

    #[test]
    fn check_constraints_reject_unknown_states() {
        let (_db, conn) = fresh();
        seed_media(&conn);
        let bad = [
            "INSERT INTO jobs(action, status) VALUES ('fix', 'bogus')",
            "INSERT INTO jobs(action) VALUES ('bogus')",
            "INSERT INTO jobs(action, trigger) VALUES ('fix', 'bogus')",
            "INSERT INTO job_items(job_id, media_file_id, action, position, status) VALUES (1, 1, 'fix', 0, 'bogus')",
            "INSERT INTO scan_runs(root_id, generation, status) VALUES (1, 1, 'bogus')",
            "INSERT INTO scan_runs(root_id, generation, trigger) VALUES (1, 1, 'bogus')",
            "INSERT INTO schedules(name, action, days_mask, time_of_day) VALUES ('s', 'verify', 0, '01:00')",
            "INSERT INTO schedules(name, action, days_mask, time_of_day, max_files) VALUES ('s', 'verify', 1, '01:00', 501)",
            "UPDATE deployment_gate SET mode = 'bogus'",
            "UPDATE deployment_gate SET generation = -1",
            "INSERT INTO deployment_gate(id, mode, phase, generation) VALUES (2, 'open', 'steady', 0)",
            "UPDATE media_files SET compliance_state = 'bogus'",
            "UPDATE media_files SET workflow_state = 'bogus'",
            "UPDATE media_files SET desired_action = 'bogus'",
            "UPDATE media_files SET discovery_state = 'bogus'",
        ];
        for sql in bad {
            assert!(
                matches!(exec(&conn, sql), Err(Error::Constraint(_))),
                "{sql}"
            );
        }
    }

    #[test]
    fn active_fix_or_convert_item_per_media_is_unique() {
        let (_db, conn) = fresh();
        seed_media(&conn);
        let add = |action: &str, status: &str| {
            exec(
                &conn,
                &format!(
                    "INSERT INTO job_items(job_id, media_file_id, action, position, status)
                 VALUES (1, 1, '{action}', 0, '{status}')"
                ),
            )
        };
        add("fix", "queued").unwrap();
        assert!(matches!(
            add("convert", "running"),
            Err(Error::Constraint(_))
        ));
        add("verify", "queued").unwrap();
        add("fix", "failed").unwrap();
        exec(
            &conn,
            "UPDATE job_items SET status='succeeded' WHERE status='queued' AND action='fix'",
        )
        .unwrap();
        add("convert", "queued").unwrap();
    }

    #[test]
    fn events_cannot_be_updated_or_deleted() {
        let (_db, conn) = fresh();
        exec(&conn, "INSERT INTO events(topic, entity_type, entity_id, payload_json) VALUES ('t','job',1,'{}')").unwrap();
        for sql in ["UPDATE events SET topic='x'", "DELETE FROM events"] {
            let error = exec(&conn, sql).unwrap_err();
            assert!(
                error.to_string().contains("events are append-only"),
                "{sql}: {error}"
            );
        }
    }

    #[test]
    fn only_an_authorized_delete_removes_an_event() {
        let (db, conn) = fresh();
        for _ in 0..2 {
            exec(&conn, "INSERT INTO events(topic, entity_type, entity_id, payload_json) VALUES ('t','job',1,'{}')").unwrap();
        }
        assert_eq!(conn.delete_event_for_retention(1).unwrap(), 1);
        let left: Vec<i64> = conn
            .raw()
            .prepare("SELECT id FROM events")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(left, [2]);
        // The permission ends with the call and belongs to this connection only.
        assert!(exec(&conn, "DELETE FROM events WHERE id = 2").is_err());
        let other = db.conn().unwrap();
        assert!(
            other
                .raw()
                .execute("DELETE FROM events WHERE id = 2", [])
                .is_err()
        );
        assert_eq!(conn.delete_event_for_retention(2).unwrap(), 1);
    }

    #[test]
    fn deleting_a_parent_row_that_children_use_is_restricted() {
        let (_db, conn) = fresh();
        seed_media(&conn);
        conn.raw()
            .execute("INSERT INTO job_items(job_id, media_file_id, action, position) VALUES (?1, 1, 'verify', 0)", params![1])
            .unwrap();
        assert!(matches!(
            exec(&conn, "DELETE FROM jobs WHERE id=1"),
            Err(Error::Constraint(_))
        ));
        assert!(matches!(
            exec(&conn, "DELETE FROM media_files WHERE id=1"),
            Err(Error::Constraint(_))
        ));
    }
}
