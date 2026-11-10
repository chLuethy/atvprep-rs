pub mod normalise;
pub mod seed;
use atvprep_api::{
    auth::AuthService, config::ManagerConfig, opaque::Opaque, router, state::AppState,
};
use atvprep_manager::{db::Db, repo::Ctx, schema};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use tower::ServiceExt;

const SECRET: &str = "test-api-secret";
pub struct Contract {
    state: AppState,
    app: axum::Router,
    session: String,
    proofs: std::collections::HashMap<String, String>,
}
impl Contract {
    pub fn new() -> Self {
        Self::with_seed("seed")
    }
    pub fn with_seed(name: &str) -> Self {
        Self::with_database(Db::memory().unwrap(), name)
    }
    pub fn with_database(db: Db, name: &str) -> Self {
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        seed::apply_named(conn.raw(), name);
        let state = AppState::new(
            db,
            ManagerConfig {
                api_secret: Some(SECRET.to_owned()),
                allowed_origins: vec!["http://testserver".to_owned()],
                ..ManagerConfig::default()
            },
        )
        .unwrap();
        let session = AuthService::new(
            Ctx {
                conn: conn.raw(),
                clock: state.clock.as_ref(),
            },
            28_800,
            3600,
        )
        .unwrap()
        .create()
        .unwrap()
        .token;
        Self {
            app: router(state.clone()),
            state,
            session,
            proofs: std::collections::HashMap::new(),
        }
    }
    pub async fn assert_parallel_settings_have_one_winner(&mut self) {
        let path = "/api/v1/settings/max_probe_workers";
        let first = self.proof(&json!({"csrf_alias":"race-a"}), "PUT", path, "settings");
        let second = self.proof(&json!({"csrf_alias":"race-b"}), "PUT", path, "settings");
        let request = |value, proof, key| {
            Request::put(path)
                .header("origin", "http://testserver")
                .header("cookie", format!("__Host-atvprep_session={}", self.session))
                .header("x-csrf-token", proof)
                .header("idempotency-key", key)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"value":value,"expected_revision":1}).to_string(),
                ))
                .unwrap()
        };
        let (first, second) = tokio::join!(
            self.app.clone().oneshot(request(4, first, "race-a")),
            self.app.clone().oneshot(request(5, second, "race-b"))
        );
        let mut statuses = [
            first.unwrap().status().as_u16(),
            second.unwrap().status().as_u16(),
        ];
        statuses.sort_unstable();
        assert_eq!(statuses, [200, 409]);
        let conn = self.state.db.conn().unwrap();
        let counts:(i64,i64,i64,i64)=conn.raw().query_row("SELECT (SELECT revision FROM settings_revision WHERE id=1),(SELECT count(*) FROM settings_audit),(SELECT count(*) FROM idempotency_records),(SELECT count(*) FROM csrf_tokens WHERE used_at IS NOT NULL)",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
        assert_eq!(counts, (2, 1, 1, 1));
    }

    pub fn seed_failed_prepare_and_change_approver(&mut self) {
        use atvprep_manager::jobs::{Jobs, NewJob};
        let conn = self.state.db.conn().unwrap();
        let ctx = Ctx {
            conn: conn.raw(),
            clock: self.state.clock.as_ref(),
        };
        let mut new = NewJob::new("fix", &[1]);
        new.settings_snapshot = json!({"pipeline":"prepare","policy":atvprep_manager::approval::PREPARE_POLICY,"delete_original":false,"delete_original_confirmed":false,"manual_approval":true,"approved_by":1,"approved_at":0});
        let job = Jobs(ctx).create(&new).unwrap();
        assert_eq!(job.id, 2);
        ctx.conn
            .execute(
                "UPDATE jobs SET status='failed',failed_count=1 WHERE id=?1",
                [job.id],
            )
            .unwrap();
        ctx.conn
            .execute(
                "UPDATE job_items SET status='failed' WHERE job_id=?1",
                [job.id],
            )
            .unwrap();
        let issued = AuthService::new(ctx, 28_800, 3600)
            .unwrap()
            .create()
            .unwrap();
        assert_eq!(issued.id, 2);
        self.session = issued.token;
    }

    pub fn assert_prepare_approvals(&self, expected: usize) {
        use atvprep_manager::{
            approval::{ApprovalItem, ApprovalVerifier, prepare_policy_block},
            jobs::Jobs,
            snapshots::InputSnapshots,
        };
        let conn = self.state.db.conn().unwrap();
        let ctx = Ctx {
            conn: conn.raw(),
            clock: self.state.clock.as_ref(),
        };
        let verifier = ApprovalVerifier::new(self.state.approval_secret.as_bytes()).unwrap();
        let mut statement=conn.raw().prepare("SELECT id FROM jobs WHERE json_extract(settings_snapshot_json,'$.pipeline')='prepare' AND status!='failed'").unwrap();
        let ids: Vec<i64> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let person = AuthService::new(ctx, 28_800, 3600)
            .unwrap()
            .authenticate(&self.session)
            .unwrap();
        let mut checked = 0;
        for job in ids {
            for item in Jobs(ctx).items(job).unwrap() {
                let snapshot = InputSnapshots(ctx).get_for_item(item.id).unwrap().unwrap();
                let identity = [
                    snapshot["file_device"].clone(),
                    snapshot["file_inode"].clone(),
                    snapshot["file_ctime_ns"].clone(),
                    snapshot["sidecar_fingerprint"].clone(),
                ];
                let approval = ApprovalItem {
                    job_id: job,
                    media_id: item.media_file_id,
                    action: &item.action,
                    identity: &identity,
                    encode: atvprep_manager::approval::item_encode(&item.settings_snapshot)
                        .unwrap(),
                    delete_original: item.settings_snapshot["delete_original"] == json!(true),
                };
                assert_eq!(prepare_policy_block(&item.settings_snapshot), None);
                assert_eq!(item.settings_snapshot["approved_by"], person);
                assert!(item.settings_snapshot["approved_at"].as_i64().unwrap() > 0);
                assert_eq!(
                    verifier.check(
                        &item.settings_snapshot,
                        &approval,
                        ctx.clock.now_micros() / 1_000_000
                    ),
                    None
                );
                let foreign = ApprovalItem {
                    job_id: job + 100,
                    ..approval
                };
                assert!(
                    verifier
                        .check(
                            &item.settings_snapshot,
                            &foreign,
                            ctx.clock.now_micros() / 1_000_000
                        )
                        .is_some()
                );
                let mut changed = identity.clone();
                changed[1] = json!(999);
                assert!(
                    verifier
                        .check(
                            &item.settings_snapshot,
                            &ApprovalItem {
                                identity: &changed,
                                ..approval
                            },
                            ctx.clock.now_micros() / 1_000_000
                        )
                        .is_some()
                );
                assert_eq!(item.settings_snapshot["delete_original"], false);
                checked += 1;
            }
        }
        assert_eq!(checked, expected);
    }

    pub async fn assert_missing_media_is_rejected_atomically(&mut self) {
        let opaque = Opaque::new(SECRET);
        let path = "/api/v1/jobs";
        let proof = self.proof(&json!({}), "POST", path, "create-job");
        let response = self
            .app
            .clone()
            .oneshot(
                Request::post(path)
                    .header("origin", "http://testserver")
                    .header("cookie", format!("__Host-atvprep_session={}", self.session))
                    .header("x-csrf-token", proof)
                    .header("idempotency-key", "missing-media")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"action":"verify","media_ids":[opaque.encode("m",999)]}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        let conn = self.state.db.conn().unwrap();
        let counts:(i64,i64,i64,i64)=conn.raw().query_row("SELECT (SELECT count(*) FROM jobs),(SELECT count(*) FROM job_items),(SELECT count(*) FROM events),(SELECT count(*) FROM idempotency_records)",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
        assert_eq!(counts, (1, 1, 1, 0));
    }

    fn observe_cookie(&mut self, cookie: &str) -> Value {
        let (name, credential) = cookie.split(';').next().unwrap().split_once('=').unwrap();
        if name == "__Host-atvprep_session" {
            credential.trim_matches('"').clone_into(&mut self.session);
        }
        normalise::cookie_view(cookie)
    }

    fn proof(&mut self, input: &Value, method: &str, path: &str, action: &str) -> String {
        let alias = input["csrf_alias"].as_str();
        if let Some(proof) = alias.and_then(|alias| self.proofs.get(alias)) {
            return proof.clone();
        }
        let conn = self.state.db.conn().unwrap();
        let proof = AuthService::new(
            Ctx {
                conn: conn.raw(),
                clock: self.state.clock.as_ref(),
            },
            28_800,
            3600,
        )
        .unwrap()
        .issue_csrf(&self.session, method, path, action)
        .unwrap();
        if let Some(alias) = alias {
            self.proofs.insert(alias.to_owned(), proof.clone());
        }
        proof
    }

    pub async fn replay(&mut self, case: &Value) {
        let opaque = Opaque::new(SECRET);
        for (index, step) in case["input"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let input = &step["request"];
            let method = input["method"].as_str().unwrap();
            let path = normalise::render(input["path"].as_str().unwrap(), &opaque);
            let mut request = Request::builder().method(method).uri(&path);
            if let Some(origin) = input["origin"].as_str() {
                request = request.header("origin", origin);
            }
            if let Some(site) = input["fetch_site"].as_str() {
                request = request.header("sec-fetch-site", site);
            }
            if let Some(scheme) = input["authorization"]["scheme"].as_str() {
                let credential = if input["authorization"]["credential"] == "<master>" {
                    SECRET
                } else {
                    "invalid"
                };
                request = request.header("authorization", format!("{scheme} {credential}"));
            }
            if input["authenticated"].as_bool() == Some(true) {
                request =
                    request.header("cookie", format!("__Host-atvprep_session={}", self.session));
            }
            if input["csrf_invalid"].as_bool() == Some(true) {
                request = request.header("x-csrf-token", "invalid");
            } else if let Some(action) = input["csrf_action"].as_str() {
                request = request.header("x-csrf-token", self.proof(input, method, &path, action));
            }
            if let Some(key) = input["idempotency_key"].as_str() {
                request = request.header("idempotency-key", key);
            }
            let body = if let Some(body) = input["body_json"].as_str() {
                request = request.header("content-type", "application/json");
                Body::from(normalise::render(body, &opaque))
            } else {
                Body::empty()
            };
            let response = self
                .app
                .clone()
                .oneshot(request.body(body).unwrap())
                .await
                .unwrap();
            let status = response.status().as_u16();
            let headers = step["response"]["headers"].as_object().map(|expected| {
                expected
                    .keys()
                    .map(|name| {
                        (
                            name.clone(),
                            response
                                .headers()
                                .get(name)
                                .and_then(|value| value.to_str().ok())
                                .map_or(Value::Null, |value| json!(value)),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>()
            });
            let cookie = response
                .headers()
                .get("set-cookie")
                .map(|value| value.to_str().unwrap().to_owned());
            let mut body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
            normalise::response(&mut body, &path, input["path"].as_str().unwrap(), &opaque);
            let mut observed = json!({"status":status,"body":body});
            if let Some(headers) = headers {
                observed["headers"] = Value::Object(headers);
            }
            if let Some(cookie) = cookie {
                observed["cookie"] = self.observe_cookie(&cookie);
            }
            assert_eq!(
                observed, step["response"],
                "{}: step {index}: {method} {path}",
                case["name"]
            );
        }
        assert_eq!(
            case["input"]["steps"].as_array().unwrap().len(),
            usize::try_from(case["expected"]["completed"].as_u64().unwrap()).unwrap()
        );
    }
}
