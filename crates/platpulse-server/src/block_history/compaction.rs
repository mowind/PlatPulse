//! Block coverage compaction: the retention step that keeps the coverage
//! intervals bounded and settles the History Gaps they no longer leave open.

use sqlx::AnyPool;
use time::OffsetDateTime;

use super::{GapSettlement, TouchedRange, settle_history_gaps_after, settle_touched_history_gaps};

/// One bounded batch of Block coverage compaction.
const COVERAGE_COMPACTION_BATCH: i64 = 128;

/// Confirm an open recoverable gap that made no progress for a whole raw
/// Block History window as missing. Backfilling a height narrows its open gap
/// and moves `updated_at`, so only a gap no Agent has filled for the window
/// is confirmed; a confirmed height is never admitted as a backfill again.
/// Returns the confirmed ranges: the only places a confirmation can have
/// settled a History Gap.
const CONFIRM_STALE_OPEN_GAPS_SQL: &str = "UPDATE block_coverage_intervals SET status = 'permanent_gap', updated_at = $1 WHERE coverage_id IN (SELECT coverage_id FROM block_coverage_intervals WHERE status = 'open_recoverable_gap' AND updated_at < $2 ORDER BY coverage_id LIMIT $3) RETURNING node_id, first_height, last_height";

/// Pairs of same-status intervals of one Node where the second starts right
/// after the first ends.
const ADJACENT_COVERAGE_PAIRS_SQL: &str = "SELECT a.coverage_id, b.coverage_id FROM block_coverage_intervals a JOIN block_coverage_intervals b ON b.node_id = a.node_id AND b.status = a.status AND b.first_height = a.last_height + 1 ORDER BY a.coverage_id LIMIT $1";

/// Resolve the Agent's own copies of Server refusals once they are older than
/// the raw Block History window. The Agent records a `server_rejected` gap for
/// every terminal refusal it receives; the Server keeps its own authoritative
/// row for an identity mismatch, so the Agent's copy is evidence only and is
/// released through the history-gap retention family instead of piling up.
const RESOLVE_AGENT_REFUSAL_COPIES_SQL: &str = "UPDATE block_history_gaps SET resolved_at = $1 WHERE gap_id IN (SELECT gap_id FROM block_history_gaps WHERE resolved_at IS NULL AND kind = 'server_rejected' AND authored_by = 'agent' AND created_at < $2 ORDER BY gap_id LIMIT $3)";

/// What one Block coverage compaction step did.
pub(crate) struct CoverageCompaction {
    /// Coverage and History Gap rows the step changed.
    pub(crate) changed: u64,
    /// The History Gap settlement the step ran, and how long it took.
    pub(crate) settlement: GapSettlement,
    pub(crate) settlement_elapsed: std::time::Duration,
}

/// Keep Block coverage bounded without deleting a coverage fact.
///
/// One retention step confirms open gaps no Agent filled since `window_start`,
/// the start of the raw Block History window, as missing (`permanent_gap`), merges adjacent intervals of
/// the same status into one row, resolves the `unrecoverable_backfill`
/// History Gaps nothing open overlaps any more, and resolves Agent copies of
/// Server refusals older than `window_start`, so the history-gap retention family can release them.
/// Every height keeps exactly the disposition it had: a merge only changes how
/// many rows describe it, and the High-Water Mark and cumulative counters are
/// never read or written here.
///
/// Settlement is bounded: the gaps around the ranges this step
/// confirmed are settled directly, and one batch of the remaining unresolved
/// gaps after `gap_cursor` is examined as a safety net, the cursor moving on
/// so successive sweeps walk the table instead of rescanning it.
pub(crate) async fn compact(
    pool: &AnyPool,
    now: OffsetDateTime,
    window_start: OffsetDateTime,
    gap_cursor: &mut i64,
) -> Result<CoverageCompaction, sqlx::Error> {
    let cutoff = crate::auth::format_rfc3339(window_start);
    let now_text = crate::auth::format_rfc3339(now);
    // Confirming and settling share one transaction so resolved_at always
    // agrees with the coverage it is derived from. Settling may record legacy
    // backfilled heights covered, so it runs before the merge that folds them
    // into their neighbours.
    let mut tx = crate::database::begin(pool).await?;
    let confirmed: Vec<(String, i64, i64)> = sqlx::query_as(CONFIRM_STALE_OPEN_GAPS_SQL)
        .bind(&now_text)
        .bind(&cutoff)
        .bind(COVERAGE_COMPACTION_BATCH)
        .fetch_all(&mut *tx)
        .await?;
    let mut changed = confirmed.len() as u64;
    let touched: Vec<TouchedRange> = confirmed
        .into_iter()
        .map(|(node_id, first, last)| TouchedRange {
            node_id,
            from_height: first.max(0) as u64,
            to_height: last.max(0) as u64,
        })
        .collect();
    let started = std::time::Instant::now();
    let mut settlement = settle_touched_history_gaps(&mut tx, &touched, &now_text).await?;
    let (catch_up, next_cursor) =
        settle_history_gaps_after(&mut tx, *gap_cursor, &now_text).await?;
    settlement.add(catch_up);
    let settlement_elapsed = started.elapsed();
    tx.commit().await?;
    *gap_cursor = next_cursor.unwrap_or(0);
    changed += settlement.resolved;
    let pairs: Vec<(i64, i64)> = sqlx::query_as(ADJACENT_COVERAGE_PAIRS_SQL)
        .bind(COVERAGE_COMPACTION_BATCH)
        .fetch_all(pool)
        .await?;
    for (lower, upper) in pairs {
        changed += merge_adjacent_intervals(pool, lower, upper).await?;
    }
    changed += sqlx::query(RESOLVE_AGENT_REFUSAL_COPIES_SQL)
        .bind(&now_text)
        .bind(&cutoff)
        .bind(COVERAGE_COMPACTION_BATCH)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(CoverageCompaction {
        changed,
        settlement,
        settlement_elapsed,
    })
}

#[derive(sqlx::FromRow)]
struct CoverageInterval {
    node_id: String,
    status: String,
    first_height: i64,
    last_height: i64,
    updated_at: String,
}

/// Fold `upper` into `lower` when, inside one transaction, both rows still
/// exist, belong to one Node and status, and still touch. A pair an earlier
/// merge of this batch consumed is skipped; the next sweep picks up the chain.
async fn merge_adjacent_intervals(
    pool: &AnyPool,
    lower: i64,
    upper: i64,
) -> Result<u64, sqlx::Error> {
    let mut tx = crate::database::begin(pool).await?;
    let interval_sql = "SELECT node_id, status, first_height, last_height, updated_at FROM block_coverage_intervals WHERE coverage_id = $1";
    let lower_interval: Option<CoverageInterval> = sqlx::query_as(interval_sql)
        .bind(lower)
        .fetch_optional(&mut *tx)
        .await?;
    let upper_interval: Option<CoverageInterval> = sqlx::query_as(interval_sql)
        .bind(upper)
        .fetch_optional(&mut *tx)
        .await?;
    let (Some(lower_interval), Some(upper_interval)) = (lower_interval, upper_interval) else {
        tx.rollback().await?;
        return Ok(0);
    };
    if lower_interval.node_id != upper_interval.node_id
        || lower_interval.status != upper_interval.status
        || lower_interval.last_height + 1 != upper_interval.first_height
    {
        tx.rollback().await?;
        return Ok(0);
    }
    // The newer stamp survives, so a merge never makes an open gap look staler
    // than its most recent progress.
    let updated_at = std::cmp::max(lower_interval.updated_at, upper_interval.updated_at);
    sqlx::query("UPDATE block_coverage_intervals SET last_height = $1, updated_at = $2 WHERE coverage_id = $3")
        .bind(upper_interval.last_height)
        .bind(updated_at)
        .bind(lower)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM block_coverage_intervals WHERE coverage_id = $1")
        .bind(upper)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(1)
}
