//! Agent runtime: supervises the long-lived workers of one Agent process.
//!
//! The CLI owns process start-up (configuration, runtime lock, boot recovery)
//! and exit-code policy. This module owns the workers, their failure policy,
//! shutdown ordering, and the final Closing report.

use std::collections::HashMap;
#[cfg(test)]
use std::future::Future;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use platpulse_core::identity::NodeId;
use tracing::Instrument as _;

use crate::block::WebSocketBlockTransport;
use crate::cli::AgentCliError;
use crate::collector::{
    RpcSnapshot, collect_and_persist_precollected_in_store, run_node_block_worker,
};
use crate::config::{AgentConfig, ValidatedAgentConfig};

/// Why the Agent runtime stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeExit {
    /// The runtime stopped normally and the Closing report was attempted.
    Stopped,
    /// Shutdown could not reach a safe point; the process must terminate
    /// immediately after flushing logs instead of dropping the Tokio runtime.
    FailClosed { stage: &'static str },
}

/// Everything the runtime needs from the CLI preflight.
pub(crate) struct RuntimeInput {
    pub config: AgentConfig,
    pub validated: ValidatedAgentConfig,
    pub node_ids: Vec<NodeId>,
    pub write_permit: crate::database::AgentStoreWritePermit,
    pub shutdown_signals: ShutdownSignals,
    pub drain_deadline: Duration,
    pub sender_deadline: Duration,
}

pub(crate) async fn run(input: RuntimeInput) -> Result<RuntimeExit, AgentCliError> {
    let RuntimeInput {
        config,
        validated,
        node_ids,
        write_permit,
        mut shutdown_signals,
        drain_deadline,
        sender_deadline,
    } = input;
    let cancel = tokio_util::sync::CancellationToken::new();

    let mut delivery_worker = tokio::spawn(run_delivery_loop(
        config.clone(),
        cancel.clone(),
        write_permit.clone(),
        sender_deadline,
    ));

    let mut rpc_workers = tokio::task::JoinSet::new();
    let mut rpc_snapshots = Vec::with_capacity(validated.inventory.nodes.len());
    let rpc_period = Duration::from_secs(config.collection_interval_seconds);
    for node in &validated.inventory.nodes {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        rpc_snapshots.push((node.rpc_endpoint.as_str().to_owned(), receiver));
        rpc_workers.spawn(
            run_rpc_snapshot_worker(
                node.rpc_endpoint.clone(),
                rpc_period,
                sender,
                cancel.clone(),
            )
            .instrument(node_span(&node.node_id)),
        );
    }

    let mut data_directory_workers = tokio::task::JoinSet::new();
    let mut data_directory_snapshots = Vec::with_capacity(validated.inventory.nodes.len());
    for node in &validated.inventory.nodes {
        let initial = if validated.data_directories.contains_key(&node.node_id) {
            crate::data_directory::starting_observations(crate::collector::timestamp())
        } else {
            crate::data_directory::disabled_observations()
        };
        let (sender, receiver) = tokio::sync::watch::channel(initial);
        data_directory_snapshots.push((node.node_id, receiver));
        if let Some(path) = validated.data_directories.get(&node.node_id) {
            data_directory_workers.spawn(
                run_data_directory_worker(path.clone(), sender, cancel.clone())
                    .instrument(node_span(&node.node_id)),
            );
        }
    }

    let mut block_workers = tokio::task::JoinSet::new();
    for node in validated.inventory.nodes.iter().cloned() {
        let span = node_span(&node.node_id);
        block_workers.spawn(
            run_node_block_worker(
                config.clone(),
                node,
                WebSocketBlockTransport::default(),
                cancel.clone(),
                write_permit.clone(),
            )
            .instrument(span),
        );
    }

    let shutdown_write_permit = write_permit.clone();
    let mut report_worker = tokio::spawn(run_report_collection_loop(
        config.clone(),
        rpc_snapshots,
        data_directory_snapshots,
        cancel.clone(),
        write_permit,
    ));
    let mut subscriptions = Vec::with_capacity(node_ids.len());
    let mut report_worker_finished = false;
    let mut delivery_worker_finished = false;
    let runtime_result = tokio::select! {
        _ = shutdown_signals.recv() => Ok(()),
        result = &mut report_worker => {
            report_worker_finished = true;
            match result {
                Ok(result) => result,
                Err(error) => Err(AgentCliError::Collection(format!(
                    "Agent report collection worker failed: {error}"
                ))),
            }
        },
        result = &mut delivery_worker => {
            delivery_worker_finished = true;
            match result {
                Ok(Ok(())) => Err(AgentCliError::Collection(
                    "Agent delivery worker stopped unexpectedly".to_owned(),
                )),
                Ok(Err(error)) => Err(error),
                Err(error) => Err(AgentCliError::Collection(format!(
                    "Agent delivery worker failed: {error}"
                ))),
            }
        },
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
        exit = data_directory_workers.join_next(), if !data_directory_workers.is_empty() => {
            match exit {
                Some(Ok(())) => Err(AgentCliError::Collection(
                    "Node data-directory worker stopped unexpectedly".to_owned(),
                )),
                Some(Err(error)) => Err(AgentCliError::Collection(format!(
                    "Node data-directory worker failed: {error}"
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
    cancel.cancel();

    let block_deadline = tokio::time::Instant::now() + drain_deadline;
    if !report_worker_finished {
        match tokio::time::timeout_at(block_deadline, &mut report_worker).await {
            Ok(_) => {}
            Err(_) => {
                tracing::warn!(
                    "Agent report collection worker join exceeded the shutdown deadline; aborting after the write fence"
                );
                let _shutdown_write_guard = match acquire_shutdown_write_fence(
                    &shutdown_write_permit,
                    block_deadline,
                    "report worker",
                )
                .await
                {
                    Ok(guard) => guard,
                    Err(exit) => return Ok(exit),
                };
                report_worker.abort();
                let _ = report_worker.await;
                drop(_shutdown_write_guard);
            }
        }
    }
    while !rpc_workers.is_empty() {
        match tokio::time::timeout_at(block_deadline, rpc_workers.join_next()).await {
            Ok(Some(Ok(()))) | Ok(None) => {}
            Ok(Some(Err(error))) => {
                tracing::warn!(error = %error, "Node RPC snapshot worker join failed during shutdown");
            }
            Err(_) => {
                rpc_workers.abort_all();
                while rpc_workers.join_next().await.is_some() {}
                break;
            }
        }
    }
    while !data_directory_workers.is_empty() {
        match tokio::time::timeout_at(block_deadline, data_directory_workers.join_next()).await {
            Ok(Some(Ok(()))) | Ok(None) => {}
            Ok(Some(Err(error))) => {
                tracing::warn!(error = %error, "Node data-directory worker join failed during shutdown");
            }
            Err(_) => {
                tracing::error!(
                    "Node data-directory scan exceeded the shutdown deadline; terminating fail-closed"
                );
                return Ok(RuntimeExit::FailClosed {
                    stage: "data-directory scan",
                });
            }
        }
    }
    let mut block_workers_timed_out = false;
    while !block_workers.is_empty() {
        match tokio::time::timeout_at(block_deadline, block_workers.join_next()).await {
            Ok(Some(Ok(exit))) => {
                if runtime_result.is_ok() {
                    if let Some(error) = exit.error {
                        tracing::warn!(
                            error = %error,
                            "Node block worker stopped with error during shutdown"
                        );
                    }
                }
                subscriptions.push(exit.subscription);
            }
            Ok(Some(Err(error))) => {
                tracing::warn!(error = %error, "Node block worker join failed during shutdown");
            }
            Ok(None) => break,
            Err(_) => {
                block_workers_timed_out = true;
                tracing::warn!(
                    "Node block worker join exceeded the shutdown deadline; aborting its supervisor"
                );
                let _shutdown_write_guard = match acquire_shutdown_write_fence(
                    &shutdown_write_permit,
                    block_deadline,
                    "block worker",
                )
                .await
                {
                    Ok(guard) => guard,
                    Err(exit) => return Ok(exit),
                };
                block_workers.abort_all();
                while block_workers.join_next().await.is_some() {}
                drop(_shutdown_write_guard);
                break;
            }
        }
    }
    if block_workers_timed_out {
        if let Err(error) =
            persist_abandoned_block_worker_gaps(&config, &node_ids, shutdown_write_permit.clone())
                .await
        {
            tracing::error!(error = %error, "Could not persist abandoned block worker gaps");
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

    let mut delivery_error = None;
    if !delivery_worker_finished {
        match tokio::time::timeout_at(block_deadline, &mut delivery_worker).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) if runtime_result.is_ok() => delivery_error = Some(error),
            Ok(Ok(Err(_))) | Ok(Err(_)) => {}
            Err(_) => {
                tracing::warn!(
                    "Agent delivery worker join exceeded the shutdown deadline; aborting after the write fence"
                );
                let _shutdown_write_guard = match acquire_shutdown_write_fence(
                    &shutdown_write_permit,
                    block_deadline,
                    "delivery worker",
                )
                .await
                {
                    Ok(guard) => guard,
                    Err(exit) => return Ok(exit),
                };
                delivery_worker.abort();
                let _ = delivery_worker.await;
                drop(_shutdown_write_guard);
            }
        }
    }

    let outcome = crate::shutdown::graceful_shutdown_with_permit(
        &config,
        &mut subscriptions,
        drain_deadline,
        sender_deadline,
        shutdown_write_permit,
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    tracing::info!(
        report_id = %outcome.report_id,
        outcome = %outcome.shutdown_state,
        "Agent stopped"
    );
    if let Some(error) = delivery_error {
        return Err(error);
    }
    runtime_result.map(|()| RuntimeExit::Stopped)
}

async fn acquire_shutdown_write_fence(
    write_permit: &crate::database::AgentStoreWritePermit,
    deadline: tokio::time::Instant,
    worker_name: &str,
) -> Result<tokio::sync::OwnedSemaphorePermit, RuntimeExit> {
    if let Some(permit) = write_permit.try_acquire() {
        return Ok(permit);
    }
    match tokio::time::timeout_at(deadline, write_permit.acquire()).await {
        Ok(permit) => Ok(permit),
        Err(_) => {
            tracing::error!(
                "Agent {worker_name} could not reach a write-safe shutdown point before the deadline; terminating fail-closed"
            );
            Err(RuntimeExit::FailClosed {
                stage: "shutdown write fence",
            })
        }
    }
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
            result = crate::rpc::AlloyRpcAdapter.connect_live(&endpoint) => match result {
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
type DataDirectoryReceiver = (
    NodeId,
    tokio::sync::watch::Receiver<crate::data_directory::DataDirectoryObservations>,
);

async fn run_data_directory_worker(
    path: PathBuf,
    observations: tokio::sync::watch::Sender<crate::data_directory::DataDirectoryObservations>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let mut tick = periodic_interval(crate::data_directory::DATA_DIRECTORY_SAMPLE_INTERVAL);
    loop {
        let attempted_at = tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tick.tick() => crate::collector::timestamp(),
        };
        let scan_path = path.clone();
        let scan_cancelled = Arc::new(AtomicBool::new(false));
        let scan_cancelled_for_thread = Arc::clone(&scan_cancelled);
        let scan_handle = match std::thread::Builder::new()
            .name("platon-data-directory-scan".to_owned())
            .spawn(move || {
                crate::data_directory::collect_observations_cancellable(
                    &scan_path,
                    attempted_at,
                    &scan_cancelled_for_thread,
                )
            }) {
            Ok(handle) => handle,
            Err(_) => {
                observations.send_replace(crate::data_directory::failed_observations(attempted_at));
                continue;
            }
        };
        let mut scan_join = tokio::task::spawn_blocking(move || scan_handle.join());
        let observation = tokio::select! {
            _ = cancel.cancelled() => {
                scan_cancelled.store(true, Ordering::Release);
                let _ = scan_join.await;
                return;
            }
            result = &mut scan_join => match result {
                Ok(Ok(observation)) => observation,
                Ok(Err(_)) | Err(_) => crate::data_directory::failed_observations(attempted_at),
            },
        };
        observations.send_replace(observation);
    }
}

/// How often a repeating operator message is repeated after its first
/// occurrence.
const REPEAT_MESSAGE_INTERVAL: Duration = Duration::from_secs(60);

/// Deduplicates a message that repeats until an operator fixes the condition
/// (issue #181: a drifted Inventory, or every report being refused).
///
/// The first occurrence prints immediately, the same message then prints at
/// most once per [REPEAT_MESSAGE_INTERVAL] with a running count, and a
/// different message is never delayed. Without this the Agent would either
/// flood the console every collection tick or leave the failure invisible.
struct RepeatingMessage {
    message: String,
    occurrences: u64,
    last_printed: Option<Instant>,
}

impl RepeatingMessage {
    fn new() -> Self {
        Self {
            message: String::new(),
            occurrences: 0,
            last_printed: None,
        }
    }

    /// Note one occurrence of message at now; returns the line to print
    /// when this occurrence is due.
    fn observe(&mut self, message: &str, now: Instant) -> Option<String> {
        if self.message != message {
            self.message = message.to_owned();
            self.occurrences = 1;
            self.last_printed = Some(now);
            return Some(self.message.clone());
        }
        self.occurrences += 1;
        let due = self
            .last_printed
            .is_none_or(|last| now.duration_since(last) >= REPEAT_MESSAGE_INTERVAL);
        if !due {
            return None;
        }
        self.last_printed = Some(now);
        Some(format!(
            "{} (repeated {} times)",
            self.message, self.occurrences
        ))
    }

    /// Forget the condition: a later recurrence is a new first occurrence.
    fn clear(&mut self) {
        self.message.clear();
        self.occurrences = 0;
        self.last_printed = None;
    }
}

async fn run_report_collection_loop(
    config: AgentConfig,
    mut snapshots: Vec<RpcSnapshotReceiver>,
    mut data_directories: Vec<DataDirectoryReceiver>,
    cancel: tokio_util::sync::CancellationToken,
    write_permit: crate::database::AgentStoreWritePermit,
) -> Result<(), AgentCliError> {
    let mut store = crate::database::AgentStore::open_with_write_permit(
        crate::database::AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let mut tick = periodic_interval(Duration::from_secs(config.collection_interval_seconds));
    let mut inventory_conflict = RepeatingMessage::new();
    // A deferred collection repeats every tick until its cause clears, so it is
    // summarized like the other repeating conditions (issue #250).
    let mut deferrals = RepeatingMessage::new();
    let result = loop {
        tokio::select! {
            _ = cancel.cancelled() => break Ok(()),
            _ = tick.tick() => {
                if cancel.is_cancelled() {
                    break Ok(());
                }
                let current = snapshots
                    .iter_mut()
                    .map(|(endpoint, receiver)| {
                        let snapshot = receiver.borrow_and_update().clone().unwrap_or_else(|| {
                            Err("RPC snapshot collection is starting".to_owned())
                        });
                        (endpoint.clone(), snapshot)
                    })
                    .collect::<HashMap<_, _>>();
                let current_data_directories = data_directories
                    .iter_mut()
                    .map(|(node_id, receiver)| (*node_id, receiver.borrow_and_update().clone()))
                    .collect::<HashMap<_, _>>();
                let collection = collect_and_persist_precollected_in_store(
                    &config,
                    current,
                    current_data_directories,
                    &mut store,
                )
                .await;
                if let Err(error) = collection {
                    // A refused Inventory declaration is an operator error that
                    // repeats every tick until agent.toml is fixed
                    // (issue #181). The process deliberately keeps running: the
                    // guard re-reads the configuration on the next tick, so
                    // bumping inventory_revision heals the Agent without a
                    // restart.
                    if let crate::collector::CollectionError::InventoryDeclaration(conflict) = &error {
                        let line = format!("Agent refuses to declare its Node Inventory: {conflict}");
                        if let Some(printed) =
                            inventory_conflict.observe(&line, Instant::now())
                        {
                            tracing::warn!("{printed}");
                        }
                        continue;
                    }
                    if crate::collector::is_deferrable_collection(&error) {
                        let line = format!("Agent report collection deferred: {error}");
                        if let Some(printed) = deferrals.observe(&line, Instant::now()) {
                            tracing::warn!("{printed}");
                        }
                        continue;
                    }
                    break Err(AgentCliError::Collection(error.to_string()));
                }
                // A successful declaration is positive evidence that the
                // conflict is gone: a later one is a new occurrence, not a
                // continuation of the old count.
                inventory_conflict.clear();
                deferrals.clear();
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
    write_permit: crate::database::AgentStoreWritePermit,
    send_timeout: Duration,
) -> Result<(), AgentCliError> {
    let mut store = crate::database::AgentStore::open_with_write_permit(
        crate::database::AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let transport = crate::reporting::HttpReportTransport::from_config(&config)
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let mut tick = periodic_interval(Duration::from_secs(1));
    let mut rejections = RepeatingMessage::new();
    // The delivery tick is 1s: an unreachable Server must not write a line
    // per second (issue #250).
    let mut deferrals = RepeatingMessage::new();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tick.tick() => {
                if cancel.is_cancelled() {
                    break;
                }
                // One pass per tick, but the pass itself is unbounded: it
                // keeps sending while the spool still holds a queued Report, so
                // catch-up throughput follows the Server instead of this tick
                // (issue #270). The send timeout bounds one HTTP send, and a
                // timed-out send still leaves the receipts it already applied
                // counted.
                //
                // A refusal is logged as its receipt is applied, not when the
                // pass returns: a rejection early in a long catch-up is one
                // condition the operator must see immediately (design
                // logging.md), and the pass has no bounded duration.
                let pass = crate::reporting::deliver_backlog_with_send_timeout(
                    &mut store,
                    &transport,
                    send_timeout,
                    &cancel,
                    |report_id, rejection| {
                        // A whole-report rejection is an applied receipt, not a
                        // transport error, so it never reaches the deferral
                        // warning below. Without this the console stays silent
                        // while every report is refused and the projections
                        // freeze (issue #177).
                        //
                        // The line names the condition, never the Report: a
                        // catch-up that refuses thousands of Reports is one
                        // condition, so the throttle below prints it once per
                        // minute (design logging.md) and the refused Report
                        // stays identifiable through its field.
                        let line = crate::reporting::rejection_line(rejection);
                        if let Some(printed) = rejections.observe(&line, Instant::now()) {
                            tracing::warn!(outcome = "rejected", report_id, "{printed}");
                        }
                    },
                )
                .await;
                // An accepted report proves the Server takes this Agent's
                // reports again, so a later refusal is a new condition. An idle
                // tick is no evidence either way.
                if pass.outcome.applied > 0 && pass.outcome.rejections.is_empty() {
                    rejections.clear();
                    deferrals.clear();
                }
                if let crate::reporting::DeliveryStop::Failed(error) = pass.stop {
                    let line = format!("Agent report delivery deferred: {error}");
                    if let Some(printed) = deferrals.observe(&line, Instant::now()) {
                        tracing::warn!("{printed}");
                    }
                }
            }
        }
    }
    store
        .close()
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))
}

/// Per-Node workers run inside this span, so every line they log is scoped to
/// its Node (observations are per Node, never merged per Agent).
fn node_span(node_id: &NodeId) -> tracing::Span {
    tracing::error_span!("node", node_id = %node_id)
}

async fn persist_abandoned_block_worker_gaps(
    config: &AgentConfig,
    node_ids: &[NodeId],
    write_permit: crate::database::AgentStoreWritePermit,
) -> Result<(), AgentCliError> {
    let mut store = crate::database::AgentStore::open_with_write_permit(
        crate::database::AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    for node_id in node_ids {
        let mut tx = store
            .write()
            .await
            .map_err(|error| AgentCliError::Collection(error.to_string()))?;
        let transaction_result: Result<(), AgentCliError> = async {
            let pending: Option<(i64, i64)> = sqlx::query_as(
                "SELECT pending_from, pending_to FROM node_recovery_state WHERE node_id=? AND pending_from IS NOT NULL AND pending_to IS NOT NULL",
            )
            .bind(node_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| AgentCliError::Collection(error.to_string()))?;
            let Some((from, to)) = pending else {
                return Ok(());
            };
            if from < 0 || to < from {
                return Ok(());
            }
            let recorded_at = crate::collector::timestamp();
            let gap = crate::shutdown::shutdown_gap(
                *node_id,
                (from as u64, to as u64),
                recorded_at,
                "block worker shutdown deadline exhausted before head resolution",
            );
            sqlx::query("INSERT OR IGNORE INTO history_gaps (node_id, from_height, to_height, kind, reason, created_at) VALUES (?, ?, ?, ?, ?, ?)")
                .bind(gap.node_id.to_string())
                .bind(gap.from_height as i64)
                .bind(gap.to_height as i64)
                .bind("unrecoverable_backfill")
                .bind(&gap.reason)
                .bind(gap.recorded_at.to_string())
                .execute(&mut *tx)
                .await
                .map_err(|error| AgentCliError::Collection(error.to_string()))?;
            sqlx::query(
                "UPDATE node_recovery_state SET pending_from=NULL, pending_to=NULL, pending_trigger=NULL, pending_reason=NULL, updated_at=? WHERE node_id=?",
            )
            .bind(gap.recorded_at.to_string())
            .bind(node_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|error| AgentCliError::Collection(error.to_string()))?;
            Ok(())
        }
        .await;
        match transaction_result {
            Ok(()) => tx
                .commit()
                .await
                .map_err(|error| AgentCliError::Collection(error.to_string()))?,
            Err(error) => {
                tx.rollback().await.map_err(|rollback_error| {
                    AgentCliError::Collection(rollback_error.to_string())
                })?;
                return Err(error);
            }
        }
    }
    store
        .close()
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    Ok(())
}

/// The stop signals of a running Agent (SIGTERM and SIGINT on Unix).
///
/// Installing replaces the default disposition at once, and a signal that
/// arrives before [ShutdownSignals::recv] is first awaited is kept, so the
/// Agent can install these first and still answer a stop sent during startup.
pub(crate) struct ShutdownSignals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
}

impl ShutdownSignals {
    pub(crate) fn install() -> Result<Self, AgentCliError> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let handler = |kind: SignalKind, name: &'static str| {
                signal(kind).map_err(|source| AgentCliError::SignalHandler {
                    signal: name,
                    source,
                })
            };
            Ok(Self {
                terminate: handler(SignalKind::terminate(), "SIGTERM")?,
                interrupt: handler(SignalKind::interrupt(), "SIGINT")?,
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    /// Wait until a stop signal has arrived since [ShutdownSignals::install].
    pub(crate) async fn recv(&mut self) {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = self.terminate.recv() => {}
                _ = self.interrupt.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
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

    #[tokio::test]
    async fn data_directory_worker_publishes_cached_observation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("data"), vec![0_u8; 11]).unwrap();
        let cancel = CancellationToken::new();
        let (sender, mut receiver) = tokio::sync::watch::channel(
            crate::data_directory::starting_observations("2026-08-12T09:00:00Z".parse().unwrap()),
        );
        let task = tokio::spawn(run_data_directory_worker(
            temp.path().to_owned(),
            sender,
            cancel.clone(),
        ));

        tokio::time::timeout(Duration::from_secs(2), receiver.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(receiver.borrow().size_bytes.latest, Some(11));
        assert!(receiver.borrow().capacity_bytes.latest.is_some());

        cancel.cancel();
        task.await.unwrap();
    }

    #[test]
    fn repeating_message_prints_once_then_reminds_with_a_count() {
        let start = Instant::now();
        let mut message = RepeatingMessage::new();
        assert_eq!(
            message.observe("declined", start),
            Some("declined".to_owned())
        );
        // Inside the interval nothing prints, no matter how often the
        // condition repeats: a per-tick retry must not flood the console.
        assert_eq!(
            message.observe("declined", start + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            message.observe("declined", start + Duration::from_secs(59)),
            None
        );
        assert_eq!(
            message.observe("declined", start + Duration::from_secs(60)),
            Some("declined (repeated 4 times)".to_owned())
        );
        // A different condition is never delayed behind the interval.
        assert_eq!(
            message.observe("other", start + Duration::from_secs(61)),
            Some("other".to_owned())
        );
        // Once the condition clears, a recurrence is a first occurrence again.
        message.clear();
        assert_eq!(
            message.observe("declined", start + Duration::from_secs(62)),
            Some("declined".to_owned())
        );
    }
}
