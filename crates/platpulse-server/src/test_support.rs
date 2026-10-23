//! The shared test-database helper (ADR 0013).
//!
//! Every Server test that needs a database gets it here instead of opening one
//! of its own, so the Storage Backend under test is chosen in one place:
//! `PLATPULSE_TEST_STORAGE=sqlite|postgres` (default `sqlite`).
//!
//! SQLite opens a fresh file per test. PostgreSQL needs
//! `PLATPULSE_TEST_POSTGRES_URL` (a TCP URL for a throwaway server) and
//! gives every test its own database cloned from one migrated template, dropped
//! again when the test ends. A PostgreSQL run never falls back to SQLite: an
//! absent or unreachable server fails the test that asked for it.
//!
//! A SQLite-only test carries its reason in an attribute, so a PostgreSQL run
//! lists it as `ignored` with that reason instead of passing it, and keeps the
//! guard for the case where the environment changed after the binary was built:
//!
//! ```ignore
//! #[cfg_attr(storage_postgres, ignore = "backup reads the SQLite file")]
//! #[test]
//! fn a_backup_test() {
//!     if !crate::test_support::skip_unless_sqlite("backup reads the SQLite file") {
//!         return;
//!     }
//! }
//! ```
//!
//! Compiled for this crate's unit tests and, through the `test-support`
//! feature, for its integration tests. A production build never contains it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
use sqlx::postgres::PgConnection;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{AnyPool, ConnectOptions, Connection};
use tempfile::TempDir;

use crate::database::{
    PostgresConnectConfig, PostgresHost, PostgresPassword, PostgresSslMode, ServerDatabase,
    ServerDatabaseConfig,
};

/// The environment variable that selects the Storage Backend under test.
pub const TEST_STORAGE_ENV: &str = "PLATPULSE_TEST_STORAGE";

/// The environment variable that carries the PostgreSQL test server URL.
pub const TEST_POSTGRES_URL_ENV: &str = "PLATPULSE_TEST_POSTGRES_URL";

/// The migrated template one run clones its per-test databases from.
const TEST_DATABASE_TEMPLATE: &str = "platpulse_test_template";

/// The per-test clone of `TEST_DATABASE_TEMPLATE`.
const TEST_DATABASE_CLONE: &str = "platpulse_test_clone";

/// How long a test waits for the PostgreSQL test server before failing.
const TEST_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the ownership connection of a test database proves it is alive.
const TEST_OWNERSHIP_HEARTBEAT: Duration = Duration::from_millis(200);

/// A Storage Backend the tests can run against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStorage {
    Sqlite,
    Postgres,
}

impl TestStorage {
    /// The backend selected by `TEST_STORAGE_ENV`; SQLite when unset.
    ///
    /// # Panics
    ///
    /// When the variable names an unknown backend: a misconfigured run must
    /// fail, never test the wrong backend.
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
            "postgres" => Self::Postgres,
            other => {
                panic!("{TEST_STORAGE_ENV}={other:?} is not a Storage Backend (sqlite|postgres)")
            }
        }
    }
}

/// Whether the selected Storage Backend can run this test.
///
/// A test that only makes sense against the SQLite file passes its reason here
/// and returns when the answer is `false`, so a PostgreSQL run records why
/// the test did not execute instead of failing it.
///
/// libtest has no skip state of its own: a SQLite-only test is marked
/// `#[cfg_attr(storage_postgres, ignore = "…")]`, which the build script sets
/// from `PLATPULSE_TEST_STORAGE`, so a PostgreSQL run lists it as `ignored`
/// with its reason instead of passing it (ADR 0013). The guard stays for the
/// case where the environment changed after the binary was built.
pub fn skip_unless_sqlite(reason: &str) -> bool {
    if TestStorage::from_env() == TestStorage::Sqlite {
        return true;
    }
    assert!(
        !reason.trim().is_empty(),
        "a skipped test must say why it is SQLite-only"
    );
    false
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
        // A PostgreSQL test database lives on the server, not in the
        // directory: the directory keeps holding whatever else the test keeps
        // next to its database. The directory still stands for the database, so
        // a test that closes its handle and asks for the same directory again
        // gets the same database back — that is the restart a Server test
        // exercises, and it makes the PostgreSQL run cover those tests instead
        // of skipping them.
        TestStorage::Postgres => match directory_database(dir) {
            Some(name) => postgres_reopen(&name).await,
            None => {
                let (name, database) = postgres_clone().await;
                remember_directory_database(dir, &name);
                database
            }
        },
    }
}

/// The PostgreSQL test database a directory stands for, if one was created.
fn directory_database(dir: &Path) -> Option<String> {
    DIRECTORY_DATABASES
        .lock()
        .expect("the test database directory map is not poisoned")
        .get(dir)
        .cloned()
}

/// Remember which PostgreSQL test database a directory stands for.
fn remember_directory_database(dir: &Path, name: &str) {
    DIRECTORY_DATABASES
        .lock()
        .expect("the test database directory map is not poisoned")
        .insert(dir.to_path_buf(), name.to_owned());
}

/// PostgreSQL test databases keyed by the directory that stands for them, so
/// reopening a directory reopens its database rather than cloning a second one.
static DIRECTORY_DATABASES: Mutex<BTreeMap<PathBuf, String>> = Mutex::new(BTreeMap::new());

/// The database configuration `test_database_in` opens, for SQLite tests that
/// need to reopen the same database (restarts, offline operations).
pub fn test_config(dir: &Path) -> ServerDatabaseConfig {
    match TestStorage::from_env() {
        TestStorage::Sqlite => ServerDatabaseConfig::new(dir.join("server.db")),
        TestStorage::Postgres => panic!(
            "test_config reopens the SQLite database file; a PostgreSQL test database is \
             created once per test (annotate the calling test with skip_unless_sqlite)"
        ),
    }
}

/// A one-connection SQLite pool with no Server migrations applied, behind the
/// backend-neutral pool type: at `path` (created when missing) or in memory.
/// For SQLite-specific tests that stage a historical schema themselves (for
/// example a migration's data conversion) and then call domain functions.
///
/// Such a test is SQLite-only by construction: a PostgreSQL run must annotate it
/// with `skip_unless_sqlite` rather than report it as passed (ADR 0013).
pub async fn unmigrated_sqlite_pool(path: Option<&Path>) -> AnyPool {
    assert!(
        skip_unless_sqlite("it stages a SQLite-only historical schema"),
        "unmigrated_sqlite_pool is SQLite-only: the calling test must check skip_unless_sqlite"
    );
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

/// Install a trigger that rejects the next matching write, so a test can prove
/// a transaction rolls back instead of half-applying.
///
/// The Storage Backends spell an aborting trigger differently: SQLite uses
/// `RAISE(ABORT, ...)` in the trigger body, PostgreSQL needs a PL/pgSQL
/// function the trigger calls. Both are ordinary database errors, so one test
/// body keeps covering both backends (ADR 0013).
///
/// `operation` is the trigger event with its optional column list, for example
/// `"INSERT"` or `"UPDATE OF params_json"`. Pair the call with
/// `drop_failure_trigger` when the test continues after the failed write.
pub async fn install_failure_trigger(
    pool: &AnyPool,
    name: &str,
    timing: &str,
    operation: &str,
    table: &str,
    message: &str,
) -> Result<(), sqlx::Error> {
    match TestStorage::from_env() {
        TestStorage::Sqlite => {
            let ddl = format!(
                "CREATE TRIGGER {name} {timing} {operation} ON {table} BEGIN SELECT RAISE(ABORT, '{}'); END",
                escape_sql_literal(message)
            );
            sqlx::query(&ddl).execute(pool).await?;
        }
        TestStorage::Postgres => {
            let function = format!("{name}_abort");
            sqlx::query(&format!(
                "CREATE OR REPLACE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS $$
                 BEGIN RAISE EXCEPTION '{}'; END $$",
                escape_sql_literal(message)
            ))
            .execute(pool)
            .await?;
            sqlx::query(&format!(
                "CREATE TRIGGER {name} {timing} {operation} ON {table} FOR EACH ROW EXECUTE FUNCTION {function}()"
            ))
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// Remove a trigger installed by `install_failure_trigger`.
pub async fn drop_failure_trigger(
    pool: &AnyPool,
    name: &str,
    table: &str,
) -> Result<(), sqlx::Error> {
    match TestStorage::from_env() {
        TestStorage::Sqlite => {
            sqlx::query(&format!("DROP TRIGGER {name}"))
                .execute(pool)
                .await?;
        }
        TestStorage::Postgres => {
            sqlx::query(&format!("DROP TRIGGER IF EXISTS {name} ON {table}"))
                .execute(pool)
                .await?;
            sqlx::query(&format!("DROP FUNCTION IF EXISTS {name}_abort()"))
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// One SQL string literal's worth of `value`: an inner quote is doubled, which
/// both backends accept.
fn escape_sql_literal(value: &str) -> String {
    value.replace('\'', "''")
}

/// A fresh database cloned from the process-wide migrated template.
async fn postgres_clone() -> (String, ServerDatabase) {
    let cluster = postgres_cluster().await;
    let (name, config) = cluster.clone_database().await;
    let database = crate::database::initialize(config)
        .await
        .unwrap_or_else(|error| panic!("open the cloned PostgreSQL test database {name}: {error}"));
    // Registered only once the database is open: a clone that never opened is
    // reclaimed by the next run's prune of abandoned databases.
    PostgresTestDatabase::register(&cluster.admin_url, &name);
    (name, database)
}

/// Reopen a test database a previous handle of the same test closed.
///
/// Closing is asynchronous on the ownership connection: a handle the test
/// dropped moments ago may still hold the session advisory lock, so the reopen
/// waits for it instead of failing the test on a race the Server never has.
async fn postgres_reopen(name: &str) -> ServerDatabase {
    let cluster = postgres_cluster().await;
    let config = cluster.base.clone().with_database(name.to_owned());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match crate::database::initialize(config.clone()).await {
            Ok(database) => return database,
            Err(error) => {
                let owned_elsewhere = error.to_string().contains("another Server owns");
                if !owned_elsewhere || std::time::Instant::now() >= deadline {
                    panic!("reopen the PostgreSQL test database {name}: {error}");
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }
}

/// A fresh PostgreSQL test database regardless of the selected Storage
/// Backend, or `None` when no PostgreSQL test server is configured.
///
/// The cross-backend schema equivalence check opens SQLite and PostgreSQL at
/// the same time, so it cannot follow `TEST_STORAGE_ENV`; it still clones a
/// fresh database per call so the check never observes another test's writes.
pub async fn optional_postgres_database() -> Option<ServerDatabase> {
    std::env::var_os(TEST_POSTGRES_URL_ENV)?;
    Some(postgres_clone().await.1)
}

/// A fresh, *empty* PostgreSQL test database handed over as the connect
/// configuration a Storage Backend Migration targets, or None when no
/// PostgreSQL test server is configured.
///
/// It is not a clone of the template: the migration only targets a database no
/// Server has ever opened, so this one is created without the migrated schema
/// and without the seeded catalog. The migration opens the target itself - it
/// must observe an empty database and commit a single transaction - so an
/// already-open ServerDatabase is not enough, and the caller needs the
/// configuration instead.
pub async fn optional_postgres_connect_config() -> Option<PostgresConnectConfig> {
    std::env::var_os(TEST_POSTGRES_URL_ENV)?;
    let cluster = postgres_cluster().await;
    let (database, config) = cluster.create_empty_database().await;
    PostgresTestDatabase::register(&cluster.admin_url, &database);
    Some(config)
}

/// Connection facts for a test that runs the real Server binary against a
/// fresh, already-migrated PostgreSQL database.
///
/// The clone is dropped when the test thread ends, exactly like
/// [`test_database`], so a subprocess test never leaks a database.
pub struct PostgresDeployment {
    admin_url: String,
    database: String,
    host: String,
    port: u16,
    user: String,
    password: String,
}

impl PostgresDeployment {
    /// The database name the Server must be configured with.
    pub fn database(&self) -> &str {
        &self.database
    }

    /// The TCP host of the test server.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The TCP port of the test server.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The role the Server must connect as.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// The password the Server must read from its password file.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Close every session on the database except this one.
    ///
    /// A test severs the Server's ownership connection this way: the Server
    /// must stop instead of reacquiring ownership in process.
    pub async fn terminate_backends(&self) {
        let mut admin = connect_admin(&self.admin_url).await;
        sqlx::query(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE datname = $1 AND pid <> pg_backend_pid()",
        )
        .bind(&self.database)
        .fetch_all(&mut admin)
        .await
        .expect("terminate the sessions of the PostgreSQL test database");
        let _ = admin.close().await;
    }
}

/// A fresh, already-migrated PostgreSQL database handed to the real Server
/// binary, or `None` when no PostgreSQL test server is configured.
pub async fn postgres_test_deployment() -> Option<PostgresDeployment> {
    std::env::var_os(TEST_POSTGRES_URL_ENV)?;
    let cluster = postgres_cluster().await;
    let (database, _config) = cluster.clone_database().await;
    // The clone carries the migrated schema already: the template was created
    // through the production startup path, so a Server opened on the clone only
    // has to connect, take the advisory lock, and verify the schema.
    PostgresTestDatabase::register(&cluster.admin_url, &database);
    let (host, port, user, password) = connection_facts(&cluster.admin_url);
    Some(PostgresDeployment {
        admin_url: cluster.admin_url.clone(),
        database,
        host,
        port,
        user,
        password,
    })
}

/// The host, port, role, and password of the test server URL.
fn connection_facts(url: &str) -> (String, u16, String, String) {
    let parsed = url::Url::parse(url).unwrap_or_else(|error| {
        panic!("{TEST_POSTGRES_URL_ENV} must be a PostgreSQL URL: {error}")
    });
    let host = parsed
        .host_str()
        .unwrap_or_else(|| panic!("{TEST_POSTGRES_URL_ENV} must name a TCP host"))
        .to_owned();
    let user = parsed.username().to_owned();
    let password = parsed.password().unwrap_or_else(|| {
        panic!(
            "{TEST_POSTGRES_URL_ENV} must carry a password: the Server always connects with \
             an explicit one and never reads '.pgpass'"
        )
    });
    (
        host,
        parsed.port().unwrap_or(5432),
        user,
        password.to_owned(),
    )
}

/// The migrated PostgreSQL template plus the settings its clones share.
struct PostgresCluster {
    /// The URL of the test server; its own database is the maintenance one.
    admin_url: String,
    /// The database name of the migrated template.
    template: String,
    /// Settings for the template; a clone swaps the database name.
    base: PostgresConnectConfig,
    /// Distinguishes the databases one test process creates.
    clones: AtomicU64,
}

/// The template of this test process, created on first use.
async fn postgres_cluster() -> &'static PostgresCluster {
    static CELL: tokio::sync::OnceCell<PostgresCluster> = tokio::sync::OnceCell::const_new();
    CELL.get_or_init(|| async { PostgresCluster::prepare().await })
        .await
}

impl PostgresCluster {
    /// Migrate one template database for the whole test process.
    async fn prepare() -> Self {
        let admin_url = std::env::var(TEST_POSTGRES_URL_ENV).unwrap_or_else(|_| {
            panic!(
                "{TEST_STORAGE_ENV}=postgres requires {TEST_POSTGRES_URL_ENV} \
                 (for example a postgres URL for a throwaway server); \
                 the test helper never falls back to SQLite"
            )
        });
        let base = server_settings(&admin_url);
        let mut admin = connect_admin(&admin_url).await;
        prune_abandoned_databases(&mut admin).await;
        let template = format!("{TEST_DATABASE_TEMPLATE}_{}", std::process::id());
        drop_database(&mut admin, &template).await;
        create_database(&mut admin, &template, None).await;
        let _ = admin.close().await;

        // Migrate and seed the template through the production path, so every
        // clone starts from exactly the database the Server expects.
        let template_config = base.clone().with_database(template.clone());
        crate::database::initialize(template_config)
            .await
            .unwrap_or_else(|error| {
                panic!("migrate the PostgreSQL test template {template}: {error}")
            })
            .close()
            .await;
        Self {
            admin_url,
            template,
            base,
            clones: AtomicU64::new(0),
        }
    }

    /// Clone a fresh database from the template for one test.
    async fn clone_database(&self) -> (String, PostgresConnectConfig) {
        // PostgreSQL refuses to copy a template another session is connected
        // to, so clones are created one at a time even while tests run in
        // parallel.
        static CLONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let clone_guard = CLONE.lock().await;
        let name = format!(
            "{TEST_DATABASE_CLONE}_{}_{}",
            std::process::id(),
            self.clones.fetch_add(1, Ordering::Relaxed)
        );
        let mut admin = connect_admin(&self.admin_url).await;
        create_database(&mut admin, &name, Some(&self.template)).await;
        let _ = admin.close().await;
        drop(clone_guard);
        let config = self.base.clone().with_database(name.clone());
        (name, config)
    }

    /// A new database with no schema and no seed, for the offline Storage
    /// Backend Migration whose target must be genuinely empty.
    async fn create_empty_database(&self) -> (String, PostgresConnectConfig) {
        static CREATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _create_guard = CREATE.lock().await;
        let name = format!(
            "{TEST_DATABASE_CLONE}_{}_{}",
            std::process::id(),
            self.clones.fetch_add(1, Ordering::Relaxed)
        );
        let mut admin = connect_admin(&self.admin_url).await;
        create_database(&mut admin, &name, None).await;
        let _ = admin.close().await;
        let config = self.base.clone().with_database(name.clone());
        (name, config)
    }
}

thread_local! {
    /// Test databases created by the current test thread, removed when the
    /// thread ends (that is: when the test finishes).
    static TEST_DATABASES: std::cell::RefCell<Vec<PostgresTestDatabase>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// A cloned PostgreSQL test database that removes itself when the test ends.
struct PostgresTestDatabase {
    admin_url: String,
    name: String,
}

impl PostgresTestDatabase {
    fn register(admin_url: &str, name: &str) {
        TEST_DATABASES.with(|databases| {
            databases.borrow_mut().push(Self {
                admin_url: admin_url.to_owned(),
                name: name.to_owned(),
            });
        });
    }
}

impl Drop for PostgresTestDatabase {
    fn drop(&mut self) {
        let admin_url = self.admin_url.clone();
        let name = self.name.clone();
        // Dropping a database needs a connection and `Drop` cannot be async.
        // The work runs on a thread of its own with its own runtime, so it does
        // not depend on the test's runtime still being there; joining it keeps
        // the cleanup deterministic (a crash only leaves the database for the
        // next run's prune).
        let cleanup = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                if let Ok(mut admin) = PgConnection::connect(&admin_url).await {
                    drop_database(&mut admin, &name).await;
                    let _ = admin.close().await;
                }
            });
        });
        let _ = cleanup.join();
    }
}

/// Connection settings derived from the test server URL.
fn server_settings(url: &str) -> PostgresConnectConfig {
    let parsed = url::Url::parse(url).unwrap_or_else(|error| {
        panic!("{TEST_POSTGRES_URL_ENV} must be a PostgreSQL URL: {error}")
    });
    let host = parsed
        .host_str()
        .unwrap_or_else(|| panic!("{TEST_POSTGRES_URL_ENV} must name a TCP host"))
        .to_owned();
    let user = parsed.username().to_owned();
    let database = match parsed.path().trim_start_matches('/') {
        "" => "postgres".to_owned(),
        database => database.to_owned(),
    };
    let password = parsed.password().unwrap_or_else(|| {
        panic!(
            "{TEST_POSTGRES_URL_ENV} must carry a password: the Server always connects with \
             an explicit one and never reads '.pgpass'"
        )
    });
    let password = PostgresPassword::new(password)
        .unwrap_or_else(|| panic!("{TEST_POSTGRES_URL_ENV} carries an empty password"));
    PostgresConnectConfig::new(
        PostgresHost::Tcp(host),
        parsed.port().unwrap_or(5432),
        database,
        user,
        password,
    )
    // A throwaway server in a test container serves plaintext connections; the
    // TLS policy itself is covered by the configuration tests.
    .with_ssl_mode(PostgresSslMode::Disable)
    .with_connect_timeout(TEST_CONNECT_TIMEOUT)
    .with_ownership_heartbeat(TEST_OWNERSHIP_HEARTBEAT)
}

/// Connect to the test server, failing the test when it cannot be reached.
async fn connect_admin(url: &str) -> PgConnection {
    match tokio::time::timeout(TEST_CONNECT_TIMEOUT, PgConnection::connect(url)).await {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => panic!(
            "cannot reach the PostgreSQL test server {}: {error}; \
             {TEST_STORAGE_ENV}=postgres never falls back to SQLite",
            display_url(url)
        ),
        Err(_) => panic!(
            "cannot reach the PostgreSQL test server {} within {TEST_CONNECT_TIMEOUT:?}",
            display_url(url)
        ),
    }
}

/// A test server URL with its password replaced, safe to put in a message.
fn display_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            if parsed.password().is_some() {
                let _ = parsed.set_password(Some("***"));
            }
            parsed.to_string()
        }
        Err(_) => "<unparsable PostgreSQL URL>".to_owned(),
    }
}

async fn create_database(admin: &mut PgConnection, name: &str, template: Option<&str>) {
    let statement = match template {
        Some(template) => format!("CREATE DATABASE {name} TEMPLATE {template}"),
        None => format!("CREATE DATABASE {name} TEMPLATE template0"),
    };
    sqlx::query(&statement)
        .execute(&mut *admin)
        .await
        .unwrap_or_else(|error| panic!("create the PostgreSQL test database {name}: {error}"));
}

async fn drop_database(admin: &mut PgConnection, name: &str) {
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
        .execute(&mut *admin)
        .await;
}

/// Drop the databases of test runs that died, so repeated local runs do not
/// pile up. A database is reclaimed only when the process that created it is
/// gone: another live run's databases are never touched.
async fn prune_abandoned_databases(admin: &mut PgConnection) {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT datname::text FROM pg_database WHERE datname LIKE 'platpulse\\_test\\_%'",
    )
    .fetch_all(&mut *admin)
    .await
    .unwrap_or_else(|error| panic!("list the PostgreSQL test databases: {error}"));
    for name in names {
        match database_owner_pid(&name) {
            Some(pid) if pid != std::process::id() && !process_exists(pid) => {
                drop_database(admin, &name).await;
            }
            _ => {}
        }
    }
}

/// The process id a test database name records.
fn database_owner_pid(name: &str) -> Option<u32> {
    let rest = name
        .strip_prefix(TEST_DATABASE_TEMPLATE)
        .or_else(|| name.strip_prefix(TEST_DATABASE_CLONE))?
        .strip_prefix('_')?;
    rest.split('_').next()?.parse().ok()
}

/// Whether a process id from another test run is still alive. Only Linux
/// exposes that without extra dependencies; elsewhere abandoned databases are
/// left in place.
fn process_exists(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
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
    fn postgres_is_a_selected_backend() {
        assert_eq!(TestStorage::parse("postgres"), TestStorage::Postgres);
        assert_eq!(TestStorage::parse(" postgres "), TestStorage::Postgres);
    }

    #[test]
    #[should_panic(expected = "is not a Storage Backend")]
    fn an_unknown_backend_fails() {
        TestStorage::parse("mysql");
    }

    #[test]
    fn a_database_name_records_its_owning_process() {
        assert_eq!(
            database_owner_pid("platpulse_test_template_4242"),
            Some(4242)
        );
        assert_eq!(
            database_owner_pid("platpulse_test_clone_4242_7"),
            Some(4242)
        );
        assert_eq!(database_owner_pid("platpulse_test_other"), None);
        assert_eq!(database_owner_pid("server"), None);
    }

    #[test]
    fn a_test_url_is_reported_without_its_password() {
        let shown = display_url("postgres://owner:secret@127.0.0.1:5432/postgres");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("owner"), "{shown}");
    }

    #[tokio::test]
    async fn the_helper_returns_a_migrated_database() {
        let (_dir, database) = test_database().await;
        assert_eq!(
            database.schema_version().await.unwrap(),
            crate::database::SERVER_SCHEMA_VERSION
        );
    }

    #[tokio::test]
    async fn every_test_gets_its_own_database() {
        let (_dir, first) = test_database().await;
        let (_other, second) = test_database().await;
        assert_ne!(
            first.pool() as *const AnyPool,
            second.pool() as *const AnyPool
        );
    }
}
