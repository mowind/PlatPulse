//! Owner-only Agent/Node current observation diagnostics.
use super::{AppState, ROUTE_GROUP_HEADER, api_not_found};
use crate::http::realtime;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
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
        Some("owner".to_owned()),
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
    let sync = sync_diagnostic(state, &node_id).await;
    let consensus = consensus_diagnostic(state, &node_id).await;
    let (health, health_reason) =
        derive_health(&lifecycle, rpc.as_ref(), sync.as_ref(), consensus.as_ref());
    let freshness = derive_freshness(
        rpc.as_ref()
            .and_then(|c| c.received_at.as_deref())
            .into_iter()
            .chain(sync.as_ref().and_then(|c| c.received_at.as_deref()))
            .chain(consensus.as_ref().and_then(|c| c.received_at.as_deref())),
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
    let capabilities = serde_json::from_str(&capabilities_json).unwrap_or_default();
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
        shutdown_last_error,
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
fn mutation_guard(
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
        Ok(record) => Json(EnrollmentTokenResponse {
            token_id: record.token_id,
            token: record.token,
            expires_at: record.expires_at,
            lifetime_hours,
            request_id: request_id.0.to_string(),
        })
        .into_response(),
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
        Ok(record) => Json(RecoveryTokenResponse {
            agent_id,
            agent_epoch,
            token_id: record.token_id,
            token: record.token,
            expires_at: record.expires_at,
            request_id: request_id.0.to_string(),
        })
        .into_response(),
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
        Ok(rotated) => Json(RotationResponse {
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
                        actor_username,
                        created_at,
                        details: after_json
                            .as_deref()
                            .and_then(|body| serde_json::from_str(body).ok()),
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

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/history",
    tag = "admin",
    params(
        ("node_id" = String, Path, description = "Node ID"),
        ("from" = Option<u64>, Query, description = "First block height"),
        ("to" = Option<u64>, Query, description = "Last block height"),
        ("limit" = Option<u16>, Query, description = "Maximum rows")
    ),
    responses((status = 200, body = AdminBlockHistoryResponse), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody))
)]
async fn admin_node_history(
    State(state): State<AppState>,
    Extension(_session): Extension<super::AuthenticatedSession>,
    axum::extract::Query(params): axum::extract::Query<super::public::HistoryQuery>,
    Path(node_id): Path<String>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let cutoff = crate::auth::format_rfc3339(crate::retention::raw_block_summary_cutoff(
        crate::auth::now_utc(),
    ));
    let oldest_raw = sqlx::query_scalar::<_, Option<String>>(
        "SELECT MIN(accepted_at) FROM block_summaries WHERE node_id=?",
    )
    .bind(&node_id)
    .fetch_one(state.db().pool())
    .await
    .unwrap_or(None);
    let has_history = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT historical_high_watermark FROM block_history_state WHERE node_id=?",
    )
    .bind(&node_id)
    .fetch_one(state.db().pool())
    .await
    .unwrap_or(None)
    .is_some_and(|height| height > 0);
    let has_expired_raw = oldest_raw.as_ref().is_some_and(|value| value < &cutoff);
    let availability = (has_expired_raw || (oldest_raw.is_none() && has_history))
        .then(|| "unavailable".to_owned());
    let rows = sqlx::query_as::<_, super::public::HistoryRow>("SELECT block_number, block_timestamp_ms, transaction_count, source, coinbase, seal_signer_match, seal_signer_key_fingerprint, node_key_fingerprint, node_key_valid_from, node_key_valid_until, seal_recovery_rule, seal_evidence, CASE WHEN protocol_proposer_kind = 'verified' THEN protocol_proposer_identity ELSE NULL END, attribution_reason, observed_at, from_height, to_height, gap_kind, gap_reason, divergence_kind, divergence_reason, divergence_retained_hash, divergence_observed_hash, divergence_observed_at FROM (SELECT block_number, block_timestamp_ms, transaction_count, 'summary', coinbase, seal_signer_match, seal_signer_key_fingerprint, node_key_fingerprint, node_key_valid_from, node_key_valid_until, seal_recovery_rule, seal_evidence, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, observed_at, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL FROM block_summaries WHERE node_id = ? UNION ALL SELECT NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, created_at, from_height, to_height, kind, reason, NULL, NULL, NULL, NULL, NULL FROM block_history_gaps WHERE node_id = ? UNION ALL SELECT NULL, NULL, NULL, 'divergence', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, retained_observed_at, height, height, NULL, NULL, 'chain_divergence', reason, retained_block_hash, observed_block_hash, observed_at FROM chain_divergence_observations WHERE node_id = ?) ORDER BY COALESCE(block_number, from_height) DESC LIMIT ?")
        .bind(&node_id).bind(&node_id).bind(&node_id).bind(limit).fetch_all(state.db().pool()).await.unwrap_or_default();
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
        availability,
        aggregate_supported: crate::retention::RAW_BLOCK_HISTORY_AGGREGATES_SUPPORTED,
        raw_retention_days: crate::retention::RAW_BLOCK_SUMMARY_RETENTION_DAYS,
    })
}
pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/events", get(admin_events))
        .route("/overview", get(overview))
        .route("/nodes/{node_id}/history", get(admin_node_history))
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
}
