//! Agent configuration (design §8.2).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use platpulse_core::identity::NodeId;
use platpulse_core::inventory::{InventoryNode, NodeInventory, ProcessSelector};
use platpulse_core::network::{NetworkKey, RpcEndpoint};
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigFile {
    pub server_url: String,
    pub credential_file: PathBuf,
    pub state_db: PathBuf,
    #[serde(default = "default_collection_interval_seconds")]
    pub collection_interval_seconds: u64,
    #[serde(default = "default_inventory_revision")]
    pub inventory_revision: u64,
    /// Conservative bounded recovery point-query policy. These defaults are
    /// intentionally finite: realtime collection never falls back to polling.
    #[serde(default)]
    pub backfill: BackfillConfig,
    #[serde(default)]
    pub nodes: Vec<AgentNodeConfig>,
}

pub const MIN_COLLECTION_INTERVAL_SECONDS: u64 = 1;
pub const MAX_COLLECTION_INTERVAL_SECONDS: u64 = 300;

fn default_collection_interval_seconds() -> u64 {
    5
}

fn default_inventory_revision() -> u64 {
    1
}

/// Deterministic limits for one bounded Gap Backfill operation.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackfillConfig {
    /// Maximum inclusive height span examined by one recovery operation.
    #[serde(default = "default_backfill_height_span")]
    pub max_height_span: u64,
    /// Maximum point queries issued by one recovery operation.
    #[serde(default = "default_backfill_block_count")]
    pub max_block_count: u64,
    /// Maximum wall time budget in milliseconds for one recovery operation.
    #[serde(default = "default_backfill_time_ms")]
    pub max_time_ms: u64,
}

pub const MIN_BACKFILL_HEIGHT_SPAN: u64 = 1;
pub const MAX_BACKFILL_HEIGHT_SPAN: u64 = 1_000_000;
pub const MIN_BACKFILL_BLOCK_COUNT: u64 = 1;
pub const MAX_BACKFILL_BLOCK_COUNT: u64 = 100_000;
pub const MIN_BACKFILL_TIME_MS: u64 = 1;
pub const MAX_BACKFILL_TIME_MS: u64 = 60_000;

fn default_backfill_height_span() -> u64 {
    256
}
fn default_backfill_block_count() -> u64 {
    128
}
fn default_backfill_time_ms() -> u64 {
    5_000
}

impl Default for BackfillConfig {
    fn default() -> Self {
        Self {
            max_height_span: default_backfill_height_span(),
            max_block_count: default_backfill_block_count(),
            max_time_ms: default_backfill_time_ms(),
        }
    }
}

impl BackfillConfig {
    pub fn validate(&self) -> Result<(), AgentConfigError> {
        if !(MIN_BACKFILL_HEIGHT_SPAN..=MAX_BACKFILL_HEIGHT_SPAN).contains(&self.max_height_span) {
            return Err(AgentConfigError::InvalidBackfill(
                "max_height_span is outside its documented bounds".to_owned(),
            ));
        }
        if !(MIN_BACKFILL_BLOCK_COUNT..=MAX_BACKFILL_BLOCK_COUNT).contains(&self.max_block_count) {
            return Err(AgentConfigError::InvalidBackfill(
                "max_block_count is outside its documented bounds".to_owned(),
            ));
        }
        if !(MIN_BACKFILL_TIME_MS..=MAX_BACKFILL_TIME_MS).contains(&self.max_time_ms) {
            return Err(AgentConfigError::InvalidBackfill(
                "max_time_ms is outside its documented bounds".to_owned(),
            ));
        }
        Ok(())
    }
}

/// One Node declaration in the Agent's authoritative local configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentNodeConfig {
    pub node_id: NodeId,
    pub network_key: NetworkKey,
    pub rpc_endpoint: RpcEndpoint,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub process: Option<ProcessSelector>,
    /// Absolute PlatON data directory measured as this Node's disk usage.
    /// The local path is never included in the Agent Inventory or Public
    /// Projection.
    #[serde(default)]
    pub data_directory: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ValidatedAgentConfig {
    pub inventory: NodeInventory,
    pub data_directories: HashMap<NodeId, PathBuf>,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub config_path: PathBuf,
    pub server_url: String,
    pub credential_file: PathBuf,
    pub state_db: PathBuf,
    pub collection_interval_seconds: u64,
    pub backfill: BackfillConfig,
}

#[derive(Debug, Error)]
pub enum AgentConfigError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid server_url in {path}: {reason}")]
    InvalidServerUrl { path: PathBuf, reason: String },
    #[error("invalid node configuration: {0}")]
    InvalidNode(String),
    #[error("invalid backfill configuration: {0}")]
    InvalidBackfill(String),
    #[error("collection_interval_seconds must be between 1 and 300")]
    InvalidCollectionInterval,
    #[error("inventory_revision must be greater than zero")]
    InvalidInventoryRevision,
}

impl AgentConfigFile {
    pub fn load(path: &Path) -> Result<Self, AgentConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| AgentConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| AgentConfigError::Parse {
            path: path.to_owned(),
            source,
        })
    }

    /// Validate the entire Node set before an Inventory is submitted.
    pub fn validate(&self) -> Result<ValidatedAgentConfig, AgentConfigError> {
        if !(MIN_COLLECTION_INTERVAL_SECONDS..=MAX_COLLECTION_INTERVAL_SECONDS)
            .contains(&self.collection_interval_seconds)
        {
            return Err(AgentConfigError::InvalidCollectionInterval);
        }
        if self.inventory_revision == 0 {
            return Err(AgentConfigError::InvalidInventoryRevision);
        }
        self.backfill.validate()?;
        let mut seen_ids = std::collections::HashSet::new();
        let mut seen_endpoints = std::collections::HashSet::new();
        let mut nodes = Vec::with_capacity(self.nodes.len());
        let mut data_directories = HashMap::new();
        for node in &self.nodes {
            if !seen_ids.insert(node.node_id) {
                return Err(AgentConfigError::InvalidNode(format!(
                    "duplicate node_id {}",
                    node.node_id
                )));
            }
            if !seen_endpoints.insert(node.rpc_endpoint.as_str()) {
                return Err(AgentConfigError::InvalidNode(
                    "duplicate RPC endpoint; endpoint failover is not supported".to_owned(),
                ));
            }
            if let Some(name) = &node.display_name {
                if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
                    return Err(AgentConfigError::InvalidNode(
                        "display_name must be 1..=128 characters without control characters"
                            .to_owned(),
                    ));
                }
            }
            if let Some(selector) = &node.process {
                let valid = match selector {
                    ProcessSelector::SystemdUnit { unit } => {
                        !unit.is_empty()
                            && unit.len() <= 512
                            && !unit.chars().any(char::is_control)
                            && !unit.contains('/')
                    }
                    ProcessSelector::PidFile { path } => {
                        !path.is_empty()
                            && path.len() <= 512
                            && !path.chars().any(char::is_control)
                            && Path::new(path).is_absolute()
                    }
                };
                if !valid {
                    return Err(AgentConfigError::InvalidNode(
                        "process selector has an invalid or missing required value".to_owned(),
                    ));
                }
            }
            if let Some(path) = &node.data_directory {
                let text = path.to_string_lossy();
                if !path.is_absolute()
                    || text.is_empty()
                    || text.len() > 4096
                    || text.chars().any(char::is_control)
                {
                    return Err(AgentConfigError::InvalidNode(
                        "data_directory must be an absolute path of at most 4096 characters"
                            .to_owned(),
                    ));
                }
                data_directories.insert(node.node_id, path.clone());
            }
            nodes.push(InventoryNode {
                node_id: node.node_id,
                display_name: node.display_name.clone(),
                network_key: node.network_key.clone(),
                rpc_endpoint: node.rpc_endpoint.clone(),
                process: node.process.clone(),
            });
        }
        Ok(ValidatedAgentConfig {
            inventory: NodeInventory {
                revision: self.inventory_revision,
                nodes,
            },
            data_directories,
        })
    }
}

/// Generate a stable Node ID for a new local declaration.
pub fn generate_node_id() -> NodeId {
    uuid::Uuid::new_v4()
        .to_string()
        .parse()
        .expect("generated UUID is valid")
}

pub fn normalize_server_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err("must be an absolute http:// or https:// URL".to_owned());
    };
    if !matches!(scheme, "http" | "https") {
        return Err("scheme must be http or https".to_owned());
    }
    if rest.is_empty()
        || rest.contains('/')
        || rest.contains('?')
        || rest.contains('#')
        || rest.contains('@')
    {
        return Err(
            "server URL must be scheme://host[:port] with no path, query, fragment, or credentials"
                .to_owned(),
        );
    }
    if scheme == "http" && !is_loopback_host(rest) {
        return Err(
            "plaintext http is only allowed for loopback development servers; use https otherwise"
                .to_owned(),
        );
    }
    Ok(format!("{scheme}://{rest}"))
}

fn is_loopback_host(host_port: &str) -> bool {
    let host = host_port
        .rsplit_once(':')
        .map_or(host_port, |(host, _)| host);
    matches!(
        host.trim_matches(['[', ']']),
        "127.0.0.1" | "::1" | "localhost"
    )
}

impl AgentConfig {
    pub fn resolve(config_path: &Path) -> Result<Self, AgentConfigError> {
        let file = AgentConfigFile::load(config_path)?;
        let server_url = normalize_server_url(&file.server_url).map_err(|reason| {
            AgentConfigError::InvalidServerUrl {
                path: config_path.to_owned(),
                reason,
            }
        })?;
        if !(MIN_COLLECTION_INTERVAL_SECONDS..=MAX_COLLECTION_INTERVAL_SECONDS)
            .contains(&file.collection_interval_seconds)
        {
            return Err(AgentConfigError::InvalidCollectionInterval);
        }
        Ok(Self {
            config_path: config_path.to_owned(),
            server_url,
            credential_file: file.credential_file,
            state_db: file.state_db,
            collection_interval_seconds: file.collection_interval_seconds,
            backfill: file.backfill,
        })
    }

    pub fn validated_inventory(&self) -> Result<ValidatedAgentConfig, AgentConfigError> {
        AgentConfigFile::load(&self.config_path)?.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_config(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("agent.toml");
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn full_config_resolves_and_normalizes_server_url() {
        let dir = tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "server_url = \"https://monitor.example.com/\"\ncredential_file = \"/tmp/c\"\nstate_db = \"/tmp/db\"\n",
        );
        let config = AgentConfig::resolve(&path).unwrap();
        assert_eq!(config.server_url, "https://monitor.example.com");
        assert_eq!(config.collection_interval_seconds, 5);
    }

    #[test]
    fn collection_interval_has_safe_configurable_bounds() {
        let dir = tempdir().unwrap();
        let config = |seconds| {
            write_config(
                dir.path(),
                &format!(
                    "server_url=\"https://example.com\"\ncredential_file=\"/tmp/c\"\nstate_db=\"/tmp/d\"\ncollection_interval_seconds={seconds}\n"
                ),
            )
        };

        assert_eq!(
            AgentConfig::resolve(&config(3))
                .unwrap()
                .collection_interval_seconds,
            3
        );
        assert!(matches!(
            AgentConfig::resolve(&config(0)),
            Err(AgentConfigError::InvalidCollectionInterval)
        ));
        assert!(matches!(
            AgentConfig::resolve(&config(301)),
            Err(AgentConfigError::InvalidCollectionInterval)
        ));
    }

    #[test]
    fn missing_required_fields_and_unknown_fields_fail() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            AgentConfig::resolve(&write_config(
                dir.path(),
                "server_url = \"https://example.com\"\n"
            ))
            .unwrap_err(),
            AgentConfigError::Parse { .. }
        ));
        let path = write_config(
            dir.path(),
            "server_url = \"https://example.com\"\ncredential_file=\"/tmp/c\"\nstate_db=\"/tmp/d\"\nnonsense=[]\n",
        );
        assert!(matches!(
            AgentConfig::resolve(&path).unwrap_err(),
            AgentConfigError::Parse { .. }
        ));
    }

    #[test]
    fn validates_whole_inventory_and_endpoint_rules() {
        let id = "0195f2a1-2b3c-4d5e-8f90-123456789abc";
        let base = format!(
            "server_url=\"https://example.com\"\ncredential_file=\"/tmp/c\"\nstate_db=\"/tmp/d\"\nnodes=[{{node_id=\"{id}\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:1\"}}]\n"
        );
        let file: AgentConfigFile = toml::from_str(&base).unwrap();
        assert_eq!(file.validate().unwrap().inventory.nodes.len(), 1);
        for endpoint in ["http://127.0.0.1:1", "https://node.example", "ipc://"] {
            let text = base.replace("ws://127.0.0.1:1", endpoint);
            assert!(toml::from_str::<AgentConfigFile>(&text).is_err());
        }
        let duplicate = base.replace("nodes=[", &format!("nodes=[{{node_id=\"{id}\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:2\"}},"));
        let file: AgentConfigFile = toml::from_str(&duplicate).unwrap();
        assert!(matches!(
            file.validate(),
            Err(AgentConfigError::InvalidNode(_))
        ));
    }

    #[test]
    fn validates_explicit_node_data_directories_without_publishing_the_path() {
        let id = "0195f2a1-2b3c-4d5e-8f90-123456789abc";
        let configured: AgentConfigFile = toml::from_str(&format!(
            "server_url=\"https://example.com\"\ncredential_file=\"/tmp/c\"\nstate_db=\"/tmp/d\"\nnodes=[{{node_id=\"{id}\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:1\",data_directory=\"/var/lib/platon/data\"}}]\n"
        ))
        .unwrap();
        let validated = configured.validate().unwrap();
        let node_id: NodeId = id.parse().unwrap();
        assert_eq!(
            validated.data_directories.get(&node_id),
            Some(&PathBuf::from("/var/lib/platon/data"))
        );
        assert!(
            !serde_json::to_string(&validated.inventory)
                .unwrap()
                .contains("/var/lib/platon/data")
        );

        let relative: AgentConfigFile = toml::from_str(&format!(
            "server_url=\"https://example.com\"\ncredential_file=\"/tmp/c\"\nstate_db=\"/tmp/d\"\nnodes=[{{node_id=\"{id}\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:1\",data_directory=\"relative/data\"}}]\n"
        ))
        .unwrap();
        assert!(matches!(
            relative.validate(),
            Err(AgentConfigError::InvalidNode(_))
        ));
    }

    #[test]
    fn generates_uuid_node_ids() {
        assert_ne!(generate_node_id(), generate_node_id());
    }
}
