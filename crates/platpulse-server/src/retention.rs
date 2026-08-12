//! Fixed Phase 1 raw Block Summary retention.
//!
//! Raw summaries are deliberately retained for a fixed seven-day baseline and
//! removed in small batches. History state, coverage, gap rows, and the
//! independently retained identity/divergence evidence live in separate tables
//! and are never touched by this cleanup. There is no aggregate fallback in
//! Phase 1, so callers must surface ranges older than this baseline as
//! unavailable rather than inventing data.

use sqlx::SqlitePool;
use time::OffsetDateTime;

/// Phase 1 raw Block Summary retention baseline from design §11.3.
pub const RAW_BLOCK_SUMMARY_RETENTION_DAYS: i64 = 7;
/// Maximum number of raw rows removed by one cleanup invocation.
pub const RAW_BLOCK_SUMMARY_CLEANUP_BATCH: i64 = 128;
/// Phase 1 does not provide 1m/1h aggregate history.
pub const RAW_BLOCK_HISTORY_AGGREGATES_SUPPORTED: bool = false;

/// RFC3339 cutoff before which raw summaries may be removed.
pub fn raw_block_summary_cutoff(now: OffsetDateTime) -> OffsetDateTime {
    now - time::Duration::days(RAW_BLOCK_SUMMARY_RETENTION_DAYS)
}

/// Delete at most one bounded batch of expired raw summaries.
///
/// The query is intentionally one short SQLite statement: repeated startup or
/// ingestion calls are idempotent, and a large historical table cannot make a
/// single cleanup transaction unbounded. The tables that preserve dedup and
/// recovery state are not joined or deleted here.
pub async fn cleanup_raw_block_summaries(
    pool: &SqlitePool,
    now: OffsetDateTime,
) -> Result<u64, sqlx::Error> {
    let cutoff = crate::auth::format_rfc3339(raw_block_summary_cutoff(now));
    let result = sqlx::query(
        "DELETE FROM block_summaries WHERE rowid IN (SELECT rowid FROM block_summaries WHERE accepted_at < ? ORDER BY accepted_at, node_id, block_number LIMIT ?)",
    )
    .bind(cutoff)
    .bind(RAW_BLOCK_SUMMARY_CLEANUP_BATCH)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cleanup_is_bounded_and_preserves_dedup_state_in_temp_sqlite() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let old =
            crate::auth::format_rfc3339(raw_block_summary_cutoff(now) - time::Duration::hours(1));
        let fresh = crate::auth::format_rfc3339(now - time::Duration::hours(1));
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('retention-agent', 1, ?, ?)")
            .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('retention-network', 'Retention', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('retention-node', 'retention-agent', 'retention-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
        let insert = "INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ('retention-node', ?, '0xhash', '0xparent', '0xgenesis', 1, 1, 'lat', 1, ?, 2, 'subscription', '0x0000000000000000000000000000000000000000', 'unknown', 'unknown', 'test', ?);";
        for height in 0..(RAW_BLOCK_SUMMARY_CLEANUP_BATCH + 5) {
            sqlx::query(insert)
                .bind(height)
                .bind(&old)
                .bind(&old)
                .execute(pool)
                .await
                .unwrap();
        }
        sqlx::query(insert)
            .bind(9_999_i64)
            .bind(&fresh)
            .bind(&fresh)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, updated_at) VALUES ('retention-node', 9999, 133, 266, 0, ?)")
            .bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ('retention-node', 0, 9999, 'covered', ?, ?)")
            .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, created_at) VALUES ('retention-node', 100, 110, 'permanent_gap', ?)")
            .bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO block_identity_window (node_id, height, block_hash, retained_until, observed_at) VALUES ('retention-node', 9999, '0xhash', ?, ?)")
            .bind(crate::auth::format_rfc3339(now + time::Duration::days(30))).bind(&fresh).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO chain_divergence_observations (node_id, height, retained_block_hash, observed_block_hash, observed_at, reason, retained_observed_at) VALUES ('retention-node', 9999, '0xhash', '0xother', ?, 'test', ?)")
            .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();

        assert_eq!(
            cleanup_raw_block_summaries(pool, now).await.unwrap(),
            RAW_BLOCK_SUMMARY_CLEANUP_BATCH as u64
        );
        assert_eq!(cleanup_raw_block_summaries(pool, now).await.unwrap(), 5);
        assert_eq!(cleanup_raw_block_summaries(pool, now).await.unwrap(), 0);
        let fresh_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE block_number=9999")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(fresh_count, 1);
        let preserved: (i64, i64, i64, i64) = sqlx::query_as("SELECT historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count FROM block_history_state WHERE node_id='retention-node'").fetch_one(pool).await.unwrap();
        assert_eq!(preserved, (9999, 133, 266, 0));
        for table in [
            "block_coverage_intervals",
            "block_history_gaps",
            "block_identity_window",
            "chain_divergence_observations",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(pool)
                .await
                .unwrap();
            assert_eq!(count, 1, "retention deleted preservation table {table}");
        }
    }
}
