//! The Auslöser of a job as the server derives it, and its facet in the job list.
//!
//! Jobs from schedules are stored with `trigger='manual'`, so the stored value alone cannot tell them
//! from jobs queued by hand. A job is from a schedule (`schedule`) when a run of a schedule points at
//! it, or, once that schedule and its runs are deleted, when its settings snapshot still names one.
//! A retry (`retry`) stays a retry even though it keeps the snapshot of its origin. By hand
//! (`manual`) is every other job stored as `manual`.
//!
//! The facet takes the Auslöser as `trigger` and single schedules as `schedule_id`, both comma
//! lists; together they are alternatives (OR): `trigger=manual&schedule_id=…` shows the jobs by hand and those
//! from that schedule.
use crate::facets::{Alongside, Facet, Tokens, in_list};
use rusqlite::types::Value;

/// The Auslöser tokens in display order.
pub(crate) const TRIGGERS: [&str; 3] = ["manual", "schedule", "retry"];

/// Most single schedules one filter may name.
const MAX_SCHEDULES: usize = 50;
/// Longest accepted `schedule_id` list, in characters: an opaque id is well under 64.
const MAX_SCHEDULE_PARAM: usize = MAX_SCHEDULES * 64;

/// SQL: the schedule job `j` came from, NULL for any other job.
pub(crate) const SCHEDULE_OF: &str = "(CASE WHEN j.trigger='retry' THEN NULL ELSE COALESCE((SELECT r.schedule_id FROM schedule_runs r WHERE r.job_id=j.id ORDER BY r.id LIMIT 1),CASE WHEN json_valid(j.settings_snapshot_json) THEN json_extract(j.settings_snapshot_json,'$.schedule_id') END) END)";

/// SQL: the derived Auslöser of job `j`; a stored value other than `manual` and `retry` is kept.
pub(crate) fn trigger_of() -> String {
    format!(
        "(CASE WHEN j.trigger='retry' THEN 'retry' WHEN {SCHEDULE_OF} IS NOT NULL THEN 'schedule' ELSE j.trigger END)"
    )
}

fn trigger_tokens() -> Vec<&'static str> {
    TRIGGERS.to_vec()
}

/// The facet Auslöser of the job list: a job with any of the chosen Auslöser or single schedules
/// shows. Unknown tokens and foreign schedule ids are refused.
pub(crate) fn facet() -> Facet<'static> {
    Facet::new(
        "trigger",
        Tokens::Fixed(trigger_tokens),
        Box::new(|chosen, values| {
            let (schedules, triggers): (Vec<Value>, Vec<Value>) = chosen
                .iter()
                .cloned()
                .partition(|value| matches!(value, Value::Integer(_)));
            let mut any = Vec::new();
            if !triggers.is_empty() {
                any.push(in_list(&trigger_of(), &triggers, values));
            }
            if !schedules.is_empty() {
                any.push(in_list(SCHEDULE_OF, &schedules, values));
            }
            format!("({})", any.join(" OR "))
        }),
    )
    .alongside(Alongside {
        key: "schedule_id",
        prefix: "sc",
        options_sql: "SELECT id FROM schedules WHERE action!='scan' ORDER BY lower(name),id",
        max_len: MAX_SCHEDULE_PARAM,
        max_count: MAX_SCHEDULES,
    })
}

/// The schedules a job can come from (every one but scans), by name: their ids and names.
pub(crate) fn job_schedules(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut statement =
        conn.prepare("SELECT id,name FROM schedules WHERE action!='scan' ORDER BY lower(name),id")?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}
