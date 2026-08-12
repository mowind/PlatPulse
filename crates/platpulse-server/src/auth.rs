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

/// Why a presented session token was not accepted.
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

/// Identity mutation errors surfaced by CLI commands.
#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("username '{0}' is already in use")]
    UsernameTaken(String),
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

fn parse_rfc3339(value: &str) -> Option<OffsetDateTime> {
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

/// Whether at least one enabled Owner exists (design §12.2).
pub async fn has_owner(db: &ServerDatabase) -> Result<bool, sqlx::Error> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE role = 'owner' AND disabled_at IS NULL",
    )
    .fetch_one(db.pool())
    .await?;
    Ok(count > 0)
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
    let created_at = format_rfc3339(now_utc());
    sqlx::query(
        "INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, before_json, after_json, created_at) VALUES (?, ?, ?, ?, NULL, ?, ?)",
    )
    .bind(actor_user_id)
    .bind(event_kind)
    .bind(target_kind)
    .bind(target_id)
    .bind(after_json.map(|value| value.to_string()))
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

/// Create the first Owner (or an additional Owner) from the CLI. The
/// password is hashed by the caller; this function owns the transaction so
/// the user row and its audit event cannot diverge.
pub async fn create_owner(
    db: &ServerDatabase,
    username: &str,
    password_hash: &str,
) -> Result<(), IdentityError> {
    let user_id = uuid::Uuid::new_v4().to_string();
    let now = format_rfc3339(now_utc());
    let mut transaction = db.pool().begin().await?;

    let insert = sqlx::query(
        "INSERT INTO users (user_id, username, role, password_hash, disabled_at, created_at, updated_at) VALUES (?, ?, 'owner', ?, NULL, ?, ?)",
    )
    .bind(&user_id)
    .bind(username)
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

    let after = json!({ "username": username, "role": "owner" });
    insert_audit_event(
        &mut *transaction,
        None,
        "owner_created",
        "user",
        username,
        Some(&after),
    )
    .await?;

    transaction.commit().await?;
    Ok(())
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
        "INSERT INTO sessions (session_id, user_id, token_digest, csrf_token_digest, created_at, last_seen_at, expires_at, revoked_at) VALUES (?, ?, ?, ?, ?, ?, ?, NULL)",
    )
    .bind(&token_id)
    .bind(user_id)
    .bind(token_digest.to_vec())
    .bind(csrf_digest.to_vec())
    .bind(&created_at)
    .bind(&created_at)
    .bind(&expires_at)
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
    if now > expires_at {
        return Err(SessionError::Expired);
    }
    let idle_cutoff = last_seen_at
        + time::Duration::try_from(config.idle_timeout)
            .expect("idle timeout fits in time::Duration");
    if now > idle_cutoff {
        return Err(SessionError::Expired);
    }

    let expected_digest = config.pepper.hmac_digest(full_token.as_bytes());
    if !bool::from(expected_digest.ct_eq(&row.token_digest)) {
        return Err(SessionError::Invalid);
    }
    let csrf_token = derive_csrf_token(config, full_token);
    let expected_csrf_digest = config.pepper.hmac_digest(csrf_token.as_bytes());
    if !bool::from(expected_csrf_digest.ct_eq(&row.csrf_token_digest)) {
        return Err(SessionError::Invalid);
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
    }

    let (session, full_token) = insert_session(
        &mut *transaction,
        config,
        &user.user_id,
        &user.username,
        &user.role,
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
    let result = sqlx::query(
        "UPDATE sessions SET revoked_at = ? WHERE session_id = ? AND revoked_at IS NULL",
    )
    .bind(format_rfc3339(now_utc()))
    .bind(session_id)
    .execute(db.pool())
    .await?;
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
    async fn login_creates_session_and_returns_the_cookie_token() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        create_test_owner(&db).await;
        let config = test_config(dir.path());

        let (session, full_token) = login(&db, &config, "admin", "correct horse battery", None)
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
            login(&db, &config, "admin", "wrong password", None)
                .await
                .unwrap_err(),
            LoginError::InvalidCredentials
        ));
        assert!(matches!(
            login(&db, &config, "ghost", "correct horse battery", None)
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
            login(&db, &config, "admin", "correct horse battery", None)
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

        let (first, first_token) = login(&db, &config, "admin", "correct horse battery", None)
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

        assert_eq!(
            authenticate_token(&db, &config, &full_token)
                .await
                .unwrap_err(),
            SessionError::Expired
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
