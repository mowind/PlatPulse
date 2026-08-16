//! Server-local SQLite startup and migration harness.
//!
//! The Server opens and validates its database before the HTTP listener is
//! constructed. The write pool is intentionally one connection so a future
//! report-ingestion path can make its SQLite transaction boundary explicit
//! without introducing per-table repositories or a hidden write queue.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use thiserror::Error;

/// The Server's embedded migration source. It is not shared with the Agent.
pub static SERVER_MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The latest migration version compiled into the Server binary.
pub const SERVER_SCHEMA_VERSION: i64 = 25;

/// The Server currently serializes all SQLite operations through one pool
/// connection. Read scaling can be added with a concrete query need; it is
/// not hidden in a repository abstraction.
pub const SERVER_WRITE_CONNECTIONS: u32 = 1;

/// Explicit timeout used for SQLite lock contention.
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const REQUIRED_TABLES: &[&str] = &[
    "server_settings",
    "users",
    "sessions",
    "audit_events",
    "networks",
    "enrollment_tokens",
    "recovery_tokens",
    "agents",
    "agent_credentials",
    "nodes",
    "agent_report_receipts",
    "component_status",
    "current_host_observations",
    "current_host_disk_mounts",
    "current_node_process_observations",
    "current_node_chain_observations",
    "current_node_rpc_namespaces",
    "current_node_rpc_methods",
    "current_node_peers",
    "current_node_peer_capabilities",
    "block_summaries",
    "block_history_state",
    "block_coverage_intervals",
    "block_identity_window",
    "block_history_gaps",
    "report_sequence_gaps",
    "chain_divergence_observations",
    "observed_network_heads",
    "network_reference_heads",
    "agent_spool_diagnostics",
];

/// Connection settings for the Server database.
#[derive(Debug, Clone)]
pub struct ServerDatabaseConfig {
    path: PathBuf,
    busy_timeout: Duration,
}

impl ServerDatabaseConfig {
    /// Create a configuration for a SQLite file. The parent directory must
    /// already exist; initialization does not guess a state directory.
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

/// The SQLite settings required by the Server startup harness.
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

/// Errors that stop Server startup before an HTTP listener can be built.
#[derive(Debug, Error)]
pub enum ServerDatabaseError {
    #[error("Server SQLite connection failed: {0}")]
    Connect(#[source] sqlx::Error),
    #[error("Server SQLite migration failed: {0}")]
    Migration(#[source] MigrateError),
    #[error("Server alert catalog seed failed: {0}")]
    CatalogSeed(String),
    #[error("Server SQLite pragma query failed: {0}")]
    PragmaQuery(#[source] sqlx::Error),
    #[error("Server SQLite required pragmas are not active: {0}")]
    PragmaMismatch(String),
    #[error("Server SQLite integrity query failed: {0}")]
    IntegrityQuery(#[source] sqlx::Error),
    #[error("Server SQLite integrity check failed: {0}")]
    IntegrityFailed(String),
    #[error("Server SQLite database file is empty")]
    EmptyDatabase,
}

/// An initialized Server database whose writes are serialized by one pool
/// connection.
pub struct ServerDatabase {
    pool: SqlitePool,
}

impl ServerDatabase {
    /// Open the Server database, migrate it, validate required pragmas, and
    /// run integrity checks before returning a database to the HTTP startup
    /// path.
    pub async fn open(config: ServerDatabaseConfig) -> Result<Self, ServerDatabaseError> {
        Self::open_with(config, true).await
    }

    /// Open an already initialized database without creating a new file. The
    /// serve path uses this guard so a missing or renamed state file can never
    /// silently become an empty writable database.
    pub async fn open_existing(config: ServerDatabaseConfig) -> Result<Self, ServerDatabaseError> {
        if std::fs::metadata(config.path())
            .map(|metadata| metadata.len() == 0)
            .unwrap_or(true)
        {
            return Err(ServerDatabaseError::EmptyDatabase);
        }
        Self::open_with(config, false).await
    }

    async fn open_with(
        config: ServerDatabaseConfig,
        create_if_missing: bool,
    ) -> Result<Self, ServerDatabaseError> {
        let options = sqlite_options(&config, create_if_missing);
        let pool = SqlitePoolOptions::new()
            .max_connections(SERVER_WRITE_CONNECTIONS)
            .min_connections(SERVER_WRITE_CONNECTIONS)
            .connect_with(options)
            .await
            .map_err(ServerDatabaseError::Connect)?;

        if let Err(error) = SERVER_MIGRATOR.run(&pool).await {
            pool.close().await;
            return Err(ServerDatabaseError::Migration(error));
        }

        // Seed the typed Alert Rule catalog (idempotent; Owner-edited rules
        // are never overwritten) right after migrations so every database
        // carries the full catalog from the start.
        let seed_result = async {
            let mut conn = pool
                .acquire()
                .await
                .map_err(|error| ServerDatabaseError::CatalogSeed(error.to_string()))?;
            crate::alerts::seed_catalog(&mut conn)
                .await
                .map_err(|error| ServerDatabaseError::CatalogSeed(error.to_string()))
        }
        .await;
        if let Err(error) = seed_result {
            pool.close().await;
            return Err(error);
        }

        let pragmas = read_pragmas(&pool)
            .await
            .map_err(ServerDatabaseError::PragmaQuery)?;
        if !pragmas.satisfy_requirements() {
            pool.close().await;
            return Err(ServerDatabaseError::PragmaMismatch(format!(
                "foreign_keys={}, journal_mode={:?}, busy_timeout_ms={}, synchronous={}",
                pragmas.foreign_keys,
                pragmas.journal_mode,
                pragmas.busy_timeout_ms,
                pragmas.synchronous
            )));
        }

        if let Err(error) = verify_integrity(&pool).await {
            pool.close().await;
            return Err(error);
        }
        Ok(Self { pool })
    }

    /// Access the serialized SQLx pool for typed SQL operations.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Read the required connection pragmas after startup.
    pub async fn pragmas(&self) -> Result<SqlitePragmas, ServerDatabaseError> {
        read_pragmas(&self.pool)
            .await
            .map_err(ServerDatabaseError::PragmaQuery)
    }

    /// Return the highest migration version recorded in this Server database.
    pub async fn schema_version(&self) -> Result<i64, ServerDatabaseError> {
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(&self.pool)
            .await
            .map_err(ServerDatabaseError::IntegrityQuery)
    }

    /// Close the serialized pool. The operation is idempotent and can be
    /// called while the HTTP state still owns the database.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Initialize the Server database before constructing a listener.
pub async fn initialize(
    config: ServerDatabaseConfig,
) -> Result<ServerDatabase, ServerDatabaseError> {
    ServerDatabase::open(config).await
}

fn sqlite_options(config: &ServerDatabaseConfig, create_if_missing: bool) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(config.path())
        .create_if_missing(create_if_missing)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(config.busy_timeout())
}

async fn read_pragmas(pool: &SqlitePool) -> Result<SqlitePragmas, sqlx::Error> {
    let foreign_keys = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
        .fetch_one(pool)
        .await?
        != 0;
    let journal_mode = sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
        .fetch_one(pool)
        .await?;
    let busy_timeout_ms = sqlx::query_scalar::<_, i64>("PRAGMA busy_timeout")
        .fetch_one(pool)
        .await?
        .max(0) as u64;
    let synchronous = sqlx::query_scalar::<_, i64>("PRAGMA synchronous")
        .fetch_one(pool)
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

async fn verify_integrity(pool: &SqlitePool) -> Result<(), ServerDatabaseError> {
    let result = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
        .fetch_one(pool)
        .await
        .map_err(ServerDatabaseError::IntegrityQuery)?;
    if result != "ok" {
        return Err(ServerDatabaseError::IntegrityFailed(result));
    }
    if sqlx::query("PRAGMA foreign_key_check")
        .fetch_optional(pool)
        .await
        .map_err(ServerDatabaseError::IntegrityQuery)?
        .is_some()
    {
        return Err(ServerDatabaseError::IntegrityFailed(
            "foreign key violations are present".into(),
        ));
    }

    for table in REQUIRED_TABLES {
        let exists = sqlx::query("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(table)
            .fetch_optional(pool)
            .await
            .map_err(ServerDatabaseError::IntegrityQuery)?
            .is_some();
        if !exists {
            return Err(ServerDatabaseError::IntegrityFailed(format!(
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
    use sqlx::sqlite::SqlitePoolOptions;
    use tempfile::tempdir;

    use super::*;

    fn config(path: &Path) -> ServerDatabaseConfig {
        ServerDatabaseConfig::new(path).with_busy_timeout(Duration::from_millis(1_234))
    }

    fn migrations_through(version: i64) -> Migrator {
        Migrator {
            migrations: Cow::Owned(
                SERVER_MIGRATOR
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
    async fn existing_open_rejects_missing_database_without_creating_one() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("missing.db");
        let result = ServerDatabase::open_existing(config(&path)).await;
        assert!(result.is_err());
        assert!(!path.exists());
    }
    #[tokio::test]
    async fn fresh_database_runs_all_server_migrations_and_required_pragmas() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(config(&path)).await.unwrap();

        assert_eq!(
            database.schema_version().await.unwrap(),
            SERVER_SCHEMA_VERSION
        );
        assert_eq!(database.pragmas().await.unwrap().busy_timeout_ms, 1_234);
        assert!(database.pragmas().await.unwrap().satisfy_requirements());
    }

    #[tokio::test]
    async fn reopening_preserves_the_migrated_database() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        sqlx::query(
            "INSERT INTO server_settings (setting_key, setting_value, updated_at) VALUES (?, ?, ?)",
        )
        .bind("test")
        .bind("value")
        .bind("now")
        .execute(database.pool())
        .await
        .unwrap();
        database.close().await;

        let reopened = ServerDatabase::open(config(&path)).await.unwrap();
        let value: String =
            sqlx::query_scalar("SELECT setting_value FROM server_settings WHERE setting_key = ?")
                .bind("test")
                .fetch_one(reopened.pool())
                .await
                .unwrap();
        assert_eq!(value, "value");
        assert_eq!(
            reopened.schema_version().await.unwrap(),
            SERVER_SCHEMA_VERSION
        );
    }

    #[tokio::test]
    async fn opening_a_previous_schema_applies_the_forward_migration() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(SERVER_WRITE_CONNECTIONS)
            .connect_with(sqlite_options(&config(&path), true))
            .await
            .unwrap();
        migrations_through(1).run(&pool).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        pool.close().await;

        let database = ServerDatabase::open(config(&path)).await.unwrap();
        assert_eq!(
            database.schema_version().await.unwrap(),
            SERVER_SCHEMA_VERSION
        );
    }

    #[tokio::test]
    async fn a_changed_applied_migration_fails_before_database_is_returned() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        sqlx::query("UPDATE _sqlx_migrations SET checksum = zeroblob(48) WHERE version = 1")
            .execute(database.pool())
            .await
            .unwrap();
        database.close().await;

        let error = match ServerDatabase::open(config(&path)).await {
            Ok(_) => panic!("tampered migration should fail"),
            Err(error) => error,
        };
        assert!(matches!(error, ServerDatabaseError::Migration(_)));
    }

    #[tokio::test]
    async fn missing_required_table_fails_integrity_startup_check() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        sqlx::query("DROP TABLE nodes")
            .execute(database.pool())
            .await
            .unwrap();
        database.close().await;

        let error = match ServerDatabase::open(config(&path)).await {
            Ok(_) => panic!("missing required table should fail"),
            Err(error) => error,
        };
        assert!(matches!(error, ServerDatabaseError::IntegrityFailed(_)));
    }

    #[tokio::test]
    async fn server_pool_serializes_writes_to_one_connection() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        assert_eq!(database.pool().size(), SERVER_WRITE_CONNECTIONS);
    }

    #[tokio::test]
    async fn component_status_is_scoped_per_node() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        sqlx::query(
            "INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("mainnet")
        .bind("Mainnet")
        .bind("0xgenesis")
        .bind(1_i64)
        .bind(1_i64)
        .bind("lat")
        .bind("now")
        .bind("now")
        .execute(database.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind("agent-1")
        .bind(1_i64)
        .bind("now")
        .bind("now")
        .execute(database.pool())
        .await
        .unwrap();
        for node_id in ["node-a", "node-b"] {
            sqlx::query(
                "INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, inventory_revision, first_seen_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(node_id)
            .bind("agent-1")
            .bind("mainnet")
            .bind("ws://127.0.0.1:1")
            .bind("active")
            .bind(1_i64)
            .bind("now")
            .bind("now")
            .execute(database.pool())
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, state_revision, value_revision) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind("agent-1")
            .bind("node")
            .bind(node_id)
            .bind(node_id)
            .bind("rpc")
            .bind("starting")
            .bind(1_i64)
            .bind(0_i64)
            .execute(database.pool())
            .await
            .unwrap();
        }

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM component_status WHERE agent_id = ? AND component_key = ?",
        )
        .bind("agent-1")
        .bind("rpc")
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn phase_one_schema_does_not_precreate_future_table_families() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        let forbidden = [
            "peers",
            "geo_location_cache",
            "validators",
            "node_validator_links",
            "block_aggregate_1m",
            "block_aggregate_1h",
        ];

        for table in forbidden {
            let exists =
                sqlx::query("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?")
                    .bind(table)
                    .fetch_optional(database.pool())
                    .await
                    .unwrap()
                    .is_some();
            assert!(!exists, "future table {table:?} was pre-created");
        }
    }
}
