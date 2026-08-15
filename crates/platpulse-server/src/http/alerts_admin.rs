//! Owner-only Alert operations (issue #48, design §17, webui.md §4.4):
//! typed Rule review and editing, independent Rule evaluation state,
//! Incident history (immutable), safe Rule preview, and time-bounded
//! Silence and Maintenance policies.
//!
//! Every mutation revalidates the browser trust boundary (JSON content
//! type, exact Origin, session CSRF), commits atomically with its Audit
//! row, and publishes an Admin invalidation so other Owner tabs refetch
//! authoritative REST. Incident history is never manually resolvable,
//! reopenable, or deletable; Silence suppresses delivery only; Maintenance
//! marks expected Incidents suppressed without changing facts. All reads
//! inside a transaction use the transaction handle: the Server pool has
//! one connection, so pool queries inside a transaction would deadlock.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::Executor;
use sqlx::sqlite::Sqlite;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::alerts::{
    AlertError, CATALOG, RuleCondition, RuleState, SubjectKind, SuppressionMatch, catalog_rule,
    rule_schema, validate_condition,
};
use crate::auth::{format_rfc3339, now_utc, parse_rfc3339};
use crate::http::admin::{mutation_error, mutation_guard_ok};
use crate::http::{AppState, AuthenticatedSession, RequestId};

const MAX_WINDOW_SECS: i64 = 366 * 24 * 60 * 60;

fn rule_key_exists(rule_key: &str) -> bool {
    catalog_rule(rule_key).is_some()
}

fn validate_time_window(starts_at: &str, ends_at: &str) -> Result<(), String> {
    let start = parse_rfc3339(starts_at).ok_or("`startsAt` must be an RFC 3339 UTC timestamp")?;
    let end = parse_rfc3339(ends_at).ok_or("`endsAt` must be an RFC 3339 UTC timestamp")?;
    if end <= start {
        return Err("`endsAt` must be after `startsAt`".to_owned());
    }
    if (end - start).whole_seconds() > MAX_WINDOW_SECS {
        return Err("the window may not exceed 366 days".to_owned());
    }
    Ok(())
}

fn validate_reason(reason: &str) -> Result<(), String> {
    if reason.trim().is_empty() {
        return Err("`reason` is required".to_owned());
    }
    if reason.chars().count() > 500 {
        return Err("`reason` may not exceed 500 characters".to_owned());
    }
    Ok(())
}

fn validate_scope_value(scope_kind: &str, scope_value: &str) -> Result<(), String> {
    if scope_value.trim().is_empty() {
        return Err("`scopeValue` is required".to_owned());
    }
    match scope_kind {
        "agent" | "node" | "network" => Ok(()),
        _ => Err("`scopeKind` must be agent, node, or network".to_owned()),
    }
}

fn validate_matcher(matcher_kind: &str, matcher_value: Option<&str>) -> Result<(), String> {
    match matcher_kind {
        "all" => Ok(()),
        "agent" | "node" | "network" => {
            if matcher_value.is_none_or(|value| value.trim().is_empty()) {
                Err("`matcherValue` is required for this matcher kind".to_owned())
            } else {
                Ok(())
            }
        }
        _ => Err("`matcherKind` must be all, agent, node, or network".to_owned()),
    }
}

fn validate_expected_rule_keys(keys: &[String]) -> Result<(), String> {
    for key in keys {
        if !rule_key_exists(key) {
            return Err(format!("unknown alert rule `{key}` in `expectedRuleKeys`"));
        }
    }
    Ok(())
}

fn validate_severity(severity: &str) -> Result<(), String> {
    if crate::alerts::SEVERITIES.contains(&severity) {
        Ok(())
    } else {
        Err("`severity` must be info, warning, or critical".to_owned())
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleEvaluationSummary {
    pub subjects: i64,
    pub normal: i64,
    pub pending: i64,
    pub firing: i64,
    pub recovering: i64,
    pub evaluation_unavailable: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertRuleSummary {
    pub rule_key: String,
    pub subject_kind: String,
    pub enabled: bool,
    pub severity: String,
    pub version: i64,
    pub condition: RuleCondition,
    pub schema: Vec<crate::alerts::ParamSchema>,
    pub created_at: String,
    pub updated_at: String,
    pub open_incidents: i64,
    pub evaluation: RuleEvaluationSummary,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleVersionDto {
    pub version: i64,
    pub severity: String,
    pub condition: RuleCondition,
    pub created_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleOverrideDto {
    pub scope_kind: String,
    pub scope_value: String,
    pub enabled: Option<bool>,
    pub severity: Option<String>,
    pub condition: Option<RuleCondition>,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleStateDto {
    pub subject_kind: String,
    pub subject_key: String,
    pub state: String,
    pub since: String,
    pub pending_since: Option<String>,
    pub firing_since: Option<String>,
    pub recovering_since: Option<String>,
    pub input_kind: String,
    pub input_value: Option<f64>,
    pub input_detail: Option<String>,
    pub evaluation_unavailable: bool,
    pub last_evaluated_at: String,
    pub open_incidents: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertRuleDetail {
    pub rule_key: String,
    pub subject_kind: String,
    pub enabled: bool,
    pub severity: String,
    pub version: i64,
    pub condition: RuleCondition,
    pub schema: Vec<crate::alerts::ParamSchema>,
    pub created_at: String,
    pub updated_at: String,
    pub versions: Vec<RuleVersionDto>,
    pub overrides: Vec<RuleOverrideDto>,
    pub states: Vec<RuleStateDto>,
    pub open_incidents: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlertRuleUpdateRequest {
    pub enabled: Option<bool>,
    pub severity: Option<String>,
    pub condition: Option<RuleCondition>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AlertRuleUpdateResponse {
    pub rule: AlertRuleDetail,
    pub audit_event_id: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleOverrideUpsertRequest {
    pub scope_kind: String,
    pub scope_value: String,
    pub enabled: Option<bool>,
    pub severity: Option<String>,
    pub condition: Option<RuleCondition>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleOverrideResponse {
    pub rule_key: String,
    pub overrides: Vec<RuleOverrideDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RulePreviewRequest {
    pub enabled: Option<bool>,
    pub severity: Option<String>,
    pub condition: Option<RuleCondition>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInput {
    pub kind: String,
    pub value: Option<f64>,
    pub detail: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RulePreviewSubject {
    pub subject_kind: String,
    pub subject_key: String,
    pub current_state: String,
    pub input: PreviewInput,
    pub would_fire: bool,
    pub projected_state: String,
    pub note: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RulePreviewResponse {
    pub rule_key: String,
    pub enabled: bool,
    pub severity: String,
    pub condition: RuleCondition,
    pub subjects: Vec<RulePreviewSubject>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncidentListItem {
    pub incident_id: String,
    pub rule_key: String,
    pub rule_version: i64,
    pub subject_kind: String,
    pub subject_key: String,
    pub severity: String,
    pub state: String,
    pub sequence: i64,
    pub opened_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncidentListResponse {
    pub incidents: Vec<IncidentListItem>,
    pub total: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncidentFilters {
    pub state: Option<String>,
    pub severity: Option<String>,
    pub rule_key: Option<String>,
    pub subject_kind: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SilenceFilters {
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceFilters {
    pub status: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncidentDetail {
    pub incident_id: String,
    pub rule_key: String,
    pub rule_version: i64,
    pub subject_kind: String,
    pub subject_key: String,
    pub severity: String,
    pub state: String,
    pub sequence: i64,
    pub opened_at: String,
    pub resolved_at: Option<String>,
    pub opened_evidence: serde_json::Value,
    pub resolved_evidence: Option<serde_json::Value>,
    pub evaluation: Option<RuleStateDto>,
    pub suppressions: Vec<SuppressionMatch>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SilenceDto {
    pub silence_id: String,
    pub matcher_kind: String,
    pub matcher_value: Option<String>,
    pub reason: String,
    pub starts_at: String,
    pub ends_at: String,
    pub created_by: String,
    pub created_at: String,
    pub cancelled_at: Option<String>,
    pub cancelled_by: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SilenceListResponse {
    pub silences: Vec<SilenceDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SilenceCreateRequest {
    pub matcher_kind: String,
    pub matcher_value: Option<String>,
    pub reason: String,
    pub starts_at: String,
    pub ends_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SilenceMutationResponse {
    pub silence: SilenceDto,
    pub audit_event_id: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceDto {
    pub window_id: String,
    pub scope_kind: String,
    pub scope_value: String,
    pub expected_rule_keys: Vec<String>,
    pub reason: String,
    pub starts_at: String,
    pub ends_at: String,
    pub created_by: String,
    pub created_at: String,
    pub cancelled_at: Option<String>,
    pub cancelled_by: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceListResponse {
    pub windows: Vec<MaintenanceDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaintenanceCreateRequest {
    pub scope_kind: String,
    pub scope_value: String,
    pub expected_rule_keys: Vec<String>,
    pub reason: String,
    pub starts_at: String,
    pub ends_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceMutationResponse {
    pub window: MaintenanceDto,
    pub audit_event_id: i64,
}

// ---------------------------------------------------------------------------
// Shared read helpers (executor-generic: transaction-safe by construction)
// ---------------------------------------------------------------------------

async fn open_incident_counts<'e, E>(executor: E) -> Result<Vec<(String, String, i64)>, sqlx::Error>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query_as::<_, (String, String, i64)>(
        "SELECT rule_key, subject_key, COUNT(*) FROM alert_incidents WHERE state = 'open' GROUP BY rule_key, subject_key",
    )
    .fetch_all(executor)
    .await
}

fn count_open_for(rule_key: &str, subject_key: &str, counts: &[(String, String, i64)]) -> i64 {
    counts
        .iter()
        .find(|(rule, subject, _)| rule == rule_key && subject == subject_key)
        .map(|(_, _, count)| *count)
        .unwrap_or(0)
}

async fn rule_evaluation_summary<'e, E>(
    executor: E,
    rule_key: &str,
) -> Result<RuleEvaluationSummary, sqlx::Error>
where
    E: Executor<'e, Database = Sqlite>,
{
    let rows = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT state, COUNT(*), COALESCE(SUM(evaluation_unavailable), 0) FROM alert_rule_state WHERE rule_key = ? GROUP BY state",
    )
    .bind(rule_key)
    .fetch_all(executor)
    .await?;
    let mut summary = RuleEvaluationSummary {
        subjects: 0,
        normal: 0,
        pending: 0,
        firing: 0,
        recovering: 0,
        evaluation_unavailable: 0,
    };
    for (state, count, unavailable) in rows {
        summary.subjects += count;
        summary.evaluation_unavailable += unavailable;
        match state.as_str() {
            "normal" => summary.normal = count,
            "pending" => summary.pending = count,
            "firing" => summary.firing = count,
            "recovering" => summary.recovering = count,
            _ => {}
        }
    }
    Ok(summary)
}

async fn load_rule_row<'e, E>(
    executor: E,
    rule_key: &str,
) -> Result<Option<(bool, String, i64, String, String, String)>, sqlx::Error>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query_as::<_, (bool, String, i64, String, String, String)>(
        "SELECT enabled, severity, version, condition_json, created_at, updated_at FROM alert_rules WHERE rule_key = ?",
    )
    .bind(rule_key)
    .fetch_optional(executor)
    .await
}

async fn rule_state_dtos<'e, E>(
    executor: E,
    rule_key: &str,
    open_counts: &[(String, String, i64)],
) -> Result<Vec<RuleStateDto>, sqlx::Error>
where
    E: Executor<'e, Database = Sqlite>,
{
    let rows = sqlx::query_as::<_, (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<f64>,
        Option<String>,
        bool,
        String,
    )>(
        "SELECT subject_kind, subject_key, state, pending_since, firing_since, recovering_since, input_kind, input_value, input_detail, evaluation_unavailable, last_evaluated_at FROM alert_rule_state WHERE rule_key = ? ORDER BY subject_kind, subject_key",
    )
    .bind(rule_key)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                subject_kind,
                subject_key,
                state,
                pending_since,
                firing_since,
                recovering_since,
                input_kind,
                input_value,
                input_detail,
                evaluation_unavailable,
                last_evaluated_at,
            )| {
                let open_incidents = count_open_for(rule_key, &subject_key, open_counts);
                RuleStateDto {
                    subject_kind,
                    subject_key,
                    state,
                    since: pending_since
                        .clone()
                        .or_else(|| firing_since.clone())
                        .or_else(|| recovering_since.clone())
                        .unwrap_or_else(|| last_evaluated_at.clone()),
                    pending_since,
                    firing_since,
                    recovering_since,
                    input_kind,
                    input_value,
                    input_detail,
                    evaluation_unavailable,
                    last_evaluated_at,
                    open_incidents,
                }
            },
        )
        .collect())
}

async fn rule_versions_dto<'e, E>(
    executor: E,
    rule_key: &str,
) -> Result<Vec<RuleVersionDto>, AlertError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let rows = sqlx::query_as::<_, (i64, String, String, String)>(
        "SELECT version, severity, condition_json, created_at FROM alert_rule_versions WHERE rule_key = ? ORDER BY version DESC",
    )
    .bind(rule_key)
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|(version, severity, condition_json, created_at)| {
            let condition: RuleCondition = serde_json::from_str(&condition_json).map_err(|_| {
                AlertError::Validation("stored rule condition is invalid".to_owned())
            })?;
            Ok(RuleVersionDto {
                version,
                severity,
                condition,
                created_at,
            })
        })
        .collect()
}

async fn rule_overrides_dto<'e, E>(
    executor: E,
    rule_key: &str,
) -> Result<Vec<RuleOverrideDto>, AlertError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let rows = sqlx::query_as::<_, (String, String, Option<bool>, Option<String>, Option<String>, String)>(
        "SELECT scope_kind, scope_value, enabled, severity, condition_json, updated_at FROM alert_rule_overrides WHERE rule_key = ? ORDER BY scope_kind, scope_value",
    )
    .bind(rule_key)
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(
            |(scope_kind, scope_value, enabled, severity, condition_json, updated_at)| {
                let condition = condition_json
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|_| {
                        AlertError::Validation("stored override condition is invalid".to_owned())
                    })?;
                Ok(RuleOverrideDto {
                    scope_kind,
                    scope_value,
                    enabled,
                    severity,
                    condition,
                    updated_at,
                })
            },
        )
        .collect()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// PAGE-ADMIN-ALERT-RULES: the typed Rule catalog with per-rule evaluation
/// summary and Open Incident counts. The list is Server-owned; the schema
/// renders the typed editor without any free-form input.
#[utoipa::path(
    get,
    path = "/api/admin/v1/alerts/rules",
    tag = "admin",
    responses((status = 200, body = Vec<AlertRuleSummary>), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn alert_rules(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let open_counts = match open_incident_counts(state.db().pool()).await {
        Ok(counts) => counts,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let mut rules = Vec::new();
    for definition in CATALOG {
        let row = match load_rule_row(state.db().pool(), definition.key).await {
            Ok(Some(row)) => row,
            Ok(None) => continue,
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        let (enabled, severity, version, condition_json, created_at, updated_at) = row;
        let condition: RuleCondition = match serde_json::from_str(&condition_json) {
            Ok(condition) => condition,
            Err(_) => continue,
        };
        let evaluation = match rule_evaluation_summary(state.db().pool(), definition.key).await {
            Ok(summary) => summary,
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        rules.push(AlertRuleSummary {
            rule_key: definition.key.to_owned(),
            subject_kind: definition.subject_kind.as_str().to_owned(),
            enabled,
            severity,
            version,
            condition,
            schema: rule_schema(definition),
            created_at,
            updated_at,
            open_incidents: open_counts
                .iter()
                .filter(|(rule, _, _)| rule == definition.key)
                .map(|(_, _, count)| *count)
                .sum(),
            evaluation,
        });
    }
    Json(rules).into_response()
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/alerts/rules/{rule_key}",
    tag = "admin",
    responses((status = 200, body = AlertRuleDetail), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn alert_rule_detail(
    State(state): State<AppState>,
    Path(rule_key): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
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
    let outcome: Result<Option<AlertRuleDetail>, AlertError> = async {
        let Some((enabled, severity, version, condition_json, created_at, updated_at)) =
            load_rule_row(&mut *tx, &rule_key).await?
        else {
            return Ok(None);
        };
        let condition: RuleCondition = serde_json::from_str(&condition_json)
            .map_err(|_| AlertError::Validation("stored rule condition is invalid".to_owned()))?;
        let Some(definition) = catalog_rule(&rule_key) else {
            return Ok(None);
        };
        let versions = rule_versions_dto(&mut *tx, &rule_key).await?;
        let overrides = rule_overrides_dto(&mut *tx, &rule_key).await?;
        let open_counts = open_incident_counts(&mut *tx).await?;
        let states = rule_state_dtos(&mut *tx, &rule_key, &open_counts).await?;
        let open_incidents: i64 = open_counts
            .iter()
            .filter(|(rule, _, _)| rule == &rule_key)
            .map(|(_, _, count)| *count)
            .sum();
        Ok(Some(AlertRuleDetail {
            rule_key: rule_key.clone(),
            subject_kind: definition.subject_kind.as_str().to_owned(),
            enabled,
            severity,
            version,
            condition,
            schema: rule_schema(definition),
            created_at,
            updated_at,
            versions,
            overrides,
            states,
            open_incidents,
        }))
    }
    .await;
    let _ = tx.rollback().await;
    match outcome {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "alert_rule_not_found",
            "unknown alert rule",
        ),
        Err(AlertError::Database(_)) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        ),
        Err(AlertError::Validation(message)) => (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_message(
                "alert_validation",
                message,
                &request_id.0,
            )),
        )
            .into_response(),
    }
}

/// PUT /api/admin/v1/alerts/rules/{rule_key}: edit the typed rule. Edits
/// create an immutable version row; Incidents keep the version they opened
/// under. Disabling stops new evaluation without deleting history.
#[utoipa::path(
    put,
    path = "/api/admin/v1/alerts/rules/{rule_key}",
    tag = "admin",
    request_body = AlertRuleUpdateRequest,
    responses((status = 200, body = AlertRuleUpdateResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn update_alert_rule(
    State(state): State<AppState>,
    Path(rule_key): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: AlertRuleUpdateRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    if body.enabled.is_none() && body.severity.is_none() && body.condition.is_none() {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "alert_validation",
            "at least one of enabled, severity, or condition is required",
        );
    }
    if let Some(severity) = &body.severity {
        if let Err(message) = validate_severity(severity) {
            return (
                StatusCode::BAD_REQUEST,
                Json(crate::http::ApiErrorBody::with_message(
                    "alert_validation",
                    message,
                    &request_id.0,
                )),
            )
                .into_response();
        }
    }
    if let Some(condition) = &body.condition {
        if let Err(message) = validate_condition(&rule_key, condition) {
            return (
                StatusCode::BAD_REQUEST,
                Json(crate::http::ApiErrorBody::with_message(
                    "alert_validation",
                    message,
                    &request_id.0,
                )),
            )
                .into_response();
        }
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
    let outcome: Result<Option<AlertRuleDetail>, AlertError> = async {
        let Some((current_enabled, current_severity, current_version, current_condition, _, _)) =
            load_rule_row(&mut *tx, &rule_key).await?
        else {
            return Ok(None);
        };
        let new_version = current_version + 1;
        let next_enabled = body.enabled.unwrap_or(current_enabled);
        let next_severity = body.severity.unwrap_or(current_severity);
        let next_condition = match &body.condition {
            Some(condition) => condition.clone(),
            None => serde_json::from_str(&current_condition).map_err(|_| {
                AlertError::Validation("stored rule condition is invalid".to_owned())
            })?,
        };
        let updated_at = format_rfc3339(now_utc());
        sqlx::query(
            "INSERT INTO alert_rule_versions (rule_key, version, severity, condition_json, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&rule_key)
        .bind(new_version)
        .bind(&next_severity)
        .bind(serde_json::to_string(&next_condition).expect("condition serializes"))
        .bind(&updated_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE alert_rules SET enabled = ?, severity = ?, version = ?, condition_json = ?, updated_at = ? WHERE rule_key = ?",
        )
        .bind(next_enabled)
        .bind(&next_severity)
        .bind(new_version)
        .bind(serde_json::to_string(&next_condition).expect("condition serializes"))
        .bind(&updated_at)
        .bind(&rule_key)
        .execute(&mut *tx)
        .await?;
        let after = serde_json::json!({
            "enabled": next_enabled,
            "severity": next_severity,
            "condition": next_condition,
            "version": new_version,
        });
        crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            "alert_rule_updated",
            "alert_rule",
            &rule_key,
            Some(&after),
        )
        .await?;
        let created_at: String =
            sqlx::query_scalar("SELECT created_at FROM alert_rules WHERE rule_key = ?")
                .bind(&rule_key)
                .fetch_one(&mut *tx)
                .await?;
        let versions = rule_versions_dto(&mut *tx, &rule_key).await?;
        let overrides = rule_overrides_dto(&mut *tx, &rule_key).await?;
        let open_counts = open_incident_counts(&mut *tx).await?;
        let states = rule_state_dtos(&mut *tx, &rule_key, &open_counts).await?;
        let open_incidents: i64 = open_counts
            .iter()
            .filter(|(rule, _, _)| rule == &rule_key)
            .map(|(_, _, count)| *count)
            .sum();
        let definition = catalog_rule(&rule_key).expect("validated");
        Ok(Some(AlertRuleDetail {
            rule_key: rule_key.clone(),
            subject_kind: definition.subject_kind.as_str().to_owned(),
            enabled: next_enabled,
            severity: next_severity,
            version: new_version,
            condition: next_condition,
            schema: rule_schema(definition),
            created_at,
            updated_at,
            versions,
            overrides,
            states,
            open_incidents,
        }))
    }
    .await;
    match outcome {
        Ok(Some(detail)) => {
            let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
                .fetch_one(&mut *tx)
                .await
            {
                Ok(value) => value,
                Err(_) => {
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
            state.admin_realtime().publish("alerts", None::<String>, 0);
            Json(AlertRuleUpdateResponse {
                rule: detail,
                audit_event_id,
            })
            .into_response()
        }
        Ok(None) => {
            let _ = tx.rollback().await;
            mutation_error(
                &request_id.0,
                StatusCode::NOT_FOUND,
                "alert_rule_not_found",
                "unknown alert rule",
            )
        }
        Err(AlertError::Validation(message)) => {
            let _ = tx.rollback().await;
            (
                StatusCode::BAD_REQUEST,
                Json(crate::http::ApiErrorBody::with_message(
                    "alert_validation",
                    message,
                    &request_id.0,
                )),
            )
                .into_response()
        }
        Err(AlertError::Database(_)) => {
            let _ = tx.rollback().await;
            mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            )
        }
    }
}

/// POST /api/admin/v1/alerts/rules/{rule_key}/preview: evaluate the rule
/// (optionally with an unsaved draft) against current facts for every
/// eligible subject WITHOUT creating Incidents, Notifications, or state
/// rows. `projectedState` shows what the next persisted transition would
/// be; `wouldFire` reflects the typed threshold comparison.
#[utoipa::path(
    post,
    path = "/api/admin/v1/alerts/rules/{rule_key}/preview",
    tag = "admin",
    request_body = RulePreviewRequest,
    responses((status = 200, body = RulePreviewResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn preview_alert_rule(
    State(state): State<AppState>,
    Path(rule_key): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: RulePreviewRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    if let Some(severity) = &body.severity {
        if let Err(message) = validate_severity(severity) {
            return (
                StatusCode::BAD_REQUEST,
                Json(crate::http::ApiErrorBody::with_message(
                    "alert_validation",
                    message,
                    &request_id.0,
                )),
            )
                .into_response();
        }
    }
    if let Some(condition) = &body.condition {
        if let Err(message) = validate_condition(&rule_key, condition) {
            return (
                StatusCode::BAD_REQUEST,
                Json(crate::http::ApiErrorBody::with_message(
                    "alert_validation",
                    message,
                    &request_id.0,
                )),
            )
                .into_response();
        }
    }
    let Some(definition) = catalog_rule(&rule_key) else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "alert_rule_not_found",
            "unknown alert rule",
        );
    };
    let loaded = match load_rule_row(state.db().pool(), &rule_key).await {
        Ok(row) => row,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let Some((current_enabled, current_severity, _, current_condition, _, _)) = loaded else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "alert_rule_not_found",
            "unknown alert rule",
        );
    };
    let base_condition: RuleCondition = match serde_json::from_str(&current_condition) {
        Ok(condition) => condition,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "stored rule condition is invalid",
            );
        }
    };
    let draft_enabled = body.enabled.unwrap_or(current_enabled);
    let draft_severity = body.severity.unwrap_or(current_severity);
    let draft_condition = body.condition.unwrap_or(base_condition);

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
    let now = now_utc();
    let outcome: Result<Vec<RulePreviewSubject>, AlertError> = async {
        // The draft is the new global base; existing Network/Node overrides
        // still apply on top (override fields win), mirroring evaluation.
        let subjects: Vec<(SubjectKind, String)> = match definition.subject_kind {
            SubjectKind::Agent | SubjectKind::Host => sqlx::query_scalar::<_, String>(
                "SELECT agent_id FROM agents ORDER BY agent_id",
            )
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|subject| (definition.subject_kind, subject))
            .collect(),
            SubjectKind::Node => sqlx::query_scalar::<_, String>(
                "SELECT node_id FROM nodes WHERE lifecycle = 'active' ORDER BY node_id",
            )
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|subject| (SubjectKind::Node, subject))
            .collect(),
            _ => Vec::new(),
        };
        let mut previews = Vec::new();
        for (subject_kind, subject_key) in subjects {
            let mut effective = crate::alerts::EffectiveRule {
                rule_key: rule_key.clone(),
                enabled: draft_enabled,
                severity: draft_severity.clone(),
                condition: draft_condition.clone(),
                version: 0,
            };
            if subject_kind == SubjectKind::Node {
                let network_key: Option<String> =
                    sqlx::query_scalar("SELECT network_key FROM nodes WHERE node_id = ?")
                        .bind(&subject_key)
                        .fetch_optional(&mut *tx)
                        .await?
                        .flatten();
                if let Some(network_key) = network_key {
                    if let Some((override_enabled, override_severity, override_condition_json)) =
                        sqlx::query_as::<_, (Option<bool>, Option<String>, Option<String>)>(
                            "SELECT enabled, severity, condition_json FROM alert_rule_overrides WHERE rule_key = ? AND scope_kind = 'network' AND scope_value = ?",
                        )
                        .bind(&rule_key)
                        .bind(&network_key)
                        .fetch_optional(&mut *tx)
                        .await?
                    {
                        apply_preview_override(
                            &mut effective,
                            override_enabled,
                            override_severity,
                            override_condition_json.as_deref(),
                        );
                    }
                }
                if let Some((override_enabled, override_severity, override_condition_json)) =
                    sqlx::query_as::<_, (Option<bool>, Option<String>, Option<String>)>(
                        "SELECT enabled, severity, condition_json FROM alert_rule_overrides WHERE rule_key = ? AND scope_kind = 'node' AND scope_value = ?",
                    )
                    .bind(&rule_key)
                    .bind(&subject_key)
                    .fetch_optional(&mut *tx)
                    .await?
                {
                    apply_preview_override(
                        &mut effective,
                        override_enabled,
                        override_severity,
                        override_condition_json.as_deref(),
                    );
                }
            }
            // The preview uses the same Server-owned freshness bound as
            // live evaluation so projections match reality.
            let stale_after_secs = crate::alerts::freshness_bound(&mut tx).await?;
            let input =
                crate::alerts::extract_input(&mut tx, &rule_key, subject_kind, &subject_key, now, stale_after_secs)
                    .await?;
            let state_row =
                match crate::alerts::load_state_public(&mut tx, &rule_key, &subject_key).await? {
                    Some(row) => row,
                None => RuleState {
                    state: "normal".to_owned(),
                    since: format_rfc3339(now),
                    pending_since: None,
                    firing_since: None,
                    recovering_since: None,
                    input_kind: "known".to_owned(),
                    input_value: None,
                    input_detail: None,
                    evidence_json: None,
                    evaluation_unavailable: false,
                    last_evaluated_at: format_rfc3339(now),
                },
            };
            let transition =
                crate::alerts::project_transition(&state_row, &input, &effective.condition, now);
            previews.push(RulePreviewSubject {
                subject_kind: subject_kind.as_str().to_owned(),
                subject_key,
                current_state: state_row.state.clone(),
                input: PreviewInput {
                    kind: input.kind_str().to_owned(),
                    value: input.value(),
                    detail: input.detail().to_owned(),
                },
                would_fire: effective.enabled
                    && input.fires(effective.condition.effective_threshold()),
                projected_state: transition.state,
                note: transition.note,
            });
        }
        Ok(previews)
    }
    .await;
    let _ = tx.rollback().await;
    match outcome {
        Ok(subjects) => Json(RulePreviewResponse {
            rule_key,
            enabled: draft_enabled,
            severity: draft_severity,
            condition: draft_condition,
            subjects,
        })
        .into_response(),
        Err(AlertError::Database(_)) => mutation_error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        ),
        Err(AlertError::Validation(message)) => (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_message(
                "alert_validation",
                message,
                &request_id.0,
            )),
        )
            .into_response(),
    }
}

fn apply_preview_override(
    effective: &mut crate::alerts::EffectiveRule,
    override_enabled: Option<bool>,
    override_severity: Option<String>,
    override_condition_json: Option<&str>,
) {
    if let Some(enabled) = override_enabled {
        effective.enabled = enabled;
    }
    if let Some(severity) = override_severity {
        effective.severity = severity;
    }
    if let Some(json) = override_condition_json {
        if let Ok(condition) = serde_json::from_str(json) {
            effective.condition = condition;
        }
    }
}

/// PUT /api/admin/v1/alerts/rules/{rule_key}/overrides: upsert a Network or
/// Node override. Override fields (enabled/severity/condition) inherit from
/// the global rule when unset. The override is audited; the base rule
/// version is not bumped (Incidents keep their base-rule version).
#[utoipa::path(
    put,
    path = "/api/admin/v1/alerts/rules/{rule_key}/overrides",
    tag = "admin",
    request_body = RuleOverrideUpsertRequest,
    responses((status = 200, body = RuleOverrideResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn upsert_rule_override(
    State(state): State<AppState>,
    Path(rule_key): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: RuleOverrideUpsertRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    if let Err(message) = validate_scope_value(&body.scope_kind, &body.scope_value) {
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_message(
                "alert_validation",
                message,
                &request_id.0,
            )),
        )
            .into_response();
    }
    if body.scope_kind != "node" && body.scope_kind != "network" {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "alert_validation",
            "override scope must be node or network",
        );
    }
    if let Some(severity) = &body.severity {
        if let Err(message) = validate_severity(severity) {
            return (
                StatusCode::BAD_REQUEST,
                Json(crate::http::ApiErrorBody::with_message(
                    "alert_validation",
                    message,
                    &request_id.0,
                )),
            )
                .into_response();
        }
    }
    if let Some(condition) = &body.condition {
        if let Err(message) = validate_condition(&rule_key, condition) {
            return (
                StatusCode::BAD_REQUEST,
                Json(crate::http::ApiErrorBody::with_message(
                    "alert_validation",
                    message,
                    &request_id.0,
                )),
            )
                .into_response();
        }
    }
    if body.enabled.is_none() && body.severity.is_none() && body.condition.is_none() {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "alert_validation",
            "an override must set at least one of enabled, severity, or condition",
        );
    }
    let target_exists: Option<i64> = if body.scope_kind == "node" {
        sqlx::query_scalar("SELECT 1 FROM nodes WHERE node_id = ?")
            .bind(&body.scope_value)
            .fetch_one(state.db().pool())
            .await
            .unwrap_or(None)
    } else {
        sqlx::query_scalar("SELECT 1 FROM networks WHERE network_key = ?")
            .bind(&body.scope_value)
            .fetch_one(state.db().pool())
            .await
            .unwrap_or(None)
    };
    if target_exists.is_none() {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "alert_validation",
            "override target does not exist",
        );
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
    let outcome: Result<Vec<RuleOverrideDto>, AlertError> = async {
        let updated_at = format_rfc3339(now_utc());
        let condition_json = body
            .condition
            .as_ref()
            .map(|condition| serde_json::to_string(condition).expect("condition serializes"));
        sqlx::query(
            "INSERT INTO alert_rule_overrides (rule_key, scope_kind, scope_value, enabled, severity, condition_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(rule_key, scope_kind, scope_value) DO UPDATE SET enabled=excluded.enabled, severity=excluded.severity, condition_json=excluded.condition_json, updated_at=excluded.updated_at",
        )
        .bind(&rule_key)
        .bind(&body.scope_kind)
        .bind(&body.scope_value)
        .bind(body.enabled)
        .bind(&body.severity)
        .bind(&condition_json)
        .bind(&updated_at)
        .bind(&updated_at)
        .execute(&mut *tx)
        .await?;
        let after = serde_json::json!({
            "scope_kind": body.scope_kind,
            "scope_value": body.scope_value,
            "enabled": body.enabled,
            "severity": body.severity,
            "condition": body.condition,
        });
        crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            "alert_rule_override_updated",
            "alert_rule",
            &rule_key,
            Some(&after),
        )
        .await?;
        rule_overrides_dto(&mut *tx, &rule_key).await
    }
    .await;
    match outcome {
        Ok(overrides) => {
            if tx.commit().await.is_err() {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
            state.admin_realtime().publish("alerts", None::<String>, 0);
            Json(RuleOverrideResponse {
                rule_key,
                overrides,
            })
            .into_response()
        }
        Err(AlertError::Database(_)) => {
            let _ = tx.rollback().await;
            mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            )
        }
        Err(AlertError::Validation(message)) => {
            let _ = tx.rollback().await;
            (
                StatusCode::BAD_REQUEST,
                Json(crate::http::ApiErrorBody::with_message(
                    "alert_validation",
                    message,
                    &request_id.0,
                )),
            )
                .into_response()
        }
    }
}

/// DELETE /api/admin/v1/alerts/rules/{rule_key}/overrides/{scope_kind}/{scope_value}:
/// remove one Network/Node override (audited). The global rule is the only
/// remaining authority for the subject.
#[utoipa::path(
    delete,
    path = "/api/admin/v1/alerts/rules/{rule_key}/overrides/{scope_kind}/{scope_value}",
    tag = "admin",
    responses((status = 200, body = RuleOverrideResponse), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn delete_rule_override(
    State(state): State<AppState>,
    Path((rule_key, scope_kind, scope_value)): Path<(String, String, String)>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    if scope_kind != "node" && scope_kind != "network" || scope_value.is_empty() {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "alert_validation",
            "invalid override scope",
        );
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
    let outcome: Result<Option<Vec<RuleOverrideDto>>, AlertError> = async {
        let deleted = sqlx::query(
            "DELETE FROM alert_rule_overrides WHERE rule_key = ? AND scope_kind = ? AND scope_value = ?",
        )
        .bind(&rule_key)
        .bind(&scope_kind)
        .bind(&scope_value)
        .execute(&mut *tx)
        .await?;
        if deleted.rows_affected() == 0 {
            return Ok(None);
        }
        crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            "alert_rule_override_deleted",
            "alert_rule",
            &rule_key,
            Some(&serde_json::json!({ "scope_kind": scope_kind, "scope_value": scope_value })),
        )
        .await?;
        Ok(Some(rule_overrides_dto(&mut *tx, &rule_key).await?))
    }
    .await;
    match outcome {
        Ok(Some(overrides)) => {
            if tx.commit().await.is_err() {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
            state.admin_realtime().publish("alerts", None::<String>, 0);
            Json(RuleOverrideResponse {
                rule_key,
                overrides,
            })
            .into_response()
        }
        Ok(None) => {
            let _ = tx.rollback().await;
            mutation_error(
                &request_id.0,
                StatusCode::NOT_FOUND,
                "alert_override_not_found",
                "override not found",
            )
        }
        Err(AlertError::Database(_)) => {
            let _ = tx.rollback().await;
            mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            )
        }
        Err(AlertError::Validation(message)) => {
            let _ = tx.rollback().await;
            (
                StatusCode::BAD_REQUEST,
                Json(crate::http::ApiErrorBody::with_message(
                    "alert_validation",
                    message,
                    &request_id.0,
                )),
            )
                .into_response()
        }
    }
}

/// PAGE-ADMIN-INCIDENTS: durable Incident history. Incidents are opened by
/// the state machine and resolved only by sustained fresh Known recovery;
/// they are never manually resolvable, reopenable, or deletable.
#[utoipa::path(
    get,
    path = "/api/admin/v1/alerts/incidents",
    tag = "admin",
    params(
        ("state" = Option<String>, Query, description = "Filter by open or resolved"),
        ("severity" = Option<String>, Query, description = "Filter by severity"),
        ("rule_key" = Option<String>, Query, description = "Filter by Rule key"),
        ("subject_kind" = Option<String>, Query, description = "Filter by subject kind"),
        ("limit" = Option<i64>, Query, description = "Maximum rows (1..=500)"),
    ),
    responses((status = 200, body = IncidentListResponse), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn alert_incidents(
    State(state): State<AppState>,
    Query(filters): Query<IncidentFilters>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    if let Some(state_filter) = &filters.state {
        if state_filter != "open" && state_filter != "resolved" {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "alert_validation",
                "incident state filter must be open or resolved",
            );
        }
        conditions.push("state = ?".to_owned());
        params.push(state_filter.clone());
    }
    if let Some(severity) = &filters.severity {
        if let Err(message) = validate_severity(severity) {
            return (
                StatusCode::BAD_REQUEST,
                Json(crate::http::ApiErrorBody::with_message(
                    "alert_validation",
                    message,
                    &request_id.0,
                )),
            )
                .into_response();
        }
        conditions.push("severity = ?".to_owned());
        params.push(severity.clone());
    }
    if let Some(rule_key) = &filters.rule_key {
        if !rule_key_exists(rule_key) {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "alert_validation",
                "unknown alert rule",
            );
        }
        conditions.push("rule_key = ?".to_owned());
        params.push(rule_key.clone());
    }
    if let Some(subject_kind) = &filters.subject_kind {
        if SubjectKind::parse_str(subject_kind).is_none() {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "alert_validation",
                "invalid subject kind",
            );
        }
        conditions.push("subject_kind = ?".to_owned());
        params.push(subject_kind.clone());
    }
    let limit = filters.limit.unwrap_or(100).clamp(1, 500);
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    let sql = format!(
        "SELECT incident_id, rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, resolved_at FROM alert_incidents{where_clause} ORDER BY opened_at DESC, incident_id LIMIT ?"
    );
    let count_sql = format!("SELECT COUNT(*) FROM alert_incidents{where_clause}");
    let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
    for param in &params {
        count_query = count_query.bind(param);
    }
    let total: i64 = match count_query.fetch_one(state.db().pool()).await {
        Ok(count) => count,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let mut query = sqlx::query_as::<
        _,
        (
            String,
            String,
            i64,
            String,
            String,
            String,
            String,
            i64,
            String,
            Option<String>,
        ),
    >(&sql);
    for param in &params {
        query = query.bind(param);
    }
    let rows = match query.bind(limit).fetch_all(state.db().pool()).await {
        Ok(rows) => rows,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    Json(IncidentListResponse {
        incidents: rows
            .into_iter()
            .map(
                |(
                    incident_id,
                    rule_key,
                    rule_version,
                    subject_kind,
                    subject_key,
                    severity,
                    state,
                    sequence,
                    opened_at,
                    resolved_at,
                )| IncidentListItem {
                    incident_id,
                    rule_key,
                    rule_version,
                    subject_kind,
                    subject_key,
                    severity,
                    state,
                    sequence,
                    opened_at,
                    resolved_at,
                },
            )
            .collect(),
        total,
    })
    .into_response()
}

/// PAGE-ADMIN-INCIDENT: one Incident with its immutable evidence, the
/// current independent evaluation state of its `(rule, subject)`, and any
/// overlapping Silence/Maintenance suppressions (both reasons stay visible
/// independently; webui.md §8.3).
#[utoipa::path(
    get,
    path = "/api/admin/v1/alerts/incidents/{incident_id}",
    tag = "admin",
    responses((status = 200, body = IncidentDetail), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn alert_incident_detail(
    State(state): State<AppState>,
    Path(incident_id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let row = match sqlx::query_as::<_, (String, i64, String, String, String, String, i64, String, Option<String>, String, Option<String>)>(
        "SELECT rule_key, rule_version, subject_kind, subject_key, severity, state, sequence, opened_at, resolved_at, opened_evidence_json, resolved_evidence_json FROM alert_incidents WHERE incident_id = ?",
    )
    .bind(&incident_id)
    .fetch_optional(state.db().pool())
    .await
    {
        Ok(row) => row,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let Some((
        rule_key,
        rule_version,
        subject_kind,
        subject_key,
        severity,
        incident_state,
        sequence,
        opened_at,
        resolved_at,
        opened_evidence_json,
        resolved_evidence_json,
    )) = row
    else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "incident_not_found",
            "Incident not found",
        );
    };
    let opened_evidence: serde_json::Value =
        serde_json::from_str(&opened_evidence_json).unwrap_or(serde_json::Value::Null);
    let resolved_evidence = resolved_evidence_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok());
    let subject_kind_enum = SubjectKind::parse_str(&subject_kind);
    let now = now_utc();

    // Independent evaluation state for the incident's (rule, subject).
    let open_counts = match open_incident_counts(state.db().pool()).await {
        Ok(counts) => counts,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let evaluation = match rule_state_dtos(state.db().pool(), &rule_key, &open_counts).await {
        Ok(states) => states
            .into_iter()
            .find(|state_row| state_row.subject_key == subject_key),
        Err(_) => None,
    };
    let mut conn = match state.db().pool().acquire().await {
        Ok(conn) => conn,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let suppressions = match subject_kind_enum {
        Some(kind) => {
            crate::alerts::suppressions_for_subject(&mut conn, &rule_key, kind, &subject_key, now)
                .await
                .unwrap_or_default()
        }
        None => Vec::new(),
    };

    Json(IncidentDetail {
        incident_id,
        rule_key,
        rule_version,
        subject_kind,
        subject_key,
        severity,
        state: incident_state,
        sequence,
        opened_at,
        resolved_at,
        opened_evidence,
        resolved_evidence,
        evaluation,
        suppressions,
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// Silence
// ---------------------------------------------------------------------------

fn silence_status(dto: &SilenceDto, now: OffsetDateTime) -> String {
    if dto.cancelled_at.is_some() {
        return "cancelled".to_owned();
    }
    match parse_rfc3339(&dto.ends_at) {
        Some(ends_at) if ends_at <= now => "expired".to_owned(),
        _ => "active".to_owned(),
    }
}

type SilenceRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn silence_dto(row: SilenceRow) -> SilenceDto {
    let (
        silence_id,
        matcher_kind,
        matcher_value,
        reason,
        starts_at,
        ends_at,
        created_by,
        created_at,
        cancelled_at,
        cancelled_by,
    ) = row;
    SilenceDto {
        silence_id,
        matcher_kind,
        matcher_value,
        reason,
        starts_at,
        ends_at,
        created_by,
        created_at,
        cancelled_at,
        cancelled_by,
        status: String::new(),
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/alerts/silences",
    tag = "admin",
    params(
        ("status" = Option<String>, Query, description = "Filter by active, expired, or cancelled"),
    ),
    responses((status = 200, body = SilenceListResponse), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn alert_silences(
    State(state): State<AppState>,
    Query(filters): Query<SilenceFilters>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let status_filter = filters.status.as_deref();
    if let Some(status) = status_filter {
        if !["active", "expired", "cancelled"].contains(&status) {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "alert_validation",
                "silence status filter must be active, expired, or cancelled",
            );
        }
    }
    let rows = match sqlx::query_as::<_, (String, String, Option<String>, String, String, String, String, String, Option<String>, Option<String>)>(
        "SELECT silence_id, matcher_kind, matcher_value, reason, starts_at, ends_at, created_by, created_at, cancelled_at, cancelled_by FROM silences ORDER BY starts_at DESC, silence_id",
    )
    .fetch_all(state.db().pool())
    .await
    {
        Ok(rows) => rows,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let now = now_utc();
    let mut silences: Vec<SilenceDto> = rows
        .into_iter()
        .map(|row| {
            let mut dto = silence_dto(row);
            dto.status = silence_status(&dto, now);
            dto
        })
        .collect();
    if let Some(status) = status_filter {
        silences.retain(|silence| silence.status == status);
    }
    Json(SilenceListResponse { silences }).into_response()
}

/// POST /api/admin/v1/alerts/silences: create a time-bounded delivery
/// Silence. It suppresses matching delivery only; evaluation and Incidents
/// are untouched (design §17.5, webui.md §8.3).
#[utoipa::path(
    post,
    path = "/api/admin/v1/alerts/silences",
    tag = "admin",
    request_body = SilenceCreateRequest,
    responses((status = 200, body = SilenceMutationResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn create_silence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: SilenceCreateRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    if let Err(message) = validate_matcher(&body.matcher_kind, body.matcher_value.as_deref()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_message(
                "alert_validation",
                message,
                &request_id.0,
            )),
        )
            .into_response();
    }
    if let Err(message) = validate_reason(&body.reason) {
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_message(
                "alert_validation",
                message,
                &request_id.0,
            )),
        )
            .into_response();
    }
    if let Err(message) = validate_time_window(&body.starts_at, &body.ends_at) {
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_message(
                "alert_validation",
                message,
                &request_id.0,
            )),
        )
            .into_response();
    }
    let target_exists: Option<i64> = match body.matcher_kind.as_str() {
        "node" => sqlx::query_scalar("SELECT 1 FROM nodes WHERE node_id = ?")
            .bind(body.matcher_value.as_deref().unwrap_or_default())
            .fetch_one(state.db().pool())
            .await
            .unwrap_or(None),
        "network" => sqlx::query_scalar("SELECT 1 FROM networks WHERE network_key = ?")
            .bind(body.matcher_value.as_deref().unwrap_or_default())
            .fetch_one(state.db().pool())
            .await
            .unwrap_or(None),
        "agent" => sqlx::query_scalar("SELECT 1 FROM agents WHERE agent_id = ?")
            .bind(body.matcher_value.as_deref().unwrap_or_default())
            .fetch_one(state.db().pool())
            .await
            .unwrap_or(None),
        _ => Some(1),
    };
    if target_exists.is_none() {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "alert_validation",
            "silence target does not exist",
        );
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
    let now_text = format_rfc3339(now_utc());
    let silence_id = uuid::Uuid::new_v4().to_string();
    if sqlx::query(
        "INSERT INTO silences (silence_id, matcher_kind, matcher_value, reason, starts_at, ends_at, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&silence_id)
    .bind(&body.matcher_kind)
    .bind(&body.matcher_value)
    .bind(&body.reason)
    .bind(&body.starts_at)
    .bind(&body.ends_at)
    .bind(&principal.0.user_id)
    .bind(&now_text)
    .execute(&mut *tx)
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
    let after = serde_json::json!({
        "matcher_kind": body.matcher_kind,
        "matcher_value": body.matcher_value,
        "reason": body.reason,
        "starts_at": body.starts_at,
        "ends_at": body.ends_at,
    });
    if crate::auth::insert_audit_event(
        &mut *tx,
        Some(&principal.0.user_id),
        "silence_created",
        "silence",
        &silence_id,
        Some(&after),
    )
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
    let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
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
    state.admin_realtime().publish("alerts", None::<String>, 0);
    let mut silence = silence_dto((
        silence_id,
        body.matcher_kind,
        body.matcher_value,
        body.reason,
        body.starts_at,
        body.ends_at,
        principal.0.user_id.clone(),
        now_text,
        None,
        None,
    ));
    silence.status = silence_status(&silence, now_utc());
    Json(SilenceMutationResponse {
        silence,
        audit_event_id,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/alerts/silences/{silence_id}",
    tag = "admin",
    responses((status = 200, body = SilenceDto), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn alert_silence_detail(
    State(state): State<AppState>,
    Path(silence_id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let row = match sqlx::query_as::<_, (String, Option<String>, String, String, String, String, String, Option<String>, Option<String>)>(
        "SELECT matcher_kind, matcher_value, reason, starts_at, ends_at, created_by, created_at, cancelled_at, cancelled_by FROM silences WHERE silence_id = ?",
    )
    .bind(&silence_id)
    .fetch_optional(state.db().pool())
    .await
    {
        Ok(row) => row,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let Some((
        matcher_kind,
        matcher_value,
        reason,
        starts_at,
        ends_at,
        created_by,
        created_at,
        cancelled_at,
        cancelled_by,
    )) = row
    else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "silence_not_found",
            "Silence not found",
        );
    };
    let mut dto = SilenceDto {
        silence_id,
        matcher_kind,
        matcher_value,
        reason,
        starts_at,
        ends_at,
        created_by,
        created_at,
        cancelled_at,
        cancelled_by,
        status: String::new(),
    };
    dto.status = silence_status(&dto, now_utc());
    Json(dto).into_response()
}

/// POST /api/admin/v1/alerts/silences/{silence_id}/cancel: cancel an
/// active Silence before its natural expiry. Cancellation is audited and
/// irreversible; the row stays visible with its outcome.
#[utoipa::path(
    post,
    path = "/api/admin/v1/alerts/silences/{silence_id}/cancel",
    tag = "admin",
    responses((status = 200, body = SilenceMutationResponse), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn cancel_silence(
    State(state): State<AppState>,
    Path(silence_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Some(response) =
        crate::http::admin::mutation_guard(&headers, &principal, state.auth(), &request_id, false)
    {
        return response;
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
    let now = now_utc();
    let now_text = format_rfc3339(now);
    let row = match sqlx::query_as::<_, (String, Option<String>, String, String, String, String, String, Option<String>, Option<String>)>(
        "SELECT matcher_kind, matcher_value, reason, starts_at, ends_at, created_by, created_at, cancelled_at, cancelled_by FROM silences WHERE silence_id = ?",
    )
    .bind(&silence_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(row) => row,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let Some((
        matcher_kind,
        matcher_value,
        reason,
        starts_at,
        ends_at,
        created_by,
        created_at,
        cancelled_at,
        _cancelled_by,
    )) = row
    else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "silence_not_found",
            "Silence not found",
        );
    };
    if cancelled_at.is_some() {
        return mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "silence_already_cancelled",
            "Silence is already cancelled",
        );
    }
    if parse_rfc3339(&ends_at).is_some_and(|ends_at| ends_at <= now) {
        return mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "silence_expired",
            "Silence already expired",
        );
    }
    if sqlx::query("UPDATE silences SET cancelled_at = ?, cancelled_by = ? WHERE silence_id = ?")
        .bind(&now_text)
        .bind(&principal.0.user_id)
        .bind(&silence_id)
        .execute(&mut *tx)
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
    if crate::auth::insert_audit_event(
        &mut *tx,
        Some(&principal.0.user_id),
        "silence_cancelled",
        "silence",
        &silence_id,
        Some(&serde_json::json!({ "cancelled_at": now_text })),
    )
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
    let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
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
    state.admin_realtime().publish("alerts", None::<String>, 0);
    let mut dto = SilenceDto {
        silence_id,
        matcher_kind,
        matcher_value,
        reason,
        starts_at,
        ends_at,
        created_by,
        created_at,
        cancelled_at: Some(now_text),
        cancelled_by: Some(principal.0.user_id),
        status: String::new(),
    };
    dto.status = silence_status(&dto, now);
    Json(SilenceMutationResponse {
        silence: dto,
        audit_event_id,
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// Maintenance Windows
// ---------------------------------------------------------------------------

type MaintenanceRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn maintenance_dto(row: MaintenanceRow) -> MaintenanceDto {
    let (
        window_id,
        scope_kind,
        scope_value,
        expected_rule_keys,
        reason,
        starts_at,
        ends_at,
        created_by,
        created_at,
        cancelled_at,
        cancelled_by,
    ) = row;
    MaintenanceDto {
        window_id,
        scope_kind,
        scope_value,
        expected_rule_keys: serde_json::from_str(&expected_rule_keys).unwrap_or_default(),
        reason,
        starts_at,
        ends_at,
        created_by,
        created_at,
        cancelled_at,
        cancelled_by,
        status: String::new(),
    }
}

fn maintenance_status(dto: &MaintenanceDto, now: OffsetDateTime) -> String {
    if dto.cancelled_at.is_some() {
        return "cancelled".to_owned();
    }
    match parse_rfc3339(&dto.ends_at) {
        Some(ends_at) if ends_at <= now => "expired".to_owned(),
        _ => "active".to_owned(),
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/alerts/maintenance",
    tag = "admin",
    params(
        ("status" = Option<String>, Query, description = "Filter by active, expired, or cancelled"),
    ),
    responses((status = 200, body = MaintenanceListResponse), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn alert_maintenance(
    State(state): State<AppState>,
    Query(filters): Query<MaintenanceFilters>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let status_filter = filters.status.as_deref();
    if let Some(status) = status_filter {
        if !["active", "expired", "cancelled"].contains(&status) {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "alert_validation",
                "maintenance status filter must be active, expired, or cancelled",
            );
        }
    }
    let rows = match sqlx::query_as::<_, (String, String, String, String, String, String, String, String, String, Option<String>, Option<String>)>(
        "SELECT window_id, scope_kind, scope_value, expected_rule_keys, reason, starts_at, ends_at, created_by, created_at, cancelled_at, cancelled_by FROM maintenance_windows ORDER BY starts_at DESC, window_id",
    )
    .fetch_all(state.db().pool())
    .await
    {
        Ok(rows) => rows,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let now = now_utc();
    let mut windows: Vec<MaintenanceDto> = rows
        .into_iter()
        .map(|row| {
            let mut dto = maintenance_dto(row);
            dto.status = maintenance_status(&dto, now);
            dto
        })
        .collect();
    if let Some(status) = status_filter {
        windows.retain(|window| window.status == status);
    }
    Json(MaintenanceListResponse { windows }).into_response()
}

/// POST /api/admin/v1/alerts/maintenance: create a time-bounded
/// Maintenance Window for an Agent, Node, or Network scope. Expected
/// conditions are a typed allowlist of rule keys; an empty list matches any
/// rule. Maintenance marks expected Incidents suppressed and suppresses
/// expected delivery, without changing facts, evaluation, or Node Health
/// (design §17.5).
#[utoipa::path(
    post,
    path = "/api/admin/v1/alerts/maintenance",
    tag = "admin",
    request_body = MaintenanceCreateRequest,
    responses((status = 200, body = MaintenanceMutationResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn create_maintenance_window(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
    body: axum::body::Bytes,
) -> Response {
    if !mutation_guard_ok(&headers, &state, &principal) {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        );
    }
    let body: MaintenanceCreateRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    if let Err(message) = validate_scope_value(&body.scope_kind, &body.scope_value) {
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_message(
                "alert_validation",
                message,
                &request_id.0,
            )),
        )
            .into_response();
    }
    if let Err(message) = validate_expected_rule_keys(&body.expected_rule_keys) {
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_message(
                "alert_validation",
                message,
                &request_id.0,
            )),
        )
            .into_response();
    }
    if let Err(message) = validate_reason(&body.reason) {
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_message(
                "alert_validation",
                message,
                &request_id.0,
            )),
        )
            .into_response();
    }
    if let Err(message) = validate_time_window(&body.starts_at, &body.ends_at) {
        return (
            StatusCode::BAD_REQUEST,
            Json(crate::http::ApiErrorBody::with_message(
                "alert_validation",
                message,
                &request_id.0,
            )),
        )
            .into_response();
    }
    let target_exists: Option<i64> = match body.scope_kind.as_str() {
        "node" => sqlx::query_scalar("SELECT 1 FROM nodes WHERE node_id = ?")
            .bind(&body.scope_value)
            .fetch_one(state.db().pool())
            .await
            .unwrap_or(None),
        "network" => sqlx::query_scalar("SELECT 1 FROM networks WHERE network_key = ?")
            .bind(&body.scope_value)
            .fetch_one(state.db().pool())
            .await
            .unwrap_or(None),
        "agent" => sqlx::query_scalar("SELECT 1 FROM agents WHERE agent_id = ?")
            .bind(&body.scope_value)
            .fetch_one(state.db().pool())
            .await
            .unwrap_or(None),
        _ => Some(1),
    };
    if target_exists.is_none() {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "alert_validation",
            "maintenance scope target does not exist",
        );
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
    let now_text = format_rfc3339(now_utc());
    let window_id = uuid::Uuid::new_v4().to_string();
    let expected_json =
        serde_json::to_string(&body.expected_rule_keys).expect("rule keys serialize");
    if sqlx::query(
        "INSERT INTO maintenance_windows (window_id, scope_kind, scope_value, expected_rule_keys, reason, starts_at, ends_at, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&window_id)
    .bind(&body.scope_kind)
    .bind(&body.scope_value)
    .bind(&expected_json)
    .bind(&body.reason)
    .bind(&body.starts_at)
    .bind(&body.ends_at)
    .bind(&principal.0.user_id)
    .bind(&now_text)
    .execute(&mut *tx)
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
    if crate::auth::insert_audit_event(
        &mut *tx,
        Some(&principal.0.user_id),
        "maintenance_created",
        "maintenance_window",
        &window_id,
        Some(&serde_json::json!({
            "scope_kind": body.scope_kind,
            "scope_value": body.scope_value,
            "expected_rule_keys": body.expected_rule_keys,
            "reason": body.reason,
            "starts_at": body.starts_at,
            "ends_at": body.ends_at,
        })),
    )
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
    let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
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
    state.admin_realtime().publish("alerts", None::<String>, 0);
    let mut dto = MaintenanceDto {
        window_id,
        scope_kind: body.scope_kind,
        scope_value: body.scope_value,
        expected_rule_keys: body.expected_rule_keys,
        reason: body.reason,
        starts_at: body.starts_at,
        ends_at: body.ends_at,
        created_by: principal.0.user_id,
        created_at: now_text,
        cancelled_at: None,
        cancelled_by: None,
        status: String::new(),
    };
    dto.status = maintenance_status(&dto, now_utc());
    Json(MaintenanceMutationResponse {
        window: dto,
        audit_event_id,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/alerts/maintenance/{window_id}",
    tag = "admin",
    responses((status = 200, body = MaintenanceDto), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn alert_maintenance_detail(
    State(state): State<AppState>,
    Path(window_id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let row = match sqlx::query_as::<_, (String, String, String, String, String, String, String, String, String, Option<String>, Option<String>)>(
        "SELECT window_id, scope_kind, scope_value, expected_rule_keys, reason, starts_at, ends_at, created_by, created_at, cancelled_at, cancelled_by FROM maintenance_windows WHERE window_id = ?",
    )
    .bind(&window_id)
    .fetch_optional(state.db().pool())
    .await
    {
        Ok(row) => row,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let Some(row) = row else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "maintenance_not_found",
            "Maintenance Window not found",
        );
    };
    let mut dto = maintenance_dto(row);
    dto.status = maintenance_status(&dto, now_utc());
    Json(dto).into_response()
}

/// POST /api/admin/v1/alerts/maintenance/{window_id}/cancel: cancel an
/// active Maintenance Window. Cancellation is audited and irreversible;
/// the window stays visible with its outcome.
#[utoipa::path(
    post,
    path = "/api/admin/v1/alerts/maintenance/{window_id}/cancel",
    tag = "admin",
    responses((status = 200, body = MaintenanceMutationResponse), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn cancel_maintenance_window(
    State(state): State<AppState>,
    Path(window_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    if let Some(response) =
        crate::http::admin::mutation_guard(&headers, &principal, state.auth(), &request_id, false)
    {
        return response;
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
    let now = now_utc();
    let now_text = format_rfc3339(now);
    let row = match sqlx::query_as::<_, (String, String, String, String, String, String, String, String, String, Option<String>, Option<String>)>(
        "SELECT window_id, scope_kind, scope_value, expected_rule_keys, reason, starts_at, ends_at, created_by, created_at, cancelled_at, cancelled_by FROM maintenance_windows WHERE window_id = ?",
    )
    .bind(&window_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(row) => row,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let Some(mut dto) = row.map(maintenance_dto) else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "maintenance_not_found",
            "Maintenance Window not found",
        );
    };
    if dto.cancelled_at.is_some() {
        return mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "maintenance_already_cancelled",
            "Maintenance Window is already cancelled",
        );
    }
    if parse_rfc3339(&dto.ends_at).is_some_and(|ends_at| ends_at <= now) {
        return mutation_error(
            &request_id.0,
            StatusCode::CONFLICT,
            "maintenance_expired",
            "Maintenance Window already expired",
        );
    }
    if sqlx::query(
        "UPDATE maintenance_windows SET cancelled_at = ?, cancelled_by = ? WHERE window_id = ?",
    )
    .bind(&now_text)
    .bind(&principal.0.user_id)
    .bind(&window_id)
    .execute(&mut *tx)
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
    if crate::auth::insert_audit_event(
        &mut *tx,
        Some(&principal.0.user_id),
        "maintenance_cancelled",
        "maintenance_window",
        &window_id,
        Some(&serde_json::json!({ "cancelled_at": now_text })),
    )
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
    let audit_event_id: i64 = match sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await
    {
        Ok(value) => value,
        Err(_) => {
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
    state.admin_realtime().publish("alerts", None::<String>, 0);
    dto.cancelled_at = Some(now_text);
    dto.cancelled_by = Some(principal.0.user_id);
    dto.status = "cancelled".to_owned();
    Json(MaintenanceMutationResponse {
        window: dto,
        audit_event_id,
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/alerts/rules", get(alert_rules))
        .route("/alerts/rules/{rule_key}", get(alert_rule_detail))
        .route("/alerts/rules/{rule_key}", put(update_alert_rule))
        .route("/alerts/rules/{rule_key}/preview", post(preview_alert_rule))
        .route(
            "/alerts/rules/{rule_key}/overrides",
            put(upsert_rule_override),
        )
        .route(
            "/alerts/rules/{rule_key}/overrides/{scope_kind}/{scope_value}",
            delete(delete_rule_override),
        )
        .route("/alerts/incidents", get(alert_incidents))
        .route(
            "/alerts/incidents/{incident_id}",
            get(alert_incident_detail),
        )
        .route("/alerts/silences", get(alert_silences))
        .route("/alerts/silences", post(create_silence))
        .route("/alerts/silences/{silence_id}", get(alert_silence_detail))
        .route("/alerts/silences/{silence_id}/cancel", post(cancel_silence))
        .route("/alerts/maintenance", get(alert_maintenance))
        .route("/alerts/maintenance", post(create_maintenance_window))
        .route(
            "/alerts/maintenance/{window_id}",
            get(alert_maintenance_detail),
        )
        .route(
            "/alerts/maintenance/{window_id}/cancel",
            post(cancel_maintenance_window),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::format_rfc3339;
    use axum::body::to_bytes;
    use axum::extract::Extension;
    use axum::http::header;
    use serde_json::Value;
    use tempfile::tempdir;
    use time::macros::datetime;

    fn base_time() -> OffsetDateTime {
        datetime!(2026-03-01 00:00:00 UTC)
    }

    async fn test_state() -> (tempfile::TempDir, AppState) {
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
        let state = AppState::new(database, None, auth);
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-a', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main', '0xgenesis', 210425, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('node-a', 'agent-a', 'mainnet', 'ws://127.0.0.1:1', 'active', 'private', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool()).await.unwrap();
        (dir, state)
    }

    fn session() -> AuthenticatedSession {
        AuthenticatedSession(crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        })
    }

    fn mutation_headers(csrf: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        headers.insert(header::ORIGIN, "http://127.0.0.1:8080".parse().unwrap());
        headers.insert("x-csrf-token", csrf.parse().unwrap());
        headers
    }

    fn request_id() -> RequestId {
        RequestId(std::sync::Arc::from("req-123"))
    }

    async fn set_rpc_error(pool: &sqlx::SqlitePool, state: &str, observed_at: OffsetDateTime) {
        let now = format_rfc3339(observed_at);
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision, error_code, error_message) VALUES ('agent-a', 'node', 'node-a', 'node-a', 'rpc', ?, ?, ?, ?, 1, 1, 'rpc_unreachable', 'connect refused') ON CONFLICT(agent_id, scope, scope_key, component_key) DO UPDATE SET state=excluded.state, received_at=excluded.received_at, error_code=excluded.error_code, error_message=excluded.error_message")
            .bind(state)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn body_json(response: Response) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn rule_list_is_typed_and_ordered_by_catalog() {
        let (_dir, state) = test_state().await;
        let response = alert_rules(State(state), Extension(request_id())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        let rules = value.as_array().unwrap();
        assert_eq!(rules.len(), CATALOG.len());
        assert_eq!(rules[0]["ruleKey"], "agent.offline");
        assert!(rules[0]["schema"].as_array().unwrap().len() >= 2);
        assert_eq!(rules[0]["evaluation"]["subjects"], 0);
    }

    #[tokio::test]
    async fn rule_update_bumps_version_writes_audit_and_keeps_immutable_versions() {
        let (_dir, state) = test_state().await;
        let response = update_alert_rule(
            State(state.clone()),
            Path("agent.offline".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"enabled":false,"severity":"critical","condition":{"for_secs":30,"recovery_for_secs":60,"threshold":180.0}}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["rule"]["version"], 2);
        assert!(!value["rule"]["enabled"].as_bool().unwrap());
        assert_eq!(value["rule"]["severity"], "critical");
        assert!(value["auditEventId"].as_i64().unwrap() > 0);

        // The previous version row is immutable history.
        let version_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM alert_rule_versions WHERE rule_key = 'agent.offline'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(version_count, 2);
        let audit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'alert_rule_updated' AND target_id = 'agent.offline'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit, 1);

        // Detail shows versions newest-first.
        let response = alert_rule_detail(
            State(state.clone()),
            Path("agent.offline".to_owned()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["versions"][0]["version"], 2);
        assert_eq!(value["versions"][1]["version"], 1);
        assert_eq!(value["versions"][1]["condition"]["threshold"], 120.0);
    }

    #[tokio::test]
    async fn rule_update_is_typed_and_guarded() {
        let (_dir, state) = test_state().await;
        // Boolean-fact rule rejects a user threshold.
        let response = update_alert_rule(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"condition":{"for_secs":30,"recovery_for_secs":60,"threshold":90.0}}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let value = body_json(response).await;
        assert_eq!(value["error"]["code"], "alert_validation");

        // Unknown rule and unknown fields are rejected.
        let response = update_alert_rule(
            State(state.clone()),
            Path("nope".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"enabled":true}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = update_alert_rule(
            State(state.clone()),
            Path("agent.offline".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"enabled":true,"script":"rm -rf"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(response).await["error"]["code"], "invalid_json");

        // CSRF mismatch is refused before the body is parsed.
        let response = update_alert_rule(
            State(state.clone()),
            Path("agent.offline".to_owned()),
            mutation_headers("wrong"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"enabled":true}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn preview_evaluates_without_writing_and_reflects_draft_condition() {
        let (_dir, state) = test_state().await;
        set_rpc_error(state.db().pool(), "error", now_utc()).await;
        let response = preview_alert_rule(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"condition":{"for_secs":30,"recovery_for_secs":60}}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["condition"]["for_secs"], 30);
        let subjects = value["subjects"].as_array().unwrap();
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0]["subjectKey"], "node-a");
        assert_eq!(subjects[0]["input"]["kind"], "known");
        assert!(subjects[0]["wouldFire"].as_bool().unwrap());
        assert_eq!(subjects[0]["projectedState"], "pending");

        // Preview must never write state or incidents.
        let states: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_rule_state")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        let incidents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_incidents")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!((states, incidents), (0, 0));

        // A disabled draft projects no firing.
        let response = preview_alert_rule(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"enabled":false}"#),
        )
        .await;
        let value = body_json(response).await;
        assert!(!value["subjects"][0]["wouldFire"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn overrides_upsert_and_delete_are_audited_and_validated() {
        let (_dir, state) = test_state().await;
        let response = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"scopeKind":"node","scopeValue":"node-a","enabled":false}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["overrides"][0]["scopeKind"], "node");

        // Unknown target is refused.
        let response = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"scopeKind":"node","scopeValue":"ghost","enabled":true}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Empty override is refused.
        let response = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"scopeKind":"node","scopeValue":"node-a"}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Delete removes it and is audited.
        let response = delete_rule_override(
            State(state.clone()),
            Path((
                "node.rpc_unreachable".to_owned(),
                "node".to_owned(),
                "node-a".to_owned(),
            )),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let audit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'alert_rule_override_deleted'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit, 1);
    }

    #[tokio::test]
    async fn silence_create_cancel_and_conflicts_are_typed() {
        let (_dir, state) = test_state().await;
        let now = now_utc();
        let starts = format_rfc3339(now - time::Duration::hours(1));
        let ends = format_rfc3339(now + time::Duration::hours(1));
        let body = format!(
            r#"{{"matcherKind":"node","matcherValue":"node-a","reason":"quiet weekend","startsAt":"{starts}","endsAt":"{ends}"}}"#
        );
        let response = create_silence(
            State(state.clone()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from(body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["silence"]["status"], "active");
        assert_eq!(value["silence"]["matcherKind"], "node");
        assert!(value["auditEventId"].as_i64().unwrap() > 0);
        let silence_id = value["silence"]["silenceId"].as_str().unwrap().to_owned();

        // Invalid windows, missing matcher values, and unknown targets.
        let response = create_silence(
            State(state.clone()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from(format!(
                r#"{{"matcherKind":"node","matcherValue":"node-a","reason":"bad","startsAt":"{ends}","endsAt":"{starts}"}}"#
            )),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = create_silence(
            State(state.clone()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from(format!(
                r#"{{"matcherKind":"node","reason":"bad","startsAt":"{starts}","endsAt":"{ends}"}}"#
            )),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = create_silence(
            State(state.clone()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from(format!(
                r#"{{"matcherKind":"node","matcherValue":"ghost","reason":"bad","startsAt":"{starts}","endsAt":"{ends}"}}"#
            )),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Cancel, then cancel again conflicts.
        let response = cancel_silence(
            State(state.clone()),
            Path(silence_id.clone()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["silence"]["status"], "cancelled");
        let response = cancel_silence(
            State(state.clone()),
            Path(silence_id),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);

        // Expired silences cannot be cancelled.
        let past = format_rfc3339(now - time::Duration::hours(3));
        let past_end = format_rfc3339(now - time::Duration::hours(2));
        let body = format!(
            r#"{{"matcherKind":"all","reason":"already gone","startsAt":"{past}","endsAt":"{past_end}"}}"#
        );
        let response = create_silence(
            State(state.clone()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from(body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let expired_id = body_json(response).await["silence"]["silenceId"]
            .as_str()
            .unwrap()
            .to_owned();
        let response = cancel_silence(
            State(state.clone()),
            Path(expired_id),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn maintenance_create_cancel_and_validation() {
        let (_dir, state) = test_state().await;
        let now = now_utc();
        let starts = format_rfc3339(now - time::Duration::hours(1));
        let ends = format_rfc3339(now + time::Duration::hours(2));
        let body = format!(
            r#"{{"scopeKind":"node","scopeValue":"node-a","expectedRuleKeys":["node.rpc_unreachable","node.process_not_running"],"reason":"scheduled reboot","startsAt":"{starts}","endsAt":"{ends}"}}"#
        );
        let response = create_maintenance_window(
            State(state.clone()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from(body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["window"]["status"], "active");
        assert_eq!(
            value["window"]["expectedRuleKeys"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let window_id = value["window"]["windowId"].as_str().unwrap().to_owned();

        // Unknown expected rule keys are refused.
        let response = create_maintenance_window(
            State(state.clone()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from(format!(
                r#"{{"scopeKind":"node","scopeValue":"node-a","expectedRuleKeys":["nope"],"reason":"r","startsAt":"{starts}","endsAt":"{ends}"}}"#
            )),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Cancel and conflict.
        let response = cancel_maintenance_window(
            State(state.clone()),
            Path(window_id.clone()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["window"]["status"], "cancelled");
        let response = cancel_maintenance_window(
            State(state.clone()),
            Path(window_id),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);

        // The list carries computed statuses.
        let response = alert_maintenance(
            State(state.clone()),
            Query(MaintenanceFilters {
                status: Some("cancelled".to_owned()),
            }),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["windows"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn incident_list_and_detail_show_state_evidence_and_suppressions() {
        let (_dir, state) = test_state().await;
        // Drive a real Incident through the evaluator.
        let now = base_time();
        set_rpc_error(state.db().pool(), "error", now).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            now,
        )
        .await
        .unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            now + time::Duration::seconds(61),
        )
        .await
        .unwrap();
        drop(conn);

        // An overlapping Silence and Maintenance Window both appear. The
        // window times are relative to real Server time (status and
        // suppression matching use the Server clock).
        let real_now = now_utc();
        let starts = format_rfc3339(real_now - time::Duration::hours(1));
        let ends = format_rfc3339(real_now + time::Duration::hours(1));
        sqlx::query("INSERT INTO silences (silence_id, matcher_kind, matcher_value, reason, starts_at, ends_at, created_by, created_at) VALUES ('sil-test', 'node', 'node-a', 'quiet', ?, ?, 'owner', ?)")
            .bind(&starts).bind(&ends).bind(&starts)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO maintenance_windows (window_id, scope_kind, scope_value, expected_rule_keys, reason, starts_at, ends_at, created_by, created_at) VALUES ('mnt-test', 'node', 'node-a', '[\"node.rpc_unreachable\"]', 'planned', ?, ?, 'owner', ?)")
            .bind(&starts).bind(&ends).bind(&starts)
            .execute(state.db().pool())
            .await
            .unwrap();

        let response = alert_incidents(
            State(state.clone()),
            Query(IncidentFilters {
                state: Some("open".to_owned()),
                severity: None,
                rule_key: Some("node.rpc_unreachable".to_owned()),
                subject_kind: None,
                limit: None,
            }),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["total"], 1);
        let incident_id = value["incidents"][0]["incidentId"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["state"], "open");
        assert_eq!(value["evaluation"]["state"], "firing");
        assert!(
            value["openedEvidence"]["input_detail"]
                .as_str()
                .unwrap()
                .contains("connect refused")
        );
        let suppressions = value["suppressions"].as_array().unwrap();
        assert_eq!(suppressions.len(), 2);
        assert!(
            suppressions
                .iter()
                .any(|s| s["kind"] == "silence" && s["marksIncident"] == false)
        );
        assert!(
            suppressions
                .iter()
                .any(|s| s["kind"] == "maintenance" && s["marksIncident"] == true)
        );

        // Incident history is immutable: no mutation endpoints exist, and a
        // direct resolution attempt must not be possible through the API.
        let response = alert_incidents(
            State(state.clone()),
            Query(IncidentFilters {
                state: Some("resolved".to_owned()),
                severity: None,
                rule_key: None,
                subject_kind: None,
                limit: None,
            }),
            Extension(request_id()),
        )
        .await;
        assert_eq!(body_json(response).await["total"], 0);
    }
}
