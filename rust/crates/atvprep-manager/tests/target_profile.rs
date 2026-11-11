//! The Zielprofil as runtime settings: its defaults, its limits and what a change of it resets.

use atvprep_core::encode::EncodeNeed;
use atvprep_manager::clock::FixedClock;
use atvprep_manager::db::{Conn, Db};
use atvprep_manager::inspections::{Inspections, NewInspection};
use atvprep_manager::media::Media;
use atvprep_manager::repo::Ctx;
use atvprep_manager::schema::migrate;
use atvprep_manager::settings::{Settings, TargetProfile, default_of, target_profile};
use serde_json::json;

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

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            conn: self.conn.raw(),
            clock: &self.clock,
        }
    }

    fn settings(&self) -> Settings<'_> {
        Settings(self.ctx())
    }

    /// Two files with a known Kodierbedarf and one that does not know it yet.
    fn with_needs(&self) {
        self.conn.raw().execute_batch(
            "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'r','/r');
             INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,
               discovery_state,compliance_state,workflow_state,desired_action,encode_need,planned_change)
             VALUES (1,1,'a.mp4','movie','.mp4',1,1,'a','present','warning','idle','none','video','reencode'),
                    (2,1,'b.mp4','movie','.mp4',1,1,'b','present','clean','idle','none','none','none'),
                    (3,1,'c.mp4','movie','.mp4',1,1,'c','present','clean','idle','none','unknown','unknown');",
        ).unwrap();
    }

    fn needs(&self) -> Vec<(String, String)> {
        let mut statement = self
            .conn
            .raw()
            .prepare("SELECT encode_need, planned_change FROM media_files ORDER BY id")
            .unwrap();
        statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    }
}

#[test]
fn the_target_profile_starts_at_the_cli_defaults() {
    let f = Fixture::new();
    assert_eq!(
        target_profile(f.conn.raw()).unwrap(),
        TargetProfile {
            crf: 18,
            prefer_hevc10: false,
            keep_av1: false,
            fix_dts: false
        }
    );
    assert_eq!(default_of("target_crf"), Some(json!(18)));
    for key in ["target_prefer_hevc10", "target_keep_av1", "target_fix_dts"] {
        assert_eq!(default_of(key), Some(json!(false)), "{key}");
    }
}

#[test]
fn the_target_profile_is_read_from_the_settings_and_invalid_values_are_refused() {
    let f = Fixture::new();
    let s = f.settings();
    s.set_many(
        &[
            ("target_crf", json!(22)),
            ("target_prefer_hevc10", json!(true)),
            ("target_keep_av1", json!(true)),
            ("target_fix_dts", json!(true)),
        ],
        "o",
        None,
    )
    .unwrap();
    assert_eq!(
        target_profile(f.conn.raw()).unwrap(),
        TargetProfile {
            crf: 22,
            prefer_hevc10: true,
            keep_av1: true,
            fix_dts: true
        }
    );
    for (key, value) in [
        ("target_crf", json!(-1)),
        ("target_crf", json!(52)),
        ("target_crf", json!("18")),
        ("target_crf", json!(18.5)),
        ("target_keep_av1", json!(1)),
        ("target_prefer_hevc10", json!("true")),
        ("target_fix_dts", json!(null)),
    ] {
        assert!(s.set(key, &value, "o", None).is_err(), "{key} = {value}");
    }
    assert_eq!(target_profile(f.conn.raw()).unwrap().crf, 22);
}

#[test]
fn changing_any_part_of_the_target_profile_makes_every_need_unknown() {
    for (key, value) in [
        ("target_crf", json!(20)),
        ("target_prefer_hevc10", json!(true)),
        ("target_keep_av1", json!(true)),
        ("target_fix_dts", json!(true)),
    ] {
        let f = Fixture::new();
        f.with_needs();
        f.settings().set(key, &value, "o", None).unwrap();
        let unknown = ("unknown".to_owned(), "unknown".to_owned());
        assert_eq!(f.needs(), vec![unknown; 3], "{key}");
    }
}

#[test]
fn a_write_that_keeps_the_target_profile_leaves_the_needs_alone() {
    let f = Fixture::new();
    f.with_needs();
    let before = f.needs();
    let s = f.settings();
    // The defaults, written explicitly for the first time, are no change of the profile.
    s.set("target_crf", &json!(18), "o", None).unwrap();
    s.set("target_keep_av1", &json!(false), "o", None).unwrap();
    s.set("max_probe_workers", &json!(3), "o", None).unwrap();
    assert_eq!(f.needs(), before);
}

#[test]
fn a_later_probe_does_not_bring_back_the_need_of_an_older_profile() {
    let f = Fixture::new();
    f.with_needs();
    let record = |tool, result: &serde_json::Value| {
        Inspections(f.ctx())
            .record(
                1,
                &NewInspection {
                    source_fingerprint: "a",
                    input_fingerprint: "a",
                    tool_name: tool,
                    tool_version: "t",
                    result,
                },
                None,
            )
            .unwrap();
    };
    let stored = || Media(f.ctx()).get(1).unwrap().unwrap().encode_need;
    record(
        "verify",
        &json!({"violations": [], "encode_need": "video", "planned_change": "reencode"}),
    );
    assert_eq!(stored(), Some(EncodeNeed::Video));

    f.settings()
        .set("target_keep_av1", &json!(true), "o", None)
        .unwrap();
    assert_eq!(stored(), None);

    record("ffprobe", &json!({"streams": []}));
    assert_eq!(stored(), None, "an ffprobe run says nothing about the need");
}
