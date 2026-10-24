//! The PostgreSQL Storage Backend (ADR 0013, issue #255).
//!
//! Everything that only exists because the Server stores its data in
//! PostgreSQL lives here: the explicit connection settings (no URL/DSN, no
//! libpq environment, no '.pgpass'), the session advisory lock that makes one
//! Server the exclusive owner of a database, the startup checks (server
//! version, schema ownership, collation), and SQLSTATE classification. The
//! domain modules reach the database through the backend-neutral
//! [`sqlx::AnyPool`] owned by [`super::ServerDatabase`] and never see these
//! details.
//!
//! Ownership is held by a dedicated native connection, separate from the
//! single work connection of the pool. A heartbeat proves the session (and so
//! its advisory lock) is still alive. Once the heartbeat fails, ownership is
//! lost for good: the work connection may not reconnect, readiness reports the
//! loss and the Server stops; it never tries to reacquire the lock in-process,
//! because another Server may already own the database by then.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgDatabaseError, PgSslMode};
use sqlx::{AnyConnection, AnyPool, ConnectOptions as _, Connection};
use thiserror::Error;
use tokio::sync::{oneshot, watch};

use super::{POSTGRES_WORK_CONNECTIONS, REQUIRED_TABLES, ServerDatabaseError};

/// The name SQLx's Any driver reports for a PostgreSQL connection.
pub(super) const BACKEND_NAME: &str = "PostgreSQL";

/// The PostgreSQL migration source: one baseline at the shared logical schema
/// version, then one migration per later Server schema change.
pub static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("./migrations-postgres");

/// The oldest supported PostgreSQL major version, as 'server_version_num'.
pub(crate) const MIN_SERVER_VERSION_NUM: i64 = 150_000;

/// The session advisory lock key that marks the owning Server ("PlatPuls").
pub(crate) const OWNERSHIP_LOCK_KEY: i64 = 0x506c_6174_5075_6c73;

/// The 'application_name' every Server connection reports.
pub(crate) const APPLICATION_NAME: &str = "platpulse-server";

/// Default connect timeout for the ownership and work connections.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default interval of the ownership heartbeat.
pub const DEFAULT_OWNERSHIP_HEARTBEAT: Duration = Duration::from_secs(5);

/// Server-side TCP keepalive for every Server session, so a vanished Server
/// host releases its session (and the advisory lock) without waiting for the
/// operating system's two-hour default. Ignored on a Unix-domain socket.
const TCP_KEEPALIVE: [(&str, &str); 3] = [
    ("tcp_keepalives_idle", "30"),
    ("tcp_keepalives_interval", "10"),
    ("tcp_keepalives_count", "3"),
];

/// libpq environment variables SQLx still honors for a setting the Server
/// cannot override explicitly. Every other 'PG*' variable is shadowed by an
/// explicit setting, so it has no effect; these are refused at startup so the
/// environment can never silently change how the Server connects.
const UNCONTROLLED_LIBPQ_VARIABLES: [&str; 3] = ["PGSSLCERT", "PGSSLKEY", "PGOPTIONS"];

/// Upper bound on the PostgreSQL password file. A password file is one short
/// line; anything larger is refused instead of read into memory.
const MAX_PASSWORD_FILE_BYTES: u64 = 4096;

/// A PostgreSQL password read from the configured password file. Its `Debug`
/// form is redacted and it has no `Display`, so it cannot reach a log line or
/// an error message by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct PostgresPassword(String);

impl PostgresPassword {
    /// Wrap a password; an empty password is refused because SQLx would then
    /// fall back to '.pgpass'.
    pub fn new(password: impl Into<String>) -> Option<Self> {
        let password = password.into();
        (!password.is_empty()).then_some(Self(password))
    }

    /// Load the password from a private file the Server user owns.
    ///
    /// The file is opened with the shared sensitive-file policy (no symlinked
    /// ancestors, no followed final symlink, a regular file owned by the
    /// Server user, no group or world access). Its content is exactly one
    /// line, optionally ending in a newline. The password never travels
    /// through the configuration file itself or the environment.
    pub fn from_file(path: &Path) -> Result<Self, PostgresSecretError> {
        use std::io::Read as _;

        let mut file = crate::file_security::open_readonly(path).map_err(|reason| {
            PostgresSecretError::Unreadable {
                path: path.to_owned(),
                reason,
            }
        })?;
        let mut content = String::new();
        (&mut file)
            .take(MAX_PASSWORD_FILE_BYTES + 1)
            .read_to_string(&mut content)
            .map_err(|source| PostgresSecretError::Read {
                path: path.to_owned(),
                source,
            })?;
        if content.len() as u64 > MAX_PASSWORD_FILE_BYTES {
            return Err(PostgresSecretError::TooLong {
                path: path.to_owned(),
                limit: MAX_PASSWORD_FILE_BYTES,
            });
        }
        let password = content.trim_end_matches(['\n', '\r']);
        if password.contains(['\n', '\r']) {
            return Err(PostgresSecretError::MultipleLines {
                path: path.to_owned(),
            });
        }
        Self::new(password).ok_or_else(|| PostgresSecretError::Empty {
            path: path.to_owned(),
        })
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PostgresPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PostgresPassword(<redacted>)")
    }
}

/// Why the PostgreSQL password file could not be read. No variant carries the
/// file content, so a failure cannot leak the password into a log line.
#[derive(Debug, Error)]
pub enum PostgresSecretError {
    #[error("cannot read the PostgreSQL password file {path}: {reason}")]
    Unreadable { path: PathBuf, reason: String },
    #[error("failed to read PostgreSQL password file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("PostgreSQL password file {path} is larger than {limit} bytes")]
    TooLong { path: PathBuf, limit: u64 },
    #[error("PostgreSQL password file {path} must hold exactly one line")]
    MultipleLines { path: PathBuf },
    #[error("PostgreSQL password file {path} holds an empty password")]
    Empty { path: PathBuf },
}

/// Where the PostgreSQL server listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostgresHost {
    /// A host name or IP address reached over TCP.
    Tcp(String),
    /// The directory holding the server's Unix-domain socket.
    Socket(PathBuf),
}

/// The TLS policy of a PostgreSQL connection. There is deliberately no
/// opportunistic mode: TLS is either verified or explicitly disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostgresSslMode {
    /// No TLS. The operator configuration only accepts it for a Unix socket.
    Disable,
    /// Verify the certificate chain, not the host name.
    VerifyCa,
    /// Verify the certificate chain and the host name.
    VerifyFull,
}

impl PostgresSslMode {
    /// Parse the operator-facing `ssl_mode` name. Unknown names are refused by
    /// the caller instead of falling back to a permissive default.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "disable" => Some(Self::Disable),
            "verify-ca" => Some(Self::VerifyCa),
            "verify-full" => Some(Self::VerifyFull),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Disable => "disable",
            Self::VerifyCa => "verify-ca",
            Self::VerifyFull => "verify-full",
        }
    }

    fn as_sqlx(self) -> PgSslMode {
        match self {
            Self::Disable => PgSslMode::Disable,
            Self::VerifyCa => PgSslMode::VerifyCa,
            Self::VerifyFull => PgSslMode::VerifyFull,
        }
    }
}

/// Explicit connection settings for the PostgreSQL Storage Backend.
///
/// Every setting is given explicitly; nothing is read from a connection URL,
/// the libpq environment or '.pgpass'. `Debug` never shows the password.
#[derive(Debug, Clone)]
pub struct PostgresConnectConfig {
    host: PostgresHost,
    port: u16,
    database: String,
    user: String,
    password: PostgresPassword,
    ssl_mode: PostgresSslMode,
    ssl_root_cert: Option<PathBuf>,
    connect_timeout: Duration,
    ownership_heartbeat: Duration,
}

impl PostgresConnectConfig {
    /// Settings with the default TLS policy (verify-full), connect timeout
    /// and ownership heartbeat.
    pub fn new(
        host: PostgresHost,
        port: u16,
        database: impl Into<String>,
        user: impl Into<String>,
        password: PostgresPassword,
    ) -> Self {
        Self {
            host,
            port,
            database: database.into(),
            user: user.into(),
            password,
            ssl_mode: PostgresSslMode::VerifyFull,
            ssl_root_cert: None,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            ownership_heartbeat: DEFAULT_OWNERSHIP_HEARTBEAT,
        }
    }

    pub fn with_ssl_mode(mut self, ssl_mode: PostgresSslMode) -> Self {
        self.ssl_mode = ssl_mode;
        self
    }

    pub fn with_ssl_root_cert(mut self, path: impl Into<PathBuf>) -> Self {
        self.ssl_root_cert = Some(path.into());
        self
    }

    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Override how often the ownership connection proves it is alive.
    pub fn with_ownership_heartbeat(mut self, interval: Duration) -> Self {
        self.ownership_heartbeat = interval;
        self
    }

    /// The same settings pointed at another database (test template clones).
    pub fn with_database(mut self, database: impl Into<String>) -> Self {
        self.database = database.into();
        self
    }

    pub fn host(&self) -> &PostgresHost {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn ssl_mode(&self) -> PostgresSslMode {
        self.ssl_mode
    }

    pub fn ssl_root_cert(&self) -> Option<&Path> {
        self.ssl_root_cert.as_deref()
    }

    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// Native options for the ownership connection. Every field SQLx would
    /// otherwise take from the environment is set explicitly, and the
    /// password is always present, so '.pgpass' is never consulted.
    pub(crate) fn native_options(&self) -> PgConnectOptions {
        let options = PgConnectOptions::new_without_pgpass();
        let options = match &self.host {
            PostgresHost::Tcp(host) => options.host(host),
            PostgresHost::Socket(directory) => options.socket(directory),
        };
        let options = options
            .port(self.port)
            .username(&self.user)
            .password(self.password.expose())
            .database(&self.database)
            .ssl_mode(self.ssl_mode.as_sqlx())
            .application_name(APPLICATION_NAME)
            .options(TCP_KEEPALIVE);
        match &self.ssl_root_cert {
            Some(path) => options.ssl_root_cert(path),
            None => options,
        }
    }

    /// The connection URL handed to SQLx's Any driver for the work pool. It
    /// is built here from the explicit settings (never from operator input)
    /// and spells out every parameter the driver would otherwise take from
    /// the environment. It carries the password, so it never leaves this
    /// module except into the driver.
    fn any_url(&self) -> url::Url {
        let mut url = url::Url::parse("postgres://localhost").expect("static URL parses");
        let query_host = match &self.host {
            PostgresHost::Tcp(host) => host.clone(),
            PostgresHost::Socket(directory) => directory.display().to_string(),
        };
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("host", &query_host);
            query.append_pair("port", &self.port.to_string());
            query.append_pair("dbname", &self.database);
            query.append_pair("user", &self.user);
            query.append_pair("password", self.password.expose());
            query.append_pair("sslmode", self.ssl_mode.as_str());
            if let Some(path) = &self.ssl_root_cert {
                query.append_pair("sslrootcert", &path.display().to_string());
            }
            query.append_pair("application_name", APPLICATION_NAME);
            for (key, value) in TCP_KEEPALIVE {
                query.append_pair(&format!("options[{key}]"), value);
            }
        }
        url
    }

    /// The libpq variables in `environment` that would still change how the
    /// Server connects despite its explicit settings.
    pub fn uncontrolled_libpq_environment(
        &self,
        environment: impl Fn(&str) -> Option<String>,
    ) -> Vec<&'static str> {
        let mut found: Vec<&'static str> = UNCONTROLLED_LIBPQ_VARIABLES
            .into_iter()
            .filter(|name| environment(name).is_some())
            .collect();
        // An explicit root certificate replaces PGSSLROOTCERT; without one the
        // variable would pick the trust anchor.
        if self.ssl_root_cert.is_none() && environment("PGSSLROOTCERT").is_some() {
            found.push("PGSSLROOTCERT");
        }
        found
    }
}

/// The ownership state published by the heartbeat task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnershipState {
    Held,
    Lost,
    Released,
}

/// Runtime handle of the PostgreSQL backend: the ownership heartbeat and its
/// published state.
pub(super) struct PostgresBackend {
    state: watch::Receiver<OwnershipState>,
    /// Signalling end of the heartbeat task. The work pool holds a second
    /// reference, so the ownership connection lives exactly as long as the
    /// database handle *and* every pool clone: dropping one handle while
    /// another one still uses the pool must not release the advisory lock.
    release: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl PostgresBackend {
    /// Whether this process still owns the database.
    pub(super) fn ownership_held(&self) -> bool {
        *self.state.borrow() == OwnershipState::Held
    }

    /// Resolve once ownership is lost. A deliberate release never resolves it.
    pub(super) async fn ownership_lost(&self) {
        let mut state = self.state.clone();
        let outcome = state
            .wait_for(|state| *state != OwnershipState::Held)
            .await
            .map(|state| *state);
        if !matches!(outcome, Ok(OwnershipState::Lost)) {
            std::future::pending::<()>().await;
        }
    }

    /// Release the advisory lock and close the ownership connection.
    pub(super) async fn release(&self) {
        let release = self
            .release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(release) = release {
            let _ = release.send(());
        }
        let task = self
            .task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(task) = task {
            let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
        }
    }
}

/// Connect, take ownership, check the server, and open the work pool.
/// Migrations and the schema checks run afterwards in [`verify_startup`].
pub(super) async fn open(
    config: &PostgresConnectConfig,
) -> Result<(AnyPool, PostgresBackend), ServerDatabaseError> {
    let uncontrolled = config.uncontrolled_libpq_environment(|name| std::env::var(name).ok());
    if !uncontrolled.is_empty() {
        return Err(ServerDatabaseError::Unsupported(format!(
            "the libpq environment variable(s) {} would change how the Server connects to PostgreSQL; unset them (the Server takes every PostgreSQL setting from [storage.postgres] only)",
            uncontrolled.join(", ")
        )));
    }

    let mut ownership = connect_native(config).await?;
    if let Err(error) = check_server(&mut ownership).await {
        let _ = ownership.close().await;
        return Err(error);
    }
    let locked: bool = match sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(OWNERSHIP_LOCK_KEY)
        .fetch_one(&mut ownership)
        .await
    {
        Ok(locked) => locked,
        Err(error) => {
            let _ = ownership.close().await;
            return Err(ServerDatabaseError::Connect(error));
        }
    };
    if !locked {
        let _ = ownership.close().await;
        return Err(ServerDatabaseError::OwnershipHeld);
    }
    if let Err(error) = check_schema_owner(&mut ownership).await {
        let _ = ownership.close().await;
        return Err(error);
    }

    let (state_tx, state_rx) = watch::channel(OwnershipState::Held);
    let (release_tx, release_rx) = oneshot::channel();
    let task = tokio::spawn(hold_ownership(
        ownership,
        config.ownership_heartbeat,
        config.connect_timeout,
        state_tx,
        release_rx,
    ));
    let backend = PostgresBackend {
        state: state_rx,
        release: Arc::new(Mutex::new(Some(release_tx))),
        task: Mutex::new(Some(task)),
    };
    match connect_pool(config, backend.state.clone(), Arc::clone(&backend.release)).await {
        Ok(pool) => Ok((pool, backend)),
        Err(error) => {
            backend.release().await;
            Err(error)
        }
    }
}

async fn connect_native(
    config: &PostgresConnectConfig,
) -> Result<PgConnection, ServerDatabaseError> {
    match tokio::time::timeout(
        config.connect_timeout,
        PgConnection::connect_with(&config.native_options()),
    )
    .await
    {
        Ok(result) => result.map_err(ServerDatabaseError::Connect),
        Err(_) => Err(ServerDatabaseError::Connect(sqlx::Error::Io(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "PostgreSQL connect timed out"),
        ))),
    }
}

async fn connect_pool(
    config: &PostgresConnectConfig,
    ownership: watch::Receiver<OwnershipState>,
    release: Arc<Mutex<Option<oneshot::Sender<()>>>>,
) -> Result<AnyPool, ServerDatabaseError> {
    sqlx::any::install_default_drivers();
    let options =
        AnyConnectOptions::from_url(&config.any_url()).map_err(ServerDatabaseError::Connect)?;
    let before_acquire = ownership.clone();
    AnyPoolOptions::new()
        .max_connections(POSTGRES_WORK_CONNECTIONS)
        .min_connections(0)
        .acquire_timeout(config.connect_timeout)
        // A connection is only handed out while ownership is held; once it is
        // lost the pool closes the connection instead...
        .before_acquire(move |_, _| {
            // Holding the release channel here ties the ownership task to the
            // pool: while any pool handle exists the heartbeat keeps running.
            let _release = &release;
            let held = *before_acquire.borrow() == OwnershipState::Held;
            Box::pin(async move { Ok(held) })
        })
        // ...and a replacement (or any reconnect) is refused.
        .after_connect(move |_, _| {
            let held = *ownership.borrow() == OwnershipState::Held;
            Box::pin(async move {
                if held {
                    Ok(())
                } else {
                    Err(sqlx::Error::Configuration(
                        "PostgreSQL ownership is no longer held; refusing a work connection".into(),
                    ))
                }
            })
        })
        .connect_with(options)
        .await
        .map_err(ServerDatabaseError::Connect)
}

/// Keep the ownership session alive until it is released or fails.
async fn hold_ownership(
    mut connection: PgConnection,
    interval: Duration,
    timeout: Duration,
    state: watch::Sender<OwnershipState>,
    mut release: oneshot::Receiver<()>,
) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    loop {
        tokio::select! {
            // A deliberate release, or the last pool handle went away.
            _ = &mut release => {
                let _ = tokio::time::timeout(
                    timeout,
                    sqlx::query("SELECT pg_advisory_unlock($1)")
                        .bind(OWNERSHIP_LOCK_KEY)
                        .execute(&mut connection),
                )
                .await;
                let _ = tokio::time::timeout(timeout, connection.close()).await;
                state.send_replace(OwnershipState::Released);
                return;
            }
            _ = tick.tick() => {
                let alive = tokio::time::timeout(
                    timeout,
                    sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&mut connection),
                )
                .await;
                let failure = match alive {
                    Ok(Ok(_)) => continue,
                    Ok(Err(error)) => error.to_string(),
                    Err(_) => "heartbeat timed out".to_owned(),
                };
                tracing::error!(
                    error = %failure,
                    "PostgreSQL ownership connection lost; the Server stops without reacquiring ownership"
                );
                state.send_replace(OwnershipState::Lost);
                return;
            }
        }
    }
}

/// The version query startup validation and Doctor both read, so the two
/// cannot drift apart.
const SERVER_VERSION_SQL: &str =
    "SELECT current_setting('server_version_num'), current_setting('server_version')";

/// `server_version_num` as an integer. An unparsable value counts as 0 so the
/// comparison against the minimum still refuses a server it cannot read.
fn server_version_num(number: &str) -> i64 {
    number.trim().parse().unwrap_or(0)
}

/// Refuse a server older than the supported minimum.
async fn check_server(connection: &mut PgConnection) -> Result<(), ServerDatabaseError> {
    let (number, display): (String, String) = sqlx::query_as(SERVER_VERSION_SQL)
        .fetch_one(&mut *connection)
        .await
        .map_err(ServerDatabaseError::Connect)?;
    let number = server_version_num(&number);
    if number < MIN_SERVER_VERSION_NUM {
        return Err(ServerDatabaseError::Unsupported(format!(
            "PostgreSQL server version {display} is older than the supported minimum 15"
        )));
    }
    Ok(())
}

/// The Server role must own the schema it migrates (directly or through
/// membership, such as the database owner and 'pg_database_owner').
async fn check_schema_owner(connection: &mut PgConnection) -> Result<(), ServerDatabaseError> {
    let (role, schema, owner, owns): (String, Option<String>, Option<String>, Option<bool>) =
        sqlx::query_as(
            "SELECT current_user::text, n.nspname::text, pg_get_userbyid(n.nspowner)::text, \
                    pg_has_role(current_user, n.nspowner, 'USAGE') \
             FROM (SELECT current_schema() AS target) t \
             LEFT JOIN pg_namespace n ON n.nspname = t.target",
        )
        .fetch_one(&mut *connection)
        .await
        .map_err(ServerDatabaseError::Connect)?;
    match (schema, owns) {
        (Some(_), Some(true)) => Ok(()),
        (Some(schema), _) => Err(ServerDatabaseError::SchemaOwnership(format!(
            "role \"{role}\" does not own the target schema \"{schema}\" (owner: \"{}\"); make the Server role the schema owner, for example: ALTER SCHEMA {schema} OWNER TO {role}",
            owner.unwrap_or_default()
        ))),
        (None, _) => Err(ServerDatabaseError::SchemaOwnership(format!(
            "role \"{role}\" has no existing schema on its search_path; create the target schema owned by the Server role"
        ))),
    }
}

/// The server's reported version, as (server_version_num, server_version).
///
/// Doctor re-reads it on every run instead of caching the value the open path
/// validated, so an upgraded or downgraded server stays visible to the
/// Operator.
pub(super) async fn server_version(pool: &AnyPool) -> Result<(i64, String), sqlx::Error> {
    let (number, display): (String, String) =
        sqlx::query_as(SERVER_VERSION_SQL).fetch_one(pool).await?;
    Ok((server_version_num(&number), display))
}

/// Every text column that does not use the byte-order "C" collation, as
/// (table, column, collation), with an empty collation for the database
/// default. Startup refuses them; Doctor reports them (ADR 0013).
pub(super) async fn collation_violations(
    pool: &AnyPool,
) -> Result<Vec<(String, String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT c.relname::text, a.attname::text, COALESCE(co.collname::text, '') \
         FROM pg_attribute a \
         JOIN pg_class c ON c.oid = a.attrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_collation co ON co.oid = a.attcollation \
         WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p') \
           AND a.attnum > 0 AND NOT a.attisdropped \
           AND a.atttypid IN ('text'::regtype, 'varchar'::regtype, 'bpchar'::regtype) \
           AND c.relname <> '_sqlx_migrations' \
           AND COALESCE(co.collname::text, '') <> 'C' \
         ORDER BY 1, 2",
    )
    .fetch_all(pool)
    .await
}

/// The size of the connected database in bytes, from `pg_database_size`.
pub(super) async fn database_bytes(pool: &AnyPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT pg_database_size(current_database())")
        .fetch_one(pool)
        .await
}

/// The offending columns a Doctor detail names, capped so one report line
/// stays readable.
pub(super) fn describe_collation_violations(
    wrong: &[(String, String, String)],
    limit: usize,
) -> Vec<String> {
    wrong
        .iter()
        .take(limit)
        .map(|(table, column, collation)| {
            let collation = if collation.is_empty() {
                "default"
            } else {
                collation.as_str()
            };
            format!("{table}.{column} ({collation})")
        })
        .collect()
}

/// Checks after migrations: every table the Server needs exists, and every
/// text column uses the byte-order "C" collation the SQLite schema has.
pub(super) async fn verify_startup(pool: &AnyPool) -> Result<(), ServerDatabaseError> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables \
         WHERE table_schema = current_schema() AND table_type = 'BASE TABLE'",
    )
    .fetch_all(pool)
    .await
    .map_err(ServerDatabaseError::IntegrityQuery)?;
    let missing: Vec<&str> = REQUIRED_TABLES
        .iter()
        .copied()
        .filter(|table| !tables.iter().any(|present| present == table))
        .collect();
    if !missing.is_empty() {
        return Err(ServerDatabaseError::IntegrityFailed(format!(
            "required tables are missing: {}",
            missing.join(", ")
        )));
    }

    let wrong = collation_violations(pool)
        .await
        .map_err(ServerDatabaseError::IntegrityQuery)?;
    if !wrong.is_empty() {
        let columns = describe_collation_violations(&wrong, 10);
        return Err(ServerDatabaseError::Collation(format!(
            "{} text column(s) do not use COLLATE \"C\": {}",
            wrong.len(),
            columns.join(", ")
        )));
    }
    Ok(())
}

/// Whether PostgreSQL reported corrupt data: 'data_corrupted' (XX001) or
/// 'index_corrupted' (XX002). Connection and other errors never match.
pub(super) fn is_corruption_error(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.try_downcast_ref::<PgDatabaseError>())
        .is_some_and(|database| matches!(database.code(), "XX001" | "XX002"))
}

/// Defer every deferrable constraint (all foreign keys are declared
/// DEFERRABLE INITIALLY IMMEDIATE) to the commit of the current transaction.
pub(super) async fn defer_foreign_keys(connection: &mut AnyConnection) -> Result<(), sqlx::Error> {
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(connection)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PostgresConnectConfig {
        PostgresConnectConfig::new(
            PostgresHost::Tcp("db.example".to_owned()),
            5433,
            "platpulse db",
            "pp user",
            PostgresPassword::new("s3cr:et/@?&=").expect("non-empty"),
        )
    }

    #[test]
    fn debug_never_shows_the_password() {
        let rendered = format!("{:?}", config());
        assert!(!rendered.contains("s3cr"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    #[test]
    fn empty_password_is_refused() {
        assert!(PostgresPassword::new("").is_none());
    }

    #[test]
    fn work_url_round_trips_every_explicit_setting() {
        let config = config()
            .with_ssl_mode(PostgresSslMode::VerifyCa)
            .with_ssl_root_cert("/etc/platpulse/ca.pem");
        let url = config.any_url();
        let parsed: PgConnectOptions = url.as_str().parse().expect("valid URL");
        assert_eq!(parsed.get_host(), "db.example");
        assert_eq!(parsed.get_port(), 5433);
        assert_eq!(parsed.get_username(), "pp user");
        assert_eq!(parsed.get_database(), Some("platpulse db"));
        assert!(matches!(parsed.get_ssl_mode(), PgSslMode::VerifyCa));
        assert_eq!(parsed.get_application_name(), Some(APPLICATION_NAME));
        let options = parsed.get_options().unwrap_or_default();
        assert!(options.contains("tcp_keepalives_idle=30"), "{options}");
    }

    #[test]
    fn socket_url_selects_the_socket_directory() {
        let config = PostgresConnectConfig::new(
            PostgresHost::Socket(PathBuf::from("/run/postgresql")),
            5432,
            "platpulse",
            "platpulse",
            PostgresPassword::new("pw").expect("non-empty"),
        )
        .with_ssl_mode(PostgresSslMode::Disable);
        let parsed: PgConnectOptions = config.any_url().as_str().parse().expect("valid URL");
        assert_eq!(parsed.get_socket(), Some(&PathBuf::from("/run/postgresql")));
        assert!(matches!(parsed.get_ssl_mode(), PgSslMode::Disable));
    }

    #[test]
    fn uncontrolled_libpq_variables_are_reported() {
        let environment = |name: &str| {
            matches!(name, "PGSSLKEY" | "PGSSLROOTCERT" | "PGHOST" | "PGPASSWORD")
                .then(|| "x".to_owned())
        };
        assert_eq!(
            config().uncontrolled_libpq_environment(environment),
            vec!["PGSSLKEY", "PGSSLROOTCERT"]
        );
        // An explicit root certificate shadows PGSSLROOTCERT.
        assert_eq!(
            config()
                .with_ssl_root_cert("/ca.pem")
                .uncontrolled_libpq_environment(environment),
            vec!["PGSSLKEY"]
        );
    }

    #[cfg(unix)]
    fn password_file(content: &[u8]) -> (tempfile::TempDir, PathBuf) {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("postgres-password");
        let mut file = std::fs::File::create(&path).expect("create password file");
        file.write_all(content).expect("write password file");
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .expect("restrict password file");
        (dir, path)
    }

    #[cfg(unix)]
    #[test]
    fn password_file_is_read_without_its_trailing_newline() {
        let (_dir, path) = password_file(b"hunter2\n");
        let password = PostgresPassword::from_file(&path).expect("password");
        assert_eq!(password.expose(), "hunter2");
    }

    #[cfg(unix)]
    #[test]
    fn password_file_must_hold_exactly_one_line() {
        let (_dir, path) = password_file(b"one\ntwo\n");
        assert!(matches!(
            PostgresPassword::from_file(&path),
            Err(PostgresSecretError::MultipleLines { .. })
        ));
        let (_dir, path) = password_file(b"\n");
        assert!(matches!(
            PostgresPassword::from_file(&path),
            Err(PostgresSecretError::Empty { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn password_file_must_be_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_dir, path) = password_file(b"hunter2\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("widen password file");
        assert!(matches!(
            PostgresPassword::from_file(&path),
            Err(PostgresSecretError::Unreadable { .. })
        ));
    }
}
