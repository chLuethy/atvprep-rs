//! Read-only media sidecar views.
use crate::facets::{Base, Facet, FacetSet, Selection, Tokens, count_rows, in_list, library_facet};
use crate::filter_params::MAX_PARAM;
use crate::{
    extract::AuthenticatedSession,
    opaque::Opaque,
    read::{error, page_meta, timestamp},
    state::AppState,
};
use atvprep_manager::{
    media_sql::{self, CONVERTED, NEEDS_AUDIO, NEEDS_VIDEO, TO_CONVERT},
    state_machine::ItemStatus,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub struct SidecarQuery {
    #[serde(default, deserialize_with = "crate::opaque::bounded_cursor")]
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct FileQuery {
    #[serde(default, deserialize_with = "crate::opaque::bounded_cursor")]
    cursor: Option<String>,
    limit: Option<usize>,
    root_id: Option<String>,
    kind: Option<String>,
    suffix: Option<String>,
    compliance_state: Option<String>,
    /// Only refused: the states are chosen positively with `status` now.
    hide: Option<String>,
    status: Option<String>,
    has: Option<String>,
    workflow_state: Option<String>,
    discovery_state: Option<String>,
    desired_action: Option<String>,
    #[serde(default, deserialize_with = "python_bool")]
    has_sidecar: Option<bool>,
    last_seen_from: Option<String>,
    last_seen_to: Option<String>,
    q: Option<String>,
    sort: Option<String>,
    dir: Option<String>,
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    root_id: Option<String>,
    path: Option<String>,
    state: Option<String>,
    /// Only refused: the states are chosen positively with `status` now.
    hide: Option<String>,
    status: Option<String>,
    has: Option<String>,
    suffix: Option<String>,
    #[serde(default, deserialize_with = "python_bool")]
    has_sidecar: Option<bool>,
    sort: Option<String>,
    dir: Option<String>,
}

fn python_bool<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    let value = <Option<String> as serde::Deserialize>::deserialize(deserializer)?;
    value
        .map(|value| match value.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" | "t" | "y" => Ok(true),
            "false" | "0" | "no" | "off" | "f" | "n" => Ok(false),
            _ => Err(serde::de::Error::custom("invalid boolean")),
        })
        .transpose()
}

use atvprep_manager::file_state::{self, STATES, TRAITS};

fn status_token(token: &str) -> bool {
    STATES.contains(&token)
}

fn trait_token(token: &str) -> bool {
    TRAITS.contains(&token)
}

fn known_state(state: &str) -> bool {
    status_token(state) || trait_token(state)
}

/// `hide`, the old way of leaving states out, is refused so that no client silently gets every
/// file.
fn hide_input_error(hide: Option<&str>) -> Option<&'static str> {
    hide.map(|_| "hide is no longer accepted, use status")
}

/// A subtitle file lies beside the file.
const SIDECAR: &str = "EXISTS (SELECT 1 FROM sidecars s WHERE s.media_file_id=m.id)";

fn sidecar_tokens() -> Vec<&'static str> {
    vec!["true", "false"]
}

fn state_tokens() -> Vec<&'static str> {
    STATES.to_vec()
}

fn trait_tokens() -> Vec<&'static str> {
    TRAITS.to_vec()
}

/// The facets of the file list in the order their clauses are bound: states (any of them),
/// properties (all of them), libraries (any), extensions (any) and whether a subtitle file lies
/// beside the file. `open_embed_ids` is the JSON list of the ids of the files with an open embedding.
fn file_facets(open_embed_ids: &str) -> FacetSet<'_> {
    use rusqlite::types::Value;
    let token = |value: &Value| match value {
        Value::Text(text) => text.clone(),
        _ => String::new(),
    };
    FacetSet::new(vec![
        Facet::new(
            "status",
            Tokens::Fixed(state_tokens),
            Box::new(move |chosen, values| {
                let any: Vec<String> = chosen
                    .iter()
                    .map(|state| state_clause(&token(state), open_embed_ids, values))
                    .collect();
                format!("({})", any.join(" OR "))
            }),
        )
        .messages("status is too long", "unknown state"),
        Facet::new(
            "has",
            Tokens::Fixed(trait_tokens),
            Box::new(move |chosen, values| {
                state_clause(&token(&chosen[0]), open_embed_ids, values)
            }),
        )
        .all()
        .messages("has is too long", "unknown state"),
        library_facet("root_id is too long", |in_root| in_root),
        Facet::new(
            "suffix",
            Tokens::Open {
                options_sql: SUFFIX_OPTIONS_SQL,
            },
            Box::new(|chosen, values| in_list("m.suffix", chosen, values)),
        )
        .grouped_by("m.suffix")
        .messages("suffix is too long", "invalid request"),
        Facet::new(
            "has_sidecar",
            Tokens::Fixed(sidecar_tokens),
            Box::new(move |chosen, _| {
                let beside = chosen.contains(&Value::from("true".to_owned()));
                format!("{}{SIDECAR}", if beside { "" } else { "NOT " })
            }),
        ),
    ])
}

/// The raw facet values of a request, by address key.
struct FacetParams<'q> {
    status: Option<&'q str>,
    has: Option<&'q str>,
    root_id: Option<&'q str>,
    suffix: Option<&'q str>,
    has_sidecar: Option<bool>,
}

impl FacetParams<'_> {
    /// The choices of the request; the error is the response status and the problem's detail.
    fn parse(
        &self,
        facets: &FacetSet<'_>,
        opaque: &Opaque,
    ) -> Result<Selection, (StatusCode, &'static str)> {
        let sidecar = self
            .has_sidecar
            .map(|beside| if beside { "true" } else { "false" });
        facets
            .parse(
                &|key| match key {
                    "status" => self.status,
                    "has" => self.has,
                    "root_id" => self.root_id,
                    "suffix" => self.suffix,
                    "has_sidecar" => sidecar,
                    _ => None,
                },
                opaque,
            )
            .map_err(|error| error.response())
    }
}

impl<'q> From<&'q BrowseQuery> for FacetParams<'q> {
    /// A folder browse has no library facet: the page leaves out the libraries not chosen.
    fn from(query: &'q BrowseQuery) -> Self {
        Self {
            status: query.status.as_deref(),
            has: query.has.as_deref(),
            root_id: None,
            suffix: query.suffix.as_deref(),
            has_sidecar: query.has_sidecar,
        }
    }
}

impl<'q> From<&'q FileQuery> for FacetParams<'q> {
    fn from(query: &'q FileQuery) -> Self {
        Self {
            status: query.status.as_deref(),
            has: query.has.as_deref(),
            root_id: query.root_id.as_deref(),
            suffix: query.suffix.as_deref(),
            has_sidecar: query.has_sidecar,
        }
    }
}

/// Browse roots or a single level in a library.
pub async fn browse(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<BrowseQuery>,
) -> Response {
    browse_inner(state, query, false).await
}

/// List all files below a library folder.
pub async fn browse_below(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<BrowseQuery>,
) -> Response {
    browse_inner(state, query, true).await
}

fn browse_input_error(query: &BrowseQuery, below: bool) -> Option<&'static str> {
    if (below && query.root_id.is_none())
        || sort_input_error(query.sort.as_deref(), query.dir.as_deref())
    {
        return Some("request validation failed");
    }
    if query
        .state
        .as_deref()
        .is_some_and(|state| !known_state(state))
    {
        return Some("unknown state");
    }
    list_input_error(query.hide.as_deref(), &FacetParams::from(query))
}

/// The input error of a list request's `hide` and facet values, if any. An id that names no library
/// is not an input error here: it is not found once the list is read.
fn list_input_error(hide: Option<&str>, params: &FacetParams<'_>) -> Option<&'static str> {
    hide_input_error(hide).or_else(|| {
        // Only the checks of the address matter here, so no embedding ids are needed.
        match params.parse(&file_facets(""), &Opaque::new(b"")) {
            Err((StatusCode::UNPROCESSABLE_ENTITY, message)) => Some(message),
            _ => None,
        }
    })
}

/// `sort` and `dir` as the list and the column view take them; no sort is recently added.
fn sort_input_error(sort: Option<&str>, dir: Option<&str>) -> bool {
    !matches!(sort, None | Some("title" | "status" | "size"))
        || !matches!(dir, None | Some("asc" | "desc"))
}

/// How many files one column lists at most.
const MAX_BROWSE_FILES: usize = 500;

/// A file directly in the column, with what it sorts by.
struct Listed {
    id: i64,
    /// The path in lower case, the tie-breaker.
    path: String,
    title: String,
    size: i64,
    rank: u8,
}

impl Listed {
    fn new(id: i64, path: &str, size: i64, rank: u8) -> Self {
        Self {
            id,
            path: path.to_lowercase(),
            title: file_title(path),
            size,
            rank,
        }
    }
}

/// What the files below a folder or library add up to.
#[derive(Default)]
struct Below {
    counts: std::collections::BTreeMap<String, u64>,
    size: i64,
    /// The worst status rank below; none without files.
    worst: Option<u8>,
    /// The highest file id below: the most recently added file.
    newest: i64,
}

impl Below {
    fn add(&mut self, key: String, id: i64, size: i64, rank: u8) {
        *self.counts.entry(key).or_default() += 1;
        self.size = self.size.saturating_add(size);
        self.worst = Some(self.worst.map_or(rank, |worst| worst.min(rank)));
        self.newest = self.newest.max(id);
    }
}

#[derive(Clone, Copy)]
enum SortKey {
    Title,
    Status,
    Size,
    Added,
}

/// The order of a column: the key of `/files` and its direction.
#[derive(Clone, Copy)]
struct Order {
    key: SortKey,
    descending: bool,
}

impl Order {
    fn of(sort: Option<&str>, dir: Option<&str>) -> Self {
        let key = match sort {
            Some("title") => SortKey::Title,
            Some("status") => SortKey::Status,
            Some("size") => SortKey::Size,
            _ => SortKey::Added,
        };
        Self {
            key,
            descending: dir == Some("desc"),
        }
    }

    fn directed(self, ordering: std::cmp::Ordering) -> std::cmp::Ordering {
        if self.descending {
            ordering.reverse()
        } else {
            ordering
        }
    }

    /// Files as the list orders them, the path breaking a tie.
    fn files(self, a: &Listed, b: &Listed) -> std::cmp::Ordering {
        let primary = match self.key {
            SortKey::Title => a.title.cmp(&b.title),
            SortKey::Status => a.rank.cmp(&b.rank),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Added => a.id.cmp(&b.id),
        };
        self.directed(primary)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.id.cmp(&b.id))
    }

    /// Folders by a value over the files below them. A tie is left as it is, so sort by name first.
    fn folders(self, a_name: &str, a: &Below, b_name: &str, b: &Below) -> std::cmp::Ordering {
        let primary = match self.key {
            SortKey::Title => by_name(a_name, b_name),
            // A library without files has nothing to fix: after the ready ones.
            SortKey::Status => a.worst.unwrap_or(u8::MAX).cmp(&b.worst.unwrap_or(u8::MAX)),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Added => a.newest.cmp(&b.newest),
        };
        self.directed(primary)
    }
}

/// Folder names in casefold order, the name itself breaking a tie.
fn by_name(a: &str, b: &str) -> std::cmp::Ordering {
    crate::casefold::python_casefold(a)
        .cmp(&crate::casefold::python_casefold(b))
        .then_with(|| a.cmp(b))
}

/// What a file sorts by under "Name": its file name without the extension, case-folded.
fn file_title(path: &str) -> String {
    let display = crate::read_jobs::subject(path);
    let name = display.rsplit('/').next().unwrap_or(&display);
    let stem = name
        .rfind('.')
        .filter(|i| *i > 0)
        .map_or(name, |i| &name[..i]);
    crate::casefold::python_casefold(stem)
}

#[allow(clippy::too_many_lines)] // Keep validation and its parameterized query together.
async fn browse_inner(state: AppState, query: BrowseQuery, below: bool) -> Response {
    const PATH: &str = "/api/v1/browse";
    if let Some(message) = browse_input_error(&query, below) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, message, PATH);
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let root = match query.root_id.as_deref() {
        Some(token) => match opaque.decode(token, "r") {
            Ok(id) if id > 0 => Some(id),
            _ => return error(StatusCode::NOT_FOUND, "resource not found", PATH),
        },
        None => None,
    };
    let folder = query.path.as_deref().unwrap_or("").trim_matches('/');
    if root.is_some()
        && (folder.chars().count() > 1024
            || folder.contains(['\0', '\\'])
            || (!folder.is_empty()
                && folder
                    .split('/')
                    .any(|part| matches!(part, "" | "." | ".."))))
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "path must be a folder inside a library",
            PATH,
        );
    }
    let folder = folder.to_owned();
    let Some(open_embed_ids) = embed_open_json(&state).await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        );
    };
    let open_ids: std::collections::BTreeSet<i64> =
        serde_json::from_str::<Vec<i64>>(&open_embed_ids)
            .unwrap_or_default()
            .into_iter()
            .collect();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();
    let mut clauses =
        vec!["NOT (m.discovery_state='missing' AND m.dismissed_at IS NOT NULL)".to_owned()];
    if let Some(value) = query.state.as_deref() {
        clauses.push(state_clause(value, &open_embed_ids, &mut values));
    }
    let facets = file_facets(&open_embed_ids);
    let selection = match FacetParams::from(&query).parse(&facets, &opaque) {
        Ok(selection) => selection,
        Err((code, message)) => return error(code, message, PATH),
    };
    let filtered = query.state.is_some() || selection.any();
    clauses.extend(facets.clauses(&selection, &mut values));
    if let Some(root) = root {
        clauses.push("m.root_id=?".to_owned());
        values.push(root.into());
        let prefix = if folder.is_empty() {
            String::new()
        } else {
            format!("{folder}/")
        };
        clauses.push("substr(m.relative_path,1,?)=?".to_owned());
        values.push(
            i64::try_from(prefix.chars().count())
                .unwrap_or(i64::MAX)
                .into(),
        );
        values.push(prefix.into());
    }
    let order = Order::of(query.sort.as_deref(), query.dir.as_deref());
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        let conn = conn.raw();
        if let Some(root) = root {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM library_roots WHERE id=?)",
                    [root],
                    |row| row.get(0),
                )
                .map_err(|_| ())?;
            if !exists {
                return Ok(None);
            }
        }
        let filter = clauses.join(" AND ");
        let mut stmt = conn
            .prepare(&format!(
                "SELECT m.id,m.root_id,m.relative_path,m.discovery_state,m.compliance_state,m.size \
                 FROM media_files m WHERE {filter}"
            ))
            .map_err(|_| ())?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(values.iter()), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|_| ())?;
        let mut groups: std::collections::BTreeMap<String, Below> =
            std::collections::BTreeMap::new();
        let mut direct = Vec::new();
        for row in rows {
            let (id, root_id, path, discovery, compliance, size) = row.map_err(|_| ())?;
            let open = open_ids.contains(&id);
            let state = file_state::state_of(&discovery, &compliance, open);
            let rank = file_state::rank(state);
            let key = if matches!(
                query.state.as_deref(),
                Some("convert" | "converted" | "encode_audio" | "encode_video" | "embed")
            ) {
                query.state.clone().unwrap_or_default()
            } else {
                state.to_owned()
            };
            let group = if root.is_none() {
                Some(root_id.to_string())
            } else if below {
                None
            } else {
                let rest = if folder.is_empty() {
                    path.as_str()
                } else {
                    path.strip_prefix(&format!("{folder}/")).unwrap_or("")
                };
                rest.split_once('/').map(|(name, _)| name.to_owned())
            };
            if let Some(group) = group {
                groups.entry(group).or_default().add(key, id, size, rank);
            } else if root.is_some() {
                direct.push(Listed::new(id, &path, size, rank));
            }
        }
        // The cut comes after the order, so the files listed are the first ones in it.
        direct.sort_by(|a, b| order.files(a, b));
        let extra = direct.len() > MAX_BROWSE_FILES;
        direct.truncate(MAX_BROWSE_FILES);
        let mut entries: Vec<(String, Option<i64>, Below)> = Vec::new();
        if root.is_none() {
            let mut roots = conn
                .prepare("SELECT id,name FROM library_roots ORDER BY id")
                .map_err(|_| ())?;
            let rows = roots
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .map_err(|_| ())?;
            for row in rows {
                let (id, name) = row.map_err(|_| ())?;
                let below = groups.remove(&id.to_string()).unwrap_or_default();
                if filtered && below.counts.is_empty() {
                    continue;
                }
                entries.push((name, Some(id), below));
            }
        } else if !below {
            entries.extend(groups.into_iter().map(|(name, below)| (name, None, below)));
        }
        // Name order first: the sort by the value is stable, so a tie stays in it.
        entries.sort_by(|a, b| by_name(&a.0, &b.0));
        entries.sort_by(|a, b| order.folders(&a.0, &a.2, &b.0, &b.2));
        let mut folders: Vec<Value> = entries
            .into_iter()
            .map(|(name, id, below)| {
                let total: u64 = below.counts.values().sum();
                json!({
                    "name": crate::read_jobs::safe_head(&name, 2048),
                    "id": id.map(|id| opaque.encode("r", id)),
                    "counts": below.counts,
                    "total": total,
                    "size": below.size,
                })
            })
            .collect();
        let mut files=Vec::new();let mut used=serde_json::to_vec(&folders).map_err(|_|())?.len();let mut truncated=extra;
        for Listed { id, .. } in direct {let Some(mut value)=load_file(conn,&opaque,id)? else {continue};if let Some(object)=value.as_object_mut(){for key in ["status_version","last_success_at","has_error","sidecar_count","inspection_available"]{object.remove(key);}}
            let size=serde_json::to_vec(&value).map_err(|_|())?.len();if size>crate::bounds::MAX_ELEMENT_BYTES || used.saturating_add(size+1024)>crate::bounds::MAX_RESPONSE_BYTES {truncated=true;break;}used+=size;files.push(value);}
        while serde_json::to_vec(&folders).map_err(|_|())?.len()+1024>crate::bounds::MAX_RESPONSE_BYTES {folders.pop();truncated=true;}
        let data=if below {json!({"files":files,"truncated":truncated})} else {json!({"root_id":query.root_id,"path":if root.is_some(){folder}else{String::new()},"folders":folders,"files":files,"truncated":truncated})};
        Ok::<_,()>(Some(json!({"data":data,"meta":{}})))
    }).await;
    match result {
        Ok(Ok(Some(body))) => Json(body).into_response(),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "resource not found", PATH),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

/// The facet counters for the list with the same filters as `/files`, as the facet engine counts
/// them (each option with every filter but its own facet's choice, a property with the choice plus
/// that property), the files that match all filters and all files there are.
pub async fn facet_counts(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<FileQuery>,
) -> Response {
    const PATH: &str = "/api/v1/files/counts";
    if let Some(response) = filter_input_error(&query, PATH) {
        return response;
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let Some(open_embed_ids) = embed_open_json(&state).await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        );
    };
    let base = match base_filter(&query, &open_embed_ids) {
        Ok(filter) => filter,
        Err((code, message)) => return error(code, message, PATH),
    };
    let selection = match FacetParams::from(&query).parse(&file_facets(&open_embed_ids), &opaque) {
        Ok(selection) => selection,
        Err((code, message)) => return error(code, message, PATH),
    };
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        let conn = conn.raw();
        let facets = file_facets(&open_embed_ids);
        let mut data = facets
            .counts(conn, &opaque, FROM, &base, &selection)
            .map_err(|_| ())?;
        let matched = facets
            .matched(conn, FROM, &base, &selection)
            .map_err(|_| ())?;
        let total = count_rows(conn, FROM, LISTED, &[]).map_err(|_| ())?;
        data.insert("matched".to_owned(), json!(matched));
        data.insert("total".to_owned(), json!(total));
        Ok::<_, ()>(json!({"data": data, "meta": {}}))
    })
    .await;
    match result {
        Ok(Ok(body)) => Json(body).into_response(),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

/// The ids of the files whose embedding is still open, as a JSON list to bind to `json_each`.
async fn embed_open_json(state: &AppState) -> Option<String> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().ok()?;
        atvprep_manager::embed::open_media_ids_json(conn.raw()).ok()
    })
    .await
    .ok()
    .flatten()
}

/// The clause for one chosen token. The states split the files without overlap (`file_state`): a
/// missing file is only "missing", "ready" has nothing left to embed and `embed_open` has. The trait
/// "embed" is any present file with an open embedding, whatever its state.
/// `open_embed_ids` is the JSON list of the ids of the files with an open embedding.
fn state_clause(
    state: &str,
    open_embed_ids: &str,
    values: &mut Vec<rusqlite::types::Value>,
) -> String {
    match state {
        "embed" => {
            values.push(open_embed_ids.to_owned().into());
            format!(
                "(m.discovery_state='present' AND {})",
                media_sql::embed_open("?")
            )
        }
        "convert" => format!("(m.discovery_state='present' AND {TO_CONVERT})"),
        "converted" => format!("(m.discovery_state='present' AND {CONVERTED})"),
        "encode_audio" => format!("(m.discovery_state='present' AND {NEEDS_AUDIO})"),
        "encode_video" => format!("(m.discovery_state='present' AND {NEEDS_VIDEO})"),
        _ => {
            values.push(open_embed_ids.to_owned().into());
            values.push(state.to_owned().into());
            format!("{}=?", file_state::state_sql("?"))
        }
    }
}

fn db_datetime(value: &str) -> Option<String> {
    let date = chrono::DateTime::parse_from_rfc3339(value)
        .map(|v| v.naive_utc())
        .ok()
        .or_else(|| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f").ok())
        .or_else(|| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f").ok())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
        })?;
    Some(date.format("%Y-%m-%d %H:%M:%S%.6f").to_string())
}

/// The files the list shows at all: a missing file the user removed is gone.
macro_rules! listed {
    () => {
        "NOT (m.discovery_state='missing' AND m.dismissed_at IS NOT NULL)"
    };
}

const LISTED: &str = listed!();

/// The table the file list reads.
const FROM: &str = "media_files m";

/// The extensions the counters offer: every one there is, up to a limit.
const SUFFIX_OPTIONS_SQL: &str = concat!(
    "SELECT DISTINCT m.suffix FROM media_files m WHERE ",
    listed!(),
    " ORDER BY m.suffix LIMIT 200"
);

/// Bounds and token checks shared by the list and its counters.
fn filter_input_error(query: &FileQuery, path: &str) -> Option<Response> {
    for (name, value) in [
        ("kind", &query.kind),
        ("compliance_state", &query.compliance_state),
        ("workflow_state", &query.workflow_state),
        ("discovery_state", &query.discovery_state),
        ("desired_action", &query.desired_action),
        ("q", &query.q),
    ] {
        if value
            .as_ref()
            .is_some_and(|value| value.chars().count() > MAX_PARAM)
        {
            return Some(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("{name} is too long"),
                path,
            ));
        }
    }
    list_input_error(query.hide.as_deref(), &FacetParams::from(query))
        .map(|message| error(StatusCode::UNPROCESSABLE_ENTITY, message, path))
}

/// Every filter of the list except the facets (`file_facets`), which the counters vary.
fn base_filter(
    query: &FileQuery,
    open_embed_ids: &str,
) -> Result<Base, (StatusCode, &'static str)> {
    let mut clauses = vec![LISTED.to_owned()];
    let mut values: Vec<rusqlite::types::Value> = Vec::new();
    for (column, value) in [
        ("kind", &query.kind),
        ("workflow_state", &query.workflow_state),
        ("discovery_state", &query.discovery_state),
        ("desired_action", &query.desired_action),
    ] {
        if let Some(value) = value.as_deref().filter(|s| !s.is_empty()) {
            clauses.push(format!("m.{column}=?"));
            values.push(value.to_owned().into());
        }
    }
    if let Some(value) = query.compliance_state.as_deref().filter(|s| !s.is_empty()) {
        clauses.push(state_clause(value, open_embed_ids, &mut values));
    }
    for (op, value) in [(">=", &query.last_seen_from), ("<=", &query.last_seen_to)] {
        if let Some(value) = value {
            let Some(date) = db_datetime(value) else {
                return Err((StatusCode::UNPROCESSABLE_ENTITY, "invalid request"));
            };
            clauses.push(format!("m.last_seen_at{op}?"));
            values.push(date.into());
        }
    }
    if let Some(value) = query.q.as_deref().filter(|s| !s.is_empty()) {
        clauses.push("lower(m.relative_path) LIKE lower(?)".to_owned());
        values.push(format!("%{value}%").into());
    }
    Ok((clauses, values))
}

/// List library files with the reference filters, facets and ordering.
#[allow(clippy::too_many_lines)] // Keep validation and its parameterized query together.
pub async fn list(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<FileQuery>,
) -> Response {
    const PATH: &str = "/api/v1/files";
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) || sort_input_error(query.sort.as_deref(), query.dir.as_deref())
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "request validation failed",
            PATH,
        );
    }
    if let Some(response) = filter_input_error(&query, PATH) {
        return response;
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let after = match query.cursor.as_deref() {
        Some(value) => match opaque.decode(value, "cursor:files") {
            Ok(id) if id > 0 => id,
            _ => return error(StatusCode::BAD_REQUEST, "invalid cursor", PATH),
        },
        None => 0,
    };
    let Some(open_embed_ids) = embed_open_json(&state).await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        );
    };
    let (mut clauses, mut values) = match base_filter(&query, &open_embed_ids) {
        Ok(filter) => filter,
        Err((code, message)) => return error(code, message, PATH),
    };
    {
        let facets = file_facets(&open_embed_ids);
        match FacetParams::from(&query).parse(&facets, &opaque) {
            Ok(chosen) => clauses.extend(facets.clauses(&chosen, &mut values)),
            Err((code, message)) => return error(code, message, PATH),
        }
    }
    // Without a sort the list is in the order the files were added; the cursor is the last id.
    if query.sort.is_none() && after > 0 {
        clauses.push(
            if query.dir.as_deref() == Some("desc") {
                "m.id<?"
            } else {
                "m.id>?"
            }
            .to_owned(),
        );
        values.push(after.into());
    }
    let result=tokio::task::spawn_blocking(move || {
        let conn=state.db.conn().map_err(|_|())?;let conn=conn.raw();let filter=clauses.join(" AND ");
        let mut facet_stmt=conn.prepare(&format!("SELECT DISTINCT m.suffix FROM media_files m WHERE {filter} ORDER BY m.suffix LIMIT 50")).map_err(|_|())?;
        let facets:Vec<String>=facet_stmt.query_map(rusqlite::params_from_iter(values.iter()),|row|row.get::<_,String>(0)).map_err(|_|())?.collect::<Result<Vec<_>,_>>().map_err(|_|())?.into_iter().map(|value|crate::read_jobs::safe_head(&value,256)).collect();
        let facet_size=serde_json::to_vec(&facets).map_err(|_|())?.len();
        let descending=query.dir.as_deref()==Some("desc");
        let ids:Vec<i64>=if query.sort.as_deref()==Some("title") {
            let mut stmt=conn.prepare(&format!("SELECT m.id,m.relative_path FROM media_files m WHERE {filter}")).map_err(|_|())?;
            let mut rows=stmt.query_map(rusqlite::params_from_iter(values.iter()),|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?))).map_err(|_|())?.collect::<Result<Vec<_>,_>>().map_err(|_|())?;
            rows.sort_by_cached_key(|(id,path)| {let display=crate::read_jobs::subject(path);let name=display.rsplit('/').next().unwrap_or(&display);let stem=name.rfind('.').filter(|i|*i>0).map_or(name,|i|&name[..i]);(crate::casefold::python_casefold(stem),*id)});
            if descending {rows.reverse();}
            rows.into_iter().skip(usize::try_from(after).unwrap_or(usize::MAX)).take(limit+1).map(|(id,_)|id).collect()
        }else {
            let order=match query.sort.as_deref() {Some("size")=>format!("m.size {} ,lower(m.relative_path),m.id",if descending {"DESC"} else {"ASC"}),Some("status")=>format!("{} {},lower(m.relative_path),m.id",file_state::rank_sql("?"),if descending {"DESC"} else {"ASC"}),_=>if descending {"m.id DESC".to_owned()} else {"m.id".to_owned()}};
            let offset=if query.sort.is_some(){after}else{0};
            let mut stmt=conn.prepare(&format!("SELECT m.id FROM media_files m WHERE {filter} ORDER BY {order} LIMIT ? OFFSET ?")).map_err(|_|())?;
            let mut bound=values.clone();if query.sort.as_deref()==Some("status") {bound.push(open_embed_ids.clone().into());}bound.push(i64::try_from(limit+1).map_err(|_|())?.into());bound.push(offset.into());
            stmt.query_map(rusqlite::params_from_iter(bound.iter()),|row|row.get(0)).map_err(|_|())?.collect::<Result<Vec<_>,_>>().map_err(|_|())?
        };
        let mut data=Vec::new();let mut last=None;let mut used=0usize;let mut truncated=false;
        for id in ids.iter().take(limit) {let Some(mut value)=load_file(conn,&opaque,*id)? else {continue}; if let Some(object)=value.as_object_mut(){for key in ["status_version","last_success_at","has_error","sidecar_count","inspection_available"] {object.remove(key);}}
            let size=serde_json::to_vec(&value).map_err(|_|())?.len();if size>crate::bounds::MAX_ELEMENT_BYTES || used.saturating_add(size).saturating_add(data.len()+facet_size+1024)>crate::bounds::MAX_RESPONSE_BYTES {truncated=true;break;}used+=size;last=Some(*id);data.push(value);
        }
        let next=if truncated {if query.sort.is_some() {if data.is_empty() {None} else {after.checked_add(i64::try_from(data.len()).map_err(|_|())?)}} else {last}} else if ids.len()>limit {if query.sort.is_some() {after.checked_add(i64::try_from(limit).map_err(|_|())?)} else {last}} else {None};
        let next=next.map(|id|opaque.encode("cursor:files",id));let mut meta=page_meta(Some(limit),next.as_deref(),truncated);meta["facets"]=json!({"suffix":facets});
        Ok::<_,()>(json!({"data":data,"meta":meta}))
    }).await;
    match result {
        Ok(Ok(body)) => Json(body).into_response(),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "authorization",
        "cookie",
        "password",
        "secret",
        "token",
        "credential",
        "api_key",
        "api-key",
        "apikey",
    ]
    .iter()
    .any(|part| key.contains(part))
}

pub(crate) fn inspection_value(value: &Value, depth: usize) -> Value {
    if depth >= 8 {
        return json!("<depth-limit>");
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .take(64)
                .map(|(key, value)| {
                    let key: String = key.chars().take(256).collect();
                    let value = if sensitive_key(&key) {
                        json!("<redacted>")
                    } else {
                        inspection_value(value, depth + 1)
                    };
                    (key, value)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(64)
                .map(|item| inspection_value(item, depth + 1))
                .collect(),
        ),
        Value::String(value) => {
            let value = crate::redact::redact_text(value);
            let mut end = value.len().min(2048);
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            json!(&value[..end])
        }
        _ => value.clone(),
    }
}

/// Read redacted, bounded inspection reports for a media file.
pub async fn inspections(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Path(token): Path<String>,
    Query(query): Query<SidecarQuery>,
) -> Response {
    const PATH: &str = "/api/v1/files/inspections";
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let media = match opaque.decode(&token, "m") {
        Ok(id) if id > 0 => id,
        _ => return error(StatusCode::NOT_FOUND, "resource not found", PATH),
    };
    let cursor = match query.cursor.as_deref() {
        Some(value) => match opaque.decode_timestamp(value, "cursor:inspections") {
            Ok((time, id)) if id > 0 => Some((time, id)),
            _ => return error(StatusCode::BAD_REQUEST, "invalid cursor", PATH),
        },
        None => None,
    };
    let result=tokio::task::spawn_blocking(move ||{
        let conn=state.db.conn().map_err(|_|())?;
        let exists:bool=conn.raw().query_row("SELECT EXISTS(SELECT 1 FROM media_files WHERE id=?1)",[media],|row|row.get(0)).map_err(|_|())?;
        if !exists{return Ok(None);}
        let mut stmt=conn.raw().prepare("SELECT id,tool_name,tool_version,result,created_at FROM inspections WHERE media_file_id=?1 AND (?2 IS NULL OR created_at<?2 OR (created_at=?2 AND id<?3)) ORDER BY created_at DESC,id DESC LIMIT ?4").map_err(|_|())?;
        let rows=stmt.query_map(params![media,cursor.as_ref().map(|c|&c.0),cursor.as_ref().map(|c|c.1),i64::try_from(limit+1).map_err(|_|())?],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?))).map_err(|_|())?;
        let mut data=Vec::new();let mut last=None;let mut used=0usize;let mut more=false;let mut truncated=false;
        for row in rows {
            let (id,name,version,report,created)=row.map_err(|_|())?;
            if data.len()==limit {more=true;break;}
            let Ok(cursor)=opaque.encode_timestamp("cursor:inspections",&created,id) else {more=true;truncated=true;break;};
            let parsed:Value=serde_json::from_str(&report).unwrap_or_else(|_|json!({"truncated":true}));
            let report=crate::redact::redact_inspection_value(&parsed,crate::bounds::MAX_INSPECTION_BYTES);
            let value=json!({"id":opaque.encode("n",id),"media_id":opaque.encode("m",media),"tool_name":crate::read_jobs::safe_head(&name,2048),"tool_version":crate::read_jobs::safe_head(&version,2048),"result":report,"created_at":timestamp(&created)});
            let size=serde_json::to_vec(&value).map_err(|_|())?.len();
            if size>60*1024 || used.saturating_add(size).saturating_add(data.len()+256)>crate::bounds::MAX_RESPONSE_BYTES {more=true;truncated=true;break;}
            used+=size;last=Some(cursor);data.push(value);
        }
        let next=if more{last}else{None};
        Ok::<_,()>(Some(json!({"data":data,"meta":page_meta(Some(limit),next.as_deref(),truncated)})))
    }).await;
    match result {
        Ok(Ok(Some(body))) => Json(body).into_response(),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "resource not found", PATH),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

fn load_file(
    conn: &rusqlite::Connection,
    opaque: &Opaque,
    media: i64,
) -> Result<Option<Value>, ()> {
    let row:Option<(Value,String,String)>=conn.query_row(&format!("SELECT m.root_id,m.relative_path,m.kind,m.suffix,m.size,m.mtime_ns,m.discovery_state,m.compliance_state,m.compliance_reasons,m.workflow_state,m.desired_action,m.last_seen_at,m.status_version,m.last_success_at,m.last_error,m.fingerprint, (SELECT count(*) FROM sidecars s WHERE s.media_file_id=m.id), EXISTS(SELECT 1 FROM inspections n WHERE n.media_file_id=m.id), {CONVERTED}, m.encode_need, m.planned_change FROM media_files m WHERE m.id=?1"),[media],|row|{
            let root:i64=row.get(0)?;let path:String=row.get(1)?;let reasons:String=row.get(8)?;let fingerprint:String=row.get(15)?;
            let parsed:Value=serde_json::from_str(&reasons).unwrap_or_else(|_|json!([]));
            let reasons:Vec<String>=parsed.as_array().map(|items|items.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
            let encode_need:String=row.get(19)?;let planned_change:String=row.get(20)?;
            let safe_reasons:Vec<String>=reasons.iter().map(|reason|crate::read_jobs::safe_head(reason,2048)).collect();
            let data=json!({"id":opaque.encode("m",media),"root_id":opaque.encode("r",root),"display_path":crate::read_jobs::subject(&path),
                "kind":row.get::<_,String>(2)?,"suffix":row.get::<_,String>(3)?,"size":row.get::<_,i64>(4)?,"mtime_ns":row.get::<_,i64>(5)?,
                "discovery_state":row.get::<_,String>(6)?,"compliance_state":row.get::<_,String>(7)?,"compliance_reasons":safe_reasons,
                "workflow_state":row.get::<_,String>(9)?,"desired_action":row.get::<_,String>(10)?,"last_seen_at":row.get::<_,Option<String>>(11)?.map(|v|timestamp(&v)),
                "encode_need":encode_need,"planned_change":planned_change,"converted_kept":row.get::<_,bool>(18)?,"fix_status":null,"pending_sidecars":0,"embed_sidecars":[],
                "status_version":row.get::<_,i64>(12)?,"last_success_at":row.get::<_,Option<String>>(13)?.map(|v|timestamp(&v)),
                "has_error":row.get::<_,Option<String>>(14)?.is_some(),"sidecar_count":row.get::<_,i64>(16)?,"inspection_available":row.get::<_,bool>(17)?});
            Ok((data,path,fingerprint))
        }).optional().map_err(|_|())?;
    let Some((mut data, path, fingerprint)) = row else {
        return Ok(None);
    };
    let classified = outcomes(conn, media, &path, &fingerprint)?;
    let embedded:Vec<Value>=classified.iter().filter(|(_,_,_,status)|status=="pending").map(|(_,name,lang,_)|json!({"name":name,"language":lang,"kind":atvprep_core::sidecars::sidecar_kind(name)})).collect();
    data["pending_sidecars"] = json!(embedded.len());
    data["embed_sidecars"] = json!(embedded);
    // Only present files with a pending subtitle file are open, as `embed::open_media_ids` has it.
    let discovery = data["discovery_state"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let compliance = data["compliance_state"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let desired = data["desired_action"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let open = discovery == "present" && !embedded.is_empty();
    data["state"] = json!(file_state::state_of(&discovery, &compliance, open));
    data["preparable"] = json!(file_state::is_preparable(
        &discovery,
        &compliance,
        &desired,
        open
    ));
    let fix_status:Option<String>=conn.query_row(&format!("SELECT CASE WHEN max(CASE WHEN status='running' THEN 2 WHEN status IN ('cancel_requested','cancelling') THEN 1 ELSE 0 END)=2 THEN 'running' WHEN max(CASE WHEN status='running' THEN 2 WHEN status IN ('cancel_requested','cancelling') THEN 1 ELSE 0 END)=1 THEN 'cancelling' ELSE 'queued' END FROM job_items WHERE media_file_id=?1 AND action IN ('fix','convert') AND status NOT IN {} HAVING count(*)>0",ItemStatus::terminal_sql_list()),[media],|row|row.get(0)).optional().map_err(|_|())?;
    data["fix_status"] = json!(fix_status);
    // The newest item of the file that has started (any action) is an interrupted fix or convert, and no
    // inspection was stored after it. Anything later clears the flag.
    let last_interrupted:bool=conn.query_row("SELECT COALESCE((SELECT i.action IN ('fix','convert') AND i.status='interrupted' AND NOT EXISTS(SELECT 1 FROM inspections n WHERE n.media_file_id=?1 AND i.finished_at IS NOT NULL AND datetime(n.created_at)>datetime(i.finished_at)) FROM job_items i WHERE i.media_file_id=?1 AND i.status<>'queued' ORDER BY i.id DESC LIMIT 1),0)",[media],|row|row.get(0)).map_err(|_|())?;
    data["last_interrupted"] = json!(last_interrupted);
    Ok(Some(data))
}

/// Read a file's state, sidecar summary and inspection availability.
pub async fn detail(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Path(token): Path<String>,
) -> Response {
    const PATH: &str = "/api/v1/files";
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let media = match opaque.decode(&token, "m") {
        Ok(id) if id > 0 => id,
        _ => return error(StatusCode::NOT_FOUND, "resource not found", PATH),
    };
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        let Some(mut data) = load_file(conn.raw(), &opaque, media)? else {
            return Ok(None);
        };
        let truncated = serde_json::to_vec(&data)
            .map_or(true, |value| value.len() > crate::bounds::MAX_ELEMENT_BYTES);
        if truncated {
            data = json!({"truncated": true});
        }
        Ok::<_, ()>(Some(
            json!({"data": data, "meta": page_meta(None, None, truncated)}),
        ))
    })
    .await;
    match result {
        Ok(Ok(Some(body))) => Json(body).into_response(),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "resource not found", PATH),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

fn outcomes(
    conn: &rusqlite::Connection,
    media: i64,
    file_name: &str,
    fingerprint: &str,
) -> Result<Vec<(i64, String, String, String)>, ()> {
    atvprep_manager::embed::classify_media(conn, media, file_name, fingerprint).map_err(|_| ())
}

/// Return safe sidecar names and their current classification for one media file.
pub async fn sidecars(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Path(token): Path<String>,
    Query(query): Query<SidecarQuery>,
) -> Response {
    const PATH: &str = "/api/v1/files/sidecars";
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let media = match opaque.decode(&token, "m") {
        Ok(id) if id > 0 => id,
        _ => return error(StatusCode::NOT_FOUND, "resource not found", PATH),
    };
    let before = match query.cursor.as_deref() {
        Some(value) => match opaque.decode(value, "cursor:sidecars") {
            Ok(id) if id > 0 => Some(id),
            _ => return error(StatusCode::BAD_REQUEST, "invalid cursor", PATH),
        },
        None => None,
    };
    let result=tokio::task::spawn_blocking(move ||{
        let conn=state.db.conn().map_err(|_|())?;
        let file:Option<(String,String)>=conn.raw().query_row("SELECT relative_path,fingerprint FROM media_files WHERE id=?1",[media],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|_|())?;
        let Some((file_name,fingerprint))=file else{return Ok(None)};
        let outcomes:std::collections::BTreeMap<i64,(String,String)>=outcomes(conn.raw(),media,&file_name,&fingerprint)?.into_iter().map(|(id,_,lang,state)|(id,(lang,state))).collect();
        let mut stmt=conn.raw().prepare("SELECT id,relative_path,size,mtime_ns,fingerprint,created_at FROM sidecars WHERE media_file_id=?1 AND (?2 IS NULL OR id<?2) ORDER BY id DESC LIMIT ?3").map_err(|_|())?;
        let rows=stmt.query_map(params![media,before,i64::try_from(limit+1).map_err(|_|())?],|row| Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?,row.get::<_,String>(4)?,row.get::<_,String>(5)?))).map_err(|_|())?;
        let mut data=Vec::new();let mut last=None;let mut used=0usize;let mut more=false;let mut truncated=false;
        for row in rows {let (id,path,size,mtime,fp,created)=row.map_err(|_|())?;if data.len()==limit {more=true;break;}let (language,state)=outcomes.get(&id).cloned().unwrap_or_else(||("und".to_owned(),"unchecked".to_owned()));let value=json!({"id":opaque.encode("c",id),"media_id":opaque.encode("m",media),"name":atvprep_manager::embed::basename(&path),"size":size,"mtime_ns":mtime,"fingerprint":crate::read_jobs::safe_head(&fp,2048),"created_at":timestamp(&created),"language":language,"state":state});let size=serde_json::to_vec(&value).map_err(|_|())?.len();if size>crate::bounds::MAX_ELEMENT_BYTES || used.saturating_add(size).saturating_add(data.len()+256)>crate::bounds::MAX_RESPONSE_BYTES{more=true;truncated=true;break;}used+=size;last=Some(id);data.push(value);}
        let next=if more{last.map(|id|opaque.encode("cursor:sidecars",id))}else{None};
        Ok::<_,()>(Some(json!({"data":data,"meta":page_meta(Some(limit),next.as_deref(),truncated)})))
    }).await;
    match result {
        Ok(Ok(Some(body))) => Json(body).into_response(),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "resource not found", PATH),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::inspection_value;
    use serde_json::json;

    #[test]
    fn inspection_reports_redact_nested_sensitive_values_and_paths() {
        let input = json!({"path":"/private/file.mp4","nested":[{"password":"hidden","message":"failed C:/private/file"}]});
        let output = inspection_value(&input, 0);
        assert_eq!(output["path"], "<path>");
        assert_eq!(output["nested"][0]["password"], "<redacted>");
        assert_eq!(output["nested"][0]["message"], "failed <path>");
        assert!(!output.to_string().contains("hidden"));
        assert!(!output.to_string().contains("private"));
    }
}

#[cfg(test)]
mod http_tests {
    use crate::router;
    use crate::routes_auth;
    use crate::test_support::state;
    use atvprep_fixtures::Fixtures;
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use tower::ServiceExt;

    #[tokio::test]
    async fn sidecars_classify_current_probes_and_page_backwards() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let fx = Fixtures::new(conn.raw());
        let file = fx
            .file(fx.library_at("Movies", "/private"), "film.mp4")
            .fingerprint("current")
            .create();
        let _ = fx.sidecar(file, "/secret/film.en.srt", "a");
        let _ = fx.sidecar(file, "film.de.srt", "b");
        let _ = fx.inspection(
            file,
            "ffprobe",
            "current",
            &serde_json::json!({"streams": []}),
        );
        let media = crate::opaque::Opaque::new(state.api_secret.as_bytes()).encode("m", 1);
        let session = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={session}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(get(&format!("/api/v1/files/{media}/sidecars?limit=1")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"][0]["name"], "film.de.srt");
        assert_eq!(body["data"][0]["state"], "pending");
        let cursor = body["meta"]["next_cursor"].as_str().unwrap();
        let response = app
            .clone()
            .oneshot(get(&format!(
                "/api/v1/files/{media}/sidecars?limit=1&cursor={cursor}"
            )))
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"][0]["name"], "film.en.srt");
        assert!(!body.to_string().contains("/secret"));
        let _ = fx.job("fix", &[file]).status("running").create();
        let response = app
            .oneshot(get(&format!("/api/v1/files/{media}")))
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"]["fix_status"], "running");
        assert_eq!(body["data"]["pending_sidecars"], 2);
        assert_eq!(body["data"]["sidecar_count"], 2);
        assert_eq!(body["data"]["inspection_available"], true);
        assert!(!body.to_string().contains("/secret"));
    }
}
