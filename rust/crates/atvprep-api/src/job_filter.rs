//! The filters of the job list and their facet counters, shared by `/jobs` and `/jobs/counts`.
//!
//! A filter is either a facet or a plain narrowing. A facet (status, action, library, Auslöser) is
//! declared in [`facets`] and handled by the facet engine ([`crate::facets`]): nothing chosen is every
//! job, chosen tokens show jobs with any of them (OR), and the counter of a token shows how many jobs
//! that token alone would show with every other filter as it is. A plain narrowing (the search,
//! `exclude_status`, the period) applies to the list and to every counter alike. The search matches
//! the display path of any item of a job.
//!
//! To add a facet, declare it in [`facets`] and its query field in [`JobQuery::raw`]. To add a plain
//! narrowing, add its field to [`JobQuery`] and its clause in [`JobFilter::parse`].
use crate::facets::{Base, Facet, FacetSet, Selection, Tokens, count_rows, in_list, library_facet};
use crate::filter_params::MAX_PARAM;
use crate::job_trigger;
use crate::opaque::Opaque;
use atvprep_manager::state_machine::{JobAction, JobStatus};
use axum::http::StatusCode;
use rusqlite::types::Value;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct JobQuery {
    #[serde(default, deserialize_with = "crate::opaque::bounded_cursor")]
    pub(crate) cursor: Option<String>,
    pub(crate) limit: Option<usize>,
    pub(crate) order: Option<String>,
    status: Option<String>,
    action: Option<String>,
    exclude_status: Option<String>,
    q: Option<String>,
    /// Queued at or after this instant (RFC 3339).
    from: Option<String>,
    /// Queued before this instant (RFC 3339).
    to: Option<String>,
    /// Opaque library ids: a job with a file in any of them.
    root_id: Option<String>,
    trigger: Option<String>,
    /// Opaque ids of single schedules, alternatives to `trigger`.
    schedule_id: Option<String>,
}

impl JobQuery {
    /// The raw list a facet reads from the query, by its address key.
    fn raw(&self, key: &str) -> Option<&str> {
        match key {
            "status" => self.status.as_deref(),
            "action" => self.action.as_deref(),
            "root_id" => self.root_id.as_deref(),
            "trigger" => self.trigger.as_deref(),
            "schedule_id" => self.schedule_id.as_deref(),
            _ => None,
        }
    }
}

fn status_tokens() -> Vec<&'static str> {
    JobStatus::ALL
        .iter()
        .map(|status| status.as_str())
        .collect()
}

/// Actions as the API names them. `prepare` also covers the history stored under `fix` and
/// `convert` (ADR 0008).
fn action_tokens() -> Vec<&'static str> {
    vec!["verify", "prepare", "inspect"]
}

/// The facets of the job list in the order their clauses are bound.
fn facets() -> FacetSet<'static> {
    FacetSet::new(vec![
        Facet::new(
            "status",
            Tokens::Fixed(status_tokens),
            Box::new(|chosen, values| in_list("j.status", chosen, values)),
        ),
        Facet::new(
            "action",
            Tokens::Fixed(action_tokens),
            Box::new(|chosen, values| {
                let stored: Vec<Value> = chosen
                    .iter()
                    .flat_map(|token| match token {
                        Value::Text(token) => match JobAction::from_public(token) {
                            Some(action) => action.stored_forms().to_vec(),
                            None => vec![token.as_str()],
                        },
                        _ => Vec::new(),
                    })
                    .map(|token| Value::from(token.to_owned()))
                    .collect();
                in_list("j.action", &stored, values)
            }),
        ),
        library_facet("invalid request", |in_root| {
            format!(
                "EXISTS(SELECT 1 FROM job_items i JOIN media_files m ON m.id=i.media_file_id WHERE i.job_id=j.id AND {in_root})"
            )
        }),
        job_trigger::facet(),
    ])
}

/// The jobs the list shows at all: cleared ones are gone from Activity.
const LISTED: &str = "j.cleared_at IS NULL";

/// A job item's display path matched by the search pattern bound to `?`.
macro_rules! item_matches {
    () => {
        "FROM job_items i JOIN media_files m ON m.id=i.media_file_id WHERE i.job_id=j.id AND lower(m.relative_path) LIKE lower(?) ESCAPE '\\'"
    };
}

/// SQL true for a job with any item whose display path matches the search.
const SEARCHED: &str = concat!("EXISTS(SELECT 1 ", item_matches!(), ")");

/// SQL for the display path of a job's first item, in job order, that matches the search.
const MATCHED: &str = concat!(
    "(SELECT m.relative_path ",
    item_matches!(),
    " ORDER BY i.position,i.id LIMIT 1)"
);

/// The table the job list reads.
const FROM: &str = "jobs j";

/// A validated job filter: the facet choices and the plain narrowings as SQL.
pub(crate) struct JobFilter {
    facets: FacetSet<'static>,
    selection: Selection,
    /// The plain narrowings with their values; every counter shares them.
    narrowing: Base,
    /// The search's LIKE pattern, when there is a search.
    search: Option<Value>,
}

impl JobFilter {
    /// Check every filter parameter and build the filter; the error is the response status and the
    /// problem's detail. An unknown library is not found, as on the file list; anything else
    /// invalid is refused with 422.
    pub(crate) fn parse(
        query: &JobQuery,
        opaque: &Opaque,
    ) -> Result<Self, (StatusCode, &'static str)> {
        let invalid = |message| (StatusCode::UNPROCESSABLE_ENTITY, message);
        let facets = facets();
        let selection = facets
            .parse(&|key| query.raw(key), opaque)
            .map_err(|error| error.response())?;
        let too_long = [&query.exclude_status, &query.q, &query.from, &query.to]
            .iter()
            .any(|value| {
                value
                    .as_ref()
                    .is_some_and(|v| v.chars().count() > MAX_PARAM)
            });
        if too_long {
            return Err(invalid("invalid request"));
        }
        let mut clauses = vec![LISTED.to_owned()];
        let mut values = Vec::new();
        if let Some(raw) = query.exclude_status.as_deref().filter(|v| !v.is_empty()) {
            let status: JobStatus = raw.parse().map_err(|_| invalid("invalid request"))?;
            clauses.push("j.status!=?".to_owned());
            values.push(Value::from(status.as_str().to_owned()));
        }
        let search = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|q| Value::from(format!("%{}%", like_literal(q))));
        if let Some(pattern) = &search {
            clauses.push(SEARCHED.to_owned());
            values.push(pattern.clone());
        }
        period_clauses(query, &mut clauses, &mut values).map_err(invalid)?;
        Ok(Self {
            facets,
            selection,
            narrowing: (clauses, values),
            search,
        })
    }

    /// With a search, a column expression for the display path of the first matching item of `j`
    /// and its value.
    pub(crate) fn matched(&self) -> Option<(&'static str, Value)> {
        self.search.clone().map(|pattern| (MATCHED, pattern))
    }

    /// The WHERE clause of the list and its values.
    pub(crate) fn sql(&self) -> (String, Vec<Value>) {
        let mut clauses = self.narrowing.0.clone();
        let mut values = self.narrowing.1.clone();
        clauses.extend(self.facets.clauses(&self.selection, &mut values));
        (clauses.join(" AND "), values)
    }

    /// The facet counters, "matched of total" and how many waiting or running jobs the filter
    /// leaves out, as the `data` of `/jobs/counts`.
    pub(crate) fn counts(
        &self,
        conn: &rusqlite::Connection,
        opaque: &Opaque,
    ) -> rusqlite::Result<serde_json::Value> {
        let mut data = self
            .facets
            .counts(conn, opaque, FROM, &self.narrowing, &self.selection)?;
        let mut schedules = Vec::new();
        for (id, name) in job_trigger::job_schedules(conn)? {
            schedules.push(serde_json::json!({
                "id": opaque.encode("sc", id),
                "name": crate::read_jobs::safe_head(&name, 2048),
            }));
        }
        data.insert("schedules".to_owned(), schedules.into());
        let (sql, values) = self.sql();
        let active = format!("j.status IN {}", JobStatus::non_terminal_sql_list());
        let matched_active = count_rows(conn, FROM, &format!("{sql} AND {active}"), &values)?;
        let total_active = count_rows(conn, FROM, &format!("{LISTED} AND {active}"), &[])?;
        data.insert(
            "matched".to_owned(),
            count_rows(conn, FROM, &sql, &values)?.into(),
        );
        data.insert(
            "total".to_owned(),
            count_rows(conn, FROM, LISTED, &[])?.into(),
        );
        data.insert(
            "hidden_active".to_owned(),
            (total_active - matched_active).into(),
        );
        Ok(data.into())
    }
}

/// The period on when a job was queued: `from` inclusive, `to` exclusive, each an RFC 3339 instant
/// with any offset. Stored times come in several spellings, so both sides compare as instants.
fn period_clauses(
    query: &JobQuery,
    clauses: &mut Vec<String>,
    values: &mut Vec<Value>,
) -> Result<(), &'static str> {
    for (op, value) in [(">=", &query.from), ("<", &query.to)] {
        let Some(value) = value.as_deref().filter(|v| !v.is_empty()) else {
            continue;
        };
        let instant = chrono::DateTime::parse_from_rfc3339(value).map_err(|_| "invalid request")?;
        clauses.push(format!("julianday(j.requested_at){op}julianday(?)"));
        values.push(Value::from(
            instant
                .naive_utc()
                .format("%Y-%m-%d %H:%M:%S%.6f")
                .to_string(),
        ));
    }
    Ok(())
}

/// `value` with `%`, `_` and the escape character matched literally by `LIKE … ESCAPE '\'`.
fn like_literal(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::like_literal;

    #[test]
    fn like_literal_escapes_patterns() {
        assert_eq!(like_literal(r"a_b%c\d"), r"a\_b\%c\\d");
    }
}
