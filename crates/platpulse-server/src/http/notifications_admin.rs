//! Owner-only Notification operations (issue #49, design §17.4/§17.5,
//! webui.md PAGE-ADMIN-DELIVERIES / PAGE-ADMIN-DELIVERY /
//! PAGE-ADMIN-CHANNELS).
//!
//! Notification Events and per-channel Delivery attempts are separate and
//! durable; Deliveries show bounded retry/backoff, Retry-After, attempt
//! history, provider results, and DeadLetter outcome. Manual retry re-arms
//! the same Delivery row -- it never creates a duplicate Event, Incident,
//! or business transition -- and duplicate parallel retries are refused by
//! the Server. Destinations and provider references are redacted by
//! construction; provider tokens never enter DTOs, logs, or Audit bodies.
//! Every mutation revalidates the browser trust boundary (JSON content
//! type, exact Origin, session CSRF), commits atomically with its Audit
//! row, and publishes an Admin invalidation so other Owner tabs refetch
//! authoritative REST. All reads inside a transaction use the transaction
//! handle: the Server pool has one connection, so pool queries inside a
//! transaction would deadlock.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::now_utc;
use crate::config::NotificationChannels;
use crate::http::admin::{mutation_error, mutation_guard};
use crate::http::{AppState, AuthenticatedSession, RequestId};
use crate::notifications::{
    AttemptRow, DeliveryRow, EventRow, RetryError, TestSendError, attempts_for_delivery,
    deliveries_for_event, load_delivery, load_event, provider_reference, rearm_delivery,
    redact_destination, send_test_delivery,
};

const MAX_PAGE: i64 = 100;

fn parse_page(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, MAX_PAGE)
}

fn validate_event_kind(kind: &Option<String>) -> Result<(), String> {
    match kind.as_deref() {
        None | Some("incident") | Some("test") => Ok(()),
        Some(_) => Err("`eventKind` must be incident or test".to_owned()),
    }
}

fn validate_delivery_state(state: &Option<String>) -> Result<(), String> {
    match state.as_deref() {
        None => Ok(()),
        Some(value) if crate::notifications::DELIVERY_STATES.contains(&value) => Ok(()),
        Some(_) => Err(format!(
            "`state` must be one of {}",
            crate::notifications::DELIVERY_STATES.join(", ")
        )),
    }
}

fn validate_channel(channel: &Option<String>) -> Result<(), String> {
    match channel.as_deref() {
        None | Some("telegram") => Ok(()),
        Some(_) => Err("`channel` must be telegram".to_owned()),
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeliverySummary {
    pub delivery_id: String,
    pub channel_kind: String,
    pub destination: String,
    pub state: String,
    pub attempt_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationEventItem {
    #[serde(flatten)]
    pub event: EventRow,
    pub deliveries: Vec<DeliverySummary>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationEventsResponse {
    pub items: Vec<NotificationEventItem>,
    pub next_before: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationEventDetail {
    #[serde(flatten)]
    pub event: EventRow,
    pub deliveries: Vec<DeliveryRow>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationDeliveriesResponse {
    pub items: Vec<DeliveryRow>,
    pub next_before: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationDeliveryDetail {
    #[serde(flatten)]
    pub delivery: DeliveryRow,
    pub attempts: Vec<AttemptRow>,
    pub event: EventRow,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelDto {
    pub channel_id: String,
    pub channel_kind: String,
    pub enabled: bool,
    /// Redacted destination summary (last four characters only).
    pub destination: String,
    /// Redacted provider reference (secret file base name only).
    pub provider_ref: String,
    pub max_attempts: u32,
    pub retry_base_seconds: u32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryRetryResponse {
    #[serde(flatten)]
    pub delivery: DeliveryRow,
    pub audit_event_id: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelTestResponse {
    pub event_id: String,
    #[serde(flatten)]
    pub delivery: DeliveryRow,
    pub audit_event_id: i64,
}

fn channel_dto(channels: &NotificationChannels) -> Option<ChannelDto> {
    let telegram = channels.telegram()?;
    Some(ChannelDto {
        channel_id: "telegram".to_owned(),
        channel_kind: "telegram".to_owned(),
        enabled: telegram.enabled,
        destination: redact_destination(&telegram.chat_id),
        provider_ref: provider_reference(&telegram.token_file),
        max_attempts: telegram.max_attempts,
        retry_base_seconds: telegram.retry_base_seconds,
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// List Notification Events (newest first) with their per-channel Delivery
/// summaries. Events are durable business records independent of Delivery
/// outcomes and remain visible across browser navigation.
#[utoipa::path(
    get,
    path = "/api/admin/v1/notifications/events",
    tag = "admin",
    params(
        ("event_kind" = Option<String>, Query, description = "Filter by event kind (incident, test)"),
        ("before" = Option<String>, Query, description = "Opaque keyset cursor from a previous page"),
        ("limit" = Option<i64>, Query, description = "Page size (1-100, default 50)"),
    ),
    responses((status = 200, body = NotificationEventsResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn notification_events(
    State(state): State<AppState>,
    Query(params): Query<NotificationEventsQuery>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if validate_event_kind(&params.event_kind).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_fields(
                "invalid_query",
                "invalid `eventKind` filter",
                &request_id.0,
                vec!["eventKind".to_owned()],
            )),
        )
            .into_response();
    }
    let limit = parse_page(params.limit);
    let rows = sqlx::query_as::<_, EventRow>(
        "SELECT event_id, event_kind, incident_id, rule_key, subject_kind, subject_key, severity, summary, created_at FROM notification_events WHERE (?1 IS NULL OR event_kind = ?1) AND (?2 IS NULL OR ?2 = '' OR (created_at, event_id) < (SELECT created_at, event_id FROM notification_events WHERE event_id = ?2)) ORDER BY created_at DESC, event_id DESC LIMIT ?3",
    )
    .bind(params.event_kind.as_deref())
    .bind(params.before.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await;
    let events = match rows {
        Ok(events) => events,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let event_ids: Vec<&str> = events.iter().map(|event| event.event_id.as_str()).collect();
    let mut items = Vec::with_capacity(events.len());
    if !event_ids.is_empty() {
        let placeholders = std::iter::repeat_n("?", event_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let query = format!(
            "SELECT delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at FROM notification_deliveries WHERE event_id IN ({placeholders}) ORDER BY created_at"
        );
        let mut builder = sqlx::query_as::<_, DeliveryRow>(&query);
        for event_id in &event_ids {
            builder = builder.bind(event_id);
        }
        let deliveries = match builder.fetch_all(state.db().pool()).await {
            Ok(rows) => rows,
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        for event in events {
            let event_deliveries: Vec<DeliverySummary> = deliveries
                .iter()
                .filter(|delivery| delivery.event_id == event.event_id)
                .map(|delivery| DeliverySummary {
                    delivery_id: delivery.delivery_id.clone(),
                    channel_kind: delivery.channel_kind.clone(),
                    destination: delivery.destination.clone(),
                    state: delivery.state.clone(),
                    attempt_count: delivery.attempt_count,
                })
                .collect();
            items.push(NotificationEventItem {
                event,
                deliveries: event_deliveries,
            });
        }
    }
    let next_before = (items.len() as i64 == limit)
        .then(|| items.last().map(|item| item.event.event_id.clone()))
        .flatten();
    Json(NotificationEventsResponse { items, next_before }).into_response()
}

/// One Notification Event with its full per-channel Deliveries.
#[utoipa::path(
    get,
    path = "/api/admin/v1/notifications/events/{event_id}",
    tag = "admin",
    params(("event_id" = String, Path, description = "Notification Event ID")),
    responses((status = 200, body = NotificationEventDetail), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn notification_event_detail(
    State(state): State<AppState>,
    Path(event_id): Path<String>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let mut conn = match state.db().pool().acquire().await {
        Ok(conn) => conn,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let event = match load_event(&mut conn, &event_id).await {
        Ok(Some(event)) => event,
        Ok(None) => {
            return mutation_error(
                &request_id.0,
                StatusCode::NOT_FOUND,
                "notification_event_not_found",
                "unknown Notification Event",
            );
        }
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let deliveries = match deliveries_for_event(&mut conn, &event_id).await {
        Ok(deliveries) => deliveries,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    Json(NotificationEventDetail { event, deliveries }).into_response()
}

/// List Notification Deliveries (the Outbox) with retry/dead-letter
/// filters. Every row carries its own per-channel state, so one failed
/// destination never erases successful Delivery state.
#[utoipa::path(
    get,
    path = "/api/admin/v1/notifications/deliveries",
    tag = "admin",
    params(
        ("state" = Option<String>, Query, description = "Delivery state filter (pending, retry_scheduled, succeeded, failed, dead_letter, suppressed, in_flight)"),
        ("channel" = Option<String>, Query, description = "Channel filter (telegram)"),
        ("before" = Option<String>, Query, description = "Opaque keyset cursor from a previous page"),
        ("limit" = Option<i64>, Query, description = "Page size (1-100, default 50)"),
    ),
    responses((status = 200, body = NotificationDeliveriesResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn notification_deliveries(
    State(state): State<AppState>,
    Query(params): Query<NotificationDeliveriesQuery>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if validate_delivery_state(&params.state).is_err() {
        let field = params.state.clone().unwrap_or_default();
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_fields(
                "invalid_query",
                "invalid `state` filter",
                &request_id.0,
                vec![field],
            )),
        )
            .into_response();
    }
    if validate_channel(&params.channel).is_err() {
        let field = params.channel.clone().unwrap_or_default();
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_fields(
                "invalid_query",
                "invalid `channel` filter",
                &request_id.0,
                vec![field],
            )),
        )
            .into_response();
    }
    let limit = parse_page(params.limit);
    let rows = sqlx::query_as::<_, DeliveryRow>(
        "SELECT delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at FROM notification_deliveries WHERE (?1 IS NULL OR state = ?1) AND (?2 IS NULL OR channel_kind = ?2) AND (?3 IS NULL OR ?3 = '' OR (created_at, delivery_id) < (SELECT created_at, delivery_id FROM notification_deliveries WHERE delivery_id = ?3)) ORDER BY created_at DESC, delivery_id DESC LIMIT ?4",
    )
    .bind(params.state.as_deref())
    .bind(params.channel.as_deref())
    .bind(params.before.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await;
    let items = match rows {
        Ok(items) => items,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let next_before = (items.len() as i64 == limit)
        .then(|| items.last().map(|item| item.delivery_id.clone()))
        .flatten();
    Json(NotificationDeliveriesResponse { items, next_before }).into_response()
}

/// One Delivery with its redacted destination, attempt history, provider
/// results, Retry-After, and DeadLetter outcome.
#[utoipa::path(
    get,
    path = "/api/admin/v1/notifications/deliveries/{delivery_id}",
    tag = "admin",
    params(("delivery_id" = String, Path, description = "Notification Delivery ID")),
    responses((status = 200, body = NotificationDeliveryDetail), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn notification_delivery_detail(
    State(state): State<AppState>,
    Path(delivery_id): Path<String>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let mut conn = match state.db().pool().acquire().await {
        Ok(conn) => conn,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let delivery = match load_delivery(&mut conn, &delivery_id).await {
        Ok(Some(delivery)) => delivery,
        Ok(None) => {
            return mutation_error(
                &request_id.0,
                StatusCode::NOT_FOUND,
                "notification_delivery_not_found",
                "unknown Notification Delivery",
            );
        }
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let attempts = match attempts_for_delivery(&mut conn, &delivery_id).await {
        Ok(attempts) => attempts,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let event = match load_event(&mut conn, &delivery.event_id).await {
        Ok(Some(event)) => event,
        Ok(None) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Notification Event is missing",
            );
        }
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    Json(NotificationDeliveryDetail {
        delivery,
        attempts,
        event,
    })
    .into_response()
}

/// Manual retry: re-arms one Delivery for the worker. It creates a new
/// Delivery attempt on the next worker pass but never a new Notification
/// Event, Incident, or business transition. Duplicate parallel retries are
/// refused (409 `delivery_already_queued`); suppressed and succeeded
/// Deliveries are not retryable (409 `delivery_not_retryable`).
#[utoipa::path(
    post,
    path = "/api/admin/v1/notifications/deliveries/{delivery_id}/retry",
    tag = "admin",
    params(("delivery_id" = String, Path, description = "Notification Delivery ID")),
    responses((status = 200, body = DeliveryRetryResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn retry_delivery(
    State(state): State<AppState>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, false) {
        return response;
    }
    let now = now_utc();
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let delivery = match rearm_delivery(&mut tx, &delivery_id, now).await {
        Ok(delivery) => delivery,
        Err(RetryError::NotFound) => {
            let _ = tx.rollback().await;
            return mutation_error(
                &request_id.0,
                StatusCode::NOT_FOUND,
                "notification_delivery_not_found",
                "unknown Notification Delivery",
            );
        }
        Err(RetryError::AlreadyQueued) => {
            let _ = tx.rollback().await;
            return (
                StatusCode::CONFLICT,
                Json(crate::http::ApiErrorBody::with_fields(
                    "delivery_already_queued",
                    "a retry for this Delivery is already queued or in flight",
                    &request_id.0,
                    vec!["deliveryId".to_owned()],
                )),
            )
                .into_response();
        }
        Err(RetryError::NotRetryable { state: _ }) => {
            let _ = tx.rollback().await;
            return (
                StatusCode::CONFLICT,
                Json(crate::http::ApiErrorBody::with_fields(
                    "delivery_not_retryable",
                    "this Delivery cannot be retried",
                    &request_id.0,
                    vec!["deliveryId".to_owned(), "state".to_owned()],
                )),
            )
                .into_response();
        }
    };
    if crate::auth::insert_audit_event(
        &mut *tx,
        Some(&principal.0.user_id),
        "notification_delivery_retried",
        "notification_delivery",
        &delivery_id,
        Some(&serde_json::json!({
            "deliveryId": delivery_id,
            "state": delivery.state,
        })),
    )
    .await
    .is_err()
    {
        let _ = tx.rollback().await;
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
            let _ = tx.rollback().await;
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if tx.commit().await.is_err() {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    state
        .admin_realtime()
        .publish("notifications", None::<String>, 0);
    Json(DeliveryRetryResponse {
        delivery,
        audit_event_id,
    })
    .into_response()
}

/// List the configured notification channels with redacted destination
/// summaries and provider references. Policy (retry bound, backoff base)
/// is Server config, not browser state.
#[utoipa::path(
    get,
    path = "/api/admin/v1/notifications/channels",
    tag = "admin",
    responses((status = 200, body = Vec<ChannelDto>), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn notification_channels(
    State(state): State<AppState>,
    Extension(_session): Extension<AuthenticatedSession>,
) -> Response {
    let mut channels: Vec<ChannelDto> = Vec::new();
    if let Some(channel) = channel_dto(state.channels()) {
        channels.push(channel);
    }
    Json(channels).into_response()
}

/// One configured channel: policy, redacted destination, redacted provider
/// reference.
#[utoipa::path(
    get,
    path = "/api/admin/v1/notifications/channels/{channel_id}",
    tag = "admin",
    params(("channel_id" = String, Path, description = "Channel ID (telegram)")),
    responses((status = 200, body = ChannelDto), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn notification_channel_detail(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if channel_id != "telegram" {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "channel_not_configured",
            "unknown notification channel",
        );
    }
    match channel_dto(state.channels()) {
        Some(channel) => Json(channel).into_response(),
        None => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "channel_not_configured",
            "this notification channel is not configured",
        ),
    }
}

/// Send a test notification through a channel. Test Notifications are
/// clearly separate from business Incidents (event kind `test`), always
/// produce an Audit Event, and send synchronously so the response carries
/// the resulting Delivery state. Provider tokens never enter the request,
/// response, Audit body, or logs.
#[utoipa::path(
    post,
    path = "/api/admin/v1/notifications/channels/{channel_id}/test",
    tag = "admin",
    params(("channel_id" = String, Path, description = "Channel ID (telegram)")),
    responses((status = 200, body = ChannelTestResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn test_notification_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, false) {
        return response;
    }
    if channel_id != "telegram" {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "channel_not_configured",
            "unknown notification channel",
        );
    }
    let summary = format!("Test notification via {channel_id}");
    let result = send_test_delivery(&state, &*state.delivery_provider(), "info", &summary).await;
    let (event_id, delivery) = match result {
        Ok(value) => value,
        Err(TestSendError::NotConfigured) => {
            return mutation_error(
                &request_id.0,
                StatusCode::NOT_FOUND,
                "channel_not_configured",
                "this notification channel is not configured",
            );
        }
        Err(TestSendError::Disabled) => {
            return (
                StatusCode::CONFLICT,
                Json(crate::http::ApiErrorBody::with_fields(
                    "channel_disabled",
                    "this notification channel is disabled",
                    &request_id.0,
                    vec!["channelId".to_owned()],
                )),
            )
                .into_response();
        }
        Err(TestSendError::Unavailable) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if crate::auth::insert_audit_event(
        &mut *tx,
        Some(&principal.0.user_id),
        "notification_test_sent",
        "notification_event",
        &event_id,
        Some(&serde_json::json!({
            "channel": channel_id,
            "eventId": event_id,
            "deliveryId": delivery.delivery_id,
            "state": delivery.state,
        })),
    )
    .await
    .is_err()
    {
        let _ = tx.rollback().await;
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
            let _ = tx.rollback().await;
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if tx.commit().await.is_err() {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    state
        .admin_realtime()
        .publish("notifications", None::<String>, 0);
    Json(ChannelTestResponse {
        event_id,
        delivery,
        audit_event_id,
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct NotificationEventsQuery {
    pub event_kind: Option<String>,
    pub before: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct NotificationDeliveriesQuery {
    pub state: Option<String>,
    pub channel: Option<String>,
    pub before: Option<String>,
    pub limit: Option<i64>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/notifications/events", get(notification_events))
        .route(
            "/notifications/events/{event_id}",
            get(notification_event_detail),
        )
        .route("/notifications/deliveries", get(notification_deliveries))
        .route(
            "/notifications/deliveries/{delivery_id}",
            get(notification_delivery_detail),
        )
        .route(
            "/notifications/deliveries/{delivery_id}/retry",
            axum::routing::post(retry_delivery),
        )
        .route("/notifications/channels", get(notification_channels))
        .route(
            "/notifications/channels/{channel_id}",
            get(notification_channel_detail),
        )
        .route(
            "/notifications/channels/{channel_id}/test",
            axum::routing::post(test_notification_channel),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::extract::Extension;
    use axum::http::header;
    use serde_json::Value;
    use tempfile::tempdir;
    use time::OffsetDateTime;
    use time::macros::datetime;

    fn base_time() -> OffsetDateTime {
        datetime!(2026-03-01 00:00:00 UTC)
    }

    struct FakeProvider {
        results: std::sync::Mutex<Vec<Result<(), crate::notifications::SendError>>>,
        texts: std::sync::Mutex<Vec<String>>,
    }

    impl FakeProvider {
        fn new(results: Vec<Result<(), crate::notifications::SendError>>) -> Self {
            Self {
                results: std::sync::Mutex::new(results),
                texts: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::notifications::DeliveryProvider for FakeProvider {
        async fn send(
            &self,
            channel: &crate::config::TelegramChannel,
            text: &str,
        ) -> Result<(), crate::notifications::SendError> {
            let _ = channel;
            self.texts.lock().expect("texts").push(text.to_owned());
            let mut results = self.results.lock().expect("results");
            if results.is_empty() {
                return Ok(());
            }
            results.remove(0)
        }
    }

    async fn test_state() -> (tempfile::TempDir, AppState, std::sync::Arc<FakeProvider>) {
        test_state_with_channels(crate::config::NotificationChannels {
            telegram: Some(crate::config::TelegramChannel {
                enabled: true,
                token_file: tempfile::tempdir().unwrap().path().join("telegram-token"),
                chat_id: "123456789".to_owned(),
                max_attempts: 3,
                retry_base_seconds: 60,
            }),
        })
        .await
    }

    async fn test_state_with_channels(
        channels: crate::config::NotificationChannels,
    ) -> (tempfile::TempDir, AppState, std::sync::Arc<FakeProvider>) {
        let dir = tempdir().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        std::fs::write(dir.path().join("telegram-token"), "fake-token\n").unwrap();
        let provider = std::sync::Arc::new(FakeProvider::new(vec![Err(
            crate::notifications::SendError::Api {
                code: 429,
                retry_after: Some(5),
            },
        )]));
        let state =
            AppState::new_with_proxy_policy(database, None, auth, Vec::new(), None, channels)
                .with_delivery_provider(provider.clone());
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(state.db().pool()).await.unwrap();
        (dir, state, provider)
    }

    fn session() -> AuthenticatedSession {
        AuthenticatedSession(crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        })
    }

    fn mutation_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        headers.insert(header::ORIGIN, "http://127.0.0.1:8080".parse().unwrap());
        headers.insert("x-csrf-token", "csrf".parse().unwrap());
        headers
    }

    fn request_id() -> RequestId {
        RequestId(std::sync::Arc::from("req-123"))
    }

    async fn body_json(response: Response) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    async fn seed_event(pool: &sqlx::SqlitePool) -> String {
        sqlx::query("INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, opened_evidence_json) VALUES ('inc-1', 'node.rpc_unreachable', 1, 'node', 'node-a', 'critical', 'open', '2026-03-01T00:00:00Z', '2026-03-01T00:00:00Z', '{}') ON CONFLICT(incident_id) DO NOTHING")
            .execute(pool)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let channels = crate::config::NotificationChannels {
            telegram: Some(crate::config::TelegramChannel {
                enabled: true,
                token_file: "unused".into(),
                chat_id: "123456789".to_owned(),
                max_attempts: 3,
                retry_base_seconds: 60,
            }),
        };
        crate::notifications::record_notification_event(
            &mut conn,
            crate::notifications::NotificationEventInput {
                kind: "incident",
                incident_id: Some("inc-1"),
                rule_key: Some("node.rpc_unreachable"),
                subject: Some((crate::alerts::SubjectKind::Node, "node-a")),
                severity: "critical",
                summary: "Incident opened: node.rpc_unreachable on node node-a",
            },
            &channels,
            base_time(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn events_list_and_detail_are_durable_and_group_deliveries() {
        let (_dir, state, _provider) = test_state().await;
        let event_id = seed_event(state.db().pool()).await;

        let response = notification_events(
            State(state.clone()),
            Query(NotificationEventsQuery {
                event_kind: None,
                before: None,
                limit: None,
            }),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        let items = value["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["eventId"], event_id);
        assert_eq!(items[0]["eventKind"], "incident");
        assert_eq!(items[0]["deliveries"][0]["state"], "pending");
        assert_eq!(items[0]["deliveries"][0]["destination"], "****6789");

        let response = notification_event_detail(
            State(state.clone()),
            Path(event_id.clone()),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["eventId"], event_id);
        assert_eq!(value["deliveries"].as_array().unwrap().len(), 1);
        assert!(!value.to_string().contains("123456789"));
        assert!(value.to_string().contains("****6789"));
    }

    #[tokio::test]
    async fn events_list_validates_event_kind_filter() {
        let (_dir, state, _provider) = test_state().await;
        let response = notification_events(
            State(state),
            Query(NotificationEventsQuery {
                event_kind: Some("bogus".to_owned()),
                before: None,
                limit: None,
            }),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn deliveries_list_filters_states_and_redacts_destinations() {
        let (_dir, state, _provider) = test_state().await;
        let event_id = seed_event(state.db().pool()).await;
        let delivery_id = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::deliveries_for_event(&mut conn, &event_id)
                .await
                .unwrap()[0]
                .delivery_id
                .clone()
        };
        sqlx::query("UPDATE notification_deliveries SET state = 'dead_letter', attempt_count = 3, last_result = 'telegram_api_error 429' WHERE delivery_id = ?")
            .bind(&delivery_id)
            .execute(state.db().pool())
            .await
            .unwrap();

        let response = notification_deliveries(
            State(state.clone()),
            Query(NotificationDeliveriesQuery {
                state: Some("dead_letter".to_owned()),
                channel: None,
                before: None,
                limit: None,
            }),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        let items = value["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["deliveryId"], delivery_id);
        assert_eq!(items[0]["state"], "dead_letter");
        assert_eq!(items[0]["lastResult"], "telegram_api_error 429");
        assert!(!value.to_string().contains("123456789"));

        let response = notification_deliveries(
            State(state),
            Query(NotificationDeliveriesQuery {
                state: Some("succeeded".to_owned()),
                channel: None,
                before: None,
                limit: None,
            }),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert!(value["items"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn deliveries_list_rejects_unknown_state() {
        let (_dir, state, _provider) = test_state().await;
        let response = notification_deliveries(
            State(state),
            Query(NotificationDeliveriesQuery {
                state: Some("bogus".to_owned()),
                channel: None,
                before: None,
                limit: None,
            }),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let value = body_json(response).await;
        assert_eq!(value["error"]["fields"][0], "bogus");
    }

    #[tokio::test]
    async fn delivery_detail_shows_attempts_and_redacted_destination() {
        let (_dir, state, _provider) = test_state().await;
        let event_id = seed_event(state.db().pool()).await;
        let delivery_id = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::deliveries_for_event(&mut conn, &event_id)
                .await
                .unwrap()[0]
                .delivery_id
                .clone()
        };
        sqlx::query(
            "INSERT INTO delivery_attempts (attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds) VALUES ('att-1', ?, 1, '2026-03-01T00:00:05Z', 'failed', 'telegram_api_error 429', 'telegram_api', 120, 5)",
        )
        .bind(&delivery_id)
        .execute(state.db().pool())
        .await
        .unwrap();

        let response = notification_delivery_detail(
            State(state),
            Path(delivery_id),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(
            value["attempts"][0]["providerResult"],
            "telegram_api_error 429"
        );
        assert_eq!(value["attempts"][0]["retryAfterSeconds"], 5);
        assert_eq!(
            value["event"]["summary"],
            "Incident opened: node.rpc_unreachable on node node-a"
        );
        assert!(!value.to_string().contains("123456789"));
    }

    #[tokio::test]
    async fn manual_retry_rearms_without_duplicating_and_audits() {
        let (_dir, state, _provider) = test_state().await;
        let event_id = seed_event(state.db().pool()).await;
        let delivery_id = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::deliveries_for_event(&mut conn, &event_id)
                .await
                .unwrap()[0]
                .delivery_id
                .clone()
        };
        sqlx::query("UPDATE notification_deliveries SET state = 'dead_letter', attempt_count = 3 WHERE delivery_id = ?")
            .bind(&delivery_id)
            .execute(state.db().pool())
            .await
            .unwrap();

        let response = retry_delivery(
            State(state.clone()),
            Path(delivery_id.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["deliveryId"], delivery_id);
        assert_eq!(value["state"], "pending");
        assert!(value["auditEventId"].as_i64().unwrap() > 0);

        // The Event count is unchanged: retry never duplicates the Event.
        let events = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notification_events")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(events, 1);
        let deliveries =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notification_deliveries")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(deliveries, 1);

        // Audit row exists with redacted body (no token, no chat id).
        let audit = sqlx::query_scalar::<_, String>(
            "SELECT after_json FROM audit_events WHERE event_kind = 'notification_delivery_retried'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(!audit.contains("123456789"));
        assert!(audit.contains("deliveryId"));
    }

    #[tokio::test]
    async fn duplicate_parallel_retries_are_refused() {
        let (_dir, state, _provider) = test_state().await;
        let event_id = seed_event(state.db().pool()).await;
        let delivery_id = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::deliveries_for_event(&mut conn, &event_id)
                .await
                .unwrap()[0]
                .delivery_id
                .clone()
        };
        // pending = already queued → 409 even before any worker pass.
        let response = retry_delivery(
            State(state.clone()),
            Path(delivery_id.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let value = body_json(response).await;
        assert_eq!(value["error"]["code"], "delivery_already_queued");
        assert_eq!(value["error"]["fields"][0], "deliveryId");

        // suppressed is not retryable.
        sqlx::query(
            "UPDATE notification_deliveries SET state = 'suppressed' WHERE delivery_id = ?",
        )
        .bind(&delivery_id)
        .execute(state.db().pool())
        .await
        .unwrap();
        let response = retry_delivery(
            State(state),
            Path(delivery_id),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let value = body_json(response).await;
        assert_eq!(value["error"]["code"], "delivery_not_retryable");
        assert_eq!(value["error"]["fields"][1], "state");
    }

    #[tokio::test]
    async fn test_notification_is_separate_from_incidents_and_audited() {
        let (_dir, state, provider) = test_state().await;
        let response = test_notification_channel(
            State(state.clone()),
            Path("telegram".to_owned()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["state"], "failed");
        assert_eq!(value["lastResult"], "telegram_api_error 429");
        assert_eq!(value["lastErrorKind"], "telegram_api");
        assert_eq!(value["retryAfterSeconds"], 5);
        let event_id = value["eventId"].as_str().unwrap().to_owned();

        let event = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::load_event(&mut conn, &event_id)
                .await
                .unwrap()
                .unwrap()
        };
        assert_eq!(event.event_kind, "test");
        assert_eq!(event.incident_id, None);
        // No Incident was created by the test action.
        let incidents = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM alert_incidents")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(incidents, 0);
        // The provider saw exactly one test message.
        assert_eq!(provider.texts.lock().unwrap().len(), 1);
        assert!(provider.texts.lock().unwrap()[0].contains("TEST"));
        assert!(!provider.texts.lock().unwrap()[0].contains("fake-token"));

        let audit = sqlx::query_scalar::<_, String>(
            "SELECT after_json FROM audit_events WHERE event_kind = 'notification_test_sent'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(!audit.contains("fake-token"));
        assert!(!audit.contains("123456789"));
        assert!(audit.contains("\"state\":\"failed\""));
    }

    #[tokio::test]
    async fn test_notification_disabled_channel_is_conflict() {
        let (_dir, state, _provider) =
            test_state_with_channels(crate::config::NotificationChannels {
                telegram: Some(crate::config::TelegramChannel {
                    enabled: false,
                    token_file: "/unused".into(),
                    chat_id: "123456789".to_owned(),
                    max_attempts: 3,
                    retry_base_seconds: 60,
                }),
            })
            .await;
        let response = test_notification_channel(
            State(state.clone()),
            Path("telegram".to_owned()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let value = body_json(response).await;
        assert_eq!(value["error"]["code"], "channel_disabled");
        let events = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notification_events")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(events, 0);
    }

    #[tokio::test]
    async fn channels_list_and_detail_redact_provider_references() {
        let (_dir, state, _provider) = test_state().await;
        let response = notification_channels(State(state.clone()), Extension(session())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        let channels = value.as_array().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0]["channelId"], "telegram");
        assert_eq!(channels[0]["destination"], "****6789");
        assert_eq!(channels[0]["providerRef"], "telegram-token");
        assert_eq!(channels[0]["maxAttempts"], 3);
        assert_eq!(channels[0]["retryBaseSeconds"], 60);
        assert!(!value.to_string().contains("fake-token"));

        let response = notification_channel_detail(
            State(state),
            Path("telegram".to_owned()),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn worker_processes_retry_schedule_until_dead_letter() {
        let (_dir, state, _provider) = test_state().await;
        let event_id = seed_event(state.db().pool()).await;
        // One provider failure per attempt: 3 attempts → dead letter.
        _provider.results.lock().unwrap().clear();
        for _ in 0..3 {
            _provider
                .results
                .lock()
                .unwrap()
                .push(Err(crate::notifications::SendError::Api {
                    code: 400,
                    retry_after: None,
                }));
        }
        let delivery_id = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::deliveries_for_event(&mut conn, &event_id)
                .await
                .unwrap()[0]
                .delivery_id
                .clone()
        };

        let processed = crate::notifications::process_due_deliveries(&state, &*_provider)
            .await
            .unwrap();
        assert_eq!(processed, 1);
        let delivery = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::load_delivery(&mut conn, &delivery_id)
                .await
                .unwrap()
                .unwrap()
        };
        assert_eq!(delivery.state, "retry_scheduled");
        assert_eq!(delivery.attempt_count, 1);
        assert!(delivery.next_attempt_at.is_some());

        // Force the retry due and process again.
        sqlx::query("UPDATE notification_deliveries SET next_attempt_at = '2020-01-01T00:00:00Z' WHERE delivery_id = ?")
            .bind(&delivery_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        let processed = crate::notifications::process_due_deliveries(&state, &*_provider)
            .await
            .unwrap();
        assert_eq!(processed, 1);
        sqlx::query("UPDATE notification_deliveries SET next_attempt_at = '2020-01-01T00:00:00Z' WHERE delivery_id = ?")
            .bind(&delivery_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        let processed = crate::notifications::process_due_deliveries(&state, &*_provider)
            .await
            .unwrap();
        assert_eq!(processed, 1);
        let delivery = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::load_delivery(&mut conn, &delivery_id)
                .await
                .unwrap()
                .unwrap()
        };
        assert_eq!(delivery.state, "dead_letter");
        assert_eq!(delivery.attempt_count, 3);
        let attempts = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::attempts_for_delivery(&mut conn, &delivery_id)
                .await
                .unwrap()
        };
        assert_eq!(attempts.len(), 3);
        assert_eq!(attempts[2].provider_result, "telegram_api_error 400");
    }

    #[tokio::test]
    async fn worker_repairs_stale_in_flight_deliveries() {
        let (_dir, state, provider) = test_state().await;
        let event_id = seed_event(state.db().pool()).await;
        let delivery_id = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::deliveries_for_event(&mut conn, &event_id)
                .await
                .unwrap()[0]
                .delivery_id
                .clone()
        };
        sqlx::query("UPDATE notification_deliveries SET state = 'in_flight' WHERE delivery_id = ?")
            .bind(&delivery_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        provider.results.lock().unwrap().clear();
        provider.results.lock().unwrap().push(Ok(()));
        let processed = crate::notifications::process_due_deliveries(&state, &*provider)
            .await
            .unwrap();
        assert_eq!(processed, 1);
        let delivery = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::load_delivery(&mut conn, &delivery_id)
                .await
                .unwrap()
                .unwrap()
        };
        assert_eq!(delivery.state, "succeeded");
        assert_eq!(delivery.attempt_count, 1);
    }

    #[tokio::test]
    async fn worker_marks_missing_channel_as_failed_config() {
        let (_dir, state, _provider) =
            test_state_with_channels(crate::config::NotificationChannels::default()).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        let event_id = crate::notifications::record_notification_event(
            &mut conn,
            crate::notifications::NotificationEventInput {
                kind: "incident",
                incident_id: None,
                rule_key: None,
                subject: None,
                severity: "info",
                summary: "summary",
            },
            &crate::config::NotificationChannels::default(),
            base_time(),
        )
        .await
        .unwrap();
        drop(conn);
        // Insert a delivery row directly for a channel that is no longer
        // configured: the worker must fail it terminally without retrying.
        sqlx::query(
            "INSERT INTO notification_deliveries (delivery_id, event_id, channel_kind, destination, state, created_at, updated_at) VALUES ('dl-orphan', ?, 'telegram', '****6789', 'pending', '2026-03-01T00:00:00Z', '2026-03-01T00:00:00Z')",
        )
        .bind(&event_id)
        .execute(state.db().pool())
        .await
        .unwrap();
        let processed = crate::notifications::process_due_deliveries(&state, &*_provider)
            .await
            .unwrap();
        assert_eq!(processed, 1);
        let delivery = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            crate::notifications::load_delivery(&mut conn, "dl-orphan")
                .await
                .unwrap()
                .unwrap()
        };
        assert_eq!(delivery.state, "failed");
        assert_eq!(delivery.last_error_kind.as_deref(), Some("config"));
        assert_eq!(delivery.attempt_count, 0);
    }

    #[tokio::test]
    async fn worker_never_sends_on_a_disabled_channel() {
        let (_dir, state, provider) =
            test_state_with_channels(crate::config::NotificationChannels {
                telegram: Some(crate::config::TelegramChannel {
                    enabled: false,
                    token_file: "/unused".into(),
                    chat_id: "123456789".to_owned(),
                    max_attempts: 3,
                    retry_base_seconds: 60,
                }),
            })
            .await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        let event_id = crate::notifications::record_notification_event(
            &mut conn,
            crate::notifications::NotificationEventInput {
                kind: "incident",
                incident_id: None,
                rule_key: None,
                subject: None,
                severity: "info",
                summary: "summary",
            },
            state.channels(),
            base_time(),
        )
        .await
        .unwrap();
        drop(conn);

        let processed = crate::notifications::process_due_deliveries(&state, &*provider)
            .await
            .unwrap();
        assert_eq!(processed, 1);
        let delivery = {
            let mut conn = state.db().pool().acquire().await.unwrap();
            let delivery_id = crate::notifications::deliveries_for_event(&mut conn, &event_id)
                .await
                .unwrap()[0]
                .delivery_id
                .clone();
            crate::notifications::load_delivery(&mut conn, &delivery_id)
                .await
                .unwrap()
                .unwrap()
        };
        // Disabled = terminal config failure: never sent, no attempt
        // recorded, and no automatic retry.
        assert_eq!(delivery.state, "failed");
        assert_eq!(delivery.last_error_kind.as_deref(), Some("config"));
        assert_eq!(delivery.attempt_count, 0);
        // The provider was never invoked.
        assert_eq!(provider.texts.lock().unwrap().len(), 0);
    }
}
