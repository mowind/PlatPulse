//! Network Registry views and mutations.
use super::*;
use crate::http::realtime::{AdminResource, PublicResource};

/// Owner-only Network Registry projection (design §4.5). The complete
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
pub(super) async fn admin_networks(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    let rows = sqlx::query_as::<_, (String, String, String, i64, i64, String, String, String, i64, i64, i64)>(
        &format!("SELECT n.network_key, n.display_name, n.genesis_hash, n.chain_id, n.p2p_network_id, n.address_hrp, n.created_at, n.updated_at, {NETWORK_NODE_COUNTS} FROM networks n ORDER BY n.network_key"),
    )
    .fetch_all(state.db().pool())
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
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
    /// The Node Currency judged from the RPC, sync and consensus observations.
    pub currency: crate::node_currency::NodeCurrencyState,
    /// The oldest of the RPC, sync and consensus Server-clock observation
    /// instants; null while any of them was never observed.
    pub currency_as_of: Option<String>,
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
pub(super) async fn admin_network_detail(
    State(state): State<AppState>,
    Path(network_key): Path<String>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64, String, String, String, i64, i64, i64)>(
        &format!("SELECT n.network_key, n.display_name, n.genesis_hash, n.chain_id, n.p2p_network_id, n.address_hrp, n.created_at, n.updated_at, {NETWORK_NODE_COUNTS} FROM networks n WHERE n.network_key = $1"),
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
        Err(error) => return storage_failure(&state, &request_id.0, &error),
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
        "SELECT node_id, agent_id, display_name, lifecycle, visibility FROM nodes WHERE network_key = $1 ORDER BY COALESCE(display_name, node_id)",
    )
    .bind(&network_key)
    .fetch_all(state.db().pool())
    .await;
    let nodes = match nodes {
        Ok(nodes) => nodes,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let mut projected = Vec::with_capacity(nodes.len());
    for (node_id, agent_id, node_display_name, lifecycle, visibility) in nodes {
        let assessed = match node_diagnostic(
            &state,
            node_id.clone(),
            network_key.clone(),
            node_display_name.clone(),
            lifecycle.clone(),
            0,
            visibility.clone(),
        )
        .await
        {
            Ok(assessed) => assessed,
            Err(error) => return storage_failure(&state, &request_id.0, &error),
        };
        let diagnostic = assessed.diagnostic;
        projected.push(AdminNetworkNode {
            node_id: node_id.clone(),
            agent_id,
            display_name: node_display_name,
            lifecycle: lifecycle.clone(),
            visibility,
            health: diagnostic.health,
            health_reason: diagnostic.health_reason,
            currency: diagnostic.currency,
            currency_as_of: diagnostic.currency_as_of,
            current_head: diagnostic.current_head,
            resync_state: diagnostic.resync_state,
            identity: assessed.identity,
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
/// (design §4.5). The Registry is never created from observed Agent text:
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
    let mut tx = match crate::database::begin(state.db().pool()).await {
        Ok(tx) => tx,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let insert = sqlx::query(
        "INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
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
        return storage_failure(&state, &request_id.0, &error);
    }
    let audit = serde_json::json!({
        "network_key": key,
        "display_name": body.display_name,
        "genesis_hash": body.genesis_hash,
        "chain_id": body.chain_id,
        "p2p_network_id": body.p2p_network_id,
        "address_hrp": body.address_hrp,
    });
    if let Err(error) = crate::auth::insert_audit_event(
        &mut *tx,
        Some(&principal.0.user_id),
        "network_created",
        "network",
        &key,
        Some(&audit),
    )
    .await
    {
        return storage_failure(&state, &request_id.0, &error);
    }
    if let Err(error) = tx.commit().await {
        return storage_failure(&state, &request_id.0, &error);
    }
    state
        .admin_realtime()
        .publish(AdminResource::Network, Some(key.clone()));
    state
        .public_realtime()
        .publish(PublicResource::Network, Some(key.clone()));
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
    responses((status = 200, body = NetworkResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn update_network(
    State(state): State<AppState>,
    Path(network_key): Path<String>,
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
        "SELECT display_name, genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks WHERE network_key = $1",
    )
    .bind(&network_key)
    .fetch_optional(state.db().pool())
    .await;
    let Some((current_name, current_genesis, current_chain, current_p2p, current_hrp)) =
        (match existing {
            Ok(row) => row,
            Err(error) => return storage_failure(&state, &request_id.0, &error),
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
    let mut tx = match crate::database::begin(state.db().pool()).await {
        Ok(tx) => tx,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    if let Err(error) = sqlx::query(
        "UPDATE networks SET display_name = $1, genesis_hash = $2, chain_id = $3, p2p_network_id = $4, address_hrp = $5, updated_at = $6 WHERE network_key = $7",
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
    {
        return storage_failure(&state, &request_id.0, &error);
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
    if let Err(error) = crate::auth::insert_audit_change(
        &mut *tx,
        Some(&principal.0.user_id),
        "network_updated",
        "network",
        &network_key,
        Some(&before_audit),
        Some(&after_audit),
    )
    .await
    {
        return storage_failure(&state, &request_id.0, &error);
    }
    if let Err(error) = tx.commit().await {
        return storage_failure(&state, &request_id.0, &error);
    }
    state
        .admin_realtime()
        .publish(AdminResource::Network, Some(network_key.clone()));
    state
        .public_realtime()
        .publish(PublicResource::Network, Some(network_key.clone()));
    Json(NetworkResponse {
        network_key,
        display_name: next_name,
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
}
