//! Minimal one-shot collection pipeline for the first Agent vertical slice.
//!
//! The collector samples Host state once, queries each configured Node through
//! its own injected RPC adapter call, builds a complete AgentReport, and
//! stores the immutable bytes before any sender can deliver them. The adapter
//! is deliberately injected so production transports and scripted RPC fakes
//! use the same report-building path.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use platpulse_core::component::{BoundedError, ComponentObservation, ComponentStatus};
use platpulse_core::identity::{AgentId, BootId, NodeId, ReportId};
use platpulse_core::inventory::{InventoryNode, NodeInventory};
use platpulse_core::network::{NetworkIdentity, RpcEndpoint};
use platpulse_core::observation::{
    ConsensusCurrent, DiskCurrent, HostObservation, LoadCurrent, MemoryCurrent,
    NodeChainObservation, NodeObservation, NodeStaticMetadata, PeerSnapshot, RpcCurrent,
    SpoolDiagnostics, SyncCurrent,
};
use platpulse_core::{
    AgentCapability, AgentReport, BootTransition, FingerprintHex, NodeCurrentDisposition,
    ReceiptDisposition, ReportReceipt, Rfc3339, SampleDispositionKind, SampleRef,
};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use sqlx::Connection;
use sysinfo::{Disks, Networks, System};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use tokio_util::sync::CancellationToken;

use crate::block::{
    HeadSubscription, NodeSubscriptions, WebSocketBlockTransport, load_block_summaries,
};
use crate::config::AgentConfig;
use crate::database::{
    AgentDatabaseConfig, AgentStore, AgentStoreWritePermit, applied_receipt_expiry_cutoff,
    delete_expired_receipt_markers,
};
use crate::reporting::ReportStoreError;
pub const MAX_SPOOL_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_SPOOL_AGE_SECONDS: u64 = 24 * 60 * 60;
pub const PREFLUSH_SPOOL_BYTES: u64 = 1536 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpoolPolicy {
    pub max_bytes: u64,
    pub max_age_seconds: u64,
    pub preflush_bytes: u64,
}

impl Default for SpoolPolicy {
    fn default() -> Self {
        Self {
            max_bytes: MAX_SPOOL_BYTES,
            max_age_seconds: MAX_SPOOL_AGE_SECONDS,
            preflush_bytes: PREFLUSH_SPOOL_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpoolCleanupSummary {
    pub dropped_reports: u64,
    pub dropped_samples: u64,
    pub sequence_range: Option<(u64, u64)>,
    pub time_range: Option<(String, String)>,
    pub height_range: Option<(u64, u64)>,
    pub pending_history_gaps: u64,
}

/// Result of probing one bounded RPC component. `Unsupported` means the
/// capability was absent on this running Node; `Error` means the call failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum ProbeValue<T> {
    Supported(T),
    Unsupported,
    Error(String),
}

/// One bounded result from the Node RPC capability/identity probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcSnapshot {
    pub client_version: String,
    pub namespaces: Vec<String>,
    pub methods: Vec<String>,
    pub network_identity: NetworkIdentity,
    pub node_key_fingerprint: FingerprintHex,
    #[serde(default)]
    pub enode: Option<String>,
    pub sync: ProbeValue<SyncCurrent>,
    pub consensus: ProbeValue<ConsensusCurrent>,
    pub peers: ProbeValue<PeerSnapshot>,
}

/// Injected Node RPC adapter. Implementations must perform one connection
/// probe and return bounded, already-redacted values.
pub trait RpcAdapter {
    fn collect(&self, endpoint: &RpcEndpoint) -> Result<RpcSnapshot, RpcCollectError>;
}

struct PrecollectedRpcAdapter {
    snapshots: HashMap<String, Result<RpcSnapshot, String>>,
}

impl RpcAdapter for PrecollectedRpcAdapter {
    fn collect(&self, endpoint: &RpcEndpoint) -> Result<RpcSnapshot, RpcCollectError> {
        match self.snapshots.get(endpoint.as_str()) {
            Some(Ok(snapshot)) => Ok(snapshot.clone()),
            Some(Err(error)) => Err(RpcCollectError::Failed(error.clone())),
            None => Err(RpcCollectError::Failed(
                "no precollected snapshot for endpoint".to_owned(),
            )),
        }
    }
}

/// Deterministic adapter useful for scripted local fakes and integration tests.
#[derive(Debug, Clone)]
pub struct ScriptedRpcAdapter {
    snapshots: HashMap<String, RpcSnapshot>,
    failures: HashSet<String>,
}

impl ScriptedRpcAdapter {
    pub fn new(snapshot: RpcSnapshot) -> Self {
        Self {
            snapshots: HashMap::new(),
            failures: HashSet::new(),
        }
        .with_default(snapshot)
    }

    fn with_default(mut self, snapshot: RpcSnapshot) -> Self {
        self.snapshots.insert("*".to_owned(), snapshot);
        self
    }

    /// Build a deterministic multi-Node fake keyed by each Node's endpoint.
    pub fn for_nodes(nodes: impl IntoIterator<Item = (RpcEndpoint, RpcSnapshot)>) -> Self {
        Self {
            snapshots: nodes
                .into_iter()
                .map(|(endpoint, snapshot)| (endpoint.as_str().to_owned(), snapshot))
                .collect(),
            failures: HashSet::new(),
        }
    }

    /// Make one endpoint fail without affecting other endpoints.
    pub fn fail_endpoint(mut self, endpoint: &RpcEndpoint) -> Self {
        self.failures.insert(endpoint.as_str().to_owned());
        self
    }
    /// Parse one scripted snapshot from JSON.
    pub fn from_json(body: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(body).map(Self::new)
    }
}
impl RpcAdapter for ScriptedRpcAdapter {
    fn collect(&self, endpoint: &RpcEndpoint) -> Result<RpcSnapshot, RpcCollectError> {
        let key = endpoint.as_str();
        if self.failures.contains(key) {
            return Err(RpcCollectError::Failed(
                "scripted transport failure".to_owned(),
            ));
        }
        self.snapshots
            .get(key)
            .or_else(|| self.snapshots.get("*"))
            .cloned()
            .ok_or_else(|| RpcCollectError::Failed("no scripted snapshot for endpoint".to_owned()))
    }
}
/// Adapter used by the production CLI until a real RPC transport is wired.
/// It fails closed instead of inventing Node data or persisting a misleading
/// Healthy report.
#[derive(Debug, Clone, Copy, Default)]
pub struct FailClosedRpcAdapter;

impl RpcAdapter for FailClosedRpcAdapter {
    fn collect(&self, _endpoint: &RpcEndpoint) -> Result<RpcSnapshot, RpcCollectError> {
        Err(RpcCollectError::Failed(
            "RPC transport is not configured; no report was persisted".to_owned(),
        ))
    }
}

#[derive(Debug, Error)]
pub enum RpcCollectError {
    #[error("RPC probe failed: {0}")]
    Failed(String),
}

#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("Agent Store initialization failed: {0}")]
    Store(#[from] crate::database::AgentDatabaseError),
    #[error("Agent recovery drain is required before starting a new collector")]
    RecoveryRequired,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Agent is not enrolled")]
    NotEnrolled,
    #[error("Agent runtime ownership failed: {0}")]
    RuntimeOwnership(String),
    #[error("invalid persisted Agent identity: {0}")]
    Identity(String),
    #[error("RPC collection failed: {0}")]
    Rpc(#[from] RpcCollectError),
    #[error("report persistence failed: {0}")]
    Report(#[from] ReportStoreError),
    #[error("report serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Agent state changed while assembling the report")]
    ConcurrentStateChange,
}

type CollectionState = (
    String,
    i64,
    Option<String>,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

async fn load_collection_state(
    store: &mut AgentStore,
) -> Result<Option<CollectionState>, sqlx::Error> {
    sqlx::query_as(
        "SELECT agent_id, agent_epoch, boot_id, report_sequence, boot_state, previous_boot_id, pending_transition, pending_previous_boot_id FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(store.connection())
    .await
}

pub(crate) fn is_transient_database_lock(error: &CollectionError) -> bool {
    match error {
        CollectionError::Database(error) => crate::database::is_lock_contention(error),
        CollectionError::Report(crate::reporting::ReportStoreError::Database(error)) => {
            crate::database::is_lock_contention(error)
        }
        _ => false,
    }
}

pub(crate) fn timestamp() -> Rfc3339 {
    let value = OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("zero nanoseconds is valid")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC timestamp is valid");
    value.parse().expect("UTC timestamp is valid")
}

pub(crate) fn ok<T>(value: T, at: Rfc3339) -> ComponentObservation<T> {
    ComponentObservation {
        status: ComponentStatus::Ok,
        attempted_at: Some(at),
        latest_observed_at: Some(at),
        received_at: None,
        state_revision: 1,
        value_revision: 1,
        latest: Some(value),
        error: None,
    }
}

#[cfg(test)]
#[allow(dead_code)]
fn disabled<T>() -> ComponentObservation<T> {
    ComponentObservation {
        status: ComponentStatus::Disabled,
        attempted_at: None,
        latest_observed_at: None,
        received_at: None,
        state_revision: 1,
        value_revision: 0,
        latest: None,
        error: None,
    }
}

fn unsupported<T>() -> ComponentObservation<T> {
    ComponentObservation {
        status: ComponentStatus::Unsupported,
        attempted_at: None,
        latest_observed_at: None,
        received_at: None,
        state_revision: 1,
        value_revision: 0,
        latest: None,
        error: None,
    }
}

/// Build a component error without fabricating a value when the exchange is
/// unavailable. The Server can still accept the rest of the report.
pub(crate) fn clock_skew_error(at: Rfc3339, message: &str) -> ComponentObservation<i64> {
    error(at, "clock_exchange_unavailable", message)
}

/// Collect Host metrics once for this Agent. Disk/network structures are
/// intentionally bounded; empty disk is an authoritative snapshot, and
/// network throughput is measured over a short local sampling interval.
pub fn collect_host(
    system: &mut System,
    disks: &mut Disks,
    at: Rfc3339,
    clock_skew: ComponentObservation<i64>,
) -> HostObservation {
    let started = std::time::Instant::now();
    system.refresh_cpu_usage();
    system.refresh_memory();
    disks.refresh();
    let cpu = system.global_cpu_info().cpu_usage().clamp(0.0, 100.0) as f64;
    let load = System::load_average();
    let network_started = std::time::Instant::now();
    let mut networks = Networks::new_with_refreshed_list();
    std::thread::sleep(std::time::Duration::from_millis(100));
    networks.refresh();
    let elapsed = network_started.elapsed().as_secs_f64().max(0.001);
    let (received, transmitted) = networks.values().fold((0_u64, 0_u64), |(rx, tx), network| {
        (
            rx.saturating_add(network.received()),
            tx.saturating_add(network.transmitted()),
        )
    });
    let rate = |bytes: u64| (bytes as f64 / elapsed).round() as u64;
    let network_throughput = ok(
        platpulse_core::NetworkThroughput {
            rx_bytes_per_sec: rate(received),
            tx_bytes_per_sec: rate(transmitted),
        },
        at,
    );
    HostObservation {
        cpu_percent: ok(cpu, at),
        memory: ok(
            MemoryCurrent {
                total_bytes: system.total_memory(),
                used_bytes: system.used_memory(),
            },
            at,
        ),
        load: ok(
            LoadCurrent {
                load1: load.one,
                load5: load.five,
                load15: load.fifteen,
            },
            at,
        ),
        disk: ok(
            DiskCurrent {
                mounts: disks
                    .list()
                    .iter()
                    .take(128)
                    .map(|disk| {
                        let total = disk.total_space();
                        let available = disk.available_space();
                        platpulse_core::MountUsage {
                            mount_path: disk.mount_point().to_string_lossy().into_owned(),
                            total_bytes: total,
                            used_bytes: total.saturating_sub(available),
                        }
                    })
                    .collect(),
            },
            at,
        ),
        network_throughput,
        monotonic_elapsed_ms: Some(started.elapsed().as_millis() as u64),
        clock_skew,
        spool: ok(
            SpoolDiagnostics {
                queued_bytes: 0,
                queued_reports: 0,
                oldest_queued_age_ms: 0,
                dropped_reports: 0,
                dropped_samples: 0,
                in_flight: None,
                last_delivery_error: None,
                last_delivery_at: None,
                capacity_bytes: Some(MAX_SPOOL_BYTES),
                max_age_seconds: Some(MAX_SPOOL_AGE_SECONDS),
                dropped_sequence_range: None,
                dropped_time_range: None,
                dropped_height_range: None,
                pending_history_gaps: Some(0),
                report_too_large: Some(false),
                store_fatal: Some(false),
                store_error: None,
                shutdown_state: None,
                shutdown_started_at: None,
                shutdown_deadline_at: None,
                shutdown_finished_at: None,
                shutdown_unresolved_range: None,
                shutdown_last_error: None,
                shutdown_forced: None,
                shutdown_report_id: None,
            },
            at,
        ),
    }
}

/// Carry a component's last successful value through a failed attempt while
/// retaining the current attempt state and error. This is the Agent-side
/// half of the Observation Envelope last-good contract; the Server applies
/// the same rule again at its Current Projection boundary.
fn preserve_last_good<T: Clone>(
    current: &mut ComponentObservation<T>,
    previous: &ComponentObservation<T>,
) {
    if current.status == ComponentStatus::Error
        && current.latest.is_none()
        && previous.latest.is_some()
    {
        current.latest = previous.latest.clone();
        current.latest_observed_at = previous.latest_observed_at;
        current.value_revision = previous.value_revision;
    }
}

fn preserve_last_good_values(current: &mut AgentReport, previous: &AgentReport) {
    preserve_last_good(&mut current.host.cpu_percent, &previous.host.cpu_percent);
    preserve_last_good(&mut current.host.memory, &previous.host.memory);
    preserve_last_good(&mut current.host.load, &previous.host.load);
    preserve_last_good(&mut current.host.disk, &previous.host.disk);
    preserve_last_good(
        &mut current.host.network_throughput,
        &previous.host.network_throughput,
    );
    preserve_last_good(&mut current.host.clock_skew, &previous.host.clock_skew);
    preserve_last_good(&mut current.host.spool, &previous.host.spool);

    for node in &mut current.nodes {
        let Some(previous_node) = previous
            .nodes
            .iter()
            .find(|candidate| candidate.node_id == node.node_id)
        else {
            continue;
        };
        preserve_last_good(&mut node.process, &previous_node.process);
        if let (Some(current_data_directory), Some(previous_data_directory)) = (
            &mut node.data_directory_size_bytes,
            &previous_node.data_directory_size_bytes,
        ) {
            preserve_last_good(current_data_directory, previous_data_directory);
        }
        if let (Some(current_data_directory), Some(previous_data_directory)) = (
            &mut node.data_directory_capacity_bytes,
            &previous_node.data_directory_capacity_bytes,
        ) {
            preserve_last_good(current_data_directory, previous_data_directory);
        }
        preserve_last_good(&mut node.chain.rpc, &previous_node.chain.rpc);
        preserve_last_good(&mut node.chain.sync, &previous_node.chain.sync);
        preserve_last_good(&mut node.chain.consensus, &previous_node.chain.consensus);
        preserve_last_good(
            &mut node.chain.network_identity,
            &previous_node.chain.network_identity,
        );
        preserve_last_good(
            &mut node.chain.static_metadata,
            &previous_node.chain.static_metadata,
        );
        if let (Some(current_peers), Some(previous_peers)) =
            (&mut node.chain.peers, &previous_node.chain.peers)
        {
            preserve_last_good(current_peers, previous_peers);
        }
    }
}

fn add_sample_capabilities(report: &mut AgentReport) {
    if !report.block_summaries.is_empty()
        && !report
            .agent_capabilities
            .contains(&AgentCapability::BlockSummary)
    {
        report
            .agent_capabilities
            .push(AgentCapability::BlockSummary);
    }
}

pub(crate) async fn load_last_report(
    store: &mut AgentStore,
) -> Result<Option<AgentReport>, sqlx::Error> {
    let body: Option<Option<Vec<u8>>> = sqlx::query_scalar::<_, Option<Vec<u8>>>(
        "SELECT last_report_body FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(store.connection())
    .await?;
    let Some(body) = body.flatten() else {
        return Ok(None);
    };
    let report = serde_json::from_slice::<AgentReport>(&body).map_err(|error| {
        sqlx::Error::Protocol(format!("last report snapshot is invalid: {error}"))
    })?;
    report.validate().map_err(|error| {
        sqlx::Error::Protocol(format!("last report snapshot failed validation: {error}"))
    })?;
    Ok(Some(report))
}

fn error<T>(at: Rfc3339, code: &str, message: &str) -> ComponentObservation<T> {
    ComponentObservation {
        status: ComponentStatus::Error,
        attempted_at: Some(at),
        latest_observed_at: None,
        received_at: None,
        state_revision: 1,
        value_revision: 0,
        latest: None,
        error: Some(BoundedError {
            code: code.to_owned(),
            message: message.to_owned(),
        }),
    }
}

fn probe_component<T>(probe: ProbeValue<T>, at: Rfc3339) -> ComponentObservation<T> {
    match probe {
        ProbeValue::Supported(value) => ok(value, at),
        ProbeValue::Unsupported => unsupported(),
        ProbeValue::Error(message) => error(at, "rpc_method_failed", &message),
    }
}

fn collect_node<A: RpcAdapter>(
    system: &mut System,
    node: &platpulse_core::inventory::InventoryNode,
    data_directory_size_bytes: Option<ComponentObservation<u64>>,
    data_directory_capacity_bytes: Option<ComponentObservation<u64>>,
    attempted: Rfc3339,
    adapter: &A,
) -> NodeObservation {
    let started = std::time::Instant::now();
    let process = crate::process::collect(system, node.process.as_ref(), attempted);
    let (rpc, network_identity, static_metadata, sync, consensus, peers) =
        match adapter.collect(&node.rpc_endpoint) {
            Ok(snapshot) => (
                ok(
                    RpcCurrent {
                        client_version: snapshot.client_version,
                        namespaces: snapshot.namespaces,
                        methods: snapshot.methods,
                    },
                    attempted,
                ),
                ok(snapshot.network_identity, attempted),
                ok(
                    NodeStaticMetadata {
                        node_key_fingerprint: snapshot.node_key_fingerprint,
                        enode: snapshot.enode,
                    },
                    attempted,
                ),
                probe_component(snapshot.sync, attempted),
                probe_component(snapshot.consensus, attempted),
                probe_component(snapshot.peers, attempted),
            ),
            Err(_) => (
                error(attempted, "rpc_unreachable", "RPC probe failed"),
                error(attempted, "rpc_unreachable", "RPC probe failed"),
                error(attempted, "rpc_unreachable", "RPC probe failed"),
                error(attempted, "rpc_unreachable", "RPC probe failed"),
                error(attempted, "rpc_unreachable", "RPC probe failed"),
                error(attempted, "rpc_unreachable", "RPC probe failed"),
            ),
        };
    NodeObservation {
        node_id: node.node_id,
        process,
        data_directory_size_bytes,
        data_directory_capacity_bytes,
        monotonic_elapsed_ms: Some(started.elapsed().as_millis() as u64),
        chain: NodeChainObservation {
            rpc,
            sync,
            consensus,
            network_identity,
            static_metadata,
            peers: Some(peers),
        },
    }
}

pub fn collect_block_summaries<R: crate::block::BlockResolver>(
    subscriptions: &mut NodeSubscriptions,
    resolver: &R,
    identities: &HashMap<platpulse_core::identity::NodeId, NetworkIdentity>,
    observed_at: Rfc3339,
) -> Vec<platpulse_core::block::BlockSummary> {
    let mut summaries = Vec::new();
    let ids: Vec<_> = identities.keys().copied().collect();
    for node_id in ids {
        if let (Some(subscription), Some(identity)) =
            (subscriptions.get_mut(&node_id), identities.get(&node_id))
        {
            if let Ok(Some(summary)) =
                subscription.resolve_next(resolver, identity.clone(), observed_at)
            {
                summaries.push(summary);
            }
        }
    }
    summaries
}

#[allow(clippy::too_many_arguments)]
pub fn collect_report_with_clock_skew<A: RpcAdapter>(
    config: &AgentConfig,
    agent_id: AgentId,
    agent_epoch: u64,
    boot_id: BootId,
    sequence: u64,
    inventory: NodeInventory,
    adapter: &A,
    clock_skew: ComponentObservation<i64>,
) -> Result<AgentReport, CollectionError> {
    let data_directories = inventory
        .nodes
        .iter()
        .map(|node| (node.node_id, crate::data_directory::disabled_observations()))
        .collect();
    collect_report_with_data_directories(
        config,
        agent_id,
        agent_epoch,
        boot_id,
        sequence,
        inventory,
        adapter,
        clock_skew,
        &data_directories,
    )
}

fn reconcile_data_directory_observation(
    mut current: ComponentObservation<u64>,
    previous: Option<&ComponentObservation<u64>>,
) -> ComponentObservation<u64> {
    if let Some(previous) = previous {
        current.state_revision = previous.state_revision
            + u64::from(current.status != previous.status || current.error != previous.error);
        current.value_revision = previous.value_revision
            + u64::from(current.latest.is_some() && current.latest != previous.latest);
    }
    current
}

fn previous_data_directory_observations(
    previous: Option<&AgentReport>,
    node_id: NodeId,
) -> (
    Option<&ComponentObservation<u64>>,
    Option<&ComponentObservation<u64>>,
) {
    let node = previous.and_then(|report| {
        report
            .nodes
            .iter()
            .find(|candidate| candidate.node_id == node_id)
    });
    (
        node.and_then(|item| item.data_directory_size_bytes.as_ref()),
        node.and_then(|item| item.data_directory_capacity_bytes.as_ref()),
    )
}

fn reconcile_data_directory_observations(
    mut current: crate::data_directory::DataDirectoryObservations,
    previous_size: Option<&ComponentObservation<u64>>,
    previous_capacity: Option<&ComponentObservation<u64>>,
) -> crate::data_directory::DataDirectoryObservations {
    current.size_bytes = reconcile_data_directory_observation(current.size_bytes, previous_size);
    current.capacity_bytes =
        reconcile_data_directory_observation(current.capacity_bytes, previous_capacity);
    current
}

fn collect_directory_observations(
    paths: &HashMap<NodeId, std::path::PathBuf>,
    inventory: &NodeInventory,
    previous: Option<&AgentReport>,
    attempted: Rfc3339,
) -> HashMap<NodeId, crate::data_directory::DataDirectoryObservations> {
    inventory
        .nodes
        .iter()
        .map(|node| {
            let (previous_size, previous_capacity) =
                previous_data_directory_observations(previous, node.node_id);
            let observations = match paths.get(&node.node_id) {
                None => reconcile_data_directory_observations(
                    crate::data_directory::disabled_observations(),
                    previous_size,
                    previous_capacity,
                ),
                Some(path) => previous_size
                    .filter(|observation| {
                        previous_capacity.is_some()
                            && observation.attempted_at.is_some_and(|last_attempt| {
                                attempted.as_datetime() >= last_attempt.as_datetime()
                                    && (attempted.as_datetime() - last_attempt.as_datetime())
                                        .whole_seconds()
                                        < crate::data_directory::DATA_DIRECTORY_SAMPLE_INTERVAL
                                            .as_secs()
                                            as i64
                            })
                    })
                    .zip(previous_capacity)
                    .map(|(size_bytes, capacity_bytes)| {
                        crate::data_directory::DataDirectoryObservations {
                            size_bytes: size_bytes.clone(),
                            capacity_bytes: capacity_bytes.clone(),
                        }
                    })
                    .unwrap_or_else(|| {
                        reconcile_data_directory_observations(
                            crate::data_directory::collect_observations(path, attempted),
                            previous_size,
                            previous_capacity,
                        )
                    }),
            };
            (node.node_id, observations)
        })
        .collect()
}

fn reconcile_precollected_observation(
    current: ComponentObservation<u64>,
    previous: Option<&ComponentObservation<u64>>,
) -> ComponentObservation<u64> {
    if current.status == ComponentStatus::Starting {
        if let Some(previous) = previous {
            return previous.clone();
        }
    }
    reconcile_data_directory_observation(current, previous)
}

fn reconcile_precollected_directory_observations(
    mut current: HashMap<NodeId, crate::data_directory::DataDirectoryObservations>,
    inventory: &NodeInventory,
    previous: Option<&AgentReport>,
) -> HashMap<NodeId, crate::data_directory::DataDirectoryObservations> {
    inventory
        .nodes
        .iter()
        .map(|node| {
            let (previous_size, previous_capacity) =
                previous_data_directory_observations(previous, node.node_id);
            let observations = match current.remove(&node.node_id) {
                Some(observations) => crate::data_directory::DataDirectoryObservations {
                    size_bytes: reconcile_precollected_observation(
                        observations.size_bytes,
                        previous_size,
                    ),
                    capacity_bytes: reconcile_precollected_observation(
                        observations.capacity_bytes,
                        previous_capacity,
                    ),
                },
                None => reconcile_data_directory_observations(
                    crate::data_directory::disabled_observations(),
                    previous_size,
                    previous_capacity,
                ),
            };
            (node.node_id, observations)
        })
        .collect()
}

fn closing_directory_observations(
    paths: &HashMap<NodeId, std::path::PathBuf>,
    inventory: &NodeInventory,
    previous: Option<&AgentReport>,
) -> HashMap<NodeId, crate::data_directory::DataDirectoryObservations> {
    inventory
        .nodes
        .iter()
        .map(|node| {
            let (previous_size, previous_capacity) =
                previous_data_directory_observations(previous, node.node_id);
            let observations = if paths.contains_key(&node.node_id) {
                crate::data_directory::DataDirectoryObservations {
                    size_bytes: previous_size
                        .cloned()
                        .unwrap_or_else(crate::data_directory::starting),
                    capacity_bytes: previous_capacity
                        .cloned()
                        .unwrap_or_else(crate::data_directory::starting),
                }
            } else {
                reconcile_data_directory_observations(
                    crate::data_directory::disabled_observations(),
                    previous_size,
                    previous_capacity,
                )
            };
            (node.node_id, observations)
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn collect_report_with_data_directories<A: RpcAdapter>(
    config: &AgentConfig,
    agent_id: AgentId,
    agent_epoch: u64,
    boot_id: BootId,
    sequence: u64,
    inventory: NodeInventory,
    adapter: &A,
    clock_skew: ComponentObservation<i64>,
    data_directories: &HashMap<NodeId, crate::data_directory::DataDirectoryObservations>,
) -> Result<AgentReport, CollectionError> {
    if inventory.nodes.is_empty() {
        let at = timestamp();
        let mut system = System::new_all();
        let mut disks = Disks::new_with_refreshed_list();
        let host = collect_host(&mut system, &mut disks, at, clock_skew);
        let report = AgentReport {
            protocol_version: platpulse_core::PROTOCOL_VERSION,
            agent_id,
            agent_epoch,
            boot_id,
            previous_boot_id: None,
            boot_transition: BootTransition::Continuing,
            report_sequence: sequence,
            report_id: ReportId::from_str(&Uuid::new_v4().to_string()).expect("UUID is valid"),
            generated_at: timestamp(),
            agent_version: crate::VERSION.to_owned(),
            agent_capabilities: vec![
                AgentCapability::RpcCapabilityProbe,
                AgentCapability::PeerSnapshot,
            ],
            inventory,
            host,
            nodes: vec![],
            block_summaries: vec![],
            history_gaps: vec![],
        };
        report
            .validate()
            .map_err(|error| CollectionError::Identity(error.to_string()))?;
        return Ok(report);
    }
    let attempted = timestamp();
    let mut system = System::new_all();
    let mut disks = Disks::new_with_refreshed_list();
    let host = collect_host(&mut system, &mut disks, attempted, clock_skew);
    let nodes = inventory
        .nodes
        .iter()
        .map(|node| {
            collect_node(
                &mut system,
                node,
                data_directories
                    .get(&node.node_id)
                    .map(|observations| observations.size_bytes.clone()),
                data_directories
                    .get(&node.node_id)
                    .map(|observations| observations.capacity_bytes.clone()),
                attempted,
                adapter,
            )
        })
        .collect::<Vec<_>>();
    let mut capabilities = vec![
        AgentCapability::RpcCapabilityProbe,
        AgentCapability::PeerSnapshot,
    ];
    if inventory.nodes.iter().any(|node| {
        node.process.as_ref().is_some_and(|selector| {
            matches!(
                selector,
                platpulse_core::inventory::ProcessSelector::SystemdUnit { .. }
            )
        })
    }) {
        capabilities.push(AgentCapability::ProcessSystemd);
    }
    if inventory.nodes.iter().any(|node| {
        node.process.as_ref().is_some_and(|selector| {
            matches!(
                selector,
                platpulse_core::inventory::ProcessSelector::PidFile { .. }
            )
        })
    }) {
        capabilities.push(AgentCapability::ProcessPidFile);
    }
    if nodes
        .iter()
        .any(|node| node.chain.sync.status != ComponentStatus::Unsupported)
    {
        capabilities.push(AgentCapability::SyncStatus);
    }
    if nodes
        .iter()
        .any(|node| node.chain.consensus.status != ComponentStatus::Unsupported)
    {
        capabilities.push(AgentCapability::ConsensusStatus);
    }
    let report = AgentReport {
        protocol_version: platpulse_core::PROTOCOL_VERSION,
        agent_id,
        agent_epoch,
        boot_id,
        previous_boot_id: None,
        boot_transition: BootTransition::Continuing,
        report_sequence: sequence,
        report_id: ReportId::from_str(&Uuid::new_v4().to_string()).expect("UUID is valid"),
        generated_at: timestamp(),
        agent_version: crate::VERSION.to_owned(),
        agent_capabilities: capabilities,
        inventory,
        host,
        nodes,
        block_summaries: vec![],
        history_gaps: vec![],
    };
    report
        .validate()
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let _ = config;
    Ok(report)
}

/// Build a complete report for every configured Node. Collection without a
/// time exchange is explicitly represented as an error, never as zero.
pub fn collect_report<A: RpcAdapter>(
    config: &AgentConfig,
    agent_id: AgentId,
    agent_epoch: u64,
    boot_id: BootId,
    sequence: u64,
    inventory: NodeInventory,
    adapter: &A,
) -> Result<AgentReport, CollectionError> {
    let at = timestamp();
    collect_report_with_clock_skew(
        config,
        agent_id,
        agent_epoch,
        boot_id,
        sequence,
        inventory,
        adapter,
        clock_skew_error(at, "Server time exchange was unavailable"),
    )
}

/// Recover an unclosed Agent boot before starting a new collector process.
/// Reports are delivered oldest-first, then an immutable Closing report is
/// persisted and delivered. A new boot is only created after apply_receipt
/// records the closing receipt transactionally.
pub async fn recover_previous_boot<A: RpcAdapter>(
    config: &AgentConfig,
    adapter: &A,
) -> Result<(), CollectionError> {
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| CollectionError::RuntimeOwnership(error.to_string()))?;
    recover_previous_boot_with_permit(config, adapter, AgentStoreWritePermit::new()).await
}

pub(crate) async fn recover_previous_boot_with_permit<A: RpcAdapter>(
    config: &AgentConfig,
    adapter: &A,
    write_permit: AgentStoreWritePermit,
) -> Result<(), CollectionError> {
    let mut store = AgentStore::open_with_write_permit(
        AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await?;
    crate::reporting::validate_receipt_history(&mut store).await?;
    let state: Option<(String, i64, Option<String>, i64, String)> = sqlx::query_as(
        "SELECT agent_id, agent_epoch, boot_id, report_sequence, boot_state FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(store.connection())
    .await?;
    let Some((agent_text, epoch, boot_text, sequence, boot_state)) = state else {
        return Err(CollectionError::NotEnrolled);
    };
    let Some(boot_text) = boot_text else {
        return Ok(());
    };
    if boot_state == "drained_pending" {
        return Ok(());
    }
    if let Some(last_report) = load_last_report(&mut store).await? {
        let last_report_id = last_report.report_id.to_string();
        let closing_receipt_exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM report_receipts WHERE report_id = ?")
                .bind(&last_report_id)
                .fetch_optional(store.connection())
                .await?;
        if last_report.boot_id.to_string() == boot_text
            && last_report.boot_transition == BootTransition::Closing
            && closing_receipt_exists.is_some()
        {
            let new_boot_id = BootId::from_str(&Uuid::new_v4().to_string()).expect("UUID is valid");
            let at = timestamp().to_string();
            let _write_permit = store.acquire_write().await;
            let mut tx = store.connection().begin().await?;
            let transaction_result: Result<(), CollectionError> = async {
                let current: Option<(String, i64, Option<String>, i64, String)> =
                    sqlx::query_as(
                        "SELECT agent_id, agent_epoch, boot_id, report_sequence, boot_state FROM agent_state WHERE singleton=1",
                    )
                    .fetch_optional(&mut *tx)
                    .await?;
                let state_matches = current.as_ref().is_some_and(
                    |(current_agent, current_epoch, current_boot, current_sequence, current_state)| {
                        current_agent == &agent_text
                            && *current_epoch == epoch
                            && current_boot.as_deref() == Some(boot_text.as_str())
                            && *current_sequence == sequence
                            && current_state == &boot_state
                    },
                );
                let receipt_exists: Option<i64> = sqlx::query_scalar(
                    "SELECT 1 FROM report_receipts WHERE report_id = ?",
                )
                .bind(&last_report_id)
                .fetch_optional(&mut *tx)
                .await?;
                if !state_matches || receipt_exists.is_none() {
                    return Err(CollectionError::ConcurrentStateChange);
                }
                let result = sqlx::query("UPDATE agent_state SET boot_id=?, report_sequence=0, boot_state='drained_pending', pending_transition='drained_previous', pending_previous_boot_id=?, previous_boot_id=?, close_report_id=?, close_applied_at=?, shutdown_state='final_stored', shutdown_finished_at=?, shutdown_last_error=NULL, shutdown_updated_at=?, updated_at=? WHERE singleton=1")
                    .bind(new_boot_id.to_string())
                    .bind(&boot_text)
                    .bind(&boot_text)
                    .bind(&last_report_id)
                    .bind(&at)
                    .bind(&at)
                    .bind(&at)
                    .bind(&at)
                    .execute(&mut *tx)
                    .await?;
                if result.rows_affected() != 1 {
                    return Err(CollectionError::ConcurrentStateChange);
                }
                Ok(())
            }
            .await;
            match transaction_result {
                Ok(()) => tx.commit().await?,
                Err(error) => {
                    tx.rollback().await?;
                    return Err(error);
                }
            }
            store.close().await?;
            return Ok(());
        }
    }
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    let transaction_result: Result<(), CollectionError> = async {
        let current: Option<(String, i64, Option<String>, i64, String)> = sqlx::query_as(
            "SELECT agent_id, agent_epoch, boot_id, report_sequence, boot_state FROM agent_state WHERE singleton=1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let state_matches = current.as_ref().is_some_and(
            |(current_agent, current_epoch, current_boot, current_sequence, current_state)| {
                current_agent == &agent_text
                    && *current_epoch == epoch
                    && current_boot.as_deref() == Some(boot_text.as_str())
                    && *current_sequence == sequence
                    && current_state == &boot_state
            },
        );
        if !state_matches {
            return Err(CollectionError::ConcurrentStateChange);
        }
        sqlx::query("UPDATE agent_state SET boot_state='draining', updated_at=? WHERE singleton=1")
            .bind(timestamp().to_string())
            .execute(&mut *tx)
            .await?;
        Ok(())
    }
    .await;
    match transaction_result {
        Ok(()) => tx.commit().await?,
        Err(error) => {
            tx.rollback().await?;
            return Err(error);
        }
    }
    drop(_write_permit);
    let transport = crate::reporting::HttpReportTransport::from_config(config)?;
    while let Some(delivered) = crate::reporting::deliver_one(&mut store, &transport).await? {
        if delivered_report_closes_boot(&delivered, &boot_text)? {
            store.close().await?;
            return Ok(());
        }
    }

    let agent_id = AgentId::from_str(&agent_text)
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let boot_id = BootId::from_str(&boot_text)
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let validated = config
        .validated_inventory()
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let previous = load_last_report(&mut store).await?;
    let data_directories = closing_directory_observations(
        &validated.data_directories,
        &validated.inventory,
        previous.as_ref(),
    );
    let at = timestamp();
    let mut closing = collect_report_with_data_directories(
        config,
        agent_id,
        epoch as u64,
        boot_id,
        sequence as u64 + 1,
        validated.inventory,
        adapter,
        clock_skew_error(at, "Server time exchange was unavailable during recovery"),
        &data_directories,
    )?;
    if let Some(previous) = previous.as_ref() {
        preserve_last_good_values(&mut closing, previous);
    }
    closing.boot_transition = BootTransition::Closing;
    closing.previous_boot_id = None;
    closing.block_summaries.clear();
    closing.history_gaps.clear();
    closing
        .validate()
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let body = serde_json::to_vec(&closing)?;
    crate::reporting::persist_closing_report(
        &mut store,
        &closing.report_id.to_string(),
        closing.agent_epoch,
        &closing.boot_id.to_string(),
        closing.report_sequence,
        &closing.generated_at.to_string(),
        &body,
        sequence as u64,
        "draining",
    )
    .await?;
    let delivered = crate::reporting::deliver_one(&mut store, &transport)
        .await?
        .ok_or(CollectionError::RecoveryRequired)?;
    if !delivered_report_closes_boot(&delivered, &boot_text)? {
        return Err(CollectionError::RecoveryRequired);
    }
    store.close().await?;
    Ok(())
}

fn delivered_report_closes_boot(
    delivered: &crate::reporting::StoredReport,
    boot_id: &str,
) -> Result<bool, CollectionError> {
    let report: AgentReport = serde_json::from_slice(&delivered.body)?;
    Ok(report.boot_id.to_string() == boot_id && report.boot_transition == BootTransition::Closing)
}

/// Collect and persist one complete immutable report. Agent state (identity,
/// boot and sequence) is advanced in the same transaction as the report body.
pub async fn collect_and_persist<A: RpcAdapter>(
    config: &AgentConfig,
    adapter: &A,
) -> Result<String, CollectionError> {
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| CollectionError::RuntimeOwnership(error.to_string()))?;
    collect_and_persist_with_permit(config, adapter, AgentStoreWritePermit::new()).await
}

pub(crate) async fn collect_and_persist_with_permit<A: RpcAdapter>(
    config: &AgentConfig,
    adapter: &A,
    write_permit: AgentStoreWritePermit,
) -> Result<String, CollectionError> {
    let mut store = AgentStore::open_with_write_permit(
        AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await?;
    let result = collect_and_persist_in_store(config, adapter, &mut store).await;
    store.close().await?;
    result
}

pub(crate) async fn collect_and_persist_precollected_in_store(
    config: &AgentConfig,
    snapshots: HashMap<String, Result<RpcSnapshot, String>>,
    data_directories: HashMap<NodeId, crate::data_directory::DataDirectoryObservations>,
    store: &mut AgentStore,
) -> Result<String, CollectionError> {
    collect_and_persist_in_store_with_data_directories(
        config,
        &PrecollectedRpcAdapter { snapshots },
        Some(data_directories),
        store,
    )
    .await
}

pub(crate) async fn collect_and_persist_in_store<A: RpcAdapter>(
    config: &AgentConfig,
    adapter: &A,
    store: &mut AgentStore,
) -> Result<String, CollectionError> {
    collect_and_persist_in_store_with_data_directories(config, adapter, None, store).await
}

async fn collect_and_persist_in_store_with_data_directories<A: RpcAdapter>(
    config: &AgentConfig,
    adapter: &A,
    precollected_data_directories: Option<
        HashMap<NodeId, crate::data_directory::DataDirectoryObservations>,
    >,
    store: &mut AgentStore,
) -> Result<String, CollectionError> {
    crate::reporting::ensure_spool_healthy(store).await?;
    let state: Option<CollectionState> = load_collection_state(store).await?;
    let (
        agent_text,
        epoch,
        boot_text,
        previous_sequence,
        boot_state,
        _previous_boot_id,
        pending_transition,
        pending_previous_boot_id,
    ) = state.ok_or(CollectionError::NotEnrolled)?;
    if boot_state == "draining" {
        return Err(CollectionError::RecoveryRequired);
    }
    let agent_id = AgentId::from_str(&agent_text)
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let boot_id = match boot_text {
        Some(value) => BootId::from_str(&value)
            .map_err(|error| CollectionError::Identity(error.to_string()))?,
        None => BootId::from_str(&Uuid::new_v4().to_string()).expect("UUID is valid"),
    };
    let validated = config
        .validated_inventory()
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let previous = load_last_report(store).await?;
    let clock_at = timestamp();
    let clock_skew = match crate::time_exchange::exchange_server_time(config).await {
        Ok(estimate) => ok(estimate.offset_ms, clock_at),
        Err(error) => clock_skew_error(clock_at, &error.to_string()),
    };
    let data_directories = match precollected_data_directories {
        Some(current) => reconcile_precollected_directory_observations(
            current,
            &validated.inventory,
            previous.as_ref(),
        ),
        None => collect_directory_observations(
            &validated.data_directories,
            &validated.inventory,
            previous.as_ref(),
            clock_at,
        ),
    };
    let mut report = collect_report_with_data_directories(
        config,
        agent_id,
        epoch as u64,
        boot_id,
        previous_sequence as u64 + 1,
        validated.inventory,
        adapter,
        clock_skew,
        &data_directories,
    )?;
    if let Some(previous) = previous.as_ref() {
        preserve_last_good_values(&mut report, previous);
    }
    if pending_transition.as_deref() == Some("drained_previous") {
        report.boot_transition = BootTransition::DrainedPrevious;
        report.previous_boot_id = pending_previous_boot_id
            .as_deref()
            .map(BootId::from_str)
            .transpose()
            .map_err(|error| CollectionError::Identity(error.to_string()))?;
    }
    report.block_summaries = load_block_summaries(store).await?;
    report.history_gaps = crate::block::load_history_gaps(store).await?;
    add_sample_capabilities(&mut report);
    report.host.spool = ok(
        spool_diagnostics_for_transition(
            current_spool_diagnostics(store).await?,
            report.boot_transition,
        ),
        report.generated_at,
    );
    report
        .validate()
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let body = serde_json::to_vec(&report)?;
    let digest = format!("0x{}", hex::encode(sha2::Sha256::digest(&body)));
    let now = report.generated_at.to_string();
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    let transaction_result: Result<(), CollectionError> = async {
        let committed_state: Option<(i64, Option<String>, i64, String)> = sqlx::query_as(
            "SELECT agent_epoch, boot_id, report_sequence, boot_state FROM agent_state WHERE singleton=1",
        )
    .fetch_optional(&mut *tx)
    .await?;
    let report_boot_id = report.boot_id.to_string();
    let state_matches =
        committed_state
            .as_ref()
            .is_some_and(|(epoch, boot_id, sequence, current_boot_state)| {
                *epoch == report.agent_epoch as i64
                    && *sequence == report.report_sequence as i64 - 1
                    && current_boot_state == &boot_state
                    && (boot_id.as_deref() == Some(report_boot_id.as_str()) || boot_id.is_none())
            });
        if !state_matches {
            return Err(CollectionError::ConcurrentStateChange);
        }
    sqlx::query("INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, in_flight, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?)")
        .bind(report.report_id.to_string()).bind(report.agent_epoch as i64).bind(report.boot_id.to_string()).bind(report.report_sequence as i64).bind(report.generated_at.to_string()).bind(&body).bind(&digest).bind(body.len() as i64).bind(&now).execute(&mut *tx).await?;
    if !crate::reporting::claim_report_samples(
        &mut tx,
        &report.report_id.to_string(),
        &report.block_summaries,
        &report.history_gaps,
    )
    .await?
    {
        return Err(CollectionError::ConcurrentStateChange);
    }
    crate::reporting::persist_last_report_snapshot(
        &mut tx,
        &report.agent_id.to_string(),
        report.agent_epoch,
        &report.boot_id.to_string(),
        report.report_sequence,
        &body,
    )
    .await?;
    let transition = match report.boot_transition {
        BootTransition::Continuing => "continuing",
        BootTransition::Closing => "closing",
        BootTransition::DrainedPrevious => "drained_previous",
        BootTransition::RecoveredAfterStale => "recovered_after_stale",
    };
    update_agent_state_for_persisted_report(&mut tx, &report, transition, &now).await?;
        Ok(())
    }
    .await;
    match transaction_result {
        Ok(()) => tx.commit().await?,
        Err(error) => {
            tx.rollback().await?;
            return Err(error);
        }
    }
    drop(_write_permit);
    crate::reporting::enforce_spool_policy(store, &crate::collector::SpoolPolicy::default(), &now)
        .await
        .map_err(CollectionError::Report)?;
    Ok(digest)
}

pub struct BlockWorkerExit {
    pub subscription: HeadSubscription,
    pub error: Option<CollectionError>,
}

type NodeRecoveryState = (
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

struct NodeRecoveryWrite {
    expected: Option<NodeRecoveryState>,
    boot_id: String,
    head: u64,
    pending: Option<(u64, u64)>,
    pending_trigger: Option<String>,
    pending_reason: Option<String>,
    observed_at: Rfc3339,
}

struct NodeRecoveryRequest<'a> {
    config: &'a AgentConfig,
    node: &'a InventoryNode,
    identity: NetworkIdentity,
    boot_id: BootId,
    observed_at: Rfc3339,
    reconnect_needed: bool,
}

async fn persist_recovery_state(
    store: &mut AgentStore,
    node_id: NodeId,
    update: NodeRecoveryWrite,
    summaries: &[platpulse_core::block::BlockSummary],
    gaps: &[platpulse_core::gap::HistoryGap],
) -> Result<(), CollectionError> {
    let NodeRecoveryWrite {
        expected,
        boot_id,
        head,
        pending,
        pending_trigger,
        pending_reason,
        observed_at,
    } = update;
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    let transaction_result: Result<(), CollectionError> = async {
        let current = sqlx::query_as::<_, (
            Option<String>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<String>,
            Option<String>,
        )>(
            "SELECT boot_id, last_head, pending_from, pending_to, pending_trigger, pending_reason FROM node_recovery_state WHERE node_id=?",
        )
        .bind(node_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if current != expected {
            return Err(CollectionError::ConcurrentStateChange);
        }
        let created_at = observed_at.to_string();
        for summary in summaries {
            crate::block::insert_block_summary(&mut tx, summary, &created_at).await?;
        }
        for gap in gaps {
            crate::block::insert_history_gap(&mut tx, gap).await?;
        }
        sqlx::query("INSERT INTO node_recovery_state (node_id, boot_id, last_head, pending_from, pending_to, pending_trigger, pending_reason, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET boot_id=excluded.boot_id,last_head=excluded.last_head,pending_from=excluded.pending_from,pending_to=excluded.pending_to,pending_trigger=excluded.pending_trigger,pending_reason=excluded.pending_reason,updated_at=excluded.updated_at")
            .bind(node_id.to_string())
            .bind(&boot_id)
            .bind(head as i64)
            .bind(pending.map(|value| value.0 as i64))
            .bind(pending.map(|value| value.1 as i64))
            .bind(pending_trigger)
            .bind(pending_reason)
            .bind(observed_at.to_string())
            .execute(&mut *tx)
            .await?;
        Ok(())
    }
    .await;
    match transaction_result {
        Ok(()) => tx.commit().await?,
        Err(error) => {
            tx.rollback().await?;
            return Err(error);
        }
    }
    Ok(())
}

async fn recover_node_blocks(
    store: &mut AgentStore,
    transport: &WebSocketBlockTransport,
    request: NodeRecoveryRequest<'_>,
) -> Result<bool, CollectionError> {
    let NodeRecoveryRequest {
        config,
        node,
        identity,
        boot_id,
        observed_at,
        reconnect_needed,
    } = request;
    let prior_recovery = sqlx::query_as::<_, NodeRecoveryState>(
        "SELECT boot_id, last_head, pending_from, pending_to, pending_trigger, pending_reason FROM node_recovery_state WHERE node_id=?",
    )
    .bind(node.node_id.to_string())
    .fetch_optional(store.connection())
    .await?;
    let Ok(head) = transport.current_head_async(&node.rpc_endpoint).await else {
        return Ok(false);
    };
    let boot_text = boot_id.to_string();
    let boot_changed =
        prior_recovery.as_ref().and_then(|row| row.0.as_deref()) != Some(boot_text.as_str());
    let plan = crate::block::plan_recovery(
        prior_recovery
            .as_ref()
            .and_then(|row| row.1)
            .map(|value| value as u64),
        head,
        boot_changed,
        reconnect_needed,
        false,
        None,
    );
    let bounds = crate::block::BackfillBounds {
        max_height_span: config.backfill.max_height_span,
        max_block_count: config.backfill.max_block_count,
        max_time: std::time::Duration::from_millis(config.backfill.max_time_ms),
    };
    if let Some(plan) = plan {
        let outcome = transport
            .gap_backfill(
                &node.rpc_endpoint,
                node.node_id,
                identity,
                observed_at,
                plan.from_height,
                plan.to_height,
                bounds,
                plan.trigger,
            )
            .await;
        let pending = outcome
            .gaps
            .first()
            .map(|gap| (gap.from_height, gap.to_height));
        persist_recovery_state(
            store,
            node.node_id,
            NodeRecoveryWrite {
                expected: prior_recovery.clone(),
                boot_id: boot_text.clone(),
                head,
                pending,
                pending_trigger: pending
                    .as_ref()
                    .map(|_| format!("{:?}", plan.trigger).to_lowercase()),
                pending_reason: outcome.gaps.first().map(|gap| gap.reason.clone()),
                observed_at,
            },
            &outcome.summaries,
            &outcome.gaps,
        )
        .await?;
    } else {
        persist_recovery_state(
            store,
            node.node_id,
            NodeRecoveryWrite {
                expected: prior_recovery,
                boot_id: boot_text,
                head,
                pending: None,
                pending_trigger: None,
                pending_reason: None,
                observed_at,
            },
            &[],
            &[],
        )
        .await?;
    }
    Ok(true)
}

/// Keep one Node's `newHeads` subscription open independently of report
/// assembly. Resolved Block Summaries are persisted immediately; the next
/// immutable report claims whatever summaries are pending at assembly time.
pub(crate) async fn run_node_block_worker(
    config: AgentConfig,
    node: InventoryNode,
    transport: WebSocketBlockTransport,
    cancel: CancellationToken,
    write_permit: AgentStoreWritePermit,
) -> BlockWorkerExit {
    // The live Alloy channel can contain `max_heads` notifications while the
    // worker's queue already contains up to twice that amount. Reserve space
    // for both before cancellation drains the channel into the shutdown-owned
    // queue.
    let queue_capacity = transport.max_heads.saturating_mul(3).max(1);
    let mut subscription = HeadSubscription::new(node.node_id, queue_capacity);
    let mut store = match AgentStore::open_with_write_permit(
        AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await
    {
        Ok(store) => store,
        Err(error) => {
            return BlockWorkerExit {
                subscription,
                error: Some(CollectionError::Store(error)),
            };
        }
    };

    loop {
        if cancel.is_cancelled() {
            return BlockWorkerExit {
                subscription,
                error: None,
            };
        }

        let latest = tokio::select! {
            _ = cancel.cancelled() => {
                return BlockWorkerExit { subscription, error: None };
            }
            result = load_last_report(&mut store) => match result {
                Ok(report) => report,
                Err(error) => {
                    return BlockWorkerExit {
                        subscription,
                        error: Some(CollectionError::Database(error)),
                    };
                }
            },
        };
        let Some((identity, boot_id)) = latest.and_then(|report| {
            report
                .nodes
                .iter()
                .find(|item| item.node_id == node.node_id)
                .and_then(|item| item.chain.network_identity.latest.clone())
                .map(|identity| (identity, report.boot_id))
        }) else {
            tokio::select! {
                _ = cancel.cancelled() => {
                    return BlockWorkerExit { subscription, error: None };
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
            }
            continue;
        };

        let live_result = tokio::select! {
            _ = cancel.cancelled() => {
                return BlockWorkerExit { subscription, error: None };
            }
            result = transport.open_live_head_subscription(&node.rpc_endpoint) => result,
        };
        let mut live = match live_result {
            Ok(live) => live,
            Err(_) => {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        return BlockWorkerExit { subscription, error: None };
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                }
                continue;
            }
        };
        let recovery_succeeded = match recover_node_blocks(
            &mut store,
            &transport,
            NodeRecoveryRequest {
                config: &config,
                node: &node,
                identity: identity.clone(),
                boot_id,
                observed_at: timestamp(),
                reconnect_needed: true,
            },
        )
        .await
        {
            Ok(succeeded) => succeeded,
            Err(error) => {
                return BlockWorkerExit {
                    subscription,
                    error: Some(error),
                };
            }
        };
        if recovery_succeeded {
            subscription.clear_loss();
        }

        'connected: loop {
            while let Some(header) = subscription.front_header().cloned() {
                let observed_at = timestamp();
                let summary = match tokio::select! {
                    _ = cancel.cancelled() => {
                        transport.drain_live_heads(&mut live, &mut subscription);
                        return BlockWorkerExit { subscription, error: None };
                    }
                    result = transport.resolve_live_head(
                        &live,
                        node.node_id,
                        &header,
                        &identity,
                        observed_at,
                    ) => result,
                } {
                    Ok(summary) => summary,
                    Err(_) => break 'connected,
                };
                let observed_at_text = observed_at.to_string();
                let persist_result =
                    crate::block::persist_block_summary(&mut store, &summary, &observed_at_text)
                        .await;
                if let Err(error) = persist_result {
                    return BlockWorkerExit {
                        subscription,
                        error: Some(CollectionError::Database(error)),
                    };
                }
                let _write_permit = tokio::select! {
                    _ = cancel.cancelled() => {
                        transport.drain_live_heads(&mut live, &mut subscription);
                        return BlockWorkerExit { subscription, error: None };
                    }
                    permit = store.acquire_write() => permit,
                };
                let state_result = sqlx::query("INSERT INTO node_recovery_state (node_id, boot_id, last_head, pending_from, pending_to, pending_trigger, pending_reason, updated_at) VALUES (?, ?, ?, NULL, NULL, NULL, NULL, ?) ON CONFLICT(node_id) DO UPDATE SET boot_id=excluded.boot_id,last_head=excluded.last_head,pending_from=NULL,pending_to=NULL,pending_trigger=NULL,pending_reason=NULL,updated_at=excluded.updated_at")
                    .bind(node.node_id.to_string())
                    .bind(boot_id.to_string())
                    .bind(summary.block_number as i64)
                    .bind(observed_at.to_string())
                    .execute(store.connection())
                    .await;
                if let Err(error) = state_result {
                    return BlockWorkerExit {
                        subscription,
                        error: Some(CollectionError::Database(error)),
                    };
                }
                subscription.pop_front();
            }

            let header = tokio::select! {
                _ = cancel.cancelled() => {
                    transport.drain_live_heads(&mut live, &mut subscription);
                    return BlockWorkerExit { subscription, error: None };
                }
                result = transport.receive_live_head(&mut live) => match result {
                    Ok(header) => header,
                    Err(crate::block::TransportError::HeadLagged
                        | crate::block::TransportError::MalformedHead(_)) => {
                        subscription.mark_loss();
                        break 'connected;
                    }
                    Err(_) => break 'connected,
                }
            };
            let header_height = header.block_number;
            if subscription.push(header).is_err() {
                break 'connected;
            }
            let _write_permit = tokio::select! {
                _ = cancel.cancelled() => {
                    transport.drain_live_heads(&mut live, &mut subscription);
                    return BlockWorkerExit { subscription, error: None };
                }
                permit = store.acquire_write() => permit,
            };
            let pending_result = sqlx::query("INSERT INTO node_recovery_state (node_id, boot_id, last_head, pending_from, pending_to, pending_trigger, pending_reason, updated_at) VALUES (?, ?, NULL, ?, ?, 'live_head', 'live head awaiting resolution', ?) ON CONFLICT(node_id) DO UPDATE SET boot_id=excluded.boot_id, pending_from=excluded.pending_from, pending_to=excluded.pending_to, pending_trigger=excluded.pending_trigger, pending_reason=excluded.pending_reason, updated_at=excluded.updated_at")
                .bind(node.node_id.to_string())
                .bind(boot_id.to_string())
                .bind(header_height as i64)
                .bind(header_height as i64)
                .bind(timestamp().to_string())
                .execute(store.connection())
                .await;
            if pending_result.is_err() {
                break 'connected;
            }
        }
        transport.drain_live_heads(&mut live, &mut subscription);

        tokio::select! {
            _ = cancel.cancelled() => {
                transport.drain_live_heads(&mut live, &mut subscription);
                return BlockWorkerExit { subscription, error: None };
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
        }
    }
}

/// Collect, persist, and include the normal per-Node subscription summaries in
/// the next immutable report. Each Node receives an independent subscription;
/// a transport failure is isolated and does not erase current observations.
pub async fn collect_and_persist_with_blocks<A: RpcAdapter>(
    config: &AgentConfig,
    adapter: &A,
    transport: &crate::block::WebSocketBlockTransport,
    subscriptions: &mut [crate::block::HeadSubscription],
) -> Result<String, CollectionError> {
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| CollectionError::RuntimeOwnership(error.to_string()))?;
    collect_and_persist_with_blocks_with_permit(
        config,
        adapter,
        transport,
        subscriptions,
        AgentStoreWritePermit::new(),
    )
    .await
}

pub(crate) async fn collect_and_persist_with_blocks_with_permit<A: RpcAdapter>(
    config: &AgentConfig,
    adapter: &A,
    transport: &crate::block::WebSocketBlockTransport,
    subscriptions: &mut [crate::block::HeadSubscription],
    write_permit: AgentStoreWritePermit,
) -> Result<String, CollectionError> {
    let mut store = AgentStore::open_with_write_permit(
        AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await?;
    crate::reporting::ensure_spool_healthy(&mut store).await?;
    let state: Option<CollectionState> = load_collection_state(&mut store).await?;
    let (
        agent_text,
        epoch,
        boot_text,
        previous_sequence,
        boot_state,
        _previous_boot_id,
        pending_transition,
        pending_previous_boot_id,
    ) = state.ok_or(CollectionError::NotEnrolled)?;
    if boot_state == "draining" {
        return Err(CollectionError::RecoveryRequired);
    }
    let agent_id = AgentId::from_str(&agent_text)
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let boot_id = match boot_text {
        Some(value) => BootId::from_str(&value)
            .map_err(|error| CollectionError::Identity(error.to_string()))?,
        None => BootId::from_str(&Uuid::new_v4().to_string()).expect("UUID is valid"),
    };
    let validated = config
        .validated_inventory()
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let clock_at = timestamp();
    let clock_skew = match crate::time_exchange::exchange_server_time(config).await {
        Ok(estimate) => ok(estimate.offset_ms, clock_at),
        Err(error) => clock_skew_error(clock_at, &error.to_string()),
    };
    let previous = load_last_report(&mut store).await?;
    let data_directories = collect_directory_observations(
        &validated.data_directories,
        &validated.inventory,
        previous.as_ref(),
        clock_at,
    );
    let inventory = validated.inventory;
    let mut report = collect_report_with_data_directories(
        config,
        agent_id,
        epoch as u64,
        boot_id,
        previous_sequence as u64 + 1,
        inventory.clone(),
        adapter,
        clock_skew,
        &data_directories,
    )?;
    if let Some(previous) = previous.as_ref() {
        preserve_last_good_values(&mut report, previous);
    }
    for node in &inventory.nodes {
        let Some(identity) = report
            .nodes
            .iter()
            .find(|item| item.node_id == node.node_id)
            .and_then(|item| item.chain.network_identity.latest.clone())
        else {
            continue;
        };
        let mut reconnect_needed = false;
        let subscription = subscriptions
            .iter_mut()
            .find(|subscription| subscription.node_id() == node.node_id);
        let summaries_result = match subscription {
            Some(subscription) => {
                transport
                    .collect_node_summaries_into(
                        &node.rpc_endpoint,
                        subscription,
                        identity.clone(),
                        report.generated_at,
                    )
                    .await
            }
            None => {
                transport
                    .collect_node_summaries(
                        &node.rpc_endpoint,
                        node.node_id,
                        identity.clone(),
                        report.generated_at,
                    )
                    .await
            }
        };
        match summaries_result {
            Ok(summaries) => {
                for summary in summaries {
                    crate::block::persist_block_summary(
                        &mut store,
                        &summary,
                        &report.generated_at.to_string(),
                    )
                    .await?;
                }
            }
            Err(_) => reconnect_needed = true,
        }
        let recovery_succeeded = recover_node_blocks(
            &mut store,
            transport,
            NodeRecoveryRequest {
                config,
                node,
                identity,
                boot_id: report.boot_id,
                observed_at: report.generated_at,
                reconnect_needed,
            },
        )
        .await?;
        if recovery_succeeded {
            if let Some(subscription) = subscriptions
                .iter_mut()
                .find(|subscription| subscription.node_id() == node.node_id)
            {
                subscription.clear_loss();
            }
        }
    }
    if pending_transition.as_deref() == Some("drained_previous") {
        report.boot_transition = BootTransition::DrainedPrevious;
        report.previous_boot_id = pending_previous_boot_id
            .as_deref()
            .map(BootId::from_str)
            .transpose()
            .map_err(|error| CollectionError::Identity(error.to_string()))?;
    }
    report.block_summaries = load_block_summaries(&mut store).await?;
    report.history_gaps = crate::block::load_history_gaps(&mut store).await?;
    add_sample_capabilities(&mut report);
    report.host.spool = ok(
        spool_diagnostics_for_transition(
            current_spool_diagnostics(&mut store).await?,
            report.boot_transition,
        ),
        report.generated_at,
    );
    report
        .validate()
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let body = serde_json::to_vec(&report)?;
    let digest = format!("0x{}", hex::encode(sha2::Sha256::digest(&body)));
    let now = report.generated_at.to_string();
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    let transaction_result: Result<(), CollectionError> = async {
        let committed_state: Option<(i64, Option<String>, i64, String)> = sqlx::query_as(
            "SELECT agent_epoch, boot_id, report_sequence, boot_state FROM agent_state WHERE singleton=1",
        )
    .fetch_optional(&mut *tx)
    .await?;
    let report_boot_id = report.boot_id.to_string();
    let state_matches =
        committed_state
            .as_ref()
            .is_some_and(|(epoch, boot_id, sequence, current_boot_state)| {
                *epoch == report.agent_epoch as i64
                    && *sequence == report.report_sequence as i64 - 1
                    && current_boot_state == &boot_state
                    && (boot_id.as_deref() == Some(report_boot_id.as_str()) || boot_id.is_none())
            });
        if !state_matches {
            return Err(CollectionError::ConcurrentStateChange);
        }
    sqlx::query("INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, in_flight, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?)")
        .bind(report.report_id.to_string()).bind(report.agent_epoch as i64).bind(report.boot_id.to_string()).bind(report.report_sequence as i64).bind(report.generated_at.to_string()).bind(&body).bind(&digest).bind(body.len() as i64).bind(&now).execute(&mut *tx).await?;
    if !crate::reporting::claim_report_samples(
        &mut tx,
        &report.report_id.to_string(),
        &report.block_summaries,
        &report.history_gaps,
    )
    .await?
    {
        return Err(CollectionError::ConcurrentStateChange);
    }
    crate::reporting::persist_last_report_snapshot(
        &mut tx,
        &report.agent_id.to_string(),
        report.agent_epoch,
        &report.boot_id.to_string(),
        report.report_sequence,
        &body,
    )
    .await?;
    let transition = match report.boot_transition {
        BootTransition::Continuing => "continuing",
        BootTransition::Closing => "closing",
        BootTransition::DrainedPrevious => "drained_previous",
        BootTransition::RecoveredAfterStale => "recovered_after_stale",
    };
    update_agent_state_for_persisted_report(&mut tx, &report, transition, &now).await?;
        Ok(())
    }
    .await;
    match transaction_result {
        Ok(()) => tx.commit().await?,
        Err(error) => {
            tx.rollback().await?;
            return Err(error);
        }
    }
    drop(_write_permit);
    crate::reporting::enforce_spool_policy(&mut store, &SpoolPolicy::default(), &now)
        .await
        .map_err(CollectionError::Report)?;
    store.close().await?;
    Ok(digest)
}

fn spool_diagnostics_for_transition(
    mut diagnostics: SpoolDiagnostics,
    transition: BootTransition,
) -> SpoolDiagnostics {
    if transition == BootTransition::DrainedPrevious {
        diagnostics.shutdown_state = Some("running".to_owned());
        diagnostics.shutdown_started_at = None;
        diagnostics.shutdown_deadline_at = None;
        diagnostics.shutdown_finished_at = None;
        diagnostics.shutdown_unresolved_range = None;
        diagnostics.shutdown_last_error = None;
        diagnostics.shutdown_forced = Some(false);
        diagnostics.shutdown_report_id = None;
    }
    diagnostics
}

async fn update_agent_state_for_persisted_report(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    report: &AgentReport,
    transition: &str,
    now: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_state SET agent_id=?, agent_epoch=?, boot_id=?, report_sequence=?, inventory_revision=?, boot_state=CASE WHEN ?='drained_previous' THEN 'active' ELSE boot_state END, pending_transition=CASE WHEN ?='drained_previous' THEN NULL ELSE pending_transition END, pending_previous_boot_id=CASE WHEN ?='drained_previous' THEN NULL ELSE pending_previous_boot_id END, updated_at=? WHERE singleton=1")
        .bind(report.agent_id.to_string()).bind(report.agent_epoch as i64).bind(report.boot_id.to_string()).bind(report.report_sequence as i64).bind(report.inventory.revision as i64).bind(transition).bind(transition).bind(transition).bind(now).execute(&mut **tx).await?;
    if transition == "drained_previous" {
        sqlx::query("UPDATE agent_state SET shutdown_state='running', shutdown_started_at=NULL, shutdown_deadline_at=NULL, shutdown_finished_at=NULL, shutdown_unresolved_from=NULL, shutdown_unresolved_to=NULL, shutdown_last_error=NULL, shutdown_forced=0, shutdown_report_id=NULL, shutdown_report_sequence=NULL, shutdown_updated_at=? WHERE singleton=1")
            .bind(now)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Read bounded local delivery state for the next immutable report.
pub(crate) async fn current_spool_diagnostics(
    store: &mut AgentStore,
) -> Result<SpoolDiagnostics, sqlx::Error> {
    let row = sqlx::query_as::<_, (i64, i64, i64, Option<String>)>(
        "SELECT COUNT(*), COALESCE(SUM(body_bytes), 0), COALESCE(MAX(in_flight), 0), MIN(created_at) FROM reports",
    )
    .fetch_one(store.connection())
    .await?;
    let failure = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT last_error, last_error_at FROM delivery_diagnostics WHERE singleton = 1",
    )
    .fetch_optional(store.connection())
    .await?;
    let oldest_queued_age_ms = row
        .3
        .as_deref()
        .and_then(|value| {
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                .ok()
                .and_then(|created| {
                    (OffsetDateTime::now_utc() - created)
                        .whole_milliseconds()
                        .try_into()
                        .ok()
                })
        })
        .unwrap_or(0);
    let state = sqlx::query_as::<_, (i64, i64, i64, Option<i64>, Option<i64>, Option<String>, Option<String>, Option<i64>, Option<i64>, i64, Option<String>, i64, i64, i64, i64, String)>(
        "SELECT max_bytes, max_age_seconds, dropped_reports, dropped_sequence_from, dropped_sequence_to, dropped_time_from, dropped_time_to, dropped_height_from, dropped_height_to, pending_history_gaps, store_error, report_too_large, store_fatal, dropped_samples, preflush_bytes, updated_at FROM spool_state WHERE singleton=1",
    ).fetch_optional(store.connection()).await?;
    let (
        capacity_bytes,
        max_age_seconds,
        dropped_reports,
        dropped_sequence_from,
        dropped_sequence_to,
        dropped_time_from,
        dropped_time_to,
        dropped_height_from,
        dropped_height_to,
        pending_history_gaps,
        store_error,
        report_too_large,
        store_fatal,
        dropped_samples,
        _,
        _,
    ) = state.unwrap_or((
        MAX_SPOOL_BYTES as i64,
        MAX_SPOOL_AGE_SECONDS as i64,
        0,
        None,
        None,
        None,
        None,
        None,
        None,
        0,
        None,
        0,
        0,
        0,
        PREFLUSH_SPOOL_BYTES as i64,
        String::new(),
    ));
    let shutdown = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>, Option<String>, Option<i64>, Option<String>)>(
        "SELECT shutdown_state, shutdown_started_at, shutdown_deadline_at, shutdown_finished_at, shutdown_last_error, shutdown_forced, shutdown_report_id, shutdown_unresolved_from, shutdown_unresolved_to FROM agent_state WHERE singleton=1",
    ).fetch_optional(store.connection()).await?;
    let (
        shutdown_state,
        shutdown_started_at,
        shutdown_deadline_at,
        shutdown_finished_at,
        shutdown_last_error,
        shutdown_forced,
        shutdown_report_id,
        shutdown_unresolved_from,
        shutdown_unresolved_to,
    ) = shutdown.unwrap_or((None, None, None, None, None, None, None, None, None));
    Ok(SpoolDiagnostics {
        queued_bytes: row.1.max(0) as u64,
        queued_reports: row.0.max(0) as u64,
        oldest_queued_age_ms,
        dropped_reports: dropped_reports.max(0) as u64,
        dropped_samples: dropped_samples.max(0) as u64,
        in_flight: Some(row.2 != 0),
        last_delivery_error: failure.as_ref().and_then(|value| value.0.clone()),
        last_delivery_at: failure
            .and_then(|value| value.1)
            .and_then(|value| value.parse().ok()),
        capacity_bytes: Some(capacity_bytes.max(0) as u64),
        max_age_seconds: Some(max_age_seconds.max(0) as u64),
        dropped_sequence_range: dropped_sequence_from
            .zip(dropped_sequence_to)
            .map(|(from, to)| (from.max(0) as u64, to.max(0) as u64)),
        dropped_time_range: dropped_time_from
            .zip(dropped_time_to)
            .and_then(|(from, to)| Some((from.parse().ok()?, to.parse().ok()?))),
        dropped_height_range: dropped_height_from
            .zip(dropped_height_to)
            .map(|(from, to)| (from.max(0) as u64, to.max(0) as u64)),
        pending_history_gaps: Some(pending_history_gaps.max(0) as u64),
        report_too_large: Some(report_too_large != 0),
        store_fatal: Some(store_fatal != 0),
        store_error,
        shutdown_state,
        shutdown_started_at: shutdown_started_at.and_then(|value| value.parse().ok()),
        shutdown_deadline_at: shutdown_deadline_at.and_then(|value| value.parse().ok()),
        shutdown_finished_at: shutdown_finished_at.and_then(|value| value.parse().ok()),
        shutdown_unresolved_range: shutdown_unresolved_from
            .zip(shutdown_unresolved_to)
            .map(|(from, to)| (from.max(0) as u64, to.parse::<u64>().unwrap_or(0))),
        shutdown_last_error,
        shutdown_forced: Some(shutdown_forced == Some(1)),
        shutdown_report_id: shutdown_report_id.and_then(|value| value.parse().ok()),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptEnvelope {
    receipt: ReportReceipt,
}

/// Apply a stored receipt and delete its report only after all receipt
/// dispositions have been durably processed, in one Agent Store transaction.
pub async fn apply_receipt(
    store: &mut AgentStore,
    report_id: &str,
    body_sha256: &str,
    disposition: &str,
    receipt_body: &[u8],
    applied_at: &str,
) -> Result<(), sqlx::Error> {
    let receipt = serde_json::from_slice::<ReceiptEnvelope>(receipt_body)
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?
        .receipt;
    receipt
        .validate()
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    let expected_disposition = receipt_disposition_name(receipt.disposition);
    if receipt.report_id.to_string() != report_id
        || receipt.report_body_sha256.to_string() != body_sha256
        || expected_disposition != disposition
    {
        return Err(sqlx::Error::Protocol(
            "receipt does not match report".to_owned(),
        ));
    }

    let expiry_cutoff = applied_receipt_expiry_cutoff(applied_at)?;
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    if let Some((marker_hash, marker_disposition)) = sqlx::query_as::<_, (String, String)>(
        "SELECT report_body_sha256, disposition FROM report_receipts WHERE report_id = ?",
    )
    .bind(report_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        let reason = if marker_hash == body_sha256 && marker_disposition == disposition {
            "duplicate receipt identity"
        } else {
            "receipt identity conflicts with an Applied Receipt Record"
        };
        sqlx::query(
            "UPDATE spool_state SET store_fatal=1, store_error=?, updated_at=? WHERE singleton=1",
        )
        .bind(reason)
        .bind(applied_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Err(sqlx::Error::Protocol(reason.to_owned()));
    }
    let transaction_result: Result<(), sqlx::Error> = async {
        let (raw_report, stored_body_sha256, stored_body_bytes): (Vec<u8>, String, i64) =
            sqlx::query_as("SELECT body, body_sha256, body_bytes FROM reports WHERE report_id = ?")
                .bind(report_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| {
                    sqlx::Error::Protocol("report is not in the Agent spool".to_owned())
                })?;
        let actual_hash = format!("0x{}", hex::encode(sha2::Sha256::digest(&raw_report)));
        if stored_body_bytes < 0
            || stored_body_bytes as usize != raw_report.len()
            || stored_body_sha256 != actual_hash
            || body_sha256 != actual_hash
        {
            return Err(sqlx::Error::Protocol(
                "stored report failed integrity validation".to_owned(),
            ));
        }
        let parsed_report: AgentReport = serde_json::from_slice(&raw_report).map_err(|error| {
            sqlx::Error::Protocol(format!("stored report is invalid: {error}"))
        })?;
        parsed_report.validate().map_err(|error| {
            sqlx::Error::Protocol(format!("stored report is invalid: {error}"))
        })?;
        if parsed_report.report_id.to_string() != report_id {
            return Err(sqlx::Error::Protocol(
                "stored report id mismatch".to_owned(),
            ));
        }
    let mut expected_nodes = std::collections::HashSet::new();
    for node in &parsed_report.inventory.nodes {
        expected_nodes.insert(node.node_id);
    }
    let mut seen_nodes = std::collections::HashSet::new();
    for node in &receipt.nodes {
        if !expected_nodes.contains(&node.node_id) || !seen_nodes.insert(node.node_id) {
            return Err(sqlx::Error::Protocol(
                "receipt contains invalid or duplicate node".to_owned(),
            ));
        }
        if node.current == NodeCurrentDisposition::Rejected
            && node.rejections.iter().any(|r| r.retryable)
        {
            return Err(sqlx::Error::Protocol(
                "rejected Node must be terminal".to_owned(),
            ));
        }
    }
    if receipt.disposition != ReceiptDisposition::Rejected
        && seen_nodes.len() != expected_nodes.len()
    {
        return Err(sqlx::Error::Protocol(
            "receipt does not cover every Node in the stored report".to_owned(),
        ));
    }
    let mut expected_samples = std::collections::HashSet::new();
    for sample in &parsed_report.block_summaries {
        expected_samples.insert((
            sample.node_id,
            "block",
            sample.block_number,
            sample.block_number,
        ));
    }
    for gap in &parsed_report.history_gaps {
        expected_samples.insert((gap.node_id, "gap", gap.from_height, gap.to_height));
    }
    let mut seen_samples = std::collections::HashSet::new();
    for sample in &receipt.samples {
        let (kind, from, to) = sample_reference(sample.sample);
        if !expected_samples.contains(&(sample.node_id, kind, from, to))
            || !seen_samples.insert((sample.node_id, kind, from, to))
        {
            return Err(sqlx::Error::Protocol(
                "receipt contains invalid or duplicate sample".to_owned(),
            ));
        }
    }
    if receipt.disposition != ReceiptDisposition::Rejected
        && seen_samples.len() != expected_samples.len()
    {
        return Err(sqlx::Error::Protocol(
            "receipt does not cover every sample in the stored report".to_owned(),
        ));
    }

    // The immutable report has already been parsed and all receipt references
    // validated above. Current observations are not copied into retry state.

    for sample in &receipt.samples {
        let (kind, from_height, to_height) = sample_reference(sample.sample);
        sqlx::query("DELETE FROM report_sample_assignments WHERE report_id = ? AND node_id = ? AND sample_kind = ? AND from_height = ? AND to_height = ?")
            .bind(report_id).bind(sample.node_id.to_string()).bind(kind)
            .bind(from_height as i64).bind(to_height as i64)
            .execute(&mut *tx).await?;
        match sample.disposition {
            SampleDispositionKind::Accepted | SampleDispositionKind::TerminalRejected => {
                if kind == "block" {
                    let block_hash = parsed_report
                        .block_summaries
                        .iter()
                        .find(|summary| {
                            summary.node_id == sample.node_id
                                && summary.block_number == from_height
                        })
                        .map(|summary| summary.block_hash.to_string())
                        .ok_or_else(|| {
                            sqlx::Error::Protocol(
                                "stored report is missing the referenced block summary".to_owned(),
                            )
                        })?;
                    sqlx::query(
                        "DELETE FROM block_summaries WHERE node_id = ? AND block_number = ? AND block_hash = ?",
                    )
                    .bind(sample.node_id.to_string())
                    .bind(from_height as i64)
                    .bind(block_hash)
                    .execute(&mut *tx)
                    .await?;
                } else {
                    let gap_kind = parsed_report
                        .history_gaps
                        .iter()
                        .find(|gap| {
                            gap.node_id == sample.node_id
                                && gap.from_height == from_height
                                && gap.to_height == to_height
                        })
                        .map(|gap| crate::block::gap_kind_name(gap.kind))
                        .ok_or_else(|| {
                            sqlx::Error::Protocol(
                                "stored report is missing the referenced history gap".to_owned(),
                            )
                        })?;
                    sqlx::query("DELETE FROM history_gaps WHERE node_id = ? AND from_height = ? AND to_height = ? AND kind = ?")
                        .bind(sample.node_id.to_string())
                        .bind(from_height as i64)
                        .bind(to_height as i64)
                        .bind(gap_kind)
                        .execute(&mut *tx)
                        .await?;
                }
                if sample.disposition == SampleDispositionKind::TerminalRejected {
                    if let Some(rejection) = &sample.rejection {
                        sqlx::query("INSERT INTO rejection_ledger (report_id, node_id, sample_kind, from_height, to_height, rejection_code, reason, rejected_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                            .bind(report_id)
                            .bind(sample.node_id.to_string())
                            .bind(kind)
                            .bind(from_height as i64)
                            .bind(to_height as i64)
                            .bind(format!("{:?}", rejection.code).to_lowercase())
                            .bind(&rejection.reason)
                            .bind(applied_at)
                            .execute(&mut *tx)
                            .await?;
                        sqlx::query("INSERT OR IGNORE INTO history_gaps (node_id, from_height, to_height, kind, created_at) VALUES (?, ?, ?, 'server_rejected', ?)")
                            .bind(sample.node_id.to_string())
                            .bind(from_height as i64)
                            .bind(to_height as i64)
                            .bind(applied_at)
                            .execute(&mut *tx)
                            .await?;
                    }
                }
            }
            // Leave retryable samples in the durable queue, now detached from
            // the old report. The next planner includes them once.
            SampleDispositionKind::RetryableRejected => {}
        }
    }

    sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, ?, ?)")
        .bind(report_id)
        .bind(body_sha256)
        .bind(disposition)
        .bind(applied_at)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM reports WHERE report_id = ? AND EXISTS (SELECT 1 FROM report_receipts WHERE report_id = ?)")
        .bind(report_id)
        .bind(report_id)
        .execute(&mut *tx)
        .await?;
    delete_expired_receipt_markers(&mut *tx, &expiry_cutoff).await?;
    if parsed_report.boot_transition == BootTransition::Closing {
        let new_boot_id =
            BootId::from_str(&Uuid::new_v4().to_string()).expect("UUID is valid");
        let closed_boot_id = parsed_report.boot_id.to_string();
        let result = sqlx::query(
            "UPDATE agent_state SET boot_id=?, report_sequence=0, boot_state='drained_pending', pending_transition='drained_previous', pending_previous_boot_id=?, previous_boot_id=?, close_report_id=?, close_applied_at=?, shutdown_state='final_stored', shutdown_finished_at=?, shutdown_last_error=NULL, shutdown_updated_at=?, updated_at=? WHERE singleton=1 AND agent_epoch=? AND boot_id=? AND report_sequence=? AND boot_state IN ('active', 'draining', 'final_stored')",
        )
        .bind(new_boot_id.to_string())
        .bind(&closed_boot_id)
        .bind(&closed_boot_id)
        .bind(report_id)
        .bind(applied_at)
        .bind(applied_at)
        .bind(applied_at)
        .bind(applied_at)
        .bind(parsed_report.agent_epoch as i64)
        .bind(&closed_boot_id)
        .bind(parsed_report.report_sequence as i64)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(sqlx::Error::Protocol(
                "Agent state changed while applying Closing receipt".to_owned(),
            ));
        }
    }
        Ok(())
    }
    .await;
    match transaction_result {
        Ok(()) => tx.commit().await,
        Err(error) => match tx.rollback().await {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(rollback_error),
        },
    }
}

fn sample_reference(sample: SampleRef) -> (&'static str, u64, u64) {
    match sample {
        SampleRef::Block { height } => ("block", height, height),
        SampleRef::Gap {
            from_height,
            to_height,
        } => ("gap", from_height, to_height),
    }
}

pub(crate) fn receipt_disposition_name(disposition: ReceiptDisposition) -> &'static str {
    match disposition {
        ReceiptDisposition::Accepted => "accepted",
        ReceiptDisposition::PartiallyAccepted => "partially_accepted",
        ReceiptDisposition::Rejected => "rejected",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::AgentDatabaseConfig;
    use platpulse_core::observation::{PeerCurrent, PeerDirection};
    use tempfile::tempdir;

    #[test]
    fn recognizes_a_delivered_closing_report_for_the_recovered_boot() {
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report.boot_transition = BootTransition::Closing;
        let stored = crate::reporting::StoredReport {
            report_id: report.report_id.to_string(),
            report_sequence: report.report_sequence,
            body: serde_json::to_vec(&report).unwrap(),
            body_sha256: "unused-in-this-check".to_owned(),
        };

        assert!(delivered_report_closes_boot(&stored, &report.boot_id.to_string()).unwrap());
        assert!(
            !delivered_report_closes_boot(&stored, "0195f2a1-0099-4099-8099-000000000099").unwrap()
        );
    }

    #[tokio::test]
    async fn stale_recovery_outcome_rolls_back_before_writing_gaps() {
        let directory = tempdir().unwrap();
        let mut store =
            AgentStore::open(AgentDatabaseConfig::new(directory.path().join("agent.db")))
                .await
                .unwrap();
        let node_id: NodeId = "0195f2a1-0014-4014-8014-000000000014".parse().unwrap();
        sqlx::query(
            "INSERT INTO node_recovery_state (node_id, boot_id, last_head, pending_from, pending_to, pending_trigger, pending_reason, updated_at) VALUES (?, ?, ?, NULL, NULL, NULL, NULL, ?)",
        )
        .bind(node_id.to_string())
        .bind("new-boot")
        .bind(10_i64)
        .bind("2026-01-01T00:00:00Z")
        .execute(store.connection())
        .await
        .unwrap();
        let gap = crate::shutdown::shutdown_gap(
            node_id,
            (11, 12),
            "2026-01-01T00:00:00Z".parse().unwrap(),
            "stale recovery outcome",
        );
        let result = persist_recovery_state(
            &mut store,
            node_id,
            NodeRecoveryWrite {
                expected: Some((Some("old-boot".to_owned()), Some(9), None, None, None, None)),
                boot_id: "old-boot".to_owned(),
                head: 12,
                pending: Some((11, 12)),
                pending_trigger: Some("backfill".to_owned()),
                pending_reason: Some("stale".to_owned()),
                observed_at: "2026-01-01T00:00:01Z".parse().unwrap(),
            },
            &[],
            &[gap],
        )
        .await;
        assert!(matches!(
            result,
            Err(CollectionError::ConcurrentStateChange)
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM history_gaps")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0
        );
    }

    fn snapshot() -> RpcSnapshot {
        RpcSnapshot {
            client_version: "fake-platon/1.0".into(),
            namespaces: vec!["platon".into(), "net".into()],
            methods: vec!["platon_blockNumber".into()],
            network_identity: NetworkIdentity {
                genesis_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .parse()
                    .unwrap(),
                chain_id: 210425,
                p2p_network_id: 210425,
                address_hrp: Some("lat".into()),
            },
            node_key_fingerprint: "0xdddddddddddddddddddddddddddddddddddddddd"
                .parse()
                .unwrap(),
            sync: ProbeValue::Supported(SyncCurrent {
                syncing: false,
                current_block: 100,
                highest_block: 100,
                pulled_states: 10,
                known_states: 10,
            }),
            consensus: ProbeValue::Supported(ConsensusCurrent {
                epoch: 1,
                view_number: 2,
                validator: true,
                highest_qc_block: 99,
                highest_lock_block: 98,
                highest_commit_block: 97,
            }),
            peers: ProbeValue::Supported(PeerSnapshot { peers: vec![] }),
            enode: None,
        }
    }

    #[test]
    fn scripted_adapter_builds_valid_one_node_report() {
        let dir = tempdir().unwrap();
        let config = AgentConfig {
            config_path: dir.path().join("agent.toml"),
            server_url: "https://example.com".into(),
            credential_file: dir.path().join("credential"),
            state_db: dir.path().join("agent.db"),
            collection_interval_seconds: 5,
            backfill: crate::config::BackfillConfig::default(),
        };
        let inventory: NodeInventory = serde_json::from_str(r#"{"revision":1,"nodes":[{"node_id":"0195f2a1-0014-4014-8014-000000000014","network_key":"platon-mainnet","rpc_endpoint":"ws://127.0.0.1:6790"}]}"#).unwrap();
        let report = collect_report(
            &config,
            "0195f2a1-0011-4011-8011-000000000011".parse().unwrap(),
            1,
            "0195f2a1-0012-4012-8012-000000000012".parse().unwrap(),
            1,
            inventory,
            &ScriptedRpcAdapter::new(snapshot()),
        )
        .unwrap();
        assert_eq!(report.validate(), Ok(()));
        assert_eq!(
            report.nodes[0]
                .chain
                .rpc
                .latest
                .as_ref()
                .unwrap()
                .client_version,
            "fake-platon/1.0"
        );
        assert_eq!(
            report.host.cpu_percent.attempted_at,
            report.host.cpu_percent.latest_observed_at
        );
        assert_eq!(report.host.cpu_percent.state_revision, 1);
        assert_eq!(report.host.cpu_percent.value_revision, 1);
        assert_eq!(
            report.nodes[0].chain.rpc.attempted_at,
            report.nodes[0].chain.rpc.latest_observed_at
        );
        assert_eq!(report.nodes[0].chain.rpc.state_revision, 1);
        assert_eq!(report.nodes[0].chain.rpc.value_revision, 1);
        assert_eq!(
            report.nodes[0]
                .chain
                .rpc
                .latest
                .as_ref()
                .unwrap()
                .namespaces,
            ["platon", "net"]
        );
    }

    #[test]
    fn two_nodes_are_collected_independently_and_host_is_shared() {
        let dir = tempdir().unwrap();
        let config = AgentConfig {
            config_path: dir.path().join("agent.toml"),
            server_url: "https://example.com".into(),
            credential_file: dir.path().join("credential"),
            state_db: dir.path().join("agent.db"),
            collection_interval_seconds: 5,
            backfill: crate::config::BackfillConfig::default(),
        };
        let first_endpoint: RpcEndpoint = "ws://127.0.0.1:6790".parse().unwrap();
        let second_endpoint: RpcEndpoint = "ws://127.0.0.1:6791".parse().unwrap();
        let first_id = "0195f2a1-0014-4014-8014-000000000014";
        let second_id = "0195f2a1-0015-4015-8015-000000000015";
        let inventory: NodeInventory = serde_json::from_str(&format!(
            r#"{{"revision":1,"nodes":[{{"node_id":"{first_id}","network_key":"platon-mainnet","rpc_endpoint":"{}"}},{{"node_id":"{second_id}","network_key":"platon-testnet","rpc_endpoint":"{}"}}]}}"#,
            first_endpoint.as_str(), second_endpoint.as_str()
        ))
        .unwrap();
        let adapter = ScriptedRpcAdapter::for_nodes([
            (first_endpoint.clone(), snapshot()),
            (
                second_endpoint.clone(),
                RpcSnapshot {
                    client_version: "fake-platon/testnet".into(),
                    namespaces: vec!["platon".into()],
                    methods: vec!["platon_blockNumber".into()],
                    network_identity: snapshot().network_identity,
                    node_key_fingerprint: "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                        .parse()
                        .unwrap(),
                    enode: None,
                    sync: ProbeValue::Unsupported,
                    consensus: ProbeValue::Unsupported,
                    peers: ProbeValue::Unsupported,
                },
            ),
        ])
        .fail_endpoint(&first_endpoint);
        let report = collect_report(
            &config,
            "0195f2a1-0011-4011-8011-000000000011".parse().unwrap(),
            1,
            "0195f2a1-0012-4012-8012-000000000012".parse().unwrap(),
            1,
            inventory,
            &adapter,
        )
        .unwrap();
        assert_eq!(report.nodes.len(), 2);
        assert!(report.host.memory.latest.unwrap().total_bytes > 0);
        assert_eq!(report.nodes[0].node_id.to_string(), first_id);
        assert_eq!(report.nodes[0].chain.rpc.status, ComponentStatus::Error);
        assert_eq!(report.nodes[0].chain.rpc.latest, None);
        assert_eq!(report.nodes[1].node_id.to_string(), second_id);
        assert_eq!(report.nodes[1].chain.rpc.status, ComponentStatus::Ok);
        assert_eq!(
            report.nodes[0].chain.peers.as_ref().unwrap().status,
            ComponentStatus::Error
        );
        assert_eq!(
            report.nodes[1].chain.peers.as_ref().unwrap().status,
            ComponentStatus::Unsupported
        );
        assert_eq!(
            report.nodes[1]
                .chain
                .rpc
                .latest
                .as_ref()
                .unwrap()
                .client_version,
            "fake-platon/testnet"
        );
    }

    #[test]
    fn peer_outcomes_are_independent_and_non_empty_values_are_typed() {
        let dir = tempdir().unwrap();
        let config = AgentConfig {
            config_path: dir.path().join("agent.toml"),
            server_url: "https://example.com".into(),
            credential_file: dir.path().join("credential"),
            state_db: dir.path().join("agent.db"),
            collection_interval_seconds: 5,
            backfill: crate::config::BackfillConfig::default(),
        };
        let first_endpoint: RpcEndpoint = "ws://127.0.0.1:6790".parse().unwrap();
        let second_endpoint: RpcEndpoint = "ws://127.0.0.1:6791".parse().unwrap();
        let first_id = "0195f2a1-0014-4014-8014-000000000014";
        let second_id = "0195f2a1-0015-4015-8015-000000000015";
        let inventory: NodeInventory = serde_json::from_str(&format!(
            r#"{{"revision":1,"nodes":[{{"node_id":"{first_id}","network_key":"platon-mainnet","rpc_endpoint":"{}"}},{{"node_id":"{second_id}","network_key":"platon-testnet","rpc_endpoint":"{}"}}]}}"#,
            first_endpoint.as_str(),
            second_endpoint.as_str()
        ))
        .unwrap();
        let mut first = snapshot();
        first.peers = ProbeValue::Supported(PeerSnapshot {
            peers: vec![PeerCurrent {
                peer_id: "peer-a".to_owned(),
                remote_ip: Some("203.0.113.4".to_owned()),
                direction: PeerDirection::Inbound,
                trusted: true,
                static_peer: false,
                consensus_peer: true,
                client_name: Some("PlatON/v1.5.1".to_owned()),
                caps: vec!["cbft/1".to_owned()],
                cbft_protocol_version: Some(1),
                cbft_highest_qc_block: Some(100),
                cbft_locked_block: Some(99),
                cbft_commit_block: Some(98),
            }],
        });
        let mut second = snapshot();
        second.peers = ProbeValue::Error("admin_peers failed".to_owned());
        let adapter =
            ScriptedRpcAdapter::for_nodes([(first_endpoint, first), (second_endpoint, second)]);
        let report = collect_report(
            &config,
            "0195f2a1-0011-4011-8011-000000000011".parse().unwrap(),
            1,
            "0195f2a1-0012-4012-8012-000000000012".parse().unwrap(),
            1,
            inventory,
            &adapter,
        )
        .unwrap();
        assert_eq!(
            report.nodes[0].chain.peers.as_ref().unwrap().status,
            ComponentStatus::Ok
        );
        assert_eq!(
            report.nodes[0]
                .chain
                .peers
                .as_ref()
                .unwrap()
                .latest
                .as_ref()
                .unwrap()
                .peers[0]
                .peer_id,
            "peer-a"
        );
        assert_eq!(
            report.nodes[1].chain.peers.as_ref().unwrap().status,
            ComponentStatus::Error
        );
        assert!(
            report.nodes[1]
                .chain
                .peers
                .as_ref()
                .unwrap()
                .latest
                .is_none()
        );
    }

    #[test]
    fn node_error_does_not_overwrite_other_nodes() {
        let mut system = System::new_all();
        let node = collect_node(
            &mut system,
            &platpulse_core::inventory::InventoryNode {
                node_id: "0195f2a1-0014-4014-8014-000000000014".parse().unwrap(),
                display_name: None,
                network_key: "platon-mainnet".parse().unwrap(),
                rpc_endpoint: "ws://127.0.0.1:6790".parse().unwrap(),
                process: None,
            },
            None,
            None,
            timestamp(),
            &FailClosedRpcAdapter,
        );
        assert_eq!(node.chain.rpc.status, ComponentStatus::Error);
        assert!(node.chain.rpc.error.is_some());
        assert_eq!(node.chain.sync.status, ComponentStatus::Error);
    }

    #[test]
    fn failed_directory_scan_is_cached_for_the_full_sample_interval() {
        let mut previous: AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let last_good = "2026-08-20T00:00:00Z".parse().unwrap();
        let failed_at = "2026-08-20T00:05:00Z".parse().unwrap();
        previous.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Error,
            attempted_at: Some(failed_at),
            latest_observed_at: Some(last_good),
            received_at: None,
            state_revision: 2,
            value_revision: 1,
            latest: Some(42),
            error: Some(BoundedError {
                code: "data_directory_scan_failed".to_owned(),
                message: "PlatON data directory could not be measured".to_owned(),
            }),
        });
        previous.nodes[0].data_directory_capacity_bytes = Some(ComponentObservation {
            status: ComponentStatus::Error,
            attempted_at: Some(failed_at),
            latest_observed_at: Some(last_good),
            received_at: None,
            state_revision: 2,
            value_revision: 1,
            latest: Some(100),
            error: Some(BoundedError {
                code: "data_directory_capacity_failed".to_owned(),
                message: "PlatON data directory filesystem capacity could not be measured"
                    .to_owned(),
            }),
        });
        let mut paths = HashMap::new();
        paths.insert(
            previous.nodes[0].node_id,
            std::path::PathBuf::from("/definitely/missing/platon-data"),
        );

        let inventory = previous.inventory.clone();
        let observations = collect_directory_observations(
            &paths,
            &inventory,
            Some(&previous),
            "2026-08-20T00:05:01Z".parse().unwrap(),
        );

        assert_eq!(
            observations[&previous.nodes[0].node_id]
                .size_bytes
                .attempted_at,
            Some(failed_at)
        );
    }

    #[test]
    fn disabled_directory_advances_state_without_rewinding_value_revision() {
        let mut previous: AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        previous.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some("2026-08-20T00:00:00Z".parse().unwrap()),
            latest_observed_at: Some("2026-08-20T00:00:00Z".parse().unwrap()),
            received_at: None,
            state_revision: 7,
            value_revision: 9,
            latest: Some(42),
            error: None,
        });
        let inventory = previous.inventory.clone();

        let observations = collect_directory_observations(
            &HashMap::new(),
            &inventory,
            Some(&previous),
            "2026-08-20T00:05:00Z".parse().unwrap(),
        );
        let observation = &observations[&previous.nodes[0].node_id];

        assert_eq!(observation.size_bytes.status, ComponentStatus::Disabled);
        assert_eq!(observation.size_bytes.state_revision, 8);
        assert_eq!(observation.size_bytes.value_revision, 9);
        assert_eq!(observation.size_bytes.latest, None);
    }

    #[test]
    fn precollected_directory_cache_preserves_restart_value_and_advances_revision() {
        let mut previous: AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let previous_observation = ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some("2026-08-20T00:00:00Z".parse().unwrap()),
            latest_observed_at: Some("2026-08-20T00:00:00Z".parse().unwrap()),
            received_at: None,
            state_revision: 4,
            value_revision: 6,
            latest: Some(42),
            error: None,
        };
        previous.nodes[0].data_directory_size_bytes = Some(previous_observation.clone());
        previous.nodes[0].data_directory_capacity_bytes =
            Some(ok(100_u64, "2026-08-20T00:00:00Z".parse().unwrap()));
        let node_id = previous.nodes[0].node_id;

        let starting = reconcile_precollected_directory_observations(
            HashMap::from([(
                node_id,
                crate::data_directory::DataDirectoryObservations {
                    size_bytes: crate::data_directory::starting(),
                    capacity_bytes: crate::data_directory::starting(),
                },
            )]),
            &previous.inventory,
            Some(&previous),
        );
        assert_eq!(starting[&node_id].size_bytes, previous_observation);

        let changed = reconcile_precollected_directory_observations(
            HashMap::from([(
                node_id,
                crate::data_directory::DataDirectoryObservations {
                    size_bytes: ok(43_u64, "2026-08-20T00:05:00Z".parse().unwrap()),
                    capacity_bytes: ok(101_u64, "2026-08-20T00:05:00Z".parse().unwrap()),
                },
            )]),
            &previous.inventory,
            Some(&previous),
        );
        assert_eq!(changed[&node_id].size_bytes.state_revision, 4);
        assert_eq!(changed[&node_id].size_bytes.value_revision, 7);
        assert_eq!(changed[&node_id].size_bytes.latest, Some(43));
    }

    #[test]
    fn closing_report_reuses_configured_directory_observation() {
        let mut previous: AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let observation = ok(42_u64, "2026-08-20T00:00:00Z".parse().unwrap());
        previous.nodes[0].data_directory_size_bytes = Some(observation.clone());
        previous.nodes[0].data_directory_capacity_bytes =
            Some(ok(100_u64, "2026-08-20T00:00:00Z".parse().unwrap()));
        let mut paths = HashMap::new();
        paths.insert(
            previous.nodes[0].node_id,
            std::path::PathBuf::from("/configured/platon-data"),
        );

        let observations =
            closing_directory_observations(&paths, &previous.inventory, Some(&previous));

        assert_eq!(
            observations[&previous.nodes[0].node_id].size_bytes,
            observation
        );
    }

    #[test]
    fn multiple_nodes_keep_distinct_directory_measurements() {
        let temp = tempdir().unwrap();
        let first_path = temp.path().join("first");
        let second_path = temp.path().join("second");
        std::fs::create_dir_all(&first_path).unwrap();
        std::fs::create_dir_all(&second_path).unwrap();
        std::fs::write(first_path.join("data"), vec![0_u8; 3]).unwrap();
        std::fs::write(second_path.join("data"), vec![0_u8; 7]).unwrap();
        let previous: AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let mut inventory = previous.inventory.clone();
        let second_id: NodeId = "0195f2a1-0099-4099-8099-000000000099".parse().unwrap();
        let mut second_node = inventory.nodes[0].clone();
        second_node.node_id = second_id;
        inventory.nodes.push(second_node);
        let mut paths = HashMap::new();
        paths.insert(inventory.nodes[0].node_id, first_path);
        paths.insert(second_id, second_path);

        let observations = collect_directory_observations(
            &paths,
            &inventory,
            None,
            "2026-08-20T00:00:00Z".parse().unwrap(),
        );

        assert_eq!(
            observations[&inventory.nodes[0].node_id].size_bytes.latest,
            Some(3)
        );
        assert_eq!(observations[&second_id].size_bytes.latest, Some(7));
    }

    #[test]
    fn failed_component_keeps_last_good_value_and_current_error() {
        let at = "2026-08-20T00:00:00Z".parse().unwrap();
        let previous = ok(42_u64, at);
        let mut current = error(at, "rpc_unreachable", "RPC probe failed");

        preserve_last_good(&mut current, &previous);

        assert_eq!(current.status, ComponentStatus::Error);
        assert_eq!(current.error.as_ref().unwrap().code, "rpc_unreachable");
        assert_eq!(current.latest, Some(42));
        assert_eq!(current.latest_observed_at, Some(at));
        assert_eq!(current.value_revision, 1);
    }

    #[test]
    fn authoritative_empty_success_is_not_replaced_by_last_good() {
        let at = "2026-08-20T00:00:00Z".parse().unwrap();
        let previous = ok(vec![1_u64], at);
        let mut current = ok(Vec::<u64>::new(), at);

        preserve_last_good(&mut current, &previous);

        assert_eq!(current.latest, Some(Vec::<u64>::new()));
    }

    #[test]
    fn fail_closed_adapter_never_fabricates_rpc_data() {
        let endpoint: RpcEndpoint = "ws://127.0.0.1:6790".parse().unwrap();
        let error = FailClosedRpcAdapter.collect(&endpoint).unwrap_err();
        assert!(error.to_string().contains("not configured"));
    }

    #[tokio::test]
    async fn collect_and_persist_advances_state_with_report_atomically() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("agent.toml");
        let db_path = dir.path().join("agent.db");
        std::fs::write(
            &config_path,
            format!(
                "server_url=\"https://example.com\"\ncredential_file=\"{}/credential\"\nstate_db=\"{}\"\ninventory_revision=1\nnodes=[{{node_id=\"0195f2a1-0014-4014-8014-000000000014\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:6790\"}}]\n",
                dir.path().display(),
                db_path.display()
            ),
        )
        .unwrap();
        let config = AgentConfig::resolve(&config_path).unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(&db_path))
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_state (singleton, agent_id, agent_epoch, report_sequence, inventory_revision, updated_at) VALUES (1, ?, 3, 4, 1, ?)")
            .bind("0195f2a1-0011-4011-8011-000000000011")
            .bind("2026-08-12T08:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, block_interval_ms, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .bind(9_i64)
            .bind(format!("0x{}", "00".repeat(32)))
            .bind(format!("0x{}", "11".repeat(32)))
            .bind(format!("0x{}", "22".repeat(32)))
            .bind(1_i64)
            .bind(1_i64)
            .bind("lat")
            .bind(0_i64)
            .bind("2026-08-12T08:00:00Z")
            .bind(0_i64)
            .bind(None::<i64>)
            .bind("subscription")
            .bind(format!("0x{}", "00".repeat(20)))
            .bind("unknown")
            .bind("unknown")
            .bind("test sample")
            .bind("2026-08-12T08:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        store.close().await.unwrap();

        collect_and_persist(&config, &ScriptedRpcAdapter::new(snapshot()))
            .await
            .unwrap();
        let endpoint: RpcEndpoint = "ws://127.0.0.1:6790".parse().unwrap();
        collect_and_persist(
            &config,
            &ScriptedRpcAdapter::new(snapshot()).fail_endpoint(&endpoint),
        )
        .await
        .unwrap();
        let mut reopened = AgentStore::open(AgentDatabaseConfig::new(&db_path))
            .await
            .unwrap();
        let state: (i64, i64) = sqlx::query_as(
            "SELECT agent_epoch, report_sequence FROM agent_state WHERE singleton=1",
        )
        .fetch_one(reopened.connection())
        .await
        .unwrap();
        assert_eq!(state, (3, 6));
        let reports: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reports")
            .fetch_one(reopened.connection())
            .await
            .unwrap();
        assert_eq!(reports, 2);
        let first_body: Vec<u8> =
            sqlx::query_scalar("SELECT body FROM reports ORDER BY report_sequence ASC LIMIT 1")
                .fetch_one(reopened.connection())
                .await
                .unwrap();
        let first_report: AgentReport = serde_json::from_slice(&first_body).unwrap();
        assert_eq!(first_report.block_summaries.len(), 1);
        let second_body: Vec<u8> =
            sqlx::query_scalar("SELECT body FROM reports ORDER BY report_sequence DESC LIMIT 1")
                .fetch_one(reopened.connection())
                .await
                .unwrap();
        let report: AgentReport = serde_json::from_slice(&second_body).unwrap();
        assert!(report.block_summaries.is_empty());
        let node = &report.nodes[0];
        assert_eq!(node.chain.rpc.status, ComponentStatus::Error);
        assert_eq!(
            node.chain.rpc.latest.as_ref().unwrap().client_version,
            "fake-platon/1.0"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM report_sample_assignments")
                .fetch_one(reopened.connection())
                .await
                .unwrap(),
            1
        );
        reopened.close().await.unwrap();
    }

    #[tokio::test]
    async fn drained_previous_transition_is_persisted_until_first_new_boot_report() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("agent.toml");
        let db_path = dir.path().join("agent.db");
        std::fs::write(
            &config_path,
            format!(
                "server_url=\"https://example.com\"\ncredential_file=\"{}/credential\"\nstate_db=\"{}\"\ninventory_revision=1\nnodes=[{{node_id=\"0195f2a1-0014-4014-8014-000000000014\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:6790\"}}]\n",
                dir.path().display(),
                db_path.display()
            ),
        )
        .unwrap();
        let config = AgentConfig::resolve(&config_path).unwrap();
        let old_boot = "0195f2a1-0040-4040-8040-000000000040";
        let new_boot = "0195f2a1-0041-4041-8041-000000000041";
        let mut store = AgentStore::open(AgentDatabaseConfig::new(&db_path))
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, boot_state, pending_transition, pending_previous_boot_id, shutdown_state, shutdown_started_at, updated_at) VALUES (1, ?, 1, ?, 0, 1, 'drained_pending', 'drained_previous', ?, 'draining', ?, ?)")
            .bind("0195f2a1-0011-4011-8011-000000000011")
            .bind(new_boot)
            .bind(old_boot)
            .bind("2026-08-12T07:59:00Z")
            .bind("2026-08-12T08:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        store.close().await.unwrap();

        let digest = collect_and_persist(&config, &ScriptedRpcAdapter::new(snapshot()))
            .await
            .unwrap();
        let mut reopened = AgentStore::open(AgentDatabaseConfig::new(&db_path))
            .await
            .unwrap();
        let body: Vec<u8> = sqlx::query_scalar("SELECT body FROM reports WHERE body_sha256=?")
            .bind(&digest)
            .fetch_one(reopened.connection())
            .await
            .unwrap();
        let report: AgentReport = serde_json::from_slice(&body).unwrap();
        assert_eq!(report.boot_transition, BootTransition::DrainedPrevious);
        assert_eq!(report.previous_boot_id.unwrap().to_string(), old_boot);
        let spool = report.host.spool.latest.unwrap();
        assert_eq!(spool.shutdown_state.as_deref(), Some("running"));
        assert_eq!(spool.shutdown_started_at, None);
        let state: (String, Option<String>, String, String, Option<String>) = sqlx::query_as(
            "SELECT boot_id, pending_transition, boot_state, shutdown_state, shutdown_started_at FROM agent_state WHERE singleton=1",
        )
        .fetch_one(reopened.connection())
        .await
        .unwrap();
        assert_eq!(
            state,
            (
                new_boot.to_owned(),
                None,
                "active".to_owned(),
                "running".to_owned(),
                None,
            )
        );
        reopened.close().await.unwrap();
    }
    #[tokio::test]
    async fn receipt_application_expires_only_one_bounded_batch_of_old_markers() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let marker_hash = "0x0000000000000000000000000000000000000000000000000000000000000000";
        for _ in 0..129 {
            sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, 'accepted', ?)")
                .bind(Uuid::new_v4().to_string())
                .bind(marker_hash)
                .bind("2025-12-31T23:59:59Z")
                .execute(store.connection())
                .await
                .unwrap();
        }
        for applied_at in ["2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z"] {
            sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, 'accepted', ?)")
                .bind(Uuid::new_v4().to_string())
                .bind(marker_hash)
                .bind(applied_at)
                .execute(store.connection())
                .await
                .unwrap();
        }

        let report_id = "0195f2a1-0013-4013-8013-000000000013";
        let body = include_bytes!("../../platpulse-core/tests/fixtures/report_v1_minimal.json");
        let hash = format!("0x{}", hex::encode(sha2::Sha256::digest(body)));
        let receipt_body = format!(
            r#"{{"receipt":{{"report_id":"{report_id}","disposition":"rejected","report_body_sha256":"{hash}","server_version":"test","supported_protocol_majors":[1],"server_time":"2026-01-01T00:00:00Z","inventory":"rejected","rejections":[{{"code":"invalid_envelope","retryable":false,"reason":"test"}}],"nodes":[],"samples":[]}}}}"#
        );
        sqlx::query("INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, created_at) VALUES (?,1,?,1,?, ?, ?, ?, ?)")
            .bind(report_id)
            .bind("0195f2a1-0012-4012-8012-000000000012")
            .bind("2026-08-12T09:00:00Z")
            .bind(&body[..])
            .bind(&hash)
            .bind(body.len() as i64)
            .bind("2026-08-12T09:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        apply_receipt(
            &mut store,
            report_id,
            &hash,
            "rejected",
            receipt_body.as_bytes(),
            "2026-01-02T00:00:00Z",
        )
        .await
        .unwrap();

        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM report_receipts")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            68
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM report_receipts WHERE applied_at < '2026-01-01T00:00:00Z'",
            )
            .fetch_one(store.connection())
            .await
            .unwrap(),
            65
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM report_receipts WHERE report_id = ?",
            )
            .bind(report_id)
            .fetch_one(store.connection())
            .await
            .unwrap(),
            1
        );
        store.close().await.unwrap();
    }

    #[tokio::test]
    async fn receipt_application_keeps_a_marker_at_the_exact_retention_boundary() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let boundary_id = Uuid::new_v4().to_string();
        let marker_hash = "0x0000000000000000000000000000000000000000000000000000000000000000";
        sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, 'accepted', ?)")
            .bind(&boundary_id)
            .bind(marker_hash)
            .bind("2026-01-01T00:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();

        let report_id = "0195f2a1-0013-4013-8013-000000000013";
        let body = include_bytes!("../../platpulse-core/tests/fixtures/report_v1_minimal.json");
        let hash = format!("0x{}", hex::encode(sha2::Sha256::digest(body)));
        let receipt_body = format!(
            r#"{{"receipt":{{"report_id":"{report_id}","disposition":"rejected","report_body_sha256":"{hash}","server_version":"test","supported_protocol_majors":[1],"server_time":"2026-01-01T00:00:00Z","inventory":"rejected","rejections":[{{"code":"invalid_envelope","retryable":false,"reason":"test"}}],"nodes":[],"samples":[]}}}}"#
        );
        sqlx::query("INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, created_at) VALUES (?, 1, ?, 1, ?, ?, ?, ?, ?)")
            .bind(report_id)
            .bind("0195f2a1-0012-4012-8012-000000000012")
            .bind("2026-08-12T09:00:00Z")
            .bind(&body[..])
            .bind(&hash)
            .bind(body.len() as i64)
            .bind("2026-08-12T09:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        apply_receipt(
            &mut store,
            report_id,
            &hash,
            "rejected",
            receipt_body.as_bytes(),
            "2026-01-02T00:00:00Z",
        )
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM report_receipts WHERE report_id = ?"
            )
            .bind(boundary_id)
            .fetch_one(store.connection())
            .await
            .unwrap(),
            1
        );
        store.close().await.unwrap();
    }

    #[tokio::test]
    async fn receipt_application_is_atomic_and_removes_report() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let report_id = "0195f2a1-0013-4013-8013-000000000013";
        let body = include_bytes!("../../platpulse-core/tests/fixtures/report_v1_minimal.json");
        let hash = format!("0x{}", hex::encode(sha2::Sha256::digest(body)));
        let receipt_body = format!(
            r#"{{"receipt":{{"report_id":"{report_id}","disposition":"rejected","report_body_sha256":"{hash}","server_version":"test","supported_protocol_majors":[1],"server_time":"2026-01-01T00:00:00Z","inventory":"rejected","rejections":[{{"code":"invalid_envelope","retryable":false,"reason":"test"}}],"nodes":[],"samples":[]}}}}"#
        );
        let applied_at = crate::database::now_rfc3339();
        sqlx::query("INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, created_at) VALUES (?,1,?,1,?, ?, ?, ?, ?)")
            .bind(report_id)
            .bind("0195f2a1-0012-4012-8012-000000000012")
            .bind("2026-08-12T09:00:00Z")
            .bind(&body[..])
            .bind(&hash)
            .bind(body.len() as i64)
            .bind("2026-08-12T09:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        sqlx::query(
            "CREATE TRIGGER abort_receipt_marker BEFORE INSERT ON report_receipts BEGIN SELECT RAISE(ABORT, 'injected receipt marker failure'); END",
        )
        .execute(store.connection())
        .await
        .unwrap();
        assert!(
            apply_receipt(
                &mut store,
                report_id,
                &hash,
                "rejected",
                receipt_body.as_bytes(),
                &applied_at,
            )
            .await
            .is_err()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id = ?")
                .bind(report_id)
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM report_receipts WHERE report_id = ?"
            )
            .bind(report_id)
            .fetch_one(store.connection())
            .await
            .unwrap(),
            0
        );
        sqlx::query("DROP TRIGGER abort_receipt_marker")
            .execute(store.connection())
            .await
            .unwrap();
        apply_receipt(
            &mut store,
            report_id,
            &hash,
            "rejected",
            receipt_body.as_bytes(),
            &applied_at,
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reports WHERE report_id=?")
            .bind(report_id)
            .fetch_one(store.connection())
            .await
            .unwrap();
        assert_eq!(count, 0);
        let marker: (String, String, String) = sqlx::query_as(
            "SELECT report_body_sha256, disposition, applied_at FROM report_receipts WHERE report_id = ?",
        )
        .bind(report_id)
        .fetch_one(store.connection())
        .await
        .unwrap();
        assert_eq!(
            marker,
            (hash.clone(), "rejected".to_owned(), applied_at.clone())
        );
        store.close().await.unwrap();

        let mut reopened = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id = ?")
                .bind(report_id)
                .fetch_one(reopened.connection())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM report_receipts WHERE report_id = ? AND report_body_sha256 = ? AND disposition = 'rejected'",
            )
            .bind(report_id)
            .bind(hash)
            .fetch_one(reopened.connection())
            .await
            .unwrap(),
            1
        );
        reopened.close().await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_and_conflicting_receipts_preserve_the_queued_report() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let report_id = "0195f2a1-0013-4013-8013-000000000013";
        let body = include_bytes!("../../platpulse-core/tests/fixtures/report_v1_minimal.json");
        let hash = format!("0x{}", hex::encode(sha2::Sha256::digest(body)));
        let receipt_body = format!(
            r#"{{"receipt":{{"report_id":"{report_id}","disposition":"rejected","report_body_sha256":"{hash}","server_version":"test","supported_protocol_majors":[1],"server_time":"2026-01-01T00:00:00Z","inventory":"rejected","rejections":[{{"code":"invalid_envelope","retryable":false,"reason":"test"}}],"nodes":[],"samples":[]}}}}"#
        );
        sqlx::query("INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, created_at) VALUES (?, 1, ?, 1, ?, ?, ?, ?, ?)")
            .bind(report_id)
            .bind("0195f2a1-0012-4012-8012-000000000012")
            .bind("2026-08-12T09:00:00Z")
            .bind(&body[..])
            .bind(&hash)
            .bind(body.len() as i64)
            .bind("2026-08-12T09:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, 'accepted', ?)")
            .bind(report_id)
            .bind(&hash)
            .bind("2026-01-01T00:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();

        let conflict = apply_receipt(
            &mut store,
            report_id,
            &hash,
            "rejected",
            receipt_body.as_bytes(),
            "2026-01-02T00:00:00Z",
        )
        .await
        .unwrap_err();
        assert!(conflict.to_string().contains("conflicts"));

        let conflicting_hash = "0x0000000000000000000000000000000000000000000000000000000000000000";
        let hash_conflict = apply_receipt(
            &mut store,
            report_id,
            conflicting_hash,
            "rejected",
            receipt_body.replace(&hash, conflicting_hash).as_bytes(),
            "2026-01-02T00:00:00Z",
        )
        .await
        .unwrap_err();
        assert!(hash_conflict.to_string().contains("conflicts"));

        sqlx::query("UPDATE report_receipts SET disposition = 'rejected' WHERE report_id = ?")
            .bind(report_id)
            .execute(store.connection())
            .await
            .unwrap();
        let duplicate = apply_receipt(
            &mut store,
            report_id,
            &hash,
            "rejected",
            receipt_body.as_bytes(),
            "2026-01-02T00:00:00Z",
        )
        .await
        .unwrap_err();
        assert!(duplicate.to_string().contains("duplicate"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id = ?")
                .bind(report_id)
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT store_fatal FROM spool_state WHERE singleton = 1")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1
        );
        store.close().await.unwrap();

        let mut reopened = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT store_fatal FROM spool_state WHERE singleton = 1")
                .fetch_one(reopened.connection())
                .await
                .unwrap(),
            1
        );
        assert!(matches!(
            crate::reporting::ensure_spool_healthy(&mut reopened).await,
            Err(ReportStoreError::StoreFatal(_))
        ));
        reopened.close().await.unwrap();
    }

    #[tokio::test]
    async fn receipt_application_keeps_an_unclaimed_block_fork() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let report_id = "0195f2a1-0013-4013-8013-000000000013";
        let node_id = report.inventory.nodes[0].node_id;
        report.report_id = report_id.parse().unwrap();
        report.report_sequence = 1;
        let mut sample = serde_json::from_slice::<AgentReport>(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_canonical.json"
        ))
        .unwrap()
        .block_summaries[0]
            .clone();
        sample.node_id = node_id;
        sample.block_number = 9;
        sample.block_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa9"
            .parse()
            .unwrap();
        sample.parent_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa8"
            .parse()
            .unwrap();
        sample.observed_at = report.generated_at;
        report.block_summaries.push(sample.clone());
        report.validate().unwrap();
        let body = serde_json::to_vec(&report).unwrap();
        let body_hash = format!("0x{}", hex::encode(sha2::Sha256::digest(&body)));
        let created_at = report.generated_at.to_string();
        crate::block::persist_block_summary(&mut store, &sample, &created_at)
            .await
            .unwrap();
        let mut fork = sample.clone();
        fork.block_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab"
            .parse()
            .unwrap();
        crate::block::persist_block_summary(&mut store, &fork, &created_at)
            .await
            .unwrap();
        crate::reporting::persist_immutable_report(
            &mut store,
            report_id,
            report.agent_epoch,
            &report.boot_id.to_string(),
            report.report_sequence,
            &created_at,
            &body,
        )
        .await
        .unwrap();
        let receipt_body = serde_json::json!({
            "receipt": {
                "report_id": report_id,
                "disposition": "accepted",
                "report_body_sha256": body_hash,
                "server_version": "test",
                "supported_protocol_majors": [1],
                "server_time": created_at,
                "inventory": "accepted",
                "rejections": [],
                "nodes": [{
                    "node_id": node_id,
                    "current": "accepted",
                    "accepted_component_revisions": [],
                    "rejections": []
                }],
                "samples": [{
                    "node_id": node_id,
                    "sample": {"kind": "block", "height": 9},
                    "disposition": "accepted"
                }]
            }
        })
        .to_string();
        sqlx::query(
            "CREATE TRIGGER abort_accepted_marker BEFORE INSERT ON report_receipts BEGIN SELECT RAISE(ABORT, 'injected receipt marker failure'); END",
        )
        .execute(store.connection())
        .await
        .unwrap();
        assert!(
            apply_receipt(
                &mut store,
                report_id,
                &body_hash,
                "accepted",
                receipt_body.as_bytes(),
                &created_at,
            )
            .await
            .is_err()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM block_summaries WHERE node_id=? AND block_number=? AND block_hash=?",
            )
            .bind(node_id.to_string())
            .bind(9_i64)
            .bind(sample.block_hash.to_string())
            .fetch_one(store.connection())
            .await
            .unwrap(),
            1
        );
        sqlx::query("DROP TRIGGER abort_accepted_marker")
            .execute(store.connection())
            .await
            .unwrap();
        apply_receipt(
            &mut store,
            report_id,
            &body_hash,
            "accepted",
            receipt_body.as_bytes(),
            &created_at,
        )
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM block_summaries WHERE node_id=? AND block_number=? AND block_hash=?",
            )
            .bind(node_id.to_string())
            .bind(9_i64)
            .bind(fork.block_hash.to_string())
            .fetch_one(store.connection())
            .await
            .unwrap(),
            1
        );
        store.close().await.unwrap();
    }
}
