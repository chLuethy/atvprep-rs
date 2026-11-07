//! "Einbetten offen": welche Untertiteldateien neben einer Datei sich noch einbetten lassen.
//!
//! Die Eigenschaft wird beim Lesen aus den Sidecars und dem aktuellen ffprobe-Ergebnis berechnet
//! und nie gespeichert; der gespeicherte Prüfzustand bleibt unberührt.

use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

use crate::Result;

/// Letzter Pfadteil eines Sidecars; ein leerer Name bekommt einen Platzhalter.
#[must_use]
pub fn basename(path: &str) -> String {
    let path = path.replace('\\', "/");
    path.rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("(unnamed sidecar)")
        .to_owned()
}

/// Was ein Reparieren mit jedem Sidecar einer Datei täte, als `(sidecar_id, name, sprache, zustand)`.
///
/// Der Vergleich mit den Spuren der Datei gilt nur, wenn das letzte ffprobe-Ergebnis zum aktuellen
/// Fingerabdruck passt; sonst sind alle Sidecars `unchecked`.
///
/// # Errors
///
/// SQLite fails.
pub fn classify_media(
    conn: &Connection,
    media: i64,
    file_name: &str,
    fingerprint: &str,
) -> Result<Vec<(i64, String, String, String)>> {
    let inspection: Option<(String, String)> = conn
        .query_row(
            "SELECT input_fingerprint,result FROM inspections WHERE media_file_id=?1 AND tool_name='ffprobe' ORDER BY id DESC LIMIT 1",
            [media],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let report = inspection.and_then(|(input, report)| {
        (input == fingerprint)
            .then(|| serde_json::from_str::<Value>(&report).ok())
            .flatten()
    });
    let streams = report
        .as_ref()
        .and_then(|value| value.get("streams"))
        .and_then(Value::as_array)
        .map(Vec::as_slice);
    let mut statement =
        conn.prepare("SELECT id,relative_path FROM sidecars WHERE media_file_id=?1 ORDER BY id")?;
    let names = statement
        .query_map([media], |row| {
            Ok((row.get::<_, i64>(0)?, basename(&row.get::<_, String>(1)?)))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let classified = atvprep_core::sidecars::classify_sidecars(
        &basename(file_name),
        &names
            .iter()
            .map(|(_, name)| name.clone())
            .collect::<Vec<_>>(),
        streams,
    );
    Ok(names
        .into_iter()
        .zip(classified)
        .map(|((id, _), (name, lang, state))| (id, name, lang, state))
        .collect())
}

/// Die Ids aller vorhandenen Dateien mit mindestens einem einbettbaren Sidecar, aufsteigend.
///
/// # Errors
///
/// SQLite fails.
pub fn open_media_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut statement = conn.prepare(
        "SELECT m.id, m.relative_path, m.fingerprint FROM media_files m
         WHERE m.discovery_state='present'
           AND EXISTS (SELECT 1 FROM sidecars s WHERE s.media_file_id=m.id AND lower(s.relative_path) LIKE '%.srt')
         ORDER BY m.id",
    )?;
    let candidates = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut open = Vec::new();
    for (id, path, fingerprint) in candidates {
        let pending = classify_media(conn, id, &path, &fingerprint)?
            .iter()
            .any(|(_, _, _, state)| state == "pending");
        if pending {
            open.push(id);
        }
    }
    Ok(open)
}

/// Die offenen Ids als JSON-Liste, um sie als ein Parameter an `json_each` zu binden.
///
/// # Errors
///
/// SQLite fails.
pub fn open_media_ids_json(conn: &Connection) -> Result<String> {
    Ok(serde_json::to_string(&open_media_ids(conn)?).unwrap_or_else(|_| "[]".to_owned()))
}
