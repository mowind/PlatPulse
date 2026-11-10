//! Receipt and rejection responses of Report Ingestion: the receipt envelopes,
//! the receipt store with the exact replay it answers, and the error responses.

use crate::http::realtime::AdminResource;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sqlx::{Any, FromRow, Transaction};

use crate::http::AppState;
use platpulse_core::protocol::SUPPORTED_PROTOCOL_MAJORS;
use platpulse_core::{
    AgentReport, InventoryDisposition, InventoryReceiptV2, ReceiptDisposition, ReportId,
    ReportInventory, ReportReceipt, ReportReceiptV2, Sha256Hex,
};

use super::*;

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
pub(super) enum IngestReceipt {
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

    pub(super) fn to_bytes(&self) -> Vec<u8> {
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
pub(super) fn replay_receipt<I: ReportInventory>(body: &[u8]) -> Result<IngestReceipt, ()> {
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
pub(super) struct ReceiptRow {
    pub(super) report_body_sha256: String,
    pub(super) receipt_body: Vec<u8>,
}

pub(super) fn rejection(
    code: platpulse_core::RejectionCode,
    reason: &str,
) -> platpulse_core::Rejection {
    platpulse_core::Rejection {
        code,
        retryable: code.is_retryable(),
        reason: crate::redaction::redact_sensitive(reason),
    }
}

pub(super) fn disposition_name(disposition: ReceiptDisposition) -> &'static str {
    match disposition {
        ReceiptDisposition::Accepted => "accepted",
        ReceiptDisposition::PartiallyAccepted => "partially_accepted",
        ReceiptDisposition::Rejected => "rejected",
    }
}

pub(super) fn rejected(
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

pub(super) fn error(
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
pub(super) fn storage_error(request_id: &str, operation: &str, failure: &sqlx::Error) -> Response {
    tracing::error!(error = %failure, "report ingestion storage error during {operation}");
    if crate::database::is_constraint_violation(failure) {
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

pub(super) fn receipt_response(receipt: IngestReceipt) -> Response {
    receipt.into_response()
}

/// A failed statement of the report transaction: log it under the request,
/// latch runtime corruption when the Storage Backend classifies it so, and
/// answer with the storage error the operator can act on.
pub(super) fn storage_failure(
    state: &AppState,
    request_id: &str,
    operation: &str,
    failure: &sqlx::Error,
) -> Response {
    state.note_database_error(failure);
    storage_error(request_id, operation, failure)
}

/// The answer to a Report identity that already holds a Receipt.
pub(super) enum Replay {
    /// The stored Receipt, re-materialized under the request's protocol major.
    Receipt(IngestReceipt),
    /// The stored Report has different bytes under the same identity.
    IdentityConflict,
    /// Replay never bypasses the live Agent boundary: a removed Agent's
    /// retained Receipt is not returned even when the immutable bytes match
    /// (design 15.10.5, issue #192).
    AgentRemoved,
    /// The stored body does not decode under the request's protocol major.
    Unreadable,
}

impl Replay {
    pub(super) fn into_response(self, request_id: &str) -> Response {
        match self {
            Self::Receipt(receipt) => receipt_response(receipt),
            Self::IdentityConflict => error(
                request_id,
                StatusCode::CONFLICT,
                "report_identity_conflict",
                "Report identity conflicts with a stored report",
            ),
            Self::AgentRemoved => error(
                request_id,
                StatusCode::UNAUTHORIZED,
                "agent_auth_required",
                "Agent credential is invalid",
            ),
            Self::Unreadable => error(
                request_id,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Stored receipt is unavailable",
            ),
        }
    }
}

/// The exact replay of a stored Report identity, or `None` when the identity
/// holds no Receipt yet.
pub(super) async fn find_replay<I: ReportInventory>(
    tx: &mut Transaction<'_, Any>,
    agent_id: &str,
    report_id: ReportId,
    hash: &Sha256Hex,
) -> Result<Option<Replay>, sqlx::Error> {
    let stored = sqlx::query_as::<_, ReceiptRow>(
        "SELECT report_body_sha256, receipt_body FROM agent_report_receipts WHERE report_id = $1",
    )
    .bind(report_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    if stored.report_body_sha256 != hash.to_string() {
        return Ok(Some(Replay::IdentityConflict));
    }
    let live = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM agents WHERE agent_id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(&mut **tx)
    .await?;
    if live.is_none() {
        return Ok(Some(Replay::AgentRemoved));
    }
    Ok(Some(
        replay_receipt::<I>(&stored.receipt_body).map_or(Replay::Unreadable, Replay::Receipt),
    ))
}

/// Durable Inventory evidence of a whole-report rejection (issue #181): which
/// code refused the Report and what Inventory the Agent declared, so Admin
/// never has to decode the receipt body, nor trust an Agent-reported
/// diagnostic, to answer "is this Agent's latest ingestion attempt an
/// Inventory rejection?".
pub(super) struct RejectionEvidence {
    code: Option<&'static str>,
    inventory_revision: Option<i64>,
    inventory_sha256: String,
    inventory_protocol_major: i64,
}

impl RejectionEvidence {
    /// A whole-report rejection decides by its first rejection.
    pub(super) fn of<I: ReportInventory>(report: &AgentReport<I>, receipt: &IngestReceipt) -> Self {
        Self {
            code: receipt
                .rejections()
                .first()
                .map(|rejection| rejection.code.as_str()),
            inventory_revision: report
                .inventory
                .declared_revision()
                .map(|revision| revision as i64),
            inventory_sha256: report.inventory.accepted_sha256().to_string(),
            // The Server-owned protocol major of the refused declaration. v2
            // declares no revision, so the Admin diagnosis explains it from the
            // accepted revision/fingerprint and this evidence instead of
            // forging a removed Agent-supplied revision (issue #188).
            inventory_protocol_major: I::protocol_version() as i64,
        }
    }
}

/// What storing one Report's Receipt decided.
pub(super) enum StoreOutcome {
    /// This attempt's Receipt is now the Report's Receipt.
    Stored,
    /// The Report identity already holds a Receipt, which answers this attempt.
    Replay(Replay),
    /// Another Report already holds this Boot's sequence.
    SequenceTaken,
}

/// Store the Receipt of one Report, accepted or rejected.
///
/// The same immutable Report can be delivered twice at once (a sender deadline
/// that expired while the first attempt was still committing, or a reconnect
/// replay), and PostgreSQL admits two concurrent Reports of one Boot into this
/// insert, so both unique keys of the table — `report_id` and
/// `(agent_id, agent_epoch, boot_id, report_sequence)` — are the barriers an
/// earlier read cannot be. A unique violation would abort the PostgreSQL
/// transaction; `ON CONFLICT DO NOTHING` reports the loser as zero rows and
/// leaves it usable to tell a replay from a taken sequence.
pub(super) async fn store_receipt<I: ReportInventory>(
    tx: &mut Transaction<'_, Any>,
    report: &AgentReport<I>,
    hash: &Sha256Hex,
    receipt: &IngestReceipt,
    received_at: &str,
    evidence: Option<RejectionEvidence>,
) -> Result<StoreOutcome, sqlx::Error> {
    let (code, inventory_revision, inventory_sha256, inventory_protocol_major) = match evidence {
        Some(evidence) => (
            evidence.code,
            evidence.inventory_revision,
            Some(evidence.inventory_sha256),
            Some(evidence.inventory_protocol_major),
        ),
        None => (None, None, None, None),
    };
    let agent_id = report.agent_id.to_string();
    let inserted = sqlx::query("INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at, rejection_code, inventory_revision, inventory_sha256, inventory_protocol_major) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) ON CONFLICT DO NOTHING")
        .bind(report.report_id.to_string())
        .bind(&agent_id)
        .bind(report.agent_epoch as i64)
        .bind(report.boot_id.to_string())
        .bind(report.report_sequence as i64)
        .bind(hash.to_string())
        .bind(disposition_name(receipt.disposition()))
        .bind(receipt.to_bytes())
        .bind(received_at)
        .bind(code)
        .bind(inventory_revision)
        .bind(inventory_sha256)
        .bind(inventory_protocol_major)
        .execute(&mut **tx)
        .await?;
    if inserted.rows_affected() == 1 {
        return Ok(StoreOutcome::Stored);
    }
    Ok(
        match find_replay::<I>(tx, &agent_id, report.report_id, hash).await? {
            Some(replay) => StoreOutcome::Replay(replay),
            None => StoreOutcome::SequenceTaken,
        },
    )
}

/// The one answer to a Report whose sequence another Report of its Boot
/// already holds: a competing body is never silently accepted. The Report's
/// own writes are discarded and the security event commits on a fresh
/// transaction; a failure to record it is a storage failure, not a quiet 409.
pub(super) async fn refuse_sequence_taken(
    state: &AppState,
    tx: Transaction<'_, Any>,
    agent_id: &str,
    request_id: &str,
) -> Response {
    if let Err(failure) = tx.rollback().await {
        return storage_failure(state, request_id, "roll back conflicting report", &failure);
    }
    let mut event_tx = match crate::database::begin(state.db().pool()).await {
        Ok(tx) => tx,
        Err(failure) => {
            return storage_failure(state, request_id, "begin security event", &failure);
        }
    };
    if let Err(failure) = record_security_event(&mut event_tx, agent_id).await {
        return storage_failure(state, request_id, "record security event", &failure);
    }
    if let Err(failure) = event_tx.commit().await {
        return storage_failure(state, request_id, "commit security event", &failure);
    }
    error(
        request_id,
        StatusCode::CONFLICT,
        "conflicting_boot",
        "Boot sequence conflicts with a stored report",
    )
}

pub(super) async fn store_rejected<I: ReportInventory>(
    state: &AppState,
    mut tx: Transaction<'_, Any>,
    report: &AgentReport<I>,
    hash: Sha256Hex,
    receipt: ReportReceipt,
    request_id: &str,
) -> Response {
    let receipt = into_ingest_receipt::<I>(receipt);
    let evidence = RejectionEvidence::of(report, &receipt);
    let received_at = now().to_string();
    match store_receipt(
        &mut tx,
        report,
        &hash,
        &receipt,
        &received_at,
        Some(evidence),
    )
    .await
    {
        Ok(StoreOutcome::Stored) => {}
        Ok(StoreOutcome::Replay(replay)) => return replay.into_response(request_id),
        Ok(StoreOutcome::SequenceTaken) => {
            return refuse_sequence_taken(state, tx, &report.agent_id.to_string(), request_id)
                .await;
        }
        Err(failure) => {
            return storage_failure(state, request_id, "store rejection receipt", &failure);
        }
    }
    if let Err(failure) = tx.commit().await {
        return storage_failure(state, request_id, "commit rejection receipt", &failure);
    }
    // A rejection writes no projection row, but it does change derived Admin
    // state: the Agent's Inventory rejection Attention Item. Without this
    // signal an already-open Agent page would keep showing the frozen Agent
    // with no explanation. The realtime layer coalesces same-resource events,
    // so a rejection every collection interval is not an event flood.
    state
        .admin_realtime()
        .publish(AdminResource::Agent, Some(report.agent_id.to_string()));
    receipt_response(receipt)
}

pub(super) async fn record_security_event(
    tx: &mut Transaction<'_, Any>,
    agent_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agents SET security_event_count=security_event_count+1 WHERE agent_id=$1")
        .bind(agent_id)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}
