//! Block history: Block Summary admission for Report ingestion, and the
//! coverage compaction retention runs. Covered intervals, History Gap opening
//! and History Gap settlement stay behind those two entry points.

use sqlx::{Any, AnyConnection, Transaction};

mod admission;
mod compaction;
mod identity;
#[cfg(test)]
mod tests;

pub(crate) use admission::{NodeAdmission, Refusal, admit};
pub(crate) use compaction::compact;

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
async fn open_uncovered_gap(
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
async fn mark_covered_height(
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
async fn open_gap_containing(
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
async fn backfill_admitted(
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
const GAP_SETTLEMENT_SLOW: std::time::Duration = std::time::Duration::from_millis(500);

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
async fn settle_history_gaps_in_range(
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
struct TouchedRange {
    node_id: String,
    from_height: u64,
    to_height: u64,
}

/// Settle the gaps around every touched range, merging the overlapping and
/// adjacent ranges of one Node first so a run of backfilled heights is one
/// bounded lookup rather than one per height.
async fn settle_touched_history_gaps(
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
async fn settle_history_gaps_after(
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

async fn close_coverage_height(
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
