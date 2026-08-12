//! `platpulse-agent` — the collector process that runs on a Host next to its
//! PlatON Nodes and reports each Node's observations independently.
//!
//! Phase 0 provided the thin-binary/library split and the Agent-local SQLite
//! startup/migration harness. P1-03 adds the Agent configuration, the
//! credential file policy, and the Enrollment client (`platpulse-agent
//! enroll`). The per-Node collectors, the full AgentStore spool behavior,
//! and the report sender arrive in later tickets. Keep startup logic in
//! this library so it can be exercised from tests; `main.rs` stays a thin
//! entry point.

pub mod cli;
pub mod config;
pub mod credential;
pub mod database;
pub mod enroll;

pub use database::{
    AGENT_MIGRATOR, AGENT_SCHEMA_VERSION, AgentDatabaseConfig, AgentDatabaseError, AgentStore,
    DEFAULT_BUSY_TIMEOUT, JournalMode, SqlitePragmas, initialize,
};

/// Name of the agent binary, as declared in `Cargo.toml`.
pub const BINARY_NAME: &str = env!("CARGO_PKG_NAME");

/// Version of the agent crate, as declared in `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One-line identity printed at process start.
pub fn startup_version_line() -> String {
    format!("{BINARY_NAME} {VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_line_names_the_agent() {
        assert!(startup_version_line().starts_with("platpulse-agent "));
    }
}
