//! Spool Backlog Age at the HTTP boundary (ADR 0014, issue #272).
//!
//! The unit tests in `crate::spool_backlog` and `crate::doctor` drive the
//! derivation and the Doctor reducer directly. This suite drives the same seams
//! the way an Agent, an Operator, and the Server Doctor do: it builds the full
//! application with `build_app` against a temporary SQLite database, enrolls
//! real Agents through the Admin and Agent HTTP APIs, submits real Reports, and
//! reads the derived reading back from the Owner-only Admin routes and from a
//! real Doctor run.
//!
//! It proves the acceptance criteria of the issue at the surface an Operator
//! uses:
//!
//! 1. Replaying a two-hour-old Report shows roughly two hours, not the stale age
//!    the immutable Report carries, and the same reading reaches the Node view.
//! 2. An unreliable measured clock falls back to the Report's own reading, and a
//!    Report that carries no Spool observation clears the reading instead of
//!    leaving an older number in place.
//! 3. Server Doctor warns above five minutes and passes below it.
//!
//! Every test owns its own TempDir and database, so nothing is shared with the
//! parallel unit tests.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tempfile::TempDir;
use time::Duration;
use tower::ServiceExt;

use platpulse_server::{AppState, auth, http, network, operations, secrets};

/// Registered Network tuple for the `platon-mainnet` key the report fixture
/// declares, matching the unit-test registry exactly.
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
/// The Node the report fixture declares.
const NODE_A: &str = "0195f2a1-0014-4014-8014-000000000014";
/// Distinct Report identities, so each submission is a new Report instead of an
/// idempotent replay of the previous one.
const REPORT_IDS: [&str; 6] = [
    "0195f2a1-0201-4201-8201-000000000201",
    "0195f2a1-0202-4202-8202-000000000202",
    "0195f2a1-0203-4203-8203-000000000203",
    "0195f2a1-0204-4204-8204-000000000204",
    "0195f2a1-0205-4205-8205-000000000205",
    "0195f2a1-0206-4206-8206-000000000206",
];

/// A fresh Server (real temp SQLite + pepper), the full router, and the pieces
/// needed to authenticate as the Owner.
struct Harness {
    _dir: TempDir,
    state: AppState,
    app: Router,
}

impl Harness {
    async fn boot() -> Self {
        let dir = TempDir::new().unwrap();
        let database = platpulse_server::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("server-pepper");
        secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = auth::AuthConfig::development(
            secrets::load_pepper_file(&pepper_path).unwrap(),
            DEVELOPMENT_ORIGIN.to_owned(),
        );
        let state = AppState::new(database, None, auth);
        // The Agent group refuses every request until the first Owner exists
        // (design §13.3), so the acceptance path must start here.
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

/// An authenticated Owner session: the cookie every Admin read presents and the
/// CSRF token every Admin mutation must present with it.
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

/// Log in through the real login route and return the authenticated Owner
/// session the Admin routes require.
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
    let token = body_json(response).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let request = Request::builder()
        .method("POST")
        .uri("/api/agent/v1/enroll")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    (
        body["agent_id"].as_str().unwrap().to_owned(),
        body["credential"].as_str().unwrap().to_owned(),
    )
}

/// Submit one Report over the real Agent route and return the receipt.
async fn submit(harness: &Harness, credential: &str, report: &Value) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri("/api/agent/v1/reports")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {credential}"))
        .body(Body::from(serde_json::to_vec(report).unwrap()))
        .unwrap();
    let response = harness.send(request).await;
    let status = response.status();
    let body = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// The minimal wire fixture, re-bound to the enrolled Agent identity and moved
/// to the generation instant the scenario needs.
///
/// The patch goes through the JSON the wire actually carries instead of a Rust
/// literal, so each case states exactly the fields it changes and everything the
/// scenario does not care about stays as the fixture declares it.
fn report_json(agent_id: &str, sequence: u64, report_id: &str, generated_at: &str) -> Value {
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    value["agent_id"] = Value::String(agent_id.to_owned());
    value["report_sequence"] = Value::from(sequence);
    value["report_id"] = Value::String(report_id.to_owned());
    value["generated_at"] = Value::String(generated_at.to_owned());
    value
}

/// An Agent wall-clock instant [seconds_ago] seconds before now, which is what
/// `generated_at` and the component timestamps are stamped with.
fn generated(seconds_ago: i64) -> String {
    auth::format_rfc3339(auth::now_utc() - Duration::seconds(seconds_ago))
}

/// The Spool observation the Report carries: the backlog the Agent measured at
/// generation time.
fn with_spool(mut report: Value, oldest_queued_age_ms: u64) -> Value {
    report["host"]["spool"]["latest"]["oldest_queued_age_ms"] = Value::from(oldest_queued_age_ms);
    report
}

/// The same Report with no Spool observation at all: the component is disabled,
/// which is never a measured zero backlog.
fn without_spool(mut report: Value) -> Value {
    // Revisions belong to the component, so only the value goes away.
    report["host"]["spool"] = serde_json::json!({
        "status": "disabled",
        "state_revision": 1,
        "value_revision": 0,
    });
    report
}

/// The Agent→Server clock skew the Report measured, in signed milliseconds
/// (positive = Agent clock ahead).
fn with_clock_skew(mut report: Value, skew_ms: i64) -> Value {
    report["host"]["clock_skew"]["latest"] = Value::from(skew_ms);
    report
}

/// Read one Agent's Owner-only diagnostic.
async fn agent_detail(harness: &Harness, session: &Session, agent_id: &str) -> Value {
    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/agents/{agent_id}"),
            session,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

/// Start one Doctor run, drive the real worker until the queue drains, and
/// return the checks the run stored.
async fn doctor_checks(harness: &Harness, session: &Session) -> Vec<Value> {
    let response = harness
        .send(admin_post("/api/admin/v1/doctor", session, ""))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    // The mutation names the Operation it queued, so the checks are read back
    // from this exact run. The overview picks the newest run by a
    // second-precision timestamp, which two runs of one test can share.
    let body = body_json(response).await;
    let operation_id = body["operation"]["operation"]["operationId"]
        .as_str()
        .unwrap_or_else(|| panic!("the Doctor mutation must name its Operation: {body}"))
        .to_owned();
    while operations::process_operations(&harness.state)
        .await
        .unwrap()
        > 0
    {}
    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/operations/{operation_id}"),
            session,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await["result"]["checks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The Spool Backlog Age check out of a Doctor run.
fn spool_backlog_check(checks: &[Value]) -> Value {
    checks
        .iter()
        .find(|check| check["checkId"] == "agent_spool_backlog")
        .unwrap_or_else(|| {
            panic!("the Doctor run must include the spool backlog check: {checks:?}")
        })
        .clone()
}

/// Acceptance 1: a Report that left the Agent two hours ago reports the two
/// hours it actually waited, not the empty Spool it measured back then, and the
/// Node view of the Agent's Node carries the same Agent-scoped reading.
#[tokio::test]
async fn a_replayed_two_hour_old_report_shows_its_real_age_not_the_stale_reading() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;

    let report = with_spool(
        with_clock_skew(
            report_json(&agent_id, 1, REPORT_IDS[0], &generated(2 * 60 * 60)),
            -4,
        ),
        0,
    );
    let receipt = submit(&harness, &credential, &report).await;
    assert_eq!(receipt["receipt"]["disposition"], "accepted", "{receipt}");

    let detail = agent_detail(&harness, &session, &agent_id).await;
    let reading = &detail["host"]["spool_backlog_age"];
    assert_eq!(reading["basis"], "receipt", "{detail}");
    let age_ms = reading["age_ms"].as_i64().unwrap();
    assert!(
        (7_195_000..7_205_000).contains(&age_ms),
        "a two-hour-old replay must show roughly two hours, got {age_ms}: {detail}"
    );
    // The Report's own number is still stored exactly as the Agent measured it;
    // it is simply not the reading the Operator is shown.
    assert_eq!(detail["host"]["spool_oldest_queued_age_ms"], 0);

    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/nodes/{NODE_A}"),
            &session,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let node = body_json(response).await;
    assert_eq!(
        node["agent_spool_backlog_age"]["basis"], "receipt",
        "{node}"
    );
    assert_eq!(
        node["agent_spool_backlog_age"]["age_ms"].as_i64().unwrap(),
        age_ms,
        "the Node view shows the owning Agent's per-Agent reading: {node}"
    );
}

/// Acceptance 2: when the measured clock skew is past what the Server trusts,
/// the reading falls back to the Report's own measurement — labelled as such —
/// and a later Report that carries no Spool observation clears the reading
/// instead of leaving the older number in place.
#[tokio::test]
async fn an_unreliable_clock_falls_back_to_the_reports_own_reading() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;

    // A quarter of an hour of skew is far past the trust threshold, so the
    // receipt-based derivation is not available. The Report's own age is still
    // exact: both of its ends are Agent-clock instants, so the offset cancels.
    let report = with_spool(
        with_clock_skew(
            report_json(&agent_id, 1, REPORT_IDS[1], &generated(30)),
            15 * 60 * 1000,
        ),
        15 * 60 * 1000,
    );
    submit(&harness, &credential, &report).await;

    let detail = agent_detail(&harness, &session, &agent_id).await;
    let reading = &detail["host"]["spool_backlog_age"];
    assert_eq!(reading["basis"], "generation", "{detail}");
    assert_eq!(reading["age_ms"], 15 * 60 * 1000, "{detail}");

    // Server Doctor reads the same fallback, so it has to label it too: the
    // Operator must not read a generation-time figure as the wait happening now.
    let check = spool_backlog_check(&doctor_checks(&harness, &session).await);
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .contains("(as of report generation)"),
        "{check}"
    );

    // The next Report says nothing about the Durable Spool. The last-good Spool
    // numbers stay, but the derived reading is unknown again rather than
    // frozen at an age nobody measured at this receipt.
    let report = without_spool(report_json(&agent_id, 2, REPORT_IDS[2], &generated(0)));
    submit(&harness, &credential, &report).await;

    let detail = agent_detail(&harness, &session, &agent_id).await;
    assert!(
        detail["host"]["spool_backlog_age"].is_null(),
        "a Report without a Spool observation must clear the reading: {detail}"
    );
    assert_eq!(detail["host"]["spool_oldest_queued_age_ms"], 15 * 60 * 1000);
}

/// Acceptance 3: Server Doctor names the Agent that has been waiting longer than
/// five minutes and passes once it catches up — a warning, never a failure.
#[tokio::test]
async fn doctor_warns_above_five_minutes_and_passes_below_it() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (slow_agent, slow_credential) = enroll_agent(&harness, &session).await;
    let (quick_agent, quick_credential) = enroll_agent(&harness, &session).await;

    // One Agent has been waiting six minutes for delivery, the other one
    // seconds. Both reported a Spool observation and both are online.
    let slow = with_spool(
        with_clock_skew(
            report_json(&slow_agent, 1, REPORT_IDS[3], &generated(6 * 60)),
            0,
        ),
        6 * 60 * 1000,
    );
    submit(&harness, &slow_credential, &slow).await;
    let quick = with_spool(
        with_clock_skew(
            report_json(&quick_agent, 1, REPORT_IDS[4], &generated(0)),
            0,
        ),
        5_000,
    );
    submit(&harness, &quick_credential, &quick).await;

    let check = spool_backlog_check(&doctor_checks(&harness, &session).await);
    assert_eq!(check["status"], "warning", "{check}");
    let detail = check["detail"].as_str().unwrap();
    assert!(detail.contains("1 of 2"), "{check}");
    // The Operator needs to know which Agent to look at, and the Agent has no
    // display name yet, so the check names it by its id.
    assert!(detail.contains(&slow_agent), "{check}");
    assert!(detail.contains("6m"), "{check}");

    // Once the slow Agent catches up, nothing is waiting and the check passes.
    let caught_up = with_spool(
        with_clock_skew(report_json(&slow_agent, 2, REPORT_IDS[5], &generated(0)), 0),
        0,
    );
    submit(&harness, &slow_credential, &caught_up).await;

    let check = spool_backlog_check(&doctor_checks(&harness, &session).await);
    assert_eq!(check["status"], "pass", "{check}");
    assert!(
        check["detail"].as_str().unwrap().contains("2 online Agent"),
        "{check}"
    );
}
