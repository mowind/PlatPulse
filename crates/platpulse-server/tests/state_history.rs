//! Recorded synchronization and consensus state history at the HTTP boundary
//! (issue #217, stories 53 and 59, design §11.4 and §11.6).
//!
//! The unit tests in crate::state_history drive the state vector, the ledger and
//! the anchor rule directly. This suite drives the same seam the way an Operator
//! and an Agent do: it builds the full application with build_app against a
//! temporary SQLite database, enrolls a real Agent through the Admin and Agent
//! HTTP APIs, submits real Reports, and reads the recorded states back through
//! the Owner-only Admin route.
//!
//! It proves the claims the issue asks for at the surface an Operator uses:
//!
//! 1. The recorded state changes and the failures between them are readable with
//!    the evidence that belongs to each one: a failure keeps the instant of the
//!    value it could not refresh, and a failed probe is Unknown rather than
//!    false.
//! 2. An unchanged state anchors once an hour instead of repeating itself, and a
//!    Node that stops reporting records no state at all.
//! 3. A replayed or corrected delivery is counted without inventing a state
//!    change, and a low-space pause is a visible protection gap with its counted
//!    losses instead of a zero.
//! 4. The key consensus heights ride the metric surface by height, so a reader
//!    who wants the numbers never has to infer them from Block history.
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
    /// recorded states, the ledger and the policy rows have to be read back from
    /// storage instead of surviving in process memory.
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

/// The minimal wire fixture, so a base component can be cloned and mutated.
fn fixture_value() -> Value {
    serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap()
}

/// A successful sync probe at `at`, syncing or not, at the two key heights.
fn sync_ok(at: &str, syncing: bool, current_block: u64, highest_block: u64) -> Value {
    let mut component = fixture_value()["nodes"][0]["chain"]["sync"].clone();
    component["status"] = Value::String("ok".to_owned());
    component["attempted_at"] = Value::String(at.to_owned());
    component["latest_observed_at"] = Value::String(at.to_owned());
    component["value_revision"] = Value::from(1);
    component["latest"]["syncing"] = Value::Bool(syncing);
    component["latest"]["current_block"] = Value::from(current_block);
    component["latest"]["highest_block"] = Value::from(highest_block);
    component
}

/// A failed sync probe: an error and no value at all, which is what a probe that
/// could not reach the Node really delivers.
fn sync_failed(at: &str, code: &str) -> Value {
    let mut component = fixture_value()["nodes"][0]["chain"]["sync"].clone();
    component["status"] = Value::String("error".to_owned());
    component["attempted_at"] = Value::String(at.to_owned());
    let object = component.as_object_mut().unwrap();
    object.remove("latest");
    object.remove("latest_observed_at");
    component["error"] = serde_json::json!({
        "code": code,
        "message": "the Chain B RPC endpoint did not answer the probe"
    });
    component
}

/// A successful consensus probe: exactly the six bounded fields the wire type
/// carries, so the shape stays as strict as the contract.
fn consensus_ok(at: &str, qc: u64, lock: u64, commit: u64) -> Value {
    let mut component = fixture_value()["nodes"][0]["chain"]["consensus"].clone();
    component["status"] = Value::String("ok".to_owned());
    component["attempted_at"] = Value::String(at.to_owned());
    component["latest_observed_at"] = Value::String(at.to_owned());
    component["value_revision"] = Value::from(1);
    component["latest"] = serde_json::json!({
        "epoch": 1,
        "view_number": 7,
        "validator": true,
        "highest_qc_block": qc,
        "highest_lock_block": lock,
        "highest_commit_block": commit
    });
    component
}

/// The fixture consensus component as it stands: unsupported, with no attempt.
fn consensus_unsupported() -> Value {
    fixture_value()["nodes"][0]["chain"]["consensus"].clone()
}

/// One Report carrying the two chain state components the test wants, with the
/// Report instant independent of each component attempt instant.
fn state_report(
    agent_id: &str,
    sequence: u64,
    generated_at: &str,
    sync: Value,
    consensus: Value,
) -> Vec<u8> {
    let mut value = fixture_value();
    let canonical: Value = serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_canonical.json"
    ))
    .unwrap();
    value["nodes"][0]["process"] = canonical["nodes"][0]["process"].clone();
    let generated = Value::String(generated_at.to_owned());
    value["nodes"][0]["process"]["attempted_at"] = generated.clone();
    value["nodes"][0]["process"]["latest_observed_at"] = generated.clone();
    value["host"]["network_throughput"]["attempted_at"] = generated.clone();
    value["host"]["network_throughput"]["latest_observed_at"] = generated.clone();
    value["nodes"][0]["chain"]["sync"] = sync;
    value["nodes"][0]["chain"]["consensus"] = consensus;
    value["agent_id"] = Value::String(agent_id.to_owned());
    value["report_sequence"] = Value::from(sequence);
    value["report_id"] = Value::String(format!(
        "0195f2a1-00{sequence:02x}-4013-8013-0000000000{sequence:02x}"
    ));
    value["generated_at"] = generated;
    serde_json::to_vec(&value).unwrap()
}

fn state_uri(node_id: &str, component: &str, from: Option<&str>, to: Option<&str>) -> String {
    let mut uri = format!("/api/admin/v1/nodes/{node_id}/state-history?component={component}");
    if let Some(from) = from {
        uri.push_str(&format!("&from={from}"));
    }
    if let Some(to) = to {
        uri.push_str(&format!("&to={to}"));
    }
    uri
}

fn state_page_uri(
    node_id: &str,
    component: &str,
    from: &str,
    to: &str,
    before: Option<&str>,
    limit: i64,
) -> String {
    let mut uri = format!(
        "/api/admin/v1/nodes/{node_id}/state-history?component={component}&from={from}&to={to}&limit={limit}"
    );
    if let Some(before) = before {
        uri.push_str(&format!("&before={before}"));
    }
    uri
}

async fn state_history(
    harness: &Harness,
    cookie: Option<&str>,
    node_id: &str,
    component: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> (StatusCode, Value) {
    let request = admin_get(&state_uri(node_id, component, from, to), cookie);
    let response = harness.send(request).await;
    let status = response.status();
    (status, body_json(response).await)
}

/// Story 53: an Operator reads the recorded state changes of one Node component
/// with the evidence of each delivery, and every other principal is refused.
#[tokio::test]
async fn owner_reads_recorded_state_changes_and_other_principals_are_refused() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let instant = |seconds: i64| auth::format_rfc3339(base + time::Duration::seconds(seconds));

    // A Node that was syncing, then caught up, then lost its RPC endpoint.
    let reports = [
        state_report(
            &agent_id,
            1,
            &instant(0),
            sync_ok(&instant(0), false, 100, 100),
            consensus_ok(&instant(0), 90, 88, 86),
        ),
        state_report(
            &agent_id,
            2,
            &instant(60),
            sync_ok(&instant(60), true, 110, 200),
            consensus_ok(&instant(60), 190, 188, 186),
        ),
        state_report(
            &agent_id,
            3,
            &instant(120),
            sync_failed(&instant(120), "rpc_unavailable"),
            consensus_unsupported(),
        ),
    ];
    for report in reports {
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let from = instant(-300);
    let to = instant(600);
    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["nodeId"], Value::String(NODE_ID.to_owned()));
    assert_eq!(body["component"], Value::String("sync".to_owned()));
    assert_eq!(body["anchorSeconds"], Value::from(3_600));
    assert_eq!(body["retentionDays"], Value::from(30));
    assert_eq!(body["truncated"], Value::Bool(false));
    assert!(body["continuation"].is_null());
    assert!(body["availability"].is_null(), "{body}");
    assert_eq!(body["series"]["observed"], Value::Bool(true));

    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3, "{body}");
    // Oldest first, exactly like every other history surface.
    assert_eq!(entries[0]["observedAt"], Value::String(instant(0)));
    assert_eq!(entries[0]["entryKind"], "change");
    assert_eq!(entries[0]["collectionState"], "ok");
    assert_eq!(entries[0]["valueSource"], "current");
    assert_eq!(entries[0]["valueObservedAt"], Value::String(instant(0)));
    assert!(entries[0]["errorCode"].is_null(), "{body}");
    assert_eq!(entries[0]["syncing"], Value::Bool(false));
    assert!(
        entries[0]["delaySeconds"].as_i64().unwrap() >= 0,
        "a receipt is never stamped before its observation: {body}"
    );
    assert_eq!(entries[0]["clockSuspect"], Value::Bool(false));
    assert_eq!(entries[1]["observedAt"], Value::String(instant(60)));
    assert_eq!(entries[1]["collectionState"], "ok");
    assert_eq!(entries[1]["syncing"], Value::Bool(true));
    // The failed probe is the third state: an error, a value the Server still
    // holds from the last success, and an unknown sync flag instead of false.
    assert_eq!(entries[2]["observedAt"], Value::String(instant(120)));
    assert_eq!(entries[2]["collectionState"], "error");
    assert_eq!(entries[2]["valueSource"], "last_good");
    assert_eq!(entries[2]["valueObservedAt"], Value::String(instant(60)));
    assert_eq!(entries[2]["errorCode"], "rpc_unavailable");
    assert!(
        entries[2]["syncing"].is_null(),
        "a failed probe is not false: {entries:?}"
    );

    let series = &body["series"];
    assert_eq!(series["entryCount"], Value::from(3));
    assert_eq!(series["changeCount"], Value::from(3));
    assert_eq!(series["anchorCount"], Value::from(0));
    assert_eq!(series["replayedCount"], Value::from(0));
    assert_eq!(series["correctedCount"], Value::from(0));
    assert_eq!(series["latestCollectionState"], "error");
    assert_eq!(series["latestValueSource"], "last_good");
    assert_eq!(series["latestValueObservedAt"], Value::String(instant(60)));
    assert!(series["latestSyncing"].is_null());
    assert_eq!(series["entriesReturned"], Value::from(3));
    assert_eq!(body["cadenceSeconds"], Value::from(60));
    // Only the two stretches between recorded states are proved coverage, and
    // they are 60 seconds each.
    assert_eq!(body["coverageSeconds"], Value::from(120));
    assert!(body["gaps"].as_array().unwrap().is_empty(), "{body}");

    // The consensus series is independent of the sync one, and a component the
    // Node cannot collect is recorded as itself at the Report instant.
    let (status, consensus) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "consensus",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{consensus}");
    let consensus_entries = consensus["entries"].as_array().unwrap();
    // The consensus heights moved between the first two Reports while the
    // consensus state did not, so the second delivery recorded nothing: the
    // heights ride the metric surface, and an unchanged state is not a change.
    assert_eq!(consensus_entries.len(), 2, "{consensus}");
    assert_eq!(consensus["series"]["entryCount"], Value::from(3));
    assert_eq!(consensus["series"]["changeCount"], Value::from(2));
    assert_eq!(
        consensus_entries[0]["observedAt"],
        Value::String(instant(0))
    );
    assert_eq!(consensus_entries[0]["collectionState"], "ok");
    assert_eq!(consensus_entries[0]["valueSource"], "current");
    assert_eq!(consensus_entries[0]["syncing"], Value::Null);
    // A component the Node cannot collect carries no attempt, so its instant is
    // the Report instant, and the value the Server still holds is last good.
    assert_eq!(
        consensus_entries[1]["observedAt"],
        Value::String(instant(120)),
        "an unattempted component is stamped at the Report instant: {consensus}"
    );
    assert_eq!(consensus_entries[1]["collectionState"], "unsupported");
    assert_eq!(consensus_entries[1]["valueSource"], "last_good");
    assert_eq!(
        consensus_entries[1]["valueObservedAt"],
        Value::String(instant(60))
    );
    assert!(consensus_entries[1]["syncing"].is_null());

    // The range is Owner-only: an anonymous caller never reaches the handler,
    // and a Viewer has no administrative authority.
    let (status, body) = state_history(&harness, None, NODE_ID, "sync", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "auth_required");

    let hash = auth::hash_password(b"correct horse battery").unwrap();
    auth::create_viewer(harness.state.db(), "viewer1", &hash)
        .await
        .unwrap();
    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    let (status, body) =
        state_history(&harness, Some(&viewer.cookie), NODE_ID, "sync", None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "owner_required");

    // An unknown Node, an unknown component, a component that is not recorded
    // at all, a missing component and an inverted range are each refused with
    // the matching code instead of an empty answer.
    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        "0195f2a1-0014-4014-8014-000000000099",
        "sync",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found");

    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "process",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_component");

    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/nodes/{NODE_ID}/state-history"),
            Some(&session.cookie),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "invalid_component");

    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&instant(600)),
        Some(&instant(0)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_history_range");

    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some("yesterday"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_history_range");
}

/// Story 53: an unchanged state is anchored once an hour instead of repeating
/// itself on every Report, so a reader never mistakes a steady Node for a
/// sequence of transitions.
#[tokio::test]
async fn an_unchanged_state_anchors_once_an_hour_instead_of_repeating_itself() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let minute = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));

    // The same state three times: half an hour apart, then an hour and one
    // minute after the first.
    for (sequence, at) in [(1_u64, minute(0)), (2, minute(30)), (3, minute(61))] {
        let report = state_report(
            &agent_id,
            sequence,
            &at,
            sync_ok(&at, false, 100, 100),
            consensus_ok(&at, 90, 88, 86),
        );
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let from = minute(-5);
    let to = minute(120);
    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        2,
        "an unchanged state is not repeated: {body}"
    );
    assert_eq!(entries[0]["observedAt"], Value::String(minute(0)));
    assert_eq!(entries[0]["entryKind"], "change");
    assert_eq!(entries[1]["observedAt"], Value::String(minute(61)));
    assert_eq!(entries[1]["entryKind"], "anchor", "{body}");
    assert_eq!(entries[1]["syncing"], Value::Bool(false));
    // The half-hour delivery is evidence even though it wrote no row, and the
    // anchor is booked apart from a real change.
    assert_eq!(body["series"]["entryCount"], Value::from(3));
    assert_eq!(body["series"]["changeCount"], Value::from(1));
    assert_eq!(body["series"]["anchorCount"], Value::from(1));
    assert_eq!(body["series"]["entriesReturned"], Value::from(2));
    assert_eq!(body["series"]["latestCollectionState"], "ok");
    assert_eq!(body["series"]["latestSyncing"], Value::Bool(false));
    assert_eq!(body["cadenceSeconds"], Value::from(1_830));
    // The stretch between two recorded states is proved coverage, and the anchor
    // window is what keeps that claim honest: an unchanged state writes a row as
    // soon as the anchor interval is due, so two rows can sit one anchor interval
    // plus one delivery of this series apart and no further, and only a longer
    // separation is reported as a gap instead of as continuity. A silence shorter
    // than that is not provable from the recorded log, and this answer never claims
    // it.
    assert_eq!(body["coverageSeconds"], Value::from(3_660));
    // The anchor cadence is disclosed, so a reader can tell an unchanged state
    // from a missing report.
    assert_eq!(body["anchorSeconds"], Value::from(3_600));
    assert_eq!(
        harness
            .count_where("node_state_observations", "component = 'sync'")
            .await,
        2,
        "only the change and the anchor are rows"
    );
}

/// Story 59: a Node that stops reporting records nothing at all, so its silence
/// is never turned into a state and never read as a normal stretch.
#[tokio::test]
async fn a_node_that_stops_reporting_records_no_invented_state() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let minute = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));

    let report = state_report(
        &agent_id,
        1,
        &minute(0),
        sync_ok(&minute(0), false, 100, 100),
        consensus_ok(&minute(0), 90, 88, 86),
    );
    let (status, value) = submit(&harness, &credential, report).await;
    assert_eq!(status, StatusCode::OK, "{value}");

    // Nobody reports again, and the window reaches far past the last Report.
    let from = minute(-5);
    let to = minute(120);
    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "silence writes no state: {body}");
    assert_eq!(entries[0]["observedAt"], Value::String(minute(0)));
    // Nothing after the newest entry is claimed: the answer says exactly what
    // the record proves and never bridges the silence with a constant state.
    assert_eq!(body["coverageSeconds"], Value::from(0), "{body}");
    assert!(
        body["gaps"].as_array().unwrap().is_empty(),
        "a gap is only reported where a stretch between two states is missing: {body}"
    );
    assert_eq!(
        body["cadenceSeconds"],
        Value::from(0),
        "one delivery proves no cadence"
    );
    assert_eq!(body["series"]["entryCount"], Value::from(1));
    assert_eq!(body["series"]["changeCount"], Value::from(1));
    // The newest recorded instant is the evidence a reader ages: it never moves
    // forward on its own, and the Reports that stopped wrote nothing.
    assert_eq!(body["series"]["lastObservedAt"], Value::String(minute(0)));
    assert_eq!(
        harness
            .count_where(
                "node_state_observations",
                "node_id = '0195f2a1-0014-4014-8014-000000000014'"
            )
            .await,
        2,
        "one row for each component, and nothing for the silence"
    );
}

/// Story 53: a replay is counted without re-recording anything, and a corrected
/// delivery rewrites the instant it disagrees about instead of inventing a
/// second state change.
#[tokio::test]
async fn a_replay_and_a_correction_are_counted_without_inventing_a_state_change() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let minute = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));
    let from = minute(-5);
    let to = minute(30);
    let observed = minute(0);

    // The first delivery of one instant.
    let first = state_report(
        &agent_id,
        1,
        &minute(0),
        sync_ok(&observed, false, 200, 200),
        consensus_ok(&observed, 190, 188, 186),
    );
    let (status, value) = submit(&harness, &credential, first).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let (status, before_replay) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{before_replay}");
    let written = before_replay["entries"].as_array().unwrap();
    assert_eq!(written.len(), 1, "{before_replay}");
    let delivered_at = written[0]["receivedAt"].clone();

    // A receipt is the second the Server accepted the Report, and the log has to
    // tell two deliveries of one instant apart by it. Waiting past that second is
    // what makes the difference visible: a replay that restamped the row would
    // answer with the newer second instead of the one the row was written at.
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;

    // The same instant again, in a newer Report and with the same state: the
    // log already counted it, so nothing is written and the replay is counted.
    let replay = state_report(
        &agent_id,
        2,
        &minute(1),
        sync_ok(&observed, false, 200, 200),
        consensus_ok(&observed, 190, 188, 186),
    );
    let (status, value) = submit(&harness, &credential, replay).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let (status, after_replay) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after_replay}");
    let replayed = after_replay["entries"].as_array().unwrap();
    assert_eq!(
        replayed.len(),
        1,
        "a replay opens no second state: {after_replay}"
    );
    assert_eq!(
        replayed[0]["receivedAt"], delivered_at,
        "a replay never restamps the row it repeats: {after_replay}"
    );
    assert_eq!(after_replay["series"]["replayedCount"], Value::from(1));

    // The same instant once more, with a state that disagrees with the row the
    // log holds: the row is corrected in place, and the correction is booked as
    // a correction rather than as a new change.
    let correction = state_report(
        &agent_id,
        3,
        &minute(2),
        sync_ok(&observed, true, 250, 250),
        consensus_ok(&observed, 190, 188, 186),
    );
    let (status, value) = submit(&harness, &credential, correction).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let (status, corrected) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{corrected}");
    let entries = corrected["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "a correction invents no instant: {corrected}"
    );
    assert_eq!(entries[0]["entryKind"], "change");
    assert_eq!(entries[0]["syncing"], Value::Bool(true), "{corrected}");
    assert_eq!(entries[0]["valueSource"], "current");
    assert_ne!(
        entries[0]["receivedAt"], delivered_at,
        "the correcting delivery is the one the row is stamped with: {corrected}"
    );
    assert_eq!(corrected["series"]["replayedCount"], Value::from(1));
    assert_eq!(corrected["series"]["correctedCount"], Value::from(1));
    assert_eq!(
        corrected["series"]["entryCount"],
        Value::from(1),
        "a replay and a correction count no new evidence: {corrected}"
    );
    assert_eq!(corrected["series"]["changeCount"], Value::from(1));
    assert_eq!(corrected["series"]["latestSyncing"], Value::Bool(true));
    assert_eq!(
        harness
            .count_where("node_state_observations", "component = 'sync'")
            .await,
        1,
        "one instant stays one row"
    );
}

/// Story 59: a low-space pause is a visible protection gap with its counted
/// losses, so a paused stretch is never recorded as a state, drawn as a change,
/// or counted as proved coverage.
#[tokio::test]
async fn a_low_space_pause_is_a_protection_gap_with_its_counted_losses() {
    let mut harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::minutes(90);
    let minute = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));
    let mut sequence = 0_u64;

    // A regularly observed state first: three Reports of the same state write
    // one change row and nothing else.
    for minutes in [0_i64, 5, 10] {
        sequence += 1;
        let at = minute(minutes);
        let report = state_report(
            &agent_id,
            sequence,
            &at,
            sync_ok(&at, false, 100, 100),
            consensus_ok(&at, 90, 88, 86),
        );
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    // Low-space protection pauses state recording: the Reports are still
    // accepted, the states are not recorded, and the losses are counted.
    let pressure = forced_policy(&harness);
    pressure.reconcile(harness.pool()).await.unwrap();
    assert!(
        pressure.status().protected,
        "the declared floor is above any real free space"
    );
    harness.install_capacity(Arc::clone(&pressure));
    for minutes in [15_i64, 20] {
        sequence += 1;
        let at = minute(minutes);
        let report = state_report(
            &agent_id,
            sequence,
            &at,
            sync_ok(&at, false, 150, 150),
            consensus_ok(&at, 140, 138, 136),
        );
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "pressure never refuses a Report: {value}"
        );
    }
    assert_eq!(
        harness
            .count_where("node_state_observations", "component = 'sync'")
            .await,
        1,
        "a paused state is not recorded"
    );
    // The skipped deliveries are counted on the shared skipped-series ledger,
    // keyed by the component instead of a metric name.
    assert_eq!(
        harness
            .count_where(
                "capacity_skipped_series",
                "scope_kind = 'node' AND metric = 'sync' AND skipped_count = 2"
            )
            .await,
        1,
        "the pause counts the losses it caused"
    );

    // Release the floor below the measured free space and let recording resume.
    // The resumed state differs, because an unchanged one would wait for its
    // hourly anchor instead of writing a row.
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
        sequence += 1;
        let at = minute(minutes);
        let report = state_report(
            &agent_id,
            sequence,
            &at,
            sync_ok(&at, true, 160, 200),
            consensus_ok(&at, 150, 148, 146),
        );
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let from = minute(-5);
    let to = minute(70);
    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        2,
        "the paused stretch records nothing: {body}"
    );
    assert_eq!(entries[0]["observedAt"], Value::String(minute(0)));
    assert_eq!(entries[0]["syncing"], Value::Bool(false));
    assert_eq!(entries[1]["observedAt"], Value::String(minute(27)));
    assert_eq!(entries[1]["syncing"], Value::Bool(true));
    let gaps = body["gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{body}");
    assert_eq!(gaps[0]["kind"], "protection_pause");
    assert_eq!(
        gaps[0]["reason"],
        "low-space protection paused state recording"
    );
    assert_eq!(gaps[0]["from"], Value::String(minute(0)));
    assert_eq!(gaps[0]["to"], Value::String(minute(27)));
    assert_eq!(gaps[0]["seconds"], Value::from(1_620));
    assert_eq!(
        gaps[0]["skippedCount"],
        Value::from(2),
        "the gap carries the losses the pause counted"
    );
    assert_eq!(
        body["coverageSeconds"],
        Value::from(0),
        "a paused stretch is never proved coverage: {body}"
    );
    assert_eq!(body["series"]["entryCount"], Value::from(5));
    assert_eq!(body["series"]["changeCount"], Value::from(2));
    assert_eq!(body["series"]["anchorCount"], Value::from(0));
    assert_eq!(body["series"]["latestSyncing"], Value::Bool(true));
    assert!(body["availability"].is_null());

    // An older page clips what it reports to where it says it stops. The pause
    // reaches past this page's cursor, so the gap it reports must end at the
    // advertised end of the page instead of running on into a stretch this page
    // never served.
    let response = harness
        .send(admin_get(
            &state_page_uri(NODE_ID, "sync", &from, &to, Some(&minute(15)), 2),
            Some(&session.cookie),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let older = body_json(response).await;
    assert_eq!(older["to"], Value::String(minute(15)), "{older}");
    let older_gaps = older["gaps"].as_array().unwrap();
    assert_eq!(older_gaps.len(), 1, "{older}");
    assert_eq!(older_gaps[0]["kind"], "protection_pause");
    assert_eq!(older_gaps[0]["from"], Value::String(minute(0)), "{older}");
    assert_eq!(
        older_gaps[0]["to"],
        Value::String(minute(15)),
        "the page clips the pause to the end it advertises: {older}"
    );
    for gap in older_gaps {
        assert!(
            gap["to"].as_str().unwrap() <= older["to"].as_str().unwrap(),
            "no gap runs past the page's own end: {older}"
        );
        assert!(
            gap["from"].as_str().unwrap() >= older["from"].as_str().unwrap(),
            "no gap starts before the page's own start: {older}"
        );
    }
}

/// Story 53: the recorded states are rows on disk, so a restarted Server serves
/// the very same answer and re-derives no state from the Reports afterwards.
#[tokio::test]
async fn a_restarted_server_serves_the_same_recorded_states() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let minute = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));
    for (sequence, at, syncing) in [
        (1_u64, minute(0), false),
        (2, minute(30), true),
        (3, minute(61), false),
    ] {
        let report = state_report(
            &agent_id,
            sequence,
            &at,
            sync_ok(&at, syncing, 100 + sequence, 200),
            consensus_ok(&at, 90, 88, 86),
        );
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let from = minute(-5);
    let to = minute(120);
    let (status, before) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{before}");
    assert_eq!(before["entries"].as_array().unwrap().len(), 3, "{before}");

    // A real restart: the router, the pool and the policy state all go away, and
    // the next Server opens the same durable directory.
    let harness = harness.restart().await;

    let (status, after) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(
        after["entries"], before["entries"],
        "the recorded states are rows on disk: the restarted Server serves what the last one served"
    );
    assert_eq!(after["series"], before["series"]);
    assert_eq!(after["gaps"], before["gaps"]);
    assert_eq!(after["coverageSeconds"], before["coverageSeconds"]);
    assert_eq!(
        harness
            .count_where("node_state_observations", "component = 'sync'")
            .await,
        3,
        "opening the database re-derives no state"
    );
}

/// Story 53: the key consensus heights are numbers, so they ride the metric
/// surface as series of their own, and a reader never has to infer them from
/// Block history or from a state change.
#[tokio::test]
async fn the_key_heights_ride_the_metric_surface_by_height() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let minute = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));

    // Two Reports whose heights move while the state does not: the numbers
    // belong to the metric axis, the state evidence stays where it was.
    let reports = [
        state_report(
            &agent_id,
            1,
            &minute(0),
            sync_ok(&minute(0), false, 1_000, 2_000),
            consensus_ok(&minute(0), 920, 910, 900),
        ),
        state_report(
            &agent_id,
            2,
            &minute(30),
            sync_ok(&minute(30), false, 1_500, 2_500),
            consensus_ok(&minute(30), 1_420, 1_410, 1_400),
        ),
    ];
    for report in reports {
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    for (metric, expected) in [
        ("sync_current_block", [1_000.0, 1_500.0]),
        ("sync_highest_block", [2_000.0, 2_500.0]),
        ("consensus_highest_qc_block", [920.0, 1_420.0]),
        ("consensus_highest_lock_block", [910.0, 1_410.0]),
        ("consensus_highest_commit_block", [900.0, 1_400.0]),
    ] {
        let uri = format!("/api/admin/v1/nodes/{NODE_ID}/metric-history?metric={metric}");
        let response = harness.send(admin_get(&uri, Some(&session.cookie))).await;
        assert_eq!(response.status(), StatusCode::OK, "{metric}");
        let body = body_json(response).await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2, "{metric} stored its heights: {body}");
        for (item, expected) in items.iter().zip(expected) {
            assert_eq!(item["value"], Value::from(expected), "{metric}: {body}");
        }
    }

    // The moving heights never wrote a second state: the state vector did not
    // change, so the log holds the one change it recorded.
    let from = minute(-5);
    let to = minute(120);
    let (status, state) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(
        state["entries"].as_array().unwrap().len(),
        1,
        "a height is not a state change: {state}"
    );

    // A failed probe stores no height at all: the failure belongs to the state
    // log, and the series is not filled in with a value nobody observed.
    let failed = state_report(
        &agent_id,
        3,
        &minute(60),
        sync_failed(&minute(60), "rpc_unavailable"),
        consensus_ok(&minute(60), 1_420, 1_410, 1_400),
    );
    let (status, value) = submit(&harness, &credential, failed).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        harness
            .count_where("node_metric_samples", "metric = 'sync_current_block'")
            .await,
        2,
        "a failed probe stores no height"
    );
    let (status, failed_state) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{failed_state}");
    let entries = failed_state["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{failed_state}");
    assert_eq!(entries[1]["collectionState"], "error");
    assert_eq!(entries[1]["valueSource"], "last_good");
    assert_eq!(
        entries[1]["valueObservedAt"],
        Value::String(minute(30)),
        "the retained value is the last successful observation: {failed_state}"
    );
}

/// Story 53: a bounded answer pages older without a hole, and the cursor it
/// returns is the coordinate the next page continues from.
#[tokio::test]
async fn a_bounded_answer_pages_older_without_a_hole() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let minute = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));

    // Five recorded changes, ten minutes apart, so a bounded answer has to cut
    // the sequence in the middle.
    for (sequence, minutes) in [(1_u64, 0_i64), (2, 10), (3, 20), (4, 30), (5, 40)] {
        let at = minute(minutes);
        let report = state_report(
            &agent_id,
            sequence,
            &at,
            sync_ok(&at, minutes % 20 == 0, 100 + minutes as u64, 200),
            consensus_ok(&at, 90, 88, 86),
        );
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let from = minute(-5);
    let to = minute(60);
    let (status, unpaged) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unpaged}");
    let unbound = unpaged["entries"].as_array().unwrap();
    assert_eq!(unbound.len(), 5, "{unpaged}");
    assert_eq!(unpaged["truncated"], Value::Bool(false));
    assert!(unpaged["continuation"].is_null());

    let mut pages: Vec<Vec<Value>> = Vec::new();
    let mut cursor: Option<String> = None;
    for page in 0..3 {
        let uri = state_page_uri(NODE_ID, "sync", &from, &to, cursor.as_deref(), 2);
        let response = harness.send(admin_get(&uri, Some(&session.cookie))).await;
        assert_eq!(response.status(), StatusCode::OK, "page {page}");
        let body = body_json(response).await;
        let entries = body["entries"].as_array().unwrap();
        if page < 2 {
            assert_eq!(entries.len(), 2, "page {page}: {body}");
            assert_eq!(body["truncated"], Value::Bool(true), "page {page}: {body}");
            cursor = Some(body["continuation"].as_str().unwrap().to_owned());
        } else {
            assert_eq!(entries.len(), 1, "page {page}: {body}");
            assert_eq!(body["truncated"], Value::Bool(false), "page {page}: {body}");
            assert!(body["continuation"].is_null(), "page {page}: {body}");
        }
        pages.push(entries.to_vec());
    }

    // The pages add up to the whole sequence, in order, with no instant read
    // twice and none missing between the cursor coordinates.
    let read: Vec<Value> = pages.iter().rev().flatten().cloned().collect();
    assert_eq!(
        Value::Array(read),
        Value::Array(unbound.clone()),
        "paging is seamless: {unpaged}"
    );
    let instants: Vec<Value> = unbound
        .iter()
        .map(|entry| entry["observedAt"].clone())
        .collect();
    assert_eq!(
        instants,
        vec![
            Value::String(minute(0)),
            Value::String(minute(10)),
            Value::String(minute(20)),
            Value::String(minute(30)),
            Value::String(minute(40)),
        ]
    );

    // A cursor outside the requested range answers a range nobody asked about,
    // so it is refused instead of silently clamped.
    for cursor in [minute(-60), minute(90)] {
        let response = harness
            .send(admin_get(
                &state_page_uri(NODE_ID, "sync", &from, &to, Some(&cursor), 2),
                Some(&session.cookie),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{cursor}");
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_history_range");
    }
}

/// Story 53: a delivery that was counted without writing a row is still evidence
/// the log has seen. Repeating it is a replay, and naming a different state for
/// that same instant corrects the record of it instead of inventing a transition
/// the regular answer never reported.
#[tokio::test]
async fn a_repeat_at_a_counted_instant_is_a_replay_and_a_changed_one_corrects_it() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let minute = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));
    let from = minute(-5);
    let to = minute(120);

    // One change row, then a delivery half an hour later carrying the very same
    // vector: it is counted as evidence and writes nothing, because an unchanged
    // state waits for its hourly anchor.
    for (sequence, at) in [(1_u64, minute(0)), (2, minute(30))] {
        let report = state_report(
            &agent_id,
            sequence,
            &at,
            sync_ok(&at, false, 100, 100),
            consensus_ok(&at, 90, 88, 86),
        );
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let (status, counted) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{counted}");
    assert_eq!(
        counted["entries"].as_array().unwrap().len(),
        1,
        "an unchanged state waits for its anchor: {counted}"
    );
    assert_eq!(counted["series"]["entryCount"], Value::from(2), "{counted}");
    assert_eq!(
        counted["series"]["replayedCount"],
        Value::from(0),
        "the second delivery of a different instant is not a replay: {counted}"
    );

    // That same delivery again: the log already counted the instant, so the repeat
    // adds no evidence at all and no row is written.
    let repeat = state_report(
        &agent_id,
        3,
        &minute(31),
        sync_ok(&minute(30), false, 100, 100),
        consensus_ok(&minute(30), 90, 88, 86),
    );
    let (status, value) = submit(&harness, &credential, repeat).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let (status, replayed) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(
        replayed["series"]["replayedCount"],
        Value::from(1),
        "{replayed}"
    );
    assert_eq!(
        replayed["series"]["entryCount"],
        Value::from(2),
        "a repeat repeats evidence instead of adding any: {replayed}"
    );
    assert_eq!(
        harness
            .count_where("node_state_observations", "component = 'sync'")
            .await,
        1,
        "a replay writes no row"
    );

    // The same counted instant once more, with a state that disagrees with the row
    // the log holds: the log corrects what it recorded for that instant rather than
    // counting a change the regular cadence never saw.
    let correction = state_report(
        &agent_id,
        4,
        &minute(32),
        sync_ok(&minute(30), true, 100, 100),
        consensus_ok(&minute(30), 90, 88, 86),
    );
    let (status, value) = submit(&harness, &credential, correction).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let (status, corrected) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{corrected}");
    let entries = corrected["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        2,
        "a correction writes the instant it names: {corrected}"
    );
    assert_eq!(entries[0]["observedAt"], Value::String(minute(0)));
    assert_eq!(entries[0]["syncing"], Value::Bool(false));
    assert_eq!(
        entries[1]["observedAt"],
        Value::String(minute(30)),
        "the corrected instant is the one the delivery named: {corrected}"
    );
    assert_eq!(entries[1]["entryKind"], "change");
    assert_eq!(entries[1]["syncing"], Value::Bool(true), "{corrected}");
    assert_eq!(
        corrected["series"]["entryCount"],
        Value::from(2),
        "{corrected}"
    );
    // The corrected row is a change of the recorded state, but the ledger counts a
    // change only where a counted delivery stated one, so the correction is booked
    // as a correction and the change total stays where it was.
    assert_eq!(
        corrected["series"]["changeCount"],
        Value::from(1),
        "{corrected}"
    );
    assert_eq!(
        corrected["series"]["anchorCount"],
        Value::from(0),
        "{corrected}"
    );
    assert_eq!(corrected["series"]["replayedCount"], Value::from(1));
    assert_eq!(corrected["series"]["correctedCount"], Value::from(1));
    assert_eq!(
        harness
            .count_where("node_state_observations", "component = 'sync'")
            .await,
        2,
        "the correction replaces a row instead of opening a third"
    );
}

/// Story 53: a late Report names an instant older than the newest one the log
/// counted. The transition it carries is still evidence, so the log records it
/// where it belongs and keeps the stretch it explains readable in order.
#[tokio::test]
async fn a_late_delivery_of_an_older_instant_states_the_transition_it_evidences() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let minute = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));
    let from = minute(-5);
    let to = minute(120);

    // A failed probe, then a success an hour later, then the success the middle of
    // that hour really was: the late Report explains the stretch the regular answer
    // never saw, and it must not be read as a second hour of silence.
    for (sequence, report_at, sync) in [
        (1_u64, minute(0), sync_failed(&minute(0), "rpc_unavailable")),
        (2, minute(60), sync_ok(&minute(60), false, 100, 100)),
        (3, minute(61), sync_ok(&minute(30), false, 100, 100)),
    ] {
        let report = state_report(
            &agent_id,
            sequence,
            &report_at,
            sync,
            consensus_ok(&report_at, 90, 88, 86),
        );
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        3,
        "the late instant is recorded where it belongs: {body}"
    );
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry["observedAt"].clone())
            .collect::<Vec<Value>>(),
        vec![
            Value::String(minute(0)),
            Value::String(minute(30)),
            Value::String(minute(60)),
        ],
        "the recorded order is the order of the instants, not the order of arrival: {body}"
    );
    assert_eq!(entries[0]["collectionState"], "error");
    assert_eq!(entries[1]["collectionState"], "ok");
    assert_eq!(
        entries[1]["entryKind"], "change",
        "the late delivery states the transition it evidences: {body}"
    );
    assert_eq!(entries[2]["collectionState"], "ok");
    assert_eq!(body["series"]["entryCount"], Value::from(3), "{body}");
    assert_eq!(body["series"]["changeCount"], Value::from(3), "{body}");
    assert_eq!(body["series"]["replayedCount"], Value::from(0), "{body}");
    assert_eq!(body["series"]["correctedCount"], Value::from(0), "{body}");
    assert!(
        body["gaps"].as_array().unwrap().is_empty(),
        "an explained stretch is not a silence: {body}"
    );
    assert_eq!(
        harness
            .count_where("node_state_observations", "component = 'sync'")
            .await,
        3,
        "one row for each instant the log holds"
    );
}

/// Story 53: an unchanged state is re-stated once the anchor is due, so the
/// delivery that states it can arrive one cadence after that instant and two
/// rows of one component can sit that far apart with nothing having been lost.
/// The answer reports a gap only where the record really proves one.
#[tokio::test]
async fn an_unchanged_state_spanning_a_slow_cadence_is_not_reported_as_silence() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(5);
    let second = |seconds: i64| auth::format_rfc3339(base + time::Duration::seconds(seconds));

    // Reports 59 minutes apart with a state that never changes, which is slower
    // than the fastest cadence an Agent may be configured with: the first instant
    // writes a row, the delivery inside the anchor window writes none, and the one
    // after it states the anchor an hour and a half later.
    for (sequence, at) in [(1_u64, 0_i64), (2, 3_540), (3, 7_080)] {
        let report = state_report(
            &agent_id,
            sequence,
            &second(at + 5),
            sync_ok(&second(at), false, 100, 100),
            consensus_ok(&second(at), 90, 88, 86),
        );
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let from = second(-60);
    let to = second(9_000);
    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        2,
        "an unchanged state is not repeated: {body}"
    );
    assert_eq!(entries[0]["entryKind"], "change");
    assert_eq!(entries[1]["observedAt"], Value::String(second(7_080)));
    assert_eq!(entries[1]["entryKind"], "anchor", "{body}");
    assert_eq!(body["cadenceSeconds"], Value::from(3_540), "{body}");
    // The three deliveries were all counted, and every one of them is evidence
    // for the stretch they span: the anchor window is the anchor interval plus the
    // cadence this series really showed, so 59 minutes between two rows is not a
    // gap and never reported as one.
    assert_eq!(body["series"]["entryCount"], Value::from(3), "{body}");
    assert_eq!(body["series"]["anchorCount"], Value::from(1), "{body}");
    assert!(
        body["gaps"].as_array().unwrap().is_empty(),
        "the ordinary distance between two anchors is not silence: {body}"
    );
    assert_eq!(body["coverageSeconds"], Value::from(7_080), "{body}");
    assert_eq!(body["anchorSeconds"], Value::from(3_600));
}

/// Story 53: a late Report can insert a row behind the newest evidence the series
/// counted. The state the ledger already holds at that newer instant is evidence
/// the Server really counted, so the log keeps it as a row of its own right then:
/// waiting for a later delivery to state it would leave the return to that state
/// out of the history, and would date it at an instant it was not proved at. A
/// delivery that follows is judged against the row before it.
#[tokio::test]
async fn a_late_insert_keeps_the_newest_counted_state_on_the_record() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(3);
    let minute = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));
    let from = minute(-5);
    let to = minute(120);

    // The regular answer never changes, so minute 0 writes a row and minute 40
    // writes none; the late Report then states the transition the log had not seen,
    // and the newest state the ledger counted is kept on the record as a row.
    for (sequence, report_at, sync) in [
        (1_u64, minute(0), sync_ok(&minute(0), false, 100, 100)),
        (2, minute(40), sync_ok(&minute(40), false, 100, 100)),
        (3, minute(41), sync_ok(&minute(30), true, 100, 100)),
    ] {
        let report = state_report(
            &agent_id,
            sequence,
            &report_at,
            sync,
            consensus_ok(&report_at, 90, 88, 86),
        );
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry["observedAt"].clone())
            .collect::<Vec<Value>>(),
        vec![
            Value::String(minute(0)),
            Value::String(minute(30)),
            Value::String(minute(40)),
        ],
        "the newest counted state is recorded without another Report: {body}"
    );
    assert_eq!(entries[0]["syncing"], Value::Bool(false));
    assert_eq!(entries[0]["entryKind"], "change");
    assert_eq!(entries[1]["syncing"], Value::Bool(true), "{body}");
    assert_eq!(entries[2]["syncing"], Value::Bool(false), "{body}");
    assert_eq!(
        entries[2]["entryKind"], "change",
        "the return to the counted state is a change of the recorded state: {body}"
    );
    assert_eq!(body["series"]["entryCount"], Value::from(3), "{body}");
    assert_eq!(
        body["series"]["changeCount"],
        Value::from(2),
        "the repair row is not a delivery: {body}"
    );
    assert_eq!(body["series"]["replayedCount"], Value::from(0), "{body}");
    assert_eq!(body["series"]["correctedCount"], Value::from(0), "{body}");
    assert_eq!(
        body["series"]["latestSyncing"],
        Value::Bool(false),
        "{body}"
    );
    assert_eq!(
        harness
            .count_where("node_state_observations", "component = 'sync'")
            .await,
        3,
        "the newest counted state is on the record: {body}"
    );

    // A Report that then restates the same state adds nothing: the log already
    // ends at the state the series holds, so no row is invented for it and the
    // recorded timeline still ends where the evidence does.
    let report = state_report(
        &agent_id,
        4,
        &minute(50),
        sync_ok(&minute(50), false, 100, 100),
        consensus_ok(&minute(50), 90, 88, 86),
    );
    let (status, value) = submit(&harness, &credential, report).await;
    assert_eq!(status, StatusCode::OK, "{value}");

    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["observedAt"].clone())
            .collect::<Vec<Value>>(),
        vec![
            Value::String(minute(0)),
            Value::String(minute(30)),
            Value::String(minute(40)),
        ],
        "an unchanged delivery writes no row of its own: {body}"
    );
    assert_eq!(body["series"]["entryCount"], Value::from(4), "{body}");
    assert_eq!(
        harness
            .count_where("node_state_observations", "component = 'sync'")
            .await,
        3,
        "the row count does not move either: {body}"
    );
}

/// Story 59: an unchanged state is anchored hourly, so a Node that comes back
/// after hours of silence states the gap the record proves and never bridges it
/// with a constant state.
#[tokio::test]
async fn a_delivery_after_hours_of_silence_reports_the_gap_it_proves() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(8);
    let minute = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));
    let from = minute(-5);
    let to = minute(300);

    // Six identical deliveries a minute apart: one change row, five counted
    // deliveries that wrote nothing. Four hours later the very same state arrives
    // again, past its anchor window, so it is recorded as the anchor that proves
    // the Node was reporting then.
    for sequence in 1..=7_u64 {
        let at = if sequence == 7 {
            minute(240)
        } else {
            minute(sequence as i64 - 1)
        };
        let report = state_report(
            &agent_id,
            sequence,
            &at,
            sync_ok(&at, false, 100, 100),
            consensus_ok(&at, 90, 88, 86),
        );
        let (status, value) = submit(&harness, &credential, report).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }

    let (status, body) = state_history(
        &harness,
        Some(&session.cookie),
        NODE_ID,
        "sync",
        Some(&from),
        Some(&to),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        2,
        "only the change and the anchor are rows: {body}"
    );
    assert_eq!(entries[0]["observedAt"], Value::String(minute(0)));
    assert_eq!(entries[0]["entryKind"], "change");
    assert_eq!(entries[1]["observedAt"], Value::String(minute(240)));
    assert_eq!(entries[1]["entryKind"], "anchor", "{body}");
    assert_eq!(body["series"]["entryCount"], Value::from(7), "{body}");
    assert_eq!(body["series"]["changeCount"], Value::from(1), "{body}");
    assert_eq!(body["series"]["anchorCount"], Value::from(1), "{body}");
    assert_eq!(body["cadenceSeconds"], Value::from(2_400), "{body}");
    let gaps = body["gaps"].as_array().unwrap();
    assert_eq!(
        gaps.len(),
        1,
        "one gap, at the one silence the log proves: {body}"
    );
    assert_eq!(gaps[0]["kind"], "collection_gap");
    assert_eq!(gaps[0]["from"], Value::String(minute(0)));
    assert_eq!(gaps[0]["to"], Value::String(minute(240)));
    assert_eq!(gaps[0]["seconds"], Value::from(14_400));
    assert!(gaps[0]["skippedCount"].is_null(), "{body}");
    assert_eq!(
        gaps[0]["reason"], "no state was recorded in this stretch",
        "the reason claims the record, not the Node: {body}"
    );
    assert_eq!(
        body["coverageSeconds"],
        Value::from(0),
        "a silence is never proved coverage: {body}"
    );
}
