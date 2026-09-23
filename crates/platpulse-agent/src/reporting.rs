use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};
use sqlx::{Connection, Sqlite, Transaction};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use thiserror::Error;

use platpulse_core::hex::Sha256Hex;
use platpulse_core::identity::ReportId;
use platpulse_core::{
    AgentReport, BootTransition, InventoryDeclaration, ReceiptDisposition, ReportReceipt,
    ReportReceiptV2, Rfc3339,
};

use serde::Deserialize;

use crate::collector::{ApplyReceiptError, SpoolCleanupSummary, SpoolPolicy, apply_receipt_typed};
use crate::config::{AgentConfig, AgentConfigError};
use crate::credential::{CredentialError, load_credential_file};
use crate::database::{
    AgentDatabaseConfig, AgentDatabaseError, AgentStore, cleanup_expired_receipt_markers,
    now_rfc3339,
};

#[derive(Debug, sqlx::FromRow)]
struct SpoolReportRow {
    report_id: String,
    agent_epoch: i64,
    boot_id: String,
    report_sequence: i64,
    generated_at: String,
    body: Vec<u8>,
    body_sha256: String,
    body_bytes: i64,
}

#[derive(Debug, Error)]
pub enum ReportStoreError {
    #[error("report body is empty")]
    Empty,
    #[error("minimum complete current report exceeds protocol hard limit")]
    ReportTooLarge,
    #[error("Agent Store is in fatal state: {0}")]
    StoreFatal(String),
    #[error("report is already being delivered")]
    DeliveryInFlight,
    #[error("receipt body is invalid: {0}")]
    InvalidReceipt(String),
    #[error("receipt does not match the stored report")]
    ReceiptMismatch,
    #[error("delivery transport failed: {0}")]
    Delivery(String),
    #[error("delivery sender deadline exhausted")]
    DeliveryDeadline,
    #[error("failed to read report body {path}: {source}")]
    ReadReport {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("report JSON is invalid: {0}")]
    InvalidReport(String),
    #[error("report inventory does not match the validated Agent configuration")]
    InventoryMismatch,
    #[error("{0}")]
    InventoryDeclaration(#[from] crate::inventory_declaration::InventoryDeclarationConflict),
    #[error("Agent configuration is invalid: {0}")]
    Config(#[from] AgentConfigError),
    #[error("Agent Store initialization failed: {0}")]
    Store(#[from] AgentDatabaseError),
    #[error("Agent runtime ownership failed: {0}")]
    RuntimeOwnership(String),
    #[error("credential load failed: {0}")]
    Credential(#[from] CredentialError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("stale Closing report {report_id} does not belong to the current Agent state")]
    StaleClosing { report_id: String },
}

impl From<crate::inventory_declaration::InventoryGuardError> for ReportStoreError {
    fn from(error: crate::inventory_declaration::InventoryGuardError) -> Self {
        use crate::inventory_declaration::InventoryGuardError;
        match error {
            InventoryGuardError::Conflict(conflict) => Self::InventoryDeclaration(conflict),
            InventoryGuardError::Database(error) => Self::Database(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReport {
    pub report_id: String,
    pub report_sequence: u64,
    pub body: Vec<u8>,
    pub body_sha256: String,
}

/// HTTP delivery transport. It posts the exact immutable body and returns
/// response bytes without parsing or rewriting them.
pub struct HttpReportTransport {
    client: reqwest::Client,
    url: String,
    url_v2: String,
    credential: String,
}

impl HttpReportTransport {
    pub fn from_config(config: &AgentConfig) -> Result<Self, ReportStoreError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .user_agent(format!("platpulse-agent/{}", crate::VERSION))
                .build()
                .map_err(|error| ReportStoreError::Delivery(error.to_string()))?,
            url: format!(
                "{}{}",
                config.server_url,
                platpulse_core::protocol::AGENT_API_REPORTS_PATH
            ),
            url_v2: format!(
                "{}{}",
                config.server_url,
                platpulse_core::protocol::AGENT_API_REPORTS_PATH_V2
            ),
            credential: load_credential_file(&config.credential_file)?,
        })
    }
}

impl ReportTransport for HttpReportTransport {
    fn send<'a>(
        &'a self,
        body: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ReportStoreError>> + Send + 'a>> {
        Box::pin(async move {
            // The protocol major selects the route group; the immutable report
            // bytes are never rewritten to fit a version.
            let url =
                if report_protocol_major(body) == platpulse_core::protocol::PROTOCOL_VERSION_V2 {
                    &self.url_v2
                } else {
                    &self.url
                };
            let response = self
                .client
                .post(url)
                .bearer_auth(&self.credential)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_vec())
                .send()
                .await
                .map_err(|error| ReportStoreError::Delivery(error.to_string()))?;
            let status = response.status();
            let bytes = response
                .bytes()
                .await
                .map_err(|error| ReportStoreError::Delivery(error.to_string()))?;
            if !status.is_success() {
                return Err(ReportStoreError::Delivery(format!(
                    "server returned HTTP {}",
                    status.as_u16()
                )));
            }
            Ok(bytes.to_vec())
        })
    }
}

/// A transport which sends the exact bytes supplied by the durable spool.
pub trait ReportTransport {
    fn send<'a>(
        &'a self,
        body: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ReportStoreError>> + Send + 'a>>;
}

/// Claim exactly one report. An existing in-flight report is resumed first;
/// otherwise the oldest unclaimed report is atomically marked in-flight.
pub async fn claim_oldest_report(
    store: &mut AgentStore,
) -> Result<Option<StoredReport>, ReportStoreError> {
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    let transaction_result: Result<Option<StoredReport>, ReportStoreError> = async {
        let in_flight: Option<String> = sqlx::query_scalar(
            "SELECT report_id FROM reports WHERE in_flight = 1 ORDER BY created_at, report_id LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let report_id = if let Some(id) = in_flight {
            id
        } else {
            let Some(id) = sqlx::query_scalar::<_, String>("SELECT report_id FROM reports WHERE in_flight = 0 ORDER BY created_at, report_id LIMIT 1")
                .fetch_optional(&mut *tx).await? else {
                return Ok(None);
            };
            sqlx::query("UPDATE reports SET in_flight = 1 WHERE report_id = ? AND in_flight = 0")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            id
        };
        let report = sqlx::query_as::<_, (String, i64, Vec<u8>, String)>("SELECT report_id, report_sequence, body, body_sha256 FROM reports WHERE report_id = ? AND in_flight = 1")
            .bind(&report_id).fetch_optional(&mut *tx).await?;
        report
            .map(
                |(report_id, report_sequence, body, body_sha256)| StoredReport {
                    report_id,
                    report_sequence: report_sequence as u64,
                    body,
                    body_sha256,
                },
            )
            .ok_or(ReportStoreError::DeliveryInFlight)
            .map(Some)
    }
    .await;
    match transaction_result {
        Ok(report) => {
            tx.commit().await?;
            Ok(report)
        }
        Err(error) => match tx.rollback().await {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(ReportStoreError::Database(rollback_error)),
        },
    }
}

/// The operator-facing summary of a whole-report rejection: every rejection the
/// Server returned, in one bounded line. Shared by the durable Agent diagnostic
/// (which the Agent Store keeps) and the delivery loop's console print, so both
/// surfaces always show the same codes and reasons (issue #181).
pub(crate) fn rejection_summary(receipt: &ReportReceipt) -> String {
    receipt
        .rejections
        .iter()
        .map(|rejection| format!("{} ({})", rejection.code.as_str(), rejection.reason))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Record a bounded delivery failure inside an existing transaction, so a
/// receipt rejection can share the same diagnostic write as a transport
/// failure without leaving the caller's transaction.
pub(crate) async fn record_delivery_failure_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    message: &str,
    at: &str,
) -> Result<(), sqlx::Error> {
    let message: String = message.chars().take(256).collect();
    sqlx::query("INSERT INTO delivery_diagnostics (singleton, last_error, last_error_at) VALUES (1, ?, ?) ON CONFLICT(singleton) DO UPDATE SET last_error=excluded.last_error, last_error_at=excluded.last_error_at")
        .bind(message).bind(at).execute(&mut **tx).await?;
    Ok(())
}

/// Record a bounded delivery failure while preserving the immutable report.
pub async fn record_delivery_failure(
    store: &mut AgentStore,
    message: &str,
    at: &str,
) -> Result<(), ReportStoreError> {
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    match record_delivery_failure_in_transaction(&mut tx, message, at).await {
        Ok(()) => tx.commit().await.map_err(ReportStoreError::from),
        Err(error) => match tx.rollback().await {
            Ok(()) => Err(ReportStoreError::from(error)),
            Err(rollback_error) => Err(ReportStoreError::from(rollback_error)),
        },
    }
}

/// Remove an orphan report that can never apply to the current Agent state.
/// The Server has already acknowledged its exact bytes, so the local row is
/// dropped; `report_sample_assignments` cascades and its samples become
/// re-assignable.
pub(crate) async fn quarantine_report(
    store: &mut AgentStore,
    report_id: &str,
) -> Result<bool, ReportStoreError> {
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    let result = sqlx::query("DELETE FROM reports WHERE report_id = ?")
        .bind(report_id)
        .execute(&mut *tx)
        .await;
    match result {
        Ok(result) => {
            tx.commit().await?;
            Ok(result.rows_affected() == 1)
        }
        Err(error) => {
            tx.rollback().await?;
            Err(ReportStoreError::Database(error))
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReportResponse {
    receipt: ReportReceipt,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReportResponseV2 {
    receipt: ReportReceiptV2,
}

/// The protocol major declared in a report body, read without fully decoding it.
pub(crate) fn report_protocol_major(body: &[u8]) -> u64 {
    #[derive(Deserialize)]
    struct ProtocolMajor {
        protocol_version: u64,
    }
    serde_json::from_slice::<ProtocolMajor>(body)
        .map(|major| major.protocol_version)
        .unwrap_or(0)
}

/// The common v2 receipt fields in the frozen v1 shape. The v2 Inventory
/// acceptance is carried separately so the confirmation record can bind the
/// Server-assigned revision and canonical fingerprint.
fn normalize_v2_receipt(receipt: ReportReceiptV2) -> ReportReceipt {
    ReportReceipt {
        report_id: receipt.report_id,
        disposition: receipt.disposition,
        report_body_sha256: receipt.report_body_sha256,
        server_version: receipt.server_version,
        supported_protocol_majors: receipt.supported_protocol_majors,
        server_time: receipt.server_time,
        rotation_hint: receipt.rotation_hint,
        inventory: receipt.inventory.map(|inventory| inventory.disposition),
        rejections: receipt.rejections,
        nodes: receipt.nodes,
        samples: receipt.samples,
    }
}

/// Convert a fully built v1 report into its v2 declaration form. Used only for
/// a Server-managed (v2) Agent configuration; the transient v1 shape never
/// reaches the wire.
pub(crate) fn into_v2_report(report: &AgentReport) -> AgentReport<InventoryDeclaration> {
    AgentReport {
        protocol_version: platpulse_core::protocol::PROTOCOL_VERSION_V2,
        agent_id: report.agent_id,
        agent_epoch: report.agent_epoch,
        boot_id: report.boot_id,
        previous_boot_id: report.previous_boot_id,
        boot_transition: report.boot_transition,
        report_sequence: report.report_sequence,
        report_id: report.report_id,
        generated_at: report.generated_at,
        agent_version: report.agent_version.clone(),
        agent_capabilities: report.agent_capabilities.clone(),
        inventory: InventoryDeclaration {
            nodes: report.inventory.nodes.clone(),
        },
        host: report.host.clone(),
        nodes: report.nodes.clone(),
        block_summaries: report.block_summaries.clone(),
        history_gaps: report.history_gaps.clone(),
    }
}

/// Re-shape a stored v2 snapshot as the transient local v1 view used for
/// last-good preservation and boot checks. It is never sent.
pub(crate) fn into_v1_snapshot(report: AgentReport<InventoryDeclaration>) -> AgentReport {
    AgentReport {
        protocol_version: platpulse_core::protocol::PROTOCOL_VERSION,
        agent_id: report.agent_id,
        agent_epoch: report.agent_epoch,
        boot_id: report.boot_id,
        previous_boot_id: report.previous_boot_id,
        boot_transition: report.boot_transition,
        report_sequence: report.report_sequence,
        report_id: report.report_id,
        generated_at: report.generated_at,
        agent_version: report.agent_version,
        agent_capabilities: report.agent_capabilities,
        inventory: platpulse_core::inventory::NodeInventory {
            revision: 1,
            nodes: report.inventory.nodes,
        },
        host: report.host,
        nodes: report.nodes,
        block_summaries: report.block_summaries,
        history_gaps: report.history_gaps,
    }
}

/// The boot identity a delivered report proves, under either protocol major.
pub(crate) fn report_boot_identity(
    body: &[u8],
) -> Result<(platpulse_core::BootId, BootTransition), serde_json::Error> {
    if report_protocol_major(body) == platpulse_core::protocol::PROTOCOL_VERSION_V2 {
        let report: AgentReport<InventoryDeclaration> = serde_json::from_slice(body)?;
        Ok((report.boot_id, report.boot_transition))
    } else {
        let report: AgentReport = serde_json::from_slice(body)?;
        Ok((report.boot_id, report.boot_transition))
    }
}

/// Decode a spooled report body under its declared major, returning the fields
/// the immutable spool needs. Each major keeps its own strict decoding.
fn parse_spooled_report(
    body: &[u8],
) -> Result<
    (
        String,
        Vec<platpulse_core::block::BlockSummary>,
        Vec<platpulse_core::gap::HistoryGap>,
    ),
    ReportStoreError,
> {
    if report_protocol_major(body) == platpulse_core::protocol::PROTOCOL_VERSION_V2 {
        let report: AgentReport<InventoryDeclaration> = serde_json::from_slice(body)
            .map_err(|error| ReportStoreError::InvalidReport(error.to_string()))?;
        Ok((
            report.agent_id.to_string(),
            report.block_summaries,
            report.history_gaps,
        ))
    } else {
        let report: AgentReport = serde_json::from_slice(body)
            .map_err(|error| ReportStoreError::InvalidReport(error.to_string()))?;
        Ok((
            report.agent_id.to_string(),
            report.block_summaries,
            report.history_gaps,
        ))
    }
}

/// Send one claimed report. No transport error is an acknowledgement; the
/// in-flight row and exact bytes remain available for the next attempt.
pub async fn deliver_one<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
) -> Result<Option<StoredReport>, ReportStoreError> {
    Ok(deliver_one_typed(store, transport, None)
        .await?
        .map(|step| step.report))
}

/// Send one claimed report and report the Server's top-level disposition, so
/// callers that distinguish acceptance from refusal (Boot recovery, issue
/// #178) never have to infer it from stored Agent state.
pub(crate) async fn deliver_one_with_disposition<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
) -> Result<Option<(StoredReport, ReceiptDisposition)>, ReportStoreError> {
    Ok(deliver_one_typed(store, transport, None)
        .await?
        .map(|step| (step.report, step.disposition)))
}

/// Deliver one report while applying a deadline only to the HTTP send. Once
/// the response arrives, receipt validation and its SQLite transaction are
/// allowed to finish without cancellation.
pub async fn deliver_one_with_send_deadline<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    send_deadline: tokio::time::Instant,
) -> Result<Option<StoredReport>, ReportStoreError> {
    Ok(deliver_one_typed(store, transport, Some(send_deadline))
        .await?
        .map(|step| step.report))
}

/// One applied report, with the operator-facing rejection summary when the
/// Server refused it whole.
struct DeliveryStep {
    report: StoredReport,
    rejection: Option<String>,
    disposition: ReceiptDisposition,
}

async fn deliver_one_typed<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    send_deadline: Option<tokio::time::Instant>,
) -> Result<Option<DeliveryStep>, ReportStoreError> {
    ensure_spool_healthy(store).await?;
    let Some(report) = claim_oldest_report(store).await? else {
        return Ok(None);
    };
    let response = match send_deadline {
        Some(deadline) => match tokio::time::timeout_at(deadline, transport.send(&report.body))
            .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                let _ = record_delivery_failure(store, &error.to_string(), &now_rfc3339()).await;
                return Err(error);
            }
            Err(_) => {
                let _ = record_delivery_failure(
                    store,
                    "delivery sender deadline exhausted",
                    &now_rfc3339(),
                )
                .await;
                return Err(ReportStoreError::DeliveryDeadline);
            }
        },
        None => match transport.send(&report.body).await {
            Ok(response) => response,
            Err(error) => {
                let _ = record_delivery_failure(store, &error.to_string(), &now_rfc3339()).await;
                return Err(error);
            }
        },
    };
    let major = report_protocol_major(&report.body);
    let (envelope_receipt, acceptance) = if major == platpulse_core::protocol::PROTOCOL_VERSION_V2 {
        let envelope: WireReportResponseV2 = serde_json::from_slice(&response)
            .map_err(|error| ReportStoreError::InvalidReceipt(error.to_string()))?;
        envelope
            .receipt
            .validate()
            .map_err(|error| ReportStoreError::InvalidReceipt(error.to_string()))?;
        let acceptance = envelope
            .receipt
            .inventory
            .as_ref()
            .and_then(|inventory| inventory.acceptance.clone());
        (normalize_v2_receipt(envelope.receipt), acceptance)
    } else {
        let envelope: WireReportResponse = serde_json::from_slice(&response)
            .map_err(|error| ReportStoreError::InvalidReceipt(error.to_string()))?;
        envelope
            .receipt
            .validate()
            .map_err(|error| ReportStoreError::InvalidReceipt(error.to_string()))?;
        (envelope.receipt, None)
    };
    let actual_hash = format!("0x{}", hex::encode(Sha256::digest(&report.body)));
    if actual_hash != report.body_sha256 {
        return Err(ReportStoreError::ReceiptMismatch);
    }
    if envelope_receipt.report_id.to_string() != report.report_id
        || envelope_receipt.report_body_sha256.to_string() != actual_hash
    {
        return Err(ReportStoreError::ReceiptMismatch);
    }
    let receipt_disposition = envelope_receipt.disposition;
    let disposition = match receipt_disposition {
        ReceiptDisposition::Accepted => "accepted",
        ReceiptDisposition::PartiallyAccepted => "partially_accepted",
        ReceiptDisposition::Rejected => "rejected",
    };
    // A whole-report rejection is an applied receipt, not a transport failure:
    // it must reach the caller as data so the delivery loop can tell the
    // operator why every report is being refused (issue #181).
    let rejection = (receipt_disposition == ReceiptDisposition::Rejected).then(|| {
        format!(
            "report {} rejected by Server: {}",
            report.report_id,
            rejection_summary(&envelope_receipt)
        )
    });
    if major == platpulse_core::protocol::PROTOCOL_VERSION_V2 {
        apply_receipt_typed::<InventoryDeclaration>(
            store,
            &report.report_id,
            &report.body_sha256,
            disposition,
            envelope_receipt,
            acceptance,
            &now_rfc3339(),
        )
        .await
        .map_err(map_apply_receipt_error)?;
    } else {
        apply_receipt_typed::<platpulse_core::inventory::NodeInventory>(
            store,
            &report.report_id,
            &report.body_sha256,
            disposition,
            envelope_receipt,
            acceptance,
            &now_rfc3339(),
        )
        .await
        .map_err(map_apply_receipt_error)?;
    }
    Ok(Some(DeliveryStep {
        report,
        rejection,
        disposition: receipt_disposition,
    }))
}

/// Map a receipt-application failure onto the delivery error type.
fn map_apply_receipt_error(error: ApplyReceiptError) -> ReportStoreError {
    match error {
        ApplyReceiptError::Database(error) => ReportStoreError::Database(error),
        ApplyReceiptError::StaleClosing { report_id } => {
            ReportStoreError::StaleClosing { report_id }
        }
    }
}

/// Claim the durable block and gap samples included in a report.
///
/// The source row and its unassigned state are checked by the same INSERT
/// statement that creates the ownership marker. A zero-row insert means the
/// collection snapshot became stale or the report repeated a sample already
/// owned by another in-flight report.
pub(crate) async fn claim_report_samples(
    tx: &mut Transaction<'_, Sqlite>,
    report_id: &str,
    block_summaries: &[platpulse_core::block::BlockSummary],
    history_gaps: &[platpulse_core::gap::HistoryGap],
) -> Result<bool, sqlx::Error> {
    for sample in block_summaries {
        let result = sqlx::query(
            "INSERT INTO report_sample_assignments (report_id, node_id, sample_kind, from_height, to_height) SELECT ?, b.node_id, 'block', b.block_number, b.block_number FROM block_summaries b WHERE b.node_id = ? AND b.block_number = ? AND b.block_hash = ? AND NOT EXISTS (SELECT 1 FROM report_sample_assignments a WHERE a.node_id = b.node_id AND a.sample_kind = 'block' AND a.from_height = b.block_number AND a.to_height = b.block_number)",
        )
        .bind(report_id)
        .bind(sample.node_id.to_string())
        .bind(sample.block_number as i64)
        .bind(sample.block_hash.to_string())
        .execute(&mut **tx)
        .await?;
        if result.rows_affected() != 1 {
            return Ok(false);
        }
    }
    for gap in history_gaps {
        let result = sqlx::query(
            "INSERT INTO report_sample_assignments (report_id, node_id, sample_kind, from_height, to_height) SELECT ?, g.node_id, 'gap', g.from_height, g.to_height FROM history_gaps g WHERE g.node_id = ? AND g.from_height = ? AND g.to_height = ? AND g.kind = ? AND NOT EXISTS (SELECT 1 FROM report_sample_assignments a WHERE a.node_id = g.node_id AND a.sample_kind = 'gap' AND a.from_height = g.from_height AND a.to_height = g.to_height)",
        )
        .bind(report_id)
        .bind(gap.node_id.to_string())
        .bind(gap.from_height as i64)
        .bind(gap.to_height as i64)
        .bind(crate::block::gap_kind_name(gap.kind))
        .execute(&mut **tx)
        .await?;
        if result.rows_affected() != 1 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The body is inserted exactly as supplied and cannot be updated later.
pub async fn persist_immutable_report(
    store: &mut AgentStore,
    report_id: &str,
    agent_epoch: u64,
    boot_id: &str,
    report_sequence: u64,
    generated_at: &str,
    body: &[u8],
) -> Result<String, ReportStoreError> {
    persist_immutable_report_inner(
        store,
        report_id,
        agent_epoch,
        boot_id,
        report_sequence,
        generated_at,
        body,
        None,
    )
    .await
}

/// Persist a Closing report while revalidating and advancing the authoritative
/// Agent lifecycle state in the same transaction as the immutable report.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_closing_report(
    store: &mut AgentStore,
    report_id: &str,
    agent_epoch: u64,
    boot_id: &str,
    report_sequence: u64,
    generated_at: &str,
    body: &[u8],
    expected_report_sequence: u64,
    expected_boot_state: &str,
) -> Result<String, ReportStoreError> {
    let report = serde_json::from_slice::<AgentReport>(body)
        .map_err(|error| ReportStoreError::InvalidReport(error.to_string()))?;
    if report.boot_transition != BootTransition::Closing {
        return Err(ReportStoreError::InvalidReport(
            "closing report has the wrong boot transition".to_owned(),
        ));
    }
    persist_immutable_report_inner(
        store,
        report_id,
        agent_epoch,
        boot_id,
        report_sequence,
        generated_at,
        body,
        Some(ClosingStateExpectation {
            agent_epoch,
            boot_id: boot_id.to_owned(),
            report_sequence: expected_report_sequence,
            boot_state: expected_boot_state.to_owned(),
        }),
    )
    .await
}

struct ClosingStateExpectation {
    agent_epoch: u64,
    boot_id: String,
    report_sequence: u64,
    boot_state: String,
}

#[allow(clippy::too_many_arguments)]
async fn persist_immutable_report_inner(
    store: &mut AgentStore,
    report_id: &str,
    agent_epoch: u64,
    boot_id: &str,
    report_sequence: u64,
    generated_at: &str,
    body: &[u8],
    closing: Option<ClosingStateExpectation>,
) -> Result<String, ReportStoreError> {
    if body.is_empty() {
        return Err(ReportStoreError::Empty);
    }
    ensure_spool_healthy(store).await?;
    if body.len() > platpulse_core::protocol::MAX_REPORT_BODY_BYTES {
        let _ = mark_report_too_large(store, generated_at).await;
        return Err(ReportStoreError::ReportTooLarge);
    }
    let digest = format!("0x{}", hex::encode(Sha256::digest(body)));
    let candidate = SpoolReportRow {
        report_id: report_id.to_owned(),
        agent_epoch: agent_epoch as i64,
        boot_id: boot_id.to_owned(),
        report_sequence: report_sequence as i64,
        generated_at: generated_at.to_owned(),
        body: body.to_vec(),
        body_sha256: digest.clone(),
        body_bytes: body.len() as i64,
    };
    if let Some(reason) = validate_spool_report(&candidate) {
        return Err(ReportStoreError::InvalidReport(reason));
    }
    let (agent_id, block_summaries, history_gaps) = parse_spooled_report(body)?;
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    let transaction_result: Result<(), sqlx::Error> = async {
        if let Some(closing) = closing.as_ref() {
            let state: Option<(i64, Option<String>, i64, String)> = sqlx::query_as(
                "SELECT agent_epoch, boot_id, report_sequence, boot_state FROM agent_state WHERE singleton=1",
            )
            .fetch_optional(&mut *tx)
            .await?;
            let Some((current_epoch, current_boot_id, current_sequence, current_boot_state)) = state
            else {
                return Err(sqlx::Error::Protocol(
                    "Agent state is missing while storing Closing report".to_owned(),
                ));
            };
            if current_epoch != closing.agent_epoch as i64
                || current_boot_id.as_deref() != Some(closing.boot_id.as_str())
                || current_sequence != closing.report_sequence as i64
                || current_boot_state != closing.boot_state
            {
                return Err(sqlx::Error::Protocol(
                    "Agent state changed while storing Closing report".to_owned(),
                ));
            }
        }
        sqlx::query(
            "INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, in_flight, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(report_id)
        .bind(agent_epoch as i64)
        .bind(boot_id)
        .bind(report_sequence as i64)
        .bind(generated_at)
        .bind(body)
        .bind(&digest)
        .bind(body.len() as i64)
        .bind(generated_at)
        .execute(&mut *tx)
        .await?;
        persist_last_report_snapshot(
            &mut tx,
            &agent_id,
            agent_epoch,
            boot_id,
            report_sequence,
            body,
        )
        .await?;
        if !claim_report_samples(
            &mut tx,
            report_id,
            &block_summaries,
            &history_gaps,
        )
        .await?
        {
            return Err(sqlx::Error::Protocol(
                "report samples changed while storing report".to_owned(),
            ));
        }
        if let Some(closing) = closing.as_ref() {
            let result = sqlx::query(
                "UPDATE agent_state SET report_sequence=?, shutdown_state='final_stored', shutdown_report_id=?, shutdown_report_sequence=?, shutdown_finished_at=?, shutdown_updated_at=?, updated_at=? WHERE singleton=1 AND agent_epoch=? AND boot_id=? AND report_sequence=? AND boot_state=?",
            )
            .bind(report_sequence as i64)
            .bind(report_id)
            .bind(report_sequence as i64)
            .bind(generated_at)
            .bind(generated_at)
            .bind(generated_at)
            .bind(closing.agent_epoch as i64)
            .bind(&closing.boot_id)
            .bind(closing.report_sequence as i64)
            .bind(&closing.boot_state)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() != 1 {
                return Err(sqlx::Error::Protocol(
                    "Agent state changed while storing Closing report".to_owned(),
                ));
            }
        }
        Ok(())
    }
    .await;
    match transaction_result {
        Ok(()) => tx.commit().await?,
        Err(error) => {
            tx.rollback().await?;
            return Err(error.into());
        }
    }
    drop(_write_permit);
    enforce_spool_policy(store, &SpoolPolicy::default(), generated_at).await?;
    Ok(digest)
}

/// Persist the latest complete observation view independently of the
/// delivery queue. The snapshot survives receipt application and is used as
/// the last-good baseline for the next collection cycle.
pub(crate) async fn persist_last_report_snapshot(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
    agent_epoch: u64,
    boot_id: &str,
    report_sequence: u64,
    body: &[u8],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_state
         SET last_report_body=?
         WHERE singleton=1
           AND agent_id=?
           AND agent_epoch=?
           AND (boot_id IS NULL OR boot_id=?)
           AND report_sequence <= ?",
    )
    .bind(body)
    .bind(agent_id)
    .bind(agent_epoch as i64)
    .bind(boot_id)
    .bind(report_sequence as i64)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

/// What one delivery pass did (issue #181).
///
/// A whole-report rejection is not an error: the Server's receipt was applied,
/// the report left the spool, and only the Agent's declaration is wrong. It is
/// returned instead of only being written to the durable Agent diagnostic, so
/// the delivery loop can tell the operator instead of leaving the failure
/// visible nowhere but the Admin page.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeliveryOutcome {
    /// Reports whose receipt was applied in this pass, accepted or rejected.
    pub applied: usize,
    /// Operator-facing summaries of the whole-report rejections applied in
    /// this pass, in delivery order.
    pub rejections: Vec<String>,
}

impl DeliveryOutcome {
    /// The most recent rejection summary, which is what a one-line-per-pass
    /// console prints.
    pub fn last_rejection(&self) -> Option<&str> {
        self.rejections.last().map(String::as_str)
    }
}

/// Deliver a bounded amount of oldest-first work.
///
/// A single queued report is the steady state: collection produced it and the
/// next tick delivers it, so the worker stays at one report per tick and
/// collection remains responsive. Anything beyond that is a backlog (a Server
/// restart, a slow receipt). A backlog must drain faster than the collection
/// cadence, otherwise delivery exactly cancels collection and the queue never
/// empties: every later report then waits behind it and the spool drifts
/// toward its overflow limit (issue #137).
pub async fn deliver_periodic<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    policy: &SpoolPolicy,
) -> Result<DeliveryOutcome, ReportStoreError> {
    deliver_periodic_inner(store, transport, policy, None).await
}

pub(crate) async fn deliver_periodic_with_send_deadline<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    policy: &SpoolPolicy,
    send_deadline: tokio::time::Instant,
) -> Result<DeliveryOutcome, ReportStoreError> {
    deliver_periodic_inner(store, transport, policy, Some(send_deadline)).await
}

async fn deliver_periodic_inner<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    policy: &SpoolPolicy,
    send_deadline: Option<tokio::time::Instant>,
) -> Result<DeliveryOutcome, ReportStoreError> {
    let queued_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(body_bytes), 0) FROM reports WHERE in_flight = 0")
            .fetch_one(store.connection())
            .await?;
    let queued_reports: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM reports WHERE in_flight = 0")
            .fetch_one(store.connection())
            .await?;
    let backlogged = queued_bytes.max(0) as u64 >= policy.preflush_bytes || queued_reports > 1;
    let max_reports = if backlogged { 8 } else { 1 };
    let mut outcome = DeliveryOutcome::default();
    for _ in 0..max_reports {
        let result = match send_deadline {
            Some(deadline) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(ReportStoreError::DeliveryDeadline);
                }
                deliver_one_typed(store, transport, Some(deadline)).await
            }
            None => deliver_one_typed(store, transport, None).await,
        }?;
        match result {
            Some(step) => {
                outcome.applied += 1;
                if let Some(rejection) = step.rejection {
                    outcome.rejections.push(rejection);
                }
            }
            None => break,
        }
    }
    Ok(outcome)
}

/// Refuse new collection/delivery when durable spool corruption was observed.
pub async fn ensure_spool_healthy(store: &mut AgentStore) -> Result<(), ReportStoreError> {
    let _write_permit = store.acquire_write().await;
    let fatal: i64 = sqlx::query_scalar("SELECT store_fatal FROM spool_state WHERE singleton=1")
        .fetch_one(store.connection())
        .await?;
    if fatal != 0 {
        return Err(ReportStoreError::StoreFatal(
            "Agent Store is marked fatal; manual recovery is required".to_owned(),
        ));
    }
    let rows = sqlx::query_as::<_, SpoolReportRow>(
        "SELECT report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes FROM reports ORDER BY created_at, report_id",
    )
    .fetch_all(store.connection())
    .await?;
    if let Some(reason) = rows.iter().find_map(validate_spool_report) {
        mark_spool_fatal_under_permit(store, &now_rfc3339(), &reason).await?;
        return Err(ReportStoreError::StoreFatal(reason));
    }
    let collision: Option<String> = sqlx::query_scalar(
        "SELECT receipt.report_id
         FROM reports
         CROSS JOIN report_receipts AS receipt
         WHERE receipt.report_id = reports.report_id
         ORDER BY receipt.applied_at, receipt.report_id
         LIMIT 1",
    )
    .fetch_optional(store.connection())
    .await?;
    if let Some(report_id) = collision {
        let reason =
            format!("queued Agent Report {report_id} conflicts with an Applied Receipt Record");
        mark_spool_fatal_under_permit(store, &now_rfc3339(), &reason).await?;
        return Err(ReportStoreError::StoreFatal(reason));
    }
    Ok(())
}

async fn validate_historical_receipt_markers(
    store: &mut AgentStore,
) -> Result<(), ReportStoreError> {
    let _write_permit = store.acquire_write().await;
    let mut markers = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT report_id, report_body_sha256, disposition, applied_at
         FROM report_receipts ORDER BY applied_at, report_id",
    )
    .fetch(store.connection());
    while let Some((report_id, body_sha256, disposition, applied_at)) = markers.try_next().await? {
        let canonical_time = applied_at
            .parse::<Rfc3339>()
            .ok()
            .is_some_and(|time| time.to_string() == applied_at);
        let valid = report_id.parse::<ReportId>().is_ok()
            && body_sha256.parse::<Sha256Hex>().is_ok()
            && canonical_time
            && matches!(
                disposition.as_str(),
                "accepted" | "partially_accepted" | "rejected"
            );
        if !valid {
            let reason = format!("Applied Receipt Record {report_id} is invalid");
            drop(markers);
            mark_spool_fatal_under_permit(store, &now_rfc3339(), &reason).await?;
            return Err(ReportStoreError::StoreFatal(reason));
        }
    }
    Ok(())
}

/// Perform the one-time startup validation of current Agent Store state.
/// Runtime delivery and collection use the bounded health gate instead, so
/// they never rescan historical Applied Receipt Records.
pub async fn validate_receipt_history(store: &mut AgentStore) -> Result<(), ReportStoreError> {
    ensure_spool_healthy(store).await?;
    validate_historical_receipt_markers(store).await?;
    loop {
        if cleanup_expired_receipt_markers(store, &now_rfc3339()).await? < 64 {
            return Ok(());
        }
    }
}

/// Apply a bounded, transactional spool policy. In-flight and newest complete
/// current report are never deleted; each dropped report contributes bounded
/// loss diagnostics without manufacturing Agent-side history.
pub async fn enforce_spool_policy(
    store: &mut AgentStore,
    policy: &SpoolPolicy,
    now: &str,
) -> Result<SpoolCleanupSummary, ReportStoreError> {
    ensure_spool_healthy(store).await?;
    let rows = sqlx::query_as::<_, SpoolReportRow>(
        "SELECT report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes FROM reports ORDER BY created_at, report_id",
    )
    .fetch_all(store.connection())
    .await?;
    let mut sample_stats = HashMap::with_capacity(rows.len());
    for row in &rows {
        if let Some(reason) = validate_spool_report(row) {
            mark_spool_fatal_store(store, now, &reason).await?;
            return Err(ReportStoreError::StoreFatal(reason));
        }
        let (_, block_summaries, history_gaps) = parse_spooled_report(&row.body)?;
        let mut sample_count = 0u64;
        let mut height_range: Option<(u64, u64)> = None;
        for sample in &block_summaries {
            sample_count += 1;
            height_range = Some(
                height_range.map_or((sample.block_number, sample.block_number), |(from, to)| {
                    (from.min(sample.block_number), to.max(sample.block_number))
                }),
            );
        }
        for gap in &history_gaps {
            sample_count += 1;
            height_range = Some(
                height_range.map_or((gap.from_height, gap.to_height), |(from, to)| {
                    (from.min(gap.from_height), to.max(gap.to_height))
                }),
            );
        }
        sample_stats.insert(row.report_id.clone(), (sample_count, height_range));
    }

    let cutoff = time::OffsetDateTime::parse(now, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|value| value - time::Duration::seconds(policy.max_age_seconds as i64));
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    let transaction_result: Result<SpoolCleanupSummary, ReportStoreError> = async {
        let rows: Vec<(String, i64, String, i64, i64)> = sqlx::query_as(
            "SELECT report_id, report_sequence, generated_at, body_bytes, in_flight FROM reports ORDER BY created_at, report_id",
        )
    .fetch_all(&mut *tx)
    .await?;
    let total: i64 = rows
        .iter()
        .map(|(_, _, _, body_bytes, _)| (*body_bytes).max(0))
        .sum();
    let current_report_id: Option<String> = sqlx::query_scalar(
        "SELECT r.report_id FROM reports r JOIN agent_state s ON s.singleton = 1 AND s.agent_id IS NOT NULL AND s.agent_epoch = r.agent_epoch AND s.boot_id = r.boot_id AND s.report_sequence = r.report_sequence ORDER BY r.created_at DESC, r.report_id DESC LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?
    .or(
        sqlx::query_scalar(
            "SELECT report_id FROM reports ORDER BY agent_epoch DESC, report_sequence DESC, created_at DESC, report_id DESC LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?,
    );
    let mut summary = SpoolCleanupSummary::default();
    let mut bytes = total;
    let max_bytes = policy.max_bytes.min(i64::MAX as u64) as i64;
    for (report_id, sequence, generated_at, body_bytes, in_flight) in rows {
        if bytes <= max_bytes
            && cutoff.as_ref().is_none_or(|cutoff| {
                time::OffsetDateTime::parse(
                    &generated_at,
                    &time::format_description::well_known::Rfc3339,
                )
                .map(|value| value >= *cutoff)
                .unwrap_or(true)
            })
        {
            continue;
        }
        let Some((sample_count, height_range)) = sample_stats.get(&report_id).copied() else {
            continue;
        };
        if in_flight == 1 || current_report_id.as_deref() == Some(report_id.as_str()) {
            continue;
        }
        let deleted = sqlx::query("DELETE FROM reports WHERE report_id = ? AND in_flight = 0")
            .bind(&report_id)
            .execute(&mut *tx)
            .await?;
        if deleted.rows_affected() == 0 {
            continue;
        }
        bytes -= body_bytes.max(0);
        summary.dropped_reports += 1;
        summary.sequence_range = Some(
            summary
                .sequence_range
                .map_or((sequence as u64, sequence as u64), |(from, to)| {
                    (from.min(sequence as u64), to.max(sequence as u64))
                }),
        );
        summary.time_range = Some(summary.time_range.take().map_or(
            (generated_at.clone(), generated_at.clone()),
            |(from, to)| (from.min(generated_at.clone()), to.max(generated_at.clone())),
        ));
        summary.dropped_samples += sample_count;
        if let Some(range) = height_range {
            summary.height_range = Some(
                summary
                    .height_range
                    .take()
                    .map_or(range, |(from, to)| (from.min(range.0), to.max(range.1))),
            );
        }
    }
    sqlx::query("UPDATE spool_state SET dropped_reports=dropped_reports+?, dropped_samples=dropped_samples+?, dropped_sequence_from=COALESCE(MIN(dropped_sequence_from, ?), ?), dropped_sequence_to=MAX(COALESCE(dropped_sequence_to, ?), ?), dropped_time_from=COALESCE(MIN(dropped_time_from, ?), ?), dropped_time_to=MAX(COALESCE(dropped_time_to, ?), ?), dropped_height_from=COALESCE(MIN(dropped_height_from, ?), ?), dropped_height_to=MAX(COALESCE(dropped_height_to, ?), ?), pending_history_gaps=pending_history_gaps+?, updated_at=? WHERE singleton=1")
        .bind(summary.dropped_reports as i64).bind(summary.dropped_samples as i64)
        .bind(summary.sequence_range.map(|v| v.0 as i64)).bind(summary.sequence_range.map(|v| v.0 as i64))
        .bind(summary.sequence_range.map(|v| v.1 as i64)).bind(summary.sequence_range.map(|v| v.1 as i64))
        .bind(summary.time_range.as_ref().map(|v| v.0.as_str())).bind(summary.time_range.as_ref().map(|v| v.0.as_str()))
        .bind(summary.time_range.as_ref().map(|v| v.1.as_str())).bind(summary.time_range.as_ref().map(|v| v.1.as_str()))
        .bind(summary.height_range.map(|v| v.0 as i64)).bind(summary.height_range.map(|v| v.0 as i64))
        .bind(summary.height_range.map(|v| v.1 as i64)).bind(summary.height_range.map(|v| v.1 as i64))
        .bind(summary.pending_history_gaps as i64).bind(now).execute(&mut *tx).await?;
        Ok(summary)
    }
    .await;
    match transaction_result {
        Ok(summary) => {
            tx.commit().await?;
            Ok(summary)
        }
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

fn validate_spool_report(row: &SpoolReportRow) -> Option<String> {
    let expected_hash = format!("0x{}", hex::encode(Sha256::digest(&row.body)));
    if row.body_bytes < 0 || row.body_bytes as usize != row.body.len() {
        return Some(format!(
            "immutable spool report {} has an invalid body length",
            row.report_id
        ));
    }
    if expected_hash != row.body_sha256 {
        return Some(format!(
            "immutable spool report {} failed hash validation",
            row.report_id
        ));
    }
    let (validation, report_id, agent_epoch, boot_id, report_sequence, generated_at) =
        if report_protocol_major(&row.body) == platpulse_core::protocol::PROTOCOL_VERSION_V2 {
            match serde_json::from_slice::<AgentReport<InventoryDeclaration>>(&row.body) {
                Ok(report) => (
                    report.validate(),
                    report.report_id.to_string(),
                    report.agent_epoch,
                    report.boot_id.to_string(),
                    report.report_sequence,
                    report.generated_at.to_string(),
                ),
                Err(error) => {
                    return Some(format!(
                        "immutable spool report {} is not a valid v2 AgentReport: {error}",
                        row.report_id
                    ));
                }
            }
        } else {
            match serde_json::from_slice::<AgentReport>(&row.body) {
                Ok(report) => (
                    report.validate(),
                    report.report_id.to_string(),
                    report.agent_epoch,
                    report.boot_id.to_string(),
                    report.report_sequence,
                    report.generated_at.to_string(),
                ),
                Err(error) => {
                    return Some(format!(
                        "immutable spool report {} is not a valid AgentReport: {error}",
                        row.report_id
                    ));
                }
            }
        };
    if let Err(error) = validation {
        return Some(format!(
            "immutable spool report {} failed protocol validation: {error}",
            row.report_id
        ));
    }
    if report_id != row.report_id
        || agent_epoch != row.agent_epoch.max(0) as u64
        || boot_id != row.boot_id
        || report_sequence != row.report_sequence.max(0) as u64
        || generated_at != row.generated_at
    {
        return Some(format!(
            "immutable spool report {} metadata does not match its stored identity",
            row.report_id
        ));
    }
    None
}

async fn mark_spool_fatal_store(
    store: &mut AgentStore,
    now: &str,
    message: &str,
) -> Result<(), ReportStoreError> {
    let _write_permit = store.acquire_write().await;
    mark_spool_fatal_under_permit(store, now, message).await
}

async fn mark_spool_fatal_under_permit(
    store: &mut AgentStore,
    now: &str,
    message: &str,
) -> Result<(), ReportStoreError> {
    sqlx::query(
        "UPDATE spool_state SET store_fatal=1, store_error=?, updated_at=? WHERE singleton=1",
    )
    .bind(message.chars().take(256).collect::<String>())
    .bind(now)
    .execute(store.connection())
    .await?;
    Ok(())
}

async fn mark_report_too_large(store: &mut AgentStore, now: &str) -> Result<(), ReportStoreError> {
    let _write_permit = store.acquire_write().await;
    sqlx::query("UPDATE spool_state SET report_too_large=1, store_error='minimum complete current report exceeds protocol limit', updated_at=? WHERE singleton=1")
        .bind(now).execute(store.connection()).await?;
    Ok(())
}

/// This is the smallest runtime path used by the Agent CLI before a
/// delivery sender is introduced: configuration is loaded and validated as a
/// whole, the report is revalidated, and the exact bytes are spooled in one
/// SQLite transaction.
pub async fn persist_report_from_config(
    config_path: &Path,
    report_path: &Path,
) -> Result<String, ReportStoreError> {
    let config = AgentConfig::resolve(config_path)?;
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| ReportStoreError::RuntimeOwnership(error.to_string()))?;
    persist_report_from_config_with_permit(
        &config,
        report_path,
        crate::database::AgentStoreWritePermit::new(),
    )
    .await
}

pub(crate) async fn persist_report_from_config_with_permit(
    config: &AgentConfig,
    report_path: &Path,
    write_permit: crate::database::AgentStoreWritePermit,
) -> Result<String, ReportStoreError> {
    let validated = config.validated_inventory()?;
    let body = std::fs::read(report_path).map_err(|source| ReportStoreError::ReadReport {
        path: report_path.to_owned(),
        source,
    })?;
    // A Server-managed (v2) Agent persists the declaration form and has no local
    // revision to guard; the report is already the revision-excluded shape.
    if validated.server_managed_inventory {
        let report: platpulse_core::AgentReport<InventoryDeclaration> =
            serde_json::from_slice(&body)
                .map_err(|error| ReportStoreError::InvalidReport(error.to_string()))?;
        report
            .validate()
            .map_err(|error| ReportStoreError::InvalidReport(error.to_string()))?;
        if report.inventory != validated.declaration {
            return Err(ReportStoreError::InventoryMismatch);
        }
        let mut store = AgentStore::open_with_write_permit(
            AgentDatabaseConfig::new(&config.state_db),
            write_permit,
        )
        .await?;
        validate_receipt_history(&mut store).await?;
        let digest = persist_immutable_report(
            &mut store,
            &report.report_id.to_string(),
            report.agent_epoch,
            &report.boot_id.to_string(),
            report.report_sequence,
            &report.generated_at.to_string(),
            &body,
        )
        .await?;
        store.close().await?;
        return Ok(digest);
    }
    let report: platpulse_core::AgentReport = serde_json::from_slice(&body)
        .map_err(|error| ReportStoreError::InvalidReport(error.to_string()))?;
    report
        .validate()
        .map_err(|error| ReportStoreError::InvalidReport(error.to_string()))?;
    if report.inventory != validated.inventory {
        return Err(ReportStoreError::InventoryMismatch);
    }
    let mut store = AgentStore::open_with_write_permit(
        AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await?;
    validate_receipt_history(&mut store).await?;
    // Refuse to spool a report whose Inventory the Server would refuse
    // (issue #181): the report is immutable, so a conflict cannot be fixed
    // after the fact. This frozen v1 guard applies only to a v1 configuration.
    crate::collector::guard_inventory_declaration(&mut store, &validated.inventory).await?;
    let digest = persist_immutable_report(
        &mut store,
        &report.report_id.to_string(),
        report.agent_epoch,
        &report.boot_id.to_string(),
        report.report_sequence,
        &report.generated_at.to_string(),
        &body,
    )
    .await?;
    store.close().await?;
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn configured_report_is_validated_and_spooled_immutably() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("agent.toml");
        let report_path = dir.path().join("report.json");
        let db_path = dir.path().join("agent.db");
        let body = include_str!("../../platpulse-core/tests/fixtures/report_v1_minimal.json");
        fs::write(&report_path, body).unwrap();
        fs::write(
            &config_path,
            format!(
                "server_url=\"https://example.com\"\ncredential_file=\"{}/credential\"\nstate_db=\"{}\"\ninventory_revision=1\nnodes=[{{node_id=\"0195f2a1-0014-4014-8014-000000000014\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:6790\"}}]\n",
                dir.path().display(),
                db_path.display()
            ),
        )
        .unwrap();

        let digest = persist_report_from_config(&config_path, &report_path)
            .await
            .unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(&db_path))
            .await
            .unwrap();
        let row: (Vec<u8>, String, i64) = sqlx::query_as(
            "SELECT body, body_sha256, report_sequence FROM reports WHERE report_id = ?",
        )
        .bind("0195f2a1-0013-4013-8013-000000000013")
        .fetch_one(store.connection())
        .await
        .unwrap();
        assert_eq!(row.0, body.as_bytes());
        assert_eq!(row.1, digest);
        assert_eq!(row.2, 1);
        store.close().await.unwrap();
    }

    #[tokio::test]
    async fn configured_report_rejects_inventory_mismatch_before_spooling() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("agent.toml");
        let report_path = dir.path().join("report.json");
        fs::write(
            &report_path,
            include_str!("../../platpulse-core/tests/fixtures/report_v1_minimal.json"),
        )
        .unwrap();
        fs::write(
            &config_path,
            format!(
                "server_url=\"https://example.com\"\ncredential_file=\"{}/credential\"\nstate_db=\"{}/agent.db\"\ninventory_revision=2\nnodes=[]\n",
                dir.path().display(),
                dir.path().display()
            ),
        )
        .unwrap();
        assert!(matches!(
            persist_report_from_config(&config_path, &report_path).await,
            Err(ReportStoreError::InventoryMismatch)
        ));
        assert!(!dir.path().join("agent.db").exists());
    }

    /// A Server-managed configuration (no inventory_revision) must spool the
    /// revision-excluded v2 declaration, exercising the production
    /// `report_body_bytes` -> `into_v2_report` path rather than a hand-built body.
    #[tokio::test]
    async fn configured_v2_report_without_inventory_revision_is_spooled_as_a_declaration() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("agent.toml");
        let report_path = dir.path().join("report.json");
        let db_path = dir.path().join("agent.db");
        let body = include_str!("../../platpulse-core/tests/fixtures/report_v2_minimal.json");
        fs::write(&report_path, body).unwrap();
        fs::write(
            &config_path,
            format!(
                "server_url=\"https://example.com\"\ncredential_file=\"{}/credential\"\nstate_db=\"{}\"\nnodes=[{{node_id=\"0195f2a1-0014-4014-8014-000000000014\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:6790\"}}]\n",
                dir.path().display(),
                db_path.display()
            ),
        )
        .unwrap();
        let digest = persist_report_from_config(&config_path, &report_path)
            .await
            .unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(&db_path))
            .await
            .unwrap();
        let stored: (Vec<u8>, String) =
            sqlx::query_as("SELECT body, body_sha256 FROM reports WHERE report_id = ?")
                .bind("0195f2a1-0013-4013-8013-000000000013")
                .fetch_one(store.connection())
                .await
                .unwrap();
        let spooled: serde_json::Value = serde_json::from_slice(&stored.0).unwrap();
        assert_eq!(
            spooled["protocol_version"], 2,
            "a Server-managed configuration must spool the v2 declaration"
        );
        assert_eq!(
            spooled["inventory"].get("revision"),
            None,
            "v2 must not carry an Agent-assigned revision"
        );
        assert_eq!(stored.1, digest);
        store.close().await.unwrap();
    }
}

#[cfg(test)]
mod delivery_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    type FakeResponse = Result<Vec<u8>, ReportStoreError>;

    struct FakeTransport {
        bodies: Arc<Mutex<Vec<Vec<u8>>>>,
        responses: Arc<Mutex<Vec<FakeResponse>>>,
    }

    impl ReportTransport for FakeTransport {
        fn send<'a>(
            &'a self,
            body: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ReportStoreError>> + Send + 'a>> {
            let bodies = Arc::clone(&self.bodies);
            let response = self.responses.lock().unwrap().remove(0);
            bodies.lock().unwrap().push(body.to_vec());
            Box::pin(async move { response })
        }
    }

    fn report_id(sequence: u64) -> String {
        format!("0195f2a1-000{sequence}-400{sequence}-800{sequence}-00000000000{sequence}")
    }

    fn report_body_at(sequence: u64, generated_at: &str) -> Vec<u8> {
        let mut report: serde_json::Value = serde_json::from_str(include_str!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report["report_id"] = serde_json::Value::String(report_id(sequence));
        report["report_sequence"] = serde_json::json!(sequence);
        report["generated_at"] = serde_json::Value::String(generated_at.to_owned());
        serde_json::to_vec(&report).unwrap()
    }

    fn report_body(sequence: u64) -> Vec<u8> {
        report_body_at(sequence, "2026-08-12T09:00:00Z")
    }

    fn receipt_body(report_id: &str, body: &[u8]) -> Vec<u8> {
        let report: AgentReport = serde_json::from_slice(body).unwrap();
        let nodes = report
            .inventory
            .nodes
            .iter()
            .map(|node| {
                serde_json::json!({
                    "node_id": node.node_id,
                    "current": "accepted",
                    "accepted_component_revisions": [],
                    "rejections": []
                })
            })
            .collect::<Vec<_>>();
        let receipt = serde_json::json!({"report_id": report_id, "disposition": "accepted", "report_body_sha256": format!("0x{}", hex::encode(Sha256::digest(body))), "server_version": "0.1.0", "supported_protocol_majors": [1], "server_time": "2026-01-01T00:00:00Z", "inventory": "accepted", "rejections": [], "nodes": nodes, "samples": []});
        serde_json::to_vec(&serde_json::json!({"receipt": receipt})).unwrap()
    }

    async fn test_store() -> AgentStore {
        let dir = tempdir().unwrap();
        AgentStore::open(AgentDatabaseConfig::new(dir.keep().join("agent.db")))
            .await
            .unwrap()
    }

    /// A whole-report rejection with an Inventory conflict, as the Server
    /// returns it when the declared Inventory content does not match the
    /// accepted content at the same revision (issue #181).
    fn rejected_receipt_body(report_id: &str, body: &[u8]) -> Vec<u8> {
        let receipt = serde_json::json!({"report_id": report_id, "disposition": "rejected", "report_body_sha256": format!("0x{}", hex::encode(Sha256::digest(body))), "server_version": "0.1.0", "supported_protocol_majors": [1], "server_time": "2026-01-01T00:00:00Z", "inventory": "rejected", "rejections": [{"code": "inventory_revision_conflict", "retryable": false, "reason": "Inventory content conflicts at the accepted revision"}], "nodes": [], "samples": []});
        serde_json::to_vec(&serde_json::json!({"receipt": receipt})).unwrap()
    }

    #[tokio::test]
    async fn large_receipt_history_keeps_empty_spool_hot_paths_bounded() {
        const SEEDED_RECEIPT_COUNT: i64 = 100_000;
        let directory = tempdir().unwrap();
        let database = directory.path().join("agent.db");
        let mut store = AgentStore::open(AgentDatabaseConfig::new(&database))
            .await
            .unwrap();
        let applied_at = now_rfc3339();

        sqlx::query(
            "WITH digits(n) AS (VALUES (0), (1), (2), (3), (4), (5), (6), (7), (8), (9))
             INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at)
             SELECT printf('0195f2a1-0013-4013-8013-%012x', a.n + 10 * b.n + 100 * c.n + 1000 * d.n + 10000 * e.n),
                    '0x0000000000000000000000000000000000000000000000000000000000000000',
                    'accepted',
                    ?
             FROM digits AS a
             CROSS JOIN digits AS b
             CROSS JOIN digits AS c
             CROSS JOIN digits AS d
             CROSS JOIN digits AS e",
        )
        .bind(&applied_at)
        .execute(store.connection())
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM report_receipts")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            SEEDED_RECEIPT_COUNT
        );
        store.close().await.unwrap();

        let mut store = AgentStore::open(AgentDatabaseConfig::new(&database))
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM report_receipts")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            SEEDED_RECEIPT_COUNT
        );
        validate_receipt_history(&mut store).await.unwrap();
        let transport = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(Vec::new())),
        };
        assert!(deliver_one(&mut store, &transport).await.unwrap().is_none());
        assert_eq!(
            enforce_spool_policy(&mut store, &SpoolPolicy::default(), &applied_at)
                .await
                .unwrap()
                .dropped_reports,
            0
        );

        let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(
            "EXPLAIN QUERY PLAN
             SELECT receipt.report_id, receipt.report_body_sha256, receipt.disposition, receipt.applied_at
             FROM reports
             CROSS JOIN report_receipts AS receipt
             WHERE receipt.report_id = reports.report_id
             ORDER BY receipt.applied_at, receipt.report_id",
        )
        .fetch_all(store.connection())
        .await
        .unwrap();
        let details = plan
            .iter()
            .map(|(_, _, _, detail)| detail)
            .collect::<Vec<_>>();
        assert!(details.iter().any(|step| step.contains("SCAN reports")));
        assert!(details.iter().any(|step| step.contains("SEARCH receipt")));
        assert!(!details.iter().any(|step| step.contains("SCAN receipt")));

        store.close().await.unwrap();
    }

    /// Issue #137: a backlog that sits below the preflush threshold must still
    /// drain. Previously the worker delivered exactly one report per tick in
    /// that state, which equals the collection cadence, so a Server restart
    /// left the queued reports permanently in the spool and the queue grew
    /// toward its overflow limit instead of recovering.
    #[tokio::test]
    async fn a_backlog_below_the_preflush_threshold_is_drained_not_stalled() {
        let mut store = test_store().await;
        let body = report_body(1);
        // Five small reports stay far below the 1.5 MiB preflush threshold,
        // so only the queued-report count can reveal the backlog.
        let mut responses = Vec::new();
        for sequence in 1..=5u64 {
            let id = report_id(sequence);
            let body = report_body(sequence);
            persist_immutable_report(
                &mut store,
                &id,
                1,
                "0195f2a1-0012-4012-8012-000000000012",
                sequence,
                "2026-08-12T09:00:00Z",
                &body,
            )
            .await
            .unwrap();
            responses.push(Ok(receipt_body(&id, &body)));
        }
        let policy = SpoolPolicy {
            max_bytes: body.len() as u64 * 64,
            max_age_seconds: 24 * 60 * 60,
            preflush_bytes: body.len() as u64 * 64,
        };
        let transport = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(responses)),
        };

        let outcome = deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(
            outcome.applied, 5,
            "a below-preflush backlog must be drained in one tick, not one report"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0,
            "every acknowledged report must leave the spool"
        );
        store.close().await.unwrap();
    }

    #[tokio::test]
    async fn startup_drains_expiry_batches_without_idle_delivery_history_work() {
        let mut store = test_store().await;
        for _ in 0..129 {
            sqlx::query(
                "INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, 'accepted', '2000-01-01T00:00:00Z')",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind("0x0000000000000000000000000000000000000000000000000000000000000000")
            .execute(store.connection())
            .await
            .unwrap();
        }
        let transport = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(Vec::new())),
        };

        assert_eq!(
            deliver_periodic(&mut store, &transport, &SpoolPolicy::default())
                .await
                .unwrap()
                .applied,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM report_receipts")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            129
        );
        validate_receipt_history(&mut store).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM report_receipts")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn successful_delivery_expires_only_its_transactional_cleanup_batch() {
        let mut store = test_store().await;
        let marker_hash = "0x0000000000000000000000000000000000000000000000000000000000000000";
        for _ in 0..129 {
            sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, 'accepted', ?)")
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(marker_hash)
                .bind("2000-01-01T00:00:00Z")
                .execute(store.connection())
                .await
                .unwrap();
        }
        let body = report_body(1);
        let id = report_id(1);
        persist_immutable_report(
            &mut store,
            &id,
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            1,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        let transport = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(vec![Ok(receipt_body(&id, &body))])),
        };

        assert!(deliver_one(&mut store, &transport).await.unwrap().is_some());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM report_receipts")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            66
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM report_receipts WHERE applied_at < '2026-09-01T00:00:00Z'",
            )
            .fetch_one(store.connection())
            .await
            .unwrap(),
            65
        );
        store.close().await.unwrap();
    }

    #[tokio::test]
    async fn older_report_cannot_replace_current_last_report_snapshot() {
        let mut store = test_store().await;
        let boot_id = "0195f2a1-0012-4012-8012-000000000012";
        sqlx::query("INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, updated_at) VALUES (1, ?, 1, ?, 2, 1, ?)")
            .bind("0195f2a1-0011-4011-8011-000000000011")
            .bind(boot_id)
            .bind("2026-08-12T09:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        let current = report_body(2);
        sqlx::query("UPDATE agent_state SET last_report_body=? WHERE singleton=1")
            .bind(&current)
            .execute(store.connection())
            .await
            .unwrap();

        persist_immutable_report(
            &mut store,
            &report_id(1),
            1,
            boot_id,
            1,
            "2026-08-12T09:00:00Z",
            &report_body(1),
        )
        .await
        .unwrap();

        let snapshot: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT last_report_body FROM agent_state WHERE singleton=1")
                .fetch_one(store.connection())
                .await
                .unwrap();
        assert_eq!(snapshot, Some(current));
    }

    #[tokio::test]
    async fn delivery_is_oldest_first_and_failure_keeps_in_flight_bytes() {
        let mut store = test_store().await;
        let first = report_body(1);
        let second = report_body(2);
        persist_immutable_report(
            &mut store,
            &report_id(1),
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            1,
            "2026-08-12T09:00:00Z",
            &first,
        )
        .await
        .unwrap();
        persist_immutable_report(
            &mut store,
            &report_id(2),
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            2,
            "2026-08-12T09:00:00Z",
            &second,
        )
        .await
        .unwrap();
        let fake = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(vec![Err(ReportStoreError::Delivery(
                "offline".into(),
            ))])),
        };
        assert!(deliver_one(&mut store, &fake).await.is_err());
        assert_eq!(
            fake.bodies.lock().unwrap().as_slice(),
            std::slice::from_ref(&first)
        );
        let row: (i64, Vec<u8>) =
            sqlx::query_as("SELECT in_flight, body FROM reports WHERE report_id = ?")
                .bind(report_id(1))
                .fetch_one(store.connection())
                .await
                .unwrap();
        assert_eq!(row, (1, first));
    }

    #[tokio::test]
    async fn invalid_receipt_preserves_report_and_valid_receipt_cleans_it() {
        let mut store = test_store().await;
        let body = report_body(3);
        let id = report_id(3);
        persist_immutable_report(
            &mut store,
            &id,
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            3,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        let invalid = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(vec![Ok(
                br#"{"receipt":{"report_id":"bad"}}"#.to_vec()
            )])),
        };
        assert!(matches!(
            deliver_one(&mut store, &invalid).await,
            Err(ReportStoreError::InvalidReceipt(_))
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id = ?")
                .bind(&id)
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1
        );
        let valid = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(vec![Ok(receipt_body(&id, &body))])),
        };
        assert!(deliver_one(&mut store, &valid).await.is_ok());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id = ?")
                .bind(&id)
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn retry_reuses_the_same_report_id_and_bytes_after_transport_failure() {
        let mut store = test_store().await;
        let id = report_id(4);
        let body = report_body(4);
        persist_immutable_report(
            &mut store,
            &id,
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            4,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        let fake = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(vec![
                Err(ReportStoreError::Delivery("offline".into())),
                Ok(receipt_body(&id, &body)),
            ])),
        };

        assert!(deliver_one(&mut store, &fake).await.is_err());
        assert!(deliver_one(&mut store, &fake).await.is_ok());
        {
            let bodies = fake.bodies.lock().unwrap();
            assert_eq!(bodies.len(), 2);
            assert_eq!(bodies[0], body);
            assert_eq!(bodies[1], bodies[0]);
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id = ?")
                .bind(&id)
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn spool_overflow_drops_oldest_report_but_keeps_newest_current_report() {
        let mut store = test_store().await;
        for sequence in 1..=3 {
            let body = report_body(sequence);
            persist_immutable_report(
                &mut store,
                &report_id(sequence),
                1,
                "0195f2a1-0012-4012-8012-000000000012",
                sequence,
                "2026-08-12T09:00:00Z",
                &body,
            )
            .await
            .unwrap();
        }
        let body_size = report_body(1).len() as u64;
        let summary = enforce_spool_policy(
            &mut store,
            &SpoolPolicy {
                max_bytes: body_size * 2,
                max_age_seconds: 365 * 24 * 60 * 60,
                preflush_bytes: body_size,
            },
            "2026-08-12T10:00:00Z",
        )
        .await
        .unwrap();

        assert_eq!(summary.dropped_reports, 1);
        assert_eq!(summary.pending_history_gaps, 0);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id = ?")
                .bind(report_id(1))
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT dropped_reports FROM spool_state WHERE singleton = 1",
            )
            .fetch_one(store.connection())
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM history_gaps")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn spool_overflow_protects_the_current_state_report_after_clock_rollback() {
        let mut store = test_store().await;
        for (sequence, generated_at) in [
            (1, "2026-08-12T10:00:00Z"),
            (2, "2026-08-12T11:00:00Z"),
            (3, "2026-08-12T09:00:00Z"),
        ] {
            let body = report_body_at(sequence, generated_at);
            persist_immutable_report(
                &mut store,
                &report_id(sequence),
                1,
                "0195f2a1-0012-4012-8012-000000000012",
                sequence,
                generated_at,
                &body,
            )
            .await
            .unwrap();
        }
        sqlx::query("INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, updated_at) VALUES (1, ?, 1, ?, 3, 1, ?)")
            .bind("0195f2a1-0011-4011-8011-000000000011")
            .bind("0195f2a1-0012-4012-8012-000000000012")
            .bind("2026-08-12T09:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        let body_size = report_body(1).len() as u64;

        enforce_spool_policy(
            &mut store,
            &SpoolPolicy {
                max_bytes: body_size * 2,
                max_age_seconds: 365 * 24 * 60 * 60,
                preflush_bytes: body_size,
            },
            "2026-08-12T12:00:00Z",
        )
        .await
        .unwrap();

        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id = ?")
                .bind(report_id(3))
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id = ?")
                .bind(report_id(1))
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn hot_receipt_validation_only_reads_queued_receipts() {
        let mut store = test_store().await;
        let pending_id = report_id(7);
        let pending_body = report_body(7);
        persist_immutable_report(
            &mut store,
            &pending_id,
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            7,
            "2026-08-12T09:00:00Z",
            &pending_body,
        )
        .await
        .unwrap();

        for _ in 0..2_000 {
            let historical_id = uuid::Uuid::new_v4().to_string();
            let historical_hash = format!("0x{}", hex::encode(Sha256::digest(&pending_body)));
            sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, 'accepted', ?)")
                .bind(historical_id)
                .bind(historical_hash)
                .bind("2026-08-12T10:00:00Z")
                .execute(store.connection())
                .await
                .unwrap();
        }

        ensure_spool_healthy(&mut store).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT store_fatal FROM spool_state WHERE singleton = 1",
            )
            .fetch_one(store.connection())
            .await
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn startup_marks_a_queued_report_and_marker_collision_fatal() {
        let mut store = test_store().await;
        let id = report_id(7);
        let body = report_body(7);
        persist_immutable_report(
            &mut store,
            &id,
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            7,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, 'accepted', '2026-08-12T10:00:00Z')",
        )
        .bind(&id)
        .bind(format!("0x{}", hex::encode(Sha256::digest(&body))))
        .execute(store.connection())
        .await
        .unwrap();

        assert!(matches!(
            validate_receipt_history(&mut store).await,
            Err(ReportStoreError::StoreFatal(_))
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT store_fatal FROM spool_state WHERE singleton = 1")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn startup_marks_an_invalid_historical_marker_fatal() {
        let mut store = test_store().await;
        sqlx::query(
            "INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, 'accepted', '2026-08-12T10:00:00Z')",
        )
        .bind("not-a-report-id")
        .bind("not-a-hash")
        .execute(store.connection())
        .await
        .unwrap();

        assert!(matches!(
            validate_receipt_history(&mut store).await,
            Err(ReportStoreError::StoreFatal(_))
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT store_fatal FROM spool_state WHERE singleton = 1")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn corrupt_spool_state_is_marked_fatal_before_delivery() {
        let mut store = test_store().await;
        let id = report_id(5);
        let body = report_body(5);
        persist_immutable_report(
            &mut store,
            &id,
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            5,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE reports SET body_sha256 = ? WHERE report_id = ?")
            .bind("0x0000000000000000000000000000000000000000000000000000000000000000")
            .bind(&id)
            .execute(store.connection())
            .await
            .unwrap();

        assert!(matches!(
            ensure_spool_healthy(&mut store).await,
            Err(ReportStoreError::StoreFatal(_))
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT store_fatal FROM spool_state WHERE singleton = 1",
            )
            .fetch_one(store.connection())
            .await
            .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn corrupt_receipt_state_is_marked_fatal_before_delivery() {
        let mut store = test_store().await;
        let id = report_id(6);
        let body = report_body(6);
        persist_immutable_report(
            &mut store,
            &id,
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            6,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, 'accepted', ?)")
            .bind(&id)
            .bind("bad")
            .bind("2026-08-12T10:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();

        assert!(matches!(
            ensure_spool_healthy(&mut store).await,
            Err(ReportStoreError::StoreFatal(_))
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id = ?",)
                .bind(&id)
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1
        );
    }

    /// Issue #181, direction 1: the record is written from the accepted
    /// receipt, and the guard then refuses exactly the declaration the Server
    /// would refuse — while accepting it once the revision is bumped.
    #[tokio::test]
    async fn accepted_receipt_records_the_declaration_and_the_guard_catches_drift() {
        let mut store = test_store().await;
        let body = report_body(1);
        let id = report_id(1);
        persist_immutable_report(
            &mut store,
            &id,
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            1,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        let transport = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(vec![Ok(receipt_body(&id, &body))])),
        };
        assert!(deliver_one(&mut store, &transport).await.unwrap().is_some());

        let declared: AgentReport = serde_json::from_slice(&body).unwrap();
        let declared = declared.inventory;
        let recorded = crate::inventory_declaration::read_inventory_declaration(store.connection())
            .await
            .unwrap()
            .expect("an accepted Inventory must be recorded");
        assert_eq!(recorded.revision, declared.revision);
        assert_eq!(recorded.sha256, declared.content_sha256().to_string());
        assert_eq!(recorded.report_id, id);

        // Editing the content without bumping the revision is exactly the
        // failure this issue is about: refuse it before it is declared.
        let mut drifted = declared.clone();
        drifted.nodes[0].rpc_endpoint = "ws://127.0.0.1:6791".parse().unwrap();
        let refused = crate::collector::guard_inventory_declaration(&mut store, &drifted)
            .await
            .expect_err("changed content at the accepted revision must be refused");
        assert!(
            matches!(
                refused,
                crate::inventory_declaration::InventoryGuardError::Conflict(
                    crate::inventory_declaration::InventoryDeclarationConflict::ContentChanged {
                        revision,
                        ..
                    }
                ) if revision == declared.revision
            ),
            "{refused:?}"
        );

        // Bumping the revision declares the new content, so the Agent heals
        // without any other intervention.
        drifted.revision += 1;
        crate::collector::guard_inventory_declaration(&mut store, &drifted)
            .await
            .expect("a bumped revision declares the new Node set");
        store.close().await.unwrap();
    }

    /// Issue #181, directions 1+2: a refused report reaches the delivery
    /// caller, never becomes the declaration record, and keeps its durable
    /// local diagnostic.
    #[tokio::test]
    async fn rejected_receipt_reaches_the_caller_and_never_becomes_a_declaration() {
        let mut store = test_store().await;
        let body = report_body(1);
        let id = report_id(1);
        persist_immutable_report(
            &mut store,
            &id,
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            1,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        let transport = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(vec![Ok(rejected_receipt_body(&id, &body))])),
        };

        let outcome = deliver_periodic(&mut store, &transport, &SpoolPolicy::default())
            .await
            .unwrap();
        assert_eq!(outcome.applied, 1);
        let rejection = outcome
            .last_rejection()
            .expect("a whole-report rejection must reach the delivery caller");
        assert!(
            rejection.contains("inventory_revision_conflict"),
            "{rejection}"
        );
        assert!(
            rejection.contains("Inventory content conflicts at the accepted revision"),
            "{rejection}"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM inventory_declaration")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0,
            "a refused declaration must never become the record"
        );
        let diagnostic: Option<String> =
            sqlx::query_scalar("SELECT last_error FROM delivery_diagnostics WHERE singleton=1")
                .fetch_one(store.connection())
                .await
                .unwrap();
        assert!(
            diagnostic
                .expect("the refusal stays visible in the Agent diagnostic")
                .contains("inventory_revision_conflict")
        );
        store.close().await.unwrap();
    }
}

/// Issue #137 end-to-end recovery, in the same style as the Enrollment tests:
/// a real Server (production code, no mocks) is stopped while the Agent keeps
/// collecting, then returns on the same address and database. The backlog must
/// drain through the real HTTP transport without losing a report.
///
/// It also hosts the issue #177 Inventory-declaration acceptance, which needs
/// the same real Server, transport and receipt application.
#[cfg(test)]
mod backlog_recovery_tests {
    use std::path::Path;

    use platpulse_server::auth::{AuthConfig, create_owner, hash_password};
    use platpulse_server::database::{ServerDatabaseConfig, initialize};
    use platpulse_server::enrollment::{
        ENROLLMENT_TOKEN_DEFAULT_LIFETIME, create_enrollment_token,
    };
    use platpulse_server::http::{AppState, build_app};
    use platpulse_server::network::create_network;
    use platpulse_server::secrets::{create_pepper_file, load_pepper_file};
    use tempfile::TempDir;
    use tokio::task::JoinHandle;
    use tokio_util::sync::CancellationToken;

    use super::{AgentReport, HttpReportTransport, deliver_periodic, persist_immutable_report};
    use crate::collector::{CollectionError, SpoolPolicy, guard_startup_inventory_declaration};
    use crate::config::{AgentConfig, BackfillConfig};
    use crate::database::{AgentDatabaseConfig, AgentStore, AgentStoreWritePermit};
    use crate::inventory_declaration::{InventoryDeclarationConflict, read_inventory_declaration};

    const REPORT_COUNT: u64 = 6;
    const BOOT_ID: &str = "0195f2a1-0012-4012-8012-000000000012";

    fn report_id(sequence: u64) -> String {
        format!("0195f2a1-0100-4000-8000-0000000000{sequence:02}")
    }

    fn report_body(sequence: u64, agent_id: &str) -> Vec<u8> {
        let mut report: serde_json::Value = serde_json::from_str(include_str!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report["agent_id"] = serde_json::Value::String(agent_id.to_owned());
        report["report_sequence"] = serde_json::json!(sequence);
        report["report_id"] = serde_json::Value::String(report_id(sequence));
        report["generated_at"] = serde_json::json!("2026-08-12T09:00:00Z");
        serde_json::to_vec(&report).unwrap()
    }

    struct RunningServer {
        state: AppState,
        shutdown: CancellationToken,
        task: JoinHandle<()>,
    }

    impl RunningServer {
        /// Stop the listener and close the owning connection, which is what a
        /// Server restart does to the database file.
        async fn stop(self) {
            self.shutdown.cancel();
            let _ = self.task.await;
            self.state.db().close().await;
        }
    }

    fn agent_config(dir: &Path, server_url: &str) -> AgentConfig {
        AgentConfig {
            config_path: dir.join("agent.toml"),
            server_url: server_url.to_owned(),
            credential_file: dir.join("credential"),
            state_db: dir.join("agent.db"),
            collection_interval_seconds: 5,
            backfill: BackfillConfig::default(),
        }
    }

    /// Boot a real Server on `addr` from the shared state directory.
    async fn boot_server(addr: std::net::SocketAddr, dir: &Path) -> (RunningServer, String) {
        let first_boot = !dir.join("server.db").exists();
        let db = initialize(ServerDatabaseConfig::new(dir.join("server.db")))
            .await
            .unwrap();
        let pepper_path = dir.join("server-pepper");
        if first_boot {
            create_pepper_file(&pepper_path).unwrap();
        }
        let pepper = load_pepper_file(&pepper_path).unwrap();
        let auth = AuthConfig::development(pepper, format!("http://{addr}"));
        let token = create_enrollment_token(&db, &pepper, None, ENROLLMENT_TOKEN_DEFAULT_LIFETIME)
            .await
            .unwrap()
            .token;
        if first_boot {
            create_owner(
                &db,
                "admin",
                &hash_password(b"correct horse battery").unwrap(),
            )
            .await
            .unwrap();
            create_network(
                &db,
                "platon-mainnet",
                "PlatON Mainnet",
                "0x0000000000000000000000000000000000000000000000000000000000000001",
                210425,
                210425,
                "lat",
            )
            .await
            .unwrap();
        }
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let state = AppState::new(db, None, auth);
        let shutdown = CancellationToken::new();
        let served = state.clone();
        let stopping = shutdown.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, build_app(served))
                .with_graceful_shutdown(async move { stopping.cancelled().await })
                .await
                .unwrap();
        });
        (
            RunningServer {
                state,
                shutdown,
                task,
            },
            token,
        )
    }

    #[tokio::test]
    async fn a_backlog_drains_after_a_server_outage_without_losing_reports() {
        let dir = TempDir::new().unwrap();
        // Reserve a concrete port so the stopped Server and the restarted
        // Server share one address, exactly like a service restart.
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);

        let (server, token) = boot_server(addr, dir.path()).await;
        let config = agent_config(dir.path(), &format!("http://{addr}"));
        let enrolled = crate::enroll::enroll_agent(&config, &token).await.unwrap();
        let agent_id = enrolled.agent_id.to_string();
        server.stop().await;

        let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db))
            .await
            .unwrap();
        let transport = HttpReportTransport::from_config(&config).unwrap();
        let policy = SpoolPolicy::default();
        for sequence in 1..=REPORT_COUNT {
            let body = report_body(sequence, &agent_id);
            persist_immutable_report(
                &mut store,
                &report_id(sequence),
                1,
                BOOT_ID,
                sequence,
                "2026-08-12T09:00:00Z",
                &body,
            )
            .await
            .unwrap();
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            REPORT_COUNT as i64
        );

        // With the Server down the tick fails and every report stays queued.
        assert!(
            deliver_periodic(&mut store, &transport, &policy)
                .await
                .is_err(),
            "delivery against a stopped Server must fail"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            REPORT_COUNT as i64,
            "a failed delivery must keep every report"
        );

        let (restarted, _token) = boot_server(addr, dir.path()).await;
        // One tick must clear the whole backlog: catching up has to be faster
        // than the collection cadence, otherwise the queue never empties.
        let delivered = deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(
            delivered.applied, REPORT_COUNT as usize,
            "one tick must drain the backlog once the Server returns"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0,
            "the Durable Spool must be empty once the backlog is acknowledged"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT dropped_reports FROM spool_state WHERE singleton = 1"
            )
            .fetch_one(store.connection())
            .await
            .unwrap(),
            0,
            "recovery must never discard a queued report"
        );
        let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_report_receipts")
            .fetch_one(restarted.state.db().pool())
            .await
            .unwrap();
        assert_eq!(
            receipts, REPORT_COUNT as i64,
            "the Server must hold one receipt per recovered report"
        );
        store.close().await.unwrap();
    }

    /// Issue #177 acceptance against a real Server: revision 1 is accepted and
    /// recorded; the same revision with a changed Node set refuses the start
    /// with the operator message; bumping to revision 2 declares the new Node
    /// set and delivery resumes. Agent and Server agree because both hash the
    /// published Inventory with the same `platpulse-core` function.
    #[tokio::test]
    async fn inventory_content_change_without_a_bump_is_refused_then_heals() {
        let dir = TempDir::new().unwrap();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);

        let (server, token) = boot_server(addr, dir.path()).await;
        let config = agent_config(dir.path(), &format!("http://{addr}"));
        let enrolled = crate::enroll::enroll_agent(&config, &token).await.unwrap();
        let agent_id = enrolled.agent_id.to_string();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db))
            .await
            .unwrap();
        let transport = HttpReportTransport::from_config(&config).unwrap();
        let policy = SpoolPolicy::default();

        // Revision 1 is accepted by the real Server, and the Agent records what
        // the receipt made effective.
        let declared: AgentReport = serde_json::from_slice(&report_body(1, &agent_id)).unwrap();
        persist_immutable_report(
            &mut store,
            &report_id(1),
            1,
            BOOT_ID,
            1,
            "2026-08-12T09:00:00Z",
            &report_body(1, &agent_id),
        )
        .await
        .unwrap();
        let outcome = deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(outcome.applied, 1);
        assert!(outcome.rejections.is_empty(), "{:?}", outcome.rejections);
        let record = read_inventory_declaration(store.connection())
            .await
            .unwrap()
            .expect("an accepted Inventory must be recorded");
        assert_eq!(record.revision, declared.inventory.revision);
        assert_eq!(
            record.sha256,
            declared.inventory.content_sha256().to_string()
        );

        // The operator edits the Node set and forgets to bump the revision.
        let mut drifted = declared.clone();
        drifted.inventory.nodes[0].rpc_endpoint = "ws://127.0.0.1:6799".parse().unwrap();
        assert_eq!(
            drifted.inventory.revision, declared.inventory.revision,
            "the drift is a content change at the accepted revision"
        );

        // The startup guard refuses it, so the Agent never declares — and
        // therefore never reports — the conflicting Inventory.
        let refused = guard_startup_inventory_declaration(
            &config,
            &drifted.inventory,
            AgentStoreWritePermit::new(),
        )
        .await
        .expect_err("changed content at the accepted revision must refuse the start");
        let message = match &refused {
            CollectionError::InventoryDeclaration(
                InventoryDeclarationConflict::ContentChanged { revision, .. },
            ) => {
                assert_eq!(*revision, declared.inventory.revision);
                refused.to_string()
            }
            other => panic!("unexpected refusal: {other:?}"),
        };
        assert_eq!(
            message,
            "inventory content changed but inventory_revision is still 1; bump inventory_revision (e.g. to 2) to declare the new Node set"
        );

        // Bumping the revision declares the new Node set: the same Server
        // accepts it and the record moves forward.
        let mut bumped: serde_json::Value =
            serde_json::from_slice(&report_body(2, &agent_id)).unwrap();
        bumped["inventory"]["revision"] = serde_json::json!(2);
        bumped["inventory"]["nodes"][0]["rpc_endpoint"] = serde_json::json!("ws://127.0.0.1:6799");
        let bumped_body = serde_json::to_vec(&bumped).unwrap();
        let bumped_inventory: AgentReport = serde_json::from_slice(&bumped_body).unwrap();
        guard_startup_inventory_declaration(
            &config,
            &bumped_inventory.inventory,
            AgentStoreWritePermit::new(),
        )
        .await
        .expect("a bumped revision declares the new Node set");
        persist_immutable_report(
            &mut store,
            &report_id(2),
            1,
            BOOT_ID,
            2,
            "2026-08-12T09:00:00Z",
            &bumped_body,
        )
        .await
        .unwrap();
        let outcome = deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(
            outcome.applied, 1,
            "the bumped declaration must be accepted: {:?}",
            outcome.rejections
        );
        assert!(outcome.rejections.is_empty(), "{:?}", outcome.rejections);
        let record = read_inventory_declaration(store.connection())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.revision, 2);
        assert_eq!(
            record.sha256,
            bumped_inventory.inventory.content_sha256().to_string()
        );

        store.close().await.unwrap();
        server.stop().await;
    }

    const V2_NODE_A: &str = "0195f2a1-0014-4014-8014-000000000014";
    const V2_NODE_B: &str = "0195f2a1-0015-4015-8015-000000000015";

    /// A v2 report body built from the frozen v2 fixture. The declaration lists
    /// the Nodes as (node_id, rpc_endpoint, display_name); the observation view
    /// is generated to match, so the report is a complete current view.
    fn v2_report_body(
        sequence: u64,
        agent_id: &str,
        agent_epoch: u64,
        nodes: &[(&str, &str, Option<&str>)],
    ) -> Vec<u8> {
        let mut report: serde_json::Value = serde_json::from_str(include_str!(
            "../../platpulse-core/tests/fixtures/report_v2_minimal.json"
        ))
        .unwrap();
        let template_node = report["inventory"]["nodes"][0].clone();
        let template_observation = report["nodes"][0].clone();
        let mut inventory_nodes = Vec::new();
        let mut observations = Vec::new();
        for (node_id, endpoint, display_name) in nodes {
            let mut node = template_node.clone();
            node["node_id"] = serde_json::json!(node_id);
            node["rpc_endpoint"] = serde_json::json!(endpoint);
            match display_name {
                Some(name) => node["display_name"] = serde_json::json!(name),
                None => {
                    node.as_object_mut().unwrap().remove("display_name");
                }
            }
            inventory_nodes.push(node);
            let mut observation = template_observation.clone();
            observation["node_id"] = serde_json::json!(node_id);
            observations.push(observation);
        }
        report["agent_id"] = serde_json::json!(agent_id);
        report["agent_epoch"] = serde_json::json!(agent_epoch);
        report["report_sequence"] = serde_json::json!(sequence);
        report["report_id"] = serde_json::json!(format!("0195f2a1-0100-4000-8000-{sequence:012}"));
        report["inventory"]["nodes"] = serde_json::Value::Array(inventory_nodes);
        report["nodes"] = serde_json::Value::Array(observations);
        serde_json::to_vec(&report).unwrap()
    }

    async fn server_inventory_revision(server: &RunningServer, agent_id: &str) -> i64 {
        sqlx::query_scalar("SELECT last_inventory_revision FROM agents WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_one(server.state.db().pool())
            .await
            .unwrap()
    }

    /// The v2 closed loop across the real Server, production enrollment, HTTP
    /// transport, and Agent receipt application: first acceptance, identical
    /// content, Node reorder, field change, A -> B -> A, empty Inventory, and a
    /// rejected report that allocates nothing.
    #[tokio::test]
    async fn v2_server_managed_inventory_revisions_follow_content_and_never_regress() {
        let dir = TempDir::new().unwrap();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let (server, token) = boot_server(addr, dir.path()).await;
        let config = agent_config(dir.path(), &format!("http://{addr}"));
        let enrolled = crate::enroll::enroll_agent(&config, &token).await.unwrap();
        let agent_id = enrolled.agent_id.to_string();

        let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db))
            .await
            .unwrap();
        let transport = HttpReportTransport::from_config(&config).unwrap();
        let policy = SpoolPolicy::default();

        let validator_a = (V2_NODE_A, "ws://127.0.0.1:6790", Some("Validator A"));
        let validator_b = (V2_NODE_B, "ws://127.0.0.1:6791", None);

        // First acceptance: the Server assigns revision 1.
        let body = v2_report_body(1, &agent_id, 1, &[validator_a]);
        persist_immutable_report(
            &mut store,
            &report_id(1),
            1,
            BOOT_ID,
            1,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        let outcome = deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(outcome.applied, 1, "{:?}", outcome.rejections);
        assert!(outcome.rejections.is_empty(), "{:?}", outcome.rejections);
        let record = read_inventory_declaration(store.connection())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.revision, 1);
        assert_eq!(server_inventory_revision(&server, &agent_id).await, 1);
        let server_hash: Option<String> =
            sqlx::query_scalar("SELECT inventory_sha256 FROM agents WHERE agent_id = ?")
                .bind(&agent_id)
                .fetch_one(server.state.db().pool())
                .await
                .unwrap();
        assert_eq!(
            record.sha256,
            server_hash.expect("the accepted declaration fingerprint is stored"),
            "the Agent confirmation record must match the Server's accepted fingerprint"
        );

        // Identical content keeps the accepted revision.
        let body = v2_report_body(2, &agent_id, 1, &[validator_a]);
        persist_immutable_report(
            &mut store,
            &report_id(2),
            1,
            BOOT_ID,
            2,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        let outcome = deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(outcome.applied, 1, "{:?}", outcome.rejections);
        assert_eq!(server_inventory_revision(&server, &agent_id).await, 1);

        // A new member is a content change: revision 2.
        let body = v2_report_body(3, &agent_id, 1, &[validator_a, validator_b]);
        persist_immutable_report(
            &mut store,
            &report_id(3),
            1,
            BOOT_ID,
            3,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(server_inventory_revision(&server, &agent_id).await, 2);

        // Node order alone is not a declaration change: unchanged, revision 2.
        let body = v2_report_body(4, &agent_id, 1, &[validator_b, validator_a]);
        persist_immutable_report(
            &mut store,
            &report_id(4),
            1,
            BOOT_ID,
            4,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(server_inventory_revision(&server, &agent_id).await, 2);

        // A declared field change advances the revision.
        let changed_a = (V2_NODE_A, "ws://127.0.0.1:6799", Some("Validator A"));
        let body = v2_report_body(5, &agent_id, 1, &[changed_a, validator_b]);
        persist_immutable_report(
            &mut store,
            &report_id(5),
            1,
            BOOT_ID,
            5,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(server_inventory_revision(&server, &agent_id).await, 3);

        // A -> B -> A allocates a fresh revision; it never reuses the old number.
        let body = v2_report_body(6, &agent_id, 1, &[validator_a, validator_b]);
        persist_immutable_report(
            &mut store,
            &report_id(6),
            1,
            BOOT_ID,
            6,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(server_inventory_revision(&server, &agent_id).await, 4);

        // An accepted empty Inventory is distinct from an uninitialized Agent.
        let body = v2_report_body(7, &agent_id, 1, &[]);
        persist_immutable_report(
            &mut store,
            &report_id(7),
            1,
            BOOT_ID,
            7,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(server_inventory_revision(&server, &agent_id).await, 5);
        let record = read_inventory_declaration(store.connection())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.revision, 5);

        // A rejected report allocates nothing and never advances the record.
        let body = v2_report_body(8, &agent_id, 0, &[validator_a]);
        persist_immutable_report(
            &mut store,
            &report_id(8),
            0,
            BOOT_ID,
            8,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        let outcome = deliver_periodic(&mut store, &transport, &policy)
            .await
            .unwrap();
        assert_eq!(outcome.applied, 1);
        assert_eq!(outcome.rejections.len(), 1, "{:?}", outcome.rejections);
        assert_eq!(server_inventory_revision(&server, &agent_id).await, 5);
        let record = read_inventory_declaration(store.connection())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.revision, 5);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0,
            "an applied rejection still removes the acknowledged report"
        );

        store.close().await.unwrap();
        server.stop().await;
    }
}
