//! Operations and Doctor acceptance through the real router (issue #208,
//! part of #202 Stories 34/35, design §8.4, webui.md §5.5/§8.4).
//!
//! These tests build the full application with build_app against a temporary
//! SQLite database and drive it only over HTTP: a real Owner session, real
//! Origin/CSRF headers, real Operation rows. They assert the Operator-visible
//! contract — queued/running/terminal status, progress, outcome, failure
//! detail, cancel-requested-is-not-completion, durable Audit, and that a
//! Viewer or anonymous caller can neither read nor start anything.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_server::{AppState, auth, database, http, operations, secrets};

const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const VIEWER_LOGIN_BODY: &str = r#"{"username":"viewer","password":"viewer password"}"#;

/// A fresh Server (real temp SQLite + pepper), the full router, and the pieces
/// needed to authenticate as the Owner.
struct Harness {
    dir: TempDir,
    state: AppState,
    app: Router,
}

impl Harness {
    /// A first boot: create the Owner/Viewer accounts, then build the router.
    async fn boot() -> Self {
        let dir = TempDir::new().unwrap();
        let harness = Self::open(dir).await;
        let owner_hash = auth::hash_password(b"correct horse battery").unwrap();
        auth::create_owner(harness.state.db(), "admin", &owner_hash)
            .await
            .unwrap();
        let viewer_hash = auth::hash_password(b"viewer password").unwrap();
        auth::create_viewer(harness.state.db(), "viewer", &viewer_hash)
            .await
            .unwrap();
        harness
    }

    /// Open (or re-open) the Server database in `dir` without seeding. The
    /// restart case drops the previous pool first so the exclusive SQLite lock
    /// is released before this second opener arrives.
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
        Self { dir, state, app }
    }

    /// Simulate a Server restart over the same durable database directory.
    async fn restart(self) -> Self {
        let Harness { dir, state, app } = self;
        drop(app);
        drop(state);
        Self::open(dir).await
    }

    fn pool(&self) -> &SqlitePool {
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

async fn body_text(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn admin_get(uri: &str, session: &Session) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::COOKIE, &session.cookie)
        .body(Body::empty())
        .unwrap()
}

fn admin_post(uri: &str, session: &Session) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
        .header("x-csrf-token", &session.csrf)
        .header(header::COOKIE, &session.cookie)
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
    let value = body_json(response).await;
    Session {
        cookie,
        csrf: value["csrfToken"].as_str().unwrap().to_owned(),
    }
}

/// Drive the real worker until the queue drains, exactly as the background
/// worker would after the HTTP response was already returned.
async fn drain(harness: &Harness) {
    while operations::process_operations(&harness.state)
        .await
        .unwrap()
        > 0
    {}
}

/// POST a Doctor run and return its Operation id. Doctor is a read-only
/// diagnostic command: no typed confirmation, no delete dialog, just a task.
async fn start_doctor_run(harness: &Harness, session: &Session) -> String {
    let response = harness
        .send(admin_post("/api/admin/v1/doctor", session))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(body["auditEventId"].as_i64().unwrap() > 0);
    body["operation"]["operation"]["operationId"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn doctor_run_is_observable_from_queued_to_terminal_over_http() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;

    // No run yet: no stored report and nothing in flight.
    let response = harness
        .send(admin_get("/api/admin/v1/doctor", &owner))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(body["lastRun"].is_null());
    assert!(body["currentRun"].is_null());
    assert_eq!(body["checks"].as_array().unwrap().len(), 0);

    let operation_id = start_doctor_run(&harness, &owner).await;

    // Queued: the row is listable and readable before any worker step, and a
    // refresh still shows the in-flight run through currentRun.
    let response = harness
        .send(admin_get(
            "/api/admin/v1/operations?kind=doctor_run",
            &owner,
        ))
        .await;
    let list = body_json(response).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["operationId"], operation_id);
    assert_eq!(list[0]["status"], "queued");
    assert_eq!(list[0]["kind"], "doctor_run");
    assert_eq!(list[0]["cancelRequested"], false);
    assert!(list[0]["startedAt"].is_null());
    assert!(list[0]["finishedAt"].is_null());

    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/operations/{operation_id}"),
            &owner,
        ))
        .await;
    let detail = body_json(response).await;
    assert_eq!(detail["operation"]["status"], "queued");
    assert_eq!(detail["cancellable"], true);
    assert_eq!(detail["warnings"].as_array().unwrap().len(), 0);
    assert_eq!(detail["errors"].as_array().unwrap().len(), 0);
    assert!(detail["result"].is_null());

    let response = harness
        .send(admin_get("/api/admin/v1/doctor", &owner))
        .await;
    let body = body_json(response).await;
    assert!(body["lastRun"].is_null());
    assert_eq!(body["currentRun"]["status"], "queued");
    assert_eq!(body["currentRun"]["operationId"], operation_id);

    drain(&harness).await;

    // Terminal: started, finished, and an outcome that keeps the previous
    // checks reachable from the same page.
    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/operations/{operation_id}"),
            &owner,
        ))
        .await;
    let detail = body_json(response).await;
    assert_eq!(detail["operation"]["status"], "succeeded_with_warnings");
    assert!(!detail["operation"]["startedAt"].is_null());
    assert!(!detail["operation"]["finishedAt"].is_null());
    assert!(detail["operation"]["auditEventId"].as_i64().unwrap() > 0);
    assert_eq!(detail["cancellable"], false);
    assert_eq!(detail["errors"].as_array().unwrap().len(), 0);
    let text = detail.to_string();
    assert!(
        !text.contains("correct horse battery"),
        "operation detail leaked a secret"
    );

    let response = harness
        .send(admin_get("/api/admin/v1/doctor", &owner))
        .await;
    let body = body_json(response).await;
    assert!(body["currentRun"].is_null());
    assert_eq!(body["lastRun"]["status"], "succeeded_with_warnings");
    assert_eq!(body["lastRun"]["operationId"], operation_id);
    let checks = body["checks"].as_array().unwrap();
    assert!(checks.len() >= 8);
    for check in checks {
        for field in ["checkId", "label", "status", "detail"] {
            assert!(!check[field].as_str().unwrap_or_default().is_empty());
        }
    }

    // Doctor diagnoses; it never repairs. No artifact appeared and no business
    // row was written by the run itself.
    let artifacts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_artifacts")
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(artifacts, 0);

    // Durable Audit: the start and the terminal outcome both survive.
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE target_id = ? AND event_kind IN ('doctor_started', 'operation_finished')",
    )
    .bind(&operation_id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(audited, 2);
}

#[tokio::test]
async fn operations_and_doctor_are_owner_only_and_csrf_guarded() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    let operation_id = start_doctor_run(&harness, &owner).await;

    // Anonymous and Viewer can neither read the task list nor start a task.
    let anonymous = harness
        .send(
            Request::builder()
                .method("GET")
                .uri("/api/admin/v1/operations")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let response = harness
        .send(admin_get("/api/admin/v1/operations", &viewer))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = harness
        .send(admin_post("/api/admin/v1/doctor", &viewer))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = harness
        .send(admin_get("/api/admin/v1/doctor", &viewer))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Real Origin and CSRF are required for the mutations, not decorative.
    let missing_csrf = harness
        .send(
            Request::builder()
                .method("POST")
                .uri("/api/admin/v1/doctor")
                .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
                .header(header::COOKIE, &owner.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);
    let missing_origin = harness
        .send(
            Request::builder()
                .method("POST")
                .uri("/api/admin/v1/doctor")
                .header("x-csrf-token", &owner.csrf)
                .header(header::COOKIE, &owner.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(missing_origin.status(), StatusCode::FORBIDDEN);

    // An unknown status filter is a client error, not a silent empty list.
    let response = harness
        .send(admin_get("/api/admin/v1/operations?status=frozen", &owner))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "invalid_query");
    let list_text = body_text(
        harness
            .send(admin_get("/api/admin/v1/operations", &owner))
            .await,
    )
    .await;
    assert!(!list_text.contains("correct horse battery"));

    // Only the guarded Owner request created work.
    let response = harness
        .send(admin_get("/api/admin/v1/operations", &owner))
        .await;
    let list = body_json(response).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["operationId"], operation_id);
}

#[tokio::test]
async fn cancel_requested_is_not_immediate_completion() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let operation_id = start_doctor_run(&harness, &owner).await;

    // The worker picked the queued run up and set it running.
    sqlx::query(
        "UPDATE operations SET status = 'running', started_at = created_at WHERE operation_id = ?",
    )
    .bind(&operation_id)
    .execute(harness.pool())
    .await
    .unwrap();

    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/operations/{operation_id}/cancel"),
            &owner,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    // A running Operation only records the request: it is still running, has
    // not finished, and the page must say so.
    assert_eq!(body["operation"]["operation"]["status"], "running");
    assert_eq!(body["operation"]["operation"]["cancelRequested"], true);
    assert!(body["operation"]["operation"]["finishedAt"].is_null());
    assert_eq!(body["operation"]["cancellable"], false);

    // The request is durable and visible in the running filter.
    let response = harness
        .send(admin_get("/api/admin/v1/operations?status=running", &owner))
        .await;
    let list = body_json(response).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["cancelRequested"], true);

    // Asking twice is a conflict, not a second completion.
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/operations/{operation_id}/cancel"),
            &owner,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "operation_not_cancellable"
    );

    // A queued Operation does cancel immediately, and stays terminal.
    let queued_id = start_doctor_run(&harness, &owner).await;
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/operations/{queued_id}/cancel"),
            &owner,
        ))
        .await;
    let body = body_json(response).await;
    assert_eq!(body["operation"]["operation"]["status"], "cancelled");
    assert!(!body["operation"]["operation"]["finishedAt"].is_null());
    assert_eq!(body["operation"]["cancellable"], false);
    drain(&harness).await;
    let response = harness
        .send(admin_get("/api/admin/v1/doctor", &owner))
        .await;
    // A cancelled diagnostic never becomes a report.
    assert!(body_json(response).await["lastRun"].is_null());
}

#[tokio::test]
async fn queued_work_survives_a_restart_and_interrupted_work_fails_honestly() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let queued_id = start_doctor_run(&harness, &owner).await;
    let interrupted_id = start_doctor_run(&harness, &owner).await;
    // The process died with this one mid-run.
    sqlx::query(
        "UPDATE operations SET status = 'running', started_at = created_at WHERE operation_id = ?",
    )
    .bind(&interrupted_id)
    .execute(harness.pool())
    .await
    .unwrap();

    let harness = harness.restart().await;
    // The restart path of a real boot: rows left running by the crash fail
    // with an explicit reason, queued rows survive.
    operations::requeue_interrupted_operations(harness.pool())
        .await
        .unwrap();
    let owner = login(&harness, OWNER_LOGIN_BODY).await;

    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/operations/{interrupted_id}"),
            &owner,
        ))
        .await;
    let body = body_json(response).await;
    assert_eq!(body["operation"]["status"], "failed");
    assert_eq!(body["errors"][0]["code"], "interrupted_by_restart");
    assert!(!body["operation"]["finishedAt"].is_null());
    assert_eq!(body["cancellable"], false);

    // The queued run is still queued after the restart, so the Doctor page
    // shows the same in-flight task a refresh before the restart showed.
    let response = harness
        .send(admin_get("/api/admin/v1/doctor", &owner))
        .await;
    let body = body_json(response).await;
    assert_eq!(body["currentRun"]["operationId"], queued_id);
    assert_eq!(body["currentRun"]["status"], "queued");

    drain(&harness).await;
    let response = harness
        .send(admin_get("/api/admin/v1/doctor", &owner))
        .await;
    let body = body_json(response).await;
    assert!(body["currentRun"].is_null());
    assert_eq!(body["lastRun"]["operationId"], queued_id);
    assert_eq!(body["lastRun"]["status"], "succeeded_with_warnings");
}
