//! Block Summary admission: every sample of a Report is judged once, in Report
//! order, and written the moment it is admitted, so the Receipt and the stored
//! history are the same decision.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use sqlx::{Any, Transaction};

use platpulse_core::block::{BlockSource, BlockSummary, SealSignerMatch};
use platpulse_core::gap::GapKind;
use platpulse_core::identity::NodeId;
use platpulse_core::{AgentReport, Rejection, RejectionCode, ReportInventory, Rfc3339};

use super::identity::{BlockIdentityRejection, RegisteredNetworkIdentity, observe_block_identity};
use super::{
    GapSettlement, TouchedRange, backfill_admitted, close_coverage_height, mark_covered_height,
    open_gap_containing, open_uncovered_gap, settle_touched_history_gaps,
};

/// Why the Server refused a sample, or every sample of a Node, outright.
///
/// The Receipt rejection and the reason of the History Gap the Server records
/// for a refused height are both read from this one value, so they cannot
/// disagree (ADR 0011 decision 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The Node ID carries the durable purge boundary (design §15.3).
    Purged,
    /// The Node belongs to another Agent.
    OwnershipMismatch,
    /// The sample's Block Identity Stamp contradicts the Registry tuple.
    IdentityMismatch(BlockIdentityRejection),
}

impl Refusal {
    fn code(&self) -> RejectionCode {
        match self {
            Self::Purged => RejectionCode::NodePurged,
            Self::OwnershipMismatch => RejectionCode::NodeOwnershipMismatch,
            Self::IdentityMismatch(_) => RejectionCode::NetworkIdentityMismatch,
        }
    }

    /// The operator-facing reason, already redacted.
    pub(crate) fn reason(&self) -> String {
        let reason = match self {
            Self::Purged => "Node was permanently purged by the Owner".to_owned(),
            Self::OwnershipMismatch => "Node belongs to another Agent".to_owned(),
            Self::IdentityMismatch(evidence) => evidence.reason(),
        };
        redacted(&reason)
    }

    /// The Receipt rejection for this refusal.
    pub(crate) fn rejection(&self) -> Rejection {
        let code = self.code();
        Rejection {
            code,
            retryable: code.is_retryable(),
            reason: self.reason(),
        }
    }

    /// A purge erased the Node's history, so the Server records no History
    /// Gap for it; every other refusal leaves the Server's own gap behind.
    fn records_gap(&self) -> bool {
        !matches!(self, Self::Purged)
    }
}

fn redacted(reason: &str) -> String {
    crate::redaction::redact_sensitive(reason)
}

fn rejection(code: RejectionCode, reason: &str) -> Rejection {
    Rejection {
        code,
        retryable: code.is_retryable(),
        reason: redacted(reason),
    }
}

/// The Report fence's verdict on each declared Node: a Node with no refusal is
/// admitted, and its samples are judged one by one.
#[derive(Debug, Default)]
pub(crate) struct NodeAdmission {
    refused: HashMap<NodeId, Refusal>,
}

impl NodeAdmission {
    pub(crate) fn refuse_purged(&mut self, node_id: NodeId) {
        self.refused.insert(node_id, Refusal::Purged);
    }

    pub(crate) fn refuse_ownership(&mut self, node_id: NodeId) {
        self.refused
            .entry(node_id)
            .or_insert(Refusal::OwnershipMismatch);
    }

    pub(crate) fn refusal(&self, node_id: NodeId) -> Option<&Refusal> {
        self.refused.get(&node_id)
    }

    pub(crate) fn admits(&self, node_id: NodeId) -> bool {
        !self.refused.contains_key(&node_id)
    }
}

/// What admission did with one Block Summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockOutcome {
    /// Written: the summary, its coverage, the high-water mark, the cumulative
    /// counters and the block tiers, plus the Observed Network Head for a live
    /// subscription sample.
    Accepted,
    /// The height is already recorded; nothing was written or counted again.
    AlreadyRecorded,
    Refused(Refusal),
    /// A plain resync replay at or below the historical high-water mark.
    ResyncReplay,
    /// A GapBackfill sample for a height that is not a recoverable hole.
    OutsideOpenGap,
    /// The block hash contradicts the identity evidence retained for the
    /// height. It is kept as a Chain Divergence Observation and never replaces
    /// the recorded summary.
    ChainDivergence,
}

impl BlockOutcome {
    /// The Receipt rejection, or `None` for a sample whose height is recorded.
    pub(crate) fn rejection(&self) -> Option<Rejection> {
        match self {
            Self::Accepted | Self::AlreadyRecorded => None,
            Self::Refused(refusal) => Some(refusal.rejection()),
            Self::ResyncReplay => Some(rejection(
                RejectionCode::ResyncReplay,
                "Normal resync replay at or below the historical high-water mark",
            )),
            Self::OutsideOpenGap => Some(rejection(
                RejectionCode::GapBackfillOutsideOpenGap,
                "GapBackfill sample is not a recoverable hole: the height is already covered, confirmed missing, Server-rejected, or below the Node's first covered height",
            )),
            Self::ChainDivergence => Some(rejection(
                RejectionCode::ChainDivergence,
                "Block hash diverges from retained Node identity evidence",
            )),
        }
    }
}

/// What admission did with one Agent-declared History Gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GapOutcome {
    Accepted,
    Refused(Refusal),
}

impl GapOutcome {
    pub(crate) fn rejection(&self) -> Option<Rejection> {
        match self {
            Self::Accepted => None,
            Self::Refused(refusal) => Some(refusal.rejection()),
        }
    }
}

/// The result of admitting one Report's block history.
#[derive(Debug)]
pub(crate) struct BlockAdmission {
    /// One outcome per `report.block_summaries` entry, in the same order.
    pub(crate) samples: Vec<BlockOutcome>,
    /// One outcome per `report.history_gaps` entry, in the same order.
    pub(crate) gaps: Vec<GapOutcome>,
    /// A stamp contradicted the Registry: a security event for the Agent.
    pub(crate) identity_contradicted: bool,
    /// The History Gap settlement pass and how long it took, or `None` when
    /// the Report touched no gap.
    pub(crate) settlement: Option<(GapSettlement, std::time::Duration)>,
}

/// What admission keeps about one Node for the rest of the Report. Ingestion
/// holds the fence of every Node the Report names for the whole transaction,
/// on either Storage Backend, and retention never reads or writes the
/// high-water mark, so the value read once stays true as long as admission
/// advances it with every summary it writes. Coverage intervals are not kept:
/// compaction merges them in transactions of its own.
struct NodeHistory {
    registered: RegisteredNetworkIdentity,
    high_watermark: Option<i64>,
}

impl NodeHistory {
    async fn load<I: ReportInventory>(
        tx: &mut Transaction<'_, Any>,
        report: &AgentReport<I>,
        node_id: NodeId,
    ) -> Result<Self, sqlx::Error> {
        // Report validation guarantees every sample's Node is declared, and the
        // fence refuses a Report whose Network key is not registered, so both
        // rows exist for an admitted Node.
        let network_key = report
            .inventory
            .nodes()
            .iter()
            .find(|node| node.node_id == node_id)
            .map(|node| node.network_key.as_str())
            .ok_or(sqlx::Error::RowNotFound)?;
        let registered = RegisteredNetworkIdentity::load(tx, network_key).await?;
        let high_watermark = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT historical_high_watermark FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_optional(&mut **tx)
        .await?
        .flatten();
        Ok(Self {
            registered,
            high_watermark,
        })
    }
}

/// Admit the block history one Report carries.
///
/// Samples are judged in Report order and each admitted one is written before
/// the next is judged, so a later sample sees what an earlier one recorded.
/// Then the Server records its own History Gap for every refused height, the
/// Agent-declared gaps of admitted Nodes are stored, and the gaps around every
/// height this Report took out of the open state are settled.
///
/// `received_at` is the Report's receipt instant; `history_at` places a
/// sample's Agent-clock observation instant on the Server clock (ADR 0014).
pub(crate) async fn admit<I: ReportInventory>(
    tx: &mut Transaction<'_, Any>,
    report: &AgentReport<I>,
    nodes: &NodeAdmission,
    received_at: &str,
    history_at: impl Fn(Rfc3339) -> Rfc3339,
) -> Result<BlockAdmission, sqlx::Error> {
    let mut histories: HashMap<NodeId, NodeHistory> = HashMap::new();
    let mut touched = Vec::new();
    let mut samples = Vec::with_capacity(report.block_summaries.len());
    let mut identity_contradicted = false;
    for sample in &report.block_summaries {
        let refusal = nodes.refusal(sample.node_id);
        // A purged Node has no Network projection left to compare against and
        // never raises an identity contradiction.
        if matches!(refusal, Some(Refusal::Purged)) {
            samples.push(BlockOutcome::Refused(Refusal::Purged));
            continue;
        }
        let history = match histories.entry(sample.node_id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                entry.insert(NodeHistory::load(tx, report, sample.node_id).await?)
            }
        };
        // A contradicted stamp is a security event for the reporting Agent
        // even when the Node is refused for its owner, and it never touches
        // the identity window: mismatched samples can never merge into history.
        let contradiction = history.registered.contradiction(report, sample);
        identity_contradicted |= contradiction.is_some();
        let outcome = match (refusal, contradiction) {
            (Some(refusal), _) => BlockOutcome::Refused(refusal.clone()),
            (None, Some(evidence)) => BlockOutcome::Refused(Refusal::IdentityMismatch(evidence)),
            (None, None) => {
                admit_sample(
                    tx,
                    sample,
                    history,
                    received_at,
                    history_at(sample.observed_at),
                    &mut touched,
                )
                .await?
            }
        };
        samples.push(outcome);
    }

    for (sample, outcome) in report.block_summaries.iter().zip(&samples) {
        let BlockOutcome::Refused(refusal) = outcome else {
            continue;
        };
        if !refusal.records_gap() {
            continue;
        }
        // The Server records the History Gap for a refused sample itself, so
        // the reason operators read is the Server's own diagnosis of what it
        // refused and why, not an Agent claim about itself.
        //
        // The row is upserted rather than ignored so the Server's text wins
        // even when the Agent recorded the same (node, from, to, kind) triple
        // first: the unique gap index folds the Agent's row into this one, and
        // authored_by is what lets the Admin surface present only the rows the
        // Server wrote as its own live refusal (AGENTS.md: the Server is the
        // trust boundary and never presents Agent input as its own evidence).
        sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, $2, $3, 'server_rejected', $4, $5, 'server') ON CONFLICT(node_id, from_height, to_height, kind) DO UPDATE SET reason=excluded.reason, created_at=excluded.created_at, authored_by=excluded.authored_by")
            .bind(sample.node_id.to_string())
            .bind(sample.block_number as i64)
            .bind(sample.block_number as i64)
            .bind(refusal.reason())
            .bind(received_at)
            .execute(&mut **tx)
            .await?;
    }

    let mut gaps = Vec::with_capacity(report.history_gaps.len());
    for gap in &report.history_gaps {
        if let Some(refusal) = nodes.refusal(gap.node_id) {
            gaps.push(GapOutcome::Refused(refusal.clone()));
            continue;
        }
        let kind = match gap.kind {
            GapKind::UnrecoverableBackfill => "unrecoverable_backfill",
            GapKind::SpoolOverflow => "spool_overflow",
            GapKind::ServerRejected => "server_rejected",
        };
        let node_id = gap.node_id.to_string();
        // An Agent-declared gap is Agent evidence: it is stored with its
        // author so the Server never presents it as its own refusal, and it
        // never overwrites the Server's own text for the same triple.
        sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, $2, $3, $4, $5, $6, 'agent') ON CONFLICT DO NOTHING")
            .bind(&node_id)
            .bind(gap.from_height as i64)
            .bind(gap.to_height as i64)
            .bind(kind)
            .bind(redacted(&gap.reason))
            .bind(gap.recorded_at.to_string())
            .execute(&mut **tx)
            .await?;
        if gap.kind == GapKind::UnrecoverableBackfill {
            open_uncovered_gap(tx, &node_id, gap.from_height, gap.to_height, received_at).await?;
            // A gap declared over heights already covered can settle at once.
            touched.push(TouchedRange {
                node_id,
                from_height: gap.from_height,
                to_height: gap.to_height,
            });
        }
        gaps.push(GapOutcome::Accepted);
    }

    // A backfill in this Report, or a gap it declared over heights already
    // covered, can settle a History Gap: resolve it in the same transaction so
    // resolved_at always agrees with the coverage it is derived from. Only the
    // gaps around the heights this Report closed and the ranges it declared
    // can have settled, so a live tip Report examines no gap at all.
    let settlement = if touched.is_empty() {
        None
    } else {
        let started = std::time::Instant::now();
        let settlement = settle_touched_history_gaps(tx, &touched, received_at).await?;
        Some((settlement, started.elapsed()))
    };

    Ok(BlockAdmission {
        samples,
        gaps,
        identity_contradicted,
        settlement,
    })
}

/// Judge one sample of an admitted Node and, when it is admitted, write it.
async fn admit_sample(
    tx: &mut Transaction<'_, Any>,
    sample: &BlockSummary,
    history: &mut NodeHistory,
    received_at: &str,
    observed_at: Rfc3339,
    touched: &mut Vec<TouchedRange>,
) -> Result<BlockOutcome, sqlx::Error> {
    let node_id = sample.node_id.to_string();
    let height = sample.block_number as i64;
    match sample.source {
        BlockSource::GapBackfill => {
            if !backfill_admitted(tx, &node_id, sample.block_number).await? {
                return Ok(BlockOutcome::OutsideOpenGap);
            }
        }
        BlockSource::Subscription => {
            if history.high_watermark.is_some_and(|mark| height <= mark) {
                return Ok(BlockOutcome::ResyncReplay);
            }
        }
    }
    if observe_block_identity(tx, sample, observed_at, received_at).await? {
        return Ok(BlockOutcome::ChainDivergence);
    }
    if !insert_summary(tx, sample, observed_at, received_at).await? {
        return Ok(BlockOutcome::AlreadyRecorded);
    }
    // Any accepted height leaves the open gap that contained it (a live
    // summary can land inside a declared range too) and becomes covered, so it
    // can never be admitted, rewritten, or counted again.
    if let Some(coverage_id) = open_gap_containing(tx, &node_id, sample.block_number).await? {
        close_coverage_height(tx, coverage_id, sample.block_number, received_at).await?;
        touched.push(TouchedRange {
            node_id: node_id.clone(),
            from_height: sample.block_number,
            to_height: sample.block_number,
        });
    }
    mark_covered_height(tx, &node_id, sample.block_number, received_at).await?;

    // Only the live subscription reports the head; a backfilled hole lies
    // below it and must never move the observed network head backwards.
    if sample.source == BlockSource::Subscription {
        sqlx::query("INSERT INTO observed_network_heads (node_id, block_number, block_hash, observed_at, confidence, eligible_sources) VALUES ($1, $2, $3, $4, 'unknown', '[\\\"subscription\\\"]') ON CONFLICT(node_id) DO UPDATE SET block_number=excluded.block_number, block_hash=excluded.block_hash, observed_at=excluded.observed_at, confidence=excluded.confidence, eligible_sources=excluded.eligible_sources")
            .bind(&node_id).bind(height).bind(sample.block_hash.to_string()).bind(observed_at.to_string()).execute(&mut **tx).await?;
    }
    // last_accepted_summary_at is the durable proof that this Node's history
    // was accepted again (ADR 0011 decision 4). The Admin refusal diagnosis
    // compares its newest Server-authored gap against it, so the diagnosis
    // clears when history resumes and stays clear after retention ages the raw
    // Block Summary rows out; updated_at cannot serve that purpose because the
    // current-head projection writes it on every Report.
    sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, last_accepted_summary_at, updated_at) VALUES ($1, $2, 1, $3, $4, $5, $6) ON CONFLICT(node_id) DO UPDATE SET historical_high_watermark=CASE WHEN block_history_state.historical_high_watermark >= excluded.historical_high_watermark THEN block_history_state.historical_high_watermark ELSE excluded.historical_high_watermark END, cumulative_block_count=block_history_state.cumulative_block_count + 1, cumulative_transaction_count=block_history_state.cumulative_transaction_count + excluded.cumulative_transaction_count, cumulative_self_seal_count=block_history_state.cumulative_self_seal_count + excluded.cumulative_self_seal_count, last_accepted_summary_at=excluded.last_accepted_summary_at, updated_at=excluded.updated_at")
        .bind(&node_id).bind(height).bind(sample.transaction_count as i64)
        .bind((sample.attribution.seal_signer_match == SealSignerMatch::SignerSelf) as i64).bind(received_at).bind(received_at).execute(&mut **tx).await?;
    history.high_watermark = Some(
        history
            .high_watermark
            .map_or(height, |mark| mark.max(height)),
    );
    // The block tiers advance exactly where the cumulative counters do: only
    // for a Block Summary this transaction has just inserted, so Block history
    // dedup decides for both.
    let observed_at = observed_at.to_string();
    crate::block_aggregates::record_accepted_block(
        tx,
        crate::block_aggregates::AcceptedBlock {
            node_id: &node_id,
            block_number: height,
            block_timestamp_ms: sample.block_timestamp_ms as i64,
            observed_at: &observed_at,
            transaction_count: sample.transaction_count as i64,
        },
        received_at,
    )
    .await?;
    Ok(BlockOutcome::Accepted)
}

/// Insert the Block Summary row; `false` when the height is already recorded.
async fn insert_summary(
    tx: &mut Transaction<'_, Any>,
    sample: &BlockSummary,
    observed_at: Rfc3339,
    received_at: &str,
) -> Result<bool, sqlx::Error> {
    let proposer = match &sample.attribution.protocol_proposer {
        platpulse_core::block::ProtocolProposer::Verified { identity } => {
            ("verified", Some(identity.as_str()))
        }
        platpulse_core::block::ProtocolProposer::Unknown {} => ("unknown", None),
    };
    let signer = match sample.attribution.seal_signer_match {
        SealSignerMatch::SignerSelf => "self",
        SealSignerMatch::Other => "other",
        SealSignerMatch::Unknown => "unknown",
    };
    let inserted = sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, block_interval_ms, source, coinbase, seal_signer_key_fingerprint, seal_signer_match, node_key_fingerprint, node_key_valid_from, node_key_valid_until, node_key_history_complete, seal_recovery_rule, seal_evidence, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, accepted_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26) ON CONFLICT DO NOTHING")
        .bind(sample.node_id.to_string()).bind(sample.block_number as i64).bind(sample.block_hash.to_string()).bind(sample.parent_hash.to_string())
        .bind(sample.network_identity.genesis_hash.to_string()).bind(sample.network_identity.chain_id as i64).bind(sample.network_identity.p2p_network_id as i64).bind(sample.network_identity.address_hrp.as_deref().unwrap_or(""))
        .bind(sample.block_timestamp_ms as i64).bind(observed_at.to_string()).bind(sample.transaction_count as i64).bind(sample.block_interval_ms.map(|v| v as i64)).bind(match sample.source { BlockSource::Subscription => "subscription", BlockSource::GapBackfill => "gap_backfill" })
        .bind(sample.attribution.coinbase.to_string()).bind(sample.attribution.seal_signer_key_fingerprint.as_ref().map(ToString::to_string)).bind(signer).bind(sample.attribution.node_key.as_ref().map(|key| key.fingerprint.to_string())).bind(sample.attribution.node_key.as_ref().and_then(|key| key.valid_from.map(|value| value.to_string()))).bind(sample.attribution.node_key.as_ref().and_then(|key| key.valid_until.map(|value| value.to_string()))).bind(sample.attribution.node_key.as_ref().is_some_and(|key| key.history_complete) as i64).bind(sample.attribution.seal_recovery_rule.as_deref()).bind(sample.attribution.seal_evidence.as_deref()).bind(proposer.0).bind(proposer.1).bind(&sample.attribution.attribution_reason).bind(received_at)
        .execute(&mut **tx).await?;
    Ok(inserted.rows_affected() == 1)
}
