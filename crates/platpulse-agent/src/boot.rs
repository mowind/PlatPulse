//! The Agent's Boot lifecycle: the single owner of the Boot columns of
//! `agent_state`.
//!
//! A Boot is one run of the Agent between a start and an accepted Closing
//! Report. Its local record moves through three phases:
//!
//! ```text
//!           accepted Closing receipt
//!   Active ─────────────────────────────▶ DrainedPending (next Boot)
//!    │  ▲                                    │
//!    │  └─ accepted DrainedPrevious receipt ─┘
//!    │ a start finds the Boot unclosed
//!    ▼          accepted Closing receipt
//!   RecoveryDraining ───────────────────▶ DrainedPending (next Boot)
//! ```
//!
//! A Closing accepted while the Boot is still DrainedPending starts the next
//! Boot DrainedPending too, still naming the Boot the Server last saw drained.
//!
//! Every transition is one named function that re-checks the state it expects
//! in the same statement that writes the next one, so a caller never writes
//! `boot_state` or `pending_transition` itself and never compares their text.
//! Callers read [`BootState`] once, decide from its typed [`BootPhase`], and
//! hand the same value back as the expectation of the write.

use std::fmt;
use std::str::FromStr;

use platpulse_core::BootTransition;
use platpulse_core::identity::{AgentId, BootId};
use sqlx::SqliteConnection;
use sqlx::sqlite::{Sqlite, SqliteArguments};
use thiserror::Error;
use uuid::Uuid;

const ACTIVE: &str = "active";
const DRAINING: &str = "draining";
const DRAINED_PENDING: &str = "drained_pending";
const DRAINED_PREVIOUS: &str = "drained_previous";

/// Where the current Boot stands between its start and its Closing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BootPhase {
    /// Collecting normally; every report continues the current Boot.
    Active,
    /// A Recovery Drain is delivering the unclosed Boot's backlog before its
    /// Closing; no new report may be collected until it finishes.
    RecoveryDraining,
    /// The previous Boot was closed by an accepted Closing, and the Server has
    /// not yet accepted a report announcing the current Boot as its successor.
    /// Every report carries the DrainedPrevious transition until it does.
    DrainedPending { previous_boot_id: BootId },
}

impl BootPhase {
    /// Decode the stored columns. A combination no transition writes is an
    /// invalid Store, not a phase to guess.
    pub(crate) fn from_columns(
        boot_state: &str,
        pending_transition: Option<&str>,
        pending_previous_boot_id: Option<&str>,
    ) -> Result<Self, BootStateError> {
        match (boot_state, pending_transition, pending_previous_boot_id) {
            (ACTIVE, None, None) => Ok(Self::Active),
            (DRAINING, None, None) => Ok(Self::RecoveryDraining),
            (DRAINED_PENDING, Some(DRAINED_PREVIOUS), Some(previous)) => Ok(Self::DrainedPending {
                previous_boot_id: parse_boot_id(previous)?,
            }),
            _ => Err(BootStateError::Invalid(format!(
                "boot_state={boot_state} pending_transition={} pending_previous_boot_id={}",
                pending_transition.unwrap_or("NULL"),
                pending_previous_boot_id.unwrap_or("NULL")
            ))),
        }
    }

    fn boot_state(self) -> &'static str {
        match self {
            Self::Active => ACTIVE,
            Self::RecoveryDraining => DRAINING,
            Self::DrainedPending { .. } => DRAINED_PENDING,
        }
    }

    fn pending_previous_boot_id(self) -> Option<BootId> {
        match self {
            Self::DrainedPending { previous_boot_id } => Some(previous_boot_id),
            Self::Active | Self::RecoveryDraining => None,
        }
    }
}

impl fmt::Display for BootPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.boot_state())
    }
}

/// The Boot the last accepted Closing closed, and that Closing's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClosedBoot {
    pub(crate) boot_id: BootId,
    pub(crate) close_report_id: String,
}

/// The enrolled Agent's Boot record as one consistent value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BootState {
    pub(crate) agent_id: AgentId,
    pub(crate) agent_epoch: u64,
    /// None until the first report of a freshly enrolled Agent is stored.
    pub(crate) boot_id: Option<BootId>,
    /// The sequence of the newest report stored for the current Boot.
    pub(crate) report_sequence: u64,
    pub(crate) phase: BootPhase,
    pub(crate) last_closed: Option<ClosedBoot>,
}

/// The Boot fields of the next collected report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NextReport {
    pub(crate) boot_id: BootId,
    pub(crate) report_sequence: u64,
    pub(crate) boot_transition: BootTransition,
    pub(crate) previous_boot_id: Option<BootId>,
}

impl BootState {
    /// The Boot fields the next collected report must carry, or None while a
    /// Recovery Drain is unfinished. A freshly enrolled Agent starts its first
    /// Boot here; it is stored only with that report.
    pub(crate) fn next_report(&self) -> Option<NextReport> {
        let (boot_transition, previous_boot_id) = match self.phase {
            BootPhase::RecoveryDraining => return None,
            BootPhase::Active => (BootTransition::Continuing, None),
            BootPhase::DrainedPending { previous_boot_id } => {
                (BootTransition::DrainedPrevious, Some(previous_boot_id))
            }
        };
        Some(NextReport {
            boot_id: self.boot_id.unwrap_or_else(new_boot_id),
            report_sequence: self.report_sequence + 1,
            boot_transition,
            previous_boot_id,
        })
    }

    /// Whether a Closing report still closes this Boot: it was the newest
    /// report of this Boot in this Epoch. Any other Closing was superseded and
    /// its receipt can never be applied.
    pub(crate) fn owns_closing(
        &self,
        agent_epoch: u64,
        boot_id: BootId,
        report_sequence: u64,
    ) -> bool {
        self.agent_epoch == agent_epoch
            && self.boot_id == Some(boot_id)
            && self.report_sequence == report_sequence
    }
}

#[derive(Debug, Error)]
pub(crate) enum BootStateError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("invalid persisted Boot state: {0}")]
    Invalid(String),
}

#[derive(sqlx::FromRow)]
struct BootRow {
    agent_id: Option<String>,
    agent_epoch: i64,
    boot_id: Option<String>,
    report_sequence: i64,
    boot_state: String,
    pending_transition: Option<String>,
    pending_previous_boot_id: Option<String>,
    previous_boot_id: Option<String>,
    close_report_id: Option<String>,
}

/// Read the Boot record, or None when the Agent is not enrolled.
pub(crate) async fn read(
    connection: &mut SqliteConnection,
) -> Result<Option<BootState>, BootStateError> {
    let row: Option<BootRow> = sqlx::query_as(
        "SELECT agent_id, agent_epoch, boot_id, report_sequence, boot_state, pending_transition, pending_previous_boot_id, previous_boot_id, close_report_id FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let Some(agent_id) = row.agent_id.as_deref() else {
        return Ok(None);
    };
    let last_closed = match (row.previous_boot_id.as_deref(), row.close_report_id) {
        (Some(boot_id), Some(close_report_id)) => Some(ClosedBoot {
            boot_id: parse_boot_id(boot_id)?,
            close_report_id,
        }),
        _ => None,
    };
    Ok(Some(BootState {
        agent_id: AgentId::from_str(agent_id)
            .map_err(|error| BootStateError::Invalid(format!("agent_id: {error}")))?,
        agent_epoch: non_negative("agent_epoch", row.agent_epoch)?,
        boot_id: row.boot_id.as_deref().map(parse_boot_id).transpose()?,
        report_sequence: non_negative("report_sequence", row.report_sequence)?,
        phase: BootPhase::from_columns(
            &row.boot_state,
            row.pending_transition.as_deref(),
            row.pending_previous_boot_id.as_deref(),
        )?,
        last_closed,
    }))
}

/// The full expectation every guarded transition re-checks; [`bind_expected`]
/// binds its parameters in this order.
macro_rules! expected_state {
    () => {
        "singleton=1 AND agent_epoch=? AND boot_id IS ? AND report_sequence=? AND boot_state=? AND pending_previous_boot_id IS ?"
    };
}

const UPDATE_RECORD_REPORT: &str = concat!(
    "UPDATE agent_state SET boot_id=?, report_sequence=?, updated_at=? WHERE ",
    expected_state!()
);

const UPDATE_BEGIN_RECOVERY_DRAIN: &str = concat!(
    "UPDATE agent_state SET boot_state='draining', updated_at=? WHERE ",
    expected_state!()
);

/// Advance the Boot to a newly stored report of it. The first report of a
/// freshly enrolled Agent also stores its first Boot. Returns false, writing
/// nothing, when the record is no longer `expected`.
pub(crate) async fn record_report(
    connection: &mut SqliteConnection,
    expected: &BootState,
    boot_id: BootId,
    report_sequence: u64,
    at: &str,
) -> Result<bool, sqlx::Error> {
    if expected.boot_id.is_some_and(|current| current != boot_id)
        || report_sequence != expected.report_sequence + 1
    {
        return Ok(false);
    }
    let query = sqlx::query(UPDATE_RECORD_REPORT)
        .bind(boot_id.to_string())
        .bind(report_sequence as i64)
        .bind(at);
    let result = bind_expected(query, expected).execute(connection).await?;
    Ok(result.rows_affected() == 1)
}

/// Start a Recovery Drain of the current Boot. Returns the draining record, or
/// None when the record is no longer `expected` or has no Boot to drain.
pub(crate) async fn begin_recovery_drain(
    connection: &mut SqliteConnection,
    expected: &BootState,
    at: &str,
) -> Result<Option<BootState>, sqlx::Error> {
    if expected.boot_id.is_none() || matches!(expected.phase, BootPhase::DrainedPending { .. }) {
        return Ok(None);
    }
    let query = sqlx::query(UPDATE_BEGIN_RECOVERY_DRAIN).bind(at);
    let result = bind_expected(query, expected).execute(connection).await?;
    Ok((result.rows_affected() == 1).then(|| BootState {
        phase: BootPhase::RecoveryDraining,
        ..expected.clone()
    }))
}

/// What an accepted Closing receipt did to the Boot record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseOutcome {
    /// The Closing closed the current Boot; the next Boot is DrainedPending.
    Closed { next_boot_id: BootId },
    /// The Closing belongs to a Boot, Epoch or sequence the record has moved
    /// past, so it can never close anything.
    Superseded,
    /// The Closing still belongs to the current Boot but the record did not
    /// change; another writer raced this one.
    Conflict,
}

/// Close `boot_id` because the Server accepted its Closing report, and start
/// the next Boot as DrainedPending. When the closed Boot was itself still
/// DrainedPending, the Boot the Server is waiting to see drained stays the
/// older one: the next report must announce the oldest unacknowledged Boot.
pub(crate) async fn close(
    connection: &mut SqliteConnection,
    agent_epoch: u64,
    boot_id: BootId,
    report_sequence: u64,
    close_report_id: &str,
    at: &str,
) -> Result<CloseOutcome, sqlx::Error> {
    let next_boot_id = new_boot_id();
    let closed = boot_id.to_string();
    let result = sqlx::query(
        "UPDATE agent_state SET boot_id=?, report_sequence=0, boot_state='drained_pending', pending_transition='drained_previous', pending_previous_boot_id=COALESCE(pending_previous_boot_id, ?), previous_boot_id=?, close_report_id=?, close_applied_at=?, updated_at=? WHERE singleton=1 AND agent_epoch=? AND boot_id=? AND report_sequence=?",
    )
    .bind(next_boot_id.to_string())
    .bind(&closed)
    .bind(&closed)
    .bind(close_report_id)
    .bind(at)
    .bind(at)
    .bind(agent_epoch as i64)
    .bind(&closed)
    .bind(report_sequence as i64)
    .execute(&mut *connection)
    .await?;
    if result.rows_affected() == 1 {
        return Ok(CloseOutcome::Closed { next_boot_id });
    }
    let current: Option<(i64, Option<String>, i64)> = sqlx::query_as(
        "SELECT agent_epoch, boot_id, report_sequence FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(connection)
    .await?;
    let owned = current.is_none_or(|(epoch, current_boot, sequence)| {
        epoch == agent_epoch as i64
            && current_boot.as_deref() == Some(closed.as_str())
            && sequence == report_sequence as i64
    });
    Ok(if owned {
        CloseOutcome::Conflict
    } else {
        CloseOutcome::Superseded
    })
}

/// Settle the current Boot because the Server accepted a report announcing it
/// as the successor of the drained previous Boot. Returns whether the record
/// moved: a later DrainedPrevious report of an already settled Boot, or one
/// delivered during a Recovery Drain, leaves the phase as it is.
pub(crate) async fn settle(
    connection: &mut SqliteConnection,
    agent_epoch: u64,
    boot_id: BootId,
    at: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE agent_state SET boot_state='active', pending_transition=NULL, pending_previous_boot_id=NULL, updated_at=? WHERE singleton=1 AND agent_epoch=? AND boot_id=? AND boot_state='drained_pending'",
    )
    .bind(at)
    .bind(agent_epoch as i64)
    .bind(boot_id.to_string())
    .execute(connection)
    .await?;
    Ok(result.rows_affected() == 1)
}

fn bind_expected<'q>(
    query: sqlx::query::Query<'q, Sqlite, SqliteArguments<'q>>,
    expected: &BootState,
) -> sqlx::query::Query<'q, Sqlite, SqliteArguments<'q>> {
    query
        .bind(expected.agent_epoch as i64)
        .bind(expected.boot_id.map(|boot_id| boot_id.to_string()))
        .bind(expected.report_sequence as i64)
        .bind(expected.phase.boot_state())
        .bind(
            expected
                .phase
                .pending_previous_boot_id()
                .map(|boot_id| boot_id.to_string()),
        )
}

fn new_boot_id() -> BootId {
    BootId::from_str(&Uuid::new_v4().to_string()).expect("a v4 UUID is a valid Boot ID")
}

fn parse_boot_id(value: &str) -> Result<BootId, BootStateError> {
    BootId::from_str(value)
        .map_err(|error| BootStateError::Invalid(format!("boot id {value}: {error}")))
}

fn non_negative(column: &str, value: i64) -> Result<u64, BootStateError> {
    u64::try_from(value).map_err(|_| BootStateError::Invalid(format!("{column} is {value}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{AgentDatabaseConfig, AgentStore};
    use tempfile::tempdir;

    const AGENT: &str = "0195f2a1-0011-4011-8011-000000000011";
    const OLD_BOOT: &str = "0195f2a1-0040-4040-8040-000000000040";
    const BOOT: &str = "0195f2a1-0041-4041-8041-000000000041";
    const AT: &str = "2026-08-12T08:00:00Z";

    async fn enrolled(
        boot_state: &str,
        pending_previous: Option<&str>,
    ) -> (tempfile::TempDir, AgentStore) {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, boot_state, pending_transition, pending_previous_boot_id, updated_at) VALUES (1, ?, 1, ?, 3, 1, ?, ?, ?, ?)")
            .bind(AGENT)
            .bind(BOOT)
            .bind(boot_state)
            .bind(pending_previous.map(|_| DRAINED_PREVIOUS))
            .bind(pending_previous)
            .bind(AT)
            .execute(store.connection())
            .await
            .unwrap();
        (dir, store)
    }

    fn boot(value: &str) -> BootId {
        value.parse().unwrap()
    }

    #[test]
    fn only_the_combinations_a_transition_writes_decode() {
        assert_eq!(
            BootPhase::from_columns("active", None, None).unwrap(),
            BootPhase::Active
        );
        assert_eq!(
            BootPhase::from_columns("draining", None, None).unwrap(),
            BootPhase::RecoveryDraining
        );
        assert_eq!(
            BootPhase::from_columns("drained_pending", Some("drained_previous"), Some(OLD_BOOT))
                .unwrap(),
            BootPhase::DrainedPending {
                previous_boot_id: boot(OLD_BOOT)
            }
        );
        for (state, pending, previous) in [
            ("drained_pending", None, None),
            ("drained_pending", Some("drained_previous"), None),
            ("drained_pending", Some("closing"), Some(OLD_BOOT)),
            ("active", Some("drained_previous"), Some(OLD_BOOT)),
            ("stopping", None, None),
        ] {
            assert!(
                matches!(
                    BootPhase::from_columns(state, pending, previous),
                    Err(BootStateError::Invalid(_))
                ),
                "{state} {pending:?} {previous:?} must be invalid"
            );
        }
    }

    #[tokio::test]
    async fn the_next_report_follows_the_phase() {
        let (_dir, mut store) = enrolled("drained_pending", Some(OLD_BOOT)).await;
        let state = read(store.connection()).await.unwrap().unwrap();
        let next = state.next_report().unwrap();
        assert_eq!(next.boot_id, boot(BOOT));
        assert_eq!(next.report_sequence, 4);
        assert_eq!(next.boot_transition, BootTransition::DrainedPrevious);
        assert_eq!(next.previous_boot_id, Some(boot(OLD_BOOT)));

        let draining = begin_recovery_drain(store.connection(), &state, AT)
            .await
            .unwrap();
        assert_eq!(draining, None, "a DrainedPending Boot was already closed");

        sqlx::query("UPDATE agent_state SET boot_state='draining', pending_transition=NULL, pending_previous_boot_id=NULL")
            .execute(store.connection())
            .await
            .unwrap();
        let state = read(store.connection()).await.unwrap().unwrap();
        assert_eq!(state.next_report(), None);
        store.close().await.unwrap();
    }

    #[tokio::test]
    async fn a_write_against_a_stale_expectation_changes_nothing() {
        let (_dir, mut store) = enrolled("active", None).await;
        let state = read(store.connection()).await.unwrap().unwrap();
        assert!(
            record_report(store.connection(), &state, boot(BOOT), 4, AT)
                .await
                .unwrap()
        );
        assert!(
            !record_report(store.connection(), &state, boot(BOOT), 4, AT)
                .await
                .unwrap(),
            "the record has already moved to sequence 4"
        );
        assert_eq!(
            begin_recovery_drain(store.connection(), &state, AT)
                .await
                .unwrap(),
            None
        );
        let current = read(store.connection()).await.unwrap().unwrap();
        assert_eq!(current.report_sequence, 4);
        assert_eq!(current.phase, BootPhase::Active);
        store.close().await.unwrap();
    }

    #[tokio::test]
    async fn closing_a_pending_boot_keeps_announcing_the_oldest_unacknowledged_boot() {
        let (_dir, mut store) = enrolled("drained_pending", Some(OLD_BOOT)).await;
        let outcome = close(store.connection(), 1, boot(BOOT), 3, "closing", AT)
            .await
            .unwrap();
        let CloseOutcome::Closed { next_boot_id } = outcome else {
            panic!("the current Closing must close the Boot: {outcome:?}");
        };
        let state = read(store.connection()).await.unwrap().unwrap();
        assert_eq!(state.boot_id, Some(next_boot_id));
        assert_eq!(state.report_sequence, 0);
        assert_eq!(
            state.phase,
            BootPhase::DrainedPending {
                previous_boot_id: boot(OLD_BOOT)
            }
        );
        assert_eq!(
            state.last_closed,
            Some(ClosedBoot {
                boot_id: boot(BOOT),
                close_report_id: "closing".to_owned()
            })
        );
        assert_eq!(
            close(store.connection(), 1, boot(BOOT), 3, "closing", AT)
                .await
                .unwrap(),
            CloseOutcome::Superseded,
            "the record has moved past the closed Boot"
        );
        store.close().await.unwrap();
    }

    #[tokio::test]
    async fn only_a_drained_pending_boot_settles() {
        let (_dir, mut store) = enrolled("draining", None).await;
        assert!(!settle(store.connection(), 1, boot(BOOT), AT).await.unwrap());
        let state = read(store.connection()).await.unwrap().unwrap();
        assert_eq!(state.phase, BootPhase::RecoveryDraining);
        store.close().await.unwrap();

        let (_dir, mut store) = enrolled("drained_pending", Some(OLD_BOOT)).await;
        assert!(!settle(store.connection(), 2, boot(BOOT), AT).await.unwrap());
        assert!(settle(store.connection(), 1, boot(BOOT), AT).await.unwrap());
        let state = read(store.connection()).await.unwrap().unwrap();
        assert_eq!(state.phase, BootPhase::Active);
        store.close().await.unwrap();
    }
}
