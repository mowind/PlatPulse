//! Human authentication (design §12): Argon2id passwords, DB-backed opaque
//! sessions keyed by HMAC digests, synchronizer CSRF tokens, the login rate
//! limiter, and the identity mutations that create the first Owner.
//!
//! Tokens follow §12.5: `pp_session_<token_id>_<secret>`. The Server stores
//! only `HMAC-SHA-256(server_pepper, full_token)` and never the plaintext;
//! the CSRF token is derived from the session token so the WebUI can fetch
//! it from `GET /api/public/v1/session` without the Server storing another
//! secret.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::http::HeaderValue;
use axum::http::header::HeaderMap;
use rand_core::{OsRng, RngCore};
use serde_json::json;
use sqlx::FromRow;
use subtle::ConstantTimeEq;
use thiserror::Error;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use crate::database::ServerDatabase;
use crate::secrets::Pepper;

/// Production session cookie name (design §12.3): `__Host-` prefix requires
/// `Secure`, `Path=/`, and no `Domain`.
pub const SESSION_COOKIE_PRODUCTION: &str = "__Host-platpulse_session";

/// Explicit development-mode cookie (design §19.1): separately named so a
/// `__Host-` cookie is never emitted without `Secure`.
pub const SESSION_COOKIE_DEVELOPMENT: &str = "platpulse_dev_session";

/// Session token prefix from design §12.5.
pub const SESSION_TOKEN_PREFIX: &str = "pp_session_";

/// Secret part of the session token: 256 bits, hex-encoded.
pub const SESSION_SECRET_BYTES: usize = 32;

/// Default idle timeout for human sessions (design §12.3).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(12 * 60 * 60);
/// Default absolute lifetime for human sessions (design §12.3).
pub const DEFAULT_ABSOLUTE_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Minimum enforced human password length.
pub const MIN_PASSWORD_LENGTH: usize = 12;
/// Maximum password length accepted from input.
pub const MAX_PASSWORD_LENGTH: usize = 1024;

/// Failed login attempts per (client, username) inside the window.
pub const LOGIN_MAX_ATTEMPTS: u32 = 5;
/// Fixed window for the login rate limiter (design §19.4).
pub const LOGIN_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// `last_seen_at` is written at most this often (design §12.3).
pub const SESSION_ACTIVITY_THROTTLE: Duration = Duration::from_secs(60);

/// `server_settings` key controlling the Server-wide Site Access Mode.
pub const SETTING_SITE_ACCESS_MODE: &str = "site_access_mode";
/// Durable authorization generation for site-wide authorization transitions.
pub const SETTING_AUTHORIZATION_GENERATION: &str = "authorization_generation";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteAccessMode {
    Public,
    Private,
}

impl SiteAccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "public" => Some(Self::Public),
            "private" => Some(Self::Private),
            _ => None,
        }
    }
}

/// Read the Server-wide Home policy. Missing or malformed values fail closed.
pub async fn site_access_mode(db: &ServerDatabase) -> Result<SiteAccessMode, sqlx::Error> {
    let value = sqlx::query_scalar::<_, String>(
        "SELECT setting_value FROM server_settings WHERE setting_key = ?",
    )
    .bind(SETTING_SITE_ACCESS_MODE)
    .fetch_optional(db.pool())
    .await?;
    Ok(value
        .as_deref()
        .and_then(SiteAccessMode::parse)
        .unwrap_or(SiteAccessMode::Private))
}

/// Read the durable authorization generation, failing closed to generation 0.
pub async fn authorization_generation(db: &ServerDatabase) -> Result<i64, sqlx::Error> {
    let value = sqlx::query_scalar::<_, String>(
        "SELECT setting_value FROM server_settings WHERE setting_key = ?",
    )
    .bind(SETTING_AUTHORIZATION_GENERATION)
    .fetch_optional(db.pool())
    .await?;
    Ok(value
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0))
}

/// Whether at least one enabled Owner exists (design §12.2).
pub async fn has_owner(db: &ServerDatabase) -> Result<bool, sqlx::Error> {
    Ok(count_enabled_owners(db.pool()).await? > 0)
}

/// Count enabled Owners (design §12.1: the final valid Owner cannot be
/// disabled or demoted, so the mutation path must know the exact count
/// before it changes a role or disabled state). Accepts any SQLx executor
/// so the count runs inside the same transaction as the mutation.
pub async fn count_enabled_owners<'e, E>(executor: E) -> Result<i64, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'owner' AND disabled_at IS NULL")
        .fetch_one(executor)
        .await
}

/// Authenticated-session policy: cookie name/attributes, strict origin, and
/// token lifetimes.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub pepper: Pepper,
    pub cookie_name: String,
    pub cookie_secure: bool,
    /// Exact origin login requests must present (design §12.4).
    pub origin: String,
    pub idle_timeout: Duration,
    pub absolute_lifetime: Duration,
}

impl AuthConfig {
    /// Production policy: `__Host-platpulse_session`, `Secure`, strict
    /// origin, default lifetimes.
    pub fn production(pepper: Pepper, origin: String) -> Self {
        Self {
            pepper,
            cookie_name: SESSION_COOKIE_PRODUCTION.to_owned(),
            cookie_secure: true,
            origin,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            absolute_lifetime: DEFAULT_ABSOLUTE_LIFETIME,
        }
    }

    /// Explicit development policy (design §19.1): separate non-`__Host-`
    /// cookie without `Secure`, so local HTTP testing never emits an
    /// invalid or misleading production cookie.
    pub fn development(pepper: Pepper, origin: String) -> Self {
        Self {
            pepper,
            cookie_name: SESSION_COOKIE_DEVELOPMENT.to_owned(),
            cookie_secure: false,
            origin,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            absolute_lifetime: DEFAULT_ABSOLUTE_LIFETIME,
        }
    }

    /// Strict Origin validation for login: the request must present the
    /// configured origin exactly (design §12.4).
    pub fn origin_matches(&self, request_origin: Option<&HeaderValue>) -> bool {
        request_origin.is_some_and(|value| value.as_bytes() == self.origin.as_bytes())
    }
}

/// A validated human session, with the plaintext CSRF token derived from
/// the presented session token.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub user_id: String,
    pub username: String,
    pub role: String,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub csrf_token: String,
}

/// Return whether the session remains valid for a long-lived stream.
/// This deliberately rechecks revocation, expiry, idle timeout, user
/// disablement, and role changes instead of trusting connect-time auth.
pub async fn session_is_current(
    db: &ServerDatabase,
    session_id: &str,
    expected_role: Option<&str>,
    config: &AuthConfig,
) -> bool {
    let row = sqlx::query_as::<_, (String, String, Option<String>, String, String)>(
            "SELECT s.last_seen_at, s.expires_at, s.revoked_at, u.role, COALESCE(u.disabled_at, '') FROM sessions s JOIN users u ON u.user_id=s.user_id WHERE s.session_id=?",
        )
        .bind(session_id)
        .fetch_optional(db.pool())
        .await
        .ok()
        .flatten();
    let Some((last_seen, expires, revoked, role, disabled)) = row else {
        return false;
    };
    if revoked.is_some() || !disabled.is_empty() || expected_role.is_some_and(|value| role != value)
    {
        return false;
    }
    let now = now_utc();
    let expires_at = parse_rfc3339(&expires).unwrap_or(now);
    let last_seen_at = parse_rfc3339(&last_seen).unwrap_or(now);
    let idle_cutoff =
        last_seen_at + time::Duration::try_from(config.idle_timeout).expect("idle timeout fits");
    if now > expires_at || now > idle_cutoff {
        let _ = expire_session(db, session_id).await;
        return false;
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// No cookie or a token that cannot be validated (unknown, malformed,
    /// digest mismatch, revoked).
    Invalid,
    /// The session is past its absolute lifetime or idle timeout.
    Expired,
    /// The owning user has been disabled (design §12.1).
    UserDisabled,
}

/// Why a login attempt failed.
#[derive(Debug, Error)]
pub enum LoginError {
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("user is disabled")]
    UserDisabled,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Identity mutation errors surfaced by CLI commands and Admin mutations.
#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("username '{0}' is already in use")]
    UsernameTaken(String),
    #[error("role must be 'owner' or 'viewer'")]
    InvalidRole,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// A fixed-window attempt limiter. One instance per protected endpoint kind
/// (design §19.4: login, Enrollment, Recovery, AgentReport are independent).
pub struct RateLimiter {
    inner: Mutex<HashMap<(String, String), (Instant, u32)>>,
    max_attempts: u32,
    window: Duration,
}

impl RateLimiter {
    pub fn new(max_attempts: u32, window: Duration) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            max_attempts,
            window,
        }
    }

    /// Whether a new attempt for `key` is currently blocked.
    pub fn is_blocked(&self, key: (&str, &str)) -> bool {
        let mut inner = self.inner.lock().expect("rate limiter mutex poisoned");
        inner.retain(|_, (started, _)| started.elapsed() < self.window);
        inner
            .get(&key_tuple(key))
            .is_some_and(|(_, attempts)| *attempts >= self.max_attempts)
    }

    /// Record a failed attempt for `key`.
    pub fn record_failure(&self, key: (&str, &str)) {
        let mut inner = self.inner.lock().expect("rate limiter mutex poisoned");
        inner.retain(|_, (started, _)| started.elapsed() < self.window);
        let entry = inner
            .entry(key_tuple(key))
            .or_insert_with(|| (Instant::now(), 0));
        entry.1 += 1;
    }

    /// Forget failures for `key` after a successful attempt.
    pub fn record_success(&self, key: (&str, &str)) {
        let mut inner = self.inner.lock().expect("rate limiter mutex poisoned");
        inner.remove(&key_tuple(key));
    }
}

fn key_tuple(key: (&str, &str)) -> (String, String) {
    (key.0.to_owned(), key.1.to_owned())
}

/// Hash a human password with Argon2id (design §12.5).
pub fn hash_password(password: &[u8]) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password, &salt)
        .map(|hash| hash.to_string())
}

/// Verify a password against a stored Argon2id PHC string. Any parse or
/// verification failure reads as `false` so callers never distinguish
/// "unknown hash format" from "wrong password".
pub fn verify_password(stored_hash: &str, password: &[u8]) -> bool {
    match PasswordHash::new(stored_hash) {
        Ok(parsed) => Argon2::default().verify_password(password, &parsed).is_ok(),
        Err(_) => false,
    }
}

static DUMMY_HASH: OnceLock<String> = OnceLock::new();

/// A valid Argon2id hash verified when the username is unknown, so login
/// timing does not reveal whether a username exists.
fn dummy_hash() -> &'static str {
    DUMMY_HASH.get_or_init(|| {
        hash_password(b"platpulse-dummy-password-for-timing")
            .expect("dummy hash must build at runtime")
    })
}

/// Generate the secret half of a session token (256 bits, hex).
fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    crate::secrets::encode_hex(&buf)
}

/// Build a fresh `pp_session_<token_id>_<secret>` token.
pub fn new_session_token() -> (String, String) {
    let token_id = uuid::Uuid::new_v4().to_string();
    let secret = random_hex(SESSION_SECRET_BYTES);
    (
        token_id.clone(),
        format!("{SESSION_TOKEN_PREFIX}{token_id}_{secret}"),
    )
}

/// Derive a coarse, non-sensitive client hint from a User-Agent header
/// (design §12.3: the Server never stores the full User-Agent or a raw IP;
/// Session review shows only a coarse browser/platform family). Unknown or
/// missing agents read as `Unknown`; no version or identifier is kept.
pub fn client_hint_from_ua(user_agent: Option<&str>) -> String {
    let Some(agent) = user_agent else {
        return "Unknown".to_owned();
    };
    let browser = if agent.contains("Edg/") {
        "Edge"
    } else if agent.contains("OPR/") || agent.contains("Opera") {
        "Opera"
    } else if agent.contains("Chrome/") {
        "Chrome"
    } else if agent.contains("Firefox/") {
        "Firefox"
    } else if agent.contains("Safari/") {
        "Safari"
    } else {
        "Unknown"
    };
    let platform = if agent.contains("Mobile")
        || agent.contains("Android")
        || agent.contains("iPhone")
        || agent.contains("iPad")
    {
        "mobile"
    } else {
        "desktop"
    };
    format!("{browser} \u{00b7} {platform}")
}

/// Parse a session token into its non-sensitive id and full value.
fn split_token(token: &str) -> Option<(&str, &str)> {
    let rest = token.strip_prefix(SESSION_TOKEN_PREFIX)?;
    let (token_id, secret) = rest.split_once('_')?;
    if token_id.is_empty() || secret.is_empty() {
        return None;
    }
    Some((token_id, token))
}

/// Derive the synchronizer CSRF token from the session token. Rotating the
/// session token therefore rotates the CSRF token (design §12.4).
fn derive_csrf_token(config: &AuthConfig, full_token: &str) -> String {
    let mut message = b"platpulse-csrf:".to_vec();
    message.extend_from_slice(full_token.as_bytes());
    crate::secrets::encode_hex(&config.pepper.hmac_digest(&message))
}

/// RFC 3339 UTC timestamp with second precision, matching the wire rule for
/// wall-clock timestamps ("…Z", no fractional seconds).
pub fn now_utc() -> OffsetDateTime {
    OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("0 ns is valid")
}

pub fn format_rfc3339(datetime: OffsetDateTime) -> String {
    datetime
        .to_offset(UtcOffset::UTC)
        .format(&Rfc3339)
        .expect("Rfc3339 formatting is infallible for valid datetimes")
}

/// Parse a canonical RFC 3339 UTC timestamp.
pub(crate) fn parse_rfc3339(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).ok()
}

/// Extract one cookie value from a `Cookie` header. Cookie names are
/// case-sensitive per RFC 6265.
pub fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let value = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    value.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}

/// Build a `Set-Cookie` value for the session cookie (design §12.3/§19.1).
pub fn session_cookie_header(name: &str, value: &str, secure: bool) -> String {
    let mut header = format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax");
    if secure {
        header.push_str("; Secure");
    }
    header
}

/// Build a `Set-Cookie` value that deletes the session cookie.
pub fn clear_cookie_header(name: &str, secure: bool) -> String {
    session_cookie_header(name, "", secure) + "; Max-Age=0"
}

#[derive(Debug, FromRow)]
struct UserRow {
    user_id: String,
    username: String,
    role: String,
    password_hash: String,
    disabled_at: Option<String>,
}

async fn find_user_by_username(
    db: &ServerDatabase,
    username: &str,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT user_id, username, role, password_hash, disabled_at FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(db.pool())
    .await
}

/// Revoke every active Session of one user (design §12.1/§12.3: password,
/// role, and disabled changes invalidate the related Sessions immediately).
/// Returns the number of Sessions actually revoked.
pub async fn revoke_user_sessions(
    executor: &mut sqlx::SqliteConnection,
    user_id: &str,
) -> Result<u64, sqlx::Error> {
    let result =
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
            .bind(format_rfc3339(now_utc()))
            .bind(user_id)
            .execute(&mut *executor)
            .await?;
    if result.rows_affected() > 0 {
        bump_authorization_generation(&mut *executor).await?;
    }
    Ok(result.rows_affected())
}

/// Advance the durable authorization generation inside the caller's
/// transaction. The generation is a transition signal, never a bearer token.
pub async fn bump_authorization_generation(
    executor: &mut sqlx::SqliteConnection,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "UPDATE server_settings
            SET setting_value = CAST(CAST(setting_value AS INTEGER) + 1 AS TEXT),
                updated_at = ?
          WHERE setting_key = ?
        RETURNING setting_value",
    )
    .bind(format_rfc3339(now_utc()))
    .bind(SETTING_AUTHORIZATION_GENERATION)
    .fetch_one(&mut *executor)
    .await
    .and_then(|value| {
        value
            .parse::<i64>()
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))
    })
}

/// Insert an identity mutation into the bounded audit sink (design §18.2).
/// Accepts any SQLx executor so identity mutations and their audit rows
/// commit in the same transaction. CLI mutations use `actor = None`
/// (`local-cli`).
pub async fn insert_audit_event<'e, E>(
    executor: E,
    actor_user_id: Option<&str>,
    event_kind: &str,
    target_kind: &str,
    target_id: &str,
    after_json: Option<&serde_json::Value>,
) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    insert_audit_change(
        executor,
        actor_user_id,
        event_kind,
        target_kind,
        target_id,
        None,
        after_json,
    )
    .await
}

/// Insert an Audit Event with explicit before/after values. Both sides are
/// redacted before persistence so configuration transitions cannot retain
/// sensitive input by accident.
pub async fn insert_audit_change<'e, E>(
    executor: E,
    actor_user_id: Option<&str>,
    event_kind: &str,
    target_kind: &str,
    target_id: &str,
    before_json: Option<&serde_json::Value>,
    after_json: Option<&serde_json::Value>,
) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let created_at = format_rfc3339(now_utc());
    let before_json = before_json.map(crate::redaction::redact_json_value);
    let after_json = after_json.map(crate::redaction::redact_json_value);
    let target_id = crate::redaction::redact_sensitive(target_id);
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, before_json, after_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(actor_user_id)
    .bind(event_kind)
    .bind(target_kind)
    .bind(target_id)
    .bind(before_json.as_ref().map(serde_json::Value::to_string))
    .bind(after_json.as_ref().map(serde_json::Value::to_string))
    .bind(created_at)
    .execute(executor)
    .await?;
    Ok(())
}

/// Standalone audit write for events that have no mutation to be atomic
/// with (e.g. failed logins).
pub async fn write_audit_event(
    db: &ServerDatabase,
    actor_user_id: Option<&str>,
    event_kind: &str,
    target_kind: &str,
    target_id: &str,
    after_json: Option<&serde_json::Value>,
) -> Result<(), sqlx::Error> {
    insert_audit_event(
        db.pool(),
        actor_user_id,
        event_kind,
        target_kind,
        target_id,
        after_json,
    )
    .await
}

/// Insert a human user with the given role. The password is hashed by the
/// caller; this function owns the transaction so the user row and its
/// audit event cannot diverge. `actor_user_id` is the acting principal
/// (Admin mutations) or `None` for local CLI provisioning. Returns the
/// new user id.
async fn insert_human_user(
    db: &ServerDatabase,
    actor_user_id: Option<&str>,
    username: &str,
    role: HumanRole,
    password_hash: &str,
) -> Result<String, IdentityError> {
    let user_id = uuid::Uuid::new_v4().to_string();
    let now = format_rfc3339(now_utc());
    let mut transaction = db.pool().begin().await?;

    let insert = sqlx::query(
        "INSERT INTO users (user_id, username, role, password_hash, disabled_at, created_at, updated_at) VALUES (?, ?, ?, ?, NULL, ?, ?)",
    )
    .bind(&user_id)
    .bind(username)
    .bind(role.role())
    .bind(password_hash)
    .bind(&now)
    .bind(&now)
    .execute(&mut *transaction)
    .await;
    if let Err(error) = insert {
        if error
            .as_database_error()
            .is_some_and(|db_error| db_error.is_unique_violation())
        {
            return Err(IdentityError::UsernameTaken(username.to_owned()));
        }
        return Err(IdentityError::Database(error));
    }

    let after = json!({ "username": username, "role": role.role() });
    insert_audit_event(
        &mut *transaction,
        actor_user_id,
        role.event_kind(),
        "user",
        username,
        Some(&after),
    )
    .await?;

    transaction.commit().await?;
    Ok(user_id)
}

/// A human principal role provisioned from the CLI (design §12.1). Bundles
/// the database role value with its audit event kind so the pairing can
/// never diverge; the `users` CHECK constraint remains the backstop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HumanRole {
    Owner,
    Viewer,
}

impl HumanRole {
    fn role(self) -> &'static str {
        match self {
            HumanRole::Owner => "owner",
            HumanRole::Viewer => "viewer",
        }
    }

    fn event_kind(self) -> &'static str {
        match self {
            HumanRole::Owner => "owner_created",
            HumanRole::Viewer => "viewer_created",
        }
    }
}

/// Create an Owner from the CLI (design §12.2). Same provisioning seam as
/// Viewer creation; only the role and audit event differ.
pub async fn create_owner(
    db: &ServerDatabase,
    username: &str,
    password_hash: &str,
) -> Result<(), IdentityError> {
    insert_human_user(db, None, username, HumanRole::Owner, password_hash)
        .await
        .map(|_| ())
}

/// Create a Viewer from the CLI (design §12.1/§13.1): a human principal
/// that may use Home but has no administrative authority. The password is
/// hashed by the caller; the user row and its `viewer_created` audit event
/// commit atomically, matching the Owner baseline.
pub async fn create_viewer(
    db: &ServerDatabase,
    username: &str,
    password_hash: &str,
) -> Result<(), IdentityError> {
    insert_human_user(db, None, username, HumanRole::Viewer, password_hash)
        .await
        .map(|_| ())
}

/// Create a human user from an Owner Admin mutation (issue #47). The
/// acting Owner is recorded as the Audit actor; `role` must be `owner` or
/// `viewer` and the password is hashed by the caller. Returns the new
/// user id.
pub async fn create_user(
    db: &ServerDatabase,
    actor_user_id: &str,
    username: &str,
    role: &str,
    password_hash: &str,
) -> Result<String, IdentityError> {
    let role = match role {
        "owner" => HumanRole::Owner,
        "viewer" => HumanRole::Viewer,
        _ => return Err(IdentityError::InvalidRole),
    };
    insert_human_user(db, Some(actor_user_id), username, role, password_hash).await
}

/// Validate a human username: non-empty, bounded length, no whitespace or
/// control characters.
pub fn validate_username(username: &str) -> Result<(), &'static str> {
    if username.is_empty() {
        return Err("username must not be empty");
    }
    if username.len() > 64 {
        return Err("username must be at most 64 characters");
    }
    if username
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("username must not contain whitespace or control characters");
    }
    Ok(())
}

/// Validate a human password: bounded length, never empty.
pub fn validate_password(password: &str) -> Result<(), &'static str> {
    if password.is_empty() {
        return Err("password must not be empty");
    }
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err("password must be at least 12 characters");
    }
    if password.len() > MAX_PASSWORD_LENGTH {
        return Err("password must be at most 1024 characters");
    }
    Ok(())
}

#[derive(Debug, FromRow)]
struct SessionRow {
    session_id: String,
    token_digest: Vec<u8>,
    csrf_token_digest: Vec<u8>,
    created_at: String,
    last_seen_at: String,
    expires_at: String,
    revoked_at: Option<String>,
    user_id: String,
    username: String,
    role: String,
    disabled_at: Option<String>,
}

async fn insert_session<'e, E>(
    executor: E,
    config: &AuthConfig,
    user_id: &str,
    username: &str,
    role: &str,
    client_hint: &str,
) -> Result<(SessionInfo, String), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let (token_id, full_token) = new_session_token();
    let token_digest = config.pepper.hmac_digest(full_token.as_bytes());
    let csrf_token = derive_csrf_token(config, &full_token);
    let csrf_digest = config.pepper.hmac_digest(csrf_token.as_bytes());
    let now = now_utc();
    let created_at = format_rfc3339(now);
    let expires_at = format_rfc3339(
        now + time::Duration::try_from(config.absolute_lifetime)
            .expect("session lifetime fits in time::Duration"),
    );

    sqlx::query(
        "INSERT INTO sessions (session_id, user_id, token_digest, csrf_token_digest, created_at, last_seen_at, expires_at, revoked_at, client_hint) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?)",
    )
    .bind(&token_id)
    .bind(user_id)
    .bind(token_digest.to_vec())
    .bind(csrf_digest.to_vec())
    .bind(&created_at)
    .bind(&created_at)
    .bind(&expires_at)
    .bind(client_hint)
    .execute(executor)
    .await?;

    Ok((
        SessionInfo {
            session_id: token_id,
            user_id: user_id.to_owned(),
            username: username.to_owned(),
            role: role.to_owned(),
            created_at: now,
            last_seen_at: now,
            expires_at: now
                + time::Duration::try_from(config.absolute_lifetime)
                    .expect("session lifetime fits in time::Duration"),
            csrf_token,
        },
        full_token,
    ))
}

/// Authenticate a presented session token against the database.
pub async fn authenticate_token(
    db: &ServerDatabase,
    config: &AuthConfig,
    token: &str,
) -> Result<SessionInfo, SessionError> {
    let Some((token_id, full_token)) = split_token(token) else {
        return Err(SessionError::Invalid);
    };

    let Some(row) = sqlx::query_as::<_, SessionRow>(
        "SELECT s.session_id, s.token_digest, s.csrf_token_digest, s.created_at, s.last_seen_at, s.expires_at, s.revoked_at, u.user_id, u.username, u.role, u.disabled_at
         FROM sessions s JOIN users u ON u.user_id = s.user_id WHERE s.session_id = ?",
    )
    .bind(token_id)
    .fetch_optional(db.pool())
    .await
    .map_err(|_| SessionError::Invalid)?
    else {
        return Err(SessionError::Invalid);
    };

    if row.revoked_at.is_some() {
        return Err(SessionError::Invalid);
    }
    if row.disabled_at.is_some() {
        return Err(SessionError::UserDisabled);
    }

    let now = now_utc();
    let expires_at = parse_rfc3339(&row.expires_at).unwrap_or(now);
    let last_seen_at = parse_rfc3339(&row.last_seen_at).unwrap_or(now);
    let idle_cutoff = last_seen_at
        + time::Duration::try_from(config.idle_timeout)
            .expect("idle timeout fits in time::Duration");
    let expected_digest = config.pepper.hmac_digest(full_token.as_bytes());
    if !bool::from(expected_digest.ct_eq(&row.token_digest)) {
        return Err(SessionError::Invalid);
    }
    let csrf_token = derive_csrf_token(config, full_token);
    let expected_csrf_digest = config.pepper.hmac_digest(csrf_token.as_bytes());
    if !bool::from(expected_csrf_digest.ct_eq(&row.csrf_token_digest)) {
        return Err(SessionError::Invalid);
    }

    if now > expires_at || now > idle_cutoff {
        expire_session(db, &row.session_id)
            .await
            .map_err(|_| SessionError::Invalid)?;
        return Err(SessionError::Expired);
    }
    Ok(SessionInfo {
        session_id: row.session_id,
        user_id: row.user_id,
        username: row.username,
        role: row.role,
        created_at: parse_rfc3339(&row.created_at).unwrap_or(now),
        last_seen_at,
        expires_at,
        csrf_token,
    })
}

/// Mark one expired session revoked and advance authorization generation once.
/// Expiry is lazy: the first request that presents an otherwise valid expired
/// token performs the durable transition; later requests observe the revoke.
async fn expire_session(db: &ServerDatabase, session_id: &str) -> Result<(), sqlx::Error> {
    let mut transaction = db.pool().begin().await?;
    let result = sqlx::query(
        "UPDATE sessions SET revoked_at = ? WHERE session_id = ? AND revoked_at IS NULL",
    )
    .bind(format_rfc3339(now_utc()))
    .bind(session_id)
    .execute(&mut *transaction)
    .await?;
    if result.rows_affected() > 0 {
        bump_authorization_generation(&mut transaction).await?;
    }
    transaction.commit().await
}

/// Login: verify credentials and create a fresh session. Returns the new
/// session and its full `pp_session_…` token (placed in the cookie; never
/// stored). If the request presented an existing valid session token, that
/// session is revoked so the Session ID rotates on login (design §12.3).
pub async fn login(
    db: &ServerDatabase,
    config: &AuthConfig,
    username: &str,
    password: &str,
    presented_token: Option<&str>,
    client_hint: &str,
) -> Result<(SessionInfo, String), LoginError> {
    let user = find_user_by_username(db, username)
        .await
        .map_err(LoginError::Database)?;
    let user = match user {
        Some(user) => {
            if !verify_password(&user.password_hash, password.as_bytes()) {
                return Err(LoginError::InvalidCredentials);
            }
            user
        }
        None => {
            verify_password(dummy_hash(), password.as_bytes());
            return Err(LoginError::InvalidCredentials);
        }
    };
    if user.disabled_at.is_some() {
        return Err(LoginError::UserDisabled);
    }

    // Session ID rotation: validate the presented token against the pool
    // first (the transaction below holds the Server's only connection),
    // then revoke it inside the transaction so rotation and the new
    // session commit atomically.
    let presented_session_id = match presented_token {
        Some(token) => authenticate_token(db, config, token)
            .await
            .ok()
            .map(|existing| existing.session_id),
        None => None,
    };
    let mut transaction = db.pool().begin().await.map_err(LoginError::Database)?;

    if let Some(session_id) = presented_session_id {
        sqlx::query(
            "UPDATE sessions SET revoked_at = ? WHERE session_id = ? AND revoked_at IS NULL",
        )
        .bind(format_rfc3339(now_utc()))
        .bind(session_id)
        .execute(&mut *transaction)
        .await
        .map_err(LoginError::Database)?;
        bump_authorization_generation(&mut transaction)
            .await
            .map_err(LoginError::Database)?;
    }

    let (session, full_token) = insert_session(
        &mut *transaction,
        config,
        &user.user_id,
        &user.username,
        &user.role,
        client_hint,
    )
    .await
    .map_err(LoginError::Database)?;
    let after = json!({ "username": user.username, "role": user.role });
    insert_audit_event(
        &mut *transaction,
        Some(&user.user_id),
        "session_created",
        "session",
        &session.session_id,
        Some(&after),
    )
    .await
    .map_err(LoginError::Database)?;

    transaction.commit().await.map_err(LoginError::Database)?;
    Ok((session, full_token))
}

/// Revoke one session by id; the owning user stays intact.
pub async fn revoke_session(db: &ServerDatabase, session_id: &str) -> Result<bool, sqlx::Error> {
    let mut transaction = db.pool().begin().await?;
    let result = sqlx::query(
        "UPDATE sessions SET revoked_at = ? WHERE session_id = ? AND revoked_at IS NULL",
    )
    .bind(format_rfc3339(now_utc()))
    .bind(session_id)
    .execute(&mut *transaction)
    .await?;
    if result.rows_affected() > 0 {
        bump_authorization_generation(&mut transaction).await?;
    }
    transaction.commit().await?;
    Ok(result.rows_affected() > 0)
}

/// Throttled activity update: write `last_seen_at` at most once per
/// [`SESSION_ACTIVITY_THROTTLE`].
pub async fn touch_session(
    db: &ServerDatabase,
    session_id: &str,
    last_seen_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    if now_utc() - last_seen_at
        < time::Duration::try_from(SESSION_ACTIVITY_THROTTLE).expect("throttle fits")
    {
        return Ok(());
    }
    sqlx::query("UPDATE sessions SET last_seen_at = ? WHERE session_id = ?")
        .bind(format_rfc3339(now_utc()))
        .bind(session_id)
        .execute(db.pool())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use crate::database::{ServerDatabaseConfig, initialize};
    use crate::secrets::{create_pepper_file, load_pepper_file};

    use super::*;

    async fn test_db(dir: &Path) -> ServerDatabase {
        initialize(ServerDatabaseConfig::new(dir.join("server.db")))
            .await
            .unwrap()
    }

    fn test_config(dir: &Path) -> AuthConfig {
        let pepper_path = dir.join("server-pepper");
        create_pepper_file(&pepper_path).unwrap();
        AuthConfig::development(
            load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".into(),
        )
    }

    async fn create_test_owner(db: &ServerDatabase) -> String {
        let hash = hash_password(b"correct horse battery").unwrap();
        create_owner(db, "admin", &hash).await.unwrap();
        "admin".to_owned()
    }

    #[test]
    fn password_hash_verifies_argon2id_roundtrip() {
        let hash = hash_password(b"correct horse battery").unwrap();
        assert!(
            hash.starts_with("$argon2id$"),
            "human passwords use Argon2id"
        );
        assert!(verify_password(&hash, b"correct horse battery"));
        assert!(!verify_password(&hash, b"wrong password"));
        assert!(!verify_password("not-a-hash", b"correct horse battery"));
    }

    #[test]
    fn session_tokens_are_parseable_and_high_entropy() {
        let (token_id, full) = new_session_token();
        let (parsed_id, _) = split_token(&full).unwrap();
        assert_eq!(parsed_id, token_id);
        assert_eq!(full.len(), SESSION_TOKEN_PREFIX.len() + 36 + 1 + 64);
        let (_, full2) = new_session_token();
        assert_ne!(full, full2);

        assert!(split_token("garbage").is_none());
        assert!(split_token("pp_session_").is_none());
        assert!(split_token("pp_session_id").is_none());
    }

    #[test]
    fn rate_limiter_blocks_after_max_attempts_and_recovers() {
        let limiter = RateLimiter::new(3, Duration::from_millis(50));
        let key = ("127.0.0.1", "admin");
        assert!(!limiter.is_blocked(key));
        limiter.record_failure(key);
        limiter.record_failure(key);
        assert!(!limiter.is_blocked(key));
        limiter.record_failure(key);
        assert!(limiter.is_blocked(key), "third failure blocks");

        limiter.record_success(key);
        assert!(!limiter.is_blocked(key), "success clears the window");
    }

    #[test]
    fn rate_limiter_window_expires() {
        let limiter = RateLimiter::new(1, Duration::from_millis(30));
        let key = ("127.0.0.1", "admin");
        limiter.record_failure(key);
        assert!(limiter.is_blocked(key));
        std::thread::sleep(Duration::from_millis(60));
        assert!(!limiter.is_blocked(key));
    }

    #[test]
    fn cookie_parsing_and_headers() {
        let mut headers = HeaderMap::new();
        assert_eq!(cookie_value(&headers, "__Host-platpulse_session"), None);
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("other=1; __Host-platpulse_session=abc123; x=y"),
        );
        assert_eq!(
            cookie_value(&headers, "__Host-platpulse_session"),
            Some("abc123")
        );
        assert_eq!(cookie_value(&headers, "other"), Some("1"));
        assert_eq!(cookie_value(&headers, "missing"), None);

        let production = session_cookie_header(SESSION_COOKIE_PRODUCTION, "tok", true);
        assert!(production.contains("__Host-platpulse_session=tok"));
        assert!(production.contains("Path=/"));
        assert!(production.contains("HttpOnly"));
        assert!(production.contains("SameSite=Lax"));
        assert!(production.contains("Secure"));
        assert!(
            !production.contains("Domain="),
            "__Host- cookies must not set Domain"
        );

        let dev = session_cookie_header(SESSION_COOKIE_DEVELOPMENT, "tok", false);
        assert!(dev.contains("platpulse_dev_session=tok"));
        assert!(
            !dev.contains("Secure"),
            "development cookie must not be Secure"
        );
    }

    #[test]
    fn username_and_password_validation() {
        assert!(validate_username("admin").is_ok());
        assert!(validate_username("a-b_c.d").is_ok());
        assert!(validate_username("").is_err());
        assert!(validate_username("has space").is_err());
        assert!(validate_username(&"x".repeat(65)).is_err());

        assert!(validate_password("correct horse battery").is_ok());
        assert!(validate_password("short").is_err());
        assert!(validate_password("").is_err());
        assert!(validate_password(&"x".repeat(1025)).is_err());
    }

    #[tokio::test]
    async fn create_owner_and_has_owner() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        assert!(!has_owner(&db).await.unwrap());
        create_test_owner(&db).await;
        assert!(has_owner(&db).await.unwrap());

        let error = create_owner(&db, "admin", "hash").await.unwrap_err();
        assert!(matches!(error, IdentityError::UsernameTaken(_)));
    }

    #[tokio::test]
    async fn create_viewer_keeps_owner_setup_gate_and_logs_in_with_viewer_role() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        let config = test_config(dir.path());

        // A Viewer alone must not satisfy the setup gate: the first human
        // principal is the Owner (design §12.2).
        let hash = hash_password(b"correct horse battery").unwrap();
        create_viewer(&db, "viewer1", &hash).await.unwrap();
        assert!(!has_owner(&db).await.unwrap());

        // The same provisioning seam rejects duplicate usernames.
        let error = create_viewer(&db, "viewer1", &hash).await.unwrap_err();
        assert!(matches!(error, IdentityError::UsernameTaken(_)));

        // The Viewer logs in with role `viewer` and its session
        // authenticates like any other human session.
        let (session, full_token) = login(
            &db,
            &config,
            "viewer1",
            "correct horse battery",
            None,
            "Unknown",
        )
        .await
        .unwrap();
        assert_eq!(session.role, "viewer");
        assert_eq!(
            authenticate_token(&db, &config, &full_token)
                .await
                .unwrap()
                .role,
            "viewer"
        );
    }

    #[tokio::test]
    async fn session_survives_database_reopen_like_a_server_restart() {
        // Sessions are DB-backed opaque tokens (design §12.3): a normal
        // Server restart (close the database, reopen the same file, reload
        // the pepper from disk) must keep a valid Session valid without
        // any re-login.
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        create_test_owner(&db).await;
        let config = test_config(dir.path());
        let (_, full_token) = login(
            &db,
            &config,
            "admin",
            "correct horse battery",
            None,
            "Unknown",
        )
        .await
        .unwrap();
        db.close().await;

        // A restart reloads the pepper from disk rather than re-creating
        // it (create_pepper_file refuses to overwrite, like `init`).
        let reopened = test_db(dir.path()).await;
        let restarted_config = AuthConfig::development(
            load_pepper_file(&dir.path().join("server-pepper")).unwrap(),
            "http://127.0.0.1:8080".into(),
        );
        let session = authenticate_token(&reopened, &restarted_config, &full_token)
            .await
            .expect("a valid session must survive a restart");
        assert_eq!(session.username, "admin");
        assert_eq!(session.role, "owner");
    }

    #[tokio::test]
    async fn login_creates_session_and_returns_the_cookie_token() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        create_test_owner(&db).await;
        let config = test_config(dir.path());

        let (session, full_token) = login(
            &db,
            &config,
            "admin",
            "correct horse battery",
            None,
            "Unknown",
        )
        .await
        .unwrap();
        assert_eq!(session.username, "admin");
        assert_eq!(session.role, "owner");
        assert!(!session.csrf_token.is_empty());
        assert!(full_token.starts_with(SESSION_TOKEN_PREFIX));

        // The presented token authenticates; a tampered secret does not.
        assert_eq!(
            authenticate_token(&db, &config, &full_token)
                .await
                .unwrap()
                .username,
            "admin"
        );
        let tampered = format!(
            "{}x{}",
            &full_token[..full_token.len() - 1],
            if full_token.ends_with('0') { "1" } else { "0" }
        );
        assert_eq!(
            authenticate_token(&db, &config, &tampered)
                .await
                .unwrap_err(),
            SessionError::Invalid
        );
    }

    #[tokio::test]
    async fn login_rejects_wrong_password_and_disabled_user() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        create_test_owner(&db).await;
        let config = test_config(dir.path());

        assert!(matches!(
            login(&db, &config, "admin", "wrong password", None, "Unknown")
                .await
                .unwrap_err(),
            LoginError::InvalidCredentials
        ));
        assert!(matches!(
            login(
                &db,
                &config,
                "ghost",
                "correct horse battery",
                None,
                "Unknown"
            )
            .await
            .unwrap_err(),
            LoginError::InvalidCredentials
        ));

        sqlx::query(
            "UPDATE users SET disabled_at = '2026-01-01T00:00:00Z' WHERE username = 'admin'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert!(matches!(
            login(
                &db,
                &config,
                "admin",
                "correct horse battery",
                None,
                "Unknown"
            )
            .await
            .unwrap_err(),
            LoginError::UserDisabled
        ));
    }

    #[tokio::test]
    async fn login_rotates_presented_session() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        create_test_owner(&db).await;
        let config = test_config(dir.path());

        let (first, first_token) = login(
            &db,
            &config,
            "admin",
            "correct horse battery",
            None,
            "Unknown",
        )
        .await
        .unwrap();
        assert_eq!(
            authenticate_token(&db, &config, &first_token)
                .await
                .unwrap()
                .session_id,
            first.session_id
        );

        // Logging in again with the presented session rotates it: the old
        // session is revoked and the new one is a different id.
        let (second, second_token) = login(
            &db,
            &config,
            "admin",
            "correct horse battery",
            Some(&first_token),
            "Unknown",
        )
        .await
        .unwrap();
        assert_ne!(first.session_id, second.session_id);
        assert_eq!(
            authenticate_token(&db, &config, &first_token)
                .await
                .unwrap_err(),
            SessionError::Invalid,
            "rotated session must be revoked"
        );
        assert_eq!(
            authenticate_token(&db, &config, &second_token)
                .await
                .unwrap()
                .session_id,
            second.session_id
        );

        // A garbage presented token does not block login.
        let (third, third_token) = login(
            &db,
            &config,
            "admin",
            "correct horse battery",
            Some("garbage"),
            "Unknown",
        )
        .await
        .unwrap();
        assert_eq!(
            authenticate_token(&db, &config, &third_token)
                .await
                .unwrap()
                .session_id,
            third.session_id
        );
    }

    #[tokio::test]
    async fn expired_session_is_rejected() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        create_test_owner(&db).await;
        let config = test_config(dir.path());

        // Insert a session row directly with a past expiry; authenticate
        // with a matching token.
        let (token_id, full_token) = new_session_token();
        let digest = config.pepper.hmac_digest(full_token.as_bytes());
        let csrf = derive_csrf_token(&config, &full_token);
        let csrf_digest = config.pepper.hmac_digest(csrf.as_bytes());
        let now = format_rfc3339(now_utc());
        let past = "2020-01-01T00:00:00Z";
        sqlx::query(
            "INSERT INTO sessions (session_id, user_id, token_digest, csrf_token_digest, created_at, last_seen_at, expires_at, revoked_at) VALUES (?, (SELECT user_id FROM users WHERE username = 'admin'), ?, ?, ?, ?, ?, NULL)",
        )
        .bind(&token_id)
        .bind(digest.to_vec())
        .bind(csrf_digest.to_vec())
        .bind(&now)
        .bind(&now)
        .bind(past)
        .execute(db.pool())
        .await
        .unwrap();

        let before_generation = authorization_generation(&db).await.unwrap();

        assert_eq!(
            authenticate_token(&db, &config, &full_token)
                .await
                .unwrap_err(),
            SessionError::Expired
        );
        assert_eq!(
            authorization_generation(&db).await.unwrap(),
            before_generation + 1
        );
        assert!(
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT revoked_at FROM sessions WHERE session_id = ?",
            )
            .bind(&token_id)
            .fetch_one(db.pool())
            .await
            .unwrap()
            .is_some()
        );
    }

    #[tokio::test]
    async fn disabled_user_session_is_rejected() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        create_test_owner(&db).await;
        let config = test_config(dir.path());

        let (token_id, full_token) = new_session_token();
        let digest = config.pepper.hmac_digest(full_token.as_bytes());
        let csrf = derive_csrf_token(&config, &full_token);
        let csrf_digest = config.pepper.hmac_digest(csrf.as_bytes());
        let now = format_rfc3339(now_utc());
        sqlx::query(
            "INSERT INTO sessions (session_id, user_id, token_digest, csrf_token_digest, created_at, last_seen_at, expires_at, revoked_at) VALUES (?, (SELECT user_id FROM users WHERE username = 'admin'), ?, ?, ?, ?, '2099-01-01T00:00:00Z', NULL)",
        )
        .bind(&token_id)
        .bind(digest.to_vec())
        .bind(csrf_digest.to_vec())
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();

        sqlx::query(
            "UPDATE users SET disabled_at = '2026-01-01T00:00:00Z' WHERE username = 'admin'",
        )
        .execute(db.pool())
        .await
        .unwrap();

        assert_eq!(
            authenticate_token(&db, &config, &full_token)
                .await
                .unwrap_err(),
            SessionError::UserDisabled
        );
    }
}
