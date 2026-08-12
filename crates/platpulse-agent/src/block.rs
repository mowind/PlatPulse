//! Per-Node block head subscriptions and hash-based resolution.
//!
//! A subscription is owned by one Node and has its own bounded queue.  The
//! resolver only requests a block by the header hash and receives transaction
//! hashes (never transaction bodies), then verifies all identity fields before
//! producing a `BlockSummary`.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use platpulse_core::block::{
    BlockProductionAttribution, BlockSource, BlockSummary, ProtocolProposer, SealSignerMatch,
};
use platpulse_core::hex::{Address, Hash32};
use platpulse_core::identity::NodeId;
use platpulse_core::network::{NetworkIdentity, RpcEndpoint, RpcScheme};
use platpulse_core::time::Rfc3339;
use serde::Deserialize;
use sqlx::{Connection, FromRow};
use thiserror::Error;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

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

/// A bounded production WebSocket transport. Each invocation opens its own
/// connection and subscription for exactly one Node; no Agent-level socket is
/// shared between Nodes.
#[derive(Debug, Clone, Copy)]
pub struct WebSocketBlockTransport {
    pub receive_timeout: Duration,
    pub max_heads: usize,
    pub max_transactions: usize,
}

impl Default for WebSocketBlockTransport {
    fn default() -> Self {
        Self {
            receive_timeout: Duration::from_millis(250),
            max_heads: 32,
            max_transactions: 100_000,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RpcEnvelope<T> {
    id: u64,
    result: Option<T>,
    error: Option<RpcErrorBody>,
}

#[derive(Debug, Deserialize)]
struct RpcErrorBody {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HeadNotification {
    params: HeadParams,
}

#[derive(Debug, Deserialize)]
struct HeadParams {
    result: HeadWire,
}

#[derive(Debug, Deserialize)]
struct HeadWire {
    number: String,
    hash: String,
    #[serde(rename = "parentHash")]
    parent_hash: String,
    timestamp: String,
    #[serde(default, alias = "author")]
    miner: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BlockWire {
    number: String,
    hash: String,
    #[serde(rename = "parentHash")]
    parent_hash: String,
    timestamp: String,
    transactions: Vec<Hash32>,
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
        .unwrap_or_else(|| "0x0000000000000000000000000000000000000000".to_owned())
        .parse()
        .map_err(|_| TransportError::Failed("invalid RPC coinbase".to_owned()))
}

impl WebSocketBlockTransport {
    async fn connect(
        &self,
        endpoint: &RpcEndpoint,
    ) -> Result<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, TransportError> {
        if !matches!(endpoint.scheme(), RpcScheme::Ws | RpcScheme::Wss) {
            return Err(TransportError::Unavailable);
        }
        connect_async(endpoint.as_str())
            .await
            .map(|(socket, _)| socket)
            .map_err(|error| {
                TransportError::Failed(format!("subscription connection failed: {error}"))
            })
    }

    async fn request<T: for<'de> Deserialize<'de>>(
        &self,
        socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        id: u64,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, TransportError> {
        let request = serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        socket
            .send(Message::Text(request.to_string().into()))
            .await
            .map_err(|error| TransportError::Failed(format!("RPC request failed: {error}")))?;
        loop {
            let message = timeout(self.receive_timeout, socket.next())
                .await
                .map_err(|_| TransportError::Failed("RPC response timed out".to_owned()))?
                .ok_or_else(|| TransportError::Failed("RPC connection closed".to_owned()))
                .and_then(|result| {
                    result.map_err(|error| TransportError::Failed(error.to_string()))
                })?;
            let Message::Text(text) = message else {
                continue;
            };
            let response: RpcEnvelope<T> = serde_json::from_str(text.as_ref())
                .map_err(|_| TransportError::Failed("malformed RPC response".to_owned()))?;
            if response.id != id {
                continue;
            }
            if let Some(error) = response.error {
                return Err(TransportError::Failed(
                    error
                        .message
                        .unwrap_or_else(|| "RPC method failed".to_owned()),
                ));
            }
            return response
                .result
                .ok_or_else(|| TransportError::Failed("RPC response omitted result".to_owned()));
        }
    }

    /// Subscribe to one Node, resolve bounded headers by hash, and close the
    /// socket. Resolve failures do not discard the queued header; callers can
    /// retry that Node without rebuilding the subscription abstraction.
    pub async fn collect_node_summaries(
        &self,
        endpoint: &RpcEndpoint,
        node_id: NodeId,
        identity: NetworkIdentity,
        observed_at: Rfc3339,
    ) -> Result<Vec<BlockSummary>, TransportError> {
        let mut socket = self.connect(endpoint).await?;
        let _subscription: String = self
            .request(
                &mut socket,
                1,
                "eth_subscribe",
                serde_json::json!(["newHeads"]),
            )
            .await?;
        let mut headers = VecDeque::with_capacity(self.max_heads);
        for _ in 0..self.max_heads {
            let message = match timeout(self.receive_timeout, socket.next()).await {
                Ok(Some(Ok(message))) => message,
                _ => break,
            };
            let Message::Text(text) = message else {
                continue;
            };
            let notification: HeadNotification = match serde_json::from_str(text.as_ref()) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let wire = notification.params.result;
            let header = HeadHeader {
                block_number: quantity(&wire.number)?,
                block_hash: parse_hash(&wire.hash)?,
                parent_hash: parse_hash(&wire.parent_hash)?,
                block_timestamp_ms: quantity(&wire.timestamp)?.saturating_mul(1000),
                coinbase: parse_address(wire.miner)?,
            };
            headers.push_back(header);
            if headers.len() >= self.max_heads {
                break;
            }
        }
        let mut summaries = Vec::new();
        let mut next_id = 2;
        while let Some(header) = headers.front().cloned() {
            let hash = format!("{}", header.block_hash);
            let block: BlockWire = match self
                .request(
                    &mut socket,
                    next_id,
                    "platon_getBlockByHash",
                    serde_json::json!([hash, false]),
                )
                .await
            {
                Ok(block) => block,
                Err(error) => return Err(error),
            };
            next_id = next_id.saturating_add(1);
            let resolved = ResolvedBlock {
                block_number: quantity(&block.number)?,
                block_hash: parse_hash(&block.hash)?,
                parent_hash: parse_hash(&block.parent_hash)?,
                block_timestamp_ms: quantity(&block.timestamp)?.saturating_mul(1000),
                transaction_hashes: block
                    .transactions
                    .into_iter()
                    .take(self.max_transactions)
                    .collect(),
                network_identity: identity.clone(),
                coinbase: header.coinbase,
            };
            if resolved.block_number != header.block_number
                || resolved.block_hash != header.block_hash
                || resolved.parent_hash != header.parent_hash
            {
                return Err(TransportError::Resolve(ResolveError::IdentityMismatch));
            }
            headers.pop_front();
            summaries.push(BlockSummary {
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
                    resolved.coinbase,
                    "seal recovery rule is not verified for this fork; protocol proposer evidence is unavailable",
                ),
            });
        }
        Ok(summaries)
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
/// Header delivered by one Node's `newHeads` subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadHeader {
    pub block_number: u64,
    pub block_hash: Hash32,
    pub parent_hash: Hash32,
    pub block_timestamp_ms: u64,
    pub coinbase: Address,
}

/// Bounded result of `platon_getBlockByHash(hash, false)`. The adapter only
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
    sqlx::query("INSERT OR IGNORE INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, block_interval_ms, source, coinbase, seal_signer_key_fingerprint, seal_signer_match, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
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
        "SELECT g.node_id, g.from_height, g.to_height, g.kind, g.created_at FROM history_gaps g WHERE NOT EXISTS (SELECT 1 FROM report_sample_assignments a WHERE a.node_id=g.node_id AND a.sample_kind='gap' AND a.from_height=g.from_height AND a.to_height=g.to_height) ORDER BY g.created_at, g.gap_id",
    )
    .fetch_all(store.connection()).await?;
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
    let rows = sqlx::query_as::<_, BlockRow>("SELECT b.node_id, b.block_number, b.block_hash, b.parent_hash, b.network_genesis_hash, b.network_chain_id, b.network_p2p_network_id, b.network_address_hrp, b.block_timestamp_ms, b.observed_at, b.transaction_count, b.block_interval_ms, b.source, b.coinbase, b.seal_signer_key_fingerprint, b.seal_signer_match, b.protocol_proposer_kind, b.protocol_proposer_identity, b.attribution_reason FROM block_summaries b WHERE NOT EXISTS (SELECT 1 FROM report_sample_assignments a WHERE a.node_id=b.node_id AND a.sample_kind='block' AND a.from_height=b.block_number AND a.to_height=b.block_number) ORDER BY b.created_at, b.sample_id").fetch_all(store.connection()).await?;
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

    struct MissingResolver;
    impl BlockResolver for MissingResolver {
        fn get_block_by_hash(&self, _hash: &Hash32) -> Result<ResolvedBlock, ResolveError> {
            Err(ResolveError::Rpc)
        }
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
