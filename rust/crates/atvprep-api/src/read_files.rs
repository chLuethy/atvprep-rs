//! Read-only media sidecar views.
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

/// The states chosen with `status` (OR). Every file is in exactly one of them.
const STATES: [&str; 7] = [
    "clean",
    "embed_open",
    "warning",
    "blocker",
    "unknown",
    "probe_failed",
    "missing",
];

fn status_token(token: &str) -> bool {
    STATES.contains(&token)
}

/// Traits that overlap and are only selected positively (`has`, AND).
const TRAITS: [&str; 5] = [
    "convert",
    "encode_audio",
    "encode_video",
    "embed",
    "converted",
];

fn trait_token(token: &str) -> bool {
    TRAITS.contains(&token)
}

fn known_state(state: &str) -> bool {
    status_token(state) || trait_token(state)
}

/// `status` takes only states, `has` only traits; both are comma-separated and bounded, as is the
/// list of extensions. `hide`, the old way of leaving states out, is refused so that no client
/// silently gets every file.
fn list_input_error(
    hide: Option<&str>,
    status: Option<&str>,
    has: Option<&str>,
    suffix: Option<&str>,
) -> Option<&'static str> {
    if hide.is_some() {
        return Some("hide is no longer accepted, use status");
    }
    if suffix.is_some_and(|value| value.chars().count() > 128) {
        return Some("suffix is too long");
    }
    for (value, allowed, too_long) in [
        (
            status,
            status_token as fn(&str) -> bool,
            "status is too long",
        ),
        (has, trait_token as fn(&str) -> bool, "has is too long"),
    ] {
        let Some(value) = value else { continue };
        if value.chars().count() > 128 {
            return Some(too_long);
        }
        if tokens(Some(value)).iter().any(|token| !allowed(token)) {
            return Some("unknown state");
        }
    }
    None
}

/// The facets the counters vary, as chosen: states (any of them), properties (all of them),
/// libraries (any), extensions (any) and whether a subtitle file lies beside the file.
#[derive(Clone, Default)]
struct Facets {
    status: Vec<String>,
    has: Vec<String>,
    roots: Vec<i64>,
    suffix: Vec<String>,
    sidecar: Option<bool>,
}

/// A facet left out, so that its own counters show what each of its options would show.
#[derive(Clone, Copy)]
enum Facet {
    Status,
    Root,
    Suffix,
    Sidecar,
}

impl Facets {
    /// The choices of a request; the lists were validated with `list_input_error` beforehand.
    fn parse(
        status: Option<&str>,
        has: Option<&str>,
        roots: Option<&str>,
        suffix: Option<&str>,
        sidecar: Option<bool>,
        opaque: &Opaque,
    ) -> Result<Self, (StatusCode, &'static str)> {
        let once = |value: Option<&str>| -> Vec<String> {
            let mut seen: Vec<String> = Vec::new();
            for token in tokens(value) {
                if !seen.iter().any(|known| known == token) {
                    seen.push(token.to_owned());
                }
            }
            seen
        };
        let mut ids = Vec::new();
        for token in tokens(roots) {
            match opaque.decode(token, "r") {
                Ok(id) if id > 0 => ids.push(id),
                _ => return Err((StatusCode::NOT_FOUND, "resource not found")),
            }
        }
        Ok(Self {
            status: once(status),
            has: once(has),
            roots: ids,
            suffix: once(suffix),
            sidecar,
        })
    }

    /// Anything is chosen at all.
    fn any(&self) -> bool {
        !self.status.is_empty()
            || !self.has.is_empty()
            || !self.roots.is_empty()
            || !self.suffix.is_empty()
            || self.sidecar.is_some()
    }

    /// The same choices with one facet cleared.
    fn without(&self, facet: Facet) -> Self {
        let mut next = self.clone();
        match facet {
            Facet::Status => next.status.clear(),
            Facet::Root => next.roots.clear(),
            Facet::Suffix => next.suffix.clear(),
            Facet::Sidecar => next.sidecar = None,
        }
        next
    }

    /// The clauses of the choices (the states as one OR clause, one per property, the libraries
    /// and extensions as one IN each), with their values bound in order.
    fn clauses(&self, open: &str, values: &mut Vec<rusqlite::types::Value>) -> Vec<String> {
        let mut clauses = Vec::new();
        let chosen: Vec<String> = self
            .status
            .iter()
            .map(|token| state_clause(token, open, values))
            .collect();
        if !chosen.is_empty() {
            clauses.push(format!("({})", chosen.join(" OR ")));
        }
        for token in &self.has {
            clauses.push(state_clause(token, open, values));
        }
        if !self.roots.is_empty() {
            clauses.push(format!(
                "m.root_id IN ({})",
                vec!["?"; self.roots.len()].join(",")
            ));
            values.extend(
                self.roots
                    .iter()
                    .map(|id| rusqlite::types::Value::from(*id)),
            );
        }
        if !self.suffix.is_empty() {
            clauses.push(format!(
                "m.suffix IN ({})",
                vec!["?"; self.suffix.len()].join(",")
            ));
            values.extend(
                self.suffix
                    .iter()
                    .cloned()
                    .map(rusqlite::types::Value::from),
            );
        }
        if let Some(sidecar) = self.sidecar {
            clauses.push(format!("{}{SIDECAR}", if sidecar { "" } else { "NOT " }));
        }
        clauses
    }
}

/// A subtitle file lies beside the file.
const SIDECAR: &str = "EXISTS (SELECT 1 FROM sidecars s WHERE s.media_file_id=m.id)";

/// The non-empty entries of a comma-separated list.
fn tokens(value: Option<&str>) -> Vec<&str> {
    value
        .unwrap_or_default()
        .split(',')
        .filter(|token| !token.is_empty())
        .collect()
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
    if below && query.root_id.is_none() {
        return Some("request validation failed");
    }
    if query
        .state
        .as_deref()
        .is_some_and(|state| !known_state(state))
    {
        return Some("unknown state");
    }
    list_input_error(
        query.hide.as_deref(),
        query.status.as_deref(),
        query.has.as_deref(),
        query.suffix.as_deref(),
    )
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
    let Some(open) = embed_open_json(&state).await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        );
    };
    let open_ids: std::collections::BTreeSet<i64> = serde_json::from_str::<Vec<i64>>(&open)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();
    let mut clauses =
        vec!["NOT (m.discovery_state='missing' AND m.dismissed_at IS NOT NULL)".to_owned()];
    if let Some(value) = query.state.as_deref() {
        clauses.push(state_clause(value, &open, &mut values));
    }
    // The library facet is the column itself here: the page leaves out the libraries not chosen.
    let selection = match Facets::parse(
        query.status.as_deref(),
        query.has.as_deref(),
        None,
        query.suffix.as_deref(),
        query.has_sidecar,
        &opaque,
    ) {
        Ok(selection) => selection,
        Err((code, message)) => return error(code, message, PATH),
    };
    let filtered = query.state.is_some() || selection.any();
    clauses.extend(selection.clauses(&open, &mut values));
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
    let result=tokio::task::spawn_blocking(move || {
        use std::collections::BTreeMap;
        let conn=state.db.conn().map_err(|_|())?;let conn=conn.raw();
        if let Some(root)=root {let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM library_roots WHERE id=?)",[root],|row|row.get(0)).map_err(|_|())?;if !exists{return Ok(None);}}
        let filter=clauses.join(" AND ");
        let mut stmt=conn.prepare(&format!("SELECT m.id,m.root_id,m.relative_path,m.discovery_state,m.compliance_state FROM media_files m WHERE {filter} ORDER BY lower(m.relative_path),m.id")).map_err(|_|())?;
        let rows=stmt.query_map(rusqlite::params_from_iter(values.iter()),|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?))).map_err(|_|())?;
        let mut counts:BTreeMap<String,BTreeMap<String,u64>>=BTreeMap::new();let mut ids=Vec::new();let mut extra=false;
        for row in rows {let(id,root_id,path,discovery,compliance)=row.map_err(|_|())?;let key=if matches!(query.state.as_deref(),Some("convert"|"converted"|"encode_audio"|"encode_video"|"embed")){query.state.clone().unwrap_or_default()}else if discovery=="missing" {"missing".to_owned()}else if compliance=="clean" && open_ids.contains(&id) {"embed_open".to_owned()}else{compliance};
            let group=if root.is_none(){Some(root_id.to_string())}else if below{None}else{let rest=if folder.is_empty(){path.as_str()}else{path.strip_prefix(&format!("{folder}/")).unwrap_or("")};rest.split_once('/').map(|(name,_)|name.to_owned())};
            if let Some(group)=group {*counts.entry(group).or_default().entry(key).or_default()+=1;}else if root.is_some(){if ids.len()<500 {ids.push(id);}else{extra=true;}}
        }
        let mut folders=Vec::new();
        if root.is_none(){let mut roots=conn.prepare("SELECT id,name FROM library_roots ORDER BY id").map_err(|_|())?;let rows=roots.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).map_err(|_|())?;for row in rows{let(id,name)=row.map_err(|_|())?;let tally=counts.remove(&id.to_string()).unwrap_or_default();if filtered && tally.is_empty(){continue;}let total:u64=tally.values().sum();folders.push(json!({"name":crate::read_jobs::safe_head(&name,2048),"id":opaque.encode("r",id),"counts":tally,"total":total}));}}
        else if !below {let mut entries:Vec<_>=counts.into_iter().collect();entries.sort_by_cached_key(|(name,_)|name.to_lowercase());for(name,tally) in entries{let total:u64=tally.values().sum();folders.push(json!({"name":crate::read_jobs::safe_head(&name,2048),"id":null,"counts":tally,"total":total}));}}
        let mut files=Vec::new();let mut used=serde_json::to_vec(&folders).map_err(|_|())?.len();let mut truncated=extra;
        for id in ids {let Some(mut value)=load_file(conn,&opaque,id)? else {continue};if let Some(object)=value.as_object_mut(){for key in ["status_version","last_success_at","has_error","sidecar_count","inspection_available"]{object.remove(key);}}
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

/// The facet counters for the list with the same filters as `/files`: each state with every filter
/// but the states, each property with every filter plus that property, each library, extension and
/// "has a subtitle file" with every filter but that facet's own choice, the files that match all
/// filters and all files there are.
#[allow(clippy::too_many_lines)] // One counter per facet, all over the same filter.
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
    let Some(open) = embed_open_json(&state).await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        );
    };
    let (base, base_values) = match base_filter(&query, &open) {
        Ok(filter) => filter,
        Err((code, message)) => return error(code, message, PATH),
    };
    let chosen = match facets_of(&query, &opaque) {
        Ok(chosen) => chosen,
        Err((code, message)) => return error(code, message, PATH),
    };
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        let conn = conn.raw();
        // Files matching every filter and `facets`, counted per value of `key` (an SQL expression).
        let grouped = |facets: &Facets, key: &str| -> Result<Vec<(rusqlite::types::Value, i64)>, ()> {
            let mut clauses = base.clone();
            let mut values = base_values.clone();
            clauses.extend(facets.clauses(&open, &mut values));
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {key}, count(*) FROM media_files m WHERE {} GROUP BY 1",
                    clauses.join(" AND ")
                ))
                .map_err(|_| ())?;
            stmt.query_map(rusqlite::params_from_iter(values.iter()), |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|_| ())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ())
        };
        let count = |facets: &Facets| -> Result<i64, ()> {
            Ok(grouped(facets, "1")?.first().map_or(0, |(_, n)| *n))
        };
        let mut by_status = serde_json::Map::new();
        for token in STATES {
            let mut with = chosen.without(Facet::Status);
            with.status = vec![token.to_owned()];
            by_status.insert(token.to_owned(), json!(count(&with)?));
        }
        let mut by_trait = serde_json::Map::new();
        for token in TRAITS {
            let mut with = chosen.clone();
            if !with.has.iter().any(|known| known == token) {
                with.has.push(token.to_owned());
            }
            by_trait.insert(token.to_owned(), json!(count(&with)?));
        }
        // Every library and every extension there is, so an option without a hit still shows with 0.
        let tally = |facet: Facet, key: &str| -> Result<std::collections::HashMap<String, i64>, ()> {
            Ok(grouped(&chosen.without(facet), key)?
                .into_iter()
                .map(|(value, n)| (sql_text(value), n))
                .collect())
        };
        let roots = tally(Facet::Root, "m.root_id")?;
        let mut by_root = serde_json::Map::new();
        let mut stmt = conn
            .prepare("SELECT id FROM library_roots ORDER BY id")
            .map_err(|_| ())?;
        for id in stmt
            .query_map([], |row| row.get::<_, i64>(0))
            .map_err(|_| ())?
        {
            let id = id.map_err(|_| ())?;
            let n = roots.get(&id.to_string()).copied().unwrap_or(0);
            by_root.insert(opaque.encode("r", id), json!(n));
        }
        let suffixes = tally(Facet::Suffix, "m.suffix")?;
        let mut known: Vec<String> = conn
            .prepare(&format!(
                "SELECT DISTINCT m.suffix FROM media_files m WHERE {LISTED} ORDER BY m.suffix LIMIT {SUFFIX_OPTIONS}"
            ))
            .map_err(|_| ())?
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| ())?
            .collect::<Result<_, _>>()
            .map_err(|_| ())?;
        known.extend(chosen.suffix.iter().cloned());
        let mut by_suffix = serde_json::Map::new();
        for suffix in known {
            let n = suffixes.get(&suffix).copied().unwrap_or(0);
            by_suffix.insert(crate::read_jobs::safe_head(&suffix, 256), json!(n));
        }
        let sidecars = tally(Facet::Sidecar, SIDECAR)?;
        let by_sidecar = json!({
            "true": sidecars.get("1").copied().unwrap_or(0),
            "false": sidecars.get("0").copied().unwrap_or(0),
        });
        let matched = count(&chosen)?;
        let total: i64 = conn
            .query_row(
                &format!("SELECT count(*) FROM media_files m WHERE {LISTED}"),
                [],
                |row| row.get(0),
            )
            .map_err(|_| ())?;
        Ok::<_, ()>(json!({
            "data": {
                "status": by_status,
                "has": by_trait,
                "root_id": by_root,
                "suffix": by_suffix,
                "has_sidecar": by_sidecar,
                "matched": matched,
                "total": total
            },
            "meta": {}
        }))
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

/// "Ready" is a clean file with nothing left to embed and `embed_open` a clean one with an open
/// embedding; the trait "embed" is any file with an open embedding, whatever its state.
/// The states split the files without overlap: a missing file is only "missing".
fn state_clause(state: &str, open: &str, values: &mut Vec<rusqlite::types::Value>) -> String {
    match state {
        "clean" => {
            values.push(open.to_owned().into());
            format!(
                "(m.discovery_state<>'missing' AND m.compliance_state='clean' AND {})",
                media_sql::embed_not_open("?")
            )
        }
        "embed_open" => {
            values.push(open.to_owned().into());
            format!(
                "(m.discovery_state<>'missing' AND m.compliance_state='clean' AND {})",
                media_sql::embed_open("?")
            )
        }
        "embed" => {
            values.push(open.to_owned().into());
            format!(
                "(m.discovery_state='present' AND {})",
                media_sql::embed_open("?")
            )
        }
        "missing" => "m.discovery_state='missing'".to_owned(),
        "convert" => format!("(m.discovery_state='present' AND {TO_CONVERT})"),
        "converted" => format!("(m.discovery_state='present' AND {CONVERTED})"),
        "encode_audio" => format!("(m.discovery_state='present' AND {NEEDS_AUDIO})"),
        "encode_video" => format!("(m.discovery_state='present' AND {NEEDS_VIDEO})"),
        _ => {
            values.push(state.to_owned().into());
            "(m.discovery_state<>'missing' AND m.compliance_state=?)".to_owned()
        }
    }
}

/// Where a file's state lands in the status sort, given a bound JSON list of the open ids: blocker,
/// inspection failed, missing, warning, embed open, not verified, ready.
fn status_rank(param: &str) -> String {
    format!(
        "(CASE WHEN m.discovery_state='missing' THEN 2 \
         WHEN m.compliance_state='clean' AND {} THEN 4 \
         ELSE CASE m.compliance_state WHEN 'blocker' THEN 0 WHEN 'probe_failed' THEN 1 \
         WHEN 'warning' THEN 3 WHEN 'unknown' THEN 5 WHEN 'clean' THEN 6 ELSE 9 END END)",
        media_sql::embed_open(param)
    )
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
const LISTED: &str = "NOT (m.discovery_state='missing' AND m.dismissed_at IS NOT NULL)";

/// Bounds and token checks shared by the list and its counters.
fn filter_input_error(query: &FileQuery, path: &str) -> Option<Response> {
    // Several libraries make a long list of ids.
    if query
        .root_id
        .as_ref()
        .is_some_and(|value| value.chars().count() > 1024)
    {
        return Some(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "root_id is too long",
            path,
        ));
    }
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
            .is_some_and(|value| value.chars().count() > 128)
        {
            return Some(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("{name} is too long"),
                path,
            ));
        }
    }
    list_input_error(
        query.hide.as_deref(),
        query.status.as_deref(),
        query.has.as_deref(),
        query.suffix.as_deref(),
    )
    .map(|message| error(StatusCode::UNPROCESSABLE_ENTITY, message, path))
}

/// SQL clauses joined with AND, and the values bound to them in order.
type Filter = (Vec<String>, Vec<rusqlite::types::Value>);

/// How many extensions the counters offer at most.
const SUFFIX_OPTIONS: usize = 200;

/// A grouping key read back from SQL, as the text the counters key it by.
fn sql_text(value: rusqlite::types::Value) -> String {
    match value {
        rusqlite::types::Value::Integer(n) => n.to_string(),
        rusqlite::types::Value::Text(text) => text,
        _ => String::new(),
    }
}

/// The facet choices of a list request.
fn facets_of(query: &FileQuery, opaque: &Opaque) -> Result<Facets, (StatusCode, &'static str)> {
    Facets::parse(
        query.status.as_deref(),
        query.has.as_deref(),
        query.root_id.as_deref(),
        query.suffix.as_deref(),
        query.has_sidecar,
        opaque,
    )
}

/// Every filter of the list except the facets (`Facets`), which the counters vary.
fn base_filter(query: &FileQuery, open: &str) -> Result<Filter, (StatusCode, &'static str)> {
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
        clauses.push(state_clause(value, open, &mut values));
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
    if !(1..=100).contains(&limit)
        || !matches!(
            query.sort.as_deref(),
            None | Some("title" | "status" | "size")
        )
        || !matches!(query.dir.as_deref(), None | Some("asc" | "desc"))
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
    let Some(open) = embed_open_json(&state).await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        );
    };
    let (mut clauses, mut values) = match base_filter(&query, &open) {
        Ok(filter) => filter,
        Err((code, message)) => return error(code, message, PATH),
    };
    match facets_of(&query, &opaque) {
        Ok(chosen) => clauses.extend(chosen.clauses(&open, &mut values)),
        Err((code, message)) => return error(code, message, PATH),
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
            let order=match query.sort.as_deref() {Some("size")=>format!("m.size {} ,lower(m.relative_path),m.id",if descending {"DESC"} else {"ASC"}),Some("status")=>format!("{} {},lower(m.relative_path),m.id",status_rank("?"),if descending {"DESC"} else {"ASC"}),_=>if descending {"m.id DESC".to_owned()} else {"m.id".to_owned()}};
            let offset=if query.sort.is_some(){after}else{0};
            let mut stmt=conn.prepare(&format!("SELECT m.id FROM media_files m WHERE {filter} ORDER BY {order} LIMIT ? OFFSET ?")).map_err(|_|())?;
            let mut bound=values.clone();if query.sort.as_deref()==Some("status") {bound.push(open.clone().into());}bound.push(i64::try_from(limit+1).map_err(|_|())?.into());bound.push(offset.into());
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
