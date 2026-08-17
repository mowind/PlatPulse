//! Restore (issue #51, design §20.2, webui.md §8.4).
//!
//! Restore is the highest-risk recovery workflow and is strictly offline:
//! the WebUI workflow selects a backup identity, re-verifies its checksum,
//! read-only integrity, and schema compatibility, then requires a typed
//! confirmation. Because an exclusive stopped-Server condition is
//! required, the serving Server can never apply a restore itself — the
//! `restore` Operation validates and then refuses with the typed
//! `restore_requires_stopped_server` failure while the current database
//! remains authoritative (`SCN-DATA-RESTORE-SERVER-RUNNING`).
//!
//! The actual application happens through the offline
//! `platpulse-server restore` CLI, which holds the same invariants:
//! checksum + `integrity_check` + schema compatibility (higher unsupported
//! schemas are refused; equal or older schemas are accepted and upgraded
//! by normal forward migration), a safety copy of the current database, an
//! atomic replacement, and never any touch of secret files (pepper,
//! credentials, provider tokens). A failed validation or execution
//! preserves the current database and records a recoverable typed
//! Operation failure. After a successful apply the outcome is recorded
//! into the restored database (as a `restore` Operation plus Audit Event),
//! so the WebUI can present the new state after the Server restarts.

#[cfg(test)]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

use nix::fcntl::{Flock, FlockArg};
use serde_json::Value;
use sha2::Digest;
use sqlx::SqlitePool;
use thiserror::Error;

use crate::config::ServerConfig;
use crate::http::AppState;

pub const ERROR_SERVER_RUNNING: &str = "restore_requires_stopped_server";
pub const ERROR_ARTIFACT_NOT_FOUND: &str = "restore_artifact_not_found";
pub const ERROR_CONFIRMATION: &str = "restore_confirmation_mismatch";
pub const ERROR_BACKUP_DIR: &str = "backup_dir_not_configured";
pub const ERROR_CHECKSUM: &str = "restore_checksum_mismatch";
pub const ERROR_INTEGRITY: &str = "restore_integrity_failed";
pub const ERROR_SCHEMA: &str = "restore_schema_incompatible";
pub const ERROR_PRIVACY: &str = "restore_privacy_failed";
pub const ERROR_IO: &str = "restore_io_failed";

/// Human-readable guidance attached to the stopped-Server refusal.
pub const SERVER_RUNNING_MESSAGE: &str = "Restore requires an exclusive stopped Server. The current database was not modified. Stop the Server and run `platpulse-server restore --artifact-id <id> --yes` to apply this backup.";

#[derive(Debug, Error)]
pub enum RestoreError {
    #[error("restore database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("restore database open error: {0}")]
    Database(#[from] crate::database::ServerDatabaseError),
    #[error("restore JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("restore IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Operation(#[from] crate::operations::OperationError),
    #[error("restore domain error: {code}: {message}")]
    Domain { code: &'static str, message: String },
}

impl RestoreError {
    fn domain(code: &'static str, message: impl Into<String>) -> Self {
        Self::Domain {
            code,
            message: message.into(),
        }
    }
}

/// Sanitized metadata of one restorable artifact, read from the
/// `backup_artifacts` registry row (identity selection).
pub struct ArtifactIdentity {
    pub artifact_id: String,
    pub filename: String,
    pub bytes: i64,
    pub sha256: String,
    pub schema_version: i64,
    pub server_version: String,
    pub created_at: String,
}

/// Outcome of one offline restore application.
#[derive(Debug)]
pub struct ApplyOutcome {
    pub status: &'static str,
    pub artifact_id: String,
    pub filename: String,
    pub schema_version: i64,
    pub safety_copy: Option<PathBuf>,
    pub warnings: Vec<String>,
    /// `true` when the current database already matched the artifact
    /// (idempotent recovery: nothing was changed).
    pub already_matching: bool,
    /// `true` when the success Operation row plus Audit Event could be
    /// recorded into the restored database.
    pub outcome_recorded: bool,
}

/// Load the artifact identity row; `None` is an invalid identity.
pub async fn load_identity(
    pool: &SqlitePool,
    artifact_id: &str,
) -> Result<Option<ArtifactIdentity>, RestoreError> {
    let row = sqlx::query_as::<_, (String, i64, String, i64, String, String)>(
        "SELECT filename, bytes, sha256, schema_version, server_version, created_at FROM backup_artifacts WHERE artifact_id = ?",
    )
    .bind(artifact_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(filename, bytes, sha256, schema_version, server_version, created_at)| ArtifactIdentity {
            artifact_id: artifact_id.to_owned(),
            filename,
            bytes,
            sha256,
            schema_version,
            server_version,
            created_at,
        },
    ))
}

/// The typed confirmation phrase for one artifact is its file base name
/// (identity-bound, never a generic word): typing the filename confirms
/// the exact backup that will replace the database.
pub fn confirmation_matches(confirmation: &str, filename: &str) -> bool {
    confirmation.trim().eq_ignore_ascii_case(filename.trim())
}

/// Read-only validation of the artifact file against its recorded
/// manifest: fresh SHA-256, full `PRAGMA integrity_check`, and schema
/// compatibility (higher unsupported schemas are refused; equal or older
/// schemas restore cleanly and are upgraded by normal forward migration).
/// The first failing check is reported with its typed code.
pub async fn check_artifact(
    path: &Path,
    expected_sha256: &str,
    expected_schema: i64,
    current_schema: i64,
) -> Result<(), RestoreError> {
    crate::file_security::validate_file(path)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    let mut file = crate::file_security::open_readonly(path)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            RestoreError::domain(
                ERROR_IO,
                format!("cannot read the backup artifact: {error}"),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = crate::secrets::encode_hex(&hasher.finalize());
    if actual != expected_sha256 {
        return Err(RestoreError::domain(
            ERROR_CHECKSUM,
            "the backup artifact checksum does not match its recorded manifest",
        ));
    }

    crate::file_security::validate_file(path)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true);
    let pool = SqlitePool::connect_with(options).await.map_err(|error| {
        RestoreError::domain(
            ERROR_INTEGRITY,
            format!("cannot open the backup artifact read-only: {error}"),
        )
    })?;
    // Restore is the highest-risk operation: the design (§20.2) demands a
    // full integrity_check, not just the lighter quick_check used by
    // backup verification.
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .map_err(|error| {
            RestoreError::domain(
                ERROR_INTEGRITY,
                format!("cannot run the artifact integrity check: {error}"),
            )
        })?;
    if integrity != "ok" {
        pool.close().await;
        return Err(RestoreError::domain(
            ERROR_INTEGRITY,
            format!("snapshot integrity check failed: {integrity}"),
        ));
    }
    let schema: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .map_err(|error| {
            RestoreError::domain(
                ERROR_SCHEMA,
                format!("cannot read the artifact schema version: {error}"),
            )
        })?;
    pool.close().await;
    // The artifact's real schema must match the recorded manifest before
    // any compatibility reasoning (a mismatched manifest is a tampered or
    // inconsistent artifact).
    if schema != expected_schema {
        return Err(RestoreError::domain(
            ERROR_SCHEMA,
            format!(
                "the backup's schema version {schema} does not match the recorded version {expected_schema}"
            ),
        ));
    }
    // Higher unsupported schemas are refused; equal or older schemas are
    // accepted and upgraded by normal forward migration at startup.
    if schema > current_schema {
        return Err(RestoreError::domain(
            ERROR_SCHEMA,
            format!(
                "the backup was created with schema {schema}, which is newer than this Server supports ({current_schema}); refusing to restore an unsupported schema"
            ),
        ));
    }
    crate::backup::validate_snapshot_privacy(path)
        .await
        .map_err(|error| RestoreError::domain(ERROR_PRIVACY, error.to_string()))?;
    Ok(())
}

pub(crate) fn validated_artifact_path(dir: &Path, filename: &str) -> Result<PathBuf, RestoreError> {
    if !crate::file_security::is_safe_basename(filename) {
        return Err(RestoreError::domain(
            ERROR_IO,
            "backup manifest contains an unsafe artifact filename",
        ));
    }
    crate::file_security::validate_private_directory(dir)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    Ok(dir.join(filename))
}

/// Lock file living next to the database (`<db_path>.lock`). `serve` holds
/// it for the process lifetime; the offline restore refuses to start while
/// it is held (design §19: exclusive commands detect a running Server).
fn lock_path_for(db_path: &Path) -> PathBuf {
    let mut name = db_path.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// An exclusive, non-blocking lock on the database. Holds the file
/// description for the caller's lifetime.
#[derive(Debug)]
pub struct ExclusiveGuard {
    _lock: Flock<std::fs::File>,
}

/// Acquire the exclusive lock; `Err(ERROR_SERVER_RUNNING)` when a Server
/// (or another restore) is running.
pub fn acquire_exclusive_lock(db_path: &Path) -> Result<ExclusiveGuard, RestoreError> {
    let parent = db_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    crate::file_security::validate_private_directory(parent)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    let lock_path = lock_path_for(db_path);
    crate::file_security::validate_no_symlinked_ancestors(&lock_path)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    if std::fs::symlink_metadata(&lock_path).is_ok() {
        crate::file_security::validate_file(&lock_path)
            .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    }
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
            .mode(0o600)
            .open(&lock_path)?
    };
    #[cfg(not(unix))]
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    crate::file_security::validate_file(&lock_path)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    let lock = Flock::lock(file, FlockArg::LockExclusiveNonblock).map_err(|(_, _)| {
        RestoreError::domain(
            ERROR_SERVER_RUNNING,
            "a running Server holds the database; restore requires an exclusive stopped-Server condition",
        )
    })?;
    Ok(ExclusiveGuard { _lock: lock })
}

// ---------------------------------------------------------------------------
// The `restore` Operation step (serving Server)
// ---------------------------------------------------------------------------

/// Worker step for a queued `restore` Operation: identity selection,
/// checksum/integrity/schema validation, typed confirmation revalidation,
/// and then the exclusive stopped-Server gate. The serving Server can
/// never satisfy that gate, so the Operation fails with the typed
/// `restore_requires_stopped_server` error BEFORE any mutation — the
/// current database stays authoritative (`SCN-DATA-RESTORE-SERVER-RUNNING`).
pub async fn execute(state: &AppState, operation_id: &str) -> Result<(), RestoreError> {
    let pool = state.db().pool();
    let params = crate::operations::operation_params(pool, operation_id).await?;
    let artifact_id = params
        .get("artifactId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let confirmation = params
        .get("confirmation")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if artifact_id.is_empty() || confirmation.is_empty() {
        fail_operation(
            state,
            operation_id,
            ERROR_CONFIRMATION,
            "the Restore Operation is missing its artifact reference or typed confirmation",
        )
        .await?;
        return Ok(());
    }

    let Some(identity) = load_identity(pool, &artifact_id).await? else {
        fail_operation(
            state,
            operation_id,
            ERROR_ARTIFACT_NOT_FOUND,
            "unknown backup artifact; select an artifact from the backup list",
        )
        .await?;
        return Ok(());
    };
    if !confirmation_matches(&confirmation, &identity.filename) {
        fail_operation(
            state,
            operation_id,
            ERROR_CONFIRMATION,
            "the typed confirmation does not match the selected backup filename",
        )
        .await?;
        return Ok(());
    }

    if crate::operations::is_cancel_requested(state, operation_id).await? {
        crate::operations::finalize(
            state,
            operation_id,
            crate::operations::STATUS_CANCELLED,
            None,
            &["operations", "backups"],
        )
        .await?;
        return Ok(());
    }

    // Read-only validation of the artifact file (checksum, integrity,
    // schema compatibility). Any failure is a recoverable typed Operation
    // failure and preserves the current database.
    let Some(backup_dir) = state.backup_dir().map(|path| path.to_path_buf()) else {
        fail_operation(
            state,
            operation_id,
            ERROR_BACKUP_DIR,
            "no backup directory is configured; set backup_dir in server.toml",
        )
        .await?;
        return Ok(());
    };
    let artifact_path = match validated_artifact_path(&backup_dir, &identity.filename) {
        Ok(path) => path,
        Err(error) => {
            let (code, message) = match error {
                RestoreError::Domain { code, message } => (code, message),
                other => (ERROR_IO, other.to_string()),
            };
            fail_operation(state, operation_id, code, &message).await?;
            return Ok(());
        }
    };
    if let Err(error) = check_artifact(
        &artifact_path,
        &identity.sha256,
        identity.schema_version,
        crate::database::SERVER_SCHEMA_VERSION,
    )
    .await
    {
        let (code, message) = match error {
            RestoreError::Domain { code, message } => (code, message),
            other => (ERROR_IO, other.to_string()),
        };
        fail_operation(state, operation_id, code, &message).await?;
        return Ok(());
    }

    if crate::operations::is_cancel_requested(state, operation_id).await? {
        crate::operations::finalize(
            state,
            operation_id,
            crate::operations::STATUS_CANCELLED,
            None,
            &["operations", "backups"],
        )
        .await?;
        return Ok(());
    }

    // The exclusive stopped-Server condition: a serving Server is running,
    // so the gate can never open here. Refuse before any mutation; the
    // current database remains authoritative.
    crate::operations::add_error(
        state,
        operation_id,
        ERROR_SERVER_RUNNING,
        SERVER_RUNNING_MESSAGE,
    )
    .await?;
    let result = serde_json::json!({
        "validation": {
            "checksum": "ok",
            "integrity": "ok",
            "schemaCompatible": true,
            "schemaVersion": identity.schema_version,
            "currentSchemaVersion": crate::database::SERVER_SCHEMA_VERSION,
        },
        "refusal": ERROR_SERVER_RUNNING,
        "artifactId": artifact_id,
    });
    crate::operations::finalize(
        state,
        operation_id,
        crate::operations::STATUS_FAILED,
        Some(&result),
        &["operations", "backups"],
    )
    .await?;
    Ok(())
}

async fn fail_operation(
    state: &AppState,
    operation_id: &str,
    code: &str,
    message: &str,
) -> Result<(), RestoreError> {
    crate::operations::add_error(state, operation_id, code, message).await?;
    crate::operations::finalize(
        state,
        operation_id,
        crate::operations::STATUS_FAILED,
        None,
        &["operations", "backups"],
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Offline application (`platpulse-server restore`)
// ---------------------------------------------------------------------------

/// How the destructive apply was confirmed (design §19: destructive
/// commands require the typed confirmation phrase; automation uses an
/// explicit `--yes`).
pub enum Confirmation {
    /// Automation marker: the operator passed `--yes`.
    Explicit,
    /// A typed phrase that must equal the artifact file base name.
    Phrase(String),
}

/// Apply one validated backup offline: exclusive lock, identity +
/// checksum + integrity + schema validation, idempotency check, safety
/// copy of the current database, atomic replacement, and outcome recording
/// into the restored database. Secret files are never touched. Every
/// failure before the atomic rename preserves the current database and is
/// recorded as a recoverable typed Operation failure.
pub async fn apply(
    config: &ServerConfig,
    artifact_id: &str,
    confirmation: Confirmation,
) -> Result<ApplyOutcome, RestoreError> {
    // Exclusive stopped-Server gate (design §19): refuse while a Server is
    // running.
    let _guard = acquire_exclusive_lock(&config.db_path)?;

    let current = crate::database::ServerDatabase::open_existing(
        crate::database::ServerDatabaseConfig::new(&config.db_path),
    )
    .await?;

    // Idempotent recovery: a database that already carries a successful
    // restore of this exact artifact is a completed restore (or a redundant
    // re-run) — never re-apply. The outcome row survives in the restored
    // database itself (the artifact snapshot predates its own manifest row,
    // so the identity may be gone after a restore). A crash after the
    // atomic rename but before the record is recovered by re-running: the
    // rename is atomic and the end state is the same artifact.
    let already_matching: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM operations WHERE kind = 'restore' AND status IN (?, ?) AND json_extract(params_json, '$.artifactId') = ?",
    )
    .bind(crate::operations::STATUS_SUCCEEDED)
    .bind(crate::operations::STATUS_SUCCEEDED_WITH_WARNINGS)
    .bind(artifact_id)
    .fetch_one(current.pool())
    .await?;
    if already_matching > 0 {
        let filename: String = sqlx::query_scalar(
            "SELECT json_extract(result_json, '$.filename') FROM operations WHERE kind = 'restore' AND status IN (?, ?) AND json_extract(params_json, '$.artifactId') = ? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(crate::operations::STATUS_SUCCEEDED)
        .bind(crate::operations::STATUS_SUCCEEDED_WITH_WARNINGS)
        .bind(artifact_id)
        .fetch_optional(current.pool())
        .await?
        .flatten()
        .unwrap_or_else(|| artifact_id.to_owned());
        current.close().await;
        return Ok(ApplyOutcome {
            status: crate::operations::STATUS_SUCCEEDED,
            artifact_id: artifact_id.to_owned(),
            filename,
            schema_version: 0,
            safety_copy: None,
            warnings: vec![format!(
                "a successful Restore of backup '{artifact_id}' is already recorded; nothing was restored"
            )],
            already_matching: true,
            outcome_recorded: true,
        });
    }

    let Some(identity) = load_identity(current.pool(), artifact_id).await? else {
        current.close().await;
        return Err(RestoreError::domain(
            ERROR_ARTIFACT_NOT_FOUND,
            "unknown backup artifact; list backups with the Admin surface and pass the artifact id",
        ));
    };
    match &confirmation {
        Confirmation::Explicit => {}
        Confirmation::Phrase(phrase) => {
            if !confirmation_matches(phrase, &identity.filename) {
                current.close().await;
                return Err(RestoreError::domain(
                    ERROR_CONFIRMATION,
                    "the typed confirmation does not match the backup file name",
                ));
            }
        }
    }
    let Some(backup_dir) = &config.backup_dir else {
        current.close().await;
        return Err(RestoreError::domain(
            ERROR_BACKUP_DIR,
            "no backup directory is configured; set backup_dir in server.toml",
        ));
    };
    let artifact_path = match validated_artifact_path(backup_dir, &identity.filename) {
        Ok(path) => path,
        Err(error) => {
            let (code, message) = match &error {
                RestoreError::Domain { code, message } => (*code, message.clone()),
                other => (ERROR_IO, other.to_string()),
            };
            record_failure(
                current.pool(),
                artifact_id,
                &identity.filename,
                code,
                &message,
            )
            .await
            .ok();
            current.close().await;
            return Err(error);
        }
    };
    if let Err(error) = check_artifact(
        &artifact_path,
        &identity.sha256,
        identity.schema_version,
        crate::database::SERVER_SCHEMA_VERSION,
    )
    .await
    {
        // Validation failure: the current database is preserved. Record the
        // recoverable typed failure in the current database.
        let (code, message) = match &error {
            RestoreError::Domain { code, message } => (*code, message.clone()),
            other => (ERROR_IO, other.to_string()),
        };
        record_failure(
            current.pool(),
            artifact_id,
            &identity.filename,
            code,
            &message,
        )
        .await
        .ok();
        current.close().await;
        return Err(error);
    }

    // Safety copy of the current database (consistent snapshot through
    // VACUUM INTO; never a copy of live -wal/-shm). Every execution
    // failure below preserves the current database and records a
    // recoverable typed Operation failure.
    let safety_copy = safety_copy_path(&config.db_path);
    let temp_safety = temp_path_for(&safety_copy);
    let _ = std::fs::remove_file(&temp_safety);
    if let Err(error) = vacuum_into(current.pool(), &temp_safety, true).await {
        let (code, message) = failure_parts(&error);
        record_failure(
            current.pool(),
            artifact_id,
            &identity.filename,
            code,
            &message,
        )
        .await
        .ok();
        current.close().await;
        let _ = std::fs::remove_file(&temp_safety);
        return Err(error);
    }
    if let Err(error) = sync_file(&temp_safety) {
        record_failure(
            current.pool(),
            artifact_id,
            &identity.filename,
            ERROR_IO,
            &error.to_string(),
        )
        .await
        .ok();
        current.close().await;
        let _ = std::fs::remove_file(&temp_safety);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temp_safety, &safety_copy) {
        record_failure(
            current.pool(),
            artifact_id,
            &identity.filename,
            ERROR_IO,
            &error.to_string(),
        )
        .await
        .ok();
        current.close().await;
        let _ = std::fs::remove_file(&temp_safety);
        return Err(error.into());
    }
    let safety_copy = Some(safety_copy);

    // Atomic replacement: copy the artifact next to the database (same
    // filesystem), fsync, then rename over the database file.
    let db_dir = config
        .db_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temp_db = db_dir.join(format!(
        ".{}.restore-tmp",
        config
            .db_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "platpulse.db".to_owned())
    ));
    let _ = std::fs::remove_file(&temp_db);
    if let Err(error) = copy_fsync(&artifact_path, &temp_db).await {
        record_failure(
            current.pool(),
            artifact_id,
            &identity.filename,
            ERROR_IO,
            &error.to_string(),
        )
        .await
        .ok();
        current.close().await;
        let _ = std::fs::remove_file(&temp_db);
        if let Some(path) = safety_copy.as_ref() {
            let _ = std::fs::remove_file(path);
        }
        return Err(error);
    }
    if let Err(error) = check_artifact(
        &temp_db,
        &identity.sha256,
        identity.schema_version,
        crate::database::SERVER_SCHEMA_VERSION,
    )
    .await
    {
        let (code, message) = failure_parts(&error);
        record_failure(
            current.pool(),
            artifact_id,
            &identity.filename,
            code,
            &message,
        )
        .await
        .ok();
        current.close().await;
        let _ = std::fs::remove_file(&temp_db);
        if let Some(path) = safety_copy.as_ref() {
            let _ = std::fs::remove_file(path);
        }
        return Err(error);
    }
    // The pool must be fully closed before the rename so no descriptor
    // keeps the old database inode alive.
    current.close().await;
    if let Err(error) = std::fs::rename(&temp_db, &config.db_path) {
        // The current database is still in place; reopen it to record the
        // recoverable typed failure.
        if let Ok(rolled_back) = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(&config.db_path),
        )
        .await
        {
            record_failure(
                rolled_back.pool(),
                artifact_id,
                &identity.filename,
                ERROR_IO,
                &format!("cannot replace the database file: {error}"),
            )
            .await
            .ok();
            rolled_back.close().await;
        }
        let _ = std::fs::remove_file(&temp_db);
        if let Some(path) = safety_copy.as_ref() {
            let _ = std::fs::remove_file(path);
        }
        return Err(error.into());
    }
    if let Err(error) = crate::file_security::secure_new_file(&config.db_path) {
        let rollback = safety_copy
            .as_ref()
            .ok_or_else(|| RestoreError::domain(ERROR_IO, "restore safety copy is unavailable"))
            .and_then(|copy| rollback_to_safety_copy(&config.db_path, copy));
        sanitize_or_remove(safety_copy.as_ref().expect("safety copy exists")).await;
        if let Err(rollback_error) = rollback {
            eprintln!(
                "restore rollback failed: {}",
                crate::redaction::redact_sensitive(&rollback_error.to_string())
            );
        }
        let _ = std::fs::remove_file(&temp_db);
        return Err(RestoreError::domain(ERROR_IO, error));
    }
    // The old database's WAL/SHM belong to the replaced file; stale files
    // are never replayed against the restored database.
    let _ = std::fs::remove_file(with_suffix(&config.db_path, "-wal"));
    let _ = std::fs::remove_file(with_suffix(&config.db_path, "-shm"));

    // Record the outcome into the restored database: a `restore` Operation
    // row plus its Audit Event (actor = local-cli), so the WebUI presents
    // the succeeded state after the Server restarts. If the restored
    // database cannot be opened (migration, integrity, or readiness
    // checks), the safety copy rolls the previous database back so the
    // current database stays authoritative.
    let schema_older = identity.schema_version < crate::database::SERVER_SCHEMA_VERSION;
    let mut warnings = Vec::new();
    if schema_older {
        warnings.push(format!(
            "the restored database used schema {}; forward migration to {} ran during this apply",
            identity.schema_version,
            crate::database::SERVER_SCHEMA_VERSION
        ));
    }
    let status = if schema_older {
        crate::operations::STATUS_SUCCEEDED_WITH_WARNINGS
    } else {
        crate::operations::STATUS_SUCCEEDED
    };
    let restored = match crate::database::ServerDatabase::open_existing(
        crate::database::ServerDatabaseConfig::new(&config.db_path),
    )
    .await
    {
        Ok(database) => database,
        Err(error) => {
            // The restored database failed its startup checks: roll the
            // safety copy back so the previous database stays
            // authoritative, then record the recoverable typed failure.
            let message = error.to_string();
            if let Err(rollback_error) =
                rollback_to_safety_copy(&config.db_path, safety_copy.as_ref().unwrap())
            {
                eprintln!(
                    "{}",
                    crate::redaction::redact_sensitive(&format!(
                        "restore rollback failed (manual recovery from {} needed): {}",
                        safety_copy.as_ref().unwrap().display(),
                        rollback_error
                    ))
                );
            }
            sanitize_or_remove(safety_copy.as_ref().unwrap()).await;
            let _ = record_execution_failure(
                config,
                artifact_id,
                &identity.filename,
                ERROR_INTEGRITY,
                &format!(
                    "the restored database failed its startup checks and was rolled back: {message}"
                ),
            )
            .await;
            return Err(error.into());
        }
    };
    let outcome_recorded = match record_outcome(
        restored.pool(),
        artifact_id,
        &identity.filename,
        status,
        &warnings,
        &[],
        Some(&serde_json::json!({
            "artifactId": artifact_id,
            "filename": identity.filename,
            "schemaVersion": identity.schema_version,
            "restoredAt": crate::auth::format_rfc3339(crate::auth::now_utc()),
            "safetyCopy": safety_copy.as_ref().and_then(|path| path.file_name()).map(|name| name.to_string_lossy().into_owned()),
        })),
    )
    .await
    {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(error) => {
            // The restore itself is complete; only the record failed. The
            // operator is told honestly; a later Server start shows the
            // restored data without a synthetic success record.
            eprintln!(
                "restore applied but the outcome could not be recorded: {}",
                crate::redaction::redact_sensitive(&error.to_string())
            );
            false
        }
    };
    restored.close().await;
    if let Some(path) = safety_copy.as_ref() {
        sanitize_or_remove(path).await;
    }
    Ok(ApplyOutcome {
        status,
        artifact_id: artifact_id.to_owned(),
        filename: identity.filename,
        schema_version: identity.schema_version,
        safety_copy,
        warnings,
        already_matching: false,
        outcome_recorded,
    })
}

/// Insert the restore outcome into `operations` plus its Audit Event in
/// one transaction. Actor is `local-cli` (design §19: `actor = local-cli`).
/// Returns `Ok(None)` when the target database predates the Operations
/// schema (the row cannot be recorded there).
async fn record_outcome(
    pool: &SqlitePool,
    artifact_id: &str,
    filename: &str,
    status: &str,
    warnings: &[String],
    errors: &[Value],
    result: Option<&Value>,
) -> Result<Option<(String, i64)>, RestoreError> {
    let has_operations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'operations'",
    )
    .fetch_one(pool)
    .await?;
    if has_operations == 0 {
        return Ok(None);
    }
    let operation_id = uuid::Uuid::new_v4().to_string();
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let params = crate::redaction::redact_json_value(&serde_json::json!({
        "artifactId": artifact_id,
        "source": "cli",
    }));
    let warnings = warnings
        .iter()
        .map(|warning| crate::redaction::redact_sensitive(warning))
        .collect::<Vec<_>>();
    let errors = errors
        .iter()
        .map(crate::redaction::redact_json_value)
        .collect::<Vec<_>>();
    let result = result.map(crate::redaction::redact_json_value);
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO operations (operation_id, kind, status, progress_percent, progress_label, request_id, params_json, warnings_json, errors_json, result_json, created_at, started_at, finished_at) VALUES (?, 'restore', ?, 100, 'Restored', ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&operation_id)
    .bind(status)
    .bind(format!("cli-restore-{operation_id}"))
    .bind(serde_json::to_string(&params)?)
    .bind(serde_json::to_string(&warnings)?)
    .bind(serde_json::to_string(&errors)?)
    .bind(result.as_ref().map(serde_json::to_string).transpose()?)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    crate::auth::insert_audit_event(
        &mut *tx,
        None,
        "restore_applied",
        "operation",
        &operation_id,
        Some(&serde_json::json!({
            "artifactId": artifact_id,
            "filename": filename,
            "status": status,
        })),
    )
    .await?;
    let audit_event_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE operations SET audit_event_id = ? WHERE operation_id = ?")
        .bind(audit_event_id)
        .bind(&operation_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some((operation_id, audit_event_id)))
}

/// Typed code/message pair of a RestoreError for Operation recording.
fn failure_parts(error: &RestoreError) -> (&'static str, String) {
    match error {
        RestoreError::Domain { code, message } => (*code, message.clone()),
        other => (ERROR_IO, other.to_string()),
    }
}

/// Best-effort recording of a recoverable typed Operation failure into a
/// database that is known to be healthy (validation/execution failures
/// preserve the current database and report the typed failure through
/// REST). Never fails the restore on a recording problem.
async fn record_failure(
    pool: &SqlitePool,
    artifact_id: &str,
    filename: &str,
    code: &'static str,
    message: &str,
) -> Result<(), RestoreError> {
    record_outcome(
        pool,
        artifact_id,
        filename,
        crate::operations::STATUS_FAILED,
        &[],
        &[serde_json::json!({ "code": code, "message": message })],
        None,
    )
    .await
    .map(|_| ())
}

/// Record an execution failure into the preserved current database after
/// the original pool was closed (rename or rollback paths).
async fn record_execution_failure(
    config: &ServerConfig,
    artifact_id: &str,
    filename: &str,
    code: &'static str,
    message: &str,
) -> Result<(), RestoreError> {
    let current = crate::database::ServerDatabase::open_existing(
        crate::database::ServerDatabaseConfig::new(&config.db_path),
    )
    .await?;
    let result = record_failure(current.pool(), artifact_id, filename, code, message).await;
    current.close().await;
    result
}

/// Restore the safety copy over the database path (same-filesystem copy,
/// fsync, atomic rename) and drop stale WAL/SHM files. Used when the
/// restored database fails its startup checks so the previous database
/// stays authoritative.
fn rollback_to_safety_copy(db_path: &Path, safety_copy: &Path) -> Result<(), RestoreError> {
    let temp = temp_path_for(db_path);
    let _ = std::fs::remove_file(&temp);
    let mut source = crate::file_security::open_readonly(safety_copy)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    #[cfg(unix)]
    let mut destination = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
            .mode(0o600)
            .open(&temp)?
    };
    #[cfg(not(unix))]
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    std::io::copy(&mut source, &mut destination)?;
    destination.sync_all()?;
    crate::file_security::validate_file(&temp)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    std::fs::rename(&temp, db_path)?;
    let _ = std::fs::remove_file(with_suffix(db_path, "-wal"));
    let _ = std::fs::remove_file(with_suffix(db_path, "-shm"));
    Ok(())
}

fn safety_copy_path(db_path: &Path) -> PathBuf {
    with_suffix(
        db_path,
        &format!(
            ".restore-safety-{}",
            crate::auth::format_rfc3339(crate::auth::now_utc()).replace([':', '.'], "-")
        ),
    )
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn temp_path_for(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

async fn sanitize_or_remove(path: &Path) {
    if crate::backup::sanitize_snapshot(path).await.is_err() {
        let _ = std::fs::remove_file(path);
    }
}

async fn vacuum_into(
    pool: &SqlitePool,
    destination: &Path,
    sanitize: bool,
) -> Result<(), RestoreError> {
    let absolute = destination
        .to_str()
        .ok_or_else(|| std::io::Error::other("database path is not valid UTF-8"))?;
    if absolute.contains('\'') {
        return Err(RestoreError::domain(
            ERROR_IO,
            "database path must not contain single quotes",
        ));
    }
    if std::fs::symlink_metadata(destination).is_ok()
        || crate::file_security::validate_no_symlinked_ancestors(destination).is_err()
    {
        return Err(RestoreError::domain(
            ERROR_IO,
            "snapshot destination path is unsafe or already exists",
        ));
    }
    sqlx::query(&format!("VACUUM INTO '{absolute}'"))
        .execute(pool)
        .await?;
    crate::file_security::secure_new_file(destination)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    if sanitize {
        crate::backup::sanitize_snapshot(destination)
            .await
            .map_err(|error| RestoreError::domain(ERROR_IO, error.to_string()))?;
    }
    Ok(())
}

async fn copy_fsync(source: &Path, destination: &Path) -> Result<(), RestoreError> {
    if std::fs::symlink_metadata(destination).is_ok()
        || crate::file_security::validate_no_symlinked_ancestors(destination).is_err()
    {
        return Err(RestoreError::domain(
            ERROR_IO,
            "copy destination path is unsafe or already exists",
        ));
    }
    let mut source_file = crate::file_security::open_readonly(source)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    #[cfg(unix)]
    let mut destination_file = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
            .mode(0o600)
            .open(destination)?
    };
    #[cfg(not(unix))]
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    std::io::copy(&mut source_file, &mut destination_file)?;
    destination_file.sync_all()?;
    crate::file_security::validate_file(destination)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    Ok(())
}

fn sync_file(path: &Path) -> Result<(), RestoreError> {
    let file = crate::file_security::open_readwrite(path)
        .map_err(|message| RestoreError::domain(ERROR_IO, message))?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
async fn file_sha256(path: &Path) -> Result<String, RestoreError> {
    let mut file = File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(crate::secrets::encode_hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn config(dir: &tempfile::TempDir, db: &str, backup_dir: &Path) -> ServerConfig {
        ServerConfig {
            config_path: None,
            state_dir: dir.path().to_path_buf(),
            db_path: dir.path().join(db),
            pepper_file: dir.path().join("pepper"),
            web_root: None,
            backup_dir: Some(backup_dir.to_path_buf()),
            listen: "127.0.0.1:0".parse().unwrap(),
            public_base_url: "http://127.0.0.1:8080".to_owned(),
            trusted_proxy_cidrs: Vec::new(),
            trusted_proxy_scheme: None,
            geo: None,
            validator_provider: None,
            development: false,
            notifications: crate::config::NotificationChannels::default(),
        }
    }

    /// Create a live Server database and a backup artifact of it; returns
    /// the artifact id and its recorded identity.
    async fn seed_backup(dir: &tempfile::TempDir) -> (ServerConfig, String, String) {
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();
        let config = config(dir, "server.db", &backup_dir);
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            &config.db_path,
        ))
        .await
        .unwrap();
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(database.pool()).await.unwrap();
        let artifact_id = "artifact-1".to_owned();
        let filename = format!("platpulse-{artifact_id}.db");
        let artifact_path = backup_dir.join(&filename);
        sqlx::query(&format!(
            "VACUUM INTO '{}'",
            artifact_path.to_str().unwrap()
        ))
        .execute(database.pool())
        .await
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&artifact_path, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        crate::backup::sanitize_snapshot(&artifact_path)
            .await
            .unwrap();
        let bytes = std::fs::metadata(&artifact_path).unwrap().len() as i64;
        let sha256 = file_sha256(&artifact_path).await.unwrap();
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO backup_artifacts (artifact_id, filename, bytes, sha256, schema_version, server_version, created_at, verification) VALUES (?, ?, ?, ?, ?, ?, ?, 'ok')")
            .bind(&artifact_id)
            .bind(&filename)
            .bind(bytes)
            .bind(&sha256)
            .bind(crate::database::SERVER_SCHEMA_VERSION)
            .bind(crate::VERSION)
            .bind(&now)
            .execute(database.pool())
            .await
            .unwrap();
        database.close().await;
        (config, artifact_id, sha256)
    }

    #[test]
    fn validated_artifact_path_rejects_traversal_and_absolute_names() {
        let dir = tempdir().unwrap();
        for filename in ["../server-pepper", "/etc/passwd", "nested/artifact.db"] {
            let error = validated_artifact_path(dir.path(), filename).unwrap_err();
            assert!(matches!(error, RestoreError::Domain { code: ERROR_IO, .. }));
        }
    }
    #[test]
    fn confirmation_matches_filename_case_insensitively() {
        assert!(confirmation_matches(" platpulse-a.db ", "platpulse-a.db"));
        assert!(confirmation_matches("PLATPULSE-A.DB", "platpulse-a.db"));
        assert!(!confirmation_matches("platpulse-b.db", "platpulse-a.db"));
    }

    #[tokio::test]
    async fn exclusive_lock_refuses_when_a_server_is_running() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("server.db");
        let _guard = acquire_exclusive_lock(&db_path).unwrap();
        let error = acquire_exclusive_lock(&db_path).unwrap_err();
        match error {
            RestoreError::Domain { code, .. } => assert_eq!(code, ERROR_SERVER_RUNNING),
            other => panic!("unexpected error: {other}"),
        }
        drop(_guard);
        assert!(acquire_exclusive_lock(&db_path).is_ok());
    }

    #[tokio::test]
    async fn check_artifact_rejects_checksum_integrity_and_newer_schema() {
        let dir = tempdir().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let snapshot = dir.path().join("snapshot.db");
        sqlx::query(&format!("VACUUM INTO '{}'", snapshot.to_str().unwrap()))
            .execute(database.pool())
            .await
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        crate::backup::sanitize_snapshot(&snapshot).await.unwrap();
        let sha256 = file_sha256(&snapshot).await.unwrap();
        let schema: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
                .fetch_one(database.pool())
                .await
                .unwrap();

        // Checksum mismatch.
        let error = check_artifact(&snapshot, &"00".repeat(32), schema, schema)
            .await
            .unwrap_err();
        match error {
            RestoreError::Domain { code, .. } => assert_eq!(code, ERROR_CHECKSUM),
            other => panic!("unexpected error: {other}"),
        }
        // Integrity failure.
        let tampered = dir.path().join("tampered.db");
        std::fs::write(&tampered, b"not a database").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tampered, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let error = check_artifact(&tampered, &sha256, schema, schema)
            .await
            .unwrap_err();
        match error {
            RestoreError::Domain { code, .. } => assert_eq!(code, ERROR_CHECKSUM),
            other => panic!("unexpected error: {other}"),
        }
        // Newer unsupported schema is refused: the artifact (schema N) is
        // newer than a Server that only supports N-1.
        let error = check_artifact(&snapshot, &sha256, schema, schema - 1)
            .await
            .unwrap_err();
        match error {
            RestoreError::Domain { code, .. } => assert_eq!(code, ERROR_SCHEMA),
            other => panic!("unexpected error: {other}"),
        }
        // Equal and older schemas pass.
        assert!(
            check_artifact(&snapshot, &sha256, schema, schema)
                .await
                .is_ok()
        );
        assert!(
            check_artifact(&snapshot, &sha256, schema, schema + 1)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn apply_restores_atomically_preserves_secrets_and_records_outcome() {
        let dir = tempdir().unwrap();
        let (config, artifact_id, sha256) = seed_backup(&dir).await;

        // Secret files that must never be restored or touched.
        let pepper = dir.path().join("server-pepper");
        std::fs::write(&pepper, b"pepper-material").unwrap();
        let provider = dir.path().join("provider-token");
        std::fs::write(&provider, b"provider-material").unwrap();

        // A distinguishing row in the live database that the backup lacks.
        let live = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(&config.db_path),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('live-network', 'Live', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(live.pool()).await.unwrap();
        live.close().await;

        let outcome = apply(&config, &artifact_id, Confirmation::Explicit)
            .await
            .unwrap();
        assert!(!outcome.already_matching);
        assert_eq!(outcome.status, crate::operations::STATUS_SUCCEEDED);

        // The restored database is authoritative and the live-only row is gone.
        let restored = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(&config.db_path),
        )
        .await
        .unwrap();
        let networks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM networks")
            .fetch_one(restored.pool())
            .await
            .unwrap();
        assert_eq!(networks, 0);
        // The restore outcome is recorded in the restored database.
        let (kind, status): (String, String) = sqlx::query_as(
            "SELECT kind, status FROM operations WHERE kind = 'restore' ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(restored.pool())
        .await
        .unwrap();
        assert_eq!(kind, "restore");
        assert_eq!(status, crate::operations::STATUS_SUCCEEDED);
        let audit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'restore_applied'",
        )
        .fetch_one(restored.pool())
        .await
        .unwrap();
        assert_eq!(audit, 1);
        // The restored database content is authoritative and the safety
        // copy exists; secret files are untouched.
        assert!(outcome.safety_copy.is_some());
        assert!(outcome.safety_copy.unwrap().exists());
        assert_eq!(std::fs::read(&pepper).unwrap(), b"pepper-material");
        assert_eq!(std::fs::read(&provider).unwrap(), b"provider-material");
        restored.close().await;

        // Idempotent recovery: re-applying the same artifact is a no-op and
        // does not record another restore row.
        let second = apply(&config, &artifact_id, Confirmation::Explicit)
            .await
            .unwrap();
        assert!(second.already_matching);
        let db = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(&config.db_path),
        )
        .await
        .unwrap();
        let rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM operations WHERE kind = 'restore'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(rows, 1);
        db.close().await;
        let _ = sha256;
    }

    #[tokio::test]
    async fn apply_preserves_current_database_on_validation_failure() {
        let dir = tempdir().unwrap();
        let (config, artifact_id, _sha256) = seed_backup(&dir).await;
        // A distinguishing row in the live database.
        let live = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(&config.db_path),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('live-network', 'Live', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(live.pool()).await.unwrap();
        live.close().await;
        // Tamper with the artifact on disk: validation must fail and the
        // current database must stay authoritative (content preserved).
        let artifact_path = config
            .backup_dir
            .as_ref()
            .unwrap()
            .join("platpulse-artifact-1.db");
        std::fs::write(&artifact_path, b"tampered").unwrap();
        let error = apply(&config, &artifact_id, Confirmation::Explicit)
            .await
            .unwrap_err();
        assert!(matches!(error, RestoreError::Domain { .. }));
        let current = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(&config.db_path),
        )
        .await
        .unwrap();
        let networks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM networks")
            .fetch_one(current.pool())
            .await
            .unwrap();
        assert_eq!(networks, 1);
        // The failure was recorded in the preserved database.
        let (kind, status): (String, String) = sqlx::query_as(
            "SELECT kind, status FROM operations WHERE kind = 'restore' ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(current.pool())
        .await
        .unwrap();
        assert_eq!(kind, "restore");
        assert_eq!(status, crate::operations::STATUS_FAILED);
        current.close().await;
    }

    #[tokio::test]
    async fn apply_refuses_without_confirmation_and_while_server_running() {
        let dir = tempdir().unwrap();
        let (config, artifact_id, _sha256) = seed_backup(&dir).await;
        let error = apply(
            &config,
            &artifact_id,
            Confirmation::Phrase("wrong-name.db".to_owned()),
        )
        .await
        .unwrap_err();
        match error {
            RestoreError::Domain { code, .. } => assert_eq!(code, ERROR_CONFIRMATION),
            other => panic!("unexpected error: {other}"),
        }
        // A running Server (holding the exclusive lock) refuses the apply.
        let _guard = acquire_exclusive_lock(&config.db_path).unwrap();
        let error = apply(&config, &artifact_id, Confirmation::Explicit)
            .await
            .unwrap_err();
        match error {
            RestoreError::Domain { code, .. } => assert_eq!(code, ERROR_SERVER_RUNNING),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn rollback_restores_the_previous_database_from_the_safety_copy() {
        let dir = tempdir().unwrap();
        let (config, artifact_id, _sha256) = seed_backup(&dir).await;
        // A distinguishing row exists in the live database (and therefore in
        // the safety copy) but not in the artifact.
        let live = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(&config.db_path),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('live-network', 'Live', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(live.pool()).await.unwrap();
        live.close().await;

        let outcome = apply(&config, &artifact_id, Confirmation::Explicit)
            .await
            .unwrap();
        assert!(!outcome.already_matching);
        let safety = outcome.safety_copy.unwrap();
        let restored = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(&config.db_path),
        )
        .await
        .unwrap();
        let networks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM networks")
            .fetch_one(restored.pool())
            .await
            .unwrap();
        assert_eq!(networks, 0);
        restored.close().await;

        // Simulate a broken restored database, then roll back: the previous
        // database (with the live row) becomes authoritative again.
        std::fs::write(&config.db_path, b"corrupted after restore").unwrap();
        rollback_to_safety_copy(&config.db_path, &safety).unwrap();
        let rolled_back = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(&config.db_path),
        )
        .await
        .unwrap();
        let networks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM networks")
            .fetch_one(rolled_back.pool())
            .await
            .unwrap();
        assert_eq!(networks, 1);
        rolled_back.close().await;
    }
}
