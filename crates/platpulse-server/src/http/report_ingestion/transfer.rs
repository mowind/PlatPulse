//! Node transfer resolution when an Agent declares a Node it does not yet own.

use sqlx::{Any, Transaction};

use platpulse_core::component::ComponentStatus;
use platpulse_core::{AgentReport, ReportInventory};

/// Outcome of evaluating one Agent's Inventory declaration of a Node it does
/// not yet own (design §4.6). A declaration only wins ownership when a
/// pending transfer targets the reporting Agent, the declared Network key
/// matches the registered Node, and the observed Network Identity matches
/// the Registry tuple; everything else keeps the source Agent authoritative.
#[derive(Debug)]
pub(super) enum TransferResolution {
    /// No pending transfer covers this declaration; ownership stays put.
    NoTransfer,
    /// The declaration completed the pending transfer; the reporting Agent
    /// owns the Node from this transaction on.
    Completed,
    /// A pending transfer exists but the report carries no usable identity
    /// observation yet; the transfer stays pending and ownership stays put.
    Unverified,
    /// The declared Network key differs from the registered Network; the
    /// transfer is terminally rejected and ownership stays put.
    NetworkKeyMismatch,
    /// The observed identity contradicts the Registry tuple; the transfer
    /// ends in a typed `identity_mismatch` outcome and ownership stays put.
    IdentityMismatch,
}

/// Evaluate a non-owner Inventory declaration against the pending transfer
/// state, inside the ingestion transaction. A successful resolution switches
/// `nodes.agent_id` in the same transaction that accepts the report, so the
/// source stays authoritative until the switch is atomic (issue #46).
pub(super) async fn resolve_node_transfer<I: ReportInventory>(
    tx: &mut Transaction<'_, Any>,
    report: &AgentReport<I>,
    node: &platpulse_core::inventory::InventoryNode,
    reporting_agent: &str,
    now_text: &str,
) -> Result<TransferResolution, sqlx::Error> {
    // Materialize transfers that expired before this declaration so the
    // history never shows a stale pending row for an expired handover.
    let expired = sqlx::query(
        "UPDATE node_transfers SET status='expired', updated_at=$1 WHERE node_id=$2 AND target_agent_id=$3 AND status='pending' AND expires_at <= $4",
    )
    .bind(now_text)
    .bind(node.node_id.to_string())
    .bind(reporting_agent)
    .bind(now_text)
    .execute(&mut **tx)
    .await?;
    if expired.rows_affected() > 0 {
        crate::auth::insert_audit_event(
            &mut **tx,
            None,
            "node_transfer_expired",
            "node",
            &node.node_id.to_string(),
            Some(&serde_json::json!({ "expired_at": now_text })),
        )
        .await?;
    }
    let pending = sqlx::query_as::<_, (String, String)>(
        "SELECT transfer_id, source_agent_id FROM node_transfers WHERE node_id=$1 AND target_agent_id=$2 AND status='pending' AND expires_at > $3 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(node.node_id.to_string())
    .bind(reporting_agent)
    .bind(now_text)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((transfer_id, source_agent_id)) = pending else {
        return Ok(TransferResolution::NoTransfer);
    };
    let registered = sqlx::query_as::<_, (String, String, i64, i64, String)>(
        "SELECT n.network_key, r.genesis_hash, r.chain_id, r.p2p_network_id, r.address_hrp FROM nodes n JOIN networks r ON r.network_key=n.network_key WHERE n.node_id=$1",
    )
    .bind(node.node_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    let Some((registered_key, genesis, chain_id, p2p, hrp)) = registered else {
        return Ok(TransferResolution::NoTransfer);
    };
    if node.network_key.as_str() != registered_key {
        sqlx::query(
            "UPDATE node_transfers SET status='rejected', rejection_code='network_key_mismatch', rejection_reason=$1, updated_at=$2 WHERE transfer_id=$3",
        )
        .bind("target declared the Node under a different Network key than the registered Network")
        .bind(now_text)
        .bind(&transfer_id)
        .execute(&mut **tx)
        .await?;
        crate::auth::insert_audit_event(
            &mut **tx,
            None,
            "node_transfer_rejected",
            "node",
            &node.node_id.to_string(),
            Some(&serde_json::json!({
                "transfer_id": transfer_id,
                "rejection_code": "network_key_mismatch",
            })),
        )
        .await?;
        return Ok(TransferResolution::NetworkKeyMismatch);
    }
    // The identity authority is the report's own Node observation: a
    // declaration without a successful identity probe cannot switch
    // ownership (source remains authoritative until validation passes).
    let identity = report
        .nodes
        .iter()
        .find(|observation| observation.node_id == node.node_id)
        .filter(|observation| observation.chain.network_identity.status == ComponentStatus::Ok)
        .and_then(|observation| observation.chain.network_identity.latest.as_ref());
    let Some(identity) = identity else {
        return Ok(TransferResolution::Unverified);
    };
    let mut mismatched_fields = Vec::new();
    if identity.genesis_hash.to_string() != genesis {
        mismatched_fields.push("genesis_hash");
    }
    if identity.chain_id != chain_id as u64 {
        mismatched_fields.push("chain_id");
    }
    if identity.p2p_network_id != p2p as u64 {
        mismatched_fields.push("p2p_network_id");
    }
    if identity.address_hrp.as_deref().unwrap_or("") != hrp {
        mismatched_fields.push("address_hrp");
    }
    if !mismatched_fields.is_empty() {
        sqlx::query(
            "UPDATE node_transfers SET status='identity_mismatch', rejection_code='identity_mismatch', rejection_reason=$1, mismatched_fields=$2, updated_at=$3 WHERE transfer_id=$4",
        )
        .bind("the target-declared Network identity contradicts the registered Network; ownership stays with the source Agent")
        .bind(serde_json::to_string(&mismatched_fields).expect("fields serialize"))
        .bind(now_text)
        .bind(&transfer_id)
        .execute(&mut **tx)
        .await?;
        crate::auth::insert_audit_event(
            &mut **tx,
            None,
            "node_transfer_identity_mismatch",
            "node",
            &node.node_id.to_string(),
            Some(&serde_json::json!({
                "transfer_id": transfer_id,
                "mismatched_fields": mismatched_fields,
            })),
        )
        .await?;
        return Ok(TransferResolution::IdentityMismatch);
    }
    sqlx::query(
        "UPDATE node_transfers SET status='completed', completed_at=$1, updated_at=$2 WHERE transfer_id=$3",
    )
    .bind(now_text)
    .bind(now_text)
    .bind(&transfer_id)
    .execute(&mut **tx)
    .await?;
    // Ownership lives in `nodes.agent_id` AND in the composite FK
    // `component_status(agent_id, node_id)` rows of the same Node. Both
    // sides must move in this transaction; FK enforcement is deferred to
    // the commit so the parent and child rows switch atomically (and the
    // pragma resets automatically when the transaction ends).
    crate::database::defer_foreign_keys(tx).await?;
    sqlx::query("UPDATE nodes SET agent_id=$1, updated_at=$2 WHERE node_id=$3")
        .bind(reporting_agent)
        .bind(now_text)
        .bind(node.node_id.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE component_status SET agent_id=$1 WHERE node_id=$2")
        .bind(reporting_agent)
        .bind(node.node_id.to_string())
        .execute(&mut **tx)
        .await?;
    // The switch is a recorded relationship change (issue #221): the previous
    // Agent's interval closes at this instant and the reporting Agent's opens.
    // The Network is passed as the registered key, which this path has already
    // verified, so a transfer never records a Network change.
    crate::relationships::record_relations(
        tx,
        &node.node_id.to_string(),
        reporting_agent,
        &registered_key,
        now_text,
    )
    .await?;
    crate::auth::insert_audit_event(
        &mut **tx,
        None,
        "node_transfer_completed",
        "node",
        &node.node_id.to_string(),
        Some(&serde_json::json!({
            "transfer_id": transfer_id,
            "source_agent_id": source_agent_id,
            "target_agent_id": reporting_agent,
        })),
    )
    .await?;
    Ok(TransferResolution::Completed)
}

#[cfg(test)]
mod tests {

    use crate::http::report_ingestion::test_helpers::*;

    use tempfile::TempDir;

    use crate::auth::{format_rfc3339, now_utc};

    use crate::http::AppState;

    use crate::network::create_network;

    use platpulse_core::component::ComponentStatus;

    use platpulse_core::{
        AgentReport, NodeCurrentDisposition, ReceiptDisposition, SampleDispositionKind,
    };

    /// Source Agent owns the fixture Node; a second Agent exists as a
    /// transfer target. The source report deliberately keeps the fixture
    /// identity (which mismatches the Registry) so identity assertions in
    /// transfer tests are deterministic.
    pub(super) async fn state_with_source_and_target() -> (TempDir, AppState, String, String, String)
    {
        let (dir, state, source_agent) = state_with_agent().await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(
            &state,
            &source_agent,
            serde_json::to_vec(&source_report).unwrap(),
        )
        .await;
        let target_agent = "0195f2a1-0019-4019-8019-000000000019".to_owned();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES ($1, 1, NULL, NULL, NULL, $2, $3)")
            .bind(&target_agent)
            .bind("2026-08-12T08:00:00Z")
            .bind("2026-08-12T08:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        (
            dir,
            state,
            source_agent,
            target_agent,
            source_report.inventory.nodes[0].node_id.to_string(),
        )
    }

    /// Build the target Agent's declaration of the source-owned Node. The
    /// report identity matches the registered Registry tuple when
    /// `matching_identity` is true (genesis `0x…01`, chain/p2p 210425,
    /// hrp `lat`); otherwise the fixture's contradictory identity is kept.
    pub(super) fn target_declaration(
        source_report: &AgentReport,
        target_agent: &str,
        matching_identity: bool,
        with_sample: bool,
    ) -> AgentReport {
        let mut report = source_report.clone();
        report.agent_id = target_agent.parse().unwrap();
        report.boot_id = "0195f2a1-0034-4034-8034-000000000034".parse().unwrap();
        report.previous_boot_id = None;
        report.report_sequence = 1;
        report.report_id = "0195f2a1-0035-4035-8035-000000000035".parse().unwrap();
        if matching_identity {
            let identity = report.nodes[0]
                .chain
                .network_identity
                .latest
                .as_mut()
                .unwrap();
            identity.genesis_hash =
                "0x0000000000000000000000000000000000000000000000000000000000000001"
                    .parse()
                    .unwrap();
            identity.address_hrp = Some("lat".to_owned());
        }
        if with_sample {
            let identity = report.nodes[0]
                .chain
                .network_identity
                .latest
                .clone()
                .unwrap();
            report
                .block_summaries
                .push(platpulse_core::block::BlockSummary {
                    node_id: report.inventory.nodes[0].node_id,
                    network_identity: identity,
                    block_number: 10,
                    block_hash:
                        "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .parse()
                            .unwrap(),
                    parent_hash:
                        "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                            .parse()
                            .unwrap(),
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

    pub(super) async fn pending_transfer(
        state: &AppState,
        node_id: &str,
        source: &str,
        target: &str,
    ) {
        // Windows are relative to the real clock so the fixture never
        // silently expires mid-suite.
        let now = now_utc();
        let created = format_rfc3339(now - time::Duration::hours(2));
        let expires = format_rfc3339(now + time::Duration::hours(2));
        sqlx::query(
            "INSERT INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, operator_reason, created_at, expires_at, updated_at) VALUES ('transfer-pending-1', $1, $2, $3, 'pending', 'move the validator', $4, $5, $6)",
        )
        .bind(node_id.to_string())
        .bind(source)
        .bind(target)
        .bind(&created)
        .bind(&expires)
        .bind(&created)
        .execute(state.db().pool())
        .await
        .unwrap();
    }

    #[tokio::test]
    pub(super) async fn matching_declaration_completes_transfer_atomically_and_source_loses_ownership()
     {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let declaration = target_declaration(&source_report, &target_agent, true, true);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Accepted);
        assert_eq!(
            receipt.samples[0].disposition,
            SampleDispositionKind::Accepted
        );

        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, target_agent);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "completed");
        let completed: Option<String> = sqlx::query_scalar(
            "SELECT completed_at FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(completed.is_some());
        // The same transaction merged the matching sample into history.
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 1);
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_completed'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
        // The target's security counter stays clean: this was a valid
        // declaration, not a contradiction.
        let target_events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(target_events, 0);

        // The source Agent's later submission of the same Node only rejects
        // that Node entry and records a security event; ownership stays with
        // the target (design §4.6).
        let mut stale_source = source_report.clone();
        stale_source.report_sequence = 2;
        stale_source.report_id = "0195f2a1-0013-4013-8013-000000000301".parse().unwrap();
        let source_receipt = submit(
            &state,
            &source_agent,
            serde_json::to_vec(&stale_source).unwrap(),
        )
        .await;
        assert_eq!(
            source_receipt.disposition,
            ReceiptDisposition::PartiallyAccepted
        );
        assert_eq!(
            source_receipt.nodes[0].current,
            NodeCurrentDisposition::Rejected
        );
        assert_eq!(
            source_receipt.nodes[0].rejections[0].code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        let owner_after: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner_after, target_agent);
        let source_events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&source_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(source_events, 1);
    }

    #[tokio::test]
    pub(super) async fn identity_mismatch_blocks_transfer_and_never_merges_history() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        // The fixture identity (0xaaa…, no hrp) contradicts the Registry.
        let declaration = target_declaration(&source_report, &target_agent, false, true);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        assert_eq!(
            receipt.nodes[0].rejections[0].code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "identity_mismatch");
        let fields: String = sqlx::query_scalar(
            "SELECT mismatched_fields FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&fields).unwrap(),
            vec!["genesis_hash", "address_hrp"]
        );
        // Ownership never switched and no history merged.
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 0);
        // The History Gap the Server leaves for the refused height states the
        // reason the Receipt gave the Agent.
        let gap_reason: String = sqlx::query_scalar(
            "SELECT reason FROM block_history_gaps WHERE node_id=$1 AND kind='server_rejected' AND authored_by='server'",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            gap_reason,
            receipt.samples[0].rejection.as_ref().unwrap().reason
        );
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_identity_mismatch'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
        let events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(events, 1);
    }

    #[tokio::test]
    pub(super) async fn declaration_without_identity_probe_keeps_transfer_pending() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let mut declaration = target_declaration(&source_report, &target_agent, true, false);
        declaration.nodes[0].chain.network_identity.status = ComponentStatus::Error;
        declaration.nodes[0].chain.network_identity.latest = None;
        declaration.nodes[0]
            .chain
            .network_identity
            .latest_observed_at = None;
        declaration.nodes[0].chain.network_identity.error =
            Some(platpulse_core::component::BoundedError {
                code: "rpc_unreachable".into(),
                message: "identity probe failed".into(),
            });
        declaration.validate().unwrap();
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "pending");
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        // An in-flight declaration without a contradiction is not a
        // security event.
        let events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(events, 0);
    }

    /// A block sample whose stamp contradicts the Registry is a security
    /// event even while its Node is refused for its owner and the transfer
    /// stays pending: the refusal does not hide the contradiction.
    #[tokio::test]
    pub(super) async fn a_contradicted_stamp_of_a_refused_node_is_still_a_security_event() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        // The sample keeps the fixture stamp (0xaaa…, no hrp), which
        // contradicts the Registry; the identity probe itself failed, so the
        // transfer cannot be resolved by this declaration.
        let mut declaration = target_declaration(&source_report, &target_agent, false, true);
        declaration.nodes[0].chain.network_identity.status = ComponentStatus::Error;
        declaration.nodes[0].chain.network_identity.latest = None;
        declaration.nodes[0]
            .chain
            .network_identity
            .latest_observed_at = None;
        declaration.nodes[0].chain.network_identity.error =
            Some(platpulse_core::component::BoundedError {
                code: "rpc_unreachable".into(),
                message: "identity probe failed".into(),
            });
        declaration.validate().unwrap();
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "pending");
        let events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(events, 1);
    }

    #[tokio::test]
    pub(super) async fn expired_transfer_is_materialized_and_declaration_stays_rejected() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        sqlx::query(
            "INSERT INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, created_at, expires_at, updated_at) VALUES ('transfer-expired-1', $1, $2, $3, 'pending', '2026-08-10T08:00:00Z', '2026-08-11T08:00:00Z', '2026-08-10T08:00:00Z')",
        )
        .bind(node_id.to_string())
        .bind(&source_agent)
        .bind(&target_agent)
        .execute(state.db().pool())
        .await
        .unwrap();
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let declaration = target_declaration(&source_report, &target_agent, true, false);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-expired-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "expired");
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_expired'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
    }

    #[tokio::test]
    pub(super) async fn declaration_under_different_network_key_rejects_transfer() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        create_network(
            state.db(),
            "platon-testnet",
            "PlatON Testnet",
            "0x0000000000000000000000000000000000000000000000000000000000000002",
            210426,
            210426,
            "lat",
        )
        .await
        .unwrap();
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let mut declaration = target_declaration(&source_report, &target_agent, true, false);
        declaration.inventory.nodes[0].network_key = "platon-testnet".parse().unwrap();
        declaration.validate().unwrap();
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "rejected");
        let code: String = sqlx::query_scalar(
            "SELECT rejection_code FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(code, "network_key_mismatch");
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
    }

    /// A whole-report rejection writes nothing but its receipt: when a later
    /// Node of the report names an unregistered Network key, an earlier
    /// Node's pending Transfer is neither completed nor resolved, and the
    /// source keeps ownership.
    #[tokio::test]
    pub(super) async fn unknown_network_key_on_a_sibling_leaves_the_transfer_pending() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let mut declaration = target_declaration(&source_report, &target_agent, true, false);
        let mut sibling = declaration.inventory.nodes[0].clone();
        sibling.node_id = "0195f2a1-0036-4036-8036-000000000036".parse().unwrap();
        sibling.network_key = "platon-unregistered".parse().unwrap();
        sibling.rpc_endpoint = "ws://127.0.0.1:6791".parse().unwrap();
        let mut sibling_observation = declaration.nodes[0].clone();
        sibling_observation.node_id = sibling.node_id;
        declaration.inventory.nodes.push(sibling);
        declaration.nodes.push(sibling_observation);
        declaration.validate().unwrap();

        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            receipt.rejections[0].code,
            platpulse_core::RejectionCode::NetworkKeyUnknown
        );
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "pending");
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_completed'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 0);
    }

    #[tokio::test]
    pub(super) async fn matching_sample_never_merges_history_for_ownership_rejected_node() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        // The observation contradicts the Registry (fixture identity), but
        // the block sample claims the registered tuple: the declaration must
        // still be rejected on the observation, and the matching sample must
        // never merge into the registered Network history.
        let mut declaration = target_declaration(&source_report, &target_agent, false, true);
        let mut sample = declaration.block_summaries[0].clone();
        sample.network_identity.genesis_hash =
            "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap();
        sample.network_identity.address_hrp = Some("lat".to_owned());
        declaration.block_summaries[0] = sample;
        declaration.validate().unwrap();
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "identity_mismatch");
        // Neither the identity window nor the summaries nor the counters
        // were touched by the matching sample.
        let window: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_identity_window WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(window, 0);
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 0);
        let counts: Option<(i64, i64)> = sqlx::query_as(
            "SELECT cumulative_block_count, cumulative_transaction_count FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_optional(state.db().pool())
        .await
        .unwrap();
        // The row exists only from the source's earlier report; the
        // rejected declaration never incremented the counters.
        assert_eq!(counts, Some((0, 0)));
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
    }

    /// Two Agents first declaring one Node at the same instant must leave it
    /// with exactly one owner whose projection it carries (issue #280). The
    /// per-Agent fence serializes one Agent only; on the PostgreSQL work
    /// connections both receipts once read "no owner", and the second upsert
    /// kept the first Agent as owner while overwriting the projection with the
    /// second Agent's declaration — a takeover without a Transfer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    pub(super) async fn two_agents_first_declaring_one_node_together_leave_exactly_one_owner() {
        for _round in 0..6 {
            let (_dir, state, first_agent) = state_with_agent().await;
            let second_agent = "0195f2a1-0019-4019-8019-000000000019".to_owned();
            sqlx::query("INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES ($1, 1, NULL, NULL, NULL, $2, $3)")
                .bind(&second_agent)
                .bind("2026-08-12T08:00:00Z")
                .bind("2026-08-12T08:00:00Z")
                .execute(state.db().pool())
                .await
                .unwrap();
            let first_report: AgentReport = serde_json::from_slice(include_bytes!(
                "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
            ))
            .unwrap();
            let node_id = first_report.inventory.nodes[0].node_id.to_string();
            let mut second_report = target_declaration(&first_report, &second_agent, true, false);
            second_report.inventory.nodes[0].rpc_endpoint = "ws://127.0.0.1:6791".parse().unwrap();
            second_report.validate().unwrap();
            let endpoints = [
                (
                    first_agent.clone(),
                    first_report.inventory.nodes[0].rpc_endpoint.to_string(),
                ),
                (second_agent.clone(), "ws://127.0.0.1:6791".to_owned()),
            ];

            let first = tokio::spawn({
                let state = state.clone();
                let agent = first_agent.clone();
                let body = serde_json::to_vec(&first_report).unwrap();
                async move { submit(&state, &agent, body).await }
            });
            let second = tokio::spawn({
                let state = state.clone();
                let agent = second_agent.clone();
                let body = serde_json::to_vec(&second_report).unwrap();
                async move { submit(&state, &agent, body).await }
            });
            let receipts = [first.await.unwrap(), second.await.unwrap()];

            let admitted = receipts
                .iter()
                .filter(|receipt| receipt.nodes[0].current == NodeCurrentDisposition::Accepted)
                .count();
            assert_eq!(
                admitted, 1,
                "exactly one Agent's declaration admits the Node"
            );
            let refused = receipts
                .iter()
                .find(|receipt| receipt.nodes[0].current == NodeCurrentDisposition::Rejected)
                .expect("the other declaration is refused");
            assert_eq!(
                refused.nodes[0].rejections[0].code,
                platpulse_core::RejectionCode::NodeOwnershipMismatch
            );

            let (owner, endpoint): (String, String) =
                sqlx::query_as("SELECT agent_id, rpc_endpoint FROM nodes WHERE node_id=$1")
                    .bind(&node_id)
                    .fetch_one(state.db().pool())
                    .await
                    .unwrap();
            let owned_endpoint = endpoints
                .iter()
                .find(|(agent, _)| *agent == owner)
                .map(|(_, endpoint)| endpoint.clone())
                .unwrap();
            assert_eq!(
                endpoint, owned_endpoint,
                "the Node projection is the owning Agent's declaration"
            );
            let agent_relations: Vec<String> = sqlx::query_scalar(
                "SELECT related_key FROM node_relationship_intervals WHERE node_id=$1 AND relation_kind='agent'",
            )
            .bind(&node_id)
            .fetch_all(state.db().pool())
            .await
            .unwrap();
            assert_eq!(
                agent_relations,
                vec![owner.clone()],
                "only the owner holds an Agent relation"
            );
        }
    }

    #[tokio::test]
    pub(super) async fn declaration_without_pending_transfer_is_an_ownership_conflict() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let declaration = target_declaration(&source_report, &target_agent, true, false);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        let events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(events, 1);
    }
}
