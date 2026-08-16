//! Protocol-level constants of the Agent→Server wire contract.
//!
//! These values are part of the frozen v1 contract: changing them (or the
//! types in this crate) requires a new protocol major and a new fixture set.

/// Major version of the Agent→Server wire protocol implemented by this crate.
///
/// The Server rejects reports whose `protocol_version` differs from this value
/// with the stable `unsupported_protocol_version` rejection code; only the
/// majors listed in `SUPPORTED_PROTOCOL_MAJORS` are accepted.
pub const PROTOCOL_VERSION: u64 = 1;

/// Protocol majors the v1 Server accepts on the Agent API.
pub const SUPPORTED_PROTOCOL_MAJORS: &[u64] = &[PROTOCOL_VERSION];

/// Hard upper bound on a single AgentReport body, enforced by the Server
/// before deserialization. Agents flush early when approaching 2 MiB or the
/// sample threshold, and enter a degraded/fatal state rather than exceeding
/// this limit (see design §8.5).
pub const MAX_REPORT_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Maximum number of Nodes in one complete Inventory.
pub const MAX_INVENTORY_NODES: usize = 256;
/// Maximum current Node observations in one report.
pub const MAX_NODE_OBSERVATIONS: usize = 256;
/// Maximum newly collected block samples in one report.
pub const MAX_BLOCK_SUMMARIES: usize = 512;
/// Maximum declared history gaps in one report.
pub const MAX_HISTORY_GAPS: usize = 256;
/// Maximum capabilities advertised by one Agent.
pub const MAX_AGENT_CAPABILITIES: usize = 32;
/// Earliest and latest representable protocol wall-clock years.
pub const MIN_TIMESTAMP_UNIX_SECONDS: i64 = 0;
pub const MAX_TIMESTAMP_UNIX_SECONDS: i64 = 4_102_444_800; // 2100-01-01T00:00:00Z

/// Maximum Peer entries in one current Peer Snapshot.
pub const MAX_PEERS: usize = 1024;
/// Maximum Peer ID length in bytes.
pub const MAX_PEER_ID_BYTES: usize = 128;
/// Maximum optional Peer client name length in bytes.
pub const MAX_PEER_CLIENT_NAME_BYTES: usize = 256;
/// Maximum capabilities carried by one Peer.
pub const MAX_PEER_CAPABILITIES: usize = 64;
/// Maximum capability string length in bytes.
pub const MAX_PEER_CAPABILITY_BYTES: usize = 128;
/// Maximum CBFT protocol version represented for one Peer.
pub const MAX_PEER_CBFT_PROTOCOL_VERSION: u64 = 1024;
/// Maximum CBFT block number accepted by the SQLite INTEGER projection.
pub const MAX_PEER_CBFT_BLOCK: u64 = i64::MAX as u64;
/// Maximum canonical literal IP length in bytes.
pub const MAX_PEER_REMOTE_IP_BYTES: usize = 45;
/// Maximum individual Agent-provided diagnostic/error code length.
pub const MAX_ERROR_CODE_BYTES: usize = 128;
/// Maximum individual Agent-provided diagnostic/error message length.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 1024;
/// Maximum attribution reason/evidence text length.
pub const MAX_DIAGNOSTIC_TEXT_BYTES: usize = 1024;

/// HTTP path of the v1 AgentReport endpoint.
pub const AGENT_API_REPORTS_PATH: &str = "/api/agent/v1/reports";
