//! Disk capacity and low-space protection at the HTTP boundary (issue #212,
//! Stories 43 to 46 of the capacity and low-space protection closed loop).
//!
//! The unit tests in crate::capacity drive the policy directly. This suite
//! drives the same seam the way an Operator and an Agent do: it builds the full
//! application with build_app against a temporary SQLite database, enrolls a
//! real Agent through the Admin and Agent HTTP APIs, and submits real reports
//! while the state filesystem is under a declared low-space policy.
//!
//! It proves the three claims the issue asks for:
//!
//! 1. A Server under storage pressure still accepts Reports and still commits
//!    the core projection. Only optional metric history pauses, and the gap it
//!    leaves is visible, counted, and attributed to a series.
//! 2. Releasing the pressure resumes optional history and closes the interval
//!    with the measurement that released it.
//! 3. A Report that fails anywhere in its receipt transaction is not accepted,
//!    and the gap it would have recorded is rolled back with it: the visible
//!    gap can never claim a sample the Server never stored.
//!
//! Every test owns its own TempDir and database, so nothing is shared with the
//! parallel unit tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_core::{AgentReport, ReceiptDisposition};
use platpulse_server::capacity::{
    ADMIN_RECENT_INTERVAL_LIMIT, CapacityConfig, CapacityProtection, recent_intervals,
    sample_filesystem,
};
use platpulse_server::{AppState, auth, database, http, network, secrets};

/// Registered Network tuple for the platon-mainnet key the report fixture
/// declares, matching the unit-test registry exactly.
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const VIEWER_LOGIN_BODY: &str = r#"{"username":"viewer1","password":"correct horse battery"}"#;
const VIEWER_PASSWORD: &[u8] = b"correct horse battery";

/// A fresh Server (real temp SQLite + pepper) and the full router.
///
/// The capacity policy is process state, so a test that changes it rebuilds the
/// router through install_capacity; both routers share the same database handle
/// and the same database file, exactly like a Server restart would.
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
        // The Agent group refuses every request until the first Owner exists
        // (design §12.2), so the acceptance path must start here.
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

    /// Install the capacity policy the deployment declared and rebuild the
    /// router over the same database.
    fn install_capacity(&mut self, capacity: Arc<CapacityProtection>) {
        self.state = self.state.clone().with_capacity(capacity);
        self.app = http::build_app(self.state.clone());
    }

    fn pool(&self) -> &sqlx::SqlitePool {
        self.state.db().pool()
    }

    fn mount(&self) -> PathBuf {
        self.state
            .db()
            .path()
            .parent()
            .expect("the database file has a parent directory")
            .to_path_buf()
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    async fn send(&self, request: Request<Body>) -> axum::response::Response {
        self.app.clone().oneshot(request).await.unwrap()
    }
}

/// A declared policy whose floor no disk can clear, so the state filesystem is
/// always under pressure. Tests need a deterministic pressure state, and the
/// largest byte count the Server can persist is the only honest way to declare
/// one without inventing a number that would become the product default.
fn forced_policy(harness: &Harness) -> Arc<CapacityProtection> {
    let config = CapacityConfig::from_declared(
        true,
        Some(CapacityConfig::MAX_PERSISTED_BYTES),
        Some(CapacityConfig::MAX_PERSISTED_BYTES),
        None,
        Some(PathBuf::from("test-capacity-floor")),
    )
    .unwrap();
    Arc::new(CapacityProtection::new(
        config,
        Some(harness.state.db().path()),
    ))
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

fn admin_get(uri: &str, cookie: Option<&str>) -> Request<Body> {
    let builder = Request::builder().method("GET").uri(uri);
    let builder = match cookie {
        Some(cookie) => builder.header(header::COOKIE, cookie),
        None => builder,
    };
    builder.body(Body::empty()).unwrap()
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
    let body = body_json(response).await;
    let csrf = body["csrfToken"].as_str().unwrap().to_owned();
    Session { cookie, csrf }
}

async fn owner_session(harness: &Harness) -> Session {
    login(harness, OWNER_LOGIN_BODY).await
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

fn receipt_from(value: &Value) -> platpulse_core::ReportReceipt {
    serde_json::from_value(value["receipt"].clone()).unwrap()
}

/// The minimal wire fixture, re-bound to the enrolled Agent identity the Server
/// actually minted, carrying a new report id and a later generation time per
/// sequence so repeat submissions are new reports rather than replays.
///
/// The fixture's Node process component is disabled, so the canonical fixture's
/// healthy process component is grafted in: the point of this suite is the
/// optional metric history a real Agent produces, and a disabled probe produces
/// none.
fn fixture_report(agent_id: &str, agent_epoch: u64, report_sequence: u64) -> AgentReport {
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    let canonical: Value = serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_canonical.json"
    ))
    .unwrap();
    value["nodes"][0]["process"] = canonical["nodes"][0]["process"].clone();
    value["agent_id"] = Value::String(agent_id.to_owned());
    value["agent_epoch"] = Value::from(agent_epoch);
    value["report_sequence"] = Value::from(report_sequence);
    value["report_id"] = Value::String(format!(
        "0195f2a1-00{agent_epoch:02x}-4035-8035-0000000000{report_sequence:02x}"
    ));
    value["generated_at"] = Value::String(format!("2026-08-12T{:02}:00:00Z", 8 + report_sequence));
    serde_json::from_value(value).unwrap()
}

fn skipped_series_keys(interval: &platpulse_server::capacity::CapacityIntervalRecord) -> Vec<&str> {
    interval
        .skipped_series
        .iter()
        .map(|series| series.metric.as_str())
        .collect()
}

/// Story 43 to 45 end to end: pressure pauses optional history only, the Report
/// is still accepted with its core projection, the gap is visible and counted,
/// and releasing the floor resumes history and closes the interval with the
/// measurement that released it.
#[tokio::test]
async fn optional_history_pauses_under_pressure_and_recovers_with_a_visible_gap() {
    let mut harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let mount = harness.mount();

    let pressure = forced_policy(&harness);
    pressure.reconcile(harness.pool()).await.unwrap();
    assert!(
        pressure.status().protected,
        "the declared floor is above any real free space"
    );
    harness.install_capacity(Arc::clone(&pressure));

    // A Report submitted while optional history is paused is still accepted.
    let (status, value) = submit(
        &harness,
        &credential,
        serde_json::to_vec(&fixture_report(&agent_id, 1, 1)).unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a Server under storage pressure still accepts Reports: {value}"
    );
    assert_eq!(
        receipt_from(&value).disposition,
        ReceiptDisposition::Accepted
    );

    // The core projection is committed: capacity pressure never weakens the
    // current-state projection or the receipt.
    assert_eq!(harness.count("agent_report_receipts").await, 1);
    assert_eq!(harness.count("current_host_observations").await, 1);
    assert_eq!(harness.count("current_node_process_observations").await, 1);

    // Optional history is paused, and the pause is a recorded gap rather than
    // silently dropped samples.
    assert_eq!(harness.count("host_metric_samples").await, 0);
    assert_eq!(harness.count("node_metric_samples").await, 0);
    let intervals = recent_intervals(harness.pool(), ADMIN_RECENT_INTERVAL_LIMIT)
        .await
        .unwrap();
    assert_eq!(intervals.len(), 1, "one interval covers the whole episode");
    let open = &intervals[0];
    assert_eq!(open.started_reason, "low_space");
    assert_eq!(open.source_mount, mount.to_string_lossy());
    assert_eq!(
        open.pause_below_bytes,
        CapacityConfig::MAX_PERSISTED_BYTES,
        "the interval records the floor that was in force"
    );
    assert!(
        open.opened_available_bytes < open.pause_below_bytes,
        "an interval opens only on a measurement below the floor"
    );
    assert!(
        open.ended_at.is_none() && open.ended_reason.is_none(),
        "the interval stays open while the pressure lasts"
    );
    assert_eq!(
        open.skipped_sample_count, 4,
        "the two Agent host series and the two Node process series produced four samples"
    );
    assert_eq!(open.skipped_series_total, 4);
    let metrics = skipped_series_keys(open);
    for expected in [
        "network_rx_bytes_per_sec",
        "network_tx_bytes_per_sec",
        "process_cpu_percent",
        "process_memory_percent",
    ] {
        assert!(
            metrics.contains(&expected),
            "the gap names the series it lost: {expected} missing from {metrics:?}"
        );
    }
    assert!(
        open.skipped_series
            .iter()
            .any(|series| series.scope_kind == "host" && series.scope_key == agent_id),
        "a host series is attributed to the Agent that owns it"
    );
    assert!(
        open.skipped_series
            .iter()
            .any(|series| series.scope_kind == "node" && !series.scope_key.is_empty()),
        "a Node series is attributed to the Node that owns it"
    );

    // The same state is visible to the Operator through the Admin API.
    let response = harness
        .send(admin_get("/api/admin/v1/capacity", Some(&session.cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let overview = body_json(response).await;
    assert_eq!(overview["protected"], Value::Bool(true));
    assert_eq!(
        overview["activeIntervalId"],
        Value::String(open.interval_id.clone())
    );
    assert_eq!(overview["recentIntervals"][0]["skippedSampleCount"], 4);
    assert!(
        overview["recentIntervals"][0]["skippedSeries"][0]["metric"]
            .as_str()
            .is_some(),
        "the Admin surface reports the metric whose samples were skipped"
    );

    // Release the floor below the measured free space. A restart-equivalent
    // reconcile adopts the open interval this process left behind.
    let available = sample_filesystem(&mount).unwrap().available_bytes;
    let released_floor = available / 2;
    let released_config = CapacityConfig::from_declared(
        true,
        Some(released_floor),
        Some(released_floor),
        None,
        Some(PathBuf::from("test-capacity-floor")),
    )
    .unwrap();
    let released = Arc::new(CapacityProtection::new(
        released_config,
        Some(harness.state.db().path()),
    ));
    released.reconcile(harness.pool()).await.unwrap();
    assert!(
        !released.status().protected,
        "free space at the declared floor is released, not protected"
    );
    harness.install_capacity(Arc::clone(&released));

    // A later Report is accepted and its optional samples are written again.
    let (status, value) = submit(
        &harness,
        &credential,
        serde_json::to_vec(&fixture_report(&agent_id, 1, 2)).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(harness.count("host_metric_samples").await, 2);
    assert_eq!(harness.count("node_metric_samples").await, 2);

    let intervals = recent_intervals(harness.pool(), ADMIN_RECENT_INTERVAL_LIMIT)
        .await
        .unwrap();
    assert_eq!(intervals.len(), 1);
    let closed = &intervals[0];
    assert_eq!(closed.ended_reason.as_deref(), Some("resumed"));
    assert!(
        closed.ended_at.is_some(),
        "the interval records when it ended"
    );
    assert!(
        closed.resumed_available_bytes.unwrap() >= released_floor,
        "the recorded release measurement is at or above the declared release level"
    );
    assert!(closed.resumed_total_bytes.unwrap() > 0);
    assert_eq!(
        closed.skipped_sample_count, 4,
        "ending the interval keeps the record of what was lost"
    );
    assert_eq!(
        closed.skipped_series_total, 4,
        "the visible gap survives the recovery"
    );
}

/// Story 46 and design §8.3: a Report whose receipt write fails is not accepted
/// as received, and the gap it would have recorded rolls back with it.
#[tokio::test]
async fn a_report_that_fails_its_receipt_write_does_not_claim_a_gap() {
    let mut harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;

    let pressure = forced_policy(&harness);
    pressure.reconcile(harness.pool()).await.unwrap();
    harness.install_capacity(Arc::clone(&pressure));
    assert_eq!(
        harness.count("capacity_protection_intervals").await,
        1,
        "the interval opens on the sampling path, outside the report transaction"
    );

    // A controlled failure source: the receipt insert of every report aborts.
    sqlx::query(
        "CREATE TRIGGER abort_capacity_receipt BEFORE INSERT ON agent_report_receipts BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END",
    )
    .execute(harness.pool())
    .await
    .unwrap();

    let (status, value) = submit(
        &harness,
        &credential,
        serde_json::to_vec(&fixture_report(&agent_id, 1, 1)).unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "the Server must not report success: {value}"
    );
    assert_eq!(value["error"]["code"], "unavailable");
    assert_eq!(harness.count("agent_report_receipts").await, 0);
    assert_eq!(
        harness.count("current_host_observations").await,
        0,
        "the core projection rolled back with the receipt"
    );
    assert_eq!(
        harness.count("capacity_skipped_series").await,
        0,
        "the gap rolled back with the Report: a visible gap never claims a sample the Server did not store"
    );
}

/// The Admin surface: Owner-only, and honest about a disabled policy instead of
/// inventing thresholds or a zero available-bytes reading.
#[tokio::test]
async fn capacity_route_is_owner_only_and_does_not_invent_a_disabled_policy() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    auth::create_viewer(
        harness.state.db(),
        "viewer1",
        &auth::hash_password(VIEWER_PASSWORD).unwrap(),
    )
    .await
    .unwrap();

    let response = harness
        .send(admin_get("/api/admin/v1/capacity", None))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body_json(response).await["error"]["code"], "auth_required");

    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    let response = harness
        .send(admin_get("/api/admin/v1/capacity", Some(&viewer.cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(body_json(response).await["error"]["code"], "owner_required");

    let response = harness
        .send(admin_get("/api/admin/v1/capacity", Some(&session.cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let overview = body_json(response).await;
    assert_eq!(overview["enabled"], Value::Bool(false));
    assert_eq!(overview["protected"], Value::Bool(false));
    assert_eq!(overview["pauseBelowBytes"], Value::Null);
    assert_eq!(overview["resumeAboveBytes"], Value::Null);
    assert_eq!(overview["policyOrigin"], Value::Null);
    assert_eq!(overview["activeIntervalId"], Value::Null);
    assert_eq!(overview["recentIntervals"], Value::Array(Vec::new()));
    assert_eq!(overview["sampleIntervalSeconds"], 60);
    assert_eq!(
        overview["mountPath"],
        Value::String(harness.mount().to_string_lossy().into_owned())
    );
    // Capacity is still measured and visible while protection is off: an
    // Operator has to be able to see the disk that has no floor declared yet.
    assert!(
        overview["sample"]["totalBytes"].as_u64().unwrap() > 0,
        "the state filesystem is measured even when the policy is disabled"
    );
    assert!(overview["sampledAt"].is_string());
    assert_eq!(
        overview["sample"]["mountPath"],
        Value::String(harness.mount().to_string_lossy().into_owned())
    );
    let measured = sample_filesystem(Path::new(&harness.mount())).unwrap();
    assert!(measured.available_bytes > 0);
}
