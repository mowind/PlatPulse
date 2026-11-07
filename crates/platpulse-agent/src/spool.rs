//! The Durable Spool: the Agent's bounded queue of immutable Agent Reports
//! awaiting Server acknowledgement.
//!
//! A Report leaves the spool only when the Server's Report Receipt for its exact
//! bytes is applied. Applying is one Agent Store transaction (ADR 0001): the
//! receipt is validated against the stored Report, every per-Node and
//! per-sample disposition is settled, the Applied Receipt Record is written,
//! the Report is removed, and the Boot transition the Report carried is
//! settled, or the transaction leaves everything as it was.

use platpulse_core::identity::BootId;
use platpulse_core::inventory::NodeInventory;
use platpulse_core::{
    AgentReport, BootTransition, InventoryAcceptance, InventoryDeclaration, InventoryDisposition,
    NodeCurrentDisposition, ReceiptDisposition, ReportInventory, ReportReceipt, ReportReceiptV2,
    SampleDisposition, SampleDispositionKind, SampleRef,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::database::{
    AgentStore, WriteTx, applied_receipt_expiry_cutoff, delete_expired_receipt_markers, now_rfc3339,
};
use crate::reporting::{ReportStoreError, ReportTransport, report_protocol_major};

/// What applying a receipt did to the Report it acknowledged, read from the
/// stored Report inside the receipt transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppliedReport {
    pub(crate) report_sequence: u64,
    pub(crate) boot_id: BootId,
    pub(crate) boot_transition: BootTransition,
    /// The Report is a Closing whose Boot the Agent had already moved past, so
    /// applying it left the Boot where it was.
    pub(crate) superseded_closing: bool,
}

/// Apply a stored v1 receipt from its raw envelope. Retained for the v1 entry
/// points and their tests; v2 delivery uses [`apply_receipt_typed`] with the
/// Server-assigned acceptance.
#[cfg(test)]
pub(crate) async fn apply_receipt(
    store: &mut AgentStore,
    report_id: &str,
    body_sha256: &str,
    disposition: &str,
    receipt_body: &[u8],
    applied_at: &str,
) -> Result<AppliedReport, sqlx::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReceiptEnvelope {
        receipt: ReportReceipt,
    }
    let receipt = serde_json::from_slice::<ReceiptEnvelope>(receipt_body)
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?
        .receipt;
    apply_receipt_typed::<NodeInventory>(
        store,
        report_id,
        body_sha256,
        disposition,
        receipt,
        None,
        applied_at,
    )
    .await
}

/// Apply a stored receipt and delete its report only after all receipt
/// dispositions have been durably processed, in one Agent Store transaction.
///
/// Applying is a read-dependent write: the stored body is hashed and re-parsed
/// to decide whether the Report may be deleted, so that read stays inside the
/// write permit for its whole span (ADR 0001). It is one Report, not a spool
/// scan; issue #269 moved the O(queued Reports) hashing and parsing out of the
/// permit, not the work that authorizes this single transaction.
pub(crate) async fn apply_receipt_typed<I: ReportInventory>(
    store: &mut AgentStore,
    report_id: &str,
    body_sha256: &str,
    disposition: &str,
    receipt: ReportReceipt,
    acceptance: Option<InventoryAcceptance>,
    applied_at: &str,
) -> Result<AppliedReport, sqlx::Error> {
    receipt
        .validate()
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    let expected_disposition = receipt_disposition_name(receipt.disposition);
    if receipt.report_id.to_string() != report_id
        || receipt.report_body_sha256.to_string() != body_sha256
        || expected_disposition != disposition
    {
        return Err(sqlx::Error::Protocol(
            "receipt does not match report".to_owned(),
        ));
    }

    let expiry_cutoff = applied_receipt_expiry_cutoff(applied_at)?;
    #[cfg(test)]
    let body_reads = store.spool_body_read_probe();
    let mut tx = store.write().await?;
    if let Some((marker_hash, marker_disposition)) = sqlx::query_as::<_, (String, String)>(
        "SELECT report_body_sha256, disposition FROM report_receipts WHERE report_id = ?",
    )
    .bind(report_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        let reason = if marker_hash == body_sha256 && marker_disposition == disposition {
            "duplicate receipt identity"
        } else {
            "receipt identity conflicts with an Applied Receipt Record"
        };
        sqlx::query(
            "UPDATE spool_state SET store_fatal=1, store_error=?, updated_at=? WHERE singleton=1",
        )
        .bind(reason)
        .bind(applied_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Err(sqlx::Error::Protocol(reason.to_owned()));
    }
    let mut superseded_closing = false;
    let mut compact_replay = false;
    let mut inventory_acceptance_conflict: Option<String> = None;
    // A stored body that no longer matches what was delivered is not a retryable
    // condition: the Report stays queued, so rolling back alone would recompute
    // the same failure on every attempt. The reason is carried out of the
    // transaction and latches the Store fatal (issue #269).
    let mut store_integrity_failure: Option<String> = None;
    let transaction_result: Result<AppliedReport, sqlx::Error> = async {
        let stored: Option<(Vec<u8>, String, i64)> =
            sqlx::query_as("SELECT body, body_sha256, body_bytes FROM reports WHERE report_id = ?")
                .bind(report_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((raw_report, stored_body_sha256, stored_body_bytes)) = stored else {
            let reason = "report is not in the Agent spool".to_owned();
            store_integrity_failure = Some(reason.clone());
            return Err(sqlx::Error::Protocol(reason));
        };
        let actual_hash = format!("0x{}", hex::encode(sha2::Sha256::digest(&raw_report)));
        if stored_body_bytes < 0
            || stored_body_bytes as usize != raw_report.len()
            || stored_body_sha256 != actual_hash
            || body_sha256 != actual_hash
        {
            let reason = "stored report failed integrity validation".to_owned();
            store_integrity_failure = Some(reason.clone());
            return Err(sqlx::Error::Protocol(reason));
        }
        let parsed_report: AgentReport<I> = match serde_json::from_slice::<AgentReport<I>>(&raw_report)
        {
            Ok(report) => report,
            Err(error) => {
                let reason = format!("stored report is invalid: {error}");
                store_integrity_failure = Some(reason.clone());
                return Err(sqlx::Error::Protocol(reason));
            }
        };
        if let Err(error) = parsed_report.validate() {
            let reason = format!("stored report is invalid: {error}");
            store_integrity_failure = Some(reason.clone());
            return Err(sqlx::Error::Protocol(reason));
        }
        if parsed_report.report_id.to_string() != report_id {
            let reason = "stored report id mismatch".to_owned();
            store_integrity_failure = Some(reason.clone());
            return Err(sqlx::Error::Protocol(reason));
        }
    let mut expected_nodes = std::collections::HashSet::new();
    for node in parsed_report.inventory.nodes() {
        expected_nodes.insert(node.node_id);
    }
    // A replay older than the Server's receipt-body window answers with the
    // compact receipt (ADR 0009): the dispositions survive but the per-Node and
    // per-sample arrays are empty. Only an appliable receipt for a Report that
    // names Nodes can be told apart from a fresh one this way, because a fresh
    // receipt lists every Node; a receipt that lists some entries but not all
    // is still refused below.
    let compact = receipt.disposition != ReceiptDisposition::Rejected
        && receipt.is_compact()
        && !expected_nodes.is_empty();
    compact_replay = compact;
    let mut seen_nodes = std::collections::HashSet::new();
    for node in &receipt.nodes {
        if !expected_nodes.contains(&node.node_id) || !seen_nodes.insert(node.node_id) {
            return Err(sqlx::Error::Protocol(
                "receipt contains invalid or duplicate node".to_owned(),
            ));
        }
        if node.current == NodeCurrentDisposition::Rejected
            && node.rejections.iter().any(|r| r.retryable)
        {
            return Err(sqlx::Error::Protocol(
                "rejected Node must be terminal".to_owned(),
            ));
        }
    }
    if receipt.disposition != ReceiptDisposition::Rejected
        && !compact
        && seen_nodes.len() != expected_nodes.len()
    {
        return Err(sqlx::Error::Protocol(
            "receipt does not cover every Node in the stored report".to_owned(),
        ));
    }
    let mut expected_samples = std::collections::HashSet::new();
    for sample in &parsed_report.block_summaries {
        expected_samples.insert((
            sample.node_id,
            "block",
            sample.block_number,
            sample.block_number,
        ));
    }
    for gap in &parsed_report.history_gaps {
        expected_samples.insert((gap.node_id, "gap", gap.from_height, gap.to_height));
    }
    let mut seen_samples = std::collections::HashSet::new();
    for sample in &receipt.samples {
        let (kind, from, to) = sample_reference(sample.sample);
        if !expected_samples.contains(&(sample.node_id, kind, from, to))
            || !seen_samples.insert((sample.node_id, kind, from, to))
        {
            return Err(sqlx::Error::Protocol(
                "receipt contains invalid or duplicate sample".to_owned(),
            ));
        }
    }
    if receipt.disposition != ReceiptDisposition::Rejected
        && !compact
        && seen_samples.len() != expected_samples.len()
    {
        return Err(sqlx::Error::Protocol(
            "receipt does not cover every sample in the stored report".to_owned(),
        ));
    }

    // The immutable report has already been parsed and all receipt references
    // validated above. Current observations are not copied into retry state.

    // A compact receipt no longer says how each sample was disposed. An
    // accepted one accepted every sample, so it settles them exactly as the full
    // receipt would. A partially accepted one cannot say which samples were
    // refused, so none is settled: deleting the Report releases their
    // assignments and they return to the durable queue for the next Report.
    // The Server answers a re-sent block it already holds as `resync_replay`,
    // whereas guessing a terminal rejection here would record one that never
    // happened.
    let compact_accepted_samples: Vec<SampleDisposition> =
        if compact && receipt.disposition == ReceiptDisposition::Accepted {
            parsed_report
                .block_summaries
                .iter()
                .map(|summary| SampleDisposition {
                    node_id: summary.node_id,
                    sample: SampleRef::Block {
                        height: summary.block_number,
                    },
                    disposition: SampleDispositionKind::Accepted,
                    rejection: None,
                })
                .chain(parsed_report.history_gaps.iter().map(|gap| SampleDisposition {
                    node_id: gap.node_id,
                    sample: SampleRef::Gap {
                        from_height: gap.from_height,
                        to_height: gap.to_height,
                    },
                    disposition: SampleDispositionKind::Accepted,
                    rejection: None,
                }))
                .collect()
        } else {
            Vec::new()
        };
    let settled_samples = if compact {
        &compact_accepted_samples
    } else {
        &receipt.samples
    };
    for sample in settled_samples {
        let (kind, from_height, to_height) = sample_reference(sample.sample);
        sqlx::query("DELETE FROM report_sample_assignments WHERE report_id = ? AND node_id = ? AND sample_kind = ? AND from_height = ? AND to_height = ?")
            .bind(report_id).bind(sample.node_id.to_string()).bind(kind)
            .bind(from_height as i64).bind(to_height as i64)
            .execute(&mut *tx).await?;
        match sample.disposition {
            SampleDispositionKind::Accepted | SampleDispositionKind::TerminalRejected => {
                if kind == "block" {
                    let block_hash = parsed_report
                        .block_summaries
                        .iter()
                        .find(|summary| {
                            summary.node_id == sample.node_id
                                && summary.block_number == from_height
                        })
                        .map(|summary| summary.block_hash.to_string())
                        .ok_or_else(|| {
                            sqlx::Error::Protocol(
                                "stored report is missing the referenced block summary".to_owned(),
                            )
                        })?;
                    sqlx::query(
                        "DELETE FROM block_summaries WHERE node_id = ? AND block_number = ? AND block_hash = ?",
                    )
                    .bind(sample.node_id.to_string())
                    .bind(from_height as i64)
                    .bind(block_hash)
                    .execute(&mut *tx)
                    .await?;
                } else {
                    let gap_kind = parsed_report
                        .history_gaps
                        .iter()
                        .find(|gap| {
                            gap.node_id == sample.node_id
                                && gap.from_height == from_height
                                && gap.to_height == to_height
                        })
                        .map(|gap| crate::block::gap_kind_name(gap.kind))
                        .ok_or_else(|| {
                            sqlx::Error::Protocol(
                                "stored report is missing the referenced history gap".to_owned(),
                            )
                        })?;
                    sqlx::query("DELETE FROM history_gaps WHERE node_id = ? AND from_height = ? AND to_height = ? AND kind = ?")
                        .bind(sample.node_id.to_string())
                        .bind(from_height as i64)
                        .bind(to_height as i64)
                        .bind(gap_kind)
                        .execute(&mut *tx)
                        .await?;
                }
                if sample.disposition == SampleDispositionKind::TerminalRejected {
                    if let Some(rejection) = &sample.rejection {
                        sqlx::query("INSERT INTO rejection_ledger (report_id, node_id, sample_kind, from_height, to_height, rejection_code, reason, rejected_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                            .bind(report_id)
                            .bind(sample.node_id.to_string())
                            .bind(kind)
                            .bind(from_height as i64)
                            .bind(to_height as i64)
                            .bind(format!("{:?}", rejection.code).to_lowercase())
                            .bind(&rejection.reason)
                            .bind(applied_at)
                            .execute(&mut *tx)
                            .await?;
                        sqlx::query("INSERT OR IGNORE INTO history_gaps (node_id, from_height, to_height, kind, created_at) VALUES (?, ?, ?, 'server_rejected', ?)")
                            .bind(sample.node_id.to_string())
                            .bind(from_height as i64)
                            .bind(to_height as i64)
                            .bind(applied_at)
                            .execute(&mut *tx)
                            .await?;
                    }
                }
            }
            // Leave retryable samples in the durable queue, now detached from
            // the old report. The next planner includes them once.
            SampleDispositionKind::RetryableRejected => {}
        }
    }

    sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, ?, ?)")
        .bind(report_id)
        .bind(body_sha256)
        .bind(disposition)
        .bind(applied_at)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM reports WHERE report_id = ? AND EXISTS (SELECT 1 FROM report_receipts WHERE report_id = ?)")
        .bind(report_id)
        .bind(report_id)
        .execute(&mut *tx)
        .await?;
    delete_expired_receipt_markers(&mut *tx, &expiry_cutoff).await?;
    // Inventory Declaration Record (issue #181): remember the Inventory this
    // receipt made effective, using the report that was actually accepted. A
    // rejection leaves the record alone, because nothing about a refused
    // declaration became effective.
    if receipt.disposition != ReceiptDisposition::Rejected
        && matches!(
            receipt.inventory,
            Some(InventoryDisposition::Accepted | InventoryDisposition::Unchanged)
        )
    {
        // v2 accepts the Server-owned revision and canonical declaration
        // fingerprint; v1 keeps its Agent-declared revision and content hash.
        // The Agent never invents a revision, never writes it back into a
        // declaration, and never treats it as permission to generate a report.
        let (revision, fingerprint) = match &acceptance {
            Some(acceptance) => {
                let declared_fingerprint = parsed_report.inventory.accepted_sha256();
                if acceptance.fingerprint != declared_fingerprint {
                    inventory_acceptance_conflict = Some(format!(
                        "receipt fingerprint {} does not match the stored declaration {}",
                        acceptance.fingerprint, declared_fingerprint
                    ));
                    return Err(sqlx::Error::Protocol(
                        "receipt inventory fingerprint does not match the stored declaration"
                            .to_owned(),
                    ));
                }
                (acceptance.revision, acceptance.fingerprint.clone())
            }
            None => (
                parsed_report.inventory.declared_revision().unwrap_or(0),
                parsed_report.inventory.accepted_sha256(),
            ),
        };
        if let Some(record) =
            crate::inventory_declaration::read_inventory_declaration(&mut tx).await?
        {
            // Equal revision with a different fingerprint fails closed and keeps
            // the report for investigation; a lower revision may complete its
            // acknowledgement but never moves the record backward.
            if record.revision == revision && record.sha256 != fingerprint.as_str() {
                inventory_acceptance_conflict = Some(format!(
                    "revision {revision} binds fingerprint {fingerprint} but the record holds {}",
                    record.sha256
                ));
                return Err(sqlx::Error::Protocol(
                    "inventory acceptance conflicts with the local confirmation record".to_owned(),
                ));
            }
        }
        crate::inventory_declaration::record_inventory_acceptance(
            &mut tx,
            revision,
            fingerprint.as_str(),
            report_id,
            applied_at,
        )
        .await?;
    }
    if receipt.disposition == ReceiptDisposition::Rejected {
        // A rejected report leaves the spool, but its rejection must stay
        // visible: the Agent keeps re-sending an unacknowledged Boot transition
        // until the Server accepts it, so the last rejection (bounded to the
        // Agent's single diagnostic row) is the operator's only local signal
        // for why delivery is not progressing (issue #164).
        let detail = rejection_summary(&receipt);
        let pending = match parsed_report.boot_transition {
            BootTransition::DrainedPrevious => format!(
                "; still retrying drained_previous previous_boot_id={}",
                parsed_report
                    .previous_boot_id
                    .map(|boot| boot.to_string())
                    .unwrap_or_else(|| "unset".to_owned())
            ),
            BootTransition::Closing => "; still retrying closing".to_owned(),
            BootTransition::Continuing | BootTransition::RecoveredAfterStale => String::new(),
        };
        let message = format!("report {report_id} rejected by Server: {detail}{pending}");
        record_delivery_failure_in_transaction(&mut tx, &message, applied_at)
            .await?;
    }
    if parsed_report.boot_transition == BootTransition::DrainedPrevious
        && receipt.disposition != ReceiptDisposition::Rejected
    {
        // The Server applied the transition, so the Boot it announced is
        // settled. Both accepted and partially_accepted prove the Boot was
        // applied; a rejected receipt leaves the Boot DrainedPending so the
        // next report re-carries the transition.
        if crate::boot::settle(
            &mut tx,
            parsed_report.agent_epoch,
            parsed_report.boot_id,
            applied_at,
        )
        .await?
        {
            crate::shutdown_record::settle(&mut tx, applied_at).await?;
        }
    }
    if parsed_report.boot_transition == BootTransition::Closing
        && receipt.disposition != ReceiptDisposition::Rejected
    {
        // Only an applied Closing may close the Boot locally (issue #178). The
        // Server keeps the Boot active when it rejects the Closing, so a local
        // rotation would make every later drained_previous report a permanent
        // conflicting_boot. A rejected Closing stays on the current Boot and is
        // rebuilt and re-sent by the next recovery drain.
        match crate::boot::close(
            &mut tx,
            parsed_report.agent_epoch,
            parsed_report.boot_id,
            parsed_report.report_sequence,
            report_id,
            applied_at,
        )
        .await?
        {
            crate::boot::CloseOutcome::Closed { .. } => {
                crate::shutdown_record::closing_applied(&mut tx, applied_at).await?;
            }
            // The Server applied a Closing of a Boot this Agent has already
            // moved past: the acknowledgement stands and the Report leaves the
            // spool, but nothing is closed again. A race with another writer is
            // not that, and rolls the whole receipt back.
            crate::boot::CloseOutcome::Superseded => superseded_closing = true,
            crate::boot::CloseOutcome::Conflict => {
                return Err(sqlx::Error::Protocol(
                    "Agent state changed while applying Closing receipt".to_owned(),
                ));
            }
        }
    }
        Ok(AppliedReport {
            report_sequence: parsed_report.report_sequence,
            boot_id: parsed_report.boot_id,
            boot_transition: parsed_report.boot_transition,
            superseded_closing,
        })
    }
    .await;
    #[cfg(test)]
    body_reads.record(1);
    match transaction_result {
        Ok(applied) => {
            tx.commit().await?;
            if compact_replay {
                tracing::warn!(
                    report_id,
                    outcome = disposition,
                    "applied the compact receipt of a replayed Report whose receipt body the Server had slimmed; per-Node and per-sample outcomes are no longer available, so a partially accepted Report returns its samples to the queue"
                );
            }
            if applied.superseded_closing {
                tracing::warn!(
                    report_id,
                    "applied the receipt of a Closing whose Boot the Agent had already moved past; the Report left the spool and the Boot stays where it is"
                );
            }
            Ok(applied)
        }
        Err(error) => match tx.rollback_retaining_permit().await {
            Ok(permit) => {
                // Two rollbacks are not retryable. A stored body that no
                // longer matches what was delivered is retained by the
                // rollback, so the delivery loop would recompute the same
                // failure forever (issue #269). A confirmation that
                // contradicts accepted state is a peer protocol violation:
                // the Report and the record are both retained rather than
                // silently dequeuing an acknowledgement. Both fail closed by
                // latching the Store fatal, and they share this one handling
                // so the fatal-persistence semantics cannot diverge.
                let reason = store_integrity_failure.or_else(|| {
                    inventory_acceptance_conflict.map(|conflict| {
                        format!(
                            "report {report_id} receipt conflicts with the local confirmation record ({conflict}); manual investigation required"
                        )
                    })
                });
                match reason {
                    None => Err(error),
                    Some(reason) => {
                        let marked = match store.write_under(permit).await {
                            Ok(tx) => {
                                crate::reporting::commit_store_fatal(tx, applied_at, &reason).await
                            }
                            Err(error) => Err(error.into()),
                        };
                        if let Err(mark_error) = marked {
                            return Err(sqlx::Error::Protocol(format!(
                                "{reason}; additionally failed to mark the store fatal: {mark_error}"
                            )));
                        }
                        Err(sqlx::Error::Protocol(reason))
                    }
                }
            }
            Err(rollback_error) => Err(rollback_error),
        },
    }
}

// ---- Delivery ----

/// One queued Report as claimed for delivery: its identity and exact bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredReport {
    pub(crate) report_id: String,
    pub(crate) report_sequence: u64,
    pub(crate) body: Vec<u8>,
    pub(crate) body_sha256: String,
}

/// Claim exactly one report. An existing in-flight report is resumed first;
/// otherwise the oldest unclaimed report is atomically marked in-flight.
async fn claim_oldest_report(
    store: &mut AgentStore,
) -> Result<Option<StoredReport>, ReportStoreError> {
    #[cfg(test)]
    let body_reads = store.spool_body_read_probe();
    let mut tx = store.write().await?;
    let transaction_result: Result<Option<StoredReport>, ReportStoreError> = async {
        let in_flight: Option<String> = sqlx::query_scalar(
            "SELECT report_id FROM reports WHERE in_flight = 1 ORDER BY created_at, report_id LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let report_id = if let Some(id) = in_flight {
            id
        } else {
            let Some(id) = sqlx::query_scalar::<_, String>("SELECT report_id FROM reports WHERE in_flight = 0 ORDER BY created_at, report_id LIMIT 1")
                .fetch_optional(&mut *tx).await? else {
                return Ok(None);
            };
            sqlx::query("UPDATE reports SET in_flight = 1 WHERE report_id = ? AND in_flight = 0")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            id
        };
        let report = sqlx::query_as::<_, (String, i64, Vec<u8>, String)>("SELECT report_id, report_sequence, body, body_sha256 FROM reports WHERE report_id = ? AND in_flight = 1")
            .bind(&report_id).fetch_optional(&mut *tx).await?;
        report
            .map(
                |(report_id, report_sequence, body, body_sha256)| StoredReport {
                    report_id,
                    report_sequence: report_sequence as u64,
                    body,
                    body_sha256,
                },
            )
            .ok_or(ReportStoreError::DeliveryInFlight)
            .map(Some)
    }
    .await;
    match transaction_result {
        Ok(report) => {
            tx.commit().await?;
            #[cfg(test)]
            if report.is_some() {
                body_reads.record(1);
            }
            Ok(report)
        }
        Err(error) => match tx.rollback().await {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(ReportStoreError::Database(rollback_error)),
        },
    }
}

/// What sending the oldest queued Report did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Sent {
    /// No Report is queued.
    Empty,
    /// The Server's receipt was applied and the Report left the spool. A
    /// whole-report rejection is an applied receipt too: it reaches the caller
    /// as data, not as an error.
    Applied(Delivered),
    /// The Server acknowledged a Closing whose Boot the Agent had already moved
    /// past. Its receipt was applied and the Report left the spool, but the
    /// Boot it named was not closed again.
    Quarantined { report_id: String },
}

/// One Report whose receipt was applied, described from the stored Report the
/// receipt transaction validated, so no caller re-parses the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Delivered {
    pub(crate) report_id: String,
    pub(crate) report_sequence: u64,
    pub(crate) boot_id: BootId,
    pub(crate) boot_transition: BootTransition,
    pub(crate) disposition: ReceiptDisposition,
    /// The Server's Inventory outcome, which the upgrade-preparation bridge
    /// needs to prove its Closing's declaration was accepted.
    pub(crate) inventory: Option<InventoryDisposition>,
    /// The operator-facing summary when the Server refused the Report whole.
    pub(crate) rejection: Option<String>,
}

impl Delivered {
    /// Whether this receipt proves the Server applied the Closing of `boot_id`.
    /// A rejected Closing is an applied receipt, not a closed Boot: the Server
    /// keeps that Boot active.
    pub(crate) fn closes_boot(&self, boot_id: BootId) -> bool {
        self.boot_id == boot_id
            && self.boot_transition == BootTransition::Closing
            && self.disposition != ReceiptDisposition::Rejected
    }
}

/// Send the oldest queued Report and apply the Server's receipt for it.
///
/// An in-flight Report is resumed before a new one is claimed, so the exact
/// bytes of an unacknowledged send are what goes out again. No transport error
/// is an acknowledgement: the Report stays queued and the failure is recorded
/// as the Agent's delivery diagnostic.
pub(crate) async fn send_oldest<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    send_budget: SendBudget,
) -> Result<Sent, ReportStoreError> {
    crate::reporting::ensure_spool_not_fatal(store).await?;
    let Some(report) = claim_oldest_report(store).await? else {
        return Ok(Sent::Empty);
    };
    crate::reporting::ensure_claimed_report_intact(store, &report).await?;
    // The per-send budget starts here, with the request, not with the Durable
    // Spool bookkeeping above (issue #270).
    let send_deadline = match send_budget {
        SendBudget::PerSend(timeout) => Some(tokio::time::Instant::now() + timeout),
        SendBudget::At(deadline) => Some(deadline),
        SendBudget::Unbounded => None,
    };
    let response = match send_deadline {
        Some(deadline) => match tokio::time::timeout_at(deadline, transport.send(&report.body))
            .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                let _ = record_delivery_failure(store, &error.to_string(), &now_rfc3339()).await;
                return Err(error);
            }
            Err(_) => {
                let _ = record_delivery_failure(
                    store,
                    "delivery sender deadline exhausted",
                    &now_rfc3339(),
                )
                .await;
                return Err(ReportStoreError::DeliveryDeadline);
            }
        },
        None => match transport.send(&report.body).await {
            Ok(response) => response,
            Err(error) => {
                let _ = record_delivery_failure(store, &error.to_string(), &now_rfc3339()).await;
                return Err(error);
            }
        },
    };
    let major = report_protocol_major(&report.body);
    let (envelope_receipt, acceptance) = if major == platpulse_core::protocol::PROTOCOL_VERSION_V2 {
        let envelope: WireReportResponseV2 = serde_json::from_slice(&response)
            .map_err(|error| ReportStoreError::InvalidReceipt(error.to_string()))?;
        envelope
            .receipt
            .validate()
            .map_err(|error| ReportStoreError::InvalidReceipt(error.to_string()))?;
        let acceptance = envelope
            .receipt
            .inventory
            .as_ref()
            .and_then(|inventory| inventory.acceptance.clone());
        (normalize_v2_receipt(envelope.receipt), acceptance)
    } else {
        let envelope: WireReportResponse = serde_json::from_slice(&response)
            .map_err(|error| ReportStoreError::InvalidReceipt(error.to_string()))?;
        envelope
            .receipt
            .validate()
            .map_err(|error| ReportStoreError::InvalidReceipt(error.to_string()))?;
        (envelope.receipt, None)
    };
    let actual_hash = format!("0x{}", hex::encode(Sha256::digest(&report.body)));
    if actual_hash != report.body_sha256 {
        return Err(ReportStoreError::ReceiptMismatch);
    }
    if envelope_receipt.report_id.to_string() != report.report_id
        || envelope_receipt.report_body_sha256.to_string() != actual_hash
    {
        return Err(ReportStoreError::ReceiptMismatch);
    }
    let receipt_disposition = envelope_receipt.disposition;
    // Copy the Inventory outcome before the receipt is consumed by the
    // receipt-application transaction below.
    let inventory_disposition = envelope_receipt.inventory;
    let disposition = crate::spool::receipt_disposition_name(receipt_disposition);
    // A whole-report rejection is an applied receipt, not a transport failure:
    // it must reach the caller as data so the delivery loop can tell the
    // operator why every report is being refused (issue #181).
    // The summary names the condition, not the Report: the delivery loop
    // aggregates repeated refusals on it (design logging.md), and a catch-up
    // that refuses thousands of Reports must produce one line per minute rather
    // than one per Report. The Report's identity travels beside it.
    let rejection = (receipt_disposition == ReceiptDisposition::Rejected)
        .then(|| rejection_summary(&envelope_receipt));
    let applied_at = now_rfc3339();
    let applied = if major == platpulse_core::protocol::PROTOCOL_VERSION_V2 {
        apply_receipt_typed::<InventoryDeclaration>(
            store,
            &report.report_id,
            &report.body_sha256,
            disposition,
            envelope_receipt,
            acceptance,
            &applied_at,
        )
        .await?
    } else {
        apply_receipt_typed::<NodeInventory>(
            store,
            &report.report_id,
            &report.body_sha256,
            disposition,
            envelope_receipt,
            acceptance,
            &applied_at,
        )
        .await?
    };
    if applied.superseded_closing {
        return Ok(Sent::Quarantined {
            report_id: report.report_id,
        });
    }
    Ok(Sent::Applied(Delivered {
        report_id: report.report_id,
        report_sequence: applied.report_sequence,
        boot_id: applied.boot_id,
        boot_transition: applied.boot_transition,
        disposition: receipt_disposition,
        inventory: inventory_disposition,
        rejection,
    }))
}

/// Send the oldest queued Report without a deadline and return what was
/// applied, for tests that only care whether a Report left the spool.
#[cfg(test)]
pub(crate) async fn send_oldest_unbounded<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
) -> Result<Option<Delivered>, ReportStoreError> {
    match send_oldest(store, transport, SendBudget::Unbounded).await? {
        Sent::Applied(delivered) => Ok(Some(delivered)),
        Sent::Empty => Ok(None),
        Sent::Quarantined { report_id } => {
            panic!("report {report_id} was a superseded Closing, not an applied Report")
        }
    }
}

/// What bounds one HTTP send (issue #270).
///
/// The Durable Spool work that precedes a send — taking the shared write permit,
/// claiming the oldest Report, re-validating its bytes — is deliberately outside
/// the budget. `sender_deadline_ms` is defined as the time the Agent waits for
/// the Server's response (design §7.2), so local bookkeeping must never be able
/// to consume it and expire it before the request leaves the Agent.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SendBudget {
    /// No bound: wait for the Server's response. Boot recovery drains this way.
    Unbounded,
    /// Abandon the send once this much time has passed since the request
    /// started. The Durable Spool delivery pass renews it for every Report.
    PerSend(std::time::Duration),
    /// Abandon the send at a moment the caller already chose: shutdown and
    /// upgrade preparation bound a whole step, which starts before the request.
    At(tokio::time::Instant),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReportResponse {
    receipt: ReportReceipt,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReportResponseV2 {
    receipt: ReportReceiptV2,
}

/// The common v2 receipt fields in the frozen v1 shape. The v2 Inventory
/// acceptance is carried separately so the confirmation record can bind the
/// Server-assigned revision and canonical fingerprint.
fn normalize_v2_receipt(receipt: ReportReceiptV2) -> ReportReceipt {
    ReportReceipt {
        report_id: receipt.report_id,
        disposition: receipt.disposition,
        report_body_sha256: receipt.report_body_sha256,
        server_version: receipt.server_version,
        supported_protocol_majors: receipt.supported_protocol_majors,
        server_time: receipt.server_time,
        rotation_hint: receipt.rotation_hint,
        inventory: receipt.inventory.map(|inventory| inventory.disposition),
        rejections: receipt.rejections,
        nodes: receipt.nodes,
        samples: receipt.samples,
    }
}

/// The operator-facing summary of a whole-report rejection: every rejection the
/// Server returned, in one bounded line. Shared by the durable Agent diagnostic
/// (which the Agent Store keeps) and the delivery loop's console print, so both
/// surfaces always show the same codes and reasons (issue #181).
pub(crate) fn rejection_summary(receipt: &ReportReceipt) -> String {
    receipt
        .rejections
        .iter()
        .map(|rejection| format!("{} ({})", rejection.code.as_str(), rejection.reason))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Record a bounded delivery failure inside an existing transaction, so a
/// receipt rejection can share the same diagnostic write as a transport
/// failure without leaving the caller's transaction.
pub(crate) async fn record_delivery_failure_in_transaction(
    tx: &mut WriteTx<'_>,
    message: &str,
    at: &str,
) -> Result<(), sqlx::Error> {
    let message: String = message.chars().take(256).collect();
    sqlx::query("INSERT INTO delivery_diagnostics (singleton, last_error, last_error_at) VALUES (1, ?, ?) ON CONFLICT(singleton) DO UPDATE SET last_error=excluded.last_error, last_error_at=excluded.last_error_at")
        .bind(message).bind(at).execute(&mut **tx).await?;
    Ok(())
}

/// Record a bounded delivery failure while preserving the immutable report.
pub(crate) async fn record_delivery_failure(
    store: &mut AgentStore,
    message: &str,
    at: &str,
) -> Result<(), ReportStoreError> {
    let mut tx = store.write().await?;
    match record_delivery_failure_in_transaction(&mut tx, message, at).await {
        Ok(()) => tx.commit().await.map_err(ReportStoreError::from),
        Err(error) => match tx.rollback().await {
            Ok(()) => Err(ReportStoreError::from(error)),
            Err(rollback_error) => Err(ReportStoreError::from(rollback_error)),
        },
    }
}

/// Remove an orphan report that can never apply to the current Agent state.
/// The Server has already acknowledged its exact bytes, so the local row is
/// dropped; `report_sample_assignments` cascades and its samples become
/// re-assignable.
pub(crate) async fn quarantine_report(
    store: &mut AgentStore,
    report_id: &str,
) -> Result<bool, ReportStoreError> {
    let mut tx = store.write().await?;
    let result = sqlx::query("DELETE FROM reports WHERE report_id = ?")
        .bind(report_id)
        .execute(&mut *tx)
        .await;
    match result {
        Ok(result) => {
            tx.commit().await?;
            Ok(result.rows_affected() == 1)
        }
        Err(error) => {
            tx.rollback().await?;
            Err(ReportStoreError::Database(error))
        }
    }
}

fn sample_reference(sample: SampleRef) -> (&'static str, u64, u64) {
    match sample {
        SampleRef::Block { height } => ("block", height, height),
        SampleRef::Gap {
            from_height,
            to_height,
        } => ("gap", from_height, to_height),
    }
}

pub(crate) fn receipt_disposition_name(disposition: ReceiptDisposition) -> &'static str {
    match disposition {
        ReceiptDisposition::Accepted => "accepted",
        ReceiptDisposition::PartiallyAccepted => "partially_accepted",
        ReceiptDisposition::Rejected => "rejected",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn delivered(boot_transition: BootTransition, disposition: ReceiptDisposition) -> Delivered {
        Delivered {
            report_id: "0195f2a1-0051-4051-8051-000000000051".to_owned(),
            report_sequence: 3,
            boot_id: BootId::from_str("0195f2a1-0050-4050-8050-000000000050").unwrap(),
            boot_transition,
            disposition,
            inventory: None,
            rejection: None,
        }
    }

    #[test]
    fn only_an_applied_closing_of_the_same_boot_closes_it() {
        let boot = BootId::from_str("0195f2a1-0050-4050-8050-000000000050").unwrap();
        let other = BootId::from_str("0195f2a1-0099-4099-8099-000000000099").unwrap();

        assert!(delivered(BootTransition::Closing, ReceiptDisposition::Accepted).closes_boot(boot));
        assert!(
            delivered(
                BootTransition::Closing,
                ReceiptDisposition::PartiallyAccepted
            )
            .closes_boot(boot)
        );
        assert!(
            !delivered(BootTransition::Closing, ReceiptDisposition::Rejected).closes_boot(boot),
            "a rejected Closing receipt must not close the Boot"
        );
        assert!(
            !delivered(BootTransition::Closing, ReceiptDisposition::Accepted).closes_boot(other)
        );
        assert!(
            !delivered(BootTransition::Continuing, ReceiptDisposition::Accepted).closes_boot(boot)
        );
    }
}
