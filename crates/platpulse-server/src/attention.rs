//! Server-owned Agent Attention occurrence/evidence boundary and shared
//! Owner Acknowledgment (main design §15.6, webui.md §15.3, issue #172).
//!
//! An Attention Item is a current Server-derived prompt, not an Alert
//! Incident or a browser-computed warning. The stable `kind + subject` pair
//! only names the problem type and subject; it cannot distinguish "the same
//! occurrence" from new evidence or a later recurrence. This module derives
//! a Server-owned evidence boundary for every Agent Attention Item and
//! stores the Owner's durable, shared Acknowledgment against that boundary.
//!
//! Consequences:
//!
//! * An acknowledged occurrence stays suppressed across ordinary refreshes,
//!   unchanged reports, a second Owner, another login session, and a Server
//!   restart, because the boundary is persisted in SQLite.
//! * New evidence (a higher gap/security/drop count, a changed shutdown
//!   state, a fresh offline episode) or a genuine recovery followed by a
//!   recurrence produces a different boundary, so the item is not swallowed.
//! * Absent (cleared) items proactively retire their stale boundary rows so a
//!   later recurrence cannot be mistaken for the occurrence the Owner saw.
//! * Nothing here changes liveness, health, diagnostics, Alert Incidents, or
//!   notification policy: acknowledgment only removes the prompt.
//!
//! The acknowledgment mutation carries the evidence boundary the Owner
//! actually saw and only applies it while that boundary is still current, so
//! a stale request can never swallow evidence that arrived after the Owner's
//! snapshot.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

/// The Server-owned Agent Attention kinds (design §8.4.2). Node, Network, and
/// Settings kinds are never acknowledgeable.
#[derive(
    Debug,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    AgentOffline,
    AgentSpoolFatal,
    AgentSpoolOverflow,
    AgentReportGap,
    AgentSecurityEvent,
    AgentShutdownIncomplete,
    AgentInventoryRejected,
    NodeUnhealthy,
    NodeHealthUnknown,
    NodeResync,
    NodeIdentityMismatch,
}

impl AttentionKind {
    /// Stable wire name, matching the snake_case serde representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentOffline => "agent_offline",
            Self::AgentSpoolFatal => "agent_spool_fatal",
            Self::AgentSpoolOverflow => "agent_spool_overflow",
            Self::AgentReportGap => "agent_report_gap",
            Self::AgentSecurityEvent => "agent_security_event",
            Self::AgentShutdownIncomplete => "agent_shutdown_incomplete",
            Self::AgentInventoryRejected => "agent_inventory_rejected",
            Self::NodeUnhealthy => "node_unhealthy",
            Self::NodeHealthUnknown => "node_health_unknown",
            Self::NodeResync => "node_resync",
            Self::NodeIdentityMismatch => "node_identity_mismatch",
        }
    }

    /// The seven kinds that carry a Server-owned evidence boundary and can be
    /// acknowledged by an Owner.
    pub fn is_agent_acknowledgeable(self) -> bool {
        matches!(
            self,
            Self::AgentOffline
                | Self::AgentSpoolFatal
                | Self::AgentSpoolOverflow
                | Self::AgentReportGap
                | Self::AgentSecurityEvent
                | Self::AgentShutdownIncomplete
                | Self::AgentInventoryRejected
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttentionSeverity {
    Critical,
    Warning,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttentionSubjectKind {
    Agent,
    Node,
    Network,
    Settings,
}

impl AttentionSubjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Node => "node",
            Self::Network => "network",
            Self::Settings => "settings",
        }
    }
}

/// One Server-owned Attention Item (design §8.4.2, §15.6).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AttentionItem {
    /// Stable item key (kind + subject) for list rendering and tests.
    pub id: String,
    pub kind: AttentionKind,
    pub severity: AttentionSeverity,
    pub subject_kind: AttentionSubjectKind,
    pub subject_id: String,
    pub subject_label: String,
    pub message: String,
    /// Last authoritative observation time for this item. None means the
    /// Server has no observation timestamp for the evidence; it is never
    /// replaced with the snapshot generation time, which is not an event or
    /// observation time.
    #[schema(required = true)]
    pub observed_at: Option<String>,
    /// Server-owned occurrence/evidence boundary. An Owner acknowledgment
    /// echoes this value back and applies only while it is still current, so
    /// new evidence is never swallowed by a stale confirmation. The WebUI
    /// treats it as opaque.
    pub evidence_key: String,
}

impl AttentionItem {
    fn agent(
        agent_id: &str,
        kind: AttentionKind,
        severity: AttentionSeverity,
        message: String,
        observed_at: Option<String>,
        evidence_key: String,
    ) -> Self {
        Self {
            id: format!("{}:agent:{agent_id}", kind.as_str()),
            kind,
            severity,
            subject_kind: AttentionSubjectKind::Agent,
            subject_id: agent_id.to_owned(),
            subject_label: agent_id.to_owned(),
            message,
            observed_at,
            evidence_key,
        }
    }
}

/// Liveness rule shared by the diagnostics list, the overview, and the
/// attention boundary: an Agent is online only when a report arrived within
/// the offline window; a never-observed Agent is unknown, never offline.
pub fn agent_liveness(last_received_at: Option<&str>) -> &'static str {
    last_received_at
        .and_then(crate::auth::parse_rfc3339)
        .map(|received| {
            if (crate::auth::now_utc() - received).whole_seconds()
                <= crate::http::agent::AGENT_OFFLINE_AFTER_SECONDS
            {
                "online"
            } else {
                "offline"
            }
        })
        .unwrap_or("unknown")
}

/// The Agent evidence needed to derive its acknowledgeable Attention Items.
/// It is deliberately separate from the Admin DTOs so the boundary can be
/// recomputed identically by the overview, the Agent detail, and the
/// acknowledgment mutation.
#[derive(Debug, Clone)]
pub struct AgentAttentionEvidence {
    pub agent_id: String,
    pub last_received_at: Option<String>,
    pub shutdown_state: String,
    pub shutdown_updated_at: Option<String>,
    pub security_event_count: i64,
    pub sequence_gap_count: i64,
    pub latest_gap_at: Option<String>,
    pub spool_store_fatal: bool,
    pub spool_dropped_sequence_to: Option<i64>,
    pub host_updated_at: Option<String>,
    /// The Agent's latest stored Report Receipt, whatever its disposition.
    pub latest_receipt_disposition: Option<String>,
    /// Evidence of that receipt when it was a rejection (issue #181): the
    /// deciding code, and the Inventory the Agent declared in the refused
    /// report. All NULL for receipts stored before the evidence columns
    /// existed.
    pub latest_rejection_code: Option<String>,
    pub latest_rejection_inventory_revision: Option<i64>,
    pub latest_rejection_inventory_sha256: Option<String>,
    /// Protocol major of the refused declaration (issue #188): 2 for a
    /// Server-managed v2 declaration that carries no Agent revision, 1 for a
    /// frozen v1 declaration, NULL for evidence recorded before this column.
    pub latest_rejection_inventory_protocol_major: Option<i64>,
    pub latest_receipt_at: Option<String>,
    /// The Inventory the Server currently accepts (last accepted revision and
    /// its content hash), from `agents`.
    pub accepted_inventory_revision: i64,
    pub accepted_inventory_sha256: Option<String>,
}

/// One row of the shared Agent Attention source query.
#[derive(Debug, Clone, FromRow)]
pub struct AgentAttentionRow {
    pub agent_id: String,
    pub last_received_at: Option<String>,
    pub shutdown_updated_at: Option<String>,
    pub shutdown_state: String,
    pub security_event_count: i64,
    pub sequence_gap_count: i64,
    pub latest_gap_at: Option<String>,
    pub spool_store_fatal: Option<i64>,
    pub spool_dropped_sequence_to: Option<i64>,
    pub host_updated_at: Option<String>,
    pub accepted_inventory_revision: i64,
    pub accepted_inventory_sha256: Option<String>,
    pub latest_receipt_disposition: Option<String>,
    pub latest_rejection_code: Option<String>,
    pub latest_rejection_inventory_revision: Option<i64>,
    pub latest_rejection_inventory_sha256: Option<String>,
    pub latest_rejection_inventory_protocol_major: Option<i64>,
    pub latest_receipt_at: Option<String>,
}

/// Shared projection read by the overview and the Agent detail. Live Agents
/// only: a removed Agent has no current Attention.
///
/// The correlated subquery picks the Agent's newest receipt by
/// `(received_at, report_sequence)`, matching the receipt table's index, so
/// the Inventory rejection boundary always reflects the latest ingestion
/// attempt rather than an arbitrary historical rejection.
pub const AGENT_ATTENTION_SELECT: &str = "SELECT a.agent_id, a.last_received_at, a.shutdown_updated_at, a.shutdown_state, a.security_event_count, (SELECT COUNT(*) FROM report_sequence_gaps g WHERE g.agent_id = a.agent_id) AS sequence_gap_count, (SELECT MAX(g.created_at) FROM report_sequence_gaps g WHERE g.agent_id = a.agent_id) AS latest_gap_at, h.spool_store_fatal, h.spool_dropped_sequence_to, h.updated_at AS host_updated_at, a.last_inventory_revision AS accepted_inventory_revision, a.inventory_sha256 AS accepted_inventory_sha256, r.disposition AS latest_receipt_disposition, r.rejection_code AS latest_rejection_code, r.inventory_revision AS latest_rejection_inventory_revision, r.inventory_sha256 AS latest_rejection_inventory_sha256, r.inventory_protocol_major AS latest_rejection_inventory_protocol_major, r.received_at AS latest_receipt_at FROM agents a LEFT JOIN current_host_observations h ON h.agent_id = a.agent_id LEFT JOIN agent_report_receipts r ON r.report_id = (SELECT r2.report_id FROM agent_report_receipts r2 WHERE r2.agent_id = a.agent_id ORDER BY r2.received_at DESC, r2.report_sequence DESC LIMIT 1) WHERE a.deleted_at IS NULL";

impl AgentAttentionRow {
    /// Every acknowledgeable Attention Item currently present for this Agent,
    /// each carrying its Server-owned evidence boundary.
    pub fn items(&self) -> Vec<AttentionItem> {
        agent_attention_items(&AgentAttentionEvidence {
            agent_id: self.agent_id.clone(),
            last_received_at: self.last_received_at.clone(),
            shutdown_state: self.shutdown_state.clone(),
            shutdown_updated_at: self.shutdown_updated_at.clone(),
            security_event_count: self.security_event_count,
            sequence_gap_count: self.sequence_gap_count,
            latest_gap_at: self.latest_gap_at.clone(),
            spool_store_fatal: self.spool_store_fatal.is_some_and(|value| value != 0),
            spool_dropped_sequence_to: self.spool_dropped_sequence_to,
            host_updated_at: self.host_updated_at.clone(),
            latest_receipt_disposition: self.latest_receipt_disposition.clone(),
            latest_rejection_code: self.latest_rejection_code.clone(),
            latest_rejection_inventory_revision: self.latest_rejection_inventory_revision,
            latest_rejection_inventory_sha256: self.latest_rejection_inventory_sha256.clone(),
            latest_rejection_inventory_protocol_major: self
                .latest_rejection_inventory_protocol_major,
            latest_receipt_at: self.latest_receipt_at.clone(),
            accepted_inventory_revision: self.accepted_inventory_revision,
            accepted_inventory_sha256: self.accepted_inventory_sha256.clone(),
        })
    }
}

/// Derive the current acknowledgeable Attention Items and their evidence
/// boundaries from authoritative Agent evidence. This is pure so every
/// surface computes the identical boundary.
pub fn agent_attention_items(evidence: &AgentAttentionEvidence) -> Vec<AttentionItem> {
    let agent_id = evidence.agent_id.as_str();
    let mut items = Vec::new();
    if agent_liveness(evidence.last_received_at.as_deref()) == "offline" {
        items.push(AttentionItem::agent(
            agent_id,
            AttentionKind::AgentOffline,
            AttentionSeverity::Warning,
            "the Agent has not reported within the liveness window".to_owned(),
            evidence.last_received_at.clone(),
            format!(
                "agent_offline:last_received={}",
                evidence.last_received_at.as_deref().unwrap_or("")
            ),
        ));
    }
    if evidence.spool_store_fatal {
        items.push(AttentionItem::agent(
            agent_id,
            AttentionKind::AgentSpoolFatal,
            AttentionSeverity::Critical,
            "the Agent spool store is in a fatal state; durable reports are at risk".to_owned(),
            evidence.host_updated_at.clone(),
            "agent_spool_fatal:store_fatal".to_owned(),
        ));
    }
    if let Some(dropped_to) = evidence.spool_dropped_sequence_to {
        items.push(AttentionItem::agent(
            agent_id,
            AttentionKind::AgentSpoolOverflow,
            AttentionSeverity::Critical,
            "the Agent spool overflowed and discarded queued reports".to_owned(),
            evidence.host_updated_at.clone(),
            format!("agent_spool_overflow:dropped_to={dropped_to}"),
        ));
    }
    if evidence.sequence_gap_count > 0 {
        items.push(AttentionItem::agent(
            agent_id,
            AttentionKind::AgentReportGap,
            AttentionSeverity::Warning,
            format!(
                "{} report sequence gap{} recorded",
                evidence.sequence_gap_count,
                if evidence.sequence_gap_count == 1 {
                    " was"
                } else {
                    "s were"
                }
            ),
            evidence.latest_gap_at.clone(),
            format!(
                "agent_report_gap:count={}:latest={}",
                evidence.sequence_gap_count,
                evidence.latest_gap_at.as_deref().unwrap_or("")
            ),
        ));
    }
    if evidence.security_event_count > 0 {
        items.push(AttentionItem::agent(
            agent_id,
            AttentionKind::AgentSecurityEvent,
            AttentionSeverity::Critical,
            format!(
                "{} security event{} recorded",
                evidence.security_event_count,
                if evidence.security_event_count == 1 {
                    " was"
                } else {
                    "s were"
                }
            ),
            // The accumulated counter has no per-event timestamp; an
            // unrelated later report must not make it look recent.
            None,
            format!(
                "agent_security_event:count={}",
                evidence.security_event_count
            ),
        ));
    }
    if matches!(
        evidence.shutdown_state.as_str(),
        "stopping" | "draining" | "send_failed" | "forced_kill_recovery"
    ) {
        items.push(AttentionItem::agent(
            agent_id,
            AttentionKind::AgentShutdownIncomplete,
            AttentionSeverity::Warning,
            format!("the Agent shutdown is {}", evidence.shutdown_state),
            evidence.shutdown_updated_at.clone(),
            format!(
                "agent_shutdown_incomplete:state={}:updated={}",
                evidence.shutdown_state,
                evidence.shutdown_updated_at.as_deref().unwrap_or("")
            ),
        ));
    }
    // An Inventory rejection is terminal for the whole report (issue #177):
    // the Agent keeps reporting on its own clock while the Server refuses
    // every report, so the projections freeze with no explanation on the
    // Agent console and nothing on the Admin surfaces. This item is derived
    // from the Agent's newest stored receipt, so a later accepted report
    // clears it without an explicit retirement, and the evidence boundary
    // keeps an identical repeated rejection from re-arming an acknowledgment
    // while genuinely new content does not.
    if let Some(code) = inventory_rejection_code(evidence) {
        let accepted_revision = evidence.accepted_inventory_revision;
        let accepted_hash = evidence.accepted_inventory_sha256.as_deref().unwrap_or("");
        let reported_revision = evidence.latest_rejection_inventory_revision;
        let reported_hash = evidence
            .latest_rejection_inventory_sha256
            .as_deref()
            .unwrap_or("");
        // The refusal is explained from the Server-accepted pair and the actual
        // refusal evidence (issue #188). A v2 declaration carries no
        // Agent-supplied revision, so the remedy must never tell the Owner to
        // bump one that no longer exists.
        let declaration = declaration_kind(evidence.latest_rejection_inventory_protocol_major);
        let message = match (code, declaration, reported_revision) {
            ("network_key_unknown", _, _) => {
                "the declared Node Inventory references a Network key the Server does not know"
                    .to_owned()
            }
            (_, "server_managed", _) => {
                "the Server-assigned Node Inventory revision range is exhausted; no declaration change can be accepted"
                    .to_owned()
            }
            (_, _, Some(reported)) if reported < accepted_revision => format!(
                "the Agent declares Node Inventory revision {reported}, below the accepted revision {accepted_revision}; bump inventory_revision to declare the current Node set"
            ),
            (_, _, Some(reported)) => format!(
                "Node Inventory content changed while inventory_revision stayed {reported}; bump inventory_revision to declare the new Node set"
            ),
            (_, _, None) => {
                "the declared Node Inventory conflicts with the Inventory the Server accepts"
                    .to_owned()
            }
        };
        items.push(AttentionItem::agent(
            agent_id,
            AttentionKind::AgentInventoryRejected,
            AttentionSeverity::Critical,
            message,
            evidence.latest_receipt_at.clone(),
            format!(
                "agent_inventory_rejected:code={code}:accepted={accepted_revision}:{accepted_hash}:reported={}:{reported_hash}",
                reported_revision.map(|value| value.to_string()).unwrap_or_default()
            ),
        ));
    }
    items
}

/// Whether a whole-report rejection code is caused by the declared Inventory
/// (issue #181). These are the refusals the Agent Inventory rejection Attention
/// Item and the Admin Inventory diagnosis both speak to: the report was
/// refused because its Inventory conflicts with accepted state, or because it
/// names a Network key the Server does not know.
pub fn is_inventory_rejection_code(code: &str) -> bool {
    matches!(code, "inventory_revision_conflict" | "network_key_unknown")
}

/// The Server-owned name of the protocol that produced a declaration (issue
/// #188): v2 is `server_managed` because the Server assigns the revision, v1 is
/// `agent_declared` because the Agent supplies it, and NULL is `unknown` for
/// evidence recorded before the protocol column existed. It is derived only
/// from Server-stored evidence, never from an Agent-reported diagnostic.
pub fn declaration_kind(protocol_major: Option<i64>) -> &'static str {
    match protocol_major {
        Some(major) if major == platpulse_core::protocol::PROTOCOL_VERSION_V2 as i64 => {
            "server_managed"
        }
        Some(major) if major == platpulse_core::protocol::PROTOCOL_VERSION as i64 => {
            "agent_declared"
        }
        _ => "unknown",
    }
}

/// The deciding rejection code when the Agent's newest stored receipt was a
/// whole-report Inventory rejection, i.e. when the report the Agent is still
/// trying to deliver was refused because of its Inventory.
///
/// `inventory_revision_conflict` covers both changed content at the accepted
/// revision and a regressed revision.
fn inventory_rejection_code(evidence: &AgentAttentionEvidence) -> Option<&str> {
    if evidence.latest_receipt_disposition.as_deref() != Some("rejected") {
        return None;
    }
    let code = evidence.latest_rejection_code.as_deref()?;
    is_inventory_rejection_code(code).then_some(code)
}

#[derive(Debug, Clone, FromRow)]
struct AcknowledgmentRow {
    kind: String,
    evidence_key: String,
}

/// One Item reference the Owner echoed back. The boundary is opaque to the
/// browser; the Server only needs to match it against the current evidence.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct AttentionAcknowledgment {
    pub kind: AttentionKind,
    pub evidence_key: String,
}

/// The outcome of an Owner acknowledgment request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcknowledgeOutcome {
    Applied {
        acknowledged: Vec<AttentionAcknowledgment>,
        skipped: Vec<AttentionAcknowledgment>,
    },
    AgentNotFound,
}

/// Evaluate the unacknowledged Attention Items for one Agent and retire any
/// acknowledgment whose occurrence has cleared or been superseded.
///
/// A stale row is removed because it names an occurrence that is no longer
/// current: keeping it would let a later recurrence with the same boundary
/// (for example the same shutdown state after a genuine recovery) be mistaken
/// for the occurrence the Owner already reviewed.
pub async fn evaluate_agent(
    pool: &SqlitePool,
    row: &AgentAttentionRow,
) -> Result<Vec<AttentionItem>, sqlx::Error> {
    let items = row.items();
    let acknowledgments = sqlx::query_as::<_, AcknowledgmentRow>(
        "SELECT kind, evidence_key FROM agent_attention_acknowledgments WHERE agent_id = ?",
    )
    .bind(&row.agent_id)
    .fetch_all(pool)
    .await?;
    let current: BTreeSet<(String, String)> = items
        .iter()
        .map(|item| (item.kind.as_str().to_owned(), item.evidence_key.clone()))
        .collect();
    for acknowledgment in &acknowledgments {
        if !current.contains(&(
            acknowledgment.kind.clone(),
            acknowledgment.evidence_key.clone(),
        )) {
            sqlx::query(
                "DELETE FROM agent_attention_acknowledgments WHERE agent_id = ? AND kind = ? AND evidence_key = ?",
            )
            .bind(&row.agent_id)
            .bind(&acknowledgment.kind)
            .bind(&acknowledgment.evidence_key)
            .execute(pool)
            .await?;
        }
    }
    let acknowledged: BTreeSet<(String, String)> = acknowledgments
        .iter()
        .map(|acknowledgment| {
            (
                acknowledgment.kind.clone(),
                acknowledgment.evidence_key.clone(),
            )
        })
        .collect();
    Ok(items
        .into_iter()
        .filter(|item| {
            !acknowledged.contains(&(item.kind.as_str().to_owned(), item.evidence_key.clone()))
        })
        .collect())
}

/// Apply an Owner acknowledgment to exactly the Agent evidence boundaries the
/// Owner echoed back. A boundary that is no longer current is skipped rather
/// than applied, so a stale request cannot swallow newer unseen evidence.
/// The Audit Event records which boundaries were acknowledged and which were
/// skipped; no raw diagnostic payload is stored.
pub async fn acknowledge(
    pool: &SqlitePool,
    agent_id: &str,
    requested: &[AttentionAcknowledgment],
    actor_user_id: Option<&str>,
) -> Result<AcknowledgeOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as::<_, AgentAttentionRow>(&format!(
        "{AGENT_ATTENTION_SELECT} AND a.agent_id = ?"
    ))
    .bind(agent_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(AcknowledgeOutcome::AgentNotFound);
    };
    let current: BTreeSet<(String, String)> = row
        .items()
        .iter()
        .map(|item| (item.kind.as_str().to_owned(), item.evidence_key.clone()))
        .collect();
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let mut acknowledged = Vec::new();
    let mut skipped = Vec::new();
    for request in requested {
        if current.contains(&(
            request.kind.as_str().to_owned(),
            request.evidence_key.clone(),
        )) {
            sqlx::query(
                "INSERT INTO agent_attention_acknowledgments (agent_id, kind, evidence_key, acknowledged_at, acknowledged_by_user_id) VALUES (?, ?, ?, ?, ?) ON CONFLICT(agent_id, kind, evidence_key) DO UPDATE SET acknowledged_at = excluded.acknowledged_at, acknowledged_by_user_id = excluded.acknowledged_by_user_id",
            )
            .bind(agent_id)
            .bind(request.kind.as_str())
            .bind(&request.evidence_key)
            .bind(&now)
            .bind(actor_user_id)
            .execute(&mut *tx)
            .await?;
            acknowledged.push(request.clone());
        } else {
            skipped.push(request.clone());
        }
    }
    if !acknowledged.is_empty() {
        let after = serde_json::json!({
            "acknowledged": acknowledged,
            "skipped": skipped,
        });
        crate::auth::insert_audit_change(
            &mut *tx,
            actor_user_id,
            "agent_attention_acknowledged",
            "agent",
            agent_id,
            None,
            Some(&after),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(AcknowledgeOutcome::Applied {
        acknowledged,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(agent_id: &str) -> AgentAttentionEvidence {
        AgentAttentionEvidence {
            agent_id: agent_id.to_owned(),
            last_received_at: None,
            shutdown_state: "running".to_owned(),
            shutdown_updated_at: None,
            security_event_count: 0,
            sequence_gap_count: 0,
            latest_gap_at: None,
            spool_store_fatal: false,
            spool_dropped_sequence_to: None,
            host_updated_at: None,
            latest_receipt_disposition: None,
            latest_rejection_code: None,
            latest_rejection_inventory_revision: None,
            latest_rejection_inventory_sha256: None,
            latest_rejection_inventory_protocol_major: None,
            latest_receipt_at: None,
            accepted_inventory_revision: 0,
            accepted_inventory_sha256: None,
        }
    }

    fn stale() -> String {
        crate::auth::format_rfc3339(crate::auth::now_utc() - time::Duration::hours(1))
    }

    fn key(items: &[AttentionItem], kind: AttentionKind) -> String {
        items
            .iter()
            .find(|item| item.kind == kind)
            .unwrap_or_else(|| panic!("missing {kind:?}"))
            .evidence_key
            .clone()
    }

    #[test]
    fn never_observed_agent_has_no_offline_prompt() {
        let items = agent_attention_items(&evidence("agent-never"));
        assert!(items.is_empty(), "unknown liveness is not Agent Offline");
    }

    #[test]
    fn offline_boundary_tracks_the_episode() {
        let mut first = evidence("agent-offline");
        first.last_received_at = Some(stale());
        let first_items = agent_attention_items(&first);
        assert_eq!(
            key(&first_items, AttentionKind::AgentOffline),
            format!(
                "agent_offline:last_received={}",
                first.last_received_at.clone().unwrap()
            )
        );

        // A fresh report is genuine recovery (no prompt), and a later offline
        // episode has a different boundary, so the earlier acknowledgment
        // cannot swallow it.
        let mut online = evidence("agent-offline");
        online.last_received_at = Some(crate::auth::format_rfc3339(crate::auth::now_utc()));
        assert!(agent_attention_items(&online).is_empty());
        let mut later = evidence("agent-offline");
        later.last_received_at = Some(crate::auth::format_rfc3339(
            crate::auth::now_utc() - time::Duration::hours(2),
        ));
        assert_ne!(
            key(&first_items, AttentionKind::AgentOffline),
            key(&agent_attention_items(&later), AttentionKind::AgentOffline)
        );
    }

    #[test]
    fn new_counter_evidence_changes_the_boundary() {
        let mut before = evidence("agent-events");
        before.security_event_count = 2;
        let mut after = evidence("agent-events");
        after.security_event_count = 3;
        assert_ne!(
            key(
                &agent_attention_items(&before),
                AttentionKind::AgentSecurityEvent
            ),
            key(
                &agent_attention_items(&after),
                AttentionKind::AgentSecurityEvent
            )
        );
    }

    #[test]
    fn inventory_rejection_is_critical_for_both_inventory_codes() {
        for code in ["inventory_revision_conflict", "network_key_unknown"] {
            let mut row = evidence("agent-inventory");
            row.latest_receipt_disposition = Some("rejected".to_owned());
            row.latest_rejection_code = Some(code.to_owned());
            row.latest_rejection_inventory_revision = Some(4);
            row.latest_rejection_inventory_sha256 = Some("reported-hash".to_owned());
            row.latest_receipt_at = Some(crate::auth::format_rfc3339(crate::auth::now_utc()));
            row.accepted_inventory_revision = 4;
            row.accepted_inventory_sha256 = Some("accepted-hash".to_owned());
            let items = agent_attention_items(&row);
            let item = items
                .iter()
                .find(|item| item.kind == AttentionKind::AgentInventoryRejected)
                .unwrap_or_else(|| panic!("missing AgentInventoryRejected for {code}"));
            assert_eq!(item.severity, AttentionSeverity::Critical, "{code}");
            assert!(AttentionKind::AgentInventoryRejected.is_agent_acknowledgeable());
            assert_eq!(item.observed_at, row.latest_receipt_at);
        }
    }

    #[test]
    fn no_inventory_rejection_item_without_inventory_rejection_evidence() {
        let mut accepted = evidence("agent-accepted");
        accepted.latest_receipt_disposition = Some("accepted".to_owned());
        // A stale rejection code on an accepted receipt is not a rejection.
        accepted.latest_rejection_code = Some("inventory_revision_conflict".to_owned());
        assert!(
            agent_attention_items(&accepted)
                .iter()
                .all(|item| item.kind != AttentionKind::AgentInventoryRejected)
        );

        let mut non_inventory = evidence("agent-stale");
        non_inventory.latest_receipt_disposition = Some("rejected".to_owned());
        non_inventory.latest_rejection_code = Some("stale_report".to_owned());
        assert!(
            agent_attention_items(&non_inventory)
                .iter()
                .all(|item| item.kind != AttentionKind::AgentInventoryRejected)
        );

        // A receipt stored before migration 0055 has no code at all.
        let mut pre_migration = evidence("agent-pre-migration");
        pre_migration.latest_receipt_disposition = Some("rejected".to_owned());
        assert!(
            agent_attention_items(&pre_migration)
                .iter()
                .all(|item| item.kind != AttentionKind::AgentInventoryRejected)
        );
    }

    #[test]
    fn inventory_rejection_evidence_key_tracks_reported_and_accepted_inventory() {
        let inventory_rejection = || {
            let mut row = evidence("agent-inventory");
            row.latest_receipt_disposition = Some("rejected".to_owned());
            row.latest_rejection_code = Some("inventory_revision_conflict".to_owned());
            row.latest_rejection_inventory_revision = Some(7);
            row.latest_rejection_inventory_sha256 = Some("reported-hash".to_owned());
            row.latest_receipt_at = Some("2026-08-12T08:00:00Z".to_owned());
            row.accepted_inventory_revision = 7;
            row.accepted_inventory_sha256 = Some("accepted-hash".to_owned());
            row
        };
        let baseline = key(
            &agent_attention_items(&inventory_rejection()),
            AttentionKind::AgentInventoryRejected,
        );
        // An identical repeated rejection must not re-arm an acknowledgment.
        assert_eq!(
            baseline,
            key(
                &agent_attention_items(&inventory_rejection()),
                AttentionKind::AgentInventoryRejected
            )
        );

        let mut reported_revision = inventory_rejection();
        reported_revision.latest_rejection_inventory_revision = Some(8);
        assert_ne!(
            baseline,
            key(
                &agent_attention_items(&reported_revision),
                AttentionKind::AgentInventoryRejected
            )
        );

        let mut reported_hash = inventory_rejection();
        reported_hash.latest_rejection_inventory_sha256 = Some("other-reported-hash".to_owned());
        assert_ne!(
            baseline,
            key(
                &agent_attention_items(&reported_hash),
                AttentionKind::AgentInventoryRejected
            )
        );

        let mut accepted_revision = inventory_rejection();
        accepted_revision.accepted_inventory_revision = 8;
        assert_ne!(
            baseline,
            key(
                &agent_attention_items(&accepted_revision),
                AttentionKind::AgentInventoryRejected
            )
        );

        let mut accepted_hash = inventory_rejection();
        accepted_hash.accepted_inventory_sha256 = Some("other-accepted-hash".to_owned());
        assert_ne!(
            baseline,
            key(
                &agent_attention_items(&accepted_hash),
                AttentionKind::AgentInventoryRejected
            )
        );
    }

    #[test]
    fn inventory_rejection_message_names_the_remedy() {
        let message = |accepted: i64, reported: Option<i64>| {
            let mut row = evidence("agent-inventory");
            row.latest_receipt_disposition = Some("rejected".to_owned());
            row.latest_rejection_code = Some("inventory_revision_conflict".to_owned());
            row.latest_rejection_inventory_revision = reported;
            row.latest_rejection_inventory_sha256 = Some("reported-hash".to_owned());
            row.accepted_inventory_revision = accepted;
            row.accepted_inventory_sha256 = Some("accepted-hash".to_owned());
            agent_attention_items(&row)
                .into_iter()
                .find(|item| item.kind == AttentionKind::AgentInventoryRejected)
                .unwrap()
                .message
        };

        // Equal revisions: content changed under the accepted revision.
        let content_conflict = message(5, Some(5));
        assert!(
            content_conflict.contains("inventory_revision"),
            "must name the field to bump: {content_conflict}"
        );
        assert!(
            content_conflict.contains("bump"),
            "must state the remedy: {content_conflict}"
        );

        // Regressed revision: the Agent is behind what the Server accepts.
        let below = message(5, Some(2));
        assert!(
            below.contains("below the accepted revision 5"),
            "must explain the regression: {below}"
        );

        // Unknown Network key has its own cause, not a revision bump.
        let mut unknown_key = evidence("agent-inventory");
        unknown_key.latest_receipt_disposition = Some("rejected".to_owned());
        unknown_key.latest_rejection_code = Some("network_key_unknown".to_owned());
        unknown_key.latest_rejection_inventory_revision = Some(5);
        unknown_key.accepted_inventory_revision = 5;
        let unknown_key = agent_attention_items(&unknown_key)
            .into_iter()
            .find(|item| item.kind == AttentionKind::AgentInventoryRejected)
            .unwrap()
            .message;
        assert!(
            unknown_key.contains("Network key"),
            "must name the unknown Network key: {unknown_key}"
        );

        // A v2 (Server-managed) refusal explains the Server-assigned remedy and
        // never tells the Owner to bump a revision the Agent no longer supplies
        // (issue #188).
        let mut server_managed = evidence("agent-inventory");
        server_managed.latest_receipt_disposition = Some("rejected".to_owned());
        server_managed.latest_rejection_code = Some("inventory_revision_conflict".to_owned());
        server_managed.latest_rejection_inventory_revision = None;
        server_managed.latest_rejection_inventory_protocol_major = Some(2);
        server_managed.latest_rejection_inventory_sha256 = Some("reported-fingerprint".to_owned());
        server_managed.accepted_inventory_revision = 5;
        server_managed.accepted_inventory_sha256 = Some("accepted-fingerprint".to_owned());
        let server_managed = agent_attention_items(&server_managed)
            .into_iter()
            .find(|item| item.kind == AttentionKind::AgentInventoryRejected)
            .unwrap()
            .message;
        assert!(
            server_managed.contains("Server-assigned"),
            "must explain the Server-managed revision: {server_managed}"
        );
        assert!(
            !server_managed.contains("bump"),
            "must not ask for an Agent revision bump: {server_managed}"
        );
    }

    #[test]
    fn node_kinds_are_not_acknowledgeable() {
        for kind in [
            AttentionKind::NodeUnhealthy,
            AttentionKind::NodeHealthUnknown,
            AttentionKind::NodeResync,
            AttentionKind::NodeIdentityMismatch,
        ] {
            assert!(!kind.is_agent_acknowledgeable(), "{kind:?}");
        }
        for kind in [
            AttentionKind::AgentOffline,
            AttentionKind::AgentSpoolFatal,
            AttentionKind::AgentSpoolOverflow,
            AttentionKind::AgentReportGap,
            AttentionKind::AgentSecurityEvent,
            AttentionKind::AgentShutdownIncomplete,
            AttentionKind::AgentInventoryRejected,
        ] {
            assert!(kind.is_agent_acknowledgeable(), "{kind:?}");
        }
    }
}
