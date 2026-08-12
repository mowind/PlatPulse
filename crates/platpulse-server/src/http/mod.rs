//! HTTP surface: route groups, health, and Web asset hosting.
//!
//! Phase 0 establishes the three route groups (`/api/public/v1`,
//! `/api/admin/v1`, `/api/agent/v1`) with independent middleware and DTO
//! namespaces, minimal `/health/*` routes, and the SPA asset pipeline with
//! `/api/*` kept out of the SPA fallback. Real Public/Admin/Agent routes
//! arrive with Phase 1 tickets.

mod admin;
mod agent;
pub(crate) mod health;
mod public;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Request, State};
use axum::http::header::{self, HeaderValue};
use axum::http::{HeaderName, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tower_http::services::fs::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::database::ServerDatabase;

/// Response header every route group middleware sets so the group namespace
/// is observable on the wire.
pub(crate) const ROUTE_GROUP_HEADER: HeaderName =
    HeaderName::from_static("x-platpulse-route-group");

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Unified API error envelope (design §13.3): clients depend only on `code`;
/// `message` never leaks SQL, RPC URLs, credentials, or stacks; `requestId`
/// correlates a response with its request log; `fields` carries per-field
/// validation details when a later ticket adds them.
#[derive(Debug, Serialize)]
pub struct ApiErrorBody {
    error: ApiError,
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    code: &'static str,
    message: &'static str,
    #[serde(rename = "requestId")]
    request_id: String,
    fields: Vec<String>,
}

impl ApiErrorBody {
    fn not_found(request_id: &str) -> Self {
        Self {
            error: ApiError {
                code: "not_found",
                message: "route not found",
                request_id: request_id.to_owned(),
                fields: Vec::new(),
            },
        }
    }
}

/// Request correlation id inserted by `request_id_middleware` and echoed in
/// API error envelopes and the `x-request-id` response header.
#[derive(Clone)]
struct RequestId(Arc<str>);

async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    let id = uuid::Uuid::new_v4().to_string();
    request
        .extensions_mut()
        .insert(RequestId(Arc::from(id.as_str())));
    let mut response = next.run(request).await;
    match response.headers_mut().get_mut(&X_REQUEST_ID) {
        Some(value) => *value = HeaderValue::from_str(&id).expect("uuid is a valid header value"),
        None => {
            response.headers_mut().insert(
                X_REQUEST_ID,
                HeaderValue::from_str(&id).expect("uuid is a valid header value"),
            );
        }
    }
    response
}

/// Server state injected into handlers. `web_index` is read once at startup:
/// the SPA entry is immutable for the process lifetime and is served with
/// `Cache-Control: no-cache` so clients revalidate.
#[derive(Clone)]
pub struct AppState {
    db: Arc<ServerDatabase>,
    web_assets: Option<PathBuf>,
    web_index: Option<Bytes>,
    web_assets_ready: bool,
}

impl AppState {
    /// Build application state. The Web assets directory is optional: the
    /// Server must start without Web assets (design §14.1) and report
    /// `web_assets_missing` from `/health/ready` instead. Readiness requires
    /// both `index.html` and the hashed `assets/` directory: an incomplete
    /// build must not report ready.
    pub fn new(db: ServerDatabase, web_assets: Option<PathBuf>) -> Self {
        let web_index = web_assets
            .as_deref()
            .and_then(|dir| std::fs::read(dir.join("index.html")).ok())
            .map(Bytes::from);
        let web_assets_ready = web_index.is_some()
            && web_assets
                .as_deref()
                .is_some_and(|dir| dir.join("assets").is_dir());
        Self {
            db: Arc::new(db),
            web_assets,
            web_index,
            web_assets_ready,
        }
    }

    fn db(&self) -> &ServerDatabase {
        &self.db
    }

    fn web_assets(&self) -> Option<&PathBuf> {
        self.web_assets.as_ref()
    }

    fn web_index(&self) -> Option<&Bytes> {
        self.web_index.as_ref()
    }

    fn web_assets_ready(&self) -> bool {
        self.web_assets_ready
    }
}

/// Assemble the complete HTTP application.
pub fn build_app(state: AppState) -> Router {
    let assets_dir = state
        .web_assets()
        .map(|dir| dir.join("assets"))
        .unwrap_or_else(|| PathBuf::from("/nonexistent/platpulse-web-assets"));

    let api = Router::<AppState>::new()
        .nest("/public/v1", public::router())
        .nest("/admin/v1", admin::router())
        .nest("/agent/v1", agent::router())
        .fallback(api_not_found);

    // Vite emits hashed assets under `assets/`; they are immutable by name
    // (design §14.1), so the response is cacheable forever.
    let assets = Router::<AppState>::new()
        .nest_service(
            "/assets",
            ServeDir::new(assets_dir).append_index_html_on_directories(false),
        )
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        ));

    Router::<AppState>::new()
        .nest("/api", api)
        .route("/health/live", get(health::live))
        .route("/health/ready", get(health::ready))
        .merge(assets)
        .fallback(spa_index)
        .layer(axum::middleware::from_fn(request_id_middleware))
        .with_state(state)
}

fn api_not_found_body(request_id: &str) -> (StatusCode, Json<ApiErrorBody>) {
    (
        StatusCode::NOT_FOUND,
        Json(ApiErrorBody::not_found(request_id)),
    )
}

/// JSON 404 for API namespaces; `/api/*` must never fall through to the SPA.
async fn api_not_found(Extension(request_id): Extension<RequestId>) -> impl IntoResponse {
    api_not_found_body(&request_id.0)
}

/// SPA fallback for non-`/api` paths: serve `index.html` with no-cache so the
/// browser revalidates every navigation while hashed assets stay immutable.
async fn spa_index(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match state.web_index() {
        Some(index) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(axum::body::Body::from(index.clone()))
            .expect("static SPA response is always valid"),
        None => api_not_found_body(&request_id.0).into_response(),
    }
}

#[cfg(test)]
mod tests {

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::header;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::database::{ServerDatabaseConfig, initialize};

    use super::*;

    const INDEX_HTML: &str = "<!doctype html><title>PlatPulse</title>";

    /// Build a state whose Web assets directory already contains the given
    /// files. `AppState` caches `index.html` at construction, so tests must
    /// write files before building the state.
    async fn test_state_with_files(web_files: &[(&str, &[u8])]) -> (TempDir, TempDir, AppState) {
        let db_dir = TempDir::new().unwrap();
        let database = initialize(ServerDatabaseConfig::new(db_dir.path().join("server.db")))
            .await
            .unwrap();
        let web_dir = TempDir::new().unwrap();
        for (path, contents) in web_files {
            let path = web_dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        let state = AppState::new(database, Some(web_dir.path().to_path_buf()));
        (db_dir, web_dir, state)
    }

    async fn test_state() -> (TempDir, TempDir, AppState) {
        test_state_with_files(&[]).await
    }

    async fn get(app: Router, uri: &str) -> axum::response::Response {
        app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn json(response: axum::response::Response) -> (StatusCode, Value) {
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value = serde_json::from_slice(&body).unwrap();
        (status, value)
    }

    fn component<'a>(value: &'a Value, name: &str) -> &'a Value {
        value["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|component| component["name"] == name)
            .unwrap()
    }

    #[tokio::test]
    async fn live_reports_ok_without_internal_information() {
        let (_, _, state) = test_state().await;
        let response = get(build_app(state), "/health/live").await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "ok");

        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(!text.contains(env!("CARGO_PKG_VERSION")), "version leaked");
        assert!(!text.contains("server.db"), "db path leaked");
    }

    #[tokio::test]
    async fn ready_reports_ready_with_migrated_database_and_web_assets() {
        let (_, web_dir, state) = test_state_with_files(&[
            ("index.html", INDEX_HTML.as_bytes()),
            ("assets/index-abc123.js", b"// app"),
        ])
        .await;
        let _ = web_dir;

        let (status, value) = json(get(build_app(state), "/health/ready").await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["status"], "ready");
        assert_eq!(component(&value, "sqlite")["status"], "ready");
        assert_eq!(component(&value, "web_assets")["status"], "ready");
    }

    #[tokio::test]
    async fn ready_reports_web_assets_missing_for_an_incomplete_build() {
        let (_, _, state) = test_state_with_files(&[("index.html", INDEX_HTML.as_bytes())]).await;

        let (status, value) = json(get(build_app(state), "/health/ready").await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            component(&value, "web_assets")["reason"],
            "web_assets_missing",
            "index.html without the hashed assets/ directory must not be ready"
        );
    }

    #[tokio::test]
    async fn ready_reports_web_assets_missing_without_assets() {
        let db_dir = TempDir::new().unwrap();
        let database = initialize(ServerDatabaseConfig::new(db_dir.path().join("server.db")))
            .await
            .unwrap();
        let state = AppState::new(database, None);

        let (status, value) = json(get(build_app(state), "/health/ready").await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(value["status"], "not_ready");
        assert_eq!(component(&value, "sqlite")["status"], "ready");
        let web = component(&value, "web_assets");
        assert_eq!(web["status"], "not_ready");
        assert_eq!(web["reason"], "web_assets_missing");

        let body = serde_json::to_string(&value).unwrap();
        assert!(!body.contains("server.db"), "db path leaked");
        assert!(!body.contains("tmp"), "filesystem path leaked");
    }

    #[tokio::test]
    async fn ready_reports_web_assets_missing_when_index_html_is_absent() {
        let (_, _, state) = test_state_with_files(&[("assets/app-123.js", b"// asset")]).await;

        let (status, value) = json(get(build_app(state), "/health/ready").await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(component(&value, "web_assets")["status"], "not_ready");
        assert_eq!(
            component(&value, "web_assets")["reason"],
            "web_assets_missing"
        );
    }

    #[tokio::test]
    async fn api_groups_have_independent_middleware_and_json_not_found() {
        for (group, uri) in [
            ("public", "/api/public/v1/missing"),
            ("admin", "/api/admin/v1/missing"),
            ("agent", "/api/agent/v1/missing"),
        ] {
            let (_, _, state) = test_state().await;
            let response = get(build_app(state), uri).await;

            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(
                response.headers()[ROUTE_GROUP_HEADER],
                group,
                "group middleware did not run for {uri}"
            );
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/json",
                "{uri} must answer JSON, not SPA HTML"
            );
            assert!(
                response.headers().contains_key(&X_REQUEST_ID),
                "{uri} must carry x-request-id"
            );

            let (_, body) = json(response).await;
            assert_eq!(body["error"]["code"], "not_found", "{uri}");
            assert_eq!(body["error"]["fields"], serde_json::json!([]), "{uri}");
            assert!(
                body["error"]["requestId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()),
                "{uri} must carry a request id in the error envelope"
            );
        }
    }

    #[tokio::test]
    async fn unmatched_api_paths_never_fall_through_to_the_spa() {
        for uri in ["/api", "/api/unknown", "/api/unknown/v1/x"] {
            let (_, _, state) =
                test_state_with_files(&[("index.html", INDEX_HTML.as_bytes())]).await;

            let response = get(build_app(state), uri).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/json",
                "{uri} must answer JSON, not SPA HTML"
            );
        }
    }

    #[tokio::test]
    async fn spa_fallback_serves_index_with_no_cache() {
        for uri in ["/", "/admin", "/some/spa/route"] {
            let (_, _, state) =
                test_state_with_files(&[("index.html", INDEX_HTML.as_bytes())]).await;

            let response = get(build_app(state), uri).await;
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "text/html; charset=utf-8"
            );
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "no-cache",
                "index.html must be revalidated, not cached"
            );
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert_eq!(&body[..], INDEX_HTML.as_bytes());
        }
    }

    #[tokio::test]
    async fn hashed_assets_are_served_with_immutable_cache() {
        let (_, web_dir, state) = test_state_with_files(&[
            ("index.html", INDEX_HTML.as_bytes()),
            ("assets/index-abc123.js", b"// app"),
        ])
        .await;
        let _ = web_dir;

        let response = get(build_app(state), "/assets/index-abc123.js").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"// app");
    }

    #[tokio::test]
    async fn missing_assets_and_spa_without_web_assets_answer_json_404() {
        let (_, _, state) = test_state_with_files(&[("index.html", INDEX_HTML.as_bytes())]).await;
        assert_eq!(
            get(build_app(state.clone()), "/assets/missing.js")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        let db_dir = TempDir::new().unwrap();
        let database = initialize(ServerDatabaseConfig::new(db_dir.path().join("x.db")))
            .await
            .unwrap();
        let state = AppState::new(database, None);
        let response = get(build_app(state), "/").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    }

    #[tokio::test]
    async fn openapi_spec_has_health_paths_and_distinct_route_group_tags() {
        let spec: Value = serde_json::from_str(&crate::openapi::spec_json()).unwrap();

        for path in ["/health/live", "/health/ready"] {
            assert!(
                spec["paths"].get(path).is_some(),
                "path {path} missing from OpenAPI spec"
            );
        }
        assert_eq!(
            spec["paths"]["/health/live"]["get"]["tags"],
            serde_json::json!(["system"])
        );

        let tags: Vec<&str> = spec["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tag| tag["name"].as_str().unwrap())
            .collect();
        for expected in ["system", "public", "admin", "agent"] {
            assert!(tags.contains(&expected), "tag {expected} missing");
        }
        assert_eq!(tags.len(), 4, "route group tags must stay distinct");
    }
}
