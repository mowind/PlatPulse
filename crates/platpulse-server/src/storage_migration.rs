//! Storage Backend Migration (ADR 0013, issue #257).
//!
//! storage migrate moves a stopped SQLite deployment into an empty PostgreSQL
//! database, offline and one way. There is deliberately no PostgreSQL ->
//! SQLite direction: once the PostgreSQL Server accepts a report, Agents have
//! applied the Receipts, emptied their Durable Spool and advanced their
//! sequence and Inventory Declaration Records, so replaying the old file
//! backwards would break ordering, Receipt and Purge-barrier evidence.
//!
//! The copy is complete and unsanitized - Agent identity, credential digests,
//! Report Receipts and Purge barriers survive exactly - and it runs inside one
//! target transaction: every table is streamed out of the source, inserted,
//! and read back, and the transaction commits only after every per-table row
//! count and content digest matches the source. Any failure rolls the target
//! back to empty, so the command stays retryable.
//!
//! The source file is never modified. A sibling marker
//! (platpulse.db.migrated) records the baseline written into the target, and
//! the SQLite backend refuses to start - and every writing CLI command refuses
//! - while that marker exists ([refuse_migrated]). storage migrate abort is
//! the only way back, and only while the target has accepted no write since
//! that baseline; after a business write the fix is forward on PostgreSQL,
//! exactly like the Inventory cutover rollback rule (issue #192).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::any::{AnyArguments, AnyRow};
use sqlx::query::Query;
use sqlx::sqlite::SqliteRow;
use sqlx::{Any, AnyConnection, Connection, Row, SqliteConnection};
use thiserror::Error;
use tokio_stream::StreamExt;

use crate::config::ServerConfig;
use crate::database::{
    NullableReal, PostgresConnectConfig, PostgresHost, SERVER_SCHEMA_VERSION, ServerDatabase,
    ServerDatabaseConfig, ServerDatabaseError, StorageConfig,
};

/// Sibling marker suffix appended to the source SQLite path.
pub const MARKER_SUFFIX: &str = ".migrated";

/// Tables that are never copied and never count as migration content.
///
/// _sqlx_migrations belongs to the target's own schema history, and
/// storage_migration is the baseline row this command writes: copying it, or
/// hashing it, would make the baseline describe content that only exists
/// because the baseline exists.
const BOOKKEEPING_TABLES: &[&str] = &["_sqlx_migrations", "storage_migration"];

/// Tables whose rows the Server migrations insert themselves.
///
/// A target that was empty when this command started holds these rows the
/// moment its migrations commit, so the copy clears them and writes the
/// source's own rows in their place. Every other copied table must be empty: if
/// it holds rows, this database has been serving something, and merging the two
/// histories is exactly what the migration must not do.
///
/// The list is deliberately hardcoded rather than derived: if a future
/// migration seeds another table, this command refuses to migrate instead of
/// guessing which rows the operator owns, which is the safe direction.
const MIGRATION_SEEDED_TABLES: &[&str] = &[
    "retention_policies",
    "server_settings",
    "validator_model_migration",
];

/// Rows per multi-row INSERT. Bounded by parameter count as well.
const MAX_ROWS_PER_INSERT: usize = 200;

/// The PostgreSQL wire protocol caps a statement at 65 535 parameters.
const MAX_BIND_PARAMETERS: usize = 65_535;

/// How far the migration the marker fences had got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerState {
    /// Written before the copy starts: the source is already fenced off, the
    /// target has not committed anything yet.
    InProgress,
    /// The target committed the copy and the baseline was verified.
    Completed,
}

/// The JSON marker written next to the source database, before the copy starts
/// and finalized after the target commits. It is read by the SQLite startup gate.
///
/// The fence is claimed first on purpose. A crash, a full disk or a permission
/// error anywhere between the claim and the finalize leaves a marker behind, so
/// the worst case is a migration that has to be finished or aborted by hand -
/// never a committed PostgreSQL target served next to a SQLite file that still
/// starts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationMarker {
    /// Whether the copy this marker fences finished.
    pub state: MarkerState,
    /// When the command claimed the source, RFC 3339 UTC.
    pub started_at: String,
    /// When the target committed, RFC 3339 UTC. Present once completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// The SQLite file that was migrated, as configured.
    pub source_path: String,
    /// The source schema version the copy came from.
    pub source_schema_version: i64,
    /// The PostgreSQL target, without credentials.
    pub target_description: String,
    /// Number of tables copied.
    #[serde(default)]
    pub table_count: u64,
    /// Number of rows copied.
    #[serde(default)]
    pub row_count: u64,
    /// The content digest the target committed with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
}

/// Why a Storage Backend Migration command was refused or failed.
#[derive(Debug, Error)]
pub enum StorageMigrationError {
    #[error(transparent)]
    Ownership(#[from] crate::ownership::OwnershipError),
    #[error(transparent)]
    Database(#[from] ServerDatabaseError),
    #[error(transparent)]
    Secret(#[from] crate::database::PostgresSecretError),
    #[error("storage migration database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("storage migration IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("storage migration JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the Storage Backend Migration was refused: {0}")]
    Refused(String),
    #[error("the Storage Backend Migration failed verification: {0}")]
    Verification(String),
    #[error(
        "the Storage Backend Migration can no longer be aborted: {0}; business writes have already resumed on PostgreSQL, so the fix is forward on PostgreSQL (repair the data there), not a rollback to the SQLite file"
    )]
    ForwardRepairRequired(String),
    #[error("the Storage Backend Migration committed but did not finish: {0}")]
    Incomplete(String),
}

/// What one storage migrate run reported.
#[derive(Debug, Clone)]
pub struct MigrationSummary {
    pub source_path: PathBuf,
    pub marker_path: PathBuf,
    pub target_description: String,
    pub table_count: u64,
    pub row_count: u64,
    /// Number of identity sequences advanced past their stored maximum.
    pub sequence_count: usize,
    pub content_sha256: String,
    pub completed_at: String,
}

/// What one storage migrate abort run reported.
#[derive(Debug, Clone)]
pub struct AbortSummary {
    pub target_description: String,
    /// The baseline the successful abort removed.
    pub completed_at: String,
    pub table_count: u64,
    pub row_count: u64,
}

/// Progress of one table inside a running migration.
pub struct TableProgress<'a> {
    pub table: &'a str,
    pub index: usize,
    pub tables: usize,
    /// Rows copied for this table.
    pub rows: u64,
    /// Rows copied so far across all tables.
    pub copied_rows: u64,
    /// Rows the source holds across all tables.
    pub total_rows: u64,
    /// Wall-clock time since the copy started.
    pub elapsed: Duration,
    /// Estimated time left, once at least one row has been copied.
    pub estimated_remaining: Option<Duration>,
}

/// The sibling marker path for a SQLite database file.
pub fn marker_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.as_os_str().to_owned();
    name.push(MARKER_SUFFIX);
    PathBuf::from(name)
}

/// Refuse to open a SQLite deployment whose file was already migrated.
///
/// This is the startup and offline-command gate: the migration never touches
/// the source file, so the marker is the only durable evidence that this file
/// must not accept a write again. It fails closed - an unreadable or malformed
/// marker refuses rather than allowing a stale SQLite deployment to resume.
pub fn refuse_migrated(db_path: &Path) -> Result<(), String> {
    let marker = marker_path(db_path);
    // Only a definitively absent marker lets the file serve. Every other
    // outcome - a permission error, an I/O error, an unreadable or malformed
    // document - keeps the fence up, because a marker this process cannot read
    // is still evidence that the deployment may have moved to PostgreSQL.
    match std::fs::symlink_metadata(&marker) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "the Server SQLite database {} may carry the Storage Backend Migration marker {} but the marker cannot be inspected ({error}); refusing to serve this file, because a marker it cannot read is not evidence that the deployment still runs on SQLite",
                db_path.display(),
                marker.display()
            ));
        }
    }
    let parsed = read_marker(&marker).map_err(|error| {
        format!(
            "the Server SQLite database {} carries the Storage Backend Migration marker {} but it cannot be read: {error}",
            db_path.display(),
            marker.display()
        )
    })?;
    Err(migrated_message(db_path, &parsed))
}

fn migrated_message(db_path: &Path, marker: &MigrationMarker) -> String {
    let target = &marker.target_description;
    match marker.state {
        MarkerState::Completed => format!(
            "the Server SQLite database {} was migrated to the PostgreSQL Storage Backend {target} on {} (sibling marker {}); this file is the fallback point, not a serving database: start the Server on PostgreSQL, or run platpulse-server storage migrate abort while the PostgreSQL target has accepted no write since the migration",
            db_path.display(),
            marker.completed_at.as_deref().unwrap_or(&marker.started_at),
            marker_path(db_path).display()
        ),
        MarkerState::InProgress => format!(
            "the Server SQLite database {} was being migrated to the PostgreSQL Storage Backend {target} when the run started at {} and the run never finished (sibling marker {}); the target either holds the copy or was rolled back to empty: run platpulse-server storage migrate abort to return this file to service, or finish the migration by hand. Do not serve this file while the marker exists",
            db_path.display(),
            marker.started_at,
            marker_path(db_path).display()
        ),
    }
}

fn read_marker(path: &Path) -> Result<MigrationMarker, StorageMigrationError> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn claim_marker(path: &Path, marker: &MigrationMarker) -> Result<(), StorageMigrationError> {
    let text = serde_json::to_vec_pretty(marker)?;
    // create_new keeps the marker the single evidence of a migration: the
    // command refuses an existing marker before it starts, and this never
    // replaces one.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    use std::io::Write as _;
    file.write_all(&text)?;
    file.sync_all()?;
    drop(file);
    crate::file_security::secure_new_file(path).map_err(StorageMigrationError::Refused)?;
    sync_parent(path)?;
    Ok(())
}

/// Rewrite the claim the run already holds into its finished form.
fn finalize_marker(path: &Path, marker: &MigrationMarker) -> Result<(), StorageMigrationError> {
    let text = serde_json::to_vec_pretty(marker)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)?;
    use std::io::Write as _;
    file.write_all(&text)?;
    file.sync_all()?;
    drop(file);
    sync_parent(path)?;
    Ok(())
}

/// Make the marker's creation or rewrite survive a power loss, not just a
/// process crash: the directory entry is what the next start reads, and
/// `sync_all` on the file covers only its content.
///
/// A failure here fails the migration before a single row is copied. The fence
/// is only worth its name if a power loss cannot erase it: a marker that
/// disappeared while PostgreSQL held the committed copy would let the Server
/// serve the SQLite file beside a deployment that already accepted writes,
/// which is exactly the return ADR 0013 forbids.
#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<(), StorageMigrationError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let directory = std::fs::File::open(parent)?;
    match directory.sync_all() {
        Ok(()) => Ok(()),
        // Some filesystems and network mounts refuse a directory sync
        // outright; the entry is then as durable as that filesystem makes it,
        // and the file's own sync_all already covered its content.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

/// Outside Unix a directory cannot be opened as a file, so the marker's own
/// `sync_all` is the whole of the guarantee; the supported deployment targets
/// are Unix.
#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> Result<(), StorageMigrationError> {
    Ok(())
}

/// Drop the claim of a run that rolled the target back to empty.
///
/// The command stays retryable only if the fence goes with the rollback. A
/// failure to remove it leaves the marker in place, which refuses SQLite - the
/// safe direction - and is reported.
fn release_marker(path: &Path) -> Result<(), StorageMigrationError> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            sync_parent(path)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// The claim a run holds on its source, released unless the run keeps it.
///
/// The migration claims its source before it opens the file and finalizes the
/// claim after the target commits. Every other way out - a source that will not
/// open, a copy that rolled back, a verification that failed - must hand the
/// file back, because a claim left behind by a run that copied nothing would
/// fence a perfectly healthy SQLite deployment. The drop is therefore the
/// release, and the two paths that deliberately keep the fence up call
/// [MarkerClaim::keep].
struct MarkerClaim {
    path: PathBuf,
    held: bool,
}

impl MarkerClaim {
    fn new(path: PathBuf) -> Self {
        Self { path, held: true }
    }

    /// Leave the marker in place: the target accepted the copy, or the operator
    /// has to settle the marker by hand.
    fn keep(mut self) {
        self.held = false;
    }
}

impl Drop for MarkerClaim {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        // A release that fails leaves the marker behind, which refuses SQLite:
        // the safe direction, and the operator is told about it.
        if let Err(error) = release_marker(&self.path) {
            tracing::warn!(error = %error, "the failed Storage Backend Migration could not release its source marker");
        }
    }
}

/// Migrate a stopped SQLite deployment into an empty PostgreSQL database.
///
/// The source deployment keeps its exclusive ownership guard and SQLite
/// EXCLUSIVE locking for the whole run; the target takes its own session
/// advisory lock while it is open. Either side refusing ownership aborts the
/// command before any copy happens.
pub async fn migrate(
    source: &ServerConfig,
    target: &ServerConfig,
    progress: &mut dyn FnMut(TableProgress<'_>),
) -> Result<MigrationSummary, StorageMigrationError> {
    let source_config = sqlite_source_config(source)?;
    let target_config = postgres_target_config(target)?;
    migrate_deployment(&source_config, &target_config, progress).await
}

/// The migration itself, over the two resolved Storage Backends.
///
/// The CLI resolves one server.toml on each side into these configurations;
/// the suite drives the same code with a temporary SQLite file and a fresh
/// PostgreSQL database.
pub async fn migrate_deployment(
    source_config: &ServerDatabaseConfig,
    target_config: &PostgresConnectConfig,
    progress: &mut dyn FnMut(TableProgress<'_>),
) -> Result<MigrationSummary, StorageMigrationError> {
    crate::init::restrict_umask();
    let source_db_path = source_config.path().to_path_buf();
    let marker = marker_path(&source_db_path);
    if std::fs::symlink_metadata(&marker).is_ok() {
        return Err(StorageMigrationError::Refused(format!(
            "the source {} already carries the migration marker {}: this deployment was migrated once, and a second migration would copy a stale file over a target that may already be serving; run platpulse-server storage migrate abort first if the target has accepted no write since that migration",
            source_db_path.display(),
            marker.display()
        )));
    }
    // Both sides are claimed before either opens: the source takes the
    // deployment ownership guard unconditionally (SQLite then opens EXCLUSIVE),
    // and the target takes its session advisory lock when it opens.
    //
    // There is deliberately no development exception here. A development Server
    // skips the flock and the exclusive locking mode so a developer can iterate
    // on a live database, but this command reads the file once, without a
    // snapshot transaction, and a writer running underneath it would make the
    // copy a torn mixture of two database states.
    let _source_ownership = crate::ownership::acquire(&source_db_path)?;

    let target_description = describe_target(target_config);
    let started_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    // The fence goes up before the first byte is copied, so no failure anywhere
    // after this point can leave a startable SQLite file next to a PostgreSQL
    // target that holds the copy.
    let mut marker_document = MigrationMarker {
        state: MarkerState::InProgress,
        started_at: started_at.clone(),
        completed_at: None,
        source_path: source_db_path.display().to_string(),
        source_schema_version: SERVER_SCHEMA_VERSION,
        target_description: target_description.clone(),
        table_count: 0,
        row_count: 0,
        content_sha256: None,
    };
    claim_marker(&marker, &marker_document)?;
    // From here on every way out of this function either finalizes the claim or
    // drops it, including the source that will not open: a claim left behind by
    // a run that copied nothing would fence a healthy SQLite file for good.
    let claim = MarkerClaim::new(marker.clone());

    let mut connection = open_source(source_config).await?;
    let source_description = source_db_path.display().to_string();
    let attempt = run_migrate(
        &mut connection,
        target_config,
        &source_description,
        &target_description,
        progress,
    )
    .await;
    let _ = connection.close().await;
    let completed = match attempt {
        Ok(completed) => completed,
        // The copy reported an outcome it cannot prove: either the target may
        // hold the committed copy, or the marker could not be rewritten. Both
        // leave the fence up, because a SQLite file that starts serving next to
        // a PostgreSQL target holding its content is the failure ADR 0013
        // exists to prevent; the operator resolves it with storage migrate
        // abort, which empties a target the copy never committed.
        Err(error @ StorageMigrationError::Incomplete(_)) => {
            claim.keep();
            return Err(StorageMigrationError::Incomplete(format!(
                "{error}: the marker {} stays in place, so this SQLite file keeps refusing to serve; inspect the PostgreSQL target, then run platpulse-server storage migrate abort",
                marker.display()
            )));
        }
        Err(error) => return Err(error),
    };

    marker_document.state = MarkerState::Completed;
    marker_document.completed_at = Some(completed.completed_at.clone());
    marker_document.table_count = completed.table_count;
    marker_document.row_count = completed.row_count;
    marker_document.content_sha256 = Some(completed.content_sha256.clone());
    if let Err(error) = finalize_marker(&marker, &marker_document) {
        // The target committed the copy while the marker still says the run is
        // in progress. That marker keeps refusing this file, so the deployment
        // cannot silently fall back to SQLite; the operator finishes the job
        // with storage migrate abort or by hand, which is why the claim stays.
        claim.keep();
        return Err(StorageMigrationError::Incomplete(format!(
            "the PostgreSQL target {target_description} committed the copy, but the marker {} could not be finalized ({error}); it still refuses this SQLite file, which is the safe state: inspect the target and run platpulse-server storage migrate abort, or complete the marker by hand",
            marker.display()
        )));
    }
    claim.keep();
    Ok(MigrationSummary {
        source_path: source_db_path.clone(),
        marker_path: marker,
        target_description,
        table_count: completed.table_count,
        row_count: completed.row_count,
        sequence_count: completed.sequence_count,
        content_sha256: completed.content_sha256,
        completed_at: completed.completed_at,
    })
}

/// Abort a committed migration that the target has not written to since.
pub async fn abort(
    source: &ServerConfig,
    target: &ServerConfig,
) -> Result<AbortSummary, StorageMigrationError> {
    let source_config = sqlite_source_config(source)?;
    let target_config = postgres_target_config(target)?;
    abort_deployment(&source_config, &target_config).await
}

/// The abort itself, over the two resolved Storage Backends.
pub async fn abort_deployment(
    source_config: &ServerDatabaseConfig,
    target_config: &PostgresConnectConfig,
) -> Result<AbortSummary, StorageMigrationError> {
    crate::init::restrict_umask();
    let source_db_path = source_config.path().to_path_buf();
    let marker = marker_path(&source_db_path);
    if std::fs::symlink_metadata(&marker).is_err() {
        return Err(StorageMigrationError::Refused(format!(
            "the source {} carries no migration marker {}: this deployment was never migrated to the PostgreSQL Storage Backend, so there is nothing to abort",
            source_db_path.display(),
            marker.display()
        )));
    }
    let parsed = read_marker(&marker)?;
    if Path::new(&parsed.source_path) != source_db_path.as_path() {
        return Err(StorageMigrationError::Refused(format!(
            "the migration marker {} records the source {} but this command was given {}: abort must be pointed at the deployment the marker belongs to",
            marker.display(),
            parsed.source_path,
            source_db_path.display()
        )));
    }
    // The marker names the one database this migration wrote. Aborting against
    // any other target would empty a database the marker never accounted for
    // while the real target keeps serving the migrated content, which is the
    // one direction the fence exists to prevent.
    let target_description = describe_target(target_config);
    if parsed.target_description != target_description {
        return Err(StorageMigrationError::Refused(format!(
            "the migration marker {} records the target {} but this command was given {}: abort must be pointed at the database that migration wrote, because emptying another database would destroy content this marker never accounted for",
            marker.display(),
            parsed.target_description,
            target_description
        )));
    }
    // The source is claimed under the same guard as the migration even though
    // nothing is written to it: an abort and a running Server on this file must
    // never overlap, and the SQLite file is what the marker protects.
    let _source_ownership = crate::ownership::acquire(&source_db_path)?;

    let database = ServerDatabase::open_for_maintenance(target_config.clone()).await?;
    // The abort writes to the target under the same exclusive ownership the
    // migration took. Losing it mid-abort means another Server may be serving
    // the target this command is emptying, so the deletes are rolled back and
    // the marker stays: the operator inspects the target and runs the abort
    // again.
    let result = while_owned(
        &database,
        || {
            StorageMigrationError::Incomplete(format!(
                "this process stopped owning the PostgreSQL target {target_description} while the abort was running (its ownership session was lost), so the abort was rolled back and the SQLite marker still fences the source; inspect the target before running the abort again"
            ))
        },
        abort_inner(&database, &target_description, &parsed),
    )
    .await;
    database.close().await;
    let summary = result?;
    release_marker(&marker)?;
    Ok(summary)
}

/// Run one destructive target write while this process still owns the target.
///
/// Ownership is a session advisory lock, and the pool's ownership guard only
/// refuses *new* work connections: the copy and the abort each check one out
/// before they start, so they would keep using it after the session died. The
/// write could then commit into a database another Server may by then be
/// serving. Racing the write against `ownership_lost` drops it instead, which
/// takes its single transaction down; for the abort the SQLite marker stays in
/// place, so the source stays fenced and the operator re-runs the command.
async fn while_owned<T, W>(
    database: &ServerDatabase,
    on_loss: impl FnOnce() -> StorageMigrationError,
    work: W,
) -> Result<T, StorageMigrationError>
where
    W: std::future::Future<Output = Result<T, StorageMigrationError>>,
{
    let work = std::pin::pin!(work);
    tokio::select! {
        // Ownership is checked first: a database that is already lost must not
        // get one more statement in, and the write future is never polled.
        biased;
        _ = database.ownership_lost() => Err(on_loss()),
        result = work => result,
    }
}

async fn run_migrate(
    source: &mut SqliteConnection,
    target_config: &PostgresConnectConfig,
    source_description: &str,
    target_description: &str,
    progress: &mut dyn FnMut(TableProgress<'_>),
) -> Result<AppliedMigration, StorageMigrationError> {
    verify_source_integrity(source).await?;
    verify_source_schema_version(source).await?;
    verify_source_cutover(source).await?;
    let tables = read_source_schema(source).await?;
    if tables.is_empty() {
        return Err(StorageMigrationError::Refused(
            "the source SQLite database holds no table to copy".to_owned(),
        ));
    }
    let mut source_rows = BTreeMap::new();
    let mut total_rows = 0_u64;
    for table in &tables {
        let rows = count_rows(source, &table.name).await?;
        total_rows += rows;
        source_rows.insert(table.name.clone(), rows);
    }

    // open_for_maintenance skips the startup writes (Alert Rule catalog seed
    // and recovery-window invalidation) so the target this command measures is
    // exactly the target it wrote. It also keeps the session advisory lock for
    // the whole run, which is why the emptiness of the target is judged inside
    // the copy's own transaction instead of by a probe on a separate connection.
    let database = ServerDatabase::open_for_maintenance(target_config.clone()).await?;
    let plan = CopyPlan {
        tables: &tables,
        source_rows: &source_rows,
        total_rows,
        source_description,
        target_description,
        started: Instant::now(),
    };
    // Ownership of the target is a session advisory lock this process holds. If
    // that session dies mid-copy, another Server can open the target while this
    // one is still copying into it, so the copy is raced against the loss of
    // ownership: dropping it takes its single transaction down, and the SQLite
    // marker keeps fencing the source for the operator to abort or repair.
    let result = while_owned(
        &database,
        || {
            StorageMigrationError::Incomplete(format!(
                "this process stopped owning the PostgreSQL target {target_description} while the copy was running (its ownership session was lost), so the copy was rolled back instead of committed; another Server may now hold the target, so inspect it before running the migration again"
            ))
        },
        apply_to_target(&database, source, &plan, progress),
    )
    .await;
    database.close().await;
    result
}

struct AppliedMigration {
    table_count: u64,
    row_count: u64,
    sequence_count: usize,
    content_sha256: String,
    completed_at: String,
}

/// What a copy needs beyond its live handles.
///
/// These travel together because every one of them is read from the source
/// before the target is touched - the schema, the per-table row counts and
/// their sum, the two descriptions the baseline records, and the run clock the
/// progress output measures. Keeping them in one value stops the copy from
/// taking a positional argument list the caller has to re-assemble.
struct CopyPlan<'a> {
    tables: &'a [Table],
    source_rows: &'a BTreeMap<String, u64>,
    total_rows: u64,
    source_description: &'a str,
    target_description: &'a str,
    started: Instant,
}

async fn apply_to_target(
    database: &ServerDatabase,
    source: &mut SqliteConnection,
    plan: &CopyPlan<'_>,
    progress: &mut dyn FnMut(TableProgress<'_>),
) -> Result<AppliedMigration, StorageMigrationError> {
    let tables = plan.tables;
    let source_rows = plan.source_rows;
    let total_rows = plan.total_rows;
    let source_description = plan.source_description;
    let target_description = plan.target_description;
    let started = plan.started;
    let mut transaction = database.pool().begin().await?;
    // Tables are copied in name order, so a child table can precede the parent
    // its foreign key needs. Every Server foreign key is DEFERRABLE, so the
    // check waits for the commit instead of failing the copy.
    crate::database::defer_foreign_keys(&mut transaction).await?;
    let target_schema = read_target_schema(&mut transaction).await?;
    compare_schemas(tables, &target_schema)?;
    prepare_target(&mut transaction, tables).await?;

    let mut digests: BTreeMap<String, TableDigest> = BTreeMap::new();
    let mut copied_rows = 0_u64;
    for (index, table) in tables.iter().enumerate() {
        let source_digest = copy_table(source, &mut transaction, table).await?;
        let expected = source_rows.get(&table.name).copied().unwrap_or(0);
        if source_digest.rows != expected {
            return Err(StorageMigrationError::Verification(format!(
                "table '{}' held {expected} row(s) when the migration planned its copy but returned {} row(s): the source changed under an exclusive lock, which must not happen",
                table.name, source_digest.rows
            )));
        }
        let target_digest = target_table_digest(&mut transaction, table).await?;
        if target_digest.rows != source_digest.rows || target_digest.sha256 != source_digest.sha256
        {
            return Err(StorageMigrationError::Verification(format!(
                "table '{}' did not copy exactly: the source holds {} row(s) with digest {}, the target holds {} row(s) with digest {}",
                table.name,
                source_digest.rows,
                source_digest.sha256,
                target_digest.rows,
                target_digest.sha256
            )));
        }
        copied_rows += source_digest.rows;
        let rows = source_digest.rows;
        digests.insert(table.name.clone(), source_digest);
        progress(TableProgress {
            table: &table.name,
            index: index + 1,
            tables: tables.len(),
            rows,
            copied_rows,
            total_rows,
            elapsed: started.elapsed(),
            estimated_remaining: estimate_remaining(started.elapsed(), copied_rows, total_rows),
        });
    }

    let content_sha256 = content_digest(&digests);
    // The advance is transactional (ALTER SEQUENCE ... RESTART, which setval is
    // not), so a copy that fails after this point takes the sequences back with
    // it and the target stays empty. It runs after the last verification and
    // before the baseline, so the digest below records what abort compares.
    let sequence_count = advance_identities(&mut transaction).await?;
    let sequence_sha256 = identity_sequence_digest(&mut transaction).await?;

    let completed_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    sqlx::query(
        "INSERT INTO storage_migration (singleton, completed_at, source_path, source_schema_version, target_schema_version, target_description, table_count, row_count, content_sha256, sequence_sha256) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(1_i64)
    .bind(&completed_at)
    .bind(source_description)
    .bind(SERVER_SCHEMA_VERSION)
    .bind(SERVER_SCHEMA_VERSION)
    .bind(target_description)
    .bind(u64_to_i64(tables.len() as u64, "table count")?)
    .bind(u64_to_i64(copied_rows, "row count")?)
    .bind(&content_sha256)
    .bind(&sequence_sha256)
    .execute(&mut *transaction)
    .await?;
    if let Err(error) = transaction.commit().await {
        // A commit can fail after PostgreSQL applied it - a connection lost
        // while the acknowledgement travels is enough - and the two outcomes
        // are indistinguishable from here. The caller keeps the source marker
        // up for this error, because handing the SQLite file back to the
        // Server while the target holds the copy is the one outcome ADR 0013
        // forbids; storage migrate abort resolves the ambiguity either way.
        return Err(StorageMigrationError::Incomplete(format!(
            "the copy's single transaction could not be confirmed as committed ({error}), so the PostgreSQL target {target_description} may or may not hold it"
        )));
    }
    Ok(AppliedMigration {
        table_count: tables.len() as u64,
        row_count: copied_rows,
        sequence_count,
        content_sha256,
        completed_at,
    })
}

async fn abort_inner(
    database: &ServerDatabase,
    target_description: &str,
    marker: &MigrationMarker,
) -> Result<AbortSummary, StorageMigrationError> {
    let mut transaction = database.pool().begin().await?;
    // Deleting in name order removes parents before children, so the foreign
    // keys are deferred to the commit exactly as they are during the copy.
    crate::database::defer_foreign_keys(&mut transaction).await?;
    let baseline: Option<BaselineRow> = sqlx::query_as(
        "SELECT completed_at, table_count, row_count, content_sha256, sequence_sha256 FROM storage_migration WHERE singleton = 1",
    )
    .fetch_optional(&mut *transaction)
    .await?;
    let tables = read_target_schema(&mut transaction).await?;
    let (completed_at, removed_rows) = match baseline {
        Some(baseline) => {
            // The commit wrote this baseline, so the target held the migrated
            // content at that moment. It must still hold exactly it.
            let mut digests: BTreeMap<String, TableDigest> = BTreeMap::new();
            let mut rows = 0_u64;
            for table in &tables {
                let digest = target_table_digest(&mut transaction, table).await?;
                rows += digest.rows;
                digests.insert(table.name.clone(), digest);
            }
            let content_sha256 = content_digest(&digests);
            let sequence_sha256 = identity_sequence_digest(&mut transaction).await?;
            if content_sha256 != baseline.content_sha256
                || rows != baseline.row_count as u64
                || tables.len() as u64 != baseline.table_count as u64
                || sequence_sha256 != baseline.sequence_sha256
            {
                // Two witnesses, because neither is enough alone: the content
                // digest sees a report or an Admin mutation that stayed, and
                // the sequence digest sees a write that consumed an identity
                // and then rolled back or was reverted. What neither sees is a
                // write that left both untouched - a no-op UPDATE, or one a
                // later statement undid - and that boundary is deliberate: such
                // a write persisted nothing, so reinstating the SQLite file
                // loses nothing durable, and the alternative (per-table DML
                // counters, or a write log maintained by the Server) is either
                // unreliable across backends or a schema change no report
                // needs.
                return Err(StorageMigrationError::ForwardRepairRequired(format!(
                    "the PostgreSQL target {target_description} no longer holds the state recorded when it was migrated at {} (baseline digest {} over {} table(s) and {} row(s) with sequence digest {}; now {} table(s), {} row(s), digest {} and sequence digest {}): a report, an Admin mutation or a background write reached it after the baseline, so the SQLite file cannot be reinstated",
                    baseline.completed_at,
                    baseline.content_sha256,
                    baseline.table_count,
                    baseline.row_count,
                    baseline.sequence_sha256,
                    tables.len(),
                    rows,
                    content_sha256,
                    sequence_sha256
                )));
            }
            // The target provably holds exactly the migrated content, so
            // deleting it returns the database to the empty, freshly
            // initialized state a new storage migrate can target again.
            delete_copied_rows(&mut transaction, &tables).await?;
            (baseline.completed_at, rows)
        }
        None => {
            // No baseline means the copy never committed: the run that claimed
            // the source was interrupted, so its transaction rolled back and
            // the target holds nothing but the reference rows its own
            // migrations insert. There is no history to verify against, and
            // nothing but those rows to hand back.
            if marker.state == MarkerState::Completed {
                return Err(StorageMigrationError::Refused(format!(
                    "the marker next to {} records a completed migration into {target_description}, but that database carries no Storage Backend Migration baseline: the target was emptied or replaced after the migration committed, so abort has no evidence to check and the marker has to be resolved by hand",
                    marker.source_path
                )));
            }
            // The rollback is only trustworthy if nothing else has used this
            // database since: a target somebody served from or migrated into
            // after the interrupted run holds rows that are not this
            // migration's to delete.
            refuse_used_target(&mut transaction, &tables, target_description).await?;
            let mut rows = 0_u64;
            for table in &tables {
                rows += target_table_digest(&mut transaction, table).await?.rows;
            }
            delete_copied_rows(&mut transaction, &tables).await?;
            (marker.started_at.clone(), rows)
        }
    };
    sqlx::query("DELETE FROM storage_migration")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(AbortSummary {
        target_description: target_description.to_owned(),
        completed_at,
        table_count: tables.len() as u64,
        row_count: removed_rows,
    })
}

/// Refuse when the target holds rows a rolled-back copy cannot have written.
///
/// The reference rows the target's own migrations insert are the only content a
/// freshly created target carries; anything else is evidence that the database
/// was served from, or migrated into, after the run that claimed the source
/// died. Deleting it would destroy durable state that no SQLite file holds.
async fn refuse_used_target(
    transaction: &mut sqlx::Transaction<'_, Any>,
    tables: &[Table],
    target_description: &str,
) -> Result<(), StorageMigrationError> {
    for table in tables {
        if MIGRATION_SEEDED_TABLES.contains(&table.name.as_str()) {
            continue;
        }
        let rows: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {}",
            quote_identifier(&table.name)
        ))
        .fetch_one(&mut **transaction)
        .await?;
        if rows > 0 {
            return Err(StorageMigrationError::ForwardRepairRequired(format!(
                "the PostgreSQL target {target_description} holds {rows} row(s) in '{}' although no migration into it ever committed: it was served from or written to after the interrupted run, so abort cannot tell those rows from a half-finished copy and the marker stays in place; repair forward on PostgreSQL or empty the target by hand first",
                table.name
            )));
        }
    }
    Ok(())
}

/// Delete the copied content the way the copy wrote it.
async fn delete_copied_rows(
    transaction: &mut sqlx::Transaction<'_, Any>,
    tables: &[Table],
) -> Result<(), StorageMigrationError> {
    for table in tables {
        sqlx::query(&format!("DELETE FROM {}", quote_identifier(&table.name)))
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct BaselineRow {
    completed_at: String,
    table_count: i64,
    row_count: i64,
    content_sha256: String,
    sequence_sha256: String,
}

/// The configured source deployment, which must be the SQLite backend.
fn sqlite_source_config(
    config: &ServerConfig,
) -> Result<ServerDatabaseConfig, StorageMigrationError> {
    match config.storage_config()? {
        StorageConfig::Sqlite(sqlite) => Ok(sqlite),
        StorageConfig::Postgres(_) => Err(StorageMigrationError::Refused(format!(
            "the migration source {} is configured for the PostgreSQL Storage Backend; the Storage Backend Migration copies a SQLite deployment into PostgreSQL, and PostgreSQL -> SQLite has no direction",
            config.db_path.display()
        ))),
    }
}

/// The configured target deployment, which must be the PostgreSQL backend.
fn postgres_target_config(
    config: &ServerConfig,
) -> Result<PostgresConnectConfig, StorageMigrationError> {
    match config.storage_config()? {
        StorageConfig::Postgres(postgres) => Ok(postgres),
        StorageConfig::Sqlite(_) => Err(StorageMigrationError::Refused(format!(
            "the migration target {} is configured for the SQLite Storage Backend; the target must be an empty PostgreSQL database, and PostgreSQL -> SQLite has no direction",
            config.db_path.display()
        ))),
    }
}

fn describe_target(config: &PostgresConnectConfig) -> String {
    let host = match config.host() {
        PostgresHost::Tcp(host) => host.clone(),
        PostgresHost::Socket(path) => path.display().to_string(),
    };
    format!(
        "postgres://{}@{}:{}/{}",
        config.user(),
        host,
        config.port(),
        config.database()
    )
}

async fn open_source(
    config: &ServerDatabaseConfig,
) -> Result<SqliteConnection, StorageMigrationError> {
    // The copy reads the file in one pass, without a snapshot transaction, so
    // it must own the file even when the configuration was written for a
    // development Server. A development Server skips the ownership flock, which
    // leaves exclusive locking as the only thing that keeps a writer out from
    // under the copy; the command therefore forces it instead of inheriting it.
    let config = config.clone().with_exclusive_locking(true);
    let options = crate::database::sqlite_deployment_options(&config, false);
    let connection = SqliteConnection::connect_with(&options).await?;
    Ok(connection)
}

async fn count_rows(
    source: &mut SqliteConnection,
    table: &str,
) -> Result<u64, StorageMigrationError> {
    let rows: i64 =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}", quote_identifier(table)))
            .fetch_one(&mut *source)
            .await?;
    Ok(rows.max(0) as u64)
}

async fn verify_source_integrity(
    source: &mut SqliteConnection,
) -> Result<(), StorageMigrationError> {
    let rows = sqlx::query("PRAGMA integrity_check")
        .fetch_all(&mut *source)
        .await?;
    let mut verdicts = Vec::with_capacity(rows.len());
    for row in &rows {
        verdicts.push(row.try_get::<String, _>(0)?);
    }
    if verdicts.len() == 1 && verdicts[0] == "ok" {
        return Ok(());
    }
    Err(StorageMigrationError::Refused(format!(
        "the source SQLite database failed integrity_check and cannot be migrated: {}",
        verdicts.join("; ")
    )))
}

async fn verify_source_schema_version(
    source: &mut SqliteConnection,
) -> Result<(), StorageMigrationError> {
    let version: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(&mut *source)
        .await
        .map_err(|error| {
            StorageMigrationError::Refused(format!(
                "the source holds no Server schema history (_sqlx_migrations): {error}"
            ))
        })?;
    let Some(version) = version else {
        return Err(StorageMigrationError::Refused(
            "the source SQLite database holds no applied Server migration".to_owned(),
        ));
    };
    if version != SERVER_SCHEMA_VERSION {
        return Err(StorageMigrationError::Refused(format!(
            "the source SQLite schema is version {version} but this binary implements version {SERVER_SCHEMA_VERSION}: start the Server on SQLite with this binary first, so the file carries the schema the migration copies into",
        )));
    }
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 0")
            .fetch_one(&mut *source)
            .await?;
    if pending > 0 {
        return Err(StorageMigrationError::Refused(format!(
            "the source SQLite database holds {pending} failed migration record(s); repair it before migrating"
        )));
    }
    Ok(())
}

async fn verify_source_cutover(source: &mut SqliteConnection) -> Result<(), StorageMigrationError> {
    // The source carries the schema version this binary implements, so both
    // cutover tables exist and these probes ask the same question
    // cutover::detect_inventory_cutover asks of a Server pool.
    let converted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inventory_migration")
        .fetch_one(&mut *source)
        .await?;
    if converted == 0 {
        return Ok(());
    }
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM inventory_cutover WHERE singleton = 1")
            .fetch_optional(&mut *source)
            .await?;
    if state.as_deref() == Some("resumed") {
        return Ok(());
    }
    Err(StorageMigrationError::Refused(format!(
        "the source SQLite database is a converted deployment whose v1 -> v2 cutover was never resumed (state {}); finish the cutover on SQLite before migrating to PostgreSQL",
        state.as_deref().unwrap_or("missing")
    )))
}

// --- schema model -------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnClass {
    Text,
    Integer,
    Real,
    Blob,
}

impl ColumnClass {
    const fn name(self) -> &'static str {
        match self {
            Self::Text => "TEXT",
            Self::Integer => "INTEGER",
            Self::Real => "REAL",
            Self::Blob => "BLOB",
        }
    }

    fn from_sqlite_declared(declared: &str) -> Option<Self> {
        let token = declared.split_whitespace().next()?.to_ascii_uppercase();
        Some(match token.as_str() {
            "TEXT" | "VARCHAR" | "CHARACTER" | "CLOB" => Self::Text,
            "INTEGER" | "INT" | "BIGINT" => Self::Integer,
            "REAL" | "DOUBLE" | "FLOAT" => Self::Real,
            "BLOB" => Self::Blob,
            _ => return None,
        })
    }

    fn from_postgres_data_type(data_type: &str) -> Option<Self> {
        Some(match data_type {
            "text" | "character varying" | "character" => Self::Text,
            "bigint" | "integer" | "smallint" => Self::Integer,
            "double precision" | "real" => Self::Real,
            "bytea" => Self::Blob,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
struct Column {
    name: String,
    class: ColumnClass,
}

#[derive(Debug, Clone)]
struct Table {
    name: String,
    columns: Vec<Column>,
    /// Indexes into columns, in primary-key order. Empty for a table with no
    /// primary key; the copy then orders by every column.
    primary_key: Vec<usize>,
}

async fn read_source_schema(
    source: &mut SqliteConnection,
) -> Result<Vec<Table>, StorageMigrationError> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(&mut *source)
    .await?;
    let mut tables = Vec::with_capacity(names.len());
    for name in names {
        if BOOKKEEPING_TABLES.contains(&name.as_str()) {
            continue;
        }
        let rows = sqlx::query(&format!("PRAGMA table_info({})", quote_identifier(&name)))
            .fetch_all(&mut *source)
            .await?;
        let mut columns = Vec::with_capacity(rows.len());
        let mut primary_key = Vec::new();
        for row in &rows {
            let column_name: String = row.try_get("name")?;
            let declared: Option<String> = row.try_get("type")?;
            let declared = declared.unwrap_or_default();
            let class = ColumnClass::from_sqlite_declared(&declared).ok_or_else(|| {
                StorageMigrationError::Refused(format!(
                    "table '{name}' column '{column_name}' is declared '{declared}', which the Storage Backend Migration does not map onto a PostgreSQL column type"
                ))
            })?;
            let pk: i64 = row.try_get("pk")?;
            if pk > 0 {
                primary_key.push((pk, columns.len()));
            }
            columns.push(Column {
                name: column_name,
                class,
            });
        }
        if columns.is_empty() {
            return Err(StorageMigrationError::Refused(format!(
                "table '{name}' reports no column, so the migration cannot copy it"
            )));
        }
        primary_key.sort_unstable();
        tables.push(Table {
            name,
            columns,
            primary_key: primary_key.into_iter().map(|(_, index)| index).collect(),
        });
    }
    Ok(tables)
}

async fn read_target_schema(
    transaction: &mut sqlx::Transaction<'_, Any>,
) -> Result<Vec<Table>, StorageMigrationError> {
    // Every selected column is cast to text: the information_schema columns are
    // domains over name/character_data, which the sqlx Any seam does not
    // decode as String.
    let columns: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT c.table_name::text, c.column_name::text, c.data_type::text FROM information_schema.columns AS c JOIN information_schema.tables AS t ON t.table_schema = c.table_schema AND t.table_name = c.table_name WHERE c.table_schema = current_schema() AND t.table_type = 'BASE TABLE' ORDER BY c.table_name, c.ordinal_position",
    )
    .fetch_all(&mut **transaction)
    .await?;
    let keys: Vec<(String, String)> = sqlx::query_as(
        "SELECT k.table_name::text, k.column_name::text FROM information_schema.table_constraints AS c JOIN information_schema.key_column_usage AS k ON k.constraint_schema = c.constraint_schema AND k.constraint_name = c.constraint_name AND k.table_name = c.table_name WHERE c.constraint_type = 'PRIMARY KEY' AND c.table_schema = current_schema() ORDER BY k.table_name, k.ordinal_position",
    )
    .fetch_all(&mut **transaction)
    .await?;

    let mut tables: Vec<Table> = Vec::new();
    for (table_name, column_name, data_type) in columns {
        if BOOKKEEPING_TABLES.contains(&table_name.as_str()) {
            continue;
        }
        let class = ColumnClass::from_postgres_data_type(&data_type).ok_or_else(|| {
            StorageMigrationError::Refused(format!(
                "the PostgreSQL target table '{table_name}' column '{column_name}' is of type '{data_type}', which the Storage Backend Migration does not map onto a SQLite column type"
            ))
        })?;
        match tables.last_mut() {
            Some(table) if table.name == table_name => table.columns.push(Column {
                name: column_name,
                class,
            }),
            _ => tables.push(Table {
                name: table_name,
                columns: vec![Column {
                    name: column_name,
                    class,
                }],
                primary_key: Vec::new(),
            }),
        }
    }
    for (table_name, column_name) in keys {
        let Some(table) = tables.iter_mut().find(|table| table.name == table_name) else {
            continue;
        };
        if let Some(index) = table
            .columns
            .iter()
            .position(|column| column.name == column_name)
        {
            table.primary_key.push(index);
        }
    }
    Ok(tables)
}

fn compare_schemas(source: &[Table], target: &[Table]) -> Result<(), StorageMigrationError> {
    let source_names: Vec<&str> = source.iter().map(|table| table.name.as_str()).collect();
    let target_names: Vec<&str> = target.iter().map(|table| table.name.as_str()).collect();
    if source_names != target_names {
        let missing: Vec<&str> = source_names
            .iter()
            .copied()
            .filter(|name| !target_names.contains(name))
            .collect();
        let extra: Vec<&str> = target_names
            .iter()
            .copied()
            .filter(|name| !source_names.contains(name))
            .collect();
        return Err(StorageMigrationError::Refused(format!(
            "the SQLite source and the PostgreSQL target do not hold the same tables: the target lacks [{}] and adds [{}]",
            missing.join(", "),
            extra.join(", ")
        )));
    }
    for (source_table, target_table) in source.iter().zip(target.iter()) {
        if source_table.columns.len() != target_table.columns.len() {
            return Err(StorageMigrationError::Refused(format!(
                "table '{}' holds {} column(s) on SQLite and {} column(s) on PostgreSQL",
                source_table.name,
                source_table.columns.len(),
                target_table.columns.len()
            )));
        }
        for (source_column, target_column) in
            source_table.columns.iter().zip(target_table.columns.iter())
        {
            if source_column.name != target_column.name
                || source_column.class != target_column.class
            {
                return Err(StorageMigrationError::Refused(format!(
                    "table '{}' column '{}' ({}) does not match the target column '{}' ({})",
                    source_table.name,
                    source_column.name,
                    source_column.class.name(),
                    target_column.name,
                    target_column.class.name()
                )));
            }
        }
        if source_table.primary_key != target_table.primary_key {
            return Err(StorageMigrationError::Refused(format!(
                "table '{}' carries a different primary key on SQLite and on PostgreSQL",
                source_table.name
            )));
        }
    }
    Ok(())
}

/// The target must hold nothing of its own, or the copy would merge two
/// histories.
///
/// This runs inside the copy's own transaction, under the advisory lock the
/// target connection already holds, so nothing can populate the target between
/// the decision and the copy.
///
/// "Nothing of its own" allows exactly one exception: the reference rows the
/// Server migrations insert when they build a fresh schema (see
/// [MIGRATION_SEEDED_TABLES]). A brand new target already carries them by the
/// time this runs, and they are replaced by the source's own rows.
async fn prepare_target(
    transaction: &mut sqlx::Transaction<'_, Any>,
    tables: &[Table],
) -> Result<(), StorageMigrationError> {
    let migrated: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM storage_migration")
        .fetch_one(&mut **transaction)
        .await?;
    if migrated > 0 {
        return Err(StorageMigrationError::Refused(
            "the PostgreSQL target already carries a Storage Backend Migration baseline: it was the target of an earlier migration, so it is not the empty database this command requires".to_owned(),
        ));
    }
    for table in tables {
        let rows: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {}",
            quote_identifier(&table.name)
        ))
        .fetch_one(&mut **transaction)
        .await?;
        if rows == 0 {
            continue;
        }
        if MIGRATION_SEEDED_TABLES.contains(&table.name.as_str()) {
            sqlx::query(&format!("DELETE FROM {}", quote_identifier(&table.name)))
                .execute(&mut **transaction)
                .await?;
            continue;
        }
        return Err(StorageMigrationError::Refused(format!(
            "the PostgreSQL target already holds {rows} row(s) in '{}': the Storage Backend Migration only targets an empty database, because a target that has been serving would merge two histories",
            table.name
        )));
    }
    Ok(())
}

// --- copy, digest, verification ----------------------------------------

#[derive(Debug, Clone)]
struct TableDigest {
    rows: u64,
    sha256: String,
}

#[derive(Debug, Clone)]
enum Cell {
    Null,
    Text(String),
    Integer(i64),
    Real(f64),
    Blob(Vec<u8>),
}

fn quote_identifier(name: &str) -> String {
    let mut quoted = String::with_capacity(name.len() + 2);
    quoted.push('"');
    for character in name.chars() {
        if character == '"' {
            quoted.push('"');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

fn column_list(table: &Table) -> String {
    table
        .columns
        .iter()
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The deterministic row order both sides must agree on.
///
/// A primary key is unique and never NULL, so ordering by it is total and
/// backend independent. node_metric_series_state has no primary key, and the
/// two backends disagree about where NULLs sort, so the fallback orders every
/// column behind an explicit IS NULL flag.
fn order_clause(table: &Table) -> String {
    if !table.primary_key.is_empty() {
        return table
            .primary_key
            .iter()
            .map(|index| quote_identifier(&table.columns[*index].name))
            .collect::<Vec<_>>()
            .join(", ");
    }
    table
        .columns
        .iter()
        .map(|column| {
            let name = quote_identifier(&column.name);
            format!("({name} IS NULL), {name}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn select_statement(table: &Table) -> String {
    format!(
        "SELECT {} FROM {} ORDER BY {}",
        column_list(table),
        quote_identifier(&table.name),
        order_clause(table)
    )
}

fn rows_per_insert(table: &Table) -> usize {
    let bounded = MAX_BIND_PARAMETERS / table.columns.len().max(1);
    bounded.clamp(1, MAX_ROWS_PER_INSERT)
}

fn insert_statement(table: &Table, rows: usize) -> String {
    let mut statement = format!(
        "INSERT INTO {} ({}) VALUES ",
        quote_identifier(&table.name),
        column_list(table)
    );
    let mut parameter = 0_usize;
    for row in 0..rows {
        if row > 0 {
            statement.push_str(", ");
        }
        statement.push('(');
        for column in 0..table.columns.len() {
            if column > 0 {
                statement.push_str(", ");
            }
            parameter += 1;
            let _ = write!(statement, "${parameter}");
        }
        statement.push(')');
    }
    statement
}

fn hash_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn hash_cell(hasher: &mut Sha256, cell: &Cell) {
    match cell {
        Cell::Null => hasher.update(b"n"),
        Cell::Text(value) => {
            hasher.update(b"t");
            hash_bytes(hasher, value.as_bytes());
        }
        Cell::Integer(value) => {
            hasher.update(b"i");
            hasher.update(value.to_be_bytes());
        }
        Cell::Real(value) => {
            hasher.update(b"r");
            hasher.update(value.to_bits().to_be_bytes());
        }
        Cell::Blob(value) => {
            hasher.update(b"b");
            hash_bytes(hasher, value);
        }
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// The content digest of the whole migration: one line per table, in table
/// name order, so it depends on nothing but the stored content.
fn content_digest(digests: &BTreeMap<String, TableDigest>) -> String {
    let mut hasher = Sha256::new();
    for (table, digest) in digests {
        hasher.update(table.as_bytes());
        hasher.update(b"\n");
        hasher.update(digest.rows.to_string().as_bytes());
        hasher.update(b"\n");
        hasher.update(digest.sha256.as_bytes());
        hasher.update(b"\n");
    }
    hex_sha256(&hasher.finalize())
}

/// Reading one column out of a row, whichever database the row came from.
///
/// The source is SQLite and the target is PostgreSQL, so their row types are
/// unrelated: this is the only thing the copy and its verification need from
/// either, and it keeps the class-directed decode in one place.
trait RowCells {
    fn text(&self, index: usize) -> Result<Option<String>, sqlx::Error>;
    fn integer(&self, index: usize) -> Result<Option<i64>, sqlx::Error>;
    fn real(&self, index: usize) -> Result<Option<f64>, sqlx::Error>;
    fn blob(&self, index: usize) -> Result<Option<Vec<u8>>, sqlx::Error>;
}

impl RowCells for SqliteRow {
    fn text(&self, index: usize) -> Result<Option<String>, sqlx::Error> {
        self.try_get(index)
    }
    fn integer(&self, index: usize) -> Result<Option<i64>, sqlx::Error> {
        self.try_get(index)
    }
    fn real(&self, index: usize) -> Result<Option<f64>, sqlx::Error> {
        self.try_get(index)
    }
    fn blob(&self, index: usize) -> Result<Option<Vec<u8>>, sqlx::Error> {
        self.try_get(index)
    }
}

impl RowCells for AnyRow {
    fn text(&self, index: usize) -> Result<Option<String>, sqlx::Error> {
        self.try_get(index)
    }
    fn integer(&self, index: usize) -> Result<Option<i64>, sqlx::Error> {
        self.try_get(index)
    }
    fn real(&self, index: usize) -> Result<Option<f64>, sqlx::Error> {
        self.try_get(index)
    }
    fn blob(&self, index: usize) -> Result<Option<Vec<u8>>, sqlx::Error> {
        self.try_get(index)
    }
}

/// Decode one cell by the storage class its column declares, so a NULL keeps
/// the class instead of collapsing into an untyped absence.
fn decode_cell<R: RowCells>(
    row: &R,
    index: usize,
    class: ColumnClass,
) -> Result<Cell, StorageMigrationError> {
    Ok(match class {
        ColumnClass::Text => row.text(index)?.map_or(Cell::Null, Cell::Text),
        ColumnClass::Integer => row.integer(index)?.map_or(Cell::Null, Cell::Integer),
        ColumnClass::Real => row.real(index)?.map_or(Cell::Null, Cell::Real),
        ColumnClass::Blob => row.blob(index)?.map_or(Cell::Null, Cell::Blob),
    })
}

type AnyQuery<'q> = Query<'q, Any, AnyArguments<'q>>;

/// Bind one cell with the class its column declares.
///
/// A NULL must be bound as a null of the column's own type: the cached
/// PostgreSQL statement then keeps float8 for a missing sample instead of
/// declaring float4 (ADR 0013, [NullableReal]).
fn bind_cell<'q>(query: AnyQuery<'q>, class: ColumnClass, cell: Cell) -> AnyQuery<'q> {
    match cell {
        Cell::Null => match class {
            ColumnClass::Text => query.bind(None::<String>),
            ColumnClass::Integer => query.bind(None::<i64>),
            ColumnClass::Real => query.bind(NullableReal(None)),
            ColumnClass::Blob => query.bind(None::<Vec<u8>>),
        },
        Cell::Text(value) => query.bind(value),
        Cell::Integer(value) => query.bind(value),
        Cell::Real(value) => query.bind(NullableReal(Some(value))),
        Cell::Blob(value) => query.bind(value),
    }
}

/// Stream one table out of the source, insert it into the target, and return
/// the digest of exactly the bytes the source held.
async fn copy_table(
    source: &mut SqliteConnection,
    transaction: &mut sqlx::Transaction<'_, Any>,
    table: &Table,
) -> Result<TableDigest, StorageMigrationError> {
    let statement = select_statement(table);
    let batch = rows_per_insert(table);
    let mut hasher = Sha256::new();
    let mut rows = 0_u64;
    let mut pending: Vec<Vec<Cell>> = Vec::with_capacity(batch);
    let mut stream = sqlx::query(&statement).fetch(&mut *source);
    while let Some(row) = stream.next().await {
        let row = row?;
        let mut cells = Vec::with_capacity(table.columns.len());
        for (index, column) in table.columns.iter().enumerate() {
            let cell = decode_cell(&row, index, column.class)?;
            hash_cell(&mut hasher, &cell);
            cells.push(cell);
        }
        rows += 1;
        pending.push(cells);
        if pending.len() == batch {
            let flushed = std::mem::take(&mut pending);
            insert_rows(transaction, table, flushed).await?;
        }
    }
    if !pending.is_empty() {
        insert_rows(transaction, table, pending).await?;
    }
    Ok(TableDigest {
        rows,
        sha256: hex_sha256(&hasher.finalize()),
    })
}

async fn insert_rows(
    target: &mut AnyConnection,
    table: &Table,
    rows: Vec<Vec<Cell>>,
) -> Result<(), StorageMigrationError> {
    if rows.is_empty() {
        return Ok(());
    }
    let statement = insert_statement(table, rows.len());
    let mut query = sqlx::query(&statement);
    for cells in rows {
        for (column, cell) in table.columns.iter().zip(cells) {
            query = bind_cell(query, column.class, cell);
        }
    }
    query.execute(&mut *target).await?;
    Ok(())
}

async fn target_table_digest(
    transaction: &mut sqlx::Transaction<'_, Any>,
    table: &Table,
) -> Result<TableDigest, StorageMigrationError> {
    let statement = select_statement(table);
    let mut hasher = Sha256::new();
    let mut rows = 0_u64;
    let mut stream = sqlx::query(&statement).fetch(&mut **transaction);
    while let Some(row) = stream.next().await {
        let row = row?;
        for (index, column) in table.columns.iter().enumerate() {
            let cell = decode_cell(&row, index, column.class)?;
            hash_cell(&mut hasher, &cell);
        }
        rows += 1;
    }
    Ok(TableDigest {
        rows,
        sha256: hex_sha256(&hasher.finalize()),
    })
}

/// Every identity column of the target schema, in name order.
async fn identity_columns(
    transaction: &mut sqlx::Transaction<'_, Any>,
) -> Result<Vec<(String, String)>, StorageMigrationError> {
    Ok(sqlx::query_as(
        "SELECT table_name::text, column_name::text FROM information_schema.columns WHERE table_schema = current_schema() AND is_identity = 'YES' ORDER BY table_name, ordinal_position",
    )
    .fetch_all(&mut **transaction)
    .await?)
}

/// The sequence behind one identity column, as the catalog names it.
async fn identity_sequence(
    transaction: &mut sqlx::Transaction<'_, Any>,
    table: &str,
    column: &str,
) -> Result<Option<String>, StorageMigrationError> {
    Ok(sqlx::query_scalar("SELECT pg_get_serial_sequence($1, $2)")
        .bind(table)
        .bind(column)
        .fetch_one(&mut **transaction)
        .await?)
}

/// The identity sequences as `<table>.<column>=<last_value>:<is_called>` lines,
/// hashed.
///
/// This is the witness of a write that leaves no row behind. PostgreSQL
/// advances a sequence outside the transaction that consumes it, so an insert
/// that rolled back still moves it. The copy advances the sequences itself, but
/// transactionally (`ALTER SEQUENCE ... RESTART`, which a rollback undoes), so
/// this digest records a state only a later writer can move - and
/// `storage migrate abort` must notice that before it hands the SQLite file
/// back.
async fn identity_sequence_digest(
    transaction: &mut sqlx::Transaction<'_, Any>,
) -> Result<String, StorageMigrationError> {
    let mut hasher = Sha256::new();
    for (table, column) in identity_columns(transaction).await? {
        let Some(sequence) = identity_sequence(transaction, &table, &column).await? else {
            continue;
        };
        // `is_called` is part of the witness: a sequence this table set past
        // its maximum is handed out exactly once before it moves, so a single
        // rolled-back insert changes nothing but this flag.
        let (last_value, is_called): (i64, i64) = sqlx::query_as(&format!(
            "SELECT last_value, is_called::int FROM {sequence}"
        ))
        .fetch_one(&mut **transaction)
        .await?;
        hasher.update(format!("{table}.{column}={last_value}:{is_called}\n").as_bytes());
    }
    Ok(hex_sha256(&hasher.finalize()))
}

/// Advance every identity sequence past the ids the copy stored, so the first
/// insert after the migration cannot collide with a copied row.
async fn advance_identities(
    transaction: &mut sqlx::Transaction<'_, Any>,
) -> Result<usize, StorageMigrationError> {
    let columns = identity_columns(transaction).await?;
    for (table, column) in &columns {
        let maximum: Option<i64> = sqlx::query_scalar(&format!(
            "SELECT MAX({}) FROM {}",
            quote_identifier(column),
            quote_identifier(table)
        ))
        .fetch_one(&mut **transaction)
        .await?;
        let next = maximum.map_or(1, |value| value.saturating_add(1));
        let Some(sequence) = identity_sequence(transaction, table, column).await? else {
            return Err(StorageMigrationError::Verification(format!(
                "the identity column {table}.{column} has no sequence, so the first insert after the migration could collide"
            )));
        };
        // ALTER SEQUENCE ... RESTART is transactional where setval is not: a
        // copy that fails after this point - the baseline insert, the deferred
        // foreign keys at the commit - takes the restarted sequences down with
        // it, so no part of a failed attempt survives.
        sqlx::query(&format!("ALTER SEQUENCE {sequence} RESTART WITH {next}"))
            .execute(&mut **transaction)
            .await?;
        // Reading the sequence back is the sequence half of the verification.
        let (applied, is_called): (i64, i64) = sqlx::query_as(&format!(
            "SELECT last_value, is_called::int FROM {sequence}"
        ))
        .fetch_one(&mut **transaction)
        .await?;
        if applied != next || is_called != 0 {
            return Err(StorageMigrationError::Verification(format!(
                "the identity sequence of {table}.{column} stored {applied} (is_called {is_called}) instead of a restart at {next}, so the first insert after the migration could collide"
            )));
        }
    }
    Ok(columns.len())
}

fn estimate_remaining(elapsed: Duration, copied: u64, total: u64) -> Option<Duration> {
    if copied == 0 || total <= copied {
        return None;
    }
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 {
        return None;
    }
    let rate = copied as f64 / seconds;
    if !rate.is_finite() || rate <= 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64((total - copied) as f64 / rate))
}

fn u64_to_i64(value: u64, what: &str) -> Result<i64, StorageMigrationError> {
    i64::try_from(value).map_err(|_| {
        StorageMigrationError::Verification(format!(
            "the {what} {value} does not fit a PostgreSQL BIGINT"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn table(name: &str, columns: &[(&str, ColumnClass)], primary_key: &[usize]) -> Table {
        Table {
            name: name.to_owned(),
            columns: columns
                .iter()
                .map(|(column, class)| Column {
                    name: (*column).to_owned(),
                    class: *class,
                })
                .collect(),
            primary_key: primary_key.to_vec(),
        }
    }

    fn digest(rows: u64, sha256: &str) -> TableDigest {
        TableDigest {
            rows,
            sha256: sha256.to_owned(),
        }
    }

    /// The source of every migration test is a SQLite deployment whichever
    /// backend this binary covers: only the target carries the storage matrix.
    fn source_config(dir: &Path) -> ServerDatabaseConfig {
        ServerDatabaseConfig::new(dir.join("server.db"))
    }

    fn marker_for(path: &Path) -> MigrationMarker {
        MigrationMarker {
            state: MarkerState::Completed,
            started_at: "2026-10-22T23:59:00Z".to_owned(),
            completed_at: Some("2026-10-23T00:00:00Z".to_owned()),
            source_path: path.display().to_string(),
            source_schema_version: SERVER_SCHEMA_VERSION,
            target_description: "postgres://platpulse@db:5432/platpulse".to_owned(),
            table_count: 82,
            row_count: 6,
            content_sha256: Some("0".repeat(64)),
        }
    }

    // --- deterministic ordering, statements and batching -------------------

    #[test]
    fn the_copy_orders_by_the_primary_key_when_there_is_one() {
        let table = table(
            "audit_events",
            &[
                ("audit_event_id", ColumnClass::Integer),
                ("event_kind", ColumnClass::Text),
            ],
            &[0],
        );
        assert_eq!(order_clause(&table), "\"audit_event_id\"");
    }

    #[test]
    fn a_table_without_a_primary_key_orders_every_column_behind_an_is_null_flag() {
        let table = table(
            "node_metric_series_state",
            &[
                ("node_id", ColumnClass::Text),
                ("metric", ColumnClass::Text),
            ],
            &[],
        );
        assert_eq!(
            order_clause(&table),
            "(\"node_id\" IS NULL), \"node_id\", (\"metric\" IS NULL), \"metric\""
        );
    }

    #[test]
    fn an_insert_statement_numbers_every_placeholder_once() {
        let table = table(
            "networks",
            &[
                ("network_key", ColumnClass::Text),
                ("chain_id", ColumnClass::Integer),
            ],
            &[0],
        );
        assert_eq!(
            insert_statement(&table, 2),
            "INSERT INTO \"networks\" (\"network_key\", \"chain_id\") VALUES ($1, $2), ($3, $4)"
        );
    }

    #[test]
    fn a_huge_table_still_stays_inside_the_bind_parameter_budget() {
        let columns: Vec<(&str, ColumnClass)> = (0..MAX_BIND_PARAMETERS)
            .map(|_| ("column", ColumnClass::Text))
            .collect();
        let huge = table("huge", &columns, &[0]);
        assert_eq!(rows_per_insert(&huge), 1);
        let narrow = table("narrow", &[("only", ColumnClass::Text)], &[0]);
        assert_eq!(rows_per_insert(&narrow), MAX_ROWS_PER_INSERT);
    }

    // --- digests -----------------------------------------------------------

    #[test]
    fn the_content_digest_covers_table_names_row_counts_and_table_digests() {
        let mut original = BTreeMap::new();
        original.insert("agents".to_owned(), digest(2, "aa"));
        original.insert("networks".to_owned(), digest(1, "bb"));

        let mut reordered = BTreeMap::new();
        reordered.insert("networks".to_owned(), digest(1, "bb"));
        reordered.insert("agents".to_owned(), digest(2, "aa"));
        assert_eq!(content_digest(&original), content_digest(&reordered));

        let mut extra_row = original.clone();
        extra_row.insert("agents".to_owned(), digest(3, "aa"));
        assert_ne!(content_digest(&original), content_digest(&extra_row));

        let mut renamed = BTreeMap::new();
        renamed.insert("agent".to_owned(), digest(2, "aa"));
        renamed.insert("networks".to_owned(), digest(1, "bb"));
        assert_ne!(content_digest(&original), content_digest(&renamed));
    }

    #[test]
    fn a_null_and_an_empty_text_hash_differently() {
        let mut null = Sha256::new();
        hash_cell(&mut null, &Cell::Null);
        let mut empty = Sha256::new();
        hash_cell(&mut empty, &Cell::Text(String::new()));
        assert_ne!(hex_sha256(&null.finalize()), hex_sha256(&empty.finalize()));
    }

    #[test]
    fn shifting_a_byte_between_two_values_changes_the_row_digest() {
        let mut left = Sha256::new();
        hash_cell(&mut left, &Cell::Text("ab".to_owned()));
        hash_cell(&mut left, &Cell::Text("c".to_owned()));
        let mut right = Sha256::new();
        hash_cell(&mut right, &Cell::Text("a".to_owned()));
        hash_cell(&mut right, &Cell::Text("bc".to_owned()));
        assert_ne!(hex_sha256(&left.finalize()), hex_sha256(&right.finalize()));
    }

    #[test]
    fn an_integer_and_a_real_with_the_same_number_hash_differently() {
        let mut integer = Sha256::new();
        hash_cell(&mut integer, &Cell::Integer(1));
        let mut real = Sha256::new();
        hash_cell(&mut real, &Cell::Real(1.0));
        assert_ne!(
            hex_sha256(&integer.finalize()),
            hex_sha256(&real.finalize())
        );
    }

    // --- the shared schema model -------------------------------------------

    #[test]
    fn both_backends_declare_the_same_four_column_classes() {
        for (declared, data_type, class) in [
            ("TEXT", "text", ColumnClass::Text),
            ("TEXT COLLATE NOCASE", "text", ColumnClass::Text),
            ("INTEGER", "bigint", ColumnClass::Integer),
            ("REAL", "double precision", ColumnClass::Real),
            ("BLOB", "bytea", ColumnClass::Blob),
        ] {
            assert_eq!(
                ColumnClass::from_sqlite_declared(declared),
                Some(class),
                "{declared}"
            );
            assert_eq!(
                ColumnClass::from_postgres_data_type(data_type),
                Some(class),
                "{data_type}"
            );
        }
        assert_eq!(ColumnClass::from_sqlite_declared(""), None);
        assert_eq!(ColumnClass::from_sqlite_declared("NUMERIC"), None);
        assert_eq!(ColumnClass::from_postgres_data_type("numeric"), None);
    }

    #[test]
    fn schema_comparison_names_the_table_the_target_lacks() {
        let source = vec![
            table("agents", &[("agent_id", ColumnClass::Text)], &[0]),
            table("nodes", &[("node_id", ColumnClass::Text)], &[0]),
        ];
        let target = vec![table("agents", &[("agent_id", ColumnClass::Text)], &[0])];
        let error = compare_schemas(&source, &target).expect_err("the target lacks a table");
        assert!(error.to_string().contains("nodes"), "{error}");
    }

    #[test]
    fn schema_comparison_reports_a_column_whose_class_drifted() {
        let source = vec![table(
            "agents",
            &[("agent_epoch", ColumnClass::Integer)],
            &[],
        )];
        let target = vec![table("agents", &[("agent_epoch", ColumnClass::Text)], &[])];
        let error = compare_schemas(&source, &target).expect_err("the classes differ");
        assert!(error.to_string().contains("INTEGER"), "{error}");
        assert!(error.to_string().contains("TEXT"), "{error}");
    }

    #[test]
    fn schema_comparison_reports_a_primary_key_that_moved() {
        let source = vec![table(
            "networks",
            &[
                ("network_key", ColumnClass::Text),
                ("chain_id", ColumnClass::Integer),
            ],
            &[0],
        )];
        let target = vec![table(
            "networks",
            &[
                ("network_key", ColumnClass::Text),
                ("chain_id", ColumnClass::Integer),
            ],
            &[1],
        )];
        let error = compare_schemas(&source, &target).expect_err("the primary keys differ");
        assert!(
            error.to_string().contains("different primary key"),
            "{error}"
        );
    }

    // --- marker and progress helpers ---------------------------------------

    #[test]
    fn the_marker_is_a_sibling_of_the_database_file() {
        assert_eq!(
            marker_path(Path::new("/srv/platpulse/server.db")),
            PathBuf::from("/srv/platpulse/server.db.migrated")
        );
    }

    #[test]
    fn the_eta_appears_only_once_the_copy_has_moved_some_rows() {
        assert_eq!(estimate_remaining(Duration::from_secs(5), 0, 10), None);
        assert_eq!(estimate_remaining(Duration::from_secs(5), 10, 10), None);
        assert_eq!(estimate_remaining(Duration::ZERO, 5, 10), None);
        assert_eq!(
            estimate_remaining(Duration::from_secs(5), 50, 100),
            Some(Duration::from_secs(5))
        );
    }

    // --- the marker gate ---------------------------------------------------

    #[tokio::test]
    #[cfg_attr(
        storage_postgres,
        ignore = "the migration marker guards the SQLite database file"
    )]
    async fn a_migrated_sqlite_file_refuses_to_serve() {
        if !crate::test_support::skip_unless_sqlite(
            "the migration marker guards the SQLite database file",
        ) {
            return;
        }
        let dir = TempDir::new().expect("a temporary directory");
        let config = crate::test_support::test_config(dir.path());
        let database = crate::database::initialize(config.clone())
            .await
            .expect("open the SQLite database");
        database.close().await;
        claim_marker(&marker_path(config.path()), &marker_for(config.path()))
            .expect("write the migration marker");

        let error = match crate::database::initialize(config.clone()).await {
            Ok(database) => {
                database.close().await;
                panic!("a migrated SQLite file must not serve");
            }
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(
            message.contains("was migrated to the PostgreSQL Storage Backend"),
            "{message}"
        );
        assert!(message.contains("storage migrate abort"), "{message}");
        let refused = refuse_migrated(config.path()).expect_err("the marker refuses every open");
        assert!(
            refused.contains("postgres://platpulse@db:5432/platpulse"),
            "{refused}"
        );
    }

    #[tokio::test]
    #[cfg_attr(
        storage_postgres,
        ignore = "the migration marker guards the SQLite database file"
    )]
    async fn a_damaged_marker_fails_closed() {
        if !crate::test_support::skip_unless_sqlite(
            "the migration marker guards the SQLite database file",
        ) {
            return;
        }
        let dir = TempDir::new().expect("a temporary directory");
        let db_path = dir.path().join("server.db");
        std::fs::write(marker_path(&db_path), "{ not a marker").expect("write a damaged marker");
        let refused =
            refuse_migrated(&db_path).expect_err("a damaged marker refuses instead of allowing");
        assert!(refused.contains("cannot be read"), "{refused}");
    }

    // --- the two-backend end-to-end checks ---------------------------------

    /// A SQLite deployment holding representative content across the whole
    /// column-class matrix, so the copy has TEXT, INTEGER, REAL (with a NULL)
    /// and BLOB to carry, plus identity rows it must advance past.
    async fn seeded_source(dir: &Path) -> (ServerDatabaseConfig, u64) {
        let config = source_config(dir);
        let database = crate::database::initialize(config.clone())
            .await
            .expect("open the SQLite source deployment");
        let pool = database.pool();
        let rule_key: String =
            sqlx::query_scalar("SELECT rule_key FROM alert_rules ORDER BY rule_key LIMIT 1")
                .fetch_one(pool)
                .await
                .expect("the seeded Alert Rule catalog holds a rule");
        sqlx::query(
            "INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('lat1', 'PlatON Testnet', 'genesis-hash', 2206132, 210425, 'lat', '2026-10-20T00:00:00Z', '2026-10-20T00:00:00Z')",
        )
        .execute(pool)
        .await
        .expect("insert a Network");
        sqlx::query(
            "INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, before_json, after_json, created_at) VALUES (NULL, 'network.create', 'network', 'lat1', NULL, '{\"network\":\"lat1\"}', '2026-10-20T00:00:00Z')",
        )
        .execute(pool)
        .await
        .expect("insert an Audit Event");
        sqlx::query(
            "INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, before_json, after_json, created_at) VALUES (NULL, 'network.update', 'network', 'lat1', '{\"network\":\"lat1\"}', NULL, '2026-10-20T00:01:00Z')",
        )
        .execute(pool)
        .await
        .expect("insert a second Audit Event");
        sqlx::query(
            "INSERT INTO enrollment_tokens (token_id, token_digest, created_at, expires_at) VALUES ('token-1', X'00010203ff', '2026-10-20T00:00:00Z', '2026-10-21T00:00:00Z')",
        )
        .execute(pool)
        .await
        .expect("insert an Enrollment Token");
        for (subject_key, input_kind, input_value) in [
            ("server", "known", Some(12.5_f64)),
            ("server-2", "unknown", None),
        ] {
            sqlx::query(
                "INSERT INTO alert_rule_state (rule_key, subject_kind, subject_key, state, since, input_kind, input_value, last_evaluated_at) VALUES ($1, 'server', $2, 'normal', '2026-10-20T00:00:00Z', $3, $4, '2026-10-20T00:00:00Z')",
            )
            .bind(&rule_key)
            .bind(subject_key)
            .bind(input_kind)
            .bind(input_value)
            .execute(pool)
            .await
            .expect("insert a rule state");
        }
        let copied_tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT IN ('_sqlx_migrations', 'storage_migration')",
        )
        .fetch_one(pool)
        .await
        .expect("count the tables the migration copies");
        database.close().await;
        (config, copied_tables as u64)
    }

    /// Empty every table the target holds, so a target a test prepared itself
    /// still looks like the strictly empty database the migration requires.
    async fn empty_target(database: &ServerDatabase) {
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT format('%I', table_name) FROM information_schema.tables WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' AND table_name <> '_sqlx_migrations'",
        )
        .fetch_all(database.pool())
        .await
        .expect("list the target tables");
        sqlx::query(&format!("TRUNCATE TABLE {}", tables.join(", ")))
            .execute(database.pool())
            .await
            .expect("empty the target");
    }

    #[tokio::test]
    async fn the_migration_copies_every_table_and_advances_the_identities() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration end-to-end check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let dir = TempDir::new().expect("a temporary directory");
        let (source, copied_tables) = seeded_source(dir.path()).await;

        let mut reported = Vec::new();
        let summary = migrate_deployment(&source, &target, &mut |progress| {
            reported.push(progress.table.to_owned());
        })
        .await
        .expect("migrate the SQLite deployment into PostgreSQL");
        assert_eq!(summary.table_count, copied_tables);
        assert_eq!(
            reported.len() as u64,
            copied_tables,
            "every copied table reported its progress"
        );
        assert!(
            summary.row_count >= 6,
            "the copy stored only {} row(s)",
            summary.row_count
        );
        assert_eq!(
            summary.sequence_count, 8,
            "every identity column was advanced"
        );
        assert_eq!(summary.content_sha256.len(), 64);
        assert!(
            marker_path(source.path()).exists(),
            "the migrated source carries its marker"
        );

        // The marker gate takes the source out of service.
        let error = match crate::database::initialize(source.clone()).await {
            Ok(database) => {
                database.close().await;
                panic!("the migrated file must not serve");
            }
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("was migrated to the PostgreSQL Storage Backend"),
            "{error}"
        );

        // The target holds exactly the copied content, bytes and NULLs included.
        let migrated = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the migrated target");
        let token: Vec<u8> = sqlx::query_scalar(
            "SELECT token_digest FROM enrollment_tokens WHERE token_id = 'token-1'",
        )
        .fetch_one(migrated.pool())
        .await
        .expect("the copied Enrollment Token digest");
        assert_eq!(token, vec![0x00, 0x01, 0x02, 0x03, 0xff]);
        let samples: Vec<Option<f64>> =
            sqlx::query_scalar("SELECT input_value FROM alert_rule_state ORDER BY subject_key")
                .fetch_all(migrated.pool())
                .await
                .expect("the copied rule-state samples");
        assert_eq!(samples, vec![Some(12.5), None]);
        let networks: i64 =
            sqlx::query_scalar("SELECT chain_id FROM networks WHERE network_key = 'lat1'")
                .fetch_one(migrated.pool())
                .await
                .expect("the copied Network");
        assert_eq!(networks, 2206132);
        // The first insert after the migration must not collide with a copied id.
        let next_id: i64 = sqlx::query_scalar(
            "INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, before_json, after_json, created_at) VALUES (NULL, 'network.update', 'network', 'lat1', NULL, NULL, '2026-10-20T00:02:00Z') RETURNING audit_event_id",
        )
        .fetch_one(migrated.pool())
        .await
        .expect("the first insert after the migration");
        assert_eq!(next_id, 3);
        migrated.close().await;

        // That insert is a business write after the baseline, so the abort has
        // to refuse and point forward instead of reinstating the file.
        let error = abort_deployment(&source, &target)
            .await
            .expect_err("a write after the baseline refuses the abort");
        assert!(
            matches!(error, StorageMigrationError::ForwardRepairRequired(_)),
            "{error}"
        );
        assert!(
            marker_path(source.path()).exists(),
            "a refused abort keeps the marker"
        );
    }

    #[tokio::test]
    async fn aborting_before_any_write_empties_the_target_and_removes_the_marker() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration abort check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let dir = TempDir::new().expect("a temporary directory");
        let (source, copied_tables) = seeded_source(dir.path()).await;
        let migrated = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("migrate the SQLite deployment");
        assert_eq!(migrated.table_count, copied_tables);

        let abort = abort_deployment(&source, &target)
            .await
            .expect("abort a migration the target has not written to");
        assert_eq!(abort.table_count, migrated.table_count);
        assert_eq!(abort.row_count, migrated.row_count);
        assert_eq!(abort.completed_at, migrated.completed_at);
        assert!(
            !marker_path(source.path()).exists(),
            "a successful abort removes the marker"
        );

        // The SQLite file serves again ...
        let reopened = crate::database::initialize(source.clone())
            .await
            .expect("the source serves again once the marker is gone");
        reopened.close().await;

        // ... the target is empty again ...
        let empty = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the aborted target");
        let remaining: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM audit_events), (SELECT COUNT(*) FROM alert_rules)",
        )
        .fetch_one(empty.pool())
        .await
        .expect("the target row counts after the abort");
        assert_eq!(remaining, (0, 0), "the abort deletes every copied row");
        empty.close().await;

        // ... and the retry succeeds.
        let retried = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("retry the migration after an abort");
        assert_eq!(retried.row_count, migrated.row_count);
    }

    #[tokio::test]
    async fn a_failure_in_the_middle_of_the_copy_leaves_the_target_empty_and_retryable() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration failure check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let injection = "injected Storage Backend Migration failure";
        let holder = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the target");
        crate::test_support::install_failure_trigger(
            holder.pool(),
            "storage_migration_refuse_tokens",
            "BEFORE",
            "INSERT",
            "enrollment_tokens",
            injection,
        )
        .await
        .expect("install the failure trigger");
        // Preparing the target left the reference rows its migrations insert;
        // a database the migrations have already run on is not empty, so the
        // test hands the migration a strictly empty one.
        empty_target(&holder).await;
        holder.close().await;

        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        let error = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect_err("the injected failure must fail the copy");
        assert!(error.to_string().contains(injection), "{error}");
        assert!(
            !marker_path(source.path()).exists(),
            "a failed migration writes no marker"
        );

        // The copy runs in one transaction, so nothing it wrote survives. The
        // tables named here sort before enrollment_tokens, so the copy had
        // already written them when the trigger fired.
        let rolled_back = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the target after the failure");
        let leftover: (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM alert_rules), (SELECT COUNT(*) FROM audit_events), (SELECT COUNT(*) FROM storage_migration)",
        )
        .fetch_one(rolled_back.pool())
        .await
        .expect("the target row counts after the failure");
        assert_eq!(leftover, (0, 0, 0), "a failed copy leaves the target empty");
        crate::test_support::drop_failure_trigger(
            rolled_back.pool(),
            "storage_migration_refuse_tokens",
            "enrollment_tokens",
        )
        .await
        .expect("remove the failure trigger");
        rolled_back.close().await;

        let retried = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("the retry succeeds once the target accepts writes again");
        assert!(retried.row_count >= 6);
    }

    #[tokio::test]
    async fn a_target_that_already_holds_rows_is_refused() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration emptiness check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let holder = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the target");
        sqlx::query(
            "INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('lat1', 'PlatON Testnet', 'genesis-hash', 2206132, 210425, 'lat', '2026-10-20T00:00:00Z', '2026-10-20T00:00:00Z')",
        )
        .execute(holder.pool())
        .await
        .expect("write into the target");
        holder.close().await;

        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        let error = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect_err("a non-empty target is refused");
        assert!(
            error.to_string().contains("only targets an empty"),
            "{error}"
        );
        assert!(
            !marker_path(source.path()).exists(),
            "a refusal writes no marker"
        );
    }

    // --- the refusals the acceptance criteria name ---------------------------

    /// A target the refusals below never reach: each one fires before the
    /// migration opens the PostgreSQL side.
    fn unused_target() -> crate::database::PostgresConnectConfig {
        crate::database::PostgresConnectConfig::new(
            crate::database::PostgresHost::Tcp("127.0.0.1".to_owned()),
            1,
            "unused",
            "unused",
            crate::database::PostgresPassword::new("unused".to_owned())
                .expect("a password for the unused target"),
        )
    }

    #[tokio::test]
    async fn a_source_a_running_server_owns_is_refused() {
        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        let guard = crate::ownership::acquire(source.path())
            .expect("hold the source the way a running Server does");

        let error = migrate_deployment(&source, &unused_target(), &mut |_| {})
            .await
            .expect_err("a source a Server still owns is refused");
        assert!(
            matches!(error, StorageMigrationError::Ownership(_)),
            "{error}"
        );
        drop(guard);
        assert!(
            !marker_path(source.path()).exists(),
            "a source that is in use never gets fenced"
        );
    }

    #[tokio::test]
    async fn a_corrupt_source_is_refused() {
        let dir = TempDir::new().expect("a temporary directory");
        let config = source_config(dir.path());
        std::fs::write(config.path(), b"this file is not a SQLite database\n")
            .expect("write a file that is not a database");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(config.path(), std::fs::Permissions::from_mode(0o600))
                .expect("keep the file private");
        }

        let error = migrate_deployment(&config, &unused_target(), &mut |_| {})
            .await
            .expect_err("a corrupt source is refused");
        let message = error.to_string().to_lowercase();
        assert!(message.contains("not a database"), "{error}");
        assert!(
            !marker_path(config.path()).exists(),
            "the claim goes with the failed copy"
        );
    }

    #[tokio::test]
    async fn a_source_from_another_schema_version_is_refused() {
        let dir = TempDir::new().expect("a temporary directory");
        let config = source_config(dir.path());
        let database = crate::database::initialize(config.clone())
            .await
            .expect("open the source deployment");
        // A binary from a later release migrated this file: its recorded
        // schema version is one this binary does not implement.
        let newer = SERVER_SCHEMA_VERSION + 1;
        sqlx::query(&format!(
            "UPDATE _sqlx_migrations SET version = {newer} WHERE version = {SERVER_SCHEMA_VERSION}"
        ))
        .execute(database.pool())
        .await
        .expect("advance the recorded schema version");
        database.close().await;

        let error = migrate_deployment(&config, &unused_target(), &mut |_| {})
            .await
            .expect_err("a source from another schema version is refused");
        assert!(
            error.to_string().contains(&format!("version {newer}")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_source_whose_cutover_never_resumed_is_refused() {
        let dir = TempDir::new().expect("a temporary directory");
        let config = source_config(dir.path());
        let database = crate::database::initialize(config.clone())
            .await
            .expect("open the source deployment");
        // A converted deployment holds its inventory_migration record; the
        // cutover that reconnects the node never resumed, so the file is
        // half-way between the two inventory protocols.
        sqlx::query(
            "INSERT INTO inventory_migration (singleton, protocol_major, preserved_revision, previous_sha256, fingerprint_sha256, converted_at) VALUES (1, 2, 0, NULL, NULL, '2026-10-20T00:00:00Z')",
        )
        .execute(database.pool())
        .await
        .expect("record the conversion");
        database.close().await;

        let error = migrate_deployment(&config, &unused_target(), &mut |_| {})
            .await
            .expect_err("a cutover that never resumed is refused");
        assert!(error.to_string().contains("never resumed"), "{error}");
        assert!(error.to_string().contains("state missing"), "{error}");
    }

    #[tokio::test]
    #[cfg_attr(
        storage_postgres,
        ignore = "the migration marker guards the SQLite database file"
    )]
    async fn an_unfinished_run_fences_the_file_and_names_the_abort() {
        if !crate::test_support::skip_unless_sqlite(
            "the migration marker guards the SQLite database file",
        ) {
            return;
        }
        let dir = TempDir::new().expect("a temporary directory");
        let db_path = dir.path().join("server.db");
        let mut marker = marker_for(&db_path);
        marker.state = MarkerState::InProgress;
        marker.completed_at = None;
        marker.table_count = 0;
        marker.row_count = 0;
        marker.content_sha256 = None;
        std::fs::write(
            marker_path(&db_path),
            serde_json::to_vec_pretty(&marker).expect("serialize the claim"),
        )
        .expect("claim the source");

        let refused = refuse_migrated(&db_path).expect_err("an unfinished run keeps the fence up");
        assert!(refused.contains("never finished"), "{refused}");
        assert!(refused.contains("storage migrate abort"), "{refused}");
    }

    #[tokio::test]
    async fn an_abort_pointed_at_another_database_is_refused() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration abort binding check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("migrate the SQLite deployment");

        let elsewhere = target.clone().with_database("platpulse_never_migrated");
        let error = abort_deployment(&source, &elsewhere)
            .await
            .expect_err("abort must name the database the marker records");
        assert!(error.to_string().contains("records the target"), "{error}");
        assert!(
            marker_path(source.path()).exists(),
            "a refused abort keeps the marker"
        );

        // The refusal left the database the marker does name untouched.
        let migrated = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the migrated target");
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
            .fetch_one(migrated.pool())
            .await
            .expect("count the migrated Audit Events");
        migrated.close().await;
        assert_eq!(rows, 2, "the copied rows survive the refused abort");
    }

    /// Losing the target's ownership session mid-write must fail the command
    /// instead of letting the write commit into a database another Server may
    /// by then be serving: the pool's ownership guard only refuses *new* work
    /// connections, so the write already running has to be dropped.
    #[tokio::test]
    async fn a_target_write_that_loses_ownership_is_dropped_instead_of_committed() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration ownership race check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let database = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the PostgreSQL target");

        // The write stands in for the copy: it opens a transaction and writes
        // into it, then waits for a signal this test never sends, so only the
        // ownership race can end it.
        let (started, mut started_rx) = tokio::sync::oneshot::channel();
        let (_release, release_rx) = tokio::sync::oneshot::channel::<()>();
        let work = async {
            let mut transaction = database.pool().begin().await?;
            sqlx::query("CREATE TABLE ownership_probe (height BIGINT)")
                .execute(&mut *transaction)
                .await?;
            sqlx::query("INSERT INTO ownership_probe (height) VALUES (1)")
                .execute(&mut *transaction)
                .await?;
            let _ = started.send(());
            let _ = release_rx.await;
            transaction.commit().await?;
            Ok::<(), StorageMigrationError>(())
        };
        let mut raced = Box::pin(while_owned(
            &database,
            || StorageMigrationError::Incomplete("ownership was lost".to_owned()),
            work,
        ));
        tokio::select! {
            _ = &mut started_rx => {}
            result = &mut raced => {
                panic!("the target write finished before ownership was severed: {result:?}")
            }
        }

        crate::test_support::terminate_postgres_sessions(&target).await;
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), &mut raced)
            .await
            .expect(
                "losing the ownership session must end the write instead of leaving it running",
            );
        assert!(
            matches!(outcome, Err(StorageMigrationError::Incomplete(_))),
            "the loss of the ownership session must be reported, got {outcome:?}"
        );
        database.close().await;

        let reopened = ServerDatabase::open_for_maintenance(target)
            .await
            .expect("reopen the target once the severed session released it");
        let tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_tables WHERE tablename = 'ownership_probe'",
        )
        .fetch_one(reopened.pool())
        .await
        .expect("count the probe table");
        reopened.close().await;
        assert_eq!(
            tables, 0,
            "a write whose ownership was lost must not leave its transaction behind"
        );
    }

    #[tokio::test]
    async fn a_write_that_keeps_the_row_count_still_refuses_the_abort() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration digest check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("migrate the SQLite deployment");

        // Same rows, same count, different bytes: only the content digest can
        // tell this apart from the migrated state.
        let migrated = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the migrated target");
        sqlx::query("UPDATE networks SET chain_id = 2206999 WHERE network_key = 'lat1'")
            .execute(migrated.pool())
            .await
            .expect("edit an Admin-owned row");
        migrated.close().await;

        let error = abort_deployment(&source, &target)
            .await
            .expect_err("an edit after the baseline refuses the abort");
        assert!(
            matches!(error, StorageMigrationError::ForwardRepairRequired(_)),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("no longer holds the state recorded"),
            "{error}"
        );
        assert!(
            marker_path(source.path()).exists(),
            "a refused abort keeps the marker"
        );
    }

    #[tokio::test]
    async fn an_interrupted_run_is_aborted_back_to_an_empty_target() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the interrupted Storage Backend Migration abort check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        let summary = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("migrate the SQLite deployment");

        // Return the target to what a run that died before its commit leaves
        // behind: no baseline, no copied rows, only the reference rows its own
        // migrations inserted. The fence stays up, still saying it is running.
        let holder = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the target");
        let mut transaction = holder.pool().begin().await.expect("begin the cleanup");
        crate::database::defer_foreign_keys(&mut transaction)
            .await
            .expect("defer the foreign keys");
        let tables = read_target_schema(&mut transaction)
            .await
            .expect("read the target schema");
        delete_copied_rows(&mut transaction, &tables)
            .await
            .expect("hand the target back the way the rollback left it");
        sqlx::query("DELETE FROM storage_migration")
            .execute(&mut *transaction)
            .await
            .expect("drop the baseline the interrupted run did not write");
        transaction.commit().await.expect("commit the cleanup");
        holder.close().await;
        let mut marker = read_marker(&marker_path(source.path())).expect("read the marker");
        marker.state = MarkerState::InProgress;
        marker.completed_at = None;
        std::fs::write(
            marker_path(source.path()),
            serde_json::to_vec_pretty(&marker).expect("serialize the claim"),
        )
        .expect("rewrite the marker into an unfinished run");
        let started_at = marker.started_at.clone();

        let abort = abort_deployment(&source, &target)
            .await
            .expect("abort an interrupted run");
        assert_eq!(abort.completed_at, started_at);
        assert_eq!(abort.table_count, summary.table_count);
        assert_eq!(abort.row_count, 0, "the rollback left nothing to remove");
        assert!(
            !marker_path(source.path()).exists(),
            "the abort releases the fence"
        );

        let emptied = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the emptied target");
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
            .fetch_one(emptied.pool())
            .await
            .expect("count the remaining Audit Events");
        emptied.close().await;
        assert_eq!(rows, 0, "the abort hands back an empty target");

        // ... and the deployment migrates again from the same file.
        let retried = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("retry the migration after an interrupted run");
        assert_eq!(retried.row_count, summary.row_count);
    }

    #[tokio::test]
    async fn a_target_that_already_carries_a_baseline_is_refused() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration baseline check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("migrate the SQLite deployment");
        // Get past the marker gate so the target itself is what refuses.
        std::fs::remove_file(marker_path(source.path())).expect("drop the marker");

        let error = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect_err("a target that served a migration is refused");
        assert!(
            error
                .to_string()
                .contains("already carries a Storage Backend Migration baseline"),
            "{error}"
        );
        assert!(
            !marker_path(source.path()).exists(),
            "the refused run releases the fence it claimed"
        );
    }

    #[tokio::test]
    async fn a_write_that_consumed_an_identity_and_rolled_back_still_refuses_the_abort() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration sequence check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("migrate the SQLite deployment");

        // A report that reached the target and rolled back leaves no row and
        // no content-digest change, but PostgreSQL moved the identity anyway:
        // sequences are not transactional. Without that evidence the abort
        // would hand the SQLite file back over a Server that already took
        // traffic.
        let migrated = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the migrated target");
        let mut transaction = migrated.pool().begin().await.expect("begin a write");
        sqlx::query(
            "INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, before_json, after_json, created_at) VALUES (NULL, 'network.update', 'network', 'lat1', NULL, NULL, '2026-10-20T00:02:00Z')",
        )
        .execute(&mut *transaction)
        .await
        .expect("stage a rolled-back write");
        transaction.rollback().await.expect("roll the write back");
        migrated.close().await;

        let error = abort_deployment(&source, &target)
            .await
            .expect_err("a moved identity refuses the abort");
        assert!(
            matches!(error, StorageMigrationError::ForwardRepairRequired(_)),
            "{error}"
        );
        assert!(error.to_string().contains("sequence digest"), "{error}");
        assert!(
            marker_path(source.path()).exists(),
            "a refused abort keeps the marker"
        );
    }

    #[tokio::test]
    async fn an_abort_that_would_delete_rows_it_did_not_copy_is_refused() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration interrupted-abort check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        let marker = marker_path(source.path());
        let mut claim = marker_for(source.path());
        claim.state = MarkerState::InProgress;
        claim.completed_at = None;
        claim.target_description = describe_target(&target);
        std::fs::write(
            &marker,
            serde_json::to_vec_pretty(&claim).expect("serialize the claim"),
        )
        .expect("claim a source whose run never committed");

        // The target is empty apart from the reference rows its own migrations
        // insert, but somebody served from it after the interrupted run: those
        // rows are not this migration's to delete.
        let target_db = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the target");
        sqlx::query(
            "INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, before_json, after_json, created_at) VALUES (NULL, 'network.update', 'network', 'lat1', NULL, NULL, '2026-10-20T00:02:00Z')",
        )
        .execute(target_db.pool())
        .await
        .expect("serve one write from the target");
        target_db.close().await;

        let error = abort_deployment(&source, &target)
            .await
            .expect_err("an abort over somebody else's rows is refused");
        assert!(
            matches!(error, StorageMigrationError::ForwardRepairRequired(_)),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("although no migration into it ever committed"),
            "{error}"
        );
        assert!(marker.exists(), "a refused abort keeps the marker");
        let still_there = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("reopen the target");
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
            .fetch_one(still_there.pool())
            .await
            .expect("count the Audit Events");
        still_there.close().await;
        assert_eq!(rows, 1, "the refused abort deleted nothing");
    }

    /// The boundary `storage migrate abort` deliberately stops at.
    ///
    /// The command compares two witnesses against the baseline: the content
    /// digest of every table and the identity sequences. A write that leaves
    /// both exactly as the baseline recorded them - a no-op UPDATE, or one a
    /// later statement undid - is invisible to them, and the abort proceeds.
    /// That is the intended trade-off rather than an oversight: such a write
    /// persisted nothing, so reinstating the SQLite file loses nothing durable,
    /// while the alternatives are worse. Per-table DML counters live in
    /// `pg_stat*`, which flushes on its own schedule and resets with the
    /// cluster, so they would refuse aborts of untouched targets; a real write
    /// log would cost a trigger on every table and a schema change no report
    /// needs.
    #[tokio::test]
    async fn a_write_that_leaves_the_target_unchanged_still_allows_the_abort() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration no-op write check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("migrate the SQLite deployment");

        let migrated = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the migrated target");
        sqlx::query("UPDATE networks SET display_name = display_name WHERE network_key = 'lat1'")
            .execute(migrated.pool())
            .await
            .expect("write a row back to the value it already held");
        migrated.close().await;

        let summary = abort_deployment(&source, &target)
            .await
            .expect("a write that stored nothing new does not block the abort");
        assert!(summary.row_count > 0, "the migrated rows were withdrawn");
        assert!(
            !marker_path(source.path()).exists(),
            "a successful abort removes the marker"
        );
    }

    #[tokio::test]
    async fn a_target_that_alters_a_copied_row_fails_the_verification() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration verification check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let holder = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the target");
        // The copy inserts what it read and reads it back; this trigger makes
        // the target silently store something else, so the mismatch appears in
        // the verification rather than in an aborted statement.
        sqlx::query(
            "CREATE FUNCTION storage_migration_corrupt_networks() RETURNS trigger AS $$ BEGIN NEW.chain_id := NEW.chain_id + 1; RETURN NEW; END; $$ LANGUAGE plpgsql",
        )
        .execute(holder.pool())
        .await
        .expect("create the corrupting function");
        sqlx::query(
            "CREATE TRIGGER storage_migration_corrupt_networks BEFORE INSERT ON networks FOR EACH ROW EXECUTE FUNCTION storage_migration_corrupt_networks()",
        )
        .execute(holder.pool())
        .await
        .expect("install the corrupting trigger");
        empty_target(&holder).await;
        holder.close().await;

        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        let error = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect_err("a copy the target altered must fail the verification");
        assert!(
            matches!(error, StorageMigrationError::Verification(_)),
            "{error}"
        );
        assert!(
            error.to_string().contains("did not copy exactly"),
            "{error}"
        );
        assert!(
            !marker_path(source.path()).exists(),
            "a rollback writes no marker"
        );

        let rolled_back = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the target after the rollback");
        let leftover: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM networks), (SELECT COUNT(*) FROM storage_migration)",
        )
        .fetch_one(rolled_back.pool())
        .await
        .expect("count what the rolled-back copy left");
        assert_eq!(leftover, (0, 0), "a failed verification leaves nothing");
        sqlx::query("DROP TRIGGER storage_migration_corrupt_networks ON networks")
            .execute(rolled_back.pool())
            .await
            .expect("drop the trigger");
        sqlx::query("DROP FUNCTION storage_migration_corrupt_networks()")
            .execute(rolled_back.pool())
            .await
            .expect("drop the function");
        rolled_back.close().await;

        let retried = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("the retry succeeds once the target stops altering rows");
        assert!(retried.row_count >= 6);
    }

    #[tokio::test]
    async fn a_late_failure_takes_the_restarted_sequences_with_it() {
        let Some(target) = crate::test_support::optional_postgres_connect_config().await else {
            eprintln!(
                "skipping the Storage Backend Migration late-failure check: {} is unset, so there is no PostgreSQL target; the PostgreSQL CI jobs set it",
                crate::test_support::TEST_POSTGRES_URL_ENV
            );
            return;
        };
        let injection = "injected Storage Backend Migration baseline failure";
        let holder = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the target");
        crate::test_support::install_failure_trigger(
            holder.pool(),
            "storage_migration_refuse_baseline",
            "BEFORE",
            "INSERT",
            "storage_migration",
            injection,
        )
        .await
        .expect("install the failure trigger");
        let before = audit_event_identity(holder.pool()).await;
        empty_target(&holder).await;
        holder.close().await;

        let dir = TempDir::new().expect("a temporary directory");
        let (source, _) = seeded_source(dir.path()).await;
        let error = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect_err("a failure after the sequences moved fails the copy");
        assert!(error.to_string().contains(injection), "{error}");

        // The failure fires after the copy advanced the identity sequences and
        // before the commit, so it proves the restart is part of the same
        // transaction: nothing of it survives.
        let rolled_back = ServerDatabase::open_for_maintenance(target.clone())
            .await
            .expect("open the target after the failure");
        assert_eq!(
            audit_event_identity(rolled_back.pool()).await,
            before,
            "a failed copy must not leave a restarted sequence behind"
        );
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
            .fetch_one(rolled_back.pool())
            .await
            .expect("count the rolled-back rows");
        assert_eq!(rows, 0, "a failed copy leaves the target empty");
        crate::test_support::drop_failure_trigger(
            rolled_back.pool(),
            "storage_migration_refuse_baseline",
            "storage_migration",
        )
        .await
        .expect("remove the failure trigger");
        rolled_back.close().await;

        let retried = migrate_deployment(&source, &target, &mut |_| {})
            .await
            .expect("the retry succeeds once the baseline can be written");
        assert!(retried.row_count >= 6);
    }

    /// The Audit Event identity sequence as `(last_value, is_called)`.
    async fn audit_event_identity(pool: &sqlx::AnyPool) -> (i64, i64) {
        sqlx::query("SELECT pg_get_serial_sequence('audit_events', 'audit_event_id')")
            .fetch_one(pool)
            .await
            .map(|_| ())
            .expect("read the Audit Event sequence name");
        let sequence: String =
            sqlx::query_scalar("SELECT pg_get_serial_sequence('audit_events', 'audit_event_id')")
                .fetch_one(pool)
                .await
                .expect("name the Audit Event sequence");
        sqlx::query_as(&format!(
            "SELECT last_value, is_called::int FROM {sequence}"
        ))
        .fetch_one(pool)
        .await
        .expect("read the Audit Event sequence")
    }
}
