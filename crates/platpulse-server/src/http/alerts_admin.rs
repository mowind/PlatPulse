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
    /// Version of the Rule that this edit was composed against. The save is
    /// rejected when it no longer matches the stored version (issue #204,
    /// Story 19): a stale tab can never silently overwrite a newer edit.
    pub expected_version: i64,
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

/// Transactional outcome of a version-checked Rule write (issue #204). The
/// baseline edit and the override upsert share the same conflict rules, so they
/// share one outcome shape rather than two parallel enums.
enum RuleWriteOutcome<T> {
    Updated(T),
    NotFound,
    /// The declared expectedVersion no longer matches the stored composed version.
    StaleVersion,
}

/// The overrides a version-checked override write left behind, plus the composed
/// configuration revision that write produced.
struct OverrideRevision {
    overrides: Vec<RuleOverrideDto>,
    version: i64,
}

/// The shared response for a version-checked Rule write that named an unknown
/// Rule (issue #204).
fn rule_write_not_found(request_id: &str) -> Response {
    mutation_error(
        request_id,
        StatusCode::NOT_FOUND,
        "alert_rule_not_found",
        "unknown alert rule",
    )
}

/// The shared response for a version-checked Rule write whose expectedVersion no
/// longer matches the stored composed version (issue #204, Story 19).
fn rule_write_version_conflict(request_id: &str) -> Response {
    mutation_error(
        request_id,
        StatusCode::CONFLICT,
        "alert_rule_version_conflict",
        "the rule changed since it was read; reload the current configuration and review before saving",
    )
}

/// Records one composed configuration revision and advances the Rule's stored
/// version to it. The override surfaces change the effective configuration
/// without touching the baseline columns, yet their save still has to
/// invalidate a concurrent reader's expectedVersion (issue #204, Story 19).
async fn record_composed_revision(
    executor: &mut sqlx::SqliteConnection,
    rule_key: &str,
    new_version: i64,
    severity: &str,
    condition_json: &str,
    updated_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO alert_rule_versions (rule_key, version, severity, condition_json, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(rule_key)
    .bind(new_version)
    .bind(severity)
    .bind(condition_json)
    .bind(updated_at)
    .execute(&mut *executor)
    .await?;
    sqlx::query("UPDATE alert_rules SET version = ?, updated_at = ? WHERE rule_key = ?")
        .bind(new_version)
        .bind(updated_at)
        .bind(rule_key)
        .execute(&mut *executor)
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleOverrideUpsertRequest {
    /// Version of the Rule that this override edit was composed against. The
    /// save is rejected when the Rule has changed since (issue #204).
    pub expected_version: i64,
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
    /// Composed configuration revision after this write. Override writes advance
    /// it exactly like a baseline edit, so a second writer still holding the
    /// previous revision is rejected instead of silently overwriting it
    /// (issue #204, Story 19).
    pub version: i64,
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

/// The durable, shared Owner confirmation recorded against one Incident
/// occurrence (parent #202, issue #203). The identity and time are the
/// accountable facts: the first successful request is authoritative and later
/// requests never overwrite them.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncidentAcknowledgment {
    pub acknowledged_by_user_id: Option<String>,
    pub acknowledged_by_username: String,
    pub acknowledged_at: String,
}

/// Build the acknowledgment from a LEFT JOIN's nullable columns. A row exists
/// only after a successful acknowledgment, so any missing column means no
/// acknowledgment was recorded for this Incident.
fn incident_acknowledgment(
    acknowledged_by_user_id: Option<String>,
    acknowledged_by_username: Option<String>,
    acknowledged_at: Option<String>,
) -> Option<IncidentAcknowledgment> {
    match (acknowledged_by_username, acknowledged_at) {
        (Some(acknowledged_by_username), Some(acknowledged_at)) => Some(IncidentAcknowledgment {
            acknowledged_by_user_id,
            acknowledged_by_username,
            acknowledged_at,
        }),
        _ => None,
    }
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
    /// Set once the subject (Agent/Node) was permanently deleted. The
    /// Incident keeps its original facts and open/resolved state; the
    /// annotation only records that the subject is gone (design §15.7).
    pub subject_deleted_at: Option<String>,
    /// Present once an Owner durably confirmed this Incident occurrence.
    pub acknowledgment: Option<IncidentAcknowledgment>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncidentListResponse {
    pub incidents: Vec<IncidentListItem>,
    pub total: i64,
}

/// Query filters for the Incident list. The wire names are the snake_case
/// names declared in the OpenAPI operation (and therefore sent by the
/// generated client); the camelCase aliases are accepted for compatibility.
/// A serde `rename_all = "camelCase"` here silently ignored the real
/// `rule_key`/`subject_kind` query parameters.
#[derive(Debug, Deserialize)]
pub struct IncidentFilters {
    pub state: Option<String>,
    pub severity: Option<String>,
    #[serde(alias = "ruleKey")]
    pub rule_key: Option<String>,
    #[serde(alias = "subjectKind")]
    pub subject_kind: Option<String>,
    /// Exact subject key (node id, agent id, or network key). Paired with
    /// subject_kind it backs the contextual Node/Agent shortcut into the
    /// Incident surface (issue #202 Story 2).
    #[serde(alias = "subjectKey")]
    pub subject_key: Option<String>,
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

/// The Rule configuration that currently applies to one subject after
/// Network/Node override resolution (issue #204, Story 20). This is the
/// current effective configuration and is deliberately separate from the
/// Incident's immutable opening rule version and evidence.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveRuleDto {
    pub rule_key: String,
    pub enabled: bool,
    pub severity: String,
    pub condition: RuleCondition,
    pub version: i64,
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
    /// Set once the subject (Agent/Node) was permanently deleted (design
    /// §15.7, issue #175).
    pub subject_deleted_at: Option<String>,
    pub opened_evidence: serde_json::Value,
    pub resolved_evidence: Option<serde_json::Value>,
    pub evaluation: Option<RuleStateDto>,
    /// Current effective Rule configuration for this subject, after
    /// Network/Node override resolution (issue #204, Story 20). None when the
    /// Rule or subject kind cannot be resolved. A disabled current Rule means
    /// the evaluation row, when present, is the last recorded assessment
    /// rather than a current one; this never rewrites the Incident's opening
    /// rule version or evidence.
    pub current_rule: Option<EffectiveRuleDto>,
    pub suppressions: Vec<SuppressionMatch>,
    /// Present once an Owner durably confirmed this Incident occurrence.
    pub acknowledgment: Option<IncidentAcknowledgment>,
}

/// The authoritative result of an Owner acknowledgment request.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncidentAcknowledgmentResponse {
    pub incident_id: String,
    pub acknowledgment: IncidentAcknowledgment,
    /// True when this request recorded the acknowledgment; false when an
    /// earlier request had already confirmed the same Incident occurrence.
    pub recorded: bool,
    /// Audit Event id of the first confirmation; absent on a no-op repeat.
    pub audit_event_id: Option<i64>,
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
    // A deleted subject's Incidents are retained evidence, not current
    // problems: they never count toward "open" rule/state views (design
    // §15.7, issue #175).
    sqlx::query_as::<_, (String, String, i64)>(
        "SELECT rule_key, subject_key, COUNT(*) FROM alert_incidents WHERE state = 'open' AND subject_deleted_at IS NULL GROUP BY rule_key, subject_key",
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
    responses((status = 200, body = AlertRuleUpdateResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
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
    let outcome: Result<RuleWriteOutcome<Box<AlertRuleDetail>>, AlertError> = async {
        let Some((current_enabled, current_severity, current_version, current_condition, _, _)) =
            load_rule_row(&mut *tx, &rule_key).await?
        else {
            return Ok(RuleWriteOutcome::NotFound);
        };
        // Version-safe edit (issue #204, Story 19): reject a save composed
        // against a version that has since changed instead of overwriting the
        // newer configuration. The Owner must refetch and review again.
        if body.expected_version != current_version {
            return Ok(RuleWriteOutcome::StaleVersion);
        }
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
        // A Rule whose effective enabled flag changes was not observed while
        // the change was in effect, so any in-flight recovery window is stale
        // (issue #203 review R1).
        if next_enabled != current_enabled {
            crate::alerts::invalidate_recovery_windows_for_rule(&mut tx, &rule_key).await?;
        }
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
        Ok(RuleWriteOutcome::Updated(Box::new(AlertRuleDetail {
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
        })))
    }
    .await;
    match outcome {
        Ok(RuleWriteOutcome::Updated(detail)) => {
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
                rule: *detail,
                audit_event_id,
            })
            .into_response()
        }
        Ok(RuleWriteOutcome::NotFound) => {
            let _ = tx.rollback().await;
            rule_write_not_found(&request_id.0)
        }
        Ok(RuleWriteOutcome::StaleVersion) => {
            let _ = tx.rollback().await;
            rule_write_version_conflict(&request_id.0)
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
            // A removed Agent (deleted_at) is no longer a subject, so the
            // preview must not project it as a current problem (issue #175).
            SubjectKind::Agent | SubjectKind::Host => sqlx::query_scalar::<_, String>(
                "SELECT agent_id FROM agents WHERE deleted_at IS NULL ORDER BY agent_id",
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
    responses((status = 200, body = RuleOverrideResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
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
    let outcome: Result<RuleWriteOutcome<OverrideRevision>, AlertError> = async {
        // Version-safe override save (issue #204): a stale tab must reload and
        // review before replacing an override on a Rule that has since changed.
        let Some((_, current_severity, current_version, current_condition, _, _)) =
            load_rule_row(&mut *tx, &rule_key).await?
        else {
            return Ok(RuleWriteOutcome::NotFound);
        };
        if body.expected_version != current_version {
            return Ok(RuleWriteOutcome::StaleVersion);
        }
        let updated_at = format_rfc3339(now_utc());
        // Replacing an override is a full replace. Clearing an explicit enabled
        // value back to inheritance (Some -> None) changes the effective flag
        // just as much as setting one, so the previous value must be read before
        // the upsert to decide whether recovery windows are invalidated (issue
        // #203 review B1).
        let previous_enabled: Option<Option<bool>> = sqlx::query_scalar(
            "SELECT enabled FROM alert_rule_overrides WHERE rule_key = ? AND scope_kind = ? AND scope_value = ?",
        )
        .bind(&rule_key)
        .bind(&body.scope_kind)
        .bind(&body.scope_value)
        .fetch_optional(&mut *tx)
        .await?;
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
        // An override that can change the effective enabled flag of its
        // subjects invalidates their in-flight recovery windows (issue #203
        // review R1).
        if body.enabled.is_some() || matches!(previous_enabled, Some(Some(_))) {
            match body.scope_kind.as_str() {
                "node" => {
                    crate::alerts::invalidate_recovery_windows_for_subject(
                        &mut tx,
                        &rule_key,
                        &body.scope_value,
                    )
                    .await?;
                }
                "network" => {
                    crate::alerts::invalidate_recovery_windows_for_network(
                        &mut tx,
                        &rule_key,
                        &body.scope_value,
                    )
                    .await?;
                }
                _ => {}
            }
        }
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
        let new_version = current_version + 1;
        record_composed_revision(
            &mut tx,
            &rule_key,
            new_version,
            &current_severity,
            &current_condition,
            &updated_at,
        )
        .await?;
        Ok(RuleWriteOutcome::Updated(OverrideRevision {
            overrides: rule_overrides_dto(&mut *tx, &rule_key).await?,
            version: new_version,
        }))
    }
    .await;
    match outcome {
        Ok(RuleWriteOutcome::Updated(OverrideRevision { overrides, version })) => {
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
                version,
                overrides,
            })
            .into_response()
        }
        Ok(RuleWriteOutcome::NotFound) => {
            let _ = tx.rollback().await;
            rule_write_not_found(&request_id.0)
        }
        Ok(RuleWriteOutcome::StaleVersion) => {
            let _ = tx.rollback().await;
            rule_write_version_conflict(&request_id.0)
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
/// remaining authority for the subject. Removing it is an explicit, rebuildable
/// change, so it carries no version precondition of its own, but it does
/// advance the composed version: a writer still holding the previous revision
/// must reload before saving (issue #204, Story 19).
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
    let outcome: Result<Option<(Vec<RuleOverrideDto>, i64)>, AlertError> = async {
        let Some((_, current_severity, current_version, current_condition, _, _)) =
            load_rule_row(&mut *tx, &rule_key).await?
        else {
            return Ok(None);
        };
        // Whether the removed override could have flipped an effective
        // enabled flag decides whether recovery windows must be invalidated
        // (issue #203 review R1).
        let existing_enabled: Option<Option<bool>> = sqlx::query_scalar(
            "SELECT enabled FROM alert_rule_overrides WHERE rule_key = ? AND scope_kind = ? AND scope_value = ?",
        )
        .bind(&rule_key)
        .bind(&scope_kind)
        .bind(&scope_value)
        .fetch_optional(&mut *tx)
        .await?;
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
        if matches!(existing_enabled, Some(Some(_))) {
            match scope_kind.as_str() {
                "node" => {
                    crate::alerts::invalidate_recovery_windows_for_subject(
                        &mut tx,
                        &rule_key,
                        &scope_value,
                    )
                    .await?;
                }
                "network" => {
                    crate::alerts::invalidate_recovery_windows_for_network(
                        &mut tx,
                        &rule_key,
                        &scope_value,
                    )
                    .await?;
                }
                _ => {}
            }
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
        let new_version = current_version + 1;
        record_composed_revision(
            &mut tx,
            &rule_key,
            new_version,
            &current_severity,
            &current_condition,
            &format_rfc3339(now_utc()),
        )
        .await?;
        Ok(Some((
            rule_overrides_dto(&mut *tx, &rule_key).await?,
            new_version,
        )))
    }
    .await;
    match outcome {
        Ok(Some((overrides, version))) => {
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
                version,
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
        ("subject_key" = Option<String>, Query, description = "Filter by exact subject key"),
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
        conditions.push("i.state = ?".to_owned());
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
        conditions.push("i.severity = ?".to_owned());
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
        conditions.push("i.rule_key = ?".to_owned());
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
        conditions.push("i.subject_kind = ?".to_owned());
        params.push(subject_kind.clone());
    }
    if let Some(subject_key) = &filters.subject_key {
        if subject_key.trim().is_empty() {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "alert_validation",
                "incident subject key filter must not be empty",
            );
        }
        conditions.push("i.subject_key = ?".to_owned());
        params.push(subject_key.clone());
    }
    let limit = filters.limit.unwrap_or(100).clamp(1, 500);
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    let sql = format!(
        "SELECT i.incident_id, i.rule_key, i.rule_version, i.subject_kind, i.subject_key, i.severity, i.state, i.sequence, i.opened_at, i.resolved_at, i.subject_deleted_at, a.acknowledged_by_user_id, a.acknowledged_by_username, a.acknowledged_at FROM alert_incidents i LEFT JOIN incident_acknowledgments a ON a.incident_id = i.incident_id{where_clause} ORDER BY i.opened_at DESC, i.incident_id LIMIT ?"
    );
    let count_sql = format!("SELECT COUNT(*) FROM alert_incidents i{where_clause}");
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
            Option<String>,
            Option<String>,
            Option<String>,
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
                    subject_deleted_at,
                    acknowledged_by_user_id,
                    acknowledged_by_username,
                    acknowledged_at,
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
                    subject_deleted_at,
                    acknowledgment: incident_acknowledgment(
                        acknowledged_by_user_id,
                        acknowledged_by_username,
                        acknowledged_at,
                    ),
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
    let row = match sqlx::query_as::<_, (String, i64, String, String, String, String, i64, String, Option<String>, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)>(
        "SELECT i.rule_key, i.rule_version, i.subject_kind, i.subject_key, i.severity, i.state, i.sequence, i.opened_at, i.resolved_at, i.opened_evidence_json, i.resolved_evidence_json, i.subject_deleted_at, a.acknowledged_by_user_id, a.acknowledged_by_username, a.acknowledged_at FROM alert_incidents i LEFT JOIN incident_acknowledgments a ON a.incident_id = i.incident_id WHERE i.incident_id = ?",
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
        subject_deleted_at,
        acknowledged_by_user_id,
        acknowledged_by_username,
        acknowledged_at,
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
    // Project the Rule's current effective configuration next to the
    // evaluation row: a disabled Rule keeps history, so the row alone must
    // never be presented as a current assessment (issue #203 review B5), and
    // the immutable opening rule version/evidence must stay visibly distinct
    // from what applies now (issue #204, Story 20). A lookup failure is a
    // server error, not an unknown state: only an unrecognized subject_kind
    // yields None (issue #203 review C1).
    let current_rule = match subject_kind_enum {
        Some(kind) => {
            match crate::alerts::effective_rule(&mut conn, &rule_key, kind, &subject_key).await {
                Ok(rule) => rule.map(|rule| EffectiveRuleDto {
                    rule_key: rule.rule_key,
                    enabled: rule.enabled,
                    severity: rule.severity,
                    condition: rule.condition,
                    version: rule.version,
                }),
                Err(_) => {
                    return mutation_error(
                        &request_id.0,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "Server database is unavailable",
                    );
                }
            }
        }
        None => None,
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
        subject_deleted_at,
        opened_evidence,
        resolved_evidence,
        evaluation,
        current_rule,
        suppressions,
        acknowledgment: incident_acknowledgment(
            acknowledged_by_user_id,
            acknowledged_by_username,
            acknowledged_at,
        ),
    })
    .into_response()
}

/// PAGE-ADMIN-INCIDENT: record the Owner's durable confirmation of one
/// Incident occurrence (parent #202, issue #203). The first successful request
/// is authoritative; a repeat or concurrent request for the same Incident is a
/// no-op that returns the stored identity and time. The acknowledgment is never
/// retracted, does not resolve the Incident, and does not change health,
/// recovery, evaluation, or notification behavior. The durable row and its
/// Audit Event commit in one transaction.
#[utoipa::path(
    post,
    path = "/api/admin/v1/alerts/incidents/{incident_id}/acknowledgments",
    tag = "admin",
    responses(
        (status = 200, body = IncidentAcknowledgmentResponse),
        (status = 403, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody),
    )
)]
pub(crate) async fn acknowledge_incident(
    State(state): State<AppState>,
    Path(incident_id): Path<String>,
    headers: HeaderMap,
    Extension(principal): Extension<AuthenticatedSession>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    // The acknowledgment request carries no body, so the generated browser
    // client omits Content-Type. Reuse the shared trust boundary with
    // `require_json_body = false`: Origin and CSRF are still mandatory.
    if crate::http::admin::mutation_guard(&headers, &principal, state.auth(), &request_id, false)
        .is_some()
    {
        return mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
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
    let exists: Option<(String,)> =
        match sqlx::query_as("SELECT incident_id FROM alert_incidents WHERE incident_id = ?")
            .bind(&incident_id)
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
    if exists.is_none() {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "incident_not_found",
            "Incident not found",
        );
    }
    let acknowledged_at = format_rfc3339(now_utc());
    // First-write-wins: the primary key makes the first successful confirmation
    // authoritative and every later request a no-op.
    let recorded = match sqlx::query(
        "INSERT INTO incident_acknowledgments (incident_id, acknowledged_by_user_id, acknowledged_by_username, acknowledged_at) VALUES (?, ?, ?, ?) ON CONFLICT(incident_id) DO NOTHING",
    )
    .bind(&incident_id)
    .bind(&principal.0.user_id)
    .bind(&principal.0.username)
    .bind(&acknowledged_at)
    .execute(&mut *tx)
    .await
    {
        Ok(result) => result.rows_affected() > 0,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let stored = match sqlx::query_as::<_, (Option<String>, String, String)>(
        "SELECT acknowledged_by_user_id, acknowledged_by_username, acknowledged_at FROM incident_acknowledgments WHERE incident_id = ?",
    )
    .bind(&incident_id)
    .fetch_one(&mut *tx)
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
    let audit_event_id = if recorded {
        let after = serde_json::json!({
            "incidentId": &incident_id,
            "acknowledgedByUserId": &stored.0,
            "acknowledgedByUsername": &stored.1,
            "acknowledgedAt": &stored.2,
        });
        if crate::auth::insert_audit_event(
            &mut *tx,
            Some(&principal.0.user_id),
            "incident_acknowledged",
            "incident",
            &incident_id,
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
        match sqlx::query_scalar::<_, i64>("SELECT last_insert_rowid()")
            .fetch_one(&mut *tx)
            .await
        {
            Ok(id) => Some(id),
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        }
    } else {
        None
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
    Json(IncidentAcknowledgmentResponse {
        incident_id,
        acknowledgment: IncidentAcknowledgment {
            acknowledged_by_user_id: stored.0,
            acknowledged_by_username: stored.1,
            acknowledged_at: stored.2,
        },
        recorded,
        audit_event_id,
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
        .route(
            "/alerts/incidents/{incident_id}/acknowledgments",
            post(acknowledge_incident),
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
                br#"{"expectedVersion":1,"enabled":false,"severity":"critical","condition":{"for_secs":30,"recovery_for_secs":60,"threshold":180.0}}"#,
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
                br#"{"expectedVersion":1,"condition":{"for_secs":30,"recovery_for_secs":60,"threshold":90.0}}"#,
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
            axum::body::Bytes::from_static(br#"{"expectedVersion":1,"enabled":true}"#),
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

    /// Issue #204: a save composed against a stale Rule version is rejected and
    /// leaves the stored Rule untouched (no silent overwrite, no new version).
    #[tokio::test]
    async fn rule_update_rejects_a_stale_expected_version() {
        let (_dir, state) = test_state().await;
        let first = update_alert_rule(
            State(state.clone()),
            Path("agent.offline".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"expectedVersion":1,"severity":"critical"}"#),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(body_json(first).await["rule"]["version"], 2);
        let before: (i64, String, bool) = sqlx::query_as(
            "SELECT version, severity, enabled FROM alert_rules WHERE rule_key = 'agent.offline'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!((before.0, before.1.as_str()), (2, "critical"));

        // A second save composed against version 1 must not overwrite version 2.
        let stale = update_alert_rule(
            State(state.clone()),
            Path("agent.offline".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"expectedVersion":1,"enabled":false}"#),
        )
        .await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert_eq!(
            body_json(stale).await["error"]["code"],
            "alert_rule_version_conflict"
        );
        let after: (i64, String, bool) = sqlx::query_as(
            "SELECT version, severity, enabled FROM alert_rules WHERE rule_key = 'agent.offline'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            after, before,
            "the stale save must leave the Rule unchanged"
        );
        let versions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM alert_rule_versions WHERE rule_key = 'agent.offline'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            versions, 2,
            "the rejected save writes no immutable version row"
        );
    }

    /// Issue #204: an override save composed against a stale Rule version is
    /// rejected so a stale tab cannot overwrite newer inheritance state.
    #[tokio::test]
    async fn override_upsert_rejects_a_stale_expected_version() {
        let (_dir, state) = test_state().await;
        let bump = update_alert_rule(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"expectedVersion":1,"severity":"critical"}"#),
        )
        .await;
        assert_eq!(bump.status(), StatusCode::OK);
        assert_eq!(body_json(bump).await["rule"]["version"], 2);

        let stale = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"expectedVersion":1,"scopeKind":"node","scopeValue":"node-a","enabled":false}"#,
            ),
        )
        .await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert_eq!(
            body_json(stale).await["error"]["code"],
            "alert_rule_version_conflict"
        );
        let overrides: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_rule_overrides")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(overrides, 0, "the rejected override writes no row");
    }

    /// Issue #204 Story 19 for the override surfaces: an override write advances
    /// the composed configuration revision, so a second writer still holding the
    /// previous revision is rejected instead of silently overwriting it.
    #[tokio::test]
    async fn override_write_advances_the_composed_version() {
        let (_dir, state) = test_state().await;

        let first = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"expectedVersion":1,"scopeKind":"node","scopeValue":"node-a","enabled":false}"#,
            ),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(body_json(first).await["version"], 2);

        // A different override, composed against the pre-write revision, is
        // stale even though the baseline columns did not move.
        let stale = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"expectedVersion":1,"scopeKind":"network","scopeValue":"mainnet","enabled":false}"#,
            ),
        )
        .await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert_eq!(
            body_json(stale).await["error"]["code"],
            "alert_rule_version_conflict"
        );
        let overrides: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_rule_overrides")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(overrides, 1, "the rejected override writes no row");

        let second = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"expectedVersion":2,"scopeKind":"network","scopeValue":"mainnet","enabled":false}"#,
            ),
        )
        .await;
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(body_json(second).await["version"], 3);

        // Removing an override advances the composed revision as well.
        let removed = delete_rule_override(
            State(state.clone()),
            Path((
                "node.rpc_unreachable".to_owned(),
                "network".to_owned(),
                "mainnet".to_owned(),
            )),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(removed.status(), StatusCode::OK);
        assert_eq!(body_json(removed).await["version"], 4);
        let stored: i64 = sqlx::query_scalar(
            "SELECT version FROM alert_rules WHERE rule_key = 'node.rpc_unreachable'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, 4);
    }

    /// Issue #204 Stories 20 and 21: a Rule edit and disable leave the
    /// Incident opening rule version, evidence, and acknowledgment intact
    /// while the separately projected current configuration moves on.
    #[tokio::test]
    async fn rule_edit_and_disable_preserve_incident_opening_facts_and_acknowledgment() {
        let (_dir, state) = test_state().await;
        let now = base_time();
        let incident_id = open_node_incident(&state, now).await;
        let ack = acknowledge(&state, &incident_id, "csrf", session()).await;
        assert_eq!(ack.status(), StatusCode::OK);

        let before = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let before = body_json(before).await;
        assert_eq!(before["ruleVersion"], 1);
        assert_eq!(before["currentRule"]["version"], 1);
        assert_eq!(before["state"], "open");
        let opened_evidence = before["openedEvidence"].clone();
        let acknowledged_at = before["acknowledgment"]["acknowledgedAt"].clone();

        // Edit the Rule (version 1 -> 2) and disable it in one save.
        let edit = update_alert_rule(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"expectedVersion":1,"enabled":false,"severity":"warning"}"#,
            ),
        )
        .await;
        assert_eq!(edit.status(), StatusCode::OK);

        let after = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let after = body_json(after).await;
        // Opening facts are frozen at the version that opened the Incident.
        assert_eq!(after["ruleVersion"], 1);
        assert_eq!(after["openedEvidence"], opened_evidence);
        assert_eq!(after["state"], "open");
        assert_eq!(after["acknowledgment"]["acknowledgedAt"], acknowledged_at);
        // The separately projected current configuration reflects the edit.
        assert_eq!(after["currentRule"]["version"], 2);
        assert_eq!(after["currentRule"]["enabled"], false);
        assert_eq!(after["currentRule"]["severity"], "warning");
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
                br#"{"expectedVersion":1,"scopeKind":"node","scopeValue":"node-a","enabled":false}"#,
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
                br#"{"expectedVersion":1,"scopeKind":"node","scopeValue":"ghost","enabled":true}"#,
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
            axum::body::Bytes::from_static(
                br#"{"expectedVersion":1,"scopeKind":"node","scopeValue":"node-a"}"#,
            ),
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
                subject_key: None,
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
                subject_key: None,
                limit: None,
            }),
            Extension(request_id()),
        )
        .await;
        assert_eq!(body_json(response).await["total"], 0);
    }

    /// Drive the real evaluator until an Incident opens for node-a.
    async fn open_node_incident(state: &AppState, observed_at: OffsetDateTime) -> String {
        set_rpc_error(state.db().pool(), "error", observed_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            observed_at,
        )
        .await
        .unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            observed_at + time::Duration::seconds(61),
        )
        .await
        .unwrap();
        drop(conn);
        sqlx::query_scalar::<_, String>(
            "SELECT incident_id FROM alert_incidents WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a' AND state = 'open' ORDER BY sequence DESC LIMIT 1",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap()
    }

    fn owner_session(user_id: &str) -> AuthenticatedSession {
        AuthenticatedSession(crate::auth::SessionInfo {
            user_id: user_id.to_owned(),
            username: user_id.to_owned(),
            ..session().0
        })
    }

    async fn acknowledge(
        state: &AppState,
        incident_id: &str,
        csrf: &str,
        principal: AuthenticatedSession,
    ) -> Response {
        acknowledge_incident(
            State(state.clone()),
            Path(incident_id.to_owned()),
            mutation_headers(csrf),
            Extension(principal),
            Extension(request_id()),
        )
        .await
    }

    #[tokio::test]
    async fn acknowledge_incident_records_the_first_owner_and_rejects_a_second() {
        let (_dir, state) = test_state().await;
        let now = base_time();
        let incident_id = open_node_incident(&state, now).await;
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner-2', 'owner-2', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();

        let response = acknowledge(&state, &incident_id, "csrf", session()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["recorded"], true);
        assert_eq!(value["acknowledgment"]["acknowledgedByUserId"], "owner");
        assert_eq!(value["acknowledgment"]["acknowledgedByUsername"], "owner");
        let acknowledged_at = value["acknowledgment"]["acknowledgedAt"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(value["auditEventId"].as_i64().unwrap() > 0);

        // A second Owner's request is a no-op: same identity and time, no new
        // Audit Event.
        let response = acknowledge(&state, &incident_id, "csrf", owner_session("owner-2")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        assert_eq!(value["recorded"], false);
        assert!(value["auditEventId"].is_null());
        assert_eq!(value["acknowledgment"]["acknowledgedByUserId"], "owner");
        assert_eq!(value["acknowledgment"]["acknowledgedByUsername"], "owner");
        assert_eq!(
            value["acknowledgment"]["acknowledgedAt"],
            acknowledged_at.clone()
        );

        let audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'incident_acknowledged' AND target_id = ?",
        )
        .bind(&incident_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit_count, 1);

        // List and detail both expose the authoritative acknowledgment.
        let response = alert_incidents(
            State(state.clone()),
            Query(IncidentFilters {
                state: None,
                severity: None,
                rule_key: None,
                subject_kind: None,
                subject_key: None,
                limit: None,
            }),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(
            value["incidents"][0]["acknowledgment"]["acknowledgedByUsername"],
            "owner"
        );
        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["acknowledgment"]["acknowledgedByUsername"], "owner");
        assert_eq!(value["acknowledgment"]["acknowledgedAt"], acknowledged_at);
    }

    #[tokio::test]
    async fn acknowledging_an_incident_does_not_change_health_rule_state_or_notification_rows() {
        let (_dir, state) = test_state().await;
        let now = base_time();
        let incident_id = open_node_incident(&state, now).await;
        let pool = state.db().pool();

        async fn dump(pool: &sqlx::SqlitePool, sql: &str) -> Vec<String> {
            sqlx::query_scalar::<_, String>(sql)
                .fetch_all(pool)
                .await
                .unwrap()
        }

        let rule_state_sql = "SELECT rule_key || '|' || subject_kind || '|' || subject_key || '|' || state || '|' || since || '|' || COALESCE(pending_since, '') || '|' || COALESCE(firing_since, '') || '|' || COALESCE(recovering_since, '') || '|' || input_kind || '|' || COALESCE(input_value, '') || '|' || COALESCE(input_detail, '') || '|' || COALESCE(evidence_json, '') || '|' || evaluation_unavailable || '|' || last_evaluated_at FROM alert_rule_state ORDER BY rule_key, subject_key";
        let incident_sql = "SELECT incident_id || '|' || rule_key || '|' || rule_version || '|' || subject_kind || '|' || subject_key || '|' || severity || '|' || state || '|' || sequence || '|' || opened_at || '|' || COALESCE(resolved_at, '') || '|' || opened_evidence_json || '|' || COALESCE(resolved_evidence_json, '') FROM alert_incidents ORDER BY incident_id";
        let event_sql = "SELECT event_id || '|' || event_kind || '|' || COALESCE(incident_id, '') || '|' || COALESCE(rule_key, '') || '|' || COALESCE(subject_kind, '') || '|' || COALESCE(subject_key, '') || '|' || severity || '|' || summary || '|' || created_at FROM notification_events ORDER BY event_id";
        let delivery_sql = "SELECT delivery_id || '|' || event_id || '|' || channel_kind || '|' || destination || '|' || state || '|' || attempt_count || '|' || COALESCE(next_attempt_at, '') || '|' || COALESCE(last_attempt_at, '') || '|' || COALESCE(last_result, '') || '|' || COALESCE(last_error_kind, '') || '|' || COALESCE(retry_after_seconds, '') || '|' || created_at || '|' || updated_at FROM notification_deliveries ORDER BY delivery_id";

        let rule_state_before = dump(pool, rule_state_sql).await;
        let incidents_before = dump(pool, incident_sql).await;
        let events_before = dump(pool, event_sql).await;
        let deliveries_before = dump(pool, delivery_sql).await;
        assert!(!rule_state_before.is_empty());
        assert!(!incidents_before.is_empty());

        let response = acknowledge(&state, &incident_id, "csrf", session()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["recorded"], true);

        // Acknowledging one occurrence is a pure additive fact: the derived
        // health projection (Rule state and open Incidents) and every
        // Notification Event/Delivery row are byte-for-byte unchanged.
        assert_eq!(rule_state_before, dump(pool, rule_state_sql).await);
        assert_eq!(incidents_before, dump(pool, incident_sql).await);
        assert_eq!(events_before, dump(pool, event_sql).await);
        assert_eq!(deliveries_before, dump(pool, delivery_sql).await);

        // The acknowledgment is durable and audited; nothing else was written.
        let stored: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM incident_acknowledgments WHERE incident_id = ?",
        )
        .bind(&incident_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(stored, 1);
        let audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'incident_acknowledged' AND target_id = ?",
        )
        .bind(&incident_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(audit_count, 1);
    }

    #[tokio::test]
    async fn acknowledge_incident_survives_restart_and_recovery_and_does_not_carry_over() {
        let (dir, state) = test_state().await;
        let now = base_time();
        let incident_id = open_node_incident(&state, now).await;
        let response = acknowledge(&state, &incident_id, "csrf", session()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body_json(response).await;
        let acknowledged_at = value["acknowledgment"]["acknowledgedAt"]
            .as_str()
            .unwrap()
            .to_owned();
        drop(state);

        // Server restart: a fresh database handle on the same file (new
        // migrations are idempotent) sees the same durable facts.
        let db_path = dir.path().join("server.db");
        let database =
            crate::database::initialize(crate::database::ServerDatabaseConfig::new(&db_path))
                .await
                .unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&dir.path().join("pepper")).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth);
        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["acknowledgment"]["acknowledgedByUserId"], "owner");
        assert_eq!(value["acknowledgment"]["acknowledgedAt"], acknowledged_at);

        // Genuine recovery resolves the old Incident; its acknowledgment stays.
        // Recovery needs a fresh ok observation kept current across the whole
        // recovery duration (fresh-Known contract).
        let recovering_at = now + time::Duration::seconds(120);
        set_rpc_error(state.db().pool(), "ok", recovering_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            recovering_at,
        )
        .await
        .unwrap();
        drop(conn);
        set_rpc_error(state.db().pool(), "ok", now + time::Duration::seconds(250)).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            now + time::Duration::seconds(260),
        )
        .await
        .unwrap();
        drop(conn);
        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["state"], "resolved");
        assert_eq!(value["acknowledgment"]["acknowledgedByUserId"], "owner");

        // A genuinely recurring fault opens a NEW Incident that does not
        // inherit the acknowledgment.
        let recurrence = now + time::Duration::seconds(300);
        let new_incident_id = open_node_incident(&state, recurrence).await;
        assert_ne!(new_incident_id, incident_id);
        let response = alert_incident_detail(
            State(state.clone()),
            Path(new_incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["state"], "open");
        assert!(value["acknowledgment"].is_null());
        assert_eq!(value["sequence"], 2);
    }

    #[tokio::test]
    async fn concurrent_acknowledgments_keep_exactly_one_first_success() {
        let (_dir, state) = test_state().await;
        let incident_id = open_node_incident(&state, base_time()).await;
        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner-2', 'owner-2', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();

        let (first, second) = tokio::join!(
            acknowledge(&state, &incident_id, "csrf", session()),
            acknowledge(&state, &incident_id, "csrf", owner_session("owner-2")),
        );
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        let first = body_json(first).await;
        let second = body_json(second).await;
        let recorded = [&first["recorded"], &second["recorded"]]
            .iter()
            .filter(|value| value.as_bool() == Some(true))
            .count();
        assert_eq!(
            recorded, 1,
            "exactly one request records the acknowledgment"
        );
        assert_eq!(
            first["acknowledgment"]["acknowledgedByUserId"],
            second["acknowledgment"]["acknowledgedByUserId"]
        );
        assert_eq!(
            first["acknowledgment"]["acknowledgedAt"],
            second["acknowledgment"]["acknowledgedAt"]
        );
        let audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'incident_acknowledged' AND target_id = ?",
        )
        .bind(&incident_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audit_count, 1);
    }

    #[tokio::test]
    async fn acknowledge_incident_rejects_unknown_incident_and_bad_csrf() {
        let (_dir, state) = test_state().await;
        let response = acknowledge(&state, "no-such-incident", "csrf", session()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let value = body_json(response).await;
        assert_eq!(value["error"]["code"], "incident_not_found");

        let incident_id = open_node_incident(&state, base_time()).await;
        let response = acknowledge(&state, &incident_id, "wrong", session()).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM incident_acknowledgments")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    /// Issue #203 review R1: disabling and re-enabling a Rule through the real
    /// Admin endpoint with no evaluation in between must invalidate the
    /// in-flight recovery window. Otherwise evaluation simply stops while the
    /// window matures, and the next fresh Known observation falsely resolves a
    /// recovery that was never observed as sustained.
    #[tokio::test]
    async fn rule_reenable_without_an_evaluation_restarts_the_recovery_window() {
        let (_dir, state) = test_state().await;
        let now = base_time();
        let incident_id = open_node_incident(&state, now).await;
        let response = acknowledge(&state, &incident_id, "csrf", session()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let acknowledged_at = body_json(response).await["acknowledgment"]["acknowledgedAt"]
            .as_str()
            .unwrap()
            .to_owned();

        // Fresh ok evidence at +120s starts the recovery window.
        let recovering_at = now + time::Duration::seconds(120);
        set_rpc_error(state.db().pool(), "ok", recovering_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            recovering_at,
        )
        .await
        .unwrap();
        drop(conn);
        let (recovering_since, unavailable): (Option<String>, bool) = sqlx::query_as(
            "SELECT recovering_since, evaluation_unavailable FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            recovering_since.as_deref(),
            Some(format_rfc3339(recovering_at).as_str())
        );
        assert!(!unavailable);

        // Disable through the Admin endpoint. No evaluation runs while the Rule
        // is disabled, so only the configuration change can clear the window.
        let disable = update_alert_rule(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"expectedVersion":1,"enabled":false}"#),
        )
        .await;
        assert_eq!(disable.status(), StatusCode::OK);
        let (recovering_since, unavailable): (Option<String>, bool) = sqlx::query_as(
            "SELECT recovering_since, evaluation_unavailable FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(
            recovering_since.is_none(),
            "disabling without an evaluation invalidates the in-flight window"
        );
        assert!(unavailable);

        // Re-enable, still with no evaluation.
        let enable = update_alert_rule(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(br#"{"expectedVersion":2,"enabled":true}"#),
        )
        .await;
        assert_eq!(enable.status(), StatusCode::OK);

        // The next fresh Known observation must start a new window rather than
        // complete the pre-disable one.
        let fresh_at = now + time::Duration::seconds(400);
        set_rpc_error(state.db().pool(), "ok", fresh_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            fresh_at,
        )
        .await
        .unwrap();
        drop(conn);
        let (state_text, recovering_since): (String, Option<String>) = sqlx::query_as(
            "SELECT state, recovering_since FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(state_text, "recovering");
        assert_eq!(
            recovering_since.as_deref(),
            Some(format_rfc3339(fresh_at).as_str()),
            "re-enabling without an evaluation restarts the recovery window"
        );
        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["state"], "open");
        assert_eq!(value["currentRule"]["enabled"], true);
        assert_eq!(value["acknowledgment"]["acknowledgedAt"], acknowledged_at);
    }

    /// Issue #203 review R1: Node and Network overrides that can flip the
    /// effective enabled flag invalidate the affected subject's recovery
    /// window when they are upserted, and again when they are deleted.
    #[tokio::test]
    async fn override_enabled_changes_invalidate_the_recovery_window() {
        let (_dir, state) = test_state().await;
        let now = base_time();
        let incident_id = open_node_incident(&state, now).await;
        let response = acknowledge(&state, &incident_id, "csrf", session()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let acknowledged_at = body_json(response).await["acknowledgment"]["acknowledgedAt"]
            .as_str()
            .unwrap()
            .to_owned();

        let recovering_at = now + time::Duration::seconds(120);
        set_rpc_error(state.db().pool(), "ok", recovering_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            recovering_at,
        )
        .await
        .unwrap();
        drop(conn);

        // A Node override that disables the Rule clears the window.
        let upsert_node = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"expectedVersion":1,"scopeKind":"node","scopeValue":"node-a","enabled":false}"#,
            ),
        )
        .await;
        assert_eq!(upsert_node.status(), StatusCode::OK);
        let (recovering_since, unavailable): (Option<String>, bool) = sqlx::query_as(
            "SELECT recovering_since, evaluation_unavailable FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(
            recovering_since.is_none(),
            "a disabling Node override invalidates the recovery window"
        );
        assert!(unavailable);

        // Deleting the override restores the global enabled flag and clears the
        // window again (still no evaluation in between).
        let delete_node = delete_rule_override(
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
        assert_eq!(delete_node.status(), StatusCode::OK);
        let (recovering_since, unavailable): (Option<String>, bool) = sqlx::query_as(
            "SELECT recovering_since, evaluation_unavailable FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(recovering_since.is_none());
        assert!(unavailable);

        // A Network override on the subject's Network invalidates it too. The
        // Node upsert and its removal already advanced the composed revision
        // twice (issue #204 Story 19), so this save carries revision 3.
        let upsert_network = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"expectedVersion":3,"scopeKind":"network","scopeValue":"mainnet","enabled":false}"#,
            ),
        )
        .await;
        assert_eq!(upsert_network.status(), StatusCode::OK);
        let (recovering_since, unavailable): (Option<String>, bool) = sqlx::query_as(
            "SELECT recovering_since, evaluation_unavailable FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(recovering_since.is_none());
        assert!(unavailable);

        // While the Network override keeps the Rule disabled, a fresh Known
        // observation is skipped and restores no window.
        let fresh_at = now + time::Duration::seconds(400);
        set_rpc_error(state.db().pool(), "ok", fresh_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            fresh_at,
        )
        .await
        .unwrap();
        drop(conn);
        let (recovering_since, _): (Option<String>, bool) = sqlx::query_as(
            "SELECT recovering_since, evaluation_unavailable FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(
            recovering_since.is_none(),
            "a disabled Rule keeps no recovery continuity while the override is active"
        );

        let delete_network = delete_rule_override(
            State(state.clone()),
            Path((
                "node.rpc_unreachable".to_owned(),
                "network".to_owned(),
                "mainnet".to_owned(),
            )),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(delete_network.status(), StatusCode::OK);

        let reopened_at = now + time::Duration::seconds(700);
        set_rpc_error(state.db().pool(), "ok", reopened_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            reopened_at,
        )
        .await
        .unwrap();
        drop(conn);
        let (state_text, recovering_since): (String, Option<String>) = sqlx::query_as(
            "SELECT state, recovering_since FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(state_text, "recovering");
        assert_eq!(
            recovering_since.as_deref(),
            Some(format_rfc3339(reopened_at).as_str())
        );
        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["state"], "open");
        assert_eq!(value["acknowledgment"]["acknowledgedAt"], acknowledged_at);
    }

    /// Issue #203 review B1: replacing an override with a request that omits
    /// `enabled` clears the explicit value back to inheritance. That can change
    /// the effective enabled flag through the Network/Node layering, so it must
    /// invalidate the recovery window even though the new request carries no
    /// `enabled` field.
    #[tokio::test]
    async fn override_inheritance_change_invalidates_the_recovery_window() {
        let (_dir, state) = test_state().await;
        let now = base_time();
        let incident_id = open_node_incident(&state, now).await;
        let response = acknowledge(&state, &incident_id, "csrf", session()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let acknowledged_at = body_json(response).await["acknowledgment"]["acknowledgedAt"]
            .as_str()
            .unwrap()
            .to_owned();

        // Global enabled, Network disabled, Node explicitly enabled: node-a is
        // effectively enabled by the Node override alone.
        let upsert_network = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"expectedVersion":1,"scopeKind":"network","scopeValue":"mainnet","enabled":false}"#,
            ),
        )
        .await;
        assert_eq!(upsert_network.status(), StatusCode::OK);
        assert_eq!(body_json(upsert_network).await["version"], 2);
        let upsert_node = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"expectedVersion":2,"scopeKind":"node","scopeValue":"node-a","enabled":true}"#,
            ),
        )
        .await;
        assert_eq!(upsert_node.status(), StatusCode::OK);

        let recovering_at = now + time::Duration::seconds(120);
        set_rpc_error(state.db().pool(), "ok", recovering_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            recovering_at,
        )
        .await
        .unwrap();
        drop(conn);
        let (recovering_since, _): (Option<String>, bool) = sqlx::query_as(
            "SELECT recovering_since, evaluation_unavailable FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            recovering_since.as_deref(),
            Some(format_rfc3339(recovering_at).as_str())
        );

        // Dropping the Node override's `enabled` makes node-a inherit the
        // disabled Network override, so the window must not survive.
        let upsert_node_inherit = upsert_rule_override(
            State(state.clone()),
            Path("node.rpc_unreachable".to_owned()),
            mutation_headers("csrf"),
            Extension(session()),
            Extension(request_id()),
            axum::body::Bytes::from_static(
                br#"{"expectedVersion":3,"scopeKind":"node","scopeValue":"node-a","severity":"warning"}"#,
            ),
        )
        .await;
        assert_eq!(upsert_node_inherit.status(), StatusCode::OK);
        let (recovering_since, unavailable): (Option<String>, bool) = sqlx::query_as(
            "SELECT recovering_since, evaluation_unavailable FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(
            recovering_since.is_none(),
            "clearing an explicit enabled override invalidates the recovery window"
        );
        assert!(unavailable);

        // The Incident keeps its original facts and confirmation.
        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["state"], "open");
        assert_eq!(value["acknowledgment"]["acknowledgedAt"], acknowledged_at);
    }

    /// Issue #203 review B2: a disabled Rule must not let a later fresh Known
    /// observation borrow a recovery window recorded before evaluation stopped.
    #[tokio::test]
    async fn disabled_rule_does_not_borrow_recovery_continuity() {
        let (_dir, state) = test_state().await;
        let now = base_time();
        let incident_id = open_node_incident(&state, now).await;

        let response = acknowledge(&state, &incident_id, "csrf", session()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let acknowledged_at = body_json(response).await["acknowledgment"]["acknowledgedAt"]
            .as_str()
            .unwrap()
            .to_owned();

        // Fresh ok evidence starts the recovery window.
        let recovering_at = now + time::Duration::seconds(120);
        set_rpc_error(state.db().pool(), "ok", recovering_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            recovering_at,
        )
        .await
        .unwrap();
        drop(conn);
        let (recovering_since, unavailable): (Option<String>, bool) = sqlx::query_as(
            "SELECT recovering_since, evaluation_unavailable FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            recovering_since.as_deref(),
            Some(format_rfc3339(recovering_at).as_str())
        );
        assert!(!unavailable);

        // Disable the Rule: evaluation stops and the in-flight window must be
        // invalidated rather than left to mature while nothing is observed.
        sqlx::query("UPDATE alert_rules SET enabled = 0 WHERE rule_key = 'node.rpc_unreachable'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let disabled_at = recovering_at + time::Duration::seconds(30);
        set_rpc_error(state.db().pool(), "ok", disabled_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            disabled_at,
        )
        .await
        .unwrap();
        drop(conn);
        let (recovering_since, unavailable): (Option<String>, bool) = sqlx::query_as(
            "SELECT recovering_since, evaluation_unavailable FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(recovering_since.is_none());
        assert!(unavailable);
        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["state"], "open");
        assert_eq!(value["currentRule"]["enabled"], false);
        assert_eq!(value["acknowledgment"]["acknowledgedAt"], acknowledged_at);

        // Re-enable: the next fresh Known observation restarts the full
        // recovery duration instead of resolving on pre-disable continuity.
        sqlx::query("UPDATE alert_rules SET enabled = 1 WHERE rule_key = 'node.rpc_unreachable'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let fresh_at = now + time::Duration::seconds(400);
        set_rpc_error(state.db().pool(), "ok", fresh_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            fresh_at,
        )
        .await
        .unwrap();
        drop(conn);
        let (state_text, recovering_since): (String, Option<String>) = sqlx::query_as(
            "SELECT state, recovering_since FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(state_text, "recovering");
        assert_eq!(
            recovering_since.as_deref(),
            Some(format_rfc3339(fresh_at).as_str())
        );
        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(body_json(response).await["state"], "open");

        // Sustained fresh Known recovery after re-enabling still resolves.
        let resolved_at = fresh_at + time::Duration::seconds(121);
        set_rpc_error(state.db().pool(), "ok", resolved_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            resolved_at,
        )
        .await
        .unwrap();
        drop(conn);
        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["state"], "resolved");
        assert_eq!(value["currentRule"]["enabled"], true);
        assert_eq!(value["acknowledgment"]["acknowledgedAt"], acknowledged_at);
    }

    /// Issue #203 review B2: a Server restart is an observation gap, so an
    /// in-flight recovery window does not survive it.
    #[tokio::test]
    async fn recovery_window_does_not_survive_a_server_restart() {
        let (dir, state) = test_state().await;
        let now = base_time();
        let incident_id = open_node_incident(&state, now).await;
        let response = acknowledge(&state, &incident_id, "csrf", session()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let acknowledged_at = body_json(response).await["acknowledgment"]["acknowledgedAt"]
            .as_str()
            .unwrap()
            .to_owned();

        let recovering_at = now + time::Duration::seconds(120);
        set_rpc_error(state.db().pool(), "ok", recovering_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            recovering_at,
        )
        .await
        .unwrap();
        drop(conn);
        drop(state);

        // Restart on the same database file.
        let db_path = dir.path().join("server.db");
        let database =
            crate::database::initialize(crate::database::ServerDatabaseConfig::new(&db_path))
                .await
                .unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&dir.path().join("pepper")).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth);

        let (recovering_since, unavailable): (Option<String>, bool) = sqlx::query_as(
            "SELECT recovering_since, evaluation_unavailable FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(recovering_since.is_none());
        assert!(unavailable);

        // The first fresh Known observation after the restart restarts the
        // window; it must not resolve on timestamps from before the gap.
        let fresh_at = now + time::Duration::seconds(250);
        set_rpc_error(state.db().pool(), "ok", fresh_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            fresh_at,
        )
        .await
        .unwrap();
        drop(conn);
        let (state_text, recovering_since): (String, Option<String>) = sqlx::query_as(
            "SELECT state, recovering_since FROM alert_rule_state \
             WHERE rule_key = 'node.rpc_unreachable' AND subject_key = 'node-a'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(state_text, "recovering");
        assert_eq!(
            recovering_since.as_deref(),
            Some(format_rfc3339(fresh_at).as_str())
        );
        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["state"], "open");
        assert_eq!(value["acknowledgment"]["acknowledgedAt"], acknowledged_at);

        // Sustained fresh evidence after the restart resolves normally.
        let resolved_at = fresh_at + time::Duration::seconds(121);
        set_rpc_error(state.db().pool(), "ok", resolved_at).await;
        let mut conn = state.db().pool().acquire().await.unwrap();
        crate::alerts::evaluate_rule(
            &mut conn,
            "node.rpc_unreachable",
            SubjectKind::Node,
            "node-a",
            resolved_at,
        )
        .await
        .unwrap();
        drop(conn);
        let response = alert_incident_detail(
            State(state.clone()),
            Path(incident_id.clone()),
            Extension(request_id()),
        )
        .await;
        let value = body_json(response).await;
        assert_eq!(value["state"], "resolved");
        assert_eq!(value["acknowledgment"]["acknowledgedAt"], acknowledged_at);
    }
}
