//! The Storage Backend Migration end to end (ADR 0013, issue #257).
//!
//! The unit tests in `platpulse_server::storage_migration` seed a SQLite file
//! with direct SQL and check the copy table by table. This suite does the
//! thing an Operator actually does: a real Server ingests a real Agent Report
//! over HTTP into a SQLite deployment, the offline migration copies that file
//! into PostgreSQL, and the Server then serves the same Public and Admin
//! responses from PostgreSQL. The copy also has to prove that a replay stays a
//! replay, that a report identity still conflicts on different bytes, and that
//! the first insert after the migration does not collide with a primary key
//! the copy left behind.
//!
//! The source is always SQLite here, even under PLATPULSE_TEST_STORAGE=postgres:
//! the migration exists to move a SQLite deployment onto PostgreSQL, so the
//! suite builds its own SQLite deployment instead of asking the shared helper
//! for a test database. The PostgreSQL target comes from
//! PLATPULSE_TEST_POSTGRES_URL; without it the test reports itself skipped
//! instead of passing on a backend it never exercised.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_core::{AgentReport, ReceiptDisposition, ReportReceipt};
use platpulse_server::{
    AppState, ServerDatabase, ServerDatabaseConfig, auth, http, network, secrets, storage_migration,
};

/// Registered Network tuple for the `platon-mainnet` key the report fixture
/// declares, matching the rest of the Server suites.
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;

/// Every Public and Admin read the migration must preserve, taken over the one
/// Node the ingested report declared.
///
/// The audit and session surfaces are deliberately absent: the second Server
/// logs in again, which is a real write of its own, so a "nothing changed"
/// comparison over them would be honest but meaningless.
fn parity_uris(node_id: &str) -> Vec<String> {
    vec![
        "/api/public/v1/networks".to_owned(),
        format!("/api/public/v1/nodes/{node_id}"),
        format!("/api/public/v1/nodes/{node_id}/metrics"),
        format!("/api/public/v1/nodes/{node_id}/history"),
        "/api/admin/v1/overview".to_owned(),
        "/api/admin/v1/nodes".to_owned(),
        format!("/api/admin/v1/nodes/{node_id}"),
        "/api/admin/v1/agents".to_owned(),
        "/api/admin/v1/alerts/rules".to_owned(),
    ]
}

struct Harness {
    state: AppState,
    app: Router,
}

impl Harness {
    /// Boot the full Server over an already opened database. Only the first
    /// boot creates the Owner and the Network: after the migration both are
    /// already in the copy, and re-creating them is exactly the kind of write
    /// this suite must not perform.
    async fn boot(dir: &std::path::Path, database: ServerDatabase, first_boot: bool) -> Self {
        let pepper_path = dir.join("server-pepper");
        if !pepper_path.exists() {
            secrets::create_pepper_file(&pepper_path).unwrap();
        }
        let auth = auth::AuthConfig::development(
            secrets::load_pepper_file(&pepper_path).unwrap(),
            DEVELOPMENT_ORIGIN.to_owned(),
        );
        let state = AppState::new(database, None, auth);
        if first_boot {
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
        }
        Self {
            app: http::build_app(state.clone()),
            state,
        }
    }

    async fn send(&self, request: Request<Body>) -> axum::response::Response {
        self.app.clone().oneshot(request).await.unwrap()
    }

    async fn get(&self, uri: &str, session: &Session) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("GET")
            .uri(uri)
            .header(header::COOKIE, &session.cookie)
            .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
            .body(Body::empty())
            .unwrap();
        let response = self.send(request).await;
        let status = response.status();
        (status, body_json(response).await)
    }

    /// Every Public and Admin read the migration must preserve: the answering
    /// status and the stored content, clock aside.
    async fn parity(&self, session: &Session, node_id: &str) -> Vec<(String, u16, Value)> {
        let mut reads = Vec::new();
        for uri in parity_uris(node_id) {
            let (status, body) = self.get(&uri, session).await;
            reads.push((uri, status.as_u16(), masked(&body)));
        }
        reads
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// An authenticated Owner session: the cookie and the CSRF token every Admin
/// mutation must present together.
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

async fn submit(harness: &Harness, credential: &str, body: Vec<u8>) -> (StatusCode, Value) {
    let response = harness
        .send(bearer_post("/api/agent/v1/reports", credential, body))
        .await;
    let status = response.status();
    (status, body_json(response).await)
}

fn receipt_from(value: &Value) -> ReportReceipt {
    serde_json::from_value(value["receipt"].clone()).unwrap()
}

/// The minimal wire fixture, re-bound to the Agent identity the Server minted.
fn fixture_report(agent_id: &str, agent_epoch: u64) -> AgentReport {
    let mut report: AgentReport = serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    report.agent_id = agent_id.parse().unwrap();
    report.agent_epoch = agent_epoch;
    report
}

/// Remove the fields that are a function of the wall clock rather than of the
/// stored state: the two Server processes answered at different instants, so
/// "now", the ages derived from it, and the timestamps they render are the
/// only values a faithful copy is allowed to disagree on.
fn masked(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !is_clock_key(key))
                .map(|(key, value)| (key.clone(), masked(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(masked).collect()),
        Value::String(text) if is_timestamp(text) => Value::String("<clock>".to_owned()),
        other => other.clone(),
    }
}

fn is_clock_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "at",
        "time",
        "timestamp",
        "age",
        "seconds",
        "ms",
        "duration",
        "elapsed",
        "uptime",
    ]
    .iter()
    .any(|suffix| key.ends_with(suffix))
}

fn is_timestamp(text: &str) -> bool {
    text.len() >= 20 && text.as_bytes().get(4) == Some(&b'-') && text.contains('T')
}

#[tokio::test]
async fn an_ingested_sqlite_deployment_serves_the_same_responses_from_postgresql() {
    let Some(target) = platpulse_server::test_support::optional_postgres_connect_config().await
    else {
        eprintln!(
            "skipping the Storage Backend Migration end-to-end test: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
            platpulse_server::test_support::TEST_POSTGRES_URL_ENV
        );
        return;
    };
    let dir = TempDir::new().expect("a temporary directory");
    let source = ServerDatabaseConfig::new(dir.path().join("server.db"));

    // A real SQLite deployment, populated only through the HTTP API.
    let sqlite = platpulse_server::initialize(source.clone())
        .await
        .expect("open the SQLite deployment");
    let harness = Harness::boot(dir.path(), sqlite, true).await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let report = fixture_report(&agent_id, 1);
    let body = serde_json::to_vec(&report).unwrap();
    let (status, receipt) = submit(&harness, &credential, body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(
        receipt_from(&receipt).disposition,
        ReceiptDisposition::Accepted
    );
    let node_id = report.inventory.nodes[0].node_id.to_string();
    let before = harness.parity(&session, &node_id).await;

    // Stop the Source Server: the migration refuses a file another process owns.
    harness.state.db().close().await;
    drop(harness);

    // The offline migration the Operator runs with the Server stopped.
    let summary = storage_migration::migrate_deployment(&source, &target, &mut |_| {})
        .await
        .expect("migrate the ingested SQLite deployment");
    assert!(summary.row_count > 0, "{summary:?}");
    assert!(summary.table_count > 0, "{summary:?}");

    // The same Server, now on PostgreSQL.
    let postgres = ServerDatabase::open(target)
        .await
        .expect("open the migrated PostgreSQL deployment");
    let harness = Harness::boot(dir.path(), postgres, false).await;
    let session = owner_session(&harness).await;
    let after = harness.parity(&session, &node_id).await;
    assert_eq!(before.len(), after.len(), "parity reads are paired");
    for ((uri, before_status, before), (_, after_status, after)) in before.iter().zip(after.iter())
    {
        assert_eq!(
            before_status, after_status,
            "{uri} answers a different status after the migration"
        );
        assert_eq!(
            before, after,
            "{uri} differs after the migration:\nbefore {before}\nafter {after}"
        );
    }
    // The comparison is only worth anything if it compared a served
    // deployment: the Public Network list must answer, and it must carry the
    // Network this file registered.
    let (_, networks_status, networks) = after
        .iter()
        .find(|(uri, ..)| uri == "/api/public/v1/networks")
        .expect("the Public Network read is part of parity");
    assert_eq!(*networks_status, 200, "{networks}");
    assert!(
        networks.to_string().contains("platon-mainnet"),
        "the migrated Public Projection must still serve the Network: {networks}"
    );

    // An exact replay is still a replay: the stored receipt comes back and
    // nothing re-accumulates.
    let (status, replay) = submit(&harness, &credential, body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(
        replay["receipt"], receipt["receipt"],
        "a replay returns the stored receipt"
    );
    assert_eq!(
        harness.parity(&session, &node_id).await,
        after,
        "a replay must not re-accumulate"
    );

    // The same report identity with different bytes still conflicts.
    let mut changed = report.clone();
    changed.generated_at = "2026-08-12T09:00:01Z".parse().unwrap();
    let (status, conflict) =
        submit(&harness, &credential, serde_json::to_vec(&changed).unwrap()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(conflict["error"]["code"], "report_identity_conflict");

    // The first insert after the migration does not collide with a copied
    // identity: a new report takes the next sequence and its own rows.
    let mut next = report.clone();
    next.report_id = "0195f2a1-0077-4077-8077-000000000077".parse().unwrap();
    next.report_sequence = report.report_sequence + 1;
    next.generated_at = "2026-08-12T09:00:30Z".parse().unwrap();
    let (status, accepted) =
        submit(&harness, &credential, serde_json::to_vec(&next).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(
        receipt_from(&accepted).disposition,
        ReceiptDisposition::Accepted
    );
    harness.state.db().close().await;
}
