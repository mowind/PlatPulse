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
    pub diagnostic: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminValidatorHistoryLink {
    pub link_id: String,
    pub node_id: String,
    pub role: String,
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
    pub role: String,
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

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorCreateRequest {
    pub validator_node_id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorLinkCreateRequest {
    pub validator_id: String,
    pub role: String,
    pub valid_from: String,
    pub valid_until: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorLinkUpdateRequest {
    pub role: String,
    pub valid_from: String,
    pub valid_until: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorLinkEndRequest {
    pub valid_until: Option<String>,
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

fn error_response(request_id: &str, error: ValidatorError) -> Response {
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
        | ValidatorError::InvalidTimezone(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
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

fn revision(value: &str) -> u64 {
    value.bytes().fold(0_u64, |accumulator, byte| {
        accumulator.wrapping_mul(31).wrapping_add(byte as u64)
    })
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
        .map(|row| AdminValidatorInsight {
            validator_node_id: record.validator_node_id.clone(),
            display_name: record.display_name.clone(),
            state: if row.outcome == "success" {
                validator::freshness(row.last_good_received_at.as_deref(), crate::auth::now_utc())
                    .to_owned()
            } else {
                row.outcome.clone()
            },
            freshness: validator::freshness(
                row.last_good_received_at.as_deref(),
                crate::auth::now_utc(),
            )
            .to_owned(),
            outcome: row.outcome,
            source: row.source,
            provider_timestamp: row.provider_timestamp,
            received_at: row.last_good_received_at.clone(),
            attempted_at: Some(row.last_attempt_received_at),
            last_good_received_at: row.last_good_received_at,
            rank: row.rank,
            stake_amount: row.stake_amount,
            reward_amount: row.reward_amount,
            reward_rate: row.reward_rate,
            delegator_count: row.delegator_count,
            epoch: row.epoch,
            block_count: row.block_count,
            counter_state: row.counter_state,
            diagnostic: row.diagnostic,
        })
        .or_else(|| {
            Some(AdminValidatorInsight {
                validator_node_id: record.validator_node_id.clone(),
                display_name: record.display_name.clone(),
                state: "unsupported".to_owned(),
                freshness: "unknown".to_owned(),
                outcome: "unsupported".to_owned(),
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

#[utoipa::path(
    post,
    path = "/api/admin/v1/networks/{network_key}/validators",
    tag = "admin",
    params(("network_key" = String, Path, description = "Registered Network key")),
    request_body = ValidatorCreateRequest,
    responses((status = 200, body = ValidatorMutationResponse), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 400, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn create_validator(
    State(state): State<AppState>,
    Path(network_key): Path<String>,
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
    let body: ValidatorCreateRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    match validator::create_validator(
        &state.database(),
        &network_key,
        &body.validator_node_id,
        body.display_name.as_deref(),
        &principal.0.user_id,
    )
    .await
    {
        Ok((record, audit_event_id)) => {
            let value = match validator_dto(&state, record).await {
                Ok(value) => value,
                Err(error) => return error_response(&request_id.0, error),
            };
            state.admin_realtime().publish(
                "validator",
                Some(value.validator_id.clone()),
                revision(&value.updated_at),
            );
            Json(ValidatorMutationResponse {
                validator: value,
                request_id: request_id.0.to_string(),
                audit_event_id,
            })
            .into_response()
        }
        Err(error) => error_response(&request_id.0, error),
    }
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
    request_body = ValidatorLinkCreateRequest,
    responses((status = 200, body = ValidatorLinkMutationResponse), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 400, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn create_node_validator_link(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
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
    let body: ValidatorLinkCreateRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    match validator::create_link(
        &state.database(),
        &node_id,
        &body.validator_id,
        &body.role,
        &body.valid_from,
        body.valid_until.as_deref(),
        &principal.0.user_id,
    )
    .await
    {
        Ok((record, audit_event_id)) => {
            let value = match link_dto(&state, record).await {
                Ok(value) => value,
                Err(error) => return error_response(&request_id.0, error),
            };
            state.admin_realtime().publish(
                "validator",
                Some(value.validator_id.clone()),
                revision(&value.updated_at),
            );
            state.admin_realtime().publish(
                "node",
                Some(value.node_id.clone()),
                revision(&value.updated_at),
            );
            Json(ValidatorLinkMutationResponse {
                link: value,
                request_id: request_id.0.to_string(),
                audit_event_id,
            })
            .into_response()
        }
        Err(error) => error_response(&request_id.0, error),
    }
}

#[utoipa::path(
    put,
    path = "/api/admin/v1/validator-links/{link_id}",
    tag = "admin",
    params(("link_id" = String, Path, description = "Node Validator Link ID")),
    request_body = ValidatorLinkUpdateRequest,
    responses((status = 200, body = ValidatorLinkMutationResponse), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn update_validator_link(
    State(state): State<AppState>,
    Path(link_id): Path<String>,
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
    let body: ValidatorLinkUpdateRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    match validator::update_link(
        &state.database(),
        &link_id,
        &body.role,
        &body.valid_from,
        body.valid_until.as_deref(),
        &principal.0.user_id,
    )
    .await
    {
        Ok((record, audit_event_id)) => {
            let value = match link_dto(&state, record).await {
                Ok(value) => value,
                Err(error) => return error_response(&request_id.0, error),
            };
            state.admin_realtime().publish(
                "validator",
                Some(value.validator_id.clone()),
                revision(&value.updated_at),
            );
            state.admin_realtime().publish(
                "node",
                Some(value.node_id.clone()),
                revision(&value.updated_at),
            );
            Json(ValidatorLinkMutationResponse {
                link: value,
                request_id: request_id.0.to_string(),
                audit_event_id,
            })
            .into_response()
        }
        Err(error) => error_response(&request_id.0, error),
    }
}

#[utoipa::path(
    post,
    path = "/api/admin/v1/validator-links/{link_id}/end",
    tag = "admin",
    params(("link_id" = String, Path, description = "Node Validator Link ID")),
    request_body = ValidatorLinkEndRequest,
    responses((status = 200, body = ValidatorLinkMutationResponse), (status = 401, body = crate::http::ApiErrorBody), (status = 403, body = crate::http::ApiErrorBody), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody), (status = 409, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(crate) async fn end_validator_link(
    State(state): State<AppState>,
    Path(link_id): Path<String>,
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
    let body: ValidatorLinkEndRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body is invalid",
            );
        }
    };
    match validator::end_link(
        &state.database(),
        &link_id,
        body.valid_until.as_deref(),
        &principal.0.user_id,
    )
    .await
    {
        Ok((record, audit_event_id)) => {
            let value = match link_dto(&state, record).await {
                Ok(value) => value,
                Err(error) => return error_response(&request_id.0, error),
            };
            state.admin_realtime().publish(
                "validator",
                Some(value.validator_id.clone()),
                revision(&value.updated_at),
            );
            state.admin_realtime().publish(
                "node",
                Some(value.node_id.clone()),
                revision(&value.updated_at),
            );
            Json(ValidatorLinkMutationResponse {
                link: value,
                request_id: request_id.0.to_string(),
                audit_event_id,
            })
            .into_response()
        }
        Err(error) => error_response(&request_id.0, error),
    }
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
