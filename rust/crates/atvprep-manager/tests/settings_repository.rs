//! Settings repository (ported from `tests/manager/db/test_settings_repository.py`) and the
//! `normalize_excluded_dirs` golden.

use atvprep_manager::clock::FixedClock;
use atvprep_manager::db::{Conn, Db};
use atvprep_manager::repo::Ctx;
use atvprep_manager::schema::migrate;
use atvprep_manager::settings::{Settings, normalize_excluded_dirs};
use atvprep_manager::{Error, Result};
use atvprep_testkit::load_golden;
use serde_json::{Value, json};

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
            clock: FixedClock(1_700_000_000_000_000),
        }
    }

    fn settings(&self) -> Settings<'_> {
        Settings(Ctx {
            conn: self.conn.raw(),
            clock: &self.clock,
        })
    }
}

#[test]
fn settings_are_allowlisted_typed_revisioned_and_audited() {
    let f = Fixture::new();
    let s = f.settings();
    assert!(s.get("max_ffmpeg_workers").unwrap().is_none());
    let first = s
        .set("max_ffmpeg_workers", &json!(3), "operator", None)
        .unwrap();
    assert_eq!(
        (first.value, first.revision, first.value_type.as_str()),
        (json!(3), 1, "int")
    );
    let audit = s.audit("max_ffmpeg_workers").unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(
        (
            audit[0].actor.as_str(),
            &audit[0].new_value,
            &audit[0].old_value
        ),
        ("operator", &json!(3), &None)
    );
    let second = s
        .set("max_ffmpeg_workers", &json!(4), "operator", Some(1))
        .unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(
        s.audit("max_ffmpeg_workers").unwrap()[1].old_value,
        Some(json!(3))
    );
    let stale = s
        .set("max_ffmpeg_workers", &json!(5), "operator", Some(1))
        .unwrap_err();
    assert_eq!(stale.to_string(), "settings revision conflict");
    assert_eq!(s.current_revision().unwrap(), 2);
}

#[test]
fn unknown_and_secret_like_keys_are_rejected_and_never_stored() {
    let f = Fixture::new();
    let s = f.settings();
    for key in [
        "not_allowed",
        "media_concurrency",
        "probe_concurrency",
        "scheduler_interval_seconds",
        "master_token",
    ] {
        let error = s.set(key, &json!(1), "operator", None).unwrap_err();
        assert!(error.to_string().contains("allowlisted"), "{key}: {error}");
    }
    assert!(s.get("master_token").unwrap().is_none());
    assert_eq!(s.current_revision().unwrap(), 0);
}

#[test]
fn values_must_have_the_right_type_and_range() {
    let f = Fixture::new();
    let s = f.settings();
    let message = |key: &str, value: Value| s.set(key, &value, "o", None).unwrap_err().to_string();
    assert_eq!(
        message("max_ffmpeg_workers", json!(0)),
        "setting must be between 1 and 32"
    );
    assert_eq!(
        message("max_ffmpeg_workers", json!(33)),
        "setting must be between 1 and 32"
    );
    assert_eq!(
        message("max_ffmpeg_workers", json!("3")),
        "setting must be int integer-compatible"
    );
    assert_eq!(
        message("max_ffmpeg_workers", json!(true)),
        "setting must be int integer-compatible"
    );
    assert_eq!(
        message("max_ffmpeg_workers", json!(2.0)),
        "setting must be int integer-compatible"
    );
    assert_eq!(
        message("delete_original", json!(1)),
        "setting must be bool integer-compatible"
    );
    assert_eq!(
        message("scan_interval_seconds", json!(4)),
        "setting must be between 5 and 86400"
    );
    assert_eq!(
        message("scan_exclude_dirs", json!("x")),
        "setting scan_exclude_dirs has an invalid value"
    );
    let list = s
        .set(
            "scan_exclude_dirs",
            &json!(["./Extras/", "extras", "Extras", " Samples "]),
            "o",
            None,
        )
        .unwrap();
    assert_eq!(
        (list.value, list.value_type.as_str()),
        (json!(["Extras", "extras", "Samples"]), "list")
    );
    assert_eq!(
        s.set("delete_original", &json!(true), "o", None)
            .unwrap()
            .value_type,
        "bool"
    );
}

#[test]
fn bulk_settings_share_one_global_revision_and_audit_revision() {
    let f = Fixture::new();
    let s = f.settings();
    let rows = s
        .set_many(
            &[
                ("scan_interval_seconds", json!(600)),
                ("delete_original", json!(true)),
            ],
            "operator",
            Some(0),
        )
        .unwrap();
    assert_eq!(rows.iter().map(|r| r.revision).collect::<Vec<_>>(), [1, 1]);
    let audits = [
        s.audit("scan_interval_seconds").unwrap(),
        s.audit("delete_original").unwrap(),
    ];
    assert!(audits.iter().all(|a| a.len() == 1 && a[0].revision == 1));
    assert_eq!(s.current_revision().unwrap(), 1);
    let values = s.values().unwrap();
    assert_eq!(values["scan_interval_seconds"], json!(600));
}

#[test]
fn bulk_updates_are_validated_before_anything_is_written() {
    let f = Fixture::new();
    let s = f.settings();
    let empty = s.set_many(&[], "o", None).unwrap_err();
    assert_eq!(empty.to_string(), "settings update must not be empty");
    let bad = s
        .set_many(
            &[
                ("delete_original", json!(true)),
                ("max_probe_workers", json!(99)),
            ],
            "o",
            None,
        )
        .unwrap_err();
    assert_eq!(
        bad.to_string(),
        "setting max_probe_workers has an invalid value"
    );
    let unknown = s.set_many(&[("nope", json!(1))], "o", None).unwrap_err();
    assert_eq!(unknown.to_string(), "setting is not allowlisted");
    assert_eq!(
        (s.current_revision().unwrap(), s.values().unwrap().len()),
        (0, 0)
    );
}

#[test]
fn a_failed_write_inside_a_transaction_leaves_the_revision_alone() {
    let mut f = Fixture::new();
    let clock = FixedClock(0);
    let outcome: Result<()> = f.conn.transaction(|tx| {
        let s = Settings(Ctx {
            conn: tx,
            clock: &clock,
        });
        s.set("max_ffmpeg_workers", &json!(2), "o", Some(0))?;
        Err(Error::Invalid("abort".to_owned()))
    });
    assert!(outcome.is_err());
    assert_eq!(f.settings().current_revision().unwrap(), 0);
}

#[test]
fn concurrent_writers_with_the_same_expected_revision_have_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("settings.db")).unwrap();
    migrate(&mut db.conn().unwrap()).unwrap();
    for (seed, expected) in [(false, 0), (true, 1)] {
        if seed {
            let mut c = db.conn().unwrap();
            c.transaction(|tx| {
                Settings(Ctx {
                    conn: tx,
                    clock: &FixedClock(0),
                })
                .set("max_ffmpeg_workers", &json!(2), "o", None)?;
                Ok(())
            })
            .unwrap();
        }
        let results: Vec<std::result::Result<(), String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = [3, 4]
                .into_iter()
                .map(|value| {
                    let db = db.clone();
                    scope.spawn(move || {
                        let mut conn = db.conn().unwrap();
                        conn.transaction(|tx| {
                            Settings(Ctx {
                                conn: tx,
                                clock: &FixedClock(0),
                            })
                            .set(
                                "max_ffmpeg_workers",
                                &json!(value),
                                "o",
                                Some(expected),
                            )?;
                            Ok(())
                        })
                        .map_err(|e| e.to_string())
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            1,
            "{results:?}"
        );
        assert!(
            results
                .iter()
                .any(|r| r.as_ref().is_err_and(|e| e.contains("revision conflict"))),
            "{results:?}"
        );
        if !seed {
            // Reset so the second round starts from revision 1 with the key present.
            db.conn().unwrap().raw().execute_batch(
                "DELETE FROM settings_audit; DELETE FROM runtime_settings; UPDATE settings_revision SET revision=0",
            ).unwrap();
        }
    }
}

#[test]
fn normalize_excluded_dirs_matches_the_python_oracle() {
    let cases = load_golden::<Value, Value>("settings_normalize");
    assert_eq!(cases.len(), 38);
    for case in cases {
        let got = match normalize_excluded_dirs(&case.input) {
            Ok(list) => json!({"ok": list}),
            Err(error) => json!({"error": error.to_string()}),
        };
        assert_eq!(got, case.expected, "{}", case.name);
    }
}

#[test]
fn changing_the_target_profile_ends_the_approval_of_change_schedules_and_pauses_them() {
    let f = Fixture::new();
    f.conn.raw().execute_batch("INSERT INTO schedules(id,name,action,days_mask,time_of_day,max_files,enabled,approved_by,approved_at,approval_mac) VALUES
        (1,'prepare on','prepare',127,'03:00',5,1,1,1700000000,'mac'),
        (2,'verify','verify',127,'03:00',5,1,NULL,NULL,NULL);").unwrap();
    let state = || -> Vec<(i64, bool, bool)> {
        let mut statement = f
            .conn
            .raw()
            .prepare("SELECT id, enabled, approval_mac IS NOT NULL FROM schedules ORDER BY id")
            .unwrap();
        statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    };
    let s = f.settings();
    // Writing the default it already has changes nothing.
    s.set("target_crf", &json!(18), "o", None).unwrap();
    assert_eq!(state(), [(1, true, true), (2, true, false)]);
    for (key, value) in [
        ("target_crf", json!(22)),
        ("target_prefer_hevc10", json!(true)),
        ("target_keep_av1", json!(true)),
        ("target_fix_dts", json!(true)),
    ] {
        f.conn
            .raw()
            .execute(
                "UPDATE schedules SET enabled=1, approved_by=1, approved_at=1700000000, approval_mac='mac' WHERE id=1",
                [],
            )
            .unwrap();
        s.set(key, &value, "o", None).unwrap();
        assert_eq!(state(), [(1, false, false), (2, true, false)], "{key}");
    }
}

#[test]
fn changing_delete_original_ends_the_approval_of_change_schedules_and_pauses_them() {
    let f = Fixture::new();
    f.conn.raw().execute_batch("INSERT INTO schedules(id,name,action,days_mask,time_of_day,max_files,enabled,approved_by,approved_at,approval_mac) VALUES
        (1,'prepare on','prepare',127,'03:00',5,1,1,1700000000,'mac'),
        (2,'prepare all on','prepare',127,'03:00',5,1,1,1700000000,'mac'),
        (3,'prepare off','prepare',127,'03:00',5,0,NULL,NULL,NULL);
        INSERT INTO schedules(id,name,action,days_mask,time_of_day,enabled) VALUES (4,'verify','verify',127,'03:00',1),(5,'scan','scan',127,'03:00',1);").unwrap();
    let state = || -> Vec<(i64, bool, bool)> {
        let mut statement = f
            .conn
            .raw()
            .prepare("SELECT id, enabled, approval_mac IS NOT NULL FROM schedules ORDER BY id")
            .unwrap();
        statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    };
    let s = f.settings();
    // Writing the value it already has changes nothing.
    s.set("delete_original", &json!(false), "o", None).unwrap();
    assert_eq!(
        state(),
        [
            (1, true, true),
            (2, true, true),
            (3, false, false),
            (4, true, false),
            (5, true, false)
        ]
    );
    s.set("delete_original", &json!(true), "o", None).unwrap();
    assert_eq!(
        state(),
        [
            (1, false, false),
            (2, false, false),
            (3, false, false),
            (4, true, false),
            (5, true, false)
        ]
    );
}
