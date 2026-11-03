//! Bounded, redacted event history for the dashboard and job views.
use crate::{
    extract::AuthenticatedSession,
    opaque::Opaque,
    read::{error, page_meta, timestamp},
    read_jobs::safe_head,
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::NaiveDateTime;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Deserialize)]
pub struct EventQuery {
    #[serde(default, deserialize_with = "crate::opaque::bounded_cursor")]
    after: Option<String>,
    limit: Option<usize>,
}

pub(crate) struct EventRow {
    pub(crate) id: i64,
    job: Option<i64>,
    pub(crate) item: Option<i64>,
    pub(crate) topic: String,
    entity_type: String,
    entity_id: i64,
    payload: String,
    pub(crate) created: String,
}

pub(crate) fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        id: row.get(0)?,
        job: row.get(1)?,
        item: row.get(2)?,
        topic: row.get(3)?,
        entity_type: row.get(4)?,
        entity_id: row.get(5)?,
        payload: row.get(6)?,
        created: row.get(7)?,
    })
}

pub(crate) fn view(event: &EventRow, opaque: &Opaque) -> Value {
    let payload: Value =
        serde_json::from_str(&event.payload).unwrap_or_else(|_| json!({"truncated":true}));
    let payload = crate::redact::bounded_event_payload(&payload);
    let kind = if event.entity_type == "job" { "j" } else { "i" };
    json!({"id":opaque.encode("e",event.id),"sequence":event.id,"job_id":event.job.map(|id|opaque.encode("j",id)),
        "item_id":event.item.map(|id|opaque.encode("i",id)),"topic":safe_head(&event.topic,2048),
        "entity_type":safe_head(&event.entity_type,2048),"entity_id":opaque.encode(kind,event.entity_id),
        "payload":payload,"created_at":timestamp(&event.created)})
}

fn time(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f"))
        .ok()
}

pub(crate) fn coalesce(rows: Vec<EventRow>) -> Vec<EventRow> {
    let mut slots: Vec<Option<EventRow>> = Vec::new();
    let mut last: HashMap<i64, (usize, Option<NaiveDateTime>)> = HashMap::new();
    for row in rows {
        if row.topic == "job.log"
            && let Some(item) = row.item
        {
            let created = time(&row.created);
            if let Some((index, Some(previous))) = last.get(&item)
                && let Some(current) = created
                && (current - *previous)
                    .num_microseconds()
                    .is_some_and(|delta| delta <= 250_000)
            {
                slots[*index] = None;
            }
            last.insert(item, (slots.len(), created));
        }
        slots.push(Some(row));
    }
    slots.into_iter().flatten().collect()
}

async fn list(state: AppState, query: EventQuery, job: Option<i64>) -> Response {
    let path = if job.is_some() {
        "/api/v1/jobs/events"
    } else {
        "/api/v1/events"
    };
    let kind = if job.is_some() {
        "cursor:job-events"
    } else {
        "cursor:events"
    };
    let limit = query.limit.unwrap_or(100);
    if !(1..=100).contains(&limit) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", path);
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let after = match query.after.as_deref() {
        Some(value) => match opaque.decode(value, kind) {
            Ok(id) if id > 0 => id,
            _ => return error(StatusCode::BAD_REQUEST, "invalid cursor", path),
        },
        None => 0,
    };
    let result=tokio::task::spawn_blocking(move ||{
        let conn=state.db.conn().map_err(|_|())?;
        if let Some(job_id)=job {
            let exists:bool=conn.raw().query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=?1)",[job_id],|row|row.get(0)).map_err(|_|())?;
            if !exists{return Ok(None);}
        }
        let boundary:Option<i64>=if job.is_none(){conn.raw().query_row("SELECT max(id) FROM events",[],|row|row.get(0)).map_err(|_|())?}else{None};
        let fetch=limit+if job.is_none(){257}else{1};
        let mut stmt=conn.raw().prepare("SELECT id,job_id,item_id,topic,entity_type,entity_id,payload_json,created_at FROM events WHERE id>?1 AND (?2 IS NULL OR job_id=?2) AND (?3 IS NULL OR id<=?3) ORDER BY id LIMIT ?4").map_err(|_|())?;
        let raw=stmt.query_map(params![after,job,boundary,i64::try_from(fetch).map_err(|_|())?],row).map_err(|_|())?.collect::<Result<Vec<_>,_>>().map_err(|_|())?;
        let raw_more=raw.len()>limit;
        let raw_last=raw.last().map(|row|row.id);
        let rows=if job.is_none(){coalesce(raw)}else{raw};
        let overflow=rows.len()>limit;
        let mut data=Vec::new();let mut used=0usize;let mut last=None;let mut bounded=false;
        for event in rows.into_iter().take(limit){
            let value=view(&event,&opaque);let size=serde_json::to_vec(&value).map_err(|_|())?.len();
            if size>crate::bounds::MAX_ELEMENT_BYTES || used.saturating_add(size).saturating_add(data.len()+512)>crate::bounds::MAX_RESPONSE_BYTES{bounded=true;break;}
            used+=size;last=Some(event.id);data.push(value);
        }
        let next_id=if bounded {last} else if raw_more {if overflow {last}else{raw_last}} else {None};
        let next=next_id.map(|id|opaque.encode(kind,id));
        let mut meta=page_meta(Some(limit),next.as_deref(),bounded);
        if job.is_none(){meta["current_cursor"]=json!(boundary.map(|id|opaque.encode(kind,id)));meta["current_sequence"]=json!(boundary.unwrap_or(0));}
        Ok::<_,()>(Some(json!({"data":data,"meta":meta})))
    }).await;
    match result {
        Ok(Ok(Some(body))) => Json(body).into_response(),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "resource not found", path),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            path,
        ),
    }
}

/// List global events through a snapshot boundary.
pub async fn events(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<EventQuery>,
) -> Response {
    list(state, query, None).await
}

/// List events for one authenticated opaque job ID.
pub async fn job_events(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Path(token): Path<String>,
    Query(query): Query<EventQuery>,
) -> Response {
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let id = match opaque.decode(&token, "j") {
        Ok(id) if id > 0 => id,
        _ => {
            return error(
                StatusCode::NOT_FOUND,
                "resource not found",
                "/api/v1/jobs/events",
            );
        }
    };
    list(state, query, Some(id)).await
}

#[cfg(test)]
mod tests {
    use super::{EventRow, coalesce, view};
    use crate::opaque::Opaque;

    fn event(id: i64, created: &str) -> EventRow {
        EventRow {
            id,
            job: Some(1),
            item: Some(1),
            topic: "job.log".to_owned(),
            entity_type: "job_item".to_owned(),
            entity_id: 1,
            payload: r#"{"password":"hidden","path":"/private/movie"}"#.to_owned(),
            created: created.to_owned(),
        }
    }

    #[test]
    fn coalesces_only_within_the_quarter_second_window() {
        let rows = coalesce(vec![
            event(1, "2026-01-01 00:00:00.000000"),
            event(2, "2026-01-01 00:00:00.250000"),
            event(3, "2026-01-01 00:00:00.500001"),
        ]);
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn event_payload_redacts_secrets_and_paths() {
        let value = view(&event(1, "2026-01-01 00:00:00"), &Opaque::new("secret"));
        assert_eq!(value["payload"]["password"], "<redacted>");
        assert_eq!(value["payload"]["path"], "<path>");
        assert!(!value.to_string().contains("hidden"));
    }
}
