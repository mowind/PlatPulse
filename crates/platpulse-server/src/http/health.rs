//! Minimal operational health routes (design §20.3).
//!
//! `/health/live` only proves the event loop responds; `/health/ready`
//! checks the components the Server owns (SQLite migrations, the first
//! Owner, Web assets). Neither response leaks versions, DB paths, or
//! internal counts.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;
use utoipa::ToSchema;

use crate::http::AppState;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// Six-hour cadence: a full integrity_check holds the single SQLite connection
// for the whole scan, and a deployment-sized database takes seconds. Scanning
// every five minutes would stall ingestion on every tick for no real detection
// benefit; a six-hour window still surfaces corruption long before the masked
// incidents this monitor exists to catch. Measured on an isolated 1.16 GB
// database, integrity_check(1) took ~2.5 s, so the budget must exceed the
// largest expected scan or readiness would report unavailable forever.
const INTEGRITY_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const INTEGRITY_SCAN_BUDGET: Duration = Duration::from_secs(60);
const INTEGRITY_ACQUIRE_TIMEOUT: Duration = Duration::from_millis(250);
// A scan that cannot run to completion is retried far sooner than the healthy
// cadence: startup workers contend for the sole connection, and a sub-second
// race must not defer the next scan by six hours. The delay doubles after each
// consecutive incomplete scan and is capped, so a persistently contended or
// interrupted database is probed at most hourly instead of being hammered.
const INTEGRITY_RETRY_START: Duration = Duration::from_secs(30);
const INTEGRITY_RETRY_CAP: Duration = Duration::from_secs(60 * 60);
// Two intervals plus a budget of slack: a missed scan degrades readiness
// rather than silently reporting healthy.
pub(super) const INTEGRITY_STALE_AFTER: Duration = Duration::from_secs(13 * 60 * 60);

/// Timing for the integrity monitor. Production uses INTEGRITY_CADENCE;
/// regression tests inject a fast cadence to drive the real loop without
/// waiting an actual six-hour interval.
#[derive(Debug, Clone, Copy)]
struct IntegrityCadence {
    /// Delay between scans that ran to completion.
    healthy: Duration,
    /// First delay after a scan that could not run to completion.
    retry_start: Duration,
    /// Upper bound for the retried delay.
    retry_cap: Duration,
}

const INTEGRITY_CADENCE: IntegrityCadence = IntegrityCadence {
    healthy: INTEGRITY_INTERVAL,
    retry_start: INTEGRITY_RETRY_START,
    retry_cap: INTEGRITY_RETRY_CAP,
};

/// Verdict of one monitor iteration; it selects the next cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IntegrityOutcome {
    /// A scan completed and reported the database healthy.
    Healthy,
    /// A scan completed (or errored) with a corruption indication; the runtime
    /// corruption latch is set and readiness fails closed until restart.
    Corrupt,
    /// The scan could not run to completion (pool contention, cancellation, or
    /// the scan budget). The last completed scan stays authoritative.
    Transient,
}

/// Run outside HTTP handlers: full scans must not be triggered by public probes.
/// A completed scan reporting corruption is latched until restart, which
/// requires another successful startup check. A scan that cannot run to
/// completion (pool contention, budget exhaustion) leaves the last completed
/// scan authoritative and is retried on a short backoff, so readiness is only
/// degraded once INTEGRITY_STALE_AFTER has actually elapsed.
pub(crate) async fn monitor_integrity(state: AppState) {
    run_integrity_monitor(state, INTEGRITY_CADENCE).await
}

async fn run_integrity_monitor(state: AppState, cadence: IntegrityCadence) {
    // Startup already verified the database, so the first scan runs immediately.
    let mut delay = Duration::ZERO;
    let mut transient_retry: Option<Duration> = None;
    loop {
        if state.is_shutting_down() || state.is_corrupt() {
            return;
        }
        tokio::select! {
            _ = state.shutdown_signal() => return,
            _ = tokio::time::sleep(delay) => {}
        }
        if state.is_shutting_down() {
            return;
        }
        delay = match check_integrity(&state).await {
            IntegrityOutcome::Healthy | IntegrityOutcome::Corrupt => {
                transient_retry = None;
                cadence.healthy
            }
            IntegrityOutcome::Transient => {
                let next = transient_retry
                    .map_or(cadence.retry_start, |previous| previous.saturating_mul(2))
                    .min(cadence.retry_cap);
                transient_retry = Some(next);
                next
            }
        };
    }
}

/// One production monitor iteration, also used by real-SQLite regression tests.
pub(super) async fn check_integrity(state: &AppState) -> IntegrityOutcome {
    if state.is_corrupt() {
        return IntegrityOutcome::Corrupt;
    }
    let result =
        bounded_integrity_query(state, "PRAGMA integrity_check(1)", INTEGRITY_SCAN_BUDGET).await;
    let corrupt = match &result {
        Ok(result) => result != "ok",
        Err(error) => is_corruption_error(error),
    };
    if corrupt {
        state.runtime.corrupt.store(true, Ordering::Release);
        state
            .runtime
            .integrity_available
            .store(false, Ordering::Release);
        mark_integrity_checked(state);
        eprintln!("SQLite runtime integrity check detected corruption; recovery required");
        return IntegrityOutcome::Corrupt;
    }
    if result.is_ok() {
        state
            .runtime
            .integrity_available
            .store(true, Ordering::Release);
        mark_integrity_checked(state);
        IntegrityOutcome::Healthy
    } else {
        // Pool contention, a shutdown race, or a scan that exhausted its budget.
        // The last completed scan stays authoritative: its availability and
        // timestamp are untouched, so readiness is only degraded once
        // INTEGRITY_STALE_AFTER has actually elapsed.
        eprintln!("SQLite runtime integrity check unavailable");
        IntegrityOutcome::Transient
    }
}

fn mark_integrity_checked(state: &AppState) {
    *state
        .runtime
        .integrity_checked_at
        .lock()
        .expect("integrity timestamp lock poisoned") = Instant::now();
}

/// Own the pool permit until the native handler has been removed. Dropping a
/// query future alone does not stop SQLite's worker thread.
struct IntegrityConnection {
    connection: Option<sqlx::pool::PoolConnection<sqlx::Sqlite>>,
    cancelled: Arc<AtomicBool>,
    cleaning: bool,
}

impl IntegrityConnection {
    async fn release(mut self) -> Result<(), sqlx::Error> {
        // If cleanup itself is cancelled, Drop closes rather than recycles the
        // connection. No caller can inherit our progress handler.
        self.cleaning = true;
        self.connection
            .as_mut()
            .expect("integrity connection is owned")
            .lock_handle()
            .await?
            .remove_progress_handler();
        drop(self.connection.take());
        Ok(())
    }
}

impl Drop for IntegrityConnection {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        let Some(mut connection) = self.connection.take() else {
            return;
        };
        if self.cleaning {
            connection.close_on_drop();
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let cleanup = Self {
                connection: Some(connection),
                cancelled: Arc::clone(&self.cancelled),
                cleaning: true,
            };
            runtime.spawn(async move {
                let _ = cleanup.release().await;
            });
        } else {
            connection.close_on_drop();
        }
    }
}

/// Bound both pool acquisition and native VM execution on the existing exclusive
/// connection. A Tokio timeout alone cannot interrupt sqlite3_step. Progress
/// callbacks cannot bound a kernel I/O stall; freshness still fails closed then.
async fn bounded_integrity_query(
    state: &AppState,
    query: &str,
    budget: Duration,
) -> Result<String, sqlx::Error> {
    bounded_integrity_query_with_progress(
        state,
        query,
        budget,
        #[cfg(test)]
        None,
    )
    .await
}

async fn bounded_integrity_query_with_progress(
    state: &AppState,
    query: &str,
    budget: Duration,
    #[cfg(test)] mut started: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<String, sqlx::Error> {
    // Cancelling Pool::acquire can discard a connection acquired just before
    // cancellation. Never risk replacing the sole EXCLUSIVE connection merely
    // because this optional monitor ran out of its acquisition budget.
    let acquire_deadline = tokio::time::Instant::now() + INTEGRITY_ACQUIRE_TIMEOUT;
    let connection = loop {
        if state.is_shutting_down() || state.db().pool().is_closed() {
            return Err(sqlx::Error::PoolClosed);
        }
        if tokio::time::Instant::now() >= acquire_deadline {
            return Err(sqlx::Error::PoolTimedOut);
        }
        if let Some(connection) = state.db().pool().try_acquire() {
            break connection;
        }
        let retry_at =
            (tokio::time::Instant::now() + Duration::from_millis(10)).min(acquire_deadline);
        tokio::select! {
            _ = state.shutdown_signal() => return Err(sqlx::Error::PoolClosed),
            _ = tokio::time::sleep_until(retry_at) => {}
        }
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut owned = IntegrityConnection {
        connection: Some(connection),
        cancelled: Arc::clone(&cancelled),
        cleaning: false,
    };
    let deadline = Instant::now() + budget;
    let runtime = Arc::clone(&state.runtime);
    let connection = owned
        .connection
        .as_mut()
        .expect("integrity connection is owned");
    connection
        .lock_handle()
        .await?
        .set_progress_handler(1_000, move || {
            #[cfg(test)]
            if let Some(started) = started.take() {
                let _ = started.send(());
            }
            !cancelled.load(Ordering::Acquire)
                && !runtime.shutting_down.load(Ordering::Acquire)
                && Instant::now() < deadline
        });
    let result = sqlx::query_scalar::<_, String>(query)
        .fetch_one(&mut **connection)
        .await;
    // Do not release the only pool connection until SQLite has stopped and the
    // handler is removed, including SQLITE_INTERRUPT and other error paths.
    let cleanup = owned.release().await;
    match result {
        Err(error) => Err(error),
        Ok(value) => {
            cleanup?;
            Ok(value)
        }
    }
}

fn is_corruption_error(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|error| error.code())
        .and_then(|code| code.parse::<i32>().ok())
        // SQLite primary and extended SQLITE_CORRUPT / SQLITE_NOTADB codes.
        .is_some_and(|code| matches!(code & 0xff, 11 | 26))
}

#[cfg(test)]
mod monitor_tests {
    use super::*;
    use crate::auth::AuthConfig;
    use crate::database::{ServerDatabaseConfig, initialize};
    use crate::secrets::{create_pepper_file, load_pepper_file};

    // A native VM workload that cannot finish before either tested deadline.
    const LONG_QUERY: &str = "WITH RECURSIVE n(x) AS (
        VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 1000000000
    ) SELECT CAST(sum(x) AS TEXT) FROM n";

    async fn state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::TempDir::new().unwrap();
        let database = initialize(ServerDatabaseConfig::new(dir.path().join("server.db")))
            .await
            .unwrap();
        let pepper = dir.path().join("pepper");
        create_pepper_file(&pepper).unwrap();
        let auth = AuthConfig::development(
            load_pepper_file(&pepper).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        (dir, AppState::new(database, None, auth))
    }

    async fn assert_pool_reusable(state: &AppState) {
        // More than 1,000 instructions: this fails if an expired/cancelled
        // handler was returned to the pool, unlike a trivial SELECT 1.
        let value: i64 = tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query_scalar(
                "WITH RECURSIVE n(x) AS (
                VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 10000
            ) SELECT sum(x) FROM n",
            )
            .fetch_one(state.db().pool()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(value, 50_005_000);
        assert_eq!(state.db().pool().size(), 1);
    }

    #[tokio::test]
    async fn native_deadline_interrupts_and_releases_pool() {
        let (_dir, state) = state().await;
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            bounded_integrity_query(&state, LONG_QUERY, Duration::from_millis(20)),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("9")
        );
        assert!(!is_corruption_error(&error));
        assert_pool_reusable(&state).await;
        check_integrity(&state).await;
        assert!(state.integrity_healthy());
    }

    #[tokio::test]
    async fn shutdown_interrupts_active_native_query() {
        let (_dir, state) = state().await;
        let query_state = state.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            bounded_integrity_query_with_progress(
                &query_state,
                LONG_QUERY,
                Duration::from_secs(60),
                Some(started_tx),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(!task.is_finished());
        assert_eq!(state.db().pool().num_idle(), 0);
        state.begin_shutdown();
        let error = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(!is_corruption_error(&error));
        assert_pool_reusable(&state).await;
    }

    #[tokio::test]
    async fn aborted_query_cleans_handler_before_pool_reuse() {
        let (_dir, state) = state().await;
        // A connection-local sentinel also detects accidental replacement of
        // the exclusive connection during normal cancellation cleanup.
        sqlx::query("CREATE TEMP TABLE integrity_connection_sentinel (value INTEGER)")
            .execute(state.db().pool())
            .await
            .unwrap();
        let query_state = state.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            bounded_integrity_query_with_progress(
                &query_state,
                LONG_QUERY,
                Duration::from_secs(60),
                Some(started_tx),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(!task.is_finished());
        assert_eq!(state.db().pool().num_idle(), 0);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_pool_reusable(&state).await;
        sqlx::query("INSERT INTO integrity_connection_sentinel VALUES (1)")
            .execute(state.db().pool())
            .await
            .unwrap();
        check_integrity(&state).await;
        assert!(state.integrity_healthy());
    }

    #[tokio::test]
    async fn unavailable_acquisition_keeps_the_last_completed_scan_authoritative() {
        let (_dir, state) = state().await;
        let connection = state.db().pool().acquire().await.unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(2), check_integrity(&state))
            .await
            .unwrap();
        assert_eq!(outcome, IntegrityOutcome::Transient);
        // A transient failure must never latch readiness unavailable: the
        // startup scan (or the last completed scan) still stands.
        assert!(state.integrity_healthy());
        assert!(!state.is_corrupt());
        drop(connection);
        assert_eq!(check_integrity(&state).await, IntegrityOutcome::Healthy);
        assert!(state.integrity_healthy());
    }

    #[tokio::test]
    async fn monitor_retries_a_transient_acquire_failure_before_the_healthy_interval() {
        let (_dir, state) = state().await;
        // The healthy interval is deliberately an hour so a passing test can
        // only be explained by the monitor retrying the transient failure.
        let cadence = IntegrityCadence {
            healthy: Duration::from_secs(60 * 60),
            retry_start: Duration::from_millis(20),
            retry_cap: Duration::from_millis(200),
        };
        // Hold the sole connection across the monitor's immediate first scan,
        // so it exhausts its acquire budget: a transient failure, not corruption.
        let connection = state.db().pool().acquire().await.unwrap();
        let monitor = tokio::spawn({
            let state = state.clone();
            async move { run_integrity_monitor(state, cadence).await }
        });
        tokio::time::sleep(INTEGRITY_ACQUIRE_TIMEOUT + Duration::from_millis(250)).await;
        let before = *state.runtime.integrity_checked_at.lock().unwrap();
        drop(connection);

        // Readiness must not latch for the whole healthy interval: the monitor
        // has to come back and complete a scan as soon as the pool is free.
        let recovered = tokio::time::timeout(Duration::from_secs(3), async {
            while *state.runtime.integrity_checked_at.lock().unwrap() <= before {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        monitor.abort();
        assert!(
            recovered.is_ok(),
            "a transient acquire failure must not defer the next scan to the healthy interval"
        );
        assert!(state.integrity_healthy());
    }

    #[tokio::test]
    async fn physical_corruption_error_latches_runtime() {
        let (_dir, state) = state().await;
        check_integrity(&state).await;
        assert!(state.integrity_healthy());
        sqlx::raw_sql(
            "CREATE TABLE integrity_broken_page (value INTEGER);
            PRAGMA writable_schema=ON;
            UPDATE sqlite_schema SET rootpage=2147483647
                WHERE name='integrity_broken_page';
            PRAGMA writable_schema=RESET;",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        let error =
            bounded_integrity_query(&state, "PRAGMA integrity_check(1)", INTEGRITY_SCAN_BUDGET)
                .await
                .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("11")
        );
        assert!(is_corruption_error(&error));
        check_integrity(&state).await;
        assert!(state.is_corrupt());
        assert!(!state.integrity_healthy());
    }

    #[tokio::test]
    async fn recognizes_real_notadb_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("not-a-database.db");
        std::fs::write(&path, vec![b'x'; 4096]).unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(path);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let error = sqlx::query("PRAGMA integrity_check(1)")
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("26")
        );
        assert!(is_corruption_error(&error));
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LiveResponse {
    status: &'static str,
}

#[utoipa::path(
    get,
    path = "/health/live",
    tag = "system",
    responses((status = 200, description = "Event loop is responding", body = LiveResponse))
)]
pub async fn live() -> impl IntoResponse {
    Json(LiveResponse { status: "ok" })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadyState {
    Ready,
    NotReady,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadyComponent {
    name: String,
    status: ReadyState,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

impl ReadyComponent {
    fn ready(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            status: ReadyState::Ready,
            reason: None,
        }
    }

    fn not_ready(name: &str, reason: &'static str) -> Self {
        Self {
            name: name.to_owned(),
            status: ReadyState::NotReady,
            reason: Some(reason),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadyResponse {
    status: ReadyState,
    components: Vec<ReadyComponent>,
}

#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "system",
    responses(
        (status = 200, description = "Server is ready to serve", body = ReadyResponse),
        (status = 503, description = "Server is not ready", body = ReadyResponse),
    )
)]
pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let sqlite = if state.is_corrupt() {
        ReadyComponent::not_ready("sqlite", "integrity_check_failed")
    } else {
        match state.db().schema_version().await {
            Ok(version) if version >= crate::database::SERVER_SCHEMA_VERSION => {
                ReadyComponent::ready("sqlite")
            }
            Ok(_) => ReadyComponent::not_ready("sqlite", "migration_pending"),
            Err(_) => ReadyComponent::not_ready("sqlite", "unavailable"),
        }
    };
    let owner = match crate::auth::has_owner(state.db()).await {
        Ok(true) => ReadyComponent::ready("owner"),
        Ok(false) => ReadyComponent::not_ready("owner", "setup_required"),
        Err(_) => ReadyComponent::not_ready("owner", "unavailable"),
    };
    let web_assets = if state.web_assets_ready() {
        ReadyComponent::ready("web_assets")
    } else {
        ReadyComponent::not_ready("web_assets", "web_assets_missing")
    };

    let shutdown = if state.is_shutting_down() {
        ReadyComponent::not_ready("shutdown", "shutting_down")
    } else {
        ReadyComponent::ready("shutdown")
    };
    let workers = if state.critical_workers_healthy() {
        ReadyComponent::ready("critical_workers")
    } else {
        ReadyComponent::not_ready("critical_workers", "worker_unhealthy")
    };
    let corruption = if state.is_corrupt() {
        ReadyComponent::not_ready("corruption", "integrity_check_failed")
    } else if !state.integrity_healthy() {
        ReadyComponent::not_ready("corruption", "integrity_check_unavailable")
    } else {
        ReadyComponent::ready("corruption")
    };
    // A converted deployment that has not passed the coordinated cutover gate
    // is deliberately not ready: it serves read-only diagnostics only while the
    // operator completes the switch (issue #192).
    let cutover = if state.db().inventory_cutover().business_writes_blocked() {
        ReadyComponent::not_ready("inventory_cutover", "cutover_not_resumed")
    } else {
        ReadyComponent::ready("inventory_cutover")
    };

    let components = vec![
        sqlite, owner, web_assets, shutdown, workers, corruption, cutover,
    ];
    let ready = components
        .iter()
        .all(|component| component.status == ReadyState::Ready);
    let status_code = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status_code,
        Json(ReadyResponse {
            status: if ready {
                ReadyState::Ready
            } else {
                ReadyState::NotReady
            },
            components,
        }),
    )
}
