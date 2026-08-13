//! `platpulse-agent` CLI (design §8.2).
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use thiserror::Error;

use crate::block::WebSocketBlockTransport;
use crate::collector::{FailClosedRpcAdapter, collect_and_persist_with_blocks};
use crate::config::{AgentConfig, AgentConfigFile, generate_node_id};
use crate::enroll::{EnrollError, enroll_agent};
use crate::rpc::AlloyRpcAdapter;

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
    /// Collect one immutable report using independent per-Node WebSocket
    /// subscriptions and hash-only block resolution.
    CollectReport(CollectReportArgs),
    /// Run the long-lived Agent runtime. Shutdown is cancellation-safe and
    /// persists a final Closing report instead of exiting after one sample.
    Run(RunArgs),
    /// Gracefully shut down after persisting a final immutable Closing report.
    Shutdown(ShutdownArgs),
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
pub struct RunArgs {
    #[arg(long)]
    pub config: PathBuf,
    #[arg(long, default_value_t = 5000)]
    pub drain_deadline_ms: u64,
    #[arg(long, default_value_t = 5000)]
    pub sender_deadline_ms: u64,
}

#[derive(Debug, Args)]
pub struct ShutdownArgs {
    #[arg(long)]
    pub config: PathBuf,
    /// Total sender deadline after the final report is stored.
    #[arg(long, default_value_t = 5000)]
    pub deadline_ms: u64,
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
    let adapter = AlloyRpcAdapter;
    crate::collector::recover_previous_boot(&config, &adapter)
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let validated = config
        .validated_inventory()
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let mut subscriptions = validated
        .inventory
        .nodes
        .iter()
        .map(|node| crate::block::HeadSubscription::new(node.node_id, 32))
        .collect::<Vec<_>>();
    let digest = collect_and_persist_with_blocks(
        &config,
        &adapter,
        &WebSocketBlockTransport::default(),
        &mut subscriptions,
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    println!("Persisted immutable collected report (sha256 {digest}).");
    Ok(())
}

pub async fn run_agent(args: &RunArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let adapter = AlloyRpcAdapter;
    crate::collector::recover_previous_boot(&config, &adapter)
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let validated = config
        .validated_inventory()
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let mut subscriptions = validated
        .inventory
        .nodes
        .iter()
        .map(|node| crate::block::HeadSubscription::new(node.node_id, 32))
        .collect::<Vec<_>>();
    let runtime = crate::shutdown::AgentRuntime::new();
    let transport = WebSocketBlockTransport::default();
    let mut delivery_tick = tokio::time::interval(Duration::from_secs(1));
    let signal = wait_for_shutdown_signal();
    tokio::pin!(signal);
    loop {
        // Delivery runs in its own 1s tick slot. The collection cycle must
        // never share the select with the tick: cancelling a collect at the
        // next tick can leave the spool and `agent_state` racing (a report
        // INSERTed under a stale sequence read while its state update is
        // still uncommitted), which surfaces as `UNIQUE constraint failed:
        // reports.boot_id, reports.report_sequence` and kills the Agent.
        tokio::select! {
            _ = &mut signal => {
                runtime.request_shutdown();
                break;
            }
            _ = delivery_tick.tick() => {
                let mut store = crate::database::AgentStore::open(
                    crate::database::AgentDatabaseConfig::new(&config.state_db),
                ).await.map_err(|error| AgentCliError::Collection(error.to_string()))?;
                let result = crate::reporting::deliver_periodic(
                    &mut store,
                    &crate::reporting::HttpReportTransport::from_config(&config)
                        .map_err(|error| AgentCliError::Collection(error.to_string()))?,
                    &crate::collector::SpoolPolicy::default(),
                ).await;
                store.close().await.map_err(|error| AgentCliError::Collection(error.to_string()))?;
                if let Err(error) = result {
                    eprintln!(
                        "Agent report delivery deferred: {}",
                        crate::redaction::redact_sensitive(&error.to_string())
                    );
                }
            }
        }
        // One full collection cycle, run to completion before the next
        // delivery slot. Only shutdown cancels it.
        tokio::select! {
            _ = &mut signal => {
                runtime.request_shutdown();
                break;
            }
            result = collect_and_persist_with_blocks(&config, &adapter, &transport, &mut subscriptions) => {
                result.map_err(|error| AgentCliError::Collection(error.to_string()))?;
            }
        }
    }
    let outcome = crate::shutdown::graceful_shutdown_with_subscriptions(
        &config,
        &adapter,
        &mut subscriptions,
        std::time::Duration::from_millis(args.drain_deadline_ms),
        std::time::Duration::from_millis(args.sender_deadline_ms),
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    println!(
        "Agent stopped with shutdown state {} (report {}).",
        outcome.shutdown_state, outcome.report_id
    );
    Ok(())
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

pub async fn run_shutdown(args: &ShutdownArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let outcome = crate::shutdown::graceful_shutdown(
        &config,
        &FailClosedRpcAdapter,
        std::time::Duration::from_millis(args.deadline_ms),
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    println!(
        "Stored final shutdown report {} (sequence {}, state {}).",
        outcome.report_id, outcome.report_sequence, outcome.shutdown_state
    );
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
