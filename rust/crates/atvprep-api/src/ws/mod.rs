//! Authenticated durable replay and bounded live WebSocket delivery.
mod data;
pub(crate) mod hub;
use crate::{opaque::Opaque, read_events, routes_auth, state::AppState};
use axum::{
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::HeaderMap,
    response::Response,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

#[derive(Deserialize)]
pub struct WsQuery {
    after: Option<String>,
}

pub async fn upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WsQuery>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let allowed = headers
        .get("origin")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| {
            state
                .config
                .allowed_origins
                .iter()
                .any(|allowed| allowed == origin)
        });
    let token =
        routes_auth::cookie(&headers, state.config.session_cookie_secure).unwrap_or_default();
    upgrade
        .max_message_size(64 * 1024)
        .max_frame_size(64 * 1024)
        .on_upgrade(move |socket| serve(socket, state, query, token, allowed))
}

async fn authenticated(state: AppState, token: String) -> bool {
    tokio::task::spawn_blocking(move || {
        let Ok(conn) = state.db.conn() else {
            return false;
        };
        routes_auth::service(&state, &conn).is_ok_and(|auth| auth.authenticate(&token).is_ok())
    })
    .await
    .unwrap_or(false)
}

async fn close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        socket.send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        }))),
    )
    .await;
}

async fn send(socket: &mut WebSocket, frame: &Value) -> Result<(), ()> {
    let encoded = serde_json::to_string(frame).map_err(|_| ())?;
    if encoded.len() > 64 * 1024 {
        return Err(());
    }
    tokio::time::timeout(
        Duration::from_secs(1),
        socket.send(Message::Text(encoded.into())),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}

struct Subscription {
    hub: Arc<hub::EventHub>,
    id: u64,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.hub.unsubscribe(self.id);
    }
}

#[allow(clippy::too_many_lines)] // Close codes and ownership remain in one connection state machine.
async fn serve(
    mut socket: WebSocket,
    state: AppState,
    query: WsQuery,
    token: String,
    allowed: bool,
) {
    if !allowed {
        close(&mut socket, 4403, "origin not allowed").await;
        return;
    }
    if !authenticated(state.clone(), token.clone()).await {
        close(&mut socket, 4401, "authentication required").await;
        return;
    }
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let mut after = match query.after.as_deref().filter(|value| !value.is_empty()) {
        Some(value) => match opaque.decode(value, "cursor:events") {
            Ok(id) if id >= 0 => id,
            _ => {
                close(&mut socket, 4400, "invalid cursor").await;
                return;
            }
        },
        None => 0,
    };
    let Ok((latest, oldest)) = data::boundary(state.clone()).await else {
        close(&mut socket, 1012, "event store unavailable").await;
        return;
    };
    let cursor = |id| opaque.encode("cursor:events", id);
    if send(&mut socket,&json!({"type":"hello","version":1,"current_event_id":if latest>0{Some(cursor(latest))}else{None},"oldest_event_id":oldest.map(cursor)})).await.is_err(){return;}
    if oldest.is_some_and(|oldest| after > 0 && after < oldest - 1) {
        let _ = send(
            &mut socket,
            &json!({"type":"gap","after":query.after,"reason":"retention"}),
        )
        .await;
        close(&mut socket, 1012, "resync required").await;
        return;
    }
    let hub = state.event_hub.clone();
    let handoff = hub.handoff.lock().await;
    let (id, mut receiver, queue) = hub.subscribe();
    let _subscription = Subscription {
        hub: hub.clone(),
        id,
    };
    let Ok((cutoff, _)) = data::boundary(state.clone()).await else {
        close(&mut socket, 1012, "event store unavailable").await;
        return;
    };
    if let Err(code) = replay(
        &mut socket,
        state.clone(),
        &opaque,
        &mut after,
        cutoff,
        &token,
    )
    .await
    {
        close(
            &mut socket,
            code,
            match code {
                4401 => "authentication required",
                1001 => "server shutdown",
                _ => "resync required",
            },
        )
        .await;
        return;
    }
    hub::EventHub::ensure_publisher(state.clone(), cutoff);
    drop(handoff);
    let mut auth_tick = tokio::time::interval(Duration::from_millis(500));
    let mut ping_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(20),
        Duration::from_secs(20),
    );
    loop {
        if queue.overflow.load(Ordering::Acquire) {
            let _ = send(
                &mut socket,
                &json!({"type":"overflow","after":cursor(after),"reason":"slow consumer"}),
            )
            .await;
            close(&mut socket, 1012, "resync required").await;
            return;
        }
        tokio::select! {
            _=auth_tick.tick()=>{
                if state.shutting_down.load(Ordering::Acquire){close(&mut socket,1001,"server shutdown").await;return;}
                if !authenticated(state.clone(),token.clone()).await{close(&mut socket,4401,"authentication required").await;return;}
            },
            _=ping_tick.tick()=>{if send(&mut socket,&json!({"type":"ping"})).await.is_err(){return;}},
            incoming=socket.recv()=>{match incoming {
                Some(Ok(Message::Text(text)))=>{if !exact_pong(text.as_bytes()){let _=send(&mut socket,&json!({"type":"error","reason":"invalid frame"})).await;close(&mut socket,1008,"invalid frame").await;return;}},
                Some(Ok(Message::Binary(bytes)))=>{if !exact_pong(&bytes){let _=send(&mut socket,&json!({"type":"error","reason":"invalid frame"})).await;close(&mut socket,1008,"invalid frame").await;return;}},
                Some(Ok(Message::Ping(_)|Message::Pong(_)))=>{},
                Some(Err(_))=>{let _=send(&mut socket,&json!({"type":"error","reason":"invalid frame"})).await;close(&mut socket,1008,"invalid frame").await;return;},
                _=>return,
            }},
            frame=receiver.recv()=>{
                let Some(encoded)=frame else{return};queue.bytes.fetch_sub(encoded.len(),Ordering::AcqRel);
                let Ok(frame)=serde_json::from_str::<Value>(&encoded)else{continue};
                let Some(sequence)=frame.get("sequence").and_then(Value::as_i64)else{continue};
                if sequence<=after{continue;}
                if send(&mut socket,&frame).await.is_err(){let _=send(&mut socket,&json!({"type":"overflow","after":cursor(after),"reason":"slow consumer"})).await;close(&mut socket,1012,"resync required").await;return;}
                after=sequence;
            }
        }
    }
}

fn exact_pong(bytes: &[u8]) -> bool {
    bytes.len() <= 64 * 1024
        && serde_json::from_slice::<Value>(bytes).is_ok_and(|frame| frame == json!({"type":"pong"}))
}

const REPLAY_SEND_BUDGET: Duration = Duration::from_secs(5);

async fn send_within_budget(
    socket: &mut WebSocket,
    frame: &Value,
    waited: &mut Duration,
) -> Result<(), u16> {
    let started = tokio::time::Instant::now();
    let sent = send(socket, frame).await;
    *waited += started.elapsed();
    if sent.is_err() || *waited > REPLAY_SEND_BUDGET {
        return Err(1012);
    }
    Ok(())
}

async fn replay(
    socket: &mut WebSocket,
    state: AppState,
    opaque: &Opaque,
    after: &mut i64,
    cutoff: i64,
    token: &str,
) -> Result<(), u16> {
    let mut carry = Vec::new();
    let mut scanned = *after;
    // The hub handoff lock is held for the whole replay, so a slow reader must not keep it forever:
    // the time spent waiting on this socket is budgeted, the time spent reading events is not.
    let mut waited = Duration::ZERO;
    loop {
        if !authenticated(state.clone(), token.to_owned()).await {
            return Err(4401);
        }
        if state.shutting_down.load(Ordering::Acquire) {
            return Err(1001);
        }
        let rows = data::page(state.clone(), scanned, cutoff)
            .await
            .map_err(|()| 1012_u16)?;
        if rows.is_empty() {
            for row in carry {
                send_within_budget(socket, &data::frame(&row, opaque), &mut waited).await?;
                *after = row.id;
            }
            return Ok(());
        }
        scanned = rows.last().map_or(scanned, |row| row.id);
        carry.extend(rows);
        carry = read_events::coalesce(carry);
        let frame_size = |row: &read_events::EventRow| {
            serde_json::to_vec(&data::frame(row, opaque)).map_or(usize::MAX, |frame| frame.len())
        };
        let mut bytes = carry
            .iter()
            .map(frame_size)
            .fold(0_usize, usize::saturating_add);
        while carry.len() > 256 || bytes > 256 * 1024 {
            let row = carry.remove(0);
            bytes = bytes.saturating_sub(frame_size(&row));
            send_within_budget(socket, &data::frame(&row, opaque), &mut waited).await?;
            *after = row.id;
        }
    }
}
