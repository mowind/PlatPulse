//! Compact, bounded Server-Sent Event invalidation hubs.
//!
//! REST remains authoritative: this module only records a bounded sequence of
//! invalidations and reset notifications. Public and Admin routers own
//! separate hubs, so an administrative event can never cross the Public
//! projection boundary.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::response::sse::Event;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::sync::Notify;
use tokio_stream::Stream;
use utoipa::ToSchema;

const EVENT_VERSION: u64 = 1;
const DEFAULT_BUFFER_CAPACITY: usize = 256;

/// A resource the Public stream can invalidate. Each names a set of Home
/// queries the browser refetches; the Public hub cannot carry anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublicResource {
    Node,
    Network,
    Validator,
    Geo,
    RewardWallet,
    /// Every Public projection, such as after a retention policy change.
    Collection,
}

/// A resource the Admin stream can invalidate. Each names a set of Admin
/// queries the browser refetches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AdminResource {
    Node,
    Peer,
    Agent,
    Geo,
    Network,
    Validator,
    Access,
    Alerts,
    Notifications,
    Operations,
    Retention,
    Backups,
    Doctor,
}

/// Why a reset event (`reset: true`) tells the browser its cached state can
/// no longer be patched by single invalidations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResetReason {
    /// The cursor missed events (buffer overflow, earlier Server process),
    /// or a Node Purge or Agent removal took rows away.
    Collection,
    /// The Site Access Mode changed.
    SiteAccessMode,
    /// The Server is stopping; the stream closes after this event. Only
    /// [`RealtimeHub::shutdown`] publishes it.
    ServerShutdown,
}

/// The closed vocabulary of one hub. Implemented only by [`PublicResource`]
/// and [`AdminResource`], so a hub cannot carry the other surface's names.
pub(crate) trait RealtimeResource:
    std::fmt::Debug + Copy + Eq + Serialize + ToSchema + Send + Sync + 'static
{
    /// OpenAPI schema of [`Invalidation::resource`]: a reference to this
    /// vocabulary or to [`ResetReason`]. Derived generics would inline the
    /// vocabulary, and the browser client needs it as its own named type.
    fn event_resource_schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::OneOfBuilder::new()
            .item(utoipa::openapi::Ref::from_schema_name(Self::name()))
            .item(utoipa::openapi::Ref::from_schema_name(ResetReason::name()))
            .description(Some(
                "The resource to refetch, or the reset reason when `reset` is true.",
            ))
            .into()
    }
}

impl RealtimeResource for PublicResource {}
impl RealtimeResource for AdminResource {}

/// The `resource` field of an Invalidation Event: the hub's own resource for
/// an invalidation, the reset reason for a reset. Both serialize as the bare
/// snake_case name, so the wire carries one string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum EventResource<R> {
    Resource(R),
    Reset(ResetReason),
}

/// One Invalidation Event (webui.md §6.3). REST stays authoritative: the event
/// only names what to refetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Invalidation<R: RealtimeResource> {
    pub version: u64,
    /// The cursor a reconnect sends back as `Last-Event-ID`.
    pub event_id: u64,
    #[schema(schema_with = R::event_resource_schema)]
    pub resource: EventResource<R>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    /// Always equal to `eventId`: the hub sequence is the one monotonic
    /// revision of a stream.
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset: Option<bool>,
}

impl<R: RealtimeResource> Invalidation<R> {
    fn reset(event_id: u64, reason: ResetReason) -> Self {
        Self {
            version: EVENT_VERSION,
            event_id,
            resource: EventResource::Reset(reason),
            resource_id: None,
            revision: event_id,
            reset: Some(true),
        }
    }

    /// The last event a stream sends before it closes.
    fn closes_stream(&self) -> bool {
        self.resource == EventResource::Reset(ResetReason::ServerShutdown)
    }
}

#[cfg(test)]
impl<R: RealtimeResource> Invalidation<R> {
    /// The `resource` string this event carries on the wire.
    pub(crate) fn wire_resource(&self) -> String {
        match serde_json::to_value(self.resource) {
            Ok(serde_json::Value::String(name)) => name,
            other => panic!("a realtime resource serializes as a string, got {other:?}"),
        }
    }
}

#[derive(Debug, Clone)]
struct StoredEvent<R: RealtimeResource> {
    event: Invalidation<R>,
}

#[derive(Debug)]
struct HubState<R: RealtimeResource> {
    next_id: u64,
    events: VecDeque<StoredEvent<R>>,
    /// Highest event ID this hub can no longer replay: removed because the
    /// bounded buffer overflowed, or issued by an earlier Server process
    /// before this hub's sequence started. Coalescing may create ID gaps
    /// without losing replay coverage.
    dropped_through: u64,
    closed: bool,
}

#[derive(Debug)]
struct HubInner<R: RealtimeResource> {
    capacity: usize,
    state: Mutex<HubState<R>>,
    notify: Notify,
    connections: AtomicU64,
}

/// A bounded invalidation stream over one surface's resources. Cloning it
/// shares the sequence and buffer.
#[derive(Clone, Debug)]
pub(crate) struct RealtimeHub<R: RealtimeResource> {
    inner: Arc<HubInner<R>>,
}

struct ConnectionGuard<R: RealtimeResource> {
    inner: Arc<HubInner<R>>,
}

impl<R: RealtimeResource> Drop for ConnectionGuard<R> {
    fn drop(&mut self) {
        self.inner.connections.fetch_sub(1, Ordering::Relaxed);
    }
}

impl<R: RealtimeResource> Default for RealtimeHub<R> {
    /// The serving hub. Its sequence starts at the wall clock so every ID it
    /// issues is above any ID an earlier process issued: the buffer is in
    /// memory, and an open tab reconnecting with a cursor from before a
    /// restart must be told to reset instead of waiting in silence until the
    /// new sequence climbs past it.
    fn default() -> Self {
        Self::starting_at(DEFAULT_BUFFER_CAPACITY, process_sequence_start())
    }
}

/// Microseconds since the Unix epoch. A process cannot issue one event per
/// microsecond of its lifetime, so the next process starts above the last ID
/// of the previous one, and the value stays a safe JavaScript integer until
/// the year 2255.
fn process_sequence_start() -> u64 {
    u64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000).unwrap_or(0)
}

/// What a bound Human stream may do after re-checking its Session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundSession {
    /// The Session is current: send this stream's events.
    Open,
    /// The Server could not read the Session: keep the connection open, send
    /// nothing, and ask again shortly (issue #267).
    Hold,
    /// The Session is over — revoked, expired, disabled, role changed.
    Close,
}

/// Re-check a bound stream's Session against the database plus the absolute
/// lifetime the Server verified when the stream was opened.
///
/// The lifetime is local on purpose: an unreadable database must not extend a
/// Session past `session_expires_at`, and holding a stream is not an
/// authorization (issue #267).
async fn bound_session(
    db: &crate::database::ServerDatabase,
    session_id: &str,
    connected_role: &str,
    session_expires_at: OffsetDateTime,
) -> BoundSession {
    if crate::auth::now_utc() > session_expires_at {
        return BoundSession::Close;
    }
    match crate::auth::session_state(db, session_id, Some(connected_role)).await {
        crate::auth::SessionState::Current => BoundSession::Open,
        crate::auth::SessionState::Ended => BoundSession::Close,
        crate::auth::SessionState::Unknown => BoundSession::Hold,
    }
}

impl<R: RealtimeResource> RealtimeHub<R> {
    #[cfg(test)]
    pub(crate) fn new(capacity: usize) -> Self {
        Self::starting_at(capacity, 0)
    }

    /// A hub whose first event ID is `start + 1`. Every cursor below `start`
    /// belongs to an earlier sequence and is answered with a reset.
    fn starting_at(capacity: usize, start: u64) -> Self {
        assert!(capacity > 0, "SSE buffer capacity must be positive");
        Self {
            inner: Arc::new(HubInner {
                capacity,
                state: Mutex::new(HubState {
                    next_id: start,
                    events: VecDeque::new(),
                    dropped_through: start,
                    closed: false,
                }),
                notify: Notify::new(),
                connections: AtomicU64::new(0),
            }),
        }
    }

    pub(crate) fn active_connections(&self) -> u64 {
        self.inner.connections.load(Ordering::Relaxed)
    }

    pub(crate) fn buffered_event_count(&self) -> u64 {
        self.inner
            .state
            .lock()
            .expect("SSE hub mutex poisoned")
            .events
            .len() as u64
    }

    /// Return the hub cursor at one instant. REST handlers capture this
    /// before querying their authoritative projection and return it to the
    /// browser so the first SSE connection can replay anything published
    /// during that query.
    pub(crate) fn current_id(&self) -> u64 {
        self.inner
            .state
            .lock()
            .expect("SSE hub mutex poisoned")
            .next_id
    }

    fn connection_guard(&self) -> ConnectionGuard<R> {
        self.inner.connections.fetch_add(1, Ordering::Relaxed);
        ConnectionGuard {
            inner: Arc::clone(&self.inner),
        }
    }

    /// A newly opened stream has already fetched its authoritative REST
    /// state, so it must not replay stale buffered invalidations. Only an
    /// explicit Last-Event-ID asks for replay after a reconnect.
    fn initial_cursor(&self, last_event_id: Option<u64>) -> u64 {
        last_event_id.unwrap_or_else(|| {
            self.inner
                .state
                .lock()
                .expect("SSE hub mutex poisoned")
                .next_id
        })
    }

    /// Publish an invalidation, coalescing the most recent event for the same
    /// `(resource, resource_id)` key. `resource_id = None` invalidates every
    /// item of the resource. Returns the event ID, which is also its revision.
    pub(crate) fn publish(&self, resource: R, resource_id: Option<impl Into<String>>) -> u64 {
        self.publish_inner(
            EventResource::Resource(resource),
            resource_id.map(Into::into),
            false,
        )
    }

    /// Publish a reset. It never carries a private Node ID.
    pub(crate) fn publish_reset(&self, reason: ResetReason) -> u64 {
        self.publish_inner(EventResource::Reset(reason), None, true)
    }

    fn publish_inner(
        &self,
        resource: EventResource<R>,
        resource_id: Option<String>,
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
            // Event IDs are the single monotonic revision for this hub; a
            // domain revision can regress or collide across Agents and
            // mutation sources.
            revision: event_id,
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
            if let Some(removed) = state.events.pop_front() {
                state.dropped_through = state.dropped_through.max(removed.event.event_id);
            }
        }
        let id = state.next_id;
        drop(state);
        self.inner.notify.notify_waiters();
        id
    }

    /// Publish the final `server_shutdown` reset and close the hub: every
    /// stream ends after sending it, and later publishes are dropped.
    pub(crate) fn shutdown(&self) {
        let mut state = self.inner.state.lock().expect("SSE hub mutex poisoned");
        if state.closed {
            return;
        }
        state.next_id = state.next_id.saturating_add(1).max(1);
        let event_id = state.next_id;
        state.events.clear();
        state.events.push_back(StoredEvent {
            event: Invalidation::reset(event_id, ResetReason::ServerShutdown),
        });
        state.closed = true;
        drop(state);
        self.inner.notify.notify_waiters();
    }
    fn next_after(&self, cursor: &mut u64) -> Option<Invalidation<R>> {
        let state = self.inner.state.lock().expect("SSE hub mutex poisoned");
        if *cursor < state.dropped_through {
            // Only an actual bounded-buffer eviction, or a cursor from an
            // earlier process, means the cursor missed authoritative
            // notifications. Event-ID gaps caused by same-resource coalescing
            // are still replayable. A stale cursor is answered even before
            // this process publishes anything.
            let first = state
                .events
                .front()
                .map_or(state.next_id, |stored| stored.event.event_id);
            *cursor = first;
            return Some(Invalidation::reset(first, ResetReason::Collection));
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
    pub(crate) fn pending_events(&self) -> Vec<Invalidation<R>> {
        self.inner
            .state
            .lock()
            .expect("SSE hub mutex poisoned")
            .events
            .iter()
            .map(|stored| stored.event.clone())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn stream(
        &self,
        last_event_id: Option<u64>,
    ) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static + use<R> {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        let hub = self.clone();
        let mut cursor = hub.initial_cursor(last_event_id);
        tokio::spawn(async move {
            let _connection = hub.connection_guard();
            loop {
                if let Some(invalidation) = hub.next_after(&mut cursor) {
                    let closing = invalidation.closes_stream();
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
    /// Build the access-reset event that closes a bound stream: an
    /// `event: reset` carrying the collection-level reset payload. Shared
    /// by the Human- and Guest-bound streams so the two authorization
    /// paths cannot drift apart.
    fn reset_event() -> Result<Event, Infallible> {
        let event = Invalidation::<R>::reset(0, ResetReason::Collection);
        let data = serde_json::to_string(&event).expect("invalidation serializes");
        Ok(Event::default().event("reset").data(data))
    }

    /// Human-bound stream: closes with a `reset` event as soon as the
    /// bound Session is no longer current — revoked, past its absolute
    /// lifetime, user disabled, or role changed
    /// (revoke/disable/role-change actively close the bound Human stream). `connected_role` is the
    /// role the stream was opened with, so any role change closes it even
    /// when the route group itself would still accept the session.
    ///
    /// A Session the database cannot answer for is neither current nor ended:
    /// the stream stays open and rechecks on the next tick, because a restart
    /// or an unreachable database must not log a reader out (issue #267).
    /// Open is not authorized, though: while the answer is missing the stream
    /// sends nothing and the cursor does not move, and the absolute lifetime
    /// `session_expires_at` (verified when the stream was opened) still ends
    /// the stream without any database answer at all.
    pub(crate) fn stream_with_session(
        &self,
        last_event_id: Option<u64>,
        db: std::sync::Arc<crate::database::ServerDatabase>,
        session_id: String,
        connected_role: String,
        session_expires_at: OffsetDateTime,
    ) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static + use<R> {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        let hub = self.clone();
        let mut cursor = hub.initial_cursor(last_event_id);
        tokio::spawn(async move {
            let _connection = hub.connection_guard();
            let mut check = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = check.tick() => {
                        if bound_session(&db, &session_id, &connected_role, session_expires_at)
                            .await
                            == BoundSession::Close
                        {
                            let _ = sender.send(Self::reset_event()).await;
                            break;
                        }
                    }
                    result = async {
                        match bound_session(&db, &session_id, &connected_role, session_expires_at)
                            .await
                        {
                            BoundSession::Close => {
                                let _ = sender.send(Self::reset_event()).await;
                                return true;
                            }
                            // The Session is neither confirmed nor revoked, so
                            // nothing may be sent on its behalf. The cursor
                            // stays where it was (no event is skipped) and the
                            // loop comes back to ask again.
                            BoundSession::Hold => {
                                let _ = tokio::time::timeout(
                                    Duration::from_secs(1),
                                    hub.inner.notify.notified(),
                                )
                                .await;
                                return false;
                            }
                            BoundSession::Open => {}
                        }
                        if let Some(invalidation) = hub.next_after(&mut cursor) {
                            let closing = invalidation.closes_stream();
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
    /// open Guest stream with a `reset` (anonymous Home
    /// closing actively closes all Guest streams).
    pub(crate) fn stream_with_guest(
        &self,
        last_event_id: Option<u64>,
        db: std::sync::Arc<crate::database::ServerDatabase>,
    ) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static + use<R> {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        let hub = self.clone();
        let mut cursor = hub.initial_cursor(last_event_id);
        tokio::spawn(async move {
            let _connection = hub.connection_guard();
            let mut check = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = check.tick() => {
                        if crate::auth::site_access_mode(&db).await.ok()
                            != Some(crate::auth::SiteAccessMode::Public)
                        {
                            let _ = sender.send(Self::reset_event()).await;
                            break;
                        }
                    }
                    result = async {
                        if crate::auth::site_access_mode(&db).await.ok()
                            != Some(crate::auth::SiteAccessMode::Public)
                        {
                            let _ = sender.send(Self::reset_event()).await;
                            return true;
                        }
                        if let Some(invalidation) = hub.next_after(&mut cursor) {
                            let closing = invalidation.closes_stream();
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

#[derive(Debug, Deserialize)]
pub(crate) struct CursorQuery {
    pub(crate) after: Option<String>,
}

/// An explicit Last-Event-ID wins over the first-stream URL cursor on
/// reconnect. Malformed values are ignored so they cannot affect stream
/// authorization or replay state.
pub(crate) fn parse_cursor(last_event_id: Option<&str>, after: Option<&str>) -> Option<u64> {
    parse_last_event_id(last_event_id).or_else(|| parse_last_event_id(after))
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
    use crate::database::ServerDatabase;
    use crate::secrets::{create_pepper_file, load_pepper_file};

    use super::*;
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn coalesces_and_replays_bounded_events() {
        let hub = RealtimeHub::<AdminResource>::new(2);
        hub.publish(AdminResource::Node, Some("a"));
        hub.publish(AdminResource::Node, Some("a"));
        let mut cursor = 0;
        let event = hub.next_after(&mut cursor).unwrap();
        assert_eq!(event.revision, 2);
        assert_eq!(event.reset, None);
        assert_eq!(event.event_id, 2);

        hub.publish(AdminResource::Node, Some("b"));
        hub.publish(AdminResource::Node, Some("c"));
        let mut cursor = 0;
        let reset = hub.next_after(&mut cursor).unwrap();
        assert_eq!(reset.reset, Some(true));
        assert_eq!(reset.resource_id, None);
    }

    #[test]
    fn coalesced_id_gaps_do_not_count_as_buffer_misses() {
        let hub = RealtimeHub::<AdminResource>::new(2);
        hub.publish(AdminResource::Node, Some("a"));
        hub.publish(AdminResource::Node, Some("a"));
        hub.publish(AdminResource::Node, Some("a"));
        let mut cursor = 0;
        let event = hub.next_after(&mut cursor).unwrap();
        assert_eq!(event.revision, 3);
        assert_eq!(event.reset, None);
    }

    #[test]
    fn an_evicted_event_requires_a_collection_reset() {
        let hub = RealtimeHub::<AdminResource>::new(2);
        hub.publish(AdminResource::Node, Some("a"));
        hub.publish(AdminResource::Node, Some("b"));
        hub.publish(AdminResource::Node, Some("c"));
        let mut cursor = 0;
        let event = hub.next_after(&mut cursor).unwrap();
        assert_eq!(event.reset, Some(true));
        assert_eq!(event.resource_id, None);
    }

    #[test]
    fn a_cursor_snapshot_replays_events_published_during_rest_query() {
        let hub = RealtimeHub::<AdminResource>::new(8);
        let cursor_at_rest_start = hub.current_id();

        hub.publish(AdminResource::Node, Some("node-a"));

        let mut cursor = cursor_at_rest_start;
        let event = hub
            .next_after(&mut cursor)
            .expect("event after REST cursor");
        assert_eq!(event.resource, EventResource::Resource(AdminResource::Node));
        assert_eq!(event.resource_id.as_deref(), Some("node-a"));
    }

    #[test]
    fn reconnect_cursor_takes_precedence_over_after_cursor() {
        assert_eq!(parse_cursor(Some("17"), Some("9")), Some(17));
        assert_eq!(parse_cursor(None, Some("9")), Some(9));
        assert_eq!(parse_cursor(None, Some("not-a-cursor")), None);
    }
    #[tokio::test]
    async fn shutdown_emits_reset_and_closes_stream() {
        let hub = RealtimeHub::<AdminResource>::new(4);
        hub.shutdown();
        let mut stream = Box::pin(hub.stream(Some(0)));
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

    #[tokio::test]
    async fn a_new_stream_skips_buffered_events_until_a_new_invalidation() {
        let hub = RealtimeHub::<AdminResource>::new(4);
        hub.publish_reset(ResetReason::SiteAccessMode);
        let mut stream = Box::pin(hub.stream(None));

        assert!(
            tokio::time::timeout(Duration::from_millis(50), stream.next())
                .await
                .is_err(),
            "a fresh stream must not replay an old reset"
        );

        hub.publish(AdminResource::Network, Some("mainnet"));
        let item = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let _ = item;
    }

    /// The sequence lives in memory, so a restarted Server starts a new one
    /// while an open tab still holds the cursor the previous process issued.
    /// That cursor must be answered with a collection reset — never with
    /// silence until the new sequence happens to climb past it.
    #[test]
    fn a_cursor_from_an_earlier_server_process_is_answered_with_a_reset() {
        let earlier = RealtimeHub::<AdminResource>::default();
        for node in 0..40 {
            earlier.publish(AdminResource::Node, Some(format!("node-{node}")));
        }
        let kept_cursor = earlier.current_id();
        earlier.shutdown();
        // A restart takes far longer than the sequence's clock tick.
        std::thread::sleep(Duration::from_millis(2));

        let restarted = RealtimeHub::<AdminResource>::default();
        // Nothing published yet: the stale cursor is answered right away.
        let mut cursor = kept_cursor;
        let event = restarted
            .next_after(&mut cursor)
            .expect("a stale cursor is answered before any new event");
        assert_eq!(event.reset, Some(true));
        assert_eq!(event.resource_id, None);

        restarted.publish(AdminResource::Network, Some("platon-mainnet"));
        let mut cursor = kept_cursor;
        let event = restarted
            .next_after(&mut cursor)
            .expect("a stale cursor is answered once the new process publishes");
        assert_eq!(event.reset, Some(true));
        assert_eq!(event.resource_id, None);

        // The browser refetches REST after the reset and resumes from the new
        // process's own cursor, which is ahead of the one it kept.
        assert!(restarted.current_id() > kept_cursor);
        let mut cursor = restarted.current_id();
        restarted.publish(AdminResource::Node, Some("node-0"));
        let event = restarted
            .next_after(&mut cursor)
            .expect("the new process's cursor replays its own events");
        assert_eq!(event.reset, None);
        assert_eq!(event.resource_id.as_deref(), Some("node-0"));
    }

    /// The closed vocabulary changes the Rust types, never the wire: an open
    /// tab keeps parsing the same JSON.
    #[test]
    fn events_keep_their_wire_shape() {
        let hub = RealtimeHub::<PublicResource>::new(8);
        hub.publish(PublicResource::RewardWallet, None::<String>);
        hub.publish(PublicResource::Node, Some("node-a"));
        hub.publish_reset(ResetReason::SiteAccessMode);
        let wire = |hub: &RealtimeHub<PublicResource>| {
            hub.pending_events()
                .iter()
                .map(|event| serde_json::to_value(event).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            wire(&hub),
            vec![
                serde_json::json!({"version": 1, "eventId": 1, "resource": "reward_wallet", "revision": 1}),
                serde_json::json!({"version": 1, "eventId": 2, "resource": "node", "resourceId": "node-a", "revision": 2}),
                serde_json::json!({"version": 1, "eventId": 3, "resource": "site_access_mode", "revision": 3, "reset": true}),
            ]
        );

        hub.shutdown();
        assert_eq!(
            wire(&hub),
            vec![
                serde_json::json!({"version": 1, "eventId": 4, "resource": "server_shutdown", "revision": 4, "reset": true})
            ]
        );
    }

    #[test]
    fn reset_never_contains_a_resource_id() {
        let hub = RealtimeHub::<AdminResource>::new(4);
        hub.publish_reset(ResetReason::Collection);
        let mut cursor = 0;
        let event = hub.next_after(&mut cursor).unwrap();
        assert_eq!(event.reset, Some(true));
        assert_eq!(event.resource_id, None);
        assert_eq!(parse_last_event_id(Some("12")), Some(12));
        assert_eq!(parse_last_event_id(Some("bad")), None);
    }

    async fn stream_db() -> (tempfile::TempDir, Arc<ServerDatabase>, AuthConfig) {
        let dir = tempdir().unwrap();
        let db = Arc::new(crate::test_support::test_database_in(dir.path()).await);
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
    /// (issue #47: Session revoke closes the associated
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
        let hub = RealtimeHub::<AdminResource>::new(8);
        let mut stream = Box::pin(hub.stream_with_session(
            None,
            Arc::clone(&db),
            session.session_id.clone(),
            session.role.clone(),
            session.expires_at,
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
            crate::auth::session_state(&db, &session.session_id, Some(&session.role)).await
                == crate::auth::SessionState::Ended,
            "the revoked session must no longer be current"
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(3), stream.next())
                .await
                .expect("stream must close after reset")
                .is_none()
        );
    }

    /// Issue #267: a database that cannot be read must not reset a bound
    /// stream. The Session is unverified, not revoked, so the stream stays
    /// open and rechecks on the next tick.
    #[tokio::test]
    async fn unreadable_database_keeps_the_bound_stream_open() {
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
        let hub = RealtimeHub::<AdminResource>::new(8);
        let mut stream = Box::pin(hub.stream_with_session(
            None,
            Arc::clone(&db),
            session.session_id.clone(),
            session.role.clone(),
            session.expires_at,
        ));
        db.pool().close().await;
        let item = tokio::time::timeout(Duration::from_millis(2_500), stream.next()).await;
        assert!(
            item.is_err(),
            "an unreadable database must not end the stream with a reset event"
        );
        assert_eq!(
            crate::auth::session_state(&db, &session.session_id, Some(&session.role)).await,
            crate::auth::SessionState::Unknown,
            "the recheck must report the Session as unverified, not ended"
        );
    }

    /// Issue #267: holding a stream is not an authorization. A Session whose
    /// absolute lifetime has passed ends the bound stream even when the
    /// database cannot be read at all, because the lifetime was verified when
    /// the stream was opened and is re-checked locally on every tick.
    #[tokio::test]
    async fn expired_session_closes_the_bound_stream_without_a_database_answer() {
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
        let hub = RealtimeHub::<AdminResource>::new(8);
        let mut stream = Box::pin(hub.stream_with_session(
            None,
            Arc::clone(&db),
            session.session_id.clone(),
            session.role.clone(),
            // The lifetime the Server verified when this stream was opened,
            // already past — as if the stream had been open long enough.
            crate::auth::now_utc() - time::Duration::seconds(1),
        ));
        db.pool().close().await;
        let item = tokio::time::timeout(Duration::from_secs(3), stream.next())
            .await
            .expect("an expired Session must produce a reset event")
            .expect("stream must stay open");
        assert!(
            item.is_ok(),
            "an expired Session must end the stream even while the database is unreadable"
        );
    }

    /// A role change must close a bound Human stream even when the route
    /// group itself would still accept the session.
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
        let hub = RealtimeHub::<AdminResource>::new(8);
        let mut stream = Box::pin(hub.stream_with_session(
            None,
            Arc::clone(&db),
            session.session_id.clone(),
            session.role.clone(),
            session.expires_at,
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
            crate::auth::session_state(&db, &session.session_id, Some(&session.role)).await
                == crate::auth::SessionState::Ended,
            "the changed role must no longer satisfy the bound role"
        );
    }

    /// Disabling anonymous Home must close every open Guest stream with a
    /// reset (anonymous Home closing closes Guest streams).
    #[tokio::test]
    async fn guest_stream_closes_when_anonymous_home_is_disabled() {
        let (_dir, db, _config) = stream_db().await;
        sqlx::query(
            "UPDATE server_settings SET setting_value = 'public' WHERE setting_key = 'site_access_mode'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let hub = RealtimeHub::<AdminResource>::new(8);
        let mut stream = Box::pin(hub.stream_with_guest(None, Arc::clone(&db)));
        sqlx::query(
            "UPDATE server_settings SET setting_value = 'private' WHERE setting_key = 'site_access_mode'",
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
            crate::auth::site_access_mode(&db).await.unwrap()
                == crate::auth::SiteAccessMode::Private,
            "anonymous Home must be disabled again"
        );
    }
}
