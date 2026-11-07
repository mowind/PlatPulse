//! The Agent's shutdown record: the single owner of the `shutdown_*` columns
//! of `agent_state`, which every Report also states in its Durable Spool
//! diagnostics.
//!
//! ```text
//!   running ──begin──▶ draining ──Closing stored──▶ final_stored ──send failed──▶ send_failed
//!      ▲                                              │
//!      │                                              └─sender deadline─▶ forced_kill_recovery
//!      └── successor Boot settled ◀── Closing applied (final_stored)
//! ```
//!
//! Every write is one named function. A Report built ahead of the write it
//! announces (the Closing Report, and the first Reports of the successor Boot)
//! takes the record it states from [`ShutdownRecord`], next to the write that
//! later makes it true, so the stored and the reported record cannot drift.

use std::fmt;

use platpulse_core::SpoolDiagnostics;
use platpulse_core::time::Rfc3339;
use sqlx::SqliteConnection;

/// Where the Agent's graceful shutdown stands, as stored in
/// `agent_state.shutdown_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownState {
    Running,
    Draining,
    FinalStored,
    SendFailed,
    ForcedKillRecovery,
}

impl ShutdownState {
    /// The stored and reported text of this state.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Draining => "draining",
            Self::FinalStored => "final_stored",
            Self::SendFailed => "send_failed",
            Self::ForcedKillRecovery => "forced_kill_recovery",
        }
    }
}

impl fmt::Display for ShutdownState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The shutdown record a Report states before the write that makes it true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownRecord {
    /// No shutdown is in progress: what a successor Boot reports while it
    /// announces the previous Boot, ahead of [`settle`].
    Running,
    /// What a Closing Report states about the shutdown that produced it, ahead
    /// of [`closing_stored`]: stored as final, not forced.
    ClosingStored {
        started_at: Rfc3339,
        deadline_at: Rfc3339,
    },
}

impl ShutdownRecord {
    /// State this record in a Report's Durable Spool diagnostics.
    pub(crate) fn describe(self, diagnostics: &mut SpoolDiagnostics) {
        match self {
            Self::Running => {
                diagnostics.shutdown_state = Some(ShutdownState::Running.as_str().to_owned());
                diagnostics.shutdown_started_at = None;
                diagnostics.shutdown_deadline_at = None;
                diagnostics.shutdown_finished_at = None;
                diagnostics.shutdown_unresolved_range = None;
                diagnostics.shutdown_last_error = None;
                diagnostics.shutdown_forced = Some(false);
                diagnostics.shutdown_report_id = None;
            }
            Self::ClosingStored {
                started_at,
                deadline_at,
            } => {
                diagnostics.shutdown_state = Some(ShutdownState::FinalStored.as_str().to_owned());
                diagnostics.shutdown_started_at = Some(started_at);
                diagnostics.shutdown_deadline_at = Some(deadline_at);
                diagnostics.shutdown_forced = Some(false);
            }
        }
    }
}

/// A graceful shutdown began draining, bounded by `deadline_at`.
pub(crate) async fn begin(
    conn: &mut SqliteConnection,
    started_at: Rfc3339,
    deadline_at: Rfc3339,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_state SET shutdown_state=?, shutdown_started_at=?, shutdown_deadline_at=?, shutdown_updated_at=?, updated_at=? WHERE singleton=1",
    )
    .bind(ShutdownState::Draining.as_str())
    .bind(started_at.to_string())
    .bind(deadline_at.to_string())
    .bind(started_at.to_string())
    .bind(started_at.to_string())
    .execute(conn)
    .await?;
    Ok(())
}

/// The shutdown drain left the heights `range` unresolved for `reason`.
pub(crate) async fn record_unresolved(
    conn: &mut SqliteConnection,
    range: (u64, u64),
    reason: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_state SET shutdown_unresolved_from=?, shutdown_unresolved_to=?, shutdown_last_error=? WHERE singleton=1",
    )
    .bind(range.0 as i64)
    .bind(range.1 as i64)
    .bind(reason)
    .execute(conn)
    .await?;
    Ok(())
}

/// The Closing Report `report_id` was stored as the Boot's final Report.
pub(crate) async fn closing_stored(
    conn: &mut SqliteConnection,
    report_id: &str,
    report_sequence: u64,
    at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_state SET shutdown_state=?, shutdown_report_id=?, shutdown_report_sequence=?, shutdown_finished_at=?, shutdown_updated_at=? WHERE singleton=1",
    )
    .bind(ShutdownState::FinalStored.as_str())
    .bind(report_id)
    .bind(report_sequence as i64)
    .bind(at)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The Server applied the stored Closing Report, closing the Boot.
pub(crate) async fn closing_applied(
    conn: &mut SqliteConnection,
    at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_state SET shutdown_state=?, shutdown_finished_at=?, shutdown_last_error=NULL, shutdown_updated_at=? WHERE singleton=1",
    )
    .bind(ShutdownState::FinalStored.as_str())
    .bind(at)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Sending the stored Closing Report failed with `error`.
pub(crate) async fn send_failed(
    conn: &mut SqliteConnection,
    error: &str,
    at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_state SET shutdown_state=?, shutdown_last_error=?, shutdown_updated_at=? WHERE singleton=1",
    )
    .bind(ShutdownState::SendFailed.as_str())
    .bind(error.chars().take(256).collect::<String>())
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The sender deadline passed before the Closing Report was applied, so the
/// process is about to be stopped and the next start recovers the Boot.
pub(crate) async fn forced_kill(conn: &mut SqliteConnection, at: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_state SET shutdown_state=?, shutdown_forced=1, shutdown_finished_at=?, shutdown_updated_at=? WHERE singleton=1",
    )
    .bind(ShutdownState::ForcedKillRecovery.as_str())
    .bind(at)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The Server settled the successor Boot, so the shutdown that closed the
/// previous Boot is fully resolved and the record returns to
/// [`ShutdownRecord::Running`]. Closing the Boot left the record final_stored
/// for that Closing Report; any other record is a shutdown the current Boot
/// has already begun, and it is kept.
pub(crate) async fn settle(conn: &mut SqliteConnection, at: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_state SET shutdown_state=?, shutdown_started_at=NULL, shutdown_deadline_at=NULL, shutdown_finished_at=NULL, shutdown_unresolved_from=NULL, shutdown_unresolved_to=NULL, shutdown_last_error=NULL, shutdown_forced=0, shutdown_report_id=NULL, shutdown_report_sequence=NULL, shutdown_updated_at=? WHERE singleton=1 AND shutdown_state=? AND shutdown_report_id IS close_report_id",
    )
    .bind(ShutdownState::Running.as_str())
    .bind(at)
    .bind(ShutdownState::FinalStored.as_str())
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{AgentDatabaseConfig, AgentStore};
    use tempfile::tempdir;

    const AT: &str = "2026-08-12T08:00:00Z";
    const CLOSING: &str = "0195f2a1-0050-4050-8050-000000000050";

    async fn enrolled() -> (tempfile::TempDir, AgentStore) {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, updated_at) VALUES (1, '0195f2a1-0011-4011-8011-000000000011', 1, '0195f2a1-0041-4041-8041-000000000041', 3, 1, ?)")
            .bind(AT)
            .execute(store.connection())
            .await
            .unwrap();
        (dir, store)
    }

    async fn stored(store: &mut AgentStore) -> (String, Option<String>, i64) {
        sqlx::query_as(
            "SELECT shutdown_state, shutdown_report_id, shutdown_forced FROM agent_state WHERE singleton=1",
        )
        .fetch_one(store.connection())
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn every_write_stores_a_state_the_schema_accepts() {
        let (_dir, mut store) = enrolled().await;
        let at: Rfc3339 = AT.parse().unwrap();
        begin(store.connection(), at, at).await.unwrap();
        assert_eq!(stored(&mut store).await.0, ShutdownState::Draining.as_str());
        record_unresolved(
            store.connection(),
            (5, 9),
            "shutdown drain deadline exhausted",
        )
        .await
        .unwrap();
        closing_stored(store.connection(), CLOSING, 4, AT)
            .await
            .unwrap();
        assert_eq!(
            stored(&mut store).await,
            ("final_stored".to_owned(), Some(CLOSING.to_owned()), 0)
        );
        send_failed(store.connection(), &"x".repeat(300), AT)
            .await
            .unwrap();
        assert_eq!(
            stored(&mut store).await.0,
            ShutdownState::SendFailed.as_str()
        );
        forced_kill(store.connection(), AT).await.unwrap();
        assert_eq!(
            stored(&mut store).await,
            (
                "forced_kill_recovery".to_owned(),
                Some(CLOSING.to_owned()),
                1
            )
        );
        closing_applied(store.connection(), AT).await.unwrap();
        assert_eq!(
            stored(&mut store).await.0,
            ShutdownState::FinalStored.as_str()
        );
    }

    #[tokio::test]
    async fn settling_clears_only_the_record_of_the_closed_boot() {
        let (_dir, mut store) = enrolled().await;
        let at: Rfc3339 = AT.parse().unwrap();
        closing_stored(store.connection(), CLOSING, 4, AT)
            .await
            .unwrap();
        sqlx::query("UPDATE agent_state SET close_report_id=?")
            .bind(CLOSING)
            .execute(store.connection())
            .await
            .unwrap();
        // A shutdown the successor Boot already began keeps its record.
        begin(store.connection(), at, at).await.unwrap();
        settle(store.connection(), AT).await.unwrap();
        assert_eq!(stored(&mut store).await.0, ShutdownState::Draining.as_str());

        closing_applied(store.connection(), AT).await.unwrap();
        settle(store.connection(), AT).await.unwrap();
        assert_eq!(
            stored(&mut store).await,
            (ShutdownState::Running.as_str().to_owned(), None, 0)
        );
    }

    #[test]
    fn a_report_states_the_record_its_write_will_store() {
        let at: Rfc3339 = AT.parse().unwrap();
        let mut diagnostics = SpoolDiagnostics {
            queued_bytes: 0,
            queued_reports: 0,
            oldest_queued_age_ms: 0,
            dropped_reports: 0,
            dropped_samples: 0,
            in_flight: None,
            last_delivery_error: None,
            last_delivery_at: None,
            capacity_bytes: None,
            max_age_seconds: None,
            dropped_sequence_range: None,
            dropped_time_range: None,
            dropped_height_range: None,
            pending_history_gaps: None,
            report_too_large: None,
            store_fatal: None,
            store_error: None,
            shutdown_state: Some("send_failed".to_owned()),
            shutdown_started_at: None,
            shutdown_deadline_at: None,
            shutdown_finished_at: None,
            shutdown_unresolved_range: Some((5, 9)),
            shutdown_last_error: Some("refused".to_owned()),
            shutdown_forced: Some(true),
            shutdown_report_id: None,
        };
        ShutdownRecord::ClosingStored {
            started_at: at,
            deadline_at: at,
        }
        .describe(&mut diagnostics);
        assert_eq!(diagnostics.shutdown_state.as_deref(), Some("final_stored"));
        assert_eq!(diagnostics.shutdown_forced, Some(false));
        assert_eq!(diagnostics.shutdown_started_at, Some(at));
        ShutdownRecord::Running.describe(&mut diagnostics);
        assert_eq!(diagnostics.shutdown_state.as_deref(), Some("running"));
        assert_eq!(diagnostics.shutdown_last_error, None);
        assert_eq!(diagnostics.shutdown_started_at, None);
    }
}
