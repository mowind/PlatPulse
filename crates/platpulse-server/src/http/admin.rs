//! Owner-only Agent/Node current observation diagnostics.
use super::{AppState, ROUTE_GROUP_HEADER, api_not_found};
use crate::http::AuthenticatedSession;
use crate::http::realtime;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio_stream::Stream;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VisibilityRequest {
    pub visibility: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VisibilityResponse {
    pub node_id: String,
    pub visibility: String,
}

pub(crate) fn mutation_error(
    request_id: &str,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    (
        status,
        Json(crate::http::ApiErrorBody::new(code, message, request_id)),
    )
        .into_response()
}

/// Shared browser mutation trust boundary (design §12.4, webui.md §6.4): a
/// JSON content type, a same-origin request, and the session CSRF header are
/// all required before any Admin mutation parses its body. A malformed body
/// must never bypass these checks or produce a framework-generated error
/// with a different envelope.
fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(crate) fn mutation_guard_ok(
    headers: &HeaderMap,
    state: &AppState,
    session: &AuthenticatedSession,
) -> bool {
    let content_type_valid = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"));
    let origin_valid = state.auth().origin_matches(headers.get(header::ORIGIN));
    let csrf_valid = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| bool::from(token.as_bytes().ct_eq(session.0.csrf_token.as_bytes())));
    content_type_valid && origin_valid && csrf_valid
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/events",
    tag = "admin",
    responses((status = 200, description = "Owner-authenticated Admin invalidation stream"))
)]
async fn admin_events(
    State(state): State<AppState>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let cursor =
        realtime::parse_last_event_id(headers.get("last-event-id").and_then(|v| v.to_str().ok()));
    Sse::new(state.admin_realtime().stream_with_session(
        cursor,
        state.database(),
        state.auth().clone(),
        _session.0.session_id.clone(),
        _session.0.role.clone(),
    ))
    .keep_alive(
        KeepAlive::new()
            .interval(realtime::keepalive_interval())
            .text("keepalive"),
    )
}

/// Owner-only visibility mutation. All browser mutation trust-boundary
/// checks are explicit here because Admin DTOs must not be writable by a
/// Viewer or by cross-origin requests.
#[utoipa::path(
    put,
    path = "/api/admin/v1/nodes/{node_id}/visibility",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    request_body = VisibilityRequest,
    responses((status = 200, body = VisibilityResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn set_visibility(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
    body: axum::body::Bytes,
) -> Response {
    // Validate the browser trust boundary before attempting to parse JSON. A
    // malformed body must not bypass the same-origin/CSRF checks or produce a
    // framework-generated error with a different envelope.
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: VisibilityRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    if body.visibility != "private" && body.visibility != "public" {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_visibility",
            "visibility must be private or public",
        );
    }
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let before = sqlx::query_as::<_, (String, String)>(
        "SELECT visibility, network_key FROM nodes WHERE node_id = ?",
    )
    .bind(&node_id)
    .fetch_optional(&mut *tx)
    .await;
    let Some((previous, network_key)) = (match before {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    }) else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    let changed_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    if sqlx::query("UPDATE nodes SET visibility = ?, updated_at = ? WHERE node_id = ?")
        .bind(&body.visibility)
        .bind(&changed_at)
        .bind(&node_id)
        .execute(&mut *tx)
        .await
        .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let before_audit = serde_json::json!({ "visibility": previous });
    let after_audit = serde_json::json!({ "visibility": body.visibility });
    if crate::auth::insert_audit_change(
        &mut *tx,
        Some(&principal.0.user_id),
        "node_visibility_changed",
        "node",
        &node_id,
        Some(&before_audit),
        Some(&after_audit),
    )
    .await
    .is_err()
        || tx.commit().await.is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let revision = changed_at.bytes().fold(0_u64, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as u64)
    });
    if body.visibility == "public" {
        state
            .public_realtime()
            .publish("node", Some(node_id.clone()), revision);
        state
            .public_realtime()
            .publish("network", Some(network_key), revision);
    } else {
        state
            .public_realtime()
            .publish_reset("collection", revision);
    }
    state
        .admin_realtime()
        .publish("node", Some(node_id.clone()), revision);
    Json(VisibilityResponse {
        node_id,
        visibility: body.visibility,
    })
    .into_response()
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeMetadataRequest {
    /// Server-owned display name (design §4.2). Required: 1..=128 visible
    /// characters; clearing is a future explicit operation.
    pub display_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeMetadataResponse {
    pub node_id: String,
    pub display_name: String,
}

/// Owner-only Server-owned metadata mutation (display name). This never
/// touches the Agent-declared endpoint, Network key, or Node ID; the Agent
/// Inventory remains authoritative for those.
#[utoipa::path(
    put,
    path = "/api/admin/v1/nodes/{node_id}/metadata",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    request_body = NodeMetadataRequest,
    responses((status = 200, body = NodeMetadataResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn set_node_metadata(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: NodeMetadataRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    if body.display_name.is_empty()
        || body.display_name.len() > crate::network::MAX_DISPLAY_NAME_LEN
        || body.display_name.chars().any(|c| c.is_control())
    {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_display_name",
            "display name must be 1..=128 characters without control characters",
        );
    }
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    // `display_name` is nullable: a Node without a Server-owned name must
    // still be renameable, so the previous value is decoded as Option.
    let before =
        sqlx::query_scalar::<_, Option<String>>("SELECT display_name FROM nodes WHERE node_id = ?")
            .bind(&node_id)
            .fetch_optional(&mut *tx)
            .await;
    let Some(previous) = (match before {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    }) else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    if previous.as_deref() == Some(body.display_name.as_str()) {
        return Json(NodeMetadataResponse {
            node_id: node_id.clone(),
            display_name: body.display_name,
        })
        .into_response();
    }
    let changed_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    if sqlx::query("UPDATE nodes SET display_name = ?, updated_at = ? WHERE node_id = ?")
        .bind(&body.display_name)
        .bind(&changed_at)
        .bind(&node_id)
        .execute(&mut *tx)
        .await
        .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let before_audit = serde_json::json!({ "display_name": previous });
    let after_audit = serde_json::json!({ "display_name": body.display_name });
    if crate::auth::insert_audit_change(
        &mut *tx,
        Some(&principal.0.user_id),
        "node_metadata_changed",
        "node",
        &node_id,
        Some(&before_audit),
        Some(&after_audit),
    )
    .await
    .is_err()
        || tx.commit().await.is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let revision = changed_at.bytes().fold(0_u64, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as u64)
    });
    state
        .admin_realtime()
        .publish("node", Some(node_id.clone()), revision);
    Json(NodeMetadataResponse {
        node_id,
        display_name: body.display_name,
    })
    .into_response()
}
/// Redacted Agent credential summary. Only the non-sensitive credential
/// id and lifecycle instants are exposed; the credential secret itself is
/// never stored by the Server and never appears in any Admin DTO.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentCredentialSummary {
    pub credential_id: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
    /// Rotation overlap deadline; the credential stops authenticating at
    /// this instant even without an explicit revoke.
    pub revoke_after: Option<String>,
    /// Server-computed validity: not revoked and not past `revoke_after`.
    pub active: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentDiagnostic {
    pub agent_id: String,
    pub agent_epoch: i64,
    pub last_report_sequence: Option<i64>,
    pub active_boot_id: Option<String>,
    pub boot_status: String,
    pub previous_boot_id: Option<String>,
    pub close_report_id: Option<String>,
    pub shutdown_state: String,
    pub shutdown_started_at: Option<String>,
    pub shutdown_deadline_at: Option<String>,
    pub shutdown_finished_at: Option<String>,
    pub shutdown_unresolved_range: Option<(i64, i64)>,
    pub shutdown_last_error: Option<String>,
    pub shutdown_forced: bool,
    pub shutdown_report_id: Option<String>,
    pub shutdown_report_sequence: Option<i64>,
    pub shutdown_updated_at: Option<String>,
    pub sequence_gap_count: i64,
    pub security_event_count: i64,
    pub clock_status: String,
    pub clock_skew_ms: Option<i64>,
    pub liveness: String,
    pub last_received_at: Option<String>,
    pub capabilities: Vec<String>,
    /// Credential state as a separate dimension (design: identity,
    /// liveness, boot/report, inventory, credentials, diagnostics).
    pub credentials: Vec<AgentCredentialSummary>,
    pub host: Option<HostDiagnostic>,
    pub nodes: Vec<NodeDiagnostic>,
}

#[derive(Debug, sqlx::FromRow)]
struct HostProjectionRow {
    cpu_percent: Option<f64>,
    memory_total_bytes: Option<i64>,
    memory_used_bytes: Option<i64>,
    load1: Option<f64>,
    load5: Option<f64>,
    load15: Option<f64>,
    network_rx_bytes_per_sec: Option<i64>,
    network_tx_bytes_per_sec: Option<i64>,
    spool_queued_bytes: Option<i64>,
    spool_queued_reports: Option<i64>,
    spool_oldest_queued_age_ms: Option<i64>,
    spool_in_flight: Option<i64>,
    spool_last_delivery_error: Option<String>,
    spool_last_delivery_at: Option<String>,
    spool_capacity_bytes: Option<i64>,
    spool_max_age_seconds: Option<i64>,
    spool_dropped_sequence_from: Option<i64>,
    spool_dropped_sequence_to: Option<i64>,
    spool_dropped_time_from: Option<String>,
    spool_dropped_time_to: Option<String>,
    spool_dropped_height_from: Option<i64>,
    spool_dropped_height_to: Option<i64>,
    spool_pending_history_gaps: Option<i64>,
    spool_report_too_large: Option<i64>,
    spool_store_fatal: Option<i64>,
    spool_store_error: Option<String>,
    updated_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct HostDiagnostic {
    pub cpu_percent: Option<f64>,
    pub memory_total_bytes: Option<i64>,
    pub memory_used_bytes: Option<i64>,
    pub load1: Option<f64>,
    pub load5: Option<f64>,
    pub load15: Option<f64>,
    pub network_rx_bytes_per_sec: Option<i64>,
    pub network_tx_bytes_per_sec: Option<i64>,
    pub spool_queued_bytes: Option<i64>,
    pub spool_queued_reports: Option<i64>,
    pub spool_oldest_queued_age_ms: Option<i64>,
    pub spool_in_flight: Option<bool>,
    pub spool_last_delivery_error: Option<String>,
    pub spool_last_delivery_at: Option<String>,
    pub spool_capacity_bytes: Option<i64>,
    pub spool_max_age_seconds: Option<i64>,
    pub spool_dropped_sequence_from: Option<i64>,
    pub spool_dropped_sequence_to: Option<i64>,
    pub spool_dropped_time_from: Option<String>,
    pub spool_dropped_time_to: Option<String>,
    pub spool_dropped_height_from: Option<i64>,
    pub spool_dropped_height_to: Option<i64>,
    pub spool_pending_history_gaps: Option<i64>,
    pub spool_report_too_large: Option<bool>,
    pub spool_store_fatal: Option<bool>,
    pub spool_store_error: Option<String>,
    pub updated_at: String,
    pub components: Vec<HostComponentDiagnostic>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct HostComponentDiagnostic {
    pub component: String,
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProcessDiagnostic {
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
    pub pid: Option<i64>,
    pub started_at: Option<String>,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<i64>,
    pub uptime_ms: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct RpcDiagnostic {
    pub client_version: Option<String>,
    pub namespaces: Vec<String>,
    pub methods: Vec<String>,
    pub state: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: Option<i64>,
    pub value_revision: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct SyncDiagnostic {
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
    pub syncing: Option<bool>,
    pub current_block: Option<i64>,
    pub highest_block: Option<i64>,
    pub pulled_states: Option<i64>,
    pub known_states: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ConsensusDiagnostic {
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
    pub epoch: Option<i64>,
    pub view_number: Option<i64>,
    pub validator: Option<bool>,
    pub highest_qc_block: Option<i64>,
    pub highest_lock_block: Option<i64>,
    pub highest_commit_block: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PeerDiagnosticEntry {
    pub peer_id: String,
    pub direction: String,
    pub trusted: bool,
    pub static_peer: bool,
    pub consensus_peer: bool,
    pub client_name: Option<String>,
    pub capabilities: Vec<String>,
    pub cbft_protocol_version: Option<i64>,
    pub cbft_highest_qc_block: Option<i64>,
    pub cbft_locked_block: Option<i64>,
    pub cbft_commit_block: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PeerPresenceInterval {
    pub peer_id: String,
    /// Server-observed arrival boundary of this connected interval.
    pub opened_at: String,
    /// Server-observed departure boundary; `None` means the interval is open.
    pub closed_at: Option<String>,
    /// Closed interval duration. Open intervals intentionally have no
    /// duration because their end boundary is not known yet.
    pub duration_seconds: Option<i64>,
    pub direction: String,
    pub trusted: bool,
    pub static_peer: bool,
    pub consensus_peer: bool,
    pub client_name: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PeerChurnDiagnostic {
    /// `unknown` means no successful Peer Snapshot exists; `empty` means a
    /// successful snapshot exists but no retained interval is available;
    /// `error` means the latest collection failed while retained intervals
    /// and the last-good value remain available.
    pub state: String,
    pub freshness: String,
    pub window_start: String,
    pub total_open_intervals: i64,
    pub recent_arrivals: Vec<PeerPresenceInterval>,
    pub recent_departures: Vec<PeerPresenceInterval>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminPeerHistory {
    /// Aggregate collection state; retained last-good rows remain visible
    /// when the latest Peer collection is in error.
    pub state: String,
    pub freshness: String,
    pub five_minute: Vec<AdminPeerAggregate>,
    pub hourly: Vec<AdminPeerAggregate>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminPeerAggregate {
    pub bucket_start: String,
    pub last_observed_at: String,
    pub sample_count: i64,
    pub total_peers: i64,
    pub average_peers: Option<f64>,
    pub inbound_count: i64,
    pub outbound_count: i64,
    pub trusted_count: i64,
    pub static_count: i64,
    pub consensus_count: i64,
    pub known_country_count: i64,
    pub unknown_country_count: i64,
    pub countries: Vec<AdminPeerCountryCount>,
    pub arrivals: i64,
    pub departures: i64,
    pub cbft_lag: AdminPeerLagSummary,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminPeerCountryCount {
    pub country_code: String,
    pub count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminPeerLagSummary {
    pub sample_count: i64,
    pub minimum: Option<i64>,
    pub average: Option<f64>,
    pub maximum: Option<i64>,
}

fn admin_peer_history(history: crate::peer_history::PeerHistory) -> AdminPeerHistory {
    let convert = |row: crate::peer_history::PeerAggregateRow| {
        let average_peers =
            (row.sample_count > 0).then(|| row.total_peers as f64 / row.sample_count as f64);
        let average_lag =
            (row.cbft_lag_count > 0).then(|| row.cbft_lag_sum as f64 / row.cbft_lag_count as f64);
        AdminPeerAggregate {
            bucket_start: row.bucket_start,
            last_observed_at: row.last_observed_at,
            sample_count: row.sample_count,
            total_peers: row.total_peers,
            average_peers,
            inbound_count: row.inbound_count,
            outbound_count: row.outbound_count,
            trusted_count: row.trusted_count,
            static_count: row.static_count,
            consensus_count: row.consensus_count,
            known_country_count: row.known_country_count,
            unknown_country_count: row.unknown_country_count,
            countries: row
                .countries
                .into_iter()
                .map(|country| AdminPeerCountryCount {
                    country_code: country.country_code,
                    count: country.count,
                })
                .collect(),
            arrivals: row.arrivals,
            departures: row.departures,
            cbft_lag: AdminPeerLagSummary {
                sample_count: row.cbft_lag_count,
                minimum: row.cbft_lag_min,
                average: average_lag,
                maximum: row.cbft_lag_max,
            },
        }
    };
    AdminPeerHistory {
        state: history.state,
        freshness: history.freshness,
        five_minute: history.five_minute.into_iter().map(convert).collect(),
        hourly: history.hourly.into_iter().map(convert).collect(),
    }
}

const PEER_CHURN_LIMIT: i64 = 128;
const PEER_CHURN_WINDOW_HOURS: i64 = 24;

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PeerDiagnostic {
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
    /// Server-owned freshness of the latest successful Peer value.
    pub freshness: String,
    /// `None` means no successful Peer Snapshot has ever been observed;
    /// `Some(0)` is an authoritative successful empty snapshot.
    pub peer_count: Option<i64>,
    pub inbound_count: Option<i64>,
    pub outbound_count: Option<i64>,
    pub trusted_count: Option<i64>,
    pub static_count: Option<i64>,
    pub consensus_count: Option<i64>,
    pub peers: Vec<PeerDiagnosticEntry>,
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ComponentDiagnostic {
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
}

const FRESHNESS_LIMIT_SECONDS: i64 = 120;

fn current_observation(timestamp: Option<&str>) -> bool {
    timestamp
        .and_then(crate::auth::parse_rfc3339)
        .is_some_and(|value| {
            (crate::auth::now_utc() - value).whole_seconds().abs() <= FRESHNESS_LIMIT_SECONDS
        })
}

fn derive_health(
    lifecycle: &str,
    rpc: Option<&RpcDiagnostic>,
    sync: Option<&SyncDiagnostic>,
    consensus: Option<&ConsensusDiagnostic>,
) -> (&'static str, &'static str) {
    if lifecycle == "retired" {
        return ("unknown", "node is retired");
    }
    if rpc.is_some_and(|component| component.state.as_deref() == Some("error")) {
        return ("unhealthy", "RPC collection failed");
    }
    if sync.is_some_and(|component| component.state == "error") {
        return ("unhealthy", "sync collection failed");
    }
    if consensus.is_some_and(|component| component.state == "error") {
        return ("unhealthy", "consensus collection failed");
    }
    let fresh = rpc
        .and_then(|component| component.received_at.as_deref())
        .is_some_and(|value| current_observation(Some(value)))
        && sync
            .and_then(|component| component.received_at.as_deref())
            .is_some_and(|value| current_observation(Some(value)))
        && consensus
            .and_then(|component| component.received_at.as_deref())
            .is_some_and(|value| current_observation(Some(value)));
    if !fresh {
        return ("unknown", "one or more observations are stale or unknown");
    }
    if rpc.is_some_and(|component| component.state.as_deref() == Some("ok"))
        && sync.is_some_and(|component| component.state == "ok")
        && consensus.is_some_and(|component| component.state == "ok")
    {
        ("healthy", "RPC, sync, and consensus are current")
    } else {
        ("unknown", "one or more observations are unsupported")
    }
}

/// Server-owned freshness dimension for the Admin Node view: `current` when
/// the newest retained observation is fresh, `stale` when an older retained
/// observation exists, and `unknown` when nothing was ever observed.
fn derive_freshness<'a>(received_at: impl Iterator<Item = &'a str>) -> &'static str {
    let latest = received_at.filter_map(crate::auth::parse_rfc3339).max();
    match latest {
        None => "unknown",
        Some(value)
            if (crate::auth::now_utc() - value).whole_seconds().abs()
                <= FRESHNESS_LIMIT_SECONDS =>
        {
            "current"
        }
        Some(_) => "stale",
    }
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NodeDiagnostic {
    pub node_id: String,
    pub network_key: String,
    pub display_name: Option<String>,
    pub lifecycle: String,
    pub inventory_revision: i64,
    pub visibility: String,
    pub health: String,
    pub health_reason: String,
    /// Server-owned freshness dimension: `current`, `stale`, or `unknown`.
    /// The WebUI formats this state; it never derives it from `Date.now()`.
    pub freshness: String,
    pub process: Option<ProcessDiagnostic>,
    pub rpc: Option<RpcDiagnostic>,
    pub sync: Option<SyncDiagnostic>,
    pub consensus: Option<ConsensusDiagnostic>,
    pub peers: Option<PeerDiagnostic>,
    pub current_head: Option<i64>,
    pub historical_high_watermark: Option<i64>,
    pub resync_state: String,
    pub network_reference_head: Option<i64>,
    pub network_reference_confidence: String,
    pub resync_progress: Option<String>,
}

async fn process_diagnostic(state: &AppState, node_id: &str) -> Option<ProcessDiagnostic> {
    sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64, Option<i64>, Option<String>, Option<f64>, Option<i64>, Option<i64>)>(
        "SELECT s.state, s.error_code, s.error_message, s.attempted_at, s.observed_at, s.received_at, s.state_revision, s.value_revision, p.pid, p.started_at, p.cpu_percent, p.memory_bytes, p.uptime_ms FROM component_status s LEFT JOIN current_node_process_observations p ON p.node_id = s.node_id WHERE s.node_id = ? AND s.component_key = 'process'"
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten()
    .map(|(state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision, pid, started_at, cpu_percent, memory_bytes, uptime_ms)| ProcessDiagnostic {
        state,
        error_code,
        error_message: redact_optional_message(error_message),
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
        pid,
        started_at,
        cpu_percent,
        memory_bytes,
        uptime_ms,
    })
}

async fn sync_diagnostic(state: &AppState, node_id: &str) -> Option<SyncDiagnostic> {
    sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT s.state, s.error_code, s.error_message, s.attempted_at, s.observed_at, s.received_at, s.state_revision, s.value_revision, c.syncing, c.current_block, c.highest_block, c.pulled_states, c.known_states FROM component_status s LEFT JOIN current_node_chain_observations c ON c.node_id = s.node_id WHERE s.node_id = ? AND s.component_key = 'sync'"
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten()
    .map(|(state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision, syncing, current_block, highest_block, pulled_states, known_states)| SyncDiagnostic {
        state,
        error_code,
        error_message: redact_optional_message(error_message),
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
        syncing: syncing.map(|value| value != 0),
        current_block,
        highest_block,
        pulled_states,
        known_states,
    })
}

async fn consensus_diagnostic(state: &AppState, node_id: &str) -> Option<ConsensusDiagnostic> {
    sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT s.state, s.error_code, s.error_message, s.attempted_at, s.observed_at, s.received_at, s.state_revision, s.value_revision, c.consensus_epoch, c.consensus_view_number, c.consensus_validator, c.consensus_highest_qc_block, c.consensus_highest_lock_block, c.consensus_highest_commit_block FROM component_status s LEFT JOIN current_node_chain_observations c ON c.node_id = s.node_id WHERE s.node_id = ? AND s.component_key = 'consensus'"
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten()
    .map(|(state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision, epoch, view_number, validator, highest_qc_block, highest_lock_block, highest_commit_block)| ConsensusDiagnostic {
        state,
        error_code,
        error_message: redact_optional_message(error_message),
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
        epoch,
        view_number,
        validator: validator.map(|value| value != 0),
        highest_qc_block,
        highest_lock_block,
        highest_commit_block,
    })
}

type PeerPresenceRow = (
    String,
    String,
    Option<String>,
    String,
    i64,
    i64,
    i64,
    Option<String>,
);

fn peer_presence_interval_from_row(row: PeerPresenceRow) -> PeerPresenceInterval {
    let (
        peer_id,
        opened_at,
        closed_at,
        direction,
        trusted,
        static_peer,
        consensus_peer,
        client_name,
    ) = row;
    let duration_seconds = closed_at.as_deref().and_then(|closed| {
        let opened = crate::auth::parse_rfc3339(&opened_at)?;
        let closed = crate::auth::parse_rfc3339(closed)?;
        Some((closed - opened).whole_seconds().max(0))
    });
    PeerPresenceInterval {
        peer_id: crate::redaction::redact_sensitive(&peer_id),
        opened_at,
        closed_at,
        duration_seconds,
        direction,
        trusted: trusted != 0,
        static_peer: static_peer != 0,
        consensus_peer: consensus_peer != 0,
        client_name: client_name.map(|name| crate::redaction::redact_sensitive(&name)),
    }
}

async fn peer_churn_diagnostic(
    state: &AppState,
    node_id: &str,
) -> Result<PeerChurnDiagnostic, sqlx::Error> {
    let window_start = crate::auth::format_rfc3339(
        crate::auth::now_utc() - time::Duration::hours(PEER_CHURN_WINDOW_HOURS),
    );
    let unknown = || PeerChurnDiagnostic {
        state: "unknown".to_owned(),
        freshness: "unknown".to_owned(),
        window_start: window_start.clone(),
        total_open_intervals: 0,
        recent_arrivals: Vec::new(),
        recent_departures: Vec::new(),
    };
    let Some((component_state, value_received_at, value_revision)) =
        sqlx::query_as::<_, (String, Option<String>, i64)>(
            "SELECT state, value_received_at, value_revision FROM component_status WHERE node_id=? AND component_key='peers'",
        )
        .bind(node_id)
        .fetch_optional(state.db().pool())
        .await?
    else {
        return Ok(unknown());
    };
    if value_revision <= 0 {
        return Ok(unknown());
    }
    let total_open_intervals = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=? AND closed_at IS NULL",
    )
    .bind(node_id)
    .fetch_one(state.db().pool())
    .await?;
    let arrivals = sqlx::query_as::<_, PeerPresenceRow>(
        "SELECT peer_id, opened_at, closed_at, direction, trusted, static_peer, consensus_peer, client_name FROM peer_presence_intervals WHERE node_id=? AND opened_at >= ? ORDER BY opened_at DESC, interval_id DESC LIMIT ?",
    )
    .bind(node_id)
    .bind(&window_start)
    .bind(PEER_CHURN_LIMIT)
    .fetch_all(state.db().pool())
    .await?
    .into_iter()
    .map(peer_presence_interval_from_row)
    .collect::<Vec<_>>();
    let departures = sqlx::query_as::<_, PeerPresenceRow>(
        "SELECT peer_id, opened_at, closed_at, direction, trusted, static_peer, consensus_peer, client_name FROM peer_presence_intervals WHERE node_id=? AND closed_at >= ? ORDER BY closed_at DESC, interval_id DESC LIMIT ?",
    )
    .bind(node_id)
    .bind(&window_start)
    .bind(PEER_CHURN_LIMIT)
    .fetch_all(state.db().pool())
    .await?
    .into_iter()
    .map(peer_presence_interval_from_row)
    .collect::<Vec<_>>();
    let freshness = derive_freshness(value_received_at.as_deref().into_iter()).to_owned();
    let has_intervals = total_open_intervals > 0 || !arrivals.is_empty() || !departures.is_empty();
    let diagnostic_state = match component_state.as_str() {
        "error" => "error",
        "unsupported" => "unsupported",
        "disabled" => "disabled",
        "starting" => "starting",
        "ok" if has_intervals => "ok",
        "ok" => "empty",
        _ => "unknown",
    };
    Ok(PeerChurnDiagnostic {
        state: diagnostic_state.to_owned(),
        freshness,
        window_start,
        total_open_intervals,
        recent_arrivals: arrivals,
        recent_departures: departures,
    })
}

async fn peer_diagnostic(state: &AppState, node_id: &str) -> Option<PeerDiagnostic> {
    let component = sqlx::query_as::<_, (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        i64,
    )>(
        "SELECT state, error_code, error_message, attempted_at, observed_at, received_at, value_received_at, state_revision, value_revision FROM component_status WHERE node_id=? AND component_key='peers'",
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten()?;
    let (
        component_state,
        error_code,
        _agent_error_message,
        attempted_at,
        observed_at,
        received_at,
        value_received_at,
        state_revision,
        value_revision,
    ) = component;
    // A projection read failure must not masquerade as an authoritative empty
    // snapshot; return Unknown by omitting the diagnostic instead.
    let rows = sqlx::query_as::<_, (
        String,
        String,
        i64,
        i64,
        i64,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    )>(
        "SELECT peer_id, direction, trusted, static_peer, consensus_peer, client_name, cbft_protocol_version, cbft_highest_qc_block, cbft_locked_block, cbft_commit_block FROM current_node_peers WHERE node_id=? ORDER BY peer_id LIMIT 1024",
    )
    .bind(node_id)
    .fetch_all(state.db().pool())
    .await
    .ok()?;
    let has_value = value_revision > 0;
    let mut inbound_count = 0;
    let mut outbound_count = 0;
    let mut trusted_count = 0;
    let mut static_count = 0;
    let mut consensus_count = 0;
    let mut peers = Vec::with_capacity(rows.len());
    for (
        peer_id,
        direction,
        trusted,
        static_peer,
        consensus_peer,
        client_name,
        cbft_protocol_version,
        cbft_highest_qc_block,
        cbft_locked_block,
        cbft_commit_block,
    ) in rows
    {
        match direction.as_str() {
            "inbound" => inbound_count += 1,
            "outbound" => outbound_count += 1,
            _ => {}
        }
        if trusted != 0 {
            trusted_count += 1;
        }
        if static_peer != 0 {
            static_count += 1;
        }
        if consensus_peer != 0 {
            consensus_count += 1;
        }
        let capabilities = sqlx::query_scalar::<_, String>(
            "SELECT capability FROM current_node_peer_capabilities WHERE node_id=? AND peer_id=? ORDER BY capability",
        )
        .bind(node_id)
        .bind(&peer_id)
        .fetch_all(state.db().pool())
        .await
        .ok()?;
        peers.push(PeerDiagnosticEntry {
            peer_id: crate::redaction::redact_sensitive(&peer_id),
            direction,
            trusted: trusted != 0,
            static_peer: static_peer != 0,
            consensus_peer: consensus_peer != 0,
            client_name: client_name.map(|name| crate::redaction::redact_sensitive(&name)),
            capabilities: capabilities
                .into_iter()
                .map(|capability| crate::redaction::redact_sensitive(&capability))
                .collect(),
            cbft_protocol_version,
            cbft_highest_qc_block,
            cbft_locked_block,
            cbft_commit_block,
        });
    }

    let freshness = if has_value {
        derive_freshness(value_received_at.as_deref().into_iter()).to_owned()
    } else {
        "unknown".to_owned()
    };
    let peer_error = component_state == "error";
    Some(PeerDiagnostic {
        state: component_state,
        error_code,
        // Peer probe messages are Agent-controlled and may contain raw RPC
        // payloads. Keep the Admin response privacy-safe; the typed code and
        // state still identify the failure.
        error_message: peer_error.then(|| "Peer Snapshot collection failed".to_owned()),
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
        freshness,
        peer_count: has_value.then_some(peers.len() as i64),
        inbound_count: has_value.then_some(inbound_count),
        outbound_count: has_value.then_some(outbound_count),
        trusted_count: has_value.then_some(trusted_count),
        static_count: has_value.then_some(static_count),
        consensus_count: has_value.then_some(consensus_count),
        peers,
    })
}

/// Collect the full Server-owned diagnostic view for one Node. Used by both
/// the Agent diagnostics list and the Owner overview so health, freshness,
/// and reasons are computed once in the Server.
async fn node_diagnostic(
    state: &AppState,
    node_id: String,
    network_key: String,
    display_name: Option<String>,
    lifecycle: String,
    inventory_revision: i64,
    visibility: String,
) -> NodeDiagnostic {
    let process = process_diagnostic(state, &node_id).await;
    let rpc = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>, Option<i64>)>(
        "SELECT c.rpc_client_version, s.state, s.error_code, s.error_message, s.attempted_at, s.observed_at, s.received_at, s.state_revision, s.value_revision FROM component_status s LEFT JOIN current_node_chain_observations c ON c.node_id = s.node_id WHERE s.node_id = ? AND s.component_key = 'rpc'",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten();
    let rpc = if let Some((
        client_version,
        component_state,
        error_code,
        error_message,
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
    )) = rpc
    {
        let namespaces = sqlx::query_scalar::<_, String>("SELECT namespace FROM current_node_rpc_namespaces WHERE node_id = ? ORDER BY namespace")
            .bind(&node_id).fetch_all(state.db().pool()).await.unwrap_or_default();
        let methods = sqlx::query_scalar::<_, String>(
            "SELECT method FROM current_node_rpc_methods WHERE node_id = ? ORDER BY method",
        )
        .bind(&node_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap_or_default();
        Some(RpcDiagnostic {
            client_version,
            namespaces,
            methods,
            state: component_state,
            error_code,
            error_message: redact_optional_message(error_message),
            attempted_at,
            observed_at,
            received_at,
            state_revision,
            value_revision,
        })
    } else {
        None
    };
    let sync = sync_diagnostic(state, &node_id).await;
    let consensus = consensus_diagnostic(state, &node_id).await;
    let peers = peer_diagnostic(state, &node_id).await;
    let (health, health_reason) =
        derive_health(&lifecycle, rpc.as_ref(), sync.as_ref(), consensus.as_ref());
    let freshness = derive_freshness(
        rpc.as_ref()
            .and_then(|c| c.received_at.as_deref())
            .into_iter()
            .chain(sync.as_ref().and_then(|c| c.received_at.as_deref()))
            .chain(consensus.as_ref().and_then(|c| c.received_at.as_deref()))
            .chain(peers.as_ref().and_then(|c| c.received_at.as_deref())),
    );
    let history = sqlx::query_as::<_, (Option<i64>, Option<i64>, Option<String>, Option<i64>, Option<String>)>(
        "SELECT c.current_block, h.historical_high_watermark, h.resync_state, r.block_number, r.confidence FROM current_node_chain_observations c LEFT JOIN block_history_state h ON h.node_id=c.node_id LEFT JOIN nodes n ON n.node_id=c.node_id LEFT JOIN network_reference_heads r ON r.network_key=n.network_key WHERE c.node_id=?"
    ).bind(&node_id).fetch_optional(state.db().pool()).await.ok().flatten();
    let (
        current_head,
        historical_high_watermark,
        resync_state,
        network_reference_head,
        network_reference_confidence,
    ) = history.unwrap_or((None, None, None, None, None));
    let resync_state = resync_state.unwrap_or_else(|| "normal".to_owned());
    NodeDiagnostic {
        node_id,
        network_key,
        display_name,
        lifecycle,
        inventory_revision,
        visibility,
        health: health.to_owned(),
        health_reason: health_reason.to_owned(),
        freshness: freshness.to_owned(),
        process,
        rpc,
        sync,
        consensus,
        peers,
        current_head,
        historical_high_watermark,
        resync_progress: historical_high_watermark
            .zip(current_head)
            .map(|(high, current)| format!("{current}/{high}")),
        network_reference_head,
        network_reference_confidence: network_reference_confidence
            .unwrap_or_else(|| "unknown".to_owned()),
        resync_state,
    }
}

/// Agent-observed Network identity tuple (design §7.1). Never overwrites the
/// Registry: it is presented as a typed observation with a match state.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ObservedNetworkIdentity {
    pub genesis_hash: Option<String>,
    pub chain_id: Option<i64>,
    pub p2p_network_id: Option<i64>,
    pub address_hrp: Option<String>,
}

/// Server-computed identity disposition for one Node: `matched` when every
/// observed identity field equals the Registry tuple, `mismatched` when any
/// observed field differs (a blocking diagnostic distinct from RPC Error or
/// Node Offline), and `unknown` when the Node was never observed.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NodeIdentityStatus {
    pub state: String,
    pub observed: Option<ObservedNetworkIdentity>,
    /// Registry fields that the observation contradicts (empty when state
    /// is `matched` or `unknown`).
    pub mismatched_fields: Vec<String>,
}

/// Compare the observed identity of one Node against its Registry tuple.
/// The Registry is the only authority: an observation never rewrites it,
/// and a Node whose identity contradicts the Registry is blocked from
/// merging history (design §7.1).
async fn node_identity_status(
    state: &AppState,
    node_id: &str,
    network_key: &str,
) -> NodeIdentityStatus {
    let observed = sqlx::query_as::<_, (Option<String>, Option<i64>, Option<i64>, Option<String>)>(
        "SELECT network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp FROM current_node_chain_observations WHERE node_id = ?",
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten();
    let Some((genesis_hash, chain_id, p2p_network_id, address_hrp)) = observed else {
        return NodeIdentityStatus {
            state: "unknown".to_owned(),
            observed: None,
            mismatched_fields: Vec::new(),
        };
    };
    let observed_fields = [
        genesis_hash.as_deref().is_some(),
        chain_id.is_some(),
        p2p_network_id.is_some(),
        address_hrp.as_deref().is_some(),
    ];
    if !observed_fields.contains(&true) {
        return NodeIdentityStatus {
            state: "unknown".to_owned(),
            observed: None,
            mismatched_fields: Vec::new(),
        };
    }
    let expected = sqlx::query_as::<_, (String, i64, i64, String)>(
        "SELECT genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks WHERE network_key = ?",
    )
    .bind(network_key)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten();
    let Some((expected_genesis, expected_chain, expected_p2p, expected_hrp)) = expected else {
        return NodeIdentityStatus {
            state: "unknown".to_owned(),
            observed: Some(ObservedNetworkIdentity {
                genesis_hash,
                chain_id,
                p2p_network_id,
                address_hrp,
            }),
            mismatched_fields: Vec::new(),
        };
    };
    let mut mismatched_fields = Vec::new();
    if genesis_hash
        .as_deref()
        .is_some_and(|value| value != expected_genesis)
    {
        mismatched_fields.push("genesis_hash".to_owned());
    }
    if chain_id.is_some_and(|value| value != expected_chain) {
        mismatched_fields.push("chain_id".to_owned());
    }
    if p2p_network_id.is_some_and(|value| value != expected_p2p) {
        mismatched_fields.push("p2p_network_id".to_owned());
    }
    if address_hrp
        .as_deref()
        .is_some_and(|value| value != expected_hrp)
    {
        mismatched_fields.push("address_hrp".to_owned());
    }
    // `matched` requires every identity field to have been observed and to
    // equal the Registry tuple; a partial observation stays `unknown` and
    // never fabricates Registry values.
    let state = if !mismatched_fields.is_empty() {
        "mismatched"
    } else if observed_fields.contains(&false) {
        "unknown"
    } else {
        "matched"
    };
    NodeIdentityStatus {
        state: state.to_owned(),
        observed: Some(ObservedNetworkIdentity {
            genesis_hash,
            chain_id,
            p2p_network_id,
            address_hrp,
        }),
        mismatched_fields,
    }
}

fn redact_optional_message(value: Option<String>) -> Option<String> {
    value.map(|message| crate::redaction::redact_sensitive(&message))
}

/// Redacted destination summary (design §9, `PATTERN-REDACTED-DETAIL`):
/// scheme and host with the port masked; path, query, and credentials are
/// never exposed.
fn redact_endpoint(endpoint: &str) -> String {
    let Some(scheme_end) = endpoint.find("://") else {
        return "redacted".to_owned();
    };
    let mut result = endpoint[..scheme_end + 3].to_owned();
    let authority = endpoint[scheme_end + 3..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let authority = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let (host, has_port) = if let Some(host) = authority.strip_prefix('[') {
        match host.find(']') {
            Some(end) => (&host[..end], host[end + 1..].starts_with(':')),
            None => (host, false),
        }
    } else if authority.matches(':').count() == 1 {
        let (host, _) = authority.split_once(':').unwrap_or((authority, ""));
        (host, true)
    } else {
        (authority, false)
    };
    let host = if host.parse::<std::net::IpAddr>().is_ok() {
        "[REDACTED_IP]"
    } else {
        host
    };
    result.push_str(host);
    if has_port {
        result.push_str(":****");
    }
    result
}

/// Server-owned lifecycle guidance (design §4.3): the Server never changes
/// Node lifecycle remotely; the latest valid Agent Inventory is the only
/// authority for Active/Retired.
fn lifecycle_guidance(lifecycle: &str) -> &'static str {
    if lifecycle == "retired" {
        "Retired: the latest Agent Inventory no longer declares this Node. Reactivation requires declaring the same Node ID again in the Agent-local TOML configuration and submitting a new Inventory; the Server never changes Node lifecycle remotely."
    } else {
        "Active: present in the latest valid Agent Inventory. The Agent-local configuration stays authoritative for this Node; the Server never pushes Endpoint or lifecycle changes."
    }
}

/// Owner-only Node inventory row: Server-owned metadata (display name,
/// visibility, lifecycle guidance) stays distinct from Agent-observed
/// identity and endpoint configuration, and each Node is its own row so
/// block, transaction, consensus, peer, and error state never merge across
/// Nodes.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminNodeListItem {
    pub node_id: String,
    pub agent_id: String,
    pub display_name: Option<String>,
    pub network_key: String,
    pub network_display_name: String,
    pub lifecycle: String,
    pub lifecycle_guidance: String,
    pub visibility: String,
    pub inventory_revision: i64,
    pub first_seen_at: String,
    /// Last Server-side metadata change (display name / visibility).
    pub updated_at: String,
    /// Redacted Agent-declared endpoint (scheme + host only).
    pub rpc_endpoint: String,
    /// Server-owned Node Health Summary severity and primary reason.
    pub health: String,
    pub health_reason: String,
    /// Server-owned freshness dimension: `current`, `stale`, or `unknown`.
    pub freshness: String,
    pub current_head: Option<i64>,
    pub resync_state: String,
    pub identity: NodeIdentityStatus,
}

#[derive(Debug, sqlx::FromRow)]
struct AdminNodeRow {
    node_id: String,
    agent_id: String,
    display_name: Option<String>,
    network_key: String,
    network_display_name: String,
    lifecycle: String,
    visibility: String,
    inventory_revision: i64,
    first_seen_at: String,
    updated_at: String,
    rpc_endpoint: String,
}

async fn admin_node_list_item(state: &AppState, row: AdminNodeRow) -> AdminNodeListItem {
    let diagnostic = node_diagnostic(
        state,
        row.node_id.clone(),
        row.network_key.clone(),
        row.display_name.clone(),
        row.lifecycle.clone(),
        row.inventory_revision,
        row.visibility.clone(),
    )
    .await;
    AdminNodeListItem {
        node_id: row.node_id.clone(),
        agent_id: row.agent_id,
        display_name: row.display_name,
        network_key: row.network_key.clone(),
        network_display_name: row.network_display_name,
        lifecycle: row.lifecycle.clone(),
        lifecycle_guidance: lifecycle_guidance(&row.lifecycle).to_owned(),
        visibility: row.visibility,
        inventory_revision: row.inventory_revision,
        first_seen_at: row.first_seen_at,
        updated_at: row.updated_at,
        rpc_endpoint: redact_endpoint(&row.rpc_endpoint),
        health: diagnostic.health,
        health_reason: diagnostic.health_reason,
        freshness: diagnostic.freshness,
        current_head: diagnostic.current_head,
        resync_state: diagnostic.resync_state,
        identity: node_identity_status(state, &row.node_id, &row.network_key).await,
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes",
    tag = "admin",
    responses((status = 200, description = "Owner-only Node inventory with Server-owned metadata and per-Node identity disposition", body = [AdminNodeListItem]))
)]
async fn admin_nodes(
    State(state): State<AppState>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let rows = sqlx::query_as::<_, AdminNodeRow>(
        "SELECT n.node_id, n.agent_id, n.display_name, n.network_key, r.display_name AS network_display_name, n.lifecycle, n.visibility, n.inventory_revision, n.first_seen_at, n.updated_at, n.rpc_endpoint FROM nodes n JOIN networks r ON r.network_key = n.network_key ORDER BY n.network_key, COALESCE(n.display_name, n.node_id)",
    )
    .fetch_all(state.db().pool())
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(admin_node_list_item(&state, row).await);
    }
    Json(items).into_response()
}

/// Owner-only Node detail: the full per-Node view with Server-owned
/// metadata, lifecycle guidance, identity disposition, and the independent
/// process/RPC/sync/consensus observation dimensions (design §4.1–§4.3).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminNodeDetail {
    pub node_id: String,
    pub agent_id: String,
    pub display_name: Option<String>,
    pub network_key: String,
    pub network_display_name: String,
    pub lifecycle: String,
    pub lifecycle_guidance: String,
    pub visibility: String,
    pub inventory_revision: i64,
    pub first_seen_at: String,
    pub updated_at: String,
    pub rpc_endpoint: String,
    pub node_key_fingerprint: Option<String>,
    pub health: String,
    pub health_reason: String,
    pub freshness: String,
    pub identity: NodeIdentityStatus,
    pub process: Option<ProcessDiagnostic>,
    pub rpc: Option<RpcDiagnostic>,
    pub sync: Option<SyncDiagnostic>,
    pub consensus: Option<ConsensusDiagnostic>,
    pub peers: Option<PeerDiagnostic>,
    pub current_head: Option<i64>,
    pub historical_high_watermark: Option<i64>,
    pub resync_state: String,
    pub resync_progress: Option<String>,
    pub network_reference_head: Option<i64>,
    pub network_reference_confidence: String,
    /// Most recent two-phase Transfer for this Node (any outcome), with the
    /// Server-owned effective status; `None` when no Transfer ever existed.
    pub transfer: Option<NodeTransfer>,
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    responses((status = 200, description = "Owner-only Node detail with Server-owned metadata and per-Node diagnostics", body = AdminNodeDetail), (status = 404, body = crate::http::ApiErrorBody))
)]
async fn admin_node_detail(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let row = sqlx::query_as::<_, AdminNodeRow>(
        "SELECT n.node_id, n.agent_id, n.display_name, n.network_key, r.display_name AS network_display_name, n.lifecycle, n.visibility, n.inventory_revision, n.first_seen_at, n.updated_at, n.rpc_endpoint FROM nodes n JOIN networks r ON r.network_key = n.network_key WHERE n.node_id = ?",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await;
    let Some(row) = (match row {
        Ok(row) => row,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    }) else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    let diagnostic = node_diagnostic(
        &state,
        row.node_id.clone(),
        row.network_key.clone(),
        row.display_name.clone(),
        row.lifecycle.clone(),
        row.inventory_revision,
        row.visibility.clone(),
    )
    .await;
    let fingerprint = sqlx::query_scalar::<_, String>(
        "SELECT node_key_fingerprint FROM current_node_chain_observations WHERE node_id = ?",
    )
    .bind(&row.node_id)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten();
    let identity = node_identity_status(&state, &row.node_id, &row.network_key).await;
    let transfer = sqlx::query_as::<_, NodeTransferRow>(&format!(
        "SELECT {TRANSFER_COLUMNS} FROM node_transfers WHERE node_id=? ORDER BY created_at DESC LIMIT 1"
    ))
    .bind(&row.node_id)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten()
    .map(node_transfer_dto);
    Json(AdminNodeDetail {
        node_id: row.node_id,
        agent_id: row.agent_id,
        display_name: row.display_name,
        network_key: row.network_key,
        network_display_name: row.network_display_name,
        lifecycle: row.lifecycle.clone(),
        lifecycle_guidance: lifecycle_guidance(&row.lifecycle).to_owned(),
        visibility: row.visibility,
        inventory_revision: row.inventory_revision,
        first_seen_at: row.first_seen_at,
        updated_at: row.updated_at,
        rpc_endpoint: redact_endpoint(&row.rpc_endpoint),
        node_key_fingerprint: fingerprint,
        health: diagnostic.health.clone(),
        health_reason: diagnostic.health_reason.clone(),
        freshness: diagnostic.freshness.clone(),
        identity,
        process: diagnostic.process,
        rpc: diagnostic.rpc,
        sync: diagnostic.sync,
        consensus: diagnostic.consensus,
        peers: diagnostic.peers,
        current_head: diagnostic.current_head,
        historical_high_watermark: diagnostic.historical_high_watermark,
        resync_state: diagnostic.resync_state,
        resync_progress: diagnostic.resync_progress,
        network_reference_head: diagnostic.network_reference_head,
        network_reference_confidence: diagnostic.network_reference_confidence,
        transfer,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/peer-churn",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    responses((status = 200, description = "Owner-only bounded Peer arrival/departure history", body = PeerChurnDiagnostic), (status = 404, body = crate::http::ApiErrorBody))
)]
async fn admin_node_peer_churn(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let exists = sqlx::query_scalar::<_, String>("SELECT node_id FROM nodes WHERE node_id=?")
        .bind(&node_id)
        .fetch_optional(state.db().pool())
        .await;
    let Some(_) = (match exists {
        Ok(node_id) => node_id,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    }) else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    match peer_churn_diagnostic(&state, &node_id).await {
        Ok(diagnostic) => Json(diagnostic).into_response(),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/peer-history",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    responses((status = 200, description = "Owner-only bounded aggregate Peer history", body = AdminPeerHistory), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_node_peer_history(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let exists = sqlx::query_scalar::<_, String>("SELECT node_id FROM nodes WHERE node_id=?")
        .bind(&node_id)
        .fetch_optional(state.db().pool())
        .await;
    let Some(_) = (match exists {
        Ok(node_id) => node_id,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    }) else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    match crate::peer_history::load_history(state.db().pool(), &node_id).await {
        Ok(history) => Json(admin_peer_history(history)).into_response(),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

#[derive(Debug, sqlx::FromRow)]
struct AgentAdminRow {
    agent_id: String,
    agent_epoch: i64,
    active_boot_id: Option<String>,
    active_boot_status: String,
    previous_boot_id: Option<String>,
    close_report_id: Option<String>,
    shutdown_state: String,
    shutdown_started_at: Option<String>,
    shutdown_deadline_at: Option<String>,
    shutdown_finished_at: Option<String>,
    shutdown_unresolved_from: Option<i64>,
    shutdown_unresolved_to: Option<i64>,
    shutdown_last_error: Option<String>,
    shutdown_forced: i64,
    shutdown_report_id: Option<String>,
    shutdown_report_sequence: Option<i64>,
    shutdown_updated_at: Option<String>,
    last_report_sequence: Option<i64>,
    agent_capabilities_json: String,
    clock_skew_ms: Option<i64>,
    clock_status: Option<String>,
    last_received_at: Option<String>,
    security_event_count: i64,
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/agents",
    tag = "admin",
    responses((status = 200, description = "Owner-only Agent and Node diagnostics", body = [AgentDiagnostic]))
)]
async fn diagnostics(
    State(state): State<AppState>,
    Extension(_session): Extension<super::AuthenticatedSession>,
) -> impl IntoResponse {
    let agents = sqlx::query_as::<_, AgentAdminRow>(

        "SELECT agent_id, agent_epoch, active_boot_id, active_boot_status, previous_boot_id, close_report_id, shutdown_state, shutdown_started_at, shutdown_deadline_at, shutdown_finished_at, shutdown_unresolved_from, shutdown_unresolved_to, shutdown_last_error, shutdown_forced, shutdown_report_id, shutdown_report_sequence, shutdown_updated_at, last_report_sequence, agent_capabilities_json, clock_skew_ms, clock_status, last_received_at, security_event_count FROM agents ORDER BY agent_id",
    )
    .fetch_all(state.db().pool())
    .await
    .unwrap_or_default();
    let mut result = Vec::with_capacity(agents.len());
    for row in agents {
        result.push(agent_diagnostic(&state, row).await);
    }
    Json(result)
}

/// Build one Server-owned Agent diagnostic with identity, liveness,
/// boot/report state, credential state, Inventory, and collector
/// diagnostics kept as separate dimensions (design §14.3, §14.4).
async fn agent_diagnostic(state: &AppState, row: AgentAdminRow) -> AgentDiagnostic {
    let AgentAdminRow {
        agent_id,
        agent_epoch,
        active_boot_id,
        active_boot_status: boot_status,
        previous_boot_id,
        close_report_id,
        shutdown_state,
        shutdown_started_at,
        shutdown_deadline_at,
        shutdown_finished_at,
        shutdown_unresolved_from,
        shutdown_unresolved_to,
        shutdown_last_error,
        shutdown_forced,
        shutdown_report_id,
        shutdown_report_sequence,
        shutdown_updated_at,
        last_report_sequence,
        agent_capabilities_json: capabilities_json,
        clock_skew_ms,
        clock_status,
        last_received_at,
        security_event_count,
    } = row;
    let capabilities = serde_json::from_str::<Vec<String>>(&capabilities_json)
        .unwrap_or_default()
        .into_iter()
        .map(|capability| crate::redaction::redact_sensitive(&capability))
        .collect();
    let liveness = agent_liveness(last_received_at.as_deref()).to_owned();
    let sequence_gap_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM report_sequence_gaps WHERE agent_id=?")
            .bind(&agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap_or(0);
    let credentials = credential_summaries(state, &agent_id).await;
    let host_components = sqlx::query_as::<_, (String, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64)>(
        "SELECT component_key, state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision FROM component_status WHERE agent_id = ? AND scope = 'host' ORDER BY component_key",
    )
    .bind(&agent_id)
    .fetch_all(state.db().pool())
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(component, state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision)| HostComponentDiagnostic {
        component,
        state,
        error_code,
        error_message: redact_optional_message(error_message),
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
    })
    .collect::<Vec<_>>();
    let host = sqlx::query_as::<_, HostProjectionRow>(
        "SELECT h.cpu_percent, h.memory_total_bytes, h.memory_used_bytes, h.load1, h.load5, h.load15, h.network_rx_bytes_per_sec, h.network_tx_bytes_per_sec, h.spool_queued_bytes, h.spool_queued_reports, h.spool_oldest_queued_age_ms, h.spool_in_flight, h.spool_last_delivery_error, h.spool_last_delivery_at, h.spool_capacity_bytes AS spool_capacity_bytes, h.spool_max_age_seconds AS spool_max_age_seconds, h.spool_dropped_sequence_from AS spool_dropped_sequence_from, h.spool_dropped_sequence_to AS spool_dropped_sequence_to, h.spool_dropped_time_from AS spool_dropped_time_from, h.spool_dropped_time_to AS spool_dropped_time_to, h.spool_dropped_height_from AS spool_dropped_height_from, h.spool_dropped_height_to AS spool_dropped_height_to, h.spool_pending_history_gaps AS spool_pending_history_gaps, h.spool_report_too_large AS spool_report_too_large, h.spool_store_fatal AS spool_store_fatal, h.spool_store_error AS spool_store_error, h.updated_at FROM current_host_observations h WHERE h.agent_id = ?",
    )
    .bind(&agent_id)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten()
    .map(|row| HostDiagnostic {
        cpu_percent: row.cpu_percent, memory_total_bytes: row.memory_total_bytes, memory_used_bytes: row.memory_used_bytes, load1: row.load1, load5: row.load5, load15: row.load15, network_rx_bytes_per_sec: row.network_rx_bytes_per_sec, network_tx_bytes_per_sec: row.network_tx_bytes_per_sec,
        spool_queued_bytes: row.spool_queued_bytes, spool_queued_reports: row.spool_queued_reports, spool_oldest_queued_age_ms: row.spool_oldest_queued_age_ms, spool_in_flight: row.spool_in_flight.map(|v| v != 0), spool_last_delivery_error: redact_optional_message(row.spool_last_delivery_error), spool_last_delivery_at: row.spool_last_delivery_at,
        spool_capacity_bytes: row.spool_capacity_bytes, spool_max_age_seconds: row.spool_max_age_seconds, spool_dropped_sequence_from: row.spool_dropped_sequence_from, spool_dropped_sequence_to: row.spool_dropped_sequence_to, spool_dropped_time_from: row.spool_dropped_time_from, spool_dropped_time_to: row.spool_dropped_time_to, spool_dropped_height_from: row.spool_dropped_height_from, spool_dropped_height_to: row.spool_dropped_height_to, spool_pending_history_gaps: row.spool_pending_history_gaps, spool_report_too_large: row.spool_report_too_large.map(|v| v != 0), spool_store_fatal: row.spool_store_fatal.map(|v| v != 0), spool_store_error: redact_optional_message(row.spool_store_error),
        updated_at: row.updated_at, components: host_components,
    });
    let rows = sqlx::query_as::<_, (String, String, Option<String>, String, i64, String)>(
        "SELECT node_id, network_key, display_name, lifecycle, inventory_revision, visibility FROM nodes WHERE agent_id = ? ORDER BY node_id",
    )
    .bind(&agent_id)
    .fetch_all(state.db().pool())
    .await
    .unwrap_or_default();
    let mut nodes = Vec::with_capacity(rows.len());
    for (node_id, network_key, display_name, lifecycle, inventory_revision, visibility) in rows {
        nodes.push(
            node_diagnostic(
                state,
                node_id,
                network_key,
                display_name,
                lifecycle,
                inventory_revision,
                visibility,
            )
            .await,
        );
    }
    AgentDiagnostic {
        agent_id,
        agent_epoch,
        last_report_sequence,
        clock_status: clock_status.unwrap_or_else(|| "unknown".to_owned()),
        clock_skew_ms,
        liveness,
        last_received_at,
        capabilities,
        active_boot_id,
        boot_status,
        previous_boot_id,
        close_report_id,
        shutdown_state,
        shutdown_started_at,
        shutdown_deadline_at,
        shutdown_finished_at,
        shutdown_unresolved_range: shutdown_unresolved_from.zip(shutdown_unresolved_to),
        shutdown_last_error: redact_optional_message(shutdown_last_error),
        shutdown_forced: shutdown_forced != 0,
        shutdown_report_id,
        shutdown_report_sequence,
        shutdown_updated_at,
        sequence_gap_count,
        security_event_count,
        credentials,
        host,
        nodes,
    }
}

/// Redacted credential state for one Agent, newest first. `active` is
/// Server-computed so the WebUI never derives security policy locally.
async fn credential_summaries(state: &AppState, agent_id: &str) -> Vec<AgentCredentialSummary> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
        "SELECT credential_id, created_at, revoked_at, revoke_after FROM agent_credentials WHERE agent_id = ? ORDER BY created_at DESC, credential_id",
    )
    .bind(agent_id)
    .fetch_all(state.db().pool())
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(credential_id, created_at, revoked_at, revoke_after)| {
        let active = revoked_at.is_none()
            && revoke_after.as_deref().is_none_or(|after| after > now.as_str());
        AgentCredentialSummary {
            credential_id,
            created_at,
            revoked_at,
            revoke_after,
            active,
        }
    })
    .collect()
}

/// Liveness rule shared by the diagnostics list and the overview: an Agent
/// is `online` only when a report arrived within the offline window.
fn agent_liveness(last_received_at: Option<&str>) -> &'static str {
    last_received_at
        .and_then(crate::auth::parse_rfc3339)
        .map(|received| {
            if (crate::auth::now_utc() - received).whole_seconds()
                <= crate::http::agent::AGENT_OFFLINE_AFTER_SECONDS
            {
                "online"
            } else {
                "offline"
            }
        })
        .unwrap_or("unknown")
}

/// Browser mutation trust boundary shared by every Admin security
/// mutation (design §13.1, §19.4): same-origin, JSON content type (when the
/// mutation carries a body), and a CSRF token matching the session —
/// checked before any body parsing. Bodyless mutations (e.g. revoke) do
/// not require the JSON content type because the generated browser client
/// omits it when there is no body.
pub(crate) fn mutation_guard(
    headers: &HeaderMap,
    principal: &super::AuthenticatedSession,
    auth: &crate::auth::AuthConfig,
    request_id: &super::RequestId,
    require_json_body: bool,
) -> Option<Response> {
    let content_type_valid = !require_json_body
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"));
    let origin_valid = auth.origin_matches(headers.get(header::ORIGIN));
    let csrf_valid = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| bool::from(token.as_bytes().ct_eq(principal.0.csrf_token.as_bytes())));
    if !content_type_valid || !origin_valid || !csrf_valid {
        return Some(mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        ));
    }
    None
}

/// One-time Enrollment Token issued to an Owner (design §4.5, §12.5). The
/// full token is delivered exactly once in the success response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct EnrollmentTokenResponse {
    pub token_id: String,
    pub token: String,
    pub expires_at: String,
    pub lifetime_hours: i64,
    pub request_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenLifetimeRequest {
    pub expires_in_hours: Option<i64>,
}

/// Create a single-use Enrollment Token for a new Agent. The response is
/// the Owner's only plaintext copy; the Server stores only the digest and
/// the Audit row records the token id and expiry — never the token.
#[utoipa::path(
    post,
    path = "/api/admin/v1/agents/enroll-token",
    tag = "admin",
    request_body = TokenLifetimeRequest,
    responses((status = 200, body = EnrollmentTokenResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_enrollment_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, true) {
        return response;
    }
    let body: TokenLifetimeRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    let lifetime_hours = body.expires_in_hours.unwrap_or(24);
    if !(1..=168).contains(&lifetime_hours) {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_lifetime",
            "enrollment token lifetime must be 1..=168 hours",
        );
    }
    let lifetime = std::time::Duration::from_secs((lifetime_hours as u64) * 3600);
    match crate::enrollment::create_enrollment_token(
        state.db(),
        &state.auth().pepper,
        Some(&principal.0.user_id),
        lifetime,
    )
    .await
    {
        Ok(record) => no_store(
            Json(EnrollmentTokenResponse {
                token_id: record.token_id,
                token: record.token,
                expires_at: record.expires_at,
                lifetime_hours,
                request_id: request_id.0.to_string(),
            })
            .into_response(),
        ),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// One-time Recovery Token issued for an existing Agent. Exchanging it
/// advances the Agent Epoch and rotates the credential without creating a
/// duplicate Agent (design §4.5).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct RecoveryTokenResponse {
    pub agent_id: String,
    pub agent_epoch: i64,
    pub token_id: String,
    pub token: String,
    pub expires_at: String,
    pub request_id: String,
}

/// Create a single-use Recovery Token for an existing Agent (credential
/// loss, design §4.5). The response is the Owner's only plaintext copy.
#[utoipa::path(
    post,
    path = "/api/admin/v1/agents/{agent_id}/recover",
    tag = "admin",
    params(("agent_id" = String, Path, description = "Agent ID")),
    request_body = TokenLifetimeRequest,
    responses((status = 200, body = RecoveryTokenResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_recovery_token(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, true) {
        return response;
    }
    let body: TokenLifetimeRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    let lifetime_hours = body.expires_in_hours.unwrap_or(24);
    if !(1..=168).contains(&lifetime_hours) {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_lifetime",
            "recovery token lifetime must be 1..=168 hours",
        );
    }
    let lifetime = std::time::Duration::from_secs((lifetime_hours as u64) * 3600);
    let epoch: Option<i64> =
        match sqlx::query_scalar("SELECT agent_epoch FROM agents WHERE agent_id = ?")
            .bind(&agent_id)
            .fetch_optional(state.db().pool())
            .await
        {
            Ok(epoch) => epoch,
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
        };
    let Some(agent_epoch) = epoch else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent not found",
        );
    };
    match crate::enrollment::create_recovery_token(
        state.db(),
        &state.auth().pepper,
        Some(&principal.0.user_id),
        &agent_id,
        lifetime,
    )
    .await
    {
        Ok(record) => no_store(
            Json(RecoveryTokenResponse {
                agent_id,
                agent_epoch,
                token_id: record.token_id,
                token: record.token,
                expires_at: record.expires_at,
                request_id: request_id.0.to_string(),
            })
            .into_response(),
        ),
        Err(crate::enrollment::RecoveryError::AgentNotFound) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent not found",
        ),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RotationRequest {
    pub overlap_hours: Option<i64>,
    pub revoke_previous: Option<bool>,
}

/// Rotation result: the new credential secret is shown once; every
/// previously valid credential either stays valid through `revoke_after`
/// or was revoked immediately (design §12.6).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct RotationResponse {
    pub agent_id: String,
    pub credential_id: String,
    pub credential: String,
    pub created_at: String,
    pub overlap_hours: i64,
    pub revoke_after: Option<String>,
    pub revoked_previous_ids: Vec<String>,
    pub overlap_credential_ids: Vec<String>,
    pub request_id: String,
}

/// Rotate an Agent credential: issue a fresh credential, keep the previous
/// one valid through an explicit overlap window, and optionally revoke it
/// immediately. Distinct from recovery: the Agent Epoch is untouched and
/// no duplicate Agent is created.
#[utoipa::path(
    post,
    path = "/api/admin/v1/agents/{agent_id}/credentials/rotate",
    tag = "admin",
    params(("agent_id" = String, Path, description = "Agent ID")),
    request_body = RotationRequest,
    responses((status = 200, body = RotationResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_rotate_credential(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, true) {
        return response;
    }
    let body: RotationRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    let overlap_hours = body.overlap_hours.unwrap_or(24);
    if !(1..=168).contains(&overlap_hours) {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_lifetime",
            "overlap window must be 1..=168 hours",
        );
    }
    let overlap = std::time::Duration::from_secs((overlap_hours as u64) * 3600);
    match crate::enrollment::rotate_agent_credential(
        state.db(),
        &state.auth().pepper,
        Some(&principal.0.user_id),
        &agent_id,
        overlap,
        body.revoke_previous.unwrap_or(false),
    )
    .await
    {
        Ok(rotated) => no_store(
            Json(RotationResponse {
                agent_id: rotated.agent_id,
                credential_id: rotated.credential_id,
                credential: rotated.credential,
                created_at: rotated.created_at,
                overlap_hours: rotated.overlap_hours,
                revoke_after: rotated.revoke_after,
                revoked_previous_ids: rotated.revoked_previous_ids,
                overlap_credential_ids: rotated.overlap_credential_ids,
                request_id: request_id.0.to_string(),
            })
            .into_response(),
        ),
        Err(crate::enrollment::RotationError::AgentNotFound) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent not found",
        ),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct RevokeResponse {
    pub agent_id: String,
    pub credential_id: String,
    pub revoked_at: String,
    pub request_id: String,
}

/// Revoke one Agent credential immediately (design §12.6: revoke takes
/// effect immediately). The Audit row records the credential id and
/// instant only.
#[utoipa::path(
    post,
    path = "/api/admin/v1/agents/{agent_id}/credentials/{credential_id}/revoke",
    tag = "admin",
    params(("agent_id" = String, Path, description = "Agent ID"), ("credential_id" = String, Path, description = "Credential ID")),
    responses((status = 200, body = RevokeResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_revoke_credential(
    State(state): State<AppState>,
    Path((agent_id, credential_id)): Path<(String, String)>,
    headers: HeaderMap,
    Extension(principal): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, false) {
        return response;
    }
    match crate::enrollment::revoke_agent_credential(
        state.db(),
        Some(&principal.0.user_id),
        &agent_id,
        &credential_id,
    )
    .await
    {
        Ok(revoked) => Json(RevokeResponse {
            agent_id: revoked.agent_id,
            credential_id: revoked.credential_id,
            revoked_at: revoked.revoked_at,
            request_id: request_id.0.to_string(),
        })
        .into_response(),
        Err(crate::enrollment::RevokeError::NotFound) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "credential_not_found",
            "agent or credential not found",
        ),
        Err(crate::enrollment::RevokeError::AlreadyRevoked) => mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "credential_already_revoked",
            "credential is already revoked",
        ),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// One redacted Audit row for an Agent lifecycle event. The stored
/// `after_json` bodies are redacted by construction: they carry ids,
/// instants, and counts, never token or credential plaintext.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentAuditItem {
    pub audit_event_id: i64,
    pub event_kind: String,
    pub actor_username: Option<String>,
    pub created_at: String,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentAuditResponse {
    pub agent_id: String,
    pub items: Vec<AgentAuditItem>,
}

/// Redacted Audit trail scoped to one Agent (design §9, §14.3: security
/// mutations carry a redacted Audit link; immutable event listing).
#[utoipa::path(
    get,
    path = "/api/admin/v1/agents/{agent_id}/audit",
    tag = "admin",
    params(("agent_id" = String, Path, description = "Agent ID")),
    responses((status = 200, body = AgentAuditResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_agent_audit(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let agent_exists =
        sqlx::query_scalar::<_, String>("SELECT agent_id FROM agents WHERE agent_id = ?")
            .bind(&agent_id)
            .fetch_optional(state.db().pool())
            .await
            .ok()
            .flatten()
            .is_some();
    if !agent_exists {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent not found",
        );
    }
    let rows = sqlx::query_as::<_, (i64, String, Option<String>, String, Option<String>)>(
        "SELECT a.audit_event_id, a.event_kind, u.username, a.created_at, a.after_json
         FROM audit_events a LEFT JOIN users u ON u.user_id = a.actor_user_id
         WHERE a.target_kind = 'agent' AND a.target_id = ?
         ORDER BY a.created_at DESC, a.audit_event_id DESC LIMIT 50",
    )
    .bind(&agent_id)
    .fetch_all(state.db().pool())
    .await
    .unwrap_or_default();
    Json(AgentAuditResponse {
        agent_id,
        items: rows
            .into_iter()
            .map(
                |(audit_event_id, event_kind, actor_username, created_at, after_json)| {
                    AgentAuditItem {
                        audit_event_id,
                        event_kind,
                        actor_username: actor_username
                            .map(|username| crate::redaction::redact_sensitive(&username)),
                        created_at,
                        details: after_json
                            .as_deref()
                            .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
                            .map(|value| crate::redaction::redact_json_value(&value)),
                    }
                },
            )
            .collect(),
    })
    .into_response()
}

/// Owner-only Agent detail: one full AgentDiagnostic (identity, liveness,
/// boot/report state, credential state, Inventory, diagnostics).
#[utoipa::path(
    get,
    path = "/api/admin/v1/agents/{agent_id}",
    tag = "admin",
    params(("agent_id" = String, Path, description = "Agent ID")),
    responses((status = 200, body = AgentDiagnostic), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_agent_detail(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let Some(row) = sqlx::query_as::<_, AgentAdminRow>(
        "SELECT agent_id, agent_epoch, active_boot_id, active_boot_status, previous_boot_id, close_report_id, shutdown_state, shutdown_started_at, shutdown_deadline_at, shutdown_finished_at, shutdown_unresolved_from, shutdown_unresolved_to, shutdown_last_error, shutdown_forced, shutdown_report_id, shutdown_report_sequence, shutdown_updated_at, last_report_sequence, agent_capabilities_json, clock_skew_ms, clock_status, last_received_at, security_event_count FROM agents WHERE agent_id = ?",
    )
    .bind(&agent_id)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten()
    else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent not found",
        );
    };
    Json(agent_diagnostic(&state, row).await).into_response()
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminOverview {
    pub generated_at: String,
    pub summary: AdminOverviewSummary,
    /// Server-owned attention queue. The WebUI presents these items and
    /// never recomputes health policy or attention in the browser.
    pub attention: Vec<AttentionItem>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminOverviewSummary {
    pub agents: AgentSummary,
    pub nodes: NodeSummary,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentSummary {
    pub total: i64,
    pub online: i64,
    pub offline: i64,
    pub unknown: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NodeSummary {
    pub total: i64,
    pub healthy: i64,
    pub unhealthy: i64,
    pub unknown: i64,
    /// Nodes in a non-active lifecycle (e.g. retired); not part of the
    /// health buckets because no observation policy applies to them.
    pub retired: i64,
    /// Nodes published to the Public projection.
    pub published: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AttentionItem {
    /// Stable item key (kind + subject) for list rendering and tests.
    pub id: String,
    pub kind: String,
    pub severity: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub subject_label: String,
    pub message: String,
    pub observed_at: String,
}

#[derive(Debug, sqlx::FromRow)]
struct OverviewAgentRow {
    agent_id: String,
    last_received_at: Option<String>,
    shutdown_state: String,
    security_event_count: i64,
    sequence_gap_count: i64,
    spool_store_fatal: Option<i64>,
    spool_dropped_sequence_to: Option<i64>,
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "critical" => 0,
        "warning" => 1,
        _ => 2,
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/overview",
    tag = "admin",
    responses((status = 200, description = "Server-owned attention queue and overview summary", body = AdminOverview))
)]
async fn overview(
    State(state): State<AppState>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let generated_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    // A database failure is a Server failure: it must surface as an error
    // envelope, never as an authoritative empty queue (webui.md §5.3).
    let agents = match sqlx::query_as::<_, OverviewAgentRow>(
        "SELECT a.agent_id, a.last_received_at, a.shutdown_state, a.security_event_count, (SELECT COUNT(*) FROM report_sequence_gaps g WHERE g.agent_id = a.agent_id) AS sequence_gap_count, h.spool_store_fatal, h.spool_dropped_sequence_to FROM agents a LEFT JOIN current_host_observations h ON h.agent_id = a.agent_id ORDER BY a.agent_id",
    )
    .fetch_all(state.db().pool())
    .await
    {
        Ok(agents) => agents,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let mut agent_summary = AgentSummary {
        total: agents.len() as i64,
        online: 0,
        offline: 0,
        unknown: 0,
    };
    let mut attention: Vec<AttentionItem> = Vec::new();
    for agent in &agents {
        let liveness = agent_liveness(agent.last_received_at.as_deref());
        match liveness {
            "online" => agent_summary.online += 1,
            "offline" => agent_summary.offline += 1,
            _ => agent_summary.unknown += 1,
        }
        if liveness == "offline" {
            attention.push(AttentionItem {
                id: format!("agent_offline:agent:{}", agent.agent_id),
                kind: "agent_offline".to_owned(),
                severity: "warning".to_owned(),
                subject_kind: "agent".to_owned(),
                subject_id: agent.agent_id.clone(),
                subject_label: agent.agent_id.clone(),
                message: "the Agent has not reported within the liveness window".to_owned(),
                observed_at: agent
                    .last_received_at
                    .clone()
                    .unwrap_or_else(|| generated_at.clone()),
            });
        }
        if agent.spool_store_fatal.is_some_and(|value| value != 0) {
            attention.push(AttentionItem {
                id: format!("agent_spool_fatal:agent:{}", agent.agent_id),
                kind: "agent_spool_fatal".to_owned(),
                severity: "critical".to_owned(),
                subject_kind: "agent".to_owned(),
                subject_id: agent.agent_id.clone(),
                subject_label: agent.agent_id.clone(),
                message: "the Agent spool store is in a fatal state; durable reports are at risk"
                    .to_owned(),
                observed_at: generated_at.clone(),
            });
        }
        if agent.spool_dropped_sequence_to.is_some() {
            attention.push(AttentionItem {
                id: format!("agent_spool_overflow:agent:{}", agent.agent_id),
                kind: "agent_spool_overflow".to_owned(),
                severity: "warning".to_owned(),
                subject_kind: "agent".to_owned(),
                subject_id: agent.agent_id.clone(),
                subject_label: agent.agent_id.clone(),
                message: "the Agent spool overflowed and discarded queued reports".to_owned(),
                observed_at: generated_at.clone(),
            });
        }
        if agent.sequence_gap_count > 0 {
            attention.push(AttentionItem {
                id: format!("agent_report_gap:agent:{}", agent.agent_id),
                kind: "agent_report_gap".to_owned(),
                severity: "warning".to_owned(),
                subject_kind: "agent".to_owned(),
                subject_id: agent.agent_id.clone(),
                subject_label: agent.agent_id.clone(),
                message: format!(
                    "{} report sequence gap{} recorded",
                    agent.sequence_gap_count,
                    if agent.sequence_gap_count == 1 {
                        " was"
                    } else {
                        "s were"
                    }
                ),
                observed_at: generated_at.clone(),
            });
        }
        if agent.security_event_count > 0 {
            attention.push(AttentionItem {
                id: format!("agent_security_event:agent:{}", agent.agent_id),
                kind: "agent_security_event".to_owned(),
                severity: "critical".to_owned(),
                subject_kind: "agent".to_owned(),
                subject_id: agent.agent_id.clone(),
                subject_label: agent.agent_id.clone(),
                message: format!(
                    "{} security event{} recorded",
                    agent.security_event_count,
                    if agent.security_event_count == 1 {
                        " was"
                    } else {
                        "s were"
                    }
                ),
                observed_at: generated_at.clone(),
            });
        }
        if matches!(
            agent.shutdown_state.as_str(),
            "stopping" | "draining" | "send_failed" | "forced_kill_recovery"
        ) {
            attention.push(AttentionItem {
                id: format!("agent_shutdown_incomplete:agent:{}", agent.agent_id),
                kind: "agent_shutdown_incomplete".to_owned(),
                severity: "warning".to_owned(),
                subject_kind: "agent".to_owned(),
                subject_id: agent.agent_id.clone(),
                subject_label: agent.agent_id.clone(),
                message: format!("the Agent shutdown is {}", agent.shutdown_state),
                observed_at: generated_at.clone(),
            });
        }
    }
    let rows = match sqlx::query_as::<_, (String, String, Option<String>, String, i64, String)>(
        "SELECT node_id, network_key, display_name, lifecycle, inventory_revision, visibility FROM nodes ORDER BY node_id",
    )
    .fetch_all(state.db().pool())
    .await
    {
        Ok(rows) => rows,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let mut node_summary = NodeSummary {
        total: rows.len() as i64,
        healthy: 0,
        unhealthy: 0,
        unknown: 0,
        retired: 0,
        published: 0,
    };
    for (node_id, network_key, display_name, lifecycle, inventory_revision, visibility) in rows {
        let diagnostic = node_diagnostic(
            &state,
            node_id.clone(),
            network_key,
            display_name.clone(),
            lifecycle.clone(),
            inventory_revision,
            visibility.clone(),
        )
        .await;
        if visibility == "public" {
            node_summary.published += 1;
        }
        if lifecycle != "active" {
            node_summary.retired += 1;
            continue;
        }
        let observed_at = diagnostic
            .rpc
            .as_ref()
            .and_then(|c| c.received_at.as_deref())
            .into_iter()
            .chain(
                diagnostic
                    .sync
                    .as_ref()
                    .and_then(|c| c.received_at.as_deref()),
            )
            .chain(
                diagnostic
                    .consensus
                    .as_ref()
                    .and_then(|c| c.received_at.as_deref()),
            )
            .filter_map(crate::auth::parse_rfc3339)
            .max()
            .map(crate::auth::format_rfc3339)
            .unwrap_or_else(|| generated_at.clone());
        match diagnostic.health.as_str() {
            "healthy" => node_summary.healthy += 1,
            "unhealthy" => {
                node_summary.unhealthy += 1;
                attention.push(AttentionItem {
                    id: format!("node_unhealthy:node:{node_id}"),
                    kind: "node_unhealthy".to_owned(),
                    severity: "critical".to_owned(),
                    subject_kind: "node".to_owned(),
                    subject_id: node_id.clone(),
                    subject_label: display_name.unwrap_or_else(|| node_id.clone()),
                    message: diagnostic.health_reason.clone(),
                    observed_at: observed_at.clone(),
                });
            }
            _ => {
                node_summary.unknown += 1;
                attention.push(AttentionItem {
                    id: format!("node_health_unknown:node:{node_id}"),
                    kind: "node_health_unknown".to_owned(),
                    severity: "warning".to_owned(),
                    subject_kind: "node".to_owned(),
                    subject_id: node_id.clone(),
                    subject_label: display_name.unwrap_or_else(|| node_id.clone()),
                    message: diagnostic.health_reason.clone(),
                    observed_at: observed_at.clone(),
                });
            }
        }
        if diagnostic.resync_state != "normal" {
            attention.push(AttentionItem {
                id: format!("node_resync:node:{node_id}"),
                kind: "node_resync".to_owned(),
                severity: "warning".to_owned(),
                subject_kind: "node".to_owned(),
                subject_id: node_id.clone(),
                subject_label: node_id.clone(),
                message: format!("the Node resync state is {}", diagnostic.resync_state),
                observed_at: observed_at.clone(),
            });
        }
    }
    attention.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| a.id.cmp(&b.id))
    });
    Json(AdminOverview {
        generated_at,
        summary: AdminOverviewSummary {
            agents: agent_summary,
            nodes: node_summary,
        },
        attention,
    })
    .into_response()
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminBlockHistoryItem {
    pub node_id: String,
    pub height: Option<i64>,
    pub block_time_ms: Option<i64>,
    pub transaction_count: Option<i64>,
    pub coinbase: Option<String>,
    pub seal_signer_match: Option<String>,
    pub seal_signer_key_fingerprint: Option<String>,
    pub node_key_fingerprint: Option<String>,
    pub node_key_valid_from: Option<String>,
    pub node_key_valid_until: Option<String>,
    pub seal_recovery_rule: Option<String>,
    pub seal_evidence: Option<String>,
    pub protocol_proposer: Option<String>,
    pub attribution_reason: Option<String>,
    pub observed_at: Option<String>,
    pub freshness: Option<String>,
    pub gap_from_height: Option<i64>,
    pub gap_to_height: Option<i64>,
    pub gap_kind: Option<String>,
    pub gap_reason: Option<String>,
    pub divergence_kind: Option<String>,
    pub divergence_reason: Option<String>,
    pub divergence_retained_hash: Option<String>,
    pub divergence_observed_hash: Option<String>,
    pub divergence_observed_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminBlockHistoryResponse {
    pub items: Vec<AdminBlockHistoryItem>,
    /// `unavailable` when the requested range extends beyond raw retention.
    pub availability: Option<String>,
    pub aggregate_supported: bool,
    pub raw_retention_days: i64,
}

#[derive(Debug, Deserialize)]
struct AdminHistoryQuery {
    limit: Option<i64>,
    from: Option<i64>,
    to: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct AdminHistoryRow {
    block_number: Option<i64>,
    block_timestamp_ms: Option<i64>,
    transaction_count: Option<i64>,
    coinbase: Option<String>,
    seal_signer_match: Option<String>,
    seal_signer_key_fingerprint: Option<String>,
    node_key_fingerprint: Option<String>,
    node_key_valid_from: Option<String>,
    node_key_valid_until: Option<String>,
    seal_recovery_rule: Option<String>,
    seal_evidence: Option<String>,
    protocol_proposer: Option<String>,
    attribution_reason: Option<String>,
    observed_at: Option<String>,
    from_height: Option<i64>,
    to_height: Option<i64>,
    gap_kind: Option<String>,
    gap_reason: Option<String>,
    divergence_kind: Option<String>,
    divergence_reason: Option<String>,
    divergence_retained_hash: Option<String>,
    divergence_observed_hash: Option<String>,
    divergence_observed_at: Option<String>,
}

fn admin_history_bounds(
    params: &AdminHistoryQuery,
    request_id: &str,
) -> Result<(Option<i64>, Option<i64>), Box<Response>> {
    if params.from.is_some_and(|value| value < 0)
        || params.to.is_some_and(|value| value < 0)
        || params
            .from
            .zip(params.to)
            .is_some_and(|(from, to)| from > to)
    {
        return Err(Box::new(mutation_error(
            request_id,
            StatusCode::BAD_REQUEST,
            "invalid_history_range",
            "history range is invalid",
        )));
    }
    Ok((params.from, params.to))
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/history",
    tag = "admin",
    params(
        ("node_id" = String, Path, description = "Node ID"),
        ("from" = Option<i64>, Query, minimum = 0, description = "First block height"),
        ("to" = Option<i64>, Query, minimum = 0, description = "Last block height"),
        ("limit" = Option<i64>, Query, minimum = 1, maximum = 200, description = "Maximum rows")
    ),
    responses(
        (status = 200, body = AdminBlockHistoryResponse),
        (status = 400, body = crate::http::ApiErrorBody),
        (status = 401, body = crate::http::ApiErrorBody),
        (status = 403, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
async fn admin_node_history(
    State(state): State<AppState>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
    axum::extract::Query(params): axum::extract::Query<AdminHistoryQuery>,
    Path(node_id): Path<String>,
) -> Response {
    let (from, to) = match admin_history_bounds(&params, &request_id.0) {
        Ok(bounds) => bounds,
        Err(response) => return *response,
    };
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    match sqlx::query_scalar::<_, i64>("SELECT 1 FROM nodes WHERE node_id=?")
        .bind(&node_id)
        .fetch_optional(state.db().pool())
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return mutation_error(
                &request_id.0,
                StatusCode::NOT_FOUND,
                "not_found",
                "resource not found",
            );
        }
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    }
    let raw_retention_days =
        match crate::retention::raw_block_summary_retention_days(state.db().pool()).await {
            Ok(days) => days,
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
        };
    let cutoff = crate::auth::format_rfc3339(crate::retention::family_cutoff(
        crate::auth::now_utc(),
        raw_retention_days,
    ));
    let oldest_raw = sqlx::query_scalar::<_, Option<String>>(
        "SELECT MIN(accepted_at) FROM block_summaries WHERE node_id=?",
    )
    .bind(&node_id)
    .fetch_one(state.db().pool())
    .await;
    let oldest_raw = match oldest_raw {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let has_history = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT historical_high_watermark FROM block_history_state WHERE node_id=?",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await;
    let has_history = match has_history {
        Ok(value) => value.flatten().is_some_and(|height| height > 0),
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let has_expired_raw = oldest_raw.as_ref().is_some_and(|value| value < &cutoff);
    let availability = (has_expired_raw || (oldest_raw.is_none() && has_history))
        .then(|| "unavailable".to_owned());
    let rows = sqlx::query_as::<_, AdminHistoryRow>("SELECT block_number, block_timestamp_ms, transaction_count, source, coinbase, seal_signer_match, seal_signer_key_fingerprint, node_key_fingerprint, node_key_valid_from, node_key_valid_until, seal_recovery_rule, seal_evidence, CASE WHEN protocol_proposer_kind = 'verified' THEN protocol_proposer_identity ELSE NULL END, attribution_reason, observed_at, from_height, to_height, gap_kind, gap_reason, divergence_kind, divergence_reason, divergence_retained_hash, divergence_observed_hash, divergence_observed_at FROM (SELECT block_number, block_timestamp_ms, transaction_count, 'summary', coinbase, seal_signer_match, seal_signer_key_fingerprint, node_key_fingerprint, node_key_valid_from, node_key_valid_until, seal_recovery_rule, seal_evidence, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, observed_at, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL FROM block_summaries WHERE node_id = ? AND accepted_at >= ? UNION ALL SELECT NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, created_at, from_height, to_height, kind, reason, NULL, NULL, NULL, NULL, NULL FROM block_history_gaps WHERE node_id = ? UNION ALL SELECT NULL, NULL, NULL, 'divergence', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, retained_observed_at, height, height, NULL, NULL, 'chain_divergence', reason, retained_block_hash, observed_block_hash, observed_at FROM chain_divergence_observations WHERE node_id = ?) WHERE (? IS NULL OR COALESCE(block_number, to_height) >= ?) AND (? IS NULL OR COALESCE(block_number, from_height) <= ?) ORDER BY COALESCE(block_number, from_height) DESC LIMIT ?")
        .bind(&node_id)
        .bind(&cutoff)
        .bind(&node_id)
        .bind(&node_id)
        .bind(from)
        .bind(from)
        .bind(to)
        .bind(to)
        .bind(200_i64)
        .fetch_all(state.db().pool())
        .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let mut rows = rows;
    rows.retain(|row| {
        let first_height = row.block_number.or(row.from_height);
        let last_height = row.block_number.or(row.to_height);
        from.is_none_or(|minimum| last_height.is_some_and(|height| height >= minimum))
            && to.is_none_or(|maximum| first_height.is_some_and(|height| height <= maximum))
    });
    rows.truncate(limit as usize);
    Json(AdminBlockHistoryResponse {
        items: rows
            .into_iter()
            .map(|row| AdminBlockHistoryItem {
                node_id: node_id.clone(),
                height: row.block_number,
                block_time_ms: row.block_timestamp_ms,
                transaction_count: row.transaction_count,
                coinbase: row.coinbase,
                seal_signer_match: row.seal_signer_match,
                seal_signer_key_fingerprint: row.seal_signer_key_fingerprint,
                node_key_fingerprint: row.node_key_fingerprint,
                node_key_valid_from: row.node_key_valid_from,
                node_key_valid_until: row.node_key_valid_until,
                seal_recovery_rule: row.seal_recovery_rule,
                seal_evidence: row
                    .seal_evidence
                    .map(|value| crate::redaction::redact_sensitive(&value)),
                protocol_proposer: row.protocol_proposer,
                attribution_reason: row
                    .attribution_reason
                    .map(|value| crate::redaction::redact_sensitive(&value)),
                freshness: row.observed_at.clone(),
                observed_at: row.observed_at,
                gap_from_height: row.from_height,
                gap_to_height: row.to_height,
                gap_kind: row.gap_kind,
                gap_reason: row
                    .gap_reason
                    .map(|value| crate::redaction::redact_sensitive(&value)),
                divergence_kind: row.divergence_kind,
                divergence_reason: row
                    .divergence_reason
                    .map(|value| crate::redaction::redact_sensitive(&value)),
                divergence_retained_hash: row.divergence_retained_hash,
                divergence_observed_hash: row.divergence_observed_hash,
                divergence_observed_at: row.divergence_observed_at,
            })
            .collect::<Vec<_>>(),
        availability,
        aggregate_supported: crate::retention::RAW_BLOCK_HISTORY_AGGREGATES_SUPPORTED,
        raw_retention_days,
    })
    .into_response()
}
/// Owner-only Network Registry projection (design §7.1). The complete
/// validated identity tuple is presented as Server-owned expected identity;
/// observed Agent text never creates or rewrites Registry entries.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminNetwork {
    pub network_key: String,
    pub display_name: String,
    pub genesis_hash: String,
    pub chain_id: i64,
    pub p2p_network_id: i64,
    pub address_hrp: String,
    pub created_at: String,
    pub updated_at: String,
    pub active_node_count: i64,
    pub retired_node_count: i64,
    /// Active Nodes whose observed identity contradicts this Registry tuple.
    pub mismatched_node_count: i64,
}

const NETWORK_NODE_COUNTS: &str = "(SELECT COUNT(*) FROM nodes nd WHERE nd.network_key = n.network_key AND nd.lifecycle = 'active'), (SELECT COUNT(*) FROM nodes nd WHERE nd.network_key = n.network_key AND nd.lifecycle = 'retired'), (SELECT COUNT(*) FROM nodes nd WHERE nd.network_key = n.network_key AND nd.lifecycle = 'active' AND EXISTS (SELECT 1 FROM current_node_chain_observations c WHERE c.node_id = nd.node_id AND ((c.network_genesis_hash IS NOT NULL AND c.network_genesis_hash != n.genesis_hash) OR (c.network_chain_id IS NOT NULL AND c.network_chain_id != n.chain_id) OR (c.network_p2p_network_id IS NOT NULL AND c.network_p2p_network_id != n.p2p_network_id) OR (c.network_address_hrp IS NOT NULL AND c.network_address_hrp != n.address_hrp))))";

#[utoipa::path(
    get,
    path = "/api/admin/v1/networks",
    tag = "admin",
    responses((status = 200, description = "Owner-only Network Registry with identity tuple and Node counts", body = [AdminNetwork]))
)]
async fn admin_networks(
    State(state): State<AppState>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let rows = sqlx::query_as::<_, (String, String, String, i64, i64, String, String, String, i64, i64, i64)>(
        &format!("SELECT n.network_key, n.display_name, n.genesis_hash, n.chain_id, n.p2p_network_id, n.address_hrp, n.created_at, n.updated_at, {NETWORK_NODE_COUNTS} FROM networks n ORDER BY n.network_key"),
    )
    .fetch_all(state.db().pool())
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    Json(
        rows.into_iter()
            .map(
                |(
                    network_key,
                    display_name,
                    genesis_hash,
                    chain_id,
                    p2p_network_id,
                    address_hrp,
                    created_at,
                    updated_at,
                    active_node_count,
                    retired_node_count,
                    mismatched_node_count,
                )| {
                    AdminNetwork {
                        network_key,
                        display_name,
                        genesis_hash,
                        chain_id,
                        p2p_network_id,
                        address_hrp,
                        created_at,
                        updated_at,
                        active_node_count,
                        retired_node_count,
                        mismatched_node_count,
                    }
                },
            )
            .collect::<Vec<_>>(),
    )
    .into_response()
}

/// One Node inside a Network detail: per-Node identity disposition against
/// the Registry tuple, plus the Server-owned health and lifecycle state.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminNetworkNode {
    pub node_id: String,
    pub agent_id: String,
    pub display_name: Option<String>,
    pub lifecycle: String,
    pub visibility: String,
    pub health: String,
    pub health_reason: String,
    pub freshness: String,
    pub current_head: Option<i64>,
    pub resync_state: String,
    pub identity: NodeIdentityStatus,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminNetworkDetail {
    pub network_key: String,
    pub display_name: String,
    pub genesis_hash: String,
    pub chain_id: i64,
    pub p2p_network_id: i64,
    pub address_hrp: String,
    pub created_at: String,
    pub updated_at: String,
    pub active_node_count: i64,
    pub retired_node_count: i64,
    pub mismatched_node_count: i64,
    pub nodes: Vec<AdminNetworkNode>,
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/networks/{network_key}",
    tag = "admin",
    params(("network_key" = String, Path, description = "Registered Network key")),
    responses((status = 200, description = "Owner-only Network detail with per-Node identity dispositions", body = AdminNetworkDetail), (status = 404, body = crate::http::ApiErrorBody))
)]
async fn admin_network_detail(
    State(state): State<AppState>,
    Path(network_key): Path<String>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64, String, String, String, i64, i64, i64)>(
        &format!("SELECT n.network_key, n.display_name, n.genesis_hash, n.chain_id, n.p2p_network_id, n.address_hrp, n.created_at, n.updated_at, {NETWORK_NODE_COUNTS} FROM networks n WHERE n.network_key = ?"),
    )
    .bind(&network_key)
    .fetch_optional(state.db().pool())
    .await;
    let Some((
        network_key,
        display_name,
        genesis_hash,
        chain_id,
        p2p_network_id,
        address_hrp,
        created_at,
        updated_at,
        active_node_count,
        retired_node_count,
        mismatched_node_count,
    )) = (match row {
        Ok(row) => row,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    })
    else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    let nodes = sqlx::query_as::<_, (String, String, Option<String>, String, String)>(
        "SELECT node_id, agent_id, display_name, lifecycle, visibility FROM nodes WHERE network_key = ? ORDER BY COALESCE(display_name, node_id)",
    )
    .bind(&network_key)
    .fetch_all(state.db().pool())
    .await;
    let nodes = match nodes {
        Ok(nodes) => nodes,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let mut projected = Vec::with_capacity(nodes.len());
    for (node_id, agent_id, node_display_name, lifecycle, visibility) in nodes {
        let diagnostic = node_diagnostic(
            &state,
            node_id.clone(),
            network_key.clone(),
            node_display_name.clone(),
            lifecycle.clone(),
            0,
            visibility.clone(),
        )
        .await;
        projected.push(AdminNetworkNode {
            node_id: node_id.clone(),
            agent_id,
            display_name: node_display_name,
            lifecycle: lifecycle.clone(),
            visibility,
            health: diagnostic.health,
            health_reason: diagnostic.health_reason,
            freshness: diagnostic.freshness,
            current_head: diagnostic.current_head,
            resync_state: diagnostic.resync_state,
            identity: node_identity_status(&state, &node_id, &network_key).await,
        });
    }
    Json(AdminNetworkDetail {
        network_key,
        display_name,
        genesis_hash,
        chain_id,
        p2p_network_id,
        address_hrp,
        created_at,
        updated_at,
        active_node_count,
        retired_node_count,
        mismatched_node_count,
        nodes: projected,
    })
    .into_response()
}

/// Owner-only Registry creation with the complete validated identity tuple
/// (design §7.1). The Registry is never created from observed Agent text:
/// this explicit Owner mutation is the only Admin insert path.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetworkCreateRequest {
    pub network_key: String,
    pub display_name: String,
    pub genesis_hash: String,
    pub chain_id: u64,
    pub p2p_network_id: u64,
    pub address_hrp: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetworkResponse {
    pub network_key: String,
    pub display_name: String,
}

#[utoipa::path(
    post,
    path = "/api/admin/v1/networks",
    tag = "admin",
    request_body = NetworkCreateRequest,
    responses((status = 200, body = NetworkResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn create_network(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: NetworkCreateRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    let key = match crate::network::validate_network_tuple(
        &body.network_key,
        &body.display_name,
        &body.genesis_hash,
        body.chain_id,
        body.p2p_network_id,
        &body.address_hrp,
    ) {
        Ok(key) => key,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_network_tuple",
                "the Network identity tuple is invalid",
            );
        }
    };
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let insert = sqlx::query(
        "INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&key)
    .bind(&body.display_name)
    .bind(&body.genesis_hash)
    .bind(body.chain_id as i64)
    .bind(body.p2p_network_id as i64)
    .bind(&body.address_hrp)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await;
    if let Err(error) = insert {
        if error
            .as_database_error()
            .is_some_and(|db_error| db_error.is_unique_violation())
        {
            return mutation_error(
                &request_id.0,
                StatusCode::CONFLICT,
                "network_key_exists",
                "the Network key is already registered",
            );
        }
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let audit = serde_json::json!({
        "network_key": key,
        "display_name": body.display_name,
        "genesis_hash": body.genesis_hash,
        "chain_id": body.chain_id,
        "p2p_network_id": body.p2p_network_id,
        "address_hrp": body.address_hrp,
    });
    if crate::auth::insert_audit_event(
        &mut *tx,
        Some(&principal.0.user_id),
        "network_created",
        "network",
        &key,
        Some(&audit),
    )
    .await
    .is_err()
        || tx.commit().await.is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let revision = now.bytes().fold(0_u64, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as u64)
    });
    state
        .admin_realtime()
        .publish("network", Some(key.clone()), revision);
    state
        .public_realtime()
        .publish("network", Some(key.clone()), revision);
    Json(NetworkResponse {
        network_key: key,
        display_name: body.display_name,
    })
    .into_response()
}

/// Owner-only Registry update: display name and/or identity tuple fields.
/// Every field is optional, but at least one must change; the merged tuple
/// is validated and the before/after state is audited. Existing Nodes whose
/// observed identity now contradicts the tuple surface as typed mismatches;
/// no Node state is rewritten.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetworkUpdateRequest {
    pub display_name: Option<String>,
    pub genesis_hash: Option<String>,
    pub chain_id: Option<u64>,
    pub p2p_network_id: Option<u64>,
    pub address_hrp: Option<String>,
}

#[utoipa::path(
    put,
    path = "/api/admin/v1/networks/{network_key}",
    tag = "admin",
    params(("network_key" = String, Path, description = "Registered Network key")),
    request_body = NetworkUpdateRequest,
    responses((status = 200, body = NetworkResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn update_network(
    State(state): State<AppState>,
    Path(network_key): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: NetworkUpdateRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    let existing = sqlx::query_as::<_, (String, String, i64, i64, String)>(
        "SELECT display_name, genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks WHERE network_key = ?",
    )
    .bind(&network_key)
    .fetch_optional(state.db().pool())
    .await;
    let Some((current_name, current_genesis, current_chain, current_p2p, current_hrp)) =
        (match existing {
            Ok(row) => row,
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
        })
    else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    if body.display_name.is_none()
        && body.genesis_hash.is_none()
        && body.chain_id.is_none()
        && body.p2p_network_id.is_none()
        && body.address_hrp.is_none()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "empty_update",
            "at least one Network field must change",
        );
    }
    let next_name = body.display_name.clone().unwrap_or(current_name.clone());
    let next_genesis = body.genesis_hash.clone().unwrap_or(current_genesis.clone());
    let next_chain = body.chain_id.unwrap_or(current_chain as u64);
    let next_p2p = body.p2p_network_id.unwrap_or(current_p2p as u64);
    let next_hrp = body.address_hrp.clone().unwrap_or(current_hrp.clone());
    if crate::network::validate_network_tuple(
        &network_key,
        &next_name,
        &next_genesis,
        next_chain,
        next_p2p,
        &next_hrp,
    )
    .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_network_tuple",
            "the Network identity tuple is invalid",
        );
    }
    if next_name == current_name
        && next_genesis == current_genesis
        && next_chain as i64 == current_chain
        && next_p2p as i64 == current_p2p
        && next_hrp == current_hrp
    {
        return Json(NetworkResponse {
            network_key: network_key.clone(),
            display_name: next_name,
        })
        .into_response();
    }
    let changed_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    if sqlx::query(
        "UPDATE networks SET display_name = ?, genesis_hash = ?, chain_id = ?, p2p_network_id = ?, address_hrp = ?, updated_at = ? WHERE network_key = ?",
    )
    .bind(&next_name)
    .bind(&next_genesis)
    .bind(next_chain as i64)
    .bind(next_p2p as i64)
    .bind(&next_hrp)
    .bind(&changed_at)
    .bind(&network_key)
    .execute(&mut *tx)
    .await
    .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let before_audit = serde_json::json!({
        "display_name": current_name,
        "genesis_hash": current_genesis,
        "chain_id": current_chain,
        "p2p_network_id": current_p2p,
        "address_hrp": current_hrp,
    });
    let after_audit = serde_json::json!({
        "display_name": next_name,
        "genesis_hash": next_genesis,
        "chain_id": next_chain,
        "p2p_network_id": next_p2p,
        "address_hrp": next_hrp,
    });
    if crate::auth::insert_audit_change(
        &mut *tx,
        Some(&principal.0.user_id),
        "network_updated",
        "network",
        &network_key,
        Some(&before_audit),
        Some(&after_audit),
    )
    .await
    .is_err()
        || tx.commit().await.is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let revision = changed_at.bytes().fold(0_u64, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as u64)
    });
    state
        .admin_realtime()
        .publish("network", Some(network_key.clone()), revision);
    state
        .public_realtime()
        .publish("network", Some(network_key.clone()), revision);
    Json(NetworkResponse {
        network_key,
        display_name: next_name,
    })
    .into_response()
}

/// Owner-authorized two-phase Node Transfer (design §4.4, issue #46). The
/// typed status is Server-owned: `pending`, `completed`, `cancelled`,
/// `expired`, `rejected`, `conflict`, or `identity_mismatch`. A `pending`
/// row past its Server-authoritative `expires_at` is reported as `expired`
/// and never auto-extends.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NodeTransfer {
    pub transfer_id: String,
    pub node_id: String,
    pub source_agent_id: String,
    pub target_agent_id: String,
    pub status: String,
    pub operator_reason: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub cancelled_at: Option<String>,
    pub completed_at: Option<String>,
    pub rejection_code: Option<String>,
    pub rejection_reason: Option<String>,
    pub mismatched_fields: Vec<String>,
    pub updated_at: String,
}

/// Create a pending Transfer: Owner picks the target Agent, an expiry
/// (1..=168 hours, default 72), and an optional operator reason that is
/// recorded in Audit.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeTransferCreateRequest {
    pub target_agent_id: String,
    pub expires_in_hours: Option<i64>,
    pub operator_reason: Option<String>,
}

/// Mutation receipt for a Transfer create/cancel: the authoritative typed
/// Transfer plus the request and Audit references for the success view.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NodeTransferMutationResponse {
    pub transfer: NodeTransfer,
    pub request_id: String,
    pub audit_event_id: i64,
}

const TRANSFER_EXPIRY_MIN_HOURS: i64 = 1;
const TRANSFER_EXPIRY_MAX_HOURS: i64 = 168;
const TRANSFER_DEFAULT_EXPIRY_HOURS: i64 = 72;
const MAX_TRANSFER_REASON_LEN: usize = 512;

/// Effective Server-owned Transfer status: a `pending` row past its
/// deadline is `expired` — the Server never extends an expiry.
fn transfer_effective_status(status: &str, expires_at: &str) -> String {
    if status == "pending"
        && expires_at <= crate::auth::format_rfc3339(crate::auth::now_utc()).as_str()
    {
        "expired".to_owned()
    } else {
        status.to_owned()
    }
}

#[derive(Debug, sqlx::FromRow)]
struct NodeTransferRow {
    transfer_id: String,
    node_id: String,
    source_agent_id: String,
    target_agent_id: String,
    status: String,
    operator_reason: Option<String>,
    created_at: String,
    expires_at: String,
    cancelled_at: Option<String>,
    completed_at: Option<String>,
    rejection_code: Option<String>,
    rejection_reason: Option<String>,
    mismatched_fields: Option<String>,
    updated_at: String,
}

fn node_transfer_dto(row: NodeTransferRow) -> NodeTransfer {
    NodeTransfer {
        status: transfer_effective_status(&row.status, &row.expires_at),
        mismatched_fields: row
            .mismatched_fields
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
            .unwrap_or_default(),
        transfer_id: row.transfer_id,
        node_id: row.node_id,
        source_agent_id: row.source_agent_id,
        target_agent_id: row.target_agent_id,
        operator_reason: row.operator_reason,
        created_at: row.created_at,
        expires_at: row.expires_at,
        cancelled_at: row.cancelled_at,
        completed_at: row.completed_at,
        rejection_code: row.rejection_code,
        rejection_reason: row.rejection_reason,
        updated_at: row.updated_at,
    }
}

const TRANSFER_COLUMNS: &str = "transfer_id, node_id, source_agent_id, target_agent_id, status, operator_reason, created_at, expires_at, cancelled_at, completed_at, rejection_code, rejection_reason, mismatched_fields, updated_at";

/// Materialize expired pending Transfers of one Node (write + one Audit
/// event each) so the Admin timeline never shows a stale pending row.
async fn materialize_expired_node_transfers(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    node_id: &str,
    now_text: &str,
) -> Result<(), sqlx::Error> {
    let expired = sqlx::query_as::<_, (String,)>(
        "SELECT transfer_id FROM node_transfers WHERE node_id=? AND status='pending' AND expires_at <= ?",
    )
    .bind(node_id)
    .bind(now_text)
    .fetch_all(&mut **tx)
    .await?;
    for (transfer_id,) in expired {
        sqlx::query(
            "UPDATE node_transfers SET status='expired', updated_at=? WHERE transfer_id=? AND status='pending'",
        )
        .bind(now_text)
        .bind(&transfer_id)
        .execute(&mut **tx)
        .await?;
        let _ = crate::auth::insert_audit_event(
            &mut **tx,
            None,
            "node_transfer_expired",
            "node",
            node_id,
            Some(&serde_json::json!({
                "transfer_id": transfer_id,
                "expired_at": now_text,
            })),
        )
        .await;
    }
    Ok(())
}

/// Materialize one expired pending Transfer (used by cancel/detail).
async fn materialize_expired_transfer(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    transfer_id: &str,
    now_text: &str,
) -> Result<Option<String>, sqlx::Error> {
    let expired = sqlx::query(
        "UPDATE node_transfers SET status='expired', updated_at=? WHERE transfer_id=? AND status='pending' AND expires_at <= ?",
    )
    .bind(now_text)
    .bind(transfer_id)
    .bind(now_text)
    .execute(&mut **tx)
    .await?;
    if expired.rows_affected() == 0 {
        return Ok(None);
    }
    let node_id =
        sqlx::query_scalar::<_, String>("SELECT node_id FROM node_transfers WHERE transfer_id=?")
            .bind(transfer_id)
            .fetch_one(&mut **tx)
            .await?;
    let _ = crate::auth::insert_audit_event(
        &mut **tx,
        None,
        "node_transfer_expired",
        "node",
        &node_id,
        Some(&serde_json::json!({
            "transfer_id": transfer_id,
            "expired_at": now_text,
        })),
    )
    .await;
    Ok(Some(node_id))
}

/// Transfer history of one Node (newest first), with pending rows past
/// their deadline materialized as `expired`.
#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/transfers",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    responses((status = 200, description = "Owner-only Transfer history for one Node", body = [NodeTransfer]), (status = 404, body = crate::http::ApiErrorBody))
)]
async fn admin_node_transfers(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let known: Option<i64> = match sqlx::query_scalar("SELECT 1 FROM nodes WHERE node_id=?")
        .bind(&node_id)
        .fetch_optional(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    if known.is_none() {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    }
    let now_text = crate::auth::format_rfc3339(crate::auth::now_utc());
    if materialize_expired_node_transfers(&mut tx, &node_id, &now_text)
        .await
        .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let rows = sqlx::query_as::<_, NodeTransferRow>(&format!(
        "SELECT {TRANSFER_COLUMNS} FROM node_transfers WHERE node_id=? ORDER BY created_at DESC"
    ))
    .bind(&node_id)
    .fetch_all(&mut *tx)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    if tx.commit().await.is_err() {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    Json(rows.into_iter().map(node_transfer_dto).collect::<Vec<_>>()).into_response()
}

/// Create a pending two-phase Transfer. The source Agent stays
/// authoritative; ownership only switches after the target Agent declares
/// the Node ID with a validated Network Identity during ingestion. A
/// conflicting pending Transfer records a typed `conflict` outcome and
/// rejects the new request without touching ownership.
#[utoipa::path(
    post,
    path = "/api/admin/v1/nodes/{node_id}/transfers",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    request_body = NodeTransferCreateRequest,
    responses((status = 200, body = NodeTransferMutationResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn create_node_transfer(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: NodeTransferCreateRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    let expiry_hours = body
        .expires_in_hours
        .unwrap_or(TRANSFER_DEFAULT_EXPIRY_HOURS);
    if !(TRANSFER_EXPIRY_MIN_HOURS..=TRANSFER_EXPIRY_MAX_HOURS).contains(&expiry_hours) {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_expiry",
            "transfer expiry must be 1..=168 hours",
        );
    }
    if body.operator_reason.as_deref().is_some_and(|reason| {
        reason.is_empty()
            || reason.chars().count() > MAX_TRANSFER_REASON_LEN
            || reason.chars().any(|c| c.is_control())
    }) {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_operator_reason",
            "operator reason must be at most 512 characters without control characters",
        );
    }
    let now_text = crate::auth::format_rfc3339(crate::auth::now_utc());
    let expires_at =
        crate::auth::format_rfc3339(crate::auth::now_utc() + time::Duration::hours(expiry_hours));
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let owner: Option<String> =
        match sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=?")
            .bind(&node_id)
            .fetch_optional(&mut *tx)
            .await
        {
            Ok(value) => value,
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
        };
    let Some(source_agent_id) = owner else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    if body.target_agent_id == source_agent_id {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_target_agent",
            "the target Agent already owns this Node",
        );
    }
    let target_known: Option<i64> =
        match sqlx::query_scalar("SELECT 1 FROM agents WHERE agent_id=?")
            .bind(&body.target_agent_id)
            .fetch_optional(&mut *tx)
            .await
        {
            Ok(value) => value,
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
        };
    if target_known.is_none() {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_target_agent",
            "the target Agent is not registered",
        );
    }
    if materialize_expired_node_transfers(&mut tx, &node_id, &now_text)
        .await
        .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let pending: Option<String> = match sqlx::query_scalar(
        "SELECT transfer_id FROM node_transfers WHERE node_id=? AND status='pending'",
    )
    .bind(&node_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    if pending.is_some() {
        // The conflict is a typed, auditable outcome: the attempt is
        // retained in history, ownership is untouched, and the response is
        // the standard error envelope (request ID included).
        let transfer_id = uuid::Uuid::new_v4().to_string();
        if sqlx::query("INSERT INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, operator_reason, created_at, expires_at, updated_at) VALUES (?, ?, ?, ?, 'conflict', ?, ?, ?, ?)")
            .bind(&transfer_id)
            .bind(&node_id)
            .bind(&source_agent_id)
            .bind(&body.target_agent_id)
            .bind(&body.operator_reason)
            .bind(&now_text)
            .bind(&expires_at)
            .bind(&now_text)
            .execute(&mut *tx)
            .await
            .is_err()
            || crate::auth::insert_audit_event(
                &mut *tx,
                Some(&principal.0.user_id),
                "node_transfer_conflict",
                "node",
                &node_id,
                Some(&serde_json::json!({
                    "transfer_id": transfer_id,
                    "pending_transfer_id": pending,
                    "target_agent_id": body.target_agent_id,
                })),
            )
            .await
            .is_err()
        {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
        let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
            .fetch_one(&mut *tx)
            .await
        {
            Ok(value) => value,
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
        };
        if tx.commit().await.is_err() {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
        // The conflict is a typed, auditable outcome: the response carries
        // the request and Audit references so the operator can look the
        // refusal up (issue #46 acceptance: errors expose references, never
        // sensitive details).
        return (
            StatusCode::CONFLICT,
            Json(crate::http::ApiErrorBody::with_fields(
                "transfer_conflict",
                "a transfer for this Node is already pending",
                &request_id.0,
                vec![format!("audit_event_id:{audit_event_id}")],
            )),
        )
            .into_response();
    }
    let transfer_id = uuid::Uuid::new_v4().to_string();
    if sqlx::query("INSERT INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, operator_reason, created_at, expires_at, updated_at) VALUES (?, ?, ?, ?, 'pending', ?, ?, ?, ?)")
        .bind(&transfer_id)
        .bind(&node_id)
        .bind(&source_agent_id)
        .bind(&body.target_agent_id)
        .bind(&body.operator_reason)
        .bind(&now_text)
        .bind(&expires_at)
        .bind(&now_text)
        .execute(&mut *tx)
        .await
        .is_err()
        || crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            "node_transfer_created",
            "node",
            &node_id,
            Some(&serde_json::json!({
                "transfer_id": transfer_id,
                "source_agent_id": source_agent_id,
                "target_agent_id": body.target_agent_id,
                "expires_at": expires_at,
                "operator_reason": body.operator_reason,
            })),
        )
        .await
        .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let row = match sqlx::query_as::<_, NodeTransferRow>(&format!(
        "SELECT {TRANSFER_COLUMNS} FROM node_transfers WHERE transfer_id=?"
    ))
    .bind(&transfer_id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(row) => row,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    if tx.commit().await.is_err() {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let revision = now_text.bytes().fold(0_u64, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as u64)
    });
    state
        .admin_realtime()
        .publish("node", Some(node_id), revision);
    Json(NodeTransferMutationResponse {
        transfer: node_transfer_dto(row),
        request_id: request_id.0.to_string(),
        audit_event_id,
    })
    .into_response()
}

/// One Transfer by id, with Server-owned effective status.
#[utoipa::path(
    get,
    path = "/api/admin/v1/transfers/{transfer_id}",
    tag = "admin",
    params(("transfer_id" = String, Path, description = "Transfer ID")),
    responses((status = 200, body = NodeTransfer), (status = 404, body = crate::http::ApiErrorBody))
)]
async fn admin_transfer_detail(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let now_text = crate::auth::format_rfc3339(crate::auth::now_utc());
    if materialize_expired_transfer(&mut tx, &transfer_id, &now_text)
        .await
        .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let row = sqlx::query_as::<_, NodeTransferRow>(&format!(
        "SELECT {TRANSFER_COLUMNS} FROM node_transfers WHERE transfer_id=?"
    ))
    .bind(&transfer_id)
    .fetch_optional(&mut *tx)
    .await;
    let row = match row {
        Ok(row) => row,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let Some(row) = row else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    if tx.commit().await.is_err() {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    Json(node_transfer_dto(row)).into_response()
}

/// Cancel a pending Transfer. Only `pending` can be cancelled; ownership
/// never changes and the outcome is typed and audited.
#[utoipa::path(
    post,
    path = "/api/admin/v1/transfers/{transfer_id}/cancel",
    tag = "admin",
    params(("transfer_id" = String, Path, description = "Transfer ID")),
    responses((status = 200, body = NodeTransferMutationResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn cancel_node_transfer(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, false) {
        return response;
    }
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let now_text = crate::auth::format_rfc3339(crate::auth::now_utc());
    if materialize_expired_transfer(&mut tx, &transfer_id, &now_text)
        .await
        .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let row = sqlx::query_as::<_, NodeTransferRow>(&format!(
        "SELECT {TRANSFER_COLUMNS} FROM node_transfers WHERE transfer_id=?"
    ))
    .bind(&transfer_id)
    .fetch_optional(&mut *tx)
    .await;
    let row = match row {
        Ok(row) => row,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let Some(row) = row else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    if row.status != "pending" {
        return mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "transfer_not_pending",
            "only a pending transfer can be cancelled",
        );
    }
    let cancelled_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    if sqlx::query("UPDATE node_transfers SET status='cancelled', cancelled_at=?, updated_at=? WHERE transfer_id=? AND status='pending'")
        .bind(&cancelled_at)
        .bind(&cancelled_at)
        .bind(&transfer_id)
        .execute(&mut *tx)
        .await
        .is_err()
        || crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            "node_transfer_cancelled",
            "node",
            &row.node_id,
            Some(&serde_json::json!({
                "transfer_id": transfer_id,
                "source_agent_id": row.source_agent_id,
                "target_agent_id": row.target_agent_id,
            })),
        )
        .await
        .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let updated = NodeTransferRow {
        status: "cancelled".to_owned(),
        cancelled_at: Some(cancelled_at.clone()),
        updated_at: cancelled_at,
        ..row
    };
    if tx.commit().await.is_err() {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    state
        .admin_realtime()
        .publish("node", Some(updated.node_id.clone()), 0);
    Json(NodeTransferMutationResponse {
        transfer: node_transfer_dto(updated),
        request_id: request_id.0.to_string(),
        audit_event_id,
    })
    .into_response()
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct GeoStatusDiagnostic {
    pub state: String,
    pub configured: bool,
    pub build_epoch: Option<u64>,
    pub digest: Option<String>,
    pub loaded_at: Option<String>,
    pub last_error: Option<String>,
    pub cache_country_count: i64,
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/geo",
    tag = "admin",
    responses((status = 200, description = "Owner-only safe Geo database status", body = GeoStatusDiagnostic))
)]
pub(crate) async fn admin_geo_status(
    State(state): State<AppState>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Extension(request_id): Extension<super::RequestId>,
) -> Response {
    let status = state.geo().status();
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let rebuild_before = crate::geo::cache_rebuild_cutoff(&now);
    let cache_country_count = match sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(DISTINCT country_code) FROM geo_location_cache WHERE expires_at > ? AND created_at > ?",
    )
    .bind(now)
    .bind(rebuild_before)
    .fetch_one(state.db().pool())
    .await
    {
        Ok(count) => count,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    Json(GeoStatusDiagnostic {
        state: status.state,
        configured: status.configured,
        build_epoch: status.build_epoch,
        digest: status.digest,
        loaded_at: status.loaded_at,
        last_error: status.last_error,
        cache_country_count,
    })
    .into_response()
}

pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/events", get(admin_events))
        .route("/overview", get(overview))
        .route("/geo", get(admin_geo_status))
        .route("/nodes", get(admin_nodes))
        .route("/nodes/{node_id}", get(admin_node_detail))
        .route("/nodes/{node_id}/metadata", put(set_node_metadata))
        .route("/nodes/{node_id}/peer-churn", get(admin_node_peer_churn))
        .route(
            "/nodes/{node_id}/peer-history",
            get(admin_node_peer_history),
        )
        .route("/nodes/{node_id}/history", get(admin_node_history))
        .route("/networks", get(admin_networks))
        .route("/networks/{network_key}", get(admin_network_detail))
        .route("/networks", post(create_network))
        .route("/networks/{network_key}", put(update_network))
        .route("/agents", get(diagnostics))
        .route("/agents/{agent_id}", get(admin_agent_detail))
        .route("/agents/{agent_id}/audit", get(admin_agent_audit))
        .route("/agents/enroll-token", post(admin_enrollment_token))
        .route("/agents/{agent_id}/recover", post(admin_recovery_token))
        .route(
            "/agents/{agent_id}/credentials/rotate",
            post(admin_rotate_credential),
        )
        .route(
            "/agents/{agent_id}/credentials/{credential_id}/revoke",
            post(admin_revoke_credential),
        )
        .route("/nodes/{node_id}/visibility", put(set_visibility))
        .route("/nodes/{node_id}/transfers", get(admin_node_transfers))
        .route("/nodes/{node_id}/transfers", post(create_node_transfer))
        .route("/transfers/{transfer_id}", get(admin_transfer_detail))
        .route(
            "/transfers/{transfer_id}/cancel",
            post(cancel_node_transfer),
        )
        .fallback(api_not_found)
        .layer(axum::middleware::from_fn(group_middleware))
}

async fn group_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        ROUTE_GROUP_HEADER,
        axum::http::HeaderValue::from_static("admin"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::AuthenticatedSession;
    use axum::body::to_bytes;
    use serde_json::Value;
    use tempfile::tempdir;
    use time::OffsetDateTime;

    #[tokio::test]
    async fn admin_geo_status_exposes_safe_metadata_only() {
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
        let state = AppState::new(database, None, auth);
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        let expires_at = crate::geo::cache_expiry(&now);
        sqlx::query("INSERT INTO geo_location_cache (canonical_ip, country_code, created_at, last_lookup_at, last_referenced_at, expires_at) VALUES ('8.8.8.8', 'US', ?, ?, ?, ?)")
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(expires_at)
            .execute(state.db().pool())
            .await
            .unwrap();
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        let response = admin_geo_status(
            State(state),
            Extension(AuthenticatedSession(session)),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["state"], "disabled");
        assert_eq!(value["configured"], false);
        assert_eq!(value["cache_country_count"], 1);
        assert!(!text.contains("8.8.8.8"));
        assert!(!text.contains("server.db"));
    }
    #[tokio::test]
    async fn visibility_mutation_accepts_json_parameters_and_updates_node() {
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
        let state = AppState::new(database, None, auth);
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-visibility-test', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('node-visibility-test', 'agent-visibility-test', 'mainnet', 'ws://127.0.0.1:1', 'active', 'private', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        headers.insert(header::ORIGIN, "http://127.0.0.1:8080".parse().unwrap());
        headers.insert("x-csrf-token", "csrf".parse().unwrap());
        let response = set_visibility(
            State(state.clone()),
            Path("node-visibility-test".to_owned()),
            headers,
            Extension(AuthenticatedSession(session)),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
            axum::body::Bytes::from_static(br#"{"visibility":"public"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let visibility: String = sqlx::query_scalar(
            "SELECT visibility FROM nodes WHERE node_id = 'node-visibility-test'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(visibility, "public");
        let (before, after, actor): (String, String, String) = sqlx::query_as(
            "SELECT before_json, after_json, actor_user_id FROM audit_events WHERE event_kind = 'node_visibility_changed'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(before.contains("private"));
        assert!(after.contains("public"));
        assert_eq!(actor, "owner");
    }

    #[tokio::test]
    async fn visibility_mutation_rejects_security_failures_with_uniform_envelope_before_json() {
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
        let state = AppState::new(database, None, auth);
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        for (content_type, origin, csrf) in [
            ("text/plain", "http://127.0.0.1:8080", "csrf"),
            ("application/json", "https://evil.example", "csrf"),
            ("application/json", "http://127.0.0.1:8080", "wrong"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
            headers.insert(header::ORIGIN, origin.parse().unwrap());
            headers.insert("x-csrf-token", csrf.parse().unwrap());
            let response = set_visibility(
                State(state.clone()),
                Path("missing".to_owned()),
                headers,
                Extension(AuthenticatedSession(session.clone())),
                Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
                axum::body::Bytes::from_static(b"not json"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["error"]["code"], "csrf_validation_failed");
            assert_eq!(value["error"]["fields"], serde_json::json!([]));
        }
    }

    #[test]
    fn stale_sync_or_consensus_keeps_admin_health_unknown() {
        let now = crate::auth::now_utc();
        let recent = crate::auth::format_rfc3339(now);
        let stale = crate::auth::format_rfc3339(now - time::Duration::hours(1));
        let rpc = RpcDiagnostic {
            client_version: Some("platon/1".to_owned()),
            namespaces: vec![],
            methods: vec![],
            state: Some("ok".to_owned()),
            error_code: None,
            error_message: None,
            attempted_at: Some(recent.clone()),
            observed_at: Some(recent.clone()),
            received_at: Some(recent.clone()),
            state_revision: Some(1),
            value_revision: Some(1),
        };
        let sync = SyncDiagnostic {
            state: "ok".to_owned(),
            error_code: None,
            error_message: None,
            attempted_at: Some(stale.clone()),
            observed_at: Some(stale.clone()),
            received_at: Some(stale.clone()),
            state_revision: 1,
            value_revision: 1,
            syncing: Some(false),
            current_block: Some(10),
            highest_block: Some(10),
            pulled_states: Some(1),
            known_states: Some(1),
        };
        let consensus = ConsensusDiagnostic {
            state: "ok".to_owned(),
            error_code: None,
            error_message: None,
            attempted_at: Some(recent.clone()),
            observed_at: Some(recent.clone()),
            received_at: Some(recent.clone()),
            state_revision: 1,
            value_revision: 1,
            epoch: Some(1),
            view_number: Some(1),
            validator: Some(true),
            highest_qc_block: Some(10),
            highest_lock_block: Some(10),
            highest_commit_block: Some(10),
        };
        assert_eq!(
            derive_health("active", Some(&rpc), Some(&sync), Some(&consensus)),
            ("unknown", "one or more observations are stale or unknown")
        );
        let mut stale_consensus = consensus;
        stale_consensus.received_at = Some(stale);
        assert_eq!(
            derive_health("active", Some(&rpc), Some(&sync), Some(&stale_consensus)),
            ("unknown", "one or more observations are stale or unknown")
        );
    }
    #[tokio::test]
    async fn host_diagnostics_include_failure_and_freshness_fields() {
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
        let state = AppState::new(database, None, auth);
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES (?, 1, ?, ?)")
            .bind("agent-host-test")
            .bind("2026-01-01T00:00:00Z")
            .bind("2026-01-01T00:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_host_observations (agent_id, updated_at) VALUES (?, ?)")
            .bind("agent-host-test")
            .bind("2026-01-01T00:00:02Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision, error_code, error_message) VALUES (?, 'host', 'host', 'cpu_percent', 'error', ?, ?, ?, 2, 1, ?, ?)")
            .bind("agent-host-test")
            .bind("2026-01-01T00:00:01Z")
            .bind("2025-12-31T23:59:00Z")
            .bind("2026-01-01T00:00:02Z")
            .bind("rpc_unreachable")
            .bind("RPC probe failed")
            .execute(state.db().pool())
            .await
            .unwrap();
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        let response = diagnostics(State(state), Extension(AuthenticatedSession(session)))
            .await
            .into_response();
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let component = &value[0]["host"]["components"][0];
        assert_eq!(component["state"], "error");
        assert_eq!(component["error_code"], "rpc_unreachable");
        assert_eq!(component["attempted_at"], "2026-01-01T00:00:01Z");
        assert_eq!(component["observed_at"], "2025-12-31T23:59:00Z");
        assert_eq!(component["received_at"], "2026-01-01T00:00:02Z");
        assert_eq!(component["state_revision"], 2);
        assert_eq!(component["value_revision"], 1);
    }

    #[test]
    fn derive_freshness_ranks_retained_observations() {
        let now = crate::auth::now_utc();
        let recent = crate::auth::format_rfc3339(now - time::Duration::seconds(30));
        let stale = crate::auth::format_rfc3339(now - time::Duration::hours(3));
        assert_eq!(
            derive_freshness([recent.as_str(), stale.as_str()].into_iter()),
            "current"
        );
        assert_eq!(derive_freshness([stale.as_str()].into_iter()), "stale");
        assert_eq!(derive_freshness([].into_iter()), "unknown");
        // Future-skewed clocks are never shown as current.
        let future = crate::auth::format_rfc3339(now + time::Duration::hours(1));
        assert_eq!(derive_freshness([future.as_str()].into_iter()), "stale");
    }

    #[tokio::test]
    async fn overview_reports_server_owned_attention_and_summary() {
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
        let state = AppState::new(database, None, auth);
        let now = crate::auth::now_utc();
        let fresh = crate::auth::format_rfc3339(now - time::Duration::seconds(20));
        let created = "2026-01-01T00:00:00Z";
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at, last_received_at, shutdown_state, security_event_count) VALUES ('agent-ov', 1, ?, ?, ?, 'running', 0)")
            .bind(created)
            .bind(created)
            .bind(&fresh)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        for (node_id, name, visibility, rpc_state) in [
            ("node-a", "Node A", "public", "ok"),
            ("node-b", "Node B", "private", "error"),
        ] {
            sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, 'agent-ov', 'mainnet', ?, 'ws://127.0.0.1:1', 'active', ?, 1, ?, ?)")
                .bind(node_id)
                .bind(name)
                .bind(visibility)
                .bind(created)
                .bind(created)
                .execute(state.db().pool())
                .await
                .unwrap();
            sqlx::query("INSERT INTO current_node_chain_observations (node_id, current_block, syncing, updated_at) VALUES (?, 100, 0, ?)")
                .bind(node_id)
                .bind(&fresh)
                .execute(state.db().pool())
                .await
                .unwrap();
            for component in ["rpc", "sync", "consensus"] {
                let component_state = if component == "rpc" { rpc_state } else { "ok" };
                sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision) VALUES ('agent-ov', 'node', ?, ?, ?, ?, ?, ?, ?, 1, 1)")
                    .bind(node_id)
                    .bind(node_id)
                    .bind(component)
                    .bind(component_state)
                    .bind(&fresh)
                    .bind(&fresh)
                    .bind(&fresh)
                    .execute(state.db().pool())
                    .await
                    .unwrap();
            }
        }
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        let response = overview(
            State(state),
            Extension(AuthenticatedSession(session)),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["summary"]["nodes"]["total"], 2);
        assert_eq!(value["summary"]["nodes"]["healthy"], 1);
        assert_eq!(value["summary"]["nodes"]["unhealthy"], 1);
        assert_eq!(value["summary"]["nodes"]["unknown"], 0);
        assert_eq!(value["summary"]["nodes"]["retired"], 0);
        assert_eq!(value["summary"]["nodes"]["published"], 1);
        assert_eq!(value["summary"]["agents"]["total"], 1);
        assert_eq!(value["summary"]["agents"]["online"], 1);
        assert_eq!(value["summary"]["agents"]["offline"], 0);
        let attention = value["attention"].as_array().unwrap();
        assert_eq!(attention.len(), 1);
        assert_eq!(attention[0]["kind"], "node_unhealthy");
        assert_eq!(attention[0]["severity"], "critical");
        assert_eq!(attention[0]["subject_kind"], "node");
        assert_eq!(attention[0]["subject_id"], "node-b");
        assert_eq!(attention[0]["subject_label"], "Node B");
        assert_eq!(attention[0]["message"], "RPC collection failed");
    }

    #[tokio::test]
    async fn overview_flags_offline_agents_spool_loss_and_unknown_nodes() {
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
        let state = AppState::new(database, None, auth);
        let now = crate::auth::now_utc();
        let stale = crate::auth::format_rfc3339(now - time::Duration::hours(1));
        let created = "2026-01-01T00:00:00Z";
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at, last_received_at, shutdown_state, security_event_count) VALUES ('agent-ov2', 1, ?, ?, ?, 'running', 2)")
            .bind(created)
            .bind(created)
            .bind(&stale)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_host_observations (agent_id, spool_store_fatal, spool_dropped_sequence_to, updated_at) VALUES ('agent-ov2', 1, 5, ?)")
            .bind(&stale)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('node-unknown', 'agent-ov2', 'mainnet', 'Node Unknown', 'ws://127.0.0.1:2', 'active', 'private', 1, ?, ?)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        let response = overview(
            State(state),
            Extension(AuthenticatedSession(session)),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["summary"]["agents"]["total"], 1);
        assert_eq!(value["summary"]["agents"]["online"], 0);
        assert_eq!(value["summary"]["agents"]["offline"], 1);
        assert_eq!(value["summary"]["nodes"]["total"], 1);
        assert_eq!(value["summary"]["nodes"]["unknown"], 1);
        assert_eq!(value["summary"]["nodes"]["retired"], 0);
        let kinds = value["attention"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    item["kind"].as_str().unwrap().to_owned(),
                    item["severity"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        assert!(kinds.contains(&("agent_security_event".to_owned(), "critical".to_owned())));
        assert!(kinds.contains(&("agent_spool_fatal".to_owned(), "critical".to_owned())));
        assert!(kinds.contains(&("agent_offline".to_owned(), "warning".to_owned())));
        assert!(kinds.contains(&("agent_spool_overflow".to_owned(), "warning".to_owned())));
        assert!(kinds.contains(&("node_health_unknown".to_owned(), "warning".to_owned())));
        // Critical items sort before warnings.
        assert_eq!(value["attention"][0]["severity"], "critical");
    }

    #[tokio::test]
    async fn overview_counts_retired_nodes_outside_health_buckets() {
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
        let state = AppState::new(database, None, auth);
        let created = "2026-01-01T00:00:00Z";
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-retired', 1, ?, ?)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('node-retired', 'agent-retired', 'mainnet', 'Node Retired', 'ws://127.0.0.1:3', 'retired', 'private', 1, ?, ?)")
            .bind(created)
            .bind(created)
            .execute(state.db().pool())
            .await
            .unwrap();
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        let response = overview(
            State(state),
            Extension(AuthenticatedSession(session)),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
        )
        .await;
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["summary"]["nodes"]["total"], 1);
        assert_eq!(value["summary"]["nodes"]["retired"], 1);
        assert_eq!(value["summary"]["nodes"]["healthy"], 0);
        assert_eq!(value["summary"]["nodes"]["unhealthy"], 0);
        assert_eq!(value["summary"]["nodes"]["unknown"], 0);
        // Retired Nodes never enter the attention queue.
        assert_eq!(value["attention"].as_array().unwrap().len(), 0);
    }

    /// Test state with an Owner user and one enrolled Agent row.
    async fn lifecycle_state() -> (tempfile::TempDir, AppState) {
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
        let state = AppState::new(database, None, auth);
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-lifecycle-test', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        // One already-issued credential, like a real enrolled Agent.
        sqlx::query("INSERT INTO agent_credentials (credential_id, agent_id, credential_digest, created_at, revoked_at, revoke_after) VALUES ('credential-initial', 'agent-lifecycle-test', x'00', '2026-01-01T00:00:00Z', NULL, NULL)")
            .execute(state.db().pool()).await.unwrap();
        (dir, state)
    }

    fn lifecycle_session() -> AuthenticatedSession {
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

    fn mutation_headers(csrf: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        headers.insert(header::ORIGIN, "http://127.0.0.1:8080".parse().unwrap());
        headers.insert("x-csrf-token", csrf.parse().unwrap());
        headers
    }

    fn request_id() -> crate::http::RequestId {
        crate::http::RequestId(std::sync::Arc::from("req-123"))
    }

    #[tokio::test]
    async fn admin_lifecycle_mutations_write_redacted_audit_and_validate() {
        let (_dir, state) = lifecycle_state().await;
        let session = lifecycle_session();
        let extension = Extension(session);

        // Enrollment token: the one-time secret appears only in the success
        // response and never in the Audit body.
        let response = admin_enrollment_token(
            State(state.clone()),
            mutation_headers("csrf"),
            extension,
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"expiresInHours": 24}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let enroll_token = value["token"].as_str().unwrap().to_owned();
        assert!(enroll_token.starts_with(crate::enrollment::ENROLLMENT_TOKEN_PREFIX));
        assert_eq!(value["lifetime_hours"], 24);
        assert_eq!(value["request_id"], "req-123");
        let audit_body: Option<String> = sqlx::query_scalar(
            "SELECT after_json FROM audit_events WHERE event_kind = 'enrollment_token_created'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(
            audit_body.as_deref().is_none_or(|body| {
                !body.contains(&enroll_token)
                    && !body.contains(crate::enrollment::ENROLLMENT_TOKEN_PREFIX)
            }),
            "enrollment audit must be redacted"
        );

        // CSRF, lifetime, and unknown-agent outcomes are typed.
        let response = admin_enrollment_token(
            State(state.clone()),
            mutation_headers("wrong-csrf"),
            Extension(lifecycle_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"expiresInHours": 24}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = admin_enrollment_token(
            State(state.clone()),
            mutation_headers("csrf"),
            Extension(lifecycle_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"expiresInHours": 0}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Recovery token for the existing Agent.
        let response = admin_recovery_token(
            State(state.clone()),
            Path("agent-lifecycle-test".to_owned()),
            mutation_headers("csrf"),
            Extension(lifecycle_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"expiresInHours": 12}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let recover_token = value["token"].as_str().unwrap().to_owned();
        assert!(recover_token.starts_with(crate::enrollment::RECOVERY_TOKEN_PREFIX));
        assert_eq!(value["agent_epoch"], 1);
        let response = admin_recovery_token(
            State(state.clone()),
            Path("no-such-agent".to_owned()),
            mutation_headers("csrf"),
            Extension(lifecycle_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"expiresInHours": 12}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // Rotation with overlap: fresh credential shown once; the previous
        // credential remains valid through revoke_after.
        let response = admin_rotate_credential(
            State(state.clone()),
            Path("agent-lifecycle-test".to_owned()),
            mutation_headers("csrf"),
            Extension(lifecycle_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"overlapHours": 24, "revokePrevious": false}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let rotated_secret = value["credential"].as_str().unwrap().to_owned();
        assert!(rotated_secret.starts_with(crate::enrollment::AGENT_CREDENTIAL_PREFIX));
        assert_eq!(value["overlap_hours"], 24);
        assert!(value["revoke_after"].is_string());
        assert_eq!(value["overlap_credential_ids"].as_array().unwrap().len(), 1);
        assert_eq!(value["revoked_previous_ids"].as_array().unwrap().len(), 0);

        // Rotation with explicit old-credential revocation.
        let response = admin_rotate_credential(
            State(state.clone()),
            Path("agent-lifecycle-test".to_owned()),
            mutation_headers("csrf"),
            Extension(lifecycle_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"overlapHours": 24, "revokePrevious": true}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["revoked_previous_ids"].as_array().unwrap().len(), 2);
        assert!(value["revoke_after"].is_null());

        // The detail view exposes the credential dimension (ids and
        // instants only, never secrets).
        let response = admin_agent_detail(
            State(state.clone()),
            Path("agent-lifecycle-test".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let credentials = value["credentials"].as_array().unwrap();
        assert_eq!(credentials.len(), 3);
        assert!(
            credentials
                .iter()
                .all(|c| c["credential_id"].is_string() && c["active"].is_boolean())
        );
        let active = credentials.iter().filter(|c| c["active"] == true).count();
        assert_eq!(active, 1, "one credential stays active after rotation");
        let response = admin_agent_detail(
            State(state.clone()),
            Path("no-such-agent".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // Explicit revocation of the last active credential.
        let credential_id =
            credentials.iter().find(|c| c["active"] == true).unwrap()["credential_id"]
                .as_str()
                .unwrap()
                .to_owned();
        let response = admin_revoke_credential(
            State(state.clone()),
            Path(("agent-lifecycle-test".to_owned(), credential_id.clone())),
            mutation_headers("csrf"),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = admin_revoke_credential(
            State(state.clone()),
            Path(("agent-lifecycle-test".to_owned(), credential_id)),
            mutation_headers("csrf"),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = admin_revoke_credential(
            State(state.clone()),
            Path((
                "agent-lifecycle-test".to_owned(),
                "unknown-credential".to_owned(),
            )),
            mutation_headers("csrf"),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // The redacted Audit trail lists every security mutation with the
        // Owner actor and never contains secret material.
        let response = admin_agent_audit(
            State(state.clone()),
            Path("agent-lifecycle-test".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let items = value["items"].as_array().unwrap();
        let kinds: Vec<&str> = items
            .iter()
            .map(|item| item["event_kind"].as_str().unwrap())
            .collect();
        for expected in [
            "recovery_token_created",
            "agent_credential_rotated",
            "agent_credential_revoked",
        ] {
            assert!(kinds.contains(&expected), "missing audit kind {expected}");
        }
        let serialized = value.to_string();
        for secret in [&enroll_token, &recover_token, &rotated_secret] {
            assert!(
                !serialized.contains(secret.as_str()),
                "audit trail must never contain one-time secrets"
            );
        }
        assert!(
            items.iter().all(|item| {
                item["actor_username"].as_str().unwrap_or("") == "owner"
                    && item["details"].is_object()
            }),
            "every lifecycle audit row names the Owner actor with redacted details"
        );
    }
    // ---- Node inventory, metadata, and Network Registry (issue #45) ----

    /// Seed helper: one registered Network plus two Nodes on one Agent.
    /// `node_healthy` observes the registered identity; `node_mismatched`
    /// observes a contradicting chain id (typed mismatch, history blocked).
    async fn node_inventory_state() -> (tempfile::TempDir, AppState) {
        let (dir, state) = lifecycle_state().await;
        let now = crate::auth::now_utc();
        let fixed = "2026-01-01T00:00:00Z".to_owned();
        let fresh = crate::auth::format_rfc3339(now);
        sqlx::query(
            "INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'PlatON Mainnet', '0x' || '01' || '00000000000000000000000000000000000000000000000000000000000000', 210425, 1, 'lat', ?, ?)",
        )
        .bind(&fixed)
        .bind(&fixed)
        .execute(state.db().pool())
        .await
        .unwrap();
        for (node_id, lifecycle) in [
            ("node-healthy", "active"),
            ("node-mismatched", "active"),
            ("node-retired", "retired"),
        ] {
            sqlx::query(
                "INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, 'agent-lifecycle-test', 'mainnet', ?, 'ws://127.0.0.1:6790', ?, 'private', 1, ?, ?)",
            )
            .bind(node_id)
            .bind(node_id)
            .bind(lifecycle)
            .bind(&fixed)
            .bind(&fixed)
            .execute(state.db().pool())
            .await
            .unwrap();
        }
        // Healthy Node: matching observed identity + fresh ok components.
        sqlx::query(
            "INSERT INTO current_node_chain_observations (node_id, rpc_client_version, syncing, current_block, highest_block, consensus_epoch, consensus_validator, consensus_highest_commit_block, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, node_key_fingerprint, updated_at) VALUES ('node-healthy', 'platon/1.5.1', 0, 100, 100, 1, 1, 100, '0x' || '01' || '00000000000000000000000000000000000000000000000000000000000000', 210425, 1, 'lat', 'fp-healthy', ?)",
        )
        .bind(&fresh)
        .execute(state.db().pool())
        .await
        .unwrap();
        for component in ["rpc", "sync", "consensus"] {
            sqlx::query(
                "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision) VALUES ('agent-lifecycle-test', 'node', 'node-healthy', 'node-healthy', ?, 'ok', ?, ?, ?, 1, 1)",
            )
            .bind(component)
            .bind(&fresh)
            .bind(&fresh)
            .bind(&fresh)
            .execute(state.db().pool())
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, value_received_at, state_revision, value_revision) VALUES ('agent-lifecycle-test', 'node', 'node-healthy', 'node-healthy', 'peers', 'ok', ?, ?, ?, ?, 1, 1)",
        )
        .bind(&fresh)
        .bind(&fresh)
        .bind(&fresh)
        .bind(&fresh)
        .execute(state.db().pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, client_name, cbft_protocol_version, cbft_highest_qc_block, cbft_locked_block, cbft_commit_block, updated_at) VALUES ('node-healthy', 'peer-healthy', '203.0.113.7', 'inbound', 1, 0, 1, 'PlatON/v1.5.1', 1, 100, 99, 98, ?)",
        )
        .bind(&fresh)
        .execute(state.db().pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO current_node_peer_capabilities (node_id, peer_id, capability, updated_at) VALUES ('node-healthy', 'peer-healthy', 'cbft/1', ?)",
        )
        .bind(&fresh)
        .execute(state.db().pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO peer_presence_intervals (node_id, peer_id, opened_at, closed_at, direction, trusted, static_peer, consensus_peer, client_name) VALUES ('node-healthy', 'peer-healthy', ?, ?, 'inbound', 1, 0, 1, 'PlatON/v1.5.1')",
        )
        .bind(&fixed)
        .bind(&fresh)
        .execute(state.db().pool())
        .await
        .unwrap();
        // Mismatched Node: RPC error with a last-good sync value retained
        sqlx::query(
            "INSERT INTO current_node_chain_observations (node_id, rpc_client_version, syncing, current_block, highest_block, consensus_epoch, consensus_validator, consensus_highest_commit_block, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, updated_at) VALUES ('node-mismatched', 'platon/1.5.1', 0, 99, 99, 1, 0, 99, '0x' || '01' || '00000000000000000000000000000000000000000000000000000000000000', 999999, 1, 'lat', ?)",
        )
        .bind(&fresh)
        .execute(state.db().pool())
        .await
        .unwrap();
        for component in ["rpc", "sync", "consensus"] {
            let (component_state, error_code, error_message) = if component == "rpc" {
                ("error", "rpc_unreachable", "RPC probe failed")
            } else {
                ("ok", "", "")
            };
            sqlx::query(
                "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision, error_code, error_message) VALUES ('agent-lifecycle-test', 'node', 'node-mismatched', 'node-mismatched', ?, ?, ?, ?, ?, 2, 1, ?, ?)",
            )
            .bind(component)
            .bind(component_state)
            .bind(&fresh)
            .bind(&fresh)
            .bind(&fresh)
            .bind(error_code)
            .bind(error_message)
            .execute(state.db().pool())
            .await
            .unwrap();
        }
        (dir, state)
    }

    #[tokio::test]
    async fn admin_node_inventory_is_per_node_with_server_owned_dimensions() {
        let (_dir, state) = node_inventory_state().await;
        let response = admin_nodes(
            State(state.clone()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let items = value.as_array().unwrap();
        assert_eq!(items.len(), 3, "every Node is its own inventory row");
        let by_id = |id: &str| {
            items
                .iter()
                .find(|item| item["node_id"] == id)
                .unwrap_or_else(|| panic!("missing Node {id}"))
                .clone()
        };
        // Server-owned metadata is present and distinct from the
        // Agent-observed identity disposition.
        let healthy = by_id("node-healthy");
        assert_eq!(healthy["display_name"], "node-healthy");
        assert_eq!(healthy["network_display_name"], "PlatON Mainnet");
        assert_eq!(healthy["lifecycle"], "active");
        assert_eq!(healthy["visibility"], "private");
        assert_eq!(healthy["health"], "healthy");
        assert_eq!(healthy["freshness"], "current");
        assert_eq!(healthy["identity"]["state"], "matched");
        assert_eq!(
            healthy["identity"]["observed"]["chain_id"], 210425,
            "the observed identity tuple is reported without rewriting the Registry"
        );
        assert!(
            healthy["identity"]["mismatched_fields"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // Endpoints are redacted destination summaries.
        assert_eq!(healthy["rpc_endpoint"], "ws://[REDACTED_IP]:****");
        assert!(
            healthy["lifecycle_guidance"]
                .as_str()
                .unwrap()
                .contains("Agent-local")
        );
        // Mismatch is a typed, distinct diagnostic; last-good values remain.
        let mismatched = by_id("node-mismatched");
        assert_eq!(mismatched["identity"]["state"], "mismatched");
        assert_eq!(
            mismatched["identity"]["mismatched_fields"],
            serde_json::json!(["chain_id"])
        );
        assert_eq!(mismatched["health"], "unhealthy");
        assert_eq!(mismatched["health_reason"], "RPC collection failed");
        // Retired guidance is explicit and never implies remote control.
        let retired = by_id("node-retired");
        assert_eq!(retired["lifecycle"], "retired");
        assert!(
            retired["lifecycle_guidance"]
                .as_str()
                .unwrap()
                .contains("Reactivation requires declaring the same Node ID")
        );
        assert!(
            retired["lifecycle_guidance"]
                .as_str()
                .unwrap()
                .contains("never changes Node lifecycle")
        );
    }

    #[tokio::test]
    async fn admin_node_detail_includes_identity_and_per_component_diagnostics() {
        let (_dir, state) = node_inventory_state().await;
        let response = admin_node_detail(
            State(state.clone()),
            Path("node-mismatched".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["node_id"], "node-mismatched");
        assert_eq!(value["identity"]["state"], "mismatched");
        assert_eq!(value["rpc"]["state"], "error");
        assert_eq!(value["rpc"]["error_message"], "RPC probe failed");
        assert_eq!(
            value["sync"]["state"], "ok",
            "last-good sync stays visible beside the RPC error"
        );

        assert_eq!(value["sync"]["current_block"], 99);
        assert_eq!(value["current_head"], 99);
        assert_eq!(value["consensus"]["validator"], false);
        let response = admin_node_detail(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["node_key_fingerprint"], "fp-healthy");
        assert_eq!(value["health"], "healthy");
        assert_eq!(value["peers"]["state"], "ok");
        assert_eq!(value["peers"]["peer_count"], 1);
        assert_eq!(value["peers"]["peers"][0]["peer_id"], "peer-healthy");
        assert_eq!(value["peers"]["peers"][0]["direction"], "inbound");
        assert_eq!(
            value["peers"]["peers"][0]["capabilities"],
            serde_json::json!(["cbft/1"])
        );
        assert!(
            !value.to_string().contains("203.0.113.7"),
            "Admin diagnostics must not expose the stored remote address"
        );
        let response = admin_node_peer_churn(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let churn: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(churn["state"], "ok");
        assert_eq!(churn["recent_departures"][0]["peer_id"], "peer-healthy");
        assert!(churn["recent_departures"][0]["duration_seconds"].is_number());
        assert!(!churn.to_string().contains("203.0.113.7"));
        sqlx::query("INSERT INTO peer_aggregate_5m (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind("node-healthy")
            .bind("2026-08-12T10:05:00Z")
            .bind(1_i64)
            .bind(1_i64)
            .bind(1_i64)
            .bind(0_i64)
            .bind(1_i64)
            .bind(0_i64)
            .bind(1_i64)
            .bind(1_i64)
            .bind(0_i64)
            .bind(1_i64)
            .bind(0_i64)
            .bind(0_i64)
            .bind(0_i64)
            .bind(Option::<i64>::None)
            .bind(Option::<i64>::None)
            .bind("2026-08-12T10:05:00Z")
            .bind("2026-08-12T10:09:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO peer_aggregate_5m_countries (node_id, bucket_start, country_code, peer_count) VALUES (?, ?, ?, ?)")
            .bind("node-healthy")
            .bind("2026-08-12T10:05:00Z")
            .bind("US")
            .bind(1_i64)
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = admin_node_peer_history(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let peer_history: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(peer_history["state"], "ok");
        assert_eq!(
            peer_history["five_minute"][0]["countries"][0]["country_code"],
            "US"
        );
        assert!(peer_history["hourly"].is_array());
        assert!(!peer_history.to_string().contains("203.0.113.7"));
        assert!(!peer_history.to_string().contains("peer-healthy"));
        sqlx::query(
            "UPDATE component_status SET state='error', error_code='admin_peers_failed', error_message='peer probe failed' WHERE node_id='node-healthy' AND component_key='peers'",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        let response = admin_node_peer_churn(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let churn: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(churn["state"], "error");
        assert_eq!(churn["recent_departures"][0]["peer_id"], "peer-healthy");
        sqlx::query(
            "UPDATE component_status SET state='unsupported', error_code=NULL, error_message=NULL WHERE node_id='node-healthy' AND component_key='peers'",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        let response = admin_node_peer_churn(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let churn: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(churn["state"], "unsupported");
        // Unknown Node is a non-leaking 404 for Admin too.
        let response = admin_node_detail(
            State(state.clone()),
            Path("node-missing".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn partial_identity_observation_stays_unknown_and_never_fabricates_values() {
        let (_dir, state) = node_inventory_state().await;
        // The retired Node observes only a chain id; genesis and HRP are
        // never observed. A partial observation must not claim `matched`
        // or invent Registry values for the missing fields.
        sqlx::query(
            "INSERT INTO current_node_chain_observations (node_id, network_chain_id, updated_at) VALUES ('node-retired', 210425, '2026-01-01T00:00:00Z')",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        let response = admin_node_detail(
            State(state.clone()),
            Path("node-retired".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["identity"]["state"], "unknown");
        assert_eq!(value["identity"]["observed"]["chain_id"], 210425);
        assert_eq!(value["identity"]["observed"]["genesis_hash"], Value::Null);
        assert_eq!(value["identity"]["observed"]["address_hrp"], Value::Null);
        assert!(
            value["identity"]["mismatched_fields"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn redact_endpoint_strips_userinfo_and_masks_the_port() {
        assert_eq!(
            redact_endpoint("ws://127.0.0.1:6790"),
            "ws://[REDACTED_IP]:****"
        );
        assert_eq!(
            redact_endpoint("wss://user:sekret@example.org:6790/path?token=x"),
            "wss://example.org:****"
        );
        assert_eq!(redact_endpoint("no-scheme"), "redacted");
    }

    #[tokio::test]
    async fn node_metadata_mutation_updates_display_name_and_audits() {
        let (_dir, state) = node_inventory_state().await;
        let session = lifecycle_session();
        // CSRF / origin / content-type guard runs before parsing.
        let response = set_node_metadata(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            mutation_headers("wrong-csrf"),
            Extension(session.clone()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"displayName":"Atlas"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        // Invalid display names are rejected without touching the row.
        let response = set_node_metadata(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            mutation_headers("csrf"),
            Extension(session.clone()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"displayName":""}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        // A Node with no Server-owned name (NULL display_name) is still
        // renameable instead of failing on the nullable decode.
        sqlx::query("UPDATE nodes SET display_name = NULL WHERE node_id = 'node-retired'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = set_node_metadata(
            State(state.clone()),
            Path("node-retired".to_owned()),
            mutation_headers("csrf"),
            Extension(session.clone()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"displayName":"Retired Atlas"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let display_name: Option<String> =
            sqlx::query_scalar("SELECT display_name FROM nodes WHERE node_id = 'node-retired'")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(display_name.as_deref(), Some("Retired Atlas"));
        // Success updates only the Server-owned display name and audits.
        let response = set_node_metadata(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            mutation_headers("csrf"),
            Extension(session.clone()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"displayName":"Atlas-01"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let display_name: String =
            sqlx::query_scalar("SELECT display_name FROM nodes WHERE node_id = 'node-healthy'")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(display_name, "Atlas-01");
        let audits: Vec<(String, String)> = sqlx::query_as(
            "SELECT event_kind, target_id FROM audit_events WHERE event_kind = 'node_metadata_changed' ORDER BY target_id",
        )
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            audits,
            vec![
                (
                    "node_metadata_changed".to_owned(),
                    "node-healthy".to_owned()
                ),
                (
                    "node_metadata_changed".to_owned(),
                    "node-retired".to_owned()
                ),
            ]
        );
        // Unknown Node is a non-leaking 404.
        let response = set_node_metadata(
            State(state.clone()),
            Path("node-missing".to_owned()),
            mutation_headers("csrf"),
            Extension(session),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"displayName":"X"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn network_registry_admin_views_report_tuple_and_typed_mismatches() {
        let (_dir, state) = node_inventory_state().await;
        let response = admin_networks(
            State(state.clone()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let networks = value.as_array().unwrap();
        assert_eq!(networks.len(), 1);
        let network = &networks[0];
        assert_eq!(network["network_key"], "mainnet");
        assert_eq!(network["display_name"], "PlatON Mainnet");
        assert_eq!(
            network["genesis_hash"],
            "0x0100000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(network["chain_id"], 210425);
        assert_eq!(network["p2p_network_id"], 1);
        assert_eq!(network["address_hrp"], "lat");
        assert_eq!(network["active_node_count"], 2);
        assert_eq!(network["retired_node_count"], 1);
        assert_eq!(network["mismatched_node_count"], 1);
        let response = admin_network_detail(
            State(state.clone()),
            Path("mainnet".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let nodes = value["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);
        let mismatched = nodes
            .iter()
            .find(|node| node["node_id"] == "node-mismatched")
            .unwrap();
        assert_eq!(mismatched["identity"]["state"], "mismatched");
        assert_eq!(
            mismatched["identity"]["mismatched_fields"],
            serde_json::json!(["chain_id"])
        );
        assert_eq!(mismatched["health"], "unhealthy");
        assert!(
            mismatched["health_reason"]
                .as_str()
                .unwrap()
                .contains("RPC")
        );
        let response = admin_network_detail(
            State(state.clone()),
            Path("missing".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn network_create_requires_an_explicit_validated_tuple() {
        let (_dir, state) = lifecycle_state().await;
        let session = lifecycle_session();
        // An invalid tuple is rejected with a typed 400.
        let response = create_network(
            State(state.clone()),
            mutation_headers("csrf"),
            Extension(session.clone()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"networkKey":"bad key!","displayName":"X","genesisHash":"nope","chainId":1,"p2pNetworkId":1,"addressHrp":"lat"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM networks")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "invalid tuples never create Registry rows");
        // A complete explicit tuple registers with an Owner audit.
        let response = create_network(
            State(state.clone()),
            mutation_headers("csrf"),
            Extension(session.clone()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"networkKey":"testnet","displayName":"PlatON Testnet","genesisHash":"0x0200000000000000000000000000000000000000000000000000000000000000","chainId":210426,"p2pNetworkId":2,"addressHrp":"lat"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let audit: (String, String) = sqlx::query_as(
            "SELECT event_kind, actor_user_id FROM audit_events WHERE event_kind = 'network_created'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit, ("network_created".to_owned(), "owner".to_owned()));
        // Duplicate keys conflict instead of overwriting.
        let response = create_network(
            State(state.clone()),
            mutation_headers("csrf"),
            Extension(session),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"networkKey":"testnet","displayName":"Again","genesisHash":"0x0200000000000000000000000000000000000000000000000000000000000000","chainId":210426,"p2pNetworkId":2,"addressHrp":"lat"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn network_update_rewrites_only_the_registry_tuple_and_audits_before_after() {
        let (_dir, state) = node_inventory_state().await;
        let session = lifecycle_session();
        // Empty updates are rejected.
        let response = update_network(
            State(state.clone()),
            Path("mainnet".to_owned()),
            mutation_headers("csrf"),
            Extension(session.clone()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        // An Owner may correct the expected identity; Nodes are untouched
        // and their typed mismatch dispositions follow.
        let response = update_network(
            State(state.clone()),
            Path("mainnet".to_owned()),
            mutation_headers("csrf"),
            Extension(session.clone()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"displayName":"PlatON Mainnet v2","chainId":210425}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let tuple: (String, i64) = sqlx::query_as(
            "SELECT display_name, chain_id FROM networks WHERE network_key = 'mainnet'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(tuple, ("PlatON Mainnet v2".to_owned(), 210_425));
        let audit: (String, String, String) = sqlx::query_as(
            "SELECT event_kind, before_json, after_json FROM audit_events WHERE event_kind = 'network_updated'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit.0, "network_updated");
        assert!(audit.1.contains("PlatON Mainnet"));
        assert!(audit.1.contains("210425"));
        assert!(audit.2.contains("PlatON Mainnet"));
        assert!(audit.2.contains("PlatON Mainnet v2"));
        assert!(audit.2.contains("210425"));
        // The registered Node list is unchanged by a Registry edit.
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE network_key = 'mainnet'")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(count, 3);
        // Unknown Network is a non-leaking 404.
        let response = update_network(
            State(state.clone()),
            Path("missing".to_owned()),
            mutation_headers("csrf"),
            Extension(session),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"displayName":"X"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// State with an Owner, two Agents, a Network, and one Node owned by
    /// the source Agent — everything a Transfer workflow needs.
    async fn transfer_state() -> (tempfile::TempDir, AppState) {
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
        let state = AppState::new(database, None, auth);
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(state.db().pool()).await.unwrap();
        for agent in ["agent-source", "agent-target"] {
            sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES (?, 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
                .bind(agent)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('node-transfer-test', 'agent-source', 'mainnet', 'Node T', 'ws://127.0.0.1:1', 'active', 'private', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        (dir, state)
    }

    fn transfer_session() -> AuthenticatedSession {
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

    #[tokio::test]
    async fn create_transfer_persists_pending_with_audit_and_request_reference() {
        let (_dir, state) = transfer_state().await;
        let response = create_node_transfer(
            State(state.clone()),
            Path("node-transfer-test".to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"targetAgentId":"agent-target","expiresInHours":48,"operatorReason":"move validator host"}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["transfer"]["status"], "pending");
        assert_eq!(value["transfer"]["source_agent_id"], "agent-source");
        assert_eq!(value["transfer"]["target_agent_id"], "agent-target");
        assert_eq!(value["transfer"]["operator_reason"], "move validator host");
        assert_eq!(value["request_id"], "req-123");
        let audit_id = value["audit_event_id"].as_i64().unwrap();
        let audit: (String, String) = sqlx::query_as(
            "SELECT event_kind, after_json FROM audit_events WHERE audit_event_id=?",
        )
        .bind(audit_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit.0, "node_transfer_created");
        assert!(audit.1.contains("agent-target"));
        assert!(!audit.1.contains("csrf"));
        // Default expiry is 72 hours when omitted.
        let response = create_node_transfer(
            State(state.clone()),
            Path("node-transfer-test".to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"targetAgentId":"agent-target"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn transfer_conflict_is_typed_persisted_and_preserves_ownership() {
        let (_dir, state) = transfer_state().await;
        let first = create_node_transfer(
            State(state.clone()),
            Path("node-transfer-test".to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"targetAgentId":"agent-target"}"#),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let conflict = create_node_transfer(
            State(state.clone()),
            Path("node-transfer-test".to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"targetAgentId":"agent-target"}"#),
        )
        .await;
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let body = to_bytes(conflict.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["code"], "transfer_conflict");
        assert_eq!(value["error"]["requestId"], "req-123");
        // The conflict attempt is retained as a typed row and audited; the
        // pending transfer and source ownership are untouched.
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT status, target_agent_id FROM node_transfers WHERE node_id='node-transfer-test' ORDER BY created_at",
        )
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("pending".to_owned(), "agent-target".to_owned()));
        assert_eq!(rows[1], ("conflict".to_owned(), "agent-target".to_owned()));
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_conflict'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
        let owner: String =
            sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id='node-transfer-test'")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(owner, "agent-source");
    }

    #[tokio::test]
    async fn transfer_create_validates_target_and_expiry() {
        let (_dir, state) = transfer_state().await;
        // The current owner cannot be the target.
        let response = create_node_transfer(
            State(state.clone()),
            Path("node-transfer-test".to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"targetAgentId":"agent-source"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        // An unregistered target is refused.
        let response = create_node_transfer(
            State(state.clone()),
            Path("node-transfer-test".to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"targetAgentId":"ghost"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        // Expiry bounds are Server-enforced (1..=168 hours).
        let response = create_node_transfer(
            State(state.clone()),
            Path("node-transfer-test".to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"targetAgentId":"agent-target","expiresInHours":200}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        // Unknown Node is a non-leaking 404.
        let response = create_node_transfer(
            State(state.clone()),
            Path("missing".to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"targetAgentId":"agent-target"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cancel_transfer_is_typed_audited_and_restricted_to_pending() {
        let (_dir, state) = transfer_state().await;
        let created = create_node_transfer(
            State(state.clone()),
            Path("node-transfer-test".to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"targetAgentId":"agent-target"}"#),
        )
        .await;
        assert_eq!(created.status(), StatusCode::OK);
        let created_body: Value =
            serde_json::from_slice(&to_bytes(created.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let transfer_id = created_body["transfer"]["transfer_id"].as_str().unwrap();

        let cancelled = cancel_node_transfer(
            State(state.clone()),
            Path(transfer_id.to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(cancelled.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(cancelled.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["transfer"]["status"], "cancelled");
        assert!(value["transfer"]["cancelled_at"].is_string());
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_cancelled'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
        let owner: String =
            sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id='node-transfer-test'")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(owner, "agent-source");

        // A second cancel is a typed 409: only pending can be cancelled.
        let again = cancel_node_transfer(
            State(state.clone()),
            Path(transfer_id.to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(again.status(), StatusCode::CONFLICT);
        // Unknown transfer is a non-leaking 404.
        let missing = cancel_node_transfer(
            State(state.clone()),
            Path("no-such-transfer".to_owned()),
            mutation_headers("csrf"),
            Extension(transfer_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn transfer_list_materializes_expired_pending_and_detail_carries_state() {
        let (_dir, state) = transfer_state().await;
        sqlx::query(
            "INSERT INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, created_at, expires_at, updated_at) VALUES ('transfer-stale-1', 'node-transfer-test', 'agent-source', 'agent-target', 'pending', '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        let response = admin_node_transfers(
            State(state.clone()),
            Path("node-transfer-test".to_owned()),
            Extension(transfer_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value.as_array().unwrap().len(), 1);
        assert_eq!(value[0]["status"], "expired");
        // The expiry was materialized (row + Audit) exactly once.
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-stale-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "expired");
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_expired'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
        // The list route 404s for unknown Nodes without leaking.
        let missing = admin_node_transfers(
            State(state.clone()),
            Path("missing".to_owned()),
            Extension(transfer_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        // The Node detail carries the most recent Transfer summary.
        let detail = admin_node_detail(
            State(state.clone()),
            Path("node-transfer-test".to_owned()),
            Extension(transfer_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(detail.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(detail.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["transfer"]["transfer_id"], "transfer-stale-1");
        assert_eq!(value["transfer"]["status"], "expired");
    }
}
