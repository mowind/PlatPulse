//! The immutable AgentReport v1 envelope.
//!
//! A report contains the Agent's complete current observation view together
//! with newly collected bounded samples. Reports are persisted before
//! sending and retried with identical bytes/`report_id`; the Server returns
//! one exact Report Receipt per `report_id`.

use std::collections::HashSet;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::block::BlockSummary;
use crate::component::validate_component;
use crate::error::WireError;
use crate::gap::HistoryGap;
use crate::identity::{AgentId, BootId, NodeId, ReportId};
use crate::inventory::NodeInventory;
use crate::observation::{HostObservation, NodeChainObservation, NodeObservation, PeerSnapshot};
use crate::protocol::PROTOCOL_VERSION;
use crate::time::Rfc3339;

/// How this report relates to the Agent's boot lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootTransition {
    /// A normal report of the current boot.
    Continuing,
    /// The final report of the current boot (graceful shutdown); after its
    /// receipt is applied the Server marks the boot closed.
    Closing,
    /// First report of a new boot after a recovery-drain finished the
    /// previous boot; must carry `previous_boot_id`.
    DrainedPrevious,
    /// Reserved for a future explicit Server-approved recovery flow; not
    /// valid in v1.
    RecoveredAfterStale,
}

/// Agent-declared capabilities. The Server may use them for diagnostics and
/// compatibility hints; they never grant Server-side authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    /// Runtime RPC namespace and method capability probing.
    RpcCapabilityProbe,
    /// Bounded synchronization status collection.
    SyncStatus,
    /// Per-Node Block Summaries with Block Production Attribution.
    BlockSummary,
    /// History Gap reporting.
    HistoryGap,
    /// Process observation through an explicit systemd unit selector.
    ProcessSystemd,
    /// Process observation through an explicit PID file selector.
    ProcessPidFile,
    /// Bounded `debug_consensusStatus` collection.
    ConsensusStatus,
    /// Bounded `admin_peers` Peer Snapshot collection.
    PeerSnapshot,
}

/// The immutable AgentReport envelope (protocol v1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentReport {
    /// Wire protocol major; must equal `PROTOCOL_VERSION` (1).
    pub protocol_version: u64,
    /// Agent identity, issued by the Server at Enrollment.
    pub agent_id: AgentId,
    /// Server-controlled generation of the Agent identity; Enrollment,
    /// Recovery, or Reset advances it.
    pub agent_epoch: u64,
    /// Identity of the boot that produced this report.
    pub boot_id: BootId,
    /// Previous boot this report drains from; required exactly for
    /// `drained_previous`/`recovered_after_stale` transitions.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub previous_boot_id: Option<BootId>,
    /// Boot lifecycle transition this report represents.
    pub boot_transition: BootTransition,
    /// Per-boot monotonic sequence, starting at 1. Gaps are allowed, but the
    /// sequence never regresses within a boot.
    pub report_sequence: u64,
    /// Immutable identity of this report; retries reuse it with identical
    /// body bytes.
    pub report_id: ReportId,
    /// UTC time the report was generated.
    pub generated_at: Rfc3339,
    /// Agent software version (contract limit: 128 chars).
    pub agent_version: String,
    /// Agent-declared capabilities (may be `[]`).
    pub agent_capabilities: Vec<AgentCapability>,
    /// The complete Node Inventory and its revision.
    pub inventory: NodeInventory,
    /// Host-level observation, collected once per Agent.
    pub host: HostObservation,
    /// Per-Node current component observations.
    pub nodes: Vec<NodeObservation>,
    /// Newly collected per-Node Block Summaries (may be `[]`).
    pub block_summaries: Vec<BlockSummary>,
    /// Newly declared History Gaps (may be `[]`).
    pub history_gaps: Vec<HistoryGap>,
}

impl AgentReport {
    /// Validates the structural wire invariants of this report.
    ///
    /// The Server revalidates every field after deserialization (it is the
    /// trust boundary); this is the shared contract check used by both
    /// sides.
    pub fn validate(&self) -> Result<(), WireError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(WireError::UnsupportedProtocolVersion {
                got: self.protocol_version,
                supported: PROTOCOL_VERSION,
            });
        }
        if self.report_sequence == 0 {
            return Err(WireError::ReportSequenceZero);
        }
        match self.boot_transition {
            BootTransition::DrainedPrevious | BootTransition::RecoveredAfterStale => {
                if self.previous_boot_id.is_none() {
                    return Err(WireError::MissingPreviousBootId {
                        transition: self.boot_transition,
                    });
                }
            }
            BootTransition::Continuing | BootTransition::Closing => {
                if self.previous_boot_id.is_some() {
                    return Err(WireError::UnexpectedPreviousBootId {
                        transition: self.boot_transition,
                    });
                }
            }
        }
        if self.boot_transition == BootTransition::RecoveredAfterStale {
            return Err(WireError::ReservedBootTransitionInV1 {
                transition: self.boot_transition,
            });
        }
        check_timestamp("generated_at", self.generated_at)?;
        self.validate_inventory()?;
        self.validate_observations()?;
        self.validate_blocks()?;
        self.validate_gaps()?;
        Ok(())
    }

    fn validate_inventory(&self) -> Result<(), WireError> {
        if self.inventory.revision == 0 {
            return Err(WireError::InventoryRevisionZero);
        }
        if self.inventory.nodes.len() > crate::protocol::MAX_INVENTORY_NODES {
            return Err(WireError::TooManyEntries {
                field: "inventory.nodes",
                len: self.inventory.nodes.len(),
                max: crate::protocol::MAX_INVENTORY_NODES,
            });
        }
        if self.agent_capabilities.len() > crate::protocol::MAX_AGENT_CAPABILITIES {
            return Err(WireError::TooManyEntries {
                field: "agent_capabilities",
                len: self.agent_capabilities.len(),
                max: crate::protocol::MAX_AGENT_CAPABILITIES,
            });
        }
        let mut seen = HashSet::with_capacity(self.inventory.nodes.len());
        for node in &self.inventory.nodes {
            if !seen.insert(node.node_id) {
                return Err(WireError::DuplicateInventoryNode {
                    node_id: node.node_id,
                });
            }
            if let Some(name) = &node.display_name {
                check_len("display_name", name, 128)?;
            }
            if let Some(selector) = &node.process {
                match selector {
                    crate::inventory::ProcessSelector::SystemdUnit { unit } => {
                        check_len("process.unit", unit, 512)?;
                    }
                    crate::inventory::ProcessSelector::PidFile { path } => {
                        check_len("process.path", path, 512)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_observations(&self) -> Result<(), WireError> {
        check_len("agent_version", &self.agent_version, 128)?;
        for capability in &self.agent_capabilities {
            let capability = match capability {
                AgentCapability::RpcCapabilityProbe => "rpc_capability_probe",
                AgentCapability::SyncStatus => "sync_status",
                AgentCapability::BlockSummary => "block_summary",
                AgentCapability::HistoryGap => "history_gap",
                AgentCapability::ProcessSystemd => "process_systemd",
                AgentCapability::ProcessPidFile => "process_pid_file",
                AgentCapability::ConsensusStatus => "consensus_status",
                AgentCapability::PeerSnapshot => "peer_snapshot",
            };
            check_len("agent_capabilities[]", capability, 128)?;
        }
        let known: HashSet<NodeId> = self
            .inventory
            .nodes
            .iter()
            .map(|node| node.node_id)
            .collect();

        if self.nodes.len() > crate::protocol::MAX_NODE_OBSERVATIONS {
            return Err(WireError::TooManyEntries {
                field: "nodes",
                len: self.nodes.len(),
                max: crate::protocol::MAX_NODE_OBSERVATIONS,
            });
        }
        if self.block_summaries.len() > crate::protocol::MAX_BLOCK_SUMMARIES {
            return Err(WireError::TooManyEntries {
                field: "block_summaries",
                len: self.block_summaries.len(),
                max: crate::protocol::MAX_BLOCK_SUMMARIES,
            });
        }
        if self.history_gaps.len() > crate::protocol::MAX_HISTORY_GAPS {
            return Err(WireError::TooManyEntries {
                field: "history_gaps",
                len: self.history_gaps.len(),
                max: crate::protocol::MAX_HISTORY_GAPS,
            });
        }
        // The report is the complete current observation view: exactly one
        // entry per Inventory Node, and no entry outside the Inventory.
        let mut observed: HashSet<NodeId> = HashSet::with_capacity(self.nodes.len());
        for obs in &self.nodes {
            if !known.contains(&obs.node_id) {
                return Err(WireError::ObservationForUnknownNode {
                    node_id: obs.node_id,
                });
            }
            if !observed.insert(obs.node_id) {
                return Err(WireError::DuplicateNodeObservation {
                    node_id: obs.node_id,
                });
            }
        }
        for node in &self.inventory.nodes {
            if !observed.contains(&node.node_id) {
                return Err(WireError::MissingNodeObservation {
                    node_id: node.node_id,
                });
            }
        }

        validate_host_components(&self.host)?;

        for obs in &self.nodes {
            validate_component("process", &obs.process)?;
            if let Some(process) = &obs.process.latest {
                if !process.cpu_percent.is_finite() || process.cpu_percent < 0.0 {
                    return Err(WireError::ValueNotFinite {
                        field: "process.cpu_percent",
                    });
                }
            }
            validate_chain_components(&obs.chain)?;
            if let Some(peers) = &obs.chain.peers {
                validate_peer_snapshot(obs.node_id, peers)?;
            }
            if let Some(sync) = &obs.chain.sync.latest {
                if sync.current_block > sync.highest_block {
                    return Err(WireError::ValueOutOfRange {
                        field: "sync.current_block",
                    });
                }
            }
            if let Some(rpc) = &obs.chain.rpc.latest {
                check_len("rpc.client_version", &rpc.client_version, 256)?;
                check_entries("rpc.namespaces", rpc.namespaces.len(), 64)?;
                for namespace in &rpc.namespaces {
                    check_len("rpc.namespaces[]", namespace, 64)?;
                }
                check_entries("rpc.methods", rpc.methods.len(), 512)?;
                for method in &rpc.methods {
                    check_len("rpc.methods[]", method, 128)?;
                }
            }
            if let Some(metadata) = &obs.chain.static_metadata.latest {
                if let Some(enode) = &metadata.enode {
                    check_len("static_metadata.enode", enode, 512)?;
                }
            }
            if let Some(identity) = &obs.chain.network_identity.latest {
                if let Some(hrp) = &identity.address_hrp {
                    check_len("network_identity.address_hrp", hrp, 16)?;
                }
            }
        }
        Ok(())
    }

    fn validate_blocks(&self) -> Result<(), WireError> {
        let known: HashSet<NodeId> = self
            .inventory
            .nodes
            .iter()
            .map(|node| node.node_id)
            .collect();
        let mut seen_samples = HashSet::with_capacity(self.block_summaries.len());
        for block in &self.block_summaries {
            if !seen_samples.insert((block.node_id, block.block_number)) {
                return Err(WireError::DuplicateBlockSample {
                    node_id: block.node_id,
                    height: block.block_number,
                });
            }
            if !known.contains(&block.node_id) {
                return Err(WireError::BlockSampleForUnknownNode {
                    node_id: block.node_id,
                });
            }
            if block.block_number > i64::MAX as u64 {
                return Err(WireError::ValueOutOfRange {
                    field: "block.block_number",
                });
            }
            if block.transaction_count > i64::MAX as u64 {
                return Err(WireError::ValueOutOfRange {
                    field: "block.transaction_count",
                });
            }
            if block
                .block_interval_ms
                .is_some_and(|value| value > i64::MAX as u64)
            {
                return Err(WireError::ValueOutOfRange {
                    field: "block.block_interval_ms",
                });
            }
            check_timestamp("block.observed_at", block.observed_at)?;
            if block.block_timestamp_ms
                > (crate::protocol::MAX_TIMESTAMP_UNIX_SECONDS as u64) * 1000
            {
                return Err(WireError::ValueOutOfRange {
                    field: "block.block_timestamp_ms",
                });
            }
            if let Some(hrp) = &block.network_identity.address_hrp {
                check_len("block.network_identity.address_hrp", hrp, 16)?;
            }
            check_len(
                "block.attribution.attribution_reason",
                &block.attribution.attribution_reason,
                256,
            )?;
            if let crate::block::ProtocolProposer::Verified { identity } =
                &block.attribution.protocol_proposer
            {
                check_len(
                    "block.attribution.protocol_proposer.identity",
                    identity,
                    128,
                )?;
            }
        }
        Ok(())
    }

    fn validate_gaps(&self) -> Result<(), WireError> {
        let known: HashSet<NodeId> = self
            .inventory
            .nodes
            .iter()
            .map(|node| node.node_id)
            .collect();
        let mut seen_gaps = HashSet::with_capacity(self.history_gaps.len());
        for gap in &self.history_gaps {
            if !seen_gaps.insert((gap.node_id, gap.from_height, gap.to_height)) {
                return Err(WireError::DuplicateHistoryGap {
                    node_id: gap.node_id,
                    from_height: gap.from_height,
                    to_height: gap.to_height,
                });
            }
            if !known.contains(&gap.node_id) {
                return Err(WireError::HistoryGapForUnknownNode {
                    node_id: gap.node_id,
                });
            }
            if gap.from_height > gap.to_height {
                return Err(WireError::ReversedGapRange {
                    node_id: gap.node_id,
                    from_height: gap.from_height,
                    to_height: gap.to_height,
                });
            }
            if gap.from_height > i64::MAX as u64 || gap.to_height > i64::MAX as u64 {
                return Err(WireError::ValueOutOfRange {
                    field: "history_gap.height",
                });
            }
            if gap
                .to_height
                .saturating_sub(gap.from_height)
                .saturating_add(1)
                > crate::protocol::MAX_HISTORY_GAP_HEIGHT_SPAN
            {
                return Err(WireError::ValueOutOfRange {
                    field: "history_gap.height_span",
                });
            }
            check_len("history_gap.reason", &gap.reason, 512)?;
        }
        Ok(())
    }
}

fn validate_host_components(host: &HostObservation) -> Result<(), WireError> {
    validate_component("cpu_percent", &host.cpu_percent)?;
    validate_component("memory", &host.memory)?;
    validate_component("load", &host.load)?;
    validate_component("disk", &host.disk)?;
    validate_component("network_throughput", &host.network_throughput)?;
    validate_component("clock_skew", &host.clock_skew)?;
    validate_component("spool", &host.spool)?;
    if let Some(cpu) = host.cpu_percent.latest {
        if !cpu.is_finite() {
            return Err(WireError::ValueNotFinite {
                field: "host.cpu_percent",
            });
        }
        if !(0.0..=100.0).contains(&cpu) {
            return Err(WireError::ValueOutOfRange {
                field: "host.cpu_percent",
            });
        }
    }
    if let Some(load) = &host.load.latest {
        for value in [load.load1, load.load5, load.load15] {
            if !value.is_finite() {
                return Err(WireError::ValueNotFinite { field: "host.load" });
            }
            if value < 0.0 {
                return Err(WireError::ValueOutOfRange { field: "host.load" });
            }
        }
    }
    if let Some(memory) = &host.memory.latest {
        if memory.used_bytes > memory.total_bytes {
            return Err(WireError::UsedExceedsTotal {
                field: "host.memory.used_bytes",
            });
        }
    }
    if let Some(disk) = &host.disk.latest {
        check_entries("disk.mounts", disk.mounts.len(), 128)?;
        for mount in &disk.mounts {
            check_len("disk.mounts[].mount_path", &mount.mount_path, 4096)?;
            if mount.used_bytes > mount.total_bytes {
                return Err(WireError::UsedExceedsTotal {
                    field: "disk.mounts[].used_bytes",
                });
            }
        }
    }
    Ok(())
}

fn validate_chain_components(chain: &NodeChainObservation) -> Result<(), WireError> {
    validate_component("rpc", &chain.rpc)?;
    validate_component("sync", &chain.sync)?;
    validate_component("consensus", &chain.consensus)?;
    validate_component("network_identity", &chain.network_identity)?;
    validate_component("static_metadata", &chain.static_metadata)?;
    if let Some(peers) = &chain.peers {
        validate_component("peers", peers)?;
    }
    Ok(())
}

fn validate_peer_snapshot(
    node_id: NodeId,
    component: &crate::component::ComponentObservation<PeerSnapshot>,
) -> Result<(), WireError> {
    let Some(snapshot) = component.latest.as_ref() else {
        return Ok(());
    };
    if snapshot.peers.len() > crate::protocol::MAX_PEERS {
        return Err(WireError::TooManyEntries {
            field: "peers",
            len: snapshot.peers.len(),
            max: crate::protocol::MAX_PEERS,
        });
    }
    let mut peer_ids = HashSet::with_capacity(snapshot.peers.len());
    for peer in &snapshot.peers {
        if peer.peer_id.is_empty() {
            return Err(WireError::ValueOutOfRange {
                field: "peers[].peer_id",
            });
        }
        check_len(
            "peers[].peer_id",
            &peer.peer_id,
            crate::protocol::MAX_PEER_ID_BYTES,
        )?;
        if !peer_ids.insert(peer.peer_id.as_str()) {
            return Err(WireError::DuplicatePeerId {
                node_id,
                peer_id: peer.peer_id.clone(),
            });
        }
        if let Some(remote_ip) = &peer.remote_ip {
            check_len(
                "peers[].remote_ip",
                remote_ip,
                crate::protocol::MAX_PEER_REMOTE_IP_BYTES,
            )?;
            if remote_ip.parse::<IpAddr>().is_err() {
                return Err(WireError::ValueOutOfRange {
                    field: "peers[].remote_ip",
                });
            }
        }
        if let Some(client_name) = &peer.client_name {
            if client_name.is_empty() {
                return Err(WireError::ValueOutOfRange {
                    field: "peers[].client_name",
                });
            }
            check_len(
                "peers[].client_name",
                client_name,
                crate::protocol::MAX_PEER_CLIENT_NAME_BYTES,
            )?;
        }
        check_entries(
            "peers[].caps",
            peer.caps.len(),
            crate::protocol::MAX_PEER_CAPABILITIES,
        )?;
        let mut capabilities = HashSet::with_capacity(peer.caps.len());
        for cap in &peer.caps {
            if cap.is_empty() {
                return Err(WireError::ValueOutOfRange {
                    field: "peers[].caps[]",
                });
            }
            if !capabilities.insert(cap.as_str()) {
                return Err(WireError::ValueOutOfRange {
                    field: "peers[].caps",
                });
            }
            check_len(
                "peers[].caps[]",
                cap,
                crate::protocol::MAX_PEER_CAPABILITY_BYTES,
            )?;
        }
        if peer
            .cbft_protocol_version
            .is_some_and(|version| version > crate::protocol::MAX_PEER_CBFT_PROTOCOL_VERSION)
        {
            return Err(WireError::ValueOutOfRange {
                field: "peers[].cbft_protocol_version",
            });
        }
        for (field, value) in [
            ("peers[].cbft_highest_qc_block", peer.cbft_highest_qc_block),
            ("peers[].cbft_locked_block", peer.cbft_locked_block),
            ("peers[].cbft_commit_block", peer.cbft_commit_block),
        ] {
            if value.is_some_and(|value| value > crate::protocol::MAX_PEER_CBFT_BLOCK) {
                return Err(WireError::ValueOutOfRange { field });
            }
        }
    }
    Ok(())
}

fn check_timestamp(field: &'static str, value: Rfc3339) -> Result<(), WireError> {
    let seconds = value.as_datetime().unix_timestamp();
    if !(crate::protocol::MIN_TIMESTAMP_UNIX_SECONDS..=crate::protocol::MAX_TIMESTAMP_UNIX_SECONDS)
        .contains(&seconds)
    {
        return Err(WireError::ValueOutOfRange { field });
    }
    Ok(())
}
fn check_len(field: &'static str, value: &str, max: usize) -> Result<(), WireError> {
    if value.len() > max {
        return Err(WireError::FieldTooLong {
            field,
            len: value.len(),
            max,
        });
    }
    Ok(())
}

fn check_entries(field: &'static str, len: usize, max: usize) -> Result<(), WireError> {
    if len > max {
        return Err(WireError::TooManyEntries { field, len, max });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::ComponentStatus;

    /// A minimal structurally valid report used to exercise invariants.
    pub(crate) fn minimal_report() -> AgentReport {
        let fixture = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/report_v1_minimal.json"
        ));
        serde_json::from_str(fixture).unwrap()
    }

    #[test]
    fn minimal_report_is_valid() {
        let report = minimal_report();
        assert_eq!(report.validate(), Ok(()));
    }

    #[test]
    fn unsupported_protocol_major_is_rejected() {
        let mut report = minimal_report();
        report.protocol_version = 2;
        assert_eq!(
            report.validate(),
            Err(WireError::UnsupportedProtocolVersion {
                got: 2,
                supported: 1
            })
        );
        report.protocol_version = 0;
        assert!(report.validate().is_err());
    }

    #[test]
    fn sequence_must_start_at_one() {
        let mut report = minimal_report();
        report.report_sequence = 0;
        assert_eq!(report.validate(), Err(WireError::ReportSequenceZero));
    }

    #[test]
    fn drained_requires_previous_boot_id() {
        let mut report = minimal_report();
        report.boot_transition = BootTransition::DrainedPrevious;
        assert_eq!(
            report.validate(),
            Err(WireError::MissingPreviousBootId {
                transition: BootTransition::DrainedPrevious
            })
        );
        report.previous_boot_id = Some(report.boot_id);
        assert_eq!(report.validate(), Ok(()));
    }

    #[test]
    fn continuing_rejects_previous_boot_id() {
        let mut report = minimal_report();
        report.previous_boot_id = Some(report.boot_id);
        assert_eq!(
            report.validate(),
            Err(WireError::UnexpectedPreviousBootId {
                transition: BootTransition::Continuing
            })
        );
    }

    #[test]
    fn recovered_after_stale_is_reserved() {
        let mut report = minimal_report();
        report.boot_transition = BootTransition::RecoveredAfterStale;
        report.previous_boot_id = Some(report.boot_id);
        assert_eq!(
            report.validate(),
            Err(WireError::ReservedBootTransitionInV1 {
                transition: BootTransition::RecoveredAfterStale
            })
        );
    }

    #[test]
    fn inventory_revision_must_be_positive() {
        let mut report = minimal_report();
        report.inventory.revision = 0;
        assert_eq!(report.validate(), Err(WireError::InventoryRevisionZero));
    }

    #[test]
    fn duplicate_inventory_node_is_rejected() {
        let mut report = minimal_report();
        let node = report.inventory.nodes[0].clone();
        report.inventory.nodes.push(node);
        assert_eq!(
            report.validate(),
            Err(WireError::DuplicateInventoryNode {
                node_id: report.inventory.nodes[0].node_id
            })
        );
    }

    #[test]
    fn observations_must_reference_inventory_nodes() {
        let mut report = minimal_report();
        report.nodes[0].node_id = "99999999-9999-4999-8999-999999999999".parse().unwrap();
        assert!(matches!(
            report.validate(),
            Err(WireError::ObservationForUnknownNode { .. })
        ));
    }

    #[test]
    fn observations_must_cover_every_inventory_node_exactly_once() {
        let mut report = minimal_report();
        report.nodes.clear();
        assert!(matches!(
            report.validate(),
            Err(WireError::MissingNodeObservation { node_id }) if node_id == report.inventory.nodes[0].node_id
        ));

        let mut report = minimal_report();
        report.nodes.push(report.nodes[0].clone());
        assert!(matches!(
            report.validate(),
            Err(WireError::DuplicateNodeObservation { node_id }) if node_id == report.inventory.nodes[0].node_id
        ));
    }

    #[test]
    fn reversed_gap_range_is_rejected() {
        let mut report = minimal_report();
        let mut gap = crate::gap::HistoryGap {
            node_id: report.inventory.nodes[0].node_id,
            kind: crate::gap::GapKind::SpoolOverflow,
            from_height: 20,
            to_height: 10,
            reason: "capacity cleanup".into(),
            recorded_at: report.generated_at,
        };
        report.history_gaps.push(gap.clone());
        assert_eq!(
            report.validate(),
            Err(WireError::ReversedGapRange {
                node_id: gap.node_id,
                from_height: 20,
                to_height: 10
            })
        );
        gap.from_height = 10;
        gap.to_height = 20;
        report.history_gaps.pop();
        report.history_gaps.push(gap);
        assert_eq!(report.validate(), Ok(()));
    }

    #[test]
    fn duplicate_block_sample_is_rejected() {
        let mut report: AgentReport = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/report_v1_canonical.json"
        )))
        .unwrap();
        let sample = report.block_summaries[0].clone();
        report.block_summaries.push(sample.clone());
        assert_eq!(
            report.validate(),
            Err(WireError::DuplicateBlockSample {
                node_id: sample.node_id,
                height: sample.block_number,
            })
        );
    }

    #[test]
    fn block_quantities_fit_the_sqlite_projection() {
        let mut report: AgentReport = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/report_v1_canonical.json"
        )))
        .unwrap();
        report.block_summaries[0].transaction_count = i64::MAX as u64 + 1;
        assert_eq!(
            report.validate(),
            Err(WireError::ValueOutOfRange {
                field: "block.transaction_count"
            })
        );
    }

    #[test]
    fn duplicate_history_gap_is_rejected() {
        let mut report = minimal_report();
        let gap = crate::gap::HistoryGap {
            node_id: report.inventory.nodes[0].node_id,
            kind: crate::gap::GapKind::SpoolOverflow,
            from_height: 10,
            to_height: 20,
            reason: "capacity cleanup".into(),
            recorded_at: report.generated_at,
        };
        report.history_gaps.push(gap.clone());
        report.history_gaps.push(gap.clone());
        assert_eq!(
            report.validate(),
            Err(WireError::DuplicateHistoryGap {
                node_id: gap.node_id,
                from_height: gap.from_height,
                to_height: gap.to_height,
            })
        );
    }

    #[test]
    fn history_gap_span_is_bounded() {
        let mut report = minimal_report();
        report.history_gaps.push(crate::gap::HistoryGap {
            node_id: report.inventory.nodes[0].node_id,
            kind: crate::gap::GapKind::SpoolOverflow,
            from_height: 1,
            to_height: crate::protocol::MAX_HISTORY_GAP_HEIGHT_SPAN + 1,
            reason: "too broad".into(),
            recorded_at: report.generated_at,
        });
        assert_eq!(
            report.validate(),
            Err(WireError::ValueOutOfRange {
                field: "history_gap.height_span"
            })
        );
    }

    #[test]
    fn history_gap_heights_fit_the_sqlite_projection() {
        let mut report = minimal_report();
        report.history_gaps.push(crate::gap::HistoryGap {
            node_id: report.inventory.nodes[0].node_id,
            kind: crate::gap::GapKind::SpoolOverflow,
            from_height: i64::MAX as u64 + 1,
            to_height: i64::MAX as u64 + 1,
            reason: "outside sqlite range".into(),
            recorded_at: report.generated_at,
        });
        assert_eq!(
            report.validate(),
            Err(WireError::ValueOutOfRange {
                field: "history_gap.height"
            })
        );
    }

    #[test]
    fn generated_timestamp_outside_contract_is_rejected() {
        let mut report = minimal_report();
        report.generated_at = "2200-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(
            report.validate(),
            Err(WireError::ValueOutOfRange {
                field: "generated_at"
            })
        );
    }
    #[test]
    fn component_invariant_violations_are_reported() {
        let mut report = minimal_report();
        let mut component = report.host.cpu_percent.clone();
        component.status = ComponentStatus::Ok;
        component.latest = Some(42.0);
        component.latest_observed_at = None;
        report.host.cpu_percent = component;
        assert!(matches!(
            report.validate(),
            Err(WireError::ComponentLatestWithoutObservedAt {
                component: "cpu_percent"
            })
        ));
    }

    #[test]
    fn bounded_error_too_long_is_rejected() {
        let mut report = minimal_report();
        let mut component = report.host.cpu_percent.clone();
        component.status = ComponentStatus::Error;
        component.error = Some(crate::component::BoundedError {
            code: "boom".into(),
            message: "x".repeat(1025),
        });
        report.host.cpu_percent = component;
        assert!(matches!(
            report.validate(),
            Err(WireError::FieldTooLong {
                field: "error.message",
                ..
            })
        ));
    }

    #[test]
    fn agent_version_too_long_is_rejected() {
        let mut report = minimal_report();
        report.agent_version = "v".repeat(129);
        assert!(matches!(
            report.validate(),
            Err(WireError::FieldTooLong {
                field: "agent_version",
                ..
            })
        ));
    }

    #[test]
    fn empty_inventory_and_empty_samples_are_authoritative() {
        let mut report = minimal_report();
        report.inventory.nodes.clear();
        report.nodes.clear();
        assert_eq!(report.validate(), Ok(()));
    }

    #[test]
    fn boot_transition_wire_forms() {
        assert_eq!(
            serde_json::to_string(&BootTransition::DrainedPrevious).unwrap(),
            "\"drained_previous\""
        );
        assert_eq!(
            serde_json::to_string(&BootTransition::RecoveredAfterStale).unwrap(),
            "\"recovered_after_stale\""
        );
        assert!(serde_json::from_str::<BootTransition>("\"jumping\"").is_err());
    }
}
