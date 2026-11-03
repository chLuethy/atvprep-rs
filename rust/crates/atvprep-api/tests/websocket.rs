//! Real WebSocket protocol checks against an ephemeral axum server.
use atvprep_api::{
    auth::AuthService, config::ManagerConfig, opaque::Opaque, router, state::AppState,
};
use atvprep_manager::{db::Db, repo::Ctx, schema};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Fixture {
    _directory: tempfile::TempDir,
    state: AppState,
    token: String,
    session: i64,
    url: String,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let db = Db::open(&directory.path().join("events.db")).unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        conn.raw()
            .execute_batch(
                "INSERT INTO jobs(id,action,status,trigger) VALUES(1,'verify','queued','manual');",
            )
            .unwrap();
        let config = ManagerConfig {
            api_secret: Some("ws-secret".to_owned()),
            allowed_origins: vec!["http://testserver".to_owned()],
            ..ManagerConfig::default()
        };
        let state = AppState::new(db, config).unwrap();
        let auth = AuthService::with_csrf_ttl(
            Ctx {
                conn: conn.raw(),
                clock: state.clock.as_ref(),
            },
            28_800,
            3600,
            900,
        )
        .unwrap();
        let session = auth.create().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/api/ws", listener.local_addr().unwrap());
        let app = router(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            _directory: directory,
            state,
            token: session.token,
            session: session.id,
            url,
            server,
        }
    }
    async fn connect(&self, suffix: &str, origin: Option<&str>, authorized: bool) -> Socket {
        let mut request = format!("{}{suffix}", self.url)
            .into_client_request()
            .unwrap();
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert("origin", origin.parse().unwrap());
        }
        if authorized {
            request.headers_mut().insert(
                "cookie",
                format!("__Host-atvprep_session={}", self.token)
                    .parse()
                    .unwrap(),
            );
        }
        tokio_tungstenite::connect_async(request).await.unwrap().0
    }
    fn event(&self, id: i64) {
        let conn = self.state.db.conn().unwrap();
        conn.raw().execute("INSERT INTO events(id,job_id,topic,entity_type,entity_id,payload_json) VALUES(?1,1,'job.updated','job',1,'{}')",[id]).unwrap();
    }
}

async fn next(socket: &mut Socket) -> Message {
    tokio::time::timeout(Duration::from_secs(4), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
async fn frame(socket: &mut Socket) -> Value {
    let message = next(socket).await;
    serde_json::from_slice(&message.into_data()).unwrap()
}
async fn code(socket: &mut Socket) -> u16 {
    let Message::Close(Some(frame)) = next(socket).await else {
        panic!("expected close frame")
    };
    u16::from(frame.code)
}

#[tokio::test]
async fn rejects_foreign_missing_origin_and_invalid_sessions_or_cursors() {
    let fixture = Fixture::new().await;
    let mut foreign = fixture.connect("", Some("http://foreign"), true).await;
    assert_eq!(code(&mut foreign).await, 4403);
    let mut missing = fixture.connect("", None, true).await;
    assert_eq!(code(&mut missing).await, 4403);
    let mut unauthorized = fixture.connect("", Some("http://testserver"), false).await;
    assert_eq!(code(&mut unauthorized).await, 4401);
    let mut invalid = fixture
        .connect("?after=invalid", Some("http://testserver"), true)
        .await;
    assert_eq!(code(&mut invalid).await, 4400);
}

#[tokio::test]
async fn replays_events_then_delivers_live_events_and_rejects_commands() {
    let fixture = Fixture::new().await;
    fixture.event(1);
    fixture.event(2);
    let mut socket = fixture.connect("", Some("http://testserver"), true).await;
    assert_eq!(frame(&mut socket).await["type"], "hello");
    assert_eq!(frame(&mut socket).await["sequence"], 1);
    assert_eq!(frame(&mut socket).await["sequence"], 2);
    socket
        .send(Message::Text(json!({"type":"pong"}).to_string().into()))
        .await
        .unwrap();
    fixture.event(3);
    let event = frame(&mut socket).await;
    assert_eq!(event["sequence"], 3);
    assert_eq!(event["type"], "event");
    socket
        .send(Message::Text(json!({"type":"command"}).to_string().into()))
        .await
        .unwrap();
    assert_eq!(frame(&mut socket).await["type"], "error");
    assert_eq!(code(&mut socket).await, 1008);
}

#[tokio::test]
async fn reports_retention_gaps_and_closes_revoked_sessions() {
    let fixture = Fixture::new().await;
    fixture.event(10);
    let after = Opaque::new("ws-secret").encode("cursor:events", 1);
    let mut gap = fixture
        .connect(&format!("?after={after}"), Some("http://testserver"), true)
        .await;
    assert_eq!(frame(&mut gap).await["type"], "hello");
    assert_eq!(frame(&mut gap).await["type"], "gap");
    assert_eq!(code(&mut gap).await, 1012);
    let mut socket = fixture.connect("", Some("http://testserver"), true).await;
    assert_eq!(frame(&mut socket).await["type"], "hello");
    assert_eq!(frame(&mut socket).await["sequence"], 10);
    let conn = fixture.state.db.conn().unwrap();
    conn.raw()
        .execute(
            "UPDATE auth_sessions SET revoked_at=?1 WHERE id=?2",
            rusqlite::params![fixture.state.clock.now(), fixture.session],
        )
        .unwrap();
    assert_eq!(code(&mut socket).await, 4401);
}

#[tokio::test]
async fn malformed_frames_close_with_policy_code_and_shutdown_closes_connections() {
    let fixture = Fixture::new().await;
    for text in [
        "{",
        "[]",
        "null",
        r#"{"type":"pong","extra":true}"#,
        r#"{"type":"PING"}"#,
    ] {
        let mut socket = fixture.connect("", Some("http://testserver"), true).await;
        assert_eq!(frame(&mut socket).await["type"], "hello");
        socket
            .send(Message::Text(text.to_owned().into()))
            .await
            .unwrap();
        assert_eq!(frame(&mut socket).await["type"], "error");
        assert_eq!(code(&mut socket).await, 1008);
    }
    let mut socket = fixture.connect("", Some("http://testserver"), true).await;
    assert_eq!(frame(&mut socket).await["type"], "hello");
    fixture
        .state
        .shutting_down
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(code(&mut socket).await, 1001);
}

#[tokio::test]
async fn replay_coalesces_logs_across_database_pages() {
    let fixture = Fixture::new().await;
    let conn = fixture.state.db.conn().unwrap();
    conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library');
        INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'film.mp4','video','.mp4',1,1,'fp','present','clean','idle','none');
        INSERT INTO job_items(id,job_id,media_file_id,action,position,status) VALUES(1,1,1,'verify',0,'queued');").unwrap();
    for id in 1..=301 {
        conn.raw().execute("INSERT INTO events(id,job_id,item_id,topic,entity_type,entity_id,payload_json,created_at) VALUES(?1,1,1,'job.log','item',1,'{\"text\":\"processed\",\"offset\":5}','2026-01-01 00:00:00.000000')",[id]).unwrap();
    }
    let mut socket = fixture.connect("", Some("http://testserver"), true).await;
    assert_eq!(frame(&mut socket).await["type"], "hello");
    let log = frame(&mut socket).await;
    assert_eq!(log["type"], "log_tail");
    assert_eq!(log["sequence"], 301);
    assert_eq!(log["text"], "processed");
    assert_eq!(log["offset"], 5);
    fixture.event(302);
    assert_eq!(frame(&mut socket).await["sequence"], 302);
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn live_handoff_delivers_each_new_sequence_once() {
    let fixture = Fixture::new().await;
    fixture.event(1);
    let mut socket = fixture.connect("", Some("http://testserver"), true).await;
    assert_eq!(frame(&mut socket).await["type"], "hello");
    for id in 2..=80 {
        fixture.event(id);
    }
    for expected in 1..=80 {
        assert_eq!(frame(&mut socket).await["sequence"], expected);
    }
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn a_slow_live_consumer_receives_an_overflow_and_the_last_delivered_cursor() {
    let fixture = Fixture::new().await;
    let mut socket = fixture.connect("", Some("http://testserver"), true).await;
    assert_eq!(frame(&mut socket).await["type"], "hello");
    fixture.event(1);
    assert_eq!(frame(&mut socket).await["sequence"], 1);
    let mut conn = fixture.state.db.conn().unwrap();
    conn.transaction(|tx| {
        let payload=json!({"chunks":["a".repeat(1800),"b".repeat(1800),"c".repeat(1800),"d".repeat(1800)]}).to_string();
        for id in 2..=101 {
            tx.execute("INSERT INTO events(id,job_id,topic,entity_type,entity_id,payload_json) VALUES(?1,1,'job.updated','job',1,?2)",rusqlite::params![id,payload])?;
        }
        Ok(())
    }).unwrap();
    // Leave the subscriber unread while one durable page exceeds its byte budget.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut delivered = 1;
    loop {
        let message = frame(&mut socket).await;
        if message["type"] == "overflow" {
            let opaque = Opaque::new("ws-secret");
            assert_eq!(
                opaque
                    .decode(message["after"].as_str().unwrap(), "cursor:events")
                    .unwrap(),
                delivered
            );
            assert_eq!(message["reason"], "slow consumer");
            break;
        }
        let sequence = message["sequence"].as_i64().unwrap();
        assert_eq!(sequence, delivered + 1);
        delivered = sequence;
    }
    assert!(delivered < 101);
    assert_eq!(code(&mut socket).await, 1012);
}

#[tokio::test]
async fn publisher_recovers_from_store_read_failure_without_skipping_or_duplicating_events() {
    let fixture = Fixture::new().await;
    let mut socket = fixture.connect("", Some("http://testserver"), true).await;
    assert_eq!(frame(&mut socket).await["type"], "hello");
    fixture.event(1);
    assert_eq!(frame(&mut socket).await["sequence"], 1);
    let conn = fixture.state.db.conn().unwrap();
    conn.raw().execute_batch("ALTER TABLE events RENAME TO offline_events;
        INSERT INTO offline_events(id,job_id,topic,entity_type,entity_id,payload_json) VALUES(2,1,'job.updated','job',1,'{}');").unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), socket.next())
            .await
            .is_err()
    );
    conn.raw()
        .execute_batch("ALTER TABLE offline_events RENAME TO events;")
        .unwrap();
    assert_eq!(frame(&mut socket).await["sequence"], 2);
    fixture.event(3);
    assert_eq!(frame(&mut socket).await["sequence"], 3);
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn heartbeat_ping_accepts_a_binary_pong_and_keeps_live_delivery() {
    let fixture = Fixture::new().await;
    let mut socket = fixture.connect("", Some("http://testserver"), true).await;
    assert_eq!(frame(&mut socket).await["type"], "hello");
    let message = tokio::time::timeout(Duration::from_secs(25), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&message.into_data()).unwrap(),
        json!({"type":"ping"})
    );
    socket
        .send(Message::Binary(br#"{"type":"pong"}"#.to_vec().into()))
        .await
        .unwrap();
    fixture.event(1);
    assert_eq!(frame(&mut socket).await["sequence"], 1);
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn byte_bounded_replay_preserves_large_event_order_and_live_handoff() {
    let fixture = Fixture::new().await;
    let mut conn = fixture.state.db.conn().unwrap();
    conn.transaction(|tx| {
        let payload = json!({"chunks":["a".repeat(1800),"b".repeat(1800),"c".repeat(1800),"d".repeat(1800)]}).to_string();
        for id in 1..=100 {
            tx.execute("INSERT INTO events(id,job_id,topic,entity_type,entity_id,payload_json) VALUES(?1,1,'job.updated','job',1,?2)",rusqlite::params![id,payload])?;
        }
        Ok(())
    }).unwrap();
    let mut socket = fixture.connect("", Some("http://testserver"), true).await;
    assert_eq!(frame(&mut socket).await["type"], "hello");
    for id in 1..=100 {
        let event = frame(&mut socket).await;
        assert_eq!(event["sequence"], id);
        assert!(serde_json::to_vec(&event).unwrap().len() < 8192);
    }
    fixture.event(101);
    assert_eq!(frame(&mut socket).await["sequence"], 101);
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn oversized_stored_payload_is_bounded_without_stopping_the_publisher() {
    let fixture = Fixture::new().await;
    let mut socket = fixture.connect("", Some("http://testserver"), true).await;
    assert_eq!(frame(&mut socket).await["type"], "hello");
    fixture.event(1);
    assert_eq!(frame(&mut socket).await["sequence"], 1);
    let conn = fixture.state.db.conn().unwrap();
    let payload = json!({"text":"x".repeat(100_000)}).to_string();
    conn.raw().execute("INSERT INTO events(id,job_id,topic,entity_type,entity_id,payload_json) VALUES(2,1,'job.updated','job',1,?1)",[payload]).unwrap();
    let event = frame(&mut socket).await;
    assert_eq!(event["sequence"], 2);
    assert_eq!(event["payload"], json!({"truncated":true}));
    fixture.event(3);
    assert_eq!(frame(&mut socket).await["sequence"], 3);
    socket.close(None).await.unwrap();
}
