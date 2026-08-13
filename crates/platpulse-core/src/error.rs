//! Wire-validation error type returned by [`crate::AgentReport::validate`].
//!
//! Format errors (invalid UUID, hash, timestamp, enum value, unknown field,
//! explicit `null`, …) are rejected at deserialization time by serde. This
//! type covers the structural invariants that cannot be expressed per-field,
//! such as boot-transition consistency, per-Node scoping, gap ranges, and the
//! Observation Envelope rules.

use std::fmt;

use crate::envelope::BootTransition;
use crate::identity::NodeId;
use crate::receipt::{ReceiptDisposition, RejectionCode, SampleDispositionKind};

/// A structural violation of the AgentReport v1 wire contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// `protocol_version` is not the major implemented by this crate.
    UnsupportedProtocolVersion { got: u64, supported: u64 },
    /// `report_sequence` must be a positive integer; sequences start at 1 per
    /// boot and only ever increase (gaps are allowed, regression is not).
    ReportSequenceZero,
    /// `inventory.revision` must be positive and monotonic per Agent.
    InventoryRevisionZero,
    /// A `DrainedPrevious`/`RecoveredAfterStale` report must identify the
    /// boot it is draining from via `previous_boot_id`.
    MissingPreviousBootId { transition: BootTransition },
    /// `Continuing`/`Closing` reports must not carry `previous_boot_id`.
    UnexpectedPreviousBootId { transition: BootTransition },
    /// `RecoveredAfterStale` is reserved for a future Server-approved
    /// recovery flow and must not be sent by v1 Agents.
    ReservedBootTransitionInV1 { transition: BootTransition },
    /// The Inventory lists the same Node ID twice; an Inventory is a complete
    /// set and must be unique.
    DuplicateInventoryNode { node_id: NodeId },
    /// A per-Node observation references a Node that is not in the Inventory.
    ObservationForUnknownNode { node_id: NodeId },
    /// The report must carry exactly one current observation per Inventory
    /// Node (complete current observation view); this Node is missing one.
    MissingNodeObservation { node_id: NodeId },
    /// Two per-Node observation entries reference the same Node.
    DuplicateNodeObservation { node_id: NodeId },
    /// A Block Summary references a Node that is not in the Inventory.
    BlockSampleForUnknownNode { node_id: NodeId },
    /// A History Gap references a Node that is not in the Inventory.
    HistoryGapForUnknownNode { node_id: NodeId },
    /// A block sample repeats the same Node/height identity in one report.
    DuplicateBlockSample { node_id: NodeId, height: u64 },
    /// A gap repeats the same Node/range identity in one report.
    DuplicateHistoryGap {
        node_id: NodeId,
        from_height: u64,
        to_height: u64,
    },
    /// must not be reversed.
    ReversedGapRange {
        node_id: NodeId,
        from_height: u64,
        to_height: u64,
    },
    /// `latest` is present without `latest_observed_at` (or the reverse): a
    /// last-good value always carries the time it was observed.
    ComponentLatestWithoutObservedAt { component: &'static str },
    /// `latest_observed_at` is present without `latest`: the last-good time
    /// always carries the value it belongs to.
    ComponentObservedAtWithoutLatest { component: &'static str },
    /// `status == error` without a `BoundedError` payload.
    ComponentErrorWithoutMessage { component: &'static str },
    /// A `BoundedError` is present while `status != error`.
    ComponentMessageWithoutError { component: &'static str },
    /// `status == starting` with a last-good value: `starting` means no
    /// successful collection has happened in this boot.
    ComponentStartingWithValue { component: &'static str },
    /// An active status (`starting`/`ok`/`error`) requires an `attempted_at`.
    ComponentAttemptedAtMissing { component: &'static str },
    /// `status == ok` requires the last successful value: an `ok` attempt
    /// produced a value (an authoritative empty result is still a value).
    ComponentOkWithoutValue { component: &'static str },
    /// `received_at` is populated by the Server at commit time; an Agent
    /// report must never carry it (a falsified commit/freshness time is a
    /// trust-boundary violation).
    ComponentCarriesReceivedAt { component: &'static str },
    /// A floating-point observation is NaN or infinite.
    ValueNotFinite { field: &'static str },
    /// A numeric observation is outside its contract range (e.g. CPU outside
    /// 0..=100, negative load, `current_block > highest_block`).
    ValueOutOfRange { field: &'static str },
    /// A used-byte counter exceeds its total-byte counter.
    UsedExceedsTotal { field: &'static str },
    /// An `accepted` receipt cannot carry any rejection.
    ReceiptAcceptedWithRejections,
    /// An `accepted` receipt cannot carry a rejected Node or Node
    /// rejections.
    ReceiptAcceptedWithRejectedNode,
    /// An `accepted` receipt cannot carry a non-accepted sample.
    ReceiptAcceptedWithRejectedSample,
    /// `accepted`/`partially_accepted` receipts always carry an Inventory
    /// disposition, and it must not be `rejected`.
    ReceiptRequiresInventory { disposition: ReceiptDisposition },
    /// `accepted`/`partially_accepted` receipts cannot carry a `rejected`
    /// Inventory disposition.
    ReceiptWithRejectedInventory { disposition: ReceiptDisposition },
    /// A `rejected` receipt must explain itself with at least one rejection.
    ReceiptRejectedWithoutRejection,
    /// A `rejected` receipt cannot carry Node entries.
    ReceiptRejectedWithNodeEntries,
    /// A `rejected` receipt cannot carry sample entries.
    ReceiptRejectedWithSampleEntries,
    /// A `rejected` receipt cannot carry an accepted/unchanged Inventory
    /// disposition.
    ReceiptRejectedWithAcceptedInventory,
    /// A `partially_accepted` receipt must reject at least one Node or
    /// sample.
    ReceiptPartialWithoutRejection,
    /// A rejected Node current must carry at least one rejection.
    NodeRejectedWithoutRejection,
    /// An accepted Node current cannot carry rejections.
    NodeAcceptedWithRejections,
    /// A rejected Node current cannot carry accepted component revisions.
    NodeRejectedWithAcceptedRevisions,
    /// An accepted sample cannot carry a rejection.
    SampleAcceptedWithRejection,
    /// A rejected Node current may not carry a retryable rejection.
    NodeRejectedWithRetryableRejection,

    /// A retryable/terminal-rejected sample must carry its rejection.
    SampleRejectedWithoutRejection,
    /// The sample disposition contradicts the rejection's retryability.
    SampleDispositionRetryableMismatch {
        disposition: SampleDispositionKind,
        retryable: bool,
    },
    /// The `retryable` flag contradicts the code's contract.
    RejectionRetryableMismatch {
        code: RejectionCode,
        got: bool,
        expected: bool,
    },
    /// A bounded string field exceeds its contract limit.
    FieldTooLong {
        field: &'static str,
        len: usize,
        max: usize,
    },
    /// A bounded collection exceeds its contract entry limit.
    TooManyEntries {
        field: &'static str,
        len: usize,
        max: usize,
    },
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedProtocolVersion { got, supported } => write!(
                f,
                "unsupported protocol version {got}; this build implements major {supported}"
            ),
            Self::ReportSequenceZero => write!(f, "report_sequence must be >= 1"),
            Self::InventoryRevisionZero => {
                write!(f, "inventory.revision must be >= 1")
            }
            Self::MissingPreviousBootId { transition } => write!(
                f,
                "boot_transition {transition:?} requires previous_boot_id"
            ),
            Self::UnexpectedPreviousBootId { transition } => write!(
                f,
                "boot_transition {transition:?} must not carry previous_boot_id"
            ),
            Self::ReservedBootTransitionInV1 { transition } => write!(
                f,
                "boot_transition {transition:?} is reserved for a future Server-approved recovery flow and is not valid in protocol v1"
            ),
            Self::DuplicateInventoryNode { node_id } => {
                write!(f, "inventory lists node {node_id} more than once")
            }
            Self::ObservationForUnknownNode { node_id } => {
                write!(
                    f,
                    "node observation references {node_id}, which is not in the inventory"
                )
            }
            Self::MissingNodeObservation { node_id } => write!(
                f,
                "the report must carry exactly one current observation per inventory node; {node_id} is missing"
            ),
            Self::DuplicateNodeObservation { node_id } => write!(
                f,
                "the report carries more than one current observation for {node_id}"
            ),
            Self::BlockSampleForUnknownNode { node_id } => {
                write!(
                    f,
                    "block summary references {node_id}, which is not in the inventory"
                )
            }
            Self::DuplicateBlockSample { node_id, height } => {
                write!(f, "block summary for {node_id} repeats height {height}")
            }
            Self::DuplicateHistoryGap {
                node_id,
                from_height,
                to_height,
            } => write!(
                f,
                "history gap for {node_id} repeats range {from_height}..{to_height}"
            ),
            Self::HistoryGapForUnknownNode { node_id } => {
                write!(
                    f,
                    "history gap references {node_id}, which is not in the inventory"
                )
            }
            Self::ReversedGapRange {
                node_id,
                from_height,
                to_height,
            } => write!(
                f,
                "history gap for {node_id} has reversed range {from_height}..{to_height}"
            ),
            Self::ComponentLatestWithoutObservedAt { component } => write!(
                f,
                "component {component}: latest is present without latest_observed_at"
            ),
            Self::ComponentObservedAtWithoutLatest { component } => write!(
                f,
                "component {component}: latest_observed_at is present without latest"
            ),
            Self::ComponentErrorWithoutMessage { component } => write!(
                f,
                "component {component}: status is error but no bounded error is present"
            ),
            Self::ComponentMessageWithoutError { component } => write!(
                f,
                "component {component}: a bounded error is present but status is not error"
            ),
            Self::ComponentStartingWithValue { component } => write!(
                f,
                "component {component}: status starting must not carry a last-good value"
            ),
            Self::ComponentAttemptedAtMissing { component } => write!(
                f,
                "component {component}: status starting/ok/error requires attempted_at"
            ),
            Self::ComponentOkWithoutValue { component } => write!(
                f,
                "component {component}: status ok requires the last successful value"
            ),
            Self::ComponentCarriesReceivedAt { component } => write!(
                f,
                "component {component}: received_at is Server-populated and must be omitted in Agent reports"
            ),
            Self::ValueNotFinite { field } => {
                write!(f, "field {field} must be a finite number")
            }
            Self::ValueOutOfRange { field } => {
                write!(f, "field {field} is outside its contract range")
            }
            Self::UsedExceedsTotal { field } => {
                write!(f, "field {field} exceeds its total counter")
            }
            Self::ReceiptAcceptedWithRejections => {
                write!(f, "an accepted receipt cannot carry rejections")
            }
            Self::ReceiptAcceptedWithRejectedNode => {
                write!(f, "an accepted receipt cannot carry a rejected Node")
            }
            Self::ReceiptAcceptedWithRejectedSample => {
                write!(f, "an accepted receipt cannot carry a non-accepted sample")
            }
            Self::ReceiptRequiresInventory { disposition } => write!(
                f,
                "a {disposition:?} receipt must carry an inventory disposition"
            ),
            Self::ReceiptWithRejectedInventory { disposition } => write!(
                f,
                "a {disposition:?} receipt cannot carry a rejected inventory disposition"
            ),
            Self::ReceiptRejectedWithoutRejection => {
                write!(f, "a rejected receipt must carry at least one rejection")
            }
            Self::ReceiptRejectedWithNodeEntries => {
                write!(f, "a rejected receipt cannot carry node entries")
            }
            Self::ReceiptRejectedWithSampleEntries => {
                write!(f, "a rejected receipt cannot carry sample entries")
            }
            Self::ReceiptRejectedWithAcceptedInventory => write!(
                f,
                "a rejected receipt cannot carry an accepted/unchanged inventory disposition"
            ),
            Self::ReceiptPartialWithoutRejection => write!(
                f,
                "a partially_accepted receipt must reject at least one Node or sample"
            ),
            Self::NodeRejectedWithoutRejection => {
                write!(
                    f,
                    "a rejected Node current must carry at least one rejection"
                )
            }
            Self::NodeAcceptedWithRejections => {
                write!(f, "an accepted Node current cannot carry rejections")
            }
            Self::NodeRejectedWithAcceptedRevisions => write!(
                f,
                "a rejected Node current cannot carry accepted component revisions"
            ),
            Self::NodeRejectedWithRetryableRejection => write!(
                f,
                "a rejected Node current cannot carry a retryable rejection"
            ),
            Self::SampleAcceptedWithRejection => {
                write!(f, "an accepted sample cannot carry a rejection")
            }
            Self::SampleRejectedWithoutRejection => {
                write!(f, "a rejected sample must carry its rejection")
            }
            Self::SampleDispositionRetryableMismatch {
                disposition,
                retryable,
            } => write!(
                f,
                "sample disposition {disposition:?} contradicts rejection retryable={retryable}"
            ),
            Self::RejectionRetryableMismatch {
                code,
                got,
                expected,
            } => write!(
                f,
                "rejection code {code:?} is {expected} by contract but the receipt flags it as {got}"
            ),
            Self::FieldTooLong { field, len, max } => {
                write!(f, "field {field} is {len} chars, contract limit is {max}")
            }
            Self::TooManyEntries { field, len, max } => {
                write!(
                    f,
                    "field {field} has {len} entries, contract limit is {max}"
                )
            }
        }
    }
}

impl std::error::Error for WireError {}
