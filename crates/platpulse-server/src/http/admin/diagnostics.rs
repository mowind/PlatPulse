//! Agent, Host, Node and Peer diagnostic DTOs and the builders that derive them.
use super::*;
use crate::node_currency;

/// Redacted Agent credential summary. Only the non-sensitive credential
/// id and lifecycle instants are exposed; the credential secret itself is
/// never stored by the Server and never appears in any Admin DTO.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentCredentialSummary {
    pub credential_id: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
    /// Rotation overlap deadline; the credential stops authenticating at
    /// this instant even without an explicit revoke.
    pub revoke_after: Option<String>,
    /// Server-computed validity: not revoked and not past `revoke_after`.
    pub active: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentDiagnostic {
    pub agent_id: String,
    /// Owner-editable Server-owned display name (design §15.2). `None` means
    /// the WebUI identifies the Agent by its stable ID.
    pub display_name: Option<String>,
    /// Owner-editable Server-owned notes.
    pub notes: Option<String>,
    pub agent_epoch: i64,
    pub last_report_sequence: Option<i64>,
    pub active_boot_id: Option<String>,
    pub boot_status: String,
    pub previous_boot_id: Option<String>,
    pub close_report_id: Option<String>,
    pub shutdown_state: String,
    pub shutdown_started_at: Option<String>,
    pub shutdown_deadline_at: Option<String>,
    pub shutdown_finished_at: Option<String>,
    pub shutdown_unresolved_range: Option<(i64, i64)>,
    pub shutdown_last_error: Option<String>,
    pub shutdown_forced: bool,
    pub shutdown_report_id: Option<String>,
    pub shutdown_report_sequence: Option<i64>,
    pub shutdown_updated_at: Option<String>,
    pub sequence_gap_count: i64,
    pub security_event_count: i64,
    pub clock_status: String,
    pub clock_skew_ms: Option<i64>,
    pub liveness: String,
    pub last_received_at: Option<String>,
    pub capabilities: Vec<String>,
    /// Credential state as a separate dimension (design: identity,
    /// liveness, boot/report, inventory, credentials, diagnostics).
    pub credentials: Vec<AgentCredentialSummary>,
    pub host: Option<HostDiagnostic>,
    pub nodes: Vec<NodeDiagnostic>,
    /// Server-owned Inventory diagnosis (issue #181): the revision/hash the
    /// Server currently accepts, and the Agent's latest Inventory-caused
    /// rejection. Node `inventory_revision` is the per-Node accepted revision;
    /// this is the Agent-wide Inventory declaration.
    pub inventory: AgentInventoryDiagnostic,
    /// Server-owned, currently unacknowledged Agent Attention Items with
    /// their occurrence/evidence boundaries (issue #172). The Agent Detail
    /// page renders these instead of rebuilding warning predicates from
    /// cumulative counters, so an acknowledged occurrence stays suppressed
    /// while raw evidence remains in Diagnostics.
    pub attention: Vec<AttentionItem>,
}

/// What the Server currently accepts for one Agent's Node Inventory, and which
/// declaration it refused last (issue #181). The pair is what makes the remedy
/// obvious: the Agent declares revision `reported_revision` with hash
/// `reported_sha256`, while the Server accepts `accepted_revision` with
/// `accepted_sha256`.
///
/// The declaration protocol is Server-recorded (issue #188). A v2
/// (Server-managed) declaration carries no Agent-assigned revision, so it is
/// explained from the Server-assigned revision/canonical fingerprint plus the
/// actual refusal evidence, never from a fabricated Agent revision. An Agent
/// whose declaration was never accepted has no accepted revision at all, which
/// is distinct from an accepted empty Inventory.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentInventoryDiagnostic {
    /// The revision of the currently accepted Node Inventory: Server-assigned
    /// for a v2 declaration, Agent-declared for v1. `None` means the Server has
    /// never accepted a declaration from this Agent.
    pub accepted_revision: Option<i64>,
    /// Content hash (v1) or canonical declaration fingerprint (v2) of that
    /// accepted Inventory. `None` means no declaration was ever accepted.
    pub accepted_sha256: Option<String>,
    /// Which protocol produced the accepted declaration: `server_managed`
    /// (v2), `agent_declared` (v1), or `unknown` when nothing has been
    /// accepted or the evidence predates this field.
    pub accepted_declaration: String,
    /// The refusal that stopped the Agent's most recent report, when that
    /// report was refused because of its Inventory. `None` means the latest
    /// ingestion attempt was not an Inventory rejection.
    pub last_rejection: Option<AgentInventoryRejectionEvidence>,
}

/// Server-recorded evidence of one whole-report Inventory rejection. All of it
/// is Server-computed or Server-stored; nothing here is Agent-reported
/// diagnostics.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AgentInventoryRejectionEvidence {
    /// Stable rejection code: `inventory_revision_conflict` or
    /// `network_key_unknown`.
    pub code: String,
    /// Which protocol produced the refused declaration: `server_managed` (v2)
    /// or `agent_declared` (v1). A v2 refusal declares no revision.
    pub declaration: String,
    /// The Inventory revision the refused report declared. `None` for a v2
    /// (Server-managed) declaration, which declares no revision.
    pub reported_revision: Option<i64>,
    /// The Inventory content hash (v1) or canonical declaration fingerprint
    /// (v2) the refused report declared.
    pub reported_sha256: Option<String>,
    /// When the Server stored the refusing Report Receipt.
    pub received_at: Option<String>,
}

/// The Admin Inventory diagnosis plus the raw protocol major of the refused
/// declaration, which the shared Attention evidence reuses so the page and the
/// item can never disagree about how to explain the same refusal.
struct InventoryDiagnosis {
    diagnostic: AgentInventoryDiagnostic,
    rejected_protocol_major: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct HostProjectionRow {
    cpu_percent: Option<f64>,
    memory_total_bytes: Option<i64>,
    memory_used_bytes: Option<i64>,
    load1: Option<f64>,
    load5: Option<f64>,
    load15: Option<f64>,
    network_rx_bytes_per_sec: Option<i64>,
    network_tx_bytes_per_sec: Option<i64>,
    spool_queued_bytes: Option<i64>,
    spool_queued_reports: Option<i64>,
    spool_oldest_queued_age_ms: Option<i64>,
    spool_in_flight: Option<i64>,
    spool_last_delivery_error: Option<String>,
    spool_last_delivery_at: Option<String>,
    spool_capacity_bytes: Option<i64>,
    spool_max_age_seconds: Option<i64>,
    spool_dropped_sequence_from: Option<i64>,
    spool_dropped_sequence_to: Option<i64>,
    spool_dropped_time_from: Option<String>,
    spool_dropped_time_to: Option<String>,
    spool_dropped_height_from: Option<i64>,
    spool_dropped_height_to: Option<i64>,
    spool_pending_history_gaps: Option<i64>,
    spool_report_too_large: Option<i64>,
    spool_store_fatal: Option<i64>,
    spool_store_error: Option<String>,
    spool_backlog_age_ms: Option<i64>,
    spool_backlog_age_basis: Option<String>,
    updated_at: String,
}

/// A derived Spool Backlog Age and the clock basis it is anchored on (ADR
/// 0014, issue #272).
///
/// The Server derives the age when it receives the Report that carries the
/// current Spool observation, because an immutable Report can only describe the
/// backlog as of its own generation instant. The basis travels with the number
/// so the WebUI can label it instead of presenting both bases as one reading.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct SpoolBacklogAgeDiagnostic {
    /// Milliseconds since the oldest queued Report was generated, never
    /// negative.
    pub age_ms: i64,
    /// `receipt` when the reading is anchored on the Server receipt instant and
    /// corrected by the measured clock skew, `generation` when the clock was
    /// unreliable and the Report's own age is shown instead.
    ///
    /// A plain string rather than a closed enum: the basis is a label, and a
    /// client that meets one a newer Server introduced must render it as an
    /// unknown label instead of failing to parse the whole reading.
    pub basis: String,
}

impl SpoolBacklogAgeDiagnostic {
    /// Build the reading from the stored age/basis pair. The two columns are
    /// written together and cleared together, so a row with only one of them
    /// carries no reading rather than half of one.
    fn from_columns(age_ms: Option<i64>, basis: Option<String>) -> Option<Self> {
        age_ms
            .zip(basis)
            .map(|(age_ms, basis)| Self { age_ms, basis })
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct HostDiagnostic {
    pub cpu_percent: Option<f64>,
    pub memory_total_bytes: Option<i64>,
    pub memory_used_bytes: Option<i64>,
    pub load1: Option<f64>,
    pub load5: Option<f64>,
    pub load15: Option<f64>,
    pub network_rx_bytes_per_sec: Option<i64>,
    pub network_tx_bytes_per_sec: Option<i64>,
    pub spool_queued_bytes: Option<i64>,
    pub spool_queued_reports: Option<i64>,
    pub spool_oldest_queued_age_ms: Option<i64>,
    pub spool_in_flight: Option<bool>,
    pub spool_last_delivery_error: Option<String>,
    pub spool_last_delivery_at: Option<String>,
    pub spool_capacity_bytes: Option<i64>,
    pub spool_max_age_seconds: Option<i64>,
    pub spool_dropped_sequence_from: Option<i64>,
    pub spool_dropped_sequence_to: Option<i64>,
    pub spool_dropped_time_from: Option<String>,
    pub spool_dropped_time_to: Option<String>,
    pub spool_dropped_height_from: Option<i64>,
    pub spool_dropped_height_to: Option<i64>,
    pub spool_pending_history_gaps: Option<i64>,
    pub spool_report_too_large: Option<bool>,
    pub spool_store_fatal: Option<bool>,
    pub spool_store_error: Option<String>,
    /// Spool Backlog Age derived when the Server received the Report that
    /// carried this Spool observation (ADR 0014, issue #272); `None` when the
    /// Agent has never reported its Durable Spool or the current observation
    /// carries none, which is never reported as zero.
    pub spool_backlog_age: Option<SpoolBacklogAgeDiagnostic>,
    pub updated_at: String,
    pub components: Vec<HostComponentDiagnostic>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct HostComponentDiagnostic {
    pub component: String,
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProcessDiagnostic {
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
    pub pid: Option<i64>,
    pub started_at: Option<String>,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<i64>,
    pub uptime_ms: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct DataDirectoryDiagnostic {
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
    pub size_bytes: Option<i64>,
    pub capacity_bytes: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct RpcDiagnostic {
    pub client_version: Option<String>,
    pub namespaces: Vec<String>,
    pub methods: Vec<String>,
    pub state: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: Option<i64>,
    pub value_revision: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct SyncDiagnostic {
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
    pub syncing: Option<bool>,
    pub current_block: Option<i64>,
    pub highest_block: Option<i64>,
    pub pulled_states: Option<i64>,
    pub known_states: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ConsensusDiagnostic {
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
    pub epoch: Option<i64>,
    pub view_number: Option<i64>,
    pub validator: Option<bool>,
    pub highest_qc_block: Option<i64>,
    pub highest_lock_block: Option<i64>,
    pub highest_commit_block: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PeerDiagnosticEntry {
    pub peer_id: String,
    pub direction: String,
    pub trusted: bool,
    pub static_peer: bool,
    pub consensus_peer: bool,
    pub client_name: Option<String>,
    pub capabilities: Vec<String>,
    pub cbft_protocol_version: Option<i64>,
    pub cbft_highest_qc_block: Option<i64>,
    pub cbft_locked_block: Option<i64>,
    pub cbft_commit_block: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PeerPresenceInterval {
    pub peer_id: String,
    /// Server-observed arrival boundary of this connected interval.
    pub opened_at: String,
    /// Server-observed departure boundary; `None` means the interval is open.
    pub closed_at: Option<String>,
    /// Closed interval duration. Open intervals intentionally have no
    /// duration because their end boundary is not known yet.
    pub duration_seconds: Option<i64>,
    pub direction: String,
    pub trusted: bool,
    pub static_peer: bool,
    pub consensus_peer: bool,
    pub client_name: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PeerChurnDiagnostic {
    /// `unknown` means no successful Peer Snapshot exists; `empty` means a
    /// successful snapshot exists but no retained interval is available;
    /// `error` means the latest collection failed while retained intervals
    /// and the last-good value remain available.
    pub state: String,
    pub freshness: String,
    pub window_start: String,
    pub total_open_intervals: i64,
    pub recent_arrivals: Vec<PeerPresenceInterval>,
    pub recent_departures: Vec<PeerPresenceInterval>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminPeerHistory {
    /// Aggregate collection state; retained last-good rows remain visible
    /// when the latest Peer collection is in error.
    pub state: String,
    pub freshness: String,
    /// The fixed newest tail of five-minute buckets, answered when no range was
    /// asked for.
    pub five_minute: Vec<AdminPeerAggregate>,
    /// The fixed newest tail of hourly buckets, answered when no range was asked
    /// for.
    pub hourly: Vec<AdminPeerAggregate>,
    /// The bounded range this answer covers, when one was asked for: a ranged
    /// read fills this and leaves both tails empty, so one response never means
    /// two things.
    pub range: Option<AdminPeerRange>,
}

/// One bounded read of a Node's Peer receipt buckets (issue #220).
///
/// Receipt buckets are what the Server actually accepted, so a range says how
/// many of them it holds, where the grain's own grid expects more of them, and
/// which older page comes next. A fixed top list silently leaves everything
/// older than its limit unreadable, and a window that cannot be paged cannot
/// rule that out.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminPeerRange {
    /// `5m` or `1h`.
    pub grain: String,
    pub grain_seconds: i64,
    /// Start of the answered range: a bucket belongs to it when its start is at
    /// or after `from` and before `to`.
    pub from: String,
    pub to: String,
    /// The page, newest bucket first.
    pub buckets: Vec<AdminPeerAggregate>,
    /// How many buckets the range holds in total, whatever the page limit is.
    pub matching: i64,
    pub returned: i64,
    /// The oldest and newest bucket the range holds, when it holds any.
    pub first_bucket: Option<String>,
    pub last_bucket: Option<String>,
    /// Aligned buckets the Node's own stretch expects inside the range and the
    /// Server holds none for: a receipt bucket exists only when a Report carried
    /// Peer evidence, so each of them is a receipt that never arrived. The grid
    /// starts at the oldest bucket the grain ever recorded, so the era before a
    /// Node's first successful snapshot is never counted here.
    pub missing_buckets: i64,
    /// `complete`, `partial`, `empty` or `unavailable`, the vocabulary the
    /// investigation window uses.
    pub coverage: String,
    /// True when the range holds more buckets than the page limit returned.
    pub truncated: bool,
    /// Pass this as `before` for the next, older page while `truncated`: a
    /// bucket start is unique inside a Node and grain, so paging at a tie cannot
    /// skip or repeat a bucket.
    pub continuation: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminPeerAggregate {
    pub bucket_start: String,
    pub last_observed_at: String,
    pub sample_count: i64,
    pub total_peers: i64,
    pub average_peers: Option<f64>,
    pub inbound_count: i64,
    pub outbound_count: i64,
    pub trusted_count: i64,
    pub static_count: i64,
    pub consensus_count: i64,
    pub known_country_count: i64,
    pub unknown_country_count: i64,
    pub countries: Vec<AdminPeerCountryCount>,
    pub arrivals: i64,
    pub departures: i64,
    pub cbft_lag: AdminPeerLagSummary,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminPeerCountryCount {
    pub country_code: String,
    pub count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminPeerLagSummary {
    pub sample_count: i64,
    pub minimum: Option<i64>,
    pub average: Option<f64>,
    pub maximum: Option<i64>,
}

fn admin_peer_aggregate(row: crate::peer_history::PeerAggregateRow) -> AdminPeerAggregate {
    let average_peers =
        (row.sample_count > 0).then(|| row.total_peers as f64 / row.sample_count as f64);
    let average_lag =
        (row.cbft_lag_count > 0).then(|| row.cbft_lag_sum as f64 / row.cbft_lag_count as f64);
    AdminPeerAggregate {
        bucket_start: row.bucket_start,
        last_observed_at: row.last_observed_at,
        sample_count: row.sample_count,
        total_peers: row.total_peers,
        average_peers,
        inbound_count: row.inbound_count,
        outbound_count: row.outbound_count,
        trusted_count: row.trusted_count,
        static_count: row.static_count,
        consensus_count: row.consensus_count,
        known_country_count: row.known_country_count,
        unknown_country_count: row.unknown_country_count,
        countries: row
            .countries
            .into_iter()
            .map(|country| AdminPeerCountryCount {
                country_code: country.country_code,
                count: country.count,
            })
            .collect(),
        arrivals: row.arrivals,
        departures: row.departures,
        cbft_lag: AdminPeerLagSummary {
            sample_count: row.cbft_lag_count,
            minimum: row.cbft_lag_min,
            average: average_lag,
            maximum: row.cbft_lag_max,
        },
    }
}

pub(super) fn admin_peer_history(history: crate::peer_history::PeerHistory) -> AdminPeerHistory {
    AdminPeerHistory {
        state: history.state,
        freshness: history.freshness,
        five_minute: history
            .five_minute
            .into_iter()
            .map(admin_peer_aggregate)
            .collect(),
        hourly: history
            .hourly
            .into_iter()
            .map(admin_peer_aggregate)
            .collect(),
        range: None,
    }
}

/// One bounded Peer receipt range as the answer to the Peer history route.
///
/// The ranged read fills `range` and leaves both fixed tails empty rather than
/// reusing one of them, so a caller always knows whether it is reading the newest
/// tail or the range it asked for.
pub(super) fn admin_peer_range_answer(
    answer: crate::peer_history::PeerRangeAnswer,
) -> AdminPeerHistory {
    AdminPeerHistory {
        state: answer.state,
        freshness: answer.freshness,
        five_minute: Vec::new(),
        hourly: Vec::new(),
        range: Some(AdminPeerRange {
            grain: answer.grain.to_owned(),
            grain_seconds: answer.grain_seconds,
            from: answer.from,
            to: answer.to,
            returned: answer.buckets.len() as i64,
            buckets: answer
                .buckets
                .into_iter()
                .map(admin_peer_aggregate)
                .collect(),
            matching: answer.matching,
            first_bucket: answer.first_bucket,
            last_bucket: answer.last_bucket,
            missing_buckets: answer.missing_buckets,
            coverage: answer.coverage.to_owned(),
            truncated: answer.truncated,
            continuation: answer.continuation,
        }),
    }
}

const PEER_CHURN_LIMIT: i64 = 128;
const PEER_CHURN_WINDOW_HOURS: i64 = 24;

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PeerDiagnostic {
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
    /// Server-owned freshness of the latest successful Peer value.
    pub freshness: String,
    /// `None` means no successful Peer Snapshot has ever been observed;
    /// `Some(0)` is an authoritative successful empty snapshot.
    pub peer_count: Option<i64>,
    pub inbound_count: Option<i64>,
    pub outbound_count: Option<i64>,
    pub trusted_count: Option<i64>,
    pub static_count: Option<i64>,
    pub consensus_count: Option<i64>,
    pub peers: Vec<PeerDiagnosticEntry>,
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ComponentDiagnostic {
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
}

const FIRST_OBSERVATION_GRACE_SECONDS: i64 = 120;

/// A Node the Server first saw this recently and that is still Never
/// Observed is awaiting its first accepted observation: its Unknown Health is
/// expected and raises no Attention Item yet.
pub(super) fn first_observation_in_grace(first_seen_at: &str, node: &AssessedNode) -> bool {
    let Some(first_seen) = crate::auth::parse_rfc3339(first_seen_at) else {
        return false;
    };
    let age = (node.assessed_at - first_seen).whole_seconds();
    (0..=FIRST_OBSERVATION_GRACE_SECONDS).contains(&age)
        && node.currency == node_currency::NodeCurrency::NeverObserved
}

/// Freshness of one retained value: `current` when the newest instant is
/// inside the window, `stale` when it is older, and `unknown` when there is
/// none. The instants are Server-clock observation instants (ADR 0014).
fn derive_freshness<'a>(observed_at: impl Iterator<Item = &'a str>) -> &'static str {
    let latest = observed_at.filter_map(crate::auth::parse_rfc3339).max();
    match latest {
        None => "unknown",
        Some(value)
            if crate::observation_time::is_fresh_at(
                value,
                crate::auth::now_utc(),
                crate::observation_time::FRESHNESS_LIMIT_SECONDS,
            ) =>
        {
            "current"
        }
        Some(_) => "stale",
    }
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NodeDiagnostic {
    pub node_id: String,
    pub network_key: String,
    pub display_name: Option<String>,
    pub lifecycle: String,
    pub inventory_revision: i64,
    pub visibility: String,
    pub health: String,
    pub health_reason: String,
    /// The Node Currency judged from the RPC, sync and consensus observations.
    /// The WebUI formats this state; it never derives it from `Date.now()`.
    pub currency: node_currency::NodeCurrencyState,
    /// The oldest of the RPC, sync and consensus Server-clock observation
    /// instants; null while any of them was never observed.
    pub currency_as_of: Option<String>,
    pub process: Option<ProcessDiagnostic>,
    pub data_directory: Option<DataDirectoryDiagnostic>,
    pub rpc: Option<RpcDiagnostic>,
    pub sync: Option<SyncDiagnostic>,
    pub consensus: Option<ConsensusDiagnostic>,
    pub peers: Option<PeerDiagnostic>,
    pub current_head: Option<i64>,
    pub historical_high_watermark: Option<i64>,
    pub resync_state: String,
    pub network_reference_head: Option<i64>,
    pub network_reference_confidence: String,
    pub resync_progress: Option<String>,
}

async fn process_diagnostic(
    state: &AppState,
    node_id: &str,
) -> Result<Option<ProcessDiagnostic>, sqlx::Error> {
    Ok(sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64, Option<i64>, Option<String>, Option<f64>, Option<i64>, Option<i64>)>(
        "SELECT s.state, s.error_code, s.error_message, s.attempted_at, s.observed_at, s.received_at, s.state_revision, s.value_revision, p.pid, p.started_at, p.cpu_percent, p.memory_bytes, p.uptime_ms FROM component_status s LEFT JOIN current_node_process_observations p ON p.node_id = s.node_id WHERE s.node_id = $1 AND s.component_key = 'process'"
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await?
    .map(|(state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision, pid, started_at, cpu_percent, memory_bytes, uptime_ms)| ProcessDiagnostic {
        state,
        error_code,
        error_message: redact_optional_message(error_message),
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
        pid,
        started_at,
        cpu_percent,
        memory_bytes,
        uptime_ms,
    }))
}

async fn data_directory_diagnostic(
    state: &AppState,
    node_id: &str,
) -> Result<Option<DataDirectoryDiagnostic>, sqlx::Error> {
    Ok(sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64, Option<i64>, Option<i64>)>(
        "SELECT s.state, s.error_code, s.error_message, s.attempted_at, s.observed_at, s.received_at, s.state_revision, s.value_revision, d.size_bytes, CASE WHEN c.state IN ('ok', 'error') THEN d.capacity_bytes ELSE NULL END FROM component_status s LEFT JOIN component_status c ON c.node_id = s.node_id AND c.component_key = 'datadirectorycapacitybytes' LEFT JOIN current_node_data_directory_observations d ON d.node_id = s.node_id WHERE s.node_id = $1 AND s.component_key = 'datadirectorysizebytes'"
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await?
    .map(|(state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision, size_bytes, capacity_bytes)| {
        let visible_size = matches!(state.as_str(), "ok" | "error").then_some(size_bytes).flatten();
        DataDirectoryDiagnostic {
            state,
            error_code,
            error_message: redact_optional_message(error_message),
            attempted_at,
            observed_at,
            received_at,
            state_revision,
            value_revision,
            size_bytes: visible_size,
            capacity_bytes: capacity_bytes.filter(|_| visible_size.is_some()),
        }
    }))
}

async fn sync_diagnostic(
    state: &AppState,
    node_id: &str,
) -> Result<Option<SyncDiagnostic>, sqlx::Error> {
    Ok(sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT s.state, s.error_code, s.error_message, s.attempted_at, s.observed_at, s.received_at, s.state_revision, s.value_revision, c.syncing, c.current_block, c.highest_block, c.pulled_states, c.known_states FROM component_status s LEFT JOIN current_node_chain_observations c ON c.node_id = s.node_id WHERE s.node_id = $1 AND s.component_key = 'sync'"
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await?
    .map(|(state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision, syncing, current_block, highest_block, pulled_states, known_states)| SyncDiagnostic {
        state,
        error_code,
        error_message: redact_optional_message(error_message),
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
        syncing: syncing.map(|value| value != 0),
        current_block,
        highest_block,
        pulled_states,
        known_states,
    }))
}

async fn consensus_diagnostic(
    state: &AppState,
    node_id: &str,
) -> Result<Option<ConsensusDiagnostic>, sqlx::Error> {
    Ok(sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT s.state, s.error_code, s.error_message, s.attempted_at, s.observed_at, s.received_at, s.state_revision, s.value_revision, c.consensus_epoch, c.consensus_view_number, c.consensus_validator, c.consensus_highest_qc_block, c.consensus_highest_lock_block, c.consensus_highest_commit_block FROM component_status s LEFT JOIN current_node_chain_observations c ON c.node_id = s.node_id WHERE s.node_id = $1 AND s.component_key = 'consensus'"
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await?
    .map(|(state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision, epoch, view_number, validator, highest_qc_block, highest_lock_block, highest_commit_block)| ConsensusDiagnostic {
        state,
        error_code,
        error_message: redact_optional_message(error_message),
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
        epoch,
        view_number,
        validator: validator.map(|value| value != 0),
        highest_qc_block,
        highest_lock_block,
        highest_commit_block,
    }))
}

type PeerPresenceRow = (
    String,
    String,
    Option<String>,
    String,
    i64,
    i64,
    i64,
    Option<String>,
);

fn peer_presence_interval_from_row(row: PeerPresenceRow) -> PeerPresenceInterval {
    let (
        peer_id,
        opened_at,
        closed_at,
        direction,
        trusted,
        static_peer,
        consensus_peer,
        client_name,
    ) = row;
    let duration_seconds = closed_at.as_deref().and_then(|closed| {
        let opened = crate::auth::parse_rfc3339(&opened_at)?;
        let closed = crate::auth::parse_rfc3339(closed)?;
        Some((closed - opened).whole_seconds().max(0))
    });
    PeerPresenceInterval {
        peer_id: crate::redaction::redact_sensitive(&peer_id),
        opened_at,
        closed_at,
        duration_seconds,
        direction,
        trusted: trusted != 0,
        static_peer: static_peer != 0,
        consensus_peer: consensus_peer != 0,
        client_name: client_name.map(|name| crate::redaction::redact_sensitive(&name)),
    }
}

pub(super) async fn peer_churn_diagnostic(
    state: &AppState,
    node_id: &str,
) -> Result<PeerChurnDiagnostic, sqlx::Error> {
    let window_start = crate::auth::format_rfc3339(
        crate::auth::now_utc() - time::Duration::hours(PEER_CHURN_WINDOW_HOURS),
    );
    let unknown = || PeerChurnDiagnostic {
        state: "unknown".to_owned(),
        freshness: "unknown".to_owned(),
        window_start: window_start.clone(),
        total_open_intervals: 0,
        recent_arrivals: Vec::new(),
        recent_departures: Vec::new(),
    };
    let Some((component_state, value_observation_at, value_revision)) =
        sqlx::query_as::<_, (String, Option<String>, i64)>(
            "SELECT state, COALESCE(value_observation_at, value_received_at), value_revision FROM component_status WHERE node_id=$1 AND component_key='peers'",
        )
        .bind(node_id)
        .fetch_optional(state.db().pool())
        .await?
    else {
        return Ok(unknown());
    };
    if value_revision <= 0 {
        return Ok(unknown());
    }
    let total_open_intervals = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND closed_at IS NULL",
    )
    .bind(node_id)
    .fetch_one(state.db().pool())
    .await?;
    let arrivals = sqlx::query_as::<_, PeerPresenceRow>(
        "SELECT peer_id, opened_at, closed_at, direction, trusted, static_peer, consensus_peer, client_name FROM peer_presence_intervals WHERE node_id=$1 AND opened_at >= $2 ORDER BY opened_at DESC, interval_id DESC LIMIT $3",
    )
    .bind(node_id)
    .bind(&window_start)
    .bind(PEER_CHURN_LIMIT)
    .fetch_all(state.db().pool())
    .await?
    .into_iter()
    .map(peer_presence_interval_from_row)
    .collect::<Vec<_>>();
    let departures = sqlx::query_as::<_, PeerPresenceRow>(
        "SELECT peer_id, opened_at, closed_at, direction, trusted, static_peer, consensus_peer, client_name FROM peer_presence_intervals WHERE node_id=$1 AND closed_at >= $2 ORDER BY closed_at DESC, interval_id DESC LIMIT $3",
    )
    .bind(node_id)
    .bind(&window_start)
    .bind(PEER_CHURN_LIMIT)
    .fetch_all(state.db().pool())
    .await?
    .into_iter()
    .map(peer_presence_interval_from_row)
    .collect::<Vec<_>>();
    let freshness = derive_freshness(value_observation_at.as_deref().into_iter()).to_owned();
    let has_intervals = total_open_intervals > 0 || !arrivals.is_empty() || !departures.is_empty();
    let diagnostic_state = match component_state.as_str() {
        "error" => "error",
        "unsupported" => "unsupported",
        "disabled" => "disabled",
        "starting" => "starting",
        "ok" if has_intervals => "ok",
        "ok" => "empty",
        _ => "unknown",
    };
    Ok(PeerChurnDiagnostic {
        state: diagnostic_state.to_owned(),
        freshness,
        window_start,
        total_open_intervals,
        recent_arrivals: arrivals,
        recent_departures: departures,
    })
}

async fn peer_diagnostic(
    state: &AppState,
    node_id: &str,
) -> Result<Option<PeerDiagnostic>, sqlx::Error> {
    let Some(component) = sqlx::query_as::<_, (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        i64,
    )>(
        "SELECT state, error_code, error_message, attempted_at, observed_at, received_at, COALESCE(value_observation_at, value_received_at), state_revision, value_revision FROM component_status WHERE node_id=$1 AND component_key='peers'",
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await?
    else {
        return Ok(None);
    };
    let (
        component_state,
        error_code,
        _agent_error_message,
        attempted_at,
        observed_at,
        received_at,
        value_observation_at,
        state_revision,
        value_revision,
    ) = component;
    // A projection read failure must not masquerade as an authoritative empty
    // snapshot, so it fails the whole diagnostic.
    let rows = sqlx::query_as::<_, (
        String,
        String,
        i64,
        i64,
        i64,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    )>(
        "SELECT peer_id, direction, trusted, static_peer, consensus_peer, client_name, cbft_protocol_version, cbft_highest_qc_block, cbft_locked_block, cbft_commit_block FROM current_node_peers WHERE node_id=$1 ORDER BY peer_id LIMIT 1024",
    )
    .bind(node_id)
    .fetch_all(state.db().pool())
    .await?;
    let has_value = value_revision > 0;
    let mut inbound_count = 0;
    let mut outbound_count = 0;
    let mut trusted_count = 0;
    let mut static_count = 0;
    let mut consensus_count = 0;
    let mut peers = Vec::with_capacity(rows.len());
    for (
        peer_id,
        direction,
        trusted,
        static_peer,
        consensus_peer,
        client_name,
        cbft_protocol_version,
        cbft_highest_qc_block,
        cbft_locked_block,
        cbft_commit_block,
    ) in rows
    {
        match direction.as_str() {
            "inbound" => inbound_count += 1,
            "outbound" => outbound_count += 1,
            _ => {}
        }
        if trusted != 0 {
            trusted_count += 1;
        }
        if static_peer != 0 {
            static_count += 1;
        }
        if consensus_peer != 0 {
            consensus_count += 1;
        }
        let capabilities = sqlx::query_scalar::<_, String>(
            "SELECT capability FROM current_node_peer_capabilities WHERE node_id=$1 AND peer_id=$2 ORDER BY capability",
        )
        .bind(node_id)
        .bind(&peer_id)
        .fetch_all(state.db().pool())
        .await?;
        peers.push(PeerDiagnosticEntry {
            peer_id: crate::redaction::redact_sensitive(&peer_id),
            direction,
            trusted: trusted != 0,
            static_peer: static_peer != 0,
            consensus_peer: consensus_peer != 0,
            client_name: client_name.map(|name| crate::redaction::redact_sensitive(&name)),
            capabilities: capabilities
                .into_iter()
                .map(|capability| crate::redaction::redact_sensitive(&capability))
                .collect(),
            cbft_protocol_version,
            cbft_highest_qc_block,
            cbft_locked_block,
            cbft_commit_block,
        });
    }

    let freshness = if has_value {
        derive_freshness(value_observation_at.as_deref().into_iter()).to_owned()
    } else {
        "unknown".to_owned()
    };
    let peer_error = component_state == "error";
    Ok(Some(PeerDiagnostic {
        state: component_state,
        error_code,
        // Peer probe messages are Agent-controlled and may contain raw RPC
        // payloads. Keep the Admin response privacy-safe; the typed code and
        // state still identify the failure.
        error_message: peer_error.then(|| "Peer Snapshot collection failed".to_owned()),
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
        freshness,
        peer_count: has_value.then_some(peers.len() as i64),
        inbound_count: has_value.then_some(inbound_count),
        outbound_count: has_value.then_some(outbound_count),
        trusted_count: has_value.then_some(trusted_count),
        static_count: has_value.then_some(static_count),
        consensus_count: has_value.then_some(consensus_count),
        peers,
    }))
}

/// One Node as the Admin routes present it: its diagnostic view with the
/// Node Currency, Health and identity disposition it was judged with.
pub(super) struct AssessedNode {
    pub(super) diagnostic: NodeDiagnostic,
    pub(super) identity: NodeIdentityStatus,
    pub(super) identity_disposition: node_currency::NodeIdentity,
    pub(super) health_reason: node_currency::HealthReason,
    pub(super) currency: node_currency::NodeCurrency,
    pub(super) assessed_at: time::OffsetDateTime,
}

/// Collect the full Server-owned diagnostic view for one Node. Every Admin
/// route that shows a Node's Health reads it here, so the Health, its reason
/// and the Node Currency come from [`node_currency`] once, exactly as the
/// Public projection reads them. A failed read fails the request instead of
/// presenting the Node as never observed.
pub(super) async fn node_diagnostic(
    state: &AppState,
    node_id: String,
    network_key: String,
    display_name: Option<String>,
    lifecycle: String,
    inventory_revision: i64,
    visibility: String,
) -> Result<AssessedNode, sqlx::Error> {
    let process = process_diagnostic(state, &node_id).await?;
    let data_directory = data_directory_diagnostic(state, &node_id).await?;
    let rpc = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>, Option<i64>)>(
        "SELECT c.rpc_client_version, s.state, s.error_code, s.error_message, s.attempted_at, s.observed_at, s.received_at, s.state_revision, s.value_revision FROM component_status s LEFT JOIN current_node_chain_observations c ON c.node_id = s.node_id WHERE s.node_id = $1 AND s.component_key = 'rpc'",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await?;
    let rpc = if let Some((
        client_version,
        component_state,
        error_code,
        error_message,
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
    )) = rpc
    {
        let namespaces = sqlx::query_scalar::<_, String>("SELECT namespace FROM current_node_rpc_namespaces WHERE node_id = $1 ORDER BY namespace")
            .bind(&node_id).fetch_all(state.db().pool()).await?;
        let methods = sqlx::query_scalar::<_, String>(
            "SELECT method FROM current_node_rpc_methods WHERE node_id = $1 ORDER BY method",
        )
        .bind(&node_id)
        .fetch_all(state.db().pool())
        .await?;
        Some(RpcDiagnostic {
            client_version,
            namespaces,
            methods,
            state: component_state,
            error_code,
            error_message: redact_optional_message(error_message),
            attempted_at,
            observed_at,
            received_at,
            state_revision,
            value_revision,
        })
    } else {
        None
    };
    let sync = sync_diagnostic(state, &node_id).await?;
    let consensus = consensus_diagnostic(state, &node_id).await?;
    let peers = peer_diagnostic(state, &node_id).await?;
    let identity = node_identity_status(state, &node_id, &network_key).await?;
    let identity_component_state = sqlx::query_scalar::<_, String>(
        "SELECT state FROM component_status WHERE node_id = $1 AND component_key = 'network_identity'",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await?;
    let identity_disposition = node_currency::NodeIdentity::from_observation(
        identity_component_state.as_deref(),
        identity.state == "mismatched",
    );
    let mut conn = state.db().pool().acquire().await?;
    let evidence = node_currency::load(&mut conn, &[node_id.as_str()])
        .await?
        .remove(&node_id)
        .unwrap_or_default();
    drop(conn);
    let assessed_at = crate::auth::now_utc();
    let currency = evidence.currency(assessed_at);
    let summary = node_currency::health_summary(
        &lifecycle,
        process.as_ref().map(|process| process.state.as_str()),
        identity_disposition,
        &evidence,
        assessed_at,
    );
    let history = sqlx::query_as::<_, (Option<i64>, Option<i64>, Option<String>, Option<i64>, Option<String>)>(
        "SELECT c.current_block, h.historical_high_watermark, h.resync_state, r.block_number, r.confidence FROM current_node_chain_observations c LEFT JOIN block_history_state h ON h.node_id=c.node_id LEFT JOIN nodes n ON n.node_id=c.node_id LEFT JOIN network_reference_heads r ON r.network_key=n.network_key WHERE c.node_id=$1"
    ).bind(&node_id).fetch_optional(state.db().pool()).await?;
    let (
        current_head,
        historical_high_watermark,
        resync_state,
        network_reference_head,
        network_reference_confidence,
    ) = history.unwrap_or((None, None, None, None, None));
    let resync_state = resync_state.unwrap_or_else(|| "normal".to_owned());
    let diagnostic = NodeDiagnostic {
        node_id,
        network_key,
        display_name,
        lifecycle,
        inventory_revision,
        visibility,
        health: summary.health.as_str().to_owned(),
        health_reason: summary.reason.text().to_owned(),
        currency: currency.state(),
        currency_as_of: evidence.as_of(),
        process,
        data_directory,
        rpc,
        sync,
        consensus,
        peers,
        current_head,
        historical_high_watermark,
        resync_progress: historical_high_watermark
            .zip(current_head)
            .map(|(high, current)| format!("{current}/{high}")),
        network_reference_head,
        network_reference_confidence: network_reference_confidence
            .unwrap_or_else(|| "unknown".to_owned()),
        resync_state,
    };
    Ok(AssessedNode {
        diagnostic,
        identity,
        identity_disposition,
        health_reason: summary.reason,
        currency,
        assessed_at,
    })
}

/// Agent-observed Network identity tuple (design §4.5). Never overwrites the
/// Registry: it is presented as a typed observation with a match state.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ObservedNetworkIdentity {
    pub genesis_hash: Option<String>,
    pub chain_id: Option<i64>,
    pub p2p_network_id: Option<i64>,
    pub address_hrp: Option<String>,
}

/// Server-computed identity disposition for one Node: `matched` when every
/// observed identity field equals the Registry tuple, `mismatched` when any
/// observed field differs (a blocking diagnostic distinct from RPC Error or
/// Node Offline), and `unknown` when the Node was never observed.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NodeIdentityStatus {
    pub state: String,
    pub observed: Option<ObservedNetworkIdentity>,
    /// Registry fields that the observation contradicts (empty when state
    /// is `matched` or `unknown`).
    pub mismatched_fields: Vec<String>,
}

/// Compare the observed identity of one Node against its Registry tuple.
/// The Registry is the only authority: an observation never rewrites it,
/// and a Node whose identity contradicts the Registry is blocked from
/// merging history (design §4.5).
pub(super) async fn node_identity_status(
    state: &AppState,
    node_id: &str,
    network_key: &str,
) -> Result<NodeIdentityStatus, sqlx::Error> {
    let observed = sqlx::query_as::<_, (Option<String>, Option<i64>, Option<i64>, Option<String>, Option<String>, Option<i64>)>(
        "SELECT c.network_genesis_hash, c.network_chain_id, c.network_p2p_network_id, c.network_address_hrp, s.state, s.value_revision FROM current_node_chain_observations c LEFT JOIN component_status s ON s.node_id = c.node_id AND s.component_key = 'network_identity' WHERE c.node_id = $1",
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await?;
    let Some((
        genesis_hash,
        chain_id,
        p2p_network_id,
        address_hrp,
        identity_state,
        identity_value_revision,
    )) = observed
    else {
        return Ok(NodeIdentityStatus {
            state: "unknown".to_owned(),
            observed: None,
            mismatched_fields: Vec::new(),
        });
    };
    let observed_identity = ObservedNetworkIdentity {
        genesis_hash: genesis_hash.clone(),
        chain_id,
        p2p_network_id,
        address_hrp: address_hrp.clone(),
    };
    // A mismatch is actionable only when the dedicated identity component
    // confirms a complete successful observation. Retained values behind an
    // error, disabled component, or missing receipt remain Unknown.
    if identity_state.as_deref() != Some("ok") || identity_value_revision.unwrap_or_default() == 0 {
        return Ok(NodeIdentityStatus {
            state: "unknown".to_owned(),
            observed: Some(observed_identity),
            mismatched_fields: Vec::new(),
        });
    }
    let observed_fields = [
        genesis_hash.as_deref().is_some(),
        chain_id.is_some(),
        p2p_network_id.is_some(),
        address_hrp.as_deref().is_some(),
    ];
    if !observed_fields.iter().all(|observed| *observed) {
        return Ok(NodeIdentityStatus {
            state: "unknown".to_owned(),
            observed: Some(observed_identity),
            mismatched_fields: Vec::new(),
        });
    }
    let expected = sqlx::query_as::<_, (String, i64, i64, String)>(
        "SELECT genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks WHERE network_key = $1",
    )
    .bind(network_key)
    .fetch_optional(state.db().pool())
    .await?;
    let Some((expected_genesis, expected_chain, expected_p2p, expected_hrp)) = expected else {
        return Ok(NodeIdentityStatus {
            state: "unknown".to_owned(),
            observed: Some(observed_identity),
            mismatched_fields: Vec::new(),
        });
    };
    let mut mismatched_fields = Vec::new();
    if genesis_hash
        .as_deref()
        .is_some_and(|value| value != expected_genesis)
    {
        mismatched_fields.push("genesis_hash".to_owned());
    }
    if chain_id.is_some_and(|value| value != expected_chain) {
        mismatched_fields.push("chain_id".to_owned());
    }
    if p2p_network_id.is_some_and(|value| value != expected_p2p) {
        mismatched_fields.push("p2p_network_id".to_owned());
    }
    if address_hrp
        .as_deref()
        .is_some_and(|value| value != expected_hrp)
    {
        mismatched_fields.push("address_hrp".to_owned());
    }
    // `matched` requires every identity field to have been observed and to
    // equal the Registry tuple; a partial observation stays `unknown` and
    // never fabricates Registry values.
    let state = if !mismatched_fields.is_empty() {
        "mismatched"
    } else if observed_fields.contains(&false) {
        "unknown"
    } else {
        "matched"
    };
    Ok(NodeIdentityStatus {
        state: state.to_owned(),
        observed: Some(ObservedNetworkIdentity {
            genesis_hash,
            chain_id,
            p2p_network_id,
            address_hrp,
        }),
        mismatched_fields,
    })
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct AgentAdminRow {
    agent_id: String,
    display_name: Option<String>,
    notes: Option<String>,
    agent_epoch: i64,
    active_boot_id: Option<String>,
    active_boot_status: String,
    previous_boot_id: Option<String>,
    close_report_id: Option<String>,
    shutdown_state: String,
    shutdown_started_at: Option<String>,
    shutdown_deadline_at: Option<String>,
    shutdown_finished_at: Option<String>,
    shutdown_unresolved_from: Option<i64>,
    shutdown_unresolved_to: Option<i64>,
    shutdown_last_error: Option<String>,
    shutdown_forced: i64,
    shutdown_report_id: Option<String>,
    shutdown_report_sequence: Option<i64>,
    shutdown_updated_at: Option<String>,
    last_report_sequence: Option<i64>,
    agent_capabilities_json: String,
    clock_skew_ms: Option<i64>,
    clock_status: Option<String>,
    last_received_at: Option<String>,
    security_event_count: i64,
    accepted_inventory_revision: i64,
    accepted_inventory_sha256: Option<String>,
    accepted_inventory_protocol_major: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/agents",
    tag = "admin",
    responses((status = 200, description = "Owner-only Agent and Node diagnostics", body = [AgentDiagnostic]), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(super) async fn diagnostics(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    let agents = sqlx::query_as::<_, AgentAdminRow>(

        "SELECT agent_id, display_name, notes, agent_epoch, active_boot_id, active_boot_status, previous_boot_id, close_report_id, shutdown_state, shutdown_started_at, shutdown_deadline_at, shutdown_finished_at, shutdown_unresolved_from, shutdown_unresolved_to, shutdown_last_error, shutdown_forced, shutdown_report_id, shutdown_report_sequence, shutdown_updated_at, last_report_sequence, agent_capabilities_json, clock_skew_ms, clock_status, last_received_at, security_event_count, last_inventory_revision AS accepted_inventory_revision, inventory_sha256 AS accepted_inventory_sha256, inventory_protocol_major AS accepted_inventory_protocol_major FROM agents WHERE deleted_at IS NULL ORDER BY agent_id",
    )
    .fetch_all(state.db().pool())
    .await;
    let agents = match agents {
        Ok(agents) => agents,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let mut result = Vec::with_capacity(agents.len());
    for row in agents {
        match agent_diagnostic(&state, row).await {
            Ok(diagnostic) => result.push(diagnostic),
            Err(error) => return storage_failure(&state, &request_id.0, &error),
        }
    }
    Json(result).into_response()
}

/// Build one Server-owned Agent diagnostic with identity, liveness,
/// boot/report state, credential state, Inventory, and collector
/// diagnostics kept as separate dimensions (design §5.1, §9.2).
pub(super) async fn agent_diagnostic(
    state: &AppState,
    row: AgentAdminRow,
) -> Result<AgentDiagnostic, sqlx::Error> {
    let AgentAdminRow {
        agent_id,
        display_name,
        notes,
        agent_epoch,
        active_boot_id,
        active_boot_status: boot_status,
        previous_boot_id,
        close_report_id,
        shutdown_state,
        shutdown_started_at,
        shutdown_deadline_at,
        shutdown_finished_at,
        shutdown_unresolved_from,
        shutdown_unresolved_to,
        shutdown_last_error,
        shutdown_forced,
        shutdown_report_id,
        shutdown_report_sequence,
        shutdown_updated_at,
        last_report_sequence,
        agent_capabilities_json: capabilities_json,
        clock_skew_ms,
        clock_status,
        last_received_at,
        security_event_count,
        accepted_inventory_revision,
        accepted_inventory_sha256,
        accepted_inventory_protocol_major,
    } = row;
    let capabilities = serde_json::from_str::<Vec<String>>(&capabilities_json)
        .unwrap_or_default()
        .into_iter()
        .map(|capability| crate::redaction::redact_sensitive(&capability))
        .collect();
    let liveness = agent_liveness(last_received_at.as_deref()).to_owned();
    let (sequence_gap_count, latest_gap_at) = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT COUNT(*), MAX(created_at) FROM report_sequence_gaps WHERE agent_id=$1",
    )
    .bind(&agent_id)
    .fetch_one(state.db().pool())
    .await?;
    let credentials = credential_summaries(state, &agent_id).await?;
    let host_components = sqlx::query_as::<_, (String, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64)>(
        "SELECT component_key, state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision FROM component_status WHERE agent_id = $1 AND scope = 'host' ORDER BY component_key",
    )
    .bind(&agent_id)
    .fetch_all(state.db().pool())
    .await?
    .into_iter()
    .map(|(component, state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision)| HostComponentDiagnostic {
        component,
        state,
        error_code,
        error_message: redact_optional_message(error_message),
        attempted_at,
        observed_at,
        received_at,
        state_revision,
        value_revision,
    })
    .collect::<Vec<_>>();
    let host = sqlx::query_as::<_, HostProjectionRow>(
        "SELECT h.cpu_percent, h.memory_total_bytes, h.memory_used_bytes, h.load1, h.load5, h.load15, h.network_rx_bytes_per_sec, h.network_tx_bytes_per_sec, h.spool_queued_bytes, h.spool_queued_reports, h.spool_oldest_queued_age_ms, h.spool_in_flight, h.spool_last_delivery_error, h.spool_last_delivery_at, h.spool_capacity_bytes AS spool_capacity_bytes, h.spool_max_age_seconds AS spool_max_age_seconds, h.spool_dropped_sequence_from AS spool_dropped_sequence_from, h.spool_dropped_sequence_to AS spool_dropped_sequence_to, h.spool_dropped_time_from AS spool_dropped_time_from, h.spool_dropped_time_to AS spool_dropped_time_to, h.spool_dropped_height_from AS spool_dropped_height_from, h.spool_dropped_height_to AS spool_dropped_height_to, h.spool_pending_history_gaps AS spool_pending_history_gaps, h.spool_report_too_large AS spool_report_too_large, h.spool_store_fatal AS spool_store_fatal, h.spool_store_error AS spool_store_error, h.spool_backlog_age_ms AS spool_backlog_age_ms, h.spool_backlog_age_basis AS spool_backlog_age_basis, h.updated_at FROM current_host_observations h WHERE h.agent_id = $1",
    )
    .bind(&agent_id)
    .fetch_optional(state.db().pool())
    .await?
    .map(|row| HostDiagnostic {
        cpu_percent: row.cpu_percent, memory_total_bytes: row.memory_total_bytes, memory_used_bytes: row.memory_used_bytes, load1: row.load1, load5: row.load5, load15: row.load15, network_rx_bytes_per_sec: row.network_rx_bytes_per_sec, network_tx_bytes_per_sec: row.network_tx_bytes_per_sec,
        spool_queued_bytes: row.spool_queued_bytes, spool_queued_reports: row.spool_queued_reports, spool_oldest_queued_age_ms: row.spool_oldest_queued_age_ms, spool_in_flight: row.spool_in_flight.map(|v| v != 0), spool_last_delivery_error: redact_optional_message(row.spool_last_delivery_error), spool_last_delivery_at: row.spool_last_delivery_at,
        spool_capacity_bytes: row.spool_capacity_bytes, spool_max_age_seconds: row.spool_max_age_seconds, spool_dropped_sequence_from: row.spool_dropped_sequence_from, spool_dropped_sequence_to: row.spool_dropped_sequence_to, spool_dropped_time_from: row.spool_dropped_time_from, spool_dropped_time_to: row.spool_dropped_time_to, spool_dropped_height_from: row.spool_dropped_height_from, spool_dropped_height_to: row.spool_dropped_height_to, spool_pending_history_gaps: row.spool_pending_history_gaps, spool_report_too_large: row.spool_report_too_large.map(|v| v != 0), spool_store_fatal: row.spool_store_fatal.map(|v| v != 0), spool_store_error: redact_optional_message(row.spool_store_error),
        spool_backlog_age: SpoolBacklogAgeDiagnostic::from_columns(row.spool_backlog_age_ms, row.spool_backlog_age_basis),
        updated_at: row.updated_at, components: host_components,
    });
    let rows = sqlx::query_as::<_, (String, String, Option<String>, String, i64, String)>(
        "SELECT node_id, network_key, display_name, lifecycle, inventory_revision, visibility FROM nodes WHERE agent_id = $1 ORDER BY node_id",
    )
    .bind(&agent_id)
    .fetch_all(state.db().pool())
    .await?;
    let mut nodes = Vec::with_capacity(rows.len());
    for (node_id, network_key, display_name, lifecycle, inventory_revision, visibility) in rows {
        nodes.push(
            node_diagnostic(
                state,
                node_id,
                network_key,
                display_name,
                lifecycle,
                inventory_revision,
                visibility,
            )
            .await?
            .diagnostic,
        );
    }
    // Server-owned Inventory diagnosis (issue #181): the fact the Server
    // accepts and the refusal that stopped the newest report. One extra
    // per-Agent query, the same shape as the gap and credential reads above.
    let inventory = agent_inventory_diagnostic(
        state,
        &agent_id,
        accepted_inventory_revision,
        accepted_inventory_sha256.clone(),
        accepted_inventory_protocol_major,
    )
    .await?;
    // Server-owned Attention Items for this Agent, with the occurrence
    // boundary and any acknowledged occurrence already applied (issue #172).
    let attention = {
        let evidence = crate::attention::AgentAttentionRow {
            agent_id: agent_id.clone(),
            last_received_at: last_received_at.clone(),
            shutdown_state: shutdown_state.clone(),
            shutdown_updated_at: shutdown_updated_at.clone(),
            security_event_count,
            sequence_gap_count,
            latest_gap_at,
            spool_store_fatal: host
                .as_ref()
                .and_then(|host| host.spool_store_fatal)
                .map(|value| if value { 1 } else { 0 }),
            spool_dropped_sequence_to: host
                .as_ref()
                .and_then(|host| host.spool_dropped_sequence_to),
            host_updated_at: host.as_ref().map(|host| host.updated_at.clone()),
            // The newest-receipt evidence is read once and feeds both the
            // Admin DTO and the attention boundary, so the page and the item
            // can never disagree about which attempt was refused.
            latest_receipt_disposition: inventory
                .diagnostic
                .last_rejection
                .as_ref()
                .map(|_| "rejected".to_owned()),
            latest_rejection_code: inventory
                .diagnostic
                .last_rejection
                .as_ref()
                .map(|evidence| evidence.code.clone()),
            latest_rejection_inventory_revision: inventory
                .diagnostic
                .last_rejection
                .as_ref()
                .and_then(|evidence| evidence.reported_revision),
            latest_rejection_inventory_sha256: inventory
                .diagnostic
                .last_rejection
                .as_ref()
                .and_then(|evidence| evidence.reported_sha256.clone()),
            latest_rejection_inventory_protocol_major: inventory.rejected_protocol_major,
            latest_receipt_at: inventory
                .diagnostic
                .last_rejection
                .as_ref()
                .and_then(|evidence| evidence.received_at.clone()),
            accepted_inventory_revision,
            accepted_inventory_sha256,
        };
        crate::attention::evaluate_agent(state.db().pool(), &evidence).await?
    };
    Ok(AgentDiagnostic {
        agent_id,
        display_name,
        notes,
        agent_epoch,
        last_report_sequence,
        clock_status: clock_status.unwrap_or_else(|| "unknown".to_owned()),
        clock_skew_ms,
        liveness,
        last_received_at,
        capabilities,
        active_boot_id,
        boot_status,
        previous_boot_id,
        close_report_id,
        shutdown_state,
        shutdown_started_at,
        shutdown_deadline_at,
        shutdown_finished_at,
        shutdown_unresolved_range: shutdown_unresolved_from.zip(shutdown_unresolved_to),
        shutdown_last_error: redact_optional_message(shutdown_last_error),
        shutdown_forced: shutdown_forced != 0,
        shutdown_report_id,
        shutdown_report_sequence,
        shutdown_updated_at,
        sequence_gap_count,
        security_event_count,
        credentials,
        host,
        nodes,
        inventory: inventory.diagnostic,
        attention,
    })
}

/// The Agent's Inventory diagnosis (issue #181): the revision/hash the Server
/// accepts, plus the newest stored Report Receipt when that receipt refused
/// the report because of its Inventory.
///
/// A read failure degrades to "no rejection" instead of failing the whole
/// diagnostics page; the accepted fact still comes from the `agents` row, and
/// a hidden refusal is loud elsewhere (the Agent console and the frozen
/// projections). One extra per-Agent query, matching the existing N+1 shape of
/// this builder.
async fn agent_inventory_diagnostic(
    state: &AppState,
    agent_id: &str,
    accepted_revision: i64,
    accepted_sha256: Option<String>,
    accepted_protocol_major: Option<i64>,
) -> Result<InventoryDiagnosis, sqlx::Error> {
    let latest = sqlx::query_as::<_, (String, Option<String>, Option<i64>, Option<String>, String, Option<i64>)>(
        "SELECT disposition, rejection_code, inventory_revision, inventory_sha256, received_at, inventory_protocol_major FROM agent_report_receipts WHERE agent_id = $1 ORDER BY received_at DESC, report_sequence DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(state.db().pool())
    .await?;
    let rejection = latest.filter(|row| {
        row.0.as_str() == "rejected"
            && row
                .1
                .as_deref()
                .is_some_and(crate::attention::is_inventory_rejection_code)
    });
    // The refusal keeps the protocol Server-recorded with the receipt, so a v2
    // declaration is explained as Server-managed and never given a fabricated
    // Agent revision (issue #188).
    let rejected_protocol_major = rejection
        .as_ref()
        .and_then(|(_, _, _, _, _, protocol_major)| *protocol_major);
    let last_rejection = rejection.map(
        |(_, code, reported_revision, reported_sha256, received_at, protocol_major)| {
            AgentInventoryRejectionEvidence {
                code: code.expect("filtered on an Inventory rejection code"),
                declaration: crate::attention::declaration_kind(protocol_major).to_owned(),
                reported_revision,
                reported_sha256,
                received_at: Some(received_at),
            }
        },
    );
    Ok(InventoryDiagnosis {
        diagnostic: AgentInventoryDiagnostic {
            // No accepted hash means the Server never accepted a declaration:
            // uninitialized, not an accepted Inventory at revision 0.
            accepted_revision: accepted_sha256.as_ref().map(|_| accepted_revision),
            accepted_sha256,
            accepted_declaration: crate::attention::declaration_kind(accepted_protocol_major)
                .to_owned(),
            last_rejection,
        },
        rejected_protocol_major,
    })
}

/// Redacted credential state for one Agent, newest first. `active` is
/// Server-computed so the WebUI never derives security policy locally.
async fn credential_summaries(
    state: &AppState,
    agent_id: &str,
) -> Result<Vec<AgentCredentialSummary>, sqlx::Error> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let rows = sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
        "SELECT credential_id, created_at, revoked_at, revoke_after FROM agent_credentials WHERE agent_id = $1 ORDER BY created_at DESC, credential_id",
    )
    .bind(agent_id)
    .fetch_all(state.db().pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(credential_id, created_at, revoked_at, revoke_after)| {
            let active = revoked_at.is_none()
                && revoke_after
                    .as_deref()
                    .is_none_or(|after| after > now.as_str());
            AgentCredentialSummary {
                credential_id,
                created_at,
                revoked_at,
                revoke_after,
                active,
            }
        })
        .collect())
}

/// The Spool Backlog Age the Server derived for one Agent when it received the
/// Report that carries its current Spool observation (ADR 0014, issue #272).
/// The reading is per Agent because Host observations are, so a Node view reads
/// it through the Agent that owns the Node.
pub(super) async fn spool_backlog_age_diagnostic(
    state: &AppState,
    agent_id: &str,
) -> Result<Option<SpoolBacklogAgeDiagnostic>, sqlx::Error> {
    sqlx::query_as::<_, (Option<i64>, Option<String>)>(
        "SELECT spool_backlog_age_ms, spool_backlog_age_basis FROM current_host_observations WHERE agent_id = $1",
    )
    .bind(agent_id)
    .fetch_optional(state.db().pool())
    .await
    .map(|row| row.and_then(|(age_ms, basis)| SpoolBacklogAgeDiagnostic::from_columns(age_ms, basis)))
}

/// Liveness rule shared by the diagnostics list, the overview, and the
/// Server-owned Attention boundary (defined once in `crate::attention`).
pub(super) fn agent_liveness(last_received_at: Option<&str>) -> &'static str {
    crate::attention::agent_liveness(last_received_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::AuthenticatedSession;
    use axum::body::to_bytes;
    use serde_json::Value;
    use tempfile::tempdir;
    use time::OffsetDateTime;

    /// A replayed Report must not make the Admin Node view look current: health
    /// and freshness are judged on the Server-clock observation instants, exactly
    /// as the Public projection judges them (ADR 0014).
    #[tokio::test]
    async fn node_diagnostic_judges_freshness_on_the_observation_instant() {
        let dir = tempdir().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth);
        let now = crate::auth::now_utc();
        let receipt = crate::auth::format_rfc3339(now);
        let replayed = crate::auth::format_rfc3339(now - time::Duration::hours(2));
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ($1, 1, $2, $3)")
            .bind("agent-admin-274")
            .bind(&receipt)
            .bind(&receipt)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('platon-mainnet', 'PlatON Mainnet', '0xgenesis', 210425, 210425, 'lat', $1, $1)")
            .bind(&receipt)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('node-admin-274', 'agent-admin-274', 'platon-mainnet', 'ws://127.0.0.1:1', 'active', 'public', 1, $1, $1)")
            .bind(&receipt)
            .execute(state.db().pool())
            .await
            .unwrap();
        for component_key in ["rpc", "sync", "consensus"] {
            sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, observation_at, state_revision, value_revision) VALUES ($1, 'node', $2, $2, $3, 'ok', $4, $4, $5, $4, 1, 1)")
                .bind("agent-admin-274")
                .bind("node-admin-274")
                .bind(component_key)
                .bind(&replayed)
                .bind(&receipt)
                .execute(state.db().pool())
                .await
                .unwrap();
        }

        // Delivered now, but observed two hours ago: the Admin view is stale and
        // Unknown while the receipt instants are current.
        let diagnostic = node_diagnostic(
            &state,
            "node-admin-274".to_owned(),
            "platon-mainnet".to_owned(),
            None,
            "active".to_owned(),
            1,
            "visible".to_owned(),
        )
        .await
        .unwrap()
        .diagnostic;
        assert_eq!(diagnostic.currency, node_currency::NodeCurrencyState::Stale);
        assert_eq!(diagnostic.health, "unknown");
        assert_eq!(diagnostic.health_reason, "observations are stale");

        // The same rows with a fresh observation instant are current and Healthy.
        sqlx::query("UPDATE component_status SET observation_at = $1 WHERE node_id = $2")
            .bind(&receipt)
            .bind("node-admin-274")
            .execute(state.db().pool())
            .await
            .unwrap();
        let diagnostic = node_diagnostic(
            &state,
            "node-admin-274".to_owned(),
            "platon-mainnet".to_owned(),
            None,
            "active".to_owned(),
            1,
            "visible".to_owned(),
        )
        .await
        .unwrap()
        .diagnostic;
        assert_eq!(
            diagnostic.currency,
            node_currency::NodeCurrencyState::Current
        );
        assert_eq!(diagnostic.health, "healthy");
        assert_eq!(
            diagnostic.health_reason,
            "RPC, sync, and consensus are current"
        );
    }

    #[tokio::test]
    async fn host_diagnostics_include_failure_and_freshness_fields() {
        let dir = tempdir().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth);
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ($1, 1, $2, $3)")
            .bind("agent-host-test")
            .bind("2026-01-01T00:00:00Z")
            .bind("2026-01-01T00:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_host_observations (agent_id, updated_at) VALUES ($1, $2)")
            .bind("agent-host-test")
            .bind("2026-01-01T00:00:02Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision, error_code, error_message) VALUES ($1, 'host', 'host', 'cpu_percent', 'error', $2, $3, $4, 2, 1, $5, $6)")
            .bind("agent-host-test")
            .bind("2026-01-01T00:00:01Z")
            .bind("2025-12-31T23:59:00Z")
            .bind("2026-01-01T00:00:02Z")
            .bind("rpc_unreachable")
            .bind("RPC probe failed")
            .execute(state.db().pool())
            .await
            .unwrap();
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        let response = diagnostics(
            State(state),
            Extension(AuthenticatedSession(session)),
            Extension(crate::http::RequestId(std::sync::Arc::from("request"))),
        )
        .await;
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let component = &value[0]["host"]["components"][0];
        assert_eq!(component["state"], "error");
        assert_eq!(component["error_code"], "rpc_unreachable");
        assert_eq!(component["attempted_at"], "2026-01-01T00:00:01Z");
        assert_eq!(component["observed_at"], "2025-12-31T23:59:00Z");
        assert_eq!(component["received_at"], "2026-01-01T00:00:02Z");
        assert_eq!(component["state_revision"], 2);
        assert_eq!(component["value_revision"], 1);
    }

    #[test]
    fn derive_freshness_ranks_retained_observations() {
        let now = crate::auth::now_utc();
        let recent = crate::auth::format_rfc3339(now - time::Duration::seconds(30));
        let stale = crate::auth::format_rfc3339(now - time::Duration::hours(3));
        assert_eq!(
            derive_freshness([recent.as_str(), stale.as_str()].into_iter()),
            "current"
        );
        assert_eq!(derive_freshness([stale.as_str()].into_iter()), "stale");
        assert_eq!(derive_freshness([].into_iter()), "unknown");
        // Future-skewed clocks are never shown as current.
        let future = crate::auth::format_rfc3339(now + time::Duration::hours(1));
        assert_eq!(derive_freshness([future.as_str()].into_iter()), "stale");
    }
}
