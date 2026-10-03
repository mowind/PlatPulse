//! Backup artifact inspection and verification through the real router
//! (issue #209, part of #202 Story 36; ADR 0008, design §8.4, webui.md §15.12).
//!
//! The artifact is created the only way the design allows — offline, through
//! `backup::create_offline`, outside the Agent's reach — and then driven purely
//! over HTTP: a real Owner session, real Origin/CSRF headers, real Operation
//! rows. These tests assert the Operator-visible contract the WebUI renders:
//! the recorded artifact state stays authoritative until the worker writes a
//! new outcome, acceptance of a verification task is not its result, a corrupt
//! and an unreadable artifact are two distinct recorded failures, a cancelled
//! mid-step verification leaves the last recorded outcome intact, and a Viewer
//! or anonymous caller can neither read nor start anything.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

use platpulse_server::{AppState, auth, backup, database, http, operations, secrets};

const DEVELOPMENT_ORIGIN: &str = "http://127.0.0.1:8080";
const OWNER_LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;
const VIEWER_LOGIN_BODY: &str = r#"{"username":"viewer","password":"viewer password"}"#;

/// A fresh Server (real temp SQLite + pepper + backup directory), the full
/// router, and the pieces needed to authenticate as the Owner.
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

    /// Open the Server database in `dir` with the configured backup directory.
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
        let state =
            AppState::new(database, None, auth).with_backup_dir(Some(dir.path().join("backups")));
        let app = http::build_app(state.clone());
        Self { dir, state, app }
    }

    fn pool(&self) -> &SqlitePool {
        self.state.db().pool()
    }

    async fn send(&self, request: Request<Body>) -> axum::response::Response {
        self.app.clone().oneshot(request).await.unwrap()
    }

    /// Create one artifact the way an Operator does: during an Offline Backup
    /// Window, through the same code path the `backup` subcommand runs.
    async fn create_offline_artifact(&self) -> String {
        backup::create_offline(&self.state).await.unwrap();
        sqlx::query_scalar::<_, String>(
            "SELECT artifact_id FROM backup_artifacts ORDER BY created_at DESC, artifact_id DESC LIMIT 1",
        )
        .fetch_one(self.pool())
        .await
        .unwrap()
    }

    /// Absolute path of the on-disk artifact an acceptance test tampers with.
    fn artifact_path(&self, artifact_id: &str) -> std::path::PathBuf {
        self.dir
            .path()
            .join("backups")
            .join(format!("platpulse-{artifact_id}.db"))
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

/// Read the artifact the way the detail page does.
async fn read_artifact(harness: &Harness, session: &Session, artifact_id: &str) -> Value {
    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/backups/{artifact_id}"),
            session,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

/// Request a verification and return the accepted Operation id. The request is
/// only an acceptance: it never carries the verification outcome.
async fn request_verification(harness: &Harness, session: &Session, artifact_id: &str) -> String {
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/backups/{artifact_id}/verify"),
            session,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(body["auditEventId"].as_i64().unwrap() > 0);
    assert_eq!(body["operation"]["operation"]["kind"], "backup_verify");
    assert_eq!(body["operation"]["operation"]["status"], "queued");
    body["operation"]["operation"]["operationId"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn read_operation(harness: &Harness, session: &Session, operation_id: &str) -> Value {
    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/operations/{operation_id}"),
            session,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

#[tokio::test]
async fn offline_artifact_is_readable_and_acceptance_is_not_the_result() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let artifact_id = harness.create_offline_artifact().await;

    // The list is the surface the Owner reads first: sanitized metadata, no
    // verdict for an artifact no verification task has ever examined.
    let response = harness
        .send(admin_get("/api/admin/v1/backups", &owner))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let list_text = body_text(response).await;
    let list: Value = serde_json::from_str(&list_text).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["artifactId"], artifact_id);
    assert_eq!(list[0]["verification"], "pending");
    assert!(list[0]["verifyOperationId"].is_null());
    assert_eq!(
        list[0]["filename"],
        format!("platpulse-{artifact_id}.db"),
        "the list carries the recorded file name, not a path"
    );

    // Detail: the recorded facts plus the explicit absence of a verdict.
    let detail = read_artifact(&harness, &owner, &artifact_id).await;
    assert_eq!(detail["artifact"]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(
        detail["artifact"]["schemaVersion"],
        database::SERVER_SCHEMA_VERSION
    );
    assert_eq!(detail["artifact"]["verification"], "pending");
    assert!(detail["artifact"]["verifiedAt"].is_null());
    assert!(detail["verificationError"].is_null());
    assert!(detail["artifact"]["verifyOperationId"].is_null());
    assert!(harness.artifact_path(&artifact_id).exists());

    // No path, secret, or snapshot content reaches the DTO.
    let detail_text = serde_json::to_string(&detail).unwrap();
    for secret in [
        harness.dir.path().to_str().unwrap(),
        "server-pepper",
        "server.db",
        "correct horse battery",
    ] {
        assert!(
            !detail_text.contains(secret),
            "leaked {secret}: {detail_text}"
        );
    }

    // Requesting verification is an acceptance only: the recorded state is
    // unchanged, and a refresh before the worker runs must still say so.
    let operation_id = request_verification(&harness, &owner, &artifact_id).await;
    let queued = read_operation(&harness, &owner, &operation_id).await;
    assert_eq!(queued["operation"]["status"], "queued");
    assert!(queued["result"].is_null());
    let accepted = read_artifact(&harness, &owner, &artifact_id).await;
    assert_eq!(accepted["artifact"]["verification"], "pending");
    assert!(accepted["artifact"]["verifyOperationId"].is_null());

    drain(&harness).await;

    // Terminal: one recorded outcome, linked to the task that produced it.
    let verified = read_artifact(&harness, &owner, &artifact_id).await;
    assert_eq!(verified["artifact"]["verification"], "ok");
    assert!(verified["artifact"]["verifiedAt"].is_string());
    assert!(verified["verificationError"].is_null());
    assert_eq!(
        verified["artifact"]["verifyOperationId"].as_str(),
        Some(operation_id.as_str())
    );
    let finished = read_operation(&harness, &owner, &operation_id).await;
    assert_eq!(finished["operation"]["status"], "succeeded");
    assert_eq!(finished["operation"]["kind"], "backup_verify");
    assert_eq!(finished["result"]["verification"], "ok");
    assert_eq!(finished["cancellable"], false);
    assert_eq!(finished["errors"].as_array().unwrap().len(), 0);
    let list_after: Value = body_json(
        harness
            .send(admin_get("/api/admin/v1/backups", &owner))
            .await,
    )
    .await;
    assert_eq!(
        list_after[0]["verifyOperationId"].as_str(),
        Some(operation_id.as_str()),
        "the list an Owner refreshes links the same verification task"
    );

    // The completion is durable Audit, not only a redrawn badge.
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'operation_finished' AND target_kind = 'operation' AND target_id = ?",
    )
    .bind(&operation_id)
    .fetch_one(harness.pool())
    .await
    .unwrap();
    assert_eq!(audited, 1);
}

#[tokio::test]
async fn corrupt_and_unreadable_artifacts_are_distinct_recorded_failures() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let artifact_id = harness.create_offline_artifact().await;
    let path = harness.artifact_path(&artifact_id);

    // A healthy artifact first, so the failures below are visibly a change of
    // a previously good record rather than a first-ever attempt.
    let first = request_verification(&harness, &owner, &artifact_id).await;
    drain(&harness).await;
    assert_eq!(
        read_artifact(&harness, &owner, &artifact_id).await["artifact"]["verification"],
        "ok"
    );

    // Corrupt: the bytes on disk no longer match the recorded checksum.
    std::fs::write(&path, b"tampered").unwrap();
    let corrupt = request_verification(&harness, &owner, &artifact_id).await;
    drain(&harness).await;
    let detail = read_artifact(&harness, &owner, &artifact_id).await;
    assert_eq!(detail["artifact"]["verification"], "failed");
    assert!(
        detail["verificationError"]
            .as_str()
            .unwrap()
            .contains("checksum")
    );
    assert_eq!(
        detail["artifact"]["verifyOperationId"].as_str(),
        Some(corrupt.as_str())
    );
    let failed = read_operation(&harness, &owner, &corrupt).await;
    assert_eq!(failed["operation"]["status"], "failed");
    assert_eq!(failed["errors"][0]["code"], "backup_verification_failed");
    assert!(
        failed["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("checksum")
    );
    assert_ne!(
        detail["artifact"]["verifyOperationId"].as_str(),
        Some(first.as_str()),
        "the linkage follows the newest verification task"
    );

    // Unreadable: the recorded artifact row and its metadata survive; only the
    // scan's verdict changes, and the reason names the file rather than
    // claiming a pass.
    std::fs::remove_file(&path).unwrap();
    let missing = request_verification(&harness, &owner, &artifact_id).await;
    drain(&harness).await;
    let detail = read_artifact(&harness, &owner, &artifact_id).await;
    assert_eq!(detail["artifact"]["artifactId"], artifact_id);
    assert_eq!(detail["artifact"]["verification"], "failed");
    assert!(
        detail["verificationError"]
            .as_str()
            .unwrap()
            .contains("cannot open")
    );
    assert_eq!(
        detail["artifact"]["verifyOperationId"].as_str(),
        Some(missing.as_str())
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_artifacts")
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(rows, 1, "verification never deletes the recorded artifact");

    // A failure never leaks the artifact directory into the DTO.
    let detail_text = serde_json::to_string(&detail).unwrap();
    assert!(!detail_text.contains(harness.dir.path().to_str().unwrap()));

    // An artifact id the Server has no row for is a 404, for reading and for
    // requesting work.
    let unknown = "00000000-0000-4000-8000-000000000000";
    let response = harness
        .send(admin_get(
            &format!("/api/admin/v1/backups/{unknown}"),
            &owner,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "backup_artifact_not_found"
    );
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/backups/{unknown}/verify"),
            &owner,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "backup_artifact_not_found"
    );
    // No work was created by a rejected request.
    let operations_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operations")
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(operations_count, 3);
}

#[tokio::test]
async fn cancelled_verification_leaves_the_last_recorded_outcome_intact() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let artifact_id = harness.create_offline_artifact().await;

    let recorded = request_verification(&harness, &owner, &artifact_id).await;
    drain(&harness).await;
    let before = read_artifact(&harness, &owner, &artifact_id).await;
    assert_eq!(before["artifact"]["verification"], "ok");

    // The Owner cancels while the second task is in flight. Queue the request
    // first, then move the row to running and record the cancellation the way
    // the worker would observe it.
    let second = request_verification(&harness, &owner, &artifact_id).await;
    sqlx::query(
        "UPDATE operations SET status = 'running', started_at = created_at WHERE operation_id = ?",
    )
    .bind(&second)
    .execute(harness.pool())
    .await
    .unwrap();
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/operations/{second}/cancel"),
            &owner,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cancel = body_json(response).await;
    // A request is not a completion: the page must keep saying running.
    assert_eq!(cancel["operation"]["operation"]["status"], "running");
    assert_eq!(cancel["operation"]["operation"]["cancelRequested"], true);
    assert!(cancel["operation"]["operation"]["finishedAt"].is_null());
    assert_eq!(cancel["operation"]["cancellable"], false);

    drain(&harness).await;

    // Cancelled and terminal, honestly: no result, and the artifact keeps the
    // outcome the previous task recorded instead of losing it.
    let cancelled = read_operation(&harness, &owner, &second).await;
    assert_eq!(cancelled["operation"]["status"], "cancelled");
    assert!(cancelled["operation"]["finishedAt"].is_string());
    assert!(cancelled["result"].is_null());
    assert_eq!(cancelled["errors"].as_array().unwrap().len(), 0);
    let after = read_artifact(&harness, &owner, &artifact_id).await;
    assert_eq!(after["artifact"]["verification"], "ok");
    assert!(after["verificationError"].is_null());
    assert_eq!(
        after["artifact"]["verifyOperationId"].as_str(),
        Some(recorded.as_str()),
        "a cancelled task never re-labels the artifact"
    );
    assert_eq!(
        after["artifact"]["verifiedAt"], before["artifact"]["verifiedAt"],
        "a cancelled task never rewrites the recorded check time"
    );
}

#[tokio::test]
async fn the_backup_surface_is_owner_only_behind_real_origin_and_csrf() {
    let harness = Harness::boot().await;
    let owner = login(&harness, OWNER_LOGIN_BODY).await;
    let viewer = login(&harness, VIEWER_LOGIN_BODY).await;
    let artifact_id = harness.create_offline_artifact().await;

    // Anonymous: nothing is readable and nothing can be started.
    let anonymous = harness
        .send(
            Request::builder()
                .method("GET")
                .uri("/api/admin/v1/backups")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let anonymous = harness
        .send(
            Request::builder()
                .method("POST")
                .uri(format!("/api/admin/v1/backups/{artifact_id}/verify"))
                .header(header::ORIGIN, DEVELOPMENT_ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    // A Viewer may not read the Admin backup surface, let alone start a scan.
    for uri in [
        "/api/admin/v1/backups".to_owned(),
        format!("/api/admin/v1/backups/{artifact_id}"),
    ] {
        let response = harness.send(admin_get(&uri, &viewer)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
    }
    let response = harness
        .send(admin_post(
            &format!("/api/admin/v1/backups/{artifact_id}/verify"),
            &viewer,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // The mutation is guarded by the real middleware: a present session is not
    // enough, and neither header is decorative.
    let missing_csrf = harness
        .send(
            Request::builder()
                .method("POST")
                .uri(format!("/api/admin/v1/backups/{artifact_id}/verify"))
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
                .uri(format!("/api/admin/v1/backups/{artifact_id}/verify"))
                .header("x-csrf-token", &owner.csrf)
                .header(header::COOKIE, &owner.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(missing_origin.status(), StatusCode::FORBIDDEN);

    // Only the guarded Owner request created work, and only it changed state.
    let operations_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operations")
        .fetch_one(harness.pool())
        .await
        .unwrap();
    assert_eq!(operations_count, 0);
    assert_eq!(
        read_artifact(&harness, &owner, &artifact_id).await["artifact"]["verification"],
        "pending"
    );
}
