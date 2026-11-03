//! Shared historical fixture, applied atomically to the isolated test database.
use rusqlite::{Connection, params_from_iter, types::Value as SqlValue};
use serde_json::Value;

pub fn apply(conn: &Connection) {
    apply_named(conn, "seed");
}

pub fn apply_named(conn: &Connection, name: &str) {
    let source = match name {
        "seed" => include_str!("../../../../testdata/api/seed.json"),
        "jobs" => include_str!("../../../../testdata/api/jobs_seed.json"),
        _ => panic!("unknown historical seed"),
    };
    let seed: Value = serde_json::from_str(source).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    for table in seed["tables"].as_object().unwrap().keys() {
        assert!(
            [
                "library_roots",
                "deployment_gate",
                "settings_revision",
                "runtime_settings",
                "media_files",
                "scan_runs",
                "jobs",
                "job_items",
                "sidecars",
                "schedules",
                "inspections",
                "events"
            ]
            .contains(&table.as_str())
        );
    }
    // Foreign keys require fixture order independent of JSON object ordering.
    for table in [
        "library_roots",
        "deployment_gate",
        "settings_revision",
        "runtime_settings",
        "media_files",
        "scan_runs",
        "jobs",
        "job_items",
        "sidecars",
        "schedules",
        "inspections",
        "events",
    ] {
        for row in seed["tables"][table].as_array().unwrap() {
            let row = row.as_object().unwrap();
            let columns: Vec<_> = row.keys().collect();
            assert!(
                columns
                    .iter()
                    .all(|name| name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            );
            let values: Vec<SqlValue> = columns
                .iter()
                .map(|name| {
                    let value = &row[*name];
                    match value {
                        Value::Null => SqlValue::Null,
                        Value::Bool(value) => SqlValue::Integer(i64::from(*value)),
                        Value::Number(value) => SqlValue::Integer(value.as_i64().unwrap()),
                        Value::String(value) if name.ends_with("_at") => SqlValue::Text(
                            chrono::DateTime::parse_from_rfc3339(value)
                                .unwrap()
                                .naive_utc()
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string(),
                        ),
                        Value::String(value) => SqlValue::Text(value.clone()),
                        _ => SqlValue::Text(serde_json::to_string(value).unwrap()),
                    }
                })
                .collect();
            let names = columns
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(",");
            let placeholders = vec!["?"; columns.len()].join(",");
            let updates = columns
                .iter()
                .map(|name| format!("{name}=excluded.{name}"))
                .collect::<Vec<_>>()
                .join(",");
            tx.execute(&format!("INSERT INTO {table}({names}) VALUES({placeholders}) ON CONFLICT DO UPDATE SET {updates}"), params_from_iter(values)).unwrap();
        }
    }
    tx.commit().unwrap();
}
