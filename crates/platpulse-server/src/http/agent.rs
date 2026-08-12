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
use axum::routing::post;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

use super::{AppState, ClientIp, ROUTE_GROUP_HEADER, RequestId, api_not_found};
use crate::enrollment::EnrollmentError;

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
        .route("/enroll", post(enroll_handler))
        .fallback(api_not_found)
        .layer(axum::middleware::from_fn(group_middleware))
}
