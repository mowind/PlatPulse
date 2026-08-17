//! Server-managed Validator identities and explicit Node Validator Links.
//!
//! Validators are keyed by `(Network, validator_node_id)` and are never
//! inferred from Agent reports, consensus membership, provider data, or
//! Node identity. Link mutations are transactional with their Audit Event and
//! reject every temporal overlap for one Node before inserting or updating.

use sqlx::{FromRow, Sqlite, Transaction};
use thiserror::Error;
use time::OffsetDateTime;

use crate::auth::{format_rfc3339, insert_audit_event, now_utc};
use crate::database::ServerDatabase;

pub const MAX_VALIDATOR_NODE_ID_LEN: usize = 256;
pub const MAX_VALIDATOR_DISPLAY_NAME_LEN: usize = 128;

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
}
