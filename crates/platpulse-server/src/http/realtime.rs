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
    closed: bool,
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
                    closed: false,
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
        if state.closed {
            return state.next_id;
        }
        state.next_id = state.next_id.saturating_add(1).max(1);
        let event_id = state.next_id;
        let event = Invalidation {
            version: EVENT_VERSION,
            event_id,
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

    pub(crate) fn shutdown(&self, resource: &str) {
        let mut state = self.inner.state.lock().expect("SSE hub mutex poisoned");
        if state.closed {
            return;
        }
        state.next_id = state.next_id.saturating_add(1).max(1);
        let event_id = state.next_id;
        state.events.clear();
        state.events.push_back(StoredEvent {
            event: Invalidation {
                version: EVENT_VERSION,
                event_id,
                resource: resource.to_owned(),
                resource_id: None,
                revision: event_id,
                reset: Some(true),
            },
        });
        state.closed = true;
        drop(state);
        self.inner.notify.notify_waiters();
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

    #[cfg(test)]
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
                    let closing = invalidation.resource == "server_shutdown";
                    let data =
                        serde_json::to_string(&invalidation).expect("invalidation serializes");
                    if sender
                        .send(Ok(Event::default()
                            .id(invalidation.event_id.to_string())
                            .event("invalidation")
                            .data(data)))
                        .await
                        .is_err()
                        || closing
                    {
                        break;
                    }
                    continue;
                }
                if hub
                    .inner
                    .state
                    .lock()
                    .expect("SSE hub mutex poisoned")
                    .closed
                {
                    break;
                }
                hub.inner.notify.notified().await;
            }
        });
        tokio_stream::wrappers::ReceiverStream::new(receiver)
    }
    /// Human-bound stream: closes with a `reset` event as soon as the
    /// bound Session is no longer current — revoked, expired, idle, user
    /// disabled, or role changed (design §13.5: revoke/disable/role-change
    /// actively close the bound Human stream). `connected_role` is the
    /// role the stream was opened with, so any role change closes it even
    /// when the route group itself would still accept the session.
    /// Build the access-reset event that closes a bound stream: an
    /// `event: reset` carrying the collection-level reset payload. Shared
    /// by the Human- and Guest-bound streams so the two authorization
    /// paths cannot drift apart.
    fn reset_event() -> Result<Event, Infallible> {
        let event = Invalidation {
            version: EVENT_VERSION,
            event_id: 0,
            resource: "collection".to_owned(),
            resource_id: None,
            revision: 0,
            reset: Some(true),
        };
        let data = serde_json::to_string(&event).expect("invalidation serializes");
        Ok(Event::default().event("reset").data(data))
    }

    pub(crate) fn stream_with_session(
        &self,
        last_event_id: Option<u64>,
        db: std::sync::Arc<crate::database::ServerDatabase>,
        auth: crate::auth::AuthConfig,
        session_id: String,
        connected_role: String,
    ) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static + use<> {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        let hub = self.clone();
        let mut cursor = last_event_id.unwrap_or(0);
        tokio::spawn(async move {
            let mut check = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = check.tick() => {
                        if !crate::auth::session_is_current(&db, &session_id, Some(&connected_role), &auth).await {
                            let _ = sender.send(Self::reset_event()).await;
                            break;
                        }
                    }
                    result = async {
                        if !crate::auth::session_is_current(&db, &session_id, Some(&connected_role), &auth).await {
                            let _ = sender.send(Self::reset_event()).await;
                            return true;
                        }
                        if let Some(invalidation) = hub.next_after(&mut cursor) {
                            let closing = invalidation.resource == "server_shutdown";
                            let id = invalidation.event_id.to_string();
                            let data = serde_json::to_string(&invalidation).expect("invalidation serializes");
                            if sender.send(Ok(Event::default().id(id).event("invalidation").data(data))).await.is_err() { return true; }
                            closing
                        } else if hub.inner.state.lock().expect("SSE hub mutex poisoned").closed { true } else { hub.inner.notify.notified().await; false }
                    } => {
                        if result { break; }
                    }
                }
            }
        });
        tokio_stream::wrappers::ReceiverStream::new(receiver)
    }

    /// Guest-bound stream: no Human Session. It stays open only while
    /// anonymous Home is enabled; disabling Guest access closes every
    /// open Guest stream with a `reset` (design §13.5: anonymous Home
    /// closing actively closes all Guest streams).
    pub(crate) fn stream_with_guest(
        &self,
        last_event_id: Option<u64>,
        db: std::sync::Arc<crate::database::ServerDatabase>,
    ) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static + use<> {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        let hub = self.clone();
        let mut cursor = last_event_id.unwrap_or(0);
        tokio::spawn(async move {
            let mut check = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = check.tick() => {
                        if !crate::auth::anonymous_home_enabled(&db).await.unwrap_or(false) {
                            let _ = sender.send(Self::reset_event()).await;
                            break;
                        }
                    }
                    result = async {
                        if !crate::auth::anonymous_home_enabled(&db).await.unwrap_or(false) {
                            let _ = sender.send(Self::reset_event()).await;
                            return true;
                        }
                        if let Some(invalidation) = hub.next_after(&mut cursor) {
                            let closing = invalidation.resource == "server_shutdown";
                            let id = invalidation.event_id.to_string();
                            let data = serde_json::to_string(&invalidation).expect("invalidation serializes");
                            if sender.send(Ok(Event::default().id(id).event("invalidation").data(data))).await.is_err() { return true; }
                            closing
                        } else if hub.inner.state.lock().expect("SSE hub mutex poisoned").closed { true } else { hub.inner.notify.notified().await; false }
                    } => {
                        if result { break; }
                    }
                }
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
    use std::sync::Arc;

    use tempfile::tempdir;

    use crate::auth::{AuthConfig, hash_password};
    use crate::database::{ServerDatabase, ServerDatabaseConfig, initialize};
    use crate::secrets::{create_pepper_file, load_pepper_file};

    use super::*;
    use tokio_stream::StreamExt;

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

    #[tokio::test]
    async fn shutdown_emits_reset_and_closes_stream() {
        let hub = RealtimeHub::new(4);
        hub.shutdown("server_shutdown");
        let mut stream = Box::pin(hub.stream(None));
        let item = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(item.is_ok());
        assert!(
            tokio::time::timeout(Duration::from_secs(1), stream.next())
                .await
                .unwrap()
                .is_none()
        );
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

    async fn stream_db() -> (tempfile::TempDir, Arc<ServerDatabase>, AuthConfig) {
        let dir = tempdir().unwrap();
        let db = Arc::new(
            initialize(ServerDatabaseConfig::new(dir.path().join("server.db")))
                .await
                .unwrap(),
        );
        let pepper_path = dir.path().join("pepper");
        create_pepper_file(&pepper_path).unwrap();
        let config = AuthConfig::development(
            load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let hash = hash_password(b"correct horse battery").unwrap();
        crate::auth::create_owner(&db, "admin", &hash)
            .await
            .unwrap();
        (dir, db, config)
    }

    /// A revoke must close a bound Human stream with a `reset` event
    /// (design §13.5, issue #47: Session revoke closes the associated
    /// Admin/Public SSE streams immediately).
    #[tokio::test]
    async fn session_revoke_closes_the_bound_stream_with_reset() {
        let (_dir, db, config) = stream_db().await;
        let (session, _) = crate::auth::login(
            &db,
            &config,
            "admin",
            "correct horse battery",
            None,
            "Unknown",
        )
        .await
        .unwrap();
        let hub = RealtimeHub::new(8);
        let mut stream = Box::pin(hub.stream_with_session(
            None,
            Arc::clone(&db),
            config.clone(),
            session.session_id.clone(),
            session.role.clone(),
        ));
        crate::auth::revoke_session(&db, &session.session_id)
            .await
            .unwrap();
        let item = tokio::time::timeout(Duration::from_secs(3), stream.next())
            .await
            .expect("revoke must produce a reset event")
            .expect("stream must stay open");
        assert!(item.is_ok(), "the reset event must not be an error");
        assert!(
            !crate::auth::session_is_current(
                &db,
                &session.session_id,
                Some(&session.role),
                &config
            )
            .await,
            "the revoked session must no longer be current"
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(3), stream.next())
                .await
                .expect("stream must close after reset")
                .is_none()
        );
    }

    /// A role change must close a bound Human stream even when the route
    /// group itself would still accept the session (design §13.5).
    #[tokio::test]
    async fn role_change_closes_the_bound_stream() {
        let (_dir, db, config) = stream_db().await;
        let (session, _) = crate::auth::login(
            &db,
            &config,
            "admin",
            "correct horse battery",
            None,
            "Unknown",
        )
        .await
        .unwrap();
        let hub = RealtimeHub::new(8);
        let mut stream = Box::pin(hub.stream_with_session(
            None,
            Arc::clone(&db),
            config.clone(),
            session.session_id.clone(),
            session.role.clone(),
        ));
        sqlx::query("UPDATE users SET role = 'viewer' WHERE username = 'admin'")
            .execute(db.pool())
            .await
            .unwrap();
        let item = tokio::time::timeout(Duration::from_secs(3), stream.next())
            .await
            .expect("role change must produce a reset event")
            .expect("stream must stay open");
        assert!(item.is_ok(), "the reset event must not be an error");
        assert!(
            !crate::auth::session_is_current(
                &db,
                &session.session_id,
                Some(&session.role),
                &config
            )
            .await,
            "the changed role must no longer satisfy the bound role"
        );
    }

    /// Disabling anonymous Home must close every open Guest stream with a
    /// reset (design §13.5: anonymous Home closing closes Guest streams).
    #[tokio::test]
    async fn guest_stream_closes_when_anonymous_home_is_disabled() {
        let (_dir, db, _config) = stream_db().await;
        sqlx::query(
            "INSERT INTO server_settings (setting_key, setting_value, updated_at) VALUES ('anonymous_home', '1', '2026-01-01T00:00:00Z')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let hub = RealtimeHub::new(8);
        let mut stream = Box::pin(hub.stream_with_guest(None, Arc::clone(&db)));
        sqlx::query(
            "UPDATE server_settings SET setting_value = '0' WHERE setting_key = 'anonymous_home'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let item = tokio::time::timeout(Duration::from_secs(3), stream.next())
            .await
            .expect("disabling guest access must produce a reset event")
            .expect("stream must stay open");
        assert!(item.is_ok(), "the reset event must not be an error");
        assert!(
            !crate::auth::anonymous_home_enabled(&db).await.unwrap(),
            "anonymous Home must be disabled again"
        );
    }
}
