//! Report Receipt: the Server's durable, exact acceptance record for one
//! AgentReport.
//!
//! The receipt is the terminal ACK for the whole immutable report:
//! - `accepted`/`partially_accepted`/`rejected` at the top;
//! - the Inventory carries a whole-set disposition only
//!   (`accepted`/`unchanged`/`rejected`) — a valid subset is never a new
//!   Inventory;
//! - Node current observations are accepted/rejected per Node;
//! - Block Summaries and History Gaps carry per-sample/range dispositions
//!   (`accepted`/`retryable_rejected`/`terminal_rejected`);
//! - every rejection carries a stable code, a `retryable` flag, and a reason.
//!
//! The Agent deletes the original report only after `apply_receipt`
//! committed in one transaction. Retrying the same `report_id` always
//! returns the first receipt.

use serde::{Deserialize, Serialize};

use crate::component::ComponentKey;
use crate::error::WireError;
use crate::hex::Sha256Hex;
use crate::identity::{NodeId, ReportId};
use crate::time::Rfc3339;

/// Top-level disposition of the whole report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptDisposition {
    /// The complete report was accepted as-is.
    Accepted,
    /// The report was committed, but some Nodes/samples were rejected.
    PartiallyAccepted,
    /// The report was not applied (envelope, protocol, or Inventory-level
    /// failure).
    Rejected,
}

/// Whole-Inventory disposition. Only one of these three, never per-Node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryDisposition {
    /// The Inventory was applied; lifecycle/ownership updated.
    Accepted,
    /// The Inventory content is unchanged since the last accepted report.
    Unchanged,
    /// The Inventory was rejected as a whole; the previous lifecycle is
    /// retained and no Node is retired/transferred because of it.
    Rejected,
}

/// Per-Node current observation disposition (atomic per Node).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeCurrentDisposition {
    /// The Node's current component observations were accepted.
    Accepted,
    /// The Node's current component observations were rejected.
    Rejected,
}

/// Per-sample/range disposition for Block Summaries and History Gaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleDispositionKind {
    /// The sample/range was applied.
    Accepted,
    /// Rejected but retryable: the Agent unbinds it from this report and
    /// re-queues it into a later report.
    RetryableRejected,
    /// Terminally rejected: the Agent records it in its rejection ledger and
    /// must not retry the identical sample.
    TerminalRejected,
}

/// The exact Report Receipt for one AgentReport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportReceipt {
    /// The report this receipt acknowledges (same as the request's
    /// `report_id`).
    pub report_id: ReportId,
    /// Whole-report disposition.
    pub disposition: ReceiptDisposition,
    /// SHA-256 of the exact report body bytes; a retry with the same
    /// `report_id` but different bytes is a protocol/security conflict.
    pub report_body_sha256: Sha256Hex,
    /// Server software version.
    pub server_version: String,
    /// Protocol majors the Server accepts.
    pub supported_protocol_majors: Vec<u64>,
    /// Server UTC time of the commit; used by the Agent to estimate clock
    /// skew. Agent liveness is derived from this value only.
    pub server_time: Rfc3339,
    /// Optional credential-rotation hint (opaque to v1 Agents).
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub rotation_hint: Option<String>,
    /// Whole-Inventory disposition; omitted when the report failed at
    /// envelope level (before Inventory validation).
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub inventory: Option<InventoryDisposition>,
    /// Envelope/Inventory-level rejections (stable code + retryable +
    /// reason). Node rejections live in `nodes[].rejections`, sample
    /// rejections in `samples[].rejection`.
    pub rejections: Vec<Rejection>,
    /// Per-Node current observation dispositions.
    pub nodes: Vec<NodeReceipt>,
    /// Per-sample/range dispositions for Block Summaries and History Gaps.
    pub samples: Vec<SampleDisposition>,
}

/// One Node's receipt entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeReceipt {
    /// The Node this entry belongs to.
    pub node_id: NodeId,
    /// Whether this Node's current observations were accepted.
    pub current: NodeCurrentDisposition,
    /// The component revisions accepted for this Node (state/value revision
    /// pairs), e.g. for `rpc`, `sync`, `consensus`, `network_identity`,
    /// `static_metadata`, `process`.
    pub accepted_component_revisions: Vec<ComponentRevision>,
    /// Stable rejections for this Node's current observations. Per-Node
    /// current rejections are terminal.
    pub rejections: Vec<Rejection>,
}

/// One accepted component revision pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentRevision {
    /// The component (stable lowercase key).
    pub component: ComponentKey,
    /// Accepted state revision.
    pub state_revision: u64,
    /// Accepted value revision.
    pub value_revision: u64,
}

/// Disposition of one Block Summary or History Gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleDisposition {
    /// The Node the sample belongs to.
    pub node_id: NodeId,
    /// Which sample/range this disposition refers to.
    pub sample: SampleRef,
    /// Accepted, retryable-rejected, or terminal-rejected.
    pub disposition: SampleDispositionKind,
    /// The rejection details; present iff `disposition != accepted`.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub rejection: Option<Rejection>,
}

/// Reference to one Block Summary or History Gap sample/range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SampleRef {
    /// One Block Summary at `height`.
    Block {
        /// Block height of the sample.
        height: u64,
    },
    /// One History Gap interval, inclusive.
    Gap {
        /// First height of the range.
        from_height: u64,
        /// Last height of the range.
        to_height: u64,
    },
}

/// A stable rejection: code + retryability + reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rejection {
    /// Stable lowercase rejection code (see [`RejectionCode`]).
    pub code: RejectionCode,
    /// Whether the rejected item may be re-queued into a later report.
    /// `true` only for codes that are safe to retry identically.
    pub retryable: bool,
    /// Bounded reason; never contains SQL, RPC URLs, credentials, or
    /// stack traces.
    pub reason: String,
}

/// Stable rejection codes of the v1 contract.
///
/// Unknown codes are rejected on parse: an Agent must never silently treat
/// an unrecognized disposition as success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionCode {
    /// Report's protocol major is not supported by the Server. Terminal.
    UnsupportedProtocolVersion,
    /// `protocol_version` is missing/zero. Terminal.
    InvalidProtocolVersion,
    /// The envelope is malformed or violates a global invariant. Terminal.
    InvalidEnvelope,
    /// The report belongs to a boot the Server already closed or never
    /// knew; only replay of an existing `report_id` is allowed. Terminal.
    StaleBoot,
    /// The same epoch/boot/sequence was submitted with different report
    /// ID/body (duplicate-agent conflict); also covers conflicting new
    /// boots. Terminal, security-relevant.
    ConflictingBoot,
    /// `boot_transition`/`previous_boot_id` combination is invalid.
    /// Terminal.
    InvalidBootTransition,
    /// A non-duplicate report regresses below the last accepted sequence.
    /// Terminal.
    StaleReport,
    /// The Inventory is structurally invalid. Terminal.
    InventoryInvalid,
    /// The Inventory lists the same Node twice. Terminal.
    InventoryDuplicateNode,
    /// The Inventory revision regressed or conflicts with accepted state.
    /// Terminal.
    InventoryRevisionConflict,
    /// The Inventory references a Network key the Server does not know.
    /// Terminal; the Server never auto-creates Networks.
    NetworkKeyUnknown,
    /// A Node current observation references a Node absent from the
    /// accepted Inventory. Terminal.
    NodeNotInInventory,
    /// The Node belongs to another Agent (ownership mismatch). Terminal;
    /// generates a security event.
    NodeOwnershipMismatch,
    /// The Node current observation violates a field/revision invariant.
    /// Terminal.
    NodeCurrentInvalid,
    /// Block history refused: observed Network Identity mismatches the
    /// registered Network. Current diagnostics may continue. Terminal.
    NetworkIdentityMismatch,
    /// `height <= historical_high_watermark` outside an open gap: plain
    /// resync replay, not written and not counted again. Terminal.
    ResyncReplay,
    /// A `gap_backfill` sample targets a height that is not inside an open
    /// `OpenRecoverableGap`. Terminal.
    GapBackfillOutsideOpenGap,
    /// A sample targets a `PermanentGap` range; late samples are never
    /// accepted there. Terminal.
    PermanentGapSample,
    /// The sample's hash/number/parentHash fails verification. Terminal.
    SampleHashMismatch,
    /// The History Gap declaration is invalid (reversed/overlapping/…).
    /// Terminal.
    HistoryGapInvalid,
    /// A `gap_backfill` sample arrived before its gap declaration was
    /// registered. Retryable: requeue into a later report.
    GapNotOpen,
    /// The Server is not ready to commit this report yet (transient).
    /// Retryable.
    ServerNotReady,
}

impl RejectionCode {
    /// Whether this code is, by contract, safe to retry identically.
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::GapNotOpen | Self::ServerNotReady)
    }
}

impl ReportReceipt {
    /// Validates the receipt's internal consistency so AgentStore can apply
    /// it deterministically: the retryable flag must match the code contract,
    /// accepted samples never carry rejections, rejected samples always do,
    /// and the top-level disposition must match the entries.
    pub fn validate(&self) -> Result<(), WireError> {
        for rejection in &self.rejections {
            check_retryable(rejection)?;
        }
        for node in &self.nodes {
            for rejection in &node.rejections {
                check_retryable(rejection)?;
            }
            if node.current == NodeCurrentDisposition::Rejected && node.rejections.is_empty() {
                return Err(WireError::NodeRejectedWithoutRejection);
            }
            if node.current == NodeCurrentDisposition::Accepted && !node.rejections.is_empty() {
                return Err(WireError::NodeAcceptedWithRejections);
            }
            if node.current == NodeCurrentDisposition::Rejected
                && !node.accepted_component_revisions.is_empty()
            {
                return Err(WireError::NodeRejectedWithAcceptedRevisions);
            }
            if node.current == NodeCurrentDisposition::Rejected
                && node.rejections.iter().any(|rejection| rejection.retryable)
            {
                return Err(WireError::NodeRejectedWithRetryableRejection);
            }
        }
        for sample in &self.samples {
            if let Some(rejection) = &sample.rejection {
                check_retryable(rejection)?;
            }
            match sample.disposition {
                SampleDispositionKind::Accepted => {
                    if sample.rejection.is_some() {
                        return Err(WireError::SampleAcceptedWithRejection);
                    }
                }
                SampleDispositionKind::RetryableRejected
                | SampleDispositionKind::TerminalRejected => {
                    let rejection = sample
                        .rejection
                        .as_ref()
                        .ok_or(WireError::SampleRejectedWithoutRejection)?;
                    let expected_retryable =
                        sample.disposition == SampleDispositionKind::RetryableRejected;
                    if rejection.retryable != expected_retryable {
                        return Err(WireError::SampleDispositionRetryableMismatch {
                            disposition: sample.disposition,
                            retryable: rejection.retryable,
                        });
                    }
                }
            }
        }
        match self.disposition {
            ReceiptDisposition::Accepted => {
                require_appliable_inventory(self, ReceiptDisposition::Accepted)?;
                if !self.rejections.is_empty() {
                    return Err(WireError::ReceiptAcceptedWithRejections);
                }
                if self.nodes.iter().any(|n| {
                    n.current == NodeCurrentDisposition::Rejected || !n.rejections.is_empty()
                }) {
                    return Err(WireError::ReceiptAcceptedWithRejectedNode);
                }
                if self
                    .samples
                    .iter()
                    .any(|s| s.disposition != SampleDispositionKind::Accepted)
                {
                    return Err(WireError::ReceiptAcceptedWithRejectedSample);
                }
            }
            ReceiptDisposition::PartiallyAccepted => {
                require_appliable_inventory(self, ReceiptDisposition::PartiallyAccepted)?;
                let nothing_rejected = self
                    .samples
                    .iter()
                    .all(|s| s.disposition == SampleDispositionKind::Accepted)
                    && self.nodes.iter().all(|n| {
                        n.current == NodeCurrentDisposition::Accepted && n.rejections.is_empty()
                    });
                if nothing_rejected {
                    return Err(WireError::ReceiptPartialWithoutRejection);
                }
            }
            ReceiptDisposition::Rejected => {
                if self.rejections.is_empty() {
                    return Err(WireError::ReceiptRejectedWithoutRejection);
                }
                if !self.nodes.is_empty() {
                    return Err(WireError::ReceiptRejectedWithNodeEntries);
                }
                if !self.samples.is_empty() {
                    return Err(WireError::ReceiptRejectedWithSampleEntries);
                }
                if let Some(disposition) = self.inventory {
                    if disposition != InventoryDisposition::Rejected {
                        return Err(WireError::ReceiptRejectedWithAcceptedInventory);
                    }
                }
            }
        }
        Ok(())
    }
}

fn check_retryable(rejection: &Rejection) -> Result<(), WireError> {
    let expected = rejection.code.is_retryable();
    if rejection.retryable != expected {
        return Err(WireError::RejectionRetryableMismatch {
            code: rejection.code,
            got: rejection.retryable,
            expected,
        });
    }
    Ok(())
}

/// `accepted`/`partially_accepted` receipts always carry an Inventory
/// disposition, and it must not be `rejected` (a rejected Inventory rejects
/// the whole report).
fn require_appliable_inventory(
    receipt: &ReportReceipt,
    disposition: ReceiptDisposition,
) -> Result<(), WireError> {
    match receipt.inventory {
        None => Err(WireError::ReceiptRequiresInventory { disposition }),
        Some(InventoryDisposition::Rejected) => {
            Err(WireError::ReceiptWithRejectedInventory { disposition })
        }
        Some(InventoryDisposition::Accepted | InventoryDisposition::Unchanged) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_disposition_wire_forms() {
        assert_eq!(
            serde_json::to_string(&ReceiptDisposition::PartiallyAccepted).unwrap(),
            "\"partially_accepted\""
        );
        assert_eq!(
            serde_json::to_string(&ReceiptDisposition::Rejected).unwrap(),
            "\"rejected\""
        );
        assert_eq!(
            serde_json::to_string(&ReceiptDisposition::Accepted).unwrap(),
            "\"accepted\""
        );
        assert_eq!(
            serde_json::to_string(&SampleDispositionKind::RetryableRejected).unwrap(),
            "\"retryable_rejected\""
        );
        assert_eq!(
            serde_json::to_string(&SampleDispositionKind::TerminalRejected).unwrap(),
            "\"terminal_rejected\""
        );
        assert_eq!(
            serde_json::to_string(&InventoryDisposition::Unchanged).unwrap(),
            "\"unchanged\""
        );
    }

    #[test]
    fn sample_ref_wire_forms() {
        assert_eq!(
            serde_json::to_string(&SampleRef::Block { height: 7 }).unwrap(),
            r#"{"kind":"block","height":7}"#
        );
        assert_eq!(
            serde_json::to_string(&SampleRef::Gap {
                from_height: 1,
                to_height: 3
            })
            .unwrap(),
            r#"{"kind":"gap","from_height":1,"to_height":3}"#
        );
    }

    #[test]
    fn retryable_codes_are_flagged() {
        assert!(RejectionCode::GapNotOpen.is_retryable());
        assert!(RejectionCode::ServerNotReady.is_retryable());
        assert!(!RejectionCode::StaleBoot.is_retryable());
        assert!(!RejectionCode::NodeOwnershipMismatch.is_retryable());
        assert!(!RejectionCode::ResyncReplay.is_retryable());
    }

    #[test]
    fn rejection_code_wire_forms() {
        assert_eq!(
            serde_json::to_string(&RejectionCode::UnsupportedProtocolVersion).unwrap(),
            "\"unsupported_protocol_version\""
        );
        assert_eq!(
            serde_json::to_string(&RejectionCode::GapBackfillOutsideOpenGap).unwrap(),
            "\"gap_backfill_outside_open_gap\""
        );
        assert_eq!(
            serde_json::to_string(&RejectionCode::NodeOwnershipMismatch).unwrap(),
            "\"node_ownership_mismatch\""
        );
        assert!(serde_json::from_str::<RejectionCode>("\"mystery_code\"").is_err());
    }

    fn accepted_receipt() -> ReportReceipt {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/receipt_v1_accepted.json"
        )))
        .unwrap()
    }

    #[test]
    fn accepted_fixture_validates() {
        assert_eq!(accepted_receipt().validate(), Ok(()));
    }

    #[test]
    fn retryable_flag_must_match_code_contract() {
        let mut receipt = accepted_receipt();
        receipt.rejections = vec![Rejection {
            code: RejectionCode::StaleBoot,
            retryable: true,
            reason: "nope".into(),
        }];
        assert_eq!(
            receipt.validate(),
            Err(WireError::RejectionRetryableMismatch {
                code: RejectionCode::StaleBoot,
                got: true,
                expected: false
            })
        );
    }

    #[test]
    fn accepted_receipt_rejects_any_rejection() {
        let mut receipt = accepted_receipt();
        receipt.nodes[0].rejections = vec![Rejection {
            code: RejectionCode::NodeCurrentInvalid,
            retryable: false,
            reason: "bad".into(),
        }];
        assert_eq!(
            receipt.validate(),
            Err(WireError::NodeAcceptedWithRejections)
        );

        let mut receipt = accepted_receipt();
        receipt.nodes[0].current = NodeCurrentDisposition::Rejected;
        assert_eq!(
            receipt.validate(),
            Err(WireError::NodeRejectedWithoutRejection)
        );

        let mut receipt = accepted_receipt();
        receipt.samples[0].disposition = SampleDispositionKind::TerminalRejected;
        receipt.samples[0].rejection = Some(Rejection {
            code: RejectionCode::SampleHashMismatch,
            retryable: false,
            reason: "bad".into(),
        });
        assert_eq!(
            receipt.validate(),
            Err(WireError::ReceiptAcceptedWithRejectedSample)
        );
    }

    #[test]
    fn accepted_receipt_requires_inventory() {
        let mut receipt = accepted_receipt();
        receipt.inventory = None;
        assert_eq!(
            receipt.validate(),
            Err(WireError::ReceiptRequiresInventory {
                disposition: ReceiptDisposition::Accepted
            })
        );
    }

    #[test]
    fn rejected_receipt_rules() {
        let mut receipt = accepted_receipt();
        receipt.disposition = ReceiptDisposition::Rejected;
        receipt.rejections = vec![Rejection {
            code: RejectionCode::InvalidEnvelope,
            retryable: false,
            reason: "bad".into(),
        }];
        receipt.inventory = None;
        receipt.nodes.clear();
        receipt.samples.clear();
        assert_eq!(receipt.validate(), Ok(()));

        // A rejected receipt cannot carry accepted inventory.
        let mut receipt = accepted_receipt();
        receipt.disposition = ReceiptDisposition::Rejected;
        receipt.rejections = vec![Rejection {
            code: RejectionCode::InvalidEnvelope,
            retryable: false,
            reason: "bad".into(),
        }];
        receipt.nodes.clear();
        receipt.samples.clear();
        assert_eq!(
            receipt.validate(),
            Err(WireError::ReceiptRejectedWithAcceptedInventory)
        );

        // A rejected receipt must carry a rejection.
        let mut receipt = accepted_receipt();
        receipt.disposition = ReceiptDisposition::Rejected;
        receipt.inventory = None;
        receipt.rejections.clear();
        receipt.nodes.clear();
        receipt.samples.clear();
        assert_eq!(
            receipt.validate(),
            Err(WireError::ReceiptRejectedWithoutRejection)
        );
    }

    #[test]
    fn partially_accepted_requires_a_rejection() {
        let mut receipt = accepted_receipt();
        receipt.disposition = ReceiptDisposition::PartiallyAccepted;
        assert_eq!(
            receipt.validate(),
            Err(WireError::ReceiptPartialWithoutRejection)
        );
    }

    #[test]
    fn sample_disposition_rejection_consistency() {
        let mut receipt = accepted_receipt();
        receipt.samples[0].disposition = SampleDispositionKind::TerminalRejected;
        assert_eq!(
            receipt.validate(),
            Err(WireError::SampleRejectedWithoutRejection)
        );

        let mut receipt = accepted_receipt();
        receipt.samples[0].rejection = Some(Rejection {
            code: RejectionCode::GapNotOpen,
            retryable: true,
            reason: "later".into(),
        });
        assert_eq!(
            receipt.validate(),
            Err(WireError::SampleAcceptedWithRejection)
        );
    }
}
