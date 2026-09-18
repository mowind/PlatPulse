//! Operator recovery for an Agent Store whose recovery drain was interrupted
//! by a stale Closing report (issue #163).
//!
//! `platpulse-agent recover` refuses to run while another runtime owns the
//! Agent Store, takes a consistent `VACUUM INTO` backup before quarantining
//! anything, then reports Closing reports that no longer belong to the current
//! Agent state and optionally removes them.

use std::path::{Path, PathBuf};

use platpulse_core::{AgentReport, BootTransition};
use thiserror::Error;
use time::OffsetDateTime;

use crate::config::AgentConfig;
use crate::database::{
    AgentDatabaseConfig, AgentDatabaseError, AgentRuntimeLock, AgentStore, AgentStoreWritePermit,
};

#[derive(Debug, Error)]
pub enum RecoverError {
    #[error("Agent Store initialization failed: {0}")]
    Store(#[from] AgentDatabaseError),
    #[error("Agent runtime ownership failed: {0}")]
    RuntimeOwnership(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Agent is not enrolled")]
    NotEnrolled,
    #[error("consistent backup failed: {0}")]
    Backup(String),
    #[error("stored report is invalid: {0}")]
    InvalidReport(String),
    #[error("Agent Store operation failed: {0}")]
    Reporting(#[from] crate::reporting::ReportStoreError),
}

/// A Closing report whose `(agent_epoch, boot_id)` no longer matches the
/// current `agent_state`, so its receipt can never be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleClosingReport {
    pub report_id: String,
    pub agent_epoch: i64,
    pub boot_id: String,
}

#[derive(Debug)]
pub struct RecoverOutcome {
    pub backup_path: PathBuf,
    pub stale: Vec<StaleClosingReport>,
    pub dropped: u64,
}

/// Take the runtime lock, back up, diagnose and optionally quarantine.
pub async fn recover_store(
    config: &AgentConfig,
    drop_stale_closing: bool,
) -> Result<RecoverOutcome, RecoverError> {
    let _runtime_lock = AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| RecoverError::RuntimeOwnership(error.to_string()))?;
    recover_store_with_permit(config, drop_stale_closing, AgentStoreWritePermit::new()).await
}

pub(crate) async fn recover_store_with_permit(
    config: &AgentConfig,
    drop_stale_closing: bool,
    write_permit: AgentStoreWritePermit,
) -> Result<RecoverOutcome, RecoverError> {
    let mut store = AgentStore::open_with_write_permit(
        AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await?;
    let backup_path = write_consistency_backup(&mut store, &config.state_db).await?;
    let stale = scan_stale_closing_reports(&mut store).await?;
    let mut dropped = 0;
    if drop_stale_closing {
        for report in &stale {
            if crate::reporting::quarantine_report(&mut store, &report.report_id).await? {
                dropped += 1;
            }
        }
    }
    store.close().await?;
    Ok(RecoverOutcome {
        backup_path,
        stale,
        dropped,
    })
}

/// Write a consistent snapshot of the live Agent Store to a sibling file.
/// `VACUUM INTO` never rewrites live WAL/SHM and refuses to overwrite an
/// existing path.
async fn write_consistency_backup(
    store: &mut AgentStore,
    state_db: &Path,
) -> Result<PathBuf, RecoverError> {
    let stamp = OffsetDateTime::now_utc().unix_timestamp_nanos();
    let mut name = state_db.as_os_str().to_os_string();
    name.push(format!(".recover-backup-{stamp}.sqlite"));
    let path = PathBuf::from(name);
    let text = path
        .to_str()
        .ok_or_else(|| RecoverError::Backup("backup path is not valid UTF-8".to_owned()))?;
    let literal = format!("'{}'", text.replace('\'', "''"));
    let _write_permit = store.acquire_write().await;
    sqlx::query(&format!("VACUUM INTO {literal}"))
        .execute(store.connection())
        .await
        .map_err(|error| RecoverError::Backup(error.to_string()))?;
    crate::database::secure_store_file(&path)?;
    Ok(path)
}

async fn scan_stale_closing_reports(
    store: &mut AgentStore,
) -> Result<Vec<StaleClosingReport>, RecoverError> {
    let current: Option<(i64, Option<String>, i64)> = sqlx::query_as(
        "SELECT agent_epoch, boot_id, report_sequence FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(store.connection())
    .await?;
    let Some((current_epoch, current_boot, current_sequence)) = current else {
        return Err(RecoverError::NotEnrolled);
    };
    let rows: Vec<(String, i64, String, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT report_id, agent_epoch, boot_id, report_sequence, body FROM reports ORDER BY created_at, report_id",
    )
    .fetch_all(store.connection())
    .await?;
    let mut stale = Vec::new();
    for (report_id, agent_epoch, boot_id, report_sequence, body) in rows {
        let report: AgentReport = serde_json::from_slice(&body)
            .map_err(|error| RecoverError::InvalidReport(format!("{report_id}: {error}")))?;
        if report.boot_transition == BootTransition::Closing
            && (agent_epoch != current_epoch
                || current_boot.as_deref() != Some(boot_id.as_str())
                || report_sequence != current_sequence)
        {
            stale.push(StaleClosingReport {
                report_id,
                agent_epoch,
                boot_id,
            });
        }
    }
    Ok(stale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;
    use sqlx::Connection;
    use tempfile::tempdir;

    fn write_config(dir: &Path) -> PathBuf {
        let config_path = dir.join("agent.toml");
        let db_path = dir.join("agent.db");
        std::fs::write(
            &config_path,
            format!(
                "server_url=\"https://example.com\"\ncredential_file=\"{}/credential\"\nstate_db=\"{}\"\ninventory_revision=1\nnodes=[{{node_id=\"0195f2a1-0014-4014-8014-000000000014\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:6790\"}}]\n",
                dir.display(),
                db_path.display()
            ),
        )
        .unwrap();
        config_path
    }

    fn closing_report(boot: &str, sequence: u64, report_id: &str) -> (Vec<u8>, String, String) {
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report.agent_epoch = 1;
        report.boot_id = boot.parse().unwrap();
        report.report_sequence = sequence;
        report.report_id = report_id.parse().unwrap();
        report.boot_transition = BootTransition::Closing;
        report.previous_boot_id = None;
        report.validate().unwrap();
        let generated_at = report.generated_at.to_string();
        let body = serde_json::to_vec(&report).unwrap();
        let hash = format!("0x{}", hex::encode(sha2::Sha256::digest(&body)));
        (body, hash, generated_at)
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_report(
        store: &mut AgentStore,
        report_id: &str,
        boot: &str,
        sequence: u64,
        body: &[u8],
        hash: &str,
        generated_at: &str,
    ) {
        sqlx::query("INSERT INTO reports (report_id, agent_epoch, boot_id, report_sequence, generated_at, body, body_sha256, body_bytes, created_at) VALUES (?, 1, ?, ?, ?, ?, ?, ?, ?)")
            .bind(report_id)
            .bind(boot)
            .bind(sequence as i64)
            .bind(generated_at)
            .bind(body)
            .bind(hash)
            .bind(body.len() as i64)
            .bind(generated_at)
            .execute(store.connection())
            .await
            .unwrap();
    }

    async fn open(config: &AgentConfig) -> AgentStore {
        AgentStore::open_with_write_permit(
            AgentDatabaseConfig::new(&config.state_db),
            AgentStoreWritePermit::new(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn recover_refuses_while_another_runtime_holds_the_lock() {
        let dir = tempdir().unwrap();
        let config = AgentConfig::resolve(&write_config(dir.path())).unwrap();
        open(&config).await.close().await.unwrap();

        let _held = AgentRuntimeLock::acquire(&config.state_db).unwrap();
        let error = recover_store(&config, false).await.unwrap_err();
        assert!(matches!(error, RecoverError::RuntimeOwnership(_)));
    }

    #[tokio::test]
    async fn recover_backs_up_and_quarantines_only_stale_closing_reports() {
        let dir = tempdir().unwrap();
        let config = AgentConfig::resolve(&write_config(dir.path())).unwrap();

        let current_boot = "0195f2a1-0060-4060-8060-000000000060";
        let stale_boot = "0195f2a1-0050-4050-8050-000000000050";
        let stale_id = "0195f2a1-0051-4051-8051-000000000051";
        let current_id = "0195f2a1-0061-4061-8061-000000000061";
        let same_boot_stale_id = "0195f2a1-0052-4052-8052-000000000052";
        let (stale_body, stale_hash, stale_at) = closing_report(stale_boot, 3, stale_id);
        let (same_body, same_hash, same_at) = closing_report(current_boot, 4, same_boot_stale_id);
        let (current_body, current_hash, current_at) = closing_report(current_boot, 6, current_id);

        let mut store = open(&config).await;
        sqlx::query("INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, boot_state, updated_at) VALUES (1, ?, 1, ?, 6, 1, 'active', ?)")
            .bind("0195f2a1-0011-4011-8011-000000000011")
            .bind(current_boot)
            .bind("2026-08-12T08:00:00Z")
            .execute(store.connection())
            .await
            .unwrap();
        insert_report(
            &mut store,
            stale_id,
            stale_boot,
            3,
            &stale_body,
            &stale_hash,
            &stale_at,
        )
        .await;
        insert_report(
            &mut store,
            same_boot_stale_id,
            current_boot,
            4,
            &same_body,
            &same_hash,
            &same_at,
        )
        .await;
        insert_report(
            &mut store,
            current_id,
            current_boot,
            6,
            &current_body,
            &current_hash,
            &current_at,
        )
        .await;
        sqlx::query("INSERT INTO report_sample_assignments (report_id, node_id, sample_kind, from_height, to_height) VALUES (?, ?, 'block', 7, 7)")
            .bind(stale_id)
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .execute(store.connection())
            .await
            .unwrap();
        store.close().await.unwrap();

        // Diagnostic-only run: a consistent backup is forced, nothing mutates.
        let outcome = recover_store_with_permit(&config, false, AgentStoreWritePermit::new())
            .await
            .unwrap();
        assert!(outcome.backup_path.exists());
        assert_ne!(outcome.backup_path, config.state_db);
        assert_eq!(outcome.stale.len(), 2);
        assert_eq!(outcome.stale[0].report_id, stale_id);
        assert_eq!(outcome.stale[1].report_id, same_boot_stale_id);
        assert_eq!(outcome.dropped, 0);

        // The backup really is the live Store snapshot.
        let backup_options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&outcome.backup_path)
            .read_only(true);
        let mut backup = sqlx::SqliteConnection::connect_with(&backup_options)
            .await
            .unwrap();
        let boot: String = sqlx::query_scalar("SELECT boot_id FROM agent_state WHERE singleton=1")
            .fetch_one(&mut backup)
            .await
            .unwrap();
        assert_eq!(boot, current_boot);
        let reports_in_backup: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reports")
            .fetch_one(&mut backup)
            .await
            .unwrap();
        assert_eq!(reports_in_backup, 3);
        backup.close().await.unwrap();

        // Quarantine run: only the stale Closing report and its sample vanish.
        let outcome = recover_store_with_permit(&config, true, AgentStoreWritePermit::new())
            .await
            .unwrap();
        assert_eq!(outcome.dropped, 2);

        let mut reopened = open(&config).await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id=?")
                .bind(stale_id)
                .fetch_one(reopened.connection())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM report_sample_assignments WHERE report_id=?"
            )
            .bind(stale_id)
            .fetch_one(reopened.connection())
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id=?")
                .bind(same_boot_stale_id)
                .fetch_one(reopened.connection())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reports WHERE report_id=?")
                .bind(current_id)
                .fetch_one(reopened.connection())
                .await
                .unwrap(),
            1
        );
        reopened.close().await.unwrap();
    }
}
