//! `/api/public/v1` route group — the Home Public Projection and human
//! session lifecycle (design §12.3/§12.4, §13.1).
//!
//! Middleware and DTO namespace are independent from Admin and Agent: DTOs
//! live in this module and are never reused as Admin DTOs by runtime field
//! filtering (design §13.1). Guest access is disabled by default, so every
//! route except `POST /login` requires a valid human Session; the session
//! guard itself lives in `super` and is attached in `build_app`.

use std::pin::Pin;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, Request, State};
use axum::http::header::{self, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tokio_stream::Stream;

use crate::http::realtime;

use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{
    LoginError, clear_cookie_header, format_rfc3339, login, session_cookie_header, touch_session,
    write_audit_event,
};
use crate::http::admin::mutation_guard;
use crate::http::{
    AppState, AuthenticatedSession, ClientIp, ROUTE_GROUP_HEADER, RequestId, api_not_found,
};
use crate::validator;

#[utoipa::path(
    get,
    path = "/api/public/v1/events",
    tag = "public",
    responses((status = 200, description = "Public invalidation stream (human-bound or Guest-bound when anonymous Home is enabled)"))
)]
pub(crate) async fn public_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<realtime::CursorQuery>,
    session: Option<Extension<AuthenticatedSession>>,
) -> Sse<
    axum::response::sse::KeepAliveStream<
        Pin<
            Box<
                dyn Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>
                    + Send,
            >,
        >,
    >,
> {
    let cursor = realtime::parse_cursor(
        headers.get("last-event-id").and_then(|v| v.to_str().ok()),
        query.after.as_deref(),
    );
    // Human sessions bind the stream to the connected role so revoke,
    // expiry, disable, and role change all close it; anonymous Guests are
    // bound to the Site Access Mode (design §13.5).
    let stream: Pin<
        Box<dyn Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>> + Send>,
    > = match session {
        Some(Extension(session)) => Box::pin(state.public_realtime().stream_with_session(
            cursor,
            state.database(),
            state.auth().clone(),
            session.0.session_id.clone(),
            session.0.role.clone(),
        )),
        None => Box::pin(
            state
                .public_realtime()
                .stream_with_guest(cursor, state.database()),
        ),
    };
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(realtime::keepalive_interval())
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
    body: Bytes,
) -> Response {
    // 1. Strict Origin validation (design §12.4): login carries no existing
    //    session, so the configured origin is the only acceptable one. Parse
    //    only after this check so malformed input cannot bypass the boundary.
    if !state.auth().origin_matches(headers.get(header::ORIGIN)) {
        return error_response(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "origin_validation_failed",
            "request origin does not match the configured origin",
        );
    }
    let body: LoginRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };

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
    let client_hint = crate::auth::client_hint_from_ua(
        headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok()),
    );
    match login(
        state.db(),
        state.auth(),
        &body.username,
        &body.password,
        presented.as_deref(),
        &client_hint,
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
                [
                    (
                        header::SET_COOKIE,
                        HeaderValue::from_str(&cookie).expect("cookie value is a valid header"),
                    ),
                    (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
                ],
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
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, false) {
        return response;
    }
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
        crate::auth::bump_authorization_generation(&mut transaction).await?;
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
    let mut response = Json(session_response(&session)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicCountryCount {
    pub country_code: String,
    pub count: i64,
    pub centroid_lat: Option<f64>,
    pub centroid_lon: Option<f64>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicPeerHistory {
    /// Aggregate collection state; history absence is never presented as a
    /// healthy zero-valued observation.
    pub state: String,
    pub freshness: String,
    pub five_minute: Vec<PublicPeerAggregate>,
    pub hourly: Vec<PublicPeerAggregate>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicPeerAggregate {
    pub bucket_start: String,
    pub last_observed_at: String,
    pub sample_count: i64,
    pub total_peers: i64,
    pub average_peers: Option<f64>,
    pub inbound_count: i64,
    pub outbound_count: i64,
    pub trusted_count: i64,
    pub static_count: i64,
    pub consensus_count: i64,
    pub known_country_count: i64,
    pub unknown_country_count: i64,
    pub countries: Vec<PublicCountryCount>,
    pub arrivals: i64,
    pub departures: i64,
    pub cbft_lag: PublicPeerLagSummary,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicPeerLagSummary {
    pub sample_count: i64,
    pub minimum: Option<i64>,
    pub average: Option<f64>,
    pub maximum: Option<i64>,
}

fn public_peer_aggregate(row: crate::peer_history::PeerAggregateRow) -> PublicPeerAggregate {
    let average_peers =
        (row.sample_count > 0).then(|| row.total_peers as f64 / row.sample_count as f64);
    let average_lag =
        (row.cbft_lag_count > 0).then(|| row.cbft_lag_sum as f64 / row.cbft_lag_count as f64);
    PublicPeerAggregate {
        bucket_start: row.bucket_start,
        last_observed_at: row.last_observed_at,
        sample_count: row.sample_count,
        total_peers: row.total_peers,
        average_peers,
        inbound_count: row.inbound_count,
        outbound_count: row.outbound_count,
        trusted_count: row.trusted_count,
        static_count: row.static_count,
        consensus_count: row.consensus_count,
        known_country_count: row.known_country_count,
        unknown_country_count: row.unknown_country_count,
        countries: row
            .countries
            .into_iter()
            .map(|country| {
                let (centroid_lat, centroid_lon) =
                    crate::geo::country_centroid(&country.country_code);
                PublicCountryCount {
                    country_code: country.country_code,
                    count: country.count,
                    centroid_lat,
                    centroid_lon,
                }
            })
            .collect(),
        arrivals: row.arrivals,
        departures: row.departures,
        cbft_lag: PublicPeerLagSummary {
            sample_count: row.cbft_lag_count,
            minimum: row.cbft_lag_min,
            average: average_lag,
            maximum: row.cbft_lag_max,
        },
    }
}

fn public_peer_history(history: crate::peer_history::PeerHistory) -> PublicPeerHistory {
    PublicPeerHistory {
        state: history.state,
        freshness: history.freshness,
        five_minute: history
            .five_minute
            .into_iter()
            .map(public_peer_aggregate)
            .collect(),
        hourly: history
            .hourly
            .into_iter()
            .map(public_peer_aggregate)
            .collect(),
    }
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicGeoInsight {
    /// Server-owned Geo database state. Raw IPs and MMDB paths never cross
    /// the Public projection boundary.
    pub state: String,
    pub last_good_at: Option<String>,
    pub database_age_seconds: Option<u64>,
    pub stale_since: Option<String>,
    pub error_reason: Option<String>,
    pub countries: Option<Vec<PublicCountryCount>>,
    pub attribution: Option<String>,
}

const PUBLIC_GEO_ERROR: &str = "Country data is currently unavailable";

fn unknown_public_geo_insight() -> PublicGeoInsight {
    PublicGeoInsight {
        state: "disabled".to_owned(),
        last_good_at: None,
        database_age_seconds: None,
        stale_since: None,
        error_reason: None,
        countries: None,
        attribution: None,
    }
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicPeerInsight {
    /// Agent-reported collection state. The WebUI presents this separately
    /// from freshness and value availability.
    pub state: String,
    /// Server-owned freshness of the last successful Peer Snapshot.
    pub freshness: String,
    /// The last successful observation time. No peer identity or address is
    /// included in the Public projection.
    pub observed_at: Option<String>,
    /// Server receipt time for the last accepted Peer snapshot.
    pub received_at: Option<String>,
    /// Server-computed boundary at which the last observation became stale.
    pub stale_since: Option<String>,
    /// `None` means no successful snapshot has ever been observed; `Some(0)`
    /// is an authoritative successful empty snapshot.
    pub peer_count: Option<i64>,
    pub inbound_count: Option<i64>,
    pub outbound_count: Option<i64>,
    pub trusted_count: Option<i64>,
    pub static_count: Option<i64>,
    pub consensus_count: Option<i64>,
}

fn unknown_public_peer_insight() -> PublicPeerInsight {
    PublicPeerInsight {
        state: "unknown".to_owned(),
        freshness: "unknown".to_owned(),
        observed_at: None,
        received_at: None,
        stale_since: None,
        peer_count: None,
        inbound_count: None,
        outbound_count: None,
        trusted_count: None,
        static_count: None,
        consensus_count: None,
    }
}

/// Node-scoped last-good consensus state projected from Agent reports.
/// `None` value fields mean no successful consensus observation has ever
/// been accepted; `Some(0)` is an authoritative zero-height observation.
/// Consensus membership is Agent-observed pool membership only; it never
/// creates or infers a Validator identity or Block Production evidence.
#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicConsensusInsight {
    /// Agent-reported collection state. The WebUI presents this separately
    /// from freshness and value availability.
    pub state: String,
    /// Server-owned freshness of the last successful consensus observation.
    pub freshness: String,
    /// The last successful observation time.
    pub observed_at: Option<String>,
    /// Server receipt time for the last accepted consensus observation.
    pub received_at: Option<String>,
    /// Server-computed boundary at which the last observation became stale.
    pub stale_since: Option<String>,
    /// Current epoch from the last good consensus observation.
    pub epoch: Option<i64>,
    /// Current view number from the last good consensus observation.
    pub view_number: Option<i64>,
    /// Whether the current validator pool contains this Node. `None` means
    /// no successful observation exists; it must never be presented as False.
    pub validator: Option<bool>,
    /// Highest QC block height from the last good consensus observation.
    pub highest_qc_block: Option<i64>,
    /// Highest lock block height from the last good consensus observation.
    pub highest_lock_block: Option<i64>,
    /// Highest commit block height from the last good consensus observation.
    pub highest_commit_block: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicNetwork {
    pub network_key: String,
    pub display_name: String,
    pub peers: PublicPeerInsight,
    pub geo: PublicGeoInsight,
    pub validators: Vec<PublicValidatorInsight>,
    pub nodes: Vec<PublicNode>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicValidatorInsight {
    pub validator_id: String,
    pub validator_node_id: String,
    pub display_name: Option<String>,
    pub node_id: Option<String>,
    pub link_role: Option<String>,
    pub state: String,
    pub freshness: String,
    pub source: Option<String>,
    pub provider_timestamp: Option<String>,
    pub received_at: Option<String>,
    pub rank: Option<i64>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
    pub counter_state: String,
    /// Canonical last-good Validator Activity (`active`, `producing`,
    /// `exiting`, `exited`, `verifying`, `locked`) or `observing`/`unknown`.
    /// Home never infers this from names, endpoints, consensus membership,
    /// rank data, or Provider data: only an effective explicit Node Validator
    /// Link exposes it on a Public Node (#100).
    pub activity: String,
    /// `current`, `stale`, or `unknown` currency of the Activity value.
    /// Provider failure with a last-good Activity is always `stale`, even
    /// when the last-good timestamp is still within the freshness window.
    pub activity_state: String,
}

#[derive(Debug, sqlx::FromRow)]
struct PublicValidatorRow {
    validator_id: String,
    validator_node_id: String,
    display_name: Option<String>,
    node_id: Option<String>,
    link_role: Option<String>,
    source: Option<String>,
    outcome: Option<String>,
    provider_timestamp: Option<String>,
    activity: Option<String>,
    last_good_received_at: Option<String>,
    rank: Option<i64>,
    stake_amount: Option<String>,
    reward_amount: Option<String>,
    reward_rate: Option<String>,
    delegator_count: Option<i64>,
    epoch: Option<i64>,
    block_count: Option<i64>,
    counter_state: Option<String>,
}

/// Map the linked Validator's canonical last-good Activity and its currency
/// for the Public projection. Provider outcomes never fabricate a value:
/// authoritative empty/not-found is Observing, a successful snapshot shows
/// the canonical label (Stale when Server freshness expired), and Error
/// with a last-good Activity is always Stale. Unsupported coverage is
/// permanent for the Public projection: it projects Unknown even when a
/// last-good Activity was previously observed (#100, #101).
fn public_validator_activity(
    outcome: &str,
    activity: Option<&str>,
    freshness: &str,
) -> (String, String) {
    match outcome {
        "empty" | "not_found" => ("observing".to_owned(), "current".to_owned()),
        "success" => match activity {
            Some(value) => (
                value.to_owned(),
                match freshness {
                    "fresh" => "current",
                    "stale" => "stale",
                    _ => "unknown",
                }
                .to_owned(),
            ),
            None => ("unknown".to_owned(), "unknown".to_owned()),
        },
        "error" => match activity {
            Some(value) => (value.to_owned(), "stale".to_owned()),
            None => ("unknown".to_owned(), "unknown".to_owned()),
        },
        "unsupported" => ("unknown".to_owned(), "unknown".to_owned()),
        _ => ("unknown".to_owned(), "unknown".to_owned()),
    }
}

async fn public_validator_insights(
    state: &AppState,
    network_key: &str,
) -> Result<Vec<PublicValidatorInsight>, sqlx::Error> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let rows = sqlx::query_as::<_, PublicValidatorRow>(
        "SELECT v.validator_id, v.validator_node_id, v.display_name, (SELECT n2.node_id FROM node_validator_links l2 JOIN nodes n2 ON n2.node_id = l2.node_id WHERE l2.validator_id = v.validator_id AND l2.valid_from <= ? AND (l2.valid_until IS NULL OR l2.valid_until > ?) AND n2.visibility = 'public' AND n2.lifecycle = 'active' ORDER BY l2.valid_from DESC, l2.link_id LIMIT 1) AS node_id, (SELECT l2.role FROM node_validator_links l2 JOIN nodes n2 ON n2.node_id = l2.node_id WHERE l2.validator_id = v.validator_id AND l2.valid_from <= ? AND (l2.valid_until IS NULL OR l2.valid_until > ?) AND n2.visibility = 'public' AND n2.lifecycle = 'active' ORDER BY l2.valid_from DESC, l2.link_id LIMIT 1) AS link_role, i.source, i.outcome, i.provider_timestamp, i.activity, i.last_good_received_at, i.rank, i.stake_amount, i.reward_amount, i.reward_rate, i.delegator_count, i.epoch, i.block_count, i.counter_state FROM validators v LEFT JOIN current_validator_insights i ON i.validator_id = v.validator_id WHERE v.network_key = ? AND EXISTS (SELECT 1 FROM node_validator_links l JOIN nodes n ON n.node_id = l.node_id WHERE l.validator_id = v.validator_id AND l.valid_from <= ? AND (l.valid_until IS NULL OR l.valid_until > ?) AND n.visibility = 'public' AND n.lifecycle = 'active') ORDER BY v.validator_node_id, v.validator_id",
    )
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .bind(network_key)
    .bind(&now)
    .bind(&now)
    .fetch_all(state.db().pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let freshness =
                validator::freshness(row.last_good_received_at.as_deref(), crate::auth::now_utc());
            let outcome = row.outcome.unwrap_or_else(|| "unsupported".to_owned());
            let state = if outcome == "success" {
                freshness
            } else {
                outcome.as_str()
            };
            let (activity, activity_state) =
                public_validator_activity(&outcome, row.activity.as_deref(), freshness);
            PublicValidatorInsight {
                validator_id: row.validator_id,
                validator_node_id: row.validator_node_id,
                display_name: row.display_name,
                node_id: row.node_id,
                link_role: row.link_role,
                state: state.to_owned(),
                freshness: freshness.to_owned(),
                source: row.source.or_else(|| Some("disabled".to_owned())),
                provider_timestamp: row.provider_timestamp,
                received_at: row.last_good_received_at,
                rank: row.rank,
                stake_amount: row.stake_amount,
                reward_amount: row.reward_amount,
                reward_rate: row.reward_rate,
                delegator_count: row.delegator_count,
                epoch: row.epoch,
                block_count: row.block_count,
                counter_state: row.counter_state.unwrap_or_else(|| "normal".to_owned()),
                activity,
                activity_state,
            }
        })
        .collect())
}

/// One effective explicit Node Validator Link for a public, active Node.
/// Activity is associated per Node, so a Validator linked concurrently to
/// several Nodes is visible on every one of them (#100).
#[derive(Debug, Clone, sqlx::FromRow)]
struct EffectiveLinkRow {
    node_id: String,
    validator_id: String,
    role: String,
}

async fn effective_public_links(
    state: &AppState,
    node_id: Option<&str>,
    network_key: Option<&str>,
) -> Result<Vec<EffectiveLinkRow>, sqlx::Error> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let mut sql = String::from(
        "SELECT l.node_id, l.validator_id, l.role FROM node_validator_links l JOIN nodes n ON n.node_id = l.node_id WHERE l.valid_from <= ? AND (l.valid_until IS NULL OR l.valid_until > ?) AND n.visibility = 'public' AND n.lifecycle = 'active'",
    );
    if node_id.is_some() {
        sql.push_str(" AND l.node_id = ?");
    }
    if network_key.is_some() {
        sql.push_str(" AND n.network_key = ?");
    }
    sql.push_str(" ORDER BY l.node_id, l.link_id");
    let mut query = sqlx::query_as::<_, EffectiveLinkRow>(&sql)
        .bind(&now)
        .bind(&now);
    if let Some(node_id) = node_id {
        query = query.bind(node_id);
    }
    if let Some(network_key) = network_key {
        query = query.bind(network_key);
    }
    query.fetch_all(state.db().pool()).await
}

/// Attach each Node's linked Validator insight with the Node-specific role
/// and effective Link identity. A Node without an effective Link keeps a
/// `None` Validator and renders Unknown Activity (#100).
fn associate_node_validators(
    validators: &[PublicValidatorInsight],
    links: &[EffectiveLinkRow],
    nodes: &mut [PublicNode],
) {
    for node in nodes.iter_mut() {
        let Some(link) = links.iter().find(|link| link.node_id == node.node_id) else {
            continue;
        };
        let Some(validator) = validators
            .iter()
            .find(|validator| validator.validator_id == link.validator_id)
        else {
            continue;
        };
        let mut associated = validator.clone();
        associated.node_id = Some(link.node_id.clone());
        associated.link_role = Some(link.role.clone());
        node.validator = Some(associated);
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicBlockHistoryItem {
    pub node_id: String,
    pub height: Option<i64>,
    pub block_time_ms: Option<i64>,
    pub transaction_count: Option<i64>,
    pub source: Option<String>,
    pub observed_at: Option<String>,
    pub freshness: Option<String>,
    pub gap_from_height: Option<i64>,
    pub gap_to_height: Option<i64>,
    pub gap_kind: Option<String>,
    pub gap_reason: Option<String>,
    pub divergence_kind: Option<String>,
    pub divergence_reason: Option<String>,
    pub coinbase: Option<String>,
    pub seal_signer_match: Option<String>,
    pub protocol_proposer: Option<String>,
}
#[derive(Debug, sqlx::FromRow)]
struct PublicHistoryRow {
    pub block_number: Option<i64>,
    pub block_timestamp_ms: Option<i64>,
    pub transaction_count: Option<i64>,
    pub source: Option<String>,
    pub coinbase: Option<String>,
    pub seal_signer_match: Option<String>,
    pub protocol_proposer: Option<String>,
    pub observed_at: Option<String>,
    pub from_height: Option<i64>,
    pub to_height: Option<i64>,
    pub gap_kind: Option<String>,
    pub divergence_kind: Option<String>,
}

const PUBLIC_NODE_METRIC_WINDOW_SECONDS: i64 = 60;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicMetricPoint {
    pub sampled_at: String,
    pub value: f64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicNodeMetricHistory {
    pub from: String,
    pub to: String,
    pub window_seconds: i64,
    pub process_cpu_percent: Vec<PublicMetricPoint>,
    pub process_memory_percent: Vec<PublicMetricPoint>,
    pub data_directory_percent: Vec<PublicMetricPoint>,
    pub network_rx_bytes_per_sec: Vec<PublicMetricPoint>,
    pub network_tx_bytes_per_sec: Vec<PublicMetricPoint>,
    pub peer_inbound_count: Vec<PublicMetricPoint>,
    pub peer_outbound_count: Vec<PublicMetricPoint>,
    pub block_interval_ms: Vec<PublicMetricPoint>,
    pub transaction_count: Vec<PublicMetricPoint>,
}

#[derive(Debug, sqlx::FromRow)]
struct MetricHistoryRow {
    metric: String,
    sampled_at: String,
    value: f64,
}

#[derive(Debug, sqlx::FromRow)]
struct BlockMetricHistoryRow {
    sampled_at: String,
    block_interval_ms: Option<i64>,
    transaction_count: i64,
}

#[utoipa::path(
    get,
    path = "/api/public/v1/nodes/{node_id}/history",
    tag = "public",
    params(
        ("node_id" = String, Path),
        ("from" = Option<i64>, Query, minimum = 0, description = "First block height"),
        ("to" = Option<i64>, Query, minimum = 0, description = "Last block height"),
        ("limit" = Option<i64>, Query, minimum = 1, maximum = 200, description = "Maximum rows")
    ),
    responses(
        (status = 200, body = [PublicBlockHistoryItem]),
        (status = 400, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(crate) async fn public_node_history(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Query(params): Query<PublicBlockHistoryQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let (from, to) = match history_bounds(&params, &request_id.0) {
        Ok(bounds) => bounds,
        Err(response) => return *response,
    };
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    // Check visibility before reading history so a guessed private/retired
    // Node ID is indistinguishable from a missing representation.
    let visible = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM nodes WHERE node_id = ? AND visibility = 'public' AND lifecycle = 'active'",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await;
    match visible {
        Ok(Some(_)) => {}
        Ok(None) => {
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
    }
    let raw_retention_days =
        match crate::retention::raw_block_summary_retention_days(state.db().pool()).await {
            Ok(days) => days,
            Err(_) => {
                return error_response(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
        };
    let cutoff = crate::auth::format_rfc3339(crate::retention::family_cutoff(
        crate::auth::now_utc(),
        raw_retention_days,
    ));
    let rows = sqlx::query_as::<_, PublicHistoryRow>("SELECT block_number, block_timestamp_ms, transaction_count, source, coinbase, seal_signer_match, protocol_proposer, observed_at, from_height, to_height, gap_kind, divergence_kind FROM (SELECT block_number, block_timestamp_ms, transaction_count, source, coinbase, seal_signer_match, CASE WHEN protocol_proposer_kind = 'verified' THEN protocol_proposer_identity ELSE NULL END AS protocol_proposer, observed_at, NULL AS from_height, NULL AS to_height, NULL AS gap_kind, NULL AS divergence_kind FROM block_summaries WHERE node_id = ? AND accepted_at >= ? AND EXISTS (SELECT 1 FROM nodes WHERE node_id = block_summaries.node_id AND visibility = 'public' AND lifecycle = 'active') UNION ALL SELECT NULL AS block_number, NULL AS block_timestamp_ms, NULL AS transaction_count, NULL AS source, NULL AS coinbase, NULL AS seal_signer_match, NULL AS protocol_proposer, created_at AS observed_at, from_height, to_height, kind AS gap_kind, NULL AS divergence_kind FROM block_history_gaps WHERE node_id = ? AND EXISTS (SELECT 1 FROM nodes WHERE node_id = block_history_gaps.node_id AND visibility = 'public' AND lifecycle = 'active') UNION ALL SELECT NULL AS block_number, NULL AS block_timestamp_ms, NULL AS transaction_count, NULL AS source, NULL AS coinbase, NULL AS seal_signer_match, NULL AS protocol_proposer, retained_observed_at AS observed_at, height AS from_height, height AS to_height, NULL AS gap_kind, 'chain_divergence' AS divergence_kind FROM chain_divergence_observations WHERE node_id = ? AND EXISTS (SELECT 1 FROM nodes WHERE node_id = chain_divergence_observations.node_id AND visibility = 'public' AND lifecycle = 'active')) WHERE (? IS NULL OR COALESCE(block_number, to_height) >= ?) AND (? IS NULL OR COALESCE(block_number, from_height) <= ?) ORDER BY COALESCE(block_number, from_height) DESC LIMIT ?")
        .bind(&node_id).bind(&cutoff).bind(&node_id).bind(&node_id)
        .bind(from).bind(from).bind(to).bind(to).bind(limit)
        .fetch_all(state.db().pool()).await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| PublicBlockHistoryItem {
                    node_id: node_id.clone(),
                    height: row.block_number,
                    block_time_ms: row.block_timestamp_ms,
                    transaction_count: row.transaction_count,
                    source: row.source,
                    observed_at: row.observed_at.clone(),
                    freshness: row.observed_at,
                    gap_from_height: row.from_height,
                    gap_to_height: row.to_height,
                    gap_kind: row.gap_kind.clone(),
                    gap_reason: row.gap_kind.as_deref().map(|kind| match kind {
                        "spool_overflow" => "Some samples could not be retained".to_owned(),
                        "unrecoverable_backfill" => {
                            "A history interval could not be recovered".to_owned()
                        }
                        "server_rejected" => "A sample was rejected by the Server".to_owned(),
                        "chain_divergence" => {
                            "A recent chain identity divergence was observed".to_owned()
                        }
                        _ => "A history interval is unavailable".to_owned(),
                    }),
                    divergence_kind: row.divergence_kind.clone(),
                    divergence_reason: row
                        .divergence_kind
                        .map(|_| "A recent chain identity divergence was observed".to_owned()),
                    coinbase: row.coinbase,
                    seal_signer_match: row.seal_signer_match,
                    protocol_proposer: row.protocol_proposer,
                })
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

fn push_metric_point(history: &mut PublicNodeMetricHistory, row: MetricHistoryRow) {
    let point = PublicMetricPoint {
        sampled_at: row.sampled_at,
        value: row.value,
    };
    match row.metric.as_str() {
        "process_cpu_percent" => history.process_cpu_percent.push(point),
        "process_memory_percent" => history.process_memory_percent.push(point),
        "data_directory_percent" => history.data_directory_percent.push(point),
        "network_rx_bytes_per_sec" => history.network_rx_bytes_per_sec.push(point),
        "network_tx_bytes_per_sec" => history.network_tx_bytes_per_sec.push(point),
        "peer_inbound_count" => history.peer_inbound_count.push(point),
        "peer_outbound_count" => history.peer_outbound_count.push(point),
        _ => {}
    }
}

#[utoipa::path(
    get,
    path = "/api/public/v1/nodes/{node_id}/metrics",
    tag = "public",
    params(("node_id" = String, Path)),
    responses(
        (status = 200, body = PublicNodeMetricHistory),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(crate) async fn public_node_metrics(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let agent_id = sqlx::query_scalar::<_, String>(
        "SELECT agent_id FROM nodes WHERE node_id=? AND visibility='public' AND lifecycle='active'",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await;
    let agent_id = match agent_id {
        Ok(Some(agent_id)) => agent_id,
        Ok(None) => {
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

    let to = crate::auth::now_utc();
    let from = to - time::Duration::seconds(PUBLIC_NODE_METRIC_WINDOW_SECONDS);
    let from = format_rfc3339(from);
    let to = format_rfc3339(to);
    let metric_rows = sqlx::query_as::<_, MetricHistoryRow>(
        "SELECT metric, observed_at AS sampled_at, value FROM node_metric_samples AS current WHERE node_id=? AND (received_at>=? OR received_at=(SELECT MAX(received_at) FROM node_metric_samples AS previous WHERE previous.node_id=current.node_id AND previous.metric=current.metric AND previous.received_at<?)) UNION ALL SELECT metric, observed_at AS sampled_at, value FROM host_metric_samples AS current WHERE agent_id=? AND (received_at>=? OR received_at=(SELECT MAX(received_at) FROM host_metric_samples AS previous WHERE previous.agent_id=current.agent_id AND previous.metric=current.metric AND previous.received_at<?)) ORDER BY sampled_at, metric",
    )
    .bind(&node_id)
    .bind(&from)
    .bind(&from)
    .bind(&agent_id)
    .bind(&from)
    .bind(&from)
    .fetch_all(state.db().pool())
    .await;
    let block_rows = sqlx::query_as::<_, BlockMetricHistoryRow>(
        "SELECT current.observed_at AS sampled_at, CASE WHEN previous.block_timestamp_ms IS NOT NULL AND current.block_timestamp_ms > previous.block_timestamp_ms THEN current.block_timestamp_ms - previous.block_timestamp_ms ELSE NULL END AS block_interval_ms, current.transaction_count FROM block_summaries AS current LEFT JOIN block_summaries AS previous ON previous.node_id=current.node_id AND previous.block_number=current.block_number-1 WHERE current.node_id=? ORDER BY current.block_number DESC LIMIT 200",
    )
    .bind(&node_id)
    .fetch_all(state.db().pool())
    .await;
    let (metric_rows, block_rows) = match (metric_rows, block_rows) {
        (Ok(metric_rows), Ok(block_rows)) => (metric_rows, block_rows),
        _ => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };

    let mut history = PublicNodeMetricHistory {
        from: from.clone(),
        to,
        window_seconds: PUBLIC_NODE_METRIC_WINDOW_SECONDS,
        process_cpu_percent: Vec::new(),
        process_memory_percent: Vec::new(),
        data_directory_percent: Vec::new(),
        network_rx_bytes_per_sec: Vec::new(),
        network_tx_bytes_per_sec: Vec::new(),
        peer_inbound_count: Vec::new(),
        peer_outbound_count: Vec::new(),
        block_interval_ms: Vec::new(),
        transaction_count: Vec::new(),
    };
    for row in metric_rows {
        push_metric_point(&mut history, row);
    }

    let mut prior_interval = None;
    let mut prior_transactions = None;
    for row in block_rows.into_iter().rev() {
        let interval = row.block_interval_ms.map(|value| PublicMetricPoint {
            sampled_at: row.sampled_at.clone(),
            value: value as f64,
        });
        let transactions = PublicMetricPoint {
            sampled_at: row.sampled_at.clone(),
            value: row.transaction_count as f64,
        };
        if row.sampled_at < from {
            if interval.is_some() {
                prior_interval = interval;
            }
            prior_transactions = Some(transactions);
        } else {
            if let Some(interval) = interval {
                history.block_interval_ms.push(interval);
            }
            history.transaction_count.push(transactions);
        }
    }
    if let Some(point) = prior_interval {
        history.block_interval_ms.insert(0, point);
    }
    if let Some(point) = prior_transactions {
        history.transaction_count.insert(0, point);
    }

    Json(history).into_response()
}

#[utoipa::path(
    get,
    path = "/api/public/v1/nodes/{node_id}/peer-history",
    tag = "public",
    params(("node_id" = String, Path)),
    responses((status = 200, body = PublicPeerHistory), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn public_node_peer_history(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let visible = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM nodes WHERE node_id=? AND visibility='public' AND lifecycle='active'",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await;
    match visible {
        Ok(Some(_)) => {}
        Ok(None) => {
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
    }
    match crate::peer_history::load_history(state.db().pool(), &node_id).await {
        Ok(history) => Json(public_peer_history(history)).into_response(),
        Err(_) => error_response(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Export exactly the same allowlisted Public history projection as JSON.
/// Visibility is checked by `public_node_history`, so this route cannot be
/// used to bypass list/detail/history privacy filtering.
#[utoipa::path(
    get,
    path = "/api/public/v1/nodes/{node_id}/history/export",
    tag = "public",
    params(
        ("node_id" = String, Path),
        ("from" = Option<i64>, Query, minimum = 0, description = "First block height"),
        ("to" = Option<i64>, Query, minimum = 0, description = "Last block height"),
        ("limit" = Option<i64>, Query, minimum = 1, maximum = 200, description = "Maximum rows")
    ),
    responses(
        (status = 200, body = [PublicBlockHistoryItem]),
        (status = 400, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(crate) async fn public_node_history_export(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Query(params): Query<PublicBlockHistoryQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let mut response = public_node_history(
        State(state),
        Path(node_id),
        Query(params),
        Extension(request_id),
    )
    .await;
    if response.status() == StatusCode::OK {
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=public-history.json"),
        );
    }
    response
}

#[derive(Debug, Deserialize)]
pub struct PublicBlockHistoryQuery {
    pub limit: Option<i64>,
    pub from: Option<i64>,
    pub to: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ValidatorHistoryQuery {
    limit: Option<i64>,
}

pub(super) fn history_bounds(
    params: &PublicBlockHistoryQuery,
    request_id: &str,
) -> Result<(Option<i64>, Option<i64>), Box<Response>> {
    if params.from.is_some_and(|value| value < 0)
        || params.to.is_some_and(|value| value < 0)
        || params
            .from
            .zip(params.to)
            .is_some_and(|(from, to)| from > to)
    {
        return Err(Box::new(error_response(
            request_id,
            StatusCode::BAD_REQUEST,
            "invalid_history_range",
            "history range is invalid",
        )));
    }
    Ok((params.from, params.to))
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
    pub peers: PublicPeerInsight,
    pub consensus: PublicConsensusInsight,
    pub process_cpu_percent: Option<f64>,
    pub process_memory_percent: Option<f64>,
    pub process_started_at: Option<String>,
    pub process_uptime_ms: Option<i64>,
    pub last_report_at: Option<String>,
    pub host_cpu_percent: Option<f64>,
    pub host_memory_percent: Option<f64>,
    pub host_storage_percent: Option<f64>,
    pub node_data_directory_size_bytes: Option<i64>,
    pub node_data_directory_capacity_bytes: Option<i64>,
    pub host_network_rx_bytes_per_sec: Option<i64>,
    pub host_network_tx_bytes_per_sec: Option<i64>,
    pub current_head: Option<i64>,
    /// Transaction count from this Node's latest persisted Block Summary.
    /// `None` means the Node has not produced a Block Summary yet.
    pub latest_block_transaction_count: Option<i64>,
    pub historical_high_watermark: Option<i64>,
    pub resync_state: String,
    pub network_reference_head: Option<i64>,
    pub network_reference_confidence: String,
    pub resync_progress: Option<String>,
    pub validator: Option<PublicValidatorInsight>,
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
    process_cpu_percent: Option<f64>,
    process_memory_percent: Option<f64>,
    process_started_at: Option<String>,
    process_uptime_ms: Option<i64>,
    last_report_at: Option<String>,
    host_cpu_percent: Option<f64>,
    host_memory_percent: Option<f64>,
    host_storage_percent: Option<f64>,
    node_data_directory_size_bytes: Option<i64>,
    node_data_directory_capacity_bytes: Option<i64>,
    host_network_rx_bytes_per_sec: Option<i64>,
    host_network_tx_bytes_per_sec: Option<i64>,
    sync_state: Option<String>,
    consensus_state: Option<String>,
    process_state: Option<String>,
    sync_received_at: Option<String>,
    consensus_received_at: Option<String>,
    peer_state: Option<String>,
    peer_observed_at: Option<String>,
    peer_value_received_at: Option<String>,
    peer_value_revision: Option<i64>,
    peer_count: Option<i64>,
    peer_inbound_count: Option<i64>,
    peer_outbound_count: Option<i64>,
    peer_trusted_count: Option<i64>,
    peer_static_count: Option<i64>,
    peer_consensus_count: Option<i64>,
    consensus_observed_at: Option<String>,
    consensus_value_received_at: Option<String>,
    consensus_value_revision: Option<i64>,
    consensus_epoch: Option<i64>,
    consensus_view_number: Option<i64>,
    consensus_validator: Option<i64>,
    consensus_highest_qc_block: Option<i64>,
    consensus_highest_lock_block: Option<i64>,
    consensus_highest_commit_block: Option<i64>,
    current_head: Option<i64>,
    latest_block_transaction_count: Option<i64>,
    historical_high_watermark: Option<i64>,
    resync_state: Option<String>,
    resync_last_progress_at: Option<String>,
    network_reference_head: Option<i64>,
    network_reference_confidence: Option<String>,
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

fn public_peer_insight(row: &PublicNodeRow) -> PublicPeerInsight {
    let has_value = row.peer_value_revision.unwrap_or_default() > 0;
    let freshness = if has_value {
        match row.peer_value_received_at.as_deref() {
            Some(timestamp) if fresh(Some(timestamp), crate::auth::now_utc()) => "current",
            Some(_) => "stale",
            None => "unknown",
        }
    } else {
        "unknown"
    };
    let stale_since = if freshness == "stale" {
        row.peer_value_received_at.as_deref().and_then(|timestamp| {
            crate::auth::parse_rfc3339(timestamp).map(|received| {
                crate::auth::format_rfc3339(
                    received + time::Duration::seconds(FRESHNESS_LIMIT_SECONDS),
                )
            })
        })
    } else {
        None
    };
    PublicPeerInsight {
        state: row
            .peer_state
            .clone()
            .unwrap_or_else(|| "unknown".to_owned()),
        freshness: freshness.to_owned(),
        observed_at: row.peer_observed_at.clone(),
        received_at: row.peer_value_received_at.clone(),
        stale_since,
        peer_count: has_value.then_some(row.peer_count).flatten(),
        inbound_count: has_value.then_some(row.peer_inbound_count).flatten(),
        outbound_count: has_value.then_some(row.peer_outbound_count).flatten(),
        trusted_count: has_value.then_some(row.peer_trusted_count).flatten(),
        static_count: has_value.then_some(row.peer_static_count).flatten(),
        consensus_count: has_value.then_some(row.peer_consensus_count).flatten(),
    }
}

fn public_consensus_insight(row: &PublicNodeRow) -> PublicConsensusInsight {
    let has_value = row.consensus_value_revision.unwrap_or_default() > 0;
    let freshness = if has_value {
        match row.consensus_value_received_at.as_deref() {
            Some(timestamp) if fresh(Some(timestamp), crate::auth::now_utc()) => "current",
            Some(_) => "stale",
            None => "unknown",
        }
    } else {
        "unknown"
    };
    let stale_since = if freshness == "stale" {
        row.consensus_value_received_at
            .as_deref()
            .and_then(|timestamp| {
                crate::auth::parse_rfc3339(timestamp).map(|received| {
                    crate::auth::format_rfc3339(
                        received + time::Duration::seconds(FRESHNESS_LIMIT_SECONDS),
                    )
                })
            })
    } else {
        None
    };
    PublicConsensusInsight {
        state: row
            .consensus_state
            .clone()
            .unwrap_or_else(|| "unknown".to_owned()),
        freshness: freshness.to_owned(),
        observed_at: row.consensus_observed_at.clone(),
        received_at: row.consensus_value_received_at.clone(),
        stale_since,
        epoch: has_value.then_some(row.consensus_epoch).flatten(),
        view_number: has_value.then_some(row.consensus_view_number).flatten(),
        validator: has_value
            .then_some(row.consensus_validator.map(|value| value != 0))
            .flatten(),
        highest_qc_block: has_value
            .then_some(row.consensus_highest_qc_block)
            .flatten(),
        highest_lock_block: has_value
            .then_some(row.consensus_highest_lock_block)
            .flatten(),
        highest_commit_block: has_value
            .then_some(row.consensus_highest_commit_block)
            .flatten(),
    }
}

fn aggregate_peer_insight(nodes: &[PublicNode]) -> PublicPeerInsight {
    let insights = nodes.iter().map(|node| &node.peers).collect::<Vec<_>>();
    let state = if insights.iter().any(|peer| peer.state == "error") {
        "error"
    } else if insights.iter().any(|peer| peer.state == "unsupported") {
        "unsupported"
    } else if insights.iter().any(|peer| peer.state == "disabled") {
        "disabled"
    } else if insights.iter().any(|peer| peer.state == "starting") {
        "starting"
    } else if insights.iter().all(|peer| peer.state == "ok")
        && insights.iter().all(|peer| peer.peer_count.is_some())
    {
        "ok"
    } else {
        "unknown"
    };
    let all_values = insights.iter().all(|peer| peer.peer_count.is_some());
    let freshness = if !all_values || insights.is_empty() {
        "unknown"
    } else if insights.iter().all(|peer| peer.freshness == "current") {
        "current"
    } else if insights.iter().any(|peer| peer.freshness == "stale") {
        "stale"
    } else {
        "unknown"
    };
    let sum = |value: fn(&PublicPeerInsight) -> Option<i64>| {
        all_values.then(|| insights.iter().filter_map(|peer| value(peer)).sum())
    };
    PublicPeerInsight {
        state: state.to_owned(),
        freshness: freshness.to_owned(),
        // A Network has multiple observation times; exposing one would imply
        // a precision the aggregate does not have.
        observed_at: None,
        received_at: None,
        stale_since: None,
        peer_count: sum(|peer| peer.peer_count),
        inbound_count: sum(|peer| peer.inbound_count),
        outbound_count: sum(|peer| peer.outbound_count),
        trusted_count: sum(|peer| peer.trusted_count),
        static_count: sum(|peer| peer.static_count),
        consensus_count: sum(|peer| peer.consensus_count),
    }
}

fn geo_timing(
    status: &crate::geo::GeoStatus,
    now: time::OffsetDateTime,
) -> (Option<u64>, Option<String>) {
    let Some(build_epoch) = status.build_epoch else {
        return (None, None);
    };
    let Some(build_epoch_seconds) = i64::try_from(build_epoch).ok() else {
        return (None, None);
    };
    let age_seconds = now
        .unix_timestamp()
        .saturating_sub(build_epoch_seconds)
        .max(0) as u64;
    let stale_since = build_epoch
        .checked_add(crate::geo::DATABASE_MAX_AGE.as_secs())
        .and_then(|epoch| i64::try_from(epoch).ok())
        .filter(|epoch| now.unix_timestamp() >= *epoch)
        .and_then(|epoch| time::OffsetDateTime::from_unix_timestamp(epoch).ok())
        .map(crate::auth::format_rfc3339);
    (Some(age_seconds), stale_since)
}

async fn public_country_distribution(state: &AppState, network_key: &str) -> PublicGeoInsight {
    let geo_status = state.geo().status();
    if geo_status.state == "disabled" {
        return unknown_public_geo_insight();
    }
    let now_utc = crate::auth::now_utc();
    let (database_age_seconds, stale_since) = geo_timing(&geo_status, now_utc);
    let now = crate::auth::format_rfc3339(now_utc);
    let rebuild_before = crate::geo::cache_rebuild_cutoff(&now);
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT g.country_code, COUNT(*) FROM current_node_peers p JOIN geo_location_cache g ON g.canonical_ip = p.remote_ip JOIN nodes n ON n.node_id = p.node_id WHERE n.network_key=? AND n.visibility='public' AND n.lifecycle='active' AND g.expires_at > ? AND g.created_at > ? GROUP BY g.country_code ORDER BY g.country_code",
    )
    .bind(network_key)
    .bind(now)
    .bind(rebuild_before)
    .fetch_all(state.db().pool())
    .await;
    let Ok(rows) = rows else {
        return PublicGeoInsight {
            state: "error".to_owned(),
            last_good_at: geo_status.loaded_at.clone(),
            database_age_seconds,
            stale_since,
            // Loader errors can contain filesystem/parser details. Public
            // responses expose only a stable, non-sensitive explanation.
            error_reason: Some(PUBLIC_GEO_ERROR.to_owned()),
            countries: None,
            attribution: None,
        };
    };
    let countries = rows
        .into_iter()
        .map(|(country_code, count)| {
            let (centroid_lat, centroid_lon) = crate::geo::country_centroid(&country_code);
            PublicCountryCount {
                country_code,
                count,
                centroid_lat,
                centroid_lon,
            }
        })
        .collect::<Vec<_>>();
    let geo_error = geo_status.state == "error";
    PublicGeoInsight {
        state: geo_status.state,
        last_good_at: geo_status.loaded_at,
        database_age_seconds,
        stale_since,
        error_reason: geo_error.then(|| PUBLIC_GEO_ERROR.to_owned()),
        countries: Some(countries),
        attribution: Some(crate::geo::MAXMIND_ATTRIBUTION.to_owned()),
    }
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
    let peers = public_peer_insight(&row);
    let consensus = public_consensus_insight(&row);
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
        peers,
        consensus,
        process_cpu_percent: row.process_cpu_percent,
        process_memory_percent: row.process_memory_percent,
        process_started_at: row.process_started_at,
        process_uptime_ms: row.process_uptime_ms,
        last_report_at: row.last_report_at,
        host_cpu_percent: row.host_cpu_percent,
        host_memory_percent: row.host_memory_percent,
        host_storage_percent: row.host_storage_percent,
        node_data_directory_size_bytes: row.node_data_directory_size_bytes,
        node_data_directory_capacity_bytes: row.node_data_directory_capacity_bytes,
        host_network_rx_bytes_per_sec: row.host_network_rx_bytes_per_sec,
        host_network_tx_bytes_per_sec: row.host_network_tx_bytes_per_sec,
        current_head: row.current_head,
        latest_block_transaction_count: row.latest_block_transaction_count,
        historical_high_watermark: row.historical_high_watermark,
        resync_state: row.resync_state.unwrap_or_else(|| "normal".to_owned()),
        network_reference_head: row.network_reference_head,
        network_reference_confidence: row
            .network_reference_confidence
            .unwrap_or_else(|| "unknown".to_owned()),
        resync_progress: row.historical_high_watermark.zip(row.current_head).map(
            |(high, current)| match row.resync_last_progress_at.as_deref() {
                Some(at) => format!("{current}/{high} (last progress {at})"),
                None => format!("{current}/{high}"),
            },
        ),
        validator: None,
    };
    (row.network_display_name, node)
}

const PUBLIC_NODE_QUERY_BASE: &str = r#"SELECT n.node_id, n.display_name, n.network_key, r.display_name AS network_display_name,
       n.lifecycle, s.state AS rpc_state, sy.state AS sync_state, co.state AS consensus_state,
       p.state AS process_state, i.state AS identity_state, s.received_at AS updated_at,
       sy.received_at AS sync_received_at, co.received_at AS consensus_received_at,
       CASE WHEN p.state IN ('ok', 'error') THEN proc.cpu_percent ELSE NULL END AS process_cpu_percent,
       CASE WHEN p.state IN ('ok', 'error') AND h.memory_total_bytes > 0 THEN (CAST(proc.memory_bytes AS REAL) * 100.0 / h.memory_total_bytes) ELSE NULL END AS process_memory_percent,
       CASE WHEN p.state IN ('ok', 'error') THEN proc.started_at ELSE NULL END AS process_started_at,
       CASE WHEN p.state IN ('ok', 'error') THEN proc.uptime_ms ELSE NULL END AS process_uptime_ms,
       a.last_received_at AS last_report_at,
       h.cpu_percent AS host_cpu_percent,
       CASE WHEN h.memory_total_bytes > 0 THEN (CAST(h.memory_used_bytes AS REAL) * 100.0 / h.memory_total_bytes) ELSE NULL END AS host_memory_percent,
       hd.storage_percent AS host_storage_percent,
       CASE WHEN ds.state IN ('ok', 'error') THEN dd.size_bytes ELSE NULL END AS node_data_directory_size_bytes,
       CASE WHEN ds.state IN ('ok', 'error') AND dc.state IN ('ok', 'error') THEN dd.capacity_bytes ELSE NULL END AS node_data_directory_capacity_bytes,
       h.network_rx_bytes_per_sec AS host_network_rx_bytes_per_sec,
       h.network_tx_bytes_per_sec AS host_network_tx_bytes_per_sec,
       ps.state AS peer_state, ps.observed_at AS peer_observed_at,
       ps.value_received_at AS peer_value_received_at,
       ps.value_revision AS peer_value_revision,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.peer_count, 0) ELSE NULL END AS peer_count,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.inbound_count, 0) ELSE NULL END AS peer_inbound_count,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.outbound_count, 0) ELSE NULL END AS peer_outbound_count,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.trusted_count, 0) ELSE NULL END AS peer_trusted_count,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.static_count, 0) ELSE NULL END AS peer_static_count,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.consensus_count, 0) ELSE NULL END AS peer_consensus_count,
       co.observed_at AS consensus_observed_at,
       co.value_received_at AS consensus_value_received_at,
       co.value_revision AS consensus_value_revision,
       c.consensus_epoch,
       c.consensus_view_number,
       c.consensus_validator,
       c.consensus_highest_qc_block,
       c.consensus_highest_lock_block,
       c.consensus_highest_commit_block,
       c.current_block AS current_head,
       bs.transaction_count AS latest_block_transaction_count,
       hs.historical_high_watermark,
       hs.resync_state, hs.resync_last_progress_at,
       nr.block_number AS network_reference_head, nr.confidence AS network_reference_confidence
  FROM nodes n
  JOIN agents a ON a.agent_id = n.agent_id
  JOIN networks r ON r.network_key = n.network_key
  LEFT JOIN component_status s ON s.node_id = n.node_id AND s.component_key = 'rpc'
  LEFT JOIN component_status sy ON sy.node_id = n.node_id AND sy.component_key = 'sync'
  LEFT JOIN component_status co ON co.node_id = n.node_id AND co.component_key = 'consensus'
  LEFT JOIN component_status p ON p.node_id = n.node_id AND p.component_key = 'process'
  LEFT JOIN component_status ds ON ds.node_id = n.node_id AND ds.component_key = 'datadirectorysizebytes'
  LEFT JOIN component_status dc ON dc.node_id = n.node_id AND dc.component_key = 'datadirectorycapacitybytes'
  LEFT JOIN component_status i ON i.node_id = n.node_id AND i.component_key = 'network_identity'
  LEFT JOIN component_status ps ON ps.node_id = n.node_id AND ps.component_key = 'peers'
  LEFT JOIN (
       SELECT node_id,
              COUNT(*) AS peer_count,
              SUM(CASE WHEN direction = 'inbound' THEN 1 ELSE 0 END) AS inbound_count,
              SUM(CASE WHEN direction = 'outbound' THEN 1 ELSE 0 END) AS outbound_count,
              SUM(CASE WHEN trusted = 1 THEN 1 ELSE 0 END) AS trusted_count,
              SUM(CASE WHEN static_peer = 1 THEN 1 ELSE 0 END) AS static_count,
              SUM(CASE WHEN consensus_peer = 1 THEN 1 ELSE 0 END) AS consensus_count
         FROM current_node_peers
        GROUP BY node_id
  ) pc ON pc.node_id = n.node_id
  LEFT JOIN current_host_observations h ON h.agent_id = n.agent_id
  LEFT JOIN current_node_process_observations proc ON proc.node_id = n.node_id
  LEFT JOIN (
       SELECT agent_id,
              MAX(CASE WHEN total_bytes > 0 THEN (CAST(used_bytes AS REAL) * 100.0 / total_bytes) ELSE NULL END) AS storage_percent
         FROM current_host_disk_mounts
        GROUP BY agent_id
  ) hd ON hd.agent_id = n.agent_id
  LEFT JOIN current_node_data_directory_observations dd ON dd.node_id = n.node_id
  LEFT JOIN current_node_chain_observations c ON c.node_id = n.node_id
  LEFT JOIN block_summaries bs ON bs.node_id = n.node_id
       AND bs.block_number = (SELECT MAX(latest.block_number) FROM block_summaries latest WHERE latest.node_id = n.node_id)
  LEFT JOIN block_history_state hs ON hs.node_id = n.node_id
  LEFT JOIN network_reference_heads nr ON nr.network_key = n.network_key"#;

fn public_node_query(filter: &str, order: &str) -> String {
    format!("{PUBLIC_NODE_QUERY_BASE} WHERE {filter} ORDER BY {order}")
}

/// Public Home projection. The query boundary only selects public, active
/// Nodes and never returns endpoint, Agent, host identity, capacity, or raw
/// errors from the Admin projection.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicValidatorHistoryEntry {
    pub kind: String,
    pub observed_at: String,
    pub provider_timestamp: Option<String>,
    pub previous_rank: Option<i64>,
    pub current_rank: Option<i64>,
    pub counter_name: Option<String>,
    pub previous_value: Option<String>,
    pub current_value: Option<String>,
    pub link_roles: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicValidatorHistoryResponse {
    pub validator_id: String,
    pub entries: Vec<PublicValidatorHistoryEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicValidatorDailySnapshot {
    pub local_date: String,
    pub month_key: String,
    pub timezone: String,
    pub sample_at: String,
    pub rank: Option<i64>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicValidatorMonthlyAggregate {
    pub month_key: String,
    pub timezone: String,
    pub snapshot_count: i64,
    pub first_sample_at: String,
    pub last_sample_at: String,
    pub rank_min: Option<i64>,
    pub rank_max: Option<i64>,
    pub rank_last: Option<i64>,
    pub stake_last: Option<String>,
    pub reward_last: Option<String>,
    pub reward_rate_last: Option<String>,
    pub delegator_count_last: Option<i64>,
    pub epoch_last: Option<i64>,
    pub block_count_last: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicValidatorAnalyticsResponse {
    pub validator_id: String,
    pub state: String,
    pub freshness: String,
    pub daily: Vec<PublicValidatorDailySnapshot>,
    pub monthly: Vec<PublicValidatorMonthlyAggregate>,
}
#[utoipa::path(
    get,
    path = "/api/public/v1/validators/{validator_id}/analytics",
    tag = "public",
    params(("validator_id" = String, Path, description = "Validator ID"), ("limit" = Option<i64>, Query, minimum = 1, maximum = 366)),
    responses((status = 200, body = PublicValidatorAnalyticsResponse), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn public_validator_analytics(
    State(state): State<AppState>,
    Path(validator_id): Path<String>,
    Query(query): Query<ValidatorHistoryQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let Some(_validator) = (match validator::get_validator(state.db(), &validator_id).await {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    }) else {
        return error_response(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    let now = format_rfc3339(crate::auth::now_utc());
    let visible = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM node_validator_links l JOIN nodes n ON n.node_id = l.node_id WHERE l.validator_id = ? AND l.valid_from <= ? AND (l.valid_until IS NULL OR l.valid_until > ?) AND n.visibility = 'public' AND n.lifecycle = 'active' LIMIT 1",
    )
    .bind(&validator_id)
    .bind(&now)
    .bind(&now)
    .fetch_optional(state.db().pool())
    .await;
    if !matches!(visible, Ok(Some(_))) {
        return error_response(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    }
    let insight = match validator::load_insight(state.db(), &validator_id).await {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let limit = query.limit.unwrap_or(31).clamp(1, 366);
    let daily = match validator::list_daily_snapshots(state.db(), &validator_id, limit).await {
        Ok(rows) => rows
            .into_iter()
            .map(|row| PublicValidatorDailySnapshot {
                local_date: row.local_date,
                month_key: row.month_key,
                timezone: row.timezone,
                sample_at: row.sample_at,
                rank: row.rank,
                stake_amount: row.stake_amount,
                reward_amount: row.reward_amount,
                reward_rate: row.reward_rate,
                delegator_count: row.delegator_count,
                epoch: row.epoch,
                block_count: row.block_count,
            })
            .collect(),
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let monthly = match validator::list_monthly_aggregates(state.db(), &validator_id, limit).await {
        Ok(rows) => rows
            .into_iter()
            .map(|row| PublicValidatorMonthlyAggregate {
                month_key: row.month_key,
                timezone: row.timezone,
                snapshot_count: row.snapshot_count,
                first_sample_at: row.first_sample_at,
                last_sample_at: row.last_sample_at,
                rank_min: row.rank_min,
                rank_max: row.rank_max,
                rank_last: row.rank_last,
                stake_last: row.stake_last,
                reward_last: row.reward_last,
                reward_rate_last: row.reward_rate_last,
                delegator_count_last: row.delegator_count_last,
                epoch_last: row.epoch_last,
                block_count_last: row.block_count_last,
            })
            .collect(),
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let freshness = insight
        .as_ref()
        .map(|row| {
            validator::freshness(row.last_good_received_at.as_deref(), crate::auth::now_utc())
        })
        .unwrap_or("unknown");
    let state_value = insight
        .as_ref()
        .map(|row| {
            if row.outcome == "success" {
                freshness
            } else {
                row.outcome.as_str()
            }
        })
        .unwrap_or("unknown");
    Json(PublicValidatorAnalyticsResponse {
        validator_id,
        state: state_value.to_owned(),
        freshness: freshness.to_owned(),
        daily,
        monthly,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/api/public/v1/validators/{validator_id}/history",
    tag = "public",
    params(("validator_id" = String, Path, description = "Validator ID"), ("limit" = Option<i64>, Query, minimum = 1, maximum = 200)),
    responses((status = 200, body = PublicValidatorHistoryResponse), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn public_validator_history(
    State(state): State<AppState>,
    Path(validator_id): Path<String>,
    Query(query): Query<ValidatorHistoryQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let Some(validator) = (match validator::get_validator(state.db(), &validator_id).await {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    }) else {
        return error_response(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    let now = format_rfc3339(crate::auth::now_utc());
    let visible = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM node_validator_links l JOIN nodes n ON n.node_id = l.node_id WHERE l.validator_id = ? AND l.valid_from <= ? AND (l.valid_until IS NULL OR l.valid_until > ?) AND n.visibility = 'public' AND n.lifecycle = 'active' LIMIT 1",
    )
    .bind(&validator_id)
    .bind(&now)
    .bind(&now)
    .fetch_optional(state.db().pool())
    .await;
    if !matches!(visible, Ok(Some(_))) {
        return error_response(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    }

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let rankings = match validator::list_ranking_history(state.db(), &validator_id, limit).await {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let counters = match validator::list_counter_history(state.db(), &validator_id, limit).await {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let mut entries = Vec::with_capacity(rankings.len() + counters.len());
    for record in rankings {
        let links = match validator::list_link_context_at(
            state.db(),
            &validator_id,
            &record.observed_at,
            true,
        )
        .await
        {
            Ok(value) => value,
            Err(_) => {
                return error_response(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
        };
        if !links.is_empty() {
            entries.push(PublicValidatorHistoryEntry {
                kind: "ranking_changed".to_owned(),
                observed_at: record.observed_at,
                provider_timestamp: record.provider_timestamp,
                previous_rank: record.previous_rank,
                current_rank: Some(record.current_rank),
                counter_name: None,
                previous_value: None,
                current_value: None,
                link_roles: links.into_iter().map(|link| link.role).collect(),
            });
        }
    }
    for record in counters {
        let links = match validator::list_link_context_at(
            state.db(),
            &validator_id,
            &record.observed_at,
            true,
        )
        .await
        {
            Ok(value) => value,
            Err(_) => {
                return error_response(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
        };
        if !links.is_empty() {
            entries.push(PublicValidatorHistoryEntry {
                kind: "counter_reset_or_correction".to_owned(),
                observed_at: record.observed_at,
                provider_timestamp: record.provider_timestamp,
                previous_rank: None,
                current_rank: None,
                counter_name: Some(record.counter_name),
                previous_value: Some(record.previous_value),
                current_value: Some(record.current_value),
                link_roles: links.into_iter().map(|link| link.role).collect(),
            });
        }
    }
    entries.sort_by(|left, right| right.observed_at.cmp(&left.observed_at));
    entries.truncate(limit as usize);
    Json(PublicValidatorHistoryResponse {
        validator_id: validator.validator_id,
        entries,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/api/public/v1/networks",
    tag = "public",
    responses((status = 200, description = "Published Network and Node projection", body = [PublicNetwork]))
)]
pub(crate) async fn public_networks(State(state): State<AppState>) -> Response {
    let rows = sqlx::query_as::<_, PublicNodeRow>(&public_node_query(
        "n.visibility = 'public' AND n.lifecycle = 'active'",
        "r.network_key, n.node_id",
    ))
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
                peers: unknown_public_peer_insight(),
                geo: unknown_public_geo_insight(),
                validators: Vec::new(),
                nodes: vec![node],
            });
        }
    }
    for network in &mut networks {
        network.peers = aggregate_peer_insight(&network.nodes);
        network.geo = public_country_distribution(&state, &network.network_key).await;
        match public_validator_insights(&state, &network.network_key).await {
            Ok(validators) => {
                match effective_public_links(&state, None, Some(&network.network_key)).await {
                    Ok(links) => {
                        network.validators = validators;
                        associate_node_validators(&network.validators, &links, &mut network.nodes);
                    }
                    Err(_) => {
                        return error_response(
                            "unknown",
                            StatusCode::SERVICE_UNAVAILABLE,
                            "unavailable",
                            "server database is unavailable",
                        );
                    }
                }
            }
            Err(_) => {
                return error_response(
                    "unknown",
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
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
    let rows = sqlx::query_as::<_, PublicNodeRow>(&public_node_query(
        "n.network_key = ? AND n.visibility = 'public' AND n.lifecycle = 'active'",
        "n.node_id",
    ))
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
    let mut nodes = rows
        .into_iter()
        .map(|row| public_node(row).1)
        .collect::<Vec<_>>();
    let peers = aggregate_peer_insight(&nodes);
    let geo = public_country_distribution(&state, &network_key).await;
    let validators = match public_validator_insights(&state, &network_key).await {
        Ok(validators) => validators,
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let links = match effective_public_links(&state, None, Some(&network_key)).await {
        Ok(links) => links,
        Err(_) => {
            return error_response(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    associate_node_validators(&validators, &links, &mut nodes);
    Json(PublicNetwork {
        network_key,
        display_name,
        peers,
        geo,
        validators,
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
    let row = sqlx::query_as::<_, PublicNodeRow>(&public_node_query(
        "n.node_id = ? AND n.visibility = 'public' AND n.lifecycle = 'active'",
        "n.node_id",
    ))
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await;
    match row {
        Ok(Some(row)) => {
            let (_, mut node) = public_node(row);
            let network_key = node.network_key.clone();
            if let Ok(links) = effective_public_links(&state, Some(&node.node_id), None).await {
                if let Some(link) = links.first() {
                    if let Ok(validators) = public_validator_insights(&state, &network_key).await {
                        if let Some(validator) = validators
                            .into_iter()
                            .find(|validator| validator.validator_id == link.validator_id)
                        {
                            let mut associated = validator;
                            associated.node_id = Some(link.node_id.clone());
                            associated.link_role = Some(link.role.clone());
                            node.validator = Some(associated);
                        }
                    }
                }
            }
            Json(node).into_response()
        }
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
/// Non-sensitive Public access projection: the Site Access Mode and durable
/// authorization generation. This is the only public
/// route reachable without a Session in both modes: the WebUI needs it to
/// decide whether an anonymous visitor may render Home or must sign in. It
/// carries no DTO from any other namespace and no session material.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicAccessSettings {
    pub mode: String,
    pub authorization_generation: i64,
}

#[utoipa::path(
    get,
    path = "/api/public/v1/access",
    tag = "public",
    responses((status = 200, description = "The current Site Access Mode and authorization generation", body = PublicAccessSettings))
)]
pub(crate) async fn public_access_settings(State(state): State<AppState>) -> Response {
    match tokio::try_join!(
        crate::auth::site_access_mode(state.db()),
        crate::auth::authorization_generation(state.db()),
    ) {
        Ok((mode, generation)) => Json(PublicAccessSettings {
            mode: mode.as_str().to_owned(),
            authorization_generation: generation,
        })
        .into_response(),
        Err(_) => error_response(
            "unknown",
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
        .route("/access", get(public_access_settings))
        .route(
            "/validators/{validator_id}/analytics",
            get(public_validator_analytics),
        )
        .route(
            "/validators/{validator_id}/history",
            get(public_validator_history),
        )
        .route("/networks", get(public_networks))
        .route("/networks/{network_key}", get(public_network))
        .route("/nodes/{node_id}", get(public_node_detail))
        .route("/nodes/{node_id}/history", get(public_node_history))
        .route("/nodes/{node_id}/metrics", get(public_node_metrics))
        .route(
            "/nodes/{node_id}/peer-history",
            get(public_node_peer_history),
        )
        .route(
            "/nodes/{node_id}/history/export",
            get(public_node_history_export),
        )
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

    #[test]
    fn geo_timing_reports_age_and_stale_boundary() {
        let build_epoch = 1_700_000_000;
        let now =
            time::OffsetDateTime::from_unix_timestamp(build_epoch + 31 * 24 * 60 * 60).unwrap();
        let status = crate::geo::GeoStatus {
            state: "stale".to_owned(),
            configured: true,
            build_epoch: Some(build_epoch as u64),
            digest: Some("digest".to_owned()),
            loaded_at: Some("2026-01-01T00:00:00Z".to_owned()),
            last_error: None,
        };
        let (age_seconds, stale_since) = geo_timing(&status, now);
        assert_eq!(age_seconds, Some(31 * 24 * 60 * 60));
        assert_eq!(
            stale_since,
            Some(crate::auth::format_rfc3339(
                time::OffsetDateTime::from_unix_timestamp(build_epoch + 30 * 24 * 60 * 60,)
                    .unwrap(),
            )),
        );
    }

    #[test]
    fn history_bounds_reject_invalid_ranges() {
        let request_id = "test";
        assert_eq!(
            history_bounds(
                &PublicBlockHistoryQuery {
                    limit: Some(10),
                    from: Some(20),
                    to: Some(10),
                },
                request_id,
            )
            .unwrap_err()
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            history_bounds(
                &PublicBlockHistoryQuery {
                    limit: Some(10),
                    from: Some(-1),
                    to: None,
                },
                request_id,
            )
            .unwrap_err()
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            history_bounds(
                &PublicBlockHistoryQuery {
                    limit: Some(10),
                    from: Some(10),
                    to: Some(20),
                },
                request_id,
            )
            .unwrap(),
            (Some(10), Some(20))
        );
    }

    #[tokio::test]
    async fn public_geo_error_is_stable_and_non_sensitive() {
        let (dir, state) = test_state().await;
        let loader = std::sync::Arc::new(crate::geo::GeoLoader::new(Some(
            dir.path().join("missing-geolite.mmdb"),
        )));
        let state = state.with_geo_loader(loader);
        let insight = public_country_distribution(&state, "mainnet").await;
        assert_eq!(insight.state, "error");
        assert_eq!(insight.error_reason.as_deref(), Some(PUBLIC_GEO_ERROR));
        assert!(
            !insight
                .error_reason
                .as_deref()
                .unwrap()
                .contains("missing-geolite")
        );
    }

    async fn seed_public_data(state: &AppState) {
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, last_received_at, created_at, updated_at) VALUES ('agent-public-test', 1, '2026-01-01T00:00:05Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:05Z')")
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
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, received_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-public', 'node-public', 'process', 'ok', '2026-01-01T00:00:00Z', 1, 1)")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO current_host_observations (agent_id, cpu_percent, memory_total_bytes, memory_used_bytes, updated_at) VALUES ('agent-public-test', 42.5, 10000, 5000, '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO current_node_process_observations (node_id, pid, started_at, cpu_percent, memory_bytes, uptime_ms, updated_at) VALUES ('node-public', 100, '2026-01-01T00:00:00Z', 12.5, 2500, 1000, '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        for (mount_path, total_bytes, used_bytes) in
            [("/", 1_000_i64, 500_i64), ("/data", 2_000_i64, 1_600_i64)]
        {
            sqlx::query("INSERT INTO current_host_disk_mounts (agent_id, mount_path, total_bytes, used_bytes, updated_at) VALUES ('agent-public-test', ?, ?, ?, '2026-01-01T00:00:00Z')")
                .bind(mount_path)
                .bind(total_bytes)
                .bind(used_bytes)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
    }

    async fn seed_block_summary(
        state: &AppState,
        node_id: &str,
        height: i64,
        transaction_count: i64,
    ) {
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES (?, ?, ?, '0xparent', '0xgenesis', 1, 1, 'lat', 1, ?, ?, 'subscription', '0x0000000000000000000000000000000000000000', 'unknown', 'unknown', 'test', ?)")
            .bind(node_id)
            .bind(height)
            .bind(format!("0x{node_id}-{height}"))
            .bind(&now)
            .bind(transaction_count)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn public_node_metrics_returns_real_recent_and_carried_samples() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::now_utc();
        let old = format_rfc3339(now - time::Duration::seconds(90));
        let recent = format_rfc3339(now - time::Duration::seconds(10));
        for (metric, observed_at, received_at, value) in [
            (
                "process_cpu_percent",
                "2026-01-01T00:00:00Z",
                old.as_str(),
                10.0,
            ),
            (
                "process_cpu_percent",
                "2026-01-01T00:01:00Z",
                recent.as_str(),
                20.0,
            ),
            (
                "data_directory_percent",
                "2026-01-01T00:00:00Z",
                old.as_str(),
                25.0,
            ),
        ] {
            sqlx::query("INSERT INTO node_metric_samples (node_id, metric, observed_at, received_at, value) VALUES ('node-public', ?, ?, ?, ?)")
                .bind(metric)
                .bind(observed_at)
                .bind(received_at)
                .bind(value)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO host_metric_samples (agent_id, metric, observed_at, received_at, value) VALUES ('agent-public-test', 'network_rx_bytes_per_sec', '2026-01-01T00:01:00Z', ?, 2048)")
            .bind(&recent)
            .execute(state.db().pool())
            .await
            .unwrap();
        seed_block_summary(&state, "node-public", 99, 3).await;
        seed_block_summary(&state, "node-public", 100, 7).await;
        sqlx::query("UPDATE block_summaries SET block_timestamp_ms=CASE block_number WHEN 99 THEN 1000 ELSE 3000 END, observed_at=CASE block_number WHEN 99 THEN ? ELSE ? END, accepted_at=CASE block_number WHEN 99 THEN ? ELSE ? END WHERE node_id='node-public' AND block_number IN (99, 100)")
            .bind(&old)
            .bind(&recent)
            .bind(&old)
            .bind(&recent)
            .execute(state.db().pool())
            .await
            .unwrap();

        let response = public_node_metrics(
            State(state.clone()),
            Path("node-public".to_owned()),
            Extension(RequestId(std::sync::Arc::from("metric-history-test"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let history: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(history["windowSeconds"], 60);
        assert_eq!(history["processCpuPercent"].as_array().unwrap().len(), 2);
        assert_eq!(history["processCpuPercent"][0]["value"], 10.0);
        assert_eq!(history["processCpuPercent"][1]["value"], 20.0);
        assert_eq!(history["dataDirectoryPercent"][0]["value"], 25.0);
        assert_eq!(history["networkRxBytesPerSec"][0]["value"], 2048.0);
        assert_eq!(history["blockIntervalMs"][0]["value"], 2000.0);
        assert_eq!(history["transactionCount"][1]["value"], 7.0);

        let hidden = public_node_metrics(
            State(state),
            Path("node-private".to_owned()),
            Extension(RequestId(std::sync::Arc::from("metric-history-hidden"))),
        )
        .await;
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    }

    async fn seed_exact_head_transaction_fixtures(state: &AppState) {
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        for node_id in [
            "node-exact",
            "node-missing",
            "node-behind",
            "node-ahead",
            "node-other",
        ] {
            sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, 'agent-public-test', 'mainnet', ?, 'ws://127.0.0.1:1', 'active', 'public', 1, ?, ?)")
                .bind(node_id)
                .bind(format!("{node_id} display"))
                .bind(&now)
                .bind(&now)
                .execute(state.db().pool())
                .await
                .unwrap();
            sqlx::query("INSERT INTO current_node_chain_observations (node_id, current_block, updated_at) VALUES (?, 100, ?)")
                .bind(node_id)
                .bind(&now)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
        for (node_id, watermark) in [
            ("node-exact", 100_i64),
            ("node-missing", 100_i64),
            ("node-behind", 99_i64),
            ("node-ahead", 200_i64),
            ("node-other", 100_i64),
        ] {
            sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, updated_at) VALUES (?, ?, ?)")
                .bind(node_id)
                .bind(watermark)
                .bind(&now)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
        for (node_id, height, transaction_count) in [
            ("node-exact", 100_i64, 7_i64),
            ("node-behind", 99_i64, 9_i64),
            ("node-ahead", 101_i64, 11_i64),
            ("node-other", 100_i64, 13_i64),
        ] {
            seed_block_summary(state, node_id, height, transaction_count).await;
        }
    }

    #[tokio::test]
    async fn public_node_latest_block_summary_transaction_count_is_node_scoped() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        seed_exact_head_transaction_fixtures(&state).await;

        let response = public_networks(State(state)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let nodes = value[0]["nodes"].as_array().unwrap();
        let node = |node_id: &str| {
            nodes
                .iter()
                .find(|node| node["nodeId"].as_str() == Some(node_id))
                .unwrap_or_else(|| panic!("missing {node_id}"))
        };

        // HEAD stays the Sync observation Current Head. TXS independently
        // uses this Node's latest persisted Block Summary.
        assert_eq!(node("node-exact")["currentHead"], 100);
        assert_eq!(node("node-exact")["latestBlockTransactionCount"], 7);
        assert_eq!(node("node-missing")["currentHead"], 100);
        assert!(node("node-missing")["latestBlockTransactionCount"].is_null());
        assert_eq!(node("node-behind")["currentHead"], 100);
        assert_eq!(node("node-behind")["latestBlockTransactionCount"], 9);
        assert_eq!(node("node-ahead")["currentHead"], 100);
        assert_eq!(node("node-ahead")["latestBlockTransactionCount"], 11);
        assert_eq!(node("node-other")["currentHead"], 100);
        assert_eq!(node("node-other")["latestBlockTransactionCount"], 13);
    }

    #[tokio::test]
    async fn public_consensus_projection_is_node_scoped_and_last_good() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        seed_exact_head_transaction_fixtures(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        let stale =
            crate::auth::format_rfc3339(crate::auth::now_utc() - time::Duration::minutes(5));
        seed_consensus_fixture(&state, "node-cons-current-true", "ok", &now, 1, 10, 9, 8, 1).await;
        seed_consensus_fixture(&state, "node-cons-current-false", "ok", &now, 0, 0, 0, 0, 1).await;
        seed_consensus_fixture(
            &state,
            "node-cons-stale-true",
            "ok",
            &stale,
            1,
            20,
            19,
            18,
            1,
        )
        .await;
        seed_consensus_fixture(&state, "node-cons-stale-false", "ok", &stale, 0, 3, 2, 1, 1).await;
        // Failed collection with no accepted value: values exist in the raw
        // chain row but value_revision=0 means they are not projected.
        seed_consensus_fixture(&state, "node-cons-error-none", "error", &now, 0, 0, 0, 0, 0).await;

        let response = public_networks(State(state.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let nodes = value[0]["nodes"].as_array().unwrap();
        let node = |node_id: &str| {
            nodes
                .iter()
                .find(|node| node["nodeId"].as_str() == Some(node_id))
                .unwrap_or_else(|| panic!("missing {node_id}"))
        };

        // Current successful membership: true renders as a boolean.
        let current_true = &node("node-cons-current-true")["consensus"];
        assert_eq!(current_true["state"], "ok");
        assert_eq!(current_true["freshness"], "current");
        assert_eq!(current_true["validator"], true);
        assert_eq!(current_true["epoch"], 1);
        assert_eq!(current_true["viewNumber"], 2);
        assert_eq!(current_true["highestQcBlock"], 10);
        assert_eq!(current_true["highestLockBlock"], 9);
        assert_eq!(current_true["highestCommitBlock"], 8);
        assert!(current_true["staleSince"].is_null());

        // Current successful non-membership plus authoritative zero heights.
        let current_false = &node("node-cons-current-false")["consensus"];
        assert_eq!(current_false["freshness"], "current");
        assert_eq!(current_false["validator"], false);
        assert_eq!(current_false["highestQcBlock"], 0);
        assert_eq!(current_false["highestLockBlock"], 0);
        assert_eq!(current_false["highestCommitBlock"], 0);

        // Stale last-good true retains membership and block heights.
        let stale_true = &node("node-cons-stale-true")["consensus"];
        assert_eq!(stale_true["freshness"], "stale");
        assert_eq!(stale_true["validator"], true);
        assert_eq!(stale_true["highestQcBlock"], 20);
        assert_eq!(stale_true["highestLockBlock"], 19);
        assert_eq!(stale_true["highestCommitBlock"], 18);
        assert!(stale_true["staleSince"].is_string());

        // Stale last-good false retains an authoritative false.
        let stale_false = &node("node-cons-stale-false")["consensus"];
        assert_eq!(stale_false["freshness"], "stale");
        assert_eq!(stale_false["validator"], false);
        assert_eq!(stale_false["highestQcBlock"], 3);
        assert_eq!(stale_false["highestLockBlock"], 2);
        assert_eq!(stale_false["highestCommitBlock"], 1);

        // Never-observed is Unknown, never zero/False, even when other chain
        // observations exist and even when membership was withheld.
        for node_id in ["node-public", "node-exact"] {
            let consensus = &node(node_id)["consensus"];
            assert_eq!(consensus["state"], "unknown", "{node_id}");
            assert_eq!(consensus["freshness"], "unknown", "{node_id}");
            assert!(consensus["validator"].is_null(), "{node_id}");
            assert!(consensus["highestQcBlock"].is_null(), "{node_id}");
            assert!(consensus["highestLockBlock"].is_null(), "{node_id}");
            assert!(consensus["highestCommitBlock"].is_null(), "{node_id}");
        }
        // node-exact has a chain observation row but no accepted consensus
        // value; nulls prove values do not default to zero.
        assert!(node("node-exact")["consensus"]["validator"].is_null());

        // A failed collection without an accepted value is Unknown, never
        // False or zero, and is not certified as current.
        let error_none = &node("node-cons-error-none")["consensus"];
        assert_eq!(error_none["state"], "error");
        assert_eq!(error_none["freshness"], "unknown");
        assert!(error_none["validator"].is_null());
        assert!(error_none["highestQcBlock"].is_null());
        assert!(error_none["highestLockBlock"].is_null());
        assert!(error_none["highestCommitBlock"].is_null());

        // A failed collection keeps the last-good value and records error
        // state so Home can mark the retained row Stale.
        sqlx::query("UPDATE component_status SET state='error', state_revision=2 WHERE node_id='node-cons-current-true' AND component_key='consensus'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = public_node_detail(
            State(state),
            Path("node-cons-current-true".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["consensus"]["state"], "error");
        assert_eq!(value["consensus"]["validator"], true);
        assert_eq!(value["consensus"]["highestQcBlock"], 10);
    }

    #[allow(clippy::too_many_arguments)]
    async fn seed_consensus_fixture(
        state: &AppState,
        node_id: &str,
        component_state: &str,
        value_received_at: &str,
        validator: i64,
        qc: i64,
        locked: i64,
        committed: i64,
        value_revision: i64,
    ) {
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, 'agent-public-test', 'mainnet', ?, 'ws://127.0.0.1:1', 'active', 'public', 1, ?, ?)")
            .bind(node_id)
            .bind(format!("{node_id} display"))
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, observed_at, received_at, value_received_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', ?, ?, 'consensus', ?, ?, ?, ?, 1, ?)")
            .bind(node_id)
            .bind(node_id)
            .bind(component_state)
            .bind(&now)
            .bind(&now)
            .bind(value_received_at)
            .bind(value_revision)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_node_chain_observations (node_id, current_block, consensus_epoch, consensus_view_number, consensus_validator, consensus_highest_qc_block, consensus_highest_lock_block, consensus_highest_commit_block, updated_at) VALUES (?, ?, 1, 2, ?, ?, ?, ?, ?)")
            .bind(node_id)
            .bind(qc.max(1))
            .bind(validator)
            .bind(qc)
            .bind(locked)
            .bind(committed)
            .bind(value_received_at)
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn public_peer_projection_is_bounded_and_preserves_dimensions() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, observed_at, received_at, value_received_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-public', 'node-public', 'peers', 'ok', ?, ?, ?, 1, 1)")
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        for (peer_id, direction, trusted, static_peer, consensus_peer) in [
            ("peer-inbound", "inbound", 1, 1, 0),
            ("peer-outbound", "outbound", 0, 0, 1),
        ] {
            sqlx::query("INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, client_name, updated_at) VALUES ('node-public', ?, '203.0.113.7', ?, ?, ?, ?, 'raw-client-name', ?)")
                .bind(peer_id)
                .bind(direction)
                .bind(trusted)
                .bind(static_peer)
                .bind(consensus_peer)
                .bind(&now)
                .execute(state.db().pool())
                .await
                .unwrap();
        }

        let response = public_networks(State(state.clone())).await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let node = &value[0]["nodes"][0];
        let peers = &node["peers"];
        assert_eq!(peers["state"], "ok");
        assert_eq!(peers["freshness"], "current");
        assert_eq!(peers["peerCount"], 2);
        assert_eq!(peers["inboundCount"], 1);
        assert_eq!(peers["outboundCount"], 1);
        assert_eq!(peers["trustedCount"], 1);
        assert_eq!(peers["staticCount"], 1);
        assert_eq!(peers["consensusCount"], 1);
        assert!(peers.get("peerId").is_none());
        assert!(peers.get("remoteIp").is_none());
        assert!(peers.get("clientName").is_none());
        assert_eq!(value[0]["peers"]["peerCount"], 2);
        assert_eq!(value[0]["geo"]["state"], "disabled");
        assert!(value[0]["geo"]["countries"].is_null());
        assert!(String::from_utf8_lossy(&body).find("203.0.113.7").is_none());

        let stale_received =
            crate::auth::format_rfc3339(crate::auth::now_utc() - time::Duration::minutes(5));
        sqlx::query("UPDATE component_status SET state='ok', state_revision=2, observed_at=?, received_at=?, value_received_at=? WHERE node_id='node-public' AND component_key='peers'")
            .bind(&now)
            .bind(&stale_received)
            .bind(&stale_received)
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = public_node_detail(
            State(state.clone()),
            Path("node-public".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["peers"]["freshness"], "stale");
        assert!(value["peers"]["staleSince"].is_string());
        assert_eq!(value["peers"]["receivedAt"], stale_received);
        assert_eq!(value["peers"]["peerCount"], 2);

        sqlx::query("UPDATE component_status SET state='error', state_revision=3 WHERE node_id='node-public' AND component_key='peers'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = public_node_detail(
            State(state.clone()),
            Path("node-public".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["peers"]["state"], "error");
        assert_eq!(value["peers"]["receivedAt"], stale_received);
        assert_eq!(value["peers"]["peerCount"], 2);

        sqlx::query("UPDATE component_status SET state='ok', state_revision=4, value_revision=2, observed_at=?, received_at=?, value_received_at=? WHERE node_id='node-public' AND component_key='peers'")
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM current_node_peers WHERE node_id='node-public'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = public_node_detail(
            State(state),
            Path("node-public".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["peers"]["state"], "ok");
        assert_eq!(value["peers"]["peerCount"], 0);
        assert_eq!(value["peers"]["inboundCount"], 0);
    }

    #[tokio::test]
    async fn public_peer_projection_does_not_expose_private_node_data() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, observed_at, received_at, value_received_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-private', 'node-private', 'peers', 'ok', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 1, 1)")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, updated_at) VALUES ('node-private', 'private-peer', '10.0.0.2', 'inbound', 1, 1, 1, '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = public_networks(State(state.clone())).await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value[0]["nodes"].as_array().unwrap().len(), 1);
        let private = public_node_detail(
            State(state.clone()),
            Path("node-private".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(private.status(), StatusCode::NOT_FOUND);

        let network = public_network(
            State(state.clone()),
            Path("mainnet".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        let body = to_bytes(network.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["nodes"].as_array().unwrap().len(), 1);
        assert!(
            String::from_utf8_lossy(&body)
                .find("private-peer")
                .is_none()
        );

        for node_id in ["node-private", "node-retired", "node-unknown"] {
            let history = public_node_history(
                State(state.clone()),
                Path(node_id.to_owned()),
                Query(PublicBlockHistoryQuery {
                    limit: None,
                    from: None,
                    to: None,
                }),
                Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
            )
            .await;
            assert_eq!(
                history.status(),
                StatusCode::NOT_FOUND,
                "history leaked {node_id}"
            );

            let export = public_node_history_export(
                State(state.clone()),
                Path(node_id.to_owned()),
                Query(PublicBlockHistoryQuery {
                    limit: None,
                    from: None,
                    to: None,
                }),
                Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
            )
            .await;
            assert_eq!(
                export.status(),
                StatusCode::NOT_FOUND,
                "export leaked {node_id}"
            );

            let peer_history = public_node_peer_history(
                State(state.clone()),
                Path(node_id.to_owned()),
                Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
            )
            .await;
            assert_eq!(
                peer_history.status(),
                StatusCode::NOT_FOUND,
                "peer history leaked {node_id}"
            );
        }
    }

    #[tokio::test]
    async fn public_process_resources_preserve_last_good_on_error_but_not_when_disabled() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;

        sqlx::query("UPDATE component_status SET state = 'error' WHERE node_id = 'node-public' AND component_key = 'process'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = public_node_detail(
            State(state.clone()),
            Path("node-public".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let node: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(node["processCpuPercent"], 12.5);
        assert_eq!(node["processMemoryPercent"], 25.0);
        assert_eq!(node["processStartedAt"], "2026-01-01T00:00:00Z");
        assert_eq!(node["processUptimeMs"], 1000);
        assert_eq!(node["lastReportAt"], "2026-01-01T00:00:05Z");

        sqlx::query("UPDATE component_status SET state = 'disabled' WHERE node_id = 'node-public' AND component_key = 'process'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = public_node_detail(
            State(state),
            Path("node-public".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let node: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(node["processCpuPercent"].is_null());
        assert!(node["processMemoryPercent"].is_null());
        assert!(node["processStartedAt"].is_null());
        assert!(node["processUptimeMs"].is_null());
        assert_eq!(node["lastReportAt"], "2026-01-01T00:00:05Z");
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
        sqlx::query("INSERT INTO peer_aggregate_5m (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind("node-public")
            .bind("2026-08-12T10:05:00Z")
            .bind(2_i64)
            .bind(10_i64)
            .bind(4_i64)
            .bind(6_i64)
            .bind(3_i64)
            .bind(2_i64)
            .bind(1_i64)
            .bind(7_i64)
            .bind(3_i64)
            .bind(2_i64)
            .bind(1_i64)
            .bind(2_i64)
            .bind(3_i64)
            .bind(0_i64)
            .bind(3_i64)
            .bind("2026-08-12T10:05:00Z")
            .bind("2026-08-12T10:09:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO peer_aggregate_5m_countries (node_id, bucket_start, country_code, peer_count) VALUES (?, ?, ?, ?)")
            .bind("node-public")
            .bind("2026-08-12T10:05:00Z")
            .bind("US")
            .bind(7_i64)
            .execute(state.db().pool())
            .await
            .unwrap();
        let peer_history = public_node_peer_history(
            State(state.clone()),
            Path("node-public".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(peer_history.status(), StatusCode::OK);
        let peer_history_body = to_bytes(peer_history.into_body(), usize::MAX)
            .await
            .unwrap();
        let peer_history_value: serde_json::Value =
            serde_json::from_slice(&peer_history_body).unwrap();
        assert_eq!(peer_history_value["state"], "ok");
        assert_eq!(peer_history_value["freshness"], "stale");
        assert_eq!(
            peer_history_value["fiveMinute"][0]["countries"][0]["countryCode"],
            "US"
        );
        assert!(peer_history_value["hourly"].is_array());
        assert!(peer_history_value["fiveMinute"][0].get("peerId").is_none());
        let peer_history_text = String::from_utf8_lossy(&peer_history_body);
        for forbidden in ["peer-a", "8.8.8.8", "provider-response"] {
            assert!(
                !peer_history_text.contains(forbidden),
                "public history leaked {forbidden}"
            );
        }
        assert_eq!(node["processCpuPercent"], 12.5);
        assert_eq!(node["processMemoryPercent"], 25.0);
        assert_eq!(node["processStartedAt"], "2026-01-01T00:00:00Z");
        assert_eq!(node["processUptimeMs"], 1000);
        assert_eq!(node["lastReportAt"], "2026-01-01T00:00:05Z");
        assert_eq!(node["hostCpuPercent"], 42.5);
        assert_eq!(node["hostMemoryPercent"], 50.0);
        assert_eq!(node["hostStoragePercent"], 80.0);
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

    #[tokio::test]
    async fn public_validator_projection_maps_link_columns_by_name() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-public-test', 'mainnet', 'validator-node', 'Public Validator', ?, ?)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, valid_from, created_at, updated_at) VALUES ('link-public-test', 'node-public', 'validator-public-test', 'primary', '2026-01-01T00:00:00Z', ?, ?)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, provider_timestamp, last_attempt_received_at, last_good_received_at, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, updated_at) VALUES ('validator-public-test', 'fixture', 'success', ?, ?, ?, 1, '100', '2', '0.1', 3, 4, 5, ?)")
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();

        let response = public_networks(State(state)).await;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let validator = &value[0]["validators"][0];
        assert_eq!(validator["nodeId"], "node-public");
        assert_eq!(validator["linkRole"], "primary");
        // A pre-activity success row is never fabricated into a canonical label.
        assert_eq!(validator["activity"], "unknown");
        assert_eq!(validator["activityState"], "unknown");
    }

    async fn seed_public_activity_node(state: &AppState, node_id: &str) {
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, 'agent-public-test', 'mainnet', ?, 'ws://127.0.0.1:1', 'active', 'public', 1, ?, ?)")
            .bind(node_id)
            .bind(format!("{node_id} display"))
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    async fn seed_validator_activity(
        state: &AppState,
        node_id: &str,
        validator_id: &str,
        outcome: &str,
        activity: Option<&str>,
        last_good_received_at: Option<&str>,
        valid_from: &str,
        valid_until: Option<&str>,
    ) {
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES (?, 'mainnet', ?, ?, ?, ?)")
            .bind(validator_id)
            .bind(format!("0x{validator_id}"))
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at) VALUES (?, ?, ?, 'primary', ?, ?, ?, ?)")
            .bind(format!("link-{validator_id}"))
            .bind(node_id)
            .bind(validator_id)
            .bind(valid_from)
            .bind(valid_until)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key, last_observation_key, updated_at) VALUES (?, 'fixture', ?, NULL, NULL, ?, ?, ?, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'normal', 'normal', NULL, NULL, 0, NULL, NULL, NULL, NULL, ?)")
            .bind(validator_id)
            .bind(outcome)
            .bind(activity)
            .bind(&now)
            .bind(last_good_received_at)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn public_node_activity_follows_only_effective_links_and_last_good_semantics() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        let stale_received =
            crate::auth::format_rfc3339(crate::auth::now_utc() - time::Duration::minutes(10));

        // Node without any link: no Activity is ever inferred.
        for node_id in ["node-no-link", "node-future-link", "node-ended-link"] {
            seed_public_activity_node(&state, node_id).await;
        }
        seed_validator_activity(
            &state,
            "node-future-link",
            "validator-future",
            "success",
            Some("producing"),
            Some(&now),
            "2099-01-01T00:00:00Z",
            None,
        )
        .await;
        seed_validator_activity(
            &state,
            "node-ended-link",
            "validator-ended",
            "success",
            Some("active"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            Some("2026-01-02T00:00:00Z"),
        )
        .await;

        for node_id in [
            "node-current",
            "node-stale",
            "node-observing",
            "node-not-found",
            "node-error-good",
            "node-error-none",
            "node-unsupported-good",
            "node-unsupported-none",
            "node-shared-a",
            "node-shared-b",
        ] {
            seed_public_activity_node(&state, node_id).await;
        }
        seed_validator_activity(
            &state,
            "node-current",
            "validator-current",
            "success",
            Some("producing"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        seed_validator_activity(
            &state,
            "node-stale",
            "validator-stale",
            "success",
            Some("active"),
            Some(&stale_received),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        seed_validator_activity(
            &state,
            "node-observing",
            "validator-observing",
            "empty",
            Some("locked"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        seed_validator_activity(
            &state,
            "node-not-found",
            "validator-not-found",
            "not_found",
            None,
            None,
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        seed_validator_activity(
            &state,
            "node-error-good",
            "validator-error-good",
            "error",
            Some("locked"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        seed_validator_activity(
            &state,
            "node-error-none",
            "validator-error-none",
            "error",
            None,
            None,
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        seed_validator_activity(
            &state,
            "node-unsupported-good",
            "validator-unsupported-good",
            "unsupported",
            Some("verifying"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        seed_validator_activity(
            &state,
            "node-unsupported-none",
            "validator-unsupported-none",
            "unsupported",
            None,
            None,
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        seed_validator_activity(
            &state,
            "node-shared-a",
            "validator-shared",
            "success",
            Some("producing"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        // The same Validator is explicitly linked to a second public Node:
        // Activity must be associated with every effective Link (#100).
        let now_link = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at) VALUES ('link-validator-shared-b', 'node-shared-b', 'validator-shared', 'standby', '2026-01-01T00:00:00Z', NULL, ?, ?)")
            .bind(&now_link)
            .bind(&now_link)
            .execute(state.db().pool())
            .await
            .unwrap();

        // Make the Provider-failed Node fully healthy so the assertion
        // proves Provider Activity never changes Node Health.
        for component in ["rpc", "sync", "consensus", "process"] {
            sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, received_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-error-good', 'node-error-good', ?, 'ok', ?, 1, 1)")
                .bind(component)
                .bind(&now)
                .execute(state.db().pool())
                .await
                .unwrap();
        }

        let response = public_networks(State(state.clone())).await;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let find_node = |node_id: &str| {
            value[0]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|node| node["nodeId"] == node_id)
                .unwrap()
                .clone()
        };

        // No effective explicit Link: no Activity is associated at all.
        for node_id in ["node-no-link", "node-future-link", "node-ended-link"] {
            let node = find_node(node_id);
            assert!(
                node["validator"].is_null(),
                "{node_id} must not have a linked Validator"
            );
        }

        // A current successful snapshot shows the canonical label as Current.
        let current = find_node("node-current");
        assert_eq!(current["validator"]["activity"], "producing");
        assert_eq!(current["validator"]["activityState"], "current");

        // A successful snapshot outside the freshness window is Stale.
        let stale = find_node("node-stale");
        assert_eq!(stale["validator"]["activity"], "active");
        assert_eq!(stale["validator"]["activityState"], "stale");

        // Authoritative empty/not-found outcomes are Observing, regardless
        // of any retained last-good Activity.
        let observing = find_node("node-observing");
        assert_eq!(observing["validator"]["activity"], "observing");
        assert_eq!(observing["validator"]["activityState"], "current");
        let not_found = find_node("node-not-found");
        assert_eq!(not_found["validator"]["activity"], "observing");

        // Provider Error with a last-good Activity keeps the label as Stale,
        // even when the last-good timestamp is still in the freshness window,
        // and Node Health stays healthy and independent.
        let errored = find_node("node-error-good");
        assert_eq!(errored["validator"]["activity"], "locked");
        assert_eq!(errored["validator"]["activityState"], "stale");
        assert_eq!(errored["health"], "healthy");

        // Provider Error without a last-good Activity is Unknown.
        let unknown = find_node("node-error-none");
        assert_eq!(unknown["validator"]["activity"], "unknown");
        assert_eq!(unknown["validator"]["activityState"], "unknown");

        // Provider Unsupported never projects a retained Activity: lack of
        // configured coverage is Unknown even after a last-good snapshot;
        // without a last-good Activity it is also Unknown (#101).
        let unsupported_good = find_node("node-unsupported-good");
        assert_eq!(unsupported_good["validator"]["activity"], "unknown");
        assert_eq!(unsupported_good["validator"]["activityState"], "unknown");
        let unsupported_none = find_node("node-unsupported-none");
        assert_eq!(unsupported_none["validator"]["activity"], "unknown");
        assert_eq!(unsupported_none["validator"]["activityState"], "unknown");

        // One Validator linked to two Nodes exposes Activity on both,
        // each with its own effective Link role.
        for (node_id, role) in [("node-shared-a", "primary"), ("node-shared-b", "standby")] {
            let shared = find_node(node_id);
            assert_eq!(
                shared["validator"]["validatorId"], "validator-shared",
                "{node_id}"
            );
            assert_eq!(shared["validator"]["activity"], "producing", "{node_id}");
            assert_eq!(shared["validator"]["activityState"], "current", "{node_id}");
            assert_eq!(shared["validator"]["linkRole"], role, "{node_id}");
        }

        // Provider Activity never appears in Server readiness: readiness
        // components never include a Validator/Provider dimension.
        let ready = crate::http::health::ready(State(state))
            .await
            .into_response();
        let ready_body = to_bytes(ready.into_body(), usize::MAX).await.unwrap();
        let ready_value: serde_json::Value = serde_json::from_slice(&ready_body).unwrap();
        assert!(
            ready_value["components"]
                .as_array()
                .unwrap()
                .iter()
                .all(|component| component["name"] != "validator")
        );
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

    async fn seed_public_analytics_row(state: &AppState, validator_id: &str, node_id: &str) {
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES (?, 'mainnet', ?, ?, ?, ?)")
            .bind(validator_id)
            .bind(format!("node-{validator_id}"))
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at) VALUES (?, ?, ?, 'primary', '2026-01-01T00:00:00Z', NULL, ?, ?)")
            .bind(format!("link-{validator_id}"))
            .bind(node_id)
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key, last_observation_key, updated_at) VALUES (?, 'explorer', 'success', NULL, ?, ?, ?, ?, 5, '1000', '10', '0.05', 8, 42, 100, 'normal', 'normal', NULL, NULL, 0, NULL, NULL, NULL, ?, ?)")
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count) VALUES (?, ?, 'UTC', '2026-01-01', '2026-01', ?, ?, ?, 'explorer', 'obs-1', 5, '1000', '10', '0.05', 8, 42, 100)")
            .bind(format!("snap-{validator_id}"))
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO validator_monthly_aggregates (aggregate_id, validator_id, timezone, month_key, snapshot_count, first_sample_at, last_sample_at, rank_min, rank_max, rank_last, stake_last, reward_last, reward_rate_last, delegator_count_last, epoch_last, block_count_last, updated_at) VALUES (?, ?, 'UTC', '2026-01', 1, ?, ?, 5, 5, 5, '1000', '10', '0.05', 8, 42, 100, ?)")
            .bind(format!("agg-{validator_id}"))
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn public_validator_analytics_is_sanitized_and_visibility_filtered() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        seed_public_analytics_row(&state, "validator-public", "node-public").await;
        seed_public_analytics_row(&state, "validator-private", "node-private").await;

        let response = public_validator_analytics(
            State(state.clone()),
            Path("validator-public".to_owned()),
            Query(ValidatorHistoryQuery { limit: None }),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["validatorId"], "validator-public");
        assert_eq!(value["daily"][0]["localDate"], "2026-01-01");
        assert_eq!(value["daily"][0]["rank"], 5);
        assert_eq!(value["monthly"][0]["monthKey"], "2026-01");
        assert_eq!(value["monthly"][0]["snapshotCount"], 1);
        // Public DTOs never carry Admin-only receipt/source fields.
        assert!(value["daily"][0].get("receivedAt").is_none());
        assert!(value["daily"][0].get("source").is_none());
        assert!(value["monthly"][0].get("updatedAt").is_none());

        for hidden in ["validator-private", "validator-unknown"] {
            let response = public_validator_analytics(
                State(state.clone()),
                Path(hidden.to_owned()),
                Query(ValidatorHistoryQuery { limit: None }),
                Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "analytics leaked {hidden}"
            );
        }
    }
}
