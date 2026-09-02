//! Agent-local SQLite startup and migration harness.
//!
//! The Agent owns one SQLite write connection. Collection is only allowed to
//! start after [`AgentStore::open`] has completed migrations, pragma checks,
//! and the integrity check. The migration source is deliberately local to
//! this crate; it is not shared with the Server.

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Connection, Executor, Sqlite, SqliteConnection};
use thiserror::Error;
#[cfg(test)]
use tokio::sync::Notify;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The Agent's embedded migration source.
///
/// This is intentionally a different static from the Server migrator, even
/// though both databases use SQLx's default `_sqlx_migrations` table.
pub static AGENT_MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The latest migration version compiled into the Agent binary.
pub const AGENT_SCHEMA_VERSION: i64 = 13;

/// Applied Receipt Records are retained for a recent duplicate/conflict window.
pub(crate) const APPLIED_RECEIPT_RETENTION: time::Duration = time::Duration::hours(24);
/// Every cleanup invocation is fixed-size, including receipt application and startup.
pub(crate) const APPLIED_RECEIPT_EXPIRY_BATCH_SIZE: i64 = 64;

/// Explicit timeout used for SQLite lock contention unless a caller chooses a
/// tighter or more generous value for a test/deployment.
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// SQLite BUSY/LOCKED result codes are transient contention, including
/// extended result codes whose low byte is the primary code.
pub(crate) fn is_lock_contention(error: &sqlx::Error) -> bool {
    let sqlx::Error::Database(error) = error else {
        return false;
    };
    error
        .code()
        .and_then(|code| code.parse::<i32>().ok())
        .is_some_and(|code| matches!(code & 0xff, 5 | 6))
}

const REQUIRED_TABLES: &[&str] = &[
    "agent_state",
    "pending_block_summaries",
    "history_gaps",
    "block_summaries",
    "reports",
    "report_receipts",
    "rejection_ledger",
    "delivery_diagnostics",
    "spool_state",
    "node_recovery_state",
    "agent_boots",
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

/// A write permit shared explicitly by the purpose-specific Agent Store
/// connections that participate in one runtime. Writes acquire it before
/// beginning their transaction or autocommit statement.
#[derive(Clone)]
pub(crate) struct AgentStoreWritePermit {
    semaphore: Arc<Semaphore>,
    #[cfg(test)]
    acquisition_attempts: Arc<AtomicUsize>,
    #[cfg(test)]
    acquisition_notify: Arc<Notify>,
}

impl AgentStoreWritePermit {
    pub(crate) fn new() -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(1)),
            #[cfg(test)]
            acquisition_attempts: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            acquisition_notify: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        self.semaphore.clone().try_acquire_owned().ok()
    }

    pub(crate) async fn acquire(&self) -> OwnedSemaphorePermit {
        #[cfg(test)]
        {
            self.acquisition_attempts.fetch_add(1, Ordering::SeqCst);
            self.acquisition_notify.notify_one();
        }
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("Agent Store write permit is never closed")
    }

    #[cfg(test)]
    pub(crate) fn test_acquisition_attempts(&self) -> usize {
        self.acquisition_attempts.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn test_acquisition_notify(&self) -> Arc<Notify> {
        self.acquisition_notify.clone()
    }
}

impl Default for AgentStoreWritePermit {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors raised while establishing the operating-system runtime ownership
/// lock. The lock is advisory and held by an open file descriptor, so process
/// termination releases ownership without a PID file or stale cleanup.
#[derive(Debug, Error)]
pub enum AgentRuntimeLockError {
    #[error("failed to open Agent runtime lock {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Agent runtime lock {path} must be a private regular file owned by the Agent user")]
    Unsafe { path: PathBuf },
    #[error("Agent Store is already owned by another runtime: {path}")]
    AlreadyOwned { path: PathBuf },
    #[error("failed to acquire Agent runtime lock {path}: {source}")]
    Acquire {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Agent runtime ownership locks are unsupported on this platform")]
    Unsupported,
}

/// An operating-system lock held for the complete Agent runtime lifecycle.
#[derive(Debug)]
pub struct AgentRuntimeLock {
    #[cfg(unix)]
    #[allow(dead_code)]
    file: nix::fcntl::Flock<std::fs::File>,
    #[cfg(not(unix))]
    #[allow(dead_code)]
    file: std::fs::File,
    path: PathBuf,
}

impl AgentRuntimeLock {
    pub fn path_for(state_db: &Path) -> PathBuf {
        let mut path = state_db.as_os_str().to_os_string();
        path.push(".lock");
        PathBuf::from(path)
    }

    pub fn acquire(state_db: impl AsRef<Path>) -> Result<Self, AgentRuntimeLockError> {
        #[cfg(unix)]
        let path = runtime_lock_path(state_db.as_ref())?;
        #[cfg(not(unix))]
        let path = Self::path_for(state_db.as_ref());
        acquire_runtime_lock(path)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn runtime_lock_path(state_db: &Path) -> Result<PathBuf, AgentRuntimeLockError> {
    use std::os::unix::fs::MetadataExt;

    let configured_metadata = match std::fs::symlink_metadata(state_db) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = state_db.parent().unwrap_or_else(|| Path::new("."));
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            let file_name = state_db
                .file_name()
                .ok_or_else(|| AgentRuntimeLockError::Open {
                    path: AgentRuntimeLock::path_for(state_db),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "state database path has no file name",
                    ),
                })?;
            let canonical_parent =
                std::fs::canonicalize(parent).map_err(|source| AgentRuntimeLockError::Open {
                    path: AgentRuntimeLock::path_for(state_db),
                    source,
                })?;
            let database = canonical_parent.join(file_name);
            let mut lock = database.as_os_str().to_os_string();
            lock.push(".lock");
            return Ok(PathBuf::from(lock));
        }
        Err(source) => {
            return Err(AgentRuntimeLockError::Open {
                path: AgentRuntimeLock::path_for(state_db),
                source,
            });
        }
    };
    if !configured_metadata.file_type().is_file() || configured_metadata.nlink() > 1 {
        return Err(AgentRuntimeLockError::Unsafe {
            path: AgentRuntimeLock::path_for(state_db),
        });
    }
    let canonical =
        std::fs::canonicalize(state_db).map_err(|source| AgentRuntimeLockError::Open {
            path: AgentRuntimeLock::path_for(state_db),
            source,
        })?;
    let canonical_metadata =
        std::fs::metadata(&canonical).map_err(|source| AgentRuntimeLockError::Open {
            path: AgentRuntimeLock::path_for(state_db),
            source,
        })?;
    if !canonical_metadata.file_type().is_file() || canonical_metadata.nlink() > 1 {
        return Err(AgentRuntimeLockError::Unsafe {
            path: AgentRuntimeLock::path_for(state_db),
        });
    }
    let mut lock = canonical.as_os_str().to_os_string();
    lock.push(".lock");
    Ok(PathBuf::from(lock))
}

#[cfg(unix)]
fn acquire_runtime_lock(path: PathBuf) -> Result<AgentRuntimeLock, AgentRuntimeLockError> {
    use nix::errno::Errno;
    use nix::fcntl::{Flock, FlockArg, OFlag};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    restrict_umask();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags((OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .mode(0o600)
        .open(&path)
        .map_err(|source| AgentRuntimeLockError::Open {
            path: path.clone(),
            source,
        })?;
    let metadata = file
        .metadata()
        .map_err(|source| AgentRuntimeLockError::Open {
            path: path.clone(),
            source,
        })?;
    if !metadata.file_type().is_file()
        || metadata.nlink() > 1
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(AgentRuntimeLockError::Unsafe { path });
    }
    match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(file) => Ok(AgentRuntimeLock { file, path }),
        Err((_, Errno::EAGAIN)) => Err(AgentRuntimeLockError::AlreadyOwned { path }),
        Err((_, error)) => Err(AgentRuntimeLockError::Acquire {
            path,
            source: std::io::Error::from_raw_os_error(error as i32),
        }),
    }
}

#[cfg(not(unix))]
fn acquire_runtime_lock(_path: PathBuf) -> Result<AgentRuntimeLock, AgentRuntimeLockError> {
    Err(AgentRuntimeLockError::Unsupported)
}

/// An initialized Agent Store with exactly one writer connection.
///
/// No collection code is run by this type. Callers receive the store only
/// after startup validation succeeds, then pass its single connection to the
/// collection/store operations that need it.
pub struct AgentStore {
    connection: SqliteConnection,
    write_permit: AgentStoreWritePermit,
}

impl AgentStore {
    /// Open the Agent database, migrate it, validate required pragmas, and
    /// run integrity checks before returning a store to the collector.
    #[cfg(test)]
    pub(crate) async fn open(config: AgentDatabaseConfig) -> Result<Self, AgentDatabaseError> {
        Self::open_with_write_permit(config, AgentStoreWritePermit::new()).await
    }

    /// Open a purpose-specific connection sharing the caller's explicitly
    /// injected write permit with the other Agent Store connections.
    pub(crate) async fn open_with_write_permit(
        config: AgentDatabaseConfig,
        write_permit: AgentStoreWritePermit,
    ) -> Result<Self, AgentDatabaseError> {
        // Design §8.2: the credential file AND the state DB must only allow
        // the Agent OS user to read. Umask 077 keeps SQLite WAL/SHM
        // siblings private; the explicit 0600 chmod below pins the file
        // itself even when a permissive umask was inherited.
        restrict_umask();
        let _write_permit = write_permit.acquire().await;
        let options = sqlite_options(&config);
        let mut connection = SqliteConnection::connect_with(&options)
            .await
            .map_err(AgentDatabaseError::Connect)?;

        AGENT_MIGRATOR
            .run_direct(&mut connection)
            .await
            .map_err(AgentDatabaseError::Migration)?;
        drop(_write_permit);

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
        let mut store = Self {
            connection,
            write_permit,
        };
        cleanup_expired_receipt_markers(&mut store, &now_rfc3339())
            .await
            .map_err(AgentDatabaseError::IntegrityQuery)?;
        secure_store_file(config.path())?;
        Ok(store)
    }

    /// Acquire the injected permit before any Agent Store write.
    pub(crate) async fn acquire_write(&self) -> OwnedSemaphorePermit {
        self.write_permit.acquire().await
    }

    /// Access the sole Agent write connection for typed SQL operations.
    pub(crate) fn connection(&mut self) -> &mut SqliteConnection {
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

/// Calculate the exclusive receipt-marker expiry boundary from application time.
pub(crate) fn applied_receipt_expiry_cutoff(applied_at: &str) -> Result<String, sqlx::Error> {
    let applied_at =
        time::OffsetDateTime::parse(applied_at, &time::format_description::well_known::Rfc3339)
            .map_err(|error| {
                sqlx::Error::Protocol(format!("invalid receipt application time: {error}"))
            })?;
    applied_at
        .checked_sub(APPLIED_RECEIPT_RETENTION)
        .ok_or_else(|| sqlx::Error::Protocol("receipt expiry cutoff is out of range".to_owned()))?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| sqlx::Error::Protocol(format!("invalid receipt expiry cutoff: {error}")))
}

/// Run one bounded expiry batch outside a receipt-application transaction.
///
/// Startup executes this once, and subsequent delivery ticks continue draining
/// the same fixed-size batches without making startup depend on receipt history.
pub(crate) async fn cleanup_expired_receipt_markers(
    store: &mut AgentStore,
    applied_at: &str,
) -> Result<u64, sqlx::Error> {
    let cutoff = applied_receipt_expiry_cutoff(applied_at)?;
    let _write_permit = store.acquire_write().await;
    delete_expired_receipt_markers(store.connection(), &cutoff).await
}

/// Delete at most one indexed batch of expired Applied Receipt Records.
pub(crate) async fn delete_expired_receipt_markers<'e, E>(
    executor: E,
    cutoff: &str,
) -> Result<u64, sqlx::Error>
where
    E: Executor<'e, Database = Sqlite>,
{
    Ok(sqlx::query(
        "DELETE FROM report_receipts WHERE report_id IN (SELECT report_id FROM report_receipts WHERE applied_at < ? ORDER BY applied_at, report_id LIMIT ?)",
    )
    .bind(cutoff)
    .bind(APPLIED_RECEIPT_EXPIRY_BATCH_SIZE)
    .execute(executor)
    .await?
    .rows_affected())
}

/// Return the Agent-local application time in canonical UTC form.
pub(crate) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("zero nanoseconds is valid")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC timestamp is valid")
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

    #[cfg(unix)]
    #[test]
    fn runtime_lock_is_exclusive_and_releases_with_the_file_handle() {
        use std::os::unix::fs::MetadataExt;

        let directory = tempdir().unwrap();
        let database = directory.path().join("agent.db");
        let first = AgentRuntimeLock::acquire(&database).unwrap();
        assert_eq!(first.path(), AgentRuntimeLock::path_for(&database));
        assert_eq!(
            std::fs::metadata(first.path()).unwrap().mode() & 0o777,
            0o600
        );
        assert!(matches!(
            AgentRuntimeLock::acquire(&database),
            Err(AgentRuntimeLockError::AlreadyOwned { .. })
        ));
        drop(first);
        let second = AgentRuntimeLock::acquire(&database).unwrap();
        drop(second);
    }

    #[cfg(unix)]
    #[test]
    fn runtime_lock_rejects_unsafe_existing_artifacts() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let database = directory.path().join("agent.db");
        let lock_path = AgentRuntimeLock::path_for(&database);
        std::fs::write(&lock_path, b"").unwrap();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let error = AgentRuntimeLock::acquire(&database).unwrap_err();
        assert!(
            matches!(error, AgentRuntimeLockError::Unsafe { .. }),
            "{error:?}"
        );

        std::fs::remove_file(&lock_path).unwrap();
        std::fs::create_dir(&lock_path).unwrap();
        assert!(matches!(
            AgentRuntimeLock::acquire(&database),
            Err(AgentRuntimeLockError::Unsafe { .. }) | Err(AgentRuntimeLockError::Open { .. })
        ));

        std::fs::remove_dir(&lock_path).unwrap();
        let hard_link_target = directory.path().join("lock-hard-link-target");
        std::fs::write(&hard_link_target, b"").unwrap();
        std::fs::hard_link(&hard_link_target, &lock_path).unwrap();
        assert!(matches!(
            AgentRuntimeLock::acquire(&database),
            Err(AgentRuntimeLockError::Unsafe { .. })
        ));

        std::fs::remove_file(&lock_path).unwrap();
        let target = directory.path().join("lock-target");
        std::fs::write(&target, b"").unwrap();
        std::os::unix::fs::symlink(&target, &lock_path).unwrap();
        assert!(matches!(
            AgentRuntimeLock::acquire(&database),
            Err(AgentRuntimeLockError::Open { .. }) | Err(AgentRuntimeLockError::Unsafe { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn runtime_lock_rejects_database_path_aliases() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("agent.db");
        std::fs::write(&database, b"").unwrap();

        let symlink = directory.path().join("symlink.db");
        std::os::unix::fs::symlink(&database, &symlink).unwrap();
        assert!(matches!(
            AgentRuntimeLock::acquire(&symlink),
            Err(AgentRuntimeLockError::Unsafe { .. })
        ));

        let hard_link = directory.path().join("hard-link.db");
        std::fs::hard_link(&database, &hard_link).unwrap();
        assert!(matches!(
            AgentRuntimeLock::acquire(&database),
            Err(AgentRuntimeLockError::Unsafe { .. })
        ));
        assert!(matches!(
            AgentRuntimeLock::acquire(&hard_link),
            Err(AgentRuntimeLockError::Unsafe { .. })
        ));
    }

    #[test]
    fn runtime_lock_accepts_fresh_bare_relative_database_path() {
        let database = format!("platpulse-agent-test-{}.db", uuid::Uuid::new_v4());
        let lock_path = format!("{database}.lock");
        let lock = AgentRuntimeLock::acquire(&database).unwrap();
        assert!(lock.path().ends_with(&lock_path));
        drop(lock);
        std::fs::remove_file(&lock_path).unwrap();
    }

    #[tokio::test]
    async fn explicitly_shared_write_permit_serializes_store_connections() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("agent.db");
        let permit = AgentStoreWritePermit::new();
        let first = AgentStore::open_with_write_permit(config(&database), permit.clone())
            .await
            .unwrap();
        let second = AgentStore::open_with_write_permit(config(&database), permit)
            .await
            .unwrap();
        let held = first.acquire_write().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), second.acquire_write())
                .await
                .is_err()
        );
        drop(held);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), second.acquire_write())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn default_store_connections_do_not_share_hidden_write_permits() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("agent.db");
        let first = AgentStore::open(config(&database)).await.unwrap();
        let second = AgentStore::open(config(&database)).await.unwrap();
        let held = first.acquire_write().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), second.acquire_write())
                .await
                .is_ok()
        );
        drop(held);
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
    async fn receipt_archive_migration_and_repeated_startup_cleanup_are_bounded() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("agent.db");
        let options = sqlite_options(&config(&path));
        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        migrations_through(12)
            .run_direct(&mut connection)
            .await
            .unwrap();
        for index in 0..300_u128 {
            sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, receipt_body, applied_at) VALUES (?, ?, 'accepted', ?, ?)")
                .bind(uuid::Uuid::from_u128(index + 1).to_string())
                .bind("0x0000000000000000000000000000000000000000000000000000000000000000")
                .bind(br#"{"receipt":{}}"# as &[u8])
                .bind(format!("2026-01-01T00:00:{index:02}Z"))
                .execute(&mut connection)
                .await
                .unwrap();
        }
        connection.close().await.unwrap();

        let mut store = AgentStore::open(config(&path)).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM report_receipts")
            .fetch_one(store.connection())
            .await
            .unwrap();
        let body_columns: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('report_receipts') WHERE name='receipt_body'",
        )
        .fetch_one(store.connection())
        .await
        .unwrap();
        assert_eq!(count, 192);
        assert_eq!(body_columns, 0);
        store.close().await.unwrap();

        for expected_count in [128, 64, 0] {
            let mut reopened = AgentStore::open(config(&path)).await.unwrap();
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM report_receipts")
                .fetch_one(reopened.connection())
                .await
                .unwrap();
            assert_eq!(count, expected_count);
            reopened.close().await.unwrap();
        }
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
