//! Agent Attention Acknowledgment acceptance through the real router
//! (issue #172, part of #165).
//!
//! These tests build the full application with build_app against a temporary
//! SQLite database, log in with real Owner sessions, and exercise the
//! Overview, Agent detail, and acknowledgment endpoints over HTTP. They
//! assert the durable, shared outcome: one occurrence disappears on both
//! surfaces and for a second Owner, ordinary refreshes and a Server restart
//! do not resurrect it, new evidence re-prompts, a stale request does not
//! swallow it, Node prompts are never acknowledged, liveness/health are
//! unchanged, and the Audit Event records the accountable facts.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_server::{AppState, auth, database, http, secrets};

const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const SECOND_OWNER_LOGIN: &str = r#"{"username":"second","password":"second owner password"}"#;

const AGENT_ID: &str = "0195f2a1-0072-4072-8072-000000000072";
const NODE_ID: &str = "0195f2a1-0073-4073-8073-000000000073";
const CREATED: &str = "2026-01-01T00:00:00Z";

struct Harness {
    dir: TempDir,
    state: AppState,
    app: Router,
}

impl Harness {
    async fn boot() -> Self {
        let dir = TempDir::new().unwrap();
        let harness = Self::open(dir).await;
        let owner_hash = auth::hash_password(b"correct horse battery").unwrap();
        auth::create_owner(harness.state.db(), "admin", &owner_hash)
            .await
            .unwrap();
        let second_hash = auth::hash_password(b"second owner password").unwrap();
        auth::create_owner(harness.state.db(), "second", &second_hash)
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

/// Seed one stale Agent (offline) with recorded security events and one
/// unhealthy Node so the Overview carries both Agent and Node prompts.
async fn seed(harness: &Harness) {
    let stale = auth::format_rfc3339(auth::now_utc() - time::Duration::hours(1));
    sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at, last_received_at, shutdown_state, security_event_count) VALUES (?, 1, ?, ?, ?, 'running', 3)")
        .bind(AGENT_ID)
        .bind(CREATED)
        .bind(CREATED)
        .bind(&stale)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main', '0xgenesis', 1, 1, 'lat', ?, ?)")
        .bind(CREATED)
        .bind(CREATED)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, ?, 'mainnet', 'Node A', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
        .bind(NODE_ID)
        .bind(AGENT_ID)
        .bind(CREATED)
        .bind(CREATED)
        .execute(harness.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO current_node_chain_observations (node_id, current_block, syncing, updated_at) VALUES (?, 100, 0, ?)")
        .bind(NODE_ID)
        .bind(&stale)
        .execute(harness.pool())
        .await
        .unwrap();
    for component in ["rpc", "sync", "consensus", "network_identity"] {
        let component_state = if component == "rpc" { "error" } else { "ok" };
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision) VALUES (?, 'node', ?, ?, ?, ?, ?, ?, ?, 1, 1)")
            .bind(AGENT_ID)
            .bind(NODE_ID)
            .bind(NODE_ID)
            .bind(component)
            .bind(component_state)
            .bind(&stale)
            .bind(&stale)
            .bind(&stale)
            .execute(harness.pool())
            .await
            .unwrap();
    }
}

fn agent_items(overview: &Value) -> Vec<&Value> {
    overview["attention"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["subject_kind"] == "agent")
        .collect()
}

fn find_item<'a>(items: &'a [&'a Value], kind: &str) -> Option<&'a Value> {
    items.iter().copied().find(|item| item["kind"] == kind)
}

async fn overview(harness: &Harness, session: &Session) -> Value {
    let response = harness
        .send(admin_get("/api/admin/v1/overview", session))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

async fn agent_detail(harness: &Harness, session: &Session) -> Value {
    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/agents/{AGENT_ID}"),
            session,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

async fn acknowledge(
    harness: &Harness,
    session: &Session,
    items: &[(&str, &str)],
) -> (StatusCode, Value) {
    let body = serde_json::json!({
        "items": items
            .iter()
            .map(|(kind, evidence_key)| serde_json::json!({ "kind": kind, "evidence_key": evidence_key }))
            .collect::<Vec<_>>(),
    });
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{AGENT_ID}/attention/acknowledgments"),
            session,
            &body.to_string(),
        ))
        .await;
    let status = response.status();
    (status, body_json(response).await)
}

fn evidence_key(item: &Value) -> &str {
    item["evidence_key"].as_str().unwrap()
}

#[tokio::test]
async fn acknowledgment_is_shared_durable_and_rearms_only_on_new_evidence() {
    let harness = Harness::boot().await;
    seed(&harness).await;
    let owner = login(&harness, OWNER_LOGIN).await;

    let before = overview(&harness, &owner).await;
    let items = agent_items(&before);
    assert_eq!(
        items.len(),
        2,
        "offline and security-event prompts expected"
    );
    let offline_key = evidence_key(find_item(&items, "agent_offline").unwrap()).to_owned();
    let security_key = evidence_key(find_item(&items, "agent_security_event").unwrap()).to_owned();
    assert!(find_item(&items, "node_unhealthy").is_none());
    assert_eq!(
        before["attention"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["subject_kind"] == "node")
            .count(),
        1,
        "the unhealthy Node prompt is present"
    );

    // Acknowledge only the security event; the offline prompt must survive.
    let (status, ack) =
        acknowledge(&harness, &owner, &[("agent_security_event", &security_key)]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ack["acknowledged"].as_array().unwrap().len(), 1);
    assert!(ack["skipped"].as_array().unwrap().is_empty());
    assert_eq!(ack["attention"].as_array().unwrap().len(), 1);

    let after = overview(&harness, &owner).await;
    let items = agent_items(&after);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "agent_offline");
    // The Node prompt is untouched and never acknowledgeable.
    assert_eq!(
        after["attention"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["subject_kind"] == "node")
            .count(),
        1
    );
    // Raw evidence and liveness remain truthful in Agent detail.
    let detail = agent_detail(&harness, &owner).await;
    assert_eq!(detail["liveness"], "offline");
    assert_eq!(detail["security_event_count"], 3);
    assert_eq!(detail["attention"].as_array().unwrap().len(), 1);
    assert_eq!(detail["attention"][0]["kind"], "agent_offline");

    // A second Owner sees the same shared result.
    let second = login(&harness, SECOND_OWNER_LOGIN).await;
    let second_overview = overview(&harness, &second).await;
    assert_eq!(agent_items(&second_overview).len(), 1);

    // The offline prompt is acknowledged too; then no Agent prompt remains
    // for either Owner, while the Node prompt stays.
    let (status, _ack) = acknowledge(&harness, &owner, &[("agent_offline", &offline_key)]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(agent_items(&overview(&harness, &owner).await).is_empty());
    assert!(agent_items(&overview(&harness, &second).await).is_empty());

    // A Server restart does not resurrect an acknowledged occurrence.
    let harness = harness.restart().await;
    assert!(agent_items(&overview(&harness, &owner).await).is_empty());

    // New evidence (a fresh security event) re-prompts with a new boundary.
    sqlx::query("UPDATE agents SET security_event_count = 4 WHERE agent_id = ?")
        .bind(AGENT_ID)
        .execute(harness.pool())
        .await
        .unwrap();
    let rearmed = overview(&harness, &owner).await;
    let items = agent_items(&rearmed);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "agent_security_event");
    assert_ne!(evidence_key(items[0]), security_key);

    // A stale request carrying the old boundary is skipped and cannot swallow
    // the newer unseen evidence.
    let (status, ack) =
        acknowledge(&harness, &owner, &[("agent_security_event", &security_key)]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(ack["acknowledged"].as_array().unwrap().is_empty());
    assert_eq!(ack["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(ack["attention"].as_array().unwrap().len(), 1);

    // The authoritative Audit Event records the accountable facts.
    let events: Vec<(String, String)> = sqlx::query_as(
        "SELECT event_kind, target_id FROM audit_events WHERE event_kind = 'agent_attention_acknowledged' ORDER BY audit_event_id",
    )
    .fetch_all(harness.pool())
    .await
    .unwrap();
    assert_eq!(events.len(), 2, "one audit row per applied acknowledgment");
    assert!(
        events
            .iter()
            .all(|(kind, target)| kind == "agent_attention_acknowledged" && target == AGENT_ID)
    );
}

#[tokio::test]
async fn bulk_acknowledgment_covers_only_the_displayed_boundaries() {
    let harness = Harness::boot().await;
    seed(&harness).await;
    let owner = login(&harness, OWNER_LOGIN).await;
    let before = overview(&harness, &owner).await;
    let items = agent_items(&before);
    let requests: Vec<(String, String)> = items
        .iter()
        .map(|item| {
            (
                item["kind"].as_str().unwrap().to_owned(),
                item["evidence_key"].as_str().unwrap().to_owned(),
            )
        })
        .collect();

    // Bulk confirmation of exactly what the Owner saw: both Agent items.
    let body = serde_json::json!({
        "items": requests
            .iter()
            .map(|(kind, evidence_key)| serde_json::json!({ "kind": kind, "evidence_key": evidence_key }))
            .collect::<Vec<_>>(),
    });
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{AGENT_ID}/attention/acknowledgments"),
            &owner,
            &body.to_string(),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let ack = body_json(response).await;
    assert_eq!(ack["acknowledged"].as_array().unwrap().len(), 2);
    assert!(ack["attention"].as_array().unwrap().is_empty());

    // A kind that is not a current Agent item (a Node kind) is never applied.
    let (status, skipped) = acknowledge(&harness, &owner, &[("node_unhealthy", NODE_ID)]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(skipped["acknowledged"].as_array().unwrap().is_empty());
    assert_eq!(skipped["skipped"].as_array().unwrap().len(), 1);
}
