//! The Observation Envelope: one uniform status/error/last-good wrapper per
//! independently collected component.
//!
//! Rules (design §5.2):
//! - a collection failure updates `status`/`error` but never overwrites the
//!   last successful `latest` value;
//! - a successful authoritative empty result (e.g. an empty list) is still a
//!   value and clears the previous set;
//! - missing/unknown/never-observed state is never represented as `0`,
//!   `false`, or Healthy;
//! - `received_at` is populated by the Server at commit time and is never
//!   sent by the Agent.

use std::marker::PhantomData;

use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::WireError;
use crate::time::Rfc3339;

/// Stable lowercase status of one component collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    /// First attempt in this boot is still in flight; no success yet.
    Starting,
    /// The last attempt succeeded; `latest` holds the last-good value.
    Ok,
    /// The last attempt failed; `error` describes it and `latest` is retained.
    Error,
    /// Collection is not configured (e.g. no process selector). Not an error.
    Disabled,
    /// The upstream method/capability is not available on this build/Node.
    Unsupported,
}

/// A bounded, safe-to-display collector error.
///
/// The Agent redacts RPC URLs, credentials, and paths before sending;
/// the Server never surfaces the raw message to unauthenticated viewers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundedError {
    /// Stable lowercase snake_case code identifying the failure kind
    /// (contract limit: 128 chars).
    pub code: String,
    /// Human-readable diagnostic (contract limit: 1024 chars).
    pub message: String,
}

/// The Observation Envelope shared by every component of the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
#[serde(deny_unknown_fields)]
pub struct ComponentObservation<T> {
    /// Current component state; `error` ↔ `error` status, `starting` never
    /// carries a last-good value.
    pub status: ComponentStatus,
    /// When the latest attempt was made. Required for `starting`/`ok`/
    /// `error`; omitted when the component was never attempted (typically
    /// `disabled`/`unsupported`).
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "strict_optional"
    )]
    pub attempted_at: Option<Rfc3339>,
    /// Agent wall-clock time at which `latest` was observed.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "strict_optional"
    )]
    pub latest_observed_at: Option<Rfc3339>,
    /// Server commit time of the report that carried this observation.
    /// Server-populated only; never sent by the Agent.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "strict_optional"
    )]
    pub received_at: Option<Rfc3339>,
    /// Revision of the status/error/attempted_at state; monotonic per
    /// component per Agent.
    pub state_revision: u64,
    /// Revision of the `latest` value; monotonic per component per Agent.
    pub value_revision: u64,
    /// Last successful value. Omitted until the first success; an
    /// authoritative empty result is a value, not an omission.
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "strict_optional"
    )]
    pub latest: Option<T>,
    /// Current failure of this component (present iff `status == error`).
    #[serde(
        default = "crate::component::default_none",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "strict_optional"
    )]
    pub error: Option<BoundedError>,
}

/// Stable lowercase keys identifying each component in receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKey {
    /// `HostObservation::cpu_percent`
    CpuPercent,
    /// `HostObservation::memory`
    Memory,
    /// `HostObservation::load`
    Load,
    /// `HostObservation::disk`
    Disk,
    /// `HostObservation::network_throughput`
    NetworkThroughput,
    /// `HostObservation::clock_skew`
    ClockSkew,
    /// `HostObservation::spool`
    Spool,
    /// `NodeObservation::process`
    Process,
    /// `NodeChainObservation::rpc`
    Rpc,
    /// `NodeChainObservation::sync`
    Sync,
    /// `NodeChainObservation::consensus`
    Consensus,
    /// `NodeChainObservation::network_identity`
    NetworkIdentity,
    /// `NodeChainObservation::static_metadata`
    StaticMetadata,
    /// `NodeChainObservation::peers`
    Peers,
}

/// Contract limits for [`BoundedError`].
pub mod error_limits {
    /// Maximum length of `BoundedError::code`.
    pub const CODE_MAX: usize = 128;
    /// Maximum length of `BoundedError::message`.
    pub const MESSAGE_MAX: usize = 1024;
}

/// Deserializer for optional fields: an omitted field is `None`, an explicit
/// `null` is a contract violation. This keeps `omitted` and `null` distinct
/// on the wire.
pub(crate) fn strict_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    deserializer.deserialize_option(StrictOptionalVisitor(PhantomData))
}

/// Default for strict-optional fields (used with `#[serde(default = …)]`);
/// avoids the `T: Default` bound that `#[serde(default)]` would add to
/// generic envelope types.
pub(crate) fn default_none<T>() -> Option<T> {
    None
}

struct StrictOptionalVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for StrictOptionalVisitor<T> {
    type Value = Option<T>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "a value or an omitted field; explicit null is not allowed"
        )
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Err(E::custom(
            "explicit null is not allowed on the wire; omit the field instead",
        ))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Some)
    }
}

/// Envelope invariants shared by every component (used by report validation).
pub(crate) fn validate_component<T>(
    component: &'static str,
    obs: &ComponentObservation<T>,
) -> Result<(), WireError> {
    if obs.latest.is_some() && obs.latest_observed_at.is_none() {
        return Err(WireError::ComponentLatestWithoutObservedAt { component });
    }
    if obs.latest_observed_at.is_some() && obs.latest.is_none() {
        return Err(WireError::ComponentObservedAtWithoutLatest { component });
    }
    if obs.status == ComponentStatus::Error && obs.error.is_none() {
        return Err(WireError::ComponentErrorWithoutMessage { component });
    }
    if obs.error.is_some() && obs.status != ComponentStatus::Error {
        return Err(WireError::ComponentMessageWithoutError { component });
    }
    if obs.status == ComponentStatus::Starting && obs.latest.is_some() {
        return Err(WireError::ComponentStartingWithValue { component });
    }
    if matches!(
        obs.status,
        ComponentStatus::Starting | ComponentStatus::Ok | ComponentStatus::Error
    ) && obs.attempted_at.is_none()
    {
        return Err(WireError::ComponentAttemptedAtMissing { component });
    }
    if obs.status == ComponentStatus::Ok && obs.latest.is_none() {
        return Err(WireError::ComponentOkWithoutValue { component });
    }
    if obs.received_at.is_some() {
        // Trust boundary: only the Server may set received_at at commit
        // time; an Agent report carrying it falsifies freshness evidence.
        return Err(WireError::ComponentCarriesReceivedAt { component });
    }
    if let Some(error) = &obs.error {
        if !error.code.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        }) {
            return Err(WireError::FieldTooLong {
                field: "error.code",
                len: error.code.len(),
                max: crate::protocol::MAX_ERROR_CODE_BYTES,
            });
        }
        if error.code.len() > crate::protocol::MAX_ERROR_CODE_BYTES {
            return Err(WireError::FieldTooLong {
                field: "error.code",
                len: error.code.len(),
                max: crate::protocol::MAX_ERROR_CODE_BYTES,
            });
        }
        if error.message.len() > crate::protocol::MAX_ERROR_MESSAGE_BYTES {
            return Err(WireError::FieldTooLong {
                field: "error.message",
                len: error.message.len(),
                max: crate::protocol::MAX_ERROR_MESSAGE_BYTES,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component() -> ComponentObservation<u64> {
        ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some("2026-08-12T10:00:00Z".parse().unwrap()),
            latest_observed_at: Some("2026-08-12T10:00:00Z".parse().unwrap()),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(42),
            error: None,
        }
    }

    #[test]
    fn serializes_snake_case_and_skips_none() {
        let json = serde_json::to_string(&component()).unwrap();
        assert!(json.contains("\"status\":\"ok\""));
        assert!(!json.contains("received_at"));
        assert!(json.contains("\"latest\":42"));
    }

    #[test]
    fn explicit_null_is_rejected() {
        let json = r#"{"status":"ok","attempted_at":"2026-08-12T10:00:00Z","latest":null}"#;
        let result: Result<ComponentObservation<u64>, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn omitted_optionals_are_fine() {
        let json = r#"{"status":"disabled","state_revision":1,"value_revision":0}"#;
        let obs: ComponentObservation<u64> = serde_json::from_str(json).unwrap();
        assert_eq!(obs.status, ComponentStatus::Disabled);
        assert_eq!(obs.attempted_at, None);
        assert_eq!(obs.latest, None);
    }

    #[test]
    fn unknown_enum_value_is_rejected() {
        let json = r#"{"status":"healthy","state_revision":1,"value_revision":0}"#;
        assert!(serde_json::from_str::<ComponentObservation<u64>>(json).is_err());
    }

    #[test]
    fn envelope_invariants() {
        let mut ok = component();
        ok.latest = None;
        assert_eq!(
            validate_component("x", &ok),
            Err(WireError::ComponentObservedAtWithoutLatest { component: "x" })
        );

        let mut err = component();
        err.status = ComponentStatus::Error;
        err.error = None;
        assert_eq!(
            validate_component("x", &err),
            Err(WireError::ComponentErrorWithoutMessage { component: "x" })
        );

        let mut err = component();
        err.error = Some(BoundedError {
            code: "boom".into(),
            message: "kaboom".into(),
        });
        assert_eq!(
            validate_component("x", &err),
            Err(WireError::ComponentMessageWithoutError { component: "x" })
        );

        let mut starting = component();
        starting.status = ComponentStatus::Starting;
        assert_eq!(
            validate_component("x", &starting),
            Err(WireError::ComponentStartingWithValue { component: "x" })
        );

        let mut bounded = component();
        bounded.status = ComponentStatus::Error;
        bounded.error = Some(BoundedError {
            code: "x".repeat(129),
            message: "y".into(),
        });
        assert!(matches!(
            validate_component("x", &bounded),
            Err(WireError::FieldTooLong {
                field: "error.code",
                ..
            })
        ));

        let disabled = ComponentObservation::<u64> {
            status: ComponentStatus::Disabled,
            attempted_at: None,
            latest_observed_at: None,
            received_at: None,
            state_revision: 1,
            value_revision: 0,
            latest: None,
            error: None,
        };
        assert_eq!(validate_component("x", &disabled), Ok(()));
    }

    #[test]
    fn received_at_is_server_populated_only() {
        let mut obs = component();
        obs.received_at = Some("2026-08-12T10:00:01Z".parse().unwrap());
        assert_eq!(
            validate_component("x", &obs),
            Err(WireError::ComponentCarriesReceivedAt { component: "x" })
        );
    }
}
