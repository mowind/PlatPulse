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
use crate::identity::NodeId;
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
