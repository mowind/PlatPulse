//! Owner-only access management (issue #47, design §12.1/§12.3, §13.1):
//! People and role controls, coarse Human Session review and revoke, the
//! immutable redacted Audit listing, and the anonymous Home (Guest) toggle.
//!
//! Every mutation revalidates the browser trust boundary (JSON content
//! type, exact Origin, session CSRF token), commits atomically with its
//! Audit row, and publishes an Admin invalidation so other Owner tabs
//! refetch authoritative REST. The final valid Owner is never disabled or
//! demoted; password/role/disabled mutations revoke the affected user's
//! Sessions immediately. People DTOs never carry password hashes,
//! credentials, session tokens, or raw network data; the Audit listing is
//! allowlisted and its details are redacted by construction.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{
    IdentityError, count_enabled_owners, format_rfc3339, hash_password, now_utc,
    revoke_user_sessions, validate_password, validate_username,
};
use crate::http::admin::{mutation_error, mutation_guard_ok};
use crate::http::{AppState, AuthenticatedSession, ROUTE_GROUP_HEADER, RequestId};

/// One allowlisted Person row. Passwords, hashes, and credentials never
/// leave the Server (design §12.1: People review is safe on ordinary
/// screens).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub user_id: String,
    pub username: String,
    pub role: String,
    pub disabled: bool,
    pub created_at: String,
    /// Active (non-revoked) Session count; never session material.
    pub session_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PeopleResponse {
    pub users: Vec<Person>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatePersonRequest {
    pub username: String,
    pub password: String,
    pub role: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonRoleRequest {
    pub role: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonStatusRequest {
    pub disabled: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResetPasswordRequest {
    pub password: String,
}

/// One coarse, non-sensitive Session row (design §12.3): creation, last
/// activity, expiry, and a coarse client hint — never the token, a full
/// User-Agent, or a raw IP.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionItem {
    pub session_id: String,
    pub user_id: String,
    pub username: String,
    pub role: String,
    pub client_hint: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    pub current: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionsResponse {
    pub sessions: Vec<SessionItem>,
}

/// Explicit JSON body for Session revoke mutations. The revoke carries no
/// parameters, but declaring a body keeps the browser trust boundary
/// uniform: every Admin mutation is a JSON request with the synchronizer
/// CSRF header (design §12.4), including bodyless ones.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevokeSessionRequest {}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevokeSessionResponse {
    pub session_id: String,
    pub revoked_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevokeOthersResponse {
    pub revoked_count: i64,
}

/// One immutable, redacted Audit row. `details` is the stored `after_json`
/// body, which is redacted by construction (ids, instants, and counts only
/// — never passwords, tokens, credentials, endpoints, raw peer IPs, or
/// complete request bodies).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditItem {
    pub audit_event_id: i64,
    pub event_kind: String,
    pub actor_username: Option<String>,
    pub target_kind: String,
    pub target_id: String,
    pub created_at: String,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditResponse {
    pub items: Vec<AuditItem>,
    /// Cursor for the next older page; absent when the listing is complete.
    pub next_before: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i64>,
    pub event_kind: Option<String>,
    pub target_kind: Option<String>,
    pub before: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccessSettingsResponse {
    pub guest_enabled: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccessSettingsRequest {
    pub guest_enabled: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct PersonRow {
    user_id: String,
    username: String,
    role: String,
    disabled_at: Option<String>,
    created_at: String,
    session_count: i64,
}

fn person_dto(row: PersonRow) -> Person {
    Person {
        user_id: row.user_id,
        username: row.username,
        role: row.role,
        disabled: row.disabled_at.is_some(),
        created_at: row.created_at,
        session_count: row.session_count,
    }
}

/// Owner-only People listing (design §12.1): every human principal with
/// role, disabled state, and active Session count. Passwords and
/// credentials are never projected.
#[utoipa::path(
    get,
    path = "/api/admin/v1/people",
    tag = "admin",
    responses((status = 200, body = PeopleResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn people_list(
    State(state): State<AppState>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let rows = sqlx::query_as::<_, PersonRow>(
        "SELECT u.user_id, u.username, u.role, u.disabled_at, u.created_at,
                COUNT(s.session_id) AS session_count
           FROM users u
           LEFT JOIN sessions s ON s.user_id = u.user_id AND s.revoked_at IS NULL
          GROUP BY u.user_id
          ORDER BY u.created_at, u.username",
    )
    .fetch_all(state.db().pool())
    .await;
    match rows {
        Ok(rows) => Json(PeopleResponse {
            users: rows.into_iter().map(person_dto).collect(),
        })
        .into_response(),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Owner-only user creation (design §12.1: Owners may create accounts; no
/// public registration). The password is hashed with Argon2id and never
/// stored or returned in plaintext; the Audit row carries only the
/// username and role.
#[utoipa::path(
    post,
    path = "/api/admin/v1/people",
    tag = "admin",
    request_body = CreatePersonRequest,
    responses((status = 200, body = Person), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn create_person(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: CreatePersonRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    if let Err(reason) = validate_username(&body.username) {
        return field_error(&request_id.0, "invalid_username", reason, &["username"]);
    }
    if let Err(reason) = validate_password(&body.password) {
        return field_error(&request_id.0, "invalid_password", reason, &["password"]);
    }
    if body.role != "owner" && body.role != "viewer" {
        return field_error(
            &request_id.0,
            "invalid_role",
            "role must be owner or viewer",
            &["role"],
        );
    }
    let password_hash = match hash_password(body.password.as_bytes()) {
        Ok(hash) => hash,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    match crate::auth::create_user(
        state.db(),
        &principal.0.user_id,
        &body.username,
        &body.role,
        &password_hash,
    )
    .await
    {
        Ok(user_id) => {
            state.admin_realtime().publish("access", None::<String>, 0);
            // The user row committed with zero Sessions; return the same
            // allowlisted DTO the listing would produce.
            Json(Person {
                user_id,
                username: body.username,
                role: body.role,
                disabled: false,
                created_at: format_rfc3339(now_utc()),
                session_count: 0,
            })
            .into_response()
        }
        Err(IdentityError::UsernameTaken(_)) => mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "username_taken",
            "this username is already in use",
        ),
        Err(IdentityError::Database(_)) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
        Err(IdentityError::InvalidRole) => mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_role",
            "role must be owner or viewer",
        ),
    }
}

/// The mutation outcomes shared by role/status/password changes.
enum PersonMutation {
    Role,
    Disabled,
    PasswordReset,
    Protected,
}

/// Outcomes of a Session revoke: success carries the revoked instant;
/// `AlreadyRevoked` and `NotFound` distinguish a revoke race from an
/// unknown Session.
enum RevokeOutcome {
    Revoked { revoked_at: String },
    AlreadyRevoked,
    NotFound,
}

/// Load a user row inside a transaction and apply the final-Owner guard:
/// the last enabled Owner cannot be disabled or demoted (design §12.1).
/// Returns `None` when the user does not exist.
async fn load_user_for_mutation(
    tx: &mut sqlx::SqliteConnection,
    user_id: &str,
) -> Result<Option<(String, String, Option<String>)>, sqlx::Error> {
    sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT username, role, disabled_at FROM users WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await
}

/// Fetch one Person DTO after a mutation committed (allowlisted view).
async fn person_after_mutation(state: &AppState, user_id: &str) -> Option<Person> {
    sqlx::query_as::<_, PersonRow>(
        "SELECT u.user_id, u.username, u.role, u.disabled_at, u.created_at,
                COUNT(s.session_id) AS session_count
           FROM users u
           LEFT JOIN sessions s ON s.user_id = u.user_id AND s.revoked_at IS NULL
          WHERE u.user_id = ?
          GROUP BY u.user_id",
    )
    .bind(user_id)
    .fetch_optional(state.db().pool())
    .await
    .ok()
    .flatten()
    .map(person_dto)
}

/// Owner-only role change (design §12.1/§12.3). The final valid Owner is
/// protected; a role change revokes every Session of the affected user
/// immediately, which closes their bound Public/Admin streams on the next
/// revalidation.
#[utoipa::path(
    put,
    path = "/api/admin/v1/people/{user_id}/role",
    tag = "admin",
    params(("user_id" = String, Path, description = "User ID")),
    request_body = PersonRoleRequest,
    responses((status = 200, body = Person), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn set_person_role(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: PersonRoleRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    if body.role != "owner" && body.role != "viewer" {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_role",
            "role must be owner or viewer",
        );
    }
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let outcome: Result<Option<PersonMutation>, sqlx::Error> = async {
        let Some((username, current_role, disabled_at)) =
            load_user_for_mutation(&mut tx, &user_id).await?
        else {
            return Ok(None);
        };
        if current_role == "owner"
            && disabled_at.is_none()
            && body.role == "viewer"
            && count_enabled_owners(&mut *tx).await? <= 1
        {
            return Ok(Some(PersonMutation::Protected));
        }
        sqlx::query("UPDATE users SET role = ?, updated_at = ? WHERE user_id = ?")
            .bind(&body.role)
            .bind(format_rfc3339(now_utc()))
            .bind(&user_id)
            .execute(&mut *tx)
            .await?;
        revoke_user_sessions(&mut *tx, &user_id).await?;
        let after = serde_json::json!({ "username": username, "role": body.role });
        crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            "user_role_changed",
            "user",
            &username,
            Some(&after),
        )
        .await?;
        Ok(Some(PersonMutation::Role))
    }
    .await;
    match outcome {
        Ok(Some(PersonMutation::Protected)) => mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "final_owner_protected",
            "the final valid Owner cannot be demoted",
        ),
        Ok(Some(PersonMutation::Role)) => {
            if tx.commit().await.is_err() {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
            state.admin_realtime().publish("access", None::<String>, 0);
            match person_after_mutation(&state, &user_id).await {
                Some(person) => Json(person).into_response(),
                None => mutation_error(
                    &request_id.0,
                    StatusCode::NOT_FOUND,
                    "user_not_found",
                    "user not found",
                ),
            }
        }
        Ok(None) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "user_not_found",
            "user not found",
        ),
        Ok(Some(PersonMutation::Disabled)) | Ok(Some(PersonMutation::PasswordReset)) => {
            unreachable!("role mutation only produces role outcomes")
        }
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Owner-only enable/disable (design §12.1). The final valid Owner cannot
/// be disabled; disabling a user revokes all of their Sessions
/// immediately, which closes their bound streams.
#[utoipa::path(
    put,
    path = "/api/admin/v1/people/{user_id}/status",
    tag = "admin",
    params(("user_id" = String, Path, description = "User ID")),
    request_body = PersonStatusRequest,
    responses((status = 200, body = Person), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn set_person_status(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: PersonStatusRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let outcome: Result<Option<PersonMutation>, sqlx::Error> = async {
        let Some((username, current_role, disabled_at)) =
            load_user_for_mutation(&mut tx, &user_id).await?
        else {
            return Ok(None);
        };
        if body.disabled
            && current_role == "owner"
            && disabled_at.is_none()
            && count_enabled_owners(&mut *tx).await? <= 1
        {
            return Ok(Some(PersonMutation::Protected));
        }
        let disabled_value = if body.disabled {
            Some(format_rfc3339(now_utc()))
        } else {
            None
        };
        sqlx::query("UPDATE users SET disabled_at = ?, updated_at = ? WHERE user_id = ?")
            .bind(&disabled_value)
            .bind(format_rfc3339(now_utc()))
            .bind(&user_id)
            .execute(&mut *tx)
            .await?;
        if body.disabled {
            revoke_user_sessions(&mut *tx, &user_id).await?;
        }
        let after = serde_json::json!({ "username": username, "disabled": body.disabled });
        crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            if body.disabled {
                "user_disabled"
            } else {
                "user_enabled"
            },
            "user",
            &username,
            Some(&after),
        )
        .await?;
        Ok(Some(PersonMutation::Disabled))
    }
    .await;
    match outcome {
        Ok(Some(PersonMutation::Protected)) => mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "final_owner_protected",
            "the final valid Owner cannot be disabled",
        ),
        Ok(Some(PersonMutation::Disabled)) => {
            if tx.commit().await.is_err() {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
            state.admin_realtime().publish("access", None::<String>, 0);
            match person_after_mutation(&state, &user_id).await {
                Some(person) => Json(person).into_response(),
                None => mutation_error(
                    &request_id.0,
                    StatusCode::NOT_FOUND,
                    "user_not_found",
                    "user not found",
                ),
            }
        }
        Ok(None) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "user_not_found",
            "user not found",
        ),
        Ok(Some(PersonMutation::Role)) | Ok(Some(PersonMutation::PasswordReset)) => {
            unreachable!("status mutation only produces status outcomes")
        }
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Owner-only password reset (design §12.1: Owners may reset accounts).
/// The new password is hashed and never returned; all Sessions of the
/// user are revoked immediately so old state cannot continue.
#[utoipa::path(
    post,
    path = "/api/admin/v1/people/{user_id}/reset-password",
    tag = "admin",
    params(("user_id" = String, Path, description = "User ID")),
    request_body = ResetPasswordRequest,
    responses((status = 204, description = "Password reset and Sessions revoked"), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn reset_person_password(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: ResetPasswordRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    if let Err(reason) = validate_password(&body.password) {
        return field_error(&request_id.0, "invalid_password", reason, &["password"]);
    }
    let password_hash = match hash_password(body.password.as_bytes()) {
        Ok(hash) => hash,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let outcome: Result<Option<PersonMutation>, sqlx::Error> = async {
        let Some((username, _, _)) = load_user_for_mutation(&mut tx, &user_id).await? else {
            return Ok(None);
        };
        sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE user_id = ?")
            .bind(&password_hash)
            .bind(format_rfc3339(now_utc()))
            .bind(&user_id)
            .execute(&mut *tx)
            .await?;
        revoke_user_sessions(&mut *tx, &user_id).await?;
        let after = serde_json::json!({ "username": username });
        crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            "password_reset",
            "user",
            &username,
            Some(&after),
        )
        .await?;
        Ok(Some(PersonMutation::PasswordReset))
    }
    .await;
    match outcome {
        Ok(Some(PersonMutation::PasswordReset)) => {
            if tx.commit().await.is_err() {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
            state.admin_realtime().publish("access", None::<String>, 0);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "user_not_found",
            "user not found",
        ),
        Ok(Some(PersonMutation::Role)) | Ok(Some(PersonMutation::Disabled)) => {
            unreachable!("password mutation only produces a password outcome")
        }
        Ok(Some(PersonMutation::Protected)) => {
            unreachable!("password reset never triggers final-Owner protection")
        }
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Owner-only coarse Session listing (design §12.3). Only active
/// (non-revoked) Sessions appear, with creation/last-activity/expiry and
/// the coarse client hint — never tokens, full User-Agents, or raw IPs.
#[utoipa::path(
    get,
    path = "/api/admin/v1/sessions",
    tag = "admin",
    responses((status = 200, body = SessionsResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn sessions_list(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        ),
    >(
        "SELECT s.session_id, u.user_id, u.username, u.role, s.client_hint,
                s.created_at, s.last_seen_at, s.expires_at
           FROM sessions s JOIN users u ON u.user_id = s.user_id
          WHERE s.revoked_at IS NULL
          ORDER BY s.last_seen_at DESC LIMIT 200",
    )
    .fetch_all(state.db().pool())
    .await;
    match rows {
        Ok(rows) => Json(SessionsResponse {
            sessions: rows
                .into_iter()
                .map(
                    |(
                        session_id,
                        user_id,
                        username,
                        role,
                        client_hint,
                        created_at,
                        last_seen_at,
                        expires_at,
                    )| {
                        let current = session_id == principal.0.session_id;
                        SessionItem {
                            session_id,
                            user_id,
                            username,
                            role,
                            client_hint,
                            created_at,
                            last_seen_at,
                            expires_at,
                            current,
                        }
                    },
                )
                .collect(),
        })
        .into_response(),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Owner-only Session revoke (design §12.3: an Owner may revoke any
/// Session). The revoked user's bound Public/Admin streams close on their
/// next revalidation, and their tabs receive the access-generation
/// transition without any token or DTO being broadcast.
#[utoipa::path(
    post,
    path = "/api/admin/v1/sessions/{session_id}/revoke",
    tag = "admin",
    params(("session_id" = String, Path, description = "Session ID")),
    request_body = RevokeSessionRequest,
    responses((status = 200, body = RevokeSessionResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn revoke_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let outcome: Result<RevokeOutcome, sqlx::Error> = async {
        let username = sqlx::query_scalar::<_, String>(
            "SELECT u.username FROM sessions s JOIN users u ON u.user_id = s.user_id WHERE s.session_id = ?",
        )
        .bind(&session_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(username) = username else {
            return Ok(RevokeOutcome::NotFound);
        };
        let revoked_at = format_rfc3339(now_utc());
        let result = sqlx::query(
            "UPDATE sessions SET revoked_at = ? WHERE session_id = ? AND revoked_at IS NULL",
        )
        .bind(&revoked_at)
        .bind(&session_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(RevokeOutcome::AlreadyRevoked);
        }
        let after = serde_json::json!({ "username": username, "sessionId": session_id });
        crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            "session_revoked",
            "session",
            &session_id,
            Some(&after),
        )
        .await?;
        Ok(RevokeOutcome::Revoked { revoked_at })
    }
    .await;
    match outcome {
        Ok(RevokeOutcome::Revoked { revoked_at }) => {
            if tx.commit().await.is_err() {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
            state.admin_realtime().publish("access", None::<String>, 0);
            Json(RevokeSessionResponse {
                session_id,
                revoked_at,
            })
            .into_response()
        }
        Ok(RevokeOutcome::AlreadyRevoked) => mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "session_already_revoked",
            "this session is already revoked",
        ),
        Ok(RevokeOutcome::NotFound) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "session_not_found",
            "session not found",
        ),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Owner-only "revoke my other Sessions" (design §12.3: keeping the
/// current Session and revoking all others are distinct operations).
#[utoipa::path(
    post,
    path = "/api/admin/v1/sessions/revoke-others",
    tag = "admin",
    request_body = RevokeSessionRequest,
    responses((status = 200, body = RevokeOthersResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn revoke_other_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let outcome: Result<i64, sqlx::Error> = async {
        let result = sqlx::query(
            "UPDATE sessions SET revoked_at = ? WHERE user_id = ? AND session_id != ? AND revoked_at IS NULL",
        )
        .bind(format_rfc3339(now_utc()))
        .bind(&principal.0.user_id)
        .bind(&principal.0.session_id)
        .execute(&mut *tx)
        .await?;
        let count = result.rows_affected();
        let after =
            serde_json::json!({ "username": principal.0.username, "revokedCount": count });
        crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            "sessions_revoked",
            "session",
            &principal.0.session_id,
            Some(&after),
        )
        .await?;
        Ok(count as i64)
    }
    .await;
    match outcome {
        Ok(count) => {
            if tx.commit().await.is_err() {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
            state.admin_realtime().publish("access", None::<String>, 0);
            Json(RevokeOthersResponse {
                revoked_count: count,
            })
            .into_response()
        }
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Owner-only immutable Audit listing (design §18.2, issue #47): newest
/// first, bounded, with optional `event_kind`/`target_kind` filters and an
/// `audit_event_id` cursor for older pages. Details are the redacted
/// `after_json` bodies written by construction.
#[utoipa::path(
    get,
    path = "/api/admin/v1/audit",
    tag = "admin",
    params(
        ("limit" = Option<i64>, Query, description = "Page size, 1..=100 (default 50)"),
        ("event_kind" = Option<String>, Query, description = "Filter by event kind"),
        ("target_kind" = Option<String>, Query, description = "Filter by target kind"),
        ("before" = Option<i64>, Query, description = "Return events older than this audit_event_id"),
    ),
    responses((status = 200, body = AuditResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn audit_list(
    State(state): State<AppState>,
    Query(params): Query<AuditQuery>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let rows = sqlx::query_as::<_, (i64, String, Option<String>, String, String, String, Option<String>)>(
        "SELECT a.audit_event_id, a.event_kind, u.username, a.target_kind, a.target_id, a.created_at, a.after_json
           FROM audit_events a LEFT JOIN users u ON u.user_id = a.actor_user_id
          WHERE (?1 IS NULL OR a.event_kind = ?1)
            AND (?2 IS NULL OR a.target_kind = ?2)
            AND (?3 IS NULL OR a.audit_event_id < ?3)
          ORDER BY a.audit_event_id DESC LIMIT ?4",
    )
    .bind(params.event_kind.as_deref())
    .bind(params.target_kind.as_deref())
    .bind(params.before)
    .bind(limit)
    .fetch_all(state.db().pool())
    .await;
    match rows {
        Ok(rows) => {
            let items: Vec<AuditItem> = rows
                .into_iter()
                .map(
                    |(
                        audit_event_id,
                        event_kind,
                        actor_username,
                        target_kind,
                        target_id,
                        created_at,
                        after_json,
                    )| AuditItem {
                        audit_event_id,
                        event_kind,
                        actor_username,
                        target_kind,
                        target_id,
                        created_at,
                        details: after_json
                            .as_deref()
                            .and_then(|body| serde_json::from_str(body).ok()),
                    },
                )
                .collect();
            let next_before = (items.len() as i64 == limit)
                .then(|| items.last().map(|item| item.audit_event_id))
                .flatten();
            Json(AuditResponse { items, next_before }).into_response()
        }
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Owner-only read of the anonymous Home (Guest) setting.
#[utoipa::path(
    get,
    path = "/api/admin/v1/access",
    tag = "admin",
    responses((status = 200, body = AccessSettingsResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn get_access_settings(
    State(state): State<AppState>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match crate::auth::anonymous_home_enabled(state.db()).await {
        Ok(guest_enabled) => Json(AccessSettingsResponse { guest_enabled }).into_response(),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        ),
    }
}

/// Owner-only toggle of anonymous Home (Guest) access (design §12.1).
/// Disabling closes every open Guest stream (their bound check sees the
/// setting change) and publishes a collection-level Public reset so open
/// pages clear cached projections and re-resolve authorization; enabling
/// publishes the same reset so anonymous visitors can render Home.
#[utoipa::path(
    put,
    path = "/api/admin/v1/access",
    tag = "admin",
    request_body = AccessSettingsRequest,
    responses((status = 200, body = AccessSettingsResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn set_access_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: AccessSettingsRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    let value = if body.guest_enabled { "1" } else { "0" };
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let outcome: Result<(), sqlx::Error> = async {
        sqlx::query(
            "INSERT INTO server_settings (setting_key, setting_value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(setting_key) DO UPDATE SET setting_value = excluded.setting_value, updated_at = excluded.updated_at",
        )
        .bind(crate::auth::SETTING_ANONYMOUS_HOME)
        .bind(value)
        .bind(format_rfc3339(now_utc()))
        .execute(&mut *tx)
        .await?;
        let after = serde_json::json!({ "guestEnabled": body.guest_enabled });
        crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            "guest_access_changed",
            "access",
            crate::auth::SETTING_ANONYMOUS_HOME,
            Some(&after),
        )
        .await?;
        Ok(())
    }
    .await;
    if outcome.is_err() {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    if tx.commit().await.is_err() {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "server database is unavailable",
        );
    }
    state.admin_realtime().publish("access", None::<String>, 0);
    state.public_realtime().publish_reset("guest_access", 0);
    Json(AccessSettingsResponse {
        guest_enabled: body.guest_enabled,
    })
    .into_response()
}

/// Field-level mutation error: the unified envelope carries the offending
/// field names so forms can mark the exact input (webui.md §10.3: field
/// errors plus page summary; issue #47: forms support field/page errors).
fn field_error(
    request_id: &str,
    code: &'static str,
    message: &'static str,
    fields: &[&str],
) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(crate::http::ApiErrorBody::with_fields(
            code,
            message,
            request_id,
            fields.iter().map(|field| (*field).to_owned()).collect(),
        )),
    )
        .into_response()
}

pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/people", get(people_list))
        .route("/people", post(create_person))
        .route("/people/{user_id}/role", put(set_person_role))
        .route("/people/{user_id}/status", put(set_person_status))
        .route(
            "/people/{user_id}/reset-password",
            post(reset_person_password),
        )
        .route("/sessions", get(sessions_list))
        .route("/sessions/{session_id}/revoke", post(revoke_session))
        .route("/sessions/revoke-others", post(revoke_other_sessions))
        .route("/audit", get(audit_list))
        .route("/access", get(get_access_settings))
        .route("/access", put(set_access_settings))
        .layer(axum::middleware::from_fn(group_middleware))
}

async fn group_middleware(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        ROUTE_GROUP_HEADER,
        axum::http::HeaderValue::from_static("admin"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::Value;
    use tempfile::tempdir;

    use crate::auth::{AuthConfig, create_owner, hash_password, login};

    async fn test_state() -> (tempfile::TempDir, AppState) {
        let dir = tempdir().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        (dir, AppState::new(database, None, auth))
    }

    fn session(user_id: &str, username: &str, role: &str) -> AuthenticatedSession {
        AuthenticatedSession(crate::auth::SessionInfo {
            session_id: format!("session-{username}"),
            user_id: user_id.to_owned(),
            username: username.to_owned(),
            role: role.to_owned(),
            created_at: time::OffsetDateTime::now_utc(),
            last_seen_at: time::OffsetDateTime::now_utc(),
            expires_at: time::OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        })
    }

    fn mutation_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers.insert(
            axum::http::header::ORIGIN,
            "http://127.0.0.1:8080".parse().unwrap(),
        );
        headers.insert("x-csrf-token", "csrf".parse().unwrap());
        headers
    }

    /// An independent development auth policy for login flows in tests
    /// (the pepper is loaded into memory, so the temp directory may drop).
    fn dev_auth_config() -> AuthConfig {
        let dir = tempdir().unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        )
    }

    async fn seed_owner_with_session(state: &AppState) -> (String, String) {
        let hash = hash_password(b"correct horse battery").unwrap();
        create_owner(state.db(), "admin", &hash).await.unwrap();
        let user_id =
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE username = 'admin'")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        (user_id, "admin".to_owned())
    }

    #[tokio::test]
    async fn people_list_never_exposes_password_material() {
        let (_dir, state) = test_state().await;
        let (user_id, _) = seed_owner_with_session(&state).await;
        let response = people_list(
            State(state),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let user = &value["users"][0];
        assert_eq!(user["username"], "admin");
        assert_eq!(user["role"], "owner");
        for forbidden in ["password", "passwordHash", "hash", "token", "csrf"] {
            assert!(
                user.get(forbidden).is_none(),
                "People DTO leaked: {forbidden}"
            );
        }
    }

    #[tokio::test]
    async fn final_owner_cannot_be_disabled_or_demoted() {
        let (_dir, state) = test_state().await;
        let (owner_id, _) = seed_owner_with_session(&state).await;
        let (user_id, _) = (owner_id.clone(), ());

        // Demote the final Owner.
        let response = set_person_role(
            State(state.clone()),
            Path(owner_id.clone()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(br#"{"role":"viewer"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["error"]["code"], "final_owner_protected");

        // Disable the final Owner.
        let response = set_person_status(
            State(state),
            Path(owner_id),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(br#"{"disabled":true}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn second_owner_can_be_disabled_and_demoted_but_sessions_revoke() {
        let (_dir, state) = test_state().await;
        let (user_id, _) = seed_owner_with_session(&state).await;
        let hash = hash_password(b"second owner password").unwrap();
        create_owner(state.db(), "ops", &hash).await.unwrap();
        let ops_id =
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE username = 'ops'")
                .fetch_one(state.db().pool())
                .await
                .unwrap();

        // Give ops an active Session, then demote them: the Session must be
        // revoked immediately (design §12.3: role changes revoke sessions).
        sqlx::query(
            "INSERT INTO sessions (session_id, user_id, token_digest, csrf_token_digest, created_at, last_seen_at, expires_at, revoked_at) VALUES ('ops-session', ?, x'01', x'02', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2099-01-01T00:00:00Z', NULL)",
        )
        .bind(&ops_id)
        .execute(state.db().pool())
        .await
        .unwrap();

        let response = set_person_role(
            State(state.clone()),
            Path(ops_id.clone()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(br#"{"role":"viewer"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let revoked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE session_id = 'ops-session' AND revoked_at IS NOT NULL",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(revoked, 1, "role change must revoke the user's Sessions");

        // Re-promote ops to Owner, then disable them: Sessions revoke again
        // and the DTO reports the disabled state.
        let response = set_person_role(
            State(state.clone()),
            Path(ops_id.clone()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(br#"{"role":"owner"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        sqlx::query(
            "INSERT INTO sessions (session_id, user_id, token_digest, csrf_token_digest, created_at, last_seen_at, expires_at, revoked_at) VALUES ('ops-session-2', ?, x'03', x'04', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2099-01-01T00:00:00Z', NULL)",
        )
        .bind(&ops_id)
        .execute(state.db().pool())
        .await
        .unwrap();
        let response = set_person_status(
            State(state.clone()),
            Path(ops_id),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(br#"{"disabled":true}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["disabled"], true);
        let revoked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE session_id = 'ops-session-2' AND revoked_at IS NOT NULL",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(revoked, 1, "disabling must revoke the user's Sessions");
    }

    #[tokio::test]
    async fn create_person_validates_and_audits() {
        let (_dir, state) = test_state().await;
        let (user_id, _) = seed_owner_with_session(&state).await;

        let response = create_person(
            State(state.clone()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(
                br#"{"username":"newuser","password":"a long enough password","role":"viewer"}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username = 'newuser'")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(count, 1);
        let audit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'viewer_created' AND actor_user_id IS NOT NULL",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit, 1, "user creation must be audited with the actor");

        // A duplicate username is a typed conflict.
        let response = create_person(
            State(state.clone()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(
                br#"{"username":"newuser","password":"a long enough password","role":"viewer"}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["error"]["code"], "username_taken");

        // A short password is a field error that names the offending
        // field, not a database write (issue #47: forms surface field
        // errors; webui.md §10.3).
        let response = create_person(
            State(state),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(
                br#"{"username":"shorty","password":"short","role":"viewer"}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["error"]["code"], "invalid_password");
        assert_eq!(value["error"]["fields"][0], "password");
    }

    #[tokio::test]
    async fn sessions_list_is_coarse_and_current_is_marked() {
        let (_dir, state) = test_state().await;
        let (user_id, _) = seed_owner_with_session(&state).await;
        let config = dev_auth_config();
        login(
            state.db(),
            &config,
            "admin",
            "correct horse battery",
            None,
            "Chrome · desktop",
        )
        .await
        .unwrap();

        let response = sessions_list(
            State(state),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let items = value["sessions"].as_array().unwrap();
        assert!(!items.is_empty());
        assert_eq!(items[0]["clientHint"], "Chrome · desktop");
        for forbidden in ["token", "csrfToken", "digest", "ip"] {
            assert!(
                items[0].get(forbidden).is_none(),
                "Session listing leaked: {forbidden}"
            );
        }
    }

    #[tokio::test]
    async fn revoke_session_is_idempotent_conflict_and_audited() {
        let (_dir, state) = test_state().await;
        let (user_id, _) = seed_owner_with_session(&state).await;
        sqlx::query(
            "INSERT INTO sessions (session_id, user_id, token_digest, csrf_token_digest, created_at, last_seen_at, expires_at, revoked_at) VALUES ('victim-session', (SELECT user_id FROM users WHERE username = 'admin'), x'05', x'06', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2099-01-01T00:00:00Z', NULL)",
        )
        .execute(state.db().pool())
        .await
        .unwrap();

        let response = revoke_session(
            State(state.clone()),
            Path("victim-session".to_owned()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // Revoking the same Session again is a typed conflict (revoke race).
        let response = revoke_session(
            State(state.clone()),
            Path("victim-session".to_owned()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);

        // Unknown Sessions are 404.
        let response = revoke_session(
            State(state.clone()),
            Path("ghost-session".to_owned()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let audit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'session_revoked'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit, 1, "only the successful revoke is audited");
    }

    #[tokio::test]
    async fn audit_listing_filters_redacts_and_paginates() {
        let (_dir, state) = test_state().await;
        let (user_id, _) = seed_owner_with_session(&state).await;
        // Seed events: one user creation and one login failure.
        create_person(
            State(state.clone()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(
                br#"{"username":"audit-user","password":"a long enough password","role":"viewer"}"#,
            ),
        )
        .await;

        let response = audit_list(
            State(state.clone()),
            Query(AuditQuery {
                limit: Some(10),
                event_kind: None,
                target_kind: None,
                before: None,
            }),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let kinds: Vec<&str> = value["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["eventKind"].as_str().unwrap())
            .collect();
        assert!(
            kinds.contains(&"viewer_created"),
            "listing must include the creation event: {kinds:?}"
        );
        // Redaction: the whole listing body must not contain sensitive keys.
        let body_text = String::from_utf8_lossy(&bytes).to_lowercase();
        for forbidden in ["password", "token", "credential", "csrf"] {
            assert!(
                !body_text.contains(forbidden),
                "Audit output leaked: {forbidden}"
            );
        }

        // Filter by event kind.
        let response = audit_list(
            State(state.clone()),
            Query(AuditQuery {
                limit: Some(10),
                event_kind: Some("login_failed".to_owned()),
                target_kind: None,
                before: None,
            }),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["items"].as_array().unwrap().len(), 0);

        // Bounded paging: limit 1 yields exactly one item and a cursor.
        let response = audit_list(
            State(state),
            Query(AuditQuery {
                limit: Some(1),
                event_kind: None,
                target_kind: None,
                before: None,
            }),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["items"].as_array().unwrap().len(), 1);
        assert!(value["nextBefore"].is_i64());
    }

    #[tokio::test]
    async fn guest_toggle_persists_and_is_audited() {
        let (_dir, state) = test_state().await;
        let (user_id, _) = seed_owner_with_session(&state).await;
        assert!(
            !crate::auth::anonymous_home_enabled(state.db())
                .await
                .unwrap(),
            "Guest access must be disabled by default"
        );

        let response = set_access_settings(
            State(state.clone()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(br#"{"guestEnabled":true}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            crate::auth::anonymous_home_enabled(state.db())
                .await
                .unwrap()
        );

        let response = set_access_settings(
            State(state.clone()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(br#"{"guestEnabled":false}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            !crate::auth::anonymous_home_enabled(state.db())
                .await
                .unwrap()
        );

        let audit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'guest_access_changed'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit, 2);
    }

    #[tokio::test]
    async fn reset_password_revokes_sessions_and_never_returns_the_password() {
        let (_dir, state) = test_state().await;
        let (user_id, _) = seed_owner_with_session(&state).await;
        let viewer_hash = hash_password(b"viewer password 123456").unwrap();
        create_owner(state.db(), "v1", &viewer_hash).await.unwrap();
        let v1_id =
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE username = 'v1'")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO sessions (session_id, user_id, token_digest, csrf_token_digest, created_at, last_seen_at, expires_at, revoked_at) VALUES ('v1-session', ?, x'07', x'08', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2099-01-01T00:00:00Z', NULL)",
        )
        .bind(&v1_id)
        .execute(state.db().pool())
        .await
        .unwrap();

        let response = reset_person_password(
            State(state.clone()),
            Path(v1_id),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
            axum::body::Bytes::from_static(br#"{"password":"brand new password 123"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let revoked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE session_id = 'v1-session' AND revoked_at IS NOT NULL",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(revoked, 1, "password reset must revoke the user's Sessions");
        let stored: Option<String> =
            sqlx::query_scalar("SELECT password_hash FROM users WHERE username = 'v1'")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        let stored = stored.unwrap();
        assert_ne!(stored, viewer_hash);
        // The stored value is an Argon2id PHC string, never the plaintext.
        assert!(stored.starts_with("$argon2id$"));
    }

    #[tokio::test]
    async fn revoke_others_keeps_the_current_session() {
        let (_dir, state) = test_state().await;
        let (user_id, _) = seed_owner_with_session(&state).await;
        // The acting session exists in storage; revoke-others must keep it.
        sqlx::query(
            "INSERT INTO sessions (session_id, user_id, token_digest, csrf_token_digest, created_at, last_seen_at, expires_at, revoked_at) VALUES ('session-admin', ?, x'0c', x'0a', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2099-01-01T00:00:00Z', NULL)",
        )
        .bind(&user_id)
        .execute(state.db().pool())
        .await
        .unwrap();
        for (id, digest) in [("other-1", "x'09'"), ("other-2", "x'0b'")] {
            sqlx::query(&format!(
                "INSERT INTO sessions (session_id, user_id, token_digest, csrf_token_digest, created_at, last_seen_at, expires_at, revoked_at) VALUES (?, ?, {digest}, x'0a', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2099-01-01T00:00:00Z', NULL)"
            ))
            .bind(id)
            .bind(&user_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        }
        let response = revoke_other_sessions(
            State(state.clone()),
            mutation_headers(),
            Extension(session(&user_id, "admin", "owner")),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["revokedCount"], 2);
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE user_id = ? AND revoked_at IS NULL",
        )
        .bind(&user_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(active, 1, "the current Session must be kept");
    }
}
