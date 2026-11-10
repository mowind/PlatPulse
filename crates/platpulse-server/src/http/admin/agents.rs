//! Agent detail, Agent Attention acknowledgment, and Agent metadata.
use super::*;
use crate::http::realtime::AdminResource;

/// Owner-only Agent detail: one full AgentDiagnostic (identity, liveness,
/// boot/report state, credential state, Inventory, diagnostics).
#[utoipa::path(
    get,
    path = "/api/admin/v1/agents/{agent_id}",
    tag = "admin",
    params(("agent_id" = String, Path, description = "Agent ID")),
    responses((status = 200, body = AgentDiagnostic), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_agent_detail(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    let row = match sqlx::query_as::<_, AgentAdminRow>(
        "SELECT agent_id, display_name, notes, agent_epoch, active_boot_id, active_boot_status, previous_boot_id, close_report_id, shutdown_state, shutdown_started_at, shutdown_deadline_at, shutdown_finished_at, shutdown_unresolved_from, shutdown_unresolved_to, shutdown_last_error, shutdown_forced, shutdown_report_id, shutdown_report_sequence, shutdown_updated_at, last_report_sequence, agent_capabilities_json, clock_skew_ms, clock_status, last_received_at, security_event_count, last_inventory_revision AS accepted_inventory_revision, inventory_sha256 AS accepted_inventory_sha256, inventory_protocol_major AS accepted_inventory_protocol_major FROM agents WHERE agent_id = $1 AND deleted_at IS NULL",
    )
    .bind(&agent_id)
    .fetch_optional(state.db().pool())
    .await
    {
        Ok(row) => row,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let Some(row) = row else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent not found",
        );
    };
    match agent_diagnostic(&state, row).await {
        Ok(diagnostic) => Json(diagnostic).into_response(),
        Err(error) => storage_failure(&state, &request_id.0, &error),
    }
}

/// One Owner acknowledgment request: the exact Agent Attention Items and
/// evidence boundaries the Owner saw, never a bare kind or a "hide forever"
/// switch (design §15.6, webui.md §15.3).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentAttentionAcknowledgmentRequest {
    pub items: Vec<crate::attention::AttentionAcknowledgment>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentAttentionAcknowledgmentResponse {
    pub agent_id: String,
    /// Authoritative unacknowledged Agent Attention Items *after* the
    /// mutation; the WebUI refetches rather than hiding the row locally.
    pub attention: Vec<AttentionItem>,
    /// Boundaries applied to the current occurrence.
    pub acknowledged: Vec<crate::attention::AttentionAcknowledgment>,
    /// Boundaries skipped because newer evidence superseded them.
    pub skipped: Vec<crate::attention::AttentionAcknowledgment>,
}

/// Owner-only shared Agent Attention Acknowledgment (design §15.6). Applies
/// the Owner's confirmation to exactly the evidence boundaries they saw,
/// writes an Audit Event, and returns the authoritative unacknowledged queue.
/// A boundary that changed since the Owner's snapshot is skipped, so a stale
/// request can never swallow newer unseen evidence, and the underlying
/// liveness, health, Alert Incidents, and notifications are untouched.
#[utoipa::path(
    post,
    path = "/api/admin/v1/agents/{agent_id}/attention/acknowledgments",
    tag = "admin",
    params(("agent_id" = String, Path, description = "Agent ID")),
    request_body = AgentAttentionAcknowledgmentRequest,
    responses((status = 200, body = AgentAttentionAcknowledgmentResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn acknowledge_agent_attention(
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
    let body: AgentAttentionAcknowledgmentRequest = match serde_json::from_slice(&body) {
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
    match crate::attention::acknowledge(
        state.db().pool(),
        &agent_id,
        &body.items,
        Some(&principal.0.user_id),
    )
    .await
    {
        Ok(crate::attention::AcknowledgeOutcome::Applied {
            acknowledged,
            skipped,
        }) => {
            let attention = sqlx::query_as::<_, crate::attention::AgentAttentionRow>(&format!(
                "{} AND a.agent_id = $1",
                crate::attention::AGENT_ATTENTION_SELECT
            ))
            .bind(&agent_id)
            .fetch_optional(state.db().pool())
            .await;
            let attention = match attention {
                Ok(Some(row)) => {
                    match crate::attention::evaluate_agent(state.db().pool(), &row).await {
                        Ok(attention) => attention,
                        Err(error) => {
                            return storage_failure(&state, &request_id.0, &error);
                        }
                    }
                }
                Ok(None) => Vec::new(),
                Err(error) => return storage_failure(&state, &request_id.0, &error),
            };
            // Presentation changed for every Owner and surface: invalidate the
            // Agent so open Admin sessions refetch the authoritative queue.
            state
                .admin_realtime()
                .publish(AdminResource::Agent, Some(agent_id.clone()));
            Json(AgentAttentionAcknowledgmentResponse {
                agent_id,
                attention,
                acknowledged,
                skipped,
            })
            .into_response()
        }
        Ok(crate::attention::AcknowledgeOutcome::AgentNotFound) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent not found",
        ),
        Err(error) => storage_failure(&state, &request_id.0, &error),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentMetadataRequest {
    /// Server-owned display name. Absent, `null`, or an empty value clears
    /// the name; the stable Agent ID remains the identifier (design §15.2).
    #[serde(default)]
    pub display_name: Option<String>,
    /// Free-form Owner notes. Absent, `null`, or an empty value clears them.
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentMetadataResponse {
    pub agent_id: String,
    pub display_name: Option<String>,
    pub notes: Option<String>,
}

/// Owner-only Server-owned Agent metadata mutation (display name and notes,
/// design §15.2). It never touches the Agent ID, Epoch, Host identity,
/// receipt-derived liveness, or local collection configuration.
#[utoipa::path(
    put,
    path = "/api/admin/v1/agents/{agent_id}/metadata",
    tag = "admin",
    params(("agent_id" = String, Path, description = "Agent ID")),
    request_body = AgentMetadataRequest,
    responses((status = 200, body = AgentMetadataResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn set_agent_metadata(
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
    let body: AgentMetadataRequest = match serde_json::from_slice(&body) {
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
    match crate::enrollment::update_agent_metadata(
        state.db(),
        Some(&principal.0.user_id),
        &agent_id,
        body.display_name.as_deref(),
        body.notes.as_deref(),
    )
    .await
    {
        Ok(profile) => {
            // Post-commit Admin invalidation: another Owner's open view
            // refetches the authoritative projection; never optimistic.
            state
                .admin_realtime()
                .publish(AdminResource::Agent, Some(agent_id.clone()));
            Json(AgentMetadataResponse {
                agent_id,
                display_name: profile.display_name,
                notes: profile.notes,
            })
            .into_response()
        }
        Err(crate::enrollment::AgentMetadataError::AgentNotFound) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent not found",
        ),
        Err(crate::enrollment::AgentMetadataError::InvalidDisplayName(message)) => mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_display_name",
            message,
        ),
        Err(crate::enrollment::AgentMetadataError::InvalidNotes(message)) => mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_notes",
            message,
        ),
        Err(crate::enrollment::AgentMetadataError::Database(error)) => {
            storage_failure(&state, &request_id.0, &error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::admin::test_helpers::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    /// Insert one stored Report Receipt directly for `agent-lifecycle-test`.
    /// The Agent detail only reads the Server-owned evidence columns, so an
    /// opaque `receipt_body` is enough.
    #[allow(clippy::too_many_arguments)]
    async fn insert_report_receipt(
        state: &AppState,
        report_id: &str,
        report_sequence: i64,
        disposition: &str,
        rejection_code: Option<&str>,
        inventory_revision: Option<i64>,
        inventory_sha256: Option<&str>,
        inventory_protocol_major: Option<i64>,
        received_at: &str,
    ) {
        sqlx::query(
            "INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at, rejection_code, inventory_revision, inventory_sha256, inventory_protocol_major) VALUES ($1, 'agent-lifecycle-test', 1, 'boot-inventory-test', $2, 'body-hash', $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(report_id)
        .bind(report_sequence)
        .bind(disposition)
        .bind(b"{}".as_slice())
        .bind(received_at)
        .bind(rejection_code)
        .bind(inventory_revision)
        .bind(inventory_sha256)
        .bind(inventory_protocol_major)
        .execute(state.db().pool())
        .await
        .unwrap();
    }

    async fn lifecycle_agent_detail(state: &AppState) -> Value {
        let response = admin_agent_detail(
            State(state.clone()),
            Path("agent-lifecycle-test".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn agent_detail_exposes_newest_inventory_rejection_evidence() {
        let (_dir, state) = lifecycle_state().await;
        sqlx::query("UPDATE agents SET last_inventory_revision=6, inventory_sha256='accepted-hash', inventory_protocol_major=1 WHERE agent_id='agent-lifecycle-test'")
            .execute(state.db().pool())
            .await
            .unwrap();

        // An Inventory rejection is the newest receipt: the Server-accepted
        // fact and the refused declaration are both visible.
        insert_report_receipt(
            &state,
            "receipt-rejected-1",
            1,
            "rejected",
            Some("inventory_revision_conflict"),
            Some(9),
            Some("refused-hash"),
            Some(1),
            "2026-08-12T08:00:00Z",
        )
        .await;
        let rejected = lifecycle_agent_detail(&state).await;
        assert_eq!(rejected["inventory"]["accepted_revision"], 6);
        assert_eq!(rejected["inventory"]["accepted_sha256"], "accepted-hash");
        assert_eq!(
            rejected["inventory"]["accepted_declaration"],
            "agent_declared"
        );
        assert_eq!(
            rejected["inventory"]["last_rejection"]["code"],
            "inventory_revision_conflict"
        );
        assert_eq!(
            rejected["inventory"]["last_rejection"]["declaration"],
            "agent_declared"
        );
        assert_eq!(
            rejected["inventory"]["last_rejection"]["reported_revision"],
            9
        );
        assert_eq!(
            rejected["inventory"]["last_rejection"]["reported_sha256"],
            "refused-hash"
        );
        assert_eq!(
            rejected["inventory"]["last_rejection"]["received_at"],
            "2026-08-12T08:00:00Z"
        );
        // The same newest receipt drives the Agent Attention Item.
        assert!(
            rejected["attention"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| {
                    item["kind"] == "agent_inventory_rejected" && item["severity"] == "critical"
                })
        );

        // Same instant but higher report_sequence: report_sequence is the
        // tie-break, so an accepted attempt clears the rejection evidence.
        insert_report_receipt(
            &state,
            "receipt-accepted-2",
            2,
            "accepted",
            None,
            None,
            None,
            Some(1),
            "2026-08-12T08:00:00Z",
        )
        .await;
        let accepted = lifecycle_agent_detail(&state).await;
        assert!(
            accepted["inventory"]["last_rejection"].is_null(),
            "an accepted newest receipt clears the rejection evidence"
        );

        // A later rejection (newer received_at) wins over report_sequence.
        insert_report_receipt(
            &state,
            "receipt-rejected-3",
            3,
            "rejected",
            Some("network_key_unknown"),
            Some(9),
            Some("refused-hash-2"),
            Some(1),
            "2026-08-12T09:00:00Z",
        )
        .await;
        let latest = lifecycle_agent_detail(&state).await;
        assert_eq!(
            latest["inventory"]["last_rejection"]["code"],
            "network_key_unknown"
        );
        assert_eq!(
            latest["inventory"]["last_rejection"]["reported_sha256"],
            "refused-hash-2"
        );
    }

    /// Issue #188: the Server-owned declaration protocol explains a v2
    /// (Server-managed) diagnosis from the accepted pair and the refusal
    /// evidence, without fabricating the Agent revision v2 removed; an Agent
    /// with no accepted declaration is Unknown, never revision 0.
    #[tokio::test]
    async fn agent_detail_explains_server_managed_inventory_evidence() {
        let (_dir, state) = lifecycle_state().await;

        // Never accepted: no revision exists, and no evidence is invented.
        let never = lifecycle_agent_detail(&state).await;
        assert!(never["inventory"]["accepted_revision"].is_null());
        assert!(never["inventory"]["accepted_sha256"].is_null());
        assert_eq!(never["inventory"]["accepted_declaration"], "unknown");
        assert!(never["inventory"]["last_rejection"].is_null());

        // A v2 declaration was accepted with a Server-assigned revision and
        // canonical fingerprint; the newest attempt was refused.
        sqlx::query("UPDATE agents SET last_inventory_revision=3, inventory_sha256='canonical-fingerprint', inventory_protocol_major=2 WHERE agent_id='agent-lifecycle-test'")
            .execute(state.db().pool())
            .await
            .unwrap();
        insert_report_receipt(
            &state,
            "receipt-v2-rejected",
            1,
            "rejected",
            Some("inventory_revision_conflict"),
            None,
            Some("refused-fingerprint"),
            Some(2),
            "2026-08-12T08:00:00Z",
        )
        .await;
        let rejected = lifecycle_agent_detail(&state).await;
        assert_eq!(rejected["inventory"]["accepted_revision"], 3);
        assert_eq!(
            rejected["inventory"]["accepted_sha256"],
            "canonical-fingerprint"
        );
        assert_eq!(
            rejected["inventory"]["accepted_declaration"],
            "server_managed"
        );
        assert_eq!(
            rejected["inventory"]["last_rejection"]["declaration"],
            "server_managed"
        );
        assert!(
            rejected["inventory"]["last_rejection"]["reported_revision"].is_null(),
            "a v2 refusal must not fabricate an Agent-supplied revision"
        );
        assert_eq!(
            rejected["inventory"]["last_rejection"]["reported_sha256"],
            "refused-fingerprint"
        );
        // The Attention message speaks to the Server-managed remedy, never to
        // bumping a revision the Agent no longer supplies.
        let message = rejected["attention"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["kind"] == "agent_inventory_rejected")
            .map(|item| item["message"].as_str().unwrap().to_owned())
            .expect("the Server-managed refusal is an Attention Item");
        assert!(
            message.contains("Server-assigned") && message.contains("exhausted"),
            "unexpected v2 message: {message}"
        );
        assert!(!message.contains("bump inventory_revision"));
    }
}
