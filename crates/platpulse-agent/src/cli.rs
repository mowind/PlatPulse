//! `platpulse-agent` CLI (design §8.2).
use std::collections::HashMap;
#[cfg(test)]
use std::future::Future;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use thiserror::Error;

use crate::block::WebSocketBlockTransport;
use crate::collector::{
    FailClosedRpcAdapter, RpcSnapshot, collect_and_persist_precollected_in_store,
    collect_and_persist_with_blocks, run_node_block_worker,
};
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
    Config(#[from] Box<crate::config::AgentConfigError>),
    #[error(transparent)]
    Enroll(#[from] Box<EnrollError>),
    #[error("failed to read the enrollment token from the TTY or stdin: {0}")]
    TokenInput(std::io::Error),
    #[error("collection failed: {0}")]
    Collection(String),
}

impl From<crate::config::AgentConfigError> for AgentCliError {
    fn from(error: crate::config::AgentConfigError) -> Self {
        Self::Config(Box::new(error))
    }
}

impl From<EnrollError> for AgentCliError {
    fn from(error: EnrollError) -> Self {
        Self::Enroll(Box::new(error))
    }
}

pub fn run_generate_node_id() {
    println!("{}", generate_node_id());
}

pub fn run_validate_config(args: &ValidateConfigArgs) -> Result<(), Box<AgentCliError>> {
    let file = AgentConfigFile::load(&args.config).map_err(|e| Box::new(AgentCliError::from(e)))?;
    let validated = file
        .validate()
        .map_err(|e| Box::new(AgentCliError::from(e)))?;
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

fn periodic_interval(period: Duration) -> tokio::time::Interval {
    let mut tick = tokio::time::interval(period);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick
}

#[cfg(test)]
async fn run_periodic_until_cancel<F, Fut, E>(
    period: Duration,
    cancel: tokio_util::sync::CancellationToken,
    mut operation: F,
) -> Result<(), E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), E>>,
{
    let mut tick = periodic_interval(period);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = tick.tick() => {
                tokio::select! {
                    _ = cancel.cancelled() => return Ok(()),
                    result = operation() => result?,
                }
            }
        }
    }
}

async fn run_rpc_snapshot_worker(
    endpoint: platpulse_core::network::RpcEndpoint,
    period: Duration,
    snapshots: tokio::sync::watch::Sender<Option<Result<RpcSnapshot, String>>>,
    cancel: tokio_util::sync::CancellationToken,
) {
    loop {
        let probe = tokio::select! {
            _ = cancel.cancelled() => return,
            result = AlloyRpcAdapter.connect_live(&endpoint) => match result {
                Ok(probe) => probe,
                Err(error) => {
                    snapshots.send_replace(Some(Err(error.to_string())));
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                    }
                    continue;
                }
            }
        };
        let mut tick = periodic_interval(period);
        loop {
            let result = tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tick.tick() => probe.collect().await,
            };
            let reconnect = result.is_err();
            snapshots.send_replace(Some(result.map_err(|error| error.to_string())));
            if reconnect {
                break;
            }
        }
    }
}

type RpcSnapshotReceiver = (
    String,
    tokio::sync::watch::Receiver<Option<Result<RpcSnapshot, String>>>,
);

async fn run_report_collection_loop(
    config: AgentConfig,
    mut snapshots: Vec<RpcSnapshotReceiver>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), AgentCliError> {
    let mut store = crate::database::AgentStore::open(crate::database::AgentDatabaseConfig::new(
        &config.state_db,
    ))
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let mut tick = periodic_interval(Duration::from_secs(config.collection_interval_seconds));
    let result = loop {
        tokio::select! {
            _ = cancel.cancelled() => break Ok(()),
            _ = tick.tick() => {
                let current = snapshots
                    .iter_mut()
                    .map(|(endpoint, receiver)| {
                        let snapshot = receiver.borrow_and_update().clone().unwrap_or_else(|| {
                            Err("RPC snapshot collection is starting".to_owned())
                        });
                        (endpoint.clone(), snapshot)
                    })
                    .collect::<HashMap<_, _>>();
                let collection = tokio::select! {
                    _ = cancel.cancelled() => break Ok(()),
                    result = collect_and_persist_precollected_in_store(
                        &config,
                        current,
                        &mut store,
                    ) => result,
                };
                if let Err(error) = collection {
                    if crate::collector::is_transient_database_lock(&error) {
                        eprintln!(
                            "Agent report collection deferred: {}",
                            crate::redaction::redact_sensitive(&error.to_string())
                        );
                        continue;
                    }
                    break Err(AgentCliError::Collection(error.to_string()));
                }
            }
        }
    };
    let close_result = store
        .close()
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()));
    result.and(close_result)
}

async fn run_delivery_loop(
    config: AgentConfig,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), AgentCliError> {
    let mut store = crate::database::AgentStore::open(crate::database::AgentDatabaseConfig::new(
        &config.state_db,
    ))
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let transport = crate::reporting::HttpReportTransport::from_config(&config)
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let policy = crate::collector::SpoolPolicy::default();
    let mut tick = periodic_interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tick.tick() => {
                let result = tokio::select! {
                    _ = cancel.cancelled() => break,
                    result = crate::reporting::deliver_periodic(
                        &mut store,
                        &transport,
                        &policy,
                    ) => result,
                };
                if let Err(error) = result {
                    eprintln!(
                        "Agent report delivery deferred: {}",
                        crate::redaction::redact_sensitive(&error.to_string())
                    );
                }
            }
        }
    }
    store
        .close()
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))
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
    let node_ids = validated
        .inventory
        .nodes
        .iter()
        .map(|node| node.node_id)
        .collect::<Vec<_>>();
    let runtime = crate::shutdown::AgentRuntime::new();
    let cancel = runtime.cancellation_token();

    let delivery_worker = tokio::spawn(run_delivery_loop(config.clone(), cancel.clone()));

    let mut rpc_workers = tokio::task::JoinSet::new();
    let mut rpc_snapshots = Vec::with_capacity(validated.inventory.nodes.len());
    let rpc_period = Duration::from_secs(config.collection_interval_seconds);
    for node in &validated.inventory.nodes {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        rpc_snapshots.push((node.rpc_endpoint.as_str().to_owned(), receiver));
        rpc_workers.spawn(run_rpc_snapshot_worker(
            node.rpc_endpoint.clone(),
            rpc_period,
            sender,
            cancel.clone(),
        ));
    }

    let mut block_workers = tokio::task::JoinSet::new();
    for node in validated.inventory.nodes.iter().cloned() {
        block_workers.spawn(run_node_block_worker(
            config.clone(),
            node,
            WebSocketBlockTransport::default(),
            cancel.clone(),
        ));
    }

    let reports = run_report_collection_loop(config.clone(), rpc_snapshots, cancel.clone());
    tokio::pin!(reports);
    let signal = wait_for_shutdown_signal();
    tokio::pin!(signal);

    let mut subscriptions = Vec::with_capacity(node_ids.len());
    let runtime_result = tokio::select! {
        _ = &mut signal => Ok(()),
        result = &mut reports => result,
        exit = rpc_workers.join_next(), if !rpc_workers.is_empty() => {
            match exit {
                Some(Ok(())) => Err(AgentCliError::Collection(
                    "Node RPC snapshot worker stopped unexpectedly".to_owned(),
                )),
                Some(Err(error)) => Err(AgentCliError::Collection(format!(
                    "Node RPC snapshot worker failed: {error}"
                ))),
                None => Ok(()),
            }
        }
        exit = block_workers.join_next(), if !block_workers.is_empty() => {
            match exit {
                Some(Ok(exit)) => {
                    subscriptions.push(exit.subscription);
                    match exit.error {
                        Some(error) => Err(AgentCliError::Collection(error.to_string())),
                        None => Err(AgentCliError::Collection(
                            "Node block worker stopped unexpectedly".to_owned(),
                        )),
                    }
                }
                Some(Err(error)) => Err(AgentCliError::Collection(format!(
                    "Node block worker failed: {error}"
                ))),
                None => Ok(()),
            }
        }
    };
    runtime.request_shutdown();

    let block_deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(args.drain_deadline_ms);
    while !rpc_workers.is_empty() {
        match tokio::time::timeout_at(block_deadline, rpc_workers.join_next()).await {
            Ok(Some(Ok(()))) | Ok(None) => {}
            Ok(Some(Err(error))) => {
                eprintln!("Node RPC snapshot worker join failed during shutdown: {error}");
            }
            Err(_) => {
                rpc_workers.abort_all();
                break;
            }
        }
    }
    while !block_workers.is_empty() {
        match tokio::time::timeout_at(block_deadline, block_workers.join_next()).await {
            Ok(Some(Ok(exit))) => {
                if runtime_result.is_ok() {
                    if let Some(error) = exit.error {
                        eprintln!(
                            "Node block worker stopped with error during shutdown: {}",
                            crate::redaction::redact_sensitive(&error.to_string())
                        );
                    }
                }
                subscriptions.push(exit.subscription);
            }
            Ok(Some(Err(error))) => {
                eprintln!("Node block worker join failed during shutdown: {error}");
            }
            Ok(None) => break,
            Err(_) => {
                block_workers.abort_all();
                break;
            }
        }
    }
    for node_id in node_ids {
        if !subscriptions
            .iter()
            .any(|subscription| subscription.node_id() == node_id)
        {
            subscriptions.push(crate::block::HeadSubscription::new(node_id, 32));
        }
    }

    match tokio::time::timeout(
        std::time::Duration::from_millis(args.sender_deadline_ms),
        delivery_worker,
    )
    .await
    {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) if runtime_result.is_ok() => return Err(error),
        Ok(Ok(Err(_))) | Ok(Err(_)) | Err(_) => {}
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
    runtime_result
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
        return Err(AgentCliError::Enroll(Box::new(
            EnrollError::ServerRejected("an enrollment token is required".to_owned()),
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
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

    #[tokio::test(start_paused = true)]
    async fn report_and_delivery_workers_advance_on_independent_cadences() {
        let report_count = Arc::new(AtomicUsize::new(0));
        let delivery_count = Arc::new(AtomicUsize::new(0));
        let report_gate = Arc::new(Notify::new());
        let cancel = CancellationToken::new();

        let report_task = {
            let report_count = Arc::clone(&report_count);
            let report_gate = Arc::clone(&report_gate);
            let cancel = cancel.clone();
            tokio::spawn(run_periodic_until_cancel(
                Duration::from_secs(5),
                cancel,
                move || {
                    let invocation = report_count.fetch_add(1, Ordering::SeqCst);
                    let report_gate = Arc::clone(&report_gate);
                    async move {
                        if invocation == 0 {
                            report_gate.notified().await;
                        }
                        Ok::<(), ()>(())
                    }
                },
            ))
        };
        let delivery_task = {
            let delivery_count = Arc::clone(&delivery_count);
            let cancel = cancel.clone();
            tokio::spawn(run_periodic_until_cancel(
                Duration::from_secs(1),
                cancel,
                move || {
                    delivery_count.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<(), ()>(()) }
                },
            ))
        };

        tokio::task::yield_now().await;
        assert_eq!(report_count.load(Ordering::SeqCst), 1);
        assert_eq!(delivery_count.load(Ordering::SeqCst), 1);

        for _ in 0..3 {
            tokio::time::advance(Duration::from_secs(1)).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(report_count.load(Ordering::SeqCst), 1);
        assert_eq!(delivery_count.load(Ordering::SeqCst), 4);

        report_gate.notify_one();
        tokio::task::yield_now().await;
        for _ in 0..2 {
            tokio::time::advance(Duration::from_secs(1)).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(report_count.load(Ordering::SeqCst), 2);
        assert_eq!(delivery_count.load(Ordering::SeqCst), 6);

        cancel.cancel();
        assert!(report_task.await.unwrap().is_ok());
        assert!(delivery_task.await.unwrap().is_ok());
    }

    #[test]
    fn stdin_token_keeps_spaces_and_strips_only_line_endings() {
        assert_eq!("a b c".trim_end_matches(['\r', '\n']), "a b c");
        assert_eq!(
            "pp_enroll_x_abc\r\n".trim_end_matches(['\r', '\n']),
            "pp_enroll_x_abc"
        );
    }
}
