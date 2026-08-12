//! I/O-free shared foundation for PlatPulse.
//!
//! This crate is the only place where Agent↔Server wire concepts may live:
//! the frozen AgentReport v1 contract (envelope, Observation Envelope,
//! Inventory, Host/Node observations, Block Summary, History Gap, Report
//! Receipt, rejection codes) and its wire validation.
//!
//! Constraints:
//! - no I/O: no Axum, SQLx, Alloy, filesystem, network, or clock access;
//!   JSON serialization is in-memory only;
//! - no Server rows and no Public/Admin DTOs — those belong to the Server
//!   crate;
//! - encoding rules are frozen: `snake_case` fields, stable lowercase
//!   enums, RFC 3339 UTC timestamps (canonical `YYYY-MM-DDTHH:MM:SSZ`),
//!   Unix-millisecond block timestamps, `_ms`-suffixed durations, JSON
//!   integers, and a strict distinction between `omitted`, `null` (never
//!   allowed), `0`, and authoritative empty arrays.
//!
//! Canonical/historical wire fixtures live in `tests/fixtures/` and are
//! frozen: any semantic drift of the v1 contract must show up as a fixture
//! test failure.

pub mod block;
pub mod component;
pub mod envelope;
pub mod error;
pub mod gap;
pub mod hex;
pub mod identity;
pub mod inventory;
pub mod network;
pub mod observation;
pub mod protocol;
pub mod receipt;
pub mod time;

pub use block::{
    BlockProductionAttribution, BlockSource, BlockSummary, ProtocolProposer, SealSignerMatch,
};
pub use component::{BoundedError, ComponentKey, ComponentObservation, ComponentStatus};
pub use envelope::{AgentCapability, AgentReport, BootTransition};
pub use error::WireError;
pub use gap::{GapKind, HistoryGap};
pub use hex::{Address, FingerprintHex, Hash32, Sha256Hex};
pub use identity::{AgentId, BootId, NodeId, ReportId};
pub use inventory::{InventoryNode, NodeInventory, ProcessSelector};
pub use network::{NetworkIdentity, NetworkKey, RpcEndpoint, RpcScheme};
pub use observation::{
    ConsensusCurrent, DiskCurrent, HostObservation, LoadCurrent, MemoryCurrent, MountUsage,
    NetworkThroughput, NodeChainObservation, NodeObservation, NodeStaticMetadata, ProcessCurrent,
    RpcCurrent, SpoolDiagnostics, SyncCurrent,
};
pub use receipt::{
    ComponentRevision, InventoryDisposition, NodeCurrentDisposition, NodeReceipt,
    ReceiptDisposition, Rejection, RejectionCode, ReportReceipt, SampleDisposition,
    SampleDispositionKind, SampleRef,
};
pub use time::Rfc3339;

/// Version of the Agent→Server wire protocol this workspace targets.
pub const PROTOCOL_VERSION: u64 = protocol::PROTOCOL_VERSION;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_v1() {
        assert_eq!(PROTOCOL_VERSION, 1);
        assert_eq!(protocol::SUPPORTED_PROTOCOL_MAJORS, &[1]);
    }
}
