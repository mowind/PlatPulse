//! Retention policies and bounded, safety-protected execution (issue #50,
//! design §11.3, webui.md §8.4).
//!
//! Phase 1 provided a fixed seven-day raw Block Summary cleanup. Phase 2
//! adds Owner-configurable per-family policies with fixed safety bounds,
//! read-only impact previews, and a batched Operation that never lowers the
//! historical high-water mark, never deletes coverage/gap/divergence state
//! or cumulative counters, never touches immutable Incident history, and
//! never removes Audit Events still referenced by Operations.
//!
//! Issue #210 adds the contract a later family plugs into — one investigation
//! floor plus a declarative list of bounded cleanup targets per family — and a
//! persisted, Server-authoritative impact preview: a retention run may only
//! execute a preview the Server still accepts, so a stale policy version, a
//! changed scope, or a moved cutoff can never delete against an unseen plan.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::http::AppState;

/// Phase 1 raw Block Summary retention baseline from design §11.3.
pub const RAW_BLOCK_SUMMARY_RETENTION_DAYS: i64 = 7;
/// Maximum number of raw rows removed by one cleanup invocation.
pub const RAW_BLOCK_SUMMARY_CLEANUP_BATCH: i64 = 128;
/// Phase 3 provides five-minute and hourly Peer aggregate history; the
/// legacy block-history aggregate families remain unsupported.
pub const RAW_BLOCK_HISTORY_AGGREGATES_SUPPORTED: bool = false;
/// Maximum number of rows removed by one bounded retention batch.
pub const RETENTION_BATCH: i64 = 128;
/// Stage-2 investigation floors (design §11.4, parent spec #202): ordinary
/// policy editing may not shorten raw history below 24 hours, nor the
/// aggregate/state history the investigation contract depends on below 30
/// days. Expressed once here and enforced by validate_policy_days.
pub const MIN_INVESTIGATION_RAW_HOURS: i64 = 24;
pub const MIN_INVESTIGATION_RAW_DAYS: i64 = 1;
pub const MIN_INVESTIGATION_AGGREGATE_DAYS: i64 = 30;
/// How long a persisted impact preview stays executable. A run must carry a
/// preview the Server still accepts (issue #210, Story 38).
pub const PREVIEW_TTL_HOURS: i64 = 24;
/// Maximum number of expired previews pruned while composing a new one.
pub const PREVIEW_PRUNE_BATCH: i64 = 128;

pub const FAMILY_RAW_BLOCK_SUMMARY: &str = "raw_block_summary";
/// Raw Node/Agent metric samples (issue #213). This is the family that makes
/// the design's 24-hour raw window a policy fact rather than a side effect of
/// the report cadence: its default is exactly that window, and ordinary policy
/// editing may not shorten it below the Raw class floor.
pub const FAMILY_RAW_METRIC_SAMPLE: &str = "raw_metric_sample";
pub const FAMILY_ONE_MINUTE_AGGREGATE: &str = "one_minute_aggregate";
/// The 5-minute aggregate tier beyond the raw window (issue #214, design
/// §11.6). This family carries the registered 30-day investigation floor for
/// metric history: widening the raw window is not what makes an old stretch
/// answerable, the bucket is.
pub const FAMILY_FIVE_MINUTE_AGGREGATE: &str = "five_minute_aggregate";
pub const FAMILY_ONE_HOUR_AGGREGATE: &str = "one_hour_aggregate";
pub const FAMILY_HISTORY_GAP: &str = "history_gap";
pub const FAMILY_DIVERGENCE_OBSERVATION: &str = "divergence_observation";
pub const FAMILY_AUDIT_EVENT: &str = "audit_event";
pub const FAMILY_ALERT_NOTIFICATION: &str = "alert_notification";
pub const FAMILY_PEER_PRESENCE_INTERVAL: &str = "peer_presence_interval";
pub const FAMILY_PEER_AGGREGATE_5M: &str = "peer_aggregate_5m";
pub const FAMILY_PEER_AGGREGATE_1H: &str = "peer_aggregate_1h";
pub const FAMILY_VALIDATOR_DAILY_SNAPSHOT: &str = "validator_daily_snapshot";
pub const FAMILY_VALIDATOR_MONTHLY_AGGREGATE: &str = "validator_monthly_aggregate";
/// ADR 0009: the Report Receipt body is slimmed after this fixed window. It is
/// a safety invariant — longer than the Agent Durable Spool bound (2 MiB /
/// 24 h) and the coordinated-upgrade backlog — not a capacity policy, and
/// nothing is deleted: only `receipt_body` loses its per-Node/per-sample
/// detail.
pub const FAMILY_REPORT_RECEIPT_BODY: &str = "report_receipt_body";
pub const RECEIPT_BODY_SLIMMING_DAYS: i64 = 30;

/// What kind of history a family holds, and therefore which investigation
/// floor its safety bound has to respect (issue #210, design §11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyClass {
    /// Raw sample history: at least 24 hours must survive ordinary editing.
    Raw,
    /// Aggregate or state history the 30-day investigation contract depends
    /// on: at least 30 days must survive ordinary editing.
    Investigation,
    /// A family whose bound is fixed by its own delivered contract (Peer,
    /// Validator, Audit, Report Receipt bodies, the finer-resolution metric
    /// tiers). This contract neither lowers nor re-derives those bounds.
    Contract,
}

impl PolicyClass {
    /// The lowest bound an ordinary policy edit may set for this class.
    pub fn floor_days(self) -> i64 {
        match self {
            PolicyClass::Raw => MIN_INVESTIGATION_RAW_DAYS,
            PolicyClass::Investigation => MIN_INVESTIGATION_AGGREGATE_DAYS,
            PolicyClass::Contract => 0,
        }
    }

    /// Design citation used when the class floor rejects an edit.
    pub fn floor_reference(self) -> Option<&'static str> {
        match self {
            PolicyClass::Raw => Some("24 hours of raw history, design §11.4"),
            PolicyClass::Investigation => Some("30 days of investigation history, design §11.4"),
            PolicyClass::Contract => None,
        }
    }
}

/// How a family's expired rows are physically released. Both variants are
/// bounded, and neither deletes protected state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupKind {
    /// Bounded DELETE of rows older than the frozen cutoff.
    Delete,
    /// ADR 0009 in-place Report Receipt body slimming: identity survives, only
    /// the per-Node and per-sample detail is cleared.
    SlimReceiptBody,
}

/// One physical storage target inside a family. count_sql is the read-only
/// estimate (its single placeholder binds the frozen cutoff); delete_sql is the
/// fixed, bounded statement that releases at most one batch. Cleanup SQL is
/// never composed from request input, so a new family declares its storage here
/// and neither the preview nor the executor needs a new branch. A family
/// spanning two tables (divergence evidence) declares two targets.
#[derive(Debug, Clone, Copy)]
pub struct CleanupTarget {
    pub table: &'static str,
    pub kind: CleanupKind,
    pub count_sql: &'static str,
    pub delete_sql: &'static str,
}

/// No physical cleanup: nothing bounded to release (unsupported families, and
/// families kept forever).
const NO_CLEANUP_TARGETS: &[CleanupTarget] = &[];

/// Policy defaults and safety bounds (design §11.3). max_days = 0 means no
/// upper bound (long-term family); retention_days = 0 keeps forever. class
/// carries the investigation floor, targets the bounded cleanup storage.
pub struct PolicyDefaults {
    pub family: &'static str,
    pub label: &'static str,
    pub default_days: i64,
    pub min_days: i64,
    pub max_days: i64,
    pub supported: bool,
    pub class: PolicyClass,
    pub targets: &'static [CleanupTarget],
}

impl PolicyDefaults {
    /// Lowest bound an ordinary edit may set for this family: its own declared
    /// minimum, or its class floor when that is stricter.
    pub fn safety_floor_days(&self) -> i64 {
        self.min_days.max(self.class.floor_days())
    }
}

const TARGET_BLOCK_SUMMARIES: &[CleanupTarget] = &[CleanupTarget {
    table: "block_summaries",
    kind: CleanupKind::Delete,
    count_sql: "SELECT COUNT(*) FROM block_summaries WHERE accepted_at < ?",
    delete_sql: "DELETE FROM block_summaries WHERE rowid IN (SELECT rowid FROM block_summaries WHERE accepted_at < ? ORDER BY accepted_at, node_id, block_number LIMIT 128)",
}];

/// Rows one Node-metric cleanup batch may release.
///
/// The per-Report budget has to cover what one accepted Report can add, or the
/// backlog grows even though every Report expires its own oldest rows. An
/// AgentReport carries at most `MAX_NODE_OBSERVATIONS` Nodes
/// (crates/platpulse-core/src/protocol.rs:33) and each of those at most the five
/// node metric series, so the most a single Report can store is 256 x 5 = 1280
/// rows; 2048 leaves headroom for that protocol maximum. The DELETE only ever
/// touches rows that already exist, so a larger bound costs a deployment nothing
/// while the backlog is small — it only decides how far one call drains when a
/// backlog exists.
const NODE_METRIC_CLEANUP_BATCH: i64 = 2048;

/// The bound must cover the rows one Report can store, or the two rates are not
/// equal at steady state. Checked at compile time because a protocol maximum
/// raised without this bound would silently reintroduce the backlog.
const _: () = assert!(
    NODE_METRIC_CLEANUP_BATCH >= platpulse_core::protocol::MAX_NODE_OBSERVATIONS as i64 * 5,
    "the per-Report Node sample batch must cover one maximal Report"
);

/// Raw metric samples live in two tables with identical shape: the per-Node
/// series and the per-Agent host series. Both are expired by the same observed
/// cutoff, so neither grows without bound now that the fixed per-series cap of
/// migration 0041 is gone (issue #213).
///
/// The per-Node bound is deliberately larger than `RETENTION_BATCH`: one Report
/// can carry 256 Nodes, so 128 rows per Report would lag a maximal Agent behind
/// its own expiry rate and the backlog would keep growing until low-space
/// protection paused the very history this family exists to keep. The per-Agent
/// host table keeps the ordinary batch: a Report adds one row per host metric
/// (a handful), far below even that bound.
const TARGET_RAW_METRIC_SAMPLES: &[CleanupTarget] = &[
    CleanupTarget {
        table: "node_metric_samples",
        kind: CleanupKind::Delete,
        count_sql: "SELECT COUNT(*) FROM node_metric_samples WHERE observed_at < ?",
        delete_sql: "DELETE FROM node_metric_samples WHERE rowid IN (SELECT rowid FROM node_metric_samples WHERE observed_at < ? ORDER BY observed_at, node_id, metric LIMIT 2048)",
    },
    CleanupTarget {
        table: "host_metric_samples",
        kind: CleanupKind::Delete,
        count_sql: "SELECT COUNT(*) FROM host_metric_samples WHERE observed_at < ?",
        delete_sql: "DELETE FROM host_metric_samples WHERE rowid IN (SELECT rowid FROM host_metric_samples WHERE observed_at < ? ORDER BY observed_at, agent_id, metric LIMIT 128)",
    },
];

/// Rows one aggregate-tier cleanup batch may release.
///
/// Each tier's delete is filtered to its own grain, so one accepted Report adds
/// at most MAX_NODE_OBSERVATIONS x 5 rows to it (one bucket per Node metric
/// series and tier); 2048 leaves headroom for that protocol maximum, exactly as
/// the raw sample batch does. Without a per-Report bound the tier a Report keeps
/// feeding would grow for as long as the process stays up, because its expired
/// rows only appear once its own window has passed.
const AGGREGATE_CLEANUP_BATCH: i64 = 2048;

/// See NODE_METRIC_CLEANUP_BATCH: a bound below the rows one maximal Report can
/// add to a single grain would let the backlog grow at steady state.
const _: () = assert!(
    AGGREGATE_CLEANUP_BATCH >= platpulse_core::protocol::MAX_NODE_OBSERVATIONS as i64 * 5,
    "the per-Report aggregate batch must cover one maximal Report"
);

/// The 1-minute tier the design §11.6 window declares: a bucket is kept for the
/// 7 days the reader can still be served at that grain, and the delete is a
/// range read of the node_metric_aggregates_expiry_idx index (grain_seconds,
/// bucket_start), never a scan of the whole tier.
const TARGET_ONE_MINUTE_AGGREGATES: &[CleanupTarget] = &[CleanupTarget {
    table: "node_metric_aggregates",
    kind: CleanupKind::Delete,
    count_sql: "SELECT COUNT(*) FROM node_metric_aggregates WHERE grain_seconds = 60 AND bucket_start < ?",
    delete_sql: "DELETE FROM node_metric_aggregates WHERE rowid IN (SELECT rowid FROM node_metric_aggregates WHERE grain_seconds = 60 AND bucket_start < ? ORDER BY bucket_start LIMIT 2048)",
}];

/// The 5-minute tier, kept for the whole 30-day investigation horizon. It is the
/// tier that answers the stretches the 1-minute tier has already released, so it
/// must outlive it by construction.
const TARGET_FIVE_MINUTE_AGGREGATES: &[CleanupTarget] = &[CleanupTarget {
    table: "node_metric_aggregates",
    kind: CleanupKind::Delete,
    count_sql: "SELECT COUNT(*) FROM node_metric_aggregates WHERE grain_seconds = 300 AND bucket_start < ?",
    delete_sql: "DELETE FROM node_metric_aggregates WHERE rowid IN (SELECT rowid FROM node_metric_aggregates WHERE grain_seconds = 300 AND bucket_start < ? ORDER BY bucket_start LIMIT 2048)",
}];

/// Stamp the series the incoming cutoff may release evidence for.
///
/// Deleting a row destroys the Server's ability to answer "did this series
/// already hold an observation at this instant?", and the policy cutoff cannot
/// say afterwards whether it ever did: widening the window moves the cutoff back
/// over instants whose rows the cleanup already released. So the cutoff the
/// cleanup applies is recorded on the series itself, as the oldest cutoff this
/// series has been pruned at, and the stamp only ever moves forward
/// (`released_before < ?` never rewrites a newer stamp).
///
/// The stamp is applied before the deletes rather than from the rows one pass
/// happens to remove: each table's delete is bounded, so a series can still hold
/// rows below the cutoff after its cleanup, and "the cutoff was applied" is the
/// honest boundary — an instant below it may have been released, exactly what
/// `metric_history::classify_delivery` must not count as new again (issue #213).
///
/// One pass over the keys of the rows this cleanup is about to delete: the
/// candidate set is the same range read of the `observed_at` index with the same
/// ordering and the same bound as the delete below, so a series is stamped
/// exactly when the rows that could answer for it are removed, and the ledger is
/// probed by primary key instead of scanned on every accepted Report. A series
/// whose expired rows sit beyond this pass's bound keeps them for now — the
/// window can still answer for those instants, and the pass that finally deletes
/// them is the pass that stamps the series.
const RAW_METRIC_RELEASED_BEFORE_SQL: &str = "WITH expired AS MATERIALIZED (SELECT node_id, metric FROM node_metric_samples WHERE observed_at < ? ORDER BY observed_at, node_id, metric LIMIT 2048) UPDATE node_metric_series_state SET released_before = ? FROM expired WHERE node_metric_series_state.node_id = expired.node_id AND node_metric_series_state.metric = expired.metric AND node_metric_series_state.released_before < ?";

const TARGET_HISTORY_GAPS: &[CleanupTarget] = &[CleanupTarget {
    table: "block_history_gaps",
    kind: CleanupKind::Delete,
    count_sql: "SELECT COUNT(*) FROM block_history_gaps WHERE resolved_at IS NOT NULL AND kind != 'permanent_gap' AND resolved_at < ?",
    delete_sql: "DELETE FROM block_history_gaps WHERE gap_id IN (SELECT gap_id FROM block_history_gaps WHERE resolved_at IS NOT NULL AND kind != 'permanent_gap' AND resolved_at < ? ORDER BY resolved_at LIMIT 128)",
}];

/// Divergence evidence spans two tables: the observation row and the block
/// identity window it was proved against.
const TARGET_DIVERGENCE_EVIDENCE: &[CleanupTarget] = &[
    CleanupTarget {
        table: "chain_divergence_observations",
        kind: CleanupKind::Delete,
        count_sql: "SELECT COUNT(*) FROM chain_divergence_observations WHERE retained_observed_at < ?",
        delete_sql: "DELETE FROM chain_divergence_observations WHERE rowid IN (SELECT rowid FROM chain_divergence_observations WHERE retained_observed_at < ? ORDER BY retained_observed_at LIMIT 128)",
    },
    CleanupTarget {
        table: "block_identity_window",
        kind: CleanupKind::Delete,
        count_sql: "SELECT COUNT(*) FROM block_identity_window WHERE retained_until < ?",
        delete_sql: "DELETE FROM block_identity_window WHERE rowid IN (SELECT rowid FROM block_identity_window WHERE retained_until < ? ORDER BY retained_until LIMIT 128)",
    },
];

const TARGET_AUDIT_EVENTS: &[CleanupTarget] = &[CleanupTarget {
    table: "audit_events",
    kind: CleanupKind::Delete,
    count_sql: "SELECT COUNT(*) FROM audit_events WHERE created_at < ? AND audit_event_id NOT IN (SELECT audit_event_id FROM operations WHERE audit_event_id IS NOT NULL)",
    delete_sql: "DELETE FROM audit_events WHERE audit_event_id IN (SELECT audit_event_id FROM audit_events WHERE created_at < ? AND audit_event_id NOT IN (SELECT audit_event_id FROM operations WHERE audit_event_id IS NOT NULL) ORDER BY audit_event_id LIMIT 128)",
}];

const TARGET_NOTIFICATION_EVENTS: &[CleanupTarget] = &[CleanupTarget {
    table: "notification_events",
    kind: CleanupKind::Delete,
    count_sql: "SELECT COUNT(*) FROM notification_events WHERE created_at < ?",
    delete_sql: "DELETE FROM notification_events WHERE event_id IN (SELECT event_id FROM notification_events WHERE created_at < ? ORDER BY created_at LIMIT 128)",
}];

const TARGET_PEER_PRESENCE_INTERVALS: &[CleanupTarget] = &[CleanupTarget {
    table: "peer_presence_intervals",
    kind: CleanupKind::Delete,
    count_sql: "SELECT COUNT(*) FROM peer_presence_intervals WHERE closed_at IS NOT NULL AND closed_at < ?",
    delete_sql: "DELETE FROM peer_presence_intervals WHERE interval_id IN (SELECT interval_id FROM peer_presence_intervals WHERE closed_at IS NOT NULL AND closed_at < ? ORDER BY closed_at, interval_id LIMIT 128)",
}];

const TARGET_PEER_AGGREGATE_5M: &[CleanupTarget] = &[CleanupTarget {
    table: "peer_aggregate_5m",
    kind: CleanupKind::Delete,
    count_sql: "SELECT COUNT(*) FROM peer_aggregate_5m WHERE bucket_start < ?",
    delete_sql: "DELETE FROM peer_aggregate_5m WHERE aggregate_id IN (SELECT aggregate_id FROM peer_aggregate_5m WHERE bucket_start < ? ORDER BY bucket_start, aggregate_id LIMIT 128)",
}];

const TARGET_PEER_AGGREGATE_1H: &[CleanupTarget] = &[CleanupTarget {
    table: "peer_aggregate_1h",
    kind: CleanupKind::Delete,
    count_sql: "SELECT COUNT(*) FROM peer_aggregate_1h WHERE bucket_start < ?",
    delete_sql: "DELETE FROM peer_aggregate_1h WHERE aggregate_id IN (SELECT aggregate_id FROM peer_aggregate_1h WHERE bucket_start < ? ORDER BY bucket_start, aggregate_id LIMIT 128)",
}];

const TARGET_RECEIPT_BODIES: &[CleanupTarget] = &[CleanupTarget {
    table: "agent_report_receipts",
    kind: CleanupKind::SlimReceiptBody,
    count_sql: "SELECT COUNT(*) FROM agent_report_receipts WHERE received_at < ? AND receipt_slimmed_at IS NULL AND report_id NOT IN (SELECT close_report_id FROM agents WHERE close_report_id IS NOT NULL)",
    delete_sql: "",
}];

pub const POLICY_CATALOG: [PolicyDefaults; 15] = [
    PolicyDefaults {
        family: FAMILY_RAW_BLOCK_SUMMARY,
        label: "Raw Block Summaries",
        default_days: 7,
        min_days: 1,
        max_days: 30,
        supported: true,
        class: PolicyClass::Raw,
        targets: TARGET_BLOCK_SUMMARIES,
    },
    PolicyDefaults {
        family: FAMILY_RAW_METRIC_SAMPLE,
        label: "Raw Metric Samples",
        // The design's raw window: 24 hours of samples the Server actually
        // received. This is the floor the Raw class also enforces, so the
        // window cannot be edited away (§11.4, issue #213).
        default_days: 1,
        min_days: 1,
        max_days: 30,
        supported: true,
        class: PolicyClass::Raw,
        targets: TARGET_RAW_METRIC_SAMPLES,
    },
    PolicyDefaults {
        family: FAMILY_ONE_MINUTE_AGGREGATE,
        label: "1-Minute Aggregates",
        // The finer-resolution tier serves the stretch between the raw window
        // and 7 days (design §11.6). Its window is its contract: a bucket kept
        // beyond what the reader can be served at this grain is storage nobody
        // asks for, and a shorter one would leave a hole between the raw window
        // and the 5-minute tier. So the bound is fixed at 7 days.
        default_days: 7,
        min_days: 7,
        max_days: 7,
        supported: true,
        class: PolicyClass::Contract,
        targets: TARGET_ONE_MINUTE_AGGREGATES,
    },
    PolicyDefaults {
        family: FAMILY_FIVE_MINUTE_AGGREGATE,
        label: "5-Minute Aggregates",
        // The coarse tier carries the investigation horizon: 30 days is both the
        // window the reader may ask for and the registered floor of this family
        // (design §11.4, §11.6).
        default_days: MIN_INVESTIGATION_AGGREGATE_DAYS,
        min_days: MIN_INVESTIGATION_AGGREGATE_DAYS,
        max_days: MIN_INVESTIGATION_AGGREGATE_DAYS,
        supported: true,
        class: PolicyClass::Investigation,
        targets: TARGET_FIVE_MINUTE_AGGREGATES,
    },
    PolicyDefaults {
        family: FAMILY_ONE_HOUR_AGGREGATE,
        label: "1-Hour Aggregates",
        default_days: 0,
        min_days: 0,
        max_days: 0,
        supported: false,
        class: PolicyClass::Contract,
        targets: NO_CLEANUP_TARGETS,
    },
    PolicyDefaults {
        family: FAMILY_HISTORY_GAP,
        label: "History Gap Records",
        default_days: 180,
        min_days: 180,
        max_days: 0,
        supported: true,
        class: PolicyClass::Investigation,
        targets: TARGET_HISTORY_GAPS,
    },
    PolicyDefaults {
        family: FAMILY_DIVERGENCE_OBSERVATION,
        label: "Divergence Evidence",
        default_days: 180,
        min_days: 180,
        max_days: 0,
        supported: true,
        class: PolicyClass::Investigation,
        targets: TARGET_DIVERGENCE_EVIDENCE,
    },
    PolicyDefaults {
        family: FAMILY_AUDIT_EVENT,
        label: "Audit Events",
        default_days: 365,
        min_days: 365,
        max_days: 0,
        supported: true,
        class: PolicyClass::Contract,
        targets: TARGET_AUDIT_EVENTS,
    },
    PolicyDefaults {
        family: FAMILY_ALERT_NOTIFICATION,
        label: "Alert Notification Events",
        default_days: 180,
        min_days: 90,
        max_days: 0,
        supported: true,
        class: PolicyClass::Contract,
        targets: TARGET_NOTIFICATION_EVENTS,
    },
    PolicyDefaults {
        family: FAMILY_PEER_PRESENCE_INTERVAL,
        label: "Peer Presence Intervals",
        default_days: 30,
        min_days: 1,
        max_days: 365,
        supported: true,
        class: PolicyClass::Contract,
        targets: TARGET_PEER_PRESENCE_INTERVALS,
    },
    PolicyDefaults {
        family: FAMILY_PEER_AGGREGATE_5M,
        label: "Peer 5-Minute Aggregates",
        default_days: 90,
        min_days: 7,
        max_days: 365,
        supported: true,
        class: PolicyClass::Contract,
        targets: TARGET_PEER_AGGREGATE_5M,
    },
    PolicyDefaults {
        family: FAMILY_PEER_AGGREGATE_1H,
        label: "Peer 1-Hour Aggregates",
        default_days: 0,
        min_days: 0,
        max_days: 0,
        supported: true,
        class: PolicyClass::Contract,
        targets: TARGET_PEER_AGGREGATE_1H,
    },
    PolicyDefaults {
        family: FAMILY_VALIDATOR_DAILY_SNAPSHOT,
        label: "Validator Daily Snapshots",
        // Daily snapshots are the durable source for calendar-month rebuilds.
        // They are kept forever so delayed retries and restarts can never
        // re-insert an old day into a partially retained month.
        default_days: 0,
        min_days: 0,
        max_days: 0,
        supported: true,
        class: PolicyClass::Contract,
        targets: NO_CLEANUP_TARGETS,
    },
    PolicyDefaults {
        family: FAMILY_VALIDATOR_MONTHLY_AGGREGATE,
        label: "Validator Monthly Aggregates",
        // Monthly aggregates are derived, long-term reporting state and must
        // never be removed by a retention run.
        default_days: 0,
        min_days: 0,
        max_days: 0,
        supported: true,
        class: PolicyClass::Contract,
        targets: NO_CLEANUP_TARGETS,
    },
    PolicyDefaults {
        family: FAMILY_REPORT_RECEIPT_BODY,
        label: "Report Receipt Bodies",
        // ADR 0009: fixed window. min == max so the Admin can never shorten it
        // into the legitimate retry window; no row is deleted, only the body.
        default_days: RECEIPT_BODY_SLIMMING_DAYS,
        min_days: RECEIPT_BODY_SLIMMING_DAYS,
        max_days: RECEIPT_BODY_SLIMMING_DAYS,
        supported: true,
        class: PolicyClass::Contract,
        targets: TARGET_RECEIPT_BODIES,
    },
];

pub fn catalog_family(family: &str) -> Option<&'static PolicyDefaults> {
    POLICY_CATALOG.iter().find(|entry| entry.family == family)
}

/// Declared bounded cleanup storage of a family: empty for unsupported families
/// and for families kept forever.
pub fn catalog_targets(family: &str) -> &'static [CleanupTarget] {
    catalog_family(family).map_or(NO_CLEANUP_TARGETS, |entry| entry.targets)
}

/// One row's slimmed Report Receipt body (ADR 0009): the protocol-shaped
/// receipt with its per-Node and per-sample/range detail cleared. Returns
/// `None` when the stored body is not a JSON object; the caller still marks
/// that row, so a bounded run always makes progress.
fn slim_receipt_body(body: &[u8]) -> Option<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object_mut()?;
    object.insert("nodes".to_owned(), Value::Array(Vec::new()));
    object.insert("samples".to_owned(), Value::Array(Vec::new()));
    serde_json::to_vec(&value).ok()
}

/// One bounded slimming batch. Rows past the frozen cutoff that still hold a
/// full body are rewritten in place; report identity, content hash,
/// disposition and the rejection/Inventory evidence columns are untouched.
///
/// A row an Agent currently references as its Closing Receipt is excluded: the
/// coordinated checkpoint and the conversion verification compare that body
/// byte-for-byte, so it must stay verbatim (ADR 0009).
async fn slim_receipt_body_batch(
    conn: &mut sqlx::SqliteConnection,
    cutoff: &str,
    now: &str,
) -> Result<u64, sqlx::Error> {
    let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT rowid, receipt_body FROM agent_report_receipts \
         WHERE received_at < ? AND receipt_slimmed_at IS NULL \
           AND report_id NOT IN (SELECT close_report_id FROM agents WHERE close_report_id IS NOT NULL) \
         ORDER BY received_at LIMIT ?",
    )
    .bind(cutoff)
    .bind(RETENTION_BATCH)
    .fetch_all(&mut *conn)
    .await?;
    let mut slimmed = 0u64;
    for (rowid, body) in rows {
        match slim_receipt_body(&body) {
            Some(slimmed_body) => {
                sqlx::query(
                    "UPDATE agent_report_receipts SET receipt_body = ?, receipt_slimmed_at = ? WHERE rowid = ?",
                )
                .bind(slimmed_body)
                .bind(now)
                .bind(rowid)
                .execute(&mut *conn)
                .await?;
            }
            None => {
                sqlx::query(
                    "UPDATE agent_report_receipts SET receipt_slimmed_at = ? WHERE rowid = ?",
                )
                .bind(now)
                .bind(rowid)
                .execute(&mut *conn)
                .await?;
            }
        }
        slimmed += 1;
    }
    Ok(slimmed)
}

#[derive(Debug, Clone)]
pub struct PolicyRow {
    pub family: String,
    pub retention_days: i64,
    pub min_days: i64,
    pub max_days: i64,
    pub supported: bool,
    pub enabled: bool,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

/// RFC3339 cutoff before which rows of a family may be removed.
pub fn family_cutoff(now: time::OffsetDateTime, retention_days: i64) -> time::OffsetDateTime {
    now - time::Duration::days(retention_days)
}

/// The width of one bucket of an aggregate tier, in seconds. Only the two tiers
/// node_metric_aggregates holds have one.
fn aggregate_grain_seconds(family: &str) -> Option<i64> {
    match family {
        FAMILY_ONE_MINUTE_AGGREGATE => Some(crate::metric_history::ONE_MINUTE_SECONDS),
        FAMILY_FIVE_MINUTE_AGGREGATE => Some(crate::metric_history::FIVE_MINUTE_SECONDS),
        _ => None,
    }
}

/// The instant a family's cleanup actually releases evidence at (review F2).
///
/// For every family but the aggregate tiers this is the family cutoff itself.
/// An aggregate tier releases its rows one bucket width behind that cutoff,
/// because a row of this table IS a bucket: the bucket the cutoff falls inside
/// also holds observations that are still inside the window, and once the row is
/// gone the reader has no evidence left for those instants at all - the reader
/// aligns its own region floor down to that bucket's boundary for exactly that
/// reason (metric_history::load_range). Deleting only a bucket that ends before
/// the cutoff is the same statement as deleting only a bucket whose own
/// observations are all expired: every observation counted into a bucket is
/// stamped before its bucket ends, so a bucket with bucket_start + width <=
/// cutoff holds nothing inside the window, and no bucket whose observations
/// reach the window can satisfy it. The cost is one bucket of extra storage per
/// tier at the boundary, and the guarantee is that a stretch the reader may
/// still ask for is never released early.
pub fn release_cutoff(
    family: &str,
    retention_days: i64,
    now: time::OffsetDateTime,
) -> time::OffsetDateTime {
    let cutoff = family_cutoff(now, retention_days);
    match aggregate_grain_seconds(family) {
        Some(grain) => cutoff - time::Duration::seconds(grain),
        None => cutoff,
    }
}

/// RFC3339 cutoff before which raw summaries may be removed.
pub fn raw_block_summary_cutoff(now: time::OffsetDateTime) -> time::OffsetDateTime {
    family_cutoff(now, RAW_BLOCK_SUMMARY_RETENTION_DAYS)
}

/// Read the current global Block History window. The constant remains the
/// bootstrap/default fallback for pre-policy fixtures; live Server cleanup
/// always uses the persisted Owner setting.
pub async fn raw_block_summary_retention_days(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT retention_days FROM retention_policies WHERE family = ?",
    )
    .bind(FAMILY_RAW_BLOCK_SUMMARY)
    .fetch_optional(pool)
    .await?
    .unwrap_or(RAW_BLOCK_SUMMARY_RETENTION_DAYS))
}

/// The bounded expired-row delete. The 'block_summaries_accepted_at_idx'
/// index covers '(accepted_at, node_id, block_number)', so the ordered 'LIMIT'
/// is a range read of at most 'RAW_BLOCK_SUMMARY_CLEANUP_BATCH' index entries.
///
/// This statement runs after every accepted AgentReport, so it must never fall
/// back to a full table scan: SQLite can still satisfy the delete, but the
/// scan plus temporary B-tree made report ingestion cost ~124 ms per report on
/// a 260 MB table even when nothing was expired (issue #137).
pub const RAW_BLOCK_SUMMARY_CLEANUP_SQL: &str = "DELETE FROM block_summaries WHERE rowid IN (SELECT rowid FROM block_summaries WHERE accepted_at < ? ORDER BY accepted_at, node_id, block_number LIMIT ?)";

/// Delete at most one bounded batch of expired raw summaries.
///
/// The query is intentionally one short SQLite statement: repeated startup or
/// ingestion calls are idempotent, and a large historical table cannot make a
/// single cleanup transaction unbounded. The tables that preserve dedup and
/// recovery state are not joined or deleted here.
pub async fn cleanup_raw_block_summaries(
    pool: &SqlitePool,
    now: time::OffsetDateTime,
) -> Result<u64, sqlx::Error> {
    let retention_days = raw_block_summary_retention_days(pool).await?;
    let cutoff = crate::auth::format_rfc3339(family_cutoff(now, retention_days));
    let result = sqlx::query(RAW_BLOCK_SUMMARY_CLEANUP_SQL)
        .bind(cutoff)
        .bind(RAW_BLOCK_SUMMARY_CLEANUP_BATCH)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// Read the raw metric-sample window. The catalog default is the 24-hour
/// window of design §11.4; a persisted Owner setting can only widen it, because
/// the catalog declares the Raw class floor as this family's minimum.
pub async fn metric_sample_retention_days(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let fallback = catalog_family(FAMILY_RAW_METRIC_SAMPLE)
        .map(|policy| policy.default_days)
        .unwrap_or(MIN_INVESTIGATION_RAW_DAYS);
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT retention_days FROM retention_policies WHERE family = ?",
    )
    .bind(FAMILY_RAW_METRIC_SAMPLE)
    .fetch_optional(pool)
    .await?
    .unwrap_or(fallback))
}

/// The same window read through a transaction that is already open.
///
/// Report ingestion holds the Server's single write connection, so a policy read
/// through the pool would wait for a connection that only the caller can
/// release. Reading inside the ingestion transaction is also the stronger
/// guarantee: no policy edit can land between the read and the samples it
/// classifies, because an edit needs that same connection.
pub async fn metric_sample_retention_days_tx(
    tx: &mut Transaction<'_, Sqlite>,
) -> Result<i64, sqlx::Error> {
    let fallback = catalog_family(FAMILY_RAW_METRIC_SAMPLE)
        .map(|policy| policy.default_days)
        .unwrap_or(MIN_INVESTIGATION_RAW_DAYS);
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT retention_days FROM retention_policies WHERE family = ?",
    )
    .bind(FAMILY_RAW_METRIC_SAMPLE)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(fallback))
}

/// Delete at most one bounded batch of expired raw metric samples per table.
///
/// This runs after every accepted AgentReport, next to the raw Block Summary
/// cleanup, so each statement is a range read of the `observed_at` index rather
/// than a scan of a large history (issue #137). Each table's bound is set from
/// the rows one Report can add to it (see `TARGET_RAW_METRIC_SAMPLES`), which is
/// what keeps the two rates equal at steady state: a per-Report bound below the
/// arrival rate would let the backlog grow even though every Report expires its
/// own share. The series ledger in `node_metric_series_state` is deliberately
/// not a target: what a series observed stays knowable after the samples
/// themselves expire, which is how the Admin surface tells "expired" apart from
/// "never observed".
pub async fn cleanup_expired_metric_samples(
    pool: &SqlitePool,
    now: time::OffsetDateTime,
) -> Result<u64, sqlx::Error> {
    let retention_days = metric_sample_retention_days(pool).await?;
    if retention_days <= 0 {
        // A keep-forever window has no cutoff, so it can never expire a row.
        return Ok(0);
    }
    let cutoff = crate::auth::format_rfc3339(family_cutoff(now, retention_days));
    sqlx::query(RAW_METRIC_RELEASED_BEFORE_SQL)
        .bind(&cutoff)
        .bind(&cutoff)
        .bind(&cutoff)
        .execute(pool)
        .await?;
    let mut removed = 0;
    for target in catalog_targets(FAMILY_RAW_METRIC_SAMPLE) {
        removed += sqlx::query(target.delete_sql)
            .bind(&cutoff)
            .execute(pool)
            .await?
            .rows_affected();
    }
    Ok(removed)
}

/// Read the persisted window of one aggregate tier.
///
/// The catalog default is the fallback for a database that has not been seeded
/// yet, exactly as the raw families do it: a tier whose window is fixed cannot
/// have been widened by an edit, but it must still be read rather than assumed,
/// because the policy table is the single place an Operator can see.
async fn aggregate_retention_days(pool: &SqlitePool, family: &str) -> Result<i64, sqlx::Error> {
    let Some(catalog) = catalog_family(family) else {
        return Ok(0);
    };
    if aggregate_grain_seconds(family).is_some() {
        // The stored rows of this family are buckets, and the stretch a bucket
        // can still be served for IS the tier's window: it has exactly one legal
        // value, so the catalog is the window (review F4). A row that says
        // otherwise - a pre-#214 placeholder, or a number an operator set while
        // the surface still offered a range - must not make the automatic
        // cleanup retain buckets the reader can never be served. ensure_seeded
        // restores the catalog tuple, and this clamp keeps the window right even
        // on a database whose seeding has not run yet.
        return Ok(catalog.default_days);
    }
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT retention_days FROM retention_policies WHERE family = ?",
    )
    .bind(family)
    .fetch_optional(pool)
    .await?
    .unwrap_or(catalog.default_days))
}

/// Delete at most one bounded batch of expired aggregate buckets per tier.
///
/// Each tier is expired by its own window, so the 1-minute buckets leave first
/// while the 5-minute buckets covering the same stretch stay: that is what makes
/// an old bucket unrecoverable as raw samples and still answerable at a coarser
/// grain (design §11.6, issue #214). Nothing here touches the raw samples, the
/// series ledger, or the aggregate rows the reader can still be served.
pub async fn cleanup_expired_metric_aggregates(
    pool: &SqlitePool,
    now: time::OffsetDateTime,
) -> Result<u64, sqlx::Error> {
    let mut removed = 0;
    for family in [FAMILY_ONE_MINUTE_AGGREGATE, FAMILY_FIVE_MINUTE_AGGREGATE] {
        let retention_days = aggregate_retention_days(pool, family).await?;
        if retention_days <= 0 {
            // A keep-forever window has no cutoff, so it can never expire a row.
            continue;
        }
        // One bucket width behind the policy cutoff: a bucket is released only
        // once every observation it counted is outside the window (review F2).
        let cutoff = crate::auth::format_rfc3339(release_cutoff(family, retention_days, now));
        for target in catalog_targets(family) {
            removed += sqlx::query(target.delete_sql)
                .bind(&cutoff)
                .execute(pool)
                .await?
                .rows_affected();
        }
    }
    Ok(removed)
}

/// Idempotent policy seeding with the design §11.3 defaults. Safe to call
/// at startup and from read handlers.
///
/// A family whose stored rows are buckets is the one exception to leave the
/// existing row alone (review F4): its window is its contract, so its catalog
/// tuple is restored - but only while the row still disagrees with it, and only
/// the four columns the contract is made of. What an operator chose stays
/// visible in the audit trail, and a window the update path would refuse cannot
/// be kept alive by a row that predates the tier.
pub async fn ensure_seeded(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let mut tx = pool.begin().await?;
    for policy in POLICY_CATALOG {
        let sql = if aggregate_grain_seconds(policy.family).is_some() {
            "INSERT INTO retention_policies (family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by) VALUES (?, ?, ?, ?, ?, 1, ?, 'defaults') ON CONFLICT(family) DO UPDATE SET retention_days = excluded.retention_days, min_days = excluded.min_days, max_days = excluded.max_days, supported = excluded.supported WHERE retention_policies.retention_days <> excluded.retention_days OR retention_policies.min_days <> excluded.min_days OR retention_policies.max_days <> excluded.max_days OR retention_policies.supported <> excluded.supported"
        } else {
            "INSERT OR IGNORE INTO retention_policies (family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by) VALUES (?, ?, ?, ?, ?, 1, ?, 'defaults')"
        };
        sqlx::query(sql)
            .bind(policy.family)
            .bind(policy.default_days)
            .bind(policy.min_days)
            .bind(policy.max_days)
            .bind(policy.supported as i64)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

pub async fn list_policies(pool: &SqlitePool) -> Result<Vec<PolicyRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, i64, i64, i64, i64, i64, String, Option<String>)>(
        "SELECT family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by FROM retention_policies ORDER BY family",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                family,
                retention_days,
                min_days,
                max_days,
                supported,
                enabled,
                updated_at,
                updated_by,
            )| {
                PolicyRow {
                    family,
                    retention_days,
                    min_days,
                    max_days,
                    supported: supported == 1,
                    enabled: enabled == 1,
                    updated_at,
                    updated_by,
                }
            },
        )
        .collect())
}

/// Validate a proposed retention days value against the family's fixed
/// safety bounds. `0` means "keep forever" and is allowed only for
/// long-term families (max_days = 0 and min_days = 0).
pub fn validate_policy_days(family: &str, days: i64) -> Result<(), String> {
    let Some(catalog) = catalog_family(family) else {
        return Err(format!("unknown retention family {family}"));
    };
    // Issue #210: the class floor is part of the contract, so it holds even if
    // a family's declared minimum were wrong; audit_catalog proves the two
    // never disagree for a supported family.
    let floor = catalog.safety_floor_days();
    if days > 0 && days < floor {
        return Err(format!(
            "{} cannot be lowered below {} days ({})",
            catalog.label,
            floor,
            catalog
                .class
                .floor_reference()
                .unwrap_or("design §11.3 safety floor")
        ));
    }
    if days < 0 {
        return Err("retention days must be zero or positive".to_owned());
    }
    if catalog.max_days == 0 {
        // Long-term family: either keep forever or stay above the floor.
        if catalog.min_days == 0 {
            if days != 0 {
                return Err(format!(
                    "{} is a long-term family and can only be kept forever (0 days)",
                    catalog.label
                ));
            }
        } else if days != 0 && days < catalog.min_days {
            return Err(format!(
                "{} cannot be lowered below {} days (design §11.3 safety floor)",
                catalog.label, catalog.min_days
            ));
        }
    } else if !(catalog.min_days..=catalog.max_days).contains(&days) {
        return Err(format!(
            "{} must be between {} and {} days (design §11.3 safety bounds)",
            catalog.label, catalog.min_days, catalog.max_days
        ));
    }
    Ok(())
}

/// Apply a validated policy change. The caller writes the Audit row in the
/// same transaction (mutation handlers audit every policy change).
pub async fn update_policy(
    pool: &SqlitePool,
    family: &str,
    retention_days: i64,
    actor_user_id: &str,
) -> Result<PolicyRow, String> {
    validate_policy_days(family, retention_days)?;
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    sqlx::query(
        "UPDATE retention_policies SET retention_days = ?, enabled = 1, updated_at = ?, updated_by = ? WHERE family = ?",
    )
    .bind(retention_days)
    .bind(&now)
    .bind(actor_user_id)
    .bind(family)
    .execute(pool)
    .await
    .map_err(|error| format!("retention policy update failed: {error}"))?;
    list_policies(pool)
        .await
        .map_err(|error| format!("retention policy reload failed: {error}"))?
        .into_iter()
        .find(|policy| policy.family == family)
        .ok_or_else(|| format!("unknown retention family {family}"))
}

/// Read-only impact estimate for a proposed policy value: the estimated upper
/// bound of rows a run at that value could release. Never writes; used by the
/// edit form before typed confirmation (webui.md §8.4) and by the persisted
/// preview.
pub async fn estimate_impact(
    pool: &SqlitePool,
    family: &str,
    retention_days: i64,
    now: time::OffsetDateTime,
) -> Result<(i64, bool), sqlx::Error> {
    let Some(catalog) = catalog_family(family) else {
        return Ok((0, true));
    };
    if !catalog.supported {
        return Ok((0, true));
    }
    let entries = plan_family_targets(pool, family, retention_days, now).await?;
    Ok((entries.iter().map(|entry| entry.total).sum(), false))
}

/// One bounded plan entry per declared cleanup target of a family, with the
/// cutoff frozen and the estimated upper bound counted now. retention_days = 0
/// (keep forever) and families without cleanup targets produce nothing.
pub async fn plan_family_targets(
    pool: &SqlitePool,
    family: &str,
    retention_days: i64,
    now: time::OffsetDateTime,
) -> Result<Vec<PlanEntry>, sqlx::Error> {
    if retention_days == 0 {
        return Ok(Vec::new());
    }
    // The estimate counts against the same instant the cleanup deletes at, so a
    // preview never promises a release the run would not perform.
    let cutoff = crate::auth::format_rfc3339(release_cutoff(family, retention_days, now));
    count_targets(pool, family, &cutoff).await
}

/// Count what each declared target may release at one frozen cutoff. The SQL is
/// the target's own estimate statement, never request input.
async fn count_targets(
    pool: &SqlitePool,
    family: &str,
    cutoff: &str,
) -> Result<Vec<PlanEntry>, sqlx::Error> {
    let mut entries = Vec::new();
    for target in catalog_targets(family) {
        let total: i64 = sqlx::query_scalar(target.count_sql)
            .bind(cutoff)
            .fetch_one(pool)
            .await?;
        entries.push(PlanEntry {
            family: family.to_owned(),
            table: target.table.to_owned(),
            cutoff: cutoff.to_owned(),
            total,
            deleted: 0,
            done: false,
        });
    }
    Ok(entries)
}

/// Human-readable list of state that retention can never delete. Shown on
/// every retention surface so the safety contract stays explicit.
pub fn protected_state_notes() -> Vec<&'static str> {
    vec![
        "historical high-water marks",
        "coverage intervals",
        "open or permanent gap records",
        "cumulative block/transaction counters",
        "immutable Incident history",
        "Audit Events referenced by Operations",
        "Rule versions and policy rows",
        "open Peer presence intervals",
        "Validator daily snapshots and monthly aggregates",
        "Report Receipt identity rows (only the receipt body is slimmed)",
    ]
}

// ---------------------------------------------------------------------------
// Persisted authoritative impact preview (issue #210, Stories 37/38/41/42)
// ---------------------------------------------------------------------------

/// How a stored preview may be used. Execution fails closed: a preview that no
/// longer matches the current policy version, scope, or cutoff is rejected
/// instead of deleting against a plan the operator never reviewed.
#[derive(Debug, Clone)]
pub enum PreviewLoad {
    /// The preview still binds: every row it names is unchanged and its scope
    /// still resolves the same way.
    Ready(RetentionPreview),
    /// No stored preview with that id (never existed, or expired and pruned).
    NotFound,
    /// The preview exists but no longer binds. Reasons name families and use
    /// fixed wording, so they are safe to return to the caller.
    Stale(Vec<String>),
}

/// Why composing a preview failed.
#[derive(Debug)]
pub enum PreviewError {
    /// The requested scope is unusable (an unknown or empty family list).
    InvalidScope(String),
    /// The Server's own storage failed.
    Database(sqlx::Error),
    /// The Server's own encoding failed.
    Encode(String),
}

impl From<sqlx::Error> for PreviewError {
    fn from(error: sqlx::Error) -> Self {
        PreviewError::Database(error)
    }
}

/// One requested family the preview will not act on, and why: recorded so a run
/// warns about exactly the families the operator named instead of silently doing
/// less than the scope implies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewSkip {
    pub family: String,
    pub code: String,
    pub message: String,
}

/// One actionable family inside a preview: the policy value and row version it
/// was composed against, the cutoff the run must use verbatim, and the estimated
/// upper bound of rows it may release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewFamily {
    pub family: String,
    pub retention_days: i64,
    pub policy_version: String,
    pub cutoff: String,
    pub estimated_rows: i64,
    pub targets: Vec<PlanEntry>,
}

/// A persisted, Server-authoritative impact preview. A scope of None means it
/// was composed for every enabled and supported family; families is exactly what
/// a run will act on, and skipped records the requested families it will not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPreview {
    pub preview_id: String,
    pub created_at: String,
    pub created_by: String,
    pub expires_at: String,
    pub policy_version: String,
    pub scope: Option<Vec<String>>,
    pub families: Vec<PreviewFamily>,
    pub skipped: Vec<PreviewSkip>,
    pub estimated_rows: i64,
}

/// What the Server stores next to a preview: the requested scope and the
/// requested families it will not act on.
#[derive(Debug, Serialize, Deserialize)]
struct PreviewScope {
    scope: Option<Vec<String>>,
    skipped: Vec<PreviewSkip>,
}

/// One stored preview row.
#[derive(sqlx::FromRow)]
struct PreviewRow {
    preview_id: String,
    created_at: String,
    created_by: String,
    policy_version: String,
    scope_json: String,
    entries_json: String,
    estimated_rows: i64,
    expires_at: String,
}

/// Fingerprint of one stored policy row: any edit changes it, so a preview (and
/// the Admin edit form) can bind to the exact version it read.
pub fn policy_version(family: &str, retention_days: i64, updated_at: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{family}|{retention_days}|{updated_at}").as_bytes());
    crate::secrets::encode_hex(&hasher.finalize())
}

/// Fingerprint of a whole previewed scope: the rows it binds plus the cutoffs
/// and estimates those rows produced.
pub fn preview_version(families: &[PreviewFamily]) -> String {
    let mut hasher = Sha256::new();
    for family in families {
        hasher.update(
            format!(
                "{}|{}|{}|{}|{}",
                family.family,
                family.retention_days,
                family.policy_version,
                family.cutoff,
                family.estimated_rows
            )
            .as_bytes(),
        );
    }
    crate::secrets::encode_hex(&hasher.finalize())
}

/// The sanctioned wording every preview surface repeats, so no surface has to
/// invent its own promises about estimates or protected state.
pub fn preview_notes() -> Vec<&'static str> {
    vec![
        "Row counts are Server estimates of what the bounded cleanup may release; a run stops as soon as no row older than the frozen cutoff remains.",
        "The preview is bound to these policy versions, this scope, and one cutoff per family; any policy or scope change requires a new preview.",
        "A run deletes only rows older than the frozen cutoff, in bounded batches, and never touches protected state.",
    ]
}

/// Why a family cannot be actioned right now.
fn skip_reason(family: &str, row: Option<&PolicyRow>) -> Option<(&'static str, String)> {
    let Some(row) = row else {
        return Some((
            "retention_unknown_family",
            format!("{family}: this family has no stored policy row, skipped"),
        ));
    };
    if catalog_family(&row.family).is_none() || !row.supported {
        return Some((
            "retention_unsupported",
            format!(
                "{}: this family is not produced in the current phase, skipped",
                row.family
            ),
        ));
    }
    if !row.enabled {
        return Some((
            "retention_disabled",
            format!("{}: policy is disabled, skipped", row.family),
        ));
    }
    if row.retention_days == 0 {
        return Some((
            "retention_keep_forever",
            format!(
                "{}: policy keeps history forever, nothing to execute",
                row.family
            ),
        ));
    }
    None
}

/// Compose and persist an authoritative impact preview. Read-only with respect
/// to retained data: it writes the preview row itself, prunes expired previews,
/// and writes no Audit Event — the audited act is the run it authorizes.
pub async fn create_preview(
    pool: &SqlitePool,
    created_by: &str,
    families: Option<Vec<String>>,
    now: time::OffsetDateTime,
) -> Result<RetentionPreview, PreviewError> {
    ensure_seeded(pool).await?;
    let scope = match families {
        Some(list) => {
            let mut requested: Vec<String> = Vec::new();
            for family in list {
                if catalog_family(&family).is_none() {
                    return Err(PreviewError::InvalidScope(format!(
                        "unknown retention family {family}"
                    )));
                }
                if !requested.contains(&family) {
                    requested.push(family);
                }
            }
            if requested.is_empty() {
                return Err(PreviewError::InvalidScope(
                    "families must not be empty when provided".to_owned(),
                ));
            }
            Some(requested)
        }
        None => None,
    };
    let policies = list_policies(pool).await?;
    let selected: Vec<&PolicyRow> = match &scope {
        Some(requested) => requested
            .iter()
            .filter_map(|family| policies.iter().find(|policy| &policy.family == family))
            .collect(),
        None => policies
            .iter()
            .filter(|policy| skip_reason(&policy.family, Some(policy)).is_none())
            .collect(),
    };
    let mut preview_families: Vec<PreviewFamily> = Vec::new();
    let mut skipped: Vec<PreviewSkip> = Vec::new();
    for policy in selected {
        if let Some((code, message)) = skip_reason(&policy.family, Some(policy)) {
            // An explicit request reports every family it will not act on; the
            // default scope simply never includes them.
            if scope.is_some() {
                skipped.push(PreviewSkip {
                    family: policy.family.clone(),
                    code: code.to_owned(),
                    message,
                });
            }
            continue;
        }
        let cutoff =
            crate::auth::format_rfc3339(release_cutoff(&policy.family, policy.retention_days, now));
        let targets = count_targets(pool, &policy.family, &cutoff).await?;
        let estimated_rows = targets.iter().map(|target| target.total).sum();
        preview_families.push(PreviewFamily {
            family: policy.family.clone(),
            retention_days: policy.retention_days,
            policy_version: policy_version(
                &policy.family,
                policy.retention_days,
                &policy.updated_at,
            ),
            cutoff,
            estimated_rows,
            targets,
        });
    }
    let version = preview_version(&preview_families);
    let estimated_rows: i64 = preview_families
        .iter()
        .map(|family| family.estimated_rows)
        .sum();
    let scope_json = serde_json::to_string(&PreviewScope {
        scope: scope.clone(),
        skipped: skipped.clone(),
    })
    .map_err(|error| PreviewError::Encode(error.to_string()))?;
    let entries_json = serde_json::to_string(&preview_families)
        .map_err(|error| PreviewError::Encode(error.to_string()))?;
    let created_at = crate::auth::format_rfc3339(now);
    let expires_at = crate::auth::format_rfc3339(now + time::Duration::hours(PREVIEW_TTL_HOURS));
    let preview_id = preview_id(&version, &scope_json, &created_at);

    let mut tx = pool.begin().await?;
    // Only live previews matter, so expired rows are pruned in the same
    // transaction — bounded exactly like every other cleanup.
    sqlx::query("DELETE FROM retention_previews WHERE preview_id IN (SELECT preview_id FROM retention_previews WHERE expires_at <= ? LIMIT 128)")
        .bind(&created_at)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO retention_previews (preview_id, created_at, created_by, policy_version, scope_json, entries_json, estimated_rows, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(preview_id) DO UPDATE SET created_at = excluded.created_at, created_by = excluded.created_by, entries_json = excluded.entries_json, estimated_rows = excluded.estimated_rows, expires_at = excluded.expires_at")
        .bind(&preview_id)
        .bind(&created_at)
        .bind(created_by)
        .bind(&version)
        .bind(&scope_json)
        .bind(&entries_json)
        .bind(estimated_rows)
        .bind(&expires_at)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(RetentionPreview {
        preview_id,
        created_at,
        created_by: created_by.to_owned(),
        expires_at,
        policy_version: version,
        scope,
        families: preview_families,
        skipped,
        estimated_rows,
    })
}

/// Deterministic preview id: the same scope, versions, and cutoffs created in
/// the same second resolve to the same row, while any change to what was
/// previewed produces a new one.
fn preview_id(version: &str, scope_json: &str, created_at: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{version}|{scope_json}|{created_at}").as_bytes());
    let digest = crate::secrets::encode_hex(&hasher.finalize());
    format!("rp-{}", &digest[..32])
}

/// Load a stored preview and prove it still binds to the current policy
/// versions, scope, and cutoffs. Anything else fails closed (Story 38).
pub async fn load_preview_for_run(
    pool: &SqlitePool,
    preview_id: &str,
    now: time::OffsetDateTime,
) -> Result<PreviewLoad, sqlx::Error> {
    let row = sqlx::query_as::<_, PreviewRow>(
        "SELECT preview_id, created_at, created_by, policy_version, scope_json, entries_json, estimated_rows, expires_at FROM retention_previews WHERE preview_id = ?",
    )
    .bind(preview_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(PreviewLoad::NotFound);
    };
    let unreadable = || {
        PreviewLoad::Stale(vec![
            "the stored preview could not be read; create a new preview".to_owned(),
        ])
    };
    let Ok(scope) = serde_json::from_str::<PreviewScope>(&row.scope_json) else {
        return Ok(unreadable());
    };
    let Ok(families) = serde_json::from_str::<Vec<PreviewFamily>>(&row.entries_json) else {
        return Ok(unreadable());
    };
    let preview = RetentionPreview {
        preview_id: row.preview_id,
        created_at: row.created_at,
        created_by: row.created_by,
        expires_at: row.expires_at,
        policy_version: row.policy_version,
        scope: scope.scope,
        families,
        skipped: scope.skipped,
        estimated_rows: row.estimated_rows,
    };
    let mut reasons: Vec<String> = Vec::new();
    match crate::auth::parse_rfc3339(&preview.expires_at) {
        Some(expires) if expires > now => {}
        _ => push_reason(&mut reasons, "the preview expired; create a new preview"),
    }
    let policies = list_policies(pool).await?;
    if preview.scope.is_none() {
        // A default-scope preview binds to exactly the families that were
        // actionable then: a family silently entering or leaving the scope is a
        // change the operator never reviewed.
        let current: Vec<String> = policies
            .iter()
            .filter(|policy| skip_reason(&policy.family, Some(policy)).is_none())
            .map(|policy| policy.family.clone())
            .collect();
        let recorded: Vec<String> = preview
            .families
            .iter()
            .map(|family| family.family.clone())
            .collect();
        if current != recorded {
            push_reason(
                &mut reasons,
                "the enabled retention scope changed since the preview",
            );
        }
    }
    for family in &preview.families {
        match policies
            .iter()
            .find(|policy| policy.family == family.family)
        {
            None => push_reason(
                &mut reasons,
                &format!(
                    "{}: the retention policy is no longer configured",
                    family.family
                ),
            ),
            Some(row) => {
                if !row.supported || !row.enabled {
                    push_reason(
                        &mut reasons,
                        &format!(
                            "{}: the retention policy is no longer enabled",
                            family.family
                        ),
                    );
                } else if row.retention_days != family.retention_days
                    || policy_version(&row.family, row.retention_days, &row.updated_at)
                        != family.policy_version
                {
                    push_reason(
                        &mut reasons,
                        &format!(
                            "{}: the retention policy changed since the preview",
                            family.family
                        ),
                    );
                }
            }
        }
    }
    if let Some(requested) = &preview.scope {
        for family in requested {
            // Families recorded as actionable are covered by the loop above.
            if preview.families.iter().any(|entry| &entry.family == family) {
                continue;
            }
            let row = policies.iter().find(|policy| &policy.family == family);
            let recorded = preview
                .skipped
                .iter()
                .find(|skip| &skip.family == family)
                .map(|skip| skip.code.as_str());
            let current = skip_reason(family, row).map(|(code, _)| code);
            if recorded != current {
                push_reason(
                    &mut reasons,
                    &format!("{family}: the retention policy or scope changed since the preview"),
                );
            }
        }
    }
    if reasons.is_empty() {
        Ok(PreviewLoad::Ready(preview))
    } else {
        // Bound the message: the client gets the first few changes, not a wall.
        if reasons.len() > 3 {
            let hidden = reasons.len() - 3;
            reasons.truncate(3);
            reasons.push(format!("and {hidden} more change(s)"));
        }
        Ok(PreviewLoad::Stale(reasons))
    }
}

/// Record a reason once, keeping the order in which changes were detected.
fn push_reason(reasons: &mut Vec<String>, reason: &str) {
    if !reasons.iter().any(|existing| existing == reason) {
        reasons.push(reason.to_owned());
    }
}

/// The latest preview that is still executable, if any, so an Owner can return
/// to the surface and execute the same reviewed plan.
pub async fn latest_live_preview(
    pool: &SqlitePool,
    now: time::OffsetDateTime,
) -> Result<Option<RetentionPreview>, sqlx::Error> {
    let preview_id = sqlx::query_scalar::<_, String>(
        "SELECT preview_id FROM retention_previews WHERE expires_at > ? ORDER BY created_at DESC, preview_id DESC LIMIT 1",
    )
    .bind(crate::auth::format_rfc3339(now))
    .fetch_optional(pool)
    .await?;
    let Some(preview_id) = preview_id else {
        return Ok(None);
    };
    match load_preview_for_run(pool, &preview_id, now).await? {
        PreviewLoad::Ready(preview) => Ok(Some(preview)),
        PreviewLoad::NotFound | PreviewLoad::Stale(_) => Ok(None),
    }
}

/// The frozen execution plan a confirmed preview authorizes: exactly the entries
/// the operator previewed, with their estimated upper bounds. Nothing is
/// re-estimated at run time, so a policy edit during the run cannot widen what
/// was reviewed.
pub fn preview_plan(preview: &RetentionPreview) -> Vec<PlanEntry> {
    preview
        .families
        .iter()
        .flat_map(|family| family.targets.iter().cloned())
        .collect()
}

// ---------------------------------------------------------------------------
// Retention run Operation (kind `retention_run`)
// ---------------------------------------------------------------------------

/// Internal execution plan stored inside the Operation's params. Each entry
/// is one physical table batch target; divergence evidence spans two tables.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct PlanEntry {
    pub family: String,
    pub table: String,
    /// RFC3339 cutoff frozen at plan time. Re-reading the live policy on
    /// every batch could lengthen a policy mid-run and delete rows the
    /// operator never reviewed; the plan always executes against the snapshot
    /// the operator confirmed.
    pub cutoff: String,
    /// The preview's estimate for this target. Reporting only — an estimate is
    /// an upper bound, never a work quota: a batch count below it must not end
    /// the entry, and a count that reaches it does not prove the target is
    /// empty.
    pub total: i64,
    pub deleted: i64,
    /// Set once a bounded batch released fewer rows than one batch can hold,
    /// which proves the frozen cutoff has no expired row left: `total` is a
    /// Server estimate (an upper bound), not a frozen row set, so a shortfall
    /// completes the entry instead of retrying it forever.
    #[serde(default)]
    pub done: bool,
}

/// Advance one retention run by one bounded batch. The run executes the plan the
/// confirmed preview authorized, persists progress in
/// params_json/progress_percent, and only reaches a terminal state through
/// finalize — so a crash never fabricates success.
pub async fn execute_step(
    state: &AppState,
    operation_id: &str,
) -> Result<(), crate::operations::OperationError> {
    let pool = state.db().pool();
    let mut params = crate::operations::operation_params(pool, operation_id).await?;
    let now = crate::auth::now_utc();

    // Families the operator named in the preview that will not be acted on are
    // warned about exactly once: the flag travels with the persisted plan.
    if params.get("warningsRecorded").and_then(Value::as_bool) != Some(true) {
        if let Some(skipped) = params.get("skippedWarnings").and_then(Value::as_array) {
            for entry in skipped {
                let code = entry
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("retention_scope_skipped");
                let message = entry
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("a requested retention family was skipped");
                crate::operations::add_warning(state, operation_id, code, message).await?;
            }
        }
        params["warningsRecorded"] = Value::Bool(true);
        sqlx::query("UPDATE operations SET params_json = ? WHERE operation_id = ?")
            .bind(serde_json::to_string(&params)?)
            .bind(operation_id)
            .execute(pool)
            .await?;
    }

    if params.get("plan").and_then(Value::as_array).is_none() {
        // A run always queues with its preview's plan, so a missing plan means
        // the record was tampered with or truncated: fail closed.
        let _ = crate::operations::add_error(
            state,
            operation_id,
            "retention_plan_missing",
            "the confirmed retention plan is missing; nothing was executed",
        )
        .await;
        let _ = crate::operations::finalize(
            state,
            operation_id,
            crate::operations::STATUS_FAILED,
            None,
            &["retention"],
        )
        .await;
        return Ok(());
    }
    let mut plan = plan_entries(&params);

    if plan.is_empty() {
        finish_run(state, operation_id, params, Vec::new()).await?;
        return Ok(());
    }

    // First entry with remaining work. The preview's estimate is never a cap
    // (issue #210: the estimated row count is not frozen), so an entry is only
    // complete once a bounded batch found no expired row left — stopping at
    // "deleted == total" could leave rows the frozen cutoff still covers.
    let Some(index) = plan.iter().position(|entry| !entry.done) else {
        finish_run(state, operation_id, params, plan).await?;
        return Ok(());
    };

    // The operator's cancel is honoured at a safe checkpoint before anything
    // else: a run that deletes nothing because it was deliberately stopped must
    // not be reported as a failed run.
    if crate::operations::is_cancel_requested(state, operation_id).await? {
        // Story 40: a cancellation records what the run already released, what
        // it stopped short of, and which phase stopped it - a bare "cancelled"
        // status would hide both the partial release and the abandoned work.
        let outcome = cancellation_outcome(
            &plan,
            params.get("previewId").and_then(Value::as_str),
            CancellationPhase::Running,
        );
        crate::operations::finalize(
            state,
            operation_id,
            crate::operations::STATUS_CANCELLED,
            Some(&outcome),
            &["retention"],
        )
        .await?;
        return Ok(());
    }

    // Story 38: the preview is the authority this run was queued from, and the
    // frozen plan may only release what that preview reviewed. The binding is
    // re-verified once, before this run's first batch, and the flag travels with
    // the persisted plan: a preview that expired, a policy that moved, or a plan
    // that no longer matches the preview stops the run with nothing executed.
    if params.get("bindingVerified").and_then(Value::as_bool) != Some(true) {
        if let Some(reason) = preview_binding_mismatch(state, &params).await? {
            let _ = crate::operations::add_error(
                state,
                operation_id,
                "retention_preview_stale",
                &format!(
                    "the confirmed retention preview no longer authorizes this run ({reason}); nothing was executed; compose a new preview"
                ),
            )
            .await;
            let _ = crate::operations::finalize(
                state,
                operation_id,
                crate::operations::STATUS_FAILED,
                None,
                &["retention"],
            )
            .await;
            return Ok(());
        }
        params["bindingVerified"] = Value::Bool(true);
    }

    let entry = plan[index].clone();
    let Some(target) = catalog_targets(&entry.family)
        .iter()
        .find(|target| target.table == entry.table)
        .copied()
    else {
        // The plan names storage this Server no longer knows about: fail closed
        // instead of skipping the batch and reporting success.
        let _ = crate::operations::add_error(
            state,
            operation_id,
            "retention_target_unknown",
            &format!(
                "{}: {} is not a known cleanup target",
                entry.family, entry.table
            ),
        )
        .await;
        let _ = crate::operations::finalize(
            state,
            operation_id,
            crate::operations::STATUS_FAILED,
            None,
            &["retention"],
        )
        .await;
        return Ok(());
    };
    // Story 40: the release and the accounting of it commit together. The
    // cancellation summary reports "already released" as work that happened, so
    // a crash between the deletion and the plan write must not leave rows
    // deleted while the record still says they were never touched.
    let mut tx = pool.begin().await?;
    let result: Result<u64, sqlx::Error> = match target.kind {
        // ADR 0009: slimming rewrites the body in place instead of deleting the
        // row, so it cannot use the declared DELETE statements.
        CleanupKind::SlimReceiptBody => {
            slim_receipt_body_batch(&mut tx, &entry.cutoff, &crate::auth::format_rfc3339(now)).await
        }
        CleanupKind::Delete => sqlx::query(target.delete_sql)
            .bind(&entry.cutoff)
            .execute(&mut *tx)
            .await
            .map(|result| result.rows_affected()),
    };
    match result {
        Ok(rows) => apply_batch(&mut plan[index], rows),
        Err(error) => {
            let _ = tx.rollback().await;
            state.note_sqlite_error(&error);
            let _ = crate::operations::add_error(
                state,
                operation_id,
                "retention_batch_failed",
                &crate::redaction::redact_sensitive(&error.to_string()),
            )
            .await;
            let _ = crate::operations::finalize(
                state,
                operation_id,
                crate::operations::STATUS_FAILED,
                None,
                &["retention"],
            )
            .await;
            return Ok(());
        }
    }

    params["plan"] = serde_json::to_value(&plan)?;
    sqlx::query("UPDATE operations SET params_json = ? WHERE operation_id = ?")
        .bind(serde_json::to_string(&params)?)
        .bind(operation_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    // Progress counts a resolved entry as complete work: a bounded batch that
    // found nothing left is finished, not stalled.
    let total: i64 = plan.iter().map(|entry| entry.total).sum();
    let resolved: i64 = plan
        .iter()
        .map(|entry| {
            if entry.done {
                entry.total
            } else {
                entry.deleted
            }
        })
        .sum();
    let percent = if total == 0 {
        100
    } else {
        (resolved * 100) / total
    };
    let entry = &plan[index];
    crate::operations::set_progress(
        state,
        operation_id,
        percent,
        &format!("{} {}/{}", entry.family, entry.deleted, entry.total),
    )
    .await?;
    Ok(())
}

/// Record one bounded batch against its entry. Every declared target selects
/// its oldest `RETENTION_BATCH` matching rows, so a batch shorter than that
/// bound proves nothing expired is left behind the frozen cutoff and completes
/// the entry — while a full batch, or one that released nothing because there
/// was nothing to release, never ends an entry early. The estimate is never
/// consulted: it is an upper bound, not a work quota.
fn apply_batch(entry: &mut PlanEntry, rows: u64) {
    entry.deleted += rows as i64;
    if rows < RETENTION_BATCH as u64 {
        entry.done = true;
    }
}

/// Finish the run: report what was released per family against what the preview
/// estimated, and mark the Operation terminal.
async fn finish_run(
    state: &AppState,
    operation_id: &str,
    params: Value,
    plan: Vec<PlanEntry>,
) -> Result<(), crate::operations::OperationError> {
    let families = family_totals(&plan);
    // Preserve warnings recorded while composing the confirmed preview
    // (unsupported/disabled/kept-forever families): never plain Success for a
    // run that did less than its scope implies.
    let status = if plan.is_empty() && params.get("previewId").is_none() {
        crate::operations::STATUS_SUCCEEDED
    } else {
        let warnings: i64 = sqlx::query_scalar(
            "SELECT json_array_length(warnings_json) FROM operations WHERE operation_id = ?",
        )
        .bind(operation_id)
        .fetch_one(state.db().pool())
        .await?;
        if warnings > 0 {
            crate::operations::STATUS_SUCCEEDED_WITH_WARNINGS
        } else {
            crate::operations::STATUS_SUCCEEDED
        }
    };
    let mut result = serde_json::json!({ "families": families });
    if let Some(preview_id) = params.get("previewId") {
        result["previewId"] = preview_id.clone();
    }
    crate::operations::finalize(state, operation_id, status, Some(&result), &["retention"]).await?;
    Ok(())
}

/// Released rows per family against the preview's estimate, merged across the
/// family's physical tables. A partial run reports the same shape as a whole
/// one, so one client parser covers both.
fn family_totals(plan: &[PlanEntry]) -> Vec<Value> {
    let mut families: Vec<Value> = Vec::new();
    for entry in plan
        .iter()
        .filter(|entry| entry.deleted > 0 || entry.total > 0)
    {
        if let Some(existing) = families
            .iter_mut()
            .find(|value| value["family"] == entry.family)
        {
            existing["deletedRows"] =
                serde_json::json!(existing["deletedRows"].as_i64().unwrap_or(0) + entry.deleted);
            existing["estimatedRows"] =
                serde_json::json!(existing["estimatedRows"].as_i64().unwrap_or(0) + entry.total);
        } else {
            families.push(serde_json::json!({
                "family": entry.family,
                "deletedRows": entry.deleted,
                "estimatedRows": entry.total,
            }));
        }
    }
    families
}

/// Which phase of a run an accepted cancellation stopped it in: a queued run
/// never started, a running run stopped between bounded batches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellationPhase {
    Queued,
    Running,
}

impl CancellationPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            CancellationPhase::Queued => "queued",
            CancellationPhase::Running => "running",
        }
    }
}

/// The recorded outcome of a cancelled run (issue #211, story 40).
///
/// Cancellation stops the run at a safe checkpoint between bounded batches:
/// whatever it already released stays released - the Server never rolls a
/// release back - and the work that remained is reported as stopped work, not
/// as done. Remaining work is counted in plan entries that were never proven
/// complete, which is the honest measure: a remaining row count cannot be
/// derived from the preview estimate, because that is an upper bound and a
/// released count may already exceed it.
pub fn cancellation_outcome(
    plan: &[PlanEntry],
    preview_id: Option<&str>,
    phase: CancellationPhase,
) -> Value {
    let released_rows: i64 = plan.iter().map(|entry| entry.deleted).sum();
    let remaining_targets = plan.iter().filter(|entry| !entry.done).count() as i64;
    let mut result = serde_json::json!({
        "cancelled": {
            "phase": phase.as_str(),
            "releasedRows": released_rows,
            "remainingTargets": remaining_targets,
            "families": family_totals(plan),
            "note": "Cancellation stops the run at a safe checkpoint between bounded batches. Rows already released stay released - the Server never rolls a release back - and the work that remained was not attempted. Compose a fresh preview if the rest is still wanted.",
        }
    });
    if let Some(preview_id) = preview_id {
        result["previewId"] = serde_json::json!(preview_id);
    }
    result
}

/// The persisted plan of one retention run, decoded in one place so every
/// reader - the cancellation accounting, the restart accounting, and the
/// execution binding - keeps the same malformed-plan policy: an entry that
/// cannot be read is dropped, and a missing or unreadable plan is empty.
fn plan_entries(params: &Value) -> Vec<PlanEntry> {
    params
        .get("plan")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// What an interrupted run owns up to after a restart: how much of the confirmed
/// plan it had already released, and how much of it never ran (issue #211, story
/// 40). Released rows stay released, so the failure must say so.
pub fn interrupted_run_message(params: &Value) -> String {
    let plan = plan_entries(params);
    let released: i64 = plan.iter().map(|entry| entry.deleted).sum();
    let remaining = plan.iter().filter(|entry| !entry.done).count();
    format!(
        "Operation was interrupted by a Server restart while it was running: {released} rows were already released and stay released, and {remaining} planned targets were not completed; compose a fresh preview for the rest"
    )
}

/// The recorded outcome of a retention run cancelled while it was still queued:
/// its planned work never started, and the shared shape keeps one client parser
/// honest for both phases.
pub fn queued_run_cancellation(params: &Value) -> Value {
    let plan = plan_entries(params);
    cancellation_outcome(
        &plan,
        params.get("previewId").and_then(Value::as_str),
        CancellationPhase::Queued,
    )
}

/// Why the recorded plan no longer matches the confirmed preview, if it does
/// not (issue #211, story 38).
///
/// The preview's policy version is the only comparable authority:
/// `preview_version` hashes the estimated row counts, which drift with
/// wall-clock time, so re-deriving it after the fact can never match and is
/// never attempted. Cutoffs are compared against the preview's own frozen plan
/// because a cutoff recomputed with a newer clock legitimately differs.
async fn preview_binding_mismatch(
    state: &AppState,
    params: &Value,
) -> Result<Option<String>, crate::operations::OperationError> {
    let Some(preview_id) = params.get("previewId").and_then(Value::as_str) else {
        return Ok(Some(
            "the recorded plan carries no confirmed preview".to_owned(),
        ));
    };
    let frozen = plan_entries(params);
    let loaded =
        match load_preview_for_run(state.db().pool(), preview_id, crate::auth::now_utc()).await {
            Ok(loaded) => loaded,
            Err(error) => {
                state.note_sqlite_error(&error);
                return Ok(Some("the stored preview could not be read".to_owned()));
            }
        };
    let preview = match loaded {
        PreviewLoad::Ready(preview) => preview,
        PreviewLoad::NotFound => {
            return Ok(Some(format!(
                "the confirmed preview {preview_id} is missing or expired"
            )));
        }
        PreviewLoad::Stale(reasons) => return Ok(Some(reasons.join("; "))),
    };
    if params.get("previewPolicyVersion").and_then(Value::as_str)
        != Some(preview.policy_version.as_str())
    {
        return Ok(Some(
            "the recorded policy version no longer matches the stored preview".to_owned(),
        ));
    }
    let authorized = preview_plan(&preview);
    if let Some(entry) = frozen.iter().find(|frozen| {
        !authorized
            .iter()
            .any(|allowed| same_binding(allowed, frozen))
    }) {
        return Ok(Some(format!(
            "{}: {} is not in the confirmed preview at the recorded cutoff",
            entry.family, entry.table
        )));
    }
    if let Some(entry) = authorized
        .iter()
        .find(|allowed| !frozen.iter().any(|frozen| same_binding(allowed, frozen)))
    {
        return Ok(Some(format!(
            "{}: {} was confirmed but is missing from the recorded plan",
            entry.family, entry.table
        )));
    }
    Ok(None)
}

/// The identity of one planned cleanup target: a family/table pair bound to the
/// cutoff it was reviewed at.
fn same_binding(left: &PlanEntry, right: &PlanEntry) -> bool {
    left.family == right.family && left.table == right.table && left.cutoff == right.cutoff
}

/// Catalog self-check (issue #210). Proves the onboarding contract before any
/// policy or cleanup surface is served: a family cannot carry a bound below its
/// class floor, an unsupported family cannot declare cleanup storage, and every
/// declared target must bind exactly one cutoff with a bounded batch. Called at
/// startup and asserted in tests.
pub fn audit_catalog() -> Result<(), String> {
    let mut families: Vec<&'static str> = Vec::new();
    for entry in POLICY_CATALOG.iter() {
        if families.contains(&entry.family) {
            return Err(format!("{}: duplicate retention family", entry.family));
        }
        families.push(entry.family);
        if entry.default_days < 0 || entry.min_days < 0 || entry.max_days < 0 {
            return Err(format!(
                "{}: retention bounds must not be negative",
                entry.family
            ));
        }
        let bounded = entry.max_days != 0;
        if bounded && (entry.min_days > entry.max_days || entry.default_days > entry.max_days) {
            return Err(format!(
                "{}: default and floor must stay within the upper bound",
                entry.family
            ));
        }
        if !bounded && entry.default_days != 0 && entry.default_days < entry.min_days {
            return Err(format!(
                "{}: a long-term default must be forever (0) or at least the floor",
                entry.family
            ));
        }
        if entry.supported && bounded && entry.min_days < entry.class.floor_days() {
            return Err(format!(
                "{}: the {} day floor is below the {} day {:?} class floor",
                entry.family,
                entry.min_days,
                entry.class.floor_days(),
                entry.class
            ));
        }
        if !entry.supported && !entry.targets.is_empty() {
            return Err(format!(
                "{}: an unsupported family must declare no cleanup target",
                entry.family
            ));
        }
        if entry.supported && bounded && entry.targets.is_empty() {
            return Err(format!(
                "{}: a supported bounded family must declare a cleanup target",
                entry.family
            ));
        }
        let mut tables: Vec<&'static str> = Vec::new();
        for target in entry.targets {
            if tables.contains(&target.table) {
                return Err(format!(
                    "{}: {} is declared twice",
                    entry.family, target.table
                ));
            }
            tables.push(target.table);
            if target.count_sql.matches('?').count() != 1 {
                return Err(format!(
                    "{}: the {} estimate must bind exactly one cutoff",
                    entry.family, target.table
                ));
            }
            if target.kind == CleanupKind::Delete {
                if target.delete_sql.matches('?').count() != 1 {
                    return Err(format!(
                        "{}: the {} cleanup must bind exactly one cutoff",
                        entry.family, target.table
                    ));
                }
                if !target.delete_sql.contains("LIMIT") {
                    return Err(format!(
                        "{}: the {} cleanup must be a bounded batch",
                        entry.family, target.table
                    ));
                }
            }
        }
    }
    if !POLICY_CATALOG
        .iter()
        .any(|entry| entry.supported && entry.class == PolicyClass::Raw)
    {
        return Err("no supported raw family carries the 24-hour investigation floor".to_owned());
    }
    if !POLICY_CATALOG
        .iter()
        .any(|entry| entry.supported && entry.class == PolicyClass::Investigation)
    {
        return Err("no supported family carries the 30-day investigation floor".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Queue a retention run exactly as the HTTP layer does: the confirmed
    /// preview's frozen plan, its policy version, and the families it will not
    /// act on.
    async fn queue_retention_run(
        pool: &SqlitePool,
        operation_id: &str,
        request_id: &str,
        preview: &RetentionPreview,
        created_at: &str,
    ) {
        let params = serde_json::json!({
            "previewId": preview.preview_id,
            "previewPolicyVersion": preview.policy_version,
            "skippedWarnings": preview.skipped,
            "plan": preview_plan(preview),
        });
        sqlx::query("INSERT INTO operations (operation_id, kind, status, request_id, params_json, warnings_json, errors_json, created_at) VALUES (?, 'retention_run', 'queued', ?, ?, '[]', '[]', ?)")
            .bind(operation_id)
            .bind(request_id)
            .bind(params.to_string())
            .bind(created_at)
            .execute(pool)
            .await
            .unwrap();
    }

    #[test]
    fn receipt_body_slimming_window_is_fixed_and_catalogued() {
        let catalog = catalog_family(FAMILY_REPORT_RECEIPT_BODY).expect("catalogued");
        assert!(catalog.supported);
        assert_eq!(catalog.default_days, RECEIPT_BODY_SLIMMING_DAYS);
        assert_eq!((catalog.min_days, catalog.max_days), (30, 30));
        assert_eq!(validate_policy_days(FAMILY_REPORT_RECEIPT_BODY, 30), Ok(()));
        assert!(validate_policy_days(FAMILY_REPORT_RECEIPT_BODY, 7).is_err());
        assert!(validate_policy_days(FAMILY_REPORT_RECEIPT_BODY, 0).is_err());
        assert!(validate_policy_days(FAMILY_REPORT_RECEIPT_BODY, 365).is_err());
    }

    #[tokio::test]
    async fn receipt_body_slimming_clears_detail_but_keeps_identity_and_closing_receipts() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let old = crate::auth::format_rfc3339(now - time::Duration::days(31));
        let fresh = crate::auth::format_rfc3339(now - time::Duration::days(1));
        let cutoff = crate::auth::format_rfc3339(family_cutoff(now, RECEIPT_BODY_SLIMMING_DAYS));
        let slimmed_at = crate::auth::format_rfc3339(now);

        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-a', 1, ?, ?)")
            .bind(&fresh)
            .bind(&fresh)
            .execute(pool)
            .await
            .unwrap();
        let body = |tag: &str| {
            serde_json::to_vec(&serde_json::json!({
                "report_id": tag,
                "disposition": "accepted",
                "report_body_sha256": "a".repeat(64),
                "server_version": "x",
                "supported_protocol_majors": [1, 2],
                "server_time": "2026-01-01T00:00:00Z",
                "rotation_hint": null,
                "inventory": "accepted",
                "rejections": [],
                "nodes": [{"node_id": "node-a", "current": "accepted"}],
                "samples": [{"kind": "block", "disposition": "accepted"}],
            }))
            .unwrap()
        };
        let insert = "INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at) VALUES (?, 'agent-a', 1, 'boot-a', ?, ?, 'accepted', ?, ?)";
        sqlx::query(insert)
            .bind("old-1")
            .bind(1_i64)
            .bind("a".repeat(64))
            .bind(body("old-1"))
            .bind(&old)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(insert)
            .bind("recent-1")
            .bind(2_i64)
            .bind("b".repeat(64))
            .bind(body("recent-1"))
            .bind(&fresh)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(insert)
            .bind("closing-1")
            .bind(3_i64)
            .bind("c".repeat(64))
            .bind(body("closing-1"))
            .bind(&old)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("UPDATE agents SET close_report_id = 'closing-1' WHERE agent_id = 'agent-a'")
            .execute(pool)
            .await
            .unwrap();

        let (estimated, unsupported) = estimate_impact(
            pool,
            FAMILY_REPORT_RECEIPT_BODY,
            RECEIPT_BODY_SLIMMING_DAYS,
            now,
        )
        .await
        .unwrap();
        assert!(!unsupported);
        assert_eq!(
            estimated, 1,
            "only the unreferenced old receipt is eligible for slimming"
        );

        // The batch shares one connection, exactly as the run's transaction
        // hands it one (issue #211, story 40).
        let mut conn = pool.acquire().await.unwrap();
        assert_eq!(
            slim_receipt_body_batch(&mut conn, &cutoff, &slimmed_at)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            slim_receipt_body_batch(&mut conn, &cutoff, &slimmed_at)
                .await
                .unwrap(),
            0
        );
        // Hand the pooled connection back: the assertions below read through the
        // pool again, and a held connection could starve it.
        drop(conn);

        // The old unreferenced row keeps its identity columns and loses detail.
        let (disposition, hash, slimmed, body_text): (String, String, Option<String>, String) =
            sqlx::query_as("SELECT disposition, report_body_sha256, receipt_slimmed_at, CAST(receipt_body AS TEXT) FROM agent_report_receipts WHERE report_id = 'old-1'")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(disposition, "accepted");
        assert_eq!(hash, "a".repeat(64));
        assert!(slimmed.is_some());
        let value: Value = serde_json::from_str(&body_text).unwrap();
        assert_eq!(value["nodes"], serde_json::json!([]));
        assert_eq!(value["samples"], serde_json::json!([]));
        assert_eq!(value["inventory"], "accepted");

        // The recent row and the Agent's Closing Receipt stay verbatim.
        for report_id in ["recent-1", "closing-1"] {
            let (slimmed, body_text): (Option<String>, String) = sqlx::query_as(
                "SELECT receipt_slimmed_at, CAST(receipt_body AS TEXT) FROM agent_report_receipts WHERE report_id = ?",
            )
            .bind(report_id)
            .fetch_one(pool)
            .await
            .unwrap();
            assert!(slimmed.is_none(), "{report_id} was slimmed");
            let value: Value = serde_json::from_str(&body_text).unwrap();
            assert!(
                !value["nodes"].as_array().unwrap().is_empty(),
                "{report_id} lost its per-Node detail"
            );
        }
    }

    #[tokio::test]
    async fn cleanup_is_bounded_and_preserves_dedup_state_in_temp_sqlite() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
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

    /// Issue #137: the cleanup statement runs on the report ingestion path.
    /// A full scan of `block_summaries` plus a temporary B-tree made one
    /// bounded cleanup cost ~124 ms on a 260 MB table even when nothing was
    /// expired, so catch-up burned a core without committing anything and
    /// Agents exhausted their 5 s sender deadline. This asserts the
    /// index-backed plan instead of a wall-clock budget so the guard is
    /// deterministic on any machine.
    #[tokio::test]
    async fn cleanup_plan_is_index_backed_and_never_scans_the_whole_table() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(&format!(
            "EXPLAIN QUERY PLAN {RAW_BLOCK_SUMMARY_CLEANUP_SQL}"
        ))
        .fetch_all(database.pool())
        .await
        .unwrap();
        let plan = rows
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            !plan.contains("SCAN block_summaries"),
            "raw Block Summary retention must not scan the table it prunes: {plan}"
        );
        assert!(
            plan.contains("block_summaries_accepted_at_idx"),
            "raw Block Summary retention must be served by the accepted_at index: {plan}"
        );
        assert!(
            !plan.contains("TEMP B-TREE"),
            "the ordered LIMIT must be satisfied by the index: {plan}"
        );
    }

    #[tokio::test]
    async fn policy_bounds_and_validation_follow_the_catalog() {
        assert_eq!(validate_policy_days("raw_block_summary", 7), Ok(()));
        assert!(validate_policy_days("raw_block_summary", 0).is_err());
        assert!(validate_policy_days("raw_block_summary", 31).is_err());
        assert_eq!(validate_policy_days("history_gap", 0), Ok(()));
        assert!(validate_policy_days("history_gap", 179).is_err());
        assert_eq!(validate_policy_days("one_hour_aggregate", 0), Ok(()));
        assert!(validate_policy_days("one_hour_aggregate", 30).is_err());
        assert_eq!(
            validate_policy_days(FAMILY_PEER_PRESENCE_INTERVAL, 30),
            Ok(())
        );
        assert!(validate_policy_days(FAMILY_PEER_PRESENCE_INTERVAL, 0).is_err());
        assert!(validate_policy_days(FAMILY_PEER_PRESENCE_INTERVAL, 366).is_err());
        assert!(validate_policy_days("unknown_family", 7).is_err());
    }

    #[tokio::test]
    async fn peer_presence_retention_deletes_only_old_closed_intervals() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = crate::auth::now_utc();
        let now_text = crate::auth::format_rfc3339(now);
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('peer-retention-network', 'Peer Retention', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('peer-retention-agent', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('peer-retention-node', 'peer-retention-agent', 'peer-retention-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        let insert = "INSERT INTO peer_presence_intervals (node_id, peer_id, direction, trusted, static_peer, consensus_peer, client_name, opened_at, closed_at) VALUES ('peer-retention-node', ?, 'inbound', 1, 0, 1, 'PlatON/v1.5.1', ?, ?)";
        sqlx::query(insert)
            .bind("peer-old")
            .bind("2020-01-01T00:00:00Z")
            .bind("2020-01-02T00:00:00Z")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(insert)
            .bind("peer-open")
            .bind("2020-01-01T00:00:00Z")
            .bind(None::<String>)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(insert)
            .bind("peer-fresh")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        ensure_seeded(pool).await.unwrap();
        let preview = create_preview(
            pool,
            "owner-1",
            Some(vec![FAMILY_PEER_PRESENCE_INTERVAL.to_owned()]),
            now,
        )
        .await
        .unwrap();
        let operation_id = "peer-retention-operation";
        queue_retention_run(
            pool,
            operation_id,
            "peer-retention-request",
            &preview,
            &now_text,
        )
        .await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let state = AppState::new(
            database,
            None,
            crate::auth::AuthConfig::development(
                crate::secrets::load_pepper_file(&pepper_path).unwrap(),
                "http://127.0.0.1:8080".to_owned(),
            ),
        );

        execute_step(&state, operation_id).await.unwrap();
        execute_step(&state, operation_id).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM operations WHERE operation_id=?",)
                .bind(operation_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            crate::operations::STATUS_SUCCEEDED
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE peer_id='peer-old'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE peer_id='peer-open'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "retention never deletes open intervals"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE peer_id='peer-fresh'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "retention preserves intervals inside the configured window"
        );
    }

    #[tokio::test]
    async fn aggregate_retention_deletes_old_five_minute_rows_and_keeps_hourly_forever() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = crate::auth::now_utc();
        let now_text = crate::auth::format_rfc3339(now);
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('aggregate-retention-network', 'Aggregate Retention', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('aggregate-retention-agent', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('aggregate-retention-node', 'aggregate-retention-agent', 'aggregate-retention-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        let old = "2020-01-01T00:00:00Z";
        let insert_5m = "INSERT INTO peer_aggregate_5m (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, first_observed_at, last_observed_at) VALUES ('aggregate-retention-node', ?, 1, 1, 1, 0, 1, 0, 1, 0, 1, 0, 0, 0, 0, ?, ?)";
        sqlx::query(insert_5m)
            .bind(old)
            .bind(old)
            .bind(old)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(insert_5m)
            .bind(&now_text)
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO peer_aggregate_5m_countries (node_id, bucket_start, country_code, peer_count) VALUES ('aggregate-retention-node', ?, 'US', 1)")
            .bind(old)
            .execute(pool)
            .await
            .unwrap();
        let insert_1h = "INSERT INTO peer_aggregate_1h (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, first_observed_at, last_observed_at) VALUES ('aggregate-retention-node', ?, 1, 1, 1, 0, 1, 0, 1, 0, 1, 0, 0, 0, 0, ?, ?)";
        sqlx::query(insert_1h)
            .bind(old)
            .bind(old)
            .bind(old)
            .execute(pool)
            .await
            .unwrap();
        ensure_seeded(pool).await.unwrap();
        let preview = create_preview(
            pool,
            "owner-1",
            Some(vec![
                FAMILY_PEER_AGGREGATE_5M.to_owned(),
                FAMILY_PEER_AGGREGATE_1H.to_owned(),
            ]),
            now,
        )
        .await
        .unwrap();
        assert_eq!(preview.families.len(), 1);
        assert_eq!(preview.skipped.len(), 1);
        let operation_id = "aggregate-retention-operation";
        queue_retention_run(
            pool,
            operation_id,
            "aggregate-retention-request",
            &preview,
            &now_text,
        )
        .await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let state = AppState::new(
            database,
            None,
            crate::auth::AuthConfig::development(
                crate::secrets::load_pepper_file(&pepper_path).unwrap(),
                "http://127.0.0.1:8080".to_owned(),
            ),
        );

        execute_step(&state, operation_id).await.unwrap();
        execute_step(&state, operation_id).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM operations WHERE operation_id=?")
                .bind(operation_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            crate::operations::STATUS_SUCCEEDED_WITH_WARNINGS
        );
        let warnings: String =
            sqlx::query_scalar("SELECT warnings_json FROM operations WHERE operation_id=?")
                .bind(operation_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert!(warnings.contains("retention_keep_forever"), "{warnings}");
        assert!(warnings.contains("keeps history forever"), "{warnings}");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_aggregate_5m WHERE bucket_start=?"
            )
            .bind(old)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_aggregate_5m_countries WHERE bucket_start=?"
            )
            .bind(old)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "country rows are removed by the aggregate foreign-key cascade"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_aggregate_5m WHERE bucket_start=?"
            )
            .bind(&now_text)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_aggregate_1h WHERE bucket_start=?"
            )
            .bind(old)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "the configured hourly zero-day policy keeps long-term rows"
        );
    }

    #[tokio::test]
    async fn seeding_is_idempotent_and_impact_counts_only_old_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        ensure_seeded(pool).await.unwrap();
        ensure_seeded(pool).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM retention_policies")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(count, POLICY_CATALOG.len() as i64);

        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        // Issue #214 made one_minute_aggregate a produced family, so the
        // not-yet-produced example is the 1-hour tier the design still defers.
        let (estimated, unsupported) = estimate_impact(pool, FAMILY_ONE_HOUR_AGGREGATE, 90, now)
            .await
            .unwrap();
        assert!(unsupported);
        assert_eq!(estimated, 0);
        let (estimated, unsupported) = estimate_impact(pool, "raw_block_summary", 7, now)
            .await
            .unwrap();
        assert!(!unsupported);
        assert_eq!(estimated, 0);
    }
    /// Issue #214: the 1-minute tier serves the stretch between the raw window
    /// and 7 days and the 5-minute tier the 30-day investigation horizon, so a
    /// bucket is released by its own grain: a 10-day-old bucket survives the
    /// fine tier's cleanup exactly because only the coarse tier still answers
    /// it, and widening the read therefore never recovers raw samples.
    #[tokio::test]
    async fn metric_aggregate_cleanup_expires_each_tier_by_its_own_window() {
        let one_minute = catalog_family(FAMILY_ONE_MINUTE_AGGREGATE).unwrap();
        assert!(one_minute.supported);
        assert_eq!(one_minute.default_days, 7);
        assert_eq!(one_minute.max_days, 7);
        assert!(validate_policy_days(FAMILY_ONE_MINUTE_AGGREGATE, 8).is_err());
        assert_eq!(validate_policy_days(FAMILY_ONE_MINUTE_AGGREGATE, 7), Ok(()));
        let five_minute = catalog_family(FAMILY_FIVE_MINUTE_AGGREGATE).unwrap();
        assert!(five_minute.supported);
        assert_eq!(five_minute.class, PolicyClass::Investigation);
        assert_eq!(five_minute.default_days, MIN_INVESTIGATION_AGGREGATE_DAYS);
        assert!(
            validate_policy_days(FAMILY_FIVE_MINUTE_AGGREGATE, 4).is_err(),
            "the coarse tier cannot be shortened below the investigation floor"
        );

        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = crate::auth::now_utc();
        let now_text = crate::auth::format_rfc3339(now);
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('tier-network', 'Tier', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('tier-agent', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('tier-node', 'tier-agent', 'tier-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        let insert = "INSERT INTO node_metric_aggregates (node_id, metric, grain_seconds, bucket_start, sample_count, min_value, max_value, last_value, first_observed_at, last_observed_at, last_received_at, updated_at) VALUES ('tier-node', 'process_cpu_percent', ?, ?, 1, 1.0, 2.0, 2.0, ?, ?, ?, ?)";
        let expired = "2020-01-01T00:00:00Z";
        let mid = crate::auth::format_rfc3339(now - time::Duration::days(10));
        for (grain, bucket) in [
            (60_i64, expired),
            (60, now_text.as_str()),
            (300, expired),
            (300, mid.as_str()),
            (300, now_text.as_str()),
        ] {
            sqlx::query(insert)
                .bind(grain)
                .bind(bucket)
                .bind(bucket)
                .bind(bucket)
                .bind(bucket)
                .bind(&now_text)
                .execute(pool)
                .await
                .unwrap();
        }
        ensure_seeded(pool).await.unwrap();

        let removed = cleanup_expired_metric_aggregates(pool, now).await.unwrap();
        assert_eq!(removed, 2, "one expired bucket per tier, nothing else");
        let remaining: Vec<(i64, String)> = sqlx::query_as(
            "SELECT grain_seconds, bucket_start FROM node_metric_aggregates ORDER BY grain_seconds, bucket_start",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        assert_eq!(
            remaining,
            vec![
                (60, now_text.clone()),
                (300, mid.clone()),
                (300, now_text.clone()),
            ],
            "each tier releases only what its own window has passed"
        );
        // The tier a widened read can no longer be served leaves while the
        // buckets that still answer the same stretch stay behind, and the raw
        // samples are not part of this cleanup at all.
        let raw_owned: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_metric_aggregates WHERE grain_seconds = 60",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(raw_owned, 1);
    }

    /// Issue #214 review F2: a bucket is released only once every observation it
    /// counted is outside the window. The cutoff falls inside a bucket whose
    /// first observations are already expired, and that bucket is the only
    /// evidence left for the stretch just inside the horizon, so the cleanup
    /// leaves it behind - and the read aligns its own floor down to the same
    /// boundary, so nothing inside the horizon is unanswerable.
    #[tokio::test]
    async fn aggregate_cleanup_keeps_the_bucket_the_cutoff_falls_inside() {
        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        assert_eq!(
            release_cutoff(FAMILY_FIVE_MINUTE_AGGREGATE, 30, now),
            family_cutoff(now, 30) - time::Duration::seconds(300),
            "an aggregate tier releases one bucket width behind its own cutoff"
        );
        assert_eq!(
            release_cutoff(FAMILY_ONE_MINUTE_AGGREGATE, 7, now),
            family_cutoff(now, 7) - time::Duration::seconds(60)
        );
        assert_eq!(
            release_cutoff(FAMILY_RAW_METRIC_SAMPLE, 1, now),
            family_cutoff(now, 1),
            "a family whose rows are not buckets releases at its own cutoff"
        );

        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = crate::auth::now_utc();
        let now_text = crate::auth::format_rfc3339(now);
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('tier-network', 'Tier', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('tier-agent', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('tier-node', 'tier-agent', 'tier-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        let insert = "INSERT INTO node_metric_aggregates (node_id, metric, grain_seconds, bucket_start, sample_count, min_value, max_value, last_value, first_observed_at, last_observed_at, last_received_at, updated_at) VALUES ('tier-node', 'process_cpu_percent', ?, ?, 1, 1.0, 2.0, 2.0, ?, ?, ?, ?)";
        // The bucket the seven-day cutoff falls inside: it starts before the
        // cutoff and reaches past it.
        let minute_straddler = crate::auth::format_rfc3339(
            now - time::Duration::days(7) - time::Duration::seconds(30),
        );
        // The bucket the thirty-day cutoff falls inside, and a bucket whose own
        // window has been passed entirely.
        let straddler = crate::auth::format_rfc3339(
            now - time::Duration::days(30) - time::Duration::seconds(120),
        );
        let released = crate::auth::format_rfc3339(
            now - time::Duration::days(30) - time::Duration::seconds(700),
        );
        let inside = crate::auth::format_rfc3339(
            now - time::Duration::days(30) + time::Duration::seconds(600),
        );
        for (grain, bucket) in [
            (60_i64, minute_straddler.as_str()),
            (300, straddler.as_str()),
            (300, released.as_str()),
            (300, inside.as_str()),
        ] {
            sqlx::query(insert)
                .bind(grain)
                .bind(bucket)
                .bind(bucket)
                .bind(bucket)
                .bind(bucket)
                .bind(&now_text)
                .execute(pool)
                .await
                .unwrap();
        }
        ensure_seeded(pool).await.unwrap();
        assert_eq!(
            aggregate_retention_days(pool, FAMILY_ONE_MINUTE_AGGREGATE)
                .await
                .unwrap(),
            7
        );
        assert_eq!(
            aggregate_retention_days(pool, FAMILY_FIVE_MINUTE_AGGREGATE)
                .await
                .unwrap(),
            30
        );

        let removed = cleanup_expired_metric_aggregates(pool, now).await.unwrap();
        assert_eq!(
            removed, 1,
            "only a bucket whose own observations have all expired leaves"
        );
        let remaining: Vec<(i64, String)> = sqlx::query_as(
            "SELECT grain_seconds, bucket_start FROM node_metric_aggregates ORDER BY grain_seconds, bucket_start",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        assert_eq!(
            remaining,
            vec![(60, minute_straddler), (300, straddler), (300, inside),],
            "the buckets that still hold an observation inside their window stay"
        );
    }

    /// Issue #214 review F4: the stored window of an aggregate tier is the
    /// tier's contract, so seeding restores the catalog tuple of that family -
    /// while the operator's own record of the value (who set it, when, and
    /// whether the family is enabled) is left as it is.
    #[tokio::test]
    async fn seeding_restores_the_tier_contract_and_keeps_the_operator_record() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        ensure_seeded(pool).await.unwrap();
        sqlx::query(
            "UPDATE retention_policies SET retention_days = 30, min_days = 7, max_days = 365, supported = 0, enabled = 0, updated_by = 'owner-1' WHERE family = 'one_minute_aggregate'",
        )
        .execute(pool)
        .await
        .unwrap();
        // The cleanup reads the contract, not the stale row, even before any
        // seeding has repaired it.
        assert_eq!(
            aggregate_retention_days(pool, FAMILY_ONE_MINUTE_AGGREGATE)
                .await
                .unwrap(),
            7
        );

        ensure_seeded(pool).await.unwrap();
        let stored: (i64, i64, i64, i64, i64, Option<String>) = sqlx::query_as(
            "SELECT retention_days, min_days, max_days, supported, enabled, updated_by FROM retention_policies WHERE family = 'one_minute_aggregate'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(
            stored,
            (7, 7, 7, 1, 0, Some("owner-1".to_owned())),
            "the contract is restored; the operator's own setting and record are not rewritten"
        );
        // The family the operator left alone is not rewritten either.
        let raw: (i64, i64, i64) = sqlx::query_as(
            "SELECT retention_days, min_days, max_days FROM retention_policies WHERE family = 'raw_metric_sample'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(raw, (1, 1, 30));
    }

    /// Issue #210: the compiled catalog is the contract itself. A family cannot
    /// be onboarded below its class floor, and both investigation floors are
    /// carried by families the Server actually produces.
    #[test]
    fn catalog_satisfies_the_investigation_contract() {
        assert_eq!(audit_catalog(), Ok(()));
        let raw = catalog_family(FAMILY_RAW_BLOCK_SUMMARY).unwrap();
        assert_eq!(raw.class, PolicyClass::Raw);
        assert_eq!(raw.safety_floor_days(), MIN_INVESTIGATION_RAW_DAYS);
        assert!(raw.safety_floor_days() * 24 >= MIN_INVESTIGATION_RAW_HOURS);
        // Issue #214: the coarse metric tier is the family that carries the
        // investigation horizon for Node metric history, so it must respect the
        // same 30-day floor the other investigation families do.
        for family in [
            FAMILY_HISTORY_GAP,
            FAMILY_DIVERGENCE_OBSERVATION,
            FAMILY_FIVE_MINUTE_AGGREGATE,
        ] {
            let entry = catalog_family(family).unwrap();
            assert_eq!(entry.class, PolicyClass::Investigation);
            assert_eq!(entry.class.floor_days(), MIN_INVESTIGATION_AGGREGATE_DAYS);
            assert!(
                entry.safety_floor_days() >= MIN_INVESTIGATION_AGGREGATE_DAYS,
                "{family} must respect the 30-day investigation floor"
            );
        }
        {
            let family = FAMILY_ONE_HOUR_AGGREGATE;
            let entry = catalog_family(family).unwrap();
            assert!(!entry.supported, "{family} is not produced in this phase");
            assert!(
                catalog_targets(family).is_empty(),
                "an unsupported family must not imply cleanup storage"
            );
        }
        // Supported families that are kept forever carry no cleanup storage
        // either, so a run can never mistake them for expendable history.
        for family in [
            FAMILY_VALIDATOR_DAILY_SNAPSHOT,
            FAMILY_VALIDATOR_MONTHLY_AGGREGATE,
        ] {
            let entry = catalog_family(family).unwrap();
            assert!(entry.supported);
            assert_eq!(entry.max_days, 0);
            assert_eq!(validate_policy_days(family, 0), Ok(()));
            assert!(validate_policy_days(family, 30).is_err());
            assert!(catalog_targets(family).is_empty());
        }
    }

    /// Issue #213: the raw metric family carries the 24-hour raw floor itself,
    /// so the window the Admin surface promises cannot be edited away, and both
    /// sample tables declare a bounded cleanup target.
    #[test]
    fn the_raw_metric_family_carries_the_twenty_four_hour_raw_floor() {
        let policy = catalog_family(FAMILY_RAW_METRIC_SAMPLE).unwrap();
        assert_eq!(policy.class, PolicyClass::Raw);
        assert_eq!(policy.default_days * 24, MIN_INVESTIGATION_RAW_HOURS);
        assert_eq!(policy.safety_floor_days() * 24, MIN_INVESTIGATION_RAW_HOURS);
        assert!(validate_policy_days(FAMILY_RAW_METRIC_SAMPLE, 0).is_err());
        assert_eq!(validate_policy_days(FAMILY_RAW_METRIC_SAMPLE, 1), Ok(()));
        let tables = catalog_targets(FAMILY_RAW_METRIC_SAMPLE)
            .iter()
            .map(|target| target.table)
            .collect::<Vec<_>>();
        assert_eq!(tables, vec!["node_metric_samples", "host_metric_samples"]);
    }

    /// Issue #213: the raw window is expired by a cutoff on observed_at while
    /// the series ledger survives, so "the series began here" and "the Server
    /// holds samples from then" stay distinguishable facts.
    #[tokio::test]
    async fn expired_raw_metric_samples_are_released_and_the_series_ledger_survives() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let fresh = crate::auth::format_rfc3339(now - time::Duration::hours(1));
        let expired_at = now - time::Duration::hours(25);
        let expired = crate::auth::format_rfc3339(expired_at);
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('metric-agent', 1, ?, ?)")
            .bind(&fresh)
            .bind(&fresh)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('metric-network', 'Metric', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&fresh)
            .bind(&fresh)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('metric-node', 'metric-agent', 'metric-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&fresh)
            .bind(&fresh)
            .execute(pool)
            .await
            .unwrap();
        let insert = "INSERT INTO node_metric_samples (node_id, metric, observed_at, received_at, value) VALUES ('metric-node', 'process_cpu_percent', ?, ?, 1.0)";
        // The Node batch is sized for a whole maximal Report (see
        // NODE_METRIC_CLEANUP_BATCH), so this test proves the bound is applied
        // and then drains what is left over — not that 128 rows is the bound.
        for seconds in 0..(NODE_METRIC_CLEANUP_BATCH + 3) {
            let observed_at =
                crate::auth::format_rfc3339(expired_at - time::Duration::seconds(seconds));
            sqlx::query(insert)
                .bind(&observed_at)
                .bind(&observed_at)
                .execute(pool)
                .await
                .unwrap();
        }
        sqlx::query(insert)
            .bind(&fresh)
            .bind(&fresh)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_metric_series_state (node_id, metric, first_observed_at, last_observed_at, last_received_at, observation_count, replayed_count, corrected_count, updated_at) VALUES ('metric-node', 'process_cpu_percent', ?, ?, ?, 132, 7, 1, ?)")
            .bind(&expired)
            .bind(&fresh)
            .bind(&fresh)
            .bind(&fresh)
            .execute(pool)
            .await
            .unwrap();

        assert_eq!(
            cleanup_expired_metric_samples(pool, now).await.unwrap(),
            NODE_METRIC_CLEANUP_BATCH as u64
        );
        assert_eq!(cleanup_expired_metric_samples(pool, now).await.unwrap(), 3);
        assert_eq!(cleanup_expired_metric_samples(pool, now).await.unwrap(), 0);
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_metric_samples WHERE node_id = 'metric-node'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(
            remaining, 1,
            "the sample inside the raw window must survive cleanup"
        );
        let ledger: (String, i64, i64, i64) = sqlx::query_as(
            "SELECT first_observed_at, observation_count, replayed_count, corrected_count FROM node_metric_series_state WHERE node_id = 'metric-node' AND metric = 'process_cpu_percent'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(ledger.0, expired);
        assert_eq!((ledger.1, ledger.2, ledger.3), (132, 7, 1));
    }

    /// Issue #213: one accepted Report can store up to
    /// `MAX_NODE_OBSERVATIONS` x 5 Node metric rows, so the per-Report expiry
    /// bound has to cover that. A smaller bound than the arrival rate would let a
    /// maximal Agent's backlog grow even though every Report expires its own
    /// share, and the growth would end in the low-space protection that pauses
    /// the very history this family exists to keep.
    #[tokio::test]
    async fn one_cleanup_call_drains_a_maximal_reports_worth_of_node_samples() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let fresh = crate::auth::format_rfc3339(now - time::Duration::hours(1));
        let expired_at = now - time::Duration::hours(25);
        let maximum_report_rows = platpulse_core::protocol::MAX_NODE_OBSERVATIONS
            * crate::metric_history::NODE_METRIC_SERIES.len();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('batch-agent', 1, ?, ?)")
            .bind(&fresh)
            .bind(&fresh)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('batch-network', 'Batch', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&fresh)
            .bind(&fresh)
            .execute(&mut *tx)
            .await
            .unwrap();
        let mut inserted = 0;
        for node in 0..platpulse_core::protocol::MAX_NODE_OBSERVATIONS {
            let node_id = format!("batch-node-{node}");
            sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, 'batch-agent', 'batch-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
                .bind(&node_id)
                .bind(&fresh)
                .bind(&fresh)
                .execute(&mut *tx)
                .await
                .unwrap();
            for metric in crate::metric_history::NODE_METRIC_SERIES {
                let observed_at =
                    crate::auth::format_rfc3339(expired_at - time::Duration::seconds(inserted));
                sqlx::query("INSERT INTO node_metric_samples (node_id, metric, observed_at, received_at, value) VALUES (?, ?, ?, ?, 1.0)")
                    .bind(&node_id)
                    .bind(metric)
                    .bind(&observed_at)
                    .bind(&observed_at)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                inserted += 1;
            }
        }
        tx.commit().await.unwrap();
        assert_eq!(inserted as usize, maximum_report_rows);

        assert_eq!(
            cleanup_expired_metric_samples(pool, now).await.unwrap(),
            maximum_report_rows as u64,
            "one accepted Report may store this many Node samples"
        );
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM node_metric_samples")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(
            remaining, 0,
            "one cleanup call must keep up with one maximal Report"
        );
    }

    /// The statement carries the bound as a literal, so this test is what keeps
    /// the constant and the SQL in step.
    #[test]
    fn the_node_metric_batch_statement_carries_its_bound() {
        assert!(
            TARGET_RAW_METRIC_SAMPLES[0]
                .delete_sql
                .contains(&format!("LIMIT {NODE_METRIC_CLEANUP_BATCH})")),
            "the Node sample batch SQL must use NODE_METRIC_CLEANUP_BATCH: {}",
            TARGET_RAW_METRIC_SAMPLES[0].delete_sql
        );
    }

    /// The class floor is a property of the class, not of a family's declared
    /// minimum: even a family that declared no minimum cannot be edited below the
    /// investigation floor.
    #[test]
    fn class_floors_hold_regardless_of_a_family_minimum() {
        let no_minimum = PolicyDefaults {
            family: "synthetic",
            label: "Synthetic",
            default_days: 0,
            min_days: 0,
            max_days: 30,
            supported: true,
            class: PolicyClass::Raw,
            targets: NO_CLEANUP_TARGETS,
        };
        assert_eq!(no_minimum.safety_floor_days(), MIN_INVESTIGATION_RAW_DAYS);
        assert_eq!(
            no_minimum.class.floor_reference(),
            Some("24 hours of raw history, design §11.4")
        );
        assert_eq!(
            PolicyClass::Investigation.floor_days(),
            MIN_INVESTIGATION_AGGREGATE_DAYS
        );
        assert_eq!(PolicyClass::Contract.floor_days(), 0);
        // The two floors stay enforceable at the trust boundary.
        assert!(validate_policy_days(FAMILY_RAW_BLOCK_SUMMARY, 0).is_err());
        assert!(validate_policy_days(FAMILY_HISTORY_GAP, 30).is_err());
    }

    /// Issue #210, Stories 37/38/41: a preview freezes the policy version, the
    /// scope, and one cutoff per family, and a later policy edit turns it stale
    /// instead of letting a run delete against a plan nobody reviewed.
    #[tokio::test]
    async fn preview_freezes_policy_scope_and_cutoff_and_turns_stale_on_edit() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let preview = create_preview(
            pool,
            "owner-1",
            Some(vec![FAMILY_RAW_BLOCK_SUMMARY.to_owned()]),
            now,
        )
        .await
        .unwrap();
        assert_eq!(
            preview.scope,
            Some(vec![FAMILY_RAW_BLOCK_SUMMARY.to_owned()])
        );
        assert_eq!(preview.families.len(), 1);
        assert!(preview.skipped.is_empty());
        let family = preview.families[0].clone();
        let policy = catalog_family(FAMILY_RAW_BLOCK_SUMMARY).unwrap();
        assert_eq!(family.retention_days, policy.default_days);
        assert_eq!(
            family.cutoff,
            crate::auth::format_rfc3339(family_cutoff(now, policy.default_days))
        );
        let rows = list_policies(pool).await.unwrap();
        let row = rows
            .iter()
            .find(|row| row.family == FAMILY_RAW_BLOCK_SUMMARY)
            .unwrap();
        assert_eq!(
            family.policy_version,
            policy_version(&row.family, row.retention_days, &row.updated_at)
        );
        assert_eq!(
            family.targets.len(),
            catalog_targets(FAMILY_RAW_BLOCK_SUMMARY).len()
        );
        assert!(family.targets.iter().all(|target| target.total == 0));

        match load_preview_for_run(pool, &preview.preview_id, now)
            .await
            .unwrap()
        {
            PreviewLoad::Ready(loaded) => {
                assert_eq!(preview_plan(&loaded).len(), family.targets.len());
                assert_eq!(loaded.policy_version, preview.policy_version);
                assert_eq!(
                    latest_live_preview(pool, now)
                        .await
                        .unwrap()
                        .map(|live| live.preview_id),
                    Some(preview.preview_id.clone())
                );
            }
            other => panic!("expected a ready preview, got {other:?}"),
        }

        // An edit after the preview invalidates it: the run fails closed.
        update_policy(pool, FAMILY_RAW_BLOCK_SUMMARY, 14, "owner-1")
            .await
            .unwrap();
        match load_preview_for_run(pool, &preview.preview_id, now)
            .await
            .unwrap()
        {
            PreviewLoad::Stale(reasons) => assert!(
                reasons
                    .iter()
                    .any(|reason| reason.starts_with(FAMILY_RAW_BLOCK_SUMMARY)),
                "{reasons:?}"
            ),
            other => panic!("expected a stale preview, got {other:?}"),
        }
        assert!(latest_live_preview(pool, now).await.unwrap().is_none());
    }

    /// An unknown family is a request error, not a silent skip; families the
    /// preview will not act on are reported instead of quietly ignored.
    #[tokio::test]
    async fn preview_rejects_unknown_scopes_and_reports_skips() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = crate::auth::now_utc();
        match create_preview(pool, "owner-1", Some(vec!["nope".to_owned()]), now).await {
            Err(PreviewError::InvalidScope(message)) => assert!(message.contains("nope")),
            other => panic!("expected an invalid scope, got {other:?}"),
        }
        match create_preview(pool, "owner-1", Some(Vec::new()), now).await {
            Err(PreviewError::InvalidScope(message)) => assert!(message.contains("empty")),
            other => panic!("expected an invalid scope, got {other:?}"),
        }
        // The default scope takes every enabled, supported, bounded family.
        let preview = create_preview(pool, "owner-1", None, now).await.unwrap();
        assert!(preview.scope.is_none());
        assert!(preview.skipped.is_empty());
        assert!(
            preview
                .families
                .iter()
                .any(|family| family.family == FAMILY_RAW_BLOCK_SUMMARY),
            "{:?}",
            preview
                .families
                .iter()
                .map(|f| &f.family)
                .collect::<Vec<_>>()
        );
        // A requested family the Server cannot action is reported, never acted on.
        let preview = create_preview(
            pool,
            "owner-1",
            Some(vec![
                FAMILY_RAW_BLOCK_SUMMARY.to_owned(),
                FAMILY_ONE_HOUR_AGGREGATE.to_owned(),
            ]),
            now,
        )
        .await
        .unwrap();
        assert_eq!(preview.families.len(), 1);
        assert_eq!(preview.skipped.len(), 1);
        assert_eq!(preview.skipped[0].family, FAMILY_ONE_HOUR_AGGREGATE);
        assert_eq!(preview.skipped[0].code, "retention_unsupported");
    }

    /// An expired preview or an unknown id is never executable, so a run cannot
    /// silently re-plan itself.
    #[tokio::test]
    async fn expired_or_unknown_previews_are_never_executable() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = crate::auth::now_utc();
        let preview = create_preview(pool, "owner-1", None, now).await.unwrap();
        assert!(matches!(
            load_preview_for_run(pool, "rp-missing", now).await.unwrap(),
            PreviewLoad::NotFound
        ));
        let later = now + time::Duration::hours(PREVIEW_TTL_HOURS + 1);
        match load_preview_for_run(pool, &preview.preview_id, later)
            .await
            .unwrap()
        {
            PreviewLoad::Stale(reasons) => assert!(
                reasons.iter().any(|reason| reason.contains("expired")),
                "{reasons:?}"
            ),
            other => panic!("expected an expired preview, got {other:?}"),
        }
        assert!(latest_live_preview(pool, later).await.unwrap().is_none());
    }

    /// A bounded batch shorter than the batch bound completes its entry: the
    /// frozen cutoff has no expired row left, so a run never stalls waiting for
    /// rows that are gone — and it must not invent deleted rows either.
    #[test]
    fn a_short_batch_completes_its_entry_and_a_full_one_does_not() {
        let mut entry = PlanEntry {
            family: FAMILY_RAW_BLOCK_SUMMARY.to_owned(),
            table: "block_summaries".to_owned(),
            cutoff: "2026-01-01T00:00:00Z".to_owned(),
            total: 40,
            deleted: 0,
            done: false,
        };
        apply_batch(&mut entry, RETENTION_BATCH as u64);
        assert_eq!(entry.deleted, RETENTION_BATCH);
        assert!(!entry.done, "a full batch may still have work behind it");
        apply_batch(&mut entry, RETENTION_BATCH as u64);
        assert!(!entry.done);
        assert_eq!(
            entry.deleted,
            RETENTION_BATCH * 2,
            "the preview estimate is an upper bound, never a cap on what the frozen cutoff releases"
        );
        apply_batch(&mut entry, 3);
        assert!(entry.done, "a short batch completes the entry");
        assert_eq!(entry.deleted, RETENTION_BATCH * 2 + 3);
        apply_batch(&mut entry, 0);
        assert_eq!(
            entry.deleted,
            RETENTION_BATCH * 2 + 3,
            "a released-nothing batch must not invent deletions"
        );
    }

    /// The preview's estimate is an upper bound, never a work quota: a run keeps
    /// releasing everything behind the frozen cutoff even when the stored plan
    /// undercounted it, and it reports what it actually released (issue #210:
    /// the estimated row count is not frozen).
    #[tokio::test]
    async fn a_run_releases_what_the_frozen_cutoff_covers_not_what_the_estimate_said() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = crate::auth::now_utc();
        let now_text = crate::auth::format_rfc3339(now);
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('estimate-network', 'Estimate', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('estimate-agent', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('estimate-node', 'estimate-agent', 'estimate-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        let insert = "INSERT INTO peer_presence_intervals (node_id, peer_id, direction, trusted, static_peer, consensus_peer, client_name, opened_at, closed_at) VALUES ('estimate-node', ?, 'inbound', 1, 0, 1, 'PlatON/v1.5.1', ?, ?)";
        for peer in ["peer-a", "peer-b", "peer-c"] {
            sqlx::query(insert)
                .bind(peer)
                .bind("2020-01-01T00:00:00Z")
                .bind("2020-01-02T00:00:00Z")
                .execute(pool)
                .await
                .unwrap();
        }
        ensure_seeded(pool).await.unwrap();
        let preview = create_preview(
            pool,
            "owner-1",
            Some(vec![FAMILY_PEER_PRESENCE_INTERVAL.to_owned()]),
            now,
        )
        .await
        .unwrap();
        assert_eq!(preview.estimated_rows, 3);
        let operation_id = "estimate-is-not-a-cap-operation";
        queue_retention_run(
            pool,
            operation_id,
            "estimate-is-not-a-cap-request",
            &preview,
            &now_text,
        )
        .await;
        // Simulate a preview whose estimate undercounted the frozen cutoff.
        let mut plan = serde_json::to_value(preview_plan(&preview)).unwrap();
        for entry in plan.as_array_mut().unwrap() {
            entry["total"] = serde_json::json!(1);
        }
        let params = serde_json::json!({
            "previewId": preview.preview_id,
            "previewPolicyVersion": preview.policy_version,
            "skippedWarnings": preview.skipped,
            "plan": plan,
        });
        sqlx::query("UPDATE operations SET params_json = ? WHERE operation_id = ?")
            .bind(params.to_string())
            .bind(operation_id)
            .execute(pool)
            .await
            .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let state = AppState::new(
            database,
            None,
            crate::auth::AuthConfig::development(
                crate::secrets::load_pepper_file(&pepper_path).unwrap(),
                "http://127.0.0.1:8080".to_owned(),
            ),
        );

        execute_step(&state, operation_id).await.unwrap();
        execute_step(&state, operation_id).await.unwrap();

        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM operations WHERE operation_id = ?")
                .bind(operation_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            crate::operations::STATUS_SUCCEEDED
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE closed_at IS NOT NULL AND closed_at < '2026-01-01T00:00:00Z'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "every row behind the frozen cutoff is released, not just the estimated one"
        );
        let result: Option<String> =
            sqlx::query_scalar("SELECT result_json FROM operations WHERE operation_id = ?")
                .bind(operation_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        let result: serde_json::Value = serde_json::from_str(result.as_deref().unwrap()).unwrap();
        assert_eq!(result["families"][0]["deletedRows"], serde_json::json!(3));
        assert_eq!(result["families"][0]["estimatedRows"], serde_json::json!(1));
    }

    /// A run whose stored plan is missing fails closed instead of reporting
    /// success.
    #[tokio::test]
    async fn a_release_and_the_accounting_of_it_commit_together() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        let now = crate::auth::now_utc();
        let now_text = crate::auth::format_rfc3339(now);
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('atomic-network', 'Atomic', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('atomic-agent', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('atomic-node', 'atomic-agent', 'atomic-network', 'ws://127.0.0.1:1', 'active', 'private', 1, ?, ?)")
            .bind(&now_text)
            .bind(&now_text)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO peer_presence_intervals (node_id, peer_id, direction, trusted, static_peer, consensus_peer, client_name, opened_at, closed_at) VALUES ('atomic-node', 'atomic-old', 'inbound', 1, 0, 1, 'PlatON/v1.5.1', '2020-01-01T00:00:00Z', '2020-01-02T00:00:00Z')")
            .execute(pool)
            .await
            .unwrap();
        ensure_seeded(pool).await.unwrap();
        let preview = create_preview(
            pool,
            "owner-1",
            Some(vec![FAMILY_PEER_PRESENCE_INTERVAL.to_owned()]),
            now,
        )
        .await
        .unwrap();
        let operation_id = "atomic-accounting-operation";
        queue_retention_run(
            pool,
            operation_id,
            "atomic-accounting-request",
            &preview,
            &now_text,
        )
        .await;
        // The only params write left in this run is the batch accounting, so the
        // injected fault lands after the deletion and before its record.
        let mut params: Value = serde_json::from_str(
            &sqlx::query_scalar::<_, String>(
                "SELECT params_json FROM operations WHERE operation_id = ?",
            )
            .bind(operation_id)
            .fetch_one(pool)
            .await
            .unwrap(),
        )
        .unwrap();
        params["warningsRecorded"] = Value::Bool(true);
        sqlx::query("UPDATE operations SET params_json = ? WHERE operation_id = ?")
            .bind(serde_json::to_string(&params).unwrap())
            .bind(operation_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER refuse_plan_accounting BEFORE UPDATE OF params_json ON operations BEGIN SELECT RAISE(ABORT, 'accounting unavailable'); END")
            .execute(pool)
            .await
            .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let state = AppState::new(
            database,
            None,
            crate::auth::AuthConfig::development(
                crate::secrets::load_pepper_file(&pepper_path).unwrap(),
                "http://127.0.0.1:8080".to_owned(),
            ),
        );

        let error = execute_step(&state, operation_id)
            .await
            .expect_err("the accounting write must fail");
        assert!(
            format!("{error:?}").contains("accounting unavailable"),
            "unexpected error: {error:?}"
        );
        let survived: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM peer_presence_intervals WHERE peer_id = 'atomic-old'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            survived, 1,
            "a release whose accounting failed is rolled back with it, so the record never under-reports what was released"
        );

        // Once the accounting write works again the same row is still there to
        // be released, and the run reports it honestly.
        sqlx::query("DROP TRIGGER refuse_plan_accounting")
            .execute(state.db().pool())
            .await
            .unwrap();
        execute_step(&state, operation_id).await.unwrap();
        execute_step(&state, operation_id).await.unwrap();
        let (status, deleted): (String, i64) = (
            sqlx::query_scalar("SELECT status FROM operations WHERE operation_id = ?")
                .bind(operation_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE peer_id = 'atomic-old'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
        );
        assert_eq!(status, crate::operations::STATUS_SUCCEEDED);
        assert_eq!(deleted, 0, "the retried batch releases the same row");
    }

    #[tokio::test]
    async fn a_run_without_a_stored_plan_fails_closed() {
        let dir = tempfile::TempDir::new().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool();
        sqlx::query("INSERT INTO operations (operation_id, kind, status, request_id, params_json, warnings_json, errors_json, created_at) VALUES ('empty-plan-operation', 'retention_run', 'queued', 'empty-plan-request', '{}', '[]', '[]', ?)")
            .bind(crate::auth::format_rfc3339(crate::auth::now_utc()))
            .execute(pool)
            .await
            .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let state = AppState::new(
            database,
            None,
            crate::auth::AuthConfig::development(
                crate::secrets::load_pepper_file(&pepper_path).unwrap(),
                "http://127.0.0.1:8080".to_owned(),
            ),
        );
        execute_step(&state, "empty-plan-operation").await.unwrap();
        let (status, errors): (String, String) = sqlx::query_as(
            "SELECT status, errors_json FROM operations WHERE operation_id='empty-plan-operation'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, crate::operations::STATUS_FAILED);
        assert!(errors.contains("retention_plan_missing"), "{errors}");
    }
}
