//! Owner-only Operations, retention, backup, and Doctor surfaces
//! (issue #50, design §11.3/§20.1/§20.3, webui.md §4.4/§4.5/§5.5/§8.4).
//!
//! Every mutation returns immediately with an Operation reference and an
//! Audit Event link. REST is authoritative and the operation queue is
//! durable: state and history survive navigation, browser close, or SSE
//! loss; SSE invalidations only accelerate refetch. Retention is
//! safety-bounded and batched; backups expose sanitized metadata only;
//! Doctor is read-only and never auto-fixes.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::http::admin::{mutation_error, mutation_guard};
use crate::http::{ApiErrorBody, AppState, AuthenticatedSession, RequestId};
use crate::operations;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationSummary {
    pub operation_id: String,
    pub kind: String,
    pub status: String,
    pub progress_percent: i64,
    pub progress_label: Option<String>,
    pub request_id: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub audit_event_id: Option<i64>,
    pub cancel_requested: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationIssue {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationDetail {
    pub operation: OperationSummary,
    pub warnings: Vec<OperationIssue>,
    pub errors: Vec<OperationIssue>,
    pub result: Option<Value>,
    /// `true` while the Operation is queued or running (cancel is allowed).
    pub cancellable: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationMutationResponse {
    pub operation: OperationDetail,
    pub audit_event_id: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPolicyDto {
    pub family: String,
    pub label: String,
    pub retention_days: i64,
    pub min_days: i64,
    pub max_days: i64,
    pub default_days: i64,
    pub supported: bool,
    pub enabled: bool,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetentionOverview {
    pub policies: Vec<RetentionPolicyDto>,
    pub protected_state: Vec<String>,
    pub last_run: Option<OperationSummary>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetentionImpactRequest {
    pub family: String,
    pub retention_days: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetentionImpact {
    pub family: String,
    pub retention_days: i64,
    /// Rows that would be removed; `None` for unsupported families.
    pub estimated_rows: Option<i64>,
    pub unsupported: bool,
    pub bounds: RetentionBounds,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetentionBounds {
    pub min_days: i64,
    pub max_days: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPolicyUpdateRequest {
    pub retention_days: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPolicyMutationResponse {
    pub policy: RetentionPolicyDto,
    pub audit_event_id: i64,
}

/// The MVP global Block History window. This is backed by the raw Block
/// Summary retention policy; the dedicated DTO keeps the public Admin seam
/// aligned with the product contract instead of exposing retention families.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryWindowResponse {
    pub window_days: i64,
    pub default_days: i64,
    pub min_days: i64,
    pub max_days: i64,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryWindowUpdateRequest {
    pub window_days: i64,
    pub confirmed: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryWindowImpactRequest {
    pub window_days: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryWindowImpact {
    pub window_days: i64,
    pub estimated_rows: Option<i64>,
    pub min_days: i64,
    pub max_days: i64,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryWindowMutationResponse {
    pub window: HistoryWindowResponse,
    pub audit_event_id: i64,
}

fn history_window_response(policy: crate::retention::PolicyRow) -> HistoryWindowResponse {
    let catalog = crate::retention::catalog_family(crate::retention::FAMILY_RAW_BLOCK_SUMMARY)
        .expect("raw Block Summary retention policy is catalogued");
    HistoryWindowResponse {
        window_days: policy.retention_days,
        default_days: catalog.default_days,
        min_days: policy.min_days,
        max_days: policy.max_days,
        updated_at: policy.updated_at,
        updated_by: policy.updated_by,
    }
}

/// Read the single global Block History window used by Home and Admin.
#[utoipa::path(
    get,
    path = "/api/admin/v1/history-window",
    tag = "admin",
    responses((status = 200, body = HistoryWindowResponse), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn history_window(
    State(state): State<AppState>,
    Extension(_principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if crate::retention::ensure_seeded(state.db().pool())
        .await
        .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    match crate::retention::list_policies(state.db().pool()).await {
        Ok(policies) => policies
            .into_iter()
            .find(|policy| policy.family == crate::retention::FAMILY_RAW_BLOCK_SUMMARY)
            .map(history_window_response)
            .map(|window| Json(window).into_response())
            .unwrap_or_else(|| {
                mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "history window is not configured",
                )
            }),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        ),
    }
}

/// Preview the bounded cleanup consequence for a proposed global window.
#[utoipa::path(
    post,
    path = "/api/admin/v1/history-window/impact",
    tag = "admin",
    request_body = HistoryWindowImpactRequest,
    responses((status = 200, body = HistoryWindowImpact), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn history_window_impact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    Json(request): Json<HistoryWindowImpactRequest>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, true) {
        return response;
    }
    let family = crate::retention::FAMILY_RAW_BLOCK_SUMMARY;
    if let Err(message) = crate::retention::validate_policy_days(family, request.window_days) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody::with_fields_message(
                "history_window_out_of_bounds",
                message,
                &request_id.0,
                vec!["windowDays".to_owned()],
            )),
        )
            .into_response();
    }
    let estimated_rows = match crate::retention::estimate_impact(
        state.db().pool(),
        family,
        request.window_days,
        crate::auth::now_utc(),
    )
    .await
    {
        Ok((rows, unsupported)) => (!unsupported).then_some(rows),
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let catalog =
        crate::retention::catalog_family(family).expect("raw Block Summary is catalogued");
    Json(HistoryWindowImpact {
        window_days: request.window_days,
        estimated_rows,
        min_days: catalog.min_days,
        max_days: catalog.max_days,
        notes: vec![
            "shortening removes expired Block Summaries asynchronously".to_owned(),
            "lengthening cannot recover deleted or missed history".to_owned(),
        ],
    })
    .into_response()
}

/// Update the global Block History window. Confirmation is explicit and the
/// existing retention transaction supplies atomic old/new Audit state.
#[utoipa::path(
    put,
    path = "/api/admin/v1/history-window",
    tag = "admin",
    request_body = HistoryWindowUpdateRequest,
    responses((status = 200, body = HistoryWindowMutationResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn update_history_window(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    Json(request): Json<HistoryWindowUpdateRequest>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, true) {
        return response;
    }
    if crate::retention::ensure_seeded(state.db().pool())
        .await
        .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    if !request.confirmed {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody::with_fields(
                "confirmation_required",
                "changing the history window requires confirmation",
                &request_id.0,
                vec!["confirmed".to_owned()],
            )),
        )
            .into_response();
    }
    let family = crate::retention::FAMILY_RAW_BLOCK_SUMMARY;
    if let Err(message) = crate::retention::validate_policy_days(family, request.window_days) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody::with_fields_message(
                "history_window_out_of_bounds",
                message,
                &request_id.0,
                vec!["windowDays".to_owned()],
            )),
        )
            .into_response();
    }
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let before_days = match sqlx::query_scalar::<_, i64>(
        "SELECT retention_days FROM retention_policies WHERE family = ?",
    )
    .bind(family)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(Some(days)) => days,
        Ok(None) => {
            let _ = tx.rollback().await;
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "history window is not configured",
            );
        }
        Err(_) => {
            let _ = tx.rollback().await;
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    if sqlx::query(
        "UPDATE retention_policies SET retention_days = ?, enabled = 1, updated_at = ?, updated_by = ? WHERE family = ?",
    )
    .bind(request.window_days)
    .bind(&now)
    .bind(&principal.0.user_id)
    .bind(family)
    .execute(&mut *tx)
    .await
    .is_err()
    {
        let _ = tx.rollback().await;
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    if crate::auth::insert_audit_change(
        &mut *tx,
        Some(&principal.0.user_id),
        "history_window_updated",
        "history_window",
        "global",
        Some(&serde_json::json!({ "windowDays": before_days })),
        Some(&serde_json::json!({ "windowDays": request.window_days })),
    )
    .await
    .is_err()
    {
        let _ = tx.rollback().await;
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    let audit_event_id = match sqlx::query_scalar::<_, i64>("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
            let _ = tx.rollback().await;
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if tx.commit().await.is_err() {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    state
        .admin_realtime()
        .publish("retention", None::<String>, audit_event_id as u64);
    state
        .public_realtime()
        .publish("collection", None::<String>, audit_event_id as u64);
    match crate::retention::list_policies(state.db().pool()).await {
        Ok(policies) => policies
            .into_iter()
            .find(|policy| policy.family == family)
            .map(|policy| {
                Json(HistoryWindowMutationResponse {
                    window: history_window_response(policy),
                    audit_event_id,
                })
                .into_response()
            })
            .unwrap_or_else(|| {
                mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "history window is not configured",
                )
            }),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        ),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetentionRunRequest {
    /// Restrict the run to these families; absent means every enabled and
    /// supported policy.
    pub families: Option<Vec<String>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackupArtifactSummary {
    pub artifact_id: String,
    pub filename: String,
    pub bytes: i64,
    pub sha256: String,
    pub schema_version: i64,
    pub server_version: String,
    pub created_at: String,
    pub data_range_min: Option<String>,
    pub data_range_max: Option<String>,
    pub verification: String,
    pub verified_at: Option<String>,
    pub create_operation_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackupArtifactDetail {
    pub artifact: BackupArtifactSummary,
    pub verification_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheckDto {
    pub check_id: String,
    pub label: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DoctorOverview {
    pub last_run: Option<OperationSummary>,
    pub checks: Vec<DoctorCheckDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestoreValidateRequest {
    pub artifact_id: String,
}

/// Read-only Restore validation outcome (issue #51, design §20.2): the
/// Server re-verifies the artifact file against its recorded manifest
/// before any confirmation. Checksum and integrity are re-computed from
/// the file; schema compatibility compares the artifact schema with the
/// current Server schema (higher unsupported schemas are refused). Checks
/// run in order and short-circuit, so `None` means the check was not
/// reached (the first failed check is reported in `error`).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestoreValidation {
    pub artifact_id: String,
    pub filename: String,
    pub bytes: i64,
    pub schema_version: i64,
    pub server_version: String,
    pub created_at: String,
    /// `true` when the file checksum matches the manifest; `None` when the
    /// checksum check was not reached.
    pub checksum_ok: Option<bool>,
    /// `true` when the full integrity check passed; `None` when it was not
    /// reached.
    pub integrity_ok: Option<bool>,
    /// `true` when the schema is supported; `None` when it was not
    /// reached.
    pub schema_compatible: Option<bool>,
    pub current_schema_version: i64,
    /// Typed validation failure code when any check fails; `None` when the
    /// artifact is restorable.
    pub error: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestoreSubmitRequest {
    pub artifact_id: String,
    /// Typed confirmation: must equal the artifact file base name.
    pub confirmation: String,
}

#[derive(Debug, Deserialize)]
pub struct OperationsQuery {
    pub status: Option<String>,
    pub kind: Option<String>,
    pub limit: Option<i64>,
}

const OPERATION_STATUSES: [&str; 6] = [
    "queued",
    "running",
    "succeeded",
    "succeeded_with_warnings",
    "failed",
    "cancelled",
];
const OPERATION_KINDS: [&str; 5] = [
    "retention_run",
    "backup_create",
    "backup_verify",
    "doctor_run",
    "restore",
];

/// One `operations` row projected to the REST summary shape.
type OperationRow = (
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    i64,
);
fn operation_summary_row(
    (
        operation_id,
        kind,
        status,
        progress_percent,
        progress_label,
        request_id,
        created_at,
        started_at,
        finished_at,
        audit_event_id,
        cancel_requested,
    ): OperationRow,
) -> OperationSummary {
    OperationSummary {
        operation_id,
        kind,
        status,
        progress_percent,
        progress_label,
        request_id,
        created_at,
        started_at,
        finished_at,
        audit_event_id,
        cancel_requested: cancel_requested == 1,
    }
}

async fn load_operation_summary(
    pool: &sqlx::SqlitePool,
    operation_id: &str,
) -> Result<Option<OperationSummary>, sqlx::Error> {
    let row = sqlx::query_as::<_, OperationRow>(
        "SELECT operation_id, kind, status, progress_percent, progress_label, request_id, created_at, started_at, finished_at, audit_event_id, cancel_requested FROM operations WHERE operation_id = ?",
    )
    .bind(operation_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(operation_summary_row))
}

async fn load_operation_detail(
    pool: &sqlx::SqlitePool,
    operation_id: &str,
) -> Result<Option<OperationDetail>, sqlx::Error> {
    let Some(summary) = load_operation_summary(pool, operation_id).await? else {
        return Ok(None);
    };
    let (warnings, errors, result): (String, String, Option<String>) = sqlx::query_as(
        "SELECT warnings_json, errors_json, result_json FROM operations WHERE operation_id = ?",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await?;
    let cancellable = matches!(summary.status.as_str(), "queued" | "running");
    let parse_issues = |text: &str| -> Vec<OperationIssue> {
        serde_json::from_str::<Vec<OperationIssue>>(text)
            .unwrap_or_default()
            .into_iter()
            .map(|mut issue| {
                issue.code = crate::redaction::redact_sensitive(&issue.code);
                issue.message = crate::redaction::redact_sensitive(&issue.message);
                issue
            })
            .collect()
    };
    Ok(Some(OperationDetail {
        warnings: parse_issues(&warnings),
        errors: parse_issues(&errors),
        result: result
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|value| crate::redaction::redact_json_value(&value)),
        cancellable,
        operation: summary,
    }))
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

/// List durable Operations with optional status/kind filters (newest
/// first, bounded page size).
#[utoipa::path(
    get,
    path = "/api/admin/v1/operations",
    tag = "admin",
    params(
        ("status" = Option<String>, Query, description = "Filter by Operation status"),
        ("kind" = Option<String>, Query, description = "Filter by Operation kind"),
        ("limit" = Option<i64>, Query, description = "Page size (1-200, default 50)"),
    ),
    responses((status = 200, body = Vec<OperationSummary>), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn operations_list(
    State(state): State<AppState>,
    Query(params): Query<OperationsQuery>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Some(status) = &params.status {
        if !OPERATION_STATUSES.contains(&status.as_str()) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiErrorBody::with_fields(
                    "invalid_query",
                    "invalid `status` filter",
                    &request_id.0,
                    vec!["status".to_owned()],
                )),
            )
                .into_response();
        }
    }
    if let Some(kind) = &params.kind {
        if !OPERATION_KINDS.contains(&kind.as_str()) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiErrorBody::with_fields(
                    "invalid_query",
                    "invalid `kind` filter",
                    &request_id.0,
                    vec!["kind".to_owned()],
                )),
            )
                .into_response();
        }
    }
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let rows = sqlx::query_as::<_, OperationRow>(
        "SELECT operation_id, kind, status, progress_percent, progress_label, request_id, created_at, started_at, finished_at, audit_event_id, cancel_requested FROM operations WHERE (?1 IS NULL OR status = ?1) AND (?2 IS NULL OR kind = ?2) ORDER BY created_at DESC, operation_id DESC LIMIT ?3",
    )
    .bind(params.status.as_deref())
    .bind(params.kind.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await;
    let items = match rows {
        Ok(rows) => rows
            .into_iter()
            .map(operation_summary_row)
            .collect::<Vec<_>>(),
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    Json(items).into_response()
}

/// Operation detail: progress, warnings, errors, result summary, request
/// ID, and the creating Audit Event link.
#[utoipa::path(
    get,
    path = "/api/admin/v1/operations/{operation_id}",
    tag = "admin",
    responses((status = 200, body = OperationDetail), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn operation_detail(
    State(state): State<AppState>,
    Path(operation_id): Path<String>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match load_operation_detail(state.db().pool(), &operation_id).await {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody::with_fields(
                "operation_not_found",
                "unknown Operation",
                &request_id.0,
                vec!["operationId".to_owned()],
            )),
        )
            .into_response(),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        ),
    }
}

/// Cancel a queued or running Operation. Queued Operations cancel
/// immediately; running Operations set the cancel flag and stop at the
/// next bounded batch. Terminal Operations cannot be cancelled.
#[utoipa::path(
    post,
    path = "/api/admin/v1/operations/{operation_id}/cancel",
    tag = "admin",
    responses((status = 200, body = OperationMutationResponse), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn cancel_operation(
    State(state): State<AppState>,
    Path(operation_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, false) {
        return response;
    }
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let current: Option<(String, i64)> =
        sqlx::query_as("SELECT status, cancel_requested FROM operations WHERE operation_id = ?")
            .bind(&operation_id)
            .fetch_optional(&mut *tx)
            .await
            .unwrap_or(None);
    let Some((status, already)) = current else {
        let _ = tx.rollback().await;
        return (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody::with_fields(
                "operation_not_found",
                "unknown Operation",
                &request_id.0,
                vec!["operationId".to_owned()],
            )),
        )
            .into_response();
    };
    if !matches!(status.as_str(), "queued" | "running") || already == 1 {
        let _ = tx.rollback().await;
        return (
            StatusCode::CONFLICT,
            Json(ApiErrorBody::with_fields(
                "operation_not_cancellable",
                "only queued or running Operations can be cancelled",
                &request_id.0,
                vec!["operationId".to_owned(), "status".to_owned()],
            )),
        )
            .into_response();
    }
    let new_status = if status == "queued" {
        operations::STATUS_CANCELLED
    } else {
        operations::STATUS_RUNNING
    };
    let finished_at = if status == "queued" { Some(&now) } else { None };
    if sqlx::query(
        "UPDATE operations SET cancel_requested = 1, status = ?, finished_at = COALESCE(finished_at, ?) WHERE operation_id = ?",
    )
    .bind(new_status)
    .bind(finished_at)
    .bind(&operation_id)
    .execute(&mut *tx)
    .await
    .is_err()
    {
        let _ = tx.rollback().await;
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    if crate::auth::insert_audit_event(
        &mut *tx,
        Some(&principal.0.user_id),
        "operation_cancelled",
        "operation",
        &operation_id,
        Some(&serde_json::json!({ "status": new_status })),
    )
    .await
    .is_err()
    {
        let _ = tx.rollback().await;
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
            let _ = tx.rollback().await;
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if tx.commit().await.is_err() {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    state
        .admin_realtime()
        .publish("operations", Some(&operation_id), 1);
    let detail = match load_operation_detail(state.db().pool(), &operation_id).await {
        Ok(Some(detail)) => detail,
        _ => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    Json(OperationMutationResponse {
        operation: detail,
        audit_event_id,
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------

/// Retention policies with their safety bounds, the protected state list,
/// and the most recent retention run.
#[utoipa::path(
    get,
    path = "/api/admin/v1/retention",
    tag = "admin",
    responses((status = 200, body = RetentionOverview), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn retention_overview(
    State(state): State<AppState>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if crate::retention::ensure_seeded(state.db().pool())
        .await
        .is_err()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    let policies = match crate::retention::list_policies(state.db().pool()).await {
        Ok(policies) => policies,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let last_run = match last_operation_of_kind(state.db().pool(), "retention_run").await {
        Ok(run) => run,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let dto = RetentionOverview {
        policies: policies
            .into_iter()
            .map(|policy| {
                let catalog = crate::retention::catalog_family(&policy.family);
                RetentionPolicyDto {
                    label: catalog
                        .map(|entry| entry.label.to_owned())
                        .unwrap_or_else(|| policy.family.clone()),
                    default_days: catalog.map(|entry| entry.default_days).unwrap_or(0),
                    family: policy.family,
                    retention_days: policy.retention_days,
                    min_days: policy.min_days,
                    max_days: policy.max_days,
                    supported: policy.supported,
                    enabled: policy.enabled,
                    updated_at: policy.updated_at,
                    updated_by: policy.updated_by,
                }
            })
            .collect(),
        protected_state: crate::retention::protected_state_notes()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        last_run,
    };
    Json(dto).into_response()
}

/// Read-only impact preview for a proposed retention value. Never writes
/// and never audits; the edit form calls this before typed confirmation.
#[utoipa::path(
    post,
    path = "/api/admin/v1/retention/impact",
    tag = "admin",
    request_body = RetentionImpactRequest,
    responses((status = 200, body = RetentionImpact), (status = 400, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn retention_impact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    Json(request): Json<RetentionImpactRequest>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, true) {
        return response;
    }
    let Some(catalog) = crate::retention::catalog_family(&request.family) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody::with_fields(
                "invalid_query",
                "unknown retention family",
                &request_id.0,
                vec!["family".to_owned()],
            )),
        )
            .into_response();
    };
    // Preview is read-only but still safety-bounded: an out-of-bounds value
    // can never be applied, so the Server refuses to estimate for it.
    if let Err(message) =
        crate::retention::validate_policy_days(&request.family, request.retention_days)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody::with_fields_message(
                "retention_out_of_bounds",
                message,
                &request_id.0,
                vec!["family".to_owned(), "retentionDays".to_owned()],
            )),
        )
            .into_response();
    }
    let unsupported = !catalog.supported;
    let mut notes = Vec::new();
    if unsupported {
        notes.push(
            "this family is not produced in the current phase; no rows can be removed".to_owned(),
        );
    }
    let (estimated_rows, unsupported) = match crate::retention::estimate_impact(
        state.db().pool(),
        &request.family,
        request.retention_days,
        crate::auth::now_utc(),
    )
    .await
    {
        Ok((rows, unsupported)) => (Some(rows), unsupported),
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    Json(RetentionImpact {
        family: request.family.clone(),
        retention_days: request.retention_days,
        estimated_rows: if unsupported { None } else { estimated_rows },
        unsupported,
        bounds: RetentionBounds {
            min_days: catalog.min_days,
            max_days: catalog.max_days,
        },
        notes,
    })
    .into_response()
}

/// Update one retention policy within its fixed safety bounds. Audited and
/// applied immediately; retention execution stays batched and protected.
#[utoipa::path(
    put,
    path = "/api/admin/v1/retention/policies/{family}",
    tag = "admin",
    request_body = RetentionPolicyUpdateRequest,
    responses((status = 200, body = RetentionPolicyMutationResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn update_retention_policy(
    State(state): State<AppState>,
    Path(family): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    Json(request): Json<RetentionPolicyUpdateRequest>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, true) {
        return response;
    }
    if crate::retention::catalog_family(&family).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody::with_fields(
                "retention_family_not_found",
                "unknown retention family",
                &request_id.0,
                vec!["family".to_owned()],
            )),
        )
            .into_response();
    }
    if let Err(message) = crate::retention::validate_policy_days(&family, request.retention_days) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody::with_fields_message(
                "retention_out_of_bounds",
                message,
                &request_id.0,
                vec!["family".to_owned(), "retentionDays".to_owned()],
            )),
        )
            .into_response();
    }
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let before: Option<(i64,)> =
        sqlx::query_as("SELECT retention_days FROM retention_policies WHERE family = ?")
            .bind(&family)
            .fetch_optional(&mut *tx)
            .await
            .unwrap_or(None);
    let Some((before_days,)) = before else {
        let _ = tx.rollback().await;
        return (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody::with_fields(
                "retention_family_not_found",
                "unknown retention family",
                &request_id.0,
                vec!["family".to_owned()],
            )),
        )
            .into_response();
    };
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    if sqlx::query(
        "UPDATE retention_policies SET retention_days = ?, enabled = 1, updated_at = ?, updated_by = ? WHERE family = ?",
    )
    .bind(request.retention_days)
    .bind(&now)
    .bind(&principal.0.user_id)
    .bind(&family)
    .execute(&mut *tx)
    .await
    .is_err()
    {
        let _ = tx.rollback().await;
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    if crate::auth::insert_audit_event(
        &mut *tx,
        Some(&principal.0.user_id),
        "retention_policy_updated",
        "retention_policy",
        &family,
        Some(&serde_json::json!({
            "beforeDays": before_days,
            "afterDays": request.retention_days,
        })),
    )
    .await
    .is_err()
    {
        let _ = tx.rollback().await;
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
            let _ = tx.rollback().await;
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if tx.commit().await.is_err() {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    state
        .admin_realtime()
        .publish("retention", None::<String>, 1);
    let policies = match crate::retention::list_policies(state.db().pool()).await {
        Ok(policies) => policies,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let Some(policy) = policies.into_iter().find(|policy| policy.family == family) else {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    };
    let catalog = crate::retention::catalog_family(&family).unwrap();
    Json(RetentionPolicyMutationResponse {
        policy: RetentionPolicyDto {
            label: catalog.label.to_owned(),
            default_days: catalog.default_days,
            family: policy.family,
            retention_days: policy.retention_days,
            min_days: policy.min_days,
            max_days: policy.max_days,
            supported: policy.supported,
            enabled: policy.enabled,
            updated_at: policy.updated_at,
            updated_by: policy.updated_by,
        },
        audit_event_id,
    })
    .into_response()
}

/// Queue a retention run. Returns immediately with the Operation
/// reference; the worker executes it in bounded batches.
#[utoipa::path(
    post,
    path = "/api/admin/v1/retention/run",
    tag = "admin",
    request_body = RetentionRunRequest,
    responses((status = 200, body = OperationMutationResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn retention_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    Json(request): Json<RetentionRunRequest>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, true) {
        return response;
    }
    if let Some(families) = &request.families {
        if families.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiErrorBody::with_fields(
                    "invalid_query",
                    "families must not be empty when provided",
                    &request_id.0,
                    vec!["families".to_owned()],
                )),
            )
                .into_response();
        }
        for family in families {
            if crate::retention::catalog_family(family).is_none() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiErrorBody::with_fields(
                        "invalid_query",
                        "unknown retention family",
                        &request_id.0,
                        vec!["families".to_owned()],
                    )),
                )
                    .into_response();
            }
        }
    }
    let params = serde_json::json!({ "families": request.families });
    queue_operation(
        &state,
        &principal,
        &request_id,
        operations::KIND_RETENTION_RUN,
        &params,
        "retention_started",
    )
    .await
}

fn redact_backup_filename(filename: String) -> String {
    crate::redaction::redact_sensitive(&filename)
}

/// List backup artifacts with sanitized metadata only (file base name,
/// size, checksum, schema, Server version, timestamps, verification).
#[utoipa::path(
    get,
    path = "/api/admin/v1/backups",
    tag = "admin",
    responses((status = 200, body = Vec<BackupArtifactSummary>), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn backups_list(
    State(state): State<AppState>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let rows = sqlx::query_as::<_, (String, String, i64, String, i64, String, String, Option<String>, Option<String>, String, Option<String>, Option<String>)>(
        "SELECT artifact_id, filename, bytes, sha256, schema_version, server_version, created_at, data_range_min, data_range_max, verification, verified_at, create_operation_id FROM backup_artifacts ORDER BY created_at DESC, artifact_id DESC",
    )
    .fetch_all(state.db().pool())
    .await;
    let items = match rows {
        Ok(rows) => rows
            .into_iter()
            .map(
                |(
                    artifact_id,
                    filename,
                    bytes,
                    sha256,
                    schema_version,
                    server_version,
                    created_at,
                    data_range_min,
                    data_range_max,
                    verification,
                    verified_at,
                    create_operation_id,
                )| {
                    BackupArtifactSummary {
                        artifact_id,
                        filename: redact_backup_filename(filename),
                        bytes,
                        sha256,
                        schema_version,
                        server_version,
                        created_at,
                        data_range_min,
                        data_range_max,
                        verification,
                        verified_at,
                        create_operation_id,
                    }
                },
            )
            .collect::<Vec<_>>(),
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    Json(items).into_response()
}

/// Backup artifact detail. Database contents are never exposed; only the
/// sanitized manifest metadata plus the verification outcome.
#[utoipa::path(
    get,
    path = "/api/admin/v1/backups/{artifact_id}",
    tag = "admin",
    responses((status = 200, body = BackupArtifactDetail), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn backup_artifact_detail(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let row = sqlx::query_as::<_, (String, String, i64, String, i64, String, String, Option<String>, Option<String>, String, Option<String>, Option<String>, Option<String>)>(
        "SELECT artifact_id, filename, bytes, sha256, schema_version, server_version, created_at, data_range_min, data_range_max, verification, verified_at, create_operation_id, verification_error FROM backup_artifacts WHERE artifact_id = ?",
    )
    .bind(&artifact_id)
    .fetch_optional(state.db().pool())
    .await;
    match row {
        Ok(Some((
            artifact_id,
            filename,
            bytes,
            sha256,
            schema_version,
            server_version,
            created_at,
            data_range_min,
            data_range_max,
            verification,
            verified_at,
            create_operation_id,
            verification_error,
        ))) => Json(BackupArtifactDetail {
            artifact: BackupArtifactSummary {
                artifact_id,
                filename: redact_backup_filename(filename),
                bytes,
                sha256,
                schema_version,
                server_version,
                created_at,
                data_range_min,
                data_range_max,
                verification,
                verified_at,
                create_operation_id,
            },
            verification_error: verification_error
                .map(|message| crate::redaction::redact_sensitive(&message)),
        })
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody::with_fields(
                "backup_artifact_not_found",
                "unknown backup artifact",
                &request_id.0,
                vec!["artifactId".to_owned()],
            )),
        )
            .into_response(),
        Err(_) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        ),
    }
}

/// Queue a backup creation. Returns immediately with the Operation
/// reference; the artifact lands in the configured backup directory.
#[utoipa::path(
    post,
    path = "/api/admin/v1/backups",
    tag = "admin",
    responses((status = 200, body = OperationMutationResponse), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn backup_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, false) {
        return response;
    }
    queue_operation(
        &state,
        &principal,
        &request_id,
        operations::KIND_BACKUP_CREATE,
        &serde_json::json!({}),
        "backup_started",
    )
    .await
}

/// Queue a backup verification (checksum, read-only integrity, schema).
/// A failed verification never deletes the artifact or any previous one.
#[utoipa::path(
    post,
    path = "/api/admin/v1/backups/{artifact_id}/verify",
    tag = "admin",
    responses((status = 200, body = OperationMutationResponse), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn backup_verify(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, false) {
        return response;
    }
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT artifact_id FROM backup_artifacts WHERE artifact_id = ?")
            .bind(&artifact_id)
            .fetch_optional(state.db().pool())
            .await
            .unwrap_or(None);
    if exists.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody::with_fields(
                "backup_artifact_not_found",
                "unknown backup artifact",
                &request_id.0,
                vec!["artifactId".to_owned()],
            )),
        )
            .into_response();
    }
    queue_operation(
        &state,
        &principal,
        &request_id,
        operations::KIND_BACKUP_VERIFY,
        &serde_json::json!({ "artifactId": artifact_id }),
        "backup_verify_started",
    )
    .await
}

// ---------------------------------------------------------------------------
// Restore (issue #51, design §20.2, webui.md §8.4)
// ---------------------------------------------------------------------------

/// Read-only Restore validation: identity selection plus fresh checksum,
/// integrity, and schema-compatibility verification of the artifact file.
/// Never writes and never audits; the restore page calls this before the
/// typed confirmation. Validation outcomes are part of the 200 response;
/// HTTP errors are reserved for unknown artifacts and missing capability.
#[utoipa::path(
    post,
    path = "/api/admin/v1/restore/validate",
    tag = "admin",
    request_body = RestoreValidateRequest,
    responses((status = 200, body = RestoreValidation), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn restore_validate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    Json(request): Json<RestoreValidateRequest>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, true) {
        return response;
    }
    let identity =
        match crate::restore::load_identity(state.db().pool(), &request.artifact_id).await {
            Ok(Some(identity)) => identity,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ApiErrorBody::with_fields(
                        "restore_artifact_not_found",
                        "unknown backup artifact",
                        &request_id.0,
                        vec!["artifactId".to_owned()],
                    )),
                )
                    .into_response();
            }
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
    let Some(backup_dir) = state.backup_dir().map(|path| path.to_path_buf()) else {
        return mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "restore_unavailable",
            "no backup directory is configured; set backup_dir in server.toml",
        );
    };
    let outcome = match crate::restore::validated_artifact_path(&backup_dir, &identity.filename) {
        Ok(path) => {
            crate::restore::check_artifact(
                &path,
                &identity.sha256,
                identity.schema_version,
                crate::database::SERVER_SCHEMA_VERSION,
            )
            .await
        }
        Err(error) => Err(error),
    };
    let (error, message) = match outcome {
        Ok(()) => (None, None),
        Err(crate::restore::RestoreError::Domain { code, message }) => {
            (Some(code.to_owned()), Some(message))
        }
        Err(other) => (
            Some(crate::restore::ERROR_IO.to_owned()),
            Some(crate::redaction::redact_sensitive(&other.to_string())),
        ),
    };
    // Checks run in order and short-circuit: a check that was never reached
    // is reported as `None`, never as a passing result.
    let (checksum_ok, integrity_ok, schema_compatible) = match error.as_deref() {
        None => (Some(true), Some(true), Some(true)),
        Some(crate::restore::ERROR_CHECKSUM) => (Some(false), None, None),
        Some(crate::restore::ERROR_INTEGRITY) => (Some(true), Some(false), None),
        Some(crate::restore::ERROR_SCHEMA) => (Some(true), Some(true), Some(false)),
        _ => (None, None, None),
    };
    Json(RestoreValidation {
        artifact_id: identity.artifact_id,
        filename: redact_backup_filename(identity.filename),
        bytes: identity.bytes,
        schema_version: identity.schema_version,
        server_version: identity.server_version,
        created_at: identity.created_at,
        checksum_ok,
        integrity_ok,
        schema_compatible,
        current_schema_version: crate::database::SERVER_SCHEMA_VERSION,
        error,
        message: message.map(|value| crate::redaction::redact_sensitive(&value)),
    })
    .into_response()
}

/// Queue the highest-risk Restore Operation. The workflow requires backup
/// identity selection, checksum/schema validation (see `restore/validate`),
/// and this typed confirmation (the artifact file base name). The worker
/// re-validates everything and then refuses while the Server is running
/// (`restore_requires_stopped_server`): the current database stays
/// authoritative and the failure is recoverable through REST.
#[utoipa::path(
    post,
    path = "/api/admin/v1/restore",
    tag = "admin",
    request_body = RestoreSubmitRequest,
    responses((status = 200, body = OperationMutationResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn restore_submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    Json(request): Json<RestoreSubmitRequest>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, true) {
        return response;
    }
    let identity =
        match crate::restore::load_identity(state.db().pool(), &request.artifact_id).await {
            Ok(Some(identity)) => identity,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ApiErrorBody::with_fields(
                        "restore_artifact_not_found",
                        "unknown backup artifact",
                        &request_id.0,
                        vec!["artifactId".to_owned()],
                    )),
                )
                    .into_response();
            }
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
    if !crate::restore::confirmation_matches(&request.confirmation, &identity.filename) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody::with_fields_message(
                "restore_confirmation_mismatch",
                "the typed confirmation must equal the backup file name".to_owned(),
                &request_id.0,
                vec!["artifactId".to_owned(), "confirmation".to_owned()],
            )),
        )
            .into_response();
    }
    queue_operation(
        &state,
        &principal,
        &request_id,
        operations::KIND_RESTORE,
        &serde_json::json!({
            "artifactId": request.artifact_id,
            "confirmation": request.confirmation,
        }),
        "restore_started",
    )
    .await
}

// ---------------------------------------------------------------------------
// Doctor
// ---------------------------------------------------------------------------

/// The most recent read-only Doctor report (previous diagnostic results
/// survive failed runs) and its checks.
#[utoipa::path(
    get,
    path = "/api/admin/v1/doctor",
    tag = "admin",
    responses((status = 200, body = DoctorOverview), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn doctor_overview(
    State(state): State<AppState>,
    Extension(_session): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let last_run = match crate::doctor::last_run(&state).await {
        Ok(run) => run,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let (summary, checks) = match last_run {
        Some((operation_id, status, result)) => {
            let summary = match load_operation_summary(state.db().pool(), &operation_id).await {
                Ok(Some(summary)) => summary,
                _ => {
                    return mutation_error(
                        &request_id.0,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "Server database is unavailable",
                    );
                }
            };
            let _ = status;
            let checks: Vec<DoctorCheckDto> = crate::doctor::checks_from_result(result.as_deref())
                .into_iter()
                .filter_map(|check| serde_json::from_value(check).ok())
                .collect();
            (Some(summary), checks)
        }
        None => (None, Vec::new()),
    };
    Json(DoctorOverview {
        last_run: summary,
        checks,
    })
    .into_response()
}

/// Queue a read-only Doctor run. Doctor never auto-fixes, deletes,
/// migrates, or rotates secrets.
#[utoipa::path(
    post,
    path = "/api/admin/v1/doctor",
    tag = "admin",
    responses((status = 200, body = OperationMutationResponse), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn doctor_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Some(response) = mutation_guard(&headers, &principal, state.auth(), &request_id, false) {
        return response;
    }
    queue_operation(
        &state,
        &principal,
        &request_id,
        operations::KIND_DOCTOR_RUN,
        &serde_json::json!({}),
        "doctor_started",
    )
    .await
}

// ---------------------------------------------------------------------------
// Shared mutation plumbing
// ---------------------------------------------------------------------------

async fn queue_operation(
    state: &AppState,
    principal: &AuthenticatedSession,
    request_id: &RequestId,
    kind: &str,
    params: &Value,
    audit_event_kind: &str,
) -> Response {
    let (operation_id, audit_event_id) = match operations::create_operation(
        state.db().pool(),
        kind,
        params,
        &request_id.0,
        &principal.0.user_id,
        audit_event_kind,
    )
    .await
    {
        Ok(ids) => ids,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    state
        .admin_realtime()
        .publish("operations", Some(&operation_id), 1);
    let detail = match load_operation_detail(state.db().pool(), &operation_id).await {
        Ok(Some(detail)) => detail,
        _ => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    Json(OperationMutationResponse {
        operation: detail,
        audit_event_id,
    })
    .into_response()
}

async fn last_operation_of_kind(
    pool: &sqlx::SqlitePool,
    kind: &str,
) -> Result<Option<OperationSummary>, sqlx::Error> {
    let row = sqlx::query_as::<_, OperationRow>(
        "SELECT operation_id, kind, status, progress_percent, progress_label, request_id, created_at, started_at, finished_at, audit_event_id, cancel_requested FROM operations WHERE kind = ? ORDER BY created_at DESC, operation_id DESC LIMIT 1",
    )
    .bind(kind)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(operation_summary_row))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/operations", get(operations_list))
        .route("/operations/{operation_id}", get(operation_detail))
        .route(
            "/operations/{operation_id}/cancel",
            axum::routing::post(cancel_operation),
        )
        .route(
            "/history-window",
            get(history_window).put(update_history_window),
        )
        .route(
            "/history-window/impact",
            axum::routing::post(history_window_impact),
        )
        .route("/retention", get(retention_overview))
        .route("/retention/impact", axum::routing::post(retention_impact))
        .route(
            "/retention/policies/{family}",
            axum::routing::put(update_retention_policy),
        )
        .route("/retention/run", axum::routing::post(retention_run))
        .route("/backups", get(backups_list).post(backup_create))
        .route("/backups/{artifact_id}", get(backup_artifact_detail))
        .route(
            "/backups/{artifact_id}/verify",
            axum::routing::post(backup_verify),
        )
        .route("/restore/validate", axum::routing::post(restore_validate))
        .route("/restore", axum::routing::post(restore_submit))
        .route("/doctor", get(doctor_overview).post(doctor_run))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::extract::Extension;
    use axum::http::header;
    use serde_json::{Value, json};
    use tempfile::tempdir;

    use crate::http::AuthenticatedSession;

    async fn test_state() -> (tempfile::TempDir, AppState) {
        let dir = tempdir().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        crate::retention::ensure_seeded(database.pool())
            .await
            .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new_with_proxy_policy(
            database,
            None,
            auth,
            Vec::new(),
            None,
            crate::config::NotificationChannels::default(),
        )
        .with_backup_dir(Some(dir.path().join("backups")));
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        (dir, state)
    }

    fn session() -> AuthenticatedSession {
        AuthenticatedSession(crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: time::OffsetDateTime::now_utc(),
            last_seen_at: time::OffsetDateTime::now_utc(),
            expires_at: time::OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        })
    }

    fn mutation_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        headers.insert(header::ORIGIN, "http://127.0.0.1:8080".parse().unwrap());
        headers.insert("x-csrf-token", "csrf".parse().unwrap());
        headers
    }

    fn request_id() -> RequestId {
        RequestId(std::sync::Arc::from("req-ops-1"))
    }

    async fn body_json(response: Response) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    async fn seed_old_data(state: &AppState) {
        let pool = state.db().pool();
        // Rows are seeded relative to the real clock because the retention
        // endpoints and worker use `now_utc()` at execution time.
        let real_now = crate::auth::now_utc();
        let old = crate::auth::format_rfc3339(real_now - time::Duration::days(400));
        let now = crate::auth::format_rfc3339(real_now);
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('ops-agent', 1, ?, ?)")
            .bind(&old).bind(&old).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('ops-network', 'Ops', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&old).bind(&old).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('ops-node', 'ops-agent', 'ops-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&old).bind(&old).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ('ops-node', 1, '0xhash', '0xparent', '0xgenesis', 1, 1, 'lat', 1, ?, 2, 'subscription', '0x0000000000000000000000000000000000000000', 'unknown', 'unknown', 'test', ?)")
            .bind(&old).bind(&old).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ('ops-node', 2, '0xhash2', '0xparent2', '0xgenesis', 1, 1, 'lat', 1, ?, 2, 'subscription', '0x0000000000000000000000000000000000000000', 'unknown', 'unknown', 'test', ?)")
            .bind(&now).bind(&now).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, updated_at) VALUES ('ops-node', 2, 2, 4, 0, ?)")
            .bind(&now).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ('ops-node', 0, 2, 'covered', ?, ?)")
            .bind(&old).bind(&old).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, resolved_at, created_at) VALUES ('ops-node', 10, 20, 'server_rejected', ?, ?)")
            .bind(&old).bind(&old).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, created_at) VALUES ('ops-node', 30, 40, 'permanent_gap', ?)")
            .bind(&old).execute(pool).await.unwrap();
        // An old Incident and its notification event; incidents are immutable.
        sqlx::query("INSERT INTO alert_incidents (incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, opened_evidence_json) VALUES ('inc-old', 'node.rpc_unreachable', 1, 'node', 'ops-node', 'critical', 'open', ?, ?, '{}')")
            .bind(&old).bind(&old).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO notification_events (event_id, event_kind, incident_id, rule_key, subject_kind, subject_key, severity, summary, created_at) VALUES ('ev-old', 'incident', 'inc-old', 'node.rpc_unreachable', 'node', 'ops-node', 'critical', 'old event', ?)")
            .bind(&old).execute(pool).await.unwrap();
        // An old Audit row not referenced by any Operation (removable) and
        // one referenced by an Operation (protected).
        sqlx::query("INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, created_at) VALUES ('owner', 'test_old', 'test', 't1', ?)")
            .bind(&old).execute(pool).await.unwrap();
        let (operation_id, audit_event_id) = crate::operations::create_operation(
            pool,
            "doctor_run",
            &json!({}),
            "req-seed",
            "owner",
            "doctor_started",
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO audit_events (actor_user_id, event_kind, target_kind, target_id, created_at) VALUES ('owner', 'linked_old', 'test', 't2', ?)")
            .bind(&old).execute(pool).await.unwrap();
        sqlx::query("UPDATE operations SET audit_event_id = (SELECT MAX(audit_event_id) FROM audit_events), status = 'succeeded', finished_at = ? WHERE operation_id = ?")
            .bind(&now).bind(&operation_id).execute(pool).await.unwrap();
        let _ = audit_event_id;
    }

    #[tokio::test]
    async fn retention_overview_lists_seeded_policies_with_bounds() {
        let (_dir, state) = test_state().await;
        let response =
            retention_overview(State(state), Extension(session()), Extension(request_id())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let policies = body["policies"].as_array().unwrap();
        assert_eq!(policies.len(), crate::retention::POLICY_CATALOG.len());
        let raw = policies
            .iter()
            .find(|policy| policy["family"] == "raw_block_summary")
            .unwrap();
        assert_eq!(raw["retentionDays"], 7);
        assert_eq!(raw["minDays"], 1);
        assert_eq!(raw["maxDays"], 30);
        assert_eq!(raw["supported"], true);
        assert!(body["protectedState"].as_array().unwrap().len() >= 6);
    }

    #[tokio::test]
    async fn policy_update_within_bounds_is_audited_and_out_of_bounds_is_rejected() {
        let (_dir, state) = test_state().await;
        let response = update_retention_policy(
            State(state.clone()),
            Path("raw_block_summary".to_owned()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RetentionPolicyUpdateRequest { retention_days: 14 }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["policy"]["retentionDays"], 14);
        assert!(body["auditEventId"].as_i64().unwrap() > 0);
        let audit: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE event_kind='retention_policy_updated' AND target_id='raw_block_summary'")
                .fetch_one(state.db().pool()).await.unwrap();
        assert_eq!(audit, 1);

        let response = update_retention_policy(
            State(state),
            Path("raw_block_summary".to_owned()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RetentionPolicyUpdateRequest {
                retention_days: 999,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn history_window_is_bounded_confirmed_and_audited() {
        let (_dir, state) = test_state().await;
        let response = history_window(
            State(state.clone()),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["windowDays"], 7);
        assert_eq!(body["defaultDays"], 7);
        assert_eq!(body["minDays"], 1);
        assert_eq!(body["maxDays"], 30);

        let response = update_history_window(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(HistoryWindowUpdateRequest {
                window_days: 14,
                confirmed: true,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["window"]["windowDays"], 14);
        assert!(body["auditEventId"].as_i64().unwrap() > 0);

        let stored: i64 = sqlx::query_scalar(
            "SELECT retention_days FROM retention_policies WHERE family = 'raw_block_summary'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, 14);
        let audit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'history_window_updated' AND target_id = 'global'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit, 1);

        let response = update_history_window(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(HistoryWindowUpdateRequest {
                window_days: 31,
                confirmed: true,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = update_history_window(
            State(state),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(HistoryWindowUpdateRequest {
                window_days: 15,
                confirmed: false,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn retention_run_deletes_only_old_rows_and_preserves_protected_state() {
        let (_dir, state) = test_state().await;
        seed_old_data(&state).await;
        let pool = state.db().pool();

        // Impact preview counts exactly the old raw rows.
        let response = retention_impact(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RetentionImpactRequest {
                family: "raw_block_summary".to_owned(),
                retention_days: 30,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["estimatedRows"], 1);
        assert_eq!(body["unsupported"], false);

        // Queue a full run and execute it through the worker until idle.
        let response = retention_run(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RetentionRunRequest { families: None }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let operation_id = body["operation"]["operation"]["operationId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(body["operation"]["operation"]["status"], "queued");
        assert!(body["auditEventId"].as_i64().unwrap() > 0);

        let mut guard = 0;
        while crate::operations::process_operations(&state).await.unwrap() > 0 && guard < 200 {
            guard += 1;
        }
        let summary = load_operation_summary(pool, &operation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summary.status, "succeeded");

        // Old raw row gone; fresh row and every protected structure intact.
        let old_raw: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE block_number = 1")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(old_raw, 0);
        let fresh_raw: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE block_number = 2")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(fresh_raw, 1);
        let protected: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count FROM block_history_state WHERE node_id='ops-node'",
        )
        .fetch_one(pool).await.unwrap();
        assert_eq!(protected, (2, 2, 4, 0));
        let coverage: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM block_coverage_intervals")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(coverage, 1);
        // The resolved gap is removable under the 180d policy; the
        // permanent gap is never touched.
        let resolved: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM block_history_gaps WHERE kind='server_rejected'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(resolved, 0);
        let permanent: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM block_history_gaps WHERE kind='permanent_gap'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(permanent, 1);
        // Incidents are immutable; notification events follow their policy.
        let incidents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_incidents")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(incidents, 1);
        let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_events")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(events, 0);
        // Unreferenced old audit rows are removed; the one linked to an
        // Operation survives (plus the run's own creation and completion
        // Audit Events: doctor_started, linked_old, retention_started,
        // operation_finished).
        let audit: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(audit, 4);
        let linked: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE event_kind='linked_old'")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(linked, 1);
    }

    #[tokio::test]
    async fn backup_create_and_verify_produce_sanitized_metadata_only() {
        let (_dir, state) = test_state().await;
        seed_old_data(&state).await;
        let pool = state.db().pool();

        let response = backup_create(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let create_id = body["operation"]["operation"]["operationId"]
            .as_str()
            .unwrap()
            .to_owned();
        while crate::operations::process_operations(&state).await.unwrap() > 0 {}

        let summary = load_operation_summary(pool, &create_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summary.status, "succeeded");
        let artifact_id = sqlx::query_scalar::<_, String>(
            "SELECT artifact_id FROM backup_artifacts ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(pool)
        .await
        .unwrap();

        // The artifact file exists next to its metadata row; metadata is
        // sanitized (no paths, no contents).
        let detail = backup_artifact_detail(
            State(state.clone()),
            Path(artifact_id.clone()),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        let body = body_json(detail).await;
        assert_eq!(
            body["artifact"]["filename"],
            format!("platpulse-{artifact_id}.db")
        );
        assert_eq!(body["artifact"]["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(
            body["artifact"]["schemaVersion"],
            crate::database::SERVER_SCHEMA_VERSION
        );
        assert_eq!(body["artifact"]["verification"], "pending");
        assert!(
            !body["artifact"]["serverVersion"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        assert!(!body["artifact"]["filename"].as_str().unwrap().contains('/'));
        let file = _dir
            .path()
            .join("backups")
            .join(format!("platpulse-{artifact_id}.db"));
        assert!(file.exists());

        // Verify through the worker.
        let response = backup_verify(
            State(state.clone()),
            Path(artifact_id.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        while crate::operations::process_operations(&state).await.unwrap() > 0 {}
        let verification: (String, Option<String>) = sqlx::query_as(
            "SELECT verification, verification_error FROM backup_artifacts WHERE artifact_id = ?",
        )
        .bind(&artifact_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(verification.0, "ok");
        assert!(verification.1.is_none());

        // Corrupting the snapshot must fail verification without deleting
        // the artifact or its metadata.
        std::fs::write(&file, b"tampered").unwrap();
        let response = backup_verify(
            State(state.clone()),
            Path(artifact_id),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        while crate::operations::process_operations(&state).await.unwrap() > 0 {}
        let verification: (String, Option<String>) = sqlx::query_as(
            "SELECT verification, verification_error FROM backup_artifacts ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(pool).await.unwrap();
        assert_eq!(verification.0, "failed");
        assert!(verification.1.unwrap().contains("checksum"));
    }

    #[tokio::test]
    async fn backup_without_configured_directory_fails_honestly() {
        let dir = tempdir().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new_with_proxy_policy(
            database,
            None,
            auth,
            Vec::new(),
            None,
            crate::config::NotificationChannels::default(),
        );
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        let response = backup_create(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        while crate::operations::process_operations(&state).await.unwrap() > 0 {}
        let summary = load_operation_summary(
            state.db().pool(),
            &sqlx::query_scalar::<_, String>("SELECT operation_id FROM operations LIMIT 1")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(summary.status, "failed");
        let detail = load_operation_detail(state.db().pool(), &summary.operation_id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            detail
                .errors
                .iter()
                .any(|issue| issue.code == "backup_dir_not_configured")
        );
    }

    #[tokio::test]
    async fn doctor_run_reports_distinct_statuses_without_mutating() {
        let (_dir, state) = test_state().await;
        let pool = state.db().pool();
        let response = doctor_run(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        while crate::operations::process_operations(&state).await.unwrap() > 0 {}

        let overview = doctor_overview(
            State(state.clone()),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(overview.status(), StatusCode::OK);
        let body = body_json(overview).await;
        let checks = body["checks"].as_array().unwrap();
        assert!(checks.len() >= 8);
        let statuses: std::collections::HashSet<&str> = checks
            .iter()
            .map(|check| check["status"].as_str().unwrap())
            .collect();
        assert!(statuses.contains("pass"));
        assert!(statuses.contains("warning")); // backup dir missing at this point
        assert!(statuses.contains("not_configured")); // no notification channels
        assert!(statuses.contains("skipped")); // no backup artifact yet
        // The run itself never writes business data.
        let audits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
            .fetch_one(pool)
            .await
            .unwrap();
        // one creation audit + one completion audit
        assert_eq!(audits, 2);
        assert_eq!(body["lastRun"]["status"], "succeeded_with_warnings");
    }

    #[tokio::test]
    async fn operations_list_filters_and_cancel_terminal_conflicts() {
        let (_dir, state) = test_state().await;
        let response = doctor_run(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let operation_id = body["operation"]["operation"]["operationId"]
            .as_str()
            .unwrap()
            .to_owned();
        while crate::operations::process_operations(&state).await.unwrap() > 0 {}

        // Filtered list shows the finished doctor run.
        let response = operations_list(
            State(state.clone()),
            Query(OperationsQuery {
                status: Some("succeeded_with_warnings".to_owned()),
                kind: Some("doctor_run".to_owned()),
                limit: None,
            }),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["operationId"], operation_id);

        // Invalid filters are rejected.
        let response = operations_list(
            State(state.clone()),
            Query(OperationsQuery {
                status: Some("nonsense".to_owned()),
                kind: None,
                limit: None,
            }),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Terminal operations cannot be cancelled.
        let response = cancel_operation(
            State(state.clone()),
            Path(operation_id.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);

        // Detail exposes request id and audit link.
        let response = operation_detail(
            State(state),
            Path(operation_id),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["operation"]["requestId"], "req-ops-1");
        assert!(body["operation"]["auditEventId"].as_i64().unwrap() > 0);
        assert_eq!(body["cancellable"], false);
    }

    #[tokio::test]
    async fn queued_operation_cancels_immediately_and_is_audited() {
        let (_dir, state) = test_state().await;
        let response = retention_run(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RetentionRunRequest { families: None }),
        )
        .await;
        let body = body_json(response).await;
        let operation_id = body["operation"]["operation"]["operationId"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = cancel_operation(
            State(state.clone()),
            Path(operation_id.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["operation"]["operation"]["status"], "cancelled");
        assert!(body["auditEventId"].as_i64().unwrap() > 0);
        let audit: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE event_kind='operation_cancelled' AND target_id=?")
                .bind(&operation_id)
                .fetch_one(state.db().pool()).await.unwrap();
        assert_eq!(audit, 1);
        // The worker must not resurrect the cancelled operation.
        let processed = crate::operations::process_operations(&state).await.unwrap();
        assert_eq!(processed, 0);
        let status: String =
            sqlx::query_scalar("SELECT status FROM operations WHERE operation_id=?")
                .bind(&operation_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(status, "cancelled");
    }

    #[tokio::test]
    async fn coverage_state_stays_inspectable_while_a_run_is_running() {
        let (_dir, state) = test_state().await;
        seed_old_data(&state).await;
        let pool = state.db().pool();
        let response = retention_run(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RetentionRunRequest { families: None }),
        )
        .await;
        let body = body_json(response).await;
        let operation_id = body["operation"]["operation"]["operationId"]
            .as_str()
            .unwrap()
            .to_owned();
        // One worker step claims the operation and deletes one batch; the
        // run stays `running` with more batches to go.
        crate::operations::process_operations(&state).await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM operations WHERE operation_id=?")
                .bind(&operation_id)
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(status, "running");
        // The coverage clarification: aggregate projections and their
        // freshness/coverage state remain inspectable mid-run — high-water
        // mark, coverage interval, and counters are untouched and readable.
        let protected: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count FROM block_history_state WHERE node_id='ops-node'",
        )
        .fetch_one(pool).await.unwrap();
        assert_eq!(protected, (2, 2, 4, 0));
        let coverage: (i64, i64, String) = sqlx::query_as(
            "SELECT first_height, last_height, status FROM block_coverage_intervals WHERE node_id='ops-node'",
        )
        .fetch_one(pool).await.unwrap();
        assert_eq!(coverage, (0, 2, "covered".to_owned()));
        // Mid-run, the raw family has not been reached yet (families run in
        // catalog order); the key fact is that the protected projections
        // above stayed readable and unchanged while the run is in flight.
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE block_number = 1")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(remaining, 1);
        // Let the run finish; the protected state still holds.
        while crate::operations::process_operations(&state).await.unwrap() > 0 {}
        let protected: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count FROM block_history_state WHERE node_id='ops-node'",
        )
        .fetch_one(pool).await.unwrap();
        assert_eq!(protected, (2, 2, 4, 0));
    }

    #[tokio::test]
    async fn running_backup_and_doctor_cancellations_are_honoured() {
        let (_dir, state) = test_state().await;
        let pool = state.db().pool();
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        // Simulate the race: the operation is already running when the
        // Owner's cancellation lands (cancel_requested set mid-step). The
        // domain functions must finalize Cancelled, never overwrite it.
        sqlx::query("INSERT INTO operations (operation_id, kind, status, cancel_requested, created_at, params_json, warnings_json, errors_json) VALUES ('race-backup', 'backup_create', 'running', 1, ?, '{}', '[]', '[]')")
            .bind(&now).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO operations (operation_id, kind, status, cancel_requested, created_at, params_json, warnings_json, errors_json) VALUES ('race-doctor', 'doctor_run', 'running', 1, ?, '{}', '[]', '[]')")
            .bind(&now).execute(pool).await.unwrap();
        crate::operations::process_operations(&state).await.unwrap();
        crate::operations::process_operations(&state).await.unwrap();
        let cancelled: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM operations WHERE status='cancelled' AND operation_id IN ('race-backup', 'race-doctor')",
        )
        .fetch_one(pool).await.unwrap();
        assert_eq!(cancelled, 2);
        // A cancelled backup never leaves an artifact behind.
        let artifacts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_artifacts")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(artifacts, 0);
    }

    #[tokio::test]
    async fn running_operation_cancel_flag_stops_the_next_batch() {
        let (_dir, state) = test_state().await;
        seed_old_data(&state).await;
        let response = retention_run(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RetentionRunRequest { families: None }),
        )
        .await;
        let body = body_json(response).await;
        let operation_id = body["operation"]["operation"]["operationId"]
            .as_str()
            .unwrap()
            .to_owned();
        // Start the run (mark running + first batch).
        crate::operations::process_operations(&state).await.unwrap();
        let response = cancel_operation(
            State(state.clone()),
            Path(operation_id.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        // The next worker step honours the cancel flag.
        crate::operations::process_operations(&state).await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM operations WHERE operation_id=?")
                .bind(&operation_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(status, "cancelled");
    }

    #[tokio::test]
    async fn restore_validate_and_submit_refuse_while_the_server_runs() {
        let (_dir, state) = test_state().await;
        let pool = state.db().pool();

        // Produce a real backup artifact through the backup Operation.
        let response = backup_create(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        while crate::operations::process_operations(&state).await.unwrap() > 0 {}
        let artifact_id = sqlx::query_scalar::<_, String>(
            "SELECT artifact_id FROM backup_artifacts ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let filename = format!("platpulse-{artifact_id}.db");
        let artifact_path = _dir.path().join("backups").join(&filename);

        // Read-only validation passes for the untouched artifact.
        let response = restore_validate(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RestoreValidateRequest {
                artifact_id: artifact_id.clone(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["checksumOk"], true);
        assert_eq!(body["integrityOk"], true);
        assert_eq!(body["schemaCompatible"], true);
        assert_eq!(body["filename"], filename.as_str());
        assert_eq!(
            body["currentSchemaVersion"],
            crate::database::SERVER_SCHEMA_VERSION
        );
        assert!(body["error"].is_null());

        // Unknown identity is a typed 404.
        let response = restore_validate(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RestoreValidateRequest {
                artifact_id: "unknown".to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "restore_artifact_not_found");

        // The typed confirmation must equal the artifact file base name.
        let response = restore_submit(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RestoreSubmitRequest {
                artifact_id: artifact_id.clone(),
                confirmation: "wrong-name.db".to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "restore_confirmation_mismatch");

        // A correct submission queues the `restore` Operation immediately.
        let response = restore_submit(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RestoreSubmitRequest {
                artifact_id: artifact_id.clone(),
                confirmation: filename.clone(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let operation_id = body["operation"]["operation"]["operationId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(body["operation"]["operation"]["kind"], "restore");
        assert!(body["auditEventId"].as_i64().unwrap() > 0);

        // Run the worker: the restore validates, then is refused before any
        // mutation (exclusive stopped-Server condition); the current
        // database stays authoritative.
        while crate::operations::process_operations(&state).await.unwrap() > 0 {}
        let detail = load_operation_detail(pool, &operation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.operation.status, "failed");
        assert_eq!(detail.errors[0].code, crate::restore::ERROR_SERVER_RUNNING);
        let result = detail.result.unwrap();
        assert_eq!(result["validation"]["checksum"], "ok");
        assert_eq!(result["refusal"], crate::restore::ERROR_SERVER_RUNNING);
        // No safety copy or replacement happened; the Owner still exists
        // and the artifact metadata is untouched.
        let owners: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role='owner'")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(owners, 1);
        let verification: String =
            sqlx::query_scalar("SELECT verification FROM backup_artifacts WHERE artifact_id = ?")
                .bind(&artifact_id)
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(verification, "pending");
        // The attempt is audited end-to-end (started + finished).
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE target_id = ? AND event_kind IN ('restore_started', 'operation_finished')",
        )
        .bind(&operation_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(audits, 2);

        // A tampered artifact reports the checksum failure as a validation
        // outcome (200), never as a mutation.
        std::fs::write(&artifact_path, b"tampered").unwrap();
        let response = restore_validate(
            State(state.clone()),
            mutation_headers(),
            Extension(session()),
            Extension(request_id()),
            Json(RestoreValidateRequest {
                artifact_id: artifact_id.clone(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["checksumOk"], false);
        assert_eq!(body["error"], "restore_checksum_mismatch");
        assert!(body["message"].as_str().unwrap().contains("checksum"));
        // The short-circuited checks are reported as not reached (null),
        // never as passing.
        assert!(body["integrityOk"].is_null());
        assert!(body["schemaCompatible"].is_null());
    }
}
