//! `platpulse-server` CLI (design §18.2): `init`, `owner create`, and
//! `serve`. Binary crates keep a thin `main.rs`; all logic lives here so it
//! can be exercised from tests.

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use thiserror::Error;

use crate::auth::{
    AuthConfig, create_owner, create_viewer, hash_password, validate_password, validate_username,
};
use crate::config::{CliOverrides, ServerConfig};
use crate::database::{ServerDatabase, ServerDatabaseConfig, initialize};
use crate::enrollment::{EnrollmentError, create_enrollment_token};
use crate::network::{NetworkError, create_network};
use crate::secrets::load_pepper_file;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "PlatPulse central ingestion, projection, and Web asset server"
)]
pub struct Cli {
    /// Print the OpenAPI 3 document as JSON and exit.
    #[arg(long)]
    pub print_openapi: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create the state directory, Server SQLite schema, and pepper file.
    Init(InitArgs),
    /// Owner account administration.
    #[command(subcommand)]
    Owner(OwnerCommand),
    /// Viewer account administration (design §12.1).
    #[command(subcommand)]
    Viewer(ViewerCommand),
    /// Network Registry administration (design §7.1).
    #[command(subcommand)]
    Network(NetworkCommand),
    /// Agent administration (design §18.2).
    #[command(subcommand)]
    Agent(AgentCommand),
    /// Create one sanitized backup artifact using the configured backup_dir.
    Backup(BackupArgs),
    /// Offline Restore (design §19/§20.2): the only path that may replace
    /// the database. Requires an exclusive stopped-Server condition, the
    /// artifact id, and a typed confirmation phrase (or explicit `--yes`).
    Restore(RestoreArgs),
    /// Run the HTTP Server.
    Serve(ServeArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// `server.toml` with at least `state_dir` (design §18.1).
    #[arg(long)]
    pub config: PathBuf,
}

#[derive(Debug, Args)]
pub struct BackupArgs {
    /// `server.toml` containing db_path, pepper_file, and backup_dir.
    #[arg(long)]
    pub config: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum OwnerCommand {
    /// Create an Owner; the password is read from the TTY or stdin only.
    Create(OwnerCreateArgs),
}

#[derive(Debug, Args)]
pub struct OwnerCreateArgs {
    /// `server.toml` (design §18.1).
    #[arg(long)]
    pub config: PathBuf,
    /// Username of the new Owner. Never pass a password on the command
    /// line: it is read from the TTY (hidden) or from stdin.
    #[arg(long)]
    pub username: String,
}

#[derive(Debug, Subcommand)]
pub enum ViewerCommand {
    /// Create a Viewer; the password is read from the TTY or stdin only.
    Create(ViewerCreateArgs),
}

#[derive(Debug, Args)]
pub struct ViewerCreateArgs {
    /// `server.toml` (design §18.1).
    #[arg(long)]
    pub config: PathBuf,
    /// Username of the new Viewer. Never pass a password on the command
    /// line: it is read from the TTY (hidden) or from stdin.
    #[arg(long)]
    pub username: String,
}

#[derive(Debug, Subcommand)]
pub enum NetworkCommand {
    /// Register a Network with its complete identity tuple (design §7.1).
    Create(NetworkCreateArgs),
}

#[derive(Debug, Args)]
pub struct NetworkCreateArgs {
    /// `server.toml` (design §18.1).
    #[arg(long)]
    pub config: PathBuf,
    /// Stable Registry key, e.g. `platon-mainnet`.
    #[arg(long)]
    pub key: String,
    /// Human-readable display name, e.g. `PlatON Mainnet`.
    #[arg(long)]
    pub display_name: String,
    /// Genesis block hash as `0x` + 64 lowercase hex nibbles.
    #[arg(long)]
    pub genesis_hash: String,
    /// Chain ID (non-negative, registry-bounded).
    #[arg(long)]
    pub chain_id: u64,
    /// P2P network ID (non-negative, registry-bounded).
    #[arg(long)]
    pub p2p_network_id: u64,
    /// Bech32 address HRP, e.g. `lat`.
    #[arg(long)]
    pub address_hrp: String,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Create a short-lived single-use Enrollment Token for a new Agent.
    CreateEnrollmentToken(EnrollmentTokenCreateArgs),
}

#[derive(Debug, Args)]
pub struct EnrollmentTokenCreateArgs {
    /// `server.toml` (design §18.1).
    #[arg(long)]
    pub config: PathBuf,
    /// Token lifetime in hours (1..=168; default 24). Short-lived by
    /// design (§4.5): the token is single-use and must not sit around.
    #[arg(long)]
    pub expires_in: Option<u64>,
}

#[derive(Debug, Args)]
pub struct RestoreArgs {
    /// `server.toml` (design §18.1).
    #[arg(long)]
    pub config: PathBuf,
    /// The backup artifact id to restore (identity selection).
    #[arg(long)]
    pub artifact_id: String,
    /// Explicit automation marker: skip the typed confirmation phrase
    /// (design §19: destructive commands require the confirm phrase;
    /// automation must pass `--yes`).
    #[arg(long)]
    pub yes: bool,
}

#[derive(Debug, Args, Default)]
pub struct ServeArgs {
    /// `server.toml`; individual flags below override its settings.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// SQLite database file for the Server.
    #[arg(long)]
    pub db_path: Option<PathBuf>,
    /// Pepper secret file (created by `init`).
    #[arg(long)]
    pub pepper_file: Option<PathBuf>,
    /// Directory containing the built WebUI (`index.html` plus hashed
    /// assets). Optional: the Server starts without Web assets and reports
    /// `web_assets_missing` from `/health/ready` instead.
    #[arg(long)]
    pub web_assets: Option<PathBuf>,
    /// Dedicated backup directory for Admin-triggered backup artifacts
    /// (design §20.1; never the Server state directory).
    #[arg(long)]
    pub backup_dir: Option<PathBuf>,
    /// Optional operator-provided GeoLite2 Country MMDB. The Server never
    /// downloads this file and never accepts credentials on the CLI.
    #[arg(long)]
    pub geo_mmdb: Option<PathBuf>,
    /// Address the HTTP listener binds to.
    #[arg(long)]
    pub listen: Option<SocketAddr>,
    /// Origin used for strict login validation, e.g.
    /// `https://platpulse.example.com`.
    #[arg(long)]
    pub base_url: Option<String>,
    /// Explicit development mode: separate non-`__Host-` cookie without
    /// `Secure` (design §19.1).
    #[arg(long)]
    pub dev: bool,
    /// PEM certificate chain for direct native Rustls HTTPS.
    #[arg(long)]
    pub tls_cert_chain: Option<PathBuf>,
    /// PEM private key for direct native Rustls HTTPS. The file must be a
    /// private regular file owned by the Server user.
    #[arg(long)]
    pub tls_private_key: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum HumanCreateError {
    #[error("{0}")]
    InvalidUsername(&'static str),
    #[error("{0}")]
    InvalidPassword(&'static str),
    #[error("failed to read the password from the TTY or stdin: {0}")]
    PasswordInput(std::io::Error),
    #[error("passwords do not match")]
    PasswordMismatch,
    #[error("failed to hash the password: {0}")]
    Hash(argon2::password_hash::Error),
    #[error(transparent)]
    Database(#[from] crate::database::ServerDatabaseError),
    #[error(transparent)]
    Identity(#[from] crate::auth::IdentityError),
}

/// Resolve settings for `owner create` and `serve` from the optional
/// config file plus CLI overrides.
pub fn resolve_serve_config(args: &ServeArgs) -> Result<ServerConfig, crate::config::ConfigError> {
    ServerConfig::resolve(
        args.config.as_deref(),
        &CliOverrides {
            db_path: args.db_path.clone(),
            pepper_file: args.pepper_file.clone(),
            web_root: args.web_assets.clone(),
            backup_dir: args.backup_dir.clone(),
            geo_mmdb: args.geo_mmdb.clone(),
            listen: args.listen,
            base_url: args.base_url.clone(),
            development: args.dev,
            tls_cert_chain_file: args.tls_cert_chain.clone(),
            tls_private_key_file: args.tls_private_key.clone(),
        },
    )
}

/// Create an Owner. The password is read exclusively from the TTY (hidden,
/// with confirmation) or from a secure stdin/fd; it never comes from argv
/// or a default (design §12.2).
pub async fn run_owner_create(
    config: &ServerConfig,
    username: &str,
) -> Result<(), HumanCreateError> {
    let (database, password_hash) = open_database_with_new_password(config, username).await?;
    let result = create_owner(&database, username, &password_hash).await;
    database.close().await;
    result.map_err(HumanCreateError::Identity)?;
    println!("Created owner '{username}'.");
    Ok(())
}

/// Create a Viewer with the same local provisioning baseline as an Owner:
/// the password is read exclusively from the TTY (hidden, with
/// confirmation) or from a secure stdin/fd, never from argv or a default,
/// and the creation writes its audit event (design §12.1). There is no
/// public or HTTP registration path.
pub async fn run_viewer_create(
    config: &ServerConfig,
    username: &str,
) -> Result<(), HumanCreateError> {
    let (database, password_hash) = open_database_with_new_password(config, username).await?;
    let result = create_viewer(&database, username, &password_hash).await;
    database.close().await;
    result.map_err(HumanCreateError::Identity)?;
    println!("Created viewer '{username}'.");
    Ok(())
}

/// Shared local provisioning steps for Owner/Viewer creation: strict
/// umask, username validation, password input from the TTY or stdin,
/// password validation, Argon2id hashing, and an initialized Server
/// database. The caller owns the database and must close it.
async fn open_database_with_new_password(
    config: &ServerConfig,
    username: &str,
) -> Result<(ServerDatabase, String), HumanCreateError> {
    crate::init::restrict_umask();
    validate_username(username).map_err(HumanCreateError::InvalidUsername)?;
    let password = read_password()?;
    validate_password(&password).map_err(HumanCreateError::InvalidPassword)?;

    let password_hash = hash_password(password.as_bytes()).map_err(HumanCreateError::Hash)?;
    let database = initialize(ServerDatabaseConfig::new(&config.db_path)).await?;
    Ok((database, password_hash))
}

/// Register a Network from the local CLI (design §7.1): the command
/// requires the complete identity tuple and writes the row plus its
/// minimal audit event in one transaction. The Server never derives
/// Registry entries from Agent input.
pub async fn run_network_create(
    config: &ServerConfig,
    args: &NetworkCreateArgs,
) -> Result<(), NetworkError> {
    crate::init::restrict_umask();
    let database = initialize(ServerDatabaseConfig::new(&config.db_path)).await?;
    let result = create_network(
        &database,
        &args.key,
        &args.display_name,
        &args.genesis_hash,
        args.chain_id,
        args.p2p_network_id,
        &args.address_hrp,
    )
    .await;
    database.close().await;
    let record = result?;
    println!(
        "Registered network '{}' ({}).",
        record.network_key, record.display_name
    );
    Ok(())
}

/// Create a single-use Enrollment Token from the local CLI (design §4.5,
/// §12.5). The plaintext token is printed exactly once and never stored;
/// the CLI output is the operator's only copy.
pub async fn run_create_enrollment_token(
    config: &ServerConfig,
    args: &EnrollmentTokenCreateArgs,
) -> Result<(), EnrollmentError> {
    crate::init::restrict_umask();
    let lifetime_hours = args.expires_in.unwrap_or(24);
    if !(1..=168).contains(&lifetime_hours) {
        return Err(EnrollmentError::InvalidLifetime(
            "enrollment token lifetime must be 1..=168 hours",
        ));
    }
    let lifetime = std::time::Duration::from_secs(lifetime_hours * 3600);

    let pepper = load_pepper_file(&config.pepper_file)?;
    let database = initialize(ServerDatabaseConfig::new(&config.db_path)).await?;
    let result = create_enrollment_token(&database, &pepper, None, lifetime).await;
    database.close().await;
    let record = result?;
    println!(
        "Enrollment token {} (single use, expires in {}h):",
        record.token_id, lifetime_hours
    );
    println!("{}", record.token);
    Ok(())
}

/// Offline Restore (design §19/§20.2, issue #51): the only path that may
/// replace the database. Refuses while a Server is running (exclusive
/// stopped-Server condition), re-validates identity/checksum/integrity/
/// schema, preserves the current database on every failure, never touches
/// secret files, and records the outcome into the restored database so the
/// WebUI can present it after the next Server start. Destructive commands
/// require the typed confirmation phrase; automation must pass `--yes`.
pub async fn run_restore(
    config: &ServerConfig,
    args: &RestoreArgs,
) -> Result<(), crate::restore::RestoreError> {
    crate::init::restrict_umask();
    let confirmation = if args.yes {
        crate::restore::Confirmation::Explicit
    } else {
        print!("Type the backup file name to confirm the restore: ");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let mut phrase = String::new();
        std::io::stdin()
            .read_line(&mut phrase)
            .map_err(crate::restore::RestoreError::Io)?;
        crate::restore::Confirmation::Phrase(phrase.trim().to_owned())
    };
    let outcome = crate::restore::apply(config, &args.artifact_id, confirmation).await?;
    if outcome.already_matching {
        println!(
            "Nothing to restore: the current database already matches backup '{}'.",
            outcome.filename
        );
        return Ok(());
    }
    println!(
        "Restored backup '{}' (schema {}).",
        outcome.filename, outcome.schema_version
    );
    for warning in &outcome.warnings {
        println!("warning: {warning}");
    }
    if let Some(safety) = &outcome.safety_copy {
        println!("Safety copy of the previous database: {}", safety.display());
    }
    if outcome.outcome_recorded {
        println!("Restore outcome recorded in the restored database.");
    } else {
        println!(
            "warning: the Restore outcome could not be recorded in the restored database; the restore itself completed."
        );
    }
    Ok(())
}

/// Create one sanitized backup using the same implementation as the Admin
/// backup Operation. The configured backup_dir remains the only destination.
pub async fn run_backup(config: &ServerConfig) -> Result<String, Box<dyn std::error::Error>> {
    crate::init::restrict_umask();
    let pepper = load_pepper_file(&config.pepper_file)?;
    let auth = if config.development {
        AuthConfig::development(pepper, config.public_base_url.clone())
    } else {
        AuthConfig::production(pepper, config.public_base_url.clone())
    };
    let database =
        ServerDatabase::open_existing(ServerDatabaseConfig::new(&config.db_path)).await?;
    let state =
        crate::http::AppState::new(database, None, auth).with_backup_dir(config.backup_dir.clone());
    Ok(crate::backup::create_scheduled(&state).await?)
}

/// Run the HTTP Server: validate the listen address, load the pepper and
/// database with strict permission checks, and serve the API plus Web
/// assets until shutdown.
pub async fn run_serve(config: &ServerConfig) -> Result<(), Box<dyn std::error::Error>> {
    crate::init::restrict_umask();
    println!("{}", crate::startup_version_line());
    let native_tls = match config.tls.as_ref() {
        Some(tls) => Some(crate::transport::load_rustls_config(tls).await?),
        None => None,
    };
    let listener_is_loopback = config.listen.ip().is_loopback();
    let trusted_proxy_cidrs: &[ipnet::IpNet] = if config.development && !listener_is_loopback {
        &[]
    } else {
        &config.trusted_proxy_cidrs
    };
    crate::validate_listen_address_with_transport(
        config.listen,
        native_tls.is_some(),
        trusted_proxy_cidrs,
        config.trusted_proxy_scheme.as_deref(),
    )?;

    // Exclusive stopped-Server detection (design §19, issue #51): the
    // serving process holds the database lock for its lifetime, so
    // `platpulse-server restore` can refuse while a Server is running, and
    // a second Server refuses to start over the same database.
    let _restore_lock = crate::restore::acquire_exclusive_lock(&config.db_path)?;

    let pepper = load_pepper_file(&config.pepper_file)?;
    let auth = if config.development {
        AuthConfig::development(pepper, config.public_base_url.clone())
    } else {
        AuthConfig::production(pepper, config.public_base_url.clone())
    };

    let database =
        crate::database::ServerDatabase::open_existing(ServerDatabaseConfig::new(&config.db_path))
            .await?;
    // Retention is a fixed, bounded startup task. Re-running after a crash is
    // safe: each invocation deletes at most one batch and never touches the
    // history state/coverage/evidence tables.
    if let Err(error) =
        crate::retention::cleanup_raw_block_summaries(database.pool(), crate::auth::now_utc()).await
    {
        eprintln!(
            "raw block retention cleanup deferred: {}",
            crate::redaction::redact_sensitive(&error.to_string())
        );
    }
    // Retention policies are seeded idempotently with the design §11.3
    // defaults; existing rows are never rewritten.
    if let Err(error) = crate::retention::ensure_seeded(database.pool()).await {
        eprintln!(
            "retention policy seeding deferred: {}",
            crate::redaction::redact_sensitive(&error.to_string())
        );
    }
    let geo_loader = std::sync::Arc::new(crate::geo::GeoLoader::new(config.geo.clone()));
    if config.geo.is_some() {
        let initial_geo_loader = std::sync::Arc::clone(&geo_loader);
        let _initial_load = tokio::task::spawn_blocking(move || initial_geo_loader.reload())
            .await
            .unwrap_or(false);
    }
    let mut state = crate::AppState::new_with_proxy_policy(
        database,
        config.web_root.clone(),
        auth,
        config.trusted_proxy_cidrs.clone(),
        config.trusted_proxy_scheme.clone(),
        config.notifications.clone(),
    )
    .with_backup_dir(config.backup_dir.clone())
    .with_geo_loader(geo_loader);
    if let Some(provider_config) = config.validator_provider.clone() {
        match crate::validator::PlatScanValidatorProvider::new(
            &provider_config.base_url,
            provider_config.networks.clone(),
            std::time::Duration::from_secs(provider_config.timeout_seconds),
        ) {
            Ok(provider) => {
                state = state.with_validator_provider(std::sync::Arc::new(provider));
            }
            Err(error) => eprintln!(
                "Validator Provider disabled: {}",
                crate::redaction::redact_sensitive(&error)
            ),
        }
    }
    // Development mode never touches a real provider: the e2e suite and
    // local development observe delivery state machines deterministically
    // through the fixed-failure double (production uses TelegramProvider).
    if config.development {
        state = state
            .with_delivery_provider(std::sync::Arc::new(crate::notifications::DevNullProvider));
    }
    let app = crate::http::build_app_with_native_tls(state.clone(), native_tls.is_some());
    state.metrics().set_listener_enabled(config.metrics.enabled);
    let mut metrics_handle = None;
    if config.metrics.enabled {
        let metrics_proxy_cidrs: &[ipnet::IpNet] =
            if config.development && !config.metrics.listen.ip().is_loopback() {
                &[]
            } else {
                &config.trusted_proxy_cidrs
            };
        if let Err(error) = crate::validate_listen_address_with_transport(
            config.metrics.listen,
            native_tls.is_some(),
            metrics_proxy_cidrs,
            config.trusted_proxy_scheme.as_deref(),
        ) {
            state.metrics().observe_listener_failure();
            return Err(Box::new(error));
        }
        match start_metrics_listener(&state, config.metrics.listen, native_tls.clone()) {
            Ok(handle) => metrics_handle = Some(handle),
            Err(error) => {
                state.metrics().observe_listener_failure();
                return Err(error);
            }
        }
        state.metrics().set_listener_ready(true);
    }

    let mut worker_handles = Vec::new();

    // Geo database reload and raw-IP cache cleanup are deliberately
    // best-effort. A malformed replacement keeps the last-good reader and
    // never interrupts report ingestion or readiness.
    {
        let geo_state = state.clone();
        worker_handles.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                if geo_state.is_shutting_down() {
                    break;
                }
                tokio::select! {
                    _ = geo_state.shutdown_signal() => break,
                    _ = tick.tick() => {}
                }
                if geo_state.is_shutting_down() {
                    break;
                }
                let before_geo = geo_state.geo().status();
                let geo_loader = std::sync::Arc::clone(geo_state.geo());
                let reload_changed =
                    tokio::task::spawn_blocking(move || geo_loader.reload_if_changed())
                        .await
                        .unwrap_or(false);
                let after_geo = geo_state.geo().status();
                let cleanup_changed = {
                    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
                    match crate::geo::cleanup_cache(geo_state.db().pool(), &now).await {
                        Ok(removed) => removed > 0,
                        Err(error) => {
                            eprintln!(
                                "geo cache cleanup deferred: {}",
                                crate::redaction::redact_sensitive(&error.to_string())
                            );
                            false
                        }
                    }
                };
                if reload_changed || before_geo != after_geo || cleanup_changed {
                    geo_state.admin_realtime().publish("geo", None::<String>, 1);
                    geo_state
                        .public_realtime()
                        .publish("geo", None::<String>, 1);
                }
            }
        }));
    }

    // Operations left `running` by a crash are honestly failed (issue #50,
    // webui.md §5.5); queued rows survive and the worker below picks them up.
    if let Err(error) = crate::operations::requeue_interrupted_operations(state.db().pool()).await {
        eprintln!(
            "operation re-arm deferred: {}",
            crate::redaction::redact_sensitive(&error.to_string())
        );
    }

    // Alert evaluation sweep: persists rule state and Incident transitions
    // for every active subject on a fixed cadence (design §17.2). Report
    // ingestion already evaluates its reported subjects in-transaction; the
    // sweep covers time-based facts and restores timers after restart. The
    // loop stops on shutdown and every sweep drains before the WAL
    // checkpoint.
    {
        let sweep_state = state.clone();
        worker_handles.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(crate::alerts::SWEEP_INTERVAL);
            loop {
                if sweep_state.is_shutting_down() {
                    break;
                }
                tokio::select! {
                    _ = sweep_state.shutdown_signal() => break,
                    _ = tick.tick() => {}
                }
                if sweep_state.is_shutting_down() {
                    break;
                }
                sweep_state.mark_critical_worker_heartbeat(0);
                match crate::alerts::sweep(&sweep_state).await {
                    Ok(changes) if changes > 0 => {
                        sweep_state
                            .admin_realtime()
                            .publish("alerts", None::<String>, 1);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!(
                            "alert evaluation sweep deferred: {}",
                            crate::redaction::redact_sensitive(&error.to_string())
                        );
                    }
                }
            }
        }));
    }

    // Notification delivery worker (design §17.4): sends due Outbox rows
    // through the configured channels with bounded retry/backoff and
    // Retry-After awareness, reaches DeadLetter after max attempts, and
    // re-arms Deliveries left in_flight by a crash. The loop stops on
    // shutdown; the worker publishes Admin invalidations only.
    {
        let worker_state = state.clone();
        worker_handles.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(crate::notifications::DELIVERY_INTERVAL);
            loop {
                if worker_state.is_shutting_down() {
                    break;
                }
                tokio::select! {
                    _ = worker_state.shutdown_signal() => break,
                    _ = tick.tick() => {}
                }
                if worker_state.is_shutting_down() {
                    break;
                }
                worker_state.mark_critical_worker_heartbeat(1);
                match crate::notifications::process_due_deliveries(
                    &worker_state,
                    &*worker_state.delivery_provider(),
                )
                .await
                {
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!(
                            "notification delivery deferred: {}",
                            crate::redaction::redact_sensitive(&error.to_string())
                        );
                    }
                }
            }
        }));
    }

    // Operations worker (issue #50, webui.md §5.5): advances one queued
    // retention/backup/Doctor Operation per tick in bounded steps. State is
    // persisted per step, so navigation, browser close, or SSE loss never
    // loses progress; SSE publishes only accelerate REST refetches.
    {
        let worker_state = state.clone();
        worker_handles.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(crate::operations::OPERATION_INTERVAL);
            loop {
                if worker_state.is_shutting_down() {
                    break;
                }
                tokio::select! {
                    _ = worker_state.shutdown_signal() => break,
                    _ = tick.tick() => {}
                }
                if worker_state.is_shutting_down() {
                    break;
                }
                worker_state.mark_critical_worker_heartbeat(2);
                match crate::operations::process_operations(&worker_state).await {
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!(
                            "operation worker deferred: {}",
                            crate::redaction::redact_sensitive(&error.to_string())
                        );
                    }
                }
            }
        }));
    }

    // Validator Provider refresh is Server-owned and deduplicated by the
    // registered Validator table, never by Node links. Provider failures are
    // persisted as diagnostics but do not enter Node health or readiness.
    if let Some(provider_config) = config.validator_provider.clone() {
        let provider_state = state.clone();
        worker_handles.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(
                provider_config.refresh_seconds,
            ));
            loop {
                if provider_state.is_shutting_down() {
                    break;
                }
                tokio::select! {
                    _ = provider_state.shutdown_signal() => break,
                    _ = tick.tick() => {}
                }
                if provider_state.is_shutting_down() {
                    break;
                }
                match crate::validator::refresh_all_with_channels_in_timezone(
                    provider_state.db(),
                    &*provider_state.validator_provider(),
                    provider_state.channels(),
                    &provider_config.timezone,
                )
                .await
                {
                    Ok(summary) if summary.invalidations > 0 || summary.alert_invalidations > 0 => {
                        if summary.invalidations > 0 {
                            for validator_id in &summary.invalidated_validator_ids {
                                provider_state.admin_realtime().publish(
                                    "validator",
                                    Some(validator_id.clone()),
                                    1,
                                );
                                provider_state.public_realtime().publish(
                                    "validator",
                                    Some(validator_id.clone()),
                                    1,
                                );
                            }
                            for network_key in &summary.invalidated_network_keys {
                                provider_state.admin_realtime().publish(
                                    "network",
                                    Some(network_key.clone()),
                                    1,
                                );
                                provider_state.public_realtime().publish(
                                    "network",
                                    Some(network_key.clone()),
                                    1,
                                );
                            }
                        }
                        if summary.alert_invalidations > 0 {
                            provider_state
                                .admin_realtime()
                                .publish("alerts", None::<String>, 1);
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!(
                            "Validator Provider refresh deferred: {}",
                            crate::redaction::redact_sensitive(&error.to_string())
                        );
                    }
                }
            }
        }));
    }

    println!("listening on {}", config.listen);
    if let Some(tls_config) = native_tls {
        let handle = axum_server::Handle::new();
        let server = axum_server::tls_rustls::bind_rustls(config.listen, tls_config)
            .handle(handle.clone())
            .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>());
        tokio::pin!(server);
        tokio::select! {
            result = &mut server => result?,
            _ = wait_for_shutdown_signal() => {
                state.begin_shutdown();
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                handle.graceful_shutdown(Some(Duration::from_secs(10)));
                let shutdown_result = tokio::time::timeout_at(deadline, async {
                    tokio::join!(&mut server, state.wait_for_ingestion(deadline))
                })
                .await;
                let (listener_stopped, drained) = match shutdown_result {
                    Ok((server_result, drained)) => (server_result.is_ok(), drained),
                    Err(_) => (false, false),
                };
                let workers_drained = drain_workers(&mut worker_handles, deadline).await;
                let metrics_drained = drain_metrics_listener(&mut metrics_handle, deadline).await;
                let checkpointed = state.checkpoint_wal().await.is_ok();
                state.db().close().await;
                if !listener_stopped || !drained || !workers_drained || !metrics_drained || !checkpointed {
                    return Err(format!("graceful shutdown incomplete (listener={listener_stopped}, drained={drained}, workers={workers_drained}, metrics={metrics_drained}, checkpointed={checkpointed})").into());
                }
            }
        }
    } else {
        let listener = tokio::net::TcpListener::bind(config.listen).await?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let server = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .into_future();
        tokio::pin!(server);
        tokio::select! {
            result = &mut server => result?,
            _ = wait_for_shutdown_signal() => {
                state.begin_shutdown();
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                let _ = shutdown_tx.send(());
                let shutdown_result = tokio::time::timeout_at(deadline, async {
                    tokio::join!(&mut server, state.wait_for_ingestion(deadline))
                })
                .await;
                let (listener_stopped, drained) = match shutdown_result {
                    Ok((server_result, drained)) => (server_result.is_ok(), drained),
                    Err(_) => (false, false),
                };
                let workers_drained = drain_workers(&mut worker_handles, deadline).await;
                let metrics_drained = drain_metrics_listener(&mut metrics_handle, deadline).await;
                let checkpointed = state.checkpoint_wal().await.is_ok();
                state.db().close().await;
                if !listener_stopped || !drained || !workers_drained || !metrics_drained || !checkpointed {
                    return Err(format!("graceful shutdown incomplete (listener={listener_stopped}, drained={drained}, workers={workers_drained}, metrics={metrics_drained}, checkpointed={checkpointed})").into());
                }
            }
        }
    }
    Ok(())
}

fn start_metrics_listener(
    state: &crate::AppState,
    listen: SocketAddr,
    tls: Option<axum_server::tls_rustls::RustlsConfig>,
) -> Result<tokio::task::JoinHandle<Result<(), String>>, Box<dyn std::error::Error>> {
    let listener =
        std::net::TcpListener::bind(listen).map_err(|_| "metrics listener bind failed")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "metrics listener setup failed")?;
    let require_trusted_proxy = !listen.ip().is_loopback() && tls.is_none();
    let app = crate::metrics::build_app(state, require_trusted_proxy);
    if let Some(tls_config) = tls {
        let server = axum_server::from_tcp_rustls(listener, tls_config)
            .map_err(|_| "metrics TLS listener could not be prepared")?;
        let handle = axum_server::Handle::new();
        let server = server
            .handle(handle.clone())
            .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>());
        let shutdown_state = state.clone();
        let metrics_state = state.clone();
        Ok(tokio::spawn(async move {
            tokio::pin!(server);
            let result = tokio::select! {
                result = &mut server => result.map_err(|_| "metrics TLS listener failed".to_owned()),
                _ = shutdown_state.shutdown_signal() => {
                    handle.graceful_shutdown(Some(Duration::from_secs(10)));
                    server.await.map_err(|_| "metrics TLS listener failed during shutdown".to_owned())
                }
            };
            if result.is_err() {
                metrics_state.metrics().observe_listener_failure();
                metrics_state.metrics().set_listener_ready(false);
            }
            result
        }))
    } else {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let shutdown_state = state.clone();
        let metrics_state = state.clone();
        let server = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            shutdown_state.shutdown_signal().await;
        })
        .into_future();
        Ok(tokio::spawn(async move {
            let result = server
                .await
                .map_err(|_| "metrics listener failed".to_owned());
            if result.is_err() {
                metrics_state.metrics().observe_listener_failure();
                metrics_state.metrics().set_listener_ready(false);
            }
            result
        }))
    }
}

async fn drain_metrics_listener(
    handle: &mut Option<tokio::task::JoinHandle<Result<(), String>>>,
    deadline: tokio::time::Instant,
) -> bool {
    let Some(handle) = handle.take() else {
        return true;
    };
    match tokio::time::timeout_at(deadline, handle).await {
        Ok(Ok(Ok(()))) => true,
        Ok(Ok(Err(_))) | Ok(Err(_)) | Err(_) => false,
    }
}

async fn drain_workers(
    workers: &mut [tokio::task::JoinHandle<()>],
    deadline: tokio::time::Instant,
) -> bool {
    let mut drained = true;
    for worker in workers.iter_mut() {
        match tokio::time::timeout_at(deadline, worker).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) | Err(_) => {
                drained = false;
                break;
            }
        }
    }
    if !drained {
        for worker in workers.iter() {
            worker.abort();
        }
    }
    drained
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn read_password() -> Result<String, HumanCreateError> {
    if std::io::stdin().is_terminal() {
        let first =
            rpassword::prompt_password("Password: ").map_err(HumanCreateError::PasswordInput)?;
        let second = rpassword::prompt_password("Confirm password: ")
            .map_err(HumanCreateError::PasswordInput)?;
        if first != second {
            return Err(HumanCreateError::PasswordMismatch);
        }
        Ok(first)
    } else {
        // Secure stdin/fd: the first line is the password; trailing line
        // endings are stripped, all other characters are kept verbatim.
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(HumanCreateError::PasswordInput)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stdin_password_keeps_spaces_and_strips_only_line_endings() {
        // read_password reads the real process stdin, which is not
        // injectable here; this test pins the trimming rule it applies.
        assert_eq!("a b c".trim_end_matches(['\r', '\n']), "a b c");
        assert_eq!("secret\r\n".trim_end_matches(['\r', '\n']), "secret");
        assert_eq!(" secret ".trim_end_matches(['\r', '\n']), " secret ");
    }
}
