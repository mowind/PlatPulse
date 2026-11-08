//! The Public Block History of one Node: accepted Block Summaries still
//! inside raw retention, History Gaps and chain divergence observations,
//! newest height first.

use time::OffsetDateTime;

use crate::database::ServerDatabase;
use crate::http::public::PublicBlockHistoryItem;

use super::ACTIVE_NODE;

/// The heights and row count one Block History read asks for. The caller has
/// already refused a malformed range; the module only reads.
#[derive(Debug, Clone, Copy)]
pub struct BlockHistoryRange {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub limit: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct PublicHistoryRow {
    block_number: Option<i64>,
    block_timestamp_ms: Option<i64>,
    transaction_count: Option<i64>,
    source: Option<String>,
    coinbase: Option<String>,
    seal_signer_match: Option<String>,
    protocol_proposer: Option<String>,
    observed_at: Option<String>,
    from_height: Option<i64>,
    to_height: Option<i64>,
    gap_kind: Option<String>,
    divergence_kind: Option<String>,
}

/// Read the Node's Block History at `at`. The Active scope is checked
/// again inside the read, so a Node retired after the caller's check
/// contributes no row.
pub(super) async fn block_history(
    db: &ServerDatabase,
    node_id: &str,
    range: BlockHistoryRange,
    at: OffsetDateTime,
) -> Result<Vec<PublicBlockHistoryItem>, sqlx::Error> {
    let raw_retention_days = crate::retention::raw_block_summary_retention_days(db.pool()).await?;
    let cutoff =
        crate::auth::format_rfc3339(crate::retention::family_cutoff(at, raw_retention_days));
    let rows = sqlx::query_as::<_, PublicHistoryRow>(&format!(
        "SELECT block_number, block_timestamp_ms, transaction_count, source, coinbase, seal_signer_match, protocol_proposer, observed_at, from_height, to_height, gap_kind, divergence_kind FROM (SELECT block_number, block_timestamp_ms, transaction_count, source, coinbase, seal_signer_match, CASE WHEN protocol_proposer_kind = 'verified' THEN protocol_proposer_identity ELSE NULL END AS protocol_proposer, observed_at, NULL AS from_height, NULL AS to_height, NULL AS gap_kind, NULL AS divergence_kind FROM block_summaries WHERE node_id = $1 AND accepted_at >= $2 UNION ALL SELECT NULL AS block_number, NULL AS block_timestamp_ms, NULL AS transaction_count, NULL AS source, NULL AS coinbase, NULL AS seal_signer_match, NULL AS protocol_proposer, created_at AS observed_at, from_height, to_height, kind AS gap_kind, NULL AS divergence_kind FROM block_history_gaps WHERE node_id = $3 UNION ALL SELECT NULL AS block_number, NULL AS block_timestamp_ms, NULL AS transaction_count, NULL AS source, NULL AS coinbase, NULL AS seal_signer_match, NULL AS protocol_proposer, retained_observed_at AS observed_at, height AS from_height, height AS to_height, NULL AS gap_kind, 'chain_divergence' AS divergence_kind FROM chain_divergence_observations WHERE node_id = $4) AS observations WHERE EXISTS (SELECT 1 FROM nodes n WHERE n.node_id = $5 AND {ACTIVE_NODE}) AND ($6 IS NULL OR COALESCE(block_number, to_height) >= $7) AND ($8 IS NULL OR COALESCE(block_number, from_height) <= $9) ORDER BY COALESCE(block_number, from_height) DESC LIMIT $10"
    ))
    .bind(node_id)
    .bind(&cutoff)
    .bind(node_id)
    .bind(node_id)
    .bind(node_id)
    .bind(range.from)
    .bind(range.from)
    .bind(range.to)
    .bind(range.to)
    .bind(range.limit)
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| PublicBlockHistoryItem {
            node_id: node_id.to_owned(),
            height: row.block_number,
            block_time_ms: row.block_timestamp_ms,
            transaction_count: row.transaction_count,
            source: row.source,
            observed_at: row.observed_at.clone(),
            freshness: row.observed_at,
            gap_from_height: row.from_height,
            gap_to_height: row.to_height,
            gap_kind: row.gap_kind.clone(),
            gap_reason: row.gap_kind.as_deref().map(|kind| match kind {
                "spool_overflow" => "Some samples could not be retained".to_owned(),
                "unrecoverable_backfill" => "A history interval could not be recovered".to_owned(),
                "server_rejected" => "A sample was rejected by the Server".to_owned(),
                "chain_divergence" => "A recent chain identity divergence was observed".to_owned(),
                _ => "A history interval is unavailable".to_owned(),
            }),
            divergence_kind: row.divergence_kind.clone(),
            divergence_reason: row
                .divergence_kind
                .map(|_| "A recent chain identity divergence was observed".to_owned()),
            coinbase: row.coinbase,
            seal_signer_match: row.seal_signer_match,
            protocol_proposer: row.protocol_proposer,
        })
        .collect())
}
