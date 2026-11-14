//! The filters of the job list and their facet counters, shared by `/jobs` and `/jobs/counts`.
//!
//! A filter is either a facet or a plain narrowing. A facet (status, action) is a comma-separated
//! list of known tokens: nothing chosen is every job, chosen tokens show jobs with any of them (OR).
//! Its counter shows, per token, how many jobs that token alone would show with every other filter
//! as it is. A plain narrowing (the search, `exclude_status`) applies to the list and to every
//! counter alike.
//!
//! To add a facet, add a [`FacetDef`] to [`FACETS`] and its query field to [`JobQuery::facet`]. To add
//! a plain narrowing (a period, a library), add its field to [`JobQuery`] and its clause in
//! [`JobFilter::parse`].
use atvprep_manager::state_machine::JobStatus;
use rusqlite::types::Value;
use serde::Deserialize;

/// Longest accepted value of any filter parameter, in characters.
const MAX_PARAM: usize = 128;

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
}

impl JobQuery {
    /// The raw list a facet reads from the query.
    fn facet(&self, key: &str) -> Option<&str> {
        match key {
            "status" => self.status.as_deref(),
            "action" => self.action.as_deref(),
            _ => None,
        }
    }
}

/// One facet of the job list: its query key, its tokens in display order and the SQL that shows a
/// job with any of the chosen tokens.
struct FacetDef {
    key: &'static str,
    tokens: fn() -> Vec<&'static str>,
    /// A clause true for a job with any of `chosen` (validated, non-empty), binding its values.
    clause: fn(chosen: &[&str], values: &mut Vec<Value>) -> String,
}

fn status_tokens() -> Vec<&'static str> {
    JobStatus::ALL
        .iter()
        .map(|status| status.as_str())
        .collect()
}

fn status_clause(chosen: &[&str], values: &mut Vec<Value>) -> String {
    values.extend(chosen.iter().map(|token| Value::from((*token).to_owned())));
    format!("j.status IN ({})", placeholders(chosen.len()))
}

/// Actions as the API names them. `prepare` also covers the history stored under `fix` and
/// `convert` (ADR 0008).
fn action_tokens() -> Vec<&'static str> {
    vec!["verify", "prepare", "inspect"]
}

fn action_clause(chosen: &[&str], values: &mut Vec<Value>) -> String {
    let stored: Vec<&str> = chosen
        .iter()
        .flat_map(|token| match *token {
            "prepare" => vec!["prepare", "fix", "convert"],
            other => vec![other],
        })
        .collect();
    values.extend(stored.iter().map(|token| Value::from((*token).to_owned())));
    format!("j.action IN ({})", placeholders(stored.len()))
}

const FACETS: [FacetDef; 2] = [
    FacetDef {
        key: "status",
        tokens: status_tokens,
        clause: status_clause,
    },
    FacetDef {
        key: "action",
        tokens: action_tokens,
        clause: action_clause,
    },
];

fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

/// The non-empty entries of a comma-separated list.
fn tokens(value: Option<&str>) -> Vec<&str> {
    value
        .unwrap_or_default()
        .split(',')
        .filter(|token| !token.is_empty())
        .collect()
}

/// The jobs the list shows at all: cleared ones are gone from Activity.
const LISTED: &str = "j.cleared_at IS NULL";

/// SQL that a job's search text must match: for now its subject, the first file (#72 widens it).
const SEARCHED: &str = "lower(COALESCE((SELECT m.relative_path FROM job_items i JOIN media_files m ON m.id=i.media_file_id WHERE i.job_id=j.id ORDER BY i.position,i.id LIMIT 1),'')) LIKE lower(?) ESCAPE '\\'";

/// A validated job filter: the chosen tokens per facet and the plain narrowings as SQL.
pub(crate) struct JobFilter {
    /// Per entry of [`FACETS`], the chosen tokens in display order; empty is every job.
    chosen: Vec<Vec<&'static str>>,
    narrowing: Vec<String>,
    narrowing_values: Vec<Value>,
}

impl JobFilter {
    /// Check every filter parameter and build the filter; the error is the problem's detail.
    pub(crate) fn parse(query: &JobQuery) -> Result<Self, &'static str> {
        let too_long = [
            &query.status,
            &query.action,
            &query.exclude_status,
            &query.q,
        ]
        .iter()
        .any(|value| {
            value
                .as_ref()
                .is_some_and(|v| v.chars().count() > MAX_PARAM)
        });
        if too_long {
            return Err("invalid request");
        }
        let mut chosen = Vec::with_capacity(FACETS.len());
        for facet in &FACETS {
            let known = (facet.tokens)();
            let asked = tokens(query.facet(facet.key));
            if asked.iter().any(|token| !known.contains(token)) {
                return Err("invalid request");
            }
            chosen.push(
                known
                    .into_iter()
                    .filter(|token| asked.contains(token))
                    .collect(),
            );
        }
        let mut narrowing = vec![LISTED.to_owned()];
        let mut narrowing_values = Vec::new();
        if let Some(raw) = query.exclude_status.as_deref().filter(|v| !v.is_empty()) {
            let status: JobStatus = raw.parse().map_err(|_| "invalid request")?;
            narrowing.push("j.status!=?".to_owned());
            narrowing_values.push(Value::from(status.as_str().to_owned()));
        }
        if let Some(q) = query.q.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            narrowing.push(SEARCHED.to_owned());
            narrowing_values.push(Value::from(format!("%{}%", like_literal(q))));
        }
        Ok(Self {
            chosen,
            narrowing,
            narrowing_values,
        })
    }

    /// The clauses (joined with AND) and their values for every filter, with facet `index` set to
    /// `instead` when given (`Some(&[])` drops that facet).
    fn clauses(&self, index: Option<usize>, instead: &[&str]) -> (String, Vec<Value>) {
        let mut clauses = self.narrowing.clone();
        let mut values = self.narrowing_values.clone();
        for (position, facet) in FACETS.iter().enumerate() {
            let chosen = if Some(position) == index {
                instead
            } else {
                &self.chosen[position]
            };
            if !chosen.is_empty() {
                clauses.push((facet.clause)(chosen, &mut values));
            }
        }
        (clauses.join(" AND "), values)
    }

    /// The WHERE clause of the list and its values.
    pub(crate) fn sql(&self) -> (String, Vec<Value>) {
        self.clauses(None, &[])
    }

    /// The facet counters, "matched of total" and how many waiting or running jobs the filter
    /// leaves out, as the `data` of `/jobs/counts`.
    pub(crate) fn counts(
        &self,
        conn: &rusqlite::Connection,
    ) -> rusqlite::Result<serde_json::Value> {
        let count = |sql: &str, values: &[Value]| -> rusqlite::Result<i64> {
            conn.query_row(
                &format!("SELECT count(*) FROM jobs j WHERE {sql}"),
                rusqlite::params_from_iter(values.iter()),
                |row| row.get(0),
            )
        };
        let mut data = serde_json::Map::new();
        for (index, facet) in FACETS.iter().enumerate() {
            let mut by_token = serde_json::Map::new();
            for token in (facet.tokens)() {
                let (sql, values) = self.clauses(Some(index), &[token]);
                by_token.insert(token.to_owned(), count(&sql, &values)?.into());
            }
            data.insert(facet.key.to_owned(), by_token.into());
        }
        let (sql, values) = self.sql();
        let active = format!("j.status IN {}", JobStatus::non_terminal_sql_list());
        let matched_active = count(&format!("{sql} AND {active}"), &values)?;
        let total_active = count(&format!("{LISTED} AND {active}"), &[])?;
        data.insert("matched".to_owned(), count(&sql, &values)?.into());
        data.insert("total".to_owned(), count(LISTED, &[])?.into());
        data.insert(
            "hidden_active".to_owned(),
            (total_active - matched_active).into(),
        );
        Ok(data.into())
    }
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
