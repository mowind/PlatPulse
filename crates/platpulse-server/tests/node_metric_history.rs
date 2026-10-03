//! Raw Node metric history at the HTTP boundary (issue #213, design §11.4).
//!
//! The unit tests in crate::metric_history drive the ledger, the silences and
//! the coverage arithmetic directly. This suite drives the same seam the way an
//! Operator and an Agent do: it builds the full application with build_app
//! against a temporary SQLite database, enrolls a real Agent through the Admin
//! and Agent HTTP APIs, submits real Reports, and reads the range back through
//! the Owner-only Admin route.
//!
//! It proves the claims the issue asks for at the surface an Operator uses:
//!
//! 1. The stored observations of one Node series are readable over a range,
//!    with the timing evidence that belongs to each sample, and a series the
//!    Node never reported is reported as never observed instead of as zeros.
//! 2. A low-space pause is a visible protection gap with its counted losses,
//!    never a zero and never a bridged line.
//! 3. Raw history older than the retention floor is reported as unavailable
//!    while the series ledger that outlives it keeps saying what was observed.
//!
//! Every test owns its own TempDir and database, so nothing is shared with the
//! parallel unit tests.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_server::AppState;
use platpulse_server::capacity::{CapacityConfig, CapacityProtection, sample_filesystem};
use platpulse_server::{auth, database, http, network, secrets};

/// Registered Network tuple for the platon-mainnet key the report fixture
/// declares, matching the unit-test registry exactly.
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const VIEWER_LOGIN_BODY: &str = r#"{"username":"viewer1","password":"correct horse battery"}"#;
/// The Node the injected observation fixture declares.
const NODE_ID: &str = "0195f2a1-0014-4014-8014-000000000014";

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

    async fn count_where(&self, table: &str, predicate: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table} WHERE {predicate}"))
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

/// An authenticated human session: the cookie every Admin GET must present.
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

/// The minimal wire fixture, re-bound to the enrolled Agent identity and stamped
/// with the observation instant the test wants.
///
/// The fixture's Node process component is disabled, so the canonical healthy
/// one is grafted in and re-stamped: the point of this suite is the raw history
/// a real Agent produces, and a disabled probe produces none. The Agent host
/// throughput component is stamped too, so every optional series of the Report
/// belongs to the same observation instant.
fn fixture_report(agent_id: &str, sequence: u64, observed_at: &str) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    let canonical: Value = serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_canonical.json"
    ))
    .unwrap();
    value["nodes"][0]["process"] = canonical["nodes"][0]["process"].clone();
    let observed = Value::String(observed_at.to_owned());
    value["nodes"][0]["process"]["attempted_at"] = observed.clone();
    value["nodes"][0]["process"]["latest_observed_at"] = observed.clone();
    value["host"]["network_throughput"]["attempted_at"] = observed.clone();
    value["host"]["network_throughput"]["latest_observed_at"] = observed;
    value["agent_id"] = Value::String(agent_id.to_owned());
    value["report_sequence"] = Value::from(sequence);
    value["report_id"] = Value::String(format!(
        "0195f2a1-00{sequence:02x}-4013-8013-0000000000{sequence:02x}"
    ));
    value["generated_at"] = Value::String(observed_at.to_owned());
    serde_json::to_vec(&value).unwrap()
}

fn history_uri(node_id: &str, metric: &str, from: Option<&str>, to: Option<&str>) -> String {
    let mut uri = format!("/api/admin/v1/nodes/{node_id}/metric-history?metric={metric}");
    if let Some(from) = from {
        uri.push_str(&format!("&from={from}"));
    }
    if let Some(to) = to {
        uri.push_str(&format!("&to={to}"));
    }
    uri
}

async fn metric_history(
    harness: &Harness,
    cookie: Option<&str>,
    node_id: &str,
    metric: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> (StatusCode, Value) {
    let request = admin_get(&history_uri(node_id, metric, from, to), cookie);
    let response = harness.send(request).await;
    let status = response.status();
    (status, body_json(response).await)
}

/// Story 46: an Operator reads the raw observations the Server actually stored
/// for one Node series over the retained window, with the timing evidence of
/// every sample, and with the boundary and identity refusals intact.
#[tokio::test]
async fn owner_reads_the_stored_series_and_other_principals_are_refused() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let instant = |seconds: i64| auth::format_rfc3339(base + time::Duration::seconds(seconds));
    let first = instant(0);
    let third = instant(120);

    for (sequence, observed_at) in [(1_u64, &first), (2, &instant(60)), (3, &third)] {
        let (status, value) = submit(
            &harness,
            &credential,
            fixture_report(&agent_id, sequence, observed_at),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    // The default answer is the retained raw window, and it carries the samples
    // the Server stored rather than a reconstructed curve.
    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["nodeId"], Value::String(NODE_ID.to_owned()));
    assert_eq!(
        body["metric"],
        Value::String("process_cpu_percent".to_owned())
    );
    assert_eq!(body["grain"], Value::String("raw".to_owned()));
    assert_eq!(body["aggregateSupported"], Value::Bool(false));
    assert_eq!(
        body["rawRetentionDays"],
        Value::from(1),
        "the raw metric family carries the 24 hour floor"
    );
    assert_eq!(body["windowSeconds"], Value::from(86_400));
    assert!(body["availability"].is_null());
    assert_eq!(body["truncated"], Value::Bool(false));
    assert!(body["gaps"].as_array().unwrap().is_empty(), "{body}");

    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{body}");
    assert_eq!(items[0]["observedAt"], Value::String(first.clone()));
    assert_eq!(items[2]["observedAt"], Value::String(third.clone()));
    for item in items {
        assert_eq!(item["value"], Value::from(2.5), "{item}");
        let delay = item["delaySeconds"]
            .as_i64()
            .expect("a live sample has a receipt delay");
        assert!(delay >= 0, "the receipt follows the observation: {item}");
        assert_eq!(item["clockSuspect"], Value::Bool(false));
        assert!(item["clockNote"].is_null());
        assert!(item["receivedAt"].as_str().is_some(), "{item}");
    }

    // The series state is independent of the window: enablement, count and
    // proved coverage are the Server's own record.
    let series = &body["series"];
    assert_eq!(series["observed"], Value::Bool(true));
    assert_eq!(series["observationCount"], Value::from(3));
    assert_eq!(series["replayedCount"], Value::from(0));
    assert_eq!(series["correctedCount"], Value::from(0));
    assert_eq!(series["sampledCount"], Value::from(3));
    assert_eq!(
        series["coverageSeconds"],
        Value::from(120),
        "the minute between the stored observations is the coverage they prove: {body}"
    );
    assert_eq!(series["windowSeconds"], Value::from(86_400));
    assert_eq!(series["firstObservedAt"], Value::String(first.clone()));
    assert_eq!(series["latestClockSuspect"], Value::Bool(false));

    // A series this Node never reported is never observed, not zero.
    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "data_directory_percent",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["series"]["observed"], Value::Bool(false));
    assert_eq!(body["series"]["observationCount"], Value::from(0));
    assert_eq!(body["series"]["sampledCount"], Value::from(0));
    assert!(body["series"]["firstObservedAt"].is_null());
    assert!(body["items"].as_array().unwrap().is_empty(), "{body}");

    // The query is validated before any history is read.
    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "carrier_pigeons",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_metric");

    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        Some(&third),
        Some(&first),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_history_range");

    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        Some("yesterday"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_history_range");

    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        "0195f2a1-00ff-4014-8014-0000000000ff",
        "process_cpu_percent",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    // The range is Owner-only: an anonymous caller never reaches the handler,
    // and a Viewer has no administrative authority.
    let (status, body) =
        metric_history(&harness, None, NODE_ID, "process_cpu_percent", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "auth_required");

    let hash = auth::hash_password(b"correct horse battery").unwrap();
    auth::create_viewer(harness.state.db(), "viewer1", &hash)
        .await
        .unwrap();
    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    let (status, body) = metric_history(
        &harness,
        Some(&viewer.cookie),
        NODE_ID,
        "process_cpu_percent",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "owner_required");
}

/// Story 47 and 49: a silence the Server cannot explain is reported as a
/// collection gap with its length, and the stretch it covers proves no coverage,
/// so no surface has to bridge it with a line or a zero.
#[tokio::test]
async fn an_unexplained_silence_is_a_collection_gap_not_proved_coverage() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let instant = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));
    let first = instant(0);
    let third = instant(120);

    // An Agent cannot be configured slower than five minutes, so the Server never
    // assumes a cadence slower than that: an hour between two Reports is a
    // silence the Server has no evidence for, not a series that got slower.
    for (sequence, observed_at) in [(1_u64, &first), (2, &instant(60)), (3, &third)] {
        let (status, value) = submit(
            &harness,
            &credential,
            fixture_report(&agent_id, sequence, observed_at),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{body}");
    for item in items {
        assert_eq!(
            item["value"],
            Value::from(2.5),
            "a silence is never filled with a value: {item}"
        );
    }

    let gaps = body["gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 2, "{body}");
    let expected = [(first.clone(), instant(60)), (instant(60), third.clone())];
    for (gap, (from, to)) in gaps.iter().zip(expected) {
        assert_eq!(gap["kind"], Value::String("collection_gap".to_owned()));
        assert_eq!(
            gap["reason"],
            Value::String("no observation was received in this stretch".to_owned())
        );
        assert_eq!(gap["from"], Value::String(from));
        assert_eq!(gap["to"], Value::String(to));
        assert_eq!(gap["seconds"], Value::from(3_600));
        assert!(
            gap["skippedCount"].is_null(),
            "nobody recorded a skipped instant: {gap}"
        );
    }

    assert_eq!(
        body["series"]["coverageSeconds"],
        Value::from(0),
        "a stretch nobody observed proves no coverage: {body}"
    );
    assert_eq!(body["series"]["observationCount"], Value::from(3));
    assert_eq!(body["series"]["replayedCount"], Value::from(0));
    assert!(body["availability"].is_null());
}

/// Story 47 and 49: a low-space pause is visible as a protection gap with its
/// counted losses, so no surface has to draw a zero or bridge the silence, and
/// the coverage the Server claims is only what its samples prove.
#[tokio::test]
async fn a_low_space_pause_is_a_protection_gap_not_a_zero() {
    let mut harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::minutes(60);
    let instant = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));
    let mut sequence = 0_u64;
    let report = |sequence: &mut u64, minutes: i64| {
        fixture_report(
            &agent_id,
            {
                *sequence += 1;
                *sequence
            },
            &instant(minutes),
        )
    };

    // A regularly observed series first: the cadence the Server measures is the
    // one this Node actually showed.
    for minutes in [0_i64, 5, 10] {
        let (status, value) = submit(&harness, &credential, report(&mut sequence, minutes)).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    // Low-space protection pauses optional history: the Reports are still
    // accepted, and the losses are counted instead of silently dropped.
    let pressure = forced_policy(&harness);
    pressure.reconcile(harness.pool()).await.unwrap();
    assert!(
        pressure.status().protected,
        "the declared floor is above any real free space"
    );
    harness.install_capacity(Arc::clone(&pressure));
    for minutes in [15_i64, 20] {
        let (status, value) = submit(&harness, &credential, report(&mut sequence, minutes)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "pressure never refuses a Report: {value}"
        );
    }
    assert_eq!(
        harness
            .count_where("node_metric_samples", "metric = 'process_cpu_percent'")
            .await,
        3,
        "a paused sample is not stored"
    );

    // Release the floor below the measured free space and let collection resume.
    let available = sample_filesystem(&harness.mount()).unwrap().available_bytes;
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
    assert!(!released.status().protected);
    harness.install_capacity(Arc::clone(&released));
    for minutes in [27_i64, 32] {
        let (status, value) = submit(&harness, &credential, report(&mut sequence, minutes)).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let from = instant(-5);
    let to = instant(40);
    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        5,
        "the window holds exactly the observations the Server stored: {body}"
    );
    for item in items {
        assert_eq!(
            item["value"],
            Value::from(2.5),
            "a paused stretch is never filled with a value: {item}"
        );
    }
    let gaps = body["gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{body}");
    let gap = &gaps[0];
    assert_eq!(gap["kind"], Value::String("protection_pause".to_owned()));
    assert_eq!(
        gap["reason"],
        Value::String("low-space protection paused sample collection".to_owned())
    );
    assert_eq!(gap["from"], Value::String(instant(10)));
    assert_eq!(gap["to"], Value::String(instant(27)));
    assert_eq!(gap["seconds"], Value::from(1_020));
    assert_eq!(
        gap["skippedCount"],
        Value::from(2),
        "the gap carries the losses the pause counted"
    );
    assert_eq!(
        body["series"]["coverageSeconds"],
        Value::from(900),
        "only the stretches between stored observations are proved coverage: {body}"
    );
    assert_eq!(body["series"]["observationCount"], Value::from(5));
    assert_eq!(body["series"]["replayedCount"], Value::from(0));
    assert!(body["availability"].is_null());
    assert_eq!(body["truncated"], Value::Bool(false));
}

/// Story 57 and 59: raw history older than the retention floor is reported as
/// unavailable rather than as zeros, and the series ledger that outlives the
/// samples keeps saying what the series observed and when it started.
#[tokio::test]
async fn history_older_than_the_retention_floor_is_reported_not_faked() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let now = auth::now_utc();
    let expired = auth::format_rfc3339(now - time::Duration::hours(40));
    let (status, value) = submit(
        &harness,
        &credential,
        fixture_report(&agent_id, 1, &expired),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");

    // The raw metric family owns the window: an observation older than the floor
    // is released by the ingestion-time cleanup instead of extending it.
    assert_eq!(
        harness
            .count_where("node_metric_samples", "metric = 'process_cpu_percent'")
            .await,
        0,
        "the retained raw window is the policy's, not the last writer's"
    );
    // The ledger is not a second copy of history and is not released with it.
    assert_eq!(
        harness
            .count_where("node_metric_series_state", "metric = 'process_cpu_percent'")
            .await,
        1
    );

    let from = auth::format_rfc3339(now - time::Duration::hours(48));
    let to = auth::format_rfc3339(now - time::Duration::hours(36));
    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["availability"],
        Value::String("unavailable".to_owned()),
        "a range older than the retained window says so: {body}"
    );
    assert!(body["items"].as_array().unwrap().is_empty(), "{body}");
    assert!(body["gaps"].as_array().unwrap().is_empty(), "{body}");
    assert_eq!(body["requestedFrom"], Value::String(from.clone()));
    assert_eq!(body["series"]["observed"], Value::Bool(true));
    assert_eq!(body["series"]["observationCount"], Value::from(1));
    assert_eq!(
        body["series"]["sampledCount"],
        Value::from(0),
        "the answer carries no sample it does not hold: {body}"
    );
    assert_eq!(
        body["series"]["firstObservedAt"],
        Value::String(expired.clone())
    );

    // A clamped range says it clamped: the part inside the retained window is
    // answered, the part outside it is named in availability.
    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        Some(&from),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["availability"], Value::String("partial".to_owned()));
    assert_eq!(body["requestedFrom"], Value::String(from.clone()));
    assert!(
        body["from"].as_str().unwrap() > from.as_str(),
        "the answered range starts at the retained floor: {body}"
    );
}

/// The reviewer's scenario at the HTTP boundary: an observation counted below
/// the raw floor is not counted a second time by a widening.
///
/// The first Report carries an observation older than the one-day window. It is
/// the first thing the Server ever hears about this series, so it is counted —
/// the ledger says what the series observed — while no sample row can answer for
/// it. Counting it is what leaves an evidence floor behind; without that floor,
/// widening the policy past the same instant and carrying the last good reading
/// would look like a second observation.
#[tokio::test]
async fn an_observation_counted_below_the_floor_is_not_recounted_by_a_widening() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let now = auth::now_utc();
    let expired = auth::format_rfc3339(now - time::Duration::hours(40));
    let (status, value) = submit(
        &harness,
        &credential,
        fixture_report(&agent_id, 1, &expired),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");

    let ledger = |column: &str| {
        let column = column.to_owned();
        let pool = harness.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>(&format!(
                "SELECT {column} FROM node_metric_series_state WHERE node_id = ? AND metric = 'process_cpu_percent'"
            ))
            .bind(NODE_ID)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(ledger("observation_count").await, 1);
    assert_eq!(
        harness
            .count_where("node_metric_samples", "metric = 'process_cpu_percent'")
            .await,
        0,
        "the count is kept while the row is not"
    );

    // The Operator widens the window over the instant the series was counted at.
    sqlx::query(
        "UPDATE retention_policies SET retention_days = 2 WHERE family = 'raw_metric_sample'",
    )
    .execute(harness.pool())
    .await
    .unwrap();

    // The Agent carries its last good reading: the same instant, a new Report.
    let (status, value) = submit(
        &harness,
        &credential,
        fixture_report(&agent_id, 2, &expired),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        ledger("observation_count").await,
        1,
        "the widened window reads the carried instant, it does not count it again"
    );
    assert_eq!(ledger("replayed_count").await, 1);
    assert_eq!(
        harness
            .count_where("node_metric_samples", "metric = 'process_cpu_percent'")
            .await,
        0,
        "and a replay never writes a row back, however wide the window is"
    );

    // The series still says when it was first observed, and the range the
    // widening was asked for answers without faking a value it does not hold.
    let from = auth::format_rfc3339(now - time::Duration::hours(48));
    let to = auth::format_rfc3339(now - time::Duration::hours(36));
    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["items"].as_array().unwrap().is_empty(), "{body}");
    assert_eq!(
        body["series"]["firstObservedAt"],
        Value::String(expired.clone())
    );
    assert_eq!(body["series"]["observationCount"], Value::from(1));
}
