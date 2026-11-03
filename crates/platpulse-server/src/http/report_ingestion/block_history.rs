//! Block history coverage: covered intervals, History Gap opening, backfill
//! admission and History Gap settlement.

use sqlx::{Any, AnyConnection, Transaction};

/// Open the part of an Agent-declared `unrecoverable_backfill` range the
/// Server has not already settled (issue #281).
///
/// A declared range can reach heights the Server already accepted (the Agent
/// backfilled them before a restart, or a live summary arrived first) or ones
/// it already confirmed missing. Opening those again would let a later
/// GapBackfill rewrite an accepted height after its raw summary expired and
/// count it twice, so only the uncovered stretches become open gaps. A height
/// the Server itself refused for an identity mismatch is never opened either
/// (ADR 0011 decision 3).
pub(super) async fn open_uncovered_gap(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    from_height: u64,
    to_height: u64,
    created_at: &str,
) -> Result<(), sqlx::Error> {
    let stretches =
        unsettled_stretches(tx, node_id, from_height, to_height, SETTLED_FOR_OPENING_SQL).await?;
    for (from, to) in stretches {
        open_coverage_gap(tx, node_id, from, to, created_at).await?;
    }
    Ok(())
}

/// Heights a declared gap may not open: covered, confirmed missing, or held
/// by a Server-authored refusal (ADR 0011 decision 3). Open intervals are left
/// out on purpose, because opening extends them.
const SETTLED_FOR_OPENING_SQL: &str = "SELECT first_height, last_height FROM block_coverage_intervals WHERE node_id = $1 AND status IN ('covered', 'permanent_gap') AND first_height <= $2 AND last_height >= $3 UNION ALL SELECT from_height, to_height FROM block_history_gaps WHERE node_id = $4 AND kind = 'server_rejected' AND authored_by = 'server' AND from_height <= $5 AND to_height >= $6 ORDER BY 1";

/// Heights with any coverage record, or a Server-authored refusal.
const RECORDED_HEIGHTS_SQL: &str = "SELECT first_height, last_height FROM block_coverage_intervals WHERE node_id = $1 AND first_height <= $2 AND last_height >= $3 UNION ALL SELECT from_height, to_height FROM block_history_gaps WHERE node_id = $4 AND kind = 'server_rejected' AND authored_by = 'server' AND from_height <= $5 AND to_height >= $6 ORDER BY 1";

/// The stretches of `from_height..=to_height` none of the ranges `settled_sql`
/// selects overlaps, lowest first.
async fn unsettled_stretches(
    conn: &mut AnyConnection,
    node_id: &str,
    from_height: u64,
    to_height: u64,
    settled_sql: &str,
) -> Result<Vec<(u64, u64)>, sqlx::Error> {
    let settled = sqlx::query_as::<_, (i64, i64)>(settled_sql)
        .bind(node_id)
        .bind(to_height as i64)
        .bind(from_height as i64)
        .bind(node_id)
        .bind(to_height as i64)
        .bind(from_height as i64)
        .fetch_all(&mut *conn)
        .await?;
    let mut stretches = Vec::new();
    let mut next = from_height;
    for (first, last) in settled {
        let (first, last) = (first.max(0) as u64, last.max(0) as u64);
        if first > next {
            stretches.push((next, first - 1));
        }
        if last >= to_height {
            return Ok(stretches);
        }
        next = next.max(last + 1);
    }
    if next <= to_height {
        stretches.push((next, to_height));
    }
    Ok(stretches)
}

async fn open_coverage_gap(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    from_height: u64,
    to_height: u64,
    created_at: &str,
) -> Result<(), sqlx::Error> {
    let overlap = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT coverage_id, first_height, last_height FROM block_coverage_intervals WHERE node_id=$1 AND status='open_recoverable_gap' AND first_height <= $2 AND last_height >= $3 ORDER BY first_height LIMIT 1",
    )
    .bind(node_id).bind(to_height as i64).bind(from_height as i64)
    .fetch_optional(&mut **tx).await?;
    if let Some((coverage_id, _first, _last)) = overlap {
        sqlx::query("UPDATE block_coverage_intervals SET first_height=CASE WHEN first_height <= $1 THEN first_height ELSE $1 END, last_height=CASE WHEN last_height >= $2 THEN last_height ELSE $2 END, updated_at=$3 WHERE coverage_id=$4")
            .bind(from_height as i64).bind(to_height as i64).bind(created_at).bind(coverage_id)
            .execute(&mut **tx).await?;
    } else {
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ($1, $2, $3, 'open_recoverable_gap', $4, $5)")
            .bind(node_id).bind(from_height as i64).bind(to_height as i64).bind(created_at).bind(created_at)
            .execute(&mut **tx).await?;
    }
    Ok(())
}

/// Record one accepted height as covered, merging it with the covered
/// intervals that end just below it and start just above it.
///
/// A height is accepted out of order whenever a hole is backfilled (issue
/// #281), so extending only the newest interval would leave one row per
/// backfilled height. Merging both neighbours keeps a fully backfilled hole
/// one interval, which is also what proves a History Gap resolved.
pub(super) async fn mark_covered_height(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    height: u64,
    now_text: &str,
) -> Result<(), sqlx::Error> {
    let below = match height.checked_sub(1) {
        Some(previous) => sqlx::query_scalar::<_, i64>(
            "SELECT coverage_id FROM block_coverage_intervals WHERE node_id=$1 AND status='covered' AND last_height=$2 ORDER BY coverage_id LIMIT 1",
        )
        .bind(node_id)
        .bind(previous as i64)
        .fetch_optional(&mut **tx)
        .await?,
        None => None,
    };
    let above = sqlx::query_as::<_, (i64, i64)>(
        "SELECT coverage_id, last_height FROM block_coverage_intervals WHERE node_id=$1 AND status='covered' AND first_height=$2 ORDER BY coverage_id LIMIT 1",
    )
    .bind(node_id)
    .bind(height as i64 + 1)
    .fetch_optional(&mut **tx)
    .await?;
    // A neighbour the coverage compaction merged away concurrently updates no
    // row; the height is then recorded on its own row, which the next
    // compaction merges, so a covered height is never lost.
    let extended = match (below, above) {
        (Some(below), Some((above, above_last))) => {
            let extended = sqlx::query("UPDATE block_coverage_intervals SET last_height=$1, updated_at=$2 WHERE coverage_id=$3")
                .bind(above_last).bind(now_text).bind(below)
                .execute(&mut **tx).await?.rows_affected() == 1;
            if extended {
                sqlx::query("DELETE FROM block_coverage_intervals WHERE coverage_id=$1")
                    .bind(above)
                    .execute(&mut **tx)
                    .await?;
            }
            extended
        }
        (Some(below), None) => {
            sqlx::query("UPDATE block_coverage_intervals SET last_height=$1, updated_at=$2 WHERE coverage_id=$3")
                .bind(height as i64).bind(now_text).bind(below)
                .execute(&mut **tx).await?.rows_affected() == 1
        }
        (None, Some((above, _))) => {
            sqlx::query("UPDATE block_coverage_intervals SET first_height=$1, updated_at=$2 WHERE coverage_id=$3")
                .bind(height as i64).bind(now_text).bind(above)
                .execute(&mut **tx).await?.rows_affected() == 1
        }
        (None, None) => false,
    };
    if !extended {
        insert_covered_interval(tx, node_id, height, height, now_text).await?;
    }
    Ok(())
}

async fn insert_covered_interval(
    conn: &mut AnyConnection,
    node_id: &str,
    first_height: u64,
    last_height: u64,
    now_text: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO block_coverage_intervals (node_id,first_height,last_height,status,created_at,updated_at) VALUES ($1, $2, $3, 'covered', $4, $5)")
        .bind(node_id).bind(first_height as i64).bind(last_height as i64).bind(now_text).bind(now_text)
        .execute(&mut *conn).await?;
    Ok(())
}

/// The open recoverable gap that contains `height`, if any.
pub(super) async fn open_gap_containing(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    height: u64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT coverage_id FROM block_coverage_intervals WHERE node_id=$1 AND status='open_recoverable_gap' AND first_height <= $2 AND last_height >= $3 LIMIT 1")
        .bind(node_id).bind(height as i64).bind(height as i64).fetch_optional(&mut **tx).await
}

/// Whether the Server admits one GapBackfill sample (issue #281).
///
/// The Server is the authority on which heights its history is missing, so an
/// Agent-declared open gap is one source of that boundary, not the only one.
/// A GapBackfill height is admitted when it is
///
/// - inside an open recoverable gap;
/// - above the Node's Historical High-Water Mark: new history, not a rewrite
///   (the head of a recovery range lands here before any gap is declared);
/// - a Server-observed hole: below the High-Water Mark but above the first
///   height the Server ever covered, and settled by nothing — no coverage
///   interval of any status, no Server-authored `server_rejected` gap (those
///   identity-mismatch holes are never backfilled, ADR 0011 decision 3), no
///   retained Block Summary, and no `unrecoverable_backfill` gap around it
///   (before #281 a height backfilled into a declared gap left no coverage
///   record; [`settle_history_gaps_in_range`] records it covered).
///
/// An Agent-authored `server_rejected` gap is only the Agent's copy of an
/// earlier refusal, such as this very `GapBackfillOutsideOpenGap`, so it never
/// makes a hole unrecoverable.
///
/// Every other height is a plain resync replay and stays
/// `GapBackfillOutsideOpenGap`, so the dedup rule still holds: an accepted
/// height is covered and can never be counted twice.
pub(super) async fn backfill_admitted(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    height: u64,
) -> Result<bool, sqlx::Error> {
    if open_gap_containing(tx, node_id, height).await?.is_some() {
        return Ok(true);
    }
    let high_water_mark: Option<i64> = sqlx::query_scalar(
        "SELECT historical_high_watermark FROM block_history_state WHERE node_id=$1",
    )
    .bind(node_id)
    .fetch_optional(&mut **tx)
    .await?;
    if high_water_mark.is_none_or(|mark| height as i64 > mark) {
        return Ok(true);
    }
    let height = height as i64;
    let hole: i64 = sqlx::query_scalar(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM block_coverage_intervals WHERE node_id=$1 AND status='covered' AND first_height < $2) AND NOT EXISTS (SELECT 1 FROM block_coverage_intervals WHERE node_id=$3 AND first_height <= $4 AND last_height >= $5) AND NOT EXISTS (SELECT 1 FROM block_history_gaps WHERE node_id=$6 AND ((kind='server_rejected' AND authored_by='server') OR kind='unrecoverable_backfill') AND from_height <= $7 AND to_height >= $8) AND NOT EXISTS (SELECT 1 FROM block_summaries WHERE node_id=$9 AND block_number=$10) THEN 1 ELSE 0 END",
    )
    .bind(node_id).bind(height)
    .bind(node_id).bind(height).bind(height)
    .bind(node_id).bind(height).bind(height)
    .bind(node_id).bind(height)
    .fetch_one(&mut **tx)
    .await?;
    Ok(hole == 1)
}

/// What one History Gap settlement pass did (issue #282): how many unresolved
/// `unrecoverable_backfill` gaps it examined and how many of them it resolved.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GapSettlement {
    pub(crate) examined: u64,
    pub(crate) resolved: u64,
}

impl GapSettlement {
    pub(crate) fn add(&mut self, other: GapSettlement) {
        self.examined += other.examined;
        self.resolved += other.resolved;
    }
}

/// A settlement pass slower than this is logged (issue #282): it is the early
/// signal that settlement is drifting back toward a whole-table scan.
pub(crate) const GAP_SETTLEMENT_SLOW: std::time::Duration = std::time::Duration::from_millis(500);

/// Count one settlement pass in the metrics and warn when it was slow.
/// `path` is `report` or `retention`.
pub(crate) fn observe_gap_settlement(
    metrics: &crate::metrics::MetricsRegistry,
    path: &'static str,
    settlement: GapSettlement,
    elapsed: std::time::Duration,
) {
    let slow = elapsed >= GAP_SETTLEMENT_SLOW;
    metrics.observe_gap_settlement(path, settlement, elapsed, slow);
    if slow {
        // The pass time goes in the message, not in `duration_ms`: that field
        // is the request duration in the logging field vocabulary, and on the report path the
        // request's completion line carries its own.
        tracing::warn!(
            "history gap settlement on the {path} path was slow: {} ms, examined {} unresolved gaps, resolved {}",
            elapsed.as_millis(),
            settlement.examined,
            settlement.resolved
        );
    }
}

/// Resolve the `unrecoverable_backfill` History Gaps of one Node that overlap
/// `from_height..=to_height` and that the Server has settled.
///
/// Such a gap opens coverage for its uncovered heights in the transaction that
/// records it, and a height leaves the open state only when it is backfilled
/// (covered) or confirmed missing (`permanent_gap`). A gap no open interval
/// overlaps therefore has its final disposition, which is what makes
/// `resolved_at` mean something and lets retention bound the table (issue
/// #281). The confirmed-missing fact itself stays on the protected coverage
/// interval, so resolving the row never hides a hole.
///
/// A gap becomes settled only at the moment its last open height leaves the
/// open state, and that height lies inside the gap. So whoever closes open
/// heights (Report Ingestion backfilling them, retention confirming them
/// missing) settles exactly the gaps around those heights, and nothing else
/// has to be looked at: issue #282 replaced the per-Report pass that probed
/// every unresolved gap of the table with this range-bounded one.
pub(crate) async fn settle_history_gaps_in_range(
    conn: &mut AnyConnection,
    node_id: &str,
    from_height: u64,
    to_height: u64,
    now_text: &str,
) -> Result<GapSettlement, sqlx::Error> {
    let mut settlement = GapSettlement::default();
    // Keyset cursor in the index order (to_height, gap_id); it starts just
    // below the range.
    let mut after = (from_height as i64 - 1, 0_i64);
    loop {
        let candidates = sqlx::query_as::<_, GapCandidate>(GAP_CANDIDATES_IN_RANGE_SQL)
            .bind(node_id)
            .bind(from_height as i64)
            .bind(to_height as i64)
            .bind(after.0)
            .bind(after.0)
            .bind(after.1)
            .bind(HISTORY_GAP_SETTLE_BATCH)
            .fetch_all(&mut *conn)
            .await?;
        settlement.add(settle_candidates(conn, &candidates, now_text).await?);
        match candidates.last() {
            Some(last) if candidates.len() as i64 == HISTORY_GAP_SETTLE_BATCH => {
                after = (last.to_height, last.gap_id);
            }
            _ => return Ok(settlement),
        }
    }
}

/// One Node's heights that left the open state, or that a History Gap was
/// declared over: the only places a gap can have become settled (issue #282).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TouchedRange {
    pub(crate) node_id: String,
    pub(crate) from_height: u64,
    pub(crate) to_height: u64,
}

/// Settle the gaps around every touched range, merging the overlapping and
/// adjacent ranges of one Node first so a run of backfilled heights is one
/// bounded lookup rather than one per height.
pub(crate) async fn settle_touched_history_gaps(
    conn: &mut AnyConnection,
    touched: &[TouchedRange],
    now_text: &str,
) -> Result<GapSettlement, sqlx::Error> {
    let mut settlement = GapSettlement::default();
    for range in merge_touched_ranges(touched) {
        settlement.add(
            settle_history_gaps_in_range(
                conn,
                &range.node_id,
                range.from_height,
                range.to_height,
                now_text,
            )
            .await?,
        );
    }
    Ok(settlement)
}

/// Sort the touched ranges per Node and merge the overlapping and adjacent ones.
fn merge_touched_ranges(touched: &[TouchedRange]) -> Vec<TouchedRange> {
    let mut ranges = touched.to_vec();
    ranges.sort();
    let mut merged: Vec<TouchedRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last)
                if last.node_id == range.node_id
                    && range.from_height <= last.to_height.saturating_add(1) =>
            {
                last.to_height = last.to_height.max(range.to_height);
            }
            _ => merged.push(range),
        }
    }
    merged
}

/// Examine one bounded batch of every Node's unresolved `unrecoverable_backfill`
/// gaps after `after_gap_id` and resolve the settled ones.
///
/// This is the retention sweep's safety net, not the main settlement path: it
/// finds the gaps no range-bounded settlement reached, such as gaps settled
/// before issue #281 shipped. Each call reads at most one batch, so a large
/// backlog costs a bounded amount of work per sweep instead of one whole-table
/// pass (issue #282). Returns the settlement and the last gap examined, or
/// `None` once the scan reached the end and starts over.
pub(crate) async fn settle_history_gaps_after(
    conn: &mut AnyConnection,
    after_gap_id: i64,
    now_text: &str,
) -> Result<(GapSettlement, Option<i64>), sqlx::Error> {
    let candidates = sqlx::query_as::<_, GapCandidate>(GAP_CANDIDATES_AFTER_SQL)
        .bind(after_gap_id)
        .bind(HISTORY_GAP_SETTLE_BATCH)
        .fetch_all(&mut *conn)
        .await?;
    let settlement = settle_candidates(conn, &candidates, now_text).await?;
    let next = match candidates.last() {
        Some(last) if candidates.len() as i64 == HISTORY_GAP_SETTLE_BATCH => Some(last.gap_id),
        _ => None,
    };
    Ok((settlement, next))
}

/// One unresolved `unrecoverable_backfill` gap read for settlement, with
/// whether an open interval still overlaps it (0 or 1, decoded as an integer
/// because `Any` has no portable boolean).
#[derive(Debug, sqlx::FromRow)]
struct GapCandidate {
    gap_id: i64,
    node_id: String,
    from_height: i64,
    to_height: i64,
    still_open: i64,
}

/// Resolve the candidates no open interval overlaps any more.
async fn settle_candidates(
    conn: &mut AnyConnection,
    candidates: &[GapCandidate],
    now_text: &str,
) -> Result<GapSettlement, sqlx::Error> {
    let mut settlement = GapSettlement::default();
    for candidate in candidates {
        settlement.examined += 1;
        if candidate.still_open == 0 {
            settlement.resolved += resolve_settled_gap(
                conn,
                candidate.gap_id,
                &candidate.node_id,
                candidate.from_height,
                candidate.to_height,
                now_text,
            )
            .await?;
        }
    }
    Ok(settlement)
}

/// Resolve one settled gap.
///
/// Before #281 a height backfilled into a declared gap only narrowed the open
/// interval and left no coverage record, so a settled gap can still hold
/// heights with no record at all. Every such height was backfilled (nothing
/// else removed an open height), so it is recorded covered before the gap is
/// resolved: otherwise it would read as a Server-observed hole once its raw
/// summary expired and be counted twice.
async fn resolve_settled_gap(
    conn: &mut AnyConnection,
    gap_id: i64,
    node_id: &str,
    from_height: i64,
    to_height: i64,
    now_text: &str,
) -> Result<u64, sqlx::Error> {
    let (from_height, to_height) = (from_height.max(0) as u64, to_height.max(0) as u64);
    for (first, last) in
        unsettled_stretches(conn, node_id, from_height, to_height, RECORDED_HEIGHTS_SQL).await?
    {
        insert_covered_interval(conn, node_id, first, last, now_text).await?;
    }
    Ok(sqlx::query(
        "UPDATE block_history_gaps SET resolved_at = $1 WHERE gap_id = $2 AND resolved_at IS NULL",
    )
    .bind(now_text)
    .bind(gap_id)
    .execute(&mut *conn)
    .await?
    .rows_affected())
}

/// One bounded batch of History Gap settlement.
const HISTORY_GAP_SETTLE_BATCH: i64 = 128;

/// The unresolved `unrecoverable_backfill` gaps matching `$filter`, read as
/// [`GapCandidate`] rows. The `still_open` probe is an index-only scan of the
/// open-interval partial index of migration 0080, bounded by the Node's open
/// intervals above the gap; open intervals may overlap, so it cannot be a
/// single `LIMIT 1` seek.
macro_rules! gap_candidates_sql {
    ($filter:literal) => {
        concat!(
            "SELECT g.gap_id, g.node_id, g.from_height, g.to_height, ",
            "CASE WHEN EXISTS (SELECT 1 FROM block_coverage_intervals c WHERE c.node_id = g.node_id AND c.status = 'open_recoverable_gap' AND c.first_height <= g.to_height AND c.last_height >= g.from_height) THEN 1 ELSE 0 END AS still_open ",
            "FROM block_history_gaps g WHERE g.resolved_at IS NULL AND g.kind = 'unrecoverable_backfill' AND ",
            $filter
        )
    };
}

/// One Node's gaps overlapping a height range, in batches keyed by
/// `(to_height, gap_id)`. The lookup walks the unsettled-gap partial index of
/// migration 0080 from the bottom of the range; ordering by anything the index
/// does not lead with (such as `gap_id` alone) would let the planner walk every
/// unresolved gap instead. The Node is a plain equality: an `IS NULL` branch
/// would let the planner fall back to a whole-table scan (issue #282).
const GAP_CANDIDATES_IN_RANGE_SQL: &str = gap_candidates_sql!(
    "g.node_id = $1 AND g.to_height >= $2 AND g.from_height <= $3 AND (g.to_height > $4 OR (g.to_height = $5 AND g.gap_id > $6)) ORDER BY g.to_height, g.gap_id LIMIT $7"
);

/// Every Node's gaps after a `gap_id` keyset cursor.
const GAP_CANDIDATES_AFTER_SQL: &str =
    gap_candidates_sql!("g.gap_id > $1 ORDER BY g.gap_id LIMIT $2");

pub(super) async fn close_coverage_height(
    tx: &mut Transaction<'_, Any>,
    coverage_id: i64,
    height: u64,
    now_text: &str,
) -> Result<(), sqlx::Error> {
    let row = sqlx::query_as::<_, (i64, i64)>(
        "SELECT first_height,last_height FROM block_coverage_intervals WHERE coverage_id=$1",
    )
    .bind(coverage_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((first, last)) = row else {
        return Ok(());
    };
    let first = first as u64;
    let last = last as u64;
    if first == last {
        sqlx::query("DELETE FROM block_coverage_intervals WHERE coverage_id=$1")
            .bind(coverage_id)
            .execute(&mut **tx)
            .await?;
    } else if height == first {
        sqlx::query("UPDATE block_coverage_intervals SET first_height=first_height+1,updated_at=$1 WHERE coverage_id=$2").bind(now_text).bind(coverage_id).execute(&mut **tx).await?;
    } else if height == last {
        sqlx::query("UPDATE block_coverage_intervals SET last_height=last_height-1,updated_at=$1 WHERE coverage_id=$2").bind(now_text).bind(coverage_id).execute(&mut **tx).await?;
    } else if height > first && height < last {
        sqlx::query(
            "UPDATE block_coverage_intervals SET last_height=$1 ,updated_at=$2 WHERE coverage_id=$3",
        )
        .bind((height - 1) as i64)
        .bind(now_text)
        .bind(coverage_id)
        .execute(&mut **tx)
        .await?;
        // The interval is split in two: the row keeps [first, height-1] and the
        // copy above it covers [height+1, last], so height+1 is the first
        // height the remaining gap still misses.
        sqlx::query("INSERT INTO block_coverage_intervals (node_id,first_height,last_height,status,created_at,updated_at) SELECT node_id,$1,$2,status,created_at,$3 FROM block_coverage_intervals WHERE coverage_id=$4")
            .bind((height + 1) as i64)
            .bind(last as i64)
            .bind(now_text)
            .bind(coverage_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    use crate::http::report_ingestion::network::observe_block_identity;
    use crate::http::report_ingestion::test_helpers::*;

    use axum::http::StatusCode;

    use crate::auth::{format_rfc3339, now_utc};

    use crate::http::AppState;

    use platpulse_core::{AgentReport, ReceiptDisposition, SampleDispositionKind};

    #[tokio::test]
    pub(super) async fn replay_sample_is_terminal_and_does_not_rewrite_history() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report.report_id = "0195f2a1-0013-4013-8013-000000000201".parse().unwrap();
        let node_id = report.inventory.nodes[0].node_id;
        let mut identity = report.nodes[0]
            .chain
            .network_identity
            .latest
            .clone()
            .unwrap();
        identity.genesis_hash =
            "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap();
        identity.address_hrp = Some("lat".to_owned());
        report
            .block_summaries
            .push(platpulse_core::block::BlockSummary {
                node_id,
                network_identity: identity,
                block_number: 10,
                block_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .parse()
                    .unwrap(),
                parent_hash: "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .parse()
                    .unwrap(),
                block_timestamp_ms: 1_000,
                observed_at: report.generated_at,
                transaction_count: 3,
                block_interval_ms: None,
                source: platpulse_core::block::BlockSource::Subscription,
                attribution: platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                    "0x1111111111111111111111111111111111111111"
                        .parse()
                        .unwrap(),
                    "test",
                ),
            });
        report.validate().unwrap();
        let first = report.clone();
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;
        sqlx::query("UPDATE block_summaries SET accepted_at=$1 WHERE node_id=$2")
            .bind("2026-01-01T00:00:00Z")
            .bind(node_id.to_string())
            .execute(state.db().pool())
            .await
            .unwrap();
        crate::retention::cleanup_raw_block_summaries(
            state.db().pool(),
            crate::auth::parse_rfc3339("2026-08-12T08:00:00Z").unwrap(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE block_history_state SET historical_high_watermark=100, cumulative_block_count=1, cumulative_transaction_count=3 WHERE node_id=$1").bind(node_id.to_string()).execute(state.db().pool()).await.unwrap();

        let mut replay = report;
        replay.report_sequence = 2;
        replay.report_id = "0195f2a1-0013-4013-8013-000000000202".parse().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(
            receipt.samples[0].disposition,
            SampleDispositionKind::TerminalRejected
        );
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::ResyncReplay
        );
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        let counters: (i64, i64) = sqlx::query_as("SELECT cumulative_block_count, cumulative_transaction_count FROM block_history_state WHERE node_id=$1").bind(node_id.to_string()).fetch_one(state.db().pool()).await.unwrap();
        assert_eq!(summaries, 0);
        assert_eq!(counters, (1, 3));
    }

    /// A 5-minute bucket edge an hour before now. The block tiers expire with
    /// their aggregate families after every accepted Report, so the instants a
    /// block case writes have to be inside the tiers' own windows.
    pub(super) fn block_base() -> time::OffsetDateTime {
        let hour_ago = now_utc() - time::Duration::hours(1);
        time::OffsetDateTime::from_unix_timestamp(hour_ago.unix_timestamp().div_euclid(300) * 300)
            .unwrap()
    }

    /// A report carrying the given Block Summaries for the fixture's Node, each
    /// stamped with the registered Network Identity: (height, block timestamp
    /// in ms, observed_at, transactions, source).
    pub(super) fn report_with_blocks(
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
        identity.genesis_hash =
            "0x0000000000000000000000000000000000000000000000000000000000000001"
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

    /// One block bucket as (block_count, transaction_total, transaction_min,
    /// transaction_max, interval_count, interval_min_ms, interval_max_ms).
    pub(super) type BlockBucket = (i64, i64, i64, i64, i64, Option<i64>, Option<i64>);

    pub(super) async fn block_bucket(
        state: &AppState,
        node_id: &str,
        grain_seconds: i64,
        bucket_start: &str,
    ) -> Option<BlockBucket> {
        sqlx::query_as("SELECT block_count, transaction_total, transaction_min, transaction_max, interval_count, interval_min_ms, interval_max_ms FROM node_block_aggregates WHERE node_id = $1 AND grain_seconds = $2 AND bucket_start = $3")
            .bind(node_id)
            .bind(grain_seconds)
            .bind(bucket_start)
            .fetch_optional(state.db().pool())
            .await
            .unwrap()
    }

    pub(super) async fn block_bucket_rows(state: &AppState) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM node_block_aggregates")
            .fetch_one(state.db().pool())
            .await
            .unwrap()
    }

    /// Every Block Coverage interval of a Node as
    /// (first_height, last_height, status), ordered by the lower bound.
    pub(super) async fn coverage_intervals(
        state: &AppState,
        node_id: &str,
    ) -> Vec<(i64, i64, String)> {
        sqlx::query_as("SELECT first_height, last_height, status FROM block_coverage_intervals WHERE node_id=$1 ORDER BY first_height")
            .bind(node_id)
            .fetch_all(state.db().pool())
            .await
            .unwrap()
    }

    /// Issue #248: an accepted block advances one bucket of each tier, by the
    /// minute and five minutes its own observation falls in, with its
    /// transactions and its interval to the held parent. The identical Report
    /// delivered again is a Replay and a later Report repeating the same
    /// heights is a repeated delivery: neither moves a bucket.
    #[tokio::test]
    pub(super) async fn accepted_blocks_advance_both_block_tiers_and_a_repeat_never_does() {
        use platpulse_core::block::BlockSource::Subscription;
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let blocks = [
            (10, 1_000, at(base, 1), 3, Subscription),
            (11, 3_000, at(base, 3), 5, Subscription),
            (12, 4_500, at(base, 5), 1, Subscription),
            (13, 6_500, at(base, 67), 2, Subscription),
        ];
        let report = report_with_blocks("0195f2a1-0013-4013-8013-000000000301", 1, &blocks);
        let node_id = report.inventory.nodes[0].node_id.to_string();
        let body = serde_json::to_vec(&report).unwrap();
        submit(&state, &agent_id, body.clone()).await;

        // Three blocks in the 09:00 minute: 3 + 5 + 1 transactions, and the
        // two intervals whose parent is held (block 10's parent is not).
        let minute = Some((3, 9, 1, 5, 2, Some(1_500), Some(2_000)));
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            minute
        );
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 60)).await,
            Some((1, 2, 2, 2, 1, Some(2_000), Some(2_000)))
        );
        // The 5-minute bucket counted all four.
        let five = Some((4, 11, 1, 5, 3, Some(1_500), Some(2_000)));
        assert_eq!(
            block_bucket(&state, &node_id, 300, &at(base, 0)).await,
            five
        );
        assert_eq!(
            block_bucket_rows(&state).await,
            3,
            "an empty bucket is never written"
        );

        // A Replay of the same Report bytes answers the stored receipt.
        submit(&state, &agent_id, body).await;
        // A later Report delivering the same heights again.
        let repeat = report_with_blocks("0195f2a1-0013-4013-8013-000000000302", 2, &blocks);
        submit(&state, &agent_id, serde_json::to_vec(&repeat).unwrap()).await;
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            minute
        );
        assert_eq!(
            block_bucket(&state, &node_id, 300, &at(base, 0)).await,
            five
        );
        assert_eq!(block_bucket_rows(&state).await, 3);
    }

    /// Issue #248: a plain resync replay - a subscription block at or below the
    /// historical high-water mark whose raw row retention has already released
    /// - is refused by Block history dedup and therefore never re-accumulates,
    /// even though no raw row is left to collide with.
    #[tokio::test]
    pub(super) async fn a_resync_replay_below_the_high_water_mark_never_accumulates() {
        use platpulse_core::block::BlockSource::Subscription;
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let blocks = [(10, 1_000, at(base, 1), 3, Subscription)];
        let first = report_with_blocks("0195f2a1-0013-4013-8013-000000000311", 1, &blocks);
        let node_id = first.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;
        sqlx::query("DELETE FROM block_summaries WHERE node_id=$1")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE block_history_state SET historical_high_watermark=100 WHERE node_id=$1",
        )
        .bind(&node_id)
        .execute(state.db().pool())
        .await
        .unwrap();

        let replay = report_with_blocks("0195f2a1-0013-4013-8013-000000000312", 2, &blocks);
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::ResyncReplay
        );
        let once = Some((1, 3, 3, 3, 0, None, None));
        assert_eq!(block_bucket(&state, &node_id, 60, &at(base, 0)).await, once);
        assert_eq!(
            block_bucket(&state, &node_id, 300, &at(base, 0)).await,
            once
        );
    }

    /// Issue #248: only an explicit OpenRecoverableGap admits a block below the
    /// high-water mark. Its backfill is counted once, in the bucket of its own
    /// observation, and completes the interval of the child that was waiting
    /// for it in the child's own bucket; a second delivery of the same backfill
    /// and a backfill outside any open gap accumulate nothing.
    #[tokio::test]
    pub(super) async fn an_open_gap_backfill_accumulates_once_and_completes_its_child_interval() {
        use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let live = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000321",
            1,
            &[
                (10, 1_000, at(base, 1), 3, Subscription),
                (12, 5_000, at(base, 5), 4, Subscription),
            ],
        );
        let node_id = live.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            Some((2, 7, 3, 4, 0, None, None)),
            "neither block has a held parent yet"
        );
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ($1, 11, 11, 'open_recoverable_gap', '2026-08-12T09:00:06Z', '2026-08-12T09:00:06Z')")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();

        let backfill = [(11, 2_500, at(base, 210), 6, GapBackfill)];
        let first = report_with_blocks("0195f2a1-0013-4013-8013-000000000322", 2, &backfill);
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;
        // Block 11 lands in its own minute with its interval to block 10...
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 180)).await,
            Some((1, 6, 6, 6, 1, Some(1_500), Some(1_500)))
        );
        // ...and completes block 12's interval in block 12's minute.
        let completed = Some((2, 7, 3, 4, 1, Some(2_500), Some(2_500)));
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            completed
        );
        let five = Some((3, 13, 3, 6, 2, Some(1_500), Some(2_500)));
        assert_eq!(
            block_bucket(&state, &node_id, 300, &at(base, 0)).await,
            five
        );

        // The gap is closed now: the same backfill again, and a backfill below
        // the high-water mark outside any open gap, are both refused.
        let again = report_with_blocks("0195f2a1-0013-4013-8013-000000000323", 3, &backfill);
        submit(&state, &agent_id, serde_json::to_vec(&again).unwrap()).await;
        let outside = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000324",
            4,
            &[(9, 500, at(base, 220), 9, GapBackfill)],
        );
        submit(&state, &agent_id, serde_json::to_vec(&outside).unwrap()).await;
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            completed
        );
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 180)).await,
            Some((1, 6, 6, 6, 1, Some(1_500), Some(1_500)))
        );
        assert_eq!(
            block_bucket(&state, &node_id, 300, &at(base, 0)).await,
            five
        );
    }

    /// Issue #266: a backfill that lands strictly inside an open gap splits the
    /// interval, so the still-missing range above it keeps its own bounds
    /// instead of being reported covered. The upper half is a copy of the same
    /// gap, so it keeps the gap's own `created_at` rather than the receipt time.
    #[tokio::test]
    pub(super) async fn a_backfill_inside_an_open_gap_splits_the_interval() {
        use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let live = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000341",
            1,
            &[(10, 1_000, at(base, 1), 3, Subscription)],
        );
        let node_id = live.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ($1, 11, 20, 'open_recoverable_gap', '2026-08-12T09:00:06Z', '2026-08-12T09:00:06Z')")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();

        let backfill = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000342",
            2,
            &[(15, 5_000, at(base, 5), 4, GapBackfill)],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&backfill).unwrap()).await;
        assert!(
            receipt.samples[0].rejection.is_none(),
            "the backfill inside the open gap is accepted, got {:?}",
            receipt.samples[0].rejection
        );

        assert_eq!(
            coverage_intervals(&state, &node_id).await,
            vec![
                (10, 10, "covered".to_owned()),
                (11, 14, "open_recoverable_gap".to_owned()),
                (15, 15, "covered".to_owned()),
                (16, 20, "open_recoverable_gap".to_owned()),
            ]
        );
        let halves: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT first_height, last_height, created_at FROM block_coverage_intervals WHERE node_id=$1 AND status='open_recoverable_gap' ORDER BY first_height",
        )
        .bind(&node_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            halves,
            vec![
                (11, 14, "2026-08-12T09:00:06Z".to_owned()),
                (16, 20, "2026-08-12T09:00:06Z".to_owned()),
            ],
            "both halves are still the same gap, so they keep its created_at"
        );
    }

    /// Issue #266: a backfill that sits on an open gap's own edge narrows the
    /// interval instead of splitting, duplicating, or dropping it — the other
    /// two arms of the split that once lost a gap's upper half.
    #[tokio::test]
    pub(super) async fn a_backfill_on_an_open_gap_edge_narrows_the_interval() {
        use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let live = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000343",
            1,
            &[(10, 1_000, at(base, 1), 3, Subscription)],
        );
        let node_id = live.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ($1, 11, 20, 'open_recoverable_gap', '2026-08-12T09:00:06Z', '2026-08-12T09:00:06Z')")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        let lower = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000344",
            2,
            &[(11, 2_500, at(base, 5), 4, GapBackfill)],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&lower).unwrap()).await;
        assert!(
            receipt.samples[0].rejection.is_none(),
            "the backfill on the gap's first height is accepted, got {:?}",
            receipt.samples[0].rejection
        );
        assert_eq!(
            coverage_intervals(&state, &node_id).await,
            vec![
                (10, 11, "covered".to_owned()),
                (12, 20, "open_recoverable_gap".to_owned()),
            ],
            "closing the gap's first height moves its lower bound up by one and covers it"
        );

        let upper = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000345",
            3,
            &[(20, 12_000, at(base, 9), 5, GapBackfill)],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&upper).unwrap()).await;
        assert!(
            receipt.samples[0].rejection.is_none(),
            "the backfill on the gap's last height is accepted, got {:?}",
            receipt.samples[0].rejection
        );
        assert_eq!(
            coverage_intervals(&state, &node_id).await,
            vec![
                (10, 11, "covered".to_owned()),
                (12, 19, "open_recoverable_gap".to_owned()),
                (20, 20, "covered".to_owned()),
            ],
            "closing the gap's last height moves its upper bound down by one"
        );
    }

    /// A report carrying the given Block Summaries plus Agent-declared
    /// `unrecoverable_backfill` gaps, as one bounded recovery round sends them.
    pub(super) fn report_with_blocks_and_gaps(
        report_id: &str,
        sequence: u64,
        blocks: &[(u64, u64, String, u64, platpulse_core::block::BlockSource)],
        gaps: &[(u64, u64)],
    ) -> AgentReport {
        let mut report = report_with_blocks(report_id, sequence, blocks);
        let node_id = report.inventory.nodes[0].node_id;
        for (from_height, to_height) in gaps.iter().copied() {
            report.history_gaps.push(platpulse_core::HistoryGap {
                node_id,
                kind: platpulse_core::GapKind::UnrecoverableBackfill,
                from_height,
                to_height,
                reason: "subscription reconnect exceeded configured backfill bounds".to_owned(),
                recorded_at: report.generated_at,
            });
        }
        report.validate().unwrap();
        report
    }

    /// Every `unrecoverable_backfill` History Gap of a Node as
    /// (from_height, to_height, resolved).
    pub(super) async fn backfill_gap_rows(
        state: &AppState,
        node_id: &str,
    ) -> Vec<(i64, i64, bool)> {
        let rows: Vec<(i64, i64, Option<String>)> = sqlx::query_as(
            "SELECT from_height, to_height, resolved_at FROM block_history_gaps WHERE node_id=$1 AND kind='unrecoverable_backfill' ORDER BY from_height",
        )
        .bind(node_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        rows.into_iter()
            .map(|(from, to, resolved)| (from, to, resolved.is_some()))
            .collect()
    }

    /// Issue #282 regression: History Gap settlement on the Report path is
    /// bounded by what the Report touched, not by the size of the tables. A
    /// Node carries 10,000 open recoverable intervals, each under an unresolved
    /// `unrecoverable_backfill` gap. A live tip Report closes nothing and runs
    /// no settlement at all; a backfill of one height examines and resolves
    /// only the gap around it; a gap declared over covered heights examines
    /// only itself.
    #[tokio::test]
    pub(super) async fn gap_settlement_examines_only_the_gaps_a_report_touched() {
        use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
        const OPEN_GAPS: i64 = 10_000;
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let live = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000371",
            1,
            &[
                (10, 10_000, at(base, 1), 1, Subscription),
                (11, 11_000, at(base, 2), 1, Subscription),
                (50_000, 50_000_000, at(base, 3), 1, Subscription),
            ],
        );
        let node_id = live.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;

        // One single-height open interval and one unresolved gap at every
        // even height from 20,000, as a long-unfilled recovery leaves them.
        let stamp = format_rfc3339(now_utc());
        let pool = state.db().pool();
        sqlx::query("WITH RECURSIVE seq(k) AS (SELECT 0 UNION ALL SELECT k + 1 FROM seq WHERE k + 1 < $1) INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) SELECT $2, 20000 + 2 * k, 20000 + 2 * k, 'open_recoverable_gap', $3, $4 FROM seq")
            .bind(OPEN_GAPS).bind(&node_id).bind(&stamp).bind(&stamp)
            .execute(pool).await.unwrap();
        sqlx::query("WITH RECURSIVE seq(k) AS (SELECT 0 UNION ALL SELECT k + 1 FROM seq WHERE k + 1 < $1) INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) SELECT $2, 20000 + 2 * k, 20000 + 2 * k, 'unrecoverable_backfill', 'seeded', $3, 'agent' FROM seq")
            .bind(OPEN_GAPS).bind(&node_id).bind(&stamp)
            .execute(pool).await.unwrap();
        let unresolved = || async {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM block_history_gaps WHERE node_id=$1 AND kind='unrecoverable_backfill' AND resolved_at IS NULL",
            )
            .bind(&node_id)
            .fetch_one(pool)
            .await
            .unwrap()
        };
        assert_eq!(unresolved().await, OPEN_GAPS);
        let metrics = state.metrics();
        assert_eq!(metrics.gap_settlement_counters("report")[0], 0);

        // A live tip Report touches no open height: no settlement runs.
        let tip = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000372",
            2,
            &[(50_001, 50_001_000, at(base, 4), 1, Subscription)],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&tip).unwrap()).await;
        assert_eq!(rejections(&receipt), vec![None]);
        let [runs, _, _, examined, resolved] = metrics.gap_settlement_counters("report");
        assert_eq!((runs, examined, resolved), (0, 0, 0));
        assert_eq!(unresolved().await, OPEN_GAPS);

        // Backfilling one height settles exactly the gap around it.
        let backfill = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000373",
            3,
            &[(30_000, 30_000_000, at(base, 5), 1, GapBackfill)],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&backfill).unwrap()).await;
        assert_eq!(rejections(&receipt), vec![None]);
        let [runs, _, _, examined, resolved] = metrics.gap_settlement_counters("report");
        assert_eq!((runs, examined, resolved), (1, 1, 1));
        assert_eq!(unresolved().await, OPEN_GAPS - 1);
        let settled: Option<String> = sqlx::query_scalar(
            "SELECT resolved_at FROM block_history_gaps WHERE node_id=$1 AND from_height=30000",
        )
        .bind(&node_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert!(settled.is_some(), "the backfilled gap is resolved");

        // A gap declared over heights already covered settles at once, and
        // examines nothing but itself.
        let declared = report_with_blocks_and_gaps(
            "0195f2a1-0013-4013-8013-000000000374",
            4,
            &[(50_002, 50_002_000, at(base, 6), 1, Subscription)],
            &[(10, 11)],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert_eq!(rejections(&receipt), vec![None]);
        let [runs, _, _, examined, resolved] = metrics.gap_settlement_counters("report");
        assert_eq!((runs, examined, resolved), (2, 2, 2));
        assert_eq!(unresolved().await, OPEN_GAPS - 1);
    }

    /// Issue #281 runtime regression: a bounded recovery round backfills the
    /// head of a hole and declares the rest a gap, while a live summary has
    /// already raised the High-Water Mark past the whole hole. The head of the
    /// hole lies in no open gap (the gap is declared in the same Report, after
    /// the samples), yet the Server must still admit it as a hole it observes
    /// itself; the declared rest is backfilled by a later round; and the hole
    /// ends as one covered interval with its History Gap resolved.
    #[tokio::test]
    pub(super) async fn a_hole_whose_head_is_outside_any_open_gap_still_backfills() {
        use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let live = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000361",
            1,
            &[
                (10, 1_000, at(base, 1), 1, Subscription),
                (11, 2_000, at(base, 2), 1, Subscription),
                (20, 11_000, at(base, 11), 1, Subscription),
            ],
        );
        let node_id = live.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;

        // The bounded round: heights 12..=14 backfilled, 15..=19 declared.
        let round = report_with_blocks_and_gaps(
            "0195f2a1-0013-4013-8013-000000000362",
            2,
            &[
                (12, 3_000, at(base, 12), 1, GapBackfill),
                (13, 4_000, at(base, 12), 1, GapBackfill),
                (14, 5_000, at(base, 12), 1, GapBackfill),
            ],
            &[(15, 19)],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&round).unwrap()).await;
        assert_eq!(
            rejections(&receipt),
            vec![None, None, None],
            "the head of a Server-observed hole is admitted without an open gap"
        );
        assert_eq!(
            coverage_intervals(&state, &node_id).await,
            vec![
                (10, 14, "covered".to_owned()),
                (15, 19, "open_recoverable_gap".to_owned()),
                (20, 20, "covered".to_owned()),
            ]
        );
        assert_eq!(
            backfill_gap_rows(&state, &node_id).await,
            vec![(15, 19, false)]
        );

        // A later round backfills the declared rest of the hole.
        let rest: Vec<_> = (15..=19)
            .map(|height| (height, height * 1_000 - 9_000, at(base, 13), 1, GapBackfill))
            .collect();
        let rest = report_with_blocks("0195f2a1-0013-4013-8013-000000000363", 3, &rest);
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&rest).unwrap()).await;
        assert_eq!(rejections(&receipt), vec![None; 5]);
        assert_eq!(
            coverage_intervals(&state, &node_id).await,
            vec![(10, 20, "covered".to_owned())],
            "a fully backfilled hole is one covered interval"
        );
        assert_eq!(
            backfill_gap_rows(&state, &node_id).await,
            vec![(15, 19, true)],
            "the History Gap is resolved once nothing of it is open"
        );
        assert_eq!(cumulative_block_count(&state, &node_id).await, 11);

        // A settled height is a replay again, even after its raw row expired:
        // it is never counted twice.
        sqlx::query("DELETE FROM block_summaries WHERE node_id=$1")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        let replay = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000364",
            4,
            &[(13, 4_000, at(base, 14), 1, GapBackfill)],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;
        assert_eq!(
            rejections(&receipt),
            vec![Some(
                platpulse_core::RejectionCode::GapBackfillOutsideOpenGap
            )]
        );
        assert_eq!(cumulative_block_count(&state, &node_id).await, 11);
    }

    /// Issue #281: the Server-observed hole is bounded by what the Server
    /// itself settled. A height above the High-Water Mark is new history; a
    /// height below the first covered height, or one a `server_rejected` gap
    /// holds (ADR 0011 decision 3), is never admitted.
    #[tokio::test]
    pub(super) async fn a_backfill_is_admitted_only_into_a_hole_the_server_observes() {
        use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let live = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000371",
            1,
            &[
                (10, 1_000, at(base, 1), 1, Subscription),
                (20, 11_000, at(base, 11), 1, Subscription),
            ],
        );
        let node_id = live.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;
        sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, 15, 15, 'server_rejected', 'identity mismatch', '2026-08-12T09:00:12Z', 'server')")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        // The Agent's own copy of an earlier GapBackfillOutsideOpenGap refusal
        // is not the Server's verdict and never makes a hole unrecoverable.
        sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, 17, 17, 'server_rejected', 'GapBackfill sample is outside an explicit open recoverable gap', '2026-08-12T09:00:12Z', 'agent')")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();

        let probe = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000372",
            2,
            &[
                (9, 500, at(base, 12), 1, GapBackfill),
                (14, 5_000, at(base, 12), 1, GapBackfill),
                (15, 6_000, at(base, 12), 1, GapBackfill),
                (17, 8_000, at(base, 12), 1, GapBackfill),
                (21, 12_000, at(base, 12), 1, GapBackfill),
            ],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&probe).unwrap()).await;
        let outside = Some(platpulse_core::RejectionCode::GapBackfillOutsideOpenGap);
        assert_eq!(
            rejections(&receipt),
            vec![outside, None, outside, None, None]
        );
        assert_eq!(
            coverage_intervals(&state, &node_id).await,
            vec![
                (10, 10, "covered".to_owned()),
                (14, 14, "covered".to_owned()),
                (17, 17, "covered".to_owned()),
                (20, 21, "covered".to_owned()),
            ]
        );
        let head: i64 =
            sqlx::query_scalar("SELECT block_number FROM observed_network_heads WHERE node_id=$1")
                .bind(&node_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(head, 20, "a backfill never moves the observed network head");

        // A declared range never opens a height the Server refused itself.
        let declared = report_with_blocks_and_gaps(
            "0195f2a1-0013-4013-8013-000000000373",
            3,
            &[],
            &[(15, 16)],
        );
        submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert!(coverage_intervals(&state, &node_id).await.contains(&(
            16,
            16,
            "open_recoverable_gap".to_owned()
        )));
        assert!(
            !coverage_intervals(&state, &node_id)
                .await
                .iter()
                .any(|(first, last, _)| *first <= 15 && 15 <= *last)
        );
    }

    /// Before #281 a height backfilled into a declared gap only narrowed the
    /// open interval and left no coverage record. Such a height is never a
    /// Server-observed hole, and settling its gap records it covered, so it
    /// cannot be admitted and counted again once its raw summary expires.
    ///
    /// Since #282 a Report settles only the gaps around the heights it closed,
    /// so a legacy gap no Report touches is settled by the retention sweep's
    /// bounded catch-up walk instead.
    #[tokio::test]
    pub(super) async fn a_height_backfilled_before_the_fix_is_never_counted_twice() {
        use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let live = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000391",
            1,
            &[
                (10, 1_000, at(base, 1), 1, Subscription),
                (20, 11_000, at(base, 11), 1, Subscription),
            ],
        );
        let node_id = live.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;
        // The legacy shape: gap 12..=14 declared and fully backfilled, so no
        // interval is left for it, and the raw rows have expired.
        sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, 12, 14, 'unrecoverable_backfill', 'legacy', '2026-08-12T09:00:12Z', 'agent')")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        let count = cumulative_block_count(&state, &node_id).await;

        let probe = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000392",
            2,
            &[
                (13, 4_000, at(base, 12), 1, GapBackfill),
                (16, 7_000, at(base, 12), 1, GapBackfill),
            ],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&probe).unwrap()).await;
        assert_eq!(
            rejections(&receipt),
            vec![
                Some(platpulse_core::RejectionCode::GapBackfillOutsideOpenGap),
                None
            ]
        );
        assert_eq!(cumulative_block_count(&state, &node_id).await, count + 1);
        // The Report closed height 16 only, so it leaves the legacy gap alone.
        assert_eq!(
            backfill_gap_rows(&state, &node_id).await,
            vec![(12, 14, false)]
        );
        // The retention sweep's catch-up walk settles it: its heights are now
        // covered.
        crate::retention::compact_block_coverage(state.db().pool(), crate::auth::now_utc(), &mut 0)
            .await
            .unwrap();
        assert_eq!(
            backfill_gap_rows(&state, &node_id).await,
            vec![(12, 14, true)]
        );
        assert!(coverage_intervals(&state, &node_id).await.contains(&(
            12,
            14,
            "covered".to_owned()
        )));
    }

    /// Issue #281: a declared range that reaches heights the Server already
    /// covered opens only its uncovered stretches, so a later backfill can
    /// never rewrite an accepted height, and the gap resolves as soon as those
    /// stretches are backfilled.
    #[tokio::test]
    pub(super) async fn a_declared_gap_opens_only_its_uncovered_heights() {
        use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let live = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000381",
            1,
            &[
                (10, 1_000, at(base, 1), 1, Subscription),
                (11, 2_000, at(base, 2), 1, Subscription),
                (14, 5_000, at(base, 5), 1, Subscription),
            ],
        );
        let node_id = live.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;
        let declared =
            report_with_blocks_and_gaps("0195f2a1-0013-4013-8013-000000000382", 2, &[], &[(9, 16)]);
        submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert_eq!(
            coverage_intervals(&state, &node_id).await,
            vec![
                (9, 9, "open_recoverable_gap".to_owned()),
                (10, 11, "covered".to_owned()),
                (12, 13, "open_recoverable_gap".to_owned()),
                (14, 14, "covered".to_owned()),
                (15, 16, "open_recoverable_gap".to_owned()),
            ]
        );
        assert_eq!(
            backfill_gap_rows(&state, &node_id).await,
            vec![(9, 16, false)]
        );

        let backfill: Vec<_> = [9, 12, 13, 15, 16]
            .into_iter()
            .map(|height| {
                (
                    height,
                    height * 1_000 - 9_000 + 1,
                    at(base, 20),
                    1,
                    GapBackfill,
                )
            })
            .collect();
        let backfill = report_with_blocks("0195f2a1-0013-4013-8013-000000000383", 3, &backfill);
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&backfill).unwrap()).await;
        assert_eq!(rejections(&receipt), vec![None; 5]);
        assert_eq!(
            coverage_intervals(&state, &node_id).await,
            vec![(9, 16, "covered".to_owned())]
        );
        assert_eq!(
            backfill_gap_rows(&state, &node_id).await,
            vec![(9, 16, true)]
        );
        assert_eq!(cumulative_block_count(&state, &node_id).await, 8);
    }

    /// Issue #248: the buckets are written inside the receipt transaction, so a
    /// Report whose transaction fails after a bucket was already advanced
    /// leaves no bucket - and no Block Summary - behind.
    #[tokio::test]
    pub(super) async fn a_rolled_back_report_leaves_no_block_bucket() {
        use platpulse_core::block::BlockSource::Subscription;
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        // The first block opens the bucket; the second one's update aborts.
        crate::test_support::install_failure_trigger(
            state.db().pool(),
            "abort_block_bucket",
            "BEFORE",
            "UPDATE",
            "node_block_aggregates",
            "injected bucket failure",
        )
        .await
        .unwrap();
        let report = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000331",
            1,
            &[
                (10, 1_000, at(base, 1), 3, Subscription),
                (11, 3_000, at(base, 3), 5, Subscription),
            ],
        );
        let status = submit_status(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_ne!(status, StatusCode::OK);
        assert_eq!(block_bucket_rows(&state).await, 0);
        let summaries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(summaries, 0);

        // Without the injected failure the same Report is accepted once.
        crate::test_support::drop_failure_trigger(
            state.db().pool(),
            "abort_block_bucket",
            "node_block_aggregates",
        )
        .await
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let node_id = report.inventory.nodes[0].node_id.to_string();
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            Some((2, 8, 3, 5, 1, Some(2_000), Some(2_000)))
        );
    }

    #[tokio::test]
    pub(super) async fn repeated_history_gap_declarations_are_deduplicated() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut first: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        first.history_gaps.push(platpulse_core::HistoryGap {
            node_id: first.inventory.nodes[0].node_id,
            kind: platpulse_core::GapKind::UnrecoverableBackfill,
            from_height: 8,
            to_height: 9,
            reason: "bounded recovery exceeded".to_owned(),
            recorded_at: first.generated_at,
        });
        first.report_id = "0195f2a1-0013-4013-8013-000000000107".parse().unwrap();
        first.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;

        let mut replay = first;
        replay.report_sequence = 2;
        replay.report_id = "0195f2a1-0013-4013-8013-000000000108".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM block_history_gaps WHERE node_id=$1 AND from_height=8 AND to_height=9",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    pub(super) async fn divergent_hash_is_append_only_idempotent_and_preserves_summary_and_counters()
     {
        let (_dir, state, _agent_id) = state_with_agent().await;
        let node_id: platpulse_core::identity::NodeId =
            "0195f2a1-0014-4014-8014-000000000014".parse().unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, $2, 'platon-mainnet', 'ws://127.0.0.1:6790', 'active', 'public', 1, $3, $4)")
            .bind(node_id.to_string()).bind("0195f2a1-0011-4011-8011-000000000011").bind("2026-08-12T08:00:00Z").bind("2026-08-12T08:00:00Z").execute(state.db().pool()).await.unwrap();
        let identity = platpulse_core::network::NetworkIdentity {
            genesis_hash: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            chain_id: 210425,
            p2p_network_id: 210425,
            address_hrp: Some("lat".into()),
        };
        let make_sample =
            |block_hash: platpulse_core::hex::Hash32| platpulse_core::block::BlockSummary {
                node_id,
                network_identity: identity.clone(),
                block_number: 42,
                block_hash,
                parent_hash: "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                    .parse()
                    .unwrap(),
                block_timestamp_ms: 1_000,
                observed_at: "2026-08-12T09:00:00Z".parse().unwrap(),
                transaction_count: 3,
                block_interval_ms: None,
                source: platpulse_core::block::BlockSource::Subscription,
                attribution: platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                    "0x1111111111111111111111111111111111111111"
                        .parse()
                        .unwrap(),
                    "test",
                ),
            };
        let hash_a: platpulse_core::hex::Hash32 =
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .parse()
                .unwrap();
        let hash_b: platpulse_core::hex::Hash32 =
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .parse()
                .unwrap();
        let first = make_sample(hash_a.clone());
        let second = make_sample(hash_b);
        let mut tx = crate::database::begin(state.db().pool()).await.unwrap();
        assert!(
            !observe_block_identity(&mut tx, &first, first.observed_at, "2026-08-12T09:00:00Z")
                .await
                .unwrap()
        );
        sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'subscription', $12, 'unknown', 'unknown', 'test', $13)")
            .bind(node_id.to_string()).bind(42_i64).bind(hash_a.to_string()).bind(first.parent_hash.to_string()).bind(identity.genesis_hash.to_string()).bind(210425_i64).bind(210425_i64).bind("lat").bind(1000_i64).bind(first.observed_at.to_string()).bind(3_i64).bind(first.attribution.coinbase.to_string()).bind("2026-08-12T09:00:00Z").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, updated_at) VALUES ($1, 42, 1, 3, 0, $2)").bind(node_id.to_string()).bind("2026-08-12T09:00:00Z").execute(&mut *tx).await.unwrap();
        assert!(
            observe_block_identity(&mut tx, &second, second.observed_at, "2026-08-12T09:01:00Z")
                .await
                .unwrap()
        );
        assert!(
            observe_block_identity(&mut tx, &second, second.observed_at, "2026-08-12T09:01:00Z")
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        let summary_hash: String = sqlx::query_scalar(
            "SELECT block_hash FROM block_summaries WHERE node_id=$1 AND block_number=42",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let counters: (i64, i64) = sqlx::query_as("SELECT cumulative_block_count, cumulative_transaction_count FROM block_history_state WHERE node_id=$1").bind(node_id.to_string()).fetch_one(state.db().pool()).await.unwrap();
        let divergence_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chain_divergence_observations WHERE node_id=$1 AND height=42",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(summary_hash, hash_a.to_string());
        assert_eq!(counters, (1, 3));
        assert_eq!(divergence_count, 1);
    }
}
