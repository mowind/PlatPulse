//! The Server's storage seam (ADR 0013).
//!
//! [`ServerDatabase`] is the only way the Server reaches its database. The
//! domain modules see a backend-neutral [`sqlx::AnyPool`] and write portable
//! SQL; everything that depends on the Storage Backend in use (file ownership,
//! connection PRAGMAs, integrity checks, result-code classification, page
//! accounting) is dispatched to a per-backend module: [`sqlite`] for the
//! SQLite file backend and [`postgres`] for the PostgreSQL backend
//! (ADR 0013, issue #255).
//!
//! The pool is intentionally one connection so the report-ingestion path can
//! make its transaction boundary explicit without introducing per-table
//! repositories or a hidden write queue.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::{AnyConnection, AnyPool};
use thiserror::Error;

mod postgres;
mod sqlite;

/// The oldest supported PostgreSQL major version, re-exported for the Doctor
/// version check (ADR 0013).
pub(crate) use postgres::MIN_SERVER_VERSION_NUM;

pub use postgres::{
    DEFAULT_CONNECT_TIMEOUT as POSTGRES_DEFAULT_CONNECT_TIMEOUT,
    DEFAULT_OWNERSHIP_HEARTBEAT as POSTGRES_DEFAULT_OWNERSHIP_HEARTBEAT,
};
pub use postgres::{
    PostgresConnectConfig, PostgresHost, PostgresPassword, PostgresSecretError, PostgresSslMode,
};
pub use sqlite::{JournalMode, SqlitePragmas};

/// The Server's embedded SQLite migration source. It is not shared with the
/// Agent. The PostgreSQL baseline lives beside it in
/// [`postgres::POSTGRES_MIGRATOR`] at the same logical schema version.
pub static SERVER_MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The Storage Backend a Server database is stored by (ADR 0013).
#[derive(Debug, Clone)]
pub enum StorageConfig {
    /// A SQLite file (the default).
    Sqlite(ServerDatabaseConfig),
    /// A PostgreSQL database (issue #255).
    Postgres(PostgresConnectConfig),
}

impl From<ServerDatabaseConfig> for StorageConfig {
    fn from(config: ServerDatabaseConfig) -> Self {
        Self::Sqlite(config)
    }
}

impl From<PostgresConnectConfig> for StorageConfig {
    fn from(config: PostgresConnectConfig) -> Self {
        Self::Postgres(config)
    }
}

impl StorageConfig {
    /// A short, loggable name of the backend in use.
    pub fn backend_name(&self) -> &'static str {
        match self {
            Self::Sqlite(_) => "sqlite",
            Self::Postgres(_) => "postgres",
        }
    }
}

/// The Storage Backend a running Server uses, in the shape the Owner-only
/// Admin Doctor and Settings surfaces show it (ADR 0013).
///
/// Nothing here belongs to a Public Projection, a Public DTO, or SSE: the
/// backend, the server version, and the connection target are Operator
/// knowledge, and the Admin offers no switch between backends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageBackendFacts {
    /// The backend in use: "sqlite" or "postgres".
    pub backend: &'static str,
    /// The PostgreSQL connection the Server was configured with. `None` on the
    /// SQLite backend, which has no connection target to name.
    pub postgres: Option<PostgresFacts>,
}

/// Where the Server connects on the PostgreSQL backend: host, port, database
/// and role, never the password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostgresFacts {
    /// The configured host name or address, or the Unix-domain socket
    /// directory.
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
}

/// The latest migration version compiled into the Server binary.
pub const SERVER_SCHEMA_VERSION: i64 = 76;

/// SQLite allows one writer at a time, so the SQLite Storage Backend keeps
/// exactly one pool connection: more would only trade correctness for lock
/// waits. Read scaling can be added with a concrete query need; it is not
/// hidden in a repository abstraction.
pub const SERVER_WRITE_CONNECTIONS: u32 = 1;

/// Work connections the PostgreSQL Storage Backend opens.
///
/// A PostgreSQL server serializes conflicting rows, not the whole database, so
/// Reports from different Agents no longer queue behind one another (ADR 0013).
/// Measured on the production host: one Report occupies its connection for
/// ~190 ms at ~3 Reports/s, and with a single connection every read — session
/// lookup, alert sweep, retention cleanup — waited behind an ingest
/// transaction.
pub const POSTGRES_WORK_CONNECTIONS: u32 = 8;

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
    "current_node_data_directory_observations",
    "current_node_chain_observations",
    "current_node_rpc_namespaces",
    "current_node_rpc_methods",
    "current_node_peers",
    "current_node_peer_capabilities",
    "peer_presence_intervals",
    "peer_aggregate_5m",
    "peer_aggregate_5m_countries",
    "peer_aggregate_1h",
    "peer_aggregate_1h_countries",
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
    "geo_location_cache",
    "validators",
    "node_validator_links",
    "node_validator_identity_status",
    "deleted_nodes",
    "agent_attention_acknowledgments",
    "incident_acknowledgments",
    "current_validator_insights",
    "validator_ranking_history",
    "validator_counter_history",
    "validator_daily_snapshots",
    "validator_monthly_aggregates",
    "validator_model_migration",
    "inventory_migration",
    "inventory_cutover",
    "notification_requests",
    "operation_requests",
    "capacity_protection_intervals",
    "capacity_skipped_series",
    "node_metric_series_state",
    "node_metric_aggregates",
    "node_block_aggregates",
    "host_metric_series_state",
    "host_metric_aggregates",
    "node_state_observations",
    "node_state_series_state",
    "node_relationship_intervals",
    "reward_wallet_balance",
    "storage_migration",
];

/// Connection settings for the Server database.
#[derive(Debug, Clone)]
pub struct ServerDatabaseConfig {
    path: PathBuf,
    busy_timeout: Duration,
    exclusive_locking: bool,
}

impl ServerDatabaseConfig {
    /// Create a configuration for a SQLite file. The parent directory must
    /// already exist; initialization does not guess a state directory.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
            exclusive_locking: true,
        }
    }

    /// Connection settings for a resolved deployment.
    ///
    /// Development mode keeps SQLite's normal locking protocol so local
    /// tooling and the browser e2e fixture refreshers may attach to a running
    /// Server; every other deployment pins the exclusive ownership of issue
    /// #137, where the CLI and Backup/Restore are stopped-Server operations.
    /// Development mode is the deliberate exception for local tooling.
    pub fn for_deployment(path: impl Into<PathBuf>, development: bool) -> Self {
        Self::new(path).with_exclusive_locking(!development)
    }

    /// Override the lock wait used by SQLite.
    pub fn with_busy_timeout(mut self, busy_timeout: Duration) -> Self {
        self.busy_timeout = busy_timeout;
        self
    }

    /// Pin exclusive ownership of the SQLite file for every connection.
    pub fn with_exclusive_locking(mut self, exclusive_locking: bool) -> Self {
        self.exclusive_locking = exclusive_locking;
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

    /// Whether this configuration pins exclusive ownership of the file.
    pub fn exclusive_locking(&self) -> bool {
        self.exclusive_locking
    }
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
    #[error("Server SQLite sensitive path is unsafe: {0}")]
    Filesystem(String),
    #[error("Server SQLite database file is empty")]
    EmptyDatabase,
    #[error("{0}")]
    AlreadyMigrated(String),
    #[error("Server PostgreSQL configuration is refused: {0}")]
    Unsupported(String),
    #[error(
        "another Server owns this PostgreSQL database: the session advisory lock is held; only one Server may serve a database at a time"
    )]
    OwnershipHeld,
    #[error("Server PostgreSQL schema ownership check failed: {0}")]
    SchemaOwnership(String),
    #[error("Server PostgreSQL collation check failed: {0}")]
    Collation(String),
}

impl ServerDatabaseError {
    /// The underlying database error, when this failure came from a query.
    /// Used to latch runtime corruption from an observed corruption code.
    pub(crate) fn as_sqlx(&self) -> Option<&sqlx::Error> {
        match self {
            Self::Connect(error) | Self::PragmaQuery(error) | Self::IntegrityQuery(error) => {
                Some(error)
            }
            _ => None,
        }
    }
}

/// How much of the database a bounded integrity check covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntegrityScope {
    /// Every page and index (`PRAGMA integrity_check` on SQLite).
    Full,
    /// The cheaper structural check, stopping at the first problem
    /// (`PRAGMA quick_check(1)` on SQLite).
    Quick,
}

/// Outcome of one bounded integrity check.
#[derive(Debug)]
pub(crate) enum IntegrityVerdict {
    /// The check completed and found nothing wrong.
    Consistent,
    /// The check completed and reported a problem.
    Inconsistent(String),
    /// The budget ran out and the check was interrupted: no verdict.
    Exhausted,
    /// The database returned an error before any verdict.
    Failed(sqlx::Error),
    /// The backend in use has no bounded integrity check. The PostgreSQL
    /// equivalent belongs to the 0.3.0 operations slice (ADR 0013, S5); an
    /// unavailable check is never reported as consistent.
    Unsupported,
}

/// Page accounting of the stored database, when the backend has one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StoragePages {
    pub page_count: Option<u64>,
    pub freelist_pages: Option<u64>,
}

/// A nullable flag column, decoded from its portable 0/1 INTEGER storage.
///
/// Flags are stored as INTEGER on every Storage Backend (ADR 0013): the
/// backend-neutral driver decodes `bool` only from a native boolean, so a
/// flag read as `bool` would fail. Plain tuple decodes read the flag as `i64`
/// and compare with zero; a `FromRow` field of type `Option<bool>` uses
/// `#[sqlx(try_from = "crate::database::NullableFlag")]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NullableFlag(pub Option<bool>);

impl sqlx::Type<sqlx::Any> for NullableFlag {
    fn type_info() -> sqlx::any::AnyTypeInfo {
        <i64 as sqlx::Type<sqlx::Any>>::type_info()
    }

    fn compatible(ty: &sqlx::any::AnyTypeInfo) -> bool {
        <i64 as sqlx::Type<sqlx::Any>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Any> for NullableFlag {
    fn decode(value: sqlx::any::AnyValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        use sqlx::ValueRef;
        if value.is_null() {
            return Ok(Self(None));
        }
        let flag = <i64 as sqlx::Decode<'r, sqlx::Any>>::decode(value)?;
        Ok(Self(Some(flag != 0)))
    }
}

impl From<NullableFlag> for Option<bool> {
    fn from(flag: NullableFlag) -> Self {
        flag.0
    }
}

/// A nullable real column, bound through the backend-neutral driver.
///
/// A direct `.bind(Option<f64>)` is not portable: the backend-neutral driver
/// converts a bound value to the native driver by its kind, and its `None`
/// kind for a `f64` becomes `Option<f32>` — so an absent sample declares the
/// parameter `float4` while a present one declares `float8`. PostgreSQL caches
/// a prepared statement together with the parameter types of its first
/// execution, so a statement first executed without a sample then rejects
/// every later real sample with `22P03 incorrect binary data format in bind
/// parameter N`. Encoding the absence as a null of the other float kind makes
/// both spellings convert to `Option<f64>`, so the parameter is always
/// `float8` (ADR 0013).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NullableReal(pub Option<f64>);

impl From<Option<f64>> for NullableReal {
    fn from(value: Option<f64>) -> Self {
        Self(value)
    }
}

impl sqlx::Type<sqlx::Any> for NullableReal {
    fn type_info() -> sqlx::any::AnyTypeInfo {
        <f64 as sqlx::Type<sqlx::Any>>::type_info()
    }

    fn compatible(ty: &sqlx::any::AnyTypeInfo) -> bool {
        <f64 as sqlx::Type<sqlx::Any>>::compatible(ty)
    }
}

impl sqlx::Encode<'_, sqlx::Any> for NullableReal {
    fn encode_by_ref(
        &self,
        buffer: &mut <sqlx::Any as sqlx::Database>::ArgumentBuffer<'_>,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        match self.0 {
            Some(value) => <f64 as sqlx::Encode<sqlx::Any>>::encode(value, buffer),
            // A null `f32` is the encoding the driver converts to
            // `Option<f64>::None` — see the type documentation.
            None => <Option<f32> as sqlx::Encode<sqlx::Any>>::encode(None, buffer),
        }
    }
}

/// Whether an open path runs the maintenance writes that follow migrations.
///
/// A serving Server seeds the typed Alert Rule catalog and invalidates carried
/// recovery windows; an offline maintenance command must observe the stored
/// state instead of changing it (issue #257).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupWrites {
    Apply,
    Skip,
}

/// The Storage Backend a deployment runs on (ADR 0013).
///
/// Callers branch on this value instead of comparing the published name:
/// matching an enum keeps every decision exhaustive, so a third backend is a
/// compile error rather than a silent fall-through into the SQLite arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageBackend {
    /// The default in-process SQLite database file.
    Sqlite,
    /// The PostgreSQL Storage Backend (issue #255).
    Postgres,
}

impl StorageBackend {
    /// The name the Admin surfaces and the readiness registry publish.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
        }
    }
}

/// The Storage Backend behind a [`ServerDatabase`].
enum Backend {
    /// Boxed because `SqliteConnectOptions` is far larger than the PostgreSQL
    /// handle; the enum itself lives once per process, so the indirection is
    /// free (clippy::large_enum_variant).
    Sqlite(Box<sqlite::SqliteBackend>),
    Postgres(postgres::PostgresBackend),
}

/// An initialized Server database whose writes are serialized by one pool
/// connection.
pub struct ServerDatabase {
    pool: AnyPool,
    backend: Backend,
    /// The SQLite file backing this database; `None` on PostgreSQL.
    path: Option<PathBuf>,
    /// The PostgreSQL connection target, resolved from configuration once at
    /// startup. `None` on the SQLite backend.
    postgres: Option<PostgresFacts>,
    /// The runtime Inventory protocol mode, resolved once at startup from the
    /// durable conversion/cutover markers (issue #192). It is fixed for the
    /// process lifetime because resuming the cutover requires a stopped Server.
    inventory_cutover: crate::cutover::InventoryCutover,
}

impl ServerDatabase {
    /// Open the Server database, migrate it, validate the backend contract,
    /// and run integrity checks before returning a database to the HTTP
    /// startup path.
    pub async fn open(config: impl Into<StorageConfig>) -> Result<Self, ServerDatabaseError> {
        Self::open_with(config.into(), true, StartupWrites::Apply).await
    }

    /// Open an already initialized database without creating a new SQLite
    /// file. The serve path uses this guard so a missing or renamed state file
    /// can never silently become an empty writable database. A PostgreSQL
    /// database must already exist; connecting to a missing one fails.
    pub async fn open_existing(
        config: impl Into<StorageConfig>,
    ) -> Result<Self, ServerDatabaseError> {
        let config = config.into();
        if let StorageConfig::Sqlite(sqlite_config) = &config {
            match std::fs::symlink_metadata(sqlite_config.path()) {
                Ok(metadata) if metadata.len() == 0 => {
                    return Err(ServerDatabaseError::EmptyDatabase);
                }
                Ok(_) => {}
                Err(_) => return Err(ServerDatabaseError::EmptyDatabase),
            }
        }
        Self::open_with(config, false, StartupWrites::Apply).await
    }

    /// Open an already initialized database for an offline maintenance command
    /// that must observe the stored state instead of changing it.
    ///
    /// Migrations, the backend contract checks and the Inventory cutover
    /// resolution run exactly as they do at startup, but the maintenance writes
    /// that follow migrations (Alert Rule catalog seeding and recovery-window
    /// invalidation) are skipped: a command that measures what the database
    /// holds - the Storage Backend Migration and its abort (ADR 0013, issue
    /// #257) - cannot compare a digest against content it just rewrote.
    pub(crate) async fn open_for_maintenance(
        config: impl Into<StorageConfig>,
    ) -> Result<Self, ServerDatabaseError> {
        Self::open_with(config.into(), false, StartupWrites::Skip).await
    }

    async fn open_with(
        config: StorageConfig,
        create_if_missing: bool,
        startup_writes: StartupWrites,
    ) -> Result<Self, ServerDatabaseError> {
        let (pool, backend, path, postgres) = match &config {
            StorageConfig::Sqlite(sqlite_config) => {
                let (pool, backend) = sqlite::open(sqlite_config, create_if_missing).await?;
                (
                    pool,
                    Backend::Sqlite(Box::new(backend)),
                    Some(sqlite_config.path().to_owned()),
                    None,
                )
            }
            StorageConfig::Postgres(postgres_config) => {
                let (pool, backend) = postgres::open(postgres_config).await?;
                (
                    pool,
                    Backend::Postgres(backend),
                    None,
                    Some(PostgresFacts {
                        host: match postgres_config.host() {
                            PostgresHost::Tcp(host) => host.clone(),
                            PostgresHost::Socket(path) => path.display().to_string(),
                        },
                        port: postgres_config.port(),
                        database: postgres_config.database().to_owned(),
                        user: postgres_config.user().to_owned(),
                    }),
                )
            }
        };
        match Self::prepare(&pool, &backend, &config, create_if_missing, startup_writes).await {
            Ok(inventory_cutover) => Ok(Self {
                pool,
                backend,
                path,
                postgres,
                inventory_cutover,
            }),
            Err(error) => {
                pool.close().await;
                if let Backend::Postgres(backend) = &backend {
                    backend.release().await;
                }
                Err(error)
            }
        }
    }

    /// Migrate, seed and verify a freshly opened pool.
    async fn prepare(
        pool: &AnyPool,
        backend: &Backend,
        config: &StorageConfig,
        create_if_missing: bool,
        startup_writes: StartupWrites,
    ) -> Result<crate::cutover::InventoryCutover, ServerDatabaseError> {
        match backend {
            Backend::Sqlite(_) => SERVER_MIGRATOR.run(pool).await,
            Backend::Postgres(_) => postgres::POSTGRES_MIGRATOR.run(pool).await,
        }
        .map_err(ServerDatabaseError::Migration)?;

        if startup_writes == StartupWrites::Apply {
            // Seed the typed Alert Rule catalog (idempotent; Owner-edited rules
            // are never overwritten) right after migrations so every database
            // carries the full catalog from the start.
            let mut conn = pool
                .acquire()
                .await
                .map_err(|error| ServerDatabaseError::CatalogSeed(error.to_string()))?;
            crate::alerts::seed_catalog(&mut conn)
                .await
                .map_err(|error| ServerDatabaseError::CatalogSeed(error.to_string()))?;
            // A restart is an observation gap; no recovery window may be
            // carried across it (issue #203 review B2).
            crate::alerts::invalidate_recovery_windows_on_startup(&mut conn)
                .await
                .map_err(|error| ServerDatabaseError::CatalogSeed(error.to_string()))?;
            drop(conn);
        }

        match (backend, config) {
            (Backend::Sqlite(sqlite_backend), StorageConfig::Sqlite(sqlite_config)) => {
                sqlite::verify_startup(pool, sqlite_backend, sqlite_config, create_if_missing)
                    .await?;
            }
            (Backend::Postgres(_), _) => postgres::verify_startup(pool).await?,
            (Backend::Sqlite(_), StorageConfig::Postgres(_)) => {
                unreachable!("a SQLite backend is always opened from a SQLite configuration")
            }
        }
        // Resolve the durable Inventory protocol mode once at startup. A
        // converted deployment (issue #191) stays gated until the coordinated
        // cutover is resumed (issue #192); a fresh or pre-conversion
        // deployment keeps the ordinary v1/v2 behaviour.
        //
        // The conversion marker is a SQLite file migration, so only a SQLite
        // deployment can carry it. A PostgreSQL baseline is created directly in
        // the current protocol and the cutover tooling refuses that backend
        // (`require_sqlite_storage`), so there is nothing to gate.
        match backend {
            Backend::Sqlite(_) => crate::cutover::detect_inventory_cutover(pool)
                .await
                .map_err(ServerDatabaseError::IntegrityQuery),
            Backend::Postgres(_) => Ok(crate::cutover::InventoryCutover::NotConverted),
        }
    }

    /// Filesystem path of the validated Server SQLite database. It is
    /// `None` on the PostgreSQL backend, which stores no Server file.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The Storage Backend this deployment runs on.
    pub fn backend(&self) -> StorageBackend {
        match &self.backend {
            Backend::Sqlite(_) => StorageBackend::Sqlite,
            Backend::Postgres(_) => StorageBackend::Postgres,
        }
    }

    /// The Storage Backend in use, as the name the Admin surfaces publish.
    pub fn storage_backend(&self) -> &'static str {
        self.backend().as_str()
    }

    /// The Storage Backend facts the Owner-only Admin surfaces show. Public
    /// Projections, Public DTOs and SSE never carry them (ADR 0013).
    pub fn storage_facts(&self) -> StorageBackendFacts {
        StorageBackendFacts {
            backend: self.storage_backend(),
            postgres: self.postgres.clone(),
        }
    }

    /// A live `SELECT 1` probe: Doctor's connection check on the PostgreSQL
    /// backend, where no in-process integrity check exists.
    pub(crate) async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT 1")
            .fetch_optional(&self.pool)
            .await
            .map(|_| ())
    }

    /// The bytes the Storage Backend occupies, as `pg_database_size` reports
    /// them. `None` on SQLite, which publishes page accounting instead, and
    /// `None` when the server cannot answer: a missing figure is never
    /// published as zero (ADR 0013).
    pub(crate) async fn storage_database_bytes(&self) -> Option<i64> {
        match &self.backend {
            Backend::Sqlite(_) => None,
            Backend::Postgres(_) => postgres::database_bytes(&self.pool).await.ok(),
        }
    }

    /// The PostgreSQL server version, re-read at Doctor time instead of
    /// reusing the value the open path validated. PostgreSQL backend only.
    pub(crate) async fn postgres_server_version(&self) -> Result<(i64, String), sqlx::Error> {
        postgres::server_version(&self.pool).await
    }

    /// The text columns that do not use COLLATE "C", re-read at Doctor time:
    /// the offending count plus one description per column, capped so a
    /// single Doctor detail stays readable. PostgreSQL backend only.
    pub(crate) async fn postgres_collation_report(
        &self,
        limit: usize,
    ) -> Result<(usize, Vec<String>), sqlx::Error> {
        let wrong = postgres::collation_violations(&self.pool).await?;
        Ok((
            wrong.len(),
            postgres::describe_collation_violations(&wrong, limit),
        ))
    }

    /// Whether this process still exclusively owns the Storage Backend.
    pub fn ownership_held(&self) -> bool {
        match &self.backend {
            Backend::Sqlite(_) => true,
            Backend::Postgres(backend) => backend.ownership_held(),
        }
    }

    /// Resolve when this process stops owning the Storage Backend. SQLite
    /// ownership is the process-lifetime file lock the CLI holds, so this
    /// future never resolves there; the PostgreSQL session advisory lock can
    /// be lost with its connection, and the Server must then stop.
    pub async fn ownership_lost(&self) {
        match &self.backend {
            Backend::Sqlite(_) => std::future::pending().await,
            Backend::Postgres(backend) => backend.ownership_lost().await,
        }
    }

    /// The Inventory protocol mode resolved at startup (issue #192).
    pub fn inventory_cutover(&self) -> crate::cutover::InventoryCutover {
        self.inventory_cutover
    }

    /// Access the serialized SQLx pool for typed SQL operations.
    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// Read the required connection pragmas after startup. SQLite-only
    /// inspection (used by the startup tests); the PostgreSQL backend (S4)
    /// replaces it with its own session checks.
    pub async fn pragmas(&self) -> Result<SqlitePragmas, ServerDatabaseError> {
        match &self.backend {
            Backend::Sqlite(_) => sqlite::read_pragmas(&self.pool)
                .await
                .map_err(ServerDatabaseError::PragmaQuery),
            Backend::Postgres(_) => Err(ServerDatabaseError::Unsupported(
                "SQLite connection pragmas do not exist on the PostgreSQL Storage Backend"
                    .to_owned(),
            )),
        }
    }

    /// Return the highest migration version recorded in this Server database.
    pub async fn schema_version(&self) -> Result<i64, ServerDatabaseError> {
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(&self.pool)
            .await
            .map_err(ServerDatabaseError::IntegrityQuery)
    }

    /// Run a read-only integrity check under an explicit budget. Budget
    /// exhaustion is reported distinctly from a failed or inconsistent result
    /// (ADR 0008, issue #194).
    pub(crate) async fn bounded_integrity_check(
        &self,
        scope: IntegrityScope,
        budget: Duration,
    ) -> IntegrityVerdict {
        match &self.backend {
            Backend::Sqlite(backend) => {
                sqlite::bounded_integrity_check(&self.pool, backend, scope, budget).await
            }
            Backend::Postgres(_) => IntegrityVerdict::Unsupported,
        }
    }

    /// Page accounting for the metrics surface. PostgreSQL has no page
    /// accounting of its own, so the sizes stay unknown there (never zero).
    pub(crate) async fn storage_pages(&self) -> StoragePages {
        match &self.backend {
            Backend::Sqlite(_) => sqlite::storage_pages(&self.pool).await,
            Backend::Postgres(_) => StoragePages::default(),
        }
    }

    /// Flush committed writes into the main SQLite file before shutdown. The
    /// PostgreSQL backend has no write-ahead file the Server has to fold back
    /// in, so nothing is needed there.
    pub async fn checkpoint_wal(&self) -> Result<(), sqlx::Error> {
        match &self.backend {
            Backend::Sqlite(_) => sqlite::checkpoint_wal(&self.pool).await,
            Backend::Postgres(_) => Ok(()),
        }
    }

    /// Close the serialized pool. The operation is idempotent and can be
    /// called while the HTTP state still owns the database.
    ///
    /// SQLx's `Pool::close` can race a connection that is being returned at the
    /// same instant: the returning connection observes the pool as open just
    /// before it is marked closed, then lands in the idle set after `close` has
    /// already scanned it. The connection would stay open and, under the
    /// exclusive file locking a production deployment pins, keep the database
    /// locked for the next offline reader. Re-close until every connection is
    /// gone; each retry is bounded by the permits the previous `close` already
    /// waited for, so an idle connection is always reclaimed.
    pub async fn close(&self) {
        for _ in 0..8 {
            self.pool.close().await;
            if self.pool.size() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        // Closing the work connection must not leave the ownership session
        // behind: it is what the next Server waits for.
        if let Backend::Postgres(backend) = &self.backend {
            backend.release().await;
        }
    }
}

/// Initialize the Server database before constructing a listener.
pub async fn initialize(
    config: impl Into<StorageConfig>,
) -> Result<ServerDatabase, ServerDatabaseError> {
    ServerDatabase::open(config).await
}

/// Native SQLite connection options for a deployment's own connection policy.
///
/// The offline Storage Backend Migration (ADR 0013, issue #257) opens the
/// source file itself rather than through a pool, but it must still open it
/// exactly as the Server would: same pragmas, same exclusive locking mode.
pub(crate) fn sqlite_deployment_options(
    config: &ServerDatabaseConfig,
    create_if_missing: bool,
) -> sqlx::sqlite::SqliteConnectOptions {
    sqlite::connect_options(config, create_if_missing)
}

/// Whether an error is the Storage Backend reporting a corrupt database
/// (SQLite: primary or extended `SQLITE_CORRUPT` / `SQLITE_NOTADB`).
///
/// Dispatch is by the error's own driver type: each backend classifies only
/// errors it can downcast to its native database error, so a PostgreSQL
/// SQLSTATE that happens to share a numeric code is never read as SQLite.
pub(crate) fn is_corruption_error(error: &sqlx::Error) -> bool {
    sqlite::is_corruption_error(error) || postgres::is_corruption_error(error)
}

/// Defer foreign-key enforcement to the commit of the current transaction.
pub(crate) async fn defer_foreign_keys(connection: &mut AnyConnection) -> Result<(), sqlx::Error> {
    match connection.backend_name() {
        sqlite::BACKEND_NAME => sqlite::defer_foreign_keys(connection).await,
        postgres::BACKEND_NAME => postgres::defer_foreign_keys(connection).await,
        other => Err(sqlx::Error::Configuration(
            format!("deferred foreign keys are not implemented for {other}").into(),
        )),
    }
}

/// Number the positional `?` placeholders of a statement composed at run
/// time, in text order, as `$1`, `$2`, ...
///
/// Statement text written out in full spells its placeholders as `$N`, a form
/// both SQLite and PostgreSQL accept (ADR 0013). A statement assembled from
/// optional fragments cannot know its own numbering while it is written, so its
/// fragments use `?` and the assembled text is numbered once here, just before
/// it is prepared. Bind order is unchanged: the n-th `?` becomes `$n`.
/// Question marks inside quoted SQL text are left alone. The input must not
/// already contain numbered placeholders.
pub(crate) fn number_placeholders(sql: &str) -> String {
    let mut numbered = String::with_capacity(sql.len() + 8);
    let mut next = 0_usize;
    let mut quote: Option<char> = None;
    for character in sql.chars() {
        match (quote, character) {
            (Some(open), _) if character == open => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(character),
            (None, '?') => {
                next += 1;
                numbered.push('$');
                numbered.push_str(&next.to_string());
                continue;
            }
            (None, '$') => debug_assert!(
                false,
                "number_placeholders received text that is already numbered: {sql}"
            ),
            (None, _) => {}
        }
        numbered.push(character);
    }
    numbered
}

/// Number of `?` placeholders in one row of a batched statement.
///
/// Report Ingestion writes a variable number of rows per statement — a Peer
/// snapshot, its capabilities, the metrics a Report carries — because the
/// Server runs every write through a single connection (ADR 0013, issue #213),
/// so the round trips have to be proportional to the Report rather than to the
/// rows inside it. The text assembled here is numbered once with
/// [`number_placeholders`] before it is prepared.
pub(crate) fn row_placeholders(columns: usize, rows: usize) -> String {
    let mut values = String::with_capacity(rows * (columns.saturating_mul(2) + 3));
    for row in 0..rows {
        if row > 0 {
            values.push(',');
        }
        values.push('(');
        for column in 0..columns {
            if column > 0 {
                values.push(',');
            }
            values.push('?');
        }
        values.push(')');
    }
    values
}

/// The `?,?,?` body of a run-time sized `IN (...)` list.
///
/// The caller writes the parentheses around it and numbers the statement with
/// [`number_placeholders`].
pub(crate) fn placeholder_list(items: usize) -> String {
    let mut list = String::with_capacity(items.saturating_mul(2));
    for item in 0..items {
        if item > 0 {
            list.push(',');
        }
        list.push('?');
    }
    list
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use sqlx::migrate::Migrator;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn numbers_positional_placeholders_in_text_order() {
        assert_eq!(
            number_placeholders("SELECT a FROM t WHERE b = ? AND c IN (?, ?) LIMIT ?"),
            "SELECT a FROM t WHERE b = $1 AND c IN ($2, $3) LIMIT $4"
        );
        assert_eq!(number_placeholders("SELECT 1"), "SELECT 1");
    }

    #[test]
    fn leaves_question_marks_inside_quoted_text_alone() {
        assert_eq!(
            number_placeholders("SELECT '?', \"a?\", 'it''s ?' FROM t WHERE x = ?"),
            "SELECT '?', \"a?\", 'it''s ?' FROM t WHERE x = $1"
        );
    }

    #[tokio::test]
    async fn numbered_placeholders_bind_in_the_same_order_as_positional_ones() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let positional: (i64, String, i64) = sqlx::query_as("SELECT ?, ?, ?")
            .bind(1_i64)
            .bind("two")
            .bind(3_i64)
            .fetch_one(&pool)
            .await
            .unwrap();
        let numbered: (i64, String, i64) = sqlx::query_as(&number_placeholders("SELECT ?, ?, ?"))
            .bind(1_i64)
            .bind("two")
            .bind(3_i64)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(positional, numbered);
    }

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
    async fn confirmed_verdict_migration_preserves_metrics_and_does_not_guess_failed_history() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(SERVER_WRITE_CONNECTIONS)
            .connect_with(sqlite::connect_options(&config(&path), true))
            .await
            .unwrap();
        // Pin the actual upgrade boundary, not latest_schema - 1.
        migrations_through(71).run(&pool).await.unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Mainnet', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(&pool).await.unwrap();
        for outcome in [
            "success",
            "empty",
            "error",
            "not_found",
            "unsupported",
            "not_configured",
        ] {
            sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, created_at, updated_at) VALUES ($1, 'mainnet', $2, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
                .bind(outcome).bind(outcome).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, activity, last_attempt_received_at, last_good_received_at, stake_amount, counter_state, change_state, candidate_observations, updated_at) VALUES ($1, 'platscan', $2, 'active', '2026-02-02T00:00:00Z', '2026-01-01T00:00:00Z', '123456789000000000000', 'normal', 'normal', 0, '2026-02-02T00:00:00Z')")
                .bind(outcome).bind(outcome).execute(&pool).await.unwrap();
        }
        pool.close().await;
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        assert_eq!(
            database.schema_version().await.unwrap(),
            SERVER_SCHEMA_VERSION
        );
        for outcome in [
            "success",
            "empty",
            "error",
            "not_found",
            "unsupported",
            "not_configured",
        ] {
            let (kind, verdict_time, detail_time, stake): (Option<String>, Option<String>, String, String) =
                sqlx::query_as("SELECT last_good_verdict_outcome, last_good_verdict_received_at, last_good_received_at, stake_amount FROM current_validator_insights WHERE validator_id = $1")
                    .bind(outcome).fetch_one(database.pool()).await.unwrap();
            assert_eq!(detail_time, "2026-01-01T00:00:00Z");
            assert_eq!(stake, "123456789000000000000");
            match outcome {
                "success" => {
                    assert_eq!(kind.as_deref(), Some("success"));
                    assert_eq!(verdict_time.as_deref(), Some("2026-01-01T00:00:00Z"));
                }
                "empty" => {
                    assert_eq!(kind.as_deref(), Some("empty"));
                    assert_eq!(verdict_time.as_deref(), Some("2026-02-02T00:00:00Z"));
                }
                _ => {
                    assert_eq!(kind, None, "ambiguous failure is not proof of old Activity");
                    assert_eq!(verdict_time, None);
                }
            }
        }
    }

    #[tokio::test]
    async fn confirmed_verdict_migration_does_not_reconfirm_cached_activity_after_metric_only_success()
     {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(SERVER_WRITE_CONNECTIONS)
            .connect_with(sqlite::connect_options(&config(&path), true))
            .await
            .unwrap();
        migrations_through(71).run(&pool).await.unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Mainnet', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, created_at, updated_at) VALUES ('metric-only', 'mainnet', 'metric-only', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, activity, last_attempt_received_at, last_good_received_at, stake_amount, counter_state, change_state, candidate_observations, updated_at) VALUES ('metric-only', 'normalized-provider', 'success', 'active', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '123456789000000000000', 'normal', 'normal', 0, '2026-01-01T00:00:00Z')")
            .execute(&pool).await.unwrap();
        // Schema 71 discarded negative provenance but retained raw Activity.
        sqlx::query("UPDATE current_validator_insights SET outcome = 'empty', last_attempt_received_at = '2026-01-02T00:00:00Z' WHERE validator_id = 'metric-only'")
            .execute(&pool).await.unwrap();
        // A supported Success(None Activity) changed metrics and their receipt,
        // but that cached Active value is not a newly confirmed identity.
        sqlx::query("UPDATE current_validator_insights SET outcome = 'success', last_attempt_received_at = '2026-01-03T00:00:00Z', last_good_received_at = '2026-01-03T00:00:00Z', stake_amount = '234567890000000000000' WHERE validator_id = 'metric-only'")
            .execute(&pool).await.unwrap();
        pool.close().await;
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        let (kind, verdict_time, activity, detail_time, stake): (Option<String>, Option<String>, String, String, String) =
            sqlx::query_as("SELECT last_good_verdict_outcome, last_good_verdict_received_at, activity, last_good_received_at, stake_amount FROM current_validator_insights WHERE validator_id = 'metric-only'")
                .fetch_one(database.pool()).await.unwrap();
        assert_eq!(
            kind, None,
            "a legacy cached Activity is not proof of the latest success verdict"
        );
        assert_eq!(verdict_time, None);
        assert_eq!(
            activity, "active",
            "preserve cached data, but do not classify from it"
        );
        assert_eq!(detail_time, "2026-01-03T00:00:00Z");
        assert_eq!(stake, "234567890000000000000");
    }
    #[tokio::test]
    async fn existing_open_rejects_missing_database_without_creating_one() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("missing.db");
        let result = ServerDatabase::open_existing(config(&path)).await;
        assert!(result.is_err());
        assert!(!path.exists());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn restart_rejects_loose_or_symlinked_database_files() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        database.close().await;

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(matches!(
            ServerDatabase::open_existing(config(&path)).await,
            Err(ServerDatabaseError::Filesystem(_))
        ));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let real_path = directory.path().join("real-server.db");
        std::fs::rename(&path, &real_path).unwrap();
        symlink(&real_path, &path).unwrap();
        assert!(matches!(
            ServerDatabase::open_existing(config(&path)).await,
            Err(ServerDatabaseError::Filesystem(_))
        ));
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
            "INSERT INTO server_settings (setting_key, setting_value, updated_at) VALUES ($1, $2, $3)",
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
            sqlx::query_scalar("SELECT setting_value FROM server_settings WHERE setting_key = $1")
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
            .connect_with(sqlite::connect_options(&config(&path), true))
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

    /// Issue #132: an existing installation keeps its retained local MMDB
    /// results, their birth/expiry times, and its enabled Geo behaviour across
    /// the provider-keyed cache migration. Nothing is deleted, and the
    /// compatibility default follows the deployment's MMDB configuration.
    #[tokio::test]
    async fn geo_cache_migration_preserves_existing_local_results() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(SERVER_WRITE_CONNECTIONS)
            .connect_with(sqlite::connect_options(&config(&path), true))
            .await
            .unwrap();
        // Pinned to the schema immediately before the provider-keyed cache
        // migration (0042). Deriving this from SERVER_SCHEMA_VERSION would
        // silently start this test one migration later on every new
        // migration instead of exercising 0042's compatibility path.
        const SCHEMA_BEFORE_GEO_PROVIDER_CACHE: i64 = 41;
        migrations_through(SCHEMA_BEFORE_GEO_PROVIDER_CACHE)
            .run(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO geo_location_cache (canonical_ip, country_code, created_at, last_lookup_at, last_referenced_at, expires_at) VALUES ('8.8.4.4', 'US', '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z', '2026-01-03T00:00:00Z', '2026-01-02T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let database = ServerDatabase::open(config(&path)).await.unwrap();
        assert_eq!(
            database.schema_version().await.unwrap(),
            SERVER_SCHEMA_VERSION
        );
        let row = sqlx::query_as::<_, (String, String, String, String, String, String)>(
            "SELECT provider, country_code, state, created_at, last_attempt_at, last_referenced_at FROM geo_location_cache WHERE canonical_ip = '8.8.4.4'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(
            row,
            (
                "local_mmdb".to_owned(),
                "US".to_owned(),
                "current".to_owned(),
                "2026-01-01T00:00:00Z".to_owned(),
                "2026-01-02T00:00:00Z".to_owned(),
                "2026-01-03T00:00:00Z".to_owned(),
            )
        );
        let retained: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT last_success_at, expires_at FROM geo_location_cache WHERE canonical_ip = '8.8.4.4'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(
            retained,
            (
                Some("2026-01-02T00:00:00Z".to_owned()),
                Some("2026-01-02T00:00:00Z".to_owned())
            )
        );
        // The durable selection is resolved from the deployment's MMDB
        // configuration: an installation that had one keeps resolving
        // locally, and the decision is durable.
        let enabled = crate::geo::ensure_provider_selection(database.pool(), true)
            .await
            .unwrap();
        assert_eq!(enabled.provider, crate::geo::GeoProvider::LocalMmdb);
        assert_eq!(enabled.generation, 1);
        assert_eq!(
            crate::geo::ensure_provider_selection(database.pool(), false)
                .await
                .unwrap(),
            enabled
        );
        database.close().await;

        // An installation without a local database stays Disabled and is
        // never silently switched to an outbound provider.
        let other = tempdir().unwrap();
        let other_database = ServerDatabase::open(config(&other.path().join("server.db")))
            .await
            .unwrap();
        let disabled = crate::geo::ensure_provider_selection(other_database.pool(), false)
            .await
            .unwrap();
        assert_eq!(disabled.provider, crate::geo::GeoProvider::Disabled);
        assert_eq!(disabled.generation, 1);
    }

    /// Issue #214: before the aggregate tiers existed the retention catalog
    /// declared one_minute_aggregate as a family that was never produced, and
    /// the seeding wrote its placeholder row (90 days, unsupported) on every
    /// database. Pinned to the schema immediately before migration 0067 so the
    /// upgrade path is exercised: the never-produced placeholder becomes the
    /// delivered 7-day tier contract, and so does a row an Owner had pointed at
    /// one of the old invented windows. The 1-minute tier is served for exactly
    /// [now - 7d, now - 24h], so a stored 30-day window is not a setting the
    /// Server can honour - it is only a licence for the cleanup to retain
    /// unreachable buckets - and the edit stays visible in audit_events instead.
    #[tokio::test]
    async fn aggregate_tier_migration_aligns_the_never_produced_policy_placeholder() {
        // The schema immediately before the aggregate tiers (0067). Deriving it
        // from SERVER_SCHEMA_VERSION would silently start this test one
        // migration later instead of exercising 0067's upgrade path.
        const SCHEMA_BEFORE_AGGREGATE_TIERS: i64 = 66;

        async fn upgraded(
            directory: &tempfile::TempDir,
            retention_days: i64,
            updated_by: &str,
        ) -> ServerDatabase {
            let path = directory.path().join("server.db");
            let pool = SqlitePoolOptions::new()
                .max_connections(SERVER_WRITE_CONNECTIONS)
                .connect_with(sqlite::connect_options(&config(&path), true))
                .await
                .unwrap();
            migrations_through(SCHEMA_BEFORE_AGGREGATE_TIERS)
                .run(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO retention_policies (family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by) VALUES ('one_minute_aggregate', $1, 7, 365, 0, 1, '2026-01-01T00:00:00Z', $2)",
            )
            .bind(retention_days)
            .bind(updated_by)
            .execute(&pool)
            .await
            .unwrap();
            pool.close().await;
            ServerDatabase::open(config(&path)).await.unwrap()
        }

        async fn policy(database: &ServerDatabase) -> (i64, i64, i64, i64) {
            sqlx::query_as(
                "SELECT retention_days, min_days, max_days, supported FROM retention_policies WHERE family = 'one_minute_aggregate'",
            )
            .fetch_one(database.pool())
            .await
            .unwrap()
        }

        // The untouched placeholder: the upgrade registers the tier it now
        // really produces, with its own fixed window.
        let untouched = tempdir().unwrap();
        let database = upgraded(&untouched, 90, "defaults").await;
        assert_eq!(
            database.schema_version().await.unwrap(),
            SERVER_SCHEMA_VERSION
        );
        assert_eq!(policy(&database).await, (7, 7, 7, 1));
        database.close().await;

        // An Owner who had set the old window is normalised too: the tier's
        // window is its contract, and the value that was edited was one of the
        // placeholders invented for a family the Server did not produce yet.
        let edited = tempdir().unwrap();
        let database = upgraded(&edited, 30, "00000000-0000-4000-8000-000000000001").await;
        assert_eq!(policy(&database).await, (7, 7, 7, 1));
        database.close().await;
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

    /// Issue #137: a production Server owns its SQLite file, so the
    /// connection must hold exclusive locking. A concurrent external SQLite
    /// connection that becomes the first attacher truncates `-shm` to 3
    /// bytes (see `unixShmLock` in SQLite), and this process then takes
    /// SIGBUS inside `walFindFrame` while the file is still mapped.
    #[tokio::test]
    async fn production_configuration_holds_exclusive_locking() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        assert_eq!(locking_mode(&database).await, "exclusive");
        // A deployment without development mode resolves to the same
        // production ownership.
        database.close().await;
        let production = ServerDatabase::open(ServerDatabaseConfig::for_deployment(&path, false))
            .await
            .unwrap();
        assert_eq!(locking_mode(&production).await, "exclusive");
    }

    /// Development mode is the deliberate exception: the browser e2e fixture
    /// refreshers and local tooling write to a running dev Server, and an
    /// exclusively locked connection would never observe those commits.
    #[tokio::test]
    async fn development_configuration_shares_the_database_with_external_writers() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(ServerDatabaseConfig::for_deployment(&path, true))
            .await
            .unwrap();
        assert_eq!(locking_mode(&database).await, "normal");

        // An external connection that is not the Server writes and commits;
        // the Server must read that committed row through its own pool.
        let external = external_pool(&path).await;
        sqlx::query(
            "INSERT INTO server_settings (setting_key, setting_value, updated_at) VALUES ('external', '1', '2026-01-01T00:00:00Z')",
        )
        .execute(&external)
        .await
        .unwrap();
        let observed: Option<String> = sqlx::query_scalar(
            "SELECT setting_value FROM server_settings WHERE setting_key = 'external'",
        )
        .fetch_optional(database.pool())
        .await
        .unwrap();
        assert_eq!(observed.as_deref(), Some("1"));
        external.close().await;
    }

    /// Connection setup cannot wait on SQLite's busy handler for the
    /// exclusive lock, so a concurrent external writer must be absorbed by a
    /// bounded retry instead of stopping Server startup.
    #[tokio::test]
    async fn open_retries_while_an_external_writer_holds_the_database() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let seed = ServerDatabase::open(config(&path)).await.unwrap();
        seed.close().await;

        let external = external_pool(&path).await;
        let mut writer = external.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO server_settings (setting_key, setting_value, updated_at) VALUES ('writer', '1', '2026-01-01T00:00:00Z')",
        )
        .execute(&mut *writer)
        .await
        .unwrap();
        let holder = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            writer.commit().await.unwrap();
            external.close().await;
        });

        let database = ServerDatabase::open(
            ServerDatabaseConfig::new(&path).with_busy_timeout(Duration::from_secs(5)),
        )
        .await
        .unwrap();
        let committed: Option<String> = sqlx::query_scalar(
            "SELECT setting_value FROM server_settings WHERE setting_key = 'writer'",
        )
        .fetch_optional(database.pool())
        .await
        .unwrap();
        assert_eq!(committed.as_deref(), Some("1"));
        holder.await.unwrap();
    }

    async fn locking_mode(database: &ServerDatabase) -> String {
        let mode: String = sqlx::query_scalar("PRAGMA locking_mode")
            .fetch_one(database.pool())
            .await
            .unwrap();
        mode.to_ascii_lowercase()
    }

    /// A second process-like connection to the same file, using SQLite's
    /// ordinary locking protocol and no Server policy.
    async fn external_pool(path: &Path) -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .busy_timeout(Duration::from_secs(5)),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn component_status_is_scoped_per_node() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        sqlx::query(
            "INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
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
            "INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ($1, $2, $3, $4)",
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
                "INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, inventory_revision, first_seen_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
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
                "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, state_revision, value_revision) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
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
            "SELECT COUNT(*) FROM component_status WHERE agent_id = $1 AND component_key = $2",
        )
        .bind("agent-1")
        .bind("rpc")
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn schema_does_not_precreate_unimplemented_future_table_families() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let database = ServerDatabase::open(config(&path)).await.unwrap();
        let forbidden = ["peers", "block_aggregate_1m", "block_aggregate_1h"];

        for table in forbidden {
            let exists =
                sqlx::query("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = $1")
                    .bind(table)
                    .fetch_optional(database.pool())
                    .await
                    .unwrap()
                    .is_some();
            assert!(!exists, "future table {table:?} was pre-created");
        }
    }

    /// Issue #175: the deletion/notification migration rebuilds the Delivery
    /// table to widen its state CHECK. Every stored Delivery and attempt fact
    /// must survive verbatim, the new `cancelled` state must be accepted, and
    /// the Incident deletion annotation column must exist.
    #[tokio::test]
    async fn subject_deletion_migration_preserves_delivery_and_attempt_facts() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("server.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(SERVER_WRITE_CONNECTIONS)
            .connect_with(sqlite::connect_options(&config(&path), true))
            .await
            .unwrap();
        migrations_through(53).run(&pool).await.unwrap();
        sqlx::query("INSERT INTO notification_events (event_id, event_kind, incident_id, rule_key, subject_kind, subject_key, severity, summary, created_at) VALUES ('ev-1', 'incident', NULL, 'node.rpc_unreachable', 'node', 'node-a', 'warning', 'Incident opened', '2026-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO notification_deliveries (delivery_id, event_id, channel_kind, destination, state, attempt_count, next_attempt_at, last_attempt_at, last_result, last_error_kind, retry_after_seconds, created_at, updated_at) VALUES ('dl-1', 'ev-1', 'telegram', '****6789', 'dead_letter', 2, NULL, '2026-01-01T00:00:01Z', 'telegram_api_error 429', 'telegram_api', NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:01Z')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO delivery_attempts (attempt_id, delivery_id, attempt_number, attempted_at, outcome, provider_result, error_kind, duration_ms, retry_after_seconds) VALUES ('at-1', 'dl-1', 1, '2026-01-01T00:00:01Z', 'failed', 'telegram_api_error 429', 'telegram_api', NULL, NULL)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let database = ServerDatabase::open(config(&path)).await.unwrap();
        assert_eq!(
            database.schema_version().await.unwrap(),
            SERVER_SCHEMA_VERSION
        );
        let delivery: (String, i64, Option<String>) = sqlx::query_as(
            "SELECT state, attempt_count, last_result FROM notification_deliveries WHERE delivery_id = 'dl-1'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(
            delivery,
            (
                "dead_letter".to_owned(),
                2,
                Some("telegram_api_error 429".to_owned())
            )
        );
        let attempts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM delivery_attempts WHERE delivery_id = 'dl-1'")
                .fetch_one(database.pool())
                .await
                .unwrap();
        assert_eq!(
            attempts, 1,
            "attempt history must survive the Delivery table rebuild"
        );
        // The widened CHECK accepts the new terminal state.
        sqlx::query(
            "UPDATE notification_deliveries SET state = 'cancelled' WHERE delivery_id = 'dl-1'",
        )
        .execute(database.pool())
        .await
        .unwrap();
        // The Incident deletion annotation column exists and starts unset.
        let annotation: Option<String> =
            sqlx::query_scalar("SELECT subject_deleted_at FROM alert_incidents LIMIT 1")
                .fetch_optional(database.pool())
                .await
                .unwrap()
                .flatten();
        assert_eq!(annotation, None);
        database.close().await;
    }

    #[test]
    fn batched_statement_text_is_numbered_in_bind_order() {
        assert_eq!(row_placeholders(3, 2), "(?,?,?),(?,?,?)");
        assert_eq!(row_placeholders(1, 1), "(?)");
        assert_eq!(row_placeholders(2, 0), "");
        assert_eq!(placeholder_list(3), "?,?,?");
        assert_eq!(placeholder_list(0), "");
        assert_eq!(
            number_placeholders(&format!(
                "INSERT INTO t (a, b) VALUES {}",
                row_placeholders(2, 2)
            )),
            "INSERT INTO t (a, b) VALUES ($1,$2),($3,$4)"
        );
    }
}
