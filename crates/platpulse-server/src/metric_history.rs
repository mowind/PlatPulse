//! Node metric history: the trusted raw window and the aggregate tiers behind
//! it, as served to the Owner-side Node Admin surface (issues #213 and #214,
//! design §11.4 and §11.6).
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
//!
//! # Beyond the raw window (issue #214, design §11.6)
//!
//! The raw window is bounded by its own retention family, and an Operator asks
//! about stretches older than it. The Server answers those stretches from the
//! *aggregate buckets* it wrote while the observations were arriving: a
//! 1-minute bucket for the stretch older than the raw window and up to day 7,
//! and a 5-minute bucket from day 7 up to the 30-day investigation horizon.
//!
//! * **An aggregate is a summary, not a reconstruction.** A bucket carries the
//!   extremes, the count and the newest value of the observations that arrived
//!   inside it. Nothing is derived from a finer or a coarser tier at read
//!   time, so widening an old stretch returns the bucket the Server wrote and
//!   never a raw sample, and an answer can never re-accumulate evidence it
//!   already counted: a bucket advances only for an observation the delivery
//!   classification accepted as new.
//! * **An empty bucket is absent.** No zero, no carried value and no line
//!   across a stretch nobody observed: a bucket exists only where an
//!   observation arrived, and its `sample_count` is what tells an Operator that
//!   the bucket is thin instead of pretending to be a closed stretch.
//! * **The tier boundaries are declared, not inferred.** Each stretch of an
//!   answer is served from exactly one tier, and the retention families
//!   (`one_minute_aggregate` and `five_minute_aggregate`) hold exactly the
//!   windows those tiers serve, so the oldest instant an answer can reach is a
//!   policy the Owner can read.
//! * **A silence is judged between real observation instants**, whatever tier
//!   they came from: the gap rule below is applied to the merged point
//!   sequence, and the coarser of the two points decides what counts as
//!   silence, so a stretch that straddles a tier boundary is still reported as
//!   the gap it is.

use sqlx::query::{Query, QueryAs, QueryScalar};
use sqlx::sqlite::SqliteArguments;
use sqlx::{Sqlite, SqliteConnection, SqlitePool, Transaction};
use time::OffsetDateTime;

use crate::auth::{format_rfc3339, parse_rfc3339};
use crate::capacity::SkippedScope;

/// The per-Host mount contract limit (platpulse-core/src/observation.rs:73):
/// one Report states at most this many mounts, which is what bounds the per-mount
/// Host storage series a single Report can add.
pub const MAX_HOST_MOUNTS: usize = 128;

/// The Node metric series the Server stores raw and serves to Admin.
///
/// The five consensus-key series were added by issue #217 (parent #202 story
/// 53): a Node's sync current/highest block and the epoch's highest QC, lock and
/// commit block are quantities, so they ride this engine with its raw window,
/// its 1-minute and 5-minute tiers, its gaps and its replay ledger. They are
/// separate series rather than one "height" series because a highest QC block
/// and a commit block are different facts about the same epoch and a reader
/// must be able to see one advance without the other.
///
/// What a Report could not collect has no sample: a failed chain probe writes
/// no row for these series, and the state log (crate::state_history) records
/// the failure itself, so a numerical aggregate never erases it.
pub const NODE_METRIC_SERIES: [&str; 10] = [
    "process_cpu_percent",
    "process_memory_percent",
    "data_directory_percent",
    "peer_inbound_count",
    "peer_outbound_count",
    "sync_current_block",
    "sync_highest_block",
    "consensus_highest_qc_block",
    "consensus_highest_lock_block",
    "consensus_highest_commit_block",
];

/// Whether a metric name is one of the stored Node series.
pub fn is_node_metric(metric: &str) -> bool {
    NODE_METRIC_SERIES.contains(&metric)
}

/// The Host metric series the Server stores raw and serves to Admin (issue
/// #215, design §11.4).
///
/// They are the readings the Agent already collects for the machine it runs on:
/// CPU, physical memory, Load 1/5/15, network rate and per-mount storage. A Host
/// runs several Nodes, so these series belong to the Agent: they are stored once
/// per Agent, referenced by every Node view of that Agent, and never deleted by
/// one Node's Purge (parent #202 story 51).
///
/// The values are the collected quantities - bytes, percent, a load average,
/// bytes per second - never a ratio this Server baked out of two of them, so a
/// stored `memory_total_bytes` or `disk_total_bytes` stays able to re-derive
/// the percentage that was true at that instant.
///
/// The two storage series are dimensioned by mount path (story 52): storage
/// history is identified by Agent and path, never by a device identity the
/// Server cannot honestly claim.
pub const HOST_METRIC_SERIES: [&str; 10] = [
    "cpu_percent",
    "memory_used_bytes",
    "memory_total_bytes",
    "load1",
    "load5",
    "load15",
    "network_rx_bytes_per_sec",
    "network_tx_bytes_per_sec",
    "disk_used_bytes",
    "disk_total_bytes",
];

/// Whether a metric name is one of the stored Host series. The name alone does
/// not identify a series: the two storage series are told apart by the mount
/// path they were read from.
pub fn is_host_metric(metric: &str) -> bool {
    HOST_METRIC_SERIES.contains(&metric)
}

/// Which tables and key columns hold one scope's metric history (issue #215).
///
/// There is one history engine, not one per scope. A scope differs in exactly
/// three ways: the tables it writes, the column that names its owner (a Node or
/// an Agent), and whether its series identity carries a dimension. Host storage
/// series do: their dimension is the mount path, so a path that moves to another
/// filesystem starts a new series instead of claiming the old one, and the skip
/// counts of two mounts are never merged into one number.
#[derive(Debug, Clone, Copy)]
pub struct HistorySchema {
    pub raw_table: &'static str,
    pub aggregate_table: &'static str,
    pub ledger_table: &'static str,
    /// The column naming the owner of the series: `node_id` or `agent_id`.
    pub scope_column: &'static str,
    /// Whether the series identity of this scope includes a dimension.
    pub has_dimension: bool,
    /// How a write of this scope that protection refused is spelled in the
    /// capacity ledger.
    pub skipped_scope: SkippedScope,
}

/// The Node scope: one set of series per Node, keyed by Node (issues #213 and
/// #214).
pub const NODE_HISTORY: HistorySchema = HistorySchema {
    raw_table: "node_metric_samples",
    aggregate_table: "node_metric_aggregates",
    ledger_table: "node_metric_series_state",
    scope_column: "node_id",
    has_dimension: false,
    skipped_scope: SkippedScope::Node,
};

/// The Host scope: one set of series per Agent, shared by every Node of that
/// Agent, dimensioned by mount path for the storage series (issue #215).
pub const HOST_HISTORY: HistorySchema = HistorySchema {
    raw_table: "host_metric_samples",
    aggregate_table: "host_metric_aggregates",
    ledger_table: "host_metric_series_state",
    scope_column: "agent_id",
    has_dimension: true,
    skipped_scope: SkippedScope::Host,
};

impl HistorySchema {
    /// Render one statement template for this scope.
    ///
    /// Every statement below is written once and filled in here: `{raw}`,
    /// `{agg}` and `{ledger}` are table names, `{scope}` is the owner
    /// column, and the dimension fragments are dropped entirely for a scope
    /// whose series have no dimension. One definition therefore keeps one bind
    /// order for both scopes, and a dimensioned series cannot be reached through
    /// a statement that forgot its dimension: the predicate is part of the
    /// template, not something each caller has to remember.
    pub fn sql(&self, template: &str) -> String {
        template
            .replace("{raw}", self.raw_table)
            .replace("{agg}", self.aggregate_table)
            .replace("{ledger}", self.ledger_table)
            .replace("{scope}", self.scope_column)
            .replace(
                "{dim_column}",
                if self.has_dimension {
                    ", dimension"
                } else {
                    ""
                },
            )
            .replace("{dim_bind}", if self.has_dimension { ", ?" } else { "" })
            .replace(
                "{dim_predicate}",
                if self.has_dimension {
                    " AND dimension = ?"
                } else {
                    ""
                },
            )
    }
}

/// The identity of one series inside a scope.
#[derive(Debug, Clone, Copy)]
pub struct SeriesKey<'a> {
    pub metric: &'a str,
    /// The mount path of a per-mount Host series. Empty for every series whose
    /// scope has no dimension.
    pub dimension: &'a str,
}

/// One scope's series: the schema that holds it, the Agent or Node that owns it,
/// and the metric - with its dimension - that identifies it inside that scope.
#[derive(Debug, Clone, Copy)]
pub struct SeriesScope<'a> {
    pub schema: &'a HistorySchema,
    pub scope_key: &'a str,
    pub series: SeriesKey<'a>,
}

impl<'a> SeriesScope<'a> {
    /// The series of one Node.
    pub fn node(node_id: &'a str, metric: &'a str) -> Self {
        Self {
            schema: &NODE_HISTORY,
            scope_key: node_id,
            series: SeriesKey {
                metric,
                dimension: "",
            },
        }
    }

    /// The shared series of one Agent. `dimension` is the mount path for the
    /// storage series and empty for every other Host series.
    pub fn host(agent_id: &'a str, metric: &'a str, dimension: &'a str) -> Self {
        Self {
            schema: &HOST_HISTORY,
            scope_key: agent_id,
            series: SeriesKey { metric, dimension },
        }
    }

    /// The statement text of one template for this scope.
    fn sql(&self, template: &str) -> String {
        self.schema.sql(template)
    }

    /// Bind the series identity of one statement: the owner, the metric, and,
    /// when the scope has one, the dimension that separates two series of that
    /// owner and metric.
    ///
    /// sqlx binds placeholders in call order, so this must be called at the
    /// point in the chain where the series predicate appears in the statement
    /// text. Every read statement and every upsert in this module starts with
    /// `{scope} = ? AND metric = ?`, so there it is called first, straight
    /// around a fresh statement. The two statements that rewrite a bucket begin
    /// with a `SET` clause instead — see their own notes — and there the call
    /// wraps a statement that has already been given its `SET` values.
    fn bind_series<'q, S: SeriesStatement<'q>>(&'q self, stmt: S) -> S {
        let stmt = stmt.bind_owner_metric(self.scope_key, self.series.metric);
        if self.schema.has_dimension {
            stmt.bind_dimension(self.series.dimension)
        } else {
            stmt
        }
    }
}

/// A statement that has not been given its series identity yet.
///
/// Every statement in this module starts with the same binds — the owner, the
/// metric, and the dimension when the scope has one — but sqlx exposes three
/// statement shapes (an untyped row statement, a typed one and a scalar one)
/// that share no trait, so each one states how it binds a single column and
/// [`SeriesScope::bind_series`] decides which columns the scope binds.
trait SeriesStatement<'q> {
    /// Bind the series owner and its metric.
    fn bind_owner_metric(self, scope_key: &'q str, metric: &'q str) -> Self;

    /// Bind the dimension that separates two series of one owner and metric.
    fn bind_dimension(self, dimension: &'q str) -> Self;
}

impl<'q> SeriesStatement<'q> for Query<'q, Sqlite, SqliteArguments<'q>> {
    fn bind_owner_metric(self, scope_key: &'q str, metric: &'q str) -> Self {
        self.bind(scope_key).bind(metric)
    }

    fn bind_dimension(self, dimension: &'q str) -> Self {
        self.bind(dimension)
    }
}

impl<'q, O> SeriesStatement<'q> for QueryAs<'q, Sqlite, O, SqliteArguments<'q>> {
    fn bind_owner_metric(self, scope_key: &'q str, metric: &'q str) -> Self {
        self.bind(scope_key).bind(metric)
    }

    fn bind_dimension(self, dimension: &'q str) -> Self {
        self.bind(dimension)
    }
}

impl<'q, O> SeriesStatement<'q> for QueryScalar<'q, Sqlite, O, SqliteArguments<'q>> {
    fn bind_owner_metric(self, scope_key: &'q str, metric: &'q str) -> Self {
        self.bind(scope_key).bind(metric)
    }

    fn bind_dimension(self, dimension: &'q str) -> Self {
        self.bind(dimension)
    }
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

/// The 1-minute bucket width: the tier that answers the stretch older than the
/// raw window and up to day 7 (design §11.6).
/// How much of a series is answered with stored observations rather than with
/// a bucket.
///
/// The raw window is part of the history contract, not a consequence of the raw
/// retention policy: the API promises 24 hours of raw samples, and a policy that
/// keeps raw rows longer keeps them so the tiers can be written from them - it
/// does not change which grain an age is answered at. A policy shorter than this
/// cannot be configured, because the raw family's own floor is one day.
pub const RAW_WINDOW_HOURS: i64 = 24;

/// The instant that separates the raw window from the aggregate tiers.
pub fn raw_window_cutoff(now: time::OffsetDateTime) -> time::OffsetDateTime {
    now - time::Duration::hours(RAW_WINDOW_HOURS)
}

pub const ONE_MINUTE_SECONDS: i64 = 60;

/// The 5-minute bucket width: the tier that answers day 7 up to the
/// investigation horizon.
pub const FIVE_MINUTE_SECONDS: i64 = 300;

/// How old a stretch must be before the 5-minute tier answers it, in days.
pub const ONE_MINUTE_MAX_AGE_DAYS: i64 = 7;

/// The investigation horizon: the oldest age any tier answers, in days
/// (design §6).
///
/// It is also the registered floor of the `five_minute_aggregate` retention
/// family, so the stretch the tier serves and the window its policy keeps are
/// deliberately the same number: a policy that kept less would leave a hole
/// inside the horizon the range contract promises.
pub const FIVE_MINUTE_MAX_AGE_DAYS: i64 = 30;

/// The bucket widths the Server writes for one counted observation, finest
/// first.
pub const AGGREGATE_GRAINS: [i64; 2] = [ONE_MINUTE_SECONDS, FIVE_MINUTE_SECONDS];

/// The grain label an Admin answer carries for each tier.
pub const GRAIN_RAW: &str = "raw";
pub const GRAIN_ONE_MINUTE: &str = "1m";
pub const GRAIN_FIVE_MINUTE: &str = "5m";

/// Where an answered point came from: a stored observation, or the buckets that
/// summarize observations the raw window no longer holds.
pub const SOURCE_RAW: &str = "raw";
pub const SOURCE_AGGREGATE: &str = "aggregate";

/// The grain label for a stored bucket width.
pub fn grain_label(grain_seconds: i64) -> &'static str {
    match grain_seconds {
        ONE_MINUTE_SECONDS => GRAIN_ONE_MINUTE,
        FIVE_MINUTE_SECONDS => GRAIN_FIVE_MINUTE,
        _ => "unknown",
    }
}

/// The plain-language name of a bucket width, for messages that must name the
/// resolution an answer was served at.
pub fn grain_description(grain_seconds: i64) -> &'static str {
    match grain_seconds {
        ONE_MINUTE_SECONDS => "1-minute",
        FIVE_MINUTE_SECONDS => "5-minute",
        _ => "unknown",
    }
}

/// The UTC-aligned start of the bucket `instant` falls in.
///
/// Buckets are aligned on the Unix epoch in UTC (`div_euclid`, so an instant
/// before the epoch aligns backwards too). That makes a bucket boundary a
/// fixed, timezone-free instant the writer, the reader, the cleanup and the
/// Owner-facing policy can each name without re-deriving it.
///
/// `None` when the instant is not a canonical RFC 3339 UTC value: an
/// unusable instant is refused rather than silently bucketed somewhere.
pub fn aligned_bucket_start(instant: &str, grain_seconds: i64) -> Option<String> {
    // A bucket start is compared as text against the stored instants and is
    // written back into a column that requires the canonical shape, so the
    // instant it is derived from must be canonical too: a fractional second
    // would align somewhere the range comparison does not agree with. A width
    // that is not a positive number of seconds has no buckets at all.
    if grain_seconds <= 0 {
        return None;
    }
    let parsed = canonical_instant(instant)?;
    let aligned = parsed.unix_timestamp().div_euclid(grain_seconds) * grain_seconds;
    Some(format_rfc3339(
        OffsetDateTime::from_unix_timestamp(aligned).ok()?,
    ))
}

/// The first bucket boundary at or after the given instant for this grain.
///
/// Tier boundaries move onto real bucket edges so that exactly one tier answers
/// each bucket: the coarse tier answers the bucket that straddles the handover
/// whole - it counted every observation of its own window when they arrived -
/// and the finer tier picks up at that bucket's end. None when the instant is
/// not canonical or the width has no buckets.
fn aligned_bucket_end(instant: &str, grain_seconds: i64) -> Option<String> {
    if grain_seconds <= 0 {
        return None;
    }
    let parsed = canonical_instant(instant)?;
    let seconds = parsed.unix_timestamp();
    let remainder = seconds.rem_euclid(grain_seconds);
    let boundary = if remainder == 0 {
        seconds
    } else {
        seconds + (grain_seconds - remainder)
    };
    Some(format_rfc3339(
        OffsetDateTime::from_unix_timestamp(boundary).ok()?,
    ))
}

/// One stored raw observation.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricSample {
    pub observed_at: String,
    pub received_at: String,
    pub value: f64,
}

/// One stored aggregate bucket: what a series' observations proved inside one
/// aligned window of one tier (issue #214).
///
/// A bucket is written when an observation arrives, so the row is the evidence
/// itself rather than a summary derived later from rows that may since have
/// been released.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricAggregate {
    /// The UTC-aligned start of the bucket.
    pub bucket_start: String,
    /// The bucket's width in seconds: 60 or 300.
    pub grain_seconds: i64,
    /// Observations the bucket counted. A replay, a repeated delivery of an
    /// instant already counted, and a sample the low-space protection refused
    /// never advance it.
    pub sample_count: i64,
    pub min_value: f64,
    pub max_value: f64,
    /// The newest value the bucket holds.
    pub last_value: f64,
    /// The bucket's oldest observation instant.
    pub first_observed_at: String,
    /// The bucket's newest observation instant.
    pub last_observed_at: String,
    /// The receipt that carried the newest observation.
    pub last_received_at: String,
    /// The widest silence between two consecutive observations the bucket
    /// counted, in seconds. Two stored instants prove a span, not coverage: the
    /// same count, first and last instant and extremes can hide a hole that
    /// would swallow most of the bucket's width. Zero means a single
    /// observation, whose span is zero anyway.
    pub max_gap_seconds: i64,
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

/// One point of a tiered range answer: a stored observation, or the bucket
/// that summarizes observations the raw window no longer holds.
///
/// One type answers both tiers so a caller never merges two shapes by hand, and
/// so an answer can state the grain and the evidence behind each point instead
/// of presenting a bucket as if it were a sample.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricRangePoint {
    /// The point's coordinate: the observation instant for a raw sample, the
    /// aligned bucket start for a bucket.
    pub instant: String,
    /// The grain this point is served at.
    pub grain: &'static str,
    /// Where the point came from.
    pub source: &'static str,
    pub value: f64,
    /// The extremes of the observations this point holds. A raw sample is its
    /// own extreme.
    pub min_value: f64,
    pub max_value: f64,
    /// Observations behind this point: 1 for a raw sample, the bucket's counted
    /// observations for a bucket.
    pub sample_count: i64,
    /// The newest observation this point holds.
    pub last_observed_at: String,
    /// The receipt that carried that newest observation.
    pub received_at: String,
    /// The instant the gap and coverage rule judges this point from: the
    /// observation instant for a raw sample, the bucket's first observation for
    /// a bucket. A bucket's own span is never used, so the rule stays a
    /// statement about instants the Server really observed, and a reader that
    /// wants the stretch a bucket began at reads this rather than the bucket
    /// label in instant.
    pub first_observed_at: String,
    /// The largest gap between two consecutive observations this point counted:
    /// 0 for a raw sample, and for a bucket the widest hole it recorded inside
    /// its own window. Two stored instants alone cannot say whether the stretch
    /// between them was observed; this figure is what turns a span into proved
    /// coverage, and what lets a reader see a hole the answer refuses to draw
    /// over.
    pub max_gap_seconds: i64,
    /// The cadence this point's tier proves: the series' observed raw cadence
    /// for raw samples, the bucket width for a bucket.
    pub cadence_seconds: i64,
    /// The cadence the gap and coverage rule judges this point with, which is
    /// not always the point's own resolution.
    ///
    /// A raw sample is judged by the cadence the answer measured for the
    /// series. A bucket is judged by that same measured cadence when the answer
    /// holds one, because a series that reports about once a minute really was
    /// silent for five of them however coarsely the stretch is drawn; when the
    /// answer could not measure a cadence, the bucket is judged by the coarsest
    /// spacing it proves itself, its width divided by the observations it
    /// counted, which is the only evidence a stretch with no raw window has.
    pub judged_cadence_seconds: i64,
}

/// One tier's stretch of a range answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricRangeSegment {
    pub grain: &'static str,
    pub source: &'static str,
    /// The stretch this segment answers. The older end is inclusive and the
    /// newer end exclusive, matching what the reader asked of the tier.
    pub from: String,
    pub to: String,
    pub point_count: i64,
    /// True when this segment hit the answer's limit, so the stretch holds more
    /// points than the answer carries for it.
    pub truncated: bool,
}

/// What the Server knows about one series over a requested stretch: the raw
/// window where it still holds samples, the aggregate buckets beyond it, and
/// the silences in between.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricRange {
    /// Every answered point, oldest first, whatever tier served it.
    pub points: Vec<MetricRangePoint>,
    /// The tiers this answer was served from, oldest stretch first.
    pub segments: Vec<MetricRangeSegment>,
    /// Series state; `None` means this series was never observed.
    pub ledger: Option<SeriesLedger>,
    pub gaps: Vec<MetricGap>,
    /// Seconds the answered points prove the series was being observed.
    pub coverage_seconds: i64,
    /// True when any segment hit the answer's limit.
    pub truncated: bool,
    /// When the answer is truncated, the coordinate to pass back as the
    /// exclusive upper bound of the next, older page. `None` when the answer
    /// already reaches the oldest end of the requested stretch.
    pub continuation: Option<String>,
}

/// Everything the tiered range reader needs to answer one request.
///
/// The instants are the caller's: the Admin route derives them from the query
/// parameters and the retention policy, so the reader never reads a clock of
/// its own and a test can pin every boundary.
#[derive(Debug, Clone)]
pub struct RangeQuery<'a> {
    /// The series this request reads: the owning scope, metric and dimension.
    pub scope: SeriesScope<'a>,
    /// The requested stretch, both ends inclusive.
    pub from: OffsetDateTime,
    pub to: OffsetDateTime,
    /// Exclusive upper bound of a paging request. `None` answers up to `to`.
    pub before: Option<OffsetDateTime>,
    /// The largest number of points the answer may carry, across every tier.
    pub limit: i64,
    /// The raw window's cutoff: the instant older than which the Server no
    /// longer holds raw samples for this series.
    pub raw_cutoff: OffsetDateTime,
    /// The instant the answer is judged from. It decides how far back the
    /// aggregate tiers can reach, so the same request read at a later instant
    /// legitimately reaches a different oldest bucket.
    pub now: OffsetDateTime,
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
    gap_threshold_seconds_with_floor(cadence_seconds, MIN_GAP_SECONDS)
}

/// The same threshold with the floor supplied by the caller.
///
/// MIN_GAP_SECONDS is the floor the metric history surfaces need: a series
/// watched over hours is allowed to stay quiet for two minutes without the
/// Server calling that quiet a gap. A window that is only 60 seconds long cannot
/// be judged by that floor at all - the floor sits beyond the whole window, so no
/// silence inside it could ever be reported and a coverage answer computed for
/// the latest-60-second charts would be dead code (issue #225). Such a window is
/// measured against its own cadence alone, which is why the floor is a parameter
/// of the rule: the rule stays one rule, and the caller that judges a window
/// shorter than the floor states the floor it wants.
pub fn gap_threshold_seconds_with_floor(cadence_seconds: i64, floor_seconds: i64) -> i64 {
    (cadence_seconds.clamp(1, MAX_OBSERVED_CADENCE_SECONDS) * GAP_CADENCE_FACTOR).max(floor_seconds)
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
    // One conversion, two floors: the rule below is the same one and only the
    // window's own floor differs, so there is nothing here to keep in step.
    continuity_with_floor(
        samples,
        pauses,
        cadence_seconds,
        window_start,
        window_end,
        MIN_GAP_SECONDS,
    )
}

/// The raw-sample entry point with the silence floor supplied by the caller.
///
/// The same rule as [`continuity`], for a window shorter than
/// [`MIN_GAP_SECONDS`]: the caller passes the floor its window can support, and
/// 0 means "judge this window against its own cadence alone". Only the
/// latest-60-second answer needs it (issue #225); every window measured in hours
/// keeps the floor [`continuity`] states, so no long-range answer changes.
pub fn continuity_with_floor(
    samples: &[MetricSample],
    pauses: &[ProtectionPause],
    cadence_seconds: i64,
    window_start: &str,
    window_end: &str,
    floor_seconds: i64,
) -> Continuity {
    let points: Vec<JudgedPoint<'_>> = samples
        .iter()
        .map(|sample| JudgedPoint {
            from: &sample.observed_at,
            until: &sample.observed_at,
            cadence_seconds,
            window_seconds: 0,
            max_gap_seconds: 0,
            bucket_start: None,
        })
        .collect();
    continuity_from(&points, pauses, window_start, window_end, floor_seconds)
}

/// A point of a series whose spacing is not the cadence of a raw sample.
///
/// A raw metric sample testifies to one instant and answers for no window. A
/// recorded state is different: the Server recorded one row BECAUSE that state
/// was the newest thing it knew, and the state stays that way until something
/// changes it, so a row also answers for the stretch that follows it - as far as
/// the point where the next report was due. window_seconds carries that width,
/// and the silence rule then measures a stretch against the cadence plus the
/// width instead of against the cadence alone, so a state that simply did not
/// change is never reported as a silence (issue #217, migration 0070).
pub struct ObservablePoint<'a> {
    /// The oldest instant the point testifies to.
    pub from: &'a str,
    /// The newest instant the point testifies to.
    pub until: &'a str,
    /// The width of the window the point answers for, in seconds.
    pub window_seconds: i64,
}

/// Derive the silences and the proved coverage of a point sequence whose points
/// carry their own window width.
///
/// The same rule as the raw-sample entry point above, with the two instants and
/// the window width of every point supplied by the caller instead of being fixed
/// at one instant and no window. Only the state history needs it, and it exists
/// so that both kinds of series are judged by one implementation rather than by
/// two that have to be kept in agreement.
pub fn continuity_with_windows(
    points: &[ObservablePoint<'_>],
    pauses: &[ProtectionPause],
    cadence_seconds: i64,
    window_start: &str,
    window_end: &str,
) -> Continuity {
    let judged: Vec<JudgedPoint<'_>> = points
        .iter()
        .map(|point| JudgedPoint {
            from: point.from,
            until: point.until,
            cadence_seconds,
            window_seconds: point.window_seconds,
            max_gap_seconds: 0,
            bucket_start: None,
        })
        .collect();
    continuity_from(&judged, pauses, window_start, window_end, MIN_GAP_SECONDS)
}

/// One point the gap and coverage rule judges, with the two instants it can
/// still testify to, the cadence of the tier that produced it and the width of
/// the window it speaks for.
///
/// A raw sample testifies to one instant, so its two ends are the same instant
/// and it speaks for no window. A bucket testifies to its first and its last
/// stored observation: the observations between them are gone, but the count
/// and the extremes remain, and the stretch the bucket counted is coverage the
/// tier must not erase.
struct JudgedPoint<'a> {
    from: &'a str,
    until: &'a str,
    cadence_seconds: i64,
    /// The width of the window this point answers for (0 for a raw sample).
    window_seconds: i64,
    /// The largest gap the point itself recorded between two consecutive
    /// observations it counted (0 for a raw sample, whose one instant has no
    /// consecutive pair).
    max_gap_seconds: i64,
    /// The coordinate of the bucket this point stands for, so two neighbouring
    /// buckets can be checked for the empty windows between them (None for a raw
    /// sample, which stands for an instant and answers only for itself).
    bucket_start: Option<&'a str>,
}

/// The silence that separates two consecutive points into a gap.
///
/// The cadence decides it, as it does for raw samples, with one addition for the
/// buckets: two neighbouring windows can leave up to one window plus one cadence
/// between the observations that bracket them without anything having been lost,
/// so the threshold is never shorter than that. `window_seconds` is the narrowest
/// window of the pair, and 0 for two raw samples.
fn silence_threshold_seconds(cadence_seconds: i64, window_seconds: i64, floor_seconds: i64) -> i64 {
    let cadence = cadence_seconds.clamp(1, MAX_OBSERVED_CADENCE_SECONDS);
    gap_threshold_seconds_with_floor(cadence, floor_seconds).max(window_seconds + cadence)
}

/// Whether the windows of two consecutive points are neighbours.
///
/// The tier writes a bucket exactly where an observation arrived, so a bucket
/// that is missing from the sequence is a window nobody observed at all - the
/// empty bucket the ticket forbids the Server to fill in or to bridge. The
/// cadence and window allowance above exists for the stretch between two windows
/// the tier really counted; it must never excuse a stretch whose own windows were
/// never written, or two buckets ten minutes apart would answer as one line
/// across the five minute window in between them.
///
/// Two buckets are neighbours when the later one starts where the earlier one
/// ends; a stretch the boundary hands from a coarse bucket to a finer one is
/// contiguous by construction. A pair involving a raw sample, or a coordinate
/// that cannot be parsed, keeps the cadence rule alone.
fn windows_abut(left: &JudgedPoint<'_>, right: &JudgedPoint<'_>) -> bool {
    let (Some(start), true) = (left.bucket_start, left.window_seconds > 0) else {
        return true;
    };
    let (Some(next), true) = (right.bucket_start, right.window_seconds > 0) else {
        return true;
    };
    match (parse_rfc3339(start), parse_rfc3339(next)) {
        (Some(start), Some(next)) => next == start + time::Duration::seconds(left.window_seconds),
        _ => true,
    }
}

/// Derive the silences and the proved coverage of a merged point sequence.
///
/// The rule is the documented one above, judged between the evidence each point
/// really holds instead of between the windows the points are labelled with. The
/// two points of a pair decide the silence threshold together, and the coarser of
/// their cadences is what the stretch is measured against; a pair of buckets
/// additionally excuses the window they answer for, so two neighbouring buckets
/// are never separated by a silence the tier itself could not have seen - but
/// only while their windows really are neighbours, because a window nobody wrote
/// is a stretch nobody observed. A
/// stretch bounded by a 5-minute bucket can be five minutes long without the
/// Server having been silent, while a stretch of the same length between two
/// 5-second samples is real silence, and a silence spanning a tier boundary is
/// still reported because the tiers are judged as one sequence rather than region
/// by region.
fn continuity_from(
    points: &[JudgedPoint<'_>],
    pauses: &[ProtectionPause],
    window_start: &str,
    window_end: &str,
    floor_seconds: i64,
) -> Continuity {
    let mut gaps = Vec::new();
    let mut coverage_seconds = 0;
    // A claim about coverage is a claim about the requested window, and a point
    // can reach outside it: the bucket whose window straddles the request's own
    // start is answered whole, and the stretch beyond the window end belongs to
    // the next page. Only the part of a stretch that lies inside the window is
    // ever counted, so the deliberate boundary overlap of the tiers can never add
    // a second to the answer's own coverage claim.
    let window_from = parse_rfc3339(window_start);
    let window_to = parse_rfc3339(window_end);
    let inside_window = |from: time::OffsetDateTime, until: time::OffsetDateTime| -> i64 {
        let from = window_from.map_or(from, |bound| from.max(bound));
        let until = window_to.map_or(until, |bound| until.min(bound));
        (until - from).whole_seconds().max(0)
    };
    // The stretch one point testifies to on its own is coverage, never a silence:
    // a bucket holds the observations it counted between its first and its last
    // one, and the tier's job is to keep that span rather than to erase it. Two
    // stored instants only say that the bucket starts and ends somewhere, though,
    // so the span is claimed as observed coverage only while the largest gap the
    // bucket itself recorded between two consecutive observations is shorter than
    // the silence this series' evidence allows: a bucket with a hole inside it
    // is not a continuous line, and the item reports the hole it proved
    // (max_gap_seconds) instead of a coverage figure nobody can support. The one
    // thing that cuts the span further is a protection pause the Server recorded
    // inside it, because a loss the Server knows about is never drawn as a
    // continuous line (issue #213).
    for point in points {
        let (Some(from), Some(until)) = (parse_rfc3339(point.from), parse_rfc3339(point.until))
        else {
            continue;
        };
        let seconds = (until - from).whole_seconds();
        if seconds <= 0 {
            continue;
        }
        if pause_intersects(pauses, point.from, point.until) {
            gaps.push(MetricGap {
                from: point.from.to_owned(),
                to: point.until.to_owned(),
                seconds,
                kind: GapKind::ProtectionPause,
                skipped_count: pause_overlapping(pauses, point.from, point.until)
                    .and_then(|pause| pause_count_within(pause, point.from, point.until)),
            });
        } else if point.max_gap_seconds
            < silence_threshold_seconds(point.cadence_seconds, 0, floor_seconds)
        {
            coverage_seconds += inside_window(from, until);
        }
    }
    for pair in points.windows(2) {
        // A silence is measured between the newest instant one point testifies to
        // and the oldest one the next does: a bucket's window is not the stretch it
        // counted, so the pair that brackets a hole is one point's last stored
        // observation and the next point's first.
        let (Some(previous), Some(next)) =
            (parse_rfc3339(pair[0].until), parse_rfc3339(pair[1].from))
        else {
            continue;
        };
        let seconds = (next - previous).whole_seconds();
        if seconds <= 0 {
            continue;
        }
        let cadence = pair[0].cadence_seconds.max(pair[1].cadence_seconds);
        let threshold = silence_threshold_seconds(
            cadence,
            pair[0].window_seconds.min(pair[1].window_seconds),
            floor_seconds,
        );
        // An unwritten window between two buckets is a silence whatever the
        // cadence allowance says: the allowance covers the stretch between two
        // windows that were counted, not a window that never was.
        if seconds >= threshold || !windows_abut(&pair[0], &pair[1]) {
            let (kind, skipped_count) = match pause_overlapping(pauses, pair[0].until, pair[1].from)
            {
                Some(pause) => (
                    GapKind::ProtectionPause,
                    pause_count_within(pause, pair[0].until, pair[1].from),
                ),
                None => (GapKind::Collection, None),
            };
            gaps.push(MetricGap {
                from: pair[0].until.to_owned(),
                to: pair[1].from.to_owned(),
                seconds,
                kind,
                skipped_count,
            });
        } else if pause_intersects(pauses, pair[0].until, pair[1].from) {
            // A pause the Server recorded is evidence in its own right: the
            // ledger names instants inside this stretch that were skipped, so the
            // stretch is reported as a pause even though it is shorter than the
            // silence this cadence would infer. A loss the Server knows about is
            // never drawn as a continuous line (issue #213). The interval-wide
            // count belongs to the stretch only when the stretch covers the whole
            // pause: the ledger records which instants were skipped, never how
            // long the unobserved stretch between them was.
            let skipped_count = pause_overlapping(pauses, pair[0].until, pair[1].from)
                .and_then(|pause| pause_count_within(pause, pair[0].until, pair[1].from));
            gaps.push(MetricGap {
                from: pair[0].until.to_owned(),
                to: pair[1].from.to_owned(),
                seconds,
                kind: GapKind::ProtectionPause,
                skipped_count,
            });
        } else {
            // Only proven continuity counts: the stretch between two stored
            // observations, never the stretch after the newest one, whose length
            // nobody observed.
            coverage_seconds += inside_window(previous, next);
        }
    }
    // A window can end inside a pause: the stretch after the newest stored
    // observation is then a pause gap too, bounded by the counted losses.
    let tail = points.last().map(|point| point.until);
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
///
/// This answers the range reader's question — the fastest interval the stored
/// evidence ever showed, so a chart can say what the window is capable of
/// resolving. It is deliberately not the question a silence verdict asks; that
/// one belongs to [`current_cadence_seconds`].
pub fn observed_cadence_seconds(samples: &[MetricSample]) -> i64 {
    samples
        .windows(2)
        .filter_map(gap_seconds)
        .min()
        .unwrap_or(0)
}

/// How far two consecutive intervals may differ and still be one rhythm.
const CADENCE_AGREEMENT_FACTOR: i64 = 2;

/// The rhythm the series is being observed at now, or 0 when its newest evidence
/// does not show one yet.
///
/// A silence verdict asks whether the Agent is still keeping the rhythm it has
/// been keeping, so this reads the newest interval and trusts it only once the
/// interval before it agrees within a factor of two. The fastest gap the window
/// ever showed is the wrong answer to that question: an Agent whose collection
/// interval was legally changed from five seconds to five minutes still holds its
/// fast pairs for another fifteen observations, and every path that keeps
/// reporting at the slow rhythm would be called silent before its next
/// observation was due.
///
/// A rhythm that has just changed is not a rhythm. Two intervals that disagree
/// answer 0, and 0 is "unknown cadence", never "zero cadence": the caller answers
/// unknown evidence as unknown rather than as silence, and the next observation
/// that agrees with its predecessor settles the question.
pub fn current_cadence_seconds(samples: &[MetricSample]) -> i64 {
    let gaps: Vec<i64> = samples.windows(2).filter_map(gap_seconds).collect();
    let Some((&newest, earlier)) = gaps.split_last() else {
        return 0;
    };
    if let Some(&previous) = earlier.last() {
        let disagree = newest > previous.saturating_mul(CADENCE_AGREEMENT_FACTOR)
            || previous > newest.saturating_mul(CADENCE_AGREEMENT_FACTOR);
        if disagree {
            return 0;
        }
    }
    newest
}

/// The positive seconds between two consecutive observations, if they are
/// ordered and distinct.
fn gap_seconds(pair: &[MetricSample]) -> Option<i64> {
    let previous = parse_rfc3339(&pair[0].observed_at)?;
    let next = parse_rfc3339(&pair[1].observed_at)?;
    let seconds = (next - previous).whole_seconds();
    (seconds > 0).then_some(seconds)
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
    scope: &SeriesScope<'_>,
    observed_at: &str,
    stored_value: Option<f64>,
    value: f64,
    cutoff: &str,
) -> Result<Delivery, sqlx::Error> {
    let sql = scope.sql(
        "SELECT last_observed_at, released_before FROM {ledger} WHERE {scope} = ? AND metric = ?{dim_predicate}",
    );
    let ledger: Option<(String, String)> = scope
        .bind_series(sqlx::query_as(&sql))
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
    scope: &SeriesScope<'_>,
    observed_at: &str,
    received_at: &str,
    delivery: Delivery,
) -> Result<(), sqlx::Error> {
    let is_new = delivery == Delivery::Observed;
    let is_correction = delivery == Delivery::Correction;
    let is_replay = delivery == Delivery::Replay;
    let sql = scope.sql(
        "INSERT INTO {ledger} ({scope}, metric{dim_column}, first_observed_at, last_observed_at, last_received_at, observation_count, replayed_count, corrected_count, updated_at) VALUES (?, ?{dim_bind}, ?, ?, ?, 1, 0, 0, ?) ON CONFLICT({scope}, metric{dim_column}) DO UPDATE SET first_observed_at = MIN({ledger}.first_observed_at, excluded.first_observed_at), last_received_at = CASE WHEN excluded.last_observed_at >= {ledger}.last_observed_at THEN excluded.last_received_at ELSE {ledger}.last_received_at END, last_observed_at = MAX({ledger}.last_observed_at, excluded.last_observed_at), observation_count = {ledger}.observation_count + ?, replayed_count = {ledger}.replayed_count + ?, corrected_count = {ledger}.corrected_count + ?, updated_at = excluded.updated_at",
    );
    scope
        .bind_series(sqlx::query(&sql))
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
    scope: &SeriesScope<'_>,
    observed_at: &str,
) -> Result<Option<f64>, sqlx::Error> {
    let sql = scope.sql(
        "SELECT value FROM {raw} WHERE {scope} = ? AND metric = ?{dim_predicate} AND observed_at = ?",
    );
    scope
        .bind_series(sqlx::query_scalar(&sql))
        .bind(observed_at)
        .fetch_optional(&mut **tx)
        .await
}

/// Store one raw observation, or restate the value of a stored one.
///
/// The statement is the schema's, so a dimensioned series is written and
/// matched on its dimension: two mounts observed in the same Report keep their
/// own rows, and a restatement reaches the mount path it names rather than the
/// first row that shares an instant.
pub async fn store_sample(
    tx: &mut Transaction<'_, Sqlite>,
    scope: &SeriesScope<'_>,
    observed_at: &str,
    received_at: &str,
    value: f64,
) -> Result<(), sqlx::Error> {
    let sql = scope.sql(
        "INSERT INTO {raw} ({scope}, metric{dim_column}, observed_at, received_at, value) VALUES (?, ?{dim_bind}, ?, ?, ?) ON CONFLICT({scope}, metric{dim_column}, observed_at) DO UPDATE SET value = excluded.value",
    );
    scope
        .bind_series(sqlx::query(&sql))
        .bind(observed_at)
        .bind(received_at)
        .bind(value)
        .execute(&mut **tx)
        .await?;
    Ok(())
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
    scope: &SeriesScope<'_>,
    floor: &str,
) -> Result<(), sqlx::Error> {
    let sql = scope.sql(
        "UPDATE {ledger} SET released_before = MAX(released_before, ?) WHERE {scope} = ? AND metric = ?{dim_predicate} AND released_before < ?",
    );
    scope
        .bind_series(sqlx::query(&sql).bind(floor))
        .bind(floor)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The bucket upsert: one row per (series, tier, bucket), advanced by exactly
/// one counted observation.
///
/// Held as a constant so the ingestion path and the tests beside it run the
/// same statement. The extremes are folded with SQLite's MIN/MAX over the
/// canonical instants and values, the newest reading follows the newest
/// *observation* rather than the newest delivery, and the row's update instant
/// moves on every accepted observation so a bucket that changed only in count
/// is still visibly current.
const AGGREGATE_UPSERT_SQL: &str = "INSERT INTO {agg} ({scope}, metric{dim_column}, grain_seconds, bucket_start, sample_count, min_value, max_value, last_value, first_observed_at, last_observed_at, last_received_at, max_gap_seconds, updated_at) VALUES (?, ?{dim_bind}, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT ({scope}, metric{dim_column}, grain_seconds, bucket_start) DO UPDATE SET sample_count = {agg}.sample_count + 1, min_value = MIN({agg}.min_value, excluded.min_value), max_value = MAX({agg}.max_value, excluded.max_value), last_value = CASE WHEN excluded.last_observed_at >= {agg}.last_observed_at THEN excluded.last_value ELSE {agg}.last_value END, first_observed_at = MIN({agg}.first_observed_at, excluded.first_observed_at), last_observed_at = MAX({agg}.last_observed_at, excluded.last_observed_at), last_received_at = CASE WHEN excluded.last_observed_at >= {agg}.last_observed_at THEN excluded.last_received_at ELSE {agg}.last_received_at END, max_gap_seconds = MAX({agg}.max_gap_seconds, excluded.max_gap_seconds), updated_at = excluded.updated_at";

/// What a bucket already knows about its own continuity.
const BUCKET_GAP_STATE_SQL: &str = "SELECT first_observed_at, last_observed_at, max_gap_seconds FROM {agg} WHERE {scope} = ? AND metric = ?{dim_predicate} AND grain_seconds = ? AND bucket_start = ?";

/// The gap a newly arriving observation is known to leave inside its bucket.
///
/// A bucket's stored first and last observation bound the arriving instant's
/// neighbours: an instant newer than the last one adds exactly one new
/// consecutive pair, an instant older than the first one adds exactly one too,
/// and an instant between them replaces one pair with two. In the last case the
/// pair it removes can only make this figure an over-estimate, never an
/// under-estimate, and over-estimating is the safe direction: a bucket's span is
/// claimed as observed coverage only while its largest recorded gap is below the
/// silence threshold, so an over-estimate can refuse a claim the evidence does
/// not support but can never invent one. The arithmetic runs in Rust over
/// canonical instants because that is what the rest of the read path compares,
/// not a SQL date function whose accepted input formats are a library detail.
fn candidate_gap_seconds(
    first_observed_at: &str,
    last_observed_at: &str,
    observed_at: &str,
) -> i64 {
    let Some(instant) = parse_rfc3339(observed_at) else {
        return 0;
    };
    if let Some(last) = parse_rfc3339(last_observed_at)
        && instant > last
    {
        return (instant - last).whole_seconds();
    }
    if let Some(first) = parse_rfc3339(first_observed_at)
        && instant < first
    {
        return (first - instant).whole_seconds();
    }
    0
}

/// Accumulate one accepted observation into both aggregate tiers.
///
/// This is called inside the receipt transaction, and only for an observation
/// the delivery classification accepted as new, so a bucket can never count
/// something the series ledger did not count: a replayed Report, a repeated
/// delivery of an instant already counted and a sample the low-space
/// protection refused do not reach this function at all. Both tiers are written
/// whatever the raw window's policy currently is, because the tiers answer a
/// stretch the raw window may cover today and release tomorrow, and skipping
/// the write under a wide policy would leave a hole if the policy were later
/// narrowed.
///
/// An instant that is not a canonical RFC 3339 UTC value is refused by the
/// bucket alignment and contributes no bucket. Stored observations are
/// canonical by construction (the tables constrain their length), so this is a
/// guard rather than a policy.
pub async fn record_aggregates(
    tx: &mut Transaction<'_, Sqlite>,
    scope: &SeriesScope<'_>,
    observed_at: &str,
    received_at: &str,
    value: f64,
) -> Result<(), sqlx::Error> {
    let gap_state_sql = scope.sql(BUCKET_GAP_STATE_SQL);
    let upsert_sql = scope.sql(AGGREGATE_UPSERT_SQL);
    for grain_seconds in AGGREGATE_GRAINS {
        let Some(bucket_start) = aligned_bucket_start(observed_at, grain_seconds) else {
            continue;
        };
        // How much of the bucket's own span may later be claimed as observed
        // coverage is decided by the largest gap between two observations it
        // counted, so that figure is maintained here rather than guessed at read
        // time from two instants and a count.
        let stored = scope
            .bind_series(sqlx::query_as::<_, (String, String, i64)>(&gap_state_sql))
            .bind(grain_seconds)
            .bind(&bucket_start)
            .fetch_optional(&mut **tx)
            .await?;
        let max_gap_seconds = stored.map_or(0, |(first, last, recorded)| {
            recorded.max(candidate_gap_seconds(&first, &last, observed_at))
        });
        scope
            .bind_series(sqlx::query(&upsert_sql))
            .bind(grain_seconds)
            .bind(&bucket_start)
            .bind(value)
            .bind(value)
            .bind(value)
            .bind(observed_at)
            .bind(observed_at)
            .bind(received_at)
            .bind(max_gap_seconds)
            .bind(received_at)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// The envelope the raw rows still retained for one bucket window prove.
const BUCKET_ENVELOPE_SQL: &str = "SELECT COUNT(*), MIN(value), MAX(value) FROM {raw} WHERE {scope} = ? AND metric = ?{dim_predicate} AND observed_at >= ? AND observed_at < ?";

/// The first raw row retained for one bucket window.
const NEWEST_RAW_FIRST_SQL: &str = "SELECT MIN(observed_at) FROM {raw} WHERE {scope} = ? AND metric = ?{dim_predicate} AND observed_at >= ? AND observed_at < ?";

/// The newest raw row retained for one bucket window.
const BUCKET_NEWEST_SQL: &str = "SELECT observed_at, received_at, value FROM {raw} WHERE {scope} = ? AND metric = ?{dim_predicate} AND observed_at >= ? AND observed_at < ? ORDER BY observed_at DESC LIMIT 1";

/// How many observations the stored bucket counted.
const BUCKET_STATE_SQL: &str = "SELECT sample_count FROM {agg} WHERE {scope} = ? AND metric = ?{dim_predicate} AND grain_seconds = ? AND bucket_start = ?";

/// Replace a bucket's whole envelope from rows that account for every
/// observation the bucket counted.
///
/// A `SET` clause comes before the `WHERE` one in the statement text, so a
/// caller binds the seven envelope values first and gives the statement its
/// series identity after them (see `SeriesScope::bind_series`).
const BUCKET_REPLACE_SQL: &str = "UPDATE {agg} SET min_value = ?, max_value = ?, last_value = ?, first_observed_at = ?, last_observed_at = ?, last_received_at = ?, updated_at = ? WHERE {scope} = ? AND metric = ?{dim_predicate} AND grain_seconds = ? AND bucket_start = ?";

/// Carry a corrected reading into a bucket whose envelope is not fully
/// accounted for by retained rows.
///
/// The corrected value widens the stored envelope - the bucket's extremes must
/// contain every reading it counted, so a correction outside them extends the
/// envelope - while an extreme the Server can no longer restate is never
/// removed. The newest reading is carried forward only when the corrected
/// instant really is the bucket's newest, and the count is never touched.
///
/// The newest reading moves as one pair: a bucket states the reading at the
/// instant it was observed at, so the instant follows the value. In the
/// ordinary case the corrected instant already is the bucket's newest and
/// nothing moves; a bucket that counted fewer observations than its window
/// holds can be corrected at an instant newer than everything it counted, and
/// letting the value move alone would report that reading under another
/// observation's instant - a delay and a newest point computed from a pair that
/// never existed. The instant the value is carried to is a real observation of
/// the bucket's own window, so the pair names one true reading.
///
/// Its `SET` clause also precedes the `WHERE` one, so the caller binds those
/// nine values before the series identity (see `SeriesScope::bind_series`).
const BUCKET_CARRY_SQL: &str = "UPDATE {agg} SET min_value = MIN({agg}.min_value, ?), max_value = MAX({agg}.max_value, ?), last_value = CASE WHEN {agg}.last_observed_at <= ? THEN ? ELSE {agg}.last_value END, last_observed_at = CASE WHEN {agg}.last_observed_at <= ? THEN ? ELSE {agg}.last_observed_at END, last_received_at = CASE WHEN {agg}.last_observed_at <= ? THEN ? ELSE {agg}.last_received_at END, updated_at = ? WHERE {scope} = ? AND metric = ?{dim_predicate} AND grain_seconds = ? AND bucket_start = ?";

/// Re-derive the buckets that counted a corrected observation.
///
/// A correction rewrites one stored observation, so the bucket that counted it
/// may hold an extreme that no longer exists. The Server repairs a bucket only
/// when it can prove the replacement: the retained raw rows of the bucket
/// window must account for every observation the bucket counted
/// (`sample_count`), in which case the envelope is recomputed from them.
/// Otherwise some of the bucket's observations have been released and their
/// values are gone, so the extremes it proved while it held them stand -
/// narrowing them to a subset of the evidence would understate a stretch the
/// Server no longer has the rows to restate - while the corrected reading is
/// folded in, because an envelope that does not contain a reading the bucket
/// counted is simply wrong. Its newest reading is carried forward only when the
/// corrected instant really is the bucket's newest. The recorded continuity is
/// left alone: a correction restates a value at an instant the bucket already
/// counted, so it cannot change the gap between two observations.
///
/// The count never changes here: a correction is not a new observation, and the
/// ledger already counted the instant.
pub async fn recompute_aggregates(
    tx: &mut Transaction<'_, Sqlite>,
    scope: &SeriesScope<'_>,
    observed_at: &str,
    received_at: &str,
    value: f64,
) -> Result<(), sqlx::Error> {
    let state_sql = scope.sql(BUCKET_STATE_SQL);
    let envelope_sql = scope.sql(BUCKET_ENVELOPE_SQL);
    let newest_sql = scope.sql(BUCKET_NEWEST_SQL);
    let first_sql = scope.sql(NEWEST_RAW_FIRST_SQL);
    let replace_sql = scope.sql(BUCKET_REPLACE_SQL);
    let carry_sql = scope.sql(BUCKET_CARRY_SQL);
    for grain_seconds in AGGREGATE_GRAINS {
        let Some(bucket_start) = aligned_bucket_start(observed_at, grain_seconds) else {
            continue;
        };
        let Some(start) = parse_rfc3339(&bucket_start) else {
            continue;
        };
        let bucket_end = format_rfc3339(start + time::Duration::seconds(grain_seconds));
        let stored = scope
            .bind_series(sqlx::query_as::<_, (i64,)>(&state_sql))
            .bind(grain_seconds)
            .bind(&bucket_start)
            .fetch_optional(&mut **tx)
            .await?;
        // A bucket the Server never counted for this instant (the tiers were
        // introduced after that observation was stored) has no envelope to
        // repair, and inventing one from the rows that happen to be retained
        // would state a count the Server never counted.
        let Some((stored_count,)) = stored else {
            continue;
        };
        let envelope = scope
            .bind_series(sqlx::query_as::<_, (i64, Option<f64>, Option<f64>)>(
                &envelope_sql,
            ))
            .bind(&bucket_start)
            .bind(&bucket_end)
            .fetch_one(&mut **tx)
            .await?;
        let (retained_count, retained_min, retained_max) = envelope;
        if retained_count == 0 {
            continue;
        }
        if retained_count == stored_count {
            let newest = scope
                .bind_series(sqlx::query_as::<_, (String, String, f64)>(&newest_sql))
                .bind(&bucket_start)
                .bind(&bucket_end)
                .fetch_one(&mut **tx)
                .await?;
            let first_observed_at = scope
                .bind_series(sqlx::query_scalar::<_, String>(&first_sql))
                .bind(&bucket_start)
                .bind(&bucket_end)
                .fetch_one(&mut **tx)
                .await?;
            // The series binds sit in the chain where the statement text
            // states them: after the seven envelope values its SET clause
            // names, and before the bucket it selects.
            scope
                .bind_series(
                    sqlx::query(&replace_sql)
                        .bind(retained_min)
                        .bind(retained_max)
                        .bind(newest.2)
                        .bind(&first_observed_at)
                        .bind(&newest.0)
                        .bind(&newest.1)
                        .bind(received_at),
                )
                .bind(grain_seconds)
                .bind(&bucket_start)
                .execute(&mut **tx)
                .await?;
        } else {
            // Some of the bucket's observations are released, so their values are
            // gone and the envelope cannot be recomputed. What the Server still
            // knows is the corrected reading itself: it is inside the bucket, so
            // the envelope must contain it, and the extremes it cannot restate
            // stand rather than being narrowed to a subset of the evidence.
            // Same order as the replace above: the values its SET clause
            // names come first, then the series identity, then the bucket.
            scope
                .bind_series(
                    sqlx::query(&carry_sql)
                        .bind(value)
                        .bind(value)
                        .bind(observed_at)
                        .bind(value)
                        .bind(observed_at)
                        .bind(observed_at)
                        .bind(observed_at)
                        .bind(received_at)
                        .bind(received_at),
                )
                .bind(grain_seconds)
                .bind(&bucket_start)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}

/// The later of two canonical instants. Ordering canonical UTC instants as text
/// orders them in time, which is what every window bound here relies on.
fn later(left: &str, right: &str) -> String {
    if left >= right {
        left.to_owned()
    } else {
        right.to_owned()
    }
}

/// The earlier of two canonical instants.
fn earlier(left: &str, right: &str) -> String {
    if left <= right {
        left.to_owned()
    } else {
        right.to_owned()
    }
}

/// Read one series range for the Owner-side Admin surface.
///
/// The tiers do not overlap: the raw window answers from the raw retention
/// cutoff forward, the 1-minute tier answers from day 7 up to that cutoff, and
/// the 5-minute tier answers from day 30 up to day 7. A bucket whose window
/// straddles a boundary is the exception, and it is answered exactly once: the
/// bucket above the raw cutoff answers its minute whole and the raw region starts
/// where that bucket ends, and each handover inside the aggregate tiers is the
/// end of the bucket that straddles it. The raw region keeps the straddling
/// minute itself when the tier holds no bucket for it, which is the state of a
/// series counted before the tiers existed: its stored samples are still inside
/// the promised raw window and are the only evidence left for them.
///
/// The limit bounds the whole answer rather than one tier. The newest tier is
/// read first, so a truncated answer always carries the newest evidence and
/// simply stops there: the older tiers are not read at all, and the caller
/// pages older with the returned continuation coordinate instead of receiving
/// an answer with a hole in the middle of it. A budget that fits the newest
/// tiers exactly still reports a truncated answer when an older tier holds
/// evidence, because the answer must never call itself complete while it
/// dropped one.
pub async fn load_range(
    pool: &SqlitePool,
    query: RangeQuery<'_>,
) -> Result<MetricRange, sqlx::Error> {
    let limit = query.limit.max(0);
    let from_text = format_rfc3339(query.from);
    let to_text = format_rfc3339(query.to);
    let raw_cutoff_text = format_rfc3339(query.raw_cutoff);
    let one_minute_floor =
        format_rfc3339(query.now - time::Duration::days(ONE_MINUTE_MAX_AGE_DAYS));
    let five_minute_floor =
        format_rfc3339(query.now - time::Duration::days(FIVE_MINUTE_MAX_AGE_DAYS));
    // The aggregate tiers are bounded by the start of a bucket, so their upper
    // bound is exclusive; the raw window is bounded by observation instants, so
    // its own bound is inclusive. A paging cursor is a coordinate the caller has
    // already seen, and the raw bound steps back the one second that separates
    // two stored instants to exclude it.
    let (aggregate_ceiling, raw_ceiling) = match query.before {
        Some(before) => (
            format_rfc3339(before),
            format_rfc3339(before - time::Duration::seconds(1)),
        ),
        None => (to_text.clone(), to_text.clone()),
    };
    // A window whose floor is above its own ceiling has nothing in it, and the
    // Admin route really produces one: a request whose whole stretch is older
    // than the investigation horizon is clamped to that horizon, so a request
    // that ends just before the horizon arrives here with a floor after its
    // ceiling (review F3). The tier floor is aligned down to a bucket edge, so
    // the read answered the bucket that straddles the horizon whole - a point
    // holding evidence after the ceiling the caller asked for, inside an answer
    // that reports the stretch as unavailable. An empty window is answered as
    // empty and consults no tier at all; the ledger still answers, because what
    // a series observed is a fact about the series and not about this stretch.
    if from_text.as_str() > aggregate_ceiling.as_str() {
        return Ok(MetricRange {
            points: Vec::new(),
            segments: Vec::new(),
            ledger: load_ledger(pool, &query.scope).await?,
            gaps: Vec::new(),
            coverage_seconds: 0,
            truncated: false,
            continuation: None,
        });
    }
    // Tier boundaries are moved onto real bucket edges before anything is read,
    // because the raw region's own floor is one of them.
    //
    // At the floor of a tier the boundary moves down: the bucket the floor falls
    // inside also holds observations inside the horizon, and once the raw rows
    // are released the tier is the only evidence left for them, so that bucket
    // is answered whole instead of being dropped. At a handover the boundary
    // moves up to the coarser tier's next edge: the coarse bucket that straddles
    // the handover counted every observation of its own window when they
    // arrived, so it is answered whole and the finer tier picks up at its end.
    // No instant is answered by two regions, and none inside a served stretch is
    // dropped for being in the wrong bucket.
    //
    // A request that starts inside the raw window is a different case: every
    // sample it asks for is still stored, so the raw region runs from the request
    // itself and no aggregate tier is consulted at all - the minute straddling
    // the raw cutoff ends below the request floor, so no instant inside the
    // window needs its bucket. Only a request that reaches below the raw cutoff
    // hands that minute to the tier which counted it (the raw region then starts
    // at the minute edge and the tier answers that minute whole), and only while
    // that tier holds the bucket, which is why the conditional is on the request
    // floor and on the tier's own evidence rather than on the cutoff alone.
    let below_raw_cutoff = from_text.as_str() < raw_cutoff_text.as_str();
    let five_minute_handoff = aligned_bucket_end(&one_minute_floor, FIVE_MINUTE_SECONDS)
        .unwrap_or_else(|| one_minute_floor.clone());
    let raw_handoff = aligned_bucket_end(&raw_cutoff_text, ONE_MINUTE_SECONDS)
        .unwrap_or_else(|| raw_cutoff_text.clone());
    // Handing the straddling minute to the tier is only honest while that tier
    // really holds that minute whole. A series that was counted before issue #214
    // has retained raw rows and no buckets at all, because migration 0067 creates
    // the tier table empty and backfills nothing: moving the raw floor to the
    // minute's end would drop observations that sit inside the promised raw
    // window, with no bucket anywhere to answer for them (the read then returned
    // neither the sample nor a bucket, while a request starting at the cutoff
    // returned the stored row). A bucket that exists is not enough either: one
    // observation delivered after the tiers were introduced opens a bucket of its
    // own while the row that predates them stays stored and uncounted, so the
    // minute still holds more observations than its bucket counted. The probe asks
    // both questions at once - how many observations the minute's bucket counted
    // and how many raw rows the minute still stores - and only a bucket that
    // accounts for every stored row is handed over. The stored evidence is the
    // better answer, and the probe is what keeps an instant from being answered
    // twice: only a minute the tier can account for leaves the raw region.
    let straddling_minute = aligned_bucket_start(&raw_cutoff_text, ONE_MINUTE_SECONDS);
    let (handed_over, straddling_minute_counted) = match straddling_minute {
        Some(ref minute) if below_raw_cutoff && minute.as_str() < raw_handoff.as_str() => {
            let (counted, stored) = straddling_minute_ledger(
                pool,
                &query.scope,
                ONE_MINUTE_SECONDS,
                minute,
                &raw_handoff,
            )
            .await?;
            (counted > 0 && counted >= stored, counted > 0)
        }
        _ => (false, false),
    };
    let raw_floor = if handed_over {
        raw_handoff.clone()
    } else {
        later(&from_text, &raw_cutoff_text)
    };
    let mut points: Vec<MetricRangePoint> = Vec::new();
    let mut segments: Vec<MetricRangeSegment> = Vec::new();
    let mut remaining = limit;
    let mut stopped = false;

    // The cadence the whole answer is judged by comes from the raw window when
    // the answer holds one, so it is measured once here and carried onto every
    // point instead of being re-derived per tier.
    let mut measured_cadence: Option<i64> = None;
    let raw_from = raw_floor;
    if remaining > 0 && raw_from <= raw_ceiling {
        let (samples, segment_truncated) =
            read_raw_samples(pool, &query.scope, &raw_from, &raw_ceiling, remaining).await?;
        let cadence = observed_cadence_seconds(&samples);
        measured_cadence = Some(cadence).filter(|cadence| *cadence > 0);
        remaining -= samples.len() as i64;
        segments.push(MetricRangeSegment {
            grain: GRAIN_RAW,
            source: SOURCE_RAW,
            from: raw_from,
            to: raw_ceiling.clone(),
            point_count: samples.len() as i64,
            truncated: segment_truncated,
        });
        points.extend(samples.into_iter().map(|sample| {
            let MetricSample {
                observed_at,
                received_at,
                value,
            } = sample;
            MetricRangePoint {
                grain: GRAIN_RAW,
                source: SOURCE_RAW,
                min_value: value,
                max_value: value,
                sample_count: 1,
                last_observed_at: observed_at.clone(),
                received_at,
                first_observed_at: observed_at.clone(),
                max_gap_seconds: 0,
                cadence_seconds: cadence,
                // Filled in once the whole answer is known: the cadence the gap
                // and coverage rule judges this point by may come from a raw
                // window this tier was not read with.
                judged_cadence_seconds: 0,
                instant: observed_at,
                value,
            }
        }));
        stopped = segment_truncated;
    }

    // The aggregate tiers, coarser one last: a tier that had to be truncated
    // means the answer already stopped, and reading a coarser tier would leave
    // the answer with a hole between the two.
    // A tier's own stretch starts at a bucket boundary, not at an arbitrary
    // instant: the floor below the tier is a hard fact ("nothing older than day
    // 30 is answered") but the bucket that contains the floor's first instant
    // also holds observations inside it. That bucket is therefore answered
    // whole, which is why each region's lower bound is aligned down to the
    // tier's own width. The coarser tier's ceiling moves to that same aligned
    // instant so the two tiers do not both summarize the same stretch - the
    // floor the aligned bound sits on is the one boundary the answer can name
    // for both.
    let five_minute_stretch =
        aligned_bucket_start(&later(&from_text, &five_minute_floor), FIVE_MINUTE_SECONDS)
            .unwrap_or_else(|| later(&from_text, &five_minute_floor));
    let one_minute_stretch =
        aligned_bucket_start(&later(&from_text, &one_minute_floor), ONE_MINUTE_SECONDS)
            .unwrap_or_else(|| later(&from_text, &one_minute_floor));
    let one_minute_stretch = later(&one_minute_stretch, &five_minute_handoff);
    // The minute the raw cutoff falls inside is answered by its bucket only while
    // the tier counted every observation the minute still stores. When the bucket
    // is there but short of that, the minute's own stored samples answer it and
    // the finer region stops at the minute's own start, so that minute is never
    // answered by both regions at once.
    let one_minute_edge = if straddling_minute_counted && !handed_over {
        straddling_minute
            .clone()
            .unwrap_or_else(|| raw_handoff.clone())
    } else {
        raw_handoff.clone()
    };
    let (one_minute_ceiling, five_minute_ceiling) = if below_raw_cutoff {
        (
            earlier(&aggregate_ceiling, &one_minute_edge),
            earlier(
                &earlier(&aggregate_ceiling, &five_minute_handoff),
                &raw_handoff,
            ),
        )
    } else {
        // The whole request sits inside the raw window, so no tier answers any
        // of it: an empty region is neither read nor reported.
        (one_minute_stretch.clone(), five_minute_stretch.clone())
    };
    let aggregate_regions = [
        (ONE_MINUTE_SECONDS, one_minute_stretch, one_minute_ceiling),
        (
            FIVE_MINUTE_SECONDS,
            five_minute_stretch,
            five_minute_ceiling,
        ),
    ];
    for (grain_seconds, region_from, region_to) in aggregate_regions {
        if stopped {
            continue;
        }
        if remaining <= 0 {
            // The budget is spent, so this older tier is not read. If it holds
            // any evidence inside the requested range the answer is truncated
            // rather than complete: the caller pages older with the continuation
            // coordinate instead of reading a finished answer that dropped a
            // tier. A budget that fits a tier exactly is not a hole as long as
            // nothing older is left.
            if region_from < region_to
                && buckets_hold_evidence(
                    pool,
                    &query.scope,
                    grain_seconds,
                    &region_from,
                    &region_to,
                )
                .await?
            {
                stopped = true;
            }
            continue;
        }
        if region_from >= region_to {
            continue;
        }
        let (buckets, segment_truncated) = read_aggregates(
            pool,
            &query.scope,
            grain_seconds,
            &region_from,
            &region_to,
            remaining,
        )
        .await?;
        remaining -= buckets.len() as i64;
        segments.push(MetricRangeSegment {
            grain: grain_label(grain_seconds),
            source: SOURCE_AGGREGATE,
            from: region_from,
            to: region_to,
            point_count: buckets.len() as i64,
            truncated: segment_truncated,
        });
        points.extend(buckets.into_iter().map(|bucket| MetricRangePoint {
            instant: bucket.bucket_start,
            grain: grain_label(bucket.grain_seconds),
            source: SOURCE_AGGREGATE,
            value: bucket.last_value,
            min_value: bucket.min_value,
            max_value: bucket.max_value,
            sample_count: bucket.sample_count,
            last_observed_at: bucket.last_observed_at,
            received_at: bucket.last_received_at,
            first_observed_at: bucket.first_observed_at,
            max_gap_seconds: bucket.max_gap_seconds,
            cadence_seconds: bucket.grain_seconds,
            judged_cadence_seconds: 0,
        }));
        stopped = segment_truncated;
    }

    points.sort_by(|left, right| left.instant.cmp(&right.instant));
    segments.sort_by(|left, right| left.from.cmp(&right.from));
    // Stopping covers both a tier that hit the budget mid-read and an older
    // tier the spent budget never reached while it still held evidence.
    let truncated = segments.iter().any(|segment| segment.truncated) || stopped;
    let continuation = truncated
        .then(|| points.first().map(|point| point.instant.clone()))
        .flatten();
    let pauses = load_pauses(pool, &query.scope, &from_text, &to_text).await?;
    // The cadence the gap and coverage rule judges a point by is not always the
    // point's own resolution: a series that reports about once a minute really
    // was silent for five of them however coarsely the stretch is drawn, so a
    // bucket is judged by the cadence the answer measured from raw samples when
    // it holds any. A stretch answered with no raw window at all falls back to
    // the spacing the bucket itself proves - its width divided by the
    // observations it counted - which is the only cadence evidence there is.
    for point in &mut points {
        point.judged_cadence_seconds = match measured_cadence {
            Some(cadence) => cadence,
            None if point.grain == GRAIN_RAW => point.cadence_seconds,
            None => (point.cadence_seconds / point.sample_count.max(1)).max(1),
        };
    }
    let judged: Vec<JudgedPoint<'_>> = points
        .iter()
        .map(|point| JudgedPoint {
            from: &point.first_observed_at,
            until: &point.last_observed_at,
            cadence_seconds: point.judged_cadence_seconds,
            // A raw sample speaks for one instant and no window; a bucket speaks
            // for its own width, which is what excuses a silence of up to one
            // window plus one cadence between two neighbouring buckets.
            window_seconds: if point.grain == GRAIN_RAW {
                0
            } else {
                point.cadence_seconds
            },
            max_gap_seconds: point.max_gap_seconds,
            // The tier writes a bucket exactly where an observation arrived, so
            // the coordinate is what tells a pair of buckets whether the windows
            // between them were counted at all.
            bucket_start: (point.grain != GRAIN_RAW).then_some(point.instant.as_str()),
        })
        .collect();
    let window_end = if query.before.is_some() {
        aggregate_ceiling.as_str()
    } else {
        to_text.as_str()
    };
    let continuity = continuity_from(&judged, &pauses, &from_text, window_end, MIN_GAP_SECONDS);
    let ledger = load_ledger(pool, &query.scope).await?;
    Ok(MetricRange {
        points,
        segments,
        ledger,
        gaps: continuity.gaps,
        coverage_seconds: continuity.coverage_seconds,
        truncated,
        continuation,
    })
}

/// The stored observations of the raw window, oldest first.
const RANGE_SAMPLE_SQL: &str = "SELECT observed_at, received_at, value FROM {raw} WHERE {scope} = ? AND metric = ?{dim_predicate} AND observed_at >= ? AND observed_at <= ? ORDER BY observed_at DESC LIMIT ?";

/// The buckets of one tier over one stretch, newest first so the limit is spent
/// on the newest evidence.
///
/// The statement is served by the aggregate table's primary key, whose leading
/// columns are exactly the series and the tier and whose last column is the
/// bucket start, so neither the read nor its limit walks a table that grows
/// with the fleet.
const RANGE_BUCKET_SQL: &str = "SELECT bucket_start, grain_seconds, sample_count, min_value, max_value, last_value, first_observed_at, last_observed_at, last_received_at, max_gap_seconds FROM {agg} WHERE {scope} = ? AND metric = ?{dim_predicate} AND grain_seconds = ? AND bucket_start >= ? AND bucket_start < ? ORDER BY bucket_start DESC LIMIT ?";

/// Whether a stretch the answer's budget could not afford holds any evidence.
///
/// The answer may stop at its budget, but it must never call itself complete
/// while an older tier still holds a point inside the requested range: a
/// finished answer that silently dropped evidence is exactly the hole paging
/// exists to prevent. The probe is one indexed lookup, never a read of the
/// stretch itself, and it is served by the aggregate table's primary key like
/// the read itself.
const BUCKET_EVIDENCE_EXISTS_SQL: &str = "SELECT EXISTS(SELECT 1 FROM {agg} WHERE {scope} = ? AND metric = ?{dim_predicate} AND grain_seconds = ? AND bucket_start >= ? AND bucket_start < ?)";

/// What one straddling minute holds on both sides of the tier seam: how many
/// observations the minute's bucket counted, and how many raw rows the minute
/// still stores.
///
/// A bucket existing is not the same as the tier accounting for the minute.
/// Migration 0067 backfills nothing, so a series counted before the aggregate
/// tiers existed keeps the raw rows no tier ever counted, and a single
/// observation delivered afterwards opens a bucket whose count covers only
/// itself. Handing such a minute over would move the raw floor past observations
/// that are still inside the promised raw window and leave them answered by
/// nobody. Inside that window the tier has released nothing - release happens
/// years later, at the aggregate family's own cutoff - so a bucket's count is at
/// least the rows still stored in its minute whenever the tier really counted
/// the minute, and only that bucket is handed over.
const STRADDLING_MINUTE_LEDGER_SQL: &str = "SELECT COALESCE((SELECT sample_count FROM {agg} WHERE {scope} = ? AND metric = ?{dim_predicate} AND grain_seconds = ? AND bucket_start = ?), 0), (SELECT COUNT(*) FROM {raw} WHERE {scope} = ? AND metric = ?{dim_predicate} AND observed_at >= ? AND observed_at < ?)";

/// The series ledger read on every range request.
const SERIES_LEDGER_SQL: &str = "SELECT first_observed_at, last_observed_at, last_received_at, observation_count, replayed_count, corrected_count FROM {ledger} WHERE {scope} = ? AND metric = ?{dim_predicate}";

async fn read_raw_samples(
    pool: &SqlitePool,
    scope: &SeriesScope<'_>,
    from: &str,
    to: &str,
    limit: i64,
) -> Result<(Vec<MetricSample>, bool), sqlx::Error> {
    let sql = scope.sql(RANGE_SAMPLE_SQL);
    let rows = scope
        .bind_series(sqlx::query_as::<_, (String, String, f64)>(&sql))
        .bind(from)
        .bind(to)
        .bind(limit + 1)
        .fetch_all(pool)
        .await?;
    let truncated = rows.len() as i64 > limit;
    let mut samples: Vec<MetricSample> = rows
        .into_iter()
        .take(limit as usize)
        .map(|(observed_at, received_at, value)| MetricSample {
            observed_at,
            received_at,
            value,
        })
        .collect();
    samples.sort_by(|left, right| left.observed_at.cmp(&right.observed_at));
    Ok((samples, truncated))
}

/// One observation of the seam: the minute's bucket count (0 when the tier holds
/// no bucket for the minute) and the number of raw rows the minute still stores.
async fn straddling_minute_ledger(
    pool: &SqlitePool,
    scope: &SeriesScope<'_>,
    grain_seconds: i64,
    minute: &str,
    minute_end: &str,
) -> Result<(i64, i64), sqlx::Error> {
    let sql = scope.sql(STRADDLING_MINUTE_LEDGER_SQL);
    // The minute is asked about twice - once of the tier and once of the raw
    // window - so its series identity is bound twice, in the order the two
    // subqueries read it.
    let first_half = scope.bind_series(sqlx::query_as::<_, (i64, i64)>(&sql));
    let (counted, stored): (i64, i64) = scope
        .bind_series(first_half.bind(grain_seconds).bind(minute))
        .bind(minute)
        .bind(minute_end)
        .fetch_one(pool)
        .await?;
    Ok((counted, stored))
}

/// Whether one aggregate tier holds any bucket inside a stretch the budget could
/// not afford. A spent budget that skipped the raw window's older tiers is
/// covered by this same probe, so no separate raw-side lookup is needed.
async fn buckets_hold_evidence(
    pool: &SqlitePool,
    scope: &SeriesScope<'_>,
    grain_seconds: i64,
    from: &str,
    to: &str,
) -> Result<bool, sqlx::Error> {
    let sql = scope.sql(BUCKET_EVIDENCE_EXISTS_SQL);
    let (exists,): (bool,) = scope
        .bind_series(sqlx::query_as(&sql))
        .bind(grain_seconds)
        .bind(from)
        .bind(to)
        .fetch_one(pool)
        .await?;
    Ok(exists)
}

async fn read_aggregates(
    pool: &SqlitePool,
    scope: &SeriesScope<'_>,
    grain_seconds: i64,
    from: &str,
    to: &str,
    limit: i64,
) -> Result<(Vec<MetricAggregate>, bool), sqlx::Error> {
    let sql = scope.sql(RANGE_BUCKET_SQL);
    let rows = scope
        .bind_series(sqlx::query_as::<
            _,
            (String, i64, i64, f64, f64, f64, String, String, String, i64),
        >(&sql))
        .bind(grain_seconds)
        .bind(from)
        .bind(to)
        .bind(limit + 1)
        .fetch_all(pool)
        .await?;
    let truncated = rows.len() as i64 > limit;
    let mut buckets: Vec<MetricAggregate> = rows
        .into_iter()
        .take(limit as usize)
        .map(
            |(
                bucket_start,
                grain_seconds,
                sample_count,
                min_value,
                max_value,
                last_value,
                first_observed_at,
                last_observed_at,
                last_received_at,
                max_gap_seconds,
            )| MetricAggregate {
                bucket_start,
                grain_seconds,
                sample_count,
                min_value,
                max_value,
                last_value,
                first_observed_at,
                last_observed_at,
                last_received_at,
                max_gap_seconds,
            },
        )
        .collect();
    buckets.sort_by(|left, right| left.bucket_start.cmp(&right.bucket_start));
    Ok((buckets, truncated))
}

async fn load_ledger(
    pool: &SqlitePool,
    scope: &SeriesScope<'_>,
) -> Result<Option<SeriesLedger>, sqlx::Error> {
    let sql = scope.sql(SERIES_LEDGER_SQL);
    Ok(scope
        .bind_series(sqlx::query_as::<_, (String, String, String, i64, i64, i64)>(&sql))
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
        ))
}

/// The protection-pause lookup runs on every Owner metric-history request.
///
/// It is a range read of one series, and the ledger it reads grows with the
/// fleet, so it must be served by an index rather than a full scan: the
/// samples beside it are already fetched through an ordered range index, and a
/// scan here would make the range route cost grow with every Node that has ever
/// recorded a protection loss (migration 0066, issue #213). Held as a constant
/// so the plan test beside it asserts the statement the route really runs.
const PAUSE_LOOKUP_SQL: &str = "SELECT first_skipped_at, last_skipped_at, skipped_count FROM capacity_skipped_series WHERE scope_kind = ? AND scope_key = ? AND metric = ? AND dimension = ? AND last_skipped_at >= ? AND first_skipped_at <= ? ORDER BY first_skipped_at";

/// Protection losses recorded for one series inside the window.
///
/// These rows are the positive evidence that a silence was chosen rather than
/// suffered, so a protected stretch is never reported as an unexplained gap.
pub(crate) async fn load_pauses(
    pool: &SqlitePool,
    scope: &SeriesScope<'_>,
    from: &str,
    to: &str,
) -> Result<Vec<ProtectionPause>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, i64)>(PAUSE_LOOKUP_SQL)
        .bind(scope.schema.skipped_scope.as_str())
        .bind(scope.scope_key)
        .bind(scope.series.metric)
        .bind(scope.series.dimension)
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

/// The Host storage series that names the mount path it was read from: the
/// bytes in use.
pub const HOST_MOUNT_USED_METRIC: &str = "disk_used_bytes";

/// The Host storage series that states how much that same mount path can hold.
pub const HOST_MOUNT_CAPACITY_METRIC: &str = "disk_total_bytes";

/// The Host series whose cadence stands for the whole Host observation.
///
/// Every Host series is written from the one observation the Agent collected for
/// its Host, so the Agent has one sampling cadence and the storage series are
/// not a second one: the mount list measures it once instead of asking the same
/// question for every mount path.
pub const HOST_CADENCE_METRIC: &str = "cpu_percent";

/// The mount paths one Agent's coverage answer carries.
///
/// Twice the per-Report mount contract, so an Agent reporting its whole contract
/// is answered in one read while a Host whose paths churned wholesale is
/// answered with its newest paths and a stated truncation instead of with an
/// unbounded read.
pub const MOUNT_COVERAGE_LIMIT: i64 = 2 * MAX_HOST_MOUNTS as i64;

/// The newest readings the coverage answer measures a Host's cadence from.
const COVERAGE_CADENCE_SAMPLES: i64 = 16;

/// The ledger's untouched release boundary: nothing has been released.
const NO_RELEASE_BOUNDARY: &str = "1970-01-01T00:00:00Z";

/// The newest stored reading of one series.
#[derive(Debug, Clone, PartialEq)]
pub struct CoverageReading {
    pub value: f64,
    /// The instant the reading was observed at, which is the ledger's newest
    /// observation: the row is joined at exactly that coordinate.
    pub observed_at: String,
    /// The receipt that carried it.
    pub received_at: String,
}

/// What one series holds, independent of any requested window: the ledger, which
/// outlives the samples, plus the newest reading the raw window still stores.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesCoverage {
    /// The series' enablement boundary: the oldest observation ever recorded.
    pub first_observed_at: String,
    /// The newest observation the Server stores.
    pub last_observed_at: String,
    /// The receipt that carried that newest observation.
    pub last_received_at: String,
    pub observation_count: i64,
    pub replayed_count: i64,
    pub corrected_count: i64,
    /// The instant before which this series' samples were released, `None` while
    /// nothing has been released. It is what explains a series whose newest
    /// observation is no longer stored: the ledger kept the observation, and the
    /// release boundary says the row is gone on purpose rather than never having
    /// existed.
    pub released_before: Option<String>,
    /// The newest stored reading, `None` when the raw window no longer holds one.
    /// A missing reading is reported as unknown, never as a zero.
    pub latest: Option<CoverageReading>,
}

/// One mount path of one Agent and one storage metric.
#[derive(Debug, Clone, PartialEq)]
pub struct HostSeriesCoverage {
    pub mount_path: String,
    pub coverage: SeriesCoverage,
}

/// The mount coverage read: one Agent's storage series, newest mount path first.
///
/// The ledger is the source of the list, because "this mount path was observed"
/// outlives the samples themselves: a mount the Agent stopped reporting is still
/// answered, with its boundary and its counts, after retention released every
/// reading. The raw table is joined on the ledger's own key to add the newest
/// stored reading when it is still there, which is one exact seek per path.
///
/// The order is `(last_observed_at DESC, dimension ASC)` and the limit drops the
/// oldest paths, so what a truncated answer leaves out is the mounts the Agent
/// stopped reporting first, and a tie between two paths reported by the same
/// observation is settled by the path itself rather than by the storage engine.
/// Migration 0069's `host_metric_series_state_mount_idx` is what makes that order
/// and that limit an index walk instead of a sort of every mount path the Agent
/// has ever reported.
const HOST_COVERAGE_SQL: &str = "SELECT l.dimension, l.first_observed_at, l.last_observed_at, l.last_received_at, l.observation_count, l.replayed_count, l.corrected_count, l.released_before, s.value, s.received_at FROM host_metric_series_state l LEFT JOIN host_metric_samples s ON s.agent_id = l.agent_id AND s.metric = l.metric AND s.dimension = l.dimension AND s.observed_at = l.last_observed_at WHERE l.agent_id = ? AND l.metric = ? ORDER BY l.last_observed_at DESC, l.dimension ASC LIMIT ?";

/// One Agent's mount paths for one storage series, newest first.
///
/// The second value is `true` when the Agent holds more mount paths than the
/// limit answers with, so the caller states the truncation instead of presenting
/// a shortened list as the complete set of mounts.
pub async fn load_host_metric_coverage<'e, E>(
    executor: E,
    agent_id: &str,
    metric: &str,
    limit: i64,
) -> Result<(Vec<HostSeriesCoverage>, bool), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let limit = limit.max(1);
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            i64,
            i64,
            i64,
            String,
            Option<f64>,
            Option<String>,
        ),
    >(HOST_COVERAGE_SQL)
    .bind(agent_id)
    .bind(metric)
    .bind(limit + 1)
    .fetch_all(executor)
    .await?;
    let truncated = rows.len() as i64 > limit;
    let covered = rows
        .into_iter()
        .take(limit as usize)
        .map(
            |(
                mount_path,
                first_observed_at,
                last_observed_at,
                last_received_at,
                observation_count,
                replayed_count,
                corrected_count,
                released_before,
                value,
                received_at,
            )| {
                let newest = last_observed_at.clone();
                HostSeriesCoverage {
                    mount_path,
                    coverage: SeriesCoverage {
                        first_observed_at,
                        last_observed_at,
                        last_received_at,
                        observation_count,
                        replayed_count,
                        corrected_count,
                        released_before: (released_before != NO_RELEASE_BOUNDARY)
                            .then_some(released_before),
                        // The join is at the ledger's own coordinate, so a row
                        // that is still stored and a value always arrive
                        // together; a released row arrives as no reading at all.
                        latest: value.zip(received_at).map(|(value, received_at)| {
                            CoverageReading {
                                value,
                                observed_at: newest,
                                received_at,
                            }
                        }),
                    },
                }
            },
        )
        .collect();
    Ok((covered, truncated))
}

/// One Agent's mount evidence, read once.
#[derive(Debug, Clone, PartialEq)]
pub struct MountCoverage {
    /// The bytes-in-use series, newest mount path first.
    pub used: Vec<HostSeriesCoverage>,
    /// The capacity series, newest mount path first.
    pub capacity: Vec<HostSeriesCoverage>,
    /// The Agent's current Host cadence, or 0 when its newest evidence does not
    /// show one.
    pub cadence_seconds: i64,
    /// Whether either storage series holds more mount paths than one read
    /// carries.
    pub truncated: bool,
}

/// The mount evidence one Admin answer is built from, read from one snapshot.
///
/// The three questions an answer asks — the bytes in use, the capacity and the
/// Agent's cadence — have to describe the same ledger, so they are asked through
/// the executor the caller hands in and never from a connection of this
/// function's own: SQLite gives a read transaction a single consistent view,
/// while an Agent that reports a whole new mount contract between two
/// independent reads would otherwise be answered with evidence from two
/// different ledgers, some paths present on one side only and a merged list
/// longer than the limit the answer states. The caller owns the transaction, so
/// the one-snapshot property is visible at the call site and testable there.
pub async fn load_mount_coverage(
    executor: &mut SqliteConnection,
    agent_id: &str,
) -> Result<MountCoverage, sqlx::Error> {
    let mut used = Vec::new();
    let mut capacity = Vec::new();
    let mut truncated = false;
    for (metric, sink) in [
        (HOST_MOUNT_USED_METRIC, &mut used),
        (HOST_MOUNT_CAPACITY_METRIC, &mut capacity),
    ] {
        let (covered, cut) =
            load_host_metric_coverage(&mut *executor, agent_id, metric, MOUNT_COVERAGE_LIMIT)
                .await?;
        *sink = covered;
        truncated |= cut;
    }
    let cadence_seconds = load_observed_cadence(
        &mut *executor,
        &SeriesScope::host(agent_id, HOST_CADENCE_METRIC, ""),
    )
    .await?;
    Ok(MountCoverage {
        used,
        capacity,
        cadence_seconds,
        truncated,
    })
}

/// The verdict one mount path is answered with.
///
/// A silence is a claim about the Agent's rhythm, so it is only made where the
/// Server holds a measured cadence to judge the age against and a reading old
/// enough to be late against it. With no cadence the answer is `unknown`, and a
/// reading stamped after the answer was built is `reported`: a clock that runs
/// a little fast is not a silence, and the age of a reading the Server cannot
/// compare with anything is an age, not a verdict.
pub fn mount_observation_state(
    cadence_seconds: i64,
    silent_seconds: Option<i64>,
    silence_threshold_seconds: i64,
) -> &'static str {
    match (cadence_seconds, silent_seconds) {
        (0, _) | (_, None) => "unknown",
        (_, Some(silent_seconds)) if silent_seconds > silence_threshold_seconds => "silent",
        _ => "reported",
    }
}

/// One mount path with whichever of the two storage series the ledger holds for
/// it.
#[derive(Debug, Clone, PartialEq)]
pub struct MountPathCoverage {
    /// The path exactly as the Agent reported it.
    pub mount_path: String,
    /// The bytes-in-use series, when the ledger holds one for this path.
    pub used: Option<SeriesCoverage>,
    /// The capacity series, when the ledger holds one for this path.
    pub capacity: Option<SeriesCoverage>,
}

impl MountPathCoverage {
    /// The newest instant either storage series of this path was observed at.
    pub fn newest_observation(&self) -> Option<&str> {
        [self.used.as_ref(), self.capacity.as_ref()]
            .into_iter()
            .flatten()
            .map(|coverage| coverage.last_observed_at.as_str())
            .max()
    }
}

/// The mount paths of one coverage read, merged by path, newest first, and cut
/// at the bound the answer states.
///
/// The bound is applied after the merge and not only to each side: the two
/// storage series are read with one limit each, so two sides that disagree on
/// which paths they hold could otherwise answer with twice the limit while still
/// stating one. The second value is `true` when the merge was cut, so the answer
/// states its truncation instead of presenting a shortened list as the whole set.
pub fn merge_mount_coverage(coverage: MountCoverage) -> (Vec<MountPathCoverage>, bool) {
    let mut merged: std::collections::HashMap<String, MountPathCoverage> =
        std::collections::HashMap::new();
    for (is_used, side) in [(true, coverage.used), (false, coverage.capacity)] {
        for HostSeriesCoverage {
            mount_path,
            coverage,
        } in side
        {
            let entry = merged
                .entry(mount_path.clone())
                .or_insert_with(|| MountPathCoverage {
                    mount_path: mount_path.clone(),
                    used: None,
                    capacity: None,
                });
            if is_used {
                entry.used = Some(coverage);
            } else {
                entry.capacity = Some(coverage);
            }
        }
    }
    let mut paths: Vec<MountPathCoverage> = merged.into_values().collect();
    paths.sort_by(|left, right| {
        right
            .newest_observation()
            .cmp(&left.newest_observation())
            .then_with(|| left.mount_path.cmp(&right.mount_path))
    });
    let limit = MOUNT_COVERAGE_LIMIT as usize;
    let truncated = coverage.truncated || paths.len() > limit;
    paths.truncate(limit);
    (paths, truncated)
}

/// The newest stored observations of one series, newest first.
const SERIES_CADENCE_SQL: &str = "SELECT observed_at, received_at, value FROM {raw} WHERE {scope} = ? AND metric = ?{dim_predicate} ORDER BY observed_at DESC LIMIT ?";

/// The cadence one series is being observed at, measured from its newest stored
/// observations, or 0 when its newest evidence does not show one.
///
/// The mount list states whether a mount path is still being reported, and that
/// judgement has to be made against the cadence the Agent really runs at instead
/// of a constant: a Host collecting every five minutes is not silent for being
/// four minutes late. The rhythm is read with [`current_cadence_seconds`], which
/// answers 0 while the newest intervals disagree — an Agent that has just changed
/// its collection interval, or one whose readings are not being stored at all,
/// states no cadence to judge a silence by.
///
/// A return of 0 is "unknown cadence", never "zero cadence": every caller must
/// answer unknown evidence as unknown rather than as silence.
pub async fn load_observed_cadence<'e, E>(
    executor: E,
    scope: &SeriesScope<'_>,
) -> Result<i64, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let sql = scope.sql(SERIES_CADENCE_SQL);
    let rows = scope
        .bind_series(sqlx::query_as::<_, (String, String, f64)>(&sql))
        .bind(COVERAGE_CADENCE_SAMPLES)
        .fetch_all(executor)
        .await?;
    // The statement answers newest first so the limit is spent on the newest
    // evidence; the cadence rule reads a window in the order it happened.
    let mut samples: Vec<MetricSample> = rows
        .into_iter()
        .map(|(observed_at, received_at, value)| MetricSample {
            observed_at,
            received_at,
            value,
        })
        .collect();
    samples.reverse();
    Ok(current_cadence_seconds(&samples))
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

    #[test]
    fn a_window_shorter_than_the_floor_is_judged_by_its_cadence_alone() {
        // The 60-second answer cannot be judged by a two-minute floor: the floor
        // is longer than the whole window, so no silence inside it could ever be
        // reported, and the coverage contract the latest-60-second charts read
        // would be dead code (issue #225). The caller states the floor instead of
        // the rule hardcoding one, and every long-range caller keeps the floor
        // `gap_threshold_seconds` states.
        assert_eq!(gap_threshold_seconds_with_floor(5, 0), 15);
        assert_eq!(gap_threshold_seconds_with_floor(60, 0), 180);
        assert_eq!(gap_threshold_seconds_with_floor(0, 0), 3);
        assert_eq!(gap_threshold_seconds_with_floor(300, 0), 900);
        assert_eq!(gap_threshold_seconds_with_floor(86_400, 0), 900);
        assert_eq!(
            gap_threshold_seconds_with_floor(5, MIN_GAP_SECONDS),
            gap_threshold_seconds(5)
        );
    }

    #[test]
    fn a_silence_inside_a_short_window_is_a_gap_once_the_floor_is_the_cadence() {
        // Five second cadence, a 26 second hole, one minute of window: the
        // two-minute floor the long-range answers use can never name this hole,
        // so the shortest chart the Owner has would draw straight across it.
        let samples = vec![
            sample("2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z", 1.0),
            sample("2026-01-01T00:00:05Z", "2026-01-01T00:00:05Z", 2.0),
            sample("2026-01-01T00:00:10Z", "2026-01-01T00:00:10Z", 3.0),
            sample("2026-01-01T00:00:36Z", "2026-01-01T00:00:36Z", 4.0),
            sample("2026-01-01T00:00:41Z", "2026-01-01T00:00:41Z", 5.0),
            sample("2026-01-01T00:00:46Z", "2026-01-01T00:00:46Z", 6.0),
        ];
        let cadence = current_cadence_seconds(&samples);
        assert_eq!(cadence, 5);
        let from = "2026-01-01T00:00:00Z";
        let to = "2026-01-01T00:01:00Z";
        let long_range = continuity(&samples, &[], cadence, from, to);
        assert!(long_range.gaps.is_empty());
        assert_eq!(long_range.coverage_seconds, 46);
        let short_window = continuity_with_floor(&samples, &[], cadence, from, to, 0);
        assert_eq!(short_window.gaps.len(), 1);
        assert_eq!(short_window.gaps[0].from, "2026-01-01T00:00:10Z");
        assert_eq!(short_window.gaps[0].to, "2026-01-01T00:00:36Z");
        assert_eq!(short_window.gaps[0].seconds, 26);
        assert_eq!(short_window.gaps[0].kind, GapKind::Collection);
        // Only the proven stretches are coverage: the hole is not, and neither
        // is the stretch after the newest observation.
        assert_eq!(short_window.coverage_seconds, 20);
    }

    #[test]
    fn an_unknown_cadence_names_no_silence_and_must_never_be_judged_with() {
        // A series that resumed once after a hole: the newest interval says six
        // seconds and the one before it says 26, so the series has not proved it
        // is being observed at one rhythm yet and states no cadence at all.
        let resumed_once = vec![
            sample("2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z", 1.0),
            sample("2026-01-01T00:00:05Z", "2026-01-01T00:00:05Z", 2.0),
            sample("2026-01-01T00:00:10Z", "2026-01-01T00:00:10Z", 3.0),
            sample("2026-01-01T00:00:36Z", "2026-01-01T00:00:36Z", 4.0),
        ];
        assert_eq!(current_cadence_seconds(&resumed_once), 0);
        // Zero is "unknown cadence", never "zero cadence": fed the rule anyway,
        // a threshold of zero would name every interval of the window a silence.
        let unjudged = continuity_with_floor(
            &resumed_once,
            &[],
            0,
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:01:00Z",
            0,
        );
        assert_eq!(unjudged.gaps.len(), 3);
        // Which is why every caller answers an unknown cadence as unknown: the
        // latest-60-second answer names no silence for such a series and claims
        // no coverage either, and the observation after next settles the rhythm.
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

    // ---- Issue #214: the aggregate tiers behind the raw window ----

    const TIER_NODE: &str = "tier-node";
    const TIER_METRIC: &str = "process_cpu_percent";

    /// The series every tier test writes and reads: one Node's own process
    /// series, the scope boundary the engine is generalized over.
    fn tier_scope() -> SeriesScope<'static> {
        SeriesScope::node(TIER_NODE, TIER_METRIC)
    }

    /// A real temp SQLite database with one private Node, so every tier test
    /// runs the statements the Server runs against the schema the Server ships.
    async fn tier_store() -> (tempfile::TempDir, sqlx::SqlitePool) {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool().clone();
        let stamp = "2026-03-31T12:00:00Z";
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('tier-network', 'Tier', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(stamp)
            .bind(stamp)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('tier-agent', 1, ?, ?)")
            .bind(stamp)
            .bind(stamp)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, 'tier-agent', 'tier-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(TIER_NODE)
            .bind(stamp)
            .bind(stamp)
            .execute(&pool)
            .await
            .unwrap();
        (dir, pool)
    }

    async fn record(pool: &sqlx::SqlitePool, observed_at: &str, received_at: &str, value: f64) {
        let mut tx = pool.begin().await.unwrap();
        record_aggregates(&mut tx, &tier_scope(), observed_at, received_at, value)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn correct(pool: &sqlx::SqlitePool, observed_at: &str, received_at: &str, value: f64) {
        let mut tx = pool.begin().await.unwrap();
        recompute_aggregates(&mut tx, &tier_scope(), observed_at, received_at, value)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn keep_raw(pool: &sqlx::SqlitePool, observed_at: &str, value: f64) {
        sqlx::query("INSERT INTO node_metric_samples (node_id, metric, observed_at, received_at, value) VALUES (?, ?, ?, ?, ?)")
            .bind(TIER_NODE)
            .bind(TIER_METRIC)
            .bind(observed_at)
            .bind(observed_at)
            .bind(value)
            .execute(pool)
            .await
            .unwrap();
    }

    /// sample_count, min, max, last, first_observed_at, last_observed_at
    async fn bucket(
        pool: &sqlx::SqlitePool,
        grain_seconds: i64,
        bucket_start: &str,
    ) -> Option<(i64, f64, f64, f64, String, String)> {
        sqlx::query_as("SELECT sample_count, min_value, max_value, last_value, first_observed_at, last_observed_at FROM node_metric_aggregates WHERE node_id = ? AND metric = ? AND grain_seconds = ? AND bucket_start = ?")
            .bind(TIER_NODE)
            .bind(TIER_METRIC)
            .bind(grain_seconds)
            .bind(bucket_start)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    async fn bucket_count(pool: &sqlx::SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM node_metric_aggregates")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Issue #214: a bucket boundary is a fixed UTC instant the writer, the
    /// reader and the Owner-facing policy can each name, and an instant that is
    /// not a canonical value is refused rather than bucketed somewhere.
    #[test]
    fn a_bucket_starts_on_the_utc_boundary_of_its_grain() {
        assert_eq!(
            aligned_bucket_start("2026-03-04T05:06:07Z", ONE_MINUTE_SECONDS),
            Some("2026-03-04T05:06:00Z".to_owned())
        );
        assert_eq!(
            aligned_bucket_start("2026-03-04T05:06:07Z", FIVE_MINUTE_SECONDS),
            Some("2026-03-04T05:05:00Z".to_owned())
        );
        assert_eq!(
            aligned_bucket_start("2026-03-04T05:03:07Z", FIVE_MINUTE_SECONDS),
            Some("2026-03-04T05:00:00Z".to_owned())
        );
        assert_eq!(
            aligned_bucket_start("2026-03-04T05:06:07.5Z", ONE_MINUTE_SECONDS),
            None
        );
        // A fractional second is not the canonical shape the stored instants
        // use, so it is refused rather than aligned into a bucket whose text
        // coordinate the range comparison would disagree with.
        assert_eq!(
            aligned_bucket_start("not-an-instant", ONE_MINUTE_SECONDS),
            None
        );
        assert_eq!(aligned_bucket_start("2026-03-04T05:06:07Z", 0), None);
        assert_eq!(grain_label(ONE_MINUTE_SECONDS), GRAIN_ONE_MINUTE);
        assert_eq!(grain_label(FIVE_MINUTE_SECONDS), GRAIN_FIVE_MINUTE);
        assert_eq!(grain_label(3600), "unknown");
        assert_eq!(grain_description(ONE_MINUTE_SECONDS), "1-minute");
        assert_eq!(grain_description(FIVE_MINUTE_SECONDS), "5-minute");
    }

    /// Issue #214: one counted observation advances both of its buckets, the
    /// envelope keeps the extremes and the count, and the newest reading follows
    /// the newest observation rather than the newest delivery.
    #[tokio::test]
    async fn a_counted_observation_advances_both_of_its_buckets() {
        let (_dir, pool) = tier_store().await;
        record(&pool, "2026-03-04T05:06:10Z", "2026-03-04T05:06:11Z", 1.0).await;
        record(&pool, "2026-03-04T05:06:50Z", "2026-03-04T05:06:51Z", 3.0).await;
        let minute = bucket(&pool, ONE_MINUTE_SECONDS, "2026-03-04T05:06:00Z")
            .await
            .expect("the minute bucket exists");
        assert_eq!(minute.0, 2);
        assert_eq!(minute.1, 1.0);
        assert_eq!(minute.2, 3.0);
        assert_eq!(minute.3, 3.0);
        assert_eq!(minute.4, "2026-03-04T05:06:10Z");
        assert_eq!(minute.5, "2026-03-04T05:06:50Z");
        let five = bucket(&pool, FIVE_MINUTE_SECONDS, "2026-03-04T05:05:00Z")
            .await
            .expect("the five-minute bucket exists");
        assert_eq!(five.0, 2);
        assert_eq!(five.2, 3.0);

        // A third observation in the next minute of the same five-minute bucket
        // opens a second minute bucket and folds into the coarser one.
        record(&pool, "2026-03-04T05:07:10Z", "2026-03-04T05:07:11Z", 2.0).await;
        assert_eq!(bucket_count(&pool).await, 3);
        let five = bucket(&pool, FIVE_MINUTE_SECONDS, "2026-03-04T05:05:00Z")
            .await
            .unwrap();
        assert_eq!(five.0, 3);
        assert_eq!(five.1, 1.0);
        assert_eq!(five.2, 3.0);

        // An older observation delivered later is a real observation, so it joins
        // the count and can move an extreme, while the newest reading stays the
        // newest observation.
        record(&pool, "2026-03-04T05:06:05Z", "2026-03-04T05:30:00Z", 9.0).await;
        let minute = bucket(&pool, ONE_MINUTE_SECONDS, "2026-03-04T05:06:00Z")
            .await
            .unwrap();
        assert_eq!(minute.0, 3);
        assert_eq!(minute.1, 1.0);
        assert_eq!(minute.2, 9.0);
        assert_eq!(
            minute.3, 3.0,
            "the newest reading is the newest observation"
        );
        assert_eq!(minute.5, "2026-03-04T05:06:50Z");
    }

    /// Issue #214: a correction restates a bucket only from rows that account for
    /// every observation it counted; otherwise the bucket keeps the extremes it
    /// proved and only carries its newest reading forward.
    #[tokio::test]
    async fn a_correction_restates_a_bucket_only_from_rows_that_account_for_it() {
        let (_dir, pool) = tier_store().await;
        record(&pool, "2026-03-04T05:06:10Z", "2026-03-04T05:06:11Z", 1.0).await;
        record(&pool, "2026-03-04T05:06:40Z", "2026-03-04T05:06:41Z", 5.0).await;
        keep_raw(&pool, "2026-03-04T05:06:10Z", 1.0).await;
        keep_raw(&pool, "2026-03-04T05:06:40Z", 2.0).await;
        correct(&pool, "2026-03-04T05:06:40Z", "2026-03-04T05:40:00Z", 2.0).await;
        let minute = bucket(&pool, ONE_MINUTE_SECONDS, "2026-03-04T05:06:00Z")
            .await
            .unwrap();
        assert_eq!(minute.0, 2, "a correction is not a new observation");
        assert_eq!(minute.1, 1.0);
        assert_eq!(minute.2, 2.0, "the corrected extreme is restated");
        assert_eq!(minute.3, 2.0);
        let five = bucket(&pool, FIVE_MINUTE_SECONDS, "2026-03-04T05:05:00Z")
            .await
            .unwrap();
        assert_eq!(five.2, 2.0);

        // One release later and the bucket can no longer be restated: the rows
        // that held the other extreme are gone.
        record(&pool, "2026-03-04T05:11:10Z", "2026-03-04T05:11:11Z", 4.0).await;
        record(&pool, "2026-03-04T05:11:40Z", "2026-03-04T05:11:41Z", 8.0).await;
        keep_raw(&pool, "2026-03-04T05:11:40Z", 0.5).await;
        correct(&pool, "2026-03-04T05:11:40Z", "2026-03-04T05:45:00Z", 0.5).await;
        let minute = bucket(&pool, ONE_MINUTE_SECONDS, "2026-03-04T05:11:00Z")
            .await
            .unwrap();
        assert_eq!(minute.0, 2, "a correction is never a new observation");
        assert_eq!(
            minute.1, 0.5,
            "the corrected reading widens the envelope even when the released rows cannot be restated"
        );
        assert_eq!(minute.2, 8.0, "an extreme the Server cannot restate stands");
        assert_eq!(minute.3, 0.5, "the newest reading is carried forward");
        assert_eq!(minute.5, "2026-03-04T05:11:40Z");

        // The corrected instant is not the bucket's newest, so nothing is
        // carried: the stored newest reading is still the newest observation.
        record(&pool, "2026-03-04T05:16:10Z", "2026-03-04T05:16:11Z", 6.0).await;
        correct(&pool, "2026-03-04T05:16:10Z", "2026-03-04T05:50:00Z", 0.25).await;
        let minute = bucket(&pool, ONE_MINUTE_SECONDS, "2026-03-04T05:16:00Z")
            .await
            .unwrap();
        assert_eq!(minute.3, 6.0);
        assert_eq!(minute.2, 6.0);

        // A bucket the Server never counted is never invented from whatever rows
        // happen to be retained.
        let before = bucket_count(&pool).await;
        keep_raw(&pool, "2026-03-04T05:21:00Z", 7.0).await;
        correct(&pool, "2026-03-04T05:21:00Z", "2026-03-04T05:55:00Z", 7.0).await;
        assert_eq!(bucket_count(&pool).await, before);
        assert!(
            bucket(&pool, ONE_MINUTE_SECONDS, "2026-03-04T05:21:00Z")
                .await
                .is_none()
        );

        // A corrected instant that is not the bucket's newest still widens the
        // envelope: the Server cannot restate the extremes the released rows
        // held, but it does know this reading was counted, and an envelope that
        // omits it is wrong. Only the newest reading is carried forward, so a
        // correction to an older instant leaves "what did the series last say"
        // answering the newest observation.
        record(&pool, "2026-03-04T05:26:10Z", "2026-03-04T05:26:11Z", 3.0).await;
        record(&pool, "2026-03-04T05:26:40Z", "2026-03-04T05:26:41Z", 7.0).await;
        keep_raw(&pool, "2026-03-04T05:26:10Z", 3.0).await;
        correct(&pool, "2026-03-04T05:26:10Z", "2026-03-04T05:59:00Z", 0.25).await;
        let minute = bucket(&pool, ONE_MINUTE_SECONDS, "2026-03-04T05:26:00Z")
            .await
            .unwrap();
        assert_eq!(minute.0, 2);
        assert_eq!(
            minute.1, 0.25,
            "a corrected reading below the envelope widens the minimum"
        );
        assert_eq!(minute.2, 7.0, "an unreachable extreme still stands");
        assert_eq!(
            minute.3, 7.0,
            "an older corrected instant is not the newest reading"
        );
        correct(&pool, "2026-03-04T05:26:10Z", "2026-03-04T06:00:00Z", 12.5).await;
        let minute = bucket(&pool, ONE_MINUTE_SECONDS, "2026-03-04T05:26:00Z")
            .await
            .unwrap();
        assert_eq!(minute.0, 2, "neither correction changed the count");
        assert_eq!(
            minute.1, 0.25,
            "a widened minimum is not narrowed back by a later correction"
        );
        assert_eq!(
            minute.2, 12.5,
            "a corrected reading above the envelope widens the maximum"
        );
        assert_eq!(minute.3, 7.0);
        assert_eq!(minute.5, "2026-03-04T05:26:40Z");
        assert!(
            minute.1 <= minute.3 && minute.3 <= minute.2,
            "the newest reading is inside the envelope"
        );
    }

    // ---- Issue #214: the tiered range reader ----

    const TIER_NOW: &str = "2026-03-31T12:00:00Z";
    const TIER_RAW_CUTOFF: &str = "2026-03-30T12:00:00Z";
    /// The requested stretch of every reader test: the whole investigation
    /// horizon, so each tier's own region is exercised side by side.
    const TIER_FROM: &str = "2026-03-01T00:00:00Z";

    fn at(value: &str) -> OffsetDateTime {
        canonical_instant(value).expect("a canonical test instant")
    }

    async fn read_tiers(
        pool: &sqlx::SqlitePool,
        from: &str,
        to: &str,
        before: Option<&str>,
        limit: i64,
    ) -> MetricRange {
        load_range(
            pool,
            RangeQuery {
                scope: tier_scope(),
                from: at(from),
                to: at(to),
                before: before.map(at),
                limit,
                raw_cutoff: at(TIER_RAW_CUTOFF),
                now: at(TIER_NOW),
            },
        )
        .await
        .unwrap()
    }

    fn grains(range: &MetricRange) -> Vec<&'static str> {
        range.points.iter().map(|point| point.grain).collect()
    }

    async fn read_at(
        pool: &sqlx::SqlitePool,
        from: &str,
        to: &str,
        now: &str,
        raw_cutoff: &str,
        limit: i64,
    ) -> MetricRange {
        load_range(
            pool,
            RangeQuery {
                scope: tier_scope(),
                from: at(from),
                to: at(to),
                before: None,
                limit,
                raw_cutoff: at(raw_cutoff),
                now: at(now),
            },
        )
        .await
        .unwrap()
    }

    fn instants(range: &MetricRange) -> Vec<String> {
        range
            .points
            .iter()
            .map(|point| point.instant.clone())
            .collect()
    }

    /// Issue #214: one request is answered from every tier that still holds the
    /// stretch, oldest tier first, and each point states the grain and the
    /// evidence behind it.
    #[tokio::test]
    async fn a_range_read_serves_each_stretch_at_its_own_grain() {
        let (_dir, pool) = tier_store().await;
        // Inside the raw window: still stored as samples.
        keep_raw(&pool, "2026-03-30T13:00:00Z", 10.0).await;
        keep_raw(&pool, "2026-03-30T13:01:00Z", 12.0).await;
        record(&pool, "2026-03-30T13:00:00Z", "2026-03-30T13:00:05Z", 10.0).await;
        record(&pool, "2026-03-30T13:01:00Z", "2026-03-30T13:01:05Z", 12.0).await;
        // Between the raw window and day 7: answered by the minute tier.
        record(&pool, "2026-03-27T09:00:00Z", "2026-03-27T09:00:05Z", 5.0).await;
        record(&pool, "2026-03-28T10:00:00Z", "2026-03-28T10:00:05Z", 1.0).await;
        record(&pool, "2026-03-28T10:01:00Z", "2026-03-28T10:01:05Z", 2.0).await;
        // Day 7 up to the horizon: only the coarse tier answers.
        record(&pool, "2026-03-10T08:00:00Z", "2026-03-10T08:00:05Z", 3.0).await;
        record(&pool, "2026-03-10T08:05:00Z", "2026-03-10T08:05:05Z", 4.0).await;

        let range = read_tiers(&pool, TIER_FROM, TIER_NOW, None, 5_000).await;
        assert_eq!(
            instants(&range),
            vec![
                "2026-03-10T08:00:00Z",
                "2026-03-10T08:05:00Z",
                "2026-03-27T09:00:00Z",
                "2026-03-28T10:00:00Z",
                "2026-03-28T10:01:00Z",
                "2026-03-30T13:00:00Z",
                "2026-03-30T13:01:00Z",
            ]
        );
        assert_eq!(
            grains(&range),
            vec!["5m", "5m", "1m", "1m", "1m", "raw", "raw"]
        );
        assert_eq!(range.segments.len(), 3);
        assert_eq!(
            range
                .segments
                .iter()
                .map(|segment| (segment.grain, segment.source, segment.point_count))
                .collect::<Vec<_>>(),
            vec![
                ("5m", SOURCE_AGGREGATE, 2),
                ("1m", SOURCE_AGGREGATE, 3),
                ("raw", SOURCE_RAW, 2),
            ]
        );
        assert_eq!(range.segments[0].from, "2026-03-01T12:00:00Z");
        assert_eq!(range.segments[0].to, "2026-03-24T12:00:00Z");
        assert_eq!(range.segments[1].from, "2026-03-24T12:00:00Z");
        assert_eq!(range.segments[1].to, "2026-03-30T12:00:00Z");
        assert_eq!(range.segments[2].from, "2026-03-30T12:00:00Z");
        assert!(!range.truncated);
        assert_eq!(range.continuation, None);

        let raw = &range.points[5];
        assert_eq!(raw.source, SOURCE_RAW);
        assert_eq!(raw.value, 10.0);
        assert_eq!(raw.min_value, 10.0);
        assert_eq!(raw.max_value, 10.0);
        assert_eq!(raw.sample_count, 1);
        assert_eq!(raw.cadence_seconds, 60);
        let minute = &range.points[3];
        assert_eq!(minute.source, SOURCE_AGGREGATE);
        assert_eq!(minute.value, 1.0);
        assert_eq!(minute.last_observed_at, "2026-03-28T10:00:00Z");
        assert_eq!(minute.received_at, "2026-03-28T10:00:05Z");
        assert_eq!(minute.first_observed_at, "2026-03-28T10:00:00Z");
        assert_eq!(minute.cadence_seconds, ONE_MINUTE_SECONDS);
        assert_eq!(range.points[0].cadence_seconds, FIVE_MINUTE_SECONDS);
        // A silence between tiers is still one silence, and the stretches the
        // points really prove are the only ones counted as coverage.
        assert_eq!(range.gaps.len(), 3);
        assert_eq!(range.gaps[0].from, "2026-03-10T08:05:00Z");
        assert_eq!(range.gaps[0].to, "2026-03-27T09:00:00Z");
        assert_eq!(range.coverage_seconds, 300 + 60 + 60);
        assert_eq!(range.ledger, None);
    }

    /// Issue #214 review F2: the thirty-day floor is a moving instant, so the
    /// bucket it falls inside also holds observations that are still inside the
    /// horizon. That bucket is the only evidence left for them once the raw rows
    /// are released, so the tier floor moves down to its boundary and the whole
    /// bucket is answered - while a bucket that ends before the floor stays out
    /// of the answer.
    #[tokio::test]
    async fn a_non_aligned_floor_answers_the_bucket_that_straddles_it() {
        let (_dir, pool) = tier_store().await;
        // Thirteen seconds inside the thirty-day horizon, in the bucket that
        // starts before it.
        record(&pool, "2026-03-01T12:00:50Z", "2026-03-01T12:00:55Z", 7.0).await;
        // Outside the horizon: the bucket that ends before the floor is not
        // pulled in by aligning the floor down.
        record(&pool, "2026-03-01T11:30:00Z", "2026-03-01T11:30:05Z", 9.0).await;

        let range = read_at(
            &pool,
            "2026-03-01T11:00:00Z",
            "2026-03-31T12:00:37Z",
            "2026-03-31T12:00:37Z",
            "2026-03-30T12:00:37Z",
            5_000,
        )
        .await;
        assert_eq!(
            instants(&range),
            vec!["2026-03-01T12:00:00Z"],
            "the observation inside the horizon is answered by its own bucket"
        );
        assert_eq!(grains(&range), vec!["5m"]);
        assert_eq!(range.segments[0].from, "2026-03-01T12:00:00Z");
        assert_eq!(range.points[0].value, 7.0);
        assert_eq!(range.points[0].sample_count, 1);
    }

    /// Issue #214 review F2: every tier handover moves onto a real bucket edge,
    /// so no observation inside a served stretch is dropped for sitting in a
    /// bucket whose start is on the other side of the boundary, and no
    /// observation is answered twice - once as a bucket count and once as a raw
    /// sample. The raw region starts where the minute bucket that straddles the
    /// raw window's floor ends, the minute tier starts where the coarse bucket
    /// that straddles day seven ends.
    #[tokio::test]
    async fn a_non_aligned_handover_answers_every_observation_once() {
        let (_dir, pool) = tier_store().await;
        // Inside the raw window but before the minute edge: the minute bucket
        // that holds it is answered whole instead.
        record(&pool, "2026-03-30T12:03:50Z", "2026-03-30T12:03:55Z", 5.0).await;
        // Plainly inside the raw window.
        keep_raw(&pool, "2026-03-30T13:00:00Z", 6.0).await;
        record(&pool, "2026-03-30T13:00:00Z", "2026-03-30T13:00:05Z", 6.0).await;
        // Inside the seven-day horizon but inside the coarse bucket that
        // straddles the floor: answered by that bucket, together with the
        // observation that is older than the horizon.
        record(&pool, "2026-03-24T12:01:00Z", "2026-03-24T12:01:05Z", 2.0).await;
        record(&pool, "2026-03-24T12:04:00Z", "2026-03-24T12:04:05Z", 3.0).await;
        // Past the coarse bucket's end: the minute tier answers it.
        record(&pool, "2026-03-24T12:06:00Z", "2026-03-24T12:06:05Z", 4.0).await;

        let range = read_at(
            &pool,
            "2026-03-01T11:00:00Z",
            "2026-03-31T11:00:00Z",
            "2026-03-31T12:03:37Z",
            "2026-03-30T12:03:37Z",
            5_000,
        )
        .await;
        assert_eq!(
            instants(&range),
            vec![
                "2026-03-24T12:00:00Z",
                "2026-03-24T12:06:00Z",
                "2026-03-30T12:03:00Z",
                "2026-03-30T13:00:00Z",
            ]
        );
        assert_eq!(grains(&range), vec!["5m", "1m", "1m", "raw"]);
        assert_eq!(
            range.points[0].sample_count, 2,
            "both observations of the straddling coarse bucket are counted once"
        );
        assert_eq!(range.points[2].value, 5.0);
        assert_eq!(range.points[2].sample_count, 1);
        // The handovers are bucket edges, and no region answers an instant that
        // another one already counted.
        assert_eq!(range.segments[0].to, "2026-03-24T12:05:00Z");
        assert_eq!(range.segments[1].from, "2026-03-24T12:05:00Z");
        assert_eq!(range.segments[1].to, "2026-03-30T12:04:00Z");
        assert_eq!(range.segments[2].from, "2026-03-30T12:04:00Z");
        assert!(
            range
                .points
                .iter()
                .all(|point| point.instant != "2026-03-30T12:03:50Z"),
            "the sample the bucket answered is not also returned raw"
        );
    }

    /// Issue #214 review F1: two stored instants prove a span, not continuity.
    /// A bucket whose own largest gap reaches the silence this series allows
    /// contributes no coverage at all and reports the hole it proved, while a
    /// bucket whose observations are dense contributes its span and nothing
    /// more.
    #[tokio::test]
    async fn a_bucket_that_proved_a_hole_claims_no_coverage() {
        let (_dir, pool) = tier_store().await;
        for (observed_at, value) in [
            ("2026-03-10T08:00:00Z", 1.0),
            ("2026-03-10T08:00:01Z", 2.0),
            ("2026-03-10T08:04:58Z", 3.0),
            ("2026-03-10T08:04:59Z", 4.0),
        ] {
            record(&pool, observed_at, observed_at, value).await;
        }
        for offset in 0..4 {
            let observed_at = format!("2026-03-10T09:00:{:02}Z", offset);
            record(&pool, &observed_at, &observed_at, offset as f64).await;
        }

        let range = read_at(&pool, TIER_FROM, TIER_NOW, TIER_NOW, TIER_RAW_CUTOFF, 5_000).await;
        assert_eq!(
            instants(&range),
            vec!["2026-03-10T08:00:00Z", "2026-03-10T09:00:00Z"]
        );
        let sparse = &range.points[0];
        let dense = &range.points[1];
        assert_eq!(sparse.sample_count, 4);
        assert_eq!(sparse.value, 4.0);
        assert_eq!(sparse.first_observed_at, "2026-03-10T08:00:00Z");
        assert_eq!(sparse.last_observed_at, "2026-03-10T08:04:59Z");
        assert_eq!(
            sparse.max_gap_seconds, 297,
            "the bucket records the widest hole it counted inside its own window"
        );
        assert_eq!(
            dense.max_gap_seconds, 1,
            "a dense bucket records the gap its own cadence explains"
        );
        assert_eq!(
            range.coverage_seconds, 3,
            "only the dense bucket proves a stretch that was observed"
        );
        assert_eq!(range.gaps.len(), 1);
        assert_eq!(range.gaps[0].from, "2026-03-10T08:04:59Z");
        assert_eq!(range.gaps[0].to, "2026-03-10T09:00:00Z");
        assert_eq!(range.gaps[0].kind, GapKind::Collection);
    }

    /// Issue #214 review N3: the window allowance exists for the stretch between
    /// two windows the tier really counted, and it must never bridge a window
    /// nobody wrote. Two five-minute buckets with one whole bucket missing
    /// between them are exactly the empty bucket the ticket forbids the Server
    /// to fill in: their own observations are 301 seconds apart, well inside the
    /// window plus cadence allowance, and the stretch between them is still a
    /// silence rather than coverage.
    #[tokio::test]
    async fn a_bucket_nobody_wrote_is_never_bridged_by_the_window_allowance() {
        let (_dir, pool) = tier_store().await;
        // Five observations at the tail of the 08:00 bucket, five at the head of
        // the 08:10 bucket: the 08:05 bucket was never written, so the stretch
        // between them is five minutes nobody observed.
        for offset in 55..60 {
            let observed_at = format!("2026-03-10T08:04:{offset:02}Z");
            record(&pool, &observed_at, &observed_at, offset as f64).await;
        }
        for offset in 0..5 {
            let observed_at = format!("2026-03-10T08:10:{offset:02}Z");
            record(&pool, &observed_at, &observed_at, offset as f64).await;
        }

        let range = read_at(&pool, TIER_FROM, TIER_NOW, TIER_NOW, TIER_RAW_CUTOFF, 5_000).await;
        assert_eq!(
            instants(&range),
            vec!["2026-03-10T08:00:00Z", "2026-03-10T08:10:00Z"]
        );
        assert_eq!(grains(&range), vec!["5m", "5m"]);
        assert!(
            bucket(&pool, FIVE_MINUTE_SECONDS, "2026-03-10T08:05:00Z")
                .await
                .is_none(),
            "the bucket between the two was never written"
        );
        assert_eq!(range.gaps.len(), 1);
        assert_eq!(range.gaps[0].from, "2026-03-10T08:04:59Z");
        assert_eq!(range.gaps[0].to, "2026-03-10T08:10:00Z");
        assert_eq!(
            range.gaps[0].seconds, 301,
            "the gap is the stretch between the two stored instants, not the window between the buckets"
        );
        assert_eq!(range.gaps[0].kind, GapKind::Collection);
        assert_eq!(
            range.coverage_seconds, 8,
            "only what each bucket observed inside its own window is coverage"
        );
    }

    /// Issue #214 review N2: the tiers were introduced empty, so a series counted
    /// before them has raw rows and no buckets at all. Handing the minute the raw
    /// cutoff falls inside to the tier that "counted" it would drop observations
    /// that are still inside the promised raw window - the tier answers nothing
    /// for them and the moved floor excludes them from the raw read. The stored
    /// evidence is served instead, and a request that starts at the cutoff sees
    /// the same samples as one that reaches below it.
    #[tokio::test]
    async fn the_upgrade_seam_keeps_the_raw_samples_the_tiers_never_counted() {
        let (_dir, pool) = tier_store().await;
        // A series from before the tier table existed: rows in the raw window,
        // not one bucket anywhere. The cutoff falls inside the 12:03 minute, and
        // the stored sample sits between the cutoff and the minute's end - the
        // one stretch the unconditional handover moved out of the raw region.
        const CUTOFF: &str = "2026-03-30T12:03:37Z";
        keep_raw(&pool, "2026-03-30T12:03:50Z", 5.0).await;
        keep_raw(&pool, "2026-03-30T13:00:00Z", 6.0).await;

        let wide = read_at(&pool, TIER_FROM, TIER_NOW, TIER_NOW, CUTOFF, 5_000).await;
        assert_eq!(
            instants(&wide),
            vec!["2026-03-30T12:03:50Z", "2026-03-30T13:00:00Z"],
            "a stored sample inside the raw window is served while no tier holds its bucket"
        );
        assert_eq!(grains(&wide), vec!["raw", "raw"]);
        assert_eq!(wide.points[0].value, 5.0);

        let narrow = read_at(&pool, CUTOFF, TIER_NOW, TIER_NOW, CUTOFF, 5_000).await;
        assert_eq!(
            instants(&narrow),
            instants(&wide),
            "a request that reaches below the cutoff sees the same samples as one that starts at it"
        );
    }

    /// Issue #214 review: the newest reading of a bucket moves as one pair. A
    /// bucket that counted fewer observations than its window holds can be
    /// corrected at an instant newer than everything it counted: the corrected
    /// reading is folded into the envelope, and the instant it was observed at
    /// moves with it. Carrying the value alone reports one observation's reading
    /// under another observation's instant, and the Admin contract reads that
    /// pair back as the point's value and its observation delay.
    #[tokio::test]
    async fn a_carried_correction_keeps_the_newest_reading_with_its_instant() {
        let (_dir, pool) = tier_store().await;
        // The bucket counted the older observation; the newer row was stored
        // before the aggregate tiers existed, so no tier ever counted it.
        keep_raw(&pool, "2026-03-04T05:06:40Z", 1.0).await;
        record(&pool, "2026-03-04T05:06:40Z", "2026-03-04T05:06:41Z", 1.0).await;
        keep_raw(&pool, "2026-03-04T05:06:58Z", 99.0).await;

        correct(&pool, "2026-03-04T05:06:58Z", "2026-03-04T05:07:00Z", 100.0).await;
        let minute = bucket(&pool, ONE_MINUTE_SECONDS, "2026-03-04T05:06:00Z")
            .await
            .expect("the minute bucket exists");
        assert_eq!(minute.0, 1, "a correction is not another observation");
        assert_eq!(minute.1, 1.0);
        assert_eq!(
            minute.2, 100.0,
            "the corrected reading is inside the envelope"
        );
        assert_eq!(
            minute.3, 100.0,
            "the corrected reading is the newest reading"
        );
        assert_eq!(
            minute.5, "2026-03-04T05:06:58Z",
            "the newest reading names the instant it was observed at"
        );

        // A correction older than everything the bucket counted moves nothing:
        // the pair still describes the newest observation of the window.
        correct(&pool, "2026-03-04T05:06:40Z", "2026-03-04T05:08:00Z", 0.5).await;
        let minute = bucket(&pool, ONE_MINUTE_SECONDS, "2026-03-04T05:06:00Z")
            .await
            .unwrap();
        assert_eq!(minute.1, 0.5, "a widened minimum is never narrowed back");
        assert_eq!(minute.3, 100.0);
        assert_eq!(minute.5, "2026-03-04T05:06:58Z");

        // The point the range reader serves states the same pair, so the newest
        // reading the Admin contract reports is the one that was observed last.
        let range = read_at(&pool, TIER_FROM, TIER_NOW, TIER_NOW, TIER_RAW_CUTOFF, 5_000).await;
        let point = range
            .points
            .iter()
            .find(|point| point.instant == "2026-03-04T05:05:00Z")
            .expect("the coarse bucket of that window is part of the answer");
        assert_eq!(point.value, 100.0);
        assert_eq!(point.last_observed_at, "2026-03-04T05:06:58Z");
        assert_eq!(point.received_at, "2026-03-04T05:07:00Z");
    }

    /// Issue #214 review: a bucket in the straddling minute is not proof that the
    /// tier counted that minute whole. A series counted before the tiers existed
    /// keeps the raw rows the tier never counted, and one observation delivered
    /// after them creates a bucket whose count covers only itself - so the minute
    /// holds two stored observations and a bucket that counted one. Handing the
    /// minute over on the bucket's mere existence moved the raw floor past both,
    /// leaving the older stored sample, still inside the promised raw window,
    /// answered by nobody. The tier keeps the minute only while its own count
    /// accounts for the rows the minute still stores.
    #[tokio::test]
    async fn a_straddling_minute_the_tier_did_not_count_whole_keeps_its_stored_samples() {
        let (_dir, pool) = tier_store().await;
        const CUTOFF: &str = "2026-03-30T12:03:37Z";
        // The cutoff falls inside the 12:03 minute. The first row was stored
        // before the tiers existed and no tier ever counted it; the second
        // arrived after them and is the one observation its bucket counted.
        keep_raw(&pool, "2026-03-30T12:03:50Z", 99.0).await;
        keep_raw(&pool, "2026-03-30T12:03:55Z", 1.0).await;
        record(&pool, "2026-03-30T12:03:55Z", "2026-03-30T12:03:56Z", 1.0).await;
        let minute = bucket(&pool, ONE_MINUTE_SECONDS, "2026-03-30T12:03:00Z")
            .await
            .expect("the later observation created its minute bucket");
        assert_eq!(minute.0, 1, "the bucket counted one of the two stored rows");

        let wide = read_at(&pool, TIER_FROM, TIER_NOW, TIER_NOW, CUTOFF, 5_000).await;
        assert_eq!(
            instants(&wide),
            vec!["2026-03-30T12:03:50Z", "2026-03-30T12:03:55Z"],
            "both stored observations inside the raw window are served"
        );
        assert_eq!(grains(&wide), vec!["raw", "raw"]);
        assert_eq!(
            wide.points[0].value, 99.0,
            "the spike the tier never counted survives in the raw window"
        );
        assert!(
            wide.points
                .iter()
                .all(|point| point.grain != GRAIN_ONE_MINUTE),
            "a minute the tier did not count whole is not also served as its bucket"
        );
        assert!(
            wide.segments
                .iter()
                .filter(|segment| segment.grain == GRAIN_ONE_MINUTE)
                .all(|segment| segment.point_count == 0),
            "the minute tier answers nothing inside that minute"
        );

        let narrow = read_at(&pool, CUTOFF, TIER_NOW, TIER_NOW, CUTOFF, 5_000).await;
        assert_eq!(
            instants(&narrow),
            instants(&wide),
            "a request that reaches below the cutoff sees the same samples as one that starts at it"
        );
    }

    /// Issue #214 second review (F3): a request whose whole stretch is older than
    /// the investigation horizon is clamped to the horizon, so it reaches the
    /// reader with a floor above its own ceiling. The tier floor is aligned down
    /// to a bucket edge, so the bucket that straddles the horizon was answered
    /// whole - a point holding evidence after the ceiling the caller asked for,
    /// inside an answer that reports the stretch as unavailable. Nothing is read
    /// from a tier when there is nothing to answer, while the series ledger still
    /// answers for the series (the HTTP boundary pins that part).
    #[tokio::test]
    async fn an_inverted_window_answers_nothing_rather_than_the_bucket_that_straddles_it() {
        let (_dir, pool) = tier_store().await;
        // The horizon (now - 30 days) lands inside a bucket, ten seconds after the
        // ceiling the request asks for.
        const NOW: &str = "2026-03-31T12:00:30Z";
        const HORIZON: &str = "2026-03-01T12:00:30Z";
        const TO: &str = "2026-03-01T12:00:20Z";
        // The horizon's own bucket holds an observation forty seconds after the
        // ceiling: real evidence, and none of it inside the requested stretch.
        record(&pool, "2026-03-01T12:00:40Z", "2026-03-01T12:00:41Z", 100.0).await;
        let straddling = bucket(&pool, FIVE_MINUTE_SECONDS, "2026-03-01T12:00:00Z")
            .await
            .expect("the observation created the bucket that straddles the horizon");
        assert_eq!(
            (straddling.0, straddling.2, straddling.5.as_str()),
            (1, 100.0, "2026-03-01T12:00:40Z"),
            "the bucket is exactly the evidence that must not be answered"
        );

        let range = read_at(&pool, HORIZON, TO, NOW, TIER_RAW_CUTOFF, 5_000).await;
        assert_eq!(
            range.points.len(),
            0,
            "an inverted window answers no point: {:?}",
            instants(&range)
        );
        assert!(
            range.segments.is_empty(),
            "no tier answered: {:?}",
            range.segments
        );
        assert!(
            range.gaps.is_empty(),
            "an empty stretch measures no silence"
        );
        assert_eq!(range.coverage_seconds, 0);
        assert!(!range.truncated, "nothing was read, so nothing was cut off");
        assert_eq!(range.continuation, None, "there is no older page to offer");
    }

    /// Issue #214: the raw window is the only place a sample can be answered
    /// from, so a bucket older than the raw window is served as a bucket even
    /// while a raw row for the same instant is still stored. Widening an old
    /// bucket never recovers raw samples.
    #[tokio::test]
    async fn an_old_bucket_is_never_widened_back_into_raw_samples() {
        let (_dir, pool) = tier_store().await;
        keep_raw(&pool, "2026-03-26T09:00:00Z", 9.0).await;
        record(&pool, "2026-03-26T09:00:00Z", "2026-03-26T09:00:05Z", 9.0).await;

        let range = read_tiers(&pool, "2026-03-25T00:00:00Z", TIER_NOW, None, 5_000).await;
        assert_eq!(instants(&range), vec!["2026-03-26T09:00:00Z"]);
        let point = &range.points[0];
        assert_eq!(point.grain, GRAIN_ONE_MINUTE);
        assert_eq!(point.source, SOURCE_AGGREGATE);
        assert_eq!(point.value, 9.0);
        assert_eq!(point.sample_count, 1);
        // The stretch the reader asked of the raw window holds no samples at
        // all, and the row that is stored for that instant is answered by the
        // bucket instead: widening the bucket cannot bring the sample back.
        assert!(
            range
                .segments
                .iter()
                .filter(|segment| segment.grain == GRAIN_RAW)
                .all(|segment| segment.point_count == 0),
            "the raw window answers no point outside its own cutoff"
        );
        assert!(
            range
                .points
                .iter()
                .all(|point| point.grain != GRAIN_RAW && point.source == SOURCE_AGGREGATE)
        );
    }

    /// Issue #214: a budget that fits the newest tiers exactly is still not a
    /// complete answer while the oldest tier holds a point. The probe is what
    /// tells the two apart, so a caller never reads a finished answer whose
    /// oldest evidence was silently dropped.
    #[tokio::test]
    async fn a_budget_fitting_the_newer_tiers_exactly_still_reports_older_evidence_as_truncated() {
        let (_dir, pool) = tier_store().await;
        for offset in 0..3 {
            let observed_at = format!("2026-03-28T10:{:02}:00Z", offset);
            record(&pool, &observed_at, &observed_at, offset as f64 + 1.0).await;
        }
        record(&pool, "2026-03-10T05:00:00Z", "2026-03-10T05:00:00Z", 4.0).await;

        let first = read_tiers(&pool, TIER_FROM, TIER_NOW, None, 3).await;
        assert_eq!(
            instants(&first),
            vec![
                "2026-03-28T10:00:00Z",
                "2026-03-28T10:01:00Z",
                "2026-03-28T10:02:00Z",
            ],
            "the one minute tier is served by the budget"
        );
        assert_eq!(grains(&first), vec!["1m", "1m", "1m"]);
        assert!(
            first.truncated,
            "the five minute tier still holds a point inside the range"
        );
        assert_eq!(first.continuation.as_deref(), Some("2026-03-28T10:00:00Z"));
        assert!(
            first
                .segments
                .iter()
                .all(|segment| segment.grain != GRAIN_FIVE_MINUTE)
        );

        let second = read_tiers(&pool, TIER_FROM, TIER_NOW, first.continuation.as_deref(), 3).await;
        assert_eq!(instants(&second), vec!["2026-03-10T05:00:00Z"]);
        assert_eq!(grains(&second), vec!["5m"]);
        assert!(
            !second.truncated,
            "nothing older is left, so the older page is complete"
        );
        assert_eq!(second.continuation, None);
    }

    /// Issue #214: the budget is one shared limit spent newest tier first, and a
    /// tier that hits it stops the answer instead of leaving a hole between two
    /// tiers. The continuation cursor is the oldest answered coordinate, so the
    /// next page holds strictly older points and never repeats or skips one.
    #[tokio::test]
    async fn a_budget_spent_newest_first_pages_older_without_a_hole() {
        let (_dir, pool) = tier_store().await;
        for index in 0..5 {
            let observed_at = format!("2026-03-30T13:{:02}:00Z", index);
            keep_raw(&pool, &observed_at, index as f64 + 1.0).await;
            record(&pool, &observed_at, &observed_at, index as f64 + 1.0).await;
        }
        for offset in 0..3 {
            let observed_at = format!("2026-03-28T10:{:02}:00Z", offset);
            record(&pool, &observed_at, &observed_at, offset as f64 + 1.0).await;
        }

        let first = read_tiers(&pool, TIER_FROM, TIER_NOW, None, 3).await;
        assert_eq!(
            instants(&first),
            vec![
                "2026-03-30T13:02:00Z",
                "2026-03-30T13:03:00Z",
                "2026-03-30T13:04:00Z",
            ],
            "the newest tier is spent first"
        );
        assert!(first.truncated);
        assert_eq!(first.continuation.as_deref(), Some("2026-03-30T13:02:00Z"));
        assert!(
            first
                .segments
                .iter()
                .all(|segment| segment.grain == GRAIN_RAW)
        );

        let second = read_tiers(&pool, TIER_FROM, TIER_NOW, first.continuation.as_deref(), 3).await;
        assert_eq!(
            instants(&second),
            vec![
                "2026-03-28T10:02:00Z",
                "2026-03-30T13:00:00Z",
                "2026-03-30T13:01:00Z",
            ],
            "the page continues with the older samples and the older tier"
        );
        assert_eq!(grains(&second), vec!["1m", "raw", "raw"]);
        assert!(second.truncated);
        assert_eq!(second.continuation.as_deref(), Some("2026-03-28T10:02:00Z"));

        let third = read_tiers(
            &pool,
            TIER_FROM,
            TIER_NOW,
            second.continuation.as_deref(),
            3,
        )
        .await;
        assert_eq!(
            instants(&third),
            vec!["2026-03-28T10:00:00Z", "2026-03-28T10:01:00Z"]
        );
        assert!(!third.truncated);
        assert_eq!(third.continuation, None);

        let mut seen = first.points.clone();
        seen.extend(second.points.clone());
        seen.extend(third.points.clone());
        let mut coordinates = seen
            .iter()
            .map(|point| point.instant.clone())
            .collect::<Vec<_>>();
        let total = coordinates.len();
        coordinates.sort();
        coordinates.dedup();
        assert_eq!(coordinates.len(), total, "a page never repeats a point");
        assert_eq!(
            coordinates,
            vec![
                "2026-03-28T10:00:00Z",
                "2026-03-28T10:01:00Z",
                "2026-03-28T10:02:00Z",
                "2026-03-30T13:00:00Z",
                "2026-03-30T13:01:00Z",
                "2026-03-30T13:02:00Z",
                "2026-03-30T13:03:00Z",
                "2026-03-30T13:04:00Z",
            ],
            "the three pages together cover the stretch without a hole"
        );
    }

    /// Issue #214: the tiers are judged as one ordered sequence, so a silence
    /// that straddles the raw cutoff is reported once instead of being hidden by
    /// the tier boundary.
    #[tokio::test]
    async fn a_silence_across_a_tier_boundary_is_still_one_gap() {
        let (_dir, pool) = tier_store().await;
        record(&pool, "2026-03-30T11:30:00Z", "2026-03-30T11:30:05Z", 1.0).await;
        keep_raw(&pool, "2026-03-30T13:00:00Z", 2.0).await;
        keep_raw(&pool, "2026-03-30T13:01:00Z", 3.0).await;
        record(&pool, "2026-03-30T13:00:00Z", "2026-03-30T13:00:05Z", 2.0).await;
        record(&pool, "2026-03-30T13:01:00Z", "2026-03-30T13:01:05Z", 3.0).await;

        let range = read_tiers(&pool, TIER_FROM, TIER_NOW, None, 5_000).await;
        assert_eq!(grains(&range), vec!["1m", "raw", "raw"]);
        assert_eq!(range.gaps.len(), 1);
        assert_eq!(range.gaps[0].from, "2026-03-30T11:30:00Z");
        assert_eq!(range.gaps[0].to, "2026-03-30T13:00:00Z");
        assert_eq!(range.gaps[0].seconds, 5_400);
        assert_eq!(range.gaps[0].kind, GapKind::Collection);
        assert_eq!(range.coverage_seconds, 60);
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

    // ---- Issue #215: the same tiers behind one Agent's shared Host series ----

    const HOST_AGENT: &str = "tier-agent";
    const HOST_STORAGE_METRIC: &str = "disk_used_bytes";

    /// One mount path of the Agent's shared storage series. The mount path is the
    /// dimension, so two mounts are two series of one scope and one metric.
    fn host_scope(mount_path: &'static str) -> SeriesScope<'static> {
        SeriesScope::host(HOST_AGENT, HOST_STORAGE_METRIC, mount_path)
    }

    async fn record_host(
        pool: &sqlx::SqlitePool,
        mount_path: &'static str,
        observed_at: &str,
        received_at: &str,
        value: f64,
    ) {
        let mut tx = pool.begin().await.unwrap();
        record_delivery(
            &mut tx,
            &host_scope(mount_path),
            observed_at,
            received_at,
            Delivery::Observed,
        )
        .await
        .unwrap();
        record_aggregates(
            &mut tx,
            &host_scope(mount_path),
            observed_at,
            received_at,
            value,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    async fn correct_host(
        pool: &sqlx::SqlitePool,
        mount_path: &'static str,
        observed_at: &str,
        received_at: &str,
        value: f64,
    ) {
        let mut tx = pool.begin().await.unwrap();
        recompute_aggregates(
            &mut tx,
            &host_scope(mount_path),
            observed_at,
            received_at,
            value,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    async fn keep_raw_host(
        pool: &sqlx::SqlitePool,
        mount_path: &'static str,
        observed_at: &str,
        value: f64,
    ) {
        sqlx::query("INSERT INTO host_metric_samples (agent_id, metric, dimension, observed_at, received_at, value) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(HOST_AGENT)
            .bind(HOST_STORAGE_METRIC)
            .bind(mount_path)
            .bind(observed_at)
            .bind(observed_at)
            .bind(value)
            .execute(pool)
            .await
            .unwrap();
    }

    /// sample_count, min, max, last, first_observed_at, last_observed_at
    async fn host_bucket(
        pool: &sqlx::SqlitePool,
        mount_path: &'static str,
        grain_seconds: i64,
        bucket_start: &str,
    ) -> Option<(i64, f64, f64, f64, String, String)> {
        sqlx::query_as("SELECT sample_count, min_value, max_value, last_value, first_observed_at, last_observed_at FROM host_metric_aggregates WHERE agent_id = ? AND metric = ? AND dimension = ? AND grain_seconds = ? AND bucket_start = ?")
            .bind(HOST_AGENT)
            .bind(HOST_STORAGE_METRIC)
            .bind(mount_path)
            .bind(grain_seconds)
            .bind(bucket_start)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    async fn host_bucket_count(pool: &sqlx::SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM host_metric_aggregates")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn host_ledger_count(pool: &sqlx::SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM host_metric_series_state")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn read_host_tiers(
        pool: &sqlx::SqlitePool,
        mount_path: &'static str,
        from: &str,
        to: &str,
        limit: i64,
    ) -> MetricRange {
        load_range(
            pool,
            RangeQuery {
                scope: host_scope(mount_path),
                from: at(from),
                to: at(to),
                before: None,
                limit,
                raw_cutoff: at(TIER_RAW_CUTOFF),
                now: at(TIER_NOW),
            },
        )
        .await
        .unwrap()
    }

    /// Issue #215: a Host series is identified by its owner, its metric and the
    /// mount path the Agent reported, so two mounts of one Agent are two series
    /// everywhere - in the tiers, in the ledger and in the answer the reader gives.
    #[tokio::test]
    async fn two_mounts_of_one_host_storage_series_keep_their_own_tiers() {
        let (_dir, pool) = tier_store().await;
        assert_eq!(host_bucket_count(&pool).await, 0);
        assert_eq!(host_ledger_count(&pool).await, 0);

        // The same Agent, metric and instant: only the mount path separates them.
        record_host(
            &pool,
            "/data",
            "2026-03-04T05:16:10Z",
            "2026-03-04T05:16:11Z",
            100.0,
        )
        .await;
        record_host(
            &pool,
            "/mnt/data",
            "2026-03-04T05:16:10Z",
            "2026-03-04T05:16:11Z",
            900.0,
        )
        .await;
        assert_eq!(
            host_bucket_count(&pool).await,
            4,
            "each mount writes its own minute and five-minute bucket"
        );
        assert_eq!(
            host_ledger_count(&pool).await,
            2,
            "each mount keeps its own ledger row"
        );
        let data = host_bucket(&pool, "/data", ONE_MINUTE_SECONDS, "2026-03-04T05:16:00Z")
            .await
            .unwrap();
        assert_eq!(data.3, 100.0);
        let logs = host_bucket(
            &pool,
            "/mnt/data",
            ONE_MINUTE_SECONDS,
            "2026-03-04T05:16:00Z",
        )
        .await
        .unwrap();
        assert_eq!(logs.3, 900.0, "one mount's reading is not the other's");

        // A second reading of one mount counts into that mount's bucket only.
        record_host(
            &pool,
            "/data",
            "2026-03-04T05:16:40Z",
            "2026-03-04T05:16:41Z",
            120.0,
        )
        .await;
        let data = host_bucket(&pool, "/data", ONE_MINUTE_SECONDS, "2026-03-04T05:16:00Z")
            .await
            .unwrap();
        assert_eq!(data.0, 2);
        assert_eq!(data.2, 120.0);
        let logs = host_bucket(
            &pool,
            "/mnt/data",
            ONE_MINUTE_SECONDS,
            "2026-03-04T05:16:00Z",
        )
        .await
        .unwrap();
        assert_eq!(logs.0, 1);
        assert_eq!(logs.2, 900.0);
        assert_eq!(host_ledger_count(&pool).await, 2);

        // The reader answers one mount path at a time, and never mixes them.
        let data_range = read_host_tiers(&pool, "/data", TIER_FROM, TIER_NOW, 5_000).await;
        assert_eq!(instants(&data_range), vec!["2026-03-04T05:15:00Z"]);
        assert_eq!(grains(&data_range), vec!["5m"]);
        assert_eq!(data_range.points[0].min_value, 100.0);
        assert_eq!(data_range.points[0].max_value, 120.0);
        assert_eq!(data_range.points[0].sample_count, 2);
        let logs_range = read_host_tiers(&pool, "/mnt/data", TIER_FROM, TIER_NOW, 5_000).await;
        assert_eq!(instants(&logs_range), vec!["2026-03-04T05:15:00Z"]);
        assert_eq!(logs_range.points[0].min_value, 900.0);
        assert_eq!(logs_range.points[0].max_value, 900.0);
        assert_eq!(logs_range.points[0].sample_count, 1);

        // A mount path the Agent never reported is an empty series, not another
        // mount's history.
        let never = read_host_tiers(&pool, "/mnt/never", TIER_FROM, TIER_NOW, 5_000).await;
        assert!(never.points.is_empty());

        // And the Node scope cannot reach the Agent's shared series at all.
        let node_range = read_tiers(&pool, TIER_FROM, TIER_NOW, None, 5_000).await;
        assert!(node_range.points.is_empty());
    }

    /// Issue #215: correcting one mount's reading restates that mount's bucket and
    /// leaves every other mount of the same Agent, metric and instant alone.
    #[tokio::test]
    async fn a_host_correction_restates_only_the_mount_it_names() {
        let (_dir, pool) = tier_store().await;
        record_host(
            &pool,
            "/data",
            "2026-03-04T05:06:10Z",
            "2026-03-04T05:06:11Z",
            1.0,
        )
        .await;
        record_host(
            &pool,
            "/data",
            "2026-03-04T05:06:40Z",
            "2026-03-04T05:06:41Z",
            5.0,
        )
        .await;
        record_host(
            &pool,
            "/mnt/data",
            "2026-03-04T05:06:40Z",
            "2026-03-04T05:06:41Z",
            900.0,
        )
        .await;
        keep_raw_host(&pool, "/data", "2026-03-04T05:06:10Z", 1.0).await;
        keep_raw_host(&pool, "/data", "2026-03-04T05:06:40Z", 2.0).await;
        correct_host(
            &pool,
            "/data",
            "2026-03-04T05:06:40Z",
            "2026-03-04T05:40:00Z",
            2.0,
        )
        .await;

        let data = host_bucket(&pool, "/data", ONE_MINUTE_SECONDS, "2026-03-04T05:06:00Z")
            .await
            .unwrap();
        assert_eq!(data.0, 2, "a correction is not a new observation");
        assert_eq!(data.1, 1.0);
        assert_eq!(data.2, 2.0, "the corrected extreme is restated");
        assert_eq!(data.3, 2.0);
        let logs = host_bucket(
            &pool,
            "/mnt/data",
            ONE_MINUTE_SECONDS,
            "2026-03-04T05:06:00Z",
        )
        .await
        .unwrap();
        assert_eq!(logs.0, 1, "the other mount counted nothing extra");
        assert_eq!(logs.1, 900.0);
        assert_eq!(logs.2, 900.0, "the other mount's extreme stands");
        assert_eq!(logs.3, 900.0);
    }

    // ---- Issue #216: the mount coverage read ----

    const COVERAGE_AGENT: &str = "coverage-agent";

    /// A real temp SQLite database with one Agent: the mount read answers the
    /// evidence one Agent's own Host series hold.
    async fn coverage_store() -> (tempfile::TempDir, sqlx::SqlitePool) {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool().clone();
        let stamp = "2026-04-01T00:00:00Z";
        sqlx::query(
            "INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES (?, 1, ?, ?)",
        )
        .bind(COVERAGE_AGENT)
        .bind(stamp)
        .bind(stamp)
        .execute(&pool)
        .await
        .unwrap();
        (dir, pool)
    }

    /// One stored reading of one series, with the ledger entry that keeps it
    /// reachable after the reading itself is released.
    async fn store_mount_reading(
        pool: &sqlx::SqlitePool,
        metric: &str,
        mount_path: &str,
        observed_at: &str,
        received_at: &str,
        value: f64,
    ) {
        let scope = SeriesScope::host(COVERAGE_AGENT, metric, mount_path);
        let mut tx = pool.begin().await.unwrap();
        record_delivery(
            &mut tx,
            &scope,
            observed_at,
            received_at,
            Delivery::Observed,
        )
        .await
        .unwrap();
        store_sample(&mut tx, &scope, observed_at, received_at, value)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// The newest paths are answered, the oldest paths are the ones a limit
    /// leaves out, and the answer says that it did.
    #[tokio::test]
    async fn the_mount_list_answers_the_newest_path_first_and_states_its_truncation() {
        let (_dir, pool) = coverage_store().await;
        for (mount_path, observed_at, value) in [
            ("/old", "2026-04-01T00:00:00Z", 1.0),
            ("/middle", "2026-04-01T00:05:00Z", 2.0),
            ("/new", "2026-04-01T00:10:00Z", 3.0),
        ] {
            store_mount_reading(
                &pool,
                HOST_MOUNT_USED_METRIC,
                mount_path,
                observed_at,
                observed_at,
                value,
            )
            .await;
        }

        let (covered, truncated) =
            load_host_metric_coverage(&pool, COVERAGE_AGENT, HOST_MOUNT_USED_METRIC, 2)
                .await
                .unwrap();
        assert!(
            truncated,
            "three mount paths with a limit of two must state the truncation"
        );
        assert_eq!(covered.len(), 2);
        assert_eq!(
            covered
                .iter()
                .map(|mount| mount.mount_path.as_str())
                .collect::<Vec<_>>(),
            vec!["/new", "/middle"],
            "the limit must leave out the paths the Agent stopped reporting first"
        );
        let newest = &covered[0].coverage;
        assert_eq!(newest.first_observed_at, "2026-04-01T00:10:00Z");
        assert_eq!(newest.last_observed_at, "2026-04-01T00:10:00Z");
        assert_eq!(newest.observation_count, 1);
        assert_eq!(newest.released_before, None);
        let latest = newest.latest.as_ref().expect("the reading is still stored");
        assert_eq!(latest.value, 3.0);
        assert_eq!(latest.observed_at, "2026-04-01T00:10:00Z");

        let (covered, truncated) =
            load_host_metric_coverage(&pool, COVERAGE_AGENT, HOST_MOUNT_USED_METRIC, 3)
                .await
                .unwrap();
        assert!(!truncated, "the whole list fits the limit");
        assert_eq!(covered.len(), 3);
    }

    /// Two mount paths reported by the one observation are ordered by the path
    /// itself, so a limit that falls between them is decided by the Server and
    /// not by the storage engine.
    #[tokio::test]
    async fn two_paths_observed_at_one_instant_are_ordered_by_the_path() {
        let (_dir, pool) = coverage_store().await;
        for mount_path in ["/b", "/a"] {
            store_mount_reading(
                &pool,
                HOST_MOUNT_USED_METRIC,
                mount_path,
                "2026-04-01T00:00:00Z",
                "2026-04-01T00:00:01Z",
                1.0,
            )
            .await;
        }
        let (covered, truncated) =
            load_host_metric_coverage(&pool, COVERAGE_AGENT, HOST_MOUNT_USED_METRIC, 1)
                .await
                .unwrap();
        assert!(truncated);
        assert_eq!(covered[0].mount_path, "/a");
    }

    /// A mount path whose readings were released is still a mount path the Agent
    /// reported, and its missing newest reading is unknown rather than zero.
    #[tokio::test]
    async fn a_released_newest_reading_is_unknown_with_its_release_boundary() {
        let (_dir, pool) = coverage_store().await;
        store_mount_reading(
            &pool,
            HOST_MOUNT_USED_METRIC,
            "/data",
            "2026-04-01T00:00:00Z",
            "2026-04-01T00:00:01Z",
            42.0,
        )
        .await;
        sqlx::query("DELETE FROM host_metric_samples WHERE agent_id = ?")
            .bind(COVERAGE_AGENT)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE host_metric_series_state SET released_before = ? WHERE agent_id = ?")
            .bind("2026-04-02T00:00:00Z")
            .bind(COVERAGE_AGENT)
            .execute(&pool)
            .await
            .unwrap();

        let (covered, truncated) = load_host_metric_coverage(
            &pool,
            COVERAGE_AGENT,
            HOST_MOUNT_USED_METRIC,
            MOUNT_COVERAGE_LIMIT,
        )
        .await
        .unwrap();
        assert!(!truncated);
        assert_eq!(covered.len(), 1);
        let coverage = &covered[0].coverage;
        assert_eq!(coverage.last_observed_at, "2026-04-01T00:00:00Z");
        assert_eq!(
            coverage.released_before.as_deref(),
            Some("2026-04-02T00:00:00Z"),
            "the release boundary explains the missing reading"
        );
        assert!(
            coverage.latest.is_none(),
            "a released reading is unknown, never a zero"
        );
    }

    /// The cadence the mount list judges silence by is the Agent's own, measured
    /// from its newest stored observations: a Host that slowed down is judged by
    /// what it does now, and one that cannot be measured answers unknown.
    #[tokio::test]
    async fn the_agent_cadence_is_measured_from_its_newest_host_observations() {
        let (_dir, pool) = coverage_store().await;
        let cadence_scope = SeriesScope::host(COVERAGE_AGENT, HOST_CADENCE_METRIC, "");
        assert_eq!(
            load_observed_cadence(&pool, &cadence_scope).await.unwrap(),
            0,
            "nothing stored is an unknown cadence, not a zero cadence"
        );
        store_mount_reading(
            &pool,
            HOST_CADENCE_METRIC,
            "",
            "2026-04-01T00:00:00Z",
            "2026-04-01T00:00:01Z",
            1.0,
        )
        .await;
        assert_eq!(
            load_observed_cadence(&pool, &cadence_scope).await.unwrap(),
            0,
            "one observation proves no cadence"
        );

        let minute = |offset: i64| {
            let base = parse_rfc3339("2026-04-01T00:00:00Z").unwrap();
            format_rfc3339(base + time::Duration::seconds(offset))
        };
        // A Host that collected every minute, then every five: the newest
        // observations are the ones that say what it does now.
        for step in 0..20 {
            let observed_at = minute(step * ONE_MINUTE_SECONDS);
            store_mount_reading(
                &pool,
                HOST_CADENCE_METRIC,
                "",
                &observed_at,
                &observed_at,
                1.0,
            )
            .await;
        }
        for step in 0..COVERAGE_CADENCE_SAMPLES {
            let observed_at = minute(20 * ONE_MINUTE_SECONDS + step * FIVE_MINUTE_SECONDS);
            store_mount_reading(
                &pool,
                HOST_CADENCE_METRIC,
                "",
                &observed_at,
                &observed_at,
                1.0,
            )
            .await;
        }
        assert_eq!(
            load_observed_cadence(&pool, &cadence_scope).await.unwrap(),
            FIVE_MINUTE_SECONDS
        );
    }

    /// A Host whose collection interval was just changed states no cadence yet.
    ///
    /// The rhythm a silence is judged by has to be the one the Agent is keeping
    /// now. A window that still holds the fast pairs from before the change would
    /// answer the old cadence and call every path that keeps reporting at the new
    /// rhythm silent before its next observation was even due.
    #[tokio::test]
    async fn a_collection_interval_that_just_changed_states_no_cadence() {
        let (_dir, pool) = coverage_store().await;
        let scope = SeriesScope::host(COVERAGE_AGENT, HOST_CADENCE_METRIC, "");
        let second = |offset: i64| {
            let base = parse_rfc3339("2026-04-01T00:00:00Z").unwrap();
            format_rfc3339(base + time::Duration::seconds(offset))
        };
        let fast = 5;
        for step in 0..3 {
            let observed_at = second(step * fast);
            store_mount_reading(
                &pool,
                HOST_CADENCE_METRIC,
                "",
                &observed_at,
                &observed_at,
                1.0,
            )
            .await;
        }
        assert_eq!(
            load_observed_cadence(&pool, &scope).await.unwrap(),
            fast,
            "two agreeing intervals are a rhythm"
        );

        // The interval is legally changed from five seconds to five minutes. The
        // window still holds the fast pairs, so the fastest gap in it is five
        // seconds, but the newest interval is the only one that says what the
        // Agent is doing now, and one interval alone is not yet a rhythm.
        let slow = FIVE_MINUTE_SECONDS;
        let changed = second(2 * fast + slow);
        store_mount_reading(&pool, HOST_CADENCE_METRIC, "", &changed, &changed, 1.0).await;
        assert_eq!(
            load_observed_cadence(&pool, &scope).await.unwrap(),
            0,
            "a rhythm that has just changed is not a rhythm, and 0 is unknown"
        );

        // The next observation keeps the slow rhythm, so that is the rhythm.
        let settled = second(2 * fast + 2 * slow);
        store_mount_reading(&pool, HOST_CADENCE_METRIC, "", &settled, &settled, 1.0).await;
        assert_eq!(
            load_observed_cadence(&pool, &scope).await.unwrap(),
            slow,
            "the cadence is judged by what the Agent does now"
        );
    }

    /// The bound an answer states is a bound it keeps.
    ///
    /// The two storage sides are read with one limit each, so sides that disagree
    /// on which paths they hold would merge into more paths than the answer
    /// states. The cut happens after the merge, in the answer's own order.
    #[test]
    fn the_merged_mount_list_is_cut_at_the_bound_it_states() {
        let stored = |mount_path: &str, observed_at: &str| HostSeriesCoverage {
            mount_path: mount_path.to_owned(),
            coverage: SeriesCoverage {
                first_observed_at: observed_at.to_owned(),
                last_observed_at: observed_at.to_owned(),
                last_received_at: observed_at.to_owned(),
                observation_count: 1,
                replayed_count: 0,
                corrected_count: 0,
                released_before: None,
                latest: None,
            },
        };
        let limit = MOUNT_COVERAGE_LIMIT as usize;
        let used: Vec<HostSeriesCoverage> = (0..limit)
            .map(|index| stored(&format!("/use-{index:03}"), "2026-04-01T00:00:00Z"))
            .collect();
        let capacity: Vec<HostSeriesCoverage> = (0..100)
            .map(|index| stored(&format!("/cap-{index:03}"), "2026-04-01T00:00:00Z"))
            .collect();
        let (paths, truncated) = merge_mount_coverage(MountCoverage {
            used,
            capacity,
            cadence_seconds: FIVE_MINUTE_SECONDS,
            truncated: false,
        });
        assert_eq!(
            paths.len(),
            limit,
            "a merged answer carries no more paths than the limit it states"
        );
        assert!(truncated, "a cut merge states its truncation");
        assert_eq!(paths[0].mount_path, "/cap-000");
        assert_eq!(paths[100].mount_path, "/use-000");
        assert_eq!(paths[limit - 1].mount_path, "/use-155");

        // Both sides of one path are one entry, and a merge that fits is not cut.
        let (paths, truncated) = merge_mount_coverage(MountCoverage {
            used: vec![stored("/data", "2026-04-01T00:00:00Z")],
            capacity: vec![stored("/data", "2026-04-01T00:00:00Z")],
            cadence_seconds: FIVE_MINUTE_SECONDS,
            truncated: false,
        });
        assert_eq!(paths.len(), 1, "one path is answered once, not twice");
        assert!(paths[0].used.is_some() && paths[0].capacity.is_some());
        assert!(!truncated);

        // A side that was itself cut keeps the answer truncated.
        let (paths, truncated) = merge_mount_coverage(MountCoverage {
            used: Vec::new(),
            capacity: Vec::new(),
            cadence_seconds: 0,
            truncated: true,
        });
        assert!(paths.is_empty());
        assert!(truncated, "a truncated side is stated by the answer");
    }

    /// One answer is one snapshot of the ledger.
    ///
    /// The two storage sides and the cadence are three questions about one
    /// ledger, so the answer asks them through the one transaction it opened: a
    /// mount contract another writer is in the middle of storing can never arrive
    /// half-applied inside an answer, and no answer mixes two ledgers.
    #[tokio::test]
    async fn one_answer_reads_one_snapshot_of_the_mount_ledger() {
        let (_dir, pool) = coverage_store().await;
        for (metric, value) in [
            (HOST_MOUNT_USED_METRIC, 100.0),
            (HOST_MOUNT_CAPACITY_METRIC, 200.0),
        ] {
            store_mount_reading(
                &pool,
                metric,
                "/data",
                "2026-04-01T00:00:00Z",
                "2026-04-01T00:00:01Z",
                value,
            )
            .await;
        }
        for offset in [0, FIVE_MINUTE_SECONDS] {
            let observed_at = format_rfc3339(
                parse_rfc3339("2026-04-01T00:00:00Z").unwrap() + time::Duration::seconds(offset),
            );
            store_mount_reading(
                &pool,
                HOST_CADENCE_METRIC,
                "",
                &observed_at,
                &observed_at,
                1.0,
            )
            .await;
        }

        // A whole new mount contract is in flight inside one transaction: it is
        // not the ledger yet, and an answer that opened a snapshot of its own
        // would not see a single row of it.
        let mut snapshot = pool.begin().await.unwrap();
        for metric in [HOST_MOUNT_USED_METRIC, HOST_MOUNT_CAPACITY_METRIC] {
            for index in 0..=MOUNT_COVERAGE_LIMIT {
                let mount_path = format!("/bulk-{index:03}");
                let scope = SeriesScope::host(COVERAGE_AGENT, metric, &mount_path);
                record_delivery(
                    &mut snapshot,
                    &scope,
                    "2026-04-01T00:10:00Z",
                    "2026-04-01T00:10:00Z",
                    Delivery::Observed,
                )
                .await
                .unwrap();
            }
        }
        let coverage = load_mount_coverage(&mut snapshot, COVERAGE_AGENT)
            .await
            .unwrap();
        assert_eq!(
            coverage.used.len(),
            MOUNT_COVERAGE_LIMIT as usize,
            "the answer reads the ledger its own transaction holds, batch in flight and all"
        );
        assert_eq!(coverage.capacity.len(), MOUNT_COVERAGE_LIMIT as usize);
        assert!(coverage.truncated, "a cut merge states its truncation");
        assert_eq!(
            coverage.cadence_seconds, FIVE_MINUTE_SECONDS,
            "the cadence is asked of the same snapshot"
        );
        snapshot.rollback().await.unwrap();

        // The batch never landed, so it is not half of a later answer either.
        let mut snapshot = pool.begin().await.unwrap();
        let coverage = load_mount_coverage(&mut snapshot, COVERAGE_AGENT)
            .await
            .unwrap();
        snapshot.commit().await.unwrap();
        assert_eq!(
            coverage.used.len(),
            1,
            "a rolled back batch is not an answer"
        );
        assert_eq!(coverage.capacity.len(), 1);
        assert!(!coverage.truncated);
        assert_eq!(coverage.cadence_seconds, FIVE_MINUTE_SECONDS);
    }

    /// A silence is only claimed where the Server can support it.
    ///
    /// The verdict belongs to the engine rather than to the handler, so the
    /// three ways an age can fail to be a silence are stated once: no measured
    /// cadence, no stored instant, and an instant that has not passed yet.
    #[test]
    fn a_mount_is_only_called_silent_against_a_rhythm_it_has() {
        assert_eq!(mount_observation_state(0, None, 0), "unknown");
        assert_eq!(
            mount_observation_state(0, Some(90_000), 0),
            "unknown",
            "an unmeasurable cadence cannot call a path silent"
        );
        assert_eq!(mount_observation_state(300, None, 900), "unknown");
        assert_eq!(
            mount_observation_state(300, Some(-1), 900),
            "reported",
            "a reading stamped after the answer was built is not a silence"
        );
        assert_eq!(
            mount_observation_state(300, Some(0), 900),
            "reported",
            "a reading from this very instant is current, not silent"
        );
        assert_eq!(
            mount_observation_state(300, Some(900), 900),
            "reported",
            "the threshold is the age a silence starts after, not the age it starts at"
        );
        assert_eq!(mount_observation_state(300, Some(901), 900), "silent");
    }

    /// The mount list runs on every Agent page open, against a ledger that grows
    /// with every mount path the fleet has ever reported, so it must be served by
    /// an index: a scan or a sort here would make the route cost grow with the
    /// fleet instead of with the answer.
    #[tokio::test]
    async fn the_mount_coverage_read_is_index_backed_and_never_sorts_the_ledger() {
        let (_dir, pool) = coverage_store().await;
        let rows: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(&format!("EXPLAIN QUERY PLAN {HOST_COVERAGE_SQL}"))
                .fetch_all(&pool)
                .await
                .unwrap();
        let plan = rows
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            !plan.contains("SCAN host_metric_series_state"),
            "the mount read must not scan the ledger: {plan}"
        );
        assert!(
            plan.contains("host_metric_series_state_mount_idx"),
            "the mount read must be served by the mount index: {plan}"
        );
        assert!(
            !plan.contains("TEMP B-TREE"),
            "the mount read must not sort the paths it bounds: {plan}"
        );
    }
}
