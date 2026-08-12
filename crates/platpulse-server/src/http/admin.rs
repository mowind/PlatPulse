//! Owner-only Agent/Node current observation diagnostics.
use super::{AppState, ROUTE_GROUP_HEADER, api_not_found};
use crate::http::realtime;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
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

fn mutation_error(
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

async fn admin_events(
    State(state): State<AppState>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let cursor =
        realtime::parse_last_event_id(headers.get("last-event-id").and_then(|v| v.to_str().ok()));
    Sse::new(state.admin_realtime().stream(cursor)).keep_alive(
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
    let content_type_valid = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"));
    let origin_valid = state.auth().origin_matches(headers.get(header::ORIGIN));
    let csrf_valid = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| bool::from(token.as_bytes().ct_eq(principal.0.csrf_token.as_bytes())));
    if !content_type_valid || !origin_valid || !csrf_valid {
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
    let before = sqlx::query_scalar::<_, String>("SELECT visibility FROM nodes WHERE node_id = ?")
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
    let audit = serde_json::json!({"visibility": body.visibility});
    if crate::auth::insert_audit_event(
        &mut *tx,
        Some(&principal.0.user_id),
        "node_visibility_changed",
        "node",
        &node_id,
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
    let _ = previous;
    let revision = changed_at.bytes().fold(0_u64, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as u64)
    });
    if body.visibility == "public" {
        state
            .public_realtime()
            .publish("node", Some(node_id.clone()), revision);
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
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentDiagnostic {
    pub agent_id: String,
    pub agent_epoch: i64,
    pub last_report_sequence: Option<i64>,
    pub clock_status: String,
    pub clock_skew_ms: Option<i64>,
    pub liveness: String,
    pub last_received_at: Option<String>,
    pub capabilities: Vec<String>,
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
    pub process: Option<ProcessDiagnostic>,
    pub rpc: Option<RpcDiagnostic>,
    pub sync: Option<SyncDiagnostic>,
    pub consensus: Option<ConsensusDiagnostic>,
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
        state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision,
        pid, started_at, cpu_percent, memory_bytes, uptime_ms,
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
        state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision,
        syncing: syncing.map(|value| value != 0), current_block, highest_block, pulled_states, known_states,
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
        state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision,
        epoch, view_number, validator: validator.map(|value| value != 0), highest_qc_block, highest_lock_block, highest_commit_block,
    })
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
    let agents = sqlx::query_as::<_, (String, i64, Option<i64>, String, String, Option<i64>, Option<String>)>(
        "SELECT agent_id, agent_epoch, last_report_sequence, agent_capabilities_json, clock_status, clock_skew_ms, last_received_at FROM agents ORDER BY agent_id",
    )
    .fetch_all(state.db().pool())
    .await
    .unwrap_or_default();
    let mut result = Vec::with_capacity(agents.len());
    for (
        agent_id,
        agent_epoch,
        last_report_sequence,
        capabilities_json,
        clock_status,
        clock_skew_ms,
        last_received_at,
    ) in agents
    {
        let capabilities = serde_json::from_str(&capabilities_json).unwrap_or_default();
        let liveness = last_received_at
            .as_deref()
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
            .to_owned();
        let host_components = sqlx::query_as::<_, (String, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64)>(
            "SELECT component_key, state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision FROM component_status WHERE agent_id = ? AND scope = 'host' ORDER BY component_key",
        )
        .bind(&agent_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(component, state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision)| HostComponentDiagnostic {
            component, state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision,
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
            spool_queued_bytes: row.spool_queued_bytes, spool_queued_reports: row.spool_queued_reports, spool_oldest_queued_age_ms: row.spool_oldest_queued_age_ms, spool_in_flight: row.spool_in_flight.map(|v| v != 0), spool_last_delivery_error: row.spool_last_delivery_error, spool_last_delivery_at: row.spool_last_delivery_at,
            spool_capacity_bytes: row.spool_capacity_bytes, spool_max_age_seconds: row.spool_max_age_seconds, spool_dropped_sequence_from: row.spool_dropped_sequence_from, spool_dropped_sequence_to: row.spool_dropped_sequence_to, spool_dropped_time_from: row.spool_dropped_time_from, spool_dropped_time_to: row.spool_dropped_time_to, spool_dropped_height_from: row.spool_dropped_height_from, spool_dropped_height_to: row.spool_dropped_height_to, spool_pending_history_gaps: row.spool_pending_history_gaps, spool_report_too_large: row.spool_report_too_large.map(|v| v != 0), spool_store_fatal: row.spool_store_fatal.map(|v| v != 0), spool_store_error: row.spool_store_error,
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
        for (node_id, network_key, display_name, lifecycle, inventory_revision, visibility) in rows
        {
            let process = process_diagnostic(&state, &node_id).await;
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
                    error_message,
                    attempted_at,
                    observed_at,
                    received_at,
                    state_revision,
                    value_revision,
                })
            } else {
                None
            };
            let sync = sync_diagnostic(&state, &node_id).await;
            let consensus = consensus_diagnostic(&state, &node_id).await;
            let (health, health_reason) =
                derive_health(&lifecycle, rpc.as_ref(), sync.as_ref(), consensus.as_ref());
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
            nodes.push(NodeDiagnostic {
                node_id,
                network_key,
                display_name,
                lifecycle,
                inventory_revision,
                visibility,
                health: health.to_owned(),
                health_reason: health_reason.to_owned(),
                process,
                rpc,
                sync,
                consensus,
                current_head,
                historical_high_watermark,
                resync_progress: historical_high_watermark
                    .zip(current_head)
                    .map(|(high, current)| format!("{current}/{high}")),
                network_reference_head,
                network_reference_confidence: network_reference_confidence
                    .unwrap_or_else(|| "unknown".to_owned()),
                resync_state,
            });
        }
        result.push(AgentDiagnostic {
            agent_id,
            agent_epoch,
            last_report_sequence,
            clock_status,
            clock_skew_ms,
            liveness,
            last_received_at,
            capabilities,
            host,
            nodes,
        });
    }
    Json(result)
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

async fn admin_node_history(
    State(state): State<AppState>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    let rows = sqlx::query_as::<_, super::public::HistoryRow>("SELECT block_number, block_timestamp_ms, transaction_count, source, coinbase, seal_signer_match, seal_signer_key_fingerprint, node_key_fingerprint, node_key_valid_from, node_key_valid_until, seal_recovery_rule, seal_evidence, CASE WHEN protocol_proposer_kind = 'verified' THEN protocol_proposer_identity ELSE NULL END, attribution_reason, observed_at, from_height, to_height, gap_kind, gap_reason, divergence_kind, divergence_reason, divergence_retained_hash, divergence_observed_hash, divergence_observed_at FROM (SELECT block_number, block_timestamp_ms, transaction_count, 'summary', coinbase, seal_signer_match, seal_signer_key_fingerprint, node_key_fingerprint, node_key_valid_from, node_key_valid_until, seal_recovery_rule, seal_evidence, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, observed_at, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL FROM block_summaries WHERE node_id = ? UNION ALL SELECT NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, created_at, from_height, to_height, kind, reason, NULL, NULL, NULL, NULL, NULL FROM block_history_gaps WHERE node_id = ? UNION ALL SELECT NULL, NULL, NULL, 'divergence', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, retained_observed_at, height, height, NULL, NULL, 'chain_divergence', reason, retained_block_hash, observed_block_hash, observed_at FROM chain_divergence_observations WHERE node_id = ?) ORDER BY COALESCE(block_number, from_height) DESC LIMIT 200")
        .bind(&node_id).bind(&node_id).bind(&node_id).fetch_all(state.db().pool()).await.unwrap_or_default();
    Json(
        rows.into_iter()
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
                seal_evidence: row.seal_evidence,
                protocol_proposer: row.protocol_proposer,
                attribution_reason: row.attribution_reason,
                freshness: row.observed_at.clone(),
                observed_at: row.observed_at,
                gap_from_height: row.from_height,
                gap_to_height: row.to_height,
                gap_kind: row.gap_kind,
                gap_reason: row.gap_reason,
                divergence_kind: row.divergence_kind,
                divergence_reason: row.divergence_reason,
                divergence_retained_hash: row.divergence_retained_hash,
                divergence_observed_hash: row.divergence_observed_hash,
                divergence_observed_at: row.divergence_observed_at,
            })
            .collect::<Vec<_>>(),
    )
}
pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/events", get(admin_events))
        .route("/nodes/{node_id}/history", get(admin_node_history))
        .route("/agents", get(diagnostics))
        .route("/nodes/{node_id}/visibility", put(set_visibility))
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
}
