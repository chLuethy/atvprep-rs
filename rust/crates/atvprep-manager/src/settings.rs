//! Runtime settings: an allowlist of typed keys, one global revision, an audit trail.

use std::collections::BTreeMap;

use rusqlite::OptionalExtension;
use serde_json::Value;

use crate::repo::Ctx;
use crate::{Error, Result};

pub const MAX_FFMPEG_WORKERS: i64 = 32;
pub const MAX_PROBE_WORKERS: i64 = 32;
pub const MAX_RETENTION_DAYS: i64 = 365;
pub const MAX_EXCLUDED_DIRS: usize = 100;

/// A setting's type and, for numbers, its inclusive range.
#[derive(Clone, Copy)]
enum Rule {
    Int(i64, i64),
    Bool,
    List,
}

const RULES: [(&str, Rule); 9] = [
    ("scan_interval_seconds", Rule::Int(5, 86_400)),
    ("settle_seconds", Rule::Int(0, 86_400)),
    ("delete_original", Rule::Bool),
    ("max_ffmpeg_workers", Rule::Int(1, MAX_FFMPEG_WORKERS)),
    ("max_probe_workers", Rule::Int(1, MAX_PROBE_WORKERS)),
    ("event_retention_days", Rule::Int(1, MAX_RETENTION_DAYS)),
    ("log_retention_days", Rule::Int(1, MAX_RETENTION_DAYS)),
    ("job_retention_days", Rule::Int(1, MAX_RETENTION_DAYS)),
    ("scan_exclude_dirs", Rule::List),
];

/// The value a setting has until someone overrides it.
#[must_use]
pub fn default_of(key: &str) -> Option<Value> {
    Some(match key {
        "scan_interval_seconds" => 3600.into(),
        "settle_seconds" => 120.into(),
        "delete_original" => false.into(),
        "max_ffmpeg_workers" => 1.into(),
        "max_probe_workers" => 2.into(),
        "event_retention_days" | "log_retention_days" | "job_retention_days" => 7.into(),
        "scan_exclude_dirs" => Value::Array(Vec::new()),
        _ => return None,
    })
}

fn rule_of(key: &str) -> Option<Rule> {
    RULES.iter().find(|(k, _)| *k == key).map(|(_, r)| *r)
}

fn type_name(rule: Rule) -> &'static str {
    match rule {
        Rule::Int(..) => "int",
        Rule::Bool => "bool",
        Rule::List => "list",
    }
}

/// Python's `str.isspace` for one character.
fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn py_strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

fn invalid_dirs() -> Error {
    Error::Invalid("setting scan_exclude_dirs has an invalid value".to_owned())
}

/// Folders the scanner skips: a bare name matches a folder of that name anywhere in a library,
/// `a/b` matches that folder relative to the library root. Anything that could point outside a
/// library, or is not plain text, is rejected rather than quietly dropped.
///
/// # Errors
///
/// The value is not a list of acceptable names.
pub fn normalize_excluded_dirs(value: &Value) -> Result<Vec<String>> {
    let entries = value
        .as_array()
        .filter(|a| a.len() <= MAX_EXCLUDED_DIRS)
        .ok_or_else(invalid_dirs)?;
    let mut result: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.as_str().ok_or_else(invalid_dirs)?;
        let stripped = py_strip(entry);
        let text = stripped
            .strip_prefix("./")
            .unwrap_or(stripped)
            .trim_matches('/');
        if text.is_empty() {
            continue;
        }
        if text.chars().count() > 255
            || text.contains('\\')
            || text.contains('\0')
            || text.split('/').any(|part| matches!(part, "" | "." | ".."))
            || stripped.starts_with('/')
        {
            return Err(invalid_dirs());
        }
        if !result.iter().any(|seen| seen == text) {
            result.push(text.to_owned());
        }
    }
    Ok(result)
}

/// A stored setting.
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub key: String,
    pub value: Value,
    pub value_type: String,
    pub revision: i64,
}

/// One audit row.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditRow {
    pub key: String,
    pub revision: i64,
    pub actor: String,
    pub old_value: Option<Value>,
    pub new_value: Value,
}

pub struct Settings<'a>(pub Ctx<'a>);

impl Settings<'_> {
    /// # Errors
    ///
    /// SQLite fails.
    pub fn current_revision(&self) -> Result<i64> {
        let allocated: Option<i64> = self
            .0
            .conn
            .query_row(
                "SELECT revision FROM settings_revision WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(revision) = allocated {
            return Ok(revision);
        }
        let max: Option<i64> =
            self.0
                .conn
                .query_row("SELECT max(revision) FROM runtime_settings", [], |r| {
                    r.get(0)
                })?;
        Ok(max.unwrap_or(0))
    }

    /// Increment the singleton revision allocator inside the caller's transaction, so the claim,
    /// the values and the audit rows commit or roll back together. With an expected revision the
    /// UPDATE only matches when nobody else got there first.
    fn claim_revision(&self, expected: Option<i64>) -> Result<i64> {
        self.0.conn.execute(
            "INSERT INTO settings_revision (id, revision)
             SELECT 1, COALESCE(MAX(revision), 0) FROM runtime_settings
             WHERE NOT EXISTS (SELECT 1 FROM settings_revision WHERE id=1)
             ON CONFLICT(id) DO NOTHING",
            [],
        )?;
        let claimed: Option<i64> = match expected {
            Some(revision) => self.0.conn.query_row(
                "UPDATE settings_revision SET revision=revision+1 WHERE id=1 AND revision=?1
                 RETURNING revision",
                [revision],
                |r| r.get(0),
            ),
            None => self.0.conn.query_row(
                "UPDATE settings_revision SET revision=revision+1 WHERE id=1 RETURNING revision",
                [],
                |r| r.get(0),
            ),
        }
        .optional()?;
        claimed.ok_or_else(|| Error::Invalid("settings revision conflict".to_owned()))
    }

    /// # Errors
    ///
    /// SQLite fails.
    pub fn get(&self, key: &str) -> Result<Option<Setting>> {
        Ok(self
            .0
            .conn
            .query_row(
                "SELECT key, value_json, value_type, revision FROM runtime_settings WHERE key = ?1",
                [key],
                |row| {
                    let text: String = row.get(1)?;
                    Ok(Setting {
                        key: row.get(0)?,
                        value: serde_json::from_str(&text).unwrap_or(Value::Null),
                        value_type: row.get(2)?,
                        revision: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// The current overrides, for the keys the allowlist knows.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn values(&self) -> Result<BTreeMap<String, Value>> {
        let mut statement = self
            .0
            .conn
            .prepare("SELECT key, value_json FROM runtime_settings")?;
        let rows =
            statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = BTreeMap::new();
        for row in rows {
            let (key, text) = row?;
            if default_of(&key).is_some() {
                out.insert(key, serde_json::from_str(&text).unwrap_or(Value::Null));
            }
        }
        Ok(out)
    }

    fn validate_single(key: &str, value: &Value) -> Result<Value> {
        match rule_of(key).ok_or_else(|| Error::Invalid("setting is not allowlisted".to_owned()))? {
            Rule::List => Ok(Value::from(normalize_excluded_dirs(value)?)),
            Rule::Bool => {
                if value.is_boolean() {
                    Ok(value.clone())
                } else {
                    Err(Error::Invalid(
                        "setting must be bool integer-compatible".to_owned(),
                    ))
                }
            }
            Rule::Int(low, high) => {
                let number = value.as_i64().ok_or_else(|| {
                    Error::Invalid("setting must be int integer-compatible".to_owned())
                })?;
                if (low..=high).contains(&number) {
                    Ok(value.clone())
                } else {
                    Err(Error::Invalid(format!(
                        "setting must be between {low} and {high}"
                    )))
                }
            }
        }
    }

    fn validate_bulk(key: &str, value: &Value) -> Result<Value> {
        let invalid = || Error::Invalid(format!("setting {key} has an invalid value"));
        match rule_of(key).ok_or_else(|| Error::Invalid("setting is not allowlisted".to_owned()))? {
            Rule::List => Ok(Value::from(normalize_excluded_dirs(value)?)),
            Rule::Bool => value
                .is_boolean()
                .then(|| value.clone())
                .ok_or_else(invalid),
            Rule::Int(low, high) => value
                .as_i64()
                .filter(|n| (low..=high).contains(n))
                .map(|_| value.clone())
                .ok_or_else(invalid),
        }
    }

    fn write(&self, key: &str, value: &Value, revision: i64, actor: &str) -> Result<Setting> {
        let old = self.get(key)?;
        let now = self.0.clock.now();
        let text = value.to_string();
        let kind = rule_of(key).map_or("int", type_name);
        self.0.conn.execute(
            "INSERT INTO runtime_settings(key, value_json, value_type, revision, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,
                 revision=excluded.revision, updated_at=excluded.updated_at",
            rusqlite::params![key, text, kind, revision, now],
        )?;
        self.0.conn.execute(
            "INSERT INTO settings_audit(key, revision, actor, old_value_json, new_value_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                key,
                revision,
                actor,
                old.as_ref().map(|s| s.value.to_string()),
                text,
                now
            ],
        )?;
        self.get(key)?
            .ok_or_else(|| Error::NotFound(format!("setting {key}")))
    }

    /// Set one allowlisted key. With `expected_revision` the write fails when the global revision
    /// moved on.
    ///
    /// # Errors
    ///
    /// Not allowlisted, wrong type or range, or a revision conflict.
    pub fn set(
        &self,
        key: &str,
        value: &Value,
        actor: &str,
        expected_revision: Option<i64>,
    ) -> Result<Setting> {
        let value = Self::validate_single(key, value)?;
        let revision = self.claim_revision(expected_revision)?;
        self.write(key, &value, revision, actor)
    }

    /// Set several keys under one new revision. `values` keeps the caller's order.
    ///
    /// # Errors
    ///
    /// Empty update, a key that is not allowlisted, an invalid value, or a revision conflict.
    pub fn set_many(
        &self,
        values: &[(&str, Value)],
        actor: &str,
        expected_revision: Option<i64>,
    ) -> Result<Vec<Setting>> {
        if values.is_empty() {
            return Err(Error::Invalid(
                "settings update must not be empty".to_owned(),
            ));
        }
        let checked = values
            .iter()
            .map(|(key, value)| Ok((*key, Self::validate_bulk(key, value)?)))
            .collect::<Result<Vec<_>>>()?;
        let revision = self.claim_revision(expected_revision)?;
        checked
            .iter()
            .map(|(key, value)| self.write(key, value, revision, actor))
            .collect()
    }

    /// # Errors
    ///
    /// SQLite fails.
    pub fn audit(&self, key: &str) -> Result<Vec<AuditRow>> {
        let mut statement = self.0.conn.prepare(
            "SELECT key, revision, actor, old_value_json, new_value_json FROM settings_audit
             WHERE key = ?1 ORDER BY revision, id",
        )?;
        let rows = statement.query_map([key], |row| {
            let old: Option<String> = row.get(3)?;
            let new: String = row.get(4)?;
            Ok(AuditRow {
                key: row.get(0)?,
                revision: row.get(1)?,
                actor: row.get(2)?,
                old_value: old.map(|t| serde_json::from_str(&t).unwrap_or(Value::Null)),
                new_value: serde_json::from_str(&new).unwrap_or(Value::Null),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}
