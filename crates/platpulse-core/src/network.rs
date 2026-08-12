//! Network and endpoint wire types.
//!
//! A PlatON Node belongs to exactly one Network. The Network's *identity* is
//! the observed tuple (genesis hash, chain ID, P2P network ID, optional
//! address HRP); a configured display name or key alone is never identity.
//! A Node has exactly one RPC Endpoint (IPC/WS/WSS) — no failover.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::hex::Hash32;

/// Stable Registry key of a Network, e.g. `platon-mainnet`.
///
/// Lowercase alphanumeric, `_`/`-` allowed after the first character,
/// 1–64 chars. Configured by the Agent; validated by the Server against the
/// Network Registry. An unknown key is rejected, never auto-created.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct NetworkKey(String);

impl NetworkKey {
    /// The registry key string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for NetworkKey {
    type Err = NetworkKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut chars = s.chars();
        let first_ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        if !first_ok {
            return Err(NetworkKeyError::Invalid);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(NetworkKeyError::Invalid);
        }
        if s.len() > 64 {
            return Err(NetworkKeyError::TooLong);
        }
        Ok(Self(s.to_owned()))
    }
}

/// Failure to parse a Network key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkKeyError {
    /// The key does not match `[a-z0-9][a-z0-9_-]{0,63}`.
    Invalid,
    /// The key exceeds 64 chars.
    TooLong,
}

impl fmt::Display for NetworkKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => write!(
                f,
                "network key must be lowercase alphanumeric, optionally with _ or - after the first char"
            ),
            Self::TooLong => write!(f, "network key must be at most 64 chars"),
        }
    }
}

impl std::error::Error for NetworkKeyError {}

impl fmt::Display for NetworkKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for NetworkKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NetworkKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// RPC transport scheme of a Node Endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RpcScheme {
    /// Local IPC socket.
    Ipc,
    /// Unencrypted WebSocket.
    Ws,
    /// TLS WebSocket.
    Wss,
}

/// The single RPC Endpoint of a Node: `ipc://…`, `ws://…`, or `wss://…`.
///
/// Exactly one Endpoint per Node; no transport or URL failover. The URI may
/// contain no userinfo/credentials and no whitespace (contract limit:
/// 2048 chars).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RpcEndpoint(String);

/// Failure to parse an RPC Endpoint URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcEndpointError {
    /// The URI does not start with `ipc://`, `ws://`, or `wss://`.
    InvalidScheme,
    /// The URI has an empty address part.
    EmptyAddress,
    /// The URI contains userinfo (`@`) or whitespace.
    InvalidCharacter,
    /// The URI exceeds 2048 chars.
    TooLong,
}

impl fmt::Display for RpcEndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidScheme => {
                write!(f, "endpoint must start with ipc://, ws://, or wss://")
            }
            Self::EmptyAddress => write!(f, "endpoint address must not be empty"),
            Self::InvalidCharacter => {
                write!(f, "endpoint must not contain userinfo (@) or whitespace")
            }
            Self::TooLong => write!(f, "endpoint must be at most 2048 chars"),
        }
    }
}

impl std::error::Error for RpcEndpointError {}

impl RpcEndpoint {
    /// The transport scheme of this Endpoint.
    pub fn scheme(&self) -> RpcScheme {
        if self.0.starts_with("ipc://") {
            RpcScheme::Ipc
        } else if self.0.starts_with("wss://") {
            RpcScheme::Wss
        } else {
            RpcScheme::Ws
        }
    }

    /// The full endpoint URI as declared by the Agent.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for RpcEndpoint {
    type Err = RpcEndpointError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rest = if let Some(rest) = s.strip_prefix("ipc://") {
            rest
        } else if let Some(rest) = s.strip_prefix("wss://") {
            rest
        } else if let Some(rest) = s.strip_prefix("ws://") {
            rest
        } else {
            return Err(RpcEndpointError::InvalidScheme);
        };
        if rest.is_empty() {
            return Err(RpcEndpointError::EmptyAddress);
        }
        if s.contains('@') || s.chars().any(char::is_whitespace) {
            return Err(RpcEndpointError::InvalidCharacter);
        }
        if s.len() > 2048 {
            return Err(RpcEndpointError::TooLong);
        }
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for RpcEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Endpoints are connection details; Display intentionally matches the
        // wire form (callers redact before logging).
        f.write_str(&self.0)
    }
}

impl Serialize for RpcEndpoint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RpcEndpoint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// The observed identity of a Network, as reported by one Node.
///
/// The Server validates this tuple against its Network Registry; a mismatch
/// keeps current diagnostics flowing but stops block history from merging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkIdentity {
    /// Observed genesis block hash.
    pub genesis_hash: Hash32,
    /// Observed chain ID.
    pub chain_id: u64,
    /// Observed P2P network ID.
    pub p2p_network_id: u64,
    /// Observed bech32 address HRP, e.g. `lat` (contract limit: 16 chars).
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub address_hrp: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_key_validation() {
        assert!("platon-mainnet".parse::<NetworkKey>().is_ok());
        assert!("platon_mainnet_2".parse::<NetworkKey>().is_ok());
        assert!("a".parse::<NetworkKey>().is_ok());
        assert!("Platon-mainnet".parse::<NetworkKey>().is_err());
        assert!("-platon".parse::<NetworkKey>().is_err());
        assert!("platon mainnet".parse::<NetworkKey>().is_err());
        assert!("x".repeat(65).parse::<NetworkKey>().is_err());
    }

    #[test]
    fn endpoint_validation() {
        assert_eq!(
            "ipc:///var/run/platon.ipc"
                .parse::<RpcEndpoint>()
                .unwrap()
                .scheme(),
            RpcScheme::Ipc
        );
        assert_eq!(
            "ws://127.0.0.1:6790"
                .parse::<RpcEndpoint>()
                .unwrap()
                .scheme(),
            RpcScheme::Ws
        );
        assert_eq!(
            "wss://node.example.com"
                .parse::<RpcEndpoint>()
                .unwrap()
                .scheme(),
            RpcScheme::Wss
        );
        assert!("http://127.0.0.1:6790".parse::<RpcEndpoint>().is_err());
        assert!("ws://".parse::<RpcEndpoint>().is_err());
        assert!("ipc://user:pass@/x".parse::<RpcEndpoint>().is_err());
        assert!("ws://127.0.0.1:6790/".parse::<RpcEndpoint>().is_ok());
    }
}
