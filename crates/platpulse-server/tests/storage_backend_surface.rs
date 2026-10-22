//! The Storage Backend boundary the Owner reads (issue #256, ADR 0013).
//!
//! These tests drive the full application over HTTP against whichever Storage
//! Backend the suite selected through the shared test helper (ADR 0013), and
//! they assert what the Operator may learn and what the Server refuses to
//! pretend: the backend identity and its PostgreSQL facts are Owner-only and
//! never carry the connection secret, the capacity surface says the policy
//! does not apply on PostgreSQL instead of reporting a disabled one, and the
//! Public Projection grows no storage field at all.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_server::{AppState, auth, http, secrets};

const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
/// The Agent and Node the Public Projection materialized here owns.
const SEEDED_AGENT_ID: &str = "agent-seeded";
const SEEDED_NODE_ID: &str = "node-seeded";
const SEEDED_NETWORK_KEY: &str = "platon-mainnet";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const VIEWER_LOGIN_BODY: &str = r#"{"username":"viewer","password":"viewer password"}"#;

/// A fresh Server (real temp database + pepper) and the full router.
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
        let auth_config = auth::AuthConfig::development(
            secrets::load_pepper_file(&pepper_path).unwrap(),
            DEVELOPMENT_ORIGIN.to_owned(),
        );
        let state = AppState::new(database, None, auth_config);
        let owner_hash = auth::hash_password(b"correct horse battery").unwrap();
        auth::create_owner(state.db(), "admin", &owner_hash)
            .await
            .unwrap();
        let viewer_hash = auth::hash_password(b"viewer password").unwrap();
        auth::create_viewer(state.db(), "viewer", &viewer_hash)
            .await
            .unwrap();
        let app = http::build_app(state.clone());
        Self {
            _dir: dir,
            state,
            app,
        }
    }

    fn backend(&self) -> &'static str {
        self.state.db().storage_backend()
    }

    /// Publish one Active Node so the Public Projection materializes a
    /// Network DTO. A storage field appended to a populated public entry is
    /// the leak these tests guard against, and an empty projection would
    /// never build that entry.
    async fn seed_public_node(&self) {
        let now = "2026-01-01T00:00:00Z";
        // The Node's Network must exist first: the schema pins the reference.
        platpulse_server::network::create_network(
            self.state.db(),
            SEEDED_NETWORK_KEY,
            "PlatON Mainnet",
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            210_425,
            210_425,
            "lat",
        )
        .await
        .unwrap();
        let pool = self.state.db().pool();
        sqlx::query(
            "INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at, \
             last_received_at, shutdown_state, security_event_count) \
             VALUES ($1, 1, $2, $3, $4, 'running', 0)",
        )
        .bind(SEEDED_AGENT_ID)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, \
             lifecycle, visibility, inventory_revision, first_seen_at, updated_at) \
             VALUES ($1, $2, $3, 'Seeded Node', 'ws://127.0.0.1:1', 'active', 'public', 1, $4, $5)",
        )
        .bind(SEEDED_NODE_ID)
        .bind(SEEDED_AGENT_ID)
        .bind(SEEDED_NETWORK_KEY)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn send(&self, request: Request<Body>) -> axum::response::Response {
        self.app.clone().oneshot(request).await.unwrap()
    }
}

/// The session cookie a logged-in caller presents. These tests only read, so
/// no CSRF token is needed.
struct Session {
    cookie: String,
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn session_get(uri: &str, session: &Session) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::COOKIE, &session.cookie)
        .body(Body::empty())
        .unwrap()
}

fn anonymous_get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
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
    Session { cookie }
}

/// The Owner sees which Storage Backend the Server runs on and, on PostgreSQL,
/// the connection target and the server version — never the password (ADR
/// 0013, issue #256).
#[tokio::test]
async fn the_storage_surface_names_the_backend_and_withholds_the_secret() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;

    let response = harness
        .send(session_get("/api/admin/v1/storage", &owner))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["backend"], harness.backend());

    if harness.backend() == "sqlite" {
        // The Server names the file it protects; there is no connection target.
        let path = harness
            .state
            .db()
            .path()
            .expect("the SQLite backend has a database file")
            .display()
            .to_string();
        assert_eq!(body["databasePath"], path);
        assert!(body["postgres"].is_null(), "{body}");
    } else {
        // The PostgreSQL backend stores no Server file, so it names the
        // connection instead and never a path.
        assert!(body["databasePath"].is_null(), "{body}");
        let postgres = &body["postgres"];
        assert!(
            !postgres["host"].as_str().unwrap_or_default().is_empty(),
            "{body}"
        );
        assert!(postgres["port"].as_u64().unwrap_or_default() > 0, "{body}");
        assert!(
            !postgres["database"].as_str().unwrap_or_default().is_empty(),
            "{body}"
        );
        assert!(
            !postgres["user"].as_str().unwrap_or_default().is_empty(),
            "{body}"
        );
        let number = postgres["serverVersionNum"]
            .as_i64()
            .expect("a reachable server reports its version");
        let minimum = postgres["minimumServerVersionNum"]
            .as_i64()
            .expect("the surface states the supported minimum");
        assert!(number >= minimum, "{body}");
        let display = postgres["serverVersion"].as_str().unwrap();
        assert!(
            display.starts_with(&(number / 10_000).to_string()),
            "the display and numeric versions must describe the same server: {body}"
        );
    }

    // The connection secret never crosses the trust boundary, and neither the
    // backend password nor the database URL appears anywhere in the payload.
    let text = body.to_string().to_lowercase();
    assert!(!text.contains("password"), "{body}");
    assert!(!text.contains("postgres://"), "{body}");
}

/// Doctor, Capacity and Settings are Owner-only; a Viewer and an anonymous
/// caller learn nothing about the Storage Backend (design §13, issue #256).
#[tokio::test]
async fn the_storage_and_capacity_surfaces_are_owner_only() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;

    for uri in ["/api/admin/v1/storage", "/api/admin/v1/capacity"] {
        let anonymous = harness.send(anonymous_get(uri)).await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED, "{uri}");
        let forbidden = harness.send(session_get(uri, &viewer)).await;
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN, "{uri}");
        let allowed = harness.send(session_get(uri, &owner)).await;
        assert_eq!(allowed.status(), StatusCode::OK, "{uri}");
    }
}

/// On the PostgreSQL backend the capacity policy is not merely switched off:
/// it does not apply, because the database disk belongs to the database
/// operator and the Server refuses a [capacity] section (ADR 0013, issue
/// #256). The SQLite backend keeps an ordinary disabled policy.
#[tokio::test]
async fn the_capacity_surface_says_what_the_storage_backend_cannot_do() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;

    let response = harness
        .send(session_get("/api/admin/v1/capacity", &owner))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    if harness.backend() == "postgres" {
        assert_eq!(body["enabled"], false);
        let reason = body["notApplicableReason"]
            .as_str()
            .expect("PostgreSQL states why capacity does not apply");
        assert!(reason.contains("database operator"), "{body}");
        assert!(body["mountPath"].is_null(), "{body}");
    } else {
        assert!(body["notApplicableReason"].is_null(), "{body}");
    }
}

/// Every key name a JSON document carries, at every depth.
fn collect_keys(value: &Value, keys: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                keys.push(key.clone());
                collect_keys(child, keys);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_keys(item, keys);
            }
        }
        _ => {}
    }
}

/// A root-level key list, sorted so the assertion reads as a frozen snapshot.
fn root_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("a DTO payload is a JSON object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The Public Projection is not a runtime-filtered Admin DTO (AGENTS.md): no
/// public response grows a storage field, so a Visitor cannot learn the
/// backend, its host, or its version (ADR 0013, issue #256).
///
/// The check inspects key names at every depth rather than scanning the
/// serialized text, and it runs against a populated Network: an empty
/// projection would pass while hiding a field inside the entry it never
/// built, so the seeded Network is the DTO snapshot the acceptance asks for.
#[tokio::test]
async fn the_public_projection_carries_no_storage_identity() {
    let harness = Harness::boot().await;
    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    harness.seed_public_node().await;

    let mut bodies = Vec::new();
    for uri in [
        "/api/public/v1/session",
        "/api/public/v1/access",
        "/api/public/v1/networks",
    ] {
        let response = harness.send(session_get(uri, &viewer)).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let body = body_json(response).await;
        let mut keys = Vec::new();
        collect_keys(&body, &mut keys);
        // The markers name Storage Backend identity, not Host observation: a
        // Node publishes hostStoragePercent and nodeDataDirectorySizeBytes,
        // which describe the Node's own disk and never the Server's backend.
        for key in &keys {
            let lowered = key.to_lowercase();
            for forbidden in [
                "databasepath",
                "serverversion",
                "storagebackend",
                "postgres",
                "sqlite",
                "databaseurl",
            ] {
                assert!(
                    !lowered.contains(forbidden),
                    "{uri} leaked the key {key}: {body}"
                );
            }
        }
        let text = body.to_string().to_lowercase();
        for forbidden in [
            "databasepath",
            "serverversion",
            "storagebackend",
            "postgres",
            "sqlite",
        ] {
            assert!(
                !text.contains(forbidden),
                "{uri} leaked {forbidden}: {body}"
            );
        }
        bodies.push(body);
    }

    // The seeded Active Node publishes a Network, so the snapshot covers a
    // populated Public DTO and not only the empty list.
    let networks = &bodies[2];
    let network = networks
        .as_array()
        .and_then(|networks| networks.first())
        .unwrap_or_else(|| panic!("the seeded Active Node must publish a Network: {networks}"));
    assert_eq!(
        root_keys(network),
        vec![
            "displayName".to_owned(),
            "geo".to_owned(),
            "networkKey".to_owned(),
            "nodes".to_owned(),
            "peers".to_owned(),
            "validatorSummary".to_owned(),
            "validators".to_owned(),
        ],
        "the Public Network DTO must keep exactly its published fields: {network}"
    );
    let node = network["nodes"]
        .as_array()
        .and_then(|nodes| nodes.first())
        .unwrap_or_else(|| panic!("the seeded Node must be listed: {network}"));
    assert_eq!(node["nodeId"], SEEDED_NODE_ID);
    assert!(
        !root_keys(node).is_empty(),
        "the Public Node DTO must publish its fields: {node}"
    );
}
