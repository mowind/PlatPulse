//! Node Currency and the Node Health Summary.
//!
//! Whether a Node's own chain evidence describes it as it is now is one
//! judgement, made here and nowhere else: the Public projection, the Admin
//! projection, the head-lag alerts and the Observed Network Head source filter
//! all ask this module. The judgement reads the Server-clock observation
//! instants of the three gating observations, RPC, sync and consensus (ADR
//! 0014), against the one Server window
//! [`FRESHNESS_LIMIT_SECONDS`](crate::observation_time::FRESHNESS_LIMIT_SECONDS).
//! Owner alert thresholds never move it: the Public projection must read the
//! same for every reader, and tuning an alert is not a statement about a Node's
//! Health.
//!
//! Peers, process and Host are not part of Node Currency. Process and network
//! identity are separate Node Health Summary dimensions, handed to
//! [`health_summary`] by the caller.

use std::collections::HashMap;

use sqlx::AnyConnection;
use time::OffsetDateTime;

use crate::observation_time::FRESHNESS_LIMIT_SECONDS;

/// Rows per load statement, far below every Storage Backend's bind limit.
const LOAD_CHUNK: usize = 500;

/// One of the three observations Node Currency is judged from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatingComponent {
    Rpc,
    Sync,
    Consensus,
}

impl GatingComponent {
    const ALL: [Self; 3] = [Self::Rpc, Self::Sync, Self::Consensus];

    /// The `component_status.component_key` the observation is stored under.
    pub fn key(self) -> &'static str {
        match self {
            Self::Rpc => "rpc",
            Self::Sync => "sync",
            Self::Consensus => "consensus",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|component| component.key() == key)
    }
}

/// The recorded state of one gating observation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ComponentEvidence {
    /// Collection state of the newest attempt; `None` when no row exists.
    state: Option<String>,
    /// Server-clock instant of the newest attempt; `None` when the Server
    /// holds no instant it can read, which is the same as never observed.
    observed_at: Option<OffsetDateTime>,
}

/// The gating evidence one Node's Currency and Health are judged from, as
/// read by [`load`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CurrencyEvidence {
    rpc: ComponentEvidence,
    sync: ComponentEvidence,
    consensus: ComponentEvidence,
}

impl CurrencyEvidence {
    fn component(&self, component: GatingComponent) -> &ComponentEvidence {
        match component {
            GatingComponent::Rpc => &self.rpc,
            GatingComponent::Sync => &self.sync,
            GatingComponent::Consensus => &self.consensus,
        }
    }

    fn component_mut(&mut self, component: GatingComponent) -> &mut ComponentEvidence {
        match component {
            GatingComponent::Rpc => &mut self.rpc,
            GatingComponent::Sync => &mut self.sync,
            GatingComponent::Consensus => &mut self.consensus,
        }
    }

    fn state(&self, component: GatingComponent) -> Option<&str> {
        self.component(component).state.as_deref()
    }

    /// The Server-clock instant `component` was last observed at, if any.
    pub fn observed_at(&self, component: GatingComponent) -> Option<OffsetDateTime> {
        self.component(component).observed_at
    }

    /// Judge the Node's Currency at `now`.
    ///
    /// A component observed outside the window, in either direction, makes the
    /// Node Stale even when another was never observed: an old observation is
    /// evidence that is no longer current, while a missing one is evidence
    /// that never existed, and only the former says the projection fell behind.
    pub fn currency(&self, now: OffsetDateTime) -> NodeCurrency {
        let mut never_observed = false;
        for component in GatingComponent::ALL {
            match self.component(component).observed_at {
                None => never_observed = true,
                Some(observed_at) => {
                    if !crate::observation_time::is_fresh_at(
                        observed_at,
                        now,
                        FRESHNESS_LIMIT_SECONDS,
                    ) {
                        return NodeCurrency::Stale {
                            component,
                            observed_at,
                        };
                    }
                }
            }
        }
        if never_observed {
            NodeCurrency::NeverObserved
        } else {
            NodeCurrency::Current
        }
    }

    /// The oldest of the three Server-clock observation instants, the instant
    /// the Node's Currency is as of; `None` when any was never observed.
    pub fn as_of(&self) -> Option<String> {
        GatingComponent::ALL
            .into_iter()
            .map(|component| self.component(component).observed_at)
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .min()
            .map(crate::auth::format_rfc3339)
    }
}

/// Whether a Node's own chain evidence describes it as it is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeCurrency {
    /// RPC, sync and consensus were all observed inside the window.
    Current,
    /// `component` was observed at `observed_at`, outside the window.
    Stale {
        component: GatingComponent,
        observed_at: OffsetDateTime,
    },
    /// At least one gating observation was never observed, and none is stale.
    NeverObserved,
}

impl NodeCurrency {
    /// Whether a head-lag figure must be withheld.
    ///
    /// A lag figure states how far behind the Node is now, so stale evidence
    /// cannot carry one. A component that was never observed does not withhold
    /// it: an Agent that never reports consensus has not fallen behind, and the
    /// lag rule's own checks decide what its evidence supports.
    pub fn blocks_lag_figure(&self) -> bool {
        matches!(self, Self::Stale { .. })
    }

    /// The state a Node DTO carries, without the stale component's detail.
    pub fn state(&self) -> NodeCurrencyState {
        match self {
            Self::Current => NodeCurrencyState::Current,
            Self::Stale { .. } => NodeCurrencyState::Stale,
            Self::NeverObserved => NodeCurrencyState::NeverObserved,
        }
    }
}

/// The Node Currency as Public and Admin Node DTOs report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeCurrencyState {
    Current,
    Stale,
    NeverObserved,
}

/// Load the gating evidence of every Node in `node_ids`.
///
/// Every requested Node has an entry; a Node without recorded observations
/// carries evidence that judges Never Observed.
pub async fn load(
    conn: &mut AnyConnection,
    node_ids: &[&str],
) -> Result<HashMap<String, CurrencyEvidence>, sqlx::Error> {
    let mut evidence: HashMap<String, CurrencyEvidence> = node_ids
        .iter()
        .map(|node_id| ((*node_id).to_owned(), CurrencyEvidence::default()))
        .collect();
    for chunk in node_ids.chunks(LOAD_CHUNK) {
        // A row written before ingestion recorded observation instants keeps
        // its receipt instant, the only instant the Server holds for it.
        let sql = crate::database::number_placeholders(&format!(
            "SELECT node_id, component_key, state, COALESCE(observation_at, received_at) FROM component_status WHERE scope = 'node' AND component_key IN ('rpc', 'sync', 'consensus') AND node_id IN ({})",
            crate::database::placeholder_list(chunk.len())
        ));
        let mut query = sqlx::query_as::<_, (String, String, String, Option<String>)>(&sql);
        for node_id in chunk {
            query = query.bind(*node_id);
        }
        for (node_id, key, state, instant) in query.fetch_all(&mut *conn).await? {
            let (Some(node), Some(component)) =
                (evidence.get_mut(&node_id), GatingComponent::from_key(&key))
            else {
                continue;
            };
            *node.component_mut(component) = ComponentEvidence {
                state: Some(state),
                observed_at: instant.as_deref().and_then(crate::auth::parse_rfc3339),
            };
        }
    }
    Ok(evidence)
}

/// What the Server knows about a Node's network identity, for its Health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeIdentity {
    /// No observation contradicts the registered Network.
    NotContradicted,
    /// A complete successful identity observation contradicts the registered
    /// Network.
    Mismatched,
    /// The newest identity observation failed.
    ObservationFailed,
}

impl NodeIdentity {
    /// Judge the identity disposition from the `network_identity` component
    /// state and whether its complete successful observation contradicts the
    /// registered Network.
    pub fn from_observation(component_state: Option<&str>, contradicts_registry: bool) -> Self {
        match component_state {
            Some("error") => Self::ObservationFailed,
            Some("ok") if contradicts_registry => Self::Mismatched,
            _ => Self::NotContradicted,
        }
    }
}

/// The display severity of a Node Health Summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeHealth {
    Healthy,
    Unhealthy,
    Unknown,
}

impl NodeHealth {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Unhealthy => "unhealthy",
            Self::Unknown => "unknown",
        }
    }
}

/// A failure named in the reason of a Node whose evidence went stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastFailure {
    ProcessFailed,
    Observation(GatingComponent),
}

/// Why a Node has the Health it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthReason {
    Retired,
    /// No gating observation of the named component was ever accepted, or of
    /// none of them when `None`.
    AwaitingFirstObservation(Option<GatingComponent>),
    Stale(Option<LastFailure>),
    ProcessFailed,
    ObservationFailed(GatingComponent),
    IdentityMismatch,
    IdentityObservationFailed,
    Current,
    Unsupported,
}

impl HealthReason {
    pub fn text(self) -> &'static str {
        match self {
            Self::Retired => "node is retired",
            Self::AwaitingFirstObservation(None) => {
                "Node is awaiting its first accepted observation"
            }
            Self::AwaitingFirstObservation(Some(GatingComponent::Rpc)) => {
                "Node is awaiting its first accepted RPC observation"
            }
            Self::AwaitingFirstObservation(Some(GatingComponent::Sync)) => {
                "Node is awaiting its first accepted sync observation"
            }
            Self::AwaitingFirstObservation(Some(GatingComponent::Consensus)) => {
                "Node is awaiting its first accepted consensus observation"
            }
            Self::Stale(None) => "observations are stale",
            Self::Stale(Some(LastFailure::ProcessFailed)) => {
                "observations are stale; last process observation failed"
            }
            Self::Stale(Some(LastFailure::Observation(GatingComponent::Rpc))) => {
                "observations are stale; last RPC observation failed"
            }
            Self::Stale(Some(LastFailure::Observation(GatingComponent::Sync))) => {
                "observations are stale; last sync observation failed"
            }
            Self::Stale(Some(LastFailure::Observation(GatingComponent::Consensus))) => {
                "observations are stale; last consensus observation failed"
            }
            Self::ProcessFailed => "process observation failed",
            Self::ObservationFailed(GatingComponent::Rpc) => "RPC observation failed",
            Self::ObservationFailed(GatingComponent::Sync) => "sync observation failed",
            Self::ObservationFailed(GatingComponent::Consensus) => "consensus observation failed",
            Self::IdentityMismatch => "observed network identity mismatches the registered Network",
            Self::IdentityObservationFailed => "network identity observation failed",
            Self::Current => "RPC, sync, and consensus are current",
            Self::Unsupported => "one or more observations are unknown or unsupported",
        }
    }
}

/// A Node Health Summary: the severity and the reason that decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthSummary {
    pub health: NodeHealth,
    pub reason: HealthReason,
}

/// Derive the Node Health Summary.
///
/// `process` is the `process` component state, `None` when the Node has no
/// process row. Currency is judged before any failure: evidence that is not
/// Current never makes a Node Healthy or Unhealthy, so a failure seen only in
/// stale evidence leaves it Unknown with that failure named in the reason.
pub fn health_summary(
    lifecycle: &str,
    process: Option<&str>,
    identity: NodeIdentity,
    evidence: &CurrencyEvidence,
    now: OffsetDateTime,
) -> HealthSummary {
    let unknown = |reason| HealthSummary {
        health: NodeHealth::Unknown,
        reason,
    };
    let unhealthy = |reason| HealthSummary {
        health: NodeHealth::Unhealthy,
        reason,
    };
    if lifecycle == "retired" {
        return unknown(HealthReason::Retired);
    }
    let failed = |component| evidence.state(component) == Some("error");
    match evidence.currency(now) {
        NodeCurrency::Current => {}
        NodeCurrency::NeverObserved => {
            let missing = |component| evidence.observed_at(component).is_none();
            let awaited = if GatingComponent::ALL.into_iter().all(missing) {
                None
            } else {
                GatingComponent::ALL
                    .into_iter()
                    .find(|component| missing(*component))
            };
            return unknown(HealthReason::AwaitingFirstObservation(awaited));
        }
        NodeCurrency::Stale { .. } => {
            let last_failure = match process {
                Some("error") => Some(LastFailure::ProcessFailed),
                _ => GatingComponent::ALL
                    .into_iter()
                    .find(|component| failed(*component))
                    .map(LastFailure::Observation),
            };
            return unknown(HealthReason::Stale(last_failure));
        }
    }
    // Process monitoring is optional: only a failed process observation
    // counts; a disabled, unsupported or absent process component does not.
    if process == Some("error") {
        return unhealthy(HealthReason::ProcessFailed);
    }
    if failed(GatingComponent::Rpc) {
        return unhealthy(HealthReason::ObservationFailed(GatingComponent::Rpc));
    }
    match identity {
        NodeIdentity::Mismatched => return unknown(HealthReason::IdentityMismatch),
        NodeIdentity::ObservationFailed => return unknown(HealthReason::IdentityObservationFailed),
        NodeIdentity::NotContradicted => {}
    }
    for component in [GatingComponent::Sync, GatingComponent::Consensus] {
        if failed(component) {
            return unhealthy(HealthReason::ObservationFailed(component));
        }
    }
    if GatingComponent::ALL
        .into_iter()
        .all(|component| evidence.state(component) == Some("ok"))
    {
        HealthSummary {
            health: NodeHealth::Healthy,
            reason: HealthReason::Current,
        }
    } else {
        unknown(HealthReason::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use time::Duration;

    use GatingComponent::{Consensus, Rpc, Sync};

    fn now() -> OffsetDateTime {
        crate::auth::parse_rfc3339("2026-11-06T12:00:00Z").expect("valid instant")
    }

    /// When a component was last observed, relative to `now`.
    #[derive(Clone, Copy)]
    enum Seen {
        Fresh,
        Old,
        Future,
        Never,
    }

    fn component(state: &str, seen: Seen) -> ComponentEvidence {
        let observed_at = match seen {
            Seen::Fresh => Some(now() - Duration::seconds(FRESHNESS_LIMIT_SECONDS)),
            Seen::Old => Some(now() - Duration::seconds(FRESHNESS_LIMIT_SECONDS + 1)),
            Seen::Future => Some(now() + Duration::seconds(FRESHNESS_LIMIT_SECONDS + 1)),
            Seen::Never => None,
        };
        ComponentEvidence {
            state: (!matches!(seen, Seen::Never)).then(|| state.to_owned()),
            observed_at,
        }
    }

    fn evidence(
        rpc: (&str, Seen),
        sync: (&str, Seen),
        consensus: (&str, Seen),
    ) -> CurrencyEvidence {
        CurrencyEvidence {
            rpc: component(rpc.0, rpc.1),
            sync: component(sync.0, sync.1),
            consensus: component(consensus.0, consensus.1),
        }
    }

    /// One table row: label, lifecycle, process state, identity, evidence,
    /// and the expected Health with its reason.
    type HealthCase = (
        &'static str,
        &'static str,
        Option<&'static str>,
        NodeIdentity,
        CurrencyEvidence,
        NodeHealth,
        HealthReason,
    );

    const OK: (&str, Seen) = ("ok", Seen::Fresh);
    const FAILED: (&str, Seen) = ("error", Seen::Fresh);

    #[test]
    fn health_summary_table() {
        use HealthReason as R;
        use NodeHealth::{Healthy, Unhealthy, Unknown};
        use NodeIdentity::{Mismatched, NotContradicted, ObservationFailed};
        let stale = |failure| R::Stale(failure);
        #[rustfmt::skip]
        let cases: Vec<HealthCase> = vec![
            ("all current and ok", "active", None, NotContradicted, evidence(OK, OK, OK), Healthy, R::Current),
            ("disabled process is optional", "active", Some("disabled"), NotContradicted, evidence(OK, OK, OK), Healthy, R::Current),
            ("ok process", "active", Some("ok"), NotContradicted, evidence(OK, OK, OK), Healthy, R::Current),
            ("retired wins over everything", "retired", Some("error"), Mismatched, evidence(FAILED, ("ok", Seen::Old), ("ok", Seen::Never)), Unknown, R::Retired),
            ("process failed", "active", Some("error"), NotContradicted, evidence(OK, OK, OK), Unhealthy, R::ProcessFailed),
            ("starting process is no failure", "active", Some("starting"), NotContradicted, evidence(OK, OK, OK), Healthy, R::Current),
            ("process before RPC", "active", Some("error"), NotContradicted, evidence(FAILED, OK, OK), Unhealthy, R::ProcessFailed),
            ("RPC failed", "active", None, NotContradicted, evidence(FAILED, OK, OK), Unhealthy, R::ObservationFailed(Rpc)),
            ("RPC before identity", "active", None, Mismatched, evidence(FAILED, OK, OK), Unhealthy, R::ObservationFailed(Rpc)),
            ("identity mismatch", "active", None, Mismatched, evidence(OK, OK, OK), Unknown, R::IdentityMismatch),
            ("identity before sync", "active", None, Mismatched, evidence(OK, FAILED, OK), Unknown, R::IdentityMismatch),
            ("identity observation failed", "active", None, ObservationFailed, evidence(OK, OK, OK), Unknown, R::IdentityObservationFailed),
            ("sync failed", "active", None, NotContradicted, evidence(OK, FAILED, OK), Unhealthy, R::ObservationFailed(Sync)),
            ("consensus failed", "active", None, NotContradicted, evidence(OK, OK, FAILED), Unhealthy, R::ObservationFailed(Consensus)),
            ("unsupported consensus", "active", None, NotContradicted, evidence(OK, OK, ("unsupported", Seen::Fresh)), Unknown, R::Unsupported),
            ("starting sync", "active", None, NotContradicted, evidence(OK, ("starting", Seen::Fresh), OK), Unknown, R::Unsupported),
            ("stale sync", "active", None, NotContradicted, evidence(OK, ("ok", Seen::Old), OK), Unknown, stale(None)),
            ("stale consensus", "active", Some("disabled"), NotContradicted, evidence(OK, OK, ("ok", Seen::Old)), Unknown, stale(None)),
            ("observed in the future", "active", None, NotContradicted, evidence(("ok", Seen::Future), OK, OK), Unknown, stale(None)),
            ("stale evidence hides an RPC failure", "active", None, NotContradicted, evidence(("error", Seen::Old), ("ok", Seen::Old), ("ok", Seen::Old)), Unknown, stale(Some(LastFailure::Observation(Rpc)))),
            ("stale evidence names a current failure", "active", None, NotContradicted, evidence(OK, FAILED, ("ok", Seen::Old)), Unknown, stale(Some(LastFailure::Observation(Sync)))),
            ("stale evidence names a consensus failure", "active", None, NotContradicted, evidence(("ok", Seen::Old), OK, FAILED), Unknown, stale(Some(LastFailure::Observation(Consensus)))),
            ("stale evidence names the process before RPC", "active", Some("error"), NotContradicted, evidence(FAILED, ("ok", Seen::Old), OK), Unknown, stale(Some(LastFailure::ProcessFailed))),
            ("stale evidence names a failed process", "active", Some("error"), NotContradicted, evidence(OK, ("ok", Seen::Old), OK), Unknown, stale(Some(LastFailure::ProcessFailed))),
            ("stale evidence ignores identity", "active", None, Mismatched, evidence(OK, ("ok", Seen::Old), OK), Unknown, stale(None)),
            ("never observed", "active", None, NotContradicted, CurrencyEvidence::default(), Unknown, R::AwaitingFirstObservation(None)),
            ("never observed consensus", "active", Some("error"), NotContradicted, evidence(FAILED, OK, ("ok", Seen::Never)), Unknown, R::AwaitingFirstObservation(Some(GatingComponent::Consensus))),
            ("stale wins over never observed", "active", None, NotContradicted, evidence(("ok", Seen::Never), ("ok", Seen::Old), OK), Unknown, stale(None)),
        ];
        for (name, lifecycle, process, identity, evidence, health, reason) in cases {
            assert_eq!(
                health_summary(lifecycle, process, identity, &evidence, now()),
                HealthSummary { health, reason },
                "{name}"
            );
        }
    }

    #[test]
    fn health_reasons_read_as_observation_wording() {
        assert_eq!(
            HealthReason::Stale(Some(LastFailure::Observation(Rpc))).text(),
            "observations are stale; last RPC observation failed"
        );
        assert_eq!(
            HealthReason::AwaitingFirstObservation(None).text(),
            "Node is awaiting its first accepted observation"
        );
        assert_eq!(NodeHealth::Unhealthy.as_str(), "unhealthy");
    }

    #[test]
    fn currency_is_judged_from_the_three_gating_instants() {
        assert_eq!(evidence(OK, OK, OK).currency(now()), NodeCurrency::Current);
        assert_eq!(
            evidence(OK, ("error", Seen::Old), OK).currency(now()),
            NodeCurrency::Stale {
                component: Sync,
                observed_at: now() - Duration::seconds(FRESHNESS_LIMIT_SECONDS + 1),
            }
        );
        assert_eq!(
            evidence(OK, OK, ("ok", Seen::Never)).currency(now()),
            NodeCurrency::NeverObserved
        );
        assert_eq!(
            CurrencyEvidence::default().currency(now()),
            NodeCurrency::NeverObserved
        );
    }

    #[test]
    fn only_stale_evidence_blocks_a_lag_figure() {
        assert!(!NodeCurrency::Current.blocks_lag_figure());
        assert!(!NodeCurrency::NeverObserved.blocks_lag_figure());
        assert!(
            evidence(OK, OK, ("ok", Seen::Old))
                .currency(now())
                .blocks_lag_figure()
        );
    }

    #[test]
    fn currency_is_as_of_the_oldest_instant() {
        let mut node = evidence(OK, ("ok", Seen::Old), OK);
        node.consensus.observed_at = Some(now());
        assert_eq!(node.as_of().as_deref(), Some("2026-11-06T11:57:59Z"));
        node.rpc = ComponentEvidence::default();
        assert_eq!(node.as_of(), None);
    }

    #[test]
    fn identity_disposition_needs_a_successful_contradicting_observation() {
        use NodeIdentity::{Mismatched, NotContradicted, ObservationFailed};
        for (state, contradicts, expected) in [
            (Some("ok"), true, Mismatched),
            (Some("ok"), false, NotContradicted),
            (Some("error"), true, ObservationFailed),
            (Some("error"), false, ObservationFailed),
            (Some("disabled"), true, NotContradicted),
            (Some("starting"), false, NotContradicted),
            (None, true, NotContradicted),
        ] {
            assert_eq!(
                NodeIdentity::from_observation(state, contradicts),
                expected,
                "{state:?} {contradicts}"
            );
        }
    }

    #[tokio::test]
    async fn load_reads_every_requested_node_with_the_receipt_fallback() {
        let (_dir, database) = crate::test_support::test_database().await;
        let mut conn = database.pool().acquire().await.unwrap();
        for statement in [
            "INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('mainnet', 'Mainnet', '0xgenesis', 1, 1, 'lat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            "INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-1', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            "INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, inventory_revision, first_seen_at, updated_at) VALUES ('node-a', 'agent-1', 'mainnet', 'ws://127.0.0.1:1', 'active', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            "INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, inventory_revision, first_seen_at, updated_at) VALUES ('node-b', 'agent-1', 'mainnet', 'ws://127.0.0.1:2', 'active', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            "INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, inventory_revision, first_seen_at, updated_at) VALUES ('node-c', 'agent-1', 'mainnet', 'ws://127.0.0.1:3', 'active', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, received_at, observation_at, state_revision, value_revision) VALUES ('agent-1', 'node', 'node-a', 'node-a', 'rpc', 'error', '2026-11-06T12:00:09Z', '2026-11-06T12:00:01Z', 1, 0)",
            "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, received_at, state_revision, value_revision) VALUES ('agent-1', 'node', 'node-a', 'node-a', 'sync', 'ok', '2026-11-06T12:00:02Z', 1, 1)",
            "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, state_revision, value_revision) VALUES ('agent-1', 'node', 'node-a', 'node-a', 'consensus', 'starting', 1, 0)",
            "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, received_at, observation_at, state_revision, value_revision) VALUES ('agent-1', 'node', 'node-a', 'node-a', 'peers', 'ok', '2026-11-06T12:00:03Z', '2026-11-06T12:00:03Z', 1, 1)",
            "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, received_at, observation_at, state_revision, value_revision) VALUES ('agent-1', 'node', 'node-b', 'node-b', 'rpc', 'ok', '2026-11-06T12:00:04Z', '2026-11-06T12:00:04Z', 1, 1)",
        ] {
            sqlx::query(statement).execute(&mut *conn).await.unwrap();
        }

        let loaded = load(&mut conn, &["node-a", "node-b", "node-missing"])
            .await
            .unwrap();

        let instant = |value: &str| crate::auth::parse_rfc3339(value);
        assert_eq!(loaded.len(), 3);
        assert_eq!(
            loaded["node-a"],
            CurrencyEvidence {
                rpc: ComponentEvidence {
                    state: Some("error".to_owned()),
                    observed_at: instant("2026-11-06T12:00:01Z"),
                },
                sync: ComponentEvidence {
                    state: Some("ok".to_owned()),
                    observed_at: instant("2026-11-06T12:00:02Z"),
                },
                consensus: ComponentEvidence {
                    state: Some("starting".to_owned()),
                    observed_at: None,
                },
            }
        );
        assert_eq!(
            loaded["node-b"].rpc.observed_at,
            instant("2026-11-06T12:00:04Z")
        );
        assert_eq!(loaded["node-b"].sync, ComponentEvidence::default());
        assert_eq!(loaded["node-missing"], CurrencyEvidence::default());
        assert!(load(&mut conn, &[]).await.unwrap().is_empty());
    }
}
