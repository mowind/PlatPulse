//! The Server clock and the Server-clock instants of a Report's observations
//! (ADR 0014).

use platpulse_core::Rfc3339;

pub(super) fn now() -> Rfc3339 {
    crate::auth::format_rfc3339(crate::auth::now_utc())
        .parse()
        .expect("formatted timestamp is valid")
}

/// The instant a Report reached the Server, as the recorded state log stamps it.
///
/// The ingestion stamps its own receipt instant in the canonical shape, so
/// parsing it back is exact. The Report's generation instant is the fallback,
/// which keeps the write path total instead of panicking on a caller that
/// passes something else.
pub(super) fn received_instant(received_at: &str, generated_at: Rfc3339) -> Rfc3339 {
    received_at.parse().unwrap_or(generated_at)
}

/// The two instants one Agent Report carries, as one value.
///
/// `received` is the Server's own instant: it answers Agent liveness,
/// idempotency and audit, and nothing about when a Node was actually observed.
/// `skew_ms` is the offset the same Report measured against Server time, which
/// is what turns the Report's own observation instants into Server-clock
/// instants (ADR 0014, issue #274). Every write that records "when was this
/// observed" takes the corrected instant from here instead of the receipt
/// instant, so a replayed Report cannot look newly observed.
#[derive(Clone, Copy)]
pub(super) struct ReportStamp<'a> {
    received: &'a str,
    skew_ms: Option<i64>,
}

impl<'a> ReportStamp<'a> {
    /// The Report's own `host.clock_skew` reading is kept as the Agent reported
    /// it: [`crate::observation_time::observation_instant`] is the one place that
    /// decides whether it may be corrected by.
    pub(super) fn new(received: &'a str, clock_skew_ms: Option<i64>) -> Self {
        Self {
            received,
            skew_ms: clock_skew_ms,
        }
    }

    /// The Server receipt instant of this Report.
    pub(super) fn received_at(&self) -> &'a str {
        self.received
    }

    /// The Server-clock instant at which `observed` happened.
    ///
    /// An unmeasured or Clock Unreliable skew, and a Report that carries no
    /// observation instant, both fall back to the receipt instant.
    pub(super) fn observed_at(&self, observed: Option<Rfc3339>) -> String {
        crate::observation_time::observation_instant(observed, self.skew_ms, self.received)
    }

    /// The Server-clock instant to stamp a history fact derived from `observed`.
    ///
    /// The same correction as [`Self::observed_at`], except that an
    /// untrustworthy skew leaves the Agent's own instant in place: a history row
    /// records when the Node did something, so it can never borrow the instant
    /// the Server happened to receive the Report.
    pub(super) fn history_at(&self, observed: Rfc3339) -> Rfc3339 {
        crate::observation_time::history_instant(observed, self.skew_ms)
    }

    /// One metric sample this Report delivers to `scope`: identified by the
    /// Agent's own instant and placed on the Server timeline by
    /// [`Self::history_at`].
    pub(super) fn metric_delivery<'s>(
        &self,
        scope: crate::metric_history::SeriesScope<'s>,
        observed: Rfc3339,
        value: f64,
    ) -> crate::metric_history::MetricDelivery<'s>
    where
        'a: 's,
    {
        crate::metric_history::MetricDelivery {
            scope,
            identity: observed,
            timeline: self.history_at(observed),
            received_at: self.received,
            value,
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::http::report_ingestion::test_helpers::*;

    use crate::auth::{format_rfc3339, now_utc};

    use crate::http::AppState;

    use platpulse_core::{NodeCurrentDisposition, ReceiptDisposition};

    // -----------------------------------------------------------------------
    // Issue #274 / ADR 0014: freshness, history and alert evidence are judged on
    // the instant an observation was taken, never on the instant its Report was
    // received.
    // -----------------------------------------------------------------------

    pub(super) const ISSUE_274_NODE: &str = "0195f2a1-0014-4014-8014-000000000014";

    /// A Report taken `offset` from now on the Agent's own clock, whose clock
    /// reads `clock_skew_ms` ahead of Server time (a negative reading is a clock
    /// that runs behind).
    ///
    /// The Report carries a successful consensus observation because a Node's
    /// freshness is the oldest of its RPC, sync and consensus observation
    /// instants, and the minimal fixture reports consensus as `unsupported`.
    pub(super) fn report_observed(offset: time::Duration, clock_skew_ms: i64) -> serde_json::Value {
        let mut raw: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        restamp_fixture(&mut raw, now_utc() + offset);
        let observed = raw["nodes"][0]["chain"]["sync"]["latest_observed_at"].clone();
        raw["host"]["clock_skew"]["latest"] = serde_json::json!(clock_skew_ms);
        raw["nodes"][0]["chain"]["consensus"] = serde_json::json!({
            "status": "ok",
            "attempted_at": observed.clone(),
            "latest_observed_at": observed,
            "state_revision": 1,
            "value_revision": 1,
            "latest": {
                "epoch": 100,
                "view_number": 500,
                "validator": false,
                "highest_qc_block": 9007199254740993u64,
                "highest_lock_block": 9007199254740992u64,
                "highest_commit_block": 9007199254740991u64
            }
        });
        raw
    }

    /// The instant a Report says its sync observation happened, as the Report
    /// carries it.
    pub(super) fn report_instant(report: &serde_json::Value) -> String {
        report["nodes"][0]["chain"]["sync"]["latest_observed_at"]
            .as_str()
            .expect("the report stamps its sync observation")
            .to_owned()
    }

    /// The Server-clock instant stored for an instant an Agent clock
    /// `skew_ms` ahead of Server time reported: corrected by the measured skew and
    /// truncated to a second, the way every stored instant is.
    pub(super) fn server_instant(observed: &str, skew_ms: i64) -> String {
        clock_text(
            crate::auth::parse_rfc3339(observed).expect("the report instant parses")
                - time::Duration::milliseconds(skew_ms),
        )
    }

    /// An instant exactly as the Server stores one.
    pub(super) fn clock_text(at: time::OffsetDateTime) -> String {
        format_rfc3339(at.replace_nanosecond(0).unwrap())
    }

    pub(super) fn seconds_from_now(instant: &str) -> i64 {
        let instant = crate::auth::parse_rfc3339(instant).expect("the stored instant parses");
        (instant - now_utc()).whole_seconds()
    }

    /// The stored observation instant of one Node component.
    pub(super) async fn component_instant(state: &AppState, component: &str) -> Option<String> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT observation_at FROM component_status WHERE node_id=$1 AND component_key=$2",
        )
        .bind(ISSUE_274_NODE)
        .bind(component)
        .fetch_optional(state.db().pool())
        .await
        .unwrap();
        row.and_then(|(instant,)| instant)
    }

    /// The evaluation fact a Node rule extracts from the stored projections.
    pub(super) async fn extracted(state: &AppState, rule_key: &str) -> crate::alerts::EvalInput {
        let mut connection = state.db().pool().acquire().await.unwrap();
        crate::alerts::extract_input(
            &mut connection,
            rule_key,
            crate::alerts::SubjectKind::Node,
            ISSUE_274_NODE,
            now_utc(),
            120,
        )
        .await
        .unwrap()
    }

    /// One Peer of a Peer Snapshot, as the Agent reports it.
    pub(super) fn peer_current(
        peer_id: &str,
        remote_ip: &str,
        direction: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "peer_id": peer_id,
            "remote_ip": remote_ip,
            "direction": direction,
            "trusted": false,
            "static_peer": false,
            "consensus_peer": false,
            "caps": [],
        })
    }

    /// One Peer Snapshot of a spool being replayed: observed at `observed` on an
    /// Agent clock that agrees with the Server, delivered now.
    pub(super) fn replayed_peer_report(
        report: (&str, i64),
        revision: i64,
        observed: &str,
        peers: serde_json::Value,
    ) -> Vec<u8> {
        replayed_peer_report_with_skew(report, revision, observed, peers, 0)
    }

    /// The same replayed Peer Snapshot, on an Agent clock whose measured skew is
    /// `clock_skew_ms` — at or beyond Clock Unreliable when the caller means it.
    pub(super) fn replayed_peer_report_with_skew(
        report: (&str, i64),
        revision: i64,
        observed: &str,
        peers: serde_json::Value,
        clock_skew_ms: i64,
    ) -> Vec<u8> {
        let (report_id, report_sequence) = report;
        let mut value = peer_snapshot_body(peers);
        value["report_id"] = serde_json::json!(report_id);
        value["report_sequence"] = serde_json::json!(report_sequence);
        value["host"]["clock_skew"]["latest"] = serde_json::json!(clock_skew_ms);
        let snapshot = &mut value["nodes"][0]["chain"]["peers"];
        snapshot["attempted_at"] = serde_json::json!(observed);
        snapshot["latest_observed_at"] = serde_json::json!(observed);
        snapshot["state_revision"] = serde_json::json!(revision);
        snapshot["value_revision"] = serde_json::json!(revision);
        serde_json::to_vec(&value).unwrap()
    }

    /// Acceptance of issue #274: a batch of Reports generated two hours ago keeps
    /// the Node Stale from the first ingested Report on — freshness, health, the
    /// Reference Head and the Node alerts all judge the observation instant, never
    /// the receipt instant.
    #[tokio::test]
    pub(super) async fn a_replayed_backlog_keeps_the_node_stale_and_off_the_reference_head() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_observed(time::Duration::hours(-2), 0);
        let observed = report_instant(&report);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&report).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        let stored = component_instant(&state, "sync").await.unwrap();
        assert_eq!(stored, server_instant(&observed, 0));
        assert!(
            seconds_from_now(&stored) < -7_000,
            "the Node was observed two hours ago: {stored}"
        );

        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(node["health"], "unknown", "{node}");
        assert_eq!(node["healthReason"], "observations are stale");
        assert_eq!(node["currency"], "stale", "{node}");
        let as_of = node["currencyAsOf"]
            .as_str()
            .expect("every observation instant is known");
        assert!(
            seconds_from_now(as_of) < -7_000,
            "the currency instant is the observation instant: {as_of}"
        );
        assert!(
            node["networkReferenceHead"].is_null(),
            "a replayed Node is never a source: {node}"
        );
        assert_eq!(node["networkReferenceConfidence"], "unknown");
        let reference: (Option<i64>, String) = sqlx::query_as(
            "SELECT block_number, confidence FROM network_reference_heads WHERE network_key='platon-mainnet'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(reference, (None, "unknown".to_owned()));

        for rule in [
            "node.rpc_unreachable",
            "node.sync_lag",
            "node.block_stalled",
        ] {
            let input = extracted(&state, rule).await;
            assert!(
                matches!(input, crate::alerts::EvalInput::Stale { .. }),
                "{rule} judges the observation instant: {input:?}"
            );
        }
    }

    /// Acceptance of issue #274: a measured skew below the Clock Unreliable
    /// threshold moves the observation onto the Server clock, so a three-minute
    /// skew is corrected by rather than read as the Agent stamped it.
    #[tokio::test]
    pub(super) async fn a_measured_three_minute_skew_corrects_the_freshness_window() {
        let (_dir, state, agent_id) = state_with_agent().await;
        // The observation happened four minutes ago; the Agent clock runs three
        // minutes ahead, so the Report itself stamps it one minute ago. Reading the
        // stamped instant would call a four-minute-old observation current.
        let report = report_observed(time::Duration::minutes(-1), 180_000);
        let observed = report_instant(&report);
        assert!(
            seconds_from_now(&observed) > -120,
            "the stamped instant is inside the freshness window: {observed}"
        );
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&report).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        let stored = component_instant(&state, "sync").await.unwrap();
        assert_eq!(stored, server_instant(&observed, 180_000));
        // The correction is judged against the instant the Report itself
        // stamped, so a full-suite run slow enough to blur wall-clock offsets
        // still fails only on a real regression.
        let judged_at = now_utc();
        let age = |instant: &str| {
            (crate::auth::parse_rfc3339(instant).expect("the stored instant parses") - judged_at)
                .whole_seconds()
        };
        assert_eq!(
            age(&observed) - age(&stored),
            180,
            "the three-minute skew is corrected on the Server clock: {stored}"
        );

        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(
            node["currencyAsOf"].as_str(),
            Some(stored.as_str()),
            "the currency instant is the corrected observation instant"
        );
        assert_eq!(node["currency"], "stale", "{node}");
        assert_eq!(node["health"], "unknown", "{node}");
        assert_eq!(node["healthReason"], "observations are stale");
    }

    /// Acceptance of issue #274: a Node is a source for the Observed Network Head
    /// only while every component's observation instant is fresh. A Node whose RPC
    /// observation is two hours old is Stale to the Public projection, so a fresh
    /// sync observation must not make it hold the head.
    #[tokio::test]
    pub(super) async fn a_mixed_age_node_is_not_a_reference_head_source() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_observed(time::Duration::ZERO, 0);
        // Sync and consensus were observed just now; RPC still holds the value of
        // an observation taken two hours ago.
        let old_rpc_at = clock_text(now_utc() - time::Duration::hours(2));
        report["nodes"][0]["chain"]["rpc"]["attempted_at"] = serde_json::json!(old_rpc_at);
        report["nodes"][0]["chain"]["rpc"]["latest_observed_at"] = serde_json::json!(old_rpc_at);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&report).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        assert_eq!(
            component_instant(&state, "rpc").await.as_deref(),
            Some(old_rpc_at.as_str())
        );
        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(
            node["currencyAsOf"].as_str(),
            Some(old_rpc_at.as_str()),
            "the currency instant is the oldest of the three observation instants: {node}"
        );
        assert_eq!(node["currency"], "stale", "{node}");
        assert_eq!(node["health"], "unknown", "{node}");
        assert!(
            node["networkReferenceHead"].is_null(),
            "a partially stale Node is never a source: {node}"
        );
        assert_eq!(node["networkReferenceConfidence"], "unknown");
        let reference: (Option<i64>, String) = sqlx::query_as(
            "SELECT block_number, confidence FROM network_reference_heads WHERE network_key='platon-mainnet'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(reference, (None, "unknown".to_owned()));
    }

    /// Two Agents reporting fresh Nodes of one Network at the same instant
    /// must leave a Reference Head both Reports' sources stand behind (issue
    /// #280). Each receipt recomputes the head from the Nodes it can see; on
    /// the PostgreSQL work connections neither saw the other's uncommitted
    /// Node, so whichever committed last overwrote the head with a snapshot
    /// that counted one source where the Network had two.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    pub(super) async fn concurrent_reports_of_one_network_leave_a_head_counting_every_source() {
        const SECOND_AGENT: &str = "0195f2a1-0019-4019-8019-000000000019";
        const SECOND_NODE: &str = "0195f2a1-0015-4015-8015-000000000015";
        for round in 0..6 {
            let (_dir, state, first_agent) = state_with_agent().await;
            sqlx::query("INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES ($1, 1, NULL, NULL, NULL, $2, $3)")
                .bind(SECOND_AGENT)
                .bind("2026-08-12T08:00:00Z")
                .bind("2026-08-12T08:00:00Z")
                .execute(state.db().pool())
                .await
                .unwrap();
            let first_report = report_observed(time::Duration::ZERO, 0);
            let mut second_report = report_observed(time::Duration::ZERO, 0);
            second_report["agent_id"] = serde_json::json!(SECOND_AGENT);
            second_report["report_id"] = serde_json::json!("0195f2a1-0032-4032-8032-000000000032");
            second_report["inventory"]["nodes"][0]["node_id"] = serde_json::json!(SECOND_NODE);
            second_report["inventory"]["nodes"][0]["rpc_endpoint"] =
                serde_json::json!("ws://127.0.0.1:6791");
            for section in ["nodes", "block_summaries", "history_gaps"] {
                if let Some(entries) = second_report[section].as_array_mut() {
                    for entry in entries {
                        entry["node_id"] = serde_json::json!(SECOND_NODE);
                    }
                }
            }

            let first = tokio::spawn({
                let state = state.clone();
                let agent = first_agent.clone();
                let body = serde_json::to_vec(&first_report).unwrap();
                async move { submit(&state, &agent, body).await }
            });
            let second = tokio::spawn({
                let state = state.clone();
                let body = serde_json::to_vec(&second_report).unwrap();
                async move { submit(&state, SECOND_AGENT, body).await }
            });
            for receipt in [first.await.unwrap(), second.await.unwrap()] {
                assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
                assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Accepted);
            }

            let reference: (Option<i64>, String, i64) = sqlx::query_as(
                "SELECT block_number, confidence, eligible_source_count FROM network_reference_heads WHERE network_key='platon-mainnet'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap();
            assert!(reference.0.is_some(), "round {round}: {reference:?}");
            assert_eq!(
                (reference.1.as_str(), reference.2),
                ("high", 2),
                "round {round}: the head counts both fresh sources"
            );
        }
    }

    /// Acceptance of issue #274: the freshness window is symmetric, so a Node
    /// whose observation instants lie in the future is not fresh either and must
    /// not hold the Observed Network Head.
    #[tokio::test]
    pub(super) async fn a_future_stamped_node_is_not_a_reference_head_source() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_observed(time::Duration::hours(2), 0);
        let observed = report_instant(&report);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&report).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        let stored = component_instant(&state, "sync").await.unwrap();
        assert_eq!(stored, server_instant(&observed, 0));
        assert!(
            seconds_from_now(&stored) > 7_000,
            "the Node claims to be observed two hours from now: {stored}"
        );
        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(node["health"], "unknown", "{node}");
        assert!(
            node["networkReferenceHead"].is_null(),
            "an instant in the future is not a fresh observation: {node}"
        );
        assert_eq!(node["networkReferenceConfidence"], "unknown");
        let reference: (Option<i64>, String) = sqlx::query_as(
            "SELECT block_number, confidence FROM network_reference_heads WHERE network_key='platon-mainnet'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(reference, (None, "unknown".to_owned()));
    }

    /// Acceptance of issue #274: a Clock Unreliable reading is not corrected by.
    /// The Agent's clock is unusable, so the receipt instant is the only instant
    /// the Server can defend, exactly as before ADR 0014.
    #[tokio::test]
    pub(super) async fn an_unreliable_clock_falls_back_to_the_receipt_instant() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let report = report_observed(time::Duration::hours(-2), 600_000);
        let observed = report_instant(&report);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&report).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        let stored = component_instant(&state, "sync").await.unwrap();
        assert_ne!(
            stored,
            server_instant(&observed, 600_000),
            "an untrustworthy skew is not corrected by"
        );
        assert!(
            seconds_from_now(&stored).abs() <= 60,
            "the receipt instant stands in: {stored}"
        );
        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(node["health"], "healthy", "{node}");
    }

    /// Acceptance of issue #274: the Peer presence interval a replayed snapshot
    /// opens is stamped with the instant its sample was observed, so a two-hour
    /// catch-up writes two hours of history where it belongs instead of squeezing
    /// it into the catch-up window.
    #[tokio::test]
    pub(super) async fn a_replayed_peer_snapshot_stamps_history_with_the_observation_instant() {
        let (_dir, state, agent_id) = state_with_agent().await;
        // One spool, filled two hours and one hour ago, delivered in one catch-up
        // now: the baseline the Server holds no predecessor for, then an arrival.
        let baseline_at = clock_text(now_utc() - time::Duration::hours(2));
        let arrival_at = clock_text(now_utc() - time::Duration::hours(1));
        let baseline = replayed_peer_report(
            ("0195f2a1-0274-4274-8274-000000000001", 1),
            1,
            &baseline_at,
            serde_json::json!([peer_current("peer-a", "8.8.8.8", "inbound")]),
        );
        let arrival = replayed_peer_report(
            ("0195f2a1-0274-4274-8274-000000000002", 2),
            2,
            &arrival_at,
            serde_json::json!([
                peer_current("peer-a", "8.8.8.8", "inbound"),
                peer_current("peer-b", "192.0.2.10", "outbound"),
            ]),
        );
        for body in [baseline, arrival] {
            assert_eq!(
                submit(&state, &agent_id, body).await.disposition,
                ReceiptDisposition::Accepted
            );
        }

        let intervals: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT peer_id, opened_at, closed_at FROM peer_presence_intervals WHERE node_id=$1 ORDER BY peer_id",
        )
        .bind(ISSUE_274_NODE)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            intervals,
            vec![("peer-b".to_owned(), arrival_at.clone(), None)],
            "the arrival is stamped when it was observed, not when it was delivered"
        );

        let buckets: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT bucket_start, first_observed_at, last_observed_at FROM peer_aggregate_5m WHERE node_id=$1 ORDER BY bucket_start",
        )
        .bind(ISSUE_274_NODE)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            buckets
                .iter()
                .map(|(_, first, _)| first.clone())
                .collect::<Vec<_>>(),
            vec![baseline_at, arrival_at],
            "each replayed snapshot fills the bucket of its own instant: {buckets:?}"
        );
        for (bucket_start, first, last) in &buckets {
            assert_eq!(first, last, "one replayed snapshot is one sample");
            assert!(
                bucket_start.as_str() <= first.as_str(),
                "the bucket {bucket_start} cannot start after its sample {first}"
            );
        }

        // A later snapshot, observed half an hour ago, no longer carries peer-b:
        // the interval closes at the instant the departure was observed, not at
        // the instant its Report arrived.
        let departure_at = clock_text(now_utc() - time::Duration::minutes(30));
        let departure = replayed_peer_report(
            ("0195f2a1-0274-4274-8274-000000000003", 3),
            3,
            &departure_at,
            serde_json::json!([peer_current("peer-a", "8.8.8.8", "inbound")]),
        );
        assert_eq!(
            submit(&state, &agent_id, departure).await.disposition,
            ReceiptDisposition::Accepted
        );
        let closed: Option<String> = sqlx::query_scalar(
            "SELECT closed_at FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-b'",
        )
        .bind(ISSUE_274_NODE)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            closed.as_deref(),
            Some(departure_at.as_str()),
            "a departure closes the interval at the instant it was observed"
        );
    }

    /// Acceptance of issue #274: a replayed Peer Snapshot is stamped with its own
    /// observation instant even when the Agent is Clock Unreliable. Freshness may
    /// fall back to the receipt instant because it answers "is this Node current
    /// right now", but history must not: the presence interval and the aggregate
    /// bucket keep the instant the snapshot names, or a drained backlog piles
    /// hours of history onto the minutes of catch-up.
    #[tokio::test]
    pub(super) async fn a_replayed_peer_snapshot_keeps_its_instant_when_the_clock_is_unreliable() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let baseline_at = clock_text(now_utc() - time::Duration::hours(2));
        let arrival_at = clock_text(now_utc() - time::Duration::hours(1));
        let skew_ms = crate::http::agent::CLOCK_UNRELIABLE_THRESHOLD_MS + 1;
        let mut bodies = Vec::new();
        for (report, revision, observed, peers) in [
            (
                ("0195f2a1-0274-4274-8274-000000000011", 1),
                1,
                baseline_at.as_str(),
                serde_json::json!([peer_current("peer-a", "8.8.8.8", "inbound")]),
            ),
            (
                ("0195f2a1-0274-4274-8274-000000000012", 2),
                2,
                arrival_at.as_str(),
                serde_json::json!([
                    peer_current("peer-a", "8.8.8.8", "inbound"),
                    peer_current("peer-b", "192.0.2.10", "outbound"),
                ]),
            ),
        ] {
            bodies.push(replayed_peer_report_with_skew(
                report, revision, observed, peers, skew_ms,
            ));
        }
        for body in bodies {
            assert_eq!(
                submit(&state, &agent_id, body).await.disposition,
                ReceiptDisposition::Accepted
            );
        }

        let intervals: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT peer_id, opened_at, closed_at FROM peer_presence_intervals WHERE node_id=$1 ORDER BY peer_id",
        )
        .bind(ISSUE_274_NODE)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            intervals,
            vec![("peer-b".to_owned(), arrival_at.clone(), None)],
            "an unusable skew does not move a presence interval onto the receipt instant"
        );

        let buckets: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT bucket_start, first_observed_at, last_observed_at FROM peer_aggregate_5m WHERE node_id=$1 ORDER BY bucket_start",
        )
        .bind(ISSUE_274_NODE)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            buckets
                .iter()
                .map(|(_, first, _)| first.clone())
                .collect::<Vec<_>>(),
            vec![baseline_at, arrival_at],
            "each replayed snapshot fills the bucket of its own instant: {buckets:?}"
        );
        for (bucket_start, first, last) in &buckets {
            assert_eq!(first, last, "one replayed snapshot is one sample");
            assert!(
                bucket_start.as_str() <= first.as_str(),
                "the bucket {bucket_start} cannot start after its sample {first}"
            );
        }

        // The fresh face of the same Report does fall back: a Clock Unreliable
        // Agent cannot say whether its Node is current, so freshness keeps the
        // receipt instant while history keeps the observation instant.
        // The window is loose on purpose: it only has to separate "the receipt
        // instant, a second or so ago" from "the observation instant, an hour
        // ago", and a tight bound would fail this test after a long pause.
        let component = component_instant(&state, "peers").await.unwrap();
        assert!(
            seconds_from_now(&component).abs() <= 900,
            "freshness falls back to the receipt instant: {component}"
        );
    }

    /// Regression test for issue #397: an Agent graceful shutdown produces a
    /// Closing report whose Nodes are copied from the last-good report but whose
    /// Host was previously not merged. Without the fix, clock_skew.latest is
    /// always None for a Closing report, so the Server falls back to the receipt
    /// instant and stamps stale Node observations as current.
    ///
    /// This test submits a report observed two hours ago with a known skew, then
    /// submits a Closing report that re-carries those Node observations (as
    /// build_shutdown_report does) but whose Host carries the last-good clock_skew
    /// (the fixed behaviour). The Server must still treat the Node as Stale after
    /// the Closing.
    #[tokio::test]
    pub(super) async fn closing_report_with_last_good_skew_keeps_the_node_stale() {
        let (_dir, state, agent_id) = state_with_agent().await;

        // First report: observed two hours ago, zero skew.  After ingestion the
        // Node is Stale (observation instant = two hours ago).
        let stale_report = report_observed(time::Duration::hours(-2), 0);
        let stale_observed = report_instant(&stale_report);
        assert_eq!(
            submit(
                &state,
                &agent_id,
                serde_json::to_vec(&stale_report).unwrap()
            )
            .await
            .disposition,
            ReceiptDisposition::Accepted
        );
        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(node["health"], "unknown", "pre-condition: {node}");
        assert_eq!(node["currency"], "stale", "pre-condition: {node}");

        // Build a Closing report that mirrors what a fixed build_shutdown_report
        // produces: Nodes are copied verbatim from the last-good report, and the
        // Host carries the last-good clock_skew (zero here).  The Closing itself
        // is stamped now (generated_at = now, sync observation = two hours ago).
        let mut closing = stale_report.clone();
        // Advance report_id and sequence so the Server treats it as a new report.
        closing["report_id"] = serde_json::json!("0195f2a1-0397-4397-8397-000000000001");
        closing["report_sequence"] = serde_json::json!(2);
        closing["boot_transition"] = serde_json::json!("closing");
        // Restamp generated_at to now so the envelope is fresh; observation
        // instants inside nodes stay two hours old, matching build_shutdown_report.
        let now_text = crate::auth::format_rfc3339(crate::auth::now_utc());
        closing["generated_at"] = serde_json::json!(now_text);
        // Host clock_skew carries the last-good value (zero skew, status ok) —
        // this is the fix: the Server can now correct the observation instants.
        // (Zero skew means the stored instant equals the Agent-stamped instant,
        // so the Node stays two hours old and remains Stale.)

        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&closing).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        // The observation instant for sync must still be the stale one: the Closing
        // did not produce a new observation, only restated the last-good one.
        let stored_after_closing = component_instant(&state, "sync").await.unwrap();
        assert_eq!(
            stored_after_closing, stale_observed,
            "Closing must not move the observation instant forward"
        );
        assert!(
            seconds_from_now(&stored_after_closing) < -7_000,
            "the Node was observed two hours ago: {stored_after_closing}"
        );

        let node = public_home_node(&state, ISSUE_274_NODE).await;
        assert_eq!(
            node["health"], "unknown",
            "Closing with stale observations must not flip the Node to Healthy: {node}"
        );
        assert_eq!(
            node["currency"], "stale",
            "Closing must not flip the Node to current: {node}"
        );
    }
}
