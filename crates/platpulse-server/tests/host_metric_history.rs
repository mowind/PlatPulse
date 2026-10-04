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
    node_count: usize,
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

    /// Declare [count] Nodes for the one Agent: the Host observation they all
    /// share is collected once for the Agent, not once per Node (Stories 46
    /// and 51), so the Agent's Host evidence must not multiply by Node count.
    fn nodes(mut self, count: usize) -> Self {
        self.node_count = count;
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

    if host.node_count > 1 {
        let mut observations = Vec::with_capacity(host.node_count);
        let mut declarations = Vec::with_capacity(host.node_count);
        for index in 0..host.node_count {
            let node_id = declared_node_id(index);
            let mut observation = value["nodes"][0].clone();
            observation["node_id"] = Value::String(node_id.clone());
            let mut declaration = value["inventory"]["nodes"][0].clone();
            declaration["node_id"] = Value::String(node_id);
            observations.push(observation);
            declarations.push(declaration);
        }
        value["nodes"] = Value::Array(observations);
        value["inventory"]["nodes"] = Value::Array(declarations);
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

/// The Node identity the multi-Node fixture declares for the [index]-th Node,
/// so a test can address a Node it asked for by number.
fn declared_node_id(index: usize) -> String {
    format!("0195f2a1-00{index:02x}-4015-8015-0000000000{index:02x}")
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

// ---------------------------------------------------------------------------
// Issue #216: the storage use history of an Agent, by mount path.
// ---------------------------------------------------------------------------

/// The Agent page's storage-mount route: the mount paths the Agent's stored
/// Host evidence is identified by (design §11.5, Stories 46, 51 and 52).
fn storage_mounts_uri(agent_id: &str) -> String {
    format!("/api/admin/v1/agents/{agent_id}/storage-mounts")
}

async fn read_mounts(
    harness: &Harness,
    cookie: Option<&str>,
    agent_id: &str,
) -> (StatusCode, Value) {
    read_history(harness, cookie, &storage_mounts_uri(agent_id)).await
}

/// The mount paths an answer names, in the order it names them.
fn answered_paths(body: &Value) -> Vec<String> {
    body["mounts"]
        .as_array()
        .unwrap_or_else(|| panic!("mounts: {body}"))
        .iter()
        .map(|mount| {
            mount["mountPath"]
                .as_str()
                .unwrap_or_else(|| panic!("mountPath: {mount}"))
                .to_owned()
        })
        .collect()
}

/// The one mount entry an answer states for a path, so an assertion names the
/// path it is about.
fn mount_entry<'a>(body: &'a Value, mount_path: &str) -> &'a Value {
    body["mounts"]
        .as_array()
        .unwrap_or_else(|| panic!("mounts: {body}"))
        .iter()
        .find(|mount| mount["mountPath"] == Value::String(mount_path.to_owned()))
        .unwrap_or_else(|| panic!("no mount {mount_path}: {body}"))
}

/// Stories 46, 51 and 52: two Agents of three Nodes each report their own mount
/// paths, the Host observation is stored once per Agent instead of once per
/// Node, and each answer lists the paths that Agent reported with the usage and
/// the capacity of every one of them.
#[tokio::test]
async fn each_agent_answers_the_mount_paths_it_reported_once_for_all_its_nodes() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (first_agent, first_credential) = enroll_agent(&harness, &session).await;
    let (second_agent, second_credential) = enroll_agent(&harness, &session).await;
    let instant = auth::now_utc() - time::Duration::minutes(5);
    let observed_at = auth::format_rfc3339(instant);

    let agents = [
        (
            &first_agent,
            &first_credential,
            HostReport::default()
                .nodes(3)
                .cpu(11.0)
                .mount("/data", 1000, 100)
                .mount("/logs", 2000, 200)
                .mount("/mnt/spare", 3000, 300),
        ),
        (
            &second_agent,
            &second_credential,
            HostReport::default()
                .nodes(3)
                .cpu(22.0)
                .mount("/var/lib", 4000, 400)
                .mount("/data2", 5000, 500),
        ),
    ];
    // The Report identity is unique across the Network, so each Agent's Report
    // carries its own sequence.
    for (index, (agent_id, credential, host)) in agents.into_iter().enumerate() {
        let (status, body) = submit(
            &harness,
            credential,
            fixture_report(agent_id, index as u64 + 1, &host, &observed_at),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    // Six Nodes, but eight collected quantities and two series per reported
    // mount path: the Host observation is the Agent's, not a Node's.
    assert_eq!(
        harness
            .count_where(
                "host_metric_samples",
                &format!("agent_id = '{first_agent}'")
            )
            .await,
        8 + 2 * 3,
        "the Host evidence of three Nodes is collected once for their Agent"
    );
    assert_eq!(
        harness
            .count_where(
                "host_metric_samples",
                &format!("agent_id = '{second_agent}'")
            )
            .await,
        8 + 2 * 2
    );
    assert!(
        harness.count_where("node_metric_samples", "1 = 1").await > 0,
        "the Node Process series are a different kind, stored per Node"
    );

    let (status, body) = read_mounts(&harness, Some(&session.cookie), &first_agent).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["agentId"].as_str(),
        Some(first_agent.as_str()),
        "{body}"
    );
    assert_eq!(body["usedMetric"], "disk_used_bytes", "{body}");
    assert_eq!(body["capacityMetric"], "disk_total_bytes", "{body}");
    assert_eq!(body["mountLimit"], Value::from(256), "{body}");
    assert_eq!(body["truncated"], Value::Bool(false), "{body}");
    // One instant, so the paths are ordered by their path: the answer is stable.
    assert_eq!(
        answered_paths(&body),
        ["/data", "/logs", "/mnt/spare"],
        "{body}"
    );
    for (mount_path, total, used) in [
        ("/data", 1000.0, 100.0),
        ("/logs", 2000.0, 200.0),
        ("/mnt/spare", 3000.0, 300.0),
    ] {
        let mount = mount_entry(&body, mount_path);
        assert_eq!(mount["used"]["metric"], "disk_used_bytes", "{mount}");
        assert_eq!(mount["capacity"]["metric"], "disk_total_bytes", "{mount}");
        assert_eq!(mount["used"]["observed"], Value::Bool(true), "{mount}");
        assert_eq!(mount["used"]["latestValue"], Value::from(used), "{mount}");
        assert_eq!(mount["used"]["latestObservedAt"], observed_at, "{mount}");
        assert_eq!(mount["used"]["observationCount"], Value::from(1), "{mount}");
        assert_eq!(mount["used"]["replayedCount"], Value::from(0), "{mount}");
        assert_eq!(
            mount["capacity"]["latestValue"],
            Value::from(total),
            "{mount}"
        );
        assert_eq!(
            mount["capacity"]["observationCount"],
            Value::from(1),
            "{mount}"
        );
        assert!(
            mount["used"]["latestDelaySeconds"].is_number()
                || mount["used"]["latestDelaySeconds"].is_null(),
            "{mount}"
        );
    }
    // One Report is one observation, not a cadence: the Server states that it
    // does not know how often this Agent samples instead of inventing a rhythm
    // and calling the path silent against it.
    assert_eq!(body["cadenceSeconds"], Value::from(0), "{body}");
    assert_eq!(body["silenceThresholdSeconds"], Value::from(0), "{body}");
    for mount in body["mounts"].as_array().unwrap() {
        assert_eq!(mount["observationState"], "unknown", "{body}");
        assert_eq!(mount["silentSeconds"], Value::Null, "{body}");
    }

    // The second Agent's answer is its own evidence, never the first one's paths.
    let (status, second_body) = read_mounts(&harness, Some(&session.cookie), &second_agent).await;
    assert_eq!(status, StatusCode::OK, "{second_body}");
    assert_eq!(
        answered_paths(&second_body),
        ["/data2", "/var/lib"],
        "{second_body}"
    );
    assert_eq!(
        mount_entry(&second_body, "/var/lib")["used"]["latestValue"],
        Value::from(400.0),
        "{second_body}"
    );
}

/// Story 59 and the risk it names: a cadence slower than the Server's habit is
/// still a measured cadence, and a path that stopped being reported is answered
/// as silent against that cadence — not as deleted, and not as zero.
#[tokio::test]
async fn a_slow_cadence_is_measured_and_a_path_that_stopped_is_answered_as_silent() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::minutes(20);
    let instant = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));

    // A fifteen-minute cadence, and the second Report stops carrying /logs.
    for (sequence, at, host) in [
        (
            1,
            0,
            HostReport::default()
                .cpu(11.0)
                .mount("/data", 1000, 100)
                .mount("/logs", 2000, 200),
        ),
        (
            2,
            15,
            HostReport::default().cpu(12.0).mount("/data", 1000, 150),
        ),
    ] {
        let (status, body) = submit(
            &harness,
            &credential,
            fixture_report(&agent_id, sequence, &host, &instant(at)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, body) = read_mounts(&harness, Some(&session.cookie), &agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["cadenceSeconds"],
        Value::from(900),
        "the fifteen minutes this Agent actually samples at: {body}"
    );
    assert_eq!(
        body["silenceThresholdSeconds"],
        Value::from(900),
        "three times that measured rhythm, capped: {body}"
    );
    assert_eq!(answered_paths(&body), ["/data", "/logs"], "{body}");

    let alive = mount_entry(&body, "/data");
    assert_eq!(alive["observationState"], "reported", "{alive}");
    assert_eq!(alive["silentSeconds"], Value::from(300), "{alive}");
    assert_eq!(alive["used"]["latestValue"], Value::from(150.0), "{alive}");
    assert_eq!(alive["used"]["observationCount"], Value::from(2), "{alive}");
    assert_eq!(
        alive["used"]["firstObservedAt"],
        instant(0),
        "the first observation of the series stays stated: {alive}"
    );

    let stopped = mount_entry(&body, "/logs");
    assert_eq!(stopped["observationState"], "silent", "{stopped}");
    assert_eq!(stopped["silentSeconds"], Value::from(1200), "{stopped}");
    // Stopping is not deletion: the path and everything counted on it stay, and
    // the answer states the age of the newest reading it does hold.
    assert_eq!(
        stopped["used"]["latestValue"],
        Value::from(200.0),
        "{stopped}"
    );
    assert_eq!(
        stopped["used"]["observationCount"],
        Value::from(1),
        "{stopped}"
    );
    assert_eq!(stopped["used"]["latestObservedAt"], instant(0), "{stopped}");
}

/// Story 52 and the risk it names: the series is the Agent's mount path, so a
/// path the Agent switched to is its own series, and nothing in the answer
/// claims two paths are one device — or that one path is still the same device.
#[tokio::test]
async fn a_path_the_agent_switched_to_is_its_own_series_and_no_device_is_named() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::minutes(30);
    let instant = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));

    for (sequence, at, host) in [
        (
            1,
            0,
            HostReport::default().cpu(11.0).mount("/data", 1000, 100),
        ),
        // The same disk is now mounted elsewhere, and the Agent says nothing
        // about the device: the Server stores what it was told and nothing more.
        (
            2,
            20,
            HostReport::default()
                .cpu(11.0)
                .mount("/data", 1000, 0)
                .mount("/mnt/data", 1000, 400),
        ),
    ] {
        let (status, body) = submit(
            &harness,
            &credential,
            fixture_report(&agent_id, sequence, &host, &instant(at)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, body) = read_mounts(&harness, Some(&session.cookie), &agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Both paths were reported at the last instant, so the path orders them.
    assert_eq!(answered_paths(&body), ["/data", "/mnt/data"], "{body}");
    assert_eq!(
        mount_entry(&body, "/data")["used"]["observationCount"],
        Value::from(2),
        "the old path kept its own two observations: {body}"
    );
    assert_eq!(
        mount_entry(&body, "/data")["used"]["latestValue"],
        Value::from(0.0),
        "{body}"
    );
    assert_eq!(
        mount_entry(&body, "/mnt/data")["used"]["observationCount"],
        Value::from(1),
        "the new path starts its own series: {body}"
    );
    assert_eq!(
        mount_entry(&body, "/mnt/data")["used"]["latestValue"],
        Value::from(400.0),
        "{body}"
    );
    // The answer carries the path and the series about it: no device, no
    // filesystem type, no identity the Agent never made verifiable.
    for mount in body["mounts"].as_array().unwrap() {
        let mut keys: Vec<&str> = mount
            .as_object()
            .unwrap_or_else(|| panic!("mount object: {mount}"))
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "capacity",
                "mountPath",
                "observationState",
                "silentSeconds",
                "used"
            ],
            "{mount}"
        );
    }
}

/// Stories 51 and 59: purging one Node of an Agent takes that Node's own
/// Process series and keeps the Host history the Agent collected once for all of
/// its Nodes, together with every mount path in it.
#[tokio::test]
async fn purging_one_node_keeps_the_mount_paths_every_node_shared() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let observed_at = auth::format_rfc3339(auth::now_utc() - time::Duration::minutes(5));
    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            1,
            &HostReport::default()
                .nodes(2)
                .cpu(11.0)
                .mount("/data", 1000, 100)
                .mount("/logs", 2000, 200),
            &observed_at,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let purged_node = declared_node_id(1);
    assert!(
        harness
            .count_where("node_metric_samples", &format!("node_id = '{purged_node}'"))
            .await
            > 0,
        "the second Node stored its own Process series"
    );

    purge_node(&harness, &session, &purged_node).await;
    assert_eq!(
        harness
            .count_where("node_metric_samples", &format!("node_id = '{purged_node}'"))
            .await,
        0,
        "the purged Node kept nothing"
    );

    let (status, body) = read_mounts(&harness, Some(&session.cookie), &agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        answered_paths(&body),
        ["/data", "/logs"],
        "the Agent's mount paths are not the purged Node's: {body}"
    );
    for mount_path in ["/data", "/logs"] {
        assert_eq!(
            mount_entry(&body, mount_path)["used"]["observationCount"],
            Value::from(1),
            "{body}"
        );
    }
}

/// Story 59: a low-space pause takes the optional history of the Reports that
/// arrive while it holds, and the mount paths already observed stay exactly as
/// observed. A path only a paused Report carried is not silently missing
/// either: the pause counts the loss of that mount path.
#[tokio::test]
async fn a_pause_keeps_the_mounts_already_observed_and_counts_what_it_dropped() {
    let mut harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::minutes(40);
    let instant = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));

    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            1,
            &HostReport::default().cpu(11.0).mount("/data", 1000, 100),
            &instant(0),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The operator's low-space policy then pauses collection, and the Reports
    // that arrive while it holds are accepted and store nothing.
    let pressure = forced_policy(&harness);
    pressure.reconcile(harness.pool()).await.unwrap();
    assert!(pressure.status().protected);
    harness.install_capacity(Arc::clone(&pressure));
    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            2,
            &HostReport::default()
                .cpu(12.0)
                .mount("/data", 1000, 300)
                .mount("/new", 5000, 10),
            &instant(30),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = read_mounts(&harness, Some(&session.cookie), &agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        answered_paths(&body),
        ["/data"],
        "the list is the paths this Agent has stored evidence for: {body}"
    );
    let kept = mount_entry(&body, "/data");
    assert_eq!(kept["used"]["observationCount"], Value::from(1), "{kept}");
    assert_eq!(kept["used"]["latestValue"], Value::from(100.0), "{kept}");
    assert_eq!(kept["used"]["latestObservedAt"], instant(0), "{kept}");
    assert_eq!(
        kept["observationState"], "unknown",
        "the pause stored nothing, so the Server has no cadence to judge silence by: {body}"
    );
    assert_eq!(
        kept["silentSeconds"],
        Value::Null,
        "and it states no silence it cannot support: {body}"
    );
    // The readings the pause dropped are counted against the mount path they
    // belong to, including on the path that was already known.
    for (metric, mount_path) in [
        ("disk_used_bytes", "/new"),
        ("disk_total_bytes", "/new"),
        ("disk_used_bytes", "/data"),
    ] {
        assert_eq!(
            harness
                .count_where(
                    "capacity_skipped_series",
                    &format!(
                        "scope_kind = 'host' AND scope_key = '{agent_id}' AND metric = '{metric}' AND dimension = '{mount_path}'"
                    )
                )
                .await,
            1,
            "{metric} of {mount_path} is one counted loss"
        );
    }
}

/// Story 59 and the promise the design makes about it: while the Server itself
/// is holding optional history back, a path that keeps being reported is not
/// silent. The pause is stated on the answer instead, and the verdict comes back
/// as soon as the pause ends.
#[tokio::test]
async fn a_pause_is_not_a_silence_while_the_agent_keeps_its_rhythm() {
    let mut harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::minutes(60);
    let instant = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));

    // A measured ten-minute rhythm: two observations, so the Server holds a
    // cadence to judge a silence against.
    for (sequence, at, used) in [(1u64, 0i64, 100u64), (2, 10, 150)] {
        let (status, body) = submit(
            &harness,
            &credential,
            fixture_report(
                &agent_id,
                sequence,
                &HostReport::default().cpu(11.0).mount("/data", 1000, used),
                &instant(at),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, body) = read_mounts(&harness, Some(&session.cookie), &agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["cadenceSeconds"], Value::from(600), "{body}");
    assert_eq!(body["silenceThresholdSeconds"], Value::from(900), "{body}");
    assert_eq!(body["collectionPaused"], Value::Bool(false), "{body}");
    assert_eq!(
        mount_entry(&body, "/data")["observationState"],
        "silent",
        "the path has really gone quiet against its own rhythm: {body}"
    );

    // The operator's low-space policy then pauses optional history, and the
    // Reports that arrive while it holds are accepted and store nothing.
    let pressure = forced_policy(&harness);
    pressure.reconcile(harness.pool()).await.unwrap();
    assert!(pressure.status().protected);
    harness.install_capacity(Arc::clone(&pressure));
    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            3,
            &HostReport::default()
                .cpu(12.0)
                .mount("/data", 1000, 999)
                .mount("/new", 5000, 10),
            &instant(40),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The rhythm is still measured and the newest stored reading is older than
    // the threshold — but the Server is the one holding the readings back, so
    // the age is not the Agent going quiet.
    let (status, body) = read_mounts(&harness, Some(&session.cookie), &agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["collectionPaused"],
        Value::Bool(true),
        "the answer states that the Server is holding collection back: {body}"
    );
    assert_eq!(body["cadenceSeconds"], Value::from(600), "{body}");
    assert_eq!(body["silenceThresholdSeconds"], Value::from(900), "{body}");
    let kept = mount_entry(&body, "/data");
    assert_eq!(
        kept["observationState"], "unknown",
        "a pause is not a silence the Agent caused: {body}"
    );
    assert_eq!(kept["silentSeconds"], Value::Null, "{kept}");
    assert_eq!(kept["used"]["latestValue"], Value::from(150.0), "{kept}");
    assert_eq!(kept["used"]["latestObservedAt"], instant(10), "{kept}");

    // Once the pause ends the same evidence is judged again, and now it is a
    // silence: the verdict was withheld for the pause, never lost.
    let database_path = harness.state.db().path().to_path_buf();
    harness.install_capacity(Arc::new(CapacityProtection::new(
        CapacityConfig::disabled(),
        Some(database_path.as_path()),
    )));
    let (status, body) = read_mounts(&harness, Some(&session.cookie), &agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["collectionPaused"], Value::Bool(false), "{body}");
    let kept = mount_entry(&body, "/data");
    assert_eq!(
        kept["observationState"], "silent",
        "the verdict comes back with the pause lifted: {body}"
    );
    assert!(
        kept["silentSeconds"].as_i64().unwrap() >= 3000,
        "the age of the newest reading it does hold: {kept}"
    );
}

/// An Agent nobody has heard from is an Agent with no mount evidence rather than
/// a missing Agent: the route answers the empty list its ledger holds.
#[tokio::test]
async fn an_agent_that_has_reported_nothing_is_an_empty_list_not_a_missing_agent() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, _credential) = enroll_agent(&harness, &session).await;

    let (status, body) = read_mounts(&harness, Some(&session.cookie), &agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["agentId"].as_str(), Some(agent_id.as_str()), "{body}");
    assert!(answered_paths(&body).is_empty(), "{body}");
    assert_eq!(body["mountLimit"], Value::from(256), "{body}");
    assert_eq!(body["truncated"], Value::Bool(false), "{body}");
    assert_eq!(body["cadenceSeconds"], Value::from(0), "{body}");
    assert_eq!(body["silenceThresholdSeconds"], Value::from(0), "{body}");
    assert_eq!(body["collectionPaused"], Value::Bool(false), "{body}");
}

/// The mount list states its own limit: an Agent with more paths than one answer
/// carries loses the oldest ones, keeps the newest, and says it truncated.
#[tokio::test]
async fn the_mount_list_states_its_truncation_instead_of_hiding_paths() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let base = auth::now_utc() - time::Duration::minutes(60);
    let instant = |minutes: i64| auth::format_rfc3339(base + time::Duration::minutes(minutes));

    // Three hundred distinct paths over three Reports, so the answer has to drop
    // the coldest ones instead of growing with the Agent's mount churn.
    for sequence in 1..=3u64 {
        let mut host = HostReport::default().cpu(11.0);
        for index in 0..100 {
            let mount_path = format!("/bulk-{:03}", (sequence as i64 - 1) * 100 + index);
            host = host.mount(&mount_path, 1000 + index as u64, 10 + index as u64);
        }
        let (status, body) = submit(
            &harness,
            &credential,
            fixture_report(
                &agent_id,
                sequence,
                &host,
                &instant(10 * (sequence as i64 - 1)),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, body) = read_mounts(&harness, Some(&session.cookie), &agent_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["truncated"], Value::Bool(true), "{body}");
    assert_eq!(body["mountLimit"], Value::from(256), "{body}");
    let paths = answered_paths(&body);
    assert_eq!(paths.len(), 256, "{body}");
    // The newest Report's paths come first, in path order, and the whole Report
    // fits inside the limit before any older path is named.
    assert_eq!(paths[0], "/bulk-200", "{body}");
    assert_eq!(paths[99], "/bulk-299", "{body}");
    assert_eq!(paths[100], "/bulk-100", "{body}");
    // The limit then cuts through the oldest Report's paths in that same order:
    // its first paths survive and its last ones are the ones dropped.
    assert_eq!(paths[200], "/bulk-000", "{body}");
    assert_eq!(paths[255], "/bulk-055", "{body}");
    assert!(
        !paths.contains(&"/bulk-056".to_owned()),
        "the oldest paths are the ones the stated limit drops: {body}"
    );
}

/// Constraints 5 and 8: the mount list is the trusted Admin surface, so it is
/// Owner-only, states a stranger as missing, and is never cached.
#[tokio::test]
async fn the_mount_list_is_owner_only_and_an_unknown_agent_is_missing() {
    let harness = Harness::boot().await;
    let session = owner_session(&harness).await;
    let (agent_id, credential) = enroll_agent(&harness, &session).await;
    let observed_at = auth::format_rfc3339(auth::now_utc() - time::Duration::minutes(5));
    let (status, body) = submit(
        &harness,
        &credential,
        fixture_report(
            &agent_id,
            1,
            &HostReport::default().cpu(11.0).mount("/data", 1000, 100),
            &observed_at,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Anonymous readers get nothing at all.
    let (status, body) = read_mounts(&harness, None, &agent_id).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "auth_required", "{body}");

    // A Viewer reads the Node and Agent views of the Admin surface, but the
    // Agent's collected evidence stays Owner-only.
    let hash = auth::hash_password(b"correct horse battery").unwrap();
    auth::create_viewer(harness.state.db(), "viewer1", &hash)
        .await
        .unwrap();
    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    let (status, body) = read_mounts(&harness, Some(&viewer.cookie), &agent_id).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "owner_required", "{body}");

    // An Agent nobody enrolled is not an Agent with no mounts.
    let unknown = "0195f2a1-00ff-40ff-80ff-0000000000ff";
    let (status, body) = read_mounts(&harness, Some(&session.cookie), unknown).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found", "{body}");

    // The answer is a live statement about evidence, so it is never cached.
    let response = harness
        .send(admin_get(
            &storage_mounts_uri(&agent_id),
            Some(&session.cookie),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[axum::http::header::CACHE_CONTROL],
        "no-store"
    );
}
