//! Backup artifact creation and verification (issue #50, ADR 0008,
//! webui.md §8.4).
//!
//! Backups are consistent SQLite snapshots produced with `VACUUM INTO`
//! (temp + fsync + atomic rename, never a copy of live `-wal`/`-shm`),
//! written into the explicitly configured backup directory with strict
//! permissions. Creation is an offline Server operation: the serving process
//! never creates backup artifacts. Only sanitized metadata is persisted and
//! exposed: file base name, size, SHA-256, schema version, Server version,
//! timestamps, and the data range. Database contents and secrets never leave
//! the Server and are never displayed. A failed create or verify preserves
//! every previous artifact and the last successful state.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::Digest;
use sqlx::Row;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use thiserror::Error;

use crate::http::AppState;

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("backup database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("backup JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("backup privacy validation failed: {0}")]
    Privacy(String),
    #[error("backup IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Operation(#[from] crate::operations::OperationError),
}

/// Identity of a freshly created offline backup artifact.
#[derive(Debug, Clone)]
pub struct CreatedArtifact {
    pub artifact_id: String,
    pub filename: String,
}

/// Create one sanitized backup artifact in an Offline Backup Window. This is
/// the only creation path: the serving process never creates backup artifacts
/// (ADR 0008). The caller must already have applied the layout guard
/// (`backup::check_layout`).
pub async fn create_offline(state: &AppState) -> Result<CreatedArtifact, BackupError> {
    let backup_dir = state
        .backup_dir()
        .cloned()
        .ok_or_else(|| BackupError::Privacy("backup_dir is not configured".to_owned()))?;
    prepare_backup_dir(&backup_dir).map_err(BackupError::Privacy)?;
    let artifact_id = uuid::Uuid::new_v4().to_string();
    let filename = format!("platpulse-{artifact_id}.db");
    let final_path = backup_dir.join(&filename);
    let temp_path = backup_dir.join(format!("{filename}.part"));
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let result = create_snapshot(
        state,
        &temp_path,
        &final_path,
        &artifact_id,
        &filename,
        &now,
    )
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result?;
    Ok(CreatedArtifact {
        artifact_id,
        filename,
    })
}

async fn create_snapshot(
    state: &AppState,
    temp_path: &Path,
    final_path: &Path,
    artifact_id: &str,
    filename: &str,
    now: &str,
) -> Result<(), BackupError> {
    let temp_absolute = temp_path
        .to_str()
        .ok_or_else(|| std::io::Error::other("backup directory path is not valid UTF-8"))?;
    if temp_absolute.contains('\'') {
        return Err(
            std::io::Error::other("backup directory path must not contain single quotes").into(),
        );
    }
    // VACUUM INTO refuses to overwrite an existing path. Refuse it before
    // SQLite sees a substituted symlink or an operator's stale partial file.
    if std::fs::symlink_metadata(temp_path).is_ok()
        || std::fs::symlink_metadata(final_path).is_ok()
        || crate::file_security::validate_no_symlinked_ancestors(temp_path).is_err()
        || crate::file_security::validate_no_symlinked_ancestors(final_path).is_err()
    {
        return Err(
            std::io::Error::other("backup artifact path is unsafe or already exists").into(),
        );
    }
    // One consistent snapshot statement on the single Server connection;
    // `VACUUM INTO` never touches live -wal/-shm and never overwrites.
    sqlx::query(&format!("VACUUM INTO '{temp_absolute}'"))
        .execute(state.db().pool())
        .await?;
    if let Err(error) = crate::file_security::secure_new_file(temp_path) {
        let _ = std::fs::remove_file(temp_path);
        return Err(std::io::Error::other(error).into());
    }
    if let Err(error) = sanitize_snapshot(temp_path).await {
        let _ = std::fs::remove_file(temp_path);
        return Err(error);
    }
    if let Err(error) = validate_snapshot_privacy(temp_path).await {
        let _ = std::fs::remove_file(temp_path);
        return Err(error);
    }

    let file = crate::file_security::open_readonly(temp_path).map_err(std::io::Error::other)?;
    let bytes = file.metadata()?.len() as i64;
    let mut reader = file;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let sha256 = crate::secrets::encode_hex(&hasher.finalize());
    let schema_version: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(state.db().pool())
            .await?;
    let data_range: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT MIN(accepted_at), MAX(accepted_at) FROM block_summaries")
            .fetch_one(state.db().pool())
            .await?;

    // fsync before the atomic rename so a crash never leaves a zero-length
    // or partial artifact under the final name (design §20.1).
    sync_file(temp_path)?;
    std::fs::rename(temp_path, final_path)?;

    crate::file_security::validate_file(final_path).map_err(std::io::Error::other)?;
    let inserted = sqlx::query(
        "INSERT INTO backup_artifacts (artifact_id, filename, bytes, sha256, schema_version, server_version, created_at, data_range_min, data_range_max, verification, create_operation_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)",
    )
    .bind(artifact_id)
    .bind(filename)
    .bind(bytes)
    .bind(&sha256)
    .bind(schema_version)
    .bind(crate::VERSION)
    .bind(now)
    .bind(&data_range.0)
    .bind(&data_range.1)
    .bind(Option::<String>::None)
    .execute(state.db().pool())
    .await;
    if let Err(error) = inserted {
        // Never leave an artifact on disk without its metadata row.
        let _ = std::fs::remove_file(final_path);
        return Err(error.into());
    }
    Ok(())
}

fn sync_file(path: &Path) -> Result<(), BackupError> {
    let file = crate::file_security::open_readwrite(path).map_err(std::io::Error::other)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) async fn sanitize_snapshot(path: &Path) -> Result<(), BackupError> {
    crate::file_security::validate_file(path).map_err(std::io::Error::other)?;
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Delete)
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;

    // Raw Peer addresses and Geo cache are deliberately absent from a
    // portable backup. Deleting followed by VACUUM matters: otherwise old
    // SQLite pages could retain the bytes after a logical DELETE.
    sqlx::query("UPDATE current_node_peers SET remote_ip=NULL")
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM geo_location_cache")
        .execute(&pool)
        .await?;

    redact_snapshot_text_columns(&pool, true).await?;
    redact_snapshot_receipts(&pool, true).await?;
    sqlx::query("VACUUM").execute(&pool).await?;
    pool.close().await;
    crate::file_security::validate_file(path).map_err(std::io::Error::other)?;
    Ok(())
}

pub(crate) async fn validate_snapshot_privacy(path: &Path) -> Result<(), BackupError> {
    crate::file_security::validate_file(path).map_err(std::io::Error::other)?;
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let result = async {
        process_snapshot_text_columns(&pool, false).await?;
        process_snapshot_receipts(&pool, false).await
    }
    .await;
    pool.close().await;
    result
}

/// Upper bound on the rows one snapshot scan keeps resident. The receipts
/// table alone can hold gigabytes, so materialising a whole table pins the
/// Server's resident set for the length of every backup attempt.
const SNAPSHOT_SCAN_BATCH: i64 = 512;

async fn redact_snapshot_text_columns(
    pool: &SqlitePool,
    sanitize: bool,
) -> Result<(), BackupError> {
    process_snapshot_text_columns(pool, sanitize).await
}

async fn process_snapshot_text_columns(
    pool: &SqlitePool,
    sanitize: bool,
) -> Result<(), BackupError> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(pool)
    .await?;
    for table in tables {
        let quoted_table = quote_identifier(&table);
        let columns = sqlx::query(&format!("PRAGMA table_info({quoted_table})"))
            .fetch_all(pool)
            .await?;
        for column in columns {
            let name: String = column.try_get("name")?;
            let declared_type: String = column.try_get("type")?;
            if !declared_type.to_ascii_uppercase().contains("TEXT") {
                continue;
            }
            let quoted_column = quote_identifier(&name);
            // Keyset pagination over rowid keeps the working set bounded: at
            // most SNAPSHOT_SCAN_BATCH rows are resident at any moment.
            let select = format!(
                "SELECT rowid AS __rowid, {quoted_column} FROM {quoted_table} \
                 WHERE typeof({quoted_column})='text' AND rowid > ? ORDER BY rowid LIMIT ?"
            );
            let update = format!("UPDATE {quoted_table} SET {quoted_column}=? WHERE rowid=?");
            let mut after: i64 = 0;
            loop {
                let rows = sqlx::query(&select)
                    .bind(after)
                    .bind(SNAPSHOT_SCAN_BATCH)
                    .fetch_all(pool)
                    .await?;
                let fetched = rows.len();
                for row in &rows {
                    let row_id: i64 = row.try_get("__rowid")?;
                    after = row_id;
                    let value: String = row.try_get(1)?;
                    let redacted = redact_stored_text(&value);
                    if redacted == value {
                        continue;
                    }
                    if !sanitize {
                        return Err(BackupError::Privacy(
                            "snapshot contains unredacted sensitive text".to_owned(),
                        ));
                    }
                    sqlx::query(&update)
                        .bind(redacted)
                        .bind(row_id)
                        .execute(pool)
                        .await?;
                }
                if fetched < SNAPSHOT_SCAN_BATCH as usize {
                    break;
                }
            }
        }
    }
    Ok(())
}

async fn redact_snapshot_receipts(pool: &SqlitePool, sanitize: bool) -> Result<(), BackupError> {
    process_snapshot_receipts(pool, sanitize).await
}

async fn process_snapshot_receipts(pool: &SqlitePool, sanitize: bool) -> Result<(), BackupError> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_report_receipts'",
    )
    .fetch_one(pool)
    .await?;
    if exists == 0 {
        return Ok(());
    }
    let mut after: i64 = 0;
    loop {
        let rows = sqlx::query(
            "SELECT rowid AS __rowid, receipt_body FROM agent_report_receipts \
             WHERE rowid > ? ORDER BY rowid LIMIT ?",
        )
        .bind(after)
        .bind(SNAPSHOT_SCAN_BATCH)
        .fetch_all(pool)
        .await?;
        let fetched = rows.len();
        for row in &rows {
            let row_id: i64 = row.try_get("__rowid")?;
            after = row_id;
            let bytes: Vec<u8> = row.try_get("receipt_body")?;
            let value = String::from_utf8(bytes)
                .map_err(|_| BackupError::Privacy("receipt body is not valid UTF-8".to_owned()))?;
            let redacted = redact_stored_text(&value);
            if redacted == value {
                continue;
            }
            if !sanitize {
                return Err(BackupError::Privacy(
                    "snapshot contains an unredacted receipt".to_owned(),
                ));
            }
            sqlx::query("UPDATE agent_report_receipts SET receipt_body=? WHERE rowid=?")
                .bind(redacted.into_bytes())
                .bind(row_id)
                .execute(pool)
                .await?;
        }
        if fetched < SNAPSHOT_SCAN_BATCH as usize {
            break;
        }
    }
    Ok(())
}

fn redact_stored_text(value: &str) -> String {
    match serde_json::from_str::<Value>(value) {
        Ok(json) => serde_json::to_string(&crate::redaction::redact_json_value(&json))
            .unwrap_or_else(|_| crate::redaction::redact_sensitive(value)),
        Err(_) => crate::redaction::redact_sensitive(value),
    }
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

// ---------------------------------------------------------------------------
// Offline layout guard (ADR 0008)
// ---------------------------------------------------------------------------

/// Canonical layout facts the guard decides over.
struct LayoutFacts {
    backup_dir: PathBuf,
    required_mount: PathBuf,
    backup_device: u64,
    mount_device: u64,
    mount_parent_device: u64,
    db_device: u64,
}

/// Pure guard decision over canonical facts. A backup directory on the live
/// database filesystem is refused because the artifact would grow beside the
/// very database it exists to survive.
fn decide_layout(facts: &LayoutFacts) -> Result<(), String> {
    if !facts.backup_dir.starts_with(&facts.required_mount) {
        return Err("backup directory is outside backup_required_mount".to_owned());
    }
    if facts.mount_device == facts.mount_parent_device {
        return Err("backup_required_mount is not a separate mounted filesystem".to_owned());
    }
    if facts.backup_device != facts.mount_device {
        return Err("backup directory is not on backup_required_mount".to_owned());
    }
    if facts.backup_device == facts.db_device {
        return Err("backup directory is on the database filesystem".to_owned());
    }
    Ok(())
}

/// Enforce the offline backup layout guard. The mount is optional: without
/// `backup_required_mount` the guard is inert and the operator is responsible
/// for placing `backup_dir` on a distinct disk (ADR 0008). Only creation is
/// gated; restore deliberately is not, so a misconfigured destination can
/// never block recovery.
pub(crate) fn check_layout(
    db_path: &Path,
    backup_dir: Option<&PathBuf>,
    required_mount: Option<&Path>,
) -> Result<(), String> {
    check_layout_with_device(db_path, backup_dir, required_mount, &device_of)
}

/// Canonicalize and read the filesystem identity of each relevant path, then
/// apply [`decide_layout`]. Split from [`check_layout`] so tests can inject
/// device identities.
fn check_layout_with_device<F>(
    db_path: &Path,
    backup_dir: Option<&PathBuf>,
    required_mount: Option<&Path>,
    device: &F,
) -> Result<(), String>
where
    F: Fn(&Path) -> std::io::Result<u64>,
{
    let Some(required_mount) = required_mount else {
        return Ok(());
    };
    let Some(backup_dir) = backup_dir else {
        return Err("backup_dir is not configured".to_owned());
    };
    if !required_mount.is_absolute() {
        return Err("backup_required_mount is not absolute".to_owned());
    }
    let canonical_mount = std::fs::canonicalize(required_mount)
        .map_err(|_| "backup_required_mount is not accessible".to_owned())?;
    let mount_device = device(&canonical_mount)
        .map_err(|_| "backup_required_mount is not accessible".to_owned())?;
    let mount_parent_device = canonical_mount
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .and_then(|parent| device(parent).ok())
        .unwrap_or(mount_device);
    crate::file_security::ensure_private_directory(backup_dir)
        .map_err(|_| "backup directory is unsafe or could not be created".to_owned())?;
    let canonical_backup = std::fs::canonicalize(backup_dir)
        .map_err(|_| "backup directory is not accessible".to_owned())?;
    let backup_device =
        device(&canonical_backup).map_err(|_| "backup directory is not accessible".to_owned())?;
    let db_device = device(db_path)
        .or_else(|_| {
            db_path
                .parent()
                .ok_or_else(|| std::io::Error::other("database path has no parent"))
                .and_then(device)
        })
        .map_err(|_| "database path is not accessible".to_owned())?;
    decide_layout(&LayoutFacts {
        backup_dir: canonical_backup,
        required_mount: canonical_mount,
        backup_device,
        mount_device,
        mount_parent_device,
        db_device,
    })
}

/// Real filesystem identity. `st_dev` is stable for mounted filesystems, so a
/// renamed or unmounted directory cannot impersonate the expected disk.
pub(crate) fn device_of(path: &Path) -> std::io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(std::fs::metadata(path)?.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(std::io::Error::other(
            "device identity is only available on unix",
        ))
    }
}

/// Verify one artifact through the `backup_verify` Operation: file
/// presence, SHA-256 recomputation, read-only SQLite integrity, and schema
/// version. The artifact row records the outcome; a failed verification
/// never deletes the artifact or any previous one.
pub async fn verify(state: &AppState, operation_id: &str) -> Result<(), BackupError> {
    let pool = state.db().pool();
    let params = crate::operations::operation_params(pool, operation_id).await?;
    let artifact_id = params
        .get("artifactId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if artifact_id.is_empty() {
        crate::operations::add_error(
            state,
            operation_id,
            "backup_verify_missing_artifact",
            "Verify Operation is missing its artifact reference",
        )
        .await?;
        crate::operations::finalize(
            state,
            operation_id,
            crate::operations::STATUS_FAILED,
            None,
            &["backups"],
        )
        .await?;
        return Ok(());
    }
    let Some(backup_dir) = state.backup_dir().map(|path| path.to_path_buf()) else {
        crate::operations::add_error(
            state,
            operation_id,
            "backup_dir_not_configured",
            "No backup directory is configured; set backup_dir in server.toml",
        )
        .await?;
        crate::operations::finalize(
            state,
            operation_id,
            crate::operations::STATUS_FAILED,
            None,
            &["backups"],
        )
        .await?;
        return Ok(());
    };

    // `data_range_min` is nullable (an empty chain has no range); decode only
    // the columns this path needs so a NULL never masquerades as a decode error.
    let artifact: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT filename, sha256, schema_version FROM backup_artifacts WHERE artifact_id = ?",
    )
    .bind(&artifact_id)
    .fetch_optional(pool)
    .await?;
    let Some((filename, expected_sha256, expected_schema)) = artifact else {
        crate::operations::add_error(
            state,
            operation_id,
            "backup_artifact_not_found",
            "Unknown backup artifact",
        )
        .await?;
        crate::operations::finalize(
            state,
            operation_id,
            crate::operations::STATUS_FAILED,
            None,
            &["backups"],
        )
        .await?;
        return Ok(());
    };
    let verified_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    let outcome = if crate::file_security::validate_private_directory(&backup_dir).is_err()
        || !crate::file_security::is_safe_basename(&filename)
    {
        Err(std::io::Error::other("backup artifact path is unsafe").into())
    } else {
        verify_artifact(
            &backup_dir.join(&filename),
            &expected_sha256,
            expected_schema,
        )
        .await
    };
    if crate::operations::is_cancel_requested(state, operation_id).await? {
        crate::operations::finalize(
            state,
            operation_id,
            crate::operations::STATUS_CANCELLED,
            None,
            &["backups"],
        )
        .await?;
        return Ok(());
    }
    let (verification, error_message, operation_status, result) = match outcome {
        Ok(()) => (
            "ok",
            None,
            crate::operations::STATUS_SUCCEEDED,
            serde_json::json!({
                "artifactId": artifact_id,
                "verification": "ok",
                "integrity": "ok",
                "checkedAt": verified_at,
            }),
        ),
        Err(error) => (
            "failed",
            Some(crate::redaction::redact_sensitive(&error.to_string())),
            crate::operations::STATUS_FAILED,
            serde_json::json!({
                "artifactId": artifact_id,
                "verification": "failed",
                "checkedAt": verified_at,
            }),
        ),
    };

    sqlx::query(
        "UPDATE backup_artifacts SET verification = ?, verified_at = ?, verification_error = ?, verify_operation_id = ? WHERE artifact_id = ?",
    )
    .bind(verification)
    .bind(&verified_at)
    .bind(&error_message)
    .bind(operation_id)
    .bind(&artifact_id)
    .execute(pool)
    .await?;
    if let Some(message) = error_message {
        crate::operations::add_error(state, operation_id, "backup_verification_failed", &message)
            .await?;
    }
    crate::operations::finalize(
        state,
        operation_id,
        operation_status,
        Some(&result),
        &["backups"],
    )
    .await?;
    Ok(())
}

async fn verify_artifact(
    path: &Path,
    expected_sha256: &str,
    expected_schema: i64,
) -> Result<(), BackupError> {
    crate::file_security::validate_file(path).map_err(std::io::Error::other)?;
    let mut file = crate::file_security::open_readonly(path).map_err(std::io::Error::other)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = crate::secrets::encode_hex(&hasher.finalize());
    if actual != expected_sha256 {
        return Err(std::io::Error::other("artifact checksum mismatch").into());
    }
    crate::file_security::validate_file(path).map_err(std::io::Error::other)?;
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true);
    let pool = sqlx::SqlitePool::connect_with(options).await?;
    let integrity: String = sqlx::query_scalar("PRAGMA quick_check(1)")
        .fetch_one(&pool)
        .await?;
    if integrity != "ok" {
        return Err(
            std::io::Error::other(format!("snapshot integrity check failed: {integrity}")).into(),
        );
    }
    let schema: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await?;
    pool.close().await;
    if schema != expected_schema {
        return Err(std::io::Error::other(format!(
            "snapshot schema version {schema} does not match the recorded version {expected_schema}"
        ))
        .into());
    }
    crate::backup::validate_snapshot_privacy(path).await?;
    Ok(())
}

/// Validate or create the configured backup directory with strict
/// permissions (design §20.1: backups rely on OS ownership/permission
/// protection). Returns a sanitized failure message on invalid layout.
fn prepare_backup_dir(path: &Path) -> Result<(), String> {
    crate::file_security::ensure_private_directory(path).map_err(|_| {
        "configured backup directory is group- or world-writable or otherwise unsafe".to_owned()
    })
}

/// Read-only metadata query for the Admin surface: last verified artifact
/// summary used by the Data overview (sanitized; never file contents).
pub async fn latest_artifact(
    pool: &SqlitePool,
) -> Result<Option<(String, String, i64, String, String)>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, String, i64, String, String)>(
        "SELECT artifact_id, filename, bytes, verification, created_at FROM backup_artifacts ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_facts(
        backup: &str,
        mount: &str,
        bdev: u64,
        mdev: u64,
        pdev: u64,
        ddev: u64,
    ) -> LayoutFacts {
        LayoutFacts {
            backup_dir: PathBuf::from(backup),
            required_mount: PathBuf::from(mount),
            backup_device: bdev,
            mount_device: mdev,
            mount_parent_device: pdev,
            db_device: ddev,
        }
    }

    #[test]
    fn decide_layout_accepts_a_separate_mounted_disk() {
        assert!(decide_layout(&layout_facts("/data/backups", "/data", 2, 2, 1, 3)).is_ok());
    }

    #[test]
    fn decide_layout_rejects_each_unsafe_layout() {
        // Backup outside the required mount.
        assert!(decide_layout(&layout_facts("/srv/backups", "/data", 2, 2, 1, 3)).is_err());
        // backup_required_mount is a plain directory, not a real mount point.
        assert!(decide_layout(&layout_facts("/data/backups", "/data", 2, 2, 2, 3)).is_err());
        // Backup directory is not on the required mount device.
        assert!(decide_layout(&layout_facts("/data/backups", "/data", 9, 2, 1, 3)).is_err());
        // Backup shares the live database filesystem.
        assert!(decide_layout(&layout_facts("/data/backups", "/data", 3, 3, 1, 3)).is_err());
    }

    /// A portable backup must not carry a raw Peer address or any retained
    /// country result, whatever provider produced it (issue #132).
    #[tokio::test]
    async fn sanitized_snapshot_drops_provider_keyed_geo_cache_and_peer_addresses() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('local_mmdb', '8.8.4.4', 'US', 'current', ?, ?, ?, ?, ?)")
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(crate::geo::cache_expiry(&now))
            .execute(database.pool())
            .await
            .unwrap();

        let snapshot = dir.path().join("portable.db");
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
        crate::backup::validate_snapshot_privacy(&snapshot)
            .await
            .unwrap();

        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&snapshot)
            .read_only(true);
        let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
        let geo_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM geo_location_cache")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(geo_rows, 0);
        let leaked_ips: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM current_node_peers WHERE remote_ip IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(leaked_ips, 0);
        pool.close().await;
    }

    #[tokio::test]
    async fn prepare_backup_dir_creates_restrictive_and_rejects_loose_permissions() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("backups");
        prepare_backup_dir(&target).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        // A world-writable directory must be refused.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o775)).unwrap();
        }
        let error = prepare_backup_dir(&target).unwrap_err();
        assert!(error.contains("group- or world-writable"));
    }

    #[tokio::test]
    async fn snapshot_redaction_masks_ip_literals_and_credentials_without_breaking_json() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE agent_report_receipts (receipt_body BLOB NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages (body) VALUES (?)")
            .bind(r#"{"error":"peer 203.0.113.7","token":"secret"}"#)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_report_receipts (receipt_body) VALUES (?)")
            .bind(br#"{"endpoint":"https://198.51.100.4"}"#.as_slice())
            .execute(&pool)
            .await
            .unwrap();

        redact_snapshot_text_columns(&pool, true).await.unwrap();
        redact_snapshot_receipts(&pool, true).await.unwrap();

        let body: String = sqlx::query_scalar("SELECT body FROM messages")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!body.contains("203.0.113.7"));
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["token"],
            "[REDACTED]"
        );
        let receipt: Vec<u8> = sqlx::query_scalar("SELECT receipt_body FROM agent_report_receipts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!String::from_utf8(receipt).unwrap().contains("198.51.100.4"));
        sqlx::query("INSERT INTO agent_report_receipts (receipt_body) VALUES (?)")
            .bind(vec![0xff_u8, b'2', b'0', b'3'])
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            redact_snapshot_receipts(&pool, true).await,
            Err(BackupError::Privacy(_))
        ));
    }
    #[tokio::test]
    async fn verify_detects_checksum_and_integrity_failures_read_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        // Corrupt bytes cannot match any real sha256.
        let path = dir.path().join("snapshot.db");
        std::fs::write(&path, b"not a database").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let error = verify_artifact(&path, "00".repeat(32).as_str(), 22)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"));
        // A valid snapshot passes sha256 but fails schema check.
        let snapshot = dir.path().join("valid.db");
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
        let mut hasher = sha2::Sha256::new();
        let bytes = std::fs::read(&snapshot).unwrap();
        hasher.update(&bytes);
        let digest = crate::secrets::encode_hex(&hasher.finalize());
        let error = verify_artifact(&snapshot, &digest, 99).await.unwrap_err();
        assert!(error.to_string().contains("schema version"));
        assert!(
            verify_artifact(&snapshot, &digest, crate::database::SERVER_SCHEMA_VERSION)
                .await
                .is_ok()
        );
    }

    /// The snapshot privacy scan must never materialise a whole table. When it
    /// does, every scheduled backup pins the entire snapshot's text in RSS for
    /// the length of the scan, and repeated attempts ratchet Server memory
    /// upward instead of releasing it.
    ///
    /// The measurement is process-wide, so the sample runs in a dedicated
    /// child process that executes only this test on a single thread; in a
    /// parallel suite every other test's allocations would otherwise be
    /// attributed to the scan.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn snapshot_privacy_scan_memory_stays_bounded() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        const CHILD_ENV: &str = "PLATPULSE_MEMORY_SAMPLE_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            let executable = std::env::current_exe().expect("test binary path");
            let status = std::process::Command::new(executable)
                .args([
                    "--exact",
                    "backup::tests::snapshot_privacy_scan_memory_stays_bounded",
                    "--test-threads=1",
                ])
                .env(CHILD_ENV, "1")
                .status()
                .expect("spawn the isolated memory sample");
            assert!(
                status.success(),
                "bounded-memory scan child failed: {status}"
            );
            return;
        }

        fn resident_kb() -> u64 {
            let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
            let pages: u64 = statm
                .split_whitespace()
                .nth(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            pages.saturating_mul(4)
        }

        let dir = tempfile::TempDir::new().unwrap();
        let snapshot = dir.path().join("privacy-scan.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&snapshot)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::query("CREATE TABLE agent_report_receipts (receipt_body BLOB NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        // ~48 MiB of valid-UTF-8 receipt bodies across many rows: large enough
        // that a whole-table materialisation is unmistakable, small enough to
        // build in a moment.
        sqlx::query(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 24000)
             INSERT INTO agent_report_receipts (receipt_body)
             SELECT hex(randomblob(1024)) FROM seq",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let baseline = resident_kb();
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(baseline));
        let sampler = {
            let stop = Arc::clone(&stop);
            let peak = Arc::clone(&peak);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    peak.fetch_max(resident_kb(), Ordering::Relaxed);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })
        };
        validate_snapshot_privacy(&snapshot).await.unwrap();
        stop.store(true, Ordering::Relaxed);
        sampler.join().unwrap();

        let growth_kb = peak.load(Ordering::Relaxed).saturating_sub(baseline);
        assert!(
            growth_kb < 16 * 1024,
            "privacy scan grew RSS by {growth_kb} KiB for a 48 MiB receipt table; \
             the scan must stream bounded batches instead of the whole table"
        );
    }
}
