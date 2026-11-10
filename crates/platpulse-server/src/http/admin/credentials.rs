//! Agent credential lifecycle: Enrollment and Recovery Tokens, rotation, revocation, and the Agent audit trail.
use super::*;

/// One-time Enrollment Token issued to an Owner (design §4.3, §15.2). The
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
    Extension(principal): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
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
        Err(error) => storage_failure(&state, &request_id.0, &error),
    }
}

/// One-time Recovery Token issued for an existing Agent. Exchanging it
/// advances the Agent Epoch and rotates the credential without creating a
/// duplicate Agent (design §4.3).
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
/// loss, design §4.3). The response is the Owner's only plaintext copy.
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
    Extension(principal): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
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
    let epoch: Option<i64> = match sqlx::query_scalar(
        "SELECT agent_epoch FROM agents WHERE agent_id = $1 AND deleted_at IS NULL",
    )
    .bind(&agent_id)
    .fetch_optional(state.db().pool())
    .await
    {
        Ok(epoch) => epoch,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
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
        Err(crate::enrollment::RecoveryError::Database(error)) => {
            storage_failure(&state, &request_id.0, &error)
        }
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
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
/// or was revoked immediately (design §10.1).
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
    Extension(principal): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
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
        Err(crate::enrollment::RotationError::Database(error)) => {
            storage_failure(&state, &request_id.0, &error)
        }
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
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

/// Revoke one Agent credential immediately (design §10.1: revoke takes
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
    Extension(principal): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
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
        Err(crate::enrollment::RevokeError::Database(error)) => {
            storage_failure(&state, &request_id.0, &error)
        }
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

/// Redacted Audit trail scoped to one Agent (design §13.3: security
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
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    match sqlx::query_scalar::<_, String>(
        "SELECT agent_id FROM agents WHERE agent_id = $1 AND deleted_at IS NULL",
    )
    .bind(&agent_id)
    .fetch_optional(state.db().pool())
    .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return mutation_error(
                &request_id.0,
                StatusCode::NOT_FOUND,
                "agent_not_found",
                "agent not found",
            );
        }
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    }
    let rows = sqlx::query_as::<_, (i64, String, Option<String>, String, Option<String>)>(
        "SELECT a.audit_event_id, a.event_kind, u.username, a.created_at, a.after_json
         FROM audit_events a LEFT JOIN users u ON u.user_id = a.actor_user_id
         WHERE a.target_kind = 'agent' AND a.target_id = $1
         ORDER BY a.created_at DESC, a.audit_event_id DESC LIMIT 50",
    )
    .bind(&agent_id)
    .fetch_all(state.db().pool())
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::admin::test_helpers::*;
    use axum::body::to_bytes;
    use serde_json::Value;

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

    #[tokio::test]
    async fn admin_agent_audit_reports_storage_failures_as_unavailable() {
        let (_dir, state) = lifecycle_state().await;

        sqlx::query("ALTER TABLE agents RENAME TO agents_unreadable")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = admin_agent_audit(
            State(state.clone()),
            Path("agent-lifecycle-test".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["error"]["code"], "unavailable", "{value}");

        sqlx::query("ALTER TABLE agents_unreadable RENAME TO agents")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("DROP TABLE audit_events")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = admin_agent_audit(
            State(state.clone()),
            Path("agent-lifecycle-test".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["error"]["code"], "unavailable", "{value}");
    }

    // ---- Node inventory, metadata, and Network Registry (issue #45) ----
}
