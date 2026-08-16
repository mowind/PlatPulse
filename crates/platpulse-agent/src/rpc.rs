//! Alloy-backed Node RPC capability probe and identity adapter.
//!
//! The production [`AlloyRpcAdapter`] implements the synchronous
//! [`RpcAdapter`] seam by running the Alloy async probe on a short-lived
//! dedicated Tokio runtime, so collector/store/report code never sees Alloy.
//! The probe reads bounded values only: client version, namespaces, capability
//! probe results, the observed Network Identity tuple, sync state, bounded
//! consensus state, and a bounded `admin_peers` DTO. A method that is absent
//! on the Node stays `Unsupported`; an incomplete payload degrades to an
//! explicit error, never to fabricated zero/false/Healthy values.

use std::borrow::Cow;
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use alloy::network::Ethereum;
use alloy::providers::{
    DynProvider, GetSubscription, IpcConnect, Provider, ProviderBuilder, WsConnect,
};
use alloy::transports::{RpcError, TransportError};
use platpulse_core::hex::{FingerprintHex, Hash32};
use platpulse_core::network::{NetworkIdentity, RpcEndpoint, RpcScheme};
use platpulse_core::observation::{ConsensusCurrent, PeerCurrent, PeerDirection, SyncCurrent};
use serde::Deserialize;
use serde_json::{Value, json, value::RawValue};
use thiserror::Error;
use tokio::sync::broadcast::error::RecvError as BroadcastRecvError;
use tokio::time::timeout;

use crate::block::{PlatonBlock, PlatonHead};
use crate::collector::{ProbeValue, RpcAdapter, RpcCollectError, RpcSnapshot};

/// Contract limits for every field the probe accepts from the Node.
pub const MAX_CLIENT_VERSION_CHARS: usize = 256;
pub const MAX_NAMESPACE_CHARS: usize = 64;
pub const MAX_NAMESPACES: usize = 64;
pub const MAX_METHODS: usize = 512;
pub const MAX_METHOD_CHARS: usize = 128;
pub const MAX_ENODE_CHARS: usize = 512;
pub const MAX_PEERS: usize = 1024;
pub const MAX_PEER_ID_CHARS: usize = 128;
pub const MAX_PEER_NAME_CHARS: usize = 256;
pub const MAX_PEER_CAPABILITIES: usize = 64;
pub const MAX_PEER_CAPABILITY_CHARS: usize = 128;
pub const MAX_REMOTE_ADDRESS_CHARS: usize = 64;
/// Maximum accepted probe response size (serialized value length). The raw
/// response is checked before nested JSON/DTO parsing so a broken Node cannot
/// create unbounded peer or validator allocations.
pub const MAX_PROBE_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

fn privileged_rpc_endpoint_allowed(endpoint: &RpcEndpoint) -> bool {
    if endpoint.scheme() == RpcScheme::Ipc {
        return true;
    }
    let Some(rest) = endpoint.as_str().split_once("://").map(|(_, rest)| rest) else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let host = if let Some(host) = authority.strip_prefix('[') {
        host.split(']').next().unwrap_or_default()
    } else {
        authority
            .rsplit_once(':')
            .filter(|(_, port)| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()))
            .map_or(authority, |(host, _)| host)
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|address| match address {
        IpAddr::V4(address) => {
            address.is_loopback() || address.is_private() || address.is_link_local()
        }
        IpAddr::V6(address) => {
            address.is_loopback() || address.is_unique_local() || address.is_unicast_link_local()
        }
    })
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Production Node RPC adapter backed by the pinned Alloy fork. Each
/// `collect` runs on a short-lived dedicated thread with its own Tokio
/// runtime so the synchronous seam also works inside an async runtime
/// without nesting `block_on`.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlloyRpcAdapter;

impl RpcAdapter for AlloyRpcAdapter {
    fn collect(&self, endpoint: &RpcEndpoint) -> Result<RpcSnapshot, RpcCollectError> {
        let endpoint = endpoint.clone();
        std::thread::Builder::new()
            .name("platpulse-rpc-probe".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| {
                        RpcCollectError::Failed(format!("RPC runtime failed: {error}"))
                    })?;
                let result = runtime.block_on(probe_node(&endpoint));
                // Dropping a Tokio runtime waits for every spawned task.
                // Alloy's WS transport task can stay stuck on a half-dead
                // node socket, so bound the shutdown instead of blocking the
                // collector forever.
                runtime.shutdown_timeout(Duration::from_secs(2));
                result
            })
            .map_err(|error| RpcCollectError::Failed(format!("RPC probe thread failed: {error}")))?
            .join()
            .map_err(|_| RpcCollectError::Failed("RPC probe thread panicked".to_owned()))?
    }
}

/// One bounded peer entry extracted from `admin_peers`. Only typed,
/// privacy-safe fields survive; raw protocol JSON, endpoint ports, and
/// non-literal address values are discarded before this DTO reaches the
/// collector.
pub type AdminPeer = PeerCurrent;

#[derive(Debug, Error)]
enum ProbeError {
    #[error("RPC call timed out")]
    Timeout,
    #[error("RPC transport failure: {0}")]
    Transport(#[from] TransportError),
    #[error("malformed RPC payload: {0}")]
    Malformed(String),
}

/// A JSON-RPC error response with code -32601 (method not found), or a
/// message claiming the method is missing, means the capability is absent
/// rather than broken.
fn is_method_missing_code(code: i64, message: &str) -> bool {
    code == -32601 || message.contains("does not exist") || message.contains("is not available")
}

fn is_method_missing(error: &TransportError) -> bool {
    match error {
        RpcError::ErrorResp(payload) => is_method_missing_code(payload.code, &payload.message),
        _ => false,
    }
}

/// Connect to one Node Endpoint through Alloy. WS and WSS share the WS
/// connector; IPC strips the `ipc://` prefix and connects the local socket.
async fn connect(endpoint: &RpcEndpoint) -> Result<DynProvider<Ethereum>, TransportError> {
    match endpoint.scheme() {
        RpcScheme::Ws | RpcScheme::Wss => ProviderBuilder::new()
            .connect_ws(WsConnect::new(endpoint.as_str()))
            .await
            .map(DynProvider::new),
        RpcScheme::Ipc => {
            let path = endpoint
                .as_str()
                .strip_prefix("ipc://")
                .unwrap_or(endpoint.as_str());
            ProviderBuilder::new()
                .connect_ipc(IpcConnect::new(path.to_owned()))
                .await
                .map(DynProvider::new)
        }
    }
}

/// One bounded raw JSON-RPC call through the Alloy provider. The decoded
/// value is size-checked before any DTO parsing so a hostile or broken Node
/// cannot force an unbounded allocation.
async fn call(
    provider: &DynProvider<Ethereum>,
    method: &'static str,
    params: Value,
) -> Result<Value, ProbeError> {
    let raw: Box<RawValue> = timeout(
        CALL_TIMEOUT,
        provider.raw_request::<_, Box<RawValue>>(Cow::Borrowed(method), params),
    )
    .await
    .map_err(|_| ProbeError::Timeout)?
    .map_err(ProbeError::from)?;
    if raw.get().len() > MAX_PROBE_RESPONSE_BYTES {
        return Err(ProbeError::Malformed(format!(
            "{method} response exceeded the {MAX_PROBE_RESPONSE_BYTES}-byte bound"
        )));
    }
    serde_json::from_str(raw.get()).map_err(|error| {
        ProbeError::Malformed(format!("{method} response was invalid JSON: {error}"))
    })
}

fn record_method(methods: &mut Vec<String>, method: &'static str) {
    if methods.len() < MAX_METHODS {
        methods.push(method.to_owned());
    }
}

fn truncate_chars(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

/// Parse a JSON-RPC hex quantity string value (`"0x1a"`).
fn parse_quantity_string(value: &Value) -> Result<u64, String> {
    let text = value
        .as_str()
        .ok_or("expected a hex quantity string".to_owned())?;
    let digits = text
        .strip_prefix("0x")
        .ok_or_else(|| "quantity must be 0x-prefixed".to_owned())?;
    u64::from_str_radix(digits, 16).map_err(|_| "invalid quantity".to_owned())
}

/// Parse a JSON number or hex/decimal string into `u64`.
fn parse_u64_value(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => {
            if let Some(hex) = text.strip_prefix("0x") {
                u64::from_str_radix(hex, 16).ok()
            } else {
                text.parse().ok()
            }
        }
        _ => None,
    }
}

/// Bounded namespace list from `rpc_modules` (sorted, deduplicated, capped).
fn parse_rpc_modules(modules: serde_json::Map<String, Value>) -> Vec<String> {
    let mut namespaces: Vec<String> = modules
        .keys()
        .map(|key| truncate_chars(key.clone(), MAX_NAMESPACE_CHARS))
        .collect();
    namespaces.sort();
    namespaces.dedup();
    namespaces.truncate(MAX_NAMESPACES);
    namespaces
}

/// P2P network ID from `admin_nodeInfo.protocols.platon.network`.
fn node_info_p2p_network_id(node_info: &Value) -> Result<u64, String> {
    let protocols = node_info
        .get("protocols")
        .and_then(Value::as_object)
        .ok_or_else(|| "protocols field missing".to_owned())?;
    let platon = protocols
        .get("platon")
        .ok_or_else(|| "platon protocol missing".to_owned())?;
    parse_u64_value(
        platon
            .get("network")
            .ok_or_else(|| "network id missing".to_owned())?,
    )
    .ok_or_else(|| "network id invalid".to_owned())
}

/// Bech32 address HRP (e.g. `lat`) from
/// `admin_nodeInfo.protocols.platon.config.addressHRP`.
fn node_info_address_hrp(node_info: &Value) -> Option<String> {
    node_info
        .get("protocols")?
        .get("platon")?
        .get("config")?
        .get("addressHRP")?
        .as_str()
        .map(|value| truncate_chars(value.to_owned(), 16))
}

/// 20-byte fingerprint of the P2P Node key, derived as keccak256(pubkey)[12..]
/// following the Ethereum address convention. The Node key is the 64-byte
/// secp256k1 public key embedded in `admin_nodeInfo.enode`.
fn node_info_key_fingerprint(node_info: &Value) -> Result<FingerprintHex, String> {
    let enode = node_info
        .get("enode")
        .and_then(Value::as_str)
        .ok_or_else(|| "enode missing".to_owned())?;
    let pubkey_hex = enode
        .strip_prefix("enode://")
        .and_then(|rest| rest.split('@').next())
        .ok_or_else(|| "enode is malformed".to_owned())?;
    let digits = pubkey_hex.strip_prefix("0x").unwrap_or(pubkey_hex);
    if digits.len() != 128 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("enode pubkey must be 64 bytes of hex".to_owned());
    }
    let bytes = hex::decode(digits).map_err(|_| "enode pubkey is not valid hex".to_owned())?;
    let digest = alloy::primitives::keccak256(&bytes);
    let encoded = format!("0x{}", hex::encode(&digest[12..]));
    encoded
        .parse()
        .map_err(|_| "node key fingerprint encoding failed".to_owned())
}

/// Bounded `SyncCurrent` from an `eth_syncing` payload. `false` is the
/// authoritative "not syncing" answer: the head is the highest known block.
/// PlatON 1.5.1 reports `syncedAccounts`/`syncedStorage` instead of geth's
/// `pulledStates`/`knownStates`; when only the PlatON counters are present
/// they fill the progress slots, and a payload with neither pair is an
/// explicit incomplete error (never fabricated zeros).
fn parse_syncing(value: Value) -> Result<SyncCurrent, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "eth_syncing payload is not an object".to_owned())?;
    let current_block = parse_u64_value(
        object
            .get("currentBlock")
            .ok_or_else(|| "currentBlock missing".to_owned())?,
    )
    .ok_or_else(|| "currentBlock invalid".to_owned())?;
    let highest_block = parse_u64_value(
        object
            .get("highestBlock")
            .ok_or_else(|| "highestBlock missing".to_owned())?,
    )
    .ok_or_else(|| "highestBlock invalid".to_owned())?;
    let pulled_states = object
        .get("pulledStates")
        .or_else(|| object.get("syncedAccounts"))
        .and_then(parse_u64_value)
        .ok_or_else(|| "pulledStates/syncedAccounts missing".to_owned())?;
    let known_states = object
        .get("knownStates")
        .or_else(|| object.get("syncedStorage"))
        .and_then(parse_u64_value)
        .ok_or_else(|| "knownStates/syncedStorage missing".to_owned())?;
    Ok(SyncCurrent {
        syncing: true,
        current_block,
        highest_block,
        pulled_states,
        known_states,
    })
}

/// Local wire form of `debug_consensusStatus`. Every field is optional so a
/// partial payload degrades to an explicit error instead of zero-filling.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConsensusStatusWire {
    #[serde(default)]
    validator: Option<bool>,
    #[serde(default)]
    state: Option<ViewStateWire>,
}

#[derive(Debug, Deserialize)]
struct ViewStateWire {
    #[serde(default, rename = "view")]
    view: Option<ViewWire>,
    #[serde(default, rename = "highestQCBlock")]
    highest_qc_block: Option<HashNumberWire>,
    #[serde(default, rename = "highestLockBlock")]
    highest_lock_block: Option<HashNumberWire>,
    #[serde(default, rename = "highestCommitBlock")]
    highest_commit_block: Option<HashNumberWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewWire {
    #[serde(default)]
    epoch: Option<u64>,
    #[serde(default)]
    view_number: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HashNumberWire {
    #[serde(default)]
    number: Option<u64>,
}

/// Map a bounded `debug_consensusStatus` payload to `ConsensusCurrent`.
/// Missing epoch/view/highest-block fields are an incomplete observation and
/// never become zero.
fn parse_consensus_status(value: Value) -> Result<ConsensusCurrent, String> {
    let wire: ConsensusStatusWire = serde_json::from_value(value)
        .map_err(|error| format!("malformed debug_consensusStatus payload: {error}"))?;
    let validator = wire
        .validator
        .ok_or_else(|| "validator field missing".to_owned())?;
    let state = wire.state.ok_or_else(|| "state field missing".to_owned())?;
    let view = state.view.ok_or_else(|| "state.view missing".to_owned())?;
    let epoch = view.epoch.ok_or_else(|| "view.epoch missing".to_owned())?;
    let view_number = view
        .view_number
        .ok_or_else(|| "view.viewNumber missing".to_owned())?;
    let highest_qc_block = state
        .highest_qc_block
        .and_then(|block| block.number)
        .ok_or_else(|| "highestQCBlock missing".to_owned())?;
    let highest_lock_block = state
        .highest_lock_block
        .and_then(|block| block.number)
        .ok_or_else(|| "highestLockBlock missing".to_owned())?;
    let highest_commit_block = state
        .highest_commit_block
        .and_then(|block| block.number)
        .ok_or_else(|| "highestCommitBlock missing".to_owned())?;
    Ok(ConsensusCurrent {
        epoch,
        view_number,
        validator,
        highest_qc_block,
        highest_lock_block,
        highest_commit_block,
    })
}

/// Parse a bounded `admin_peers` array. A malformed entry rejects the whole
/// snapshot so missing flags cannot be fabricated as false; oversized or
/// structurally unusable snapshots fail closed. Only literal IPs and typed
/// CBFT fields survive this boundary.
pub fn parse_admin_peers(value: Value) -> Result<Vec<AdminPeer>, String> {
    let peers = value
        .as_array()
        .ok_or_else(|| "admin_peers response is not an array".to_owned())?;
    if peers.len() > MAX_PEERS {
        return Err(format!("admin_peers exceeded the {MAX_PEERS}-peer bound"));
    }
    let mut bounded = Vec::with_capacity(peers.len());
    let mut peer_ids = HashSet::with_capacity(peers.len());
    for peer in peers {
        let peer = peer
            .as_object()
            .ok_or_else(|| "admin_peers peer entry is not an object".to_owned())?;
        let id = peer
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "admin_peers peer id is missing or not a string".to_owned())?;
        if id.is_empty() || id.chars().count() > MAX_PEER_ID_CHARS {
            return Err(format!(
                "admin_peers Peer ID exceeded the {MAX_PEER_ID_CHARS}-character bound"
            ));
        }
        if !peer_ids.insert(id) {
            return Err(format!("admin_peers repeated Peer ID {id}"));
        }

        let network = peer
            .get("network")
            .and_then(Value::as_object)
            .ok_or_else(|| "admin_peers network object is missing".to_owned())?;
        let inbound = required_peer_bool(network, "inbound")?;
        let trusted = required_peer_bool(network, "trusted")?;
        let static_peer = required_peer_bool(network, "static")?;
        let consensus_peer = required_peer_bool(network, "consensus")?;
        let remote_ip = match network.get("remoteAddress") {
            None => None,
            Some(value) => {
                let value = value
                    .as_str()
                    .ok_or_else(|| "admin_peers remoteAddress is not a string".to_owned())?;
                if value.chars().count() > MAX_REMOTE_ADDRESS_CHARS {
                    return Err(format!(
                        "admin_peers remoteAddress exceeded the {MAX_REMOTE_ADDRESS_CHARS}-character bound"
                    ));
                }
                Some(parse_literal_remote_ip(value).ok_or_else(|| {
                    "admin_peers remoteAddress is not a literal IP address".to_owned()
                })?)
            }
        };
        let client_name = match peer.get("name") {
            None => None,
            Some(value) => {
                let value = value
                    .as_str()
                    .ok_or_else(|| "admin_peers name is not a string".to_owned())?;
                if value.is_empty() {
                    return Err("admin_peers name must not be empty".to_owned());
                }
                if value.chars().count() > MAX_PEER_NAME_CHARS {
                    return Err(format!(
                        "admin_peers name exceeded the {MAX_PEER_NAME_CHARS}-character bound"
                    ));
                }
                Some(value.to_owned())
            }
        };
        let caps = parse_peer_caps(peer.get("caps"))?;
        let (cbft_protocol_version, cbft_highest_qc_block, cbft_locked_block, cbft_commit_block) =
            parse_cbft_fields(peer.get("protocols"))?;

        bounded.push(PeerCurrent {
            peer_id: id.to_owned(),
            remote_ip,
            direction: if inbound {
                PeerDirection::Inbound
            } else {
                PeerDirection::Outbound
            },
            trusted,
            static_peer,
            consensus_peer,
            client_name,
            caps,
            cbft_protocol_version,
            cbft_highest_qc_block,
            cbft_locked_block,
            cbft_commit_block,
        });
    }
    Ok(bounded)
}

fn required_peer_bool(
    network: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<bool, String> {
    network
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("admin_peers network.{field} is not a boolean"))
}

fn parse_peer_caps(value: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let caps = value
        .as_array()
        .ok_or_else(|| "admin_peers caps is not an array".to_owned())?;
    if caps.len() > MAX_PEER_CAPABILITIES {
        return Err(format!(
            "admin_peers exceeded the {MAX_PEER_CAPABILITIES}-capability bound"
        ));
    }
    let mut seen = HashSet::with_capacity(caps.len());
    let mut bounded = Vec::with_capacity(caps.len());
    for cap in caps {
        let cap = cap
            .as_str()
            .ok_or_else(|| "admin_peers capability is not a string".to_owned())?;
        if cap.is_empty() {
            return Err("admin_peers capability must not be empty".to_owned());
        }
        if cap.chars().count() > MAX_PEER_CAPABILITY_CHARS {
            return Err(format!(
                "admin_peers capability exceeded the {MAX_PEER_CAPABILITY_CHARS}-character bound"
            ));
        }
        if !seen.insert(cap) {
            return Err("admin_peers repeated capability".to_owned());
        }
        bounded.push(cap.to_owned());
    }
    Ok(bounded)
}

type CbftFields = (Option<u64>, Option<u64>, Option<u64>, Option<u64>);

fn parse_cbft_fields(value: Option<&Value>) -> Result<CbftFields, String> {
    let Some(value) = value else {
        return Ok((None, None, None, None));
    };
    let protocols = value
        .as_object()
        .ok_or_else(|| "admin_peers protocols is not an object".to_owned())?;
    let Some(value) = protocols.get("cbft") else {
        return Ok((None, None, None, None));
    };
    let cbft = value
        .as_object()
        .ok_or_else(|| "admin_peers protocols.cbft is not an object".to_owned())?;
    let parse = |name: &str| -> Result<Option<u64>, String> {
        let Some(value) = cbft.get(name) else {
            return Ok(None);
        };
        let parsed = parse_u64_value(value)
            .ok_or_else(|| format!("admin_peers CBFT field {name} is not a u64"))?;
        if name == "protocolVersion"
            && parsed > platpulse_core::protocol::MAX_PEER_CBFT_PROTOCOL_VERSION
        {
            return Err(format!(
                "admin_peers CBFT protocol version {parsed} is out of bounds"
            ));
        }
        if name != "protocolVersion" && parsed > platpulse_core::protocol::MAX_PEER_CBFT_BLOCK {
            return Err(format!(
                "admin_peers CBFT block field {name} is out of bounds"
            ));
        }
        Ok(Some(parsed))
    };
    Ok((
        parse("protocolVersion")?,
        parse("highestQCBn")?,
        parse("lockedBn")?,
        parse("commitBn")?,
    ))
}

fn parse_literal_remote_ip(value: &str) -> Option<String> {
    if let Ok(address) = value.parse::<SocketAddr>() {
        return Some(address.ip().to_string());
    }
    value
        .parse::<IpAddr>()
        .ok()
        .map(|address| address.to_string())
}

/// Verify that `eth_subscribe("newHeads")` is available. An idle chain may
/// not emit a head within the window; the established subscription itself is
/// the capability evidence.
async fn probe_subscription(provider: &DynProvider<Ethereum>) -> Result<(), ProbeError> {
    let mut call = provider.client().request("eth_subscribe", ("newHeads",));
    call.set_is_subscription();
    let subscription =
        GetSubscription::<_, PlatonHead>::new(provider.weak_client(), call).channel_size(1);
    let mut subscription = timeout(CALL_TIMEOUT, subscription)
        .await
        .map_err(|_| ProbeError::Timeout)?
        .map_err(ProbeError::from)?;
    match timeout(CALL_TIMEOUT, subscription.recv_result()).await {
        Ok(Ok(Err(error))) => Err(ProbeError::Malformed(format!(
            "malformed head notification: {error}"
        ))),
        Ok(Err(BroadcastRecvError::Closed)) => Err(ProbeError::Malformed(
            "head notification stream closed".to_owned(),
        )),
        Ok(_) | Err(_) => Ok(()),
    }
}

/// One bounded capability/identity probe of a single Node.
async fn probe_node(endpoint: &RpcEndpoint) -> Result<RpcSnapshot, RpcCollectError> {
    timeout(PROBE_TIMEOUT, probe_node_inner(endpoint))
        .await
        .map_err(|_| RpcCollectError::Failed("RPC capability probe timed out".to_owned()))?
}

async fn probe_node_inner(endpoint: &RpcEndpoint) -> Result<RpcSnapshot, RpcCollectError> {
    if !privileged_rpc_endpoint_allowed(endpoint) {
        return Err(RpcCollectError::Failed(
            "privileged RPC probe requires IPC or a loopback/private endpoint".to_owned(),
        ));
    }
    let provider = timeout(CONNECT_TIMEOUT, connect(endpoint))
        .await
        .map_err(|_| RpcCollectError::Failed("RPC connection timed out".to_owned()))?
        .map_err(|error| RpcCollectError::Failed(format!("RPC connection failed: {error}")))?;

    let mut methods: Vec<String> = Vec::new();

    // Client version is the RPC component's identity; an unusable answer
    // fails the whole probe rather than persisting a partial snapshot.
    let client_version = match call(&provider, "web3_clientVersion", json!([])).await {
        Ok(Value::String(value)) if !value.is_empty() => {
            record_method(&mut methods, "web3_clientVersion");
            truncate_chars(value, MAX_CLIENT_VERSION_CHARS)
        }
        Ok(_) => {
            return Err(RpcCollectError::Failed(
                "web3_clientVersion returned no usable value".to_owned(),
            ));
        }
        Err(ProbeError::Transport(error)) if is_method_missing(&error) => {
            return Err(RpcCollectError::Failed(
                "web3_clientVersion is not available on this node".to_owned(),
            ));
        }
        Err(error) => {
            return Err(RpcCollectError::Failed(format!(
                "web3_clientVersion probe failed: {error}"
            )));
        }
    };

    let namespaces = match call(&provider, "rpc_modules", json!([])).await {
        Ok(Value::Object(modules)) => {
            record_method(&mut methods, "rpc_modules");
            parse_rpc_modules(modules)
        }
        _ => Vec::new(),
    };

    let head = match call(&provider, "eth_blockNumber", json!([])).await {
        Ok(value) => {
            record_method(&mut methods, "eth_blockNumber");
            parse_quantity_string(&value).map_err(|error| {
                RpcCollectError::Failed(format!("eth_blockNumber probe failed: {error}"))
            })?
        }
        Err(error) => {
            return Err(RpcCollectError::Failed(format!(
                "eth_blockNumber probe failed: {error}"
            )));
        }
    };

    // Genesis hash comes from the authoritative block 0 through the standard
    // eth_getBlockByNumber interface.
    let genesis_hash: Hash32 = match call(&provider, "eth_getBlockByNumber", json!(["0x0", false]))
        .await
    {
        Ok(value) => {
            record_method(&mut methods, "eth_getBlockByNumber");
            let genesis: PlatonBlock = serde_json::from_value(value).map_err(|error| {
                RpcCollectError::Failed(format!("genesis block response was malformed: {error}"))
            })?;
            genesis
                .hash
                .parse()
                .map_err(|_| RpcCollectError::Failed("genesis block hash was invalid".to_owned()))?
        }
        Err(error) => {
            return Err(RpcCollectError::Failed(format!(
                "eth_getBlockByNumber probe failed: {error}"
            )));
        }
    };

    let chain_id = match call(&provider, "eth_chainId", json!([])).await {
        Ok(value) => {
            record_method(&mut methods, "eth_chainId");
            parse_quantity_string(&value).map_err(|error| {
                RpcCollectError::Failed(format!("eth_chainId probe failed: {error}"))
            })?
        }
        Err(error) => {
            return Err(RpcCollectError::Failed(format!(
                "eth_chainId probe failed: {error}"
            )));
        }
    };

    let node_info = match call(&provider, "admin_nodeInfo", json!([])).await {
        Ok(value) => {
            record_method(&mut methods, "admin_nodeInfo");
            value
        }
        Err(ProbeError::Transport(error)) if is_method_missing(&error) => {
            return Err(RpcCollectError::Failed(
                "admin_nodeInfo is not available; network identity cannot be observed".to_owned(),
            ));
        }
        Err(error) => {
            return Err(RpcCollectError::Failed(format!(
                "admin_nodeInfo probe failed: {error}"
            )));
        }
    };
    let p2p_network_id = node_info_p2p_network_id(&node_info).map_err(|error| {
        RpcCollectError::Failed(format!("admin_nodeInfo is incomplete: {error}"))
    })?;
    let node_key_fingerprint = node_info_key_fingerprint(&node_info).map_err(|error| {
        RpcCollectError::Failed(format!("admin_nodeInfo is incomplete: {error}"))
    })?;
    let address_hrp = node_info_address_hrp(&node_info);
    let enode = node_info
        .get("enode")
        .and_then(Value::as_str)
        .map(|value| truncate_chars(value.to_owned(), MAX_ENODE_CHARS));

    let sync = match call(&provider, "eth_syncing", json!([])).await {
        Ok(Value::Bool(false)) => {
            record_method(&mut methods, "eth_syncing");
            ProbeValue::Supported(SyncCurrent {
                syncing: false,
                current_block: head,
                highest_block: head,
                pulled_states: 0,
                known_states: 0,
            })
        }
        Ok(value @ Value::Object(_)) => {
            record_method(&mut methods, "eth_syncing");
            match parse_syncing(value) {
                Ok(current) => ProbeValue::Supported(current),
                Err(error) => ProbeValue::Error(format!("eth_syncing payload incomplete: {error}")),
            }
        }
        Ok(_) => ProbeValue::Error("eth_syncing returned an unusable payload".to_owned()),
        Err(ProbeError::Transport(error)) if is_method_missing(&error) => ProbeValue::Unsupported,
        Err(error) => ProbeValue::Error(format!("eth_syncing failed: {error}")),
    };

    let consensus = match call(&provider, "debug_consensusStatus", json!([])).await {
        Ok(value) => {
            record_method(&mut methods, "debug_consensusStatus");
            match parse_consensus_status(value) {
                Ok(current) => ProbeValue::Supported(current),
                Err(error) => {
                    ProbeValue::Error(format!("debug_consensusStatus payload incomplete: {error}"))
                }
            }
        }
        Err(ProbeError::Transport(error)) if is_method_missing(&error) => ProbeValue::Unsupported,
        Err(error) => ProbeValue::Error(format!("debug_consensusStatus failed: {error}")),
    };

    let peers = match call(&provider, "admin_peers", json!([])).await {
        Ok(value) => {
            record_method(&mut methods, "admin_peers");
            match parse_admin_peers(value) {
                Ok(peers) => {
                    ProbeValue::Supported(platpulse_core::observation::PeerSnapshot { peers })
                }
                Err(error) => ProbeValue::Error(format!(
                    "admin_peers payload failed bounded validation: {error}"
                )),
            }
        }
        Err(ProbeError::Transport(error)) if is_method_missing(&error) => ProbeValue::Unsupported,
        Err(error) => ProbeValue::Error(format!("admin_peers probe failed: {error}")),
    };

    if probe_subscription(&provider).await.is_ok() {
        record_method(&mut methods, "eth_subscribe");
    }

    Ok(RpcSnapshot {
        client_version,
        namespaces,
        methods,
        network_identity: NetworkIdentity {
            genesis_hash,
            chain_id,
            p2p_network_id,
            address_hrp,
        },
        node_key_fingerprint,
        enode,
        sync,
        consensus,
        peers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privileged_rpc_probe_rejects_public_endpoints() {
        for endpoint in [
            "ipc:///data/platon.ipc",
            "ws://127.0.0.1:6790",
            "wss://[fd00::1]:6790",
            "ws://10.0.0.4:6790",
            "ws://localhost:6790",
        ] {
            assert!(privileged_rpc_endpoint_allowed(&endpoint.parse().unwrap()));
        }
        for endpoint in ["ws://8.8.8.8:6790", "wss://node.example.com:6790"] {
            assert!(!privileged_rpc_endpoint_allowed(&endpoint.parse().unwrap()));
        }
    }

    #[test]
    fn quantity_strings_are_bounded() {
        assert_eq!(parse_quantity_string(&json!("0x10")).unwrap(), 16);
        assert!(parse_quantity_string(&json!("16")).is_err());
        assert!(parse_quantity_string(&json!("0x")).is_err());
        assert!(parse_quantity_string(&json!(16)).is_err());
    }

    #[test]
    fn u64_values_accept_numbers_and_quantities() {
        assert_eq!(parse_u64_value(&json!(210425)), Some(210425));
        assert_eq!(parse_u64_value(&json!("210425")), Some(210425));
        assert_eq!(parse_u64_value(&json!("0x335a4")), Some(210_340));
        assert_eq!(parse_u64_value(&json!("nope")), None);
        assert_eq!(parse_u64_value(&json!(true)), None);
    }

    #[test]
    fn rpc_modules_are_sorted_bounded_and_deduplicated() {
        let modules = serde_json::Map::from_iter([
            ("net".to_owned(), json!("1.0")),
            ("platon".to_owned(), json!("1.0")),
            ("web3".to_owned(), json!("1.0")),
            ("admin".to_owned(), json!("1.0")),
        ]);
        let namespaces = parse_rpc_modules(modules);
        assert_eq!(namespaces, ["admin", "net", "platon", "web3"]);

        let oversized = serde_json::Map::from_iter(
            (0..MAX_NAMESPACES + 10).map(|index| (format!("ns-{index:03}"), json!("1.0"))),
        );
        assert_eq!(parse_rpc_modules(oversized).len(), MAX_NAMESPACES);
    }

    #[test]
    fn node_info_identity_fields_are_bounded() {
        let pubkey = "ab".repeat(64);
        let node_info = json!({
            "enode": format!("enode://{pubkey}@127.0.0.1:16789"),
            "protocols": {
                "platon": {
                    "network": 210425,
                    "genesis": "0xgenesis",
                    "config": {"addressHRP": "lat"}
                }
            }
        });
        assert_eq!(node_info_p2p_network_id(&node_info).unwrap(), 210425);
        assert_eq!(node_info_address_hrp(&node_info).as_deref(), Some("lat"));
        let fingerprint = node_info_key_fingerprint(&node_info).unwrap();
        assert_eq!(fingerprint.as_str().len(), 42);
        assert!(node_info_key_fingerprint(&json!({})).is_err());
        assert!(node_info_key_fingerprint(&json!({"enode": "enode://short@h"})).is_err());
        assert!(node_info_key_fingerprint(&json!({"enode": "http://nope@h"})).is_err());
        assert!(node_info_p2p_network_id(&json!({})).is_err());
        assert!(node_info_p2p_network_id(&json!({"protocols": {}})).is_err());
        assert!(node_info_p2p_network_id(&json!({"protocols": {"platon": {}}})).is_err());
        assert_eq!(
            node_info_address_hrp(&json!({"protocols": {"platon": {}}})),
            None
        );
    }

    #[test]
    fn syncing_false_is_authoritative_head_and_payload_fields_are_required() {
        let parsed = parse_syncing(json!({
            "currentBlock": "0x100",
            "highestBlock": "0x200",
            "pulledStates": 5,
            "knownStates": "0x10"
        }))
        .unwrap();
        assert!(parsed.syncing);
        assert_eq!(parsed.current_block, 0x100);
        assert_eq!(parsed.highest_block, 0x200);
        assert_eq!(parsed.pulled_states, 5);
        assert_eq!(parsed.known_states, 0x10);

        // PlatON 1.5.1 reports syncedAccounts/syncedStorage instead of
        // pulledStates/knownStates; both counters must stay real.
        let platon = parse_syncing(json!({
            "currentBlock": "0x100",
            "highestBlock": "0x200",
            "syncedAccounts": "0x10",
            "syncedStorage": "0x20"
        }))
        .unwrap();
        assert_eq!(platon.pulled_states, 0x10);
        assert_eq!(platon.known_states, 0x20);

        assert!(parse_syncing(json!({"currentBlock": "0x1"})).is_err());
        assert!(
            parse_syncing(json!({
                "currentBlock": "0x1",
                "highestBlock": "0x2"
            }))
            .is_err()
        );
    }

    #[test]
    fn consensus_status_maps_complete_payload_and_rejects_incomplete() {
        let complete = json!({
            "validator": true,
            "state": {
                "view": {"epoch": 7, "viewNumber": 3},
                "highestQCBlock": {"hash": "0xaa", "number": 100},
                "highestLockBlock": {"hash": "0xbb", "number": 99},
                "highestCommitBlock": {"hash": "0xcc", "number": 98}
            }
        });
        let parsed = parse_consensus_status(complete).unwrap();
        assert_eq!(parsed.epoch, 7);
        assert_eq!(parsed.view_number, 3);
        assert!(parsed.validator);
        assert_eq!(parsed.highest_qc_block, 100);
        assert_eq!(parsed.highest_lock_block, 99);
        assert_eq!(parsed.highest_commit_block, 98);

        for partial in [
            json!({"state": {"view": {"epoch": 1, "viewNumber": 1}}}),
            json!({"validator": true, "state": {}}),
            json!({"validator": true, "state": {"view": {"epoch": 1}}}),
            json!({"validator": true, "state": {"view": {"epoch": 1, "viewNumber": 1}, "highestQCBlock": {"number": 1}}}),
            json!({}),
        ] {
            assert!(
                parse_consensus_status(partial).is_err(),
                "partial payload must not fabricate zeros"
            );
        }
    }

    #[test]
    fn admin_peers_dto_is_bounded_and_rejects_malformed_peers() {
        let value = json!([
            {"id": "a".repeat(64), "name": "peer-a", "network": {"remoteAddress": "8.8.8.8:16789", "inbound": false, "trusted": false, "static": false, "consensus": false}, "caps": []},
            {"id": "b".repeat(64), "name": "peer-b", "network": {"inbound": true, "trusted": false, "static": false, "consensus": true}, "caps": []},
            {"id": "c".repeat(64), "name": "peer-c", "network": {"inbound": false, "trusted": true, "static": false, "consensus": false}, "caps": []}
        ]);
        let peers = parse_admin_peers(value).unwrap();
        assert_eq!(peers.len(), 3);
        assert_eq!(peers[0].remote_ip.as_deref(), Some("8.8.8.8"));
        assert_eq!(peers[0].direction, PeerDirection::Outbound);
        assert_eq!(peers[1].remote_ip, None);
        assert_eq!(peers[2].remote_ip, None);
        assert!(parse_admin_peers(json!([{"id": "missing-network"}])).is_err());
        assert!(
            parse_admin_peers(json!([{
                "id": "bad-flags",
                "network": {"inbound": "yes", "trusted": false, "static": false, "consensus": false}
            }]))
            .is_err()
        );

        let oversized = json!(vec![
            json!({"id": "x", "name": "", "network": {}});
            MAX_PEERS + 1
        ]);
        assert!(parse_admin_peers(oversized).is_err());
        assert!(parse_admin_peers(json!({"not": "an array"})).is_err());
    }

    #[test]
    fn admin_peers_parses_typed_flags_capabilities_and_cbft_without_raw_json() {
        let peers = parse_admin_peers(json!([{
            "id": "peer-a",
            "name": "PlatON/v1.5.1",
            "network": {
                "remoteAddress": "[2001:db8::4]:16789",
                "inbound": true,
                "trusted": true,
                "static": true,
                "consensus": false
            },
            "caps": ["cbft/1", "platon/1"],
            "protocols": {
                "cbft": {
                    "protocolVersion": "0x1",
                    "highestQCBn": "0x64",
                    "lockedBn": 99,
                    "commitBn": "98"
                },
                "secret": {"raw": "discarded"}
            }
        }]))
        .unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].peer_id, "peer-a");
        assert_eq!(peers[0].remote_ip.as_deref(), Some("2001:db8::4"));
        assert_eq!(peers[0].direction, PeerDirection::Inbound);
        assert!(peers[0].trusted);
        assert!(peers[0].static_peer);
        assert!(!peers[0].consensus_peer);
        assert_eq!(peers[0].caps, ["cbft/1", "platon/1"]);
        assert_eq!(peers[0].cbft_protocol_version, Some(1));
        assert_eq!(peers[0].cbft_highest_qc_block, Some(100));
        assert_eq!(peers[0].cbft_locked_block, Some(99));
        assert_eq!(peers[0].cbft_commit_block, Some(98));

        assert!(parse_admin_peers(json!([{"id":"peer-a", "caps": [true]}])).is_err());
        assert!(parse_admin_peers(json!([{
            "id": "peer-a",
            "network": {"inbound": false, "trusted": false, "static": false, "consensus": false},
            "caps": ["cbft/1", "cbft/1"]
        }]))
        .is_err());
        assert!(
            parse_admin_peers(json!([
                {"id":"peer-a"}, {"id":"peer-a"}
            ]))
            .is_err()
        );
    }

    #[test]
    fn method_missing_detection_recognizes_jsonrpc_32601() {
        assert!(is_method_missing_code(
            -32601,
            "the method debug_consensusStatus does not exist/is not available"
        ));
        assert!(is_method_missing_code(
            -1,
            "the method debug_consensusStatus does not exist/is not available"
        ));
        assert!(is_method_missing_code(
            -1,
            "method admin_peers is not available"
        ));
        assert!(!is_method_missing_code(-32000, "boom"));
        assert!(!is_method_missing_code(-32001, "some other error"));
    }
}
