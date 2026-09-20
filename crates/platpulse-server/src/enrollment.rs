//! Agent Enrollment, Recovery, and credential lifecycle (design §4.5,
//! §12.5, §12.6).
//!
//! An Enrollment Token is a short-lived, single-use secret that can only be
//! exchanged once for a stable Agent identity (UUID), the Agent Epoch, and
//! an Agent Credential. A Recovery Token is bound to an existing Agent and
//! exchanges once for an Epoch advance plus a fresh credential without
//! creating a duplicate Agent. Rotation issues a new credential while the
//! previous one stays valid through an explicit overlap window (or is
//! revoked immediately); revocation takes effect immediately. The Server
//! stores only pepper-keyed HMAC digests: neither the one-time tokens nor
//! the credential plaintext ever touches the database, argv, URLs, or error
//! messages.
//!
//! Token formats follow §12.5: `pp_enroll_<token_id>_<secret>`,
//! `pp_recover_<token_id>_<secret>` for recovery, and
//! `pp_agent_<credential_id>_<secret>` for Agent Credentials. Lookup
//! happens by the non-sensitive id; the digest is compared in constant
//! time. A consumed, expired, revoked, or unknown token is rejected without
//! creating a second Agent identity.

use std::time::Duration;

use rand_core::{OsRng, RngCore};
use sqlx::FromRow;
use subtle::ConstantTimeEq;
use thiserror::Error;

use crate::auth::{
    format_rfc3339, insert_audit_change, insert_audit_event, now_utc, parse_rfc3339,
};
use crate::database::ServerDatabase;
use crate::secrets::Pepper;

/// Enrollment Token prefix from design §12.5.
pub const ENROLLMENT_TOKEN_PREFIX: &str = "pp_enroll_";

/// Recovery Token prefix from design §12.5.
pub const RECOVERY_TOKEN_PREFIX: &str = "pp_recover_";

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

/// Minimum lifetime of a Recovery Token.
pub const RECOVERY_TOKEN_MIN_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// Maximum lifetime of a Recovery Token.
pub const RECOVERY_TOKEN_MAX_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Default lifetime of a Recovery Token.
pub const RECOVERY_TOKEN_DEFAULT_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// Minimum overlap window kept for the previous credential on rotation.
pub const CREDENTIAL_OVERLAP_MIN: Duration = Duration::from_secs(60 * 60);

/// Maximum overlap window kept for the previous credential on rotation.
pub const CREDENTIAL_OVERLAP_MAX: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Default overlap window kept for the previous credential on rotation.
pub const CREDENTIAL_OVERLAP_DEFAULT: Duration = Duration::from_secs(24 * 60 * 60);

/// Failed recovery attempts per client inside the window.
pub const RECOVER_MAX_ATTEMPTS: u32 = 10;

/// Fixed window for the recovery rate limiter (design §19.4: Recovery has
/// an independent rate limit, like login/Enrollment/AgentReport).
pub const RECOVER_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(15 * 60);

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

/// A one-time token issued by the Server for an operator to hand to an
/// Agent. The plaintext `token` is returned exactly once; only the digest
/// is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedToken {
    /// Non-sensitive lookup id (the `pp_<kind>_<id>_<secret>` id half).
    pub token_id: String,
    /// Full one-time token; the operator's only plaintext copy.
    pub token: String,
    /// Server-authoritative expiry instant.
    pub expires_at: String,
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
/// only its pepper-keyed digest. `actor` names the audit principal (`None`
/// means a CLI operator, design §18.2). Returns the record including the
/// full token; the full token is delivered exactly once.
pub async fn create_enrollment_token(
    db: &ServerDatabase,
    pepper: &Pepper,
    actor: Option<&str>,
    lifetime: Duration,
) -> Result<IssuedToken, sqlx::Error> {
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
        actor,
        "enrollment_token_created",
        "enrollment_token",
        &token_id,
        Some(&after),
    )
    .await?;
    transaction.commit().await?;
    Ok(IssuedToken {
        token_id,
        token: full_token,
        expires_at,
    })
}

/// Build a fresh `pp_recover_<token_id>_<secret>` token. The caller
/// receives the plaintext once; the Server only ever stores the digest.
pub fn new_recovery_token() -> (String, String) {
    let token_id = uuid::Uuid::new_v4().to_string();
    let secret = random_hex(AGENT_CREDENTIAL_SECRET_BYTES);
    (
        token_id.clone(),
        format!("{RECOVERY_TOKEN_PREFIX}{token_id}_{secret}"),
    )
}

/// Why a Recovery Token could not be created or exchanged.
#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("recovery token is invalid or unknown")]
    Invalid,
    #[error("recovery token has expired")]
    Expired,
    #[error("recovery token has already been consumed")]
    Consumed,
    #[error("{0}")]
    InvalidLifetime(&'static str),
    #[error("agent not found")]
    AgentNotFound,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl PartialEq for RecoveryError {
    /// Compares the token-outcome and lifetime variants (used by tests);
    /// error-bearing variants always compare unequal.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Invalid, Self::Invalid)
            | (Self::Expired, Self::Expired)
            | (Self::Consumed, Self::Consumed)
            | (Self::AgentNotFound, Self::AgentNotFound) => true,
            (Self::InvalidLifetime(a), Self::InvalidLifetime(b)) => a == b,
            _ => false,
        }
    }
}

/// Create a single-use Recovery Token for an existing Agent (design §4.5:
/// when credentials are lost, the Owner issues a one-time Recovery Token
/// for that Agent). Only the digest is stored; the full token is delivered
/// exactly once.
pub async fn create_recovery_token(
    db: &ServerDatabase,
    pepper: &Pepper,
    actor: Option<&str>,
    agent_id: &str,
    lifetime: Duration,
) -> Result<IssuedToken, RecoveryError> {
    if lifetime < RECOVERY_TOKEN_MIN_LIFETIME || lifetime > RECOVERY_TOKEN_MAX_LIFETIME {
        return Err(RecoveryError::InvalidLifetime(
            "recovery token lifetime must be 1..=168 hours",
        ));
    }
    let (token_id, full_token) = new_recovery_token();
    let digest = pepper.hmac_digest(full_token.as_bytes());
    let now = now_utc();
    let created_at = format_rfc3339(now);
    let expires_at = format_rfc3339(
        now + time::Duration::try_from(lifetime)
            .expect("recovery token lifetime fits in time::Duration"),
    );

    let mut transaction = db.pool().begin().await?;
    let agent_exists =
        sqlx::query_scalar::<_, String>("SELECT agent_id FROM agents WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_optional(&mut *transaction)
            .await?
            .is_some();
    if !agent_exists {
        return Err(RecoveryError::AgentNotFound);
    }
    sqlx::query(
        "INSERT INTO recovery_tokens (token_id, token_digest, agent_id, created_at, expires_at, consumed_at, revoked_at) VALUES (?, ?, ?, ?, ?, NULL, NULL)",
    )
    .bind(&token_id)
    .bind(digest.to_vec())
    .bind(agent_id)
    .bind(&created_at)
    .bind(&expires_at)
    .execute(&mut *transaction)
    .await?;
    // Redacted audit: the token plaintext and digest never enter Audit.
    let after = serde_json::json!({ "agent_id": agent_id, "expires_at": expires_at });
    insert_audit_event(
        &mut *transaction,
        actor,
        "recovery_token_created",
        "agent",
        agent_id,
        Some(&after),
    )
    .await?;
    transaction.commit().await?;
    Ok(IssuedToken {
        token_id,
        token: full_token,
        expires_at,
    })
}

/// The identity issued by a successful Recovery exchange. `credential` is
/// the full `pp_agent_…` token, shown to the recovering Agent exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredAgent {
    pub agent_id: String,
    pub agent_epoch: i64,
    pub credential: String,
}

/// Exchange a single-use Recovery Token for an Epoch advance and a fresh
/// credential on the SAME Agent identity (design §4.5: Recovery rotates the
/// credential and advances the Agent Epoch without creating a duplicate
/// Agent). All prior credentials are revoked in the same transaction; a
/// consumed, expired, revoked, or unknown token never touches the Agent.
pub async fn recover(
    db: &ServerDatabase,
    pepper: &Pepper,
    token: &str,
) -> Result<RecoveredAgent, RecoveryError> {
    let Some((token_id, full_token)) = split_kind_token(token, RECOVERY_TOKEN_PREFIX) else {
        return Err(RecoveryError::Invalid);
    };

    let mut transaction = db.pool().begin().await?;
    let row: Option<RecoveryRow> = sqlx::query_as(
        "SELECT token_digest, agent_id, expires_at, consumed_at, revoked_at FROM recovery_tokens WHERE token_id = ?",
    )
    .bind(token_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        return Err(RecoveryError::Invalid);
    };
    if row.revoked_at.is_some() {
        return Err(RecoveryError::Invalid);
    }
    if row.consumed_at.is_some() {
        return Err(RecoveryError::Consumed);
    }
    let now = now_utc();
    let expires_at = match parse_rfc3339(&row.expires_at) {
        Some(timestamp) => timestamp,
        None => return Err(RecoveryError::Expired),
    };
    if now >= expires_at {
        return Err(RecoveryError::Expired);
    }
    let expected_digest = pepper.hmac_digest(full_token.as_bytes());
    if !bool::from(expected_digest.ct_eq(&row.token_digest)) {
        return Err(RecoveryError::Invalid);
    }

    let epoch: Option<i64> =
        sqlx::query_scalar("SELECT agent_epoch FROM agents WHERE agent_id = ?")
            .bind(&row.agent_id)
            .fetch_optional(&mut *transaction)
            .await?;
    let Some(epoch) = epoch else {
        return Err(RecoveryError::AgentNotFound);
    };
    let new_epoch = epoch + 1;
    let now_text = format_rfc3339(now);
    sqlx::query("UPDATE agents SET agent_epoch = ?, updated_at = ? WHERE agent_id = ?")
        .bind(new_epoch)
        .bind(&now_text)
        .bind(&row.agent_id)
        .execute(&mut *transaction)
        .await?;

    // Recovery means the credential was lost or compromised: every still
    // valid credential is revoked and one fresh credential is issued.
    let revoked = sqlx::query(
        "UPDATE agent_credentials SET revoked_at = ? WHERE agent_id = ? AND revoked_at IS NULL",
    )
    .bind(&now_text)
    .bind(&row.agent_id)
    .execute(&mut *transaction)
    .await?;
    let revoked_count = revoked.rows_affected() as i64;

    let (credential_id, credential) = new_agent_credential();
    let credential_digest = pepper.hmac_digest(credential.as_bytes());
    sqlx::query(
        "INSERT INTO agent_credentials (credential_id, agent_id, credential_digest, created_at, revoked_at, revoke_after) VALUES (?, ?, ?, ?, NULL, NULL)",
    )
    .bind(&credential_id)
    .bind(&row.agent_id)
    .bind(credential_digest.to_vec())
    .bind(&now_text)
    .execute(&mut *transaction)
    .await?;

    let consumed = sqlx::query(
        "UPDATE recovery_tokens SET consumed_at = ? WHERE token_id = ? AND consumed_at IS NULL",
    )
    .bind(&now_text)
    .bind(token_id)
    .execute(&mut *transaction)
    .await?;
    if consumed.rows_affected() == 0 {
        return Err(RecoveryError::Consumed);
    }

    let after = serde_json::json!({
        "agent_epoch": new_epoch,
        "revoked_credential_count": revoked_count,
        "credential_id": credential_id,
    });
    insert_audit_event(
        &mut *transaction,
        None,
        "agent_recovered",
        "agent",
        &row.agent_id,
        Some(&after),
    )
    .await?;

    transaction.commit().await?;
    Ok(RecoveredAgent {
        agent_id: row.agent_id,
        agent_epoch: new_epoch,
        credential,
    })
}

#[derive(Debug, FromRow)]
struct RecoveryRow {
    token_digest: Vec<u8>,
    agent_id: String,
    expires_at: String,
    consumed_at: Option<String>,
    revoked_at: Option<String>,
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
    revoke_after: Option<String>,
    agent_id: String,
}

/// Authenticate a presented Agent Credential (`pp_agent_…`). Lookup by the
/// non-sensitive credential id, then constant-time digest comparison;
/// revoked, overlap-expired, unknown, and malformed tokens all read as
/// `None`.
pub async fn authenticate_agent_credential(
    db: &ServerDatabase,
    pepper: &Pepper,
    token: &str,
) -> Result<Option<AgentAuthInfo>, sqlx::Error> {
    let Some((credential_id, full_token)) = split_kind_token(token, AGENT_CREDENTIAL_PREFIX) else {
        return Ok(None);
    };
    let Some(row) = sqlx::query_as::<_, CredentialRow>(
        "SELECT c.credential_digest, c.revoked_at, c.revoke_after, a.agent_id
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
    // Overlap windows are enforced lazily at authentication time: a
    // rotated-out credential stops working at its `revoke_after` instant
    // even without an explicit revoke. Fail closed on unparseable values.
    if row
        .revoke_after
        .as_deref()
        .is_some_and(|after| parse_rfc3339(after).is_none_or(|deadline| now_utc() >= deadline))
    {
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

/// The result of a credential rotation: the new credential secret (shown
/// once) plus the fate of every previously valid credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotatedCredential {
    pub agent_id: String,
    pub credential_id: String,
    pub credential: String,
    pub created_at: String,
    pub overlap_hours: i64,
    pub revoke_after: Option<String>,
    pub revoked_previous_ids: Vec<String>,
    pub overlap_credential_ids: Vec<String>,
}

/// Why a credential rotation failed.
#[derive(Debug, Error)]
pub enum RotationError {
    #[error("{0}")]
    InvalidLifetime(&'static str),
    #[error("agent not found")]
    AgentNotFound,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl PartialEq for RotationError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AgentNotFound, Self::AgentNotFound) => true,
            (Self::InvalidLifetime(a), Self::InvalidLifetime(b)) => a == b,
            _ => false,
        }
    }
}

/// Rotate an Agent credential (design §12.6: overlap rotation supported;
/// revoke takes effect immediately). A fresh credential is issued; every
/// currently valid credential either stays valid through an explicit
/// overlap window or is revoked immediately, per `revoke_previous`.
/// Rotation never touches the Agent Epoch or creates a duplicate Agent.
pub async fn rotate_agent_credential(
    db: &ServerDatabase,
    pepper: &Pepper,
    actor: Option<&str>,
    agent_id: &str,
    overlap: Duration,
    revoke_previous: bool,
) -> Result<RotatedCredential, RotationError> {
    if overlap < CREDENTIAL_OVERLAP_MIN || overlap > CREDENTIAL_OVERLAP_MAX {
        return Err(RotationError::InvalidLifetime(
            "overlap window must be 1..=168 hours",
        ));
    }
    let now = now_utc();
    let now_text = format_rfc3339(now);
    let mut transaction = db.pool().begin().await?;
    let agent_exists =
        sqlx::query_scalar::<_, String>("SELECT agent_id FROM agents WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_optional(&mut *transaction)
            .await?
            .is_some();
    if !agent_exists {
        return Err(RotationError::AgentNotFound);
    }

    let valid: Vec<String> = sqlx::query_scalar(
        "SELECT credential_id FROM agent_credentials
         WHERE agent_id = ? AND revoked_at IS NULL
           AND (revoke_after IS NULL OR revoke_after > ?)
         ORDER BY created_at, credential_id",
    )
    .bind(agent_id)
    .bind(&now_text)
    .fetch_all(&mut *transaction)
    .await?;
    let mut revoked_previous_ids = Vec::new();
    let mut overlap_credential_ids = Vec::new();
    let overlap_deadline = format_rfc3339(
        now + time::Duration::try_from(overlap).expect("overlap window fits in time::Duration"),
    );
    if revoke_previous {
        for credential_id in &valid {
            sqlx::query(
                "UPDATE agent_credentials SET revoked_at = ? WHERE credential_id = ? AND revoked_at IS NULL",
            )
            .bind(&now_text)
            .bind(credential_id)
            .execute(&mut *transaction)
            .await?;
            revoked_previous_ids.push(credential_id.clone());
        }
    } else {
        for credential_id in &valid {
            sqlx::query(
                "UPDATE agent_credentials SET revoke_after = ? WHERE credential_id = ? AND revoked_at IS NULL",
            )
            .bind(&overlap_deadline)
            .bind(credential_id)
            .execute(&mut *transaction)
            .await?;
            overlap_credential_ids.push(credential_id.clone());
        }
    }

    let (credential_id, credential) = new_agent_credential();
    let credential_digest = pepper.hmac_digest(credential.as_bytes());
    sqlx::query(
        "INSERT INTO agent_credentials (credential_id, agent_id, credential_digest, created_at, revoked_at, revoke_after) VALUES (?, ?, ?, ?, NULL, NULL)",
    )
    .bind(&credential_id)
    .bind(agent_id)
    .bind(credential_digest.to_vec())
    .bind(&now_text)
    .execute(&mut *transaction)
    .await?;

    let overlap_hours = (overlap.as_secs() / 3600) as i64;
    let before = serde_json::json!({
        "active_credential_ids": valid.clone(),
        "revoke_previous": revoke_previous,
    });
    let after = serde_json::json!({
        "credential_id": credential_id,
        "overlap_hours": overlap_hours,
        "revoked_previous_ids": revoked_previous_ids.clone(),
        "overlap_credential_ids": overlap_credential_ids.clone(),
    });
    insert_audit_change(
        &mut *transaction,
        actor,
        "agent_credential_rotated",
        "agent",
        agent_id,
        Some(&before),
        Some(&after),
    )
    .await?;
    transaction.commit().await?;
    Ok(RotatedCredential {
        agent_id: agent_id.to_owned(),
        credential_id,
        credential,
        created_at: now_text,
        overlap_hours,
        revoke_after: (!revoke_previous && !valid.is_empty()).then_some(overlap_deadline),
        revoked_previous_ids,
        overlap_credential_ids,
    })
}

/// The result of an explicit credential revocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokedCredential {
    pub agent_id: String,
    pub credential_id: String,
    pub revoked_at: String,
}

/// Why a credential revocation failed.
#[derive(Debug, Error)]
pub enum RevokeError {
    #[error("agent or credential not found")]
    NotFound,
    #[error("credential is already revoked")]
    AlreadyRevoked,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl PartialEq for RevokeError {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::NotFound, Self::NotFound) | (Self::AlreadyRevoked, Self::AlreadyRevoked)
        )
    }
}

/// Revoke one Agent credential immediately (design §12.6: revoke takes
/// effect immediately). The audit row records only the credential id and
/// instant — never the credential or its digest.
pub async fn revoke_agent_credential(
    db: &ServerDatabase,
    actor: Option<&str>,
    agent_id: &str,
    credential_id: &str,
) -> Result<RevokedCredential, RevokeError> {
    let now_text = format_rfc3339(now_utc());
    let mut transaction = db.pool().begin().await?;
    let updated = sqlx::query(
        "UPDATE agent_credentials SET revoked_at = ? WHERE credential_id = ? AND agent_id = ? AND revoked_at IS NULL",
    )
    .bind(&now_text)
    .bind(credential_id)
    .bind(agent_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() == 0 {
        let exists: Option<String> = sqlx::query_scalar(
            "SELECT credential_id FROM agent_credentials WHERE credential_id = ? AND agent_id = ?",
        )
        .bind(credential_id)
        .bind(agent_id)
        .fetch_optional(&mut *transaction)
        .await?;
        return Err(if exists.is_some() {
            RevokeError::AlreadyRevoked
        } else {
            RevokeError::NotFound
        });
    }
    let before = serde_json::json!({ "revoked_at": null });
    let after = serde_json::json!({
        "credential_id": credential_id,
        "revoked_at": now_text,
    });
    insert_audit_change(
        &mut *transaction,
        actor,
        "agent_credential_revoked",
        "agent",
        agent_id,
        Some(&before),
        Some(&after),
    )
    .await?;
    transaction.commit().await?;
    Ok(RevokedCredential {
        agent_id: agent_id.to_owned(),
        credential_id: credential_id.to_owned(),
        revoked_at: now_text,
    })
}

/// Maximum length of an Agent display name (design §15.2, webui.md §15.1).
/// Matches the Node/Network display-name bound so the UI enforces one rule.
pub const MAX_AGENT_DISPLAY_NAME_LEN: usize = 128;

/// Maximum length of Owner-authored Agent notes.
pub const MAX_AGENT_NOTES_LEN: usize = 2000;

/// Owner-editable Server-owned Agent metadata. Both values are optional: an
/// Agent without a display name is still identified by its stable Agent ID.
/// This is distinct from Agent ID, Host identity, liveness, the Agent Epoch,
/// and local collection configuration, none of which are editable here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMetadata {
    pub display_name: Option<String>,
    pub notes: Option<String>,
}

/// Why an Agent metadata mutation failed.
#[derive(Debug, Error)]
pub enum AgentMetadataError {
    #[error("{0}")]
    InvalidDisplayName(&'static str),
    #[error("{0}")]
    InvalidNotes(&'static str),
    #[error("agent not found")]
    AgentNotFound,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl PartialEq for AgentMetadataError {
    /// Compares the validation and not-found variants (used by tests);
    /// error-bearing variants always compare unequal.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::InvalidDisplayName(a), Self::InvalidDisplayName(b))
            | (Self::InvalidNotes(a), Self::InvalidNotes(b)) => a == b,
            (Self::AgentNotFound, Self::AgentNotFound) => true,
            _ => false,
        }
    }
}

/// Normalize and validate a submitted Agent display name. Surrounding
/// whitespace is trimmed; an empty value clears the name (`None`) so the UI
/// can fall back to the stable Agent ID. Control characters are rejected so
/// the value stays a single visible line.
pub fn normalize_agent_display_name(
    value: Option<&str>,
) -> Result<Option<String>, AgentMetadataError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > MAX_AGENT_DISPLAY_NAME_LEN {
        return Err(AgentMetadataError::InvalidDisplayName(
            "display name must be at most 128 characters",
        ));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(AgentMetadataError::InvalidDisplayName(
            "display name must not contain control characters",
        ));
    }
    Ok(Some(trimmed.to_owned()))
}

/// Normalize and validate submitted Agent notes. Surrounding whitespace is
/// trimmed; an empty value clears the notes (`None`). Newlines and tabs are
/// allowed because notes are multi-line; other control characters are not.
pub fn normalize_agent_notes(value: Option<&str>) -> Result<Option<String>, AgentMetadataError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > MAX_AGENT_NOTES_LEN {
        return Err(AgentMetadataError::InvalidNotes(
            "notes must be at most 2000 characters",
        ));
    }
    if trimmed
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return Err(AgentMetadataError::InvalidNotes(
            "notes must not contain control characters",
        ));
    }
    Ok(Some(trimmed.to_owned()))
}

/// Replace the Server-owned display name and notes of one Agent. Both
/// fields are full-replacement values; `None` or an empty value clears a
/// field. An unchanged submission performs no write and writes no Audit
/// row. The Audit row records the before/after metadata only; it never
/// touches the Agent ID, Epoch, Host identity, liveness, or collection
/// configuration.
pub async fn update_agent_metadata(
    db: &ServerDatabase,
    actor: Option<&str>,
    agent_id: &str,
    display_name: Option<&str>,
    notes: Option<&str>,
) -> Result<AgentMetadata, AgentMetadataError> {
    let next = AgentMetadata {
        display_name: normalize_agent_display_name(display_name)?,
        notes: normalize_agent_notes(notes)?,
    };

    let mut transaction = db.pool().begin().await?;
    let existing = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT display_name, notes FROM agents WHERE agent_id = ?",
    )
    .bind(agent_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((previous_display_name, previous_notes)) = existing else {
        return Err(AgentMetadataError::AgentNotFound);
    };
    if previous_display_name == next.display_name && previous_notes == next.notes {
        // No-op: never write or audit a submission that changes nothing.
        return Ok(next);
    }

    let changed_at = format_rfc3339(now_utc());
    sqlx::query("UPDATE agents SET display_name = ?, notes = ?, updated_at = ? WHERE agent_id = ?")
        .bind(next.display_name.as_deref())
        .bind(next.notes.as_deref())
        .bind(&changed_at)
        .bind(agent_id)
        .execute(&mut *transaction)
        .await?;

    let before = serde_json::json!({
        "display_name": previous_display_name,
        "notes": previous_notes,
    });
    let after = serde_json::json!({
        "display_name": next.display_name,
        "notes": next.notes,
    });
    insert_audit_change(
        &mut *transaction,
        actor,
        "agent_metadata_changed",
        "agent",
        agent_id,
        Some(&before),
        Some(&after),
    )
    .await?;
    transaction.commit().await?;
    Ok(next)
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

    /// A users row so audit events with an Owner actor satisfy the
    /// `audit_events.actor_user_id` foreign key.
    async fn insert_owner_row(db: &ServerDatabase) {
        sqlx::query(
            "INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at)
             VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(db.pool())
        .await
        .unwrap();
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

        let record = create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
            .await
            .unwrap();
        let (token_id, full_token) = (record.token_id, record.token);
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

        let (_, full_token) = {
            let record =
                create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
                    .await
                    .unwrap();
            (record.token_id, record.token)
        };
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
        let other_token = {
            let record =
                create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
                    .await
                    .unwrap();
            record.token
        };
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

        let (_, full_token) = {
            let record =
                create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
                    .await
                    .unwrap();
            (record.token_id, record.token)
        };
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
        assert_eq!(
            RECOVERY_TOKEN_DEFAULT_LIFETIME,
            Duration::from_secs(24 * 3600)
        );
        assert!(RECOVERY_TOKEN_MIN_LIFETIME <= RECOVERY_TOKEN_DEFAULT_LIFETIME);
        assert!(RECOVERY_TOKEN_DEFAULT_LIFETIME <= RECOVERY_TOKEN_MAX_LIFETIME);
        assert!(CREDENTIAL_OVERLAP_MIN <= CREDENTIAL_OVERLAP_DEFAULT);
        assert!(CREDENTIAL_OVERLAP_DEFAULT <= CREDENTIAL_OVERLAP_MAX);
    }

    #[tokio::test]
    async fn recovery_advances_epoch_without_duplicate_agent() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        let pepper = test_pepper(dir.path());
        insert_owner_row(&db).await;

        let record = create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
            .await
            .unwrap();
        let enrolled = enroll(&db, &pepper, &record.token).await.unwrap();
        assert_eq!(enrolled.agent_epoch, 1);

        // The Owner issues a one-time Recovery Token for the existing Agent.
        let issued = create_recovery_token(
            &db,
            &pepper,
            Some("owner"),
            &enrolled.agent_id,
            RECOVERY_TOKEN_DEFAULT_LIFETIME,
        )
        .await
        .unwrap();
        assert!(issued.token.starts_with(RECOVERY_TOKEN_PREFIX));
        assert_eq!(
            issued.token.len(),
            RECOVERY_TOKEN_PREFIX.len() + 36 + 1 + 64
        );

        // Exchange: the same identity advances one Epoch and receives a
        // fresh credential; no second Agent row is created.
        let recovered = recover(&db, &pepper, &issued.token).await.unwrap();
        assert_eq!(recovered.agent_id, enrolled.agent_id);
        assert_eq!(recovered.agent_epoch, 2);
        assert_ne!(recovered.credential, enrolled.credential);
        let agents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(agents, 1, "recovery must not create a duplicate Agent");
        let epoch: i64 = sqlx::query_scalar("SELECT agent_epoch FROM agents WHERE agent_id = ?")
            .bind(&enrolled.agent_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(epoch, 2);

        // The new credential authenticates; every pre-recovery credential
        // is revoked immediately.
        let auth = authenticate_agent_credential(&db, &pepper, &recovered.credential)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(auth.agent_id, enrolled.agent_id);
        assert!(
            authenticate_agent_credential(&db, &pepper, &enrolled.credential)
                .await
                .unwrap()
                .is_none(),
            "recovery must revoke prior credentials"
        );

        // The token is single use.
        assert_eq!(
            recover(&db, &pepper, &issued.token).await.unwrap_err(),
            RecoveryError::Consumed
        );

        // Audit rows are redacted: neither token nor credential plaintext
        // may appear anywhere in the audit bodies.
        let audit: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT event_kind, after_json FROM audit_events")
                .fetch_all(db.pool())
                .await
                .unwrap();
        let kinds: Vec<&str> = audit.iter().map(|(kind, _)| kind.as_str()).collect();
        assert!(kinds.contains(&"recovery_token_created"));
        assert!(kinds.contains(&"agent_recovered"));
        for (_, after) in &audit {
            let body = after.as_deref().unwrap_or("");
            assert!(
                !body.contains(&issued.token) && !body.contains(RECOVERY_TOKEN_PREFIX),
                "audit body must never contain the recovery token"
            );
            assert!(
                !body.contains(&recovered.credential) && !body.contains(&enrolled.credential),
                "audit body must never contain credential plaintext"
            );
        }
    }

    #[tokio::test]
    async fn recovery_rejects_unknown_expired_and_cross_kind_tokens() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        let pepper = test_pepper(dir.path());

        let record = create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
            .await
            .unwrap();
        let enrolled = enroll(&db, &pepper, &record.token).await.unwrap();

        // Unknown / malformed / wrong-prefix tokens: invalid.
        assert_eq!(
            recover(&db, &pepper, "pp_recover_unknown_abc")
                .await
                .unwrap_err(),
            RecoveryError::Invalid
        );
        assert_eq!(
            recover(&db, &pepper, &record.token).await.unwrap_err(),
            RecoveryError::Invalid,
            "an enrollment token must never recover an Agent"
        );

        // Expired token: insert a row directly with a past expiry.
        let (token_id, expired_token) = new_recovery_token();
        let digest = pepper.hmac_digest(expired_token.as_bytes());
        let now = format_rfc3339(now_utc());
        sqlx::query(
            "INSERT INTO recovery_tokens (token_id, token_digest, agent_id, created_at, expires_at, consumed_at, revoked_at) VALUES (?, ?, ?, ?, '2020-01-01T00:00:00Z', NULL, NULL)",
        )
        .bind(&token_id)
        .bind(digest.to_vec())
        .bind(&enrolled.agent_id)
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();
        assert_eq!(
            recover(&db, &pepper, &expired_token).await.unwrap_err(),
            RecoveryError::Expired
        );

        // A tampered secret never advances the Epoch.
        let issued = create_recovery_token(
            &db,
            &pepper,
            None,
            &enrolled.agent_id,
            RECOVERY_TOKEN_DEFAULT_LIFETIME,
        )
        .await
        .unwrap();
        let tampered = format!(
            "{}x{}",
            &issued.token[..issued.token.len() - 1],
            if issued.token.ends_with('0') {
                "1"
            } else {
                "0"
            }
        );
        assert_eq!(
            recover(&db, &pepper, &tampered).await.unwrap_err(),
            RecoveryError::Invalid
        );
        let epoch: i64 = sqlx::query_scalar("SELECT agent_epoch FROM agents WHERE agent_id = ?")
            .bind(&enrolled.agent_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(epoch, 1, "failed exchanges must not advance the Epoch");

        // Unknown Agent: token creation is refused.
        assert_eq!(
            create_recovery_token(
                &db,
                &pepper,
                None,
                "no-such-agent",
                RECOVERY_TOKEN_DEFAULT_LIFETIME,
            )
            .await
            .unwrap_err(),
            RecoveryError::AgentNotFound
        );
        assert_eq!(
            create_recovery_token(
                &db,
                &pepper,
                None,
                &enrolled.agent_id,
                Duration::from_secs(30),
            )
            .await
            .unwrap_err(),
            RecoveryError::InvalidLifetime("recovery token lifetime must be 1..=168 hours")
        );
    }

    #[tokio::test]
    async fn rotation_keeps_overlap_and_revoke_previous_is_immediate() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        let pepper = test_pepper(dir.path());
        insert_owner_row(&db).await;

        let record = create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
            .await
            .unwrap();
        let enrolled = enroll(&db, &pepper, &record.token).await.unwrap();

        // Rotation with an overlap window: the old credential stays valid
        // until the deadline, the new one is immediately active.
        let rotated = rotate_agent_credential(
            &db,
            &pepper,
            Some("owner"),
            &enrolled.agent_id,
            CREDENTIAL_OVERLAP_DEFAULT,
            false,
        )
        .await
        .unwrap();
        assert_eq!(rotated.agent_id, enrolled.agent_id);
        assert_eq!(rotated.overlap_hours, 24);
        assert_eq!(rotated.revoked_previous_ids, Vec::<String>::new());
        assert_eq!(rotated.overlap_credential_ids.len(), 1);
        assert!(rotated.revoke_after.is_some());
        assert!(rotated.credential.starts_with(AGENT_CREDENTIAL_PREFIX));
        assert!(
            authenticate_agent_credential(&db, &pepper, &rotated.credential)
                .await
                .unwrap()
                .is_some(),
            "the rotated-in credential authenticates immediately"
        );
        assert!(
            authenticate_agent_credential(&db, &pepper, &enrolled.credential)
                .await
                .unwrap()
                .is_some(),
            "the previous credential stays valid through the overlap window"
        );
        // The Epoch is untouched by rotation.
        let epoch: i64 = sqlx::query_scalar("SELECT agent_epoch FROM agents WHERE agent_id = ?")
            .bind(&enrolled.agent_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(epoch, 1);

        // Overlap enforcement is lazy: past the deadline the old credential
        // stops working without any explicit revoke.
        sqlx::query("UPDATE agent_credentials SET revoke_after = '2020-01-01T00:00:00Z' WHERE credential_id = ?")
            .bind(&rotated.overlap_credential_ids[0])
            .execute(db.pool())
            .await
            .unwrap();
        assert!(
            authenticate_agent_credential(&db, &pepper, &enrolled.credential)
                .await
                .unwrap()
                .is_none(),
            "an overlap-expired credential must be rejected at authentication"
        );

        // Explicit old-credential revocation: previous credentials are
        // revoked in the rotation transaction and stop working immediately.
        let rotated = rotate_agent_credential(
            &db,
            &pepper,
            Some("owner"),
            &enrolled.agent_id,
            CREDENTIAL_OVERLAP_DEFAULT,
            true,
        )
        .await
        .unwrap();
        assert_eq!(rotated.revoked_previous_ids.len(), 1);
        assert!(rotated.overlap_credential_ids.is_empty());
        assert!(rotated.revoke_after.is_none());
        assert!(
            authenticate_agent_credential(&db, &pepper, &rotated.credential)
                .await
                .unwrap()
                .is_some()
        );

        // Unknown Agent and bad overlap are typed.
        assert_eq!(
            rotate_agent_credential(
                &db,
                &pepper,
                None,
                "no-such-agent",
                CREDENTIAL_OVERLAP_DEFAULT,
                false,
            )
            .await
            .unwrap_err(),
            RotationError::AgentNotFound
        );
        assert_eq!(
            rotate_agent_credential(
                &db,
                &pepper,
                None,
                &enrolled.agent_id,
                Duration::from_secs(3600 * 200),
                false,
            )
            .await
            .unwrap_err(),
            RotationError::InvalidLifetime("overlap window must be 1..=168 hours")
        );

        // Audit carries only ids and instants, never secret material.
        let audit: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT before_json, after_json FROM audit_events WHERE event_kind = 'agent_credential_rotated'",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        for (before, after) in audit {
            let before = before.unwrap_or_default();
            let body = after.unwrap_or_default();
            assert!(before.contains("active_credential_ids"));
            assert!(body.contains("credential_id"));
            assert!(
                !body.contains(AGENT_CREDENTIAL_PREFIX) && !body.contains(&enrolled.credential),
                "rotation audit must be redacted"
            );
        }
    }

    #[tokio::test]
    async fn explicit_revocation_is_immediate_and_typed() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        let pepper = test_pepper(dir.path());
        insert_owner_row(&db).await;

        let record = create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
            .await
            .unwrap();
        let enrolled = enroll(&db, &pepper, &record.token).await.unwrap();
        let credential_id = {
            let auth = authenticate_agent_credential(&db, &pepper, &enrolled.credential)
                .await
                .unwrap()
                .unwrap();
            auth.credential_id
        };

        let revoked =
            revoke_agent_credential(&db, Some("owner"), &enrolled.agent_id, &credential_id)
                .await
                .unwrap();
        assert_eq!(revoked.credential_id, credential_id);
        assert!(
            authenticate_agent_credential(&db, &pepper, &enrolled.credential)
                .await
                .unwrap()
                .is_none(),
            "revocation takes effect immediately"
        );

        // Repeated revoke and foreign credentials are typed.
        assert_eq!(
            revoke_agent_credential(&db, Some("owner"), &enrolled.agent_id, &credential_id)
                .await
                .unwrap_err(),
            RevokeError::AlreadyRevoked
        );
        assert_eq!(
            revoke_agent_credential(
                &db,
                Some("owner"),
                &enrolled.agent_id,
                "credential-not-here",
            )
            .await
            .unwrap_err(),
            RevokeError::NotFound
        );

        let audit: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT before_json, after_json FROM audit_events WHERE event_kind = 'agent_credential_revoked'",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(audit.len(), 1);
        let before = audit[0].0.clone().unwrap_or_default();
        let body = audit[0].1.clone().unwrap_or_default();
        assert!(before.contains("revoked_at"));
        assert!(
            !body.contains(AGENT_CREDENTIAL_PREFIX),
            "revoke audit must be redacted"
        );
    }

    #[test]
    fn agent_display_name_and_notes_are_normalized_and_bounded() {
        assert_eq!(normalize_agent_display_name(None).unwrap(), None);
        assert_eq!(normalize_agent_display_name(Some("   ")).unwrap(), None);
        assert_eq!(
            normalize_agent_display_name(Some("  Host A  ")).unwrap(),
            Some("Host A".to_owned())
        );
        let too_long = "x".repeat(MAX_AGENT_DISPLAY_NAME_LEN + 1);
        assert_eq!(
            normalize_agent_display_name(Some(&too_long)).unwrap_err(),
            AgentMetadataError::InvalidDisplayName("display name must be at most 128 characters")
        );
        assert_eq!(
            normalize_agent_display_name(Some("bad\nname")).unwrap_err(),
            AgentMetadataError::InvalidDisplayName(
                "display name must not contain control characters"
            )
        );
        // Exactly at the bound is accepted.
        let at_bound = "x".repeat(MAX_AGENT_DISPLAY_NAME_LEN);
        assert!(
            normalize_agent_display_name(Some(&at_bound))
                .unwrap()
                .is_some()
        );

        assert_eq!(normalize_agent_notes(None).unwrap(), None);
        assert_eq!(normalize_agent_notes(Some("  ")).unwrap(), None);
        assert_eq!(
            normalize_agent_notes(Some("  line one\nline two\tend  ")).unwrap(),
            Some("line one\nline two\tend".to_owned())
        );
        let too_long_notes = "x".repeat(MAX_AGENT_NOTES_LEN + 1);
        assert_eq!(
            normalize_agent_notes(Some(&too_long_notes)).unwrap_err(),
            AgentMetadataError::InvalidNotes("notes must be at most 2000 characters")
        );
        assert_eq!(
            normalize_agent_notes(Some("bad\u{0}note")).unwrap_err(),
            AgentMetadataError::InvalidNotes("notes must not contain control characters")
        );
    }

    #[tokio::test]
    async fn agent_profile_round_trips_and_persists_for_other_readers() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        let pepper = test_pepper(dir.path());
        insert_owner_row(&db).await;

        let record = create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
            .await
            .unwrap();
        let enrolled = enroll(&db, &pepper, &record.token).await.unwrap();

        // A fresh Agent has no Server-owned name or notes: the stable Agent
        // ID is the identifier, and no name is inferred from Node or
        // diagnostic fields.
        let initial: (Option<String>, Option<String>) =
            sqlx::query_as("SELECT display_name, notes FROM agents WHERE agent_id = ?")
                .bind(&enrolled.agent_id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(initial, (None, None));

        let profile = update_agent_metadata(
            &db,
            Some("owner"),
            &enrolled.agent_id,
            Some("  Host A Agent "),
            Some("Primary Host\ncovers Node A"),
        )
        .await
        .unwrap();
        assert_eq!(profile.display_name.as_deref(), Some("Host A Agent"));
        assert_eq!(
            profile.notes.as_deref(),
            Some("Primary Host\ncovers Node A")
        );

        // The persistence is observable by any later reader (refresh,
        // restart, another Owner) and leaves the Agent ID and Epoch alone.
        let stored: (Option<String>, Option<String>) =
            sqlx::query_as("SELECT display_name, notes FROM agents WHERE agent_id = ?")
                .bind(&enrolled.agent_id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(stored.0.as_deref(), Some("Host A Agent"));
        assert_eq!(stored.1.as_deref(), Some("Primary Host\ncovers Node A"));
        let epoch: i64 = sqlx::query_scalar("SELECT agent_epoch FROM agents WHERE agent_id = ?")
            .bind(&enrolled.agent_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(epoch, 1);

        // Clearing is explicit and returns to the stable-ID fallback.
        let cleared =
            update_agent_metadata(&db, Some("owner"), &enrolled.agent_id, None, Some("   "))
                .await
                .unwrap();
        assert_eq!(
            cleared,
            AgentMetadata {
                display_name: None,
                notes: None,
            }
        );

        // Unknown Agent is typed and never inserts an Agent.
        assert_eq!(
            update_agent_metadata(&db, Some("owner"), "no-such-agent", Some("x"), None)
                .await
                .unwrap_err(),
            AgentMetadataError::AgentNotFound
        );
    }

    #[tokio::test]
    async fn agent_profile_audit_records_before_and_after_once() {
        let dir = tempdir().unwrap();
        let db = test_db(dir.path()).await;
        let pepper = test_pepper(dir.path());
        insert_owner_row(&db).await;

        let record = create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
            .await
            .unwrap();
        let enrolled = enroll(&db, &pepper, &record.token).await.unwrap();

        update_agent_metadata(
            &db,
            Some("owner"),
            &enrolled.agent_id,
            Some("Named"),
            Some("notes"),
        )
        .await
        .unwrap();
        let audit: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT before_json, after_json FROM audit_events WHERE event_kind = 'agent_metadata_changed'",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(audit.len(), 1);
        let before = audit[0].0.clone().unwrap_or_default();
        let after = audit[0].1.clone().unwrap_or_default();
        assert!(before.contains("\"display_name\":null"));
        assert!(after.contains("\"display_name\":\"Named\""));
        assert!(after.contains("\"notes\":\"notes\""));

        // A no-op submission performs no write and adds no Audit row.
        update_agent_metadata(
            &db,
            Some("owner"),
            &enrolled.agent_id,
            Some("Named"),
            Some("notes"),
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'agent_metadata_changed'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
    }
}
