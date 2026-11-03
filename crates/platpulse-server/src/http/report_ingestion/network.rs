//! Observed Network Head references and the Network Identity checks on stamped
//! block samples.

use sqlx::{Any, Transaction};

use platpulse_core::component::ComponentStatus;
use platpulse_core::{AgentReport, ReportInventory, Rfc3339};

/// Recompute the Network Reference Head of every Network this Report touched.
///
/// A source must have been *observed* within the window on the Server clock, not
/// merely delivered: a Node whose Reports are being replayed from a backlog is
/// Stale, and a Stale Node is never a source of the Observed Network Head
/// (ADR 0014, issue #274). `now` is the receipt instant of the triggering
/// Report, which is the only instant the Server can defend when it finds no
/// eligible source at all.
pub(super) async fn update_network_references(
    tx: &mut Transaction<'_, Any>,
    network_keys: &[String],
    now: &str,
) -> Result<(), sqlx::Error> {
    // The head is recomputed from every Node of the Network, so it is a
    // read-then-write over rows other Agents' Reports write concurrently on
    // the PostgreSQL work connections (issue #280). Locking the Network's head
    // row before the read serializes the recomputations: a receipt waiting
    // here reads the Nodes the previous one committed (each statement of a
    // READ COMMITTED transaction takes a fresh snapshot), so the last head
    // written counts every source. The rows are locked in ascending key order
    // so two Reports touching the same Networks never wait on each other in
    // a cycle. On SQLite the single writer serializes this already, and the
    // no-op update costs nothing.
    let mut locked_keys = network_keys.iter().collect::<Vec<_>>();
    locked_keys.sort();
    locked_keys.dedup();
    for network_key in locked_keys {
        sqlx::query("INSERT INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES ($1, NULL, $2, 'unknown', 0, NULL) ON CONFLICT(network_key) DO UPDATE SET confidence=network_reference_heads.confidence")
            .bind(network_key)
            .bind(now)
            .execute(&mut **tx)
            .await?;
    }
    for network_key in network_keys {
        // The window is judged here on parsed instants: an unparseable evidence
        // instant is never fresh.
        let now_instant = crate::auth::parse_rfc3339(now);
        // A Node whose head was never observed is not a source: its NULL head
        // must not stand in as block 0 (the native SQLite decode once did).
        let rows = sqlx::query_as::<
            _,
            (
                String,
                i64,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            "SELECT n.node_id, c.current_block, h.resync_state, COALESCE(rpc.observation_at, rpc.received_at), COALESCE(sy.observation_at, sy.received_at), COALESCE(co.observation_at, co.received_at) FROM nodes n JOIN current_node_chain_observations c ON c.node_id=n.node_id LEFT JOIN block_history_state h ON h.node_id=n.node_id LEFT JOIN component_status sy ON sy.agent_id=n.agent_id AND sy.scope='node' AND sy.scope_key=n.node_id AND sy.component_key='sync' LEFT JOIN component_status rpc ON rpc.agent_id=n.agent_id AND rpc.scope='node' AND rpc.scope_key=n.node_id AND rpc.component_key='rpc' LEFT JOIN component_status co ON co.agent_id=n.agent_id AND co.scope='node' AND co.scope_key=n.node_id AND co.component_key='consensus' WHERE n.network_key=$1 AND n.lifecycle='active' AND c.current_block IS NOT NULL ORDER BY c.current_block DESC",
        )
        .bind(network_key)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .filter(|(_, _, _, rpc_at, sync_at, consensus_at)| {
            // A source must be a Node the Public projection would also call
            // fresh: every one of the three component observation instants
            // inside the window, in either direction. Judging sync alone let a
            // Node whose RPC and consensus were observed hours ago hold the
            // reference head (ADR 0014, issue #274).
            now_instant.is_some_and(|now| {
                [rpc_at, sync_at, consensus_at].into_iter().all(|at| {
                    crate::observation_time::is_fresh(
                        at.as_deref(),
                        now,
                        crate::observation_time::FRESHNESS_LIMIT_SECONDS,
                    )
                })
            })
        })
        .collect::<Vec<_>>();
        let eligible = rows
            .iter()
            .filter(|(_, _, state, _, _, _)| state.as_deref() != Some("resyncing"))
            .collect::<Vec<_>>();
        let candidates = if eligible.is_empty() {
            rows.iter().collect::<Vec<_>>()
        } else {
            eligible
        };
        // The head is a sync fact, so the instant it was observed is the sync
        // component's observation instant.
        let Some((node_id, head, _, _, sync_at, _)) = candidates.first() else {
            // No Node was observed recently enough to hold a reference: the row
            // records that the Server looked at its own clock `now` and found
            // none, which is why the reference is Unknown rather than stale.
            sqlx::query("INSERT INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES ($1, NULL, $2, 'unknown', 0, NULL) ON CONFLICT(network_key) DO UPDATE SET block_number=NULL, observed_at=excluded.observed_at, confidence='unknown', eligible_source_count=0, contributing_node_id=NULL")
                .bind(network_key).bind(now).execute(&mut **tx).await?;
            continue;
        };
        let confidence = if candidates.len() >= 2 { "high" } else { "low" };
        // The reference head is the head of the winning source at the instant it
        // was observed, not the instant this Report arrived.
        sqlx::query("INSERT INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT(network_key) DO UPDATE SET block_number=excluded.block_number, observed_at=excluded.observed_at, confidence=excluded.confidence, eligible_source_count=excluded.eligible_source_count, contributing_node_id=excluded.contributing_node_id")
            .bind(network_key).bind(*head).bind(sync_at.as_deref().unwrap_or(now)).bind(confidence).bind(candidates.len() as i64).bind(node_id)
            .execute(&mut **tx).await?;
    }
    Ok(())
}

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
pub(super) struct BlockIdentityRejection {
    fields: Vec<StampedIdentityField>,
    divergence: StampDivergence,
}

impl BlockIdentityRejection {
    /// Operator-facing reason: every differing field with its stamped and
    /// registered value, the side that was found stale, and the remedy for
    /// that side. Field names avoid the redaction keyword list and values are
    /// stated without a separator so the sanctioned diagnostics survive
    /// redaction intact.
    pub(super) fn reason(&self) -> String {
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
struct RegisteredNetworkIdentity {
    genesis_hash: String,
    chain_id: i64,
    p2p_network_id: i64,
    address_hrp: String,
}

impl RegisteredNetworkIdentity {
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

/// Identity of one stamped sample: the Node plus the exact Block Identity
/// Stamp its sample carries. One Report can carry more than one stamp for the
/// same Node when its samples span a subscription boundary (the wire format
/// validates each sample's stamp on its own), so every rejection is keyed by
/// the stamp it diagnosed rather than by the Node alone.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct StampedIdentityKey {
    node_id: platpulse_core::identity::NodeId,
    genesis_hash: String,
    chain_id: u64,
    p2p_network_id: u64,
    address_hrp: Option<String>,
}

impl StampedIdentityKey {
    /// The key of the stamp one sample carries. Every field is kept as it was
    /// stamped, so two different stamps can never collapse into one key: an
    /// absent HRP is not the same stamp as a Network registered under the
    /// literal HRP "unset".
    pub(super) fn new(
        node_id: platpulse_core::identity::NodeId,
        stamp: &platpulse_core::network::NetworkIdentity,
    ) -> Self {
        Self {
            node_id,
            genesis_hash: stamp.genesis_hash.to_string(),
            chain_id: stamp.chain_id,
            p2p_network_id: stamp.p2p_network_id,
            address_hrp: stamp.address_hrp.clone(),
        }
    }
}

/// Samples whose stamped Network Identity contradicts the Registry, keyed by
/// Node and stamp, each carrying the differing fields plus the diagnosed cause.
pub(super) async fn block_network_identity_rejections<I: ReportInventory>(
    tx: &mut Transaction<'_, Any>,
    report: &AgentReport<I>,
    purged_nodes: &std::collections::HashSet<platpulse_core::identity::NodeId>,
) -> Result<std::collections::HashMap<StampedIdentityKey, BlockIdentityRejection>, sqlx::Error> {
    let mut rejections = std::collections::HashMap::new();
    for sample in &report.block_summaries {
        // A purged Node has no Network projection left to compare against and
        // must never raise an identity contradiction.
        if purged_nodes.contains(&sample.node_id) {
            continue;
        }
        let Some(node) = report
            .inventory
            .nodes()
            .iter()
            .find(|node| node.node_id == sample.node_id)
        else {
            continue;
        };
        let registered = sqlx::query_as::<_, (String, i64, i64, String)>(
            "SELECT genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks WHERE network_key = $1",
        ).bind(node.network_key.as_str()).fetch_optional(&mut **tx).await?;
        let Some((genesis_hash, chain_id, p2p_network_id, address_hrp)) = registered else {
            continue;
        };
        let registered = RegisteredNetworkIdentity {
            genesis_hash,
            chain_id,
            p2p_network_id,
            address_hrp,
        };
        let fields = registered.differing_fields(&sample.network_identity);
        if fields.is_empty() {
            continue;
        }
        // Every contradicted sample gets the diagnosis of the stamp it
        // actually carries, so the Receipt and the History Gap name the fields
        // that differed for that sample (ADR 0011 decision 4). The refusal
        // itself stays Node-scoped: one contradicted stamp already condemns the
        // Node's samples, and the operator-facing remedy is per Node.
        let divergence = match fresh_network_identity(report, sample.node_id) {
            None => StampDivergence::Unobserved,
            Some(observed) if registered.matches(observed) => StampDivergence::StaleStamp,
            Some(_) => StampDivergence::NetworkConflict,
        };
        rejections
            .entry(StampedIdentityKey::new(
                sample.node_id,
                &sample.network_identity,
            ))
            .or_insert(BlockIdentityRejection { fields, divergence });
    }
    Ok(rejections)
}

/// Record the identity evidence one Block Summary adds to its Node's window,
/// and the divergence observation a contradicted stamp produces.
///
/// `observed_at` is the skew-corrected observation instant of the sample
/// ([ReportStamp::history_at], ADR 0014), never the receipt instant: this history
/// remembers when the Node's chain said something, not when its Report arrived.
pub(super) async fn observe_block_identity(
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
    use crate::http::report_ingestion::test_helpers::*;

    use crate::http::AppState;

    use platpulse_core::component::ComponentStatus;

    use platpulse_core::{AgentReport, ReceiptDisposition, SampleDispositionKind};

    /// The Registry tuple of the Network `state_with_agent` registers.
    pub(super) fn registered_identity() -> platpulse_core::network::NetworkIdentity {
        platpulse_core::network::NetworkIdentity {
            genesis_hash: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            chain_id: 210425,
            p2p_network_id: 210425,
            address_hrp: Some("lat".to_owned()),
        }
    }

    /// The identity a Node served before its build upgrade: the legacy chain
    /// and p2p ids, and a genesis hash that is not the registered one.
    pub(super) fn pre_upgrade_identity() -> platpulse_core::network::NetworkIdentity {
        platpulse_core::network::NetworkIdentity {
            genesis_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .parse()
                .unwrap(),
            chain_id: 100,
            p2p_network_id: 100,
            address_hrp: Some("lat".to_owned()),
        }
    }

    /// A v1 report whose Node observation is `observed`, carrying one sample
    /// per requested height stamped with `stamp`. The observation is the
    /// Node's own fresh identity; the stamp is what the Agent cached and
    /// replays into every sample until its Block Worker reconnects.
    pub(super) fn report_with_stamped_samples(
        report_id: &str,
        observed: &platpulse_core::network::NetworkIdentity,
        stamp: &platpulse_core::network::NetworkIdentity,
        heights: &[u64],
    ) -> AgentReport {
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report.report_id = report_id.parse().unwrap();
        let node_id = report.inventory.nodes[0].node_id;
        let observation = &mut report.nodes[0].chain.network_identity;
        observation.status = ComponentStatus::Ok;
        observation.latest = Some(observed.clone());
        observation.latest_observed_at = Some(report.generated_at);
        observation.error = None;
        for height in heights {
            report
                .block_summaries
                .push(platpulse_core::block::BlockSummary {
                    node_id,
                    network_identity: stamp.clone(),
                    block_number: *height,
                    block_hash: format!("0x{:064x}", height + 1).parse().unwrap(),
                    parent_hash: format!("0x{:064x}", height).parse().unwrap(),
                    block_timestamp_ms: 1_000,
                    observed_at: report.generated_at,
                    transaction_count: 3,
                    block_interval_ms: None,
                    source: platpulse_core::block::BlockSource::Subscription,
                    attribution:
                        platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                            "0x1111111111111111111111111111111111111111"
                                .parse()
                                .unwrap(),
                            "test",
                        ),
                });
        }
        report.validate().unwrap();
        report
    }

    pub(super) async fn recorded_gap_reasons(
        state: &AppState,
        node_id: platpulse_core::identity::NodeId,
    ) -> Vec<(i64, i64, String)> {
        sqlx::query_as(
            "SELECT from_height, to_height, reason FROM block_history_gaps WHERE node_id=$1 ORDER BY from_height, to_height",
        )
        .bind(node_id.to_string())
        .fetch_all(state.db().pool())
        .await
        .unwrap()
    }

    /// The History Gap rows a Node carries with their author, in height order.
    pub(super) async fn recorded_gap_rows(
        state: &AppState,
        node_id: platpulse_core::identity::NodeId,
    ) -> Vec<(i64, i64, String, String)> {
        sqlx::query_as(
            "SELECT from_height, to_height, reason, authored_by FROM block_history_gaps WHERE node_id=$1 ORDER BY from_height, to_height",
        )
        .bind(node_id.to_string())
        .fetch_all(state.db().pool())
        .await
        .unwrap()
    }

    /// #235 decision 4: after a Node build upgrade the Node reports the
    /// registered Network while the Agent keeps stamping samples with the
    /// identity it cached. Every rejection must name the differing fields with
    /// their stamped and registered values, point at the Agent restart as the
    /// remedy, and leave the Server's own diagnosis as the History Gap reason
    /// operators read — the Agent's later declaration of the same gap may not
    /// replace it.
    #[tokio::test]
    pub(super) async fn stale_stamp_rejection_names_the_differing_fields_and_the_agent_restart() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000301",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10, 11],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.samples.len(), 2);
        for sample in &receipt.samples {
            assert_eq!(sample.disposition, SampleDispositionKind::TerminalRejected);
            let rejection = sample.rejection.as_ref().unwrap();
            assert_eq!(
                rejection.code,
                platpulse_core::RejectionCode::NetworkIdentityMismatch
            );
            assert!(!rejection.retryable);
            // The reason names every field that actually differed, on both
            // sides, and never invents one that matched.
            assert!(
                rejection.reason.contains(
                    "genesis_hash stamped 0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa registered 0x0000000000000000000000000000000000000000000000000000000000000001"
                ),
                "{}",
                rejection.reason
            );
            assert!(
                rejection
                    .reason
                    .contains("chain_id stamped 100 registered 210425"),
                "{}",
                rejection.reason
            );
            assert!(
                rejection
                    .reason
                    .contains("p2p_network_id stamped 100 registered 210425"),
                "{}",
                rejection.reason
            );
            assert!(
                !rejection.reason.contains("address_hrp"),
                "{}",
                rejection.reason
            );
            // ...and points at the stale side plus the remedy for it.
            assert!(
                rejection
                    .reason
                    .contains("restarting the Agent on that Host is the only remedy"),
                "{}",
                rejection.reason
            );
            assert!(
                rejection
                    .reason
                    .contains("never edited to match a stamped sample"),
                "{}",
                rejection.reason
            );
        }

        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 0);

        // The Server recorded its own diagnosis for exactly the rejected
        // heights, one row per sample.
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 2);
        assert_eq!(gaps[0].0, 10);
        assert_eq!(gaps[0].1, 10);
        assert_eq!(gaps[1].0, 11);
        assert_eq!(gaps[1].1, 11);
        let reason = receipt.samples[0]
            .rejection
            .as_ref()
            .unwrap()
            .reason
            .clone();
        assert_eq!(gaps[0].2, reason);
        assert_eq!(gaps[1].2, reason);

        // The Agent reports the same gap back with its own wording, as the
        // in-process Agent does once it applies the Receipt. The unique gap
        // identity folds it into the Server's row, so the diagnosis survives.
        let mut declared = report.clone();
        declared.report_sequence += 1;
        declared.report_id = "0195f2a1-0013-4013-8013-000000000302".parse().unwrap();
        declared.block_summaries.clear();
        declared.history_gaps = [10u64, 11]
            .into_iter()
            .map(|height| platpulse_core::gap::HistoryGap {
                node_id,
                kind: platpulse_core::gap::GapKind::ServerRejected,
                from_height: height,
                to_height: height,
                reason: "locally retained history gap".to_owned(),
                recorded_at: report.generated_at,
            })
            .collect();
        declared.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert_eq!(recorded_gap_reasons(&state, node_id).await, gaps);
    }

    /// One Report can carry more than one Block Identity Stamp for the same
    /// Node: samples spooled across a subscription boundary keep the stamp
    /// they were collected with, and the wire format validates each sample's
    /// stamp on its own. Every rejection must name the fields that actually
    /// differed for *its* sample (ADR 0011 decision 4), and the History Gap
    /// the Server leaves behind must carry that same text.
    #[tokio::test]
    pub(super) async fn each_stamped_sample_is_diagnosed_on_its_own_stamp() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000305",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let mut hrp_stamp = registered_identity();
        hrp_stamp.address_hrp = Some("lax".to_owned());
        let mut second = report.block_summaries[0].clone();
        second.block_number = 11;
        second.block_hash = format!("0x{:064x}", 12).parse().unwrap();
        second.parent_hash = format!("0x{:064x}", 11).parse().unwrap();
        second.network_identity = hrp_stamp;
        report.block_summaries.push(second);
        report.validate().unwrap();

        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.samples.len(), 2);
        let rejection_at = |height: u64| {
            receipt
                .samples
                .iter()
                .find(|sample| {
                    sample.sample == platpulse_core::receipt::SampleRef::Block { height }
                })
                .and_then(|sample| sample.rejection.clone())
                .unwrap()
        };

        let legacy = rejection_at(10);
        assert_eq!(
            legacy.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            legacy.reason.contains("genesis_hash stamped 0xaaaa")
                && legacy
                    .reason
                    .contains("chain_id stamped 100 registered 210425")
                && !legacy.reason.contains("address_hrp"),
            "{}",
            legacy.reason
        );

        let hrp = rejection_at(11);
        assert_eq!(
            hrp.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            hrp.reason
                .contains("address_hrp stamped lax registered lat")
                && !hrp.reason.contains("chain_id")
                && !hrp.reason.contains("genesis_hash"),
            "{}",
            hrp.reason
        );

        // Each refused height carries its own sample's diagnosis instead of the
        // first sample's fields repeated for the whole Node.
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 2);
        assert_eq!((gaps[0].0, gaps[0].1), (10, 10));
        assert_eq!(gaps[0].2, legacy.reason);
        assert_eq!((gaps[1].0, gaps[1].1), (11, 11));
        assert_eq!(gaps[1].2, hrp.reason);
    }

    /// The diagnosis key is built from the stamp's own fields, so a stamp that
    /// carries no HRP can never share the key of a stamp whose HRP reads
    /// "unset" - the literal is a legal Registry value, and collapsing the two
    /// would reject a matching sample on a sibling sample's contradiction.
    #[test]
    pub(super) fn an_absent_hrp_is_not_the_stamp_registered_as_unset() {
        let node_id: platpulse_core::identity::NodeId =
            "0195f2a1-0013-4013-8013-000000000307".parse().unwrap();
        let mut absent = registered_identity();
        absent.address_hrp = None;
        let mut literal = registered_identity();
        literal.address_hrp = Some("unset".to_owned());

        assert_ne!(
            StampedIdentityKey::new(node_id, &absent),
            StampedIdentityKey::new(node_id, &literal)
        );
        // Equal stamps still share a key: the same contradicted stamp must not
        // be diagnosed twice.
        assert_eq!(
            StampedIdentityKey::new(node_id, &literal),
            StampedIdentityKey::new(node_id, &literal.clone())
        );
    }

    /// A Network registered under the literal HRP "unset" is a legal Registry
    /// tuple, and a sample whose stamp matches it must be accepted even when a
    /// sibling sample of the same Report carries no HRP at all. Before the
    /// diagnosis was keyed by an explicit field list both stamps produced the
    /// same key, so the matching summary was refused (and a permanent History
    /// Gap invented for it) while the data plane stored it anyway.
    #[tokio::test]
    pub(super) async fn samples_are_diagnosed_by_their_own_stamp_not_by_a_sibling_ones() {
        let (_dir, state, agent_id) = state_with_agent().await;
        sqlx::query("UPDATE networks SET address_hrp = 'unset'")
            .execute(state.db().pool())
            .await
            .unwrap();

        let mut registered = registered_identity();
        registered.address_hrp = Some("unset".to_owned());
        let mut absent = registered.clone();
        absent.address_hrp = None;
        let mut report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000307",
            &registered,
            &absent,
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let mut matching = report.block_summaries[0].clone();
        matching.block_number = 11;
        matching.block_hash = format!("0x{:064x}", 12).parse().unwrap();
        matching.parent_hash = format!("0x{:064x}", 11).parse().unwrap();
        matching.network_identity = registered.clone();
        report.block_summaries.push(matching);
        report.validate().unwrap();

        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        // Both samples must be reported on by the Receipt: a missing entry
        // would let the "no rejection" assertion below pass for the wrong
        // reason, so each height is looked up by presence, not by absence.
        let sample_at = |height: u64| {
            receipt
                .samples
                .iter()
                .find(|sample| {
                    sample.sample == platpulse_core::receipt::SampleRef::Block { height }
                })
                .unwrap_or_else(|| panic!("the Receipt reports the sample for height {height}"))
                .clone()
        };
        assert_eq!(receipt.samples.len(), 2);

        let refused = sample_at(10);
        assert_eq!(refused.disposition, SampleDispositionKind::TerminalRejected);
        let absent = refused
            .rejection
            .expect("the HRP-less stamp is contradicted");
        assert_eq!(
            absent.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        // The refusal names the missing HRP without pretending it was this
        // Network's registered value.
        assert!(
            absent
                .reason
                .contains("address_hrp stamped not reported registered unset"),
            "{}",
            absent.reason
        );
        // The sibling stamp that genuinely matches the Registry is accepted on
        // its own merits, not condemned through a key it shares with a
        // normalized spelling of the absent HRP.
        let matched = sample_at(11);
        assert_eq!(matched.disposition, SampleDispositionKind::Accepted);
        assert!(
            matched.rejection.is_none(),
            "the matching stamp is accepted, not judged by its sibling"
        );

        let accepted: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id = $1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(accepted, 1);
        // The data plane, the Receipt and the Server's own gap row agree: the
        // accepted summary is counted once and only the contradicted height is
        // refused.
        let counted: i64 = sqlx::query_scalar(
            "SELECT cumulative_block_count FROM block_history_state WHERE node_id = $1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(counted, 1);
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!((gaps[0].0, gaps[0].1), (10, 10));
        assert_eq!(gaps[0].2, absent.reason);
    }

    /// A contradicted stamp is the more specific finding about a sample, so it
    /// outranks the generic refusals in the Receipt: a GapBackfill sample
    /// outside an open recoverable gap reports the identity contradiction the
    /// Server records on its History Gap, instead of two surfaces naming two
    /// different causes. A sample whose stamp matches the Registry keeps the
    /// backfill-protocol rejection it always had.
    #[tokio::test]
    pub(super) async fn identity_refusal_agrees_with_the_gap_it_records() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut mismatched = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000306",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = mismatched.inventory.nodes[0].node_id;
        mismatched.block_summaries[0].source = platpulse_core::block::BlockSource::GapBackfill;
        mismatched.validate().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&mismatched).unwrap()).await;
        let rejection = receipt.samples[0].rejection.as_ref().unwrap();
        assert_eq!(
            rejection.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            rejection.reason.contains("genesis_hash stamped 0xaaaa"),
            "{}",
            rejection.reason
        );
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].2, rejection.reason);

        // The same sample shape with a matching stamp is new history above the
        // High-Water Mark, which a backfill may supply (issue #281), and the
        // Server records no identity gap for it.
        let mut matching = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000307",
            &registered_identity(),
            &registered_identity(),
            &[20],
        );
        matching.report_sequence = mismatched.report_sequence + 1;
        matching.block_summaries[0].source = platpulse_core::block::BlockSource::GapBackfill;
        matching.validate().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&matching).unwrap()).await;
        assert!(receipt.samples[0].rejection.is_none());
        assert_eq!(recorded_gap_reasons(&state, node_id).await, gaps);
    }

    /// History Gap rows carry their author: the Server writes its own refusals
    /// as 'server', and a gap an Agent declares about itself stays 'agent'
    /// evidence. Only the Server's own rows may be presented as its live
    /// refusal diagnosis, so a reporting Agent cannot fabricate one
    /// (AGENTS.md: the Server is the trust boundary).
    #[tokio::test]
    pub(super) async fn history_gaps_carry_their_author() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000308",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let server_reason = receipt.samples[0]
            .rejection
            .as_ref()
            .unwrap()
            .reason
            .clone();
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![(10, 10, server_reason.clone(), "server".to_owned())]
        );

        // The Agent applies the Receipt and reports the same gap back with its
        // own wording, plus a gap of its own that the Server never refused. The
        // Server's row keeps its text and its author, and the Agent's row is
        // stored as the Agent's.
        let mut declared = report.clone();
        declared.report_sequence += 1;
        declared.report_id = "0195f2a1-0013-4013-8013-000000000309".parse().unwrap();
        declared.block_summaries.clear();
        declared.history_gaps = [10u64, 12]
            .into_iter()
            .map(|height| platpulse_core::gap::HistoryGap {
                node_id,
                kind: platpulse_core::gap::GapKind::ServerRejected,
                from_height: height,
                to_height: height,
                reason: "locally retained history gap".to_owned(),
                recorded_at: report.generated_at,
            })
            .collect();
        declared.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![
                (10, 10, server_reason, "server".to_owned()),
                (
                    12,
                    12,
                    "locally retained history gap".to_owned(),
                    "agent".to_owned()
                ),
            ]
        );
    }

    /// A refusal is the Server's own finding, so it must correct a row the
    /// Agent declared first for the same height: the Agent's insert keeps its
    /// hands off an existing row, and the Server's upsert owns the row it
    /// refused. Otherwise a reporting Agent could dictate what the Admin
    /// diagnosis says about a refusal the Server made (AGENTS.md: the Server
    /// is the trust boundary).
    #[tokio::test]
    pub(super) async fn a_server_refusal_corrects_the_gap_the_agent_declared_first() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut declared = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000310",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[],
        );
        let node_id = declared.inventory.nodes[0].node_id;
        declared.history_gaps = std::iter::once(platpulse_core::gap::HistoryGap {
            node_id,
            kind: platpulse_core::gap::GapKind::ServerRejected,
            from_height: 10,
            to_height: 10,
            reason: "locally retained history gap".to_owned(),
            recorded_at: declared.generated_at,
        })
        .collect();
        declared.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![(
                10,
                10,
                "locally retained history gap".to_owned(),
                "agent".to_owned()
            )]
        );

        let mut refused = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000311",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        refused.report_sequence = declared.report_sequence + 1;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&refused).unwrap()).await;
        let server_reason = receipt.samples[0]
            .rejection
            .as_ref()
            .unwrap()
            .reason
            .clone();
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![(10, 10, server_reason, "server".to_owned())]
        );
        // The Server also stamps the row with its own clock instead of keeping
        // the Agent's claim about when the refusal happened. The Receipt's
        // `server_time` and the row's `created_at` are both canonical
        // second-precision UTC text, and the row's text is minted first, so it
        // can only sort at or before the Receipt's time — never at the Agent's
        // past-dated `recorded_at`.
        let (created_at, authored_by): (String, String) = sqlx::query_as(
            "SELECT created_at, authored_by FROM block_history_gaps WHERE node_id = $1 AND from_height = 10 AND to_height = 10",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(authored_by, "server");
        assert_ne!(
            created_at,
            declared.generated_at.to_string(),
            "the Agent's own timestamp must not survive the Server's refusal"
        );
        assert!(created_at.as_str() <= receipt.server_time.to_string().as_str());
    }

    /// Acceptance evidence for the Admin refusal diagnosis outlives raw Block
    /// Summary retention: the Server stamps the moment it last accepted a
    /// summary on the durable per-Node history state (ADR 0011 decision 4).
    #[tokio::test]
    pub(super) async fn accepted_history_records_when_it_resumed() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000310",
            &registered_identity(),
            &registered_identity(),
            &[20, 21],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);

        let accepted: String = sqlx::query_scalar(
            "SELECT accepted_at FROM block_summaries WHERE node_id=$1 ORDER BY block_number DESC LIMIT 1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let recorded: Option<String> = sqlx::query_scalar(
            "SELECT last_accepted_summary_at FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            recorded,
            Some(accepted),
            "the acceptance stamp is the same Server-authored text as the summary it proves"
        );
    }

    /// A Node that really serves another Network is not a stale stamp: no
    /// Agent restart can reconcile it, and the reason must say so instead of
    /// sending an operator through the restart runbook.
    #[tokio::test]
    pub(super) async fn cross_network_conflict_is_not_reported_as_a_stale_stamp() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000303",
            &pre_upgrade_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        let rejection = receipt.samples[0].rejection.as_ref().unwrap();
        assert_eq!(
            rejection.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            rejection.reason.contains(
                "genuine cross-Network conflict that restarting the Agent cannot reconcile"
            ),
            "{}",
            rejection.reason
        );
        assert!(
            !rejection.reason.contains("is the only remedy"),
            "{}",
            rejection.reason
        );
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].2, rejection.reason);
    }

    /// Without a complete successful identity observation the Server cannot
    /// tell a stale stamp from a cross-Network conflict, and the reason must
    /// say that rather than guess.
    #[tokio::test]
    pub(super) async fn unobserved_identity_is_not_reported_as_a_stale_stamp() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000304",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        report.nodes[0].chain.network_identity.status = ComponentStatus::Error;
        report.nodes[0].chain.network_identity.latest = None;
        report.nodes[0].chain.network_identity.latest_observed_at = None;
        report.nodes[0].chain.network_identity.error =
            Some(platpulse_core::component::BoundedError {
                code: "rpc_unreachable".into(),
                message: "identity probe failed".into(),
            });
        report.validate().unwrap();
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        let rejection = receipt.samples[0].rejection.as_ref().unwrap();
        assert_eq!(
            rejection.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            rejection
                .reason
                .contains("no complete successful Network Identity observation"),
            "{}",
            rejection.reason
        );
        assert!(
            rejection.reason.contains(
                "Confirm the Node's current identity and then restart the Agent on that Host"
            ),
            "{}",
            rejection.reason
        );
        assert!(
            !rejection.reason.contains("is the only remedy"),
            "{}",
            rejection.reason
        );
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].2, rejection.reason);
    }

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
