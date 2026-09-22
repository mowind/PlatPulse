//! Node Inventory: the complete set of Nodes an Agent declares from its local
//! configuration.
//!
//! The Inventory is a complete set, not a patch: it identifies which Nodes
//! currently belong to the Agent. The Server validates it as a whole
//! (`accepted` / `unchanged` / `rejected`) — a valid subset never counts as a
//! new Inventory, and a rejected Inventory never retires or transfers Nodes.
//! Local config is the source of truth for connection details; the Server
//! never pushes endpoints.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::envelope::AgentCapability;
use crate::hex::Sha256Hex;
use crate::identity::NodeId;
use crate::network::{NetworkKey, RpcEndpoint};

/// The complete Node Inventory of one Agent, with its monotonic revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeInventory {
    /// Monotonic per-Agent revision; must be >= 1 and must not regress.
    pub revision: u64,
    /// The complete set of Nodes. May be empty (authoritative empty set),
    /// but must not list a Node twice.
    pub nodes: Vec<InventoryNode>,
}

impl NodeInventory {
    /// The canonical content hash of this Inventory: SHA-256 over its
    /// canonical JSON serialization (revision and nodes), exactly the value
    /// the Server persists as `agents.inventory_sha256` and compares when a
    /// report declares the already-accepted revision. It lives here so the
    /// Server's comparison and the Agent's Inventory Declaration Record can
    /// never drift apart.
    ///
    /// The serialization includes `revision`, so only two Inventories at the
    /// same revision are meaningfully comparable by this value.
    pub fn content_sha256(&self) -> Sha256Hex {
        let bytes = serde_json::to_vec(self).expect("inventory serializes");
        format!("0x{:x}", Sha256::digest(&bytes))
            .parse()
            .expect("a sha256 digest is canonical lowercase hex")
    }
}

/// One declared Node in the Inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryNode {
    /// The stable PlatPulse Node ID, created and persisted by the Agent.
    pub node_id: NodeId,
    /// Bootstrap display-name suggestion for first sighting only
    /// (contract limit: 128 chars). Never overrides the Server-managed
    /// display name of an already-registered Node.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub display_name: Option<String>,
    /// Configured Network Registry key, e.g. `platon-mainnet`.
    pub network_key: NetworkKey,
    /// The Node's single RPC Endpoint (IPC/WS/WSS, no failover).
    pub rpc_endpoint: RpcEndpoint,
    /// Optional explicit process selector; absent means process collection
    /// is `disabled` while RPC/chain collection continues.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::component::strict_optional"
    )]
    pub process: Option<ProcessSelector>,
}

/// The explicit process selector of a Node. At most one per Node; process
/// identity is never guessed from name, command line, or RPC port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessSelector {
    /// A systemd unit, e.g. `platon-validator-a.service`
    /// (contract limit: 512 chars).
    SystemdUnit {
        /// Unit name.
        unit: String,
    },
    /// A PID file, e.g. `/run/platon-validator-a.pid`
    /// (contract limit: 512 chars).
    PidFile {
        /// Path to the PID file.
        path: String,
    },
    /// A supervisord program, e.g. `platon-validator-a`. Use the
    /// `group:process` form when the program declares `numprocs > 1`
    /// (contract limit: 512 chars).
    Supervisor {
        /// The `supervisorctl` program (or `group:process`) name.
        program: String,
    },
}

impl ProcessSelector {
    /// The Agent capability that declaring this selector advertises.
    pub fn capability(&self) -> AgentCapability {
        match self {
            Self::SystemdUnit { .. } => AgentCapability::ProcessSystemd,
            Self::PidFile { .. } => AgentCapability::ProcessPidFile,
            Self::Supervisor { .. } => AgentCapability::ProcessSupervisor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_selector_wire_forms() {
        let unit = ProcessSelector::SystemdUnit {
            unit: "platon-validator-a.service".into(),
        };
        let json = serde_json::to_string(&unit).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"systemd_unit","unit":"platon-validator-a.service"}"#
        );
        let back: ProcessSelector = serde_json::from_str(&json).unwrap();
        assert_eq!(back, unit);

        let pid = ProcessSelector::PidFile {
            path: "/run/platon-a.pid".into(),
        };
        let json = serde_json::to_string(&pid).unwrap();
        assert_eq!(json, r#"{"kind":"pid_file","path":"/run/platon-a.pid"}"#);
        let back: ProcessSelector = serde_json::from_str(&json).unwrap();
        assert_eq!(back, pid);

        // A supervisord program selector, including the group:process form
        // used when the program declares numprocs > 1.
        let supervisor = ProcessSelector::Supervisor {
            program: "platon-validator-a".into(),
        };
        let json = serde_json::to_string(&supervisor).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"supervisor","program":"platon-validator-a"}"#
        );
        let back: ProcessSelector = serde_json::from_str(&json).unwrap();
        assert_eq!(back, supervisor);

        let grouped = ProcessSelector::Supervisor {
            program: "validators:platon-validator-a".into(),
        };
        let json = serde_json::to_string(&grouped).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"supervisor","program":"validators:platon-validator-a"}"#
        );
        let back: ProcessSelector = serde_json::from_str(&json).unwrap();
        assert_eq!(back, grouped);

        // Unknown payload fields are rejected, not silently dropped.
        assert!(
            serde_json::from_str::<ProcessSelector>(
                r#"{"kind":"systemd_unit","unit":"x.service","bogus":1}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<ProcessSelector>(r#"{"kind":"pid_file"}"#).is_err());
        assert!(serde_json::from_str::<ProcessSelector>(r#"{"kind":"supervisor"}"#).is_err());
        assert!(
            serde_json::from_str::<ProcessSelector>(
                r#"{"kind":"supervisor","program":"x","bogus":1}"#
            )
            .is_err()
        );
    }

    fn one_node(display_name: Option<&str>) -> InventoryNode {
        InventoryNode {
            node_id: "0195f2a1-2b3c-4d5e-8f90-123456789abc".parse().unwrap(),
            display_name: display_name.map(str::to_owned),
            network_key: "platon-mainnet".parse().unwrap(),
            rpc_endpoint: "ws://127.0.0.1:6790".parse().unwrap(),
            process: None,
        }
    }

    #[test]
    fn inventory_content_hash_is_the_canonical_serialized_digest() {
        let inventory = NodeInventory {
            revision: 4,
            nodes: vec![one_node(None)],
        };
        let expected = format!(
            "0x{:x}",
            Sha256::digest(serde_json::to_vec(&inventory).unwrap())
        );
        assert_eq!(inventory.content_sha256().as_str(), expected);
        assert_eq!(inventory.content_sha256(), inventory.content_sha256());
    }

    #[test]
    fn inventory_content_hash_changes_with_content_and_with_revision() {
        let base = NodeInventory {
            revision: 4,
            nodes: vec![one_node(None)],
        };
        let renamed = NodeInventory {
            revision: 4,
            nodes: vec![one_node(Some("validator-a"))],
        };
        let bumped = NodeInventory {
            revision: 5,
            nodes: base.nodes.clone(),
        };
        let removed = NodeInventory {
            revision: 4,
            nodes: Vec::new(),
        };
        assert_ne!(base.content_sha256(), renamed.content_sha256());
        assert_ne!(base.content_sha256(), bumped.content_sha256());
        assert_ne!(base.content_sha256(), removed.content_sha256());
    }
}
