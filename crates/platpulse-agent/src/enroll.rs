//! Agent Enrollment client (design §4.5, §8.2, §12.5/§12.6).
//!
//! `enroll_agent` exchanges a single-use Enrollment Token (read from the
//! TTY/stdin by the CLI, never argv) for the stable Agent identity, Agent
//! Epoch, and a fresh 256-bit Agent Credential. The credential is written
//! to its own 0600 file and the identity is persisted in the Agent Store;
//! the plaintext credential is never printed, logged, or placed in a URL.
//! A locally already-enrolled Agent refuses to enroll again rather than
//! silently replacing its identity.

use std::path::PathBuf;

use reqwest::StatusCode;
use serde::Deserialize;
use sqlx::Connection;
use thiserror::Error;

use crate::config::{AgentConfig, AgentConfigError};
use crate::credential::{CredentialError, write_credential_file};
use crate::database::{AgentDatabaseConfig, AgentDatabaseError, AgentStore};

/// Outcome of a successful local Enrollment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrolledAgent {
    pub agent_id: String,
    pub agent_epoch: i64,
    pub credential_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum EnrollError {
    #[error("failed to load {path}: {source}")]
    Config {
        path: PathBuf,
        #[source]
        source: AgentConfigError,
    },
    #[error("failed to open the agent store: {0}")]
    Store(#[from] AgentDatabaseError),
    #[error(
        "this agent is already enrolled as {0}; enrollment is one-time per identity (Recovery handles credential loss)"
    )]
    AlreadyEnrolled(String),
    #[error("could not reach the server: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("enrollment failed: {0}")]
    ServerRejected(String),
    #[error(
        "the server speaks protocol version {server}, but this agent supports {agent}; upgrade one side"
    )]
    ProtocolMismatch { server: u64, agent: u64 },
    #[error("failed to store the credential: {0}")]
    CredentialWrite(#[from] CredentialError),
    #[error("failed to persist the enrolled identity: {0}")]
    IdentityPersist(#[source] sqlx::Error),
}

/// Server enrollment response (Agent wire: snake_case, design §9.1).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct EnrollResponse {
    agent_id: String,
    agent_epoch: i64,
    credential: String,
    protocol_version: u64,
}

/// Unified Server error envelope; only the bounded `code` is surfaced.
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    code: String,
}

fn rejection_message(code: &str) -> String {
    match code {
        "enrollment_token_invalid" => "the enrollment token is invalid or unknown".to_owned(),
        "enrollment_token_expired" => {
            "the enrollment token has expired; generate a new one".to_owned()
        }
        "enrollment_token_consumed" => {
            "the enrollment token was already used; generate a new one".to_owned()
        }
        "enrollment_rate_limited" => "too many enrollment attempts; try again later".to_owned(),
        "setup_required" => "the server is not set up yet".to_owned(),
        "unavailable" => "the server is temporarily unavailable".to_owned(),
        _ => format!("the server rejected the enrollment ({code})"),
    }
}

/// Check the Agent Store for an existing identity. A fresh Agent has no
/// identity row; an enrolled one must never be silently re-enrolled.
async fn existing_agent_id(store: &mut AgentStore) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT agent_id FROM agent_state WHERE singleton = 1")
        .fetch_optional(store.connection())
        .await
}

/// Persist the enrolled identity in the Agent Store. The singleton row is
/// created on first Enrollment and never silently overwritten with a lower
/// epoch afterwards (the already-enrolled check above guards that).
async fn persist_identity(
    store: &mut AgentStore,
    agent_id: &str,
    agent_epoch: i64,
) -> Result<(), sqlx::Error> {
    let now = time_now_rfc3339();
    let mut transaction = store.connection().begin().await?;
    sqlx::query(
        "INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, updated_at)
         VALUES (1, ?, ?, NULL, 0, 0, ?)
         ON CONFLICT(singleton) DO UPDATE SET agent_id = excluded.agent_id, agent_epoch = excluded.agent_epoch, updated_at = excluded.updated_at",
    )
    .bind(agent_id)
    .bind(agent_epoch)
    .bind(&now)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await
}

/// Run one Enrollment: validate the local state, present the Enrollment
/// Token to the Server over the Agent API, store the credential file, and
/// persist the issued identity. The token is single-use: a failure after
/// the Server accepted it leaves a credential file/identity state that
/// Recovery (Phase 2) addresses; the Server never issues a second identity
/// for the same token.
pub async fn enroll_agent(config: &AgentConfig, token: &str) -> Result<EnrolledAgent, EnrollError> {
    if token.is_empty() {
        return Err(EnrollError::ServerRejected(
            "an enrollment token is required".to_owned(),
        ));
    }

    let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db)).await?;
    if let Some(agent_id) = existing_agent_id(&mut store)
        .await
        .map_err(EnrollError::IdentityPersist)?
    {
        return Err(EnrollError::AlreadyEnrolled(agent_id));
    }

    let client = reqwest::Client::builder()
        .user_agent(format!("platpulse-agent/{}", crate::VERSION))
        .build()
        .map_err(EnrollError::Transport)?;
    let response = client
        .post(format!("{}/api/agent/v1/enroll", config.server_url))
        .bearer_auth(token)
        .send()
        .await
        .map_err(EnrollError::Transport)?;

    let status = response.status();
    let bytes = response.bytes().await.map_err(EnrollError::Transport)?;
    if status != StatusCode::OK {
        let code = serde_json::from_slice::<ErrorEnvelope>(&bytes)
            .ok()
            .map(|envelope| envelope.error.code)
            .unwrap_or_else(|| format!("http_{}", status.as_u16()));
        return Err(EnrollError::ServerRejected(rejection_message(&code)));
    }

    let response: EnrollResponse = serde_json::from_slice(&bytes).map_err(|_| {
        EnrollError::ServerRejected("the server returned an unreadable response".to_owned())
    })?;
    if response.protocol_version != platpulse_core::PROTOCOL_VERSION {
        return Err(EnrollError::ProtocolMismatch {
            server: response.protocol_version,
            agent: platpulse_core::PROTOCOL_VERSION,
        });
    }

    // Credential first (0600, exclusive, fsync'd), then the identity row.
    write_credential_file(&config.credential_file, &response.credential)?;
    persist_identity(&mut store, &response.agent_id, response.agent_epoch)
        .await
        .map_err(EnrollError::IdentityPersist)?;

    Ok(EnrolledAgent {
        agent_id: response.agent_id,
        agent_epoch: response.agent_epoch,
        credential_path: config.credential_file.clone(),
    })
}

/// Canonical RFC 3339 UTC timestamp for the Agent Store's `updated_at`.
fn time_now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("0 ns is valid");
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("Rfc3339 formatting is infallible for valid datetimes")
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::path::Path;

    use sqlx::sqlite::SqlitePoolOptions;
    use tempfile::tempdir;

    use crate::config::AgentConfig;

    use super::*;

    /// A real dev-mode Server (production code, no mocks) listening on an
    /// ephemeral loopback port, with one Owner and an Enrollment Token
    /// ready. Server-side assertions are made through a second SQLite
    /// connection to the same database file (the Server's single writer
    /// connection stays untouched between requests).
    struct TestServer {
        db_path: PathBuf,
        addr: SocketAddr,
        token: String,
    }

    async fn boot_server(dir: &Path) -> TestServer {
        let db_path = dir.join("server.db");
        let db = platpulse_server::database::initialize(
            platpulse_server::database::ServerDatabaseConfig::new(&db_path),
        )
        .await
        .unwrap();
        let pepper_path = dir.join("server-pepper");
        platpulse_server::secrets::create_pepper_file(&pepper_path).unwrap();
        let pepper = platpulse_server::secrets::load_pepper_file(&pepper_path).unwrap();
        let auth =
            platpulse_server::auth::AuthConfig::development(pepper, "http://127.0.0.1:8080".into());
        platpulse_server::auth::create_owner(
            &db,
            "admin",
            &platpulse_server::auth::hash_password(b"correct horse battery").unwrap(),
        )
        .await
        .unwrap();
        let (_, token) = platpulse_server::enrollment::create_enrollment_token(
            &db,
            &pepper,
            platpulse_server::enrollment::ENROLLMENT_TOKEN_DEFAULT_LIFETIME,
        )
        .await
        .unwrap();

        let state = platpulse_server::http::AppState::new(db, None, auth);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, platpulse_server::http::build_app(state))
                .await
                .unwrap();
        });
        TestServer {
            db_path,
            addr,
            token,
        }
    }

    fn agent_config(dir: &Path, server: &TestServer) -> AgentConfig {
        AgentConfig {
            config_path: dir.join("agent.toml"),
            server_url: format!("http://{}", server.addr),
            credential_file: dir.join("credential"),
            state_db: dir.join("agent.db"),
            backfill: crate::config::BackfillConfig::default(),
        }
    }

    async fn server_agent_count(server: &TestServer) -> i64 {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite://{}", server.db_path.display()))
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
            .fetch_one(&pool)
            .await
            .unwrap();
        pool.close().await;
        count
    }

    #[tokio::test]
    async fn enroll_agent_end_to_end_against_the_real_server() {
        let dir = tempdir().unwrap();
        let server = boot_server(dir.path()).await;
        let config = agent_config(dir.path(), &server);

        let enrolled = enroll_agent(&config, &server.token).await.unwrap();
        assert_eq!(enrolled.agent_epoch, 1);
        assert!(!enrolled.agent_id.is_empty());

        // The credential file exists with restrictive permissions and
        // round-trips through the strict loader.
        let metadata = std::fs::metadata(&config.credential_file).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
        let credential = crate::credential::load_credential_file(&config.credential_file).unwrap();
        assert!(credential.starts_with("pp_agent_"));
        assert_eq!(credential.len(), 36 + 1 + 64 + "pp_agent_".len());

        // The Agent Store holds the issued identity.
        let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db))
            .await
            .unwrap();
        let (agent_id, agent_epoch): (String, i64) =
            sqlx::query_as("SELECT agent_id, agent_epoch FROM agent_state WHERE singleton = 1")
                .fetch_one(store.connection())
                .await
                .unwrap();
        assert_eq!(agent_id, enrolled.agent_id);
        assert_eq!(agent_epoch, 1);
        store.close().await.unwrap();

        // Exactly one Server-side Agent identity exists.
        assert_eq!(server_agent_count(&server).await, 1);
    }

    #[tokio::test]
    async fn consumed_token_is_rejected_and_mints_no_second_identity() {
        let dir = tempdir().unwrap();
        let server = boot_server(dir.path()).await;

        // First enrollment consumes the token.
        let first = agent_config(&dir.path().join("first"), &server);
        std::fs::create_dir_all(first.config_path.parent().unwrap()).unwrap();
        enroll_agent(&first, &server.token).await.unwrap();

        // A second, fresh Agent presenting the same token is rejected by
        // the Server with the stable consumed code.
        let second = agent_config(&dir.path().join("second"), &server);
        std::fs::create_dir_all(second.config_path.parent().unwrap()).unwrap();
        let error = enroll_agent(&second, &server.token).await.unwrap_err();
        assert!(matches!(error, EnrollError::ServerRejected(_)));
        assert!(error.to_string().contains("already used"), "{error}");
        assert!(
            !error.to_string().contains(&server.token),
            "errors must never echo the token"
        );

        // No second identity was minted.
        assert_eq!(server_agent_count(&server).await, 1);
    }

    #[tokio::test]
    async fn unknown_token_and_local_re_enrollment_are_refused() {
        let dir = tempdir().unwrap();
        let server = boot_server(dir.path()).await;

        // Unknown token.
        let fresh = agent_config(&dir.path().join("fresh"), &server);
        std::fs::create_dir_all(fresh.config_path.parent().unwrap()).unwrap();
        let error = enroll_agent(&fresh, "pp_enroll_unknown_abc")
            .await
            .unwrap_err();
        assert!(matches!(error, EnrollError::ServerRejected(_)));
        assert!(error.to_string().contains("invalid or unknown"));

        // A successful enrollment persists the identity; afterwards the
        // same local Agent refuses a second enrollment instead of
        // replacing it.
        enroll_agent(&fresh, &server.token).await.unwrap();
        let error = enroll_agent(&fresh, &server.token).await.unwrap_err();
        assert!(matches!(error, EnrollError::AlreadyEnrolled(_)));
        assert!(error.to_string().contains("already enrolled"));

        assert_eq!(server_agent_count(&server).await, 1);
    }

    #[test]
    fn rejection_messages_are_stable_and_never_carry_secrets() {
        assert_eq!(
            rejection_message("enrollment_token_expired"),
            "the enrollment token has expired; generate a new one"
        );
        assert_eq!(
            rejection_message("some_future_code"),
            "the server rejected the enrollment (some_future_code)"
        );
    }

    #[test]
    fn rfc3339_timestamp_is_canonical_utc() {
        let value = time_now_rfc3339();
        assert!(value.ends_with('Z'), "{value}");
        assert_eq!(value.len(), 20, "YYYY-MM-DDTHH:MM:SSZ");
    }
}
