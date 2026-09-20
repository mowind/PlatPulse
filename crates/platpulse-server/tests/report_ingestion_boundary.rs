//! Report Ingestion and Agent Enrollment acceptance through the real router
//! (issue #166, part of #165).
//!
//! The existing `report_ingestion` unit tests drive the handler directly and
//! inject a synthetic `AgentAuthInfo`; those tests stay in place. This suite
//! exercises the same boundary the way a real Agent does: it builds the full
//! application with `build_app` against a temporary SQLite database, mints an
//! Enrollment Token through the Admin HTTP API, exchanges it for a real Agent
//! Credential, and submits reports over HTTP with that credential. It also
//! settles an ownership transfer race end to end, so credential rejection and
//! ownership resolution are proven at the HTTP/router boundary rather than by
//! injecting a Server-internal identity.
//!
//! Every test owns its own `TempDir` and database, so nothing is shared with
//! the parallel unit tests.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_core::block::{BlockProductionAttribution, BlockSource, BlockSummary};
use platpulse_core::{
    AgentReport, NodeCurrentDisposition, ReceiptDisposition, RejectionCode, ReportReceipt,
    SampleDispositionKind,
};
use platpulse_server::{AppState, auth, database, http, network, secrets};

/// Registered Network tuple for the `platon-mainnet` key the report fixture
/// declares, matching the unit-test registry exactly.
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;

/// A fresh Server (real temp SQLite + pepper), the full router, and the
/// pieces needed to authenticate as the Owner.
struct Harness {
    _dir: TempDir,
    state: AppState,
    app: Router,
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
        let state = AppState::new(database, None, auth);
        // The Agent group refuses every request until the first Owner
        // exists (design §12.2), so the acceptance path must start here.
        let hash = auth::hash_password(b"correct horse battery").unwrap();
        auth::create_owner(state.db(), "admin", &hash)
            .await
            .unwrap();
        network::create_network(
            state.db(),
            "platon-mainnet",
            "PlatON Mainnet",
            NETWORK_GENESIS,
            210425,
            210425,
            "lat",
        )
        .await
        .unwrap();
        let app = http::build_app(state.clone());
        Self {
            _dir: dir,
            state,
            app,
        }
    }

    async fn send(&self, request: Request<Body>) -> axum::response::Response {
        self.app.clone().oneshot(request).await.unwrap()
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// An authenticated Owner session: the cookie and the CSRF token every
/// Admin mutation must present together.
struct Session {
    cookie: String,
    csrf: String,
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

/// Log in through the real login route and return the authenticated Owner
/// session the Admin mutations require.
async fn owner_session(harness: &Harness) -> Session {
    let request = Request::builder()
        .method("POST")
        .uri("/api/public/v1/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
        .body(Body::from(OWNER_LOGIN_BODY))
        .unwrap();
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_owned();
    let body = body_json(response).await;
    let csrf = body["csrfToken"].as_str().unwrap().to_owned();
    Session { cookie, csrf }
}

/// Mint an Enrollment Token through the Admin API and exchange it for a real
/// Agent identity and credential through the Agent API.
async fn enroll_agent(harness: &Harness, session: &Session) -> (String, String) {
    let response = harness
        .send(admin_post(
            "/api/admin/v1/agents/enroll-token",
            session,
            r#"{"expiresInHours": 24}"#,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let token = body["token"].as_str().unwrap().to_owned();

    let response = harness
        .send(bearer_post("/api/agent/v1/enroll", &token, Vec::new()))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    (
        body["agent_id"].as_str().unwrap().to_owned(),
        body["credential"].as_str().unwrap().to_owned(),
    )
}

/// Submit a report over the real Agent route. A `None` credential exercises
/// the missing-credential path.
async fn submit(harness: &Harness, credential: Option<&str>, body: Vec<u8>) -> (StatusCode, Value) {
    let request = match credential {
        Some(token) => bearer_post("/api/agent/v1/reports", token, body),
        None => Request::builder()
            .method("POST")
            .uri("/api/agent/v1/reports")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap(),
    };
    let response = harness.send(request).await;
    let status = response.status();
    (status, body_json(response).await)
}

fn receipt_from(value: &Value) -> ReportReceipt {
    serde_json::from_value(value["receipt"].clone()).unwrap()
}

/// The minimal wire fixture, re-bound to the enrolled Agent identity the
/// Server actually minted (the fixture predates Server-side enrollment and
/// carries a static id).
fn fixture_report(agent_id: &str, agent_epoch: u64) -> AgentReport {
    let mut report: AgentReport = serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    report.agent_id = agent_id.parse().unwrap();
    report.agent_epoch = agent_epoch;
    report
}

/// Build the transfer target's declaration of the source-owned Node, with a
/// Network identity that matches the registered Registry tuple (unlike the
/// fixture, whose identity deliberately contradicts it).
fn target_declaration(source_report: &AgentReport, target_agent: &str) -> AgentReport {
    let mut report = source_report.clone();
    report.agent_id = target_agent.parse().unwrap();
    report.boot_id = "0195f2a1-0034-4034-8034-000000000034".parse().unwrap();
    report.previous_boot_id = None;
    report.report_sequence = 1;
    report.report_id = "0195f2a1-0035-4035-8035-000000000035".parse().unwrap();
    let identity = report.nodes[0]
        .chain
        .network_identity
        .latest
        .as_mut()
        .unwrap();
    identity.genesis_hash = NETWORK_GENESIS.parse().unwrap();
    identity.address_hrp = Some("lat".to_owned());
    report.block_summaries.push(BlockSummary {
        node_id: report.inventory.nodes[0].node_id,
        network_identity: report.nodes[0]
            .chain
            .network_identity
            .latest
            .clone()
            .unwrap(),
        block_number: 10,
        block_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .parse()
            .unwrap(),
        parent_hash: "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .parse()
            .unwrap(),
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
    });
    report.validate().unwrap();
    report
}

/// A real credential is bound to the Agent identity the Server minted: the
/// report is accepted, ownership is persisted, an exact replay is idempotent,
/// and the same credential cannot declare a different Agent.
#[tokio::test]
async fn real_credential_report_round_trip_is_bound_to_the_enrolled_identity() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;

    let report = fixture_report(&agent_id, 1);
    let body = serde_json::to_vec(&report).unwrap();
    let (status, value) = submit(&harness, Some(&credential), body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let receipt = receipt_from(&value);
    assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
    assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Accepted);

    // The Node projection is owned by the Agent the credential resolved to.
    let node_id = report.inventory.nodes[0].node_id.to_string();
    let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id = ?")
        .bind(&node_id)
        .fetch_one(harness.state.db().pool())
        .await
        .unwrap();
    assert_eq!(owner, agent_id);
    let receipts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_report_receipts WHERE report_id = ?")
            .bind(report.report_id.to_string())
            .fetch_one(harness.state.db().pool())
            .await
            .unwrap();
    assert_eq!(receipts, 1);

    // An exact replay returns the stored immutable receipt rather than
    // re-applying the report.
    let (status, replay) = submit(&harness, Some(&credential), body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["receipt"], value["receipt"]);

    // The credential is scoped to its Agent: a report naming another
    // identity is refused before any projection changes.
    let mut foreign = report.clone();
    foreign.agent_id = "0195f2a1-0099-4099-8099-000000000099".parse().unwrap();
    foreign.report_id = "0195f2a1-0099-4099-8099-000000000098".parse().unwrap();
    let (status, body) = submit(
        &harness,
        Some(&credential),
        serde_json::to_vec(&foreign).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "agent_identity_mismatch");
}

/// Missing, unknown, and one-time Enrollment Tokens are rejected by the real
/// router before the report handler runs.
#[tokio::test]
async fn router_rejects_missing_unknown_and_enrollment_credentials() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, _credential) = enroll_agent(&harness, &session).await;
    let body = serde_json::to_vec(&fixture_report(&agent_id, 1)).unwrap();

    let (status, value) = submit(&harness, None, body.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "agent_auth_required");

    let (status, value) = submit(&harness, Some("pp_agent_unknown_abc"), body.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "agent_auth_required");

    // A fresh Enrollment Token is not an Agent Credential.
    let response = harness
        .send(admin_post(
            "/api/admin/v1/agents/enroll-token",
            &session,
            r#"{"expiresInHours": 24}"#,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let token = body_json(response).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, value) = submit(&harness, Some(&token), body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "agent_auth_required");
}

/// Rotation with an immediate revoke invalidates the old credential at the
/// router boundary while the replacement keeps reporting.
#[tokio::test]
async fn rotated_out_credential_is_rejected_but_the_replacement_still_reports() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let body = serde_json::to_vec(&fixture_report(&agent_id, 1)).unwrap();
    let (status, _) = submit(&harness, Some(&credential), body.clone()).await;
    assert_eq!(status, StatusCode::OK);

    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/agents/{agent_id}/credentials/rotate"),
            &session,
            r#"{"revokePrevious": true}"#,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let rotated = body_json(response).await;
    let replacement = rotated["credential"].as_str().unwrap().to_owned();
    assert!(
        rotated["revoked_previous_ids"]
            .as_array()
            .is_some_and(|ids| !ids.is_empty()),
        "rotation must revoke the previous credential: {rotated}"
    );

    let (status, value) = submit(&harness, Some(&credential), body.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{value}");
    assert_eq!(value["error"]["code"], "agent_auth_required");

    let (status, _) = submit(&harness, Some(&replacement), body).await;
    assert_eq!(status, StatusCode::OK);
}

/// The ownership transfer race is settled by the router: a matching target
/// declaration completes the transfer, and the source's later declaration is
/// rejected per Node with a recorded security event.
#[tokio::test]
async fn pending_transfer_race_is_settled_at_the_router_boundary() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (source_agent, source_credential) = enroll_agent(&harness, &session).await;
    let (target_agent, target_credential) = enroll_agent(&harness, &session).await;

    let source_report = fixture_report(&source_agent, 1);
    let node_id = source_report.inventory.nodes[0].node_id.to_string();
    let (status, value) = submit(
        &harness,
        Some(&source_credential),
        serde_json::to_vec(&source_report).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        receipt_from(&value).disposition,
        ReceiptDisposition::Accepted
    );

    // The Owner opens the two-phase Transfer through the real Admin route;
    // only the target's matching declaration switches ownership.
    let transfer_response = harness
        .send(admin_post(
            &format!("/api/admin/v1/nodes/{node_id}/transfers"),
            &session,
            &format!(
                r#"{{"targetAgentId":"{target_agent}","expiresInHours":24,"operatorReason":"move the validator"}}"#
            ),
        ))
        .await;
    assert_eq!(transfer_response.status(), StatusCode::OK);
    assert_eq!(
        body_json(transfer_response).await["transfer"]["status"],
        "pending"
    );

    // The target's matching declaration wins the Node atomically.
    let declaration = target_declaration(&source_report, &target_agent);
    let (status, value) = submit(
        &harness,
        Some(&target_credential),
        serde_json::to_vec(&declaration).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let receipt = receipt_from(&value);
    assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
    assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Accepted);
    assert_eq!(
        receipt.samples[0].disposition,
        SampleDispositionKind::Accepted
    );
    let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id = ?")
        .bind(&node_id)
        .fetch_one(harness.state.db().pool())
        .await
        .unwrap();
    assert_eq!(owner, target_agent);
    let transfer: String =
        sqlx::query_scalar("SELECT status FROM node_transfers WHERE node_id = ?")
            .bind(&node_id)
            .fetch_one(harness.state.db().pool())
            .await
            .unwrap();
    assert_eq!(transfer, "completed");

    // The source's next legitimate report loses only that Node entry.
    let mut stale_source = source_report.clone();
    stale_source.report_sequence = 2;
    stale_source.report_id = "0195f2a1-0013-4013-8013-000000000301".parse().unwrap();
    let (status, value) = submit(
        &harness,
        Some(&source_credential),
        serde_json::to_vec(&stale_source).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let receipt = receipt_from(&value);
    assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
    assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
    assert_eq!(
        receipt.nodes[0].rejections[0].code,
        RejectionCode::NodeOwnershipMismatch
    );
    let events: i64 =
        sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id = ?")
            .bind(&source_agent)
            .fetch_one(harness.state.db().pool())
            .await
            .unwrap();
    assert_eq!(events, 1);
}
