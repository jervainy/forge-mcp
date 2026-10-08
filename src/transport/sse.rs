//! Stateful SSE delivery for the 2025-11-25 Streamable HTTP transport.
//!
//! Every stream has a distinct ID and a bounded, in-memory replay buffer.
//! The session owning this hub is authenticated before the hub is accessed.

use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    response::{IntoResponse, Response, Sse, sse::{Event, KeepAlive}},
};
use futures::stream;
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

const MAX_STREAMS: usize = 64;
const MAX_EVENTS: usize = 256;
const REPLAY_TTL: Duration = Duration::from_secs(600);
const HEARTBEAT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamError {
    InvalidCursor,
    UnknownStream,
    ExpiredCursor,
    AlreadyConnected,
    AtCapacity,
    SessionClosed,
}

#[derive(Clone, Default)]
pub struct SseHub {
    inner: Arc<Mutex<HashMap<String, Arc<StreamLog>>>>,
    closed: Arc<AtomicBool>,
}

impl SseHub {
    pub async fn create(&self) -> Result<Arc<StreamLog>, StreamError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(StreamError::SessionClosed);
        }

        let mut streams = self.inner.lock().await;
        streams.retain(|_, stream| {
            stream.connected.load(Ordering::SeqCst)
                || (stream.born.elapsed() < REPLAY_TTL)
        });
        if streams.len() >= MAX_STREAMS {
            return Err(StreamError::AtCapacity);
        }

        let id = Uuid::new_v4().to_string();
        let log = Arc::new(StreamLog::new(id.clone()));
        streams.insert(id, log.clone());
        Ok(log)
    }

    /// A Last-Event-ID never crosses a stream boundary. A response from
    /// a POST stream can be resumed only through that stream's UUID.
    pub async fn resume(&self, last_id: &str) -> Result<(Arc<StreamLog>, u64), StreamError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(StreamError::SessionClosed);
        }

        let (id, position) = parse_event_id(last_id)?;
        let streams = self.inner.lock().await;
        let log = streams.get(id).cloned().ok_or(StreamError::UnknownStream)?;
        drop(streams);

        log.validate_cursor(position).await?;
        Ok((log, position))
    }

    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let logs: Vec<_> = self.inner.lock().await.values().cloned().collect();
        for log in logs {
            log.finish().await;
        }
        self.inner.lock().await.clear();
    }
}

fn parse_event_id(value: &str) -> Result<(&str, u64), StreamError> {
    let (id, position) = value.rsplit_once(':').ok_or(StreamError::InvalidCursor)?;
    Uuid::parse_str(id).map_err(|_| StreamError::InvalidCursor)?;
    let position = position.parse::<u64>().map_err(|_| StreamError::InvalidCursor)?;
    Ok((id, position))
}

#[derive(Clone)]
struct StoredEvent {
    position: u64,
    data: String,
}

struct EventState {
    events: VecDeque<StoredEvent>,
    next: u64,
    finished: bool,
}

pub struct StreamLog {
    id: String,
    born: Instant,
    state: Mutex<EventState>,
    changed: watch::Sender<u64>,
    connected: AtomicBool,
}

impl StreamLog {
    fn new(id: String) -> Self {
        let (changed, _) = watch::channel(0_u64);
        let mut events = VecDeque::new();
        // Priming event: a valid SSE ID with empty data, per MCP transport.
        events.push_back(StoredEvent {
            position: 0,
            data: String::new(),
        });
        Self {
            id,
            born: Instant::now(),
            state: Mutex::new(EventState {
                events,
                next: 1,
                finished: false,
            }),
            changed,
            connected: AtomicBool::new(false),
        }
    }

    pub async fn emit_json(&self, value: &impl serde::Serialize) {
        let Ok(data) = serde_json::to_string(value) else {
            return;
        };
        let mut state = self.state.lock().await;
        if state.finished {
            return;
        }
        let position = state.next;
        state.next += 1;
        state.events.push_back(StoredEvent { position, data });
        if state.events.len() > MAX_EVENTS {
            state.events.pop_front();
        }
        self.changed.send_replace(position);
    }

    pub async fn finish(&self) {
        let mut state = self.state.lock().await;
        state.finished = true;
        self.changed.send_replace(state.next);
    }

    async fn validate_cursor(&self, cursor: u64) -> Result<(), StreamError> {
        let state = self.state.lock().await;
        let Some(first) = state.events.front() else {
            return Err(StreamError::ExpiredCursor);
        };
        if cursor < first.position || cursor >= state.next {
            return Err(StreamError::ExpiredCursor);
        }
        Ok(())
    }

    pub fn subscribe(self: &Arc<Self>, cursor: Option<u64>) -> Result<Listener, StreamError> {
        if self.connected.swap(true, Ordering::SeqCst) {
            return Err(StreamError::AlreadyConnected);
        }
        Ok(Listener {
            log: self.clone(),
            changes: self.changed.subscribe(),
            cursor,
        })
    }
}

pub struct Listener {
    log: Arc<StreamLog>,
    changes: watch::Receiver<u64>,
    cursor: Option<u64>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.log.connected.store(false, Ordering::SeqCst);
    }
}

impl Listener {
    async fn next(&mut self) -> Option<Event> {
        loop {
            {
                let state = self.log.state.lock().await;
                let next = state.events.iter().find(|event| {
                    self.cursor.is_none_or(|last| event.position > last)
                });
                if let Some(next) = next {
                    self.cursor = Some(next.position);
                    return Some(
                        Event::default()
                            .id(format!("{}:{}", self.log.id, next.position))
                            .data(next.data.clone()),
                    );
                }
                if state.finished {
                    return None;
                }
            }
            if self.changes.changed().await.is_err() {
                return None;
            }
        }
    }
}

pub fn response(listener: Listener) -> Response {
    let stream = stream::unfold(listener, |mut listener| async move {
        listener
            .next()
            .await
            .map(|event| (Ok::<Event, Infallible>(event), listener))
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(HEARTBEAT).text("keepalive"))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn post_stream_replays_only_unseen_events() {
        let hub = SseHub::default();
        let log = hub.create().await.unwrap();
        let mut subscriber = log.subscribe(None).unwrap();
        assert!(subscriber.next().await.is_some());
        assert_eq!(subscriber.cursor, Some(0));
        let cursor = format!("{}:0", log.id);
        log.emit_json(&serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}}))
            .await;
        log.finish().await;
        drop(subscriber);

        let (replay, last) = hub.resume(&cursor).await.unwrap();
        assert_eq!(last, 0);
        let mut subscriber = replay.subscribe(Some(last)).unwrap();
        assert!(subscriber.next().await.is_some());
        assert_eq!(subscriber.cursor, Some(1));
        assert!(subscriber.next().await.is_none());
    }

    #[tokio::test]
    async fn streams_are_isolated_and_bound_to_same_session() {
        let hub = SseHub::default();
        let first = hub.create().await.unwrap();
        let second = hub.create().await.unwrap();
        let cursor = format!("{}:0", first.id);
        let (replay, _) = hub.resume(&cursor).await.unwrap();
        assert!(Arc::ptr_eq(&first, &replay));
        assert!(!Arc::ptr_eq(&second, &replay));
        assert!(SseHub::default().resume(&cursor).await.is_err());
    }

    #[tokio::test]
    async fn disconnect_and_delete_end_streams() {
        let hub = SseHub::default();
        let log = hub.create().await.unwrap();
        let listener = log.subscribe(None).unwrap();
        assert!(matches!(log.subscribe(None), Err(StreamError::AlreadyConnected)));
        drop(listener);
        assert!(log.subscribe(None).is_ok());
        hub.shutdown().await;
        assert!(matches!(hub.create().await, Err(StreamError::SessionClosed)));
    }
}
