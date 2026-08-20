//! Server-managed Validator identities and explicit Node Validator Links.
//!
//! Validators are keyed by `(Network, validator_node_id)` and are never
//! inferred from Agent reports, consensus membership, provider data, or
//! Node identity. Link mutations are transactional with their Audit Event and
//! reject every temporal overlap for one Node before inserting or updating.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use reqwest::StatusCode;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Sqlite, Transaction};
use std::sync::Arc;
use thiserror::Error;
use time::OffsetDateTime;

use crate::auth::{format_rfc3339, insert_audit_event, now_utc};
use crate::database::ServerDatabase;

pub const MAX_VALIDATOR_NODE_ID_LEN: usize = 256;
pub const MAX_VALIDATOR_DISPLAY_NAME_LEN: usize = 128;
pub const MAX_PROVIDER_DIAGNOSTIC_LEN: usize = 256;
pub const MAX_PROVIDER_BODY_LEN: usize = 64 * 1024;

/// Exact normalized values returned by a Server-side Validator Provider.
/// Provider-specific JSON never crosses this boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidatorObservation {
    pub provider_timestamp: Option<String>,
    pub rank: Option<i64>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatorProviderResult {
    Success(ValidatorObservation),
    NotFound,
    AuthoritativeEmpty,
    Error(String),
    Unsupported(String),
}

#[async_trait]
pub trait ValidatorProvider: Send + Sync {
    fn source(&self) -> &str;

    async fn fetch(&self, network_key: &str, validator_node_id: &str) -> ValidatorProviderResult;
}

pub type SharedValidatorProvider = Arc<dyn ValidatorProvider>;

/// The default provider is explicit rather than pretending that no provider
/// means a zero-valued Validator.
#[derive(Debug, Default)]
pub struct DisabledValidatorProvider;

#[async_trait]
impl ValidatorProvider for DisabledValidatorProvider {
    fn source(&self) -> &str {
        "disabled"
    }

    async fn fetch(&self, _network_key: &str, _validator_node_id: &str) -> ValidatorProviderResult {
        ValidatorProviderResult::Unsupported("provider is not configured".to_owned())
    }
}

/// Initial Explorer adapter. Its response is deliberately reduced to the
/// normalized observation above; unknown fields and response diagnostics are
/// discarded at the trust boundary.
#[derive(Clone)]
pub struct ExplorerValidatorProvider {
    client: reqwest::Client,
    base_url: String,
}

impl ExplorerValidatorProvider {
    pub fn new(base_url: &str, timeout: std::time::Duration) -> Result<Self, String> {
        let base_url = base_url.trim().trim_end_matches('/');
        if !(base_url.starts_with("https://") || base_url.starts_with("http://"))
            || base_url.contains('@')
            || base_url.contains('?')
            || base_url.contains('#')
        {
            return Err(
                "Explorer base URL must be an absolute HTTP(S) URL without credentials".to_owned(),
            );
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|_| "unable to construct Explorer client".to_owned())?;
        Ok(Self {
            client,
            base_url: base_url.to_owned(),
        })
    }

    fn endpoint(&self, network_key: &str, validator_node_id: &str) -> String {
        format!(
            "{}/api/v1/networks/{}/validators/{}",
            self.base_url,
            url_path_segment(network_key),
            url_path_segment(validator_node_id)
        )
    }
}

#[async_trait]
impl ValidatorProvider for ExplorerValidatorProvider {
    fn source(&self) -> &str {
        "explorer"
    }

    async fn fetch(&self, network_key: &str, validator_node_id: &str) -> ValidatorProviderResult {
        let response = match self
            .client
            .get(self.endpoint(network_key, validator_node_id))
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => return ValidatorProviderResult::Error("Explorer request failed".to_owned()),
        };
        match response.status() {
            StatusCode::NOT_FOUND => return ValidatorProviderResult::NotFound,
            StatusCode::NOT_IMPLEMENTED | StatusCode::METHOD_NOT_ALLOWED => {
                return ValidatorProviderResult::Unsupported(
                    "Explorer endpoint is unsupported".to_owned(),
                );
            }
            status if status.is_client_error() || status.is_server_error() => {
                return ValidatorProviderResult::Error(
                    "Explorer returned an unsuccessful response".to_owned(),
                );
            }
            _ => {}
        }
        let body = match response.bytes().await {
            Ok(body) if body.len() <= MAX_PROVIDER_BODY_LEN => body,
            Ok(_) => {
                return ValidatorProviderResult::Error(
                    "Explorer response exceeded the size limit".to_owned(),
                );
            }
            Err(_) => {
                return ValidatorProviderResult::Error(
                    "Explorer response could not be read".to_owned(),
                );
            }
        };
        let value: Value = match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(_) => {
                return ValidatorProviderResult::Error(
                    "Explorer response was malformed".to_owned(),
                );
            }
        };
        match normalize_explorer_response(&value) {
            Ok(Some(observation)) => ValidatorProviderResult::Success(observation),
            Ok(None) => ValidatorProviderResult::AuthoritativeEmpty,
            Err(error) => ValidatorProviderResult::Error(error),
        }
    }
}

fn url_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            byte => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn normalize_explorer_response(value: &Value) -> Result<Option<ValidatorObservation>, String> {
    if let Some(array) = value.as_array() {
        return match array.as_slice() {
            [] => Ok(None),
            [single] => normalize_explorer_response(single),
            _ => Err("Explorer returned multiple Validator observations".to_owned()),
        };
    }
    let object = value
        .as_object()
        .ok_or_else(|| "Explorer response was not an object".to_owned())?;
    for key in ["data", "validators", "result"] {
        if let Some(array) = object.get(key).and_then(Value::as_array) {
            if array.is_empty() {
                return Ok(None);
            }
            if array.len() > 1 {
                return Err("Explorer returned multiple Validator observations".to_owned());
            }
            return normalize_explorer_response(&array[0]);
        }
    }
    let read_string = |names: &[&str]| -> Result<Option<String>, String> {
        let Some(value) = names.iter().find_map(|name| object.get(*name)) else {
            return Ok(None);
        };
        let value = match value.as_str() {
            Some(value) => value.to_owned(),
            None if value.is_number() => value.to_string(),
            _ => return Err("Explorer returned an invalid text value".to_owned()),
        };
        normalize_bounded_text(&value)
    };
    let read_int = |names: &[&str]| -> Result<Option<i64>, String> {
        let Some(value) = names.iter().find_map(|name| object.get(*name)) else {
            return Ok(None);
        };
        if let Some(number) = value.as_i64() {
            return if number >= 0 {
                Ok(Some(number))
            } else {
                Err("Explorer returned a negative integer".to_owned())
            };
        }
        let text = match value.as_str() {
            Some(text) => text,
            None => return Err("Explorer returned an invalid integer".to_owned()),
        };
        let parsed = text
            .parse::<i64>()
            .map_err(|_| "Explorer returned an out-of-range integer".to_owned())?;
        if parsed < 0 {
            return Err("Explorer returned a negative integer".to_owned());
        }
        Ok(Some(parsed))
    };
    let provider_timestamp = read_string(&["provider_timestamp", "timestamp", "updated_at"])?
        .map(|value| canonical_timestamp(&value).map_err(|error| error.to_string()))
        .transpose()?;
    let observation = ValidatorObservation {
        provider_timestamp,
        rank: read_int(&["rank", "ranking"])?,
        stake_amount: read_string(&["stake_amount", "stake", "staking_amount"])?,
        reward_amount: read_string(&["reward_amount", "reward", "rewards"])?,
        reward_rate: read_string(&["reward_rate", "rate", "percentage"])?,
        delegator_count: read_int(&["delegator_count", "delegators", "delegatorCount"])?,
        epoch: read_int(&["epoch", "epoch_number"])?,
        block_count: read_int(&["block_count", "blocks"])?,
    };
    if observation == ValidatorObservation::default() {
        return Err("Explorer response did not contain supported Validator fields".to_owned());
    }
    for value in [
        observation.stake_amount.as_deref(),
        observation.reward_amount.as_deref(),
        observation.reward_rate.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_nonnegative_decimal(value)?;
    }
    Ok(Some(observation))
}

fn normalize_bounded_text(value: &str) -> Result<Option<String>, String> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err("Explorer returned an invalid bounded value".to_owned());
    }
    Ok(Some(value.to_owned()))
}

fn validate_nonnegative_decimal(value: &str) -> Result<(), String> {
    let mut dots = 0;
    let mut digits = 0;
    for character in value.chars() {
        match character {
            '0'..='9' => digits += 1,
            '.' => dots += 1,
            _ => return Err("Explorer returned an invalid numeric value".to_owned()),
        }
    }
    if digits == 0 || dots > 1 {
        return Err("Explorer returned an invalid numeric value".to_owned());
    }
    Ok(())
}

fn provider_diagnostic(value: String) -> String {
    let value = crate::redaction::redact_sensitive(&value)
        .replace("https://", "[redacted-url]/")
        .replace("http://", "[redacted-url]/");
    value.chars().take(MAX_PROVIDER_DIAGNOSTIC_LEN).collect()
}

fn observation_key(observation: &ValidatorObservation) -> String {
    let bytes = format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        observation.provider_timestamp,
        observation.rank,
        observation.stake_amount,
        observation.reward_amount,
        observation.reward_rate,
        observation.delegator_count,
        observation.epoch,
        observation.block_count
    );
    let mut hash = Sha256::new();
    hash.update(bytes.as_bytes());
    format!("{:x}", hash.finalize())
}

fn decimal_decreased(previous: Option<&str>, current: Option<&str>) -> bool {
    let Some((previous, current)) = previous.zip(current) else {
        return false;
    };
    let normalize = |value: &str| {
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        let whole = whole.trim_start_matches('0');
        let whole = if whole.is_empty() { "0" } else { whole };
        let fraction = fraction.trim_end_matches('0');
        (whole.to_owned(), fraction.to_owned())
    };
    let (previous_whole, previous_fraction) = normalize(previous);
    let (current_whole, current_fraction) = normalize(current);
    previous_whole.len() > current_whole.len()
        || (previous_whole.len() == current_whole.len()
            && (previous_whole > current_whole
                || (previous_whole == current_whole && {
                    let width = previous_fraction.len().max(current_fraction.len());
                    let previous_fraction = format!("{:0<width$}", previous_fraction);
                    let current_fraction = format!("{:0<width$}", current_fraction);
                    previous_fraction > current_fraction
                })))
}

fn counter_decreases(
    existing: Option<&ValidatorInsightRecord>,
    observation: &ValidatorObservation,
) -> Vec<(&'static str, String, String)> {
    let Some(existing) = existing else {
        return Vec::new();
    };
    let mut decreases = Vec::new();
    if decimal_decreased(
        existing.stake_amount.as_deref(),
        observation.stake_amount.as_deref(),
    ) {
        decreases.push((
            "stake_amount",
            existing.stake_amount.clone().unwrap_or_default(),
            observation.stake_amount.clone().unwrap_or_default(),
        ));
    }
    if decimal_decreased(
        existing.reward_amount.as_deref(),
        observation.reward_amount.as_deref(),
    ) {
        decreases.push((
            "reward_amount",
            existing.reward_amount.clone().unwrap_or_default(),
            observation.reward_amount.clone().unwrap_or_default(),
        ));
    }
    if matches!(
        (existing.block_count, observation.block_count),
        (Some(previous), Some(current)) if current < previous
    ) {
        decreases.push((
            "block_count",
            existing.block_count.unwrap_or_default().to_string(),
            observation.block_count.unwrap_or_default().to_string(),
        ));
    }
    decreases
}

#[derive(Debug, Clone, FromRow)]
pub struct ValidatorRecord {
    pub validator_id: String,
    pub network_key: String,
    pub validator_node_id: String,
    pub display_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct NodeValidatorLinkRecord {
    pub link_id: String,
    pub node_id: String,
    pub validator_id: String,
    pub role: String,
    pub valid_from: String,
    pub valid_until: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Error)]
pub enum ValidatorError {
    #[error("validator Node ID must be 1..=256 characters without control characters")]
    InvalidValidatorNodeId,
    #[error(
        "validator display name must be empty or 1..=128 characters without control characters"
    )]
    InvalidDisplayName,
    #[error("link role must be primary, standby, or observer")]
    InvalidRole,
    #[error("invalid RFC3339 timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("valid_until must be later than valid_from")]
    InvalidValidity,
    #[error("network was not found")]
    NetworkNotFound,
    #[error("validator identity is already registered for this Network")]
    ValidatorAlreadyExists,
    #[error("validator was not found")]
    ValidatorNotFound,
    #[error("Node was not found")]
    NodeNotFound,
    #[error("Node is not active")]
    NodeNotActive,
    #[error("validator and Node belong to different Networks")]
    NetworkMismatch,
    #[error("Node already has an overlapping Validator Link")]
    LinkOverlap,
    #[error("Validator Link was not found")]
    LinkNotFound,
    #[error("cannot end a link before its valid-from boundary")]
    EndBeforeStart,
    #[error("cannot update a link that has already ended")]
    LinkAlreadyEnded,
    #[error("a link replacement must begin after the existing link")]
    LinkReplacementMustAdvance,
    #[error("invalid Validator analytics IANA timezone: {0}")]
    InvalidTimezone(String),
    #[error("provider returned an invalid Validator observation: {0}")]
    InvalidProviderObservation(String),
    #[error("alert evaluation failed: {0}")]
    Alert(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub fn validate_validator_node_id(value: &str) -> Result<(), ValidatorError> {
    if value.is_empty()
        || value.chars().count() > MAX_VALIDATOR_NODE_ID_LEN
        || value.chars().any(|character| character.is_control())
    {
        return Err(ValidatorError::InvalidValidatorNodeId);
    }
    Ok(())
}

pub fn validate_display_name(value: Option<&str>) -> Result<(), ValidatorError> {
    if let Some(value) = value {
        if value.is_empty()
            || value.chars().count() > MAX_VALIDATOR_DISPLAY_NAME_LEN
            || value.chars().any(|character| character.is_control())
        {
            return Err(ValidatorError::InvalidDisplayName);
        }
    }
    Ok(())
}

pub fn validate_role(value: &str) -> Result<(), ValidatorError> {
    matches!(value, "primary" | "standby" | "observer")
        .then_some(())
        .ok_or(ValidatorError::InvalidRole)
}

fn parse_timestamp(value: &str) -> Result<OffsetDateTime, ValidatorError> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|error| ValidatorError::InvalidTimestamp(error.to_string()))
}

fn canonical_timestamp(value: &str) -> Result<String, ValidatorError> {
    Ok(format_rfc3339(parse_timestamp(value)?))
}

fn canonical_validity(
    valid_from: &str,
    valid_until: Option<&str>,
) -> Result<(String, Option<String>), ValidatorError> {
    validate_validity(valid_from, valid_until)?;
    Ok((
        canonical_timestamp(valid_from)?,
        valid_until.map(canonical_timestamp).transpose()?,
    ))
}

pub fn validate_validity(
    valid_from: &str,
    valid_until: Option<&str>,
) -> Result<(), ValidatorError> {
    let from = parse_timestamp(valid_from)?;
    if let Some(until) = valid_until {
        if parse_timestamp(until)? <= from {
            return Err(ValidatorError::InvalidValidity);
        }
    }
    Ok(())
}

pub async fn create_validator(
    db: &ServerDatabase,
    network_key: &str,
    validator_node_id: &str,
    display_name: Option<&str>,
    actor_user_id: &str,
) -> Result<(ValidatorRecord, i64), ValidatorError> {
    validate_validator_node_id(validator_node_id)?;
    validate_display_name(display_name)?;
    let now = format_rfc3339(now_utc());
    let validator_id = uuid::Uuid::new_v4().to_string();
    let mut tx = db.pool().begin().await?;

    let known: Option<i64> = sqlx::query_scalar("SELECT 1 FROM networks WHERE network_key = ?")
        .bind(network_key)
        .fetch_optional(&mut *tx)
        .await?;
    if known.is_none() {
        return Err(ValidatorError::NetworkNotFound);
    }

    let insert = sqlx::query(
        "INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&validator_id)
    .bind(network_key)
    .bind(validator_node_id)
    .bind(display_name)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await;
    if let Err(error) = insert {
        if error
            .as_database_error()
            .is_some_and(|database_error| database_error.is_unique_violation())
        {
            return Err(ValidatorError::ValidatorAlreadyExists);
        }
        return Err(ValidatorError::Database(error));
    }

    insert_audit_event(
        &mut *tx,
        Some(actor_user_id),
        "validator_created",
        "validator",
        &validator_id,
        Some(&serde_json::json!({
            "network_key": network_key,
            "validator_node_id": validator_node_id,
            "display_name": display_name,
        })),
    )
    .await?;
    let audit_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((
        ValidatorRecord {
            validator_id,
            network_key: network_key.to_owned(),
            validator_node_id: validator_node_id.to_owned(),
            display_name: display_name.map(str::to_owned),
            created_at: now.clone(),
            updated_at: now,
        },
        audit_id,
    ))
}

pub async fn get_validator(
    db: &ServerDatabase,
    validator_id: &str,
) -> Result<Option<ValidatorRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorRecord>(
        "SELECT validator_id, network_key, validator_node_id, display_name, created_at, updated_at FROM validators WHERE validator_id = ?",
    )
    .bind(validator_id)
    .fetch_optional(db.pool())
    .await?)
}

pub async fn list_validators(
    db: &ServerDatabase,
    network_key: Option<&str>,
) -> Result<Vec<ValidatorRecord>, ValidatorError> {
    let rows = if let Some(network_key) = network_key {
        sqlx::query_as::<_, ValidatorRecord>(
            "SELECT validator_id, network_key, validator_node_id, display_name, created_at, updated_at FROM validators WHERE network_key = ? ORDER BY validator_node_id, validator_id",
        )
        .bind(network_key)
        .fetch_all(db.pool())
        .await?
    } else {
        sqlx::query_as::<_, ValidatorRecord>(
            "SELECT validator_id, network_key, validator_node_id, display_name, created_at, updated_at FROM validators ORDER BY network_key, validator_node_id, validator_id",
        )
        .fetch_all(db.pool())
        .await?
    };
    Ok(rows)
}

async fn link_overlaps(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    valid_from: &str,
    valid_until: Option<&str>,
    exclude_link_id: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let mut query = String::from(
        "SELECT 1 FROM node_validator_links WHERE node_id = ? AND (? < COALESCE(valid_until, '9999-12-31T23:59:59Z')) AND (valid_until IS NULL OR valid_from < ?)",
    );
    if exclude_link_id.is_some() {
        query.push_str(" AND link_id != ?");
    }
    query.push_str(" LIMIT 1");
    let mut statement = sqlx::query_scalar::<_, i64>(&query)
        .bind(node_id)
        .bind(valid_from)
        .bind(valid_until.unwrap_or("9999-12-31T23:59:59Z"));
    if let Some(exclude_link_id) = exclude_link_id {
        statement = statement.bind(exclude_link_id);
    }
    Ok(statement.fetch_optional(&mut **tx).await?.is_some())
}

async fn validate_link_parentage(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    validator_id: &str,
) -> Result<(), ValidatorError> {
    let node = sqlx::query_as::<_, (String, String)>(
        "SELECT network_key, lifecycle FROM nodes WHERE node_id = ?",
    )
    .bind(node_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((node_network, lifecycle)) = node else {
        return Err(ValidatorError::NodeNotFound);
    };
    if lifecycle != "active" {
        return Err(ValidatorError::NodeNotActive);
    }
    let validator_network = sqlx::query_scalar::<_, String>(
        "SELECT network_key FROM validators WHERE validator_id = ?",
    )
    .bind(validator_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(validator_network) = validator_network else {
        return Err(ValidatorError::ValidatorNotFound);
    };
    if node_network != validator_network {
        return Err(ValidatorError::NetworkMismatch);
    }
    Ok(())
}

fn is_overlap_database_error(error: &sqlx::Error) -> bool {
    error.to_string().contains("node_validator_link_overlap")
}

pub async fn create_link(
    db: &ServerDatabase,
    node_id: &str,
    validator_id: &str,
    role: &str,
    valid_from: &str,
    valid_until: Option<&str>,
    actor_user_id: &str,
) -> Result<(NodeValidatorLinkRecord, i64), ValidatorError> {
    validate_role(role)?;
    let (valid_from, valid_until) = canonical_validity(valid_from, valid_until)?;
    let now = format_rfc3339(now_utc());
    let link_id = uuid::Uuid::new_v4().to_string();
    let mut tx = db.pool().begin().await?;
    validate_link_parentage(&mut tx, node_id, validator_id).await?;
    if link_overlaps(&mut tx, node_id, &valid_from, valid_until.as_deref(), None).await? {
        return Err(ValidatorError::LinkOverlap);
    }
    let insert = sqlx::query(
        "INSERT INTO node_validator_links (link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&link_id)
    .bind(node_id)
    .bind(validator_id)
    .bind(role)
    .bind(&valid_from)
    .bind(valid_until.as_deref())
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await;
    if let Err(error) = insert {
        if is_overlap_database_error(&error) {
            return Err(ValidatorError::LinkOverlap);
        }
        return Err(ValidatorError::Database(error));
    }
    insert_audit_event(
        &mut *tx,
        Some(actor_user_id),
        "node_validator_link_created",
        "node_validator_link",
        &link_id,
        Some(&serde_json::json!({
            "node_id": node_id,
            "validator_id": validator_id,
            "role": role,
            "valid_from": valid_from,
            "valid_until": valid_until,
        })),
    )
    .await?;
    let audit_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((
        NodeValidatorLinkRecord {
            link_id,
            node_id: node_id.to_owned(),
            validator_id: validator_id.to_owned(),
            role: role.to_owned(),
            valid_from: valid_from.to_owned(),
            valid_until,

            created_at: now.clone(),
            updated_at: now,
        },
        audit_id,
    ))
}

pub async fn get_link(
    db: &ServerDatabase,
    link_id: &str,
) -> Result<Option<NodeValidatorLinkRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, NodeValidatorLinkRecord>(
        "SELECT link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at FROM node_validator_links WHERE link_id = ?",
    )
    .bind(link_id)
    .fetch_optional(db.pool())
    .await?)
}

pub async fn list_links(
    db: &ServerDatabase,
    node_id: Option<&str>,
    validator_id: Option<&str>,
    network_key: Option<&str>,
) -> Result<Vec<NodeValidatorLinkRecord>, ValidatorError> {
    let mut sql = String::from(
        "SELECT l.link_id, l.node_id, l.validator_id, l.role, l.valid_from, l.valid_until, l.created_at, l.updated_at FROM node_validator_links l JOIN validators v ON v.validator_id = l.validator_id JOIN nodes n ON n.node_id = l.node_id WHERE 1=1",
    );
    if node_id.is_some() {
        sql.push_str(" AND l.node_id = ?");
    }
    if validator_id.is_some() {
        sql.push_str(" AND l.validator_id = ?");
    }
    if network_key.is_some() {
        sql.push_str(" AND v.network_key = ?");
    }
    sql.push_str(" ORDER BY l.valid_from DESC, l.link_id DESC");
    let mut query = sqlx::query_as::<_, NodeValidatorLinkRecord>(&sql);
    if let Some(value) = node_id {
        query = query.bind(value);
    }
    if let Some(value) = validator_id {
        query = query.bind(value);
    }
    if let Some(value) = network_key {
        query = query.bind(value);
    }
    Ok(query.fetch_all(db.pool()).await?)
}

pub async fn update_link(
    db: &ServerDatabase,
    link_id: &str,
    role: &str,
    valid_from: &str,
    valid_until: Option<&str>,
    actor_user_id: &str,
) -> Result<(NodeValidatorLinkRecord, i64), ValidatorError> {
    validate_role(role)?;
    let (valid_from, valid_until) = canonical_validity(valid_from, valid_until)?;
    let now = format_rfc3339(now_utc());
    let mut tx = db.pool().begin().await?;
    let existing = sqlx::query_as::<_, (String, String, String, String, Option<String>)>(
        "SELECT node_id, validator_id, role, valid_from, valid_until FROM node_validator_links WHERE link_id = ?",
    )
    .bind(link_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((node_id, validator_id, _old_role, old_from, old_until)) = existing else {
        return Err(ValidatorError::LinkNotFound);
    };
    if old_until.is_some() {
        return Err(ValidatorError::LinkAlreadyEnded);
    }
    validate_link_parentage(&mut tx, &node_id, &validator_id).await?;
    if parse_timestamp(&valid_from)? <= parse_timestamp(&old_from)? {
        return Err(ValidatorError::LinkReplacementMustAdvance);
    }
    if link_overlaps(
        &mut tx,
        &node_id,
        &valid_from,
        valid_until.as_deref(),
        Some(link_id),
    )
    .await?
    {
        return Err(ValidatorError::LinkOverlap);
    }
    sqlx::query(
        "UPDATE node_validator_links SET valid_until = ?, updated_at = ? WHERE link_id = ?",
    )
    .bind(&valid_from)
    .bind(&now)
    .bind(link_id)
    .execute(&mut *tx)
    .await?;
    let replacement_id = uuid::Uuid::new_v4().to_string();
    let insert = sqlx::query(
        "INSERT INTO node_validator_links (link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&replacement_id)
    .bind(&node_id)
    .bind(&validator_id)
    .bind(role)
    .bind(&valid_from)
    .bind(valid_until.as_deref())
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await;
    if let Err(error) = insert {
        if is_overlap_database_error(&error) {
            return Err(ValidatorError::LinkOverlap);
        }
        return Err(ValidatorError::Database(error));
    }
    insert_audit_event(
        &mut *tx,
        Some(actor_user_id),
        "node_validator_link_replaced",
        "node_validator_link",
        link_id,
        Some(&serde_json::json!({
            "replacement_link_id": replacement_id,
            "node_id": node_id,
            "validator_id": validator_id,
            "role": role,
            "valid_from": valid_from,
            "valid_until": valid_until,
        })),
    )
    .await?;
    let audit_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await?;
    let row = sqlx::query_as::<_, NodeValidatorLinkRecord>(
        "SELECT link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at FROM node_validator_links WHERE link_id = ?",
    )
    .bind(&replacement_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((row, audit_id))
}

pub async fn end_link(
    db: &ServerDatabase,
    link_id: &str,
    ended_at: Option<&str>,
    actor_user_id: &str,
) -> Result<(NodeValidatorLinkRecord, i64), ValidatorError> {
    let now = format_rfc3339(now_utc());
    let end = canonical_timestamp(ended_at.unwrap_or(&now))?;
    let end_time = parse_timestamp(&end)?;
    let mut tx = db.pool().begin().await?;
    let valid_from = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT valid_from, valid_until FROM node_validator_links WHERE link_id = ?",
    )
    .bind(link_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((valid_from, valid_until)) = valid_from else {
        return Err(ValidatorError::LinkNotFound);
    };
    if valid_until.is_some() {
        return Err(ValidatorError::LinkAlreadyEnded);
    }
    if end_time <= parse_timestamp(&valid_from)? {
        return Err(ValidatorError::EndBeforeStart);
    }
    sqlx::query(
        "UPDATE node_validator_links SET valid_until = CASE WHEN valid_until IS NULL OR valid_until > ? THEN ? ELSE valid_until END, updated_at = ? WHERE link_id = ?",
    )
    .bind(&end)
    .bind(&end)
    .bind(&now)
    .bind(link_id)
    .execute(&mut *tx)
    .await?;
    insert_audit_event(
        &mut *tx,
        Some(actor_user_id),
        "node_validator_link_ended",
        "node_validator_link",
        link_id,
        Some(&serde_json::json!({ "valid_until": end })),
    )
    .await?;
    let audit_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *tx)
        .await?;
    let row = sqlx::query_as::<_, NodeValidatorLinkRecord>(
        "SELECT link_id, node_id, validator_id, role, valid_from, valid_until, created_at, updated_at FROM node_validator_links WHERE link_id = ?",
    )
    .bind(link_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((row, audit_id))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorInsightRecord {
    pub validator_id: String,
    pub source: Option<String>,
    pub outcome: String,
    pub diagnostic: Option<String>,
    pub provider_timestamp: Option<String>,
    pub last_attempt_received_at: String,
    pub last_good_received_at: Option<String>,
    pub last_good_provider_timestamp: Option<String>,
    pub rank: Option<i64>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
    pub counter_state: String,
    pub change_state: String,
    pub candidate_previous_rank: Option<i64>,
    pub candidate_rank: Option<i64>,
    pub candidate_observations: i64,
    pub candidate_observed_at: Option<String>,
    pub candidate_provider_timestamp: Option<String>,
    pub candidate_observation_key: Option<String>,
    pub last_observation_key: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorRankingHistoryRecord {
    pub history_id: String,
    pub validator_id: String,
    pub previous_rank: Option<i64>,
    pub current_rank: i64,
    pub observed_at: String,
    pub provider_timestamp: Option<String>,
    pub observation_key: String,
    pub candidate_observed_at: Option<String>,
    pub candidate_provider_timestamp: Option<String>,
    pub candidate_observation_key: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorCounterHistoryRecord {
    pub history_id: String,
    pub validator_id: String,
    pub counter_name: String,
    pub previous_value: String,
    pub current_value: String,
    pub observed_at: String,
    pub provider_timestamp: Option<String>,
    pub observation_key: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorLinkContextRecord {
    pub link_id: String,
    pub node_id: String,
    pub role: String,
    pub valid_from: String,
    pub valid_until: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorDailySnapshotRecord {
    pub snapshot_id: String,
    pub validator_id: String,
    pub timezone: String,
    pub local_date: String,
    pub month_key: String,
    pub sample_at: String,
    pub received_at: String,
    pub provider_timestamp: Option<String>,
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

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorMonthlyAggregateRecord {
    pub aggregate_id: String,
    pub validator_id: String,
    pub timezone: String,
    pub month_key: String,
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

/// Convert the observation's provider time when available, otherwise the
/// Server receipt time, into the configured IANA calendar day and month.
/// Provider time makes delayed observations deterministic across refresh
/// retries; receipt time remains the honest fallback for providers without it.
pub fn analytics_period(
    observation: &ValidatorObservation,
    received_at: &str,
    timezone: &str,
) -> Result<(String, String, String), ValidatorError> {
    let timezone = timezone
        .parse::<Tz>()
        .map_err(|_| ValidatorError::InvalidTimezone(timezone.to_owned()))?;
    let timestamp = observation
        .provider_timestamp
        .as_deref()
        .unwrap_or(received_at);
    let parsed = parse_timestamp(timestamp)?;
    let utc = DateTime::<Utc>::from_timestamp(parsed.unix_timestamp(), parsed.nanosecond())
        .ok_or_else(|| ValidatorError::InvalidTimezone(timezone.to_string()))?;
    let local = utc.with_timezone(&timezone);
    Ok((
        local.format("%Y-%m-%d").to_string(),
        local.format("%Y-%m").to_string(),
        format_rfc3339(parsed),
    ))
}

pub async fn list_daily_snapshots(
    db: &ServerDatabase,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<ValidatorDailySnapshotRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorDailySnapshotRecord>(
        "SELECT snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count FROM validator_daily_snapshots WHERE validator_id = ? ORDER BY sample_at DESC, local_date DESC LIMIT ?",
    )
    .bind(validator_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?)
}

pub async fn list_monthly_aggregates(
    db: &ServerDatabase,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<ValidatorMonthlyAggregateRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorMonthlyAggregateRecord>(
        "SELECT aggregate_id, validator_id, timezone, month_key, snapshot_count, first_sample_at, last_sample_at, rank_min, rank_max, rank_last, stake_last, reward_last, reward_rate_last, delegator_count_last, epoch_last, block_count_last, updated_at FROM validator_monthly_aggregates WHERE validator_id = ? ORDER BY month_key DESC, timezone LIMIT ?",
    )
    .bind(validator_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?)
}

async fn record_daily_snapshot(
    tx: &mut Transaction<'_, Sqlite>,
    validator_id: &str,
    source: &str,
    observation: &ValidatorObservation,
    observation_key: &str,
    received_at: &str,
    timezone: &str,
) -> Result<(bool, String), ValidatorError> {
    let (local_date, month_key, sample_at) = analytics_period(observation, received_at, timezone)?;
    let result = sqlx::query("INSERT INTO validator_daily_snapshots (snapshot_id, validator_id, timezone, local_date, month_key, sample_at, received_at, provider_timestamp, source, observation_key, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(validator_id, timezone, local_date) DO UPDATE SET snapshot_id=excluded.snapshot_id, month_key=excluded.month_key, sample_at=excluded.sample_at, received_at=excluded.received_at, provider_timestamp=excluded.provider_timestamp, source=excluded.source, observation_key=excluded.observation_key, rank=excluded.rank, stake_amount=excluded.stake_amount, reward_amount=excluded.reward_amount, reward_rate=excluded.reward_rate, delegator_count=excluded.delegator_count, epoch=excluded.epoch, block_count=excluded.block_count WHERE excluded.sample_at > validator_daily_snapshots.sample_at OR (excluded.sample_at = validator_daily_snapshots.sample_at AND excluded.observation_key > validator_daily_snapshots.observation_key)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(validator_id)
        .bind(timezone)
        .bind(&local_date)
        .bind(&month_key)
        .bind(&sample_at)
        .bind(received_at)
        .bind(observation.provider_timestamp.as_deref())
        .bind(bounded_source(source))
        .bind(observation_key)
        .bind(observation.rank)
        .bind(observation.stake_amount.as_deref())
        .bind(observation.reward_amount.as_deref())
        .bind(observation.reward_rate.as_deref())
        .bind(observation.delegator_count)
        .bind(observation.epoch)
        .bind(observation.block_count)
        .execute(&mut **tx)
        .await?;
    Ok((result.rows_affected() > 0, month_key))
}

async fn rebuild_monthly_aggregate(
    tx: &mut Transaction<'_, Sqlite>,
    validator_id: &str,
    timezone: &str,
    month_key: &str,
    updated_at: &str,
) -> Result<(), ValidatorError> {
    let summary = sqlx::query_as::<_, (i64, String, String, Option<i64>, Option<i64>)>(
        "SELECT COUNT(*), MIN(sample_at), MAX(sample_at), MIN(rank), MAX(rank) FROM validator_daily_snapshots WHERE validator_id = ? AND timezone = ? AND month_key = ?",
    )
    .bind(validator_id)
    .bind(timezone)
    .bind(month_key)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((snapshot_count, first_sample_at, last_sample_at, rank_min, rank_max)) = summary
    else {
        return Ok(());
    };
    let latest = sqlx::query_as::<_, (Option<i64>, Option<String>, Option<String>, Option<String>, Option<i64>, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, NULL FROM validator_daily_snapshots WHERE validator_id = ? AND timezone = ? AND month_key = ? ORDER BY sample_at DESC, observation_key DESC LIMIT 1",
    )
    .bind(validator_id)
    .bind(timezone)
    .bind(month_key)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("INSERT INTO validator_monthly_aggregates (aggregate_id, validator_id, timezone, month_key, snapshot_count, first_sample_at, last_sample_at, rank_min, rank_max, rank_last, stake_last, reward_last, reward_rate_last, delegator_count_last, epoch_last, block_count_last, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(validator_id, timezone, month_key) DO UPDATE SET snapshot_count=excluded.snapshot_count, first_sample_at=excluded.first_sample_at, last_sample_at=excluded.last_sample_at, rank_min=excluded.rank_min, rank_max=excluded.rank_max, rank_last=excluded.rank_last, stake_last=excluded.stake_last, reward_last=excluded.reward_last, reward_rate_last=excluded.reward_rate_last, delegator_count_last=excluded.delegator_count_last, epoch_last=excluded.epoch_last, block_count_last=excluded.block_count_last, updated_at=excluded.updated_at")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(validator_id)
        .bind(timezone)
        .bind(month_key)
        .bind(snapshot_count)
        .bind(first_sample_at)
        .bind(last_sample_at)
        .bind(rank_min)
        .bind(rank_max)
        .bind(latest.0)
        .bind(latest.1)
        .bind(latest.2)
        .bind(latest.3)
        .bind(latest.4)
        .bind(latest.5)
        .bind(latest.6)
        .bind(updated_at)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub async fn list_ranking_history(
    db: &ServerDatabase,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<ValidatorRankingHistoryRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorRankingHistoryRecord>(
        "SELECT history_id, validator_id, previous_rank, current_rank, observed_at, provider_timestamp, observation_key, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key FROM validator_ranking_history WHERE validator_id = ? ORDER BY observed_at DESC, history_id DESC LIMIT ?",
    )
    .bind(validator_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?)
}

pub async fn list_counter_history(
    db: &ServerDatabase,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<ValidatorCounterHistoryRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorCounterHistoryRecord>(
        "SELECT history_id, validator_id, counter_name, previous_value, current_value, observed_at, provider_timestamp, observation_key FROM validator_counter_history WHERE validator_id = ? ORDER BY observed_at DESC, history_id DESC LIMIT ?",
    )
    .bind(validator_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?)
}

pub async fn list_link_context_at(
    db: &ServerDatabase,
    validator_id: &str,
    observed_at: &str,
    public_only: bool,
) -> Result<Vec<ValidatorLinkContextRecord>, ValidatorError> {
    let mut sql = String::from(
        "SELECT l.link_id, l.node_id, l.role, l.valid_from, l.valid_until FROM node_validator_links l JOIN nodes n ON n.node_id = l.node_id WHERE l.validator_id = ? AND l.valid_from <= ? AND (l.valid_until IS NULL OR l.valid_until > ?)",
    );
    if public_only {
        sql.push_str(" AND n.visibility = 'public' AND n.lifecycle = 'active'");
    }
    sql.push_str(" ORDER BY l.node_id, l.link_id");
    Ok(sqlx::query_as::<_, ValidatorLinkContextRecord>(&sql)
        .bind(validator_id)
        .bind(observed_at)
        .bind(observed_at)
        .fetch_all(db.pool())
        .await?)
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshSummary {
    pub attempted: usize,
    pub successful: usize,
    pub changed: usize,
    pub invalidations: usize,
    pub alert_invalidations: usize,
    pub invalidated_network_keys: Vec<String>,
    pub invalidated_validator_ids: Vec<String>,
}

pub async fn load_insight(
    db: &ServerDatabase,
    validator_id: &str,
) -> Result<Option<ValidatorInsightRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorInsightRecord>(
        "SELECT validator_id, source, outcome, diagnostic, provider_timestamp, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key, last_observation_key, updated_at FROM current_validator_insights WHERE validator_id = ?",
    )
    .bind(validator_id)
    .fetch_optional(db.pool())
    .await?)
}

pub async fn list_insights(
    db: &ServerDatabase,
    network_key: Option<&str>,
) -> Result<Vec<ValidatorInsightRecord>, ValidatorError> {
    let mut sql = String::from(
        "SELECT i.validator_id, i.source, i.outcome, i.diagnostic, i.provider_timestamp, i.last_attempt_received_at, i.last_good_received_at, i.last_good_provider_timestamp, i.rank, i.stake_amount, i.reward_amount, i.reward_rate, i.delegator_count, i.epoch, i.block_count, i.counter_state, i.change_state, i.candidate_previous_rank, i.candidate_rank, i.candidate_observations, i.candidate_observed_at, i.candidate_provider_timestamp, i.candidate_observation_key, i.last_observation_key, i.updated_at FROM current_validator_insights i JOIN validators v ON v.validator_id = i.validator_id",
    );
    if network_key.is_some() {
        sql.push_str(" WHERE v.network_key = ?");
    }
    sql.push_str(" ORDER BY v.network_key, v.validator_node_id, i.validator_id");
    let query = sqlx::query_as::<_, ValidatorInsightRecord>(&sql);
    let rows = if let Some(network_key) = network_key {
        query.bind(network_key).fetch_all(db.pool()).await?
    } else {
        query.fetch_all(db.pool()).await?
    };
    Ok(rows)
}

pub fn freshness(last_good_received_at: Option<&str>, now: OffsetDateTime) -> &'static str {
    let Some(received_at) = last_good_received_at.and_then(crate::auth::parse_rfc3339) else {
        return "unknown";
    };
    if (now - received_at).whole_seconds().abs() <= 120 {
        "fresh"
    } else {
        "stale"
    }
}

pub async fn refresh_all(
    db: &ServerDatabase,
    provider: &dyn ValidatorProvider,
) -> Result<RefreshSummary, ValidatorError> {
    refresh_all_with_channels_in_timezone(
        db,
        provider,
        &crate::config::NotificationChannels::default(),
        "UTC",
    )
    .await
}

pub async fn refresh_all_with_channels(
    db: &ServerDatabase,
    provider: &dyn ValidatorProvider,
    channels: &crate::config::NotificationChannels,
) -> Result<RefreshSummary, ValidatorError> {
    refresh_all_with_channels_in_timezone(db, provider, channels, "UTC").await
}

pub async fn refresh_all_with_channels_in_timezone(
    db: &ServerDatabase,
    provider: &dyn ValidatorProvider,
    channels: &crate::config::NotificationChannels,
    timezone: &str,
) -> Result<RefreshSummary, ValidatorError> {
    timezone
        .parse::<Tz>()
        .map_err(|_| ValidatorError::InvalidTimezone(timezone.to_owned()))?;
    let validators = sqlx::query_as::<_, (String, String, String)>(
        "SELECT validator_id, network_key, validator_node_id FROM validators ORDER BY validator_id",
    )
    .fetch_all(db.pool())
    .await?;
    let mut provider_results = Vec::with_capacity(validators.len());
    for (validator_id, network_key, validator_node_id) in validators {
        let result = provider.fetch(&network_key, &validator_node_id).await;
        provider_results.push((validator_id, network_key, result));
    }

    let mut summary = RefreshSummary {
        attempted: provider_results.len(),
        ..RefreshSummary::default()
    };
    let mut tx = db.pool().begin().await?;
    for (validator_id, network_key, result) in provider_results {
        let changed =
            apply_provider_result(&mut tx, provider.source(), &validator_id, result, timezone)
                .await?;
        let alert_changes = crate::alerts::evaluate_validator_in_transaction(
            &mut tx,
            &validator_id,
            channels,
            crate::auth::now_utc(),
        )
        .await
        .map_err(|error| ValidatorError::Alert(error.to_string()))?;
        if alert_changes > 0 {
            summary.alert_invalidations += alert_changes;
        }
        if changed.0 {
            summary.successful += 1;
        }
        if changed.1 {
            summary.changed += 1;
        }
        if changed.2 {
            summary.invalidations += 1;
            summary.invalidated_network_keys.push(network_key);
            summary.invalidated_validator_ids.push(validator_id);
        }
    }
    tx.commit().await?;
    Ok(summary)
}

async fn apply_provider_result(
    tx: &mut Transaction<'_, Sqlite>,
    source: &str,
    validator_id: &str,
    result: ValidatorProviderResult,
    timezone: &str,
) -> Result<(bool, bool, bool), ValidatorError> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let existing = sqlx::query_as::<_, ValidatorInsightRecord>(
        "SELECT validator_id, source, outcome, diagnostic, provider_timestamp, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key, last_observation_key, updated_at FROM current_validator_insights WHERE validator_id = ?",
    )
    .bind(validator_id)
    .fetch_optional(&mut **tx)
    .await?;
    match result {
        ValidatorProviderResult::Success(observation) => {
            validate_observation(&observation)?;
            let key = observation_key(&observation);
            if existing
                .as_ref()
                .and_then(|row| row.last_observation_key.as_deref())
                == Some(key.as_str())
            {
                let counter_state = "normal";
                let change_state = "normal";
                sqlx::query(
                    "UPDATE current_validator_insights SET outcome = 'success', diagnostic = NULL, last_attempt_received_at = ?, last_good_received_at = ?, counter_state = ?, change_state = ?, updated_at = ? WHERE validator_id = ?",
                )
                .bind(&now)
                .bind(&now)
                .bind(counter_state)
                .bind(change_state)
                .bind(&now)
                .bind(validator_id)
                .execute(&mut **tx)
                .await?;
                let (analytics_changed, month_key) = record_daily_snapshot(
                    tx,
                    validator_id,
                    source,
                    &observation,
                    &key,
                    &now,
                    timezone,
                )
                .await?;
                rebuild_monthly_aggregate(tx, validator_id, timezone, &month_key, &now).await?;
                return Ok((true, false, analytics_changed));
            }

            // A rank-less success cannot establish ranking evidence. The first
            // successful rank establishes the baseline and is never a change.
            let baseline_exists = existing
                .as_ref()
                .is_some_and(|row| row.last_good_received_at.is_some() && row.rank.is_some());
            let previous_rank = existing.as_ref().and_then(|row| row.rank);
            let decreases = counter_decreases(existing.as_ref(), &observation);
            let counter_changed = !decreases.is_empty();
            let counter_state = if counter_changed {
                "counter_reset"
            } else {
                "normal"
            };

            let mut candidate_previous_rank = existing
                .as_ref()
                .and_then(|row| row.candidate_previous_rank);
            let mut candidate_rank = existing.as_ref().and_then(|row| row.candidate_rank);
            let mut candidate_observations = existing
                .as_ref()
                .map_or(0, |row| row.candidate_observations);
            let mut candidate_observed_at = existing
                .as_ref()
                .and_then(|row| row.candidate_observed_at.clone());
            let mut candidate_provider_timestamp = existing
                .as_ref()
                .and_then(|row| row.candidate_provider_timestamp.clone());
            let mut candidate_observation_key = existing
                .as_ref()
                .and_then(|row| row.candidate_observation_key.clone());
            let mut confirmed_ranking_change = false;

            if baseline_exists {
                match (previous_rank, observation.rank) {
                    (Some(previous), Some(current)) if previous == current => {
                        candidate_previous_rank = None;
                        candidate_rank = None;
                        candidate_observations = 0;
                        candidate_observed_at = None;
                        candidate_provider_timestamp = None;
                        candidate_observation_key = None;
                    }
                    (Some(previous), Some(current))
                        if candidate_previous_rank == Some(previous)
                            && candidate_rank == Some(current)
                            && candidate_observations == 1 =>
                    {
                        sqlx::query("INSERT OR IGNORE INTO validator_ranking_history (history_id, validator_id, previous_rank, current_rank, observed_at, provider_timestamp, observation_key, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                            .bind(uuid::Uuid::new_v4().to_string())
                            .bind(validator_id)
                            .bind(previous)
                            .bind(current)
                            .bind(&now)
                            .bind(observation.provider_timestamp.as_deref())
                            .bind(&key)
                            .bind(candidate_observed_at.as_deref())
                            .bind(candidate_provider_timestamp.as_deref())
                            .bind(candidate_observation_key.as_deref())
                            .execute(&mut **tx)
                            .await?;
                        confirmed_ranking_change = true;
                        candidate_previous_rank = None;
                        candidate_rank = None;
                        candidate_observations = 0;
                        candidate_observed_at = None;
                        candidate_provider_timestamp = None;
                        candidate_observation_key = None;
                    }
                    (Some(previous), Some(current)) => {
                        candidate_previous_rank = Some(previous);
                        candidate_rank = Some(current);
                        candidate_observations = 1;
                        candidate_observed_at = Some(now.clone());
                        candidate_provider_timestamp = observation.provider_timestamp.clone();
                        candidate_observation_key = Some(key.clone());
                    }
                    _ => {
                        candidate_previous_rank = None;
                        candidate_rank = None;
                        candidate_observations = 0;
                        candidate_observed_at = None;
                        candidate_provider_timestamp = None;
                        candidate_observation_key = None;
                    }
                }
            } else {
                candidate_previous_rank = None;
                candidate_rank = None;
                candidate_observations = 0;
                candidate_observed_at = None;
                candidate_provider_timestamp = None;
                candidate_observation_key = None;
            }

            for (counter_name, previous_value, current_value) in decreases {
                sqlx::query("INSERT OR IGNORE INTO validator_counter_history (history_id, validator_id, counter_name, previous_value, current_value, observed_at, provider_timestamp, observation_key) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                    .bind(uuid::Uuid::new_v4().to_string())
                    .bind(validator_id)
                    .bind(counter_name)
                    .bind(previous_value)
                    .bind(current_value)
                    .bind(&now)
                    .bind(observation.provider_timestamp.as_deref())
                    .bind(&key)
                    .execute(&mut **tx)
                    .await?;
            }

            let stored_rank = if candidate_rank.is_some() && !confirmed_ranking_change {
                previous_rank
            } else {
                observation.rank
            };
            let change_state = if confirmed_ranking_change {
                "ranking_changed"
            } else {
                "normal"
            };
            let source = bounded_source(source);
            sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key, last_observation_key, updated_at) VALUES (?, ?, 'success', NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(validator_id) DO UPDATE SET source=excluded.source, outcome=excluded.outcome, diagnostic=NULL, provider_timestamp=excluded.provider_timestamp, last_attempt_received_at=excluded.last_attempt_received_at, last_good_received_at=excluded.last_good_received_at, last_good_provider_timestamp=excluded.last_good_provider_timestamp, rank=excluded.rank, stake_amount=excluded.stake_amount, reward_amount=excluded.reward_amount, reward_rate=excluded.reward_rate, delegator_count=excluded.delegator_count, epoch=excluded.epoch, block_count=excluded.block_count, counter_state=excluded.counter_state, change_state=excluded.change_state, candidate_previous_rank=excluded.candidate_previous_rank, candidate_rank=excluded.candidate_rank, candidate_observations=excluded.candidate_observations, candidate_observed_at=excluded.candidate_observed_at, candidate_provider_timestamp=excluded.candidate_provider_timestamp, candidate_observation_key=excluded.candidate_observation_key, last_observation_key=excluded.last_observation_key, updated_at=excluded.updated_at")
                .bind(validator_id)
                .bind(&source)
                .bind(observation.provider_timestamp.as_deref())
                .bind(&now)
                .bind(&now)
                .bind(observation.provider_timestamp.as_deref())
                .bind(stored_rank)
                .bind(observation.stake_amount.as_deref())
                .bind(observation.reward_amount.as_deref())
                .bind(observation.reward_rate.as_deref())
                .bind(observation.delegator_count)
                .bind(observation.epoch)
                .bind(observation.block_count)
                .bind(counter_state)
                .bind(change_state)
                .bind(candidate_previous_rank)
                .bind(candidate_rank)
                .bind(candidate_observations)
                .bind(candidate_observed_at)
                .bind(candidate_provider_timestamp)
                .bind(candidate_observation_key)
                .bind(&key)
                .bind(&now)
                .execute(&mut **tx)
                .await?;
            let (analytics_changed, month_key) = record_daily_snapshot(
                tx,
                validator_id,
                &source,
                &observation,
                &key,
                &now,
                timezone,
            )
            .await?;
            rebuild_monthly_aggregate(tx, validator_id, timezone, &month_key, &now).await?;
            Ok((
                true,
                confirmed_ranking_change,
                confirmed_ranking_change || counter_changed || analytics_changed,
            ))
        }
        outcome => {
            let (name, diagnostic) = match outcome {
                ValidatorProviderResult::NotFound => ("not_found", None),
                ValidatorProviderResult::AuthoritativeEmpty => ("empty", None),
                ValidatorProviderResult::Error(value) => {
                    ("error", Some(provider_diagnostic(value)))
                }
                ValidatorProviderResult::Unsupported(value) => {
                    ("unsupported", Some(provider_diagnostic(value)))
                }
                ValidatorProviderResult::Success(_) => unreachable!(),
            };
            let invalidated = existing
                .as_ref()
                .is_none_or(|row| row.outcome != name || row.diagnostic != diagnostic);
            let source = bounded_source(source);
            sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch, block_count, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key, last_observation_key, updated_at) VALUES (?, ?, ?, ?, NULL, ?, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'normal', 'normal', NULL, NULL, 0, NULL, NULL, NULL, NULL, ?) ON CONFLICT(validator_id) DO UPDATE SET source=excluded.source, outcome=excluded.outcome, diagnostic=excluded.diagnostic, last_attempt_received_at=excluded.last_attempt_received_at, provider_timestamp=current_validator_insights.provider_timestamp, last_good_received_at=current_validator_insights.last_good_received_at, last_good_provider_timestamp=current_validator_insights.last_good_provider_timestamp, rank=current_validator_insights.rank, stake_amount=current_validator_insights.stake_amount, reward_amount=current_validator_insights.reward_amount, reward_rate=current_validator_insights.reward_rate, delegator_count=current_validator_insights.delegator_count, epoch=current_validator_insights.epoch, block_count=current_validator_insights.block_count, counter_state=current_validator_insights.counter_state, change_state='normal', candidate_previous_rank=NULL, candidate_rank=NULL, candidate_observations=0, candidate_observed_at=NULL, candidate_provider_timestamp=NULL, candidate_observation_key=NULL, last_observation_key=current_validator_insights.last_observation_key, updated_at=excluded.updated_at")
                .bind(validator_id)
                .bind(source)
                .bind(name)
                .bind(diagnostic)
                .bind(&now)
                .bind(&now)
                .execute(&mut **tx)
                .await?;
            Ok((false, false, invalidated))
        }
    }
}

fn bounded_source(source: &str) -> String {
    source.chars().take(64).collect()
}

fn validate_observation(observation: &ValidatorObservation) -> Result<(), ValidatorError> {
    let has_supported_value = observation.rank.is_some()
        || observation.stake_amount.is_some()
        || observation.reward_amount.is_some()
        || observation.reward_rate.is_some()
        || observation.delegator_count.is_some()
        || observation.epoch.is_some()
        || observation.block_count.is_some();
    if !has_supported_value {
        return Err(ValidatorError::InvalidProviderObservation(
            "empty observation".to_owned(),
        ));
    }
    if observation.rank.is_some_and(|value| value < 0)
        || observation.delegator_count.is_some_and(|value| value < 0)
        || observation.epoch.is_some_and(|value| value < 0)
        || observation.block_count.is_some_and(|value| value < 0)
    {
        return Err(ValidatorError::InvalidProviderObservation(
            "negative integer".to_owned(),
        ));
    }
    for value in [
        observation.stake_amount.as_deref(),
        observation.reward_amount.as_deref(),
        observation.reward_rate.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        normalize_bounded_text(value).map_err(ValidatorError::InvalidProviderObservation)?;
        validate_nonnegative_decimal(value).map_err(ValidatorError::InvalidProviderObservation)?;
    }
    if let Some(timestamp) = observation.provider_timestamp.as_deref() {
        canonical_timestamp(timestamp)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::database::{ServerDatabaseConfig, initialize};
    use crate::network::create_network;

    use super::*;

    async fn test_db() -> (tempfile::TempDir, ServerDatabase) {
        let dir = tempdir().unwrap();
        let db = initialize(ServerDatabaseConfig::new(dir.path().join("server.db")))
            .await
            .unwrap();
        create_network(
            &db,
            "platon-mainnet",
            "Mainnet",
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            1,
            1,
            "lat",
        )
        .await
        .unwrap();
        crate::auth::create_owner(
            &db,
            "owner",
            &crate::auth::hash_password(b"validator-test-password").unwrap(),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-1', 1, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, lifecycle, visibility, inventory_revision, first_seen_at, updated_at, rpc_endpoint) VALUES ('node-1', 'agent-1', 'platon-mainnet', 'active', 'private', 1, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 'http://127.0.0.1:1')")
            .execute(db.pool())
            .await
            .unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn duplicate_identity_and_temporal_overlap_are_rejected() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let duplicate = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap_err();
        assert!(matches!(duplicate, ValidatorError::ValidatorAlreadyExists));
        create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "primary",
            "2025-01-01T00:00:00Z",
            Some("2025-02-01T00:00:00Z"),
            &owner_id,
        )
        .await
        .unwrap();
        let overlap = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "standby",
            "2025-01-15T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap_err();
        assert!(matches!(overlap, ValidatorError::LinkOverlap));
        let second = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "observer",
            "2025-02-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        assert_eq!(second.0.role, "observer");
    }

    #[tokio::test]
    async fn validity_is_canonical_and_roles_can_change() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0x123", None, &owner_id)
            .await
            .unwrap();
        let (link, _) = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "observer",
            "2025-01-01T01:00:00+01:00",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        assert_eq!(link.valid_from, "2025-01-01T00:00:00Z");
        assert_eq!(link.valid_until, None);
        let (updated, _) = update_link(
            &db,
            &link.link_id,
            "primary",
            "2025-01-01T12:00:00Z",
            Some("2025-01-02T00:00:00Z"),
            &owner_id,
        )
        .await
        .unwrap();
        assert_eq!(updated.role, "primary");
        assert_eq!(updated.valid_from, "2025-01-01T12:00:00Z");
        assert_eq!(
            list_links(&db, Some("node-1"), None, None)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[derive(Default)]
    struct FakeProvider {
        results: std::sync::Mutex<Vec<ValidatorProviderResult>>,
        calls: std::sync::Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl ValidatorProvider for FakeProvider {
        fn source(&self) -> &str {
            "fake"
        }

        async fn fetch(
            &self,
            network_key: &str,
            validator_node_id: &str,
        ) -> ValidatorProviderResult {
            self.calls
                .lock()
                .unwrap()
                .push((network_key.to_owned(), validator_node_id.to_owned()));
            self.results.lock().unwrap().remove(0)
        }
    }

    #[test]
    fn explorer_response_normalization_preserves_numeric_strings_and_rejects_invalid_data() {
        let value = serde_json::json!({
            "rank": "7",
            "stake": "123456789012345678901234567890.123456789",
            "reward_rate": "0.125000000000000001",
            "epoch": 42,
            "timestamp": "2025-01-01T00:00:00Z"
        });
        let observation = normalize_explorer_response(&value).unwrap().unwrap();
        assert_eq!(observation.rank, Some(7));
        assert_eq!(
            observation.stake_amount.as_deref(),
            Some("123456789012345678901234567890.123456789")
        );
        assert_eq!(
            observation.reward_rate.as_deref(),
            Some("0.125000000000000001")
        );
        assert!(normalize_explorer_response(&serde_json::json!({"stake": "-1"})).is_err());
        assert_eq!(
            normalize_explorer_response(&serde_json::json!({"data": []})).unwrap(),
            None
        );
        assert_eq!(
            normalize_explorer_response(&serde_json::json!({"data": [{"rank": 3}]}))
                .unwrap()
                .unwrap()
                .rank,
            Some(3)
        );
        assert!(validate_observation(&ValidatorObservation::default()).is_err());
    }

    #[test]
    fn explorer_path_segments_are_encoded_without_leaking_raw_identifiers() {
        assert_eq!(url_path_segment("node/a?b"), "node%2Fa%3Fb");
    }
    #[tokio::test]
    async fn provider_refresh_preserves_last_good_and_confirms_rank_after_two_observations() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xprovider", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    rank: Some(1),
                    stake_amount: Some("123456789012345678901234567890".to_owned()),
                    reward_rate: Some("0.125000000000000001".to_owned()),
                    ..ValidatorObservation::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:01:00Z".to_owned()),
                    rank: Some(2),
                    stake_amount: Some("123456789012345678901234567889".to_owned()),
                    ..ValidatorObservation::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:02:00Z".to_owned()),
                    rank: Some(2),
                    stake_amount: Some("123456789012345678901234567888".to_owned()),
                    ..ValidatorObservation::default()
                }),
                ValidatorProviderResult::Error(
                    "provider timeout at https://secret.example".to_owned(),
                ),
            ]),
            ..FakeProvider::default()
        };
        let first = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(first.attempted, 1);
        assert_eq!(first.invalidations, 1);
        let second = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(second.changed, 0);
        assert_eq!(second.invalidations, 1);
        let third = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(third.changed, 1);
        assert_eq!(third.invalidations, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM validator_ranking_history")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "success");
        assert_eq!(insight.rank, Some(2));
        assert_eq!(
            insight.stake_amount.as_deref(),
            Some("123456789012345678901234567888")
        );
        assert_eq!(insight.counter_state, "counter_reset");
        let incidents = sqlx::query_as::<_, (String, String, String)>(
            "SELECT rule_key, state, opened_evidence_json FROM alert_incidents WHERE subject_key = ? ORDER BY rule_key",
        )
        .bind(&validator.validator_id)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            incidents.len(),
            2,
            "ranking and counter signals are independent"
        );
        assert!(incidents.iter().all(|(rule_key, state, _)| state == "open"
            && (rule_key == "validator.ranking_changed" || rule_key == "validator.counter_reset")));
        let counter_evidence = incidents
            .iter()
            .find(|(rule_key, _, _)| rule_key == "validator.counter_reset")
            .map(|(_, _, evidence)| evidence)
            .unwrap();
        assert!(counter_evidence.contains("123456789012345678901234567890"));
        assert!(counter_evidence.contains("123456789012345678901234567889"));
        let ranking_state: (bool, String) = sqlx::query_as(
            "SELECT evaluation_unavailable, input_kind FROM alert_rule_state WHERE rule_key = 'validator.ranking_changed' AND subject_key = ?",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(ranking_state, (false, "known".to_owned()));
        let counter_state: (bool, String) = sqlx::query_as(
            "SELECT evaluation_unavailable, input_kind FROM alert_rule_state WHERE rule_key = 'validator.counter_reset' AND subject_key = ?",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(counter_state, (false, "known".to_owned()));
        refresh_all(&db, &provider).await.unwrap();
        let failed = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.outcome, "error");
        assert_eq!(failed.rank, Some(2));
        assert_eq!(
            failed.stake_amount.as_deref(),
            Some("123456789012345678901234567888")
        );
        let failed_diagnostic = failed.diagnostic.unwrap_or_default();
        assert!(failed_diagnostic.contains("provider timeout"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM alert_incidents WHERE subject_key = ? AND state = 'open'",
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            2
        );
        let unavailable: Vec<(bool, String)> = sqlx::query_as(
            "SELECT evaluation_unavailable, input_kind FROM alert_rule_state WHERE subject_key = ? AND rule_key LIKE 'validator.%' ORDER BY rule_key",
        )
        .bind(&validator.validator_id)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            unavailable,
            vec![
                (true, "unsupported".to_owned()),
                (true, "unsupported".to_owned())
            ]
        );
    }

    #[tokio::test]
    async fn ranking_changes_need_consecutive_successes_and_replay_is_idempotent() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xrank", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    rank: Some(1),
                    ..Default::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:01:00Z".to_owned()),
                    rank: Some(2),
                    ..Default::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:01:00Z".to_owned()),
                    rank: Some(2),
                    ..Default::default()
                }),
                ValidatorProviderResult::Error("temporary failure".to_owned()),
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:02:00Z".to_owned()),
                    rank: Some(2),
                    ..Default::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:03:00Z".to_owned()),
                    rank: Some(2),
                    ..Default::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:03:00Z".to_owned()),
                    rank: Some(2),
                    ..Default::default()
                }),
            ]),
            ..FakeProvider::default()
        };

        refresh_all(&db, &provider).await.unwrap();
        refresh_all(&db, &provider).await.unwrap();
        let candidate = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(candidate.rank, Some(1));
        assert_eq!(candidate.candidate_rank, Some(2));
        db.close().await;
        let db = initialize(ServerDatabaseConfig::new(_dir.path().join("server.db")))
            .await
            .unwrap();
        refresh_all(&db, &provider).await.unwrap();
        let replayed_candidate = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replayed_candidate.rank, Some(1));
        assert_eq!(replayed_candidate.candidate_rank, Some(2));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_ranking_history WHERE validator_id = ?"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            0
        );
        refresh_all(&db, &provider).await.unwrap();
        let after_failure = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after_failure.outcome, "error");
        assert_eq!(after_failure.candidate_rank, None);
        refresh_all(&db, &provider).await.unwrap();
        refresh_all(&db, &provider).await.unwrap();
        let confirmed = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(confirmed.rank, Some(2));
        assert_eq!(confirmed.candidate_rank, None);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_ranking_history WHERE validator_id = ?"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );
        refresh_all(&db, &provider).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_ranking_history WHERE validator_id = ?"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn counter_correction_history_keeps_exact_decimal_evidence() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xcounter", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(ValidatorObservation {
                    stake_amount: Some("100.000000000000000001".to_owned()),
                    ..Default::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    stake_amount: Some("99.999999999999999999".to_owned()),
                    ..Default::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    stake_amount: Some("99.999999999999999999".to_owned()),
                    ..Default::default()
                }),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        refresh_all(&db, &provider).await.unwrap();
        refresh_all(&db, &provider).await.unwrap();
        let row = sqlx::query_as::<_, ValidatorCounterHistoryRecord>(
            "SELECT history_id, validator_id, counter_name, previous_value, current_value, observed_at, provider_timestamp, observation_key FROM validator_counter_history WHERE validator_id = ?",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(row.counter_name, "stake_amount");
        assert_eq!(row.previous_value, "100.000000000000000001");
        assert_eq!(row.current_value, "99.999999999999999999");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_counter_history WHERE validator_id = ?"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );
    }
    #[tokio::test]
    async fn ending_a_link_preserves_history_and_allows_replacement() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xdef", None, &owner_id)
            .await
            .unwrap();
        let (link, _) = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "primary",
            "2025-01-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        end_link(&db, &link.link_id, Some("2025-03-01T00:00:00Z"), &owner_id)
            .await
            .unwrap();
        assert!(matches!(
            end_link(&db, &link.link_id, Some("2025-03-02T00:00:00Z"), &owner_id).await,
            Err(ValidatorError::LinkAlreadyEnded)
        ));

        let (replacement, _) = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "standby",
            "2025-03-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        assert_eq!(replacement.role, "standby");
        assert_eq!(
            list_links(&db, Some("node-1"), None, None)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn analytics_period_uses_configured_iana_timezone_and_calendar_months() {
        let observation = ValidatorObservation {
            provider_timestamp: Some("2025-03-01T00:30:00Z".to_owned()),
            ..Default::default()
        };
        let received_at = "2025-03-02T01:00:00Z";
        let tokyo = analytics_period(&observation, received_at, "Asia/Tokyo").unwrap();
        assert_eq!(tokyo.0, "2025-03-01");
        assert_eq!(tokyo.1, "2025-03");
        assert_eq!(tokyo.2, "2025-03-01T00:30:00Z");
        let los_angeles =
            analytics_period(&observation, received_at, "America/Los_Angeles").unwrap();
        assert_eq!(los_angeles.0, "2025-02-28");
        assert_eq!(los_angeles.1, "2025-02");
        assert_eq!(los_angeles.2, "2025-03-01T00:30:00Z");

        // Without a provider timestamp, the Server receipt time is the honest
        // fallback and is converted in the same configured timezone.
        let no_provider_time = ValidatorObservation::default();
        let fallback =
            analytics_period(&no_provider_time, "2025-03-02T01:00:00Z", "Asia/Tokyo").unwrap();
        assert_eq!(fallback.0, "2025-03-02");
        assert_eq!(fallback.1, "2025-03");

        // Calendar-month rollover is local-time based.
        let month_end = ValidatorObservation {
            provider_timestamp: Some("2025-02-28T23:30:00Z".to_owned()),
            ..Default::default()
        };
        let tokyo_month_end = analytics_period(&month_end, received_at, "Asia/Tokyo").unwrap();
        assert_eq!(tokyo_month_end.0, "2025-03-01");
        assert_eq!(tokyo_month_end.1, "2025-03");

        // Daylight-saving transitions do not split a local calendar day.
        let before_dst = ValidatorObservation {
            provider_timestamp: Some("2025-03-09T06:59:00Z".to_owned()),
            ..Default::default()
        };
        let after_dst = ValidatorObservation {
            provider_timestamp: Some("2025-03-09T07:01:00Z".to_owned()),
            ..Default::default()
        };
        let before = analytics_period(&before_dst, received_at, "America/New_York").unwrap();
        let after = analytics_period(&after_dst, received_at, "America/New_York").unwrap();
        assert_eq!(before.0, "2025-03-09");
        assert_eq!(after.0, "2025-03-09");
        assert_eq!(before.1, after.1);

        assert!(matches!(
            analytics_period(&observation, received_at, "Not/AZone"),
            Err(ValidatorError::InvalidTimezone(_))
        ));
    }

    #[tokio::test]
    async fn analytics_snapshots_are_calendar_scoped_and_not_multiplied_by_linked_nodes() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, lifecycle, visibility, inventory_revision, first_seen_at, updated_at, rpc_endpoint) VALUES ('node-2', 'agent-1', 'platon-mainnet', 'active', 'private', 1, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 'http://127.0.0.1:2')")
            .execute(db.pool())
            .await
            .unwrap();
        let (validator, _) =
            create_validator(&db, "platon-mainnet", "0xanalytics", None, &owner_id)
                .await
                .unwrap();
        create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "primary",
            "2025-01-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        create_link(
            &db,
            "node-2",
            &validator.validator_id,
            "standby",
            "2025-01-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();

        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-31T15:30:00Z".to_owned()),
                    rank: Some(1),
                    stake_amount: Some("10".to_owned()),
                    ..Default::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-02-28T15:30:00Z".to_owned()),
                    rank: Some(2),
                    stake_amount: Some("20".to_owned()),
                    ..Default::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-02-28T15:30:00Z".to_owned()),
                    rank: Some(2),
                    stake_amount: Some("20".to_owned()),
                    ..Default::default()
                }),
            ]),
            ..FakeProvider::default()
        };

        let channels = crate::config::NotificationChannels::default();
        for _ in 0..3 {
            let summary =
                refresh_all_with_channels_in_timezone(&db, &provider, &channels, "Asia/Tokyo")
                    .await
                    .unwrap();
            assert_eq!(
                summary.attempted, 1,
                "one Validator is fetched once per refresh"
            );
        }
        assert_eq!(provider.calls.lock().unwrap().len(), 3);

        let daily = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(
            daily.len(),
            2,
            "two local calendar days, no per-Node duplication"
        );
        assert_eq!(daily[0].local_date, "2025-03-01");
        assert_eq!(daily[0].month_key, "2025-03");
        assert_eq!(daily[0].rank, Some(2));
        assert_eq!(daily[1].local_date, "2025-02-01");
        assert_eq!(daily[1].month_key, "2025-02");
        assert_eq!(daily[1].rank, Some(1));

        let monthly = list_monthly_aggregates(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(monthly.len(), 2);
        assert_eq!(monthly[0].month_key, "2025-03");
        assert_eq!(monthly[0].snapshot_count, 1);
        assert_eq!(monthly[0].rank_last, Some(2));
        assert_eq!(monthly[1].month_key, "2025-02");
        assert_eq!(monthly[1].snapshot_count, 1);
        assert_eq!(monthly[1].rank_last, Some(1));
    }

    #[tokio::test]
    async fn delayed_observation_uses_provider_calendar_day_and_replay_is_idempotent() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xdelayed", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:30:00Z".to_owned()),
                    rank: Some(7),
                    ..Default::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:30:00Z".to_owned()),
                    rank: Some(7),
                    ..Default::default()
                }),
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:30:00Z".to_owned()),
                    rank: Some(7),
                    ..Default::default()
                }),
            ]),
            ..FakeProvider::default()
        };
        let channels = crate::config::NotificationChannels::default();
        refresh_all_with_channels_in_timezone(&db, &provider, &channels, "America/Los_Angeles")
            .await
            .unwrap();
        refresh_all_with_channels_in_timezone(&db, &provider, &channels, "America/Los_Angeles")
            .await
            .unwrap();

        let daily = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].local_date, "2024-12-31");
        assert_eq!(daily[0].month_key, "2024-12");
        assert_eq!(daily[0].rank, Some(7));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_monthly_aggregates WHERE validator_id = ?"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );

        // A Server restart must not replay or duplicate the accepted sample.
        db.close().await;
        let db = initialize(ServerDatabaseConfig::new(_dir.path().join("server.db")))
            .await
            .unwrap();
        refresh_all_with_channels_in_timezone(&db, &provider, &channels, "America/Los_Angeles")
            .await
            .unwrap();
        let daily_after_restart = list_daily_snapshots(&db, &validator.validator_id, 10)
            .await
            .unwrap();
        assert_eq!(daily_after_restart.len(), 1);
        let monthly_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM validator_monthly_aggregates WHERE validator_id = ?",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(monthly_count, 1);
    }

    #[tokio::test]
    async fn provider_failure_keeps_analytics_and_marks_current_state_not_healthy() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xstate", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    rank: Some(3),
                    stake_amount: Some("300".to_owned()),
                    ..Default::default()
                }),
                ValidatorProviderResult::Error("provider timeout".to_owned()),
            ]),
            ..FakeProvider::default()
        };
        let channels = crate::config::NotificationChannels::default();
        refresh_all_with_channels_in_timezone(&db, &provider, &channels, "UTC")
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM validator_daily_snapshots")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        refresh_all_with_channels_in_timezone(&db, &provider, &channels, "UTC")
            .await
            .unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.rank, Some(3));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM validator_daily_snapshots")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1,
            "a failed provider attempt must not add an analytics sample"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_monthly_aggregates WHERE validator_id = ?"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );
    }
}
