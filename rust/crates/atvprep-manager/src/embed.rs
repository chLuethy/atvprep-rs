//! "Embed open": which subtitle files beside a file can still be embedded.
//!
//! The trait is computed on read from the sidecars and the current ffprobe result
//! and never stored; the stored check state stays untouched.

use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

use crate::Result;

/// Last path component of a sidecar; an empty name gets a placeholder.
#[must_use]
pub fn basename(path: &str) -> String {
    let path = path.replace('\\', "/");
    path.rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("(unnamed sidecar)")
        .to_owned()
}

/// What a repair would do with each sidecar of a file, as `(sidecar_id, name, language, state)`.
///
/// The comparison with the file's tracks applies only when the latest ffprobe result matches the current
/// fingerprint; otherwise all sidecars are `unchecked`.
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

/// The ids of all existing files with at least one embeddable sidecar, ascending.
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

/// The open ids as a JSON list, to bind them as a single parameter to `json_each`.
///
/// # Errors
///
/// SQLite fails.
pub fn open_media_ids_json(conn: &Connection) -> Result<String> {
    Ok(serde_json::to_string(&open_media_ids(conn)?).unwrap_or_else(|_| "[]".to_owned()))
}
