//! Authenticated runtime and structural settings views.
use crate::{
    auth::AuthService,
    extract::AuthenticatedSession,
    mutation::{self, MutationError, MutationRequest},
    opaque::Opaque,
    read::{error, page_meta},
    read_files::inspection_value,
    read_jobs::{safe_head, subject},
    routes_auth,
    state::AppState,
};
use atvprep_manager::{
    repo::Ctx,
    settings::{Setting, Settings, default_of},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const KEYS: [&str; 9] = [
    "delete_original",
    "event_retention_days",
    "job_retention_days",
    "log_retention_days",
    "max_ffmpeg_workers",
    "max_probe_workers",
    "scan_exclude_dirs",
    "scan_interval_seconds",
    "settle_seconds",
];

fn bounded(value: Value) -> Value {
    if crate::bounds::value_needs_bound(&value) {
        json!({"truncated":true})
    } else {
        value
    }
}

fn safe_argument(value: &str) -> String {
    if value.starts_with('/') || value.starts_with('\\') || value.as_bytes().get(1) == Some(&b':') {
        "<path>".to_owned()
    } else {
        safe_head(value, 2048)
    }
}

fn safe_arguments(values: &[String]) -> Value {
    let mut redact_next = false;
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        if redact_next {
            result.push("<redacted>".to_owned());
            redact_next = false;
            continue;
        }
        let lower = value.to_ascii_lowercase();
        let sensitive = [
            "password",
            "secret",
            "token",
            "credential",
            "api-key",
            "api_key",
            "authorization",
            "cookie",
        ]
        .iter()
        .any(|part| lower.contains(part));
        if sensitive && value.starts_with('-') && !value.contains('=') {
            redact_next = true;
        }
        result.push(safe_argument(value));
    }
    json!(result)
}

fn structural(state: &AppState, revision: i64) -> Vec<Value> {
    let config = &state.config;
    let path = |value: bool| if value { json!("<path>") } else { Value::Null };
    let fields = [
        (
            "library_roots",
            json!(
                config
                    .library_roots
                    .iter()
                    .map(|_| "<path>")
                    .collect::<Vec<_>>()
            ),
        ),
        ("state_dir", json!("<path>")),
        ("database_path", json!("<path>")),
        ("bind_host", json!(safe_head(&config.bind_host, 2048))),
        ("bind_port", json!(config.bind_port)),
        ("atvprep_command", safe_arguments(&config.atvprep_command)),
        ("fix_extra_args", safe_arguments(&config.fix_extra_args)),
        (
            "convert_extra_args",
            safe_arguments(&config.convert_extra_args),
        ),
        ("quarantine_dir", path(config.quarantine_dir.is_some())),
        ("log_tail_bytes", json!(config.log_tail_bytes)),
        (
            "session_absolute_ttl_seconds",
            json!(config.session_absolute_ttl_seconds),
        ),
        (
            "session_idle_ttl_seconds",
            json!(config.session_idle_ttl_seconds),
        ),
        ("csrf_ttl_seconds", json!(config.csrf_ttl_seconds)),
        ("session_cookie_secure", json!(config.session_cookie_secure)),
    ];
    fields.into_iter().map(|(key,value)|json!({"key":key,"value":bounded(value),"revision":0,"default_value":null,
        "source":config.config_source,"last_changed_revision":null,"effective_revision":revision,"read_only":true,"restart_required":true,
        "guidance":"Configured outside the runtime settings page; restart the manager after deployment/config changes."})).collect()
}

fn runtime_value(key: &str, value: Value) -> Value {
    let value = bounded(value);
    if key == "scan_exclude_dirs"
        && let Value::Array(items) = value
    {
        Value::Array(
            items
                .into_iter()
                .map(|item| match item {
                    Value::String(value) => json!(subject(&value)),
                    other => other,
                })
                .collect(),
        )
    } else {
        value
    }
}

fn setting_response(setting: Setting, effective_revision: i64) -> Value {
    json!({"key":setting.key,"value":runtime_value(&setting.key,setting.value),"revision":setting.revision,
        "default_value":default_of(&setting.key).map(bounded),"source":"db_override","last_changed_revision":setting.revision,
        "effective_revision":effective_revision,"read_only":false,"restart_required":false,
        "guidance":"Editable runtime override; changes are audited and take effect without restart."})
}

#[derive(Deserialize, Serialize)]
pub struct SettingUpdate {
    value: Value,
    expected_revision: Option<i64>,
}
#[derive(Deserialize, Serialize)]
pub struct SettingsPatch {
    settings: IndexMap<String, Value>,
    expected_revision: Option<i64>,
}

#[derive(Deserialize)]
pub struct AuditQuery {
    key: Option<String>,
    #[serde(default, deserialize_with = "crate::opaque::bounded_cursor")]
    cursor: Option<String>,
    limit: Option<usize>,
}

fn audit_value(value: &Value) -> Value {
    let value = inspection_value(value, 0);
    if serde_json::to_vec(&value).map_or(true, |encoded| encoded.len() > 8192) {
        json!({"truncated":true})
    } else {
        value
    }
}

/// List bounded, redacted setting changes in revision order.
pub async fn audit(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<AuditQuery>,
) -> Response {
    const PATH: &str = "/api/v1/settings/audit";
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let after = match query.cursor.as_deref() {
        Some(value) => match opaque.decode(value, "cursor:settings-audit") {
            Ok(id) if id > 0 => id,
            _ => return error(StatusCode::BAD_REQUEST, "invalid cursor", PATH),
        },
        None => 0,
    };
    let result=tokio::task::spawn_blocking(move ||{
        let conn=state.db.conn().map_err(|_|())?;
        let mut stmt=conn.raw().prepare("SELECT id,key,revision,actor,old_value_json,new_value_json FROM settings_audit WHERE id>?1 AND (?2 IS NULL OR key=?2) ORDER BY revision,id LIMIT ?3").map_err(|_|())?;
        let rows=stmt.query_map(rusqlite::params![after,query.key.filter(|key|!key.is_empty()),i64::try_from(limit+1).map_err(|_|())?],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?,row.get::<_,String>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,String>(5)?))).map_err(|_|())?;
        let mut data=Vec::new();let mut last=None;let mut used=0usize;let mut more=false;let mut bounded_page=false;let mut source_truncated=false;
        for row in rows {
            let (id,key,revision,actor,old,new)=row.map_err(|_|())?;if data.len()==limit{more=true;break;}
            let old:Value=old.and_then(|value|serde_json::from_str(&value).ok()).unwrap_or(Value::Null);
            let new:Value=serde_json::from_str(&new).unwrap_or(Value::Null);
            source_truncated|=crate::bounds::value_needs_bound(&old)||crate::bounds::value_needs_bound(&new)||actor.len()>2048;
            let value=json!({"key":safe_head(&key,2048),"revision":revision,"actor":safe_head(&actor,2048),"old_value":audit_value(&old),"new_value":audit_value(&new)});
            let size=serde_json::to_vec(&value).map_err(|_|())?.len();if size>crate::bounds::MAX_ELEMENT_BYTES || used.saturating_add(size).saturating_add(data.len()+256)>crate::bounds::MAX_RESPONSE_BYTES{more=true;bounded_page=true;break;}
            used+=size;last=Some(id);data.push(value);
        }
        let next=if more{last.map(|id|opaque.encode("cursor:settings-audit",id))}else{None};
        Ok::<_,()>(json!({"data":data,"meta":page_meta(Some(limit),next.as_deref(),bounded_page||source_truncated)}))
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

fn expected_revision(headers: &HeaderMap, body: Option<i64>) -> Result<Option<i64>, MutationError> {
    let header = headers
        .get(header::IF_MATCH)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| MutationError::InvalidInput("invalid If-Match revision"))
                .and_then(|value| {
                    value
                        .trim()
                        .trim_matches('"')
                        .parse::<i64>()
                        .map_err(|_| MutationError::InvalidInput("invalid If-Match revision"))
                })
        })
        .transpose()?;
    if header.is_some() && body.is_some() && header != body {
        return Err(MutationError::Rejected("settings revision conflict"));
    }
    Ok(body.or(header))
}

fn settings_error(error: atvprep_manager::Error) -> MutationError {
    match error {
        atvprep_manager::Error::Invalid(message) if message.contains("revision conflict") => {
            MutationError::Rejected("settings revision conflict")
        }
        atvprep_manager::Error::Invalid(message) => MutationError::Validation(message),
        atvprep_manager::Error::Constraint(_) => {
            MutationError::Rejected("settings changed concurrently")
        }
        _ => MutationError::Database,
    }
}

async fn mutate(
    state: AppState,
    headers: HeaderMap,
    method: &'static str,
    path: String,
    body: Value,
    work: impl FnOnce(Ctx<'_>, i64) -> Result<(u16, Value), MutationError> + Send + 'static,
) -> Response {
    let token =
        routes_auth::cookie(&headers, state.config.session_cookie_secure).unwrap_or_default();
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let origin_allowed = routes_auth::allowed(&headers, &state);
    let error_path = path.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| MutationError::Database)?;
        let ctx = Ctx {
            conn: conn.raw(),
            clock: state.clock.as_ref(),
        };
        let absolute = i64::try_from(state.config.session_absolute_ttl_seconds).unwrap_or(0);
        let idle = i64::try_from(state.config.session_idle_ttl_seconds).unwrap_or(0);
        let csrf_ttl = i64::try_from(state.config.csrf_ttl_seconds).unwrap_or(0);
        let request = MutationRequest {
            token: &token,
            csrf: &csrf,
            origin_allowed,
            requires_open_gate: false,
            method,
            path: &path,
            action: "settings",
            idempotency_key: &key,
            body: &body,
            absolute_ttl: absolute,
            idle_ttl: idle,
            csrf_ttl,
        };
        mutation::execute(ctx, &request, |tx| {
            let id = AuthService::with_csrf_ttl(tx, absolute, idle, csrf_ttl)
                .map_err(|_| MutationError::Database)?
                .authenticate(&token)
                .map_err(|_| MutationError::Unauthorized)?;
            work(tx, id)
        })
    })
    .await;
    match result {
        Ok(Ok((status, body))) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(body),
        )
            .into_response(),
        Ok(Err(error)) => error.response(&error_path),
        Err(_) => MutationError::Database.response(&error_path),
    }
}

/// Update multiple runtime settings under one revision.
pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut payload): Json<SettingsPatch>,
) -> Response {
    const PATH: &str = "/api/v1/settings";
    if payload.settings.keys().any(|key| default_of(key).is_none()) {
        return MutationError::InvalidInput("setting is not allowlisted").response(PATH);
    }
    payload.expected_revision = match expected_revision(&headers, payload.expected_revision) {
        Ok(value) => value,
        Err(error) => return error.response(PATH),
    };
    let body = serde_json::to_value(&payload).unwrap_or(Value::Null);
    mutate(
        state,
        headers,
        "PATCH",
        PATH.to_owned(),
        body,
        move |ctx, id| {
            let repo = Settings(ctx);
            let entries: Vec<_> = payload.settings.into_iter().collect();
            let refs: Vec<_> = entries
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect();
            let rows = repo
                .set_many(&refs, &format!("session:{id}"), payload.expected_revision)
                .map_err(settings_error)?;
            let revision = rows.first().map_or(0, |row| row.revision);
            let rows: Vec<_> = rows
                .into_iter()
                .map(|row| setting_response(row, revision))
                .collect();
            Ok((200, json!({"data":{"revision":revision,"settings":rows}})))
        },
    )
    .await
}

/// Update one allowlisted runtime setting.
pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Json(mut payload): Json<SettingUpdate>,
) -> Response {
    let path = format!("/api/v1/settings/{key}");
    if default_of(&key).is_none() {
        return MutationError::InvalidInput("setting is not allowlisted").response(&path);
    }
    payload.expected_revision = match expected_revision(&headers, payload.expected_revision) {
        Ok(value) => value,
        Err(error) => return error.response(&path),
    };
    let body =
        json!({"key":key,"value":payload.value,"expected_revision":payload.expected_revision});
    mutate(state, headers, "PUT", path, body, move |ctx, id| {
        let setting = Settings(ctx)
            .set(
                &key,
                &payload.value,
                &format!("session:{id}"),
                payload.expected_revision,
            )
            .map_err(settings_error)?;
        let revision = setting.revision;
        Ok((200, json!({"data":setting_response(setting,revision)})))
    })
    .await
}

/// List editable settings and redacted structural configuration.
pub async fn list(State(state): State<AppState>, _session: AuthenticatedSession) -> Response {
    const PATH: &str = "/api/v1/settings";
    let result=tokio::task::spawn_blocking(move ||{
        let conn=state.db.conn().map_err(|_|())?;
        let repo=Settings(Ctx{conn:conn.raw(),clock:state.clock.as_ref()});
        let revision=repo.current_revision().map_err(|_|())?;
        let mut items=Vec::new();
        for key in KEYS {
            let default=default_of(key).ok_or(())?;
            let setting=repo.get(key).map_err(|_|())?;
            let item=if let Some(setting)=setting {json!({"key":key,"value":runtime_value(key,setting.value),"revision":setting.revision,"default_value":bounded(default),"source":"db_override","last_changed_revision":setting.revision,"effective_revision":revision,"read_only":false,"restart_required":false,"guidance":"Editable runtime override; changes are audited and take effect without restart."})}
                else{json!({"key":key,"value":runtime_value(key,default.clone()),"revision":0,"default_value":bounded(default),"source":"default","last_changed_revision":null,"effective_revision":revision})};
            items.push(item);
        }
        items.extend(structural(&state,revision));
        let mut truncated=items.iter().any(|item|item["value"]==json!({"truncated":true}));
        let mut body=json!({"data":{"revision":revision,"settings":items,"truncated":truncated}});
        if serde_json::to_vec(&body).map_or(true,|data|data.len()>crate::bounds::MAX_RESPONSE_BYTES){
            truncated=true;
            if let Some(settings)=body["data"]["settings"].as_array_mut(){for item in settings {item["value"]=json!({"truncated":true});item["default_value"]=Value::Null;}}
            body["data"]["truncated"]=json!(truncated);
        }
        Ok::<_,()>(body)
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

#[cfg(test)]
mod tests {
    use super::{bounded, safe_arguments};
    use serde_json::json;

    #[test]
    fn settings_values_are_bounded_and_sensitive_arguments_are_hidden() {
        assert_eq!(bounded(json!(vec!["entry"; 65])), json!({"truncated":true}));
        let arguments = safe_arguments(&[
            "--token".to_owned(),
            "private-value".to_owned(),
            "/private/file".to_owned(),
        ]);
        assert!(!arguments.to_string().contains("private-value"));
        assert!(!arguments.to_string().contains("/private"));
    }
}
