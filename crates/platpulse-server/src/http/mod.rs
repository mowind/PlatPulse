//! HTTP surface: route groups, health, and Web asset hosting.
//!
//! Phase 0 established the three route groups (`/api/public/v1`,
//! `/api/admin/v1`, `/api/agent/v1`) with independent middleware and DTO
//! namespaces, minimal `/health/*` routes, and the SPA asset pipeline with
//! `/api/*` kept out of the SPA fallback.
//!
//! P1-01 adds human authentication: public routes (except login) require a
//! valid human Session because Guest access is disabled by default, Admin
//! additionally requires the Owner role, and the Agent group is refused
//! with `setup_required` until the first Owner exists (design §12.2).

pub(crate) mod admin;
pub(crate) mod agent;
pub(crate) mod health;
pub(crate) mod public;
pub(crate) mod realtime;
pub(crate) mod report_ingestion;

use ipnet::IpNet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Extension, Request, State};
use axum::http::HeaderMap;
use axum::http::header::{self, HeaderValue};
use axum::http::{HeaderName, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tower_http::services::fs::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::auth::{
    AuthConfig, LOGIN_MAX_ATTEMPTS, LOGIN_RATE_LIMIT_WINDOW, RateLimiter, SessionError,
    SessionInfo, authenticate_token, cookie_value, has_owner,
};
use crate::database::ServerDatabase;
use crate::enrollment::{
    ENROLL_MAX_ATTEMPTS, ENROLL_RATE_LIMIT_WINDOW, authenticate_agent_credential,
};
use crate::http::realtime::RealtimeHub;

/// Response header every route group middleware sets so the group namespace
/// is observable on the wire.
pub(crate) const ROUTE_GROUP_HEADER: HeaderName =
    HeaderName::from_static("x-platpulse-route-group");

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Unified API error envelope (design §13.3): clients depend only on `code`;
/// `message` never leaks SQL, RPC URLs, credentials, or stacks; `requestId`
/// correlates a response with its request log; `fields` carries per-field
/// validation details when a later ticket adds them.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ApiErrorBody {
    error: ApiError,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ApiError {
    code: &'static str,
    message: &'static str,
    #[serde(rename = "requestId")]
    request_id: String,
    fields: Vec<String>,
}

impl ApiErrorBody {
    fn not_found(request_id: &str) -> Self {
        Self::new("not_found", "route not found", request_id)
    }

    pub(crate) fn new(code: &'static str, message: &'static str, request_id: &str) -> Self {
        Self {
            error: ApiError {
                code,
                message,
                request_id: request_id.to_owned(),
                fields: Vec::new(),
            },
        }
    }
}

/// Request correlation id inserted by `request_id_middleware` and echoed in
/// API error envelopes and the `x-request-id` response header.
#[derive(Clone)]
pub(crate) struct RequestId(Arc<str>);

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

/// The authenticated human session carried by a request that passed the
/// session guard.
#[derive(Clone)]
pub(crate) struct AuthenticatedSession(pub SessionInfo);

#[derive(Debug)]
pub(crate) struct ServerRuntime {
    accepting: AtomicBool,
    shutting_down: AtomicBool,
    corrupt: AtomicBool,
    critical_workers: AtomicBool,
    critical_worker_heartbeat_ms: AtomicU64,
    critical_worker_enabled: AtomicBool,
    in_flight_ingestion: AtomicUsize,
    drained: Notify,
}

impl ServerRuntime {
    fn new() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            shutting_down: AtomicBool::new(false),
            corrupt: AtomicBool::new(false),
            critical_workers: AtomicBool::new(true),
            critical_worker_heartbeat_ms: AtomicU64::new(0),
            critical_worker_enabled: AtomicBool::new(true),
            in_flight_ingestion: AtomicUsize::new(0),
            drained: Notify::new(),
        }
    }

    fn begin_ingestion(&self) -> bool {
        if !self.accepting.load(Ordering::Acquire) {
            return false;
        }
        self.in_flight_ingestion.fetch_add(1, Ordering::AcqRel);
        if !self.accepting.load(Ordering::Acquire) {
            self.finish_ingestion();
            return false;
        }
        true
    }

    fn finish_ingestion(&self) {
        if self.in_flight_ingestion.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.drained.notify_waiters();
        }
    }

    fn mark_worker_heartbeat(&self) {
        self.critical_worker_heartbeat_ms
            .store(now_millis(), Ordering::Release);
        self.critical_workers.store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn fail_critical_worker(&self) {
        self.critical_worker_enabled.store(false, Ordering::Release);
    }

    #[cfg(test)]
    fn recover_critical_worker(&self) {
        self.critical_worker_enabled.store(true, Ordering::Release);
        self.mark_worker_heartbeat();
    }

    fn critical_workers_healthy(&self) -> bool {
        if self.shutting_down.load(Ordering::Acquire) {
            return false;
        }
        let heartbeat = self.critical_worker_heartbeat_ms.load(Ordering::Acquire);
        self.critical_workers.load(Ordering::Acquire)
            && self.critical_worker_enabled.load(Ordering::Acquire)
            && heartbeat != 0
            && now_millis().saturating_sub(heartbeat) <= 2_000
    }

    fn begin_shutdown(&self) {
        self.accepting.store(false, Ordering::Release);
        self.shutting_down.store(true, Ordering::Release);
        self.critical_workers.store(false, Ordering::Release);
    }

    async fn wait_for_ingestion(&self, deadline: tokio::time::Instant) -> bool {
        while self.in_flight_ingestion.load(Ordering::Acquire) != 0 {
            if tokio::time::timeout_at(deadline, self.drained.notified())
                .await
                .is_err()
            {
                return false;
            }
        }
        true
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub(crate) struct IngestionGuard<'a> {
    runtime: &'a ServerRuntime,
}
impl Drop for IngestionGuard<'_> {
    fn drop(&mut self) {
        self.runtime.finish_ingestion();
    }
}

/// the SPA entry is immutable for the process lifetime and is served with
/// `Cache-Control: no-cache` so clients revalidate.
#[derive(Clone)]
pub struct AppState {
    db: Arc<ServerDatabase>,
    auth: Arc<AuthConfig>,
    login_limiter: Arc<RateLimiter>,
    enroll_limiter: Arc<RateLimiter>,
    web_assets: Option<PathBuf>,
    web_index: Option<Bytes>,
    web_assets_ready: bool,
    runtime: Arc<ServerRuntime>,
    proxy_policy: ProxyTrustPolicy,
    pub(crate) public_realtime: RealtimeHub,
    pub(crate) admin_realtime: RealtimeHub,
}

impl AppState {
    /// Build application state with the given authentication policy. The
    /// Web assets directory is optional: the Server must start without Web
    /// assets (design §14.1) and report `web_assets_missing` from
    /// `/health/ready` instead. Readiness requires both `index.html` and
    /// the hashed `assets/` directory: an incomplete build must not report
    /// ready.
    pub fn new(db: ServerDatabase, web_assets: Option<PathBuf>, auth: AuthConfig) -> Self {
        Self::new_with_proxy_policy(db, web_assets, auth, Vec::new(), None)
    }

    /// Build state with the listener's explicit trusted-proxy policy. Proxy
    /// headers are accepted only from a matching peer and only when the
    /// configured asserted scheme is HTTPS.
    pub fn new_with_proxy_policy(
        db: ServerDatabase,
        web_assets: Option<PathBuf>,
        auth: AuthConfig,
        trusted_proxy_cidrs: Vec<IpNet>,
        trusted_proxy_scheme: Option<String>,
    ) -> Self {
        let web_index = web_assets
            .as_deref()
            .and_then(|dir| std::fs::read(dir.join("index.html")).ok())
            .map(Bytes::from);
        let web_assets_ready = web_index.is_some()
            && web_assets
                .as_deref()
                .is_some_and(|dir| dir.join("assets").is_dir());
        let runtime = Arc::new(ServerRuntime::new());
        let worker_runtime = Arc::clone(&runtime);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
            loop {
                tick.tick().await;
                if worker_runtime.shutting_down.load(Ordering::Acquire) {
                    break;
                }
                if worker_runtime
                    .critical_worker_enabled
                    .load(Ordering::Acquire)
                {
                    worker_runtime.mark_worker_heartbeat();
                }
            }
        });
        Self {
            db: Arc::new(db),
            auth: Arc::new(auth),
            login_limiter: Arc::new(RateLimiter::new(
                LOGIN_MAX_ATTEMPTS,
                LOGIN_RATE_LIMIT_WINDOW,
            )),
            enroll_limiter: Arc::new(RateLimiter::new(
                ENROLL_MAX_ATTEMPTS,
                ENROLL_RATE_LIMIT_WINDOW,
            )),
            web_assets,
            web_index,
            web_assets_ready,
            runtime,
            proxy_policy: ProxyTrustPolicy {
                trusted_proxy_cidrs,
                trusted_proxy_scheme,
            },
            public_realtime: RealtimeHub::default(),
            admin_realtime: RealtimeHub::default(),
        }
    }

    pub(crate) fn begin_shutdown(&self) {
        self.runtime.begin_shutdown();
        self.public_realtime.shutdown("server_shutdown");
        self.admin_realtime.shutdown("server_shutdown");
    }

    pub async fn wait_for_ingestion(&self, deadline: tokio::time::Instant) -> bool {
        self.runtime.wait_for_ingestion(deadline).await
    }

    pub async fn checkpoint_wal(&self) -> Result<(), sqlx::Error> {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(self.db.pool())
            .await
            .map(|_| ())
    }

    pub(crate) fn ingestion_guard(&self) -> Option<IngestionGuard<'_>> {
        self.runtime.begin_ingestion().then_some(IngestionGuard {
            runtime: &self.runtime,
        })
    }

    pub(crate) fn is_shutting_down(&self) -> bool {
        self.runtime.shutting_down.load(Ordering::Acquire)
    }
    pub(crate) fn is_corrupt(&self) -> bool {
        self.runtime.corrupt.load(Ordering::Acquire)
    }
    pub(crate) fn critical_workers_healthy(&self) -> bool {
        self.runtime.critical_workers_healthy()
    }

    pub(crate) fn public_realtime(&self) -> RealtimeHub {
        self.public_realtime.clone()
    }

    pub(crate) fn admin_realtime(&self) -> RealtimeHub {
        self.admin_realtime.clone()
    }

    pub(crate) fn db(&self) -> &ServerDatabase {
        &self.db
    }

    pub(crate) fn database(&self) -> Arc<ServerDatabase> {
        Arc::clone(&self.db)
    }

    pub(crate) fn auth(&self) -> &AuthConfig {
        &self.auth
    }

    pub(crate) fn login_limiter(&self) -> &RateLimiter {
        &self.login_limiter
    }

    pub(crate) fn enroll_limiter(&self) -> &RateLimiter {
        &self.enroll_limiter
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

/// Content-Security-Policy enforced on every response (design §19.4: no
/// inline or third-party script). The Vite production bundle only loads
/// hashed same-origin assets, so `script-src 'self'` holds; style
/// attributes set through the CSSOM are not blocked by `style-src`.
const CSP_HEADER_VALUE: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; font-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// Assemble the complete HTTP application.
pub fn build_app(state: AppState) -> Router {
    let assets_dir = state
        .web_assets()
        .map(|dir| dir.join("assets"))
        .unwrap_or_else(|| PathBuf::from("/nonexistent/platpulse-web-assets"));

    // Public routes (except login) require a human Session; Admin requires
    // an Owner session; Agent is refused with `setup_required` until the
    // first Owner exists. Guards run outside the group middleware so their
    // responses never carry route-group headers or DTOs.
    let public_group = public::router().layer(axum::middleware::from_fn_with_state(
        state.clone(),
        human_session_guard,
    ));
    let admin_group = admin::router()
        .layer(axum::middleware::from_fn(owner_role_guard))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            human_session_guard,
        ));
    let agent_group = {
        let routes = agent::router().merge(crate::http::report_ingestion::router());
        routes.layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_group_guard,
        ))
    };

    let api = Router::<AppState>::new()
        .nest("/public/v1", public_group)
        .nest("/admin/v1", admin_group)
        .nest("/agent/v1", agent_group)
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
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP_HEADER_VALUE),
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            proxy_header_guard,
        ))
        .layer(axum::middleware::from_fn(request_id_middleware))
        .with_state(state)
}

#[derive(Clone)]
struct ProxyTrustPolicy {
    trusted_proxy_cidrs: Vec<IpNet>,
    trusted_proxy_scheme: Option<String>,
}

#[derive(Clone, Copy)]
struct TrustedHttpsProxy;

fn request_peer_ip(request: &Request) -> Option<std::net::IpAddr> {
    request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0.ip())
}

fn peer_is_trusted(policy: &ProxyTrustPolicy, peer: Option<std::net::IpAddr>) -> bool {
    peer.is_some_and(|peer| {
        policy
            .trusted_proxy_cidrs
            .iter()
            .any(|cidr| cidr.contains(&peer))
    }) && policy.trusted_proxy_scheme.as_deref() == Some("https")
}

fn forwarded_proto(headers: &HeaderMap) -> Option<Result<&str, ()>> {
    if headers.contains_key("forwarded") && headers.contains_key("x-forwarded-proto") {
        return Some(Err(()));
    }
    if let Some(value) = headers.get("forwarded") {
        let text = match value.to_str() {
            Ok(text) => text,
            Err(_) => return Some(Err(())),
        };
        let mut proto = None;
        for element in text.split(',') {
            let Some(element_proto) = element.split(';').find_map(|part| {
                let (key, value) = part.trim().split_once('=')?;
                key.eq_ignore_ascii_case("proto")
                    .then_some(value.trim_matches('"'))
            }) else {
                return Some(Err(()));
            };
            if proto.is_some_and(|previous| previous != element_proto) {
                return Some(Err(()));
            }
            proto = Some(element_proto);
        }
        return Some(proto.ok_or(()));
    }
    headers.get("x-forwarded-proto").map(|value| {
        let text = value.to_str().map_err(|_| ())?;
        let mut values = text.split(',').map(str::trim);
        let first = values.next().filter(|value| !value.is_empty()).ok_or(())?;
        if values.any(|value| value != first) {
            return Err(());
        }
        Ok(first)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyTrustError {
    UntrustedHeaders,
    HttpsRequired,
}

fn evaluate_proxy_request(
    policy: &ProxyTrustPolicy,
    peer: Option<std::net::IpAddr>,
    headers: &HeaderMap,
) -> Result<bool, ProxyTrustError> {
    let has_forwarded =
        headers.contains_key("forwarded") || headers.contains_key("x-forwarded-proto");
    let trusted = peer_is_trusted(policy, peer);
    if has_forwarded && !trusted {
        return Err(ProxyTrustError::UntrustedHeaders);
    }
    if has_forwarded {
        if forwarded_proto(headers) != Some(Ok("https")) {
            return Err(ProxyTrustError::HttpsRequired);
        }
        return Ok(true);
    }
    Ok(false)
}

async fn proxy_header_guard(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    match evaluate_proxy_request(
        &state.proxy_policy,
        request_peer_ip(&request),
        request.headers(),
    ) {
        Ok(true) => {
            request.extensions_mut().insert(TrustedHttpsProxy);
        }
        Ok(false) => {}
        Err(ProxyTrustError::UntrustedHeaders) => {
            return error_from_proxy_request(
                &request,
                StatusCode::FORBIDDEN,
                "untrusted_proxy_headers",
                "forwarded headers are not accepted from this peer",
            );
        }
        Err(ProxyTrustError::HttpsRequired) => {
            return error_from_proxy_request(
                &request,
                StatusCode::FORBIDDEN,
                "proxy_scheme_required",
                "trusted proxy requests must assert https",
            );
        }
    }
    next.run(request).await
}

fn error_from_proxy_request(
    request: &Request,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| &*id.0)
        .unwrap_or("unknown");
    (status, Json(ApiErrorBody::new(code, message, request_id))).into_response()
}

/// Client address for rate limiting: the real socket address when the
/// Server runs with connection info, `unknown` in tests. Stamped by the
/// guards so handlers can read it as a plain extension.
#[derive(Clone)]
pub(crate) struct ClientIp(pub String);

fn stamp_client_ip(request: &mut Request) {
    if request.extensions().get::<ClientIp>().is_none() {
        let ip = request
            .extensions()
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0.ip().to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        request.extensions_mut().insert(ClientIp(ip));
    }
}

/// Human session guard for the Public and Admin groups: every request
/// except `POST /api/public/v1/login` must present a valid session cookie
/// (design §12.2: Guest disabled by default; §13.1: Public API requires a
/// Viewer/Owner Session).
pub(crate) async fn human_session_guard(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    stamp_client_ip(&mut request);
    if request.method() == Method::POST && request.uri().path().ends_with("/login") {
        return next.run(request).await;
    }
    let Some(token) = cookie_value(request.headers(), &state.auth().cookie_name) else {
        return session_error_response(
            &request,
            StatusCode::UNAUTHORIZED,
            "auth_required",
            "authentication required",
        );
    };
    match authenticate_token(state.db(), state.auth(), token).await {
        Ok(session) => {
            request
                .extensions_mut()
                .insert(AuthenticatedSession(session));
            next.run(request).await
        }
        Err(SessionError::Invalid) => session_error_response(
            &request,
            StatusCode::UNAUTHORIZED,
            "auth_required",
            "authentication required",
        ),
        Err(SessionError::Expired) => session_error_response(
            &request,
            StatusCode::UNAUTHORIZED,
            "auth_required",
            "session expired",
        ),
        Err(SessionError::UserDisabled) => session_error_response(
            &request,
            StatusCode::UNAUTHORIZED,
            "auth_required",
            "user disabled",
        ),
    }
}

/// Owner role guard for the Admin group (design §13.1: Admin only accepts
/// Owner Sessions). Runs after the session guard, which is registered
/// outside it.
pub(crate) async fn owner_role_guard(
    Extension(principal): Extension<AuthenticatedSession>,
    request: Request,
    next: Next,
) -> Response {
    if principal.0.role == "owner" {
        next.run(request).await
    } else {
        session_error_response(
            &request,
            StatusCode::FORBIDDEN,
            "owner_required",
            "the Admin API requires an Owner session",
        )
    }
}

/// Extract a Bearer token from the `Authorization` header. The scheme is
/// case-insensitive per RFC 7235; a missing or empty token reads as `None`.
pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return None;
    }
    Some(token)
}

/// Agent group guard: until the first Owner exists, no Agent Enrollment or
/// reporting is allowed (design §12.2). `POST /enroll` authenticates an
/// Enrollment Token in the handler; every other Agent route requires a
/// valid Agent Credential, so an Enrollment Token can never submit reports
/// and a Human Session can never enroll or report (design §12.6, §13.1).
pub(crate) async fn agent_group_guard(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    stamp_client_ip(&mut request);
    let peer = request_peer_ip(&request);
    if peer.is_some_and(|ip| !ip.is_loopback())
        && request.extensions().get::<TrustedHttpsProxy>().is_none()
    {
        return session_error_response(
            &request,
            StatusCode::FORBIDDEN,
            "agent_transport_insecure",
            "agent authentication requires TLS or a trusted HTTPS proxy",
        );
    }
    match has_owner(state.db()).await {
        Ok(true) => {}
        Ok(false) => {
            return session_error_response(
                &request,
                StatusCode::SERVICE_UNAVAILABLE,
                "setup_required",
                "server setup is incomplete; create the first owner",
            );
        }
        Err(_) => {
            return session_error_response(
                &request,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    }

    let is_enroll = request.method() == Method::POST && request.uri().path().ends_with("/enroll");
    if is_enroll {
        // The handler authenticates the Enrollment Token and rate-limits
        // enrollment independently (design §19.4).
        return next.run(request).await;
    }

    let Some(token) = bearer_token(request.headers()) else {
        return session_error_response(
            &request,
            StatusCode::UNAUTHORIZED,
            "agent_auth_required",
            "an agent credential is required",
        );
    };
    match authenticate_agent_credential(state.db(), &state.auth().pepper, token).await {
        Ok(Some(auth)) => {
            request
                .extensions_mut()
                .insert(crate::enrollment::AgentAuthInfo {
                    agent_id: auth.agent_id,
                    credential_id: auth.credential_id,
                });
            next.run(request).await
        }
        Ok(None) => session_error_response(
            &request,
            StatusCode::UNAUTHORIZED,
            "agent_auth_required",
            "invalid agent credential",
        ),
        Err(_) => session_error_response(
            &request,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

fn session_error_response(
    request: &Request,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| &*id.0)
        .unwrap_or("unknown");
    (status, Json(ApiErrorBody::new(code, message, request_id))).into_response()
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
    use std::path::Path;

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::header;
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::auth::{create_owner, hash_password};
    use crate::database::{ServerDatabaseConfig, initialize};
    use crate::secrets::{create_pepper_file, load_pepper_file};

    use super::*;

    const INDEX_HTML: &str = "<!doctype html><title>PlatPulse</title>";

    /// Test state with a development-mode auth policy (no Secure cookie) so
    /// plain HTTP test requests behave like the e2e server.
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
        let state = AppState::new(
            database,
            Some(web_dir.path().to_path_buf()),
            test_auth(db_dir.path()),
        );
        (db_dir, web_dir, state)
    }

    fn test_auth(dir: &Path) -> AuthConfig {
        let pepper_path = dir.join("server-pepper");
        create_pepper_file(&pepper_path).unwrap();
        AuthConfig::development(
            load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        )
    }

    /// Create the first owner directly through the identity seam.
    async fn seed_owner(state: &AppState) {
        let hash = hash_password(b"correct horse battery").unwrap();
        create_owner(state.db(), "admin", &hash).await.unwrap();
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

    const LOGIN_BODY: &str = r#"{"username":"admin","password":"correct horse battery"}"#;

    fn login_request() -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/api/public/v1/login")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "http://127.0.0.1:8080")
            .body(Body::from(LOGIN_BODY))
            .unwrap()
    }

    async fn login_cookie(app: Router) -> String {
        let response = app.oneshot(login_request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn every_response_carries_the_content_security_policy() {
        let (_, _, state) = test_state().await;
        for uri in ["/health/live", "/api/public/v1/login", "/"] {
            let response = get(build_app(state.clone()), uri).await;
            let policy = response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .expect("CSP header must be present")
                .to_str()
                .unwrap();
            assert!(policy.contains("script-src 'self'"), "{uri}: {policy}");
            assert!(
                !policy.contains("unsafe-inline")
                    || policy.contains("style-src 'self' 'unsafe-inline'"),
                "{uri}: inline script must never be allowed"
            );
        }
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
    async fn shutdown_marks_readiness_false_and_rejects_new_ingestion() {
        let (_, _, state) = test_state_with_files(&[
            ("index.html", INDEX_HTML.as_bytes()),
            ("assets/index-abc123.js", b"// app"),
        ])
        .await;
        seed_owner(&state).await;
        state.begin_shutdown();
        let (status, value) = json(get(build_app(state.clone()), "/health/ready").await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(component(&value, "shutdown")["reason"], "shutting_down");
        assert!(state.ingestion_guard().is_none());
    }
    #[tokio::test]
    async fn ready_reports_setup_required_without_owner() {
        let (_, _, state) = test_state_with_files(&[
            ("index.html", INDEX_HTML.as_bytes()),
            ("assets/index-abc123.js", b"// app"),
        ])
        .await;

        let (status, value) = json(get(build_app(state), "/health/ready").await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(value["status"], "not_ready");
        assert_eq!(component(&value, "owner")["status"], "not_ready");
        assert_eq!(component(&value, "owner")["reason"], "setup_required");
    }

    #[tokio::test]
    async fn ready_reports_ready_with_owner_and_web_assets() {
        let (_, web_dir, state) = test_state_with_files(&[
            ("index.html", INDEX_HTML.as_bytes()),
            ("assets/index-abc123.js", b"// app"),
        ])
        .await;
        let _ = web_dir;
        seed_owner(&state).await;

        let (status, value) = json(get(build_app(state), "/health/ready").await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["status"], "ready");
        assert_eq!(component(&value, "sqlite")["status"], "ready");
        assert_eq!(component(&value, "owner")["status"], "ready");
        assert_eq!(component(&value, "web_assets")["status"], "ready");
    }

    #[tokio::test]
    async fn ready_reports_critical_worker_stale_and_recovers() {
        let (_, web_dir, state) = test_state_with_files(&[
            ("index.html", INDEX_HTML.as_bytes()),
            ("assets/index-abc123.js", b"// app"),
        ])
        .await;
        let _ = web_dir;
        seed_owner(&state).await;
        state
            .runtime
            .critical_worker_heartbeat_ms
            .store(1, Ordering::Release);
        state.runtime.fail_critical_worker();
        let (status, value) = json(get(build_app(state.clone()), "/health/ready").await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            component(&value, "critical_workers")["reason"],
            "worker_unhealthy"
        );

        state.runtime.recover_critical_worker();
        let (status, value) = json(get(build_app(state), "/health/ready").await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(component(&value, "critical_workers")["status"], "ready");
    }

    #[tokio::test]
    async fn ready_reports_web_assets_missing_for_an_incomplete_build() {
        let (_, _, state) = test_state_with_files(&[("index.html", INDEX_HTML.as_bytes())]).await;
        seed_owner(&state).await;

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
        let state = AppState::new(database, None, test_auth(db_dir.path()));
        seed_owner(&state).await;

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

    #[test]
    fn proxy_forwarding_requires_matching_peer_and_https_scheme() {
        let policy = ProxyTrustPolicy {
            trusted_proxy_cidrs: vec!["10.0.0.0/8".parse().unwrap()],
            trusted_proxy_scheme: Some("https".to_owned()),
        };
        assert!(peer_is_trusted(&policy, Some("10.1.2.3".parse().unwrap())));
        assert!(!peer_is_trusted(
            &policy,
            Some("192.0.2.1".parse().unwrap())
        ));
        assert!(!peer_is_trusted(
            &ProxyTrustPolicy {
                trusted_proxy_cidrs: policy.trusted_proxy_cidrs.clone(),
                trusted_proxy_scheme: Some("http".to_owned()),
            },
            Some("10.1.2.3".parse().unwrap())
        ));

        let mut headers = HeaderMap::new();
        headers.insert(
            "forwarded",
            HeaderValue::from_static("for=10.1.2.3;proto=https"),
        );
        assert_eq!(forwarded_proto(&headers), Some(Ok("https")));
        headers.insert("forwarded", HeaderValue::from_static("proto=http"));
        assert_eq!(forwarded_proto(&headers), Some(Ok("http")));
        assert!(matches!(
            evaluate_proxy_request(&policy, Some("192.0.2.1".parse().unwrap()), &headers,),
            Err(ProxyTrustError::UntrustedHeaders)
        ));
        headers.insert("forwarded", HeaderValue::from_static("proto=https"));
        assert!(matches!(
            evaluate_proxy_request(&policy, Some("10.1.2.3".parse().unwrap()), &headers),
            Ok(true)
        ));
        headers.insert("forwarded", HeaderValue::from_static("proto=http"));
        assert!(matches!(
            evaluate_proxy_request(&policy, Some("10.1.2.3".parse().unwrap()), &headers),
            Err(ProxyTrustError::HttpsRequired)
        ));
    }

    #[test]
    fn forwarded_proto_rejects_conflicting_chain_values() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "forwarded",
            HeaderValue::from_static("proto=https, proto=http"),
        );
        assert_eq!(forwarded_proto(&headers), Some(Err(())));
        headers.clear();
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https,http"));
        assert_eq!(forwarded_proto(&headers), Some(Err(())));
    }
    #[tokio::test]
    async fn ready_reports_web_assets_missing_when_index_html_is_absent() {
        let (_, _, state) = test_state_with_files(&[("assets/app-123.js", b"// asset")]).await;
        seed_owner(&state).await;

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
            let app = build_app(state.clone());

            if group == "agent" {
                // Without an owner the agent group refuses everything.
                let (status, body) = json(get(app, uri).await).await;
                assert_eq!(
                    status,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "{uri} without owner"
                );
                assert_eq!(body["error"]["code"], "setup_required");
                continue;
            }

            // Public/Admin 404s require a session (Guest disabled).
            let response = get(app.clone(), uri).await;
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{uri} without session"
            );

            seed_owner(&state).await;
            let cookie = login_cookie(app.clone()).await;
            let request = Request::builder()
                .uri(uri)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap();
            let response = app.oneshot(request).await.unwrap();

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
        let state = AppState::new(database, None, test_auth(db_dir.path()));
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

    #[tokio::test]
    async fn login_requires_the_configured_origin() {
        let (_, _, state) = test_state().await;
        seed_owner(&state).await;
        let request = Request::builder()
            .method("POST")
            .uri("/api/public/v1/login")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "https://evil.example.com")
            .body(Body::from(LOGIN_BODY))
            .unwrap();

        let (status, body) = json(build_app(state).oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "origin_validation_failed");
    }

    #[tokio::test]
    async fn login_is_refused_until_an_owner_exists() {
        let (_, _, state) = test_state().await;
        let (status, body) = json(build_app(state).oneshot(login_request()).await.unwrap()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "setup_required");
    }

    #[tokio::test]
    async fn login_sets_the_production_cookie_and_returns_a_session() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;

        let response = app.oneshot(login_request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        // Development test policy: separate cookie name, no Secure.
        assert!(
            set_cookie.starts_with("platpulse_dev_session="),
            "dev cookie: {set_cookie}"
        );
        assert!(!set_cookie.contains("Secure"));
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Lax"));
        assert!(set_cookie.contains("Path=/"));

        let (_, body) = json(response).await;
        assert_eq!(body["session"]["username"], "admin");
        assert_eq!(body["session"]["role"], "owner");
        assert!(!body["csrfToken"].as_str().unwrap().is_empty());
        assert!(
            body["session"]["createdAt"]
                .as_str()
                .unwrap()
                .ends_with('Z')
        );
    }

    #[tokio::test]
    async fn production_policy_emits_host_secure_cookie() {
        let (db_dir, _, _) = test_state().await;
        let database = initialize(ServerDatabaseConfig::new(db_dir.path().join("server.db")))
            .await
            .unwrap();
        let state = AppState::new(
            database,
            None,
            AuthConfig::production(
                load_pepper_file(&db_dir.path().join("server-pepper")).unwrap(),
                "http://127.0.0.1:8080".to_owned(),
            ),
        );
        let app = build_app(state.clone());
        seed_owner(&state).await;

        let response = app.oneshot(login_request()).await.unwrap();
        let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(
            set_cookie.starts_with("__Host-platpulse_session="),
            "production cookie: {set_cookie}"
        );
        assert!(set_cookie.contains("Secure"));
        assert!(!set_cookie.contains("Domain="));
    }

    #[tokio::test]
    async fn login_rate_limit_is_independent_and_blocks_after_failures() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;

        for _ in 0..LOGIN_MAX_ATTEMPTS {
            let request = Request::builder()
                .method("POST")
                .uri("/api/public/v1/login")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, "http://127.0.0.1:8080")
                .body(Body::from(
                    r#"{"username":"admin","password":"wrong password"}"#,
                ))
                .unwrap();
            let status = app.clone().oneshot(request).await.unwrap().status();
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        // The sixth attempt is blocked even with the correct password.
        let response = app.oneshot(login_request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let (_, body) = json(response).await;
        assert_eq!(body["error"]["code"], "login_rate_limited");
    }

    #[tokio::test]
    async fn session_endpoint_requires_a_valid_cookie() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;

        // No cookie.
        let (status, body) = json(get(app.clone(), "/api/public/v1/session").await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "auth_required");

        // Login, then use the cookie.
        let response = app.clone().oneshot(login_request()).await.unwrap();
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        let request = Request::builder()
            .uri("/api/public/v1/session")
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap();
        let (status, body) = json(app.oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["session"]["username"], "admin");
        assert!(!body["csrfToken"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn login_rotates_the_session_id() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;

        let first = app.clone().oneshot(login_request()).await.unwrap();
        let first_cookie = first.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();

        // Log in again presenting the old cookie: the old session is
        // revoked and a new cookie is issued.
        let request = Request::builder()
            .method("POST")
            .uri("/api/public/v1/login")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "http://127.0.0.1:8080")
            .header(header::COOKIE, &first_cookie)
            .body(Body::from(LOGIN_BODY))
            .unwrap();
        let second = app.clone().oneshot(request).await.unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second_cookie = second.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        assert_ne!(first_cookie, second_cookie);

        let stale = Request::builder()
            .uri("/api/public/v1/session")
            .header(header::COOKIE, &first_cookie)
            .body(Body::empty())
            .unwrap();
        let (status, _) = json(app.clone().oneshot(stale).await.unwrap()).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "rotated session must be revoked"
        );

        let fresh = Request::builder()
            .uri("/api/public/v1/session")
            .header(header::COOKIE, &second_cookie)
            .body(Body::empty())
            .unwrap();
        let (status, _) = json(app.oneshot(fresh).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn logout_revokes_the_current_session() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;

        let login = app.clone().oneshot(login_request()).await.unwrap();
        let cookie = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();

        let request = Request::builder()
            .method("POST")
            .uri("/api/public/v1/logout")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let cleared = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(
            cleared.contains("Max-Age=0"),
            "logout must clear the cookie"
        );

        let stale = Request::builder()
            .uri("/api/public/v1/session")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let (status, _) = json(app.oneshot(stale).await.unwrap()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_group_requires_an_owner_session() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;

        // Anonymous: 401, not 404.
        let (status, body) = json(get(app.clone(), "/api/admin/v1/missing").await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "auth_required");

        // Viewer session: 403 owner_required.
        crate::auth::create_viewer(
            state.db(),
            "viewer1",
            &hash_password(b"correct horse battery").unwrap(),
        )
        .await
        .unwrap();
        let viewer_login = Request::builder()
            .method("POST")
            .uri("/api/public/v1/login")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "http://127.0.0.1:8080")
            .body(Body::from(
                r#"{"username":"viewer1","password":"correct horse battery"}"#,
            ))
            .unwrap();
        let response = app.clone().oneshot(viewer_login).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let viewer_cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        let request = Request::builder()
            .uri("/api/admin/v1/missing")
            .header(header::COOKIE, &viewer_cookie)
            .body(Body::empty())
            .unwrap();
        let (status, body) = json(app.clone().oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "owner_required");

        // Owner session passes the guard (404 from the empty route group).
        let login = app.clone().oneshot(login_request()).await.unwrap();
        let owner_cookie = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        let request = Request::builder()
            .uri("/api/admin/v1/missing")
            .header(header::COOKIE, owner_cookie)
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[ROUTE_GROUP_HEADER], "admin");
    }

    #[tokio::test]
    async fn agent_group_is_blocked_until_an_owner_exists() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());

        let (status, body) = json(get(app.clone(), "/api/agent/v1/enroll").await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "setup_required");

        seed_owner(&state).await;
        // After setup the group opens, but every request still needs an
        // Enrollment Token (POST /enroll) or an Agent Credential; a
        // bare request is refused before routing, never answered 404.
        let (status, body) = json(get(app, "/api/agent/v1/enroll").await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "agent_auth_required");
    }

    fn bearer_request(uri: &str, token: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    async fn issue_enrollment_token(state: &AppState) -> String {
        crate::enrollment::create_enrollment_token(
            state.db(),
            &state.auth().pepper,
            crate::enrollment::ENROLLMENT_TOKEN_DEFAULT_LIFETIME,
        )
        .await
        .unwrap()
        .1
    }

    #[tokio::test]
    async fn enrollment_issues_one_identity_and_consumes_the_token() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;
        let token = issue_enrollment_token(&state).await;

        let response = app
            .clone()
            .oneshot(bearer_request("/api/agent/v1/enroll", &token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[ROUTE_GROUP_HEADER], "agent");
        let (_, body) = json(response).await;
        let agent_id = body["agent_id"].as_str().unwrap();
        assert_eq!(body["agent_epoch"], 1);
        assert_eq!(body["protocol_version"], 1);
        let credential = body["credential"].as_str().unwrap();
        assert!(credential.starts_with("pp_agent_"));
        assert_eq!(credential.len(), "pp_agent_".len() + 36 + 1 + 64);

        // The same token cannot enroll twice and never mints a second
        // identity.
        let (status, body) = json(
            app.oneshot(bearer_request("/api/agent/v1/enroll", &token))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "enrollment_token_consumed");
        let agents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(agents, 1);
        let _ = agent_id;
    }

    #[tokio::test]
    async fn enrollment_rejects_missing_invalid_and_expired_tokens() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;

        // No Bearer token at all.
        let request = Request::builder()
            .method("POST")
            .uri("/api/agent/v1/enroll")
            .body(Body::empty())
            .unwrap();
        let (status, body) = json(app.clone().oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "enrollment_token_invalid");

        // Unknown token.
        let (status, body) = json(
            app.clone()
                .oneshot(bearer_request(
                    "/api/agent/v1/enroll",
                    "pp_enroll_unknown_abc",
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "enrollment_token_invalid");

        // Expired token (inserted directly with a past expiry).
        let (token_id, full_token) = crate::enrollment::new_enrollment_token();
        let digest = state.auth().pepper.hmac_digest(full_token.as_bytes());
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query(
            "INSERT INTO enrollment_tokens (token_id, token_digest, created_at, expires_at, consumed_at, consumed_agent_id, revoked_at) VALUES (?, ?, ?, '2020-01-01T00:00:00Z', NULL, NULL, NULL)",
        )
        .bind(&token_id)
        .bind(digest.to_vec())
        .bind(&now)
        .execute(state.db().pool())
        .await
        .unwrap();
        let (status, body) = json(
            app.oneshot(bearer_request("/api/agent/v1/enroll", &full_token))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "enrollment_token_expired");

        let agents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(agents, 0, "failed enrollments must not mint identities");
    }

    #[tokio::test]
    async fn enrollment_tokens_cannot_submit_reports_or_reach_human_apis() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;
        let token = issue_enrollment_token(&state).await;

        // The Enrollment Token is not an Agent Credential: it cannot reach
        // report routes (or any other Agent route).
        let (status, body) = json(
            app.clone()
                .oneshot(bearer_request("/api/agent/v1/reports", &token))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "agent_auth_required");

        // The Enrollment Token cannot reach human-facing APIs either.
        let (status, body) = json(
            app.clone()
                .oneshot(bearer_request("/api/public/v1/session", &token))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "auth_required");

        let (status, body) = json(
            app.oneshot(bearer_request("/api/admin/v1/missing", &token))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "auth_required");
    }

    #[tokio::test]
    async fn agent_credentials_reach_agent_routes_but_not_human_apis() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;
        let token = issue_enrollment_token(&state).await;
        let response = app
            .clone()
            .oneshot(bearer_request("/api/agent/v1/enroll", &token))
            .await
            .unwrap();
        let (_, body) = json(response).await;
        let credential = body["credential"].as_str().unwrap().to_owned();

        // The Agent Credential passes the Agent guard (404 from the
        // currently empty route group means the guard accepted it).
        let request = Request::builder()
            .method("GET")
            .uri("/api/agent/v1/missing")
            .header(header::AUTHORIZATION, format!("Bearer {credential}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[ROUTE_GROUP_HEADER], "agent");

        // The Agent Credential cannot access Human/Public/Admin routes:
        // those require a session cookie, and the credential is not one.
        for uri in ["/api/public/v1/session", "/api/admin/v1/missing"] {
            let request = Request::builder()
                .method("GET")
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {credential}"))
                .body(Body::empty())
                .unwrap();
            let (status, body) = json(app.clone().oneshot(request).await.unwrap()).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
            assert_eq!(body["error"]["code"], "auth_required", "{uri}");
        }
    }

    #[tokio::test]
    async fn human_sessions_cannot_enroll_or_report() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;
        let cookie = login_cookie(app.clone()).await;

        // A logged-in Owner session cannot enroll: the Agent API only
        // accepts the one-time Enrollment Token, never a cookie.
        let request = Request::builder()
            .method("POST")
            .uri("/api/agent/v1/enroll")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let (status, body) = json(app.clone().oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "enrollment_token_invalid");

        // A Human Session cannot submit reports either: the Agent guard
        // demands an Agent Credential Bearer token.
        let request = Request::builder()
            .method("POST")
            .uri("/api/agent/v1/reports")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let (status, body) = json(app.oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "agent_auth_required");
    }

    #[tokio::test]
    async fn enrollment_rate_limit_is_independent_and_blocks_after_failures() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;

        for _ in 0..crate::enrollment::ENROLL_MAX_ATTEMPTS {
            let (status, _) = json(
                app.clone()
                    .oneshot(bearer_request(
                        "/api/agent/v1/enroll",
                        "pp_enroll_unknown_abc",
                    ))
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        let (status, body) = json(
            app.oneshot(bearer_request(
                "/api/agent/v1/enroll",
                "pp_enroll_unknown_abc",
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["error"]["code"], "enrollment_rate_limited");
    }

    #[tokio::test]
    async fn enrollment_error_bodies_never_echo_the_token() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;
        let token = issue_enrollment_token(&state).await;

        for request in [
            bearer_request("/api/agent/v1/enroll", &token),
            bearer_request("/api/agent/v1/reports", &token),
        ] {
            let response = app.clone().oneshot(request).await.unwrap();
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(
                !text.contains(&token),
                "error body must never echo the presented token"
            );
        }
    }

    #[tokio::test]
    async fn viewer_session_reaches_public_home_but_never_admin() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;
        crate::auth::create_viewer(
            state.db(),
            "viewer1",
            &hash_password(b"correct horse battery").unwrap(),
        )
        .await
        .unwrap();

        // Viewer login through the real HTTP flow.
        let login = Request::builder()
            .method("POST")
            .uri("/api/public/v1/login")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "http://127.0.0.1:8080")
            .body(Body::from(
                r#"{"username":"viewer1","password":"correct horse battery"}"#,
            ))
            .unwrap();
        let response = app.clone().oneshot(login).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();

        // The Viewer session may use the private Home/Public group.
        let request = Request::builder()
            .uri("/api/public/v1/session")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let (status, body) = json(app.clone().oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["session"]["role"], "viewer");

        // The Admin group answers every Viewer request with the same
        // stable forbidden envelope, including unknown routes (the role
        // guard runs before routing).
        for uri in [
            "/api/admin/v1/missing",
            "/api/admin/v1/session",
            "/api/admin/v1/sessions",
        ] {
            let request = Request::builder()
                .uri(uri)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap();
            let (status, body) = json(app.clone().oneshot(request).await.unwrap()).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
            assert_eq!(body["error"]["code"], "owner_required", "{uri}");
        }
    }

    #[tokio::test]
    async fn session_tokens_never_leave_the_cookie() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;
        crate::auth::create_viewer(
            state.db(),
            "viewer1",
            &hash_password(b"correct horse battery").unwrap(),
        )
        .await
        .unwrap();

        // The session token lives only in the cookie (design §12.3); the
        // Server has no logging layer, and no code path ever places a
        // token in a URL. The assertions below pin the remaining surface:
        // JSON bodies — login, 401/404 envelopes, and the Viewer 403.
        let login = app.clone().oneshot(login_request()).await.unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let cookie = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        let token = cookie.split('=').nth(1).unwrap().split(';').next().unwrap();
        assert!(!token.is_empty());

        // The login response body carries the session projection and CSRF
        // token, never the session token itself.
        let (_, body) = json(login).await;
        let body_text = serde_json::to_string(&body).unwrap();
        assert!(!body_text.contains(token), "login body leaked the token");

        // Error envelopes (401 without a cookie, 404 with an Owner
        // session) must not echo the presented token either.
        for (uri, cookie_opt) in [
            ("/api/public/v1/missing", Some(&cookie)),
            ("/api/admin/v1/missing", Some(&cookie)),
            ("/api/public/v1/session", None),
        ] {
            let mut builder = Request::builder().uri(uri);
            if let Some(value) = cookie_opt {
                builder = builder.header(header::COOKIE, value);
            }
            let response = app
                .clone()
                .oneshot(builder.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(!text.contains(token), "{uri} error body leaked the token");
        }

        // The Viewer `owner_required` 403 body must not echo the Viewer
        // session token either.
        let viewer_login = Request::builder()
            .method("POST")
            .uri("/api/public/v1/login")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "http://127.0.0.1:8080")
            .body(Body::from(
                r#"{"username":"viewer1","password":"correct horse battery"}"#,
            ))
            .unwrap();
        let response = app.clone().oneshot(viewer_login).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let viewer_cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        let viewer_token = viewer_cookie
            .split('=')
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let request = Request::builder()
            .uri("/api/admin/v1/missing")
            .header(header::COOKIE, &viewer_cookie)
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            !text.contains(viewer_token),
            "owner_required body leaked the token"
        );
    }

    #[tokio::test]
    async fn public_reads_require_a_session_when_guest_is_disabled() {
        let (_, _, state) = test_state().await;
        let app = build_app(state.clone());
        seed_owner(&state).await;

        let (status, body) = json(get(app, "/api/public/v1/events").await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "auth_required");
        assert_eq!(body["error"]["message"], json!("authentication required"));
    }
}
