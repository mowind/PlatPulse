use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

use platpulse_server::database::{ServerDatabaseConfig, initialize};
use platpulse_server::http::{AppState, build_app};
use platpulse_server::startup_version_line;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "PlatPulse central ingestion, projection, and Web asset server"
)]
struct Cli {
    /// SQLite database file for the Server.
    #[arg(long, default_value = "platpulse.db")]
    db_path: PathBuf,
    /// Directory containing the built WebUI (`index.html` plus hashed
    /// assets). Optional: the Server starts without Web assets and reports
    /// `web_assets_missing` from `/health/ready` instead.
    #[arg(long)]
    web_assets: Option<PathBuf>,
    /// Address the HTTP listener binds to.
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    /// Print the OpenAPI 3 document as JSON and exit.
    #[arg(long)]
    print_openapi: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    if cli.print_openapi {
        println!("{}", platpulse_server::openapi::spec_json());
        return Ok(());
    }

    println!("{}", startup_version_line());
    platpulse_server::validate_listen_address(cli.listen)?;
    let database = initialize(ServerDatabaseConfig::new(&cli.db_path)).await?;
    let app = build_app(AppState::new(database, cli.web_assets));
    let listener = tokio::net::TcpListener::bind(cli.listen).await?;
    println!("listening on {}", cli.listen);
    axum::serve(listener, app).await?;
    Ok(())
}
