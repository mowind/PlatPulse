//! Raw Node metric history: the trusted 24-hour window behind the Owner-side
//! Node Admin surface (issue #213, design §11.4).
//!
//! The Server stores one row per (node, metric, observed_at) in
//! `node_metric_samples` and summarizes the series in
//! `node_metric_series_state`. This module is the read side: it answers "what
//! does the Server actually know about this series, and where is it silent?".
//!
//! Three rules make that answer trustworthy, and each of them is deliberately
//! visible in the data rather than assumed:
//!
//! * **A gap is silence, not a line.** The range reader reports the silences
//!   between stored observations (relative to the series' own observed
//!   cadence, never below `MIN_GAP_SECONDS`) and the coverage the
//!   observations actually prove, so nothing downstream has to connect across
//!   a missing stretch, extend a constant value into unobserved time, or turn
//!   a gap into a zero.
//! * **A pause is evidenced.** Low-space protection does not write samples; it
//!   writes skipped-series rows (issue #212). A silence the operator chose is
//!   therefore reported as a pause with its counted losses, separately from a
//!   silence nobody explained.
//! * **A receipt and its observation stay together.** Delay and the
//!   suspicious-clock indication are derived from the retained
//!   (observed_at, received_at) pair at read time, so no derived flag can drift
//!   from the evidence that produced it, and the rule has exactly one home:
//!   `sample_timing`.
//! * **Novelty comes from the ledger, not from the row.** Retention releases a
//!   sample row once it leaves the raw window while the ledger keeps the
//!   series' high-water mark, so a delivery is classified against that mark and
//!   the window's own cutoff together: a reading past the mark is new, an
//!   instant the window still covers but holds no sample for is a genuine
//!   out-of-order observation, and an instant the window has already released is
//!   a replay: never counted, and never stored again either — recreating the row
//!   would invent coverage and receipt evidence for a stretch the Server
//!   deliberately stopped holding. An observation that is counted while no row
//!   can answer for it leaves an evidence floor behind instead
//!   (`counted_evidence_floor`), which is what keeps a later carry of the same
//!   instant from being counted twice once a widened policy moves the cutoff
//!   back over it.

use sqlx::{Sqlite, SqlitePool, Transaction};
use time::OffsetDateTime;

use crate::auth::{format_rfc3339, parse_rfc3339};

/// The Node metric series the Server stores raw and serves to Admin.
pub const NODE_METRIC_SERIES: [&str; 5] = [
    "process_cpu_percent",
    "process_memory_percent",
    "data_directory_percent",
    "peer_inbound_count",
    "peer_outbound_count",
];

/// Whether a metric name is one of the stored Node series.
pub fn is_node_metric(metric: &str) -> bool {
    NODE_METRIC_SERIES.contains(&metric)
}

/// The design's raw window: the most recent 24 hours (§11.4).
pub const DEFAULT_WINDOW_HOURS: i64 = 24;

/// Samples one range answer carries unless the caller asks for fewer.
pub const DEFAULT_SAMPLE_LIMIT: i64 = 5_000;

/// The largest answer the range route will produce. A truncated answer says so
/// instead of silently dropping history.
pub const MAX_SAMPLE_LIMIT: i64 = 20_000;

/// An observation stamped this much later than the Server received it is a
/// clock suspect: the Agent's wall clock is ahead of the receipt.
pub const CLOCK_SKEW_TOLERANCE_SECONDS: i64 = 300;

/// The shortest silence ever reported as a gap, whatever the cadence is.
pub const MIN_GAP_SECONDS: i64 = 120;

/// How many times the series' own observed cadence a silence must span before
/// it is reported as a gap. The Server is never told an Agent's sampling
/// interval, so it measures the cadence it actually observed rather than
/// assuming one.
pub const GAP_CADENCE_FACTOR: i64 = 3;

/// The slowest collection interval a PlatPulse Agent may be configured with
/// (`crates/platpulse-agent/src/config.rs`: "collection_interval_seconds must
/// be between 1 and 300"), so a measured cadence is capped here.
///
/// The cap is what keeps a sparse series honest: two observations a day apart
/// would otherwise make the whole day look like one believable cadence and
/// report the stretch nobody observed as proved coverage, connected by a line.
pub const MAX_OBSERVED_CADENCE_SECONDS: i64 = 300;

/// One stored raw observation.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricSample {
    pub observed_at: String,
    pub received_at: String,
    pub value: f64,
}

/// The series ledger: what this series observed, counted without replay
/// inflation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesLedger {
    /// The series' enablement boundary: the oldest observation ever recorded.
    /// It outlives the samples themselves and is not evidence that raw samples
    /// survive that far back.
    pub first_observed_at: String,
    /// The newest observation the Server stores.
    pub last_observed_at: String,
    /// The most recent receipt carrying that newest observation. A re-delivery
    /// advances it, which is how a carried last-good value is told apart from
    /// fresh coverage.
    pub last_received_at: String,
    /// Distinct observation times stored. A replay never advances this.
    pub observation_count: i64,
    /// Deliveries of an observation the Server already held unchanged.
    pub replayed_count: i64,
    /// Deliveries that rewrote a stored observation with a different value.
    pub corrected_count: i64,
}

/// A protection interval's counted losses for one series (issue #212).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectionPause {
    pub first_skipped_at: String,
    pub last_skipped_at: String,
    pub skipped_count: i64,
}

/// Why the Server holds no observation for a stretch of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    /// Reports stopped arriving, or their observations did not cover the
    /// stretch: nobody has explained it.
    Collection,
    /// Low-space protection deliberately did not store optional history.
    ProtectionPause,
}

impl GapKind {
    /// Stable wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            GapKind::Collection => "collection_gap",
            GapKind::ProtectionPause => "protection_pause",
        }
    }
}

/// One evidenced stretch without observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricGap {
    pub from: String,
    pub to: String,
    pub seconds: i64,
    pub kind: GapKind,
    /// Counted skipped readings when protection explains the gap.
    pub skipped_count: Option<i64>,
}

/// What a series observed over a requested window.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricWindow {
    /// Stored observations, oldest first. When the answer is truncated these
    /// are the newest samples of the window, so no gap is ever invented at the
    /// oldest end.
    pub samples: Vec<MetricSample>,
    /// Series state; `None` means this series was never observed.
    pub ledger: Option<SeriesLedger>,
    pub gaps: Vec<MetricGap>,
    /// Seconds the stored observations prove they were being observed.
    pub coverage_seconds: i64,
    /// True when the window held more samples than the caller's limit.
    pub truncated: bool,
}

/// Delay and clock suspicion for one observation/receipt pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleTiming {
    /// `received_at - observed_at`, in seconds. Negative when the observation
    /// claims to be newer than the receipt.
    pub delay_seconds: i64,
    /// Set when the pair is a suspicious-clock indication, explaining which
    /// way the clocks disagree.
    pub clock_note: Option<String>,
}

/// Delay and clock suspicion derived from the retained timestamps.
///
/// `None` when either timestamp is not a canonical RFC 3339 UTC value: an
/// unusable pair is reported as unknown rather than as a zero delay.
pub fn sample_timing(observed_at: &str, received_at: &str) -> Option<SampleTiming> {
    let observed = parse_rfc3339(observed_at)?;
    let received = parse_rfc3339(received_at)?;
    let delay_seconds = (received - observed).whole_seconds();
    let clock_note = (delay_seconds < -CLOCK_SKEW_TOLERANCE_SECONDS).then(|| {
        format!(
            "the observation is stamped {}s after the Server received it: the Agent clock is ahead",
            -delay_seconds
        )
    });
    Some(SampleTiming {
        delay_seconds,
        clock_note,
    })
}

/// The silence that separates two consecutive observations into a gap.
///
/// The cadence is clamped to the range an Agent can actually be configured
/// with, so no silence is ever excused by a cadence nobody could have chosen.
pub fn gap_threshold_seconds(cadence_seconds: i64) -> i64 {
    (cadence_seconds.clamp(1, MAX_OBSERVED_CADENCE_SECONDS) * GAP_CADENCE_FACTOR)
        .max(MIN_GAP_SECONDS)
}

/// Gaps and proved coverage for one series window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Continuity {
    pub gaps: Vec<MetricGap>,
    pub coverage_seconds: i64,
}

/// Derive the silences and the proved coverage of a window.
///
/// `cadence_seconds` is the fastest cadence the window actually showed: a
/// series observed every 5 seconds that is silent for an hour is a gap, while a
/// series observed every 10 minutes is not reported as continuously failing
/// just because its instrument is slow.
///
/// `window_start` and `window_end` are the bounds of the requested window. A
/// gap is an answer about that window, so nothing is reported outside it: a
/// window that ends inside a protection pause reports the pause only as far as
/// the window goes, a pause that reaches back before the window reports only
/// the part inside it, and a counted loss is attached only to a stretch that
/// covers the whole protection interval.
pub fn continuity(
    samples: &[MetricSample],
    pauses: &[ProtectionPause],
    cadence_seconds: i64,
    window_start: &str,
    window_end: &str,
) -> Continuity {
    let threshold = gap_threshold_seconds(cadence_seconds);
    let mut gaps = Vec::new();
    let mut coverage_seconds = 0;
    for pair in samples.windows(2) {
        let (Some(previous), Some(next)) = (
            parse_rfc3339(&pair[0].observed_at),
            parse_rfc3339(&pair[1].observed_at),
        ) else {
            continue;
        };
        let seconds = (next - previous).whole_seconds();
        if seconds <= 0 {
            continue;
        }
        if seconds >= threshold {
            let (kind, skipped_count) =
                match pause_overlapping(pauses, &pair[0].observed_at, &pair[1].observed_at) {
                    Some(pause) => (
                        GapKind::ProtectionPause,
                        pause_count_within(pause, &pair[0].observed_at, &pair[1].observed_at),
                    ),
                    None => (GapKind::Collection, None),
                };
            gaps.push(MetricGap {
                from: pair[0].observed_at.clone(),
                to: pair[1].observed_at.clone(),
                seconds,
                kind,
                skipped_count,
            });
        } else if pause_intersects(pauses, &pair[0].observed_at, &pair[1].observed_at) {
            // A pause the Server recorded is evidence in its own right: the
            // ledger names instants inside this stretch that were skipped, so the
            // stretch is reported as a pause even though it is shorter than the
            // silence this cadence would infer. A loss the Server knows about is
            // never drawn as a continuous line (issue #213). The interval-wide
            // count belongs to the stretch only when the stretch covers the whole
            // pause: the ledger records which instants were skipped, never how
            // long the unobserved stretch between them was.
            let skipped_count =
                pause_overlapping(pauses, &pair[0].observed_at, &pair[1].observed_at).and_then(
                    |pause| pause_count_within(pause, &pair[0].observed_at, &pair[1].observed_at),
                );
            gaps.push(MetricGap {
                from: pair[0].observed_at.clone(),
                to: pair[1].observed_at.clone(),
                seconds,
                kind: GapKind::ProtectionPause,
                skipped_count,
            });
        } else {
            // Only proven continuity counts: the stretch between two stored
            // observations, never the stretch after the newest one, whose length
            // nobody observed.
            coverage_seconds += seconds;
        }
    }
    // A window can end inside a pause: the stretch after the newest stored
    // observation is then a pause gap too, bounded by the counted losses.
    let tail = samples.last().map(|sample| sample.observed_at.as_str());
    for pause in pauses {
        if let Some(tail) = tail
            && tail >= pause.first_skipped_at.as_str()
        {
            continue;
        }
        let from = match tail {
            Some(tail) if tail < pause.first_skipped_at.as_str() => tail.to_owned(),
            _ => pause.first_skipped_at.clone(),
        };
        // The ledger records the whole protection interval, not the part of it
        // this answer covers, so a pause that reaches back before the requested
        // window reports a gap that starts where the window starts.
        let from = if from.as_str() < window_start {
            window_start.to_owned()
        } else {
            from
        };
        // The pause can run past the end of the requested window: the gap stops
        // where the window stops, and the interval-wide loss count is not
        // claimed for a stretch that only clips the pause.
        let to = if pause.last_skipped_at.as_str() > window_end {
            window_end.to_owned()
        } else {
            pause.last_skipped_at.clone()
        };
        let (Some(from_at), Some(to_at)) = (parse_rfc3339(&from), parse_rfc3339(&to)) else {
            continue;
        };
        let seconds = (to_at - from_at).whole_seconds();
        if seconds <= 0 {
            continue;
        }
        let skipped_count = pause_count_within(pause, &from, &to);
        gaps.push(MetricGap {
            from,
            to,
            seconds,
            kind: GapKind::ProtectionPause,
            skipped_count,
        });
    }
    gaps.sort_by(|left, right| left.from.cmp(&right.from));
    Continuity {
        gaps,
        coverage_seconds,
    }
}

fn pause_overlapping<'a>(
    pauses: &'a [ProtectionPause],
    from: &str,
    to: &str,
) -> Option<&'a ProtectionPause> {
    pauses.iter().find(|pause| {
        pause.last_skipped_at.as_str() >= from && pause.first_skipped_at.as_str() <= to
    })
}

/// The counted losses a stretch may claim, or `None` when the stretch does not
/// cover the whole protection interval.
///
/// The capacity ledger keeps one count per series and protection interval with
/// no per-skip timestamps, so a stretch that clips a corner of a longer pause
/// cannot honestly say how many of those losses happened inside it.
fn pause_count_within(pause: &ProtectionPause, from: &str, to: &str) -> Option<i64> {
    (pause.first_skipped_at.as_str() >= from && pause.last_skipped_at.as_str() <= to)
        .then_some(pause.skipped_count)
}

/// Whether a recorded protection pause touches the stretch `(from, to)`.
///
/// The ledger bounds the instants of one series that were skipped, not the
/// length of the stretch that went unobserved, so a pause recorded as a single
/// instant (`first == last`) still disproves the whole stretch between the two
/// stored observations that bracket it.
fn pause_intersects(pauses: &[ProtectionPause], from: &str, to: &str) -> bool {
    pauses
        .iter()
        .any(|pause| pause.first_skipped_at.as_str() < to && pause.last_skipped_at.as_str() > from)
}

/// The fastest cadence the window showed, or 0 when it holds fewer than two
/// observations.
pub fn observed_cadence_seconds(samples: &[MetricSample]) -> i64 {
    samples
        .windows(2)
        .filter_map(|pair| {
            let previous = parse_rfc3339(&pair[0].observed_at)?;
            let next = parse_rfc3339(&pair[1].observed_at)?;
            let seconds = (next - previous).whole_seconds();
            (seconds > 0).then_some(seconds)
        })
        .min()
        .unwrap_or(0)
}

/// What one delivery turned out to be, judged against the series ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// An observation the Server holds no evidence of: a reading past the
    /// series' high-water mark, or an instant the raw window still covers but
    /// holds no sample for (an out-of-order or silence-filling reading). Only
    /// this advances the observation count and the series clock.
    Observed,
    /// A delivery carrying the value the Server already holds for that instant,
    /// such as a carried last-good sample, or an instant behind the high-water
    /// mark whose row the raw window has already released.
    Replay,
    /// A delivery at or behind the high-water mark whose value differs from
    /// what the Server holds for that instant: a restatement of a reading it
    /// already had.
    Correction,
}

/// Classify one delivery against the ledger, the retained row, and the
/// raw-window cutoff.
///
/// The raw sample row alone cannot answer whether an observation is new:
/// retention releases rows once they leave the raw window while the ledger keeps
/// counting, so judging novelty by row presence would count a replayed
/// observation as a fresh reading every time its row has expired (issue #213).
/// The three questions are therefore asked in this order:
///
/// 1. Past the series' high-water mark: nothing has ever been observed there, so
///    the reading is new. A carried last-good keeps its original observation
///    time, which is behind the mark, and can never arrive here.
/// 2. A sample exists at that instant: the delivery repeats a reading the Server
///    still holds, and only the value decides between a replay and a correction.
/// 3. No sample exists but the instant is still inside the raw window: the
///    Server holds no evidence of that instant, so this is a genuine observation
///    that arrived out of order or filled a silence. It is stored and counted
///    like any other, which keeps the retained rows and the ledger equal.
/// 4. No sample exists and the instant is older than the evidence floor: the
///    window can no longer answer with it, and an expired replay is
///    indistinguishable from an observation so old that no history could hold
///    it. It is treated as a replay and never counted, because counting it would
///    let a replayed last-good inflate the lifetime count — the one thing the
///    ledger exists to prevent (design §11.3).
///
/// The evidence floor is the later of the policy cutoff and the series'
/// `released_before` stamp, the oldest cutoff the cleanup has ever pruned this
/// series at. Widening the raw retention policy moves the window back in time,
/// but it cannot restore the identity evidence a cleanup already released: the
/// floor only ever moves forward, so a carried last-good replayed after a
/// widening stays a replay instead of being counted a second time.
pub async fn classify_delivery(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    metric: &str,
    observed_at: &str,
    stored_value: Option<f64>,
    value: f64,
    cutoff: &str,
) -> Result<Delivery, sqlx::Error> {
    let ledger: Option<(String, String)> = sqlx::query_as(
        "SELECT last_observed_at, released_before FROM node_metric_series_state WHERE node_id = ? AND metric = ?",
    )
    .bind(node_id)
    .bind(metric)
    .fetch_optional(&mut **tx)
    .await?;
    let (high_water, released_before) = match ledger {
        Some((last_observed_at, released_before)) => {
            (Some(last_observed_at), Some(released_before))
        }
        None => (None, None),
    };
    // Canonical instants compare as text in the same order they were written.
    if high_water.is_none_or(|last| observed_at > last.as_str()) {
        return Ok(Delivery::Observed);
    }
    if stored_value.is_some_and(|stored| stored != value) {
        return Ok(Delivery::Correction);
    }
    let floor = match released_before {
        Some(released) if released.as_str() > cutoff => released,
        _ => cutoff.to_owned(),
    };
    if stored_value.is_none() && observed_at >= floor.as_str() {
        return Ok(Delivery::Observed);
    }
    Ok(Delivery::Replay)
}

/// Parse a caller-supplied range bound, but only in the canonical shape the
/// stored observations use.
///
/// Stored instants compare as text (`'YYYY-MM-DDTHH:MM:SSZ'`), so a bound that
/// parses to the right instant in any other shape — a fractional second, an
/// offset such as `+08:00`, another precision — is a different string and would
/// silently exclude rows that are inside the range the caller meant. Accepting
/// only the canonical second-precision UTC form makes the range the caller
/// asked for and the range the SQL compares identical, and a bound that is not
/// in it is refused as an invalid range instead of quietly answering the wrong
/// window.
pub fn canonical_instant(value: &str) -> Option<OffsetDateTime> {
    let parsed = crate::auth::parse_rfc3339(value)?;
    // The RFC 3339 parser accepts fractional seconds and prints them again, so
    // the round-trip check alone would let `…T00:00:00.5Z` through. The stored
    // instants have second precision, and text comparison puts `…:00.5Z` before
    // `…:00Z`, so such a bound would answer a different window than the caller
    // asked for: require the second-precision shape as well.
    (parsed.nanosecond() == 0 && crate::auth::format_rfc3339(parsed) == value).then_some(parsed)
}

/// Whether one delivery falls outside the retained raw window with nothing
/// held for it.
///
/// Such a delivery writes no sample row: the range route can no longer answer
/// with the instant, and the cleanup that runs on the same Report would release
/// the row again. Everything else keeps its row — a new observation, a
/// correction of a row that exists, and a replay whose row is still inside the
/// window — so the retained rows and the ledger never disagree about what was
/// observed.
///
/// The cutoff is the raw family's own policy cutoff
/// (`retention::family_cutoff`), never the oldest surviving row: a series whose
/// oldest row is newer than the cutoff, because its history arrived out of
/// order, still owns the stretch between them.
pub fn outside_retained_window(stored_value: Option<f64>, observed_at: &str, cutoff: &str) -> bool {
    stored_value.is_none() && observed_at < cutoff
}

/// Move the series ledger for one classified delivery.
///
/// `delivery` comes from `classify_delivery`: only a new observation advances
/// the count, so an accepted Report replay cannot inflate the history (design
/// §11.3, issue #213) and a correction is counted as a restatement rather than
/// as fresh coverage.
pub async fn record_delivery(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    metric: &str,
    observed_at: &str,
    received_at: &str,
    delivery: Delivery,
) -> Result<(), sqlx::Error> {
    let is_new = delivery == Delivery::Observed;
    let is_correction = delivery == Delivery::Correction;
    let is_replay = delivery == Delivery::Replay;
    sqlx::query(
        "INSERT INTO node_metric_series_state (node_id, metric, first_observed_at, last_observed_at, last_received_at, observation_count, replayed_count, corrected_count, updated_at) VALUES (?, ?, ?, ?, ?, 1, 0, 0, ?) ON CONFLICT(node_id, metric) DO UPDATE SET first_observed_at = MIN(node_metric_series_state.first_observed_at, excluded.first_observed_at), last_received_at = CASE WHEN excluded.last_observed_at >= node_metric_series_state.last_observed_at THEN excluded.last_received_at ELSE node_metric_series_state.last_received_at END, last_observed_at = MAX(node_metric_series_state.last_observed_at, excluded.last_observed_at), observation_count = node_metric_series_state.observation_count + ?, replayed_count = node_metric_series_state.replayed_count + ?, corrected_count = node_metric_series_state.corrected_count + ?, updated_at = excluded.updated_at",
    )
    .bind(node_id)
    .bind(metric)
    .bind(observed_at)
    .bind(observed_at)
    .bind(received_at)
    .bind(received_at)
    .bind(if is_new { 1_i64 } else { 0 })
    .bind(if is_replay { 1_i64 } else { 0 })
    .bind(if is_correction { 1_i64 } else { 0 })
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The value the Server already holds for one observation time, if any.
pub async fn stored_value(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    metric: &str,
    observed_at: &str,
) -> Result<Option<f64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT value FROM node_metric_samples WHERE node_id = ? AND metric = ? AND observed_at = ?",
    )
    .bind(node_id)
    .bind(metric)
    .bind(observed_at)
    .fetch_optional(&mut **tx)
    .await
}

/// The evidence floor a counted observation that no row can answer for leaves.
///
/// The Server counts an observation it cannot store — an instant the raw window
/// no longer holds — because the ledger answers "what did this series observe",
/// not "what can this Server still return". Counting it while nothing records
/// that it was counted would let a later carry of the same instant be counted a
/// second time once a widened policy moves the cutoff back over it. The stamp is
/// the next second, because the instant itself is still the instant the series
/// observed: the smallest stamp above it is what says "the evidence for this
/// instant is no longer held" (issue #213).
pub fn counted_evidence_floor(observed_at: &str) -> Option<String> {
    let parsed = parse_rfc3339(observed_at)?;
    Some(format_rfc3339(parsed + time::Duration::seconds(1)))
}

/// Move one series' evidence floor forward, never back.
pub async fn stamp_evidence_floor(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    metric: &str,
    floor: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE node_metric_series_state SET released_before = MAX(released_before, ?) WHERE node_id = ? AND metric = ? AND released_before < ?",
    )
    .bind(floor)
    .bind(node_id)
    .bind(metric)
    .bind(floor)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Read one series window for the Admin surface.
///
/// `limit` bounds the samples in the answer. A truncated answer carries the
/// newest samples of the window: the oldest end is then reported as truncated,
/// never as a gap nobody recorded.
pub async fn load_window(
    pool: &SqlitePool,
    node_id: &str,
    metric: &str,
    from: OffsetDateTime,
    to: OffsetDateTime,
    limit: i64,
) -> Result<MetricWindow, sqlx::Error> {
    let from_text = format_rfc3339(from);
    let to_text = format_rfc3339(to);
    let rows = sqlx::query_as::<_, (String, String, f64)>(
        "SELECT observed_at, received_at, value FROM node_metric_samples WHERE node_id = ? AND metric = ? AND observed_at >= ? AND observed_at <= ? ORDER BY observed_at DESC LIMIT ?",
    )
    .bind(node_id)
    .bind(metric)
    .bind(&from_text)
    .bind(&to_text)
    .bind(limit.max(0) + 1)
    .fetch_all(pool)
    .await?;
    let truncated = rows.len() as i64 > limit.max(0);
    let mut samples: Vec<MetricSample> = rows
        .into_iter()
        .take(limit.max(0) as usize)
        .map(|(observed_at, received_at, value)| MetricSample {
            observed_at,
            received_at,
            value,
        })
        .collect();
    samples.sort_by(|left, right| left.observed_at.cmp(&right.observed_at));
    let ledger = sqlx::query_as::<_, (String, String, String, i64, i64, i64)>(
        "SELECT first_observed_at, last_observed_at, last_received_at, observation_count, replayed_count, corrected_count FROM node_metric_series_state WHERE node_id = ? AND metric = ?",
    )
    .bind(node_id)
    .bind(metric)
    .fetch_optional(pool)
    .await?
    .map(
        |(
            first_observed_at,
            last_observed_at,
            last_received_at,
            observation_count,
            replayed_count,
            corrected_count,
        )| SeriesLedger {
            first_observed_at,
            last_observed_at,
            last_received_at,
            observation_count,
            replayed_count,
            corrected_count,
        },
    );
    let pauses = load_pauses(pool, node_id, metric, &from_text, &to_text).await?;
    let cadence = observed_cadence_seconds(&samples);
    let continuity = continuity(&samples, &pauses, cadence, &from_text, &to_text);
    Ok(MetricWindow {
        samples,
        ledger,
        gaps: continuity.gaps,
        coverage_seconds: continuity.coverage_seconds,
        truncated,
    })
}

/// The protection-pause lookup runs on every Owner metric-history request.
///
/// It is a range read of one series, and the ledger it reads grows with the
/// fleet, so it must be served by an index rather than a full scan: the
/// samples beside it are already fetched through an ordered range index, and a
/// scan here would make the range route cost grow with every Node that has ever
/// recorded a protection loss (migration 0066, issue #213). Held as a constant
/// so the plan test beside it asserts the statement the route really runs.
const PAUSE_LOOKUP_SQL: &str = "SELECT first_skipped_at, last_skipped_at, skipped_count FROM capacity_skipped_series WHERE scope_kind = 'node' AND scope_key = ? AND metric = ? AND last_skipped_at >= ? AND first_skipped_at <= ? ORDER BY first_skipped_at";

/// Protection losses recorded for one series inside the window.
///
/// These rows are the positive evidence that a silence was chosen rather than
/// suffered, so a protected stretch is never reported as an unexplained gap.
async fn load_pauses(
    pool: &SqlitePool,
    node_id: &str,
    metric: &str,
    from: &str,
    to: &str,
) -> Result<Vec<ProtectionPause>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, i64)>(PAUSE_LOOKUP_SQL)
        .bind(node_id)
        .bind(metric)
        .bind(from)
        .bind(to)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(
            |(first_skipped_at, last_skipped_at, skipped_count)| ProtectionPause {
                first_skipped_at,
                last_skipped_at,
                skipped_count,
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(observed_at: &str, received_at: &str, value: f64) -> MetricSample {
        MetricSample {
            observed_at: observed_at.to_owned(),
            received_at: received_at.to_owned(),
            value,
        }
    }

    fn pause(first: &str, last: &str, skipped: i64) -> ProtectionPause {
        ProtectionPause {
            first_skipped_at: first.to_owned(),
            last_skipped_at: last.to_owned(),
            skipped_count: skipped,
        }
    }

    #[test]
    fn a_delay_is_measured_from_the_retained_timestamps() {
        let timing = sample_timing("2026-01-01T00:00:00Z", "2026-01-01T00:00:04Z").unwrap();
        assert_eq!(timing.delay_seconds, 4);
        assert_eq!(timing.clock_note, None);
        assert_eq!(
            sample_timing("not a timestamp", "2026-01-01T00:00:04Z"),
            None
        );
    }

    #[test]
    fn an_observation_stamped_after_its_receipt_is_a_clock_suspect() {
        let timing = sample_timing("2026-01-01T00:10:00Z", "2026-01-01T00:00:00Z").unwrap();
        assert_eq!(timing.delay_seconds, -600);
        assert!(timing.clock_note.unwrap().contains("Agent clock is ahead"));
        // A small disagreement is ordinary transport, not a suspicious clock.
        let ordinary = sample_timing("2026-01-01T00:00:04Z", "2026-01-01T00:00:00Z").unwrap();
        assert_eq!(ordinary.clock_note, None);
    }

    #[test]
    fn silence_is_a_gap_relative_to_the_series_own_cadence() {
        let fast = vec![
            sample("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z", 1.0),
            sample("2026-01-01T00:00:05Z", "2026-01-01T00:00:06Z", 2.0),
            sample("2026-01-01T01:00:00Z", "2026-01-01T01:00:01Z", 3.0),
        ];
        assert_eq!(observed_cadence_seconds(&fast), 5);
        let fast_continuity = continuity(
            &fast,
            &[],
            observed_cadence_seconds(&fast),
            "2026-01-01T00:00:00Z",
            "2026-01-01T01:00:00Z",
        );
        assert_eq!(fast_continuity.coverage_seconds, 5);
        assert_eq!(fast_continuity.gaps.len(), 1);
        assert_eq!(fast_continuity.gaps[0].kind, GapKind::Collection);
        assert_eq!(fast_continuity.gaps[0].seconds, 3595);

        // The same silence for a series that was only ever observed every ten
        // minutes is its normal cadence, not a failure.
        let slow = vec![
            sample("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z", 1.0),
            sample("2026-01-01T00:10:00Z", "2026-01-01T00:10:01Z", 2.0),
            sample("2026-01-01T01:00:00Z", "2026-01-01T01:00:01Z", 3.0),
        ];
        assert_eq!(observed_cadence_seconds(&slow), 600);
        let slow_continuity = continuity(
            &slow,
            &[],
            observed_cadence_seconds(&slow),
            "2026-01-01T00:00:00Z",
            "2026-01-01T01:00:00Z",
        );
        assert_eq!(slow_continuity.gaps.len(), 1);
        assert_eq!(slow_continuity.gaps[0].seconds, 3000);
        assert_eq!(slow_continuity.coverage_seconds, 600);
    }

    #[test]
    fn a_silence_the_operator_chose_is_reported_as_a_pause() {
        let samples = vec![
            sample("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z", 1.0),
            sample("2026-01-01T01:00:00Z", "2026-01-01T01:00:01Z", 2.0),
        ];
        let pauses = vec![pause("2026-01-01T00:00:30Z", "2026-01-01T00:59:00Z", 640)];
        let continuity = continuity(
            &samples,
            &pauses,
            5,
            "2026-01-01T00:00:00Z",
            "2026-01-01T01:00:00Z",
        );
        assert_eq!(continuity.gaps.len(), 1);
        assert_eq!(continuity.gaps[0].kind, GapKind::ProtectionPause);
        assert_eq!(continuity.gaps[0].skipped_count, Some(640));
        assert_eq!(continuity.coverage_seconds, 0);
    }

    #[test]
    fn a_window_that_ends_inside_a_pause_reports_the_paused_tail() {
        let samples = vec![sample("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z", 1.0)];
        let pauses = vec![pause("2026-01-01T00:00:30Z", "2026-01-01T00:20:00Z", 230)];
        let continuity = continuity(
            &samples,
            &pauses,
            5,
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:20:00Z",
        );
        assert_eq!(continuity.gaps.len(), 1);
        assert_eq!(continuity.gaps[0].kind, GapKind::ProtectionPause);
        assert_eq!(continuity.gaps[0].from, "2026-01-01T00:00:00Z");
        assert_eq!(continuity.gaps[0].to, "2026-01-01T00:20:00Z");
        assert_eq!(continuity.gaps[0].seconds, 1200);
        // The reported stretch covers the whole protection interval, so the
        // interval-wide loss count belongs to it.
        assert_eq!(continuity.gaps[0].skipped_count, Some(230));
    }

    #[test]
    fn a_pause_that_does_not_reach_the_window_tail_adds_no_gap() {
        let samples = vec![
            sample("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z", 1.0),
            sample("2026-01-01T00:00:05Z", "2026-01-01T00:00:06Z", 2.0),
            sample("2026-01-01T05:00:00Z", "2026-01-01T05:00:01Z", 3.0),
        ];
        let pauses = vec![pause("2026-01-01T02:00:00Z", "2026-01-01T04:00:00Z", 1440)];
        let continuity = continuity(
            &samples,
            &pauses,
            5,
            "2026-01-01T00:00:00Z",
            "2026-01-01T05:00:00Z",
        );
        assert_eq!(continuity.gaps.len(), 1);
        assert_eq!(continuity.gaps[0].kind, GapKind::ProtectionPause);
        assert_eq!(continuity.gaps[0].from, "2026-01-01T00:00:05Z");
        assert_eq!(continuity.gaps[0].to, "2026-01-01T05:00:00Z");
    }

    #[test]
    fn a_window_inside_a_longer_pause_reports_only_the_window() {
        // A one hour question asked about a ten hour pause: the answer covers the
        // hour it was asked about, and the interval-wide loss count is not
        // claimed for a stretch that only clips the pause.
        let pauses = vec![pause("2026-01-01T00:00:30Z", "2026-01-01T10:00:00Z", 1180)];
        let continuity = continuity(
            &[],
            &pauses,
            5,
            "2026-01-01T00:00:00Z",
            "2026-01-01T01:00:00Z",
        );
        assert_eq!(continuity.gaps.len(), 1);
        assert_eq!(continuity.gaps[0].kind, GapKind::ProtectionPause);
        assert_eq!(continuity.gaps[0].from, "2026-01-01T00:00:30Z");
        assert_eq!(continuity.gaps[0].to, "2026-01-01T01:00:00Z");
        assert_eq!(continuity.gaps[0].seconds, 3570);
        assert_eq!(continuity.gaps[0].skipped_count, None);
    }

    #[test]
    fn a_count_belongs_to_the_whole_pause_not_to_the_stretch_around_it() {
        // The observations bracket only part of the pause, so the reported gap
        // ends before the pause does and cannot claim its losses.
        let samples = vec![
            sample("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z", 1.0),
            sample("2026-01-01T01:00:00Z", "2026-01-01T01:00:01Z", 2.0),
        ];
        let pauses = vec![pause("2026-01-01T00:00:30Z", "2026-01-01T02:00:00Z", 180)];
        let continuity = continuity(
            &samples,
            &pauses,
            5,
            "2026-01-01T00:00:00Z",
            "2026-01-01T01:00:00Z",
        );
        assert_eq!(continuity.gaps.len(), 1);
        assert_eq!(continuity.gaps[0].kind, GapKind::ProtectionPause);
        assert_eq!(continuity.gaps[0].to, "2026-01-01T01:00:00Z");
        assert_eq!(continuity.gaps[0].skipped_count, None);
    }

    #[test]
    fn a_known_pause_below_the_gap_threshold_is_reported_as_a_pause() {
        // Sixty seconds apart is not a silence worth inferring at this cadence,
        // but the instants the operator skipped inside it were still not
        // observed: the pair that brackets the pause is reported as a pause and
        // proves no coverage at all, so nothing can draw a line across it
        // (issue #213).
        let samples = vec![
            sample("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z", 1.0),
            sample("2026-01-01T00:01:00Z", "2026-01-01T00:01:01Z", 2.0),
        ];
        let pauses = vec![pause("2026-01-01T00:00:10Z", "2026-01-01T00:00:40Z", 2)];
        let continuity = continuity(
            &samples,
            &pauses,
            5,
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:01:00Z",
        );
        assert_eq!(continuity.coverage_seconds, 0);
        assert_eq!(continuity.gaps.len(), 1);
        assert_eq!(continuity.gaps[0].kind, GapKind::ProtectionPause);
        assert_eq!(continuity.gaps[0].from, "2026-01-01T00:00:00Z");
        assert_eq!(continuity.gaps[0].to, "2026-01-01T00:01:00Z");
        assert_eq!(continuity.gaps[0].seconds, 60);
        // The counted losses belong to this stretch: it covers the whole pause.
        assert_eq!(continuity.gaps[0].skipped_count, Some(2));
    }

    #[test]
    fn one_skipped_instant_disproves_the_whole_stretch_it_sits_in() {
        // A single recorded skip names one instant the Server did not observe,
        // not the length of the unobserved stretch around it, so the pair that
        // brackets it proves no coverage (issue #213).
        let samples = vec![
            sample("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z", 1.0),
            sample("2026-01-01T00:01:00Z", "2026-01-01T00:01:01Z", 2.0),
        ];
        let pauses = vec![pause("2026-01-01T00:00:30Z", "2026-01-01T00:00:30Z", 1)];
        let continuity = continuity(
            &samples,
            &pauses,
            5,
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:01:00Z",
        );
        assert_eq!(continuity.coverage_seconds, 0);
        assert_eq!(continuity.gaps.len(), 1);
        assert_eq!(continuity.gaps[0].kind, GapKind::ProtectionPause);
        assert_eq!(continuity.gaps[0].seconds, 60);
        assert_eq!(continuity.gaps[0].skipped_count, Some(1));
    }

    #[test]
    fn a_pause_that_starts_before_the_window_reports_only_the_window() {
        // The ledger records the whole protection interval, not the part of it
        // this answer asked about: a one hour window inside a ten hour pause
        // reports the hour, and the interval-wide loss count stays unknown
        // because this stretch does not cover the pause (issue #213).
        let pauses = vec![pause("2026-01-01T00:00:30Z", "2026-01-01T10:00:00Z", 1180)];
        let continuity = continuity(
            &[],
            &pauses,
            5,
            "2026-01-01T00:02:00Z",
            "2026-01-01T01:00:00Z",
        );
        assert_eq!(continuity.gaps.len(), 1);
        assert_eq!(continuity.gaps[0].from, "2026-01-01T00:02:00Z");
        assert_eq!(continuity.gaps[0].to, "2026-01-01T01:00:00Z");
        assert_eq!(continuity.gaps[0].seconds, 3480);
        assert_eq!(continuity.gaps[0].skipped_count, None);
    }

    #[test]
    fn one_observation_proves_no_coverage_beyond_itself() {
        let single = vec![sample("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z", 1.0)];
        let continuity = continuity(
            &single,
            &[],
            observed_cadence_seconds(&single),
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        );
        assert_eq!(continuity.coverage_seconds, 0);
        assert!(continuity.gaps.is_empty());
        assert_eq!(observed_cadence_seconds(&[]), 0);
    }

    #[test]
    fn counting_an_observation_no_row_can_answer_for_leaves_a_floor_above_it() {
        // The floor is the smallest stamp that says "this instant is no longer
        // provable", so the instant itself is a replay from then on and the next
        // second is not claimed to be observed.
        assert_eq!(
            counted_evidence_floor("2026-01-01T00:00:00Z").as_deref(),
            Some("2026-01-01T00:00:01Z")
        );
        assert_eq!(
            counted_evidence_floor("2026-01-01T23:59:59Z").as_deref(),
            Some("2026-01-02T00:00:00Z")
        );
        assert_eq!(counted_evidence_floor("2026-01-01 00:00:00"), None);
        assert_eq!(counted_evidence_floor("not an instant"), None);
    }

    #[test]
    fn the_gap_floor_keeps_a_fast_series_from_reporting_jitter() {
        assert_eq!(gap_threshold_seconds(5), MIN_GAP_SECONDS);
        assert_eq!(gap_threshold_seconds(60), 180);
        assert_eq!(gap_threshold_seconds(0), MIN_GAP_SECONDS);
        // A measured cadence is capped at the slowest interval an Agent may be
        // configured with, so two observations a day apart cannot pass a whole
        // day off as one proved cadence.
        assert_eq!(gap_threshold_seconds(MAX_OBSERVED_CADENCE_SECONDS), 900);
        assert_eq!(gap_threshold_seconds(86_400), 900);
    }

    /// Issue #213: the pause lookup runs on every Owner metric-history request
    /// against a ledger that grows with the fleet, so it must be served by an
    /// index — the samples beside it already are, and a scan here would make the
    /// range route cost grow with every Node that has ever lost optional history.
    #[tokio::test]
    async fn the_pause_lookup_is_index_backed_and_never_scans_the_ledger() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let rows: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(&format!("EXPLAIN QUERY PLAN {PAUSE_LOOKUP_SQL}"))
                .fetch_all(database.pool())
                .await
                .unwrap();
        let plan = rows
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            !plan.contains("SCAN capacity_skipped_series"),
            "the pause lookup must not scan the capacity ledger: {plan}"
        );
        assert!(
            plan.contains("capacity_skipped_series_lookup_idx"),
            "the pause lookup must be served by the series lookup index: {plan}"
        );
    }

    #[test]
    fn only_a_delivery_the_window_cannot_answer_skips_its_row() {
        let cutoff = "2026-01-01T00:00:00Z";
        assert!(!outside_retained_window(
            None,
            "2026-01-01T12:00:00Z",
            cutoff
        ));
        assert!(!outside_retained_window(
            Some(1.0),
            "2025-12-01T00:00:00Z",
            cutoff
        ));
        assert!(!outside_retained_window(None, cutoff, cutoff));
        assert!(outside_retained_window(
            None,
            "2025-12-31T23:59:59Z",
            cutoff
        ));
    }
}
