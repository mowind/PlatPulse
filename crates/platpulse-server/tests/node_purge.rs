//! Node Purge acceptance through the real router (issue #167, part of #165).
//!
//! Node Purge is an explicit, irreversible Owner disposition. These tests
//! build the full application with build_app against a temporary SQLite
//! database, mint a real Agent credential through the Admin API, submit a real
//! report, and then exercise the preview and mutation over HTTP. They assert
//! the durable outcome: the Node and its owned observation/history/Links are
//! gone, shared Agent/Host/Network/Validator data and Incident/Audit evidence
//! remain, the minimal deletion identity is persisted, and the public detail
//! URL becomes non-leakingly unavailable.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_core::block::{BlockProductionAttribution, BlockSource, BlockSummary};
use platpulse_core::{
    AgentReport, InventoryDisposition, NodeCurrentDisposition, ReceiptDisposition, RejectionCode,
    ReportReceipt, SampleDispositionKind,
};
use platpulse_server::{AppState, auth, database, http, network, secrets};

const NETWORK_KEY: &str = "platon-mainnet";
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const VIEWER_LOGIN_BODY: &str = r#"{"username":"viewer","password":"viewer password"}"#;
/// A second Node ID the report can declare next to the fixture's Node.
const SECOND_NODE_ID: &str = "0195f2a1-0015-4015-8015-000000000015";

/// A fresh Server (real temp SQLite + pepper), the full router, and the pieces
/// needed to authenticate as the Owner.
struct Harness {
    dir: TempDir,
    state: AppState,
    app: Router,
}

impl Harness {
    async fn boot() -> Self {
        let dir = TempDir::new().unwrap();
        Self::seed(dir).await
    }

    /// A first boot: create the Owner/Viewer accounts and the registered
    /// Network, then build the router.
    async fn seed(dir: TempDir) -> Self {
        let harness = Self::open(dir).await;
        let owner_hash = auth::hash_password(b"correct horse battery").unwrap();
        auth::create_owner(harness.state.db(), "admin", &owner_hash)
            .await
            .unwrap();
        let viewer_hash = auth::hash_password(b"viewer password").unwrap();
        auth::create_viewer(harness.state.db(), "viewer", &viewer_hash)
            .await
            .unwrap();
        network::create_network(
            harness.state.db(),
            NETWORK_KEY,
            "PlatON Mainnet",
            NETWORK_GENESIS,
            210425,
            210425,
            "lat",
        )
        .await
        .unwrap();
        harness
    }

    /// Open (or re-open) the Server database in `dir` without seeding. The
    /// restart case drops the previous pool first so the exclusive SQLite lock
    /// is released before this second opener arrives.
    async fn open(dir: TempDir) -> Self {
        let database = database::initialize(database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pepper_path = dir.path().join("server-pepper");
        if !pepper_path.exists() {
            secrets::create_pepper_file(&pepper_path).unwrap();
        }
        let auth = auth::AuthConfig::development(
            secrets::load_pepper_file(&pepper_path).unwrap(),
            DEVELOPMENT_ORIGIN.to_owned(),
        );
        let state = AppState::new(database, None, auth);
        let app = http::build_app(state.clone());
        Self { dir, state, app }
    }

    /// Simulate a Server restart over the same durable database directory.
    async fn restart(self) -> Self {
        let Harness { dir, state, app } = self;
        drop(app);
        drop(state);
        Self::open(dir).await
    }

    fn pool(&self) -> &SqlitePool {
        self.state.db().pool()
    }

    async fn send(&self, request: Request<Body>) -> axum::response::Response {
        self.app.clone().oneshot(request).await.unwrap()
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

struct Session {
    cookie: String,
    csrf: String,
}

fn admin_get(uri: &str, session: &Session) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::COOKIE, &session.cookie)
        .body(Body::empty())
        .unwrap()
}

fn admin_post(uri: &str, session: &Session, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
        .header("x-csrf-token", &session.csrf)
        .header(header::COOKIE, &session.cookie)
        .body(Body::from(body.to_owned()))
        .unwrap()
}

fn bearer_post(uri: &str, token: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body))
        .unwrap()
}

async fn login(harness: &Harness, body: &str) -> Session {
    let request = Request::builder()
        .method("POST")
        .uri("/api/public/v1/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
        .body(Body::from(body.to_owned()))
        .unwrap();
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_owned();
    let value = body_json(response).await;
    Session {
        cookie,
        csrf: value["csrfToken"].as_str().unwrap().to_owned(),
    }
}

async fn enroll_agent(harness: &Harness, session: &Session) -> (String, String) {
    let response = harness
        .send(admin_post(
            "/api/admin/v1/agents/enroll-token",
            session,
            r#"{"expiresInHours": 24}"#,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let token = body_json(response).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let response = harness
        .send(bearer_post("/api/agent/v1/enroll", &token, Vec::new()))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let value = body_json(response).await;
    (
        value["agent_id"].as_str().unwrap().to_owned(),
        value["credential"].as_str().unwrap().to_owned(),
    )
}

fn fixture_report(agent_id: &str, agent_epoch: u64) -> AgentReport {
    let mut report: AgentReport = serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    report.agent_id = agent_id.parse().unwrap();
    report.agent_epoch = agent_epoch;
    report
}

async fn count_for_node(harness: &Harness, table: &str, node_id: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE node_id = ?");
    sqlx::query_scalar(&sql)
        .bind(node_id)
        .fetch_one(harness.pool())
        .await
        .unwrap()
}

async fn node_row_count(harness: &Harness, node_id: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE node_id = ?")
        .bind(node_id)
        .fetch_one(harness.pool())
        .await
        .unwrap()
}

async fn post_report(
    harness: &Harness,
    credential: &str,
    report: &AgentReport,
) -> (StatusCode, Value) {
    let response = harness
        .send(bearer_post(
            "/api/agent/v1/reports",
            credential,
            serde_json::to_vec(report).unwrap(),
        ))
        .await;
    let status = response.status();
    (status, body_json(response).await)
}

fn receipt_of(value: &Value) -> ReportReceipt {
    serde_json::from_value(value["receipt"].clone()).unwrap()
}

async fn purge_node(harness: &Harness, session: &Session, node_id: &str) {
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/nodes/{node_id}/purge"),
            session,
            &format!("{{\"confirmNodeId\":\"{node_id}\"}}"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// The fixture's single Node plus a second declared Node, with the declared
/// Network Identity aligned to the registered Network and one Block Summary
/// per Node. One legal report therefore exercises a purged entry next to a
/// still-valid sibling, including per-sample dispositions.
fn two_node_report(
    agent_id: &str,
    agent_epoch: u64,
    report_sequence: u64,
    report_id: &str,
    inventory_revision: u64,
    first_height: u64,
) -> AgentReport {
    let mut report = fixture_report(agent_id, agent_epoch);
    report.report_sequence = report_sequence;
    report.report_id = report_id.parse().unwrap();
    report.inventory.revision = inventory_revision;
    // The fixture ships a deliberately mismatching Network Identity; align it
    // so the sibling's Block Summary is genuinely admissible.
    for node in &mut report.nodes {
        let identity = node.chain.network_identity.latest.as_mut().unwrap();
        identity.genesis_hash = NETWORK_GENESIS.parse().unwrap();
        identity.address_hrp = Some("lat".to_owned());
    }
    let mut inventory_node = report.inventory.nodes[0].clone();
    inventory_node.node_id = SECOND_NODE_ID.parse().unwrap();
    report.inventory.nodes.push(inventory_node);
    let mut observation = report.nodes[0].clone();
    observation.node_id = SECOND_NODE_ID.parse().unwrap();
    report.nodes.push(observation);

    // One Block Summary per Node, paired by the shared Inventory/Observation
    // order instead of a positional index.
    let block_summaries: Vec<BlockSummary> = report
        .inventory
        .nodes
        .iter()
        .zip(report.nodes.iter())
        .enumerate()
        .map(|(index, (node, observation))| BlockSummary {
            node_id: node.node_id,
            network_identity: observation.chain.network_identity.latest.clone().unwrap(),
            block_number: first_height + index as u64,
            block_hash: format!("0x{:064x}", 0xaa + index).parse().unwrap(),
            parent_hash: format!("0x{:064x}", 0xbb + index).parse().unwrap(),
            block_timestamp_ms: 1_000,
            observed_at: report.generated_at,
            transaction_count: 3,
            block_interval_ms: None,
            source: BlockSource::Subscription,
            attribution: BlockProductionAttribution::unknown_attribution(
                "0x1111111111111111111111111111111111111111"
                    .parse()
                    .unwrap(),
                "test",
            ),
        })
        .collect();
    report.block_summaries.extend(block_summaries);
    report.validate().unwrap();
    report
}

/// Every Node-owned table a Purge is responsible for, with the direct inserts
/// that populate one row of each. Kept in the test so the acceptance fails if
/// a future Node-owned table is added without a purge path.
const NODE_OWNED_TABLES: &[&str] = &[
    "component_status",
    "current_node_process_observations",
    "current_node_data_directory_observations",
    "current_node_chain_observations",
    "current_node_rpc_namespaces",
    "current_node_rpc_methods",
    "current_node_peers",
    "current_node_peer_capabilities",
    "peer_presence_intervals",
    "peer_aggregate_5m",
    "peer_aggregate_5m_countries",
    "peer_aggregate_1h",
    "peer_aggregate_1h_countries",
    "block_summaries",
    "block_history_state",
    "block_coverage_intervals",
    "block_identity_window",
    "block_history_gaps",
    "chain_divergence_observations",
    "observed_network_heads",
    "node_metric_samples",
    "node_validator_links",
    "node_transfers",
];

/// Populate one row of every Node-owned table that the minimal report does not
/// already create, so the Foreign Key ordering of the delete is exercised for
/// real. The report itself creates component_status, chain, RPC namespaces and
/// methods for the Node.
async fn seed_directly_owned_rows(
    harness: &Harness,
    node_id: &str,
    agent_id: &str,
    validator_id: &str,
) {
    let pool = harness.pool();
    let now = "2026-01-01T00:00:00Z";

    sqlx::query("INSERT OR IGNORE INTO current_node_process_observations (node_id, pid, started_at, cpu_percent, memory_bytes, uptime_ms, updated_at) VALUES (?, 1, ?, 1.0, 1024, 10, ?)")
        .bind(node_id).bind(now).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO current_node_data_directory_observations (node_id, size_bytes, capacity_bytes, updated_at) VALUES (?, 10, 100, ?)")
        .bind(node_id).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, updated_at) VALUES (?, 'peer-1', NULL, 'outbound', 0, 0, 0, ?)")
        .bind(node_id).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO current_node_peer_capabilities (node_id, peer_id, capability, updated_at) VALUES (?, 'peer-1', 'platon', ?)")
        .bind(node_id).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO peer_presence_intervals (node_id, peer_id, direction, trusted, static_peer, consensus_peer, opened_at) VALUES (?, 'peer-1', 'outbound', 0, 0, 0, ?)")
        .bind(node_id).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO peer_aggregate_5m (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, first_observed_at, last_observed_at) VALUES (?, ?, 1, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, ?, ?)")
        .bind(node_id).bind(now).bind(now).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO peer_aggregate_5m_countries (node_id, bucket_start, country_code, peer_count) VALUES (?, ?, 'US', 1)")
        .bind(node_id).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO peer_aggregate_1h (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, first_observed_at, last_observed_at) VALUES (?, ?, 1, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, ?, ?)")
        .bind(node_id).bind(now).bind(now).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO peer_aggregate_1h_countries (node_id, bucket_start, country_code, peer_count) VALUES (?, ?, 'US', 1)")
        .bind(node_id).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, block_interval_ms, source, coinbase, seal_signer_key_fingerprint, seal_signer_match, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, accepted_at, node_key_history_complete) VALUES (?, 1, '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', '0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc', ?, 210425, 210425, 'lat', 1000, ?, 1, NULL, 'subscription', 'lat1coinbase', NULL, 'unknown', 'unknown', NULL, 'test', ?, 0)")
        .bind(node_id).bind(NETWORK_GENESIS).bind(now).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO block_history_state (node_id, updated_at) VALUES (?, ?)")
        .bind(node_id)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT OR IGNORE INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES (?, 1, 1, 'covered', ?, ?)")
        .bind(node_id).bind(now).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO block_identity_window (node_id, height, block_hash) VALUES (?, 1, '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb')")
        .bind(node_id).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO block_history_gaps (node_id, from_height, to_height, kind, created_at) VALUES (?, 2, 3, 'permanent', ?)")
        .bind(node_id).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO chain_divergence_observations (node_id, height, retained_block_hash, observed_block_hash, observed_at, reason) VALUES (?, 1, '0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd', '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', ?, 'test')")
        .bind(node_id).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO observed_network_heads (node_id, block_number, block_hash, observed_at, confidence, eligible_sources) VALUES (?, 1, '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', ?, 'high', '[\"subscription\"]')")
        .bind(node_id).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO node_metric_samples (node_id, metric, observed_at, received_at, value) VALUES (?, 'process_cpu_percent', ?, ?, 1.0)")
        .bind(node_id).bind(now).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO node_validator_links (link_id, node_id, validator_id, role, valid_from, created_at, updated_at) VALUES (?, ?, ?, 'observer', ?, ?, ?)")
        .bind(format!("link-{node_id}")).bind(node_id).bind(validator_id).bind(now).bind(now).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, created_at, expires_at, updated_at) VALUES (?, ?, ?, ?, 'cancelled', ?, ?, ?)")
        .bind(format!("transfer-{node_id}")).bind(node_id).bind(agent_id).bind(agent_id).bind(now).bind(now).bind(now).execute(pool).await.unwrap();
}

async fn seed_shared_data(harness: &Harness, agent_id: &str, node_id: &str) {
    let pool = harness.pool();
    let now = "2026-01-01T00:00:00Z";
    // Another Node owned by the same Agent, with its own Node-scoped estimate.
    sqlx::query("INSERT OR IGNORE INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('sibling-node', ?, ?, 'Sibling', 'ws://127.0.0.1:2', 'active', 'public', 1, ?, ?)")
        .bind(agent_id).bind(NETWORK_KEY).bind(now).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, state_revision, value_revision) VALUES (?, 'node', 'sibling-node', 'sibling-node', 'rpc', 'ok', 1, 1)")
        .bind(agent_id).execute(pool).await.unwrap();
    // An independent Validator identity and one durable daily snapshot.
    sqlx::query("INSERT OR IGNORE INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-1', ?, '0xvalidator', 'Validator One', ?, ?)")
        .bind(NETWORK_KEY).bind(now).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, source, observation_key) VALUES ('snapshot-1', 'validator-1', 'UTC', '2026-01-01', '2026-01', ?, ?, 'test', 'observation-1')")
        .bind(now).bind(now).execute(pool).await.unwrap();
    // A Network-scoped reference head that is not Node-owned.
    sqlx::query("INSERT OR IGNORE INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES (?, 1, ?, 'high', 1, NULL)")
        .bind(NETWORK_KEY).bind(now).execute(pool).await.unwrap();
    // Existing Alert Incident evidence for the Node subject.
    let rule_key: String = sqlx::query_scalar("SELECT rule_key FROM alert_rules LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT OR IGNORE INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, opened_evidence_json) VALUES ('incident-1', ?, 1, 'node', ?, 'warning', 'open', 1, ?, '{}')")
        .bind(rule_key).bind(node_id).bind(now).execute(pool).await.unwrap();
}

/// The canonical acceptance: preview, purge, and durable outcome.
#[tokio::test]
async fn owner_purge_removes_only_node_owned_data_and_leaves_shared_evidence() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let (agent_id, credential) = enroll_agent(&harness, &owner).await;
    let report = fixture_report(&agent_id, 1);
    let node_id = report.inventory.nodes[0].node_id.to_string();
    let response = harness
        .send(bearer_post(
            "/api/agent/v1/reports",
            &credential,
            serde_json::to_vec(&report).unwrap(),
        ))
        .await;
    let value = body_json(response).await;
    assert_eq!(value["receipt"]["disposition"], "accepted", "{value}");

    sqlx::query("UPDATE nodes SET visibility = 'public' WHERE node_id = ?")
        .bind(&node_id)
        .execute(harness.pool())
        .await
        .unwrap();
    seed_shared_data(&harness, &agent_id, &node_id).await;
    seed_directly_owned_rows(&harness, &node_id, &agent_id, "validator-1").await;

    // The public detail URL is readable before the purge.
    let before = harness
        .send(admin_get(
            &format!("/api/public/v1/nodes/{node_id}"),
            &owner,
        ))
        .await;
    assert_eq!(before.status(), StatusCode::OK);
    // Public Home statistics include the published Node before the purge.
    let home_before = body_json(
        harness
            .send(admin_get("/api/public/v1/networks", &owner))
            .await,
    )
    .await;
    assert!(
        home_before.to_string().contains(&node_id),
        "published Node is missing from Public Home before the purge"
    );

    // Preview reports the Node and a non-empty owned scope.
    let preview = harness
        .send(admin_get(
            &format!("/api/admin/v1/nodes/{node_id}/purge"),
            &owner,
        ))
        .await;
    assert_eq!(preview.status(), StatusCode::OK);
    let preview = body_json(preview).await;
    assert_eq!(preview["target"]["node_id"], node_id);
    assert_eq!(preview["target"]["agent_id"], agent_id);
    assert_eq!(preview["target"]["lifecycle"], "active");
    let preview_total = preview["counts"]["total_owned_rows"].as_i64().unwrap();
    assert!(preview_total > 0, "preview must show a non-empty scope");

    // Purge.
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/nodes/{node_id}/purge"),
            &owner,
            &format!(r#"{{"confirmNodeId":"{node_id}"}}"#),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["node_id"], node_id);
    assert_eq!(
        body["removed"]["total_owned_rows"].as_i64().unwrap(),
        preview_total,
        "the committed scope must equal the confirmed preview scope"
    );

    // Every Node-owned table is empty for the Node and the Node itself is gone.
    for table in NODE_OWNED_TABLES {
        assert_eq!(
            count_for_node(&harness, table, &node_id).await,
            0,
            "{table} still holds rows for the purged Node"
        );
    }
    let node_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE node_id = ?")
        .bind(&node_id)
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(node_rows, 0);

    // Minimal deletion identity and Audit Event are retained.
    let deleted: (String, String, String) = sqlx::query_as(
        "SELECT agent_id, network_key, deleted_at FROM deleted_nodes WHERE node_id = ?",
    )
    .bind(&node_id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(deleted.0, agent_id);
    assert_eq!(deleted.1, NETWORK_KEY);
    assert!(!deleted.2.is_empty());
    let audit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'node_purged' AND target_kind = 'node' AND target_id = ?",
    )
    .bind(&node_id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(audit, 1);

    // Shared Agent/Host, sibling Node, Network, Validator history, and Incident
    // evidence survive.
    let agents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE agent_id = ?")
        .bind(&agent_id)
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(agents, 1);
    let host: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM current_host_observations WHERE agent_id = ?")
            .bind(&agent_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(host, 1);
    assert_eq!(
        count_for_node(&harness, "component_status", "sibling-node").await,
        1
    );
    let validators: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM validators WHERE validator_id = 'validator-1'")
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(validators, 1);
    let snapshots: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM validator_daily_snapshots WHERE validator_id = 'validator-1'",
    )
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(snapshots, 1, "independent Validator history must survive");
    let reference: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM network_reference_heads WHERE network_key = ?")
            .bind(NETWORK_KEY)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(reference, 1);
    let incidents: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM alert_incidents WHERE incident_id = 'incident-1'")
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(incidents, 1, "existing Incident evidence must be retained");

    // The Node leaves Admin and Public current views, and the public detail URL
    // becomes a non-leaking unavailable outcome.
    let list = body_json(harness.send(admin_get("/api/admin/v1/nodes", &owner)).await).await;
    assert!(
        !list.to_string().contains(&node_id),
        "purged Node still appears in the Admin list"
    );
    let overview = body_json(
        harness
            .send(admin_get("/api/admin/v1/overview", &owner))
            .await,
    )
    .await;
    assert!(
        !overview.to_string().contains(&node_id),
        "purged Node still appears in Attention/Overview"
    );
    let public = harness
        .send(admin_get(
            &format!("/api/public/v1/nodes/{node_id}"),
            &owner,
        ))
        .await;
    assert_eq!(public.status(), StatusCode::NOT_FOUND);
    let public_body = body_json(public).await;
    assert_eq!(public_body["error"]["code"], "not_found");
    assert!(!public_body.to_string().contains(&node_id));
    // Public Home/statistics no longer contain the purged Node, while the
    // sibling Node on the same Network remains published.
    let home_after = body_json(
        harness
            .send(admin_get("/api/public/v1/networks", &owner))
            .await,
    )
    .await;
    assert!(
        !home_after.to_string().contains(&node_id),
        "purged Node still appears in Public Home/statistics"
    );
    assert!(
        home_after.to_string().contains("sibling-node"),
        "purge must not remove other Nodes from Public Home"
    );
}

/// A retried or raced mutation cannot delete an unseen scope.
#[tokio::test]
async fn purge_is_owner_only_csrf_checked_and_confirmation_bound() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    let (agent_id, credential) = enroll_agent(&harness, &owner).await;
    let report = fixture_report(&agent_id, 1);
    let node_id = report.inventory.nodes[0].node_id.to_string();
    harness
        .send(bearer_post(
            "/api/agent/v1/reports",
            &credential,
            serde_json::to_vec(&report).unwrap(),
        ))
        .await;

    // A Viewer cannot read the preview or mutate.
    let viewer_preview = harness
        .send(admin_get(
            &format!("/api/admin/v1/nodes/{node_id}/purge"),
            &viewer,
        ))
        .await;
    assert_eq!(viewer_preview.status(), StatusCode::FORBIDDEN);
    let viewer_purge = harness
        .send(admin_post(
            &format!("/api/admin/v1/nodes/{node_id}/purge"),
            &viewer,
            &format!(r#"{{"confirmNodeId":"{node_id}"}}"#),
        ))
        .await;
    assert_eq!(viewer_purge.status(), StatusCode::FORBIDDEN);

    // Missing CSRF is refused before the body is trusted.
    let no_csrf = Request::builder()
        .method("POST")
        .uri(format!("/api/admin/v1/nodes/{node_id}/purge"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
        .header(header::COOKIE, &owner.cookie)
        .body(Body::from(format!(r#"{{"confirmNodeId":"{node_id}"}}"#)))
        .unwrap();
    let response = harness.send(no_csrf).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "csrf_validation_failed"
    );

    // A mismatched confirmation is refused and deletes nothing.
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/nodes/{node_id}/purge"),
            &owner,
            r#"{"confirmNodeId":"some-other-node"}"#,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "confirmation_mismatch"
    );
    assert!(count_for_node(&harness, "component_status", &node_id).await > 0);

    // An unknown Node is the same non-leaking 404 as every other Admin read.
    let unknown = harness
        .send(admin_post(
            "/api/admin/v1/nodes/does-not-exist/purge",
            &owner,
            r#"{"confirmNodeId":"does-not-exist"}"#,
        ))
        .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

/// Active and Retired Nodes are both explicitly purgeable; no offline timer is
/// involved.
#[tokio::test]
async fn active_and_retired_nodes_are_both_purgeable() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let (agent_id, _credential) = enroll_agent(&harness, &owner).await;
    let now = "2026-01-01T00:00:00Z";
    for (node_id, lifecycle) in [("active-node", "active"), ("retired-node", "retired")] {
        sqlx::query("INSERT OR IGNORE INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, ?, ?, 'ws://127.0.0.1:1', ?, 'private', 1, ?, ?)")
            .bind(node_id).bind(&agent_id).bind(NETWORK_KEY).bind(lifecycle).bind(now).bind(now)
            .execute(harness.pool()).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, state_revision, value_revision) VALUES (?, 'node', ?, ?, 'rpc', 'ok', 1, 1)")
            .bind(&agent_id).bind(node_id).bind(node_id).execute(harness.pool()).await.unwrap();
        let response = harness
            .send(admin_post(
                &format!("/api/admin/v1/nodes/{node_id}/purge"),
                &owner,
                &format!(r#"{{"confirmNodeId":"{node_id}"}}"#),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{node_id} {lifecycle}");
        assert_eq!(
            count_for_node(&harness, "component_status", node_id).await,
            0
        );
    }
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes")
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

/// A purged Node ID is refused per entry while a valid sibling in the same
/// legal report is admitted; the Receipt carries that per-Node and per-sample
/// outcome instead of reporting a whole success.
#[tokio::test]
async fn purged_node_is_rejected_per_entry_while_its_sibling_is_admitted() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let (agent_id, credential) = enroll_agent(&harness, &owner).await;

    let first = two_node_report(
        &agent_id,
        1,
        1,
        "0195f2a1-0020-4020-8020-000000000020",
        1,
        10,
    );
    let purged = first.inventory.nodes[0].node_id.to_string();
    let sibling = first.inventory.nodes[1].node_id.to_string();
    let (status, value) = post_report(&harness, &credential, &first).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(receipt_of(&value).disposition, ReceiptDisposition::Accepted);

    purge_node(&harness, &owner, &purged).await;
    assert_eq!(node_row_count(&harness, &purged).await, 0);

    // A new Inventory revision still declares both IDs. New heights keep the
    // sibling's fresh sample genuinely admissible.
    let second = two_node_report(
        &agent_id,
        1,
        2,
        "0195f2a1-0021-4021-8021-000000000021",
        2,
        20,
    );
    let (status, value) = post_report(&harness, &credential, &second).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let receipt = receipt_of(&value);
    assert_eq!(
        receipt.disposition,
        ReceiptDisposition::PartiallyAccepted,
        "a partial admission must never be reported as a whole success: {value}"
    );
    let purged_entry = receipt
        .nodes
        .iter()
        .find(|node| node.node_id.to_string() == purged)
        .unwrap();
    assert_eq!(purged_entry.current, NodeCurrentDisposition::Rejected);
    assert_eq!(purged_entry.rejections.len(), 1);
    assert_eq!(purged_entry.rejections[0].code, RejectionCode::NodePurged);
    assert!(!purged_entry.rejections[0].retryable);
    assert!(purged_entry.accepted_component_revisions.is_empty());
    let sibling_entry = receipt
        .nodes
        .iter()
        .find(|node| node.node_id.to_string() == sibling)
        .unwrap();
    assert_eq!(sibling_entry.current, NodeCurrentDisposition::Accepted);
    assert!(sibling_entry.rejections.is_empty());

    // The purged Node's sample is terminal, never accepted; the sibling's is
    // accepted.
    let purged_sample = receipt
        .samples
        .iter()
        .find(|sample| sample.node_id.to_string() == purged)
        .expect("the purged Node's Block Summary still appears in the Receipt");
    assert_eq!(
        purged_sample.disposition,
        SampleDispositionKind::TerminalRejected
    );
    assert_eq!(
        purged_sample.rejection.as_ref().unwrap().code,
        RejectionCode::NodePurged
    );
    let sibling_sample = receipt
        .samples
        .iter()
        .find(|sample| sample.node_id.to_string() == sibling)
        .unwrap();
    assert_eq!(sibling_sample.disposition, SampleDispositionKind::Accepted);

    // The purged ID was not rebuilt; the sibling is live.
    assert_eq!(node_row_count(&harness, &purged).await, 0);
    for table in [
        "component_status",
        "block_summaries",
        "block_history_state",
        "block_identity_window",
        "observed_network_heads",
        "node_transfers",
    ] {
        assert_eq!(
            count_for_node(&harness, table, &purged).await,
            0,
            "{table} rebuilt the purged Node"
        );
    }
    assert_eq!(node_row_count(&harness, &sibling).await, 1);
    assert!(count_for_node(&harness, "component_status", &sibling).await > 0);
    assert!(count_for_node(&harness, "block_summaries", &sibling).await > 0);
}

/// An authenticated replay returns the stored immutable Receipt and never
/// re-applies the removed projection; a same-revision declaration and a new
/// Inventory revision are both refused at the same boundary.
#[tokio::test]
async fn replay_same_revision_and_new_revision_cannot_rebuild_a_purged_node() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let (agent_id, credential) = enroll_agent(&harness, &owner).await;

    let first = fixture_report(&agent_id, 1);
    let node_id = first.inventory.nodes[0].node_id.to_string();
    let (status, value) = post_report(&harness, &credential, &first).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let original = value["receipt"].clone();
    let dedup_before: (String, String) = sqlx::query_as(
        "SELECT report_body_sha256, disposition FROM agent_report_receipts WHERE report_id = ?",
    )
    .bind(first.report_id.to_string())
    .fetch_one(harness.pool())
    .await
    .unwrap();

    purge_node(&harness, &owner, &node_id).await;

    // Exact replay returns the stored, immutable Receipt unchanged.
    let (status, replayed) = post_report(&harness, &credential, &first).await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["receipt"], original);
    assert_eq!(node_row_count(&harness, &node_id).await, 0);
    assert_eq!(
        count_for_node(&harness, "component_status", &node_id).await,
        0
    );
    // The deletion does not rewrite the old Receipt or the dedup boundary.
    let dedup_after: (String, String) = sqlx::query_as(
        "SELECT report_body_sha256, disposition FROM agent_report_receipts WHERE report_id = ?",
    )
    .bind(first.report_id.to_string())
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(dedup_after, dedup_before);

    // A new report id at the same Inventory revision is not a replay: the
    // purged ID is still rejected and never inserted.
    let mut same_revision = first.clone();
    same_revision.report_sequence = 2;
    same_revision.report_id = "0195f2a1-0030-4030-8030-000000000030".parse().unwrap();
    let (status, value) = post_report(&harness, &credential, &same_revision).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let receipt = receipt_of(&value);
    assert_eq!(receipt.inventory, Some(InventoryDisposition::Unchanged));
    assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
    assert_eq!(
        receipt.nodes[0].rejections[0].code,
        RejectionCode::NodePurged
    );
    assert_eq!(node_row_count(&harness, &node_id).await, 0);

    // A new Inventory revision cannot rebuild it either.
    let mut new_revision = first.clone();
    new_revision.report_sequence = 3;
    new_revision.report_id = "0195f2a1-0031-4031-8031-000000000031".parse().unwrap();
    new_revision.inventory.revision = 2;
    let (status, value) = post_report(&harness, &credential, &new_revision).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let receipt = receipt_of(&value);
    assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
    assert_eq!(
        receipt.nodes[0].rejections[0].code,
        RejectionCode::NodePurged
    );
    assert_eq!(node_row_count(&harness, &node_id).await, 0);
    assert_eq!(
        count_for_node(&harness, "component_status", &node_id).await,
        0
    );
    assert_eq!(
        count_for_node(&harness, "block_history_state", &node_id).await,
        0
    );
}

/// The deletion identity is durable: a freshly opened Server over the same
/// database still returns the stored Receipt and still refuses late reports.
#[tokio::test]
async fn a_purged_node_stays_unreconstructable_after_a_server_restart() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let (agent_id, credential) = enroll_agent(&harness, &owner).await;
    let first = fixture_report(&agent_id, 1);
    let node_id = first.inventory.nodes[0].node_id.to_string();
    let (status, value) = post_report(&harness, &credential, &first).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let original = value["receipt"].clone();
    purge_node(&harness, &owner, &node_id).await;

    let harness = harness.restart().await;
    let _owner = login(&harness, OWNER_LOGIN_BODY).await;

    // The stored Receipt still wins for a replay.
    let (status, value) = post_report(&harness, &credential, &first).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["receipt"], original);
    assert_eq!(node_row_count(&harness, &node_id).await, 0);

    // A late report after the restart is rejected at the same boundary.
    let mut late = first.clone();
    late.report_sequence = 2;
    late.report_id = "0195f2a1-0040-4040-8040-000000000040".parse().unwrap();
    let (status, value) = post_report(&harness, &credential, &late).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let receipt = receipt_of(&value);
    assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
    assert_eq!(
        receipt.nodes[0].rejections[0].code,
        RejectionCode::NodePurged
    );
    assert_eq!(node_row_count(&harness, &node_id).await, 0);

    let deleted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deleted_nodes WHERE node_id = ?")
        .bind(&node_id)
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(deleted, 1, "the purge identity must survive a restart");
}

/// A periodic background pass over the subjects that still exist cannot
/// reconstruct a purged Node.
#[tokio::test]
async fn background_evaluation_and_retention_do_not_rebuild_a_purged_node() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let (agent_id, credential) = enroll_agent(&harness, &owner).await;
    let report = fixture_report(&agent_id, 1);
    let node_id = report.inventory.nodes[0].node_id.to_string();
    let (status, value) = post_report(&harness, &credential, &report).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    purge_node(&harness, &owner, &node_id).await;

    // The Alert evaluation sweep and the raw block retention pass run after
    // the deletion; neither recreates a Node row or Node-owned history.
    platpulse_server::alerts::sweep(&harness.state)
        .await
        .unwrap();
    platpulse_server::retention::cleanup_raw_block_summaries(
        harness.pool(),
        platpulse_server::auth::now_utc(),
    )
    .await
    .unwrap();

    assert_eq!(node_row_count(&harness, &node_id).await, 0);
    for table in [
        "component_status",
        "block_summaries",
        "block_history_state",
        "block_identity_window",
    ] {
        assert_eq!(
            count_for_node(&harness, table, &node_id).await,
            0,
            "{table} was rebuilt after the purge"
        );
    }
}

/// A purged entry's Network key is not registry-validated (it must not block a
/// valid sibling), and that unvalidated key must not leak into shared
/// Network-scoped state.
#[tokio::test]
async fn a_purged_entrys_unknown_network_key_neither_blocks_siblings_nor_leaks() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let (agent_id, credential) = enroll_agent(&harness, &owner).await;

    let first = two_node_report(
        &agent_id,
        1,
        1,
        "0195f2a1-0050-4050-8050-000000000050",
        1,
        10,
    );
    let purged = first.inventory.nodes[0].node_id.to_string();
    let sibling = first.inventory.nodes[1].node_id.to_string();
    let (status, value) = post_report(&harness, &credential, &first).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    purge_node(&harness, &owner, &purged).await;

    // The same legal report now declares the purged ID under an unregistered
    // Network key next to a valid sibling. The report is still admitted per
    // Node rather than whole-rejected as NetworkKeyUnknown.
    let mut declared = two_node_report(
        &agent_id,
        1,
        2,
        "0195f2a1-0051-4051-8051-000000000051",
        2,
        20,
    );
    declared.inventory.nodes[0].network_key = "platon-testnet".parse().unwrap();
    declared.validate().unwrap();
    let (status, value) = post_report(&harness, &credential, &declared).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let receipt = receipt_of(&value);
    assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
    let purged_entry = receipt
        .nodes
        .iter()
        .find(|node| node.node_id.to_string() == purged)
        .unwrap();
    assert_eq!(purged_entry.rejections[0].code, RejectionCode::NodePurged);
    let sibling_entry = receipt
        .nodes
        .iter()
        .find(|node| node.node_id.to_string() == sibling)
        .unwrap();
    assert_eq!(sibling_entry.current, NodeCurrentDisposition::Accepted);
    assert_eq!(node_row_count(&harness, &purged).await, 0);

    // The purged entry's Network key never reaches the shared Network
    // projection.
    let leaked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM network_reference_heads WHERE network_key = 'platon-testnet'",
    )
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(
        leaked, 0,
        "a purged entry's unvalidated Network key must not create shared Network state"
    );
}
