//! Byte-bounded live subscriptions and a single durable outbox publisher.
use crate::{opaque::Opaque, state::AppState};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[derive(Default, Debug)]
pub struct QueueState {
    pub bytes: AtomicUsize,
    pub overflow: AtomicBool,
}
#[derive(Debug)]
struct Subscriber {
    sender: mpsc::Sender<String>,
    state: Arc<QueueState>,
}
#[derive(Default, Debug)]
pub struct EventHub {
    pub handoff: tokio::sync::Mutex<()>,
    subscribers: Mutex<HashMap<u64, Subscriber>>,
    next: AtomicU64,
    running: AtomicBool,
    cursor: AtomicI64,
}

impl EventHub {
    pub fn subscribe(&self) -> (u64, mpsc::Receiver<String>, Arc<QueueState>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel(100);
        let state = Arc::new(QueueState::default());
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                id,
                Subscriber {
                    sender,
                    state: state.clone(),
                },
            );
        (id, receiver, state)
    }
    pub fn unsubscribe(&self, id: u64) {
        if let Some(subscriber) = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id)
        {
            subscriber.state.bytes.store(0, Ordering::Release);
        }
    }
    fn empty(&self) -> bool {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }
    fn overflow(&self) {
        for subscriber in self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
        {
            subscriber.state.overflow.store(true, Ordering::Release);
        }
    }
    fn publish(&self, frame: &Value) {
        let Ok(encoded) = serde_json::to_string(frame) else {
            self.overflow();
            return;
        };
        if encoded.len() > 64 * 1024 {
            self.overflow();
            return;
        }
        for subscriber in self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
        {
            if subscriber.state.overflow.load(Ordering::Acquire) {
                continue;
            }
            let bytes = subscriber
                .state
                .bytes
                .fetch_add(encoded.len(), Ordering::AcqRel);
            if bytes.saturating_add(encoded.len()) > 256 * 1024
                || subscriber.sender.try_send(encoded.clone()).is_err()
            {
                subscriber
                    .state
                    .bytes
                    .fetch_sub(encoded.len(), Ordering::AcqRel);
                subscriber.state.overflow.store(true, Ordering::Release);
            }
        }
    }
    pub fn ensure_publisher(state: AppState, cutoff: i64) {
        if state.event_hub.running.swap(true, Ordering::AcqRel) {
            return;
        }
        state.event_hub.cursor.store(cutoff, Ordering::Release);
        tokio::spawn(publisher(state));
    }
}

async fn publisher(state: AppState) {
    let hub = state.event_hub.clone();
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let mut pending: BTreeMap<String, (i64, Value)> = BTreeMap::new();
    let mut flush = Instant::now();
    let mut after = hub.cursor.load(Ordering::Acquire);
    let mut delay = Duration::from_millis(50);
    loop {
        let lock = hub.handoff.lock().await;
        if hub.empty() {
            hub.running.store(false, Ordering::Release);
            return;
        }
        let rows = super::data::page(state.clone(), after, i64::MAX).await;
        if let Ok(rows) = rows {
            delay = Duration::from_millis(50);
            for row in rows {
                let message = super::data::frame(&row, &opaque);
                if row.topic == "job.log" {
                    let key = row
                        .item
                        .map_or_else(|| format!("event:{}", row.id), |id| format!("item:{id}"));
                    pending.insert(key, (row.id, message));
                    let bytes: usize = pending
                        .values()
                        .map(|(_, value)| {
                            serde_json::to_vec(value).map_or(usize::MAX, |value| value.len())
                        })
                        .fold(0, usize::saturating_add);
                    if pending.len() > 100 || bytes > 256 * 1024 {
                        hub.overflow();
                        hub.running.store(false, Ordering::Release);
                        return;
                    }
                } else {
                    flush_before(&hub, &mut pending, row.id);
                    hub.publish(&message);
                    hub.cursor.store(row.id, Ordering::Release);
                }
                after = row.id;
            }
        } else {
            delay = (delay * 2).min(Duration::from_secs(1));
        }
        if flush.elapsed() >= Duration::from_millis(250) {
            flush_before(&hub, &mut pending, i64::MAX);
            flush = Instant::now();
        }
        drop(lock);
        tokio::time::sleep(delay).await;
    }
}

fn flush_before(hub: &EventHub, pending: &mut BTreeMap<String, (i64, Value)>, before: i64) {
    let mut selected: Vec<_> = pending
        .iter()
        .filter(|(_, value)| value.0 < before)
        .map(|(key, value)| (value.0, key.clone()))
        .collect();
    selected.sort();
    for (id, key) in selected {
        if let Some((_, value)) = pending.remove(&key) {
            hub.publish(&value);
            hub.cursor.store(id, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn subscribers_are_bounded_by_count_and_bytes() {
        let hub = EventHub::default();
        let (id, _receiver, state) = hub.subscribe();
        for sequence in 0..100 {
            hub.publish(&json!({"sequence":sequence}));
        }
        assert!(!state.overflow.load(Ordering::Acquire));
        hub.publish(&json!({"sequence":101}));
        assert!(state.overflow.load(Ordering::Acquire));
        assert!(state.bytes.load(Ordering::Acquire) <= 256 * 1024);
        hub.unsubscribe(id);
        assert_eq!(state.bytes.load(Ordering::Acquire), 0);
        let (_, _receiver, state) = hub.subscribe();
        for sequence in 0..5 {
            hub.publish(&json!({"sequence":sequence,"text":"x".repeat(64_000)}));
        }
        assert!(state.overflow.load(Ordering::Acquire));
        assert!(state.bytes.load(Ordering::Acquire) <= 256 * 1024);
    }
}
