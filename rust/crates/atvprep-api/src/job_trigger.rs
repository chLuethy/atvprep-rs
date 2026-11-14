//! The Auslöser of a job as the server derives it, and its facet in the job list.
//!
//! Jobs from schedules are stored with `trigger='manual'`, so the stored value alone cannot tell them
//! from jobs queued by hand. A job is from a schedule (`schedule`) when a run of a schedule points at
//! it, or, once that schedule and its runs are deleted, when its settings snapshot still names one.
//! A retry (`retry`) stays a retry even though it keeps the snapshot of its origin. By hand
//! (`manual`) is every other job stored as `manual`.
//!
//! The facet takes the Auslöser as `trigger` and single schedules as `schedule`, both comma lists;
//! together they are alternatives (OR): `trigger=manual&schedule=…` shows the jobs by hand and those
//! from that schedule.
use crate::opaque::Opaque;
use rusqlite::types::Value;

/// The Auslöser tokens in display order.
pub(crate) const TRIGGERS: [&str; 3] = ["manual", "schedule", "retry"];

/// Most single schedules one filter may name.
const MAX_SCHEDULES: usize = 50;
/// Longest accepted `schedule` list, in characters: an opaque id is well under 64.
const MAX_SCHEDULE_PARAM: usize = MAX_SCHEDULES * 64;

/// SQL: the schedule job `j` came from, NULL for any other job.
pub(crate) const SCHEDULE_OF: &str = "(CASE WHEN j.trigger='retry' THEN NULL ELSE COALESCE((SELECT r.schedule_id FROM schedule_runs r WHERE r.job_id=j.id ORDER BY r.id LIMIT 1),CASE WHEN json_valid(j.settings_snapshot_json) THEN json_extract(j.settings_snapshot_json,'$.schedule_id') END) END)";

/// SQL: the derived Auslöser of job `j`; a stored value other than `manual` and `retry` is kept.
pub(crate) fn trigger_of() -> String {
    format!(
        "(CASE WHEN j.trigger='retry' THEN 'retry' WHEN {SCHEDULE_OF} IS NOT NULL THEN 'schedule' ELSE j.trigger END)"
    )
}

/// The chosen Auslöser and single schedules; both empty is every job.
#[derive(Clone, Default)]
pub(crate) struct TriggerChoice {
    triggers: Vec<&'static str>,
    schedules: Vec<i64>,
}

impl TriggerChoice {
    /// Check `trigger` and `schedule` from the query; unknown tokens and foreign ids are refused.
    pub(crate) fn parse(
        trigger: Option<&str>,
        schedule: Option<&str>,
        opaque: &Opaque,
    ) -> Result<Self, &'static str> {
        let trigger = trigger.unwrap_or_default();
        let schedule = schedule.unwrap_or_default();
        if trigger.chars().count() > 128 || schedule.chars().count() > MAX_SCHEDULE_PARAM {
            return Err("invalid request");
        }
        let asked: Vec<&str> = trigger.split(',').filter(|t| !t.is_empty()).collect();
        if asked.iter().any(|token| !TRIGGERS.contains(token)) {
            return Err("invalid request");
        }
        let mut schedules = Vec::new();
        for token in schedule.split(',').filter(|t| !t.is_empty()) {
            let id = opaque
                .decode(token, "sc")
                .ok()
                .filter(|id| *id > 0)
                .ok_or("invalid request")?;
            if !schedules.contains(&id) {
                schedules.push(id);
            }
        }
        if schedules.len() > MAX_SCHEDULES {
            return Err("invalid request");
        }
        Ok(Self {
            triggers: TRIGGERS
                .into_iter()
                .filter(|token| asked.contains(token))
                .collect(),
            schedules,
        })
    }

    /// Only this one Auslöser, for its counter.
    pub(crate) fn only_trigger(token: &'static str) -> Self {
        Self {
            triggers: vec![token],
            schedules: Vec::new(),
        }
    }

    /// Only this one schedule, for its counter.
    pub(crate) fn only_schedule(id: i64) -> Self {
        Self {
            triggers: Vec::new(),
            schedules: vec![id],
        }
    }

    /// A clause true for a job with any of the choices, binding its values; None for no choice.
    pub(crate) fn clause(&self, values: &mut Vec<Value>) -> Option<String> {
        let mut any = Vec::new();
        if !self.triggers.is_empty() {
            values.extend(self.triggers.iter().map(|t| Value::from((*t).to_owned())));
            any.push(format!(
                "{} IN ({})",
                trigger_of(),
                vec!["?"; self.triggers.len()].join(",")
            ));
        }
        if !self.schedules.is_empty() {
            values.extend(self.schedules.iter().map(|id| Value::from(*id)));
            any.push(format!(
                "{SCHEDULE_OF} IN ({})",
                vec!["?"; self.schedules.len()].join(",")
            ));
        }
        (!any.is_empty()).then(|| format!("({})", any.join(" OR ")))
    }
}

/// The schedules a job can come from (every one but scans), by name: their ids and names.
pub(crate) fn job_schedules(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut statement =
        conn.prepare("SELECT id,name FROM schedules WHERE action!='scan' ORDER BY lower(name),id")?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}
