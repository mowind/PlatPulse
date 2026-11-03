//! Fixtures shared by the Admin route tests.
use super::*;
use crate::http::AuthenticatedSession;
use tempfile::tempdir;
use time::OffsetDateTime;

/// Test state with an Owner user and one enrolled Agent row.
pub(super) async fn lifecycle_state() -> (tempfile::TempDir, AppState) {
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
    sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-lifecycle-test', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
        .execute(state.db().pool()).await.unwrap();
    // One already-issued credential, like a real enrolled Agent.
    sqlx::query("INSERT INTO agent_credentials (credential_id, agent_id, credential_digest, created_at, revoked_at, revoke_after) VALUES ('credential-initial', 'agent-lifecycle-test', $1, '2026-01-01T00:00:00Z', NULL, NULL)")
        .bind(vec![0u8])
        .execute(state.db().pool()).await.unwrap();
    (dir, state)
}

pub(super) fn lifecycle_session() -> AuthenticatedSession {
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

pub(super) fn mutation_headers(csrf: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/json; charset=utf-8".parse().unwrap(),
    );
    headers.insert(header::ORIGIN, "http://127.0.0.1:8080".parse().unwrap());
    headers.insert("x-csrf-token", csrf.parse().unwrap());
    headers
}

pub(super) fn request_id() -> crate::http::RequestId {
    crate::http::RequestId(std::sync::Arc::from("req-123"))
}

/// Seed helper: one registered Network plus two Nodes on one Agent.
/// `node_healthy` observes the registered identity; `node_mismatched`
/// observes a contradicting chain id (typed mismatch, history blocked).
pub(super) async fn node_inventory_state() -> (tempfile::TempDir, AppState) {
    let (dir, state) = lifecycle_state().await;
    let now = crate::auth::now_utc();
    let fixed = "2026-01-01T00:00:00Z".to_owned();
    let fresh = crate::auth::format_rfc3339(now);
    sqlx::query(
        "INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'PlatON Mainnet', '0x' || '01' || '00000000000000000000000000000000000000000000000000000000000000', 210425, 1, 'lat', $1, $2)",
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
            "INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, 'agent-lifecycle-test', 'mainnet', $2, 'ws://127.0.0.1:6790', $3, 'private', 1, $4, $5)",
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
        "INSERT INTO current_node_chain_observations (node_id, rpc_client_version, syncing, current_block, highest_block, consensus_epoch, consensus_validator, consensus_highest_commit_block, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, node_key_fingerprint, updated_at) VALUES ('node-healthy', 'platon/1.5.1', 0, 100, 100, 1, 1, 100, '0x' || '01' || '00000000000000000000000000000000000000000000000000000000000000', 210425, 1, 'lat', 'fp-healthy', $1)",
    )
    .bind(&fresh)
    .execute(state.db().pool())
    .await
    .unwrap();
    for component in ["rpc", "sync", "consensus", "network_identity"] {
        sqlx::query(
            "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision) VALUES ('agent-lifecycle-test', 'node', 'node-healthy', 'node-healthy', $1, 'ok', $2, $3, $4, 1, 1)",
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
        "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, value_received_at, state_revision, value_revision) VALUES ('agent-lifecycle-test', 'node', 'node-healthy', 'node-healthy', 'peers', 'ok', $1, $2, $3, $4, 1, 1)",
    )
    .bind(&fresh)
    .bind(&fresh)
    .bind(&fresh)
    .bind(&fresh)
    .execute(state.db().pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, client_name, cbft_protocol_version, cbft_highest_qc_block, cbft_locked_block, cbft_commit_block, updated_at) VALUES ('node-healthy', 'peer-healthy', '203.0.113.7', 'inbound', 1, 0, 1, 'PlatON/v1.5.1', 1, 100, 99, 98, $1)",
    )
    .bind(&fresh)
    .execute(state.db().pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO current_node_peer_capabilities (node_id, peer_id, capability, updated_at) VALUES ('node-healthy', 'peer-healthy', 'cbft/1', $1)",
    )
    .bind(&fresh)
    .execute(state.db().pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO peer_presence_intervals (node_id, peer_id, opened_at, closed_at, direction, trusted, static_peer, consensus_peer, client_name) VALUES ('node-healthy', 'peer-healthy', $1, $2, 'inbound', 1, 0, 1, 'PlatON/v1.5.1')",
    )
    .bind(&fixed)
    .bind(&fresh)
    .execute(state.db().pool())
    .await
    .unwrap();
    // Mismatched Node: RPC error with a last-good sync value retained
    sqlx::query(
        "INSERT INTO current_node_chain_observations (node_id, rpc_client_version, syncing, current_block, highest_block, consensus_epoch, consensus_validator, consensus_highest_commit_block, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, updated_at) VALUES ('node-mismatched', 'platon/1.5.1', 0, 99, 99, 1, 0, 99, '0x' || '01' || '00000000000000000000000000000000000000000000000000000000000000', 999999, 1, 'lat', $1)",
    )
    .bind(&fresh)
    .execute(state.db().pool())
    .await
    .unwrap();
    for component in ["rpc", "sync", "consensus", "network_identity"] {
        let (component_state, error_code, error_message) = if component == "rpc" {
            ("error", "rpc_unreachable", "RPC probe failed")
        } else {
            ("ok", "", "")
        };
        sqlx::query(
            "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision, error_code, error_message) VALUES ('agent-lifecycle-test', 'node', 'node-mismatched', 'node-mismatched', $1, $2, $3, $4, $5, 2, 1, $6, $7)",
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
