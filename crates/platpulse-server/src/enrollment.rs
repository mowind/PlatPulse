//! Agent Enrollment (design §4.5, §12.5, §12.6).
//!
//! An Enrollment Token is a short-lived, single-use secret that can only be
//! exchanged once for a stable Agent identity (UUID), the Agent Epoch, and
//! an Agent Credential. The Server stores only pepper-keyed HMAC digests:
//! neither the enrollment token nor the credential plaintext ever touches
//! the database, argv, URLs, or error messages.
//!
//! Token formats follow §12.5: `pp_enroll_<token_id>_<secret>` for
//! enrollment and `pp_agent_<credential_id>_<secret>` for Agent
//! Credentials. Lookup happens by the non-sensitive id; the digest is
//! compared in constant time. A consumed, expired, revoked, or unknown
//! token is rejected without creating a second Agent identity.

use std::time::Duration;

use rand_core::{OsRng, RngCore};
use sqlx::FromRow;
use subtle::ConstantTimeEq;
use thiserror::Error;

use crate::auth::{format_rfc3339, insert_audit_event, now_utc, parse_rfc3339};
use crate::database::ServerDatabase;
use crate::secrets::Pepper;

/// Enrollment Token prefix from design §12.5.
pub const ENROLLMENT_TOKEN_PREFIX: &str = "pp_enroll_";

/// Agent Credential prefix from design §12.5.
pub const AGENT_CREDENTIAL_PREFIX: &str = "pp_agent_";

/// Secret half of an Agent Credential: 256 bits, hex-encoded.
pub const AGENT_CREDENTIAL_SECRET_BYTES: usize = 32;

/// Minimum lifetime of an Enrollment Token (design: short-lived).
pub const ENROLLMENT_TOKEN_MIN_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// Maximum lifetime of an Enrollment Token.
pub const ENROLLMENT_TOKEN_MAX_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Default lifetime of an Enrollment Token.
pub const ENROLLMENT_TOKEN_DEFAULT_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// Failed enrollment attempts per client inside the window.
pub const ENROLL_MAX_ATTEMPTS: u32 = 10;

/// Fixed window for the enrollment rate limiter (design §19.4: Enrollment
/// has an independent rate limit, like login/Recovery/AgentReport).
pub const ENROLL_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// The Agent identity issued by a successful Enrollment. `credential` is
/// the full `pp_agent_…` token, shown to the Agent exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrolledAgent {
    pub agent_id: String,
    pub agent_epoch: i64,
    pub credential: String,
}

/// Why an Enrollment Token did not enroll an Agent.
#[derive(Debug, Error)]
pub enum EnrollmentError {
    #[error("enrollment token is invalid or unknown")]
    Invalid,
    #[error("enrollment token has expired")]
    Expired,
    #[error("enrollment token has already been consumed")]
    Consumed,
    #[error("{0}")]
    InvalidLifetime(&'static str),
    #[error("failed to load the pepper file: {0}")]
    Pepper(#[from] crate::secrets::PepperError),
    #[error("server database initialization failed: {0}")]
    ServerDatabase(#[from] crate::database::ServerDatabaseError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl PartialEq for EnrollmentError {
    /// Compares the token-outcome and lifetime variants (used by tests);
    /// error-bearing variants always compare unequal.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Invalid, Self::Invalid)
            | (Self::Expired, Self::Expired)
            | (Self::Consumed, Self::Consumed) => true,
            (Self::InvalidLifetime(a), Self::InvalidLifetime(b)) => a == b,
            _ => false,
        }
    }
}

/// Identity of an authenticated Agent Credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAuthInfo {
    pub agent_id: String,
    pub credential_id: String,
}

/// Split a `pp_<kind>_<token_id>_<secret>` token into its non-sensitive id
/// and the full value used for digest comparison.
fn split_kind_token<'a>(token: &'a str, prefix: &str) -> Option<(&'a str, &'a str)> {
    let rest = token.strip_prefix(prefix)?;
    let (token_id, secret) = rest.split_once('_')?;
    if token_id.is_empty() || secret.is_empty() {
        return None;
    }
    Some((token_id, token))
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    crate::secrets::encode_hex(&buf)
}

/// Build a fresh `pp_enroll_<token_id>_<secret>` token. The caller receives
/// the plaintext once; the Server only ever stores the digest.
pub fn new_enrollment_token() -> (String, String) {
    let token_id = uuid::Uuid::new_v4().to_string();
    let secret = random_hex(AGENT_CREDENTIAL_SECRET_BYTES);
    (
        token_id.clone(),
        format!("{ENROLLMENT_TOKEN_PREFIX}{token_id}_{secret}"),
    )
}

/// Build a fresh `pp_agent_<credential_id>_<secret>` credential token with
/// a 256-bit secret half.
fn new_agent_credential() -> (String, String) {
    let credential_id = uuid::Uuid::new_v4().to_string();
    let secret = random_hex(AGENT_CREDENTIAL_SECRET_BYTES);
    (
        credential_id.clone(),
        format!("{AGENT_CREDENTIAL_PREFIX}{credential_id}_{secret}"),
    )
}

/// Create a single-use Enrollment Token with the given lifetime and store
/// only its pepper-keyed digest. Returns `(token_id, full_token)`; the
/// full token is printed by the CLI exactly once.
pub async fn create_enrollment_token(
    db: &ServerDatabase,
    pepper: &Pepper,
    lifetime: Duration,
) -> Result<(String, String), sqlx::Error> {
    let (token_id, full_token) = new_enrollment_token();
    let digest = pepper.hmac_digest(full_token.as_bytes());
    let now = now_utc();
    let created_at = format_rfc3339(now);
    let expires_at = format_rfc3339(
        now + time::Duration::try_from(lifetime)
            .expect("enrollment token lifetime fits in time::Duration"),
    );

    let mut transaction = db.pool().begin().await?;
    sqlx::query(
        "INSERT INTO enrollment_tokens (token_id, token_digest, created_at, expires_at, consumed_at, consumed_agent_id, revoked_at) VALUES (?, ?, ?, ?, NULL, NULL, NULL)",
    )
    .bind(&token_id)
    .bind(digest.to_vec())
    .bind(&created_at)
    .bind(&expires_at)
    .execute(&mut *transaction)
    .await?;
    let after = serde_json::json!({ "expires_at": expires_at });
    insert_audit_event(
        &mut *transaction,
        None,
        "enrollment_token_created",
        "enrollment_token",
        &token_id,
        Some(&after),
    )
    .await?;
    transaction.commit().await?;
    Ok((token_id, full_token))
}

/// Exchange an Enrollment Token for a stable Agent identity, its Agent
/// Epoch (1 at Enrollment), and a fresh 256-bit Agent Credential.
///
/// All mutations — token consumption, Agent creation, credential issuance,
/// and the audit row — commit in one transaction, so a repeated, expired,
/// or already-consumed token can never produce a second Agent identity.
pub async fn enroll(
    db: &ServerDatabase,
    pepper: &Pepper,
    token: &str,
) -> Result<EnrolledAgent, EnrollmentError> {
    let Some((token_id, full_token)) = split_kind_token(token, ENROLLMENT_TOKEN_PREFIX) else {
        return Err(EnrollmentError::Invalid);
    };

    let mut transaction = db.pool().begin().await?;
    let row: Option<EnrollmentRow> = sqlx::query_as(
        "SELECT token_digest, expires_at, consumed_at, revoked_at FROM enrollment_tokens WHERE token_id = ?",
    )
    .bind(token_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        return Err(EnrollmentError::Invalid);
    };
    if row.revoked_at.is_some() {
        return Err(EnrollmentError::Invalid);
    }
    if row.consumed_at.is_some() {
        return Err(EnrollmentError::Consumed);
    }
    let now = now_utc();
    // Fail closed: an unparseable stored expiry is treated as expired, and
    // the token is refused at the expiry instant itself (`now >= expires_at`)
    // rather than one nanosecond after it.
    let expires_at = match parse_rfc3339(&row.expires_at) {
        Some(timestamp) => timestamp,
        None => return Err(EnrollmentError::Expired),
    };
    if now >= expires_at {
        return Err(EnrollmentError::Expired);
    }
    let expected_digest = pepper.hmac_digest(full_token.as_bytes());
    if !bool::from(expected_digest.ct_eq(&row.token_digest)) {
        return Err(EnrollmentError::Invalid);
    }

    // A valid, unconsumed token: issue the identity in the same
    // transaction that consumes it.
    let agent_id = uuid::Uuid::new_v4().to_string();
    let agent_epoch: i64 = 1;
    let now_text = format_rfc3339(now);
    sqlx::query(
        "INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES (?, ?, NULL, NULL, NULL, ?, ?)",
    )
    .bind(&agent_id)
    .bind(agent_epoch)
    .bind(&now_text)
    .bind(&now_text)
    .execute(&mut *transaction)
    .await?;

    let (credential_id, credential) = new_agent_credential();
    let credential_digest = pepper.hmac_digest(credential.as_bytes());
    sqlx::query(
        "INSERT INTO agent_credentials (credential_id, agent_id, credential_digest, created_at, revoked_at) VALUES (?, ?, ?, ?, NULL)",
    )
    .bind(&credential_id)
    .bind(&agent_id)
    .bind(credential_digest.to_vec())
    .bind(&now_text)
    .execute(&mut *transaction)
    .await?;

    let consumed = sqlx::query(
        "UPDATE enrollment_tokens SET consumed_at = ?, consumed_agent_id = ? WHERE token_id = ? AND consumed_at IS NULL",
    )
    .bind(&now_text)
    .bind(&agent_id)
    .bind(token_id)
    .execute(&mut *transaction)
    .await?;
    if consumed.rows_affected() == 0 {
        return Err(EnrollmentError::Consumed);
    }

    let after = serde_json::json!({ "agent_id": agent_id, "agent_epoch": agent_epoch });
    insert_audit_event(
        &mut *transaction,
        None,
        "agent_enrolled",
        "agent",
        &agent_id,
        Some(&after),
    )
    .await?;

    transaction.commit().await?;
    Ok(EnrolledAgent {
        agent_id,
        agent_epoch,
        credential,
    })
}

#[derive(Debug, FromRow)]
struct EnrollmentRow {
    token_digest: Vec<u8>,
    expires_at: String,
    consumed_at: Option<String>,
    revoked_at: Option<String>,
}

#[derive(Debug, FromRow)]
struct CredentialRow {
    credential_digest: Vec<u8>,
    revoked_at: Option<String>,
    agent_id: String,
}

/// Authenticate a presented Agent Credential (`pp_agent_…`). Lookup by the
/// non-sensitive credential id, then constant-time digest comparison;
/// revoked, unknown, and malformed tokens all read as `None`.
pub async fn authenticate_agent_credential(
    db: &ServerDatabase,
    pepper: &Pepper,
    token: &str,
) -> Result<Option<AgentAuthInfo>, sqlx::Error> {
    let Some((credential_id, full_token)) = split_kind_token(token, AGENT_CREDENTIAL_PREFIX) else {
        return Ok(None);
    };
    let Some(row) = sqlx::query_as::<_, CredentialRow>(
        "SELECT c.credential_digest, c.revoked_at, a.agent_id
         FROM agent_credentials c JOIN agents a ON a.agent_id = c.agent_id
         WHERE c.credential_id = ?",
    )
    .bind(credential_id)
    .fetch_optional(db.pool())
    .await?
    else {
        return Ok(None);
    };
    if row.revoked_at.is_some() {
        return Ok(None);
    }
    let expected_digest = pepper.hmac_digest(full_token.as_bytes());
    if !bool::from(expected_digest.ct_eq(&row.credential_digest)) {
        return Ok(None);
    }
    Ok(Some(AgentAuthInfo {
        agent_id: row.agent_id,
        credential_id: credential_id.to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use tempfile::tempdir;

    use crate::auth::format_rfc3339;
    use crate::database::{ServerDatabaseConfig, initialize};
    use crate::secrets::{create_pepper_file, load_pepper_file};

    use super::*;

    async fn test_db(dir: &Path) -> ServerDatabase {
        initialize(ServerDatabaseConfig::new(dir.join("server.db")))
            .await
            .unwrap()
    }

    fn test_pepper(dir: &Path) -> Pepper {
        let path = dir.join("server-pepper");
        create_pepper_file(&path).unwrap();
        load_pepper_file(&path).unwrap()
    }

    #[test]
    fn enrollment_tokens_are_parseable_and_high_entropy() {
        let (token_id, full) = new_enrollment_token();
        assert_eq!(full.len(), ENROLLMENT_TOKEN_PREFIX.len() + 36 + 1 + 64);
        let (parsed_id, _) = split_kind_token(&full, ENROLLMENT_TOKEN_PREFIX).unwrap();
        assert_eq!(parsed_id, token_id);
        let (_, full2) = new_enrollment_token();
        assert_ne!(full, full2);

        assert!(split_kind_token("garbage", ENROLLMENT_TOKEN_PREFIX).is_none());
        assert!(split_kind_token("pp_enroll_", ENROLLMENT_TOKEN_PREFIX).is_none());
        assert!(split_kind_token("pp_enroll_id", ENROLLMENT_TOKEN_PREFIX).is_none());
        assert!(
            split_kind_token(&full, AGENT_CREDENTIAL_PREFIX).is_none(),
            "kind prefixes must not cross"
        );
    }

    #[tokio::test]
    async fn enrollment_issues_one_identity_and_consumes_the_token() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        let pepper = test_pepper(dir.path());

        let (token_id, full_token) =
            create_enrollment_token(&db, &pepper, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
                .await
                .unwrap();
        let enrolled = enroll(&db, &pepper, &full_token).await.unwrap();
        assert_eq!(enrolled.agent_epoch, 1);
        assert!(!enrolled.agent_id.is_empty());
        assert!(enrolled.credential.starts_with(AGENT_CREDENTIAL_PREFIX));
        assert_eq!(
            enrolled.credential.len(),
            AGENT_CREDENTIAL_PREFIX.len() + 36 + 1 + 64,
            "credential secret must be 256-bit"
        );

        // Exactly one Agent exists; the token is consumed and no second
        // identity can be minted from it.
        let agents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(agents, 1);
        let consumed: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT consumed_at, consumed_agent_id FROM enrollment_tokens WHERE token_id = ?",
        )
        .bind(&token_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(consumed.0.is_some());
        assert_eq!(consumed.1.as_deref(), Some(enrolled.agent_id.as_str()));

        // The issued credential authenticates.
        let auth = authenticate_agent_credential(&db, &pepper, &enrolled.credential)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(auth.agent_id, enrolled.agent_id);

        // The plaintext never reaches the database: only digests are stored.
        let stored: Vec<u8> = sqlx::query_scalar(
            "SELECT credential_digest FROM agent_credentials WHERE credential_id = (SELECT credential_id FROM agent_credentials LIMIT 1)",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let credential_digest = pepper.hmac_digest(enrolled.credential.as_bytes());
        assert_eq!(stored, credential_digest.to_vec());
        let stored_tokens: Vec<String> =
            sqlx::query_scalar("SELECT hex(token_digest) FROM enrollment_tokens")
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert!(stored_tokens.iter().all(|digest| digest != &full_token));
    }

    #[tokio::test]
    async fn repeated_expired_and_unknown_tokens_never_create_a_second_agent() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        let pepper = test_pepper(dir.path());

        let (_, full_token) =
            create_enrollment_token(&db, &pepper, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
                .await
                .unwrap();
        enroll(&db, &pepper, &full_token).await.unwrap();

        // Repeat with the same token: consumed, no second identity.
        assert_eq!(
            enroll(&db, &pepper, &full_token).await.unwrap_err(),
            EnrollmentError::Consumed
        );

        // Unknown / malformed / wrong-secret tokens: invalid.
        assert_eq!(
            enroll(&db, &pepper, "pp_enroll_unknown_abc")
                .await
                .unwrap_err(),
            EnrollmentError::Invalid
        );
        let (_, other_token) =
            create_enrollment_token(&db, &pepper, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
                .await
                .unwrap();
        let tampered = format!(
            "{}x{}",
            &other_token[..other_token.len() - 1],
            if other_token.ends_with('0') { "1" } else { "0" }
        );
        assert_eq!(
            enroll(&db, &pepper, &tampered).await.unwrap_err(),
            EnrollmentError::Invalid
        );

        // Expired: insert a row directly with a past expiry.
        let (token_id, expired_token) = new_enrollment_token();
        let digest = pepper.hmac_digest(expired_token.as_bytes());
        let now = format_rfc3339(now_utc());
        sqlx::query(
            "INSERT INTO enrollment_tokens (token_id, token_digest, created_at, expires_at, consumed_at, consumed_agent_id, revoked_at) VALUES (?, ?, ?, '2020-01-01T00:00:00Z', NULL, NULL, NULL)",
        )
        .bind(&token_id)
        .bind(digest.to_vec())
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();
        assert_eq!(
            enroll(&db, &pepper, &expired_token).await.unwrap_err(),
            EnrollmentError::Expired
        );

        let agents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(agents, 1, "no failed attempt may mint an Agent identity");
    }

    #[tokio::test]
    async fn credential_authentication_rejects_revoked_and_tampered_tokens() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        let pepper = test_pepper(dir.path());

        let (_, full_token) =
            create_enrollment_token(&db, &pepper, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
                .await
                .unwrap();
        let enrolled = enroll(&db, &pepper, &full_token).await.unwrap();

        let tampered = format!(
            "{}x{}",
            &enrolled.credential[..enrolled.credential.len() - 1],
            if enrolled.credential.ends_with('0') {
                "1"
            } else {
                "0"
            }
        );
        assert!(
            authenticate_agent_credential(&db, &pepper, &tampered)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            authenticate_agent_credential(&db, &pepper, "pp_agent_unknown_x")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            authenticate_agent_credential(&db, &pepper, "not-a-token")
                .await
                .unwrap()
                .is_none()
        );

        // Revoking the credential takes effect immediately.
        sqlx::query(
            "UPDATE agent_credentials SET revoked_at = '2026-01-01T00:00:00Z' WHERE agent_id = ?",
        )
        .bind(&enrolled.agent_id)
        .execute(db.pool())
        .await
        .unwrap();
        assert!(
            authenticate_agent_credential(&db, &pepper, &enrolled.credential)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn token_lifetime_bounds_are_stable() {
        assert_eq!(
            ENROLLMENT_TOKEN_DEFAULT_LIFETIME,
            Duration::from_secs(24 * 3600)
        );
        assert!(ENROLLMENT_TOKEN_MIN_LIFETIME <= ENROLLMENT_TOKEN_DEFAULT_LIFETIME);
        assert!(ENROLLMENT_TOKEN_DEFAULT_LIFETIME <= ENROLLMENT_TOKEN_MAX_LIFETIME);
    }
}
