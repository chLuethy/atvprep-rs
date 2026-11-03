//! Durable bounded event reads shared by replay and the publisher.
use crate::{
    opaque::Opaque,
    read_events::{self, EventRow},
    state::AppState,
};
use serde_json::{Value, json};

pub async fn boundary(state: AppState) -> Result<(i64, Option<i64>), ()> {
    tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        conn.raw()
            .query_row(
                "SELECT COALESCE(max(id),0),min(id) FROM events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| ())
    })
    .await
    .map_err(|_| ())?
}

pub async fn page(state: AppState, after: i64, through: i64) -> Result<Vec<EventRow>, ()> {
    tokio::task::spawn_blocking(move || {
        let conn=state.db.conn().map_err(|_|())?;
        let mut statement=conn.raw().prepare("SELECT id,job_id,item_id,topic,entity_type,entity_id,CASE WHEN length(CAST(payload_json AS BLOB))<=16384 THEN payload_json ELSE '{\"truncated\":true}' END,created_at FROM events WHERE id>?1 AND id<=?2 ORDER BY id LIMIT 100").map_err(|_|())?;
        statement.query_map(rusqlite::params![after,through],read_events::row).map_err(|_|())?.collect::<Result<Vec<_>,_>>().map_err(|_|())
    }).await.map_err(|_|())?
}

pub fn frame(row: &EventRow, opaque: &Opaque) -> Value {
    let event = read_events::view(row, opaque);
    let id = opaque.encode("cursor:events", row.id);
    if row.topic == "job.log" {
        let payload = &event["payload"];
        let text = payload
            .get("text")
            .or_else(|| payload.get("log_tail"))
            .and_then(Value::as_str)
            .unwrap_or("");
        json!({"type":"log_tail","id":id,"sequence":row.id,"item_id":event["item_id"],"offset":payload.get("offset").and_then(Value::as_u64).unwrap_or(0).min((1_u64<<53)-1),"text":text})
    } else {
        json!({"type":"event","id":id,"sequence":row.id,"topic":crate::read_jobs::safe_head(&row.topic,256),"payload":event["payload"],"entity_type":event["entity_type"],"entity_id":event["entity_id"]})
    }
}
