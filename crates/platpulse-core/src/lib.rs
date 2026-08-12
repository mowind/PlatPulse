//! I/O-free shared foundation for PlatPulse.
//!
//! This crate is the only place where Agent↔Server wire concepts may live
//! (AgentReport v1, Observation Envelope, Block Summary, History Gap,
//! receipt/error codes, wire validation). It must never depend on I/O
//! frameworks such as Axum, SQLx, or Alloy, and it must never contain
//! Server rows or Public/Admin DTOs.
//!
//! Phase 0 establishes the crate boundary only; the AgentReport v1 wire
//! protocol is tracked by a later ticket.

/// Version of the Agent→Server wire protocol this workspace targets.
pub const PROTOCOL_VERSION: u64 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_v1() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
