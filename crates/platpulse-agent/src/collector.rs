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
    NodeChainObservation, NodeObservation, NodeStaticMetadata, RpcCurrent, SpoolDiagnostics,
    SyncCurrent,
};
use platpulse_core::{AgentCapability, AgentReport, BootTransition, FingerprintHex, Rfc3339};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use sqlx::Connection;
use sysinfo::{Disks, System};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::AgentConfig;
use crate::database::{AgentDatabaseConfig, AgentStore};
use crate::reporting::ReportStoreError;

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

fn timestamp() -> Rfc3339 {
    let value = OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("zero nanoseconds is valid")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC timestamp is valid");
    value.parse().expect("UTC timestamp is valid")
}

fn ok<T>(value: T, at: Rfc3339) -> ComponentObservation<T> {
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
fn clock_skew_error(at: Rfc3339, message: &str) -> ComponentObservation<i64> {
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
    let (rpc, network_identity, static_metadata, sync, consensus) =
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
            ),
            Err(_) => (
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
        },
    }
}

/// Build a complete report with the supplied clock-skew observation.
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
            agent_capabilities: vec![AgentCapability::RpcCapabilityProbe],
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
    let mut capabilities = vec![AgentCapability::RpcCapabilityProbe];
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

/// Collect and persist one complete immutable report. Agent state (identity,
/// boot and sequence) is advanced in the same transaction as the report body.
pub async fn collect_and_persist<A: RpcAdapter>(
    config: &AgentConfig,
    adapter: &A,
) -> Result<String, CollectionError> {
    let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db)).await?;
    let state: Option<(String, i64, Option<String>, i64)> = sqlx::query_as(
        "SELECT agent_id, agent_epoch, boot_id, report_sequence FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(store.connection())
    .await?;
    let (agent_text, epoch, boot_text, previous_sequence) =
        state.ok_or(CollectionError::NotEnrolled)?;
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
    let report = collect_report_with_clock_skew(
        config,
        agent_id,
        epoch as u64,
        boot_id,
        previous_sequence as u64 + 1,
        validated.inventory,
        adapter,
        clock_skew,
    )?;
    let body = serde_json::to_vec(&report)?;
    let digest = format!("0x{}", hex::encode(sha2::Sha256::digest(&body)));
    let now = report.generated_at.to_string();
    let mut tx = store.connection().begin().await?;
    sqlx::query("INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, in_flight, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?)")
        .bind(report.report_id.to_string()).bind(report.agent_epoch as i64).bind(report.boot_id.to_string()).bind(report.report_sequence as i64).bind(report.generated_at.to_string()).bind(&body).bind(&digest).bind(body.len() as i64).bind(&now).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, updated_at) VALUES (1, ?, ?, ?, ?, ?, ?) ON CONFLICT(singleton) DO UPDATE SET boot_id=excluded.boot_id, report_sequence=excluded.report_sequence, inventory_revision=excluded.inventory_revision, updated_at=excluded.updated_at")
        .bind(report.agent_id.to_string()).bind(report.agent_epoch as i64).bind(report.boot_id.to_string()).bind(report.report_sequence as i64).bind(report.inventory.revision as i64).bind(&now).execute(&mut *tx).await?;
    tx.commit().await?;
    store.close().await?;
    Ok(digest)
}

/// Apply a stored receipt and delete its report only after the receipt is
/// durably recorded, in one Agent Store transaction.
pub async fn apply_receipt(
    store: &mut AgentStore,
    report_id: &str,
    body_sha256: &str,
    disposition: &str,
    receipt_body: &[u8],
    applied_at: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = store.connection().begin().await?;
    sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, receipt_body, applied_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(report_id) DO NOTHING")
        .bind(report_id).bind(body_sha256).bind(disposition).bind(receipt_body).bind(applied_at).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM reports WHERE report_id = ? AND EXISTS (SELECT 1 FROM report_receipts WHERE report_id = ?)")
        .bind(report_id).bind(report_id).execute(&mut *tx).await?;
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::AgentDatabaseConfig;
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
    async fn receipt_application_is_atomic_and_removes_report() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        sqlx::query("INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, created_at) VALUES ('r',1,'b',1,'now',X'01','h',1,'now')").execute(store.connection()).await.unwrap();
        apply_receipt(&mut store, "r", "h", "accepted", b"{}", "now")
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reports WHERE report_id='r'")
            .fetch_one(store.connection())
            .await
            .unwrap();
        assert_eq!(count, 0);
        store.close().await.unwrap();
    }
}
