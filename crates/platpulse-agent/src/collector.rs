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
use platpulse_core::identity::{AgentId, BootId, ReportId};
use platpulse_core::inventory::NodeInventory;
use platpulse_core::network::{NetworkIdentity, RpcEndpoint};
use platpulse_core::observation::{
    ConsensusCurrent, DiskCurrent, HostObservation, LoadCurrent, MemoryCurrent, NetworkThroughput,
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
use sysinfo::{Disks, System};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::block::{NodeSubscriptions, load_block_summaries};
use crate::config::AgentConfig;
use crate::database::{AgentDatabaseConfig, AgentStore};
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
    #[error("invalid persisted Agent identity: {0}")]
    Identity(String),
    #[error("RPC collection failed: {0}")]
    Rpc(#[from] RpcCollectError),
    #[error("report persistence failed: {0}")]
    Report(#[from] ReportStoreError),
    #[error("report serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
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
/// intentionally bounded; empty disk and zero throughput are authoritative
/// snapshots from this minimal collector, not duplicated per Node.
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
        network_throughput: ok(
            NetworkThroughput {
                rx_bytes_per_sec: 0,
                tx_bytes_per_sec: 0,
            },
            at,
        ),
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
        .map(|node| collect_node(&mut system, node, attempted, adapter))
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
    let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db)).await?;
    crate::reporting::ensure_spool_healthy(&mut store).await?;
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
    sqlx::query("UPDATE agent_state SET boot_state='draining', updated_at=? WHERE singleton=1")
        .bind(timestamp().to_string())
        .execute(store.connection())
        .await?;
    let transport = crate::reporting::HttpReportTransport::from_config(config)?;
    while crate::reporting::deliver_one(&mut store, &transport)
        .await?
        .is_some()
    {}

    let agent_id = AgentId::from_str(&agent_text)
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let boot_id = BootId::from_str(&boot_text)
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let inventory = config
        .validated_inventory()
        .map_err(|error| CollectionError::Identity(error.to_string()))?
        .inventory;
    let at = timestamp();
    let mut closing = collect_report_with_clock_skew(
        config,
        agent_id,
        epoch as u64,
        boot_id,
        sequence as u64 + 1,
        inventory,
        adapter,
        clock_skew_error(at, "Server time exchange was unavailable during recovery"),
    )?;
    closing.boot_transition = BootTransition::Closing;
    closing.previous_boot_id = None;
    closing.block_summaries.clear();
    closing.history_gaps.clear();
    closing
        .validate()
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let body = serde_json::to_vec(&closing)?;
    crate::reporting::persist_immutable_report(
        &mut store,
        &closing.report_id.to_string(),
        closing.agent_epoch,
        &closing.boot_id.to_string(),
        closing.report_sequence,
        &closing.generated_at.to_string(),
        &body,
    )
    .await?;
    crate::reporting::deliver_one(&mut store, &transport)
        .await?
        .ok_or(CollectionError::RecoveryRequired)?;
    let new_boot_id = BootId::from_str(&Uuid::new_v4().to_string()).expect("UUID is valid");
    sqlx::query("UPDATE agent_state SET boot_id=?, report_sequence=0, boot_state='drained_pending', pending_transition='drained_previous', pending_previous_boot_id=?, previous_boot_id=?, close_report_id=NULL, close_applied_at=?, updated_at=? WHERE singleton=1")
        .bind(new_boot_id.to_string())
        .bind(boot_text.clone())
        .bind(boot_text)
        .bind(timestamp().to_string())
        .bind(timestamp().to_string())
        .execute(store.connection())
        .await?;
    store.close().await?;
    Ok(())
}
/// Collect and persist one complete immutable report. Agent state (identity,
/// boot and sequence) is advanced in the same transaction as the report body.
pub async fn collect_and_persist<A: RpcAdapter>(
    config: &AgentConfig,
    adapter: &A,
) -> Result<String, CollectionError> {
    let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db)).await?;
    crate::reporting::ensure_spool_healthy(&mut store).await?;
    #[allow(clippy::type_complexity)]
    let state: Option<(String, i64, Option<String>, i64, String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT agent_id, agent_epoch, boot_id, report_sequence, boot_state, previous_boot_id, pending_transition, pending_previous_boot_id FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(store.connection())
    .await?;
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
    let mut report = collect_report_with_clock_skew(
        config,
        agent_id,
        epoch as u64,
        boot_id,
        previous_sequence as u64 + 1,
        validated.inventory,
        adapter,
        clock_skew,
    )?;
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
    report.host.spool = ok(
        current_spool_diagnostics(&mut store).await?,
        report.generated_at,
    );
    report
        .validate()
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let body = serde_json::to_vec(&report)?;
    let digest = format!("0x{}", hex::encode(sha2::Sha256::digest(&body)));
    let now = report.generated_at.to_string();
    let mut tx = store.connection().begin().await?;
    sqlx::query("INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, in_flight, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?)")
        .bind(report.report_id.to_string()).bind(report.agent_epoch as i64).bind(report.boot_id.to_string()).bind(report.report_sequence as i64).bind(report.generated_at.to_string()).bind(&body).bind(&digest).bind(body.len() as i64).bind(&now).execute(&mut *tx).await?;
    let transition = match report.boot_transition {
        BootTransition::Continuing => "continuing",
        BootTransition::Closing => "closing",
        BootTransition::DrainedPrevious => "drained_previous",
        BootTransition::RecoveredAfterStale => "recovered_after_stale",
    };
    sqlx::query("UPDATE agent_state SET agent_id=?, agent_epoch=?, boot_id=?, report_sequence=?, inventory_revision=?, boot_state=CASE WHEN ?='drained_previous' THEN 'active' ELSE boot_state END, pending_transition=CASE WHEN ?='drained_previous' THEN NULL ELSE pending_transition END, pending_previous_boot_id=CASE WHEN ?='drained_previous' THEN NULL ELSE pending_previous_boot_id END, updated_at=? WHERE singleton=1")
        .bind(report.agent_id.to_string()).bind(report.agent_epoch as i64).bind(report.boot_id.to_string()).bind(report.report_sequence as i64).bind(report.inventory.revision as i64).bind(transition).bind(transition).bind(transition).bind(&now).execute(&mut *tx).await?;
    tx.commit().await?;
    crate::reporting::enforce_spool_policy(
        &mut store,
        &crate::collector::SpoolPolicy::default(),
        &now,
    )
    .await
    .map_err(CollectionError::Report)?;
    store.close().await?;
    Ok(digest)
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
    let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db)).await?;
    crate::reporting::ensure_spool_healthy(&mut store).await?;
    #[allow(clippy::type_complexity)]
    let state: Option<(String, i64, Option<String>, i64, String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT agent_id, agent_epoch, boot_id, report_sequence, boot_state, previous_boot_id, pending_transition, pending_previous_boot_id FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(store.connection())
    .await?;
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
    let inventory = validated.inventory;
    let mut report = collect_report_with_clock_skew(
        config,
        agent_id,
        epoch as u64,
        boot_id,
        previous_sequence as u64 + 1,
        inventory.clone(),
        adapter,
        clock_skew,
    )?;
    for node in &inventory.nodes {
        let Some(identity) = report
            .nodes
            .iter()
            .find(|item| item.node_id == node.node_id)
            .and_then(|item| item.chain.network_identity.latest.clone())
        else {
            continue;
        };
        let prior_recovery = sqlx::query_as::<_, (Option<String>, Option<i64>, Option<i64>, Option<i64>, Option<String>)>(
            "SELECT boot_id, last_head, pending_from, pending_to, pending_trigger FROM node_recovery_state WHERE node_id=?",
        )
        .bind(node.node_id.to_string())
        .fetch_optional(store.connection())
        .await?;
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
            Err(_) => {
                reconnect_needed = true;
            }
        }
        if let Ok(head) = transport.current_head_async(&node.rpc_endpoint).await {
            let boot_changed = prior_recovery.as_ref().and_then(|row| row.0.as_deref())
                != Some(report.boot_id.to_string().as_str());
            let plan = crate::block::plan_recovery(
                prior_recovery
                    .as_ref()
                    .and_then(|row| row.1)
                    .map(|v| v as u64),
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
                        report.generated_at,
                        plan.from_height,
                        plan.to_height,
                        bounds,
                        plan.trigger,
                    )
                    .await;
                for summary in outcome.summaries {
                    crate::block::persist_block_summary(
                        &mut store,
                        &summary,
                        &report.generated_at.to_string(),
                    )
                    .await?;
                }
                for gap in &outcome.gaps {
                    crate::block::persist_history_gap(&mut store, gap).await?;
                }
                let pending = outcome
                    .gaps
                    .first()
                    .map(|gap| (gap.from_height, gap.to_height));
                sqlx::query("INSERT INTO node_recovery_state (node_id, boot_id, last_head, pending_from, pending_to, pending_trigger, pending_reason, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET boot_id=excluded.boot_id,last_head=excluded.last_head,pending_from=excluded.pending_from,pending_to=excluded.pending_to,pending_trigger=excluded.pending_trigger,pending_reason=excluded.pending_reason,updated_at=excluded.updated_at")
                    .bind(node.node_id.to_string()).bind(report.boot_id.to_string()).bind(head as i64)
                    .bind(pending.map(|v| v.0 as i64)).bind(pending.map(|v| v.1 as i64))
                    .bind(pending.as_ref().map(|_| format!("{:?}", plan.trigger).to_lowercase()))
                    .bind(outcome.gaps.first().map(|gap| gap.reason.as_str())).bind(report.generated_at.to_string())
                    .execute(store.connection()).await?;
            } else {
                sqlx::query("INSERT INTO node_recovery_state (node_id, boot_id, last_head, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET boot_id=excluded.boot_id,last_head=excluded.last_head,updated_at=excluded.updated_at")
                    .bind(node.node_id.to_string()).bind(report.boot_id.to_string()).bind(head as i64).bind(report.generated_at.to_string())
                    .execute(store.connection()).await?;
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
    report.host.spool = ok(
        current_spool_diagnostics(&mut store).await?,
        report.generated_at,
    );
    report
        .validate()
        .map_err(|error| CollectionError::Identity(error.to_string()))?;
    let body = serde_json::to_vec(&report)?;
    let digest = format!("0x{}", hex::encode(sha2::Sha256::digest(&body)));
    let now = report.generated_at.to_string();
    let mut tx = store.connection().begin().await?;
    sqlx::query("INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, in_flight, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?)")
        .bind(report.report_id.to_string()).bind(report.agent_epoch as i64).bind(report.boot_id.to_string()).bind(report.report_sequence as i64).bind(report.generated_at.to_string()).bind(&body).bind(&digest).bind(body.len() as i64).bind(&now).execute(&mut *tx).await?;
    let transition = match report.boot_transition {
        BootTransition::Continuing => "continuing",
        BootTransition::Closing => "closing",
        BootTransition::DrainedPrevious => "drained_previous",
        BootTransition::RecoveredAfterStale => "recovered_after_stale",
    };
    sqlx::query("UPDATE agent_state SET agent_id=?, agent_epoch=?, boot_id=?, report_sequence=?, inventory_revision=?, boot_state=CASE WHEN ?='drained_previous' THEN 'active' ELSE boot_state END, pending_transition=CASE WHEN ?='drained_previous' THEN NULL ELSE pending_transition END, pending_previous_boot_id=CASE WHEN ?='drained_previous' THEN NULL ELSE pending_previous_boot_id END, updated_at=? WHERE singleton=1")
        .bind(report.agent_id.to_string()).bind(report.agent_epoch as i64).bind(report.boot_id.to_string()).bind(report.report_sequence as i64).bind(report.inventory.revision as i64).bind(transition).bind(transition).bind(transition).bind(&now).execute(&mut *tx).await?;
    tx.commit().await?;
    crate::reporting::enforce_spool_policy(&mut store, &SpoolPolicy::default(), &now)
        .await
        .map_err(CollectionError::Report)?;
    store.close().await?;
    Ok(digest)
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
    let envelope: serde_json::Value = serde_json::from_slice(receipt_body)
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    let receipt: ReportReceipt = serde_json::from_value(
        envelope
            .get("receipt")
            .cloned()
            .ok_or_else(|| sqlx::Error::Protocol("receipt envelope missing receipt".to_owned()))?,
    )
    .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
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

    let mut tx = store.connection().begin().await?;
    let existing_receipt: Option<(String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT report_body_sha256, disposition, receipt_body FROM report_receipts WHERE report_id = ?",
    )
    .bind(report_id)
    .fetch_optional(&mut *tx)
    .await?;
    if existing_receipt.is_some() {
        sqlx::query(
            "UPDATE spool_state SET store_fatal = 1, store_error = ?, updated_at = ? WHERE singleton = 1",
        )
        .bind("pending report has a pre-existing receipt")
        .bind(applied_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Err(sqlx::Error::Protocol(
            "pending report has a pre-existing receipt".to_owned(),
        ));
    }
    let (raw_report, stored_body_sha256, stored_body_bytes): (Vec<u8>, String, i64) =
        sqlx::query_as("SELECT body, body_sha256, body_bytes FROM reports WHERE report_id = ?")
            .bind(report_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| sqlx::Error::Protocol("report is not in the Agent spool".to_owned()))?;
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
    let parsed_report: AgentReport = serde_json::from_slice(&raw_report)
        .map_err(|error| sqlx::Error::Protocol(format!("stored report is invalid: {error}")))?;
    parsed_report
        .validate()
        .map_err(|error| sqlx::Error::Protocol(format!("stored report is invalid: {error}")))?;
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
                    sqlx::query(
                        "DELETE FROM block_summaries WHERE node_id = ? AND block_number = ?",
                    )
                    .bind(sample.node_id.to_string())
                    .bind(from_height as i64)
                    .execute(&mut *tx)
                    .await?;
                } else {
                    sqlx::query("DELETE FROM history_gaps WHERE node_id = ? AND from_height = ? AND to_height = ?")
                        .bind(sample.node_id.to_string())
                        .bind(from_height as i64)
                        .bind(to_height as i64)
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

    sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, receipt_body, applied_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(report_id) DO NOTHING")
        .bind(report_id)
        .bind(body_sha256)
        .bind(disposition)
        .bind(receipt_body)
        .bind(applied_at)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM reports WHERE report_id = ? AND EXISTS (SELECT 1 FROM report_receipts WHERE report_id = ?)")
        .bind(report_id)
        .bind(report_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
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
            timestamp(),
            &FailClosedRpcAdapter,
        );
        assert_eq!(node.chain.rpc.status, ComponentStatus::Error);
        assert!(node.chain.rpc.error.is_some());
        assert_eq!(node.chain.sync.status, ComponentStatus::Error);
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
        store.close().await.unwrap();

        collect_and_persist(&config, &ScriptedRpcAdapter::new(snapshot()))
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
        assert_eq!(state, (3, 5));
        let reports: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reports")
            .fetch_one(reopened.connection())
            .await
            .unwrap();
        assert_eq!(reports, 1);
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
        sqlx::query("INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, boot_state, pending_transition, pending_previous_boot_id, updated_at) VALUES (1, ?, 1, ?, 0, 1, 'drained_pending', 'drained_previous', ?, ?)")
            .bind("0195f2a1-0011-4011-8011-000000000011")
            .bind(new_boot)
            .bind(old_boot)
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
        let state: (String, Option<String>, String) = sqlx::query_as(
            "SELECT boot_id, pending_transition, boot_state FROM agent_state WHERE singleton=1",
        )
        .fetch_one(reopened.connection())
        .await
        .unwrap();
        assert_eq!(state, (new_boot.to_owned(), None, "active".to_owned()));
        reopened.close().await.unwrap();
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
            "now",
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reports WHERE report_id=?")
            .bind(report_id)
            .fetch_one(store.connection())
            .await
            .unwrap();
        assert_eq!(count, 0);
        store.close().await.unwrap();
    }
}
