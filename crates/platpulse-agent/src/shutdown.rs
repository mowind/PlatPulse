//! Graceful shutdown coordination primitives.
//!
//! The Agent has no global queue: each Node owns a bounded subscription. This
//! module provides the bounded, cancellation-safe drain used by shutdown and
//! tests. A resolver that is already in flight is allowed to finish only until
//! the same shutdown deadline; anything still queued is converted to one
//! explicit History Gap by the caller.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use platpulse_core::block::BlockSummary;
use platpulse_core::hex::Hash32;
use platpulse_core::identity::NodeId;
use platpulse_core::network::NetworkIdentity;
use platpulse_core::time::Rfc3339;

use tokio::time::timeout_at;
use tokio_util::sync::CancellationToken;

use crate::block::{HeadSubscription, ResolveError, ResolvedBlock, WebSocketBlockTransport};
use crate::collector::{FailClosedRpcAdapter, RpcAdapter};

/// Runtime coordinator owning collection intake and the cancellation signal.
/// A shutdown request first cancels intake, then drains each Node queue, so no
/// new headers can race with the bounded shutdown drain.
pub struct AgentRuntime {
    cancel: CancellationToken,
}

impl AgentRuntime {
    pub fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
        }
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn run_until_shutdown<A: RpcAdapter>(
        &self,
        config: &crate::config::AgentConfig,
        adapter: &A,
        transport: &WebSocketBlockTransport,
        subscriptions: &mut [HeadSubscription],
        drain_deadline: Duration,
        sender_deadline: Duration,
    ) -> Result<ShutdownOutcome, crate::collector::CollectionError> {
        let token = self.cancel.clone();
        tokio::select! {
            _ = token.cancelled() => {}
            result = crate::collector::collect_and_persist_with_blocks(config, adapter, transport, subscriptions) => {
                result?;
            }
        }
        graceful_shutdown_with_subscriptions(
            config,
            adapter,
            subscriptions,
            drain_deadline,
            sender_deadline,
        )
        .await
    }
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Production entry point used by supervisors: it waits for SIGTERM/ctrl-c
/// and performs the same cancellation-safe drain as an explicit request.
pub async fn run_until_signal(
    config: &crate::config::AgentConfig,
) -> Result<ShutdownOutcome, crate::collector::CollectionError> {
    let runtime = AgentRuntime::new();
    let token = runtime.cancellation_token();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => runtime.request_shutdown(),
        _ = token.cancelled() => {}
    }
    let mut queues = Vec::new();
    graceful_shutdown_with_subscriptions(
        config,
        &FailClosedRpcAdapter,
        &mut queues,
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .await
}

/// Async hash resolver used while draining one Node subscription.
pub trait AsyncBlockResolver {
    fn resolve<'a>(
        &'a self,
        hash: &'a Hash32,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedBlock, ResolveError>> + Send + 'a>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownDrainResult {
    pub summaries: Vec<BlockSummary>,
    pub unresolved_range: Option<(u64, u64)>,
    pub timed_out: bool,
    pub elapsed: Duration,
}

/// Drain one Node's queued and in-flight resolutions until `deadline`.
///
/// The queue is only popped after identity validation succeeds. Therefore a
/// timeout, resolver error, or forced cancellation leaves the unresolved range
/// available for durable gap persistence rather than silently discarding it.
pub async fn drain_subscription<R: AsyncBlockResolver>(
    subscription: &mut HeadSubscription,
    resolver: &R,
    identity: NetworkIdentity,
    observed_at: Rfc3339,
    deadline: Instant,
) -> ShutdownDrainResult {
    let started = Instant::now();
    let mut summaries = Vec::new();
    let mut timed_out = false;
    while !subscription.is_empty() {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            timed_out = true;
            break;
        };
        let Some(header) = subscription.front_header() else {
            break;
        };
        let resolved = match timeout_at(
            tokio::time::Instant::now() + remaining,
            resolver.resolve(&header.block_hash),
        )
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => break,
            Err(_) => {
                timed_out = true;
                break;
            }
        };
        if resolved.block_number != header.block_number
            || resolved.block_hash != header.block_hash
            || resolved.parent_hash != header.parent_hash
            || resolved.network_identity != identity
        {
            break;
        }
        subscription.pop_front();
        summaries.push(BlockSummary {
            node_id: subscription.node_id(),
            network_identity: resolved.network_identity,
            block_number: resolved.block_number,
            block_hash: resolved.block_hash,
            parent_hash: resolved.parent_hash,
            block_timestamp_ms: resolved.block_timestamp_ms,
            observed_at,
            transaction_count: resolved.transaction_hashes.len() as u64,
            block_interval_ms: None,
            source: platpulse_core::block::BlockSource::Subscription,
            attribution: platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                resolved.coinbase,
                "shutdown drain preserved resolved block without seal recovery transport",
            ),
        });
    }
    let unresolved_range = if subscription.is_empty() {
        None
    } else {
        subscription.drain_unresolved_range()
    };
    ShutdownDrainResult {
        summaries,
        unresolved_range,
        timed_out,
        elapsed: started.elapsed(),
    }
}

/// Convert a pending shutdown range to a bounded, explicit history gap.
pub fn shutdown_gap(
    node_id: NodeId,
    range: (u64, u64),
    recorded_at: Rfc3339,
    reason: impl Into<String>,
) -> platpulse_core::gap::HistoryGap {
    platpulse_core::gap::HistoryGap {
        node_id,
        from_height: range.0,
        to_height: range.1,
        kind: platpulse_core::gap::GapKind::UnrecoverableBackfill,
        reason: reason.into().chars().take(256).collect(),
        recorded_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::HeadHeader;
    use std::sync::Arc;
    use tokio::sync::Notify;

    fn hash(byte: char) -> Hash32 {
        format!("0x{}", byte.to_string().repeat(64))
            .parse()
            .unwrap()
    }
    fn address() -> platpulse_core::hex::Address {
        format!("0x{}", "a".repeat(40)).parse().unwrap()
    }
    fn identity() -> NetworkIdentity {
        NetworkIdentity {
            genesis_hash: hash('b'),
            chain_id: 1,
            p2p_network_id: 1,
            address_hrp: Some("lat".into()),
        }
    }
    fn node() -> NodeId {
        "0195f2a1-0014-4014-8014-000000000014".parse().unwrap()
    }
    fn header(height: u64) -> HeadHeader {
        HeadHeader {
            block_number: height,
            block_hash: hash((b'c' + height as u8) as char),
            parent_hash: hash('d'),
            block_timestamp_ms: height * 1000,
            coinbase: address(),
        }
    }
    fn block(height: u64) -> ResolvedBlock {
        ResolvedBlock {
            block_number: height,
            block_hash: hash((b'c' + height as u8) as char),
            parent_hash: hash('d'),
            block_timestamp_ms: height * 1000,
            transaction_hashes: vec![],
            network_identity: identity(),
            coinbase: address(),
        }
    }
    struct Fake {
        delay: Duration,
        notify: Option<Arc<Notify>>,
    }
    impl AsyncBlockResolver for Fake {
        fn resolve<'a>(
            &'a self,
            hash: &'a Hash32,
        ) -> Pin<Box<dyn Future<Output = Result<ResolvedBlock, ResolveError>> + Send + 'a>>
        {
            let value = if *hash == block(1).block_hash {
                block(1)
            } else {
                block(2)
            };
            let delay = self.delay;
            let notify = self.notify.clone();
            Box::pin(async move {
                if let Some(n) = notify {
                    n.notify_one();
                }
                tokio::time::sleep(delay).await;
                Ok(value)
            })
        }
    }
    #[test]
    fn cancellation_request_stops_intake_signal() {
        let runtime = AgentRuntime::new();
        let token = runtime.cancellation_token();
        assert!(!token.is_cancelled());
        runtime.request_shutdown();
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_intake_preserves_unresolved_range() {
        let mut subscription = HeadSubscription::new(node(), 4);
        subscription.push(header(1)).unwrap();
        subscription.push(header(2)).unwrap();
        assert_eq!(subscription.cancel_intake(), Some((1, 2)));
        assert!(subscription.is_empty());
    }
    #[tokio::test]
    async fn drains_queued_and_in_flight_until_deadline_then_records_range() {
        let mut sub = HeadSubscription::new(node(), 4);
        sub.push(header(1)).unwrap();
        sub.push(header(2)).unwrap();
        let result = drain_subscription(
            &mut sub,
            &Fake {
                delay: Duration::ZERO,
                notify: None,
            },
            identity(),
            "2026-01-01T00:00:00Z".parse().unwrap(),
            Instant::now() + Duration::from_secs(1),
        )
        .await;
        assert_eq!(result.summaries.len(), 2);
        assert_eq!(result.unresolved_range, None);
    }
    #[tokio::test]
    async fn in_flight_timeout_keeps_the_unresolved_range() {
        let mut sub = HeadSubscription::new(node(), 2);
        sub.push(header(1)).unwrap();
        let result = drain_subscription(
            &mut sub,
            &Fake {
                delay: Duration::from_secs(1),
                notify: None,
            },
            identity(),
            "2026-01-01T00:00:00Z".parse().unwrap(),
            Instant::now() + Duration::from_millis(5),
        )
        .await;
        assert!(result.timed_out);
        assert_eq!(result.unresolved_range, Some((1, 1)));
        assert!(result.summaries.is_empty());
    }
    #[test]
    fn shutdown_gap_is_typed_and_reason_bounded() {
        let gap = shutdown_gap(
            node(),
            (11, 19),
            "2026-01-01T00:00:00Z".parse().unwrap(),
            "deadline exhausted",
        );
        assert_eq!(
            gap.kind,
            platpulse_core::gap::GapKind::UnrecoverableBackfill
        );
        assert_eq!((gap.from_height, gap.to_height), (11, 19));
        assert_eq!(gap.reason, "deadline exhausted");
    }

    #[test]
    fn shutdown_deadline_preserves_subsecond_precision() {
        let start: Rfc3339 = "2026-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(
            started_at_plus(start, Duration::from_millis(1500)).to_string(),
            "2026-01-01T00:00:01Z"
        );
    }

    #[tokio::test]
    async fn resolver_failure_preserves_queued_range_without_discarding_it() {
        struct Failure;
        impl AsyncBlockResolver for Failure {
            fn resolve<'a>(
                &'a self,
                _: &'a Hash32,
            ) -> Pin<Box<dyn Future<Output = Result<ResolvedBlock, ResolveError>> + Send + 'a>>
            {
                Box::pin(async { Err(ResolveError::Rpc) })
            }
        }
        let mut sub = HeadSubscription::new(node(), 2);
        sub.push(header(1)).unwrap();
        let result = drain_subscription(
            &mut sub,
            &Failure,
            identity(),
            "2026-01-01T00:00:00Z".parse().unwrap(),
            Instant::now() + Duration::from_secs(1),
        )
        .await;
        assert_eq!(result.unresolved_range, Some((1, 1)));
        assert!(!result.timed_out);
    }
}

/// Outcome of one bounded graceful shutdown attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownOutcome {
    pub report_id: platpulse_core::identity::ReportId,
    pub report_sequence: u64,
    pub stored: bool,
    pub receipt_applied: bool,
    pub sender_deadline_exhausted: bool,
    pub shutdown_state: &'static str,
}

fn shutdown_timestamp() -> Rfc3339 {
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("valid timestamp")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("valid timestamp")
        .parse()
        .expect("valid timestamp")
}

struct TransportResolver<'a> {
    transport: &'a WebSocketBlockTransport,
    endpoint: &'a platpulse_core::network::RpcEndpoint,
    identity: NetworkIdentity,
}

impl AsyncBlockResolver for TransportResolver<'_> {
    fn resolve<'a>(
        &'a self,
        hash: &'a Hash32,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedBlock, ResolveError>> + Send + 'a>> {
        Box::pin(async move {
            self.transport
                .get_block_by_hash_async(self.endpoint, hash, &self.identity)
                .await
                .map_err(|_| ResolveError::Rpc)
        })
    }
}

/// Drain the live Node queues, persist every resolved sample and every
/// unresolved range, then persist a canonical immutable Closing report.
pub async fn graceful_shutdown_with_subscriptions<A: crate::collector::RpcAdapter>(
    config: &crate::config::AgentConfig,
    adapter: &A,
    subscriptions: &mut [HeadSubscription],
    drain_deadline: Duration,
    sender_deadline: Duration,
) -> Result<ShutdownOutcome, crate::collector::CollectionError> {
    use std::str::FromStr;
    let mut store = crate::database::AgentStore::open(crate::database::AgentDatabaseConfig::new(
        &config.state_db,
    ))
    .await?;
    let state: Option<(String, i64, Option<String>, i64, String)> = sqlx::query_as(
        "SELECT agent_id, agent_epoch, boot_id, report_sequence, boot_state FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(store.connection())
    .await?;
    let Some((agent_text, epoch, boot_text, sequence, boot_state)) = state else {
        return Err(crate::collector::CollectionError::NotEnrolled);
    };
    if boot_state == "draining" {
        return Err(crate::collector::CollectionError::RecoveryRequired);
    }
    let agent_id = platpulse_core::identity::AgentId::from_str(&agent_text)
        .map_err(|error| crate::collector::CollectionError::Identity(error.to_string()))?;
    let boot_id = boot_text
        .ok_or(crate::collector::CollectionError::RecoveryRequired)
        .and_then(|value| {
            platpulse_core::identity::BootId::from_str(&value)
                .map_err(|error| crate::collector::CollectionError::Identity(error.to_string()))
        })?;
    let started = shutdown_timestamp();
    let deadline = started_at_plus(started, drain_deadline);
    sqlx::query("UPDATE agent_state SET shutdown_state='draining', shutdown_started_at=?, shutdown_deadline_at=?, shutdown_updated_at=?, updated_at=? WHERE singleton=1")
        .bind(started.to_string()).bind(deadline.to_string()).bind(started.to_string()).bind(started.to_string())
        .execute(store.connection()).await?;

    let inventory = config
        .validated_inventory()
        .map_err(|error| crate::collector::CollectionError::Identity(error.to_string()))?
        .inventory;
    let drain_until = Instant::now() + drain_deadline;
    let transport = WebSocketBlockTransport::default();
    for subscription in subscriptions.iter_mut() {
        let Some(node) = inventory
            .nodes
            .iter()
            .find(|node| node.node_id == subscription.node_id())
        else {
            continue;
        };
        let Some(identity) = crate::collector::collect_report(
            config,
            agent_id,
            epoch as u64,
            boot_id,
            sequence.max(1) as u64,
            inventory.clone(),
            adapter,
        )?
        .nodes
        .into_iter()
        .find(|node| node.node_id == subscription.node_id())
        .and_then(|node| node.chain.network_identity.latest) else {
            continue;
        };
        let resolver = TransportResolver {
            transport: &transport,
            endpoint: &node.rpc_endpoint,
            identity: identity.clone(),
        };
        let result =
            drain_subscription(subscription, &resolver, identity, started, drain_until).await;
        for summary in result.summaries {
            crate::block::persist_block_summary(&mut store, &summary, &started.to_string()).await?;
        }
        if let Some(range) = result.unresolved_range {
            let gap = shutdown_gap(
                subscription.node_id(),
                range,
                started,
                if result.timed_out {
                    "shutdown drain deadline exhausted"
                } else {
                    "shutdown resolver failed"
                },
            );
            crate::block::persist_history_gap(&mut store, &gap).await?;
            sqlx::query("UPDATE agent_state SET shutdown_unresolved_from=?, shutdown_unresolved_to=?, shutdown_last_error=? WHERE singleton=1")
                .bind(range.0 as i64).bind(range.1 as i64).bind(&gap.reason).execute(store.connection()).await?;
        }
    }

    let send_deadline = started_at_plus(started, sender_deadline);
    let mut report = crate::collector::collect_report(
        config,
        agent_id,
        epoch as u64,
        boot_id,
        sequence as u64 + 1,
        inventory,
        adapter,
    )?;
    report.boot_transition = platpulse_core::BootTransition::Closing;
    report.block_summaries = crate::block::load_block_summaries(&mut store).await?;
    report.history_gaps = crate::block::load_history_gaps(&mut store).await?;
    report.host.spool = crate::collector::current_spool_diagnostics(&mut store)
        .await
        .map(|value| crate::collector::ok(value, report.generated_at))
        .map_err(crate::collector::CollectionError::Database)?;
    if let Some(spool) = report.host.spool.latest.as_mut() {
        spool.shutdown_state = Some("final_stored".to_owned());
        spool.shutdown_started_at = Some(started);
        spool.shutdown_deadline_at = Some(send_deadline);
        spool.shutdown_forced = Some(false);
    }
    report
        .validate()
        .map_err(|error| crate::collector::CollectionError::Identity(error.to_string()))?;
    let body = serde_json::to_vec(&report)?;
    let generated = report.generated_at.to_string();
    crate::reporting::persist_immutable_report(
        &mut store,
        &report.report_id.to_string(),
        report.agent_epoch,
        &report.boot_id.to_string(),
        report.report_sequence,
        &generated,
        &body,
    )
    .await?;
    sqlx::query("UPDATE agent_state SET report_sequence=?, shutdown_state='final_stored', shutdown_report_id=?, shutdown_report_sequence=?, shutdown_finished_at=?, shutdown_updated_at=?, updated_at=? WHERE singleton=1")
        .bind(report.report_sequence as i64).bind(report.report_id.to_string()).bind(report.report_sequence as i64).bind(&generated).bind(&generated).bind(&generated).execute(store.connection()).await?;
    let mut receipt_applied = false;
    let mut exhausted = false;
    let send_until = tokio::time::Instant::now() + sender_deadline;
    let sender = crate::reporting::HttpReportTransport::from_config(config)?;
    loop {
        let Some(remaining) = send_until.checked_duration_since(tokio::time::Instant::now()) else {
            exhausted = true;
            break;
        };
        match tokio::time::timeout(
            remaining,
            crate::reporting::deliver_one(&mut store, &sender),
        )
        .await
        {
            Ok(Ok(Some(_))) => receipt_applied = true,
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                let at = shutdown_timestamp();
                sqlx::query("UPDATE agent_state SET shutdown_state='send_failed', shutdown_last_error=?, shutdown_updated_at=? WHERE singleton=1").bind(error.to_string().chars().take(256).collect::<String>()).bind(at.to_string()).execute(store.connection()).await?;
                store.close().await?;
                return Ok(ShutdownOutcome {
                    report_id: report.report_id,
                    report_sequence: report.report_sequence,
                    stored: true,
                    receipt_applied,
                    sender_deadline_exhausted: false,
                    shutdown_state: "send_failed",
                });
            }
            Err(_) => {
                exhausted = true;
                break;
            }
        }
    }
    if exhausted {
        let at = shutdown_timestamp();
        sqlx::query("UPDATE agent_state SET shutdown_state='forced_kill_recovery', shutdown_forced=1, shutdown_finished_at=?, shutdown_updated_at=? WHERE singleton=1").bind(at.to_string()).bind(at.to_string()).execute(store.connection()).await?;
    }
    store.close().await?;
    Ok(ShutdownOutcome {
        report_id: report.report_id,
        report_sequence: report.report_sequence,
        stored: true,
        receipt_applied,
        sender_deadline_exhausted: exhausted,
        shutdown_state: if exhausted {
            "forced_kill_recovery"
        } else {
            "final_stored"
        },
    })
}

fn started_at_plus(started: Rfc3339, duration: Duration) -> Rfc3339 {
    let parsed = time::OffsetDateTime::parse(
        &started.to_string(),
        &time::format_description::well_known::Rfc3339,
    )
    .expect("shutdown timestamp");
    (parsed
        + time::Duration::seconds(duration.as_secs() as i64)
        + time::Duration::nanoseconds(i64::from(duration.subsec_nanos())))
    .format(&time::format_description::well_known::Rfc3339)
    .expect("shutdown deadline")
    .parse()
    .expect("shutdown deadline")
}

/// caller owns the periodic/subscription tasks and must stop them before this
/// one-shot operation; unreceipted bytes remain in the spool on every failure.
pub async fn graceful_shutdown<A: crate::collector::RpcAdapter>(
    config: &crate::config::AgentConfig,
    adapter: &A,
    sender_deadline: Duration,
) -> Result<ShutdownOutcome, crate::collector::CollectionError> {
    let mut subscriptions = Vec::new();
    graceful_shutdown_with_subscriptions(
        config,
        adapter,
        &mut subscriptions,
        sender_deadline,
        sender_deadline,
    )
    .await
}
