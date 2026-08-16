//! Durable, recoverable long-running Operations (issue #50, webui.md §5.5).
//!
//! Every mutation returns immediately with an Operation reference; a worker
//! advances queued operations in bounded steps and persists progress,
//! warnings, errors, result summaries, the creating request ID, and the
//! linking Audit Event. Operation state and history are REST-authoritative
//! and survive navigation, browser close, or SSE loss — SSE only
//! accelerates refetch. A crashed worker re-arms by failing operations
//! left `running` (honest interruption), never by fabricating success.

use serde_json::Value;
use sqlx::SqlitePool;
use thiserror::Error;

use crate::http::AppState;

/// Worker cadence: queued operations are picked up within one tick and
/// bounded steps (one retention batch, one artifact, one Doctor run) keep
/// the single SQLite connection usable between steps.
pub const OPERATION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Maximum bytes of one sanitized JSON payload column (params/warnings/
/// errors/result). Keeps Operation rows bounded on disk.
pub const OPERATION_JSON_LIMIT: usize = 64 * 1024;

pub const STATUS_QUEUED: &str = "queued";
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_SUCCEEDED: &str = "succeeded";
pub const STATUS_SUCCEEDED_WITH_WARNINGS: &str = "succeeded_with_warnings";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_CANCELLED: &str = "cancelled";

pub const KIND_RETENTION_RUN: &str = "retention_run";
pub const KIND_BACKUP_CREATE: &str = "backup_create";
pub const KIND_BACKUP_VERIFY: &str = "backup_verify";
pub const KIND_DOCTOR_RUN: &str = "doctor_run";
pub const KIND_RESTORE: &str = "restore";

#[derive(Debug, Error)]
pub enum OperationError {
    #[error("operation database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("operation JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("operation domain error: {0}")]
    Domain(String),
}

impl From<crate::backup::BackupError> for OperationError {
    fn from(error: crate::backup::BackupError) -> Self {
        OperationError::Domain(error.to_string())
    }
}

impl From<crate::restore::RestoreError> for OperationError {
    fn from(error: crate::restore::RestoreError) -> Self {
        OperationError::Domain(error.to_string())
    }
}

/// Create an Operation row and its creating Audit Event in one transaction.
/// Returns the operation id; the caller keeps the audit link for the
/// success response (design §8.4: Operation history links to Audit).
pub async fn create_operation(
    pool: &SqlitePool,
    kind: &str,
    params: &Value,
    request_id: &str,
    actor_user_id: &str,
    event_kind: &str,
) -> Result<(String, i64), OperationError> {
    let operation_id = uuid::Uuid::new_v4().to_string();
    let created_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    let sanitized_params = crate::redaction::redact_json_value(params);
    let params_text = serde_json::to_string(&sanitized_params)?;
    if params_text.len() > OPERATION_JSON_LIMIT {
        return Err(OperationError::Json(serde_json::Error::io(
            std::io::Error::other("operation params exceed the bounded size"),
        )));
    }
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO operations (operation_id, kind, status, progress_percent, request_id, params_json, warnings_json, errors_json, created_by_user_id, created_at) VALUES (?, ?, ?, 0, ?, ?, '[]', '[]', ?, ?)",
    )
    .bind(&operation_id)
    .bind(kind)
    .bind(STATUS_QUEUED)
    .bind(request_id)
    .bind(&params_text)
    .bind(actor_user_id)
    .bind(&created_at)
    .execute(&mut *tx)
    .await?;
    crate::auth::insert_audit_event(
        &mut *tx,
        Some(actor_user_id),
        event_kind,
        "operation",
        &operation_id,
        Some(&serde_json::json!({
            "kind": kind,
            "params": sanitized_params,
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
    Ok((operation_id, audit_event_id))
}

/// Next operation to advance: an in-flight multi-step `running` operation
/// first (bounded retention batches continue across ticks), otherwise the
/// oldest queued operation. Terminal rows are never picked up.
pub async fn next_queued(pool: &SqlitePool) -> Result<Option<String>, OperationError> {
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT operation_id FROM operations WHERE status IN (?, ?) ORDER BY CASE status WHEN 'running' THEN 0 ELSE 1 END, created_at, operation_id LIMIT 1",
    )
    .bind(STATUS_RUNNING)
    .bind(STATUS_QUEUED)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id,)| id))
}

/// Claim a queued operation for execution, or confirm an in-flight
/// multi-step operation is still running. Returns `false` when the row is
/// terminal (e.g. cancelled before the worker picked it up).
pub async fn mark_running(pool: &SqlitePool, operation_id: &str) -> Result<bool, OperationError> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let result = sqlx::query(
        "UPDATE operations SET status = ?, started_at = COALESCE(started_at, ?) WHERE operation_id = ? AND status = ?",
    )
    .bind(STATUS_RUNNING)
    .bind(&now)
    .bind(operation_id)
    .bind(STATUS_QUEUED)
    .execute(pool)
    .await?;
    if result.rows_affected() == 1 {
        return Ok(true);
    }
    let status: String = sqlx::query_scalar("SELECT status FROM operations WHERE operation_id = ?")
        .bind(operation_id)
        .fetch_one(pool)
        .await?;
    Ok(status == STATUS_RUNNING)
}

pub async fn operation_kind(
    pool: &SqlitePool,
    operation_id: &str,
) -> Result<String, OperationError> {
    Ok(
        sqlx::query_scalar("SELECT kind FROM operations WHERE operation_id = ?")
            .bind(operation_id)
            .fetch_one(pool)
            .await?,
    )
}

pub async fn operation_params(
    pool: &SqlitePool,
    operation_id: &str,
) -> Result<Value, OperationError> {
    let text: String =
        sqlx::query_scalar("SELECT params_json FROM operations WHERE operation_id = ?")
            .bind(operation_id)
            .fetch_one(pool)
            .await?;
    Ok(serde_json::from_str(&text)?)
}

/// Persist a progress step (percent 0–100 plus a short label). Publishes
/// an `operations` invalidation so open Admin views refetch through REST.
pub async fn set_progress(
    state: &AppState,
    operation_id: &str,
    percent: i64,
    label: &str,
) -> Result<(), OperationError> {
    let percent = percent.clamp(0, 100);
    sqlx::query(
        "UPDATE operations SET progress_percent = ?, progress_label = ? WHERE operation_id = ?",
    )
    .bind(percent)
    .bind(label)
    .bind(operation_id)
    .execute(state.db().pool())
    .await?;
    state
        .admin_realtime()
        .publish("operations", Some(operation_id), 1);
    Ok(())
}

/// Append a sanitized warning to the Operation's warning list.
pub async fn add_warning(
    state: &AppState,
    operation_id: &str,
    code: &str,
    message: &str,
) -> Result<(), OperationError> {
    let message = crate::redaction::redact_sensitive(message);
    append_json_list(
        state,
        operation_id,
        "warnings_json",
        &serde_json::json!({ "code": code, "message": message }),
    )
    .await
}

/// Append a sanitized error to the Operation's error list.
pub async fn add_error(
    state: &AppState,
    operation_id: &str,
    code: &str,
    message: &str,
) -> Result<(), OperationError> {
    let message = crate::redaction::redact_sensitive(message);
    append_json_list(
        state,
        operation_id,
        "errors_json",
        &serde_json::json!({ "code": code, "message": message }),
    )
    .await
}

async fn append_json_list(
    state: &AppState,
    operation_id: &str,
    column: &str,
    entry: &Value,
) -> Result<(), OperationError> {
    let text: String = sqlx::query_scalar(&format!(
        "SELECT {column} FROM operations WHERE operation_id = ?"
    ))
    .bind(operation_id)
    .fetch_one(state.db().pool())
    .await?;
    let mut list: Vec<Value> = serde_json::from_str(&text)?;
    list.push(entry.clone());
    let encoded = serde_json::to_string(&list)?;
    if encoded.len() > OPERATION_JSON_LIMIT {
        return Ok(()); // bounded sink: stop recording, never fail the run
    }
    sqlx::query(&format!(
        "UPDATE operations SET {column} = ? WHERE operation_id = ?"
    ))
    .bind(&encoded)
    .bind(operation_id)
    .execute(state.db().pool())
    .await?;
    Ok(())
}

/// `true` when the Owner asked to cancel this operation. Long-running steps
/// check this between bounded batches.
pub async fn is_cancel_requested(
    state: &AppState,
    operation_id: &str,
) -> Result<bool, OperationError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT cancel_requested FROM operations WHERE operation_id = ?",
    )
    .bind(operation_id)
    .fetch_one(state.db().pool())
    .await?
        == 1)
}

/// Terminal write for one Operation: status, result summary, finish time,
/// and a completion Audit Event. Publishes `operations` plus the
/// domain-specific resources so REST pages refetch (SSE never carries the
/// payload itself).
pub async fn finalize(
    state: &AppState,
    operation_id: &str,
    status: &str,
    result: Option<&Value>,
    publish_resources: &[&str],
) -> Result<(), OperationError> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let sanitized_result = result.map(crate::redaction::redact_json_value);
    let result_text = sanitized_result
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let mut tx = state.db().pool().begin().await?;
    sqlx::query(
        "UPDATE operations SET status = ?, result_json = ?, finished_at = ? WHERE operation_id = ?",
    )
    .bind(status)
    .bind(&result_text)
    .bind(&now)
    .bind(operation_id)
    .execute(&mut *tx)
    .await?;
    crate::auth::insert_audit_event(
        &mut *tx,
        None,
        "operation_finished",
        "operation",
        operation_id,
        Some(&serde_json::json!({
            "status": status,
            "result": sanitized_result,
        })),
    )
    .await?;
    tx.commit().await?;
    state
        .admin_realtime()
        .publish("operations", Some(operation_id), 1);
    for resource in publish_resources {
        state.admin_realtime().publish(*resource, None::<String>, 1);
    }
    Ok(())
}

/// Mark operations left `running` by a crash as failed (honest
/// interruption). Queued rows survive and are picked up by the new worker.
pub async fn requeue_interrupted_operations(pool: &SqlitePool) -> Result<u64, OperationError> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let result = sqlx::query(
        "UPDATE operations SET status = ?, finished_at = ?, errors_json = ? WHERE status = ?",
    )
    .bind(STATUS_FAILED)
    .bind(&now)
    .bind(
        serde_json::json!([{
            "code": "interrupted_by_restart",
            "message": "Operation was interrupted by a Server restart; review state and re-run if needed",
        }])
        .to_string(),
    )
    .bind(STATUS_RUNNING)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Advance the oldest queued operation by one bounded step. Returns `Ok(0)`
/// when the queue is empty, `Ok(1)` after a step was taken.
pub async fn process_operations(state: &AppState) -> Result<usize, OperationError> {
    let Some(operation_id) = next_queued(state.db().pool()).await? else {
        return Ok(0);
    };
    if !mark_running(state.db().pool(), &operation_id).await? {
        return Ok(1); // cancelled before pickup; already terminal
    }
    let kind = operation_kind(state.db().pool(), &operation_id).await?;
    match kind.as_str() {
        KIND_RETENTION_RUN => crate::retention::execute_step(state, &operation_id).await?,
        KIND_BACKUP_CREATE => crate::backup::create(state, &operation_id).await?,
        KIND_BACKUP_VERIFY => crate::backup::verify(state, &operation_id).await?,
        KIND_DOCTOR_RUN => crate::doctor::run(state, &operation_id).await?,
        KIND_RESTORE => crate::restore::execute(state, &operation_id).await?,
        _ => {
            let _ = crate::operations::add_error(
                state,
                &operation_id,
                "unknown_operation_kind",
                "Server does not know this Operation kind",
            )
            .await;
            let _ = crate::operations::finalize(
                state,
                &operation_id,
                STATUS_FAILED,
                None,
                &["operations"],
            )
            .await;
        }
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_operation_links_audit_in_one_transaction() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(database.pool())
            .await
            .unwrap();
        let (operation_id, audit_event_id) = create_operation(
            database.pool(),
            KIND_DOCTOR_RUN,
            &serde_json::json!({
                "confirmation": "203.0.113.7",
                "token": "secret",
            }),
            "req-1",
            "owner",
            "doctor_started",
        )
        .await
        .unwrap();
        let row: (String, String, i64, String) = sqlx::query_as(
            "SELECT kind, status, audit_event_id, created_by_user_id FROM operations WHERE operation_id = ?",
        )
        .bind(&operation_id)
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(
            row,
            (
                KIND_DOCTOR_RUN.to_owned(),
                STATUS_QUEUED.to_owned(),
                audit_event_id,
                "owner".to_owned()
            )
        );
        let params: String =
            sqlx::query_scalar("SELECT params_json FROM operations WHERE operation_id = ?")
                .bind(&operation_id)
                .fetch_one(database.pool())
                .await
                .unwrap();
        assert!(!params.contains("203.0.113.7"));
        assert!(!params.contains("secret"));
        let audit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='doctor_started' AND target_id=?",
        )
        .bind(&operation_id)
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(audit, 1);
    }

    #[tokio::test]
    async fn requeue_fails_interrupted_running_operations_and_keeps_queued() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO operations (operation_id, kind, status, created_at, params_json, warnings_json, errors_json) VALUES ('op-a', 'retention_run', 'running', ?, '{}', '[]', '[]')")
            .bind(&now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO operations (operation_id, kind, status, created_at, params_json, warnings_json, errors_json) VALUES ('op-b', 'doctor_run', 'queued', ?, '{}', '[]', '[]')")
            .bind(&now)
            .execute(pool)
            .await
            .unwrap();
        assert_eq!(requeue_interrupted_operations(pool).await.unwrap(), 1);
        let failed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM operations WHERE operation_id='op-a' AND status='failed'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(failed, 1);
        let queued: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM operations WHERE operation_id='op-b' AND status='queued'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(queued, 1);
        let errors: String =
            sqlx::query_scalar("SELECT errors_json FROM operations WHERE operation_id='op-a'")
                .fetch_one(pool)
                .await
                .unwrap();
        assert!(errors.contains("interrupted_by_restart"));
    }
}
