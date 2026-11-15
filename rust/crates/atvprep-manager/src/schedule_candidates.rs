//! Which files a run of a schedule queues, in one place.
//!
//! The API shows this selection's count when a schedule is turned on (ADR 0006), and a run queues
//! exactly its ids. Both ask here, so the number an operator confirms is the number that is queued
//! as long as the files do not change in between.

use atvprep_core::encode::EncodeAllowance;

use crate::Result;
use crate::media_sql;
use crate::state_machine::JobAction;

/// What a schedule selects by: its action, library, compliance state and Kodierfreigabe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSelection {
    pub action: JobAction,
    pub root_id: Option<i64>,
    pub compliance_state: Option<String>,
    /// Only preparing reads it; an unknown stored name counts as the narrowest level.
    pub encode: EncodeAllowance,
}

impl ScheduleSelection {
    /// Load the selection of a stored schedule.
    ///
    /// # Errors
    ///
    /// SQLite rejects the read.
    pub fn load(conn: &rusqlite::Connection, id: i64) -> Result<Option<Self>> {
        Ok(rusqlite::OptionalExtension::optional(conn.query_row(
            "SELECT action, root_id, compliance_state, encode FROM schedules WHERE id=?1",
            [id],
            |row| {
                Ok(Self {
                    // A stored name no action has is treated as verifying, as it always was.
                    action: JobAction::from_public(&row.get::<_, String>(0)?)
                        .unwrap_or(JobAction::Verify),
                    root_id: row.get(1)?,
                    compliance_state: row.get(2)?,
                    encode: EncodeAllowance::parse(&row.get::<_, String>(3)?)
                        .unwrap_or(EncodeAllowance::Copy),
                })
            },
        ))?)
    }
}

/// The files one run would queue, at most `limit` of them, in the order they are queued.
///
/// A scan schedule selects no files.
///
/// # Errors
///
/// SQLite rejects the read.
pub fn candidate_ids(
    conn: &rusqlite::Connection,
    selection: &ScheduleSelection,
    limit: Option<i64>,
) -> Result<Vec<i64>> {
    if selection.action == JobAction::Scan {
        return Ok(Vec::new());
    }
    let embed_open = crate::embed::open_media_ids_json(conn)?;
    let mut statement = conn.prepare(&select_sql(selection))?;
    let ids = statement
        .query_map(
            rusqlite::params![
                selection.root_id,
                selection.compliance_state,
                selection.encode.as_str(),
                embed_open,
                limit.unwrap_or(-1),
            ],
            |row| row.get(0),
        )?
        .collect::<std::result::Result<Vec<i64>, _>>()?;
    Ok(ids)
}

/// How many files one run would queue (at most `limit`); always the size of [`candidate_ids`].
///
/// # Errors
///
/// SQLite rejects the read.
pub fn candidate_count(
    conn: &rusqlite::Connection,
    selection: &ScheduleSelection,
    limit: Option<i64>,
) -> Result<i64> {
    Ok(i64::try_from(candidate_ids(conn, selection, limit)?.len()).unwrap_or(i64::MAX))
}

/// `?1` library, `?2` compliance state, `?3` Kodierfreigabe, `?4` open embed ids, `?5` limit
/// (negative: none). Every parameter is bound, whether the action's condition uses it or not.
fn select_sql(selection: &ScheduleSelection) -> String {
    let (condition, order) = if selection.action.changes_files() {
        (media_sql::change_candidate(3, 4), "m.id")
    } else {
        (
            media_sql::MP4_LIKE.to_owned(),
            "(SELECT max(id) FROM job_items AS prior \
              WHERE prior.media_file_id=m.id AND prior.action='verify') ASC, m.id",
        )
    };
    format!(
        "SELECT m.id FROM media_files m
         WHERE m.discovery_state='present'
           AND (?1 IS NULL OR m.root_id=?1)
           AND (?2 IS NULL OR m.compliance_state=?2)
           AND {no_open_fix}
           AND {condition}
         ORDER BY {order} LIMIT ?5",
        no_open_fix = media_sql::no_open_prepare_item()
    )
}
