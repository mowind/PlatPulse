//! The SQLite Storage Backend (ADR 0013).
//!
//! Everything that only exists because the Server stores its data in a SQLite
//! file lives here: the file's ownership and permission checks, the
//! connection-local PRAGMAs, startup integrity, the bounded interruptible
//! integrity queries, result-code classification and page accounting. The
//! domain modules reach the database through the backend-neutral
//! [`sqlx::AnyPool`] owned by [`super::ServerDatabase`] and never see these
//! details.
//!
//! The pool is still a single serialized connection. SQLx's Any driver hides
//! the native SQLite handle, so the progress-handler interruption that bounds
//! an integrity check runs on a short-lived native maintenance connection: the
//! check first takes the pool's only connection, closes it (releasing the
//! file), and holds [`SqliteBackend::gate`] until its native connection is
//! closed again, so no replacement pool connection can configure itself (and
//! take the exclusive file lock) in between.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
use sqlx::error::DatabaseError;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteConnection, SqliteError, SqliteJournalMode, SqliteLockingMode,
    SqliteSynchronous,
};
use sqlx::{AnyConnection, AnyPool, ConnectOptions, Connection};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

use super::{
    IntegrityScope, IntegrityVerdict, REQUIRED_TABLES, SERVER_WRITE_CONNECTIONS,
    ServerDatabaseConfig, ServerDatabaseError, StoragePages,
};

/// The name SQLx's Any driver reports for a SQLite connection.
pub(super) const BACKEND_NAME: &str = "SQLite";

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

/// SQLite-only state the Server keeps next to its pool.
pub(super) struct SqliteBackend {
    /// Native options for maintenance connections; never creates the file.
    maintenance: SqliteConnectOptions,
    busy_timeout: Duration,
    /// Held for writing while a native maintenance connection owns the file;
    /// a new pool connection waits on it before taking any file lock.
    gate: Arc<RwLock<()>>,
}

/// Prepare the file, open the serialized pool, and secure the files SQLite
/// created.
pub(super) async fn open(
    config: &ServerDatabaseConfig,
    create_if_missing: bool,
) -> Result<(AnyPool, SqliteBackend), ServerDatabaseError> {
    prepare_database_path(config, create_if_missing)?;
    let gate = Arc::new(RwLock::new(()));
    let pool = connect_pool(config, create_if_missing, Arc::clone(&gate)).await?;

    // SQLite creates the main file and WAL sidecars with the process
    // umask. Secure files created by this startup path before exposing
    // the database to the rest of the Server; pre-existing unsafe files
    // were rejected by `prepare_database_path` above.
    if let Err(error) = secure_database_files(config.path(), create_if_missing) {
        pool.close().await;
        return Err(error);
    }
    Ok((
        pool,
        SqliteBackend {
            maintenance: connect_options(config, false),
            busy_timeout: config.busy_timeout(),
            gate,
        },
    ))
}

/// Startup checks that follow migration: required PRAGMAs, the bounded
/// whole-database integrity check, foreign keys and the required tables.
pub(super) async fn verify_startup(
    pool: &AnyPool,
    backend: &SqliteBackend,
    config: &ServerDatabaseConfig,
    created_by_startup: bool,
) -> Result<(), ServerDatabaseError> {
    let pragmas = read_pragmas(pool)
        .await
        .map_err(ServerDatabaseError::PragmaQuery)?;
    if !pragmas.satisfy_requirements() {
        return Err(ServerDatabaseError::PragmaMismatch(format!(
            "foreign_keys={}, journal_mode={:?}, busy_timeout_ms={}, synchronous={}",
            pragmas.foreign_keys,
            pragmas.journal_mode,
            pragmas.busy_timeout_ms,
            pragmas.synchronous
        )));
    }
    verify_integrity(pool, backend).await?;
    secure_database_files(config.path(), created_by_startup)
}

fn prepare_database_path(
    config: &ServerDatabaseConfig,
    create_if_missing: bool,
) -> Result<(), ServerDatabaseError> {
    let path = config.path();
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    crate::file_security::validate_private_directory(parent)
        .map_err(|error| ServerDatabaseError::Filesystem(format!("database parent: {error}")))?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.len() == 0 {
                return Err(ServerDatabaseError::EmptyDatabase);
            }
            if create_if_missing {
                crate::file_security::validate_owned_regular_file(path).map_err(|error| {
                    ServerDatabaseError::Filesystem(format!("database file: {error}"))
                })?;
            } else {
                crate::file_security::validate_file(path).map_err(|error| {
                    ServerDatabaseError::Filesystem(format!("database file: {error}"))
                })?;
            };
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create_if_missing => {}
        Err(_) => {
            return Err(ServerDatabaseError::Filesystem(
                "cannot inspect the Server SQLite path".to_owned(),
            ));
        }
    }
    validate_existing_database_sidecars(path)?;
    Ok(())
}

fn validate_existing_database_sidecars(path: &Path) -> Result<(), ServerDatabaseError> {
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        match std::fs::symlink_metadata(&sidecar) {
            Ok(_) => crate::file_security::validate_file(&sidecar).map_err(|error| {
                ServerDatabaseError::Filesystem(format!("database sidecar: {error}"))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(ServerDatabaseError::Filesystem(
                    "cannot inspect the Server SQLite sidecar".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn secure_database_files(path: &Path, created_by_startup: bool) -> Result<(), ServerDatabaseError> {
    if created_by_startup {
        crate::file_security::secure_new_file(path)
            .map_err(|error| ServerDatabaseError::Filesystem(format!("database file: {error}")))?;
    } else {
        crate::file_security::validate_file(path)
            .map_err(|error| ServerDatabaseError::Filesystem(format!("database file: {error}")))?;
    }
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        if std::fs::symlink_metadata(&sidecar).is_ok() {
            let result = if created_by_startup {
                crate::file_security::secure_new_file(&sidecar)
            } else {
                crate::file_security::validate_file(&sidecar)
            };
            result.map_err(|error| {
                ServerDatabaseError::Filesystem(format!("database sidecar: {error}"))
            })?;
        }
    }
    Ok(())
}

/// Native connection options carrying every Server policy. The pool applies
/// the same policy through [`configure_session`] because the Any driver only
/// forwards the file URL.
pub(super) fn connect_options(
    config: &ServerDatabaseConfig,
    create_if_missing: bool,
) -> SqliteConnectOptions {
    let options = SqliteConnectOptions::new()
        .filename(config.path())
        .create_if_missing(create_if_missing)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(config.busy_timeout());
    if !config.exclusive_locking() {
        // Development mode deliberately shares the file: local tooling and
        // the browser e2e fixture refreshers write to a running dev Server.
        return options;
    }
    // A production Server owns its SQLite file: every read and write goes
    // through the single serialized pool connection. Exclusive locking keeps
    // that ownership true at the file layer, so no other process can attach
    // to the write-ahead log shared memory. Without it a concurrent external
    // SQLite connection (any sqlite3 invocation, including read-only) can
    // become the first attacher, and SQLite then truncates -shm to 3 bytes
    // while this process still has it mapped, raising SIGBUS inside
    // walFindFrame (issue #137) instead of failing cleanly.
    options.locking_mode(SqliteLockingMode::Exclusive)
}

/// Delay between connection-setup retries. The total wait is bounded by the
/// configured busy timeout, never by an unbounded loop.
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// SQLite primary result code for "the database file is locked".
const SQLITE_BUSY: i32 = 5;
/// SQLite primary result codes for a corrupt page and a non-database file.
const SQLITE_CORRUPT: i32 = 11;
const SQLITE_NOTADB: i32 = 26;

async fn connect_pool(
    config: &ServerDatabaseConfig,
    create_if_missing: bool,
    gate: Arc<RwLock<()>>,
) -> Result<AnyPool, ServerDatabaseError> {
    sqlx::any::install_default_drivers();
    let url = connect_options(config, create_if_missing).to_url_lossy();
    let options = AnyConnectOptions::from_url(&url).map_err(ServerDatabaseError::Connect)?;
    let session = SessionPolicy {
        exclusive_locking: config.exclusive_locking(),
        busy_timeout: config.busy_timeout(),
    };
    AnyPoolOptions::new()
        .max_connections(SERVER_WRITE_CONNECTIONS)
        .min_connections(SERVER_WRITE_CONNECTIONS)
        .after_connect(move |connection, _| {
            let gate = Arc::clone(&gate);
            Box::pin(async move {
                // A native maintenance connection may own the file right now.
                let _open = gate.read().await;
                configure_session(connection, session).await
            })
        })
        .connect_with(options)
        .await
        .map_err(ServerDatabaseError::Connect)
}

#[derive(Debug, Clone, Copy)]
struct SessionPolicy {
    exclusive_locking: bool,
    busy_timeout: Duration,
}

/// Apply the Server's connection-local PRAGMAs, in the order SQLx applies
/// them to a native connection, absorbing a transient SQLITE_BUSY.
///
/// SQLite cannot invoke its busy handler for the exclusive lock that PRAGMA
/// journal_mode/locking_mode take during connection setup, so a concurrent
/// external SQLite connection surfaces as an immediate "database is locked"
/// even though the configured busy timeout would have absorbed it. The retry
/// is bounded by that same timeout and never hides another error.
async fn configure_session(
    connection: &mut AnyConnection,
    policy: SessionPolicy,
) -> Result<(), sqlx::Error> {
    let deadline = Instant::now() + policy.busy_timeout;
    let busy_timeout_ms = policy.busy_timeout.as_millis().min(i32::MAX as u128);
    let mut statements = vec![format!("PRAGMA busy_timeout = {busy_timeout_ms}")];
    if policy.exclusive_locking {
        statements.push("PRAGMA locking_mode = EXCLUSIVE".to_owned());
    }
    statements.extend(
        [
            "PRAGMA journal_mode = WAL",
            "PRAGMA foreign_keys = ON",
            "PRAGMA synchronous = FULL",
        ]
        .map(str::to_owned),
    );
    for statement in &statements {
        loop {
            match sqlx::Executor::execute(&mut *connection, statement.as_str()).await {
                Ok(_) => break,
                Err(error) if is_busy_error(&error) && Instant::now() < deadline => {
                    tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

/// Open one native connection with the Server policy, absorbing a transient
/// SQLITE_BUSY the same way pool connections do.
async fn connect_native(
    options: &SqliteConnectOptions,
    busy_timeout: Duration,
) -> Result<SqliteConnection, sqlx::Error> {
    let deadline = Instant::now() + busy_timeout;
    loop {
        match options.connect().await {
            Ok(connection) => return Ok(connection),
            Err(error) if is_busy_error(&error) && Instant::now() < deadline => {
                tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
            }
            Err(error) => return Err(error),
        }
    }
}

/// The SQLite primary result code of a database error raised by SQLite.
/// Extended result codes (for example SQLITE_BUSY_SNAPSHOT) share it in their
/// low byte.
fn primary_result_code(error: &sqlx::Error) -> Option<i32> {
    error
        .as_database_error()?
        .try_downcast_ref::<SqliteError>()?
        .code()?
        .parse::<i32>()
        .ok()
        .map(|code| code & 0xff)
}

/// Whether SQLite reported the database file as locked.
pub(super) fn is_busy_error(error: &sqlx::Error) -> bool {
    primary_result_code(error) == Some(SQLITE_BUSY)
}

/// Whether SQLite reported a corrupt page or a file that is not a database.
pub(super) fn is_corruption_error(error: &sqlx::Error) -> bool {
    primary_result_code(error).is_some_and(|code| matches!(code, SQLITE_CORRUPT | SQLITE_NOTADB))
}

/// Defer foreign-key enforcement to the end of the current transaction.
pub(super) async fn defer_foreign_keys(connection: &mut AnyConnection) -> Result<(), sqlx::Error> {
    sqlx::query("PRAGMA defer_foreign_keys=ON")
        .execute(connection)
        .await
        .map(|_| ())
}

pub(super) async fn read_pragmas(pool: &AnyPool) -> Result<SqlitePragmas, sqlx::Error> {
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

/// Start-up runs one full integrity check as a fail-closed gate. It is bounded
/// so a pathological database cannot block start-up silently: budget exhaustion
/// is an explicit failure, never a silent "healthy" (ADR 0008, issue #194).
const STARTUP_INTEGRITY_BUDGET: Duration = Duration::from_secs(600);

async fn verify_integrity(
    pool: &AnyPool,
    backend: &SqliteBackend,
) -> Result<(), ServerDatabaseError> {
    match bounded_integrity_check(
        pool,
        backend,
        IntegrityScope::Full,
        STARTUP_INTEGRITY_BUDGET,
    )
    .await
    {
        IntegrityVerdict::Consistent => {}
        IntegrityVerdict::Inconsistent(result) => {
            return Err(ServerDatabaseError::IntegrityFailed(result));
        }
        IntegrityVerdict::Exhausted => {
            return Err(ServerDatabaseError::IntegrityFailed(
                "the startup integrity check exceeded its budget".to_owned(),
            ));
        }
        // This function only runs on the SQLite backend, which has a check.
        IntegrityVerdict::Unsupported => {
            return Err(ServerDatabaseError::IntegrityFailed(
                "the SQLite integrity check reported no verdict".to_owned(),
            ));
        }
        IntegrityVerdict::Failed(error) => {
            return Err(ServerDatabaseError::IntegrityQuery(error));
        }
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
        let exists = sqlx::query("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = $1")
            .bind(*table)
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

/// Run SQLite's integrity check under an explicit budget.
pub(super) async fn bounded_integrity_check(
    pool: &AnyPool,
    backend: &SqliteBackend,
    scope: IntegrityScope,
    budget: Duration,
) -> IntegrityVerdict {
    let query = match scope {
        IntegrityScope::Full => "PRAGMA integrity_check",
        IntegrityScope::Quick => "PRAGMA quick_check(1)",
    };
    match bounded_scalar_query(pool, backend, query, budget).await {
        BoundedQuery::Completed(result) if result == "ok" => IntegrityVerdict::Consistent,
        BoundedQuery::Completed(result) => IntegrityVerdict::Inconsistent(result),
        BoundedQuery::Exhausted => IntegrityVerdict::Exhausted,
        BoundedQuery::Failed(error) => IntegrityVerdict::Failed(error),
    }
}

/// Page accounting of the database file.
pub(super) async fn storage_pages(pool: &AnyPool) -> StoragePages {
    StoragePages {
        page_count: pragma_u64(pool, "PRAGMA page_count").await,
        freelist_pages: pragma_u64(pool, "PRAGMA freelist_count").await,
    }
}

async fn pragma_u64(pool: &AnyPool, query: &str) -> Option<u64> {
    let value = sqlx::query_scalar::<_, i64>(query)
        .fetch_one(pool)
        .await
        .ok()?;
    u64::try_from(value).ok()
}

/// Flush the write-ahead log into the main database file.
///
/// Deliberately a FULL checkpoint rather than TRUNCATE: resetting the WAL on
/// the shutdown path is the trigger identified in
/// docs/research/server-db-corruption-investigation.md for the upstream
/// WAL-reset corruption bug (SQLite < 3.51.3), and the truncation buys
/// nothing - the frames are already in the main file and any remaining WAL
/// content is replayed on the next start.
pub(super) async fn checkpoint_wal(pool: &AnyPool) -> Result<(), sqlx::Error> {
    sqlx::query("PRAGMA wal_checkpoint(FULL)")
        .execute(pool)
        .await
        .map(|_| ())
}

/// Outcome of one bounded scalar query.
#[derive(Debug)]
enum BoundedQuery {
    /// The statement ran to completion; carries SQLite's result string.
    Completed(String),
    /// The deadline passed and the progress handler interrupted the statement.
    Exhausted,
    /// The pool or SQLite returned an error before any verdict.
    Failed(sqlx::Error),
}

/// How long to wait for the single pool connection before giving up.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Run one scalar query under a bounded acquisition and an interruptible
/// execution window (issue #194).
///
/// A full-database `PRAGMA integrity_check` cannot complete inside any useful
/// budget on a large database, and a Tokio timeout alone cannot interrupt
/// `sqlite3_step`. The query therefore runs on a native maintenance
/// connection with a progress handler, while the pool's only connection is
/// closed and its replacement waits on the gate. The gate is not reopened
/// until SQLite has stopped and the native connection is closed.
async fn bounded_scalar_query(
    pool: &AnyPool,
    backend: &SqliteBackend,
    query: &str,
    budget: Duration,
) -> BoundedQuery {
    // Cancelling `Pool::acquire` can discard a connection acquired just before
    // cancellation, so acquisition is polled rather than cancelled.
    let acquire_deadline = Instant::now() + ACQUIRE_TIMEOUT;
    let pooled = loop {
        if pool.is_closed() {
            return BoundedQuery::Failed(sqlx::Error::PoolClosed);
        }
        if let Some(connection) = pool.try_acquire() {
            break connection;
        }
        if Instant::now() >= acquire_deadline {
            return BoundedQuery::Failed(sqlx::Error::PoolTimedOut);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    // Close the gate while this task still owns the only pool permit, then
    // release the file: the replacement connection cannot configure itself
    // (and take the file lock) until the maintenance session ends.
    let gate = Arc::clone(&backend.gate).write_owned().await;
    if let Err(error) = pooled.close().await {
        return BoundedQuery::Failed(error);
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut session = MaintenanceSession {
        connection: None,
        gate: Some(gate),
        cancelled: Arc::clone(&cancelled),
    };
    match connect_native(&backend.maintenance, backend.busy_timeout).await {
        Ok(connection) => session.connection = Some(connection),
        Err(error) => return BoundedQuery::Failed(error),
    }
    let deadline = Instant::now() + budget;
    let expired = Arc::new(AtomicBool::new(false));
    let expired_flag = Arc::clone(&expired);
    let connection = session
        .connection
        .as_mut()
        .expect("maintenance connection is owned");
    // The handle guard is scoped so it is dropped before the query borrows the
    // connection mutably; `set_progress_handler` persists on the connection.
    let handler = match connection.lock_handle().await {
        Ok(mut handle) => {
            handle.set_progress_handler(1_000, move || {
                if cancelled.load(Ordering::Acquire) {
                    return false;
                }
                if Instant::now() >= deadline {
                    expired_flag.store(true, Ordering::Release);
                    return false;
                }
                true
            });
            Ok(())
        }
        Err(error) => Err(error),
    };
    if let Err(error) = handler {
        // The handler error is the verdict; a failed close of the unused
        // maintenance connection adds nothing an operator can act on.
        drop(session.release().await);
        return BoundedQuery::Failed(error);
    }
    let result = sqlx::query_scalar::<_, String>(query)
        .fetch_one(&mut *connection)
        .await;
    let exhausted = expired.load(Ordering::Acquire);
    let cleanup = session.release().await;
    // Re-establish the serving connection now rather than on the next request,
    // so a failure to reconfigure it surfaces here.
    let reconnect = pool.acquire().await.map(drop);
    if exhausted {
        return BoundedQuery::Exhausted;
    }
    match (result, cleanup, reconnect) {
        (Err(error), _, _) | (Ok(_), Err(error), _) | (Ok(_), Ok(()), Err(error)) => {
            BoundedQuery::Failed(error)
        }
        (Ok(value), Ok(()), Ok(())) => BoundedQuery::Completed(value),
    }
}

/// Own the native maintenance connection and the closed gate together. The
/// gate is reopened only after the native connection is closed; dropping the
/// query future alone does not stop SQLite's worker thread.
struct MaintenanceSession {
    connection: Option<SqliteConnection>,
    gate: Option<OwnedRwLockWriteGuard<()>>,
    cancelled: Arc<AtomicBool>,
}

impl MaintenanceSession {
    async fn release(mut self) -> Result<(), sqlx::Error> {
        let connection = self.connection.take();
        let gate = self.gate.take();
        // The close runs on a detached task that owns the gate, so cancelling
        // this future cannot reopen the gate before SQLite lets go of the file.
        let closing = tokio::spawn(async move {
            let result = match connection {
                Some(connection) => connection.close().await,
                None => Ok(()),
            };
            drop(gate);
            result
        });
        closing.await.unwrap_or(Err(sqlx::Error::WorkerCrashed))
    }
}

impl Drop for MaintenanceSession {
    fn drop(&mut self) {
        // Interrupts a statement still running on the native connection.
        self.cancelled.store(true, Ordering::Release);
        let Some(connection) = self.connection.take() else {
            return;
        };
        let gate = self.gate.take();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = connection.close().await;
                drop(gate);
            });
        } else {
            // Without a runtime the worker closes the handle once the
            // interrupted statement returns.
            drop(connection);
            drop(gate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A native VM workload that cannot finish before the tested deadline.
    const LONG_QUERY: &str = "WITH RECURSIVE n(x) AS (
        VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 1000000000
    ) SELECT CAST(sum(x) AS TEXT) FROM n";

    async fn database() -> (tempfile::TempDir, super::super::ServerDatabase) {
        crate::test_support::test_database().await
    }

    fn backend(database: &super::super::ServerDatabase) -> &SqliteBackend {
        match &database.backend {
            super::super::Backend::Sqlite(backend) => backend,
            super::super::Backend::Postgres(_) => {
                panic!("the SQLite backend tests opened a PostgreSQL database")
            }
        }
    }

    async fn assert_pool_reusable(pool: &AnyPool) {
        let value: i64 = tokio::time::timeout(
            Duration::from_secs(5),
            sqlx::query_scalar(
                "WITH RECURSIVE n(x) AS (
                    VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 10000
                ) SELECT sum(x) FROM n",
            )
            .fetch_one(pool),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(value, 50_005_000);
        assert_eq!(pool.size(), SERVER_WRITE_CONNECTIONS);
        // The replacement connection carries the full Server policy again.
        let pragmas = read_pragmas(pool).await.unwrap();
        assert!(pragmas.satisfy_requirements());
    }

    #[cfg_attr(storage_postgres, ignore = "it exercises the SQLite backend directly")]
    #[tokio::test]
    async fn completed_query_returns_its_value() {
        if !crate::test_support::skip_unless_sqlite("it exercises the SQLite backend directly") {
            return;
        }
        let (_dir, database) = database().await;
        match bounded_scalar_query(
            database.pool(),
            backend(&database),
            "PRAGMA quick_check(1)",
            Duration::from_secs(5),
        )
        .await
        {
            BoundedQuery::Completed(value) => assert_eq!(value, "ok"),
            other => panic!("expected Completed, got {}", describe(&other)),
        }
        assert_pool_reusable(database.pool()).await;
    }

    #[cfg_attr(storage_postgres, ignore = "it exercises the SQLite backend directly")]
    #[tokio::test]
    async fn native_deadline_interrupts_and_releases_pool() {
        if !crate::test_support::skip_unless_sqlite("it exercises the SQLite backend directly") {
            return;
        }
        let (_dir, database) = database().await;
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            bounded_scalar_query(
                database.pool(),
                backend(&database),
                LONG_QUERY,
                Duration::from_millis(20),
            ),
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, BoundedQuery::Exhausted),
            "expected Exhausted, got {}",
            describe(&outcome)
        );
        assert_pool_reusable(database.pool()).await;
    }

    #[cfg_attr(storage_postgres, ignore = "it exercises the SQLite backend directly")]
    #[tokio::test]
    async fn aborted_query_releases_the_file_before_pool_reuse() {
        if !crate::test_support::skip_unless_sqlite("it exercises the SQLite backend directly") {
            return;
        }
        let (_dir, database) = database().await;
        let database = Arc::new(database);
        let task_database = Arc::clone(&database);
        let task = tokio::spawn(async move {
            bounded_scalar_query(
                task_database.pool(),
                backend(&task_database),
                LONG_QUERY,
                Duration::from_secs(60),
            )
            .await
        });
        // Wait until the maintenance session owns the file.
        for _ in 0..500 {
            if backend(&database).gate.try_read().is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(backend(&database).gate.try_read().is_err());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        // The exclusive pool connection can only be re-established after the
        // interrupted native connection released the file.
        assert_pool_reusable(database.pool()).await;
        sqlx::query(
            "INSERT INTO server_settings (setting_key, setting_value, updated_at) VALUES ('after-abort', '1', '2026-01-01T00:00:00Z')",
        )
        .execute(database.pool())
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn classifies_busy_and_corruption_only_for_sqlite_errors() {
        assert!(!is_busy_error(&sqlx::Error::PoolTimedOut));
        assert!(!is_corruption_error(&sqlx::Error::PoolTimedOut));
    }

    fn describe(outcome: &BoundedQuery) -> &'static str {
        match outcome {
            BoundedQuery::Completed(_) => "Completed",
            BoundedQuery::Exhausted => "Exhausted",
            BoundedQuery::Failed(_) => "Failed",
        }
    }
}
