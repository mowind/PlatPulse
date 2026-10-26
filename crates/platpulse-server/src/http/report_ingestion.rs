//! Transactional AgentReport ingestion for the first Inventory vertical slice.
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::str::FromStr;

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Extension, Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Any, FromRow, Transaction};

use crate::enrollment::AgentAuthInfo;
use crate::http::{AppState, ROUTE_GROUP_HEADER, RequestId};
use crate::metric_history::Delivery;
use crate::peer_history::PeerPresenceDelta;
use platpulse_core::component::{ComponentKey, ComponentObservation, ComponentStatus};
use platpulse_core::observation::{PeerDirection, PeerSnapshot};
use platpulse_core::protocol::SUPPORTED_PROTOCOL_MAJORS;
use platpulse_core::{
    AgentReport, ComponentRevision, InventoryAcceptance, InventoryDeclaration,
    InventoryDisposition, InventoryReceiptV2, NodeCurrentDisposition, NodeReceipt,
    ReceiptDisposition, ReportId, ReportInventory, ReportReceipt, ReportReceiptV2, Rfc3339,
    SampleDisposition, SampleDispositionKind, SampleRef, Sha256Hex,
};

#[derive(Debug, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReportResponse {
    pub receipt: ReportReceipt,
}

/// Success payload of one v2 AgentReport ingestion.
#[derive(Debug, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReportResponseV2 {
    pub receipt: ReportReceiptV2,
}

/// Protocol-discriminated exact receipt of one ingestion attempt.
///
/// v1 and v2 receipts must never share a serialized form: a v2 receipt binds the
/// Server-assigned Inventory revision and canonical fingerprint, while a v1
/// receipt echoes the Agent-declared disposition. A stored receipt is replayed
/// only through the shape that produced it.
enum IngestReceipt {
    V1(Box<ReportReceipt>),
    V2(Box<ReportReceiptV2>),
}

impl IngestReceipt {
    fn disposition(&self) -> ReceiptDisposition {
        match self {
            Self::V1(receipt) => receipt.disposition,
            Self::V2(receipt) => receipt.disposition,
        }
    }

    fn rejections(&self) -> &[platpulse_core::Rejection] {
        match self {
            Self::V1(receipt) => &receipt.rejections,
            Self::V2(receipt) => &receipt.rejections,
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::V1(receipt) => serde_json::to_vec(receipt),
            Self::V2(receipt) => serde_json::to_vec(receipt),
        }
        .expect("receipt serializes")
    }

    fn into_response(self) -> Response {
        let disposition = self.disposition();
        let mut response = match self {
            Self::V1(receipt) => {
                (StatusCode::OK, Json(ReportResponse { receipt: *receipt })).into_response()
            }
            Self::V2(receipt) => {
                (StatusCode::OK, Json(ReportResponseV2 { receipt: *receipt })).into_response()
            }
        };
        response.extensions_mut().insert(disposition);
        response
    }
}

/// Re-materialize a stored receipt under the request's protocol major. A stored
/// body that does not decode as that major is not replayable to this Agent.
fn replay_receipt<I: ReportInventory>(body: &[u8]) -> Result<IngestReceipt, ()> {
    if I::protocol_version() == platpulse_core::protocol::PROTOCOL_VERSION_V2 {
        serde_json::from_slice::<ReportReceiptV2>(body)
            .map(|receipt| IngestReceipt::V2(Box::new(receipt)))
            .map_err(|_| ())
    } else {
        serde_json::from_slice::<ReportReceipt>(body)
            .map(|receipt| IngestReceipt::V1(Box::new(receipt)))
            .map_err(|_| ())
    }
}

/// A whole-Inventory rejection is protocol-shape-independent apart from the
/// Inventory outcome, so the v1 rejection receipt is mapped to the v2 wire form
/// (which carries no accepted revision/fingerprint).
fn into_ingest_receipt<I: ReportInventory>(receipt: ReportReceipt) -> IngestReceipt {
    if I::protocol_version() == platpulse_core::protocol::PROTOCOL_VERSION_V2 {
        IngestReceipt::V2(Box::new(ReportReceiptV2 {
            report_id: receipt.report_id,
            disposition: receipt.disposition,
            report_body_sha256: receipt.report_body_sha256,
            server_version: receipt.server_version,
            supported_protocol_majors: receipt.supported_protocol_majors,
            server_time: receipt.server_time,
            rotation_hint: receipt.rotation_hint,
            inventory: receipt.inventory.map(|disposition| InventoryReceiptV2 {
                disposition,
                acceptance: None,
            }),
            rejections: receipt.rejections,
            nodes: receipt.nodes,
            samples: receipt.samples,
        }))
    } else {
        IngestReceipt::V1(Box::new(receipt))
    }
}

#[derive(Debug, FromRow)]
struct ReceiptRow {
    report_body_sha256: String,
    receipt_body: Vec<u8>,
}

#[derive(Debug, FromRow)]
struct AgentRow {
    agent_epoch: i64,
    active_boot_id: Option<String>,
    active_boot_status: String,
    previous_boot_id: Option<String>,
    close_report_id: Option<String>,
    last_report_sequence: Option<i64>,
    last_inventory_revision: i64,
}

fn now() -> Rfc3339 {
    crate::auth::format_rfc3339(crate::auth::now_utc())
        .parse()
        .expect("formatted timestamp is valid")
}

/// The instant a Report reached the Server, as the recorded state log stamps it.
///
/// The ingestion stamps its own receipt instant in the canonical shape, so
/// parsing it back is exact. The Report's generation instant is the fallback,
/// which keeps the write path total instead of panicking on a caller that
/// passes something else.
fn received_instant(received_at: &str, generated_at: Rfc3339) -> Rfc3339 {
    received_at.parse().unwrap_or(generated_at)
}

/// The two instants one Agent Report carries, as one value.
///
/// `received` is the Server's own instant: it answers Agent liveness,
/// idempotency and audit, and nothing about when a Node was actually observed.
/// `skew_ms` is the offset the same Report measured against Server time, which
/// is what turns the Report's own observation instants into Server-clock
/// instants (ADR 0014, issue #274). Every write that records "when was this
/// observed" takes the corrected instant from here instead of the receipt
/// instant, so a replayed Report cannot look newly observed.
#[derive(Clone, Copy)]
struct ReportStamp<'a> {
    received: &'a str,
    skew_ms: Option<i64>,
}

impl<'a> ReportStamp<'a> {
    /// The Report's own `host.clock_skew` reading is kept as the Agent reported
    /// it: [`crate::observation_time::observation_instant`] is the one place that
    /// decides whether it may be corrected by.
    fn new(received: &'a str, clock_skew_ms: Option<i64>) -> Self {
        Self {
            received,
            skew_ms: clock_skew_ms,
        }
    }

    /// The Server receipt instant of this Report.
    fn received_at(&self) -> &'a str {
        self.received
    }

    /// The Server-clock instant at which `observed` happened.
    ///
    /// An unmeasured or Clock Unreliable skew, and a Report that carries no
    /// observation instant, both fall back to the receipt instant.
    fn observed_at(&self, observed: Option<Rfc3339>) -> String {
        crate::observation_time::observation_instant(observed, self.skew_ms, self.received)
    }

    /// The Server-clock instant to stamp a history fact derived from `observed`.
    ///
    /// The same correction as [`Self::observed_at`], except that an
    /// untrustworthy skew leaves the Agent's own instant in place: a history row
    /// records when the Node did something, so it can never borrow the instant
    /// the Server happened to receive the Report.
    fn history_at(&self, observed: Rfc3339) -> Rfc3339 {
        crate::observation_time::history_instant(observed, self.skew_ms)
    }
}

/// A polled Chain Head can trail the subscribed Block History by the blocks
/// that arrived between polls, so one below-high-water sample is observational
/// skew rather than a resync. A below-high-water episode must persist for this
/// long before the resync projection treats it as real; a genuine replay lasts
/// far longer, while skew clears on the next report.
const RESYNC_TRANSITION_GRACE_SECONDS: i64 = 30;

fn rejection(code: platpulse_core::RejectionCode, reason: &str) -> platpulse_core::Rejection {
    platpulse_core::Rejection {
        code,
        retryable: code.is_retryable(),
        reason: crate::redaction::redact_sensitive(reason),
    }
}

/// Stable rejection for a Node ID that carries the durable purge boundary
/// (issue #170). Terminal: the Agent must not expect a retry to succeed.
fn purged_rejection() -> platpulse_core::Rejection {
    rejection(
        platpulse_core::RejectionCode::NodePurged,
        "Node was permanently purged by the Owner",
    )
}

/// Stable rejection for a Node entry the reporting Agent does not own.
fn ownership_mismatch_rejection() -> platpulse_core::Rejection {
    rejection(
        platpulse_core::RejectionCode::NodeOwnershipMismatch,
        "Node belongs to another Agent",
    )
}

fn disposition_name(disposition: ReceiptDisposition) -> &'static str {
    match disposition {
        ReceiptDisposition::Accepted => "accepted",
        ReceiptDisposition::PartiallyAccepted => "partially_accepted",
        ReceiptDisposition::Rejected => "rejected",
    }
}
fn rejected(
    report_id: ReportId,
    hash: Sha256Hex,
    code: platpulse_core::RejectionCode,
    reason: &str,
) -> ReportReceipt {
    // A whole-report rejection is answered inside a 200 receipt, so its reason
    // code is logged here; the completion line only says "rejected".
    tracing::warn!(outcome = code.as_str(), "report rejected: {reason}");
    ReportReceipt {
        report_id,
        disposition: ReceiptDisposition::Rejected,
        report_body_sha256: hash,
        server_version: crate::VERSION.to_owned(),
        supported_protocol_majors: SUPPORTED_PROTOCOL_MAJORS.to_vec(),
        server_time: now(),
        rotation_hint: None,
        inventory: Some(InventoryDisposition::Rejected),
        rejections: vec![rejection(code, reason)],
        nodes: vec![],
        samples: vec![],
    }
}

fn error(
    request_id: &str,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    // Every refusal names its reason under the request span (issue #250); a
    // retrying Agent repeats it, which is exactly what an operator must see.
    tracing::warn!(status = status.as_u16(), outcome = code, "report refused");
    (
        status,
        Json(crate::http::ApiErrorBody::new(code, message, request_id)),
    )
        .into_response()
}

/// Turn a failed report-transaction database write into a response an
/// operator can act on. Folding every `sqlx::Error` into a bare
/// `unavailable` hides a schema/constraint bug behind an ordinary outage
/// signal and leaves the Agent retrying forever with no Server-side clue
/// (issue #179). Log the redacted error under its request id and keep a
/// distinct code for a constraint violation.
fn storage_error(request_id: &str, operation: &str, failure: &sqlx::Error) -> Response {
    let constraint = matches!(
        failure,
        sqlx::Error::Database(database)
            if database.is_unique_violation() || database.is_check_violation()
    );
    tracing::error!(error = %failure, "report ingestion storage error during {operation}");
    if constraint {
        return error(
            request_id,
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_conflict",
            "Server storage rejected the report",
        );
    }
    error(
        request_id,
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Server database is unavailable",
    )
}

fn receipt_response(receipt: IngestReceipt) -> Response {
    receipt.into_response()
}

async fn store_rejected<I: ReportInventory>(
    state: &AppState,
    mut tx: Transaction<'_, Any>,
    report: &AgentReport<I>,
    hash: Sha256Hex,
    receipt: ReportReceipt,
    request_id: &str,
) -> Response {
    let receipt = into_ingest_receipt::<I>(receipt);
    let stored = receipt.to_bytes();
    let disposition = receipt.disposition();
    // Durable Inventory evidence (issue #181): which code refused the report
    // and what Inventory the Agent declared. Admin must never have to decode
    // the receipt body, nor trust an Agent-reported diagnostic, to answer "is
    // this Agent's latest ingestion attempt an Inventory rejection?". A
    // whole-report rejection decides by its first rejection.
    let rejection_code = receipt
        .rejections()
        .first()
        .map(|rejection| rejection.code.as_str());
    let result = sqlx::query("INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at, rejection_code, inventory_revision, inventory_sha256, inventory_protocol_major) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)")
        .bind(report.report_id.to_string()).bind(report.agent_id.to_string())
        .bind(report.agent_epoch as i64).bind(report.boot_id.to_string())
        .bind(report.report_sequence as i64).bind(hash.to_string())
        .bind(disposition_name(disposition)).bind(&stored)
        .bind(now().to_string())
        .bind(rejection_code)
        .bind(report.inventory.declared_revision().map(|revision| revision as i64))
        .bind(report.inventory.accepted_sha256().to_string())
        // The Server-owned protocol major of the refused declaration. v2
        // declares no revision, so the Admin diagnosis explains it from the
        // accepted revision/fingerprint and this evidence instead of forging a
        // removed Agent-supplied revision (issue #188).
        .bind(I::protocol_version() as i64)
        .execute(&mut *tx).await;
    if result.is_err() || tx.commit().await.is_err() {
        return error(
            request_id,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    // A rejection writes no projection row, but it does change derived Admin
    // state: the Agent's Inventory rejection Attention Item. Without this
    // signal an already-open Agent page would keep showing the frozen Agent
    // with no explanation. The realtime layer coalesces same-resource events,
    // so a rejection every collection interval is not an event flood.
    state.admin_realtime().publish(
        "agent",
        Some(report.agent_id.to_string()),
        report.report_sequence,
    );
    receipt_response(receipt)
}

async fn record_security_event(
    tx: &mut Transaction<'_, Any>,
    agent_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agents SET security_event_count=security_event_count+1 WHERE agent_id=$1")
        .bind(agent_id)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}

fn status_name(status: ComponentStatus) -> &'static str {
    match status {
        ComponentStatus::Starting => "starting",
        ComponentStatus::Ok => "ok",
        ComponentStatus::Error => "error",
        ComponentStatus::Disabled => "disabled",
        ComponentStatus::Unsupported => "unsupported",
    }
}

/// Return the persisted component key used by the Server projections.
///
/// Most legacy keys predate the wire enum's snake_case names. Network identity
/// is read by the Admin projection under its stable network_identity key, so
/// ingestion must use that same key rather than the Rust debug spelling.
pub(crate) fn component_storage_key(key: ComponentKey) -> String {
    match key {
        ComponentKey::NetworkIdentity => "network_identity".to_owned(),
        key => format!("{key:?}").to_lowercase(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn save_component<T: serde::Serialize>(
    tx: &mut Transaction<'_, Any>,
    agent_id: &str,
    scope: &str,
    scope_key: &str,
    node_id: Option<&str>,
    key: ComponentKey,
    component: &ComponentObservation<T>,
    stamp: ReportStamp<'_>,
) -> Result<(), sqlx::Error> {
    let received_at = stamp.received_at();
    let error_code = component.error.as_ref().map(|e| e.code.as_str());
    let redacted_error_message = component
        .error
        .as_ref()
        .map(|error| crate::redaction::redact_sensitive(&error.message));
    let error_message = redacted_error_message.as_deref();
    let has_value = component.status == ComponentStatus::Ok && component.latest.is_some();
    let value_received_at = has_value.then_some(received_at);
    // The state evidence is the newest attempt, which a failed collection also
    // carries; the value evidence is the instant the value the Server still
    // holds was observed. Both are Server-clock instants, so the freshness and
    // the alert evidence of a replayed Report stay where the Report observed
    // them (ADR 0014, issue #274). The Agent's raw instant stays in `observed_at`.
    let observation_at = stamp.observed_at(component.attempted_at.or(component.latest_observed_at));
    let value_observation_at = has_value
        .then(|| stamp.observed_at(component.latest_observed_at.or(component.attempted_at)));
    sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, value_received_at, observation_at, value_observation_at, state_revision, value_revision, error_code, error_message) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) ON CONFLICT(agent_id, scope, scope_key, component_key) DO UPDATE SET state=excluded.state, attempted_at=excluded.attempted_at, observed_at=COALESCE(excluded.observed_at, component_status.observed_at), received_at=excluded.received_at, value_received_at=COALESCE(excluded.value_received_at, component_status.value_received_at), observation_at=excluded.observation_at, value_observation_at=COALESCE(excluded.value_observation_at, component_status.value_observation_at), state_revision=excluded.state_revision, value_revision=CASE WHEN excluded.value_revision > 0 THEN excluded.value_revision ELSE component_status.value_revision END, error_code=excluded.error_code, error_message=excluded.error_message")
        .bind(agent_id).bind(scope).bind(scope_key).bind(node_id).bind(component_storage_key(key))
        .bind(status_name(component.status)).bind(component.attempted_at.map(|v| v.to_string()))
        .bind(component.latest_observed_at.map(|v| v.to_string())).bind(received_at)
        .bind(value_received_at).bind(&observation_at).bind(value_observation_at)
        .bind(component.state_revision as i64).bind(component.value_revision as i64)
        .bind(error_code).bind(error_message).execute(&mut **tx).await?;
    Ok(())
}

/// Record the Peer presence intervals of one successful Peer snapshot.
///
/// `observed_at` is the snapshot's observation instant on the Server clock, not
/// its receipt instant: an interval says when a Peer was really seen, so a
/// replayed Report must produce the intervals of its own window instead of
/// squeezing hours of churn into the catch-up window (ADR 0014, issue #274).
async fn save_peer_presence(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    snapshot: &PeerSnapshot,
    observed_at: &str,
    had_previous_successful_snapshot: bool,
    stored: &StoredPeerState,
) -> Result<PeerPresenceDelta, sqlx::Error> {
    // The first successful snapshot is the baseline. Presence intervals are
    // derived only from a difference between two successful snapshots.
    if !had_previous_successful_snapshot {
        return Ok(PeerPresenceDelta::default());
    }
    // The Peer set the Server holds is the previous successful snapshot, and
    // the Report has already read it once for its diff write (issue #271), so
    // the boundary is derived from that read instead of a second query.
    let previous_peer_ids = &stored.peers;
    let open_peer_ids: HashSet<String> = sqlx::query_scalar(
        "SELECT peer_id FROM peer_presence_intervals WHERE node_id=$1 AND closed_at IS NULL",
    )
    .bind(node_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect();
    let incoming: HashSet<String> = snapshot
        .peers
        .iter()
        .map(|peer| crate::redaction::redact_peer_identity(&peer.peer_id))
        .collect();
    let delta = PeerPresenceDelta {
        arrivals: incoming
            .iter()
            .filter(|peer_id| !previous_peer_ids.contains_key(peer_id.as_str()))
            .count() as i64,
        departures: previous_peer_ids
            .keys()
            .filter(|peer_id| !incoming.contains(peer_id.as_str()))
            .count() as i64,
    };

    // An interval is a snapshot of the arrival boundary. Once opened, its
    // identity/operational fields do not change while the Peer remains
    // present; a later changed snapshot is still the same connection.
    for peer in &snapshot.peers {
        let safe_peer_id = crate::redaction::redact_peer_identity(&peer.peer_id);
        if previous_peer_ids.contains_key(safe_peer_id.as_str())
            || open_peer_ids.contains(&safe_peer_id)
        {
            continue;
        }
        let direction = match peer.direction {
            PeerDirection::Inbound => "inbound",
            PeerDirection::Outbound => "outbound",
        };
        let safe_client_name = peer
            .client_name
            .as_deref()
            .map(crate::redaction::redact_sensitive);
        sqlx::query("INSERT INTO peer_presence_intervals (node_id, peer_id, direction, trusted, static_peer, consensus_peer, client_name, opened_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
            .bind(node_id)
            .bind(&safe_peer_id)
            .bind(direction)
            .bind(peer.trusted as i64)
            .bind(peer.static_peer as i64)
            .bind(peer.consensus_peer as i64)
            .bind(safe_client_name.as_deref())
            .bind(observed_at)
            .execute(&mut **tx)
            .await?;
    }

    for peer_id in open_peer_ids {
        if !incoming.contains(peer_id.as_str()) {
            // Server timestamps are normally monotonic, but a wall-clock
            // adjustment must not violate the interval ordering constraint.
            sqlx::query("UPDATE peer_presence_intervals SET closed_at=CASE WHEN opened_at > $1 THEN opened_at ELSE $2 END WHERE node_id=$3 AND peer_id=$4 AND closed_at IS NULL")
                .bind(observed_at)
                .bind(observed_at)
                .bind(node_id)
                .bind(peer_id)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(delta)
}

/// Rows one batched statement carries when a Peer snapshot is written.
///
/// A snapshot holds a few hundred Peers and their capabilities, and Report
/// Ingestion writes it through the Server's single write connection (ADR 0013,
/// issue #213), so the snapshot costs one round trip per chunk instead of one
/// per row.
const PEER_INSERT_ROWS: usize = 64;
const PEER_CAPABILITY_INSERT_ROWS: usize = 200;

/// Rows one batched statement carries when a Node's RPC surface is written. A
/// Node reports a few hundred methods, and they reach the Server in one
/// statement per chunk rather than one per method (issue #213).
const RPC_INSERT_ROWS: usize = 200;

/// One Peer of a successful snapshot, already redacted and canonicalised.
///
/// The Server stores a Peer and a snapshot reports a Peer in the same shape, so
/// the difference between two snapshots is a field comparison rather than a
/// second query: [`StoredPeerState`] holds the rows the Server already has, and
/// a row the snapshot no longer reports is exactly the row the write deletes.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
struct CurrentPeerRow {
    peer_id: String,
    remote_ip: Option<String>,
    direction: String,
    trusted: i64,
    static_peer: i64,
    consensus_peer: i64,
    client_name: Option<String>,
    cbft_protocol_version: Option<i64>,
    cbft_highest_qc_block: Option<i64>,
    cbft_locked_block: Option<i64>,
    cbft_commit_block: Option<i64>,
}

/// The Peer set one Node already has, with the capabilities recorded for it.
///
/// Report Ingestion reads it once per Report: the presence interval boundary
/// and the diff write both derive from the same rows (issue #271), and a
/// snapshot that repeats them writes nothing.
#[derive(Debug, Default)]
struct StoredPeerState {
    peers: BTreeMap<String, CurrentPeerRow>,
    capabilities: BTreeMap<String, BTreeSet<String>>,
}

async fn load_stored_peers(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
) -> Result<StoredPeerState, sqlx::Error> {
    let peers = sqlx::query_as::<_, CurrentPeerRow>(
        "SELECT peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, client_name, cbft_protocol_version, cbft_highest_qc_block, cbft_locked_block, cbft_commit_block FROM current_node_peers WHERE node_id=$1",
    )
    .bind(node_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut stored = StoredPeerState {
        peers: peers
            .into_iter()
            .map(|peer| (peer.peer_id.clone(), peer))
            .collect(),
        capabilities: BTreeMap::new(),
    };
    let capabilities = sqlx::query_as::<_, (String, String)>(
        "SELECT peer_id, capability FROM current_node_peer_capabilities WHERE node_id=$1",
    )
    .bind(node_id)
    .fetch_all(&mut **tx)
    .await?;
    for (peer_id, capability) in capabilities {
        stored
            .capabilities
            .entry(peer_id)
            .or_default()
            .insert(capability);
    }
    Ok(stored)
}

/// Write only the difference between the Peer set a snapshot reports and the
/// one the Server holds: arrivals are inserted, departures deleted, and a Peer
/// whose attributes changed is rewritten.
///
/// A Report that repeats a Peer set writes no row here at all (issue #271).
/// Peer freshness is the `peers` component's last value receipt
/// (`component_status.value_received_at`), so the row stamp records a change
/// instead of the receipt, and an unchanged snapshot leaves both Peer tables
/// exactly as they were.
async fn save_current_peers(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    snapshot: &PeerSnapshot,
    received_at: &str,
    stored: &StoredPeerState,
) -> Result<(), sqlx::Error> {
    // The snapshot is reduced to the rows the Server can compare, keyed by Peer
    // ID so the diff walks both sides in one pass.
    let mut reported: BTreeMap<String, (CurrentPeerRow, BTreeSet<String>)> = BTreeMap::new();
    for peer in &snapshot.peers {
        let safe_peer_id = crate::redaction::redact_peer_identity(&peer.peer_id);
        let capabilities = peer
            .caps
            .iter()
            .map(|capability| crate::redaction::redact_sensitive(capability))
            .collect();
        reported.insert(
            safe_peer_id.clone(),
            (
                CurrentPeerRow {
                    peer_id: safe_peer_id,
                    remote_ip: peer
                        .remote_ip
                        .as_deref()
                        .and_then(crate::geo::GeoLoader::canonical_public_ip),
                    direction: match peer.direction {
                        PeerDirection::Inbound => "inbound".to_owned(),
                        PeerDirection::Outbound => "outbound".to_owned(),
                    },
                    trusted: peer.trusted as i64,
                    static_peer: peer.static_peer as i64,
                    consensus_peer: peer.consensus_peer as i64,
                    client_name: peer
                        .client_name
                        .as_deref()
                        .map(crate::redaction::redact_sensitive),
                    cbft_protocol_version: peer.cbft_protocol_version.map(|value| value as i64),
                    cbft_highest_qc_block: peer.cbft_highest_qc_block.map(|value| value as i64),
                    cbft_locked_block: peer.cbft_locked_block.map(|value| value as i64),
                    cbft_commit_block: peer.cbft_commit_block.map(|value| value as i64),
                },
                capabilities,
            ),
        );
    }

    // A successful snapshot is authoritative, including an empty list: every
    // Peer it no longer reports loses its row and the capabilities recorded
    // with it. The capabilities are deleted explicitly rather than left to the
    // foreign key cascade, so both tables stay clean on every Storage Backend.
    let departed: Vec<&str> = stored
        .peers
        .keys()
        .filter(|peer_id| !reported.contains_key(*peer_id))
        .map(String::as_str)
        .collect();
    for chunk in departed.chunks(PEER_INSERT_ROWS) {
        let sql = crate::database::number_placeholders(&format!(
            "DELETE FROM current_node_peer_capabilities WHERE node_id=? AND peer_id IN ({})",
            crate::database::placeholder_list(chunk.len())
        ));
        let mut statement = sqlx::query(&sql).bind(node_id);
        for peer_id in chunk {
            statement = statement.bind(*peer_id);
        }
        statement.execute(&mut **tx).await?;
        let sql = crate::database::number_placeholders(&format!(
            "DELETE FROM current_node_peers WHERE node_id=? AND peer_id IN ({})",
            crate::database::placeholder_list(chunk.len())
        ));
        let mut statement = sqlx::query(&sql).bind(node_id);
        for peer_id in chunk {
            statement = statement.bind(*peer_id);
        }
        statement.execute(&mut **tx).await?;
    }

    // Arrivals and changed attributes share one statement shape, so the round
    // trips stay proportional to what changed instead of to the Peer count: a
    // Peer the Server does not hold is inserted, and a Peer whose content
    // differs is rewritten with the snapshot's values. An unchanged Peer is in
    // neither list, which is what makes a repeated snapshot write nothing.
    let mut rewritten: Vec<&CurrentPeerRow> = Vec::new();
    let mut arrived_capabilities: Vec<(&str, &str)> = Vec::new();
    let mut departed_capabilities: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (peer_id, (row, capabilities)) in &reported {
        if stored.peers.get(peer_id) != Some(row) {
            rewritten.push(row);
        }
        let recorded = stored.capabilities.get(peer_id);
        for capability in capabilities {
            if !recorded.is_some_and(|recorded| recorded.contains(capability)) {
                arrived_capabilities.push((peer_id.as_str(), capability.as_str()));
            }
        }
        if let Some(recorded) = recorded {
            for capability in recorded {
                if !capabilities.contains(capability) {
                    departed_capabilities
                        .entry(peer_id.as_str())
                        .or_default()
                        .push(capability.as_str());
                }
            }
        }
    }

    for chunk in rewritten.chunks(PEER_INSERT_ROWS) {
        let sql = crate::database::number_placeholders(&format!(
            "INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, client_name, cbft_protocol_version, cbft_highest_qc_block, cbft_locked_block, cbft_commit_block, updated_at) VALUES {} ON CONFLICT(node_id, peer_id) DO UPDATE SET remote_ip=excluded.remote_ip, direction=excluded.direction, trusted=excluded.trusted, static_peer=excluded.static_peer, consensus_peer=excluded.consensus_peer, client_name=excluded.client_name, cbft_protocol_version=excluded.cbft_protocol_version, cbft_highest_qc_block=excluded.cbft_highest_qc_block, cbft_locked_block=excluded.cbft_locked_block, cbft_commit_block=excluded.cbft_commit_block, updated_at=excluded.updated_at",
            crate::database::row_placeholders(13, chunk.len())
        ));
        let mut statement = sqlx::query(&sql);
        for peer in chunk {
            statement = statement
                .bind(node_id)
                .bind(&peer.peer_id)
                .bind(peer.remote_ip.as_deref())
                .bind(peer.direction.as_str())
                .bind(peer.trusted)
                .bind(peer.static_peer)
                .bind(peer.consensus_peer)
                .bind(peer.client_name.as_deref())
                .bind(peer.cbft_protocol_version)
                .bind(peer.cbft_highest_qc_block)
                .bind(peer.cbft_locked_block)
                .bind(peer.cbft_commit_block)
                .bind(received_at);
        }
        statement.execute(&mut **tx).await?;
    }

    // A Peer whose capabilities changed is diffed per capability. A capability
    // rarely disappears from a connected Peer, so a removal is one bounded
    // statement for that Peer while every addition shares the batched insert.
    for (peer_id, capabilities) in &departed_capabilities {
        for chunk in capabilities.chunks(PEER_CAPABILITY_INSERT_ROWS) {
            let sql = crate::database::number_placeholders(&format!(
                "DELETE FROM current_node_peer_capabilities WHERE node_id=? AND peer_id=? AND capability IN ({})",
                crate::database::placeholder_list(chunk.len())
            ));
            let mut statement = sqlx::query(&sql).bind(node_id).bind(*peer_id);
            for capability in chunk {
                statement = statement.bind(*capability);
            }
            statement.execute(&mut **tx).await?;
        }
    }

    for chunk in arrived_capabilities.chunks(PEER_CAPABILITY_INSERT_ROWS) {
        let sql = crate::database::number_placeholders(&format!(
            "INSERT INTO current_node_peer_capabilities (node_id, peer_id, capability, updated_at) VALUES {}",
            crate::database::row_placeholders(4, chunk.len())
        ));
        let mut statement = sqlx::query(&sql);
        for (peer_id, capability) in chunk {
            statement = statement
                .bind(node_id)
                .bind(*peer_id)
                .bind(*capability)
                .bind(received_at);
        }
        statement.execute(&mut **tx).await?;
    }

    // Geo cache hygiene is not part of this write: `crate::geo::cleanup_cache`
    // refreshes Peer references and drops unreferenced addresses on its own
    // maintenance cycle (issue #271).
    Ok(())
}

async fn open_coverage_gap(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    from_height: u64,
    to_height: u64,
    created_at: &str,
) -> Result<(), sqlx::Error> {
    let overlap = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT coverage_id, first_height, last_height FROM block_coverage_intervals WHERE node_id=$1 AND status='open_recoverable_gap' AND first_height <= $2 AND last_height >= $3 ORDER BY first_height LIMIT 1",
    )
    .bind(node_id).bind(to_height as i64).bind(from_height as i64)
    .fetch_optional(&mut **tx).await?;
    if let Some((coverage_id, _first, _last)) = overlap {
        sqlx::query("UPDATE block_coverage_intervals SET first_height=CASE WHEN first_height <= $1 THEN first_height ELSE $1 END, last_height=CASE WHEN last_height >= $2 THEN last_height ELSE $2 END, updated_at=$3 WHERE coverage_id=$4")
            .bind(from_height as i64).bind(to_height as i64).bind(created_at).bind(coverage_id)
            .execute(&mut **tx).await?;
    } else {
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ($1, $2, $3, 'open_recoverable_gap', $4, $5)")
            .bind(node_id).bind(from_height as i64).bind(to_height as i64).bind(created_at).bind(created_at)
            .execute(&mut **tx).await?;
    }
    Ok(())
}

async fn append_coverage_height(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    height: u64,
    created_at: &str,
) -> Result<(), sqlx::Error> {
    let previous = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT coverage_id, first_height, last_height FROM block_coverage_intervals WHERE node_id=$1 AND status='covered' AND last_height < $2 ORDER BY last_height DESC LIMIT 1",
    )
    .bind(node_id).bind(height as i64).fetch_optional(&mut **tx).await?;
    if let Some((coverage_id, first, last)) = previous {
        if last + 1 == height as i64 {
            sqlx::query("UPDATE block_coverage_intervals SET last_height=$1,updated_at=$2 WHERE coverage_id=$3")
                .bind(height as i64).bind(created_at).bind(coverage_id).execute(&mut **tx).await?;
            return Ok(());
        }
        let _ = (first, last);
    }
    sqlx::query("INSERT INTO block_coverage_intervals (node_id,first_height,last_height,status,created_at,updated_at) VALUES ($1, $2, $3, 'covered', $4, $5) ON CONFLICT DO NOTHING")
        .bind(node_id).bind(height as i64).bind(height as i64).bind(created_at).bind(created_at).execute(&mut **tx).await?;
    Ok(())
}

async fn coverage_allows_backfill(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    height: u64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT coverage_id FROM block_coverage_intervals WHERE node_id=$1 AND status='open_recoverable_gap' AND first_height <= $2 AND last_height >= $3 LIMIT 1")
        .bind(node_id).bind(height as i64).bind(height as i64).fetch_optional(&mut **tx).await
}

async fn close_coverage_height(
    tx: &mut Transaction<'_, Any>,
    coverage_id: i64,
    height: u64,
    now_text: &str,
) -> Result<(), sqlx::Error> {
    let row = sqlx::query_as::<_, (i64, i64)>(
        "SELECT first_height,last_height FROM block_coverage_intervals WHERE coverage_id=$1",
    )
    .bind(coverage_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((first, last)) = row else {
        return Ok(());
    };
    let first = first as u64;
    let last = last as u64;
    if first == last {
        sqlx::query("DELETE FROM block_coverage_intervals WHERE coverage_id=$1")
            .bind(coverage_id)
            .execute(&mut **tx)
            .await?;
    } else if height == first {
        sqlx::query("UPDATE block_coverage_intervals SET first_height=first_height+1,updated_at=$1 WHERE coverage_id=$2").bind(now_text).bind(coverage_id).execute(&mut **tx).await?;
    } else if height == last {
        sqlx::query("UPDATE block_coverage_intervals SET last_height=last_height-1,updated_at=$1 WHERE coverage_id=$2").bind(now_text).bind(coverage_id).execute(&mut **tx).await?;
    } else if height > first && height < last {
        sqlx::query(
            "UPDATE block_coverage_intervals SET last_height=$1 ,updated_at=$2 WHERE coverage_id=$3",
        )
        .bind((height - 1) as i64)
        .bind(now_text)
        .bind(coverage_id)
        .execute(&mut **tx)
        .await?;
        // The interval is split in two: the row keeps [first, height-1] and the
        // copy above it covers [height+1, last], so height+1 is the first
        // height the remaining gap still misses.
        sqlx::query("INSERT INTO block_coverage_intervals (node_id,first_height,last_height,status,created_at,updated_at) SELECT node_id,$1,$2,status,created_at,$3 FROM block_coverage_intervals WHERE coverage_id=$4")
            .bind((height + 1) as i64)
            .bind(last as i64)
            .bind(now_text)
            .bind(coverage_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Recompute the Network Reference Head of every Network this Report touched.
///
/// A source must have been *observed* within the window on the Server clock, not
/// merely delivered: a Node whose Reports are being replayed from a backlog is
/// Stale, and a Stale Node is never a source of the Observed Network Head
/// (ADR 0014, issue #274). `now` is the receipt instant of the triggering
/// Report, which is the only instant the Server can defend when it finds no
/// eligible source at all.
async fn update_network_references(
    tx: &mut Transaction<'_, Any>,
    network_keys: &[String],
    now: &str,
) -> Result<(), sqlx::Error> {
    for network_key in network_keys {
        // The window is judged here on parsed instants: an unparseable evidence
        // instant is never fresh.
        let now_instant = crate::auth::parse_rfc3339(now);
        // A Node whose head was never observed is not a source: its NULL head
        // must not stand in as block 0 (the native SQLite decode once did).
        let rows = sqlx::query_as::<
            _,
            (
                String,
                i64,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            "SELECT n.node_id, c.current_block, h.resync_state, COALESCE(rpc.observation_at, rpc.received_at), COALESCE(sy.observation_at, sy.received_at), COALESCE(co.observation_at, co.received_at) FROM nodes n JOIN current_node_chain_observations c ON c.node_id=n.node_id LEFT JOIN block_history_state h ON h.node_id=n.node_id LEFT JOIN component_status sy ON sy.agent_id=n.agent_id AND sy.scope='node' AND sy.scope_key=n.node_id AND sy.component_key='sync' LEFT JOIN component_status rpc ON rpc.agent_id=n.agent_id AND rpc.scope='node' AND rpc.scope_key=n.node_id AND rpc.component_key='rpc' LEFT JOIN component_status co ON co.agent_id=n.agent_id AND co.scope='node' AND co.scope_key=n.node_id AND co.component_key='consensus' WHERE n.network_key=$1 AND n.lifecycle='active' AND c.current_block IS NOT NULL ORDER BY c.current_block DESC",
        )
        .bind(network_key)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .filter(|(_, _, _, rpc_at, sync_at, consensus_at)| {
            // A source must be a Node the Public projection would also call
            // fresh: every one of the three component observation instants
            // inside the window, in either direction. Judging sync alone let a
            // Node whose RPC and consensus were observed hours ago hold the
            // reference head (ADR 0014, issue #274).
            now_instant.is_some_and(|now| {
                [rpc_at, sync_at, consensus_at].into_iter().all(|at| {
                    crate::observation_time::is_fresh(
                        at.as_deref(),
                        now,
                        crate::observation_time::FRESHNESS_LIMIT_SECONDS,
                    )
                })
            })
        })
        .collect::<Vec<_>>();
        let eligible = rows
            .iter()
            .filter(|(_, _, state, _, _, _)| state.as_deref() != Some("resyncing"))
            .collect::<Vec<_>>();
        let candidates = if eligible.is_empty() {
            rows.iter().collect::<Vec<_>>()
        } else {
            eligible
        };
        // The head is a sync fact, so the instant it was observed is the sync
        // component's observation instant.
        let Some((node_id, head, _, _, sync_at, _)) = candidates.first() else {
            // No Node was observed recently enough to hold a reference: the row
            // records that the Server looked at its own clock `now` and found
            // none, which is why the reference is Unknown rather than stale.
            sqlx::query("INSERT INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES ($1, NULL, $2, 'unknown', 0, NULL) ON CONFLICT(network_key) DO UPDATE SET block_number=NULL, observed_at=excluded.observed_at, confidence='unknown', eligible_source_count=0, contributing_node_id=NULL")
                .bind(network_key).bind(now).execute(&mut **tx).await?;
            continue;
        };
        let confidence = if candidates.len() >= 2 { "high" } else { "low" };
        // The reference head is the head of the winning source at the instant it
        // was observed, not the instant this Report arrived.
        sqlx::query("INSERT INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT(network_key) DO UPDATE SET block_number=excluded.block_number, observed_at=excluded.observed_at, confidence=excluded.confidence, eligible_source_count=excluded.eligible_source_count, contributing_node_id=excluded.contributing_node_id")
            .bind(network_key).bind(*head).bind(sync_at.as_deref().unwrap_or(now)).bind(confidence).bind(candidates.len() as i64).bind(node_id)
            .execute(&mut **tx).await?;
    }
    Ok(())
}

/// One Network Identity field whose stamped value contradicted the Registry
/// tuple. Both sides are kept so a rejection can name what actually differed
/// instead of only asserting that something did (ADR 0011 decision 4).
#[derive(Debug, Clone, PartialEq, Eq)]
struct StampedIdentityField {
    field: &'static str,
    stamped: String,
    registered: String,
}

/// Why a Node's stamped samples contradicted the Registry, as far as the
/// Server can tell from the fresh component observation carried by the same
/// Report. The mismatch stays terminal and unrecoverable in every case; this
/// only chooses the remedy the rejection points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StampDivergence {
    /// The Node's current Network Identity observation matches the Registry
    /// while its stamped samples do not: the Agent stamps samples with an
    /// identity it cached before the Node's build changed (issue #235).
    StaleStamp,
    /// The current observation contradicts the Registry too: the Node serves a
    /// different Network, and no Agent restart can reconcile that.
    NetworkConflict,
    /// The Report carried no complete successful identity observation, so a
    /// stale stamp cannot yet be told apart from a cross-Network conflict.
    Unobserved,
}

/// A Node's rejected samples with the diagnosed cause, used for both the
/// Receipt rejection and the History Gap the Server records for them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockIdentityRejection {
    fields: Vec<StampedIdentityField>,
    divergence: StampDivergence,
}

impl BlockIdentityRejection {
    /// Operator-facing reason: every differing field with its stamped and
    /// registered value, the side that was found stale, and the remedy for
    /// that side. Field names avoid the redaction keyword list and values are
    /// stated without a separator so the sanctioned diagnostics survive
    /// redaction intact.
    fn reason(&self) -> String {
        let differences = self
            .fields
            .iter()
            .map(|field| {
                format!(
                    "{} stamped {} registered {}",
                    field.field, field.stamped, field.registered
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let (diagnosis, disposition) = match self.divergence {
            StampDivergence::StaleStamp => (
                "the Node's current Network Identity observation matches the registered Network, \
                 so this Agent stamps samples with an identity it cached before the Node's build \
                 changed and restarting the Agent on that Host is the only remedy",
                "The rejected heights stay a History Gap and are never backfilled; the Registry tuple is never edited to match a stamped sample",
            ),
            StampDivergence::NetworkConflict => (
                "the Node's current Network Identity observation contradicts the registered Network \
                 as well, so this is a genuine cross-Network conflict that restarting the Agent \
                 cannot reconcile",
                "The rejected heights are never merged into block history and are never backfilled; the Registry tuple is never edited to match a stamped sample",
            ),
            StampDivergence::Unobserved => (
                "this Report carried no complete successful Network Identity observation, so a \
                 stale stamp cannot yet be told apart from a genuine cross-Network conflict",
                "Confirm the Node's current identity and then restart the Agent on that Host; the rejected heights stay a History Gap and are never backfilled",
            ),
        };
        format!(
            "Block network identity does not match the registered Network ({differences}). Because {diagnosis}. {disposition}."
        )
    }
}

/// The Registry identity tuple a Node's Network is registered under.
struct RegisteredNetworkIdentity {
    genesis_hash: String,
    chain_id: i64,
    p2p_network_id: i64,
    address_hrp: String,
}

impl RegisteredNetworkIdentity {
    fn matches(&self, observed: &platpulse_core::network::NetworkIdentity) -> bool {
        observed.genesis_hash.to_string() == self.genesis_hash
            && observed.chain_id == self.chain_id as u64
            && observed.p2p_network_id == self.p2p_network_id as u64
            && observed.address_hrp.as_deref().unwrap_or("") == self.address_hrp
    }

    fn differing_fields(
        &self,
        stamped: &platpulse_core::network::NetworkIdentity,
    ) -> Vec<StampedIdentityField> {
        let mut fields = Vec::new();
        let genesis_hash = stamped.genesis_hash.to_string();
        if genesis_hash != self.genesis_hash {
            fields.push(StampedIdentityField {
                field: "genesis_hash",
                stamped: genesis_hash,
                registered: self.genesis_hash.clone(),
            });
        }
        if stamped.chain_id != self.chain_id as u64 {
            fields.push(StampedIdentityField {
                field: "chain_id",
                stamped: stamped.chain_id.to_string(),
                registered: self.chain_id.to_string(),
            });
        }
        if stamped.p2p_network_id != self.p2p_network_id as u64 {
            fields.push(StampedIdentityField {
                field: "p2p_network_id",
                stamped: stamped.p2p_network_id.to_string(),
                registered: self.p2p_network_id.to_string(),
            });
        }
        if stamped.address_hrp.as_deref().unwrap_or("") != self.address_hrp {
            fields.push(StampedIdentityField {
                field: "address_hrp",
                // A sample's stamp need not carry an HRP. Say so in words
                // that can never be a registered value: a Network's address
                // HRP must not contain whitespace (crates/platpulse-server/src/network.rs),
                // so "not reported" stays unambiguous next to a registered
                // HRP that literally reads "unset".
                stamped: stamped
                    .address_hrp
                    .clone()
                    .unwrap_or_else(|| "not reported".to_owned()),
                registered: self.address_hrp.clone(),
            });
        }
        fields
    }
}

/// The Node's own Network Identity as observed in this very Report. This is
/// what lets the Server separate an Agent's stale stamp from a Node that
/// really serves another Network (ADR 0011 decision 4); a Node whose identity
/// was not observed completely yields nothing rather than a guess.
fn fresh_network_identity<I: ReportInventory>(
    report: &AgentReport<I>,
    node_id: platpulse_core::identity::NodeId,
) -> Option<&platpulse_core::network::NetworkIdentity> {
    report
        .nodes
        .iter()
        .find(|observation| observation.node_id == node_id)
        .filter(|observation| observation.chain.network_identity.status == ComponentStatus::Ok)
        .and_then(|observation| observation.chain.network_identity.latest.as_ref())
        .filter(|identity| identity.address_hrp.is_some())
}

/// Identity of one stamped sample: the Node plus the exact Block Identity
/// Stamp its sample carries. One Report can carry more than one stamp for the
/// same Node when its samples span a subscription boundary (the wire format
/// validates each sample's stamp on its own), so every rejection is keyed by
/// the stamp it diagnosed rather than by the Node alone.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StampedIdentityKey {
    node_id: platpulse_core::identity::NodeId,
    genesis_hash: String,
    chain_id: u64,
    p2p_network_id: u64,
    address_hrp: Option<String>,
}

impl StampedIdentityKey {
    /// The key of the stamp one sample carries. Every field is kept as it was
    /// stamped, so two different stamps can never collapse into one key: an
    /// absent HRP is not the same stamp as a Network registered under the
    /// literal HRP "unset".
    fn new(
        node_id: platpulse_core::identity::NodeId,
        stamp: &platpulse_core::network::NetworkIdentity,
    ) -> Self {
        Self {
            node_id,
            genesis_hash: stamp.genesis_hash.to_string(),
            chain_id: stamp.chain_id,
            p2p_network_id: stamp.p2p_network_id,
            address_hrp: stamp.address_hrp.clone(),
        }
    }
}

/// Samples whose stamped Network Identity contradicts the Registry, keyed by
/// Node and stamp, each carrying the differing fields plus the diagnosed cause.
async fn block_network_identity_rejections<I: ReportInventory>(
    tx: &mut Transaction<'_, Any>,
    report: &AgentReport<I>,
    purged_nodes: &std::collections::HashSet<platpulse_core::identity::NodeId>,
) -> Result<std::collections::HashMap<StampedIdentityKey, BlockIdentityRejection>, sqlx::Error> {
    let mut rejections = std::collections::HashMap::new();
    for sample in &report.block_summaries {
        // A purged Node has no Network projection left to compare against and
        // must never raise an identity contradiction.
        if purged_nodes.contains(&sample.node_id) {
            continue;
        }
        let Some(node) = report
            .inventory
            .nodes()
            .iter()
            .find(|node| node.node_id == sample.node_id)
        else {
            continue;
        };
        let registered = sqlx::query_as::<_, (String, i64, i64, String)>(
            "SELECT genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks WHERE network_key = $1",
        ).bind(node.network_key.as_str()).fetch_optional(&mut **tx).await?;
        let Some((genesis_hash, chain_id, p2p_network_id, address_hrp)) = registered else {
            continue;
        };
        let registered = RegisteredNetworkIdentity {
            genesis_hash,
            chain_id,
            p2p_network_id,
            address_hrp,
        };
        let fields = registered.differing_fields(&sample.network_identity);
        if fields.is_empty() {
            continue;
        }
        // Every contradicted sample gets the diagnosis of the stamp it
        // actually carries, so the Receipt and the History Gap name the fields
        // that differed for that sample (ADR 0011 decision 4). The refusal
        // itself stays Node-scoped: one contradicted stamp already condemns the
        // Node's samples, and the operator-facing remedy is per Node.
        let divergence = match fresh_network_identity(report, sample.node_id) {
            None => StampDivergence::Unobserved,
            Some(observed) if registered.matches(observed) => StampDivergence::StaleStamp,
            Some(_) => StampDivergence::NetworkConflict,
        };
        rejections
            .entry(StampedIdentityKey::new(
                sample.node_id,
                &sample.network_identity,
            ))
            .or_insert(BlockIdentityRejection { fields, divergence });
    }
    Ok(rejections)
}

/// Record the identity evidence one Block Summary adds to its Node's window,
/// and the divergence observation a contradicted stamp produces.
///
/// `observed_at` is the skew-corrected observation instant of the sample
/// ([ReportStamp::history_at], ADR 0014), never the receipt instant: this history
/// remembers when the Node's chain said something, not when its Report arrived.
async fn observe_block_identity(
    tx: &mut Transaction<'_, Any>,
    sample: &platpulse_core::block::BlockSummary,
    observed_at: Rfc3339,
    received_at: &str,
) -> Result<bool, sqlx::Error> {
    const MAX_WINDOW_HEIGHTS: i64 = 2_048;
    let node_id = sample.node_id.to_string();
    // Identity evidence has its own retention and bounded height count. It is
    // deliberately independent from raw Block Summary retention.
    sqlx::query("DELETE FROM block_identity_window WHERE node_id=$1 AND retained_until IS NOT NULL AND retained_until < $2")
        .bind(&node_id)
        .bind(received_at)
        .execute(&mut **tx)
        .await?;
    let existing = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT block_hash, retained_until FROM block_identity_window WHERE node_id=$1 AND height=$2",
    )
    .bind(&node_id)
    .bind(sample.block_number as i64)
    .fetch_optional(&mut **tx)
    .await?;
    let retention_until = crate::auth::parse_rfc3339(received_at)
        .and_then(|value| value.checked_add(time::Duration::days(180)))
        .map(crate::auth::format_rfc3339);
    let observed_hash = sample.block_hash.to_string();
    if let Some((retained_hash, _)) = existing {
        if retained_hash != observed_hash {
            sqlx::query("INSERT INTO chain_divergence_observations (node_id, height, retained_block_hash, observed_block_hash, observed_at, reason, retained_observed_at) SELECT $1, $2, block_hash, $3, $4, 'chain_divergence', observed_at FROM block_identity_window WHERE node_id=$5 AND height=$6 ON CONFLICT DO NOTHING")
                .bind(&node_id)
                .bind(sample.block_number as i64)
                .bind(&observed_hash)
                .bind(observed_at.to_string())
                .bind(&node_id)
                .bind(sample.block_number as i64)
                .execute(&mut **tx)
                .await?;
            return Ok(true);
        }
        sqlx::query("UPDATE block_identity_window SET observed_at=$1, retained_until=$2 WHERE node_id=$3 AND height=$4")
            .bind(observed_at.to_string())
            .bind(retention_until)
            .bind(&node_id)
            .bind(sample.block_number as i64)
            .execute(&mut **tx)
            .await?;
        return Ok(false);
    }
    sqlx::query("INSERT INTO block_identity_window (node_id, height, block_hash, retained_until, observed_at) VALUES ($1, $2, $3, $4, $5)")
        .bind(&node_id)
        .bind(sample.block_number as i64)
        .bind(&observed_hash)
        .bind(retention_until)
        .bind(observed_at.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM block_identity_window WHERE node_id=$1 AND height NOT IN (SELECT height FROM block_identity_window WHERE node_id=$2 ORDER BY height DESC LIMIT $3)")
        .bind(&node_id)
        .bind(&node_id)
        .bind(MAX_WINDOW_HEIGHTS)
        .execute(&mut **tx)
        .await?;
    Ok(false)
}

fn metric_observed_at<T>(observation: &ComponentObservation<T>) -> Option<Rfc3339> {
    observation.latest_observed_at.or_else(|| {
        (observation.status == ComponentStatus::Ok)
            .then_some(observation.attempted_at)
            .flatten()
    })
}

/// The Host series one Report carries, as (metric, dimension, observed_at,
/// value).
///
/// Host evidence belongs to the Agent, never to a Node of it: one Report states
/// each quantity once and the history stores it once, so six Nodes on two Hosts
/// bill two series sets and no Node owns a copy of its Host's totals (design
/// §11.6, issue #215). The storage series are dimensioned by mount path, so a
/// path that moves to another filesystem starts a new series instead of
/// inheriting the old one's history.
///
/// Only quantities the Report actually carried are listed: a component with no
/// latest value states nothing, and a missing reading is not zero.
fn host_series_samples(
    host: &platpulse_core::observation::HostObservation,
) -> Vec<(&'static str, &str, Rfc3339, f64)> {
    let mut samples: Vec<(&'static str, &str, Rfc3339, f64)> = Vec::new();
    if let (Some(cpu_percent), Some(observed_at)) = (
        host.cpu_percent.latest,
        metric_observed_at(&host.cpu_percent),
    ) {
        samples.push(("cpu_percent", "", observed_at, cpu_percent));
    }
    if let (Some(memory), Some(observed_at)) =
        (host.memory.latest, metric_observed_at(&host.memory))
    {
        samples.push((
            "memory_used_bytes",
            "",
            observed_at,
            memory.used_bytes as f64,
        ));
        samples.push((
            "memory_total_bytes",
            "",
            observed_at,
            memory.total_bytes as f64,
        ));
    }
    if let (Some(load), Some(observed_at)) = (host.load.latest, metric_observed_at(&host.load)) {
        samples.push(("load1", "", observed_at, load.load1));
        samples.push(("load5", "", observed_at, load.load5));
        samples.push(("load15", "", observed_at, load.load15));
    }
    if let (Some(network), Some(observed_at)) = (
        host.network_throughput.latest,
        metric_observed_at(&host.network_throughput),
    ) {
        samples.push((
            "network_rx_bytes_per_sec",
            "",
            observed_at,
            network.rx_bytes_per_sec as f64,
        ));
        samples.push((
            "network_tx_bytes_per_sec",
            "",
            observed_at,
            network.tx_bytes_per_sec as f64,
        ));
    }
    if let (Some(disk), Some(observed_at)) =
        (host.disk.latest.as_ref(), metric_observed_at(&host.disk))
    {
        for mount in &disk.mounts {
            samples.push((
                "disk_used_bytes",
                mount.mount_path.as_str(),
                observed_at,
                mount.used_bytes as f64,
            ));
            samples.push((
                "disk_total_bytes",
                mount.mount_path.as_str(),
                observed_at,
                mount.total_bytes as f64,
            ));
        }
    }
    samples
}

/// The raw metric window a delivery in this Report is judged against.
///
/// The cutoff comes from the persisted `raw_metric_sample` policy, the same
/// family the post-ingestion cleanup reads, and it is read through the open
/// ingestion transaction: ingestion holds the Server's single write connection,
/// so a pool read here would wait on the caller's own transaction (issue #213).
async fn metric_window_cutoff(tx: &mut Transaction<'_, Any>) -> Result<String, sqlx::Error> {
    let days = crate::retention::metric_sample_retention_days_tx(tx).await?;
    Ok(crate::auth::format_rfc3339(
        crate::retention::family_cutoff(crate::auth::now_utc(), days),
    ))
}

/// The recorded-state window a delivery in this Report is judged against.
///
/// The state log is its own retention family (issue #217), so its floor comes
/// from its own policy and is read through the open ingestion transaction for
/// the same reason the raw window is (issue #213).
async fn state_window_cutoff(tx: &mut Transaction<'_, Any>) -> Result<String, sqlx::Error> {
    let days = crate::retention::state_retention_days_tx(tx).await?;
    Ok(crate::auth::format_rfc3339(
        crate::retention::family_cutoff(crate::auth::now_utc(), days),
    ))
}

/// Record one metric sample for a scope's series, unless optional history is
/// paused.
///
/// One writer serves both scopes (issue #215): a Host series is the same
/// evidence as a Node series. The scope selects the tables, the column that
/// names the owner, and whether the series identity carries a dimension (the
/// mount path of a storage series), so every rule below - the delivery
/// classification, the raw window's cutoff, the aggregate tiers, the ledger and
/// the skipped-series record - is one rule for both.
///
/// Nothing here expires history: raw samples are bounded by the retention
/// policy family `raw_metric_sample` (issue #213), because a fixed per-series
/// row cap tied the retained window to the report cadence and deleted readings
/// the Server had already accepted. The Server keeps at least the last 24 hours
/// of raw samples and never connects across a stretch it did not observe.
///
/// Under low-space protection the sample is not written: the Server records the
/// gap instead of deleting history it already holds (design §11.5, issue #212).
/// The gap record shares the ingestion transaction, so "sample skipped" and
/// "gap recorded" commit together and a failure rolls the whole Report back.
/// The series ledger moves in this same transaction.
async fn save_metric_sample(
    tx: &mut Transaction<'_, Any>,
    history: &crate::capacity::HistoryGate,
    scope: &crate::metric_history::SeriesScope<'_>,
    stamp: ReportStamp<'_>,
    source_observed_at: Rfc3339,
    raw_cutoff: &str,
    value: f64,
) -> Result<(), sqlx::Error> {
    // A metric sample's stable observation identity is kept separate from its
    // skew-corrected timeline (ADR 0014, issue #279).
    //
    // The Agent's raw observation instant serves as identity: the raw table's
    // unique key, stored_value lookup, and the series ledger's delivery
    // classification (last_observed_at high-water mark) all use it. A
    // per-Report skew reading jitters by milliseconds, and a correction that
    // crosses a second boundary (e.g. 501 ms vs 499 ms) moves the corrected
    // instant into a neighbouring second. If the instant were both identity
    // and timeline, a carried last-good sample crossing that boundary would appear
    // past the ledger's high-water mark and be double-counted (issue #213).
    //
    // Skew-corrected `observed_at` places the sample on the Server timeline for
    // range queries, aggregate buckets and retention expiry.
    let source_observed_str = source_observed_at.to_string();
    let observed_at = stamp.history_at(source_observed_at).to_string();
    let received_at = stamp.received_at();
    // What the Server already holds for this observation time, and how the
    // series ledger classifies this delivery: new history, a replay of an
    // observation it already has, or a correction to one (issue #213). The
    // ledger and the raw window's own cutoff answer that, not the sample row
    // alone: retention releases the row once it leaves the window while the
    // ledger keeps counting.
    let stored = crate::metric_history::stored_sample(tx, scope, &source_observed_str).await?;
    let stored_value = stored.as_ref().map(|s| s.value);
    let delivery = crate::metric_history::classify_delivery(
        tx,
        scope,
        &source_observed_str,
        stored_value,
        value,
        raw_cutoff,
    )
    .await?;
    if let crate::capacity::HistoryGate::Paused { interval_id } = history {
        // An observation the Server has already counted is not new history: a
        // repeated last-good sample is not a second lost sample (design §11.5,
        // issue #212). Only a first-time observation counts as a gap.
        if delivery != Delivery::Observed {
            return Ok(());
        }
        return crate::capacity::record_skipped_series(
            tx,
            interval_id,
            scope.schema.skipped_scope,
            scope.scope_key,
            scope.series.metric,
            scope.series.dimension,
            &observed_at,
        )
        .await;
    }
    // A delivery the raw window can no longer answer with, and that the Server
    // holds nothing for, is not written back: the ledger still counts what it
    // knows about the series, but the cleanup that runs on this same Report
    // would release the row again. A replay writes no row either, even when a
    // widened window covers its instant again: the row was released on purpose,
    // and recreating it would invent coverage and a fresh receipt for a stretch
    // the Server deliberately stopped holding (issue #213).
    let released =
        crate::metric_history::outside_retained_window(stored_value, &observed_at, raw_cutoff);
    if !released && delivery != Delivery::Replay {
        crate::metric_history::store_sample(
            tx,
            scope,
            &source_observed_str,
            &observed_at,
            received_at,
            value,
        )
        .await?;
    }
    // The tiers beyond the raw window accumulate in this same transaction: an
    // observation the Server counted advances its minute and five-minute
    // buckets even while its raw row is released, and a correction recomputes
    // the buckets it can still restate from retained rows. A replay moves
    // nothing — it is the same observation, not another one. Deriving the tiers
    // at read time instead would state a count the Server never counted, and
    // would count one stretch twice the moment it is read at two grains (issue
    // #214, design §11.6).
    match delivery {
        Delivery::Observed => {
            crate::metric_history::record_aggregates(tx, scope, &observed_at, received_at, value)
                .await?;
        }
        Delivery::Correction => {
            let target_observed_at = stored
                .as_ref()
                .map(|s| s.observed_at.as_str())
                .unwrap_or(observed_at.as_str());
            crate::metric_history::recompute_aggregates(
                tx,
                scope,
                target_observed_at,
                received_at,
                value,
            )
            .await?;
        }
        Delivery::Replay => {}
    }
    // The ledger moves with the sample it describes, so a committed Report can
    // never leave the count describing a sample the Server does not hold.
    crate::metric_history::record_delivery(tx, scope, &source_observed_str, received_at, delivery)
        .await?;
    // A first-time observation the window cannot hold is counted while it is
    // stored nowhere, so the instant it was counted at is recorded as evidence
    // the Server no longer holds: a later carry of the same instant — after the
    // Operator widens the window back over it — is a replay, never a second
    // observation.
    if released && delivery == Delivery::Observed {
        if let Some(floor) = crate::metric_history::counted_evidence_floor(&source_observed_str) {
            crate::metric_history::stamp_evidence_floor(tx, scope, &floor).await?;
        }
    }
    Ok(())
}

async fn save_current<I: ReportInventory>(
    tx: &mut Transaction<'_, Any>,
    history: &crate::capacity::HistoryGate,
    report: &AgentReport<I>,
    stamp: ReportStamp<'_>,
    geo_provider: crate::geo::GeoProvider,
) -> Result<(), sqlx::Error> {
    let received_at = stamp.received_at();
    // Both retention windows are read once for the whole Report: they depend
    // on the policy in this transaction's snapshot and on the clock, never on
    // the series being written, and Report Ingestion holds the Server's single
    // write connection (issue #213), so reading them per sample spent a round
    // trip per sample to re-learn the same two instants.
    let raw_cutoff = metric_window_cutoff(tx).await?;
    let state_cutoff = state_window_cutoff(tx).await?;
    let agent_id = report.agent_id.to_string();
    let host = &report.host;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::CpuPercent,
        &host.cpu_percent,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::Memory,
        &host.memory,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::Load,
        &host.load,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::Disk,
        &host.disk,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::NetworkThroughput,
        &host.network_throughput,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::ClockSkew,
        &host.clock_skew,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::Spool,
        &host.spool,
        stamp,
    )
    .await?;

    let spool_last_delivery_error = host
        .spool
        .latest
        .as_ref()
        .and_then(|value| value.last_delivery_error.as_deref())
        .map(crate::redaction::redact_sensitive);
    let spool_store_error = host
        .spool
        .latest
        .as_ref()
        .and_then(|value| value.store_error.as_deref())
        .map(crate::redaction::redact_sensitive);
    // Spool Backlog Age is derived here, at receipt, because the Report itself
    // cannot express it: an immutable Report can only say how old its oldest
    // queued Report was when it was generated, which after a replay is hours
    // stale. The reading and its clock basis are recorded together so the Admin
    // projections never mix the two bases (ADR 0014, issue #272).
    let spool_backlog = crate::spool_backlog::spool_backlog_age(
        host.spool.latest.as_ref(),
        report.generated_at,
        received_instant(received_at, report.generated_at),
        host.clock_skew.latest,
    );
    sqlx::query("INSERT INTO current_host_observations (agent_id, cpu_percent, memory_total_bytes, memory_used_bytes, load1, load5, load15, network_rx_bytes_per_sec, network_tx_bytes_per_sec, clock_skew_ms, spool_queued_bytes, spool_queued_reports, spool_oldest_queued_age_ms, spool_dropped_reports, spool_dropped_samples, spool_in_flight, spool_last_delivery_error, spool_last_delivery_at, spool_capacity_bytes, spool_max_age_seconds, spool_dropped_sequence_from, spool_dropped_sequence_to, spool_dropped_time_from, spool_dropped_time_to, spool_dropped_height_from, spool_dropped_height_to, spool_pending_history_gaps, spool_report_too_large, spool_store_fatal, spool_store_error, spool_backlog_age_ms, spool_backlog_age_basis, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33) ON CONFLICT(agent_id) DO UPDATE SET cpu_percent=COALESCE(excluded.cpu_percent, current_host_observations.cpu_percent), memory_total_bytes=COALESCE(excluded.memory_total_bytes, current_host_observations.memory_total_bytes), memory_used_bytes=COALESCE(excluded.memory_used_bytes, current_host_observations.memory_used_bytes), load1=COALESCE(excluded.load1, current_host_observations.load1), load5=COALESCE(excluded.load5, current_host_observations.load5), load15=COALESCE(excluded.load15, current_host_observations.load15), network_rx_bytes_per_sec=COALESCE(excluded.network_rx_bytes_per_sec, current_host_observations.network_rx_bytes_per_sec), network_tx_bytes_per_sec=COALESCE(excluded.network_tx_bytes_per_sec, current_host_observations.network_tx_bytes_per_sec), clock_skew_ms=COALESCE(excluded.clock_skew_ms, current_host_observations.clock_skew_ms), spool_queued_bytes=COALESCE(excluded.spool_queued_bytes, current_host_observations.spool_queued_bytes), spool_queued_reports=COALESCE(excluded.spool_queued_reports, current_host_observations.spool_queued_reports), spool_oldest_queued_age_ms=COALESCE(excluded.spool_oldest_queued_age_ms, current_host_observations.spool_oldest_queued_age_ms), spool_dropped_reports=COALESCE(excluded.spool_dropped_reports, current_host_observations.spool_dropped_reports), spool_dropped_samples=COALESCE(excluded.spool_dropped_samples, current_host_observations.spool_dropped_samples), spool_in_flight=COALESCE(excluded.spool_in_flight, current_host_observations.spool_in_flight), spool_last_delivery_error=COALESCE(excluded.spool_last_delivery_error, current_host_observations.spool_last_delivery_error), spool_last_delivery_at=COALESCE(excluded.spool_last_delivery_at, current_host_observations.spool_last_delivery_at), spool_capacity_bytes=COALESCE(excluded.spool_capacity_bytes, current_host_observations.spool_capacity_bytes), spool_max_age_seconds=COALESCE(excluded.spool_max_age_seconds, current_host_observations.spool_max_age_seconds), spool_dropped_sequence_from=COALESCE(excluded.spool_dropped_sequence_from, current_host_observations.spool_dropped_sequence_from), spool_dropped_sequence_to=COALESCE(excluded.spool_dropped_sequence_to, current_host_observations.spool_dropped_sequence_to), spool_dropped_time_from=COALESCE(excluded.spool_dropped_time_from, current_host_observations.spool_dropped_time_from), spool_dropped_time_to=COALESCE(excluded.spool_dropped_time_to, current_host_observations.spool_dropped_time_to), spool_dropped_height_from=COALESCE(excluded.spool_dropped_height_from, current_host_observations.spool_dropped_height_from), spool_dropped_height_to=COALESCE(excluded.spool_dropped_height_to, current_host_observations.spool_dropped_height_to), spool_pending_history_gaps=COALESCE(excluded.spool_pending_history_gaps, current_host_observations.spool_pending_history_gaps), spool_report_too_large=COALESCE(excluded.spool_report_too_large, current_host_observations.spool_report_too_large), spool_store_fatal=COALESCE(excluded.spool_store_fatal, current_host_observations.spool_store_fatal), spool_store_error=COALESCE(excluded.spool_store_error, current_host_observations.spool_store_error), spool_backlog_age_ms=excluded.spool_backlog_age_ms, spool_backlog_age_basis=excluded.spool_backlog_age_basis, updated_at=excluded.updated_at")
        .bind(&agent_id)
        .bind(crate::database::NullableReal(host.cpu_percent.latest))
        .bind(host.memory.latest.map(|v| v.total_bytes as i64))
        .bind(host.memory.latest.map(|v| v.used_bytes as i64))
        .bind(crate::database::NullableReal(host.load.latest.map(|v| v.load1)))
        .bind(crate::database::NullableReal(host.load.latest.map(|v| v.load5)))
        .bind(crate::database::NullableReal(host.load.latest.map(|v| v.load15)))
        .bind(host.network_throughput.latest.map(|v| v.rx_bytes_per_sec as i64))
        .bind(host.network_throughput.latest.map(|v| v.tx_bytes_per_sec as i64))
        .bind(host.clock_skew.latest)
        .bind(host.spool.latest.as_ref().map(|v| v.queued_bytes as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.queued_reports as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.oldest_queued_age_ms as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.dropped_reports as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.dropped_samples as i64))
        .bind(host.spool.latest.as_ref().and_then(|v| v.in_flight.map(|value| value as i64)))
        .bind(spool_last_delivery_error.as_deref())
        .bind(host.spool.latest.as_ref().and_then(|v| v.last_delivery_at.as_ref()).map(ToString::to_string))
        .bind(host.spool.latest.as_ref().and_then(|v| v.capacity_bytes.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.max_age_seconds.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_sequence_range.map(|x| x.0 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_sequence_range.map(|x| x.1 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_time_range.as_ref().map(|x| x.0.to_string())))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_time_range.as_ref().map(|x| x.1.to_string())))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_height_range.map(|x| x.0 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_height_range.map(|x| x.1 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.pending_history_gaps.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.report_too_large.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.store_fatal.map(|x| x as i64)))
        .bind(spool_store_error.as_deref())
        .bind(spool_backlog.as_ref().map(|age| age.age_ms))
        .bind(spool_backlog.as_ref().map(|age| age.basis.as_str()))
        .bind(received_at)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO agent_spool_diagnostics (agent_id, max_bytes, max_age_seconds, dropped_sequence_from, dropped_sequence_to, dropped_time_from, dropped_time_to, dropped_height_from, dropped_height_to, pending_history_gaps, report_too_large, store_fatal, store_error, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) ON CONFLICT(agent_id) DO UPDATE SET max_bytes=excluded.max_bytes, max_age_seconds=excluded.max_age_seconds, dropped_sequence_from=excluded.dropped_sequence_from, dropped_sequence_to=excluded.dropped_sequence_to, dropped_time_from=excluded.dropped_time_from, dropped_time_to=excluded.dropped_time_to, dropped_height_from=excluded.dropped_height_from, dropped_height_to=excluded.dropped_height_to, pending_history_gaps=excluded.pending_history_gaps, report_too_large=excluded.report_too_large, store_fatal=excluded.store_fatal, store_error=excluded.store_error, updated_at=excluded.updated_at")
        .bind(&agent_id)
        .bind(host.spool.latest.as_ref().and_then(|v| v.capacity_bytes.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.max_age_seconds.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_sequence_range.map(|x| x.0 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_sequence_range.map(|x| x.1 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_time_range.as_ref().map(|x| x.0.to_string())))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_time_range.as_ref().map(|x| x.1.to_string())))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_height_range.map(|x| x.0 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_height_range.map(|x| x.1 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.pending_history_gaps.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.report_too_large.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.store_fatal.map(|x| x as i64)))
        .bind(spool_store_error.as_deref())
        .bind(received_at)
        .execute(&mut **tx)
        .await?;
    if let Some(disk) = host.disk.latest.as_ref() {
        sqlx::query("DELETE FROM current_host_disk_mounts WHERE agent_id = $1")
            .bind(&agent_id)
            .execute(&mut **tx)
            .await?;
        for mount in &disk.mounts {
            sqlx::query("INSERT INTO current_host_disk_mounts (agent_id, mount_path, total_bytes, used_bytes, updated_at) VALUES ($1, $2, $3, $4, $5)")
                .bind(&agent_id).bind(&mount.mount_path).bind(mount.total_bytes as i64).bind(mount.used_bytes as i64).bind(received_at).execute(&mut **tx).await?;
        }
    }
    // Every Host series this Report carries is stored once, under the Agent
    // that owns it: a Node view reads these series through its Agent and keeps
    // no copy of its own (design §11.6, issue #215).
    for (metric, dimension, observed_at, value) in host_series_samples(host) {
        save_metric_sample(
            tx,
            history,
            &crate::metric_history::SeriesScope::host(&agent_id, metric, dimension),
            stamp,
            observed_at,
            &raw_cutoff,
            value,
        )
        .await?;
    }
    for node in &report.nodes {
        let node_id = node.node_id.to_string();
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::Process,
            &node.process,
            stamp,
        )
        .await?;
        if let Some(data_directory_size) = &node.data_directory_size_bytes {
            save_component(
                tx,
                &agent_id,
                "node",
                &node_id,
                Some(&node_id),
                ComponentKey::DataDirectorySizeBytes,
                data_directory_size,
                stamp,
            )
            .await?;
        }
        if let Some(data_directory_capacity) = &node.data_directory_capacity_bytes {
            save_component(
                tx,
                &agent_id,
                "node",
                &node_id,
                Some(&node_id),
                ComponentKey::DataDirectoryCapacityBytes,
                data_directory_capacity,
                stamp,
            )
            .await?;
        }
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::Rpc,
            &node.chain.rpc,
            stamp,
        )
        .await?;
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::Sync,
            &node.chain.sync,
            stamp,
        )
        .await?;
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::Consensus,
            &node.chain.consensus,
            stamp,
        )
        .await?;
        // Issue #217: the sync and consensus states are recorded as evidence of
        // their own. A numeric aggregate cannot hold an Error or Unknown stretch
        // without erasing it, and it cannot say a state stayed the same without
        // inventing a transition, so the state log is an append-only family
        // written beside the projection it describes.
        //
        // It runs after the component saves because the projection is what the
        // Owner actually sees: the instant of a value the Server still holds for
        // a component is read back from its own row, so a failure that carries
        // no value reports the age of the last-good value instead of a fresh
        // instant.
        let sync_state = crate::state_history::StateObservation::of_component(
            &node.chain.sync,
            node.chain.sync.latest.map(|value| value.syncing),
            crate::state_history::retained_value_at(
                tx,
                &agent_id,
                &node_id,
                crate::state_history::COMPONENT_SYNC,
            )
            .await?,
            stamp.history_at(crate::state_history::component_instant(
                &node.chain.sync,
                report.generated_at,
            )),
            received_instant(received_at, report.generated_at),
        );
        crate::state_history::record_state(
            tx,
            history,
            &node_id,
            crate::state_history::COMPONENT_SYNC,
            &sync_state,
            &state_cutoff,
        )
        .await?;
        // Consensus carries no syncing flag of its own: the flag belongs to the
        // sync value, so the consensus state vector never claims one.
        let consensus_state = crate::state_history::StateObservation::of_component(
            &node.chain.consensus,
            None,
            crate::state_history::retained_value_at(
                tx,
                &agent_id,
                &node_id,
                crate::state_history::COMPONENT_CONSENSUS,
            )
            .await?,
            stamp.history_at(crate::state_history::component_instant(
                &node.chain.consensus,
                report.generated_at,
            )),
            received_instant(received_at, report.generated_at),
        );
        crate::state_history::record_state(
            tx,
            history,
            &node_id,
            crate::state_history::COMPONENT_CONSENSUS,
            &consensus_state,
            &state_cutoff,
        )
        .await?;
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::NetworkIdentity,
            &node.chain.network_identity,
            stamp,
        )
        .await?;
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::StaticMetadata,
            &node.chain.static_metadata,
            stamp,
        )
        .await?;
        if let Some(peers) = &node.chain.peers {
            // A failed or unsupported collection keeps the last successful set
            // and its age; only a successful snapshot is authoritative,
            // including an empty list. Repetition of that rule lives in the
            // writers below.
            let snapshot = if peers.status == ComponentStatus::Ok {
                peers.latest.as_ref()
            } else {
                None
            };
            let had_previous_successful_snapshot = match snapshot {
                None => false,
                Some(_) => sqlx::query_scalar::<_, Option<String>>(
                    "SELECT value_received_at FROM component_status WHERE agent_id=$1 AND scope='node' AND scope_key=$2 AND component_key='peers'",
                )
                .bind(&agent_id)
                .bind(&node_id)
                .fetch_optional(&mut **tx)
                .await?
                .flatten()
                .is_some(),
            };
            // The current Peer set is read once per Report: the presence
            // interval boundary and the diff write share that read (#271).
            let stored_peers = match snapshot {
                None => StoredPeerState::default(),
                Some(_) => load_stored_peers(tx, &node_id).await?,
            };
            save_component(
                tx,
                &agent_id,
                "node",
                &node_id,
                Some(&node_id),
                ComponentKey::Peers,
                peers,
                stamp,
            )
            .await?;
            // A Peer Snapshot says what was seen at one instant on the Server
            // clock; the presence interval and the history bucket both belong to
            // that instant, so a replayed snapshot produces its own window instead
            // of one stretched across the catch-up (ADR 0014, #274).
            //
            // The snapshot carries its own observation instant, so the history
            // instant is taken without the receipt-time fallback that freshness
            // allows: borrowing the receipt instant here is exactly the
            // compression of history this rule forbids. Only a snapshot that
            // names no instant at all falls back, and then the receipt instant is
            // the only instant the Server can defend.
            let peers_observed_at = match peers.latest_observed_at.or(peers.attempted_at) {
                Some(observed) => stamp.history_at(observed).to_string(),
                None => stamp.observed_at(None),
            };
            let presence_delta = match snapshot {
                None => PeerPresenceDelta::default(),
                Some(snapshot) => {
                    save_peer_presence(
                        tx,
                        &node_id,
                        snapshot,
                        &peers_observed_at,
                        had_previous_successful_snapshot,
                        &stored_peers,
                    )
                    .await?
                }
            };
            if let Some(snapshot) = snapshot {
                // The current Peer set is a projection write under an idempotent
                // report: it keeps the receipt instant, the freshness of its value
                // comes from the peers component's own observation stamp.
                save_current_peers(tx, &node_id, snapshot, received_at, &stored_peers).await?;
                let local_head = node.chain.sync.latest.map(|sync| sync.current_block);
                crate::peer_history::record_successful_snapshot(
                    tx,
                    &node_id,
                    snapshot,
                    crate::peer_history::SnapshotInstants {
                        observed_at: &peers_observed_at,
                        received_at,
                    },
                    local_head,
                    presence_delta,
                    geo_provider,
                )
                .await?;
            }
        }
        if let Some(process) = node.process.latest {
            sqlx::query("INSERT INTO current_node_process_observations (node_id, pid, started_at, cpu_percent, memory_bytes, uptime_ms, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT(node_id) DO UPDATE SET pid=excluded.pid, started_at=excluded.started_at, cpu_percent=excluded.cpu_percent, memory_bytes=excluded.memory_bytes, uptime_ms=excluded.uptime_ms, updated_at=excluded.updated_at")
                .bind(&node_id).bind(process.pid as i64).bind(process.started_at.to_string()).bind(process.cpu_percent).bind(process.memory_bytes as i64).bind(process.uptime_ms as i64).bind(received_at).execute(&mut **tx).await?;
        }
        if let Some(size_bytes) = node
            .data_directory_size_bytes
            .as_ref()
            .and_then(|observation| observation.latest)
        {
            sqlx::query("INSERT INTO current_node_data_directory_observations (node_id, size_bytes, updated_at) VALUES ($1, $2, $3) ON CONFLICT(node_id) DO UPDATE SET size_bytes=excluded.size_bytes, updated_at=excluded.updated_at")
                .bind(&node_id)
                .bind(size_bytes as i64)
                .bind(received_at)
                .execute(&mut **tx)
                .await?;
        }
        if let Some(capacity_bytes) = node
            .data_directory_capacity_bytes
            .as_ref()
            .and_then(|observation| observation.latest)
        {
            sqlx::query("UPDATE current_node_data_directory_observations SET capacity_bytes=$1, updated_at=$2 WHERE node_id=$3")
                .bind(capacity_bytes as i64)
                .bind(received_at)
                .bind(&node_id)
                .execute(&mut **tx)
                .await?;
        }
        if let (Some(process), Some(observed_at)) =
            (node.process.latest, metric_observed_at(&node.process))
        {
            save_metric_sample(
                tx,
                history,
                &crate::metric_history::SeriesScope::node(&node_id, "process_cpu_percent"),
                stamp,
                observed_at,
                &raw_cutoff,
                process.cpu_percent,
            )
            .await?;
            if let Some(memory) = host.memory.latest.filter(|memory| memory.total_bytes > 0) {
                save_metric_sample(
                    tx,
                    history,
                    &crate::metric_history::SeriesScope::node(&node_id, "process_memory_percent"),
                    stamp,
                    observed_at,
                    &raw_cutoff,
                    process.memory_bytes as f64 * 100.0 / memory.total_bytes as f64,
                )
                .await?;
            }
        }
        if let (Some(size), Some(capacity), Some(size_observed_at), Some(capacity_observed_at)) = (
            node.data_directory_size_bytes
                .as_ref()
                .and_then(|observation| observation.latest),
            node.data_directory_capacity_bytes
                .as_ref()
                .and_then(|observation| observation.latest)
                .filter(|capacity| *capacity > 0),
            node.data_directory_size_bytes
                .as_ref()
                .and_then(metric_observed_at),
            node.data_directory_capacity_bytes
                .as_ref()
                .and_then(metric_observed_at),
        ) {
            // The ratio is only as fresh as the older of the two readings it
            // divides: stamping it at the newer one would present a carried
            // size over a new capacity (or the reverse) as a fresh observation
            // of the percentage.
            save_metric_sample(
                tx,
                history,
                &crate::metric_history::SeriesScope::node(&node_id, "data_directory_percent"),
                stamp,
                size_observed_at.min(capacity_observed_at),
                &raw_cutoff,
                size as f64 * 100.0 / capacity as f64,
            )
            .await?;
        }
        if let Some(peers) = node.chain.peers.as_ref()
            && let (Some(snapshot), Some(observed_at)) =
                (peers.latest.as_ref(), metric_observed_at(peers))
        {
            let inbound = snapshot
                .peers
                .iter()
                .filter(|peer| peer.direction == PeerDirection::Inbound)
                .count();
            let outbound = snapshot.peers.len().saturating_sub(inbound);
            save_metric_sample(
                tx,
                history,
                &crate::metric_history::SeriesScope::node(&node_id, "peer_inbound_count"),
                stamp,
                observed_at,
                &raw_cutoff,
                inbound as f64,
            )
            .await?;
            save_metric_sample(
                tx,
                history,
                &crate::metric_history::SeriesScope::node(&node_id, "peer_outbound_count"),
                stamp,
                observed_at,
                &raw_cutoff,
                outbound as f64,
            )
            .await?;
        }
        // Issue #217: the key sync and consensus heights are numeric history, so
        // they ride the metric engine with the other Node series: the same raw
        // window, the same one-minute and five-minute tiers, the same gaps and
        // the same replay/correction ledger. A probe that failed writes no row
        // here at all, because an absent observation is Unknown rather than a
        // restatement of the last reading; the failure itself is recorded by the
        // state log above.
        //
        // A block height is an integer and a metric sample is a real. Every
        // height a chain can reach is far below 2^53, so the stored value is the
        // height exactly and the history keeps the precision it was measured at.
        // A height beyond that bound would be rounded by the sample: the canonical
        // fixture carries 2^53 + 1 to prove the JSON layer itself loses nothing.
        if let (Some(sync), Some(observed_at)) =
            (node.chain.sync.latest, metric_observed_at(&node.chain.sync))
        {
            for (metric, height) in [
                ("sync_current_block", sync.current_block),
                ("sync_highest_block", sync.highest_block),
            ] {
                save_metric_sample(
                    tx,
                    history,
                    &crate::metric_history::SeriesScope::node(&node_id, metric),
                    stamp,
                    observed_at,
                    &raw_cutoff,
                    height as f64,
                )
                .await?;
            }
        }
        if let (Some(consensus), Some(observed_at)) = (
            node.chain.consensus.latest,
            metric_observed_at(&node.chain.consensus),
        ) {
            for (metric, height) in [
                ("consensus_highest_qc_block", consensus.highest_qc_block),
                ("consensus_highest_lock_block", consensus.highest_lock_block),
                (
                    "consensus_highest_commit_block",
                    consensus.highest_commit_block,
                ),
            ] {
                save_metric_sample(
                    tx,
                    history,
                    &crate::metric_history::SeriesScope::node(&node_id, metric),
                    stamp,
                    observed_at,
                    &raw_cutoff,
                    height as f64,
                )
                .await?;
            }
        }
        if node.chain.rpc.latest.is_some()
            || node.chain.sync.latest.is_some()
            || node.chain.consensus.latest.is_some()
            || node.chain.network_identity.latest.is_some()
            || node.chain.static_metadata.latest.is_some()
        {
            let sync = node.chain.sync.latest;
            let current = sync.map(|v| v.current_block as i64);
            let current_timestamp = report.generated_at.to_string();
            // `observation_at` answers "when was this head seen" and is the instant
            // head freshness and head-lag evidence are judged on; `updated_at` stays
            // the receipt instant because the resync grace window measures how long
            // the Server has been watching the episode (ADR 0014, issue #274).
            let head_observed_at = stamp
                .observed_at(metric_observed_at(&node.chain.sync).or(Some(report.generated_at)));
            // Only a head that actually advances is progress. Every report
            // re-sends the latest sample, so bumping the timestamp
            // unconditionally would turn the "(last progress ...)" diagnostic
            // into a clock instead of an answer.
            sqlx::query("INSERT INTO block_history_state (node_id, current_head, resync_state, resync_last_progress_at, updated_at, observation_at) VALUES ($1, $2, 'normal', $3, $4, $5) ON CONFLICT(node_id) DO UPDATE SET current_head=excluded.current_head, resync_last_progress_at=CASE WHEN excluded.current_head IS NOT NULL AND (block_history_state.current_head IS NULL OR excluded.current_head > block_history_state.current_head) THEN excluded.resync_last_progress_at ELSE block_history_state.resync_last_progress_at END, updated_at=excluded.updated_at, observation_at=excluded.observation_at")
                .bind(&node_id)
                .bind(current)
                .bind(&current_timestamp)
                .bind(received_at)
                .bind(&head_observed_at)
                .execute(&mut **tx)
                .await?;
            // The polled Chain Head can trail the subscribed Block History by
            // the blocks that arrived between polls, so a single
            // below-high-water sample is observation skew, not a resync. An
            // episode has to persist for the grace window before it flips the
            // projection; resync_started_at marks the episode start and is
            // cleared once the head re-reaches the high-water mark.
            // Whether the open episode has outlasted the grace window is judged
            // on parsed instants; an episode or update instant that cannot be
            // parsed has not outlasted it.
            let episode: Option<(Option<String>, String)> = sqlx::query_as(
                "SELECT resync_started_at, updated_at FROM block_history_state WHERE node_id=$1",
            )
            .bind(&node_id)
            .fetch_optional(&mut **tx)
            .await?;
            let grace_elapsed = episode.is_some_and(|(started_at, updated_at)| {
                let started_at = started_at.as_deref().and_then(crate::auth::parse_rfc3339);
                let updated_at = crate::auth::parse_rfc3339(&updated_at);
                matches!((started_at, updated_at), (Some(started_at), Some(updated_at))
                    if updated_at - started_at
                        >= time::Duration::seconds(RESYNC_TRANSITION_GRACE_SECONDS))
            });
            sqlx::query("UPDATE block_history_state SET resync_state=CASE WHEN current_head IS NULL OR historical_high_watermark IS NULL THEN resync_state WHEN current_head >= historical_high_watermark THEN 'normal' WHEN resync_started_at IS NOT NULL AND $1 = 1 THEN 'resyncing' ELSE resync_state END, resync_started_at=CASE WHEN current_head IS NULL OR historical_high_watermark IS NULL THEN resync_started_at WHEN current_head >= historical_high_watermark THEN NULL ELSE COALESCE(resync_started_at, updated_at) END WHERE node_id=$2")
                .bind(i64::from(grace_elapsed))
                .bind(&node_id)
                .execute(&mut **tx)
                .await?;

            let rpc = node.chain.rpc.latest.as_ref();
            let sync = node.chain.sync.latest;
            let consensus = node.chain.consensus.latest;
            let identity = node.chain.network_identity.latest.as_ref();
            let metadata = node.chain.static_metadata.latest.as_ref();
            sqlx::query("INSERT INTO current_node_chain_observations (node_id, rpc_client_version, syncing, current_block, highest_block, pulled_states, known_states, consensus_epoch, consensus_view_number, consensus_validator, consensus_highest_qc_block, consensus_highest_lock_block, consensus_highest_commit_block, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, node_key_fingerprint, enode, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20) ON CONFLICT(node_id) DO UPDATE SET rpc_client_version=COALESCE(excluded.rpc_client_version, current_node_chain_observations.rpc_client_version), syncing=COALESCE(excluded.syncing, current_node_chain_observations.syncing), current_block=COALESCE(excluded.current_block, current_node_chain_observations.current_block), highest_block=COALESCE(excluded.highest_block, current_node_chain_observations.highest_block), pulled_states=COALESCE(excluded.pulled_states, current_node_chain_observations.pulled_states), known_states=COALESCE(excluded.known_states, current_node_chain_observations.known_states), consensus_epoch=COALESCE(excluded.consensus_epoch, current_node_chain_observations.consensus_epoch), consensus_view_number=COALESCE(excluded.consensus_view_number, current_node_chain_observations.consensus_view_number), consensus_validator=COALESCE(excluded.consensus_validator, current_node_chain_observations.consensus_validator), consensus_highest_qc_block=COALESCE(excluded.consensus_highest_qc_block, current_node_chain_observations.consensus_highest_qc_block), consensus_highest_lock_block=COALESCE(excluded.consensus_highest_lock_block, current_node_chain_observations.consensus_highest_lock_block), consensus_highest_commit_block=COALESCE(excluded.consensus_highest_commit_block, current_node_chain_observations.consensus_highest_commit_block), network_genesis_hash=COALESCE(excluded.network_genesis_hash, current_node_chain_observations.network_genesis_hash), network_chain_id=COALESCE(excluded.network_chain_id, current_node_chain_observations.network_chain_id), network_p2p_network_id=COALESCE(excluded.network_p2p_network_id, current_node_chain_observations.network_p2p_network_id), network_address_hrp=COALESCE(excluded.network_address_hrp, current_node_chain_observations.network_address_hrp), node_key_fingerprint=COALESCE(excluded.node_key_fingerprint, current_node_chain_observations.node_key_fingerprint), enode=COALESCE(excluded.enode, current_node_chain_observations.enode), updated_at=excluded.updated_at")
                .bind(&node_id).bind(rpc.map(|v| &v.client_version)).bind(sync.map(|v| v.syncing as i64)).bind(sync.map(|v| v.current_block as i64)).bind(sync.map(|v| v.highest_block as i64)).bind(sync.map(|v| v.pulled_states as i64)).bind(sync.map(|v| v.known_states as i64)).bind(consensus.map(|v| v.epoch as i64)).bind(consensus.map(|v| v.view_number as i64)).bind(consensus.map(|v| v.validator as i64)).bind(consensus.map(|v| v.highest_qc_block as i64)).bind(consensus.map(|v| v.highest_lock_block as i64)).bind(consensus.map(|v| v.highest_commit_block as i64)).bind(identity.map(|v| v.genesis_hash.to_string())).bind(identity.map(|v| v.chain_id as i64)).bind(identity.map(|v| v.p2p_network_id as i64)).bind(identity.and_then(|v| v.address_hrp.as_deref())).bind(metadata.map(|v| v.node_key_fingerprint.to_string())).bind(metadata.and_then(|v| v.enode.as_deref())).bind(received_at).execute(&mut **tx).await?;
            if let Some(rpc) = rpc {
                sqlx::query("DELETE FROM current_node_rpc_namespaces WHERE node_id = $1")
                    .bind(&node_id)
                    .execute(&mut **tx)
                    .await?;
                for chunk in rpc.namespaces.chunks(RPC_INSERT_ROWS) {
                    let sql = crate::database::number_placeholders(&format!(
                        "INSERT INTO current_node_rpc_namespaces (node_id, namespace, updated_at) VALUES {}",
                        crate::database::row_placeholders(3, chunk.len())
                    ));
                    let mut statement = sqlx::query(&sql);
                    for namespace in chunk {
                        statement = statement.bind(&node_id).bind(namespace).bind(received_at);
                    }
                    statement.execute(&mut **tx).await?;
                }
                sqlx::query("DELETE FROM current_node_rpc_methods WHERE node_id = $1")
                    .bind(&node_id)
                    .execute(&mut **tx)
                    .await?;
                for chunk in rpc.methods.chunks(RPC_INSERT_ROWS) {
                    let sql = crate::database::number_placeholders(&format!(
                        "INSERT INTO current_node_rpc_methods (node_id, method, updated_at) VALUES {}",
                        crate::database::row_placeholders(3, chunk.len())
                    ));
                    let mut statement = sqlx::query(&sql);
                    for method in chunk {
                        statement = statement.bind(&node_id).bind(method).bind(received_at);
                    }
                    statement.execute(&mut **tx).await?;
                }
            }
        }
    }
    for sample in &report.block_summaries {
        let node_id = sample.node_id.to_string();
        let registered_identity = sqlx::query_as::<_, (String, i64, i64, String)>("SELECT genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks n JOIN nodes nd ON nd.network_key = n.network_key WHERE nd.node_id = $1")
            .bind(&node_id)
            .fetch_optional(&mut **tx)
            .await?;
        let identity_matches = registered_identity.is_some_and(|(genesis, chain_id, p2p, hrp)| {
            sample.network_identity.genesis_hash.to_string() == genesis
                && sample.network_identity.chain_id == chain_id as u64
                && sample.network_identity.p2p_network_id == p2p as u64
                && sample.network_identity.address_hrp.as_deref().unwrap_or("") == hrp
        });
        if !identity_matches {
            continue;
        }

        let proposer = match &sample.attribution.protocol_proposer {
            platpulse_core::block::ProtocolProposer::Verified { identity } => {
                ("verified", Some(identity.as_str()))
            }
            platpulse_core::block::ProtocolProposer::Unknown {} => ("unknown", None),
        };
        let signer = match sample.attribution.seal_signer_match {
            platpulse_core::block::SealSignerMatch::SignerSelf => "self",
            platpulse_core::block::SealSignerMatch::Other => "other",
            platpulse_core::block::SealSignerMatch::Unknown => "unknown",
        };
        if sample.source == platpulse_core::block::BlockSource::GapBackfill
            && coverage_allows_backfill(&mut *tx, &node_id, sample.block_number)
                .await?
                .is_none()
        {
            continue;
        }
        if sample.source == platpulse_core::block::BlockSource::Subscription {
            let historical_high_watermark: Option<i64> = sqlx::query_scalar(
                "SELECT historical_high_watermark FROM block_history_state WHERE node_id=$1",
            )
            .bind(&node_id)
            .fetch_optional(&mut **tx)
            .await?;
            if historical_high_watermark.is_some_and(|height| sample.block_number as i64 <= height)
            {
                continue;
            }
        }
        let coverage_id = if sample.source == platpulse_core::block::BlockSource::GapBackfill {
            coverage_allows_backfill(&mut *tx, &node_id, sample.block_number).await?
        } else {
            None
        };
        let inserted = sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, block_interval_ms, source, coinbase, seal_signer_key_fingerprint, seal_signer_match, node_key_fingerprint, node_key_valid_from, node_key_valid_until, node_key_history_complete, seal_recovery_rule, seal_evidence, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, accepted_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26) ON CONFLICT DO NOTHING")
            .bind(&node_id).bind(sample.block_number as i64).bind(sample.block_hash.to_string()).bind(sample.parent_hash.to_string())
            .bind(sample.network_identity.genesis_hash.to_string()).bind(sample.network_identity.chain_id as i64).bind(sample.network_identity.p2p_network_id as i64).bind(sample.network_identity.address_hrp.as_deref().unwrap_or(""))
            .bind(sample.block_timestamp_ms as i64).bind(stamp.history_at(sample.observed_at).to_string()).bind(sample.transaction_count as i64).bind(sample.block_interval_ms.map(|v| v as i64)).bind(match sample.source { platpulse_core::block::BlockSource::Subscription => "subscription", platpulse_core::block::BlockSource::GapBackfill => "gap_backfill" })
            .bind(sample.attribution.coinbase.to_string()).bind(sample.attribution.seal_signer_key_fingerprint.as_ref().map(ToString::to_string)).bind(signer).bind(sample.attribution.node_key.as_ref().map(|key| key.fingerprint.to_string())).bind(sample.attribution.node_key.as_ref().and_then(|key| key.valid_from.map(|value| value.to_string()))).bind(sample.attribution.node_key.as_ref().and_then(|key| key.valid_until.map(|value| value.to_string()))).bind(sample.attribution.node_key.as_ref().is_some_and(|key| key.history_complete) as i64).bind(sample.attribution.seal_recovery_rule.as_deref()).bind(sample.attribution.seal_evidence.as_deref()).bind(proposer.0).bind(proposer.1).bind(&sample.attribution.attribution_reason).bind(received_at)
            .execute(&mut **tx).await?;
        if inserted.rows_affected() == 0 {
            continue;
        }
        if let Some(coverage_id) = coverage_id {
            close_coverage_height(&mut *tx, coverage_id, sample.block_number, received_at).await?;
        } else {
            append_coverage_height(&mut *tx, &node_id, sample.block_number, received_at).await?;
        }

        sqlx::query("INSERT INTO observed_network_heads (node_id, block_number, block_hash, observed_at, confidence, eligible_sources) VALUES ($1, $2, $3, $4, 'unknown', '[\\\"subscription\\\"]') ON CONFLICT(node_id) DO UPDATE SET block_number=excluded.block_number, block_hash=excluded.block_hash, observed_at=excluded.observed_at, confidence=excluded.confidence, eligible_sources=excluded.eligible_sources")
            .bind(&node_id).bind(sample.block_number as i64).bind(sample.block_hash.to_string()).bind(stamp.history_at(sample.observed_at).to_string()).execute(&mut **tx).await?;
        // last_accepted_summary_at is the durable proof that this Node's
        // history was accepted again (ADR 0011 decision 4). The Admin refusal
        // diagnosis compares its newest Server-authored gap against it, so the
        // diagnosis clears when history resumes and stays clear after
        // retention ages the raw Block Summary rows out; updated_at cannot
        // serve that purpose because the current-head projection writes it on
        // every Report.
        sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, last_accepted_summary_at, updated_at) VALUES ($1, $2, 1, $3, $4, $5, $6) ON CONFLICT(node_id) DO UPDATE SET historical_high_watermark=CASE WHEN block_history_state.historical_high_watermark >= excluded.historical_high_watermark THEN block_history_state.historical_high_watermark ELSE excluded.historical_high_watermark END, cumulative_block_count=block_history_state.cumulative_block_count + 1, cumulative_transaction_count=block_history_state.cumulative_transaction_count + excluded.cumulative_transaction_count, cumulative_self_seal_count=block_history_state.cumulative_self_seal_count + excluded.cumulative_self_seal_count, last_accepted_summary_at=excluded.last_accepted_summary_at, updated_at=excluded.updated_at")
            .bind(&node_id).bind(sample.block_number as i64).bind(sample.transaction_count as i64)
            .bind((sample.attribution.seal_signer_match == platpulse_core::block::SealSignerMatch::SignerSelf) as i64).bind(received_at).bind(received_at).execute(&mut **tx).await?;
        // The block tiers (issue #248) advance exactly where the cumulative
        // counters do: only for a Block Summary this transaction has just
        // inserted, so Block history dedup decides for both.
        let observed_at = stamp.history_at(sample.observed_at).to_string();
        crate::block_aggregates::record_accepted_block(
            tx,
            crate::block_aggregates::AcceptedBlock {
                node_id: &node_id,
                block_number: sample.block_number as i64,
                block_timestamp_ms: sample.block_timestamp_ms as i64,
                observed_at: &observed_at,
                transaction_count: sample.transaction_count as i64,
            },
            received_at,
        )
        .await?;
    }
    Ok(())
}

/// Outcome of evaluating one Agent's Inventory declaration of a Node it does
/// not yet own (design §4.6). A declaration only wins ownership when a
/// pending transfer targets the reporting Agent, the declared Network key
/// matches the registered Node, and the observed Network Identity matches
/// the Registry tuple; everything else keeps the source Agent authoritative.
#[derive(Debug)]
enum TransferResolution {
    /// No pending transfer covers this declaration; ownership stays put.
    NoTransfer,
    /// The declaration completed the pending transfer; the reporting Agent
    /// owns the Node from this transaction on.
    Completed,
    /// A pending transfer exists but the report carries no usable identity
    /// observation yet; the transfer stays pending and ownership stays put.
    Unverified,
    /// The declared Network key differs from the registered Network; the
    /// transfer is terminally rejected and ownership stays put.
    NetworkKeyMismatch,
    /// The observed identity contradicts the Registry tuple; the transfer
    /// ends in a typed `identity_mismatch` outcome and ownership stays put.
    IdentityMismatch,
}

/// Evaluate a non-owner Inventory declaration against the pending transfer
/// state, inside the ingestion transaction. A successful resolution switches
/// `nodes.agent_id` in the same transaction that accepts the report, so the
/// source stays authoritative until the switch is atomic (issue #46).
async fn resolve_node_transfer<I: ReportInventory>(
    tx: &mut Transaction<'_, Any>,
    report: &AgentReport<I>,
    node: &platpulse_core::inventory::InventoryNode,
    reporting_agent: &str,
    now_text: &str,
) -> Result<TransferResolution, sqlx::Error> {
    // Materialize transfers that expired before this declaration so the
    // history never shows a stale pending row for an expired handover.
    let expired = sqlx::query(
        "UPDATE node_transfers SET status='expired', updated_at=$1 WHERE node_id=$2 AND target_agent_id=$3 AND status='pending' AND expires_at <= $4",
    )
    .bind(now_text)
    .bind(node.node_id.to_string())
    .bind(reporting_agent)
    .bind(now_text)
    .execute(&mut **tx)
    .await?;
    if expired.rows_affected() > 0 {
        crate::auth::insert_audit_event(
            &mut **tx,
            None,
            "node_transfer_expired",
            "node",
            &node.node_id.to_string(),
            Some(&serde_json::json!({ "expired_at": now_text })),
        )
        .await?;
    }
    let pending = sqlx::query_as::<_, (String, String)>(
        "SELECT transfer_id, source_agent_id FROM node_transfers WHERE node_id=$1 AND target_agent_id=$2 AND status='pending' AND expires_at > $3 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(node.node_id.to_string())
    .bind(reporting_agent)
    .bind(now_text)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((transfer_id, source_agent_id)) = pending else {
        return Ok(TransferResolution::NoTransfer);
    };
    let registered = sqlx::query_as::<_, (String, String, i64, i64, String)>(
        "SELECT n.network_key, r.genesis_hash, r.chain_id, r.p2p_network_id, r.address_hrp FROM nodes n JOIN networks r ON r.network_key=n.network_key WHERE n.node_id=$1",
    )
    .bind(node.node_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    let Some((registered_key, genesis, chain_id, p2p, hrp)) = registered else {
        return Ok(TransferResolution::NoTransfer);
    };
    if node.network_key.as_str() != registered_key {
        sqlx::query(
            "UPDATE node_transfers SET status='rejected', rejection_code='network_key_mismatch', rejection_reason=$1, updated_at=$2 WHERE transfer_id=$3",
        )
        .bind("target declared the Node under a different Network key than the registered Network")
        .bind(now_text)
        .bind(&transfer_id)
        .execute(&mut **tx)
        .await?;
        crate::auth::insert_audit_event(
            &mut **tx,
            None,
            "node_transfer_rejected",
            "node",
            &node.node_id.to_string(),
            Some(&serde_json::json!({
                "transfer_id": transfer_id,
                "rejection_code": "network_key_mismatch",
            })),
        )
        .await?;
        return Ok(TransferResolution::NetworkKeyMismatch);
    }
    // The identity authority is the report's own Node observation: a
    // declaration without a successful identity probe cannot switch
    // ownership (source remains authoritative until validation passes).
    let identity = report
        .nodes
        .iter()
        .find(|observation| observation.node_id == node.node_id)
        .filter(|observation| observation.chain.network_identity.status == ComponentStatus::Ok)
        .and_then(|observation| observation.chain.network_identity.latest.as_ref());
    let Some(identity) = identity else {
        return Ok(TransferResolution::Unverified);
    };
    let mut mismatched_fields = Vec::new();
    if identity.genesis_hash.to_string() != genesis {
        mismatched_fields.push("genesis_hash");
    }
    if identity.chain_id != chain_id as u64 {
        mismatched_fields.push("chain_id");
    }
    if identity.p2p_network_id != p2p as u64 {
        mismatched_fields.push("p2p_network_id");
    }
    if identity.address_hrp.as_deref().unwrap_or("") != hrp {
        mismatched_fields.push("address_hrp");
    }
    if !mismatched_fields.is_empty() {
        sqlx::query(
            "UPDATE node_transfers SET status='identity_mismatch', rejection_code='identity_mismatch', rejection_reason=$1, mismatched_fields=$2, updated_at=$3 WHERE transfer_id=$4",
        )
        .bind("the target-declared Network identity contradicts the registered Network; ownership stays with the source Agent")
        .bind(serde_json::to_string(&mismatched_fields).expect("fields serialize"))
        .bind(now_text)
        .bind(&transfer_id)
        .execute(&mut **tx)
        .await?;
        crate::auth::insert_audit_event(
            &mut **tx,
            None,
            "node_transfer_identity_mismatch",
            "node",
            &node.node_id.to_string(),
            Some(&serde_json::json!({
                "transfer_id": transfer_id,
                "mismatched_fields": mismatched_fields,
            })),
        )
        .await?;
        return Ok(TransferResolution::IdentityMismatch);
    }
    sqlx::query(
        "UPDATE node_transfers SET status='completed', completed_at=$1, updated_at=$2 WHERE transfer_id=$3",
    )
    .bind(now_text)
    .bind(now_text)
    .bind(&transfer_id)
    .execute(&mut **tx)
    .await?;
    // Ownership lives in `nodes.agent_id` AND in the composite FK
    // `component_status(agent_id, node_id)` rows of the same Node. Both
    // sides must move in this transaction; FK enforcement is deferred to
    // the commit so the parent and child rows switch atomically (and the
    // pragma resets automatically when the transaction ends).
    crate::database::defer_foreign_keys(tx).await?;
    sqlx::query("UPDATE nodes SET agent_id=$1, updated_at=$2 WHERE node_id=$3")
        .bind(reporting_agent)
        .bind(now_text)
        .bind(node.node_id.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE component_status SET agent_id=$1 WHERE node_id=$2")
        .bind(reporting_agent)
        .bind(node.node_id.to_string())
        .execute(&mut **tx)
        .await?;
    // The switch is a recorded relationship change (issue #221): the previous
    // Agent's interval closes at this instant and the reporting Agent's opens.
    // The Network is passed as the registered key, which this path has already
    // verified, so a transfer never records a Network change.
    crate::relationships::record_relations(
        tx,
        &node.node_id.to_string(),
        reporting_agent,
        &registered_key,
        now_text,
    )
    .await?;
    crate::auth::insert_audit_event(
        &mut **tx,
        None,
        "node_transfer_completed",
        "node",
        &node.node_id.to_string(),
        Some(&serde_json::json!({
            "transfer_id": transfer_id,
            "source_agent_id": source_agent_id,
            "target_agent_id": reporting_agent,
        })),
    )
    .await?;
    Ok(TransferResolution::Completed)
}

pub(crate) fn validate_report_body_size(len: usize) -> Result<(), StatusCode> {
    if len > platpulse_core::protocol::MAX_REPORT_BODY_BYTES {
        Err(StatusCode::PAYLOAD_TOO_LARGE)
    } else {
        Ok(())
    }
}

async fn handler(
    State(state): State<AppState>,
    Extension(auth): Extension<AgentAuthInfo>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    let parsed: AgentReport = match serde_json::from_slice(&body) {
        Ok(report) => report,
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_report",
                "Agent report is invalid",
            );
        }
    };
    ingest_report(state, auth, request_id, body, parsed).await
}

/// v2 ingestion entry point: the report declares Inventory content only and the
/// Server allocates the accepted revision and canonical fingerprint.
async fn handler_v2(
    State(state): State<AppState>,
    Extension(auth): Extension<AgentAuthInfo>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    let parsed: AgentReport<InventoryDeclaration> = match serde_json::from_slice(&body) {
        Ok(report) => report,
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_report",
                "Agent report is invalid",
            );
        }
    };
    ingest_report(state, auth, request_id, body, parsed).await
}

async fn ingest_report<I: ReportInventory>(
    state: AppState,
    auth: AgentAuthInfo,
    request_id: RequestId,
    body: Bytes,
    parsed: AgentReport<I>,
) -> Response {
    // Only the authenticated Agent identity is trusted before validation; the
    // report identity is attached once the report validated and matched it.
    let span = tracing::Span::current();
    span.record("agent_id", auth.agent_id.as_str());
    if state.is_shutting_down() {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "shutting_down",
            "Server is shutting down",
        );
    }
    let _ingestion = match state.ingestion_guard() {
        Some(guard) => guard,
        None => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "shutting_down",
                "Server is shutting down",
            );
        }
    };
    if validate_report_body_size(body.len()).is_err() {
        return error(
            &request_id.0,
            StatusCode::PAYLOAD_TOO_LARGE,
            "report_too_large",
            "Agent report exceeds the protocol size limit",
        );
    }
    let digest = Sha256::digest(&body);
    let hash = Sha256Hex::from_str(&format!("0x{digest:x}")).expect("SHA-256 output is valid");
    if parsed.validate().is_err() {
        return error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_report",
            "Agent report is invalid",
        );
    }
    if parsed.agent_id.to_string() != auth.agent_id {
        return error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "agent_identity_mismatch",
            "Agent identity is not authorized",
        );
    }
    span.record("boot_id", tracing::field::display(&parsed.boot_id));
    span.record("report_id", tracing::field::display(&parsed.report_id));
    span.record("report_sequence", parsed.report_sequence);
    tracing::debug!(
        "report received with {} Node observation(s)",
        parsed.nodes.len()
    );
    // Issue #192: a converted deployment stays gated until the explicit cutover
    // resume. No collection/ingestion, Admin mutation or external-effect worker
    // is authorized before then, so a converted database can never be served as
    // if the switch had happened.
    let cutover = state.db().inventory_cutover();
    if cutover.business_writes_blocked() {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "inventory_cutover_required",
            "Converted deployment awaits the coordinated Inventory cutover; stop the Server and run 'platpulse-server cutover resume'",
        );
    }
    // Acceptance below reads this Agent's active Boot, last accepted sequence
    // and Inventory revision and then writes the next state from them, which is
    // a read-then-write invariant two concurrent Reports of one Agent would
    // break (ADR 0007, ADR 0013): the newer Report could commit first and the
    // older one afterwards, moving the sequence and the Current Projection
    // backwards. The fence is taken before the transaction so a waiting Report
    // holds no connection, and it is per Agent, so Reports of different Agents
    // still ingest in parallel. SQLite's single write connection made this hold
    // already; the PostgreSQL work connections do not.
    let _agent_fence = state.ingest_fence().hold(&auth.agent_id).await;
    // Hold the retention sweep off this receipt: retention and ingestion write
    // the same series-state rows, and on PostgreSQL the deadlock that follows
    // may abort the Report. Hygiene must never be able to refuse a Report, so
    // the sweep waits for the receipts in flight instead.
    let _sweep_off = crate::retention::hold_sweep_off().await;
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(db_error) => {
            state.note_database_error(&db_error);
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };

    let existing = match sqlx::query_as::<_, ReceiptRow>(
        "SELECT report_body_sha256, receipt_body FROM agent_report_receipts WHERE report_id = $1",
    )
    .bind(parsed.report_id.to_string())
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(row) => row,
        Err(db_error) => {
            state.note_database_error(&db_error);
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if let Some(existing) = existing {
        if existing.report_body_sha256 != hash.to_string() {
            return error(
                &request_id.0,
                StatusCode::CONFLICT,
                "report_identity_conflict",
                "Report identity conflicts with a stored report",
            );
        }
        // Replay never bypasses the live Agent boundary: a removed Agent's
        // retained Receipt is not returned even when the immutable bytes match
        // (design 15.10.5, issue #192).
        match sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM agents WHERE agent_id = $1 AND deleted_at IS NULL",
        )
        .bind(&auth.agent_id)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                return error(
                    &request_id.0,
                    StatusCode::UNAUTHORIZED,
                    "agent_auth_required",
                    "Agent credential is invalid",
                );
            }
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        }
        let receipt = match replay_receipt::<I>(&existing.receipt_body) {
            Ok(receipt) => receipt,
            Err(()) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Stored receipt is unavailable",
                );
            }
        };
        return receipt_response(receipt);
    }
    // After the coordinated cutover the frozen v1 route is replay-only. An
    // unseen v1 Report is explicitly unsupported: it creates no acceptance
    // Receipt, advances no Boot, allocates no revision and changes no
    // projection. It is not mixed-version ingestion, an automatic downgrade or
    // an old-backlog submission path (design 15.10.5).
    if cutover.v1_replay_only()
        && I::protocol_version() != platpulse_core::protocol::PROTOCOL_VERSION_V2
    {
        return error(
            &request_id.0,
            StatusCode::CONFLICT,
            "report_not_replayable",
            "Frozen v1 Reports are replay-only after the Inventory cutover; new Reports must use protocol v2",
        );
    }
    // A removed Agent carries the durable removal boundary (design §15.2,
    // #171). Reading the row inside the receipt transaction with the live
    // predicate makes a late report fail exactly like an invalid credential,
    // so no later Inventory, retry, or restart can reconstruct its Nodes.
    let agent = match sqlx::query_as::<_, AgentRow>(
        "SELECT agent_epoch, active_boot_id, active_boot_status, previous_boot_id, close_report_id, last_report_sequence, last_inventory_revision FROM agents WHERE agent_id = $1 AND deleted_at IS NULL",
    )
    .bind(&auth.agent_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error(
                &request_id.0,
                StatusCode::UNAUTHORIZED,
                "agent_auth_required",
                "Agent credential is invalid",
            );
        }
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if parsed.agent_epoch != agent.agent_epoch as u64 {
        return store_rejected(
            &state,
            tx,
            &parsed,
            hash.clone(),
            rejected(
                parsed.report_id,
                hash,
                platpulse_core::RejectionCode::StaleBoot,
                "Agent epoch is stale",
            ),
            &request_id.0,
        )
        .await;
    }

    let old_boot_status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM agent_boots WHERE agent_id=$1 AND agent_epoch=$2 AND boot_id=$3",
    )
    .bind(&auth.agent_id)
    .bind(parsed.agent_epoch as i64)
    .bind(parsed.boot_id.to_string())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ())
    .ok()
    .flatten();
    let _boot_markers = (&agent.previous_boot_id, &agent.close_report_id);
    let declared_revision = parsed.inventory.declared_revision();
    // Frozen v1: the Agent declares its own monotonically increasing revision.
    if let Some(declared) = declared_revision {
        if declared < agent.last_inventory_revision as u64 {
            return store_rejected(
                &state,
                tx,
                &parsed,
                hash.clone(),
                rejected(
                    parsed.report_id,
                    hash,
                    platpulse_core::RejectionCode::InventoryRevisionConflict,
                    "Inventory revision is older than the accepted revision",
                ),
                &request_id.0,
            )
            .await;
        }
    }

    let inventory_hash = parsed.inventory.accepted_sha256();
    // An Agent without an accepted Inventory stores NULL: no prior hash.
    let prior_inventory_hash: Option<String> = match sqlx::query_scalar::<_, Option<String>>(
        "SELECT inventory_sha256 FROM agents WHERE agent_id=$1",
    )
    .bind(&auth.agent_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(hash) => hash.flatten(),
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if let Some(declared) = declared_revision {
        if declared == agent.last_inventory_revision as u64
            && prior_inventory_hash.is_some()
            && prior_inventory_hash.as_deref() != Some(inventory_hash.as_str())
        {
            return store_rejected(
                &state,
                tx,
                &parsed,
                hash.clone(),
                rejected(
                    parsed.report_id,
                    hash,
                    platpulse_core::RejectionCode::InventoryRevisionConflict,
                    "Inventory content conflicts at the accepted revision",
                ),
                &request_id.0,
            )
            .await;
        }
    }

    // v2 declares content only: the Server allocates the accepted revision inside
    // this authoritative transaction. Identical content keeps the accepted
    // revision; any content change (including A -> B -> A) advances it. The
    // comparison and allocation commit with the receipt, so concurrent reports
    // can never allocate the same revision twice.
    let inventory_unchanged = prior_inventory_hash.as_deref() == Some(inventory_hash.as_str());
    let effective_inventory_revision: u64 = match declared_revision {
        Some(declared) => declared,
        None if inventory_unchanged => agent.last_inventory_revision.max(0) as u64,
        None => {
            // The column is a positive signed 64-bit integer, so the ceiling is
            // i64::MAX even though the arithmetic runs in u64. The checked add
            // plus the signed-range filter keep exhaustion an explicit refusal
            // instead of a cast that would wrap negative.
            let last = agent.last_inventory_revision.max(0) as u64;
            let Some(next) = last.checked_add(1).filter(|next| *next <= i64::MAX as u64) else {
                return store_rejected(
                    &state,
                    tx,
                    &parsed,
                    hash.clone(),
                    rejected(
                        parsed.report_id,
                        hash,
                        platpulse_core::RejectionCode::InventoryRevisionConflict,
                        "Inventory revision range is exhausted",
                    ),
                    &request_id.0,
                )
                .await;
            };
            next
        }
    };

    // A sequence is unique within one boot. Never silently accept a competing body.
    let sequence_conflict = match sqlx::query_scalar::<_, String>("SELECT report_id FROM agent_report_receipts WHERE agent_id = $1 AND agent_epoch = $2 AND boot_id = $3 AND report_sequence = $4")
        .bind(&auth.agent_id).bind(parsed.agent_epoch as i64).bind(parsed.boot_id.to_string()).bind(parsed.report_sequence as i64).fetch_optional(&mut *tx).await {
            Ok(v) => v, Err(_) => return error(&request_id.0, StatusCode::SERVICE_UNAVAILABLE, "unavailable", "Server database is unavailable")
    };
    if sequence_conflict.is_some() {
        if record_security_event(&mut tx, &auth.agent_id)
            .await
            .is_err()
            || tx.commit().await.is_err()
        {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
        return error(
            &request_id.0,
            StatusCode::CONFLICT,
            "conflicting_boot",
            "Boot sequence conflicts with a stored report",
        );
    }
    if let Some(active) = &agent.active_boot_id {
        if active != &parsed.boot_id.to_string() {
            if old_boot_status.as_deref() == Some("closed")
                && parsed.boot_transition != platpulse_core::BootTransition::DrainedPrevious
            {
                return store_rejected(
                    &state,
                    tx,
                    &parsed,
                    hash.clone(),
                    rejected(
                        parsed.report_id,
                        hash,
                        platpulse_core::RejectionCode::StaleBoot,
                        "Report belongs to a closed boot; only exact replay is accepted",
                    ),
                    &request_id.0,
                )
                .await;
            }
            if parsed.boot_transition != platpulse_core::BootTransition::DrainedPrevious
                || parsed
                    .previous_boot_id
                    .as_ref()
                    .map(ToString::to_string)
                    .as_deref()
                    != Some(active.as_str())
            {
                if let Err(failure) = record_security_event(&mut tx, &auth.agent_id).await {
                    return storage_error(&request_id.0, "record security event", &failure);
                }
                return store_rejected(
                    &state,
                    tx,
                    &parsed,
                    hash.clone(),
                    rejected(
                        parsed.report_id,
                        hash,
                        platpulse_core::RejectionCode::ConflictingBoot,
                        "Report belongs to a competing boot",
                    ),
                    &request_id.0,
                )
                .await;
            }
            if agent.active_boot_status != "closed" {
                if let Err(failure) = record_security_event(&mut tx, &auth.agent_id).await {
                    return storage_error(&request_id.0, "record security event", &failure);
                }
                return store_rejected(
                    &state,
                    tx,
                    &parsed,
                    hash.clone(),
                    rejected(
                        parsed.report_id,
                        hash,
                        platpulse_core::RejectionCode::ConflictingBoot,
                        "Previous boot has not completed its closing receipt",
                    ),
                    &request_id.0,
                )
                .await;
            }
        } else if agent.active_boot_status == "closing"
            || agent.active_boot_status == "closed"
            || agent
                .last_report_sequence
                .is_some_and(|last| parsed.report_sequence <= last as u64)
        {
            return store_rejected(
                &state,
                tx,
                &parsed,
                hash.clone(),
                rejected(
                    parsed.report_id,
                    hash,
                    platpulse_core::RejectionCode::StaleReport,
                    "Report sequence is not newer than the accepted report",
                ),
                &request_id.0,
            )
            .await;
        }
    }
    if let Some(last) = agent.last_report_sequence
        && parsed.boot_id.to_string() == agent.active_boot_id.as_deref().unwrap_or_default()
        && parsed.report_sequence > (last as u64).saturating_add(1)
    {
        // Every statement in the ingestion transaction must succeed: a backend
        // that aborts the transaction on a failed statement (ADR 0013) would
        // otherwise fail the whole commit later with no clue why.
        if let Err(failure) = sqlx::query("INSERT INTO report_sequence_gaps (agent_id, boot_id, from_sequence, to_sequence, created_at) VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING")
            .bind(&auth.agent_id)
            .bind(parsed.boot_id.to_string())
            .bind(last.saturating_add(1))
            .bind(parsed.report_sequence.saturating_sub(1) as i64)
            .bind(now().to_string())
            .execute(&mut *tx)
            .await
        {
            return storage_error(&request_id.0, "record report sequence gap", &failure);
        }
    }
    let now_text = now().to_string();
    // Every observation instant this Report contributes is read through the
    // stamp: the Report's own measured skew turns Agent-clock instants into
    // Server-clock ones, and the receipt instant stays the fallback (ADR 0014).
    let stamp = ReportStamp::new(&now_text, parsed.host.clock_skew.latest);
    let capabilities =
        serde_json::to_string(&parsed.agent_capabilities).expect("capabilities serialize");
    let mut ownership_mismatches = std::collections::HashSet::new();
    let mut purged_nodes = std::collections::HashSet::new();
    let mut ownership_contradiction = false;
    for node in parsed.inventory.nodes() {
        // A purged Node ID is a permanent admission boundary (design §15.3,
        // ADR 0004, issue #170): the Owner already erased this Node's
        // projection, monitoring history, and Validator Links, and no later
        // Inventory revision, retry, restart, or background write may
        // reconstruct them. Reject the entry per Node so a valid sibling in
        // the same legal report still reports, and do not let a stale or
        // unknown Network key on the removed ID block the rest of the report.
        let purged = match crate::node_purge::is_purged(&mut tx, &node.node_id.to_string()).await {
            Ok(value) => value,
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        if purged {
            purged_nodes.insert(node.node_id);
            continue;
        }
        let known = match sqlx::query_scalar::<_, String>(
            "SELECT network_key FROM networks WHERE network_key = $1",
        )
        .bind(node.network_key.as_str())
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(v) => v,
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        if known.is_none() {
            return store_rejected(
                &state,
                tx,
                &parsed,
                hash.clone(),
                rejected(
                    parsed.report_id,
                    hash,
                    platpulse_core::RejectionCode::NetworkKeyUnknown,
                    "Network key is not registered",
                ),
                &request_id.0,
            )
            .await;
        }
        let owner =
            match sqlx::query_scalar::<_, String>("SELECT agent_id FROM nodes WHERE node_id = $1")
                .bind(node.node_id.to_string())
                .fetch_optional(&mut *tx)
                .await
            {
                Ok(v) => v,
                Err(_) => {
                    return error(
                        &request_id.0,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "Server database is unavailable",
                    );
                }
            };
        if owner.is_some_and(|owner| owner != auth.agent_id) {
            // The reporting Agent declared a Node owned by another Agent.
            // Only a pending transfer targeting this Agent with a validated
            // Network Identity may switch ownership (design §4.6); every
            // other outcome keeps the source authoritative and is a
            // contradiction worth a security event.
            match resolve_node_transfer(&mut tx, &parsed, node, &auth.agent_id, &now_text).await {
                Ok(TransferResolution::Completed) => {
                    // Ownership switched atomically; this Node is no longer a
                    // mismatch and its current/samples join this report.
                }
                Ok(TransferResolution::Unverified) => {
                    // Legitimate in-flight declaration without an identity
                    // probe yet: the transfer stays pending and the Node
                    // entry stays rejected for this report.
                    ownership_mismatches.insert(node.node_id);
                }
                Ok(TransferResolution::NoTransfer)
                | Ok(TransferResolution::NetworkKeyMismatch)
                | Ok(TransferResolution::IdentityMismatch) => {
                    ownership_mismatches.insert(node.node_id);
                    ownership_contradiction = true;
                }
                Err(_) => {
                    return error(
                        &request_id.0,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "Server database is unavailable",
                    );
                }
            }
        }
    }
    // A non-owner declaration without a valid pending transfer (or with a
    // rejected/contradicting transfer) is a security conflict: the source
    // Agent's later submissions after a completed Transfer also land here
    // and must be visible and auditable (design §4.6). The counter is
    // recorded once per report together with block identity mismatches.
    // Inventory is accepted as a complete set only after structural/network
    // validation. Ownership-invalid Nodes reject only their own current and
    // samples; they are not allowed to retire valid siblings.
    // A Node is inadmissible when it belongs to another Agent or its ID was
    // explicitly purged. Both are per-Node outcomes; the ordinary ownership
    // rejection still records its security event below, while a purge is a
    // stable, non-security disposition.
    let mut rejected_nodes = ownership_mismatches.clone();
    rejected_nodes.extend(purged_nodes.iter().copied());
    let accepted_inventory_ids = parsed
        .inventory
        .nodes()
        .iter()
        .filter(|node| !rejected_nodes.contains(&node.node_id))
        .map(|node| node.node_id.to_string())
        .collect::<Vec<_>>();

    // Equal content is unchanged; do not retire siblings or rewrite inventory
    // ownership on a replay. A newly accepted declaration remains authoritative.
    if !inventory_unchanged && parsed.inventory.nodes().is_empty() {
        if sqlx::query("UPDATE nodes SET lifecycle='retired', updated_at=$1 WHERE agent_id=$2")
            .bind(&now_text)
            .bind(&auth.agent_id)
            .execute(&mut *tx)
            .await
            .is_err()
        {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    } else if !inventory_unchanged && !accepted_inventory_ids.is_empty() {
        let placeholders = std::iter::repeat_n("?", accepted_inventory_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE nodes SET lifecycle='retired', updated_at=? WHERE agent_id=? AND node_id NOT IN ({placeholders})"
        );
        let sql = crate::database::number_placeholders(&sql);
        let mut query = sqlx::query(&sql).bind(&now_text).bind(&auth.agent_id);
        for node_id in &accepted_inventory_ids {
            query = query.bind(node_id);
        }
        if query.execute(&mut *tx).await.is_err() {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    }
    for node in parsed.inventory.nodes() {
        if rejected_nodes.contains(&node.node_id) {
            continue;
        }
        let result = sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, $2, $3, $4, $5, 'active', 'private', $6, $7, $8) ON CONFLICT(node_id) DO UPDATE SET network_key=excluded.network_key, display_name=COALESCE(nodes.display_name, excluded.display_name), rpc_endpoint=excluded.rpc_endpoint, lifecycle='active', inventory_revision=excluded.inventory_revision, updated_at=excluded.updated_at")
            .bind(node.node_id.to_string()).bind(&auth.agent_id).bind(node.network_key.as_str()).bind(&node.display_name).bind(node.rpc_endpoint.as_str()).bind(effective_inventory_revision as i64).bind(&now_text).bind(&now_text).execute(&mut *tx).await;
        if result.is_err() {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
        // Record the relations the Server accepted with this Report (issue
        // #221). A first Report opens the intervals; a change of reporting
        // Agent or Network key closes the old interval and opens the new one,
        // so history is never read from the Node's current columns.
        if crate::relationships::record_relations(
            &mut tx,
            &node.node_id.to_string(),
            &auth.agent_id,
            node.network_key.as_str(),
            &now_text,
        )
        .await
        .is_err()
        {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    }
    let identity_rejections =
        match block_network_identity_rejections(&mut tx, &parsed, &purged_nodes).await {
            Ok(value) => value,
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
    if !identity_rejections.is_empty() {
        // Identity mismatch is a typed, audited outcome (design §4.5): the
        // samples are never merged and the Agent's security counter records
        // the contradiction so the Admin surface can surface it.
        ownership_contradiction = true;
    }
    if ownership_contradiction {
        if let Err(failure) = record_security_event(&mut tx, &auth.agent_id).await {
            return storage_error(&request_id.0, "record security event", &failure);
        }
    }
    let mut outside_open_gap: std::collections::HashSet<(platpulse_core::identity::NodeId, u64)> =
        std::collections::HashSet::new();
    for sample in &parsed.block_summaries {
        if sample.source != platpulse_core::block::BlockSource::GapBackfill {
            continue;
        }
        match coverage_allows_backfill(&mut tx, &sample.node_id.to_string(), sample.block_number)
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                outside_open_gap.insert((sample.node_id, sample.block_number));
            }
            Err(failure) => {
                return storage_error(&request_id.0, "check open gap coverage", &failure);
            }
        }
    }
    let mut replay_samples: std::collections::HashSet<(platpulse_core::identity::NodeId, u64)> =
        std::collections::HashSet::new();
    let mut divergence_samples: std::collections::HashSet<(platpulse_core::identity::NodeId, u64)> =
        std::collections::HashSet::new();
    for sample in &parsed.block_summaries {
        // History mutation is ownership-scoped: a sample from a Node the
        // reporting Agent does not own must never touch the identity
        // window, even when its declared identity happens to match the
        // Registry (issue #46: mismatch can never merge new history).
        if rejected_nodes.contains(&sample.node_id) {
            continue;
        }
        let node_id = sample.node_id.to_string();
        let registered_identity = match sqlx::query_as::<_, (String, i64, i64, String)>("SELECT genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks n JOIN nodes nd ON nd.network_key = n.network_key WHERE nd.node_id = $1")
            .bind(&node_id)
            .fetch_optional(&mut *tx)
            .await {
                Ok(value) => value,
                Err(_) => return error(&request_id.0, StatusCode::SERVICE_UNAVAILABLE, "unavailable", "Server database is unavailable"),
            };
        let identity_matches = registered_identity.is_some_and(|(genesis, chain_id, p2p, hrp)| {
            sample.network_identity.genesis_hash.to_string() == genesis
                && sample.network_identity.chain_id == chain_id as u64
                && sample.network_identity.p2p_network_id == p2p as u64
                && sample.network_identity.address_hrp.as_deref().unwrap_or("") == hrp
        });
        if !identity_matches {
            continue;
        }
        if sample.source == platpulse_core::block::BlockSource::GapBackfill
            && outside_open_gap.contains(&(sample.node_id, sample.block_number))
        {
            continue;
        }
        let high: Option<i64> = match sqlx::query_scalar(
            "SELECT historical_high_watermark FROM block_history_state WHERE node_id=$1",
        )
        .bind(&node_id)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(v) => v,
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        if sample.source == platpulse_core::block::BlockSource::Subscription
            && high.is_some_and(|value| sample.block_number as i64 <= value)
        {
            replay_samples.insert((sample.node_id, sample.block_number));
            continue;
        }
        let retained: Option<String> = match sqlx::query_scalar(
            "SELECT block_hash FROM block_identity_window WHERE node_id=$1 AND height=$2",
        )
        .bind(&node_id)
        .bind(sample.block_number as i64)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(v) => v,
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        let candidate = sample.source == platpulse_core::block::BlockSource::GapBackfill
            || high.is_none_or(|value| sample.block_number as i64 > value)
            || retained.is_some();
        if !candidate {
            continue;
        }
        let block_observed_at = stamp.history_at(sample.observed_at);
        match observe_block_identity(&mut tx, sample, block_observed_at, &now_text).await {
            Ok(true) => {
                divergence_samples.insert((sample.node_id, sample.block_number));
            }
            Ok(false) => {}
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        }
    }

    let mut projection_report = parsed.clone();
    projection_report
        .nodes
        .retain(|node| !rejected_nodes.contains(&node.node_id));
    projection_report
        .block_summaries
        .retain(|sample| !rejected_nodes.contains(&sample.node_id));
    let geo_provider = state.geo_config().provider;
    // The low-space gate is read once from memory for this Report. It is never
    // read through the pool: ingestion already holds the single write
    // connection, and a second acquire inside the transaction would deadlock.
    let history = state.capacity().history_gate();
    if let Err(save_error) =
        save_current(&mut tx, &history, &projection_report, stamp, geo_provider).await
    {
        return storage_error(&request_id.0, "save current observations", &save_error);
    }
    // Only admitted Nodes feed the Network-scoped reference head; a purged
    // Node's unvalidated Network key must not reach shared Network state.
    let network_keys = projection_report
        .inventory
        .nodes()
        .iter()
        .filter(|node| !rejected_nodes.contains(&node.node_id))
        .map(|node| node.network_key.to_string())
        .collect::<Vec<_>>();
    if update_network_references(&mut tx, &network_keys, &now_text)
        .await
        .is_err()
    {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    for sample in &parsed.block_summaries {
        // The Server records the History Gap for a rejected sample itself, so
        // the reason operators read is the Server's own diagnosis of what it
        // rejected and why, not an Agent claim about itself.
        //
        // The row is upserted rather than ignored so the Server's text wins
        // even when the Agent recorded the same (node, from, to, kind) triple
        // first: the unique gap index folds the Agent's row into this one, and
        // authored_by is what lets the Admin surface present only the rows the
        // Server wrote as its own live refusal (AGENTS.md: the Server is the
        // trust boundary and never presents Agent input as its own evidence).
        let reason = if ownership_mismatches.contains(&sample.node_id) {
            "Node ownership mismatch".to_owned()
        } else if let Some(evidence) = identity_rejections.get(&StampedIdentityKey::new(
            sample.node_id,
            &sample.network_identity,
        )) {
            crate::redaction::redact_sensitive(&evidence.reason())
        } else {
            continue;
        };
        let inserted = sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, $2, $3, 'server_rejected', $4, $5, 'server') ON CONFLICT(node_id, from_height, to_height, kind) DO UPDATE SET reason=excluded.reason, created_at=excluded.created_at, authored_by=excluded.authored_by")
            .bind(sample.node_id.to_string())
            .bind(sample.block_number as i64)
            .bind(sample.block_number as i64)
            .bind(&reason)
            .bind(&now_text)
            .execute(&mut *tx)
            .await;
        if inserted.is_err() {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    }
    for gap in &parsed.history_gaps {
        if rejected_nodes.contains(&gap.node_id) {
            continue;
        }
        let kind = match gap.kind {
            platpulse_core::gap::GapKind::UnrecoverableBackfill => "unrecoverable_backfill",
            platpulse_core::gap::GapKind::SpoolOverflow => "spool_overflow",
            platpulse_core::gap::GapKind::ServerRejected => "server_rejected",
        };
        // An Agent-declared gap is Agent evidence: it is stored with its
        // author so the Server never presents it as its own refusal, and it
        // never overwrites the Server's own text for the same triple.
        let inserted = sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, $2, $3, $4, $5, $6, 'agent') ON CONFLICT DO NOTHING")
            .bind(gap.node_id.to_string()).bind(gap.from_height as i64).bind(gap.to_height as i64)
            .bind(kind)
            .bind(crate::redaction::redact_sensitive(&gap.reason))
            .bind(gap.recorded_at.to_string())
            .execute(&mut *tx)
            .await;
        if inserted.is_err() {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
        if matches!(
            gap.kind,
            platpulse_core::gap::GapKind::UnrecoverableBackfill
        ) && open_coverage_gap(
            &mut tx,
            &gap.node_id.to_string(),
            gap.from_height,
            gap.to_height,
            &now_text,
        )
        .await
        .is_err()
        {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    }
    let nodes: Vec<NodeReceipt> = parsed
        .inventory
        .nodes()
        .iter()
        .map(|node| {
            let rejected = rejected_nodes.contains(&node.node_id);
            NodeReceipt {
                node_id: node.node_id,
                current: if rejected {
                    NodeCurrentDisposition::Rejected
                } else {
                    NodeCurrentDisposition::Accepted
                },
                accepted_component_revisions: if rejected {
                    vec![]
                } else if let Some(observed) = parsed
                    .nodes
                    .iter()
                    .find(|candidate| candidate.node_id == node.node_id)
                {
                    let mut revisions = vec![
                        ComponentRevision {
                            component: ComponentKey::Process,
                            state_revision: observed.process.state_revision,
                            value_revision: observed.process.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::Rpc,
                            state_revision: observed.chain.rpc.state_revision,
                            value_revision: observed.chain.rpc.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::Sync,
                            state_revision: observed.chain.sync.state_revision,
                            value_revision: observed.chain.sync.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::Consensus,
                            state_revision: observed.chain.consensus.state_revision,
                            value_revision: observed.chain.consensus.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::NetworkIdentity,
                            state_revision: observed.chain.network_identity.state_revision,
                            value_revision: observed.chain.network_identity.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::StaticMetadata,
                            state_revision: observed.chain.static_metadata.state_revision,
                            value_revision: observed.chain.static_metadata.value_revision,
                        },
                    ];
                    if let Some(data_directory_size) = &observed.data_directory_size_bytes {
                        revisions.push(ComponentRevision {
                            component: ComponentKey::DataDirectorySizeBytes,
                            state_revision: data_directory_size.state_revision,
                            value_revision: data_directory_size.value_revision,
                        });
                    }
                    if let Some(data_directory_capacity) = &observed.data_directory_capacity_bytes {
                        revisions.push(ComponentRevision {
                            component: ComponentKey::DataDirectoryCapacityBytes,
                            state_revision: data_directory_capacity.state_revision,
                            value_revision: data_directory_capacity.value_revision,
                        });
                    }
                    if let Some(peers) = &observed.chain.peers {
                        revisions.push(ComponentRevision {
                            component: ComponentKey::Peers,
                            state_revision: peers.state_revision,
                            value_revision: peers.value_revision,
                        });
                    }
                    revisions
                } else {
                    vec![]
                },
                rejections: if purged_nodes.contains(&node.node_id) {
                    vec![purged_rejection()]
                } else if rejected {
                    vec![ownership_mismatch_rejection()]
                } else {
                    vec![]
                },
            }
        })
        .collect();
    let samples = parsed
        .block_summaries
        .iter()
        .map(|sample| {
            let identity_rejection = identity_rejections.get(&StampedIdentityKey::new(
                sample.node_id,
                &sample.network_identity,
            ));
            let rejected = rejected_nodes.contains(&sample.node_id)
                || identity_rejection.is_some()
                || outside_open_gap.contains(&(sample.node_id, sample.block_number))
                || divergence_samples.contains(&(sample.node_id, sample.block_number))
                || replay_samples.contains(&(sample.node_id, sample.block_number));
            SampleDisposition {
                node_id: sample.node_id,
                sample: SampleRef::Block {
                    height: sample.block_number,
                },
                disposition: if rejected {
                    SampleDispositionKind::TerminalRejected
                } else {
                    SampleDispositionKind::Accepted
                },
                rejection: rejected.then(|| {
                    if purged_nodes.contains(&sample.node_id) {
                        return purged_rejection();
                    }
                    if ownership_mismatches.contains(&sample.node_id) {
                        return ownership_mismatch_rejection();
                    }
                    // A contradicted stamp is the more specific finding about
                    // this sample, and it is what the Server's own History Gap
                    // row for the same height already records, so it outranks
                    // the generic refusals below: the Receipt must state the
                    // same reason as the gap it leaves behind (ADR 0011
                    // decision 4). Samples whose stamp matches the Registry
                    // never carry a diagnosis here, so the other refusals keep
                    // their priority for every other sample.
                    if let Some(evidence) = identity_rejection {
                        return rejection(
                            platpulse_core::RejectionCode::NetworkIdentityMismatch,
                            &evidence.reason(),
                        );
                    }
                    if outside_open_gap.contains(&(sample.node_id, sample.block_number)) {
                        return rejection(
                            platpulse_core::RejectionCode::GapBackfillOutsideOpenGap,
                            "GapBackfill sample is outside an explicit open recoverable gap",
                        );
                    }
                    if divergence_samples.contains(&(sample.node_id, sample.block_number)) {
                        return rejection(
                            platpulse_core::RejectionCode::ChainDivergence,
                            "Block hash diverges from retained Node identity evidence",
                        );
                    }
                    if replay_samples.contains(&(sample.node_id, sample.block_number)) {
                        return rejection(
                            platpulse_core::RejectionCode::ResyncReplay,
                            "Normal resync replay at or below the historical high-water mark",
                        );
                    }
                    // Only a rejected sample with no identity diagnosis and no
                    // other attribution reaches here; the bounded fallback
                    // keeps every rejection typed.
                    rejection(
                        platpulse_core::RejectionCode::NetworkIdentityMismatch,
                        "Block network identity does not match the registered Network",
                    )
                }),
            }
        })
        .chain(parsed.history_gaps.iter().map(|gap| {
            let rejected = rejected_nodes.contains(&gap.node_id);
            SampleDisposition {
                node_id: gap.node_id,
                sample: SampleRef::Gap {
                    from_height: gap.from_height,
                    to_height: gap.to_height,
                },
                disposition: if rejected {
                    SampleDispositionKind::TerminalRejected
                } else {
                    SampleDispositionKind::Accepted
                },
                rejection: rejected.then(|| {
                    if purged_nodes.contains(&gap.node_id) {
                        purged_rejection()
                    } else {
                        ownership_mismatch_rejection()
                    }
                }),
            }
        }))
        .collect::<Vec<_>>();
    let inventory_changed = !inventory_unchanged;
    let inventory_disposition = if inventory_unchanged {
        InventoryDisposition::Unchanged
    } else {
        InventoryDisposition::Accepted
    };
    let disposition = if nodes
        .iter()
        .any(|node| node.current == NodeCurrentDisposition::Rejected)
        || samples
            .iter()
            .any(|sample| sample.disposition != SampleDispositionKind::Accepted)
    {
        ReceiptDisposition::PartiallyAccepted
    } else {
        ReceiptDisposition::Accepted
    };
    let acceptance = if declared_revision.is_none() {
        Some(InventoryAcceptance {
            revision: effective_inventory_revision,
            fingerprint: inventory_hash.clone(),
        })
    } else {
        None
    };
    let receipt = if declared_revision.is_none() {
        IngestReceipt::V2(Box::new(ReportReceiptV2 {
            report_id: parsed.report_id,
            disposition,
            report_body_sha256: hash.clone(),
            server_version: crate::VERSION.to_owned(),
            supported_protocol_majors: SUPPORTED_PROTOCOL_MAJORS.to_vec(),
            server_time: now(),
            rotation_hint: None,
            inventory: Some(InventoryReceiptV2 {
                disposition: inventory_disposition,
                acceptance,
            }),
            rejections: vec![],
            nodes,
            samples,
        }))
    } else {
        IngestReceipt::V1(Box::new(ReportReceipt {
            report_id: parsed.report_id,
            disposition,
            report_body_sha256: hash.clone(),
            server_version: crate::VERSION.to_owned(),
            supported_protocol_majors: SUPPORTED_PROTOCOL_MAJORS.to_vec(),
            server_time: now(),
            rotation_hint: None,
            inventory: Some(inventory_disposition),
            rejections: vec![],
            nodes,
            samples,
        }))
    };
    let stored = receipt.to_bytes();
    // The same immutable Report can be delivered twice at once: a sender
    // deadline that expired while the first attempt was still committing, or a
    // reconnect replay. The insert therefore has to be conflict tolerant
    // instead of an error path. A unique violation aborts the surrounding
    // PostgreSQL transaction, so the recovery read below could never run and
    // the second attempt surfaced as 503 storage_conflict; ON CONFLICT DO
    // NOTHING reports the loser as zero rows and leaves the transaction
    // usable, and the Receipt the winner stored answers both attempts.
    let inserted = sqlx::query("INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT (report_id) DO NOTHING").bind(parsed.report_id.to_string()).bind(&auth.agent_id).bind(parsed.agent_epoch as i64).bind(parsed.boot_id.to_string()).bind(parsed.report_sequence as i64).bind(hash.to_string()).bind(disposition_name(disposition)).bind(&stored).bind(&now_text).execute(&mut *tx).await;
    match inserted {
        Ok(result) if result.rows_affected() == 1 => {}
        Ok(_) => {
            // Another attempt stored this Report first. Replaying its Receipt
            // is the idempotent answer, but it must respect the same Agent
            // boundary the pre-check applies: a removed Agent never gets a
            // retained Receipt back (design 15.10.5, issue #192).
            let concurrent = match sqlx::query_as::<_, ReceiptRow>(
                "SELECT report_body_sha256, receipt_body FROM agent_report_receipts WHERE report_id = $1",
            )
            .bind(parsed.report_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            {
                Ok(row) => row,
                Err(db_error) => {
                    state.note_database_error(&db_error);
                    return error(
                        &request_id.0,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "Server database is unavailable",
                    );
                }
            };
            let Some(concurrent) = concurrent else {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Stored receipt is unavailable",
                );
            };
            if concurrent.report_body_sha256 != hash.to_string() {
                return error(
                    &request_id.0,
                    StatusCode::CONFLICT,
                    "report_identity_conflict",
                    "Report identity conflicts with a stored report",
                );
            }
            match sqlx::query_scalar::<_, i64>(
                "SELECT 1 FROM agents WHERE agent_id = $1 AND deleted_at IS NULL",
            )
            .bind(&auth.agent_id)
            .fetch_optional(&mut *tx)
            .await
            {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return error(
                        &request_id.0,
                        StatusCode::UNAUTHORIZED,
                        "agent_auth_required",
                        "Agent credential is invalid",
                    );
                }
                Err(db_error) => {
                    state.note_database_error(&db_error);
                    return error(
                        &request_id.0,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "Server database is unavailable",
                    );
                }
            }
            let Ok(receipt) = replay_receipt::<I>(&concurrent.receipt_body) else {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Stored receipt is unavailable",
                );
            };
            let _ = tx.rollback().await;
            return receipt_response(receipt);
        }
        Err(insert_failure) => {
            // PostgreSQL admits two concurrent Reports of one Boot into this
            // insert, so the unique key (agent_id, agent_epoch, boot_id,
            // report_sequence) is the last sequence barrier the earlier read
            // cannot be. A sequence another Report already holds is the same
            // refusal a sequential duplicate receives, not a storage failure.
            // The failed statement aborted this transaction, so the security
            // event is recorded on a fresh one.
            if matches!(
                insert_failure,
                sqlx::Error::Database(ref database) if database.is_unique_violation()
            ) {
                let _ = tx.rollback().await;
                if let Ok(mut retry_tx) = state.db().pool().begin().await {
                    if record_security_event(&mut retry_tx, &auth.agent_id)
                        .await
                        .is_ok()
                    {
                        let _ = retry_tx.commit().await;
                    }
                }
                return error(
                    &request_id.0,
                    StatusCode::CONFLICT,
                    "conflicting_boot",
                    "Boot sequence conflicts with a stored report",
                );
            }
            state.note_database_error(&insert_failure);
            return storage_error(&request_id.0, "store report receipt", &insert_failure);
        }
    }
    let lifecycle_status = match parsed.boot_transition {
        platpulse_core::BootTransition::Closing => "closed",
        _ => "active",
    };
    // `agent_boots_one_active` permits at most one active/closing Boot per
    // (agent_id, agent_epoch). Activating a Boot must be a single atomic write
    // rather than one that relies on `agents.active_boot_status` and the Boot
    // row already agreeing: a manual repair or a crash/fork window can leave a
    // stale active row behind, and the new Boot's first `drained_previous`
    // report would then hard-fail the upsert forever (issue #179). Close every
    // other active/closing row in this epoch first. For any accepted report the
    // only such row is the previous Boot, which this transaction is replacing.
    if let Err(stale_failure) = sqlx::query(
        "UPDATE agent_boots SET status='closed', closed_at=COALESCE(closed_at, $1), updated_at=$2 \
         WHERE agent_id=$3 AND agent_epoch=$4 AND boot_id<>$5 AND status IN ('active','closing')",
    )
    .bind(&now_text)
    .bind(&now_text)
    .bind(&auth.agent_id)
    .bind(parsed.agent_epoch as i64)
    .bind(parsed.boot_id.to_string())
    .execute(&mut *tx)
    .await
    {
        return storage_error(&request_id.0, "close stale agent boot", &stale_failure);
    }
    let boot_upsert = sqlx::query("INSERT INTO agent_boots (agent_id, agent_epoch, boot_id, status, previous_boot_id, last_sequence, close_report_id, closed_at, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, CASE WHEN $7='closed' THEN $8 ELSE NULL END, CASE WHEN $9='closed' THEN $10 ELSE NULL END, $11, $12) ON CONFLICT(agent_id, agent_epoch, boot_id) DO UPDATE SET status=excluded.status, previous_boot_id=COALESCE(excluded.previous_boot_id, agent_boots.previous_boot_id), last_sequence=CASE WHEN agent_boots.last_sequence >= excluded.last_sequence THEN agent_boots.last_sequence ELSE excluded.last_sequence END, close_report_id=COALESCE(excluded.close_report_id, agent_boots.close_report_id), closed_at=COALESCE(excluded.closed_at, agent_boots.closed_at), updated_at=excluded.updated_at")
        .bind(&auth.agent_id)
        .bind(parsed.agent_epoch as i64)
        .bind(parsed.boot_id.to_string())
        .bind(lifecycle_status)
        .bind(parsed.previous_boot_id.map(|v| v.to_string()))
        .bind(parsed.report_sequence as i64)
        .bind(lifecycle_status)
        .bind(parsed.report_id.to_string())
        .bind(lifecycle_status)
        .bind(&now_text)
        .bind(&now_text)
        .bind(&now_text)
        .execute(&mut *tx)
        .await;
    if let Err(boot_failure) = boot_upsert {
        return storage_error(&request_id.0, "upsert agent boot", &boot_failure);
    }
    let clock_skew_ms = parsed.host.clock_skew.latest;
    let clock_status = match clock_skew_ms {
        Some(value) if value.abs() > crate::http::agent::CLOCK_UNRELIABLE_THRESHOLD_MS => {
            "clock_unreliable"
        }
        Some(_) => "known",
        None => "unknown",
    };
    let should_activate_new_boot = parsed.boot_transition
        == platpulse_core::BootTransition::DrainedPrevious
        && parsed
            .previous_boot_id
            .as_ref()
            .map(ToString::to_string)
            .as_deref()
            == agent.active_boot_id.as_deref();
    let shutdown_diag = parsed.host.spool.latest.as_ref();
    let shutdown_state = shutdown_diag
        .and_then(|v| v.shutdown_state.as_deref())
        .unwrap_or(match parsed.boot_transition {
            platpulse_core::BootTransition::Closing => "final_stored",
            _ => "running",
        });
    let shutdown_last_error = shutdown_diag
        .and_then(|value| value.shutdown_last_error.as_deref())
        .map(crate::redaction::redact_sensitive);
    let updated = sqlx::query("UPDATE agents SET active_boot_id=$1, active_boot_status=$2, previous_boot_id=$3, close_report_id=CASE WHEN $4='closed' THEN $5 ELSE close_report_id END, last_report_sequence=$6, last_inventory_revision=$7, inventory_sha256=$8, inventory_protocol_major=$9, last_received_at=$10, clock_skew_ms=$11, clock_status=$12, agent_capabilities_json=$13, shutdown_state=$14, shutdown_started_at=$15, shutdown_deadline_at=$16, shutdown_finished_at=$17, shutdown_unresolved_from=$18, shutdown_unresolved_to=$19, shutdown_last_error=$20, shutdown_forced=$21, shutdown_report_id=CASE WHEN $22='closed' THEN $23 ELSE shutdown_report_id END, shutdown_report_sequence=$24, shutdown_updated_at=$25, updated_at=$26 WHERE agent_id=$27")
        .bind(parsed.boot_id.to_string()).bind(lifecycle_status)
        .bind(parsed.previous_boot_id.map(|v| v.to_string()))
        .bind(lifecycle_status).bind(parsed.report_id.to_string())
        .bind(parsed.report_sequence as i64).bind(effective_inventory_revision as i64)
        .bind(inventory_hash.as_str()).bind(I::protocol_version() as i64)
        .bind(&now_text).bind(clock_skew_ms).bind(clock_status)
        .bind(capabilities)
        .bind(shutdown_state)
        .bind(shutdown_diag.and_then(|v| v.shutdown_started_at.as_ref()).map(ToString::to_string))
        .bind(shutdown_diag.and_then(|v| v.shutdown_deadline_at.as_ref()).map(ToString::to_string))
        .bind(shutdown_diag.and_then(|v| v.shutdown_finished_at.as_ref()).map(ToString::to_string))
        .bind(shutdown_diag.and_then(|v| v.shutdown_unresolved_range.map(|range| range.0 as i64)))
        .bind(shutdown_diag.and_then(|v| v.shutdown_unresolved_range.map(|range| range.1 as i64)))
        .bind(shutdown_last_error.as_deref())
        .bind(shutdown_diag.and_then(|v| v.shutdown_forced).unwrap_or(false) as i64)
        .bind(lifecycle_status)
        .bind(parsed.report_id.to_string())
        .bind(parsed.report_sequence as i64)
        .bind(&now_text)
        .bind(&now_text)
        .bind(&auth.agent_id)
        .execute(&mut *tx)
        .await;
    if let Err(update_failure) = updated {
        return storage_error(&request_id.0, "update agent report state", &update_failure);
    }
    if should_activate_new_boot {
        if let Err(failure) = sqlx::query("UPDATE agents SET active_boot_status='active', previous_boot_id=$1, close_applied_at=$2 WHERE agent_id=$3")
            .bind(parsed.previous_boot_id.map(|v| v.to_string()))
            .bind(&now_text)
            .bind(&auth.agent_id)
            .execute(&mut *tx)
            .await
        {
            return storage_error(&request_id.0, "activate new boot", &failure);
        }
        if let Err(failure) = sqlx::query("UPDATE agent_boots SET status='closed', closed_at=$1, updated_at=$2 WHERE agent_id=$3 AND agent_epoch=$4 AND boot_id=$5")
            .bind(&now_text).bind(&now_text).bind(&auth.agent_id)
            .bind(parsed.agent_epoch as i64)
            .bind(parsed.previous_boot_id.map(|v| v.to_string()))
            .execute(&mut *tx)
            .await
        {
            return storage_error(&request_id.0, "close previous boot", &failure);
        }
    }
    // Alert evaluation runs in the same transaction as the accepted
    // projection (design §8.3: Alert input and invalidation belong to the
    // report transaction), so a transition is never observed without its
    // accepted facts. The sweep re-evaluates the remaining subjects. Only
    // Nodes owned by this Agent may be evaluated: a partially accepted or
    // hostile report must never advance another Agent's Node alert state.
    // Fail safe: without ownership proof no reported Node is evaluated here;
    // the sweep re-evaluates owned subjects on its own schedule.
    let owned: Vec<String> =
        match sqlx::query_scalar("SELECT node_id FROM nodes WHERE agent_id = $1")
            .bind(&auth.agent_id)
            .fetch_all(&mut *tx)
            .await
        {
            Ok(owned) => owned,
            Err(failure) => {
                return storage_error(&request_id.0, "load owned nodes", &failure);
            }
        };
    let node_ids: Vec<String> = parsed
        .nodes
        .iter()
        .map(|node| node.node_id.to_string())
        .filter(|node_id| owned.contains(node_id))
        .collect();
    let alert_changes = match crate::alerts::evaluate_report(
        &mut tx,
        &auth.agent_id,
        &node_ids,
        state.channels(),
        crate::auth::now_utc(),
    )
    .await
    {
        Ok(changes) => changes,
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    let peer_component_present = parsed.nodes.iter().any(|node| node.chain.peers.is_some());
    let public_node_ids = projection_report
        .nodes
        .iter()
        .map(|node| node.node_id.to_string())
        .collect::<Vec<_>>();
    let public_network_keys = projection_report
        .inventory
        .nodes()
        .iter()
        .map(|node| node.network_key.to_string())
        .collect::<HashSet<_>>();
    if let Err(commit_failure) = tx.commit().await {
        return storage_error(&request_id.0, "commit report transaction", &commit_failure);
    }
    // Retention sweeps (issues #213/#214/#217) are bounded hygiene rather than
    // part of the receipt, so they run on the crate::retention::run_worker
    // cadence instead of here. Sweeping inside this handler made a cleanup
    // statement and a concurrent Report lock the same node_metric_series_state
    // / host_metric_series_state rows in opposite orders, which PostgreSQL
    // resolved as a deadlock (40P01) whose victim could have been the Report.
    // Peer references are cache hygiene rather than part of the receipt too:
    // `crate::geo::cleanup_cache` refreshes them, drops the addresses no
    // current Peer references, and applies the cache bound on its own
    // maintenance cycle (issue #271). A Report neither scans nor rewrites the
    // Geo cache, so cache hygiene can never refuse one.
    // Every committed report can change the Owner-side Node/Agent health
    // projection (including host-only reports), while the Public projection
    // changes only when the report contains a Node observation.
    state
        .admin_realtime()
        .publish("node", None::<String>, parsed.report_sequence);
    if inventory_changed || parsed.inventory.nodes().is_empty() {
        state
            .public_realtime()
            .publish("node", None::<String>, parsed.report_sequence);
    }
    if !inventory_changed && !public_node_ids.is_empty() {
        for node_id in &public_node_ids {
            state
                .public_realtime()
                .publish("node", Some(node_id.clone()), parsed.report_sequence);
        }
    }
    for network_key in &public_network_keys {
        state.public_realtime().publish(
            "network",
            Some(network_key.clone()),
            parsed.report_sequence,
        );
    }
    if peer_component_present {
        // A new Peer reference is a reason to resolve its country now; the
        // background path is woken instead of resolving inside this receipt.
        state.notify_geo_backfill();
        state
            .admin_realtime()
            .publish("peer", None::<String>, parsed.report_sequence);
        state
            .admin_realtime()
            .publish("geo", None::<String>, parsed.report_sequence);
        for network_key in &public_network_keys {
            state.public_realtime().publish(
                "network",
                Some(network_key.clone()),
                parsed.report_sequence,
            );
        }
    }
    if alert_changes > 0 {
        state
            .admin_realtime()
            .publish("alerts", None::<String>, parsed.report_sequence);
    }
    receipt_response(receipt)
}

fn valid_report_content_type(value: Option<&axum::http::HeaderValue>) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|media_type| {
                media_type.trim().eq_ignore_ascii_case("application/json")
            })
        })
}

fn report_content_encoding_supported(value: Option<&axum::http::HeaderValue>) -> bool {
    value.is_none()
}

async fn report_request_boundary(request: Request, next: Next) -> Response {
    if request.method() != axum::http::Method::POST {
        return error_from_request(
            &request,
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "report endpoint requires POST",
        );
    }
    if !valid_report_content_type(request.headers().get(axum::http::header::CONTENT_TYPE)) {
        return error_from_request(
            &request,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_content_type",
            "Agent reports require application/json",
        );
    }
    if !report_content_encoding_supported(
        request.headers().get(axum::http::header::CONTENT_ENCODING),
    ) {
        return error_from_request(
            &request,
            StatusCode::BAD_REQUEST,
            "content_encoding_unsupported",
            "compressed Agent reports are not supported",
        );
    }
    next.run(request).await
}

async fn body_size_boundary(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, platpulse_core::protocol::MAX_REPORT_BODY_BYTES + 1).await {
        Ok(bytes) if validate_report_body_size(bytes.len()).is_ok() => bytes,
        Ok(_) => {
            let request = Request::from_parts(parts, Body::empty());
            return error_from_request(
                &request,
                StatusCode::PAYLOAD_TOO_LARGE,
                "report_too_large",
                "Agent report exceeds the protocol size limit",
            );
        }
        Err(_) => {
            let request = Request::from_parts(parts, Body::empty());
            return error_from_request(
                &request,
                StatusCode::PAYLOAD_TOO_LARGE,
                "report_too_large",
                "Agent report exceeds the protocol size limit",
            );
        }
    };
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}
fn error_from_request(
    request: &Request,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| &*id.0)
        .unwrap_or("unknown");
    error(request_id, status, code, message)
}

fn report_layers(router: Router<AppState>) -> Router<AppState> {
    router
        .layer(from_fn(body_size_boundary))
        .layer(from_fn(report_request_boundary))
        .layer(from_fn(
            |request: axum::extract::Request, next: Next| async move {
                let mut response = next.run(request).await;
                response.headers_mut().insert(
                    ROUTE_GROUP_HEADER,
                    axum::http::HeaderValue::from_static("agent"),
                );
                response
            },
        ))
}

pub(crate) fn router() -> Router<AppState> {
    report_layers(Router::<AppState>::new().route("/reports", axum::routing::post(handler)))
}

/// v2 report ingestion router, nested at `/api/agent/v2`.
pub(crate) fn router_v2() -> Router<AppState> {
    report_layers(Router::<AppState>::new().route("/reports", axum::routing::post(handler_v2)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthConfig, format_rfc3339, now_utc};
    use crate::enrollment::{
        AgentAuthInfo, RECOVERY_TOKEN_DEFAULT_LIFETIME, create_recovery_token, recover,
    };
    use crate::network::create_network;
    use crate::secrets::{create_pepper_file, load_pepper_file};
    use axum::body::{Bytes, to_bytes};
    use axum::extract::{Extension, State};
    use axum::http::StatusCode;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn state_with_agent() -> (TempDir, AppState, String) {
        let dir = TempDir::new().unwrap();
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        create_pepper_file(&pepper_path).unwrap();
        let auth = AuthConfig::development(
            load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        create_network(
            &database,
            "platon-mainnet",
            "PlatON Mainnet",
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            210425,
            210425,
            "lat",
        )
        .await
        .unwrap();
        let agent_id = "0195f2a1-0011-4011-8011-000000000011".to_owned();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES ($1, 1, NULL, NULL, NULL, $2, $3)").bind(&agent_id).bind("2026-08-12T08:00:00Z").bind("2026-08-12T08:00:00Z").execute(database.pool()).await.unwrap();
        (dir, AppState::new(database, None, auth), agent_id)
    }

    /// Reopen the same database file the way a restarted Server does: the same
    /// schema, the same rows, and nothing carried over in process memory.
    async fn restart(dir: &TempDir) -> AppState {
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        let auth = AuthConfig::development(
            load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        AppState::new(database, None, auth)
    }

    async fn submit(state: &AppState, agent_id: &str, body: Vec<u8>) -> ReportReceipt {
        let response = handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.to_owned(),
                credential_id: "test-credential".to_owned(),
            }),
            Extension(RequestId(Arc::from("test-request"))),
            Bytes::from(body),
        )
        .await;
        if response.status() != StatusCode::OK {
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            panic!(
                "unexpected report response for agent {agent_id}: {}",
                String::from_utf8_lossy(&body)
            );
        }
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice::<ReportResponse>(&body)
            .unwrap()
            .receipt
    }

    /// The instant the report fixture is written around.
    const FIXTURE_INSTANT: &str = "2026-08-12T09:00:00Z";

    /// Move every instant in a report fixture onto the Server clock, keeping the
    /// fixture's own relative offsets.
    ///
    /// Raw metric samples are kept for the 24 hours of the `raw_metric_sample`
    /// policy (issue #213), so a fixture stamped in August is released the moment
    /// it is stored: a case about an accepted observation has to deliver one the
    /// raw window can still hold.
    fn restamp_fixture(raw: &mut serde_json::Value, now: time::OffsetDateTime) {
        fn walk(value: &mut serde_json::Value, delta: time::Duration) {
            match value {
                serde_json::Value::String(text) => {
                    if let Some(instant) = crate::auth::parse_rfc3339(text) {
                        *text = format_rfc3339(instant + delta);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items.iter_mut() {
                        walk(item, delta);
                    }
                }
                serde_json::Value::Object(fields) => {
                    for field in fields.values_mut() {
                        walk(field, delta);
                    }
                }
                _ => {}
            }
        }
        let anchor =
            crate::auth::parse_rfc3339(FIXTURE_INSTANT).expect("the fixture anchor parses");
        walk(raw, now - anchor);
    }

    /// The report fixture with every instant moved onto the Server clock.
    fn report_near_now() -> AgentReport {
        let mut raw: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        restamp_fixture(&mut raw, now_utc());
        serde_json::from_value(raw).unwrap()
    }

    async fn submit_status(state: &AppState, agent_id: &str, body: Vec<u8>) -> StatusCode {
        handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.to_owned(),
                credential_id: "test-credential".to_owned(),
            }),
            Extension(RequestId(Arc::from("test-request"))),
            Bytes::from(body),
        )
        .await
        .status()
    }

    /// The two Nodes the v2 fixtures declare. A content change in these tests
    /// moves an RPC Endpoint, which the canonical declaration fingerprint covers.
    const V2_NODE_A: &str = "0195f2a1-0014-4014-8014-000000000014";
    const V2_NODE_B: &str = "0195f2a1-0015-4015-8015-000000000015";
    const V2_BOOT_ID: &str = "0195f2a1-0012-4012-8012-000000000012";

    /// A v2 report built from the frozen v2 fixture. The declaration lists one
    /// Node per endpoint and the observation view is regenerated to match, so
    /// the report is a complete current view whose only declared change is the
    /// endpoint (or Node set).
    fn v2_report_body(
        report_id: &str,
        sequence: u64,
        epoch: u64,
        boot_id: &str,
        endpoints: &[&str],
    ) -> Vec<u8> {
        let mut report: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v2_minimal.json"
        ))
        .unwrap();
        let template_node = report["inventory"]["nodes"][0].clone();
        let template_observation = report["nodes"][0].clone();
        let mut inventory_nodes = Vec::new();
        let mut observations = Vec::new();
        for (index, endpoint) in endpoints.iter().enumerate() {
            let node_id = if index == 0 { V2_NODE_A } else { V2_NODE_B };
            let mut node = template_node.clone();
            node["node_id"] = serde_json::json!(node_id);
            node["rpc_endpoint"] = serde_json::json!(endpoint);
            inventory_nodes.push(node);
            let mut observation = template_observation.clone();
            observation["node_id"] = serde_json::json!(node_id);
            observations.push(observation);
        }
        report["agent_epoch"] = serde_json::json!(epoch);
        report["report_sequence"] = serde_json::json!(sequence);
        report["report_id"] = serde_json::json!(report_id);
        report["boot_id"] = serde_json::json!(boot_id);
        report["inventory"]["nodes"] = serde_json::json!(inventory_nodes);
        report["nodes"] = serde_json::json!(observations);
        serde_json::to_vec(&report).unwrap()
    }

    fn declaration_fingerprint(body: &[u8]) -> String {
        let report: AgentReport<InventoryDeclaration> = serde_json::from_slice(body).unwrap();
        report.inventory.fingerprint().to_string()
    }

    async fn submit_v2_raw(
        state: &AppState,
        agent_id: &str,
        body: Vec<u8>,
    ) -> (StatusCode, Vec<u8>) {
        let response = handler_v2(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.to_owned(),
                credential_id: "test-credential".to_owned(),
            }),
            Extension(RequestId(Arc::from("test-request"))),
            Bytes::from(body),
        )
        .await;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, bytes.to_vec())
    }

    async fn submit_v2(state: &AppState, agent_id: &str, body: Vec<u8>) -> ReportReceiptV2 {
        let (status, bytes) = submit_v2_raw(state, agent_id, body).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "unexpected v2 report response: {}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice::<ReportResponseV2>(&bytes)
            .unwrap()
            .receipt
    }

    fn v2_acceptance(receipt: &ReportReceiptV2) -> InventoryAcceptance {
        receipt
            .inventory
            .as_ref()
            .expect("v2 receipt carries an Inventory outcome")
            .acceptance
            .clone()
            .expect("an appliable v2 receipt binds an acceptance pair")
    }

    async fn accepted_inventory_revision(state: &AppState, agent_id: &str) -> i64 {
        sqlx::query_scalar("SELECT last_inventory_revision FROM agents WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap()
    }

    async fn accepted_inventory_fingerprint(state: &AppState, agent_id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT inventory_sha256 FROM agents WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap()
    }

    /// Issue #187: a failed write anywhere in the v2 receipt transaction must
    /// roll back the whole thing — allocation, projection, and receipt — and
    /// publish no post-commit invalidation. An injected trigger is the
    /// controlled failure source; the production path itself is untouched.
    async fn v2_write_failure_rolls_back_all_writes(
        trigger_name: &str,
        timing: &str,
        operation: &str,
        table: &str,
        message: &str,
    ) {
        let (dir, state, agent_id) = state_with_agent().await;
        let body = v2_report_body(
            "0195f2a1-0240-4240-8240-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let events_before = state.admin_realtime().pending_events().len();
        crate::test_support::install_failure_trigger(
            state.db().pool(),
            trigger_name,
            timing,
            operation,
            table,
            message,
        )
        .await
        .unwrap();

        let (status, response) = submit_v2_raw(&state, &agent_id, body.clone()).await;
        assert_ne!(
            status,
            StatusCode::OK,
            "the injected failure must not be accepted: {}",
            String::from_utf8_lossy(&response)
        );

        assert_eq!(
            accepted_inventory_revision(&state, &agent_id).await,
            0,
            "a rollback must not leave an allocated revision"
        );
        assert_eq!(
            accepted_inventory_fingerprint(&state, &agent_id).await,
            None
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_report_receipts")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            0,
            "a rollback must not leave a receipt"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            0,
            "a rollback must not leave a projection"
        );
        assert_eq!(
            state.admin_realtime().pending_events().len(),
            events_before,
            "a rollback must publish no invalidation"
        );

        // Drop the trigger and reopen the real database: the rolled-back state
        // is durable, and no invalidation is replayed across the restart.
        crate::test_support::drop_failure_trigger(state.db().pool(), trigger_name, table)
            .await
            .unwrap();
        state.db().close().await;
        let database = crate::test_support::test_database_in(dir.path()).await;
        let auth = AuthConfig::development(
            load_pepper_file(&dir.path().join("pepper")).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let reopened = AppState::new(database, None, auth);
        assert_eq!(
            accepted_inventory_revision(&reopened, &agent_id).await,
            0,
            "a rollback must stay rolled back after reopening the database"
        );
        assert_eq!(
            accepted_inventory_fingerprint(&reopened, &agent_id).await,
            None
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_report_receipts")
                .fetch_one(reopened.db().pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes")
                .fetch_one(reopened.db().pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(reopened.admin_realtime().pending_events().len(), 0);

        // The database is still usable: the same report now commits revision 1.
        let recovered = submit_v2(&reopened, &agent_id, body).await;
        assert_eq!(v2_acceptance(&recovered).revision, 1);
        assert_eq!(accepted_inventory_revision(&reopened, &agent_id).await, 1);
    }

    #[test]
    fn boundary_rejects_method_content_type_and_encoding() {
        use axum::http::HeaderValue;
        assert!(valid_report_content_type(Some(&HeaderValue::from_static(
            "application/json; charset=utf-8"
        ))));
        assert!(!valid_report_content_type(Some(&HeaderValue::from_static(
            "text/plain"
        ))));
        assert!(report_content_encoding_supported(None));
        assert!(!report_content_encoding_supported(Some(
            &HeaderValue::from_static("gzip")
        )));
        assert!(validate_report_body_size(platpulse_core::protocol::MAX_REPORT_BODY_BYTES).is_ok());
        assert_eq!(
            validate_report_body_size(platpulse_core::protocol::MAX_REPORT_BODY_BYTES + 1),
            Err(StatusCode::PAYLOAD_TOO_LARGE)
        );
    }
    #[tokio::test]
    async fn persists_node_data_directory_size_and_preserves_last_good_value() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_near_now();
        let observed_at = report.generated_at;
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(12_884_901_888),
            error: None,
        });
        report.nodes[0].data_directory_capacity_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(51_539_607_552),
            error: None,
        });

        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert!(
            receipt.nodes[0]
                .accepted_component_revisions
                .iter()
                .any(|revision| revision.component == ComponentKey::DataDirectorySizeBytes)
        );
        assert!(
            receipt.nodes[0]
                .accepted_component_revisions
                .iter()
                .any(|revision| revision.component == ComponentKey::DataDirectoryCapacityBytes)
        );
        let stored: (i64, Option<i64>) = sqlx::query_as(
            "SELECT size_bytes, capacity_bytes FROM current_node_data_directory_observations WHERE node_id=$1",
        )
        .bind(report.nodes[0].node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, (12_884_901_888, Some(51_539_607_552)));
        let disk_sample: (f64, String) = sqlx::query_as(
            "SELECT value, received_at FROM node_metric_samples WHERE node_id=$1 AND metric='data_directory_percent'",
        )
        .bind(report.nodes[0].node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(disk_sample.0, 25.0);
        assert!(!disk_sample.1.is_empty());
        let host_network_samples: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM host_metric_samples WHERE agent_id=$1 AND metric IN ('network_rx_bytes_per_sec', 'network_tx_bytes_per_sec')",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(host_network_samples, 2);

        report.report_sequence += 1;
        report.report_id = "0195f2a1-0090-4090-8090-000000000090".parse().unwrap();
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Error,
            attempted_at: Some(observed_at),
            latest_observed_at: None,
            received_at: None,
            state_revision: 2,
            value_revision: 1,
            latest: None,
            error: Some(platpulse_core::BoundedError {
                code: "data_directory_scan_failed".to_owned(),
                message: "PlatON data directory could not be measured".to_owned(),
            }),
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let stored: i64 = sqlx::query_scalar(
            "SELECT size_bytes FROM current_node_data_directory_observations WHERE node_id=$1",
        )
        .bind(report.nodes[0].node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, 12_884_901_888);

        report.report_sequence += 1;
        report.report_id = "0195f2a1-0091-4091-8091-000000000091".parse().unwrap();
        report.nodes[0].data_directory_size_bytes = None;
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let stored: i64 = sqlx::query_scalar(
            "SELECT size_bytes FROM current_node_data_directory_observations WHERE node_id=$1",
        )
        .bind(report.nodes[0].node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, 12_884_901_888);
    }

    /// Issue #213: the series ledger counts what the series observed. A
    /// restated delivery — the same observation arriving again, including a
    /// carried last-good — is not a new observation, and a restated value that
    /// really did change is counted as a correction instead.
    #[tokio::test]
    async fn replay_and_correction_never_inflate_the_series_observation_count() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_near_now();
        let node_id = report.nodes[0].node_id.to_string();
        let observed_at = report.generated_at;
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(12_884_901_888),
            error: None,
        });
        report.nodes[0].data_directory_capacity_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(51_539_607_552),
            error: None,
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 0, 0)
        );

        // The same observation delivered again in a new Report: the value is
        // already stored, so this is a replay and not a second observation.
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0092-4092-8092-000000000092".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 1, 0)
        );
        let stored_series: (String, String) = sqlx::query_as(
            "SELECT first_observed_at, last_observed_at FROM node_metric_series_state WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored_series.0, observed_at.to_string());
        assert_eq!(stored_series.1, observed_at.to_string());

        // The same instant observed with a different value is a correction: it
        // rewrites the sample without inventing an observation or advancing the
        // series clock.
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0093-4093-8093-000000000093".parse().unwrap();
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 2,
            value_revision: 2,
            latest: Some(25_769_803_776),
            error: None,
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 1, 1)
        );

        // A genuinely newer observation is the only thing that advances the
        // count and the series clock.
        let later: platpulse_core::Rfc3339 =
            format_rfc3339(observed_at.as_datetime() + time::Duration::minutes(5))
                .parse()
                .unwrap();
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0094-4094-8094-000000000094".parse().unwrap();
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(later),
            latest_observed_at: Some(later),
            received_at: None,
            state_revision: 3,
            value_revision: 3,
            latest: Some(12_884_901_888),
            error: None,
        });
        report.nodes[0].data_directory_capacity_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(later),
            latest_observed_at: Some(later),
            received_at: None,
            state_revision: 3,
            value_revision: 3,
            latest: Some(51_539_607_552),
            error: None,
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (2, 1, 1)
        );
        let samples: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_metric_samples WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(samples, 2, "a replay must not add a second sample row");
        let last_observed: String = sqlx::query_scalar(
            "SELECT last_observed_at FROM node_metric_series_state WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(last_observed, later.to_string());
    }

    /// The scenario the review raised: an observation is retained, released by a
    /// cleanup, and then the Operator widens the retention window past it. Its
    /// row is gone from the sample table, so the only evidence that can keep a
    /// carried copy from being counted a second time is the series'
    /// released-before floor, and that floor has to survive a restart.
    #[tokio::test]
    async fn a_retention_widening_cannot_recount_an_observation_it_released() {
        let (dir, state, agent_id) = state_with_agent().await;
        let mut report = report_near_now();
        let node_id = report.nodes[0].node_id.to_string();
        let observed_at = report.generated_at;
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(12_884_901_888),
            error: None,
        });
        report.nodes[0].data_directory_capacity_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(51_539_607_552),
            error: None,
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 0, 0)
        );

        // Time passes. The cleanup runs with a clock far enough ahead that the
        // real one-day cutoff reaches the stored observation: it releases the
        // row and stamps the series with the cutoff it applied.
        let released_at = now_utc() + time::Duration::days(2);
        let removed =
            crate::retention::cleanup_expired_metric_samples(state.db().pool(), released_at)
                .await
                .unwrap();
        assert!(removed >= 1, "the cleanup released the stored observation");
        let released_before: String = sqlx::query_scalar(
            "SELECT released_before FROM node_metric_series_state WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(
            released_before.as_str() > observed_at.to_string().as_str(),
            "the series is stamped with a cutoff the released observation is below: {released_before}"
        );

        // The Operator then widens the window to two days. The observation is
        // inside the widened window again while its row is gone, so the current
        // cutoff on its own would call the carried copy a brand new observation.
        sqlx::query(
            "UPDATE retention_policies SET retention_days = 2 WHERE family = 'raw_metric_sample'",
        )
        .execute(state.db().pool())
        .await
        .unwrap();

        // Restart: the floor is durable evidence, not a memory of this process.
        // The old Server has to let go of the file first, exactly as a restart
        // would.
        state.db().pool().close().await;
        drop(state);
        let state = restart(&dir).await;
        let after_restart: String = sqlx::query_scalar(
            "SELECT released_before FROM node_metric_series_state WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(after_restart, released_before);

        // The Agent carries its last good reading in a new Report. It is the
        // observation that was already counted, so it is a replay: the lifetime
        // count does not move, and no row comes back for it. Recreating the row
        // would restamp receipt evidence for an instant the Server released on
        // purpose, and a carry of adjacent expired instants would rebuild an
        // interval the retention window deliberately stopped holding.
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0095-4095-8095-000000000095".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 1, 0)
        );
        let samples: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_metric_samples WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            samples, 0,
            "a replay never writes a row back, however wide the window is: the count of observations is what a replay must never move"
        );
    }

    /// The reviewer's second scenario: the very first observation of a series is
    /// older than the raw window, so it is counted while no row can answer for
    /// it. Counting it is what leaves an evidence floor behind; without that
    /// floor a later widening of the window over the same instant would turn the
    /// Agent's carried last good reading into a brand new observation.
    #[tokio::test]
    async fn an_observation_counted_below_the_window_is_not_recounted_by_a_widening() {
        let (dir, state, agent_id) = state_with_agent().await;
        let mut report = report_near_now();
        let node_id = report.nodes[0].node_id.to_string();
        // Forty hours back: outside the one-day window, inside a two-day one.
        let observed_at = format_rfc3339(now_utc() - time::Duration::hours(40));
        report.generated_at = observed_at.parse().unwrap();
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(report.generated_at),
            latest_observed_at: Some(report.generated_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(12_884_901_888),
            error: None,
        });
        report.nodes[0].data_directory_capacity_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(report.generated_at),
            latest_observed_at: Some(report.generated_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(51_539_607_552),
            error: None,
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 0, 0),
            "the first observation of a series is counted even though the window cannot hold it"
        );
        let samples: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_metric_samples WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(samples, 0, "and no row can answer for it");
        let floor: String = sqlx::query_scalar(
            "SELECT released_before FROM node_metric_series_state WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(
            floor.as_str() > observed_at.as_str(),
            "counting an observation nothing holds leaves an evidence floor above it: {floor}"
        );

        // The Operator widens the window past the instant. Its row never comes
        // back, so the floor is the only thing that can refuse a second count.
        sqlx::query(
            "UPDATE retention_policies SET retention_days = 2 WHERE family = 'raw_metric_sample'",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        // Restart: the floor is durable evidence, not a memory of this process.
        state.db().pool().close().await;
        drop(state);
        let state = restart(&dir).await;

        report.report_sequence += 1;
        report.report_id = "0195f2a1-0096-4096-8096-000000000096".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 1, 0),
            "the widened window reads the carried instant again, it does not count it again"
        );
    }

    async fn metric_ledger(pool: &sqlx::AnyPool, node_id: &str, metric: &str) -> (i64, i64, i64) {
        sqlx::query_as(
            "SELECT observation_count, replayed_count, corrected_count FROM node_metric_series_state WHERE node_id = $1 AND metric = $2",
        )
        .bind(node_id)
        .bind(metric)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// One Node exactly as the Home list route renders it, so the assertion
    /// proves the Public Projection the Home cards read.
    async fn public_home_node(state: &AppState, node_id: &str) -> serde_json::Value {
        let response = crate::http::public::public_networks(State(state.clone())).await;
        let body = body_json(response).await;
        body.as_array()
            .unwrap()
            .iter()
            .flat_map(|network| network["nodes"].as_array().unwrap())
            .find(|node| node["nodeId"] == node_id)
            .cloned()
            .expect("the Node is in the Public Home list")
    }

    async fn admin_agent_json(state: &AppState, agent_id: &str) -> serde_json::Value {
        let response = crate::http::admin::admin_agent_detail(
            State(state.clone()),
            axum::extract::Path(agent_id.to_owned()),
            Extension(crate::http::AuthenticatedSession(
                crate::auth::SessionInfo {
                    session_id: "session".to_owned(),
                    user_id: "owner".to_owned(),
                    username: "owner".to_owned(),
                    role: "owner".to_owned(),
                    created_at: crate::auth::now_utc(),
                    last_seen_at: crate::auth::now_utc(),
                    expires_at: crate::auth::now_utc(),
                    csrf_token: "csrf".to_owned(),
                },
            )),
            Extension(RequestId(Arc::from("test-request"))),
        )
        .await;
        body_json(response).await
    }

    fn disabled_process() -> ComponentObservation<platpulse_core::observation::ProcessCurrent> {
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

    #[tokio::test]
    async fn supervisor_process_selector_projects_real_values_and_keeps_last_good() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let node_id = report.inventory.nodes[0].node_id.to_string();
        report.inventory.nodes[0].process = Some(platpulse_core::ProcessSelector::Supervisor {
            program: "validators:platon-validator-a".into(),
        });
        report.agent_capabilities = vec![platpulse_core::AgentCapability::ProcessSupervisor];
        let observed_at = report.generated_at;
        report.nodes[0].process = ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(platpulse_core::observation::ProcessCurrent {
                pid: 4242,
                started_at: observed_at,
                cpu_percent: 12.5,
                memory_bytes: 268_435_456,
                uptime_ms: 86_400_000,
            }),
            error: None,
        };
        report.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        // Ingestion persisted the supervisor observation before projection.
        let stored: (i64, f64, i64, i64) = sqlx::query_as(
            "SELECT pid, cpu_percent, memory_bytes, uptime_ms FROM current_node_process_observations WHERE node_id=$1",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, (4242, 12.5, 268_435_456, 86_400_000));

        // Public and Admin both render real values instead of Unknown.
        let public = public_home_node(&state, &node_id).await;
        assert_eq!(public["processState"], "ok");
        assert_eq!(public["processCpuPercent"], 12.5);
        assert_eq!(public["processUptimeMs"], 86_400_000);
        assert_eq!(public["processStartedAt"], observed_at.to_string());
        assert!(public["processMemoryPercent"].as_f64().unwrap() > 0.0);
        let admin = admin_agent_json(&state, &agent_id).await;
        let process = &admin["nodes"][0]["process"];
        assert_eq!(process["state"], "ok");
        assert_eq!(process["pid"], 4242);
        assert_eq!(process["cpu_percent"], 12.5);
        assert_eq!(process["uptime_ms"], 86_400_000);

        // A failed collection keeps the last-good value with an explicit error.
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0091-4091-8091-000000000091".parse().unwrap();
        report.nodes[0].process = ComponentObservation {
            status: ComponentStatus::Error,
            attempted_at: Some(observed_at),
            latest_observed_at: None,
            received_at: None,
            state_revision: 2,
            value_revision: 1,
            latest: None,
            error: Some(platpulse_core::BoundedError {
                code: "process_selector_error".into(),
                message: "supervisor control command failed".into(),
            }),
        };
        report.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        let public = public_home_node(&state, &node_id).await;
        assert_eq!(public["processState"], "error");
        assert_eq!(public["processCpuPercent"], 12.5);
        assert_eq!(public["processUptimeMs"], 86_400_000);
        let admin = admin_agent_json(&state, &agent_id).await;
        let process = &admin["nodes"][0]["process"];
        assert_eq!(process["state"], "error");
        assert_eq!(process["error_code"], "process_selector_error");
        assert_eq!(process["cpu_percent"], 12.5);

        // A Node with no selector stays Disabled: unknown is never rendered as 0.
        let second_id = "0195f2a1-0015-4015-8015-000000000015".parse().unwrap();
        let mut second = report.inventory.nodes[0].clone();
        second.node_id = second_id;
        second.rpc_endpoint = "ws://127.0.0.1:6791".parse().unwrap();
        second.process = None;
        report.inventory.nodes.push(second);
        report.inventory.revision = 2;
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0092-4092-8092-000000000092".parse().unwrap();
        let mut second_obs = report.nodes[0].clone();
        second_obs.node_id = second_id;
        second_obs.process = disabled_process();
        report.nodes.push(second_obs);
        report.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        let public = public_home_node(&state, &second_id.to_string()).await;
        assert_eq!(public["processState"], "disabled");
        assert!(public["processCpuPercent"].is_null());
        assert!(public["processUptimeMs"].is_null());
        let admin = admin_agent_json(&state, &agent_id).await;
        let second_diagnostic = admin["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["node_id"] == second_id.to_string())
            .unwrap();
        assert_eq!(second_diagnostic["process"]["state"], "disabled");
        assert!(second_diagnostic["process"]["pid"].is_null());
        assert!(second_diagnostic["process"]["cpu_percent"].is_null());
    }

    #[tokio::test]
    async fn boot_sequence_gaps_and_competing_boots_are_recorded_and_rejected() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        report.report_sequence = 3;
        report.report_id = "0195f2a1-0026-4026-8026-000000000026".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let gaps: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM report_sequence_gaps WHERE agent_id=$1")
                .bind(&agent_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(gaps, 1);

        let old_boot = report.boot_id;
        report.boot_id = "0195f2a1-0027-4027-8027-000000000027".parse().unwrap();
        report.report_sequence = 1;
        report.report_id = "0195f2a1-0028-4028-8028-000000000028".parse().unwrap();
        let response = handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.clone(),
                credential_id: "test".into(),
            }),
            Extension(RequestId(Arc::from("test-request"))),
            Bytes::from(serde_json::to_vec(&report).unwrap()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let receipt: ReportResponse =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(receipt.receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            receipt.receipt.rejections[0].code,
            platpulse_core::RejectionCode::ConflictingBoot
        );
        assert_ne!(old_boot, report.boot_id);
    }

    #[tokio::test]
    async fn conflicting_report_sequence_records_one_security_event() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&original).unwrap()).await;

        let mut conflict = original;
        conflict.report_id = "0195f2a1-0034-4034-8034-000000000034".parse().unwrap();
        conflict.host.cpu_percent.latest = Some(42.0);
        let response = handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.clone(),
                credential_id: "test".into(),
            }),
            Extension(RequestId(Arc::from("sequence-conflict"))),
            Bytes::from(serde_json::to_vec(&conflict).unwrap()),
        )
        .await;

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT security_event_count FROM agents WHERE agent_id=$1",
            )
            .bind(&agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );
    }
    #[tokio::test]
    async fn closing_then_drained_previous_atomically_rotates_boot_and_rejects_old_reports() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut closing: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let old_boot = closing.boot_id;
        closing.report_sequence = 2;
        closing.report_id = "0195f2a1-0030-4030-8030-000000000030".parse().unwrap();
        closing.boot_transition = platpulse_core::BootTransition::Closing;
        submit(&state, &agent_id, serde_json::to_vec(&closing).unwrap()).await;

        let mut stale = closing.clone();
        stale.report_id = "0195f2a1-0031-4031-8031-000000000031".parse().unwrap();
        stale.report_sequence = 3;
        stale.boot_transition = platpulse_core::BootTransition::Continuing;
        let stale_receipt = submit(&state, &agent_id, serde_json::to_vec(&stale).unwrap()).await;
        assert_eq!(
            stale_receipt.rejections[0].code,
            platpulse_core::RejectionCode::StaleReport
        );

        let mut next = closing;
        next.boot_id = "0195f2a1-0032-4032-8032-000000000032".parse().unwrap();
        next.previous_boot_id = Some(old_boot);
        next.boot_transition = platpulse_core::BootTransition::DrainedPrevious;
        next.report_sequence = 1;
        next.report_id = "0195f2a1-0033-4033-8033-000000000033".parse().unwrap();
        let next_receipt = submit(&state, &agent_id, serde_json::to_vec(&next).unwrap()).await;
        assert_eq!(next_receipt.disposition, ReceiptDisposition::Accepted);

        let statuses = sqlx::query_as::<_, (String, String)>(
            "SELECT boot_id, status FROM agent_boots WHERE agent_id=$1 ORDER BY boot_id",
        )
        .bind(&agent_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(statuses.len(), 2);
        assert!(
            statuses
                .iter()
                .any(|(boot, status)| boot == &old_boot.to_string() && status == "closed")
        );
        assert!(
            statuses
                .iter()
                .any(|(boot, status)| boot == &next.boot_id.to_string() && status == "active")
        );
        let security_events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&agent_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(security_events, 0);
    }

    // Issue #179: a manual repair (or a crash/fork window) can leave
    // `agents.active_boot_status = 'closed'` while the matching `agent_boots`
    // row is still `active`. The first report of the next Boot must close that
    // stale row inside the same transaction instead of hard-failing the
    // `agent_boots_one_active` upsert and answering a bare 503 forever.
    #[tokio::test]
    async fn inconsistent_agents_boot_status_does_not_block_a_new_boot() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let first: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let old_boot = first.boot_id;
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;

        // Operator repair that only touched `agents`: the Boot row is stale.
        sqlx::query("UPDATE agents SET active_boot_status='closed' WHERE agent_id=$1")
            .bind(&agent_id)
            .execute(state.db().pool())
            .await
            .unwrap();

        let mut next = first.clone();
        next.boot_id = "0195f2a1-0094-4094-8094-000000000094".parse().unwrap();
        next.previous_boot_id = Some(old_boot);
        next.boot_transition = platpulse_core::BootTransition::DrainedPrevious;
        next.report_sequence = 1;
        next.report_id = "0195f2a1-0095-4095-8095-000000000095".parse().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&next).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);

        let statuses = sqlx::query_as::<_, (String, String)>(
            "SELECT boot_id, status FROM agent_boots WHERE agent_id=$1 ORDER BY boot_id",
        )
        .bind(&agent_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(statuses.len(), 2);
        assert!(
            statuses
                .iter()
                .any(|(boot, status)| boot == &old_boot.to_string() && status == "closed")
        );
        assert!(
            statuses
                .iter()
                .any(|(boot, status)| boot == &next.boot_id.to_string() && status == "active")
        );
        let active: (Option<String>, String) = sqlx::query_as(
            "SELECT active_boot_id, active_boot_status FROM agents WHERE agent_id=$1",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            active,
            (Some(next.boot_id.to_string()), "active".to_owned())
        );
    }

    // Issue #179: a database constraint failure during ingestion must not be
    // reported as a content-free `unavailable`; the operator needs a concrete
    // code to tell a constraint bug from a genuinely unavailable database.
    #[tokio::test]
    async fn database_constraint_failures_are_not_reported_as_unavailable() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let failure = sqlx::query("INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES ($1, 1, NULL, NULL, NULL, $2, $3)")
            .bind(&agent_id)
            .bind("2026-08-12T08:00:00Z")
            .bind("2026-08-12T08:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap_err();
        let response = storage_error("test-request", "test constraint", &failure);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "storage_conflict");
        assert_eq!(body["error"]["requestId"], "test-request");
    }

    #[tokio::test]
    async fn accepted_inventory_persists_observations_and_replay_is_exact() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let body = include_bytes!("../../../platpulse-core/tests/fixtures/report_v1_minimal.json");
        let first = submit(&state, &agent_id, body.to_vec()).await;
        assert_eq!(first.disposition, ReceiptDisposition::Accepted);
        assert_eq!(first.inventory, Some(InventoryDisposition::Accepted));
        assert_eq!(first.nodes.len(), 1);
        let statuses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM component_status")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert!(statuses >= 13);
        let memory: i64 = sqlx::query_scalar(
            "SELECT memory_used_bytes FROM current_host_observations WHERE agent_id = $1",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(memory, 4_294_967_296);
        let replay = submit(&state, &agent_id, body.to_vec()).await;
        assert_eq!(replay, first);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_report_receipts")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn peer_snapshot_projects_per_node_and_preserves_last_good_on_error() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:00Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 1,
            "value_revision": 1,
            "latest": {
                "peers": [{
                    "peer_id": "peer-a",
                    "remote_ip": "203.0.113.4",
                    "direction": "inbound",
                    "trusted": true,
                    "static_peer": false,
                    "consensus_peer": true,
                    "client_name": "PlatON/v1.5.1",
                    "caps": ["cbft/1"],
                    "cbft_protocol_version": 1,
                    "cbft_highest_qc_block": 100,
                    "cbft_locked_block": 99,
                    "cbft_commit_block": 98
                }]
            }
        });
        let first_body = serde_json::to_vec(&value).unwrap();
        let first = submit(&state, &agent_id, first_body.clone()).await;
        assert_eq!(first.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            state
                .admin_realtime()
                .pending_events()
                .iter()
                .map(|event| event.resource.as_str())
                .collect::<Vec<_>>(),
            vec!["node", "peer", "geo"]
        );
        assert_eq!(
            state
                .public_realtime()
                .pending_events()
                .iter()
                .map(|event| event.resource.as_str())
                .collect::<Vec<_>>(),
            vec!["node", "network"]
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND closed_at IS NULL",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "the first successful Peer Snapshot establishes the baseline only"
        );
        let first_value_received_at: String = sqlx::query_scalar(
            "SELECT value_received_at FROM component_status WHERE node_id=$1 AND component_key='peers'",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(!first_value_received_at.is_empty());
        sqlx::query("UPDATE component_status SET value_received_at=$1 WHERE node_id=$2 AND component_key='peers'")
            .bind("2026-01-01T00:00:00Z")
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .execute(state.db().pool())
            .await
            .unwrap();
        let current: (String, String, i64, String, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT peer_id, direction, trusted, client_name, remote_ip, cbft_commit_block FROM current_node_peers WHERE node_id=$1",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(current.0, "peer-a");
        assert_eq!(current.1, "inbound");
        assert_eq!(current.2, 1);
        assert_eq!(current.3, "PlatON/v1.5.1");
        assert_eq!(current.4, None);
        assert_eq!(current.5, Some(98));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM current_node_peer_capabilities WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );

        let replay = submit(&state, &agent_id, first_body).await;
        assert_eq!(replay, first);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "a replayed immutable report must not create a baseline presence interval"
        );

        let mut duplicate_value = value.clone();
        duplicate_value["report_id"] = serde_json::json!("0195f2a1-0098-4098-8098-000000000098");
        duplicate_value["report_sequence"] = serde_json::json!(2);
        let peer = value["nodes"][0]["chain"]["peers"]["latest"]["peers"][0].clone();
        duplicate_value["nodes"][0]["chain"]["peers"]["latest"]["peers"] =
            serde_json::json!([peer.clone(), peer]);
        let duplicate_status = submit_status(
            &state,
            &agent_id,
            serde_json::to_vec(&duplicate_value).unwrap(),
        )
        .await;
        assert_eq!(duplicate_status, StatusCode::BAD_REQUEST);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM current_node_peers WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "a duplicate Peer ID must not alter current state"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0099-4099-8099-000000000099");
        value["report_sequence"] = serde_json::json!(2);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "error",
            "attempted_at": "2026-08-12T10:00:05Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 2,
            "value_revision": 1,
            "latest": {
                "peers": [{
                    "peer_id": "peer-a",
                    "remote_ip": "203.0.113.4",
                    "direction": "inbound",
                    "trusted": true,
                    "static_peer": false,
                    "consensus_peer": true,
                    "client_name": "PlatON/v1.5.1",
                    "caps": ["cbft/1"],
                    "cbft_protocol_version": 1,
                    "cbft_highest_qc_block": 100,
                    "cbft_locked_block": 99,
                    "cbft_commit_block": 98
                }]
            },
            "error": {"code": "admin_peers_failed", "message": "peer probe failed"}
        });
        let error_receipt = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(error_receipt.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND closed_at IS NULL",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "a failed Peer collection cannot close or invent a baseline interval"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM current_node_peers WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "a Peer collection error must not erase last-good current peers"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT state FROM component_status WHERE node_id=$1 AND component_key='peers'",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            "error"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT value_received_at FROM component_status WHERE node_id=$1 AND component_key='peers'",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            "2026-01-01T00:00:00Z",
            "an error receipt must not replace the last successful value receipt"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0104-4104-8104-000000000104");
        value["report_sequence"] = serde_json::json!(3);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:07Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 3,
            "value_revision": 2,
            "latest": {
                "peers": [{
                    "peer_id": "peer-a",
                    "remote_ip": "203.0.113.4",
                    "direction": "outbound",
                    "trusted": false,
                    "static_peer": true,
                    "consensus_peer": false,
                    "client_name": "PlatON/v1.5.2",
                    "caps": ["cbft/2"],
                    "cbft_protocol_version": 2,
                    "cbft_highest_qc_block": 101,
                    "cbft_locked_block": 100,
                    "cbft_commit_block": 99
                }]
            }
        });
        let unchanged_receipt =
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(unchanged_receipt.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "a successful snapshot that retains the Peer does not invent an arrival"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0100-4100-8100-000000000100");
        value["report_sequence"] = serde_json::json!(4);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "unsupported",
            "state_revision": 3,
            "value_revision": 1
        });
        let unsupported_receipt =
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(
            unsupported_receipt.disposition,
            ReceiptDisposition::Accepted
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM current_node_peers WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "Unsupported must preserve last-good current peers"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0101-4101-8101-000000000101");
        value["report_sequence"] = serde_json::json!(5);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:10Z",
            "latest_observed_at": "2026-08-12T10:00:10Z",
            "state_revision": 4,
            "value_revision": 2,
            "latest": {"peers": []}
        });
        let empty_receipt = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(empty_receipt.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a'",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "an empty snapshot removes the baseline Peer without inventing a departure interval"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM current_node_peers WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "a successful empty snapshot must clear current peers"
        );
        let empty_value_received_at: String = sqlx::query_scalar(
            "SELECT value_received_at FROM component_status WHERE node_id=$1 AND component_key='peers'",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_ne!(empty_value_received_at, "2026-01-01T00:00:00Z");
        sqlx::query("UPDATE component_status SET value_received_at=$1 WHERE node_id=$2 AND component_key='peers'")
            .bind("2026-01-02T00:00:00Z")
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .execute(state.db().pool())
            .await
            .unwrap();

        value["report_id"] = serde_json::json!("0195f2a1-0102-4102-8102-000000000102");
        value["report_sequence"] = serde_json::json!(6);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "error",
            "attempted_at": "2026-08-12T10:00:15Z",
            "latest_observed_at": "2026-08-12T10:00:10Z",
            "state_revision": 5,
            "value_revision": 2,
            "latest": {"peers": []},
            "error": {"code": "admin_peers_failed", "message": "peer probe failed after empty snapshot"}
        });
        let empty_error_receipt =
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(
            empty_error_receipt.disposition,
            ReceiptDisposition::Accepted
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT value_received_at FROM component_status WHERE node_id=$1 AND component_key='peers'",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            "2026-01-02T00:00:00Z",
            "an error after an empty snapshot must preserve its value receipt"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0103-4103-8103-000000000103");
        value["report_sequence"] = serde_json::json!(7);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:20Z",
            "latest_observed_at": "2026-08-12T10:00:20Z",
            "state_revision": 6,
            "value_revision": 3,
            "latest": {
                "peers": [{
                    "peer_id": "peer-a",
                    "remote_ip": "203.0.113.5",
                    "direction": "outbound",
                    "trusted": false,
                    "static_peer": true,
                    "consensus_peer": false,
                    "client_name": "PlatON/v1.5.2",
                    "caps": ["cbft/2"],
                    "cbft_protocol_version": 2,
                    "cbft_highest_qc_block": 200,
                    "cbft_locked_block": 199,
                    "cbft_commit_block": 198
                }]
            }
        });
        let reappearance_receipt =
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(
            reappearance_receipt.disposition,
            ReceiptDisposition::Accepted
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a'",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "a Peer that reappears after the baseline was cleared opens a new interval"
        );
        let open_intervals: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' AND closed_at IS NULL",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(open_intervals, 1);
        let first_interval_client: String = sqlx::query_scalar(
            "SELECT client_name FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' ORDER BY interval_id LIMIT 1",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            first_interval_client, "PlatON/v1.5.2",
            "the arrival interval records the reappearance metadata"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0105-4105-8105-000000000105");
        value["report_sequence"] = serde_json::json!(8);
        value["inventory"]["revision"] = serde_json::json!(2);
        value["inventory"]["nodes"] = serde_json::json!([]);
        value["nodes"] = serde_json::json!([]);
        let retired = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(retired.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT lifecycle FROM nodes WHERE node_id=$1",)
                .bind("0195f2a1-0014-4014-8014-000000000014")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            "retired"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' AND closed_at IS NULL",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "retiring a Node without a snapshot does not fabricate a departure"
        );
    }

    /// Every stamp a Peer write can move: one entry per current Peer and one
    /// per recorded capability, so a Report that rewrites nothing is visible as
    /// an unchanged stamp instead of a guess about timing.
    async fn peer_stamps(state: &AppState, node_id: &str) -> Vec<(String, String)> {
        let mut stamps: Vec<(String, String)> = sqlx::query_as(
            "SELECT peer_id, updated_at FROM current_node_peers WHERE node_id=$1 ORDER BY peer_id",
        )
        .bind(node_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        stamps.extend(
            sqlx::query_as::<_, (String, String)>(
                "SELECT peer_id || ':' || capability, updated_at FROM current_node_peer_capabilities WHERE node_id=$1 ORDER BY peer_id, capability",
            )
            .bind(node_id)
            .fetch_all(state.db().pool())
            .await
            .unwrap(),
        );
        stamps
    }

    /// Replace every Peer stamp with a sentinel, so the next Report can only
    /// leave it in place by writing no row at all.
    async fn stamp_peer_rows(state: &AppState, node_id: &str, stamp: &str) {
        for statement in [
            "UPDATE current_node_peers SET updated_at=$1 WHERE node_id=$2",
            "UPDATE current_node_peer_capabilities SET updated_at=$1 WHERE node_id=$2",
        ] {
            sqlx::query(statement)
                .bind(stamp)
                .bind(node_id)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
    }

    fn stamp_of(stamps: &[(String, String)], key: &str) -> String {
        stamps
            .iter()
            .find(|(recorded, _)| recorded == key)
            .unwrap_or_else(|| panic!("{key} is not recorded in {stamps:?}"))
            .1
            .clone()
    }

    fn peer_snapshot_body(peers: serde_json::Value) -> serde_json::Value {
        let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:00Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 1,
            "value_revision": 1,
            "latest": {"peers": peers}
        });
        value
    }

    /// Acceptance of issue #271: a Report whose Peer set has not changed writes
    /// no row to the Peer tables or the Geo cache, and the Peer Snapshot still
    /// proves it is fresh through its own component receipt rather than a row
    /// stamp.
    #[tokio::test]
    async fn repeated_peer_snapshot_writes_no_peer_row() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let node_id = "0195f2a1-0014-4014-8014-000000000014";
        let sentinel = "2026-01-01T00:00:00Z";
        let peers = serde_json::json!([
            {
                "peer_id": "peer-a",
                "remote_ip": "8.8.8.8",
                "direction": "inbound",
                "trusted": true,
                "static_peer": false,
                "consensus_peer": true,
                "client_name": "PlatON/v1.5.1",
                "caps": ["cbft/1"],
                "cbft_protocol_version": 1,
                "cbft_highest_qc_block": 100,
                "cbft_locked_block": 99,
                "cbft_commit_block": 98
            },
            {
                "peer_id": "peer-b",
                "remote_ip": "192.0.2.10",
                "direction": "outbound",
                "trusted": false,
                "static_peer": true,
                "consensus_peer": false,
                "caps": ["cbft/2"]
            }
        ]);
        let mut value = peer_snapshot_body(peers);
        value["report_id"] = serde_json::json!("0195f2a1-0180-4180-8180-000000000180");
        value["report_sequence"] = serde_json::json!(1);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        // A country result as the maintenance cycle leaves it for the public
        // address this snapshot references.
        let cache_now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('local_mmdb', '8.8.8.8', 'US', 'current', $1, $2, $3, $4, $5)")
            .bind(&cache_now)
            .bind(&cache_now)
            .bind(&cache_now)
            .bind(&cache_now)
            .bind("2026-01-02T00:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        stamp_peer_rows(&state, node_id, sentinel).await;
        sqlx::query("UPDATE geo_location_cache SET last_referenced_at=$1")
            .bind(sentinel)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("UPDATE component_status SET value_received_at=$1 WHERE node_id=$2 AND component_key='peers'")
            .bind(sentinel)
            .bind(node_id)
            .execute(state.db().pool())
            .await
            .unwrap();

        // A row-level write counter, so "writes no row" is counted instead of
        // inferred from an unchanged stamp: both backends fire these triggers
        // for every row a statement touches even when the new values equal the
        // old ones. The counter is test-only DDL and never reaches a production
        // schema.
        sqlx::query("CREATE TABLE peer_write_probe (probe TEXT NOT NULL)")
            .execute(state.db().pool())
            .await
            .unwrap();
        for (table, name) in [
            ("current_node_peers", "peers"),
            ("current_node_peer_capabilities", "capabilities"),
            ("geo_location_cache", "geo"),
        ] {
            if cfg!(storage_postgres) {
                sqlx::query(
                    "CREATE OR REPLACE FUNCTION platpulse_probe_write() RETURNS trigger \
                     LANGUAGE plpgsql AS $$ BEGIN \
                     INSERT INTO peer_write_probe (probe) VALUES (TG_TABLE_NAME::text || ' ' || TG_OP); \
                     RETURN NULL; END $$",
                )
                .execute(state.db().pool())
                .await
                .unwrap();
                sqlx::query(&format!(
                    "CREATE TRIGGER probe_{name} AFTER INSERT OR UPDATE OR DELETE ON {table} \
                     FOR EACH ROW EXECUTE FUNCTION platpulse_probe_write()"
                ))
                .execute(state.db().pool())
                .await
                .unwrap();
            } else {
                for event in ["INSERT", "UPDATE", "DELETE"] {
                    sqlx::query(&format!(
                        "CREATE TRIGGER probe_{name}_{event} AFTER {event} ON {table} \
                         BEGIN INSERT INTO peer_write_probe (probe) VALUES ('{name} {event}'); END"
                    ))
                    .execute(state.db().pool())
                    .await
                    .unwrap();
                }
            }
        }
        // Self-check: a no-op UPDATE of the same rows must be counted, so the
        // zero below cannot pass because the triggers never fire.
        sqlx::query("UPDATE current_node_peers SET trusted = trusted")
            .execute(state.db().pool())
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM peer_write_probe")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            2,
            "the write probe counts a same-value Peer UPDATE"
        );
        sqlx::query("DELETE FROM peer_write_probe")
            .execute(state.db().pool())
            .await
            .unwrap();

        value["report_id"] = serde_json::json!("0195f2a1-0181-4181-8181-000000000181");
        value["report_sequence"] = serde_json::json!(2);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        let stamps = peer_stamps(&state, node_id).await;
        assert_eq!(
            stamps,
            vec![
                ("peer-a".to_owned(), sentinel.to_owned()),
                ("peer-b".to_owned(), sentinel.to_owned()),
                ("peer-a:cbft/1".to_owned(), sentinel.to_owned()),
                ("peer-b:cbft/2".to_owned(), sentinel.to_owned()),
            ],
            "a repeated Peer set must not rewrite a Peer or a capability row"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM peer_write_probe")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            0,
            "a repeated Peer set must not write a Peer, capability or Geo cache row"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT last_referenced_at FROM geo_location_cache WHERE canonical_ip='8.8.8.8'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            sentinel,
            "the receipt no longer refreshes Peer references; the maintenance cycle owns them"
        );
        assert_ne!(
            sqlx::query_scalar::<_, String>(
                "SELECT value_received_at FROM component_status WHERE node_id=$1 AND component_key='peers'",
            )
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            sentinel,
            "the Peer Snapshot still stamps its own freshness on every successful snapshot"
        );
    }

    /// The diff write of issue #271: an arrival is inserted, a departure is
    /// deleted with its capabilities, and a Peer whose attributes changed is
    /// the only row rewritten. A Peer the snapshot repeats untouched keeps its
    /// stamp, so the Server never rewrites the whole set to absorb one change.
    #[tokio::test]
    async fn peer_snapshot_writes_only_the_difference() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let node_id = "0195f2a1-0014-4014-8014-000000000014";
        let sentinel = "2026-01-01T00:00:00Z";
        let peer_a = serde_json::json!({
            "peer_id": "peer-a",
            "remote_ip": "8.8.8.8",
            "direction": "inbound",
            "trusted": true,
            "static_peer": false,
            "consensus_peer": true,
            "client_name": "PlatON/v1.5.1",
            "caps": ["cbft/1"],
            "cbft_protocol_version": 1,
            "cbft_highest_qc_block": 100,
            "cbft_locked_block": 99,
            "cbft_commit_block": 98
        });
        let peer_b = serde_json::json!({
            "peer_id": "peer-b",
            "direction": "outbound",
            "trusted": false,
            "static_peer": true,
            "consensus_peer": false,
            "caps": ["cbft/2"]
        });
        let peer_c = serde_json::json!({
            "peer_id": "peer-c",
            "direction": "inbound",
            "trusted": false,
            "static_peer": false,
            "consensus_peer": false,
            "caps": ["cbft/3"]
        });
        let mut value = peer_snapshot_body(serde_json::json!([peer_a.clone(), peer_b]));
        value["report_id"] = serde_json::json!("0195f2a1-0190-4190-8190-000000000190");
        value["report_sequence"] = serde_json::json!(1);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        stamp_peer_rows(&state, node_id, sentinel).await;

        // peer-b departs, peer-c arrives, and peer-a is repeated unchanged.
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] =
            serde_json::json!([peer_a.clone(), peer_c]);
        value["report_id"] = serde_json::json!("0195f2a1-0191-4191-8191-000000000191");
        value["report_sequence"] = serde_json::json!(2);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        let stamps = peer_stamps(&state, node_id).await;
        assert_eq!(
            stamps
                .iter()
                .map(|(recorded, _)| recorded.as_str())
                .collect::<Vec<_>>(),
            vec!["peer-a", "peer-c", "peer-a:cbft/1", "peer-c:cbft/3"],
            "the departure is deleted and the arrival inserted"
        );
        assert_eq!(
            stamp_of(&stamps, "peer-a"),
            sentinel,
            "an unchanged Peer keeps its stamp instead of being rewritten"
        );
        assert_ne!(stamp_of(&stamps, "peer-c"), sentinel);
        assert_ne!(stamp_of(&stamps, "peer-c:cbft/3"), sentinel);

        // peer-a now reports a newer highest QC block and one extra capability.
        let mut changed = peer_a.clone();
        changed["cbft_highest_qc_block"] = serde_json::json!(101);
        changed["caps"] = serde_json::json!(["cbft/1", "cbft/9"]);
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] =
            serde_json::json!([changed, peer_c.clone()]);
        value["report_id"] = serde_json::json!("0195f2a1-0192-4192-8192-000000000192");
        value["report_sequence"] = serde_json::json!(3);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        let stamps = peer_stamps(&state, node_id).await;
        assert_ne!(
            stamp_of(&stamps, "peer-a"),
            sentinel,
            "a Peer whose attributes changed is rewritten"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT cbft_highest_qc_block FROM current_node_peers WHERE node_id=$1 AND peer_id='peer-a'",
            )
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            101
        );
        assert_eq!(
            stamp_of(&stamps, "peer-a:cbft/1"),
            sentinel,
            "a capability the snapshot repeats is not rewritten"
        );
        assert_ne!(stamp_of(&stamps, "peer-a:cbft/9"), sentinel);

        // The capability that is no longer reported disappears on its own.
        let mut narrowed = changed;
        narrowed["caps"] = serde_json::json!(["cbft/9"]);
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] =
            serde_json::json!([narrowed, peer_c]);
        value["report_id"] = serde_json::json!("0195f2a1-0193-4193-8193-000000000193");
        value["report_sequence"] = serde_json::json!(4);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        let capabilities: Vec<String> = sqlx::query_scalar(
            "SELECT capability FROM current_node_peer_capabilities WHERE node_id=$1 AND peer_id='peer-a' ORDER BY capability",
        )
        .bind(node_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(capabilities, vec!["cbft/9".to_owned()]);
    }

    #[tokio::test]
    async fn peer_presence_is_isolated_per_node_and_survives_server_restart() {
        let (dir, state, agent_id) = state_with_agent().await;
        let node_a = "0195f2a1-0014-4014-8014-000000000014";
        let node_b = "0195f2a1-0015-4015-8015-000000000015";
        let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:00Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 1,
            "value_revision": 1,
            "latest": {
                "peers": [{
                    "peer_id": "peer-a",
                    "remote_ip": "203.0.113.10",
                    "direction": "inbound",
                    "trusted": true,
                    "static_peer": false,
                    "consensus_peer": true,
                    "client_name": "PlatON/v1.5.1",
                    "caps": ["cbft/1"],
                    "cbft_protocol_version": 1,
                    "cbft_highest_qc_block": 100,
                    "cbft_locked_block": 99,
                    "cbft_commit_block": 98
                }]
            }
        });
        let mut second_inventory = value["inventory"]["nodes"][0].clone();
        second_inventory["node_id"] = serde_json::json!(node_b);
        second_inventory["rpc_endpoint"] = serde_json::json!("ws://127.0.0.1:6791");
        let mut inventory_nodes = value["inventory"]["nodes"].as_array().unwrap().clone();
        inventory_nodes.push(second_inventory);
        value["inventory"]["nodes"] = serde_json::json!(inventory_nodes);

        let mut second_observation = value["nodes"][0].clone();
        second_observation["node_id"] = serde_json::json!(node_b);
        let mut peer_b = second_observation["chain"]["peers"]["latest"]["peers"][0].clone();
        peer_b["peer_id"] = serde_json::json!("peer-b");
        peer_b["remote_ip"] = serde_json::json!("203.0.113.11");
        second_observation["chain"]["peers"]["latest"]["peers"] = serde_json::json!([peer_b]);
        let mut observations = value["nodes"].as_array().unwrap().clone();
        observations.push(second_observation);
        value["nodes"] = serde_json::json!(observations);
        value["report_id"] = serde_json::json!("0195f2a1-0110-4110-8110-000000000110");
        value["report_sequence"] = serde_json::json!(1);
        let first = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(first.disposition, ReceiptDisposition::Accepted);
        for (node_id, peer_id) in [(node_a, "peer-a"), (node_b, "peer-b")] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id=$2 AND closed_at IS NULL",
                )
                .bind(node_id)
                .bind(peer_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
                0,
                "each Node establishes its own successful baseline"
            );
        }

        value["report_id"] = serde_json::json!("0195f2a1-0111-4111-8111-000000000111");
        value["report_sequence"] = serde_json::json!(2);
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] = serde_json::json!([]);
        let second = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(second.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' AND closed_at IS NULL",
            )
            .bind(node_a)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "an empty snapshot closes only that Node's interval"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-b' AND closed_at IS NULL",
            )
            .bind(node_b)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "a different Node's unchanged baseline does not create an interval"
        );

        state.db().close().await;
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        let auth = AuthConfig::development(
            load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let restarted = AppState::new(database, None, auth);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-b' AND closed_at IS NULL",
            )
            .bind(node_b)
            .fetch_one(restarted.db().pool())
            .await
            .unwrap(),
            0,
            "a Server restart does not fabricate a Peer departure or arrival"
        );

        let mut peer_a = value["nodes"][1]["chain"]["peers"]["latest"]["peers"][0].clone();
        peer_a["peer_id"] = serde_json::json!("peer-a");
        peer_a["remote_ip"] = serde_json::json!("203.0.113.10");
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] = serde_json::json!([peer_a]);
        value["report_id"] = serde_json::json!("0195f2a1-0112-4112-8112-000000000112");
        value["report_sequence"] = serde_json::json!(3);
        let after_restart =
            submit(&restarted, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(after_restart.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' AND closed_at IS NULL",
            )
            .bind(node_a)
            .fetch_one(restarted.db().pool())
            .await
            .unwrap(),
            1,
            "the first post-restart difference opens a Node-scoped interval"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-b'",
            )
            .bind(node_b)
            .fetch_one(restarted.db().pool())
            .await
            .unwrap(),
            0,
            "Node B's unchanged baseline does not create Node A's interval"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0113-4113-8113-000000000113");
        value["report_sequence"] = serde_json::json!(4);
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] = serde_json::json!([]);
        let departure = submit(&restarted, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(departure.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' AND closed_at IS NOT NULL",
            )
            .bind(node_a)
            .fetch_one(restarted.db().pool())
            .await
            .unwrap(),
            1,
            "a later successful empty snapshot closes the open interval"
        );
    }

    #[tokio::test]
    async fn replay_sample_is_terminal_and_does_not_rewrite_history() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report.report_id = "0195f2a1-0013-4013-8013-000000000201".parse().unwrap();
        let node_id = report.inventory.nodes[0].node_id;
        let mut identity = report.nodes[0]
            .chain
            .network_identity
            .latest
            .clone()
            .unwrap();
        identity.genesis_hash =
            "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap();
        identity.address_hrp = Some("lat".to_owned());
        report
            .block_summaries
            .push(platpulse_core::block::BlockSummary {
                node_id,
                network_identity: identity,
                block_number: 10,
                block_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .parse()
                    .unwrap(),
                parent_hash: "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .parse()
                    .unwrap(),
                block_timestamp_ms: 1_000,
                observed_at: report.generated_at,
                transaction_count: 3,
                block_interval_ms: None,
                source: platpulse_core::block::BlockSource::Subscription,
                attribution: platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                    "0x1111111111111111111111111111111111111111"
                        .parse()
                        .unwrap(),
                    "test",
                ),
            });
        report.validate().unwrap();
        let first = report.clone();
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;
        sqlx::query("UPDATE block_summaries SET accepted_at=$1 WHERE node_id=$2")
            .bind("2026-01-01T00:00:00Z")
            .bind(node_id.to_string())
            .execute(state.db().pool())
            .await
            .unwrap();
        crate::retention::cleanup_raw_block_summaries(
            state.db().pool(),
            crate::auth::parse_rfc3339("2026-08-12T08:00:00Z").unwrap(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE block_history_state SET historical_high_watermark=100, cumulative_block_count=1, cumulative_transaction_count=3 WHERE node_id=$1").bind(node_id.to_string()).execute(state.db().pool()).await.unwrap();

        let mut replay = report;
        replay.report_sequence = 2;
        replay.report_id = "0195f2a1-0013-4013-8013-000000000202".parse().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(
            receipt.samples[0].disposition,
            SampleDispositionKind::TerminalRejected
        );
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::ResyncReplay
        );
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        let counters: (i64, i64) = sqlx::query_as("SELECT cumulative_block_count, cumulative_transaction_count FROM block_history_state WHERE node_id=$1").bind(node_id.to_string()).fetch_one(state.db().pool()).await.unwrap();
        assert_eq!(summaries, 0);
        assert_eq!(counters, (1, 3));
    }

    /// A 5-minute bucket edge an hour before now. The block tiers expire with
    /// their aggregate families after every accepted Report, so the instants a
    /// block case writes have to be inside the tiers' own windows.
    fn block_base() -> time::OffsetDateTime {
        let hour_ago = now_utc() - time::Duration::hours(1);
        time::OffsetDateTime::from_unix_timestamp(hour_ago.unix_timestamp().div_euclid(300) * 300)
            .unwrap()
    }

    fn at(base: time::OffsetDateTime, seconds: i64) -> String {
        format_rfc3339(base + time::Duration::seconds(seconds))
    }

    /// A report carrying the given Block Summaries for the fixture's Node, each
    /// stamped with the registered Network Identity: (height, block timestamp
    /// in ms, observed_at, transactions, source).
    fn report_with_blocks(
        report_id: &str,
        sequence: u64,
        blocks: &[(u64, u64, String, u64, platpulse_core::block::BlockSource)],
    ) -> AgentReport {
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report.report_id = report_id.parse().unwrap();
        report.report_sequence = sequence;
        let node_id = report.inventory.nodes[0].node_id;
        let mut identity = report.nodes[0]
            .chain
            .network_identity
            .latest
            .clone()
            .unwrap();
        identity.genesis_hash =
            "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap();
        identity.address_hrp = Some("lat".to_owned());
        for (height, timestamp_ms, observed_at, transactions, source) in blocks.iter().cloned() {
            report
                .block_summaries
                .push(platpulse_core::block::BlockSummary {
                    node_id,
                    network_identity: identity.clone(),
                    block_number: height,
                    block_hash: format!("0x{height:064x}").parse().unwrap(),
                    parent_hash: format!("0x{:064x}", height.saturating_sub(1))
                        .parse()
                        .unwrap(),
                    block_timestamp_ms: timestamp_ms,
                    observed_at: observed_at.parse().unwrap(),
                    transaction_count: transactions,
                    block_interval_ms: None,
                    source,
                    attribution:
                        platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                            "0x1111111111111111111111111111111111111111"
                                .parse()
                                .unwrap(),
                            "test",
                        ),
                });
        }
        report.validate().unwrap();
        report
    }

    /// One block bucket as (block_count, transaction_total, transaction_min,
    /// transaction_max, interval_count, interval_min_ms, interval_max_ms).
    type BlockBucket = (i64, i64, i64, i64, i64, Option<i64>, Option<i64>);

    async fn block_bucket(
        state: &AppState,
        node_id: &str,
        grain_seconds: i64,
        bucket_start: &str,
    ) -> Option<BlockBucket> {
        sqlx::query_as("SELECT block_count, transaction_total, transaction_min, transaction_max, interval_count, interval_min_ms, interval_max_ms FROM node_block_aggregates WHERE node_id = $1 AND grain_seconds = $2 AND bucket_start = $3")
            .bind(node_id)
            .bind(grain_seconds)
            .bind(bucket_start)
            .fetch_optional(state.db().pool())
            .await
            .unwrap()
    }

    async fn block_bucket_rows(state: &AppState) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM node_block_aggregates")
            .fetch_one(state.db().pool())
            .await
            .unwrap()
    }

    /// Every Block Coverage interval of a Node as
    /// (first_height, last_height, status), ordered by the lower bound.
    async fn coverage_intervals(state: &AppState, node_id: &str) -> Vec<(i64, i64, String)> {
        sqlx::query_as("SELECT first_height, last_height, status FROM block_coverage_intervals WHERE node_id=$1 ORDER BY first_height")
            .bind(node_id)
            .fetch_all(state.db().pool())
            .await
            .unwrap()
    }

    /// Issue #248: an accepted block advances one bucket of each tier, by the
    /// minute and five minutes its own observation falls in, with its
    /// transactions and its interval to the held parent. The identical Report
    /// delivered again is a Replay and a later Report repeating the same
    /// heights is a repeated delivery: neither moves a bucket.
    #[tokio::test]
    async fn accepted_blocks_advance_both_block_tiers_and_a_repeat_never_does() {
        use platpulse_core::block::BlockSource::Subscription;
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let blocks = [
            (10, 1_000, at(base, 1), 3, Subscription),
            (11, 3_000, at(base, 3), 5, Subscription),
            (12, 4_500, at(base, 5), 1, Subscription),
            (13, 6_500, at(base, 67), 2, Subscription),
        ];
        let report = report_with_blocks("0195f2a1-0013-4013-8013-000000000301", 1, &blocks);
        let node_id = report.inventory.nodes[0].node_id.to_string();
        let body = serde_json::to_vec(&report).unwrap();
        submit(&state, &agent_id, body.clone()).await;

        // Three blocks in the 09:00 minute: 3 + 5 + 1 transactions, and the
        // two intervals whose parent is held (block 10's parent is not).
        let minute = Some((3, 9, 1, 5, 2, Some(1_500), Some(2_000)));
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            minute
        );
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 60)).await,
            Some((1, 2, 2, 2, 1, Some(2_000), Some(2_000)))
        );
        // The 5-minute bucket counted all four.
        let five = Some((4, 11, 1, 5, 3, Some(1_500), Some(2_000)));
        assert_eq!(
            block_bucket(&state, &node_id, 300, &at(base, 0)).await,
            five
        );
        assert_eq!(
            block_bucket_rows(&state).await,
            3,
            "an empty bucket is never written"
        );

        // A Replay of the same Report bytes answers the stored receipt.
        submit(&state, &agent_id, body).await;
        // A later Report delivering the same heights again.
        let repeat = report_with_blocks("0195f2a1-0013-4013-8013-000000000302", 2, &blocks);
        submit(&state, &agent_id, serde_json::to_vec(&repeat).unwrap()).await;
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            minute
        );
        assert_eq!(
            block_bucket(&state, &node_id, 300, &at(base, 0)).await,
            five
        );
        assert_eq!(block_bucket_rows(&state).await, 3);
    }

    /// Issue #248: a plain resync replay - a subscription block at or below the
    /// historical high-water mark whose raw row retention has already released
    /// - is refused by Block history dedup and therefore never re-accumulates,
    /// even though no raw row is left to collide with.
    #[tokio::test]
    async fn a_resync_replay_below_the_high_water_mark_never_accumulates() {
        use platpulse_core::block::BlockSource::Subscription;
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let blocks = [(10, 1_000, at(base, 1), 3, Subscription)];
        let first = report_with_blocks("0195f2a1-0013-4013-8013-000000000311", 1, &blocks);
        let node_id = first.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;
        sqlx::query("DELETE FROM block_summaries WHERE node_id=$1")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE block_history_state SET historical_high_watermark=100 WHERE node_id=$1",
        )
        .bind(&node_id)
        .execute(state.db().pool())
        .await
        .unwrap();

        let replay = report_with_blocks("0195f2a1-0013-4013-8013-000000000312", 2, &blocks);
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::ResyncReplay
        );
        let once = Some((1, 3, 3, 3, 0, None, None));
        assert_eq!(block_bucket(&state, &node_id, 60, &at(base, 0)).await, once);
        assert_eq!(
            block_bucket(&state, &node_id, 300, &at(base, 0)).await,
            once
        );
    }

    /// Issue #248: only an explicit OpenRecoverableGap admits a block below the
    /// high-water mark. Its backfill is counted once, in the bucket of its own
    /// observation, and completes the interval of the child that was waiting
    /// for it in the child's own bucket; a second delivery of the same backfill
    /// and a backfill outside any open gap accumulate nothing.
    #[tokio::test]
    async fn an_open_gap_backfill_accumulates_once_and_completes_its_child_interval() {
        use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let live = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000321",
            1,
            &[
                (10, 1_000, at(base, 1), 3, Subscription),
                (12, 5_000, at(base, 5), 4, Subscription),
            ],
        );
        let node_id = live.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            Some((2, 7, 3, 4, 0, None, None)),
            "neither block has a held parent yet"
        );
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ($1, 11, 11, 'open_recoverable_gap', '2026-08-12T09:00:06Z', '2026-08-12T09:00:06Z')")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();

        let backfill = [(11, 2_500, at(base, 210), 6, GapBackfill)];
        let first = report_with_blocks("0195f2a1-0013-4013-8013-000000000322", 2, &backfill);
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;
        // Block 11 lands in its own minute with its interval to block 10...
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 180)).await,
            Some((1, 6, 6, 6, 1, Some(1_500), Some(1_500)))
        );
        // ...and completes block 12's interval in block 12's minute.
        let completed = Some((2, 7, 3, 4, 1, Some(2_500), Some(2_500)));
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            completed
        );
        let five = Some((3, 13, 3, 6, 2, Some(1_500), Some(2_500)));
        assert_eq!(
            block_bucket(&state, &node_id, 300, &at(base, 0)).await,
            five
        );

        // The gap is closed now: the same backfill again, and a backfill below
        // the high-water mark outside any open gap, are both refused.
        let again = report_with_blocks("0195f2a1-0013-4013-8013-000000000323", 3, &backfill);
        submit(&state, &agent_id, serde_json::to_vec(&again).unwrap()).await;
        let outside = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000324",
            4,
            &[(9, 500, at(base, 220), 9, GapBackfill)],
        );
        submit(&state, &agent_id, serde_json::to_vec(&outside).unwrap()).await;
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            completed
        );
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 180)).await,
            Some((1, 6, 6, 6, 1, Some(1_500), Some(1_500)))
        );
        assert_eq!(
            block_bucket(&state, &node_id, 300, &at(base, 0)).await,
            five
        );
    }

    /// Issue #266: a backfill that lands strictly inside an open gap splits the
    /// interval, so the still-missing range above it keeps its own bounds
    /// instead of being reported covered. The upper half is a copy of the same
    /// gap, so it keeps the gap's own `created_at` rather than the receipt time.
    #[tokio::test]
    async fn a_backfill_inside_an_open_gap_splits_the_interval() {
        use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let live = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000341",
            1,
            &[(10, 1_000, at(base, 1), 3, Subscription)],
        );
        let node_id = live.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ($1, 11, 20, 'open_recoverable_gap', '2026-08-12T09:00:06Z', '2026-08-12T09:00:06Z')")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();

        let backfill = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000342",
            2,
            &[(15, 5_000, at(base, 5), 4, GapBackfill)],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&backfill).unwrap()).await;
        assert!(
            receipt.samples[0].rejection.is_none(),
            "the backfill inside the open gap is accepted, got {:?}",
            receipt.samples[0].rejection
        );

        assert_eq!(
            coverage_intervals(&state, &node_id).await,
            vec![
                (10, 10, "covered".to_owned()),
                (11, 14, "open_recoverable_gap".to_owned()),
                (16, 20, "open_recoverable_gap".to_owned()),
            ]
        );
        let halves: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT first_height, last_height, created_at FROM block_coverage_intervals WHERE node_id=$1 AND status='open_recoverable_gap' ORDER BY first_height",
        )
        .bind(&node_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            halves,
            vec![
                (11, 14, "2026-08-12T09:00:06Z".to_owned()),
                (16, 20, "2026-08-12T09:00:06Z".to_owned()),
            ],
            "both halves are still the same gap, so they keep its created_at"
        );
    }

    /// Issue #266: a backfill that sits on an open gap's own edge narrows the
    /// interval instead of splitting, duplicating, or dropping it — the other
    /// two arms of the split that once lost a gap's upper half.
    #[tokio::test]
    async fn a_backfill_on_an_open_gap_edge_narrows_the_interval() {
        use platpulse_core::block::BlockSource::{GapBackfill, Subscription};
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        let live = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000343",
            1,
            &[(10, 1_000, at(base, 1), 3, Subscription)],
        );
        let node_id = live.inventory.nodes[0].node_id.to_string();
        submit(&state, &agent_id, serde_json::to_vec(&live).unwrap()).await;
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ($1, 11, 20, 'open_recoverable_gap', '2026-08-12T09:00:06Z', '2026-08-12T09:00:06Z')")
            .bind(&node_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        let lower = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000344",
            2,
            &[(11, 2_500, at(base, 5), 4, GapBackfill)],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&lower).unwrap()).await;
        assert!(
            receipt.samples[0].rejection.is_none(),
            "the backfill on the gap's first height is accepted, got {:?}",
            receipt.samples[0].rejection
        );
        assert_eq!(
            coverage_intervals(&state, &node_id).await,
            vec![
                (10, 10, "covered".to_owned()),
                (12, 20, "open_recoverable_gap".to_owned()),
            ],
            "closing the gap's first height moves its lower bound up by one"
        );

        let upper = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000345",
            3,
            &[(20, 12_000, at(base, 9), 5, GapBackfill)],
        );
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&upper).unwrap()).await;
        assert!(
            receipt.samples[0].rejection.is_none(),
            "the backfill on the gap's last height is accepted, got {:?}",
            receipt.samples[0].rejection
        );
        assert_eq!(
            coverage_intervals(&state, &node_id).await,
            vec![
                (10, 10, "covered".to_owned()),
                (12, 19, "open_recoverable_gap".to_owned()),
            ],
            "closing the gap's last height moves its upper bound down by one"
        );
    }

    /// Issue #248: the buckets are written inside the receipt transaction, so a
    /// Report whose transaction fails after a bucket was already advanced
    /// leaves no bucket - and no Block Summary - behind.
    #[tokio::test]
    async fn a_rolled_back_report_leaves_no_block_bucket() {
        use platpulse_core::block::BlockSource::Subscription;
        let (_dir, state, agent_id) = state_with_agent().await;
        let base = block_base();
        // The first block opens the bucket; the second one's update aborts.
        crate::test_support::install_failure_trigger(
            state.db().pool(),
            "abort_block_bucket",
            "BEFORE",
            "UPDATE",
            "node_block_aggregates",
            "injected bucket failure",
        )
        .await
        .unwrap();
        let report = report_with_blocks(
            "0195f2a1-0013-4013-8013-000000000331",
            1,
            &[
                (10, 1_000, at(base, 1), 3, Subscription),
                (11, 3_000, at(base, 3), 5, Subscription),
            ],
        );
        let status = submit_status(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_ne!(status, StatusCode::OK);
        assert_eq!(block_bucket_rows(&state).await, 0);
        let summaries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(summaries, 0);

        // Without the injected failure the same Report is accepted once.
        crate::test_support::drop_failure_trigger(
            state.db().pool(),
            "abort_block_bucket",
            "node_block_aggregates",
        )
        .await
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let node_id = report.inventory.nodes[0].node_id.to_string();
        assert_eq!(
            block_bucket(&state, &node_id, 60, &at(base, 0)).await,
            Some((2, 8, 3, 5, 1, Some(2_000), Some(2_000)))
        );
    }

    #[tokio::test]
    async fn two_node_inventory_keeps_host_scoped_once_and_nodes_separate() {
        let (_dir, state, agent_id) = state_with_agent().await;
        create_network(
            state.db(),
            "platon-testnet",
            "PlatON Testnet",
            "0x0000000000000000000000000000000000000000000000000000000000000002",
            210426,
            210426,
            "lat",
        )
        .await
        .unwrap();
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let first_node = report.inventory.nodes[0].clone();
        let first_observation = report.nodes[0].clone();
        let second_id = "0195f2a1-0015-4015-8015-000000000015".parse().unwrap();
        let mut second_node = first_node;
        second_node.node_id = second_id;
        second_node.network_key = "platon-testnet".parse().unwrap();
        second_node.rpc_endpoint = "ws://127.0.0.1:6791".parse().unwrap();
        let mut second_observation = first_observation;
        second_observation.node_id = second_id;
        second_observation
            .chain
            .rpc
            .latest
            .as_mut()
            .unwrap()
            .client_version = "fake-platon/testnet".to_owned();
        report.inventory.nodes.push(second_node);
        report.nodes.push(second_observation);
        report.report_id = "0195f2a1-0013-4013-8013-000000000015".parse().unwrap();
        report.validate().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
        assert_eq!(receipt.nodes.len(), 2);
        let node_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE agent_id = $1")
            .bind(&agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(node_count, 2);
        let host_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM current_host_observations WHERE agent_id = $1",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(host_count, 1);
        let node_rpc_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM component_status WHERE agent_id = $1 AND scope = 'node' AND component_key = 'rpc'",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(node_rpc_count, 2);
        let node_identity_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM component_status WHERE agent_id = $1 AND scope = 'node' AND component_key = 'network_identity'",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            node_identity_count, 2,
            "accepted Network Identity observations use the stable snake_case component key"
        );
        let clients: Vec<String> = sqlx::query_scalar(
            "SELECT rpc_client_version FROM current_node_chain_observations ORDER BY node_id",
        )
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            clients,
            vec![
                "PlatON/v1.4.0-unstable/linux-amd64/go1.21.1",
                "fake-platon/testnet"
            ]
        );
    }

    #[tokio::test]
    async fn accepted_empty_inventory_retires_omitted_node() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&original).unwrap()).await;
        let mut empty = original;
        empty.report_sequence = 2;
        empty.report_id = "0195f2a1-0013-4013-8013-000000000099".parse().unwrap();
        empty.inventory.revision = 2;
        empty.inventory.nodes.clear();
        empty.nodes.clear();
        submit(&state, &agent_id, serde_json::to_vec(&empty).unwrap()).await;
        let lifecycle: String =
            sqlx::query_scalar("SELECT lifecycle FROM nodes WHERE node_id = $1")
                .bind("0195f2a1-0014-4014-8014-000000000014")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(lifecycle, "retired");
        assert!(
            state
                .public_realtime()
                .pending_events()
                .iter()
                .find(|event| event.resource == "node" && event.resource_id.is_none())
                .is_some_and(|event| event.revision == event.event_id && event.revision > 0)
        );

        let mut stale = empty.clone();
        stale.report_sequence = 3;
        stale.report_id = "0195f2a1-0013-4013-8013-000000000098".parse().unwrap();
        stale.inventory.revision = 1;
        let stale_receipt = submit(&state, &agent_id, serde_json::to_vec(&stale).unwrap()).await;
        assert_eq!(stale_receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            stale_receipt.rejections[0].code,
            platpulse_core::RejectionCode::InventoryRevisionConflict
        );

        let mut restored: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        restored.report_sequence = 4;
        restored.report_id = "0195f2a1-0013-4013-8013-000000000100".parse().unwrap();
        restored.inventory.revision = 3;
        submit(&state, &agent_id, serde_json::to_vec(&restored).unwrap()).await;
        let lifecycle: String =
            sqlx::query_scalar("SELECT lifecycle FROM nodes WHERE node_id = $1")
                .bind("0195f2a1-0014-4014-8014-000000000014")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(lifecycle, "active");
    }

    #[tokio::test]
    async fn equal_revision_inventory_conflict_does_not_mutate_node_projection() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&original).unwrap()).await;

        let mut conflict = original;
        conflict.report_sequence = 2;
        conflict.report_id = "0195f2a1-0013-4013-8013-000000000106".parse().unwrap();
        conflict.inventory.nodes[0].rpc_endpoint = "ws://127.0.0.1:6799".parse().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&conflict).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            receipt.rejections[0].code,
            platpulse_core::RejectionCode::InventoryRevisionConflict
        );

        let endpoint: String =
            sqlx::query_scalar("SELECT rpc_endpoint FROM nodes WHERE node_id=$1")
                .bind("0195f2a1-0014-4014-8014-000000000014")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(endpoint, "ws://127.0.0.1:6790");
    }

    #[tokio::test]
    async fn equal_revision_inventory_conflict_records_durable_rejection_evidence() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let accepted = submit(&state, &agent_id, serde_json::to_vec(&original).unwrap()).await;
        assert_eq!(accepted.disposition, ReceiptDisposition::Accepted);

        let mut conflict = original.clone();
        conflict.report_sequence = 2;
        conflict.report_id = "0195f2a1-0013-4013-8013-000000000120".parse().unwrap();
        conflict.inventory.nodes[0].rpc_endpoint = "ws://127.0.0.1:6799".parse().unwrap();
        let reported_revision = conflict.inventory.revision;
        let reported_sha256 = conflict.inventory.content_sha256();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&conflict).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            receipt.rejections[0].code,
            platpulse_core::RejectionCode::InventoryRevisionConflict
        );

        // Issue #181: the refusal is durable Server evidence, not only a
        // decoded receipt body: the newest receipt names the deciding code
        // and the revision/hash the refused report declared.
        let (disposition, rejection_code, inventory_revision, inventory_sha256): (
            String,
            Option<String>,
            Option<i64>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT disposition, rejection_code, inventory_revision, inventory_sha256 FROM agent_report_receipts WHERE agent_id=$1 ORDER BY received_at DESC, report_sequence DESC LIMIT 1",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(disposition, "rejected");
        assert_eq!(
            rejection_code.as_deref(),
            Some("inventory_revision_conflict")
        );
        assert_eq!(inventory_revision, Some(reported_revision as i64));
        assert_eq!(
            inventory_sha256.as_deref(),
            Some(reported_sha256.as_str()),
            "the refused report's declared Inventory hash must be recorded"
        );

        // Issue #181: a rejection writes no projection row, so the Admin
        // invalidation is the only signal an already-open Agent page gets. An
        // accepted report publishes node/peer/geo work but never scopes an
        // `agent` event, so this event can only come from the refusal.
        assert!(
            state
                .admin_realtime()
                .pending_events()
                .iter()
                .any(|event| event.resource == "agent"
                    && event.resource_id.as_deref() == Some(agent_id.as_str())),
            "a rejected report must invalidate the Admin view of its Agent"
        );
    }

    #[tokio::test]
    async fn repeated_history_gap_declarations_are_deduplicated() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut first: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        first.history_gaps.push(platpulse_core::HistoryGap {
            node_id: first.inventory.nodes[0].node_id,
            kind: platpulse_core::GapKind::UnrecoverableBackfill,
            from_height: 8,
            to_height: 9,
            reason: "bounded recovery exceeded".to_owned(),
            recorded_at: first.generated_at,
        });
        first.report_id = "0195f2a1-0013-4013-8013-000000000107".parse().unwrap();
        first.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;

        let mut replay = first;
        replay.report_sequence = 2;
        replay.report_id = "0195f2a1-0013-4013-8013-000000000108".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM block_history_gaps WHERE node_id=$1 AND from_height=8 AND to_height=9",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn stale_inventory_revision_is_rejected_without_lifecycle_mutation() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        original.inventory.revision = 2;
        submit(&state, &agent_id, serde_json::to_vec(&original).unwrap()).await;
        let mut stale = original.clone();
        stale.report_sequence = 2;
        stale.report_id = "0195f2a1-0013-4013-8013-000000000101".parse().unwrap();
        stale.inventory.revision = 1;
        let response = handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.clone(),
                credential_id: "test".into(),
            }),
            Extension(RequestId(Arc::from("stale"))),
            Bytes::from(serde_json::to_vec(&stale).unwrap()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let receipt: ReportResponse =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(receipt.receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            receipt.receipt.rejections[0].code,
            platpulse_core::RejectionCode::InventoryRevisionConflict
        );
        let lifecycle: String =
            sqlx::query_scalar("SELECT lifecycle FROM nodes WHERE node_id = $1")
                .bind("0195f2a1-0014-4014-8014-000000000014")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(lifecycle, "active");
    }

    #[tokio::test]
    async fn divergent_hash_is_append_only_idempotent_and_preserves_summary_and_counters() {
        let (_dir, state, _agent_id) = state_with_agent().await;
        let node_id: platpulse_core::identity::NodeId =
            "0195f2a1-0014-4014-8014-000000000014".parse().unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, $2, 'platon-mainnet', 'ws://127.0.0.1:6790', 'active', 'public', 1, $3, $4)")
            .bind(node_id.to_string()).bind("0195f2a1-0011-4011-8011-000000000011").bind("2026-08-12T08:00:00Z").bind("2026-08-12T08:00:00Z").execute(state.db().pool()).await.unwrap();
        let identity = platpulse_core::network::NetworkIdentity {
            genesis_hash: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            chain_id: 210425,
            p2p_network_id: 210425,
            address_hrp: Some("lat".into()),
        };
        let make_sample =
            |block_hash: platpulse_core::hex::Hash32| platpulse_core::block::BlockSummary {
                node_id,
                network_identity: identity.clone(),
                block_number: 42,
                block_hash,
                parent_hash: "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                    .parse()
                    .unwrap(),
                block_timestamp_ms: 1_000,
                observed_at: "2026-08-12T09:00:00Z".parse().unwrap(),
                transaction_count: 3,
                block_interval_ms: None,
                source: platpulse_core::block::BlockSource::Subscription,
                attribution: platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                    "0x1111111111111111111111111111111111111111"
                        .parse()
                        .unwrap(),
                    "test",
                ),
            };
        let hash_a: platpulse_core::hex::Hash32 =
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .parse()
                .unwrap();
        let hash_b: platpulse_core::hex::Hash32 =
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .parse()
                .unwrap();
        let first = make_sample(hash_a.clone());
        let second = make_sample(hash_b);
        let mut tx = state.db().pool().begin().await.unwrap();
        assert!(
            !observe_block_identity(&mut tx, &first, first.observed_at, "2026-08-12T09:00:00Z")
                .await
                .unwrap()
        );
        sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'subscription', $12, 'unknown', 'unknown', 'test', $13)")
            .bind(node_id.to_string()).bind(42_i64).bind(hash_a.to_string()).bind(first.parent_hash.to_string()).bind(identity.genesis_hash.to_string()).bind(210425_i64).bind(210425_i64).bind("lat").bind(1000_i64).bind(first.observed_at.to_string()).bind(3_i64).bind(first.attribution.coinbase.to_string()).bind("2026-08-12T09:00:00Z").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, updated_at) VALUES ($1, 42, 1, 3, 0, $2)").bind(node_id.to_string()).bind("2026-08-12T09:00:00Z").execute(&mut *tx).await.unwrap();
        assert!(
            observe_block_identity(&mut tx, &second, second.observed_at, "2026-08-12T09:01:00Z")
                .await
                .unwrap()
        );
        assert!(
            observe_block_identity(&mut tx, &second, second.observed_at, "2026-08-12T09:01:00Z")
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        let summary_hash: String = sqlx::query_scalar(
            "SELECT block_hash FROM block_summaries WHERE node_id=$1 AND block_number=42",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let counters: (i64, i64) = sqlx::query_as("SELECT cumulative_block_count, cumulative_transaction_count FROM block_history_state WHERE node_id=$1").bind(node_id.to_string()).fetch_one(state.db().pool()).await.unwrap();
        let divergence_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chain_divergence_observations WHERE node_id=$1 AND height=42",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(summary_hash, hash_a.to_string());
        assert_eq!(counters, (1, 3));
        assert_eq!(divergence_count, 1);
    }

    #[tokio::test]
    async fn current_head_regresses_without_rewriting_history_and_restart_preserves_state() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut first: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let node_id = first.inventory.nodes[0].node_id;
        first.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .current_block = 100;
        first.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .highest_block = 100;
        first.report_id = "0195f2a1-0013-4013-8013-000000000104".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;
        // A below-high-water episode is already underway: a real resync lasts
        // far longer than the grace window, so the regression is classified as
        // resyncing rather than a transient poll/subscription skew.
        sqlx::query("UPDATE block_history_state SET historical_high_watermark=100, cumulative_block_count=4, resync_started_at='2026-08-12T08:00:00Z' WHERE node_id=$1")
            .bind(node_id.to_string()).execute(state.db().pool()).await.unwrap();
        let mut replay = first.clone();
        replay.report_sequence = 2;
        replay.report_id = "0195f2a1-0013-4013-8013-000000000105".parse().unwrap();
        replay.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .current_block = 4;
        replay.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .highest_block = 4;
        submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;
        let current: i64 =
            sqlx::query_scalar("SELECT current_head FROM block_history_state WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        let high: i64 = sqlx::query_scalar(
            "SELECT historical_high_watermark FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let cumulative: i64 = sqlx::query_scalar(
            "SELECT cumulative_block_count FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let state_name: String =
            sqlx::query_scalar("SELECT resync_state FROM block_history_state WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(current, 4);
        assert_eq!(high, 100);
        assert_eq!(cumulative, 4);
        assert_eq!(state_name, "resyncing");
        // The restart asks for the same directory, so the shared helper hands
        // back the same database on either Storage Backend (ADR 0013).
        // A restart is a new process: the previous connection is gone before
        // the next one opens. The Server holds exclusive locking, so the
        // AppState-owned pool must be closed here to model that.
        state.db().close().await;
        let reopened = crate::test_support::test_database_in(_dir.path()).await;
        let persisted: (i64, i64, String) = sqlx::query_as("SELECT current_head, historical_high_watermark, resync_state FROM block_history_state WHERE node_id=$1").bind(node_id.to_string()).fetch_one(reopened.pool()).await.unwrap();
        assert_eq!(persisted, (4, 100, "resyncing".to_owned()));
    }

    #[tokio::test]
    async fn transient_below_high_water_sample_does_not_flap_resync() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut first: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let node_id = first.inventory.nodes[0].node_id;
        first.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .current_block = 100;
        first.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .highest_block = 100;
        first.report_id = "0195f2a1-0013-4013-8013-000000000204".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;
        sqlx::query("UPDATE block_history_state SET historical_high_watermark=100, cumulative_block_count=4 WHERE node_id=$1")
            .bind(node_id.to_string()).execute(state.db().pool()).await.unwrap();

        // The subscription observed a block that the polled Chain Head had not
        // returned yet. That is observation skew, not a resync, and a healthy
        // Node must not flap on it.
        let mut replay = first.clone();
        replay.report_sequence = 2;
        replay.report_id = "0195f2a1-0013-4013-8013-000000000205".parse().unwrap();
        replay.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .current_block = 4;
        replay.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .highest_block = 4;
        submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;

        let (state_name, episode_started_at, progress_at): (
            String,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT resync_state, resync_started_at, resync_last_progress_at FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(state_name, "normal");
        assert!(
            episode_started_at.is_some(),
            "a below-high-water sample still opens an episode clock"
        );
        assert_eq!(
            progress_at.as_deref(),
            Some("2026-08-12T09:00:00Z"),
            "a regressed head must not advance the last-progress time"
        );
    }

    #[tokio::test]
    async fn sustained_below_high_water_marks_resyncing_and_catch_up_clears_it() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut base: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let node_id = base.inventory.nodes[0].node_id;
        base.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .current_block = 100;
        base.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .highest_block = 100;
        base.report_id = "0195f2a1-0013-4013-8013-000000000304".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&base).unwrap()).await;
        sqlx::query("UPDATE block_history_state SET historical_high_watermark=100, cumulative_block_count=4 WHERE node_id=$1")
            .bind(node_id.to_string()).execute(state.db().pool()).await.unwrap();

        let body = |sequence: u64, report_id: &str, head: u64| {
            let mut report = base.clone();
            report.report_sequence = sequence;
            report.report_id = report_id.parse().unwrap();
            report.nodes[0]
                .chain
                .sync
                .latest
                .as_mut()
                .unwrap()
                .current_block = head;
            report.nodes[0]
                .chain
                .sync
                .latest
                .as_mut()
                .unwrap()
                .highest_block = head;
            serde_json::to_vec(&report).unwrap()
        };

        // First below-high-water report: the episode opens but stays normal.
        submit(
            &state,
            &agent_id,
            body(2, "0195f2a1-0013-4013-8013-000000000305", 4),
        )
        .await;
        // Age the episode past the grace window, then report the same head.
        sqlx::query("UPDATE block_history_state SET resync_started_at='2026-08-12T08:00:00Z' WHERE node_id=$1")
            .bind(node_id.to_string()).execute(state.db().pool()).await.unwrap();
        submit(
            &state,
            &agent_id,
            body(3, "0195f2a1-0013-4013-8013-000000000306", 4),
        )
        .await;
        let state_name: String =
            sqlx::query_scalar("SELECT resync_state FROM block_history_state WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(state_name, "resyncing");

        // Re-reaching the high-water mark clears the episode.
        submit(
            &state,
            &agent_id,
            body(4, "0195f2a1-0013-4013-8013-000000000307", 101),
        )
        .await;
        let cleared: (String, Option<String>) = sqlx::query_as(
            "SELECT resync_state, resync_started_at FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(cleared, ("normal".to_owned(), None));
    }

    #[tokio::test]
    async fn failed_node_keeps_host_and_other_node_projection() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let second_id = "0195f2a1-0015-4015-8015-000000000015".parse().unwrap();
        let mut second = report.inventory.nodes[0].clone();
        second.node_id = second_id;
        second.rpc_endpoint = "ws://127.0.0.1:6791".parse().unwrap();
        report.inventory.nodes.push(second);
        let mut second_obs = report.nodes[0].clone();
        second_obs.node_id = second_id;
        second_obs.chain.rpc.latest.as_mut().unwrap().client_version = "other".into();
        report.nodes.push(second_obs);
        report.report_id = "0195f2a1-0013-4013-8013-000000000102".parse().unwrap();
        report.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let mut failed = report.clone();
        failed.report_sequence = 2;
        failed.report_id = "0195f2a1-0013-4013-8013-000000000103".parse().unwrap();
        failed.nodes[0].chain.rpc.status = ComponentStatus::Error;
        failed.nodes[0].chain.rpc.latest = None;
        failed.nodes[0].chain.rpc.latest_observed_at = None;
        failed.nodes[0].chain.rpc.value_revision = 0;
        failed.nodes[0].chain.rpc.error = Some(platpulse_core::component::BoundedError {
            code: "rpc_unreachable".into(),
            message: "RPC probe failed".into(),
        });
        failed.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&failed).unwrap()).await;
        let client: String = sqlx::query_scalar(
            "SELECT rpc_client_version FROM current_node_chain_observations WHERE node_id = $1",
        )
        .bind(second_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(client, "other");
        let memory: i64 = sqlx::query_scalar(
            "SELECT memory_used_bytes FROM current_host_observations WHERE agent_id = $1",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(memory, 4_294_967_296);
        let state: String = sqlx::query_scalar(
            "SELECT state FROM component_status WHERE node_id = $1 AND component_key = 'rpc'",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(state, "error");
    }

    /// Source Agent owns the fixture Node; a second Agent exists as a
    /// transfer target. The source report deliberately keeps the fixture
    /// identity (which mismatches the Registry) so identity assertions in
    /// transfer tests are deterministic.
    async fn state_with_source_and_target() -> (TempDir, AppState, String, String, String) {
        let (dir, state, source_agent) = state_with_agent().await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(
            &state,
            &source_agent,
            serde_json::to_vec(&source_report).unwrap(),
        )
        .await;
        let target_agent = "0195f2a1-0019-4019-8019-000000000019".to_owned();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES ($1, 1, NULL, NULL, NULL, $2, $3)")
            .bind(&target_agent)
            .bind("2026-08-12T08:00:00Z")
            .bind("2026-08-12T08:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        (
            dir,
            state,
            source_agent,
            target_agent,
            source_report.inventory.nodes[0].node_id.to_string(),
        )
    }

    /// Build the target Agent's declaration of the source-owned Node. The
    /// report identity matches the registered Registry tuple when
    /// `matching_identity` is true (genesis `0x…01`, chain/p2p 210425,
    /// hrp `lat`); otherwise the fixture's contradictory identity is kept.
    fn target_declaration(
        source_report: &AgentReport,
        target_agent: &str,
        matching_identity: bool,
        with_sample: bool,
    ) -> AgentReport {
        let mut report = source_report.clone();
        report.agent_id = target_agent.parse().unwrap();
        report.boot_id = "0195f2a1-0034-4034-8034-000000000034".parse().unwrap();
        report.previous_boot_id = None;
        report.report_sequence = 1;
        report.report_id = "0195f2a1-0035-4035-8035-000000000035".parse().unwrap();
        if matching_identity {
            let identity = report.nodes[0]
                .chain
                .network_identity
                .latest
                .as_mut()
                .unwrap();
            identity.genesis_hash =
                "0x0000000000000000000000000000000000000000000000000000000000000001"
                    .parse()
                    .unwrap();
            identity.address_hrp = Some("lat".to_owned());
        }
        if with_sample {
            let identity = report.nodes[0]
                .chain
                .network_identity
                .latest
                .clone()
                .unwrap();
            report
                .block_summaries
                .push(platpulse_core::block::BlockSummary {
                    node_id: report.inventory.nodes[0].node_id,
                    network_identity: identity,
                    block_number: 10,
                    block_hash:
                        "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .parse()
                            .unwrap(),
                    parent_hash:
                        "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                            .parse()
                            .unwrap(),
                    block_timestamp_ms: 1_000,
                    observed_at: report.generated_at,
                    transaction_count: 3,
                    block_interval_ms: None,
                    source: platpulse_core::block::BlockSource::Subscription,
                    attribution:
                        platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                            "0x1111111111111111111111111111111111111111"
                                .parse()
                                .unwrap(),
                            "test",
                        ),
                });
        }
        report.validate().unwrap();
        report
    }

    async fn pending_transfer(state: &AppState, node_id: &str, source: &str, target: &str) {
        // Windows are relative to the real clock so the fixture never
        // silently expires mid-suite.
        let now = now_utc();
        let created = format_rfc3339(now - time::Duration::hours(2));
        let expires = format_rfc3339(now + time::Duration::hours(2));
        sqlx::query(
            "INSERT INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, operator_reason, created_at, expires_at, updated_at) VALUES ('transfer-pending-1', $1, $2, $3, 'pending', 'move the validator', $4, $5, $6)",
        )
        .bind(node_id.to_string())
        .bind(source)
        .bind(target)
        .bind(&created)
        .bind(&expires)
        .bind(&created)
        .execute(state.db().pool())
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn matching_declaration_completes_transfer_atomically_and_source_loses_ownership() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let declaration = target_declaration(&source_report, &target_agent, true, true);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Accepted);
        assert_eq!(
            receipt.samples[0].disposition,
            SampleDispositionKind::Accepted
        );

        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, target_agent);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "completed");
        let completed: Option<String> = sqlx::query_scalar(
            "SELECT completed_at FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(completed.is_some());
        // The same transaction merged the matching sample into history.
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 1);
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_completed'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
        // The target's security counter stays clean: this was a valid
        // declaration, not a contradiction.
        let target_events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(target_events, 0);

        // The source Agent's later submission of the same Node only rejects
        // that Node entry and records a security event; ownership stays with
        // the target (design §4.6).
        let mut stale_source = source_report.clone();
        stale_source.report_sequence = 2;
        stale_source.report_id = "0195f2a1-0013-4013-8013-000000000301".parse().unwrap();
        let source_receipt = submit(
            &state,
            &source_agent,
            serde_json::to_vec(&stale_source).unwrap(),
        )
        .await;
        assert_eq!(
            source_receipt.disposition,
            ReceiptDisposition::PartiallyAccepted
        );
        assert_eq!(
            source_receipt.nodes[0].current,
            NodeCurrentDisposition::Rejected
        );
        assert_eq!(
            source_receipt.nodes[0].rejections[0].code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        let owner_after: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner_after, target_agent);
        let source_events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&source_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(source_events, 1);
    }

    #[tokio::test]
    async fn identity_mismatch_blocks_transfer_and_never_merges_history() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        // The fixture identity (0xaaa…, no hrp) contradicts the Registry.
        let declaration = target_declaration(&source_report, &target_agent, false, true);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        assert_eq!(
            receipt.nodes[0].rejections[0].code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "identity_mismatch");
        let fields: String = sqlx::query_scalar(
            "SELECT mismatched_fields FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&fields).unwrap(),
            vec!["genesis_hash", "address_hrp"]
        );
        // Ownership never switched and no history merged.
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 0);
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_identity_mismatch'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
        let events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(events, 1);
    }

    #[tokio::test]
    async fn declaration_without_identity_probe_keeps_transfer_pending() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let mut declaration = target_declaration(&source_report, &target_agent, true, false);
        declaration.nodes[0].chain.network_identity.status = ComponentStatus::Error;
        declaration.nodes[0].chain.network_identity.latest = None;
        declaration.nodes[0]
            .chain
            .network_identity
            .latest_observed_at = None;
        declaration.nodes[0].chain.network_identity.error =
            Some(platpulse_core::component::BoundedError {
                code: "rpc_unreachable".into(),
                message: "identity probe failed".into(),
            });
        declaration.validate().unwrap();
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "pending");
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        // An in-flight declaration without a contradiction is not a
        // security event.
        let events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(events, 0);
    }

    #[tokio::test]
    async fn expired_transfer_is_materialized_and_declaration_stays_rejected() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        sqlx::query(
            "INSERT INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, created_at, expires_at, updated_at) VALUES ('transfer-expired-1', $1, $2, $3, 'pending', '2026-08-10T08:00:00Z', '2026-08-11T08:00:00Z', '2026-08-10T08:00:00Z')",
        )
        .bind(node_id.to_string())
        .bind(&source_agent)
        .bind(&target_agent)
        .execute(state.db().pool())
        .await
        .unwrap();
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let declaration = target_declaration(&source_report, &target_agent, true, false);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-expired-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "expired");
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_expired'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
    }

    #[tokio::test]
    async fn declaration_under_different_network_key_rejects_transfer() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        create_network(
            state.db(),
            "platon-testnet",
            "PlatON Testnet",
            "0x0000000000000000000000000000000000000000000000000000000000000002",
            210426,
            210426,
            "lat",
        )
        .await
        .unwrap();
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let mut declaration = target_declaration(&source_report, &target_agent, true, false);
        declaration.inventory.nodes[0].network_key = "platon-testnet".parse().unwrap();
        declaration.validate().unwrap();
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "rejected");
        let code: String = sqlx::query_scalar(
            "SELECT rejection_code FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(code, "network_key_mismatch");
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
    }

    #[tokio::test]
    async fn matching_sample_never_merges_history_for_ownership_rejected_node() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        // The observation contradicts the Registry (fixture identity), but
        // the block sample claims the registered tuple: the declaration must
        // still be rejected on the observation, and the matching sample must
        // never merge into the registered Network history.
        let mut declaration = target_declaration(&source_report, &target_agent, false, true);
        let mut sample = declaration.block_summaries[0].clone();
        sample.network_identity.genesis_hash =
            "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap();
        sample.network_identity.address_hrp = Some("lat".to_owned());
        declaration.block_summaries[0] = sample;
        declaration.validate().unwrap();
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "identity_mismatch");
        // Neither the identity window nor the summaries nor the counters
        // were touched by the matching sample.
        let window: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_identity_window WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(window, 0);
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 0);
        let counts: Option<(i64, i64)> = sqlx::query_as(
            "SELECT cumulative_block_count, cumulative_transaction_count FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_optional(state.db().pool())
        .await
        .unwrap();
        // The row exists only from the source's earlier report; the
        // rejected declaration never incremented the counters.
        assert_eq!(counts, Some((0, 0)));
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
    }

    /// The Registry tuple of the Network `state_with_agent` registers.
    fn registered_identity() -> platpulse_core::network::NetworkIdentity {
        platpulse_core::network::NetworkIdentity {
            genesis_hash: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            chain_id: 210425,
            p2p_network_id: 210425,
            address_hrp: Some("lat".to_owned()),
        }
    }

    /// The identity a Node served before its build upgrade: the legacy chain
    /// and p2p ids, and a genesis hash that is not the registered one.
    fn pre_upgrade_identity() -> platpulse_core::network::NetworkIdentity {
        platpulse_core::network::NetworkIdentity {
            genesis_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .parse()
                .unwrap(),
            chain_id: 100,
            p2p_network_id: 100,
            address_hrp: Some("lat".to_owned()),
        }
    }

    /// A v1 report whose Node observation is `observed`, carrying one sample
    /// per requested height stamped with `stamp`. The observation is the
    /// Node's own fresh identity; the stamp is what the Agent cached and
    /// replays into every sample until its Block Worker reconnects.
    fn report_with_stamped_samples(
        report_id: &str,
        observed: &platpulse_core::network::NetworkIdentity,
        stamp: &platpulse_core::network::NetworkIdentity,
        heights: &[u64],
    ) -> AgentReport {
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report.report_id = report_id.parse().unwrap();
        let node_id = report.inventory.nodes[0].node_id;
        let observation = &mut report.nodes[0].chain.network_identity;
        observation.status = ComponentStatus::Ok;
        observation.latest = Some(observed.clone());
        observation.latest_observed_at = Some(report.generated_at);
        observation.error = None;
        for height in heights {
            report
                .block_summaries
                .push(platpulse_core::block::BlockSummary {
                    node_id,
                    network_identity: stamp.clone(),
                    block_number: *height,
                    block_hash: format!("0x{:064x}", height + 1).parse().unwrap(),
                    parent_hash: format!("0x{:064x}", height).parse().unwrap(),
                    block_timestamp_ms: 1_000,
                    observed_at: report.generated_at,
                    transaction_count: 3,
                    block_interval_ms: None,
                    source: platpulse_core::block::BlockSource::Subscription,
                    attribution:
                        platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                            "0x1111111111111111111111111111111111111111"
                                .parse()
                                .unwrap(),
                            "test",
                        ),
                });
        }
        report.validate().unwrap();
        report
    }

    async fn recorded_gap_reasons(
        state: &AppState,
        node_id: platpulse_core::identity::NodeId,
    ) -> Vec<(i64, i64, String)> {
        sqlx::query_as(
            "SELECT from_height, to_height, reason FROM block_history_gaps WHERE node_id=$1 ORDER BY from_height, to_height",
        )
        .bind(node_id.to_string())
        .fetch_all(state.db().pool())
        .await
        .unwrap()
    }

    /// The History Gap rows a Node carries with their author, in height order.
    async fn recorded_gap_rows(
        state: &AppState,
        node_id: platpulse_core::identity::NodeId,
    ) -> Vec<(i64, i64, String, String)> {
        sqlx::query_as(
            "SELECT from_height, to_height, reason, authored_by FROM block_history_gaps WHERE node_id=$1 ORDER BY from_height, to_height",
        )
        .bind(node_id.to_string())
        .fetch_all(state.db().pool())
        .await
        .unwrap()
    }

    /// #235 decision 4: after a Node build upgrade the Node reports the
    /// registered Network while the Agent keeps stamping samples with the
    /// identity it cached. Every rejection must name the differing fields with
    /// their stamped and registered values, point at the Agent restart as the
    /// remedy, and leave the Server's own diagnosis as the History Gap reason
    /// operators read — the Agent's later declaration of the same gap may not
    /// replace it.
    #[tokio::test]
    async fn stale_stamp_rejection_names_the_differing_fields_and_the_agent_restart() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000301",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10, 11],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.samples.len(), 2);
        for sample in &receipt.samples {
            assert_eq!(sample.disposition, SampleDispositionKind::TerminalRejected);
            let rejection = sample.rejection.as_ref().unwrap();
            assert_eq!(
                rejection.code,
                platpulse_core::RejectionCode::NetworkIdentityMismatch
            );
            assert!(!rejection.retryable);
            // The reason names every field that actually differed, on both
            // sides, and never invents one that matched.
            assert!(
                rejection.reason.contains(
                    "genesis_hash stamped 0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa registered 0x0000000000000000000000000000000000000000000000000000000000000001"
                ),
                "{}",
                rejection.reason
            );
            assert!(
                rejection
                    .reason
                    .contains("chain_id stamped 100 registered 210425"),
                "{}",
                rejection.reason
            );
            assert!(
                rejection
                    .reason
                    .contains("p2p_network_id stamped 100 registered 210425"),
                "{}",
                rejection.reason
            );
            assert!(
                !rejection.reason.contains("address_hrp"),
                "{}",
                rejection.reason
            );
            // ...and points at the stale side plus the remedy for it.
            assert!(
                rejection
                    .reason
                    .contains("restarting the Agent on that Host is the only remedy"),
                "{}",
                rejection.reason
            );
            assert!(
                rejection
                    .reason
                    .contains("never edited to match a stamped sample"),
                "{}",
                rejection.reason
            );
        }

        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 0);

        // The Server recorded its own diagnosis for exactly the rejected
        // heights, one row per sample.
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 2);
        assert_eq!(gaps[0].0, 10);
        assert_eq!(gaps[0].1, 10);
        assert_eq!(gaps[1].0, 11);
        assert_eq!(gaps[1].1, 11);
        let reason = receipt.samples[0]
            .rejection
            .as_ref()
            .unwrap()
            .reason
            .clone();
        assert_eq!(gaps[0].2, reason);
        assert_eq!(gaps[1].2, reason);

        // The Agent reports the same gap back with its own wording, as the
        // in-process Agent does once it applies the Receipt. The unique gap
        // identity folds it into the Server's row, so the diagnosis survives.
        let mut declared = report.clone();
        declared.report_sequence += 1;
        declared.report_id = "0195f2a1-0013-4013-8013-000000000302".parse().unwrap();
        declared.block_summaries.clear();
        declared.history_gaps = [10u64, 11]
            .into_iter()
            .map(|height| platpulse_core::gap::HistoryGap {
                node_id,
                kind: platpulse_core::gap::GapKind::ServerRejected,
                from_height: height,
                to_height: height,
                reason: "locally retained history gap".to_owned(),
                recorded_at: report.generated_at,
            })
            .collect();
        declared.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert_eq!(recorded_gap_reasons(&state, node_id).await, gaps);
    }

    /// One Report can carry more than one Block Identity Stamp for the same
    /// Node: samples spooled across a subscription boundary keep the stamp
    /// they were collected with, and the wire format validates each sample's
    /// stamp on its own. Every rejection must name the fields that actually
    /// differed for *its* sample (ADR 0011 decision 4), and the History Gap
    /// the Server leaves behind must carry that same text.
    #[tokio::test]
    async fn each_stamped_sample_is_diagnosed_on_its_own_stamp() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000305",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let mut hrp_stamp = registered_identity();
        hrp_stamp.address_hrp = Some("lax".to_owned());
        let mut second = report.block_summaries[0].clone();
        second.block_number = 11;
        second.block_hash = format!("0x{:064x}", 12).parse().unwrap();
        second.parent_hash = format!("0x{:064x}", 11).parse().unwrap();
        second.network_identity = hrp_stamp;
        report.block_summaries.push(second);
        report.validate().unwrap();

        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.samples.len(), 2);
        let rejection_at = |height: u64| {
            receipt
                .samples
                .iter()
                .find(|sample| {
                    sample.sample == platpulse_core::receipt::SampleRef::Block { height }
                })
                .and_then(|sample| sample.rejection.clone())
                .unwrap()
        };

        let legacy = rejection_at(10);
        assert_eq!(
            legacy.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            legacy.reason.contains("genesis_hash stamped 0xaaaa")
                && legacy
                    .reason
                    .contains("chain_id stamped 100 registered 210425")
                && !legacy.reason.contains("address_hrp"),
            "{}",
            legacy.reason
        );

        let hrp = rejection_at(11);
        assert_eq!(
            hrp.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            hrp.reason
                .contains("address_hrp stamped lax registered lat")
                && !hrp.reason.contains("chain_id")
                && !hrp.reason.contains("genesis_hash"),
            "{}",
            hrp.reason
        );

        // Each refused height carries its own sample's diagnosis instead of the
        // first sample's fields repeated for the whole Node.
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 2);
        assert_eq!((gaps[0].0, gaps[0].1), (10, 10));
        assert_eq!(gaps[0].2, legacy.reason);
        assert_eq!((gaps[1].0, gaps[1].1), (11, 11));
        assert_eq!(gaps[1].2, hrp.reason);
    }

    /// The diagnosis key is built from the stamp's own fields, so a stamp that
    /// carries no HRP can never share the key of a stamp whose HRP reads
    /// "unset" - the literal is a legal Registry value, and collapsing the two
    /// would reject a matching sample on a sibling sample's contradiction.
    #[test]
    fn an_absent_hrp_is_not_the_stamp_registered_as_unset() {
        let node_id: platpulse_core::identity::NodeId =
            "0195f2a1-0013-4013-8013-000000000307".parse().unwrap();
        let mut absent = registered_identity();
        absent.address_hrp = None;
        let mut literal = registered_identity();
        literal.address_hrp = Some("unset".to_owned());

        assert_ne!(
            StampedIdentityKey::new(node_id, &absent),
            StampedIdentityKey::new(node_id, &literal)
        );
        // Equal stamps still share a key: the same contradicted stamp must not
        // be diagnosed twice.
        assert_eq!(
            StampedIdentityKey::new(node_id, &literal),
            StampedIdentityKey::new(node_id, &literal.clone())
        );
    }

    /// A Network registered under the literal HRP "unset" is a legal Registry
    /// tuple, and a sample whose stamp matches it must be accepted even when a
    /// sibling sample of the same Report carries no HRP at all. Before the
    /// diagnosis was keyed by an explicit field list both stamps produced the
    /// same key, so the matching summary was refused (and a permanent History
    /// Gap invented for it) while the data plane stored it anyway.
    #[tokio::test]
    async fn samples_are_diagnosed_by_their_own_stamp_not_by_a_sibling_ones() {
        let (_dir, state, agent_id) = state_with_agent().await;
        sqlx::query("UPDATE networks SET address_hrp = 'unset'")
            .execute(state.db().pool())
            .await
            .unwrap();

        let mut registered = registered_identity();
        registered.address_hrp = Some("unset".to_owned());
        let mut absent = registered.clone();
        absent.address_hrp = None;
        let mut report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000307",
            &registered,
            &absent,
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let mut matching = report.block_summaries[0].clone();
        matching.block_number = 11;
        matching.block_hash = format!("0x{:064x}", 12).parse().unwrap();
        matching.parent_hash = format!("0x{:064x}", 11).parse().unwrap();
        matching.network_identity = registered.clone();
        report.block_summaries.push(matching);
        report.validate().unwrap();

        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        // Both samples must be reported on by the Receipt: a missing entry
        // would let the "no rejection" assertion below pass for the wrong
        // reason, so each height is looked up by presence, not by absence.
        let sample_at = |height: u64| {
            receipt
                .samples
                .iter()
                .find(|sample| {
                    sample.sample == platpulse_core::receipt::SampleRef::Block { height }
                })
                .unwrap_or_else(|| panic!("the Receipt reports the sample for height {height}"))
                .clone()
        };
        assert_eq!(receipt.samples.len(), 2);

        let refused = sample_at(10);
        assert_eq!(refused.disposition, SampleDispositionKind::TerminalRejected);
        let absent = refused
            .rejection
            .expect("the HRP-less stamp is contradicted");
        assert_eq!(
            absent.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        // The refusal names the missing HRP without pretending it was this
        // Network's registered value.
        assert!(
            absent
                .reason
                .contains("address_hrp stamped not reported registered unset"),
            "{}",
            absent.reason
        );
        // The sibling stamp that genuinely matches the Registry is accepted on
        // its own merits, not condemned through a key it shares with a
        // normalized spelling of the absent HRP.
        let matched = sample_at(11);
        assert_eq!(matched.disposition, SampleDispositionKind::Accepted);
        assert!(
            matched.rejection.is_none(),
            "the matching stamp is accepted, not judged by its sibling"
        );

        let accepted: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id = $1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(accepted, 1);
        // The data plane, the Receipt and the Server's own gap row agree: the
        // accepted summary is counted once and only the contradicted height is
        // refused.
        let counted: i64 = sqlx::query_scalar(
            "SELECT cumulative_block_count FROM block_history_state WHERE node_id = $1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(counted, 1);
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!((gaps[0].0, gaps[0].1), (10, 10));
        assert_eq!(gaps[0].2, absent.reason);
    }

    /// A contradicted stamp is the more specific finding about a sample, so it
    /// outranks the generic refusals in the Receipt: a GapBackfill sample
    /// outside an open recoverable gap reports the identity contradiction the
    /// Server records on its History Gap, instead of two surfaces naming two
    /// different causes. A sample whose stamp matches the Registry keeps the
    /// backfill-protocol rejection it always had.
    #[tokio::test]
    async fn identity_refusal_agrees_with_the_gap_it_records() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut mismatched = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000306",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = mismatched.inventory.nodes[0].node_id;
        mismatched.block_summaries[0].source = platpulse_core::block::BlockSource::GapBackfill;
        mismatched.validate().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&mismatched).unwrap()).await;
        let rejection = receipt.samples[0].rejection.as_ref().unwrap();
        assert_eq!(
            rejection.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            rejection.reason.contains("genesis_hash stamped 0xaaaa"),
            "{}",
            rejection.reason
        );
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].2, rejection.reason);

        // The same sample shape with a matching stamp is still refused for the
        // backfill protocol, and the Server records no identity gap for it.
        let mut matching = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000307",
            &registered_identity(),
            &registered_identity(),
            &[20],
        );
        matching.report_sequence = mismatched.report_sequence + 1;
        matching.block_summaries[0].source = platpulse_core::block::BlockSource::GapBackfill;
        matching.validate().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&matching).unwrap()).await;
        let rejection = receipt.samples[0].rejection.as_ref().unwrap();
        assert_eq!(
            rejection.code,
            platpulse_core::RejectionCode::GapBackfillOutsideOpenGap
        );
        assert_eq!(recorded_gap_reasons(&state, node_id).await, gaps);
    }

    /// History Gap rows carry their author: the Server writes its own refusals
    /// as 'server', and a gap an Agent declares about itself stays 'agent'
    /// evidence. Only the Server's own rows may be presented as its live
    /// refusal diagnosis, so a reporting Agent cannot fabricate one
    /// (AGENTS.md: the Server is the trust boundary).
    #[tokio::test]
    async fn history_gaps_carry_their_author() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000308",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let server_reason = receipt.samples[0]
            .rejection
            .as_ref()
            .unwrap()
            .reason
            .clone();
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![(10, 10, server_reason.clone(), "server".to_owned())]
        );

        // The Agent applies the Receipt and reports the same gap back with its
        // own wording, plus a gap of its own that the Server never refused. The
        // Server's row keeps its text and its author, and the Agent's row is
        // stored as the Agent's.
        let mut declared = report.clone();
        declared.report_sequence += 1;
        declared.report_id = "0195f2a1-0013-4013-8013-000000000309".parse().unwrap();
        declared.block_summaries.clear();
        declared.history_gaps = [10u64, 12]
            .into_iter()
            .map(|height| platpulse_core::gap::HistoryGap {
                node_id,
                kind: platpulse_core::gap::GapKind::ServerRejected,
                from_height: height,
                to_height: height,
                reason: "locally retained history gap".to_owned(),
                recorded_at: report.generated_at,
            })
            .collect();
        declared.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![
                (10, 10, server_reason, "server".to_owned()),
                (
                    12,
                    12,
                    "locally retained history gap".to_owned(),
                    "agent".to_owned()
                ),
            ]
        );
    }

    /// A refusal is the Server's own finding, so it must correct a row the
    /// Agent declared first for the same height: the Agent's insert keeps its
    /// hands off an existing row, and the Server's upsert owns the row it
    /// refused. Otherwise a reporting Agent could dictate what the Admin
    /// diagnosis says about a refusal the Server made (AGENTS.md: the Server
    /// is the trust boundary).
    #[tokio::test]
    async fn a_server_refusal_corrects_the_gap_the_agent_declared_first() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut declared = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000310",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[],
        );
        let node_id = declared.inventory.nodes[0].node_id;
        declared.history_gaps = std::iter::once(platpulse_core::gap::HistoryGap {
            node_id,
            kind: platpulse_core::gap::GapKind::ServerRejected,
            from_height: 10,
            to_height: 10,
            reason: "locally retained history gap".to_owned(),
            recorded_at: declared.generated_at,
        })
        .collect();
        declared.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&declared).unwrap()).await;
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![(
                10,
                10,
                "locally retained history gap".to_owned(),
                "agent".to_owned()
            )]
        );

        let mut refused = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000311",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        refused.report_sequence = declared.report_sequence + 1;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&refused).unwrap()).await;
        let server_reason = receipt.samples[0]
            .rejection
            .as_ref()
            .unwrap()
            .reason
            .clone();
        assert_eq!(
            recorded_gap_rows(&state, node_id).await,
            vec![(10, 10, server_reason, "server".to_owned())]
        );
        // The Server also stamps the row with its own clock instead of keeping
        // the Agent's claim about when the refusal happened. The Receipt's
        // `server_time` and the row's `created_at` are both canonical
        // second-precision UTC text, and the row's text is minted first, so it
        // can only sort at or before the Receipt's time — never at the Agent's
        // past-dated `recorded_at`.
        let (created_at, authored_by): (String, String) = sqlx::query_as(
            "SELECT created_at, authored_by FROM block_history_gaps WHERE node_id = $1 AND from_height = 10 AND to_height = 10",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(authored_by, "server");
        assert_ne!(
            created_at,
            declared.generated_at.to_string(),
            "the Agent's own timestamp must not survive the Server's refusal"
        );
        assert!(created_at.as_str() <= receipt.server_time.to_string().as_str());
    }

    /// Acceptance evidence for the Admin refusal diagnosis outlives raw Block
    /// Summary retention: the Server stamps the moment it last accepted a
    /// summary on the durable per-Node history state (ADR 0011 decision 4).
    #[tokio::test]
    async fn accepted_history_records_when_it_resumed() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000310",
            &registered_identity(),
            &registered_identity(),
            &[20, 21],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);

        let accepted: String = sqlx::query_scalar(
            "SELECT accepted_at FROM block_summaries WHERE node_id=$1 ORDER BY block_number DESC LIMIT 1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let recorded: Option<String> = sqlx::query_scalar(
            "SELECT last_accepted_summary_at FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            recorded,
            Some(accepted),
            "the acceptance stamp is the same Server-authored text as the summary it proves"
        );
    }

    /// A Node that really serves another Network is not a stale stamp: no
    /// Agent restart can reconcile it, and the reason must say so instead of
    /// sending an operator through the restart runbook.
    #[tokio::test]
    async fn cross_network_conflict_is_not_reported_as_a_stale_stamp() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000303",
            &pre_upgrade_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        let rejection = receipt.samples[0].rejection.as_ref().unwrap();
        assert_eq!(
            rejection.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            rejection.reason.contains(
                "genuine cross-Network conflict that restarting the Agent cannot reconcile"
            ),
            "{}",
            rejection.reason
        );
        assert!(
            !rejection.reason.contains("is the only remedy"),
            "{}",
            rejection.reason
        );
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].2, rejection.reason);
    }

    /// Without a complete successful identity observation the Server cannot
    /// tell a stale stamp from a cross-Network conflict, and the reason must
    /// say that rather than guess.
    #[tokio::test]
    async fn unobserved_identity_is_not_reported_as_a_stale_stamp() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_with_stamped_samples(
            "0195f2a1-0013-4013-8013-000000000304",
            &registered_identity(),
            &pre_upgrade_identity(),
            &[10],
        );
        report.nodes[0].chain.network_identity.status = ComponentStatus::Error;
        report.nodes[0].chain.network_identity.latest = None;
        report.nodes[0].chain.network_identity.latest_observed_at = None;
        report.nodes[0].chain.network_identity.error =
            Some(platpulse_core::component::BoundedError {
                code: "rpc_unreachable".into(),
                message: "identity probe failed".into(),
            });
        report.validate().unwrap();
        let node_id = report.inventory.nodes[0].node_id;
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        let rejection = receipt.samples[0].rejection.as_ref().unwrap();
        assert_eq!(
            rejection.code,
            platpulse_core::RejectionCode::NetworkIdentityMismatch
        );
        assert!(
            rejection
                .reason
                .contains("no complete successful Network Identity observation"),
            "{}",
            rejection.reason
        );
        assert!(
            rejection.reason.contains(
                "Confirm the Node's current identity and then restart the Agent on that Host"
            ),
            "{}",
            rejection.reason
        );
        assert!(
            !rejection.reason.contains("is the only remedy"),
            "{}",
            rejection.reason
        );
        let gaps = recorded_gap_reasons(&state, node_id).await;
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].2, rejection.reason);
    }

    /// The widest reason this can render is four differing fields with 32-byte
    /// hashes on both sides. It must stay inside the bounded diagnostic budget
    /// the Receipt contract validates against, in every classification and
    /// after redaction.
    #[test]
    fn identity_rejection_reason_stays_inside_the_diagnostic_budget() {
        let hex = |lead: char| format!("0x{}", lead.to_string().repeat(64));
        let field =
            |name: &'static str, stamped: String, registered: String| StampedIdentityField {
                field: name,
                stamped,
                registered,
            };
        for divergence in [
            StampDivergence::StaleStamp,
            StampDivergence::NetworkConflict,
            StampDivergence::Unobserved,
        ] {
            let rejection = BlockIdentityRejection {
                fields: vec![
                    field("genesis_hash", hex('a'), hex('b')),
                    field(
                        "chain_id",
                        "18446744073709551615".to_owned(),
                        "210425".to_owned(),
                    ),
                    field(
                        "p2p_network_id",
                        "18446744073709551615".to_owned(),
                        "210425".to_owned(),
                    ),
                    field("address_hrp", hex('c'), hex('d')),
                ],
                divergence,
            };
            let reason = crate::redaction::redact_sensitive(&rejection.reason());
            assert!(
                reason.len() <= platpulse_core::protocol::MAX_ERROR_MESSAGE_BYTES,
                "{:?}: {} bytes",
                divergence,
                reason.len()
            );
        }
    }

    #[tokio::test]
    async fn successful_peer_snapshots_update_both_aggregate_families_once() {
        let (_dir, state, agent_id) = state_with_agent().await;
        // Country aggregation reads the retained results of the selected
        // provider only, exactly like the Public projection does.
        let state = state.with_geo_provider(crate::geo::GeoSelection {
            provider: crate::geo::GeoProvider::LocalMmdb,
            generation: 1,
        });
        let node_id = "0195f2a1-0014-4014-8014-000000000014";
        let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:00Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 1,
            "value_revision": 1,
            "latest": {
                "peers": [
                    {
                        "peer_id": "peer-a",
                        "remote_ip": "8.8.8.8",
                        "direction": "inbound",
                        "trusted": true,
                        "static_peer": false,
                        "consensus_peer": true,
                        "client_name": "PlatON/v1.5.1",
                        "caps": ["cbft/1"],
                        "cbft_protocol_version": 1,
                        "cbft_highest_qc_block": 100,
                        "cbft_locked_block": 99,
                        "cbft_commit_block": 98
                    },
                    {
                        "peer_id": "peer-b",
                        "remote_ip": "192.0.2.10",
                        "direction": "outbound",
                        "trusted": false,
                        "static_peer": true,
                        "consensus_peer": false,
                        "client_name": "PlatON/v1.5.2",
                        "caps": ["cbft/2"]
                    }
                ]
            }
        });
        value["report_id"] = serde_json::json!("0195f2a1-0200-4200-8200-000000000200");
        value["report_sequence"] = serde_json::json!(1);
        let body = serde_json::to_vec(&value).unwrap();
        // A cache row as the Server itself writes it: created now with its
        // absolute expiry capped by the hard retention boundary.
        let cache_now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('local_mmdb', '8.8.8.8', 'US', 'current', $1, $2, $3, $4, $5)")
            .bind(&cache_now)
            .bind(&cache_now)
            .bind(&cache_now)
            .bind(&cache_now)
            .bind(crate::geo::cache_expiry(&cache_now))
            .execute(state.db().pool())
            .await
            .unwrap();
        assert_eq!(
            submit(&state, &agent_id, body.clone()).await.disposition,
            ReceiptDisposition::Accepted
        );
        assert_eq!(
            submit_status(&state, &agent_id, body).await,
            StatusCode::OK,
            "a report replay must return the durable receipt without reapplying it"
        );

        for table in ["peer_aggregate_5m", "peer_aggregate_1h"] {
            let row: (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(&format!(
                "SELECT sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count FROM {table} WHERE node_id=$1"
            ))
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap();
            assert_eq!(row, (1, 2, 1, 1, 1, 1, 1));
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_aggregate_5m_countries WHERE node_id=$1",
            )
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "known public addresses are reduced to country counts only"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT peer_count FROM peer_aggregate_5m_countries WHERE node_id=$1 AND country_code='US'",
            )
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );

        value["report_id"] = serde_json::json!("0195f2a1-0201-4201-8201-000000000201");
        value["report_sequence"] = serde_json::json!(2);
        let second = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(second.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT sample_count FROM peer_aggregate_5m WHERE node_id=$1",
            )
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            2
        );

        value["report_id"] = serde_json::json!("0195f2a1-0202-4202-8202-000000000202");
        value["report_sequence"] = serde_json::json!(3);
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] = serde_json::json!([]);
        let third = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(third.disposition, ReceiptDisposition::Accepted);
        let churn: (i64, i64, i64) = sqlx::query_as(
            "SELECT total_peers, arrivals, departures FROM peer_aggregate_5m WHERE node_id=$1",
        )
        .bind(node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(churn, (4, 0, 2));
        // Report Ingestion no longer writes the cache, so the row outlives the
        // last current Peer reference until the maintenance cycle collects it
        // (issue #271).
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM geo_location_cache")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            1,
            "a receipt must not write the Geo cache"
        );
        let now_text = crate::auth::format_rfc3339(crate::auth::now_utc());
        assert_eq!(
            crate::geo::cleanup_cache(
                state.db().pool(),
                &now_text,
                crate::geo::GeoProvider::LocalMmdb,
            )
            .await
            .unwrap(),
            1,
            "the maintenance cycle drops a row when its last current Peer reference disappears"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM geo_location_cache")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            0
        );
        let history = crate::peer_history::load_history(state.db().pool(), node_id)
            .await
            .unwrap();
        assert_eq!(history.state, "empty");
        assert_eq!(history.five_minute[0].total_peers, 4);
        sqlx::query("UPDATE component_status SET state='starting' WHERE node_id=$1 AND component_key='peers'")
            .bind(node_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        assert_eq!(
            crate::peer_history::load_history(state.db().pool(), node_id)
                .await
                .unwrap()
                .state,
            "starting"
        );
    }

    #[tokio::test]
    async fn aggregate_history_is_isolated_per_node() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let node_a = "0195f2a1-0014-4014-8014-000000000014";
        let node_b = "0195f2a1-0015-4015-8015-000000000015";
        let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let mut second_inventory = value["inventory"]["nodes"][0].clone();
        second_inventory["node_id"] = serde_json::json!(node_b);
        second_inventory["rpc_endpoint"] = serde_json::json!("ws://127.0.0.1:6791");
        value["inventory"]["nodes"] =
            serde_json::json!([value["inventory"]["nodes"][0].clone(), second_inventory]);
        let peer = serde_json::json!({
            "peer_id": "peer-a",
            "direction": "inbound",
            "trusted": true,
            "static_peer": false,
            "consensus_peer": true,
            "caps": []
        });
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:00Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 1,
            "value_revision": 1,
            "latest": {"peers": [peer.clone()]}
        });
        let mut second_node = value["nodes"][0].clone();
        second_node["node_id"] = serde_json::json!(node_b);
        second_node["chain"]["peers"]["latest"]["peers"] = serde_json::json!([{
            "peer_id": "peer-b",
            "direction": "outbound",
            "trusted": false,
            "static_peer": true,
            "consensus_peer": false,
            "caps": []
        }]);
        value["nodes"] = serde_json::json!([value["nodes"][0].clone(), second_node]);
        value["report_id"] = serde_json::json!("0195f2a1-0210-4210-8210-000000000210");
        value["report_sequence"] = serde_json::json!(1);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        for node_id in [node_a, node_b] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM peer_aggregate_5m WHERE node_id=$1",
                )
                .bind(node_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
                1
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT total_peers FROM peer_aggregate_5m WHERE node_id=$1",
            )
            .bind(node_a)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT total_peers FROM peer_aggregate_5m WHERE node_id=$1",
            )
            .bind(node_b)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn declaration_without_pending_transfer_is_an_ownership_conflict() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let declaration = target_declaration(&source_report, &target_agent, true, false);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        let events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=$1")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(events, 1);
    }

    fn v2_with_transition(
        body: Vec<u8>,
        transition: platpulse_core::BootTransition,
        previous_boot_id: Option<&str>,
    ) -> Vec<u8> {
        let mut report: AgentReport<InventoryDeclaration> = serde_json::from_slice(&body).unwrap();
        report.boot_transition = transition;
        report.previous_boot_id = previous_boot_id.map(|id| id.parse().expect("valid boot id"));
        serde_json::to_vec(&report).unwrap()
    }

    /// Issue #187: a Server that committed a receipt but lost the response
    /// returns the *original* receipt for the identical report identity and
    /// bytes, even after a newer declaration was accepted. Different bytes
    /// under the same identity are a conflict.
    #[tokio::test]
    async fn v2_replay_returns_the_original_receipt_and_identity_conflict_is_rejected() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let first_body = v2_report_body(
            "0195f2a1-0200-4200-8200-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let first = submit_v2(&state, &agent_id, first_body.clone()).await;
        assert_eq!(first.disposition, ReceiptDisposition::Accepted);
        assert_eq!(v2_acceptance(&first).revision, 1);
        let first_fingerprint = v2_acceptance(&first).fingerprint;

        let second = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0200-4200-8200-000000000002",
                2,
                1,
                V2_BOOT_ID,
                &["ws://127.0.0.1:6791"],
            ),
        )
        .await;
        let second_fingerprint = v2_acceptance(&second).fingerprint;
        assert_ne!(first_fingerprint, second_fingerprint);
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);

        // The exact original bytes return the exact original receipt and move
        // nothing: no allocation, no projection, no revision change.
        let replayed = submit_v2(&state, &agent_id, first_body).await;
        assert_eq!(replayed, first, "the replay must be the original receipt");
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);
        assert_eq!(
            accepted_inventory_fingerprint(&state, &agent_id)
                .await
                .as_deref(),
            Some(second_fingerprint.as_str()),
            "a replay must not move the accepted declaration back"
        );

        let tampered = v2_report_body(
            "0195f2a1-0200-4200-8200-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6799"],
        );
        let (status, _body) = submit_v2_raw(&state, &agent_id, tampered).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "same identity with different bytes must conflict"
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);
    }

    /// Issue #187: the existing Epoch/Boot/Report Sequence fences stay
    /// authoritative. A late, older, or competing report cannot allocate a new
    /// revision merely because its content differs, while a legitimate newer
    /// report that restores earlier content is still a new declaration change.
    #[tokio::test]
    async fn v2_ordering_fences_refuse_late_reports_without_allocating() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let content_a = "ws://127.0.0.1:6790";
        let content_b = "ws://127.0.0.1:6791";
        let content_c = "ws://127.0.0.1:6792";

        let first = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0210-4210-8210-000000000001",
                1,
                1,
                V2_BOOT_ID,
                &[content_a],
            ),
        )
        .await;
        assert_eq!(v2_acceptance(&first).revision, 1);
        let second = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0210-4210-8210-000000000002",
                3,
                1,
                V2_BOOT_ID,
                &[content_b],
            ),
        )
        .await;
        assert_eq!(v2_acceptance(&second).revision, 2);
        let accepted_before = accepted_inventory_fingerprint(&state, &agent_id).await;
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);

        // A late report with an unused but older sequence and new content is a
        // stale report: refused with no allocation.
        let stale = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0210-4210-8210-000000000003",
                2,
                1,
                V2_BOOT_ID,
                &[content_c],
            ),
        )
        .await;
        assert_eq!(stale.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            stale.rejections[0].code,
            platpulse_core::RejectionCode::StaleReport
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);
        assert_eq!(
            accepted_inventory_fingerprint(&state, &agent_id).await,
            accepted_before
        );

        // A stale Epoch is refused before any allocation.
        let old_epoch = v2_report_body(
            "0195f2a1-0210-4210-8210-000000000004",
            4,
            0,
            V2_BOOT_ID,
            &[content_c],
        );
        assert_eq!(
            submit_v2(&state, &agent_id, old_epoch).await.disposition,
            ReceiptDisposition::Rejected
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);

        // A competing Boot is refused before any allocation.
        let competing_boot = v2_report_body(
            "0195f2a1-0210-4210-8210-000000000005",
            4,
            1,
            "0195f2a1-0212-4212-8212-000000000099",
            &[content_c],
        );
        assert_eq!(
            submit_v2(&state, &agent_id, competing_boot)
                .await
                .disposition,
            ReceiptDisposition::Rejected
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 2);

        // A legitimate newer report that restores earlier content is a new
        // declaration change, not a reuse of the historical revision.
        let restored = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0210-4210-8210-000000000006",
                4,
                1,
                V2_BOOT_ID,
                &[content_a],
            ),
        )
        .await;
        assert_eq!(restored.disposition, ReceiptDisposition::Accepted);
        assert_eq!(v2_acceptance(&restored).revision, 3);
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 3);
    }

    /// Issue #187: the revision stays within the positive signed 64-bit range.
    /// At the ceiling an unchanged declaration is still accepted and a change
    /// that must increment fails explicitly, never wrapping or resetting.
    #[tokio::test]
    async fn v2_revision_exhaustion_rejects_only_a_required_increment() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let body_a = v2_report_body(
            "0195f2a1-0220-4220-8220-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let fingerprint_a = declaration_fingerprint(&body_a);
        sqlx::query(
            "UPDATE agents SET last_inventory_revision = $1, inventory_sha256 = $2 WHERE agent_id = $3",
        )
        .bind(i64::MAX)
        .bind(&fingerprint_a)
        .bind(&agent_id)
        .execute(state.db().pool())
        .await
        .unwrap();

        let unchanged = submit_v2(&state, &agent_id, body_a).await;
        assert_eq!(unchanged.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            unchanged.inventory.as_ref().unwrap().disposition,
            InventoryDisposition::Unchanged
        );
        assert_eq!(v2_acceptance(&unchanged).revision, i64::MAX as u64);
        assert_eq!(
            accepted_inventory_revision(&state, &agent_id).await,
            i64::MAX
        );

        let changed = v2_report_body(
            "0195f2a1-0220-4220-8220-000000000002",
            2,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6791"],
        );
        let rejected = submit_v2(&state, &agent_id, changed).await;
        assert_eq!(rejected.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            rejected.rejections[0].code,
            platpulse_core::RejectionCode::InventoryRevisionConflict
        );
        assert_eq!(
            accepted_inventory_revision(&state, &agent_id).await,
            i64::MAX,
            "exhaustion must keep the existing revision"
        );
        assert_eq!(
            accepted_inventory_fingerprint(&state, &agent_id)
                .await
                .as_deref(),
            Some(fingerprint_a.as_str()),
            "exhaustion must keep the existing fingerprint"
        );
    }

    /// Issue #187: declaration comparison and version allocation live in one
    /// authoritative transaction. Two concurrent declarations that occupy the
    /// same sequence cannot both allocate: exactly one commits revision 1 and
    /// the other is fenced out by the existing sequence barrier.
    #[tokio::test]
    async fn concurrent_v2_ingestion_never_shares_or_duplicates_a_revision() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let left = v2_report_body(
            "0195f2a1-0230-4230-8230-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let right = v2_report_body(
            "0195f2a1-0230-4230-8230-000000000002",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6791"],
        );
        let (left_result, right_result) = tokio::join!(
            submit_v2_raw(&state, &agent_id, left),
            submit_v2_raw(&state, &agent_id, right),
        );
        let statuses = [left_result.0, right_result.0];
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::OK)
                .count(),
            1,
            "exactly one concurrent declaration may commit"
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::CONFLICT)
                .count(),
            1,
            "the loser is refused by the sequence barrier"
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 1);
        let winner = if left_result.0 == StatusCode::OK {
            &left_result.1
        } else {
            &right_result.1
        };
        let receipt: ReportResponseV2 = serde_json::from_slice(winner).unwrap();
        assert_eq!(v2_acceptance(&receipt.receipt).revision, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM agent_report_receipts WHERE agent_id = $1"
            )
            .bind(&agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );
    }

    /// Issue #187: a failed write anywhere in the v2 receipt transaction rolls
    /// back allocation, projection, and receipt, and emits no post-commit
    /// invalidation. An injected trigger is the failure source.
    #[tokio::test]
    async fn v2_receipt_failure_rolls_back_allocation_and_projection() {
        v2_write_failure_rolls_back_all_writes(
            "abort_v2_receipt",
            "BEFORE",
            "INSERT",
            "agent_report_receipts",
            "injected receipt failure",
        )
        .await;
    }

    #[tokio::test]
    async fn v2_projection_failure_rolls_back_allocation_and_receipt() {
        v2_write_failure_rolls_back_all_writes(
            "abort_v2_projection",
            "BEFORE",
            "INSERT",
            "nodes",
            "injected projection failure",
        )
        .await;
    }

    #[tokio::test]
    async fn v2_allocation_failure_rolls_back_projection_and_receipt() {
        v2_write_failure_rolls_back_all_writes(
            "abort_v2_allocation",
            "BEFORE",
            "UPDATE",
            "agents",
            "injected allocation failure",
        )
        .await;
    }

    /// Issue #187: the accepted revision, fingerprint, and exact receipt are
    /// durable. A Server restart must return the original receipt and must not
    /// recompute the accepted pair against the current state.
    #[tokio::test]
    async fn v2_accepted_revision_and_exact_replay_survive_a_server_restart() {
        let (dir, state, agent_id) = state_with_agent().await;
        let body = v2_report_body(
            "0195f2a1-0250-4250-8250-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let first = submit_v2(&state, &agent_id, body.clone()).await;
        let fingerprint = v2_acceptance(&first).fingerprint;
        state.db().close().await;

        let database = crate::test_support::test_database_in(dir.path()).await;
        let auth = AuthConfig::development(
            load_pepper_file(&dir.path().join("pepper")).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let restarted = AppState::new(database, None, auth);
        assert_eq!(accepted_inventory_revision(&restarted, &agent_id).await, 1);
        assert_eq!(
            accepted_inventory_fingerprint(&restarted, &agent_id)
                .await
                .as_deref(),
            Some(fingerprint.as_str())
        );
        assert_eq!(submit_v2(&restarted, &agent_id, body).await, first);
    }

    /// Issue #187: the revision belongs to the Agent identity. A Boot rotation
    /// (Closing then DrainedPrevious) and an Owner-authorized Agent Recovery
    /// (epoch advance) preserve it; only a genuine content change increments it.
    #[tokio::test]
    async fn v2_revision_survives_boot_rotation_and_agent_recovery() {
        let (dir, state, agent_id) = state_with_agent().await;
        let content_a = "ws://127.0.0.1:6790";
        let content_b = "ws://127.0.0.1:6791";
        let first = submit_v2(
            &state,
            &agent_id,
            v2_report_body(
                "0195f2a1-0260-4260-8260-000000000001",
                1,
                1,
                V2_BOOT_ID,
                &[content_a],
            ),
        )
        .await;
        assert_eq!(v2_acceptance(&first).revision, 1);

        let closing = v2_with_transition(
            v2_report_body(
                "0195f2a1-0260-4260-8260-000000000002",
                2,
                1,
                V2_BOOT_ID,
                &[content_a],
            ),
            platpulse_core::BootTransition::Closing,
            None,
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &agent_id, closing).await).revision,
            1
        );

        let rotated_boot = "0195f2a1-0262-4262-8262-000000000062";
        let drained = v2_with_transition(
            v2_report_body(
                "0195f2a1-0260-4260-8260-000000000003",
                1,
                1,
                rotated_boot,
                &[content_b],
            ),
            platpulse_core::BootTransition::DrainedPrevious,
            Some(V2_BOOT_ID),
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &agent_id, drained).await).revision,
            2
        );

        let unchanged_after_rotation = v2_report_body(
            "0195f2a1-0260-4260-8260-000000000004",
            2,
            1,
            rotated_boot,
            &[content_b],
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &agent_id, unchanged_after_rotation).await).revision,
            2
        );

        // Owner-authorized Agent Recovery runs through the real enrollment path:
        // it advances the Agent Epoch and rotates the credential without
        // touching the accepted Inventory Revision.
        let pepper = load_pepper_file(&dir.path().join("pepper")).unwrap();
        let token = create_recovery_token(
            state.db(),
            &pepper,
            None,
            &agent_id,
            RECOVERY_TOKEN_DEFAULT_LIFETIME,
        )
        .await
        .unwrap();
        let recovered_agent = recover(state.db(), &pepper, &token.token).await.unwrap();
        assert_eq!(recovered_agent.agent_id, agent_id);
        assert_eq!(recovered_agent.agent_epoch, 2);
        let recovered = v2_report_body(
            "0195f2a1-0260-4260-8260-000000000005",
            3,
            2,
            rotated_boot,
            &[content_b],
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &agent_id, recovered).await).revision,
            2
        );
        let changed = v2_report_body(
            "0195f2a1-0260-4260-8260-000000000006",
            4,
            2,
            rotated_boot,
            &[content_a],
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &agent_id, changed).await).revision,
            3
        );
        assert_eq!(accepted_inventory_revision(&state, &agent_id).await, 3);
    }

    /// Issue #187: the revision belongs to one Agent identity. A second Agent
    /// identity has its own independent sequence, so one Agent's declaration
    /// change never advances another's revision.
    #[tokio::test]
    async fn v2_distinct_agent_identities_keep_independent_revisions() {
        let (_dir, state, first_agent) = state_with_agent().await;
        let second_agent = "0195f2a1-0011-4011-8011-000000000022";
        sqlx::query(
            "INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES ($1, 1, NULL, NULL, NULL, $2, $3)",
        )
        .bind(second_agent)
        .bind("2026-08-12T08:00:00Z")
        .bind("2026-08-12T08:00:00Z")
        .execute(state.db().pool())
        .await
        .unwrap();

        let first_body = v2_report_body(
            "0195f2a1-0270-4270-8270-000000000001",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6790"],
        );
        let mut second_value: serde_json::Value = serde_json::from_slice(&v2_report_body(
            "0195f2a1-0270-4270-8270-000000000002",
            1,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6791"],
        ))
        .unwrap();
        second_value["agent_id"] = serde_json::json!(second_agent);
        let second_body = serde_json::to_vec(&second_value).unwrap();

        let first = submit_v2(&state, &first_agent, first_body).await;
        assert_eq!(v2_acceptance(&first).revision, 1);
        let second = submit_v2(&state, second_agent, second_body).await;
        assert_eq!(
            v2_acceptance(&second).revision,
            1,
            "a new Agent identity starts its own sequence"
        );

        // A content change for the first identity advances only its revision.
        let first_changed = v2_report_body(
            "0195f2a1-0270-4270-8270-000000000003",
            2,
            1,
            V2_BOOT_ID,
            &["ws://127.0.0.1:6799"],
        );
        assert_eq!(
            v2_acceptance(&submit_v2(&state, &first_agent, first_changed).await).revision,
            2
        );
    }

    // -----------------------------------------------------------------------
    // Issue #274 / ADR 0014: freshness, history and alert evidence are judged on
    // the instant an observation was taken, never on the instant its Report was
    // received.
    // -----------------------------------------------------------------------

    const ISSUE_274_NODE: &str = "0195f2a1-0014-4014-8014-000000000014";

    /// A Report taken `offset` from now on the Agent's own clock, whose clock
    /// reads `clock_skew_ms` ahead of Server time (a negative reading is a clock
    /// that runs behind).
    ///
    /// The Report carries a successful consensus observation because a Node's
    /// freshness is the oldest of its RPC, sync and consensus observation
    /// instants, and the minimal fixture reports consensus as `unsupported`.
    fn report_observed(offset: time::Duration, clock_skew_ms: i64) -> serde_json::Value {
        let mut raw: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        restamp_fixture(&mut raw, now_utc() + offset);
        let observed = raw["nodes"][0]["chain"]["sync"]["latest_observed_at"].clone();
        raw["host"]["clock_skew"]["latest"] = serde_json::json!(clock_skew_ms);
        raw["nodes"][0]["chain"]["consensus"] = serde_json::json!({
            "status": "ok",
            "attempted_at": observed.clone(),
            "latest_observed_at": observed,
            "state_revision": 1,
            "value_revision": 1,
            "latest": {
                "epoch": 100,
                "view_number": 500,
                "validator": false,
                "highest_qc_block": 9007199254740993u64,
                "highest_lock_block": 9007199254740992u64,
                "highest_commit_block": 9007199254740991u64
            }
        });
        raw
    }

    /// The instant a Report says its sync observation happened, as the Report
    /// carries it.
    fn report_instant(report: &serde_json::Value) -> String {
        report["nodes"][0]["chain"]["sync"]["latest_observed_at"]
            .as_str()
            .expect("the report stamps its sync observation")
            .to_owned()
    }

    /// The Server-clock instant stored for an instant an Agent clock
    /// `skew_ms` ahead of Server time reported: corrected by the measured skew and
    /// truncated to a second, the way every stored instant is.
    fn server_instant(observed: &str, skew_ms: i64) -> String {
        clock_text(
            crate::auth::parse_rfc3339(observed).expect("the report instant parses")
                - time::Duration::milliseconds(skew_ms),
        )
    }

    /// An instant exactly as the Server stores one.
    fn clock_text(at: time::OffsetDateTime) -> String {
        format_rfc3339(at.replace_nanosecond(0).unwrap())
    }

    fn seconds_from_now(instant: &str) -> i64 {
        let instant = crate::auth::parse_rfc3339(instant).expect("the stored instant parses");
        (instant - now_utc()).whole_seconds()
    }

    /// The stored observation instant of one Node component.
    async fn component_instant(state: &AppState, component: &str) -> Option<String> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT observation_at FROM component_status WHERE node_id=$1 AND component_key=$2",
        )
        .bind(ISSUE_274_NODE)
        .bind(component)
        .fetch_optional(state.db().pool())
        .await
        .unwrap();
        row.and_then(|(instant,)| instant)
    }

    /// The evaluation fact a Node rule extracts from the stored projections.
    async fn extracted(state: &AppState, rule_key: &str) -> crate::alerts::EvalInput {
        let mut connection = state.db().pool().acquire().await.unwrap();
        crate::alerts::extract_input(
            &mut connection,
            rule_key,
            crate::alerts::SubjectKind::Node,
            ISSUE_274_NODE,
            now_utc(),
            120,
        )
        .await
        .unwrap()
    }

    /// One Peer of a Peer Snapshot, as the Agent reports it.
    fn peer_current(peer_id: &str, remote_ip: &str, direction: &str) -> serde_json::Value {
        serde_json::json!({
            "peer_id": peer_id,
            "remote_ip": remote_ip,
            "direction": direction,
            "trusted": false,
            "static_peer": false,
            "consensus_peer": false,
            "caps": [],
        })
    }

    /// One Peer Snapshot of a spool being replayed: observed at `observed` on an
    /// Agent clock that agrees with the Server, delivered now.
    fn replayed_peer_report(
        report: (&str, i64),
        revision: i64,
        observed: &str,
        peers: serde_json::Value,
    ) -> Vec<u8> {
        replayed_peer_report_with_skew(report, revision, observed, peers, 0)
    }

    /// The same replayed Peer Snapshot, on an Agent clock whose measured skew is
    /// `clock_skew_ms` — at or beyond Clock Unreliable when the caller means it.
    fn replayed_peer_report_with_skew(
        report: (&str, i64),
        revision: i64,
        observed: &str,
        peers: serde_json::Value,
        clock_skew_ms: i64,
    ) -> Vec<u8> {
        let (report_id, report_sequence) = report;
        let mut value = peer_snapshot_body(peers);
        value["report_id"] = serde_json::json!(report_id);
        value["report_sequence"] = serde_json::json!(report_sequence);
        value["host"]["clock_skew"]["latest"] = serde_json::json!(clock_skew_ms);
        let snapshot = &mut value["nodes"][0]["chain"]["peers"];
        snapshot["attempted_at"] = serde_json::json!(observed);
        snapshot["latest_observed_at"] = serde_json::json!(observed);
        snapshot["state_revision"] = serde_json::json!(revision);
        snapshot["value_revision"] = serde_json::json!(revision);
        serde_json::to_vec(&value).unwrap()
    }

    /// Acceptance of issue #274: a batch of Reports generated two hours ago keeps
    /// the Node Stale from the first ingested Report on — freshness, health, the
    /// Reference Head and the Node alerts all judge the observation instant, never
    /// the receipt instant.
    #[tokio::test]
    async fn a_replayed_backlog_keeps_the_node_stale_and_off_the_reference_head() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_observed(time::Duration::hours(-2), 0);
        let observed = report_instant(&report);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&report).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        let stored = component_instant(&state, "sync").await.unwrap();
        assert_eq!(stored, server_instant(&observed, 0));
        assert!(
            seconds_from_now(&stored) < -7_000,
            "the Node was observed two hours ago: {stored}"
        );

        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(node["health"], "unknown", "{node}");
        assert_eq!(
            node["healthReason"],
            "one or more observations are stale or unknown"
        );
        let freshness = node["freshness"]
            .as_str()
            .expect("every observation instant is known");
        assert!(
            seconds_from_now(freshness) < -7_000,
            "freshness is the observation instant: {freshness}"
        );
        assert!(
            node["networkReferenceHead"].is_null(),
            "a replayed Node is never a source: {node}"
        );
        assert_eq!(node["networkReferenceConfidence"], "unknown");
        let reference: (Option<i64>, String) = sqlx::query_as(
            "SELECT block_number, confidence FROM network_reference_heads WHERE network_key='platon-mainnet'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(reference, (None, "unknown".to_owned()));

        for rule in [
            "node.rpc_unreachable",
            "node.sync_lag",
            "node.block_stalled",
        ] {
            let input = extracted(&state, rule).await;
            assert!(
                matches!(input, crate::alerts::EvalInput::Stale { .. }),
                "{rule} judges the observation instant: {input:?}"
            );
        }
    }

    /// Acceptance of issue #274: a measured skew below the Clock Unreliable
    /// threshold moves the observation onto the Server clock, so a three-minute
    /// skew is corrected by rather than read as the Agent stamped it.
    #[tokio::test]
    async fn a_measured_three_minute_skew_corrects_the_freshness_window() {
        let (_dir, state, agent_id) = state_with_agent().await;
        // The observation happened four minutes ago; the Agent clock runs three
        // minutes ahead, so the Report itself stamps it one minute ago. Reading the
        // stamped instant would call a four-minute-old observation current.
        let report = report_observed(time::Duration::minutes(-1), 180_000);
        let observed = report_instant(&report);
        assert!(
            seconds_from_now(&observed) > -120,
            "the stamped instant is inside the freshness window: {observed}"
        );
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&report).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        let stored = component_instant(&state, "sync").await.unwrap();
        assert_eq!(stored, server_instant(&observed, 180_000));
        // The correction is judged against the instant the Report itself
        // stamped, so a full-suite run slow enough to blur wall-clock offsets
        // still fails only on a real regression.
        let judged_at = now_utc();
        let age = |instant: &str| {
            (crate::auth::parse_rfc3339(instant).expect("the stored instant parses") - judged_at)
                .whole_seconds()
        };
        assert_eq!(
            age(&observed) - age(&stored),
            180,
            "the three-minute skew is corrected on the Server clock: {stored}"
        );

        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(
            node["freshness"].as_str(),
            Some(stored.as_str()),
            "freshness is the corrected observation instant"
        );
        assert_eq!(node["health"], "unknown", "{node}");
        assert_eq!(
            node["healthReason"],
            "one or more observations are stale or unknown"
        );
    }

    /// Acceptance of issue #274: a Node is a source for the Observed Network Head
    /// only while every component's observation instant is fresh. A Node whose RPC
    /// observation is two hours old is Stale to the Public projection, so a fresh
    /// sync observation must not make it hold the head.
    #[tokio::test]
    async fn a_mixed_age_node_is_not_a_reference_head_source() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_observed(time::Duration::ZERO, 0);
        // Sync and consensus were observed just now; RPC still holds the value of
        // an observation taken two hours ago.
        let old_rpc_at = clock_text(now_utc() - time::Duration::hours(2));
        report["nodes"][0]["chain"]["rpc"]["attempted_at"] = serde_json::json!(old_rpc_at);
        report["nodes"][0]["chain"]["rpc"]["latest_observed_at"] = serde_json::json!(old_rpc_at);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&report).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        assert_eq!(
            component_instant(&state, "rpc").await.as_deref(),
            Some(old_rpc_at.as_str())
        );
        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(
            node["freshness"].as_str(),
            Some(old_rpc_at.as_str()),
            "freshness is the oldest of the three observation instants: {node}"
        );
        assert_eq!(node["health"], "unknown", "{node}");
        assert!(
            node["networkReferenceHead"].is_null(),
            "a partially stale Node is never a source: {node}"
        );
        assert_eq!(node["networkReferenceConfidence"], "unknown");
        let reference: (Option<i64>, String) = sqlx::query_as(
            "SELECT block_number, confidence FROM network_reference_heads WHERE network_key='platon-mainnet'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(reference, (None, "unknown".to_owned()));
    }

    /// Acceptance of issue #274: the freshness window is symmetric, so a Node
    /// whose observation instants lie in the future is not fresh either and must
    /// not hold the Observed Network Head.
    #[tokio::test]
    async fn a_future_stamped_node_is_not_a_reference_head_source() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_observed(time::Duration::hours(2), 0);
        let observed = report_instant(&report);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&report).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        let stored = component_instant(&state, "sync").await.unwrap();
        assert_eq!(stored, server_instant(&observed, 0));
        assert!(
            seconds_from_now(&stored) > 7_000,
            "the Node claims to be observed two hours from now: {stored}"
        );
        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(node["health"], "unknown", "{node}");
        assert!(
            node["networkReferenceHead"].is_null(),
            "an instant in the future is not a fresh observation: {node}"
        );
        assert_eq!(node["networkReferenceConfidence"], "unknown");
        let reference: (Option<i64>, String) = sqlx::query_as(
            "SELECT block_number, confidence FROM network_reference_heads WHERE network_key='platon-mainnet'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(reference, (None, "unknown".to_owned()));
    }

    /// Acceptance of issue #274: a Clock Unreliable reading is not corrected by.
    /// The Agent's clock is unusable, so the receipt instant is the only instant
    /// the Server can defend, exactly as before ADR 0014.
    #[tokio::test]
    async fn an_unreliable_clock_falls_back_to_the_receipt_instant() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_observed(time::Duration::hours(-2), 600_000);
        let observed = report_instant(&report);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&report).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        let stored = component_instant(&state, "sync").await.unwrap();
        assert_ne!(
            stored,
            server_instant(&observed, 600_000),
            "an untrustworthy skew is not corrected by"
        );
        assert!(
            seconds_from_now(&stored).abs() <= 60,
            "the receipt instant stands in: {stored}"
        );
        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(node["health"], "healthy", "{node}");
    }

    /// Acceptance of issue #274: the Peer presence interval a replayed snapshot
    /// opens is stamped with the instant its sample was observed, so a two-hour
    /// catch-up writes two hours of history where it belongs instead of squeezing
    /// it into the catch-up window.
    #[tokio::test]
    async fn a_replayed_peer_snapshot_stamps_history_with_the_observation_instant() {
        let (_dir, state, agent_id) = state_with_agent().await;
        // One spool, filled two hours and one hour ago, delivered in one catch-up
        // now: the baseline the Server holds no predecessor for, then an arrival.
        let baseline_at = clock_text(now_utc() - time::Duration::hours(2));
        let arrival_at = clock_text(now_utc() - time::Duration::hours(1));
        let baseline = replayed_peer_report(
            ("0195f2a1-0274-4274-8274-000000000001", 1),
            1,
            &baseline_at,
            serde_json::json!([peer_current("peer-a", "8.8.8.8", "inbound")]),
        );
        let arrival = replayed_peer_report(
            ("0195f2a1-0274-4274-8274-000000000002", 2),
            2,
            &arrival_at,
            serde_json::json!([
                peer_current("peer-a", "8.8.8.8", "inbound"),
                peer_current("peer-b", "192.0.2.10", "outbound"),
            ]),
        );
        for body in [baseline, arrival] {
            assert_eq!(
                submit(&state, &agent_id, body).await.disposition,
                ReceiptDisposition::Accepted
            );
        }

        let intervals: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT peer_id, opened_at, closed_at FROM peer_presence_intervals WHERE node_id=$1 ORDER BY peer_id",
        )
        .bind(ISSUE_274_NODE)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            intervals,
            vec![("peer-b".to_owned(), arrival_at.clone(), None)],
            "the arrival is stamped when it was observed, not when it was delivered"
        );

        let buckets: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT bucket_start, first_observed_at, last_observed_at FROM peer_aggregate_5m WHERE node_id=$1 ORDER BY bucket_start",
        )
        .bind(ISSUE_274_NODE)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            buckets
                .iter()
                .map(|(_, first, _)| first.clone())
                .collect::<Vec<_>>(),
            vec![baseline_at, arrival_at],
            "each replayed snapshot fills the bucket of its own instant: {buckets:?}"
        );
        for (bucket_start, first, last) in &buckets {
            assert_eq!(first, last, "one replayed snapshot is one sample");
            assert!(
                bucket_start.as_str() <= first.as_str(),
                "the bucket {bucket_start} cannot start after its sample {first}"
            );
        }

        // A later snapshot, observed half an hour ago, no longer carries peer-b:
        // the interval closes at the instant the departure was observed, not at
        // the instant its Report arrived.
        let departure_at = clock_text(now_utc() - time::Duration::minutes(30));
        let departure = replayed_peer_report(
            ("0195f2a1-0274-4274-8274-000000000003", 3),
            3,
            &departure_at,
            serde_json::json!([peer_current("peer-a", "8.8.8.8", "inbound")]),
        );
        assert_eq!(
            submit(&state, &agent_id, departure).await.disposition,
            ReceiptDisposition::Accepted
        );
        let closed: Option<String> = sqlx::query_scalar(
            "SELECT closed_at FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-b'",
        )
        .bind(ISSUE_274_NODE)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            closed.as_deref(),
            Some(departure_at.as_str()),
            "a departure closes the interval at the instant it was observed"
        );
    }

    /// Acceptance of issue #274: a replayed Peer Snapshot is stamped with its own
    /// observation instant even when the Agent is Clock Unreliable. Freshness may
    /// fall back to the receipt instant because it answers "is this Node current
    /// right now", but history must not: the presence interval and the aggregate
    /// bucket keep the instant the snapshot names, or a drained backlog piles
    /// hours of history onto the minutes of catch-up.
    #[tokio::test]
    async fn a_replayed_peer_snapshot_keeps_its_instant_when_the_clock_is_unreliable() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let baseline_at = clock_text(now_utc() - time::Duration::hours(2));
        let arrival_at = clock_text(now_utc() - time::Duration::hours(1));
        let skew_ms = crate::http::agent::CLOCK_UNRELIABLE_THRESHOLD_MS + 1;
        let mut bodies = Vec::new();
        for (report, revision, observed, peers) in [
            (
                ("0195f2a1-0274-4274-8274-000000000011", 1),
                1,
                baseline_at.as_str(),
                serde_json::json!([peer_current("peer-a", "8.8.8.8", "inbound")]),
            ),
            (
                ("0195f2a1-0274-4274-8274-000000000012", 2),
                2,
                arrival_at.as_str(),
                serde_json::json!([
                    peer_current("peer-a", "8.8.8.8", "inbound"),
                    peer_current("peer-b", "192.0.2.10", "outbound"),
                ]),
            ),
        ] {
            bodies.push(replayed_peer_report_with_skew(
                report, revision, observed, peers, skew_ms,
            ));
        }
        for body in bodies {
            assert_eq!(
                submit(&state, &agent_id, body).await.disposition,
                ReceiptDisposition::Accepted
            );
        }

        let intervals: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT peer_id, opened_at, closed_at FROM peer_presence_intervals WHERE node_id=$1 ORDER BY peer_id",
        )
        .bind(ISSUE_274_NODE)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            intervals,
            vec![("peer-b".to_owned(), arrival_at.clone(), None)],
            "an unusable skew does not move a presence interval onto the receipt instant"
        );

        let buckets: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT bucket_start, first_observed_at, last_observed_at FROM peer_aggregate_5m WHERE node_id=$1 ORDER BY bucket_start",
        )
        .bind(ISSUE_274_NODE)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            buckets
                .iter()
                .map(|(_, first, _)| first.clone())
                .collect::<Vec<_>>(),
            vec![baseline_at, arrival_at],
            "each replayed snapshot fills the bucket of its own instant: {buckets:?}"
        );
        for (bucket_start, first, last) in &buckets {
            assert_eq!(first, last, "one replayed snapshot is one sample");
            assert!(
                bucket_start.as_str() <= first.as_str(),
                "the bucket {bucket_start} cannot start after its sample {first}"
            );
        }

        // The fresh face of the same Report does fall back: a Clock Unreliable
        // Agent cannot say whether its Node is current, so freshness keeps the
        // receipt instant while history keeps the observation instant.
        // The window is loose on purpose: it only has to separate "the receipt
        // instant, a second or so ago" from "the observation instant, an hour
        // ago", and a tight bound would fail this test after a long pause.
        let component = component_instant(&state, "peers").await.unwrap();
        assert!(
            seconds_from_now(&component).abs() <= 900,
            "freshness falls back to the receipt instant: {component}"
        );
    }
}
