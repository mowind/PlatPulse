//! Node Purge impact preview and purge.
use super::*;
use crate::http::realtime::{AdminResource, PublicResource, ResetReason};

/// Owner-confirmed Node Purge request. The echoed Node ID is friction and a
/// scope re-check, never the authorization boundary; the Server still
/// re-measures the impact inside the mutation transaction.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodePurgeRequest {
    pub confirm_node_id: String,
}

/// Server-computed scope of a Node Purge. These are counts of Node-owned rows
/// only; shared Agent/Host/Network/Validator data is never included.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminNodePurgeCounts {
    pub component_statuses: i64,
    pub process_observations: i64,
    pub data_directory_observations: i64,
    pub chain_observations: i64,
    pub rpc_namespaces: i64,
    pub rpc_methods: i64,
    pub current_peers: i64,
    pub current_peer_capabilities: i64,
    pub peer_presence_intervals: i64,
    pub peer_aggregate_5m: i64,
    pub peer_aggregate_5m_countries: i64,
    pub peer_aggregate_1h: i64,
    pub peer_aggregate_1h_countries: i64,
    pub block_summaries: i64,
    pub block_history_states: i64,
    pub block_coverage_intervals: i64,
    pub block_identity_window: i64,
    pub block_history_gaps: i64,
    pub chain_divergence_observations: i64,
    pub observed_network_heads: i64,
    pub metric_samples: i64,
    pub metric_series_state: i64,
    pub metric_aggregates: i64,
    /// Block-tier buckets counted from this Node's blocks (issue #248).
    pub block_aggregates: i64,
    /// Recorded sync/consensus state entries (issue #217).
    pub state_observations: i64,
    /// The recorded-state series ledger.
    pub state_series_state: i64,
    pub capacity_skipped_series: i64,
    pub validator_links: i64,
    pub validator_identity_status: i64,
    pub transfers: i64,
    /// Recorded relationship intervals: the relations the Server wrote down
    /// for this Node over time (issue #221). They describe this Node's place,
    /// so a Purge removes them with it.
    pub relationship_intervals: i64,
    pub total_owned_rows: i64,
}

/// The exact Node an Owner is about to delete, with Server-owned metadata and
/// the latest Inventory lifecycle. It is deliberately separate from the
/// purge counts so the confirmation shows identity and impact distinctly.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminNodePurgeTarget {
    pub node_id: String,
    pub agent_id: String,
    pub network_key: String,
    pub network_display_name: String,
    pub display_name: Option<String>,
    pub lifecycle: String,
    pub lifecycle_guidance: String,
    pub visibility: String,
    pub inventory_revision: i64,
    /// Redacted Agent-declared endpoint.
    pub rpc_endpoint: String,
    pub first_seen_at: String,
    pub updated_at: String,
}

/// Owner-only impact preview returned before a Purge. It reuses the same
/// Server-side measurement the mutation performs, so a confirmation cannot
/// describe a different scope than the one deleted.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminNodePurgeImpact {
    pub target: AdminNodePurgeTarget,
    pub counts: AdminNodePurgeCounts,
}

/// Authoritative completion of a Node Purge. The response is only produced
/// after the transaction that removed the rows, recorded the deletion
/// identity, and appended the Audit Event has committed.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NodePurgeResponse {
    pub node_id: String,
    pub deleted_at: String,
    pub removed: AdminNodePurgeCounts,
}

pub(super) fn node_purge_counts(
    counts: crate::node_purge::NodePurgeCounts,
) -> AdminNodePurgeCounts {
    AdminNodePurgeCounts {
        component_statuses: counts.component_statuses,
        process_observations: counts.process_observations,
        data_directory_observations: counts.data_directory_observations,
        chain_observations: counts.chain_observations,
        rpc_namespaces: counts.rpc_namespaces,
        rpc_methods: counts.rpc_methods,
        current_peers: counts.current_peers,
        current_peer_capabilities: counts.current_peer_capabilities,
        peer_presence_intervals: counts.peer_presence_intervals,
        peer_aggregate_5m: counts.peer_aggregate_5m,
        peer_aggregate_5m_countries: counts.peer_aggregate_5m_countries,
        peer_aggregate_1h: counts.peer_aggregate_1h,
        peer_aggregate_1h_countries: counts.peer_aggregate_1h_countries,
        block_summaries: counts.block_summaries,
        block_history_states: counts.block_history_states,
        block_coverage_intervals: counts.block_coverage_intervals,
        block_identity_window: counts.block_identity_window,
        block_history_gaps: counts.block_history_gaps,
        chain_divergence_observations: counts.chain_divergence_observations,
        observed_network_heads: counts.observed_network_heads,
        metric_samples: counts.metric_samples,
        metric_series_state: counts.metric_series_state,
        metric_aggregates: counts.metric_aggregates,
        block_aggregates: counts.block_aggregates,
        state_observations: counts.state_observations,
        state_series_state: counts.state_series_state,
        capacity_skipped_series: counts.capacity_skipped_series,
        validator_links: counts.validator_links,
        validator_identity_status: counts.validator_identity_status,
        transfers: counts.transfers,
        relationship_intervals: counts.relationship_intervals,
        total_owned_rows: counts.total_owned_rows(),
    }
}

fn node_purge_impact(impact: crate::node_purge::NodePurgeImpact) -> AdminNodePurgeImpact {
    AdminNodePurgeImpact {
        target: AdminNodePurgeTarget {
            node_id: impact.target.node_id,
            agent_id: impact.target.agent_id,
            network_key: impact.target.network_key,
            network_display_name: impact.target.network_display_name,
            display_name: impact.target.display_name,
            lifecycle_guidance: lifecycle_guidance(&impact.target.lifecycle).to_owned(),
            lifecycle: impact.target.lifecycle,
            visibility: impact.target.visibility,
            inventory_revision: impact.target.inventory_revision,
            rpc_endpoint: redact_endpoint(&impact.target.rpc_endpoint),
            first_seen_at: impact.target.first_seen_at,
            updated_at: impact.target.updated_at,
        },
        counts: node_purge_counts(impact.counts),
    }
}

/// Owner-only impact preview for an explicit, permanent Node Purge. Read-only:
/// it never mutates, and a missing Node returns the same non-leaking 404 as
/// every other Admin Node read.
#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/purge",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    responses((status = 200, body = AdminNodePurgeImpact), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_node_purge_preview(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    match crate::node_purge::preview(state.db().pool(), &node_id).await {
        Ok(Some(impact)) => Json(node_purge_impact(impact)).into_response(),
        Ok(None) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        ),
        Err(error) => storage_failure(&state, &request_id.0, &error),
    }
}

/// Owner-only permanent Node Purge. Re-validates the browser trust boundary,
/// re-measures the impact inside the mutation transaction, removes the Node
/// and its owned data, persists the minimal deletion identity, appends the
/// Audit Event, and only then reports completion. The remote Agent process is
/// never contacted.
#[utoipa::path(
    post,
    path = "/api/admin/v1/nodes/{node_id}/purge",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    request_body = NodePurgeRequest,
    responses((status = 200, body = NodePurgeResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn purge_node(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
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
    let body: NodePurgeRequest = match serde_json::from_slice(&body) {
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
    if body.confirm_node_id != node_id {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            "confirmation does not match the Node being deleted",
        );
    }
    // A Report admitting this Node reads its purge tombstone before it
    // upserts the projection; holding the Node fence keeps a Purge from
    // committing between those two steps and the Report rebuilding the Node
    // it has just deleted (issue #280).
    let _node_fence = state.write_fences().nodes.hold(&node_id).await;
    match crate::node_purge::execute(state.db().pool(), &node_id, &principal.0.user_id).await {
        Ok(Some(outcome)) => {
            // A purge is a collection change for both namespaces. The Admin
            // event may name the Node; the Public reset never carries a
            // private Node ID.
            state
                .admin_realtime()
                .publish(AdminResource::Node, Some(node_id.clone()));
            state
                .admin_realtime()
                .publish_reset(ResetReason::Collection);
            state.public_realtime().publish(
                PublicResource::Network,
                Some(outcome.impact.target.network_key.clone()),
            );
            state
                .public_realtime()
                .publish_reset(ResetReason::Collection);
            Json(NodePurgeResponse {
                node_id: outcome.impact.target.node_id,
                deleted_at: outcome.deleted_at,
                removed: node_purge_counts(outcome.impact.counts),
            })
            .into_response()
        }
        Ok(None) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        ),
        Err(error) => storage_failure(&state, &request_id.0, &error),
    }
}
