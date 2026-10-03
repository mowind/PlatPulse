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
use platpulse_server::metric_history;
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
/// router through install_capacity; restart closes the pool and opens the same
/// durable database directory again, which is what the process does on start.
struct Harness {
    _dir: TempDir,
    state: AppState,
    app: Router,
}

impl Harness {
    /// Open (or re-open) the Server in `dir` without seeding it. The restart
    /// case drops the previous pool first so the exclusive SQLite lock is
    /// released before this second opener arrives.
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
        Self {
            _dir: dir,
            state,
            app,
        }
    }

    async fn boot() -> Self {
        let harness = Self::open(TempDir::new().unwrap()).await;
        // The Agent group refuses every request until the first Owner exists
        // (design §12.2), so the acceptance path must start here.
        let hash = auth::hash_password(b"correct horse battery").unwrap();
        auth::create_owner(harness.state.db(), "admin", &hash)
            .await
            .unwrap();
        network::create_network(
            harness.state.db(),
            "platon-mainnet",
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

    /// Simulate a Server restart over the same durable database directory: the
    /// policy rows, the raw samples and the tier buckets have to be read back
    /// from storage instead of surviving in process memory.
    async fn restart(self) -> Self {
        let Harness { _dir, state, app } = self;
        state.db().pool().close().await;
        drop(app);
        drop(state);
        Self::open(_dir).await
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
    assert_eq!(
        body["aggregateSupported"],
        Value::Bool(true),
        "the answer says the tiers behind the raw window are served"
    );
    assert_eq!(
        body["rawRetentionDays"],
        Value::from(1),
        "the raw metric family carries the 24 hour floor"
    );
    assert_eq!(
        body["historyHorizonDays"],
        Value::from(30),
        "the coarse tier and its retention family declare the investigation horizon"
    );
    assert_eq!(body["windowSeconds"], Value::from(86_400));
    assert!(body["availability"].is_null());
    assert_eq!(body["truncated"], Value::Bool(false));
    assert!(body["continuation"].is_null());
    assert!(body["gaps"].as_array().unwrap().is_empty(), "{body}");

    // The whole window is inside the raw tier, and the segments say which grain
    // answered which stretch instead of leaving a reader to infer it.
    let segments = body["segments"].as_array().unwrap();
    assert_eq!(segments.len(), 1, "{body}");
    assert_eq!(segments[0]["grain"], Value::String("raw".to_owned()));
    assert_eq!(segments[0]["source"], Value::String("raw".to_owned()));
    assert_eq!(segments[0]["pointCount"], Value::from(3));
    assert_eq!(segments[0]["truncated"], Value::Bool(false));

    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{body}");
    assert_eq!(items[0]["observedAt"], Value::String(first.clone()));
    assert_eq!(items[2]["observedAt"], Value::String(third.clone()));
    for item in items {
        assert_eq!(item["value"], Value::from(2.5), "{item}");
        assert_eq!(item["grain"], Value::String("raw".to_owned()));
        assert_eq!(item["source"], Value::String("raw".to_owned()));
        assert_eq!(item["sampleCount"], Value::from(1), "{item}");
        assert_eq!(item["minValue"], Value::from(2.5), "{item}");
        assert_eq!(item["maxValue"], Value::from(2.5), "{item}");
        assert_eq!(
            item["lastObservedAt"], item["observedAt"],
            "a sample is its own newest observation: {item}"
        );
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
    // Optional history is not written another way either: a stretch collection
    // skipped leaves no bucket behind, so a pause can never be summarized after
    // the fact by a tier that would look like evidence.
    for paused in [instant(15), instant(20)] {
        let bucket =
            metric_history::aligned_bucket_start(&paused, metric_history::ONE_MINUTE_SECONDS)
                .expect("a canonical instant aligns to its minute");
        assert_eq!(
            harness
                .count_where(
                    "node_metric_aggregates",
                    &format!(
                        "metric = 'process_cpu_percent' AND grain_seconds = 60 AND bucket_start = '{bucket}'"
                    ),
                )
                .await,
            0,
            "a paused observation is not summarized either: {bucket}"
        );
    }

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
    let resumed =
        metric_history::aligned_bucket_start(&instant(27), metric_history::ONE_MINUTE_SECONDS)
            .expect("a canonical instant aligns to its minute");
    assert_eq!(
        harness
            .count_where(
                "node_metric_aggregates",
                &format!(
                    "metric = 'process_cpu_percent' AND grain_seconds = 60 AND bucket_start = '{resumed}'"
                ),
            )
            .await,
        1,
        "collection that resumes is summarized again: {resumed}"
    );

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

/// Story 57, 58 and 59: history older than the raw floor is answered by the
/// bucket that counted it, a range older than the investigation horizon is
/// reported as unavailable rather than as zeros, and the series ledger that
/// outlives every tier keeps saying what the series observed and when it
/// started.
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

    // The observation is older than the raw window and well inside the
    // investigation horizon: it is answered by the bucket that counted it, at the
    // tier's own grain, and never as a sample the Server no longer holds.
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
    assert!(
        body["availability"].is_null(),
        "a range inside the horizon is answerable: {body}"
    );
    assert_eq!(body["grain"], Value::String("1m".to_owned()));
    assert_eq!(body["requestedFrom"], Value::String(from.clone()));
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{body}");
    assert_eq!(items[0]["value"], Value::from(2.5));
    assert_eq!(items[0]["source"], Value::String("aggregate".to_owned()));
    assert_eq!(items[0]["sampleCount"], Value::from(1));
    assert!(items[0]["receivedAt"].as_str().is_some(), "{body}");
    assert!(body["gaps"].as_array().unwrap().is_empty(), "{body}");
    assert_eq!(body["series"]["observed"], Value::Bool(true));
    assert_eq!(body["series"]["observationCount"], Value::from(1));
    assert_eq!(
        body["series"]["sampledCount"],
        Value::from(1),
        "the answer carries the bucket that counted the released observation: {body}"
    );
    assert_eq!(
        body["series"]["firstObservedAt"],
        Value::String(expired.clone())
    );

    // Beyond the investigation horizon no tier holds anything: the range is
    // reported as unavailable instead of as zeros, while the ledger that
    // outlives every tier still says what was observed.
    let ancient_from = auth::format_rfc3339(now - time::Duration::days(40));
    let ancient_to = auth::format_rfc3339(now - time::Duration::days(35));
    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        Some(&ancient_from),
        Some(&ancient_to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["availability"],
        Value::String("unavailable".to_owned()),
        "a range older than any tier says so: {body}"
    );
    assert_eq!(body["grain"], Value::String("none".to_owned()));
    assert!(body["items"].as_array().unwrap().is_empty(), "{body}");
    assert!(body["segments"].as_array().unwrap().is_empty(), "{body}");
    assert!(body["gaps"].as_array().unwrap().is_empty(), "{body}");
    assert_eq!(body["requestedFrom"], Value::String(ancient_from.clone()));
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

    // A clamped range says it clamped: the part inside the horizon is answered,
    // the part outside it is named in availability.
    let (status, body) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        Some(&ancient_from),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["availability"], Value::String("partial".to_owned()));
    assert_eq!(body["requestedFrom"], Value::String(ancient_from.clone()));
    assert!(
        body["from"].as_str().unwrap() > ancient_from.as_str(),
        "the answered range starts at the investigation horizon: {body}"
    );
    assert_eq!(
        body["windowSeconds"],
        Value::from(30 * 86_400),
        "the answered range is the horizon, not the range nobody can serve: {body}"
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
    // widening was asked for answers with the evidence that really exists: no
    // sample row came back, so nothing raw answers, but the minute tier that
    // counted the instant is still the record of it. The stretch is older than
    // the window the Server serves raw- and that window is fixed at one day
    // whatever the storage policy says - so the tier answers, and it answers
    // with the single observation the ledger counted, not with a second one.
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
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{body}");
    assert_eq!(items[0]["grain"], Value::String("1m".to_owned()));
    assert_eq!(items[0]["source"], Value::String("aggregate".to_owned()));
    assert_eq!(items[0]["sampleCount"], Value::from(1));
    assert_eq!(items[0]["value"], Value::from(2.5));
    assert_eq!(items[0]["firstObservedAt"], Value::String(expired.clone()));
    assert_eq!(
        body["series"]["firstObservedAt"],
        Value::String(expired.clone())
    );
    assert_eq!(body["series"]["observationCount"], Value::from(1));
}

/// The five-minute aligned instant a number of days before a single reference.
///
/// Backdated seeding has to land where the tiers cut their buckets, and every
/// anchor of one test is derived from one reference instant, so the distance
/// between two anchors is exactly the number of days between them.
fn aligned_before(reference: i64, days: i64) -> i64 {
    (reference - days * 86_400).div_euclid(300) * 300
}

fn instant_at(unix_seconds: i64) -> String {
    auth::format_rfc3339(
        time::OffsetDateTime::from_unix_timestamp(unix_seconds).expect("an instant a test chose"),
    )
}

/// The minimal wire fixture carrying a Node process reading of its own, so a
/// test can seed the history a tier is supposed to summarize.
fn fixture_process_report(
    agent_id: &str,
    sequence: u64,
    observed_at: &str,
    cpu_percent: f64,
) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(&fixture_report(agent_id, sequence, observed_at))
        .expect("the fixture is a JSON document");
    value["nodes"][0]["process"]["latest"]["cpu_percent"] = Value::from(cpu_percent);
    serde_json::to_vec(&value).unwrap()
}

/// One metric-history read of an already built URI, for the paging parameters
/// the shared helper does not carry.
async fn metric_history_page(harness: &Harness, cookie: &str, uri: &str) -> (StatusCode, Value) {
    let response = harness.send(admin_get(uri, Some(cookie))).await;
    let status = response.status();
    (status, body_json(response).await)
}

/// Story 58 and 59 (issue #214): a stretch older than the raw window is answered
/// by the bucket that counted it — at the tier's own grain, with the counted
/// observations, the extremes and the newest reading kept — and never by a raw
/// sample the Server no longer stores.
#[tokio::test]
async fn a_stretch_beyond_the_raw_window_is_answered_by_its_bucket() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;

    // Ascending backdated Reports: the Server counts a first-time observation
    // even when it is already older than the raw window, and what that counted
    // observation leaves behind is the tiers.
    let reference = auth::now_utc().unix_timestamp();
    let ten_days = aligned_before(reference, 10);
    let eight_days = aligned_before(reference, 8);
    let six_days = aligned_before(reference, 6);
    let three_days = aligned_before(reference, 3);
    let mut sequence = 0_u64;
    let report = |sequence: &mut u64, unix_seconds: i64, cpu_percent: f64| {
        *sequence += 1;
        fixture_process_report(&agent_id, *sequence, &instant_at(unix_seconds), cpu_percent)
    };
    // Three observations inside one coarse bucket and three different fine
    // buckets: the five-minute tier counts all three, the minute tier one each.
    for (offset, cpu_percent) in [(0_i64, 1.0_f64), (60, 9.0), (120, 5.0)] {
        let (status, value) = submit(
            &harness,
            &credential,
            report(&mut sequence, ten_days + offset, cpu_percent),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }
    // One observation on each side of the seven day tier boundary, so a single
    // answer has to serve two grains.
    for (unix_seconds, cpu_percent) in [(eight_days, 3.0_f64), (six_days, 6.0)] {
        let (status, value) = submit(
            &harness,
            &credential,
            report(&mut sequence, unix_seconds, cpu_percent),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }
    for (offset, cpu_percent) in [(0_i64, 2.0_f64), (60, 8.0), (120, 4.0)] {
        let (status, value) = submit(
            &harness,
            &credential,
            report(&mut sequence, three_days + offset, cpu_percent),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    // Every one of these observations is older than the raw window, so no row
    // can answer for it: whatever the read returns is a bucket.
    assert_eq!(
        harness
            .count_where("node_metric_samples", "metric = 'process_cpu_percent'")
            .await,
        0,
        "the raw window is the policy's and none of these instants is inside it"
    );

    // Older than seven days the five-minute tier answers, and the bucket keeps
    // the spike and the count the raw samples would have shown.
    let from = instant_at(ten_days - 3_600);
    let to = instant_at(ten_days + 1_800);
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
    assert_eq!(body["grain"], Value::String("5m".to_owned()));
    assert_eq!(body["historyHorizonDays"], Value::from(30));
    assert_eq!(body["rawRetentionDays"], Value::from(1));
    assert!(body["availability"].is_null());
    assert_eq!(body["truncated"], Value::Bool(false));
    assert!(body["continuation"].is_null());
    let items = body["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "one coarse bucket answers the stretch: {body}"
    );
    let bucket = &items[0];
    assert_eq!(bucket["observedAt"], Value::String(instant_at(ten_days)));
    assert_eq!(bucket["grain"], Value::String("5m".to_owned()));
    assert_eq!(bucket["source"], Value::String("aggregate".to_owned()));
    assert_eq!(bucket["sampleCount"], Value::from(3), "{bucket}");
    assert_eq!(bucket["minValue"], Value::from(1.0), "{bucket}");
    assert_eq!(
        bucket["maxValue"],
        Value::from(9.0),
        "the bucket keeps the spike: {bucket}"
    );
    assert_eq!(
        bucket["value"],
        Value::from(5.0),
        "the bucket answers with its newest reading: {bucket}"
    );
    assert_eq!(
        bucket["lastObservedAt"],
        Value::String(instant_at(ten_days + 120))
    );
    let segments = body["segments"].as_array().unwrap();
    assert_eq!(segments.len(), 1, "{body}");
    assert_eq!(segments[0]["grain"], Value::String("5m".to_owned()));
    assert_eq!(segments[0]["source"], Value::String("aggregate".to_owned()));
    assert_eq!(segments[0]["pointCount"], Value::from(1));

    // Inside the last seven days the minute tier answers, one bucket per
    // observation, and the coverage the buckets prove is the coverage the
    // samples proved before them.
    let from = instant_at(three_days - 3_600);
    let to = instant_at(three_days + 1_800);
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
    assert_eq!(body["grain"], Value::String("1m".to_owned()));
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{body}");
    for (item, (offset, cpu_percent)) in items.iter().zip([(0_i64, 2.0_f64), (60, 8.0), (120, 4.0)])
    {
        assert_eq!(
            item["observedAt"],
            Value::String(instant_at(three_days + offset))
        );
        assert_eq!(item["grain"], Value::String("1m".to_owned()));
        assert_eq!(item["source"], Value::String("aggregate".to_owned()));
        assert_eq!(item["sampleCount"], Value::from(1), "{item}");
        assert_eq!(item["value"], Value::from(cpu_percent), "{item}");
        assert_eq!(item["minValue"], Value::from(cpu_percent), "{item}");
        assert_eq!(item["maxValue"], Value::from(cpu_percent), "{item}");
    }
    assert_eq!(
        body["series"]["coverageSeconds"],
        Value::from(120),
        "the buckets prove the minute of coverage the samples proved: {body}"
    );

    // A range straddling the seven day boundary is served by both tiers in one
    // answer, and the answer names the grain of each stretch. The silence
    // between the two stretches is a gap, never a line and never coverage.
    let from = instant_at(eight_days - 3_600);
    let to = instant_at(six_days + 1_800);
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
        body["grain"],
        Value::String("1m".to_owned()),
        "the answer reports the finest grain it carries: {body}"
    );
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{body}");
    assert_eq!(
        items[0]["observedAt"],
        Value::String(instant_at(eight_days))
    );
    assert_eq!(items[0]["grain"], Value::String("5m".to_owned()));
    assert_eq!(items[0]["value"], Value::from(3.0));
    assert_eq!(items[1]["observedAt"], Value::String(instant_at(six_days)));
    assert_eq!(items[1]["grain"], Value::String("1m".to_owned()));
    assert_eq!(items[1]["value"], Value::from(6.0));
    let segments = body["segments"].as_array().unwrap();
    assert_eq!(segments.len(), 2, "{body}");
    assert_eq!(segments[0]["grain"], Value::String("5m".to_owned()));
    assert_eq!(segments[0]["pointCount"], Value::from(1));
    assert_eq!(segments[1]["grain"], Value::String("1m".to_owned()));
    assert_eq!(segments[1]["pointCount"], Value::from(1));
    let gaps = body["gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{body}");
    assert_eq!(gaps[0]["kind"], Value::String("collection_gap".to_owned()));
    assert_eq!(gaps[0]["from"], Value::String(instant_at(eight_days)));
    assert_eq!(gaps[0]["to"], Value::String(instant_at(six_days)));
    assert_eq!(gaps[0]["seconds"], Value::from(2 * 86_400));
    assert_eq!(
        body["series"]["coverageSeconds"],
        Value::from(0),
        "a silence no tier covers is never claimed as coverage: {body}"
    );
}

/// The reviewer's question for a budgeted answer: a truncated range hands back
/// the coordinate that pages strictly older points, so paging walks the whole
/// stretch with no repeat and no hole, and a cursor outside the requested range
/// is refused instead of answering a stretch nobody asked about.
#[tokio::test]
async fn a_truncated_answer_pages_older_without_a_hole() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;

    let reference = auth::now_utc().unix_timestamp();
    let three_days = aligned_before(reference, 3);
    let mut sequence = 0_u64;
    let report = |sequence: &mut u64, unix_seconds: i64, cpu_percent: f64| {
        *sequence += 1;
        fixture_process_report(&agent_id, *sequence, &instant_at(unix_seconds), cpu_percent)
    };
    for (offset, cpu_percent) in [(0_i64, 2.0_f64), (60, 8.0), (120, 4.0)] {
        let (status, value) = submit(
            &harness,
            &credential,
            report(&mut sequence, three_days + offset, cpu_percent),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let from = instant_at(three_days - 3_600);
    let to = instant_at(three_days + 1_800);
    let uri = format!(
        "{}&limit=2",
        history_uri(NODE_ID, "process_cpu_percent", Some(&from), Some(&to))
    );
    let (status, body) = metric_history_page(&harness, &session.cookie, &uri).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["grain"], Value::String("1m".to_owned()));
    assert_eq!(
        body["truncated"],
        Value::Bool(true),
        "the limit is smaller than the stretch holds: {body}"
    );
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{body}");
    assert_eq!(
        items[0]["observedAt"],
        Value::String(instant_at(three_days + 60))
    );
    assert_eq!(
        items[1]["observedAt"],
        Value::String(instant_at(three_days + 120))
    );
    let continuation = body["continuation"].as_str().unwrap().to_owned();
    assert_eq!(
        continuation,
        instant_at(three_days + 60),
        "the cursor is the oldest point this answer carries: {body}"
    );

    // The next page carries strictly older points: the cursor is an exclusive
    // bound, so nothing repeats and nothing is skipped.
    let uri = format!(
        "{}&before={}&limit=2",
        history_uri(NODE_ID, "process_cpu_percent", Some(&from), Some(&to)),
        continuation
    );
    let (status, body) = metric_history_page(&harness, &session.cookie, &uri).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["truncated"], Value::Bool(false));
    assert!(body["continuation"].is_null(), "{body}");
    assert_eq!(body["to"], Value::String(continuation.clone()));
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{body}");
    assert_eq!(
        items[0]["observedAt"],
        Value::String(instant_at(three_days))
    );

    // A cursor that is not strictly inside the requested range cannot narrow it.
    for before in [
        instant_at(three_days - 3_600),
        instant_at(three_days + 3_600),
        "yesterday".to_owned(),
    ] {
        let uri = format!(
            "{}&before={}",
            history_uri(NODE_ID, "process_cpu_percent", Some(&from), Some(&to)),
            before
        );
        let (status, body) = metric_history_page(&harness, &session.cookie, &uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"]["code"], "invalid_history_range");
    }
}

/// Issue #214 acceptance (restart): the aggregate tiers are durable storage, not
/// process state. A Server that reopens the same database serves the same
/// buckets — and the observations an earlier process already counted stay
/// counted, which is the double accumulation the ticket names as its main risk.
#[tokio::test]
async fn a_restarted_server_serves_the_same_buckets_and_never_re_accumulates() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;

    // Three observations inside ONE five-minute bucket ten days back, older
    // than the raw window: only the bucket can answer for them.
    let reference = auth::now_utc().unix_timestamp();
    let ten_days = aligned_before(reference, 10);
    let mut sequence = 0_u64;
    let report = |sequence: &mut u64, unix_seconds: i64, cpu_percent: f64| {
        *sequence += 1;
        fixture_process_report(&agent_id, *sequence, &instant_at(unix_seconds), cpu_percent)
    };
    let mut delivered = Vec::new();
    for (offset, cpu_percent) in [(0_i64, 1.0_f64), (60, 9.0), (120, 5.0)] {
        let body = report(&mut sequence, ten_days + offset, cpu_percent);
        delivered.push(body.clone());
        let (status, value) = submit(&harness, &credential, body).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }
    assert_eq!(
        harness
            .count_where("node_metric_samples", "metric = 'process_cpu_percent'")
            .await,
        0,
        "none of these instants is inside the raw window"
    );

    let from = instant_at(ten_days - 3_600);
    let to = instant_at(ten_days + 1_800);
    let (status, before) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{before}");
    let items = before["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{before}");
    assert_eq!(items[0]["sampleCount"], Value::from(3), "{before}");
    assert_eq!(items[0]["minValue"], Value::from(1.0), "{before}");
    assert_eq!(items[0]["maxValue"], Value::from(9.0), "{before}");
    assert_eq!(items[0]["value"], Value::from(5.0), "{before}");
    let buckets = harness
        .count_where(
            "node_metric_aggregates",
            "metric = 'process_cpu_percent' AND grain_seconds = 300",
        )
        .await;
    assert_eq!(buckets, 1, "one coarse bucket counted the three");

    // A real restart: the router, the pool and the policy state all go away, and
    // the next Server opens the same durable directory.
    let harness = harness.restart().await;

    let (status, after) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(
        after["items"], before["items"],
        "the tiers are rows on disk: the restarted Server serves what the last one served"
    );
    assert_eq!(after["segments"], before["segments"]);
    assert_eq!(after["coverageSeconds"], before["coverageSeconds"]);
    assert_eq!(
        harness
            .count_where(
                "node_metric_aggregates",
                "metric = 'process_cpu_percent' AND grain_seconds = 300"
            )
            .await,
        buckets,
        "opening the database does not re-derive a bucket"
    );

    // The Agent delivers its last Report again, byte for byte, as a retrying
    // Agent would. The stored Receipt answers and nothing is counted twice.
    let (status, value) = submit(&harness, &credential, delivered[2].clone()).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        harness
            .count_where(
                "node_metric_aggregates",
                "metric = 'process_cpu_percent' AND grain_seconds = 300"
            )
            .await,
        buckets
    );

    // A new Report carrying the very observation the Server already counted
    // below the floor is a replay, not a second observation.
    let carried_again = report(&mut sequence, ten_days + 120, 7.0);
    let (status, value) = submit(&harness, &credential, carried_again).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert!(
        value["receipt"]["disposition"].is_string(),
        "the retrying Agent still gets its receipt: {value}"
    );
    let (status, replayed) = metric_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process_cpu_percent",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(
        replayed["series"]["replayedCount"],
        Value::from(1),
        "the carried instant is a replay: {replayed}"
    );
    let items = replayed["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "a replay opens no second bucket: {replayed}"
    );
    assert_eq!(
        items[0]["sampleCount"],
        Value::from(3),
        "a replay is never accumulated again: {replayed}"
    );
    assert_eq!(items[0]["minValue"], Value::from(1.0), "{replayed}");
    assert_eq!(items[0]["maxValue"], Value::from(9.0), "{replayed}");
    assert_eq!(
        items[0]["value"],
        Value::from(5.0),
        "not even the newest reading moves: {replayed}"
    );
    assert_eq!(
        harness
            .count_where(
                "node_metric_aggregates",
                "metric = 'process_cpu_percent' AND grain_seconds = 300"
            )
            .await,
        buckets
    );
}
