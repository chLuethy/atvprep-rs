//! Versioned, optimistic-concurrency policy for durable jobs and job items (port of
//! `atvprep_manager/jobs_state_machine.py`).

use rusqlite::{OptionalExtension, named_params};
use serde_json::{Value, json};

use crate::clock::format_micros;
use crate::events::Events;
use crate::jobs::Jobs;
use crate::repo::Ctx;
use crate::{Error, Result};

/// Declares a status enum with its database/JSON strings, parsing and `SQLite` conversions.
macro_rules! status_enum {
    ($(#[$meta:meta])* $name:ident, $what:literal, { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Every status, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The string stored in the database and sent over the wire.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl std::str::FromStr for $name {
            type Err = Error;

            fn from_str(text: &str) -> Result<Self> {
                match text {
                    $($text => Ok(Self::$variant),)+
                    other => Err(Error::Invalid(format!(concat!("unknown ", $what, " status: {}"), other))),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl rusqlite::types::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(self.as_str().into())
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                value.as_str()?.parse().map_err(|error: Error| {
                    rusqlite::types::FromSqlError::Other(error.to_string().into())
                })
            }
        }
    };
}

status_enum! {
    /// The status of one job item.
    ItemStatus, "item", {
        Queued => "queued",
        Claimed => "claimed",
        Running => "running",
        CancelRequested => "cancel_requested",
        Cancelling => "cancelling",
        Succeeded => "succeeded",
        Failed => "failed",
        Cancelled => "cancelled",
        Interrupted => "interrupted",
        StaleInput => "stale_input",
    }
}

status_enum! {
    /// The status of a job (the parent of its items).
    JobStatus, "job", {
        Queued => "queued",
        Running => "running",
        CancelRequested => "cancel_requested",
        Succeeded => "succeeded",
        PartialSuccess => "partial_success",
        Failed => "failed",
        Cancelled => "cancelled",
    }
}

impl ItemStatus {
    /// What an item may become from this status.
    #[must_use]
    pub const fn targets(self) -> &'static [Self] {
        use ItemStatus::{
            CancelRequested, Cancelled, Cancelling, Claimed, Failed, Interrupted, Queued, Running,
            StaleInput, Succeeded,
        };
        match self {
            Queued => &[Claimed, Cancelled, StaleInput],
            Claimed => &[Running, CancelRequested, Cancelled, Interrupted, StaleInput],
            Running => &[
                Succeeded,
                Failed,
                StaleInput,
                CancelRequested,
                Cancelled,
                Interrupted,
            ],
            CancelRequested => &[Cancelling, Interrupted],
            Cancelling => &[Cancelled, Failed, Interrupted],
            Interrupted => &[Failed],
            Succeeded | Failed | Cancelled | StaleInput => &[],
        }
    }

    /// Whether `to` is an allowed next status.
    #[must_use]
    pub fn can_become(self, to: Self) -> bool {
        self.targets().contains(&to)
    }

    /// No further transition is expected: the item is finished one way or another.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        match self {
            Self::Succeeded
            | Self::Failed
            | Self::StaleInput
            | Self::Cancelled
            | Self::Interrupted => true,
            Self::Queued
            | Self::Claimed
            | Self::Running
            | Self::CancelRequested
            | Self::Cancelling => false,
        }
    }
}

impl JobStatus {
    /// What a job may become from this status.
    #[must_use]
    pub const fn targets(self) -> &'static [Self] {
        use JobStatus::{
            CancelRequested, Cancelled, Failed, PartialSuccess, Queued, Running, Succeeded,
        };
        match self {
            Queued => &[Running, CancelRequested, Cancelled, Failed, PartialSuccess],
            Running => &[
                CancelRequested,
                Succeeded,
                PartialSuccess,
                Failed,
                Cancelled,
            ],
            CancelRequested => &[Cancelled, Failed, PartialSuccess, Succeeded],
            Succeeded | PartialSuccess | Failed | Cancelled => &[],
        }
    }

    /// Whether `to` is an allowed next status.
    #[must_use]
    pub fn can_become(self, to: Self) -> bool {
        self.targets().contains(&to)
    }

    /// No further transition is expected: the job is finished one way or another.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        match self {
            Self::Succeeded | Self::PartialSuccess | Self::Failed | Self::Cancelled => true,
            Self::Queued | Self::Running | Self::CancelRequested => false,
        }
    }
}

/// What an item may become from each status (string form; unknown statuses allow nothing).
#[must_use]
pub fn item_allowed(from: &str, to: &str) -> bool {
    match (from.parse::<ItemStatus>(), to.parse::<ItemStatus>()) {
        (Ok(from), Ok(to)) => from.can_become(to),
        _ => false,
    }
}

/// What a job may become from each status (string form; unknown statuses allow nothing).
#[must_use]
pub fn parent_allowed(from: &str, to: &str) -> bool {
    match (from.parse::<JobStatus>(), to.parse::<JobStatus>()) {
        (Ok(from), Ok(to)) => from.can_become(to),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionResult {
    pub entity_id: i64,
    pub status: String,
    pub status_version: i64,
    pub changed: bool,
}

/// The deterministic parent status for a non-empty item snapshot.
///
/// # Errors
///
/// `statuses` is empty.
pub fn aggregate_parent_status(statuses: &[&str]) -> Result<&'static str> {
    if statuses.is_empty() {
        return Err(Error::Invalid(
            "job must contain at least one item".to_owned(),
        ));
    }
    let any = |wanted: &[&str]| statuses.iter().any(|s| wanted.contains(s));
    let all = |wanted: &str| statuses.iter().all(|s| *s == wanted);
    Ok(if any(&["queued", "claimed"]) {
        if all("queued") { "queued" } else { "running" }
    } else if any(&["running"]) {
        "running"
    } else if any(&["cancel_requested", "cancelling"]) {
        "cancel_requested"
    } else if all("succeeded") {
        "succeeded"
    } else if all("cancelled") {
        "cancelled"
    } else if any(&["succeeded"]) {
        // An interrupted item whose retry runs as its own job is finished here; it counts as failed.
        "partial_success"
    } else {
        "failed"
    })
}

/// A timestamp as the lease stores it, with or without `T`, `Z` or `+00:00`, in a form that
/// compares like time; `None` when it is not a timestamp.
fn comparable(text: &str) -> Option<String> {
    let text = text.replace('T', " ");
    let text = text
        .strip_suffix('Z')
        .or_else(|| text.strip_suffix("+00:00"))
        .unwrap_or(&text);
    let bytes = text.as_bytes();
    let shaped = bytes.len() >= 19
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b' '
        && bytes[13] == b':'
        && bytes[16] == b':';
    shaped.then(|| text.to_owned())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Item,
    Job,
}

pub struct JobStateMachine<'a>(pub Ctx<'a>);

struct Entity {
    id: i64,
    status: String,
    version: i64,
    fencing: Option<i64>,
    job_id: i64,
}

impl JobStateMachine<'_> {
    fn load(&self, kind: Kind, id: i64) -> Result<Entity> {
        let sql = match kind {
            Kind::Item => {
                "SELECT id, status, status_version, executor_fencing_token, job_id FROM job_items WHERE id=?1"
            }
            Kind::Job => "SELECT id, status, status_version, NULL, id FROM jobs WHERE id=?1",
        };
        self.0
            .conn
            .query_row(sql, [id], |r| {
                Ok(Entity {
                    id: r.get(0)?,
                    status: r.get(1)?,
                    version: r.get(2)?,
                    fencing: r.get(3)?,
                    job_id: r.get(4)?,
                })
            })
            .optional()?
            .ok_or_else(|| {
                Error::Invalid(match kind {
                    Kind::Item => format!("unknown job item id: {id}"),
                    Kind::Job => format!("unknown job id: {id}"),
                })
            })
    }

    /// Move an item. A worker (any status but `queued`) must present the item's fencing token,
    /// and while the global media lease is in use it must be the current holder's.
    ///
    /// # Errors
    ///
    /// Unknown item, [`Error::Conflict`] for a stale version, token or lease, [`Error::Illegal`].
    pub fn transition_item(
        &self,
        item_id: i64,
        target: &str,
        expected_version: i64,
        fencing_token: Option<i64>,
        payload: Option<&Value>,
    ) -> Result<TransitionResult> {
        crate::db::atomic_write(self.0, |ctx| {
            JobStateMachine(ctx).transition_item_inner(
                item_id,
                target,
                expected_version,
                fencing_token,
                payload,
            )
        })
    }

    fn transition_item_inner(
        &self,
        item_id: i64,
        target: &str,
        expected_version: i64,
        fencing_token: Option<i64>,
        payload: Option<&Value>,
    ) -> Result<TransitionResult> {
        let entity = self.load(Kind::Item, item_id)?;
        self.transition(
            Kind::Item,
            &entity,
            target,
            expected_version,
            fencing_token,
            payload,
        )
    }

    /// Move a job.
    ///
    /// # Errors
    ///
    /// Unknown job, [`Error::Conflict`] for a stale version, [`Error::Illegal`].
    pub fn transition_job(
        &self,
        job_id: i64,
        target: &str,
        expected_version: i64,
        payload: Option<&Value>,
    ) -> Result<TransitionResult> {
        crate::db::atomic_write(self.0, |ctx| {
            JobStateMachine(ctx).transition_job_inner(job_id, target, expected_version, payload)
        })
    }

    fn transition_job_inner(
        &self,
        job_id: i64,
        target: &str,
        expected_version: i64,
        payload: Option<&Value>,
    ) -> Result<TransitionResult> {
        let entity = self.load(Kind::Job, job_id)?;
        self.transition(Kind::Job, &entity, target, expected_version, None, payload)
    }

    fn lease(&self) -> Result<Option<(i64, Option<String>)>> {
        Ok(self
            .0
            .conn
            .query_row(
                "SELECT fencing_token, lease_expires_at FROM media_executor_leases WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    #[allow(clippy::too_many_lines)]
    fn transition(
        &self,
        kind: Kind,
        entity: &Entity,
        target: &str,
        expected_version: i64,
        fencing_token: Option<i64>,
        payload: Option<&Value>,
    ) -> Result<TransitionResult> {
        let topic = if kind == Kind::Item {
            "job.item.transition"
        } else {
            "job.transition"
        };
        let current = entity.status.as_str();
        if expected_version != entity.version {
            return Err(Error::Conflict(format!(
                "stale {topic} writer: expected version {expected_version}, current {}",
                entity.version
            )));
        }
        let worker = kind == Kind::Item && current != "queued";
        if worker && fencing_token.is_none() {
            return Err(Error::Conflict(
                "worker item transition requires a fencing token".to_owned(),
            ));
        }
        if worker && fencing_token != entity.fencing {
            return Err(Error::Conflict("stale item fencing token".to_owned()));
        }
        // Transitions made under the global media lease are guarded. Callers that never took a
        // lease (token 0) stay supported; once a lease is in use, missing or expired ownership
        // fails closed.
        let now = self.0.clock.now_micros();
        let now_text = format_micros(now);
        let mut leased = false;
        if worker
            && let Some(token) = fencing_token
            && let Some((lease_token, expires)) = self.lease()?
            && lease_token != 0
        {
            leased = true;
            let live = expires
                .as_deref()
                .and_then(comparable)
                .is_some_and(|e| e > now_text);
            if !live || lease_token != token {
                return Err(Error::Conflict(
                    "global media executor lease no longer held by this worker".to_owned(),
                ));
            }
        }
        if target == current {
            return Ok(TransitionResult {
                entity_id: entity.id,
                status: current.to_owned(),
                status_version: entity.version,
                changed: false,
            });
        }
        let legal = match kind {
            Kind::Item => item_allowed(current, target),
            Kind::Job => parent_allowed(current, target),
        };
        if !legal {
            return Err(Error::Illegal(format!(
                "cannot transition {current} to {target}"
            )));
        }
        let terminal = match kind {
            Kind::Item => target
                .parse::<ItemStatus>()
                .is_ok_and(ItemStatus::is_terminal),
            Kind::Job => target
                .parse::<JobStatus>()
                .is_ok_and(JobStatus::is_terminal),
        };
        let table = if kind == Kind::Item {
            "job_items"
        } else {
            "jobs"
        };
        let mut sets = vec!["status=:target", "status_version=status_version+1"];
        let mut conditions = vec!["id=:id", "status=:current", "status_version=:expected"];
        if target == "running" {
            sets.push("started_at=COALESCE(started_at, :now)");
        }
        if terminal {
            sets.push("finished_at=COALESCE(finished_at, :now)");
        }
        if kind == Kind::Item && fencing_token.is_some() {
            if leased {
                conditions.push(
                    "EXISTS (SELECT 1 FROM media_executor_leases WHERE id=1
                             AND fencing_token=:fence AND lease_expires_at > :now)",
                );
            }
            if current == "queued" && target == "claimed" {
                conditions
                    .push("(executor_fencing_token=:fence OR executor_fencing_token IS NULL)");
                sets.push("executor_fencing_token=:fence");
            } else {
                conditions.push("executor_fencing_token=:fence");
            }
        }
        let sql = format!(
            "UPDATE {table} SET {} WHERE {}",
            sets.join(", "),
            conditions.join(" AND ")
        );
        let mut statement = self.0.conn.prepare(&sql)?;
        let mut params: Vec<(&str, &dyn rusqlite::ToSql)> = vec![
            (":target", &target),
            (":id", &entity.id),
            (":current", &current),
            (":expected", &expected_version),
            (":now", &now_text),
        ];
        let fence = fencing_token.unwrap_or(0);
        if kind == Kind::Item && fencing_token.is_some() {
            params.push((":fence", &fence));
        }
        let used: Vec<(&str, &dyn rusqlite::ToSql)> = params
            .into_iter()
            .filter(|(name, _)| sql.contains(*name))
            .collect();
        if statement.execute(used.as_slice())? != 1 {
            return Err(Error::Conflict(format!(
                "concurrent {topic} update rejected"
            )));
        }
        drop(statement);
        let mut body = json!({
            "from": current, "to": target,
            "from_version": expected_version, "to_version": expected_version + 1,
        });
        if let (Some(extra), Some(map)) = (payload.and_then(Value::as_object), body.as_object_mut())
        {
            for (key, value) in extra {
                map.insert(key.clone(), value.clone());
            }
        }
        if kind == Kind::Item {
            let parent = self.load(Kind::Job, entity.job_id)?;
            if parent.status != "cancel_requested" {
                self.aggregate_job(entity.job_id, parent.version)?;
            }
        }
        Events(self.0).append(
            entity.job_id,
            topic,
            if kind == Kind::Item { "item" } else { "job" },
            entity.id,
            &body,
            (kind == Kind::Item).then_some(entity.id),
        )?;
        Ok(TransitionResult {
            entity_id: entity.id,
            status: target.to_owned(),
            status_version: expected_version + 1,
            changed: true,
        })
    }

    /// Reconcile a parent from its current items without regressing terminals.
    ///
    /// # Errors
    ///
    /// Unknown job, no items, an illegal parent change, or a parent that stays busy.
    pub fn aggregate_job(&self, job_id: i64, expected_version: i64) -> Result<TransitionResult> {
        crate::db::atomic_write(self.0, |ctx| {
            JobStateMachine(ctx).aggregate_job_inner(job_id, expected_version)
        })
    }

    fn aggregate_job_inner(&self, job_id: i64, expected_version: i64) -> Result<TransitionResult> {
        let mut expected = expected_version;
        for _ in 0..5 {
            let job = self.load(Kind::Job, job_id)?;
            if job.version != expected {
                expected = job.version;
            }
            let statuses: Vec<String> = {
                let mut statement = self
                    .0
                    .conn
                    .prepare("SELECT status FROM job_items WHERE job_id=?1")?;
                let rows = statement.query_map([job_id], |r| r.get(0))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            let refs: Vec<&str> = statuses.iter().map(String::as_str).collect();
            let target = aggregate_parent_status(&refs)?;
            let count = |wanted: &[&str]| {
                i64::try_from(refs.iter().filter(|s| wanted.contains(s)).count())
                    .unwrap_or(i64::MAX)
            };
            self.0.conn.execute(
                "UPDATE jobs SET succeeded_count=:ok, failed_count=:failed, cancelled_count=:cancelled,
                                 total_count=:total WHERE id=:id",
                named_params! {
                    ":ok": count(&["succeeded"]),
                    ":failed": count(&["failed", "stale_input"]),
                    ":cancelled": count(&["cancelled"]),
                    ":total": i64::try_from(refs.len()).unwrap_or(i64::MAX),
                    ":id": job_id,
                },
            )?;
            match self.transition_job(job_id, target, expected, None) {
                Err(Error::Conflict(_)) => expected = self.load(Kind::Job, job_id)?.version,
                other => return other,
            }
        }
        Err(Error::Conflict(
            "parent aggregation remained concurrently busy".to_owned(),
        ))
    }

    /// Fail an item closed as `stale_input` with the divergence reasons.
    ///
    /// # Errors
    ///
    /// Unknown item or an illegal transition.
    pub fn mark_item_stale(&self, item_id: i64, reasons: &[String]) -> Result<TransitionResult> {
        let item = self.load(Kind::Item, item_id)?;
        self.transition(
            Kind::Item,
            &item,
            "stale_input",
            item.version,
            None,
            Some(&json!({"reasons": reasons})),
        )
    }

    /// Cancel queued legacy automatic work without executing any item; manual, retry, running and
    /// finished jobs stay as they are. Returns how many jobs were settled.
    ///
    /// # Errors
    ///
    /// SQLite or a transition fails.
    pub fn cancel_queued_automatic_jobs(&self) -> Result<usize> {
        let ids: Vec<i64> = {
            let mut statement = self.0.conn.prepare(
                "SELECT id FROM jobs WHERE trigger='automatic' AND status='queued'
                 AND action IN ('fix','convert','inspect','verify') ORDER BY id",
            )?;
            let rows = statement.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let mut cancelled = 0;
        for id in ids {
            let items = Jobs(self.0).items(id)?;
            if items.is_empty() || items.iter().any(|i| i.status != "queued") {
                continue;
            }
            for item in items {
                self.transition_item(item.id, "cancelled", item.status_version, None, None)?;
            }
            cancelled += 1;
        }
        Ok(cancelled + self.settle_stranded_items()?)
    }

    /// Cancel items still queued under cancelled or failed automatic jobs. Older cancellations
    /// closed the parent but left its items queued, so the UI kept showing them as waiting. Only
    /// the items change, written directly because the state machine would roll the terminal
    /// parent over to cancelled.
    fn settle_stranded_items(&self) -> Result<usize> {
        let rows: Vec<(i64, i64)> = {
            let mut statement = self.0.conn.prepare(
                "SELECT job_items.id, job_items.job_id FROM job_items
                 JOIN jobs ON jobs.id = job_items.job_id
                 WHERE jobs.trigger='automatic' AND jobs.status IN ('cancelled','failed')
                   AND job_items.status='queued' ORDER BY job_items.id",
            )?;
            let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for (item_id, job_id) in &rows {
            self.0.conn.execute(
                "UPDATE job_items SET status='cancelled', status_version=status_version+1 WHERE id=?1",
                [item_id],
            )?;
            Events(self.0).append(
                *job_id,
                "job.item.transition",
                "job_item",
                *item_id,
                &json!({"from": "queued", "to": "cancelled", "reason": "stranded under terminal job"}),
                None,
            )?;
        }
        let jobs: std::collections::BTreeSet<i64> = rows.iter().map(|(_, job)| *job).collect();
        Ok(jobs.len())
    }
}
