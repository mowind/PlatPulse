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
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tower::ServiceExt;

use platpulse_core::{AgentReport, ReceiptDisposition};
use platpulse_server::capacity::{
    ADMIN_RECENT_INTERVAL_LIMIT, CapacityConfig, CapacityProtection, ProtectionTransition,
    SkippedScope, recent_intervals, record_skipped_series, sample_filesystem,
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

/// An interval a previous process left open, written directly: a test that needs
/// that durable state must not depend on a policy opening it first.
///
/// The recorded thresholds are the largest ones the Server can persist, which is
/// the same "no disk can clear this floor" declaration forced_policy makes.
async fn seed_open_interval(harness: &Harness, interval_id: &str) {
    let mount = harness.mount();
    let available = sample_filesystem(&mount).unwrap().available_bytes;
    sqlx::query(
        "INSERT INTO capacity_protection_intervals (interval_id, source_mount, started_at, started_reason, opened_total_bytes, opened_available_bytes, pause_below_bytes, resume_above_bytes, created_at, updated_at) VALUES (?, ?, '2026-03-01T02:00:00Z', 'low_space', ?, ?, ?, ?, '2026-03-01T02:00:00Z', '2026-03-01T02:00:00Z')",
    )
    .bind(interval_id)
    .bind(mount.to_string_lossy().to_string())
    .bind(available as i64)
    .bind(available as i64)
    .bind(CapacityConfig::MAX_PERSISTED_BYTES as i64)
    .bind(CapacityConfig::MAX_PERSISTED_BYTES as i64)
    .execute(harness.pool())
    .await
    .unwrap();
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
///
/// Every instant stays the fixture's own, in August 2026. A case that asserts a
/// stored sample has to move them onto the Server clock with [`report_at`]: raw
/// metric samples are held only for the 24 hours
/// of the `raw_metric_sample` retention policy (issue #213), so a Report stamped
/// in August is released as it arrives and stores nothing.
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

/// The newest instant a report carries, so it can be moved onto the clock whole.
fn latest_instant(value: &Value) -> Option<OffsetDateTime> {
    match value {
        Value::String(text) => OffsetDateTime::parse(text, &Rfc3339).ok(),
        Value::Array(items) => items.iter().filter_map(latest_instant).max(),
        Value::Object(fields) => fields.values().filter_map(latest_instant).max(),
        _ => None,
    }
}

/// Move every instant in a report by the same offset, keeping the report's own
/// relative timing.
fn shift_instants(value: &mut Value, delta: time::Duration) {
    match value {
        Value::String(text) => {
            if let Ok(instant) = OffsetDateTime::parse(text, &Rfc3339) {
                *text = auth::format_rfc3339(instant + delta);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                shift_instants(item, delta);
            }
        }
        Value::Object(fields) => {
            for field in fields.values_mut() {
                shift_instants(field, delta);
            }
        }
        _ => {}
    }
}

/// The same report with every instant moved onto the Server clock, its newest
/// instant landing on `at`.
///
/// Raw metric samples are kept for the 24 hours of the `raw_metric_sample`
/// retention policy (issue #213), so a Report stamped in August is released the
/// moment it is stored. Any case that asserts the Server wrote a sample has to
/// deliver one the raw window still holds, as a live Agent does.
fn report_at(report: &AgentReport, at: OffsetDateTime) -> Value {
    let mut value = serde_json::to_value(report).unwrap();
    let newest = latest_instant(&value).expect("the fixture carries observed instants");
    shift_instants(&mut value, at - newest);
    value
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
    // Its readings are the ones a live Agent would deliver, so the only reason
    // the Server stores no sample is the pause.
    let paused_report = report_at(
        &fixture_report(&agent_id, 1, 1),
        auth::now_utc() - time::Duration::minutes(10),
    );
    let (status, value) = submit(
        &harness,
        &credential,
        serde_json::to_vec(&paused_report).unwrap(),
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
        open.skipped_sample_count, 14,
        "the eight Host series, the two Node process series, the two Node key \
         heights and the two recorded-state components one Report carries \
         produced fourteen skipped deliveries"
    );
    assert_eq!(open.skipped_series_total, 14);
    let metrics = skipped_series_keys(open);
    for expected in [
        "cpu_percent",
        "memory_total_bytes",
        "memory_used_bytes",
        "load1",
        "load5",
        "load15",
        "network_rx_bytes_per_sec",
        "network_tx_bytes_per_sec",
        "process_cpu_percent",
        "process_memory_percent",
        // Issue #217: a paused Node also loses its key consensus heights,
        // which are numeric series, and its recorded sync/consensus state
        // deliveries, which are named by the component rather than the metric.
        "sync_current_block",
        "sync_highest_block",
        "sync",
        "consensus",
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
    // Issue #217: a paused Node also loses its five key heights and its two
    // recorded sync/consensus state deliveries, so the Owner sees the whole
    // cost of the pause through the same surface.
    assert_eq!(overview["recentIntervals"][0]["skippedSampleCount"], 14);
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
    // Its readings are newer than the ones the pause skipped, as a resumed
    // Agent's would be.
    let resumed_report = report_at(&fixture_report(&agent_id, 1, 2), auth::now_utc());
    let (status, value) = submit(
        &harness,
        &credential,
        serde_json::to_vec(&resumed_report).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        harness.count("host_metric_samples").await,
        8,
        "one Report states its Host's cpu, memory, load and network once each"
    );
    assert_eq!(
        harness.count("node_metric_samples").await,
        4,
        "the resumed Report stores its two process series and the two key heights\
         it states; the fixture's consensus component is unsupported, so it\
         states no height at all"
    );

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
        closed.skipped_sample_count, 14,
        "ending the interval keeps the record of what was lost, key heights and\
         recorded states included"
    );
    assert_eq!(
        closed.skipped_series_total, 14,
        "the visible gap survives the recovery"
    );
}

/// Design §11.4: a restart adopts the interval a previous process left open.
///
/// Startup reconciliation was the only thing that ever read that record. If it
/// failed on its own, the process kept the default unprotected gate, and because
/// the partial unique index allows a single open interval, every later attempt
/// to open one was refused: protection stayed defeated while the recorded
/// interval stayed open forever. A tick now retries the read before it decides
/// anything.
#[tokio::test]
async fn a_tick_adopts_an_open_interval_the_startup_read_missed() {
    let mut harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let available = sample_filesystem(&harness.mount()).unwrap().available_bytes;

    // What a previous process left behind: one open interval, and no memory of
    // it in this process.
    const ORPHAN_INTERVAL: &str = "0195f2a1-0600-4600-8600-000000000600";
    seed_open_interval(&harness, ORPHAN_INTERVAL).await;

    let pressure = forced_policy(&harness);
    // No reconcile: this is the tick that follows a startup read that failed.
    let transition = pressure.check_now(harness.pool()).await.unwrap();
    assert_eq!(
        transition,
        ProtectionTransition::Unchanged,
        "the adopted interval already matches today's policy"
    );
    let status = pressure.status();
    assert!(
        status.protected,
        "the retried read pauses optional history instead of writing it"
    );
    assert_eq!(status.active_interval_id.as_deref(), Some(ORPHAN_INTERVAL));
    assert_eq!(
        harness.count("capacity_protection_intervals").await,
        1,
        "adoption never opens a second interval"
    );
    harness.install_capacity(Arc::clone(&pressure));

    // The adopted interval really gates optional history.
    let (status_code, value) = submit(
        &harness,
        &credential,
        serde_json::to_vec(&fixture_report(&agent_id, 1, 1)).unwrap(),
    )
    .await;
    assert_eq!(status_code, StatusCode::OK, "{value}");
    assert_eq!(harness.count("host_metric_samples").await, 0);
    assert_eq!(harness.count("node_metric_samples").await, 0);
    let intervals = recent_intervals(harness.pool(), ADMIN_RECENT_INTERVAL_LIMIT)
        .await
        .unwrap();
    assert_eq!(intervals[0].interval_id, ORPHAN_INTERVAL);
    assert_eq!(intervals[0].skipped_sample_count, 14);

    // And when the pressure is gone, a tick closes the adopted interval instead
    // of leaving it open forever.
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
    released.check_now(harness.pool()).await.unwrap();
    assert!(!released.status().protected);
    let intervals = recent_intervals(harness.pool(), ADMIN_RECENT_INTERVAL_LIMIT)
        .await
        .unwrap();
    assert_eq!(intervals.len(), 1);
    assert_eq!(intervals[0].ended_reason.as_deref(), Some("resumed"));
    assert!(
        intervals[0].resumed_available_bytes.unwrap() >= released_floor,
        "the recorded release measurement is the one that closed it"
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

/// Run a real Doctor run over HTTP and return its storage capacity check.
async fn storage_capacity_check(harness: &Harness, session: &Session) -> Value {
    let response = harness
        .send(admin_post("/api/admin/v1/doctor", session, ""))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    while platpulse_server::operations::process_operations(&harness.state)
        .await
        .unwrap()
        > 0
    {}
    let response = harness
        .send(admin_get("/api/admin/v1/doctor", Some(&session.cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    body["checks"]
        .as_array()
        .expect("a finished Doctor run reports its checks")
        .iter()
        .find(|check| check["checkId"] == "storage_capacity")
        .cloned()
        .expect("Doctor reports the storage capacity check")
}

/// Issue #212 review: the recorded gap counts readings the Server did not
/// store, not repeat receipts of one reading. An Agent that re-sends its
/// last-good sample while history is paused has not lost a second sample.
#[tokio::test]
async fn a_replayed_reading_is_not_counted_as_a_second_lost_sample() {
    let mut harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;

    let pressure = forced_policy(&harness);
    pressure.reconcile(harness.pool()).await.unwrap();
    harness.install_capacity(Arc::clone(&pressure));

    let first = fixture_report(&agent_id, 1, 1);
    let (status, value) = submit(&harness, &credential, serde_json::to_vec(&first).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let intervals = recent_intervals(harness.pool(), ADMIN_RECENT_INTERVAL_LIMIT)
        .await
        .unwrap();
    assert_eq!(intervals.len(), 1);
    assert_eq!(intervals[0].skipped_sample_count, 14);

    // The second Report is a new Report (new id, later generation time) that
    // replays the same readings at the same observation times.
    let repeated = fixture_report(&agent_id, 1, 2);
    let mut first_value = serde_json::to_value(&first).unwrap();
    let mut repeated_value = serde_json::to_value(&repeated).unwrap();
    for field in ["report_id", "report_sequence", "generated_at"] {
        assert_ne!(
            first_value[field], repeated_value[field],
            "the replayed Report is a new Report, not a duplicate receipt"
        );
        first_value[field] = Value::Null;
        repeated_value[field] = Value::Null;
    }
    assert_eq!(
        first_value, repeated_value,
        "every observation is the same reading at the same observation time"
    );

    let (status, value) = submit(
        &harness,
        &credential,
        serde_json::to_vec(&repeated).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        receipt_from(&value).disposition,
        ReceiptDisposition::Accepted
    );

    let intervals = recent_intervals(harness.pool(), ADMIN_RECENT_INTERVAL_LIMIT)
        .await
        .unwrap();
    assert_eq!(intervals.len(), 1, "the replay opens no second interval");
    // The ten metric series and the two sync heights keep the observation times
    // they already lost, so replaying them counts nothing new. The consensus
    // component never attempted, so its state delivery is stamped with the
    // Report's own generation time: the replay states a later instant, and that
    // one delivery is lost again rather than counted twice over the same one.
    assert_eq!(
        intervals[0].skipped_sample_count, 15,
        "a replayed reading is not counted twice at one instant"
    );
    assert_eq!(intervals[0].skipped_series_total, 14);
    assert_eq!(harness.count("capacity_skipped_series").await, 14);
    assert_eq!(harness.count("host_metric_samples").await, 0);
    assert_eq!(harness.count("node_metric_samples").await, 0);
}

/// Issue #212 review: the count is a high-water mark, so an accepted Report that
/// re-sends older readings after newer ones cannot inflate it. A late-arriving
/// older reading is under-counted: telling one apart from a replay needs one
/// ledger row per skipped reading, which would cost at least as much disk as the
/// history the pause is protecting. The number never claims more distinct lost
/// readings than the series accounted for.
#[tokio::test]
async fn skipped_counting_advances_only_on_a_reading_newer_than_the_mark() {
    let harness = Harness::boot().await;
    const INTERVAL: &str = "0195f2a1-0600-4600-8600-000000000601";
    const AGENT: &str = "0195f2a1-0011-4011-8011-000000000011";
    const METRIC: &str = "network_rx_bytes_per_sec";
    seed_open_interval(&harness, INTERVAL).await;

    let mut counts = Vec::new();
    let mut windows = Vec::new();
    for observed_at in [
        "2026-03-01T02:00:00Z",
        "2026-03-01T02:05:00Z",
        "2026-03-01T02:00:00Z",
        "2026-03-01T02:05:00Z",
        "2026-03-01T02:10:00Z",
    ] {
        let mut tx = harness.pool().begin().await.unwrap();
        record_skipped_series(
            &mut tx,
            INTERVAL,
            SkippedScope::Host,
            AGENT,
            METRIC,
            "",
            observed_at,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let (count, first, last): (i64, String, String) = sqlx::query_as(
            "SELECT skipped_count, first_skipped_at, last_skipped_at FROM capacity_skipped_series WHERE interval_id = ? AND scope_kind = ? AND scope_key = ? AND metric = ?",
        )
        .bind(INTERVAL)
        .bind(SkippedScope::Host.as_str())
        .bind(AGENT)
        .bind(METRIC)
        .fetch_one(harness.pool())
        .await
        .unwrap();
        counts.push(count);
        windows.push((first, last));
    }

    assert_eq!(
        counts,
        vec![1, 2, 2, 2, 3],
        "only a reading newer than every counted reading advances the count"
    );
    assert_eq!(
        windows[1].0, "2026-03-01T02:00:00Z",
        "the recorded window still starts at the first reading it counted"
    );
    assert_eq!(
        windows[1].1, "2026-03-01T02:05:00Z",
        "and ends at the newest reading it counted"
    );
    assert_eq!(
        windows[4].1, "2026-03-01T02:10:00Z",
        "a newer reading moves the mark forward"
    );
}
/// Issue #212 review and the AGENTS.md last-good rule: a state filesystem that
/// stops being measurable is not Healthy. The retained measurement still
/// explains the protection decision, and Doctor reports the unknown state as a
/// warning instead of passing on stale evidence.
#[tokio::test]
async fn doctor_warns_when_the_state_filesystem_cannot_be_measured() {
    let mut harness = Harness::boot().await;
    let session = owner_session(&harness).await;

    // The measured directory is separate from the Server's own state
    // directory, so the test can make it disappear without touching the
    // database it asserts on. The floor is below any real disk, so the policy
    // is enabled and is never protected: the unmeasurable sample is what
    // Doctor has to report, not a pause.
    //
    // The test then runs Doctor exactly once, because the overview reports the
    // newest completed run ordered by created_at and operation_id (design
    // §8.4); two runs inside one second are ordered by id, so a single run is
    // what makes the check the Operator reads unambiguous.
    let measured = TempDir::new().unwrap();
    let config = CapacityConfig::from_declared(
        true,
        Some(1),
        Some(1),
        None,
        Some(PathBuf::from("test-capacity-floor")),
    )
    .unwrap();
    let capacity = Arc::new(CapacityProtection::new(
        config,
        Some(&measured.path().join("server.db")),
    ));
    capacity.check_now(harness.pool()).await.unwrap();
    assert!(
        capacity.status().sample.is_some(),
        "the measurement the policy acts on is recorded"
    );
    assert!(capacity.status().sampling_error.is_none());
    harness.install_capacity(Arc::clone(&capacity));

    // The filesystem disappears between ticks: the last good measurement is
    // retained and the failure is recorded rather than reported as healthy.
    let mount = measured.path().to_path_buf();
    measured.close().unwrap();
    assert!(!mount.exists(), "the measured directory is gone");
    capacity.check_now(harness.pool()).await.unwrap();
    assert!(
        capacity.status().sample.is_some(),
        "the last good measurement is retained"
    );
    assert!(capacity.status().sampling_error.is_some());

    let check = storage_capacity_check(&harness, &session).await;
    assert_eq!(
        check["status"], "warning",
        "an unmeasurable state filesystem must not be reported as healthy: {check}"
    );
    assert!(
        check["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("could not be measured"),
        "{check}"
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
