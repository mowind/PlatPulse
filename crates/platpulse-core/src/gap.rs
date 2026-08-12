//! History Gap: an explicit interval for which time-series samples were lost
//! or intentionally dropped, while a later current-state report may still be
//! authoritative.
//!
//! Gaps are inclusive intervals (`from_height..=to_height`). Terminal
//! `server_rejected` samples are reported back as a `server_rejected` gap so
//! a rejection is never silently lost.

use serde::{Deserialize, Serialize};

use crate::identity::NodeId;
use crate::time::Rfc3339;

/// Why a History Gap exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// Backfill exceeded its bounded height/count/time limit (or the Agent
    /// restarted across an unrecoverable span); collection resumes from the
    /// current head.
    UnrecoverableBackfill,
    /// The spool capacity cleanup dropped the samples covering this range.
    SpoolOverflow,
    /// The Server terminally rejected the samples in this range; the Agent
    /// reports the resulting gap back so the Server records it too.
    ServerRejected,
}

/// An explicit lost/backfillable interval for one Node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryGap {
    /// The Node whose history is affected (must be in the report Inventory).
    pub node_id: NodeId,
    /// Why the samples are missing.
    pub kind: GapKind,
    /// First missing height (inclusive).
    pub from_height: u64,
    /// Last missing height (inclusive); must be >= `from_height`.
    pub to_height: u64,
    /// Bounded human-readable reason (contract limit: 512 chars).
    pub reason: String,
    /// When the Agent recorded the gap.
    pub recorded_at: Rfc3339,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_kind_wire_forms() {
        assert_eq!(
            serde_json::to_string(&GapKind::UnrecoverableBackfill).unwrap(),
            "\"unrecoverable_backfill\""
        );
        assert_eq!(
            serde_json::to_string(&GapKind::SpoolOverflow).unwrap(),
            "\"spool_overflow\""
        );
        assert_eq!(
            serde_json::to_string(&GapKind::ServerRejected).unwrap(),
            "\"server_rejected\""
        );
    }
}
