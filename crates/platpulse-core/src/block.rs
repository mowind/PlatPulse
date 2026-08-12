//! Block Summary: the per-Node observation of one block, with operational
//! metadata and Block Production Attribution but without transaction bodies.
//!
//! Encoding rules: `block_timestamp_ms` is the authoritative chain time as
//! Unix milliseconds; `observed_at` is the Agent wall clock; `block_interval_ms`
//! is derived from adjacent authoritative block timestamps; `source`
//! distinguishes the normal subscription flow from bounded Gap Backfill.

use serde::{Deserialize, Serialize};

use crate::hex::{Address, FingerprintHex, Hash32};
use crate::identity::NodeId;
use crate::network::NetworkIdentity;
use crate::time::Rfc3339;

/// How a Block Summary was collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockSource {
    /// Normal per-Node `newHeads` subscription ingestion.
    Subscription,
    /// Bounded point-query backfill of a registered recoverable gap.
    GapBackfill,
}

/// One observed block. The Server verifies `number`/`hash`/`parent_hash`
/// against the subscribed header before accepting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockSummary {
    /// The Node this block belongs to (must be in the report Inventory).
    pub node_id: NodeId,
    /// The Network Identity observed with this block; the Server validates
    /// it against the Registry and stops merging history on mismatch.
    pub network_identity: NetworkIdentity,
    /// Block height.
    pub block_number: u64,
    /// Block hash as resolved by `platon_getBlockByHash`.
    pub block_hash: Hash32,
    /// Parent block hash.
    pub parent_hash: Hash32,
    /// Authoritative chain timestamp of the block, Unix milliseconds.
    pub block_timestamp_ms: u64,
    /// Agent wall-clock time the block was observed.
    pub observed_at: Rfc3339,
    /// Number of transaction hashes in the block body (authoritative 0 for
    /// an empty block).
    pub transaction_count: u64,
    /// Interval to the previous authoritative block timestamp; omitted when
    /// the previous block is unknown.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub block_interval_ms: Option<u64>,
    /// Collection source: `subscription` or `gap_backfill`.
    pub source: BlockSource,
    /// Block Production Attribution evidence.
    pub attribution: BlockProductionAttribution,
}

/// Evidence describing how an observed block relates to the monitored Node.
///
/// Coinbase, Seal Signer Match, and Protocol Proposer are distinct concepts
/// and are never collapsed into one inferred producer flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockProductionAttribution {
    /// Header coinbase address (a header field, not a producer proof).
    pub coinbase: Address,
    /// Recovered seal-signing key fingerprint; omitted when the key cannot
    /// be recovered or the fork rules are not verified.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub seal_signer_key_fingerprint: Option<FingerprintHex>,
    /// Comparison of the recovered seal signer against the Node's P2P key.
    pub seal_signer_match: SealSignerMatch,
    /// Protocol proposer; `verified` only with authoritative consensus
    /// evidence (contract limit on identity: 128 chars).
    pub protocol_proposer: ProtocolProposer,
    /// Why this attribution is what it is (contract limit: 256 chars).
    pub attribution_reason: String,
}

/// Tri-state comparison between the block's seal-signing key and the
/// monitored Node's recorded P2P key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SealSignerMatch {
    /// Recovered seal key matches the current valid Node key.
    #[serde(rename = "self")]
    SignerSelf,
    /// A signer is known and does not match the Node key.
    Other,
    /// Key missing, parse failed, fork unverified, or key history incomplete.
    Unknown,
}

/// Protocol proposer of the block. Remains unknown unless authoritative
/// consensus evidence identifies it; Coinbase, validator membership, and QC
/// membership are insufficient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProtocolProposer {
    /// Authoritative evidence identifies the proposer. `identity` is the
    /// Network-scoped validator node identifier (hex string).
    Verified {
        /// Validator node identifier of the verified proposer.
        identity: String,
    },
    /// No authoritative evidence; default for v1.
    Unknown {},
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_source_wire_forms() {
        assert_eq!(
            serde_json::to_string(&BlockSource::Subscription).unwrap(),
            "\"subscription\""
        );
        assert_eq!(
            serde_json::to_string(&BlockSource::GapBackfill).unwrap(),
            "\"gap_backfill\""
        );
        assert!(serde_json::from_str::<BlockSource>("\"polling\"").is_err());
    }

    #[test]
    fn seal_signer_match_wire_forms() {
        assert_eq!(
            serde_json::to_string(&SealSignerMatch::SignerSelf).unwrap(),
            "\"self\""
        );
        assert_eq!(
            serde_json::to_string(&SealSignerMatch::Other).unwrap(),
            "\"other\""
        );
        assert_eq!(
            serde_json::to_string(&SealSignerMatch::Unknown).unwrap(),
            "\"unknown\""
        );
    }

    #[test]
    fn protocol_proposer_wire_forms() {
        let verified = ProtocolProposer::Verified {
            identity: format!("0x{}", "c".repeat(64)),
        };
        assert_eq!(
            serde_json::to_string(&verified).unwrap(),
            format!(r#"{{"kind":"verified","identity":"0x{}"}}"#, "c".repeat(64))
        );
        assert_eq!(
            serde_json::to_string(&ProtocolProposer::Unknown {}).unwrap(),
            r#"{"kind":"unknown"}"#
        );
        // Unknown payload fields are rejected, not silently dropped.
        assert!(
            serde_json::from_str::<ProtocolProposer>(r#"{"kind":"unknown","bogus":1}"#).is_err()
        );
        assert!(serde_json::from_str::<ProtocolProposer>(r#"{"kind":"verified"}"#).is_err());
        assert!(
            serde_json::from_str::<ProtocolProposer>(
                r#"{"kind":"verified","identity":"x","bogus":1}"#
            )
            .is_err()
        );
    }
}
