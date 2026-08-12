//! Agent configuration (design §8.2).

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
    #[serde(default = "default_inventory_revision")]
    pub inventory_revision: u64,
    #[serde(default)]
    pub nodes: Vec<AgentNodeConfig>,
}

fn default_inventory_revision() -> u64 {
    1
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
}

#[derive(Debug, Clone)]
pub struct ValidatedAgentConfig {
    pub inventory: NodeInventory,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub config_path: PathBuf,
    pub server_url: String,
    pub credential_file: PathBuf,
    pub state_db: PathBuf,
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
        if self.inventory_revision == 0 {
            return Err(AgentConfigError::InvalidInventoryRevision);
        }
        let mut seen_ids = std::collections::HashSet::new();
        let mut seen_endpoints = std::collections::HashSet::new();
        let mut nodes = Vec::with_capacity(self.nodes.len());
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
        Ok(Self {
            config_path: config_path.to_owned(),
            server_url,
            credential_file: file.credential_file,
            state_db: file.state_db,
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
    fn generates_uuid_node_ids() {
        assert_ne!(generate_node_id(), generate_node_id());
    }
}
