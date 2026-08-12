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

/// Suggested flush threshold at which Agents are expected to flush early.
pub const EARLY_FLUSH_BODY_BYTES: usize = 2 * 1024 * 1024;

/// HTTP path of the v1 AgentReport endpoint.
pub const AGENT_API_REPORTS_PATH: &str = "/api/agent/v1/reports";
