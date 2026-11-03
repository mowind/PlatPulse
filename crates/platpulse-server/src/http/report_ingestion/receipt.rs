//! Receipt and rejection responses of Report Ingestion: the receipt envelopes,
//! the stored receipt an exact replay returns, and the error responses.

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

/// Stable rejection for a Node ID that carries the durable purge boundary
/// (issue #170). Terminal: the Agent must not expect a retry to succeed.
pub(super) fn purged_rejection() -> platpulse_core::Rejection {
    rejection(
        platpulse_core::RejectionCode::NodePurged,
        "Node was permanently purged by the Owner",
    )
}

/// Stable rejection for a Node entry the reporting Agent does not own.
pub(super) fn ownership_mismatch_rejection() -> platpulse_core::Rejection {
    rejection(
        platpulse_core::RejectionCode::NodeOwnershipMismatch,
        "Node belongs to another Agent",
    )
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

pub(super) fn receipt_response(receipt: IngestReceipt) -> Response {
    receipt.into_response()
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
