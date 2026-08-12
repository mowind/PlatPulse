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

        // Unknown payload fields are rejected, not silently dropped.
        assert!(
            serde_json::from_str::<ProcessSelector>(
                r#"{"kind":"systemd_unit","unit":"x.service","bogus":1}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<ProcessSelector>(r#"{"kind":"pid_file"}"#).is_err());
    }
}
