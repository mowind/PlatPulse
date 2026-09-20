//! Agent Removal acceptance through the real router (issue #171, part of
//! #165).
//!
//! Agent Removal is an explicit, irreversible Owner disposition that is
//! deliberately distinct from Credential Revocation. These tests build the
//! full application with build_app against a temporary SQLite database, mint
//! real Agent credentials through the Admin API, submit real reports, and then
//! exercise the preview and mutation over HTTP. They assert the durable
//! outcome: every credential is revoked, every owned Node is purged through
//! the Node Purge path, the Agent carries a durable removal marker, shared
//! Agent/Host/Validator/Incident evidence survives, and Recovery, Rotation,
//! an outstanding Recovery Token, a late Report, or a restart cannot resurrect
//! the identity.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_core::AgentReport;
use platpulse_core::block::{BlockProductionAttribution, BlockSource, BlockSummary};
use platpulse_server::{AppState, auth, database, http, network, secrets};

const NETWORK_KEY: &str = "platon-mainnet";
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const VIEWER_LOGIN_BODY: &str = r#"{"username":"viewer","password":"viewer password"}"#;

const NODE_A1: &str = "0195f2a1-0014-4014-8014-000000000014";
const NODE_A2: &str = "0195f2a1-0015-4015-8015-000000000015";
const NODE_B: &str = "0195f2a1-00b0-40b0-80b0-0000000000b0";

const REPORT_1: &str = "0195f2a1-0013-4013-8013-000000000013";
const REPORT_2: &str = "0195f2a1-0023-4023-8023-000000000023";
const REPORT_B: &str = "0195f2a1-00b3-40b3-80b3-0000000000b3";

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

fn admin_put(uri: &str, session: &Session, body: &str) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
        .header("x-csrf-token", &session.csrf)
        .header(header::COOKIE, &session.cookie)
        .body(Body::from(body.to_owned()))
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

/// One legal report declaring the given Node IDs for one Agent, with the
/// declared Network Identity aligned to the registered Network and one Block
/// Summary per Node so every Node is genuinely admitted.
fn multi_node_report(
    agent_id: &str,
    agent_epoch: u64,
    report_sequence: u64,
    report_id: &str,
    inventory_revision: u64,
    first_height: u64,
    node_ids: &[&str],
) -> AgentReport {
    let mut report = fixture_report(agent_id, agent_epoch);
    report.report_sequence = report_sequence;
    report.report_id = report_id.parse().unwrap();
    report.inventory.revision = inventory_revision;
    for node in &mut report.nodes {
        let identity = node.chain.network_identity.latest.as_mut().unwrap();
        identity.genesis_hash = NETWORK_GENESIS.parse().unwrap();
        identity.address_hrp = Some("lat".to_owned());
    }
    let base_inventory = report.inventory.nodes[0].clone();
    let base_observation = report.nodes[0].clone();
    report.inventory.nodes = node_ids
        .iter()
        .map(|node_id| {
            let mut node = base_inventory.clone();
            node.node_id = node_id.parse().unwrap();
            node
        })
        .collect();
    report.nodes = node_ids
        .iter()
        .map(|node_id| {
            let mut node = base_observation.clone();
            node.node_id = node_id.parse().unwrap();
            node
        })
        .collect();
    report.block_summaries = report
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
    report.validate().unwrap();
    report
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

async fn submit_accepted(harness: &Harness, credential: &str, report: &AgentReport) {
    let (status, value) = post_report(harness, credential, report).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        value["receipt"]["disposition"],
        serde_json::json!("accepted"),
        "{value}"
    );
}

async fn count_for_node(harness: &Harness, table: &str, node_id: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE node_id = ?");
    sqlx::query_scalar(&sql)
        .bind(node_id)
        .fetch_one(harness.pool())
        .await
        .unwrap()
}

/// Independent Validator history, a Network reference head, Host metrics, and
/// existing Alert Incident evidence that a removal must never erase.
async fn seed_shared_evidence(harness: &Harness, agent_id: &str, node_id: &str) {
    let pool = harness.pool();
    let now = "2026-01-01T00:00:00Z";
    sqlx::query("INSERT OR IGNORE INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-1', ?, '0xvalidator', 'Validator One', ?, ?)")
        .bind(NETWORK_KEY).bind(now).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, source, observation_key) VALUES ('snapshot-1', 'validator-1', 'UTC', '2026-01-01', '2026-01', ?, ?, 'test', 'observation-1')")
        .bind(now).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES (?, 1, ?, 'high', 1, NULL)")
        .bind(NETWORK_KEY).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT OR IGNORE INTO host_metric_samples (agent_id, metric, observed_at, received_at, value) VALUES (?, 'network_rx_bytes_per_sec', ?, ?, 1.0)")
        .bind(agent_id).bind(now).bind(now).execute(pool).await.unwrap();
    let rule_key: String = sqlx::query_scalar("SELECT rule_key FROM alert_rules LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT OR IGNORE INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, opened_evidence_json) VALUES ('incident-agent-1', ?, 1, 'agent', ?, 'warning', 'open', 1, ?, '{}')")
        .bind(rule_key).bind(agent_id).bind(now).execute(pool).await.unwrap();
    let _ = node_id;
}

async fn preview(harness: &Harness, session: &Session, agent_id: &str) -> Value {
    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/agents/{agent_id}/removal"),
            session,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

fn owned_node_ids(preview: &Value) -> Vec<String> {
    let mut ids: Vec<String> = preview["owned_nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["node_id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    ids
}

fn removal_body(agent_id: &str, node_ids: &[String]) -> String {
    serde_json::json!({
        "confirmAgentId": agent_id,
        "confirmedNodeIds": node_ids,
    })
    .to_string()
}

async fn remove(
    harness: &Harness,
    session: &Session,
    agent_id: &str,
    node_ids: &[String],
) -> (StatusCode, Value) {
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{agent_id}/removal"),
            session,
            &removal_body(agent_id, node_ids),
        ))
        .await;
    let status = response.status();
    (status, body_json(response).await)
}

/// The canonical acceptance: a removal is distinct from credential
/// revocation, revokes every credential, purges every owned Node, and leaves
/// shared evidence intact.
#[tokio::test]
async fn owner_removal_revokes_credentials_purges_nodes_and_keeps_shared_evidence() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let (agent_a, credential_a) = enroll_agent(&harness, &owner).await;
    let report = multi_node_report(&agent_a, 1, 1, REPORT_1, 1, 1, &[NODE_A1, NODE_A2]);
    submit_accepted(&harness, &credential_a, &report).await;

    let (agent_b, credential_b) = enroll_agent(&harness, &owner).await;
    let report_b = multi_node_report(&agent_b, 1, 1, REPORT_B, 1, 10, &[NODE_B]);
    submit_accepted(&harness, &credential_b, &report_b).await;

    seed_shared_evidence(&harness, &agent_a, NODE_A1).await;

    // Credential Revocation and Delete Agent are different actions: rotating
    // and then revoking one credential leaves the Agent and its Nodes intact.
    let rotated = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{agent_a}/credentials/rotate"),
            &owner,
            r#"{"overlapHours":24,"revokePrevious":false}"#,
        ))
        .await;
    assert_eq!(rotated.status(), StatusCode::OK);
    let rotated = body_json(rotated).await;
    let new_credential_id = rotated["credential_id"].as_str().unwrap().to_owned();
    let revoked = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{agent_a}/credentials/{new_credential_id}/revoke"),
            &owner,
            "{}",
        ))
        .await;
    assert_eq!(revoked.status(), StatusCode::OK);
    let list_after_revoke = body_json(
        harness
            .send(admin_get("/api/admin/v1/agents", &owner))
            .await,
    )
    .await;
    assert!(
        list_after_revoke.to_string().contains(&agent_a),
        "credential revocation must not remove the Agent"
    );
    assert!(count_for_node(&harness, "component_status", NODE_A1).await > 0);

    // A pre-issued Recovery Token must not survive the removal.
    let recovery = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{agent_a}/recover"),
            &owner,
            r#"{"expiresInHours":24}"#,
        ))
        .await;
    assert_eq!(recovery.status(), StatusCode::OK);
    let recovery_token = body_json(recovery).await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    // Preview lists the authoritative owned Nodes and blocking state.
    let impact = preview(&harness, &owner, &agent_a).await;
    assert_eq!(impact["target"]["agent_id"], agent_a);
    assert_eq!(impact["can_remove"], true);
    assert!(impact["active_credential_count"].as_i64().unwrap() >= 1);
    let confirmed = owned_node_ids(&impact);
    assert_eq!(confirmed, vec![NODE_A1.to_owned(), NODE_A2.to_owned()]);
    assert!(impact["counts"]["total_owned_rows"].as_i64().unwrap() > 0);

    // Remove.
    let (status, body) = remove(&harness, &owner, &agent_a, &confirmed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["agent_id"], agent_a);
    assert_eq!(body["purged_nodes"].as_array().unwrap().len(), 2);
    assert_eq!(body["revoked_credential_count"].as_i64().unwrap(), 1);

    // Every owned Node is gone through the Node Purge path and keeps a deletion
    // identity; the Agent row is marked, not physically deleted.
    for node_id in [NODE_A1, NODE_A2] {
        let nodes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE node_id = ?")
            .bind(node_id)
            .fetch_one(harness.pool())
            .await
            .unwrap();
        assert_eq!(nodes, 0, "owned Node must be purged");
        assert_eq!(
            count_for_node(&harness, "component_status", node_id).await,
            0
        );
        let deleted: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM deleted_nodes WHERE node_id = ?")
                .bind(node_id)
                .fetch_one(harness.pool())
                .await
                .unwrap();
        assert_eq!(deleted, 1);
    }
    let deleted_at: Option<String> =
        sqlx::query_scalar("SELECT deleted_at FROM agents WHERE agent_id = ?")
            .bind(&agent_a)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert!(deleted_at.is_some(), "the Agent removal marker is durable");
    let deleted_by: Option<String> =
        sqlx::query_scalar("SELECT deleted_by_user_id FROM agents WHERE agent_id = ?")
            .bind(&agent_a)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert!(deleted_by.is_some(), "the Audit actor is recorded");
    let audit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'agent_removed' AND target_kind = 'agent' AND target_id = ?",
    )
    .bind(&agent_a)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(audit, 1);

    // The removed Agent leaves the Admin list, Overview, and its detail route.
    let list = body_json(
        harness
            .send(admin_get("/api/admin/v1/agents", &owner))
            .await,
    )
    .await;
    assert!(!list.to_string().contains(&agent_a));
    let overview = body_json(
        harness
            .send(admin_get("/api/admin/v1/overview", &owner))
            .await,
    )
    .await;
    assert!(!overview.to_string().contains(&agent_a));
    let detail = harness
        .send(admin_get(
            &format!("/api/admin/v1/agents/{agent_a}"),
            &owner,
        ))
        .await;
    assert_eq!(detail.status(), StatusCode::NOT_FOUND);

    // Public Node detail is non-leakingly unavailable.
    let public = harness
        .send(admin_get(
            &format!("/api/public/v1/nodes/{NODE_A1}"),
            &owner,
        ))
        .await;
    assert_eq!(public.status(), StatusCode::NOT_FOUND);
    assert!(!body_json(public).await.to_string().contains(NODE_A1));

    // Recovery, Rotation, Revocation, a late Report, and the outstanding
    // Recovery Token all fail; none of them resurrects the identity.
    let late = harness
        .send(bearer_post(
            "/api/agent/v1/reports",
            &credential_a,
            serde_json::to_vec(&multi_node_report(
                &agent_a,
                1,
                2,
                REPORT_2,
                2,
                20,
                &[NODE_A1],
            ))
            .unwrap(),
        ))
        .await;
    assert_eq!(late.status(), StatusCode::UNAUTHORIZED);
    let recover_again = harness
        .send(bearer_post(
            "/api/agent/v1/recover",
            &recovery_token,
            Vec::new(),
        ))
        .await;
    assert_eq!(recover_again.status(), StatusCode::UNAUTHORIZED);
    let admin_recover = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{agent_a}/recover"),
            &owner,
            r#"{"expiresInHours":24}"#,
        ))
        .await;
    assert_eq!(admin_recover.status(), StatusCode::NOT_FOUND);
    let rotate = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{agent_a}/credentials/rotate"),
            &owner,
            r#"{"overlapHours":24,"revokePrevious":false}"#,
        ))
        .await;
    assert_eq!(rotate.status(), StatusCode::NOT_FOUND);
    let revoke = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{agent_a}/credentials/{new_credential_id}/revoke"),
            &owner,
            "{}",
        ))
        .await;
    assert_eq!(revoke.status(), StatusCode::NOT_FOUND);
    let metadata = harness
        .send(admin_put(
            &format!("/api/admin/v1/agents/{agent_a}/metadata"),
            &owner,
            r#"{"displayName":"renamed"}"#,
        ))
        .await;
    assert_eq!(metadata.status(), StatusCode::NOT_FOUND);
    let detail_again = harness
        .send(admin_get(
            &format!("/api/admin/v1/agents/{agent_a}"),
            &owner,
        ))
        .await;
    assert_eq!(detail_again.status(), StatusCode::NOT_FOUND);

    // Shared Agent/Host, Validator history, Network reference, and Incident
    // evidence survive, and the other Agent/Node is untouched.
    let host: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM current_host_observations WHERE agent_id = ?")
            .bind(&agent_a)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(host, 1, "Host observation evidence is retained");
    let host_samples: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM host_metric_samples WHERE agent_id = ?")
            .bind(&agent_a)
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert!(host_samples >= 1, "Host metric evidence is retained");
    let validators: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM validator_daily_snapshots WHERE validator_id = 'validator-1'",
    )
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(validators, 1, "independent Validator history survives");
    let incidents: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM alert_incidents WHERE incident_id = 'incident-agent-1'",
    )
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(incidents, 1, "Incident evidence is retained");
    assert!(count_for_node(&harness, "component_status", NODE_B).await > 0);
    assert!(
        body_json(
            harness
                .send(admin_get("/api/admin/v1/agents", &owner))
                .await
        )
        .await
        .to_string()
        .contains(&agent_b),
        "another Agent is unaffected"
    );

    // A removed Agent cannot be re-engaged as a Transfer target: the live
    // predicate refuses it exactly like any other unregistered Agent.
    let transfer_to_removed = harness
        .send(admin_post(
            &format!("/api/admin/v1/nodes/{NODE_B}/transfers"),
            &owner,
            &serde_json::json!({"targetAgentId": agent_a, "expiresInHours": 24}).to_string(),
        ))
        .await;
    assert_eq!(transfer_to_removed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(transfer_to_removed).await["error"]["code"],
        "invalid_target_agent"
    );

    // The removal survives a Server restart.
    let harness = harness.restart().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let node_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE agent_id = ?")
        .bind(&agent_a)
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(node_rows, 0);
    let detail = harness
        .send(admin_get(
            &format!("/api/admin/v1/agents/{agent_a}"),
            &owner,
        ))
        .await;
    assert_eq!(detail.status(), StatusCode::NOT_FOUND);
}

/// An unhandled Transfer blocks the removal until it is cancelled.
#[tokio::test]
async fn pending_transfer_blocks_removal_until_handled() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let (agent_a, credential_a) = enroll_agent(&harness, &owner).await;
    submit_accepted(
        &harness,
        &credential_a,
        &multi_node_report(&agent_a, 1, 1, REPORT_1, 1, 1, &[NODE_A1]),
    )
    .await;
    let (agent_c, _) = enroll_agent(&harness, &owner).await;

    let created = harness
        .send(admin_post(
            &format!("/api/admin/v1/nodes/{NODE_A1}/transfers"),
            &owner,
            &serde_json::json!({"targetAgentId": agent_c, "expiresInHours": 24}).to_string(),
        ))
        .await;
    assert_eq!(created.status(), StatusCode::OK);
    let transfer_id = body_json(created).await["transfer"]["transfer_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let impact = preview(&harness, &owner, &agent_a).await;
    assert_eq!(impact["can_remove"], false);
    assert_eq!(impact["pending_transfers"].as_array().unwrap().len(), 1);
    assert_eq!(impact["pending_transfers"][0]["direction"], "source");

    let (status, body) = remove(&harness, &owner, &agent_a, &[NODE_A1.to_owned()]).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "pending_transfer");
    assert!(count_for_node(&harness, "component_status", NODE_A1).await > 0);

    // Handling the Transfer unblocks the removal.
    let cancelled = harness
        .send(admin_post(
            &format!("/api/admin/v1/transfers/{transfer_id}/cancel"),
            &owner,
            "{}",
        ))
        .await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    let (status, body) = remove(&harness, &owner, &agent_a, &[NODE_A1.to_owned()]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["purged_nodes"].as_array().unwrap().len(), 1);
}

/// A change to the owned set between the preview and the mutation is refused
/// until the Owner refetches and confirms again.
#[tokio::test]
async fn changed_ownership_requires_refetch_and_reconfirmation() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let (agent_a, credential_a) = enroll_agent(&harness, &owner).await;
    submit_accepted(
        &harness,
        &credential_a,
        &multi_node_report(&agent_a, 1, 1, REPORT_1, 1, 1, &[NODE_A1]),
    )
    .await;

    let impact = preview(&harness, &owner, &agent_a).await;
    let stale_confirmed = owned_node_ids(&impact);
    assert_eq!(stale_confirmed, vec![NODE_A1.to_owned()]);

    // A second Node appears after the confirmation was rendered.
    submit_accepted(
        &harness,
        &credential_a,
        &multi_node_report(&agent_a, 1, 2, REPORT_2, 2, 21, &[NODE_A1, NODE_A2]),
    )
    .await;

    let (status, body) = remove(&harness, &owner, &agent_a, &stale_confirmed).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "ownership_changed");
    assert!(count_for_node(&harness, "component_status", NODE_A1).await > 0);
    assert!(count_for_node(&harness, "component_status", NODE_A2).await > 0);

    // Refetch and confirm the authoritative set.
    let impact = preview(&harness, &owner, &agent_a).await;
    let confirmed = owned_node_ids(&impact);
    assert_eq!(confirmed, vec![NODE_A1.to_owned(), NODE_A2.to_owned()]);
    let (status, body) = remove(&harness, &owner, &agent_a, &confirmed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["purged_nodes"].as_array().unwrap().len(), 2);
}

/// Owner-only, CSRF/Origin checked, and confirmation bound.
#[tokio::test]
async fn removal_is_owner_only_csrf_checked_and_confirmation_bound() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    let (agent_a, credential_a) = enroll_agent(&harness, &owner).await;
    submit_accepted(
        &harness,
        &credential_a,
        &multi_node_report(&agent_a, 1, 1, REPORT_1, 1, 1, &[NODE_A1]),
    )
    .await;

    let viewer_preview = harness
        .send(admin_get(
            &format!("/api/admin/v1/agents/{agent_a}/removal"),
            &viewer,
        ))
        .await;
    assert_eq!(viewer_preview.status(), StatusCode::FORBIDDEN);
    let viewer_remove = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{agent_a}/removal"),
            &viewer,
            &removal_body(&agent_a, &[NODE_A1.to_owned()]),
        ))
        .await;
    assert_eq!(viewer_remove.status(), StatusCode::FORBIDDEN);

    // Missing CSRF is refused before the body is trusted.
    let no_csrf = Request::builder()
        .method("POST")
        .uri(format!("/api/admin/v1/agents/{agent_a}/removal"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
        .header(header::COOKIE, &owner.cookie)
        .body(Body::from(removal_body(&agent_a, &[NODE_A1.to_owned()])))
        .unwrap();
    let response = harness.send(no_csrf).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "csrf_validation_failed"
    );

    // A mismatched confirmation is refused and deletes nothing.
    let mismatched = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{agent_a}/removal"),
            &owner,
            &serde_json::json!({
                "confirmAgentId": "some-other-agent",
                "confirmedNodeIds": [NODE_A1],
            })
            .to_string(),
        ))
        .await;
    assert_eq!(mismatched.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(mismatched).await["error"]["code"],
        "confirmation_mismatch"
    );
    assert!(count_for_node(&harness, "component_status", NODE_A1).await > 0);

    // An empty confirmation for a Node-owning Agent is an ownership change.
    let (status, body) = remove(&harness, &owner, &agent_a, &[]).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "ownership_changed");
    assert!(count_for_node(&harness, "component_status", NODE_A1).await > 0);
}
