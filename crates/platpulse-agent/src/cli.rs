//! `platpulse-agent` CLI (design §8.2).
use std::io::IsTerminal;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use thiserror::Error;

use crate::collector::{FailClosedRpcAdapter, collect_and_persist};
use crate::config::{AgentConfig, AgentConfigFile, generate_node_id};
use crate::enroll::{EnrollError, enroll_agent};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "PlatPulse collector agent that monitors PlatON nodes on one Host"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Enroll this Agent with the Server and store its credential.
    Enroll(EnrollArgs),
    /// Generate and print a stable Node ID.
    GenerateNodeId,
    /// Validate the complete agent.toml and all Node declarations.
    ValidateConfig(ValidateConfigArgs),
    /// Collect one report using the configured production RPC transport.
    /// This build fails closed because no RPC transport is configured yet.
    CollectReport(CollectReportArgs),
    /// Validate and persist one immutable report before delivery.
    PersistReport(PersistReportArgs),
}

#[derive(Debug, Args)]
pub struct EnrollArgs {
    #[arg(long)]
    pub config: PathBuf,
}

#[derive(Debug, Args)]
pub struct ValidateConfigArgs {
    #[arg(long)]
    pub config: PathBuf,
}

#[derive(Debug, Args)]
pub struct CollectReportArgs {
    #[arg(long)]
    pub config: PathBuf,
}
#[derive(Debug, Args)]
pub struct PersistReportArgs {
    #[arg(long)]
    pub config: PathBuf,
    #[arg(long)]
    pub report: PathBuf,
}

#[derive(Debug, Error)]
pub enum AgentCliError {
    #[error(transparent)]
    Config(#[from] crate::config::AgentConfigError),
    #[error(transparent)]
    Enroll(#[from] EnrollError),
    #[error("failed to read the enrollment token from the TTY or stdin: {0}")]
    TokenInput(std::io::Error),
    #[error("collection failed: {0}")]
    Collection(String),
}

pub fn run_generate_node_id() {
    println!("{}", generate_node_id());
}

pub fn run_validate_config(args: &ValidateConfigArgs) -> Result<(), Box<AgentCliError>> {
    let file =
        AgentConfigFile::load(&args.config).map_err(|e| Box::new(AgentCliError::Config(e)))?;
    let validated = file
        .validate()
        .map_err(|e| Box::new(AgentCliError::Config(e)))?;
    println!(
        "Validated {} Node(s), inventory revision {}.",
        validated.inventory.nodes.len(),
        validated.inventory.revision
    );
    Ok(())
}

pub async fn run_collect_report(args: &CollectReportArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let digest = collect_and_persist(&config, &FailClosedRpcAdapter)
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    println!("Persisted immutable collected report (sha256 {digest}).");
    Ok(())
}

pub async fn run_persist_report(args: &PersistReportArgs) -> Result<(), AgentCliError> {
    let digest = crate::reporting::persist_report_from_config(&args.config, &args.report)
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    println!("Persisted immutable report (sha256 {digest}).");
    Ok(())
}

pub async fn run_enroll(args: &EnrollArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let token = read_enrollment_token().map_err(AgentCliError::TokenInput)?;
    if token.is_empty() {
        return Err(AgentCliError::Enroll(EnrollError::ServerRejected(
            "an enrollment token is required".to_owned(),
        )));
    }
    let enrolled = enroll_agent(&config, &token).await?;
    println!(
        "Enrolled agent {} (epoch {}).",
        enrolled.agent_id, enrolled.agent_epoch
    );
    println!(
        "Credential stored at {}.",
        enrolled.credential_path.display()
    );
    Ok(())
}

fn read_enrollment_token() -> Result<String, std::io::Error> {
    if std::io::stdin().is_terminal() {
        rpassword::prompt_password("Enrollment token: ")
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stdin_token_keeps_spaces_and_strips_only_line_endings() {
        assert_eq!("a b c".trim_end_matches(['\r', '\n']), "a b c");
        assert_eq!(
            "pp_enroll_x_abc\r\n".trim_end_matches(['\r', '\n']),
            "pp_enroll_x_abc"
        );
    }
}
