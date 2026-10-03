//! Durable, recoverable long-running Operations (issue #50, webui.md §5.5).
//!
//! Every mutation returns immediately with an Operation reference; a worker
//! advances queued operations in bounded steps and persists progress,
//! warnings, errors, result summaries, the creating request ID, and the
//! linking Audit Event. Operation state and history are REST-authoritative
//! and survive navigation, browser close, or SSE loss — SSE only
//! accelerates refetch. A crashed worker re-arms by failing operations
//! left `running` (honest interruption), never by fabricating success.
//!
//! An Owner command also carries its own identity (issue #211): the browser
//! supplies an opaque request id per intent and the Server records it beside
//! the Operation it queued. That recorded identity - not a disabled button -
//! is what makes a doubled click, a second tab, a repeated HTTP request, or a
//! retry after a lost response reconcile to one Operation, and a kind whose
//! duplicate would release production history twice refuses a second open run
//! outright.

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

/// How long a recorded Owner command stays replayable. A retention run is
/// confirmed against a preview that lives for 24 hours (issue #210), so the
/// command that bound it is kept at least that long: an Owner who retries a
/// lost response or reopens the tab still reconciles to the recorded Operation
/// instead of queueing a second cleanup.
pub const OPERATION_REQUEST_TTL_HOURS: i64 = 24;

/// Bounded key for a recorded command identity, mirroring the CHECK in
/// migration 0064.
pub const OPERATION_REQUEST_ID_MAX_LEN: usize = 128;

/// The kinds where a second open run would release production history twice,
/// so the ledger refuses one instead of reconciling to it. Migration 0064 pins
/// the same list in a partial unique index, which is what makes the refusal hold
/// for a concurrent writer too. Every other kind is only deduplicated by intent:
/// a distinct command of the same kind is a distinct command.
pub const OPERATION_EXCLUSIVE_KINDS: &[&str] = &[KIND_RETENTION_RUN];

/// An Owner-supplied request id is opaque to the Server: only its shape is
/// validated so ledger keys stay bounded. The bound is counted in characters,
/// so a non-ASCII id of the documented length is not rejected on bytes alone.
pub fn validate_request_id(request_id: &str) -> bool {
    !request_id.trim().is_empty() && request_id.chars().count() <= OPERATION_REQUEST_ID_MAX_LEN
}

/// Fingerprint pinning a command to the intent it was confirmed for (issue
/// #211: the retention preview it bound), so reusing a request id for a
/// different intent is a conflict, never a silent second action.
pub fn request_intent_fingerprint(kind: &str, target: &str) -> String {
    format!("{kind}:{target}")
}

/// What queueing one Owner command did. The Server decides the outcome; the
/// browser only reports it.
#[derive(Debug, Clone)]
pub enum QueueOutcome {
    /// A new Operation was queued and is now the recorded command.
    Queued {
        operation_id: String,
        audit_event_id: i64,
    },
    /// The Server reconciled to an already-recorded command for the same
    /// request id or the same intent: nothing was queued a second time.
    Replayed {
        operation_id: String,
        audit_event_id: i64,
    },
    /// Another run of this destructive kind is already queued or running, so
    /// this command was refused instead of doubling the work one confirmation
    /// authorized.
    AlreadyOpen { operation_id: String },
    /// The request id was already used for a different intent.
    RequestConflict { operation_id: String },
}

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
///
/// This entry point queues unconditionally: it is for commands whose repeat is
/// harmless or impossible. A command a browser can re-send (issue #211) goes
/// through create_operation_once instead.
pub async fn create_operation(
    pool: &SqlitePool,
    kind: &str,
    params: &Value,
    request_id: &str,
    actor_user_id: &str,
    event_kind: &str,
) -> Result<(String, i64), OperationError> {
    let (sanitized_params, params_text) = prepare_params(params)?;
    let mut tx = pool.begin().await?;
    let ids = insert_operation(
        &mut tx,
        kind,
        &params_text,
        &sanitized_params,
        request_id,
        actor_user_id,
        event_kind,
    )
    .await?;
    tx.commit().await?;
    Ok(ids)
}

/// Queue one Owner command at most once (issue #211, story 39).
///
/// The browser supplies the request id, so the Server - not a disabled button -
/// is authoritative: a doubled click, a second tab, a repeated HTTP request, or
/// a retry after a lost response either replays the recorded Operation or is
/// refused, and never queues the same cleanup twice. The decision and the row
/// writes share one transaction, so a crash cannot leave a queued Operation
/// without the identity that records it.
pub async fn create_operation_once(
    pool: &SqlitePool,
    kind: &str,
    params: &Value,
    request_id: &str,
    intent_fingerprint: &str,
    actor_user_id: &str,
    event_kind: &str,
) -> Result<QueueOutcome, OperationError> {
    let now = crate::auth::now_utc();
    let (sanitized_params, params_text) = prepare_params(params)?;
    let mut tx = pool.begin().await?;

    // Whatever the browser believes about its own last attempt, a recorded
    // identity is the answer: the same command replays, a reused identity for a
    // different command is a conflict, and a second open run of a destructive
    // kind is refused instead of releasing the same history twice.
    if let Some(outcome) =
        recorded_outcome(&mut tx, kind, request_id, intent_fingerprint, now).await?
    {
        return Ok(outcome);
    }
    let (operation_id, audit_event_id) = match record_operation(
        &mut tx,
        kind,
        &params_text,
        &sanitized_params,
        request_id,
        intent_fingerprint,
        actor_user_id,
        event_kind,
    )
    .await
    {
        Ok(ids) => ids,
        Err(error) => {
            // A concurrent command may have won the race between the checks
            // above and this write (the unique indexes in migration 0064 reject
            // the loser): reconcile to the recorded outcome instead of
            // reporting an opaque database failure.
            let _ = tx.rollback().await;
            if let Some(outcome) =
                reconcile_recorded_command(pool, kind, request_id, intent_fingerprint, now).await?
            {
                return Ok(outcome);
            }
            return Err(error);
        }
    };
    tx.commit().await?;
    Ok(QueueOutcome::Queued {
        operation_id,
        audit_event_id,
    })
}

/// Sanitize and bound one params payload: the Operation row is authoritative
/// history, so a payload that cannot be stored fails the command instead of
/// being silently truncated.
fn prepare_params(params: &Value) -> Result<(Value, String), OperationError> {
    let sanitized = crate::redaction::redact_json_value(params);
    let text = serde_json::to_string(&sanitized)?;
    if text.len() > OPERATION_JSON_LIMIT {
        return Err(OperationError::Json(serde_json::Error::io(
            std::io::Error::other("operation params exceed the bounded size"),
        )));
    }
    Ok((sanitized, text))
}

/// Insert the Operation row, its creating Audit Event, and the audit link.
/// Returns the Operation id and the Audit Event id.
async fn insert_operation(
    executor: &mut sqlx::SqliteConnection,
    kind: &str,
    params_text: &str,
    sanitized_params: &Value,
    request_id: &str,
    actor_user_id: &str,
    event_kind: &str,
) -> Result<(String, i64), OperationError> {
    let operation_id = uuid::Uuid::new_v4().to_string();
    let created_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    sqlx::query(
        "INSERT INTO operations (operation_id, kind, status, progress_percent, request_id, params_json, warnings_json, errors_json, created_by_user_id, created_at) VALUES (?, ?, ?, 0, ?, ?, '[]', '[]', ?, ?)",
    )
    .bind(&operation_id)
    .bind(kind)
    .bind(STATUS_QUEUED)
    .bind(request_id)
    .bind(params_text)
    .bind(actor_user_id)
    .bind(&created_at)
    .execute(&mut *executor)
    .await?;
    crate::auth::insert_audit_event(
        &mut *executor,
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
        .fetch_one(&mut *executor)
        .await?;
    sqlx::query("UPDATE operations SET audit_event_id = ? WHERE operation_id = ?")
        .bind(audit_event_id)
        .bind(&operation_id)
        .execute(&mut *executor)
        .await?;
    Ok((operation_id, audit_event_id))
}

/// Write the Operation row, its Audit Event, and the recorded command identity
/// in one step: the ledger never holds a command without its Operation, and a
/// queued Operation is never left without the identity that lets a repeat
/// reconcile to it.
#[allow(clippy::too_many_arguments)]
async fn record_operation(
    executor: &mut sqlx::SqliteConnection,
    kind: &str,
    params_text: &str,
    sanitized_params: &Value,
    request_id: &str,
    intent_fingerprint: &str,
    actor_user_id: &str,
    event_kind: &str,
) -> Result<(String, i64), OperationError> {
    let (operation_id, audit_event_id) = insert_operation(
        &mut *executor,
        kind,
        params_text,
        sanitized_params,
        request_id,
        actor_user_id,
        event_kind,
    )
    .await?;
    let now = crate::auth::now_utc();
    // Closed windows are dropped before the new row is recorded: pruning runs
    // inside the accepted command's transaction, so it can never race a lookup,
    // and an expired row can never block re-confirming the same intent after
    // its replay window has passed.
    prune_expired_operation_requests(&mut *executor, now).await?;
    insert_operation_request(
        &mut *executor,
        request_id,
        kind,
        intent_fingerprint,
        &operation_id,
        audit_event_id,
        now,
    )
    .await?;
    Ok((operation_id, audit_event_id))
}

/// A recorded Owner command: the intent it was confirmed for and the Operation
/// and Audit Event it produced. Kept for OPERATION_REQUEST_TTL_HOURS so a retry
/// or a reloaded tab reconciles to the same Operation.
#[derive(Debug, sqlx::FromRow)]
struct OperationRequestRow {
    kind: String,
    intent_fingerprint: String,
    operation_id: String,
    audit_event_id: i64,
}

impl OperationRequestRow {
    /// The recorded row's answer for a command whose intent is known: the same
    /// intent replays, a different one is a conflict.
    fn outcome_for(&self, kind: &str, intent_fingerprint: &str) -> QueueOutcome {
        if self.kind == kind && self.intent_fingerprint == intent_fingerprint {
            QueueOutcome::Replayed {
                operation_id: self.operation_id.clone(),
                audit_event_id: self.audit_event_id,
            }
        } else {
            QueueOutcome::RequestConflict {
                operation_id: self.operation_id.clone(),
            }
        }
    }
}

/// Look a command up by its request id. An expired row reads as absent, so a
/// read-only lookup never has to write.
async fn load_operation_request(
    executor: &mut sqlx::SqliteConnection,
    request_id: &str,
    now: time::OffsetDateTime,
) -> Result<Option<OperationRequestRow>, sqlx::Error> {
    sqlx::query_as::<_, OperationRequestRow>(
        "SELECT kind, intent_fingerprint, operation_id, audit_event_id FROM operation_requests WHERE request_id = ? AND expires_at > ?",
    )
    .bind(request_id)
    .bind(crate::auth::format_rfc3339(now))
    .fetch_optional(&mut *executor)
    .await
}

/// Look a command up by the intent it was confirmed for, which is what makes a
/// second tab with its own request id reconcile instead of queueing again.
async fn load_operation_request_by_intent(
    executor: &mut sqlx::SqliteConnection,
    kind: &str,
    intent_fingerprint: &str,
    now: time::OffsetDateTime,
) -> Result<Option<OperationRequestRow>, sqlx::Error> {
    sqlx::query_as::<_, OperationRequestRow>(
        "SELECT kind, intent_fingerprint, operation_id, audit_event_id FROM operation_requests WHERE kind = ? AND intent_fingerprint = ? AND expires_at > ?",
    )
    .bind(kind)
    .bind(intent_fingerprint)
    .bind(crate::auth::format_rfc3339(now))
    .fetch_optional(&mut *executor)
    .await
}

/// Record the command identity beside the Operation it queued.
async fn insert_operation_request(
    executor: &mut sqlx::SqliteConnection,
    request_id: &str,
    kind: &str,
    intent_fingerprint: &str,
    operation_id: &str,
    audit_event_id: i64,
    now: time::OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO operation_requests (request_id, kind, intent_fingerprint, operation_id, audit_event_id, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(request_id)
    .bind(kind)
    .bind(intent_fingerprint)
    .bind(operation_id)
    .bind(audit_event_id)
    .bind(crate::auth::format_rfc3339(now))
    .bind(crate::auth::format_rfc3339(
        now + time::Duration::hours(OPERATION_REQUEST_TTL_HOURS),
    ))
    .execute(&mut *executor)
    .await
    .map(|_| ())
}

/// Drop recorded commands whose replay window closed.
async fn prune_expired_operation_requests(
    executor: &mut sqlx::SqliteConnection,
    now: time::OffsetDateTime,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM operation_requests WHERE expires_at <= ?")
        .bind(crate::auth::format_rfc3339(now))
        .execute(&mut *executor)
        .await?;
    Ok(result.rows_affected())
}

/// The one queued or running Operation of this kind, if any: the partial unique
/// index in migration 0064 makes the same promise to a concurrent writer, so
/// this check answers the common case and the index backstops the race.
async fn open_operation_of_kind(
    executor: &mut sqlx::SqliteConnection,
    kind: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT operation_id FROM operations WHERE kind = ? AND status IN (?, ?) ORDER BY created_at, operation_id LIMIT 1",
    )
    .bind(kind)
    .bind(STATUS_QUEUED)
    .bind(STATUS_RUNNING)
    .fetch_optional(&mut *executor)
    .await
}

/// Re-read the ledger after a rejected write: whatever won the race is the
/// honest answer.
async fn reconcile_recorded_command(
    pool: &SqlitePool,
    kind: &str,
    request_id: &str,
    intent_fingerprint: &str,
    now: time::OffsetDateTime,
) -> Result<Option<QueueOutcome>, OperationError> {
    let mut conn = pool.acquire().await?;
    recorded_outcome(&mut conn, kind, request_id, intent_fingerprint, now).await
}

/// The command one identity already recorded, if any: the same request id is
/// the answer whatever the browser believes about its own last attempt, and the
/// same intent under another request id (a second tab) is the same command, not
/// a second one (issue #211, story 39).
async fn recorded_command(
    conn: &mut sqlx::SqliteConnection,
    kind: &str,
    request_id: &str,
    intent_fingerprint: &str,
    now: time::OffsetDateTime,
) -> Result<Option<QueueOutcome>, OperationError> {
    if let Some(row) = load_operation_request(conn, request_id, now).await? {
        return Ok(Some(row.outcome_for(kind, intent_fingerprint)));
    }
    Ok(
        load_operation_request_by_intent(conn, kind, intent_fingerprint, now)
            .await?
            .map(|row| QueueOutcome::Replayed {
                operation_id: row.operation_id,
                audit_event_id: row.audit_event_id,
            }),
    )
}

/// The command one identity already recorded, for a caller that must answer a
/// retry *before* validating a new execution: an accepted confirmation
/// reconciles to the run it recorded even when the preview it named has expired
/// or its policy has moved since (issue #211, story 39).
pub async fn replay_recorded_command(
    pool: &SqlitePool,
    kind: &str,
    request_id: &str,
    intent_fingerprint: &str,
) -> Result<Option<QueueOutcome>, OperationError> {
    let mut conn = pool.acquire().await?;
    recorded_command(
        &mut conn,
        kind,
        request_id,
        intent_fingerprint,
        crate::auth::now_utc(),
    )
    .await
}

/// The whole decision ladder in one place, so the checks inside the queueing
/// transaction and the re-read after a lost race cannot drift: an already
/// recorded identity first, then the destructive-kind exclusion that makes one
/// confirmation authorize one release.
async fn recorded_outcome(
    conn: &mut sqlx::SqliteConnection,
    kind: &str,
    request_id: &str,
    intent_fingerprint: &str,
    now: time::OffsetDateTime,
) -> Result<Option<QueueOutcome>, OperationError> {
    if let Some(outcome) = recorded_command(conn, kind, request_id, intent_fingerprint, now).await?
    {
        return Ok(Some(outcome));
    }
    if OPERATION_EXCLUSIVE_KINDS.contains(&kind) {
        if let Some(operation_id) = open_operation_of_kind(conn, kind).await? {
            return Ok(Some(QueueOutcome::AlreadyOpen { operation_id }));
        }
    }
    Ok(None)
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
///
/// A cleanup a restart interrupted keeps its accounting (issue #211, story 40):
/// the persisted plan already knows how much of the confirmed work was released,
/// so the failure reports that instead of leaving the operator to guess whether
/// the deleted history came back.
pub async fn requeue_interrupted_operations(pool: &SqlitePool) -> Result<u64, OperationError> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let interrupted: Vec<String> =
        sqlx::query_scalar("SELECT params_json FROM operations WHERE status = ? AND kind = ?")
            .bind(STATUS_RUNNING)
            .bind(KIND_RETENTION_RUN)
            .fetch_all(pool)
            .await?;
    let mut counted = 0u64;
    for params_json in interrupted {
        let params: Value = serde_json::from_str(&params_json).unwrap_or(Value::Null);
        let message = crate::retention::interrupted_run_message(&params);
        counted += sqlx::query(
            "UPDATE operations SET status = ?, finished_at = ?, errors_json = ? WHERE status = ? AND kind = ? AND params_json = ?",
        )
        .bind(STATUS_FAILED)
        .bind(&now)
        .bind(
            serde_json::json!([{ "code": "interrupted_by_restart", "message": message }])
                .to_string(),
        )
        .bind(STATUS_RUNNING)
        .bind(KIND_RETENTION_RUN)
        .bind(&params_json)
        .execute(pool)
        .await?
        .rows_affected();
    }
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
    Ok(counted + result.rows_affected())
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
        // ADR 0008 retired in-process creation. Cancellation still wins: an
        // in-flight row cancelled mid-step is finalized Cancelled, never
        // overwritten. Any other surviving row (a queued historical
        // `backup_create`) fails loudly instead of creating an artifact inside
        // the serving process.
        KIND_BACKUP_CREATE => {
            if crate::operations::is_cancel_requested(state, &operation_id).await? {
                crate::operations::finalize(
                    state,
                    &operation_id,
                    STATUS_CANCELLED,
                    None,
                    &["backups"],
                )
                .await?;
            } else {
                crate::operations::add_error(
                    state,
                    &operation_id,
                    "backup_create_retired",
                    "In-process backup creation was retired (ADR 0008); run `platpulse-server backup` in an offline backup window",
                )
                .await?;
                crate::operations::finalize(
                    state,
                    &operation_id,
                    STATUS_FAILED,
                    None,
                    &["backups"],
                )
                .await?;
            }
        }
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

    async fn fresh_database() -> (tempfile::TempDir, crate::database::ServerDatabase) {
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
        (dir, database)
    }

    /// The ledger's promise at the seam its callers use: one confirmation
    /// queues one Operation, whatever the browser repeats.
    #[tokio::test]
    async fn create_operation_once_replays_the_recorded_command_and_refuses_a_reused_identity() {
        let (_dir, database) = fresh_database().await;
        async fn queue(
            pool: &sqlx::SqlitePool,
            request_id: &str,
            fingerprint: &str,
        ) -> QueueOutcome {
            create_operation_once(
                pool,
                KIND_DOCTOR_RUN,
                &serde_json::json!({ "target": "node-a" }),
                request_id,
                fingerprint,
                "owner",
                "doctor_started",
            )
            .await
            .unwrap()
        }
        let (operation_id, audit_event_id) =
            match queue(database.pool(), "req-1", "doctor_run:rp-1").await {
                QueueOutcome::Queued {
                    operation_id,
                    audit_event_id,
                } => (operation_id, audit_event_id),
                other => panic!("expected a queued command, got {other:?}"),
            };
        // The same browser id is the same command.
        match queue(database.pool(), "req-1", "doctor_run:rp-1").await {
            QueueOutcome::Replayed {
                operation_id: replayed,
                audit_event_id: replayed_audit,
            } => {
                assert_eq!(replayed, operation_id);
                assert_eq!(replayed_audit, audit_event_id);
            }
            other => panic!("expected a replay, got {other:?}"),
        }
        // Another tab's id for the same intent is still the same command.
        match queue(database.pool(), "req-2", "doctor_run:rp-1").await {
            QueueOutcome::Replayed {
                operation_id: replayed,
                ..
            } => assert_eq!(replayed, operation_id),
            other => panic!("expected a replay, got {other:?}"),
        }
        // The same id meaning something else is a conflict, never a second run.
        match queue(database.pool(), "req-1", "doctor_run:rp-2").await {
            QueueOutcome::RequestConflict {
                operation_id: conflict,
            } => assert_eq!(conflict, operation_id),
            other => panic!("expected a conflict, got {other:?}"),
        }
        // A different intent under a fresh id is a genuinely new command.
        match queue(database.pool(), "req-3", "doctor_run:rp-2").await {
            QueueOutcome::Queued { .. } => {}
            other => panic!("expected a queued command, got {other:?}"),
        }
        let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operations")
            .fetch_one(database.pool())
            .await
            .unwrap();
        assert_eq!(queued, 2);
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'doctor_started'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(
            audits, 2,
            "a replay must not record a second command in Audit"
        );
    }

    /// A closed replay window is not a replayed command: once the TTL has
    /// passed, the same identity starts a new command instead of resurrecting a
    /// stale one, and the expired row never blocks it.
    #[tokio::test]
    async fn an_expired_command_identity_is_not_replayed() {
        let (_dir, database) = fresh_database().await;
        let (first, _) = match create_operation_once(
            database.pool(),
            KIND_DOCTOR_RUN,
            &serde_json::json!({ "target": "node-a" }),
            "req-1",
            "doctor_run:rp-1",
            "owner",
            "doctor_started",
        )
        .await
        .unwrap()
        {
            QueueOutcome::Queued {
                operation_id,
                audit_event_id,
            } => (operation_id, audit_event_id),
            other => panic!("expected a queued command, got {other:?}"),
        };
        sqlx::query(
            "UPDATE operation_requests SET expires_at = '2000-01-01T00:00:00Z' WHERE request_id = 'req-1'",
        )
        .execute(database.pool())
        .await
        .unwrap();
        let second = match create_operation_once(
            database.pool(),
            KIND_DOCTOR_RUN,
            &serde_json::json!({ "target": "node-a" }),
            "req-1",
            "doctor_run:rp-1",
            "owner",
            "doctor_started",
        )
        .await
        .unwrap()
        {
            QueueOutcome::Queued { operation_id, .. } => operation_id,
            other => panic!("expected a fresh command, got {other:?}"),
        };
        assert_ne!(second, first);
        let recorded: String = sqlx::query_scalar(
            "SELECT operation_id FROM operation_requests WHERE request_id = 'req-1'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(recorded, second);
        let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operation_requests")
            .fetch_one(database.pool())
            .await
            .unwrap();
        assert_eq!(queued, 1, "the expired row is pruned, not accumulated");
    }

    /// The schema itself refuses a second open run of the one kind whose
    /// duplicate would release production history twice, so a writer that
    /// slipped past the caller's check still cannot double a cleanup.
    #[tokio::test]
    async fn the_schema_refuses_two_open_runs_of_a_destructive_kind() {
        let (_dir, database) = fresh_database().await;
        let pool = database.pool();
        let insert = "INSERT INTO operations (operation_id, kind, status, progress_percent, params_json, warnings_json, errors_json, created_at) VALUES (?, ?, ?, 0, '{}', '[]', '[]', '2026-01-01T00:00:00Z')";
        sqlx::query(insert)
            .bind("op-1")
            .bind(KIND_RETENTION_RUN)
            .bind(STATUS_QUEUED)
            .execute(pool)
            .await
            .unwrap();
        assert!(
            sqlx::query(insert)
                .bind("op-2")
                .bind(KIND_RETENTION_RUN)
                .bind(STATUS_QUEUED)
                .execute(pool)
                .await
                .is_err(),
            "a second queued retention run must be refused"
        );
        assert!(
            sqlx::query(insert)
                .bind("op-3")
                .bind(KIND_RETENTION_RUN)
                .bind(STATUS_RUNNING)
                .execute(pool)
                .await
                .is_err(),
            "a running retention run is still an open one"
        );
        // Other kinds are unaffected: the guard is scoped to the cleanup whose
        // duplicate is destructive.
        sqlx::query(insert)
            .bind("op-4")
            .bind(KIND_DOCTOR_RUN)
            .bind(STATUS_QUEUED)
            .execute(pool)
            .await
            .unwrap();
        // Once the open run is terminal, the next run may queue.
        sqlx::query("UPDATE operations SET status = ? WHERE operation_id = 'op-1'")
            .bind(STATUS_CANCELLED)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(insert)
            .bind("op-5")
            .bind(KIND_RETENTION_RUN)
            .bind(STATUS_QUEUED)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_restart_reports_what_an_interrupted_cleanup_already_released() {
        let (_dir, database) = fresh_database().await;
        let pool = database.pool();
        let plan = serde_json::json!([
            {
                "family": "raw_block_summary",
                "table": "block_summaries",
                "cutoff": "2020-01-01T00:00:00Z",
                "total": 10,
                "deleted": 4
            },
            {
                "family": "raw_block_summary",
                "table": "block_summaries",
                "cutoff": "2020-01-01T00:00:00Z",
                "total": 6,
                "deleted": 0,
                "done": true
            }
        ]);
        let params = serde_json::json!({ "previewId": "preview-1", "plan": plan });
        let insert = "INSERT INTO operations (operation_id, kind, status, params_json, warnings_json, errors_json, created_at) VALUES (?, ?, ?, ?, '[]', '[]', '2026-01-01T00:00:00Z')";
        sqlx::query(insert)
            .bind("interrupted-run")
            .bind(KIND_RETENTION_RUN)
            .bind(STATUS_RUNNING)
            .bind(params.to_string())
            .execute(pool)
            .await
            .unwrap();
        // Another kind was running as well, so the generic interruption is still
        // reported for it.
        sqlx::query(insert)
            .bind("interrupted-doctor")
            .bind(KIND_DOCTOR_RUN)
            .bind(STATUS_RUNNING)
            .bind("{}")
            .execute(pool)
            .await
            .unwrap();

        assert_eq!(requeue_interrupted_operations(pool).await.unwrap(), 2);

        let (status, errors): (String, String) = sqlx::query_as(
            "SELECT status, errors_json FROM operations WHERE operation_id = 'interrupted-run'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(status, STATUS_FAILED);
        let errors: Value = serde_json::from_str(&errors).unwrap();
        assert_eq!(errors[0]["code"], "interrupted_by_restart");
        let message = errors[0]["message"].as_str().unwrap();
        assert!(
            message.contains("4 rows were already released"),
            "released work must survive the restart report: {message}"
        );
        assert!(
            message.contains("1 planned targets were not completed"),
            "abandoned work must be named: {message}"
        );
        let doctor: Value = serde_json::from_str(
            &sqlx::query_scalar::<_, String>(
                "SELECT errors_json FROM operations WHERE operation_id = 'interrupted-doctor'",
            )
            .fetch_one(pool)
            .await
            .unwrap(),
        )
        .unwrap();
        assert!(
            doctor[0]["message"]
                .as_str()
                .unwrap()
                .contains("review state and re-run")
        );
    }
}
