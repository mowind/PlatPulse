//! Agent Removal impact preview and removal.
use super::*;
use crate::http::realtime::{AdminResource, ResetReason};

/// One Node an Agent authoritatively owns, listed in the removal confirmation
/// so the Owner sees exactly which Nodes will be permanently purged.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminAgentRemovalNode {
    pub node_id: String,
    pub network_key: String,
    pub network_display_name: String,
    pub display_name: Option<String>,
    pub lifecycle: String,
    pub visibility: String,
    pub inventory_revision: i64,
}

/// A still-pending Transfer involving the Agent that blocks its removal until
/// it is completed, cancelled, or expired.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminAgentRemovalTransfer {
    pub transfer_id: String,
    pub node_id: String,
    pub source_agent_id: String,
    pub target_agent_id: String,
    pub direction: String,
    pub expires_at: String,
}

/// The exact Agent an Owner is about to remove.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminAgentRemovalTarget {
    pub agent_id: String,
    pub display_name: Option<String>,
    pub notes: Option<String>,
    pub agent_epoch: i64,
    pub last_received_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Owner-only impact preview returned before an Agent Removal. It reuses the
/// same Server-side measurement the mutation performs, so a confirmation
/// cannot describe a different scope than the one erased.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminAgentRemovalImpact {
    pub target: AdminAgentRemovalTarget,
    pub owned_nodes: Vec<AdminAgentRemovalNode>,
    pub pending_transfers: Vec<AdminAgentRemovalTransfer>,
    pub counts: AdminNodePurgeCounts,
    pub credential_count: i64,
    pub active_credential_count: i64,
    /// False while an unhandled Transfer blocks the removal.
    pub can_remove: bool,
}

/// Owner-confirmed Agent Removal request. The echoed Agent ID and the Node ID
/// list are a scope re-check, never the authorization boundary: the Server
/// still re-measures ownership inside the mutation transaction and refuses to
/// delete a Node the Owner did not confirm.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentRemovalRequest {
    pub confirm_agent_id: String,
    #[serde(default)]
    pub confirmed_node_ids: Vec<String>,
}

/// One Node permanently purged by the Agent Removal.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentRemovalPurgedNode {
    pub node_id: String,
    pub removed: AdminNodePurgeCounts,
}

/// Authoritative completion of an Agent Removal. The response is only produced
/// after the transaction that revoked the credentials, purged the owned Nodes,
/// marked the Agent removed, and appended the Audit Event has committed.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentRemovalResponse {
    pub agent_id: String,
    pub deleted_at: String,
    pub revoked_credential_count: i64,
    pub purged_nodes: Vec<AgentRemovalPurgedNode>,
    pub removed: AdminNodePurgeCounts,
}

fn agent_removal_impact(
    impact: crate::agent_removal::AgentRemovalImpact,
) -> AdminAgentRemovalImpact {
    let can_remove = impact.pending_transfers.is_empty();
    AdminAgentRemovalImpact {
        target: AdminAgentRemovalTarget {
            agent_id: impact.target.agent_id,
            display_name: impact.target.display_name,
            notes: impact.target.notes,
            agent_epoch: impact.target.agent_epoch,
            last_received_at: impact.target.last_received_at,
            created_at: impact.target.created_at,
            updated_at: impact.target.updated_at,
        },
        owned_nodes: impact
            .owned_nodes
            .into_iter()
            .map(|node| AdminAgentRemovalNode {
                node_id: node.node_id,
                network_key: node.network_key,
                network_display_name: node.network_display_name,
                display_name: node.display_name,
                lifecycle: node.lifecycle,
                visibility: node.visibility,
                inventory_revision: node.inventory_revision,
            })
            .collect(),
        pending_transfers: impact
            .pending_transfers
            .into_iter()
            .map(|transfer| AdminAgentRemovalTransfer {
                transfer_id: transfer.transfer_id,
                node_id: transfer.node_id,
                source_agent_id: transfer.source_agent_id,
                target_agent_id: transfer.target_agent_id,
                direction: transfer.direction,
                expires_at: transfer.expires_at,
            })
            .collect(),
        counts: node_purge_counts(impact.counts),
        credential_count: impact.credential_count,
        active_credential_count: impact.active_credential_count,
        can_remove,
    }
}

fn agent_removal_response(
    result: crate::agent_removal::AgentRemovalResult,
) -> AgentRemovalResponse {
    let crate::agent_removal::AgentRemovalResult {
        target,
        purged_nodes,
        counts,
        revoked_credential_count,
        deleted_at,
    } = result;
    AgentRemovalResponse {
        agent_id: target.agent_id,
        deleted_at,
        revoked_credential_count,
        purged_nodes: purged_nodes
            .into_iter()
            .map(|node| AgentRemovalPurgedNode {
                node_id: node.node_id,
                removed: node_purge_counts(node.counts),
            })
            .collect(),
        removed: node_purge_counts(counts),
    }
}

/// Owner-only impact preview for an explicit Agent Removal. Read-only: it
/// never mutates, and a missing or already-removed Agent returns the same
/// non-leaking 404 as every other Admin Agent read.
#[utoipa::path(
    get,
    path = "/api/admin/v1/agents/{agent_id}/removal",
    tag = "admin",
    params(("agent_id" = String, Path, description = "Agent ID")),
    responses((status = 200, body = AdminAgentRemovalImpact), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_agent_removal_preview(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    match crate::agent_removal::preview(state.db().pool(), &agent_id).await {
        Ok(Some(impact)) => Json(agent_removal_impact(impact)).into_response(),
        Ok(None) => mutation_error(
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

/// Owner-only Agent Removal. Re-validates the browser trust boundary,
/// re-measures ownership inside the mutation transaction, refuses when an
/// unhandled Transfer exists or the owned-Node set changed since the
/// confirmation, then revokes every credential, purges the owned Nodes through
/// the Node Purge path, marks the Agent removed, appends the Audit Event, and
/// only then reports completion. The remote Agent process is never contacted
/// and local configuration is never changed.
#[utoipa::path(
    post,
    path = "/api/admin/v1/agents/{agent_id}/removal",
    tag = "admin",
    params(("agent_id" = String, Path, description = "Agent ID")),
    request_body = AgentRemovalRequest,
    responses((status = 200, body = AgentRemovalResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn remove_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
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
    let body: AgentRemovalRequest = match serde_json::from_slice(&body) {
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
    if body.confirm_agent_id != agent_id {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            "confirmation does not match the Agent being removed",
        );
    }
    // Agent Removal purges every owned Node through the Node Purge path, so it
    // shares the Purge's boundaries (issue #280): the Agent fence keeps a
    // Report already past authentication from committing between the removal's
    // reads and its deletes, and the Node fences keep a Transfer of a confirmed
    // Node from being created alongside it. A confirmed set that no longer
    // matches the owned Nodes is refused by the removal itself.
    let fences = state.write_fences();
    let _agent_fence = fences.agents.hold(&agent_id).await;
    let _node_fences = fences
        .nodes
        .hold_all(body.confirmed_node_ids.iter().cloned())
        .await;
    match crate::agent_removal::execute(
        state.db().pool(),
        &agent_id,
        &body.confirmed_node_ids,
        &principal.0.user_id,
    )
    .await
    {
        Ok(crate::agent_removal::AgentRemovalOutcome::Removed(result)) => {
            state
                .admin_realtime()
                .publish(AdminResource::Agent, Some(agent_id.clone()));
            state
                .admin_realtime()
                .publish_reset(ResetReason::Collection);
            state
                .public_realtime()
                .publish_reset(ResetReason::Collection);
            // The Admin stream may name each purged Node so its view can
            // invalidate precisely. The Public stream gets only the
            // collection reset: a purged Node may be private, and the sibling
            // Node Purge deliberately never publishes a Node ID publicly.
            for node in &result.purged_nodes {
                state
                    .admin_realtime()
                    .publish(AdminResource::Node, Some(node.node_id.clone()));
            }
            Json(agent_removal_response(*result)).into_response()
        }
        Ok(crate::agent_removal::AgentRemovalOutcome::NotFound) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent not found",
        ),
        Ok(crate::agent_removal::AgentRemovalOutcome::PendingTransfer(_)) => mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "pending_transfer",
            "complete, cancel, or let the pending Transfer expire before removing the Agent",
        ),
        Ok(crate::agent_removal::AgentRemovalOutcome::OwnershipChanged { .. }) => mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "ownership_changed",
            "the Agent's owned Nodes changed; refetch and confirm again",
        ),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}
