//! Stable identity types of the Agent↔Server wire contract.
//!
//! Encoding rule: every identity is a canonical hyphenated lowercase UUID
//! string. Non-UUID strings (endpoint URLs, display names, validator keys,
//! P2P node IDs) never substitute for these identities.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// The wrapped UUID.
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

uuid_id!(
    /// Globally unique Agent identity, issued by the Server at Enrollment.
    /// Stable across boots; only Enrollment/Recovery/Reset creates a new one.
    AgentId
);
uuid_id!(
    /// Globally unique PlatON Node identity created and persisted by the
    /// Agent. Retained across display-name, endpoint, and ownership changes.
    NodeId
);
uuid_id!(
    /// Identity of one Agent boot. A new Boot ID is generated at every normal
    /// start; crash recovery keeps the previous Boot ID until its backlog is
    /// drained and the boot is closed.
    BootId
);
uuid_id!(
    /// Identity of one immutable AgentReport. HTTP retries reuse the same
    /// `report_id` and the identical body bytes.
    ReportId
);

#[cfg(test)]
mod tests {
    use super::*;

    const UUID_V4: &str = "0195f2a1-2b3c-4d5e-8f90-123456789abc";

    #[test]
    fn parses_canonical_uuid() {
        let id: NodeId = UUID_V4.parse().unwrap();
        assert_eq!(id.to_string(), UUID_V4);
        assert_eq!(id.as_uuid().to_string(), UUID_V4);
    }

    #[test]
    fn rejects_non_uuid_strings() {
        assert!("not-a-uuid".parse::<AgentId>().is_err());
        assert!("0195f2a1".parse::<BootId>().is_err());
        assert!(
            "0195f2a1-2b3c-4d5e-8f90-123456789abcZZ"
                .parse::<ReportId>()
                .is_err()
        );
    }

    #[test]
    fn identities_are_distinct_types() {
        let agent: AgentId = UUID_V4.parse().unwrap();
        let node: NodeId = UUID_V4.parse().unwrap();
        let boot: BootId = UUID_V4.parse().unwrap();
        let report: ReportId = UUID_V4.parse().unwrap();
        assert_eq!(agent.to_string(), node.to_string());
        assert_eq!(boot.to_string(), report.to_string());
    }
}
