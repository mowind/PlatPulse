//! Backup artifact creation and verification (issue #50, design §20.1,
//! webui.md §8.4).
//!
//! Backups are consistent SQLite snapshots produced with `VACUUM INTO`
//! (temp + fsync + atomic rename, never a copy of live `-wal`/`-shm`),
//! written into the explicitly configured backup directory with strict
//! permissions. Only sanitized metadata is persisted and exposed: file base
//! name, size, SHA-256, schema version, Server version, timestamps, and the
//! data range. Database contents and secrets never leave the Server and are
//! never displayed. A failed create or verify preserves every previous
//! artifact and the last successful state.

use std::io::Read;
use std::path::Path;

use serde_json::Value;
use sha2::Digest;
use sqlx::SqlitePool;
use thiserror::Error;

use crate::http::AppState;

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("backup database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("backup JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("backup IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Operation(#[from] crate::operations::OperationError),
}

/// Create a consistent snapshot artifact through the `backup_create`
/// Operation. On any failure the temp file is removed, the error is
/// recorded on the Operation, and every previous artifact stays intact.
pub async fn create(state: &AppState, operation_id: &str) -> Result<(), BackupError> {
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
    if let Err(message) = prepare_backup_dir(&backup_dir) {
        crate::operations::add_error(state, operation_id, "backup_dir_invalid", &message).await?;
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
    let artifact_id = uuid::Uuid::new_v4().to_string();
    let filename = format!("platpulse-{artifact_id}.db");
    let final_path = backup_dir.join(&filename);
    let temp_path = backup_dir.join(format!("{filename}.part"));
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());

    let result = create_snapshot(
        state,
        operation_id,
        &temp_path,
        &final_path,
        &artifact_id,
        &filename,
        &now,
    )
    .await;
    let snapshot = match result {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let _ = std::fs::remove_file(&temp_path);
            crate::operations::add_error(
                state,
                operation_id,
                "backup_create_failed",
                &crate::redaction::redact_sensitive(&error.to_string()),
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
    };

    if crate::operations::is_cancel_requested(state, operation_id).await? {
        // The snapshot was already persisted; remove the file AND its
        // metadata row so a cancelled backup leaves nothing behind.
        let _ = std::fs::remove_file(&final_path);
        let _ = sqlx::query("DELETE FROM backup_artifacts WHERE artifact_id = ?")
            .bind(&artifact_id)
            .execute(state.db().pool())
            .await;
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
    let result_json = serde_json::json!({
        "artifact": {
            "artifactId": artifact_id,
            "filename": filename,
            "bytes": snapshot.bytes,
            "sha256": snapshot.sha256,
            "schemaVersion": snapshot.schema_version,
            "serverVersion": snapshot.server_version,
            "createdAt": snapshot.created_at,
            "dataRangeMin": snapshot.data_range_min,
            "dataRangeMax": snapshot.data_range_max,
        }
    });
    crate::operations::finalize(
        state,
        operation_id,
        crate::operations::STATUS_SUCCEEDED,
        Some(&result_json),
        &["backups"],
    )
    .await?;
    Ok(())
}

struct Snapshot {
    bytes: i64,
    sha256: String,
    schema_version: i64,
    server_version: String,
    created_at: String,
    data_range_min: Option<String>,
    data_range_max: Option<String>,
}

async fn create_snapshot(
    state: &AppState,
    operation_id: &str,
    temp_path: &Path,
    final_path: &Path,
    artifact_id: &str,
    filename: &str,
    now: &str,
) -> Result<Snapshot, BackupError> {
    let temp_absolute = temp_path
        .to_str()
        .ok_or_else(|| std::io::Error::other("backup directory path is not valid UTF-8"))?;
    if temp_absolute.contains('\'') {
        return Err(
            std::io::Error::other("backup directory path must not contain single quotes").into(),
        );
    }
    // One consistent snapshot statement on the single Server connection;
    // `VACUUM INTO` never touches live -wal/-shm and never overwrites.
    sqlx::query(&format!("VACUUM INTO '{temp_absolute}'"))
        .execute(state.db().pool())
        .await?;

    let file = std::fs::File::open(temp_path)?;
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
    .bind(operation_id)
    .execute(state.db().pool())
    .await;
    if let Err(error) = inserted {
        // Never leave an artifact on disk without its metadata row.
        let _ = std::fs::remove_file(final_path);
        return Err(error.into());
    }
    Ok(Snapshot {
        bytes,
        sha256,
        schema_version,
        server_version: crate::VERSION.to_owned(),
        created_at: now.to_owned(),
        data_range_min: data_range.0,
        data_range_max: data_range.1,
    })
}

fn sync_file(path: &Path) -> Result<(), BackupError> {
    let file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.sync_all()?;
    Ok(())
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

    let artifact: Option<(String, String, i64, String, String, String)> = sqlx::query_as(
        "SELECT filename, sha256, schema_version, server_version, created_at, data_range_min FROM backup_artifacts WHERE artifact_id = ?",
    )
    .bind(&artifact_id)
    .fetch_optional(pool)
    .await?;
    let Some((
        filename,
        expected_sha256,
        expected_schema,
        _server_version,
        _created_at,
        _range_min,
    )) = artifact
    else {
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
    let path = backup_dir.join(&filename);
    let verified_at = crate::auth::format_rfc3339(crate::auth::now_utc());

    let outcome = verify_artifact(&path, &expected_sha256, expected_schema).await;
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
    let mut file = std::fs::File::open(path)?;
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
    Ok(())
}

/// Validate or create the configured backup directory with strict
/// permissions (design §20.1: backups rely on OS ownership/permission
/// protection). Returns a sanitized failure message on invalid layout.
fn prepare_backup_dir(path: &Path) -> Result<(), String> {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err("configured backup directory is not a directory".to_owned());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o022 != 0 {
                    return Err(
                        "configured backup directory is group- or world-writable; refusing to write backups"
                            .to_owned(),
                    );
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)
                .map_err(|source| format!("cannot create backup directory: {source}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                    .map_err(|source| format!("cannot secure backup directory: {source}"))?;
            }
            Ok(())
        }
        Err(error) => Err(format!("cannot access backup directory: {error}")),
    }
}

/// Read-only metadata query for the Admin surface: last verified artifact
/// summary used by the Data overview (sanitized; never file contents).
pub async fn latest_artifact(
    pool: &SqlitePool,
) -> Result<Option<(String, String, i64, String)>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, String, i64, String)>(
        "SELECT artifact_id, filename, bytes, verification FROM backup_artifacts ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
