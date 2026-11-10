//! Admin storage failures are answered as failures (issue #413).
//!
//! These tests build the full application with build_app on the Storage
//! Backend the test run selects (ADR 0013) and break the database under a
//! real Admin request. A failed statement must leave a log line under the
//! request id, a unique or check violation must answer `storage_conflict`
//! rather than an ordinary outage, and a failed existence read must answer
//! 503 rather than claim the subject does not exist.

#![allow(
    clippy::unwrap_used,
    reason = "helpers outside #[test] functions unwrap test fixtures"
)]

use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use sqlx::AnyPool;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_server::logging::{LogConfig, LogFormat, subscriber};
use platpulse_server::{AppState, auth, http, network, secrets};

const NETWORK_KEY: &str = "platon-mainnet";
const NETWORK_GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const UNKNOWN_OPERATION: &str = "0195f2a1-0000-4000-8000-000000000413";

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
        let owner_hash = auth::hash_password(b"correct horse battery").unwrap();
        auth::create_owner(state.db(), "admin", &owner_hash)
            .await
            .unwrap();
        network::create_network(
            state.db(),
            NETWORK_KEY,
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

    fn pool(&self) -> &AnyPool {
        self.state.db().pool()
    }

    async fn send(&self, request: Request<Body>) -> axum::response::Response {
        self.app.clone().oneshot(request).await.unwrap()
    }
}

struct Session {
    cookie: String,
    csrf: String,
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
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

async fn login(harness: &Harness) -> Session {
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
    let value = body_json(response).await;
    Session {
        cookie,
        csrf: value["csrfToken"].as_str().unwrap().to_owned(),
    }
}

fn silence_body(matcher_kind: &str, matcher_value: &str, reason: &str) -> String {
    serde_json::json!({
        "matcherKind": matcher_kind,
        "matcherValue": matcher_value,
        "reason": reason,
        "startsAt": "2026-01-01T00:00:00Z",
        "endsAt": "2026-01-02T00:00:00Z",
    })
    .to_string()
}

/// The bytes the Server's log egress writes, captured for one test.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

struct SinkWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Sink {
    fn writer(&self) -> impl Fn() -> SinkWriter + Send + Sync + 'static {
        let inner = Arc::clone(&self.0);
        move || SinkWriter(Arc::clone(&inner))
    }

    fn events(&self) -> Vec<Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}

/// A unique violation is a schema or invariant bug, not an outage: the
/// mutation answers `storage_conflict`, writes nothing, and leaves an error
/// line under the request id that names the failing call site.
#[tokio::test]
async fn a_constraint_failure_on_an_admin_mutation_is_a_logged_storage_conflict() {
    let harness = Harness::boot().await;
    let owner = login(&harness).await;
    // A test-only rule: no two Silences may share a reason.
    sqlx::query("CREATE UNIQUE INDEX test_one_silence_per_reason ON silences (reason)")
        .execute(harness.pool())
        .await
        .unwrap();
    let body = silence_body("network", NETWORK_KEY, "planned upgrade");
    let first = harness
        .send(admin_post("/api/admin/v1/alerts/silences", &owner, &body))
        .await;
    assert_eq!(first.status(), StatusCode::OK);

    let sink = Sink::default();
    let config = LogConfig {
        level: None,
        format: LogFormat::Json,
    };
    let (subscriber, _) = subscriber(&config, None, sink.writer());
    let guard = tracing::subscriber::set_default(subscriber);
    let second = harness
        .send(admin_post("/api/admin/v1/alerts/silences", &owner, &body))
        .await;
    drop(guard);

    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    let request_id = second.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let value = body_json(second).await;
    assert_eq!(value["error"]["code"], "storage_conflict", "{value}");
    assert_eq!(
        value["error"]["message"],
        "Server storage rejected the change"
    );
    let rendered = value.to_string();
    for leak in ["silences", "UNIQUE", "unique", "sqlx"] {
        assert!(
            !rendered.contains(leak),
            "the error envelope leaked an internal detail: {rendered}"
        );
    }
    let silences: i64 = sqlx::query_scalar("SELECT count(*) FROM silences")
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(silences, 1, "the rejected Silence must not be stored");

    let events = sink.events();
    let logged = events
        .iter()
        .find(|event| {
            event["message"]
                .as_str()
                .is_some_and(|message| message.starts_with("admin storage error at "))
        })
        .unwrap_or_else(|| panic!("no admin storage error was logged: {events:?}"));
    assert_eq!(logged["level"], "ERROR");
    assert_eq!(logged["request_id"], request_id.as_str());
    assert!(
        logged["message"]
            .as_str()
            .unwrap()
            .contains("alerts_admin.rs:"),
        "{logged}"
    );
    assert!(logged["error"].as_str().is_some(), "{logged}");
}

/// A Silence target that cannot be read is not a target that does not exist:
/// the request answers 503 instead of a validation error.
#[tokio::test]
async fn a_failed_silence_target_read_is_unavailable_not_a_missing_target() {
    let harness = Harness::boot().await;
    let owner = login(&harness).await;
    sqlx::query("ALTER TABLE networks RENAME TO networks_unreadable")
        .execute(harness.pool())
        .await
        .unwrap();

    let response = harness
        .send(admin_post(
            "/api/admin/v1/alerts/silences",
            &owner,
            &silence_body("network", NETWORK_KEY, "planned upgrade"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let value = body_json(response).await;
    assert_eq!(value["error"]["code"], "unavailable", "{value}");
    assert_eq!(value["error"]["message"], "Server database is unavailable");
}

/// A Silence whose target is really absent still answers the validation
/// error: only a failed read becomes 503.
#[tokio::test]
async fn an_absent_silence_target_is_still_a_validation_error() {
    let harness = Harness::boot().await;
    let owner = login(&harness).await;

    let response = harness
        .send(admin_post(
            "/api/admin/v1/alerts/silences",
            &owner,
            &silence_body("network", "no-such-network", "planned upgrade"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let value = body_json(response).await;
    assert_eq!(value["error"]["code"], "alert_validation", "{value}");
}

/// An Operation that cannot be read is not an unknown Operation: cancel
/// answers 503 instead of 404 `operation_not_found`.
#[tokio::test]
async fn a_failed_operation_read_on_cancel_is_unavailable_not_not_found() {
    let harness = Harness::boot().await;
    let owner = login(&harness).await;
    sqlx::query("ALTER TABLE operations RENAME TO operations_unreadable")
        .execute(harness.pool())
        .await
        .unwrap();

    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/operations/{UNKNOWN_OPERATION}/cancel"),
            &owner,
            "{}",
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let value = body_json(response).await;
    assert_eq!(value["error"]["code"], "unavailable", "{value}");
}

/// An unknown Operation is still 404 when the read succeeds.
#[tokio::test]
async fn an_unknown_operation_on_cancel_is_still_not_found() {
    let harness = Harness::boot().await;
    let owner = login(&harness).await;

    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/operations/{UNKNOWN_OPERATION}/cancel"),
            &owner,
            "{}",
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let value = body_json(response).await;
    assert_eq!(value["error"]["code"], "operation_not_found", "{value}");
}
