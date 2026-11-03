//! Jobs and their items: queueing, claiming under the executor lease, heartbeats, retries.
//! Status changes go through the state machine (`state_machine.rs`), not through this module.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, named_params};
use serde_json::{Value, json};

use crate::clock::format_micros;
use crate::events::Events;
use crate::models::{Job, JobItem};
use crate::repo::Ctx;
use crate::snapshots::{InputSnapshot, InputSnapshots, build_for_media, compare_snapshot};
use crate::{Error, Result};

const ACTIONS: [&str; 4] = ["inspect", "verify", "fix", "convert"];
const TRIGGERS: [&str; 3] = ["automatic", "manual", "retry"];
const ITEM_DONE: &str = "('succeeded','failed','cancelled','interrupted','stale_input')";

/// What a new job is made of.
pub struct NewJob<'a> {
    pub action: &'a str,
    pub media_ids: &'a [i64],
    pub trigger: &'a str,
    pub settings_revision: i64,
    pub settings_snapshot: Value,
    /// Raw input overrides stored on the item, by media id.
    pub input_snapshots: BTreeMap<i64, Value>,
}

impl<'a> NewJob<'a> {
    #[must_use]
    pub fn new(action: &'a str, media_ids: &'a [i64]) -> Self {
        Self {
            action,
            media_ids,
            trigger: "automatic",
            settings_revision: 0,
            settings_snapshot: json!({}),
            input_snapshots: BTreeMap::new(),
        }
    }
}

/// The optional global limits of [`Jobs::claim_item`].
#[derive(Default)]
pub struct ClaimLimits<'a> {
    pub global_limit: Option<i64>,
    pub global_limit_key: Option<&'a str>,
    pub global_limit_default: i64,
    pub limit_class: Option<&'a str>,
}

/// One item to queue.
struct ItemSeed<'a> {
    job_id: i64,
    media_file_id: i64,
    action: &'a str,
    position: i64,
    origin_item_id: Option<i64>,
    input: &'a Value,
    settings: &'a Value,
}

pub struct Jobs<'a>(pub Ctx<'a>);

fn item_columns_in(ids: &[i64]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

impl Jobs<'_> {
    /// Persist one current control-plane setting.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn set_runtime_setting(
        &self,
        key: &str,
        value: &Value,
        value_type: &str,
        revision: i64,
    ) -> Result<()> {
        self.0.conn.execute(
            "INSERT INTO runtime_settings(key, value_json, value_type, revision, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,
                 value_type=excluded.value_type, revision=excluded.revision",
            rusqlite::params![
                key,
                value.to_string(),
                value_type,
                revision,
                self.0.clock.now()
            ],
        )?;
        Ok(())
    }

    /// A current integer control-plane setting, at least 1; the default when missing or not a number.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn read_runtime_int(&self, key: &str, default: i64) -> Result<i64> {
        let text: Option<String> = self
            .0
            .conn
            .query_row(
                "SELECT value_json FROM runtime_settings WHERE key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        let parsed = text
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_i64());
        Ok(parsed.unwrap_or(default).max(1))
    }

    fn active_mutation_exists(&self, media_ids: &[i64]) -> Result<bool> {
        let sql = format!(
            "SELECT 1 FROM job_items WHERE media_file_id IN ({}) AND action IN ('fix','convert')
             AND status NOT IN {ITEM_DONE} LIMIT 1",
            item_columns_in(media_ids)
        );
        Ok(self
            .0
            .conn
            .query_row(&sql, [], |_| Ok(()))
            .optional()?
            .is_some())
    }

    fn insert_item(&self, seed: &ItemSeed<'_>) -> Result<i64> {
        self.0.conn.execute(
            "INSERT INTO job_items(job_id, media_file_id, action, position, origin_item_id, status,
                                   input_snapshot_json, settings_snapshot_json)
             VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?7)",
            rusqlite::params![
                seed.job_id,
                seed.media_file_id,
                seed.action,
                seed.position,
                seed.origin_item_id,
                seed.input.to_string(),
                seed.settings.to_string()
            ],
        )?;
        let id = self.0.conn.last_insert_rowid();
        // The durable, comparable snapshot is built from live database state, so execution
        // references exactly the queued input.
        if let Some(snapshot) = build_for_media(self.0, seed.media_file_id)? {
            InputSnapshots(self.0).record(id, &snapshot.to_json())?;
        }
        Ok(id)
    }

    /// Queue a job with one item per media id.
    ///
    /// # Errors
    ///
    /// No media, an unknown action or trigger, or an unfinished `fix`/`convert` item for a file.
    pub fn create(&self, new: &NewJob<'_>) -> Result<Job> {
        if new.media_ids.is_empty() {
            return Err(Error::Invalid(
                "job must contain at least one media item".to_owned(),
            ));
        }
        if !ACTIONS.contains(&new.action) || !TRIGGERS.contains(&new.trigger) {
            return Err(Error::Invalid("invalid job action or trigger".to_owned()));
        }
        if matches!(new.action, "fix" | "convert") && self.active_mutation_exists(new.media_ids)? {
            return Err(Error::Invalid(
                "active job item already exists for media".to_owned(),
            ));
        }
        self.0.conn.execute(
            "INSERT INTO jobs(action, status, trigger, settings_revision, settings_snapshot_json,
                              total_count, requested_at)
             VALUES (?1, 'queued', ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                new.action,
                new.trigger,
                new.settings_revision,
                new.settings_snapshot.to_string(),
                i64::try_from(new.media_ids.len()).unwrap_or(i64::MAX),
                self.0.clock.now()
            ],
        )?;
        let job_id = self.0.conn.last_insert_rowid();
        for (position, media_id) in (0_i64..).zip(new.media_ids) {
            let input = new
                .input_snapshots
                .get(media_id)
                .cloned()
                .unwrap_or_else(|| json!({}));
            self.insert_item(&ItemSeed {
                job_id,
                media_file_id: *media_id,
                action: new.action,
                position,
                origin_item_id: None,
                input: &input,
                settings: &new.settings_snapshot,
            })?;
        }
        Events(self.0).append(
            job_id,
            "job.created",
            "job",
            job_id,
            &json!({"action": new.action, "item_count": new.media_ids.len()}),
            None,
        )?;
        self.require(job_id)
    }

    /// # Errors
    ///
    /// SQLite fails.
    pub fn get(&self, job_id: i64) -> Result<Option<Job>> {
        Ok(self
            .0
            .conn
            .query_row("SELECT * FROM jobs WHERE id = ?1", [job_id], Job::from_row)
            .optional()?)
    }

    fn require(&self, job_id: i64) -> Result<Job> {
        self.get(job_id)?
            .ok_or_else(|| Error::Invalid(format!("unknown job id: {job_id}")))
    }

    /// # Errors
    ///
    /// SQLite fails.
    pub fn get_item(&self, item_id: i64) -> Result<Option<JobItem>> {
        Ok(self
            .0
            .conn
            .query_row(
                "SELECT * FROM job_items WHERE id = ?1",
                [item_id],
                JobItem::from_row,
            )
            .optional()?)
    }

    /// The items of a job in queue order.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn items(&self, job_id: i64) -> Result<Vec<JobItem>> {
        let mut statement = self
            .0
            .conn
            .prepare("SELECT * FROM job_items WHERE job_id = ?1 ORDER BY position, id")?;
        let rows = statement.query_map([job_id], JobItem::from_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Hide finished jobs from Activity. Rows, items and events are kept.
    ///
    /// # Errors
    ///
    /// Only finished statuses may be cleared.
    pub fn clear_finished(&self, statuses: &[&str]) -> Result<usize> {
        let allowed = ["succeeded", "partial_success", "failed", "cancelled"];
        if statuses.is_empty() || !statuses.iter().all(|s| allowed.contains(s)) {
            return Err(Error::Invalid(
                "only finished jobs can be cleared".to_owned(),
            ));
        }
        let list = statuses
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(",");
        Ok(self.0.conn.execute(
            &format!(
                "UPDATE jobs SET cleared_at = ?1 WHERE status IN ({list}) AND cleared_at IS NULL"
            ),
            [self.0.clock.now()],
        )?)
    }

    /// Refuse a retry when a file is no longer the one the job was queued for.
    fn require_same_files(&self, items: &[JobItem]) -> Result<()> {
        let snapshots = InputSnapshots(self.0);
        for old in items {
            let Some(stored) = snapshots.get_for_item(old.id)? else {
                return Err(Error::Invalid(
                    "the job has no recorded file identity; queue it again".to_owned(),
                ));
            };
            let stored = InputSnapshot::from_json(&stored)?;
            let current = snapshots.build_current(old.id)?;
            let changed = compare_snapshot(&stored, current.as_ref())
                .iter()
                .any(|r| r.starts_with("file.") || r == "media.missing");
            if changed {
                return Err(Error::Invalid(
                    "a file changed since the job was queued; queue it again".to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// Queue a new job for the failed, stale or interrupted items of a finished job.
    ///
    /// # Errors
    ///
    /// Unknown job, nothing to retry, an unfinished sibling, an unfinished mutation of a file, or
    /// (for prepare jobs) a file that changed since.
    pub fn retry(&self, job_id: i64, approved_by: Option<i64>) -> Result<Job> {
        let original = self.require(job_id)?;
        let old_items = self.items(job_id)?;
        let retry_items: Vec<&JobItem> = old_items
            .iter()
            .filter(|i| matches!(i.status.as_str(), "failed" | "stale_input" | "interrupted"))
            .collect();
        let finished = [
            "succeeded",
            "cancelled",
            "failed",
            "stale_input",
            "interrupted",
        ];
        if retry_items.is_empty()
            || old_items
                .iter()
                .any(|i| !finished.contains(&i.status.as_str()))
        {
            return Err(Error::Invalid(
                "job has no retryable failed items".to_owned(),
            ));
        }
        let media_ids: Vec<i64> = retry_items.iter().map(|i| i.media_file_id).collect();
        if matches!(original.action.as_str(), "fix" | "convert")
            && self.active_mutation_exists(&media_ids)?
        {
            return Err(Error::Invalid(
                "active job item already exists for media".to_owned(),
            ));
        }
        let mut snapshot = original.settings_snapshot.clone();
        if snapshot.get("pipeline").and_then(Value::as_str) == Some("prepare") {
            // A retry is a new decision by the person who clicked it, about the file the job was for.
            let owned: Vec<JobItem> = retry_items.iter().map(|i| (*i).clone()).collect();
            self.require_same_files(&owned)?;
            if let Some(object) = snapshot.as_object_mut() {
                object.remove("approval_mac");
                if let Some(person) = approved_by {
                    object.insert("approved_by".to_owned(), json!(person));
                    object.insert(
                        "approved_at".to_owned(),
                        json!(self.0.clock.now_micros() / 1_000_000),
                    );
                }
            }
        }
        self.0.conn.execute(
            "INSERT INTO jobs(action, status, trigger, origin_job_id, settings_revision,
                              settings_snapshot_json, total_count, requested_at)
             VALUES (?1, 'queued', 'retry', ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                original.action,
                job_id,
                original.settings_revision,
                snapshot.to_string(),
                i64::try_from(retry_items.len()).unwrap_or(i64::MAX),
                self.0.clock.now()
            ],
        )?;
        let new_id = self.0.conn.last_insert_rowid();
        for old in &retry_items {
            self.insert_item(&ItemSeed {
                job_id: new_id,
                media_file_id: old.media_file_id,
                action: &old.action,
                position: old.position,
                origin_item_id: Some(old.id),
                input: &old.input_snapshot,
                settings: &snapshot,
            })?;
        }
        Events(self.0).append(
            new_id,
            "job.retry_created",
            "job",
            new_id,
            &json!({"origin_job_id": job_id}),
            None,
        )?;
        self.require(new_id)
    }

    /// Claim the next queued item of a job (jobs that are queued or running only). With a global
    /// limit key the media-executor lease and fencing token must be current, and the live limit
    /// only counts active items of the same class (`verify` or heavy).
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn claim_item(
        &self,
        job_id: i64,
        worker_id: &str,
        fencing_token: i64,
        lease_seconds: i64,
        limits: &ClaimLimits<'_>,
    ) -> Result<Option<JobItem>> {
        crate::db::atomic_write(self.0, |ctx| {
            Jobs(ctx).claim_item_inner(job_id, worker_id, fencing_token, lease_seconds, limits)
        })
    }

    fn claim_item_inner(
        &self,
        job_id: i64,
        worker_id: &str,
        fencing_token: i64,
        lease_seconds: i64,
        limits: &ClaimLimits<'_>,
    ) -> Result<Option<JobItem>> {
        let now = self.0.clock.now_micros();
        if let Some(limit) = limits.global_limit {
            let running: i64 = self.0.conn.query_row(
                "SELECT count(*) FROM job_items WHERE status IN ('claimed','running')",
                [],
                |r| r.get(0),
            )?;
            if running >= limit.max(1) {
                return Ok(None);
            }
        }
        let id: Option<i64> = self
            .0
            .conn
            .query_row(
                "UPDATE job_items SET status='claimed', status_version=status_version + 1,
                 attempt=attempt + 1, claimed_by=:worker_id, executor_fencing_token=:token,
                 heartbeat_at=:now, lease_expires_at=:expires
                 WHERE id = (SELECT job_items.id FROM job_items JOIN jobs ON jobs.id=job_items.job_id
                             WHERE job_items.job_id=:job_id AND job_items.status='queued'
                               AND jobs.status IN ('queued','running')
                             ORDER BY job_items.position, job_items.id LIMIT 1)
                 AND status='queued'
                 AND EXISTS(SELECT 1 FROM deployment_gate WHERE id=1 AND mode='open' AND phase='steady')
                 AND ((:global_limit_key IS NULL AND :global_limit IS NULL)
                      OR EXISTS (SELECT 1 FROM media_executor_leases AS lease
                                 WHERE lease.id=1 AND lease.fencing_token=:token
                                   AND lease.lease_expires_at > :now))
                 AND (:global_limit_key IS NULL AND
                      (:global_limit IS NULL OR
                      (SELECT count(*) FROM job_items AS active
                       WHERE active.status IN ('claimed','running')) < :global_limit)
                      OR (:global_limit_key IS NOT NULL AND
                          (SELECT count(*) FROM job_items AS active
                           WHERE active.status IN ('claimed','running')
                             AND (:limit_class IS NULL
                                  OR (active.action='verify') = (:limit_class='verify'))) <
                          COALESCE((SELECT CAST(json_extract(value_json, '$') AS INTEGER)
                                    FROM runtime_settings WHERE key=:global_limit_key),
                                   :global_limit_default)))
                 RETURNING id",
                named_params! {
                    ":worker_id": worker_id,
                    ":token": fencing_token,
                    ":now": format_micros(now),
                    ":expires": format_micros(now + lease_seconds * 1_000_000),
                    ":job_id": job_id,
                    ":global_limit": limits.global_limit,
                    ":global_limit_key": limits.global_limit_key,
                    ":global_limit_default": limits.global_limit_default.max(1),
                    ":limit_class": limits.limit_class,
                },
                |r| r.get(0),
            )
            .optional()?;
        id.map_or(Ok(None), |id| self.get_item(id))
    }

    /// Claim under the live media setting and the active lease. A verify only reads the file, so it
    /// is bounded by `max_probe_workers` among verifies; fixes and conversions rewrite files and
    /// share `max_ffmpeg_workers`.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn claim_media_item(
        &self,
        job_id: i64,
        worker_id: &str,
        fencing_token: i64,
        lease_seconds: i64,
        setting_default: i64,
    ) -> Result<Option<JobItem>> {
        let action: Option<String> = self
            .0
            .conn
            .query_row("SELECT action FROM jobs WHERE id = ?1", [job_id], |r| {
                r.get(0)
            })
            .optional()?;
        let verifying = action.as_deref() == Some("verify");
        let limits = ClaimLimits {
            global_limit: None,
            global_limit_key: Some(if verifying {
                "max_probe_workers"
            } else {
                "max_ffmpeg_workers"
            }),
            global_limit_default: if verifying { 2 } else { setting_default },
            limit_class: Some(if verifying { "verify" } else { "heavy" }),
        };
        self.claim_item(job_id, worker_id, fencing_token, lease_seconds, &limits)
    }

    /// Show a running item's latest output. Only its owner may, and only while it runs.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn set_item_log_tail(&self, item_id: i64, fencing_token: i64, tail: &str) -> Result<bool> {
        Ok(self.0.conn.execute(
            "UPDATE job_items SET log_tail=?3
             WHERE id=?1 AND executor_fencing_token=?2 AND status='running'",
            rusqlite::params![item_id, fencing_token, tail],
        )? == 1)
    }

    /// Record how far a running item has come. Only its owner may, and never backwards.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn set_item_progress(
        &self,
        item_id: i64,
        fencing_token: i64,
        percent: i64,
    ) -> Result<bool> {
        Ok(self.0.conn.execute(
            "UPDATE job_items SET progress=?3
             WHERE id=?1 AND executor_fencing_token=?2 AND status='running'
               AND (progress IS NULL OR progress < ?3)",
            rusqlite::params![item_id, fencing_token, percent.clamp(0, 100)],
        )? == 1)
    }

    /// Extend the lease on a claimed or running item. `false` when the item is no longer this
    /// worker's: fencing mismatch, executor lease lost or expired, or the status changed.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn heartbeat_item(
        &self,
        item_id: i64,
        fencing_token: i64,
        lease_seconds: i64,
    ) -> Result<bool> {
        let now = self.0.clock.now_micros();
        Ok(self.0.conn.execute(
            "UPDATE job_items SET heartbeat_at=?3, lease_expires_at=?4
             WHERE id=?1 AND executor_fencing_token=?2 AND status IN ('claimed','running')
               AND EXISTS (SELECT 1 FROM media_executor_leases
                           WHERE id=1 AND fencing_token=?2 AND lease_expires_at > ?3)",
            rusqlite::params![
                item_id,
                fencing_token,
                format_micros(now),
                format_micros(now + lease_seconds * 1_000_000)
            ],
        )? == 1)
    }
}
