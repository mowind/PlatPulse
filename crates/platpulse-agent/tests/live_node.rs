//! Live PlatON node integration checks, gated on `PLATPULSE_TEST_RPC_URL`.
//!
//! These tests exercise the real Alloy transport against a running PlatON
//! node, for example:
//!
//! ```text
//! PLATPULSE_TEST_RPC_URL=ws://127.0.0.1:6790 \
//!   cargo test -p platpulse-agent --test live_node
//! ```
//!
//! Without the variable the tests skip, so normal CI stays deterministic.

use platpulse_agent::block::{ResolvedBlock, WebSocketBlockTransport};
use platpulse_agent::collector::{ProbeValue, RpcAdapter};
use platpulse_agent::rpc::AlloyRpcAdapter;
use platpulse_core::network::RpcEndpoint;
use platpulse_core::time::Rfc3339;

fn live_endpoint() -> Option<RpcEndpoint> {
    std::env::var("PLATPULSE_TEST_RPC_URL")
        .ok()
        .and_then(|url| url.parse().ok())
}

fn node_id() -> platpulse_core::identity::NodeId {
    "0195f2a1-0014-4014-8014-000000000014".parse().unwrap()
}

fn observed_at() -> Rfc3339 {
    "2026-01-01T00:00:00Z".parse().unwrap()
}

#[test]
fn live_probe_observes_real_identity_sync_and_consensus() {
    let Some(endpoint) = live_endpoint() else {
        eprintln!("skipping: PLATPULSE_TEST_RPC_URL is not set");
        return;
    };
    let snapshot = AlloyRpcAdapter
        .collect(&endpoint)
        .expect("live probe failed");
    assert!(
        !snapshot.client_version.is_empty(),
        "client version must be observed"
    );
    assert!(
        !snapshot.namespaces.is_empty(),
        "rpc_modules must be observed"
    );
    assert!(
        !snapshot.methods.is_empty(),
        "capability probe must record methods"
    );
    assert!(
        snapshot
            .network_identity
            .genesis_hash
            .as_str()
            .starts_with("0x")
    );
    assert!(snapshot.node_key_fingerprint.as_str().starts_with("0x"));
    assert!(
        matches!(snapshot.sync, ProbeValue::Supported(_)),
        "eth_syncing must be supported on the live node"
    );
    assert!(
        matches!(snapshot.consensus, ProbeValue::Supported(_)),
        "debug_consensusStatus must be supported on the live node"
    );
    eprintln!(
        "live probe: client={:?} namespaces={:?} methods={:?} identity={:?} hrp={:?} enode={:?}",
        snapshot.client_version,
        snapshot.namespaces,
        snapshot.methods,
        snapshot.network_identity,
        snapshot.network_identity.address_hrp,
        snapshot
            .enode
            .as_deref()
            .map(|value| &value[..value.len().min(32)])
    );
}

#[tokio::test]
async fn live_transport_resolves_genesis_and_current_head_blocks() {
    let Some(endpoint) = live_endpoint() else {
        eprintln!("skipping: PLATPULSE_TEST_RPC_URL is not set");
        return;
    };
    let transport = WebSocketBlockTransport::default();
    // A node mid-sync may legitimately report head 0; the call itself must
    // succeed and the observed head must round-trip.
    let head = transport
        .current_head_async(&endpoint)
        .await
        .expect("eth_blockNumber failed on live node");
    let snapshot = AlloyRpcAdapter
        .collect(&endpoint)
        .expect("live probe failed");
    let genesis = transport
        .get_block_by_number_async(&endpoint, &snapshot.network_identity, 0)
        .await
        .expect("eth_getBlockByNumber(0) failed on live node");
    assert_eq!(genesis.block_number, 0);
    assert_eq!(genesis.network_identity, snapshot.network_identity);
    assert_eq!(genesis.block_hash, snapshot.network_identity.genesis_hash);
    // eth_* reports seconds; the transport must convert to Unix ms.
    assert!(
        genesis.block_timestamp_ms > 1_500_000_000_000,
        "genesis timestamp must be Unix ms (eth seconds -> ms), got {}",
        genesis.block_timestamp_ms
    );
    let by_hash = transport
        .get_block_by_hash_async(&endpoint, &genesis.block_hash, &snapshot.network_identity)
        .await
        .expect("eth_getBlockByHash failed on live node");
    assert_eq!(by_hash.block_number, 0);
    assert_eq!(by_hash.block_hash, genesis.block_hash);
    if head > 0 {
        let latest = transport
            .get_block_by_number_async(&endpoint, &snapshot.network_identity, head)
            .await
            .expect("eth_getBlockByNumber(head) failed on live node");
        assert_eq!(latest.block_number, head);
        assert_eq!(latest.network_identity, snapshot.network_identity);
    }
    let _: ResolvedBlock = by_hash;
}

#[tokio::test]
async fn live_transport_subscribes_and_resolves_new_heads() {
    let Some(endpoint) = live_endpoint() else {
        eprintln!("skipping: PLATPULSE_TEST_RPC_URL is not set");
        return;
    };
    let transport = WebSocketBlockTransport {
        receive_timeout: std::time::Duration::from_secs(3),
        ..WebSocketBlockTransport::default()
    };
    let snapshot = AlloyRpcAdapter
        .collect(&endpoint)
        .expect("live probe failed");
    // The subscription round-trip must succeed even when the chain is idle
    // (e.g. a node still catching up); new heads are asserted only when they
    // actually arrive.
    let summaries = transport
        .collect_node_summaries(
            &endpoint,
            node_id(),
            snapshot.network_identity.clone(),
            observed_at(),
        )
        .await
        .expect("live subscription failed");
    for summary in &summaries {
        assert_eq!(summary.node_id, node_id());
        assert_eq!(summary.network_identity, snapshot.network_identity);
    }
    eprintln!("live subscription resolved {} head(s)", summaries.len());
}
