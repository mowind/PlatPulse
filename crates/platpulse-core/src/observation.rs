//! Host, Node Process, and Node Chain current observations.
//!
//! Every observation is scoped per Node (never merged into an Agent-level
//! chain view), and every independently collected value travels inside the
//! Observation Envelope so a failure never erases the last successful value.
//! Host observations are collected once per Agent and referenced by Node
//! views; they are never duplicated per Node.

use serde::{Deserialize, Serialize};

use crate::component::ComponentObservation;
use crate::hex::FingerprintHex;
use crate::identity::{NodeId, ReportId};
use crate::network::NetworkIdentity;
use crate::time::Rfc3339;

/// Host-level operational state, collected once per Agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostObservation {
    /// Overall CPU utilization in percent (0..=100).
    pub cpu_percent: ComponentObservation<f64>,
    /// Host memory usage.
    pub memory: ComponentObservation<MemoryCurrent>,
    /// 1/5/15-minute load averages.
    pub load: ComponentObservation<LoadCurrent>,
    /// Per-mount disk usage.
    pub disk: ComponentObservation<DiskCurrent>,
    /// Host network throughput (bytes/s).
    pub network_throughput: ComponentObservation<NetworkThroughput>,
    /// Monotonic elapsed time spent collecting this Host snapshot, in
    /// milliseconds. It is a duration and is never compared with wall-clock
    /// timestamps.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub monotonic_elapsed_ms: Option<u64>,
    /// Estimated Agent↔Server wall-clock skew, signed milliseconds
    /// (negative = Agent clock behind).
    pub clock_skew: ComponentObservation<i64>,
    /// AgentStore/spool state.
    pub spool: ComponentObservation<SpoolDiagnostics>,
}

/// Host memory snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryCurrent {
    /// Total physical memory in bytes.
    pub total_bytes: u64,
    /// Used physical memory in bytes (`used <= total`).
    pub used_bytes: u64,
}

/// Host load averages.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadCurrent {
    /// 1-minute load average (>= 0).
    pub load1: f64,
    /// 5-minute load average (>= 0).
    pub load5: f64,
    /// 15-minute load average (>= 0).
    pub load15: f64,
}

/// Per-mount disk usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskCurrent {
    /// One entry per monitored mount. Contract limit: 128 entries.
    pub mounts: Vec<MountUsage>,
}

/// Disk usage of one mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountUsage {
    /// Mount path — sensitive, never part of the Public Projection
    /// (contract limit: 4096 chars).
    pub mount_path: String,
    /// Total capacity in bytes.
    pub total_bytes: u64,
    /// Used bytes (`used <= total`).
    pub used_bytes: u64,
}

/// Host network throughput.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkThroughput {
    /// Receive rate in bytes per second.
    pub rx_bytes_per_sec: u64,
    /// Transmit rate in bytes per second.
    pub tx_bytes_per_sec: u64,
}

/// AgentStore/spool diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpoolDiagnostics {
    /// Bytes currently queued for delivery.
    pub queued_bytes: u64,
    /// Reports currently queued for delivery (in-flight report included).
    pub queued_reports: u64,
    /// Age of the oldest queued report in milliseconds.
    pub oldest_queued_age_ms: u64,
    /// Cumulative reports dropped by capacity cleanup.
    pub dropped_reports: u64,
    /// Cumulative block samples dropped by capacity cleanup.
    pub dropped_samples: u64,
    /// Whether one report is currently claimed by the delivery sender.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub in_flight: Option<bool>,
    /// Last bounded delivery failure, if one was recorded locally.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub last_delivery_error: Option<String>,
    /// Server/transport time at which the last delivery failure was recorded.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub last_delivery_at: Option<Rfc3339>,
    /// Configured byte capacity of the durable spool.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub capacity_bytes: Option<u64>,
    /// Configured maximum report age in seconds.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub max_age_seconds: Option<u64>,
    /// Cumulative dropped sequence range, if capacity cleanup has occurred.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub dropped_sequence_range: Option<(u64, u64)>,
    /// Cumulative dropped generated-time range, RFC3339 strings are retained
    /// as typed values at the projection boundary.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub dropped_time_range: Option<(Rfc3339, Rfc3339)>,
    /// Cumulative dropped block-height range.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub dropped_height_range: Option<(u64, u64)>,
    /// Number of pending spool-overflow history gaps.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub pending_history_gaps: Option<u64>,
    /// The minimum complete current report exceeded the protocol hard limit.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub report_too_large: Option<bool>,
    /// Store integrity or quick-check failure is fatal until operator action.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub store_fatal: Option<bool>,
    /// Bounded store-failure detail safe for the Admin projection.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub store_error: Option<String>,
    /// Last bounded graceful-shutdown lifecycle state.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub shutdown_state: Option<String>,
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub shutdown_started_at: Option<Rfc3339>,
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub shutdown_deadline_at: Option<Rfc3339>,
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub shutdown_finished_at: Option<Rfc3339>,
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub shutdown_unresolved_range: Option<(u64, u64)>,
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub shutdown_last_error: Option<String>,
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub shutdown_forced: Option<bool>,
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub shutdown_report_id: Option<ReportId>,
}

/// One Node's combined current observation (process + chain).
///
/// Observations from different Nodes remain separate and are never merged
/// into one Agent-level chain observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeObservation {
    /// The Node these observations belong to. Must be present in the
    /// Inventory of the same report.
    pub node_id: NodeId,
    /// Process-level observation; `disabled` when no selector is configured,
    /// `unsupported` when the Agent cannot collect it.
    pub process: ComponentObservation<ProcessCurrent>,
    /// Recursive byte size of the explicitly configured PlatON data directory.
    /// Older Agents omit this component; an omitted value is not zero.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub data_directory_size_bytes: Option<ComponentObservation<u64>>,
    /// Total byte capacity of the filesystem containing the configured data
    /// directory. It is sampled with the directory size so Public projections
    /// can present a truthful capacity ratio. Older Agents omit this component.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub data_directory_capacity_bytes: Option<ComponentObservation<u64>>,
    /// Agent monotonic elapsed time spent collecting this Node snapshot, in
    /// milliseconds. This is duration data, not an RFC3339 timestamp.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub monotonic_elapsed_ms: Option<u64>,
    /// Chain-facing observation of this Node.
    pub chain: NodeChainObservation,
}

/// Node process snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessCurrent {
    /// Process ID; identity is validated against start time/executable to
    /// avoid PID reuse.
    pub pid: u64,
    /// Process start time.
    pub started_at: Rfc3339,
    /// Process CPU utilization in percent.
    pub cpu_percent: f64,
    /// Process resident memory in bytes.
    pub memory_bytes: u64,
    /// Process uptime in milliseconds.
    pub uptime_ms: u64,
}

/// Chain-facing current observations of one Node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeChainObservation {
    /// RPC reachability and capability probe results.
    pub rpc: ComponentObservation<RpcCurrent>,
    /// Synchronization state and current head.
    pub sync: ComponentObservation<SyncCurrent>,
    /// Bounded consensus current state (`debug_consensusStatus`).
    pub consensus: ComponentObservation<ConsensusCurrent>,
    /// Observed Network Identity of this Node.
    pub network_identity: ComponentObservation<NetworkIdentity>,
    /// Slow-changing Node identity/metadata (5-minute cadence).
    pub static_metadata: ComponentObservation<NodeStaticMetadata>,
    /// Optional bounded current Peer Snapshot. Older Agents omit this field;
    /// an omitted component is not an authoritative empty snapshot.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub peers: Option<ComponentObservation<PeerSnapshot>>,
}

/// RPC reachability + capability probe results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcCurrent {
    /// Client version string observed from the Node
    /// (contract limit: 256 chars).
    pub client_version: String,
    /// Namespaces actually registered (contract limit: 64 entries of 64
    /// chars).
    pub namespaces: Vec<String>,
    /// Methods actually probed as available (contract limit: 512 entries of
    /// 128 chars).
    pub methods: Vec<String>,
}

/// Synchronization state of one Node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncCurrent {
    /// Whether the Node reports active syncing.
    pub syncing: bool,
    /// Current head block height.
    pub current_block: u64,
    /// Highest known block height.
    pub highest_block: u64,
    /// Pulled states (sync progress).
    pub pulled_states: u64,
    /// Known states (sync progress).
    pub known_states: u64,
}

/// Bounded consensus current state (`debug_consensusStatus`).
///
/// `validator == true` only means the current validator pool contains this
/// Node; it never creates a Validator or proves a block was proposed by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsensusCurrent {
    /// Current epoch.
    pub epoch: u64,
    /// Current view number.
    pub view_number: u64,
    /// Whether the current validator membership includes this Node.
    pub validator: bool,
    /// Highest QC block height.
    pub highest_qc_block: u64,
    /// Highest lock block height.
    pub highest_lock_block: u64,
    /// Highest commit block height.
    pub highest_commit_block: u64,
}

/// Slow-changing Node identity metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeStaticMetadata {
    /// Fingerprint of the P2P Node key (`admin_nodeInfo`), used for
    /// seal-signer comparisons.
    pub node_key_fingerprint: FingerprintHex,
    /// Observed enode URI (sensitive, Admin-only; contract limit: 512
    /// chars).
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub enode: Option<String>,
}

/// A bounded successful Peer Snapshot for one Node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerSnapshot {
    /// Peers observed in this snapshot. An empty list is authoritative.
    pub peers: Vec<PeerCurrent>,
}

/// Direction of a current P2P connection from the observed Node's view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerDirection {
    Inbound,
    Outbound,
}

/// Bounded, privacy-safe current Peer metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerCurrent {
    /// Stable remote Peer ID; never replaced with an IP address.
    pub peer_id: String,
    /// Canonical literal IPv4/IPv6 address, when the Node supplied one.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub remote_ip: Option<String>,
    pub direction: PeerDirection,
    pub trusted: bool,
    pub static_peer: bool,
    pub consensus_peer: bool,
    /// Bounded client name from the Peer handshake.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub client_name: Option<String>,
    /// Bounded advertised capability names.
    pub caps: Vec<String>,
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub cbft_protocol_version: Option<u64>,
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub cbft_highest_qc_block: Option<u64>,
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub cbft_locked_block: Option<u64>,
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub cbft_commit_block: Option<u64>,
}
