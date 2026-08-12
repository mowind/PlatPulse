use sha2::{Digest, Sha256};
use sqlx::Connection;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use thiserror::Error;

use platpulse_core::{ReceiptDisposition, ReportReceipt};
use serde::Deserialize;

use crate::collector::apply_receipt;
use crate::config::{AgentConfig, AgentConfigError};
use crate::credential::{CredentialError, load_credential_file};
use crate::database::{AgentDatabaseConfig, AgentDatabaseError, AgentStore};

#[derive(Debug, Error)]
pub enum ReportStoreError {
    #[error("report body is empty")]
    Empty,
    #[error("report body exceeds protocol limit")]
    TooLarge,
    #[error("report is already being delivered")]
    DeliveryInFlight,
    #[error("receipt body is invalid: {0}")]
    InvalidReceipt(String),
    #[error("receipt does not match the stored report")]
    ReceiptMismatch,
    #[error("delivery transport failed: {0}")]
    Delivery(String),
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
    let mut tx = store.connection().begin().await?;
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
            tx.commit().await?;
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
    tx.commit().await?;
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

/// Record a bounded delivery failure while preserving the immutable report.
pub async fn record_delivery_failure(
    store: &mut AgentStore,
    message: &str,
    at: &str,
) -> Result<(), ReportStoreError> {
    let message = message.chars().take(256).collect::<String>();
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
    let Some(report) = claim_oldest_report(store).await? else {
        return Ok(None);
    };
    let response = match transport.send(&report.body).await {
        Ok(response) => response,
        Err(error) => {
            let _ = record_delivery_failure(store, &error.to_string(), &now_rfc3339()).await;
            return Err(error);
        }
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

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("valid timestamp")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("valid timestamp")
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
    if body.is_empty() {
        return Err(ReportStoreError::Empty);
    }
    if body.len() > platpulse_core::protocol::MAX_REPORT_BODY_BYTES {
        return Err(ReportStoreError::TooLarge);
    }
    let digest = format!("0x{}", hex::encode(Sha256::digest(body)));
    let mut tx = store.connection().begin().await?;
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
    tx.commit().await?;
    Ok(digest)
}

/// Validate a configured inventory and persist one immutable report from a
/// file. This is the smallest runtime path used by the Agent CLI before a
/// delivery sender is introduced: configuration is loaded and validated as a
/// whole, the report is revalidated, and the exact bytes are spooled in one
/// SQLite transaction.
pub async fn persist_report_from_config(
    config_path: &Path,
    report_path: &Path,
) -> Result<String, ReportStoreError> {
    let config = AgentConfig::resolve(config_path)?;
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
    let mut store = AgentStore::open(AgentDatabaseConfig::new(&config.state_db)).await?;
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

    fn receipt_body(report_id: &str, body: &[u8]) -> Vec<u8> {
        let receipt = serde_json::json!({"report_id": report_id, "disposition": "accepted", "report_body_sha256": format!("0x{}", hex::encode(Sha256::digest(body))), "server_version": "0.1.0", "supported_protocol_majors": [1], "server_time": "2026-01-01T00:00:00Z", "inventory": "accepted", "rejections": [], "nodes": [], "samples": []});
        serde_json::to_vec(&serde_json::json!({"receipt": receipt})).unwrap()
    }

    async fn test_store() -> AgentStore {
        let dir = tempdir().unwrap();
        AgentStore::open(AgentDatabaseConfig::new(dir.keep().join("agent.db")))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn delivery_is_oldest_first_and_failure_keeps_in_flight_bytes() {
        let mut store = test_store().await;
        let first = b"first immutable body";
        let second = b"second immutable body";
        persist_immutable_report(
            &mut store,
            &report_id(1),
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            1,
            "2026-01-01T00:00:01Z",
            first,
        )
        .await
        .unwrap();
        persist_immutable_report(
            &mut store,
            &report_id(2),
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            2,
            "2026-01-01T00:00:02Z",
            second,
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
        assert_eq!(fake.bodies.lock().unwrap().as_slice(), [first.to_vec()]);
        let row: (i64, Vec<u8>) =
            sqlx::query_as("SELECT in_flight, body FROM reports WHERE report_id = ?")
                .bind(report_id(1))
                .fetch_one(store.connection())
                .await
                .unwrap();
        assert_eq!(row, (1, first.to_vec()));
    }

    #[tokio::test]
    async fn invalid_receipt_preserves_report_and_valid_receipt_cleans_it() {
        let mut store = test_store().await;
        let body = b"receipt validation body";
        let id = report_id(3);
        persist_immutable_report(
            &mut store,
            &id,
            1,
            "0195f2a1-0012-4012-8012-000000000012",
            3,
            "2026-01-01T00:00:03Z",
            body,
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
            responses: Arc::new(Mutex::new(vec![Ok(receipt_body(&id, body))])),
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
}
