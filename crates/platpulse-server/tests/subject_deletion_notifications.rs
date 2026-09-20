//! Deleted-subject notification cancellation and Incident annotation
//! (issue #175, part of #165).
//!
//! A Node Purge or Agent Removal is not recovery. These tests build the full
//! application with build_app against a temporary SQLite database, submit real
//! reports, seed real Notification Events/Deliveries and Alert Incidents, and
//! exercise the preview/mutation over HTTP. They assert the durable outcome:
//! the deleted subject leaves current evaluation, its retained Incidents are
//! annotated as subject-deleted without being rewritten to a false recovery,
//! its not-yet-sent notifications (including resolution Events with no
//! incident_id) are cancelled, already-sent/handed-off facts survive, and no
//! other subject's notification policy is touched.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_core::AgentReport;
use platpulse_core::block::{BlockProductionAttribution, BlockSource, BlockSummary};
use platpulse_server::alerts::SubjectKind;
use platpulse_server::config::{NotificationChannels, TelegramChannel};
use platpulse_server::notifications::{
    DeliveryProvider, NotificationEventInput, SendError, process_due_deliveries,
    record_notification_event,
};
use platpulse_server::{AppState, auth, database, http, network, secrets};

const NETWORK_KEY: &str = "platon-mainnet";
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;

const NODE_A: &str = "0195f2a1-0175-4175-8175-000000000175";
const NODE_B: &str = "0195f2a1-0275-4275-8275-000000000275";
const REPORT_A: &str = "0195f2a1-0375-4375-8375-000000000375";
const REPORT_B: &str = "0195f2a1-0475-4475-8475-000000000475";

/// A deterministic provider that records every message it was asked to send.
struct CountingProvider {
    sends: Mutex<Vec<String>>,
}

impl CountingProvider {
    fn new() -> Self {
        Self {
            sends: Mutex::new(Vec::new()),
        }
    }

    fn sends(&self) -> Vec<String> {
        self.sends.lock().expect("sends").clone()
    }
}

#[async_trait::async_trait]
impl DeliveryProvider for CountingProvider {
    async fn send(&self, _channel: &TelegramChannel, text: &str) -> Result<(), SendError> {
        self.sends.lock().expect("sends").push(text.to_owned());
        Ok(())
    }
}

struct Harness {
    _dir: TempDir,
    state: AppState,
    app: Router,
    channels: NotificationChannels,
    provider: Arc<CountingProvider>,
}

impl Harness {
    async fn boot() -> Self {
        let dir = TempDir::new().unwrap();
        let database = database::initialize(database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pepper_path = dir.path().join("server-pepper");
        secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = auth::AuthConfig::development(
            secrets::load_pepper_file(&pepper_path).unwrap(),
            DEVELOPMENT_ORIGIN.to_owned(),
        );
        let channels = NotificationChannels {
            telegram: Some(TelegramChannel {
                enabled: true,
                token_file: dir.path().join("telegram-token"),
                chat_id: "123456789".to_owned(),
                max_attempts: 3,
                retry_base_seconds: 60,
            }),
        };
        let state = AppState::new_with_proxy_policy(
            database,
            None,
            auth,
            Vec::new(),
            None,
            channels.clone(),
        );
        let app = http::build_app(state.clone());

        let owner_hash = auth::hash_password(b"correct horse battery").unwrap();
        auth::create_owner(state.db(), "admin", &owner_hash)
            .await
            .unwrap();
        network::create_network(
            state.db(),
            NETWORK_KEY,
            "PlatON Mainnet",
            NETWORK_GENESIS,
            210425,
            210425,
            "lat",
        )
        .await
        .unwrap();

        Self {
            _dir: dir,
            state,
            app,
            channels,
            provider: Arc::new(CountingProvider::new()),
        }
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

async fn login(harness: &Harness) -> Session {
    let request = Request::builder()
        .method("POST")
        .uri("/api/public/v1/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
        .body(Body::from(OWNER_LOGIN_BODY.to_owned()))
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

async fn submit_accepted(harness: &Harness, credential: &str, report: &AgentReport) {
    let response = harness
        .send(bearer_post(
            "/api/agent/v1/reports",
            credential,
            serde_json::to_vec(report).unwrap(),
        ))
        .await;
    let status = response.status();
    let value = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["receipt"]["disposition"], "accepted", "{value}");
}

async fn seed_incident(
    harness: &Harness,
    incident_id: &str,
    rule_key: &str,
    subject_kind: &str,
    subject_key: &str,
) {
    sqlx::query(
        "INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, opened_evidence_json) VALUES (?, ?, 1, ?, ?, 'warning', 'open', 1, '2026-01-01T00:00:00Z', '{}')",
    )
    .bind(incident_id)
    .bind(rule_key)
    .bind(subject_kind)
    .bind(subject_key)
    .execute(harness.pool())
    .await
    .unwrap();
}

async fn seed_rule_state(harness: &Harness, rule_key: &str, subject_kind: &str, subject_key: &str) {
    sqlx::query(
        "INSERT OR REPLACE INTO alert_rule_state (rule_key, subject_kind, subject_key, state, since, input_kind, last_evaluated_at) VALUES (?, ?, ?, 'firing', '2026-01-01T00:00:00Z', 'known', '2026-01-01T00:00:00Z')",
    )
    .bind(rule_key)
    .bind(subject_kind)
    .bind(subject_key)
    .execute(harness.pool())
    .await
    .unwrap();
}

/// Create one durable Notification Event plus its telegram Delivery.
async fn seed_event(
    harness: &Harness,
    incident_id: Option<&str>,
    subject: Option<(SubjectKind, &str)>,
) -> String {
    let mut conn = harness.pool().acquire().await.unwrap();
    record_notification_event(
        &mut conn,
        NotificationEventInput {
            kind: "incident",
            incident_id,
            rule_key: Some("node.rpc_unreachable"),
            subject,
            severity: "warning",
            summary: "Incident opened: test",
        },
        &harness.channels,
        auth::now_utc(),
    )
    .await
    .unwrap()
}

async fn delivery_state(
    harness: &Harness,
    event_id: &str,
) -> (String, Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT state, last_result, next_attempt_at FROM notification_deliveries WHERE event_id = ?",
    )
    .bind(event_id)
    .fetch_one(harness.pool())
    .await
    .unwrap()
}

async fn incident_deleted_at(harness: &Harness, incident_id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT subject_deleted_at FROM alert_incidents WHERE incident_id = ?")
        .bind(incident_id)
        .fetch_one(harness.pool())
        .await
        .unwrap()
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

async fn remove_agent(
    harness: &Harness,
    session: &Session,
    agent_id: &str,
    confirmed: &[String],
) -> Value {
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{agent_id}/removal"),
            session,
            &serde_json::json!({
                "confirmAgentId": agent_id,
                "confirmedNodeIds": confirmed,
            })
            .to_string(),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

/// A Purge cancels only the not-yet-sent notifications of the deleted Node,
/// annotates its retained Incidents without faking a recovery, removes it from
/// current evaluation, and leaves every other subject untouched.
#[tokio::test]
async fn purge_cancels_unsent_notifications_and_annotates_incidents_without_false_recovery() {
    let harness = Harness::boot().await;
    let owner = login(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &owner).await;
    let report = fixture_report(&agent_id, 1);
    let node_id = report.inventory.nodes[0].node_id.to_string();
    submit_accepted(&harness, &credential, &report).await;

    // The deleted Node's own open Incident, plus a shared Validator and the
    // owning Agent's Incident that must not be touched.
    seed_incident(
        &harness,
        "inc-node",
        "node.rpc_unreachable",
        "node",
        &node_id,
    )
    .await;
    seed_incident(&harness, "inc-agent", "agent.offline", "agent", &agent_id).await;
    seed_incident(
        &harness,
        "inc-validator",
        "validator.ranking_changed",
        "validator",
        "validator-a",
    )
    .await;
    seed_rule_state(&harness, "node.rpc_unreachable", "node", &node_id).await;
    seed_rule_state(&harness, "agent.offline", "agent", &agent_id).await;

    // node_id: pending open Event, pending resolution Event with no
    // incident_id, an already-succeeded Event, and an in-flight Event.
    let node_open = seed_event(
        &harness,
        Some("inc-node"),
        Some((SubjectKind::Node, &node_id)),
    )
    .await;
    let node_resolution = seed_event(&harness, None, Some((SubjectKind::Node, &node_id))).await;
    let node_sent = seed_event(&harness, None, Some((SubjectKind::Node, &node_id))).await;
    let node_in_flight = seed_event(&harness, None, Some((SubjectKind::Node, &node_id))).await;
    // Other subjects: a sibling Node, the owning Agent, and a shared Validator.
    let sibling = seed_event(&harness, None, Some((SubjectKind::Node, "sibling-node"))).await;
    let agent_event = seed_event(&harness, None, Some((SubjectKind::Agent, &agent_id))).await;
    let validator_event = seed_event(
        &harness,
        None,
        Some((SubjectKind::Validator, "validator-a")),
    )
    .await;

    sqlx::query("UPDATE notification_deliveries SET state = 'succeeded', last_result = 'ok' WHERE event_id = ?")
        .bind(&node_sent)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE notification_deliveries SET state = 'in_flight' WHERE event_id = ?")
        .bind(&node_in_flight)
        .execute(harness.pool())
        .await
        .unwrap();

    purge_node(&harness, &owner, &node_id).await;

    // Unsent Deliveries are cancelled; the resolution Event with no
    // incident_id is covered because matching is by subject, not Incident.
    for event in [&node_open, &node_resolution] {
        let (state, last_result, next_attempt_at) = delivery_state(&harness, event).await;
        assert_eq!(state, "cancelled", "unsent Delivery must be cancelled");
        assert_eq!(last_result.as_deref(), Some("cancelled_subject_deleted"));
        assert_eq!(next_attempt_at, None);
    }
    // Already-sent and already-handed-off facts are never claimed as recalled.
    assert_eq!(delivery_state(&harness, &node_sent).await.0, "succeeded");
    assert_eq!(
        delivery_state(&harness, &node_in_flight).await.0,
        "in_flight"
    );
    // Other subjects keep their notification policy.
    for event in [&sibling, &agent_event, &validator_event] {
        assert_eq!(delivery_state(&harness, event).await.0, "pending");
    }

    // The Owner-only mutation is audited together with what it retired.
    let after_json: String = sqlx::query_scalar(
        "SELECT after_json FROM audit_events WHERE event_kind = 'node_purged' AND target_id = ?",
    )
    .bind(&node_id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    let after: Value = serde_json::from_str(&after_json).unwrap();
    assert_eq!(after["deliveries_cancelled"], 2);
    assert!(after["incidents_annotated"].as_i64().unwrap() >= 1);

    // The retained Incident keeps its open state and gains the deletion
    // annotation; the other subjects' Incidents are unannotated.
    assert!(incident_deleted_at(&harness, "inc-node").await.is_some());
    assert_eq!(incident_deleted_at(&harness, "inc-agent").await, None);
    assert_eq!(incident_deleted_at(&harness, "inc-validator").await, None);
    let node_incident_state: String =
        sqlx::query_scalar("SELECT state FROM alert_incidents WHERE incident_id = 'inc-node'")
            .fetch_one(harness.pool())
            .await
            .unwrap();
    assert_eq!(
        node_incident_state, "open",
        "an open Incident is never faked into resolved"
    );

    // The Node leaves current evaluation; other subjects keep theirs.
    let node_states: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM alert_rule_state WHERE subject_kind = 'node' AND subject_key = ?",
    )
    .bind(&node_id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(node_states, 0);
    let agent_states: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM alert_rule_state WHERE subject_kind = 'agent' AND subject_key = ?",
    )
    .bind(&agent_id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert!(agent_states >= 1);

    // The Incident evidence is still listed, now annotated; it no longer
    // counts as a current open problem for the Rule summary.
    let list = body_json(
        harness
            .send(admin_get(
                "/api/admin/v1/alerts/incidents?state=open",
                &owner,
            ))
            .await,
    )
    .await;
    let listed = list["incidents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|incident| incident["incidentId"] == "inc-node")
        .expect("retained Incident evidence must still be listed");
    assert_eq!(listed["state"], "open");
    assert!(listed["subjectDeletedAt"].is_string());

    let rules = body_json(
        harness
            .send(admin_get("/api/admin/v1/alerts/rules", &owner))
            .await,
    )
    .await;
    let node_rule = rules
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["ruleKey"] == "node.rpc_unreachable")
        .unwrap();
    assert_eq!(
        node_rule["openIncidents"], 0,
        "a deleted subject must not count as a current open problem"
    );
}

/// An Agent Removal cancels the Agent, Host, and owned-Node notifications and
/// annotates their Incidents, while another Agent's subjects are untouched.
#[tokio::test]
async fn agent_removal_retires_agent_host_and_owned_nodes_only() {
    let harness = Harness::boot().await;
    let owner = login(&harness).await;
    let (agent_a, credential_a) = enroll_agent(&harness, &owner).await;
    submit_accepted(
        &harness,
        &credential_a,
        &multi_node_report(&agent_a, 1, 1, REPORT_A, 1, 1, &[NODE_A]),
    )
    .await;
    let (agent_b, credential_b) = enroll_agent(&harness, &owner).await;
    submit_accepted(
        &harness,
        &credential_b,
        &multi_node_report(&agent_b, 1, 1, REPORT_B, 1, 10, &[NODE_B]),
    )
    .await;

    seed_incident(&harness, "inc-a-agent", "agent.offline", "agent", &agent_a).await;
    seed_incident(
        &harness,
        "inc-a-host",
        "host.memory_pressure",
        "host",
        &agent_a,
    )
    .await;
    seed_incident(
        &harness,
        "inc-a-node",
        "node.rpc_unreachable",
        "node",
        NODE_A,
    )
    .await;
    seed_incident(&harness, "inc-b-agent", "agent.offline", "agent", &agent_b).await;
    seed_incident(
        &harness,
        "inc-b-node",
        "node.rpc_unreachable",
        "node",
        NODE_B,
    )
    .await;
    seed_rule_state(&harness, "agent.offline", "agent", &agent_a).await;
    seed_rule_state(&harness, "agent.offline", "agent", &agent_b).await;

    let a_agent = seed_event(&harness, None, Some((SubjectKind::Agent, &agent_a))).await;
    let a_host = seed_event(&harness, None, Some((SubjectKind::Host, &agent_a))).await;
    let a_node = seed_event(&harness, None, Some((SubjectKind::Node, NODE_A))).await;
    let a_sent = seed_event(&harness, None, Some((SubjectKind::Agent, &agent_a))).await;
    let b_agent = seed_event(&harness, None, Some((SubjectKind::Agent, &agent_b))).await;
    let b_node = seed_event(&harness, None, Some((SubjectKind::Node, NODE_B))).await;
    sqlx::query("UPDATE notification_deliveries SET state = 'succeeded', last_result = 'ok' WHERE event_id = ?")
        .bind(&a_sent)
        .execute(harness.pool())
        .await
        .unwrap();

    remove_agent(&harness, &owner, &agent_a, &[NODE_A.to_owned()]).await;

    for event in [&a_agent, &a_host, &a_node] {
        let (state, last_result, _) = delivery_state(&harness, event).await;
        assert_eq!(state, "cancelled");
        assert_eq!(last_result.as_deref(), Some("cancelled_subject_deleted"));
    }
    assert_eq!(delivery_state(&harness, &a_sent).await.0, "succeeded");
    assert_eq!(delivery_state(&harness, &b_agent).await.0, "pending");
    assert_eq!(delivery_state(&harness, &b_node).await.0, "pending");

    for incident in ["inc-a-agent", "inc-a-host", "inc-a-node"] {
        assert!(
            incident_deleted_at(&harness, incident).await.is_some(),
            "{incident} must be annotated as subject-deleted"
        );
    }
    for incident in ["inc-b-agent", "inc-b-node"] {
        assert_eq!(
            incident_deleted_at(&harness, incident).await,
            None,
            "{incident} belongs to another Agent and must be untouched"
        );
    }

    let removed_states: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM alert_rule_state WHERE (subject_kind = 'agent' AND subject_key = ?) OR (subject_kind = 'host' AND subject_key = ?)",
    )
    .bind(&agent_a)
    .bind(&agent_a)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(removed_states, 0);
    let surviving_states: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM alert_rule_state WHERE subject_kind = 'agent' AND subject_key = ?",
    )
    .bind(&agent_b)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(surviving_states, 1);
}

/// A Delivery that only becomes due after its subject was deleted is never
/// handed to a provider: the worker's pre-send check cancels it, while a live
/// subject's Delivery is still sent.
#[tokio::test]
async fn delivery_worker_never_hands_a_deleted_subjects_message_to_a_provider() {
    let harness = Harness::boot().await;
    let owner = login(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &owner).await;
    let report = fixture_report(&agent_id, 1);
    let node_id = report.inventory.nodes[0].node_id.to_string();
    submit_accepted(&harness, &credential, &report).await;
    purge_node(&harness, &owner, &node_id).await;

    // Simulate a Delivery that became due only after the deletion (crash
    // requeue or a send that failed mid-deletion).
    let deleted_event = seed_event(&harness, None, Some((SubjectKind::Node, &node_id))).await;
    sqlx::query("UPDATE notification_deliveries SET state = 'retry_scheduled', next_attempt_at = '2020-01-01T00:00:00Z' WHERE event_id = ?")
        .bind(&deleted_event)
        .execute(harness.pool())
        .await
        .unwrap();
    let live_event = seed_event(&harness, None, Some((SubjectKind::Node, "live-node"))).await;

    let processed = process_due_deliveries(&harness.state, &*harness.provider)
        .await
        .unwrap();
    assert!(processed >= 1);

    let sends = harness.provider.sends();
    assert_eq!(
        sends.len(),
        1,
        "only the live subject may be sent: {sends:?}"
    );
    assert!(sends[0].contains("live-node"));
    assert!(!sends[0].contains(&node_id));

    let (deleted_state, deleted_result, _) = delivery_state(&harness, &deleted_event).await;
    assert_eq!(deleted_state, "cancelled");
    assert_eq!(deleted_result.as_deref(), Some("cancelled_subject_deleted"));
    assert_eq!(delivery_state(&harness, &live_event).await.0, "succeeded");
}
