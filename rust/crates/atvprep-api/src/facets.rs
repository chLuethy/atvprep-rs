//! The facet engine of the file list and the job list.
//!
//! A facet is a filter on one address key (`status`, `root_id`, …) whose value is a comma-separated
//! list of tokens. A list declares its facets ([`Facet`]: the key, where its tokens come from, the
//! SQL clause of a choice) and gets from the engine ([`FacetSet`]):
//!
//! - the parsing of the address with its input errors ([`FacetError`]),
//! - the clauses of the chosen tokens,
//! - the counters: per option, how many rows that option alone would show with every other facet as
//!   chosen, "the filter without the facet's own choice".
//!
//! To add a facet, declare it in the list's facet set; counters and input checks follow.
use crate::filter_params::{MAX_PARAM, placeholders, tokens};
use crate::opaque::Opaque;
use axum::http::StatusCode;
use rusqlite::{Connection, types::Value};
use serde_json::{Map, json};

/// SQL clauses joined with AND, and the values bound to them in order.
pub(crate) type Base = (Vec<String>, Vec<Value>);

/// The clause for chosen tokens (validated, non-empty), binding its values.
pub(crate) type Clause<'a> = Box<dyn Fn(&[Value], &mut Vec<Value>) -> String + Send + Sync + 'a>;

/// Where the tokens of a facet come from.
#[derive(Clone, Copy)]
pub(crate) enum Tokens {
    /// A fixed list in display order; any other token is refused.
    Fixed(fn() -> Vec<&'static str>),
    /// Any text. The counters offer the values `options_sql` lists (one text column) and the chosen.
    Open { options_sql: &'static str },
    /// Opaque ids of a kind (`prefix`), a token that does not decode is not found; `options_sql`
    /// lists the ids (one integer column) the counters offer.
    Ids {
        prefix: &'static str,
        options_sql: &'static str,
    },
}

/// How several chosen tokens of a facet combine.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Combine {
    /// Rows with any of them; a counter shows the option alone.
    Any,
    /// Rows with all of them; a counter shows the choice plus the option.
    All,
}

/// A second address key of a facet whose tokens are opaque ids, alternatives to the first key's
/// tokens (a job's single schedules beside its Auslöser).
#[derive(Clone, Copy)]
pub(crate) struct Alongside {
    pub(crate) key: &'static str,
    pub(crate) prefix: &'static str,
    pub(crate) options_sql: &'static str,
    pub(crate) max_len: usize,
    pub(crate) max_count: usize,
}

/// One facet of a list.
pub(crate) struct Facet<'a> {
    key: &'static str,
    tokens: Tokens,
    combine: Combine,
    max_len: usize,
    too_long: &'static str,
    unknown: &'static str,
    /// An SQL expression whose value is the option of a row: all counters then come from one
    /// `GROUP BY` instead of one query per option.
    group_key: Option<&'static str>,
    alongside: Option<Alongside>,
    /// Its ids are resolved (and a missing one reported) before any other value is checked.
    resolved_first: bool,
    clause: Clause<'a>,
}

impl<'a> Facet<'a> {
    pub(crate) fn new(key: &'static str, tokens: Tokens, clause: Clause<'a>) -> Self {
        Self {
            key,
            tokens,
            combine: Combine::Any,
            max_len: MAX_PARAM,
            too_long: "invalid request",
            unknown: "invalid request",
            group_key: None,
            alongside: None,
            resolved_first: false,
            clause,
        }
    }

    /// The messages of an input that is too long and of a token the facet does not know.
    pub(crate) fn messages(mut self, too_long: &'static str, unknown: &'static str) -> Self {
        self.too_long = too_long;
        self.unknown = unknown;
        self
    }

    /// The longest accepted value of the key, in characters.
    pub(crate) fn max_len(mut self, max_len: usize) -> Self {
        self.max_len = max_len;
        self
    }

    /// Rows with all chosen tokens rather than any.
    pub(crate) fn all(mut self) -> Self {
        self.combine = Combine::All;
        self
    }

    /// Count the options with one `GROUP BY` on this SQL expression.
    pub(crate) fn grouped_by(mut self, group_key: &'static str) -> Self {
        self.group_key = Some(group_key);
        self
    }

    /// Resolve this facet's ids, and report one that names nothing (404), before any other value is
    /// checked. Without it every invalid value (422) is reported before an id that names nothing.
    pub(crate) fn resolved_first(mut self) -> Self {
        self.resolved_first = true;
        self
    }

    /// A second key whose ids are alternatives to the tokens.
    pub(crate) fn alongside(mut self, alongside: Alongside) -> Self {
        self.alongside = Some(alongside);
        self
    }
}

/// The library facet of both lists: its tokens are the opaque ids of the configured libraries, an
/// id that does not decode is not found. `wrap` turns the `column IN (…)` clause the engine builds
/// for the chosen libraries into the list's clause.
pub(crate) fn library_facet(too_long: &'static str, wrap: fn(String) -> String) -> Facet<'static> {
    Facet::new(
        "root_id",
        Tokens::Ids {
            prefix: "r",
            options_sql: "SELECT id FROM library_roots ORDER BY id",
        },
        Box::new(move |chosen, values| wrap(in_list("m.root_id", chosen, values))),
    )
    .max_len(crate::filter_params::MAX_ID_LIST)
    .messages(too_long, "invalid request")
}

/// `column IN (…)` for `chosen`, binding them.
pub(crate) fn in_list(column: &str, chosen: &[Value], values: &mut Vec<Value>) -> String {
    values.extend(chosen.iter().cloned());
    format!("{column} IN ({})", placeholders(chosen.len()))
}

/// Why the address of a list was refused.
#[derive(Debug)]
pub(crate) enum FacetError {
    /// Invalid input (422) with its message.
    Invalid(&'static str),
    /// An id that names nothing (404).
    NotFound,
}

impl FacetError {
    /// The response status and the problem's detail.
    pub(crate) fn response(&self) -> (StatusCode, &'static str) {
        match self {
            Self::Invalid(message) => (StatusCode::UNPROCESSABLE_ENTITY, message),
            Self::NotFound => (StatusCode::NOT_FOUND, "resource not found"),
        }
    }
}

/// What was chosen, per facet of the set in its order; empty is every row.
#[derive(Clone, Default)]
pub(crate) struct Selection {
    chosen: Vec<Vec<Value>>,
}

impl Selection {
    /// Anything is chosen at all.
    pub(crate) fn any(&self) -> bool {
        self.chosen.iter().any(|chosen| !chosen.is_empty())
    }
}

/// The facets of a list in the order their clauses are bound.
pub(crate) struct FacetSet<'a> {
    facets: Vec<Facet<'a>>,
}

impl<'a> FacetSet<'a> {
    pub(crate) fn new(facets: Vec<Facet<'a>>) -> Self {
        Self { facets }
    }

    /// Check the address (`raw` gives the value of a key) and read the choices. Every length and
    /// token is checked before an id is decoded, so that invalid input is reported before not found;
    /// a facet that is [`Facet::resolved_first`] goes ahead of that, after its own length check.
    pub(crate) fn parse<'v>(
        &self,
        raw: &dyn Fn(&str) -> Option<&'v str>,
        opaque: &Opaque,
    ) -> Result<Selection, FacetError> {
        let mut early: Vec<Option<Vec<Value>>> = vec![None; self.facets.len()];
        for (index, facet) in self.facets.iter().enumerate() {
            if facet.resolved_first {
                if raw(facet.key).is_some_and(|v| v.chars().count() > facet.max_len) {
                    return Err(FacetError::Invalid(facet.too_long));
                }
                early[index] = Some(Self::read(facet, raw, opaque)?);
            }
        }
        for facet in &self.facets {
            let value = raw(facet.key);
            if value.is_some_and(|v| v.chars().count() > facet.max_len) {
                return Err(FacetError::Invalid(facet.too_long));
            }
            if let Tokens::Fixed(known) = facet.tokens {
                let known = known();
                if tokens(value).iter().any(|token| !known.contains(token)) {
                    return Err(FacetError::Invalid(facet.unknown));
                }
            }
            if let Some(along) = facet.alongside
                && raw(along.key).is_some_and(|v| v.chars().count() > along.max_len)
            {
                return Err(FacetError::Invalid(facet.too_long));
            }
        }
        let mut chosen = Vec::with_capacity(self.facets.len());
        for (facet, resolved) in self.facets.iter().zip(early) {
            chosen.push(match resolved {
                Some(resolved) => resolved,
                None => Self::read(facet, raw, opaque)?,
            });
        }
        Ok(Selection { chosen })
    }

    fn read<'v>(
        facet: &Facet<'a>,
        raw: &dyn Fn(&str) -> Option<&'v str>,
        opaque: &Opaque,
    ) -> Result<Vec<Value>, FacetError> {
        let asked = tokens(raw(facet.key));
        let mut chosen: Vec<Value> = match facet.tokens {
            Tokens::Fixed(known) => known()
                .into_iter()
                .filter(|token| asked.contains(token))
                .map(|token| Value::from(token.to_owned()))
                .collect(),
            Tokens::Open { .. } => {
                let mut seen: Vec<Value> = Vec::new();
                for token in asked {
                    let value = Value::from(token.to_owned());
                    if !seen.contains(&value) {
                        seen.push(value);
                    }
                }
                seen
            }
            Tokens::Ids { prefix, .. } => {
                let mut ids = Vec::new();
                for token in asked {
                    match opaque.decode(token, prefix) {
                        Ok(id) if id > 0 => {
                            let id = Value::from(id);
                            if !ids.contains(&id) {
                                ids.push(id);
                            }
                        }
                        _ => return Err(FacetError::NotFound),
                    }
                }
                ids
            }
        };
        if let Some(along) = facet.alongside {
            let mut ids = Vec::new();
            for token in tokens(raw(along.key)) {
                let id = opaque
                    .decode(token, along.prefix)
                    .ok()
                    .filter(|id| *id > 0)
                    .ok_or(FacetError::Invalid(facet.unknown))?;
                let id = Value::from(id);
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            if ids.len() > along.max_count {
                return Err(FacetError::Invalid(facet.unknown));
            }
            chosen.extend(ids);
        }
        Ok(chosen)
    }

    /// The clauses of the choices, with their values bound in order.
    pub(crate) fn clauses(&self, selection: &Selection, values: &mut Vec<Value>) -> Vec<String> {
        self.clauses_instead(selection, None, values)
    }

    /// As [`Self::clauses`], with the choice of facet `instead.0` set to `instead.1`.
    fn clauses_instead(
        &self,
        selection: &Selection,
        instead: Option<(usize, &[Value])>,
        values: &mut Vec<Value>,
    ) -> Vec<String> {
        let mut clauses = Vec::new();
        for (index, facet) in self.facets.iter().enumerate() {
            let chosen = match instead {
                Some((at, chosen)) if at == index => chosen,
                _ => &selection.chosen[index],
            };
            if chosen.is_empty() {
                continue;
            }
            match facet.combine {
                Combine::Any => clauses.push((facet.clause)(chosen, values)),
                Combine::All => {
                    for token in chosen {
                        clauses.push((facet.clause)(std::slice::from_ref(token), values));
                    }
                }
            }
        }
        clauses
    }

    /// The WHERE clause of `base` and the choices (`instead` as in [`Self::clauses_instead`]).
    fn where_sql(
        &self,
        base: &Base,
        selection: &Selection,
        instead: Option<(usize, &[Value])>,
    ) -> (String, Vec<Value>) {
        let mut clauses = base.0.clone();
        let mut values = base.1.clone();
        clauses.extend(self.clauses_instead(selection, instead, &mut values));
        let sql = if clauses.is_empty() {
            "1".to_owned()
        } else {
            clauses.join(" AND ")
        };
        (sql, values)
    }

    /// The rows of `from` (a table with its alias) that match `base` and every choice.
    pub(crate) fn matched(
        &self,
        conn: &Connection,
        from: &str,
        base: &Base,
        selection: &Selection,
    ) -> rusqlite::Result<i64> {
        let (sql, values) = self.where_sql(base, selection, None);
        count_rows(conn, from, &sql, &values)
    }

    /// The counters of every facet, by key: per option, the rows it would show with every other
    /// facet as chosen. A facet with a second key adds that key's counters under its own key.
    pub(crate) fn counts(
        &self,
        conn: &Connection,
        opaque: &Opaque,
        from: &str,
        base: &Base,
        selection: &Selection,
    ) -> rusqlite::Result<Map<String, serde_json::Value>> {
        let mut data = Map::new();
        for (index, facet) in self.facets.iter().enumerate() {
            let grouped = match facet.group_key {
                Some(key) => Some(self.grouped(conn, from, base, selection, index, key)?),
                None => None,
            };
            let count_of = |option: &Value| -> rusqlite::Result<i64> {
                if let Some(grouped) = &grouped {
                    return Ok(grouped.get(&sql_text(option)).copied().unwrap_or(0));
                }
                let mut with = match facet.combine {
                    Combine::Any => Vec::new(),
                    Combine::All => selection.chosen[index].clone(),
                };
                if !with.contains(option) {
                    with.push(option.clone());
                }
                let (sql, values) = self.where_sql(base, selection, Some((index, &with)));
                count_rows(conn, from, &sql, &values)
            };
            let mut by_option = Map::new();
            for (name, option) in facet.options(conn, opaque, &selection.chosen[index])? {
                by_option.insert(name, json!(count_of(&option)?));
            }
            data.insert(facet.key.to_owned(), by_option.into());
            if let Some(along) = facet.alongside {
                let mut by_id = Map::new();
                for id in ids(conn, along.options_sql)? {
                    let count = count_of(&Value::from(id))?;
                    by_id.insert(opaque.encode(along.prefix, id), json!(count));
                }
                data.insert(along.key.to_owned(), by_id.into());
            }
        }
        Ok(data)
    }

    /// The rows matching `base` and every choice but facet `index`, counted per value of `key`.
    fn grouped(
        &self,
        conn: &Connection,
        from: &str,
        base: &Base,
        selection: &Selection,
        index: usize,
        key: &str,
    ) -> rusqlite::Result<std::collections::HashMap<String, i64>> {
        let (sql, values) = self.where_sql(base, selection, Some((index, &[])));
        let mut statement = conn.prepare(&format!(
            "SELECT {key}, count(*) FROM {from} WHERE {sql} GROUP BY 1"
        ))?;
        statement
            .query_map(rusqlite::params_from_iter(values.iter()), |row| {
                Ok((sql_text(&row.get::<_, Value>(0)?), row.get::<_, i64>(1)?))
            })?
            .collect()
    }
}

impl Facet<'_> {
    /// The options the counters of the facet list as (name in the response, value): its fixed
    /// tokens, or what the database lists, and the chosen ones that it does not.
    fn options(
        &self,
        conn: &Connection,
        opaque: &Opaque,
        chosen: &[Value],
    ) -> rusqlite::Result<Vec<(String, Value)>> {
        Ok(match self.tokens {
            Tokens::Fixed(known) => known()
                .into_iter()
                .map(|token| (token.to_owned(), Value::from(token.to_owned())))
                .collect(),
            Tokens::Open { options_sql } => {
                let mut options: Vec<String> = conn
                    .prepare(options_sql)?
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<_, _>>()?;
                options.extend(chosen.iter().map(sql_text));
                options
                    .into_iter()
                    .map(|text| (crate::read_jobs::safe_head(&text, 256), Value::from(text)))
                    .collect()
            }
            Tokens::Ids {
                prefix,
                options_sql,
                ..
            } => ids(conn, options_sql)?
                .into_iter()
                .map(|id| (opaque.encode(prefix, id), Value::from(id)))
                .collect(),
        })
    }
}

fn ids(conn: &Connection, sql: &str) -> rusqlite::Result<Vec<i64>> {
    conn.prepare(sql)?
        .query_map([], |row| row.get(0))?
        .collect()
}

/// The rows of `from` matching the WHERE clause `sql`.
pub(crate) fn count_rows(
    conn: &Connection,
    from: &str,
    sql: &str,
    values: &[Value],
) -> rusqlite::Result<i64> {
    conn.query_row(
        &format!("SELECT count(*) FROM {from} WHERE {sql}"),
        rusqlite::params_from_iter(values.iter()),
        |row| row.get(0),
    )
}

/// A value read back from SQL, as the text the counters key it by.
fn sql_text(value: &Value) -> String {
    match value {
        Value::Integer(n) => n.to_string(),
        Value::Text(text) => text.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opaque::Opaque;
    use rusqlite::Connection;

    fn colors() -> Vec<&'static str> {
        vec!["red", "green", "blue"]
    }

    fn sizes() -> Vec<&'static str> {
        vec!["big", "small"]
    }

    /// Two facets over `things`: the color and the size, each with any of its chosen options.
    fn definitions() -> FacetSet<'static> {
        FacetSet::new(vec![
            Facet::new(
                "color",
                Tokens::Fixed(colors),
                Box::new(|chosen, values| in_list("t.color", chosen, values)),
            )
            .messages("color is too long", "unknown color"),
            Facet::new(
                "size",
                Tokens::Fixed(sizes),
                Box::new(|chosen, values| in_list("t.size", chosen, values)),
            )
            .messages("size is too long", "unknown size"),
        ])
    }

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE things(id INTEGER PRIMARY KEY, color TEXT, size TEXT);
             INSERT INTO things(color,size) VALUES
               ('red','big'),('red','small'),('green','big'),('blue','big'),('blue','small');",
        )
        .unwrap();
        conn
    }

    fn raw<'a>(color: Option<&'a str>, size: Option<&'a str>) -> impl Fn(&str) -> Option<&'a str> {
        move |key| match key {
            "color" => color,
            "size" => size,
            _ => None,
        }
    }

    #[test]
    fn a_counter_is_the_filter_without_its_own_facet() {
        let facets = definitions();
        let opaque = Opaque::new(b"secret");
        let selection = facets
            .parse(&raw(Some("red"), Some("big")), &opaque)
            .unwrap();
        let conn = database();
        let base = (Vec::new(), Vec::new());
        let counts = facets
            .counts(&conn, &opaque, "things t", &base, &selection)
            .unwrap();
        // Color with the size as chosen (big): red 1, green 1, blue 1.
        assert_eq!(
            counts["color"],
            serde_json::json!({"red": 1, "green": 1, "blue": 1})
        );
        // Size with the color as chosen (red): big 1, small 1.
        assert_eq!(counts["size"], serde_json::json!({"big": 1, "small": 1}));
        assert_eq!(
            facets
                .matched(&conn, "things t", &base, &selection)
                .unwrap(),
            1
        );
    }

    #[test]
    fn several_chosen_options_of_a_facet_show_rows_with_any_of_them() {
        let facets = definitions();
        let opaque = Opaque::new(b"secret");
        let selection = facets.parse(&raw(Some("red,blue"), None), &opaque).unwrap();
        let conn = database();
        let base = (Vec::new(), Vec::new());
        assert_eq!(
            facets
                .matched(&conn, "things t", &base, &selection)
                .unwrap(),
            4
        );
    }

    #[test]
    fn unknown_and_too_long_input_is_refused_with_the_facets_message() {
        let facets = definitions();
        let opaque = Opaque::new(b"secret");
        let unknown = facets.parse(&raw(Some("red,pink"), None), &opaque);
        assert!(matches!(unknown, Err(FacetError::Invalid("unknown color"))));
        let long = "x".repeat(200);
        let too_long = facets.parse(&raw(None, Some(&long)), &opaque);
        assert!(matches!(
            too_long,
            Err(FacetError::Invalid("size is too long"))
        ));
    }
}
