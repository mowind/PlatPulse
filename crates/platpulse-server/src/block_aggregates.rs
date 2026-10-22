//! Block-series aggregate tiers (issue #248, ADR 0012, webui.md
//! §15.38).
//!
//! Block interval and Transactions / block are derived from `block_summaries`,
//! which has no tier of its own and holds roughly 600,000 rows per Node over
//! seven days. Bucketing those rows for each anonymous Public request would
//! derive a grain at read time and let the request choose its own cost, so the
//! Server keeps write-time tiers instead (migration 0075): the 1-minute and
//! 5-minute buckets of `node_block_aggregates`, advanced inside the receipt
//! transaction that accepted the block.
//!
//! The module has two halves. [`record_accepted_block`] is called by Report
//! Ingestion right after a Block Summary row was inserted - and only then - so
//! the tiers inherit Block history dedup instead of re-implementing it: a plain
//! resync replay, a repeated height and a Replay never reach it, and an
//! `OpenRecoverableGap` backfill reaches it exactly once per height.
//! [`load_block_window`] answers one fixed Public Metric Window from exactly one
//! tier and judges its silences with the same long-history rule the sample
//! series use (`metric_history::judge_fixed_window`).

use sqlx::{Any, AnyPool, Transaction};
use time::OffsetDateTime;

use crate::auth::format_rfc3339;
use crate::metric_history::{
    self, AGGREGATE_GRAINS, GRAIN_RAW, MetricRange, MetricRangePoint, MetricRangeSegment,
    MetricSample, SOURCE_AGGREGATE, SOURCE_RAW, WindowTier,
};

/// One Block Summary the Server has just accepted as new.
#[derive(Debug, Clone, Copy)]
pub struct AcceptedBlock<'a> {
    pub node_id: &'a str,
    pub block_number: i64,
    pub block_timestamp_ms: i64,
    /// The Agent's observation instant: the instant the Live chart plots the
    /// block at, and so the instant its bucket is chosen by.
    pub observed_at: &'a str,
    pub transaction_count: i64,
}

/// The block timestamp and observation instant of one held height of a Node.
const HEIGHT_TIMESTAMP_SQL: &str = "SELECT block_timestamp_ms, observed_at FROM block_summaries WHERE node_id = $1 AND block_number = $2";

/// What a bucket already knows about its own continuity.
const BUCKET_GAP_STATE_SQL: &str = "SELECT first_observed_at, last_observed_at, max_gap_seconds FROM node_block_aggregates WHERE node_id = $1 AND grain_seconds = $2 AND bucket_start = $3";

/// The bucket upsert: one accepted block, with its interval when its parent is
/// already held. Each extreme is folded with a `CASE` comparison (SQL both
/// Storage Backends accept, ADR 0013), and a comparison with a NULL side is
/// never true, so the interval extremes fold through COALESCE: a bucket that held
/// no interval yet takes the arriving one, and an arriving block without one
/// leaves the bucket's as they were.
const BLOCK_UPSERT_SQL: &str = "INSERT INTO node_block_aggregates (node_id, grain_seconds, bucket_start, block_count, transaction_total, transaction_min, transaction_max, last_transaction_count, interval_count, interval_min_ms, interval_max_ms, last_interval_ms, last_interval_observed_at, first_observed_at, last_observed_at, last_received_at, max_gap_seconds, updated_at) VALUES ($1, $2, $3, 1, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) ON CONFLICT (node_id, grain_seconds, bucket_start) DO UPDATE SET block_count = node_block_aggregates.block_count + 1, transaction_total = node_block_aggregates.transaction_total + excluded.transaction_total, transaction_min = CASE WHEN node_block_aggregates.transaction_min <= excluded.transaction_min THEN node_block_aggregates.transaction_min ELSE excluded.transaction_min END, transaction_max = CASE WHEN node_block_aggregates.transaction_max >= excluded.transaction_max THEN node_block_aggregates.transaction_max ELSE excluded.transaction_max END, last_transaction_count = CASE WHEN excluded.last_observed_at >= node_block_aggregates.last_observed_at THEN excluded.last_transaction_count ELSE node_block_aggregates.last_transaction_count END, interval_count = node_block_aggregates.interval_count + excluded.interval_count, interval_min_ms = CASE WHEN COALESCE(node_block_aggregates.interval_min_ms, excluded.interval_min_ms) <= COALESCE(excluded.interval_min_ms, node_block_aggregates.interval_min_ms) THEN COALESCE(node_block_aggregates.interval_min_ms, excluded.interval_min_ms) ELSE COALESCE(excluded.interval_min_ms, node_block_aggregates.interval_min_ms) END, interval_max_ms = CASE WHEN COALESCE(node_block_aggregates.interval_max_ms, excluded.interval_max_ms) >= COALESCE(excluded.interval_max_ms, node_block_aggregates.interval_max_ms) THEN COALESCE(node_block_aggregates.interval_max_ms, excluded.interval_max_ms) ELSE COALESCE(excluded.interval_max_ms, node_block_aggregates.interval_max_ms) END, last_interval_ms = CASE WHEN excluded.last_interval_observed_at IS NOT NULL AND (node_block_aggregates.last_interval_observed_at IS NULL OR excluded.last_interval_observed_at >= node_block_aggregates.last_interval_observed_at) THEN excluded.last_interval_ms ELSE node_block_aggregates.last_interval_ms END, last_interval_observed_at = CASE WHEN excluded.last_interval_observed_at IS NOT NULL AND (node_block_aggregates.last_interval_observed_at IS NULL OR excluded.last_interval_observed_at >= node_block_aggregates.last_interval_observed_at) THEN excluded.last_interval_observed_at ELSE node_block_aggregates.last_interval_observed_at END, first_observed_at = CASE WHEN node_block_aggregates.first_observed_at <= excluded.first_observed_at THEN node_block_aggregates.first_observed_at ELSE excluded.first_observed_at END, last_observed_at = CASE WHEN node_block_aggregates.last_observed_at >= excluded.last_observed_at THEN node_block_aggregates.last_observed_at ELSE excluded.last_observed_at END, last_received_at = CASE WHEN excluded.last_observed_at >= node_block_aggregates.last_observed_at THEN excluded.last_received_at ELSE node_block_aggregates.last_received_at END, max_gap_seconds = CASE WHEN node_block_aggregates.max_gap_seconds >= excluded.max_gap_seconds THEN node_block_aggregates.max_gap_seconds ELSE excluded.max_gap_seconds END, updated_at = excluded.updated_at";

/// Credit the interval of a block that was accepted before its parent, once
/// the parent arrives. Only an existing bucket is updated: a bucket retention
/// has already released is never re-created holding an interval and no block,
/// and the `interval_count < block_count` guard keeps the bucket from claiming
/// more intervals than the blocks it counted.
const CHILD_INTERVAL_SQL: &str = "UPDATE node_block_aggregates SET interval_count = interval_count + 1, interval_min_ms = CASE WHEN COALESCE(interval_min_ms, $1) <= $2 THEN COALESCE(interval_min_ms, $1) ELSE $2 END, interval_max_ms = CASE WHEN COALESCE(interval_max_ms, $3) >= $4 THEN COALESCE(interval_max_ms, $3) ELSE $4 END, last_interval_ms = CASE WHEN last_interval_observed_at IS NULL OR $5 >= last_interval_observed_at THEN $6 ELSE last_interval_ms END, last_interval_observed_at = CASE WHEN last_interval_observed_at IS NULL OR $7 >= last_interval_observed_at THEN $8 ELSE last_interval_observed_at END, updated_at = $9 WHERE node_id = $10 AND grain_seconds = $11 AND bucket_start = $12 AND interval_count < block_count";

/// The interval between a block and its parent, by the Live rule: only a
/// strictly older parent timestamp gives one.
fn interval_ms(block_timestamp_ms: i64, parent_timestamp_ms: i64) -> Option<i64> {
    (block_timestamp_ms > parent_timestamp_ms).then(|| block_timestamp_ms - parent_timestamp_ms)
}

async fn held_block_times(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    block_number: i64,
) -> Result<Option<(i64, String)>, sqlx::Error> {
    sqlx::query_as::<_, (i64, String)>(HEIGHT_TIMESTAMP_SQL)
        .bind(node_id)
        .bind(block_number)
        .fetch_optional(&mut **tx)
        .await
}

/// Accumulate one accepted block into both tiers.
///
/// The caller has just inserted the block's Block Summary in the same receipt
/// transaction and calls this only when that insert added a row, so a block the
/// dedup rules refused - a resync replay at or below the high-water mark, a
/// height already held, a Replay of a whole Report - never reaches the tiers,
/// and a rolled-back transaction leaves no bucket behind.
///
/// The block's interval comes from its parent's held timestamp. A block whose
/// child is already held (it arrived out of order, typically through an
/// `OpenRecoverableGap` backfill) also completes the child's interval, which is
/// credited to the bucket the child was counted in; whichever of the two
/// arrived second counts the interval, so it is counted once.
///
/// An observation instant that is not canonical contributes no bucket (the
/// alignment refuses it); stored instants are canonical by construction.
pub async fn record_accepted_block(
    tx: &mut Transaction<'_, Any>,
    block: AcceptedBlock<'_>,
    received_at: &str,
) -> Result<(), sqlx::Error> {
    let parent = match block.block_number.checked_sub(1) {
        Some(height) if height >= 0 => held_block_times(tx, block.node_id, height).await?,
        _ => None,
    };
    let interval =
        parent.and_then(|(parent_ms, _)| interval_ms(block.block_timestamp_ms, parent_ms));
    for grain_seconds in AGGREGATE_GRAINS {
        let Some(bucket_start) =
            metric_history::aligned_bucket_start(block.observed_at, grain_seconds)
        else {
            continue;
        };
        let stored = sqlx::query_as::<_, (String, String, i64)>(BUCKET_GAP_STATE_SQL)
            .bind(block.node_id)
            .bind(grain_seconds)
            .bind(&bucket_start)
            .fetch_optional(&mut **tx)
            .await?;
        let max_gap_seconds = stored.map_or(0, |(first, last, recorded)| {
            recorded.max(metric_history::candidate_gap_seconds(
                &first,
                &last,
                block.observed_at,
            ))
        });
        sqlx::query(BLOCK_UPSERT_SQL)
            .bind(block.node_id)
            .bind(grain_seconds)
            .bind(&bucket_start)
            .bind(block.transaction_count)
            .bind(block.transaction_count)
            .bind(block.transaction_count)
            .bind(block.transaction_count)
            .bind(i64::from(interval.is_some()))
            .bind(interval)
            .bind(interval)
            .bind(interval)
            .bind(interval.map(|_| block.observed_at))
            .bind(block.observed_at)
            .bind(block.observed_at)
            .bind(received_at)
            .bind(max_gap_seconds)
            .bind(received_at)
            .execute(&mut **tx)
            .await?;
    }
    let Some(child_height) = block.block_number.checked_add(1) else {
        return Ok(());
    };
    let Some((child_ms, child_observed_at)) =
        held_block_times(tx, block.node_id, child_height).await?
    else {
        return Ok(());
    };
    let Some(child_interval) = interval_ms(child_ms, block.block_timestamp_ms) else {
        return Ok(());
    };
    for grain_seconds in AGGREGATE_GRAINS {
        let Some(bucket_start) =
            metric_history::aligned_bucket_start(&child_observed_at, grain_seconds)
        else {
            continue;
        };
        sqlx::query(CHILD_INTERVAL_SQL)
            .bind(child_interval)
            .bind(child_interval)
            .bind(child_interval)
            .bind(child_interval)
            .bind(&child_observed_at)
            .bind(child_interval)
            .bind(&child_observed_at)
            .bind(&child_observed_at)
            .bind(received_at)
            .bind(block.node_id)
            .bind(grain_seconds)
            .bind(&bucket_start)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// One fixed Public window of the two block series.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockWindow {
    /// Block interval, in milliseconds: raw blocks with a known parent, or the
    /// buckets that counted at least one interval.
    pub block_interval_ms: MetricRange,
    /// Transactions per block: raw blocks, or every bucket.
    pub transaction_count: MetricRange,
    /// The transaction total of each `transaction_count` point, in the same
    /// order: `Some` for a bucket, `None` for a raw block, whose own count is
    /// its value.
    pub transaction_totals: Vec<Option<i64>>,
}

/// Everything the block window reader needs.
#[derive(Debug, Clone, Copy)]
pub struct BlockWindowQuery<'a> {
    pub node_id: &'a str,
    /// The window's older end, inclusive; a bucket boundary for a bucket tier.
    pub from: OffsetDateTime,
    /// The window's newer end, inclusive.
    pub to: OffsetDateTime,
    pub tier: WindowTier,
    /// The largest number of points a series may carry; the newest are kept.
    pub limit: i64,
}

/// The held Block Summaries one Node observed inside the window, newest first so
/// the limit is spent on the newest evidence, with their Live-rule interval to
/// the held parent.
///
/// The range is read through `block_summaries_observed_idx` (migration 0075),
/// so it is bounded by the window itself: a block backfilled at a low height is
/// found by the instant it was observed, and a quiet Node is never scanned.
///
/// The index is left to the planner: `INDEXED BY` is SQLite-only, and the
/// portable SQL the two Storage Backends share cannot name an index (ADR 0013).
const RAW_BLOCK_SQL: &str = "SELECT current.observed_at, current.accepted_at, CASE WHEN previous.block_timestamp_ms IS NOT NULL AND current.block_timestamp_ms > previous.block_timestamp_ms THEN current.block_timestamp_ms - previous.block_timestamp_ms ELSE NULL END, current.transaction_count FROM block_summaries AS current LEFT JOIN block_summaries AS previous ON previous.node_id = current.node_id AND previous.block_number = current.block_number - 1 WHERE current.node_id = $1 AND current.observed_at >= $2 AND current.observed_at <= $3 ORDER BY current.observed_at DESC, current.block_number DESC LIMIT $4";

/// The buckets of one tier over one stretch, newest first so the limit is spent
/// on the newest evidence. Served by the primary key.
const RANGE_BUCKET_SQL: &str = "SELECT bucket_start, block_count, transaction_total, transaction_min, transaction_max, last_transaction_count, interval_count, interval_min_ms, interval_max_ms, last_interval_ms, first_observed_at, last_observed_at, last_received_at, max_gap_seconds FROM node_block_aggregates WHERE node_id = $1 AND grain_seconds = $2 AND bucket_start >= $3 AND bucket_start < $4 ORDER BY bucket_start DESC LIMIT $5";

type BucketRow = (
    String,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    String,
    String,
    String,
    i64,
);

fn raw_point(
    observed_at: &str,
    received_at: &str,
    value: f64,
    cadence_seconds: i64,
) -> MetricRangePoint {
    MetricRangePoint {
        instant: observed_at.to_owned(),
        grain: GRAIN_RAW,
        source: SOURCE_RAW,
        value,
        min_value: value,
        max_value: value,
        sample_count: 1,
        last_observed_at: observed_at.to_owned(),
        received_at: received_at.to_owned(),
        first_observed_at: observed_at.to_owned(),
        max_gap_seconds: 0,
        cadence_seconds,
        judged_cadence_seconds: 0,
    }
}

fn judged_range(
    mut points: Vec<MetricRangePoint>,
    tier: WindowTier,
    from: &str,
    to: &str,
    truncated: bool,
) -> MetricRange {
    // Low-space protection never pauses Block Summaries (it pauses optional
    // metric history), so no protection pause can explain a block silence:
    // every silence is a collection gap.
    let continuity = metric_history::judge_fixed_window(&mut points, &[], from, to);
    let grain_seconds = tier.grain_seconds();
    let segments = vec![MetricRangeSegment {
        grain: tier.grain_label(),
        source: if grain_seconds == 0 {
            SOURCE_RAW
        } else {
            SOURCE_AGGREGATE
        },
        from: from.to_owned(),
        to: to.to_owned(),
        point_count: points.len() as i64,
        truncated,
    }];
    MetricRange {
        points,
        segments,
        // The block series have no value-less ledger: block_summaries is
        // their only record (see `public::metric_series_has_ledger`).
        ledger: None,
        gaps: continuity.gaps,
        coverage_seconds: continuity.coverage_seconds,
        truncated,
        continuation: None,
    }
}

/// Answer one fixed window of both block series from exactly one tier
/// (ADR 0012, issue #248).
///
/// Raw (the 1-hour preset) reads the newest held Block Summaries of the Node,
/// with the interval derived from the parent row as the Live answer does. A
/// bucket tier reads only what the Server wrote while the blocks arrived: a
/// bucket's interval point is drawn by its maximum interval, its transaction
/// point by the most transactions one block of the bucket carried, and a
/// stretch the tier holds nothing for - including everything from before
/// migration 0075, which backfilled nothing - is answered as unobserved.
pub async fn load_block_window(
    pool: &AnyPool,
    query: BlockWindowQuery<'_>,
) -> Result<BlockWindow, sqlx::Error> {
    let limit = query.limit.max(0);
    let from = format_rfc3339(query.from);
    let to = format_rfc3339(query.to);
    if from > to {
        return Ok(BlockWindow {
            block_interval_ms: judged_range(Vec::new(), query.tier, &from, &to, false),
            transaction_count: judged_range(Vec::new(), query.tier, &from, &to, false),
            transaction_totals: Vec::new(),
        });
    }
    let grain_seconds = query.tier.grain_seconds();
    if grain_seconds == 0 {
        let mut rows = sqlx::query_as::<_, (String, String, Option<i64>, i64)>(RAW_BLOCK_SQL)
            .bind(query.node_id)
            .bind(&from)
            .bind(&to)
            .bind(limit + 1)
            .fetch_all(pool)
            .await?;
        // More blocks inside the window than the answer may carry: the oldest
        // fall away, which the segment states.
        let truncated = rows.len() as i64 > limit;
        rows.truncate(limit as usize);
        rows.reverse();
        let samples = |values: Vec<(&String, &String, f64)>| -> Vec<MetricSample> {
            values
                .into_iter()
                .map(|(observed_at, received_at, value)| MetricSample {
                    observed_at: observed_at.clone(),
                    received_at: received_at.clone(),
                    value,
                })
                .collect()
        };
        let intervals = samples(
            rows.iter()
                .filter_map(|(observed, accepted, interval, _)| {
                    interval.map(|value| (observed, accepted, value as f64))
                })
                .collect(),
        );
        let transactions = samples(
            rows.iter()
                .map(|(observed, accepted, _, count)| (observed, accepted, *count as f64))
                .collect(),
        );
        let points = |samples: Vec<MetricSample>| -> Vec<MetricRangePoint> {
            let cadence = metric_history::observed_cadence_seconds(&samples);
            samples
                .iter()
                .map(|sample| {
                    raw_point(
                        &sample.observed_at,
                        &sample.received_at,
                        sample.value,
                        cadence,
                    )
                })
                .collect()
        };
        let interval_points = points(intervals);
        let transaction_points = points(transactions);
        let totals = vec![None; transaction_points.len()];
        return Ok(BlockWindow {
            block_interval_ms: judged_range(interval_points, query.tier, &from, &to, truncated),
            transaction_count: judged_range(transaction_points, query.tier, &from, &to, truncated),
            transaction_totals: totals,
        });
    }
    // The bucket table bounds a tier by bucket start with an exclusive ceiling;
    // the window's own end is inclusive, so a bucket that opened exactly at
    // `to` is answered.
    let ceiling = format_rfc3339(query.to + time::Duration::seconds(1));
    let mut rows = sqlx::query_as::<_, BucketRow>(RANGE_BUCKET_SQL)
        .bind(query.node_id)
        .bind(grain_seconds)
        .bind(&from)
        .bind(&ceiling)
        .bind(limit + 1)
        .fetch_all(pool)
        .await?;
    let truncated = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    rows.reverse();
    let mut interval_points = Vec::new();
    let mut transaction_points = Vec::with_capacity(rows.len());
    let mut totals = Vec::with_capacity(rows.len());
    for (
        bucket_start,
        block_count,
        transaction_total,
        transaction_min,
        transaction_max,
        last_transaction_count,
        interval_count,
        interval_min_ms,
        interval_max_ms,
        last_interval_ms,
        first_observed_at,
        last_observed_at,
        last_received_at,
        max_gap_seconds,
    ) in rows
    {
        let bucket_point = |value: i64, min: i64, max: i64, count: i64| MetricRangePoint {
            instant: bucket_start.clone(),
            grain: query.tier.grain_label(),
            source: SOURCE_AGGREGATE,
            value: value as f64,
            min_value: min as f64,
            max_value: max as f64,
            sample_count: count,
            last_observed_at: last_observed_at.clone(),
            received_at: last_received_at.clone(),
            first_observed_at: first_observed_at.clone(),
            max_gap_seconds,
            cadence_seconds: grain_seconds,
            judged_cadence_seconds: 0,
        };
        if let (true, Some(min), Some(max), Some(last)) = (
            interval_count > 0,
            interval_min_ms,
            interval_max_ms,
            last_interval_ms,
        ) {
            interval_points.push(bucket_point(last, min, max, interval_count));
        }
        transaction_points.push(bucket_point(
            last_transaction_count,
            transaction_min,
            transaction_max,
            block_count,
        ));
        totals.push(Some(transaction_total));
    }
    Ok(BlockWindow {
        block_interval_ms: judged_range(interval_points, query.tier, &from, &to, truncated),
        transaction_count: judged_range(transaction_points, query.tier, &from, &to, truncated),
        transaction_totals: totals,
    })
}
