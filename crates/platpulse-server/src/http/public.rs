//! `/api/public/v1` route group — the Home Public Projection and human
//! session lifecycle (design §12.3/§12.4, §13.1).
//!
//! Middleware and DTO namespace are independent from Admin and Agent: DTOs
//! live in this module and are never reused as Admin DTOs by runtime field
//! filtering (design §13.1). Guest access is disabled by default, so every
//! route except `POST /login` requires a valid human Session; the session
//! guard itself lives in `super` and is attached in `build_app`.

use axum::extract::{Extension, Request, State};
use axum::http::StatusCode;
use axum::http::header::{self, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
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
        .fallback(api_not_found)
        .layer(axum::middleware::from_fn(group_middleware))
}
