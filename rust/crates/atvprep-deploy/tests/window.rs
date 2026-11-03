//! Recovery invariants of actual SQLite-backed offline release windows.
use atvprep_deploy::{
    Error,
    window::{WindowOptions, run},
};
use atvprep_manager::{
    clock::SystemClock,
    db::Db,
    deployment_gate::{DeploymentGate, Operator},
    repo::Ctx,
    schema,
};
use std::{cell::Cell, time::Duration};

#[test]
fn dry_run_success_and_failed_activation_preserve_recovery_state() {
    for fail in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.sqlite");
        let db = Db::open(&path).unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let operator = Operator::from_local_cli(&path).unwrap();
        let mut options = WindowOptions {
            execute: false,
            services_stopped: true,
            imports_stopped: true,
            timeout: Duration::from_secs(2),
            backup_directory: temp.path().to_owned(),
        };
        let report = run(
            &conn,
            &SystemClock,
            &operator,
            &options,
            |_| panic!("dry migration"),
            || panic!("dry activation"),
        )
        .unwrap();
        assert!(!report.executed);
        assert!(!temp.path().join("window.jsonl").exists());
        options.execute = true;
        let migrated = Cell::new(false);
        let result = run(
            &conn,
            &SystemClock,
            &operator,
            &options,
            |generation| {
                let gate = DeploymentGate(Ctx {
                    conn: conn.raw(),
                    clock: &SystemClock,
                });
                let status = gate.status().unwrap();
                assert_eq!(status.phase, "offline-migration");
                assert_eq!(status.generation, generation);
                migrated.set(true);
                Ok(())
            },
            || {
                if fail {
                    Err(Error::Invalid("activation fixture failed".to_owned()))
                } else {
                    Ok(())
                }
            },
        );
        assert!(migrated.get());
        assert_eq!(result.is_err(), fail);
        let gate = DeploymentGate(Ctx {
            conn: conn.raw(),
            clock: &SystemClock,
        })
        .status()
        .unwrap();
        assert_eq!(gate.mode, "maintenance");
        assert_eq!(
            gate.phase,
            if fail { "offline-migration" } else { "steady" }
        );
        let backup = rusqlite::Connection::open(temp.path().join("manager-backup.sqlite")).unwrap();
        assert_eq!(
            backup
                .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        let journal = std::fs::read_to_string(temp.path().join("window.jsonl")).unwrap();
        let entries: Vec<serde_json::Value> = journal
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            entries.last().unwrap()["step"],
            if fail { "activate" } else { "finish" }
        );
        assert_eq!(entries.last().unwrap()["ok"], !fail);
    }
}

#[test]
fn active_permit_times_out_without_backup_or_migration() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.sqlite");
    let db = Db::open(&path).unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    let operator = Operator::from_local_cli(&path).unwrap();
    let gate = DeploymentGate(Ctx {
        conn: conn.raw(),
        clock: &SystemClock,
    });
    let _permit = gate.acquire_permit("scan", "window-test").unwrap().unwrap();
    let options = WindowOptions {
        execute: true,
        services_stopped: true,
        imports_stopped: true,
        timeout: Duration::from_millis(20),
        backup_directory: temp.path().to_owned(),
    };
    assert!(matches!(
        run(
            &conn,
            &SystemClock,
            &operator,
            &options,
            |_| panic!("migration before drain"),
            || panic!("activation before drain")
        ),
        Err(Error::DrainTimeout)
    ));
    assert_eq!(gate.status().unwrap().mode, "maintenance");
    assert!(!temp.path().join("manager-backup.sqlite").exists());
}
