//! Owner-only Validator Registry and Node Validator Link HTTP boundary.
//!
//! These routes expose only Server-owned Validator/link DTOs. Agent reports,
//! consensus membership, provider values, endpoints, and raw diagnostics are
//! not accepted as an implicit relationship source.

use super::admin::{mutation_error, mutation_guard_ok};
use super::{AppState, AuthenticatedSession, RequestId};
use crate::validator::{
    self, NodeValidatorLinkRecord, ValidatorCounterHistoryRecord, ValidatorError,
    ValidatorRankingHistoryRecord, ValidatorRecord,
};
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Validator {
    pub validator_id: String,
    pub network_key: String,
    pub validator_node_id: String,
    pub display_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub link_count: i64,
    pub insight: Option<AdminValidatorInsight>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorInsight {
    pub validator_node_id: String,
    pub display_name: Option<String>,
    pub state: String,
    pub freshness: String,
    pub outcome: String,
    pub source: Option<String>,
    pub provider_timestamp: Option<String>,
    pub received_at: Option<String>,
    pub attempted_at: Option<String>,
    pub last_good_received_at: Option<String>,
    pub rank: Option<i64>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
    pub counter_state: String,
    /// Canonical last-good Validator Activity (#173): Observing stands for an
    /// authoritative absence as well as for evidence that cannot be observed.
    pub activity: Option<String>,
    /// Currency of `activity`: `current`, `stale`, or `unknown`.
    pub activity_state: String,
    /// Current Validator Status: `validator`, `not_validator`, or `unknown`.
    pub current_validator_status: String,
    /// Currency of the Current Validator Status verdict: `current`, `stale`,
    /// or `unknown`.
    pub current_validator_status_state: String,
    /// `locked` or `exiting` while the staking identity is confirmed valid but
    /// is not normally participating.
    pub current_validator_status_qualifier: Option<String>,
    /// Server-computed age of the last-good observation in whole seconds;
    /// never 0 for a Validator that has never refreshed successfully.
    pub last_good_age_seconds: Option<i64>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorHistoryLink {
    pub link_id: String,
    pub node_id: String,
    /// Legacy manual role; automatic Links carry none (#173).
    pub role: Option<String>,
    pub valid_from: String,
    pub valid_until: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorHistoryEntry {
    pub history_id: String,
    pub kind: String,
    pub observed_at: String,
    pub provider_timestamp: Option<String>,
    pub previous_rank: Option<i64>,
    pub current_rank: Option<i64>,
    pub candidate_observed_at: Option<String>,
    pub candidate_provider_timestamp: Option<String>,
    pub counter_name: Option<String>,
    pub previous_value: Option<String>,
    pub current_value: Option<String>,
    pub observation_key: String,
    pub links: Vec<AdminValidatorHistoryLink>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorHistoryResponse {
    pub validator_id: String,
    pub network_key: String,
    pub entries: Vec<AdminValidatorHistoryEntry>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorHistoryQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorDailySnapshot {
    pub local_date: String,
    pub month_key: String,
    pub timezone: String,
    pub sample_at: String,
    pub received_at: String,
    pub provider_timestamp: Option<String>,
    pub source: String,
    pub rank: Option<i64>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorMonthlyAggregate {
    pub month_key: String,
    pub timezone: String,
    pub snapshot_count: i64,
    pub first_sample_at: String,
    pub last_sample_at: String,
    pub rank_min: Option<i64>,
    pub rank_max: Option<i64>,
    pub rank_last: Option<i64>,
    pub stake_last: Option<String>,
    pub reward_last: Option<String>,
    pub reward_rate_last: Option<String>,
    pub delegator_count_last: Option<i64>,
    pub epoch_last: Option<i64>,
    pub block_count_last: Option<i64>,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorAnalyticsResponse {
    pub validator_id: String,
    pub state: String,
    pub freshness: String,
    pub daily: Vec<AdminValidatorDailySnapshot>,
    pub monthly: Vec<AdminValidatorMonthlyAggregate>,
}

async fn admin_history_entry_links(
    state: &AppState,
    validator_id: &str,
    observed_at: &str,
) -> Result<Vec<AdminValidatorHistoryLink>, ValidatorError> {
    Ok(
        validator::list_link_context_at(state.db(), validator_id, observed_at, false)
            .await?
            .into_iter()
            .map(|link| AdminValidatorHistoryLink {
                link_id: link.link_id,
                node_id: link.node_id,
                role: link.role,
                valid_from: link.valid_from,
                valid_until: link.valid_until,
            })
            .collect(),
    )
}

async fn admin_history_entries(
    state: &AppState,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<AdminValidatorHistoryEntry>, ValidatorError> {
    let rankings = validator::list_ranking_history(state.db(), validator_id, limit).await?;
    let counters = validator::list_counter_history(state.db(), validator_id, limit).await?;
    let mut entries = Vec::with_capacity(rankings.len() + counters.len());
    for record in rankings {
        entries.push(admin_ranking_history_entry(state, record).await?);
    }
    for record in counters {
        entries.push(admin_counter_history_entry(state, record).await?);
    }
    entries.sort_by(|left, right| right.observed_at.cmp(&left.observed_at));
    entries.truncate(limit as usize);
    Ok(entries)
}

async fn admin_ranking_history_entry(
    state: &AppState,
    record: ValidatorRankingHistoryRecord,
) -> Result<AdminValidatorHistoryEntry, ValidatorError> {
    let links = admin_history_entry_links(state, &record.validator_id, &record.observed_at).await?;
    Ok(AdminValidatorHistoryEntry {
        history_id: record.history_id,
        kind: "ranking_changed".to_owned(),
        observed_at: record.observed_at,
        provider_timestamp: record.provider_timestamp,
        previous_rank: record.previous_rank,
        current_rank: Some(record.current_rank),
        candidate_observed_at: record.candidate_observed_at,
        candidate_provider_timestamp: record.candidate_provider_timestamp,
        counter_name: None,
        previous_value: None,
        current_value: None,
        observation_key: record.observation_key,
        links,
    })
}

async fn admin_counter_history_entry(
    state: &AppState,
    record: ValidatorCounterHistoryRecord,
) -> Result<AdminValidatorHistoryEntry, ValidatorError> {
    let links = admin_history_entry_links(state, &record.validator_id, &record.observed_at).await?;
    Ok(AdminValidatorHistoryEntry {
        history_id: record.history_id,
        kind: "counter_reset_or_correction".to_owned(),
        observed_at: record.observed_at,
        provider_timestamp: record.provider_timestamp,
        previous_rank: None,
        current_rank: None,
        candidate_observed_at: None,
        candidate_provider_timestamp: None,
        counter_name: Some(record.counter_name),
        previous_value: Some(record.previous_value),
        current_value: Some(record.current_value),
        observation_key: record.observation_key,
        links,
    })
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeValidatorLink {
    pub link_id: String,
    pub node_id: String,
    pub validator_id: String,
    pub network_key: String,
    pub validator_node_id: String,
    pub node_display_name: Option<String>,
    /// Legacy manual role; automatic Links carry none (#173).
    pub role: Option<String>,
    pub valid_from: String,
    pub valid_until: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorDetail {
    #[serde(flatten)]
    pub validator: Validator,
    pub links: Vec<NodeValidatorLink>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorMutationResponse {
    pub validator: Validator,
    pub request_id: String,
    pub audit_event_id: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorLinkMutationResponse {
    pub link: NodeValidatorLink,
    pub request_id: String,
    pub audit_event_id: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorListQuery {
    pub network_key: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorLinkListQuery {
    pub network_key: Option<String>,
    pub validator_id: Option<String>,
    pub node_id: Option<String>,
}

pub(crate) fn error_response(request_id: &str, error: ValidatorError) -> Response {
    let (status, code) = match &error {
        ValidatorError::NetworkNotFound
        | ValidatorError::ValidatorNotFound
        | ValidatorError::NodeNotFound
        | ValidatorError::LinkNotFound => (StatusCode::NOT_FOUND, "not_found"),
        ValidatorError::ValidatorAlreadyExists | ValidatorError::LinkOverlap => {
            (StatusCode::CONFLICT, "conflict")
        }
        ValidatorError::NodeNotActive
        | ValidatorError::NetworkMismatch
        | ValidatorError::EndBeforeStart
        | ValidatorError::LinkAlreadyEnded
        | ValidatorError::LinkReplacementMustAdvance => {
            (StatusCode::CONFLICT, "invalid_relationship")
        }
        ValidatorError::InvalidValidatorNodeId
        | ValidatorError::InvalidDisplayName
        | ValidatorError::InvalidRole
        | ValidatorError::InvalidTimestamp(_)
        | ValidatorError::InvalidValidity
        | ValidatorError::InvalidProviderObservation(_)
        | ValidatorError::InvalidTimezone(_)
        | ValidatorError::InvalidTrendWindow(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
        ValidatorError::Database(_) | ValidatorError::Alert(_) => {
            (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        }
    };
    let message = match error {
        ValidatorError::Database(_) | ValidatorError::Alert(_) => {
            "server database is unavailable".to_owned()
        }
        error => error.to_string(),
    };
    (
        status,
        Json(crate::http::ApiErrorBody::with_message(
            code, message, request_id,
        )),
    )
        .into_response()
}

/// One Node's automatic Validator identity coverage on the Owner-only Admin
/// surface (#218, main design §15.4). Every field is Server-owned evidence: the
/// discovery state, the sanitized reason, and the currently open automatic Link
/// interval. Nothing here is a manual role, an ownership claim, or a
/// consensus-membership statement.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminNodeValidatorIdentity {
    pub node_id: String,
    pub node_display_name: Option<String>,
    pub network_key: String,
    pub lifecycle: String,
    /// Discovery state of the last evaluation, or `not_evaluated` when the
    /// discovery dimension has never examined this Node.
    pub state: String,
    /// Sanitized explanation carried by every state except `identified`.
    pub reason: Option<String>,
    /// The full P2P public key currently observed from this Node.
    pub observed_validator_node_key: Option<String>,
    /// The Validator of the currently open automatic Link interval, if any.
    pub validator_id: Option<String>,
    /// That Validator's own chain identity key.
    pub validator_node_key: Option<String>,
    /// Whether the Public projection also shows this association: Public
    /// requires an Active Node, so an inactive Node keeps its identity history
    /// without a projected correspondence.
    pub association_effective: bool,
    /// When the discovery dimension last evaluated this Node.
    pub evaluated_at: Option<String>,
}

impl From<validator::NodeValidatorIdentityRecord> for AdminNodeValidatorIdentity {
    fn from(record: validator::NodeValidatorIdentityRecord) -> Self {
        let state = record
            .state
            .clone()
            .unwrap_or_else(|| "not_evaluated".to_owned());
        Self {
            association_effective: record.association_effective(),
            reason: validator::automatic_identity_reason(Some(&state)),
            node_id: record.node_id,
            node_display_name: record.node_display_name,
            network_key: record.network_key,
            lifecycle: record.lifecycle,
            state,
            observed_validator_node_key: record.observed_node_key,
            validator_id: record.validator_id,
            validator_node_key: record.validator_node_key,
            evaluated_at: record.evaluated_at,
        }
    }
}

async fn validator_dto(
    state: &AppState,
    record: ValidatorRecord,
) -> Result<Validator, ValidatorError> {
    let link_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM node_validator_links WHERE validator_id = ?")
            .bind(&record.validator_id)
            .fetch_one(state.db().pool())
            .await?;
    let insight = validator::load_insight(state.db(), &record.validator_id)
        .await?
        .map(|row| {
            let now = crate::auth::now_utc();
            let freshness = validator::freshness(
                row.last_good_received_at.as_deref(),
                now,
                state.validator_freshness_seconds(),
            );
            let outcome = row.outcome.as_str();
            let (activity, activity_state) =
                validator::project_activity(outcome, row.activity.as_deref(), freshness);
            // Current Validator Status is projected from the same canonical
            // outcome/Activity/freshness inputs as every other surface, so a
            // Provider failure can neither invent an absence nor a fresh zero.
            let status = validator::current_validator_status(
                Some(outcome),
                row.activity.as_deref(),
                freshness,
            );
            AdminValidatorInsight {
                validator_node_id: record.validator_node_id.clone(),
                display_name: record.display_name.clone(),
                state: if outcome == "success" {
                    freshness.to_owned()
                } else {
                    row.outcome.clone()
                },
                freshness: freshness.to_owned(),
                outcome: row.outcome,
                source: row.source,
                provider_timestamp: row.provider_timestamp,
                received_at: row.last_good_received_at.clone(),
                attempted_at: Some(row.last_attempt_received_at),
                last_good_received_at: row.last_good_received_at.clone(),
                rank: row.rank,
                stake_amount: row.stake_amount,
                reward_amount: row.reward_amount,
                reward_rate: row.reward_rate,
                delegator_count: row.delegator_count,
                epoch: row.epoch,
                block_count: row.block_count,
                counter_state: row.counter_state,
                activity: Some(activity),
                activity_state,
                current_validator_status: status.status.as_str().to_owned(),
                current_validator_status_state: status.state.to_owned(),
                current_validator_status_qualifier: status
                    .qualifier
                    .map(|value| value.as_str().to_owned()),
                last_good_age_seconds: validator::last_good_age_seconds(
                    row.last_good_received_at.as_deref(),
                    now,
                ),
                diagnostic: row.diagnostic,
            }
        })
        .or_else(|| {
            Some(AdminValidatorInsight {
                validator_node_id: record.validator_node_id.clone(),
                display_name: record.display_name.clone(),
                state: "not_configured".to_owned(),
                freshness: "unknown".to_owned(),
                outcome: "not_configured".to_owned(),
                source: Some("disabled".to_owned()),
                provider_timestamp: None,
                received_at: None,
                attempted_at: None,
                last_good_received_at: None,
                rank: None,
                stake_amount: None,
                reward_amount: None,
                reward_rate: None,
                delegator_count: None,
                epoch: None,
                block_count: None,
                counter_state: "normal".to_owned(),
                // Without a configured Provider the identity is Unknown, not
                // absent: no Validator, no Activity label, and no age.
                activity: Some("unknown".to_owned()),
                activity_state: "unknown".to_owned(),
                current_validator_status: "unknown".to_owned(),
                current_validator_status_state: "unknown".to_owned(),
                current_validator_status_qualifier: None,
                last_good_age_seconds: None,
                diagnostic: None,
            })
        });
    Ok(Validator {
        validator_id: record.validator_id,
        network_key: record.network_key,
        validator_node_id: record.validator_node_id,
        display_name: record.display_name,
        created_at: record.created_at,
        updated_at: record.updated_at,
        link_count,
        insight,
    })
}

async fn link_dto(
    state: &AppState,
    record: NodeValidatorLinkRecord,
) -> Result<NodeValidatorLink, ValidatorError> {
    let row = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT v.network_key, v.validator_node_id, n.display_name FROM validators v JOIN nodes n ON n.node_id = ? WHERE v.validator_id = ?",
    )
    .bind(&record.node_id)
    .bind(&record.validator_id)
    .fetch_optional(state.db().pool())
    .await?;
    let Some((network_key, validator_node_id, node_display_name)) = row else {
        return Err(ValidatorError::LinkNotFound);
    };
    Ok(NodeValidatorLink {
        link_id: record.link_id,
        node_id: record.node_id,
        validator_id: record.validator_id,
        network_key,
        validator_node_id,
        node_display_name,
        role: record.role,
        valid_from: record.valid_from,
        valid_until: record.valid_until,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/validators",
    tag = "admin",
    params(ValidatorListQuery),
    responses((status = 200, body = [Validator]), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_validators(
    State(state): State<AppState>,
    Query(query): Query<ValidatorListQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match validator::list_validators(&state.database(), query.network_key.as_deref()).await {
        Ok(records) => {
            let mut result = Vec::with_capacity(records.len());
            for record in records {
                match validator_dto(&state, record).await {
                    Ok(value) => result.push(value),
                    Err(error) => return error_response(&request_id.0, error),
                }
            }
            Json(result).into_response()
        }
        Err(error) => error_response(&request_id.0, error),
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/validators/{validator_id}",
    tag = "admin",
    params(("validator_id" = String, Path, description = "Validator ID")),
    responses((status = 200, body = ValidatorDetail), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_validator_detail(
    State(state): State<AppState>,
    Path(validator_id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let Some(record) = (match validator::get_validator(&state.database(), &validator_id).await {
        Ok(value) => value,
        Err(error) => return error_response(&request_id.0, error),
    }) else {
        return error_response(&request_id.0, ValidatorError::ValidatorNotFound);
    };
    let validator = match validator_dto(&state, record).await {
        Ok(value) => value,
        Err(error) => return error_response(&request_id.0, error),
    };
    let records =
        match validator::list_links(&state.database(), None, Some(&validator_id), None).await {
            Ok(value) => value,
            Err(error) => return error_response(&request_id.0, error),
        };
    let mut links = Vec::with_capacity(records.len());
    for record in records {
        match link_dto(&state, record).await {
            Ok(value) => links.push(value),
            Err(error) => return error_response(&request_id.0, error),
        }
    }
    Json(ValidatorDetail { validator, links }).into_response()
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/validator-links/{link_id}",
    tag = "admin",
    params(("link_id" = String, Path, description = "Node Validator Link ID")),
    responses((status = 200, body = NodeValidatorLink), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_validator_link_detail(
    State(state): State<AppState>,
    Path(link_id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let record = match validator::get_link(&state.database(), &link_id).await {
        Ok(Some(value)) => value,
        Ok(None) => return error_response(&request_id.0, ValidatorError::LinkNotFound),
        Err(error) => return error_response(&request_id.0, error),
    };
    match link_dto(&state, record).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => error_response(&request_id.0, error),
    }
}

/// Manual Validator registration, explicit binding and role management were
/// retired by #173: the Server now identifies a Node Validator Link from the
/// validated Network and the observed full P2P public key. The stored legacy
/// rows are deleted by the one-time migration (#174); until then they are
/// never a Public association or a fallback. The endpoints remain registered
/// only to answer with an explicit retirement status instead of a silent
/// success that no longer has an effect.
fn manual_validator_management_retired(request_id: &str) -> Response {
    mutation_error(
        request_id,
        StatusCode::GONE,
        "manual_validator_management_retired",
        "manual Validator management was retired; Validators are identified automatically",
    )
}

#[utoipa::path(
    post,
    path = "/api/admin/v1/networks/{network_key}/validators",
    tag = "admin",
    params(("network_key" = String, Path, description = "Registered Network key")),
    responses((status = 410, body = crate::http::ApiErrorBody), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 400, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn create_validator(
    State(state): State<AppState>,
    Path(_network_key): Path<String>,
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
    manual_validator_management_retired(&request_id.0)
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/validator-links",
    tag = "admin",
    params(ValidatorLinkListQuery),
    responses((status = 200, body = [NodeValidatorLink]), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_validator_links(
    State(state): State<AppState>,
    Query(query): Query<ValidatorLinkListQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let records = match validator::list_links(
        &state.database(),
        query.node_id.as_deref(),
        query.validator_id.as_deref(),
        query.network_key.as_deref(),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return error_response(&request_id.0, error),
    };
    let mut result = Vec::with_capacity(records.len());
    for record in records {
        match link_dto(&state, record).await {
            Ok(value) => result.push(value),
            Err(error) => return error_response(&request_id.0, error),
        }
    }
    Json(result).into_response()
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/validator-links",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    responses((status = 200, body = [NodeValidatorLink]), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_node_validator_links(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let records = match validator::list_links(&state.database(), Some(&node_id), None, None).await {
        Ok(value) => value,
        Err(error) => return error_response(&request_id.0, error),
    };
    if records.is_empty() {
        match sqlx::query_scalar::<_, i64>("SELECT 1 FROM nodes WHERE node_id = ?")
            .bind(&node_id)
            .fetch_optional(state.db().pool())
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => return error_response(&request_id.0, ValidatorError::NodeNotFound),
            Err(error) => {
                return error_response(&request_id.0, ValidatorError::Database(error));
            }
        }
    }
    let mut result = Vec::with_capacity(records.len());
    for record in records {
        match link_dto(&state, record).await {
            Ok(value) => result.push(value),
            Err(error) => return error_response(&request_id.0, error),
        }
    }
    Json(result).into_response()
}

#[utoipa::path(
    post,
    path = "/api/admin/v1/nodes/{node_id}/validator-links",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    responses((status = 410, body = crate::http::ApiErrorBody), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 400, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn create_node_validator_link(
    State(state): State<AppState>,
    Path(_node_id): Path<String>,
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
    manual_validator_management_retired(&request_id.0)
}

#[utoipa::path(
    put,
    path = "/api/admin/v1/validator-links/{link_id}",
    tag = "admin",
    params(("link_id" = String, Path, description = "Node Validator Link ID")),
    responses((status = 410, body = crate::http::ApiErrorBody), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn update_validator_link(
    State(state): State<AppState>,
    Path(_link_id): Path<String>,
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
    manual_validator_management_retired(&request_id.0)
}

#[utoipa::path(
    post,
    path = "/api/admin/v1/validator-links/{link_id}/end",
    tag = "admin",
    params(("link_id" = String, Path, description = "Node Validator Link ID")),
    responses((status = 410, body = crate::http::ApiErrorBody), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn end_validator_link(
    State(state): State<AppState>,
    Path(_link_id): Path<String>,
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
    manual_validator_management_retired(&request_id.0)
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/validators/{validator_id}/history",
    tag = "admin",
    params(("validator_id" = String, Path, description = "Validator ID"), ValidatorHistoryQuery),
    responses((status = 200, body = AdminValidatorHistoryResponse), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_validator_history(
    State(state): State<AppState>,
    Path(validator_id): Path<String>,
    Query(query): Query<ValidatorHistoryQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let Some(record) = (match validator::get_validator(&state.database(), &validator_id).await {
        Ok(value) => value,
        Err(error) => return error_response(&request_id.0, error),
    }) else {
        return error_response(&request_id.0, ValidatorError::ValidatorNotFound);
    };
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    match admin_history_entries(&state, &validator_id, limit).await {
        Ok(entries) => Json(AdminValidatorHistoryResponse {
            validator_id,
            network_key: record.network_key,
            entries,
        })
        .into_response(),
        Err(error) => error_response(&request_id.0, error),
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/validators/{validator_id}/analytics",
    tag = "admin",
    params(("validator_id" = String, Path, description = "Validator ID"), ValidatorHistoryQuery),
    responses((status = 200, body = AdminValidatorAnalyticsResponse), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_validator_analytics(
    State(state): State<AppState>,
    Path(validator_id): Path<String>,
    Query(query): Query<ValidatorHistoryQuery>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let Some(insight) = (match validator::load_insight(&state.database(), &validator_id).await {
        Ok(value) => value,
        Err(error) => return error_response(&request_id.0, error),
    }) else {
        if validator::get_validator(&state.database(), &validator_id)
            .await
            .ok()
            .flatten()
            .is_none()
        {
            return error_response(&request_id.0, ValidatorError::ValidatorNotFound);
        }
        return Json(AdminValidatorAnalyticsResponse {
            validator_id,
            state: "unknown".to_owned(),
            freshness: "unknown".to_owned(),
            daily: Vec::new(),
            monthly: Vec::new(),
        })
        .into_response();
    };
    let limit = query.limit.unwrap_or(31).clamp(1, 366);
    let daily = match validator::list_daily_snapshots(&state.database(), &validator_id, limit).await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|row| AdminValidatorDailySnapshot {
                local_date: row.local_date,
                month_key: row.month_key,
                timezone: row.timezone,
                sample_at: row.sample_at,
                received_at: row.received_at,
                provider_timestamp: row.provider_timestamp,
                source: row.source,
                rank: row.rank,
                stake_amount: row.stake_amount,
                reward_amount: row.reward_amount,
                reward_rate: row.reward_rate,
                delegator_count: row.delegator_count,
                epoch: row.epoch,
                block_count: row.block_count,
            })
            .collect(),
        Err(error) => return error_response(&request_id.0, error),
    };
    let monthly =
        match validator::list_monthly_aggregates(&state.database(), &validator_id, limit).await {
            Ok(rows) => rows
                .into_iter()
                .map(|row| AdminValidatorMonthlyAggregate {
                    month_key: row.month_key,
                    timezone: row.timezone,
                    snapshot_count: row.snapshot_count,
                    first_sample_at: row.first_sample_at,
                    last_sample_at: row.last_sample_at,
                    rank_min: row.rank_min,
                    rank_max: row.rank_max,
                    rank_last: row.rank_last,
                    stake_last: row.stake_last,
                    reward_last: row.reward_last,
                    reward_rate_last: row.reward_rate_last,
                    delegator_count_last: row.delegator_count_last,
                    epoch_last: row.epoch_last,
                    block_count_last: row.block_count_last,
                    updated_at: row.updated_at,
                })
                .collect(),
            Err(error) => return error_response(&request_id.0, error),
        };
    let freshness = validator::freshness(
        insight.last_good_received_at.as_deref(),
        crate::auth::now_utc(),
        state.validator_freshness_seconds(),
    );
    let state = if insight.outcome == "success" {
        freshness
    } else {
        insight.outcome.as_str()
    };
    Json(AdminValidatorAnalyticsResponse {
        validator_id,
        state: state.to_owned(),
        freshness: freshness.to_owned(),
        daily,
        monthly,
    })
    .into_response()
}
/// Automatic Validator identity coverage for every Node (#218). This is the
/// Admin-only counterpart of the Public per-Node identity fields: it exposes
/// the discovery state for Nodes that were never resolved to a Validator, so
/// verification, conflict, and absence states are inspectable instead of being
/// silently absent.
#[utoipa::path(
    get,
    path = "/api/admin/v1/validator-identities",
    tag = "admin",
    responses((status = 200, body = [AdminNodeValidatorIdentity]), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_validator_identities(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match validator::list_node_validator_identities(&state.database(), None).await {
        Ok(records) => Json(
            records
                .into_iter()
                .map(AdminNodeValidatorIdentity::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => error_response(&request_id.0, error),
    }
}

/// Query for one bounded Validator daily-trend page (#219). Every bound is an
/// instant or a configured local date; the Server owns the calendar mapping.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorTrendRequest {
    /// RFC3339 instant bounding the requested window from below.
    pub from: Option<String>,
    /// RFC3339 instant bounding the requested window from above.
    pub to: Option<String>,
    /// Paging cursor: a configured local date (YYYY-MM-DD). Only strictly older
    /// days answer, so one page never re-answers a day the caller already holds.
    pub before: Option<String>,
    /// Days to answer, 1..=366. Defaults to 90.
    pub limit: Option<i64>,
}

/// One stored configured calendar day of the trend, with the UTC instants that
/// local day really covers and the timestamps the bucket was chosen from.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorTrendPoint {
    pub local_date: String,
    pub month_key: String,
    /// UTC instant the configured local day starts at, inclusive.
    pub day_start: String,
    /// UTC instant the next configured local day starts at, exclusive. A
    /// daylight-saving day is 23 or 25 hours wide here instead of a pretended 24.
    pub day_end: String,
    pub sample_at: String,
    pub received_at: String,
    pub provider_timestamp: Option<String>,
    /// Which timestamp chose this calendar day: the Provider timestamp or the
    /// Server receipt time.
    pub sample_time: String,
    /// receivedAt minus providerTimestamp in whole seconds for this one row.
    /// Null when the observation carried no Provider timestamp, because then
    /// there is no delay to measure; never 0 for an unknown delay.
    pub delay_seconds: Option<i64>,
    /// The observation is stamped after its receipt: the Provider clock is
    /// ahead of the Server clock.
    pub clock_suspect: bool,
    pub source: String,
    pub observation_key: String,
    pub rank: Option<i64>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
}

/// A stretch of configured local days this answer proves holds no snapshot.
/// A surface draws it as silence, never as a zero.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorTrendGap {
    pub from_local_date: String,
    pub to_local_date: String,
    pub days: i64,
}

/// One configured calendar month the answer touches, with the month boundary
/// mapped into the UTC investigation coordinate (#219).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorTrendMonth {
    pub month_key: String,
    /// UTC instant the month's first configured local day starts at.
    pub month_start: String,
    /// UTC instant the next month's first configured local day starts at.
    pub month_end: String,
    pub observed_days: i64,
    pub first_local_date: Option<String>,
    pub last_local_date: Option<String>,
}

/// One Node association of this Validator that still resolves to a Node row.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorTrendAssociation {
    pub link_id: String,
    pub node_id: String,
    pub node_display_name: Option<String>,
    pub node_lifecycle: String,
    /// manual or automatic, the model this interval was created under.
    pub origin: String,
    pub valid_from: String,
    pub valid_until: Option<String>,
    /// This interval has no end boundary yet.
    pub current: bool,
}

/// One bounded Validator daily-trend page (#219).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorTrendResponse {
    pub validator_id: String,
    pub network_key: String,
    /// The configured IANA timezone every local date in this answer is formed
    /// in. A stored bucket from another zone is counted, never merged.
    pub timezone: String,
    /// The coverage verdict this answer carries on its own: complete, partial,
    /// unavailable (nothing observed in the stretch), or empty (the caller
    /// asked for a stretch of no days).
    pub coverage: String,
    /// The counters in this answer are cumulative Provider counters as of each
    /// sample. This endpoint never derives period earnings, net profit, or a
    /// silently UTC re-bucketed series from them.
    pub counter_semantics: String,
    /// The UTC coordinate the caller asked for, echoed so a clamped or paged
    /// answer says what it narrowed.
    pub requested_from: String,
    pub requested_to: String,
    pub requested_from_local_date: String,
    pub requested_to_local_date: String,
    /// The configured local dates this answer really covers.
    pub answered_from_local_date: String,
    pub answered_to_local_date: String,
    pub requested_days: i64,
    /// The requested window was narrowed to the bounded maximum window.
    pub clamped: bool,
    /// Days the answered stretch covers, days that carry a snapshot, and the
    /// difference, so coverage is disclosed instead of assumed.
    pub expected_days: i64,
    pub observed_days: i64,
    pub missing_days: i64,
    pub gaps: Vec<AdminValidatorTrendGap>,
    pub first_observed_local_date: Option<String>,
    pub last_observed_local_date: Option<String>,
    /// Points in ascending configured local-date order.
    pub points: Vec<AdminValidatorTrendPoint>,
    pub months: Vec<AdminValidatorTrendMonth>,
    /// The window holds more days than the caller's limit answered.
    pub truncated: bool,
    /// Pass this back as before for the next, strictly older page.
    pub continuation: Option<String>,
    /// Stored rows for this Validator inside the answered stretch that were
    /// formed in another timezone. They are disclosed here and never merged
    /// into the configured calendar.
    pub foreign_rows: i64,
    pub foreign_timezones: Vec<String>,
    /// Association intervals that still resolve to a Node row.
    pub associations: Vec<AdminValidatorTrendAssociation>,
    pub associations_truncated: bool,
    /// Nodes of this Validator's Network that Purge already deleted. Purge
    /// removes that Node's association rows with the Node, so those intervals
    /// are unavailable here rather than never having existed (Story 68).
    pub deleted_nodes: i64,
    /// True when deleted Nodes mean this association list is not the whole
    /// association history. Retained Validator snapshots are never dropped.
    pub association_history_partial: bool,
}

/// The coverage verdict one trend answer can carry on its own.
fn trend_coverage(page: &validator::ValidatorTrendPage) -> &'static str {
    if page.expected_days == 0 {
        "empty"
    } else if page.observed_days == 0 {
        "unavailable"
    } else if page.missing_days == 0 && !page.truncated && !page.clamped {
        "complete"
    } else {
        "partial"
    }
}

/// Bounded Validator daily-trend page: rank, stake, and delegator count per
/// configured calendar day (#219, main design §15.4.1).
///
/// The window is answered in the configured IANA calendar and every boundary is
/// mapped back into the UTC investigation coordinate, so retention, pagination,
/// and coverage read on the real days instead of a silently UTC-aligned bucket.
#[utoipa::path(
    get,
    path = "/api/admin/v1/validators/{validator_id}/trend",
    tag = "admin",
    params(("validator_id" = String, Path, description = "Validator ID"), ValidatorTrendRequest),
    responses((status = 200, body = AdminValidatorTrendResponse), (status = 400, body = crate::http::ApiErrorBody), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn admin_validator_trend(
    State(state): State<AppState>,
    Path(validator_id): Path<String>,
    Query(query): Query<ValidatorTrendRequest>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let record = match validator::get_validator(state.db(), &validator_id).await {
        Ok(Some(record)) => record,
        Ok(None) => return error_response(&request_id.0, ValidatorError::ValidatorNotFound),
        Err(error) => return error_response(&request_id.0, error),
    };
    let request = validator::ValidatorTrendQuery {
        validator_id: validator_id.clone(),
        timezone: state.validator_timezone().to_owned(),
        from: query.from,
        to: query.to,
        before: query.before,
        limit: query.limit.unwrap_or(validator::TREND_DEFAULT_LIMIT),
    };
    let page = match validator::load_daily_trend(state.db(), &request, crate::auth::now_utc()).await
    {
        Ok(page) => page,
        Err(error) => return error_response(&request_id.0, error),
    };
    let coverage = trend_coverage(&page).to_owned();
    let associations = page
        .associations
        .into_iter()
        .map(|association| {
            let current = association.valid_until.is_none();
            AdminValidatorTrendAssociation {
                link_id: association.link_id,
                node_id: association.node_id,
                node_display_name: association.node_display_name,
                node_lifecycle: association.node_lifecycle,
                origin: association.origin,
                valid_from: association.valid_from,
                valid_until: association.valid_until,
                current,
            }
        })
        .collect();
    Json(AdminValidatorTrendResponse {
        validator_id,
        network_key: record.network_key,
        timezone: page.timezone,
        coverage,
        counter_semantics: "cumulative".to_owned(),
        requested_from: page.requested_from,
        requested_to: page.requested_to,
        requested_from_local_date: page.requested_from_local_date,
        requested_to_local_date: page.requested_to_local_date,
        answered_from_local_date: page.answered_from_local_date,
        answered_to_local_date: page.answered_to_local_date,
        requested_days: page.requested_days,
        clamped: page.clamped,
        expected_days: page.expected_days,
        observed_days: page.observed_days,
        missing_days: page.missing_days,
        gaps: page
            .gaps
            .into_iter()
            .map(|gap| AdminValidatorTrendGap {
                from_local_date: gap.from_local_date,
                to_local_date: gap.to_local_date,
                days: gap.days,
            })
            .collect(),
        first_observed_local_date: page.first_observed_local_date,
        last_observed_local_date: page.last_observed_local_date,
        points: page
            .points
            .into_iter()
            .map(|point| AdminValidatorTrendPoint {
                local_date: point.local_date,
                month_key: point.month_key,
                day_start: point.day_start,
                day_end: point.day_end,
                sample_at: point.sample_at,
                received_at: point.received_at,
                provider_timestamp: point.provider_timestamp,
                sample_time: point.sample_time,
                delay_seconds: point.delay_seconds,
                clock_suspect: point.clock_suspect,
                source: point.source,
                observation_key: point.observation_key,
                rank: point.rank,
                stake_amount: point.stake_amount,
                reward_amount: point.reward_amount,
                reward_rate: point.reward_rate,
                delegator_count: point.delegator_count,
                epoch: point.epoch,
                block_count: point.block_count,
            })
            .collect(),
        months: page
            .months
            .into_iter()
            .map(|month| AdminValidatorTrendMonth {
                month_key: month.month_key,
                month_start: month.month_start,
                month_end: month.month_end,
                observed_days: month.observed_days,
                first_local_date: month.first_local_date,
                last_local_date: month.last_local_date,
            })
            .collect(),
        truncated: page.truncated,
        continuation: page.continuation,
        foreign_rows: page.foreign_rows,
        foreign_timezones: page.foreign_timezones,
        associations,
        associations_truncated: page.associations_truncated,
        deleted_nodes: page.deleted_nodes,
        association_history_partial: page.association_history_partial,
    })
    .into_response()
}
pub(crate) fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/validators", get(admin_validators))
        .route("/validators/{validator_id}", get(admin_validator_detail))
        .route(
            "/validators/{validator_id}/analytics",
            get(admin_validator_analytics),
        )
        .route(
            "/validators/{validator_id}/history",
            get(admin_validator_history),
        )
        .route(
            "/validators/{validator_id}/trend",
            get(admin_validator_trend),
        )
        .route("/validator-identities", get(admin_validator_identities))
        .route("/networks/{network_key}/validators", post(create_validator))
        .route("/validator-links", get(admin_validator_links))
        .route(
            "/validator-links/{link_id}",
            get(admin_validator_link_detail),
        )
        .route(
            "/nodes/{node_id}/validator-links",
            get(admin_node_validator_links),
        )
        .route(
            "/nodes/{node_id}/validator-links",
            post(create_node_validator_link),
        )
        .route("/validator-links/{link_id}", put(update_validator_link))
        .route("/validator-links/{link_id}/end", post(end_validator_link))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use tempfile::tempdir;

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
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Main Network', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        (dir, state)
    }

    async fn seed_admin_analytics_row(state: &AppState, validator_id: &str) {
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES (?, 'mainnet', ?, ?, ?, ?)")
            .bind(validator_id)
            .bind(format!("node-{validator_id}"))
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key, last_observation_key, updated_at) VALUES (?, 'explorer', 'success', NULL, ?, ?, ?, ?, 5, '1000', '10', '0.05', 8, 42, 100, 'normal', 'normal', NULL, NULL, 0, NULL, NULL, NULL, ?, ?)")
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count) VALUES (?, ?, 'UTC', '2026-01-01', '2026-01', ?, ?, ?, 'explorer', 'obs-1', 5, '1000', '10', '0.05', 8, 42, 100)")
            .bind(format!("snap-{validator_id}"))
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO validator_monthly_aggregates (aggregate_id, validator_id, timezone, month_key, snapshot_count, first_sample_at, last_sample_at, rank_min, rank_max, rank_last, stake_last, reward_last, reward_rate_last, delegator_count_last, epoch_last, block_count_last, updated_at) VALUES (?, ?, 'UTC', '2026-01', 1, ?, ?, 5, 5, 5, '1000', '10', '0.05', 8, 42, 100, ?)")
            .bind(format!("agg-{validator_id}"))
            .bind(validator_id)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    /// One stored Validator day for the trend endpoint tests: the sample time is
    /// the Provider timestamp when the Provider gave one and the receipt time
    /// otherwise, exactly like the production writer.
    async fn seed_admin_trend_day(
        state: &AppState,
        validator_id: &str,
        timezone: &str,
        local_date: &str,
        received_at: &str,
        provider_timestamp: Option<&str>,
    ) {
        sqlx::query("INSERT INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'explorer', ?, 5, '1000', '25', '0.05', 8, 42, 100)")
            .bind(format!("snapshot-{validator_id}-{local_date}"))
            .bind(validator_id)
            .bind(timezone)
            .bind(local_date)
            .bind(&local_date[..7])
            .bind(provider_timestamp.unwrap_or(received_at))
            .bind(received_at)
            .bind(provider_timestamp)
            .bind(format!("observation-{timezone}-{local_date}"))
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn admin_validator_analytics_includes_admin_dto_fields_and_handles_unknown() {
        let (_dir, state) = test_state().await;
        seed_admin_analytics_row(&state, "validator-1").await;

        let response = admin_validator_analytics(
            State(state.clone()),
            Path("validator-1".to_owned()),
            Query(ValidatorHistoryQuery { limit: None }),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["validatorId"], "validator-1");
        assert_eq!(value["daily"][0]["localDate"], "2026-01-01");
        assert!(value["daily"][0]["receivedAt"].as_str().is_some());
        assert_eq!(value["daily"][0]["source"], "explorer");
        assert_eq!(value["monthly"][0]["monthKey"], "2026-01");
        assert_eq!(value["monthly"][0]["snapshotCount"], 1);
        assert!(
            value["monthly"][0]
                .get("updatedAt")
                .and_then(serde_json::Value::as_str)
                .is_some()
        );

        let missing = admin_validator_analytics(
            State(state.clone()),
            Path("validator-missing".to_owned()),
            Query(ValidatorHistoryQuery { limit: None }),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        // A registered Validator with no insight is honest "unknown" instead
        // of pretending the aggregate history is a healthy zero.
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-empty', 'mainnet', 'node-validator-empty', 'Empty', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        let empty = admin_validator_analytics(
            State(state),
            Path("validator-empty".to_owned()),
            Query(ValidatorHistoryQuery { limit: None }),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(empty.status(), StatusCode::OK);
        let body = to_bytes(empty.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["state"], "unknown");
        assert!(value["daily"].as_array().unwrap().is_empty());
        assert!(value["monthly"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn admin_validator_trend_answers_its_configured_calendar_with_coverage_and_gaps() {
        let (_dir, state) = test_state().await;
        let state = state.with_validator_timezone("Asia/Tokyo".to_owned());
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('validator-1', 'mainnet', 'node-key-1', 'First', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(state.db().pool())
            .await
            .unwrap();
        seed_admin_trend_day(
            &state,
            "validator-1",
            "Asia/Tokyo",
            "2026-02-01",
            "2026-02-01T00:01:00Z",
            None,
        )
        .await;
        seed_admin_trend_day(
            &state,
            "validator-1",
            "Asia/Tokyo",
            "2026-02-03",
            "2026-02-03T00:00:30Z",
            Some("2026-02-03T00:00:00Z"),
        )
        .await;

        let ask =
            |from: Option<&str>, to: Option<&str>, before: Option<&str>, limit: Option<i64>| {
                admin_validator_trend(
                    State(state.clone()),
                    Path("validator-1".to_owned()),
                    Query(ValidatorTrendRequest {
                        from: from.map(str::to_owned),
                        to: to.map(str::to_owned),
                        before: before.map(str::to_owned),
                        limit,
                    }),
                    Extension(RequestId(std::sync::Arc::from("test"))),
                )
            };

        let response = ask(
            Some("2026-02-01T00:00:00Z"),
            Some("2026-02-04T00:00:00Z"),
            None,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["validatorId"], "validator-1");
        assert_eq!(value["networkKey"], "mainnet");
        assert_eq!(value["timezone"], "Asia/Tokyo");
        // Cumulative counters are labelled as such: read as of each sample, never
        // as period earnings or a net-profit delta.
        assert_eq!(value["counterSemantics"], "cumulative");
        assert_eq!(value["requestedFrom"], "2026-02-01T00:00:00Z");
        assert_eq!(value["requestedTo"], "2026-02-04T00:00:00Z");
        assert_eq!(value["requestedFromLocalDate"], "2026-02-01");
        assert_eq!(value["requestedToLocalDate"], "2026-02-04");
        assert_eq!(value["requestedDays"], 4);
        assert_eq!(value["clamped"], false);
        assert_eq!(value["expectedDays"], 4);
        assert_eq!(value["observedDays"], 2);
        assert_eq!(value["missingDays"], 2);
        assert_eq!(value["coverage"], "partial");
        // The configured local day is answered on its real UTC stretch, and the
        // month boundary is the configured calendar's, not a UTC-aligned bucket.
        assert_eq!(value["answeredFromLocalDate"], "2026-02-01");
        assert_eq!(value["answeredToLocalDate"], "2026-02-04");
        assert_eq!(value["firstObservedLocalDate"], "2026-02-01");
        assert_eq!(value["lastObservedLocalDate"], "2026-02-03");
        assert_eq!(value["points"][0]["localDate"], "2026-02-01");
        assert_eq!(value["points"][0]["dayStart"], "2026-01-31T15:00:00Z");
        assert_eq!(value["points"][0]["dayEnd"], "2026-02-01T15:00:00Z");
        assert_eq!(value["points"][0]["monthKey"], "2026-02");
        assert_eq!(value["points"][0]["rank"], 5);
        assert_eq!(value["points"][0]["stakeAmount"], "1000");
        assert_eq!(value["points"][0]["delegatorCount"], 8);
        // Without a Provider timestamp the sample is a receipt time and the
        // delay is unknown, which is not the same as a fresh zero delay.
        assert_eq!(value["points"][0]["sampleTime"], "receipt");
        assert!(value["points"][0]["delaySeconds"].is_null());
        assert_eq!(value["points"][0]["clockSuspect"], false);
        assert_eq!(value["points"][1]["sampleTime"], "provider");
        assert_eq!(value["points"][1]["delaySeconds"], 30);
        assert_eq!(value["months"][0]["monthKey"], "2026-02");
        assert_eq!(value["months"][0]["monthStart"], "2026-01-31T15:00:00Z");
        assert_eq!(value["months"][0]["monthEnd"], "2026-02-28T15:00:00Z");
        assert_eq!(value["months"][0]["observedDays"], 2);
        assert_eq!(value["truncated"], false);
        assert!(value["continuation"].is_null());
        assert_eq!(value["foreignRows"], 0);
        assert_eq!(
            value["gaps"].as_array().unwrap().len(),
            2,
            "both unread days inside the answered stretch are gaps: {value}"
        );
        assert_eq!(value["gaps"][0]["fromLocalDate"], "2026-02-02");
        assert_eq!(value["gaps"][0]["toLocalDate"], "2026-02-02");
        assert_eq!(value["gaps"][0]["days"], 1);
        assert_eq!(value["gaps"][1]["fromLocalDate"], "2026-02-04");
        // No Node association exists for this Validator, and nothing was purged:
        // the empty list is complete, not partial.
        assert!(value["associations"].as_array().unwrap().is_empty());
        assert_eq!(value["associationsTruncated"], false);
        assert_eq!(value["deletedNodes"], 0);
        assert_eq!(value["associationHistoryPartial"], false);

        // A window every day of which carries a snapshot is complete on its own.
        let complete = ask(
            Some("2026-02-01T00:00:00Z"),
            Some("2026-02-01T00:00:00Z"),
            None,
            None,
        )
        .await;
        assert_eq!(complete.status(), StatusCode::OK);
        let body = to_bytes(complete.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["expectedDays"], 1);
        assert_eq!(value["observedDays"], 1);
        assert_eq!(value["coverage"], "complete");

        // A stretch with days but no snapshot is unavailable, never healthy-empty.
        let unobserved = ask(
            Some("2026-05-01T00:00:00Z"),
            Some("2026-05-02T00:00:00Z"),
            None,
            None,
        )
        .await;
        let body = to_bytes(unobserved.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["expectedDays"], 2);
        assert_eq!(value["observedDays"], 0);
        assert_eq!(value["coverage"], "unavailable");
        assert!(value["points"].as_array().unwrap().is_empty());

        // A cursor past every day asks for no day at all: honestly empty, not an
        // error and not partial.
        let empty = ask(
            Some("2026-06-01T00:00:00Z"),
            Some("2026-06-30T00:00:00Z"),
            Some("2026-01-01"),
            None,
        )
        .await;
        assert_eq!(empty.status(), StatusCode::OK);
        let body = to_bytes(empty.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["expectedDays"], 0);
        assert_eq!(value["coverage"], "empty");

        // Pagination is disclosed, and the caller can ask for the older page.
        let first = ask(
            Some("2026-02-01T00:00:00Z"),
            Some("2026-02-04T00:00:00Z"),
            None,
            Some(1),
        )
        .await;
        let body = to_bytes(first.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["truncated"], true);
        assert_eq!(value["continuation"], "2026-02-03");
        assert_eq!(value["points"].as_array().unwrap().len(), 1);

        // A configured zone that cannot parse is a sanitized 400, and an unknown
        // Validator is a 404 that leaks nothing about the database.
        let bad_zone = {
            let bad_zone_state = state
                .clone()
                .with_validator_timezone("Not/AZone".to_owned());
            admin_validator_trend(
                State(bad_zone_state),
                Path("validator-1".to_owned()),
                Query(ValidatorTrendRequest {
                    from: None,
                    to: None,
                    before: None,
                    limit: None,
                }),
                Extension(RequestId(std::sync::Arc::from("test"))),
            )
            .await
        };
        assert_eq!(bad_zone.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(bad_zone.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["code"], "invalid_request");
        assert_eq!(value["error"]["requestId"], "test");

        let inverted = ask(
            Some("2026-06-02T00:00:00Z"),
            Some("2026-06-01T00:00:00Z"),
            None,
            None,
        )
        .await;
        assert_eq!(inverted.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(inverted.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["code"], "invalid_request");
        assert_eq!(
            value["error"]["message"],
            "invalid Validator trend window: from must not be later than to"
        );

        let missing = admin_validator_trend(
            State(state.clone()),
            Path("validator-missing".to_owned()),
            Query(ValidatorTrendRequest {
                from: None,
                to: None,
                before: None,
                limit: None,
            }),
            Extension(RequestId(std::sync::Arc::from("test"))),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(missing.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["code"], "not_found");
    }

    #[test]
    fn validator_api_dtos_use_browser_camel_case() {
        let value = serde_json::to_value(Validator {
            validator_id: "validator-1".to_owned(),
            network_key: "mainnet".to_owned(),
            validator_node_id: "node-key".to_owned(),
            display_name: Some("Primary".to_owned()),
            created_at: "2025-01-01T00:00:00Z".to_owned(),
            updated_at: "2025-01-01T00:00:00Z".to_owned(),
            link_count: 1,
            insight: None,
        })
        .unwrap();
        assert_eq!(value["validatorId"], "validator-1");
        assert_eq!(value["validatorNodeId"], "node-key");
        assert_eq!(value["linkCount"], 1);
        assert!(value.get("validator_id").is_none());
    }
}
