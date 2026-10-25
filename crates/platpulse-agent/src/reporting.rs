use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};
use sqlx::{Connection, Sqlite, Transaction};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use thiserror::Error;

use platpulse_core::hex::Sha256Hex;
use platpulse_core::identity::ReportId;
use platpulse_core::{
    AgentReport, BootTransition, InventoryDeclaration, InventoryDisposition, ReceiptDisposition,
    ReportReceipt, ReportReceiptV2, Rfc3339,
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

/// The one projection every queued-Report integrity check reads, so a check can
/// never validate a row shape another check does not see.
const SPOOL_REPORT_PROJECTION: &str = "SELECT report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes FROM reports";

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
    #[cfg(test)]
    let body_reads = store.spool_body_read_probe();
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
            #[cfg(test)]
            if report.is_some() {
                body_reads.record(1);
            }
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

/// What bounds one HTTP send (issue #270).
///
/// The Durable Spool work that precedes a send — taking the shared write permit,
/// claiming the oldest Report, re-validating its bytes — is deliberately outside
/// the budget. `sender_deadline_ms` is defined as the time the Agent waits for
/// the Server's response (design §7.2), so local bookkeeping must never be able
/// to consume it and expire it before the request leaves the Agent.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SendBudget {
    /// Abandon the send once this much time has passed since the request
    /// started. The Durable Spool delivery pass renews it for every Report.
    PerSend(std::time::Duration),
    /// Abandon the send at a moment the caller already chose: shutdown and
    /// upgrade preparation bound a whole step, which starts before the request.
    At(tokio::time::Instant),
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
    Ok(
        deliver_one_typed(store, transport, Some(SendBudget::At(send_deadline)))
            .await?
            .map(|step| step.report),
    )
}

/// Send one claimed report under a deadline and report both the immutable
/// report and the Server's top-level disposition. The upgrade-preparation
/// bridge (issue #189) must distinguish an accepted/partially accepted Closing
/// from a rejected one without inferring it from later Agent state.
pub(crate) async fn deliver_one_with_disposition_and_send_deadline<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    send_deadline: tokio::time::Instant,
) -> Result<
    Option<(
        StoredReport,
        ReceiptDisposition,
        Option<InventoryDisposition>,
    )>,
    ReportStoreError,
> {
    Ok(
        deliver_one_typed(store, transport, Some(SendBudget::At(send_deadline)))
            .await?
            .map(|step| (step.report, step.disposition, step.inventory)),
    )
}

/// One applied report, with the operator-facing rejection summary when the
/// Server refused it whole.
struct DeliveryStep {
    report: StoredReport,
    rejection: Option<String>,
    disposition: ReceiptDisposition,
    /// The Server's Inventory outcome for this report, kept so a caller that
    /// must prove the declaration was accepted (the upgrade-preparation
    /// bridge, issue #189) does not have to infer it from later state.
    inventory: Option<InventoryDisposition>,
}

async fn deliver_one_typed<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    send_budget: Option<SendBudget>,
) -> Result<Option<DeliveryStep>, ReportStoreError> {
    ensure_spool_not_fatal(store).await?;
    let Some(report) = claim_oldest_report(store).await? else {
        return Ok(None);
    };
    ensure_claimed_report_intact(store, &report).await?;
    // The per-send budget starts here, with the request, not with the Durable
    // Spool bookkeeping above (issue #270).
    let send_deadline = match send_budget {
        Some(SendBudget::PerSend(timeout)) => Some(tokio::time::Instant::now() + timeout),
        Some(SendBudget::At(deadline)) => Some(deadline),
        None => None,
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
    // Copy the Inventory outcome before the receipt is consumed by the
    // receipt-application transaction below.
    let inventory_disposition = envelope_receipt.inventory;
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
        inventory: inventory_disposition,
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
    ensure_spool_not_fatal(store).await?;
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
    enforce_stored_spool_policy(store, generated_at).await?;
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

/// Why one delivery pass stopped (issue #270).
///
/// A pass is not bounded by the collection cadence: it keeps sending while the
/// Durable Spool still holds a queued Report. The stop reason is what lets the
/// caller tell "production and delivery are level again" apart from "the pass
/// failed and the queue may still hold Reports".
#[derive(Debug)]
pub enum DeliveryStop {
    /// No queued Report is left, so production and delivery are level again.
    SpoolEmpty,
    /// The pass stopped on an error; `outcome` still counts every receipt
    /// applied before it.
    Failed(ReportStoreError),
    /// The caller asked the pass to stop between two Reports.
    Cancelled,
}

/// What one delivery pass did and why it stopped (issue #270).
#[derive(Debug)]
pub struct DeliveryPass {
    /// Receipts applied before the pass stopped, including the ones applied
    /// before a later send in the same pass failed.
    pub outcome: DeliveryOutcome,
    /// The reason the pass returned.
    pub stop: DeliveryStop,
}

/// Deliver queued Reports oldest-first, one at a time, until the spool is empty.
///
/// A single queued report is the steady state: collection produced it and the
/// next tick delivers it. Anything beyond that is a backlog (a Server restart, a
/// slow receipt), and a backlog must drain faster than the collection cadence,
/// otherwise delivery exactly cancels collection and the queue never empties:
/// every later report then waits behind it and the spool drifts toward its
/// overflow limit (issue #137).
///
/// The pass therefore stops when the queue is empty, when a send or a receipt
/// fails, or when the caller cancels — never because the next collection tick is
/// due (issue #270). Catch-up throughput follows the Server's ingest rate
/// instead of the tick rate, so a backlog whose Report takes longer to ingest
/// than one tick still drains.
///
/// `send_timeout` bounds one HTTP send, not the pass: a timed-out Report is
/// abandoned to the next pass while every receipt already applied in this one
/// still counts. Its clock starts when the request starts, so spool bookkeeping
/// never eats into it.
///
/// `on_rejection` runs as each refused receipt is applied, so a rejection seen
/// early in a long catch-up reaches the operator immediately instead of after
/// the queue drains.
pub(crate) async fn deliver_backlog_with_send_timeout<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    send_timeout: std::time::Duration,
    cancel: &tokio_util::sync::CancellationToken,
    on_rejection: impl FnMut(&str),
) -> DeliveryPass {
    deliver_backlog(store, transport, Some(send_timeout), cancel, on_rejection).await
}

/// Deliver until the Agent Store holds no queued Report (issue #270).
pub async fn deliver_periodic<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
) -> Result<DeliveryOutcome, ReportStoreError> {
    let pass = deliver_backlog(
        store,
        transport,
        None,
        &tokio_util::sync::CancellationToken::new(),
        |_| {},
    )
    .await;
    match pass.stop {
        DeliveryStop::Failed(error) => Err(error),
        DeliveryStop::SpoolEmpty | DeliveryStop::Cancelled => Ok(pass.outcome),
    }
}

async fn deliver_backlog<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    send_timeout: Option<std::time::Duration>,
    cancel: &tokio_util::sync::CancellationToken,
    mut on_rejection: impl FnMut(&str),
) -> DeliveryPass {
    let mut outcome = DeliveryOutcome::default();
    loop {
        // Cancellation is checked between two Reports, so a shutdown never
        // abandons one mid-receipt; a single send stays bounded by
        // `send_timeout`.
        if cancel.is_cancelled() {
            return DeliveryPass {
                outcome,
                stop: DeliveryStop::Cancelled,
            };
        }
        match deliver_one_typed(store, transport, send_timeout.map(SendBudget::PerSend)).await {
            Ok(Some(step)) => {
                outcome.applied += 1;
                if let Some(rejection) = step.rejection {
                    on_rejection(&rejection);
                    outcome.rejections.push(rejection);
                }
            }
            Ok(None) => {
                return DeliveryPass {
                    outcome,
                    stop: DeliveryStop::SpoolEmpty,
                };
            }
            Err(error) => {
                return DeliveryPass {
                    outcome,
                    stop: DeliveryStop::Failed(error),
                };
            }
        }
    }
}

/// The whole-spool integrity boundary, run once while the Agent boots.
///
/// ADR 0001 makes start-up the place where queue integrity is established: every
/// queued Report is streamed, hashed and re-parsed here, and a queue that fails
/// leaves the Store fatal so no worker ever starts against it. Runtime paths
/// call `ensure_spool_not_fatal` instead.
///
/// Hashing and parsing happen outside the write permit (ADR 0001 keeps them
/// there), the scan is streamed one Report at a time so a large Durable Spool
/// costs a stream instead of a full fetch, and a Report that fails the scan is
/// re-read under the permit before the Store goes fatal, so no fatal write is
/// decided by a read taken outside it.
pub async fn ensure_spool_healthy(store: &mut AgentStore) -> Result<(), ReportStoreError> {
    ensure_spool_not_fatal(store).await?;
    #[cfg(test)]
    let body_reads = store.spool_body_read_probe();
    let scan = format!("{SPOOL_REPORT_PROJECTION} ORDER BY created_at, report_id");
    let mut rows = sqlx::query_as::<_, SpoolReportRow>(&scan).fetch(store.connection());
    while let Some(row) = rows.try_next().await? {
        #[cfg(test)]
        body_reads.record(1);
        if let Some(reason) = validate_spool_report(&row) {
            drop(rows);
            let _write_permit = store.acquire_write().await;
            return latch_spool_defect_under_permit(store, &row, &reason).await;
        }
    }
    drop(rows);
    // The collision scan is a read that decides a fatal write, so it keeps the
    // write permit through the mark (ADR 0001).
    let _write_permit = store.acquire_write().await;
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

/// Refuse new collection or delivery when durable spool corruption was
/// latched earlier, without re-reading the queued bodies.
///
/// ensure_spool_healthy hashes and re-parses every queued Report, so calling it
/// on the hot path makes a backlog drain quadratic: with a few thousand queued
/// Reports the Agent spent its sender deadline rescanning the spool instead of
/// sending, and the drain slowed down as the spool grew. Corruption detection
/// therefore stays at startup (validate_receipt_history and the collector open
/// paths) and in the per-Report gate ensure_claimed_report_intact, while the
/// runtime paths only read the flag those checks latch.
pub async fn ensure_spool_not_fatal(store: &mut AgentStore) -> Result<(), ReportStoreError> {
    let state: Option<(i64, Option<String>)> =
        sqlx::query_as("SELECT store_fatal, store_error FROM spool_state WHERE singleton=1")
            .fetch_optional(store.connection())
            .await?;
    let Some((fatal, message)) = state else {
        return Ok(());
    };
    if fatal == 0 {
        return Ok(());
    }
    Err(ReportStoreError::StoreFatal(message.unwrap_or_else(|| {
        "Agent Store is marked fatal; manual recovery is required".to_owned()
    })))
}

/// Validate the one Report a delivery is about to send.
///
/// This keeps the safety property the full spool scan used to enforce -- the
/// body still hashes to its stored digest, still parses as the Report it claims
/// to be, and no Applied Receipt Record already covers it -- at O(1) instead of
/// O(queued Reports).
async fn ensure_claimed_report_intact(
    store: &mut AgentStore,
    report: &StoredReport,
) -> Result<(), ReportStoreError> {
    // The claim already copied this body under the permit, so hashing and
    // parsing it again here stays outside the permit (ADR 0001 keeps parsing
    // out of it), and the permit is only re-taken to latch a defect or to read
    // the Applied Receipt Record the decision to send depends on.
    let row = sqlx::query_as::<_, SpoolReportRow>(&format!(
        "{SPOOL_REPORT_PROJECTION} WHERE report_id = ?"
    ))
    .bind(&report.report_id)
    .fetch_optional(store.connection())
    .await?;
    #[cfg(test)]
    if row.is_some() {
        store.record_spool_body_read(1);
    }
    let suspect = row.and_then(|row| {
        let reason = validate_spool_report(&row).or_else(|| {
            (row.body != report.body || row.body_sha256 != report.body_sha256).then(|| {
                format!(
                    "immutable spool report {} changed after it was claimed",
                    report.report_id
                )
            })
        })?;
        Some((row, reason))
    });
    let _write_permit = store.acquire_write().await;
    if let Some((suspect, reason)) = suspect {
        return latch_spool_defect_under_permit(store, &suspect, &reason).await;
    }
    let collision: Option<String> = sqlx::query_scalar(
        "SELECT receipt.report_id FROM report_receipts AS receipt WHERE receipt.report_id = ? LIMIT 1",
    )
    .bind(&report.report_id)
    .fetch_optional(store.connection())
    .await?;
    if collision.is_some() {
        let reason = format!(
            "queued Agent Report {} conflicts with an Applied Receipt Record",
            report.report_id
        );
        mark_spool_fatal_under_permit(store, &now_rfc3339(), &reason).await?;
        return Err(ReportStoreError::StoreFatal(reason));
    }
    Ok(())
}

/// Latch a queued-Report defect that an outside-permit read already found.
///
/// The write permit must span the read that authorizes the write (ADR 0001), and
/// parsing stays outside it, so this never re-validates: it re-reads the row under
/// the permit and compares it byte for byte with the exact snapshot that failed
/// validation. Identical bytes make that snapshot the authoritative row, and only
/// then may the Store go fatal.
///
/// A Report that is gone by then (an applied receipt or a policy drop removed it)
/// has no queued body left to condemn, so nothing is latched. A Report whose bytes
/// changed is the impossible state ADR 0001 fails closed on: nothing rewrites a
/// queued body.
async fn latch_spool_defect_under_permit(
    store: &mut AgentStore,
    suspect: &SpoolReportRow,
    reason: &str,
) -> Result<(), ReportStoreError> {
    let row = sqlx::query_as::<_, SpoolReportRow>(&format!(
        "{SPOOL_REPORT_PROJECTION} WHERE report_id = ?"
    ))
    .bind(&suspect.report_id)
    .fetch_optional(store.connection())
    .await?;
    #[cfg(test)]
    if row.is_some() {
        store.record_spool_body_read(1);
    }
    let reason = match row {
        None => return Ok(()),
        Some(row) if same_spool_row(&row, suspect) => reason.to_owned(),
        Some(_) => format!(
            "queued Agent Report {} changed while its integrity was being confirmed",
            suspect.report_id
        ),
    };
    mark_spool_fatal_under_permit(store, &now_rfc3339(), &reason).await?;
    Err(ReportStoreError::StoreFatal(reason))
}

/// Whether two reads describe the same immutable queued Report.
fn same_spool_row(left: &SpoolReportRow, right: &SpoolReportRow) -> bool {
    left.report_id == right.report_id
        && left.agent_epoch == right.agent_epoch
        && left.boot_id == right.boot_id
        && left.report_sequence == right.report_sequence
        && left.generated_at == right.generated_at
        && left.body_bytes == right.body_bytes
        && left.body_sha256 == right.body_sha256
        && left.body == right.body
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

/// Record the configured Spool policy in the Agent Store.
///
/// The stored columns are the single source of truth for enforcement, the
/// host.spool diagnostics, and the Admin evidence panel: a configured capacity
/// that only lived in the configuration file would make the Agent drop reports
/// against a bound no surface ever showed.
pub async fn apply_spool_policy(
    store: &mut AgentStore,
    policy: &SpoolPolicy,
    now: &str,
) -> Result<(), ReportStoreError> {
    let _write_permit = store.acquire_write().await;
    sqlx::query(
        "UPDATE spool_state SET max_bytes=?, max_age_seconds=?, preflush_bytes=?, updated_at=? WHERE singleton=1",
    )
    .bind(policy.max_bytes.min(i64::MAX as u64) as i64)
    .bind(policy.max_age_seconds.min(i64::MAX as u64) as i64)
    .bind(policy.preflush_bytes.min(i64::MAX as u64) as i64)
    .bind(now)
    .execute(store.connection())
    .await?;
    Ok(())
}

/// The Spool policy currently recorded in the Agent Store.
pub async fn stored_spool_policy(store: &mut AgentStore) -> Result<SpoolPolicy, ReportStoreError> {
    let (max_bytes, max_age_seconds, preflush_bytes): (i64, i64, i64) = sqlx::query_as(
        "SELECT max_bytes, max_age_seconds, preflush_bytes FROM spool_state WHERE singleton=1",
    )
    .fetch_one(store.connection())
    .await?;
    Ok(SpoolPolicy {
        max_bytes: max_bytes.max(0) as u64,
        max_age_seconds: max_age_seconds.max(0) as u64,
        preflush_bytes: preflush_bytes.max(0) as u64,
    })
}

/// Enforce the policy recorded in the Agent Store rather than a compiled-in
/// default, so the configured capacity is what actually governs retention.
pub async fn enforce_stored_spool_policy(
    store: &mut AgentStore,
    now: &str,
) -> Result<SpoolCleanupSummary, ReportStoreError> {
    let policy = stored_spool_policy(store).await?;
    enforce_spool_policy(store, &policy, now).await
}

/// One Report the Spool policy drops, validated while no write permit was held.
struct PlannedSpoolDrop {
    row: SpoolReportRow,
    sample_count: u64,
    height_range: Option<(u64, u64)>,
}

/// What one cleanup pass decided: the Reports it drops, or one corrupt Report it
/// must not drop at all.
struct SpoolCleanupPlan {
    drops: Vec<PlannedSpoolDrop>,
    defect: Option<(SpoolReportRow, String)>,
}

/// Apply a bounded, transactional spool policy. In-flight and newest complete
/// current report are never deleted; each dropped report contributes bounded
/// loss diagnostics without manufacturing Agent-side history.
pub async fn enforce_spool_policy(
    store: &mut AgentStore,
    policy: &SpoolPolicy,
    now: &str,
) -> Result<SpoolCleanupSummary, ReportStoreError> {
    ensure_spool_not_fatal(store).await?;

    let cutoff = time::OffsetDateTime::parse(now, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|value| value - time::Duration::seconds(policy.max_age_seconds as i64));
    let plan = plan_spool_cleanup(store, policy, cutoff).await?;
    if let Some((suspect, reason)) = plan.defect {
        let _write_permit = store.acquire_write().await;
        return match latch_spool_defect_under_permit(store, &suspect, &reason).await {
            // An applied receipt removed the Report between the two reads, so the
            // pass that would have dropped it has nothing left to drop.
            Ok(()) => Ok(SpoolCleanupSummary::default()),
            Err(error) => Err(error),
        };
    }
    if plan.drops.is_empty() {
        return Ok(SpoolCleanupSummary::default());
    }

    #[cfg(test)]
    let body_reads = store.spool_body_read_probe();
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    let mut defect: Option<String> = None;
    let transaction_result: Result<SpoolCleanupSummary, ReportStoreError> = async {
        let mut summary = SpoolCleanupSummary::default();
        for planned in plan.drops {
            // Re-read under the permit and compare against the snapshot that was
            // validated outside it. Nothing here hashes or parses a body: the
            // bytes were already proven intact, and this only proves they are
            // still the bytes queued (ADR 0001, issue #269).
            let row: Option<SpoolReportRow> = sqlx::query_as(&format!(
                "{SPOOL_REPORT_PROJECTION} WHERE report_id = ?"
            ))
            .bind(&planned.row.report_id)
            .fetch_optional(&mut *tx)
            .await?;
            #[cfg(test)]
            if row.is_some() {
                body_reads.record(1);
            }
            let Some(row) = row else {
                // An applied receipt deleted it between the two reads.
                continue;
            };
            if !same_spool_row(&row, &planned.row) {
                let reason = format!(
                    "queued Agent Report {} changed while its integrity was being confirmed",
                    planned.row.report_id
                );
                defect = Some(reason.clone());
                return Err(ReportStoreError::StoreFatal(reason));
            }
            let deleted = sqlx::query("DELETE FROM reports WHERE report_id = ? AND in_flight = 0")
                .bind(&planned.row.report_id)
                .execute(&mut *tx)
                .await?;
            if deleted.rows_affected() == 0 {
                continue;
            }
            let sequence = planned.row.report_sequence as u64;
            let generated_at = planned.row.generated_at;
            summary.dropped_reports += 1;
            summary.sequence_range = Some(
                summary
                    .sequence_range
                    .map_or((sequence, sequence), |(from, to)| {
                        (from.min(sequence), to.max(sequence))
                    }),
            );
            summary.time_range = Some(summary.time_range.take().map_or(
                (generated_at.clone(), generated_at.clone()),
                |(from, to)| (from.min(generated_at.clone()), to.max(generated_at.clone())),
            ));
            summary.dropped_samples += planned.sample_count;
            if let Some(range) = planned.height_range {
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
            match defect {
                Some(reason) => {
                    mark_spool_fatal_under_permit(store, now, &reason).await?;
                    Err(ReportStoreError::StoreFatal(reason))
                }
                None => Err(error),
            }
        }
    }
}

/// Decide what one cleanup pass drops, validating every Report it selects.
///
/// This is the half that hashes and parses a body, so it runs outside the write
/// permit (ADR 0001), and it reads a body only for a Report the policy is about
/// to drop, so the steady state still reads none. The permit pass that follows
/// only compares snapshots and deletes.
async fn plan_spool_cleanup(
    store: &mut AgentStore,
    policy: &SpoolPolicy,
    cutoff: Option<time::OffsetDateTime>,
) -> Result<SpoolCleanupPlan, ReportStoreError> {
    #[cfg(test)]
    let body_reads = store.spool_body_read_probe();
    let rows: Vec<(String, i64, String, i64, i64)> = sqlx::query_as(
        "SELECT report_id, report_sequence, generated_at, body_bytes, in_flight FROM reports ORDER BY created_at, report_id",
    )
    .fetch_all(store.connection())
    .await?;
    let total: i64 = rows
        .iter()
        .map(|(_, _, _, body_bytes, _)| (*body_bytes).max(0))
        .sum();
    let current_report_id: Option<String> = sqlx::query_scalar(
        "SELECT r.report_id FROM reports r JOIN agent_state s ON s.singleton = 1 AND s.agent_id IS NOT NULL AND s.agent_epoch = r.agent_epoch AND s.boot_id = r.boot_id AND s.report_sequence = r.report_sequence ORDER BY r.created_at DESC, r.report_id DESC LIMIT 1",
    )
    .fetch_optional(store.connection())
    .await?
    .or(
        sqlx::query_scalar(
            "SELECT report_id FROM reports ORDER BY agent_epoch DESC, report_sequence DESC, created_at DESC, report_id DESC LIMIT 1",
        )
        .fetch_optional(store.connection())
        .await?,
    );
    let mut plan = SpoolCleanupPlan {
        drops: Vec::new(),
        defect: None,
    };
    let mut bytes = total;
    let max_bytes = policy.max_bytes.min(i64::MAX as u64) as i64;
    for (report_id, _sequence, generated_at, body_bytes, in_flight) in rows {
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
        if in_flight == 1 || current_report_id.as_deref() == Some(report_id.as_str()) {
            continue;
        }
        let row: Option<SpoolReportRow> =
            sqlx::query_as(&format!("{SPOOL_REPORT_PROJECTION} WHERE report_id = ?"))
                .bind(&report_id)
                .fetch_optional(store.connection())
                .await?;
        let Some(row) = row else {
            continue;
        };
        #[cfg(test)]
        body_reads.record(1);
        // Discarding is the other way a Report leaves the Durable Spool, so it
        // validates first exactly like delivery does (issue #269): corruption is
        // never quietly deleted, it latches the Store fatal.
        if let Some(reason) = validate_spool_report(&row) {
            plan.defect = Some((row, reason));
            return Ok(plan);
        }
        let (sample_count, height_range) = spool_report_loss_shape(&row.body)?;
        bytes -= body_bytes.max(0);
        plan.drops.push(PlannedSpoolDrop {
            row,
            sample_count,
            height_range,
        });
    }
    Ok(plan)
}

/// What one dropped Report contributes to the bounded loss diagnostics: how many
/// samples it carried and the height range those samples covered.
fn spool_report_loss_shape(body: &[u8]) -> Result<(u64, Option<(u64, u64)>), ReportStoreError> {
    let (_, block_summaries, history_gaps) = parse_spooled_report(body)?;
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
    Ok((sample_count, height_range))
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

pub(crate) async fn mark_spool_fatal_under_permit(
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
        apply_spool_policy(
            &mut store,
            &config.spool.policy(),
            &crate::database::now_rfc3339(),
        )
        .await?;
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
    apply_spool_policy(
        &mut store,
        &config.spool.policy(),
        &crate::database::now_rfc3339(),
    )
    .await?;
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

/// Builders for a large, still-valid Durable Spool backlog.
///
/// Issue #269 is about what integrity work *costs*, not about whether it is
/// correct, so the tests that guard it need a backlog large enough that a
/// per-Report rescan is unmistakable (thousands of Reports, ~100 MiB) while
/// every queued body is still a Report the protocol accepts. A padded body is
/// built from the one bounded, repeatable wire element that can inflate a
/// single Report: valid History Gaps.
#[cfg(test)]
pub(crate) mod spool_backlog_fixture {
    use super::*;

    const PADDED_GAP_COUNT: u64 = 60;
    const PADDED_GAP_REASON_CHARS: usize = 480;

    /// A Report identity that sorts in sequence order, so "the oldest queued
    /// Report" is predictable without depending on timestamps or a clock.
    pub(crate) fn report_id(sequence: u64) -> String {
        format!("0195f2a1-0015-4015-8015-{sequence:012x}")
    }

    pub(crate) fn body_digest(body: &[u8]) -> String {
        format!("0x{}", hex::encode(Sha256::digest(body)))
    }

    /// A valid v1 Report of roughly 40 KiB, owned by `boot_id` and stamped
    /// `generated_at`, so it survives both the protocol validator and the
    /// Spool age bound.
    pub(crate) fn report_body(boot_id: &str, generated_at: &str, sequence: u64) -> Vec<u8> {
        let mut report: serde_json::Value = serde_json::from_str(include_str!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report["report_id"] = serde_json::Value::String(report_id(sequence));
        report["boot_id"] = serde_json::Value::String(boot_id.to_owned());
        report["report_sequence"] = serde_json::json!(sequence);
        report["generated_at"] = serde_json::Value::String(generated_at.to_owned());
        let reason = "recoverable backfill exceeded the bounded height window".repeat(9);
        let gaps: Vec<serde_json::Value> = (0..PADDED_GAP_COUNT)
            .map(|gap| {
                serde_json::json!({
                    "node_id": "0195f2a1-0014-4014-8014-000000000014",
                    "kind": "unrecoverable_backfill",
                    "from_height": gap * 1_000 + 1,
                    "to_height": gap * 1_000 + 1_000,
                    "reason": reason.chars().take(PADDED_GAP_REASON_CHARS).collect::<String>(),
                    "recorded_at": generated_at,
                })
            })
            .collect();
        report["history_gaps"] = serde_json::Value::Array(gaps);
        serde_json::to_vec(&report).unwrap()
    }

    /// The accepted receipt the Server would answer a seeded Report with: every
    /// Node accepted and every padded History Gap covered by an accepted sample,
    /// which is what the apply path requires before it deletes the Report.
    pub(crate) fn receipt_body(report_id: &str, body: &[u8]) -> Vec<u8> {
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
        let samples = report
            .history_gaps
            .iter()
            .map(|gap| {
                serde_json::json!({
                    "node_id": gap.node_id,
                    "sample": {
                        "kind": "gap",
                        "from_height": gap.from_height,
                        "to_height": gap.to_height
                    },
                    "disposition": "accepted"
                })
            })
            .collect::<Vec<_>>();
        let receipt = serde_json::json!({"report_id": report_id, "disposition": "accepted", "report_body_sha256": body_digest(body), "server_version": "0.1.0", "supported_protocol_majors": [1], "server_time": "2026-01-01T00:00:00Z", "inventory": "accepted", "rejections": [], "nodes": nodes, "samples": samples});
        serde_json::to_vec(&serde_json::json!({"receipt": receipt})).unwrap()
    }

    /// Queue `reports` valid Reports in one transaction and return the queued
    /// byte total, so a test can state how large the backlog it asserts against
    /// actually is instead of trusting a comment.
    pub(crate) async fn seed(
        store: &mut AgentStore,
        boot_id: &str,
        generated_at: &str,
        first_sequence: u64,
        reports: u64,
    ) -> u64 {
        let _write_permit = store.acquire_write().await;
        let mut tx = store.connection().begin().await.unwrap();
        let mut queued_bytes = 0_u64;
        for sequence in first_sequence..first_sequence + reports {
            let body = report_body(boot_id, generated_at, sequence);
            queued_bytes += body.len() as u64;
            sqlx::query(
                "INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, in_flight, created_at) VALUES (?, 1, ?, ?, ?, ?, ?, ?, 0, ?)",
            )
            .bind(report_id(sequence))
            .bind(boot_id)
            .bind(sequence as i64)
            .bind(generated_at)
            .bind(&body)
            .bind(body_digest(&body))
            .bind(body.len() as i64)
            .bind(generated_at)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        queued_bytes
    }
}

/// Issue #270: catch-up has to be a property of the Durable Spool, not of the
/// collection cadence.
///
/// The delivery loop ticks once per second, and before #270 a pass delivered at
/// most eight Reports per tick behind one absolute deadline shared by the whole
/// tick. Field measurement puts one Report at ~1.3 s of Server ingest against a
/// 1 s tick, so delivery ran at the collection cadence, an outage backlog never
/// drained, and the shared write permit was spent on a queue that could not
/// shrink. A pass now drains the queue instead: it stops on an empty queue, on an
/// error, or on cancellation, and sender_deadline_ms bounds one HTTP send.
#[cfg(test)]
mod catch_up_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tempfile::{TempDir, tempdir};
    use tokio::sync::{Notify, watch};
    use tokio_util::sync::CancellationToken;

    use crate::database::AgentStoreWritePermit;

    const BOOT_ID: &str = "0195f2a1-0016-4016-8016-000000000016";

    async fn store_with_permit(
        database: &std::path::Path,
        permit: &AgentStoreWritePermit,
    ) -> AgentStore {
        AgentStore::open_with_write_permit(AgentDatabaseConfig::new(database), permit.clone())
            .await
            .unwrap()
    }

    /// A real temporary Agent Store holding `reports` queued Reports, plus the
    /// permit and the directory its deliveries share. The TempDir comes back so
    /// the caller keeps it alive for the whole test.
    async fn seeded_store(reports: u64) -> (TempDir, AgentStoreWritePermit, AgentStore) {
        let directory = tempdir().unwrap();
        let permit = AgentStoreWritePermit::new();
        let mut store = store_with_permit(&directory.path().join("agent.db"), &permit).await;
        spool_backlog_fixture::seed(&mut store, BOOT_ID, &now_rfc3339(), 1, reports).await;
        (directory, permit, store)
    }

    /// The Report identity a queued body carries, so a transport can answer with
    /// the receipt the Server would send for exactly that Report.
    fn queued_report_id(body: &[u8]) -> String {
        let report: serde_json::Value = serde_json::from_slice(body).unwrap();
        report["report_id"].as_str().unwrap().to_owned()
    }

    async fn spooled_reports(store: &mut AgentStore) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM reports")
            .fetch_one(store.connection())
            .await
            .unwrap()
    }

    async fn queued_reports(store: &mut AgentStore) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM reports WHERE in_flight = 0")
            .fetch_one(store.connection())
            .await
            .unwrap()
    }

    /// What one delivery costs the Agent Store on this machine with no Server in
    /// the loop, measured instead of assumed, so a scaled outage budget tracks the
    /// machine that runs the test rather than a constant that only holds on the
    /// author's.
    async fn store_floor(sample: u64) -> Duration {
        let (_directory, _permit, mut store) = seeded_store(sample).await;
        let transport = IngestTransport::new(Duration::ZERO);
        let started = Instant::now();
        let outcome = deliver_periodic(&mut store, &transport).await.unwrap();
        let elapsed = started.elapsed();
        store.close().await.unwrap();
        assert_eq!(outcome.applied as u64, sample);
        elapsed / sample as u32
    }

    /// A Report the collection side may persist while another Report is queued.
    /// The padded fixture Reports all claim the same History Gaps, and a claim
    /// only succeeds against gap rows the Agent Store already holds, so this one
    /// carries no samples: the permit probe needs a real persistence through the
    /// collection path, not the sample bookkeeping.
    fn collection_report_body(sequence: u64) -> Vec<u8> {
        let body = spool_backlog_fixture::report_body(BOOT_ID, &now_rfc3339(), sequence);
        let mut report: serde_json::Value = serde_json::from_slice(&body).unwrap();
        report["history_gaps"] = serde_json::json!([]);
        serde_json::to_vec(&report).unwrap()
    }

    /// The Server side of a catch-up: every send costs ingest, and from the
    /// stall_from-th send on the Server stops answering for stall, which is what
    /// a send deadline has to cut off. Sends are recorded so a test can state
    /// what the pass actually did instead of inferring it.
    struct IngestTransport {
        ingest: Duration,
        stall_from: Option<usize>,
        stall: Duration,
        sent: Arc<Mutex<Vec<String>>>,
    }

    impl IngestTransport {
        fn new(ingest: Duration) -> Self {
            Self {
                ingest,
                stall_from: None,
                stall: Duration::ZERO,
                sent: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn sent_ids(&self) -> Vec<String> {
            self.sent.lock().unwrap().clone()
        }
    }

    impl ReportTransport for IngestTransport {
        fn send<'a>(
            &'a self,
            body: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ReportStoreError>> + Send + 'a>> {
            let report_id = queued_report_id(body);
            let index = {
                let mut sent = self.sent.lock().unwrap();
                sent.push(report_id.clone());
                sent.len()
            };
            let delay = match self.stall_from {
                Some(stall_from) if index >= stall_from => self.stall,
                _ => self.ingest,
            };
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                Ok(spool_backlog_fixture::receipt_body(&report_id, body))
            })
        }
    }

    /// A transport whose sends park until the test opens the gate, so a test can
    /// look at what the pass holds while a send is in flight.
    struct GatedTransport {
        gate: watch::Receiver<bool>,
        started: Arc<Notify>,
        sent: Arc<Mutex<Vec<String>>>,
    }

    impl ReportTransport for GatedTransport {
        fn send<'a>(
            &'a self,
            body: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ReportStoreError>> + Send + 'a>> {
            let report_id = queued_report_id(body);
            self.sent.lock().unwrap().push(report_id.clone());
            self.started.notify_one();
            let mut gate = self.gate.clone();
            Box::pin(async move {
                let _ = gate.wait_for(|open| *open).await;
                Ok(spool_backlog_fixture::receipt_body(&report_id, body))
            })
        }
    }

    /// A Server that refuses one Report whole, and notes how many refusals the
    /// pass had already reported to its caller when the third send began.
    struct RefusingTransport {
        refuse: String,
        sent: std::sync::atomic::AtomicUsize,
        reported: Arc<Mutex<Vec<String>>>,
        reported_by_third_send: Arc<Mutex<Option<usize>>>,
    }

    impl ReportTransport for RefusingTransport {
        fn send<'a>(
            &'a self,
            body: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ReportStoreError>> + Send + 'a>> {
            let report_id = queued_report_id(body);
            let index = self.sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if index == 3 {
                *self.reported_by_third_send.lock().unwrap() =
                    Some(self.reported.lock().unwrap().len());
            }
            let refused = report_id == self.refuse;
            Box::pin(async move {
                if refused {
                    Ok(super::delivery_tests::rejected_receipt_body(
                        &report_id, body,
                    ))
                } else {
                    Ok(spool_backlog_fixture::receipt_body(&report_id, body))
                }
            })
        }
    }

    /// The pass is not capped: 64 queued Reports leave the spool in one pass with
    /// no ticker involved at all, oldest first. The pre-#270 pass stopped at
    /// eight and needed eight ticks for the same queue.
    #[tokio::test]
    async fn one_pass_drains_a_backlog_without_waiting_for_a_tick() {
        const BACKLOG: u64 = 64;
        let (_directory, _permit, mut store) = seeded_store(BACKLOG).await;
        let transport = IngestTransport::new(Duration::ZERO);

        let outcome = deliver_periodic(&mut store, &transport).await.unwrap();

        assert_eq!(
            outcome.applied as u64, BACKLOG,
            "one pass must drain the whole queue, not a batch of it"
        );
        assert_eq!(spooled_reports(&mut store).await, 0);
        let sent = transport.sent_ids();
        assert_eq!(sent.len() as u64, BACKLOG);
        assert_eq!(
            sent.first().map(String::as_str),
            Some(spool_backlog_fixture::report_id(1).as_str()),
            "the oldest queued Report goes first"
        );
        assert_eq!(
            sent.last().map(String::as_str),
            Some(spool_backlog_fixture::report_id(BACKLOG).as_str())
        );
    }

    /// Acceptance: a single send timeout must not lose the receipts already
    /// applied in the same round. The pass reports how far it got and stops, and
    /// the Report whose send timed out stays in the spool for the next pass.
    #[tokio::test]
    async fn a_send_timeout_keeps_the_receipts_already_applied_in_the_pass() {
        let (_directory, _permit, mut store) = seeded_store(5).await;
        let mut transport = IngestTransport::new(Duration::from_millis(1));
        transport.stall_from = Some(3);
        transport.stall = Duration::from_secs(10);

        let pass = deliver_backlog_with_send_timeout(
            &mut store,
            &transport,
            Duration::from_millis(100),
            &CancellationToken::new(),
            |_| {},
        )
        .await;

        assert_eq!(
            pass.outcome.applied, 2,
            "the two receipts applied before the stalled send must still count"
        );
        assert!(
            matches!(
                pass.stop,
                DeliveryStop::Failed(ReportStoreError::DeliveryDeadline)
            ),
            "the pass must stop on the send deadline, not carry on: {:?}",
            pass.stop
        );
        assert_eq!(
            transport.sent_ids().len(),
            3,
            "the timed-out send is not retried within the pass"
        );
        assert_eq!(
            spooled_reports(&mut store).await,
            3,
            "no Report is lost when one send times out"
        );
        assert_eq!(
            queued_reports(&mut store).await,
            2,
            "the timed-out Report stays claimed for the next pass and the two behind it stay queued"
        );
    }

    /// sender_deadline_ms bounds one HTTP send, not the pass: five 60 ms sends
    /// overrun a 100 ms budget together, yet each of them fits its own budget and
    /// every receipt is applied. Sharing one deadline across the pass would stop
    /// after the first two.
    #[tokio::test]
    async fn the_send_deadline_bounds_each_send_not_the_whole_pass() {
        const BACKLOG: u64 = 5;
        let (_directory, _permit, mut store) = seeded_store(BACKLOG).await;
        let transport = IngestTransport::new(Duration::from_millis(60));

        let pass = deliver_backlog_with_send_timeout(
            &mut store,
            &transport,
            Duration::from_millis(100),
            &CancellationToken::new(),
            |_| {},
        )
        .await;

        assert_eq!(
            pass.outcome.applied as u64, BACKLOG,
            "each send fits its own 100 ms budget, so the pass must deliver all five"
        );
        assert!(matches!(pass.stop, DeliveryStop::SpoolEmpty));
        assert_eq!(spooled_reports(&mut store).await, 0);
    }

    /// A refusal reaches the caller while the pass is still draining, not when it
    /// returns: the second Report is refused whole and the third send only starts
    /// once that refusal has been reported. Without it a refusal seen early in a
    /// long catch-up would stay invisible until the queue emptied, which
    /// design logging.md forbids.
    #[tokio::test]
    async fn a_refusal_is_reported_as_its_receipt_is_applied() {
        const BACKLOG: u64 = 3;
        let (_directory, _permit, mut store) = seeded_store(BACKLOG).await;

        let reported = Arc::new(Mutex::new(Vec::<String>::new()));
        let reported_by_third_send = Arc::new(Mutex::new(None));
        let transport = RefusingTransport {
            refuse: spool_backlog_fixture::report_id(2),
            sent: std::sync::atomic::AtomicUsize::new(0),
            reported: Arc::clone(&reported),
            reported_by_third_send: Arc::clone(&reported_by_third_send),
        };

        let pass = deliver_backlog_with_send_timeout(
            &mut store,
            &transport,
            Duration::from_secs(5),
            &CancellationToken::new(),
            |rejection| reported.lock().unwrap().push(rejection.to_owned()),
        )
        .await;

        assert_eq!(
            pass.outcome.applied as u64, BACKLOG,
            "a whole-report refusal is still an applied receipt"
        );
        assert_eq!(
            pass.outcome.rejections.len(),
            1,
            "exactly one Report is refused"
        );
        assert_eq!(
            *reported_by_third_send.lock().unwrap(),
            Some(1),
            "the refusal must be reported before the next send starts, not after the pass"
        );
        {
            let reported = reported.lock().unwrap();
            assert_eq!(
                reported.len(),
                1,
                "the caller sees the refusal once: {reported:?}"
            );
            assert!(
                reported[0].contains(&spool_backlog_fixture::report_id(2)),
                "the reported refusal must name the refused Report: {reported:?}"
            );
        }
        assert!(matches!(pass.stop, DeliveryStop::SpoolEmpty));
        assert_eq!(spooled_reports(&mut store).await, 0);
    }

    /// Issue #270 acceptance with time scaled proportionally, so the scenario
    /// fits a test run: 6,000 queued Reports (~200 MiB, roughly two hours of
    /// collection at the default interval) drain through a Server that spends
    /// INGEST per Report. The issue allows 60 minutes for a 300 ms ingest, twice
    /// the pure ingest time; the budget here keeps that ratio at twice the sum of
    /// the modelled Server time and the measured local Agent Store cost, so it
    /// holds on a machine slower than the one that wrote it. A pass that waits for
    /// the next 1 s tick, or that delivers one Report per tick, needs 6,000 s.
    #[tokio::test]
    async fn catch_up_drains_six_thousand_reports_within_the_scaled_outage_budget() {
        const BACKLOG: u64 = 6_000;
        const INGEST: Duration = Duration::from_millis(5);
        const TICK: Duration = Duration::from_secs(1);
        const SEND_TIMEOUT: Duration = Duration::from_secs(5);
        // The default collection_interval_seconds, which sets the steady-state
        // production rate catch-up has to beat.
        const COLLECTION_INTERVAL: Duration = Duration::from_secs(5);
        const CALIBRATION: u64 = 60;
        let floor = store_floor(CALIBRATION).await;
        let budget = (INGEST + floor) * (BACKLOG as u32) * 2;
        let hard_limit = budget * 2;

        let directory = tempdir().unwrap();
        let database = directory.path().join("agent.db");
        let permit = AgentStoreWritePermit::new();
        let mut store = store_with_permit(&database, &permit).await;
        let mut sampler = store_with_permit(&database, &permit).await;
        let queued_bytes =
            spool_backlog_fixture::seed(&mut store, BOOT_ID, &now_rfc3339(), 1, BACKLOG).await;
        assert!(
            queued_bytes > 200 * 1024 * 1024,
            "the guarded backlog must be a realistic outage, got {queued_bytes} bytes"
        );

        // The queue depth is measured from a second Agent Store connection on the
        // same database, so "the backlog shrinks" is observed and not inferred.
        let sampling = tokio::spawn(async move {
            let mut depths = Vec::new();
            let mut sample = tokio::time::interval(Duration::from_millis(100));
            loop {
                sample.tick().await;
                let depth = spooled_reports(&mut sampler).await;
                depths.push(depth);
                if depth == 0 {
                    break;
                }
            }
            sampler.close().await.unwrap();
            depths
        });

        let transport = Arc::new(IngestTransport::new(INGEST));
        let cancel = CancellationToken::new();
        let (applied, elapsed, sent) = tokio::time::timeout(hard_limit, async {
            let started = Instant::now();
            let mut tick = tokio::time::interval(TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut applied = 0;
            loop {
                tick.tick().await;
                let pass = deliver_backlog_with_send_timeout(
                    &mut store,
                    &*transport,
                    SEND_TIMEOUT,
                    &cancel,
                    |_| {},
                )
                .await;
                let DeliveryPass { outcome, stop } = pass;
                applied += outcome.applied;
                match stop {
                    DeliveryStop::SpoolEmpty if applied > 0 => break,
                    DeliveryStop::Failed(error) => panic!("catch-up must not fail: {error}"),
                    _ => {}
                }
            }
            (applied, started.elapsed(), transport.sent_ids())
        })
        .await
        .expect("the catch-up must drain the backlog instead of stalling on it");

        assert_eq!(applied as u64, BACKLOG);
        assert_eq!(
            sent.len() as u64,
            BACKLOG,
            "every Report goes out exactly once"
        );
        assert_eq!(spooled_reports(&mut store).await, 0);
        assert!(
            elapsed <= budget,
            "the backlog must drain within {budget:?}, twice the scaled Server ingest time plus the measured {floor:?} Agent Store floor, took {elapsed:?}",
        );
        assert!(
            elapsed < TICK * (BACKLOG as u32),
            "catch-up must not wait for a collection tick per Report, took {elapsed:?}",
        );
        // Acceptance: catch-up throughput stays at least three times the
        // steady-state production rate. The default collection_interval_seconds
        // is 5, so collection produces 0.2 Reports/s and the floor is 0.6.
        let production_rate = 1.0 / COLLECTION_INTERVAL.as_secs_f64();
        let catch_up_rate = BACKLOG as f64 / elapsed.as_secs_f64();
        assert!(
            catch_up_rate >= 3.0 * production_rate,
            "catch-up delivered {catch_up_rate:.2} Reports/s, below three times the {production_rate:.2} Reports/s steady-state production rate",
        );

        let depths = sampling.await.unwrap();
        assert!(
            depths.len() >= 2,
            "the progress of the queue must be sampled"
        );
        assert_eq!(depths[0], BACKLOG as i64);
        assert_eq!(
            *depths.last().unwrap(),
            0,
            "the backlog must be gone, not just smaller"
        );
        // Two samples can land inside one delivery pass under load, so the
        // sampled depth repeats instead of falling by the pass size. What the
        // test is about is that the backlog never grows while it drains: a
        // strictly falling sequence would make passing the test a question of
        // sampler timing rather than of catch-up behaviour.
        assert!(
            depths.windows(2).all(|pair| pair[1] <= pair[0]),
            "the backlog must never grow while it drains, saw {depths:?}"
        );
        store.close().await.unwrap();
    }

    /// The invariant that makes catch-up safe: a pass holds the shared write
    /// permit only around its own Agent Store transactions, never across the HTTP
    /// send. A collection tick that runs while a send is in flight must still be
    /// able to write, which is what keeps the collection cadence independent of
    /// how deep the backlog is.
    #[tokio::test]
    async fn a_pass_never_holds_the_write_permit_across_a_send() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("agent.db");
        let permit = AgentStoreWritePermit::new();
        let mut delivery_store = store_with_permit(&database, &permit).await;
        let mut collection_store = store_with_permit(&database, &permit).await;
        spool_backlog_fixture::seed(&mut delivery_store, BOOT_ID, &now_rfc3339(), 1, 1).await;

        let (release, gate) = watch::channel(false);
        let transport = Arc::new(GatedTransport {
            gate,
            started: Arc::new(Notify::new()),
            sent: Arc::new(Mutex::new(Vec::new())),
        });
        let started = Arc::clone(&transport.started);
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let transport = Arc::clone(&transport);
            let cancel = cancel.clone();
            async move {
                deliver_backlog_with_send_timeout(
                    &mut delivery_store,
                    &*transport,
                    Duration::from_secs(5),
                    &cancel,
                    |_| {},
                )
                .await
            }
        });

        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .expect("the pass must reach its first send");

        let body = collection_report_body(2);
        tokio::time::timeout(
            Duration::from_secs(5),
            persist_immutable_report(
                &mut collection_store,
                &spool_backlog_fixture::report_id(2),
                1,
                BOOT_ID,
                2,
                &now_rfc3339(),
                &body,
            ),
        )
        .await
        .expect("a send in flight must not hold the shared write permit")
        .unwrap();
        assert_eq!(
            spooled_reports(&mut collection_store).await,
            2,
            "the collection-side write must reach the spool while the send is open"
        );

        release.send(true).unwrap();
        let pass = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            pass.outcome.applied, 2,
            "the pass must pick up the Report the collection side wrote during the send"
        );
        assert!(matches!(pass.stop, DeliveryStop::SpoolEmpty));
        assert_eq!(
            spooled_reports(&mut collection_store).await,
            0,
            "both Reports must leave the spool"
        );
        collection_store.close().await.unwrap();
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
    pub(super) fn rejected_receipt_body(report_id: &str, body: &[u8]) -> Vec<u8> {
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
    ///
    /// Issue #270 removed the threshold's last delivery-side role: the pass
    /// drains until the queue is empty, so five small reports leave the spool in
    /// one pass without any spool policy being consulted at all.
    #[tokio::test]
    async fn a_backlog_of_several_small_reports_is_drained_in_one_pass() {
        let mut store = test_store().await;
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
        let transport = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(responses)),
        };

        let outcome = deliver_periodic(&mut store, &transport).await.unwrap();
        assert_eq!(
            outcome.applied, 5,
            "a backlog must be drained in one pass, not one report per pass"
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
            deliver_periodic(&mut store, &transport)
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

    /// Issue #269: discarding is the other way a Report leaves the Durable Spool,
    /// so cleanup validates before it deletes. A Report corrupted after start-up
    /// must latch the Store fatal instead of disappearing with the overflow.
    #[tokio::test]
    async fn spool_overflow_marks_a_corrupt_report_fatal_instead_of_dropping_it() {
        let mut store = test_store().await;
        for sequence in 1..=2 {
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
        let mut truncated = report_body(1);
        truncated.truncate(truncated.len() / 2);
        sqlx::query("UPDATE reports SET body = ? WHERE report_id = ?")
            .bind(&truncated)
            .bind(report_id(1))
            .execute(store.connection())
            .await
            .unwrap();

        let error = enforce_spool_policy(
            &mut store,
            &SpoolPolicy {
                max_bytes: body_size,
                max_age_seconds: 365 * 24 * 60 * 60,
                preflush_bytes: body_size,
            },
            "2026-08-12T10:00:00Z",
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, ReportStoreError::StoreFatal(_)),
            "cleanup must fail closed on a corrupt Report, got {error:?}"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT store_fatal FROM spool_state WHERE singleton = 1")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            2,
            "a corrupt Report must not be discarded"
        );
        store.close().await.unwrap();
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

    /// The compiled-in policy is only a default: the recorded configuration is
    /// what drops reports and what every diagnostics surface reports as
    /// capacity.
    #[tokio::test]
    async fn configured_spool_policy_governs_enforcement_and_diagnostics() {
        let mut store = test_store().await;
        let body_size = report_body(1).len() as u64;
        let policy = SpoolPolicy {
            max_bytes: body_size * 2,
            max_age_seconds: 365 * 24 * 60 * 60,
            preflush_bytes: body_size,
        };
        apply_spool_policy(&mut store, &policy, "2026-08-12T09:00:00Z")
            .await
            .unwrap();
        assert_eq!(stored_spool_policy(&mut store).await.unwrap(), policy);

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

        // Persisting enforces the recorded capacity, not the shipped default.
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            2
        );
        let diagnostics = crate::collector::current_spool_diagnostics(&mut store)
            .await
            .unwrap();
        assert_eq!(diagnostics.capacity_bytes, Some(body_size * 2));
        assert_eq!(
            diagnostics.max_age_seconds,
            Some(365 * 24 * 60 * 60),
            "diagnostics must describe the configured bounds"
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

    async fn store_fatal_flag(store: &mut AgentStore) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT store_fatal FROM spool_state WHERE singleton = 1")
            .fetch_one(store.connection())
            .await
            .unwrap()
    }

    async fn queue_one_report(store: &mut AgentStore, sequence: u64) -> (String, Vec<u8>) {
        let id = report_id(sequence);
        let body = report_body(sequence);
        persist_immutable_report(
            store,
            &id,
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            sequence,
            "2026-08-12T09:00:00Z",
            &body,
        )
        .await
        .unwrap();
        (id, body)
    }

    /// Issue #269: a queue whose integrity does not hold is a start-up failure.
    /// The whole spool is validated once while the Agent boots, and that
    /// validation latches the Store fatal, so the delivery loop never runs
    /// against a queue it cannot trust.
    #[tokio::test]
    async fn startup_marks_a_corrupt_queued_report_fatal() {
        let mut store = test_store().await;
        let (id, _body) = queue_one_report(&mut store, 5).await;
        sqlx::query("UPDATE reports SET body_sha256 = ? WHERE report_id = ?")
            .bind("0x0000000000000000000000000000000000000000000000000000000000000000")
            .bind(&id)
            .execute(store.connection())
            .await
            .unwrap();

        assert!(matches!(
            validate_receipt_history(&mut store).await,
            Err(ReportStoreError::StoreFatal(_))
        ));
        assert_eq!(store_fatal_flag(&mut store).await, 1);
    }

    /// Issue #269 acceptance: the delivery hot path validates the one Report it
    /// claimed — still before the send, still latched fatal — so a body that
    /// rotted on disk is caught without the whole spool being re-read.
    #[tokio::test]
    async fn corrupt_claimed_report_is_marked_fatal_before_it_is_sent() {
        let mut store = test_store().await;
        let (id, body) = queue_one_report(&mut store, 5).await;
        sqlx::query("UPDATE reports SET body = ? WHERE report_id = ?")
            .bind(b"{\"report_id\":\"truncated\"}".to_vec())
            .bind(&id)
            .execute(store.connection())
            .await
            .unwrap();
        let transport = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(vec![Ok(receipt_body(&id, &body))])),
        };

        assert!(matches!(
            deliver_one(&mut store, &transport).await,
            Err(ReportStoreError::StoreFatal(_))
        ));
        assert!(
            transport.bodies.lock().unwrap().is_empty(),
            "a corrupt Report must never reach the Server"
        );
        assert_eq!(store_fatal_flag(&mut store).await, 1);

        // The latch holds: the next delivery refuses before it touches the spool.
        let reads = store.spool_body_read_probe();
        reads.reset();
        assert!(matches!(
            deliver_one(&mut store, &transport).await,
            Err(ReportStoreError::StoreFatal(_))
        ));
        assert_eq!(reads.reads(), 0);
    }

    /// Issue #269: the same store status was previously only re-derived from a
    /// whole-spool scan at every delivery; a claimed Report that also carries an
    /// Applied Receipt Record is still refused, and nothing is sent or deleted.
    #[tokio::test]
    async fn claimed_report_with_an_applied_receipt_is_refused_before_it_is_sent() {
        let mut store = test_store().await;
        let (id, body) = queue_one_report(&mut store, 6).await;
        sqlx::query("INSERT INTO report_receipts (report_id, report_body_sha256, disposition, applied_at) VALUES (?, ?, 'accepted', ?)")
            .bind(&id)
            .bind(format!("0x{}", hex::encode(Sha256::digest(&body))))
            .bind("2026-08-12T10:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        let transport = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(Vec::new())),
        };

        assert!(matches!(
            deliver_one(&mut store, &transport).await,
            Err(ReportStoreError::StoreFatal(_))
        ));
        assert!(transport.bodies.lock().unwrap().is_empty());
        assert_eq!(store_fatal_flag(&mut store).await, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id = ?")
                .bind(&id)
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1,
            "a conflicting Report is kept for manual recovery"
        );
    }

    /// Issue #269: without the per-delivery whole-spool scan, an integrity
    /// failure found while applying a receipt has to latch the Store fatal.
    /// Returning a plain database error would roll back forever: the Report
    /// stays queued, so every retry recomputes the same failure.
    #[tokio::test]
    async fn apply_marks_a_corrupt_report_fatal_instead_of_retrying_forever() {
        let mut store = test_store().await;
        let (id, body) = queue_one_report(&mut store, 8).await;
        let receipt = receipt_body(&id, &body);
        let digest = format!("0x{}", hex::encode(Sha256::digest(&body)));
        sqlx::query("UPDATE reports SET body = ? WHERE report_id = ?")
            .bind(b"{\"report_id\":\"truncated\"}".to_vec())
            .bind(&id)
            .execute(store.connection())
            .await
            .unwrap();

        assert!(
            crate::collector::apply_receipt(
                &mut store,
                &id,
                &digest,
                "accepted",
                &receipt,
                "2026-08-12T09:00:01Z",
            )
            .await
            .is_err()
        );
        assert_eq!(store_fatal_flag(&mut store).await, 1);
        let transport = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(Vec::new())),
        };
        assert!(matches!(
            deliver_one(&mut store, &transport).await,
            Err(ReportStoreError::StoreFatal(_))
        ));
        assert!(transport.bodies.lock().unwrap().is_empty());
    }

    /// Issue #269 acceptance: with thousands of Reports and ~100 MiB queued, one
    /// delivery reads a constant number of Report bodies — the one it claims,
    /// the one it re-checks, and the one it applies. The assertion is a read
    /// count, not a duration, so it cannot be satisfied by a faster machine.
    #[tokio::test]
    async fn backlogged_delivery_reads_a_constant_number_of_spool_bodies() {
        const BACKLOG: u64 = 3_000;
        const BOOT_ID: &str = "0195f2a1-0041-4041-8041-000000000041";
        let mut store = test_store().await;
        let generated_at = now_rfc3339();
        let queued_bytes =
            spool_backlog_fixture::seed(&mut store, BOOT_ID, &generated_at, 1, BACKLOG).await;
        assert!(
            queued_bytes > 100 * 1024 * 1024,
            "the guarded backlog must be realistically large, got {queued_bytes} bytes"
        );

        // The start-up boundary is where the whole spool is paid for.
        ensure_spool_healthy(&mut store).await.unwrap();

        let id = spool_backlog_fixture::report_id(1);
        let transport = FakeTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(vec![Ok(spool_backlog_fixture::receipt_body(
                &id,
                &spool_backlog_fixture::report_body(BOOT_ID, &generated_at, 1),
            ))])),
        };
        let probe = store.spool_body_read_probe();
        probe.reset();
        let delivered = deliver_one(&mut store, &transport).await.unwrap();
        assert_eq!(delivered.map(|report| report.report_id), Some(id));
        assert!(
            probe.reads() <= 3,
            "one delivery must read only the Reports it touches, read {} bodies",
            probe.reads()
        );
        assert_eq!(transport.bodies.lock().unwrap().len(), 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            BACKLOG as i64 - 1
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

        let outcome = deliver_periodic(&mut store, &transport).await.unwrap();
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
    use std::fs;
    use std::future::Future;
    use std::path::Path;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};

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

    use super::{
        AgentReport, HttpReportTransport, ReportStoreError, ReportTransport, deliver_periodic,
        persist_immutable_report,
    };
    use crate::collector::{CollectionError, guard_startup_inventory_declaration};
    use crate::config::{AgentConfig, BackfillConfig, SpoolConfig};
    use crate::database::{AgentDatabaseConfig, AgentStore, AgentStoreWritePermit};
    use crate::http_test_support::ReservedAddress;
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

    /// A real HTTP transport that discards the response of its first successful
    /// send, simulating a Server that committed a report but whose receipt never
    /// reached the Agent (issue #187).
    struct LostResponseTransport {
        inner: HttpReportTransport,
        drop_next: AtomicBool,
    }

    impl ReportTransport for LostResponseTransport {
        fn send<'a>(
            &'a self,
            body: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ReportStoreError>> + Send + 'a>> {
            Box::pin(async move {
                let response = self.inner.send(body).await?;
                if self.drop_next.swap(false, Ordering::SeqCst) {
                    return Err(ReportStoreError::Delivery(
                        "simulated lost receipt".to_owned(),
                    ));
                }
                Ok(response)
            })
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
            spool: SpoolConfig::default(),
        }
    }

    /// Boot a real Server on the reserved listener from the shared state
    /// directory.
    async fn boot_server(listener: tokio::net::TcpListener, dir: &Path) -> (RunningServer, String) {
        let addr = listener.local_addr().unwrap();
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
        let address = ReservedAddress::reserve();
        let addr = address.addr();

        let (server, token) = boot_server(address.listener().await, dir.path()).await;
        let config = agent_config(dir.path(), &format!("http://{addr}"));
        let enrolled = crate::enroll::enroll_agent(&config, &token).await.unwrap();
        let agent_id = enrolled.agent_id.to_string();
        server.stop().await;

        let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db))
            .await
            .unwrap();
        let transport = HttpReportTransport::from_config(&config).unwrap();
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
            deliver_periodic(&mut store, &transport).await.is_err(),
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

        let (restarted, _token) = boot_server(address.listener().await, dir.path()).await;
        // One tick must clear the whole backlog: catching up has to be faster
        // than the collection cadence, otherwise the queue never empties.
        let delivered = deliver_periodic(&mut store, &transport).await.unwrap();
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
        let address = ReservedAddress::reserve();
        let addr = address.addr();

        let (server, token) = boot_server(address.listener().await, dir.path()).await;
        let config = agent_config(dir.path(), &format!("http://{addr}"));
        let enrolled = crate::enroll::enroll_agent(&config, &token).await.unwrap();
        let agent_id = enrolled.agent_id.to_string();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db))
            .await
            .unwrap();
        let transport = HttpReportTransport::from_config(&config).unwrap();

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
        let outcome = deliver_periodic(&mut store, &transport).await.unwrap();
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
        let outcome = deliver_periodic(&mut store, &transport).await.unwrap();
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
        let address = ReservedAddress::reserve();
        let addr = address.addr();
        let (server, token) = boot_server(address.listener().await, dir.path()).await;
        let config = agent_config(dir.path(), &format!("http://{addr}"));
        let enrolled = crate::enroll::enroll_agent(&config, &token).await.unwrap();
        let agent_id = enrolled.agent_id.to_string();

        let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db))
            .await
            .unwrap();
        let transport = HttpReportTransport::from_config(&config).unwrap();

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
        let outcome = deliver_periodic(&mut store, &transport).await.unwrap();
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
        let outcome = deliver_periodic(&mut store, &transport).await.unwrap();
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
        deliver_periodic(&mut store, &transport).await.unwrap();
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
        deliver_periodic(&mut store, &transport).await.unwrap();
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
        deliver_periodic(&mut store, &transport).await.unwrap();
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
        deliver_periodic(&mut store, &transport).await.unwrap();
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
        deliver_periodic(&mut store, &transport).await.unwrap();
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
        let outcome = deliver_periodic(&mut store, &transport).await.unwrap();
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

    /// Issue #187: a Server-managed Agent keeps generating and persisting new
    /// immutable Reports through the normal configuration/persistence entry while
    /// the Server is offline. No confirmation record exists yet, and the missing
    /// confirmation is never treated as permission or as a blocker. Once the
    /// Server returns, the backlog drains and the Server assigns a fresh revision
    /// per accepted content change, without rewriting the original bytes.
    #[tokio::test]
    async fn a_v2_agent_declares_while_the_server_is_offline_and_drains_after_restart() {
        let dir = TempDir::new().unwrap();
        let address = ReservedAddress::reserve();
        let addr = address.addr();

        let (server, token) = boot_server(address.listener().await, dir.path()).await;
        let config = agent_config(dir.path(), &format!("http://{addr}"));
        let enrolled = crate::enroll::enroll_agent(&config, &token).await.unwrap();
        let agent_id = enrolled.agent_id.to_string();
        server.stop().await;

        // The Server is down: three content changes are still declared and
        // spooled through the production persistence path, with no local
        // revision and no confirmation record to consult.
        let config_path = dir.path().join("agent.toml");
        let report_path = dir.path().join("report.json");
        let endpoints = [
            "ws://127.0.0.1:6790",
            "ws://127.0.0.1:6791",
            "ws://127.0.0.1:6792",
        ];
        for (index, endpoint) in endpoints.iter().enumerate() {
            let sequence = index as u64 + 1;
            fs::write(
                &report_path,
                v2_report_body(sequence, &agent_id, 1, &[(V2_NODE_A, endpoint, None)]),
            )
            .unwrap();
            fs::write(
                &config_path,
                format!(
                    "server_url=\"http://{addr}\"\ncredential_file=\"{}\"\nstate_db=\"{}\"\nnodes=[{{node_id=\"{V2_NODE_A}\",network_key=\"platon-mainnet\",rpc_endpoint=\"{endpoint}\"}}]\n",
                    config.credential_file.display(),
                    config.state_db.display()
                ),
            )
            .unwrap();
            crate::reporting::persist_report_from_config(&config_path, &report_path)
                .await
                .unwrap();
        }

        let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db))
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            3,
            "the offline Server must not stop new Reports from being persisted"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM inventory_declaration")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0,
            "no confirmation record exists while the Server is offline"
        );
        let transport = HttpReportTransport::from_config(&config).unwrap();
        assert!(
            deliver_periodic(&mut store, &transport).await.is_err(),
            "delivery against a stopped Server must fail"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            3,
            "a failed delivery must keep every queued Report"
        );

        let (restarted, _token) = boot_server(address.listener().await, dir.path()).await;
        let delivered = deliver_periodic(&mut store, &transport).await.unwrap();
        assert_eq!(
            delivered.applied, 3,
            "one tick must drain the v2 backlog: {:?}",
            delivered.rejections
        );
        assert!(
            delivered.rejections.is_empty(),
            "{:?}",
            delivered.rejections
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
            server_inventory_revision(&restarted, &agent_id).await,
            3,
            "each accepted content change advances the Server-assigned revision"
        );
        let record = read_inventory_declaration(store.connection())
            .await
            .unwrap()
            .expect("the final accepted declaration is recorded");
        assert_eq!(record.revision, 3);
        let server_hash: Option<String> =
            sqlx::query_scalar("SELECT inventory_sha256 FROM agents WHERE agent_id = ?")
                .bind(&agent_id)
                .fetch_one(restarted.state.db().pool())
                .await
                .unwrap();
        assert_eq!(
            record.sha256,
            server_hash.expect("the accepted declaration fingerprint is stored"),
            "the Agent confirmation record must match the Server's accepted fingerprint"
        );
        store.close().await.unwrap();
        restarted.stop().await;
    }

    /// Issue #187: when the Server committed a Report but its Receipt was lost,
    /// the Agent resends the identical bytes. The Server returns the original
    /// exact Receipt without re-allocating, re-projecting, or counting twice.
    #[tokio::test]
    async fn a_lost_v2_receipt_is_replayed_exactly_without_duplicating_effects() {
        let dir = TempDir::new().unwrap();
        let address = ReservedAddress::reserve();
        let addr = address.addr();
        let (server, token) = boot_server(address.listener().await, dir.path()).await;
        let config = agent_config(dir.path(), &format!("http://{addr}"));
        let enrolled = crate::enroll::enroll_agent(&config, &token).await.unwrap();
        let agent_id = enrolled.agent_id.to_string();

        let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db))
            .await
            .unwrap();
        let body = v2_report_body(1, &agent_id, 1, &[(V2_NODE_A, "ws://127.0.0.1:6790", None)]);
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
        let transport = LostResponseTransport {
            inner: HttpReportTransport::from_config(&config).unwrap(),
            drop_next: AtomicBool::new(true),
        };
        assert!(
            deliver_periodic(&mut store, &transport).await.is_err(),
            "a lost response must surface as a transport failure"
        );
        // The Server committed despite the lost response.
        assert_eq!(server_inventory_revision(&server, &agent_id).await, 1);
        let receipt_body: Vec<u8> = sqlx::query_scalar(
            "SELECT receipt_body FROM agent_report_receipts WHERE report_id = ?",
        )
        .bind(report_id(1))
        .fetch_one(server.state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes WHERE agent_id = ?")
                .bind(&agent_id)
                .fetch_one(server.state.db().pool())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1,
            "the unacknowledged Report stays queued with its exact bytes"
        );

        let delivered = deliver_periodic(&mut store, &transport).await.unwrap();
        assert_eq!(delivered.applied, 1, "{:?}", delivered.rejections);
        assert!(
            delivered.rejections.is_empty(),
            "{:?}",
            delivered.rejections
        );
        assert_eq!(
            server_inventory_revision(&server, &agent_id).await,
            1,
            "a replayed Report must not allocate a new revision"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM agent_report_receipts WHERE report_id = ?"
            )
            .bind(report_id(1))
            .fetch_one(server.state.db().pool())
            .await
            .unwrap(),
            1,
            "the replay must not store a second Receipt"
        );
        let replayed_body: Vec<u8> = sqlx::query_scalar(
            "SELECT receipt_body FROM agent_report_receipts WHERE report_id = ?",
        )
        .bind(report_id(1))
        .fetch_one(server.state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            replayed_body, receipt_body,
            "the stored Receipt must stay byte-for-byte the original"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes WHERE agent_id = ?")
                .bind(&agent_id)
                .fetch_one(server.state.db().pool())
                .await
                .unwrap(),
            1,
            "the replay must not duplicate the projection"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0
        );
        let record = read_inventory_declaration(store.connection())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.revision, 1);
        store.close().await.unwrap();
        server.stop().await;
    }
}
