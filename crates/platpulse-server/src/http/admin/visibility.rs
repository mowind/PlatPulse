//! Node visibility and Node metadata mutations.
use super::*;
use crate::http::realtime::{AdminResource, PublicResource};

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
    Extension(principal): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
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
    let mut tx = match crate::database::begin(state.db().pool()).await {
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
    let before = sqlx::query_scalar::<_, String>("SELECT visibility FROM nodes WHERE node_id = $1")
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
    if sqlx::query("UPDATE nodes SET visibility = $1, updated_at = $2 WHERE node_id = $3")
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
    // Visibility is Admin-only metadata: the Public Projection filters on
    // lifecycle alone, so no Home reader has anything to refetch.
    state
        .admin_realtime()
        .publish(AdminResource::Node, Some(node_id.clone()));
    Json(VisibilityResponse {
        node_id,
        visibility: body.visibility,
    })
    .into_response()
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeMetadataRequest {
    /// Server-owned display name (design §4.4). Required: 1..=128 visible
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
    let mut tx = match crate::database::begin(state.db().pool()).await {
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
    let before = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT display_name, network_key FROM nodes WHERE node_id = $1",
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
    if previous.as_deref() == Some(body.display_name.as_str()) {
        return Json(NodeMetadataResponse {
            node_id: node_id.clone(),
            display_name: body.display_name,
        })
        .into_response();
    }
    let changed_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    if sqlx::query("UPDATE nodes SET display_name = $1, updated_at = $2 WHERE node_id = $3")
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
    state
        .admin_realtime()
        .publish(AdminResource::Node, Some(node_id.clone()));
    state
        .public_realtime()
        .publish(PublicResource::Node, Some(node_id.clone()));
    state
        .public_realtime()
        .publish(PublicResource::Network, Some(network_key));
    Json(NodeMetadataResponse {
        node_id,
        display_name: body.display_name,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::AuthenticatedSession;
    use crate::http::admin::test_helpers::*;
    use axum::body::to_bytes;
    use tempfile::tempdir;
    use time::OffsetDateTime;

    #[tokio::test]
    async fn visibility_mutation_accepts_json_parameters_and_updates_node() {
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
        let public_before = state.public_realtime().current_id();
        let admin_before = state.admin_realtime().current_id();
        let response = set_visibility(
            State(state.clone()),
            Path("node-visibility-test".to_owned()),
            headers.clone(),
            Extension(AuthenticatedSession(session.clone())),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
            axum::body::Bytes::from_static(br#"{"visibility":"public"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        // Home never reads visibility, so only the Admin view is invalidated.
        assert_eq!(state.public_realtime().current_id(), public_before);
        assert!(state.admin_realtime().current_id() > admin_before);
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

        let admin_before = state.admin_realtime().current_id();
        let response = set_visibility(
            State(state.clone()),
            Path("node-visibility-test".to_owned()),
            headers,
            Extension(AuthenticatedSession(session)),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
            axum::body::Bytes::from_static(br#"{"visibility":"private"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(state.public_realtime().current_id(), public_before);
        assert!(state.admin_realtime().current_id() > admin_before);
    }

    #[tokio::test]
    async fn visibility_mutation_rejects_security_failures_with_uniform_envelope_before_json() {
        let dir = tempdir().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
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
}
