//! How Report ingestion carries Block history admission into the Receipt and
//! the metrics. What admission decides is tested on `crate::block_history::admit`
//! itself.

use crate::http::report_ingestion::test_helpers::*;

use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
use platpulse_core::{AgentReport, ReceiptDisposition, RejectionCode, SampleDispositionKind};

/// A report carrying the given Block Summaries for the fixture's Node, each
/// stamped with the registered Network Identity: (height, block timestamp in
/// ms, observed_at, transactions, source).
fn report_with_blocks(
    report_id: &str,
    sequence: u64,
    blocks: &[(u64, u64, String, u64, platpulse_core::block::BlockSource)],
) -> AgentReport {
    let mut report: AgentReport = serde_json::from_slice(include_bytes!(
        "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    report.report_id = report_id.parse().unwrap();
    report.report_sequence = sequence;
    let node_id = report.inventory.nodes[0].node_id;
    let mut identity = report.nodes[0]
        .chain
        .network_identity
        .latest
        .clone()
        .unwrap();
    identity.genesis_hash = "0x0000000000000000000000000000000000000000000000000000000000000001"
        .parse()
        .unwrap();
    identity.address_hrp = Some("lat".to_owned());
    for (height, timestamp_ms, observed_at, transactions, source) in blocks.iter().cloned() {
        report
            .block_summaries
            .push(platpulse_core::block::BlockSummary {
                node_id,
                network_identity: identity.clone(),
                block_number: height,
                block_hash: format!("0x{height:064x}").parse().unwrap(),
                parent_hash: format!("0x{:064x}", height.saturating_sub(1))
                    .parse()
                    .unwrap(),
                block_timestamp_ms: timestamp_ms,
                observed_at: observed_at.parse().unwrap(),
                transaction_count: transactions,
                block_interval_ms: None,
                source,
                attribution: platpulse_core::block::BlockProductionAttribution::unknown_attribution(
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

/// Every Block Summary outcome reaches the Receipt as its sample disposition,
/// in Report order: an accepted sample is accepted, and a chain divergence, a
/// resync replay and a backfill outside any open gap are terminal. The
/// History Gap settlement the Report ran is counted on the report path.
#[tokio::test]
async fn block_outcomes_reach_the_receipt_and_the_settlement_metrics() {
    let (_dir, state, agent_id) = state_with_agent().await;
    let base = crate::auth::now_utc() - time::Duration::hours(1);
    let live = report_with_blocks(
        "0195f2a1-0013-4013-8013-000000000301",
        1,
        &[(10, 1_000, at(base, 1), 3, Subscription)],
    );
    let node_id = live.inventory.nodes[0].node_id.to_string();
    submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;
    let pool = state.db().pool();
    sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ($1, 11, 20, 'open_recoverable_gap', '2026-08-12T09:00:06Z', '2026-08-12T09:00:06Z')")
        .bind(&node_id).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, 11, 20, 'unrecoverable_backfill', 'declared', '2026-08-12T09:00:06Z', 'agent')")
        .bind(&node_id).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO block_identity_window (node_id, height, block_hash, retained_until, observed_at) VALUES ($1, 15, $2, NULL, $3)")
        .bind(&node_id).bind(format!("0x{:064x}", 0xdead_u64)).bind(at(base, 4))
        .execute(pool).await.unwrap();

    let mixed = report_with_blocks(
        "0195f2a1-0013-4013-8013-000000000302",
        2,
        &[
            (12, 3_000, at(base, 5), 1, GapBackfill),
            (15, 6_000, at(base, 5), 1, GapBackfill),
            (5, 500, at(base, 5), 1, Subscription),
            (9, 900, at(base, 5), 1, GapBackfill),
        ],
    );
    let receipt = submit(&state, &agent_id, serde_json::to_vec(&mixed).unwrap()).await;
    assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
    let terminal = SampleDispositionKind::TerminalRejected;
    assert_eq!(
        receipt
            .samples
            .iter()
            .map(|sample| (
                sample.disposition,
                sample.rejection.as_ref().map(|rejection| rejection.code)
            ))
            .collect::<Vec<_>>(),
        vec![
            (SampleDispositionKind::Accepted, None),
            (terminal, Some(RejectionCode::ChainDivergence)),
            (terminal, Some(RejectionCode::ResyncReplay)),
            (terminal, Some(RejectionCode::GapBackfillOutsideOpenGap)),
        ]
    );
    // Backfilling height 12 settled the gap around it, which stays open.
    let [runs, _, _, examined, resolved] = state.metrics().gap_settlement_counters("report");
    assert_eq!((runs, examined, resolved), (1, 1, 0));
}
