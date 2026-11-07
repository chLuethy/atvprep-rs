//! Authenticated read views for manager state.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::{extract::AuthenticatedSession, opaque::Opaque, state::AppState};

/// Aggregate the file, scan and job counts used by the dashboard.
pub async fn dashboard(State(state): State<AppState>, _session: AuthenticatedSession) -> Response {
    const PATH: &str = "/api/v1/dashboard/summary";
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        let counts = |sql: &str| -> Result<BTreeMap<String, i64>, ()> {
            let mut stmt = conn.raw().prepare(sql).map_err(|_| ())?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|_| ())?;
            rows.collect::<Result<BTreeMap<_,_>,_>>().map_err(|_| ())
        };
        let files = counts("SELECT compliance_state,count(*) FROM media_files WHERE NOT (discovery_state='missing' AND dismissed_at IS NOT NULL) GROUP BY compliance_state ORDER BY compliance_state")?;
        let scans = counts("SELECT status,count(*) FROM scan_runs GROUP BY status ORDER BY status")?;
        let jobs = counts("SELECT status,count(*) FROM jobs GROUP BY status ORDER BY status")?;
        let (missing, convert, reencode, converted): (i64,i64,i64,i64) = conn.raw().query_row(
            "SELECT COALESCE(SUM(CASE WHEN m.discovery_state='missing' AND m.dismissed_at IS NULL THEN 1 ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN m.discovery_state='present' AND lower(m.suffix) NOT IN ('.mp4','.m4v','.mov') AND NOT EXISTS (SELECT 1 FROM media_files s WHERE s.root_id=m.root_id AND s.discovery_state='present' AND s.relative_path=substr(m.relative_path,1,length(m.relative_path)-length(m.suffix))||'.mp4') THEN 1 ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN m.discovery_state='present' AND m.needs_reencode=1 THEN 1 ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN m.discovery_state='present' AND lower(m.suffix) NOT IN ('.mp4','.m4v','.mov') AND EXISTS (SELECT 1 FROM media_files s WHERE s.root_id=m.root_id AND s.discovery_state='present' AND s.relative_path=substr(m.relative_path,1,length(m.relative_path)-length(m.suffix))||'.mp4') THEN 1 ELSE 0 END),0) FROM media_files m", [],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).map_err(|_| ())?;
        let setting: Option<String> = conn.raw().query_row("SELECT value_json FROM runtime_settings WHERE key='delete_original'", [], |row| row.get(0)).optional().map_err(|_| ())?;
        let delete_original = setting.and_then(|value| serde_json::from_str::<bool>(&value).ok()).unwrap_or(false);
        Ok::<_, ()>(json!({"data":{"files":files,"missing":missing,"convert":convert,"reencode":reencode,"converted":converted,"delete_original":delete_original,"scans":scans,"jobs":jobs},"meta":{}}))
    }).await;
    match result {
        Ok(Ok(body)) => Json(body).into_response(),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

/// List configured roots without exposing filesystem paths.
pub async fn roots(State(state): State<AppState>, _session: AuthenticatedSession) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        let opaque = Opaque::new(state.api_secret.as_bytes());
        let mut query = conn.raw().prepare(
            "SELECT r.id, r.name, s.id, s.generation, s.status, s.started_at, s.finished_at
             FROM library_roots r LEFT JOIN scan_runs s ON s.id =
                 (SELECT max(id) FROM scan_runs WHERE root_id = r.id)
             ORDER BY r.id"
        ).map_err(|_| ())?;
        let rows = query.query_map([], |row| {
            let root_id: i64 = row.get(0)?;
            let scan_id: Option<i64> = row.get(2)?;
            let scan = if let Some(scan_id) = scan_id {
                Some(json!({"id": opaque.encode("s", scan_id), "root_id": opaque.encode("r", root_id),
                    "generation": row.get::<_, i64>(3)?, "status": row.get::<_, String>(4)?,
                    "started_at": row.get::<_, Option<String>>(5)?.map(|value| timestamp(&value)),
                    "finished_at": row.get::<_, Option<String>>(6)?.map(|value| timestamp(&value))}))
            } else { None };
            let name: String = row.get(1)?;
            let display_name = if name.starts_with('/') || name.starts_with('\\') || name.as_bytes().get(1) == Some(&b':') {
                crate::read_jobs::subject(&name)
            } else { name };
            Ok(json!({"id": opaque.encode("r", root_id), "name": display_name, "latest_scan": scan}))
        }).map_err(|_| ())?;
        let mut data = Vec::new();
        let mut used = 0_usize;
        let mut truncated = false;
        for row in rows {
            let value = row.map_err(|_| ())?;
            let size = serde_json::to_vec(&value).map_err(|_| ())?.len();
            if size > crate::bounds::MAX_ELEMENT_BYTES
                || used.saturating_add(size).saturating_add(data.len() + 128) > crate::bounds::MAX_RESPONSE_BYTES {
                truncated = true;
                break;
            }
            used += size;
            data.push(value);
        }
        Ok::<_, ()>((data, truncated))
    }).await;
    match result {
        Ok(Ok((data, truncated))) => Json(json!({"data": data, "meta": if truncated { json!({"truncated": true}) } else { json!({}) }})).into_response(),
        _ => crate::errors::ProblemDetail::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal server error",
            "/api/v1/roots",
        )
        .into_response(),
    }
}

pub(crate) fn timestamp(value: &str) -> String {
    if value.is_empty()
        || value.len() > 64
        || !value.bytes().all(|byte| {
            byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b':' | b'.' | b' ' | b'T' | b'Z')
        })
    {
        return "[invalid timestamp]".to_owned();
    }
    let value = value.replace(' ', "T");
    if value.ends_with('Z') || value.contains('+') {
        value
    } else {
        format!("{value}Z")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn malformed_timestamp_does_not_expose_private_data() {
        assert_eq!(super::timestamp("/private/path"), "[invalid timestamp]");
        assert_eq!(
            super::timestamp("2026-11-01 12:00:00"),
            "2026-11-01T12:00:00Z"
        );
    }
}

#[derive(Deserialize)]
pub struct ScanQuery {
    #[serde(default, deserialize_with = "crate::opaque::bounded_cursor")]
    cursor: Option<String>,
    limit: Option<usize>,
    root_id: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    #[serde(default, deserialize_with = "crate::opaque::bounded_cursor")]
    cursor: Option<String>,
    limit: Option<usize>,
}

/// List recent job events, ordered by timestamp and ID.
pub async fn activity(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<ActivityQuery>,
) -> Response {
    const PATH: &str = "/api/v1/dashboard/activity";
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let cursor = match query.cursor.as_deref() {
        Some(value) => match opaque.decode_timestamp(value, "cursor:activity") {
            Ok((time, id)) if id > 0 => Some((time, id)),
            _ => return error(StatusCode::BAD_REQUEST, "invalid cursor", PATH),
        },
        None => None,
    };
    let result=tokio::task::spawn_blocking(move ||{
        let conn=state.db.conn().map_err(|_|())?;
        let mut stmt=conn.raw().prepare("SELECT id,topic,created_at FROM events WHERE job_id IS NOT NULL AND (?1 IS NULL OR created_at<?1 OR (created_at=?1 AND id<?2)) ORDER BY created_at DESC,id DESC LIMIT ?3").map_err(|_|())?;
        let rows=stmt.query_map(params![cursor.as_ref().map(|c|&c.0),cursor.as_ref().map(|c|c.1),i64::try_from(limit+1).map_err(|_|())?],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?))).map_err(|_|())?;
        let mut data=Vec::new();let mut last=None;let mut used=0usize;let mut more=false;let mut truncated=false;
        for row in rows {
            let (id, topic, created) = row.map_err(|_| ())?;
            if data.len() == limit { more = true; break; }
            let Ok(cursor) = opaque.encode_timestamp("cursor:activity", &created, id) else {
                truncated = true; more = true; break;
            };
            let bounded_topic = crate::read_jobs::safe_head(&topic, 2048);
            if bounded_topic.len() < topic.len() { truncated = true; }
            let value = json!({"kind":"job_event","id":opaque.encode("e",id),"topic":bounded_topic,"created_at":timestamp(&created)});
            let size = serde_json::to_vec(&value).map_err(|_| ())?.len();
            if size > crate::bounds::MAX_ELEMENT_BYTES || used.saturating_add(size).saturating_add(data.len()+256) > crate::bounds::MAX_RESPONSE_BYTES { more = true; truncated = true; break; }
            used += size; last = Some(cursor); data.push(value);
        }
        let next=if more {last}else{None};
        Ok::<_,()>(json!({"data":data,"meta":page_meta(Some(limit),next.as_deref(),truncated)}))
    }).await;
    match result {
        Ok(Ok(body)) => Json(body).into_response(),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

pub(crate) fn error(status: StatusCode, message: &str, path: &str) -> Response {
    let code = match status {
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::INTERNAL_SERVER_ERROR => "internal_error",
        _ => "invalid_request",
    };
    crate::errors::ProblemDetail::new(status, code, message, path).into_response()
}

pub(crate) fn page_meta(limit: Option<usize>, next: Option<&str>, truncated: bool) -> Value {
    json!({"limit":limit,"next_cursor":next,"current_cursor":null,"current_sequence":null,"truncated":truncated,"facets":{}})
}

fn scan_view(row: &rusqlite::Row<'_>, opaque: &Opaque) -> rusqlite::Result<(i64, Value)> {
    let id: i64 = row.get(0)?;
    let root_id: i64 = row.get(1)?;
    Ok((
        id,
        json!({"id":opaque.encode("s", id), "root_id":opaque.encode("r", root_id),
        "generation":row.get::<_, i64>(2)?, "status":row.get::<_, String>(3)?,
        "started_at":row.get::<_, Option<String>>(4)?.map(|v| timestamp(&v)),
        "finished_at":row.get::<_, Option<String>>(5)?.map(|v| timestamp(&v))}),
    ))
}

/// List scans using a cursor bound to this endpoint.
pub async fn scans(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<ScanQuery>,
) -> Response {
    const PATH: &str = "/api/v1/scans";
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit)
        || query
            .status
            .as_ref()
            .is_some_and(|s| s.chars().count() > 128)
        || query
            .root_id
            .as_ref()
            .is_some_and(|s| s.chars().count() > 128)
    {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let after = match query.cursor.as_deref() {
        Some(token) => match opaque.decode(token, "cursor:scans") {
            Ok(id) if id > 0 => id,
            _ => return error(StatusCode::BAD_REQUEST, "invalid cursor", PATH),
        },
        None => 0,
    };
    let root = match query.root_id.as_deref().filter(|s| !s.is_empty()) {
        Some(token) => match opaque.decode(token, "r") {
            Ok(id) if id > 0 => Some(id),
            _ => return error(StatusCode::NOT_FOUND, "resource not found", PATH),
        },
        None => None,
    };
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        let mut stmt = conn.raw().prepare("SELECT id,root_id,generation,status,started_at,finished_at FROM scan_runs WHERE id>?1 AND (?2 IS NULL OR root_id=?2) AND (?3 IS NULL OR status=?3) ORDER BY id LIMIT ?4").map_err(|_| ())?;
        let status = query.status.filter(|s| !s.is_empty());
        let rows = stmt.query_map(params![after,root,status,i64::try_from(limit + 1).map_err(|_| ())?], |row| scan_view(row, &opaque)).map_err(|_| ())?;
        let mut data = Vec::new();
        let mut last_id = None;
        let mut used = 0_usize;
        let mut more = false;
        let mut truncated = false;
        for row in rows {
            let (id, value) = row.map_err(|_| ())?;
            if data.len() == limit { more = true; break; }
            let size = serde_json::to_vec(&value).map_err(|_| ())?.len();
            if size > crate::bounds::MAX_ELEMENT_BYTES || used.saturating_add(size).saturating_add(data.len() + 256) > crate::bounds::MAX_RESPONSE_BYTES {
                more = true;
                truncated = true;
                break;
            }
            used += size;
            data.push(value);
            last_id = Some(id);
        }
        let next = if more { last_id.map(|id| opaque.encode("cursor:scans", id)) } else { None };
        Ok::<_, ()>(json!({"data":data,"meta":page_meta(Some(limit),next.as_deref(),truncated)}))
    }).await;
    match result {
        Ok(Ok(body)) => Json(body).into_response(),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

/// Read an individual scan using its opaque identifier.
pub async fn scan(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Path(token): Path<String>,
) -> Response {
    let path = "/api/v1/scans";
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let id = match opaque.decode(&token, "s") {
        Ok(id) if id > 0 => id,
        _ => return error(StatusCode::NOT_FOUND, "resource not found", path),
    };
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        conn.raw().query_row("SELECT id,root_id,generation,status,started_at,finished_at FROM scan_runs WHERE id=?1", [id], |row| scan_view(row, &opaque)).optional().map_err(|_| ())
    }).await;
    match result {
        Ok(Ok(Some((_, data)))) => {
            let truncated = serde_json::to_vec(&data)
                .map_or(true, |value| value.len() > crate::bounds::MAX_ELEMENT_BYTES);
            let data = if truncated {
                json!({"truncated":true})
            } else {
                data
            };
            Json(json!({"data":data,"meta":page_meta(None,None,truncated)})).into_response()
        }
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "resource not found", path),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            path,
        ),
    }
}
