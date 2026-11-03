//! The same timestamped session/CSRF operations as the unchanged Python service.
use atvprep_api::auth::AuthService;
use atvprep_manager::{clock::FixedClock, db::Db, repo::Ctx, schema};
use serde_json::{Value, json};
use std::collections::HashMap;
const BASE: i64 = 1_700_000_000_000_000;
fn text<'a>(step: &'a Value, key: &str, default: &'a str) -> &'a str {
    step[key].as_str().unwrap_or(default)
}

#[test]
#[allow(clippy::too_many_lines)] // Keep recorded operations and their credential aliases in one interpreter.
fn timestamped_auth_lifecycles_match_python_and_only_hashes_are_stored() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../testdata/golden/auth_service.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let mut sessions: HashMap<String, String> = HashMap::new();
        let mut csrf: HashMap<String, String> = HashMap::new();
        let mut observed = Vec::new();
        for step in case["input"].as_array().unwrap() {
            let clock = FixedClock(BASE + step["at"].as_i64().unwrap() * 1_000_000);
            let service = AuthService::with_csrf_ttl(
                Ctx {
                    conn: conn.raw(),
                    clock: &clock,
                },
                60,
                20,
                10,
            )
            .unwrap();
            let alias = text(step, "session", "a");
            let token = sessions
                .get(alias)
                .cloned()
                .unwrap_or_else(|| alias.to_owned());
            let result = (|| -> Result<Value, ()> {
                match text(step, "op", "") {
                    "create" => {
                        let issued = service.create().map_err(|_| ())?;
                        assert_ne!(issued.token, issued.csrf_token);
                        sessions.insert(alias.to_owned(), issued.token);
                        csrf.insert(format!("{alias}:bootstrap"), issued.csrf_token);
                    }
                    "authenticate" => {
                        let id = service.authenticate(&token).map_err(|_| ())?;
                        let last: String = conn
                            .raw()
                            .query_row(
                                "SELECT last_seen_at FROM auth_sessions WHERE id=?1",
                                [id],
                                |row| row.get(0),
                            )
                            .unwrap();
                        let last =
                            chrono::DateTime::parse_from_rfc3339(&(last.replace(' ', "T") + "Z"))
                                .unwrap()
                                .timestamp_micros();
                        return Ok(json!({"ok":true,"last_seen_seconds":(last-BASE)/1_000_000}));
                    }
                    "rotate" => {
                        let issued = service.rotate(&token).map_err(|_| ())?;
                        let alias = text(step, "new_session", "");
                        assert_ne!(token, issued.token);
                        sessions.insert(alias.to_owned(), issued.token);
                        csrf.insert(format!("{alias}:bootstrap"), issued.csrf_token);
                    }
                    "revoke" => service.revoke(&token).map_err(|_| ())?,
                    "issue" => {
                        let issued = service
                            .issue_csrf(
                                &token,
                                text(step, "method", "POST"),
                                text(step, "path", "/api/v1/jobs"),
                                text(step, "action", "create"),
                            )
                            .map_err(|_| ())?;
                        csrf.insert(text(step, "name", "").to_owned(), issued);
                    }
                    "validate" => {
                        service
                            .validate_csrf(
                                &token,
                                csrf.get(text(step, "name", ""))
                                    .map_or("invalid", String::as_str),
                                text(step, "method", "POST"),
                                text(step, "path", "/api/v1/jobs"),
                                text(step, "action", "create"),
                                step["allow_used"].as_bool().unwrap_or(false),
                            )
                            .map_err(|_| ())?;
                    }
                    op => panic!("unknown operation {op}"),
                }
                Ok(json!({"ok":true}))
            })()
            .unwrap_or_else(|()| json!({"ok":false}));
            observed.push(result);
        }
        assert_eq!(json!(observed), case["expected"], "{}", case["name"]);
        for (table, column) in [
            ("auth_sessions", "session_token_hash"),
            ("auth_sessions", "csrf_secret_hash"),
            ("csrf_tokens", "token_hash"),
        ] {
            let mut statement = conn
                .raw()
                .prepare(&format!("SELECT {column} FROM {table}"))
                .unwrap();
            for hash in statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
            {
                let hash = hash.unwrap();
                assert_eq!(hash.len(), 64);
                assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
                assert!(
                    !sessions
                        .values()
                        .chain(csrf.values())
                        .any(|secret| secret == &hash)
                );
            }
        }
    }
}
