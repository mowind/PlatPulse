//! `platpulse-server` CLI (design §18.2): `init`, `owner create`, and
//! `serve`. Binary crates keep a thin `main.rs`; all logic lives here so it
//! can be exercised from tests.

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use thiserror::Error;

use crate::auth::{
    AuthConfig, create_owner, create_viewer, hash_password, validate_password, validate_username,
};
use crate::config::{CliOverrides, ServerConfig};
use crate::database::{ServerDatabase, ServerDatabaseConfig, initialize};
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
    /// Run the HTTP Server.
    Serve(ServeArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// `server.toml` with at least `state_dir` (design §18.1).
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
            listen: args.listen,
            base_url: args.base_url.clone(),
            development: args.dev,
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

/// Run the HTTP Server: validate the listen address, load the pepper and
/// database with strict permission checks, and serve the API plus Web
/// assets until shutdown.
pub async fn run_serve(config: &ServerConfig) -> Result<(), Box<dyn std::error::Error>> {
    crate::init::restrict_umask();
    println!("{}", crate::startup_version_line());
    crate::validate_listen_address(config.listen)?;

    let pepper = load_pepper_file(&config.pepper_file)?;
    let auth = if config.development {
        AuthConfig::development(pepper, config.public_base_url.clone())
    } else {
        AuthConfig::production(pepper, config.public_base_url.clone())
    };

    let database = initialize(ServerDatabaseConfig::new(&config.db_path)).await?;
    crate::init::secure_database_file(&config.db_path)?;
    let state = crate::AppState::new(database, config.web_root.clone(), auth);
    let app = crate::http::build_app(state);

    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    println!("listening on {}", config.listen);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
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
