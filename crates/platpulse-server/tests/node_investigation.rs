//! One UTC investigation window over a Node's own evidence at the HTTP boundary
//! (issue #220, stories 54 to 59, design §11.4 to §11.7).
//!
//! Unit tests in crate::investigation drive the window arithmetic, the coverage
//! judgement and the per-source readers directly. This suite drives the same
//! seam the way an Operator and an Agent do: it builds the full application with
//! build_app against a temporary SQLite database, enrolls a real Agent through
//! the Admin and Agent HTTP APIs, submits real Reports, and reads the
//! investigation back through the Owner-only Admin route.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tempfile::TempDir;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tower::ServiceExt;

use platpulse_server::AppState;
use platpulse_server::capacity::{CapacityConfig, CapacityProtection};
use platpulse_server::{auth, database, http, network, secrets};

/// Registered Network tuple for the platon-mainnet key the report fixture
/// declares, matching the unit-test registry exactly.
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const VIEWER_LOGIN_BODY: &str = r#"{"username":"viewer1","password":"correct horse battery"}"#;
/// The Node the injected observation fixture declares.
const NODE_ID: &str = "0195f2a1-0014-4014-8014-000000000014";
/// The source keys, in the fixed order every answer carries them.
const SOURCE_ORDER: [&str; 6] = [
    "node_metrics",
    "node_state",
    "host_metrics",
    "peers",
    "incidents",
    "validator",
];
const COVERAGE_VOCABULARY: [&str; 5] =
    ["complete", "partial", "empty", "unavailable", "unsupported"];
const INVALID_WINDOW_CODE: &str = "invalid_investigation_window";

/// A fresh Server (real temp SQLite + pepper) and the full router.
struct Harness {
    _dir: TempDir,
    state: AppState,
    app: Router,
}

impl Harness {
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

    /// Install the capacity policy the deployment declared and rebuild the
    /// router over the same database.
    fn install_capacity(&mut self, capacity: Arc<CapacityProtection>) {
        self.state = self.state.clone().with_capacity(capacity);
        self.app = http::build_app(self.state.clone());
    }

    fn pool(&self) -> &sqlx::SqlitePool {
        self.state.db().pool()
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
/// always under pressure.
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

/// A Viewer session, which the Admin API must refuse.
async fn viewer_session(harness: &Harness) -> Session {
    let hash = auth::hash_password(b"correct horse battery").unwrap();
    auth::create_viewer(harness.state.db(), "viewer1", &hash)
        .await
        .unwrap();
    login(harness, VIEWER_LOGIN_BODY).await
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

/// A successful sync probe at the instant, syncing or not, at the two key
/// heights.
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

/// A successful consensus probe: exactly the six bounded fields the wire type
/// carries.
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

/// One Report at the instant, carrying the process and host components the
/// canonical fixture holds so the metric history is written too.
fn report(agent_id: &str, sequence: u64, generated_at: &str) -> Vec<u8> {
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
    value["nodes"][0]["chain"]["sync"] =
        sync_ok(generated_at, false, 100 + sequence, 120 + sequence);
    value["nodes"][0]["chain"]["consensus"] =
        consensus_ok(generated_at, 90 + sequence, 88 + sequence, 86 + sequence);
    value["agent_id"] = Value::String(agent_id.to_owned());
    value["report_sequence"] = Value::from(sequence);
    value["report_id"] = Value::String(format!(
        "0195f2a1-00{sequence:02x}-4013-8013-0000000000{sequence:02x}"
    ));
    value["generated_at"] = generated;
    serde_json::to_vec(&value).unwrap()
}

/// Submit one Report per offset, in order, each at base plus that many seconds.
async fn submit_every(
    harness: &Harness,
    credential: &str,
    agent_id: &str,
    base: OffsetDateTime,
    offsets: &[i64],
) {
    for offset in offsets {
        // The sequence is derived from the offset, so two calls in one test
        // never reuse a Report identity.
        let sequence = (*offset as u64) / 60 + 1;
        let at = auth::format_rfc3339(base + time::Duration::seconds(*offset));
        let (status, value) = submit(harness, credential, report(agent_id, sequence, &at)).await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }
}

fn investigation_uri(node_id: &str, query: &str) -> String {
    if query.is_empty() {
        format!("/api/admin/v1/nodes/{node_id}/investigation")
    } else {
        format!("/api/admin/v1/nodes/{node_id}/investigation?{query}")
    }
}

/// Read the investigation as the Owner, refusing anything but a 200.
async fn investigate(harness: &Harness, cookie: &str, node_id: &str, query: &str) -> Value {
    let response = harness
        .send(admin_get(&investigation_uri(node_id, query), Some(cookie)))
        .await;
    let status = response.status();
    let body = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// Read the investigation through the real route and keep the refusal.
async fn investigate_response(
    harness: &Harness,
    cookie: Option<&str>,
    node_id: &str,
    query: &str,
) -> (StatusCode, Value) {
    let response = harness
        .send(admin_get(&investigation_uri(node_id, query), cookie))
        .await;
    let status = response.status();
    (status, body_json(response).await)
}

/// The source entry of one key, which every answer must carry.
fn source<'a>(body: &'a Value, key: &str) -> &'a Value {
    body["sources"]
        .as_array()
        .expect("sources is a list")
        .iter()
        .find(|entry| entry["key"] == Value::String(key.to_owned()))
        .unwrap_or_else(|| panic!("the {key} source is missing from {body}"))
}

fn boundary_kinds(source: &Value) -> Vec<String> {
    source["boundaries"]
        .as_array()
        .expect("boundaries is a list")
        .iter()
        .map(|boundary| boundary["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn instant(value: &Value) -> OffsetDateTime {
    OffsetDateTime::parse(value.as_str().expect("a timestamp"), &Rfc3339).unwrap()
}

/// Stories 54 to 58: an Operator reads one UTC window over one Node and gets
/// every source the Node has, each under its own time basis, and every other
/// principal is refused.
#[tokio::test]
async fn owner_reads_one_window_with_every_source_and_other_principals_are_refused() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    // Fifteen Reports five minutes apart, the oldest of them before the newest
    // window opens, so one window answers completely while a wider one has to
    // disclose where this Node's history starts.
    let base = auth::now_utc() - time::Duration::minutes(70);
    let reported: Vec<i64> = (0..=14).map(|step| step * 300).collect();
    submit_every(&harness, &credential, &agent_id, base, &reported).await;

    let body = investigate(&harness, &session.cookie, NODE_ID, "").await;

    // The window is the adjustable default: 24 hours inside a 30 day horizon,
    // with the raw retention the Server actually keeps named beside it.
    assert_eq!(body["window"]["preset"], "24h");
    assert_eq!(body["window"]["custom"], false);
    assert_eq!(body["window"]["durationSeconds"], 86_400);
    assert_eq!(body["window"]["horizonDays"], 30);
    assert_eq!(body["window"]["rawRetentionDays"], 1);
    assert_eq!(body["window"]["clampedToNow"], false);
    let presets = body["window"]["supportedPresets"].as_array().unwrap();
    assert_eq!(
        presets
            .iter()
            .map(|preset| preset["key"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        vec!["1h", "6h", "24h", "7d", "30d"]
    );
    assert_eq!(presets[0]["hours"], 1);
    assert_eq!(presets[4]["hours"], 720);
    assert!(!presets[0]["label"].as_str().unwrap().is_empty());
    let answered = instant(&body["window"]["answeredAt"]);
    let from = instant(&body["window"]["from"]);
    let to = instant(&body["window"]["to"]);
    assert_eq!(to - from, time::Duration::hours(24));
    assert!(to <= answered, "the window never answers the future");

    assert_eq!(body["nodeId"], NODE_ID);
    assert_eq!(body["agentId"], agent_id);
    assert_eq!(body["lifecycle"], "active");

    // Every source is present, in the fixed order, and says which time is its
    // own, for which subject it answers, and where its evidence can be read.
    assert_eq!(
        body["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["key"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        SOURCE_ORDER.to_vec()
    );
    for entry in body["sources"].as_array().unwrap() {
        let key = entry["key"].as_str().unwrap();
        let coverage = entry["coverage"].as_str().unwrap();
        assert!(
            COVERAGE_VOCABULARY.contains(&coverage),
            "{key} answered an unknown coverage {coverage}"
        );
        assert!(!entry["label"].as_str().unwrap().is_empty());
        assert!(!entry["coverageLabel"].as_str().unwrap().is_empty());
        assert!(!entry["timeBasisLabel"].as_str().unwrap().is_empty());
        let paths = entry["answerPaths"].as_array().unwrap();
        if coverage == "unsupported" {
            // A source that applies to nobody here holds no evidence to point
            // at, but it still has to say why it holds none.
            assert!(
                paths.is_empty(),
                "{key} is unsupported and still points at evidence: {entry}"
            );
            assert!(
                !entry["notes"].as_array().unwrap().is_empty(),
                "{key} is unsupported without saying why"
            );
        } else {
            assert!(
                !paths.is_empty(),
                "{key} answers with no way to read its evidence"
            );
        }
        for path in paths {
            assert!(
                path["path"].as_str().unwrap().starts_with("/api/admin/v1/"),
                "{key} points outside the Admin API"
            );
            assert!(!path["label"].as_str().unwrap().is_empty());
        }
    }

    let node_metrics = source(&body, "node_metrics");
    assert_eq!(node_metrics["subjectKind"], "node");
    assert_eq!(node_metrics["subject"], NODE_ID);
    assert_eq!(node_metrics["timeBasis"], "metric_observation");
    // This window opens before the Node's history does, so the answer is partial
    // and names the stretch it cannot vouch for rather than inventing it.
    assert_eq!(node_metrics["coverage"], "partial", "{node_metrics}");
    assert_eq!(
        boundary_kinds(node_metrics),
        vec!["pre_enablement".to_owned()]
    );
    let raw = node_metrics["grains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|grain| grain["grain"] == "raw")
        .expect("the raw grain answers the newest window");
    assert_eq!(raw["available"], true);
    assert!(raw["pointCount"].as_i64().unwrap() > 0);
    assert!(raw["firstObservedAt"].is_string());
    assert!(raw["lastObservedAt"].is_string());

    // A window that lies inside what the Node has reported answers complete,
    // with no boundary and no missing point to disclose.
    let covered = investigate(&harness, &session.cookie, NODE_ID, "window=1h").await;
    assert_eq!(covered["window"]["preset"], "1h");
    assert_eq!(covered["window"]["durationSeconds"], 3_600);
    let covered_metrics = source(&covered, "node_metrics");
    assert_eq!(covered_metrics["coverage"], "complete", "{covered_metrics}");
    assert!(
        boundary_kinds(covered_metrics).is_empty(),
        "{covered_metrics}"
    );
    assert!(covered_metrics["truncated"].as_bool() == Some(false));

    let host_metrics = source(&body, "host_metrics");
    assert_eq!(host_metrics["subjectKind"], "agent");
    assert_eq!(host_metrics["subject"], agent_id);
    assert_eq!(host_metrics["timeBasis"], "metric_observation");

    assert_eq!(
        source(&body, "node_state")["timeBasis"],
        "state_observation"
    );
    assert_eq!(source(&body, "peers")["timeBasis"], "peer_receipt_bucket");
    assert_eq!(
        source(&body, "incidents")["timeBasis"],
        "incident_evaluation"
    );

    // The Node has no Validator link, which is unsupported rather than empty,
    // and the answer says why in the words the Operator reads.
    let validator = source(&body, "validator");
    assert_eq!(validator["coverage"], "unsupported");
    assert!(!validator["notes"].as_array().unwrap().is_empty());

    // The collector rows ride along, so a reader sees the collection state
    // behind the history without a second request.
    let components = body["components"].as_array().unwrap();
    assert!(!components.is_empty());
    for component in components {
        assert!(!component["scope"].as_str().unwrap().is_empty());
        assert!(!component["componentKey"].as_str().unwrap().is_empty());
        assert!(!component["state"].as_str().unwrap().is_empty());
    }
    assert!(!body["notes"].as_array().unwrap().is_empty());

    // A Viewer is refused, and so is an anonymous caller.
    let viewer = viewer_session(&harness).await;
    let (status, _) = investigate_response(&harness, Some(&viewer.cookie), NODE_ID, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = investigate_response(&harness, None, NODE_ID, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Stories 54 and 55: the window is a preset or an explicit range, and an
/// unusable one is refused with the code the WebUI reads instead of silently
/// answering a different window.
#[tokio::test]
async fn the_window_is_a_preset_or_an_explicit_range_and_an_unusable_one_is_refused() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::minutes(90);
    submit_every(&harness, &credential, &agent_id, base, &[0, 60]).await;

    for (preset, hours) in [
        ("1h", 1_i64),
        ("6h", 6),
        ("24h", 24),
        ("7d", 168),
        ("30d", 720),
    ] {
        let body = investigate(
            &harness,
            &session.cookie,
            NODE_ID,
            &format!("window={preset}"),
        )
        .await;
        assert_eq!(body["window"]["preset"], preset);
        assert_eq!(body["window"]["custom"], false);
        assert_eq!(body["window"]["durationSeconds"], hours * 3_600);
    }

    // An explicit range is echoed exactly and named custom, instead of being
    // forced into the nearest preset.
    let from = auth::now_utc() - time::Duration::hours(2);
    let to = auth::now_utc() - time::Duration::hours(1);
    let from_text = auth::format_rfc3339(from);
    let to_text = auth::format_rfc3339(to);
    let body = investigate(
        &harness,
        &session.cookie,
        NODE_ID,
        &format!("from={from_text}&to={to_text}"),
    )
    .await;
    assert_eq!(body["window"]["preset"], "custom");
    assert_eq!(body["window"]["custom"], true);
    assert_eq!(body["window"]["from"], from_text);
    assert_eq!(body["window"]["to"], to_text);
    assert_eq!(body["window"]["requestedFrom"], from_text);
    assert_eq!(body["window"]["requestedTo"], to_text);
    assert_eq!(body["window"]["durationSeconds"], 3_600);

    // A window that reaches past now is clipped and says so, rather than
    // answering a stretch the Server cannot have observed yet.
    let future = auth::format_rfc3339(auth::now_utc() + time::Duration::hours(2));
    let body = investigate(
        &harness,
        &session.cookie,
        NODE_ID,
        &format!("from={from_text}&to={future}"),
    )
    .await;
    assert_eq!(body["window"]["clampedToNow"], true);
    assert_eq!(body["window"]["requestedTo"], future);
    assert!(instant(&body["window"]["to"]) < instant(&body["window"]["requestedTo"]));

    // Everything the Server cannot answer is refused with one code and a reason.
    let oldest = auth::format_rfc3339(auth::now_utc() - time::Duration::days(31));
    let now_text = auth::format_rfc3339(auth::now_utc());
    let refusals = [
        ("window=bogus".to_owned(), "an unknown preset"),
        (format!("from={from_text}"), "half a range"),
        (
            format!("window=24h&from={from_text}&to={to_text}"),
            "a preset and a range",
        ),
        (format!("from={to_text}&to={from_text}"), "a reversed range"),
        (
            format!("from={oldest}&to={now_text}"),
            "a range wider than the horizon",
        ),
        (
            format!("from={now_text}&to={future}"),
            "a range that starts in the future",
        ),
    ];
    for (query, why) in refusals {
        let (status, body) =
            investigate_response(&harness, Some(&session.cookie), NODE_ID, &query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
        assert_eq!(body["error"]["code"], INVALID_WINDOW_CODE, "{why}: {body}");
        assert!(
            !body["error"]["message"].as_str().unwrap().is_empty(),
            "{why} was refused without a reason"
        );
    }
}

/// Story 56: a silence between two Reports is partial coverage with the
/// interruption it proves, and a source whose history starts inside the window
/// says so rather than answering as if it had covered the whole of it.
#[tokio::test]
async fn a_silence_between_reports_is_partial_coverage_and_never_a_zero() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::hours(2);
    // Two Reports a minute apart, a 31 minute silence, then two more.
    submit_every(
        &harness,
        &credential,
        &agent_id,
        base,
        &[0, 60, 1_920, 1_980],
    )
    .await;

    let body = investigate(&harness, &session.cookie, NODE_ID, "").await;
    let node_metrics = source(&body, "node_metrics");
    assert_eq!(node_metrics["coverage"], "partial");
    assert_eq!(node_metrics["coverageLabel"], "Partial");

    // The interruption is named as a collection failure with the stretch it
    // covers, so the reader sees where the evidence stops instead of a zero.
    let failure = node_metrics["boundaries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|boundary| boundary["kind"] == "collection_failure")
        .unwrap_or_else(|| panic!("the silence is not disclosed: {node_metrics}"));
    assert!(failure["detail"].as_str().unwrap().len() > 20);
    assert!(
        instant(&failure["to"]) - instant(&failure["at"]) >= time::Duration::minutes(30),
        "the disclosed stretch is shorter than the silence"
    );

    let raw = node_metrics["grains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|grain| grain["grain"] == "raw")
        .expect("the raw grain answers the newest window");
    let holes = raw["holes"].as_array().unwrap();
    assert!(!holes.is_empty(), "the raw grain lists no hole: {raw}");
    assert!(holes[0]["seconds"].as_i64().unwrap() >= 1_800);
    assert!(holes[0]["series"].is_string());
    // The cadence the Server measured rides beside the hole, so a reader can
    // tell a missed sample from a slow cadence.
    assert!(raw["cadenceSeconds"].as_i64().unwrap() > 0);
    assert!(raw["longestGapSeconds"].as_i64().unwrap() >= 1_800);

    // The Incident source starts when the Server first saw this Node, which is
    // inside this window: it answers partial and names where its own history
    // begins instead of reporting an empty window it cannot vouch for.
    let incidents = source(&body, "incidents");
    assert_eq!(incidents["coverage"], "partial", "{incidents}");
    assert_eq!(boundary_kinds(incidents), vec!["pre_enablement".to_owned()]);
    assert_eq!(incidents["grains"].as_array().unwrap()[0]["pointCount"], 0);
}

/// Story 59: a low-space pause is a pause rather than a collection failure, and
/// the loss it caused is counted instead of reading as silence.
#[tokio::test]
async fn a_low_space_pause_is_a_pause_and_not_a_collection_failure() {
    let mut harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::minutes(60);
    // A regularly observed Node first, so the sources have evidence to answer
    // with before protection opens.
    submit_every(&harness, &credential, &agent_id, base, &[0, 300]).await;

    let pressure = forced_policy(&harness);
    pressure.reconcile(harness.pool()).await.unwrap();
    assert!(pressure.status().protected);
    harness.install_capacity(Arc::clone(&pressure));
    submit_every(&harness, &credential, &agent_id, base, &[600, 900, 1_200]).await;
    assert!(
        harness
            .count_where(
                "capacity_skipped_series",
                "scope_kind = 'node' AND metric = 'sync'"
            )
            .await
            > 0,
        "the pause recorded no refused state series"
    );

    let body = investigate(&harness, &session.cookie, NODE_ID, "").await;
    for key in ["node_state", "node_metrics"] {
        let entry = source(&body, key);
        let pauses: Vec<&Value> = entry["boundaries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|boundary| boundary["kind"] == "low_space_pause")
            .collect();
        assert_eq!(
            pauses.len(),
            1,
            "the {key} source does not disclose exactly one pause: {entry}"
        );
        let pause = pauses[0];
        assert!(pause["detail"].as_str().unwrap().contains("optional"));
        let pause_from = instant(&pause["at"]);
        let pause_to = instant(&pause["to"]);
        assert!(pause_to >= pause_from);

        // No stretch the pause explains may also be reported as a failure: the
        // two stories must never merge.
        for boundary in entry["boundaries"].as_array().unwrap() {
            if boundary["kind"] != "collection_failure" {
                continue;
            }
            let from = instant(&boundary["at"]);
            let to = boundary["to"].is_string().then(|| instant(&boundary["to"]));
            assert!(
                to.is_none() || from > pause_to || to.unwrap() < pause_from,
                "the {key} source reports both a pause and a failure over {from}"
            );
        }
    }
}

/// Stories 57 and 58: a Node the Server never held evidence for is unavailable
/// with the boundary that says why, rather than an empty window of zeros.
#[tokio::test]
async fn a_node_with_no_evidence_is_unavailable_rather_than_empty() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, _) = enroll_agent(&harness, &session).await;

    // A Node the Agent declared, but that has never reported an observation.
    let quiet_node = "0195f2a1-0014-4014-8014-0000000000ff";
    let seen_at = auth::format_rfc3339(auth::now_utc() - time::Duration::hours(25));
    sqlx::query(
        "INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, \
         lifecycle, visibility, inventory_revision, first_seen_at, updated_at) \
         VALUES (?, ?, 'platon-mainnet', 'Quiet Node', 'http://127.0.0.1:8545', 'active', \
         'private', 1, ?, ?)",
    )
    .bind(quiet_node)
    .bind(&agent_id)
    .bind(&seen_at)
    .bind(&seen_at)
    .execute(harness.pool())
    .await
    .unwrap();

    let body = investigate(&harness, &session.cookie, quiet_node, "").await;

    for key in ["node_metrics", "host_metrics", "peers"] {
        let entry = source(&body, key);
        assert_eq!(
            entry["coverage"], "unavailable",
            "the {key} source reported a window it never observed"
        );
        assert!(
            boundary_kinds(entry).contains(&"never_observed".to_owned()),
            "the {key} source does not say it never observed this subject: {entry}"
        );
        assert_eq!(entry["firstObservedAt"], Value::Null);
        assert_eq!(entry["grains"].as_array().unwrap()[0]["available"], false);
        let detail = entry["boundaries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|boundary| boundary["detail"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(detail.contains("never stored an observation"), "{detail}");
    }

    // The Server has held this Node since before the window began and opened no
    // Incident for it, so the window is known-empty: not unknown, and not a zero
    // standing in for evidence nobody has.
    let incidents = source(&body, "incidents");
    assert_eq!(incidents["coverage"], "empty", "{incidents}");
    assert!(boundary_kinds(incidents).is_empty(), "{incidents}");
    assert_eq!(incidents["grains"].as_array().unwrap()[0]["pointCount"], 0);
}

/// A purged Node says it was purged and when, and an unknown Node is simply not
/// found: neither reads as a Node with no evidence.
#[tokio::test]
async fn a_purged_node_says_when_it_was_purged_and_an_unknown_one_is_not_found() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, _) = enroll_agent(&harness, &session).await;

    let unknown = "0195f2a1-0014-4014-8014-0000000000aa";
    let (status, body) = investigate_response(&harness, Some(&session.cookie), unknown, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    let deleted_at = auth::format_rfc3339(auth::now_utc() - time::Duration::minutes(10));
    sqlx::query(
        "INSERT INTO deleted_nodes (node_id, agent_id, network_key, display_name, \
         deleted_by_user_id, deleted_at) VALUES (?, ?, 'platon-mainnet', 'Purged Node', NULL, ?)",
    )
    .bind(unknown)
    .bind(&agent_id)
    .bind(&deleted_at)
    .execute(harness.pool())
    .await
    .unwrap();

    let (status, body) = investigate_response(&harness, Some(&session.cookie), unknown, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "node_purged");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&deleted_at),
        "the purge answer does not say when: {body}"
    );
}
