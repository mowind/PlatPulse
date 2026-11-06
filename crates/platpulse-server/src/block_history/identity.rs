//! Network Identity evidence on stamped Block Summaries: the diagnosis of a
//! stamp that contradicts the Registry, and the per-height block hash window
//! that detects chain divergence.

use sqlx::{Any, Transaction};

use platpulse_core::component::ComponentStatus;
use platpulse_core::{AgentReport, ReportInventory, Rfc3339};

/// One Network Identity field whose stamped value contradicted the Registry
/// tuple. Both sides are kept so a rejection can name what actually differed
/// instead of only asserting that something did (ADR 0011 decision 4).
#[derive(Debug, Clone, PartialEq, Eq)]
struct StampedIdentityField {
    field: &'static str,
    stamped: String,
    registered: String,
}

/// Why a Node's stamped samples contradicted the Registry, as far as the
/// Server can tell from the fresh component observation carried by the same
/// Report. The mismatch stays terminal and unrecoverable in every case; this
/// only chooses the remedy the rejection points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StampDivergence {
    /// The Node's current Network Identity observation matches the Registry
    /// while its stamped samples do not: the Agent stamps samples with an
    /// identity it cached before the Node's build changed (issue #235).
    StaleStamp,
    /// The current observation contradicts the Registry too: the Node serves a
    /// different Network, and no Agent restart can reconcile that.
    NetworkConflict,
    /// The Report carried no complete successful identity observation, so a
    /// stale stamp cannot yet be told apart from a cross-Network conflict.
    Unobserved,
}

/// A Node's rejected samples with the diagnosed cause, used for both the
/// Receipt rejection and the History Gap the Server records for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockIdentityRejection {
    fields: Vec<StampedIdentityField>,
    divergence: StampDivergence,
}

impl BlockIdentityRejection {
    /// Operator-facing reason: every differing field with its stamped and
    /// registered value, the side that was found stale, and the remedy for
    /// that side. Field names avoid the redaction keyword list and values are
    /// stated without a separator so the sanctioned diagnostics survive
    /// redaction intact.
    pub(crate) fn reason(&self) -> String {
        let differences = self
            .fields
            .iter()
            .map(|field| {
                format!(
                    "{} stamped {} registered {}",
                    field.field, field.stamped, field.registered
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let (diagnosis, disposition) = match self.divergence {
            StampDivergence::StaleStamp => (
                "the Node's current Network Identity observation matches the registered Network, \
                 so this Agent stamps samples with an identity it cached before the Node's build \
                 changed and restarting the Agent on that Host is the only remedy",
                "The rejected heights stay a History Gap and are never backfilled; the Registry tuple is never edited to match a stamped sample",
            ),
            StampDivergence::NetworkConflict => (
                "the Node's current Network Identity observation contradicts the registered Network \
                 as well, so this is a genuine cross-Network conflict that restarting the Agent \
                 cannot reconcile",
                "The rejected heights are never merged into block history and are never backfilled; the Registry tuple is never edited to match a stamped sample",
            ),
            StampDivergence::Unobserved => (
                "this Report carried no complete successful Network Identity observation, so a \
                 stale stamp cannot yet be told apart from a genuine cross-Network conflict",
                "Confirm the Node's current identity and then restart the Agent on that Host; the rejected heights stay a History Gap and are never backfilled",
            ),
        };
        format!(
            "Block network identity does not match the registered Network ({differences}). Because {diagnosis}. {disposition}."
        )
    }
}

/// The Registry identity tuple a Node's Network is registered under.
#[derive(Debug, sqlx::FromRow)]
pub(super) struct RegisteredNetworkIdentity {
    genesis_hash: String,
    chain_id: i64,
    p2p_network_id: i64,
    address_hrp: String,
}

impl RegisteredNetworkIdentity {
    /// The tuple of the Network registered under `network_key`.
    pub(super) async fn load(
        tx: &mut Transaction<'_, Any>,
        network_key: &str,
    ) -> Result<Self, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            "SELECT genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks WHERE network_key = $1",
        )
        .bind(network_key)
        .fetch_one(&mut **tx)
        .await
    }

    /// The diagnosis of a sample whose stamp contradicts this tuple, or
    /// `None` when the stamp matches it. Every sample is judged on the stamp it
    /// actually carries, so the Receipt and the History Gap name the fields that
    /// differed for that sample (ADR 0011 decision 4); the remedy is chosen from
    /// the Node's own identity observation in the same Report.
    pub(super) fn contradiction<I: ReportInventory>(
        &self,
        report: &AgentReport<I>,
        sample: &platpulse_core::block::BlockSummary,
    ) -> Option<BlockIdentityRejection> {
        let fields = self.differing_fields(&sample.network_identity);
        if fields.is_empty() {
            return None;
        }
        let divergence = match fresh_network_identity(report, sample.node_id) {
            None => StampDivergence::Unobserved,
            Some(observed) if self.matches(observed) => StampDivergence::StaleStamp,
            Some(_) => StampDivergence::NetworkConflict,
        };
        Some(BlockIdentityRejection { fields, divergence })
    }

    fn matches(&self, observed: &platpulse_core::network::NetworkIdentity) -> bool {
        observed.genesis_hash.to_string() == self.genesis_hash
            && observed.chain_id == self.chain_id as u64
            && observed.p2p_network_id == self.p2p_network_id as u64
            && observed.address_hrp.as_deref().unwrap_or("") == self.address_hrp
    }

    fn differing_fields(
        &self,
        stamped: &platpulse_core::network::NetworkIdentity,
    ) -> Vec<StampedIdentityField> {
        let mut fields = Vec::new();
        let genesis_hash = stamped.genesis_hash.to_string();
        if genesis_hash != self.genesis_hash {
            fields.push(StampedIdentityField {
                field: "genesis_hash",
                stamped: genesis_hash,
                registered: self.genesis_hash.clone(),
            });
        }
        if stamped.chain_id != self.chain_id as u64 {
            fields.push(StampedIdentityField {
                field: "chain_id",
                stamped: stamped.chain_id.to_string(),
                registered: self.chain_id.to_string(),
            });
        }
        if stamped.p2p_network_id != self.p2p_network_id as u64 {
            fields.push(StampedIdentityField {
                field: "p2p_network_id",
                stamped: stamped.p2p_network_id.to_string(),
                registered: self.p2p_network_id.to_string(),
            });
        }
        if stamped.address_hrp.as_deref().unwrap_or("") != self.address_hrp {
            fields.push(StampedIdentityField {
                field: "address_hrp",
                // A sample's stamp need not carry an HRP. Say so in words
                // that can never be a registered value: a Network's address
                // HRP must not contain whitespace (crates/platpulse-server/src/network.rs),
                // so "not reported" stays unambiguous next to a registered
                // HRP that literally reads "unset".
                stamped: stamped
                    .address_hrp
                    .clone()
                    .unwrap_or_else(|| "not reported".to_owned()),
                registered: self.address_hrp.clone(),
            });
        }
        fields
    }
}

/// The Node's own Network Identity as observed in this very Report. This is
/// what lets the Server separate an Agent's stale stamp from a Node that
/// really serves another Network (ADR 0011 decision 4); a Node whose identity
/// was not observed completely yields nothing rather than a guess.
fn fresh_network_identity<I: ReportInventory>(
    report: &AgentReport<I>,
    node_id: platpulse_core::identity::NodeId,
) -> Option<&platpulse_core::network::NetworkIdentity> {
    report
        .nodes
        .iter()
        .find(|observation| observation.node_id == node_id)
        .filter(|observation| observation.chain.network_identity.status == ComponentStatus::Ok)
        .and_then(|observation| observation.chain.network_identity.latest.as_ref())
        .filter(|identity| identity.address_hrp.is_some())
}

/// Record the identity evidence one Block Summary adds to its Node's window,
/// and the divergence observation a contradicted stamp produces.
///
/// `observed_at` is the skew-corrected observation instant of the sample
/// ([ReportStamp::history_at], ADR 0014), never the receipt instant: this history
/// remembers when the Node's chain said something, not when its Report arrived.
pub(crate) async fn observe_block_identity(
    tx: &mut Transaction<'_, Any>,
    sample: &platpulse_core::block::BlockSummary,
    observed_at: Rfc3339,
    received_at: &str,
) -> Result<bool, sqlx::Error> {
    const MAX_WINDOW_HEIGHTS: i64 = 2_048;
    let node_id = sample.node_id.to_string();
    // Identity evidence has its own retention and bounded height count. It is
    // deliberately independent from raw Block Summary retention.
    sqlx::query("DELETE FROM block_identity_window WHERE node_id=$1 AND retained_until IS NOT NULL AND retained_until < $2")
        .bind(&node_id)
        .bind(received_at)
        .execute(&mut **tx)
        .await?;
    let existing = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT block_hash, retained_until FROM block_identity_window WHERE node_id=$1 AND height=$2",
    )
    .bind(&node_id)
    .bind(sample.block_number as i64)
    .fetch_optional(&mut **tx)
    .await?;
    let retention_until = crate::auth::parse_rfc3339(received_at)
        .and_then(|value| value.checked_add(time::Duration::days(180)))
        .map(crate::auth::format_rfc3339);
    let observed_hash = sample.block_hash.to_string();
    if let Some((retained_hash, _)) = existing {
        if retained_hash != observed_hash {
            sqlx::query("INSERT INTO chain_divergence_observations (node_id, height, retained_block_hash, observed_block_hash, observed_at, reason, retained_observed_at) SELECT $1, $2, block_hash, $3, $4, 'chain_divergence', observed_at FROM block_identity_window WHERE node_id=$5 AND height=$6 ON CONFLICT DO NOTHING")
                .bind(&node_id)
                .bind(sample.block_number as i64)
                .bind(&observed_hash)
                .bind(observed_at.to_string())
                .bind(&node_id)
                .bind(sample.block_number as i64)
                .execute(&mut **tx)
                .await?;
            return Ok(true);
        }
        sqlx::query("UPDATE block_identity_window SET observed_at=$1, retained_until=$2 WHERE node_id=$3 AND height=$4")
            .bind(observed_at.to_string())
            .bind(retention_until)
            .bind(&node_id)
            .bind(sample.block_number as i64)
            .execute(&mut **tx)
            .await?;
        return Ok(false);
    }
    sqlx::query("INSERT INTO block_identity_window (node_id, height, block_hash, retained_until, observed_at) VALUES ($1, $2, $3, $4, $5)")
        .bind(&node_id)
        .bind(sample.block_number as i64)
        .bind(&observed_hash)
        .bind(retention_until)
        .bind(observed_at.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM block_identity_window WHERE node_id=$1 AND height NOT IN (SELECT height FROM block_identity_window WHERE node_id=$2 ORDER BY height DESC LIMIT $3)")
        .bind(&node_id)
        .bind(&node_id)
        .bind(MAX_WINDOW_HEIGHTS)
        .execute(&mut **tx)
        .await?;
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widest reason this can render is four differing fields with 32-byte
    /// hashes on both sides. It must stay inside the bounded diagnostic budget
    /// the Receipt contract validates against, in every classification and
    /// after redaction.
    #[test]
    pub(super) fn identity_rejection_reason_stays_inside_the_diagnostic_budget() {
        let hex = |lead: char| format!("0x{}", lead.to_string().repeat(64));
        let field =
            |name: &'static str, stamped: String, registered: String| StampedIdentityField {
                field: name,
                stamped,
                registered,
            };
        for divergence in [
            StampDivergence::StaleStamp,
            StampDivergence::NetworkConflict,
            StampDivergence::Unobserved,
        ] {
            let rejection = BlockIdentityRejection {
                fields: vec![
                    field("genesis_hash", hex('a'), hex('b')),
                    field(
                        "chain_id",
                        "18446744073709551615".to_owned(),
                        "210425".to_owned(),
                    ),
                    field(
                        "p2p_network_id",
                        "18446744073709551615".to_owned(),
                        "210425".to_owned(),
                    ),
                    field("address_hrp", hex('c'), hex('d')),
                ],
                divergence,
            };
            let reason = crate::redaction::redact_sensitive(&rejection.reason());
            assert!(
                reason.len() <= platpulse_core::protocol::MAX_ERROR_MESSAGE_BYTES,
                "{:?}: {} bytes",
                divergence,
                reason.len()
            );
        }
    }
}
