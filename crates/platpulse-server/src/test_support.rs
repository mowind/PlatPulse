//! The shared test-database helper (ADR 0013).
//!
//! Every Server test that needs a database gets it here instead of opening a
//! file of its own, so the Storage Backend under test is chosen in one place:
//! `PLATPULSE_TEST_STORAGE=sqlite|postgres` (default `sqlite`). Only SQLite is
//! implemented in this slice; selecting PostgreSQL fails the run rather than
//! silently falling back to SQLite.
//!
//! Compiled for this crate's unit tests and, through the `test-support`
//! feature, for its integration tests. A production build never contains it.

use std::path::Path;

use sqlx::AnyPool;
use sqlx::ConnectOptions;
use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
use sqlx::sqlite::SqliteConnectOptions;
use tempfile::TempDir;

use crate::database::{ServerDatabase, ServerDatabaseConfig};

/// The environment variable that selects the Storage Backend under test.
pub const TEST_STORAGE_ENV: &str = "PLATPULSE_TEST_STORAGE";

/// A Storage Backend the tests can run against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStorage {
    Sqlite,
}

impl TestStorage {
    /// The backend selected by [`TEST_STORAGE_ENV`]; SQLite when unset.
    ///
    /// # Panics
    ///
    /// When the variable names PostgreSQL (not implemented yet) or an unknown
    /// backend: a misconfigured run must fail, never test the wrong backend.
    pub fn from_env() -> Self {
        match std::env::var(TEST_STORAGE_ENV) {
            Err(std::env::VarError::NotPresent) => Self::Sqlite,
            Ok(value) => Self::parse(&value),
            Err(error) => panic!("{TEST_STORAGE_ENV} is not valid Unicode: {error}"),
        }
    }

    fn parse(value: &str) -> Self {
        match value.trim() {
            "" | "sqlite" => Self::Sqlite,
            "postgres" => panic!(
                "{TEST_STORAGE_ENV}=postgres: the PostgreSQL Storage Backend is not implemented yet (ADR 0013)"
            ),
            other => {
                panic!("{TEST_STORAGE_ENV}={other:?} is not a Storage Backend (sqlite|postgres)")
            }
        }
    }
}

/// A fresh, fully migrated Server database in its own private directory. The
/// directory must outlive the database.
pub async fn test_database() -> (TempDir, ServerDatabase) {
    let dir = TempDir::new().expect("create the test database directory");
    let database = test_database_in(dir.path()).await;
    (dir, database)
}

/// A fresh, fully migrated Server database whose SQLite file (if any) lives in
/// `dir`. Tests that keep more state next to the database (peppers, backup
/// artifacts, Web assets) use this form.
pub async fn test_database_in(dir: &Path) -> ServerDatabase {
    match TestStorage::from_env() {
        TestStorage::Sqlite => crate::database::initialize(test_config(dir))
            .await
            .expect("open the SQLite test database"),
    }
}

/// The database configuration [`test_database_in`] opens, for tests that need
/// to reopen the same database (restarts, offline operations).
pub fn test_config(dir: &Path) -> ServerDatabaseConfig {
    match TestStorage::from_env() {
        TestStorage::Sqlite => ServerDatabaseConfig::new(dir.join("server.db")),
    }
}

/// A one-connection SQLite pool with no Server migrations applied, behind the
/// backend-neutral pool type: at `path` (created when missing) or in memory.
/// For SQLite-specific tests that stage a historical schema themselves (for
/// example a migration's data conversion) and then call domain functions.
///
/// Such a test is SQLite-only by construction: it asserts the selected Storage
/// Backend is SQLite, so a PostgreSQL run cannot report it as passed (ADR 0013).
pub async fn unmigrated_sqlite_pool(path: Option<&Path>) -> AnyPool {
    match TestStorage::from_env() {
        TestStorage::Sqlite => {}
    }
    sqlx::any::install_default_drivers();
    let url = match path {
        Some(path) => SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .to_url_lossy()
            .to_string(),
        None => "sqlite::memory:".to_owned(),
    };
    let options = AnyConnectOptions::from_url(&url.parse().expect("a valid SQLite URL"))
        .expect("SQLite connect options");
    AnyPoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await
        .expect("open the unmigrated SQLite pool")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_is_the_default_and_explicit_choice() {
        assert_eq!(TestStorage::parse(""), TestStorage::Sqlite);
        assert_eq!(TestStorage::parse("sqlite"), TestStorage::Sqlite);
    }

    #[test]
    #[should_panic(expected = "not implemented yet")]
    fn postgres_fails_instead_of_falling_back() {
        TestStorage::parse("postgres");
    }

    #[test]
    #[should_panic(expected = "is not a Storage Backend")]
    fn an_unknown_backend_fails() {
        TestStorage::parse("mysql");
    }

    #[tokio::test]
    async fn the_helper_returns_a_migrated_database() {
        let (_dir, database) = test_database().await;
        assert_eq!(
            database.schema_version().await.unwrap(),
            crate::database::SERVER_SCHEMA_VERSION
        );
    }
}
