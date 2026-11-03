//! Automatic Validator identity discovery from observed P2P public keys.

use crate::auth::{format_rfc3339, now_utc};
use crate::database::ServerDatabase;
use sqlx::{Any, FromRow, Transaction};

use super::error::ValidatorError;
use super::registry::parse_timestamp;

/// Extract the full 64-byte P2P public key from an observed enode URI as the
/// canonical `0x` + 128 lowercase hex lookup identifier (#173).
///
/// The P2P public key is the chain identity, not a PlatPulse Node UUID, a
/// shortened fingerprint, a display name, or an IP address. A malformed or
/// shortened key is never truncated into a lookup candidate: it stays an
/// explicit unidentified reason.
pub fn observed_p2p_public_key(enode: &str) -> Result<String, String> {
    let rest = enode
        .strip_prefix("enode://")
        .ok_or_else(|| "observed enode must start with enode://".to_owned())?;
    let key = rest.split('@').next().unwrap_or_default();
    let digits = key.strip_prefix("0x").unwrap_or(key);
    if digits.len() != 128 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("observed enode public key must be 64 bytes of hexadecimal".to_owned());
    }
    Ok(format!("0x{}", digits.to_ascii_lowercase()))
}

/// Sanitized, non-sensitive explanation for a Node whose automatic Validator
/// identity is not established (#173, #218). `identified` has no reason, and a
/// caller that does not project the discovery dimension at all passes `None`.
/// Every other state is explained without disclosing secrets and without
/// implying ownership or consensus membership.
pub fn automatic_identity_reason(state: Option<&str>) -> Option<String> {
    match state {
        Some("identified") | None => None,
        Some("not_evaluated") => Some(
            "No automatic Validator identity evaluation has been recorded for this Node yet."
                .to_owned(),
        ),
        Some("missing_public_key") => Some(
            "No full P2P public key has been observed for this Node, so no Validator can be identified."
                .to_owned(),
        ),
        Some("invalid_public_key") => Some(
            "The observed P2P public key could not be validated, so no Validator was searched."
                .to_owned(),
        ),
        Some("network_identity_missing") => Some(
            "No Network Identity has been observed for this Node, so no Validator can be identified."
                .to_owned(),
        ),
        Some("network_identity_mismatch") => Some(
            "The observed Network Identity does not match this Node's registered Network; no cross-Network Validator was searched."
                .to_owned(),
        ),
        Some(_) => Some("No Validator identity has been established for this Node.".to_owned()),
    }
}

/// One Node's Server-recorded automatic Validator identity: the discovery
/// evidence state (#173) plus the currently open automatic Link interval.
/// Nothing here is a manual role, an ownership claim, or a consensus-membership
/// statement.
#[derive(Debug, Clone, FromRow)]
pub struct NodeValidatorIdentityRecord {
    pub node_id: String,
    pub node_display_name: Option<String>,
    pub network_key: String,
    pub lifecycle: String,
    /// Discovery state of the last evaluation; `None` when never evaluated.
    pub state: Option<String>,
    /// Full P2P public key currently observed from the Node, when identified.
    pub observed_node_key: Option<String>,
    /// When the discovery dimension last evaluated this Node.
    pub evaluated_at: Option<String>,
    /// The Validator of the currently open automatic Link interval, if any.
    pub validator_id: Option<String>,
    /// That Validator's own chain identity key.
    pub validator_node_key: Option<String>,
}

impl NodeValidatorIdentityRecord {
    /// Whether the open interval is effective for the Public projection. The
    /// interval is Server-recorded evidence on its own; Public additionally
    /// requires the Node to be Active, so an inactive Node keeps its identity
    /// history without a projected association (#173, #218).
    pub fn association_effective(&self) -> bool {
        self.validator_id.is_some() && self.lifecycle == "active"
    }
}

/// List each Node's automatic Validator identity coverage, optionally narrowed
/// to one Node. The open automatic interval is selected with the same temporal
/// predicate the Public projection uses, so Admin never reports a
/// correspondence that Public would not, or the reverse.
pub async fn list_node_validator_identities(
    db: &ServerDatabase,
    node_id: Option<&str>,
) -> Result<Vec<NodeValidatorIdentityRecord>, ValidatorError> {
    let now = format_rfc3339(now_utc());
    let mut sql = String::from(
        "SELECT n.node_id, n.display_name AS node_display_name, n.network_key, n.lifecycle, s.state, s.observed_node_key, s.updated_at AS evaluated_at, l.validator_id, v.validator_node_id AS validator_node_key FROM nodes n LEFT JOIN node_validator_identity_status s ON s.node_id = n.node_id LEFT JOIN node_validator_links l ON l.link_id = (SELECT l2.link_id FROM node_validator_links l2 WHERE l2.node_id = n.node_id AND l2.origin = 'automatic' AND l2.valid_from <= ? AND (l2.valid_until IS NULL OR l2.valid_until > ?) ORDER BY l2.valid_from DESC, l2.link_id DESC LIMIT 1) LEFT JOIN validators v ON v.validator_id = l.validator_id",
    );
    if node_id.is_some() {
        sql.push_str(" WHERE n.node_id = ?");
    }
    sql.push_str(" ORDER BY n.network_key, n.node_id");
    let sql = crate::database::number_placeholders(&sql);
    let query = sqlx::query_as::<_, NodeValidatorIdentityRecord>(&sql)
        .bind(&now)
        .bind(&now);
    let rows = if let Some(node_id) = node_id {
        query.bind(node_id).fetch_all(db.pool()).await?
    } else {
        query.fetch_all(db.pool()).await?
    };
    Ok(rows)
}

/// One Active Node's automatic-identity inputs: the observed Network Identity
/// and enode plus the registered Network Identity they must match. The full P2P
/// public key is only ever derived from the observed enode (#173).
#[derive(Debug, FromRow)]
struct IdentityCandidateRow {
    node_id: String,
    network_key: String,
    network_genesis_hash: Option<String>,
    network_chain_id: Option<i64>,
    network_p2p_network_id: Option<i64>,
    network_address_hrp: Option<String>,
    enode: Option<String>,
    registered_genesis_hash: String,
    registered_chain_id: i64,
    registered_p2p_network_id: i64,
    registered_address_hrp: String,
}

/// Result of one automatic-identity discovery pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IdentityDiscoverySummary {
    /// Active Nodes examined.
    pub considered: usize,
    /// Nodes with a matching registered Network Identity and a full key.
    pub identified: usize,
    /// Automatic Link intervals opened by this pass.
    pub newly_linked: usize,
    /// Open automatic Link intervals closed because the chain key changed.
    pub closed_intervals: usize,
    /// Nodes left unidentified, each with a persisted reason.
    pub unidentified: usize,
}

impl IdentityDiscoverySummary {
    /// Whether this pass opened or closed an automatic Link, so a surface that
    /// projects the automatic model has something new to read. A pass that only
    /// re-confirmed the state it had already stored changed nothing a page shows.
    pub fn changed_associations(&self) -> bool {
        self.newly_linked > 0 || self.closed_intervals > 0
    }
}

/// Identify Node Validator Links automatically from each Active Node's
/// validated Network and observed full P2P public key (#173, main design
/// §15.4, ADR 0005).
///
/// The Server is the only writer of the automatic model. A Node whose observed
/// Network Identity is missing or does not match its registered Network, or
/// whose enode is absent or malformed, is recorded with an explicit reason and
/// is never guessed into a correspondence. A changed chain key closes the
/// previous interval before the new one opens; the two Validators' cumulative
/// values are never spliced. Legacy manual Links are never used as a fallback:
/// their rows are left for the one-time migration (#174).
pub async fn discover_automatic_links(
    db: &ServerDatabase,
) -> Result<IdentityDiscoverySummary, ValidatorError> {
    let now = format_rfc3339(now_utc());
    let mut tx = crate::database::begin(db.pool()).await?;
    let rows = sqlx::query_as::<_, IdentityCandidateRow>(
        "SELECT n.node_id, n.network_key, c.network_genesis_hash, c.network_chain_id, c.network_p2p_network_id, c.network_address_hrp, c.enode, r.genesis_hash AS registered_genesis_hash, r.chain_id AS registered_chain_id, r.p2p_network_id AS registered_p2p_network_id, r.address_hrp AS registered_address_hrp FROM nodes n JOIN networks r ON r.network_key = n.network_key LEFT JOIN current_node_chain_observations c ON c.node_id = n.node_id WHERE n.lifecycle = 'active' ORDER BY n.node_id",
    )
    .fetch_all(&mut *tx)
    .await?;

    let mut summary = IdentityDiscoverySummary {
        considered: rows.len(),
        ..IdentityDiscoverySummary::default()
    };
    for row in rows {
        let identity_observed = row.network_genesis_hash.is_some()
            && row.network_chain_id.is_some()
            && row.network_p2p_network_id.is_some();
        let identity_matches = row.network_genesis_hash.as_deref()
            == Some(row.registered_genesis_hash.as_str())
            && row.network_chain_id == Some(row.registered_chain_id)
            && row.network_p2p_network_id == Some(row.registered_p2p_network_id)
            // The address HRP is optional Network Identity evidence: an absent
            // value is not a mismatch, but a present value that disagrees is.
            && row
                .network_address_hrp
                .as_deref()
                .is_none_or(|hrp| hrp == row.registered_address_hrp.as_str());
        let (state, observed_key) = if !identity_observed {
            ("network_identity_missing", None)
        } else if !identity_matches {
            ("network_identity_mismatch", None)
        } else {
            match row.enode.as_deref() {
                None => ("missing_public_key", None),
                Some(enode) => match observed_p2p_public_key(enode) {
                    Ok(key) => ("identified", Some(key)),
                    Err(_) => ("invalid_public_key", None),
                },
            }
        };

        if state == "identified" {
            let key = observed_key
                .as_deref()
                .expect("an identified Node always has an observed key");
            sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ($1, $2, $3, NULL, $4, $5) ON CONFLICT (network_key, validator_node_id) DO NOTHING")
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&row.network_key)
                .bind(key)
                .bind(&now)
                .bind(&now)
                .execute(&mut *tx)
                .await?;
            let validator_id: String = sqlx::query_scalar(
                "SELECT validator_id FROM validators WHERE network_key = $1 AND validator_node_id = $2",
            )
            .bind(&row.network_key)
            .bind(key)
            .fetch_one(&mut *tx)
            .await?;

            let open: Option<(String, String, String)> = sqlx::query_as(
                "SELECT link_id, validator_id, valid_from FROM node_validator_links WHERE node_id = $1 AND origin = 'automatic' AND (valid_until IS NULL OR valid_until > $2) ORDER BY valid_from DESC, link_id DESC LIMIT 1",
            )
            .bind(&row.node_id)
            .bind(&now)
            .fetch_optional(&mut *tx)
            .await?;

            match open {
                Some((_, current, _)) if current == validator_id => {}
                Some((link_id, _, valid_from)) => {
                    // Close the old interval strictly before opening the new
                    // one, so the overlap trigger and the CHECK hold even when
                    // both happen inside the same second.
                    let close_at = close_interval_boundary(&now, &valid_from)?;
                    sqlx::query(
                        "UPDATE node_validator_links SET valid_until = $1, updated_at = $2 WHERE link_id = $3",
                    )
                    .bind(&close_at)
                    .bind(&now)
                    .bind(&link_id)
                    .execute(&mut *tx)
                    .await?;
                    summary.closed_intervals += 1;
                    insert_automatic_link(&mut tx, &row.node_id, &validator_id, &close_at, &now)
                        .await?;
                    summary.newly_linked += 1;
                }
                None => {
                    insert_automatic_link(&mut tx, &row.node_id, &validator_id, &now, &now).await?;
                    summary.newly_linked += 1;
                }
            }
            summary.identified += 1;
        } else {
            // Contradictory evidence ends the previous association interval so
            // Public never keeps projecting a Validator the Node's observed
            // identity no longer supports. Absence of evidence (a missing
            // observation or key) is not contradiction: the last established
            // association is retained and marked stale by Provider freshness.
            if matches!(state, "network_identity_mismatch" | "invalid_public_key") {
                let open: Option<(String, String)> = sqlx::query_as(
                    "SELECT link_id, valid_from FROM node_validator_links WHERE node_id = $1 AND origin = 'automatic' AND (valid_until IS NULL OR valid_until > $2) ORDER BY valid_from DESC, link_id DESC LIMIT 1",
                )
                .bind(&row.node_id)
                .bind(&now)
                .fetch_optional(&mut *tx)
                .await?;
                if let Some((link_id, valid_from)) = open {
                    let close_at = close_interval_boundary(&now, &valid_from)?;
                    sqlx::query(
                        "UPDATE node_validator_links SET valid_until = $1, updated_at = $2 WHERE link_id = $3",
                    )
                    .bind(&close_at)
                    .bind(&now)
                    .bind(&link_id)
                    .execute(&mut *tx)
                    .await?;
                    summary.closed_intervals += 1;
                }
            }
            summary.unidentified += 1;
        }

        sqlx::query("INSERT INTO node_validator_identity_status (node_id, state, observed_node_key, updated_at) VALUES ($1, $2, $3, $4) ON CONFLICT(node_id) DO UPDATE SET state = excluded.state, observed_node_key = excluded.observed_node_key, updated_at = excluded.updated_at")
            .bind(&row.node_id)
            .bind(state)
            .bind(observed_key.as_deref())
            .bind(&now)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(summary)
}

/// The boundary at which an old interval closes. It must be strictly after the
/// old interval's valid_from; a same-second key change advances by one second
/// instead of writing an impossible interval.
fn close_interval_boundary(now: &str, valid_from: &str) -> Result<String, ValidatorError> {
    let now = parse_timestamp(now)?;
    let from = parse_timestamp(valid_from)?;
    let boundary = if now > from {
        now
    } else {
        from + time::Duration::seconds(1)
    };
    Ok(format_rfc3339(boundary))
}

async fn insert_automatic_link(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    validator_id: &str,
    valid_from: &str,
    updated_at: &str,
) -> Result<(), ValidatorError> {
    sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at) VALUES ($1, $2, $3, NULL, 'automatic', $4, NULL, $5, $6)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(node_id)
        .bind(validator_id)
        .bind(valid_from)
        .bind(updated_at)
        .bind(updated_at)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {

    use crate::database::ServerDatabase;

    use crate::validator::registry::{create_link, create_validator};
    use crate::validator::test_helpers::test_db;

    use super::*;

    fn full_node_key(byte: u8) -> String {
        format!("0x{}", format!("{byte:02x}").repeat(64))
    }

    fn enode_uri(key: &str) -> String {
        format!("enode://{}@10.0.0.1:30303", &key[2..])
    }

    const REGISTERED_GENESIS: &str =
        "0x0000000000000000000000000000000000000000000000000000000000000001";

    async fn set_chain_observation(
        db: &ServerDatabase,
        node_id: &str,
        genesis: &str,
        chain_id: i64,
        p2p: i64,
        hrp: &str,
        enode: Option<&str>,
    ) {
        sqlx::query("INSERT INTO current_node_chain_observations (node_id, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, enode, updated_at) VALUES ($1, $2, $3, $4, $5, $6, '2026-01-01T00:00:00Z') ON CONFLICT(node_id) DO UPDATE SET network_genesis_hash=excluded.network_genesis_hash, network_chain_id=excluded.network_chain_id, network_p2p_network_id=excluded.network_p2p_network_id, network_address_hrp=excluded.network_address_hrp, enode=excluded.enode, updated_at=excluded.updated_at")
            .bind(node_id)
            .bind(genesis)
            .bind(chain_id)
            .bind(p2p)
            .bind(hrp)
            .bind(enode)
            .execute(db.pool())
            .await
            .unwrap();
    }

    async fn identity_state(db: &ServerDatabase, node_id: &str) -> String {
        sqlx::query_scalar("SELECT state FROM node_validator_identity_status WHERE node_id = $1")
            .bind(node_id)
            .fetch_one(db.pool())
            .await
            .unwrap()
    }

    #[test]
    fn automatic_identity_reason_explains_every_unresolved_state() {
        // An identified Node, or a caller that does not project the discovery
        // dimension, carries no reason.
        assert!(automatic_identity_reason(Some("identified")).is_none());
        assert!(automatic_identity_reason(None).is_none());
        for state in [
            "not_evaluated",
            "missing_public_key",
            "invalid_public_key",
            "network_identity_missing",
            "network_identity_mismatch",
            "something_unknown",
        ] {
            let reason = automatic_identity_reason(Some(state))
                .unwrap_or_else(|| panic!("{state} must carry a reason"));
            assert!(!reason.is_empty());
            // The copy never claims ownership, consensus membership, or a
            // negative verdict.
            assert!(!reason.to_lowercase().contains("owner"));
            assert!(!reason.to_lowercase().contains("not a validator"));
        }
    }

    #[test]
    fn a_discovery_pass_reports_an_association_change_only_when_it_made_one() {
        let mut summary = IdentityDiscoverySummary {
            considered: 3,
            identified: 1,
            unidentified: 2,
            ..IdentityDiscoverySummary::default()
        };
        assert!(!summary.changed_associations());
        summary.newly_linked = 1;
        assert!(summary.changed_associations());
        summary.newly_linked = 0;
        summary.closed_intervals = 1;
        assert!(summary.changed_associations());
    }

    #[test]
    fn association_effectiveness_requires_an_open_interval_and_an_active_node() {
        let record = |lifecycle: &str, validator_id: Option<&str>| NodeValidatorIdentityRecord {
            node_id: "0195f2a1-0000-4000-8000-000000000001".to_owned(),
            node_display_name: None,
            network_key: "platon-mainnet".to_owned(),
            lifecycle: lifecycle.to_owned(),
            state: Some("identified".to_owned()),
            observed_node_key: Some(format!("0x{}", "ab".repeat(64))),
            evaluated_at: Some("2026-01-01T00:00:00Z".to_owned()),
            validator_id: validator_id.map(str::to_owned),
            validator_node_key: validator_id.map(str::to_owned),
        };
        assert!(record("active", Some("validator-1")).association_effective());
        // An inactive Node keeps its recorded interval without projecting it.
        assert!(!record("inactive", Some("validator-1")).association_effective());
        assert!(!record("active", None).association_effective());
    }

    #[test]
    fn observed_p2p_public_key_requires_the_full_key() {
        let key = full_node_key(0xab);
        assert_eq!(observed_p2p_public_key(&enode_uri(&key)).unwrap(), key);
        // A key carried without a host is still the full observed key.
        assert_eq!(
            observed_p2p_public_key(&format!("enode://{}", &key[2..])).unwrap(),
            key
        );
        // Uppercase hex normalizes to the canonical lowercase form.
        assert_eq!(
            observed_p2p_public_key(&format!("enode://{}@h", "AB".repeat(64))).unwrap(),
            key
        );
        // A shortened key, a wrong scheme, or a missing key is never truncated
        // into a lookup candidate.
        assert!(observed_p2p_public_key("enode://abcd@h").is_err());
        assert!(observed_p2p_public_key(&format!("http://{}@h", &key[2..])).is_err());
        assert!(observed_p2p_public_key("enode://@h").is_err());
    }

    #[tokio::test]
    async fn discovery_identifies_the_full_key_and_is_idempotent() {
        let (_dir, db) = test_db().await;
        let key = full_node_key(0x11);
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some(&enode_uri(&key)),
        )
        .await;
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.identified, 1);
        assert_eq!(summary.newly_linked, 1);
        assert_eq!(summary.unidentified, 0);
        assert_eq!(identity_state(&db, "node-1").await, "identified");
        let validator_node_id: String = sqlx::query_scalar(
            "SELECT validator_node_id FROM validators WHERE network_key = 'platon-mainnet'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(validator_node_id, key);
        let (origin, valid_until): (String, Option<String>) = sqlx::query_as(
            "SELECT origin, valid_until FROM node_validator_links WHERE node_id = 'node-1'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(origin, "automatic");
        assert_eq!(valid_until, None);

        // A second pass over the same key must not open a duplicate interval.
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.newly_linked, 0);
        let links: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_validator_links WHERE node_id = 'node-1'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(links, 1);
    }

    #[tokio::test]
    async fn discovery_records_a_reason_instead_of_guessing() {
        let (_dir, db) = test_db().await;
        let key = full_node_key(0x31);

        // No observation at all.
        discover_automatic_links(&db).await.unwrap();
        assert_eq!(
            identity_state(&db, "node-1").await,
            "network_identity_missing"
        );

        // Matching identity but no observed enode.
        set_chain_observation(&db, "node-1", REGISTERED_GENESIS, 1, 1, "lat", None).await;
        discover_automatic_links(&db).await.unwrap();
        assert_eq!(identity_state(&db, "node-1").await, "missing_public_key");

        // A malformed enode is not truncated into a lookup candidate.
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some("enode://abcd@host"),
        )
        .await;
        discover_automatic_links(&db).await.unwrap();
        assert_eq!(identity_state(&db, "node-1").await, "invalid_public_key");

        // An observed Network Identity that does not match the registered
        // Network is never searched against another Network.
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            999,
            1,
            "lat",
            Some(&enode_uri(&key)),
        )
        .await;
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.unidentified, 1);
        assert_eq!(
            identity_state(&db, "node-1").await,
            "network_identity_mismatch"
        );
        let validators: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM validators")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(validators, 0);
        let links: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM node_validator_links")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(links, 0);

        // A legacy manual Link is never a fallback for a missing observation.
        sqlx::query("INSERT INTO validators (validator_id, network_key, validator_node_id, display_name, created_at, updated_at) VALUES ('legacy', 'platon-mainnet', '0xlegacy', NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_validator_links (link_id, node_id, validator_id, role, origin, valid_from, created_at, updated_at) VALUES ('legacy-link', 'node-1', 'legacy', 'primary', 'manual', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(db.pool())
            .await
            .unwrap();
        discover_automatic_links(&db).await.unwrap();
        let automatic: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_validator_links WHERE node_id = 'node-1' AND origin = 'automatic'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            automatic, 0,
            "a manual link is never the automatic fallback"
        );
    }

    #[tokio::test]
    async fn discovery_closes_the_old_interval_on_a_chain_key_change() {
        let (_dir, db) = test_db().await;
        let key_a = full_node_key(0x21);
        let key_b = full_node_key(0x22);
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some(&enode_uri(&key_a)),
        )
        .await;
        discover_automatic_links(&db).await.unwrap();

        // The chain key changes: the old association interval ends and the new
        // identity is identified without merging the two Validators.
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some(&enode_uri(&key_b)),
        )
        .await;
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.closed_intervals, 1);
        assert_eq!(summary.newly_linked, 1);

        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT v.validator_node_id, l.valid_until FROM node_validator_links l JOIN validators v ON v.validator_id = l.validator_id WHERE l.node_id = 'node-1' AND l.origin = 'automatic' ORDER BY l.valid_from, l.link_id",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, key_a);
        assert!(rows[0].1.is_some(), "the old interval must be closed");
        assert_eq!(rows[1].0, key_b);
        assert_eq!(rows[1].1, None);
        let validators: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM validators WHERE network_key = 'platon-mainnet'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(validators, 2, "the two chain identities stay independent");
    }

    #[tokio::test]
    async fn automatic_and_manual_overlap_triggers_are_partitioned_by_origin() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xmanual", None, &owner_id)
            .await
            .unwrap();
        create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "primary",
            "2026-01-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        // A legacy manual interval must not block automatic discovery, and the
        // automatic interval must not collide with it.
        let key = full_node_key(0x41);
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some(&enode_uri(&key)),
        )
        .await;
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.newly_linked, 1);
        let origins: Vec<String> = sqlx::query_scalar(
            "SELECT origin FROM node_validator_links WHERE node_id = 'node-1' ORDER BY origin",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(origins, vec!["automatic".to_owned(), "manual".to_owned()]);
    }

    #[tokio::test]
    async fn discovery_ends_the_interval_on_contradiction_but_keeps_last_good_on_absence() {
        let (_dir, db) = test_db().await;
        let key = full_node_key(0x51);
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            1,
            1,
            "lat",
            Some(&enode_uri(&key)),
        )
        .await;
        discover_automatic_links(&db).await.unwrap();
        let open = || {
            let pool = db.pool().clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM node_validator_links WHERE node_id = 'node-1' AND origin = 'automatic' AND valid_until IS NULL",
                )
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(open().await, 1);

        // A transient loss of key evidence retains the last established
        // association; absence is not contradiction.
        set_chain_observation(&db, "node-1", REGISTERED_GENESIS, 1, 1, "lat", None).await;
        discover_automatic_links(&db).await.unwrap();
        assert_eq!(
            open().await,
            1,
            "missing evidence must not drop a last-good association"
        );
        assert_eq!(identity_state(&db, "node-1").await, "missing_public_key");

        // An observed Network Identity that contradicts the registered Network
        // ends the old interval instead of projecting it as a fallback.
        set_chain_observation(
            &db,
            "node-1",
            REGISTERED_GENESIS,
            999,
            1,
            "lat",
            Some(&enode_uri(&key)),
        )
        .await;
        let summary = discover_automatic_links(&db).await.unwrap();
        assert_eq!(summary.closed_intervals, 1);
        assert_eq!(
            open().await,
            0,
            "contradictory evidence must end the old association"
        );
        assert_eq!(
            identity_state(&db, "node-1").await,
            "network_identity_mismatch"
        );
    }
}
