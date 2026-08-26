//! Per-Node block head subscriptions and hash-based resolution.
//!
//! A subscription is owned by one Node and has its own bounded queue.  The
//! resolver only requests a block by the header hash and receives transaction
//! hashes (never transaction bodies), then verifies all identity fields before
//! producing a `BlockSummary`. The production transport is backed by the
//! pinned Alloy fork; the synchronous `BlockTransport` seam remains available
//! for scripted fakes and tests.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use alloy::network::Ethereum;
use alloy::providers::{
    DynProvider, GetSubscription, IpcConnect, Provider, ProviderBuilder, WsConnect,
};
use platpulse_core::block::{
    BlockProductionAttribution, BlockSource, BlockSummary, ProtocolProposer, SealSignerMatch,
};
use platpulse_core::hex::{Address, Hash32};
use platpulse_core::identity::NodeId;
use platpulse_core::network::{NetworkIdentity, RpcEndpoint, RpcScheme};
use platpulse_core::protocol::{MAX_BLOCK_SUMMARIES, MAX_HISTORY_GAPS};
use platpulse_core::time::Rfc3339;
use serde::Deserialize;
use sqlx::{Connection, FromRow};
use thiserror::Error;
use tokio::sync::broadcast::error::RecvError as BroadcastRecvError;
use tokio::time::timeout;

use crate::database::AgentStore;
/// Production-oriented transport seam for per-Node PubSub/RPC. Implementations
/// select IPC, WS, or WSS from the endpoint and must return only bounded header
/// metadata and transaction hashes. The fail-closed implementation is used by
/// the CLI until a deployment supplies a concrete transport.
pub trait BlockTransport {
    fn subscribe_heads(&self, endpoint: &RpcEndpoint) -> Result<Vec<HeadHeader>, TransportError>;
    fn get_block_by_hash(
        &self,
        endpoint: &RpcEndpoint,
        hash: &Hash32,
    ) -> Result<ResolvedBlock, TransportError>;
    fn get_block_by_number(
        &self,
        _endpoint: &RpcEndpoint,
        _height: u64,
    ) -> Result<ResolvedBlock, TransportError> {
        Err(TransportError::Unavailable)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TransportError {
    #[error("block transport is unavailable for endpoint")]
    Unavailable,
    #[error("resolved block failed: {0}")]
    Resolve(#[from] ResolveError),
    #[error("block transport request failed: {0}")]
    Failed(String),
}

/// A bounded production transport backed by the pinned Alloy fork. Each
/// invocation opens its own connection and subscription for exactly one
/// Node; no Agent-level socket is shared between Nodes.
#[derive(Debug, Clone, Copy)]
pub struct WebSocketBlockTransport {
    pub receive_timeout: Duration,
    /// Bound for establishing one WS/IPC connection. A node that accepts TCP
    /// but stalls the HTTP upgrade must not block the collector forever.
    pub connect_timeout: Duration,
    pub max_heads: usize,
    pub max_transactions: usize,
    /// Maximum accepted JSON-RPC response size (serialized value length).
    pub max_response_bytes: usize,
    /// Methods allowed by this Node's configured capability probe. Unknown or
    /// admin/debug methods fail closed before a request is sent.
    pub allowed_methods: &'static [&'static str],
}

pub(crate) struct LiveHeadSubscription {
    provider: DynProvider<Ethereum>,
    heads: alloy::pubsub::Subscription<PlatonHead>,
}

impl Default for WebSocketBlockTransport {
    fn default() -> Self {
        Self {
            receive_timeout: Duration::from_secs(3),
            connect_timeout: Duration::from_secs(10),
            max_heads: 32,
            max_transactions: 100_000,
            max_response_bytes: 2 * 1024 * 1024,
            allowed_methods: &[
                "eth_subscribe",
                "eth_getBlockByHash",
                "eth_getBlockByNumber",
                "eth_blockNumber",
            ],
        }
    }
}

/// PlatON `newHeads` notification payload. Fields stay as wire strings so
/// quantity/timestamp parsing and bounds apply exactly once. The `eth_*`
/// alias reports seconds timestamps; [`timestamp_ms`] converts to ms.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlatonHead {
    pub number: String,
    pub hash: String,
    pub parent_hash: String,
    pub timestamp: String,
    #[serde(default, alias = "author")]
    pub miner: Option<String>,
}

/// PlatON `eth_getBlockBy{Hash,Number}(…, false)` response. Only
/// transaction hashes are ever retained; full bodies are never requested.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlatonBlock {
    pub number: String,
    pub hash: String,
    pub parent_hash: String,
    pub timestamp: String,
    #[serde(default, alias = "author")]
    pub miner: Option<String>,
    pub transactions: Vec<Hash32>,
}

fn quantity(value: &str) -> Result<u64, TransportError> {
    let digits = value.strip_prefix("0x").unwrap_or(value);
    u64::from_str_radix(digits, 16)
        .map_err(|_| TransportError::Failed("invalid RPC quantity".to_owned()))
}

fn parse_hash(value: &str) -> Result<Hash32, TransportError> {
    value
        .parse()
        .map_err(|_| TransportError::Failed("invalid RPC block hash".to_owned()))
}

fn parse_address(value: Option<String>) -> Result<Address, TransportError> {
    value
        .unwrap_or_else(|| ZERO_COINBASE.to_owned())
        .to_ascii_lowercase()
        .parse()
        .map_err(|_| TransportError::Failed("invalid RPC coinbase".to_owned()))
}

/// Sentinel used when a block/header omits `miner`; never a real coinbase.
const ZERO_COINBASE: &str = "0x0000000000000000000000000000000000000000";
fn timestamp_ms(value: &str) -> Result<u64, TransportError> {
    quantity(value)?
        .checked_mul(1000)
        .ok_or_else(|| TransportError::Failed("RPC timestamp out of range".to_owned()))
}

fn method_allowed(method: &str, allowed: &[&str]) -> bool {
    allowed.contains(&method)
}

/// Alloy decodes the raw frame before we see it, so the size bound applies to
/// the decoded value's serialized length instead of the wire bytes.
fn check_response_size(
    value: &serde_json::Value,
    max_response_bytes: usize,
) -> Result<(), TransportError> {
    if value.to_string().len() > max_response_bytes {
        return Err(TransportError::Failed(
            "RPC response exceeded configured size limit".to_owned(),
        ));
    }
    Ok(())
}

/// Parse and bound one `eth_getBlockBy…` response value.
fn parse_block_value(
    value: serde_json::Value,
    max_response_bytes: usize,
    max_transactions: usize,
) -> Result<PlatonBlock, TransportError> {
    check_response_size(&value, max_response_bytes)?;
    let block: PlatonBlock = serde_json::from_value(value)
        .map_err(|_| TransportError::Failed("malformed RPC block response".to_owned()))?;
    if block.transactions.len() > max_transactions {
        return Err(TransportError::Failed(
            "RPC block transaction list exceeded configured size limit".to_owned(),
        ));
    }
    Ok(block)
}

impl WebSocketBlockTransport {
    async fn connect(
        &self,
        endpoint: &RpcEndpoint,
    ) -> Result<DynProvider<Ethereum>, TransportError> {
        let connect = async {
            match endpoint.scheme() {
                RpcScheme::Ws | RpcScheme::Wss => ProviderBuilder::new()
                    .connect_ws(WsConnect::new(endpoint.as_str()))
                    .await
                    .map(DynProvider::new)
                    .map_err(|error| {
                        TransportError::Failed(format!("RPC connection failed: {error}"))
                    }),
                RpcScheme::Ipc => {
                    let path = endpoint
                        .as_str()
                        .strip_prefix("ipc://")
                        .ok_or(TransportError::Unavailable)?;
                    ProviderBuilder::new()
                        .connect_ipc(IpcConnect::new(path.to_owned()))
                        .await
                        .map(DynProvider::new)
                        .map_err(|error| {
                            TransportError::Failed(format!("RPC connection failed: {error}"))
                        })
                }
            }
        };
        timeout(self.connect_timeout, connect)
            .await
            .map_err(|_| TransportError::Failed("RPC connection timed out".to_owned()))?
    }

    /// One bounded raw JSON-RPC call through Alloy. The method allowlist is
    /// enforced before the request is sent; the decoded value is size-bounded
    /// before it is parsed into a DTO.
    async fn request_value(
        &self,
        provider: &DynProvider<Ethereum>,
        method: &'static str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, TransportError> {
        if !method_allowed(method, self.allowed_methods) {
            return Err(TransportError::Failed("RPC method unavailable".to_owned()));
        }
        let value: serde_json::Value = timeout(
            self.receive_timeout,
            provider.raw_request::<_, serde_json::Value>(Cow::Borrowed(method), params),
        )
        .await
        .map_err(|_| TransportError::Failed("RPC request timed out".to_owned()))?
        .map_err(|error| TransportError::Failed(format!("RPC request failed: {error}")))?;
        check_response_size(&value, self.max_response_bytes)?;
        Ok(value)
    }

    fn resolved_from_block(
        &self,
        block: PlatonBlock,
        identity: &NetworkIdentity,
    ) -> Result<ResolvedBlock, TransportError> {
        Ok(ResolvedBlock {
            block_number: quantity(&block.number)?,
            block_hash: parse_hash(&block.hash)?,
            parent_hash: parse_hash(&block.parent_hash)?,
            block_timestamp_ms: timestamp_ms(&block.timestamp)?,
            transaction_hashes: block.transactions,
            network_identity: identity.clone(),
            coinbase: parse_address(block.miner)?,
        })
    }

    async fn get_block_by_hash_with_provider(
        &self,
        provider: &DynProvider<Ethereum>,
        hash: &Hash32,
        identity: &NetworkIdentity,
    ) -> Result<ResolvedBlock, TransportError> {
        let value = self
            .request_value(
                provider,
                "eth_getBlockByHash",
                serde_json::json!([hash.to_string(), false]),
            )
            .await?;
        let block = parse_block_value(value, self.max_response_bytes, self.max_transactions)?;
        self.resolved_from_block(block, identity)
    }

    pub async fn get_block_by_hash_async(
        &self,
        endpoint: &RpcEndpoint,
        hash: &Hash32,
        identity: &NetworkIdentity,
    ) -> Result<ResolvedBlock, TransportError> {
        let provider = self.connect(endpoint).await?;
        self.get_block_by_hash_with_provider(&provider, hash, identity)
            .await
    }

    pub async fn get_block_by_number_async(
        &self,
        endpoint: &RpcEndpoint,
        identity: &NetworkIdentity,
        height: u64,
    ) -> Result<ResolvedBlock, TransportError> {
        let provider = self.connect(endpoint).await?;
        let value = self
            .request_value(
                &provider,
                "eth_getBlockByNumber",
                serde_json::json!([format!("0x{height:x}"), false]),
            )
            .await?;
        let block = parse_block_value(value, self.max_response_bytes, self.max_transactions)?;
        self.resolved_from_block(block, identity)
    }

    pub async fn current_head_async(&self, endpoint: &RpcEndpoint) -> Result<u64, TransportError> {
        let provider = self.connect(endpoint).await?;
        let value = self
            .request_value(&provider, "eth_blockNumber", serde_json::json!([]))
            .await?;
        let text = value
            .as_str()
            .ok_or_else(|| TransportError::Failed("invalid block number response".to_owned()))?;
        quantity(text)
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn gap_backfill(
        &self,
        endpoint: &RpcEndpoint,
        node_id: NodeId,
        identity: NetworkIdentity,
        observed_at: Rfc3339,
        from_height: u64,
        to_height: u64,
        bounds: BackfillBounds,
        trigger: BackfillTrigger,
    ) -> BackfillOutcome {
        let mut summaries = Vec::new();
        let mut height = from_height;
        let started = Instant::now();
        let end = to_height
            .min(from_height.saturating_add(bounds.max_height_span.saturating_sub(1)))
            .min(from_height.saturating_add(bounds.max_block_count.saturating_sub(1)));
        while height <= end && started.elapsed() <= bounds.max_time {
            let block = match self
                .get_block_by_number_async(endpoint, &identity, height)
                .await
            {
                Ok(block) => block,
                Err(error) => {
                    return BackfillOutcome {
                        summaries,
                        gaps: vec![make_gap(
                            node_id,
                            height,
                            to_height,
                            observed_at,
                            format!("{} point query failed: {error}", trigger.label()),
                        )],
                    };
                }
            };
            if block.block_number != height || block.network_identity != identity {
                return BackfillOutcome {
                    summaries,
                    gaps: vec![make_gap(
                        node_id,
                        height,
                        to_height,
                        observed_at,
                        format!("{} point query identity mismatch", trigger.label()),
                    )],
                };
            }
            summaries.push(BlockSummary { node_id, network_identity: identity.clone(), block_number: block.block_number, block_hash: block.block_hash, parent_hash: block.parent_hash, block_timestamp_ms: block.block_timestamp_ms, observed_at, transaction_count: block.transaction_hashes.len() as u64, block_interval_ms: None, source: BlockSource::GapBackfill, attribution: BlockProductionAttribution::unknown_attribution(block.coinbase, "seal recovery rule is not verified for this fork; protocol proposer evidence is unavailable") });
            height = height.saturating_add(1);
        }
        let gaps = if height <= to_height {
            vec![make_gap(
                node_id,
                height,
                to_height,
                observed_at,
                format!("{} exceeded configured backfill bounds", trigger.label()),
            )]
        } else {
            vec![]
        };
        BackfillOutcome { summaries, gaps }
    }

    pub async fn collect_node_summaries_into(
        &self,
        endpoint: &RpcEndpoint,
        subscription: &mut HeadSubscription,
        identity: NetworkIdentity,
        observed_at: Rfc3339,
    ) -> Result<Vec<BlockSummary>, TransportError> {
        let provider = self.connect(endpoint).await?;
        let mut heads = self.subscribe_heads(&provider).await?;
        let mut received_head = false;
        for _ in 0..self.max_heads {
            let receive_timeout = if received_head {
                Duration::from_millis(10)
            } else {
                self.receive_timeout
            };
            match timeout(receive_timeout, heads.recv_result()).await {
                Ok(Ok(Ok(head))) => {
                    received_head = true;
                    subscription
                        .push(HeadHeader {
                            block_number: quantity(&head.number)?,
                            block_hash: parse_hash(&head.hash)?,
                            parent_hash: parse_hash(&head.parent_hash)?,
                            block_timestamp_ms: timestamp_ms(&head.timestamp)?,
                            coinbase: parse_address(head.miner)?,
                        })
                        .map_err(|_| TransportError::Failed("head queue full".to_owned()))?;
                }
                Ok(Ok(Err(error))) => {
                    return Err(TransportError::Failed(format!(
                        "malformed head notification: {error}"
                    )));
                }
                Ok(Err(BroadcastRecvError::Lagged(_))) => {
                    return Err(TransportError::Failed(
                        "head notification queue lagged; reconnect required".to_owned(),
                    ));
                }
                Ok(Err(BroadcastRecvError::Closed)) | Err(_) => break,
            }
        }
        let mut summaries = Vec::new();
        while let Some(header) = subscription.front_header().cloned() {
            let resolved = self
                .get_block_by_hash_async(endpoint, &header.block_hash, &identity)
                .await?;
            if resolved.block_number != header.block_number
                || resolved.block_hash != header.block_hash
                || resolved.parent_hash != header.parent_hash
            {
                return Err(TransportError::Resolve(ResolveError::IdentityMismatch));
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
                source: BlockSource::Subscription,
                attribution: BlockProductionAttribution::unknown_attribution(
                    // Prefer the resolved block's coinbase; fall back to the
                    // subscribed header's coinbase exactly as the transport
                    // did before the Alloy migration (a missing `miner` is
                    // not a zero coinbase).
                    if resolved.coinbase.as_str() != ZERO_COINBASE {
                        resolved.coinbase
                    } else {
                        header.coinbase
                    },
                    "seal recovery rule is not verified for this fork; protocol proposer evidence is unavailable",
                ),
            });
        }
        Ok(summaries)
    }

    pub(crate) async fn open_live_head_subscription(
        &self,
        endpoint: &RpcEndpoint,
    ) -> Result<LiveHeadSubscription, TransportError> {
        let provider = self.connect(endpoint).await?;
        let heads = self.subscribe_heads(&provider).await?;
        Ok(LiveHeadSubscription { provider, heads })
    }

    pub(crate) async fn receive_live_head(
        &self,
        subscription: &mut LiveHeadSubscription,
    ) -> Result<HeadHeader, TransportError> {
        let head = match subscription.heads.recv_result().await {
            Ok(Ok(head)) => head,
            Ok(Err(error)) => {
                return Err(TransportError::Failed(format!(
                    "malformed head notification: {error}"
                )));
            }
            Err(BroadcastRecvError::Lagged(_)) => {
                return Err(TransportError::Failed(
                    "head notification queue lagged; reconnect required".to_owned(),
                ));
            }
            Err(BroadcastRecvError::Closed) => {
                return Err(TransportError::Failed(
                    "head subscription closed; reconnect required".to_owned(),
                ));
            }
        };
        Ok(HeadHeader {
            block_number: quantity(&head.number)?,
            block_hash: parse_hash(&head.hash)?,
            parent_hash: parse_hash(&head.parent_hash)?,
            block_timestamp_ms: timestamp_ms(&head.timestamp)?,
            coinbase: parse_address(head.miner)?,
        })
    }

    pub(crate) async fn resolve_live_head(
        &self,
        subscription: &LiveHeadSubscription,
        node_id: NodeId,
        header: &HeadHeader,
        identity: &NetworkIdentity,
        observed_at: Rfc3339,
    ) -> Result<BlockSummary, TransportError> {
        let resolved = self
            .get_block_by_hash_with_provider(&subscription.provider, &header.block_hash, identity)
            .await?;
        if resolved.block_number != header.block_number
            || resolved.block_hash != header.block_hash
            || resolved.parent_hash != header.parent_hash
        {
            return Err(TransportError::Resolve(ResolveError::IdentityMismatch));
        }
        Ok(BlockSummary {
            node_id,
            network_identity: resolved.network_identity,
            block_number: resolved.block_number,
            block_hash: resolved.block_hash,
            parent_hash: resolved.parent_hash,
            block_timestamp_ms: resolved.block_timestamp_ms,
            observed_at,
            transaction_count: resolved.transaction_hashes.len() as u64,
            block_interval_ms: None,
            source: BlockSource::Subscription,
            attribution: BlockProductionAttribution::unknown_attribution(
                if resolved.coinbase.as_str() != ZERO_COINBASE {
                    resolved.coinbase
                } else {
                    header.coinbase.clone()
                },
                "seal recovery rule is not verified for this fork; protocol proposer evidence is unavailable",
            ),
        })
    }

    /// Subscribe to `newHeads` through Alloy. The local `PlatonHead` DTO keeps
    /// PlatON's reduced header payload decodable; `channel_size` bounds the
    /// in-flight notification buffer.
    async fn subscribe_heads(
        &self,
        provider: &DynProvider<Ethereum>,
    ) -> Result<alloy::pubsub::Subscription<PlatonHead>, TransportError> {
        let mut call = provider.client().request("eth_subscribe", ("newHeads",));
        call.set_is_subscription();
        let subscription = GetSubscription::<_, PlatonHead>::new(provider.weak_client(), call)
            .channel_size(self.max_heads.max(1));
        timeout(self.receive_timeout, subscription)
            .await
            .map_err(|_| TransportError::Failed("block subscription timed out".to_owned()))?
            .map_err(|error| TransportError::Failed(format!("block subscription failed: {error}")))
    }

    pub async fn collect_node_summaries(
        &self,
        endpoint: &RpcEndpoint,
        node_id: NodeId,
        identity: NetworkIdentity,
        observed_at: Rfc3339,
    ) -> Result<Vec<BlockSummary>, TransportError> {
        let mut subscription = HeadSubscription::new(node_id, self.max_heads);
        self.collect_node_summaries_into(endpoint, &mut subscription, identity, observed_at)
            .await
    }
}
/// Explicitly fail closed: no fabricated heads or block values are emitted.
#[derive(Debug, Clone, Copy, Default)]
pub struct FailClosedBlockTransport;

impl BlockTransport for FailClosedBlockTransport {
    fn subscribe_heads(&self, _endpoint: &RpcEndpoint) -> Result<Vec<HeadHeader>, TransportError> {
        Err(TransportError::Unavailable)
    }

    fn get_block_by_hash(
        &self,
        _endpoint: &RpcEndpoint,
        _hash: &Hash32,
    ) -> Result<ResolvedBlock, TransportError> {
        Err(TransportError::Unavailable)
    }

    fn get_block_by_number(
        &self,
        _endpoint: &RpcEndpoint,
        _height: u64,
    ) -> Result<ResolvedBlock, TransportError> {
        Err(TransportError::Unavailable)
    }
}

struct EndpointResolver<'a, T> {
    transport: &'a T,
    endpoint: &'a RpcEndpoint,
}

impl<T: BlockTransport> BlockResolver for EndpointResolver<'_, T> {
    fn get_block_by_hash(&self, hash: &Hash32) -> Result<ResolvedBlock, ResolveError> {
        self.transport
            .get_block_by_hash(self.endpoint, hash)
            .map_err(|_| ResolveError::Rpc)
    }
}

/// Poll and resolve at most one bounded batch for a Node. A subscription
/// failure is isolated to that Node; successful summaries remain usable.
pub fn collect_transport_summaries<T: BlockTransport>(
    transport: &T,
    endpoint: &RpcEndpoint,
    subscription: &mut HeadSubscription,
    identity: NetworkIdentity,
    observed_at: Rfc3339,
) -> Result<Vec<BlockSummary>, TransportError> {
    for header in transport.subscribe_heads(endpoint)? {
        subscription
            .push(header)
            .map_err(|_| TransportError::Failed("head queue full".to_owned()))?;
    }
    let resolver = EndpointResolver {
        transport,
        endpoint,
    };
    let mut summaries = Vec::new();
    while !subscription.is_empty() {
        match subscription.resolve_next(&resolver, identity.clone(), observed_at)? {
            Some(summary) => summaries.push(summary),
            None => break,
        }
    }
    Ok(summaries)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackfillTrigger {
    StartupRace,
    Restart,
    Reconnect,
    HeightJump,
    QueueOverflow,
    Shutdown,
}

impl BackfillTrigger {
    fn label(self) -> &'static str {
        match self {
            Self::StartupRace => "startup race",
            Self::Restart => "restart",
            Self::Reconnect => "subscription reconnect",
            Self::HeightJump => "head height jump",
            Self::QueueOverflow => "header queue overflow",
            Self::Shutdown => "unresolved shutdown range",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryPlan {
    pub trigger: BackfillTrigger,
    pub from_height: u64,
    pub to_height: u64,
}

/// Decide whether a finite recovery is required. A normal adjacent head never
/// schedules a point query; only the declared recovery triggers do.
pub fn plan_recovery(
    previous_head: Option<u64>,
    current_head: u64,
    boot_changed: bool,
    reconnect: bool,
    queue_overflow: bool,
    shutdown_range: Option<(u64, u64)>,
) -> Option<RecoveryPlan> {
    if let Some((from, to)) = shutdown_range {
        return (from <= to).then_some(RecoveryPlan {
            trigger: BackfillTrigger::Shutdown,
            from_height: from,
            to_height: to,
        });
    }
    let from = previous_head
        .map(|head| head.saturating_add(1))
        .unwrap_or_else(|| current_head.saturating_sub(1));
    if from > current_head {
        return None;
    }
    let trigger = if queue_overflow {
        BackfillTrigger::QueueOverflow
    } else if reconnect {
        BackfillTrigger::Reconnect
    } else if boot_changed && previous_head.is_some() {
        BackfillTrigger::Restart
    } else if previous_head.is_none() {
        BackfillTrigger::StartupRace
    } else if current_head > previous_head.expect("checked above").saturating_add(1) {
        BackfillTrigger::HeightJump
    } else {
        return None;
    };
    Some(RecoveryPlan {
        trigger,
        from_height: from,
        to_height: current_head,
    })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackfillBounds {
    pub max_height_span: u64,
    pub max_block_count: u64,
    pub max_time: Duration,
}

impl Default for BackfillBounds {
    fn default() -> Self {
        Self {
            max_height_span: 256,
            max_block_count: 128,
            max_time: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackfillOutcome {
    pub summaries: Vec<BlockSummary>,
    pub gaps: Vec<platpulse_core::gap::HistoryGap>,
}

/// Perform one finite height point-query recovery for an explicit trigger.
/// This is never called by the normal realtime subscription path.
#[allow(clippy::too_many_arguments)]
pub fn backfill_missing<T: BlockTransport>(
    transport: &T,
    endpoint: &RpcEndpoint,
    node_id: NodeId,
    identity: NetworkIdentity,
    observed_at: Rfc3339,
    from_height: u64,
    to_height: u64,
    bounds: BackfillBounds,
    trigger: BackfillTrigger,
) -> BackfillOutcome {
    let mut outcome = BackfillOutcome::default();
    if from_height > to_height || bounds.max_height_span == 0 || bounds.max_block_count == 0 {
        return outcome;
    }
    let started = Instant::now();
    let end = to_height
        .min(from_height.saturating_add(bounds.max_height_span.saturating_sub(1)))
        .min(from_height.saturating_add(bounds.max_block_count.saturating_sub(1)));
    let mut height = from_height;
    while height <= end && started.elapsed() <= bounds.max_time {
        let block = match transport.get_block_by_number(endpoint, height) {
            Ok(block) => block,
            Err(error) => {
                outcome.gaps.push(make_gap(
                    node_id,
                    height,
                    to_height,
                    observed_at,
                    format!("{} point query failed: {error}", trigger.label()),
                ));
                return outcome;
            }
        };
        if block.block_number != height || block.network_identity != identity {
            outcome.gaps.push(make_gap(
                node_id,
                height,
                to_height,
                observed_at,
                format!("{} point query identity mismatch", trigger.label()),
            ));
            return outcome;
        }
        outcome.summaries.push(BlockSummary { node_id, network_identity: identity.clone(), block_number: block.block_number, block_hash: block.block_hash, parent_hash: block.parent_hash, block_timestamp_ms: block.block_timestamp_ms, observed_at, transaction_count: block.transaction_hashes.len() as u64, block_interval_ms: None, source: BlockSource::GapBackfill, attribution: BlockProductionAttribution::unknown_attribution(block.coinbase, "seal recovery rule is not verified for this fork; protocol proposer evidence is unavailable") });
        height = height.saturating_add(1);
    }
    if height <= to_height {
        outcome.gaps.push(make_gap(
            node_id,
            height,
            to_height,
            observed_at,
            format!("{} exceeded configured backfill bounds", trigger.label()),
        ));
    }
    outcome
}

fn make_gap(
    node_id: NodeId,
    from_height: u64,
    to_height: u64,
    recorded_at: Rfc3339,
    reason: String,
) -> platpulse_core::gap::HistoryGap {
    platpulse_core::gap::HistoryGap {
        node_id,
        kind: platpulse_core::gap::GapKind::UnrecoverableBackfill,
        from_height,
        to_height,
        reason,
        recorded_at,
    }
}

fn gap_kind_name(kind: platpulse_core::gap::GapKind) -> &'static str {
    match kind {
        platpulse_core::gap::GapKind::UnrecoverableBackfill => "unrecoverable_backfill",
        platpulse_core::gap::GapKind::SpoolOverflow => "spool_overflow",
        platpulse_core::gap::GapKind::ServerRejected => "server_rejected",
    }
}

pub async fn persist_history_gap(
    store: &mut AgentStore,
    gap: &platpulse_core::gap::HistoryGap,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO history_gaps (node_id, from_height, to_height, kind, reason, created_at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(gap.node_id.to_string())
        .bind(gap.from_height as i64)
        .bind(gap.to_height as i64)
        .bind(gap_kind_name(gap.kind))
        .bind(&gap.reason)
        .bind(gap.recorded_at.to_string())
        .execute(store.connection())
        .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadHeader {
    pub block_number: u64,
    pub block_hash: Hash32,
    pub parent_hash: Hash32,
    pub block_timestamp_ms: u64,
    pub coinbase: Address,
}

/// Bounded result of `eth_getBlockByHash(hash, false)`. The adapter only
/// exposes transaction hashes, making it impossible to fetch or retain bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBlock {
    pub block_number: u64,
    pub block_hash: Hash32,
    pub parent_hash: Hash32,
    pub block_timestamp_ms: u64,
    pub transaction_hashes: Vec<Hash32>,
    pub network_identity: NetworkIdentity,
    pub coinbase: Address,
}

/// RPC adapter used by the block resolver. Implementations must issue a hash
/// point query with full transaction objects disabled.
pub trait BlockResolver {
    fn get_block_by_hash(&self, hash: &Hash32) -> Result<ResolvedBlock, ResolveError>;
}

impl From<TransportError> for ResolveError {
    fn from(_: TransportError) -> Self {
        ResolveError::Rpc
    }
}
/// Scripted resolver used by tests; unlike production transport it is
/// explicitly populated and never invents block data.
#[derive(Debug, Clone, Default)]
pub struct ScriptedBlockResolver {
    blocks: HashMap<Hash32, ResolvedBlock>,
}

impl ScriptedBlockResolver {
    pub fn with_block(mut self, block: ResolvedBlock) -> Self {
        let hash = block.block_hash.clone();
        self.blocks.insert(hash, block);
        self
    }
}

impl BlockResolver for ScriptedBlockResolver {
    fn get_block_by_hash(&self, hash: &Hash32) -> Result<ResolvedBlock, ResolveError> {
        self.blocks.get(hash).cloned().ok_or(ResolveError::Rpc)
    }
}

/// Persist a resolved sample before constructing a report. Duplicate hash
/// deliveries are idempotent and never mutate the original sample.
pub async fn persist_block_summary(
    store: &mut AgentStore,
    summary: &BlockSummary,
    created_at: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = store.connection().begin().await?;
    sqlx::query("INSERT OR IGNORE INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, block_interval_ms, source, coinbase, seal_signer_key_fingerprint, seal_signer_match, node_key_fingerprint, node_key_valid_from, node_key_valid_until, node_key_history_complete, seal_recovery_rule, seal_evidence, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(summary.node_id.to_string()).bind(summary.block_number as i64).bind(summary.block_hash.to_string()).bind(summary.parent_hash.to_string())
        .bind(summary.network_identity.genesis_hash.to_string()).bind(summary.network_identity.chain_id as i64).bind(summary.network_identity.p2p_network_id as i64).bind(summary.network_identity.address_hrp.as_deref()).bind(summary.block_timestamp_ms as i64).bind(summary.observed_at.to_string()).bind(summary.transaction_count as i64).bind(summary.block_interval_ms.map(|v| v as i64)).bind(match summary.source { BlockSource::Subscription => "subscription", BlockSource::GapBackfill => "gap_backfill" }).bind(summary.attribution.coinbase.to_string()).bind(summary.attribution.seal_signer_key_fingerprint.as_ref().map(ToString::to_string)).bind(match summary.attribution.seal_signer_match { SealSignerMatch::SignerSelf => "self", SealSignerMatch::Other => "other", SealSignerMatch::Unknown => "unknown" }).bind(summary.attribution.node_key.as_ref().map(|key| key.fingerprint.to_string())).bind(summary.attribution.node_key.as_ref().and_then(|key| key.valid_from.map(|value| value.to_string()))).bind(summary.attribution.node_key.as_ref().and_then(|key| key.valid_until.map(|value| value.to_string()))).bind(summary.attribution.node_key.as_ref().is_some_and(|key| key.history_complete) as i64).bind(summary.attribution.seal_recovery_rule.as_deref()).bind(summary.attribution.seal_evidence.as_deref()).bind(match &summary.attribution.protocol_proposer { ProtocolProposer::Verified { .. } => "verified", ProtocolProposer::Unknown {} => "unknown" }).bind(match &summary.attribution.protocol_proposer { ProtocolProposer::Verified { identity } => Some(identity.as_str()), ProtocolProposer::Unknown {} => None }).bind(&summary.attribution.attribution_reason).bind(created_at).execute(&mut *tx).await?;
    tx.commit().await
}

#[derive(Debug, FromRow)]
struct BlockRow {
    node_id: String,
    block_number: i64,
    block_hash: String,
    parent_hash: String,
    network_genesis_hash: String,
    network_chain_id: i64,
    network_p2p_network_id: i64,
    network_address_hrp: Option<String>,
    block_timestamp_ms: i64,
    observed_at: String,
    transaction_count: i64,
    block_interval_ms: Option<i64>,
    source: String,
    coinbase: String,
    seal_signer_key_fingerprint: Option<String>,
    seal_signer_match: String,
    protocol_proposer_kind: String,
    protocol_proposer_identity: Option<String>,
    attribution_reason: String,
}

pub async fn load_history_gaps(
    store: &mut AgentStore,
) -> Result<Vec<platpulse_core::gap::HistoryGap>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, i64, i64, String, String)>(
        "SELECT g.node_id, g.from_height, g.to_height, g.kind, g.created_at FROM history_gaps g WHERE NOT EXISTS (SELECT 1 FROM report_sample_assignments a WHERE a.node_id=g.node_id AND a.sample_kind='gap' AND a.from_height=g.from_height AND a.to_height=g.to_height) ORDER BY g.created_at, g.gap_id LIMIT ?",
    )
    .bind(MAX_HISTORY_GAPS as i64)
    .fetch_all(store.connection())
    .await?;
    rows.into_iter()
        .map(|(node_id, from, to, kind, at)| {
            let kind = match kind.as_str() {
                "server_rejected" => platpulse_core::gap::GapKind::ServerRejected,
                "spool_overflow" => platpulse_core::gap::GapKind::SpoolOverflow,
                _ => platpulse_core::gap::GapKind::UnrecoverableBackfill,
            };
            Ok(platpulse_core::gap::HistoryGap {
                node_id: node_id
                    .parse()
                    .map_err(|_| sqlx::Error::Protocol("gap node id".into()))?,
                kind,
                from_height: from as u64,
                to_height: to as u64,
                reason: "locally retained history gap".to_owned(),
                recorded_at: at
                    .parse()
                    .map_err(|_| sqlx::Error::Protocol("gap timestamp".into()))?,
            })
        })
        .collect()
}

/// Load all pending block summaries in deterministic oldest-first order.
pub async fn load_block_summaries(
    store: &mut AgentStore,
) -> Result<Vec<BlockSummary>, sqlx::Error> {
    let rows = sqlx::query_as::<_, BlockRow>("SELECT b.node_id, b.block_number, b.block_hash, b.parent_hash, b.network_genesis_hash, b.network_chain_id, b.network_p2p_network_id, b.network_address_hrp, b.block_timestamp_ms, b.observed_at, b.transaction_count, b.block_interval_ms, b.source, b.coinbase, b.seal_signer_key_fingerprint, b.seal_signer_match, b.protocol_proposer_kind, b.protocol_proposer_identity, b.attribution_reason FROM block_summaries b WHERE NOT EXISTS (SELECT 1 FROM report_sample_assignments a WHERE a.node_id=b.node_id AND a.sample_kind='block' AND a.from_height=b.block_number AND a.to_height=b.block_number) ORDER BY b.created_at, b.sample_id LIMIT ?")
        .bind(MAX_BLOCK_SUMMARIES as i64)
        .fetch_all(store.connection())
        .await?;
    rows.into_iter()
        .map(|row| {
            let BlockRow {
                node_id,
                block_number: number,
                block_hash: hash,
                parent_hash: parent,
                network_genesis_hash: genesis,
                network_chain_id: chain,
                network_p2p_network_id: p2p,
                network_address_hrp: hrp,
                block_timestamp_ms: timestamp,
                observed_at: observed,
                transaction_count: txs,
                block_interval_ms: interval,
                source,
                coinbase,
                seal_signer_key_fingerprint: fingerprint,
                seal_signer_match: signer,
                protocol_proposer_kind: proposer_kind,
                protocol_proposer_identity: proposer_id,
                attribution_reason: reason,
            } = row;
            let proposer = if proposer_kind == "verified" {
                ProtocolProposer::Verified {
                    identity: proposer_id.unwrap_or_default(),
                }
            } else {
                ProtocolProposer::Unknown {}
            };
            Ok(BlockSummary {
                node_id: node_id
                    .parse()
                    .map_err(|_| sqlx::Error::Protocol("node_id".into()))?,
                block_number: number as u64,
                block_hash: hash
                    .parse()
                    .map_err(|_| sqlx::Error::Protocol("hash".into()))?,
                parent_hash: parent
                    .parse()
                    .map_err(|_| sqlx::Error::Protocol("parent".into()))?,
                network_identity: NetworkIdentity {
                    genesis_hash: genesis
                        .parse()
                        .map_err(|_| sqlx::Error::Protocol("genesis".into()))?,
                    chain_id: chain as u64,
                    p2p_network_id: p2p as u64,
                    address_hrp: hrp,
                },
                block_timestamp_ms: timestamp as u64,
                observed_at: observed
                    .parse()
                    .map_err(|_| sqlx::Error::Protocol("observed".into()))?,
                transaction_count: txs as u64,
                block_interval_ms: interval.map(|v| v as u64),
                source: if source == "gap_backfill" {
                    BlockSource::GapBackfill
                } else {
                    BlockSource::Subscription
                },
                attribution: BlockProductionAttribution {
                    coinbase: coinbase
                        .parse()
                        .map_err(|_| sqlx::Error::Protocol("coinbase".into()))?,
                    seal_signer_key_fingerprint: fingerprint
                        .map(|v| v.parse())
                        .transpose()
                        .map_err(|_| sqlx::Error::Protocol("fingerprint".into()))?,
                    seal_signer_match: match signer.as_str() {
                        "self" => SealSignerMatch::SignerSelf,
                        "other" => SealSignerMatch::Other,
                        _ => SealSignerMatch::Unknown,
                    },
                    node_key: None,
                    seal_recovery_rule: None,
                    seal_evidence: None,
                    protocol_proposer: proposer,
                    attribution_reason: reason,
                },
            })
        })
        .collect()
}
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ResolveError {
    #[error("block resolution RPC failed")]
    Rpc,
    #[error("resolved block identity does not match subscribed header")]
    IdentityMismatch,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum QueueError {
    #[error("head subscription queue is full")]
    Full,
}

/// One Node's bounded subscription queue. It has no shared Agent-level queue.
#[derive(Debug, Clone)]
pub struct HeadSubscription {
    node_id: NodeId,
    capacity: usize,
    queue: VecDeque<HeadHeader>,
}

impl HeadSubscription {
    pub fn new(node_id: NodeId, capacity: usize) -> Self {
        assert!(capacity > 0, "subscription queue capacity must be positive");
        Self {
            node_id,
            capacity,
            queue: VecDeque::with_capacity(capacity),
        }
    }

    pub fn node_id(&self) -> NodeId {
        self.node_id
    }
    pub fn len(&self) -> usize {
        self.queue.len()
    }
    pub fn front_height(&self) -> Option<u64> {
        self.queue.front().map(|header| header.block_number)
    }

    pub fn front_header(&self) -> Option<&HeadHeader> {
        self.queue.front()
    }

    pub fn pop_front(&mut self) -> Option<HeadHeader> {
        self.queue.pop_front()
    }

    pub fn backfill_range(&self, current_head: u64) -> Option<(u64, u64)> {
        self.front_height()
            .and_then(|first| (first < current_head).then_some((first + 1, current_head)))
    }

    pub fn drain_unresolved_range(&mut self) -> Option<(u64, u64)> {
        let first = self.queue.front()?.block_number;
        let last = self.queue.back()?.block_number;
        self.queue.clear();
        Some((first, last))
    }

    /// Drop all queued headers after intake has been cancelled, preserving the
    /// inclusive range for durable shutdown-gap reporting.
    pub fn cancel_intake(&mut self) -> Option<(u64, u64)> {
        self.drain_unresolved_range()
    }

    pub fn is_overflowing(&self) -> bool {
        self.queue.len() >= self.capacity
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn push(&mut self, header: HeadHeader) -> Result<(), QueueError> {
        if self.queue.len() >= self.capacity {
            return Err(QueueError::Full);
        }
        self.queue.push_back(header);
        Ok(())
    }

    pub fn resolve_next<R: BlockResolver>(
        &mut self,
        resolver: &R,
        network_identity: NetworkIdentity,
        observed_at: Rfc3339,
    ) -> Result<Option<BlockSummary>, ResolveError> {
        let Some(header) = self.queue.front().cloned() else {
            return Ok(None);
        };
        let resolved = resolver.get_block_by_hash(&header.block_hash)?;
        if resolved.block_number != header.block_number
            || resolved.block_hash != header.block_hash
            || resolved.parent_hash != header.parent_hash
        {
            return Err(ResolveError::IdentityMismatch);
        }
        if resolved.network_identity != network_identity {
            return Err(ResolveError::IdentityMismatch);
        }
        self.queue.pop_front();
        Ok(Some(BlockSummary {
            node_id: self.node_id,
            network_identity: resolved.network_identity,
            block_number: resolved.block_number,
            block_hash: resolved.block_hash,
            parent_hash: resolved.parent_hash,
            block_timestamp_ms: resolved.block_timestamp_ms,
            observed_at,
            transaction_count: resolved.transaction_hashes.len() as u64,
            block_interval_ms: None,
            source: BlockSource::Subscription,
            attribution: BlockProductionAttribution::unknown_attribution(
                resolved.coinbase,
                "seal recovery rule is not verified for this fork; protocol proposer evidence is unavailable",
            ),
        }))
    }
}

/// Registry of independent per-Node subscriptions. A full queue or resolver
/// error is reported for that Node only.
#[derive(Debug, Default)]
pub struct NodeSubscriptions {
    queues: HashMap<NodeId, HeadSubscription>,
}

impl NodeSubscriptions {
    pub fn register(&mut self, node_id: NodeId, capacity: usize) {
        self.queues
            .entry(node_id)
            .or_insert_with(|| HeadSubscription::new(node_id, capacity));
    }
    pub fn get_mut(&mut self, node_id: &NodeId) -> Option<&mut HeadSubscription> {
        self.queues.get_mut(node_id)
    }
    pub fn len(&self) -> usize {
        self.queues.len()
    }
    pub fn is_empty(&self) -> bool {
        self.queues.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{AgentDatabaseConfig, AgentStore};
    use tempfile::tempdir;

    fn hash(byte: char) -> Hash32 {
        format!("0x{}", byte.to_string().repeat(64))
            .parse()
            .unwrap()
    }
    fn address() -> Address {
        format!("0x{}", "a".repeat(40)).parse().unwrap()
    }
    fn identity() -> NetworkIdentity {
        NetworkIdentity {
            genesis_hash: hash('b'),
            chain_id: 1,
            p2p_network_id: 1,
            address_hrp: Some("lat".to_owned()),
        }
    }
    fn header() -> HeadHeader {
        HeadHeader {
            block_number: 9,
            block_hash: hash('c'),
            parent_hash: hash('d'),
            block_timestamp_ms: 1_000,
            coinbase: address(),
        }
    }
    struct Fake {
        block: ResolvedBlock,
        calls: std::cell::Cell<usize>,
    }
    impl BlockResolver for Fake {
        fn get_block_by_hash(&self, hash: &Hash32) -> Result<ResolvedBlock, ResolveError> {
            self.calls.set(self.calls.get() + 1);
            assert_eq!(hash, &self.block.block_hash);
            Ok(self.block.clone())
        }
    }

    #[test]
    fn queue_is_independent_and_hash_resolution_counts_only_hashes() {
        let node: NodeId = "0195f2a1-0014-4014-8014-000000000014".parse().unwrap();
        let mut sub = HeadSubscription::new(node, 1);
        sub.push(header()).unwrap();
        assert_eq!(sub.push(header()), Err(QueueError::Full));
        let fake = Fake {
            block: ResolvedBlock {
                block_number: 9,
                block_hash: hash('c'),
                parent_hash: hash('d'),
                block_timestamp_ms: 1_000,
                transaction_hashes: vec![hash('e'), hash('f')],
                network_identity: identity(),
                coinbase: address(),
            },
            calls: std::cell::Cell::new(0),
        };
        let summary = sub
            .resolve_next(&fake, identity(), "2026-01-01T00:00:00Z".parse().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(summary.transaction_count, 2);
        assert_eq!(fake.calls.get(), 1);
        assert!(sub.is_empty());
    }

    #[tokio::test]
    async fn persisted_block_summary_round_trips_through_the_agent_store() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let observed_at: Rfc3339 = "2026-01-01T00:00:00Z".parse().unwrap();
        let summary = BlockSummary {
            node_id: "0195f2a1-0014-4014-8014-000000000014".parse().unwrap(),
            network_identity: identity(),
            block_number: 9,
            block_hash: hash('c'),
            parent_hash: hash('d'),
            block_timestamp_ms: 1_000,
            observed_at,
            transaction_count: 2,
            block_interval_ms: None,
            source: BlockSource::Subscription,
            attribution: BlockProductionAttribution::unknown_attribution(
                address(),
                "test attribution",
            ),
        };

        persist_block_summary(&mut store, &summary, &observed_at.to_string())
            .await
            .unwrap();

        let loaded = load_block_summaries(&mut store).await.unwrap();
        assert_eq!(loaded, vec![summary]);
    }

    #[tokio::test]
    async fn loading_block_summaries_is_bounded_by_report_contract() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let observed_at: Rfc3339 = "2026-01-01T00:00:00Z".parse().unwrap();
        let template = BlockSummary {
            node_id: "0195f2a1-0014-4014-8014-000000000014".parse().unwrap(),
            network_identity: identity(),
            block_number: 0,
            block_hash: hash('c'),
            parent_hash: hash('d'),
            block_timestamp_ms: 1_000,
            observed_at,
            transaction_count: 2,
            block_interval_ms: None,
            source: BlockSource::Subscription,
            attribution: BlockProductionAttribution::unknown_attribution(
                address(),
                "test attribution",
            ),
        };

        for block_number in 0..=MAX_BLOCK_SUMMARIES as u64 {
            let mut summary = template.clone();
            summary.block_number = block_number;
            summary.block_hash = format!("0x{block_number:064x}").parse().unwrap();
            summary.parent_hash = format!("0x{:064x}", block_number + 1).parse().unwrap();
            persist_block_summary(&mut store, &summary, &observed_at.to_string())
                .await
                .unwrap();
        }

        let loaded = load_block_summaries(&mut store).await.unwrap();
        assert_eq!(loaded.len(), MAX_BLOCK_SUMMARIES);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM block_summaries")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            (MAX_BLOCK_SUMMARIES + 1) as i64
        );
    }

    struct MissingResolver;
    impl BlockResolver for MissingResolver {
        fn get_block_by_hash(&self, _hash: &Hash32) -> Result<ResolvedBlock, ResolveError> {
            Err(ResolveError::Rpc)
        }
    }

    #[test]
    fn rpc_boundary_rejects_unsafe_methods_and_overflow_timestamps() {
        assert!(method_allowed(
            "eth_getBlockByHash",
            &["eth_getBlockByHash"]
        ));
        assert!(!method_allowed(
            "debug_traceTransaction",
            &["eth_getBlockByHash"]
        ));
        assert_eq!(timestamp_ms("0x1").unwrap(), 1_000);
        assert_eq!(
            timestamp_ms("0xffffffffffffffff"),
            Err(TransportError::Failed(
                "RPC timestamp out of range".to_owned()
            ))
        );
        assert_eq!(
            parse_hash("not-a-hash").unwrap_err(),
            TransportError::Failed("invalid RPC block hash".to_owned())
        );
    }
    #[test]
    fn rpc_request_quantity_and_timestamp_are_bounded() {
        assert_eq!(quantity("0x10").unwrap(), 16);
        assert_eq!(
            quantity("0x").unwrap_err(),
            TransportError::Failed("invalid RPC quantity".to_owned())
        );
        assert_eq!(quantity("0x1").unwrap().saturating_mul(1000), 1000);
    }

    #[test]
    fn backfill_bounds_produce_gap_backfill_and_exact_gap() {
        struct FakeTransport {
            identity: NetworkIdentity,
        }
        impl BlockTransport for FakeTransport {
            fn subscribe_heads(&self, _: &RpcEndpoint) -> Result<Vec<HeadHeader>, TransportError> {
                Ok(vec![])
            }
            fn get_block_by_hash(
                &self,
                _: &RpcEndpoint,
                _: &Hash32,
            ) -> Result<ResolvedBlock, TransportError> {
                Err(TransportError::Unavailable)
            }
            fn get_block_by_number(
                &self,
                _: &RpcEndpoint,
                height: u64,
            ) -> Result<ResolvedBlock, TransportError> {
                Ok(ResolvedBlock {
                    block_number: height,
                    block_hash: hash('c'),
                    parent_hash: hash('d'),
                    block_timestamp_ms: height * 1000,
                    transaction_hashes: vec![],
                    network_identity: self.identity.clone(),
                    coinbase: address(),
                })
            }
        }
        let transport = FakeTransport {
            identity: identity(),
        };
        let endpoint: RpcEndpoint = "ws://127.0.0.1:6790".parse().unwrap();
        let at: Rfc3339 = "2026-01-01T00:00:00Z".parse().unwrap();
        let result = backfill_missing(
            &transport,
            &endpoint,
            "0195f2a1-0014-4014-8014-000000000014".parse().unwrap(),
            identity(),
            at,
            9,
            12,
            BackfillBounds {
                max_height_span: 2,
                max_block_count: 2,
                max_time: Duration::from_secs(1),
            },
            BackfillTrigger::HeightJump,
        );
        assert_eq!(result.summaries.len(), 2);
        assert!(
            result
                .summaries
                .iter()
                .all(|sample| sample.source == BlockSource::GapBackfill)
        );
        assert_eq!(result.gaps[0].from_height, 11);
        assert_eq!(result.gaps[0].to_height, 12);
    }

    #[test]
    fn deterministic_rpc_fake_rejects_oversized_malformed_and_mismatched_blocks() {
        assert!(matches!(
            check_response_size(&serde_json::json!({ "a": "b" }), 1),
            Err(TransportError::Failed(message)) if message.contains("size limit")
        ));
        assert!(check_response_size(&serde_json::json!({ "a": "b" }), 1024).is_ok());
        assert!(matches!(
            parse_block_value(serde_json::json!({ "number": "0x1" }), 1024, 10),
            Err(TransportError::Failed(message)) if message.contains("malformed")
        ));
        let oversized = serde_json::json!({
            "number": "0x1",
            "hash": format!("0x{}", "a".repeat(64)),
            "parentHash": format!("0x{}", "b".repeat(64)),
            "timestamp": "0x1",
            "transactions": vec![format!("0x{}", "c".repeat(64)); 11]
        });
        assert!(matches!(
            parse_block_value(oversized, 1024, 10),
            Err(TransportError::Failed(message)) if message.contains("transaction list")
        ));
        assert!(!method_allowed(
            "debug_traceTransaction",
            &["eth_getBlockByHash"]
        ));

        let wrong = ResolvedBlock {
            block_number: 10,
            block_hash: hash('e'),
            parent_hash: hash('d'),
            block_timestamp_ms: 1_000,
            transaction_hashes: vec![],
            network_identity: identity(),
            coinbase: address(),
        };
        let mut sub =
            HeadSubscription::new("0195f2a1-0014-4014-8014-000000000014".parse().unwrap(), 1);
        sub.push(header()).unwrap();
        let fake = MismatchResolver { block: wrong };
        assert_eq!(
            sub.resolve_next(&fake, identity(), "2026-01-01T00:00:00Z".parse().unwrap()),
            Err(ResolveError::IdentityMismatch)
        );
    }

    #[test]
    fn production_subscription_waits_long_enough_for_the_first_head() {
        assert_eq!(
            WebSocketBlockTransport::default().receive_timeout,
            Duration::from_secs(3)
        );
    }

    #[test]
    fn rpc_block_accepts_checksum_case_coinbase_and_canonicalizes_it() {
        let block = PlatonBlock {
            number: "0x1".to_owned(),
            hash: format!("0x{}", "a".repeat(64)),
            parent_hash: format!("0x{}", "b".repeat(64)),
            timestamp: "0x1".to_owned(),
            miner: Some("0x58CD1c8953742b5a1A946753a8EDb39C1DFE739b".to_owned()),
            transactions: vec![],
        };

        let resolved = WebSocketBlockTransport::default()
            .resolved_from_block(block, &identity())
            .unwrap();

        assert_eq!(
            resolved.coinbase.as_str(),
            "0x58cd1c8953742b5a1a946753a8edb39c1dfe739b"
        );
    }

    struct MismatchResolver {
        block: ResolvedBlock,
    }

    impl BlockResolver for MismatchResolver {
        fn get_block_by_hash(&self, _hash: &Hash32) -> Result<ResolvedBlock, ResolveError> {
            Ok(self.block.clone())
        }
    }

    #[test]
    fn queue_overflow_and_shutdown_are_explicit_recovery_ranges() {
        let node: NodeId = "0195f2a1-0014-4014-8014-000000000014".parse().unwrap();
        let mut sub = HeadSubscription::new(node, 1);
        sub.push(header()).unwrap();
        assert!(sub.is_overflowing());
        assert_eq!(sub.drain_unresolved_range(), Some((9, 9)));
        assert!(sub.is_empty());
    }

    #[test]
    fn mismatch_is_node_local_and_does_not_reconnect_or_consume_other_queue() {
        let node_a: NodeId = "0195f2a1-0014-4014-8014-000000000014".parse().unwrap();
        let node_b: NodeId = "0195f2a1-0015-4015-8015-000000000015".parse().unwrap();
        let mut queues = NodeSubscriptions::default();
        queues.register(node_a, 2);
        queues.register(node_b, 2);
        assert_eq!(queues.len(), 2);
        queues.get_mut(&node_a).unwrap().push(header()).unwrap();
        assert!(queues.get_mut(&node_b).unwrap().is_empty());
        let failure = queues.get_mut(&node_a).unwrap().resolve_next(
            &MissingResolver,
            identity(),
            "2026-01-01T00:00:00Z".parse().unwrap(),
        );
        assert_eq!(failure, Err(ResolveError::Rpc));
        assert_eq!(queues.get_mut(&node_a).unwrap().len(), 1);
    }
}
