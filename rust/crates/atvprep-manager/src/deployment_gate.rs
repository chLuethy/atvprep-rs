//! Fail-closed deployment gate and work permits.

use crate::repo::Ctx;
use crate::{Error, Result};

/// Match Python's printable Unicode operator notes, including its character limit.
#[must_use]
pub fn valid_operator_text(value: &str) -> bool {
    static PRINTABLE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    PRINTABLE
        .get_or_init(|| regex::Regex::new(r"^[\p{L}\p{M}\p{N}\p{P}\p{S} ]{1,200}$").ok())
        .as_ref()
        .is_some_and(|pattern| pattern.is_match(value))
}

const HEX: &[u8; 16] = b"0123456789abcdef";
use rusqlite::OptionalExtension;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateStatus {
    pub mode: String,
    pub phase: String,
    pub generation: i64,
    pub healthy: bool,
    pub reason: Option<String>,
    pub permits: i64,
    pub quiesced: bool,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Permit {
    pub id: i64,
    pub kind: String,
    pub holder: String,
    pub generation: i64,
    token: String,
}
impl std::fmt::Debug for Permit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Permit")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("holder", &self.holder)
            .field("generation", &self.generation)
            .field("token", &"[redacted]")
            .finish()
    }
}

pub struct OpenPermit {
    pub id: i64,
    pub kind: String,
    pub holder: String,
    pub pid: Option<i64>,
    pub host: Option<String>,
    pub proc_start: Option<String>,
    pub issued_at: String,
}
impl Permit {
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }
}
/// An authenticated actor allowed to modify the deployment gate.
///
/// Values are constructed only from a validated manager session or the owner of a local database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operator {
    kind: &'static str,
    reference: String,
}
impl Operator {
    /// Require the database-owner identity established by the local CLI.
    ///
    /// # Errors
    /// Browser-session actors cannot execute an offline release window.
    pub fn require_local(&self) -> Result<()> {
        if self.kind != "local-cli" {
            return Err(Error::Invalid(
                "offline releases require a local database owner".to_owned(),
            ));
        }
        Ok(())
    }
    /// Build an operator for a previously authenticated, unrevoked admin session.
    ///
    /// # Errors
    ///
    /// The session identifier is not a positive database identifier.
    pub fn from_admin_session(session_id: i64) -> Result<Self> {
        if session_id <= 0 {
            return Err(Error::Invalid(
                "only an authorized operator can change the gate".to_owned(),
            ));
        }
        Ok(Self {
            kind: "admin-session",
            reference: session_id.to_string(),
        })
    }

    /// Build an operator for the effective user that owns an existing manager database.
    ///
    /// # Errors
    ///
    /// The database cannot be inspected, its owner differs from the effective user, or that user
    /// identity cannot be read.
    pub fn from_local_cli(database: &Path) -> Result<Self> {
        let owner = std::fs::metadata(database)
            .map_err(|_| {
                Error::Invalid("only an authorized operator can change the gate".to_owned())
            })?
            .uid();
        let output = Command::new("/usr/bin/id")
            .args(["-u"])
            .env_clear()
            .output()
            .map_err(|_| {
                Error::Invalid("only an authorized operator can change the gate".to_owned())
            })?;
        let effective_uid = String::from_utf8(output.stdout)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|_| output.status.success())
            .ok_or_else(|| {
                Error::Invalid("only an authorized operator can change the gate".to_owned())
            })?;
        if owner != effective_uid {
            return Err(Error::Invalid(
                "only an authorized operator can change the gate".to_owned(),
            ));
        }
        Ok(Self {
            kind: "local-cli",
            reference: format!("uid-{owner}"),
        })
    }
}
fn authorized(operator: &Operator) -> Result<(&str, String)> {
    if !matches!(operator.kind, "admin-session" | "local-cli") || operator.reference.is_empty() {
        return Err(Error::Invalid(
            "only an authorized operator can change the gate".to_owned(),
        ));
    }
    Ok((operator.kind, operator.reference.clone()))
}
fn token() -> Result<String> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| Error::Invalid(format!("cannot issue deployment permit: {e}")))?;

    let mut result = String::with_capacity(32);
    for byte in bytes {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 15)]));
    }
    Ok(result)
}
pub struct DeploymentGate<'a>(pub Ctx<'a>);

struct GateChange<'a> {
    conn: &'a rusqlite::Connection,
    complete: bool,
}
impl<'a> GateChange<'a> {
    fn new(conn: &'a rusqlite::Connection) -> Result<Self> {
        conn.execute_batch("SAVEPOINT atvprep_gate_change")?;
        Ok(Self {
            conn,
            complete: false,
        })
    }
    fn commit(mut self) -> Result<()> {
        self.conn
            .execute_batch("RELEASE SAVEPOINT atvprep_gate_change")?;
        self.complete = true;
        Ok(())
    }
}
impl Drop for GateChange<'_> {
    fn drop(&mut self) {
        if !self.complete {
            let _ = self.conn.execute_batch(
                "ROLLBACK TO SAVEPOINT atvprep_gate_change; RELEASE SAVEPOINT atvprep_gate_change",
            );
        }
    }
}

pub struct GateEvent {
    pub id: i64,
    pub generation: i64,
    pub kind: String,
    pub actor_kind: Option<String>,
    pub at: String,
}

impl DeploymentGate<'_> {
    /// Delete only old released permits in a bounded pass; preserve open permits and history.
    ///
    /// # Errors
    /// Rejects invalid retention limits or failed storage operations.
    pub fn prune_permits(&self, keep_days: i64, limit: i64) -> Result<usize> {
        if !(1..=3650).contains(&keep_days) || !(1..=1000).contains(&limit) {
            return Err(Error::Invalid("invalid permit retention limits".to_owned()));
        }
        let cutoff = crate::clock::format_micros(
            self.0
                .clock
                .now_micros()
                .saturating_sub(keep_days * 86_400_000_000),
        );
        Ok(self.0.conn.execute("DELETE FROM deployment_permits WHERE id IN (SELECT id FROM deployment_permits WHERE released_at IS NOT NULL AND released_at<?1 ORDER BY id LIMIT ?2)",rusqlite::params![cutoff,limit])?)
    }

    fn record_change(&self, event: &str, actor: &str) -> Result<()> {
        self.0.conn.execute("INSERT INTO deployment_events(generation,kind,actor_kind,at) SELECT generation,?1,?2,?3 FROM deployment_gate WHERE id=1",rusqlite::params![event,actor,self.0.clock.now()])?;
        Ok(())
    }

    /// Read a bounded page of the immutable gate-change history.
    ///
    /// # Errors
    /// Rejects invalid pagination bounds and database failures.
    pub fn history(&self, after: i64, limit: i64) -> Result<Vec<GateEvent>> {
        if after < 0 || !(1..=1000).contains(&limit) {
            return Err(Error::Invalid("invalid gate history bounds".to_owned()));
        }
        let mut statement=self.0.conn.prepare("SELECT id,generation,kind,actor_kind,at FROM deployment_events WHERE id>?1 ORDER BY id LIMIT ?2")?;
        Ok(statement
            .query_map(rusqlite::params![after, limit], |row| {
                Ok(GateEvent {
                    id: row.get(0)?,
                    generation: row.get(1)?,
                    kind: row.get(2)?,
                    actor_kind: row.get(3)?,
                    at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    /// Read the current status, returning a closed unhealthy state for invalid gate data.
    ///
    /// # Errors
    ///
    /// SQLite fails while reading supporting state.
    pub fn status(&self) -> Result<GateStatus> {
        let row: Option<(String, String, i64, Option<String>)> = self
            .0
            .conn
            .query_row(
                "SELECT mode,phase,generation,reason FROM deployment_gate WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((mode, phase, generation, reason)) = row else {
            return Ok(GateStatus {
                mode: "maintenance".to_owned(),
                phase: "offline-migration".to_owned(),
                generation: 0,
                healthy: false,
                reason: Some("the gate state cannot be read".to_owned()),
                permits: 0,
                quiesced: false,
            });
        };
        if !matches!(mode.as_str(), "open" | "maintenance")
            || !matches!(phase.as_str(), "steady" | "offline-migration")
            || generation < 0
        {
            return Ok(GateStatus {
                mode: "maintenance".to_owned(),
                phase: "offline-migration".to_owned(),
                generation: 0,
                healthy: false,
                reason: Some("the gate state cannot be read".to_owned()),
                permits: 0,
                quiesced: false,
            });
        }
        let permits: i64 = self.0.conn.query_row(
            "SELECT count(*) FROM deployment_permits WHERE released_at IS NULL",
            [],
            |r| r.get(0),
        )?;
        let active:i64=self.0.conn.query_row("SELECT (SELECT count(*) FROM job_items WHERE status IN ('claimed','running','cancel_requested','cancelling'))+(SELECT count(*) FROM scan_runs WHERE status IN ('claimed','running'))",[],|r|r.get(0))?;
        Ok(GateStatus {
            mode: mode.clone(),
            phase,
            generation,
            healthy: true,
            reason,
            permits,
            quiesced: mode == "maintenance" && permits == 0 && active == 0,
        })
    }
    /// Close the gate and advance its generation.
    ///
    /// # Errors
    ///
    /// The operator or reason is invalid, or SQLite fails.
    pub fn close(&self, operator: &Operator, reason: &str) -> Result<GateStatus> {
        let (kind, reference) = authorized(operator)?;
        let audit = GateChange::new(self.0.conn)?;
        if !valid_operator_text(reason) {
            return Err(Error::Invalid(
                "the reason must be 1 to 200 printable characters".to_owned(),
            ));
        }
        self.0.conn.execute("INSERT INTO deployment_gate(id,mode,phase,generation,reason,changed_by_kind,changed_by_ref,changed_at) SELECT 1,'maintenance',COALESCE((SELECT phase FROM deployment_gate WHERE id=1 AND phase IN ('steady','offline-migration')),'offline-migration'),MAX(COALESCE((SELECT CAST(generation AS INTEGER)+1 FROM deployment_gate WHERE id=1),1),COALESCE((SELECT max(generation)+1 FROM deployment_events),1)),?1,?2,?3,?4 ON CONFLICT(id) DO UPDATE SET mode=excluded.mode,phase=excluded.phase,generation=excluded.generation,reason=excluded.reason,changed_by_kind=excluded.changed_by_kind,changed_by_ref=excluded.changed_by_ref,changed_at=excluded.changed_at",rusqlite::params![reason,kind,reference,self.0.clock.now()])?;
        self.record_change("close", kind)?;
        let status = self.status()?;
        audit.commit()?;
        Ok(status)
    }
    fn refusal(&self, generation: i64, need_phase: Option<&str>, opener: Option<&str>) -> Error {
        let unhealthy = "the gate is not healthy; close it to repair it first";
        let Ok(status) = self.status() else {
            return Error::Conflict(unhealthy.to_owned());
        };
        let message = if !status.healthy {
            unhealthy
        } else if status.generation != generation {
            "the gate changed since you looked; read it again"
        } else if opener == Some("admin-session")
            && self
                .0
                .conn
                .query_row(
                    "SELECT changed_by_kind FROM deployment_gate WHERE id=1",
                    [],
                    |row| row.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
                .as_deref()
                == Some("local-cli")
        {
            "the gate was closed from the host; open it from the host"
        } else if need_phase.is_some_and(|phase| status.phase != phase) {
            if need_phase == Some("steady") {
                "the offline migration is not finished"
            } else {
                "the gate is not in an offline migration"
            }
        } else if status.mode != "maintenance" {
            "the gate is not closed"
        } else if need_phase == Some("offline-migration") && !status.quiesced {
            "work is still running"
        } else {
            "the gate change was refused"
        };
        Error::Conflict(message.to_owned())
    }

    /// Reopen a steady gate only when its generation is current.
    ///
    /// # Errors
    ///
    /// The operator is invalid, the state changed, or SQLite fails.
    pub fn open(&self, operator: &Operator, generation: i64) -> Result<GateStatus> {
        let (kind, reference) = authorized(operator)?;
        let audit = GateChange::new(self.0.conn)?;
        let n=self.0.conn.execute("UPDATE deployment_gate SET mode='open',generation=generation+1,reason=NULL,changed_by_kind=?1,changed_by_ref=?2,changed_at=?3 WHERE id=1 AND mode='maintenance' AND phase='steady' AND generation=?4 AND NOT(?1='admin-session' AND changed_by_kind='local-cli')",rusqlite::params![kind,reference,self.0.clock.now(),generation])?;
        if n == 0 {
            return Err(self.refusal(generation, Some("steady"), Some(kind)));
        }
        self.record_change("open", kind)?;
        let status = self.status()?;
        audit.commit()?;
        Ok(status)
    }
    /// Begin an offline migration while the gate is closed.
    ///
    /// # Errors
    ///
    /// The operator is invalid, the gate generation changed, or SQLite fails.
    pub fn begin_offline_migration(
        &self,
        operator: &Operator,
        generation: i64,
    ) -> Result<GateStatus> {
        let (kind, reference) = authorized(operator)?;
        let audit = GateChange::new(self.0.conn)?;
        let changed = self.0.conn.execute(
            "UPDATE deployment_gate SET phase='offline-migration', generation=generation+1,
             changed_by_kind=?1, changed_by_ref=?2, changed_at=?3
             WHERE id=1 AND mode='maintenance' AND generation=?4",
            rusqlite::params![kind, reference, self.0.clock.now(), generation],
        )?;
        if changed == 0 {
            return Err(self.refusal(generation, None, None));
        }
        self.record_change("begin-migration", kind)?;
        let status = self.status()?;
        audit.commit()?;
        Ok(status)
    }

    /// Finish an offline migration only after all work and permits stopped.
    ///
    /// # Errors
    ///
    /// The operator is invalid, work is still active, the gate generation changed, or SQLite fails.
    pub fn finish_offline_migration(
        &self,
        operator: &Operator,
        generation: i64,
    ) -> Result<GateStatus> {
        let (kind, reference) = authorized(operator)?;
        let audit = GateChange::new(self.0.conn)?;
        let changed = self.0.conn.execute(
            "UPDATE deployment_gate SET phase='steady', generation=generation+1,
             changed_by_kind=?1, changed_by_ref=?2, changed_at=?3
             WHERE id=1 AND mode='maintenance' AND phase='offline-migration' AND generation=?4
               AND NOT EXISTS (SELECT 1 FROM deployment_permits WHERE released_at IS NULL)
               AND NOT EXISTS (SELECT 1 FROM job_items WHERE status IN ('claimed','running','cancel_requested','cancelling'))
               AND NOT EXISTS (SELECT 1 FROM scan_runs WHERE status IN ('claimed','running'))",
            rusqlite::params![kind, reference, self.0.clock.now(), generation],
        )?;
        if changed == 0 {
            return Err(self.refusal(generation, Some("offline-migration"), None));
        }
        self.record_change("finish-migration", kind)?;
        let status = self.status()?;
        audit.commit()?;
        Ok(status)
    }

    /// Acquire a permit while the gate is open.
    ///
    /// # Errors
    ///
    /// The permit request is invalid, random bytes cannot be read, or SQLite fails.
    pub fn acquire_permit(&self, kind: &str, holder: &str) -> Result<Option<Permit>> {
        if !matches!(kind, "job" | "scan" | "schedule")
            || holder.is_empty()
            || holder.len() > 128
            || !holder
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:@/-".contains(&byte))
        {
            return Err(Error::Invalid("invalid permit request".to_owned()));
        }
        let token = token()?;
        crate::db::retry_busy(|| {
            let atomic = GateChange::new(self.0.conn)?;
            let row:Option<(i64,i64)>=self.0.conn.query_row("INSERT INTO deployment_permits(kind,holder,token,pid,host,proc_start,generation,issued_at) SELECT ?1,?2,?3,?4,?5,?6,generation,?7 FROM deployment_gate WHERE id=1 AND mode='open' AND phase='steady' RETURNING id,generation",rusqlite::params![kind,holder,token,i64::from(std::process::id()),crate::procid::host_name(),crate::procid::identity_of_self(),self.0.clock.now()],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
            atomic.commit()?;
            Ok(row.map(|(id, generation)| Permit {
                id,
                kind: kind.to_owned(),
                holder: holder.to_owned(),
                generation,
                token: token.clone(),
            }))
        })
    }

    /// Release a permit with its secret token.
    ///
    /// # Errors
    ///
    /// The permit does not exist or SQLite fails.
    pub fn release_permit(&self, permit: &Permit) -> Result<bool> {
        crate::db::retry_busy(|| {
            let n=self.0.conn.execute("UPDATE deployment_permits SET released_at=?3 WHERE id=?1 AND token=?2 AND released_at IS NULL",rusqlite::params![permit.id,permit.token,self.0.clock.now()])?;
            if n == 0 {
                let known: Option<i64> = self
                    .0
                    .conn
                    .query_row(
                        "SELECT id FROM deployment_permits WHERE id=?1 AND token=?2",
                        rusqlite::params![permit.id, permit.token],
                        |r| r.get(0),
                    )
                    .optional()?;
                if known.is_none() {
                    return Err(Error::NotFound("no such permit".to_owned()));
                }
            }
            Ok(n == 1)
        })
    }

    /// List a bounded set of open permits without exposing holder tokens.
    ///
    /// # Errors
    /// Rejects invalid bounds and database failures.
    pub fn open_permits(&self, limit: i64) -> Result<Vec<OpenPermit>> {
        if !(1..=1000).contains(&limit) {
            return Err(Error::Invalid(
                "permit limit must be between 1 and 1000".to_owned(),
            ));
        }
        let mut statement=self.0.conn.prepare("SELECT id,kind,holder,pid,host,proc_start,issued_at FROM deployment_permits WHERE released_at IS NULL ORDER BY id LIMIT ?1")?;
        Ok(statement
            .query_map([limit], |row| {
                Ok(OpenPermit {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    holder: row.get(2)?,
                    pid: row.get(3)?,
                    host: row.get(4)?,
                    proc_start: row.get(5)?,
                    issued_at: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Record an operator's evidence before ending a permit; a live holder is refused.
    ///
    /// # Errors
    /// Rejects open gates, live processes, invalid evidence and unknown permits.
    pub fn abandon_permit(&self, id: i64, operator: &Operator, evidence: &str) -> Result<()> {
        let (kind, reference) = authorized(operator)?;
        if !valid_operator_text(evidence) {
            return Err(Error::Invalid(
                "the evidence must be 1 to 200 printable characters".to_owned(),
            ));
        }
        if self.status()?.mode != "maintenance" {
            return Err(Error::Invalid(
                "permits can only be abandoned while the gate is closed".to_owned(),
            ));
        }
        let process:Option<(Option<String>,Option<i64>,Option<String>)>=self.0.conn.query_row("SELECT host,pid,proc_start FROM deployment_permits WHERE id=?1 AND released_at IS NULL",[id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        if let Some((host, pid, start)) = process
            && crate::procid::liveness(host.as_deref(), pid, start.as_deref())
                == crate::procid::Liveness::Alive
        {
            return Err(Error::Invalid(
                "the holder process is still running; stop it first".to_owned(),
            ));
        }
        let changed=self.0.conn.execute("UPDATE deployment_permits SET released_at=?1,abandoned_by=?2,abandon_evidence=?3 WHERE id=?4 AND released_at IS NULL AND EXISTS(SELECT 1 FROM deployment_gate WHERE id=1 AND mode='maintenance')",rusqlite::params![self.0.clock.now(),format!("{kind}:{reference}"),evidence,id])?;
        if changed != 1 {
            return Err(Error::Invalid("no such open permit".to_owned()));
        }
        Ok(())
    }

    /// Automatically abandon only holders proven gone in the same host/namespace/boot.
    ///
    /// # Errors
    /// Rejects open gates, unauthorized operators and database failures.
    pub fn reap_permits(&self, operator: &Operator) -> Result<Vec<i64>> {
        authorized(operator)?;
        if self.status()?.mode != "maintenance" {
            return Err(Error::Invalid(
                "permits can only be reaped while the gate is closed".to_owned(),
            ));
        }
        let mut reaped = Vec::new();
        for permit in self.open_permits(1000)? {
            if crate::procid::liveness(
                permit.host.as_deref(),
                permit.pid,
                permit.proc_start.as_deref(),
            ) == crate::procid::Liveness::Gone
            {
                self.abandon_permit(
                    permit.id,
                    operator,
                    "holder process is proven gone in its recorded host, namespace and boot",
                )?;
                reaped.push(permit.id);
            }
        }
        Ok(reaped)
    }

    /// Delete released permits older than `keep_days`; open permits are never removed.
    ///
    /// # Errors
    ///
    /// `keep_days` is not positive or SQLite rejects the deletion.
    pub fn prune_released_permits(&self, keep_days: i64) -> Result<usize> {
        if keep_days < 1 {
            return Err(Error::Invalid(
                "keep_days must be a positive number".to_owned(),
            ));
        }
        self.0
            .conn
            .execute(
                "DELETE FROM deployment_permits
             WHERE released_at IS NOT NULL AND released_at < datetime('now', ?1)",
                [format!("-{keep_days} days")],
            )
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::Operator;

    #[test]
    fn refusals_identify_the_actual_barrier_in_python_priority_order() {
        use super::DeploymentGate;
        use crate::{clock::FixedClock, db::Db, repo::Ctx, schema};
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        let gate = DeploymentGate(Ctx {
            conn: conn.raw(),
            clock: &clock,
        });
        let admin = Operator::from_admin_session(1).unwrap();
        let local = Operator {
            kind: "local-cli",
            reference: "test-owner".to_owned(),
        };
        assert_eq!(
            gate.open(&admin, 0).unwrap_err().to_string(),
            "the gate is not closed"
        );
        assert_eq!(
            gate.begin_offline_migration(&admin, 0)
                .unwrap_err()
                .to_string(),
            "the gate is not closed"
        );
        assert_eq!(
            gate.finish_offline_migration(&admin, 0)
                .unwrap_err()
                .to_string(),
            "the gate is not in an offline migration"
        );
        gate.close(&local, "更新 😀").unwrap();
        assert_eq!(
            gate.open(&admin, 0).unwrap_err().to_string(),
            "the gate changed since you looked; read it again"
        );
        assert_eq!(
            gate.open(&admin, 1).unwrap_err().to_string(),
            "the gate was closed from the host; open it from the host"
        );
        gate.begin_offline_migration(&local, 1).unwrap();
        assert_eq!(
            gate.open(&local, 2).unwrap_err().to_string(),
            "the offline migration is not finished"
        );
        conn.raw().execute("INSERT INTO deployment_permits(kind,holder,token,generation) VALUES('job','legacy','legacy',2)",[]).unwrap();
        assert_eq!(
            gate.finish_offline_migration(&local, 2)
                .unwrap_err()
                .to_string(),
            "work is still running"
        );
        conn.raw()
            .execute(
                "UPDATE deployment_permits SET released_at=CURRENT_TIMESTAMP",
                [],
            )
            .unwrap();
        gate.finish_offline_migration(&local, 2).unwrap();
        gate.open(&local, 3).unwrap();
        conn.raw()
            .execute("DELETE FROM deployment_gate", [])
            .unwrap();
        assert_eq!(
            gate.open(&local, 0).unwrap_err().to_string(),
            "the gate is not healthy; close it to repair it first"
        );
    }

    #[test]
    fn local_cli_operator_requires_an_existing_owned_database() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("manager.sqlite");
        assert!(Operator::from_local_cli(&database).is_err());
        std::fs::File::create(&database).unwrap();
        assert!(Operator::from_local_cli(&database).is_ok());
    }

    #[test]
    fn pruning_keeps_open_and_recent_permits() {
        let database = crate::db::Db::memory().unwrap();
        let mut conn = database.conn().unwrap();
        crate::schema::migrate(&mut conn).unwrap();
        conn.raw()
            .execute_batch(
                "INSERT INTO deployment_permits(kind, holder, token, generation, released_at)
                 VALUES ('job', 'old', 'one', 0, datetime('now', '-8 days'));
                 INSERT INTO deployment_permits(kind, holder, token, generation, released_at)
                 VALUES ('job', 'recent', 'two', 0, datetime('now', '-6 days'));
                 INSERT INTO deployment_permits(kind, holder, token, generation)
                 VALUES ('job', 'open', 'three', 0);",
            )
            .unwrap();
        let gate = super::DeploymentGate(crate::repo::Ctx {
            conn: conn.raw(),
            clock: &crate::clock::FixedClock(0),
        });
        assert_eq!(gate.prune_released_permits(7).unwrap(), 1);
        let remaining: i64 = conn
            .raw()
            .query_row("SELECT count(*) FROM deployment_permits", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining, 2);
        assert!(gate.prune_released_permits(0).is_err());
    }
}

#[cfg(test)]
mod permit_process_tests {
    use super::*;
    #[test]
    fn live_holders_cannot_be_abandoned_and_unknown_records_are_not_reaped() {
        let db = crate::db::Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        crate::schema::migrate(&mut conn).unwrap();
        let clock = crate::clock::SystemClock;
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let gate = DeploymentGate(ctx);
        let operator = Operator::from_admin_session(1).unwrap();
        let permit = gate.acquire_permit("job", "worker").unwrap().unwrap();
        assert!(!format!("{permit:?}").contains(permit.token()));
        assert!(gate.reap_permits(&operator).is_err());
        gate.close(&operator, "release").unwrap();
        assert!(
            gate.abandon_permit(permit.id, &operator, "operator checked")
                .is_err()
        );
        ctx.conn.execute("INSERT INTO deployment_permits(kind,holder,token,generation) VALUES('job','legacy','legacy',0)",[]).unwrap();
        let legacy = ctx.conn.last_insert_rowid();
        let identity = crate::procid::identity_of_self().unwrap();
        let (place, started) = identity.rsplit_once('|').unwrap();
        let previous = if started.starts_with("proc:") {
            format!("{place}|proc:0")
        } else {
            format!("{place}|ps:Mon Jan  1 00:00:00 2001")
        };
        ctx.conn.execute("INSERT INTO deployment_permits(kind,holder,token,generation,pid,host,proc_start) VALUES('job','old','old',0,?1,?2,?3)",rusqlite::params![i64::from(std::process::id()),crate::procid::host_name(),previous]).unwrap();
        let old = ctx.conn.last_insert_rowid();
        assert_eq!(gate.reap_permits(&operator).unwrap(), [old]);
        let open = gate.open_permits(100).unwrap();
        assert_eq!(open.len(), 2);
        gate.abandon_permit(legacy, &operator, "legacy holder checked separately")
            .unwrap();
        assert_eq!(gate.open_permits(100).unwrap().len(), 1);
        gate.release_permit(&permit).unwrap();
        assert!(gate.open_permits(100).unwrap().is_empty());
    }
}

#[cfg(test)]
mod gate_history_tests {
    use super::*;
    #[test]
    fn history_is_atomic_and_missing_rows_do_not_reset_generations() {
        let db = crate::db::Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        crate::schema::migrate(&mut conn).unwrap();
        let clock = crate::clock::FixedClock(1_000_000);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let gate = DeploymentGate(ctx);
        let operator = Operator::from_admin_session(1).unwrap();
        let closed = gate.close(&operator, "release").unwrap();
        assert_eq!(closed.generation, 1);
        gate.open(&operator, closed.generation).unwrap();
        let history = gate.history(0, 100).unwrap();
        assert_eq!(
            history
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            ["fresh-open", "close", "open"]
        );
        ctx.conn.execute("DELETE FROM deployment_gate", []).unwrap();
        let repaired = gate.close(&operator, "repair").unwrap();
        assert_eq!(repaired.generation, 3);
        assert_eq!(repaired.phase, "offline-migration");
        assert_eq!(repaired.mode, "maintenance");
        ctx.conn.execute_batch("CREATE TRIGGER reject_gate_audit BEFORE INSERT ON deployment_events BEGIN SELECT RAISE(ABORT,'history unavailable'); END;").unwrap();
        assert!(gate.finish_offline_migration(&operator, 3).is_err());
        let unchanged = gate.status().unwrap();
        assert_eq!(unchanged.generation, 3);
        assert_eq!(unchanged.phase, "offline-migration");
        ctx.conn
            .execute_batch("DROP TRIGGER reject_gate_audit;")
            .unwrap();
        let finished = gate.finish_offline_migration(&operator, 3).unwrap();
        assert_eq!(finished.generation, 4);
        assert_eq!(gate.history(3, 100).unwrap().len(), 2);
    }
}
