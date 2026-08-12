//! Compact, bounded Server-Sent Event invalidation hubs.
//!
//! REST remains authoritative: this module only records a bounded sequence of
//! invalidations and reset notifications. Public and Admin routers own
//! separate hubs, so an administrative event can never cross the Public
//! projection boundary.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::response::sse::Event;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_stream::Stream;

const EVENT_VERSION: u64 = 1;
const DEFAULT_BUFFER_CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Invalidation {
    pub version: u64,
    pub event_id: u64,
    pub resource: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset: Option<bool>,
}

#[derive(Debug, Clone)]
struct StoredEvent {
    event: Invalidation,
}

#[derive(Debug)]
struct HubState {
    next_id: u64,
    events: VecDeque<StoredEvent>,
}

#[derive(Debug)]
struct HubInner {
    capacity: usize,
    state: Mutex<HubState>,
    notify: Notify,
}

/// A bounded invalidation stream. Cloning it shares the sequence and buffer.
#[derive(Clone, Debug)]
pub(crate) struct RealtimeHub {
    inner: Arc<HubInner>,
}

impl Default for RealtimeHub {
    fn default() -> Self {
        Self::new(DEFAULT_BUFFER_CAPACITY)
    }
}

impl RealtimeHub {
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "SSE buffer capacity must be positive");
        Self {
            inner: Arc::new(HubInner {
                capacity,
                state: Mutex::new(HubState {
                    next_id: 0,
                    events: VecDeque::new(),
                }),
                notify: Notify::new(),
            }),
        }
    }

    /// Publish an invalidation, coalescing the most recent event for the same
    /// `(resource, resource_id)` key. `resource_id = None` is a collection
    /// event and is intentionally used for Public privacy resets.
    pub(crate) fn publish(
        &self,
        resource: impl Into<String>,
        resource_id: Option<impl Into<String>>,
        revision: u64,
    ) -> u64 {
        self.publish_inner(
            resource.into(),
            resource_id.map(Into::into),
            revision,
            false,
        )
    }

    /// Publish a collection-level reset. It never carries a private Node ID.
    pub(crate) fn publish_reset(&self, resource: impl Into<String>, revision: u64) -> u64 {
        self.publish_inner(resource.into(), None, revision, true)
    }

    fn publish_inner(
        &self,
        resource: String,
        resource_id: Option<String>,
        revision: u64,
        reset: bool,
    ) -> u64 {
        let mut state = self.inner.state.lock().expect("SSE hub mutex poisoned");
        state.next_id = state.next_id.saturating_add(1).max(1);
        let event = Invalidation {
            version: EVENT_VERSION,
            event_id: state.next_id,
            resource,
            resource_id,
            revision,
            reset: reset.then_some(true),
        };
        // Keep only the newest notification for a key. This bounds both the
        // queue and event pressure when ingestion updates rapidly.
        state.events.retain(|stored| {
            stored.event.resource != event.resource
                || stored.event.resource_id != event.resource_id
                || stored.event.reset != event.reset
        });
        state.events.push_back(StoredEvent { event });
        while state.events.len() > self.inner.capacity {
            state.events.pop_front();
        }
        let id = state.next_id;
        drop(state);
        self.inner.notify.notify_waiters();
        id
    }

    fn next_after(&self, cursor: &mut u64) -> Option<Invalidation> {
        let state = self.inner.state.lock().expect("SSE hub mutex poisoned");
        let first = state.events.front()?.event.event_id;
        if *cursor < first.saturating_sub(1) {
            // A replay cursor fell behind the bounded buffer. A reset is
            // represented by the first retained event id, then normal replay
            // continues from that point.
            *cursor = first;
            return Some(Invalidation {
                version: EVENT_VERSION,
                event_id: first,
                resource: "collection".to_owned(),
                resource_id: None,
                revision: first,
                reset: Some(true),
            });
        }
        let event = state
            .events
            .iter()
            .find(|stored| stored.event.event_id > *cursor)
            .map(|stored| stored.event.clone());
        if let Some(event) = &event {
            *cursor = event.event_id;
        }
        event
    }

    pub(crate) fn stream(
        &self,
        last_event_id: Option<u64>,
    ) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static + use<> {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        let hub = self.clone();
        let mut cursor = last_event_id.unwrap_or(0);
        tokio::spawn(async move {
            loop {
                if let Some(invalidation) = hub.next_after(&mut cursor) {
                    let id = invalidation.event_id.to_string();
                    let data = serde_json::to_string(&invalidation)
                        .expect("invalidation has only serializable fields");
                    if sender
                        .send(Ok(Event::default().id(id).event("invalidation").data(data)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
                hub.inner.notify.notified().await;
            }
        });
        tokio_stream::wrappers::ReceiverStream::new(receiver)
    }
}

/// Parse the browser's reconnect cursor without allowing malformed values to
/// affect authorization or stream state.
pub(crate) fn parse_last_event_id(value: Option<&str>) -> Option<u64> {
    value.and_then(|value| value.parse::<u64>().ok())
}

/// Keepalive policy shared by the Public and Admin streams.
pub(crate) fn keepalive_interval() -> Duration {
    Duration::from_secs(15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn coalesces_and_replays_bounded_events() {
        let hub = RealtimeHub::new(2);
        hub.publish("node", Some("a"), 1);
        hub.publish("node", Some("a"), 2);
        let mut cursor = 0;
        let event = hub.next_after(&mut cursor).unwrap();
        assert_eq!(event.revision, 2);
        assert_eq!(event.event_id, 2);

        hub.publish("node", Some("b"), 3);
        hub.publish("node", Some("c"), 4);
        let mut cursor = 0;
        let reset = hub.next_after(&mut cursor).unwrap();
        assert_eq!(reset.reset, Some(true));
        assert_eq!(reset.resource_id, None);
    }

    #[test]
    fn reset_never_contains_a_resource_id() {
        let hub = RealtimeHub::new(4);
        hub.publish_reset("public", 9);
        let mut cursor = 0;
        let event = hub.next_after(&mut cursor).unwrap();
        assert_eq!(event.reset, Some(true));
        assert_eq!(event.resource_id, None);
        assert_eq!(parse_last_event_id(Some("12")), Some(12));
        assert_eq!(parse_last_event_id(Some("bad")), None);
    }
}
