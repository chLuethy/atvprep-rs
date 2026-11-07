//! "Einbetten offen": eine Datei mit einer einbettbaren Untertiteldatei daneben, berechnet beim Lesen.
use atvprep_manager::{
    db::{Conn, Db},
    embed, schema,
};

const SRT_DE: &str = "film.de.srt";

fn seed(db: &Db) -> Conn {
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw()
        .execute_batch(
            "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'test','/library');",
        )
        .unwrap();
    conn
}

/// Eine Datei samt Sidecars; `streams` ist das ffprobe-Ergebnis, `None` heißt: nie geprüft.
fn add_file(conn: &Conn, id: i64, state: &str, sidecars: &[&str], streams: Option<&str>) {
    let raw = conn.raw();
    raw.execute(
        "INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action)
         VALUES(?1,1,?2,'video','.mp4',1,1,'fp','present',?3,'idle','none')",
        rusqlite::params![id, format!("film{id}.mp4"), state],
    )
    .unwrap();
    for sidecar in sidecars {
        raw.execute(
            "INSERT INTO sidecars(media_file_id,relative_path,fingerprint) VALUES(?1,?2,'x')",
            rusqlite::params![id, sidecar.replace("film", &format!("film{id}"))],
        )
        .unwrap();
    }
    if let Some(streams) = streams {
        raw.execute(
            "INSERT INTO inspections(media_file_id,source_fingerprint,input_fingerprint,tool_name,tool_version,result)
             VALUES(?1,'source','fp','ffprobe','1',?2)",
            rusqlite::params![id, format!("{{\"streams\":{streams}}}")],
        )
        .unwrap();
    }
}

#[test]
fn a_file_with_an_embeddable_sidecar_is_open() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    add_file(&conn, 1, "clean", &[SRT_DE], Some("[]"));
    assert_eq!(embed::open_media_ids(conn.raw()).unwrap(), vec![1]);
}

#[test]
fn an_already_embedded_sidecar_is_not_open() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    let german = r#"[{"codec_type":"subtitle","codec_name":"mov_text","tags":{"language":"deu"}}]"#;
    add_file(&conn, 1, "clean", &[SRT_DE], Some(german));
    assert!(embed::open_media_ids(conn.raw()).unwrap().is_empty());
}

#[test]
fn files_without_sidecar_or_current_probe_or_presence_are_not_open() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    add_file(&conn, 1, "clean", &[], Some("[]"));
    add_file(&conn, 2, "clean", &[SRT_DE], None);
    add_file(&conn, 3, "clean", &["film.de.ass"], Some("[]"));
    add_file(&conn, 4, "clean", &[SRT_DE], Some("[]"));
    conn.raw()
        .execute(
            "UPDATE media_files SET discovery_state='missing' WHERE id=4",
            [],
        )
        .unwrap();
    assert!(embed::open_media_ids(conn.raw()).unwrap().is_empty());
}

#[test]
fn the_property_does_not_depend_on_the_stored_state() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    add_file(&conn, 1, "warning", &[SRT_DE], Some("[]"));
    add_file(&conn, 2, "clean", &[SRT_DE], Some("[]"));
    assert_eq!(embed::open_media_ids(conn.raw()).unwrap(), vec![1, 2]);
}
