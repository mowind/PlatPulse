//! `/api/public/v1` route group — the Home Public Projection and human
//! session lifecycle (design §10.2, §8.4).
//!
//! Middleware and DTO namespace are independent from Admin and Agent: DTOs
//! live in this module and are never reused as Admin DTOs by runtime field
//! filtering (design §2.1). Guest access is disabled by default, so every
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
use crate::node_currency;
use crate::public_projection::{BlockHistoryRange, PublicProjection, Unavailable};

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
    // bound to the Site Access Mode (design §10.2).
    let stream: Pin<
        Box<dyn Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>> + Send>,
    > = match session {
        Some(Extension(session)) => Box::pin(state.public_realtime().stream_with_session(
            cursor,
            state.database(),
            session.0.session_id.clone(),
            session.0.role.clone(),
            session.0.expires_at,
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
/// (design §10.2). Fields are camelCase per the browser wire rule;
/// timestamps are RFC 3339 UTC.
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
/// limit, and Session ID rotation (design §10.2).
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
    // 1. Strict Origin validation (design §10.2): login carries no existing
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

    // 2. Setup gate (design §13.3): no Owner, no login.
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

    // 4. Independent login rate limit (design §10.2).
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

/// Logout: revoke the current Session and clear the cookie (design §10.2).
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
        let mut transaction = crate::database::begin(state.db().pool()).await?;
        let revoked = sqlx::query(
            "UPDATE sessions SET revoked_at = $1 WHERE session_id = $2 AND revoked_at IS NULL",
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

/// Current session projection and CSRF token for the WebUI (design §10.2).
/// `last_seen_at` is refreshed at most once per throttle window; it is
/// activity evidence only and never decides when the Session expires.
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
    /// Peer records in scope attributed to this country. Records are counted
    /// per Node and are never deduplicated by IP or Peer identity.
    pub count: i64,
    /// Of `count`, the records whose country result is retained last-good
    /// data past its cache lifetime. They keep their country and are never
    /// re-counted as Unknown.
    pub stale_count: i64,
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

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicGeoInsight {
    /// Server-owned Geo database state. Raw IPs and MMDB paths never cross
    /// the Public projection boundary. This is the database dimension only:
    /// it never claims that Peers are currently online or freshly observed.
    pub state: String,
    pub last_good_at: Option<String>,
    pub database_age_seconds: Option<u64>,
    pub stale_since: Option<String>,
    pub error_reason: Option<String>,
    /// How complete the Peer-record basis of this projection is: `complete`
    /// (every Active Node in this Network reported a successful Peer
    /// Snapshot), `partial` (some did), `unobserved` (none did), or
    /// `unavailable`
    /// (no country projection exists because Geo is Disabled or its read
    /// failed; `state` keeps those two apart).
    pub scope: String,
    /// Peer records in scope with a retained country attribution, including
    /// retained last-good rows reported as `staleCount` per country. Peer
    /// records are counted per Node and are never deduplicated by IP.
    /// `None` when there is no reliable denominator (`unobserved` or
    /// `unavailable` scope), so Unknown is never read as zero.
    pub known_country_count: Option<i64>,
    /// Peer records in the same scope with no retained country attribution.
    /// `None` under the same conditions as `known_country_count`.
    pub unknown_country_count: Option<i64>,
    /// The `known_country_count` + `unknown_country_count` denominator the
    /// Server actually observed: the same per-Node Peer-record total the
    /// aggregate Peer Insight reports whenever that total is available. The
    /// browser never subtracts an independent Peer total to derive Unknown.
    pub available_peer_count: Option<i64>,
    /// Of `unknown_country_count`, the records without a usable public
    /// remote IP.
    pub unknown_without_remote_ip_count: Option<i64>,
    /// Of `unknown_country_count`, the records that do have a usable public
    /// remote IP but no retained country result. Both reasons are computed
    /// here so the browser never derives one by subtraction; they are never
    /// split further than the Server can substantiate.
    pub unknown_with_public_ip_count: Option<i64>,
    pub countries: Option<Vec<PublicCountryCount>>,
    pub attribution: Option<String>,
}

/// The compact per-Node country buckets a Network response carries on every
/// listed Node. They are the source of the Node Selection Geo Aggregate: the
/// browser sums the buckets of the Nodes that match the current filters, so a
/// selection never needs a second request and never parses a country itself.
///
/// Provider and database state stay Network-level (`PublicGeoInsight`): they
/// are read once per response rather than per Node. `scope` follows the same
/// vocabulary restricted to what one Node can be: `complete`, `unobserved`
/// or `unavailable`. Every count is `None` whenever the Node has no reliable
/// denominator, so Unknown is never read as a zero.
#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicNodePeerCountries {
    pub scope: String,
    pub known_country_count: Option<i64>,
    pub unknown_country_count: Option<i64>,
    pub available_peer_count: Option<i64>,
    pub unknown_without_remote_ip_count: Option<i64>,
    pub unknown_with_public_ip_count: Option<i64>,
    pub countries: Vec<PublicCountryCount>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicPeerInsight {
    /// Agent-reported collection state. The WebUI presents this separately
    /// from freshness and value availability.
    pub state: String,
    /// Server-owned freshness of the last successful Peer Snapshot, judged on
    /// the Server-clock observation instant (ADR 0014).
    pub freshness: String,
    /// The last successful observation time. No peer identity or address is
    /// included in the Public projection.
    pub observed_at: Option<String>,
    /// Server receipt time for the last accepted Peer snapshot.
    pub received_at: Option<String>,
    /// Server-computed boundary at which the last observation became stale: the
    /// observation instant plus the freshness window.
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
    /// Server-owned freshness of the last successful consensus observation,
    /// judged on the Server-clock observation instant (ADR 0014).
    pub freshness: String,
    /// The last successful observation time.
    pub observed_at: Option<String>,
    /// Server receipt time for the last accepted consensus observation.
    pub received_at: Option<String>,
    /// Server-computed boundary at which the last observation became stale: the
    /// observation instant plus the freshness window.
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
    /// Deduplicated, exact current-selection Cumulative Validator totals for
    /// this Network. The browser selects which Network groups to show; it
    /// never adds up duplicate Node projections itself (#159).
    pub validator_summary: PublicValidatorSummary,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicValidatorInsight {
    pub validator_id: String,
    pub validator_node_id: String,
    pub display_name: Option<String>,
    pub node_id: Option<String>,
    pub state: String,
    pub freshness: String,
    pub source: Option<String>,
    pub provider_timestamp: Option<String>,
    pub received_at: Option<String>,
    /// PlatScan's position within this Network's complete live-staking ALL
    /// cohort (including candidates), adopted verbatim and independent of the
    /// monitored Node set or Home filters. `None` is never zero and never
    /// means unranked by itself: see `rank_state` (#158).
    pub rank: Option<i64>,
    /// `ranked`, `unranked`, `error`, `not_configured`, `unsupported`, or
    /// `unknown`. Only a complete successful list fetch can establish
    /// `unranked`; a failed, truncated, duplicated, or drifted fetch keeps the
    /// last-good rank and never fabricates an unranked outcome (#158).
    pub rank_state: String,
    /// `fresh`, `stale`, or `unknown` currency of the ranking result, tracked
    /// independently from the detail `freshness` (#158).
    pub rank_freshness: String,
    /// Server receipt time of the last successful ranking list fetch. It is
    /// distinct from `received_at`, which is the detail fetch time (#158).
    pub rank_received_at: Option<String>,
    /// Complete live-staking cohort size from the same successful ranking
    /// fetch; `None` when no complete list has been obtained (#158).
    pub rank_cohort_size: Option<i64>,
    pub stake_amount: Option<String>,
    /// Gross cumulative Validator rewards over chain history, including the
    /// operator and delegator allocations (PlatScan `rewardValue`). It
    /// excludes principal and ordinary transfers and is **not** the operator's
    /// net earnings: the Server never subtracts `totalDeleReward` and never
    /// derives a historical net value from the current distribution ratio.
    /// `None` means unknown, never zero (#155).
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    /// The currently effective delegation reward distribution percentage
    /// (PlatScan detail `rewardPer`), normalized to percentage points: the
    /// source's already-scaled `20` is 20%, never 0.20% or 2000%. It is
    /// **not** the annualized `reward_rate`, the operator commission, a
    /// pending next-period ratio, or a back-computed net earning. `None`
    /// means unknown; a legitimate `0` and `100` are preserved (#157).
    pub delegation_reward_percentage: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    /// Cumulative Validator produced blocks over chain history (PlatScan
    /// `blockQty`), not blocks observed by the monitored Node. `None` means
    /// never observed; `Some(0)` is an authoritative zero (#154).
    pub block_count: Option<i64>,
    /// Cumulative scheduled-block denominator from the **same** successful
    /// observation as `block_count` (PlatScan `expectBlockQty`). `None` means
    /// unknown; `Some(0)` is an authoritative "no scheduled duties" value that
    /// must never be presented as a rate (#156).
    pub expected_block_count: Option<i64>,
    /// Cumulative actual / cumulative scheduled blocks from the same
    /// observation, as a percentage string without the `%` sign (for example
    /// `90.909091`). The Server never mixes a fresh numerator with an older
    /// denominator. It is **not** an exact missed-block rate: whole-round
    /// scheduled duties are counted before they elapse. `None` is either
    /// unknown or not applicable — see `block_rate_state` (#156).
    pub block_rate: Option<String>,
    /// `ok`, `not_applicable`, or `unknown`. `not_applicable` is a known
    /// zero scheduled-block denominator (no production duties), never 0% or
    /// 100%; `unknown` means the pair is incomplete. Only `ok` carries a value
    /// in `block_rate` (#156).
    pub block_rate_state: String,
    /// PlatScan's own 24-hour production rate, normalized to a percentage
    /// string without the `%` sign and taken directly from `genBlocksRate`.
    /// The investigated source sums the preceding seven settlement periods
    /// excluding the current one, so it is not a strict rolling 86,400-second
    /// window, and a source-reported `0` can also mean absent evidence or an
    /// upstream error. `None` means unknown, never a synthesized zero (#156).
    pub gen_blocks_rate: Option<String>,
    pub counter_state: String,
    /// Canonical last-good Validator Activity (`candidate`, `active`,
    /// `producing`, `exiting`, `exited`, `verifying`, `locked`) or
    /// `observing`/`unknown`.
    /// Home never infers this from names, endpoints, consensus membership,
    /// rank data, or Provider data: only an effective explicit Node Validator
    /// Link exposes it on a Public Node (#100).
    pub activity: String,
    /// `current`, `stale`, or `unknown` currency of the Activity value.
    /// Provider failure with a last-good Activity is always `stale`, even
    /// when the last-good timestamp is still within the freshness window.
    pub activity_state: String,
    /// Receipt time of the last confirmed Activity/identity verdict, including
    /// authoritative absence. Independent of detail metrics received_at.
    pub activity_received_at: Option<String>,
    /// Server-owned Current Validator Status of the automatically identified
    /// chain identity (#173): `validator`, `not_validator`, or `unknown`.
    /// This is currently valid staking identity, not current consensus
    /// selection or Node Health. `not_validator` requires authoritative
    /// evidence (completed exit or a 200 empty identifier); HTTP 404,
    /// transport failures and unsupported coverage stay `unknown`.
    pub current_validator_status: String,
    /// `current`, `stale`, or `unknown` currency of the Current Validator
    /// Status. A retained last-good verdict after a failed refresh is `stale`.
    pub current_validator_status_state: String,
    /// `locked` or `exiting` while a confirmed-valid identity is not normally
    /// participating; null for an ordinary candidate/active/producing identity
    /// or for any unknown/negative status.
    pub current_validator_status_qualifier: Option<String>,
}

/// Per-Network cumulative total for one metric, deduplicated by Validator.
///
/// The Server owns the eligible set, the deduplication, the exact arithmetic,
/// and the coverage metadata, so Home never sums duplicate Node projections.
/// Coverage is tracked for blocks and rewards independently because one metric
/// can be missing while the other has a value (#159).
#[derive(Debug, Serialize, ToSchema, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicValidatorBlockTotal {
    /// Exact base-10 integer sum over the eligible Validators, serialized as a
    /// string to avoid both i64 overflow and JavaScript number precision loss.
    /// `None` means no eligible Validator reported a value, never a fabricated zero.
    pub known_sum: Option<String>,
    /// Distinct eligible Validators this metric should cover.
    pub expected_count: i64,
    /// Eligible Validators contributing a known value, including retained
    /// stale last-good values.
    pub valued_count: i64,
    /// Contributing Validators whose value is not current (a retained
    /// last-good value after a failed or aged collection).
    pub stale_count: i64,
    /// `complete`, `partial`, or `unknown` coverage of `expected_count`.
    /// `partial` is a known-values subtotal, not a complete total.
    pub state: String,
}

/// Exact-decimal counterpart of `PublicValidatorBlockTotal` for gross
/// cumulative Validator rewards. `known_sum` is a bounded decimal string in
/// the Network native unit; it never round-trips through binary floating point.
#[derive(Debug, Serialize, ToSchema, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicValidatorRewardTotal {
    pub known_sum: Option<String>,
    pub expected_count: i64,
    pub valued_count: i64,
    pub stale_count: i64,
    pub state: String,
}

/// Home's current-selection Cumulative Validator summary for one Network.
///
/// Membership is the Network's Active Nodes; temporarily offline Active Nodes
/// stay in scope and Retired Nodes do not. Every distinct Validator referenced
/// by an effective Node Validator Link is counted once regardless of primary,
/// standby, or observer role, and Validators are never shared across Networks.
/// Totals can fall when filters, effective Links, or Active membership change;
/// they are not an ownership ledger and are not guaranteed monotonic (#159).
#[derive(Debug, Serialize, ToSchema, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicValidatorSummary {
    pub blocks: PublicValidatorBlockTotal,
    pub rewards: PublicValidatorRewardTotal,
    /// Distinct eligible Validators referenced by an effective Link.
    pub eligible_validator_count: i64,
    /// Active Nodes with an effective Link.
    pub linked_node_count: i64,
    /// Active Nodes without any effective Link, reported separately so an
    /// unlinked Node is never mistaken for a zero-valued Validator.
    pub unlinked_node_count: i64,
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

pub(crate) const PUBLIC_NODE_METRIC_WINDOW_SECONDS: i64 = 60;

/// The Node Detail Metric Window (ADR 0012): the fixed presets the Public
/// metrics route answers. Never an arbitrary range: the enum is what bounds
/// the cost of an anonymous request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub(crate) enum PublicMetricWindow {
    /// The latest 60 seconds, answered exactly as before the Metric Window.
    #[serde(rename = "live")]
    Live,
    /// The latest hour, from raw observations.
    #[serde(rename = "1h")]
    OneHour,
    /// The latest six hours, from 1-minute buckets.
    #[serde(rename = "6h")]
    SixHours,
    /// The latest 24 hours, from 5-minute buckets.
    #[serde(rename = "24h")]
    TwentyFourHours,
    /// The latest seven days, from 5-minute buckets.
    #[serde(rename = "7d")]
    SevenDays,
}

impl PublicMetricWindow {
    /// The preset a request names. No parameter is Live; anything outside the
    /// enum is refused rather than answered as a nearby window.
    pub(crate) fn parse(raw: Option<&str>) -> Option<Self> {
        match raw {
            None | Some("live") => Some(Self::Live),
            Some("1h") => Some(Self::OneHour),
            Some("6h") => Some(Self::SixHours),
            Some("24h") => Some(Self::TwentyFourHours),
            Some("7d") => Some(Self::SevenDays),
            Some(_) => None,
        }
    }

    pub(crate) fn seconds(self) -> i64 {
        match self {
            Self::Live => PUBLIC_NODE_METRIC_WINDOW_SECONDS,
            Self::OneHour => 3_600,
            Self::SixHours => 6 * 3_600,
            Self::TwentyFourHours => 24 * 3_600,
            Self::SevenDays => 7 * 86_400,
        }
    }

    /// The tier the Server answers each longer preset from. Live is answered by
    /// its own latest-60-second read and never reaches a tier.
    pub(crate) fn tier(self) -> crate::metric_history::WindowTier {
        use crate::metric_history::WindowTier;
        match self {
            Self::Live | Self::OneHour => WindowTier::Raw,
            Self::SixHours => WindowTier::OneMinute,
            Self::TwentyFourHours | Self::SevenDays => WindowTier::FiveMinute,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PublicNodeMetricsQuery {
    pub window: Option<String>,
}

/// What one aggregate bucket of a longer Metric Window holds (ADR 0012).
///
/// A bucket stores a count, extremes and its newest value but no sum, so there
/// is no mean to draw: the point's `value` is the bucket's maximum, which is
/// the reading a monitor must not hide, and the rest of the bucket is stated
/// here for the tooltip.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicMetricBucket {
    /// The bucket's own interval: its UTC-aligned start, inclusive...
    pub start: String,
    /// ...and its end, exclusive.
    pub end: String,
    pub grain_seconds: i64,
    pub min_value: f64,
    pub max_value: f64,
    /// The newest observation the bucket counted.
    pub last_value: f64,
    /// Observations the bucket counted. A replay never advances it.
    pub sample_count: i64,
    pub first_observed_at: String,
    pub last_observed_at: String,
    /// The newest bucket, whose interval the answer has not reached the end of
    /// yet: it is still accumulating, so a later answer can widen it.
    pub open: bool,
    /// Present only on a Transactions / block bucket (issue #248): every
    /// transaction the bucket's blocks carried. Its `sampleCount` is then the
    /// number of blocks, so the reader can state both without a mean the
    /// chart would have to invent.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub total_value: Option<f64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicMetricPoint {
    /// The observation instant, or a bucket's start.
    pub sampled_at: String,
    /// The observed value, or a bucket's maximum.
    pub value: f64,
    /// Present only on a point of an aggregated window: the bucket the point
    /// stands for. A raw observation, including every Live point, has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub bucket: Option<PublicMetricBucket>,
}

/// Which Metric Window an answer is for, and at what grain the Server read
/// it. Absent from a Live answer, which keeps its latest-60-second shape field
/// for field.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicMetricWindowAnswer {
    pub window: PublicMetricWindow,
    /// `raw`, `1m` or `5m`: the one tier the whole window was read from.
    pub grain: String,
    /// The bucket width, 0 for raw observations.
    pub grain_seconds: i64,
    /// The series this window does not provide, by response field name, so a
    /// series without evidence at this grain is named rather than answered as
    /// an empty series that would read as nothing observed. Since the block
    /// aggregate tiers (issue #248) every window provides all nine series and
    /// the list is empty; it stays part of the answer so a reader keeps one
    /// place to ask.
    pub unavailable_series: Vec<String>,
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
    /// Live: each held block with a held parent. A longer window: the raw
    /// blocks (1h) or the block-tier buckets, each drawn by its longest
    /// interval (issue #248).
    pub block_interval_ms: Vec<PublicMetricPoint>,
    /// Live: each held block. A longer window: the raw blocks (1h) or the
    /// block-tier buckets, each drawn by the most transactions one of its
    /// blocks carried, with the bucket's total in `bucket.totalValue`.
    pub transaction_count: Vec<PublicMetricPoint>,
    /// The same nine series, judged: which of them were observed, where the
    /// silences are, and how much of the window is covered. One entry per
    /// series, in the order above, so `series` is the contract a chart reads
    /// before it draws a line (issue #225).
    pub series: Vec<PublicMetricSeriesCoverage>,
    /// The Metric Window of a longer preset (ADR 0012). Absent from Live.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub metric_window: Option<PublicMetricWindowAnswer>,
}
/// One stretch of the window with no observation in it.
///
/// The latest-60-second answer holds no protection pause, so what it reports is
/// always a collection gap the series itself proved. A longer Metric Window is
/// judged by the Admin long-history rule, which names a protection pause where
/// the Server recorded one (ADR 0012).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicMetricGap {
    /// The newest instant before the silence.
    pub from: String,
    /// The oldest instant after it: the first observation the answer carries,
    /// which is exactly where a chart may start drawing again.
    pub to: String,
    pub seconds: i64,
    /// `collection_gap` today, from the shared gap vocabulary the Server uses
    /// everywhere else (`GapKind::as_str`).
    pub kind: String,
}

/// What one series of the latest-60-second answer can vouch for.
///
/// The six charts are drawn from the observations above, and those observations
/// alone cannot say whether a straight line between two of them crosses a
/// stretch nobody observed. This is that answer, per series, so the chart draws
/// a break where the Server proved a silence, stops where the observations stop
/// instead of running to the right edge, and never fills a hole with a zero
/// (issue #225, Story 49). The figures are computed by the same rule the metric
/// history uses ([crate::metric_history]), with the silence floor of a 60-second
/// window rather than the two-minute floor of a window measured in hours.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicMetricSeriesCoverage {
    /// The response field this coverage belongs to (`processCpuPercent`,
    /// `blockIntervalMs`, ...), because the nine arrays carry no name of their
    /// own.
    pub metric: String,
    /// Whether the Server can vouch for any observation of this series: this
    /// answer carries one (the last-good point that precedes the window counts)
    /// or the series ledger remembers observations whose samples have since
    /// expired. `false` means the Server holds no evidence of this series, which
    /// is a proof that nothing was ever recorded only where
    /// `neverObservedProven` says it is one: where evidence may have expired
    /// without leaving a ledger, `false` is an unknown rather than a verdict
    /// (issue #225, Story 49).
    pub observed: bool,
    /// Whether `observed` of `false` here is a proof that nothing was ever
    /// recorded. The Server keeps a value-less observation ledger for every
    /// series it stores under a metric name, so a ledger with no row for this
    /// series is positive evidence of never-observed; the two block series have
    /// no ledger - `block_summaries` is their only record, and retention removes
    /// it - so once their rows expire the Server cannot tell a series that went
    /// quiet long ago from one nobody ever reported. It answers `false` in that
    /// case and this flag says so, rather than either claiming an observation it
    /// cannot prove or hiding one that may have happened.
    pub never_observed_proven: bool,
    /// How many observations fall inside the window itself. A series that is
    /// observed but counted zero here has not been observed recently - a sample
    /// that expired counts as much as a window that simply held none - which
    /// the tail and `lastObservedAt` date rather than a zero.
    pub observation_count: i64,
    /// The oldest observation of this series the answer can point at, from the
    /// observations it carries or from the ledger that outlives them.
    pub first_observed_at: Option<String>,
    /// The newest one, from either source. It is the `observedAt` a later
    /// answer would report as the last-good value, so `unobservedTailSeconds`
    /// measures from here. An instant the window has not reached is never one of
    /// them: a clock running ahead dates nothing, and may not hide the
    /// last-good point that really arrived (issue #225).
    pub last_observed_at: Option<String>,
    /// The cadence currently in force: the newest interval between two
    /// consecutive observations, trusted only while the interval before it
    /// agrees with it. 0 means the Server has no cadence verdict for this
    /// series, and an unknown cadence is answered as unknown rather than as
    /// silence.
    ///
    /// A series that went quiet inside the window and came back once lands
    /// here: the single interval after the silence disagrees with the cadence
    /// before it, and sixty seconds cannot tell an Agent that stopped reporting
    /// from an Agent whose collection interval was changed - inside one window
    /// those look alike - so no silence is named until the next observation
    /// settles it. Neither reading is invented in the meantime.
    pub cadence_seconds: i64,
    /// The silence a cadence makes a gap: three times the cadence, capped at
    /// five minutes of cadence. 0 with `cadenceSeconds` 0, because a series
    /// with no known rhythm is never called silent.
    pub gap_threshold_seconds: i64,
    /// The seconds of the window the observations between them prove were
    /// observed. The stretch after the newest observation is never part of it:
    /// nobody observed how long it is. 0 with `cadenceSeconds` 0, because
    /// without a rhythm the answer cannot tell a covered stretch from a silence
    /// and so claims neither.
    pub covered_seconds: i64,
    /// The seconds between the newest observation and the end of the window, so
    /// the chart knows the line must not be extended through them. It can exceed
    /// the window when the newest observation is the older last-good point: the
    /// series really has been quiet that long.
    pub unobserved_tail_seconds: i64,
    /// The silences inside the window, oldest first. Each one is a stretch a
    /// chart must break at rather than connect across.
    pub gaps: Vec<PublicMetricGap>,
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
    let range = BlockHistoryRange { from, to, limit };
    match public_projection(&state)
        .block_history(&node_id, range, crate::auth::now_utc())
        .await
    {
        Ok(Some(items)) => Json(items).into_response(),
        Ok(None) => resource_not_found(&request_id),
        Err(Unavailable) => database_unavailable(&request_id),
    }
}

#[utoipa::path(
    get,
    path = "/api/public/v1/nodes/{node_id}/metrics",
    tag = "public",
    params(
        ("node_id" = String, Path),
        ("window" = Option<PublicMetricWindow>, Query, description = "Metric Window preset; Live when absent")
    ),
    responses(
        (status = 200, body = PublicNodeMetricHistory),
        (status = 400, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(crate) async fn public_node_metrics(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    params: Result<Query<PublicNodeMetricsQuery>, axum::extract::rejection::QueryRejection>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    // The preset is checked before the Node, as the history route checks its
    // range: a malformed request is refused the same way for every Node ID,
    // including a query string the extractor cannot read (a repeated
    // `window`), which is answered with the same error body.
    let window = params
        .ok()
        .and_then(|Query(params)| PublicMetricWindow::parse(params.window.as_deref()));
    let Some(window) = window else {
        return error_response(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_metric_window",
            "metric window must be one of live, 1h, 6h, 24h, 7d",
        );
    };
    match public_projection(&state)
        .metrics(&node_id, window, crate::auth::now_utc())
        .await
    {
        Ok(Some(history)) => Json(history).into_response(),
        Ok(None) => resource_not_found(&request_id),
        Err(Unavailable) => database_unavailable(&request_id),
    }
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
    match public_projection(&state)
        .peer_history(&node_id, crate::auth::now_utc())
        .await
    {
        Ok(Some(history)) => Json(history).into_response(),
        Ok(None) => resource_not_found(&request_id),
        Err(Unavailable) => database_unavailable(&request_id),
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
    /// The Node Currency judged from the RPC, sync and consensus observations.
    pub currency: node_currency::NodeCurrencyState,
    /// The oldest Server-clock instant at which the RPC, sync and consensus
    /// observations were taken: the Report's own observation time corrected by
    /// the Agent's measured clock skew, falling back to the Server receipt time
    /// when that skew is unmeasured or the Agent is Clock Unreliable (ADR 0014).
    /// Null while any of the three was never observed.
    pub currency_as_of: Option<String>,
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
    pub resync_last_progress_at: Option<String>,
    pub validator: Option<PublicValidatorInsight>,
    /// Server-owned automatic-identity state for this Node (#173):
    /// `identified`, `missing_public_key`, `invalid_public_key`,
    /// `network_identity_missing`, or `network_identity_mismatch`. `None`
    /// means no automatic discovery pass has run yet. It is separate from
    /// Current Validator Status: identity correspondence can be established
    /// while the Provider still cannot confirm current staking validity.
    pub validator_identity_state: Option<String>,
    /// Sanitized explanation when this Node has no established automatic
    /// correspondence. It never contains a raw diagnostic or a public key.
    pub validator_identity_reason: Option<String>,
    /// This Node's compact country buckets over exactly its own current Peer
    /// records. The Network list ships these instead of one full country
    /// projection per Node, so the browser can sum the Nodes that match the
    /// current filters without a second request.
    pub peer_countries: PublicNodePeerCountries,
}

/// Public Node Detail response: every `PublicNode` field plus the Node Peer
/// Country View over exactly this Node's current Peer records. The Node list
/// keeps `PublicNode`, so a Home or Network response never carries a full
/// country projection per Node; a listed Node carries only the compact
/// `peerCountries` buckets the Home selection aggregate sums.
#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicNodeDetail {
    #[serde(flatten)]
    pub node: PublicNode,
    pub geo: PublicGeoInsight,
}

#[utoipa::path(
    get,
    path = "/api/public/v1/networks",
    tag = "public",
    responses((status = 200, description = "Published Network and Node projection", body = [PublicNetwork]))
)]
pub(crate) async fn public_networks(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match public_projection(&state)
        .networks(crate::auth::now_utc())
        .await
    {
        Ok(networks) => Json(networks).into_response(),
        Err(Unavailable) => database_unavailable(&request_id),
    }
}

/// The Public Projection over this Server's database and current Geo and
/// Validator settings.
fn public_projection(state: &AppState) -> PublicProjection<'_> {
    PublicProjection::new(
        state.db(),
        state.geo_config().provider,
        state.geo_status(),
        state.validator_freshness_seconds(),
    )
}

fn resource_not_found(request_id: &RequestId) -> Response {
    error_response(
        &request_id.0,
        StatusCode::NOT_FOUND,
        "not_found",
        "resource not found",
    )
}

fn database_unavailable(request_id: &RequestId) -> Response {
    error_response(
        &request_id.0,
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "server database is unavailable",
    )
}

#[utoipa::path(
    get,
    path = "/api/public/v1/nodes/{node_id}",
    tag = "public",
    params(("node_id" = String, Path, description = "Published Node ID")),
    responses((status = 200, description = "Published Node projection with the Node Peer Country View", body = PublicNodeDetail), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn public_node_detail(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match public_projection(&state)
        .node(&node_id, crate::auth::now_utc())
        .await
    {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => resource_not_found(&request_id),
        Err(Unavailable) => database_unavailable(&request_id),
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

/// Freshness of the Reward Wallet Balance. `unavailable` means no refresh has
/// ever succeeded; it is never a zero balance.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublicRewardWalletState {
    Current,
    Stale,
    Unavailable,
}

/// Why the latest refresh produced no balance. The raw upstream error stays
/// in the Server log.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublicRewardWalletErrorCategory {
    UpstreamUnavailable,
    InvalidResponse,
}

/// The Reward Wallet Balance (CONTEXT.md): the fixed Reward Wallet's freely
/// available native balance as PlatScan reports it. One Network-independent
/// figure; neither the Home Network selection nor the list filters narrow it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicRewardWallet {
    /// The fixed Reward Wallet address on PlatON mainnet.
    pub address: String,
    /// Exact decimal LAT string of the last successful refresh; absent until
    /// one has succeeded.
    pub balance: Option<String>,
    /// Always `LAT`.
    pub unit: String,
    /// `stale` once the last success is older than three refresh cycles.
    pub state: PublicRewardWalletState,
    pub last_good_at: Option<String>,
    pub last_attempt_at: Option<String>,
    /// Category of the latest attempt's failure; absent after a success.
    pub error_category: Option<PublicRewardWalletErrorCategory>,
}

impl From<crate::reward_wallet::RewardWalletReading> for PublicRewardWallet {
    fn from(reading: crate::reward_wallet::RewardWalletReading) -> Self {
        use crate::reward_wallet::{BalanceState, ErrorCategory};
        Self {
            address: reading.address.to_owned(),
            balance: reading.balance,
            unit: crate::reward_wallet::REWARD_WALLET_UNIT.to_owned(),
            state: match reading.state {
                BalanceState::Current => PublicRewardWalletState::Current,
                BalanceState::Stale => PublicRewardWalletState::Stale,
                BalanceState::Unavailable => PublicRewardWalletState::Unavailable,
            },
            last_good_at: reading.last_good_at,
            last_attempt_at: reading.last_attempt_at,
            error_category: reading.error_category.map(|category| match category {
                ErrorCategory::UpstreamUnavailable => {
                    PublicRewardWalletErrorCategory::UpstreamUnavailable
                }
                ErrorCategory::InvalidResponse => PublicRewardWalletErrorCategory::InvalidResponse,
            }),
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/public/v1/reward-wallet",
    tag = "public",
    responses(
        (status = 200, description = "The Reward Wallet Balance, independent of every Home selection", body = PublicRewardWallet),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(crate) async fn public_reward_wallet(State(state): State<AppState>) -> Response {
    match crate::reward_wallet::read(state.db().pool(), crate::auth::now_utc()).await {
        Ok(reading) => Json(PublicRewardWallet::from(reading)).into_response(),
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
        .route("/networks", get(public_networks))
        .route("/reward-wallet", get(public_reward_wallet))
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
    use crate::geo_test_support::geo_readings;
    use crate::public_projection::{GeoScope, PUBLIC_GEO_ERROR, PUBLIC_METRIC_SERIES};
    use axum::body::to_bytes;
    use tempfile::tempdir;

    async fn test_state() -> (tempfile::TempDir, AppState) {
        let dir = tempdir().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        (dir, AppState::new(database, None, auth))
    }

    /// Every Network of the Public Projection, read at the current instant
    /// through the module the Public routes answer from.
    async fn networks_value(state: &AppState) -> serde_json::Value {
        let networks = public_projection(state)
            .networks(crate::auth::now_utc())
            .await
            .unwrap();
        serde_json::to_value(networks).unwrap()
    }

    /// One Node of the Public Projection at the current instant; `None` when
    /// it is not an Active Node.
    async fn node_value(state: &AppState, node_id: &str) -> Option<serde_json::Value> {
        public_projection(state)
            .node(node_id, crate::auth::now_utc())
            .await
            .unwrap()
            .map(|node| serde_json::to_value(node).unwrap())
    }

    /// One Node's metrics for `window` ending at the current instant.
    async fn metrics_value(
        state: &AppState,
        node_id: &str,
        window: PublicMetricWindow,
    ) -> Option<serde_json::Value> {
        public_projection(state)
            .metrics(node_id, window, crate::auth::now_utc())
            .await
            .unwrap()
            .map(|history| serde_json::to_value(history).unwrap())
    }

    /// `node-public`'s metrics for the preset a request would name.
    async fn metric_window_value(state: &AppState, window: Option<&str>) -> serde_json::Value {
        let window = PublicMetricWindow::parse(window).expect("a valid preset");
        metrics_value(state, "node-public", window)
            .await
            .expect("node-public is an Active Node")
    }

    /// One Node's Peer history at the current instant.
    async fn peer_history_value(state: &AppState, node_id: &str) -> Option<serde_json::Value> {
        public_projection(state)
            .peer_history(node_id, crate::auth::now_utc())
            .await
            .unwrap()
            .map(|history| serde_json::to_value(history).unwrap())
    }

    /// One Node's whole Block History at the current instant.
    async fn block_history_value(state: &AppState, node_id: &str) -> Option<serde_json::Value> {
        let range = BlockHistoryRange {
            from: None,
            to: None,
            limit: 50,
        };
        public_projection(state)
            .block_history(node_id, range, crate::auth::now_utc())
            .await
            .unwrap()
            .map(|history| serde_json::to_value(history).unwrap())
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
        let state = state
            .with_geo_loader(loader)
            .with_geo_provider(crate::geo::GeoSelection {
                provider: crate::geo::GeoProvider::LocalMmdb,
                generation: 1,
            });
        let insight = geo_insight_for(&state, "mainnet").await;
        assert_eq!(insight.state, "error");
        assert_eq!(insight.error_reason.as_deref(), Some(PUBLIC_GEO_ERROR));
        assert!(
            !insight
                .error_reason
                .as_deref()
                .unwrap()
                .contains("missing-geolite")
        );

        // A failed loader with a readable store still serves the retained
        // reading: the state is reported, the countries are not discarded.
        seed_geo_network(&state, "mainnet", &["geo-error-node"]).await;
        mark_peer_snapshot(&state, "geo-error-node", 1).await;
        insert_geo_peer(&state, "geo-error-node", "err-1", Some("8.8.8.8")).await;
        insert_geo_cache(&state, "8.8.8.8", "US", 60, 3600).await;
        let list = public_network_list(&state).await;
        assert_eq!(list["geo"]["state"], "error");
        assert_eq!(list["geo"]["knownCountryCount"], 1);
        assert_eq!(list["nodes"][0]["peerCountries"]["knownCountryCount"], 1);
        assert_network_geo_is_the_node_sum(&list);

        // An unreadable store is the one case with no reading at all: neither
        // the Network nor a listed Node may fabricate a count.
        sqlx::query("DROP TABLE geo_location_cache")
            .execute(state.db().pool())
            .await
            .unwrap();
        let list = public_network_list(&state).await;
        assert_eq!(list["geo"]["state"], "error");
        assert_eq!(list["geo"]["scope"], "unavailable");
        assert!(list["geo"]["knownCountryCount"].is_null());
        assert!(list["geo"]["countries"].is_null());
        let buckets = &list["nodes"][0]["peerCountries"];
        assert_eq!(buckets["scope"], "unavailable");
        assert!(buckets["knownCountryCount"].is_null());
        assert!(buckets["countries"].as_array().unwrap().is_empty());
    }

    /// A selected Local MMDB provider whose loader reports Current.
    /// Projection tests seed `geo_location_cache` directly, which is exactly
    /// the retained state a real provider produces.
    fn geo_enabled(state: AppState) -> AppState {
        state
            .with_geo_loader(std::sync::Arc::new(
                crate::geo::GeoLoader::enabled_for_tests(),
            ))
            .with_geo_provider(crate::geo::GeoSelection {
                provider: crate::geo::GeoProvider::LocalMmdb,
                generation: 1,
            })
    }

    fn seconds_ago(seconds: i64) -> String {
        format_rfc3339(crate::auth::now_utc() - time::Duration::seconds(seconds))
    }

    async fn seed_geo_network(state: &AppState, network_key: &str, node_ids: &[&str]) {
        let now = seconds_ago(0);
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, last_received_at, created_at, updated_at) VALUES ('agent-public-test', 1, $1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ($1, $2, '0xgenesis', 1, 1, 'lat', $3, $4) ON CONFLICT DO NOTHING")
            .bind(network_key)
            .bind(format!("{network_key} display"))
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        for node_id in node_ids {
            sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, 'agent-public-test', $2, $3, 'ws://127.0.0.1:1', 'active', 'public', 1, $4, $5)")
                .bind(node_id)
                .bind(network_key)
                .bind(format!("{node_id} display"))
                .bind(&now)
                .bind(&now)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
    }

    /// A successful Peer Snapshot basis. `value_revision = 0` models a Node
    /// that has never produced one.
    async fn mark_peer_snapshot(state: &AppState, node_id: &str, value_revision: i64) {
        let now = seconds_ago(0);
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, observation_at, value_received_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', $1, $2, 'peers', 'ok', $3, $4, $5, $5, $6, 1, $7) ON CONFLICT(agent_id, scope, scope_key, component_key) DO UPDATE SET state = 'ok', value_revision = excluded.value_revision, received_at = excluded.received_at, observation_at = excluded.observation_at, observed_at = excluded.observed_at, value_received_at = excluded.value_received_at")
            .bind(node_id)
            .bind(node_id)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(value_revision)
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    async fn insert_geo_peer(
        state: &AppState,
        node_id: &str,
        peer_id: &str,
        remote_ip: Option<&str>,
    ) {
        sqlx::query("INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, updated_at) VALUES ($1, $2, $3, 'inbound', 0, 0, 0, $4)")
            .bind(node_id)
            .bind(peer_id)
            .bind(remote_ip)
            .bind(seconds_ago(0))
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    /// Retain one country result. `created_seconds_ago` beyond the 30-day
    /// boundary is unretainable; a negative `expires_in_seconds` is expired
    /// last-good data that is still inside that boundary.
    async fn insert_geo_cache(
        state: &AppState,
        ip: &str,
        country: &str,
        created_seconds_ago: i64,
        expires_in_seconds: i64,
    ) {
        let now = crate::auth::now_utc();
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('local_mmdb', $1, $2, 'current', $3, $4, $5, $6, $7)")
            .bind(ip)
            .bind(country)
            .bind(format_rfc3339(now - time::Duration::seconds(created_seconds_ago)))
            .bind(format_rfc3339(now))
            .bind(format_rfc3339(now))
            .bind(format_rfc3339(now))
            .bind(format_rfc3339(now + time::Duration::seconds(expires_in_seconds)))
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    /// The same scoped Geo read the public handlers perform, so a test projects
    /// one Network or one Node exactly as a request would.
    async fn scoped_geo_insight(state: &AppState, scope: GeoScope<'_>) -> PublicGeoInsight {
        geo_readings(state, Some(scope)).await.insight(scope)
    }

    async fn geo_insight_for(state: &AppState, network_key: &str) -> PublicGeoInsight {
        scoped_geo_insight(state, GeoScope::Network(network_key)).await
    }

    /// Sum one compact bucket field over a response's listed Nodes. `None`
    /// when not one of them has a reliable basis.
    fn sum_node_buckets(nodes: &[serde_json::Value], field: &str) -> Option<i64> {
        let mut total: Option<i64> = None;
        for node in nodes {
            if let Some(count) = node["peerCountries"][field].as_i64() {
                total = Some(total.unwrap_or(0) + count);
            }
        }
        total
    }

    /// The Node IDs one response lists, each with the Network it was listed
    /// under, in the shape `PublicGeoReadings::restrict_to_listed_nodes` takes.
    fn listed_nodes(
        network_key: &str,
        node_ids: &[&str],
    ) -> std::collections::BTreeMap<String, String> {
        node_ids
            .iter()
            .map(|node_id| ((*node_id).to_owned(), network_key.to_owned()))
            .collect()
    }

    /// The listed Nodes' country buckets merged by country code, as
    /// `(count, stale_count)`.
    fn sum_node_countries(
        nodes: &[serde_json::Value],
    ) -> std::collections::BTreeMap<String, (i64, i64)> {
        let mut total: std::collections::BTreeMap<String, (i64, i64)> =
            std::collections::BTreeMap::new();
        for node in nodes {
            for country in node["peerCountries"]["countries"].as_array().unwrap() {
                let entry = total
                    .entry(country["countryCode"].as_str().unwrap().to_owned())
                    .or_insert((0, 0));
                entry.0 += country["count"].as_i64().unwrap();
                entry.1 += country["staleCount"].as_i64().unwrap();
            }
        }
        total
    }

    /// The invariant a selection aggregate depends on: a Network's own Geo
    /// read is exactly the sum of the compact Node buckets the same response
    /// carries, so selecting every Node can never disagree with the
    /// unfiltered reading.
    fn assert_network_geo_is_the_node_sum(network: &serde_json::Value) {
        let nodes = network["nodes"].as_array().unwrap();
        for field in [
            "knownCountryCount",
            "unknownCountryCount",
            "availablePeerCount",
            "unknownWithoutRemoteIpCount",
            "unknownWithPublicIpCount",
        ] {
            assert_eq!(
                network["geo"][field].as_i64(),
                sum_node_buckets(nodes, field),
                "{field} must be the sum of the listed Nodes' buckets"
            );
        }
        let from_geo: std::collections::BTreeMap<String, (i64, i64)> = network["geo"]["countries"]
            .as_array()
            .map(|countries| {
                countries
                    .iter()
                    .map(|country| {
                        (
                            country["countryCode"].as_str().unwrap().to_owned(),
                            (
                                country["count"].as_i64().unwrap(),
                                country["staleCount"].as_i64().unwrap(),
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(from_geo, sum_node_countries(nodes));
    }

    #[tokio::test]
    async fn public_country_distribution_counts_peer_records_without_ip_deduplication() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-a", "geo-b"]).await;
        seed_geo_network(&state, "testnet", &["geo-c"]).await;
        for node_id in ["geo-a", "geo-b", "geo-c"] {
            mark_peer_snapshot(&state, node_id, 1).await;
        }

        // Two Peer records on one Node share a public IP; a third Peer record
        // on another Node observes the same IP again. Every record counts.
        insert_geo_peer(&state, "geo-a", "geo-a-1", Some("8.8.8.8")).await;
        insert_geo_peer(&state, "geo-a", "geo-a-2", Some("8.8.8.8")).await;
        insert_geo_peer(&state, "geo-b", "geo-b-1", Some("8.8.8.8")).await;
        insert_geo_peer(&state, "geo-b", "geo-b-2", Some("9.9.9.9")).await;
        // No usable public IP: Unknown, never zero and never a country.
        insert_geo_peer(&state, "geo-b", "geo-b-3", None).await;
        // Another Network's Peer records never leak into this projection.
        insert_geo_peer(&state, "geo-c", "geo-c-1", Some("8.8.8.8")).await;
        // A Retired Node is filtered out of every bucket as well.
        sqlx::query("UPDATE nodes SET lifecycle = 'retired' WHERE node_id = 'geo-c'")
            .execute(state.db().pool())
            .await
            .unwrap();

        insert_geo_cache(&state, "8.8.8.8", "US", 60, 3600).await;
        insert_geo_cache(&state, "9.9.9.9", "DE", 60, 3600).await;

        let value = public_network_list(&state).await;

        let countries = value["geo"]["countries"].as_array().unwrap();
        assert_eq!(countries.len(), 2);
        let us = countries
            .iter()
            .find(|country| country["countryCode"] == "US")
            .unwrap();
        assert_eq!(us["count"], 3, "three Peer records, not two distinct IPs");
        assert_eq!(us["staleCount"], 0);
        let de = countries
            .iter()
            .find(|country| country["countryCode"] == "DE")
            .unwrap();
        assert_eq!(de["count"], 1);

        assert_eq!(value["geo"]["state"], "current");
        assert_eq!(value["geo"]["scope"], "complete");
        assert_eq!(value["geo"]["knownCountryCount"], 4);
        assert_eq!(value["geo"]["unknownCountryCount"], 1);
        assert_eq!(value["geo"]["availablePeerCount"], 5);
        assert_eq!(value["geo"]["unknownWithoutRemoteIpCount"], 1);
        assert_eq!(value["geo"]["unknownWithPublicIpCount"], 0);
        // The country buckets and the aggregate Peer Insight are computed
        // from the same Peer-record basis on the same projection, so the
        // Server's denominator always equals the total it reports there.
        assert_eq!(value["peers"]["peerCount"], 5);
        assert_eq!(
            value["geo"]["knownCountryCount"].as_i64().unwrap()
                + value["geo"]["unknownCountryCount"].as_i64().unwrap(),
            value["geo"]["availablePeerCount"].as_i64().unwrap()
        );

        let text = value.to_string();
        for raw in ["8.8.8.8", "9.9.9.9", "geo-a-1"] {
            assert!(!text.contains(raw), "Public projection leaked {raw}");
        }
    }

    #[tokio::test]
    async fn public_country_distribution_keeps_retained_last_good_countries_as_stale() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-stale"]).await;
        mark_peer_snapshot(&state, "geo-stale", 1).await;

        // Expired but inside the hard retention boundary: retained last-good.
        insert_geo_peer(&state, "geo-stale", "stale-1", Some("8.8.4.4")).await;
        insert_geo_peer(&state, "geo-stale", "stale-2", Some("1.1.1.1")).await;
        // Expired and outside the boundary: no longer retainable.
        insert_geo_peer(&state, "geo-stale", "stale-3", Some("9.9.9.9")).await;
        insert_geo_cache(&state, "8.8.4.4", "US", 3600, -60).await;
        insert_geo_cache(&state, "1.1.1.1", "AU", 60, 3600).await;
        insert_geo_cache(&state, "9.9.9.9", "DE", 31 * 24 * 3600 + 60, 3600).await;

        let insight = geo_insight_for(&state, "mainnet").await;
        assert_eq!(insight.scope, "complete");
        let countries = insight.countries.as_ref().unwrap();
        assert_eq!(countries.len(), 2, "the unretainable country is not shown");
        let us = countries
            .iter()
            .find(|country| country.country_code == "US")
            .unwrap();
        assert_eq!(us.count, 1);
        assert_eq!(us.stale_count, 1, "retained last-good stays Stale");
        let au = countries
            .iter()
            .find(|country| country.country_code == "AU")
            .unwrap();
        assert_eq!(au.count, 1);
        assert_eq!(au.stale_count, 0);
        // Stale countries are not re-counted as Unknown, and the one
        // unretainable record is.
        assert_eq!(insight.known_country_count, Some(2));
        assert_eq!(insight.unknown_country_count, Some(1));
        assert_eq!(insight.available_peer_count, Some(3));
        assert_eq!(insight.unknown_without_remote_ip_count, Some(0));
        assert_eq!(insight.unknown_with_public_ip_count, Some(1));
    }

    #[tokio::test]
    async fn public_country_distribution_distinguishes_unobserved_from_an_empty_snapshot() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-never", "geo-empty"]).await;
        mark_peer_snapshot(&state, "geo-empty", 1).await;
        mark_peer_snapshot(&state, "geo-never", 0).await;

        // One Active Node has never produced a Peer Snapshot: partial scope
        // with a real denominator, never a fabricated complete zero.
        let insight = geo_insight_for(&state, "mainnet").await;
        assert_eq!(insight.scope, "partial");
        assert_eq!(insight.known_country_count, Some(0));
        assert_eq!(insight.unknown_country_count, Some(0));
        assert_eq!(insight.available_peer_count, Some(0));

        // No Active Node has a successful snapshot: no reliable denominator,
        // so Unknown and Known stay absent instead of becoming zero.
        sqlx::query("UPDATE component_status SET value_revision = 0 WHERE node_id = 'geo-empty' AND component_key = 'peers'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let insight = geo_insight_for(&state, "mainnet").await;
        assert_eq!(insight.scope, "unobserved");
        assert!(insight.known_country_count.is_none());
        assert!(insight.unknown_country_count.is_none());
        assert!(insight.available_peer_count.is_none());
        assert!(insight.unknown_with_public_ip_count.is_none());
        assert!(insight.countries.is_none());

        // Successful empty Peer Snapshots are an authoritative zero.
        mark_peer_snapshot(&state, "geo-empty", 1).await;
        mark_peer_snapshot(&state, "geo-never", 1).await;
        let insight = geo_insight_for(&state, "mainnet").await;
        assert_eq!(insight.scope, "complete");
        assert_eq!(insight.known_country_count, Some(0));
        assert_eq!(insight.unknown_country_count, Some(0));
        assert_eq!(insight.available_peer_count, Some(0));
        assert_eq!(insight.countries.as_ref().map(Vec::len), Some(0));
    }

    #[tokio::test]
    async fn public_geo_state_is_independent_from_failed_peer_collection() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-failed"]).await;
        mark_peer_snapshot(&state, "geo-failed", 1).await;
        insert_geo_peer(&state, "geo-failed", "failed-1", Some("8.8.8.8")).await;
        insert_geo_cache(&state, "8.8.8.8", "US", 60, 3600).await;

        // A failed collection keeps the retained Peer Snapshot and its own
        // stale state; Geo being Current never claims the Peers are fresh.
        let stale_received = seconds_ago(600);
        sqlx::query("UPDATE component_status SET state = 'error', state_revision = 2, received_at = $1, observation_at = $1, observed_at = $2, value_received_at = $3 WHERE node_id = 'geo-failed' AND component_key = 'peers'")
            .bind(&stale_received)
            .bind(&stale_received)
            .bind(&stale_received)
            .execute(state.db().pool())
            .await
            .unwrap();

        let value = public_network_list(&state).await;
        assert_eq!(value["peers"]["state"], "error");
        assert_eq!(value["peers"]["freshness"], "stale");
        assert_eq!(value["peers"]["peerCount"], 1);
        assert_eq!(value["geo"]["state"], "current");
        assert_eq!(value["geo"]["knownCountryCount"], 1);
    }

    #[tokio::test]
    async fn disabled_geo_projects_no_counts_at_all() {
        let (_dir, state) = test_state().await;
        seed_geo_network(&state, "mainnet", &["geo-disabled"]).await;
        mark_peer_snapshot(&state, "geo-disabled", 1).await;
        insert_geo_peer(&state, "geo-disabled", "disabled-1", Some("8.8.8.8")).await;
        insert_geo_cache(&state, "8.8.8.8", "US", 60, 3600).await;

        let insight = geo_insight_for(&state, "mainnet").await;
        assert_eq!(insight.state, "disabled");
        assert_eq!(insight.scope, "unavailable");
        assert!(insight.countries.is_none());
        assert!(insight.known_country_count.is_none());
        assert!(insight.unknown_country_count.is_none());
        assert!(insight.available_peer_count.is_none());
        assert!(insight.unknown_with_public_ip_count.is_none());
        assert!(insight.attribution.is_none());

        // A disabled Geo never lets a listed Node claim a country either.
        let list = public_network_list(&state).await;
        assert_eq!(list["geo"]["scope"], "unavailable");
        let buckets = &list["nodes"][0]["peerCountries"];
        assert_eq!(buckets["scope"], "unavailable");
        assert!(buckets["knownCountryCount"].is_null());
        assert!(buckets["unknownCountryCount"].is_null());
        assert!(buckets["availablePeerCount"].is_null());
        assert!(buckets["unknownWithoutRemoteIpCount"].is_null());
        assert!(buckets["unknownWithPublicIpCount"].is_null());
        assert!(buckets["countries"].as_array().unwrap().is_empty());
    }

    async fn node_insight_for(state: &AppState, node_id: &str) -> PublicGeoInsight {
        scoped_geo_insight(state, GeoScope::Node(node_id)).await
    }

    async fn public_node_detail_value(state: &AppState, node_id: &str) -> serde_json::Value {
        node_value(state, node_id).await.expect("an Active Node")
    }

    #[tokio::test]
    async fn node_country_view_counts_only_the_node_and_shares_the_peer_count_denominator() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-node-a", "geo-node-b"]).await;
        mark_peer_snapshot(&state, "geo-node-a", 1).await;
        mark_peer_snapshot(&state, "geo-node-b", 1).await;

        insert_geo_peer(&state, "geo-node-a", "a-1", Some("8.8.8.8")).await;
        insert_geo_peer(&state, "geo-node-a", "a-2", Some("8.8.8.8")).await;
        insert_geo_peer(&state, "geo-node-a", "a-3", None).await;
        // Another Node's records are a different Node's view.
        insert_geo_peer(&state, "geo-node-b", "b-1", Some("9.9.9.9")).await;
        insert_geo_cache(&state, "8.8.8.8", "US", 60, 3600).await;
        insert_geo_cache(&state, "9.9.9.9", "DE", 60, 3600).await;

        let value = public_node_detail_value(&state, "geo-node-a").await;
        assert_eq!(value["peers"]["peerCount"], 3);
        assert_eq!(value["geo"]["scope"], "complete");
        assert_eq!(value["geo"]["knownCountryCount"], 2);
        assert_eq!(value["geo"]["unknownCountryCount"], 1);
        assert_eq!(
            value["geo"]["availablePeerCount"], value["peers"]["peerCount"],
            "the Node map and the Peer Count tile share one denominator"
        );
        // Two records on one IP: the same address is never deduplicated.
        let countries = value["geo"]["countries"].as_array().unwrap();
        assert_eq!(countries.len(), 1);
        assert_eq!(countries[0]["countryCode"], "US");
        assert_eq!(countries[0]["count"], 2);
        let text = value.to_string();
        assert!(!text.contains("8.8.8.8") && !text.contains("a-1"));
    }
    /// The Node scope is exactly one Node, so it can only be `complete` or
    /// `unobserved`; a second Node's snapshot never makes it `partial`.
    #[tokio::test]
    async fn node_country_view_is_complete_or_unobserved_but_never_partial() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-one", "geo-two"]).await;

        // Never produced a Peer Snapshot: no reliable denominator.
        let insight = node_insight_for(&state, "geo-one").await;
        assert_eq!(insight.scope, "unobserved");
        assert!(insight.known_country_count.is_none());
        assert!(insight.unknown_country_count.is_none());
        assert!(insight.available_peer_count.is_none());
        assert!(insight.countries.is_none());

        // A neighbouring Node's successful snapshot does not leak in.
        mark_peer_snapshot(&state, "geo-two", 1).await;
        let insight = node_insight_for(&state, "geo-one").await;
        assert_eq!(insight.scope, "unobserved");
        assert!(insight.known_country_count.is_none());

        // A successful empty snapshot is an authoritative zero.
        mark_peer_snapshot(&state, "geo-one", 1).await;
        let insight = node_insight_for(&state, "geo-one").await;
        assert_eq!(insight.scope, "complete");
        assert_eq!(insight.known_country_count, Some(0));
        assert_eq!(insight.unknown_country_count, Some(0));
        assert_eq!(insight.available_peer_count, Some(0));
        assert_eq!(insight.countries.as_ref().map(Vec::len), Some(0));
    }

    /// Retention and the Unknown split are the same rules the Network scope
    /// uses: retained last-good stays Stale, an unretainable row and a row
    /// without a usable public IP are the two distinct Unknown reasons.
    #[tokio::test]
    async fn node_country_view_keeps_retained_last_good_countries_and_splits_unknown_reasons() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-stale-node"]).await;
        mark_peer_snapshot(&state, "geo-stale-node", 1).await;

        insert_geo_peer(&state, "geo-stale-node", "s-1", Some("8.8.4.4")).await;
        insert_geo_peer(&state, "geo-stale-node", "s-2", Some("9.9.9.9")).await;
        insert_geo_peer(&state, "geo-stale-node", "s-3", None).await;
        // Expired but inside the hard retention boundary: last-good Stale.
        insert_geo_cache(&state, "8.8.4.4", "US", 3600, -60).await;
        // Expired beyond the boundary: no longer retainable.
        insert_geo_cache(&state, "9.9.9.9", "DE", 31 * 24 * 3600 + 60, 3600).await;

        let insight = node_insight_for(&state, "geo-stale-node").await;
        assert_eq!(insight.scope, "complete");
        let countries = insight.countries.as_ref().unwrap();
        assert_eq!(countries.len(), 1);
        assert_eq!(countries[0].country_code, "US");
        assert_eq!(countries[0].stale_count, 1);
        assert_eq!(insight.known_country_count, Some(1));
        assert_eq!(insight.unknown_country_count, Some(2));
        assert_eq!(
            insight.known_country_count.unwrap() + insight.unknown_country_count.unwrap(),
            insight.available_peer_count.unwrap()
        );
        assert_eq!(insight.unknown_without_remote_ip_count, Some(1));
        assert_eq!(insight.unknown_with_public_ip_count, Some(1));

        // A listed Node and its Detail page serve those same numbers as
        // compact buckets.
        let list = public_network_list(&state).await;
        let buckets = &list["nodes"][0]["peerCountries"];
        assert_eq!(buckets["scope"], "complete");
        assert_eq!(buckets["knownCountryCount"], 1);
        assert_eq!(buckets["unknownCountryCount"], 2);
        assert_eq!(buckets["availablePeerCount"], 3);
        assert_eq!(buckets["unknownWithoutRemoteIpCount"], 1);
        assert_eq!(buckets["unknownWithPublicIpCount"], 1);
        let bucket_countries = buckets["countries"].as_array().unwrap();
        assert_eq!(
            bucket_countries.len(),
            1,
            "the unretainable row is no country"
        );
        assert_eq!(bucket_countries[0]["countryCode"], "US");
        assert_eq!(bucket_countries[0]["count"], 1);
        assert_eq!(bucket_countries[0]["staleCount"], 1);
        let detail = public_node_detail_value(&state, "geo-stale-node").await;
        assert_eq!(detail["peerCountries"], *buckets);
        assert_network_geo_is_the_node_sum(&list);
    }
    #[tokio::test]
    async fn node_country_view_projects_nothing_while_geo_is_disabled() {
        let (_dir, state) = test_state().await;
        seed_geo_network(&state, "mainnet", &["geo-disabled-node"]).await;
        mark_peer_snapshot(&state, "geo-disabled-node", 1).await;
        insert_geo_peer(&state, "geo-disabled-node", "d-1", Some("8.8.8.8")).await;
        insert_geo_cache(&state, "8.8.8.8", "US", 60, 3600).await;

        let insight = node_insight_for(&state, "geo-disabled-node").await;
        assert_eq!(insight.state, "disabled");
        assert_eq!(insight.scope, "unavailable");
        assert!(insight.countries.is_none());
        assert!(insight.known_country_count.is_none());
        assert!(insight.available_peer_count.is_none());
        assert!(insight.attribution.is_none());
    }

    /// A country retained by a provider that is not selected is not a country
    /// for the current projection, exactly as in the Network scope.
    #[tokio::test]
    async fn node_country_view_ignores_rows_retained_by_another_provider() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-provider-node"]).await;
        mark_peer_snapshot(&state, "geo-provider-node", 1).await;
        insert_geo_peer(&state, "geo-provider-node", "p-1", Some("8.8.8.8")).await;
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('ipinfo', '8.8.8.8', 'SE', 'current', $1, $2, $3, $4, $5)")
            .bind(seconds_ago(60))
            .bind(seconds_ago(60))
            .bind(seconds_ago(60))
            .bind(seconds_ago(60))
            .bind(format_rfc3339(crate::auth::now_utc() + time::Duration::hours(1)))
            .execute(state.db().pool())
            .await
            .unwrap();

        let insight = node_insight_for(&state, "geo-provider-node").await;
        assert_eq!(insight.known_country_count, Some(0));
        assert_eq!(insight.unknown_country_count, Some(1));
        assert_eq!(insight.unknown_with_public_ip_count, Some(1));
        assert_eq!(insight.countries.as_ref().map(Vec::len), Some(0));
    }

    /// The public Node route already 404s a Retired Node; the projection must
    /// not resurrect its Peer records through a direct call either.
    #[tokio::test]
    async fn node_country_view_ignores_a_retired_nodes_peer_records() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-retired-node"]).await;
        mark_peer_snapshot(&state, "geo-retired-node", 1).await;
        insert_geo_peer(&state, "geo-retired-node", "r-1", Some("8.8.8.8")).await;
        insert_geo_cache(&state, "8.8.8.8", "US", 60, 3600).await;
        sqlx::query("UPDATE nodes SET lifecycle = 'retired' WHERE node_id = 'geo-retired-node'")
            .execute(state.db().pool())
            .await
            .unwrap();

        assert!(node_value(&state, "geo-retired-node").await.is_none());

        let insight = node_insight_for(&state, "geo-retired-node").await;
        assert_eq!(insight.scope, "unobserved");
        assert!(insight.known_country_count.is_none());
        assert!(insight.unknown_country_count.is_none());
        assert!(insight.available_peer_count.is_none());
    }

    /// A listed Node carries the compact buckets a Home selection aggregate
    /// sums — never a Network-shaped Geo Insight of its own — and those
    /// buckets are the same reading that Node's own route serves.
    #[tokio::test]
    async fn listed_nodes_carry_compact_peer_country_buckets_not_a_geo_insight() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-dto-node"]).await;
        mark_peer_snapshot(&state, "geo-dto-node", 1).await;
        insert_geo_peer(&state, "geo-dto-node", "dto-1", Some("8.8.8.8")).await;
        insert_geo_cache(&state, "8.8.8.8", "US", 60, 3600).await;

        let list = public_network_list(&state).await;
        assert!(
            list["nodes"][0].get("geo").is_none(),
            "a listed Node carries no country projection"
        );
        assert_eq!(list["geo"]["knownCountryCount"], 1);

        // The bucket carries exactly what a selection needs to sum: counts,
        // the Unknown reasons, and enough of each country to place its dot.
        let buckets = &list["nodes"][0]["peerCountries"];
        assert_eq!(buckets["scope"], "complete");
        assert_eq!(buckets["knownCountryCount"], 1);
        assert_eq!(buckets["unknownCountryCount"], 0);
        assert_eq!(buckets["availablePeerCount"], 1);
        assert_eq!(buckets["unknownWithoutRemoteIpCount"], 0);
        assert_eq!(buckets["unknownWithPublicIpCount"], 0);
        let countries = buckets["countries"].as_array().unwrap();
        assert_eq!(countries.len(), 1);
        assert_eq!(countries[0]["countryCode"], "US");
        assert_eq!(countries[0]["count"], 1);
        assert_eq!(countries[0]["staleCount"], 0);
        assert!(
            countries[0]["centroidLat"].is_number() && countries[0]["centroidLon"].is_number(),
            "a listed country bucket already carries its centroid"
        );
        assert_network_geo_is_the_node_sum(&list);

        let detail = public_node_detail_value(&state, "geo-dto-node").await;
        assert_eq!(detail["geo"]["knownCountryCount"], 1);
        assert_eq!(
            detail["geo"]["availablePeerCount"],
            detail["peers"]["peerCount"]
        );
        // The Node's compact buckets and its Node-scoped projection are one
        // reading: selecting only this Node reproduces its Detail page.
        assert_eq!(detail["peerCountries"], *buckets);
        assert_eq!(
            detail["peerCountries"]["knownCountryCount"],
            detail["geo"]["knownCountryCount"]
        );
        assert_eq!(
            detail["peerCountries"]["unknownCountryCount"],
            detail["geo"]["unknownCountryCount"]
        );
        assert_eq!(
            detail["peerCountries"]["availablePeerCount"],
            detail["geo"]["availablePeerCount"]
        );
        assert_eq!(
            detail["peerCountries"]["countries"]
                .as_array()
                .unwrap()
                .len(),
            detail["geo"]["countries"].as_array().unwrap().len()
        );
    }
    #[tokio::test]
    async fn public_network_list_reuses_one_consistent_geo_projection() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-list-a"]).await;
        seed_geo_network(&state, "testnet", &["geo-list-b"]).await;
        mark_peer_snapshot(&state, "geo-list-a", 1).await;
        mark_peer_snapshot(&state, "geo-list-b", 1).await;
        insert_geo_peer(&state, "geo-list-a", "list-a-1", Some("8.8.8.8")).await;
        insert_geo_peer(&state, "geo-list-b", "list-b-1", Some("8.8.8.8")).await;
        insert_geo_peer(&state, "geo-list-b", "list-b-2", None).await;
        insert_geo_cache(&state, "8.8.8.8", "US", 60, 3600).await;

        let value = networks_value(&state).await;
        let by_key = |key: &str| {
            value
                .as_array()
                .unwrap()
                .iter()
                .find(|network| network["networkKey"] == key)
                .unwrap()
                .clone()
        };
        let mainnet = by_key("mainnet");
        assert_eq!(mainnet["geo"]["knownCountryCount"], 1);
        assert_eq!(mainnet["geo"]["unknownCountryCount"], 0);
        assert_eq!(mainnet["geo"]["availablePeerCount"], 1);
        assert_eq!(mainnet["peers"]["peerCount"], 1);
        let testnet = by_key("testnet");
        assert_eq!(testnet["geo"]["knownCountryCount"], 1);
        assert_eq!(testnet["geo"]["unknownCountryCount"], 1);
        assert_eq!(testnet["geo"]["unknownWithoutRemoteIpCount"], 1);
        assert_eq!(testnet["geo"]["unknownWithPublicIpCount"], 0);
        assert_eq!(testnet["geo"]["availablePeerCount"], 2);
        assert_eq!(testnet["peers"]["peerCount"], 2);
        // Each listed Node carries its own compact buckets, and each Network
        // reading is exactly the sum of them.
        assert_eq!(mainnet["nodes"][0]["peerCountries"]["knownCountryCount"], 1);
        assert_eq!(
            mainnet["nodes"][0]["peerCountries"]["availablePeerCount"],
            1
        );
        assert_eq!(testnet["nodes"][0]["peerCountries"]["knownCountryCount"], 1);
        assert_eq!(
            testnet["nodes"][0]["peerCountries"]["unknownCountryCount"],
            1
        );
        assert_eq!(
            testnet["nodes"][0]["peerCountries"]["unknownWithoutRemoteIpCount"],
            1
        );
        assert_eq!(
            testnet["nodes"][0]["peerCountries"]["availablePeerCount"],
            2
        );
        assert_network_geo_is_the_node_sum(&mainnet);
        assert_network_geo_is_the_node_sum(&testnet);
    }

    /// The Network reading is the sum of its Active Node buckets by
    /// construction, and a Node with no successful Peer Snapshot contributes
    /// no buckets at all — so it can never be read as a country zero.
    #[tokio::test]
    async fn network_geo_reading_is_the_sum_of_its_active_node_buckets() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-sum-a", "geo-sum-b", "geo-sum-c"]).await;
        mark_peer_snapshot(&state, "geo-sum-a", 1).await;
        mark_peer_snapshot(&state, "geo-sum-b", 1).await;
        mark_peer_snapshot(&state, "geo-sum-c", 0).await;

        // Node A: two Peer records in the United States, one of them retained
        // last-good past its expiry, and one record without a usable IP.
        insert_geo_peer(&state, "geo-sum-a", "sum-a-1", Some("8.8.8.8")).await;
        insert_geo_peer(&state, "geo-sum-a", "sum-a-2", Some("9.9.9.9")).await;
        insert_geo_peer(&state, "geo-sum-a", "sum-a-3", None).await;
        insert_geo_cache(&state, "8.8.8.8", "US", 60, 3600).await;
        insert_geo_cache(&state, "9.9.9.9", "US", 3600, -60).await;
        // Node B: one address past the retention boundary and one in Germany.
        insert_geo_peer(&state, "geo-sum-b", "sum-b-1", Some("5.5.5.5")).await;
        insert_geo_peer(&state, "geo-sum-b", "sum-b-2", Some("6.6.6.6")).await;
        insert_geo_cache(&state, "5.5.5.5", "US", 31 * 24 * 3600 + 60, 3600).await;
        insert_geo_cache(&state, "6.6.6.6", "DE", 60, 3600).await;
        // Node C: never produced a Peer Snapshot, so its single record is no
        // country at the Network either.
        insert_geo_peer(&state, "geo-sum-c", "sum-c-1", Some("8.8.8.8")).await;

        let list = public_network_list(&state).await;
        assert_eq!(list["geo"]["scope"], "partial");
        assert_eq!(list["geo"]["knownCountryCount"], 3);
        assert_eq!(list["geo"]["unknownCountryCount"], 2);
        assert_eq!(list["geo"]["availablePeerCount"], 5);
        assert_eq!(list["geo"]["unknownWithoutRemoteIpCount"], 1);
        assert_eq!(list["geo"]["unknownWithPublicIpCount"], 1);
        assert_network_geo_is_the_node_sum(&list);

        let node = |node_id: &str| {
            list["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|node| node["nodeId"] == node_id)
                .unwrap()
                .clone()
        };
        let a = node("geo-sum-a");
        assert_eq!(a["peerCountries"]["scope"], "complete");
        assert_eq!(a["peerCountries"]["knownCountryCount"], 2);
        assert_eq!(a["peerCountries"]["unknownCountryCount"], 1);
        assert_eq!(a["peerCountries"]["unknownWithoutRemoteIpCount"], 1);
        assert_eq!(a["peerCountries"]["unknownWithPublicIpCount"], 0);
        let a_countries = a["peerCountries"]["countries"].as_array().unwrap();
        assert_eq!(a_countries.len(), 1);
        assert_eq!(a_countries[0]["countryCode"], "US");
        assert_eq!(a_countries[0]["count"], 2);
        assert_eq!(a_countries[0]["staleCount"], 1);
        let b = node("geo-sum-b");
        assert_eq!(b["peerCountries"]["scope"], "complete");
        assert_eq!(b["peerCountries"]["knownCountryCount"], 1);
        assert_eq!(b["peerCountries"]["unknownCountryCount"], 1);
        assert_eq!(b["peerCountries"]["countries"][0]["countryCode"], "DE");
        let c = node("geo-sum-c");
        assert_eq!(c["peerCountries"]["scope"], "unobserved");
        assert!(c["peerCountries"]["knownCountryCount"].is_null());
        assert!(c["peerCountries"]["unknownCountryCount"].is_null());
        assert!(c["peerCountries"]["availablePeerCount"].is_null());
        assert!(
            c["peerCountries"]["countries"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    /// A response's Geo reading covers exactly the Nodes that response lists.
    /// The listed rows and the Geo rows are separate reads, so without the
    /// restriction a Node activated in between would enter the Network reading
    /// with no compact bucket of its own — the one way the sum invariant a
    /// selection aggregate relies on could break.
    #[tokio::test]
    async fn network_geo_reading_covers_only_the_listed_nodes() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-scan-a", "geo-scan-b"]).await;
        mark_peer_snapshot(&state, "geo-scan-a", 1).await;
        mark_peer_snapshot(&state, "geo-scan-b", 1).await;
        insert_geo_peer(&state, "geo-scan-a", "scan-a-1", Some("7.7.7.7")).await;
        insert_geo_peer(&state, "geo-scan-b", "scan-b-1", Some("4.4.4.4")).await;
        insert_geo_cache(&state, "7.7.7.7", "US", 60, 3600).await;
        insert_geo_cache(&state, "4.4.4.4", "DE", 60, 3600).await;

        let mut readings = geo_readings(&state, None).await;
        // The response lists only Node A, so only Node A's bucket is read.
        readings.restrict_to_listed_nodes(&listed_nodes("mainnet", &["geo-scan-a"]));

        let network = readings.insight(GeoScope::Network("mainnet"));
        assert_eq!(network.scope, "complete");
        assert_eq!(network.known_country_count, Some(1));
        assert_eq!(network.available_peer_count, Some(1));
        let countries = network.countries.as_ref().unwrap();
        assert_eq!(countries.len(), 1);
        assert_eq!(countries[0].country_code, "US");
        assert_eq!(countries[0].count, 1);

        let listed = readings.node_peer_countries("geo-scan-a");
        assert_eq!(listed.scope, "complete");
        assert_eq!(listed.known_country_count, Some(1));
        // A Node the response does not list projects nothing at all.
        let unlisted = readings.node_peer_countries("geo-scan-b");
        assert_eq!(unlisted.scope, "unavailable");
        assert!(unlisted.known_country_count.is_none());
    }

    /// The other half of the same race: the Node the response lists is the one
    /// this Geo batch lost. It keeps its place in the Active Node count, so the
    /// Network reading stays `partial` instead of claiming a basis over the
    /// Nodes that remain, and its own compact bucket carries no basis either —
    /// summing the listed buckets still reproduces the reading exactly.
    #[tokio::test]
    async fn network_geo_reading_keeps_a_listed_node_the_batch_lost() {
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        seed_geo_network(&state, "mainnet", &["geo-scan-a", "geo-scan-b"]).await;
        mark_peer_snapshot(&state, "geo-scan-a", 1).await;
        mark_peer_snapshot(&state, "geo-scan-b", 1).await;
        insert_geo_peer(&state, "geo-scan-a", "scan-a-1", Some("7.7.7.7")).await;
        insert_geo_peer(&state, "geo-scan-b", "scan-b-1", Some("4.4.4.4")).await;
        insert_geo_cache(&state, "7.7.7.7", "US", 60, 3600).await;
        insert_geo_cache(&state, "4.4.4.4", "DE", 60, 3600).await;

        let mut readings = geo_readings(&state, None).await;
        // Node C was listed by the response the later Geo read never saw, and
        // Node B is no longer listed at all.
        readings.restrict_to_listed_nodes(&listed_nodes("mainnet", &["geo-scan-a", "geo-scan-c"]));

        let network = readings.insight(GeoScope::Network("mainnet"));
        assert_eq!(network.scope, "partial");
        assert_eq!(network.known_country_count, Some(1));
        assert_eq!(network.available_peer_count, Some(1));
        let countries = network.countries.as_ref().unwrap();
        assert_eq!(countries.len(), 1);
        assert_eq!(countries[0].country_code, "US");

        let lost = readings.node_peer_countries("geo-scan-c");
        assert_eq!(lost.scope, "unobserved");
        assert!(lost.known_country_count.is_none());
        assert!(lost.countries.is_empty());
        // Node B is not listed, so it contributes neither denominator nor count.
        assert!(
            readings
                .node_peer_countries("geo-scan-b")
                .known_country_count
                .is_none()
        );
    }

    /// Compact buckets grow by Nodes × countries, and the sum invariant holds
    /// at a copy far larger than any real Network: one response is enough for
    /// a selection aggregate, with no per-Node follow-up request.
    #[tokio::test]
    async fn listed_node_peer_countries_scale_with_nodes_and_countries() {
        const NODES: usize = 256;
        const COUNTRIES: [&str; 10] = ["AU", "BR", "CA", "DE", "FR", "GB", "IN", "JP", "NL", "US"];
        let (_dir, state) = test_state().await;
        let state = geo_enabled(state);
        let node_ids: Vec<String> = (0..NODES)
            .map(|index| format!("geo-scale-{index}"))
            .collect();
        let node_refs: Vec<&str> = node_ids.iter().map(String::as_str).collect();
        seed_geo_network(&state, "mainnet", &node_refs).await;
        for node_id in &node_ids {
            mark_peer_snapshot(&state, node_id, 1).await;
        }
        for (index, country_code) in COUNTRIES.iter().enumerate() {
            insert_geo_cache(
                &state,
                &format!("10.10.0.{}", index + 1),
                country_code,
                60,
                3600,
            )
            .await;
        }
        // One statement per batch keeps the fixture cheap while still writing
        // the same rows the ingest path produces.
        let now = seconds_ago(0);
        for chunk in node_ids.chunks(32) {
            let mut rows = Vec::new();
            for node_id in chunk {
                for (index, _) in COUNTRIES.iter().enumerate() {
                    rows.push(format!(
                        "('{node_id}', 'scale-{node_id}-{index}', '10.10.0.{}', 'inbound', 0, 0, 0, '{now}')",
                        index + 1
                    ));
                }
            }
            sqlx::query(&format!(
                "INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, updated_at) VALUES {}",
                rows.join(", ")
            ))
            .execute(state.db().pool())
            .await
            .unwrap();
        }

        let list = public_network_list(&state).await;
        let nodes = list["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), NODES);
        assert_eq!(list["geo"]["scope"], "complete");
        assert_eq!(
            list["geo"]["knownCountryCount"],
            (NODES * COUNTRIES.len()) as i64
        );
        assert_eq!(list["geo"]["unknownCountryCount"], 0);
        assert_eq!(
            list["geo"]["availablePeerCount"],
            (NODES * COUNTRIES.len()) as i64
        );
        let countries = list["geo"]["countries"].as_array().unwrap();
        assert_eq!(countries.len(), COUNTRIES.len());
        for country in countries {
            assert_eq!(country["count"], NODES as i64);
            assert_eq!(country["staleCount"], 0);
        }
        for node in nodes {
            assert_eq!(node["peerCountries"]["scope"], "complete");
            assert_eq!(node["peerCountries"]["knownCountryCount"], 10);
            assert_eq!(
                node["peerCountries"]["countries"].as_array().unwrap().len(),
                COUNTRIES.len()
            );
        }
        assert_network_geo_is_the_node_sum(&list);
    }

    /// The Owner-facing path end to end against the real Server: the Admin
    /// API selects IPinfo, the background pass resolves through a
    /// deterministic loopback stand-in, and the real Public Network list
    /// serves the resulting counts with the provider's own attribution. No
    /// Peer address leaves the machine.
    #[tokio::test]
    async fn selecting_ipinfo_resolves_in_the_background_and_reaches_the_public_list() {
        let (_dir, state) = test_state().await;
        let stub = crate::geo_external::stub::StubServer::start(vec![
            crate::geo_external::stub::StubReply::country("SE"),
        ])
        .await;
        let state = state.with_external_geo_client(
            crate::geo::GeoProvider::Ipinfo,
            std::sync::Arc::new(crate::geo_external::ExternalGeoClient::for_tests(
                crate::geo::GeoProvider::Ipinfo
                    .external_profile()
                    .expect("IPinfo resolves outside this Server"),
                stub.base_url(),
                std::time::Duration::from_secs(2),
            )),
        );
        seed_geo_network(&state, "mainnet", &["geo-ipinfo-a", "geo-ipinfo-b"]).await;
        mark_peer_snapshot(&state, "geo-ipinfo-a", 1).await;
        // Peer rows exist only where a Peer Snapshot value was accepted, so
        // both Nodes report one and the Network reading counts every row.
        mark_peer_snapshot(&state, "geo-ipinfo-b", 1).await;
        // One address shared by three Peer records across two Nodes, plus a
        // documentation address the trust boundary refuses.
        for (node_id, peer_id, remote_ip) in [
            ("geo-ipinfo-a", "ip-1", "89.160.20.112"),
            ("geo-ipinfo-a", "ip-2", "89.160.20.112"),
            ("geo-ipinfo-b", "ip-3", "89.160.20.112"),
            ("geo-ipinfo-b", "ip-4", "203.0.113.9"),
        ] {
            insert_geo_peer(&state, node_id, peer_id, Some(remote_ip)).await;
        }

        // Nothing is scheduled while Disabled: the Public state is the
        // neutral Disabled notice, never a country count of zero.
        let before = public_network_list(&state).await;
        assert_eq!(before["geo"]["state"], "disabled");
        assert!(before["geo"]["knownCountryCount"].is_null());
        assert_eq!(stub.request_count(), 0);

        // The Owner selects IPinfo through the real Admin API.
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        let session = crate::http::AuthenticatedSession(crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: time::OffsetDateTime::now_utc(),
            last_seen_at: time::OffsetDateTime::now_utc(),
            expires_at: time::OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        });
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        headers.insert(
            axum::http::header::ORIGIN,
            "http://127.0.0.1:8080".parse().unwrap(),
        );
        headers.insert("x-csrf-token", "csrf".parse().unwrap());
        let response = crate::http::admin::update_geo_provider(
            State(state.clone()),
            headers,
            axum::Extension(session),
            axum::Extension(crate::http::RequestId(std::sync::Arc::from("req-ipinfo"))),
            Json(crate::http::admin::GeoProviderUpdateRequest {
                provider: "ipinfo".to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // The background pass resolves the shared address once.
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        assert_eq!(
            crate::geo_backfill::run_pass(&state, &now)
                .await
                .unwrap()
                .resolved,
            1
        );
        assert_eq!(stub.request_count(), 1, "one query per canonical address");

        let after = public_network_list(&state).await;
        assert_eq!(after["geo"]["state"], "current");
        assert_eq!(after["geo"]["knownCountryCount"], 3);
        assert_eq!(after["geo"]["unknownCountryCount"], 1);
        assert_eq!(after["geo"]["countries"][0]["countryCode"], "SE");
        assert_eq!(
            after["geo"]["countries"][0]["count"], 3,
            "counts stay per Peer record, not per deduplicated address"
        );
        assert!(
            after["geo"]["attribution"]
                .as_str()
                .unwrap()
                .contains("IPinfo"),
            "the external provider owns the Public attribution"
        );
        let text = after.to_string();
        assert!(!text.contains("89.160.20.112"), "no raw Peer address");
        assert!(!text.contains("/json"), "no external request detail");
    }

    /// The same Owner-facing path for the second External Geo Provider
    /// (issue #135): the Admin API selects GeoJS, the background pass resolves
    /// through a deterministic loopback stand-in for the fixed GeoJS
    /// destination, and the real Public Network list serves the resulting
    /// counts with GeoJS's own credit. No Peer address leaves the machine.
    #[tokio::test]
    async fn selecting_geojs_resolves_in_the_background_and_reaches_the_public_list() {
        let (_dir, state) = test_state().await;
        let stub = crate::geo_external::stub::StubServer::start(vec![
            crate::geo_external::stub::StubReply::document(
                r#"{"ip":"89.160.20.112","country":"Sweden","country_code":"SE"}"#,
            ),
        ])
        .await;
        let state = state.with_external_geo_client(
            crate::geo::GeoProvider::GeoJs,
            std::sync::Arc::new(crate::geo_external::ExternalGeoClient::for_tests(
                crate::geo::GeoProvider::GeoJs
                    .external_profile()
                    .expect("GeoJS resolves outside this Server"),
                stub.base_url(),
                std::time::Duration::from_secs(2),
            )),
        );
        seed_geo_network(&state, "mainnet", &["geo-geojs-a", "geo-geojs-b"]).await;
        mark_peer_snapshot(&state, "geo-geojs-a", 1).await;
        // Peer rows exist only where a Peer Snapshot value was accepted, so
        // both Nodes report one and the Network reading counts every row.
        mark_peer_snapshot(&state, "geo-geojs-b", 1).await;
        // One address shared by three Peer records across two Nodes, plus a
        // documentation address the trust boundary refuses.
        for (node_id, peer_id, remote_ip) in [
            ("geo-geojs-a", "gj-1", "89.160.20.112"),
            ("geo-geojs-a", "gj-2", "89.160.20.112"),
            ("geo-geojs-b", "gj-3", "89.160.20.112"),
            ("geo-geojs-b", "gj-4", "203.0.113.9"),
        ] {
            insert_geo_peer(&state, node_id, peer_id, Some(remote_ip)).await;
        }

        // Nothing is scheduled or sent while Disabled.
        let before = public_network_list(&state).await;
        assert_eq!(before["geo"]["state"], "disabled");
        assert!(before["geo"]["knownCountryCount"].is_null());
        assert_eq!(stub.request_count(), 0);

        // The Owner selects GeoJS through the real Admin API.
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        let session = crate::http::AuthenticatedSession(crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: time::OffsetDateTime::now_utc(),
            last_seen_at: time::OffsetDateTime::now_utc(),
            expires_at: time::OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        });
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        headers.insert(
            axum::http::header::ORIGIN,
            "http://127.0.0.1:8080".parse().unwrap(),
        );
        headers.insert("x-csrf-token", "csrf".parse().unwrap());
        let response = crate::http::admin::update_geo_provider(
            State(state.clone()),
            headers,
            axum::Extension(session),
            axum::Extension(crate::http::RequestId(std::sync::Arc::from("req-geojs"))),
            Json(crate::http::admin::GeoProviderUpdateRequest {
                provider: "geojs".to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // The background pass resolves the shared address once, through the
        // GeoJS path, and keeps only the two-letter code.
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        assert_eq!(
            crate::geo_backfill::run_pass(&state, &now)
                .await
                .unwrap()
                .resolved,
            1
        );
        assert_eq!(stub.request_count(), 1, "one query per canonical address");
        assert_eq!(
            stub.requests()[0].path,
            "/v1/ip/geo/89.160.20.112.json",
            "the fixed GeoJS destination, never a configured one"
        );

        let after = public_network_list(&state).await;
        assert_eq!(after["geo"]["state"], "current");
        assert_eq!(after["geo"]["knownCountryCount"], 3);
        assert_eq!(after["geo"]["unknownCountryCount"], 1);
        assert_eq!(after["geo"]["countries"][0]["countryCode"], "SE");
        assert_eq!(
            after["geo"]["countries"][0]["count"], 3,
            "counts stay per Peer record, not per deduplicated address"
        );
        assert_eq!(
            after["geo"]["attribution"].as_str().unwrap(),
            crate::geo_geojs::GEOJS_ATTRIBUTION,
            "GeoJS is credited wherever its results are shown"
        );
        let text = after.to_string();
        assert!(!text.contains("89.160.20.112"), "no raw Peer address");
        assert!(
            !text.contains("/v1/ip/geo/"),
            "no external request path or provider error crosses the boundary"
        );
        assert!(
            !text.contains("Sweden"),
            "GeoJS's country *name* is discarded; only the two-letter code is retained"
        );
    }

    /// One Network's Public Geo Insight from the real Public list route.
    async fn public_network_list(state: &AppState) -> serde_json::Value {
        let value = networks_value(state).await;
        value
            .as_array()
            .unwrap()
            .iter()
            .find(|network| network["networkKey"] == "mainnet")
            .cloned()
            .expect("the seeded Network is in the Public list")
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
            sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, 'agent-public-test', 'mainnet', $2, 'ws://127.0.0.1:1', $3, $4, 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
                .bind(node_id).bind(format!("{node_id} display")).bind(lifecycle).bind(visibility)
                .execute(state.db().pool()).await.unwrap();
        }
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, received_at, observation_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-public', 'node-public', 'rpc', 'ok', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 1, 1)")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, received_at, observation_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-public', 'node-public', 'process', 'ok', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 1, 1)")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO current_host_observations (agent_id, cpu_percent, memory_total_bytes, memory_used_bytes, updated_at) VALUES ('agent-public-test', 42.5, 10000, 5000, '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO current_node_process_observations (node_id, pid, started_at, cpu_percent, memory_bytes, uptime_ms, updated_at) VALUES ('node-public', 100, '2026-01-01T00:00:00Z', 12.5, 2500, 1000, '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        for (mount_path, total_bytes, used_bytes) in
            [("/", 1_000_i64, 500_i64), ("/data", 2_000_i64, 1_600_i64)]
        {
            sqlx::query("INSERT INTO current_host_disk_mounts (agent_id, mount_path, total_bytes, used_bytes, updated_at) VALUES ('agent-public-test', $1, $2, $3, '2026-01-01T00:00:00Z')")
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
        sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ($1, $2, $3, '0xparent', '0xgenesis', 1, 1, 'lat', 1, $4, $5, 'subscription', '0x0000000000000000000000000000000000000000', 'unknown', 'unknown', 'test', $6)")
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
            sqlx::query("INSERT INTO node_metric_samples (node_id, metric, source_observed_at, observed_at, received_at, value) VALUES ('node-public', $1, $2, $3, $4, $5)")
                .bind(metric)
                .bind(observed_at)
                .bind(observed_at)
                .bind(received_at)
                .bind(value)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO host_metric_samples (agent_id, metric, source_observed_at, observed_at, received_at, value) VALUES ('agent-public-test', 'network_rx_bytes_per_sec', '2026-01-01T00:01:00Z', '2026-01-01T00:01:00Z', $1, 2048)")
            .bind(&recent)
            .execute(state.db().pool())
            .await
            .unwrap();
        seed_block_summary(&state, "node-public", 99, 3).await;
        seed_block_summary(&state, "node-public", 100, 7).await;
        sqlx::query("UPDATE block_summaries SET block_timestamp_ms=CASE block_number WHEN 99 THEN 1000 ELSE 3000 END, observed_at=CASE block_number WHEN 99 THEN $1 ELSE $2 END, accepted_at=CASE block_number WHEN 99 THEN $3 ELSE $4 END WHERE node_id='node-public' AND block_number IN (99, 100)")
            .bind(&old)
            .bind(&recent)
            .bind(&old)
            .bind(&recent)
            .execute(state.db().pool())
            .await
            .unwrap();

        let history = metrics_value(&state, "node-public", PublicMetricWindow::Live)
            .await
            .unwrap();
        assert_eq!(history["windowSeconds"], 60);
        assert_eq!(history["processCpuPercent"].as_array().unwrap().len(), 2);
        assert_eq!(history["processCpuPercent"][0]["value"], 10.0);
        assert_eq!(history["processCpuPercent"][1]["value"], 20.0);
        assert_eq!(history["dataDirectoryPercent"][0]["value"], 25.0);
        assert_eq!(history["networkRxBytesPerSec"][0]["value"], 2048.0);
        assert_eq!(history["blockIntervalMs"][0]["value"], 2000.0);
        assert_eq!(history["transactionCount"][1]["value"], 7.0);

        assert!(
            metrics_value(&state, "node-private", PublicMetricWindow::Live)
                .await
                .is_some()
        );
    }

    // ---- Issue #247 / ADR 0012: the Node Detail Metric Window ----

    /// Call the real handler with a window parameter and read its JSON body.
    async fn metric_window_answer(
        state: &AppState,
        window: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let response = public_node_metrics(
            State(state.clone()),
            Path("node-public".to_owned()),
            Ok(Query(PublicNodeMetricsQuery {
                window: window.map(str::to_owned),
            })),
            Extension(RequestId(std::sync::Arc::from("metric-window-test"))),
        )
        .await;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    /// One observation delivered the way ingestion delivers it, in a receipt
    /// transaction, against a raw window that has already moved past it: it is
    /// counted into both aggregate tiers and the ledger, but no raw row is
    /// written, so a window that still answered from raw samples would find
    /// nothing here. Deliveries to one series go in ascending order, as an
    /// Agent's new observations do.
    async fn count_observation(
        state: &AppState,
        node_scoped: bool,
        metric: &str,
        observed_at: &str,
        value: f64,
    ) {
        deliver_observation(
            state,
            node_scoped,
            metric,
            observed_at,
            value,
            "9999-12-31T23:59:59Z",
        )
        .await;
    }

    /// One observation delivered while the raw window still holds it: its raw
    /// row is kept and it is counted into both aggregate tiers and the ledger.
    async fn keep_observation(state: &AppState, metric: &str, observed_at: &str, value: f64) {
        deliver_observation(
            state,
            true,
            metric,
            observed_at,
            value,
            "1970-01-01T00:00:00Z",
        )
        .await;
    }

    async fn deliver_observation(
        state: &AppState,
        node_scoped: bool,
        metric: &str,
        observed_at: &str,
        value: f64,
        raw_cutoff: &str,
    ) {
        let scope = if node_scoped {
            crate::metric_history::SeriesScope::node("node-public", metric)
        } else {
            crate::metric_history::SeriesScope::host("agent-public-test", metric, "")
        };
        let instant: platpulse_core::Rfc3339 = observed_at.parse().unwrap();
        let mut tx = crate::database::begin(state.db().pool()).await.unwrap();
        let delivery = crate::metric_history::record(
            &mut tx,
            &crate::capacity::HistoryGate::Record,
            &crate::metric_history::RawWindow::with_cutoff(raw_cutoff),
            crate::metric_history::MetricDelivery {
                scope,
                identity: instant,
                timeline: instant,
                received_at: observed_at,
                value,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(delivery, crate::metric_history::Delivery::Observed);
    }

    async fn keep_raw_sample(state: &AppState, metric: &str, observed_at: &str, value: f64) {
        sqlx::query("INSERT INTO node_metric_samples (node_id, metric, source_observed_at, observed_at, received_at, value) VALUES ('node-public', $1, $2, $3, $4, $5)")
            .bind(metric)
            .bind(observed_at)
            .bind(observed_at)
            .bind(observed_at)
            .bind(value)
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    /// The current instant, between two bucket edges, so a test that seeds the
    /// open bucket and then asks for the window cannot watch that bucket close.
    ///
    /// Both margins matter. The far one is generous on purpose: a seeded bucket
    /// has to stay open for the whole test, and a loaded suite where the
    /// PostgreSQL statements and the HTTP round trip take seconds must not
    /// straddle the edge. The near one keeps the instants a test derives from
    /// this one inside the bucket it belongs to - samples seeded a couple of
    /// seconds behind it, and a bucket-relative offset that runs ahead of it -
    /// which at exactly the edge would land in the closed bucket or in the
    /// future (the flake that failed two window tests at 00:00 of a 5m edge).
    async fn settled_now(grain_seconds: i64) -> time::OffsetDateTime {
        const MARGIN_SECONDS: i64 = 20;
        const EDGE_SECONDS: i64 = 5;
        loop {
            let now = crate::auth::now_utc();
            let into_bucket = now.unix_timestamp().rem_euclid(grain_seconds);
            if into_bucket >= EDGE_SECONDS && grain_seconds - into_bucket > MARGIN_SECONDS {
                return time::OffsetDateTime::from_unix_timestamp(now.unix_timestamp()).unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    fn bucket_floor(instant: time::OffsetDateTime, grain_seconds: i64) -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp(
            instant.unix_timestamp().div_euclid(grain_seconds) * grain_seconds,
        )
        .unwrap()
    }

    #[test]
    fn the_metric_window_is_a_fixed_enum_with_a_server_owned_tier() {
        use crate::metric_history::WindowTier;
        assert_eq!(
            PublicMetricWindow::parse(None),
            Some(PublicMetricWindow::Live)
        );
        assert_eq!(
            PublicMetricWindow::parse(Some("live")),
            Some(PublicMetricWindow::Live)
        );
        for (raw, window, seconds, tier) in [
            ("1h", PublicMetricWindow::OneHour, 3_600, WindowTier::Raw),
            (
                "6h",
                PublicMetricWindow::SixHours,
                21_600,
                WindowTier::OneMinute,
            ),
            (
                "24h",
                PublicMetricWindow::TwentyFourHours,
                86_400,
                WindowTier::FiveMinute,
            ),
            (
                "7d",
                PublicMetricWindow::SevenDays,
                604_800,
                WindowTier::FiveMinute,
            ),
        ] {
            assert_eq!(PublicMetricWindow::parse(Some(raw)), Some(window), "{raw}");
            assert_eq!(window.seconds(), seconds, "{raw}");
            assert_eq!(window.tier(), tier, "{raw}");
        }
        assert_eq!(PublicMetricWindow::Live.seconds(), 60);
        // No arbitrary range, no 30-day preset, no spelling variants.
        for raw in ["", "LIVE", "1H", "2h", "30d", "720h", "3600", " 1h", "1h "] {
            assert_eq!(PublicMetricWindow::parse(Some(raw)), None, "{raw:?}");
        }
    }

    #[tokio::test]
    async fn an_unknown_metric_window_is_refused_before_the_node_is_read() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let (status, body) = metric_window_answer(&state, Some("30d")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "invalid_metric_window");
        // A repeated parameter the extractor cannot read is refused with the
        // same error body, not the framework's plain-text rejection.
        let repeated = Query::<PublicNodeMetricsQuery>::try_from_uri(
            &"http://platpulse.test/metrics?window=1h&window=6h"
                .parse()
                .unwrap(),
        );
        assert!(repeated.is_err());
        let response = public_node_metrics(
            State(state.clone()),
            Path("node-public".to_owned()),
            repeated,
            Extension(RequestId(std::sync::Arc::from("metric-window-repeated"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "invalid_metric_window");
        // A missing Node is still a 404 for a valid window, and a 400 for an
        // invalid one: the refusal does not depend on which Node was named.
        let response = public_node_metrics(
            State(state.clone()),
            Path("node-missing".to_owned()),
            Ok(Query(PublicNodeMetricsQuery {
                window: Some("bogus".to_owned()),
            })),
            Extension(RequestId(std::sync::Arc::from("metric-window-missing"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = public_node_metrics(
            State(state),
            Path("node-missing".to_owned()),
            Ok(Query(PublicNodeMetricsQuery {
                window: Some("6h".to_owned()),
            })),
            Extension(RequestId(std::sync::Arc::from("metric-window-missing"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// Live is answered field for field as before the Metric Window: the same
    /// keys, no window descriptor, no bucket on any point, and the two block
    /// series still present.
    #[tokio::test]
    async fn the_live_window_keeps_its_answer_field_for_field() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = settled_now(60).await;
        let recent = format_rfc3339(now - time::Duration::seconds(10));
        keep_observation(&state, "process_cpu_percent", &recent, 20.0).await;
        seed_block_summary(&state, "node-public", 100, 7).await;
        for window in [None, Some("live")] {
            let body = metric_window_value(&state, window).await;
            let mut keys: Vec<&str> = body
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                [
                    "blockIntervalMs",
                    "dataDirectoryPercent",
                    "from",
                    "networkRxBytesPerSec",
                    "networkTxBytesPerSec",
                    "peerInboundCount",
                    "peerOutboundCount",
                    "processCpuPercent",
                    "processMemoryPercent",
                    "series",
                    "to",
                    "transactionCount",
                    "windowSeconds",
                ]
            );
            assert_eq!(body["windowSeconds"], 60);
            assert_eq!(body["series"].as_array().unwrap().len(), 9);
            let cpu = body["processCpuPercent"].as_array().unwrap();
            assert_eq!(cpu.len(), 1);
            let mut point_keys: Vec<&str> = cpu[0]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            point_keys.sort_unstable();
            assert_eq!(point_keys, ["sampledAt", "value"]);
            assert_eq!(body["transactionCount"][0]["value"], 7.0);
        }
    }

    /// 6h is answered from 1-minute buckets alone: a raw sample the window
    /// still holds is not drawn, a bucket is drawn by its maximum with the rest
    /// of it beside the point, and the newest bucket is open. Since #248 every
    /// series is provided, so nothing is named as unavailable.
    #[tokio::test]
    async fn a_six_hour_window_reads_one_minute_buckets_by_their_maximum() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = settled_now(60).await;
        let current = bucket_floor(now, 60);
        let previous = current - time::Duration::seconds(60);
        let at = |base: time::OffsetDateTime, seconds: i64| {
            format_rfc3339(base + time::Duration::seconds(seconds))
        };
        // The previous minute: three observations, a spike in the middle.
        for (seconds, value) in [(5, 10.0), (25, 90.0), (45, 30.0)] {
            count_observation(
                &state,
                true,
                "process_cpu_percent",
                &at(previous, seconds),
                value,
            )
            .await;
        }
        // The current, still-accumulating minute.
        count_observation(
            &state,
            true,
            "process_cpu_percent",
            &format_rfc3339(current),
            12.0,
        )
        .await;
        // A raw sample with no bucket: the 6h answer must not borrow it.
        keep_raw_sample(&state, "process_memory_percent", &at(previous, 10), 44.0).await;
        count_observation(
            &state,
            false,
            "network_rx_bytes_per_sec",
            &at(previous, 5),
            2048.0,
        )
        .await;

        let body = metric_window_value(&state, Some("6h")).await;
        assert_eq!(body["windowSeconds"], 21_600);
        assert_eq!(body["metricWindow"]["window"], "6h");
        assert_eq!(body["metricWindow"]["grain"], "1m");
        assert_eq!(body["metricWindow"]["grainSeconds"], 60);
        assert_eq!(
            body["metricWindow"]["unavailableSeries"],
            serde_json::json!([])
        );
        // No block was accepted: both block series are present and empty.
        assert_eq!(body["blockIntervalMs"], serde_json::json!([]));
        assert_eq!(body["transactionCount"], serde_json::json!([]));
        let metrics: Vec<&str> = body["series"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["metric"].as_str().unwrap())
            .collect();
        assert_eq!(metrics, PUBLIC_METRIC_SERIES);
        // The window starts on a bucket edge and spans the whole six hours.
        let from = crate::auth::parse_rfc3339(body["from"].as_str().unwrap()).unwrap();
        let to = crate::auth::parse_rfc3339(body["to"].as_str().unwrap()).unwrap();
        assert_eq!(from.unix_timestamp() % 60, 0);
        assert!((to - from).whole_seconds() >= 21_600);

        let cpu = body["processCpuPercent"].as_array().unwrap();
        assert_eq!(cpu.len(), 2, "{cpu:?}");
        assert_eq!(cpu[0]["sampledAt"], format_rfc3339(previous));
        assert_eq!(cpu[0]["value"], 90.0, "a bucket is drawn by its maximum");
        let bucket = &cpu[0]["bucket"];
        assert_eq!(bucket["start"], format_rfc3339(previous));
        assert_eq!(bucket["end"], format_rfc3339(current));
        assert_eq!(bucket["grainSeconds"], 60);
        assert_eq!(bucket["minValue"], 10.0);
        assert_eq!(bucket["maxValue"], 90.0);
        assert_eq!(bucket["lastValue"], 30.0);
        assert_eq!(bucket["sampleCount"], 3);
        assert_eq!(bucket["open"], false);
        assert_eq!(cpu[1]["bucket"]["open"], true, "the newest minute is open");
        assert_eq!(cpu[1]["bucket"]["sampleCount"], 1);
        let cpu_coverage = body["series"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["metric"] == "processCpuPercent")
            .unwrap();
        assert_eq!(cpu_coverage["observed"], true);
        assert_eq!(cpu_coverage["observationCount"], 4);
        assert_eq!(cpu_coverage["gaps"], serde_json::json!([]));

        // Raw-only evidence is unobserved at this grain: no point, no zero.
        assert_eq!(body["processMemoryPercent"], serde_json::json!([]));
        let rx = body["networkRxBytesPerSec"].as_array().unwrap();
        assert_eq!(rx.len(), 1);
        assert_eq!(rx[0]["value"], 2048.0);
    }

    /// 1h is answered from raw samples - exactly the stored observations, with
    /// no bucket - and 24h/7d from 5-minute buckets, across the raw/aggregate
    /// boundary: each preset reads its own tier, never the other one.
    #[tokio::test]
    async fn each_longer_window_reads_only_its_own_tier() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = settled_now(300).await;
        let recent = format_rfc3339(now - time::Duration::seconds(2));
        let older = format_rfc3339(now - time::Duration::seconds(30 * 60));
        let two_days = format_rfc3339(now - time::Duration::days(2));
        count_observation(&state, true, "peer_inbound_count", &two_days, 9.0).await;
        for (instant, value) in [(&older, 3.0), (&recent, 4.0)] {
            keep_observation(&state, "peer_inbound_count", instant, value).await;
        }

        let hour = metric_window_value(&state, Some("1h")).await;
        assert_eq!(hour["metricWindow"]["grain"], "raw");
        assert_eq!(hour["metricWindow"]["grainSeconds"], 0);
        let points = hour["peerInboundCount"].as_array().unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0]["sampledAt"], older.as_str());
        assert_eq!(points[1]["value"], 4.0);
        assert!(points.iter().all(|point| point.get("bucket").is_none()));
        let hour_coverage = &hour["series"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["metric"] == "peerInboundCount")
            .unwrap()
            .clone();
        // Two observations 28 minutes apart are a silence by the long-history
        // rule (the cadence is capped, the floor is two minutes).
        assert_eq!(hour_coverage["gaps"].as_array().unwrap().len(), 1);
        assert_eq!(hour_coverage["gaps"][0]["kind"], "collection_gap");

        let day = metric_window_value(&state, Some("24h")).await;
        assert_eq!(day["metricWindow"]["grain"], "5m");
        let points = day["peerInboundCount"].as_array().unwrap();
        assert_eq!(points.len(), 2, "two days ago is outside 24h: {points:?}");
        assert!(
            points
                .iter()
                .all(|point| point["bucket"]["grainSeconds"] == 300)
        );
        assert_eq!(
            points[1]["bucket"]["open"], true,
            "the newest bucket is open: {points:?}"
        );

        let week = metric_window_value(&state, Some("7d")).await;
        assert_eq!(week["metricWindow"]["grain"], "5m");
        assert_eq!(week["windowSeconds"], 604_800);
        let points = week["peerInboundCount"].as_array().unwrap();
        assert_eq!(points.len(), 3);
        assert_eq!(points[0]["value"], 9.0);
        // Two days ago to half an hour ago, and half an hour ago to now: the
        // windows between those buckets were never written, so both are gaps.
        let week_gaps = week["series"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["metric"] == "peerInboundCount")
            .unwrap()["gaps"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(week_gaps, 2);
    }

    /// A missing bucket inside the window is a gap, never a line and never a
    /// zero; a series observed only before the tiers existed (raw rows, no
    /// buckets) is unobserved at an aggregate grain even though its ledger dates
    /// it.
    #[tokio::test]
    async fn a_missing_bucket_is_a_gap_and_pre_tier_history_is_unobserved() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = settled_now(300).await;
        let current = bucket_floor(now, 300);
        // Two neighbouring 5-minute buckets, then one missing, then the open one.
        for back in [4, 3, 1] {
            let start = current - time::Duration::seconds(300 * back);
            for offset in (0..300).step_by(60) {
                count_observation(
                    &state,
                    false,
                    "network_tx_bytes_per_sec",
                    &format_rfc3339(start + time::Duration::seconds(offset)),
                    100.0 + offset as f64,
                )
                .await;
            }
        }
        // Pre-#214 history: a raw sample and its ledger row, no bucket at all.
        let before_tiers = format_rfc3339(now - time::Duration::hours(3));
        keep_raw_sample(&state, "process_cpu_percent", &before_tiers, 50.0).await;
        sqlx::query("INSERT INTO node_metric_series_state (node_id, metric, first_observed_at, last_observed_at, last_received_at, observation_count, updated_at) VALUES ('node-public', 'process_cpu_percent', $1, $2, $3, 1, $4)")
            .bind(&before_tiers)
            .bind(&before_tiers)
            .bind(&before_tiers)
            .bind(&before_tiers)
            .execute(state.db().pool())
            .await
            .unwrap();

        let body = metric_window_value(&state, Some("24h")).await;
        let tx = body["networkTxBytesPerSec"].as_array().unwrap();
        assert_eq!(tx.len(), 3);
        assert!(tx.iter().all(|point| point["value"] == 340.0));
        let coverage = |metric: &str| {
            body["series"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["metric"] == metric)
                .unwrap()
                .clone()
        };
        let tx_coverage = coverage("networkTxBytesPerSec");
        let gaps = tx_coverage["gaps"].as_array().unwrap();
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        let second = current - time::Duration::seconds(300 * 3);
        assert_eq!(
            gaps[0]["from"],
            format_rfc3339(second + time::Duration::seconds(240))
        );
        assert_eq!(
            gaps[0]["to"],
            format_rfc3339(current - time::Duration::seconds(300))
        );

        assert_eq!(body["processCpuPercent"], serde_json::json!([]));
        let cpu = coverage("processCpuPercent");
        assert_eq!(cpu["observed"], true, "the ledger still dates the series");
        assert_eq!(cpu["observationCount"], 0);
        assert_eq!(cpu["lastObservedAt"], before_tiers.as_str());
        // Never observed at all: no ledger, no bucket, nothing drawn.
        let memory = coverage("processMemoryPercent");
        assert_eq!(memory["observed"], false);
        assert_eq!(memory["neverObservedProven"], true);
    }

    // ---- Issue #248: the block aggregate tiers behind the longer windows ----

    /// One block accepted the way ingestion accepts it: its Block Summary row
    /// and its bucket advance in one transaction.
    async fn accept_block(
        state: &AppState,
        height: i64,
        timestamp_ms: i64,
        observed_at: &str,
        transactions: i64,
    ) {
        let mut tx = crate::database::begin(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ('node-public', $1, $2, '0xparent', '0xgenesis', 1, 1, 'lat', $3, $4, $5, 'subscription', '0x0000000000000000000000000000000000000000', 'unknown', 'unknown', 'test', $6)")
            .bind(height)
            .bind(format!("0xblock-{height}"))
            .bind(timestamp_ms)
            .bind(observed_at)
            .bind(transactions)
            .bind(observed_at)
            .execute(&mut *tx)
            .await
            .unwrap();
        crate::block_aggregates::record_accepted_block(
            &mut tx,
            crate::block_aggregates::AcceptedBlock {
                node_id: "node-public",
                block_number: height,
                block_timestamp_ms: timestamp_ms,
                observed_at,
                transaction_count: transactions,
            },
            observed_at,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    fn series_coverage(body: &serde_json::Value, metric: &str) -> serde_json::Value {
        body["series"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["metric"] == metric)
            .unwrap_or_else(|| panic!("{metric} has no coverage: {body}"))
            .clone()
    }

    /// Four 5-minute buckets back: one block; two buckets back nothing; the
    /// previous bucket: three blocks; the current, open bucket: one block.
    async fn seed_block_tiers(state: &AppState, current: time::OffsetDateTime) {
        let at = |seconds: i64| format_rfc3339(current + time::Duration::seconds(seconds));
        accept_block(state, 90, 1_000, &at(-1_200 + 10), 6).await;
        accept_block(state, 100, 10_000, &at(-300 + 10), 2).await;
        accept_block(state, 101, 12_000, &at(-300 + 12), 9).await;
        accept_block(state, 102, 15_000, &at(-300 + 14), 4).await;
        accept_block(state, 103, 16_000, &at(1), 1).await;
    }

    /// 24h and 7d draw both block series from the 5-minute block tier: an
    /// interval bucket by its longest interval, a transaction bucket by the
    /// most transactions one of its blocks carried, with the bucket's total
    /// beside it. A bucket nobody wrote is a gap, and a bucket whose blocks
    /// have no held parent has no interval point at all, never a zero.
    #[tokio::test]
    async fn a_long_window_draws_the_block_tiers_by_their_maximum() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = settled_now(300).await;
        let current = bucket_floor(now, 300);
        seed_block_tiers(&state, current).await;

        for window in ["24h", "7d"] {
            let body = metric_window_value(&state, Some(window)).await;
            assert_eq!(
                body["metricWindow"]["unavailableSeries"],
                serde_json::json!([])
            );
            let metrics: Vec<&str> = body["series"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["metric"].as_str().unwrap())
                .collect();
            assert_eq!(metrics, PUBLIC_METRIC_SERIES, "{window}");

            let intervals = body["blockIntervalMs"].as_array().unwrap();
            assert_eq!(intervals.len(), 2, "{window}: {intervals:?}");
            let previous = format_rfc3339(current - time::Duration::seconds(300));
            assert_eq!(intervals[0]["sampledAt"], previous.as_str());
            assert_eq!(intervals[0]["value"], 3_000.0, "the longest interval");
            let bucket = &intervals[0]["bucket"];
            assert_eq!(bucket["minValue"], 2_000.0);
            assert_eq!(bucket["maxValue"], 3_000.0);
            assert_eq!(bucket["lastValue"], 3_000.0);
            assert_eq!(bucket["sampleCount"], 2, "block 100 has no held parent");
            assert_eq!(bucket["grainSeconds"], 300);
            assert_eq!(bucket["open"], false);
            assert!(bucket.get("totalValue").is_none());
            assert_eq!(intervals[1]["value"], 1_000.0);
            assert_eq!(intervals[1]["bucket"]["open"], true);

            let transactions = body["transactionCount"].as_array().unwrap();
            assert_eq!(transactions.len(), 3, "{window}: {transactions:?}");
            assert_eq!(transactions[1]["sampledAt"], previous.as_str());
            assert_eq!(transactions[1]["value"], 9.0, "the busiest block");
            let bucket = &transactions[1]["bucket"];
            assert_eq!(bucket["minValue"], 2.0);
            assert_eq!(bucket["lastValue"], 4.0);
            assert_eq!(bucket["sampleCount"], 3);
            assert_eq!(bucket["totalValue"], 15.0);
            assert_eq!(transactions[2]["bucket"]["open"], true);

            // Blocks have no value-less ledger, so an absence is never proof.
            let tx_coverage = series_coverage(&body, "transactionCount");
            assert_eq!(tx_coverage["observed"], true);
            assert_eq!(tx_coverage["neverObservedProven"], false);
            assert_eq!(tx_coverage["observationCount"], 5);
            let gaps = tx_coverage["gaps"].as_array().unwrap();
            assert_eq!(gaps.len(), 1, "{window}: {gaps:?}");
            assert_eq!(gaps[0]["kind"], "collection_gap");
            assert_eq!(
                gaps[0]["from"],
                format_rfc3339(current - time::Duration::seconds(1_190))
            );
            assert_eq!(
                gaps[0]["to"],
                format_rfc3339(current - time::Duration::seconds(290))
            );
            let interval_coverage = series_coverage(&body, "blockIntervalMs");
            assert_eq!(interval_coverage["observationCount"], 3);
            assert_eq!(interval_coverage["gaps"], serde_json::json!([]));
        }
    }

    /// 6h reads the 1-minute block tier and 1h the held Block Summaries
    /// themselves, with the interval of each block to its held parent and no
    /// bucket on any point; history older than the tier is unobserved.
    #[tokio::test]
    async fn the_short_windows_read_the_minute_tier_and_the_raw_blocks() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = settled_now(300).await;
        let current = bucket_floor(now, 300);
        seed_block_tiers(&state, current).await;
        // Held from before the tiers existed: a Block Summary with no bucket.
        sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ('node-public', 50, '0xblock-50', '0xparent', '0xgenesis', 1, 1, 'lat', 500, $1, 8, 'subscription', '0x0000000000000000000000000000000000000000', 'unknown', 'unknown', 'test', $2)")
            .bind(format_rfc3339(now - time::Duration::hours(3)))
            .bind(format_rfc3339(now - time::Duration::hours(3)))
            .execute(state.db().pool())
            .await
            .unwrap();

        let six = metric_window_value(&state, Some("6h")).await;
        assert_eq!(six["metricWindow"]["grain"], "1m");
        let transactions = six["transactionCount"].as_array().unwrap();
        assert_eq!(transactions.len(), 3, "{transactions:?}");
        assert!(
            transactions
                .iter()
                .all(|point| point["bucket"]["grainSeconds"] == 60)
        );
        assert!(
            transactions.iter().all(|point| point["value"] != 8.0),
            "pre-tier history is never drawn from a bucket"
        );
        let intervals = six["blockIntervalMs"].as_array().unwrap();
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[0]["value"], 3_000.0);

        let hour = metric_window_value(&state, Some("1h")).await;
        assert_eq!(hour["metricWindow"]["grain"], "raw");
        let transactions = hour["transactionCount"].as_array().unwrap();
        let counts: Vec<f64> = transactions
            .iter()
            .map(|point| point["value"].as_f64().unwrap())
            .collect();
        assert_eq!(counts, [6.0, 2.0, 9.0, 4.0, 1.0]);
        assert!(
            transactions
                .iter()
                .all(|point| point.get("bucket").is_none())
        );
        let intervals: Vec<f64> = hour["blockIntervalMs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|point| point["value"].as_f64().unwrap())
            .collect();
        assert_eq!(intervals, [2_000.0, 3_000.0, 1_000.0]);
        assert_eq!(
            series_coverage(&hour, "blockIntervalMs")["observationCount"],
            3
        );

        // A block backfilled at a low height is read by the instant it was
        // observed, not lost behind the newer heights.
        accept_block(
            &state,
            7,
            400,
            &format_rfc3339(now - time::Duration::seconds(30)),
            11,
        )
        .await;
        let hour = metric_window_value(&state, Some("1h")).await;
        let counts: Vec<f64> = hour["transactionCount"]
            .as_array()
            .unwrap()
            .iter()
            .map(|point| point["value"].as_f64().unwrap())
            .collect();
        assert!(counts.contains(&11.0), "{counts:?}");
    }

    /// A backfilled block keeps an old height but carries the instant it was
    /// really observed, so the series is dated by that instant and never by the
    /// order of heights (issue #225).
    #[tokio::test]
    async fn public_node_metrics_dates_a_backfilled_block_by_its_observation() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::now_utc();
        let stamp = |offset: i64| format_rfc3339(now - time::Duration::seconds(offset));
        // Height and time disagree here: the oldest height arrived last, as the
        // GapBackfill of a block the Agent had missed, and the newest height is
        // the one observed twenty-five seconds ago.
        for (height, offset, block_timestamp_ms) in [
            (99_i64, 90_i64, 1_000_i64),
            (100, 5, 2_000),
            (101, 15, 3_000),
            (102, 25, 4_000),
        ] {
            sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ('node-public', $1, $2, '0xparent', '0xgenesis', 1, 1, 'lat', $3, $4, 1, 'subscription', '0x0000000000000000000000000000000000000000', 'unknown', 'unknown', 'test', $5)")
                .bind(height)
                .bind(format!("0xpublic-backfilled-{height}"))
                .bind(block_timestamp_ms)
                .bind(stamp(offset))
                .bind(stamp(offset))
                .execute(state.db().pool())
                .await
                .unwrap();
        }

        let history = metrics_value(&state, "node-public", PublicMetricWindow::Live)
            .await
            .unwrap();
        let sampled: Vec<String> = history["blockIntervalMs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|point| point["sampledAt"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            sampled,
            vec![stamp(25), stamp(15), stamp(5)],
            "the chart draws them in observation order"
        );
        let coverage = history["series"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["metric"] == "blockIntervalMs")
            .cloned()
            .expect("the answer carries the block interval series");
        assert_eq!(coverage["observationCount"], 3);
        assert_eq!(coverage["cadenceSeconds"], 10);
        assert_eq!(coverage["gapThresholdSeconds"], 30);
        assert_eq!(coverage["coveredSeconds"], 20);
        assert_eq!(coverage["firstObservedAt"], stamp(25).as_str());
        assert_eq!(coverage["lastObservedAt"], stamp(5).as_str());
        let tail = coverage["unobservedTailSeconds"].as_i64().unwrap();
        assert!((5..=7).contains(&tail), "block interval tail was {tail}");
    }

    /// Retention expires the sample; the ledger is what keeps the series a
    /// series that was observed, dated by when it was really observed, with an
    /// empty chart and no invented point (issue #225, Story 49).
    #[tokio::test]
    async fn public_node_metrics_dates_a_series_whose_samples_expired() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::now_utc();
        let stamp = |offset: i64| format_rfc3339(now - time::Duration::seconds(offset));
        sqlx::query("INSERT INTO node_metric_samples (node_id, metric, source_observed_at, observed_at, received_at, value) VALUES ('node-public', 'process_cpu_percent', $1, $2, $3, 12.5)")
            .bind(stamp(4000))
            .bind(stamp(4000))
            .bind(stamp(4000))
            .execute(state.db().pool())
            .await
            .unwrap();
        // The ledger row report ingestion writes beside the sample.
        sqlx::query("INSERT INTO node_metric_series_state (node_id, metric, first_observed_at, last_observed_at, last_received_at, observation_count, updated_at) VALUES ('node-public', 'process_cpu_percent', $1, $2, $3, 1, $4)")
            .bind(stamp(4000))
            .bind(stamp(4000))
            .bind(stamp(4000))
            .bind(stamp(4000))
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM node_metric_samples WHERE node_id='node-public' AND metric='process_cpu_percent'")
            .execute(state.db().pool())
            .await
            .unwrap();

        let history = metrics_value(&state, "node-public", PublicMetricWindow::Live)
            .await
            .unwrap();
        let series = |metric: &str| {
            history["series"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["metric"] == metric)
                .cloned()
                .unwrap_or_else(|| panic!("the answer carries the {metric} series"))
        };

        let cpu = series("processCpuPercent");
        assert_eq!(cpu["observed"], true, "the series was observed");
        assert_eq!(cpu["observationCount"], 0, "but not in this window");
        assert_eq!(cpu["firstObservedAt"], stamp(4000).as_str());
        assert_eq!(cpu["lastObservedAt"], stamp(4000).as_str());
        let tail = cpu["unobservedTailSeconds"].as_i64().unwrap();
        assert!(
            (4000..=4002).contains(&tail),
            "the ledger dates the tail instead of zeroing it, got {tail}"
        );
        assert_eq!(cpu["cadenceSeconds"], 0);
        assert_eq!(cpu["gaps"].as_array().unwrap().len(), 0);
        assert_eq!(history["processCpuPercent"].as_array().unwrap().len(), 0);
        // Only the series the ledger remembers is dated that way.
        assert_eq!(series("processMemoryPercent")["observed"], false);
    }

    /// The two block series have no ledger: `block_summaries` is the only
    /// place they live and retention removes those rows, so a block series whose
    /// rows are gone is a series nobody can date - it must not borrow an age
    /// from anywhere else (issue #225, Story 49).
    #[tokio::test]
    async fn public_node_metrics_never_dates_a_block_series_whose_rows_expired() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::now_utc();
        let stamp = |offset: i64| format_rfc3339(now - time::Duration::seconds(offset));
        for (height, offset) in [(99_i64, 25_i64), (100, 20), (101, 15)] {
            sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ('node-public', $1, $2, '0xparent', '0xgenesis', 1, 1, 'lat', $3, $4, 1, 'subscription', '0x0000000000000000000000000000000000000000', 'unknown', 'unknown', 'test', $5)")
                .bind(height)
                .bind(format!("0xpublic-expired-block-{height}"))
                .bind(2_000_i64 + height)
                .bind(stamp(offset))
                .bind(stamp(offset))
                .execute(state.db().pool())
                .await
                .unwrap();
        }

        let history = || {
            let state = state.clone();
            async move {
                let response = public_node_metrics(
                    State(state),
                    Path("node-public".to_owned()),
                    Ok(Query(PublicNodeMetricsQuery::default())),
                    Extension(RequestId(std::sync::Arc::from("metric-block-expired-test"))),
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()
            }
        };
        let series = |history: &serde_json::Value, metric: &str| -> serde_json::Value {
            history["series"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["metric"] == metric)
                .cloned()
                .unwrap_or_else(|| panic!("the answer carries the {metric} series"))
        };

        let with_rows = history().await;
        assert_eq!(
            series(&with_rows, "blockIntervalMs")["observed"],
            true,
            "the rows themselves are what makes a block series observed"
        );
        assert!(!with_rows["blockIntervalMs"].as_array().unwrap().is_empty());
        assert_eq!(
            series(&with_rows, "processMemoryPercent")["neverObservedProven"],
            true,
            "a series stored under a metric name has a ledger that can prove nobody ever reported it"
        );

        sqlx::query("DELETE FROM block_summaries WHERE node_id='node-public'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let expired = history().await;
        for metric in ["blockIntervalMs", "transactionCount"] {
            let entry = series(&expired, metric);
            assert_eq!(
                entry["observed"], false,
                "{metric} has no ledger to be dated from"
            );
            assert_eq!(
                entry["neverObservedProven"], false,
                "{metric} has no ledger, so its absence is unknown rather than proven"
            );
            assert_eq!(entry["observationCount"], 0);
            assert_eq!(entry["firstObservedAt"], serde_json::Value::Null);
            assert_eq!(entry["lastObservedAt"], serde_json::Value::Null);
            assert_eq!(
                entry["unobservedTailSeconds"], 0,
                "nothing observed means no age to name"
            );
            assert_eq!(entry["gaps"].as_array().unwrap().len(), 0);
        }
        assert_eq!(expired["blockIntervalMs"].as_array().unwrap().len(), 0);
        assert_eq!(expired["transactionCount"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn public_node_metrics_answers_the_gap_and_the_tail_of_every_series() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::now_utc();
        let stamp = |offset: i64| format_rfc3339(now - time::Duration::seconds(offset));
        // A six second cadence whose newest intervals prove it, with a hole the
        // two-minute floor of every longer answer would excuse.
        for offset in [56, 50, 44, 18, 12, 6] {
            sqlx::query("INSERT INTO node_metric_samples (node_id, metric, source_observed_at, observed_at, received_at, value) VALUES ('node-public', 'process_cpu_percent', $1, $2, $3, 12.5)")
                .bind(stamp(offset))
                .bind(stamp(offset))
                .bind(stamp(offset))
                .execute(state.db().pool())
                .await
                .unwrap();
        }
        // A series whose newest observation arrived before the window: the
        // last-good value the chart draws, which has to be dated.
        sqlx::query("INSERT INTO node_metric_samples (node_id, metric, source_observed_at, observed_at, received_at, value) VALUES ('node-public', 'process_memory_percent', $1, $2, $3, 25.0)")
            .bind(stamp(90))
            .bind(stamp(90))
            .bind(stamp(90))
            .execute(state.db().pool())
            .await
            .unwrap();

        let history = metrics_value(&state, "node-public", PublicMetricWindow::Live)
            .await
            .unwrap();
        let series = |metric: &str| {
            history["series"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["metric"] == metric)
                .cloned()
                .unwrap_or_else(|| panic!("the answer carries the {metric} series"))
        };

        let cpu = series("processCpuPercent");
        assert_eq!(cpu["observed"], true);
        assert_eq!(cpu["observationCount"], 6);
        assert_eq!(cpu["cadenceSeconds"], 6);
        assert_eq!(cpu["gapThresholdSeconds"], 18);
        assert_eq!(cpu["coveredSeconds"], 24);
        assert_eq!(cpu["gaps"].as_array().unwrap().len(), 1);
        assert_eq!(cpu["gaps"][0]["from"], stamp(44).as_str());
        assert_eq!(cpu["gaps"][0]["to"], stamp(18).as_str());
        assert_eq!(cpu["gaps"][0]["seconds"], 26);
        assert_eq!(cpu["gaps"][0]["kind"], "collection_gap");
        assert_eq!(cpu["firstObservedAt"], stamp(56).as_str());
        assert_eq!(cpu["lastObservedAt"], stamp(6).as_str());
        // The handler reads its own clock, one whole second past this test's at
        // the latest, so the tail is asserted as the small range it can be.
        let cpu_tail = cpu["unobservedTailSeconds"].as_i64().unwrap();
        assert!((6..=8).contains(&cpu_tail), "cpu tail was {cpu_tail}");

        let memory = series("processMemoryPercent");
        assert_eq!(memory["observed"], true);
        assert_eq!(memory["observationCount"], 0);
        assert_eq!(memory["firstObservedAt"], stamp(90).as_str());
        assert_eq!(memory["lastObservedAt"], stamp(90).as_str());
        assert_eq!(memory["cadenceSeconds"], 0);
        assert_eq!(memory["gapThresholdSeconds"], 0);
        assert_eq!(memory["coveredSeconds"], 0);
        assert!(memory["gaps"].as_array().unwrap().is_empty());
        let memory_tail = memory["unobservedTailSeconds"].as_i64().unwrap();
        assert!(
            (90..=92).contains(&memory_tail),
            "memory tail was {memory_tail}"
        );

        let peers = series("peerInboundCount");
        assert_eq!(peers["observed"], false);
        assert_eq!(peers["observationCount"], 0);
        assert!(peers["firstObservedAt"].is_null());
        assert!(peers["lastObservedAt"].is_null());
        assert_eq!(peers["gapThresholdSeconds"], 0);
        assert_eq!(peers["unobservedTailSeconds"], 0);
        assert!(peers["gaps"].as_array().unwrap().is_empty());

        let names: Vec<&str> = history["series"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["metric"].as_str().unwrap())
            .collect();
        assert_eq!(names, PUBLIC_METRIC_SERIES);
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
            sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, 'agent-public-test', 'mainnet', $2, 'ws://127.0.0.1:1', 'active', 'public', 1, $3, $4)")
                .bind(node_id)
                .bind(format!("{node_id} display"))
                .bind(&now)
                .bind(&now)
                .execute(state.db().pool())
                .await
                .unwrap();
            sqlx::query("INSERT INTO current_node_chain_observations (node_id, current_block, updated_at) VALUES ($1, 100, $2)")
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
            sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, updated_at) VALUES ($1, $2, $3)")
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

        let value = networks_value(&state).await;
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

        let value = networks_value(&state).await;
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
        let value = node_value(&state, "node-cons-current-true").await.unwrap();
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
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, 'agent-public-test', 'mainnet', $2, 'ws://127.0.0.1:1', 'active', 'public', 1, $3, $4)")
            .bind(node_id)
            .bind(format!("{node_id} display"))
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, observed_at, received_at, observation_at, value_received_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', $1, $2, 'consensus', $3, $4, $5, $5, $6, 1, $7)")
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
        sqlx::query("INSERT INTO current_node_chain_observations (node_id, current_block, consensus_epoch, consensus_view_number, consensus_validator, consensus_highest_qc_block, consensus_highest_lock_block, consensus_highest_commit_block, updated_at) VALUES ($1, $2, 1, 2, $3, $4, $5, $6, $7)")
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
    async fn an_obsolete_observation_renders_stale_without_a_boundary() {
        // A Report may legally stamp the far end of the calendar, so a judgement
        // of its freshness must answer Stale without doing arithmetic that the
        // calendar cannot hold: the reading states Staleness and no boundary
        // instead of failing the request (AGENTS.md: the Server is the trust
        // boundary).
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, observed_at, received_at, observation_at, value_received_at, value_observation_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-public', 'node-public', 'peers', 'ok', $1, $2, $2, $3, $4, 1, 1)")
            .bind("9999-12-31T23:59:59Z")
            .bind(&now)
            .bind(&now)
            .bind("9999-12-31T23:59:59Z")
            .execute(state.db().pool())
            .await
            .unwrap();

        let response = public_node_detail(
            State(state.clone()),
            Path("node-public".to_owned()),
            Extension(crate::http::RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["peers"]["freshness"], "stale");
        assert!(value["peers"]["staleSince"].is_null());
    }

    #[tokio::test]
    async fn public_peer_projection_is_bounded_and_preserves_dimensions() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, observed_at, received_at, observation_at, value_received_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-public', 'node-public', 'peers', 'ok', $1, $2, $2, $3, 1, 1)")
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
            sqlx::query("INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, client_name, updated_at) VALUES ('node-public', $1, '203.0.113.7', $2, $3, $4, $5, 'raw-client-name', $6)")
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

        let value = networks_value(&state).await;
        let node = value[0]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["nodeId"] == "node-public")
            .unwrap();
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
        // Network aggregates remain unknown when any Active Node has never
        // produced a Peer Snapshot; known Node values are not presented as
        // a complete Network total.
        assert_eq!(value[0]["peers"]["state"], "unknown");
        assert!(value[0]["peers"]["peerCount"].is_null());
        assert_eq!(value[0]["geo"]["state"], "disabled");
        assert!(value[0]["geo"]["countries"].is_null());
        assert!(!value.to_string().contains("203.0.113.7"));

        let stale_received =
            crate::auth::format_rfc3339(crate::auth::now_utc() - time::Duration::minutes(5));
        sqlx::query("UPDATE component_status SET state='ok', state_revision=2, observed_at=$1, received_at=$2, observation_at=$2, value_received_at=$3 WHERE node_id='node-public' AND component_key='peers'")
            .bind(&now)
            .bind(&stale_received)
            .bind(&stale_received)
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = node_value(&state, "node-public").await.unwrap();
        assert_eq!(value["peers"]["freshness"], "stale");
        assert!(value["peers"]["staleSince"].is_string());
        assert_eq!(value["peers"]["receivedAt"], stale_received);
        assert_eq!(value["peers"]["peerCount"], 2);

        sqlx::query("UPDATE component_status SET state='error', state_revision=3 WHERE node_id='node-public' AND component_key='peers'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = node_value(&state, "node-public").await.unwrap();
        assert_eq!(value["peers"]["state"], "error");
        assert_eq!(value["peers"]["receivedAt"], stale_received);
        assert_eq!(value["peers"]["peerCount"], 2);

        sqlx::query("UPDATE component_status SET state='ok', state_revision=4, value_revision=2, observed_at=$1, received_at=$2, observation_at=$2, value_received_at=$3 WHERE node_id='node-public' AND component_key='peers'")
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
        let value = node_value(&state, "node-public").await.unwrap();
        assert_eq!(value["peers"]["state"], "ok");
        assert_eq!(value["peers"]["peerCount"], 0);
        assert_eq!(value["peers"]["inboundCount"], 0);
    }

    #[tokio::test]
    async fn public_peer_projection_includes_all_active_nodes_without_raw_peer_data() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, observed_at, received_at, observation_at, value_received_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-private', 'node-private', 'peers', 'ok', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 1, 1)")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, updated_at) VALUES ('node-private', 'private-peer', '10.0.0.2', 'inbound', 1, 1, 1, '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = networks_value(&state).await;
        assert_eq!(value[0]["nodes"].as_array().unwrap().len(), 2);
        assert!(node_value(&state, "node-private").await.is_some());

        let network = public_network_list(&state).await;
        assert_eq!(network["nodes"].as_array().unwrap().len(), 2);
        assert!(network.to_string().find("private-peer").is_none());

        for node_id in ["node-private", "node-retired", "node-unknown"] {
            let listed = node_id == "node-private";
            assert_eq!(
                block_history_value(&state, node_id).await.is_some(),
                listed,
                "unexpected history visibility for {node_id}"
            );
            assert_eq!(
                peer_history_value(&state, node_id).await.is_some(),
                listed,
                "unexpected peer history visibility for {node_id}"
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
        let node = node_value(&state, "node-public").await.unwrap();
        assert_eq!(node["processCpuPercent"], 12.5);
        assert_eq!(node["processMemoryPercent"], 25.0);
        assert_eq!(node["processStartedAt"], "2026-01-01T00:00:00Z");
        assert_eq!(node["processUptimeMs"], 1000);
        assert_eq!(node["lastReportAt"], "2026-01-01T00:00:05Z");

        sqlx::query("UPDATE component_status SET state = 'disabled' WHERE node_id = 'node-public' AND component_key = 'process'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let node = node_value(&state, "node-public").await.unwrap();
        assert!(node["processCpuPercent"].is_null());
        assert!(node["processMemoryPercent"].is_null());
        assert!(node["processStartedAt"].is_null());
        assert!(node["processUptimeMs"].is_null());
        assert_eq!(node["lastReportAt"], "2026-01-01T00:00:05Z");
    }

    #[tokio::test]
    async fn public_projection_includes_every_active_node_and_is_allowlisted() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let value = networks_value(&state).await;
        let nodes = value[0]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["nodeId"], "node-private");
        assert_eq!(nodes[1]["nodeId"], "node-public");
        let node = &nodes[1];
        sqlx::query("INSERT INTO peer_aggregate_5m (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)")
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
        sqlx::query("INSERT INTO peer_aggregate_5m_countries (node_id, bucket_start, country_code, peer_count) VALUES ($1, $2, $3, $4)")
            .bind("node-public")
            .bind("2026-08-12T10:05:00Z")
            .bind("US")
            .bind(7_i64)
            .execute(state.db().pool())
            .await
            .unwrap();
        let peer_history_value = peer_history_value(&state, "node-public").await.unwrap();
        assert_eq!(peer_history_value["state"], "ok");
        assert_eq!(peer_history_value["freshness"], "stale");
        assert_eq!(
            peer_history_value["fiveMinute"][0]["countries"][0]["countryCode"],
            "US"
        );
        assert!(peer_history_value["hourly"].is_array());
        assert!(peer_history_value["fiveMinute"][0].get("peerId").is_none());
        let peer_history_text = peer_history_value.to_string();
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

        assert!(node_value(&state, "node-private").await.is_some());
        assert!(node_value(&state, "node-retired").await.is_none());
    }

    #[tokio::test]
    async fn public_validator_projection_maps_link_columns_by_name() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-public-test', 'mainnet', 'validator-node', 'Public Validator', $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, created_at, updated_at) VALUES ('link-public-test', 'node-public', 'validator-public-test', 'primary', 'automatic', '2026-01-01T00:00:00Z', $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, provider_timestamp, last_attempt_received_at, last_good_received_at, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, updated_at) VALUES ('validator-public-test', 'fixture', 'success', $1, $2, $3, 1, '100', '2', '0.1', 3, 4, 5, $4)")
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();

        let value = networks_value(&state).await;
        let validator = &value[0]["validators"][0];
        assert_eq!(validator["nodeId"], "node-public");
        // A pre-activity success row is never fabricated into a canonical label
        // nor into a Current Validator Status verdict.
        assert_eq!(validator["activity"], "unknown");
        assert_eq!(validator["activityState"], "unknown");
        assert_eq!(validator["currentValidatorStatus"], "unknown");
        assert_eq!(validator["currentValidatorStatusState"], "unknown");
        // Cumulative Validator block count and its counter state reach the
        // Public projection unchanged, with a real last-success time.
        assert_eq!(validator["blockCount"], 5);
        assert_eq!(validator["counterState"], "normal");
        assert_eq!(validator["state"], "fresh");
        assert_eq!(validator["freshness"], "fresh");
        assert_eq!(validator["source"], "fixture");
        assert!(validator["receivedAt"].as_str().is_some());
    }

    #[tokio::test]
    async fn public_validator_rank_is_distinguishable_and_independent_of_detail() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        let stale =
            crate::auth::format_rfc3339(crate::auth::now_utc() - time::Duration::minutes(10));
        seed_validator_activity(
            &state,
            "node-public",
            "validator-rank",
            "success",
            Some("producing"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;

        async fn first_validator(state: &AppState) -> serde_json::Value {
            networks_value(state).await[0]["validators"][0].clone()
        }

        // A complete list that contains the Validator is ranked, with its own
        // fetch time and cohort size separate from the detail freshness.
        sqlx::query("UPDATE current_validator_insights SET rank = 4, rank_outcome = 'success', rank_last_good_received_at = $1, rank_cohort_size = 300 WHERE validator_id = 'validator-rank'")
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        let validator = first_validator(&state).await;
        assert_eq!(validator["rank"], 4);
        assert_eq!(validator["rankState"], "ranked");
        assert_eq!(validator["rankFreshness"], "fresh");
        assert_eq!(validator["rankCohortSize"], 300);
        assert!(validator["rankReceivedAt"].as_str().is_some());

        // A complete list that omits the Validator is authoritative unranked,
        // never zero and never an outage.
        sqlx::query("UPDATE current_validator_insights SET rank = NULL WHERE validator_id = 'validator-rank'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let validator = first_validator(&state).await;
        assert!(validator["rank"].is_null());
        assert_eq!(validator["rankState"], "unranked");

        // A failed list retains the last-good rank and marks it stale; it is
        // never reported as unranked.
        sqlx::query("UPDATE current_validator_insights SET rank = 4, rank_outcome = 'error', rank_diagnostic = 'ranking down', rank_last_good_received_at = $1 WHERE validator_id = 'validator-rank'")
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        let validator = first_validator(&state).await;
        assert_eq!(validator["rank"], 4);
        assert_eq!(validator["rankState"], "error");
        assert_eq!(validator["rankFreshness"], "stale");

        // A failure without any last-good rank is unknown, not unranked.
        sqlx::query("UPDATE current_validator_insights SET rank = NULL, rank_outcome = 'error', rank_last_good_received_at = NULL WHERE validator_id = 'validator-rank'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let validator = first_validator(&state).await;
        assert!(validator["rank"].is_null());
        assert_eq!(validator["rankState"], "error");
        assert_eq!(validator["rankFreshness"], "unknown");

        // A pre-ranking row stays Unknown rather than a fabricated unranked.
        sqlx::query("UPDATE current_validator_insights SET rank = NULL, rank_outcome = NULL, rank_last_good_received_at = NULL, rank_cohort_size = NULL WHERE validator_id = 'validator-rank'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let validator = first_validator(&state).await;
        assert_eq!(validator["rankState"], "unknown");
        assert_eq!(validator["rankFreshness"], "unknown");

        // Ranking freshness is independent from detail freshness: the detail
        // observation stays fresh while an old ranking fetch is stale.
        sqlx::query("UPDATE current_validator_insights SET rank = 7, rank_outcome = 'success', rank_last_good_received_at = $1, rank_cohort_size = 120 WHERE validator_id = 'validator-rank'")
            .bind(&stale)
            .execute(state.db().pool())
            .await
            .unwrap();
        let validator = first_validator(&state).await;
        assert_eq!(validator["state"], "fresh");
        assert_eq!(validator["rankState"], "ranked");
        assert_eq!(validator["rankFreshness"], "stale");
        assert_eq!(validator["rank"], 7);
    }
    #[tokio::test]
    async fn public_validator_reward_amount_is_exact_gross_and_retained_after_failure() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        seed_validator_activity(
            &state,
            "node-public",
            "validator-reward",
            "success",
            Some("producing"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        // A 12-decimal native-unit value reaches the Public projection with
        // every source digit intact; it is never parsed into binary floating
        // point. A source-reported zero is a real value.
        sqlx::query("UPDATE current_validator_insights SET reward_amount = '1234567.890123456789' WHERE validator_id = 'validator-reward'")
            .execute(state.db().pool())
            .await
            .unwrap();

        let value = networks_value(&state).await;
        let validator = &value[0]["validators"][0];
        assert_eq!(validator["rewardAmount"], "1234567.890123456789");
        assert_eq!(validator["state"], "fresh");

        // A later source failure retains the exact last-good gross reward and
        // never rewrites it as zero.
        sqlx::query("UPDATE current_validator_insights SET outcome = 'error' WHERE validator_id = 'validator-reward'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = networks_value(&state).await;
        let validator = &value[0]["validators"][0];
        assert_eq!(validator["state"], "error");
        assert_eq!(validator["rewardAmount"], "1234567.890123456789");

        // Server freshness expiry marks the retained reward stale, never
        // unknown and never zero.
        sqlx::query("UPDATE current_validator_insights SET outcome = 'success', last_good_received_at = $1 WHERE validator_id = 'validator-reward'")
            .bind(crate::auth::format_rfc3339(
                crate::auth::now_utc() - time::Duration::minutes(10),
            ))
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = networks_value(&state).await;
        let validator = &value[0]["validators"][0];
        assert_eq!(validator["state"], "stale");
        assert_eq!(validator["rewardAmount"], "1234567.890123456789");
    }

    #[tokio::test]
    async fn public_validator_reward_follows_the_effective_link_without_splicing() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        seed_validator_activity(
            &state,
            "node-public",
            "validator-a",
            "success",
            Some("producing"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        sqlx::query("UPDATE current_validator_insights SET reward_amount = '111.111111111111' WHERE validator_id = 'validator-a'")
            .execute(state.db().pool())
            .await
            .unwrap();
        // Validator B is registered with its own history but has no effective
        // link yet.
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-b', 'mainnet', '0xvalidator-b', 'Validator B', $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, last_attempt_received_at, last_good_received_at, reward_amount, counter_state, change_state, candidate_observations, updated_at) VALUES ('validator-b', 'fixture', 'success', $1, $2, '222.222222222222', 'normal', 'normal', 0, $3)")
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();

        let value = networks_value(&state).await;
        assert_eq!(
            value[0]["validators"][0]["validatorNodeId"],
            "0xvalidator-a"
        );
        assert_eq!(
            value[0]["validators"][0]["rewardAmount"],
            "111.111111111111"
        );

        // The effective Link moves from A to B: the Node presents B's full
        // cumulative reward, never A+B and never a spliced history.
        sqlx::query("UPDATE node_validator_links SET valid_until = '2026-02-01T00:00:00Z' WHERE validator_id = 'validator-a'")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, created_at, updated_at) VALUES ('link-validator-b', 'node-public', 'validator-b', 'primary', 'automatic', '2026-02-01T00:00:00Z', $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();

        let value = networks_value(&state).await;
        let validators = value[0]["validators"].as_array().unwrap();
        assert_eq!(
            validators.len(),
            1,
            "only the newly linked Validator is effective: {validators:?}"
        );
        assert_eq!(validators[0]["validatorNodeId"], "0xvalidator-b");
        assert_eq!(validators[0]["rewardAmount"], "222.222222222222");
    }

    #[tokio::test]
    async fn public_validator_block_rate_uses_one_observation_and_labels_zero_denominator() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        seed_validator_activity(
            &state,
            "node-public",
            "validator-rate",
            "success",
            Some("producing"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        sqlx::query("UPDATE current_validator_insights SET block_count = 100, expected_block_count = 110, gen_blocks_rate = '75.5' WHERE validator_id = 'validator-rate'")
            .execute(state.db().pool())
            .await
            .unwrap();

        let value = networks_value(&state).await;
        let validator = &value[0]["validators"][0];
        assert_eq!(validator["expectedBlockCount"], 110);
        assert_eq!(validator["blockRate"], "90.909091");
        assert_eq!(validator["blockRateState"], "ok");
        assert_eq!(validator["genBlocksRate"], "75.5");

        // A known zero scheduled-block denominator is Not applicable, never a
        // 0% or 100% rate, and the source 24h rate stays independent.
        sqlx::query("UPDATE current_validator_insights SET block_count = 0, expected_block_count = 0 WHERE validator_id = 'validator-rate'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = networks_value(&state).await;
        let validator = &value[0]["validators"][0];
        assert!(validator["blockRate"].is_null());
        assert_eq!(validator["blockRateState"], "not_applicable");
        assert_eq!(validator["genBlocksRate"], "75.5");

        // A fresh numerator without a denominator is Unknown: the Server never
        // divides it by the previous observation's denominator.
        sqlx::query("UPDATE current_validator_insights SET block_count = 200, expected_block_count = NULL WHERE validator_id = 'validator-rate'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = networks_value(&state).await;
        let validator = &value[0]["validators"][0];
        assert!(validator["blockRate"].is_null());
        assert_eq!(validator["blockRateState"], "unknown");
    }

    #[tokio::test]
    async fn public_validator_gen_blocks_rate_keeps_source_zero_and_last_good_on_failure() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        seed_validator_activity(
            &state,
            "node-public",
            "validator-gen-rate",
            "success",
            Some("producing"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;

        // The investigated source emits an explicit "0%" for absent evidence
        // or an upstream error; a successful field is reproduced as 0.
        sqlx::query("UPDATE current_validator_insights SET gen_blocks_rate = '0' WHERE validator_id = 'validator-gen-rate'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = networks_value(&state).await;
        assert_eq!(value[0]["validators"][0]["genBlocksRate"], "0");

        // A local failure retains the last-good source value and marks the
        // overall state errored; it never rewrites the value as unknown.
        sqlx::query("UPDATE current_validator_insights SET outcome = 'error' WHERE validator_id = 'validator-gen-rate'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = networks_value(&state).await;
        let validator = &value[0]["validators"][0];
        assert_eq!(validator["state"], "error");
        assert_eq!(validator["genBlocksRate"], "0");

        // A successful response that omits the field is Unknown, never a
        // synthesized 0.
        sqlx::query("UPDATE current_validator_insights SET outcome = 'success', gen_blocks_rate = NULL WHERE validator_id = 'validator-gen-rate'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = networks_value(&state).await;
        assert!(value[0]["validators"][0]["genBlocksRate"].is_null());
    }

    #[tokio::test]
    async fn public_validator_delegation_reward_percentage_is_percentage_points_and_retained() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        seed_validator_activity(
            &state,
            "node-public",
            "validator-delegation",
            "success",
            Some("producing"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;

        // The source value is already a delegation distribution percentage in
        // percentage points: 20 renders as 20, not 0.2 or 2000.
        sqlx::query("UPDATE current_validator_insights SET delegation_reward_percentage = '20' WHERE validator_id = 'validator-delegation'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = networks_value(&state).await;
        assert_eq!(
            value[0]["validators"][0]["delegationRewardPercentage"],
            "20"
        );

        // Legitimate 0 and 100 boundaries are preserved exactly.
        for boundary in ["0", "100"] {
            sqlx::query("UPDATE current_validator_insights SET delegation_reward_percentage = $1 WHERE validator_id = 'validator-delegation'")
                .bind(boundary)
                .execute(state.db().pool())
                .await
                .unwrap();
            let value = networks_value(&state).await;
            assert_eq!(
                value[0]["validators"][0]["delegationRewardPercentage"],
                boundary
            );
        }

        // A source failure retains the last-good percentage; it is never
        // rewritten as unknown or zero.
        sqlx::query("UPDATE current_validator_insights SET outcome = 'error' WHERE validator_id = 'validator-delegation'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = networks_value(&state).await;
        let validator = &value[0]["validators"][0];
        assert_eq!(validator["state"], "error");
        assert_eq!(validator["delegationRewardPercentage"], "100");

        // A successful response that omits the field is unknown, never a
        // fabricated zero.
        sqlx::query("UPDATE current_validator_insights SET outcome = 'success', delegation_reward_percentage = NULL WHERE validator_id = 'validator-delegation'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = networks_value(&state).await;
        assert!(value[0]["validators"][0]["delegationRewardPercentage"].is_null());
    }

    #[tokio::test]
    async fn public_validator_legacy_row_reports_unknown_rates_without_backfill() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        seed_validator_activity(
            &state,
            "node-public",
            "validator-legacy",
            "success",
            Some("producing"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;

        let value = networks_value(&state).await;
        let validator = &value[0]["validators"][0];
        // Migration 0045 adds the columns without values: historical rows stay
        // Unknown rather than acquiring a fabricated rate.
        assert!(validator["expectedBlockCount"].is_null());
        assert!(validator["blockRate"].is_null());
        assert_eq!(validator["blockRateState"], "unknown");
        assert!(validator["genBlocksRate"].is_null());
        assert!(validator["delegationRewardPercentage"].is_null());
    }

    #[tokio::test]
    async fn public_validator_not_configured_is_explicit_and_retains_last_good_block_count() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        seed_validator_activity(
            &state,
            "node-public",
            "validator-unconfigured",
            "not_configured",
            Some("producing"),
            Some(&now),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;
        sqlx::query("UPDATE current_validator_insights SET block_count = 42, provider_timestamp = '2025-12-31T00:00:00Z' WHERE validator_id = 'validator-unconfigured'")
            .execute(state.db().pool())
            .await
            .unwrap();

        let value = networks_value(&state).await;
        let validator = &value[0]["validators"][0];
        assert_eq!(validator["state"], "not_configured");
        // A retained last-good block count survives the unconfigured source;
        // the unconfigured source never fabricates or projects Activity.
        assert_eq!(validator["blockCount"], 42);
        assert_eq!(validator["providerTimestamp"], "2025-12-31T00:00:00Z");
        assert_eq!(validator["activity"], "unknown");
        assert_eq!(validator["activityState"], "unknown");
        assert_eq!(validator["freshness"], "fresh");
    }

    #[tokio::test]
    async fn public_validator_freshness_follows_configured_refresh_interval() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let five_minutes_ago =
            crate::auth::format_rfc3339(crate::auth::now_utc() - time::Duration::minutes(5));
        seed_validator_activity(
            &state,
            "node-public",
            "validator-slow-refresh",
            "success",
            Some("producing"),
            Some(&five_minutes_ago),
            "2026-01-01T00:00:00Z",
            None,
        )
        .await;

        // The default window (two 60s intervals) marks this last-good value
        // stale...
        let default_state = state.clone();
        let value = networks_value(&default_state).await;
        assert_eq!(value[0]["validators"][0]["state"], "stale");

        // ... while a slower configured refresh keeps the same age fresh.
        let slow = state.with_validator_freshness_seconds(600);
        let value = networks_value(&slow).await;
        assert_eq!(value[0]["validators"][0]["freshness"], "fresh");
        assert_eq!(value[0]["validators"][0]["state"], "fresh");
    }

    async fn seed_public_activity_node(state: &AppState, node_id: &str) {
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, 'agent-public-test', 'mainnet', $2, 'ws://127.0.0.1:1', 'active', 'public', 1, $3, $4)")
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
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ($1, 'mainnet', $2, $3, $4, $5)")
            .bind(validator_id)
            .bind(format!("0x{validator_id}"))
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at) VALUES ($1, $2, $3, 'primary', 'automatic', $4, $5, $6, $7)")
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
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key, last_observation_key, updated_at) VALUES ($1, 'fixture', $2, NULL, NULL, $3, $4, $5, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'normal', 'normal', NULL, NULL, 0, NULL, NULL, NULL, NULL, $6)")
            .bind(validator_id)
            .bind(outcome)
            .bind(activity)
            .bind(&now)
            .bind(last_good_received_at)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        // This fixture explicitly declares its prior confirmed evidence; an
        // arbitrary failed row must not obtain a production cache fallback.
        let (verdict, receipt) = if outcome == "empty" {
            (Some("empty"), Some(now.as_str()))
        } else if activity.is_some() && last_good_received_at.is_some() {
            (Some("success"), last_good_received_at)
        } else {
            (None, None)
        };
        sqlx::query("UPDATE current_validator_insights SET last_good_verdict_outcome = $1, last_good_verdict_received_at = $2 WHERE validator_id = $3")
            .bind(verdict).bind(receipt).bind(validator_id).execute(state.db().pool()).await.unwrap();
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
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at) VALUES ('link-validator-shared-b', 'node-shared-b', 'validator-shared', 'standby', 'automatic', '2026-01-01T00:00:00Z', NULL, $1, $2)")
            .bind(&now_link)
            .bind(&now_link)
            .execute(state.db().pool())
            .await
            .unwrap();

        // Make the Provider-failed Node fully healthy so the assertion
        // proves Provider Activity never changes Node Health.
        for component in ["rpc", "sync", "consensus", "process"] {
            sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, received_at, observation_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-error-good', 'node-error-good', $1, 'ok', $2, $2, 1, 1)")
                .bind(component)
                .bind(&now)
                .execute(state.db().pool())
                .await
                .unwrap();
        }

        let value = networks_value(&state).await;
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

        // A current successful snapshot shows the canonical label as Current
        // and establishes a currently valid staking identity.
        let current = find_node("node-current");
        assert_eq!(current["validator"]["activity"], "producing");
        assert_eq!(current["validator"]["activityState"], "current");
        assert_eq!(current["validator"]["currentValidatorStatus"], "validator");
        assert_eq!(
            current["validator"]["currentValidatorStatusState"],
            "current"
        );
        assert!(current["validator"]["currentValidatorStatusQualifier"].is_null());

        // A successful snapshot outside the freshness window is Stale.
        let stale = find_node("node-stale");
        assert_eq!(stale["validator"]["activity"], "active");
        assert_eq!(stale["validator"]["activityState"], "stale");

        // The strictly validated empty form is authoritative absence: Activity
        // stays Observing and Current Validator Status is Not Validator.
        let observing = find_node("node-observing");
        assert_eq!(observing["validator"]["activity"], "observing");
        assert_eq!(observing["validator"]["activityState"], "current");
        assert_eq!(
            observing["validator"]["currentValidatorStatus"],
            "not_validator"
        );
        // HTTP 404 is a routing/deployment anomaly, not proof of absence
        // (#168): it never becomes a negative verdict.
        let not_found = find_node("node-not-found");
        assert_eq!(not_found["validator"]["activity"], "unknown");
        assert_eq!(not_found["validator"]["currentValidatorStatus"], "unknown");

        // Provider Error with a last-good Activity keeps the label as Stale,
        // even when the last-good timestamp is still in the freshness window,
        // and Node Health stays healthy and independent.
        let errored = find_node("node-error-good");
        assert_eq!(errored["validator"]["activity"], "locked");
        assert_eq!(errored["validator"]["activityState"], "stale");
        // A retained locked last-good verdict stays a valid identity with the
        // explicit locked qualifier, marked stale, and never changes Node
        // health or Server readiness.
        assert_eq!(errored["validator"]["currentValidatorStatus"], "validator");
        assert_eq!(errored["validator"]["currentValidatorStatusState"], "stale");
        assert_eq!(
            errored["validator"]["currentValidatorStatusQualifier"],
            "locked"
        );
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

        // One Validator automatically linked to two Nodes exposes the same
        // identity on both; no manual role is projected (#173).
        for node_id in ["node-shared-a", "node-shared-b"] {
            let shared = find_node(node_id);
            assert_eq!(
                shared["validator"]["validatorId"], "validator-shared",
                "{node_id}"
            );
            assert_eq!(shared["validator"]["activity"], "producing", "{node_id}");
            assert_eq!(shared["validator"]["activityState"], "current", "{node_id}");
            assert_eq!(
                shared["validator"]["currentValidatorStatus"], "validator",
                "{node_id}"
            );
            assert!(shared["validator"].get("linkRole").is_none(), "{node_id}");
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

    /// Seed one eligible Validator with its explicit Link and (optionally) a
    /// current detail insight. A `None` received time leaves the Validator
    /// registered and linked with no insight row, which is exactly the
    /// never-observed state coverage must count rather than skip.
    #[allow(clippy::too_many_arguments)]
    async fn seed_summary_validator(
        state: &AppState,
        network_key: &str,
        node_id: &str,
        validator_id: &str,
        role: &str,
        block_count: Option<i64>,
        reward_amount: Option<&str>,
        received_at: Option<&str>,
    ) {
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(validator_id)
            .bind(network_key)
            .bind(format!("0x{validator_id}"))
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at) VALUES ($1, $2, $3, $4, 'automatic', '2026-01-01T00:00:00Z', NULL, $5, $6)")
            .bind(format!("link-{validator_id}"))
            .bind(node_id)
            .bind(validator_id)
            .bind(role)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        if let Some(received_at) = received_at {
            sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key, last_observation_key, updated_at) VALUES ($1, 'fixture', 'success', NULL, NULL, 'producing', $2, $3, NULL, NULL, $4, NULL, NULL, NULL, $5, 'normal', 'normal', NULL, NULL, 0, NULL, NULL, NULL, NULL, $6)")
                .bind(validator_id)
                .bind(&now)
                .bind(received_at)
                .bind(reward_amount)
                .bind(block_count)
                .bind(&now)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
    }

    async fn public_summary(state: &AppState, network_key: &str) -> serde_json::Value {
        let value = networks_value(state).await;
        value
            .as_array()
            .unwrap()
            .iter()
            .find(|network| network["networkKey"] == network_key)
            .cloned()
            .unwrap_or_else(|| panic!("Network {network_key} is in the Public list"))
    }

    #[tokio::test]
    async fn public_validator_summary_deduplicates_and_separates_networks_with_independent_coverage()
     {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        let stale =
            crate::auth::format_rfc3339(crate::auth::now_utc() - time::Duration::minutes(10));
        for node_id in [
            "node-standby",
            "node-offline",
            "node-partial",
            "node-gap",
            "node-unlinked",
        ] {
            seed_public_activity_node(&state, node_id).await;
        }
        // A second Network proves unrelated chains and units never combine.
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('testnet', 'Test Network', '0xtestnet', 2, 2, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('node-testnet', 'agent-public-test', 'testnet', 'Testnet node', 'ws://127.0.0.1:1', 'active', 'public', 1, $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();

        // One Validator shared by a primary and a standby Node: it must be
        // counted once with the role ignored.
        seed_summary_validator(
            &state,
            "mainnet",
            "node-public",
            "validator-shared",
            "primary",
            Some(100),
            Some("10.5"),
            Some(&now),
        )
        .await;
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at) VALUES ('link-validator-shared-standby', 'node-standby', 'validator-shared', 'standby', 'automatic', '2026-01-01T00:00:00Z', NULL, $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        // A stale last-good value still contributes, marked stale.
        seed_summary_validator(
            &state,
            "mainnet",
            "node-private",
            "validator-stale",
            "observer",
            Some(5),
            Some("0.25"),
            Some(&stale),
        )
        .await;
        // A temporarily offline Active Node stays in scope.
        seed_summary_validator(
            &state,
            "mainnet",
            "node-offline",
            "validator-offline",
            "primary",
            Some(1000),
            Some("1.000000000001"),
            Some(&now),
        )
        .await;
        // A block count without a reward makes the two coverages differ.
        seed_summary_validator(
            &state,
            "mainnet",
            "node-gap",
            "validator-gap",
            "primary",
            Some(3),
            None,
            Some(&now),
        )
        .await;
        // Never observed: eligible but with no value, so not a zero.
        seed_summary_validator(
            &state,
            "mainnet",
            "node-partial",
            "validator-partial",
            "primary",
            None,
            None,
            None,
        )
        .await;
        // Linked only to a Retired Node: never eligible.
        seed_summary_validator(
            &state,
            "mainnet",
            "node-retired",
            "validator-retired",
            "primary",
            Some(999),
            Some("9"),
            Some(&now),
        )
        .await;
        seed_summary_validator(
            &state,
            "testnet",
            "node-testnet",
            "validator-testnet",
            "primary",
            Some(7),
            Some("2"),
            Some(&now),
        )
        .await;
        // The same on-chain Validator identifier registered on another Network
        // is a distinct identity there and must never share this Network's
        // totals.
        sqlx::query("UPDATE validators SET validator_node_id = (SELECT validator_node_id FROM validators WHERE validator_id = 'validator-shared') WHERE validator_id = 'validator-testnet'")
            .execute(state.db().pool())
            .await
            .unwrap();
        // A Network whose only eligible Validator has no value at all shows
        // Unknown, never a fabricated zero.
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('quietnet', 'Quiet Network', '0xquietnet', 3, 3, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('node-quiet', 'agent-public-test', 'quietnet', 'Quiet node', 'ws://127.0.0.1:1', 'active', 'public', 1, $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        seed_summary_validator(
            &state,
            "quietnet",
            "node-quiet",
            "validator-quiet",
            "primary",
            None,
            None,
            None,
        )
        .await;

        let mainnet = public_summary(&state, "mainnet").await;
        let summary = &mainnet["validatorSummary"];
        assert_eq!(mainnet["validators"].as_array().unwrap().len(), 5);
        assert_eq!(summary["eligibleValidatorCount"], 5);
        assert_eq!(summary["linkedNodeCount"], 6);
        assert_eq!(summary["unlinkedNodeCount"], 1);
        // Deduplicated: 100 + 5 + 1000 + 3, not counting the shared Validator
        // twice and excluding the Retired-only Validator.
        assert_eq!(summary["blocks"]["knownSum"], "1108");
        assert_eq!(summary["blocks"]["expectedCount"], 5);
        assert_eq!(summary["blocks"]["valuedCount"], 4);
        assert_eq!(summary["blocks"]["staleCount"], 1);
        assert_eq!(summary["blocks"]["state"], "partial");
        // Exact decimal sum including the widest source precision.
        assert_eq!(summary["rewards"]["knownSum"], "11.750000000001");
        assert_eq!(summary["rewards"]["expectedCount"], 5);
        assert_eq!(summary["rewards"]["valuedCount"], 3);
        assert_eq!(summary["rewards"]["staleCount"], 1);
        assert_eq!(summary["rewards"]["state"], "partial");

        let testnet = public_summary(&state, "testnet").await;
        // The same on-chain identifier appears in both Networks, but each
        // Network keeps its own identity and total.
        assert_eq!(
            testnet["validators"][0]["validatorNodeId"],
            "0xvalidator-shared"
        );
        assert_eq!(testnet["validators"][0]["validatorId"], "validator-testnet");
        assert_eq!(testnet["validatorSummary"]["blocks"]["knownSum"], "7");
        assert_eq!(testnet["validatorSummary"]["rewards"]["knownSum"], "2");
        assert_eq!(testnet["validatorSummary"]["blocks"]["state"], "complete");
        assert_eq!(testnet["validatorSummary"]["unlinkedNodeCount"], 0);

        let quiet = public_summary(&state, "quietnet").await;
        assert_eq!(
            quiet["validatorSummary"]["blocks"]["knownSum"],
            serde_json::Value::Null
        );
        assert_eq!(
            quiet["validatorSummary"]["rewards"]["knownSum"],
            serde_json::Value::Null
        );
        assert_eq!(quiet["validatorSummary"]["blocks"]["state"], "unknown");
        assert_eq!(quiet["validatorSummary"]["rewards"]["valuedCount"], 0);
        assert_eq!(quiet["validatorSummary"]["eligibleValidatorCount"], 1);
    }

    #[tokio::test]
    async fn public_validator_summary_keeps_block_totals_exact_beyond_integer_boundaries() {
        for (left, right, expected) in [
            (i64::MAX, 1, "9223372036854775808"),
            (i64::MAX, i64::MAX, "18446744073709551614"),
            (9_007_199_254_740_992, 1, "9007199254740993"),
            (0, 0, "0"),
        ] {
            let (_dir, state) = test_state().await;
            seed_public_data(&state).await;
            let now = crate::auth::format_rfc3339(crate::auth::now_utc());
            for (node_id, validator_id, count) in [
                ("node-public", "validator-left", left),
                ("node-private", "validator-right", right),
            ] {
                seed_summary_validator(
                    &state,
                    "mainnet",
                    node_id,
                    validator_id,
                    "primary",
                    Some(count),
                    None,
                    Some(&now),
                )
                .await;
            }

            // Read the serialized Public API, not the accumulator: JSON numbers
            // would still lose precision in browser consumers even after fixing
            // the Server's i64 overflow. Individual counts retain their contract.
            let network = public_summary(&state, "mainnet").await;
            let summary = &network["validatorSummary"];
            assert_eq!(summary["blocks"]["knownSum"].as_str(), Some(expected));
            assert_eq!(summary["blocks"]["expectedCount"], 2);
            assert_eq!(summary["blocks"]["valuedCount"], 2);
            assert_eq!(summary["blocks"]["staleCount"], 0);
            assert_eq!(summary["blocks"]["state"], "complete");
            let validators = network["validators"].as_array().unwrap();
            let validator = validators
                .iter()
                .find(|validator| validator["validatorId"] == "validator-left")
                .unwrap();
            assert_eq!(validator["blockCount"].as_i64(), Some(left));
            assert_eq!(summary["rewards"]["knownSum"], serde_json::Value::Null);
            assert_eq!(summary["rewards"]["state"], "unknown");
        }
    }

    #[tokio::test]
    async fn public_validator_summary_uses_the_effective_validator_after_a_link_switch() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        seed_summary_validator(
            &state,
            "mainnet",
            "node-public",
            "validator-a",
            "primary",
            Some(100),
            Some("10"),
            Some(&now),
        )
        .await;
        let before = public_summary(&state, "mainnet").await;
        assert_eq!(before["validatorSummary"]["blocks"]["knownSum"], "100");
        assert_eq!(before["validatorSummary"]["rewards"]["knownSum"], "10");

        // The effective Link moves A -> B: the summary presents B's full
        // cumulative value, never A + B and never a negative asset flow.
        sqlx::query("UPDATE node_validator_links SET valid_until = '2026-02-01T00:00:00Z' WHERE validator_id = 'validator-a'")
            .execute(state.db().pool())
            .await
            .unwrap();
        // B's Link starts exactly when A's ends, so the same Node never has
        // two overlapping effective Links.
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-b', 'mainnet', '0xvalidator-b', 'validator-b', $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at) VALUES ('link-validator-b', 'node-public', 'validator-b', 'primary', 'automatic', '2026-02-01T00:00:00Z', NULL, $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, last_attempt_received_at, last_good_received_at, reward_amount, block_count, updated_at) VALUES ('validator-b', 'fixture', 'success', $1, $2, '2', 5, $3)")
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        let after = public_summary(&state, "mainnet").await;
        assert_eq!(after["validators"].as_array().unwrap().len(), 1);
        assert_eq!(after["validatorSummary"]["eligibleValidatorCount"], 1);
        assert_eq!(after["validatorSummary"]["blocks"]["knownSum"], "5");
        assert_eq!(after["validatorSummary"]["blocks"]["state"], "complete");
        assert_eq!(after["validatorSummary"]["rewards"]["knownSum"], "2");
    }

    #[tokio::test]
    async fn admin_identity_and_public_detail_read_the_same_effective_link() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        seed_summary_validator(
            &state,
            "mainnet",
            "node-public",
            "validator-a",
            "primary",
            Some(100),
            Some("10"),
            Some(&now),
        )
        .await;
        // A's interval ends exactly where B's begins: on the half-open
        // interval only B is effective now.
        sqlx::query("UPDATE node_validator_links SET valid_until = '2026-02-01T00:00:00Z' WHERE validator_id = 'validator-a'")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-b', 'mainnet', '0xvalidator-b', 'validator-b', $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at) VALUES ('link-validator-b', 'node-public', 'validator-b', 'primary', 'automatic', '2026-02-01T00:00:00Z', NULL, $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();

        let admin =
            crate::validator::list_node_validator_identities(state.db(), Some("node-public"))
                .await
                .unwrap();
        assert_eq!(admin.len(), 1);
        assert_eq!(admin[0].validator_id.as_deref(), Some("validator-b"));
        assert_eq!(
            admin[0].validator_node_key.as_deref(),
            Some("0xvalidator-b")
        );
        let detail = public_node_detail_value(&state, "node-public").await;
        assert_eq!(detail["validator"]["validatorId"], "validator-b");
        assert_eq!(detail["validator"]["nodeId"], "node-public");
    }

    /// A Validator or Link read failure is a database failure: the Node is
    /// not answered without its Validator, and the list is not answered at
    /// all.
    #[tokio::test]
    async fn the_projection_is_unavailable_when_validators_or_links_are_unreadable() {
        for table in ["current_validator_insights", "node_validator_links"] {
            let (_dir, state) = test_state().await;
            seed_public_data(&state).await;
            let now = crate::auth::format_rfc3339(crate::auth::now_utc());
            seed_summary_validator(
                &state,
                "mainnet",
                "node-public",
                "validator-a",
                "primary",
                Some(100),
                Some("10"),
                Some(&now),
            )
            .await;
            let detail = public_node_detail_value(&state, "node-public").await;
            assert_eq!(detail["validator"]["validatorId"], "validator-a");

            sqlx::query(&format!("ALTER TABLE {table} RENAME TO {table}_unreadable"))
                .execute(state.db().pool())
                .await
                .unwrap();
            let projection = public_projection(&state);
            let at = crate::auth::now_utc();
            assert!(projection.node("node-public", at).await.is_err(), "{table}");
            assert!(projection.networks(at).await.is_err(), "{table}");
        }
    }

    fn instant(timestamp: &str) -> time::OffsetDateTime {
        time::OffsetDateTime::parse(timestamp, &time::format_description::well_known::Rfc3339)
            .unwrap()
    }

    /// Every entry reads one Network's Active Node at a fixed instant and
    /// answers nothing for its retired Node, so a guessed retired Node ID is
    /// indistinguishable from an unknown one.
    #[tokio::test]
    async fn every_projection_entry_answers_the_active_node_and_not_the_retired_one() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let projection = public_projection(&state);
        let at = instant("2026-01-01T00:00:30Z");
        let range = || BlockHistoryRange {
            from: None,
            to: None,
            limit: 50,
        };

        let networks = projection.networks(at).await.unwrap();
        let listed: Vec<&str> = networks[0]
            .nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect();
        assert_eq!(listed, ["node-private", "node-public"]);

        for node_id in ["node-public", "node-retired"] {
            let active = node_id == "node-public";
            assert_eq!(
                projection.node(node_id, at).await.unwrap().is_some(),
                active,
                "{node_id}"
            );
            assert_eq!(
                projection
                    .block_history(node_id, range(), at)
                    .await
                    .unwrap()
                    .is_some(),
                active,
                "{node_id}"
            );
            for window in [PublicMetricWindow::Live, PublicMetricWindow::OneHour] {
                assert_eq!(
                    projection
                        .metrics(node_id, window, at)
                        .await
                        .unwrap()
                        .is_some(),
                    active,
                    "{node_id} {window:?}"
                );
            }
            assert_eq!(
                projection
                    .peer_history(node_id, at)
                    .await
                    .unwrap()
                    .is_some(),
                active,
                "{node_id}"
            );
        }
    }

    /// Freshness is judged at the instant the caller names, not at the
    /// moment the query happens to run: the same stored Peer observation is
    /// current shortly after it and stale a day later.
    #[tokio::test]
    async fn the_projection_judges_freshness_at_the_instant_it_is_given() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, observed_at, received_at, observation_at, value_received_at, state_revision, value_revision) VALUES ('agent-public-test', 'node', 'node-public', 'node-public', 'peers', 'ok', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 1, 1)")
            .execute(state.db().pool())
            .await
            .unwrap();
        let projection = public_projection(&state);
        let peers_at = |at: time::OffsetDateTime| {
            let projection = &projection;
            async move {
                let node = projection.node("node-public", at).await.unwrap().unwrap();
                let listed = projection.networks(at).await.unwrap()[0]
                    .nodes
                    .iter()
                    .find(|node| node.node_id == "node-public")
                    .unwrap()
                    .peers
                    .freshness
                    .clone();
                assert_eq!(
                    serde_json::to_value(&node).unwrap()["peers"]["freshness"],
                    listed
                );
                listed
            }
        };
        assert_eq!(peers_at(instant("2026-01-01T00:00:05Z")).await, "current");
        assert_eq!(peers_at(instant("2026-01-02T00:00:00Z")).await, "stale");
    }

    /// The route for one Public read: what every Node route shares is how it
    /// turns the projection's answer into a status.
    async fn node_route(state: &AppState, route: &str, node_id: &str) -> Response {
        let request_id = || Extension(RequestId(std::sync::Arc::from("route-status")));
        let state = State(state.clone());
        let path = Path(node_id.to_owned());
        let range = || {
            Query(PublicBlockHistoryQuery {
                limit: None,
                from: None,
                to: None,
            })
        };
        match route {
            "detail" => public_node_detail(state, path, request_id()).await,
            "history" => public_node_history(state, path, range(), request_id()).await,
            "export" => public_node_history_export(state, path, range(), request_id()).await,
            "metrics" => {
                let window = Ok(Query(PublicNodeMetricsQuery::default()));
                public_node_metrics(state, path, window, request_id()).await
            }
            "peer-history" => public_node_peer_history(state, path, request_id()).await,
            other => unreachable!("no Public Node route {other}"),
        }
    }

    async fn error_request_id(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"]["requestId"].clone()
    }

    /// Each Public route answers the projection's three outcomes the same
    /// way: 200 with the body, 404 for a Node outside the projection and 503
    /// when the database cannot be read, both errors echoing the request's
    /// own id. Only a 200 export is offered as a download.
    #[tokio::test]
    async fn public_routes_map_the_projection_answer_to_a_status_with_the_request_id() {
        let (_dir, state) = test_state().await;
        seed_public_data(&state).await;
        let routes = ["detail", "history", "export", "metrics", "peer-history"];
        let list = || {
            public_networks(
                State(state.clone()),
                Extension(RequestId(std::sync::Arc::from("route-status"))),
            )
        };

        assert_eq!(list().await.status(), StatusCode::OK);
        for route in routes {
            let found = node_route(&state, route, "node-public").await;
            assert_eq!(found.status(), StatusCode::OK, "{route}");
            assert_eq!(
                found.headers().get(header::CONTENT_DISPOSITION).is_some(),
                route == "export",
                "{route}"
            );
            for node_id in ["node-retired", "node-unknown"] {
                let missing = node_route(&state, route, node_id).await;
                assert_eq!(missing.status(), StatusCode::NOT_FOUND, "{route} {node_id}");
                assert!(missing.headers().get(header::CONTENT_DISPOSITION).is_none());
                assert_eq!(error_request_id(missing).await, "route-status");
            }
        }

        sqlx::query("ALTER TABLE nodes RENAME TO nodes_unreadable")
            .execute(state.db().pool())
            .await
            .unwrap();
        let unavailable = list().await;
        assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error_request_id(unavailable).await, "route-status");
        for route in routes {
            let unavailable = node_route(&state, route, "node-public").await;
            assert_eq!(
                unavailable.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{route}"
            );
            assert_eq!(error_request_id(unavailable).await, "route-status");
        }
    }

    #[tokio::test]
    async fn public_node_exposes_automatic_identity_reason_and_never_uses_a_manual_link() {
        let (_dir, state) = test_state().await;
        seed_geo_network(&state, "mainnet", &["node-identified", "node-reason"]).await;
        let now = seconds_ago(0);

        // No automatic discovery has run and no Link exists: the identity
        // fields are absent rather than guessed.
        let before = public_network_list(&state).await;
        let node = before["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["nodeId"] == "node-reason")
            .unwrap();
        assert!(node["validatorIdentityState"].is_null());
        assert!(node["validatorIdentityReason"].is_null());
        assert!(node["validator"].is_null());

        // A persisted automatic-identity reason is projected verbatim; the
        // Server owns and sanitizes it.
        sqlx::query("INSERT INTO node_validator_identity_status (node_id, state, observed_node_key, updated_at) VALUES ('node-reason', 'network_identity_mismatch', NULL, $1)")
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        let after = public_network_list(&state).await;
        let node = after["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["nodeId"] == "node-reason")
            .unwrap();
        assert_eq!(node["validatorIdentityState"], "network_identity_mismatch");
        assert!(
            node["validatorIdentityReason"]
                .as_str()
                .unwrap()
                .contains("registered Network")
        );

        // A legacy manual Link is never projected as the automatic
        // correspondence, even when it is the only link on the Node (#173).
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('manual-validator', 'mainnet', '0xmanual', NULL, $1, $2)")
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, created_at, updated_at) VALUES ('manual-link', 'node-reason', 'manual-validator', 'primary', 'manual', $1, $2, $3)")
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        let manual = public_network_list(&state).await;
        let node = manual["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["nodeId"] == "node-reason")
            .unwrap();
        assert!(node["validator"].is_null());
        assert!(manual["validators"].as_array().unwrap().is_empty());
    }
}
