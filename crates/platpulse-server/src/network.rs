//! Network Registry storage (design §7.1): the Server-managed set of
//! expected Network identities.
//!
//! Phase 1 provides the registry storage plus local CLI bootstrap
//! (`platpulse-server network create`). The Registry is the only authority
//! for Network keys: an unknown key reported by an Agent is rejected with
//! the frozen `RejectionCode::NetworkKeyUnknown` at ingestion time and is
//! never auto-created or rewritten from Agent free text or observations.
//! Display name, `network_key`, genesis hash, chain ID, P2P network ID and
//! address HRP are one tuple: the key alone is never identity.

use std::str::FromStr;

use platpulse_core::hex::Hash32;
use platpulse_core::network::NetworkKey;
use sqlx::FromRow;
use thiserror::Error;

use crate::auth::{format_rfc3339, insert_audit_event, now_utc};
use crate::database::ServerDatabase;

/// Maximum length of a registered Network display name.
pub const MAX_DISPLAY_NAME_LEN: usize = 128;

/// Maximum length of a registered address HRP (matches the v1 wire limit).
pub const MAX_ADDRESS_HRP_LEN: usize = 16;

/// Largest chain/P2P network id the registry stores (SQLite INTEGER is
/// signed 64-bit; the v1 wire type is u64, real PlatON ids are far below).
pub const MAX_NETWORK_ID: u64 = i64::MAX as u64;

/// One registered Network row.
#[derive(Debug, Clone, FromRow)]
pub struct NetworkRecord {
    pub network_key: String,
    pub display_name: String,
    pub genesis_hash: String,
    pub chain_id: i64,
    pub p2p_network_id: i64,
    pub address_hrp: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("invalid network key: {0}")]
    InvalidKey(#[source] platpulse_core::network::NetworkKeyError),
    #[error("{0}")]
    InvalidDisplayName(&'static str),
    #[error("invalid genesis hash: {0}")]
    InvalidGenesisHash(#[source] platpulse_core::hex::HexError),
    #[error("{0}")]
    InvalidAddressHrp(&'static str),
    #[error("{0}")]
    InvalidNetworkId(&'static str),
    #[error("network key '{0}' is already registered")]
    AlreadyExists(String),
    #[error("server database initialization failed: {0}")]
    ServerDatabase(#[from] crate::database::ServerDatabaseError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Validate the full identity tuple a `network create` invocation must
/// carry (design §7.1). Returns the canonical key string.
pub fn validate_network_tuple(
    key: &str,
    display_name: &str,
    genesis_hash: &str,
    chain_id: u64,
    p2p_network_id: u64,
    address_hrp: &str,
) -> Result<String, NetworkError> {
    let key = key
        .parse::<NetworkKey>()
        .map_err(NetworkError::InvalidKey)?;
    if display_name.is_empty() || display_name.len() > MAX_DISPLAY_NAME_LEN {
        return Err(NetworkError::InvalidDisplayName(
            "display name must be 1..=128 characters",
        ));
    }
    if display_name.chars().any(|c| c.is_control()) {
        return Err(NetworkError::InvalidDisplayName(
            "display name must not contain control characters",
        ));
    }
    Hash32::from_str(genesis_hash).map_err(NetworkError::InvalidGenesisHash)?;
    if chain_id > MAX_NETWORK_ID {
        return Err(NetworkError::InvalidNetworkId(
            "chain id is too large for the registry",
        ));
    }
    if p2p_network_id > MAX_NETWORK_ID {
        return Err(NetworkError::InvalidNetworkId(
            "p2p network id is too large for the registry",
        ));
    }
    if address_hrp.is_empty() || address_hrp.len() > MAX_ADDRESS_HRP_LEN {
        return Err(NetworkError::InvalidAddressHrp(
            "address hrp must be 1..=16 characters",
        ));
    }
    if address_hrp
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(NetworkError::InvalidAddressHrp(
            "address hrp must not contain whitespace or control characters",
        ));
    }
    Ok(key.as_str().to_owned())
}

/// Register a Network from the local CLI. The row and its minimal audit
/// event commit in one transaction (design §7.1, §18.2). The Registry is
/// never created from Agent input, so this is the only insert path.
pub async fn create_network(
    db: &ServerDatabase,
    key: &str,
    display_name: &str,
    genesis_hash: &str,
    chain_id: u64,
    p2p_network_id: u64,
    address_hrp: &str,
) -> Result<NetworkRecord, NetworkError> {
    let key = validate_network_tuple(
        key,
        display_name,
        genesis_hash,
        chain_id,
        p2p_network_id,
        address_hrp,
    )?;
    let now = format_rfc3339(now_utc());

    let mut transaction = db.pool().begin().await?;
    let insert = sqlx::query(
        "INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&key)
    .bind(display_name)
    .bind(genesis_hash)
    .bind(chain_id as i64)
    .bind(p2p_network_id as i64)
    .bind(address_hrp)
    .bind(&now)
    .bind(&now)
    .execute(&mut *transaction)
    .await;
    if let Err(error) = insert {
        if error
            .as_database_error()
            .is_some_and(|db_error| db_error.is_unique_violation())
        {
            return Err(NetworkError::AlreadyExists(key));
        }
        return Err(NetworkError::Database(error));
    }

    let after = serde_json::json!({
        "network_key": key,
        "display_name": display_name,
        "genesis_hash": genesis_hash,
        "chain_id": chain_id,
        "p2p_network_id": p2p_network_id,
        "address_hrp": address_hrp,
    });
    insert_audit_event(
        &mut *transaction,
        None,
        "network_created",
        "network",
        &key,
        Some(&after),
    )
    .await?;

    transaction.commit().await?;
    Ok(NetworkRecord {
        network_key: key,
        display_name: display_name.to_owned(),
        genesis_hash: genesis_hash.to_owned(),
        chain_id: chain_id as i64,
        p2p_network_id: p2p_network_id as i64,
        address_hrp: address_hrp.to_owned(),
        created_at: now.clone(),
        updated_at: now,
    })
}

/// Look up a registered Network by its key. Read-only: an unknown key
/// returns `None` and never creates or rewrites a Registry entry, which is
/// what ingestion maps to the stable `RejectionCode::NetworkKeyUnknown`.
pub async fn find_network_by_key(
    db: &ServerDatabase,
    key: &str,
) -> Result<Option<NetworkRecord>, sqlx::Error> {
    sqlx::query_as::<_, NetworkRecord>(
        "SELECT network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at FROM networks WHERE network_key = ?",
    )
    .bind(key)
    .fetch_optional(db.pool())
    .await
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::database::{ServerDatabaseConfig, initialize};

    use super::*;

    async fn test_db() -> (tempfile::TempDir, ServerDatabase) {
        let dir = tempdir().unwrap();
        let db = initialize(ServerDatabaseConfig::new(dir.path().join("server.db")))
            .await
            .unwrap();
        (dir, db)
    }

    fn genesis() -> &'static str {
        "0x0000000000000000000000000000000000000000000000000000000000000001"
    }

    #[tokio::test]
    async fn create_network_writes_tuple_and_audit_atomically() {
        let (_dir, db) = test_db().await;
        let record = create_network(
            &db,
            "platon-mainnet",
            "PlatON Mainnet",
            genesis(),
            210_425,
            1,
            "lat",
        )
        .await
        .unwrap();
        assert_eq!(record.network_key, "platon-mainnet");
        assert_eq!(record.chain_id, 210_425);
        assert_eq!(record.address_hrp, "lat");

        let found = find_network_by_key(&db, "platon-mainnet")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.display_name, "PlatON Mainnet");
        assert_eq!(found.genesis_hash, genesis());
        assert_eq!(found.p2p_network_id, 1);

        let audit: (String, String, String) = sqlx::query_as(
            "SELECT event_kind, target_kind, target_id FROM audit_events WHERE event_kind = 'network_created'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            audit,
            (
                "network_created".into(),
                "network".into(),
                "platon-mainnet".into()
            )
        );
    }

    #[tokio::test]
    async fn duplicate_key_is_rejected_without_a_second_row() {
        let (_dir, db) = test_db().await;
        create_network(&db, "platon-mainnet", "Mainnet", genesis(), 1, 1, "lat")
            .await
            .unwrap();
        let error = create_network(&db, "platon-mainnet", "Other", genesis(), 1, 1, "lat")
            .await
            .unwrap_err();
        assert!(matches!(error, NetworkError::AlreadyExists(_)));

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM networks")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
        let audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind = 'network_created'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(audit_count, 1);
    }

    #[tokio::test]
    async fn unknown_key_lookup_never_auto_creates_a_network() {
        let (_dir, db) = test_db().await;
        let found = find_network_by_key(&db, "platon-testnet").await.unwrap();
        assert!(found.is_none());

        // No Agent-visible path may rewrite the Registry: after the lookup
        // the table is still empty, so ingestion has nothing to accept and
        // maps the missing key to the frozen NetworkKeyUnknown code.
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM networks")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);

        // The stable rejection code for the missing key is frozen in core:
        // `network_key_unknown`, never auto-created.
        assert_eq!(
            serde_json::to_value(platpulse_core::RejectionCode::NetworkKeyUnknown).unwrap(),
            serde_json::json!("network_key_unknown")
        );
    }

    #[test]
    fn tuple_validation_rejects_partial_identity() {
        let genesis = format!("0x{}", "a".repeat(64));
        assert!(validate_network_tuple("platon-mainnet", "Mainnet", &genesis, 1, 1, "lat").is_ok());
        assert!(matches!(
            validate_network_tuple("Platon-Mainnet", "Mainnet", &genesis, 1, 1, "lat"),
            Err(NetworkError::InvalidKey(_))
        ));
        assert!(matches!(
            validate_network_tuple("platon-mainnet", "", &genesis, 1, 1, "lat"),
            Err(NetworkError::InvalidDisplayName(_))
        ));
        assert!(matches!(
            validate_network_tuple("platon-mainnet", "Mainnet", "0xzz", 1, 1, "lat"),
            Err(NetworkError::InvalidGenesisHash(_))
        ));
        assert!(matches!(
            validate_network_tuple("platon-mainnet", "Mainnet", &genesis, 1, 1, ""),
            Err(NetworkError::InvalidAddressHrp(_))
        ));
        assert!(matches!(
            validate_network_tuple("platon-mainnet", "Mainnet", &genesis, u64::MAX, 1, "lat"),
            Err(NetworkError::InvalidNetworkId(_))
        ));
        assert!(matches!(
            validate_network_tuple("platon-mainnet", "Mainnet", &genesis, 1, u64::MAX, "lat"),
            Err(NetworkError::InvalidNetworkId(_))
        ));
    }
}
