//! Bounded, interruptible SQLite integrity queries (issue #194).
//!
//! A full-database `PRAGMA integrity_check` is only meaningful offline: on the
//! single serving connection it cannot complete inside any useful budget, and
//! every attempt stalls ingestion (ADR 0008). Startup and Doctor still want a
//! verdict, so they run one with an explicit deadline and a progress handler
//! that can interrupt the native statement, and they report "exhausted"
//! distinctly from a failed or corrupt result.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sqlx::SqlitePool;

/// Outcome of one bounded integrity query.
#[derive(Debug)]
pub(crate) enum BoundedQuery {
    /// The statement ran to completion; carries SQLite's result string.
    Completed(String),
    /// The deadline passed and the progress handler interrupted the statement.
    Exhausted,
    /// The pool or SQLite returned an error before any verdict.
    Failed(sqlx::Error),
}

/// How long to wait for the single EXCLUSIVE connection before giving up.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Run one scalar query under a bounded acquisition and an interruptible
/// execution window. A Tokio timeout alone cannot interrupt `sqlite3_step`,
/// and the connection is not recycled until SQLite has stopped and the
/// progress handler is removed.
pub(crate) async fn bounded_scalar_query(
    pool: &SqlitePool,
    query: &str,
    budget: Duration,
) -> BoundedQuery {
    // Cancelling `Pool::acquire` can discard a connection acquired just before
    // cancellation, so acquisition is polled rather than cancelled.
    let acquire_deadline = Instant::now() + ACQUIRE_TIMEOUT;
    let connection = loop {
        if pool.is_closed() {
            return BoundedQuery::Failed(sqlx::Error::PoolClosed);
        }
        if let Some(connection) = pool.try_acquire() {
            break connection;
        }
        if Instant::now() >= acquire_deadline {
            return BoundedQuery::Failed(sqlx::Error::PoolTimedOut);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut owned = OwnedConnection {
        connection: Some(connection),
        cancelled: Arc::clone(&cancelled),
        cleaning: false,
    };
    let deadline = Instant::now() + budget;
    let expired = Arc::new(AtomicBool::new(false));
    let expired_flag = Arc::clone(&expired);
    let connection = owned
        .connection
        .as_mut()
        .expect("bounded query connection is owned");
    // The handle guard is scoped so it is dropped before the query borrows the
    // connection mutably; `set_progress_handler` persists on the connection.
    let handler = match connection.lock_handle().await {
        Ok(mut handle) => {
            handle.set_progress_handler(1_000, move || {
                if cancelled.load(Ordering::Acquire) {
                    return false;
                }
                if Instant::now() >= deadline {
                    expired_flag.store(true, Ordering::Release);
                    return false;
                }
                true
            });
            Ok(())
        }
        Err(error) => Err(error),
    };
    if let Err(error) = handler {
        return BoundedQuery::Failed(error);
    }
    let result = sqlx::query_scalar::<_, String>(query)
        .fetch_one(&mut **connection)
        .await;
    let exhausted = expired.load(Ordering::Acquire);
    let cleanup = owned.release().await;
    if exhausted {
        return BoundedQuery::Exhausted;
    }
    match result {
        Err(error) => BoundedQuery::Failed(error),
        Ok(value) => match cleanup {
            Err(error) => BoundedQuery::Failed(error),
            Ok(()) => BoundedQuery::Completed(value),
        },
    }
}

/// Own the pool permit until the native handler has been removed. Dropping the
/// query future alone does not stop SQLite's worker thread.
struct OwnedConnection {
    connection: Option<sqlx::pool::PoolConnection<sqlx::Sqlite>>,
    cancelled: Arc<AtomicBool>,
    cleaning: bool,
}

impl OwnedConnection {
    async fn release(mut self) -> Result<(), sqlx::Error> {
        // If cleanup itself is cancelled, Drop closes rather than recycles the
        // connection. No caller can inherit our progress handler.
        self.cleaning = true;
        self.connection
            .as_mut()
            .expect("bounded query connection is owned")
            .lock_handle()
            .await?
            .remove_progress_handler();
        drop(self.connection.take());
        Ok(())
    }
}

impl Drop for OwnedConnection {
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    // A native VM workload that cannot finish before the tested deadline.
    const LONG_QUERY: &str = "WITH RECURSIVE n(x) AS (
        VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 1000000000
    ) SELECT CAST(sum(x) AS TEXT) FROM n";

    async fn pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(dir.path().join("bounded-query.db"))
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        (dir, pool)
    }

    async fn assert_pool_reusable(pool: &SqlitePool) {
        // More than 1,000 instructions: this fails if an expired or cancelled
        // handler was returned to the pool, unlike a trivial SELECT 1.
        let value: i64 = tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query_scalar(
                "WITH RECURSIVE n(x) AS (
                    VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 10000
                ) SELECT sum(x) FROM n",
            )
            .fetch_one(pool),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(value, 50_005_000);
        assert_eq!(pool.size(), 1);
    }

    #[tokio::test]
    async fn completed_query_returns_its_value() {
        let (_dir, pool) = pool().await;
        match bounded_scalar_query(&pool, "PRAGMA quick_check(1)", Duration::from_secs(5)).await {
            BoundedQuery::Completed(value) => assert_eq!(value, "ok"),
            other => panic!("expected Completed, got {}", describe(&other)),
        }
    }

    #[tokio::test]
    async fn native_deadline_interrupts_and_releases_pool() {
        let (_dir, pool) = pool().await;
        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            bounded_scalar_query(&pool, LONG_QUERY, Duration::from_millis(20)),
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, BoundedQuery::Exhausted),
            "expected Exhausted, got {}",
            describe(&outcome)
        );
        assert_pool_reusable(&pool).await;
    }

    #[tokio::test]
    async fn aborted_query_cleans_handler_before_pool_reuse() {
        let (_dir, pool) = pool().await;
        // A connection-local sentinel also detects accidental replacement of
        // the exclusive connection during normal cancellation cleanup.
        sqlx::query("CREATE TEMP TABLE bounded_query_sentinel (value INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        let query_pool = pool.clone();
        let task = tokio::spawn(async move {
            bounded_scalar_query(&query_pool, LONG_QUERY, Duration::from_secs(60)).await
        });
        for _ in 0..200 {
            if pool.num_idle() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(pool.num_idle(), 0);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_pool_reusable(&pool).await;
        sqlx::query("INSERT INTO bounded_query_sentinel VALUES (1)")
            .execute(&pool)
            .await
            .unwrap();
    }

    fn describe(outcome: &BoundedQuery) -> &'static str {
        match outcome {
            BoundedQuery::Completed(_) => "Completed",
            BoundedQuery::Exhausted => "Exhausted",
            BoundedQuery::Failed(_) => "Failed",
        }
    }
}
