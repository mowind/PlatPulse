//! Owner-only Validator Registry and Node Validator Link HTTP boundary.
//!
//! These routes expose only Server-owned Validator/link DTOs. Agent reports,
//! consensus membership, provider values, endpoints, and raw diagnostics are
//! not accepted as an implicit relationship source.

use super::admin::{mutation_error, mutation_guard_ok};
use super::{AppState, AuthenticatedSession, RequestId};
use crate::validator::{self, NodeValidatorLinkRecord, ValidatorError, ValidatorRecord};
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
    let (status, code) = match error {
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
        | ValidatorError::InvalidValidity => (StatusCode::BAD_REQUEST, "invalid_request"),
        ValidatorError::Database(_) => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    };
    (
        status,
        Json(crate::http::ApiErrorBody::with_message(
            code,
            error.to_string(),
            request_id,
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
    Ok(Validator {
        validator_id: record.validator_id,
        network_key: record.network_key,
        validator_node_id: record.validator_node_id,
        display_name: record.display_name,
        created_at: record.created_at,
        updated_at: record.updated_at,
        link_count,
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
    if records.is_empty()
        && sqlx::query_scalar::<_, i64>("SELECT 1 FROM nodes WHERE node_id = ?")
            .bind(&node_id)
            .fetch_optional(state.db().pool())
            .await
            .ok()
            .flatten()
            .is_none()
    {
        return error_response(&request_id.0, ValidatorError::NodeNotFound);
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

pub(crate) fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/validators", get(admin_validators))
        .route("/validators/{validator_id}", get(admin_validator_detail))
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
        })
        .unwrap();
        assert_eq!(value["validatorId"], "validator-1");
        assert_eq!(value["validatorNodeId"], "node-key");
        assert_eq!(value["linkCount"], 1);
        assert!(value.get("validator_id").is_none());
    }
}
