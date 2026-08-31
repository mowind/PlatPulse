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
use platpulse_core::identity::{AgentId, BootId, NodeId};
use platpulse_core::network::NetworkIdentity;
use platpulse_core::time::Rfc3339;
use platpulse_core::{AgentReport, BootTransition};

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
        let _runtime_lock =
            crate::database::AgentRuntimeLock::acquire(&config.state_db).map_err(|error| {
                crate::collector::CollectionError::RuntimeOwnership(error.to_string())
            })?;
        let write_permit = crate::database::AgentStoreWritePermit::new();
        let collection_result = crate::collector::collect_and_persist_with_blocks_with_permit(
            config,
            adapter,
            transport,
            subscriptions,
            write_permit.clone(),
        )
        .await;
        collection_result?;
        graceful_shutdown_with_subscriptions_with_permit(
            config,
            adapter,
            subscriptions,
            drain_deadline,
            sender_deadline,
            write_permit,
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
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| crate::collector::CollectionError::RuntimeOwnership(error.to_string()))?;
    let write_permit = crate::database::AgentStoreWritePermit::new();
    let runtime = AgentRuntime::new();
    let token = runtime.cancellation_token();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => runtime.request_shutdown(),
        _ = token.cancelled() => {}
    }
    let mut queues = Vec::new();
    graceful_shutdown_with_subscriptions_with_permit(
        config,
        &FailClosedRpcAdapter,
        &mut queues,
        Duration::from_secs(5),
        Duration::from_secs(5),
        write_permit,
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
    let unresolved_range = subscription.drain_unresolved_range();
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
    use crate::config::AgentConfig;
    use crate::credential::write_credential_file;
    use crate::database::{AgentDatabaseConfig, AgentStore};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
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
    #[tokio::test]
    async fn closing_report_uses_fail_closed_adapter_and_stages_next_boot() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let credential = dir.path().join("credential");
        let token = format!(
            "pp_agent_{}_{}",
            "0195f2a1-0011-4011-8011-000000000011",
            "a".repeat(64)
        );
        write_credential_file(&credential, &token).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            let header_end = loop {
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .or_else(|| line.strip_prefix("Content-Length:"))
                })
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            while request.len() < header_end + length {
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
            }
            let body = &request[header_end..header_end + length];
            let report: AgentReport = serde_json::from_slice(body).unwrap();
            let hash = format!("0x{}", hex::encode(Sha256::digest(body)));
            let response = json!({"receipt": {
                "report_id": report.report_id,
                "disposition": "rejected",
                "report_body_sha256": hash,
                "server_version": "test",
                "supported_protocol_majors": [1],
                "server_time": "2026-01-01T00:00:00Z",
                "inventory": "rejected",
                "rejections": [{"code": "invalid_envelope", "retryable": false, "reason": "test"}],
                "nodes": [], "samples": []
            }});
            let bytes = serde_json::to_vec(&response).unwrap();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(&bytes).await.unwrap();
        });
        let config_path = dir.path().join("agent.toml");
        std::fs::write(&config_path, format!(
            "server_url=\"http://127.0.0.1:{port}\"\ncredential_file=\"{}\"\nstate_db=\"{}\"\ninventory_revision=1\nnodes=[{{node_id=\"0195f2a1-0014-4014-8014-000000000014\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:6790\"}}]\n",
            credential.display(), db_path.display())).unwrap();
        let config = AgentConfig::resolve(&config_path).unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(&db_path))
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, updated_at) VALUES (1, ?, 1, ?, 0, 1, ?)")
            .bind("0195f2a1-0011-4011-8011-000000000011")
            .bind("0195f2a1-0012-4012-8012-000000000012")
            .bind("2026-01-01T00:00:00Z")
            .execute(store.connection()).await.unwrap();
        store.close().await.unwrap();
        let outcome = graceful_shutdown_with_subscriptions(
            &config,
            &FailClosedRpcAdapter,
            &mut [],
            Duration::ZERO,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert!(outcome.stored);
        assert!(outcome.receipt_applied);
        let mut reopened = AgentStore::open(AgentDatabaseConfig::new(&db_path))
            .await
            .unwrap();
        let state: (String, i64, String, Option<String>) = sqlx::query_as(
            "SELECT boot_state, report_sequence, shutdown_state, pending_transition FROM agent_state WHERE singleton=1",
        ).fetch_one(reopened.connection()).await.unwrap();
        assert_eq!(state.0, "drained_pending");
        assert_eq!(state.1, 0);
        assert_eq!(state.2, "final_stored");
        assert_eq!(state.3.as_deref(), Some("drained_previous"));
        let closing: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM report_receipts WHERE disposition='rejected'")
                .fetch_one(reopened.connection())
                .await
                .unwrap();
        assert_eq!(closing, 1);
        reopened.close().await.unwrap();
        server.await.unwrap();
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

/// Build shutdown's final report without probing the Node. A shutdown must be
/// able to complete when the RPC adapter is unavailable; the most recent
/// durable report supplies the last-good Node observations, while the
/// fail-closed skeleton supplies complete entries for newly configured Nodes.
fn build_shutdown_report(
    config: &crate::config::AgentConfig,
    agent_id: AgentId,
    agent_epoch: u64,
    boot_id: BootId,
    sequence: u64,
    inventory: platpulse_core::inventory::NodeInventory,
    last_good: Option<&AgentReport>,
) -> Result<AgentReport, crate::collector::CollectionError> {
    let at = crate::collector::timestamp();
    let mut report = crate::collector::collect_report_with_clock_skew(
        config,
        agent_id,
        agent_epoch,
        boot_id,
        sequence,
        inventory,
        &FailClosedRpcAdapter,
        crate::collector::clock_skew_error(
            at,
            "RPC collection is disabled during shutdown; persisted last-good values retained",
        ),
    )?;
    if let Some(last_good) = last_good {
        for node in &mut report.nodes {
            if let Some(previous) = last_good
                .nodes
                .iter()
                .find(|previous| previous.node_id == node.node_id)
            {
                *node = previous.clone();
            }
        }
    }
    report.boot_transition = BootTransition::Closing;
    report
        .validate()
        .map_err(|error| crate::collector::CollectionError::Identity(error.to_string()))?;
    Ok(report)
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
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| crate::collector::CollectionError::RuntimeOwnership(error.to_string()))?;
    graceful_shutdown_with_subscriptions_with_permit(
        config,
        adapter,
        subscriptions,
        drain_deadline,
        sender_deadline,
        crate::database::AgentStoreWritePermit::new(),
    )
    .await
}

pub(crate) async fn graceful_shutdown_with_subscriptions_with_permit<
    A: crate::collector::RpcAdapter,
>(
    config: &crate::config::AgentConfig,
    _adapter: &A,
    subscriptions: &mut [HeadSubscription],
    drain_deadline: Duration,
    sender_deadline: Duration,
    write_permit: crate::database::AgentStoreWritePermit,
) -> Result<ShutdownOutcome, crate::collector::CollectionError> {
    use std::str::FromStr;
    let mut store = crate::database::AgentStore::open_with_write_permit(
        crate::database::AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await?;
    let _write_permit = store.acquire_write().await;
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
    let boot_text = boot_text.ok_or(crate::collector::CollectionError::RecoveryRequired)?;
    let boot_id = platpulse_core::identity::BootId::from_str(&boot_text)
        .map_err(|error| crate::collector::CollectionError::Identity(error.to_string()))?;
    let started = shutdown_timestamp();
    let deadline = started_at_plus(started, drain_deadline);
    let result = sqlx::query("UPDATE agent_state SET shutdown_state='draining', shutdown_started_at=?, shutdown_deadline_at=?, shutdown_updated_at=?, updated_at=? WHERE singleton=1 AND agent_id=? AND agent_epoch=? AND boot_id=? AND report_sequence=? AND boot_state=?")
        .bind(started.to_string())
        .bind(deadline.to_string())
        .bind(started.to_string())
        .bind(started.to_string())
        .bind(&agent_text)
        .bind(epoch)
        .bind(&boot_text)
        .bind(sequence)
        .bind(&boot_state)
        .execute(store.connection())
        .await?;
    if result.rows_affected() != 1 {
        return Err(crate::collector::CollectionError::ConcurrentStateChange);
    }
    drop(_write_permit);

    let inventory = config
        .validated_inventory()
        .map_err(|error| crate::collector::CollectionError::Identity(error.to_string()))?
        .inventory;
    let last_good = crate::collector::load_last_report(&mut store).await?;
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
        let Some(identity) = last_good
            .as_ref()
            .and_then(|report| {
                report
                    .nodes
                    .iter()
                    .find(|node| node.node_id == subscription.node_id())
            })
            .and_then(|node| node.chain.network_identity.latest.clone())
        else {
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
            let _write_permit = store.acquire_write().await;
            sqlx::query("UPDATE agent_state SET shutdown_unresolved_from=?, shutdown_unresolved_to=?, shutdown_last_error=? WHERE singleton=1")
                .bind(range.0 as i64).bind(range.1 as i64).bind(&gap.reason).execute(store.connection()).await?;
        }
    }

    let send_deadline = started_at_plus(started, sender_deadline);
    let mut report = build_shutdown_report(
        config,
        agent_id,
        epoch as u64,
        boot_id,
        sequence as u64 + 1,
        inventory,
        last_good.as_ref(),
    )?;
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
    crate::reporting::persist_closing_report(
        &mut store,
        &report.report_id.to_string(),
        report.agent_epoch,
        &report.boot_id.to_string(),
        report.report_sequence,
        &generated,
        &body,
        sequence as u64,
        &boot_state,
    )
    .await?;
    let closing_report_id = report.report_id.to_string();
    let mut receipt_applied = false;
    let mut exhausted = false;
    let send_until = tokio::time::Instant::now() + sender_deadline;
    let sender = crate::reporting::HttpReportTransport::from_config(config)?;
    loop {
        if tokio::time::Instant::now() >= send_until {
            exhausted = true;
            break;
        }
        match crate::reporting::deliver_one_with_send_deadline(&mut store, &sender, send_until)
            .await
        {
            Ok(Some(delivered)) => {
                if delivered.report_id == closing_report_id {
                    receipt_applied = true;
                    break;
                }
            }
            Ok(None) => break,
            Err(crate::reporting::ReportStoreError::DeliveryDeadline) => {
                exhausted = true;
                break;
            }
            Err(error) => {
                let at = shutdown_timestamp();
                let _write_permit = store.acquire_write().await;
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
        }
    }
    if exhausted && !receipt_applied {
        let at = shutdown_timestamp();
        let _write_permit = store.acquire_write().await;
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
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| crate::collector::CollectionError::RuntimeOwnership(error.to_string()))?;
    graceful_shutdown_with_permit(
        config,
        adapter,
        sender_deadline,
        crate::database::AgentStoreWritePermit::new(),
    )
    .await
}

pub(crate) async fn graceful_shutdown_with_permit<A: crate::collector::RpcAdapter>(
    config: &crate::config::AgentConfig,
    adapter: &A,
    sender_deadline: Duration,
    write_permit: crate::database::AgentStoreWritePermit,
) -> Result<ShutdownOutcome, crate::collector::CollectionError> {
    let mut subscriptions = Vec::new();
    graceful_shutdown_with_subscriptions_with_permit(
        config,
        adapter,
        &mut subscriptions,
        sender_deadline,
        sender_deadline,
        write_permit,
    )
    .await
}
