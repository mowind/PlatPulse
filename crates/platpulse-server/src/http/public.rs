//! `/api/public/v1` route group — the Home Public Projection and human
//! session lifecycle (design §12.3/§12.4, §13.1).
//!
//! Middleware and DTO namespace are independent from Admin and Agent: DTOs
//! live in this module and are never reused as Admin DTOs by runtime field
//! filtering (design §13.1). Guest access is disabled by default, so every
//! route except `POST /login` requires a valid human Session; the session
//! guard itself lives in `super` and is attached in `build_app`.

use axum::extract::{Extension, Path, Query, Request, State};
use axum::http::header::{self, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tokio_stream::Stream;

use crate::http::realtime::{keepalive_interval, parse_last_event_id};

use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{
    LoginError, clear_cookie_header, format_rfc3339, login, session_cookie_header, touch_session,
    write_audit_event,
};
use crate::http::{
    AppState, AuthenticatedSession, ClientIp, ROUTE_GROUP_HEADER, RequestId, api_not_found,
};

async fn public_events(
    State(state): State<AppState>,
    Extension(_session): Extension<AuthenticatedSession>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let cursor = parse_last_event_id(headers.get("last-event-id").and_then(|v| v.to_str().ok()));
    Sse::new(state.public_realtime().stream(cursor)).keep_alive(
        KeepAlive::new()
            .interval(keepalive_interval())
            .text("keepalive"),
    )
}
async fn group_middleware(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(ROUTE_GROUP_HEADER, HeaderValue::from_static("public"));
    response
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Non-sensitive session projection plus the synchronizer CSRF token
/// (design §12.3/§12.4). Fields are camelCase per the browser wire rule
/// (§13.3); timestamps are RFC 3339 UTC.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionResponse {
    session: SessionProjection,
    csrf_token: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionProjection {
    user_id: String,
    username: String,
    role: String,
    created_at: String,
    last_seen_at: String,
    expires_at: String,
}

fn project(session: &crate::auth::SessionInfo) -> SessionProjection {
    SessionProjection {
        user_id: session.user_id.clone(),
        username: session.username.clone(),
        role: session.role.clone(),
        created_at: format_rfc3339(session.created_at),
        last_seen_at: format_rfc3339(session.last_seen_at),
        expires_at: format_rfc3339(session.expires_at),
    }
}

fn session_response(session: &crate::auth::SessionInfo) -> SessionResponse {
    SessionResponse {
        session: project(session),
        csrf_token: session.csrf_token.clone(),
    }
}

/// Login with strict configured-Origin validation, an independent rate
/// limit, and Session ID rotation (design §12.2–§12.4).
#[utoipa::path(
    post,
    path = "/api/public/v1/login",
    tag = "public",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Logged in; a session cookie is set", body = SessionResponse),
        (status = 401, description = "Invalid credentials", body = crate::http::ApiErrorBody),
        (status = 403, description = "Origin validation failed or the user is disabled", body = crate::http::ApiErrorBody),
        (status = 429, description = "Too many login attempts", body = crate::http::ApiErrorBody),
        (status = 503, description = "Server setup is incomplete", body = crate::http::ApiErrorBody),
    )
)]
pub(crate) async fn login_handler(
    State(state): State<AppState>,
    headers: header::HeaderMap,
    Extension(request_id): Extension<RequestId>,
    Extension(client): Extension<ClientIp>,
    Json(body): Json<LoginRequest>,
) -> Response {
    // 1. Strict Origin validation (design §12.4): login carries no existing
    //    session, so the configured origin is the only acceptable one.
    if !state.auth().origin_matches(headers.get(header::ORIGIN)) {
        return error_response(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "origin_validation_failed",
            "request origin does not match the configured origin",
        );
    }

    // 2. Setup gate (design §12.2): no Owner, no login.
    match crate::auth::has_owner(state.db()).await {
        Ok(true) => {}
        Ok(false) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "setup_required",
                "server setup is incomplete; create the first owner",
            );
        }
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    }

    // 3. Cheap input bounds before any credential work: oversized or empty
    //    fields can never match a stored hash, so reject them without
    //    touching Argon2id or the rate limiter.
    if body.username.is_empty()
        || body.username.len() > 64
        || body.password.is_empty()
        || body.password.len() > crate::auth::MAX_PASSWORD_LENGTH
    {
        return error_response(
            &request_id.0,
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "invalid username or password",
        );
    }

    // 4. Independent login rate limit (design §19.4).
    let limiter_key = (client.0.as_str(), body.username.as_str());
    if state.login_limiter().is_blocked(limiter_key) {
        return error_response(
            &request_id.0,
            StatusCode::TOO_MANY_REQUESTS,
            "login_rate_limited",
            "too many login attempts; try again later",
        );
    }

    // 5. Credentials; a presented valid session is rotated on success.
    let presented =
        crate::auth::cookie_value(&headers, &state.auth().cookie_name).map(str::to_owned);
    match login(
        state.db(),
        state.auth(),
        &body.username,
        &body.password,
        presented.as_deref(),
    )
    .await
    {
        Ok((session, full_token)) => {
            state.login_limiter().record_success(limiter_key);
            let cookie = session_cookie_header(
                &state.auth().cookie_name,
                &full_token,
                state.auth().cookie_secure,
            );
            (
                StatusCode::OK,
                [(
                    header::SET_COOKIE,
                    HeaderValue::from_str(&cookie).expect("cookie value is a valid header"),
                )],
                Json(session_response(&session)),
            )
                .into_response()
        }
        Err(LoginError::InvalidCredentials) => {
            state.login_limiter().record_failure(limiter_key);
            let after = serde_json::json!({ "username": body.username });
            write_audit_event(
                state.db(),
                None,
                "login_failed",
                "user",
                &body.username,
                Some(&after),
            )
            .await
            .ok();
            error_response(
                &request_id.0,
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "invalid username or password",
            )
        }
        Err(LoginError::UserDisabled) => error_response(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "user_disabled",
            "this user is disabled",
        ),
        Err(LoginError::Database(_)) => error_response(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Logout: revoke the current Session and clear the cookie (design §12.3).
/// Revocation and its audit row commit in one transaction; a database
/// failure keeps the session valid and returns 500 instead of pretending
/// the user signed out.
#[utoipa::path(
    post,
    path = "/api/public/v1/logout",
    tag = "public",
    responses(
        (status = 204, description = "The current session is revoked and its cookie cleared"),
        (status = 401, description = "No valid session", body = crate::http::ApiErrorBody),
        (status = 500, description = "The session could not be revoked", body = crate::http::ApiErrorBody),
    )
)]
pub(crate) async fn logout_handler(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let session = principal.0;
    // Revocation and its audit row commit in one transaction; on failure
    // the response fails loudly and the cookie is kept so the client does
    // not believe it signed out while the session is still valid.
    let result = async {
        let mut transaction = state.db().pool().begin().await?;
        let revoked = sqlx::query(
            "UPDATE sessions SET revoked_at = ? WHERE session_id = ? AND revoked_at IS NULL",
        )
        .bind(crate::auth::format_rfc3339(crate::auth::now_utc()))
        .bind(&session.session_id)
        .execute(&mut *transaction)
        .await?;
        if revoked.rows_affected() == 0 {
            // The session disappeared between the guard and here.
            return Err(sqlx::Error::RowNotFound);
        }
        crate::auth::insert_audit_event(
            &mut *transaction,
            Some(&session.user_id),
            "session_revoked",
            "session",
            &session.session_id,
            None,
        )
        .await?;
        transaction.commit().await
    }
    .await;

    if result.is_err() {
        return error_response(
            &request_id.0,
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_revocation_failed",
            "could not revoke the session; try again",
        );
    }

    let cookie = clear_cookie_header(&state.auth().cookie_name, state.auth().cookie_secure);
    (
        StatusCode::NO_CONTENT,
        [(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie).expect("cookie value is a valid header"),
        )],
    )
        .into_response()
}

/// Current session projection and CSRF token for the WebUI (design §12.4).
/// `last_seen_at` is refreshed at most once per throttle window.
#[utoipa::path(
    get,
    path = "/api/public/v1/session",
    tag = "public",
    responses(
        (status = 200, description = "The current session and CSRF token", body = SessionResponse),
        (status = 401, description = "No valid session", body = crate::http::ApiErrorBody),
    )
)]
pub(crate) async fn session_handler(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthenticatedSession>,
) -> Response {
    let session = principal.0;
    touch_session(state.db(), &session.session_id, session.last_seen_at)
        .await
        .ok();
    Json(session_response(&session)).into_response()
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicNetwork {
    pub network_key: String,
    pub display_name: String,
    pub nodes: Vec<PublicNode>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicBlockHistoryItem {
    pub node_id: String,
    pub height: Option<i64>,
    pub block_time_ms: Option<i64>,
    pub transaction_count: Option<i64>,
    pub observed_at: Option<String>,
    pub freshness: Option<String>,
    pub gap_from_height: Option<i64>,
    pub gap_to_height: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/public/v1/nodes/{node_id}/history",
    tag = "public",
    params(("node_id" = String, Path), ("limit" = Option<i64>, Query)),
    responses((status = 200, body = [PublicBlockHistoryItem]))
)]
pub(crate) async fn public_node_history(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Query(params): Query<HistoryQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let rows = sqlx::query_as::<_, (Option<i64>, Option<i64>, Option<i64>, Option<String>, Option<i64>, Option<i64>)>("SELECT block_number, block_timestamp_ms, transaction_count, observed_at, from_height, to_height FROM (SELECT block_number, block_timestamp_ms, transaction_count, observed_at, NULL AS from_height, NULL AS to_height, node_id FROM block_summaries WHERE node_id = ? AND EXISTS (SELECT 1 FROM nodes WHERE node_id = block_summaries.node_id AND visibility = 'public' AND lifecycle = 'active') UNION ALL SELECT NULL, NULL, NULL, created_at, from_height, to_height, node_id FROM block_history_gaps WHERE node_id = ? AND EXISTS (SELECT 1 FROM nodes WHERE node_id = block_history_gaps.node_id AND visibility = 'public' AND lifecycle = 'active')) ORDER BY COALESCE(block_number, from_height) DESC LIMIT ?")
        .bind(&node_id).bind(&node_id).bind(limit).fetch_all(state.db().pool()).await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(
                    |(
                        height,
                        block_time_ms,
                        transaction_count,
                        observed_at,
                        from_height,
                        to_height,
                    )| PublicBlockHistoryItem {
                        node_id: node_id.clone(),
                        height,
                        block_time_ms,
                        transaction_count,
                        freshness: observed_at.clone(),
                        observed_at,
                        gap_from_height: from_height,
                        gap_to_height: to_height,
                    },
                )
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => error_response(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicNode {
    pub node_id: String,
    pub display_name: Option<String>,
    pub network_key: String,
    pub health: String,
    pub health_reason: String,
    pub freshness: Option<String>,
    pub rpc_state: String,
    pub sync_state: String,
    pub consensus_state: String,
    pub process_state: String,
    pub host_cpu_percent: Option<f64>,
}

#[derive(Debug, sqlx::FromRow)]
struct PublicNodeRow {
    node_id: String,
    display_name: Option<String>,
    network_key: String,
    network_display_name: String,
    lifecycle: String,
    rpc_state: Option<String>,
    identity_state: Option<String>,
    updated_at: Option<String>,
    host_cpu_percent: Option<f64>,
    sync_state: Option<String>,
    consensus_state: Option<String>,
    process_state: Option<String>,
    sync_received_at: Option<String>,
    consensus_received_at: Option<String>,
}

const FRESHNESS_LIMIT_SECONDS: i64 = 120;

fn fresh(timestamp: Option<&str>, now: time::OffsetDateTime) -> bool {
    timestamp
        .and_then(crate::auth::parse_rfc3339)
        .is_some_and(|observed| (now - observed).whole_seconds().abs() <= FRESHNESS_LIMIT_SECONDS)
}

#[allow(clippy::too_many_arguments)]
fn health_for(
    lifecycle: &str,
    rpc: Option<&str>,
    identity: Option<&str>,
    sync: Option<&str>,
    consensus: Option<&str>,
    process: Option<&str>,
    received_at: Option<&str>,
    sync_received_at: Option<&str>,
    consensus_received_at: Option<&str>,
) -> (&'static str, &'static str) {
    if lifecycle == "retired" {
        return ("unknown", "node is retired");
    }
    if matches!(process, Some("error")) {
        return ("unhealthy", "process observation failed");
    }
    if matches!(process, Some("stopped")) {
        return ("unhealthy", "selected process is stopped");
    }
    if matches!(process, Some("unknown")) {
        return ("unknown", "process observation is unknown");
    }
    if matches!(process, Some("disabled")) {
        // Process monitoring is optional and does not by itself make chain
        // health unknown.
    }
    if matches!(rpc, Some("error")) {
        return ("unhealthy", "RPC observation failed");
    }
    if matches!(identity, Some("error")) {
        return ("unknown", "network identity mismatch");
    }
    if matches!(sync, Some("error")) {
        return ("unhealthy", "sync observation failed");
    }
    if matches!(consensus, Some("error")) {
        return ("unhealthy", "consensus observation failed");
    }
    let current = crate::auth::now_utc();
    if !fresh(received_at, current)
        || !fresh(sync_received_at, current)
        || !fresh(consensus_received_at, current)
    {
        return ("unknown", "one or more observations are stale or unknown");
    }
    if matches!(rpc, Some("ok")) && matches!(sync, Some("ok")) && matches!(consensus, Some("ok")) {
        ("healthy", "RPC, sync, and consensus are current")
    } else {
        (
            "unknown",
            "one or more observations are unknown or unsupported",
        )
    }
}
fn freshness_for(row: &PublicNodeRow) -> Option<String> {
    let mut timestamps = [
        row.updated_at.as_ref(),
        row.sync_received_at.as_ref(),
        row.consensus_received_at.as_ref(),
    ];
    if timestamps.iter().any(Option::is_none) {
        return None;
    }
    timestamps.sort();
    timestamps.first().and_then(|value| (*value).cloned())
}

fn public_node(row: PublicNodeRow) -> (String, PublicNode) {
    let (health, health_reason) = health_for(
        &row.lifecycle,
        row.rpc_state.as_deref(),
        row.identity_state.as_deref(),
        row.sync_state.as_deref(),
        row.consensus_state.as_deref(),
        row.process_state.as_deref(),
        row.updated_at.as_deref(),
        row.sync_received_at.as_deref(),
        row.consensus_received_at.as_deref(),
    );
    let freshness = freshness_for(&row);
    let node = PublicNode {
        node_id: row.node_id.clone(),
        display_name: row.display_name,
        network_key: row.network_key,
        health: health.to_owned(),
        health_reason: health_reason.to_owned(),
        freshness,
        rpc_state: row.rpc_state.unwrap_or_else(|| "unknown".to_owned()),
        sync_state: row.sync_state.unwrap_or_else(|| "unknown".to_owned()),
        consensus_state: row.consensus_state.unwrap_or_else(|| "unknown".to_owned()),
        process_state: row.process_state.unwrap_or_else(|| "unknown".to_owned()),
        host_cpu_percent: row.host_cpu_percent,
    };
    (row.network_display_name, node)
}

/// Public Home projection. The query boundary only selects public, active
/// Nodes and never returns endpoint, Agent, host identity, capacity, or raw
/// errors from the Admin projection.
#[utoipa::path(
    get,
    path = "/api/public/v1/networks",
    tag = "public",
    responses((status = 200, description = "Published Network and Node projection", body = [PublicNetwork]))
)]
pub(crate) async fn public_networks(State(state): State<AppState>) -> Response {
    let rows = sqlx::query_as::<_, PublicNodeRow>(
        "SELECT n.node_id, n.display_name, n.network_key, r.display_name AS network_display_name,
                n.lifecycle, s.state AS rpc_state, sy.state AS sync_state, co.state AS consensus_state, p.state AS process_state, i.state AS identity_state, s.received_at AS updated_at, sy.received_at AS sync_received_at, co.received_at AS consensus_received_at, h.cpu_percent AS host_cpu_percent
           FROM nodes n
           JOIN networks r ON r.network_key = n.network_key
           LEFT JOIN component_status s ON s.node_id = n.node_id AND s.component_key = 'rpc'
           LEFT JOIN component_status sy ON sy.node_id = n.node_id AND sy.component_key = 'sync'
           LEFT JOIN component_status co ON co.node_id = n.node_id AND co.component_key = 'consensus'
           LEFT JOIN component_status p ON p.node_id = n.node_id AND p.component_key = 'process'
           LEFT JOIN component_status i ON i.node_id = n.node_id AND i.component_key = 'network_identity'
           LEFT JOIN current_host_observations h ON h.agent_id = n.agent_id
          WHERE n.visibility = 'public' AND n.lifecycle = 'active'
          ORDER BY r.network_key, n.node_id",
    )
    .fetch_all(state.db().pool())
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(_) => {
            return error_response(
                "unknown",
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let mut networks: Vec<PublicNetwork> = Vec::new();
    for row in rows {
        let network_key = row.network_key.clone();
        let (network_display_name, node) = public_node(row);
        if let Some(network) = networks
            .iter_mut()
            .find(|network| network.network_key == network_key)
        {
            network.nodes.push(node);
        } else {
            networks.push(PublicNetwork {
                network_key,
                display_name: network_display_name,
                nodes: vec![node],
            });
        }
    }
    Json(networks).into_response()
}

#[utoipa::path(
    get,
    path = "/api/public/v1/networks/{network_key}",
    tag = "public",
    params(("network_key" = String, Path, description = "Registered Network key")),
    responses((status = 200, description = "Published Network projection", body = PublicNetwork), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn public_network(
    State(state): State<AppState>,
    Path(network_key): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let rows = sqlx::query_as::<_, PublicNodeRow>(
        "SELECT n.node_id, n.display_name, n.network_key, r.display_name AS network_display_name,
                n.lifecycle, s.state AS rpc_state, sy.state AS sync_state, co.state AS consensus_state, p.state AS process_state, i.state AS identity_state, s.received_at AS updated_at, sy.received_at AS sync_received_at, co.received_at AS consensus_received_at, h.cpu_percent AS host_cpu_percent
           FROM nodes n JOIN networks r ON r.network_key = n.network_key
           LEFT JOIN component_status s ON s.node_id = n.node_id AND s.component_key = 'rpc'
           LEFT JOIN component_status sy ON sy.node_id = n.node_id AND sy.component_key = 'sync'
           LEFT JOIN component_status co ON co.node_id = n.node_id AND co.component_key = 'consensus'
           LEFT JOIN component_status p ON p.node_id = n.node_id AND p.component_key = 'process'
           LEFT JOIN component_status i ON i.node_id = n.node_id AND i.component_key = 'network_identity'
           LEFT JOIN current_host_observations h ON h.agent_id = n.agent_id
          WHERE n.network_key = ? AND n.visibility = 'public' AND n.lifecycle = 'active'
          ORDER BY n.node_id",
    )
    .bind(&network_key)
    .fetch_all(state.db().pool())
    .await;
    let rows = match rows {
        Ok(rows) if !rows.is_empty() => rows,
        Ok(_) => {
            return error_response(
                &request_id.0,
                StatusCode::NOT_FOUND,
                "not_found",
                "resource not found",
            );
        }
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let display_name = rows[0].network_display_name.clone();
    let nodes = rows.into_iter().map(|row| public_node(row).1).collect();
    Json(PublicNetwork {
        network_key,
        display_name,
        nodes,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/api/public/v1/nodes/{node_id}",
    tag = "public",
    params(("node_id" = String, Path, description = "Published Node ID")),
    responses((status = 200, description = "Published Node projection", body = PublicNode), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn public_node_detail(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let row = sqlx::query_as::<_, PublicNodeRow>(
        "SELECT n.node_id, n.display_name, n.network_key, r.display_name AS network_display_name,
                n.lifecycle, s.state AS rpc_state, sy.state AS sync_state, co.state AS consensus_state, p.state AS process_state, i.state AS identity_state, s.received_at AS updated_at, sy.received_at AS sync_received_at, co.received_at AS consensus_received_at, h.cpu_percent AS host_cpu_percent
           FROM nodes n JOIN networks r ON r.network_key = n.network_key
           LEFT JOIN component_status s ON s.node_id = n.node_id AND s.component_key = 'rpc'
           LEFT JOIN component_status sy ON sy.node_id = n.node_id AND sy.component_key = 'sync'
           LEFT JOIN component_status co ON co.node_id = n.node_id AND co.component_key = 'consensus'
           LEFT JOIN component_status p ON p.node_id = n.node_id AND p.component_key = 'process'
           LEFT JOIN component_status i ON i.node_id = n.node_id AND i.component_key = 'network_identity'
           LEFT JOIN current_host_observations h ON h.agent_id = n.agent_id
          WHERE n.node_id = ? AND n.visibility = 'public' AND n.lifecycle = 'active'",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await;
    match row {
        Ok(Some(row)) => Json(public_node(row).1).into_response(),
        Ok(None) => error_response(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        ),
        Err(_) => error_response(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}
fn error_response(
    request_id: &str,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    (
        status,
        Json(crate::http::ApiErrorBody::new(code, message, request_id)),
    )
        .into_response()
}

pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/login", post(login_handler))
        .route("/logout", post(logout_handler))
        .route("/session", get(session_handler))
        .route("/networks", get(public_networks))
        .route("/networks/{network_key}", get(public_network))
        .route("/nodes/{node_id}", get(public_node_detail))
        .route("/nodes/{node_id}/history", get(public_node_history))
        .route("/events", get(public_events))
        .fallback(api_not_found)
        .layer(axum::middleware::from_fn(group_middleware))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use tempfile::tempdir;

    async fn test_state() -> (tempfile::TempDir, AppState) {
        let dir = tempdir().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        (dir, AppState::new(database, None, auth))
    }

    async fn seed_public_data(state: &AppState) {
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-public-test', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main Network', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        for (node_id, lifecycle, visibility) in [
            ("node-public", "active", "public"),
            ("node-private", "active", "private"),
            ("node-retired", "retired", "public"),
        ] {
            sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, 'agent-public-test', 'mainnet', ?, 'ws://127.0.0.1:1', ?, ?, 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
                .bind(node_id).bind(format!("{node_id} display")).bind(lifecycle).bind(visibility)
                .execute(state.db().pool()).await.unwrap();
        }
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, received_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-public', 'node-public', 'rpc', 'ok', '2026-01-01T00:00:00Z', 1, 1)")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO current_host_observations (agent_id, cpu_percent, updated_at) VALUES ('agent-public-test', 42.5, '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
    }

    #[tokio::test]
    async fn public_projection_filters_private_and_retired_nodes_and_is_allowlisted() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let response = public_networks(State(state.clone())).await;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let node = &value[0]["nodes"][0];
        assert_eq!(value[0]["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(node["nodeId"], "node-public");
        assert_eq!(node["hostCpuPercent"], 42.5);
        for forbidden in [
            "rpcEndpoint",
            "agentId",
            "hostIdentifier",
            "rawError",
            "capacity",
            "credential",
        ] {
            assert!(
                node.get(forbidden).is_none(),
                "public field leaked: {forbidden}"
            );
        }

        let private = public_node_detail(
            State(state.clone()),
            Path("node-private".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(private.status(), StatusCode::NOT_FOUND);
        let retired = public_node_detail(
            State(state),
            Path("node-retired".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(retired.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn stale_sync_dimension_prevents_healthy_public_health() {
        let now = crate::auth::now_utc();
        let recent = crate::auth::format_rfc3339(now);
        let stale = crate::auth::format_rfc3339(now - time::Duration::hours(1));
        assert_eq!(
            health_for(
                "active",
                Some("ok"),
                Some("ok"),
                Some("ok"),
                Some("ok"),
                Some(&recent),
                Some(&stale),
                Some(&recent),
                Some("disabled"),
            ),
            ("unknown", "one or more observations are stale or unknown")
        );
    }

    #[test]
    fn stale_consensus_dimension_prevents_healthy_public_health() {
        let now = crate::auth::now_utc();
        let recent = crate::auth::format_rfc3339(now);
        let stale = crate::auth::format_rfc3339(now - time::Duration::hours(1));
        assert_eq!(
            health_for(
                "active",
                Some("ok"),
                Some("ok"),
                Some("ok"),
                Some("ok"),
                Some(&recent),
                Some(&recent),
                Some(&stale),
                Some("disabled"),
            ),
            ("unknown", "one or more observations are stale or unknown")
        );
    }

    #[tokio::test]
    async fn public_network_detail_excludes_private_and_retired_nodes() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let response = public_network(
            State(state),
            Path("mainnet".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(value["nodes"][0]["nodeId"], "node-public");
    }
}
