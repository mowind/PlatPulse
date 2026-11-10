//! The Report admission fence: the whole-Report verdict a Report receives
//! before it writes anything, and [`Admission::apply`], the Inventory writes
//! an admitted Report makes once the verdict is final.
//!
//! The order is the contract. An exact replay answers first. Then come the
//! frozen v1 route, the live Agent and its Epoch, the Report Sequence slot,
//! the Boot fences, and the stale-sequence fence. After those come the
//! revision a v1 Agent declares and Network admission of every Node. The
//! revision the Server allocates for a v2 declaration comes last, because
//! ADR 0007 allows allocation only after whole-Inventory validation and every
//! fence. A Report that breaks several rules gets the earliest verdict.
//!
//! A taken sequence comes before the Boot fences: a different body in a slot
//! another Report holds is a competing body whatever its Boot, and the
//! security event it records must not be hidden behind a stale-Boot refusal.

use axum::response::Response;
use sqlx::{Any, FromRow, Transaction};

use crate::block_history::NodeAdmission;
use crate::cutover::InventoryCutover;
use crate::http::AppState;
use platpulse_core::protocol::PROTOCOL_VERSION_V2;
use platpulse_core::{AgentReport, BootTransition, RejectionCode, ReportInventory, Sha256Hex};

use super::*;

/// The Agent row every fence reads, taken with the live predicate.
#[derive(FromRow)]
pub(super) struct AgentRow {
    pub(super) agent_epoch: i64,
    pub(super) active_boot_id: Option<String>,
    pub(super) active_boot_status: String,
    pub(super) last_report_sequence: Option<i64>,
    pub(super) last_inventory_revision: i64,
    /// NULL until the Agent has an accepted Inventory.
    pub(super) inventory_sha256: Option<String>,
}

pub(super) enum ReportVerdict {
    /// This identity already holds a Receipt.
    Replay(Replay),
    /// Answered without a Receipt.
    Refused(ReportRefusal),
    /// Answered with a rejection Receipt.
    Rejected(ReportRejection),
    Admitted(Admission),
}

pub(super) enum ReportRefusal {
    /// An unseen v1 Report after the Inventory cutover (design 15.10.5).
    NotReplayable,
    /// The Agent was removed (design §15.2, #171); a late Report fails like
    /// an invalid credential.
    AgentRemoved,
    /// Another Report holds this (Epoch, Boot, sequence) slot.
    SequenceTaken,
}

pub(super) struct ReportRejection {
    pub(super) code: RejectionCode,
    pub(super) reason: &'static str,
    /// A competing Boot is a contradiction the Agent's security event counts.
    pub(super) security_event: bool,
}

impl ReportRejection {
    fn plain(code: RejectionCode, reason: &'static str) -> ReportVerdict {
        ReportVerdict::Rejected(Self {
            code,
            reason,
            security_event: false,
        })
    }

    fn competing_boot(reason: &'static str) -> ReportVerdict {
        ReportVerdict::Rejected(Self {
            code: RejectionCode::ConflictingBoot,
            reason,
            security_event: true,
        })
    }
}

/// What an admitted Report may write.
pub(super) struct Admission {
    pub(super) agent: AgentRow,
    /// The revision a frozen v1 Agent declares; v2 declares content only.
    pub(super) declared_revision: Option<u64>,
    pub(super) inventory_hash: Sha256Hex,
    /// The declaration equals the accepted Inventory.
    pub(super) inventory_unchanged: bool,
    /// The declared (v1) or allocated (v2) revision the Inventory is accepted at.
    pub(super) inventory_revision: u64,
    /// Purged Nodes are refused per Node; ownership is decided later.
    pub(super) nodes: NodeAdmission,
}

/// A storage statement failed while a Report was ingested.
pub(super) struct IngestFailure {
    pub(super) operation: &'static str,
    pub(super) error: sqlx::Error,
}

impl IngestFailure {
    pub(super) fn of(operation: &'static str) -> impl FnOnce(sqlx::Error) -> Self {
        move |error| Self { operation, error }
    }

    pub(super) fn into_response(self, state: &AppState, request_id: &str) -> Response {
        storage_failure(state, request_id, self.operation, &self.error)
    }
}

/// What applying an admitted Inventory leaves for the rest of the Report.
pub(super) enum InventoryApplied {
    /// The Inventory is projected. `ownership_contradiction` is set when the
    /// Report declared another Agent's Node without a valid pending transfer
    /// (or with a rejected or contradicting one): the source Agent's later
    /// Reports after a completed transfer land here too, and the Report
    /// records one security event for them (design §4.6).
    Applied { ownership_contradiction: bool },
    /// A Node's owner changed between admission and projection. The Report
    /// is refused whole so nothing of this decision commits, and the Agent's
    /// retry is judged against the owner it can now see (issue #280).
    OwnerChanged,
}

impl Admission {
    /// Write the Inventory side of an admitted Report: the sequence gap it
    /// opens, the transfers its declarations complete, the Nodes it retires
    /// and the Nodes it projects with their relations. Ownership refusals are
    /// recorded in [`Admission::nodes`] for the rest of the Report.
    ///
    /// Every statement must succeed: a backend that aborts the transaction on
    /// a failed statement (ADR 0013) would otherwise fail the commit later
    /// with no clue why.
    pub(super) async fn apply<I: ReportInventory>(
        &mut self,
        tx: &mut Transaction<'_, Any>,
        agent_id: &str,
        report: &AgentReport<I>,
        now_text: &str,
    ) -> Result<InventoryApplied, IngestFailure> {
        if let Some(last) = self.agent.last_report_sequence
            && report.boot_id.to_string()
                == self.agent.active_boot_id.as_deref().unwrap_or_default()
            && report.report_sequence > (last as u64).saturating_add(1)
        {
            sqlx::query("INSERT INTO report_sequence_gaps (agent_id, boot_id, from_sequence, to_sequence, created_at) VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING")
                .bind(agent_id)
                .bind(report.boot_id.to_string())
                .bind(last.saturating_add(1))
                .bind(report.report_sequence.saturating_sub(1) as i64)
                .bind(now_text)
                .execute(&mut **tx)
                .await
                .map_err(IngestFailure::of("record report sequence gap"))?;
        }
        let ownership_contradiction = self
            .resolve_ownership(tx, agent_id, report, now_text)
            .await?;
        // Inventory is accepted as a complete set: ownership-refused and
        // purged Nodes reject only their own current and samples and never
        // retire valid siblings. Equal content is unchanged, so a replayed
        // declaration neither retires siblings nor rewrites ownership.
        if !self.inventory_unchanged {
            self.retire_undeclared(tx, agent_id, report, now_text)
                .await?;
        }
        for node in report.inventory.nodes() {
            if !self.nodes.admits(node.node_id) {
                continue;
            }
            let projected = sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, $2, $3, $4, $5, 'active', 'private', $6, $7, $8) ON CONFLICT(node_id) DO UPDATE SET network_key=excluded.network_key, display_name=COALESCE(nodes.display_name, excluded.display_name), rpc_endpoint=excluded.rpc_endpoint, lifecycle='active', inventory_revision=excluded.inventory_revision, updated_at=excluded.updated_at WHERE nodes.agent_id=excluded.agent_id")
                .bind(node.node_id.to_string())
                .bind(agent_id)
                .bind(node.network_key.as_str())
                .bind(&node.display_name)
                .bind(node.rpc_endpoint.as_str())
                .bind(self.inventory_revision as i64)
                .bind(now_text)
                .bind(now_text)
                .execute(&mut **tx)
                .await
                .map_err(IngestFailure::of("project inventory node"))?;
            // The projection only ever follows its owner: a row another Agent
            // owns is never overwritten here, even if a writer got past the
            // ownership check above.
            if projected.rows_affected() != 1 {
                tracing::warn!(
                    node_id = %node.node_id,
                    "node owner changed between admission and projection; report refused for retry"
                );
                return Ok(InventoryApplied::OwnerChanged);
            }
            // A first Report opens the relation intervals; a change of
            // reporting Agent or Network key closes the old interval and opens
            // the new one, so history is never read from the Node's current
            // columns.
            crate::relationships::record_relations(
                tx,
                &node.node_id.to_string(),
                agent_id,
                node.network_key.as_str(),
                now_text,
            )
            .await
            .map_err(IngestFailure::of("record node relations"))?;
        }
        Ok(InventoryApplied::Applied {
            ownership_contradiction,
        })
    }

    /// Decide every declared Node another Agent owns. Only a pending transfer
    /// targeting this Agent with a validated Network Identity switches
    /// ownership (design §4.6); every other outcome keeps the source
    /// authoritative and refuses the Node for this Report. Returns whether a
    /// refusal is a contradiction worth a security event.
    async fn resolve_ownership<I: ReportInventory>(
        &mut self,
        tx: &mut Transaction<'_, Any>,
        agent_id: &str,
        report: &AgentReport<I>,
        now_text: &str,
    ) -> Result<bool, IngestFailure> {
        let mut contradiction = false;
        for node in report.inventory.nodes() {
            if !self.nodes.admits(node.node_id) {
                // Purged in Network admission.
                continue;
            }
            let owner =
                sqlx::query_scalar::<_, String>("SELECT agent_id FROM nodes WHERE node_id = $1")
                    .bind(node.node_id.to_string())
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(IngestFailure::of("read node owner"))?;
            if owner.is_none_or(|owner| owner == agent_id) {
                continue;
            }
            match resolve_node_transfer(tx, report, node, agent_id, now_text)
                .await
                .map_err(IngestFailure::of("resolve node transfer"))?
            {
                // Ownership switched atomically; the Node's current and
                // samples join this Report.
                TransferResolution::Completed => {}
                // A legitimate in-flight declaration without an identity
                // probe yet: the transfer stays pending.
                TransferResolution::Unverified => self.nodes.refuse_ownership(node.node_id),
                TransferResolution::NoTransfer
                | TransferResolution::NetworkKeyMismatch
                | TransferResolution::IdentityMismatch => {
                    self.nodes.refuse_ownership(node.node_id);
                    contradiction = true;
                }
            }
        }
        Ok(contradiction)
    }

    /// Retire the Agent's Nodes the declaration no longer lists. An empty
    /// Inventory retires them all; a declaration whose every Node was refused
    /// retires nothing.
    async fn retire_undeclared<I: ReportInventory>(
        &self,
        tx: &mut Transaction<'_, Any>,
        agent_id: &str,
        report: &AgentReport<I>,
        now_text: &str,
    ) -> Result<(), IngestFailure> {
        let retire = IngestFailure::of("retire undeclared nodes");
        if report.inventory.nodes().is_empty() {
            sqlx::query("UPDATE nodes SET lifecycle='retired', updated_at=$1 WHERE agent_id=$2")
                .bind(now_text)
                .bind(agent_id)
                .execute(&mut **tx)
                .await
                .map_err(retire)?;
            return Ok(());
        }
        let accepted = report
            .inventory
            .nodes()
            .iter()
            .filter(|node| self.nodes.admits(node.node_id))
            .map(|node| node.node_id.to_string())
            .collect::<Vec<_>>();
        if accepted.is_empty() {
            return Ok(());
        }
        let placeholders = std::iter::repeat_n("?", accepted.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = crate::database::number_placeholders(&format!(
            "UPDATE nodes SET lifecycle='retired', updated_at=? WHERE agent_id=? AND node_id NOT IN ({placeholders})"
        ));
        let mut query = sqlx::query(&sql).bind(now_text).bind(agent_id);
        for node_id in &accepted {
            query = query.bind(node_id);
        }
        query.execute(&mut **tx).await.map_err(retire)?;
        Ok(())
    }
}

pub(super) async fn admit_report<I: ReportInventory>(
    tx: &mut Transaction<'_, Any>,
    agent_id: &str,
    report: &AgentReport<I>,
    hash: &Sha256Hex,
    cutover: InventoryCutover,
) -> Result<ReportVerdict, IngestFailure> {
    if let Some(replay) = find_replay::<I>(tx, agent_id, report.report_id, hash)
        .await
        .map_err(IngestFailure::of("read stored receipt"))?
    {
        return Ok(ReportVerdict::Replay(replay));
    }
    // After the coordinated cutover the frozen v1 route is replay-only. An
    // unseen v1 Report is explicitly unsupported: it creates no acceptance
    // Receipt, advances no Boot, allocates no revision and changes no
    // projection. It is not mixed-version ingestion, an automatic downgrade or
    // an old-backlog submission path (design 15.10.5).
    if cutover.v1_replay_only() && I::protocol_version() != PROTOCOL_VERSION_V2 {
        return Ok(ReportVerdict::Refused(ReportRefusal::NotReplayable));
    }
    // Reading the row inside the receipt transaction with the live predicate
    // means no later Inventory, retry, or restart can reconstruct a removed
    // Agent's Nodes.
    let Some(agent) = sqlx::query_as::<_, AgentRow>(
        "SELECT agent_epoch, active_boot_id, active_boot_status, last_report_sequence, last_inventory_revision, inventory_sha256 FROM agents WHERE agent_id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(IngestFailure::of("read agent"))?
    else {
        return Ok(ReportVerdict::Refused(ReportRefusal::AgentRemoved));
    };
    if report.agent_epoch != agent.agent_epoch as u64 {
        return Ok(ReportRejection::plain(
            RejectionCode::StaleBoot,
            "Agent epoch is stale",
        ));
    }
    // A sequence is unique within one boot. This early read spares a doomed
    // Report its projection work; store_receipt is the barrier that holds
    // under concurrency.
    let taken = sqlx::query_scalar::<_, String>(
        "SELECT report_id FROM agent_report_receipts WHERE agent_id = $1 AND agent_epoch = $2 AND boot_id = $3 AND report_sequence = $4",
    )
    .bind(agent_id)
    .bind(report.agent_epoch as i64)
    .bind(report.boot_id.to_string())
    .bind(report.report_sequence as i64)
    .fetch_optional(&mut **tx)
    .await
    .map_err(IngestFailure::of("read report sequence"))?;
    if taken.is_some() {
        return Ok(ReportVerdict::Refused(ReportRefusal::SequenceTaken));
    }
    if let Some(verdict) = boot_fence(tx, agent_id, report, &agent).await? {
        return Ok(verdict);
    }

    let declared_revision = report.inventory.declared_revision();
    let inventory_hash = report.inventory.accepted_sha256();
    let inventory_unchanged = agent.inventory_sha256.as_deref() == Some(inventory_hash.as_str());
    // Frozen v1: the Agent declares its own monotonically increasing revision.
    if let Some(declared) = declared_revision {
        let accepted = agent.last_inventory_revision as u64;
        if declared < accepted {
            return Ok(ReportRejection::plain(
                RejectionCode::InventoryRevisionConflict,
                "Inventory revision is older than the accepted revision",
            ));
        }
        if declared == accepted && agent.inventory_sha256.is_some() && !inventory_unchanged {
            return Ok(ReportRejection::plain(
                RejectionCode::InventoryRevisionConflict,
                "Inventory content conflicts at the accepted revision",
            ));
        }
    }

    // Network admission is a whole-report verdict, so it is decided for every
    // Node before the report writes anything: a sequence gap or a completed
    // Transfer written ahead of it would be committed together with the
    // rejection receipt.
    let mut nodes = NodeAdmission::default();
    for node in report.inventory.nodes() {
        // A purged Node ID is a permanent admission boundary (design §15.3,
        // ADR 0004, issue #170): the Owner already erased this Node's
        // projection, monitoring history, and Validator Links, and no later
        // Inventory revision, retry, restart, or background write may
        // reconstruct them. Reject the entry per Node so a valid sibling in
        // the same legal report still reports, and do not let a stale or
        // unknown Network key on the removed ID block the rest of the report.
        if crate::node_purge::is_purged(tx, &node.node_id.to_string())
            .await
            .map_err(IngestFailure::of("read node purge"))?
        {
            nodes.refuse_purged(node.node_id);
            continue;
        }
        let known = sqlx::query_scalar::<_, String>(
            "SELECT network_key FROM networks WHERE network_key = $1",
        )
        .bind(node.network_key.as_str())
        .fetch_optional(&mut **tx)
        .await
        .map_err(IngestFailure::of("read network"))?;
        if known.is_none() {
            return Ok(ReportRejection::plain(
                RejectionCode::NetworkKeyUnknown,
                "Network key is not registered",
            ));
        }
    }

    // v2 declares content only: the Server allocates the accepted revision inside
    // this authoritative transaction. Identical content keeps the accepted
    // revision; any content change (including A -> B -> A) advances it. The
    // comparison and allocation commit with the receipt, so concurrent reports
    // can never allocate the same revision twice.
    let inventory_revision = match declared_revision {
        Some(declared) => declared,
        None if inventory_unchanged => agent.last_inventory_revision.max(0) as u64,
        None => {
            // The column is a positive signed 64-bit integer, so the ceiling is
            // i64::MAX even though the arithmetic runs in u64. The checked add
            // plus the signed-range filter keep exhaustion an explicit refusal
            // instead of a cast that would wrap negative.
            let last = agent.last_inventory_revision.max(0) as u64;
            let Some(next) = last.checked_add(1).filter(|next| *next <= i64::MAX as u64) else {
                return Ok(ReportRejection::plain(
                    RejectionCode::InventoryRevisionConflict,
                    "Inventory revision range is exhausted",
                ));
            };
            next
        }
    };
    Ok(ReportVerdict::Admitted(Admission {
        agent,
        declared_revision,
        inventory_hash,
        inventory_unchanged,
        inventory_revision,
        nodes,
    }))
}

/// The Boot fences: a Report either continues the active Boot with a newer
/// sequence, or opens its successor through an exact drained-previous
/// transition after the active Boot closed.
async fn boot_fence<I: ReportInventory>(
    tx: &mut Transaction<'_, Any>,
    agent_id: &str,
    report: &AgentReport<I>,
    agent: &AgentRow,
) -> Result<Option<ReportVerdict>, IngestFailure> {
    let Some(active) = agent.active_boot_id.as_deref() else {
        return Ok(None);
    };
    if active == report.boot_id.to_string() {
        let stale = agent.active_boot_status == "closing"
            || agent.active_boot_status == "closed"
            || agent
                .last_report_sequence
                .is_some_and(|last| report.report_sequence <= last as u64);
        return Ok(stale.then(|| {
            ReportRejection::plain(
                RejectionCode::StaleReport,
                "Report sequence is not newer than the accepted report",
            )
        }));
    }
    if report.boot_transition != BootTransition::DrainedPrevious {
        // A failed read must not pass for "this boot was never seen": that
        // would turn the closed-boot refusal into a competing boot.
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM agent_boots WHERE agent_id=$1 AND agent_epoch=$2 AND boot_id=$3",
        )
        .bind(agent_id)
        .bind(report.agent_epoch as i64)
        .bind(report.boot_id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(IngestFailure::of("read boot status"))?;
        if status.as_deref() == Some("closed") {
            return Ok(Some(ReportRejection::plain(
                RejectionCode::StaleBoot,
                "Report belongs to a closed boot; only exact replay is accepted",
            )));
        }
        return Ok(Some(ReportRejection::competing_boot(
            "Report belongs to a competing boot",
        )));
    }
    if report
        .previous_boot_id
        .map(|previous| previous.to_string())
        .as_deref()
        != Some(active)
    {
        return Ok(Some(ReportRejection::competing_boot(
            "Report belongs to a competing boot",
        )));
    }
    if agent.active_boot_status != "closed" {
        return Ok(Some(ReportRejection::competing_boot(
            "Previous boot has not completed its closing receipt",
        )));
    }
    Ok(None)
}
