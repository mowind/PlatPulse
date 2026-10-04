//! Shared Host resource history at the HTTP boundary (issue #215, design §11.4).
//!
//! The unit tests in crate::metric_history drive the scope-aware ledger
//! directly. This suite drives the same seam the way an Operator and an Agent
//! do: it builds the full application with build_app against a temporary SQLite
//! database, enrolls real Agents through the Admin and Agent HTTP APIs, submits
//! real Reports, and reads the range back through the Owner-only Admin routes.
//!
//! It proves what the issue asks for at the surface an Operator uses:
//!
//! 1. The Host quantities one Agent already collects (CPU, physical memory, Load
//!    1/5/15, network rate and per-mount storage usage and capacity) are stored
//!    once for that Agent and readable from both the Agent's own history and the
//!    page of any Node it runs, whose answer still names the Agent the evidence
//!    belongs to.
//! 2. A storage series is identified by the mount path the Agent reported, so a
//!    path change is a different series instead of a claimed device identity, and
//!    a path nobody reported is never observed instead of zero.
//! 3. Node Process history and Host history never answer for each other, and a
//!    Node's Purge never deletes the Host evidence the Agent's other Nodes share.
//! 4. A low-space pause is a counted protection gap on the shared series too,
//!    with each mount path's losses kept apart.
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
use platpulse_server::capacity::{CapacityConfig, CapacityProtection};
use platpulse_server::{auth, database, http, network, secrets};

/// Registered Network tuple for the platon-mainnet key the report fixture
/// declares, matching the unit-test registry exactly.
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const VIEWER_LOGIN_BODY: &str = r#"{"username":"viewer1","password":"correct horse battery"}"#;
/// The Node the injected observation fixture declares.
const NODE_A: &str = "0195f2a1-0014-4014-8014-000000000014";
/// A second Node of the same Agent, declared beside the first one.
const NODE_B: &str = "0195f2a1-0015-4015-8015-000000000015";
/// The Host CPU the minimal fixture already reports.
const FIXTURE_CPU_PERCENT: f64 = 11.0;

/// A fresh Server (real temp SQLite + pepper) and the full router.
///
/// The capacity policy is process state, so a test that changes it rebuilds the
/// router through install_capacity.
struct Harness {
    _dir: TempDir,
    state: AppState,
    app: Router,
}

impl Harness {
    /// Open the Server in [dir] without seeding it.
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

async fn purge_node(harness: &Harness, session: &Session, node_id: &str) {
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/nodes/{node_id}/purge"),
            session,
            &format!("{{\"confirmNodeId\":\"{node_id}\"}}"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// The Host quantities one Report carries, on top of the Node observation the
/// minimal fixture already declares.
#[derive(Default)]
struct HostReport {
    mounts: Vec<(String, u64, u64)>,
    cpu_percent: Option<f64>,
    second_node: bool,
    node_id: Option<String>,
}

impl HostReport {
    fn mount(mut self, mount_path: &str, total_bytes: u64, used_bytes: u64) -> Self {
        self.mounts
            .push((mount_path.to_owned(), total_bytes, used_bytes));
        self
    }

    fn cpu(mut self, percent: f64) -> Self {
        self.cpu_percent = Some(percent);
        self
    }

    fn with_second_node(mut self) -> Self {
        self.second_node = true;
        self
    }

    /// Move the fixture's Node to another identity, so a second Agent's Node is
    /// unambiguous instead of sharing the first Agent's Node ID.
    fn node(mut self, node_id: &str) -> Self {
        self.node_id = Some(node_id.to_owned());
        self
    }
}

/// The minimal wire fixture, re-bound to the enrolled Agent identity and stamped
/// with the Host quantities and the one observation instant the test wants.
///
/// The fixture's Node process component is disabled, so the canonical healthy
/// one is grafted in and re-stamped: a disabled probe produces no Node history,
/// and this suite needs Node Process evidence beside the Host evidence to prove
/// the two kinds stay apart. Every Host component is stamped too, so all the
/// series of one Report belong to the same observation instant.
fn fixture_report(agent_id: &str, sequence: u64, host: &HostReport, observed_at: &str) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    let canonical: Value = serde_json::from_slice(include_bytes!(
        "../../platpulse-core/tests/fixtures/report_v1_canonical.json"
    ))
    .unwrap();
    value["nodes"][0]["process"] = canonical["nodes"][0]["process"].clone();
    if host.second_node {
        let mut observation = value["nodes"][0].clone();
        observation["node_id"] = Value::String(NODE_B.to_owned());
        value["nodes"].as_array_mut().unwrap().push(observation);
        let mut declaration = value["inventory"]["nodes"][0].clone();
        declaration["node_id"] = Value::String(NODE_B.to_owned());
        value["inventory"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(declaration);
    }

    if let Some(node_id) = &host.node_id {
        value["nodes"][0]["node_id"] = Value::String(node_id.clone());
        value["inventory"]["nodes"][0]["node_id"] = Value::String(node_id.clone());
    }

    // The mount paths the Agent collected are the dimensions its storage series
    // are identified by (design §11.5, Story 52).
    let mounts: Vec<Value> = host
        .mounts
        .iter()
        .map(|(mount_path, total_bytes, used_bytes)| {
            serde_json::json!({
                "mount_path": mount_path,
                "total_bytes": total_bytes,
                "used_bytes": used_bytes,
            })
        })
        .collect();
    value["host"]["disk"]["latest"]["mounts"] = Value::Array(mounts);
    if let Some(percent) = host.cpu_percent {
        value["host"]["cpu_percent"]["latest"] = Value::from(percent);
    }

    let observed = Value::String(observed_at.to_owned());
    for component in [
        "cpu_percent",
        "memory",
        "load",
        "disk",
        "network_throughput",
    ] {
        value["host"][component]["attempted_at"] = observed.clone();
        value["host"][component]["latest_observed_at"] = observed.clone();
    }
    let node_count = value["nodes"].as_array().unwrap().len();
    for index in 0..node_count {
        value["nodes"][index]["process"]["attempted_at"] = observed.clone();
        value["nodes"][index]["process"]["latest_observed_at"] = observed.clone();
    }
    value["agent_id"] = Value::String(agent_id.to_owned());
    value["report_sequence"] = Value::from(sequence);
    value["report_id"] = Value::String(format!(
        "0195f2a1-00{sequence:02x}-4015-8015-0000000000{sequence:02x}"
    ));
    value["generated_at"] = Value::String(observed_at.to_owned());
    serde_json::to_vec(&value).unwrap()
}

/// The percent-encoding a query value carrying a mount path needs, so a path
/// with a separator reaches the Server as one dimension.
fn encoded(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

fn with_range(uri: &mut String, from: Option<&str>, to: Option<&str>) {
    if let Some(from) = from {
        uri.push_str(&format!("&from={from}"));
    }
    if let Some(to) = to {
        uri.push_str(&format!("&to={to}"));
    }
}

/// The Agent route: the shared Host series of the Agent that collected them.
fn agent_history_uri(
    agent_id: &str,
    metric: &str,
    dimension: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> String {
    let mut uri = format!(
        "/api/admin/v1/agents/{agent_id}/metric-history?metric={metric}&dimension={}",
        encoded(dimension)
    );
    with_range(&mut uri, from, to);
    uri
}

/// The Node route for the same shared series: a Node page names the Agent whose
/// evidence it shows while asking as one of the Agent's Nodes.
fn node_host_history_uri(
    node_id: &str,
    metric: &str,
    dimension: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> String {
    let mut uri = format!(
        "/api/admin/v1/nodes/{node_id}/host-metric-history?metric={metric}&dimension={}",
        encoded(dimension)
    );
    with_range(&mut uri, from, to);
    uri
}

/// The Node Process route of issue #213, for the kind boundary this suite proves.
fn node_history_uri(node_id: &str, metric: &str, from: Option<&str>, to: Option<&str>) -> String {
    let mut uri = format!("/api/admin/v1/nodes/{node_id}/metric-history?metric={metric}");
    with_range(&mut uri, from, to);
    uri
}

async fn read_history(harness: &Harness, cookie: Option<&str>, uri: &str) -> (StatusCode, Value) {
    let response = harness.send(admin_get(uri, cookie)).await;
    let status = response.status();
    (status, body_json(response).await)
}
/// The one item a series with a single stored observation must answer with.
fn single_item<'a>(body: &'a Value, metric: &str) -> &'a Value {
    let items = body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("{metric} answers items: {body}"));
    assert_eq!(items.len(), 1, "{metric}: {body}");
    &items[0]
}

/// Story 50 and 51: one Report stores each collected Host quantity once for the
/// Agent that collected it, and the Agent's own history answers every one of them
/// with the value and the timing evidence the Report carried.
#[tokio::test]
async fn a_report_stores_each_host_quantity_once_for_the_agent_that_collected_it() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let instant = auth::now_utc() - time::Duration::minutes(5);
    let observed_at = auth::format_rfc3339(instant);
    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            1,
            &HostReport::default().mount("/data", 1000, 100),
            &observed_at,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // One row per collected quantity, plus the two rows of the one mount path:
    // the Agent's Host evidence is stored once, not once per Node it runs.
    let agent_rows = format!("agent_id = '{agent_id}'");
    assert_eq!(
        harness
            .count_where("host_metric_samples", &agent_rows)
            .await,
        10,
        "eight collected quantities and the usage and capacity of one mount"
    );
    assert_eq!(
        harness
            .count_where(
                "host_metric_samples",
                &format!("{agent_rows} AND metric = 'cpu_percent'")
            )
            .await,
        1,
        "the Agent's CPU is one series"
    );
    assert_eq!(
        harness
            .count_where("host_metric_series_state", &agent_rows)
            .await,
        10,
        "every stored series has its ledger entry"
    );

    let from = auth::format_rfc3339(instant - time::Duration::hours(1));
    let to = auth::format_rfc3339(auth::now_utc() + time::Duration::minutes(1));
    let collected = [
        ("cpu_percent", "", FIXTURE_CPU_PERCENT),
        ("memory_used_bytes", "", 4_294_967_296.0),
        ("memory_total_bytes", "", 17_179_869_184.0),
        ("load1", "", 0.4),
        ("load5", "", 0.35),
        ("load15", "", 0.3),
        ("network_rx_bytes_per_sec", "", 0.0),
        ("network_tx_bytes_per_sec", "", 0.0),
        ("disk_used_bytes", "/data", 100.0),
        ("disk_total_bytes", "/data", 1000.0),
    ];
    for (metric, dimension, value) in collected {
        let (status, body) = read_history(
            &harness,
            Some(&session.cookie),
            &agent_history_uri(&agent_id, metric, dimension, Some(&from), Some(&to)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{metric}: {body}");
        assert_eq!(
            body["scopeKind"],
            Value::String("host".to_owned()),
            "{body}"
        );
        assert_eq!(
            body["scopeKey"],
            Value::String(agent_id.clone()),
            "the shared series is stored under the Agent: {body}"
        );
        assert!(
            body["nodeId"].is_null(),
            "the Agent route names no Node: {body}"
        );
        assert_eq!(body["metric"], Value::String(metric.to_owned()), "{body}");
        assert_eq!(
            body["dimension"],
            Value::String(dimension.to_owned()),
            "{body}"
        );
        assert_eq!(body["grain"], Value::String("raw".to_owned()), "{body}");
        let item = single_item(&body, metric);
        assert_eq!(item["value"], Value::from(value), "{metric}: {body}");
        assert_eq!(item["grain"], Value::String("raw".to_owned()), "{body}");
        assert_eq!(item["source"], Value::String("raw".to_owned()), "{body}");
        assert_eq!(
            item["observedAt"],
            Value::String(observed_at.clone()),
            "{metric}: the sample carries the instant the Agent observed it"
        );
        assert_eq!(
            body["series"]["firstObservedAt"],
            Value::String(observed_at.clone()),
            "{metric}: {body}"
        );
        assert_eq!(body["series"]["observationCount"], Value::from(1), "{body}");
    }

    // Reading history stores nothing: the evidence is the Agent's Report.
    assert_eq!(
        harness
            .count_where("host_metric_samples", &agent_rows)
            .await,
        10
    );
}
/// Story 51: the Host evidence belongs to the Agent, so every Node page of that
/// Agent reads the same stored series and still names the Agent it came from.
#[tokio::test]
async fn both_node_pages_of_one_agent_read_the_same_shared_series() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let instant = auth::now_utc() - time::Duration::minutes(5);
    let observed_at = auth::format_rfc3339(instant);
    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            1,
            &HostReport::default()
                .with_second_node()
                .mount("/data", 1000, 100),
            &observed_at,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Two Nodes in one Report do not double the Agent's Host evidence, and the
    // Node Process series each Node owns are still stored beside it.
    let agent_rows = format!("agent_id = '{agent_id}'");
    assert_eq!(
        harness
            .count_where("host_metric_samples", &agent_rows)
            .await,
        10,
        "the Agent's Host quantities are stored once per Report, not once per Node"
    );
    assert_eq!(
        harness
            .count_where(
                "host_metric_samples",
                &format!("{agent_rows} AND metric = 'cpu_percent'")
            )
            .await,
        1
    );
    for node_id in [NODE_A, NODE_B] {
        assert!(
            harness
                .count_where("node_metric_samples", &format!("node_id = '{node_id}'"))
                .await
                > 0,
            "{node_id} keeps its own Node Process series"
        );
    }

    let from = auth::format_rfc3339(instant - time::Duration::hours(1));
    let to = auth::format_rfc3339(auth::now_utc() + time::Duration::minutes(1));
    for node_id in [NODE_A, NODE_B] {
        let (status, body) = read_history(
            &harness,
            Some(&session.cookie),
            &node_host_history_uri(node_id, "cpu_percent", "", Some(&from), Some(&to)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{node_id}: {body}");
        assert_eq!(body["scopeKind"], "host", "{node_id}: {body}");
        assert_eq!(
            body["scopeKey"], agent_id,
            "the page still names the Agent that collected the evidence: {body}"
        );
        assert_eq!(body["nodeId"], node_id, "{body}");
        let item = single_item(&body, "cpu_percent");
        assert_eq!(item["value"], Value::from(FIXTURE_CPU_PERCENT), "{body}");
        assert_eq!(
            item["observedAt"],
            Value::String(observed_at.clone()),
            "{node_id} reads the one stored observation: {body}"
        );
    }

    let (status, body) = read_history(
        &harness,
        Some(&session.cookie),
        &agent_history_uri(&agent_id, "cpu_percent", "", Some(&from), Some(&to)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["nodeId"].is_null(),
        "the Agent answer belongs to no particular Node: {body}"
    );
    let agent_item = single_item(&body, "cpu_percent");
    assert_eq!(
        agent_item["value"],
        Value::from(FIXTURE_CPU_PERCENT),
        "{body}"
    );
    assert_eq!(
        agent_item["observedAt"],
        Value::String(observed_at.clone()),
        "both surfaces answer the same stored observation: {body}"
    );
    assert_eq!(
        harness
            .count_where(
                "host_metric_samples",
                &format!("{agent_rows} AND metric = 'cpu_percent'")
            )
            .await,
        1,
        "reading both pages stored nothing new"
    );
}

/// Story 51: Node Process history and shared Host history never answer for each
/// other, so a Node page cannot show the wrong kind of number.
#[tokio::test]
async fn each_kind_of_route_refuses_the_other_kinds_series() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let instant = auth::now_utc() - time::Duration::minutes(5);
    let observed_at = auth::format_rfc3339(instant);
    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            1,
            &HostReport::default().mount("/data", 1000, 100),
            &observed_at,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let from = auth::format_rfc3339(instant - time::Duration::hours(1));
    let to = auth::format_rfc3339(auth::now_utc() + time::Duration::minutes(1));

    // The Node Process route of issue #213 keeps answering its own kind.
    let (status, body) = read_history(
        &harness,
        Some(&session.cookie),
        &node_history_uri(NODE_A, "process_cpu_percent", Some(&from), Some(&to)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scopeKind"], "node", "{body}");
    assert_eq!(body["scopeKey"], NODE_A, "{body}");
    assert_eq!(body["nodeId"], NODE_A, "{body}");

    let (status, body) = read_history(
        &harness,
        Some(&session.cookie),
        &node_history_uri(NODE_A, "cpu_percent", Some(&from), Some(&to)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_metric", "{body}");

    // A series of the other kind and a series nobody reports are refused the
    // same way: neither page can invent a number for it.
    for uri in [
        agent_history_uri(&agent_id, "process_cpu_percent", "", Some(&from), Some(&to)),
        node_host_history_uri(NODE_A, "process_memory_percent", "", Some(&from), Some(&to)),
        node_host_history_uri(NODE_B, "carrier_pigeons", "", Some(&from), Some(&to)),
    ] {
        let (status, body) = read_history(&harness, Some(&session.cookie), &uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert_eq!(body["error"]["code"], "invalid_metric", "{uri}: {body}");
    }
}
/// Story 52: a storage series is identified by the mount path the Agent
/// reported, so a path change is a different series instead of a claimed
/// physical-device identity, and a path nobody reported is never observed.
#[tokio::test]
async fn a_storage_series_is_named_by_the_mount_path_the_agent_reported() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let instant = auth::now_utc() - time::Duration::minutes(5);
    let observed_at = auth::format_rfc3339(instant);
    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            1,
            &HostReport::default()
                .mount("/data", 1000, 100)
                .mount("/mnt/data", 9000, 900),
            &observed_at,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Two mounts are two series of the same Agent, and each of them has its own
    // ledger entry: neither overwrites the other.
    let agent_rows = format!("agent_id = '{agent_id}'");
    for metric in ["disk_used_bytes", "disk_total_bytes"] {
        let series = format!("{agent_rows} AND metric = '{metric}'");
        assert_eq!(
            harness.count_where("host_metric_samples", &series).await,
            2,
            "{metric} keeps one row per mount path"
        );
        assert_eq!(
            harness
                .count_where("host_metric_series_state", &series)
                .await,
            2,
            "{metric} keeps one ledger entry per mount path"
        );
    }

    let from = auth::format_rfc3339(instant - time::Duration::hours(1));
    let to = auth::format_rfc3339(auth::now_utc() + time::Duration::minutes(1));
    for (mount_path, used, total) in [("/data", 100.0, 1000.0), ("/mnt/data", 900.0, 9000.0)] {
        let (status, body) = read_history(
            &harness,
            Some(&session.cookie),
            &agent_history_uri(
                &agent_id,
                "disk_used_bytes",
                mount_path,
                Some(&from),
                Some(&to),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{mount_path}: {body}");
        assert_eq!(body["dimension"], mount_path, "{body}");
        assert_eq!(
            single_item(&body, "disk_used_bytes")["value"],
            Value::from(used),
            "{mount_path}: {body}"
        );

        let (status, body) = read_history(
            &harness,
            Some(&session.cookie),
            &agent_history_uri(
                &agent_id,
                "disk_total_bytes",
                mount_path,
                Some(&from),
                Some(&to),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{mount_path}: {body}");
        assert_eq!(body["dimension"], mount_path, "{body}");
        assert_eq!(
            single_item(&body, "disk_total_bytes")["value"],
            Value::from(total),
            "{mount_path}: {body}"
        );
    }

    // The same series through a Node page names both the Agent and the path.
    let (status, body) = read_history(
        &harness,
        Some(&session.cookie),
        &node_host_history_uri(
            NODE_A,
            "disk_used_bytes",
            "/mnt/data",
            Some(&from),
            Some(&to),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scopeKey"], agent_id, "{body}");
    assert_eq!(body["nodeId"], NODE_A, "{body}");
    assert_eq!(body["dimension"], "/mnt/data", "{body}");
    assert_eq!(
        single_item(&body, "disk_used_bytes")["value"],
        Value::from(900.0),
        "{body}"
    );

    // A path the Agent never reported is a series nobody observed: absent.
    let (status, body) = read_history(
        &harness,
        Some(&session.cookie),
        &agent_history_uri(
            &agent_id,
            "disk_used_bytes",
            "/mnt/never",
            Some(&from),
            Some(&to),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["items"].as_array().unwrap().is_empty(), "{body}");
    assert_eq!(body["series"]["observed"], Value::Bool(false), "{body}");
    assert!(
        body["series"]["firstObservedAt"].is_null(),
        "a series nobody reported has no first observation: {body}"
    );

    // The path is part of the series identity, so a missing path never falls
    // back to another mount's rows, and a series that exists once per Agent has
    // no path to give: both are series nobody reported.
    for uri in [
        agent_history_uri(&agent_id, "disk_used_bytes", "", Some(&from), Some(&to)),
        agent_history_uri(&agent_id, "cpu_percent", "/data", Some(&from), Some(&to)),
        node_host_history_uri(NODE_A, "disk_total_bytes", "", Some(&from), Some(&to)),
        node_host_history_uri(NODE_A, "load1", "/data", Some(&from), Some(&to)),
    ] {
        let (status, body) = read_history(&harness, Some(&session.cookie), &uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        assert!(
            body["items"].as_array().unwrap().is_empty(),
            "{uri}: {body}"
        );
        assert_eq!(
            body["series"]["observed"],
            Value::Bool(false),
            "{uri}: {body}"
        );
    }

    // A query the route cannot even read is refused before any series is chosen.
    let malformed =
        format!("/api/admin/v1/agents/{agent_id}/metric-history?metric=cpu_percent&limit=zero");
    let (status, body) = read_history(&harness, Some(&session.cookie), &malformed).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_query", "{body}");
}

/// Story 52, and the review of issue #215: a mount path is an identity the Agent
/// reported, so the Server compares it literally. A path that differs from another
/// only in surrounding whitespace is a different mount, and reading one never answers
/// with the other's evidence. The production panels read the path exactly as the
/// Operator typed it for the same reason: trimming an input would silently substitute
/// one mount's series for another's.
#[tokio::test]
async fn a_mount_path_is_an_identity_the_server_compares_literally() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let instant = auth::now_utc() - time::Duration::minutes(5);
    let observed_at = auth::format_rfc3339(instant);
    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            1,
            &HostReport::default()
                .mount("/data", 1000, 100)
                .mount("/data ", 2000, 200),
            &observed_at,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Two mounts of one Report are two series, and a trailing space is part of
    // the path the Agent mounted rather than something the Server rewrites.
    let agent_rows = format!("agent_id = '{agent_id}' AND metric = 'disk_used_bytes'");
    assert_eq!(
        harness
            .count_where("host_metric_samples", &agent_rows)
            .await,
        2,
        "a padded mount path is its own series"
    );
    assert_eq!(
        harness
            .count_where("host_metric_series_state", &agent_rows)
            .await,
        2,
        "a padded mount path keeps its own ledger entry"
    );

    let from = auth::format_rfc3339(instant - time::Duration::hours(1));
    let to = auth::format_rfc3339(auth::now_utc() + time::Duration::minutes(1));
    for (mount_path, used) in [("/data", 100.0), ("/data ", 200.0)] {
        let (status, body) = read_history(
            &harness,
            Some(&session.cookie),
            &agent_history_uri(
                &agent_id,
                "disk_used_bytes",
                mount_path,
                Some(&from),
                Some(&to),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{mount_path:?}: {body}");
        assert_eq!(body["dimension"], mount_path, "{mount_path:?}: {body}");
        assert_eq!(
            single_item(&body, "disk_used_bytes")["value"],
            Value::from(used),
            "{mount_path:?} answers its own mount's evidence: {body}"
        );
    }

    // Both Node routes read the same literal identities, so a Node page and the
    // Agent page cannot disagree about which mount an answer belongs to.
    for (mount_path, used) in [("/data", 100.0), ("/data ", 200.0)] {
        let (status, body) = read_history(
            &harness,
            Some(&session.cookie),
            &node_host_history_uri(
                NODE_A,
                "disk_used_bytes",
                mount_path,
                Some(&from),
                Some(&to),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{mount_path:?}: {body}");
        assert_eq!(body["dimension"], mount_path, "{mount_path:?}: {body}");
        assert_eq!(
            single_item(&body, "disk_used_bytes")["value"],
            Value::from(used),
            "{mount_path:?} through a Node page: {body}"
        );
    }

    // Neither spelling answers for the other: a Server that trimmed the path
    // would merge the two series and answer both queries with one of them.
    assert_eq!(
        single_item(
            &read_history(
                &harness,
                Some(&session.cookie),
                &agent_history_uri(
                    &agent_id,
                    "disk_used_bytes",
                    "/data",
                    Some(&from),
                    Some(&to),
                ),
            )
            .await
            .1,
            "disk_used_bytes"
        )["value"],
        Value::from(100.0),
        "the plain path never answers with the padded mount's reading"
    );
}

/// Story 59 and design §11.6: the shared history shows what the Server stored
/// for a registered Agent, and only the Owner may read it.
#[tokio::test]
async fn an_unknown_owner_is_missing_and_the_shared_history_is_owner_only() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let instant = auth::now_utc() - time::Duration::minutes(5);
    let observed_at = auth::format_rfc3339(instant);
    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            1,
            &HostReport::default().mount("/data", 1000, 100),
            &observed_at,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let from = auth::format_rfc3339(instant - time::Duration::hours(1));
    let to = auth::format_rfc3339(auth::now_utc() + time::Duration::minutes(1));

    // An Agent nobody enrolled, and a Node that never reported, are both simply
    // not there: the shared series of a stranger is not an empty series.
    let unknown = "0195f2a1-00ff-40ff-80ff-0000000000ff";
    for uri in [
        agent_history_uri(unknown, "cpu_percent", "", Some(&from), Some(&to)),
        node_host_history_uri(unknown, "cpu_percent", "", Some(&from), Some(&to)),
    ] {
        let (status, body) = read_history(&harness, Some(&session.cookie), &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert_eq!(body["error"]["code"], "not_found", "{uri}: {body}");
    }

    // Anonymous readers get nothing at all.
    let (status, body) = read_history(
        &harness,
        None,
        &agent_history_uri(&agent_id, "cpu_percent", "", Some(&from), Some(&to)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "auth_required", "{body}");

    // A Viewer reads the Node and Agent views of the Admin surface, but the
    // cross-Agent resource history stays Owner-only.
    let hash = auth::hash_password(b"correct horse battery").unwrap();
    auth::create_viewer(harness.state.db(), "viewer1", &hash)
        .await
        .unwrap();
    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    for uri in [
        agent_history_uri(&agent_id, "cpu_percent", "", Some(&from), Some(&to)),
        node_host_history_uri(NODE_A, "cpu_percent", "", Some(&from), Some(&to)),
    ] {
        let (status, body) = read_history(&harness, Some(&viewer.cookie), &uri).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
        assert_eq!(body["error"]["code"], "owner_required", "{uri}: {body}");
    }

    // A range the Server cannot read is refused as a range, never answered as an
    // empty history.
    for uri in [
        agent_history_uri(&agent_id, "cpu_percent", "", Some(&to), Some(&from)),
        node_host_history_uri(NODE_A, "cpu_percent", "", Some(&to), Some(&from)),
        agent_history_uri(
            &agent_id,
            "cpu_percent",
            "",
            Some("not-an-instant"),
            Some(&to),
        ),
        node_host_history_uri(
            NODE_A,
            "cpu_percent",
            "",
            Some(&from),
            Some("not-an-instant"),
        ),
    ] {
        let (status, body) = read_history(&harness, Some(&session.cookie), &uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert_eq!(
            body["error"]["code"], "invalid_history_range",
            "{uri}: {body}"
        );
    }
}
/// Story 51: the shared series of one Agent is never another Agent's evidence.
#[tokio::test]
async fn two_agents_keep_their_own_host_series_apart() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (first_agent, first_credential) = enroll_agent(&harness, &session).await;
    let (second_agent, second_credential) = enroll_agent(&harness, &session).await;

    let instant = auth::now_utc() - time::Duration::minutes(5);
    let observed_at = auth::format_rfc3339(instant);
    // The second Agent observes the same quantities on a Node of its own, so the
    // two Agents' shared series are told apart by identity alone.
    let (status, body) = submit(
        &harness,
        &first_credential,
        fixture_report(&first_agent, 1, &HostReport::default(), &observed_at),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = submit(
        &harness,
        &second_credential,
        fixture_report(
            &second_agent,
            2,
            &HostReport::default().cpu(41.25).node(NODE_B),
            &observed_at,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let from = auth::format_rfc3339(instant - time::Duration::hours(1));
    let to = auth::format_rfc3339(auth::now_utc() + time::Duration::minutes(1));
    for (agent_id, expected) in [
        (first_agent.as_str(), FIXTURE_CPU_PERCENT),
        (second_agent.as_str(), 41.25),
    ] {
        assert_eq!(
            harness
                .count_where("host_metric_samples", &format!("agent_id = '{agent_id}'"))
                .await,
            8,
            "{agent_id} stores its own eight collected quantities, no mounts reported"
        );
        let (status, body) = read_history(
            &harness,
            Some(&session.cookie),
            &agent_history_uri(agent_id, "cpu_percent", "", Some(&from), Some(&to)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{agent_id}: {body}");
        assert_eq!(body["scopeKey"], agent_id, "{agent_id}: {body}");
        assert_eq!(
            single_item(&body, "cpu_percent")["value"],
            Value::from(expected),
            "{agent_id} answers its own CPU: {body}"
        );
    }
    assert_eq!(
        harness.count_where("host_metric_samples", "1 = 1").await,
        16,
        "two Agents observed once each, not one Agent twice"
    );

    // Each Node page reads the Agent that owns it.
    for (node_id, owner) in [
        (NODE_A, first_agent.as_str()),
        (NODE_B, second_agent.as_str()),
    ] {
        let (status, body) = read_history(
            &harness,
            Some(&session.cookie),
            &node_host_history_uri(node_id, "cpu_percent", "", Some(&from), Some(&to)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{node_id}: {body}");
        assert_eq!(body["scopeKey"], owner, "{node_id}: {body}");
        assert_eq!(body["nodeId"], node_id, "{node_id}: {body}");
    }
}

/// Story 59: purging one Node deletes that Node's own history and keeps the Host
/// evidence every Node of the Agent reads.
#[tokio::test]
async fn purging_one_node_keeps_the_host_series_every_node_of_the_agent_shares() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let instant = auth::now_utc() - time::Duration::minutes(5);
    let observed_at = auth::format_rfc3339(instant);
    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            1,
            &HostReport::default()
                .with_second_node()
                .mount("/data", 1000, 100),
            &observed_at,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        harness.count_where("host_metric_samples", "1 = 1").await,
        10
    );
    assert!(
        harness
            .count_where("node_metric_samples", &format!("node_id = '{NODE_B}'"))
            .await
            > 0
    );

    purge_node(&harness, &session, NODE_B).await;

    assert_eq!(
        harness
            .count_where("node_metric_samples", &format!("node_id = '{NODE_B}'"))
            .await,
        0,
        "the purged Node's own process history is gone"
    );
    assert_eq!(
        harness.count_where("host_metric_samples", "1 = 1").await,
        10,
        "the Host evidence the Agent collected for all of its Nodes stays"
    );
    assert!(
        harness
            .count_where("node_metric_samples", &format!("node_id = '{NODE_A}'"))
            .await
            > 0,
        "and the other Node's own history stays with it"
    );

    let from = auth::format_rfc3339(instant - time::Duration::hours(1));
    let to = auth::format_rfc3339(auth::now_utc() + time::Duration::minutes(1));
    let (status, body) = read_history(
        &harness,
        Some(&session.cookie),
        &agent_history_uri(&agent_id, "cpu_percent", "", Some(&from), Some(&to)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        single_item(&body, "cpu_percent")["value"],
        Value::from(FIXTURE_CPU_PERCENT),
        "the Agent still answers the shared series: {body}"
    );

    // The purged Node is not there any more, and its sibling still reads the
    // very evidence the purge did not touch.
    let (status, body) = read_history(
        &harness,
        Some(&session.cookie),
        &node_host_history_uri(NODE_B, "cpu_percent", "", Some(&from), Some(&to)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found", "{body}");

    let (status, body) = read_history(
        &harness,
        Some(&session.cookie),
        &node_host_history_uri(NODE_A, "cpu_percent", "", Some(&from), Some(&to)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["nodeId"], NODE_A, "{body}");
    assert_eq!(
        single_item(&body, "cpu_percent")["value"],
        Value::from(FIXTURE_CPU_PERCENT),
        "{body}"
    );
}

/// Story 59: a low-space pause is counted protection evidence on the shared
/// series too, and each mount path's losses are counted apart.
#[tokio::test]
async fn a_low_space_pause_counts_the_losses_of_each_mount_apart() {
    let mut harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::minutes(30);
    let instant = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));
    let host = || {
        HostReport::default()
            .mount("/data", 1000, 100)
            .mount("/mnt/data", 9000, 900)
    };

    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(&agent_id, 1, &host(), &instant(0)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let agent_rows = format!("agent_id = '{agent_id}'");
    assert_eq!(
        harness
            .count_where("host_metric_samples", &agent_rows)
            .await,
        12,
        "eight collected quantities and two mount paths of usage and capacity"
    );
    let node_rows = harness.count_where("node_metric_samples", "1 = 1").await;
    assert!(
        node_rows > 0,
        "the unpaused Report stored its Node Process series too"
    );

    // The operator's low-space policy pauses collection: the Reports that arrive
    // while it holds are accepted and store nothing.
    let pressure = forced_policy(&harness);
    pressure.reconcile(harness.pool()).await.unwrap();
    assert!(
        pressure.status().protected,
        "the declared floor is unreachable, so the pause holds"
    );
    harness.install_capacity(Arc::clone(&pressure));
    for sequence in [2, 3] {
        let (status, body) = submit(
            &harness,
            &credential,
            fixture_report(
                &agent_id,
                sequence,
                &host(),
                &instant(10 * (sequence as i64 - 1)),
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a pause is not a delivery failure: {body}"
        );
    }
    assert_eq!(
        harness
            .count_where("host_metric_samples", &agent_rows)
            .await,
        12,
        "a paused Report stores no Host sample"
    );
    assert_eq!(
        harness.count_where("node_metric_samples", "1 = 1").await,
        node_rows,
        "and no Node sample either"
    );

    // Every series the paused Reports carried is counted, and the two mount paths
    // of one metric keep their own counts instead of merging into one series.
    let host_skips = format!("scope_kind = 'host' AND scope_key = '{agent_id}'");
    assert_eq!(
        harness
            .count_where("capacity_skipped_series", &host_skips)
            .await,
        12,
        "eight collected quantities and four storage series, one per mount path"
    );
    assert_eq!(
        harness
            .count_where(
                "capacity_skipped_series",
                &format!("{host_skips} AND skipped_count = 2")
            )
            .await,
        12,
        "each series lost one reading per paused Report"
    );
    for metric in ["disk_used_bytes", "disk_total_bytes"] {
        for mount_path in ["/data", "/mnt/data"] {
            assert_eq!(
                harness
                    .count_where(
                        "capacity_skipped_series",
                        &format!(
                            "{host_skips} AND metric = '{metric}' AND dimension = '{mount_path}'"
                        )
                    )
                    .await,
                1,
                "{metric} of {mount_path} is its own counted series"
            );
        }
    }

    // The pause is evidence on the series an Operator reads: one protection gap
    // carrying the losses of that one mount path, never an unexplained silence.
    let (status, body) = read_history(
        &harness,
        Some(&session.cookie),
        &agent_history_uri(
            &agent_id,
            "disk_used_bytes",
            "/mnt/data",
            Some(&instant(-5)),
            Some(&instant(40)),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(
        single_item(&body, "disk_used_bytes")["value"],
        Value::from(900.0),
        "{body}"
    );
    let gaps = body["gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{body}");
    assert_eq!(gaps[0]["kind"], "protection_pause", "{body}");
    assert_eq!(gaps[0]["skippedCount"], Value::from(2), "{body}");
    // The gap runs from the newest stored observation to the last counted loss:
    // the Server holds no observation between them and never claims one.
    assert_eq!(gaps[0]["from"], instant(0), "{body}");
    assert_eq!(gaps[0]["to"], instant(20), "{body}");
    assert_eq!(
        gaps[0]["seconds"],
        Value::from(1200),
        "the twenty minutes from that observation to the last counted loss"
    );
}
