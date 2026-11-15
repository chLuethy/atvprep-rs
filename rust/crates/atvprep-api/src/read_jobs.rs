//! Read-only views of queued and completed jobs.
use crate::{
    extract::AuthenticatedSession,
    job_filter::{JobFilter, JobQuery},
    opaque::Opaque,
    read::{error, page_meta, timestamp},
    state::AppState,
};
use atvprep_manager::state_machine::{ItemStatus, JobStatus};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

fn job_select() -> String {
    job_select_with("NULL")
}

/// [`job_select`] with `extra` as a last column, the path of the item a search matched.
fn job_select_with(extra: &str) -> String {
    format!(
        "SELECT j.id,j.action,j.status,j.trigger,j.origin_job_id,j.requested_at,j.started_at,j.finished_at,j.total_count,j.succeeded_count,j.failed_count,j.cancelled_count,j.settings_snapshot_json,
 (SELECT m.relative_path FROM job_items i JOIN media_files m ON m.id=i.media_file_id WHERE i.job_id=j.id ORDER BY i.position,i.id LIMIT 1),
 (SELECT CAST(SUM(CASE WHEN i.status IN {terminal} THEN 100 ELSE COALESCE(i.progress,0) END)*1.0/COUNT(*) AS INTEGER) FROM job_items i WHERE i.job_id=j.id),
 {extra},
 {trigger},{schedule},(SELECT s.name FROM schedules s WHERE s.id={schedule})
 FROM jobs j",
        terminal = ItemStatus::terminal_sql_list(),
        trigger = crate::job_trigger::trigger_of(),
        schedule = crate::job_trigger::SCHEDULE_OF,
    )
}

pub(crate) fn subject(value: &str) -> String {
    let value = value.replace('\\', "/");
    let invalid = value.starts_with('/')
        || value.as_bytes().get(1) == Some(&b':')
        || value.split('/').any(|part| matches!(part, "" | "." | ".."));
    let value = if invalid {
        value
            .rsplit('/')
            .next()
            .filter(|v| !matches!(*v, "" | "." | ".."))
            .unwrap_or("(unnamed file)")
    } else {
        &value
    };
    let mut end = value.len().min(2048);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

pub(crate) fn safe_head(value: &str, limit: usize) -> String {
    let redacted = safe_diagnostic(value, usize::MAX);
    let mut end = redacted.len().min(limit);
    while !redacted.is_char_boundary(end) {
        end -= 1;
    }
    redacted[..end].to_owned()
}

pub(crate) fn safe_diagnostic(value: &str, limit: usize) -> String {
    crate::redact::redact_log_tail(value, limit)
}

/// The action as the API names it: every job that changes files is Vorbereiten (`prepare`), also the
/// history entries stored under the former names `fix` and `convert` (ADR 0008).
fn public_action(stored: &str) -> &str {
    if matches!(stored, "fix" | "convert") {
        "prepare"
    } else {
        stored
    }
}

fn view(row: &rusqlite::Row<'_>, opaque: &Opaque) -> rusqlite::Result<(i64, Value)> {
    let id: i64 = row.get(0)?;
    let action: String = row.get(1)?;
    let snapshot: String = row.get(12)?;
    let snapshot: Value = serde_json::from_str(&snapshot).unwrap_or(Value::Null);
    let public_action = public_action(&action);
    let origin: Option<i64> = row.get(4)?;
    let mut job = json!({"id":opaque.encode("j",id),"action":public_action,"status":row.get::<_,JobStatus>(2)?,"trigger":row.get::<_,String>(16)?,
        "origin_job_id":origin.map(|v|opaque.encode("j",v)),"requested_at":row.get::<_,Option<String>>(5)?.map(|v|timestamp(&v)),
        "started_at":row.get::<_,Option<String>>(6)?.map(|v|timestamp(&v)),"finished_at":row.get::<_,Option<String>>(7)?.map(|v|timestamp(&v)),
        "total_count":row.get::<_,i64>(8)?,"succeeded_count":row.get::<_,i64>(9)?,"failed_count":row.get::<_,i64>(10)?,"cancelled_count":row.get::<_,i64>(11)?,
        "delete_confirmed":snapshot.get("delete_original_confirmed").and_then(Value::as_bool).unwrap_or(false),
        "subject":row.get::<_,Option<String>>(13)?.map(|value| subject(&value)),"progress":row.get::<_,Option<i64>>(14)?});
    // Only a job that may remove originals carries the field; every other job reads as before.
    if snapshot.get("delete_original").and_then(Value::as_bool) == Some(true) {
        job["delete_original"] = json!(true);
    }
    // Only a job from a schedule names it; the name is gone once the schedule is deleted.
    if let Some(schedule) = row.get::<_, Option<i64>>(17)? {
        let name = row.get::<_, Option<String>>(18)?;
        job["schedule"] =
            json!({"id":opaque.encode("sc",schedule),"name":name.map(|name|safe_head(&name,2048))});
    }
    // A search hit in a later item names that item; a hit in the first file reads as before.
    let matched = row
        .get::<_, Option<String>>(15)?
        .map(|value| subject(&value));
    if let Some(matched) = matched.filter(|path| Some(path.as_str()) != job["subject"].as_str()) {
        job["matched_path"] = json!(matched);
    }
    Ok((id, job))
}

#[derive(Deserialize)]
pub struct ItemQuery {
    #[serde(default, deserialize_with = "crate::opaque::bounded_cursor")]
    cursor: Option<String>,
    limit: Option<usize>,
}

/// List the distinct jobs associated with a media file.
pub async fn file_jobs(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Path(token): Path<String>,
    Query(query): Query<ItemQuery>,
) -> Response {
    const PATH: &str = "/api/v1/files/jobs";
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let media_id = match opaque.decode(&token, "m") {
        Ok(id) if id > 0 => id,
        _ => return error(StatusCode::NOT_FOUND, "resource not found", PATH),
    };
    let after = match query.cursor.as_deref() {
        Some(value) => match opaque.decode(value, "cursor:file-jobs") {
            Ok(id) if id > 0 => id,
            _ => return error(StatusCode::BAD_REQUEST, "invalid cursor", PATH),
        },
        None => 0,
    };
    let result=tokio::task::spawn_blocking(move ||{
        let conn=state.db.conn().map_err(|_|())?;
        let exists:bool=conn.raw().query_row("SELECT EXISTS(SELECT 1 FROM media_files WHERE id=?1)",[media_id],|row|row.get(0)).map_err(|_|())?;
        if !exists{return Ok(None);}
        let sql=format!("{job_select} WHERE j.id>?1 AND EXISTS(SELECT 1 FROM job_items i WHERE i.job_id=j.id AND i.media_file_id=?2) ORDER BY j.id LIMIT ?3",job_select=job_select());
        let mut stmt=conn.raw().prepare(&sql).map_err(|_|())?;
        let rows=stmt.query_map(params![after,media_id,i64::try_from(limit+1).map_err(|_|())?],|row|view(row,&opaque)).map_err(|_|())?;
        let mut data=Vec::new();let mut last=None;let mut used=0usize;let mut more=false;let mut truncated=false;
        for row in rows {let (id,value)=row.map_err(|_|())?;if data.len()==limit {more=true;break;}let size=serde_json::to_vec(&value).map_err(|_|())?.len();if size>crate::bounds::MAX_ELEMENT_BYTES || used.saturating_add(size).saturating_add(data.len()+256)>crate::bounds::MAX_RESPONSE_BYTES {more=true;truncated=true;break;}used+=size;last=Some(id);data.push(value);}
        let next=if more {last.map(|id|opaque.encode("cursor:file-jobs",id))}else{None};
        Ok::<_,()>(Some(json!({"data":data,"meta":page_meta(Some(limit),next.as_deref(),truncated)})))
    }).await;
    match result {
        Ok(Ok(Some(body))) => Json(body).into_response(),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "resource not found", PATH),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

#[derive(Deserialize)]
pub struct QueueQuery {
    #[serde(default, deserialize_with = "crate::opaque::bounded_cursor")]
    cursor: Option<String>,
    limit: Option<usize>,
}

/// Show active jobs and the total queue size.
pub async fn queue(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<QueueQuery>,
) -> Response {
    const PATH: &str = "/api/v1/dashboard/queue";
    let limit = query.limit.unwrap_or(10);
    if !(1..=50).contains(&limit) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let after = match query.cursor.as_deref() {
        Some(value) => match opaque.decode(value, "cursor:queue") {
            Ok(id) if id > 0 => id,
            _ => return error(StatusCode::BAD_REQUEST, "invalid cursor", PATH),
        },
        None => 0,
    };
    let result=tokio::task::spawn_blocking(move ||{
        let conn=state.db.conn().map_err(|_|())?;
        let total:i64=conn.raw().query_row("SELECT count(*) FROM jobs WHERE status IN ('queued','running','cancel_requested')",[],|row|row.get(0)).map_err(|_|())?;
        let sql=format!("{job_select} WHERE j.status NOT IN {terminal} AND j.id>?1 ORDER BY j.id LIMIT ?2",job_select=job_select(),terminal=JobStatus::terminal_sql_list());
        let mut stmt=conn.raw().prepare(&sql).map_err(|_|())?;
        let rows=stmt.query_map(params![after,i64::try_from(limit+1).map_err(|_|())?],|row|view(row,&opaque)).map_err(|_|())?;
        let mut data=Vec::new();let mut last=None;let mut used=0usize;let mut more=false;let mut truncated=false;
        for row in rows {let (id,value)=row.map_err(|_|())?;if data.len()==limit {more=true;break;} let size=serde_json::to_vec(&value).map_err(|_|())?.len();if size>crate::bounds::MAX_ELEMENT_BYTES || used.saturating_add(size).saturating_add(data.len()+256)>crate::bounds::MAX_RESPONSE_BYTES {more=true;truncated=true;break;}used+=size;last=Some(id);data.push(value);}
        let next=if more {last.map(|id|opaque.encode("cursor:queue",id))}else{None};
        Ok::<_,()>(json!({"data":{"jobs":data,"total":total},"meta":page_meta(Some(limit),next.as_deref(),truncated)}))
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

fn item_view(row: &rusqlite::Row<'_>, opaque: &Opaque) -> rusqlite::Result<((i64, i64), Value)> {
    let id: i64 = row.get(0)?;
    let job_id: i64 = row.get(1)?;
    let media_id: i64 = row.get(2)?;
    let action: String = row.get(3)?;
    let position: i64 = row.get(4)?;
    let snapshot: String = row.get(7)?;
    let snapshot: Value = serde_json::from_str(&snapshot).unwrap_or(Value::Null);
    let action = public_action(&action);
    let output: Option<i64> = row.get(8)?;
    let detail: Option<String> = row.get(10)?;
    let detail = detail.map(|value| safe_diagnostic(&value, 1024));
    let mut row_json = json!({"id":opaque.encode("i",id),"job_id":opaque.encode("j",job_id),"media_id":opaque.encode("m",media_id),
        "action":action,"position":position,"status":row.get::<_,ItemStatus>(5)?,"attempt":row.get::<_,i64>(6)?,
        "delete_confirmed":snapshot.get("delete_original_confirmed").and_then(Value::as_bool).unwrap_or(false),
        "output_media_id":output.map(|id|opaque.encode("m",id)),"error_code":row.get::<_,Option<String>>(9)?.map(|v| safe_head(&v,128)),
        "error_detail":detail,"progress":row.get::<_,Option<i64>>(11)?,"started_at":row.get::<_,Option<String>>(12)?.map(|v|timestamp(&v)),
        "finished_at":row.get::<_,Option<String>>(13)?.map(|v|timestamp(&v)),"display_path":row.get::<_,Option<String>>(14)?.map(|v|subject(&v))});
    // Only an item under the prepare policy has a Kodierfreigabe; every other item keeps its shape.
    // Next to it the file's stored Kodierbedarf, so a higher allowance can be offered to fit it.
    if let Some(level) = atvprep_manager::approval::item_encode(&snapshot) {
        row_json["encode"] = json!(level.as_str());
        if let Some(need) = row.get::<_, Option<String>>(15)? {
            row_json["encode_need"] = json!(need);
        }
    }
    // The change a Vorbereiten item was queued with; an item without one (older entries, Prüfen,
    // Analysieren) keeps its shape.
    if let Some(change) = snapshot.get("planned_change").and_then(Value::as_str) {
        row_json["planned_change"] = json!(safe_head(change, 32));
        let steps: Vec<String> = snapshot
            .get("planned_steps")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .take(atvprep_manager::jobs::MAX_PLANNED_STEPS)
            .map(|step| safe_head(step, atvprep_manager::jobs::MAX_PLANNED_STEP_BYTES))
            .collect();
        row_json["planned_steps"] = json!(steps);
    }
    Ok(((position, id), row_json))
}

/// List a job's items in position order with an authenticated composite cursor.
pub async fn items(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Path(token): Path<String>,
    Query(query): Query<ItemQuery>,
) -> Response {
    const PATH: &str = "/api/v1/jobs/items";
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let job_id = match opaque.decode(&token, "j") {
        Ok(id) if id > 0 => id,
        _ => return error(StatusCode::NOT_FOUND, "resource not found", PATH),
    };
    let cursor = match query.cursor.as_deref() {
        Some(value) => match opaque.decode_pair(value, "cursor:job-items") {
            Ok((position, id)) if position >= 0 && id > 0 => Some((position, id)),
            _ => return error(StatusCode::BAD_REQUEST, "invalid cursor", PATH),
        },
        None => None,
    };
    let result=tokio::task::spawn_blocking(move || {
        let conn=state.db.conn().map_err(|_|())?;
        let exists:bool=conn.raw().query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=?1)",[job_id],|row|row.get(0)).map_err(|_|())?;
        if !exists { return Ok(None); }
        let mut stmt=conn.raw().prepare("SELECT i.id,i.job_id,i.media_file_id,i.action,i.position,i.status,i.attempt,i.settings_snapshot_json,i.output_media_file_id,i.error_code,i.error_detail,i.progress,i.started_at,i.finished_at,m.relative_path,m.encode_need FROM job_items i LEFT JOIN media_files m ON m.id=i.media_file_id WHERE i.job_id=?1 AND (?2 IS NULL OR i.position>?2 OR (i.position=?2 AND i.id>?3)) ORDER BY i.position,i.id LIMIT ?4").map_err(|_|())?;
        let rows=stmt.query_map(params![job_id,cursor.map(|c|c.0),cursor.map(|c|c.1),i64::try_from(limit+1).map_err(|_|())?],|row|item_view(row,&opaque)).map_err(|_|())?;
        let mut data=Vec::new();let mut last=None;let mut used=0usize;let mut more=false;let mut truncated=false;
        for row in rows {let (key,value)=row.map_err(|_|())?;if data.len()==limit {more=true;break;} let size=serde_json::to_vec(&value).map_err(|_|())?.len();if size>crate::bounds::MAX_ELEMENT_BYTES || used.saturating_add(size).saturating_add(data.len()+256)>crate::bounds::MAX_RESPONSE_BYTES {more=true;truncated=true;break;}used+=size;last=Some(key);data.push(value);}
        let next=if more {last.map(|(position,id)|opaque.encode_pair("cursor:job-items",position,id))}else{None};
        Ok::<_,()>(Some(json!({"data":data,"meta":page_meta(Some(limit),next.as_deref(),truncated)})))
    }).await;
    match result {
        Ok(Ok(Some(body))) => Json(body).into_response(),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "resource not found", PATH),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

/// The most of an item's log one response returns: a whole log as the manager stores it.
const ITEM_LOG_LIMIT: usize = atvprep_manager::worker::logs::ITEM_LOG_LIMIT;

/// Return a bounded and redacted log tail for one job item.
pub async fn item_log(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Path(token): Path<String>,
) -> Response {
    const PATH: &str = "/api/v1/job-items/log";
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let id = match opaque.decode(&token, "i") {
        Ok(id) if id > 0 => id,
        _ => return error(StatusCode::NOT_FOUND, "resource not found", PATH),
    };
    let result=tokio::task::spawn_blocking(move || {
        let conn=state.db.conn().map_err(|_|())?;
        let row:Option<(ItemStatus,Option<String>)>=conn.raw().query_row("SELECT status,log_tail FROM job_items WHERE id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|_|())?;
        Ok::<_,()>(row.map(|(status,tail)| {
            let redacted=tail.map(|value|safe_diagnostic(&value,usize::MAX));
            let truncated=redacted.as_ref().is_some_and(|value|value.len()>ITEM_LOG_LIMIT);
            let tail=redacted.map(|value| {
                let mut start=value.len().saturating_sub(ITEM_LOG_LIMIT);
                while !value.is_char_boundary(start) {start+=1;}
                value[start..].to_owned()
            });
            json!({"data":{"item_id":opaque.encode("i",id),"status":status,"log_tail":tail},"meta":page_meta(None,None,truncated)})
        }))
    }).await;
    match result {
        Ok(Ok(Some(body))) => Json(body).into_response(),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "resource not found", PATH),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

/// List jobs with stable, endpoint-bound cursors, filtered as `JobFilter` describes.
pub async fn jobs(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<JobQuery>,
) -> Response {
    const PATH: &str = "/api/v1/jobs";
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit)
        || query
            .order
            .as_deref()
            .is_some_and(|v| !matches!(v, "asc" | "desc"))
    {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let filter = match JobFilter::parse(&query, &opaque) {
        Ok(filter) => filter,
        Err((status, message)) => return error(status, message, PATH),
    };
    let after = match query.cursor.as_deref() {
        Some(v) => match opaque.decode(v, "cursor:jobs") {
            Ok(id) if id > 0 => Some(id),
            _ => return error(StatusCode::BAD_REQUEST, "invalid cursor", PATH),
        },
        None => None,
    };
    let descending = query.order.as_deref() == Some("desc");
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        let (filter_sql, filter_values) = filter.sql();
        let (matched, mut values) = match filter.matched() {
            Some((column, pattern)) => (column, vec![pattern]),
            None => ("NULL", Vec::new()),
        };
        values.extend(filter_values);
        let mut sql = format!(
            "{job_select} WHERE {filter_sql}",
            job_select = job_select_with(matched)
        );
        if let Some(after) = after {
            sql.push_str(if descending {
                " AND j.id<?"
            } else {
                " AND j.id>?"
            });
            values.push(after.into());
        }
        sql.push_str(if descending {
            " ORDER BY j.id DESC LIMIT ?"
        } else {
            " ORDER BY j.id ASC LIMIT ?"
        });
        values.push(i64::try_from(limit + 1).map_err(|_| ())?.into());
        let mut stmt = conn.raw().prepare(&sql).map_err(|_| ())?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(values.iter()), |row| {
                view(row, &opaque)
            })
            .map_err(|_| ())?;
        let mut data = Vec::new();
        let mut last = None;
        let mut used = 0usize;
        let mut more = false;
        let mut truncated = false;
        for row in rows {
            let (id, value) = row.map_err(|_| ())?;
            if data.len() == limit {
                more = true;
                break;
            }
            let size = serde_json::to_vec(&value).map_err(|_| ())?.len();
            if size > crate::bounds::MAX_ELEMENT_BYTES
                || used.saturating_add(size).saturating_add(data.len() + 256)
                    > crate::bounds::MAX_RESPONSE_BYTES
            {
                more = true;
                truncated = true;
                break;
            }
            used += size;
            last = Some(id);
            data.push(value);
        }
        let next = if more {
            last.map(|id| opaque.encode("cursor:jobs", id))
        } else {
            None
        };
        Ok::<_, ()>(json!({"data":data,"meta":page_meta(Some(limit),next.as_deref(),truncated)}))
    })
    .await;
    match result {
        Ok(Ok(body)) => Json(body).into_response(),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

/// The facet counters of the job list with the same filters as `/jobs`: per status and per action
/// with every other filter, the jobs that match all filters of all there are, and the waiting or
/// running jobs the filter leaves out.
pub async fn job_counts(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<JobQuery>,
) -> Response {
    const PATH: &str = "/api/v1/jobs/counts";
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let filter = match JobFilter::parse(&query, &opaque) {
        Ok(filter) => filter,
        Err((status, message)) => return error(status, message, PATH),
    };
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        filter.counts(conn.raw(), &opaque).map_err(|_| ())
    })
    .await;
    match result {
        Ok(Ok(data)) => Json(json!({"data": data, "meta": {}})).into_response(),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

/// Read a job by its opaque identifier.
pub async fn job(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Path(token): Path<String>,
) -> Response {
    const PATH: &str = "/api/v1/jobs";
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let id = match opaque.decode(&token, "j") {
        Ok(id) if id > 0 => id,
        _ => return error(StatusCode::NOT_FOUND, "resource not found", PATH),
    };
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        conn.raw()
            .query_row(&format!("{} WHERE j.id=?1", job_select()), [id], |row| {
                view(row, &opaque)
            })
            .optional()
            .map_err(|_| ())
    })
    .await;
    match result {
        Ok(Ok(Some((_, data)))) => {
            let truncated = serde_json::to_vec(&data)
                .map_or(true, |v| v.len() > crate::bounds::MAX_ELEMENT_BYTES);
            let data = if truncated {
                json!({"truncated":true})
            } else {
                data
            };
            Json(json!({"data":data,"meta":page_meta(None,None,truncated)})).into_response()
        }
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "resource not found", PATH),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{safe_diagnostic, safe_head, subject};

    #[test]
    fn subject_hides_absolute_and_traversal_paths() {
        assert_eq!(subject("Films/Movie.mkv"), "Films/Movie.mkv");
        assert_eq!(subject("/private/library/Movie.mkv"), "Movie.mkv");
        assert_eq!(subject("../secret/Movie.mkv"), "Movie.mkv");
        assert_eq!(subject("C:\\private\\Movie.mkv"), "Movie.mkv");
    }

    #[test]
    fn diagnostics_do_not_expose_credentials_or_windows_paths() {
        assert_eq!(safe_diagnostic("token=abc", 1024), "token=<redacted>");
        assert_eq!(
            safe_diagnostic("failed C:\\private\\film.mkv", 1024),
            "failed <path>"
        );
        assert_eq!(
            safe_diagnostic("failed C:/private/film.mkv", 1024),
            "failed <path>"
        );
        assert!(!safe_diagnostic("failed /private/film.mkv", 1024).contains("/private"));
    }

    #[test]
    fn text_limits_preserve_utf8_prefixes() {
        assert_eq!(safe_head("abécdef", 5), "abéc");
        assert_eq!(subject(&"é".repeat(2000)).len(), 2048);
    }
}

#[cfg(test)]
mod http_tests {
    use crate::router;
    use crate::routes_auth;
    use crate::test_support::state;
    use atvprep_fixtures::Fixtures;
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use tower::ServiceExt;

    #[tokio::test]
    async fn jobs_filter_page_and_show_prepare_snapshot() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let fx = Fixtures::new(conn.raw());
        let root = fx.library("Movies");
        let files: Vec<i64> = ["a.mkv", "b.mkv", "c.mkv"]
            .iter()
            .map(|path| fx.file(root, path).create())
            .collect();
        let _ = fx
            .job("fix", &files[..1])
            .settings(serde_json::json!({"pipeline": "prepare", "delete_original_confirmed": true, "delete_original": true}))
            .create();
        let _ = fx.job("verify", &files[1..2]).status("succeeded").create();
        let _ = fx.job("convert", &files[2..]).create();
        fx.sql("UPDATE jobs SET cleared_at='2026-01-01' WHERE id=3;");
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(get("/api/v1/jobs?limit=1"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"][0]["action"], "prepare");
        assert_eq!(body["data"][0]["delete_confirmed"], true);
        assert_eq!(body["data"][0]["delete_original"], true);
        let id = body["data"][0]["id"].as_str().unwrap();
        let detail = app
            .clone()
            .oneshot(get(&format!("/api/v1/jobs/{id}")))
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK);
        let cursor = body["meta"]["next_cursor"].as_str().unwrap();
        let response = app
            .clone()
            .oneshot(get(&format!("/api/v1/jobs?limit=1&cursor={cursor}")))
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"][0]["action"], "verify");
        assert!(body["meta"]["next_cursor"].is_null());
        assert_eq!(
            app.oneshot(get(&format!("/api/v1/jobs?cursor={id}")))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn dashboard_queue_reports_total_and_paginates_active_jobs() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let fx = Fixtures::new(conn.raw());
        let file = fx.file(fx.library("Movies"), "a.mkv").create();
        let _ = fx.job("verify", &[file]).create();
        let _ = fx.job("verify", &[file]).status("running").create();
        let _ = fx.job("verify", &[file]).status("succeeded").create();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(get("/api/v1/dashboard/queue?limit=1"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"]["total"], 2);
        assert_eq!(body["data"]["jobs"][0]["status"], "queued");
        let cursor = body["meta"]["next_cursor"].as_str().unwrap();
        let response = app
            .oneshot(get(&format!(
                "/api/v1/dashboard/queue-preview?limit=1&cursor={cursor}"
            )))
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"]["jobs"][0]["status"], "running");
        assert!(body["meta"]["next_cursor"].is_null());
    }

    #[tokio::test]
    async fn job_list_filters_accept_known_statuses_and_reject_unknown_ones() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let fx = Fixtures::new(conn.raw());
        let file = fx.file(fx.library("Movies"), "a.mkv").create();
        let _ = fx.job("verify", &[file]).create();
        let _ = fx.job("verify", &[file]).status("running").create();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(get("/api/v1/jobs?status=running"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"].as_array().unwrap().len(), 1);
        assert_eq!(body["data"][0]["status"], "running");
        for path in [
            "/api/v1/jobs?status=bogus",
            "/api/v1/jobs?exclude_status=bogus",
        ] {
            let response = app.clone().oneshot(get(path)).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{path}"
            );
            let body: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
            assert_eq!(body["error"]["code"], "invalid_request", "{path}");
        }
    }

    #[tokio::test]
    async fn job_items_page_across_equal_positions_without_exposing_paths() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let fx = Fixtures::new(conn.raw());
        let root = fx.library_at("Movies", "/private");
        let one = fx.file(root, "/private/one.mkv").fingerprint("a").create();
        let two = fx.file(root, "two.mkv").fingerprint("b").create();
        let _ = fx.job("verify", &[one, two]).create();
        // Both items share position 0, which a job never produces: the tie is what the page
        // boundary must survive.
        fx.sql("UPDATE job_items SET position=0; UPDATE job_items SET error_detail='failed /private/one.mkv' WHERE id=1;");
        let job = crate::opaque::Opaque::new(state.api_secret.as_bytes()).encode("j", 1);
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let first = app
            .clone()
            .oneshot(get(&format!("/api/v1/jobs/{job}/items?limit=1")))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first: serde_json::Value =
            serde_json::from_slice(&to_bytes(first.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(first["data"][0]["display_path"], "one.mkv");
        assert!(!first.to_string().contains("/private"));
        let cursor = first["meta"]["next_cursor"].as_str().unwrap();
        let second = app
            .oneshot(get(&format!(
                "/api/v1/jobs/{job}/items?limit=1&cursor={cursor}"
            )))
            .await
            .unwrap();
        let second: serde_json::Value =
            serde_json::from_slice(&to_bytes(second.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(second["data"][0]["display_path"], "two.mkv");
        assert!(second["meta"]["next_cursor"].is_null());
    }

    #[tokio::test]
    async fn job_item_logs_are_bounded_and_redact_credentials() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let fx = Fixtures::new(conn.raw());
        let file = fx
            .file(fx.library_at("Movies", "/private"), "film.mkv")
            .fingerprint("a")
            .create();
        let _ = fx.job("verify", &[file]).create();
        conn.raw()
            .execute(
                "UPDATE job_items SET log_tail=?1 WHERE id=1",
                [format!(
                    "[10:00:00] Start: verify\n{}",
                    format!("{}\n", "é".repeat(100)).repeat(300)
                )],
            )
            .unwrap();
        let id = crate::opaque::Opaque::new(state.api_secret.as_bytes()).encode("i", 1);
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = || {
            axum::http::Request::get(format!("/api/v1/job-items/{id}/log"))
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let read = |app: axum::Router| async move {
            let response = app.oneshot(get()).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            serde_json::from_slice::<serde_json::Value>(
                &to_bytes(response.into_body(), 1 << 20).await.unwrap(),
            )
            .unwrap()
        };
        // A whole manager log (up to 64 KiB) comes back whole, its first step line included.
        let body = read(app.clone()).await;
        assert_eq!(body["meta"]["truncated"], false);
        assert!(
            body["data"]["log_tail"]
                .as_str()
                .unwrap()
                .starts_with("[10:00:00] Start: verify\n")
        );
        conn.raw()
            .execute(
                "UPDATE job_items SET log_tail=?1 WHERE id=1",
                ["é\n".repeat(30_000)],
            )
            .unwrap();
        let body = read(app.clone()).await;
        assert_eq!(body["meta"]["truncated"], true);
        assert!(body["data"]["log_tail"].as_str().unwrap().len() <= 64 * 1024);
        conn.raw()
            .execute(
                "UPDATE job_items SET log_tail='token=private-value' WHERE id=1",
                [],
            )
            .unwrap();
        let response = app.oneshot(get()).await.unwrap();
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        assert!(
            !std::str::from_utf8(&body)
                .unwrap()
                .contains("private-value")
        );
    }
}
