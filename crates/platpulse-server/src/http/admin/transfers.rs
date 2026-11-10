//! Node Transfers.
use super::*;
use crate::http::realtime::AdminResource;

/// Owner-authorized two-phase Node Transfer (design §4.6, issue #46). The
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
pub(super) struct NodeTransferRow {
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

pub(super) fn node_transfer_dto(row: NodeTransferRow) -> NodeTransfer {
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

pub(super) const TRANSFER_COLUMNS: &str = "transfer_id, node_id, source_agent_id, target_agent_id, status, operator_reason, created_at, expires_at, cancelled_at, completed_at, rejection_code, rejection_reason, mismatched_fields, updated_at";

/// Materialize expired pending Transfers of one Node (write + one Audit
/// event each) so the Admin timeline never shows a stale pending row.
async fn materialize_expired_node_transfers(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    node_id: &str,
    now_text: &str,
) -> Result<(), sqlx::Error> {
    let expired = sqlx::query_as::<_, (String,)>(
        "SELECT transfer_id FROM node_transfers WHERE node_id=$1 AND status='pending' AND expires_at <= $2",
    )
    .bind(node_id)
    .bind(now_text)
    .fetch_all(&mut **tx)
    .await?;
    for (transfer_id,) in expired {
        sqlx::query(
            "UPDATE node_transfers SET status='expired', updated_at=$1 WHERE transfer_id=$2 AND status='pending'",
        )
        .bind(now_text)
        .bind(&transfer_id)
        .execute(&mut **tx)
        .await?;
        crate::auth::insert_audit_event(
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
        .await?;
    }
    Ok(())
}

/// Materialize one expired pending Transfer (used by cancel/detail).
async fn materialize_expired_transfer(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    transfer_id: &str,
    now_text: &str,
) -> Result<Option<String>, sqlx::Error> {
    let expired = sqlx::query(
        "UPDATE node_transfers SET status='expired', updated_at=$1 WHERE transfer_id=$2 AND status='pending' AND expires_at <= $3",
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
        sqlx::query_scalar::<_, String>("SELECT node_id FROM node_transfers WHERE transfer_id=$1")
            .bind(transfer_id)
            .fetch_one(&mut **tx)
            .await?;
    crate::auth::insert_audit_event(
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
    .await?;
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
pub(super) async fn admin_node_transfers(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    let mut tx = match crate::database::begin(state.db().pool()).await {
        Ok(tx) => tx,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let known: Option<i64> = match sqlx::query_scalar("SELECT 1 FROM nodes WHERE node_id=$1")
        .bind(&node_id)
        .fetch_optional(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
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
    if let Err(error) = materialize_expired_node_transfers(&mut tx, &node_id, &now_text).await {
        return storage_failure(&state, &request_id.0, &error);
    }
    let rows = sqlx::query_as::<_, NodeTransferRow>(&format!(
        "SELECT {TRANSFER_COLUMNS} FROM node_transfers WHERE node_id=$1 ORDER BY created_at DESC"
    ))
    .bind(&node_id)
    .fetch_all(&mut *tx)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    if let Err(error) = tx.commit().await {
        return storage_failure(&state, &request_id.0, &error);
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
    // The owner this Transfer is created from must still own the Node when it
    // commits; a Report admitting or completing a Transfer for the same Node
    // writes ownership under the same fence (issue #280).
    let _node_fence = state.write_fences().nodes.hold(&node_id).await;
    let mut tx = match crate::database::begin(state.db().pool()).await {
        Ok(tx) => tx,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let owner: Option<String> =
        match sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(&node_id)
            .fetch_optional(&mut *tx)
            .await
        {
            Ok(value) => value,
            Err(error) => return storage_failure(&state, &request_id.0, &error),
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
    // A removed Agent is not a selectable Transfer target: keeping the live
    // predicate here stops a Node from being pointed back at an identity the
    // Owner permanently deleted (design §15.2, issue #171).
    let target_known: Option<i64> =
        match sqlx::query_scalar("SELECT 1 FROM agents WHERE agent_id=$1 AND deleted_at IS NULL")
            .bind(&body.target_agent_id)
            .fetch_optional(&mut *tx)
            .await
        {
            Ok(value) => value,
            Err(error) => return storage_failure(&state, &request_id.0, &error),
        };
    if target_known.is_none() {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_target_agent",
            "the target Agent is not registered",
        );
    }
    if let Err(error) = materialize_expired_node_transfers(&mut tx, &node_id, &now_text).await {
        return storage_failure(&state, &request_id.0, &error);
    }
    let pending: Option<String> = match sqlx::query_scalar(
        "SELECT transfer_id FROM node_transfers WHERE node_id=$1 AND status='pending'",
    )
    .bind(&node_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(value) => value,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    if pending.is_some() {
        // The conflict is a typed, auditable outcome: the attempt is
        // retained in history, ownership is untouched, and the response is
        // the standard error envelope (request ID included).
        let transfer_id = uuid::Uuid::new_v4().to_string();
        if let Err(error) = sqlx::query("INSERT INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, operator_reason, created_at, expires_at, updated_at) VALUES ($1, $2, $3, $4, 'conflict', $5, $6, $7, $8)")
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
        {
            return storage_failure(&state, &request_id.0, &error);
        }
        let audit_event_id = match crate::auth::insert_audit_event(
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
        {
            Ok(value) => value,
            Err(error) => return storage_failure(&state, &request_id.0, &error),
        };
        if let Err(error) = tx.commit().await {
            return storage_failure(&state, &request_id.0, &error);
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
    if let Err(error) = sqlx::query("INSERT INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, operator_reason, created_at, expires_at, updated_at) VALUES ($1, $2, $3, $4, 'pending', $5, $6, $7, $8)")
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
    {
        return storage_failure(&state, &request_id.0, &error);
    }
    let audit_event_id = match crate::auth::insert_audit_event(
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
    {
        Ok(value) => value,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let row = match sqlx::query_as::<_, NodeTransferRow>(&format!(
        "SELECT {TRANSFER_COLUMNS} FROM node_transfers WHERE transfer_id=$1"
    ))
    .bind(&transfer_id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(row) => row,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    if let Err(error) = tx.commit().await {
        return storage_failure(&state, &request_id.0, &error);
    }
    state
        .admin_realtime()
        .publish(AdminResource::Node, Some(node_id));
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
pub(super) async fn admin_transfer_detail(
    State(state): State<AppState>,
    Path(transfer_id): Path<String>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    let mut tx = match crate::database::begin(state.db().pool()).await {
        Ok(tx) => tx,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let now_text = crate::auth::format_rfc3339(crate::auth::now_utc());
    if let Err(error) = materialize_expired_transfer(&mut tx, &transfer_id, &now_text).await {
        return storage_failure(&state, &request_id.0, &error);
    }
    let row = sqlx::query_as::<_, NodeTransferRow>(&format!(
        "SELECT {TRANSFER_COLUMNS} FROM node_transfers WHERE transfer_id=$1"
    ))
    .bind(&transfer_id)
    .fetch_optional(&mut *tx)
    .await;
    let row = match row {
        Ok(row) => row,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let Some(row) = row else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    if let Err(error) = tx.commit().await {
        return storage_failure(&state, &request_id.0, &error);
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
    Extension(principal): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, false) {
        return response;
    }
    // A Report completing this Transfer and the Owner cancelling it decide
    // on the same pending row; both hold the Transfer's Node fence so exactly
    // one of them wins (issue #280). A Transfer never changes its Node, so the
    // Node ID read before the fence is the one the fence must cover.
    let transfer_node: Result<Option<String>, sqlx::Error> =
        sqlx::query_scalar("SELECT node_id FROM node_transfers WHERE transfer_id=$1")
            .bind(&transfer_id)
            .fetch_optional(state.db().pool())
            .await;
    let transfer_node = match transfer_node {
        Ok(node) => node,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let _node_fence = match &transfer_node {
        Some(node_id) => Some(state.write_fences().nodes.hold(node_id).await),
        None => None,
    };
    let mut tx = match crate::database::begin(state.db().pool()).await {
        Ok(tx) => tx,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let now_text = crate::auth::format_rfc3339(crate::auth::now_utc());
    if let Err(error) = materialize_expired_transfer(&mut tx, &transfer_id, &now_text).await {
        return storage_failure(&state, &request_id.0, &error);
    }
    let row = sqlx::query_as::<_, NodeTransferRow>(&format!(
        "SELECT {TRANSFER_COLUMNS} FROM node_transfers WHERE transfer_id=$1"
    ))
    .bind(&transfer_id)
    .fetch_optional(&mut *tx)
    .await;
    let row = match row {
        Ok(row) => row,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
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
    if let Err(error) = sqlx::query("UPDATE node_transfers SET status='cancelled', cancelled_at=$1, updated_at=$2 WHERE transfer_id=$3 AND status='pending'")
        .bind(&cancelled_at)
        .bind(&cancelled_at)
        .bind(&transfer_id)
        .execute(&mut *tx)
        .await
    {
        return storage_failure(&state, &request_id.0, &error);
    }
    let audit_event_id = match crate::auth::insert_audit_event(
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
    {
        Ok(value) => value,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let updated = NodeTransferRow {
        status: "cancelled".to_owned(),
        cancelled_at: Some(cancelled_at.clone()),
        updated_at: cancelled_at,
        ..row
    };
    if let Err(error) = tx.commit().await {
        return storage_failure(&state, &request_id.0, &error);
    }
    state
        .admin_realtime()
        .publish(AdminResource::Node, Some(updated.node_id.clone()));
    Json(NodeTransferMutationResponse {
        transfer: node_transfer_dto(updated),
        request_id: request_id.0.to_string(),
        audit_event_id,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::AuthenticatedSession;
    use crate::http::admin::test_helpers::*;
    use axum::body::to_bytes;
    use serde_json::Value;
    use tempfile::tempdir;
    use time::OffsetDateTime;

    /// State with an Owner, two Agents, a Network, and one Node owned by
    /// the source Agent — everything a Transfer workflow needs.
    async fn transfer_state() -> (tempfile::TempDir, AppState) {
        let dir = tempdir().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth);
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(state.db().pool()).await.unwrap();
        for agent in ["agent-source", "agent-target"] {
            sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ($1, 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
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
            "SELECT event_kind, after_json FROM audit_events WHERE audit_event_id=$1",
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
