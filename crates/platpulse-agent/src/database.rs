//! Agent-local SQLite startup and migration harness.
//!
//! The Agent owns one SQLite write connection. Collection is only allowed to
//! start after [`AgentStore::open`] has completed migrations, pragma checks,
//! and the integrity check. The migration source is deliberately local to
//! this crate; it is not shared with the Server.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Connection, SqliteConnection};
use thiserror::Error;

/// The Agent's embedded migration source.
///
/// This is intentionally a different static from the Server migrator, even
/// though both databases use SQLx's default `_sqlx_migrations` table.
pub static AGENT_MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The latest migration version compiled into the Agent binary.
pub const AGENT_SCHEMA_VERSION: i64 = 4;

/// Explicit timeout used for SQLite lock contention unless a caller chooses a
/// tighter or more generous value for a test/deployment.
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const REQUIRED_TABLES: &[&str] = &[
    "agent_state",
    "pending_block_summaries",
    "history_gaps",
    "block_summaries",
    "reports",
    "report_receipts",
    "rejection_ledger",
];

/// Connection settings for the Agent Store.
#[derive(Debug, Clone)]
pub struct AgentDatabaseConfig {
    path: PathBuf,
    busy_timeout: Duration,
}

impl AgentDatabaseConfig {
    /// Create a configuration for a SQLite file. The parent directory must
    /// already exist; startup never creates an unexpected directory tree.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        }
    }

    /// Override the lock wait used by SQLite.
    pub fn with_busy_timeout(mut self, busy_timeout: Duration) -> Self {
        self.busy_timeout = busy_timeout;
        self
    }

    /// SQLite file used by this configuration.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Lock wait used by this configuration.
    pub fn busy_timeout(&self) -> Duration {
        self.busy_timeout
    }
}

/// The SQLite settings that are required on every Agent connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlitePragmas {
    /// `PRAGMA foreign_keys` (`1` means enabled).
    pub foreign_keys: bool,
    /// Effective journal mode, normally `wal` for a file database.
    pub journal_mode: JournalMode,
    /// `PRAGMA busy_timeout` in milliseconds.
    pub busy_timeout_ms: u64,
    /// Numeric SQLite synchronous mode (`2` is FULL).
    pub synchronous: i64,
}

impl SqlitePragmas {
    /// Whether all durability/concurrency settings required by PlatPulse are
    /// active for this connection.
    pub fn satisfy_requirements(self) -> bool {
        self.foreign_keys
            && self.journal_mode == JournalMode::Wal
            && self.busy_timeout_ms > 0
            && self.synchronous == 2
    }
}

/// The journal mode reported by SQLite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalMode {
    Wal,
    Other,
}

/// Errors that stop Agent startup before collection can begin.
#[derive(Debug, Error)]
pub enum AgentDatabaseError {
    #[error("Agent SQLite connection failed: {0}")]
    Connect(#[source] sqlx::Error),
    #[error("Agent SQLite migration failed: {0}")]
    Migration(#[source] MigrateError),
    #[error("Agent SQLite pragma query failed: {0}")]
    PragmaQuery(#[source] sqlx::Error),
    #[error("Agent SQLite required pragmas are not active: {0}")]
    PragmaMismatch(String),
    #[error("Agent SQLite integrity query failed: {0}")]
    IntegrityQuery(#[source] sqlx::Error),
    #[error("Agent SQLite integrity check failed: {0}")]
    IntegrityFailed(String),
    #[error("failed to secure Agent Store file {path}: {source}")]
    SecureStore {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// An initialized Agent Store with exactly one writer connection.
///
/// No collection code is run by this type. Callers receive the store only
/// after startup validation succeeds, then pass its single connection to the
/// collection/store operations that need it.
pub struct AgentStore {
    connection: SqliteConnection,
}

impl AgentStore {
    /// Open the Agent database, migrate it, validate required pragmas, and
    /// run integrity checks before returning a store to the collector.
    pub async fn open(config: AgentDatabaseConfig) -> Result<Self, AgentDatabaseError> {
        // Design §8.2: the credential file AND the state DB must only allow
        // the Agent OS user to read. Umask 077 keeps SQLite WAL/SHM
        // siblings private; the explicit 0600 chmod below pins the file
        // itself even when a permissive umask was inherited.
        restrict_umask();
        let options = sqlite_options(&config);
        let mut connection = SqliteConnection::connect_with(&options)
            .await
            .map_err(AgentDatabaseError::Connect)?;

        AGENT_MIGRATOR
            .run_direct(&mut connection)
            .await
            .map_err(AgentDatabaseError::Migration)?;

        let pragmas = read_pragmas(&mut connection)
            .await
            .map_err(AgentDatabaseError::PragmaQuery)?;
        if !pragmas.satisfy_requirements() {
            return Err(AgentDatabaseError::PragmaMismatch(format!(
                "foreign_keys={}, journal_mode={:?}, busy_timeout_ms={}, synchronous={}",
                pragmas.foreign_keys,
                pragmas.journal_mode,
                pragmas.busy_timeout_ms,
                pragmas.synchronous
            )));
        }

        verify_integrity(&mut connection).await?;
        secure_store_file(config.path())?;
        Ok(Self { connection })
    }

    /// Access the sole Agent write connection for typed SQL operations.
    pub fn connection(&mut self) -> &mut SqliteConnection {
        &mut self.connection
    }

    /// Read the required connection pragmas after startup.
    pub async fn pragmas(&mut self) -> Result<SqlitePragmas, AgentDatabaseError> {
        read_pragmas(&mut self.connection)
            .await
            .map_err(AgentDatabaseError::PragmaQuery)
    }

    /// Return the highest migration version recorded in this Agent database.
    pub async fn schema_version(&mut self) -> Result<i64, AgentDatabaseError> {
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(&mut self.connection)
            .await
            .map_err(AgentDatabaseError::IntegrityQuery)
    }

    /// Close the single writer connection.
    pub async fn close(self) -> Result<(), sqlx::Error> {
        self.connection.close().await
    }
}

/// Initialize the Agent Store before any collector is started.
pub async fn initialize(config: AgentDatabaseConfig) -> Result<AgentStore, AgentDatabaseError> {
    AgentStore::open(config).await
}

/// Restrict the Agent Store file to the Agent OS user (0600) after open.
#[cfg(unix)]
pub(crate) fn secure_store_file(path: &Path) -> Result<(), AgentDatabaseError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
        AgentDatabaseError::SecureStore {
            path: path.to_owned(),
            source,
        }
    })
}

#[cfg(not(unix))]
pub(crate) fn secure_store_file(_path: &Path) -> Result<(), AgentDatabaseError> {
    Ok(())
}

/// Run with umask 077 so SQLite WAL/SHM siblings inherit
/// agent-user-only permissions (design §8.2).
#[cfg(unix)]
pub(crate) fn restrict_umask() {
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));
}

#[cfg(not(unix))]
pub(crate) fn restrict_umask() {}

fn sqlite_options(config: &AgentDatabaseConfig) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(config.path())
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(config.busy_timeout())
}

async fn read_pragmas(connection: &mut SqliteConnection) -> Result<SqlitePragmas, sqlx::Error> {
    let foreign_keys = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
        .fetch_one(&mut *connection)
        .await?
        != 0;
    let journal_mode = sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
        .fetch_one(&mut *connection)
        .await?;
    let busy_timeout_ms = sqlx::query_scalar::<_, i64>("PRAGMA busy_timeout")
        .fetch_one(&mut *connection)
        .await?
        .max(0) as u64;
    let synchronous = sqlx::query_scalar::<_, i64>("PRAGMA synchronous")
        .fetch_one(&mut *connection)
        .await?;

    Ok(SqlitePragmas {
        foreign_keys,
        journal_mode: if journal_mode.eq_ignore_ascii_case("wal") {
            JournalMode::Wal
        } else {
            JournalMode::Other
        },
        busy_timeout_ms,
        synchronous,
    })
}

async fn verify_integrity(connection: &mut SqliteConnection) -> Result<(), AgentDatabaseError> {
    let result = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
        .fetch_one(&mut *connection)
        .await
        .map_err(AgentDatabaseError::IntegrityQuery)?;
    if result != "ok" {
        return Err(AgentDatabaseError::IntegrityFailed(result));
    }
    if sqlx::query("PRAGMA foreign_key_check")
        .fetch_optional(&mut *connection)
        .await
        .map_err(AgentDatabaseError::IntegrityQuery)?
        .is_some()
    {
        return Err(AgentDatabaseError::IntegrityFailed(
            "foreign key violations are present".into(),
        ));
    }

    for table in REQUIRED_TABLES {
        let exists = sqlx::query("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(table)
            .fetch_optional(&mut *connection)
            .await
            .map_err(AgentDatabaseError::IntegrityQuery)?
            .is_some();
        if !exists {
            return Err(AgentDatabaseError::IntegrityFailed(format!(
                "required table {table:?} is missing"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use sqlx::migrate::Migrator;
    use tempfile::tempdir;

    use super::*;

    fn config(path: &Path) -> AgentDatabaseConfig {
        AgentDatabaseConfig::new(path).with_busy_timeout(Duration::from_millis(1_234))
    }

    fn migrations_through(version: i64) -> Migrator {
        Migrator {
            migrations: Cow::Owned(
                AGENT_MIGRATOR
                    .iter()
                    .filter(|migration| migration.version <= version)
                    .cloned()
                    .collect(),
            ),
            ignore_missing: false,
            locking: true,
            no_tx: false,
        }
    }

    #[tokio::test]
    async fn fresh_database_runs_all_agent_migrations_and_required_pragmas() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("agent.db");
        let mut store = AgentStore::open(config(&path)).await.unwrap();

        assert_eq!(store.schema_version().await.unwrap(), AGENT_SCHEMA_VERSION);
        assert_eq!(store.pragmas().await.unwrap().busy_timeout_ms, 1_234);
        assert!(store.pragmas().await.unwrap().satisfy_requirements());
    }

    #[tokio::test]
    async fn store_file_is_restricted_to_the_agent_user() {
        // Design §8.2: the state DB must only allow the Agent OS user to
        // read; opening the store pins the file to 0600.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let directory = tempdir().unwrap();
            let path = directory.path().join("agent.db");
            let store = AgentStore::open(config(&path)).await.unwrap();
            let metadata = std::fs::metadata(&path).unwrap();
            assert_eq!(
                metadata.permissions().mode() & 0o777,
                0o600,
                "Agent Store file must be 0600"
            );
            store.close().await.unwrap();
        }
    }

    #[tokio::test]
    async fn reopening_preserves_the_migrated_database() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("agent.db");
        let mut store = AgentStore::open(config(&path)).await.unwrap();
        sqlx::query("INSERT INTO agent_state (singleton, updated_at) VALUES (1, 'now')")
            .execute(store.connection())
            .await
            .unwrap();
        store.close().await.unwrap();

        let mut reopened = AgentStore::open(config(&path)).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_state")
            .fetch_one(reopened.connection())
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            reopened.schema_version().await.unwrap(),
            AGENT_SCHEMA_VERSION
        );
    }

    #[tokio::test]
    async fn opening_a_previous_schema_applies_the_forward_migration() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("agent.db");
        let options = sqlite_options(&config(&path));
        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        migrations_through(1)
            .run_direct(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
                .fetch_one(&mut connection)
                .await
                .unwrap(),
            1
        );
        connection.close().await.unwrap();

        let mut store = AgentStore::open(config(&path)).await.unwrap();
        assert_eq!(store.schema_version().await.unwrap(), AGENT_SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn a_changed_applied_migration_fails_before_store_is_returned() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("agent.db");
        let mut store = AgentStore::open(config(&path)).await.unwrap();
        sqlx::query("UPDATE _sqlx_migrations SET checksum = zeroblob(48) WHERE version = 1")
            .execute(store.connection())
            .await
            .unwrap();
        store.close().await.unwrap();

        let error = match AgentStore::open(config(&path)).await {
            Ok(_) => panic!("tampered migration should fail"),
            Err(error) => error,
        };
        assert!(matches!(error, AgentDatabaseError::Migration(_)));
    }

    #[tokio::test]
    async fn missing_required_table_fails_integrity_startup_check() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("agent.db");
        let mut store = AgentStore::open(config(&path)).await.unwrap();
        sqlx::query("DROP TABLE reports")
            .execute(store.connection())
            .await
            .unwrap();
        store.close().await.unwrap();

        let error = match AgentStore::open(config(&path)).await {
            Ok(_) => panic!("missing required table should fail"),
            Err(error) => error,
        };
        assert!(matches!(error, AgentDatabaseError::IntegrityFailed(_)));
    }
}
