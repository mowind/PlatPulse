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
use platpulse_core::envelope::{MAX_BLOCK_TIMESTAMP_MS, check_block_chain_time_ms};
use platpulse_core::hex::{Address, Hash32};
use platpulse_core::identity::NodeId;
use platpulse_core::network::{NetworkIdentity, RpcEndpoint, RpcScheme};
use platpulse_core::protocol::{MAX_BLOCK_SUMMARIES, MAX_HISTORY_GAPS};
use platpulse_core::time::Rfc3339;
use serde::Deserialize;
use sqlx::{Connection, FromRow, SqliteConnection};
use thiserror::Error;
use tokio::sync::broadcast::error::{
    RecvError as BroadcastRecvError, TryRecvError as BroadcastTryRecvError,
};
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
    #[error("malformed head notification: {0}")]
    MalformedHead(String),
    #[error("head notification queue lagged; reconnect required")]
    HeadLagged,
    #[error("head subscription closed; reconnect required")]
    HeadClosed,
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
/// aliases report seconds only while the Node runs with
/// `--http.ethcompatible`; without that flag they report native
/// milliseconds. [`timestamp_ms`] accepts either unit.
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

/// Magnitude that separates Unix milliseconds from Unix seconds (issue #243).
///
/// PlatON reports native milliseconds unless the Node was started with
/// `--http.ethcompatible`, in which case the `eth_*` aliases divide by
/// 1000 and report seconds. The Agent cannot read the flag off the RPC surface,
/// so it decides the unit from the value itself: 1e11 ms is
/// 1973-03-03T09:46:40Z, while the same magnitude read as seconds is the year
/// 5138 — beyond the contract's own 2100 ceiling, so every plausible chain
/// timestamp below the floor is seconds and everything at or above it is
/// already milliseconds.
const MILLISECOND_MAGNITUDE_FLOOR: u64 = 100_000_000_000;

/// Convert an RPC chain timestamp to Unix milliseconds, accepting both the
/// native millisecond form and the `--http.ethcompatible` seconds form.
fn timestamp_ms(value: &str) -> Result<u64, TransportError> {
    let raw = quantity(value)?;
    if raw >= MILLISECOND_MAGNITUDE_FLOOR {
        return Ok(raw);
    }
    // Anything below the floor is seconds, so it is smaller than 1e11 and the
    // scaling cannot overflow. An implausible or out-of-contract timestamp is
    // refused exactly once, at the persistence boundary
    // ([`BlockSummaryStoreError::ChainTimeOutOfContract`]), where the
    // operator gets the actionable message instead of a transport error.
    Ok(raw * 1000)
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
                        remainder: Some((height, to_height)),
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
                    remainder: None,
                };
            }
            summaries.push(BlockSummary { node_id, network_identity: identity.clone(), block_number: block.block_number, block_hash: block.block_hash, parent_hash: block.parent_hash, block_timestamp_ms: block.block_timestamp_ms, observed_at, transaction_count: block.transaction_hashes.len() as u64, block_interval_ms: None, source: BlockSource::GapBackfill, attribution: BlockProductionAttribution::unknown_attribution(block.coinbase, "seal recovery rule is not verified for this fork; protocol proposer evidence is unavailable") });
            height = height.saturating_add(1);
        }
        let (gaps, remainder) = if height <= to_height {
            (
                vec![make_gap(
                    node_id,
                    height,
                    to_height,
                    observed_at,
                    format!("{} exceeded configured backfill bounds", trigger.label()),
                )],
                Some((height, to_height)),
            )
        } else {
            (vec![], None)
        };
        BackfillOutcome {
            summaries,
            gaps,
            remainder,
        }
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

    pub(crate) fn drain_live_heads(
        &self,
        subscription: &mut LiveHeadSubscription,
        queue: &mut HeadSubscription,
    ) {
        loop {
            match subscription.heads.try_recv_result() {
                Ok(Ok(head)) => match self.parse_head(head) {
                    Ok(header) => {
                        if queue.push(header).is_err() {
                            queue.mark_loss();
                        }
                    }
                    Err(_) => queue.mark_loss(),
                },
                Ok(Err(_)) => queue.mark_loss(),
                Err(BroadcastTryRecvError::Empty | BroadcastTryRecvError::Closed) => break,
                Err(BroadcastTryRecvError::Lagged(_)) => queue.mark_loss(),
            }
        }
    }

    fn parse_head(&self, head: PlatonHead) -> Result<HeadHeader, TransportError> {
        Ok(HeadHeader {
            block_number: quantity(&head.number)?,
            block_hash: parse_hash(&head.hash)?,
            parent_hash: parse_hash(&head.parent_hash)?,
            block_timestamp_ms: timestamp_ms(&head.timestamp)?,
            coinbase: parse_address(head.miner)?,
        })
    }

    pub(crate) async fn receive_live_head(
        &self,
        subscription: &mut LiveHeadSubscription,
    ) -> Result<HeadHeader, TransportError> {
        match subscription.heads.recv_result().await {
            Ok(Ok(head)) => self.parse_head(head),
            Ok(Err(error)) => Err(TransportError::MalformedHead(error.to_string())),
            Err(BroadcastRecvError::Lagged(_)) => Err(TransportError::HeadLagged),
            Err(BroadcastRecvError::Closed) => Err(TransportError::HeadClosed),
        }
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
    /// A later step through a range an earlier recovery declared as a gap
    /// because it exceeded its bounds (issue #281).
    Backlog,
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
            Self::Backlog => "declared gap backlog",
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
    /// The declared range a later step may still backfill: set when the
    /// recovery stopped at its bounds or at a failed point query, never on an
    /// identity mismatch, which no retry can repair (issue #281).
    pub remainder: Option<(u64, u64)>,
}

/// How many declared ranges one Node keeps queued for later backfill. The
/// highest ranges win: the Server confirms an unfilled hole missing after its
/// raw Block History window, so the oldest range is the least useful one.
pub(crate) const BACKFILL_BACKLOG_LIMIT: i64 = 16;

/// How many consecutive backlog steps may make no progress before the range
/// is given up; the Server later confirms what stayed missing.
pub(crate) const BACKFILL_BACKLOG_STALL_LIMIT: u32 = 8;

/// The most heights one backlog step queries, whatever the configured
/// recovery bounds allow.
const BACKLOG_STEP_MAX_HEIGHTS: u64 = 64;

/// The longest one backlog step may run, so the live subscription keeps
/// flowing between steps.
const BACKLOG_STEP_MAX_TIME: Duration = Duration::from_secs(1);

/// A declared range queued for later bounded backfill steps (issue #281).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BacklogRange {
    pub from_height: u64,
    pub to_height: u64,
    /// Consecutive steps that recovered nothing.
    pub stalls: u32,
}

impl BacklogRange {
    /// A freshly declared range.
    pub fn declared((from_height, to_height): (u64, u64)) -> Self {
        Self {
            from_height,
            to_height,
            stalls: 0,
        }
    }

    /// What a step over this range leaves queued: the remainder with its
    /// stall count, or None once the range is complete, unrepairable (an
    /// identity mismatch leaves no remainder), or stalled too long.
    pub fn after_step(self, outcome: &BackfillOutcome) -> Option<Self> {
        let (from_height, to_height) = outcome.remainder?;
        let stalls = if outcome.summaries.is_empty() {
            self.stalls.saturating_add(1)
        } else {
            0
        };
        (stalls < BACKFILL_BACKLOG_STALL_LIMIT).then_some(Self {
            from_height,
            to_height,
            stalls,
        })
    }
}

/// The bounds of one backlog step: small enough that the live subscription
/// keeps flowing between steps, whatever the configured recovery bounds are.
pub fn backlog_step_bounds(bounds: BackfillBounds) -> BackfillBounds {
    BackfillBounds {
        max_height_span: bounds.max_height_span.min(BACKLOG_STEP_MAX_HEIGHTS),
        max_block_count: bounds.max_block_count.min(BACKLOG_STEP_MAX_HEIGHTS),
        max_time: bounds.max_time.min(BACKLOG_STEP_MAX_TIME),
    }
}

async fn upsert_backfill_backlog(
    connection: &mut SqliteConnection,
    node: &str,
    range: BacklogRange,
    updated_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO backfill_backlog (node_id, from_height, to_height, stalls, updated_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(node_id, from_height) DO UPDATE SET to_height = MAX(backfill_backlog.to_height, excluded.to_height), stalls = MIN(backfill_backlog.stalls, excluded.stalls), updated_at = excluded.updated_at")
        .bind(node)
        .bind(range.from_height as i64)
        .bind(range.to_height as i64)
        .bind(i64::from(range.stalls))
        .bind(updated_at)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

/// Queue a declared range for later backfill, keeping only the newest
/// [`BACKFILL_BACKLOG_LIMIT`] ranges of the Node.
pub(crate) async fn queue_backfill_backlog(
    connection: &mut SqliteConnection,
    node_id: NodeId,
    range: (u64, u64),
    updated_at: &str,
) -> Result<(), sqlx::Error> {
    let node = node_id.to_string();
    upsert_backfill_backlog(connection, &node, BacklogRange::declared(range), updated_at).await?;
    sqlx::query("DELETE FROM backfill_backlog WHERE node_id = ? AND from_height NOT IN (SELECT from_height FROM backfill_backlog WHERE node_id = ? ORDER BY from_height DESC LIMIT ?)")
        .bind(&node)
        .bind(&node)
        .bind(BACKFILL_BACKLOG_LIMIT)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

/// The lowest queued range of a Node.
pub(crate) async fn next_backfill_backlog(
    connection: &mut SqliteConnection,
    node_id: NodeId,
) -> Result<Option<BacklogRange>, sqlx::Error> {
    let row = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT from_height, to_height, stalls FROM backfill_backlog WHERE node_id = ? ORDER BY from_height LIMIT 1",
    )
    .bind(node_id.to_string())
    .fetch_optional(&mut *connection)
    .await?;
    Ok(row.map(|(from_height, to_height, stalls)| BacklogRange {
        from_height: from_height.max(0) as u64,
        to_height: to_height.max(0) as u64,
        stalls: u32::try_from(stalls).unwrap_or(BACKFILL_BACKLOG_STALL_LIMIT),
    }))
}

/// Replace the range a step worked on with what it left behind.
pub(crate) async fn settle_backfill_backlog(
    connection: &mut SqliteConnection,
    node_id: NodeId,
    worked: BacklogRange,
    next: Option<BacklogRange>,
    updated_at: &str,
) -> Result<(), sqlx::Error> {
    let node = node_id.to_string();
    sqlx::query("DELETE FROM backfill_backlog WHERE node_id = ? AND from_height = ?")
        .bind(&node)
        .bind(worked.from_height as i64)
        .execute(&mut *connection)
        .await?;
    if let Some(next) = next {
        upsert_backfill_backlog(connection, &node, next, updated_at).await?;
    }
    Ok(())
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
                outcome.remainder = Some((height, to_height));
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
        outcome.remainder = Some((height, to_height));
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

pub(crate) fn gap_kind_name(kind: platpulse_core::gap::GapKind) -> &'static str {
    match kind {
        platpulse_core::gap::GapKind::UnrecoverableBackfill => "unrecoverable_backfill",
        platpulse_core::gap::GapKind::SpoolOverflow => "spool_overflow",
        platpulse_core::gap::GapKind::ServerRejected => "server_rejected",
    }
}

pub(crate) async fn insert_history_gap(
    connection: &mut SqliteConnection,
    gap: &platpulse_core::gap::HistoryGap,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO history_gaps (node_id, from_height, to_height, kind, reason, created_at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(gap.node_id.to_string())
        .bind(gap.from_height as i64)
        .bind(gap.to_height as i64)
        .bind(gap_kind_name(gap.kind))
        .bind(&gap.reason)
        .bind(gap.recorded_at.to_string())
        .execute(&mut *connection)
        .await?;
    Ok(())
}

pub async fn persist_history_gap(
    store: &mut AgentStore,
    gap: &platpulse_core::gap::HistoryGap,
) -> Result<(), sqlx::Error> {
    let _write_permit = store.acquire_write().await;
    insert_history_gap(store.connection(), gap).await
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

/// Failure while persisting a Block Summary into the Agent Store.
#[derive(Debug, Error)]
pub enum BlockSummaryStoreError {
    /// The local SQLite Store rejected the statement.
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    /// The sample's chain timestamp is outside the AgentReport v1 contract, so
    /// it must never reach the Store (issue #243). A stored out-of-contract
    /// row is a permanent poison pill: report assembly refuses it on every
    /// collection tick, and the row only leaves the Store once an accepted
    /// report covers its height, which can never happen.
    #[error(
        "Block Summary for node {node_id} at height {block_number} carries chain timestamp {block_timestamp_ms} ms, outside the AgentReport v1 contract limit of 4102444800000 ms (2100-01-01T00:00:00Z); such a value is what a Node without --http.ethcompatible produces when its millisecond timestamp is read as seconds, so check the Node startup flags and its eth_getBlockByNumber timestamp before reporting this as an Agent defect"
    )]
    ChainTimeOutOfContract {
        node_id: NodeId,
        block_number: u64,
        block_timestamp_ms: u64,
    },
}

/// Refuse a sample whose chain timestamp the envelope would refuse, so the
/// Agent Store and the wire contract can never disagree about one value.
fn check_block_summary_chain_time(summary: &BlockSummary) -> Result<(), BlockSummaryStoreError> {
    check_block_chain_time_ms(summary.block_timestamp_ms).map_err(|_| {
        BlockSummaryStoreError::ChainTimeOutOfContract {
            node_id: summary.node_id,
            block_number: summary.block_number,
            block_timestamp_ms: summary.block_timestamp_ms,
        }
    })
}

pub(crate) async fn insert_block_summary(
    connection: &mut SqliteConnection,
    summary: &BlockSummary,
    created_at: &str,
) -> Result<(), BlockSummaryStoreError> {
    check_block_summary_chain_time(summary)?;
    sqlx::query("INSERT OR IGNORE INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, block_interval_ms, source, coinbase, seal_signer_key_fingerprint, seal_signer_match, node_key_fingerprint, node_key_valid_from, node_key_valid_until, node_key_history_complete, seal_recovery_rule, seal_evidence, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(summary.node_id.to_string()).bind(summary.block_number as i64).bind(summary.block_hash.to_string()).bind(summary.parent_hash.to_string())
        .bind(summary.network_identity.genesis_hash.to_string()).bind(summary.network_identity.chain_id as i64).bind(summary.network_identity.p2p_network_id as i64).bind(summary.network_identity.address_hrp.as_deref()).bind(summary.block_timestamp_ms as i64).bind(summary.observed_at.to_string()).bind(summary.transaction_count as i64).bind(summary.block_interval_ms.map(|v| v as i64)).bind(match summary.source { BlockSource::Subscription => "subscription", BlockSource::GapBackfill => "gap_backfill" }).bind(summary.attribution.coinbase.to_string()).bind(summary.attribution.seal_signer_key_fingerprint.as_ref().map(ToString::to_string)).bind(match summary.attribution.seal_signer_match { SealSignerMatch::SignerSelf => "self", SealSignerMatch::Other => "other", SealSignerMatch::Unknown => "unknown" }).bind(summary.attribution.node_key.as_ref().map(|key| key.fingerprint.to_string())).bind(summary.attribution.node_key.as_ref().and_then(|key| key.valid_from.map(|value| value.to_string()))).bind(summary.attribution.node_key.as_ref().and_then(|key| key.valid_until.map(|value| value.to_string()))).bind(summary.attribution.node_key.as_ref().is_some_and(|key| key.history_complete) as i64).bind(summary.attribution.seal_recovery_rule.as_deref()).bind(summary.attribution.seal_evidence.as_deref()).bind(match &summary.attribution.protocol_proposer { ProtocolProposer::Verified { .. } => "verified", ProtocolProposer::Unknown {} => "unknown" }).bind(match &summary.attribution.protocol_proposer { ProtocolProposer::Verified { identity } => Some(identity.as_str()), ProtocolProposer::Unknown {} => None }).bind(&summary.attribution.attribution_reason).bind(created_at).execute(&mut *connection).await?;
    Ok(())
}

/// Persist a resolved sample before constructing a report. Duplicate hash
/// deliveries are idempotent and never mutate the original sample.
pub async fn persist_block_summary(
    store: &mut AgentStore,
    summary: &BlockSummary,
    created_at: &str,
) -> Result<(), BlockSummaryStoreError> {
    // The contract check runs before the write permit: a refused sample must
    // not even open a write transaction.
    check_block_summary_chain_time(summary)?;
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    let result = insert_block_summary(&mut tx, summary, created_at).await;
    match result {
        Ok(()) => tx.commit().await.map_err(BlockSummaryStoreError::from),
        Err(error) => match tx.rollback().await {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(rollback_error.into()),
        },
    }
}

/// Result of the startup sweep that removes Block Summaries outside the
/// AgentReport v1 contract (issue #243).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct OutOfContractPurge {
    /// Deleted samples per Node, ordered by Node id.
    pub purged_by_node: Vec<(NodeId, u64)>,
    /// Total number of deleted samples.
    pub purged: u64,
    /// History Gap rows declared for the discarded heights.
    pub declared_gaps: usize,
}

impl OutOfContractPurge {
    /// True when the Store held nothing to delete.
    pub fn is_empty(&self) -> bool {
        self.purged == 0
    }
}

/// Reason recorded on every History Gap the startup sweep declares.
const PURGED_SAMPLE_GAP_REASON: &str = "Block Summary dropped by the startup sweep: chain timestamp outside the AgentReport v1 contract";

/// Delete every Block Summary of a monitored Node whose chain timestamp the v1
/// contract refuses, and declare the History Gap the discarded heights become.
///
/// A stored out-of-contract sample is a permanent poison pill (issue #243):
/// report assembly refuses it on every collection tick, and the row only
/// leaves the Store once an accepted report covers its height, which can never
/// happen. This sweep performs the same repair an operator would perform by
/// hand against `agent.db` (see docs/deployment.md), before any worker
/// starts.
///
/// Only Nodes the current Inventory declares are swept. A History Gap for a
/// Node the report does not carry is itself refused by the contract
/// (`WireError::HistoryGapForUnknownNode`), so sweeping a retired Node's rows
/// would trade one permanent refusal for another; its rows stay in the Store
/// until the Node is declared again.
///
/// The samples are dropped on purpose, so their heights become explicit
/// `unrecoverable_backfill` History Gaps, one per Node and contiguous range:
/// CONTEXT.md defines a History Gap as the interval for samples that were
/// "lost or intentionally dropped", and a silent hole would let a Node's Block
/// history read as continuous where samples were thrown away. The delete and
/// the gap declarations share one transaction, so a Store can never end up
/// with the rows gone and the loss undeclared.
pub async fn purge_out_of_contract_block_summaries(
    store: &mut AgentStore,
    monitored_nodes: &[NodeId],
) -> Result<OutOfContractPurge, sqlx::Error> {
    if monitored_nodes.is_empty() {
        return Ok(OutOfContractPurge {
            purged_by_node: Vec::new(),
            purged: 0,
            declared_gaps: 0,
        });
    }
    let monitored = monitored_nodes
        .iter()
        .map(|node_id| node_id.to_string())
        .collect::<Vec<_>>();
    let in_scope = vec!["?"; monitored.len()].join(", ");
    let selection = format!(
        "SELECT node_id, block_number FROM block_summaries WHERE block_timestamp_ms NOT BETWEEN 0 AND ? AND node_id IN ({in_scope}) ORDER BY node_id, block_number"
    );
    let deletion = format!(
        "DELETE FROM block_summaries WHERE block_timestamp_ms NOT BETWEEN 0 AND ? AND node_id IN ({in_scope})"
    );
    let _write_permit = store.acquire_write().await;
    let mut tx = store.connection().begin().await?;
    let mut select =
        sqlx::query_as::<_, (String, i64)>(&selection).bind(MAX_BLOCK_TIMESTAMP_MS as i64);
    for node_id in &monitored {
        select = select.bind(node_id);
    }
    let rows = select.fetch_all(&mut *tx).await?;
    let mut delete = sqlx::query(&deletion).bind(MAX_BLOCK_TIMESTAMP_MS as i64);
    for node_id in &monitored {
        delete = delete.bind(node_id);
    }
    let purged = delete.execute(&mut *tx).await?.rows_affected();
    let plan = purged_sample_plan(rows)?;
    let recorded_at = crate::collector::timestamp();
    for (node_id, from_height, to_height) in &plan.ranges {
        insert_history_gap(
            &mut tx,
            &crate::shutdown::shutdown_gap(
                *node_id,
                (*from_height, *to_height),
                recorded_at,
                PURGED_SAMPLE_GAP_REASON,
            ),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(OutOfContractPurge {
        purged_by_node: plan.per_node,
        purged,
        declared_gaps: plan.ranges.len(),
    })
}

/// What one sweep discarded: how many rows left the Store per Node, and the
/// inclusive height ranges their History Gaps cover.
struct PurgedSamplePlan {
    per_node: Vec<(NodeId, u64)>,
    ranges: Vec<(NodeId, u64, u64)>,
}

/// Count the discarded samples per Node and fold one Node's consecutive
/// heights into the inclusive ranges its History Gaps cover. Heights repeat
/// when a Node stored several samples for one block, so a height already
/// inside a range extends nothing.
///
/// A run is split once it would reach beyond
/// [`platpulse_core::protocol::MAX_HISTORY_GAP_HEIGHT_SPAN`]: a wider range
/// violates the contract itself, and every later report would then be refused
/// for a gap the sweep invented - the repair would become the failure it
/// repaired.
fn purged_sample_plan(rows: Vec<(String, i64)>) -> Result<PurgedSamplePlan, sqlx::Error> {
    let mut per_node: Vec<(NodeId, u64)> = Vec::new();
    let mut ranges: Vec<(NodeId, u64, u64)> = Vec::new();
    for (node_id, height) in rows {
        let node_id: NodeId = node_id
            .parse()
            .map_err(|_| sqlx::Error::Protocol("node_id".into()))?;
        let height = height.max(0) as u64;
        match per_node.last_mut() {
            Some((last, count)) if *last == node_id => *count += 1,
            _ => per_node.push((node_id, 1)),
        }
        if ranges
            .last()
            .is_some_and(|(last, _, to)| *last == node_id && height <= *to)
        {
            continue;
        }
        match ranges.last_mut() {
            Some((last, from_height, to_height))
                if *last == node_id
                    && height == to_height.saturating_add(1)
                    && height.saturating_sub(*from_height).saturating_add(1)
                        <= platpulse_core::protocol::MAX_HISTORY_GAP_HEIGHT_SPAN =>
            {
                *to_height = height;
            }
            _ => ranges.push((node_id, height, height)),
        }
    }
    Ok(PurgedSamplePlan { per_node, ranges })
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

/// Load the History Gaps still pending delivery for the Nodes `monitored_nodes`
/// names, oldest first.
///
/// The report only carries the Nodes its Inventory declares, so a gap that
/// outlives its Node's declaration is filtered here: declaring it would be
/// refused as a gap for an unknown Node, which is a failure the Agent cannot
/// repair by itself.
pub async fn load_history_gaps(
    store: &mut AgentStore,
    monitored_nodes: &[NodeId],
) -> Result<Vec<platpulse_core::gap::HistoryGap>, sqlx::Error> {
    if monitored_nodes.is_empty() {
        return Ok(Vec::new());
    }
    let monitored = monitored_nodes
        .iter()
        .map(|node_id| node_id.to_string())
        .collect::<Vec<_>>();
    let in_scope = vec!["?"; monitored.len()].join(", ");
    let query = format!(
        "SELECT g.node_id, g.from_height, g.to_height, g.kind, g.created_at, g.reason FROM history_gaps g WHERE g.node_id IN ({in_scope}) AND NOT EXISTS (SELECT 1 FROM report_sample_assignments a WHERE a.node_id=g.node_id AND a.sample_kind='gap' AND a.from_height=g.from_height AND a.to_height=g.to_height) AND g.gap_id = (SELECT MIN(candidate.gap_id) FROM history_gaps candidate WHERE candidate.node_id=g.node_id AND candidate.from_height=g.from_height AND candidate.to_height=g.to_height AND NOT EXISTS (SELECT 1 FROM report_sample_assignments a WHERE a.node_id=candidate.node_id AND a.sample_kind='gap' AND a.from_height=candidate.from_height AND a.to_height=candidate.to_height)) ORDER BY g.created_at, g.gap_id LIMIT ?"
    );
    let mut select = sqlx::query_as::<_, (String, i64, i64, String, String, String)>(&query);
    for node_id in &monitored {
        select = select.bind(node_id);
    }
    let rows = select
        .bind(MAX_HISTORY_GAPS as i64)
        .fetch_all(store.connection())
        .await?;
    rows.into_iter()
        .map(|(node_id, from, to, kind, at, reason)| {
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
                // The reason recorded with the gap travels with it (issue
                // #281), bounded to the contract limit; an empty one falls
                // back to a neutral description.
                reason: if reason.trim().is_empty() {
                    "locally retained history gap".to_owned()
                } else {
                    reason.chars().take(512).collect()
                },
                recorded_at: at
                    .parse()
                    .map_err(|_| sqlx::Error::Protocol("gap timestamp".into()))?,
            })
        })
        .collect()
}

/// Load all pending block summaries in deterministic oldest-first order.
///
/// Samples whose chain timestamp is outside the AgentReport v1 contract are
/// never handed to report assembly (issue #243): a single stored
/// out-of-contract row would otherwise refuse every report forever. The bound
/// applies to the dedup subquery too, so a good sample at the same height is
/// still selected instead of being starved by a poisoned one.
pub async fn load_block_summaries(
    store: &mut AgentStore,
) -> Result<Vec<BlockSummary>, sqlx::Error> {
    let rows = sqlx::query_as::<_, BlockRow>("SELECT b.node_id, b.block_number, b.block_hash, b.parent_hash, b.network_genesis_hash, b.network_chain_id, b.network_p2p_network_id, b.network_address_hrp, b.block_timestamp_ms, b.observed_at, b.transaction_count, b.block_interval_ms, b.source, b.coinbase, b.seal_signer_key_fingerprint, b.seal_signer_match, b.protocol_proposer_kind, b.protocol_proposer_identity, b.attribution_reason FROM block_summaries b WHERE b.block_timestamp_ms BETWEEN 0 AND ? AND NOT EXISTS (SELECT 1 FROM report_sample_assignments a WHERE a.node_id=b.node_id AND a.sample_kind='block' AND a.from_height=b.block_number AND a.to_height=b.block_number) AND b.sample_id = (SELECT MIN(candidate.sample_id) FROM block_summaries candidate WHERE candidate.node_id=b.node_id AND candidate.block_number=b.block_number AND candidate.block_timestamp_ms BETWEEN 0 AND ? AND NOT EXISTS (SELECT 1 FROM report_sample_assignments a WHERE a.node_id=candidate.node_id AND a.sample_kind='block' AND a.from_height=candidate.block_number AND a.to_height=candidate.block_number)) ORDER BY b.created_at, b.sample_id LIMIT ?")
        .bind(MAX_BLOCK_TIMESTAMP_MS as i64)
        .bind(MAX_BLOCK_TIMESTAMP_MS as i64)
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
    loss_from: Option<u64>,
    loss_to: Option<u64>,
    last_resolved_height: Option<u64>,
}

impl HeadSubscription {
    pub fn new(node_id: NodeId, capacity: usize) -> Self {
        assert!(capacity > 0, "subscription queue capacity must be positive");
        Self {
            node_id,
            capacity,
            queue: VecDeque::with_capacity(capacity),
            loss_from: None,
            loss_to: None,
            last_resolved_height: None,
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
        let header = self.queue.pop_front()?;
        self.last_resolved_height = Some(header.block_number);
        Some(header)
    }

    pub fn backfill_range(&self, current_head: u64) -> Option<(u64, u64)> {
        self.front_height()
            .and_then(|first| (first < current_head).then_some((first + 1, current_head)))
    }

    pub fn drain_unresolved_range(&mut self) -> Option<(u64, u64)> {
        let queued_first = self.queue.front().map(|header| header.block_number);
        let queued_last = self.queue.back().map(|header| header.block_number);
        let loss_range = self.loss_from.map(|from| {
            let to = self.loss_to.unwrap_or(from);
            (from, queued_last.map_or(to, |queued| to.max(queued)))
        });
        self.queue.clear();
        self.loss_from = None;
        self.loss_to = None;
        match (loss_range, queued_first.zip(queued_last)) {
            (Some(loss), Some(queued)) => Some((loss.0.min(queued.0), loss.1.max(queued.1))),
            (Some(loss), None) => Some(loss),
            (None, Some(queued)) => Some(queued),
            (None, None) => None,
        }
    }

    pub(crate) fn mark_loss(&mut self) {
        if self.loss_from.is_none() {
            self.loss_from = Some(
                self.queue
                    .back()
                    .map(|header| header.block_number.saturating_add(1))
                    .or_else(|| {
                        self.last_resolved_height
                            .map(|height| height.saturating_add(1))
                    })
                    .unwrap_or(0),
            );
        }
    }

    /// Mark the current loss range as covered by a successful recovery pass
    /// without disturbing headers that were already queued.
    pub(crate) fn clear_loss(&mut self) {
        self.loss_from = None;
        self.loss_to = None;
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
        if let (Some(from), None) = (self.loss_from, self.loss_to) {
            self.loss_to = Some(header.block_number.saturating_sub(1).max(from));
        }
        if self.queue.len() >= self.capacity {
            self.mark_loss();
            self.loss_to = Some(header.block_number);
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
    use crate::database::{AgentDatabaseConfig, AgentStore, AgentStoreWritePermit};
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

    #[test]
    fn lost_head_notifications_become_an_explicit_unresolved_range() {
        let node: NodeId = "0195f2a1-0014-4014-8014-000000000014".parse().unwrap();
        let mut sub = HeadSubscription::new(node, 4);
        let mut resolved = header();
        resolved.block_number = 10;
        sub.push(resolved).unwrap();
        assert!(sub.pop_front().is_some());
        sub.mark_loss();
        let mut retained = header();
        retained.block_number = 13;
        sub.push(retained).unwrap();
        assert!(sub.pop_front().is_some());
        assert_eq!(sub.drain_unresolved_range(), Some((11, 12)));
    }

    #[test]
    fn successful_recovery_can_acknowledge_loss_without_dropping_queued_heads() {
        let node: NodeId = "0195f2a1-0014-4014-8014-000000000014".parse().unwrap();
        let mut sub = HeadSubscription::new(node, 4);
        sub.mark_loss();
        sub.push(header()).unwrap();
        sub.clear_loss();
        assert_eq!(sub.len(), 1);
        assert_eq!(sub.drain_unresolved_range(), Some((9, 9)));
    }

    #[tokio::test]
    async fn collection_delivery_and_block_writes_serialize_without_busy_errors() {
        let dir = tempdir().unwrap();
        let database = dir.path().join("agent.db");
        let permit = AgentStoreWritePermit::new();
        let permit_observer = permit.clone();
        let acquisition_notify = permit_observer.test_acquisition_notify();
        let database_config =
            || AgentDatabaseConfig::new(&database).with_busy_timeout(Duration::from_millis(20));
        let mut block_store = AgentStore::open_with_write_permit(database_config(), permit.clone())
            .await
            .unwrap();
        let mut collection_store =
            AgentStore::open_with_write_permit(database_config(), permit.clone())
                .await
                .unwrap();
        let mut delivery_store =
            AgentStore::open_with_write_permit(database_config(), permit.clone())
                .await
                .unwrap();
        let mut blocking_store =
            AgentStore::open_with_write_permit(database_config(), permit.clone())
                .await
                .unwrap();
        let baseline_acquisition_attempts = permit_observer.test_acquisition_attempts();
        let observed_at: Rfc3339 = "2026-01-01T00:00:00Z".parse().unwrap();
        let summary = BlockSummary {
            node_id: test_node_id(),
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
        let body = include_bytes!("../../platpulse-core/tests/fixtures/report_v1_minimal.json");
        let observed_at_text = observed_at.to_string();

        let held = blocking_store.acquire_write().await;
        let mut blocking_transaction = blocking_store.connection().begin().await.unwrap();
        sqlx::query("UPDATE agent_state SET report_sequence = report_sequence WHERE singleton = 1")
            .execute(&mut *blocking_transaction)
            .await
            .unwrap();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(4));
        let block_barrier = barrier.clone();
        let block_task = tokio::spawn(async move {
            block_barrier.wait().await;
            let result = persist_block_summary(&mut block_store, &summary, &observed_at_text).await;
            (result, block_store)
        });
        let collection_barrier = barrier.clone();
        let collection_task = tokio::spawn(async move {
            collection_barrier.wait().await;
            let result = crate::reporting::persist_immutable_report(
                &mut collection_store,
                "0195f2a1-0013-4013-8013-000000000013",
                1,
                "0195f2a1-0012-4012-8012-000000000012",
                1,
                "2026-08-12T09:00:00Z",
                body,
            )
            .await;
            (result, collection_store)
        });
        let delivery_barrier = barrier.clone();
        let delivery_task = tokio::spawn(async move {
            delivery_barrier.wait().await;
            let result = crate::reporting::record_delivery_failure(
                &mut delivery_store,
                "test delivery failure",
                "2026-01-01T00:00:00Z",
            )
            .await;
            (result, delivery_store)
        });
        barrier.wait().await;
        loop {
            let notified = acquisition_notify.notified();
            if permit_observer.test_acquisition_attempts() >= baseline_acquisition_attempts + 4 {
                break;
            }
            notified.await;
        }
        assert!(!block_task.is_finished());
        assert!(!collection_task.is_finished());
        assert!(!delivery_task.is_finished());
        blocking_transaction.commit().await.unwrap();
        drop(held);
        blocking_store.close().await.unwrap();

        let (block_result, block_store) = block_task.await.unwrap();
        let (collection_result, collection_store) = collection_task.await.unwrap();
        let (delivery_result, delivery_store) = delivery_task.await.unwrap();

        assert!(block_result.is_ok(), "block write failed: {block_result:?}");
        assert!(
            collection_result.is_ok(),
            "collection write failed: {collection_result:?}"
        );
        assert!(
            delivery_result.is_ok(),
            "delivery write failed: {delivery_result:?}"
        );
        block_store.close().await.unwrap();
        collection_store.close().await.unwrap();
        delivery_store.close().await.unwrap();
    }

    #[tokio::test]
    async fn persisted_block_summary_round_trips_through_the_agent_store() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let observed_at: Rfc3339 = "2026-01-01T00:00:00Z".parse().unwrap();
        let summary = BlockSummary {
            node_id: test_node_id(),
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
            node_id: test_node_id(),
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

    #[tokio::test]
    async fn pending_loaders_choose_one_row_per_receipt_key() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let observed_at: Rfc3339 = "2026-01-01T00:00:00Z".parse().unwrap();
        let mut first = BlockSummary {
            node_id: test_node_id(),
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
        persist_block_summary(&mut store, &first, &observed_at.to_string())
            .await
            .unwrap();
        first.block_hash = hash('e');
        persist_block_summary(&mut store, &first, &observed_at.to_string())
            .await
            .unwrap();
        let loaded_blocks = load_block_summaries(&mut store).await.unwrap();
        assert_eq!(loaded_blocks.len(), 1);
        assert_eq!(loaded_blocks[0].block_hash, hash('c'));

        let mut first_gap = platpulse_core::gap::HistoryGap {
            node_id: first.node_id,
            kind: platpulse_core::gap::GapKind::UnrecoverableBackfill,
            from_height: 20,
            to_height: 21,
            reason: "first gap".to_owned(),
            recorded_at: observed_at,
        };
        persist_history_gap(&mut store, &first_gap).await.unwrap();
        first_gap.kind = platpulse_core::gap::GapKind::ServerRejected;
        persist_history_gap(&mut store, &first_gap).await.unwrap();
        let loaded_gaps = load_history_gaps(&mut store, &[first.node_id])
            .await
            .unwrap();
        assert_eq!(loaded_gaps.len(), 1);
        assert_eq!(
            loaded_gaps[0].kind,
            platpulse_core::gap::GapKind::UnrecoverableBackfill
        );
    }

    /// The Node answers in either unit (issue #243): PlatON native headers carry
    /// milliseconds, while --http.ethcompatible makes the eth_* aliases divide by
    /// 1000. The magnitude decides, and the two readings of one magnitude cannot
    /// be confused because the seconds reading of a millisecond value lands far
    /// beyond the 2100 contract ceiling.
    #[test]
    fn timestamp_unit_is_chosen_by_magnitude() {
        // Seconds, as an --http.ethcompatible Node reports them (the issue tabulates
        // this height as 2026-10-15T03:24:00Z).
        assert_eq!(timestamp_ms("0x6ad04a70").unwrap(), 1_792_035_440_000);
        // Native milliseconds, as a Node without the flag reports them: the header
        // time of the very block the issue reports, 1_792_034_712_000 ms.
        assert_eq!(timestamp_ms("0x1a13d97a9c0").unwrap(), 1_792_034_712_000);
        // What a pre-guard build stored for that same block: the native milliseconds
        // read as seconds and multiplied by 1000. That magnitude is already
        // milliseconds, so it is passed through here and refused at persistence.
        assert_eq!(
            timestamp_ms("0x65dd8986f1600").unwrap(),
            1_792_034_712_000_000
        );
        // The floor itself is already milliseconds: 1973-03-03T09:46:40Z.
        assert_eq!(
            timestamp_ms(&format!("0x{:x}", MILLISECOND_MAGNITUDE_FLOOR)).unwrap(),
            MILLISECOND_MAGNITUDE_FLOOR
        );
        assert_eq!(
            timestamp_ms(&format!("0x{:x}", MILLISECOND_MAGNITUDE_FLOOR - 1)).unwrap(),
            (MILLISECOND_MAGNITUDE_FLOOR - 1) * 1_000
        );
    }

    /// The value a real millisecond Node reports survives the whole path (issue
    /// #243, item (c)): parsed from the RPC quantity, written past the guard, and
    /// accepted by the wire contract - the block that used to become a poison pill.
    #[tokio::test]
    async fn native_millisecond_value_round_trips_through_store_and_contract() {
        let observed_at: Rfc3339 = "2026-10-15T03:24:42Z".parse().unwrap();
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let native_ms = timestamp_ms("0x1a13d97a9c0").unwrap();
        assert_eq!(native_ms, 1_792_034_712_000);

        let sample = summary(161_253_346, 'c', native_ms, observed_at);
        persist_block_summary(&mut store, &sample, &observed_at.to_string())
            .await
            .unwrap();
        let loaded = load_block_summaries(&mut store).await.unwrap();
        assert_eq!(loaded, vec![sample]);

        let mut report: platpulse_core::AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report.block_summaries = loaded;
        assert_eq!(report.validate(), Ok(()));
    }

    fn out_of_contract_timestamp() -> u64 {
        1_792_034_712_000_000
    }

    fn test_node_id() -> NodeId {
        "0195f2a1-0014-4014-8014-000000000014".parse().unwrap()
    }

    /// A Node the fixture Inventory does not declare, used for the rows and
    /// gaps a sweep must leave alone.
    fn retired_node_id() -> NodeId {
        "0195f2a1-0015-4015-8015-000000000015".parse().unwrap()
    }

    fn gap(
        node_id: NodeId,
        from_height: u64,
        to_height: u64,
        recorded_at: Rfc3339,
    ) -> platpulse_core::gap::HistoryGap {
        platpulse_core::gap::HistoryGap {
            node_id,
            kind: platpulse_core::gap::GapKind::UnrecoverableBackfill,
            from_height,
            to_height,
            reason: "test gap".to_owned(),
            recorded_at,
        }
    }

    /// Poison one row for `node_id`: written valid, then edited to the value a
    /// build that predates the guard would have stored.
    async fn poison_row(
        store: &mut AgentStore,
        node_id: NodeId,
        block_number: u64,
        observed_at: Rfc3339,
    ) {
        let mut sample = summary(block_number, 'c', 1_000, observed_at);
        sample.node_id = node_id;
        persist_block_summary(store, &sample, &observed_at.to_string())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE block_summaries SET block_timestamp_ms = ? WHERE node_id = ? AND block_number = ?",
        )
        .bind(out_of_contract_timestamp() as i64)
        .bind(node_id.to_string())
        .bind(block_number as i64)
        .execute(store.connection())
        .await
        .unwrap();
    }

    fn summary(
        block_number: u64,
        block_hash: char,
        block_timestamp_ms: u64,
        observed_at: Rfc3339,
    ) -> BlockSummary {
        BlockSummary {
            node_id: test_node_id(),
            network_identity: identity(),
            block_number,
            block_hash: hash(block_hash),
            parent_hash: hash('d'),
            block_timestamp_ms,
            observed_at,
            transaction_count: 2,
            block_interval_ms: None,
            source: BlockSource::Subscription,
            attribution: BlockProductionAttribution::unknown_attribution(
                address(),
                "test attribution",
            ),
        }
    }

    async fn poisoned_store(block_number: u64, observed_at: Rfc3339) -> (AgentStore, BlockSummary) {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let mut poisoned = summary(block_number, 'c', 1_000, observed_at);
        persist_block_summary(&mut store, &poisoned, &observed_at.to_string())
            .await
            .unwrap();
        // Written behind the guard, exactly like a row left by a build that
        // predates it: the Agent must recover instead of failing forever.
        sqlx::query("UPDATE block_summaries SET block_timestamp_ms = ? WHERE node_id = ? AND block_number = ?")
            .bind(out_of_contract_timestamp() as i64)
            .bind(poisoned.node_id.to_string())
            .bind(block_number as i64)
            .execute(store.connection())
            .await
            .unwrap();
        // The row now holds the value the pre-guard build would have stored;
        // hand back the same sample in memory so callers can compare with it.
        poisoned.block_timestamp_ms = out_of_contract_timestamp();
        (store, poisoned)
    }

    /// Issue #243 regression: the sample a millisecond-only Node produced must
    /// never reach the Store. A stored one is a permanent poison pill - every
    /// later report is refused, and the row only leaves the Store once an
    /// accepted report covers its height, which can never happen.
    #[tokio::test]
    async fn out_of_contract_sample_is_refused_before_it_reaches_the_store() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let observed_at: Rfc3339 = "2026-10-15T03:24:42Z".parse().unwrap();
        let poisoned = summary(161_253_346, 'c', out_of_contract_timestamp(), observed_at);

        let error = persist_block_summary(&mut store, &poisoned, &observed_at.to_string())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            BlockSummaryStoreError::ChainTimeOutOfContract {
                block_number: 161_253_346,
                block_timestamp_ms: 1_792_034_712_000_000,
                ..
            }
        ));
        let message = error.to_string();
        assert!(
            message.contains("0195f2a1-0014-4014-8014-000000000014"),
            "{message}"
        );
        assert!(
            message.contains(&MAX_BLOCK_TIMESTAMP_MS.to_string()),
            "{message}"
        );
        assert!(message.contains("--http.ethcompatible"), "{message}");
        assert_eq!(MAX_BLOCK_TIMESTAMP_MS, 4_102_444_800_000);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM block_summaries")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0
        );
        assert!(load_block_summaries(&mut store).await.unwrap().is_empty());
    }

    /// A Store poisoned before the guard existed must not stall the Agent
    /// (issue #243). The loader skips out-of-contract rows, and the dedup
    /// subquery applies the same bound so a good sample at the poisoned height
    /// is still selected instead of being starved by a smaller sample id.
    ///
    /// Skipping is not deleting: the row stays in the Store, so the Agent keeps
    /// its evidence until the startup sweep removes it and declares the height
    /// as a History Gap.
    #[tokio::test]
    async fn out_of_contract_rows_are_not_loaded_and_do_not_starve_a_good_sample() {
        let observed_at: Rfc3339 = "2026-10-15T03:24:42Z".parse().unwrap();
        let (mut store, poisoned) = poisoned_store(9, observed_at).await;
        let good = summary(9, 'e', 1_000, observed_at);
        persist_block_summary(&mut store, &good, &observed_at.to_string())
            .await
            .unwrap();

        assert_eq!(load_block_summaries(&mut store).await.unwrap(), vec![good]);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM block_summaries")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            2,
            "the loader excludes the poisoned row instead of deleting it"
        );
        assert_eq!(
            load_history_gaps(&mut store, &[poisoned.node_id])
                .await
                .unwrap(),
            vec![]
        );
        assert_eq!(poisoned.block_timestamp_ms, out_of_contract_timestamp());
    }

    /// The startup sweep is the automatic form of the operator procedure in
    /// docs/deployment.md: it removes the poison, reports what it removed, and
    /// declares the affected heights as History Gaps so the loss is stated
    /// instead of reading as continuous history. In-contract samples stay.
    #[tokio::test]
    async fn startup_purge_removes_out_of_contract_rows_and_declares_gaps() {
        let observed_at: Rfc3339 = "2026-10-15T03:24:42Z".parse().unwrap();
        let (mut store, poisoned) = poisoned_store(161_253_346, observed_at).await;
        let good = summary(9, 'e', 1_000, observed_at);
        persist_block_summary(&mut store, &good, &observed_at.to_string())
            .await
            .unwrap();

        let purge = purge_out_of_contract_block_summaries(&mut store, &[poisoned.node_id])
            .await
            .unwrap();
        assert!(!purge.is_empty());
        assert_eq!(purge.purged, 1);
        assert_eq!(purge.purged_by_node, vec![(poisoned.node_id, 1)]);
        assert_eq!(purge.declared_gaps, 1);
        assert_eq!(load_block_summaries(&mut store).await.unwrap(), vec![good]);

        let gaps = load_history_gaps(&mut store, &[poisoned.node_id])
            .await
            .unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].node_id, poisoned.node_id);
        assert_eq!(
            (gaps[0].from_height, gaps[0].to_height),
            (161_253_346, 161_253_346)
        );
        assert_eq!(
            gaps[0].kind,
            platpulse_core::gap::GapKind::UnrecoverableBackfill
        );
        // The loader reports every locally retained gap under one reason, so the
        // sweep wording is asserted where it is stored.
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT reason FROM history_gaps")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            PURGED_SAMPLE_GAP_REASON
        );

        let again = purge_out_of_contract_block_summaries(&mut store, &[poisoned.node_id])
            .await
            .unwrap();
        assert!(again.is_empty());
        assert_eq!(again.purged, 0);
        assert_eq!(again.declared_gaps, 0);
        assert!(again.purged_by_node.is_empty());
        assert_eq!(
            load_history_gaps(&mut store, &[poisoned.node_id])
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// Consecutive poisoned heights become one declared range per Node, so a
    /// long outage does not spray one row per block into history_gaps.
    #[tokio::test]
    async fn purge_folds_consecutive_heights_into_one_gap_range_per_node() {
        let observed_at: Rfc3339 = "2026-10-15T03:24:42Z".parse().unwrap();
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        for block_number in [100_u64, 101, 102, 200] {
            let sample = summary(block_number, 'c', 1_000, observed_at);
            persist_block_summary(&mut store, &sample, &observed_at.to_string())
                .await
                .unwrap();
        }
        sqlx::query("UPDATE block_summaries SET block_timestamp_ms = ?")
            .bind(out_of_contract_timestamp() as i64)
            .execute(store.connection())
            .await
            .unwrap();

        let purge = purge_out_of_contract_block_summaries(&mut store, &[test_node_id()])
            .await
            .unwrap();
        assert_eq!(purge.purged, 4);
        assert_eq!(purge.declared_gaps, 2);
        let gaps = load_history_gaps(&mut store, &[test_node_id()])
            .await
            .unwrap();
        assert_eq!(gaps.len(), 2);
        assert_eq!((gaps[0].from_height, gaps[0].to_height), (100, 102));
        assert_eq!((gaps[1].from_height, gaps[1].to_height), (200, 200));
    }

    /// A Node the Inventory no longer declares is not swept (issue #243): a
    /// History Gap for a Node the report does not carry is refused by the
    /// contract, so deleting those rows would trade the poison pill for a
    /// permanent gap refusal. The rows wait until the Node is declared again.
    #[tokio::test]
    async fn purge_leaves_the_rows_of_nodes_outside_the_inventory_alone() {
        let observed_at: Rfc3339 = "2026-10-15T03:24:42Z".parse().unwrap();
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        poison_row(&mut store, test_node_id(), 161_253_346, observed_at).await;
        poison_row(&mut store, retired_node_id(), 161_253_347, observed_at).await;

        let purge = purge_out_of_contract_block_summaries(&mut store, &[test_node_id()])
            .await
            .unwrap();
        assert_eq!(purge.purged, 1);
        assert_eq!(purge.purged_by_node, vec![(test_node_id(), 1)]);
        assert_eq!(purge.declared_gaps, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM block_summaries WHERE node_id = ?")
                .bind(retired_node_id().to_string())
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1,
            "the retired Node keeps its rows until it is declared again"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM history_gaps WHERE node_id = ?")
                .bind(retired_node_id().to_string())
                .fetch_one(store.connection())
                .await
                .unwrap(),
            0,
            "no History Gap is declared for a Node outside the Inventory"
        );
    }

    /// The gap loader reports only the Nodes the report declares: a gap that
    /// outlives its Node's Inventory entry would otherwise make every report
    /// fatal with a contract error the Agent cannot repair.
    #[tokio::test]
    async fn gaps_of_nodes_outside_the_inventory_are_never_loaded() {
        let observed_at: Rfc3339 = "2026-10-15T03:24:42Z".parse().unwrap();
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        persist_history_gap(&mut store, &gap(test_node_id(), 20, 21, observed_at))
            .await
            .unwrap();
        persist_history_gap(&mut store, &gap(retired_node_id(), 30, 31, observed_at))
            .await
            .unwrap();

        let loaded = load_history_gaps(&mut store, &[test_node_id()])
            .await
            .unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].node_id, test_node_id());

        let both = load_history_gaps(&mut store, &[test_node_id(), retired_node_id()])
            .await
            .unwrap();
        assert_eq!(both.len(), 2);
    }

    /// The sweep must not invent a gap the contract refuses (issue #243): a run
    /// of poisoned heights longer than the wire span limit is split, and the
    /// ranges it declares still validate as one report.
    #[test]
    fn the_wire_span_limit_splits_a_long_purged_run() {
        let observed_at: Rfc3339 = "2026-10-15T03:24:42Z".parse().unwrap();
        let rows = (0..=1_000_000_i64)
            .map(|height| (test_node_id().to_string(), height))
            .collect::<Vec<_>>();

        let plan = purged_sample_plan(rows).unwrap();
        assert_eq!(plan.per_node, vec![(test_node_id(), 1_000_001)]);
        assert_eq!(
            plan.ranges,
            vec![
                (test_node_id(), 0, 999_999),
                (test_node_id(), 1_000_000, 1_000_000),
            ]
        );
        for (_, from_height, to_height) in &plan.ranges {
            assert!(
                to_height.saturating_sub(*from_height)
                    < platpulse_core::protocol::MAX_HISTORY_GAP_HEIGHT_SPAN
            );
        }

        let mut report: platpulse_core::AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report.history_gaps = plan
            .ranges
            .iter()
            .map(|(node_id, from_height, to_height)| {
                gap(*node_id, *from_height, *to_height, observed_at)
            })
            .collect();
        assert_eq!(report.validate(), Ok(()));
    }

    /// The guard, the Store loader and the wire contract must agree on one
    /// number; the operator-facing message quotes it, so it is asserted here.
    #[test]
    fn the_refusal_message_quotes_the_shared_contract_limit() {
        let refused = summary(
            1,
            'c',
            out_of_contract_timestamp(),
            "2026-01-01T00:00:00Z".parse().unwrap(),
        );
        let error = check_block_summary_chain_time(&refused).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&MAX_BLOCK_TIMESTAMP_MS.to_string())
        );
        let at_the_limit = summary(
            1,
            'c',
            MAX_BLOCK_TIMESTAMP_MS,
            "2026-01-01T00:00:00Z".parse().unwrap(),
        );
        assert!(check_block_summary_chain_time(&at_the_limit).is_ok());
    }

    /// The regression the issue requires to land with the fix, kept under the
    /// name the issue uses so it is greppable:
    /// `cargo test -p platpulse-agent --lib zz_repro_out_of_contract`.
    ///
    /// The issue's reproduction persisted a sample whose chain timestamp came
    /// from a Node answering native milliseconds, loaded it into a report, and
    /// watched every tick fail with `invalid persisted Agent identity: field
    /// block.block_timestamp_ms is outside its contract range` until systemd
    /// restarted the Agent forever. The assertions below follow those steps and
    /// state what each fix changed: (a) a fresh write of the value is refused,
    /// (b) the loader no longer selects the poisoned row so the tick validates,
    /// and (e) the failure that remains for a hand-attached sample is named as a
    /// stored-sample problem, not an Agent identity problem.
    #[tokio::test]
    async fn zz_repro_out_of_contract() {
        let observed_at: Rfc3339 = "2026-10-15T03:24:42Z".parse().unwrap();
        let (mut store, poisoned) = poisoned_store(161_253_346, observed_at).await;
        let poisoned_node_id = poisoned.node_id;
        assert_eq!(poisoned.block_timestamp_ms, out_of_contract_timestamp());
        let mut report: platpulse_core::AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();

        // Fix (a): the same value can no longer enter the Store at all.
        let fresh = summary(161_253_347, 'e', out_of_contract_timestamp(), observed_at);
        assert!(matches!(
            persist_block_summary(&mut store, &fresh, &observed_at.to_string())
                .await
                .unwrap_err(),
            BlockSummaryStoreError::ChainTimeOutOfContract {
                block_number: 161_253_347,
                ..
            }
        ));

        // Fix (b): the row written behind the guard is skipped on every reload,
        // so the tick validates instead of failing the way the issue reports.
        for tick in 0..3 {
            report.block_summaries = load_block_summaries(&mut store).await.unwrap();
            assert!(report.block_summaries.is_empty(), "tick {tick}");
            assert_eq!(report.validate(), Ok(()), "tick {tick}");
        }
        // Skipping is not deleting: the poison is still there for the sweep.
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM block_summaries")
                .fetch_one(store.connection())
                .await
                .unwrap(),
            1
        );

        // The issue's own assertion, kept in the sample the issue attached by
        // hand: the exact contract error and its exact wording.
        report.block_summaries.push(poisoned);
        let error = report.validate().unwrap_err();
        assert_eq!(
            error,
            platpulse_core::WireError::ValueOutOfRange {
                field: platpulse_core::envelope::BLOCK_TIMESTAMP_FIELD,
            }
        );
        assert_eq!(
            error.to_string(),
            "field block.block_timestamp_ms is outside its contract range"
        );

        // Fix (e): that failure is a stored-sample problem and it is classified
        // as one, instead of impersonating a non-deferrable identity error.
        let fatal = crate::collector::report_validation_error(error);
        assert!(matches!(
            fatal,
            crate::collector::CollectionError::PersistedSampleOutOfContract(_)
        ));
        assert_eq!(
            fatal.to_string(),
            "persisted Agent sample is outside the AgentReport v1 contract: field block.block_timestamp_ms is outside its contract range"
        );
        assert!(!fatal.to_string().contains("Agent identity"));
        assert!(!crate::collector::is_deferrable_collection(&fatal));

        // The startup sweep is what removes the poison, and the height it held
        // is declared as a History Gap so the history is not silently short.
        let purge = purge_out_of_contract_block_summaries(&mut store, &[poisoned_node_id])
            .await
            .unwrap();
        assert_eq!(purge.purged, 1);
        assert_eq!(purge.declared_gaps, 1);
        assert_eq!(purge.purged_by_node, vec![(poisoned_node_id, 1)]);
        let gaps = load_history_gaps(&mut store, &[poisoned_node_id])
            .await
            .unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].node_id, poisoned_node_id);
        assert_eq!(
            (gaps[0].from_height, gaps[0].to_height),
            (161_253_346, 161_253_346)
        );

        report.block_summaries = load_block_summaries(&mut store).await.unwrap();
        assert_eq!(report.validate(), Ok(()));
    }

    /// Issue #243 end to end, along the path the issue reports: a Store poisoned
    /// behind the guard loads no sample, so report assembly validates instead of
    /// failing on every tick; the same sample attached by hand still reproduces
    /// the exact contract error and its wording.
    #[tokio::test]
    async fn a_poisoned_store_no_longer_refuses_report_assembly() {
        let observed_at: Rfc3339 = "2026-10-15T03:24:42Z".parse().unwrap();
        let (mut store, poisoned) = poisoned_store(161_253_346, observed_at).await;
        let mut report: platpulse_core::AgentReport = serde_json::from_slice(include_bytes!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();

        report.block_summaries = load_block_summaries(&mut store).await.unwrap();
        assert!(report.block_summaries.is_empty());
        assert_eq!(report.validate(), Ok(()));

        report.block_summaries.push(poisoned);
        assert_eq!(
            report.validate(),
            Err(platpulse_core::WireError::ValueOutOfRange {
                field: platpulse_core::envelope::BLOCK_TIMESTAMP_FIELD
            })
        );
        assert_eq!(
            report.validate().unwrap_err().to_string(),
            "field block.block_timestamp_ms is outside its contract range"
        );
    }

    struct MissingResolver;
    impl BlockResolver for MissingResolver {
        fn get_block_by_hash(&self, _hash: &Hash32) -> Result<ResolvedBlock, ResolveError> {
            Err(ResolveError::Rpc)
        }
    }

    #[test]
    fn rpc_boundary_rejects_unsafe_methods_and_invalid_hashes() {
        assert!(method_allowed(
            "eth_getBlockByHash",
            &["eth_getBlockByHash"]
        ));
        assert!(!method_allowed(
            "debug_traceTransaction",
            &["eth_getBlockByHash"]
        ));
        assert_eq!(timestamp_ms("0x1").unwrap(), 1_000);
        // An out-of-contract timestamp is no longer a transport failure: the
        // unit is decided by magnitude and the value is refused once, at the
        // persistence boundary, with an actionable message (issue #243).
        assert_eq!(timestamp_ms("0xffffffffffffffff").unwrap(), u64::MAX);
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
        assert_eq!(
            result.remainder,
            Some((11, 12)),
            "the declared rest of the range stays queued for later steps"
        );

        // Issue #281: a later backlog step backfills the declared rest, and a
        // finished range leaves nothing queued.
        let step = backfill_missing(
            &transport,
            &endpoint,
            "0195f2a1-0014-4014-8014-000000000014".parse().unwrap(),
            identity(),
            at,
            11,
            12,
            backlog_step_bounds(BackfillBounds::default()),
            BackfillTrigger::Backlog,
        );
        assert_eq!(
            step.summaries
                .iter()
                .map(|sample| sample.block_number)
                .collect::<Vec<_>>(),
            vec![11, 12]
        );
        assert!(step.gaps.is_empty());
        assert_eq!(BacklogRange::declared((11, 12)).after_step(&step), None);
    }

    #[test]
    fn a_backlog_range_stays_queued_until_done_or_stalled() {
        let progressed = BackfillOutcome {
            summaries: vec![summary(
                5,
                'a',
                1_000,
                "2026-01-01T00:00:00Z".parse().unwrap(),
            )],
            gaps: vec![],
            remainder: Some((6, 9)),
        };
        let stalled_range = BacklogRange {
            from_height: 5,
            to_height: 9,
            stalls: 3,
        };
        assert_eq!(
            stalled_range.after_step(&progressed),
            Some(BacklogRange {
                from_height: 6,
                to_height: 9,
                stalls: 0
            }),
            "progress resets the stall count"
        );
        let stalled = BackfillOutcome {
            summaries: vec![],
            gaps: vec![],
            remainder: Some((5, 9)),
        };
        assert_eq!(
            BacklogRange::declared((5, 9)).after_step(&stalled),
            Some(BacklogRange {
                from_height: 5,
                to_height: 9,
                stalls: 1
            })
        );
        assert_eq!(
            BacklogRange {
                stalls: BACKFILL_BACKLOG_STALL_LIMIT - 1,
                ..stalled_range
            }
            .after_step(&stalled),
            None,
            "a range that keeps failing is given up; the Server confirms it missing"
        );
        let mismatched = BackfillOutcome {
            summaries: vec![],
            gaps: vec![],
            remainder: None,
        };
        assert_eq!(BacklogRange::declared((5, 9)).after_step(&mismatched), None);
        let bounds = backlog_step_bounds(BackfillBounds {
            max_height_span: 10_000,
            max_block_count: 10_000,
            max_time: Duration::from_secs(60),
        });
        assert_eq!(bounds.max_block_count, 64);
        assert_eq!(bounds.max_time, Duration::from_secs(1));
    }

    #[tokio::test]
    async fn the_backfill_backlog_keeps_the_newest_ranges_and_settles_steps() {
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let node = test_node_id();
        for index in 0..(BACKFILL_BACKLOG_LIMIT as u64 + 2) {
            let from = 100 + index * 10;
            queue_backfill_backlog(
                store.connection(),
                node,
                (from, from + 4),
                "2026-01-01T00:00:00Z",
            )
            .await
            .unwrap();
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backfill_backlog")
            .fetch_one(store.connection())
            .await
            .unwrap();
        assert_eq!(count, BACKFILL_BACKLOG_LIMIT);
        assert_eq!(
            next_backfill_backlog(store.connection(), node)
                .await
                .unwrap(),
            Some(BacklogRange::declared((120, 124))),
            "the two lowest ranges were dropped"
        );
        let remainder = BacklogRange {
            from_height: 123,
            to_height: 124,
            stalls: 2,
        };
        settle_backfill_backlog(
            store.connection(),
            node,
            BacklogRange::declared((120, 124)),
            Some(remainder),
            "2026-01-01T00:00:01Z",
        )
        .await
        .unwrap();
        assert_eq!(
            next_backfill_backlog(store.connection(), node)
                .await
                .unwrap(),
            Some(remainder)
        );
        settle_backfill_backlog(
            store.connection(),
            node,
            remainder,
            None,
            "2026-01-01T00:00:02Z",
        )
        .await
        .unwrap();
        assert_eq!(
            next_backfill_backlog(store.connection(), node)
                .await
                .unwrap(),
            Some(BacklogRange::declared((130, 134)))
        );
    }

    /// Issue #281: a reported gap keeps the reason it was recorded with.
    #[tokio::test]
    async fn a_loaded_history_gap_keeps_its_recorded_reason() {
        let observed_at: Rfc3339 = "2026-10-15T03:24:42Z".parse().unwrap();
        let dir = tempdir().unwrap();
        let mut store = AgentStore::open(AgentDatabaseConfig::new(dir.path().join("agent.db")))
            .await
            .unwrap();
        let mut declared = gap(test_node_id(), 20, 21, observed_at);
        declared.reason = "subscription reconnect exceeded configured backfill bounds".to_owned();
        persist_history_gap(&mut store, &declared).await.unwrap();
        let loaded = load_history_gaps(&mut store, &[test_node_id()])
            .await
            .unwrap();
        assert_eq!(loaded[0].reason, declared.reason);
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
