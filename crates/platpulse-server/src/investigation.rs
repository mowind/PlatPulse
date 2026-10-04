//! The investigation coordinate: one UTC window, and how much of it each
//! evidence family can actually answer (issues #220, #221).
//!
//! An investigation starts from a subject (a Node) and one window, and asks
//! every family of evidence the Server holds for that Node the same question:
//! *is this window answerable, and by what?* The module owns the window itself
//! (its presets, its accepted range, and the instant the Server answered at) and
//! the per-source coverage coordinate. It deliberately owns no evidence: the
//! metric, state, peer, incident and Validator read paths stay where they are,
//! and this module hands the caller the exact paths and coordinates to ask them
//! with. Inventing a second metric reader here would be a second answer to the
//! same question, and the two would drift.
//!
//! Two rules shape every answer below, and they come straight from the design
//! (§5.1, §11.6, issue #213/#214/#217):
//!
//! * Unknown is never zero. A source with no evidence says so and names why —
//!   never observed, released by cleanup, before the Agent was enabled, or
//!   paused for low space — instead of reporting an empty, healthy-looking
//!   window.
//! * The window is answered as asked. The Server never widens, narrows or clamps
//!   the requested range to what its retention happens to hold: it reports the
//!   range it was given and states, per source, the boundary beyond which it
//!   holds nothing.
//!
//! A third rule says who the evidence is about (issue #221): a source answers
//! about its own subject, and evidence belonging to a subject the Node was
//! related to is attributed only over the intervals the Server itself recorded,
//! labelled as a related subject, and named as unknown for every stretch no record
//! covers. The relation a Node has now is never read backwards into the window.

use platpulse_core::component::ComponentKey;
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};
use time::{Duration, OffsetDateTime};

use crate::auth::format_rfc3339;
use crate::http::report_ingestion::component_storage_key;
use crate::metric_history::{
    FIVE_MINUTE_MAX_AGE_DAYS, FIVE_MINUTE_SECONDS, HOST_HISTORY, HOST_METRIC_SERIES, HistorySchema,
    NODE_HISTORY, NODE_METRIC_SERIES, ONE_MINUTE_MAX_AGE_DAYS, ONE_MINUTE_SECONDS,
    canonical_instant, gap_threshold_seconds, raw_window_cutoff,
};
use crate::relationships::{
    RELATION_AGENT, RELATION_KINDS, RecordedRelations, RelationSpan, RelationView,
    RelationshipInterval, clipped_span, describe_span, load_intervals, relation_kind_label,
};
use crate::retention::{
    FAMILY_FIVE_MINUTE_AGGREGATE, FAMILY_OBSERVATION_STATE, FAMILY_PEER_AGGREGATE_1H,
    FAMILY_PEER_AGGREGATE_5M, FAMILY_VALIDATOR_DAILY_SNAPSHOT,
};

// ----------------------------------------------------------------- window ---

/// One selectable investigation window, as the Server itself defines it.
///
/// The list is served with every answer so a client never hardcodes it: a
/// deployment whose horizon changes must not leave a stale preset behind in the
/// WebUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowPreset {
    /// The value the `window` query parameter accepts.
    pub key: &'static str,
    /// How far back the preset reaches, counted from the answering instant.
    pub hours: i64,
    /// The human label for the preset.
    pub label: &'static str,
}

/// The selectable windows, narrowest first.
pub const WINDOW_PRESETS: [WindowPreset; 5] = [
    WindowPreset {
        key: "1h",
        hours: 1,
        label: "Last hour",
    },
    WindowPreset {
        key: "6h",
        hours: 6,
        label: "Last 6 hours",
    },
    WindowPreset {
        key: "24h",
        hours: 24,
        label: "Last 24 hours",
    },
    WindowPreset {
        key: "7d",
        hours: 168,
        label: "Last 7 days",
    },
    WindowPreset {
        key: "30d",
        hours: 720,
        label: "Last 30 days",
    },
];

/// The preset an investigation uses when the caller names none (24 hours).
pub const DEFAULT_WINDOW_PRESET: &str = "24h";

/// The narrowest window an investigation accepts, in hours.
pub const MIN_WINDOW_HOURS: i64 = 1;

/// The widest window an investigation accepts, in hours (30 days).
pub const MAX_WINDOW_HOURS: i64 = 720;

/// The preset key reported for a window the caller named with `from`/`to`.
pub const CUSTOM_WINDOW_PRESET: &str = "custom";

/// The error code a rejected window answer carries.
pub const INVALID_WINDOW_CODE: &str = "invalid_investigation_window";

/// How long the raw metric tier reaches back before the aggregate tiers take
/// over (design §11.6; `metric_history::RAW_WINDOW_HOURS`).
pub const RAW_RETENTION_DAYS: i64 = 1;

/// The longest interruption the Server still calls continuous, in seconds, when
/// the metric tiers carry no measured cadence of their own. The value matches
/// the metric read path's own floor (`MIN_GAP_SECONDS`), so one silence is not
/// a gap in one surface and continuity in another.
pub const FALLBACK_GAP_SECONDS: i64 = 120;

/// A window resolved against the Server's own clock.
///
/// `requested_*` is what the caller asked for; `from`/`to` are what the
/// Server answers over. They differ in one case only: a custom range that ends
/// in the future is clipped to the answering instant, and `clamped_to_now`
/// records that so the answer never claims to cover time that has not happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWindow {
    /// The preset key, or `custom`.
    pub preset: String,
    /// The range the caller asked for, before clipping.
    pub requested_from: OffsetDateTime,
    /// The range the caller asked for, before clipping.
    pub requested_to: OffsetDateTime,
    /// The start of the range the Server answers over.
    pub from: OffsetDateTime,
    /// The end of the range the Server answers over.
    pub to: OffsetDateTime,
    /// The instant the Server resolved the window at.
    pub answered_at: OffsetDateTime,
    /// Whether `to` was clipped to `answered_at`.
    pub clamped_to_now: bool,
}

impl ResolvedWindow {
    /// The width of the answered range in seconds.
    pub fn duration_seconds(&self) -> i64 {
        (self.to - self.from).whole_seconds()
    }

    /// Whether the caller named the range explicitly rather than a preset.
    pub fn is_custom(&self) -> bool {
        self.preset == CUSTOM_WINDOW_PRESET
    }

    /// The answered start as a canonical instant.
    pub fn from_rfc3339(&self) -> String {
        format_rfc3339(self.from)
    }

    /// The answered end as a canonical instant.
    pub fn to_rfc3339(&self) -> String {
        format_rfc3339(self.to)
    }

    /// The preset label shown beside the window.
    pub fn preset_label(&self) -> &'static str {
        preset_label(&self.preset)
    }

    /// The window as the answer reports it.
    ///
    /// Every bound the answer applied is stated here rather than left for a
    /// client to assume: the answered range, whether it was clipped, the widest
    /// window the Server accepts, how far the raw tier reaches, and the presets
    /// themselves (design §15.16, story 54).
    pub fn to_response(&self) -> InvestigationWindowResponse {
        InvestigationWindowResponse {
            preset: self.preset.clone(),
            preset_label: self.preset_label().to_owned(),
            custom: self.is_custom(),
            requested_from: format_rfc3339(self.requested_from),
            requested_to: format_rfc3339(self.requested_to),
            from: self.from_rfc3339(),
            to: self.to_rfc3339(),
            duration_seconds: self.duration_seconds(),
            answered_at: format_rfc3339(self.answered_at),
            clamped_to_now: self.clamped_to_now,
            horizon_days: MAX_WINDOW_HOURS / 24,
            raw_retention_days: RAW_RETENTION_DAYS,
            supported_presets: WINDOW_PRESETS
                .iter()
                .map(|preset| InvestigationPresetResponse {
                    key: preset.key.to_owned(),
                    label: preset.label.to_owned(),
                    hours: preset.hours,
                })
                .collect(),
        }
    }
}

/// The label of a preset key (`custom` included).
pub fn preset_label(key: &str) -> &'static str {
    if key == CUSTOM_WINDOW_PRESET {
        return "Custom range";
    }
    WINDOW_PRESETS
        .iter()
        .find(|preset| preset.key == key)
        .map(|preset| preset.label)
        .unwrap_or("Custom range")
}

/// The hours a preset key stands for.
pub fn preset_hours(key: &str) -> Option<i64> {
    WINDOW_PRESETS
        .iter()
        .find(|preset| preset.key == key)
        .map(|preset| preset.hours)
}

/// The accepted preset keys, comma separated, for an error message.
fn preset_keys() -> String {
    WINDOW_PRESETS
        .iter()
        .map(|preset| format!("{} ({})", preset.key, preset.label.to_lowercase()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve the requested investigation window against the Server's clock.
///
/// `window` names a preset and `from`/`to` name an explicit range; the two
/// forms are mutually exclusive, and the explicit form needs both ends. Every
/// rejection carries its own message because each one describes a different
/// mistake: a caller that passed `from` without `to` did not pass a window that
/// is too wide.
pub fn resolve_window(
    window: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    now: OffsetDateTime,
) -> Result<ResolvedWindow, String> {
    let window = window.map(str::trim).filter(|value| !value.is_empty());
    let from = from.map(str::trim).filter(|value| !value.is_empty());
    let to = to.map(str::trim).filter(|value| !value.is_empty());

    // An investigation opened without naming a stretch gets the adjustable
    // default window: the endpoint owns one clock, and making the caller repeat
    // the default would put the constant in the WebUI instead of the Server
    // (issue #220, story 54).
    let window = match (window, from, to) {
        (None, None, None) => Some(DEFAULT_WINDOW_PRESET),
        _ => window,
    };

    match (window, from, to) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            Err("choose either a preset window or an explicit from/to range, not both".to_owned())
        }
        (Some(key), None, None) => {
            let hours = preset_hours(key).ok_or_else(|| {
                format!(
                    "{key} is not a supported window; supported windows are {}",
                    preset_keys()
                )
            })?;
            let from = now - time::Duration::hours(hours);
            Ok(ResolvedWindow {
                preset: key.to_owned(),
                requested_from: from,
                requested_to: now,
                from,
                to: now,
                answered_at: now,
                clamped_to_now: false,
            })
        }
        (None, Some(from_value), Some(to_value)) => {
            let requested_from = canonical_instant(from_value)
                .ok_or_else(|| unusable_instant("from", from_value))?;
            let requested_to =
                canonical_instant(to_value).ok_or_else(|| unusable_instant("to", to_value))?;
            if requested_from >= requested_to {
                return Err(
                    "the window ends at or before it starts: from must be an earlier instant than to"
                        .to_owned(),
                );
            }
            let width = (requested_to - requested_from).whole_seconds();
            if width < MIN_WINDOW_HOURS * 3600 {
                return Err(format!(
                    "the window is {width} seconds wide; the supported minimum is {MIN_WINDOW_HOURS} hour"
                ));
            }
            if width > MAX_WINDOW_HOURS * 3600 {
                return Err(format!(
                    "the window is {width} seconds wide; the supported maximum is {MAX_WINDOW_HOURS} hours"
                ));
            }
            let clamped = requested_to > now;
            let to = if clamped { now } else { requested_to };
            if requested_from >= to {
                return Err(
                    "the window lies in the future, so the Server cannot have observed any of it"
                        .to_owned(),
                );
            }
            Ok(ResolvedWindow {
                preset: CUSTOM_WINDOW_PRESET.to_owned(),
                requested_from,
                requested_to,
                from: requested_from,
                to,
                answered_at: now,
                clamped_to_now: clamped,
            })
        }
        (None, _, _) => Err(
            "an explicit window needs both from and to: one end alone does not describe a range"
                .to_owned(),
        ),
    }
}

/// The message for an instant the Server cannot use as a range end.
fn unusable_instant(field: &str, value: &str) -> String {
    format!(
        "{field}={value} is not a canonical RFC 3339 instant; pass a UTC instant such as 2030-01-01T00:00:00Z"
    )
}

// ------------------------------------------------------------- vocabulary ---

/// How much of the window a source can answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Every tier that serves the window answered it, without a boundary.
    Complete,
    /// The source answered, and at least part of the window is unreachable.
    Partial,
    /// The source applies and the window holds no evidence at all.
    Empty,
    /// The Server holds no evidence for the window and never will.
    Unavailable,
    /// The source does not apply to this subject at all.
    Unsupported,
}

impl Coverage {
    /// The stable wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Coverage::Complete => "complete",
            Coverage::Partial => "partial",
            Coverage::Empty => "empty",
            Coverage::Unavailable => "unavailable",
            Coverage::Unsupported => "unsupported",
        }
    }

    /// The label shown beside the value.
    pub fn label(self) -> &'static str {
        match self {
            Coverage::Complete => "Complete",
            Coverage::Partial => "Partial",
            Coverage::Empty => "Empty window",
            Coverage::Unavailable => "Unavailable",
            Coverage::Unsupported => "Not applicable",
        }
    }
}

/// The clock a source's timestamps belong to.
///
/// The four clocks of an investigation are not interchangeable (design §11.4):
/// a metric observation time is the Agent's clock, a peer bucket is the Server's
/// receipt clock, an Incident is judged at evaluation time, and a Validator
/// snapshot carries a third party's own timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeBasis {
    /// The instant the Agent observed a metric.
    MetricObservation,
    /// The instant the Agent observed a state vector.
    StateObservation,
    /// The Server's receipt-aligned peer bucket.
    PeerReceiptBucket,
    /// The instant an Incident occurred, as the evaluator judged it.
    IncidentEvaluation,
    /// The third-party timestamp a Validator snapshot carries.
    ValidatorSnapshotSource,
    /// The instant the Server wrote a record of its own, such as a relationship.
    ServerRecord,
}

impl TimeBasis {
    /// The stable wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            TimeBasis::MetricObservation => "metric_observation",
            TimeBasis::StateObservation => "state_observation",
            TimeBasis::PeerReceiptBucket => "peer_receipt_bucket",
            TimeBasis::IncidentEvaluation => "incident_evaluation",
            TimeBasis::ValidatorSnapshotSource => "validator_snapshot_source",
            TimeBasis::ServerRecord => "server_record",
        }
    }

    /// The label shown beside the value.
    pub fn label(self) -> &'static str {
        match self {
            TimeBasis::MetricObservation => "Metric observation time",
            TimeBasis::StateObservation => "State observation time",
            TimeBasis::PeerReceiptBucket => "Peer receipt bucket",
            TimeBasis::IncidentEvaluation => "Incident occurrence time",
            TimeBasis::ValidatorSnapshotSource => "Validator source time",
            TimeBasis::ServerRecord => "Server record time",
        }
    }
}

/// Why part of a window cannot be answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryKind {
    /// The Agent was not reporting for this source yet.
    PreEnablement,
    /// Cleanup released the evidence that covered this stretch.
    RetentionCleanup,
    /// Optional history was refused while protection paused collection.
    LowSpacePause,
    /// The Agent was reporting, and this stretch holds no observation.
    CollectionFailure,
    /// The Server has never held a single observation for this source.
    NeverObserved,
    /// The newest evidence is older than the end of the window.
    StaleTail,
    /// The Server holds no record of which subject this stretch belonged to.
    RelationshipUnknown,
}

impl BoundaryKind {
    /// The stable wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            BoundaryKind::PreEnablement => "pre_enablement",
            BoundaryKind::RetentionCleanup => "retention_cleanup",
            BoundaryKind::LowSpacePause => "low_space_pause",
            BoundaryKind::CollectionFailure => "collection_failure",
            BoundaryKind::NeverObserved => "never_observed",
            BoundaryKind::StaleTail => "stale_tail",
            BoundaryKind::RelationshipUnknown => "relationship_unknown",
        }
    }

    /// The label shown beside the value.
    pub fn label(self) -> &'static str {
        match self {
            BoundaryKind::PreEnablement => "Before this evidence started",
            BoundaryKind::RetentionCleanup => "Released by retention cleanup",
            BoundaryKind::LowSpacePause => "Optional history paused for low space",
            BoundaryKind::CollectionFailure => "Collection gap",
            BoundaryKind::NeverObserved => "Never observed",
            BoundaryKind::StaleTail => "Newest evidence is older than the window end",
            BoundaryKind::RelationshipUnknown => "No recorded relationship",
        }
    }
}

/// The grain a source's own evidence is stored at.
pub const GRAIN_RAW: &str = "raw";
/// The one-minute metric tier.
pub const GRAIN_ONE_MINUTE: &str = "1m";
/// The five-minute metric tier.
pub const GRAIN_FIVE_MINUTE: &str = "5m";
/// State-log entries (change or hourly anchor).
pub const GRAIN_STATE_ENTRY: &str = "state_entry";
/// Five-minute peer receipt buckets.
pub const GRAIN_PEER_5M: &str = "receipt_bucket_5m";
/// Hourly peer receipt buckets.
pub const GRAIN_PEER_1H: &str = "receipt_bucket_1h";
/// Alert Incident occurrences.
pub const GRAIN_OCCURRENCE: &str = "occurrence";
/// Validator local days.
pub const GRAIN_VALIDATOR_DAY: &str = "validator_day";
/// Recorded relationship intervals: one point per interval, with no bucket width.
pub const GRAIN_RECORDED_INTERVAL: &str = "recorded_interval";

/// The fixed source keys, in the order an investigation answers them.
pub const SOURCE_NODE_METRICS: &str = "node_metrics";
/// The Node's synchronization and consensus state log.
pub const SOURCE_NODE_STATE: &str = "node_state";
/// The Agent's Host observations.
pub const SOURCE_HOST_METRICS: &str = "host_metrics";
/// Peer receipt buckets.
pub const SOURCE_PEERS: &str = "peers";
/// The relationships the Server itself recorded for the Node.
pub const SOURCE_RELATIONSHIPS: &str = "relationships";
/// Alert Incidents whose occurrence intersects the window.
pub const SOURCE_INCIDENTS: &str = "incidents";
/// Daily Validator snapshots for the Validators linked in the window.
pub const SOURCE_VALIDATOR: &str = "validator";

/// The source keys an answer always carries, whether or not each holds evidence.
pub const SOURCE_ORDER: [&str; 7] = [
    SOURCE_RELATIONSHIPS,
    SOURCE_NODE_METRICS,
    SOURCE_NODE_STATE,
    SOURCE_HOST_METRICS,
    SOURCE_PEERS,
    SOURCE_INCIDENTS,
    SOURCE_VALIDATOR,
];

/// An attribution is about the subject the source itself answers about.
pub const SUBJECT_ROLE_SOURCE: &str = "source";
/// An attribution is about a subject the source's subject is related to.
pub const SUBJECT_ROLE_RELATED: &str = "related";
/// The attribution rests on an interval the Server recorded itself.
pub const BASIS_RECORDED_RELATIONSHIP: &str = "recorded_relationship";
/// The attribution rests on a Node Validator Link the Server recorded itself.
pub const BASIS_RECORDED_VALIDATOR_LINK: &str = "recorded_validator_link";

/// The label shown beside an attribution's basis.
pub fn basis_label(basis: &str) -> &'static str {
    match basis {
        BASIS_RECORDED_VALIDATOR_LINK => "Recorded Validator Link",
        _ => "Recorded relationship",
    }
}

// ------------------------------------------------------------------- DTOs ---

/// One selectable window, as reported to a client.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationPresetResponse {
    /// The value the `window` query parameter accepts.
    pub key: String,
    /// The preset's human label.
    pub label: String,
    /// How far back the preset reaches, in hours.
    pub hours: i64,
}

/// The window an investigation answered over.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationWindowResponse {
    /// The preset key the answer used, or `custom`.
    pub preset: String,
    /// The label of that preset.
    pub preset_label: String,
    /// Whether the caller named the range explicitly rather than a preset.
    pub custom: bool,
    /// The range the caller asked for, before any clipping.
    pub requested_from: String,
    /// The range the caller asked for, before any clipping.
    pub requested_to: String,
    /// The start of the range the Server answered over.
    pub from: String,
    /// The end of the range the Server answered over.
    pub to: String,
    /// The width of the answered range, in seconds.
    pub duration_seconds: i64,
    /// The instant the Server resolved the window at.
    pub answered_at: String,
    /// Whether `to` was clipped to `answered_at` because the range ended in the future.
    pub clamped_to_now: bool,
    /// The widest window an investigation accepts, in days.
    pub horizon_days: i64,
    /// How long the raw metric tier reaches back, in days.
    pub raw_retention_days: i64,
    /// Every window a client may ask for, so it never hardcodes the list.
    pub supported_presets: Vec<InvestigationPresetResponse>,
}

/// One stretch of the window a source cannot answer, and why.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationBoundaryResponse {
    /// The machine-readable boundary kind.
    pub kind: String,
    /// The label shown beside the kind.
    pub kind_label: String,
    /// The instant the boundary starts at.
    pub at: String,
    /// The instant the boundary ends at, when it ends inside the window.
    pub to: Option<String>,
    /// Why this stretch cannot be answered.
    pub detail: String,
}

/// One located interruption in a grain's evidence.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationHoleResponse {
    /// The series the interruption was measured on, when the grain holds many.
    pub series: Option<String>,
    /// The instant the interruption starts after.
    pub from: String,
    /// The instant the interruption ends at, where evidence resumes.
    pub to: String,
    /// How long the interruption lasted, in seconds.
    pub seconds: Option<i64>,
}

/// What one grain of a source can answer for the window.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationGrainResponse {
    /// The grain's name (for example `raw`, `1m`, `5m`, `validator_day`).
    pub grain: String,
    /// The bucket width, when the grain has one.
    pub grain_seconds: Option<i64>,
    /// Whether this grain holds any point inside the window.
    ///
    /// A grain that serves the window but stored nothing is not available: an
    /// empty grain and a grain that holds an empty stretch are different
    /// statements, and only the counts distinguish them.
    pub available: bool,
    /// The stretch of the window this grain serves, when it serves any.
    pub from: Option<String>,
    /// The stretch of the window this grain serves, when it serves any.
    pub to: Option<String>,
    /// Why the grain is unavailable, or how to read its counts.
    pub note: Option<String>,
    /// How many stored points of this grain fall inside its stretch.
    pub point_count: i64,
    /// How many observations those points aggregate.
    pub sample_count: i64,
    /// How many points an unbroken grain would hold, when that is knowable.
    pub expected_points: Option<i64>,
    /// How many expected points are missing, when that is knowable.
    pub missing_points: Option<i64>,
    /// The oldest point of this grain inside its stretch.
    pub first_observed_at: Option<String>,
    /// The newest point of this grain inside its stretch.
    pub last_observed_at: Option<String>,
    /// The fastest interval the grain actually showed, in seconds.
    pub cadence_seconds: Option<i64>,
    /// The longest interval between two adjacent points, in seconds.
    pub longest_gap_seconds: Option<i64>,
    /// The interruptions this grain proves, longest first (at most one per tier).
    pub holes: Vec<InvestigationHoleResponse>,
}

/// One collector row the Server holds for the investigated subject.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationComponentResponse {
    /// The scope the collector was recorded in (`node` or `host`).
    pub scope: String,
    /// The scope's own key (the Node id, or `host` for the Host scope).
    pub scope_key: String,
    /// The collector's key as the Server stores it.
    pub component_key: String,
    /// The last collection state (`ok`, `error`, `disabled`, …).
    pub state: String,
    /// The error code the last failed collection carried.
    pub error_code: Option<String>,
    /// The instant the Agent attempted the last collection.
    pub observed_at: Option<String>,
    /// The instant the Server received the last collection.
    pub received_at: Option<String>,
}

/// Where the evidence behind a source is answered by the Server.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationAnswerPathResponse {
    /// What the endpoint answers.
    pub label: String,
    /// The request path, including the window this investigation was resolved over.
    pub path: String,
    /// How to read the response, when it needs saying.
    pub note: Option<String>,
}

/// One subject a source's evidence is attributed to, and the Server record that
/// attribution rests on.
///
/// A source answers about its own subject, but some of its evidence belongs to a
/// subject the Node is related to (the Agent that reported it, its Host, its
/// Network, a linked Validator). Each such attribution states which subject it is,
/// whether that subject is the source's own or a related one, and the recorded
/// basis (the extent of a recorded relationship or Validator link) it is read
/// over. Nothing is attributed outside a record: a stretch the Server never
/// recorded is reported as an unknown boundary instead (stories 60, 61).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationRelatedSubjectResponse {
    /// The kind of the related subject (`agent`, `host`, `network`, `validator`).
    pub subject_kind: String,
    /// The related subject's own key, as the Server records it.
    pub subject: String,
    /// Whether this is the source's own subject (`source`) or a related one (`related`).
    pub role: String,
    /// The machine-readable basis of the attribution.
    pub basis: String,
    /// The label shown beside the basis.
    pub basis_label: String,
    /// The instant the recorded basis starts at, clipped to the window.
    pub from: String,
    /// The instant the recorded basis ends at, clipped to the window.
    pub to: String,
    /// What the record says, and how far it may be read.
    pub detail: String,
}

/// A family of evidence and how much of the window it can answer.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationSourceResponse {
    /// The stable source key.
    pub key: String,
    /// The source's human label.
    pub label: String,
    /// The kind of subject this source describes.
    pub subject_kind: String,
    /// The subject's identifier.
    pub subject: String,
    /// The machine-readable coverage verdict.
    pub coverage: String,
    /// The label shown beside the verdict.
    pub coverage_label: String,
    /// The clock this source's timestamps belong to.
    pub time_basis: String,
    /// The label shown beside the clock.
    pub time_basis_label: String,
    /// The collection state the Server last recorded for this source, when it records one.
    pub source_state: Option<String>,
    /// The error code the last failed collection carried, when there was one.
    pub error_code: Option<String>,
    /// The oldest evidence the Server still holds for this source.
    pub first_observed_at: Option<String>,
    /// The newest evidence the Server holds for this source.
    pub last_observed_at: Option<String>,
    /// The newest receipt of that evidence.
    pub last_received_at: Option<String>,
    /// The cleanup cutoff the newest release of this source's evidence used.
    pub released_before: Option<String>,
    /// The instant beyond which this source's cleanup cannot answer, when it has one.
    pub retained_from: Option<String>,
    /// The retention this source's oldest served grain is declared with, in days.
    pub retention_days: Option<i64>,
    /// Whether the answer omits detail the Server holds (for example further holes).
    pub truncated: bool,
    /// The stretches of the window this source cannot answer.
    pub boundaries: Vec<InvestigationBoundaryResponse>,
    /// What each grain of this source can answer.
    pub grains: Vec<InvestigationGrainResponse>,
    /// The collectors behind this source, with their last recorded state.
    pub components: Vec<InvestigationComponentResponse>,
    /// The existing endpoints that answer this source's evidence for the window.
    pub answer_paths: Vec<InvestigationAnswerPathResponse>,
    /// The subjects this source's evidence is attributed to, each with the recorded
    /// basis it is read over. Empty when the source answers only about itself and no
    /// subject was recorded as related to it.
    pub related_subjects: Vec<InvestigationRelatedSubjectResponse>,
    /// Disclosures that shape how the numbers above must be read.
    pub notes: Vec<String>,
}

/// The investigation coordinate for one Node and one window.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationResponse {
    /// The investigated Node.
    pub node_id: String,
    /// The Agent that reports this Node.
    pub agent_id: String,
    /// The Node's display name, when the Owner set one.
    pub display_name: Option<String>,
    /// The Node's lifecycle.
    pub lifecycle: String,
    /// The Node's visibility.
    pub visibility: String,
    /// The window the answer was resolved over.
    pub window: InvestigationWindowResponse,
    /// Every evidence family, in a fixed order, whether or not it holds evidence.
    pub sources: Vec<InvestigationSourceResponse>,
    /// Every collector row the Server holds for this Node and its Agent.
    pub components: Vec<InvestigationComponentResponse>,
    /// Disclosures that shape how the whole answer must be read.
    pub notes: Vec<String>,
}

// ----------------------------------------------------------- measurement ---

/// One located interruption in a grain's evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hole {
    series: Option<String>,
    from: OffsetDateTime,
    to: OffsetDateTime,
    seconds: i64,
}

/// One stretch of the window a source cannot answer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Boundary {
    kind: BoundaryKind,
    at: OffsetDateTime,
    to: Option<OffsetDateTime>,
    detail: String,
}

/// What one grain measured inside its own stretch of the window.
#[derive(Debug, Clone)]
struct GrainEvidence {
    grain: &'static str,
    grain_seconds: Option<i64>,
    /// The stretch of the window this grain serves; `None` when the window lies
    /// entirely outside what the grain reaches.
    span: Option<(OffsetDateTime, OffsetDateTime)>,
    point_count: i64,
    sample_count: i64,
    expected_points: Option<i64>,
    first_observed_at: Option<OffsetDateTime>,
    last_observed_at: Option<OffsetDateTime>,
    cadence_seconds: Option<i64>,
    longest_gap_seconds: Option<i64>,
    holes: Vec<Hole>,
    note: Option<String>,
}

impl GrainEvidence {
    /// A grain the window lies entirely outside, or one that holds nothing yet.
    fn unserved(grain: &'static str, grain_seconds: Option<i64>, note: impl Into<String>) -> Self {
        GrainEvidence {
            grain,
            grain_seconds,
            span: None,
            point_count: 0,
            sample_count: 0,
            expected_points: None,
            first_observed_at: None,
            last_observed_at: None,
            cadence_seconds: None,
            longest_gap_seconds: None,
            holes: Vec::new(),
            note: Some(note.into()),
        }
    }

    /// The points an unbroken grain would hold that no row answers, when the
    /// grain's grid makes that knowable.
    fn missing_points(&self) -> Option<i64> {
        self.expected_points
            .map(|expected| (expected - self.point_count).max(0))
    }

    fn to_response(&self) -> InvestigationGrainResponse {
        InvestigationGrainResponse {
            grain: self.grain.to_owned(),
            grain_seconds: self.grain_seconds,
            available: self.span.is_some() && self.point_count > 0,
            from: self.span.map(|(from, _)| format_rfc3339(from)),
            to: self.span.map(|(_, to)| format_rfc3339(to)),
            note: self.note.clone(),
            point_count: self.point_count,
            sample_count: self.sample_count,
            expected_points: self.expected_points,
            missing_points: self.missing_points(),
            first_observed_at: self.first_observed_at.map(format_rfc3339),
            last_observed_at: self.last_observed_at.map(format_rfc3339),
            cadence_seconds: self.cadence_seconds,
            longest_gap_seconds: self.longest_gap_seconds,
            holes: self
                .holes
                .iter()
                .map(|hole| InvestigationHoleResponse {
                    series: hole.series.clone(),
                    from: format_rfc3339(hole.from),
                    to: format_rfc3339(hole.to),
                    seconds: Some(hole.seconds),
                })
                .collect(),
        }
    }
}

/// The ledger coordinates of one source: what the Server holds and what its
/// cleanup has released.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Ledger {
    first_observed_at: Option<OffsetDateTime>,
    last_observed_at: Option<OffsetDateTime>,
    last_received_at: Option<OffsetDateTime>,
    released_before: Option<OffsetDateTime>,
}

impl Ledger {
    /// Fold a source's per-series ledger rows into one coordinate.
    ///
    /// The release floor is the newest of the series' floors: a source can only
    /// claim to answer what every one of its series still holds, so the most
    /// recent cleanup is the honest boundary for the source as a whole.
    fn reduce(rows: &[LedgerRow]) -> Ledger {
        let mut ledger = Ledger::default();
        for row in rows {
            if let Some(value) = canonical_instant(&row.first_observed_at) {
                ledger.first_observed_at = Some(
                    ledger
                        .first_observed_at
                        .map_or(value, |current| current.min(value)),
                );
            }
            if let Some(value) = canonical_instant(&row.last_observed_at) {
                ledger.last_observed_at = Some(
                    ledger
                        .last_observed_at
                        .map_or(value, |current| current.max(value)),
                );
            }
            if let Some(value) = canonical_instant(&row.last_received_at) {
                ledger.last_received_at = Some(
                    ledger
                        .last_received_at
                        .map_or(value, |current| current.max(value)),
                );
            }
            if let Some(value) = canonical_instant(&row.released_before) {
                ledger.released_before = Some(
                    ledger
                        .released_before
                        .map_or(value, |current| current.max(value)),
                );
            }
        }
        ledger
    }
}

/// One ledger row as the Server stores it.
#[derive(Debug, FromRow)]
struct LedgerRow {
    first_observed_at: String,
    last_observed_at: String,
    last_received_at: String,
    released_before: String,
}

/// A collector row as the Server stores it.
#[derive(Debug, Clone, FromRow)]
struct ComponentRow {
    scope: String,
    scope_key: String,
    component_key: String,
    state: String,
    error_code: Option<String>,
    observed_at: Option<String>,
    received_at: Option<String>,
}

/// The state the collectors of one source last reported.
///
/// A source is only as healthy as its own collectors, and an error in any of them
/// is the source's state: a source whose writer refused work is not reported as
/// healthy because another of its collectors succeeded. Without an error, the
/// first collector that reported at all answers.
fn source_state(rows: &[ComponentRow], keys: &[&str]) -> (Option<String>, Option<String>) {
    let named = || {
        rows.iter()
            .filter(|row| keys.contains(&row.component_key.as_str()))
    };
    match named().find(|row| row.state == COLLECTOR_STATE_ERROR) {
        Some(row) => (Some(row.state.clone()), row.error_code.clone()),
        None => match named().next() {
            Some(row) => (Some(row.state.clone()), row.error_code.clone()),
            None => (None, None),
        },
    }
}

impl ComponentRow {
    fn to_response(&self) -> InvestigationComponentResponse {
        InvestigationComponentResponse {
            scope: self.scope.clone(),
            scope_key: self.scope_key.clone(),
            component_key: self.component_key.clone(),
            state: self.state.clone(),
            error_code: self.error_code.clone(),
            observed_at: self.observed_at.clone(),
            received_at: self.received_at.clone(),
        }
    }
}

/// Optional history the Server refused for part of the window because the disk
/// was full.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pause {
    /// The scope the refused write belonged to (`node` or `host`).
    scope: String,
    /// The instant protection opened.
    started_at: OffsetDateTime,
    /// The instant protection closed, when it has closed.
    ended_at: Option<OffsetDateTime>,
    /// The oldest refused observation.
    first_skipped_at: OffsetDateTime,
    /// The newest refused observation.
    last_skipped_at: OffsetDateTime,
    /// The metric or state component whose samples were refused.
    series: String,
    /// How many observations were refused.
    skipped_count: i64,
}

impl Pause {
    /// The stretch of refused history.
    fn span(&self) -> (OffsetDateTime, OffsetDateTime) {
        (self.first_skipped_at, self.last_skipped_at)
    }

    /// Whether a stretch of the window overlaps this pause's refused history.
    fn covers(&self, from: OffsetDateTime, to: OffsetDateTime) -> bool {
        let (pause_from, pause_to) = self.span();
        from <= pause_to && to >= pause_from
    }

    fn boundary(&self) -> Boundary {
        Boundary {
            kind: BoundaryKind::LowSpacePause,
            at: self.first_skipped_at,
            to: Some(self.last_skipped_at),
            detail: format!(
                "{} optional {} sample(s) were not stored while the Server protected a nearly full disk, from {} to {} (protection opened {}{})",
                self.skipped_count,
                self.series,
                format_rfc3339(self.first_skipped_at),
                format_rfc3339(self.last_skipped_at),
                format_rfc3339(self.started_at),
                match self.ended_at {
                    Some(ended_at) => format!(", protection closed {}", format_rfc3339(ended_at)),
                    None => ", protection had not closed yet".to_owned(),
                }
            ),
        }
    }
}

/// The source-scoped facts a coverage verdict is judged from.
struct CoverageFacts<'a> {
    window: &'a ResolvedWindow,
    /// Why the source does not apply to this subject at all.
    unsupported: Option<&'a str>,
    /// Whether the source records occurrences rather than continuous evidence,
    /// so a silence between two of them is not a gap.
    event_source: bool,
    /// How long after the newest observation the window may still be answered.
    grace_seconds: i64,
    ledger: &'a Ledger,
    retained_from: Option<OffsetDateTime>,
    grains: &'a [GrainEvidence],
    /// The protection intervals this source's own refused samples were recorded
    /// under, so a stretch they cover is answered as a pause rather than as a
    /// collection failure (design §11.5).
    pauses: &'a [Pause],
    boundaries: Vec<Boundary>,
    truncated: bool,
}

/// What a coverage verdict says.
struct CoverageVerdict {
    coverage: Coverage,
    boundaries: Vec<Boundary>,
}

/// Judge how much of the window a source can answer.
///
/// The order of the questions is the point. A source that does not apply to the
/// subject, or one the Server has never held a single observation for, is
/// answered before any arithmetic runs: those cases have no measurable coverage
/// to report, and reporting them as an empty fraction would describe them as
/// known-empty windows (design §5.1).
fn judge_coverage(facts: &CoverageFacts<'_>) -> CoverageVerdict {
    let mut boundaries = facts.boundaries.clone();
    if facts.unsupported.is_some() {
        return CoverageVerdict {
            coverage: Coverage::Unsupported,
            boundaries,
        };
    }
    let Some(first_observed_at) = facts.ledger.first_observed_at else {
        boundaries.push(Boundary {
            kind: BoundaryKind::NeverObserved,
            at: facts.window.from,
            to: None,
            detail: "the Server has never stored an observation for this source on this subject"
                .to_owned(),
        });
        return CoverageVerdict {
            coverage: Coverage::Unavailable,
            boundaries,
        };
    };
    // A window that ends in the same second as the source's first observation
    // still overlaps the source's history, so only a window that ends strictly
    // before it is answered as one nothing can be said about. Reading the tie as
    // "after the window ends" would let a millisecond decide the verdict.
    if first_observed_at > facts.window.to {
        boundaries.push(Boundary {
            kind: BoundaryKind::PreEnablement,
            at: facts.window.from,
            to: Some(facts.window.to),
            detail: format!(
                "the first observation of this source is {}, which is after the window ends",
                format_rfc3339(first_observed_at)
            ),
        });
        return CoverageVerdict {
            coverage: Coverage::Unavailable,
            boundaries,
        };
    }
    if let Some(retained_from) = facts.retained_from {
        if retained_from >= facts.window.to {
            boundaries.push(Boundary {
                kind: BoundaryKind::RetentionCleanup,
                at: facts.window.from,
                to: Some(facts.window.to),
                detail: format!(
                    "cleanup has released this source's evidence before {}, so none of the window is still stored",
                    format_rfc3339(retained_from)
                ),
            });
            return CoverageVerdict {
                coverage: Coverage::Unavailable,
                boundaries,
            };
        }
    }

    if first_observed_at > facts.window.from {
        boundaries.push(Boundary {
            kind: BoundaryKind::PreEnablement,
            at: facts.window.from,
            to: Some(first_observed_at),
            detail: format!(
                "the earliest observation of this source is {}, so the window opens before the Agent reported any of it",
                format_rfc3339(first_observed_at)
            ),
        });
    }
    if let Some(retained_from) = facts.retained_from {
        if retained_from > facts.window.from {
            boundaries.push(Boundary {
                kind: BoundaryKind::RetentionCleanup,
                at: facts.window.from,
                to: Some(retained_from),
                detail: format!(
                    "evidence older than {} was released by cleanup",
                    format_rfc3339(retained_from)
                ),
            });
        }
    }
    for grain in facts.grains {
        boundaries.extend(
            grain
                .holes
                .iter()
                // A refused sample is not a collection failure (design §11.5): when
                // protection covers the stretch, the pause boundary already names it
                // in the Server's own words, and the hole itself is still reported as
                // the grain's own silence beside its stored points. This is the same
                // verdict the metric history engine reaches for a stretch a protection
                // interval intersects (crates/platpulse-server/src/metric_history.rs:1000).
                .filter(|hole| {
                    !facts
                        .pauses
                        .iter()
                        .any(|pause| pause.covers(hole.from, hole.to))
                })
                .map(|hole| hole_boundary(grain.grain, hole)),
        );
    }
    if !facts.event_source {
        if let Some(last_observed_at) = facts.ledger.last_observed_at {
            let silence = (facts.window.to - last_observed_at).whole_seconds();
            if silence > facts.grace_seconds {
                boundaries.push(Boundary {
                    kind: BoundaryKind::StaleTail,
                    at: last_observed_at,
                    to: Some(facts.window.to),
                    detail: format!(
                        "the newest observation of this source is {}, {silence}s before the window ends",
                        format_rfc3339(last_observed_at)
                    ),
                });
            }
        }
    }

    boundaries.sort_by(|left, right| {
        left.at
            .cmp(&right.at)
            .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
    });
    boundaries.dedup_by(|left, right| left.kind == right.kind && left.at == right.at);

    let point_count: i64 = facts.grains.iter().map(|grain| grain.point_count).sum();
    let missing = facts.grains.iter().any(|grain| {
        grain
            .missing_points()
            .map(|missing| missing > 0)
            .unwrap_or(false)
    });
    let coverage = if !boundaries.is_empty() || facts.truncated || missing {
        Coverage::Partial
    } else if point_count == 0 {
        Coverage::Empty
    } else {
        Coverage::Complete
    };
    CoverageVerdict {
        coverage,
        boundaries,
    }
}

/// The boundary a located interruption becomes.
fn hole_boundary(grain: &str, hole: &Hole) -> Boundary {
    let series = hole
        .series
        .as_ref()
        .map(|series| format!(" on {series}"))
        .unwrap_or_default();
    Boundary {
        kind: BoundaryKind::CollectionFailure,
        at: hole.from,
        to: Some(hole.to),
        detail: format!(
            "the {grain} grain holds no observation{series} for {}s, from {} to {}",
            hole.seconds,
            format_rfc3339(hole.from),
            format_rfc3339(hole.to)
        ),
    }
}

/// The boundary marking a stretch the Server recorded for another subject.
///
/// The relation is known here — the records name whose stretch it is — and what is
/// unknown is whether this source's own subject held it. Saying so is the only
/// honest answer: the evidence behind this boundary belongs to another subject and
/// must be read in that subject's own family (issue #221, story 61).
fn other_subject_boundary(kind: &str, span: &RelationSpan) -> Boundary {
    Boundary {
        kind: BoundaryKind::RelationshipUnknown,
        at: span.from,
        to: Some(span.to),
        detail: format!(
            "the Server recorded {} {} for this stretch, so this family holds no evidence attributed to {} it answers about",
            crate::relationships::relation_kind_label(kind),
            span.related_key,
            crate::relationships::related_subject_label(kind)
        ),
    }
}

/// The boundary that marks a stretch the whole record leaves uncovered.
///
/// A stretch with no recorded interval is not an uninteresting stretch: the Node
/// was somewhere, and the Server simply did not write down where. Reporting it as
/// the relations the Node has now would back-fill the past from the present, and
/// reporting it as nothing at all would read a missing record as a fact, so it is
/// named as what it is (issue #221, stories 61, 66).
fn uncovered_record_boundary(
    from: OffsetDateTime,
    to: OffsetDateTime,
    recorded_from: Option<OffsetDateTime>,
) -> Boundary {
    let detail = match recorded_from {
        None => "the Server has never recorded which Agent or Network this Node belonged to, and recording is prospective, so this stretch is not attributed to the relations the Node has now"
            .to_owned(),
        Some(_) => format!(
            "the Server recorded nothing about which Agent or Network this Node belonged to from {} to {}, so this stretch is not attributed to the relations it had before or after it",
            format_rfc3339(from),
            format_rfc3339(to)
        ),
    };
    Boundary {
        kind: BoundaryKind::RelationshipUnknown,
        at: from,
        to: Some(to),
        detail,
    }
}

/// The boundary that marks a stretch one relation holds no record for.
///
/// Read by a family that answers about one related subject: the stretch was given
/// to nobody this source can read, so it is named rather than attributed to the
/// relation the Node has now (issue #221, stories 61, 66).
fn uncovered_relation_boundary(
    kind: &str,
    from: OffsetDateTime,
    to: OffsetDateTime,
    recorded_from: Option<OffsetDateTime>,
) -> Boundary {
    let relation = crate::relationships::relation_kind_label(kind);
    let detail = match recorded_from {
        None => format!(
            "the Server has never recorded a {relation} for this Node, and recording is prospective, so this stretch is not attributed to the relation the Node has now"
        ),
        Some(_) => format!(
            "the Server recorded no {relation} for this Node from {} to {}, so this stretch is not attributed to the relation it had before or after it",
            format_rfc3339(from),
            format_rfc3339(to)
        ),
    };
    Boundary {
        kind: BoundaryKind::RelationshipUnknown,
        at: from,
        to: Some(to),
        detail,
    }
}

/// One recorded stretch of a relation, as the attribution an answer names it by.
///
/// The fields that state where an attribution comes from — the recorded basis, its
/// label, and the two edges of the stretch — are written in one place, so every
/// family that attributes its evidence to the recorded relations says it the same
/// way; only the role, the kind of subject and the sentence differ (issue #221,
/// stories 60, 61).
fn relation_attribution(
    subject_kind: &str,
    span: &RelationSpan,
    role: &str,
    detail: String,
) -> InvestigationRelatedSubjectResponse {
    InvestigationRelatedSubjectResponse {
        subject_kind: subject_kind.to_owned(),
        subject: span.related_key.clone(),
        role: role.to_owned(),
        basis: BASIS_RECORDED_RELATIONSHIP.to_owned(),
        basis_label: basis_label(BASIS_RECORDED_RELATIONSHIP).to_owned(),
        from: format_rfc3339(span.from),
        to: format_rfc3339(span.to),
        detail,
    }
}

// --------------------------------------------------------------- readers ---

/// Why an investigation could not be answered.
#[derive(Debug)]
pub enum InvestigationError {
    /// This Server does not know the Node.
    NotFound,
    /// The Node was purged. Its evidence is gone by design, not by accident, so
    /// this is told apart from an unknown id.
    Purged { deleted_at: String },
    /// The Server could not read its own storage.
    Unavailable(sqlx::Error),
}

impl From<sqlx::Error> for InvestigationError {
    fn from(error: sqlx::Error) -> Self {
        InvestigationError::Unavailable(error)
    }
}

/// The Node an investigation is about.
#[derive(Debug, Clone, FromRow)]
struct NodeRow {
    node_id: String,
    agent_id: String,
    display_name: Option<String>,
    lifecycle: String,
    visibility: String,
    /// When the Server first saw this Node, which is the earliest instant any of
    /// its sources could have started.
    first_seen_at: String,
}

/// The ledger rows one scope holds, and whether the read hit its own bound.
const LEDGER_ROW_LIMIT: i64 = 2048;

/// The collector state a source reports as an error.
const COLLECTOR_STATE_ERROR: &str = "error";

/// How long a metric source may stay silent at the end of a window before the
/// tail of that window is reported as stale rather than answered.
///
/// The same three-cadence rule the metric engine judges holes with
/// ([crate::metric_history::gap_threshold_seconds]) sets the widest gap a healthy
/// series may show, so a source that has been quiet longer than that has stopped.
const METRIC_GRACE_SECONDS: i64 = 3 * crate::metric_history::MIN_GAP_SECONDS;

/// How long the state sources may stay silent before the tail is stale: state
/// rows are written when the component changes and as hourly anchors, so two
/// anchor intervals of silence are what a healthy component may show.
const STATE_GRACE_SECONDS: i64 = 2 * crate::state_history::STATE_ANCHOR_SECONDS;

/// How long Peer receipt buckets may stop arriving before the tail is stale.
const PEER_GRACE_SECONDS: i64 = 3 * 3600;

/// How long Validator daily snapshots may stop arriving before the tail is stale.
/// A daily snapshot is written once per local day, and a provider may answer late.
const VALIDATOR_GRACE_SECONDS: i64 = 36 * 3600;

/// What every source reader needs that the orchestration resolved once.
struct SourceContext<'a> {
    pool: &'a SqlitePool,
    window: &'a ResolvedWindow,
    now: OffsetDateTime,
    node_id: &'a str,
    agent_id: &'a str,
    /// When the Server first saw this Node, which is the earliest instant any of
    /// its sources could have started.
    first_seen_at: OffsetDateTime,
    /// The configured Validator timezone, for the local day grid.
    timezone: &'a str,
    /// Every collector row the Server holds for this Node and its Agent.
    components: &'a [ComponentRow],
    /// Every protection pause this Node's history and its Agent's history lost
    /// samples to.
    pauses: &'a [Pause],
    policies: &'a RetentionPolicies,
}

/// The declared retention of each family, as one lookup.
struct RetentionPolicies {
    days: Vec<(String, i64)>,
}

impl RetentionPolicies {
    fn from_rows(rows: &[crate::retention::PolicyRow]) -> Self {
        RetentionPolicies {
            days: rows
                .iter()
                .map(|row| (row.family.clone(), row.retention_days))
                .collect(),
        }
    }

    /// How many days the family is kept. A family with no finite retention, or
    /// one the Server does not know, has no bound to apply.
    fn days(&self, family: &str) -> Option<i64> {
        self.days
            .iter()
            .find(|(key, _)| key == family)
            .map(|(_, days)| *days)
            .filter(|days| *days > 0)
    }
}

/// The instant beyond which this source's cleanup cannot answer.
///
/// Two facts say where a source stops: the release floor its ledger recorded
/// when cleanup ran, and the retention its family is declared with. The later of
/// the two is the honest boundary, and which one produced it is stated beside it
/// so a declared policy is never reported as a cleanup that happened.
fn retained_from(
    now: OffsetDateTime,
    released_before: Option<OffsetDateTime>,
    retention_days: Option<i64>,
) -> Option<OffsetDateTime> {
    let declared = retention_days.map(|days| now - Duration::days(days));
    match (released_before, declared) {
        (Some(released), Some(declared)) => Some(released.max(declared)),
        (Some(released), None) => Some(released),
        (None, declared) => declared,
    }
}

/// How the boundary that a retention bound explains is worded.
fn retention_detail(
    cutoff: OffsetDateTime,
    released_before: Option<OffsetDateTime>,
    retention_days: Option<i64>,
) -> String {
    match (released_before, retention_days) {
        (Some(_), Some(days)) => format!(
            "cleanup, and the {days} day retention this family is declared with, leave {} as the oldest instant this source can answer",
            format_rfc3339(cutoff)
        ),
        (Some(_), None) => format!(
            "cleanup released this source's evidence before {}",
            format_rfc3339(cutoff)
        ),
        (None, Some(days)) => format!(
            "this family is kept {days} day(s), so evidence older than {} may already have been released (no release floor is recorded for it)",
            format_rfc3339(cutoff)
        ),
        (None, None) => format!(
            "the oldest instant this source can answer is {}",
            format_rfc3339(cutoff)
        ),
    }
}

/// How many buckets of one epoch-aligned grain a range holds.
///
/// A stored observation belongs to the bucket its own instant aligns to
/// ([crate::metric_history::aligned_bucket_start]), so the buckets a range covers
/// are the boundaries from the one `from` sits in through the one `to` sits in,
/// inclusive.
fn aligned_count(from: OffsetDateTime, to: OffsetDateTime, grain_seconds: i64) -> i64 {
    if grain_seconds <= 0 {
        return 0;
    }
    let from = from.unix_timestamp();
    let to = to.unix_timestamp();
    if to < from {
        return 0;
    }
    to.div_euclid(grain_seconds) - from.div_euclid(grain_seconds) + 1
}

/// Seconds since the Unix epoch for one instant column, computed inside SQLite.
///
/// The interval between two adjacent observations is what proves a stretch of
/// silence, and the difference is taken where the rows are: a window of buckets
/// must never be fetched only to subtract two timestamps in Rust.
fn epoch_of(column: &str) -> String {
    format!("CAST(ROUND((julianday({column}) - 2440587.5) * 86400) AS INTEGER)")
}

/// The columns that name one series inside a scope.
fn series_key_columns(schema: &HistorySchema) -> &'static str {
    if schema.has_dimension {
        "metric, dimension"
    } else {
        "metric"
    }
}

/// The statics one grain's rows are measured into.
#[derive(Debug, FromRow)]
struct TierStats {
    point_count: i64,
    sample_count: i64,
    first_observed_at: Option<String>,
    last_observed_at: Option<String>,
    cadence_seconds: Option<i64>,
    longest_gap_seconds: Option<i64>,
}

/// The widest interval one grain holds, with the two observations that bound it.
#[derive(Debug, FromRow)]
struct WorstGapRow {
    metric: String,
    dimension: String,
    previous: String,
    at: String,
    seconds: i64,
}

/// One metric tier's statistics for a scope inside a range.
fn metric_stats_sql(schema: &HistorySchema, tier: Option<i64>) -> String {
    let columns = series_key_columns(schema);
    let dimension = if schema.has_dimension {
        ", dimension"
    } else {
        ""
    };
    let instants = TierInstants::of(columns, tier);
    let spacing = instants.spacing();
    // The widest stretch this tier can prove: a bucket knows the widest silence
    // it counted inside itself as well as the silence before it.
    let widest = if tier.is_some() {
        format!("max({spacing}, COALESCE(max_gap_seconds, 0))")
    } else {
        spacing.clone()
    };
    let extras = if tier.is_some() {
        ", max_gap_seconds, sample_count, first_observed_at, last_observed_at"
    } else {
        ""
    };
    let grain_predicate = instants.grain_predicate;
    let table = if tier.is_some() { "{agg}" } else { "{raw}" };
    let sample_count = if tier.is_some() {
        "COALESCE(SUM(sample_count), 0)"
    } else {
        "0"
    };
    // A bucket tier answers one point per bucket, so its resolution is the
    // bucket's own width: the silence between two buckets is not a cadence.
    let (cadence, first, last) = if tier.is_some() {
        (
            "NULL".to_owned(),
            "MIN(first_observed_at)".to_owned(),
            "MAX(last_observed_at)".to_owned(),
        )
    } else {
        (
            format!("MIN(CASE WHEN previous IS NOT NULL THEN {spacing} END)"),
            "MIN(at)".to_owned(),
            "MAX(at)".to_owned(),
        )
    };
    schema.sql(&format!(
        "WITH ordered AS (\
             SELECT metric{dimension}, {at} AS at, {received} AS received_at, \
                    {previous} AS previous{extras} \
             FROM {table} \
             WHERE {{scope}} = ?{grain_predicate} AND {range} >= ? AND {range} <= ?\
         ) \
         SELECT COUNT(*) AS point_count, {sample_count} AS sample_count, \
                {first} AS first_observed_at, {last} AS last_observed_at, \
                MAX(received_at) AS last_received_at, \
                {cadence} AS cadence_seconds, \
                MAX(CASE WHEN previous IS NOT NULL THEN {widest} END) AS longest_gap_seconds \
         FROM ordered",
        at = instants.at,
        received = instants.received,
        previous = instants.previous,
        range = instants.range,
    ))
}

/// The widest interval one metric tier holds for a scope inside a range.
///
/// This runs only once a grain's own statistics prove that an interval wide
/// enough to be an interruption exists, so the common answer pays one ranged
/// read per tier rather than two.
fn metric_worst_gap_sql(schema: &HistorySchema, tier: Option<i64>) -> String {
    let columns = series_key_columns(schema);
    let dimension = if schema.has_dimension {
        ", dimension"
    } else {
        ", '' AS dimension"
    };
    let instants = TierInstants::of(columns, tier);
    let grain_predicate = instants.grain_predicate;
    let table = if tier.is_some() { "{agg}" } else { "{raw}" };
    let at = epoch_of("at");
    let previous = epoch_of("previous");
    schema.sql(&format!(
        "WITH ordered AS (\
             SELECT metric{dimension}, {first} AS at, {earlier} AS previous \
             FROM {table} \
             WHERE {{scope}} = ?{grain_predicate} AND {range} >= ? AND {range} <= ?\
         ) \
         SELECT metric, dimension, previous, at, {at} - {previous} AS seconds \
           FROM ordered \
          WHERE previous IS NOT NULL \
          ORDER BY seconds DESC, at ASC \
          LIMIT 1",
        first = instants.at,
        earlier = instants.previous,
        range = instants.range,
    ))
}

/// The instants one tier's silences are judged between.
///
/// The raw tier holds one row per observation, so a collection failure runs
/// between two stored instants. A bucket is a summary of what it counted, and
/// its two stored instants alone are not continuity evidence: the same count of
/// four observations describes a bucket observed every second and one with a
/// 296-second hole in the middle. A silence between two buckets therefore runs
/// from the newest observation the earlier bucket counted to the oldest the
/// later one counted - judged between real instants rather than between bucket
/// edges, so the answer can never claim a stretch the Server may have observed
/// (migrations/0067_node_metric_aggregates.sql:29-42).
struct TierInstants {
    /// The column naming the instant this row's own point was observed at.
    at: &'static str,
    /// The window expression naming the previous row's newest observation.
    previous: String,
    /// The column this tier records the receipt time in.
    received: &'static str,
    /// The column the range filter is applied to.
    range: &'static str,
    /// The predicate naming this tier's grain, if it is an aggregate tier.
    grain_predicate: &'static str,
}

impl TierInstants {
    fn of(columns: &str, tier: Option<i64>) -> Self {
        match tier {
            Some(_) => TierInstants {
                at: "first_observed_at",
                previous: format!(
                    "LAG(last_observed_at) OVER (PARTITION BY {columns} ORDER BY bucket_start)"
                ),
                received: "last_received_at",
                range: "bucket_start",
                grain_predicate: " AND grain_seconds = ?",
            },
            None => TierInstants {
                at: "observed_at",
                previous: format!(
                    "LAG(observed_at) OVER (PARTITION BY {columns} ORDER BY observed_at)"
                ),
                received: "received_at",
                range: "observed_at",
                grain_predicate: "",
            },
        }
    }

    /// The seconds between this row's point and the previous one.
    fn spacing(&self) -> String {
        format!("{} - {}", epoch_of("at"), epoch_of("previous"))
    }
}

/// One series' ledger as the history engine records it.
async fn load_metric_ledger(
    pool: &SqlitePool,
    schema: &HistorySchema,
    scope_key: &str,
) -> Result<(Vec<LedgerRow>, bool), sqlx::Error> {
    // The order names the series the same way the grain queries partition them:
    // the Node ledger has no dimension column, the host ledger does.
    let statement = schema.sql(&format!(
        "SELECT first_observed_at, last_observed_at, last_received_at, released_before \
         FROM {{ledger}} WHERE {{scope}} = ? ORDER BY {} LIMIT ?",
        series_key_columns(schema)
    ));
    let rows = sqlx::query_as::<_, LedgerRow>(&statement)
        .bind(scope_key)
        .bind(LEDGER_ROW_LIMIT + 1)
        .fetch_all(pool)
        .await?;
    let truncated = rows.len() as i64 > LEDGER_ROW_LIMIT;
    Ok((
        rows.into_iter().take(LEDGER_ROW_LIMIT as usize).collect(),
        truncated,
    ))
}

/// The series label one interrupted grain names.
fn series_label(metric: &str, dimension: &str) -> Option<String> {
    if dimension.is_empty() {
        Some(metric.to_owned())
    } else {
        Some(format!("{metric} ({dimension})"))
    }
}

/// The interruption a tier's own statistics proved, as a hole.
///
/// The two instants the gap query returns are the observations on either side of
/// the interruption, so the hole is the stretch between them; a row whose own
/// instants the Server stores canonically is the only one that can name a hole.
fn metric_hole(row: &WorstGapRow) -> Option<Hole> {
    Some(Hole {
        series: series_label(&row.metric, &row.dimension),
        from: canonical_instant(&row.previous)?,
        to: canonical_instant(&row.at)?,
        seconds: row.seconds,
    })
}

/// One grain measured from its own statistics.
fn tier_grain(
    grain: &'static str,
    grain_seconds: Option<i64>,
    span: (OffsetDateTime, OffsetDateTime),
    stats: TierStats,
    expected_points: Option<i64>,
    hole_threshold_seconds: i64,
    worst: Option<(Hole, &'static str)>,
) -> GrainEvidence {
    let cadence_seconds = stats
        .cadence_seconds
        .or(grain_seconds)
        .filter(|seconds| *seconds > 0);
    let longest_gap_seconds = stats.longest_gap_seconds;
    let mut holes = Vec::new();
    let mut note = None;
    if let Some(longest) = longest_gap_seconds {
        if longest > hole_threshold_seconds {
            if let Some((hole, wording)) = worst {
                holes.push(hole);
                note = Some(wording.to_owned());
            }
        }
    }
    // A grain that serves the window and holds nothing is stated rather than
    // left to be inferred from a zero count.
    if note.is_none() && stats.point_count == 0 {
        note = Some("this grain serves this window and holds no point inside it".to_owned());
    }
    GrainEvidence {
        grain,
        grain_seconds,
        span: Some(span),
        point_count: stats.point_count,
        sample_count: stats.sample_count,
        expected_points,
        first_observed_at: stats
            .first_observed_at
            .as_deref()
            .and_then(canonical_instant),
        last_observed_at: stats
            .last_observed_at
            .as_deref()
            .and_then(canonical_instant),
        cadence_seconds,
        longest_gap_seconds,
        holes,
        note,
    }
}

/// How many buckets one scope's series should hold inside a stretch.
///
/// Each series is counted from its own first observation: a series that started
/// inside the window cannot have buckets before it existed, and counting them as
/// missing would report a Node that was enabled yesterday as a Node with a hole
/// the size of the window.
fn grid_expected_points(
    ledger: &[LedgerRow],
    span: (OffsetDateTime, OffsetDateTime),
    grain_seconds: i64,
) -> i64 {
    let (from, to) = span;
    ledger
        .iter()
        .filter_map(|row| canonical_instant(&row.first_observed_at))
        .map(|first| aligned_count(from.max(first), to, grain_seconds))
        .sum()
}

/// What a metric tier is measured over.
struct TierRequest {
    grain: &'static str,
    grain_seconds: Option<i64>,
    oldest: OffsetDateTime,
    newest: OffsetDateTime,
    /** How old a stretch must be before this tier answers it, for the note. */
    account: &'static str,
}

/// Measure one metric source: the Node's own series, or the Agent's Host ones.
async fn metric_source(
    ctx: &SourceContext<'_>,
    schema: &HistorySchema,
    builder: SourceBuilder,
    collectors: &Collectors,
    // The series names the collector writes, so a pause is only attributed to
    // the source whose own samples were refused.
    pause_series: &[&str],
    answer_label: &str,
    answer_path: String,
) -> Result<InvestigationSourceResponse, sqlx::Error> {
    let scope_key = if schema.has_dimension {
        ctx.agent_id
    } else {
        ctx.node_id
    };
    let (ledger_rows, ledger_truncated) = load_metric_ledger(ctx.pool, schema, scope_key).await?;
    let ledger = Ledger::reduce(&ledger_rows);
    let retention_days = ctx.policies.days(FAMILY_FIVE_MINUTE_AGGREGATE);
    let retained = retained_from(ctx.now, ledger.released_before, retention_days);
    let collector_keys = collectors.keys();
    let (source_state, error_code) = source_state(ctx.components, &collector_keys);

    let raw_cutoff = raw_window_cutoff(ctx.now);
    let one_minute_cutoff = ctx.now - Duration::days(ONE_MINUTE_MAX_AGE_DAYS);
    let five_minute_cutoff = ctx.now - Duration::days(FIVE_MINUTE_MAX_AGE_DAYS);
    let mut builder = builder;
    builder.ledger = ledger.clone();
    builder.retained_from = retained;
    builder.retention_days = retention_days;
    builder.source_state = source_state;
    builder.error_code = error_code;
    builder.components = collectors.rows(ctx.components);
    builder.pauses = ctx
        .pauses
        .iter()
        .filter(|pause| {
            pause.scope == if schema.has_dimension { "host" } else { "node" }
                && pause_series.contains(&pause.series.as_str())
        })
        .cloned()
        .collect();
    builder.truncated = ledger_truncated;
    if ledger_truncated {
        builder.notes.push(format!(
            "this source holds more than {LEDGER_ROW_LIMIT} series; the counts below cover the first {LEDGER_ROW_LIMIT} of them"
        ));
    }
    builder = builder.answer(answer_label, answer_path, None);

    let tiers = vec![
        TierRequest {
            grain: GRAIN_RAW,
            grain_seconds: None,
            oldest: ctx.window.from.max(raw_cutoff),
            newest: ctx.window.to,
            account: "the raw tier answers the last 24 hours of stored observations",
        },
        TierRequest {
            grain: GRAIN_ONE_MINUTE,
            grain_seconds: Some(ONE_MINUTE_SECONDS),
            oldest: ctx.window.from.max(one_minute_cutoff),
            newest: ctx.window.to.min(raw_cutoff),
            account: "the 1-minute tier answers the stretch the raw window no longer holds, up to 7 days old",
        },
        TierRequest {
            grain: GRAIN_FIVE_MINUTE,
            grain_seconds: Some(FIVE_MINUTE_SECONDS),
            oldest: ctx.window.from.max(five_minute_cutoff),
            newest: ctx.window.to.min(one_minute_cutoff),
            account: "the 5-minute tier answers the oldest stretch, from 7 to 30 days old",
        },
    ];

    for tier in tiers {
        if tier.oldest > tier.newest {
            builder = builder.grain(GrainEvidence::unserved(
                tier.grain,
                tier.grain_seconds,
                format!("this tier does not serve this window: {}", tier.account),
            ));
            continue;
        }
        let span = (tier.oldest, tier.newest);
        let stats = match tier.grain_seconds {
            Some(grain_seconds) => {
                sqlx::query_as::<_, TierStats>(&metric_stats_sql(schema, Some(grain_seconds)))
                    .bind(scope_key)
                    .bind(grain_seconds)
                    .bind(format_rfc3339(span.0))
                    .bind(format_rfc3339(span.1))
                    .fetch_one(ctx.pool)
                    .await?
            }
            None => {
                sqlx::query_as::<_, TierStats>(&metric_stats_sql(schema, None))
                    .bind(scope_key)
                    .bind(format_rfc3339(span.0))
                    .bind(format_rfc3339(span.1))
                    .fetch_one(ctx.pool)
                    .await?
            }
        };
        let cadence = stats
            .cadence_seconds
            .or(tier.grain_seconds)
            .unwrap_or(FALLBACK_GAP_SECONDS);
        let threshold = gap_threshold_seconds(cadence);
        let expected_points = tier
            .grain_seconds
            .map(|grain_seconds| grid_expected_points(&ledger_rows, span, grain_seconds));
        let worst = if stats.longest_gap_seconds.unwrap_or(0) > threshold {
            let wording = if tier.grain_seconds.is_some() {
                "the longest interruption is listed; the buckets this tier does not hold are counted in missing_points"
            } else {
                "the longest interruption is listed; the tier's statistics carry any shorter ones"
            };
            fetch_worst_gap(ctx.pool, schema, scope_key, tier.grain_seconds, span)
                .await?
                .and_then(|row| metric_hole(&row).map(|hole| (hole, wording)))
        } else {
            None
        };
        builder = builder.grain(tier_grain(
            tier.grain,
            tier.grain_seconds,
            span,
            stats,
            expected_points,
            threshold,
            worst,
        ));
    }

    if let Some(retained) = retained {
        builder = builder.note(retention_detail(
            retained,
            ledger.released_before,
            retention_days,
        ));
    }
    Ok(builder.finish(ctx.window))
}

/// The one interruption a tier's own statistics proved, when it proved one.
async fn fetch_worst_gap(
    pool: &SqlitePool,
    schema: &HistorySchema,
    scope_key: &str,
    grain_seconds: Option<i64>,
    span: (OffsetDateTime, OffsetDateTime),
) -> Result<Option<WorstGapRow>, sqlx::Error> {
    let sql = metric_worst_gap_sql(schema, grain_seconds);
    let mut statement = sqlx::query_as::<_, WorstGapRow>(&sql);
    statement = statement.bind(scope_key);
    if let Some(grain_seconds) = grain_seconds {
        statement = statement.bind(grain_seconds);
    }
    statement
        .bind(format_rfc3339(span.0))
        .bind(format_rfc3339(span.1))
        .fetch_optional(pool)
        .await
}

// --------------------------------------------------------------- builder ---

/// Assemble one source's answer from what its own reader measured.
struct SourceBuilder {
    key: &'static str,
    label: &'static str,
    subject_kind: &'static str,
    subject: String,
    time_basis: TimeBasis,
    unsupported: Option<String>,
    event_source: bool,
    grace_seconds: i64,
    ledger: Ledger,
    retained_from: Option<OffsetDateTime>,
    retention_days: Option<i64>,
    source_state: Option<String>,
    error_code: Option<String>,
    pauses: Vec<Pause>,
    components: Vec<ComponentRow>,
    answer_paths: Vec<InvestigationAnswerPathResponse>,
    related_subjects: Vec<InvestigationRelatedSubjectResponse>,
    boundaries: Vec<Boundary>,
    grains: Vec<GrainEvidence>,
    notes: Vec<String>,
    truncated: bool,
}

impl SourceBuilder {
    fn new(
        key: &'static str,
        label: &'static str,
        subject_kind: &'static str,
        subject: &str,
        time_basis: TimeBasis,
        grace_seconds: i64,
    ) -> Self {
        SourceBuilder {
            key,
            label,
            subject_kind,
            subject: subject.to_owned(),
            time_basis,
            unsupported: None,
            event_source: false,
            grace_seconds,
            ledger: Ledger::default(),
            retained_from: None,
            retention_days: None,
            source_state: None,
            error_code: None,
            pauses: Vec::new(),
            components: Vec::new(),
            answer_paths: Vec::new(),
            related_subjects: Vec::new(),
            boundaries: Vec::new(),
            grains: Vec::new(),
            notes: Vec::new(),
            truncated: false,
        }
    }

    fn note(mut self, text: impl Into<String>) -> Self {
        self.notes.push(text.into());
        self
    }

    fn grain(mut self, grain: GrainEvidence) -> Self {
        self.grains.push(grain);
        self
    }

    fn answer(mut self, label: &str, path: String, note: Option<&str>) -> Self {
        self.answer_paths.push(InvestigationAnswerPathResponse {
            label: label.to_owned(),
            path,
            note: note.map(str::to_owned),
        });
        self
    }

    /// Attribute this source's own evidence to the relations the Server recorded.
    ///
    /// Every recorded span inside the window becomes one named attribution: the
    /// subject is marked as this source's own subject when the recorded key is the
    /// subject the source answers about, and as a related subject when it is a
    /// different one. Every stretch the records leave uncovered becomes an unknown
    /// boundary instead, because the relation the Node has now is not evidence
    /// about what it was then, and the Server never back-fills one (stories 60, 61).
    fn attribute_relation(mut self, view: &RelationView, subject_kind: &str) -> Self {
        for span in &view.spans {
            let role = if span.related_key == self.subject {
                SUBJECT_ROLE_SOURCE
            } else {
                SUBJECT_ROLE_RELATED
            };
            self.related_subjects.push(relation_attribution(
                subject_kind,
                span,
                role,
                crate::relationships::describe_span(view.kind, span),
            ));
        }
        for (from, to) in &view.unknown_spans {
            self.boundaries
                .push(uncovered_record_boundary(*from, *to, view.recorded_from));
        }
        self
    }

    /// Attribute this source's evidence to the recorded stretches of its own
    /// subject, and mark every other stretch as unknown for this source.
    ///
    /// For a family whose subject is the relation itself — the Agent that collected
    /// and reported one measurement — a stretch the records give to a different
    /// subject is evidence about a different subject, and this family holds nothing
    /// for it. Reading the current subject back over it would present another
    /// subject's measurement as this one's (issue #221, stories 60, 61).
    fn attribute_own_relation(mut self, view: &RelationView, subject_kind: &str) -> Self {
        for span in &view.spans {
            // A stretch the record gives to another subject still names that
            // subject here — the attribution is the Server's, and the boundary
            // only states that this family holds nothing for the stretch itself.
            let own = span.related_key == self.subject;
            self.related_subjects.push(relation_attribution(
                subject_kind,
                span,
                if own {
                    SUBJECT_ROLE_SOURCE
                } else {
                    SUBJECT_ROLE_RELATED
                },
                crate::relationships::describe_span(view.kind, span),
            ));
            if !own {
                self.boundaries
                    .push(other_subject_boundary(view.kind, span));
            }
        }
        for (from, to) in &view.unknown_spans {
            self.boundaries.push(uncovered_relation_boundary(
                view.kind,
                *from,
                *to,
                view.recorded_from,
            ));
        }
        self
    }

    /// The pause boundaries this source's refused samples explain.
    ///
    /// A refused sample is not a collection failure: the Agent was reporting and
    /// the Server chose not to store optional history, so the two must never
    /// merge into one story (design §11.5, story 59).
    fn pause_boundaries(&self, window: &ResolvedWindow) -> Vec<Boundary> {
        self.pauses
            .iter()
            .filter(|pause| pause.covers(window.from, window.to))
            .map(Pause::boundary)
            .collect()
    }

    fn finish(self, window: &ResolvedWindow) -> InvestigationSourceResponse {
        let paused = self.pause_boundaries(window);
        let SourceBuilder {
            key,
            label,
            subject_kind,
            subject,
            time_basis,
            unsupported,
            event_source,
            grace_seconds,
            ledger,
            retained_from,
            retention_days,
            source_state,
            error_code,
            pauses,
            components,
            answer_paths,
            related_subjects,
            boundaries,
            grains,
            notes,
            truncated,
        } = self;

        // The pause boundaries come last, so a refused stretch is read after the
        // boundaries that explain the rest of the window.
        let boundaries: Vec<Boundary> = boundaries.into_iter().chain(paused).collect();

        let mut ledger = ledger;
        ledger.first_observed_at = ledger.first_observed_at.or_else(|| {
            grains
                .iter()
                .filter_map(|grain| grain.first_observed_at)
                .min()
        });
        ledger.last_observed_at = ledger.last_observed_at.or_else(|| {
            grains
                .iter()
                .filter_map(|grain| grain.last_observed_at)
                .max()
        });

        let facts = CoverageFacts {
            window,
            unsupported: unsupported.as_deref(),
            event_source,
            grace_seconds,
            ledger: &ledger,
            retained_from,
            grains: &grains,
            pauses: &pauses,
            boundaries,
            truncated,
        };
        let verdict = judge_coverage(&facts);
        let mut notes = notes;
        if let Some(reason) = &unsupported {
            notes.push(reason.clone());
        }

        InvestigationSourceResponse {
            key: key.to_owned(),
            label: label.to_owned(),
            subject_kind: subject_kind.to_owned(),
            subject,
            coverage: verdict.coverage.as_str().to_owned(),
            coverage_label: verdict.coverage.label().to_owned(),
            time_basis: time_basis.as_str().to_owned(),
            time_basis_label: time_basis.label().to_owned(),
            source_state,
            error_code,
            first_observed_at: ledger.first_observed_at.map(format_rfc3339),
            last_observed_at: ledger.last_observed_at.map(format_rfc3339),
            last_received_at: ledger.last_received_at.map(format_rfc3339),
            released_before: ledger.released_before.map(format_rfc3339),
            retained_from: retained_from.map(format_rfc3339),
            retention_days,
            truncated,
            boundaries: verdict
                .boundaries
                .iter()
                .map(|boundary| InvestigationBoundaryResponse {
                    kind: boundary.kind.as_str().to_owned(),
                    kind_label: boundary.kind.label().to_owned(),
                    at: format_rfc3339(boundary.at),
                    to: boundary.to.map(format_rfc3339),
                    detail: boundary.detail.clone(),
                })
                .collect(),
            grains: grains.iter().map(GrainEvidence::to_response).collect(),
            components: components.iter().map(ComponentRow::to_response).collect(),
            answer_paths,
            related_subjects,
            notes,
        }
    }
}

// --------------------------------------------------------------- state ---

/// The collector component keys one history source is written by.
///
/// The keys come from the ingestion projection itself
/// ([crate::http::report_ingestion::component_storage_key]) instead of being
/// written out as literals, so a renamed component cannot leave a source reading
/// a key nothing writes.
struct Collectors(Vec<String>);

impl Collectors {
    fn of(keys: &[ComponentKey]) -> Self {
        Collectors(keys.iter().map(|key| component_storage_key(*key)).collect())
    }

    /// The keys as the collector-row lookup wants them.
    fn keys(&self) -> Vec<&str> {
        self.0.iter().map(String::as_str).collect()
    }

    /// The collector rows this source is written by.
    fn rows(&self, rows: &[ComponentRow]) -> Vec<ComponentRow> {
        rows.iter()
            .filter(|row| self.0.iter().any(|key| key == &row.component_key))
            .cloned()
            .collect()
    }

    /// Whether one series name belongs to one of these collectors.
    fn writes(&self, series: &str) -> bool {
        self.0.iter().any(|key| key == series)
    }
}

/// One state component's own statistics inside a stretch.
#[derive(Debug, FromRow)]
struct StateStatsRow {
    point_count: i64,
    first_observed_at: Option<String>,
    last_observed_at: Option<String>,
    longest_gap_seconds: Option<i64>,
}

/// The widest silence one state component shows inside a stretch.
#[derive(Debug, FromRow)]
struct StateHoleRow {
    component: String,
    previous: String,
    at: String,
    seconds: i64,
}

/// One state component's stored entries, each beside the previous one.
const STATE_ORDERED_SQL: &str = "\
    SELECT component, observed_at, received_at, \
           LAG(observed_at) OVER (PARTITION BY component ORDER BY observed_at) AS previous \
      FROM node_state_observations \
     WHERE node_id = ? AND observed_at >= ? AND observed_at <= ?";

/// SQL: what the state entries of one Node hold inside a stretch.
fn state_stats_sql() -> String {
    format!(
        "WITH ordered AS ({STATE_ORDERED_SQL}) \
         SELECT COUNT(*) AS point_count, \
                MIN(observed_at) AS first_observed_at, \
                MAX(observed_at) AS last_observed_at, \
                MAX({gap}) AS longest_gap_seconds \
           FROM ordered",
        gap = state_gap_seconds(),
    )
}

/// SQL: the widest silence between two state entries, as one row.
fn state_hole_sql() -> String {
    let gap = state_gap_seconds();
    format!(
        "WITH ordered AS ({STATE_ORDERED_SQL}) \
         SELECT component, previous, observed_at AS at, {gap} AS seconds \
           FROM ordered \
          WHERE previous IS NOT NULL AND {gap} > {STATE_GRACE_SECONDS} \
          ORDER BY seconds DESC, at ASC LIMIT 1"
    )
}

/// The seconds between one state entry and the entry before it.
fn state_gap_seconds() -> String {
    format!("({} - {})", epoch_of("observed_at"), epoch_of("previous"))
}

/// The interruption a state component's own rows prove, as a hole.
///
/// State rows are written when the component changes and as hourly anchors, so a
/// silence longer than two anchors is two missed anchors, not a component that
/// had nothing to say (design §11.7).
fn state_hole(row: StateHoleRow) -> Option<(Hole, &'static str)> {
    Some((
        Hole {
            series: Some(row.component.clone()),
            from: canonical_instant(&row.previous)?,
            to: canonical_instant(&row.at)?,
            seconds: row.seconds,
        },
        "the state grain holds no entry across two hourly anchors, so this component's state is unknown for that stretch",
    ))
}

/// Measure the Node's sync and consensus history.
async fn state_source(ctx: &SourceContext<'_>) -> Result<InvestigationSourceResponse, sqlx::Error> {
    let collectors = Collectors::of(&[ComponentKey::Sync, ComponentKey::Consensus]);
    let mut rows = Vec::new();
    for component in crate::state_history::STATE_COMPONENTS {
        let row = sqlx::query_as::<_, LedgerRow>(
            "SELECT first_observed_at, last_observed_at, last_received_at, released_before \
               FROM node_state_series_state \
              WHERE node_id = ? AND component = ?",
        )
        .bind(ctx.node_id)
        .bind(component)
        .fetch_optional(ctx.pool)
        .await?;
        rows.extend(row);
    }
    let ledger = Ledger::reduce(&rows);
    let retention_days = ctx.policies.days(FAMILY_OBSERVATION_STATE);
    let retained = retained_from(ctx.now, ledger.released_before, retention_days);
    let collector_keys = collectors.keys();
    let (source_state, error_code) = source_state(ctx.components, &collector_keys);

    let span = (ctx.window.from, ctx.window.to);
    let (from, to) = (format_rfc3339(span.0), format_rfc3339(span.1));
    let stats = sqlx::query_as::<_, StateStatsRow>(&state_stats_sql())
        .bind(ctx.node_id)
        .bind(&from)
        .bind(&to)
        .fetch_one(ctx.pool)
        .await?;
    let hole = if stats.longest_gap_seconds.unwrap_or(0) > STATE_GRACE_SECONDS {
        sqlx::query_as::<_, StateHoleRow>(&state_hole_sql())
            .bind(ctx.node_id)
            .bind(&from)
            .bind(&to)
            .fetch_optional(ctx.pool)
            .await?
            .and_then(state_hole)
    } else {
        None
    };

    let mut builder = SourceBuilder::new(
        SOURCE_NODE_STATE,
        "Sync and consensus entries",
        "node",
        ctx.node_id,
        TimeBasis::StateObservation,
        STATE_GRACE_SECONDS,
    );
    builder.ledger = ledger.clone();
    builder.retained_from = retained;
    builder.retention_days = retention_days;
    builder.source_state = source_state;
    builder.error_code = error_code;
    builder.components = collectors.rows(ctx.components);
    builder.pauses = ctx
        .pauses
        .iter()
        .filter(|pause| pause.scope == "node" && collectors.writes(&pause.series))
        .cloned()
        .collect();
    builder = builder.note(
        "entries are stored when the component changes and as hourly anchors, so the count is entries rather than changes, and a quiet stretch means an unchanged component",
    );
    builder = builder.grain(tier_grain(
        GRAIN_STATE_ENTRY,
        None,
        span,
        TierStats {
            point_count: stats.point_count,
            sample_count: stats.point_count,
            first_observed_at: stats.first_observed_at,
            last_observed_at: stats.last_observed_at,
            cadence_seconds: None,
            longest_gap_seconds: stats.longest_gap_seconds,
        },
        None,
        STATE_GRACE_SECONDS,
        hole,
    ));
    builder = builder.answer(
        "Sync and consensus entries",
        format!(
            "/api/admin/v1/nodes/{}/state-history?component=sync&from={from}&to={to}",
            ctx.node_id
        ),
        Some("one read per component; the entries carry the collection state and value source of each write"),
    );
    if let Some(retained) = retained {
        builder = builder.note(retention_detail(
            retained,
            ledger.released_before,
            retention_days,
        ));
    }
    Ok(builder.finish(ctx.window))
}

// --------------------------------------------------------------- peers ---

/// The seconds one five-minute Peer receipt bucket holds.
const PEER_FIVE_MINUTE_SECONDS: i64 = 300;

/// The seconds one hourly Peer receipt bucket holds.
const PEER_HOURLY_SECONDS: i64 = 3_600;

/// One Peer receipt bucket tier, as the Server's own aggregation holds it.
struct PeerTier {
    grain: &'static str,
    table: &'static str,
    grain_seconds: i64,
    /// The stretch this tier answers for, or None when it holds nothing here.
    span: Option<(OffsetDateTime, OffsetDateTime)>,
    /// Why this tier does not serve this window, said when it does not.
    account: String,
}

/// One stretch of receipt buckets, counted.
#[derive(Debug, FromRow)]
struct PeerStatsRow {
    point_count: i64,
    sample_count: i64,
    first_observed_at: Option<String>,
    last_observed_at: Option<String>,
}

/// The two bounds the Peer aggregation itself records.
#[derive(Debug, FromRow)]
struct PeerBoundsRow {
    first_bucket: Option<String>,
    last_bucket: Option<String>,
    last_observed: Option<String>,
}

/// The widest stretch two receipt buckets are apart.
#[derive(Debug, FromRow)]
struct AlignedGapRow {
    previous: String,
    at: String,
    missing: i64,
}

/// SQL: the widest stretch between two receipt buckets, in missing buckets.
///
/// The Peer engine writes a bucket only when a Report carries Peer evidence, so
/// the grid itself is what proves a missing bucket: two buckets that are an hour
/// apart at a five-minute grain are eleven buckets the Server never received.
fn peer_gap_sql(table: &str, grain_seconds: i64) -> String {
    let gap = format!("({} - {})", epoch_of("bucket_start"), epoch_of("previous"));
    format!(
        "WITH ordered AS (\
             SELECT bucket_start, LAG(bucket_start) OVER (ORDER BY bucket_start) AS previous \
               FROM {table} \
              WHERE node_id = ? AND bucket_start >= ? AND bucket_start <= ?\
         ) \
         SELECT previous, bucket_start AS at, {gap} / {grain_seconds} - 1 AS missing \
           FROM ordered \
          WHERE previous IS NOT NULL AND {gap} > {grain_seconds} \
          ORDER BY missing DESC, at ASC LIMIT 1"
    )
}

/// Measure the Node's Peer history as the Server received it.
async fn peers_source(ctx: &SourceContext<'_>) -> Result<InvestigationSourceResponse, sqlx::Error> {
    let collectors = Collectors::of(&[ComponentKey::Peers]);
    let collector_keys = collectors.keys();
    let (source_state, error_code) = source_state(ctx.components, &collector_keys);
    // The hourly tier is the durable record: cleanup never releases it, so this
    // source can answer any stretch it ever received. The five-minute tier is the
    // finer grain and the one cleanup releases first, so a receipt it no longer
    // holds is answered, at the coarser grain, by the hourly one — and only the
    // stretch that really left the finer tier is answered coarsely, so a window
    // inside it is never reported as evidence the Server never stored.
    let retention_days = ctx.policies.days(FAMILY_PEER_AGGREGATE_1H);
    let retained = retained_from(ctx.now, None, retention_days);
    let five_minute_days = ctx.policies.days(FAMILY_PEER_AGGREGATE_5M);
    let five_minute_released = retained_from(ctx.now, None, five_minute_days);

    // The ledger is the whole receipt record, over both tiers: a bound taken from
    // one table alone would report the other tier's cleanup as a Node that never
    // reported. A bucket start is the instant the Server accepted a Report, so the
    // newest bucket it wrote is the newest receipt coordinate the Server can name,
    // and the bucket's own newest observation is the newest instant a receipt was
    // counted at.
    let bounds = sqlx::query_as::<_, PeerBoundsRow>(
        "SELECT MIN(bucket_start) AS first_bucket, MAX(bucket_start) AS last_bucket, \
                MAX(last_observed_at) AS last_observed \
           FROM (SELECT bucket_start, last_observed_at FROM peer_aggregate_5m WHERE node_id = ? \
                 UNION ALL \
                 SELECT bucket_start, last_observed_at FROM peer_aggregate_1h WHERE node_id = ?)",
    )
    .bind(ctx.node_id)
    .bind(ctx.node_id)
    .fetch_one(ctx.pool)
    .await?;
    let ledger = Ledger {
        first_observed_at: bounds.first_bucket.as_deref().and_then(canonical_instant),
        last_observed_at: bounds.last_observed.as_deref().and_then(canonical_instant),
        last_received_at: bounds.last_bucket.as_deref().and_then(canonical_instant),
        released_before: None,
    };

    let five_minute_oldest = match five_minute_released {
        Some(released) => ctx.window.from.max(released),
        None => ctx.window.from,
    };
    let hourly_newest = five_minute_released.map(|released| ctx.window.to.min(released));
    let tiers = [
        PeerTier {
            grain: GRAIN_PEER_5M,
            table: "peer_aggregate_5m",
            grain_seconds: PEER_FIVE_MINUTE_SECONDS,
            span: (five_minute_oldest <= ctx.window.to)
                .then_some((five_minute_oldest, ctx.window.to)),
            account: match five_minute_days {
                Some(days) => format!(
                    "the five-minute tier is kept {days} day(s), so receipts older than {} were released by cleanup and are answered by the hourly tier",
                    format_rfc3339(five_minute_released.unwrap_or(ctx.window.from))
                ),
                None => "the five-minute tier keeps every receipt it ever stored".to_string(),
            },
        },
        PeerTier {
            grain: GRAIN_PEER_1H,
            table: "peer_aggregate_1h",
            grain_seconds: PEER_HOURLY_SECONDS,
            span: match hourly_newest {
                Some(newest) if ctx.window.from <= newest => Some((ctx.window.from, newest)),
                _ => None,
            },
            account: match five_minute_released {
                Some(released) => format!(
                    "the hourly tier holds the receipts the five-minute tier released, before {}",
                    format_rfc3339(released)
                ),
                None => "the five-minute tier holds every receipt this window covers, so the hourly grain has no stretch of its own here".to_string(),
            },
        },
    ];

    let (window_from, window_to) = (
        format_rfc3339(ctx.window.from),
        format_rfc3339(ctx.window.to),
    );
    let mut builder = SourceBuilder::new(
        SOURCE_PEERS,
        "Peer history",
        "node",
        ctx.node_id,
        TimeBasis::PeerReceiptBucket,
        PEER_GRACE_SECONDS,
    );
    builder.ledger = ledger.clone();
    builder.retained_from = retained;
    builder.retention_days = retention_days;
    builder.source_state = source_state;
    builder.error_code = error_code;
    builder.components = collectors.rows(ctx.components);
    builder.pauses = ctx
        .pauses
        .iter()
        .filter(|pause| pause.scope == "node" && collectors.writes(&pause.series))
        .cloned()
        .collect();
    builder = builder.note(
        "a bucket is aligned to the instant the Server accepted the Report (the receipt), not to the instant the Agent counted the peers: it says when the evidence arrived",
    );
    builder = builder.note(
        "the Server stores a bucket only when a Report carries Peer evidence, so a stretch with no bucket means none arrived rather than the peer set being unchanged",
    );

    for tier in tiers {
        let Some(span) = tier.span else {
            builder = builder.grain(GrainEvidence::unserved(
                tier.grain,
                Some(tier.grain_seconds),
                format!("this tier does not serve this window: {}", tier.account),
            ));
            continue;
        };
        let (from, to) = (format_rfc3339(span.0), format_rfc3339(span.1));
        let stats = sqlx::query_as::<_, PeerStatsRow>(&format!(
            "SELECT COUNT(*) AS point_count, \
                    COALESCE(SUM(sample_count), 0) AS sample_count, \
                    MIN(bucket_start) AS first_observed_at, \
                    MAX(bucket_start) AS last_observed_at \
               FROM {} \
              WHERE node_id = ? AND bucket_start >= ? AND bucket_start <= ?",
            tier.table
        ))
        .bind(ctx.node_id)
        .bind(&from)
        .bind(&to)
        .fetch_one(ctx.pool)
        .await?;
        let hole =
            sqlx::query_as::<_, AlignedGapRow>(&peer_gap_sql(tier.table, tier.grain_seconds))
                .bind(ctx.node_id)
                .bind(&from)
                .bind(&to)
                .fetch_optional(ctx.pool)
                .await?
                .and_then(|row| {
                    Some(Hole {
                        series: None,
                        from: canonical_instant(&row.previous)?
                            + Duration::seconds(tier.grain_seconds),
                        to: canonical_instant(&row.at)?,
                        seconds: row.missing * tier.grain_seconds,
                    })
                });
        if hole.is_some() {
            builder = builder.note(
                "the longest silence between two Peer receipts is listed; the Server received no Report carrying Peer evidence for that stretch, and the buckets it did not receive are counted in missing_points",
            );
        }
        // The whole span is aggregated in one query — COUNT, MIN/MAX and one LAG —
        // so this tier answers every bucket its stretch holds and never clips a
        // page: the read tails the peer panel pages with are not what storage keeps.
        builder = builder.grain(tier_grain(
            tier.grain,
            Some(tier.grain_seconds),
            span,
            TierStats {
                point_count: stats.point_count,
                sample_count: stats.sample_count,
                first_observed_at: stats.first_observed_at,
                last_observed_at: stats.last_observed_at.clone(),
                    cadence_seconds: Some(tier.grain_seconds),
                // The measured silence is the distance between the two receipts,
                // which is one bucket wider than the number missing between them.
                longest_gap_seconds: hole.as_ref().map(|hole| hole.seconds + tier.grain_seconds),
            },
            None,
            tier.grain_seconds,
            hole.map(|hole| {
                (
                    hole,
                    "a stretch carries no receipt bucket, so the Server holds no Peer evidence for it",
                )
            }),
        ));
    }

    for (grain, label) in [
        ("5m", "Peer receipt buckets, five minutes"),
        ("1h", "Peer receipt buckets, one hour"),
    ] {
        builder = builder.answer(
            label,
            format!(
                "/api/admin/v1/nodes/{}/peer-history?grain={grain}&from={window_from}&to={window_to}",
                ctx.node_id
            ),
            None,
        );
    }
    Ok(builder.finish(ctx.window))
}

// ------------------------------------------------------- relationships ---

/// The relationships the Server recorded for this Node inside the window.
///
/// This family is the coordinate the rest of the answer is read against. The
/// Server writes down which Agent and which Network a Node belonged to as it
/// accepts Reports and completes Node Transfers, starting from the moment that
/// recording exists: nothing here is back-filled from the Agent or Network key the
/// Node happens to have now, and a stretch the record does not cover is named as
/// unknown instead of being described with the current relation (issue #221,
/// stories 60, 61, 66).
fn relationships_source(
    ctx: &SourceContext<'_>,
    relations: &RecordedRelations,
    intervals: &[RelationshipInterval],
) -> InvestigationSourceResponse {
    // The record's own ledger: the earliest instant the Server could have held a
    // record of this Node, the instant the record reaches to, and the last time the
    // Server wrote one down. An interval the Server has not closed is read as still
    // holding, so a current record produces no staleness claim.
    let first_observed_at = intervals
        .iter()
        .filter_map(|interval| interval.valid_from_at())
        .min()
        .map(|instant| instant.min(ctx.first_seen_at))
        .unwrap_or(ctx.first_seen_at);
    let last_observed_at = intervals
        .iter()
        .map(|interval| interval.valid_until_at().unwrap_or(ctx.window.to))
        .max();
    let last_recorded_at = intervals
        .iter()
        .filter_map(|interval| canonical_instant(&interval.recorded_at))
        .max();

    let mut builder = SourceBuilder::new(
        SOURCE_RELATIONSHIPS,
        "Recorded relationships",
        "node",
        ctx.node_id,
        TimeBasis::ServerRecord,
        0,
    );
    // The record is the Server's own writing rather than a collected series, so a
    // stretch it does not cover is an unknown boundary rather than a silence of a
    // cadence that never existed.
    builder.event_source = true;
    builder.ledger = Ledger {
        first_observed_at: Some(first_observed_at),
        last_observed_at,
        last_received_at: last_recorded_at,
        released_before: None,
    };
    builder = builder.note(
        "this family is the Server's own record of which subjects this Node belonged to: it is written as the Server accepts a Report or completes a Node Transfer, and it is never back-filled from the Agent or Network key the Node has now",
    );
    builder = builder.note(
        "the recorded relations are the Server's own writing and are read in this answer, which is the only place the Server serves them, so this family points at no other path",
    );

    let mut recorded_spans = 0;
    for kind in RELATION_KINDS {
        let view = relations.view(kind);
        recorded_spans += view.spans.len();
        if view.spans.is_empty() {
            builder = builder.note(format!(
                "the Server holds no record of a {} for this Node inside this window, and holding no record is not evidence that it had none",
                relation_kind_label(kind)
            ));
        }
        for span in &view.spans {
            builder = builder.note(describe_span(kind, span));
        }
        builder = builder.attribute_relation(view, kind);
    }

    // How many of the intervals the Server holds overlap this window at all: one read
    // only for the record's own start is not a sample of this window.
    let recorded_intervals = intervals
        .iter()
        .filter(|interval| {
            interval
                .valid_from_at()
                .and_then(|from| {
                    clipped_span(
                        from,
                        interval.valid_until_at(),
                        ctx.window.from,
                        ctx.window.to,
                    )
                })
                .is_some()
        })
        .count();

    let spans = relations.spans();
    builder = builder.grain(tier_grain(
        GRAIN_RECORDED_INTERVAL,
        None,
        (ctx.window.from, ctx.window.to),
        TierStats {
            point_count: recorded_spans as i64,
            sample_count: recorded_intervals as i64,
            first_observed_at: spans.first().map(|(_, span)| format_rfc3339(span.from)),
            last_observed_at: spans.last().map(|(_, span)| format_rfc3339(span.to)),
            cadence_seconds: None,
            // Intervals are not a series: the distance between two of them is not a
            // missing observation, and no interruption can be proved from it.
            longest_gap_seconds: None,
        },
        None,
        i64::MAX,
        None,
    ));
    builder.finish(ctx.window)
}

// ----------------------------------------------------------- incidents ---

/// The window-overlap predicate Alert Incidents are read with.
///
/// An occurrence belongs to a window when it had already opened and had not
/// closed when the window started: an Incident still open at a window's start is
/// part of that window's story, and one resolved before it is not. The binds are
/// the window's end and then its start.
pub const INCIDENT_WINDOW_PREDICATE: &str =
    "i.opened_at < ? AND (i.resolved_at IS NULL OR i.resolved_at > ?)";

/// The statement one subject's Incident statistics are read with.
///
/// The binds are the stretch's end, its start, the subject kind and the subject
/// key. An Incident belongs to the stretch it overlapped rather than to the instant
/// it opened, so one that opened before the stretch and had not closed when it
/// started is counted in it too.
fn incident_stats_sql() -> String {
    format!(
        "SELECT COUNT(*) AS point_count, \
                MIN(i.opened_at) AS first_opened_at, \
                MAX(i.opened_at) AS last_opened_at, \
                MAX(COALESCE(i.resolved_at, i.opened_at)) AS last_evidence_at \
           FROM alert_incidents i \
          WHERE {INCIDENT_WINDOW_PREDICATE} AND i.subject_kind = ? AND i.subject_key = ?"
    )
}

/// What one stretch of Alert Incidents holds for a subject.
#[derive(Debug, FromRow)]
struct IncidentStatsRow {
    point_count: i64,
    first_opened_at: Option<String>,
    last_opened_at: Option<String>,
    last_evidence_at: Option<String>,
}

/// Incidents on one related subject, inside the stretch recorded for that relation.
struct RelatedIncidents {
    subject_kind: &'static str,
    subject: String,
    from: OffsetDateTime,
    to: OffsetDateTime,
    stats: IncidentStatsRow,
}

/// The Incident statistics one subject carries inside one stretch.
async fn load_incident_stats(
    ctx: &SourceContext<'_>,
    subject_kind: &str,
    subject_key: &str,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Result<IncidentStatsRow, sqlx::Error> {
    sqlx::query_as::<_, IncidentStatsRow>(&incident_stats_sql())
        .bind(format_rfc3339(to))
        .bind(format_rfc3339(from))
        .bind(subject_kind)
        .bind(subject_key)
        .fetch_one(ctx.pool)
        .await
}

/// The instants one subject's Incident statistics name, as canonical instants.
fn incident_instants(
    stats: &IncidentStatsRow,
) -> (
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
) {
    (
        stats.first_opened_at.as_deref().and_then(canonical_instant),
        stats.last_opened_at.as_deref().and_then(canonical_instant),
        stats
            .last_evidence_at
            .as_deref()
            .and_then(canonical_instant),
    )
}

/// Measure the Alert Incidents that cover a window.
///
/// The Node's own Incidents are read over the whole window: they are about this
/// Node whatever it was related to. Incidents on a related subject are read only
/// over the stretches the Server recorded that relation for, and are marked as
/// related, because an Incident opened while a different Agent reported this Node
/// is not evidence about the Agent that reports it now (issue #221, stories 60, 61).
async fn incidents_source(
    ctx: &SourceContext<'_>,
    relations: &RecordedRelations,
) -> Result<InvestigationSourceResponse, sqlx::Error> {
    let (from, to) = (
        format_rfc3339(ctx.window.from),
        format_rfc3339(ctx.window.to),
    );
    let stats =
        load_incident_stats(ctx, "node", ctx.node_id, ctx.window.from, ctx.window.to).await?;

    // Only a recorded Agent relation can carry Incidents on a related subject: the
    // Alert catalog evaluates Agent and Host subjects on the Agent's own key, and
    // holds no rule whose subject is a Network, so a recorded Network relation
    // cannot appear in this family at all (crates/platpulse-server/src/alerts/mod.rs,
    // the rule catalog).
    let mut related: Vec<RelatedIncidents> = Vec::new();
    for (kind, span) in relations.spans() {
        if kind != RELATION_AGENT {
            continue;
        }
        for subject_kind in ["agent", "host"] {
            let related_stats =
                load_incident_stats(ctx, subject_kind, &span.related_key, span.from, span.to)
                    .await?;
            if related_stats.point_count > 0 {
                related.push(RelatedIncidents {
                    subject_kind,
                    subject: span.related_key.clone(),
                    from: span.from,
                    to: span.to,
                    stats: related_stats,
                });
            }
        }
    }

    // An Incident is an occurrence, not a series: the earliest instant this
    // source could have opened one is when the Server first saw the Node, so a
    // window with no Incidents in it is an empty window rather than an
    // unobserved one, and a window that ends before the Node existed is not
    // answered as an empty list either.
    let (mut first_opened, mut last_opened, mut last_evidence) = incident_instants(&stats);
    for incident in &related {
        let (first, last, evidence) = incident_instants(&incident.stats);
        first_opened = [first_opened, first].into_iter().flatten().min();
        last_opened = [last_opened, last].into_iter().flatten().max();
        last_evidence = [last_evidence, evidence].into_iter().flatten().max();
    }
    let point_count: i64 = stats.point_count
        + related
            .iter()
            .map(|incident| incident.stats.point_count)
            .sum::<i64>();
    let ledger = Ledger {
        first_observed_at: Some(ctx.first_seen_at),
        last_observed_at: last_opened,
        last_received_at: last_evidence,
        released_before: None,
    };

    let mut builder = SourceBuilder::new(
        SOURCE_INCIDENTS,
        "Alert incidents",
        "node",
        ctx.node_id,
        TimeBasis::IncidentEvaluation,
        0,
    );
    builder.event_source = true;
    builder.ledger = ledger;
    builder = builder.note(
        "Incidents are evaluated by the Server from its own history, so no Agent collector reports on them and none is named here",
    );
    builder = builder.note(
        "Incidents on a related subject are counted only inside the stretches the Server recorded that relation for, and the Alert catalog holds no rule on a Network subject, so a recorded Network relation cannot appear in this family",
    );
    let mut grain = tier_grain(
        GRAIN_OCCURRENCE,
        None,
        (ctx.window.from, ctx.window.to),
        TierStats {
            point_count,
            sample_count: point_count,
            first_observed_at: first_opened.map(format_rfc3339),
            last_observed_at: last_opened.map(format_rfc3339),
            cadence_seconds: None,
            // An occurrence source has no cadence: a quiet stretch between two
            // Incidents is not a missing observation, so no interruption can be
            // proved from the distance between them.
            longest_gap_seconds: None,
        },
        None,
        i64::MAX,
        None,
    );
    let related_count: i64 = related
        .iter()
        .map(|incident| incident.stats.point_count)
        .sum();
    if related_count > 0 {
        grain.note = Some(format!(
            "{related_count} of these Incidents are on subjects recorded as related to this Node, inside the stretches the Server held that relation for"
        ));
    }
    builder = builder.grain(grain);
    // A related subject's Incidents can only be counted where the record names a
    // relation, so a window the record does not cover wholly is answered with that
    // gap named here rather than read as an absence of Incidents (issue #221,
    // stories 60, 61).
    let wholly_recorded =
        relations.agent.unknown_spans.is_empty() && relations.network.unknown_spans.is_empty();
    if related_count == 0 && !wholly_recorded {
        builder = builder.note(
            "no Incident on a related subject is counted for this window: the record of this Node's relations does not cover all of it, and a stretch with no recorded relation is a gap in the record rather than an absence of Incidents",
        );
    }
    builder = builder.answer(
        "Incidents covering this window",
        format!(
            "/api/admin/v1/alerts/incidents?subject_kind=node&subject_key={}&from={from}&to={to}",
            ctx.node_id
        ),
        Some("an Incident that opened before the window and had not closed when it started is listed too"),
    );
    for incident in &related {
        builder = builder.note(format!(
            "{} Alert Incidents on {} {} fall inside the stretch the Server recorded that relation for, from {} to {}; they are read as Incidents on a related subject and never as this Node's own",
            incident.stats.point_count,
            incident.subject_kind,
            incident.subject,
            format_rfc3339(incident.from),
            format_rfc3339(incident.to)
        ));
        builder = builder.answer(
            &format!(
                "Incidents on related {} {}",
                incident.subject_kind, incident.subject
            ),
            format!(
                "/api/admin/v1/alerts/incidents?subject_kind={}&subject_key={}&from={}&to={}",
                incident.subject_kind,
                incident.subject,
                format_rfc3339(incident.from),
                format_rfc3339(incident.to)
            ),
            Some("only the stretch the Server recorded this relation for is asked, not the whole window"),
        );
        builder.related_subjects.push(InvestigationRelatedSubjectResponse {
            subject_kind: incident.subject_kind.to_owned(),
            subject: incident.subject.clone(),
            role: SUBJECT_ROLE_RELATED.to_owned(),
            basis: BASIS_RECORDED_RELATIONSHIP.to_owned(),
            basis_label: basis_label(BASIS_RECORDED_RELATIONSHIP).to_owned(),
            from: format_rfc3339(incident.from),
            to: format_rfc3339(incident.to),
            detail: format!(
                "{} Incident(s) on this related subject inside the stretch the Server recorded it for",
                incident.stats.point_count
            ),
        });
    }
    Ok(builder.finish(ctx.window))
}

// ----------------------------------------------------------- validator ---

/// One Validator link that overlaps the window.
#[derive(Debug, FromRow)]
struct ValidatorLinkRow {
    validator_id: String,
    role: Option<String>,
    origin: String,
    valid_from: String,
    valid_until: Option<String>,
}

/// What the Validator daily snapshots hold inside the window's days.
#[derive(Debug, FromRow)]
struct ValidatorGridRow {
    entries: i64,
    days: i64,
    first_sample_at: Option<String>,
    last_sample_at: Option<String>,
}

/// Where the linked snapshots start and stop, over all time.
#[derive(Debug, FromRow)]
struct ValidatorHorizonRow {
    last_sample_at: Option<String>,
    last_received_at: Option<String>,
}

/// SQL: the Validator links that overlap a window.
const VALIDATOR_LINK_SQL: &str = "\
    SELECT validator_id, role, origin, valid_from, valid_until \
      FROM node_validator_links \
     WHERE node_id = ? AND valid_from < ? AND (valid_until IS NULL OR valid_until > ?) \
     ORDER BY valid_from DESC";

/// SQL: where the snapshots of those links start and stop.
///
/// A snapshot belongs to a link only while the Server held it: `received_at` is the
/// Server's own receipt clock, the clock the edges of the link itself are written
/// from, so a day read under an earlier link — or before any link existed — is not
/// credited to this one, and two links to the same Validator inside one window
/// cannot each claim the same row (issue #221, stories 65, 66).
const VALIDATOR_HORIZON_SQL: &str = "\
    SELECT MAX(s.sample_at) AS last_sample_at, \
           MAX(s.received_at) AS last_received_at \
      FROM validator_daily_snapshots s \
      JOIN node_validator_links l ON l.validator_id = s.validator_id \
       AND s.received_at >= l.valid_from \
       AND (l.valid_until IS NULL OR s.received_at < l.valid_until) \
     WHERE l.node_id = ? AND l.valid_from < ? AND (l.valid_until IS NULL OR l.valid_until > ?) \
       AND s.timezone = ?";

/// SQL: the days and rows those snapshots hold inside the window's local days.
///
/// Bounded by the same link correspondence as the horizon above, so the days counted
/// here are the days the Server read this Validator for this Node.
const VALIDATOR_GRID_SQL: &str = "\
    SELECT COUNT(*) AS entries, COUNT(DISTINCT s.local_date) AS days, \
           MIN(s.sample_at) AS first_sample_at, MAX(s.sample_at) AS last_sample_at \
      FROM validator_daily_snapshots s \
      JOIN node_validator_links l ON l.validator_id = s.validator_id \
       AND s.received_at >= l.valid_from \
       AND (l.valid_until IS NULL OR s.received_at < l.valid_until) \
     WHERE l.node_id = ? AND l.valid_from < ? AND (l.valid_until IS NULL OR l.valid_until > ?) \
       AND s.timezone = ? AND s.local_date >= ? AND s.local_date <= ?";

/// The seconds in one day, for the Validator source's day grid.
const DAY_SECONDS: i64 = 86_400;

/// The local calendar date one instant falls in.
fn local_date(timezone: &str, instant: OffsetDateTime) -> Result<String, sqlx::Error> {
    crate::validator::local_date_at(timezone, instant).map_err(|error| {
        sqlx::Error::Protocol(format!(
            "the configured Validator timezone {timezone:?} cannot date {instant}: {error}"
        ))
    })
}

/// How many local days a pair of local dates covers, inclusive.
fn local_days_between(from: &str, to: &str) -> Option<i64> {
    let from = chrono::NaiveDate::parse_from_str(from, "%Y-%m-%d").ok()?;
    let to = chrono::NaiveDate::parse_from_str(to, "%Y-%m-%d").ok()?;
    Some(to.signed_duration_since(from).num_days() + 1)
}

/// How one recorded Validator link reads in a note.
///
/// A Node Validator Link is the Server's own record of the stretch it read a
/// Validator for this Node. Its edges are correspondence times — when the Server
/// began and stopped treating the link as current — rather than the instant a key
/// changed on chain, and it is not proof that the Node was continuously secured by
/// that Validator between them (issue #221, stories 65, 66).
fn link_description(link: &ValidatorLinkRow) -> String {
    let role = link.role.clone().unwrap_or_else(|| "unassigned".to_owned());
    match &link.valid_until {
        Some(valid_until) => format!(
            "the Server recorded {} ({role}, {}) as linked from {} to {valid_until}",
            link.validator_id, link.origin, link.valid_from
        ),
        None => format!(
            "the Server recorded {} ({role}, {}) as linked from {} and still holds it",
            link.validator_id, link.origin, link.valid_from
        ),
    }
}

/// The stretch of the window one recorded link covers, when it covers any.
fn link_span(
    link: &ValidatorLinkRow,
    window: &ResolvedWindow,
) -> Option<(OffsetDateTime, OffsetDateTime)> {
    clipped_span(
        canonical_instant(&link.valid_from)?,
        link.valid_until.as_deref().and_then(canonical_instant),
        window.from,
        window.to,
    )
}

/// Measure the Validator snapshots a Node's links answer with.
///
/// The snapshot is written once per local day per Validator, so the grain is the
/// local day grid of the configured timezone. The day-level authority stays the
/// Validator trend engine, which also discloses rows recorded in other
/// timezones; this source states the days it can answer, and for which Validators.
async fn validator_source(
    ctx: &SourceContext<'_>,
) -> Result<InvestigationSourceResponse, sqlx::Error> {
    let (from, to) = (
        format_rfc3339(ctx.window.from),
        format_rfc3339(ctx.window.to),
    );
    let links = sqlx::query_as::<_, ValidatorLinkRow>(VALIDATOR_LINK_SQL)
        .bind(ctx.node_id)
        .bind(&to)
        .bind(&from)
        .fetch_all(ctx.pool)
        .await?;

    let mut builder = SourceBuilder::new(
        SOURCE_VALIDATOR,
        "Validator daily snapshots",
        "node",
        ctx.node_id,
        TimeBasis::ValidatorSnapshotSource,
        VALIDATOR_GRACE_SECONDS,
    );
    if links.is_empty() {
        // Holding no link is not the same as the Node never having had one: the
        // links this Server keeps are its own prospective record, so the absence is
        // named as an absence of record and nothing is back-filled from the
        // relation the Node has now (issue #221, stories 61, 66).
        builder = builder.note(
            "the Server holds no link interval for this Node inside this window, and holding no record of a link is not evidence that the Node had none: this family states what the Server recorded and never what the current link implies about the past",
        );
        builder.unsupported = Some(
            "no Validator is linked to this Node for any part of this window, so the Server holds no Validator snapshots for it"
                .to_owned(),
        );
        return Ok(builder.finish(ctx.window));
    }

    let local_from = local_date(ctx.timezone, ctx.window.from)?;
    let local_to = local_date(ctx.timezone, ctx.window.to)?;
    let expected_days = local_days_between(&local_from, &local_to).unwrap_or(1);
    let horizon = sqlx::query_as::<_, ValidatorHorizonRow>(VALIDATOR_HORIZON_SQL)
        .bind(ctx.node_id)
        .bind(&to)
        .bind(&from)
        .bind(ctx.timezone)
        .fetch_one(ctx.pool)
        .await?;
    let grid = sqlx::query_as::<_, ValidatorGridRow>(VALIDATOR_GRID_SQL)
        .bind(ctx.node_id)
        .bind(&to)
        .bind(&from)
        .bind(ctx.timezone)
        .bind(&local_from)
        .bind(&local_to)
        .fetch_one(ctx.pool)
        .await?;

    let retention_days = ctx.policies.days(FAMILY_VALIDATOR_DAILY_SNAPSHOT);
    let retained = retained_from(ctx.now, None, retention_days);
    // The source starts when its earliest link starts: before that instant the
    // Node had no Validator to read, so the window is not judged as a silence.
    let ledger = Ledger {
        first_observed_at: links
            .iter()
            .filter_map(|link| canonical_instant(&link.valid_from))
            .min(),
        last_observed_at: horizon
            .last_sample_at
            .as_deref()
            .and_then(canonical_instant),
        last_received_at: horizon
            .last_received_at
            .as_deref()
            .and_then(canonical_instant),
        released_before: None,
    };

    builder.ledger = ledger.clone();
    builder.retained_from = retained;
    builder.retention_days = retention_days;
    builder = builder.note(format!(
        "the day grid is the {local} calendar the Server is configured with, from {local_from} to {local_to}; the Validator trend engine is the day-level authority and also discloses rows recorded in other timezones",
        local = ctx.timezone
    ));
    builder = builder.note(
        "a Validator link is the Server's own record of when it began and stopped reading a Validator for this Node: its edges are correspondence times rather than key-change times, and it proves no continuous protection between them",
    );
    for link in &links {
        builder = builder.note(link_description(link));
        let span = link_span(link, ctx.window);
        if let Some((link_from, link_to)) = span {
            builder
                .related_subjects
                .push(InvestigationRelatedSubjectResponse {
                    subject_kind: "validator".to_owned(),
                    subject: link.validator_id.clone(),
                    role: SUBJECT_ROLE_RELATED.to_owned(),
                    basis: BASIS_RECORDED_VALIDATOR_LINK.to_owned(),
                    basis_label: basis_label(BASIS_RECORDED_VALIDATOR_LINK).to_owned(),
                    from: format_rfc3339(link_from),
                    to: format_rfc3339(link_to),
                    detail: link_description(link),
                });
        }
        // The trend is asked for the stretch this link was recorded for rather than
        // for the whole window: the days outside the correspondence were read under
        // another link, or under none, and asking for them here would present them
        // as this link's (issue #221, stories 65, 66).
        let (asked_from, asked_to) = span.unwrap_or((ctx.window.from, ctx.window.to));
        builder = builder.answer(
            "Validator daily trend",
            format!(
                "/api/admin/v1/validators/{}/trend?from={}&to={}",
                link.validator_id,
                format_rfc3339(asked_from),
                format_rfc3339(asked_to)
            ),
            span.map(|_| {
                "only the stretch the Server recorded this link for is asked, not the whole window"
            }),
        );
    }
    builder = builder.grain(tier_grain(
        GRAIN_VALIDATOR_DAY,
        Some(DAY_SECONDS),
        (ctx.window.from, ctx.window.to),
        TierStats {
            point_count: grid.days,
            sample_count: grid.entries,
            first_observed_at: grid.first_sample_at,
            last_observed_at: grid.last_sample_at,
            cadence_seconds: Some(DAY_SECONDS),
            longest_gap_seconds: None,
        },
        Some(expected_days),
        DAY_SECONDS,
        None,
    ));
    if let Some(retained) = retained {
        builder = builder.note(retention_detail(retained, None, retention_days));
    }
    Ok(builder.finish(ctx.window))
}

// --------------------------------------------------------- orchestration ---

/// The Node an investigation is about.
const NODE_SQL: &str = "\
    SELECT node_id, agent_id, display_name, lifecycle, visibility, first_seen_at \
      FROM nodes WHERE node_id = ?";

/// Whether a Node id belongs to a Purge rather than to nothing at all.
const DELETED_NODE_SQL: &str = "\
    SELECT deleted_at FROM deleted_nodes WHERE node_id = ? ORDER BY deleted_at DESC LIMIT 1";

/// The collector rows of one Node and its Agent.
const COMPONENT_SQL: &str = "\
    SELECT scope, scope_key, component_key, state, error_code, observed_at, received_at \
      FROM component_status \
     WHERE (scope = 'node' AND scope_key = ?) OR (scope = 'host' AND agent_id = ?) \
     ORDER BY scope ASC, component_key ASC";

/// The protection pauses that overlap a window, in either scope.
const PAUSE_SQL: &str = "\
    SELECT s.scope_kind AS scope, p.started_at AS started_at, p.ended_at AS ended_at, \
           s.metric AS series, s.skipped_count AS skipped_count, \
           s.first_skipped_at AS first_skipped_at, s.last_skipped_at AS last_skipped_at \
      FROM capacity_skipped_series s \
      JOIN capacity_protection_intervals p ON p.interval_id = s.interval_id \
     WHERE s.last_skipped_at >= ? AND s.first_skipped_at <= ? \
       AND ((s.scope_kind = 'node' AND s.scope_key = ?) \
            OR (s.scope_kind = 'host' AND s.scope_key = ?)) \
     ORDER BY s.first_skipped_at ASC LIMIT ?";

/// How many pauses one investigation lists.
const PAUSE_ROW_LIMIT: i64 = 512;

/// One protection pause as the Server stores it.
#[derive(Debug, FromRow)]
struct PauseRow {
    scope: String,
    started_at: String,
    ended_at: Option<String>,
    series: String,
    skipped_count: i64,
    first_skipped_at: String,
    last_skipped_at: String,
}

impl PauseRow {
    fn to_pause(&self) -> Option<Pause> {
        Some(Pause {
            scope: self.scope.clone(),
            started_at: canonical_instant(&self.started_at)?,
            ended_at: self.ended_at.as_deref().and_then(canonical_instant),
            first_skipped_at: canonical_instant(&self.first_skipped_at)?,
            last_skipped_at: canonical_instant(&self.last_skipped_at)?,
            series: self.series.clone(),
            skipped_count: self.skipped_count,
        })
    }
}

/// Every pause the Server refused optional history during, for one Node.
async fn load_pauses(
    pool: &SqlitePool,
    node_id: &str,
    agent_id: &str,
    window: &ResolvedWindow,
) -> Result<Vec<Pause>, sqlx::Error> {
    let rows = sqlx::query_as::<_, PauseRow>(PAUSE_SQL)
        .bind(format_rfc3339(window.from))
        .bind(format_rfc3339(window.to))
        .bind(node_id)
        .bind(agent_id)
        .bind(PAUSE_ROW_LIMIT)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().filter_map(PauseRow::to_pause).collect())
}

/// Answer one Node's history over one window.
///
/// The sources are read in a fixed order and answered as one object: each keeps
/// its own time basis, its own coverage and its own boundaries, because the point
/// of the answer is that a window is not one uninterrupted series. What the
/// caller gets is where the evidence is; the evidence itself stays with each
/// family's own endpoint (design §11.4, story 54-59).
pub async fn investigate_node(
    pool: &SqlitePool,
    node_id: &str,
    window: &ResolvedWindow,
    now: OffsetDateTime,
    timezone: &str,
) -> Result<InvestigationResponse, InvestigationError> {
    let Some(node) = sqlx::query_as::<_, NodeRow>(NODE_SQL)
        .bind(node_id)
        .fetch_optional(pool)
        .await?
    else {
        let deleted_at: Option<String> = sqlx::query_scalar(DELETED_NODE_SQL)
            .bind(node_id)
            .fetch_optional(pool)
            .await?;
        return Err(match deleted_at {
            Some(deleted_at) => InvestigationError::Purged { deleted_at },
            None => InvestigationError::NotFound,
        });
    };

    let components = sqlx::query_as::<_, ComponentRow>(COMPONENT_SQL)
        .bind(node.node_id.as_str())
        .bind(node.agent_id.as_str())
        .fetch_all(pool)
        .await?;
    let pauses = load_pauses(pool, &node.node_id, &node.agent_id, window).await?;
    let policies = RetentionPolicies::from_rows(&crate::retention::list_policies(pool).await?);
    // A Node whose first sighting the Server cannot date is read as if it had
    // existed for the whole window: claiming a pre-enablement boundary from an
    // unusable instant would invent a boundary out of a parse failure.
    let first_seen_at = canonical_instant(&node.first_seen_at).unwrap_or(window.from);

    let ctx = SourceContext {
        pool,
        window,
        now,
        node_id: &node.node_id,
        agent_id: &node.agent_id,
        first_seen_at,
        timezone,
        components: &components,
        pauses: &pauses,
        policies: &policies,
    };

    // The relations the whole answer is read against: the Agent and Network the
    // Server itself wrote down, per stretch of the window (issue #221). Sources that
    // attribute evidence to a related subject read their stretches from here rather
    // than from the keys the Node happens to have now.
    // Every interval the Server holds for this Node is read, not only the ones that
    // fall inside the window: an interval that opened exactly at the window's end is
    // still the relation the Node had while the window was open, and the record's own
    // start is what lets this answer say when the Server began recording at all.
    let intervals = load_intervals(pool, &node.node_id).await?;
    let relations = RecordedRelations::derive(intervals.clone(), window.from, window.to);

    let mut sources = Vec::with_capacity(SOURCE_ORDER.len());
    sources.push(relationships_source(&ctx, &relations, &intervals));
    sources.push(
        metric_source(
            &ctx,
            &NODE_HISTORY,
            SourceBuilder::new(
                SOURCE_NODE_METRICS,
                "Node metrics",
                "node",
                &node.node_id,
                TimeBasis::MetricObservation,
                METRIC_GRACE_SECONDS,
            ),
            &Collectors::of(&[
                ComponentKey::Process,
                ComponentKey::DataDirectorySizeBytes,
                ComponentKey::DataDirectoryCapacityBytes,
            ]),
            &NODE_METRIC_SERIES,
            "Node metric history",
            format!(
                "/api/admin/v1/nodes/{}/metric-history?from={}&to={}",
                node.node_id,
                window.from_rfc3339(),
                window.to_rfc3339()
            ),
        )
        .await?,
    );
    sources.push(state_source(&ctx).await?);
    sources.push(
        metric_source(
            &ctx,
            &HOST_HISTORY,
            SourceBuilder::new(
                SOURCE_HOST_METRICS,
                "Host metrics",
                "agent",
                &node.agent_id,
                TimeBasis::MetricObservation,
                METRIC_GRACE_SECONDS,
            )
            // Host metrics are collected once per Agent and read by Agent key, so
            // the stretches the records gave to another Agent are not this family's
            // evidence and are named as unknown instead of being attributed here.
            .attribute_own_relation(relations.view(RELATION_AGENT), "agent"),
            &Collectors::of(&[
                ComponentKey::CpuPercent,
                ComponentKey::Memory,
                ComponentKey::Load,
                ComponentKey::Disk,
                ComponentKey::NetworkThroughput,
                ComponentKey::ClockSkew,
                ComponentKey::Spool,
            ]),
            &HOST_METRIC_SERIES,
            "Host metric history",
            format!(
                "/api/admin/v1/agents/{}/metric-history?from={}&to={}",
                node.agent_id,
                window.from_rfc3339(),
                window.to_rfc3339()
            ),
        )
        .await?,
    );
    sources.push(peers_source(&ctx).await?);
    sources.push(incidents_source(&ctx, &relations).await?);
    sources.push(validator_source(&ctx).await?);
    debug_assert_eq!(sources.len(), SOURCE_ORDER.len());

    let mut notes = Vec::new();
    if window.clamped_to_now {
        notes.push(format!(
            "the requested window ended in the future and was clipped to {}",
            format_rfc3339(window.answered_at)
        ));
    }
    if !pauses.is_empty() {
        notes.push(format!(
            "{} optional series were refused while the Server protected a nearly full disk; those stretches are reported as pauses rather than as collection failures",
            pauses.len()
        ));
    }
    notes.push(
        "each source answers as of its own time basis and reports its own boundaries; a source that holds nothing says so instead of reporting zero".to_owned(),
    );

    Ok(InvestigationResponse {
        node_id: node.node_id,
        agent_id: node.agent_id,
        display_name: node.display_name,
        lifecycle: node.lifecycle,
        visibility: node.visibility,
        window: window.to_response(),
        sources,
        components: components.iter().map(ComponentRow::to_response).collect(),
        notes,
    })
}
