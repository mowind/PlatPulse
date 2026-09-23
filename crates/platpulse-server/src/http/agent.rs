//! `/api/agent/v1` route group — enrollment, recovery, and report ingestion.
//!
//! Middleware and DTO namespace are independent from Public and Admin, and
//! the browser client is never generated from Agent wire DTOs (design
//! §13.4). Phase 1 adds `POST /enroll`: an Enrollment Token (Bearer) is
//! exchanged once for a stable Agent identity, Agent Epoch, and a fresh
//! Agent Credential (design §4.5, §12.5). Every other Agent route requires
//! an Agent Credential; the guard in `super` enforces that boundary, so a
//! Human Session can never enroll and an Enrollment Token can never submit
//! reports or reach human-facing APIs.

use axum::extract::{Extension, Request, State};
use axum::http::StatusCode;
use axum::http::header::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

pub const CLOCK_UNRELIABLE_THRESHOLD_MS: i64 = 5 * 60 * 1000;
pub const AGENT_OFFLINE_AFTER_SECONDS: i64 = 120;

/// Server-authoritative time exchange. The response timestamp is generated
/// by the Server, never derived from Agent wall-clock input.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ServerTimeResponse {
    pub server_time: String,
}

#[utoipa::path(
    get,
    path = "/api/agent/v1/time",
    tag = "agent",
    responses((status = 200, body = ServerTimeResponse))
)]
pub(crate) async fn server_time(
    Extension(_auth): Extension<crate::enrollment::AgentAuthInfo>,
) -> impl IntoResponse {
    Json(ServerTimeResponse {
        server_time: format_rfc3339(crate::auth::now_utc()),
    })
}

use super::{AppState, ClientIp, ROUTE_GROUP_HEADER, RequestId, api_not_found};
use crate::auth::format_rfc3339;
use crate::enrollment::{AgentAuthInfo, EnrollmentError};

async fn group_middleware(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(ROUTE_GROUP_HEADER, HeaderValue::from_static("agent"));
    response
}

/// Success payload of one Agent Enrollment (Agent wire: snake_case,
/// design §9.1). `credential` is the full `pp_agent_…` token and is
/// delivered to the enrolling Agent exactly once; the Server stores only
/// its pepper-keyed digest.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct EnrollResponse {
    /// Stable Agent identity issued by the Server (UUID).
    agent_id: String,
    /// Agent Epoch advanced by this Enrollment (1 for a new Agent).
    agent_epoch: i64,
    /// Full Agent Credential token; single delivery.
    credential: String,
    /// Agent→Server protocol major the Server speaks.
    protocol_version: u64,
}

/// Success payload of one Recovery exchange (Agent wire: snake_case,
/// design §4.5). The Agent identity is preserved, the Agent Epoch
/// advances, and `credential` is a fresh `pp_agent_…` token delivered
/// exactly once.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct RecoverResponse {
    /// The existing Agent identity the Recovery Token was bound to.
    agent_id: String,
    /// Agent Epoch advanced by this Recovery exchange.
    agent_epoch: i64,
    /// Full Agent Credential token; single delivery.
    credential: String,
    /// Agent→Server protocol major the Server speaks.
    protocol_version: u64,
}

/// Exchange a single-use Enrollment Token for an Agent identity and
/// credential. The token arrives in the `Authorization: Bearer` header —
/// never in a URL or body — and the same token can never enroll twice.
#[utoipa::path(
    post,
    path = "/api/agent/v1/enroll",
    tag = "agent",
    responses(
        (status = 200, description = "Enrolled; the response carries the one-time Agent Credential", body = EnrollResponse),
        (status = 401, description = "Missing, invalid, or expired enrollment token", body = crate::http::ApiErrorBody),
        (status = 409, description = "The enrollment token was already consumed", body = crate::http::ApiErrorBody),
        (status = 429, description = "Too many enrollment attempts", body = crate::http::ApiErrorBody),
        (status = 503, description = "Server setup is incomplete or the database is unavailable", body = crate::http::ApiErrorBody),
    )
)]
pub(crate) async fn enroll_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(request_id): Extension<RequestId>,
    Extension(client): Extension<ClientIp>,
) -> Response {
    let Some(token) = super::bearer_token(&headers) else {
        return error_response(
            &request_id.0,
            StatusCode::UNAUTHORIZED,
            "enrollment_token_invalid",
            "an enrollment token is required",
        );
    };

    // Independent enrollment rate limit (design §19.4).
    let limiter_key = (client.0.as_str(), "enroll");
    if state.enroll_limiter().is_blocked(limiter_key) {
        return error_response(
            &request_id.0,
            StatusCode::TOO_MANY_REQUESTS,
            "enrollment_rate_limited",
            "too many enrollment attempts; try again later",
        );
    }

    match crate::enrollment::enroll(state.db(), &state.auth().pepper, token).await {
        Ok(enrolled) => {
            state.enroll_limiter().record_success(limiter_key);
            (
                StatusCode::OK,
                Json(EnrollResponse {
                    agent_id: enrolled.agent_id,
                    agent_epoch: enrolled.agent_epoch,
                    credential: enrolled.credential,
                    protocol_version: platpulse_core::PROTOCOL_VERSION,
                }),
            )
                .into_response()
        }
        Err(EnrollmentError::Invalid) => {
            state.enroll_limiter().record_failure(limiter_key);
            error_response(
                &request_id.0,
                StatusCode::UNAUTHORIZED,
                "enrollment_token_invalid",
                "invalid enrollment token",
            )
        }
        Err(EnrollmentError::Expired) => {
            state.enroll_limiter().record_failure(limiter_key);
            error_response(
                &request_id.0,
                StatusCode::UNAUTHORIZED,
                "enrollment_token_expired",
                "enrollment token has expired",
            )
        }
        Err(EnrollmentError::Consumed) => {
            state.enroll_limiter().record_failure(limiter_key);
            error_response(
                &request_id.0,
                StatusCode::CONFLICT,
                "enrollment_token_consumed",
                "enrollment token has already been used",
            )
        }
        Err(EnrollmentError::InvalidLifetime(_)) => error_response(
            &request_id.0,
            StatusCode::INTERNAL_SERVER_ERROR,
            "unavailable",
            "enrollment configuration error",
        ),
        Err(EnrollmentError::Pepper(_)) | Err(EnrollmentError::ServerDatabase(_)) => {
            error_response(
                &request_id.0,
                StatusCode::INTERNAL_SERVER_ERROR,
                "unavailable",
                "server secret configuration error",
            )
        }
        Err(EnrollmentError::Database(_)) => error_response(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Exchange a single-use Recovery Token for an Epoch advance and a fresh
/// credential on the SAME Agent identity (design §4.5: Recovery rotates the
/// credential and advances the Agent Epoch without creating a duplicate
/// Agent). The token arrives in the `Authorization: Bearer` header — never
/// in a URL or body — and the same token can never recover twice.
#[utoipa::path(
    post,
    path = "/api/agent/v1/recover",
    tag = "agent",
    responses(
        (status = 200, description = "Recovered; the response carries the one-time Agent Credential", body = RecoverResponse),
        (status = 401, description = "Missing, invalid, or expired recovery token", body = crate::http::ApiErrorBody),
        (status = 409, description = "The recovery token was already consumed", body = crate::http::ApiErrorBody),
        (status = 429, description = "Too many recovery attempts", body = crate::http::ApiErrorBody),
        (status = 503, description = "Server setup is incomplete or the database is unavailable", body = crate::http::ApiErrorBody),
    )
)]
pub(crate) async fn recover_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(request_id): Extension<RequestId>,
    Extension(client): Extension<ClientIp>,
) -> Response {
    let Some(token) = super::bearer_token(&headers) else {
        return error_response(
            &request_id.0,
            StatusCode::UNAUTHORIZED,
            "recovery_token_invalid",
            "a recovery token is required",
        );
    };

    // Independent recovery rate limit (design §19.4).
    let limiter_key = (client.0.as_str(), "recover");
    if state.recover_limiter().is_blocked(limiter_key) {
        return error_response(
            &request_id.0,
            StatusCode::TOO_MANY_REQUESTS,
            "recovery_rate_limited",
            "too many recovery attempts; try again later",
        );
    }

    match crate::enrollment::recover(state.db(), &state.auth().pepper, token).await {
        Ok(recovered) => {
            state.recover_limiter().record_success(limiter_key);
            (
                StatusCode::OK,
                Json(RecoverResponse {
                    agent_id: recovered.agent_id,
                    agent_epoch: recovered.agent_epoch,
                    credential: recovered.credential,
                    protocol_version: platpulse_core::PROTOCOL_VERSION,
                }),
            )
                .into_response()
        }
        Err(crate::enrollment::RecoveryError::Invalid)
        | Err(crate::enrollment::RecoveryError::AgentNotFound) => {
            state.recover_limiter().record_failure(limiter_key);
            error_response(
                &request_id.0,
                StatusCode::UNAUTHORIZED,
                "recovery_token_invalid",
                "invalid recovery token",
            )
        }
        Err(crate::enrollment::RecoveryError::Expired) => {
            state.recover_limiter().record_failure(limiter_key);
            error_response(
                &request_id.0,
                StatusCode::UNAUTHORIZED,
                "recovery_token_expired",
                "recovery token has expired",
            )
        }
        Err(crate::enrollment::RecoveryError::Consumed) => {
            state.recover_limiter().record_failure(limiter_key);
            error_response(
                &request_id.0,
                StatusCode::CONFLICT,
                "recovery_token_consumed",
                "recovery token has already been used",
            )
        }
        Err(crate::enrollment::RecoveryError::InvalidLifetime(_)) => error_response(
            &request_id.0,
            StatusCode::INTERNAL_SERVER_ERROR,
            "unavailable",
            "recovery configuration error",
        ),
        Err(crate::enrollment::RecoveryError::Database(_)) => error_response(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Server-owned v1 baseline evidence for the Agent-side upgrade preparation
/// bridge (issue #189).
///
/// Migration under [ADR 0007](docs/adr/0007-server-managed-inventory-revision.md)
/// is only safe when the v1 Agent can prove that its last successfully applied
/// Closing declaration is exactly what the Server most recently accepted. The
/// v1 Report Receipt carries no accepted revision or hash, so the Agent cannot
/// derive that fact from its own local state: this authenticated Agent route is
/// the authoritative read of the Server's accepted values.
///
/// It is a read-only diagnostic, not a new ingestion path. It never advances a
/// Boot, allocates a revision, or accepts anything, and it is not part of the
/// frozen v1 report wire contract.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PreparationBaselineResponse {
    /// The authenticated Agent identity.
    pub agent_id: String,
    /// The Agent Epoch the Server currently requires.
    pub agent_epoch: i64,
    /// The last accepted Inventory revision; 0 means none was ever accepted.
    pub accepted_inventory_revision: i64,
    /// Content hash of the last accepted Inventory; absent means none.
    pub accepted_inventory_sha256: Option<String>,
    /// Protocol major that produced the accepted declaration; absent is unknown.
    pub accepted_inventory_protocol_major: Option<i64>,
    /// Server-observed active/closed Boot identity.
    pub active_boot_id: Option<String>,
    pub active_boot_status: String,
    pub previous_boot_id: Option<String>,
    /// The Closing report whose accepted receipt closed the active Boot.
    pub close_report_id: Option<String>,
    /// The Server-recorded disposition of that Closing receipt; the Agent binds
    /// it into the migration evidence instead of trusting its own bounded
    /// Applied Receipt Record (issue #189).
    pub close_report_disposition: Option<String>,
    pub last_report_sequence: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct PreparationBaselineRow {
    agent_epoch: i64,
    last_inventory_revision: i64,
    inventory_sha256: Option<String>,
    inventory_protocol_major: Option<i64>,
    active_boot_id: Option<String>,
    active_boot_status: String,
    previous_boot_id: Option<String>,
    close_report_id: Option<String>,
    close_report_disposition: Option<String>,
    last_report_sequence: Option<i64>,
}

/// Read the Server's accepted Inventory baseline for the calling Agent.
pub(crate) async fn preparation_baseline(
    State(state): State<AppState>,
    Extension(auth): Extension<AgentAuthInfo>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let row = sqlx::query_as::<_, PreparationBaselineRow>(
        "SELECT a.agent_epoch, a.last_inventory_revision, a.inventory_sha256, a.inventory_protocol_major, a.active_boot_id, a.active_boot_status, a.previous_boot_id, a.close_report_id, a.last_report_sequence, r.disposition AS close_report_disposition FROM agents a LEFT JOIN agent_report_receipts r ON r.report_id = a.close_report_id WHERE a.agent_id = ? AND a.deleted_at IS NULL",
    )
    .bind(&auth.agent_id)
    .fetch_optional(state.db().pool())
    .await;
    match row {
        Ok(Some(row)) => (
            StatusCode::OK,
            Json(PreparationBaselineResponse {
                agent_id: auth.agent_id,
                agent_epoch: row.agent_epoch,
                accepted_inventory_revision: row.last_inventory_revision,
                accepted_inventory_sha256: row.inventory_sha256,
                accepted_inventory_protocol_major: row.inventory_protocol_major,
                active_boot_id: row.active_boot_id,
                active_boot_status: row.active_boot_status,
                previous_boot_id: row.previous_boot_id,
                close_report_id: row.close_report_id,
                close_report_disposition: row.close_report_disposition,
                last_report_sequence: row.last_report_sequence,
            }),
        )
            .into_response(),
        Ok(None) => error_response(
            &request_id.0,
            StatusCode::UNAUTHORIZED,
            "agent_auth_required",
            "Agent credential is invalid",
        ),
        Err(_) => error_response(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
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
        Json(super::ApiErrorBody::new(code, message, request_id)),
    )
        .into_response()
}

pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/time", get(server_time))
        .route("/preparation", get(preparation_baseline))
        .route("/enroll", post(enroll_handler))
        .route("/recover", post(recover_handler))
        .fallback(api_not_found)
        .layer(axum::middleware::from_fn(group_middleware))
}
