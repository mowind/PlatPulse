use sha2::{Digest, Sha256};
use sqlx::{Connection, Sqlite, Transaction};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use thiserror::Error;

use platpulse_core::hex::Sha256Hex;
use platpulse_core::identity::ReportId;
use platpulse_core::{AgentReport, BootTransition, ReceiptDisposition, ReportReceipt};

use serde::Deserialize;

use crate::collector::{SpoolCleanupSummary, SpoolPolicy, apply_receipt};
use crate::config::{AgentConfig, AgentConfigError};
use crate::credential::{CredentialError, load_credential_file};
use crate::database::{AgentDatabaseConfig, AgentDatabaseError, AgentStore, now_rfc3339};

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
    credential: String,
}

impl HttpReportTransport {
    pub fn from_config(config: &AgentConfig) -> Result<Self, ReportStoreError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .user_agent(format!("platpulse-agent/{}", crate::VERSION))
                .build()
                .map_err(|error| ReportStoreError::Delivery(error.to_string()))?,
            url: format!("{}/api/agent/v1/reports", config.server_url),
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
            let response = self
                .client
                .post(&self.url)
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

/// Record a bounded delivery failure while preserving the immutable report.
pub async fn record_delivery_failure(
    store: &mut AgentStore,
    message: &str,
    at: &str,
) -> Result<(), ReportStoreError> {
    let message = message.chars().take(256).collect::<String>();
    let _write_permit = store.acquire_write().await;
    sqlx::query("INSERT INTO delivery_diagnostics (singleton, last_error, last_error_at) VALUES (1, ?, ?) ON CONFLICT(singleton) DO UPDATE SET last_error=excluded.last_error, last_error_at=excluded.last_error_at")
        .bind(message).bind(at).execute(store.connection()).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReportResponse {
    receipt: ReportReceipt,
}

/// Send one claimed report. No transport error is an acknowledgement; the
/// in-flight row and exact bytes remain available for the next attempt.
pub async fn deliver_one<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
) -> Result<Option<StoredReport>, ReportStoreError> {
    deliver_one_inner(store, transport, None).await
}

/// Deliver one report while applying a deadline only to the HTTP send. Once
/// the response arrives, receipt validation and its SQLite transaction are
/// allowed to finish without cancellation.
pub async fn deliver_one_with_send_deadline<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    send_deadline: tokio::time::Instant,
) -> Result<Option<StoredReport>, ReportStoreError> {
    deliver_one_inner(store, transport, Some(send_deadline)).await
}

async fn deliver_one_inner<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    send_deadline: Option<tokio::time::Instant>,
) -> Result<Option<StoredReport>, ReportStoreError> {
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
    let envelope: WireReportResponse = serde_json::from_slice(&response)
        .map_err(|error| ReportStoreError::InvalidReceipt(error.to_string()))?;
    envelope
        .receipt
        .validate()
        .map_err(|error| ReportStoreError::InvalidReceipt(error.to_string()))?;
    let actual_hash = format!("0x{}", hex::encode(Sha256::digest(&report.body)));
    if actual_hash != report.body_sha256 {
        return Err(ReportStoreError::ReceiptMismatch);
    }
    if envelope.receipt.report_id.to_string() != report.report_id
        || envelope.receipt.report_body_sha256.to_string() != actual_hash
    {
        return Err(ReportStoreError::ReceiptMismatch);
    }
    let disposition = match envelope.receipt.disposition {
        ReceiptDisposition::Accepted => "accepted",
        ReceiptDisposition::PartiallyAccepted => "partially_accepted",
        ReceiptDisposition::Rejected => "rejected",
    };
    apply_receipt(
        store,
        &report.report_id,
        &report.body_sha256,
        disposition,
        &response,
        &now_rfc3339(),
    )
    .await
    .map_err(ReportStoreError::Database)?;
    Ok(Some(report))
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
    let report = serde_json::from_slice::<AgentReport>(body)
        .map_err(|error| ReportStoreError::InvalidReport(error.to_string()))?;
    let agent_id = report.agent_id.to_string();
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
            &report.block_summaries,
            &report.history_gaps,
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

/// Deliver a bounded amount of oldest-first work. Once the durable spool
/// reaches the preflush threshold, drain a small batch; otherwise keep the
/// periodic worker to one report per tick so collection remains responsive.
pub async fn deliver_periodic<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    policy: &SpoolPolicy,
) -> Result<usize, ReportStoreError> {
    deliver_periodic_inner(store, transport, policy, None).await
}

pub(crate) async fn deliver_periodic_with_send_deadline<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    policy: &SpoolPolicy,
    send_deadline: tokio::time::Instant,
) -> Result<usize, ReportStoreError> {
    deliver_periodic_inner(store, transport, policy, Some(send_deadline)).await
}

async fn deliver_periodic_inner<T: ReportTransport>(
    store: &mut AgentStore,
    transport: &T,
    policy: &SpoolPolicy,
    send_deadline: Option<tokio::time::Instant>,
) -> Result<usize, ReportStoreError> {
    let queued_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(body_bytes), 0) FROM reports WHERE in_flight = 0")
            .fetch_one(store.connection())
            .await?;
    let max_reports = if queued_bytes.max(0) as u64 >= policy.preflush_bytes {
        8
    } else {
        1
    };
    let mut delivered = 0;
    for _ in 0..max_reports {
        let result = match send_deadline {
            Some(deadline) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(ReportStoreError::DeliveryDeadline);
                }
                deliver_one_with_send_deadline(store, transport, deadline).await
            }
            None => deliver_one(store, transport).await,
        }?;
        match result {
            Some(_) => delivered += 1,
            None => break,
        }
    }
    Ok(delivered)
}

/// Refuse new collection/delivery when durable spool corruption was observed.
pub async fn ensure_spool_healthy(store: &mut AgentStore) -> Result<(), ReportStoreError> {
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
        mark_spool_fatal_store(store, &now_rfc3339(), &reason).await?;
        return Err(ReportStoreError::StoreFatal(reason));
    }
    if let Some(reason) = validate_receipt_rows(
        store,
        "SELECT receipt.report_id, receipt.report_body_sha256, receipt.disposition, receipt.applied_at
         FROM reports
         CROSS JOIN report_receipts AS receipt
         WHERE receipt.report_id = reports.report_id
         ORDER BY receipt.applied_at, receipt.report_id",
    )
    .await?
    {
        mark_spool_fatal_store(store, &now_rfc3339(), &reason).await?;
        return Err(ReportStoreError::StoreFatal(reason));
    }
    Ok(())
}

/// Validate only markers joined to queued reports at startup.
///
/// Applied Receipt Records have already received bounded expiry cleanup during
/// Agent Store open and do not otherwise affect runtime decisions. Reading the
/// entire recent retention window here would make startup history-sized work.
pub async fn validate_receipt_history(store: &mut AgentStore) -> Result<(), ReportStoreError> {
    ensure_spool_healthy(store).await
}

async fn validate_receipt_rows(
    store: &mut AgentStore,
    query: &str,
) -> Result<Option<String>, ReportStoreError> {
    let receipts = sqlx::query_as::<_, (String, String, String, String)>(query)
        .fetch_all(store.connection())
        .await?;
    for (report_id, body_sha256, disposition, _applied_at) in receipts {
        let valid_identity = report_id.parse::<ReportId>().is_ok();
        let valid_hash = body_sha256.parse::<Sha256Hex>().is_ok();
        let valid_disposition = matches!(
            disposition.as_str(),
            "accepted" | "partially_accepted" | "rejected"
        );
        if !valid_identity || !valid_hash || !valid_disposition {
            return Ok(Some(format!(
                "stored receipt marker for report {report_id} is invalid"
            )));
        }
    }
    Ok(None)
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
        let report: AgentReport = serde_json::from_slice(&row.body).map_err(|error| {
            ReportStoreError::InvalidReport(format!("stored report is invalid: {error}"))
        })?;
        let mut sample_count = 0u64;
        let mut height_range: Option<(u64, u64)> = None;
        for sample in &report.block_summaries {
            sample_count += 1;
            height_range = Some(
                height_range.map_or((sample.block_number, sample.block_number), |(from, to)| {
                    (from.min(sample.block_number), to.max(sample.block_number))
                }),
            );
        }
        for gap in &report.history_gaps {
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
    let report: AgentReport = match serde_json::from_slice(&row.body) {
        Ok(report) => report,
        Err(error) => {
            return Some(format!(
                "immutable spool report {} is not a valid AgentReport: {error}",
                row.report_id
            ));
        }
    };
    if let Err(error) = report.validate() {
        return Some(format!(
            "immutable spool report {} failed protocol validation: {error}",
            row.report_id
        ));
    }
    if report.report_id.to_string() != row.report_id
        || report.agent_epoch != row.agent_epoch.max(0) as u64
        || report.boot_id.to_string() != row.boot_id
        || report.report_sequence != row.report_sequence.max(0) as u64
        || report.generated_at.to_string() != row.generated_at
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
}
