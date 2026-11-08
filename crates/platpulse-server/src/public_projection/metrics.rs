//! The Public metric answer for one Node: the Live window of raw samples and
//! the longer Metric Windows read from their tiers, each series with the
//! coverage that says what its points can and cannot show.

use std::collections::HashMap;

use time::OffsetDateTime;

use crate::auth::format_rfc3339;
use crate::database::ServerDatabase;
use crate::http::public::{
    PUBLIC_NODE_METRIC_WINDOW_SECONDS, PublicMetricBucket, PublicMetricGap, PublicMetricPoint,
    PublicMetricSeriesCoverage, PublicMetricWindow, PublicMetricWindowAnswer,
    PublicNodeMetricHistory,
};

/// The most points one series of a longer Metric Window may carry. The
/// presets bound every answer by themselves - an hour of one-second raw
/// samples is 3,601 points and seven days of 5-minute buckets 2,017 - so the
/// limit is a guard, not a page size, and when it applies the newest are kept.
const PUBLIC_METRIC_WINDOW_POINT_LIMIT: i64 = 4_000;

#[derive(Debug, sqlx::FromRow)]
struct MetricHistoryRow {
    metric: String,
    sampled_at: String,
    value: f64,
}

#[derive(Debug, sqlx::FromRow)]
struct BlockMetricHistoryRow {
    sampled_at: String,
    block_interval_ms: Option<i64>,
    transaction_count: i64,
}

/// The series ledger's answer for one metric: when the series was first and
/// last observed, and no metric value at all.
///
/// Raw samples expire and the ledger does not ([crate::retention]), which is
/// what lets a Public answer keep telling a series that was observed and then
/// went quiet apart from one nobody ever observed (issue #225, Story 49). It is
/// Public-safe precisely because it holds instants instead of evidence: the
/// chart of an expired series still has nothing to draw.
#[derive(Debug, sqlx::FromRow)]
struct MetricSeriesLedgerRow {
    metric: String,
    first_observed_at: String,
    last_observed_at: String,
}

fn push_metric_point(history: &mut PublicNodeMetricHistory, row: MetricHistoryRow) {
    let point = PublicMetricPoint {
        sampled_at: row.sampled_at,
        value: row.value,
        bucket: None,
    };
    match row.metric.as_str() {
        "process_cpu_percent" => history.process_cpu_percent.push(point),
        "process_memory_percent" => history.process_memory_percent.push(point),
        "data_directory_percent" => history.data_directory_percent.push(point),
        "network_rx_bytes_per_sec" => history.network_rx_bytes_per_sec.push(point),
        "network_tx_bytes_per_sec" => history.network_tx_bytes_per_sec.push(point),
        "peer_inbound_count" => history.peer_inbound_count.push(point),
        "peer_outbound_count" => history.peer_outbound_count.push(point),
        _ => {}
    }
}

/// The stored metric names this answer reads, and the response field each one
/// belongs to.
///
/// One table, because its two readers must not drift: the series are named by
/// the stored name their samples and their ledger row are written under, and the
/// value-less observation ledger answers for exactly those series. The two block
/// series are deliberately absent - they live in `block_summaries`, which
/// retention removes and which has no ledger beside it - which is what
/// `metric_series_has_ledger` reports (issue #225, Story 49).
const METRIC_STORED_NAMES: [(&str, &str); 7] = [
    ("process_cpu_percent", "processCpuPercent"),
    ("process_memory_percent", "processMemoryPercent"),
    ("data_directory_percent", "dataDirectoryPercent"),
    ("network_rx_bytes_per_sec", "networkRxBytesPerSec"),
    ("network_tx_bytes_per_sec", "networkTxBytesPerSec"),
    ("peer_inbound_count", "peerInboundCount"),
    ("peer_outbound_count", "peerOutboundCount"),
];

/// The response field a stored metric name belongs to, when this answer carries
/// it. Samples and the series ledger are both written under the stored name,
/// while the response and its coverage speak in field names.
fn public_metric_name(metric: &str) -> Option<&'static str> {
    METRIC_STORED_NAMES
        .iter()
        .find(|(stored, _)| *stored == metric)
        .map(|(_, name)| *name)
}

/// Whether the Server keeps a value-less observation ledger for a response
/// field: it does for every series stored under a metric name, whose ledger row
/// outlives the samples retention removes, and it does not for the two block
/// series. A series without a ledger keeps no evidence of its own absence, so
/// its coverage reports `neverObservedProven` false instead of claiming that
/// nobody ever observed it (issue #225, Story 49).
fn metric_series_has_ledger(metric: &str) -> bool {
    METRIC_STORED_NAMES.iter().any(|(_, name)| *name == metric)
}

/// The nine series this answer carries, named the way the response names them.
pub(crate) const PUBLIC_METRIC_SERIES: [&str; 9] = [
    "processCpuPercent",
    "processMemoryPercent",
    "dataDirectoryPercent",
    "networkRxBytesPerSec",
    "networkTxBytesPerSec",
    "peerInboundCount",
    "peerOutboundCount",
    "blockIntervalMs",
    "transactionCount",
];

/// The sample series of the response a longer window fills, by response field
/// name. The two block series are not among them: outside Live they are absent.
fn public_metric_points_mut<'a>(
    history: &'a mut PublicNodeMetricHistory,
    metric: &str,
) -> Option<&'a mut Vec<PublicMetricPoint>> {
    Some(match metric {
        "processCpuPercent" => &mut history.process_cpu_percent,
        "processMemoryPercent" => &mut history.process_memory_percent,
        "dataDirectoryPercent" => &mut history.data_directory_percent,
        "networkRxBytesPerSec" => &mut history.network_rx_bytes_per_sec,
        "networkTxBytesPerSec" => &mut history.network_tx_bytes_per_sec,
        "peerInboundCount" => &mut history.peer_inbound_count,
        "peerOutboundCount" => &mut history.peer_outbound_count,
        "blockIntervalMs" => &mut history.block_interval_ms,
        "transactionCount" => &mut history.transaction_count,
        _ => return None,
    })
}

/// The observations a series of the response carries, by response field name.
fn public_metric_points<'a>(
    history: &'a PublicNodeMetricHistory,
    metric: &str,
) -> Option<&'a [PublicMetricPoint]> {
    Some(match metric {
        "processCpuPercent" => &history.process_cpu_percent,
        "processMemoryPercent" => &history.process_memory_percent,
        "dataDirectoryPercent" => &history.data_directory_percent,
        "networkRxBytesPerSec" => &history.network_rx_bytes_per_sec,
        "networkTxBytesPerSec" => &history.network_tx_bytes_per_sec,
        "peerInboundCount" => &history.peer_inbound_count,
        "peerOutboundCount" => &history.peer_outbound_count,
        "blockIntervalMs" => &history.block_interval_ms,
        "transactionCount" => &history.transaction_count,
        _ => return None,
    })
}

/// A sample for the cadence rule alone.
///
/// The cadence reads the observation instants and nothing else, and the public
/// payload carries no reception time, so the instant stands in for both.
fn cadence_sample(point: &PublicMetricPoint) -> crate::metric_history::MetricSample {
    crate::metric_history::MetricSample {
        observed_at: point.sampled_at.clone(),
        received_at: point.sampled_at.clone(),
        value: point.value,
    }
}

/// The seconds between the newest observation and the end of the window.
///
/// A series with no observation at all reports 0: it has no last-good point to
/// be quiet after, and `observed` says which case this is. A series whose newest
/// observation is the older last-good point reports the real distance, which can
/// be longer than the window - that is how long it has really been quiet.
fn unobserved_tail_seconds(last_observed_at: Option<&str>, to: &str) -> i64 {
    let (Some(last), Some(end)) = (
        last_observed_at.and_then(crate::auth::parse_rfc3339),
        crate::auth::parse_rfc3339(to),
    ) else {
        return 0;
    };
    (end - last).whole_seconds().max(0)
}

/// Judge one series of the latest-60-second answer.
///
/// The response carries one observation older than the window on purpose - the
/// last-good value the chart already draws - so the count, the silences and the
/// coverage are about the window itself, while the older point still dates the
/// series and still speaks about its rhythm: a series that went quiet before the
/// window and came back inside it has changed cadence, and the newest interval
/// alone would call that recovered series comfortable. The older point is never
/// fed to the continuity rule, though, because the answer may only report
/// silences of the window it was asked about, and a silence that began before
/// the window starts is not one of them.
///
/// Raw samples expire and the ledger does not, so the series is dated from the
/// answer's own observations while it carries any and from the ledger when it
/// carries none: `observed` is about what the Server recorded, not about what
/// it still retains (issue #225, Story 49).
fn public_metric_coverage(
    metric: &str,
    points: &[PublicMetricPoint],
    from: &str,
    to: &str,
    ledger: Option<&MetricSeriesLedgerRow>,
) -> PublicMetricSeriesCoverage {
    let inside: Vec<&PublicMetricPoint> = points
        .iter()
        .filter(|point| point.sampled_at.as_str() >= from && point.sampled_at.as_str() <= to)
        .collect();
    // Only an instant the window has already reached may date this series. An
    // Agent whose clock runs ahead can stamp an observation the window cannot
    // hold yet, and such an instant may neither end the series nor end its
    // tail: the answer would then report a silence nobody has been through and
    // hide the last-good point it really has (issue #225).
    let last_observed_at = points
        .iter()
        .filter(|point| point.sampled_at.as_str() <= to)
        .max_by(|left, right| left.sampled_at.cmp(&right.sampled_at))
        .map(|point| point.sampled_at.clone());
    let first_observed_at = points
        .iter()
        .filter(|point| point.sampled_at.as_str() <= to)
        .min_by(|left, right| left.sampled_at.cmp(&right.sampled_at))
        .map(|point| point.sampled_at.clone());
    // The ledger speaks only when the samples cannot: a series whose raw
    // samples have all expired is still a series that was observed, and
    // answering it as one nobody ever observed would be a different and untrue
    // statement. An instant the window has not reached is not a date either,
    // ledger or not.
    let (first_observed_at, last_observed_at) = match (first_observed_at, last_observed_at) {
        (Some(first), Some(last)) => (Some(first), Some(last)),
        _ => match ledger.filter(|row| row.last_observed_at.as_str() <= to) {
            Some(row) => (
                Some(row.first_observed_at.clone()),
                Some(row.last_observed_at.clone()),
            ),
            None => (None, None),
        },
    };
    let mut cadence_samples: Vec<crate::metric_history::MetricSample> =
        Vec::with_capacity(inside.len() + 1);
    if let Some(previous) = points
        .iter()
        .filter(|point| point.sampled_at.as_str() < from)
        .max_by(|left, right| left.sampled_at.cmp(&right.sampled_at))
    {
        cadence_samples.push(cadence_sample(previous));
    }
    cadence_samples.extend(inside.iter().map(|point| cadence_sample(point)));
    let cadence_seconds = crate::metric_history::current_cadence_seconds(&cadence_samples);
    let (gaps, covered_seconds) = if cadence_seconds > 0 {
        let inside_samples: Vec<crate::metric_history::MetricSample> =
            inside.iter().map(|point| cadence_sample(point)).collect();
        let continuity = crate::metric_history::continuity_with_floor(
            &inside_samples,
            &[],
            cadence_seconds,
            from,
            to,
            0,
        );
        (continuity.gaps, continuity.coverage_seconds)
    } else {
        // No rhythm, no silence, and no coverage either: the stretch between the
        // outermost observations is not observed just because both ends were,
        // and claiming it would call the middle of a silence covered.
        (Vec::new(), 0)
    };
    PublicMetricSeriesCoverage {
        metric: metric.to_owned(),
        observed: last_observed_at.is_some(),
        never_observed_proven: metric_series_has_ledger(metric),
        observation_count: inside.len() as i64,
        first_observed_at,
        last_observed_at: last_observed_at.clone(),
        cadence_seconds,
        gap_threshold_seconds: if cadence_seconds > 0 {
            crate::metric_history::gap_threshold_seconds_with_floor(cadence_seconds, 0)
        } else {
            0
        },
        covered_seconds,
        unobserved_tail_seconds: unobserved_tail_seconds(last_observed_at.as_deref(), to),
        gaps: gaps
            .into_iter()
            .map(|gap| PublicMetricGap {
                from: gap.from,
                to: gap.to,
                seconds: gap.seconds,
                kind: gap.kind.as_str().to_owned(),
            })
            .collect(),
    }
}

/// Answer the Live window: the latest 60 seconds of raw samples ending at
/// `at`, each series led by its last-good point before the window.
pub(super) async fn live_metrics(
    db: &ServerDatabase,
    node_id: &str,
    agent_id: &str,
    at: OffsetDateTime,
) -> Result<PublicNodeMetricHistory, sqlx::Error> {
    let to = at;
    let from = to - time::Duration::seconds(PUBLIC_NODE_METRIC_WINDOW_SECONDS);
    let from = format_rfc3339(from);
    let to = format_rfc3339(to);
    let metric_rows = sqlx::query_as::<_, MetricHistoryRow>(
        "SELECT metric, observed_at AS sampled_at, value FROM node_metric_samples AS current WHERE node_id=$1 AND observed_at<=$2 AND (received_at>=$3 OR received_at=(SELECT MAX(received_at) FROM node_metric_samples AS previous WHERE previous.node_id=current.node_id AND previous.metric=current.metric AND previous.observed_at<=$4 AND previous.received_at<$5)) UNION ALL SELECT metric, observed_at AS sampled_at, value FROM host_metric_samples AS current WHERE agent_id=$6 AND observed_at<=$7 AND metric IN ('network_rx_bytes_per_sec', 'network_tx_bytes_per_sec') AND (received_at>=$8 OR received_at=(SELECT MAX(received_at) FROM host_metric_samples AS previous WHERE previous.agent_id=current.agent_id AND previous.metric=current.metric AND previous.observed_at<=$9 AND previous.received_at<$10)) ORDER BY sampled_at, metric",
    )
    .bind(node_id)
    .bind(&to)
    .bind(&from)
    .bind(&to)
    .bind(&from)
    .bind(agent_id)
    .bind(&to)
    .bind(&from)
    .bind(&to)
    .bind(&from)
    .fetch_all(db.pool())
    .await?;
    let mut block_rows = sqlx::query_as::<_, BlockMetricHistoryRow>(
        "SELECT current.observed_at AS sampled_at, CASE WHEN previous.block_timestamp_ms IS NOT NULL AND current.block_timestamp_ms > previous.block_timestamp_ms THEN current.block_timestamp_ms - previous.block_timestamp_ms ELSE NULL END AS block_interval_ms, current.transaction_count FROM block_summaries AS current LEFT JOIN block_summaries AS previous ON previous.node_id=current.node_id AND previous.block_number=current.block_number-1 WHERE current.node_id=$1 AND current.observed_at<=$2 ORDER BY current.block_number DESC LIMIT 200",
    )
    .bind(node_id)
    .bind(&to)
    .fetch_all(db.pool())
    .await?;
    // What the samples can no longer say. The ledger holds one row per series
    // and no value, and retention never prunes it, so an expired series is still
    // dated here instead of being answered as one nobody observed (#225).
    let ledger_rows = sqlx::query_as::<_, MetricSeriesLedgerRow>(
        "SELECT metric, MIN(first_observed_at) AS first_observed_at, MAX(last_observed_at) AS last_observed_at FROM node_metric_series_state WHERE node_id=$1 GROUP BY metric UNION ALL SELECT metric, MIN(first_observed_at) AS first_observed_at, MAX(last_observed_at) AS last_observed_at FROM host_metric_series_state WHERE agent_id=$2 AND metric IN ('network_rx_bytes_per_sec', 'network_tx_bytes_per_sec') GROUP BY metric",
    )
    .bind(node_id)
    .bind(agent_id)
    .fetch_all(db.pool())
    .await?;

    let mut history = PublicNodeMetricHistory {
        from: from.clone(),
        to: to.clone(),
        window_seconds: PUBLIC_NODE_METRIC_WINDOW_SECONDS,
        process_cpu_percent: Vec::new(),
        process_memory_percent: Vec::new(),
        data_directory_percent: Vec::new(),
        network_rx_bytes_per_sec: Vec::new(),
        network_tx_bytes_per_sec: Vec::new(),
        peer_inbound_count: Vec::new(),
        peer_outbound_count: Vec::new(),
        block_interval_ms: Vec::new(),
        transaction_count: Vec::new(),
        series: Vec::new(),
        metric_window: None,
    };
    for row in metric_rows {
        push_metric_point(&mut history, row);
    }
    let mut block_interval_ms = Vec::new();
    let mut transaction_count = Vec::new();

    // Height is not time here. A block accepted late keeps its own height but
    // carries the instant it was really observed, so ordering by height can end
    // the series at an instant that is not the newest one, and with it date the
    // whole series - first, last and tail - wrongly (issue #225).
    block_rows.sort_by(|left, right| left.sampled_at.cmp(&right.sampled_at));
    let mut prior_interval = None;
    let mut prior_transactions = None;
    for row in block_rows {
        let interval = row.block_interval_ms.map(|value| PublicMetricPoint {
            sampled_at: row.sampled_at.clone(),
            value: value as f64,
            bucket: None,
        });
        let transactions = PublicMetricPoint {
            sampled_at: row.sampled_at.clone(),
            value: row.transaction_count as f64,
            bucket: None,
        };
        if row.sampled_at < from {
            if interval.is_some() {
                prior_interval = interval;
            }
            prior_transactions = Some(transactions);
        } else {
            if let Some(interval) = interval {
                block_interval_ms.push(interval);
            }
            transaction_count.push(transactions);
        }
    }
    if let Some(point) = prior_interval {
        block_interval_ms.insert(0, point);
    }
    if let Some(point) = prior_transactions {
        transaction_count.insert(0, point);
    }
    history.block_interval_ms = block_interval_ms;
    history.transaction_count = transaction_count;
    // The coverage is judged after the payload is whole: the rule answers about
    // the observations this answer really carries, and the last-good point that
    // precedes the window is one of them.
    let ledger: HashMap<&str, &MetricSeriesLedgerRow> = ledger_rows
        .iter()
        .filter_map(|row| public_metric_name(&row.metric).map(|name| (name, row)))
        .collect();
    let series = PUBLIC_METRIC_SERIES
        .iter()
        .filter_map(|metric| {
            public_metric_points(&history, metric).map(|points| {
                public_metric_coverage(metric, points, &from, &to, ledger.get(*metric).copied())
            })
        })
        .collect();
    history.series = series;

    Ok(history)
}

/// The older end of a longer window. A bucket tier starts at a bucket edge, so
/// the oldest bucket the window holds is answered whole rather than straddling
/// the start, and the axis still spans the whole requested window.
fn window_start(window: PublicMetricWindow, to: time::OffsetDateTime) -> time::OffsetDateTime {
    let from = to - time::Duration::seconds(window.seconds());
    let grain = window.tier().grain_seconds();
    if grain <= 0 {
        return from;
    }
    let aligned = from.unix_timestamp().div_euclid(grain) * grain;
    time::OffsetDateTime::from_unix_timestamp(aligned).unwrap_or(from)
}

/// One point of a longer window: a raw observation as itself, a bucket by its
/// maximum with the rest of the bucket beside it. A bucket whose start is not
/// a usable instant has no interval to state and no place on the axis, so it is
/// left out rather than answered with an empty end.
fn window_point(
    point: crate::metric_history::MetricRangePoint,
    to: time::OffsetDateTime,
) -> Option<PublicMetricPoint> {
    if point.grain == crate::metric_history::GRAIN_RAW {
        return Some(PublicMetricPoint {
            sampled_at: point.instant,
            value: point.value,
            bucket: None,
        });
    }
    // A bucket point's cadence is its own width (`bucket_range_point`).
    let grain_seconds = point.cadence_seconds;
    let end = crate::observation_time::shift_instant(&point.instant, grain_seconds)?;
    Some(PublicMetricPoint {
        sampled_at: point.instant.clone(),
        value: point.max_value,
        bucket: Some(PublicMetricBucket {
            start: point.instant,
            end: format_rfc3339(end),
            grain_seconds,
            min_value: point.min_value,
            max_value: point.max_value,
            last_value: point.value,
            sample_count: point.sample_count,
            first_observed_at: point.first_observed_at,
            last_observed_at: point.last_observed_at,
            open: end > to,
            total_value: None,
        }),
    })
}

/// Judge one series of a longer window from the fixed-tier answer.
///
/// The silences and the coverage are the Admin long-history rule's own
/// (`crate::metric_history::load_window`); this only states them in the
/// Public coverage vocabulary. Dating follows the Live rule: the answer's own
/// points while it carries any, the value-less ledger when it carries none, and
/// never an instant the window has not reached.
fn public_window_coverage(
    metric: &str,
    range: &crate::metric_history::MetricRange,
    tier: crate::metric_history::WindowTier,
    to: &str,
) -> PublicMetricSeriesCoverage {
    let reached: Vec<&crate::metric_history::MetricRangePoint> = range
        .points
        .iter()
        .filter(|point| point.first_observed_at.as_str() <= to)
        .collect();
    let first_observed_at = reached
        .iter()
        .map(|point| point.first_observed_at.as_str())
        .min()
        .map(str::to_owned);
    let last_observed_at = reached
        .iter()
        .map(|point| point.last_observed_at.as_str())
        .filter(|instant| *instant <= to)
        .max()
        .map(str::to_owned);
    let (first_observed_at, last_observed_at) = match (first_observed_at, last_observed_at) {
        (Some(first), Some(last)) => (Some(first), Some(last)),
        _ => match range
            .ledger
            .as_ref()
            .filter(|ledger| ledger.last_observed_at.as_str() <= to)
        {
            Some(ledger) => (
                Some(ledger.first_observed_at.clone()),
                Some(ledger.last_observed_at.clone()),
            ),
            None => (None, None),
        },
    };
    let grain_seconds = tier.grain_seconds();
    let cadence_seconds = if grain_seconds == 0 {
        range
            .points
            .first()
            .map_or(0, |point| point.cadence_seconds)
    } else {
        range
            .points
            .last()
            .map_or(0, |point| point.judged_cadence_seconds)
    };
    PublicMetricSeriesCoverage {
        metric: metric.to_owned(),
        observed: last_observed_at.is_some(),
        never_observed_proven: metric_series_has_ledger(metric),
        observation_count: range.points.iter().map(|point| point.sample_count).sum(),
        first_observed_at,
        last_observed_at: last_observed_at.clone(),
        cadence_seconds,
        gap_threshold_seconds: if cadence_seconds > 0 {
            crate::metric_history::window_silence_threshold_seconds(cadence_seconds, grain_seconds)
        } else {
            0
        },
        covered_seconds: range.coverage_seconds,
        unobserved_tail_seconds: unobserved_tail_seconds(last_observed_at.as_deref(), to),
        gaps: range
            .gaps
            .iter()
            .map(|gap| PublicMetricGap {
                from: gap.from.clone(),
                to: gap.to.clone(),
                seconds: gap.seconds,
                kind: gap.kind.as_str().to_owned(),
            })
            .collect(),
    }
}

/// Answer a longer Metric Window (ADR 0012, issue #247).
///
/// Every series is read from the one tier the preset fixes: the seven sample
/// series from the metric tiers, the two block series from the block tiers
/// (issue #248). The window ends at `at`, so a later request answers a later
/// window.
pub(super) async fn window_metrics(
    db: &ServerDatabase,
    node_id: &str,
    agent_id: &str,
    window: PublicMetricWindow,
    at: OffsetDateTime,
) -> Result<PublicNodeMetricHistory, sqlx::Error> {
    let now = at;
    let to_instant = time::OffsetDateTime::from_unix_timestamp(now.unix_timestamp()).unwrap_or(now);
    let from_instant = window_start(window, to_instant);
    let from = format_rfc3339(from_instant);
    let to = format_rfc3339(to_instant);
    let tier = window.tier();
    let mut history = PublicNodeMetricHistory {
        from: from.clone(),
        to: to.clone(),
        window_seconds: window.seconds(),
        process_cpu_percent: Vec::new(),
        process_memory_percent: Vec::new(),
        data_directory_percent: Vec::new(),
        network_rx_bytes_per_sec: Vec::new(),
        network_tx_bytes_per_sec: Vec::new(),
        peer_inbound_count: Vec::new(),
        peer_outbound_count: Vec::new(),
        block_interval_ms: Vec::new(),
        transaction_count: Vec::new(),
        series: Vec::new(),
        metric_window: Some(PublicMetricWindowAnswer {
            window,
            grain: tier.grain_label().to_owned(),
            grain_seconds: tier.grain_seconds(),
            unavailable_series: Vec::new(),
        }),
    };
    // The same seven sample series the Live answer reads, named by the one
    // table both answers share; a Host series belongs to the Node's Agent.
    for (stored, name) in METRIC_STORED_NAMES {
        let scope = if crate::metric_history::is_node_metric(stored) {
            crate::metric_history::SeriesScope::node(node_id, stored)
        } else {
            crate::metric_history::SeriesScope::host(agent_id, stored, "")
        };
        let range = crate::metric_history::load_window(
            db.pool(),
            crate::metric_history::WindowQuery {
                scope,
                from: from_instant,
                to: to_instant,
                tier,
                limit: PUBLIC_METRIC_WINDOW_POINT_LIMIT,
            },
        )
        .await?;
        history
            .series
            .push(public_window_coverage(name, &range, tier, &to));
        if let Some(series) = public_metric_points_mut(&mut history, name) {
            series.extend(
                range
                    .points
                    .into_iter()
                    .filter_map(|point| window_point(point, to_instant)),
            );
        }
    }
    let blocks = crate::block_aggregates::load_block_window(
        db.pool(),
        crate::block_aggregates::BlockWindowQuery {
            node_id,
            from: from_instant,
            to: to_instant,
            tier,
            limit: PUBLIC_METRIC_WINDOW_POINT_LIMIT,
        },
    )
    .await?;
    history.series.push(public_window_coverage(
        "blockIntervalMs",
        &blocks.block_interval_ms,
        tier,
        &to,
    ));
    history.series.push(public_window_coverage(
        "transactionCount",
        &blocks.transaction_count,
        tier,
        &to,
    ));
    history.block_interval_ms = blocks
        .block_interval_ms
        .points
        .into_iter()
        .filter_map(|point| window_point(point, to_instant))
        .collect();
    history.transaction_count = blocks
        .transaction_count
        .points
        .into_iter()
        .zip(blocks.transaction_totals)
        .filter_map(|(point, total)| {
            let mut point = window_point(point, to_instant)?;
            if let Some(bucket) = point.bucket.as_mut() {
                bucket.total_value = total.map(|total| total as f64);
            }
            Some(point)
        })
        .collect();
    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public_metric_point(sampled_at: &str, value: f64) -> PublicMetricPoint {
        PublicMetricPoint {
            sampled_at: sampled_at.to_owned(),
            value,
            bucket: None,
        }
    }

    const COVERAGE_FROM: &str = "2026-01-01T00:00:00Z";
    const COVERAGE_TO: &str = "2026-01-01T00:01:00Z";

    /// The ledger is the only reason an expired series is not answered as a
    /// series nobody ever observed: raw samples are pruned, what a series
    /// observed is not (issue #225, Story 49).
    #[test]
    fn public_metric_coverage_dates_an_expired_series_from_the_ledger() {
        let ledger = MetricSeriesLedgerRow {
            metric: "processCpuPercent".to_owned(),
            first_observed_at: "2025-12-01T00:00:00Z".to_owned(),
            last_observed_at: "2025-12-01T00:10:00Z".to_owned(),
        };
        // No raw sample survives, so the answer carries none of them.
        let coverage = public_metric_coverage(
            "processCpuPercent",
            &[],
            COVERAGE_FROM,
            COVERAGE_TO,
            Some(&ledger),
        );
        assert!(coverage.observed, "the ledger remembers the observations");
        assert_eq!(coverage.observation_count, 0);
        assert_eq!(
            coverage.first_observed_at.as_deref(),
            Some("2025-12-01T00:00:00Z")
        );
        assert_eq!(
            coverage.last_observed_at.as_deref(),
            Some("2025-12-01T00:10:00Z")
        );
        // 2025-12-01T00:10:00Z to 2026-01-01T00:01:00Z: 31 days, 9 minutes short.
        assert_eq!(coverage.unobserved_tail_seconds, 31 * 86_400 - 9 * 60);
        // Dated, and still nothing to draw or to claim: an expired series has no
        // samples to measure a rhythm from, so no cadence, no gap, no coverage.
        assert_eq!(coverage.cadence_seconds, 0);
        assert_eq!(coverage.gap_threshold_seconds, 0);
        assert_eq!(coverage.covered_seconds, 0);
        assert!(coverage.gaps.is_empty());
    }

    /// A clock running ahead of the window dates nothing, ledger included.
    #[test]
    fn public_metric_coverage_never_lets_a_future_instant_date_a_series() {
        let ledger = MetricSeriesLedgerRow {
            metric: "processCpuPercent".to_owned(),
            first_observed_at: "2026-01-01T00:00:30Z".to_owned(),
            last_observed_at: "2026-01-01T00:02:00Z".to_owned(),
        };
        let coverage = public_metric_coverage(
            "processCpuPercent",
            &[],
            COVERAGE_FROM,
            COVERAGE_TO,
            Some(&ledger),
        );
        assert!(!coverage.observed);
        assert_eq!(coverage.first_observed_at, None);
        assert_eq!(coverage.last_observed_at, None);
        assert_eq!(coverage.unobserved_tail_seconds, 0);
    }

    #[test]
    fn public_metric_coverage_reports_a_series_nobody_observed() {
        let coverage =
            public_metric_coverage("processCpuPercent", &[], COVERAGE_FROM, COVERAGE_TO, None);
        assert!(!coverage.observed);
        // This series has a ledger, so its absence is proved and the answer may
        // say that nobody ever observed it rather than only that the Server
        // holds no evidence (issue #225, Story 49).
        assert!(coverage.never_observed_proven);
        assert_eq!(coverage.observation_count, 0);
        assert_eq!(coverage.first_observed_at, None);
        assert_eq!(coverage.last_observed_at, None);
        assert_eq!(coverage.cadence_seconds, 0);
        assert_eq!(coverage.gap_threshold_seconds, 0);
        assert_eq!(coverage.covered_seconds, 0);
        // Nothing was ever observed, so nothing is quiet either: the tail is a
        // silence after evidence, and `observed` is how the answer says which
        // case this is.
        assert_eq!(coverage.unobserved_tail_seconds, 0);
        assert!(coverage.gaps.is_empty());
    }

    /// The two block series have no ledger to speak for them, so the same
    /// `observed` of `false` cannot be a proof that nobody ever reported them:
    /// it is an unknown, and this flag says which of the two it is (issue #225,
    /// Story 49).
    #[test]
    fn public_metric_coverage_calls_a_block_series_absence_unknown() {
        for metric in ["blockIntervalMs", "transactionCount"] {
            let coverage = public_metric_coverage(metric, &[], COVERAGE_FROM, COVERAGE_TO, None);
            assert!(!coverage.observed);
            assert!(
                !coverage.never_observed_proven,
                "{metric} has no ledger, so its absence is unknown rather than proven"
            );
            assert_eq!(coverage.unobserved_tail_seconds, 0);
        }
    }

    #[test]
    fn public_metric_coverage_breaks_the_line_at_a_silence_of_a_short_window() {
        let points: Vec<PublicMetricPoint> = [
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:05Z",
            "2026-01-01T00:00:10Z",
            "2026-01-01T00:00:15Z",
            "2026-01-01T00:00:41Z",
            "2026-01-01T00:00:46Z",
            "2026-01-01T00:00:51Z",
        ]
        .iter()
        .map(|sampled_at| public_metric_point(sampled_at, 10.0))
        .collect();
        let coverage = public_metric_coverage(
            "processCpuPercent",
            &points,
            COVERAGE_FROM,
            COVERAGE_TO,
            None,
        );
        assert!(coverage.observed);
        assert_eq!(coverage.observation_count, 7);
        assert_eq!(coverage.cadence_seconds, 5);
        assert_eq!(coverage.gap_threshold_seconds, 15);
        assert_eq!(coverage.first_observed_at.as_deref(), Some(COVERAGE_FROM));
        assert_eq!(
            coverage.last_observed_at.as_deref(),
            Some("2026-01-01T00:00:51Z")
        );
        assert_eq!(coverage.gaps.len(), 1);
        assert_eq!(coverage.gaps[0].from, "2026-01-01T00:00:15Z");
        // The gap ends where the chart may draw again: the first observation
        // after the silence, not the window end.
        assert_eq!(coverage.gaps[0].to, "2026-01-01T00:00:41Z");
        assert_eq!(coverage.gaps[0].seconds, 26);
        assert_eq!(coverage.gaps[0].kind, "collection_gap");
        // The six proven stretches, and nothing after the newest observation.
        assert_eq!(coverage.covered_seconds, 25);
        assert_eq!(coverage.unobserved_tail_seconds, 9);
    }

    #[test]
    fn public_metric_coverage_answers_a_changed_cadence_as_unknown_rather_than_silence() {
        let points: Vec<PublicMetricPoint> = [
            "2025-12-31T23:50:00Z",
            "2026-01-01T00:00:30Z",
            "2026-01-01T00:00:55Z",
        ]
        .iter()
        .map(|sampled_at| public_metric_point(sampled_at, 10.0))
        .collect();
        let coverage = public_metric_coverage(
            "processCpuPercent",
            &points,
            COVERAGE_FROM,
            COVERAGE_TO,
            None,
        );
        // The first point is the last-good value carried into the answer, and it
        // is what dates the series: 630 seconds then 25 means the series is not
        // being observed at one rhythm, so the window states no silence at all.
        assert!(coverage.observed);
        assert_eq!(coverage.observation_count, 2);
        assert_eq!(coverage.cadence_seconds, 0);
        assert_eq!(coverage.gap_threshold_seconds, 0);
        assert!(coverage.gaps.is_empty());
        // And where no rhythm is settled there is no coverage to claim either:
        // the stretch between the two in-window observations is not observed
        // just because both of its ends were.
        assert_eq!(coverage.covered_seconds, 0);
        assert_eq!(
            coverage.first_observed_at.as_deref(),
            Some("2025-12-31T23:50:00Z")
        );
        assert_eq!(
            coverage.last_observed_at.as_deref(),
            Some("2026-01-01T00:00:55Z")
        );
        assert_eq!(coverage.unobserved_tail_seconds, 5);
    }

    #[test]
    fn public_metric_coverage_counts_an_observed_zero_as_evidence() {
        let points = vec![
            public_metric_point("2026-01-01T00:00:10Z", 0.0),
            public_metric_point("2026-01-01T00:00:20Z", 0.0),
        ];
        let coverage = public_metric_coverage(
            "processCpuPercent",
            &points,
            COVERAGE_FROM,
            COVERAGE_TO,
            None,
        );
        assert!(coverage.observed);
        assert_eq!(coverage.observation_count, 2);
        assert_eq!(coverage.cadence_seconds, 10);
        assert_eq!(coverage.gap_threshold_seconds, 30);
        assert_eq!(coverage.covered_seconds, 10);
        assert_eq!(coverage.unobserved_tail_seconds, 40);
        assert!(coverage.gaps.is_empty());
    }

    #[test]
    fn public_metric_coverage_dates_the_series_by_the_window_alone() {
        let points = vec![
            public_metric_point("2025-12-31T23:00:00Z", 10.0),
            public_metric_point("2026-01-01T01:00:00Z", 10.0),
        ];
        let coverage = public_metric_coverage(
            "processCpuPercent",
            &points,
            COVERAGE_FROM,
            COVERAGE_TO,
            None,
        );
        // The window holds neither instant: one belongs to the carried last-good
        // stretch, the other to a clock ahead of the Server. Neither is counted
        // and neither is drawn, but only the older one may date the series - an
        // instant the window cannot hold yet may not replace the last-good point
        // the answer really has and report a comfortable zero-second tail.
        assert!(coverage.observed);
        assert_eq!(coverage.observation_count, 0);
        assert_eq!(coverage.cadence_seconds, 0);
        assert_eq!(coverage.gap_threshold_seconds, 0);
        assert_eq!(coverage.covered_seconds, 0);
        assert_eq!(
            coverage.first_observed_at.as_deref(),
            Some("2025-12-31T23:00:00Z")
        );
        assert_eq!(
            coverage.last_observed_at.as_deref(),
            Some("2025-12-31T23:00:00Z")
        );
        // An hour before the window, and the window's own minute after it.
        assert_eq!(coverage.unobserved_tail_seconds, 3660);
        assert!(coverage.gaps.is_empty());
    }

    /// The shape the Home fixtures replay, pinned at the Server seam: a series
    /// observed every five seconds, silent for thirty, then observed twice more.
    /// The two intervals after the silence agree, so the rhythm is settled again
    /// and the silence inside the window is named as the gap it is (issue #225).
    #[test]
    fn public_metric_coverage_names_the_silence_of_a_recovered_series() {
        let points: Vec<PublicMetricPoint> = [
            "2026-01-01T00:00:05Z",
            "2026-01-01T00:00:10Z",
            "2026-01-01T00:00:15Z",
            "2026-01-01T00:00:45Z",
            "2026-01-01T00:00:50Z",
            "2026-01-01T00:00:55Z",
        ]
        .iter()
        .map(|sampled_at| public_metric_point(sampled_at, 20.0))
        .collect();
        let coverage = public_metric_coverage(
            "processCpuPercent",
            &points,
            COVERAGE_FROM,
            COVERAGE_TO,
            None,
        );
        assert!(coverage.observed);
        assert_eq!(coverage.observation_count, 6);
        assert_eq!(coverage.cadence_seconds, 5);
        assert_eq!(coverage.gap_threshold_seconds, 15);
        assert_eq!(coverage.gaps.len(), 1);
        assert_eq!(coverage.gaps[0].from, "2026-01-01T00:00:15Z");
        assert_eq!(coverage.gaps[0].to, "2026-01-01T00:00:45Z");
        assert_eq!(coverage.gaps[0].seconds, 30);
        assert_eq!(coverage.gaps[0].kind, "collection_gap");
        // Four proven five-second stretches, and nothing after the newest
        // observation but the five seconds the window still had to run.
        assert_eq!(coverage.covered_seconds, 20);
        assert_eq!(coverage.unobserved_tail_seconds, 5);
    }
}
