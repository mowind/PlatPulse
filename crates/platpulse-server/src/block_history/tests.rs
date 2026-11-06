//! Block history driven through its two entry points on a real database.
//! `admit`: replay dedup, Gap Backfill admission, coverage intervals, the block
//! tiers, the Server's own History Gaps and History Gap settlement. `compact`:
//! confirming stale gaps, merging intervals and bounded settlement.

use platpulse_core::AgentReport;
use platpulse_core::block::BlockSource::{self, GapBackfill, Subscription};
use platpulse_core::identity::NodeId;
use sqlx::AnyPool;
use tempfile::TempDir;

use super::admission::{BlockAdmission, BlockOutcome, GapOutcome};
use super::identity::observe_block_identity;
use super::{GapSettlement, NodeAdmission, Refusal, admit, compact};
use crate::auth::{format_rfc3339, now_utc};
use crate::database::ServerDatabase;

const GENESIS: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";

/// One sample as (height, block timestamp in ms, observed_at, transactions,
/// source).
type Sample = (u64, u64, String, u64, BlockSource);

/// One block bucket as (block_count, transaction_total, transaction_min,
/// transaction_max, interval_count, interval_min_ms, interval_max_ms).
type BlockBucket = (i64, i64, i64, i64, i64, Option<i64>, Option<i64>);

/// A migrated database holding what admission reads before it writes: the
/// registered Network, the Agent, and the report fixture's Node.
struct History {
    _dir: TempDir,
    database: ServerDatabase,
    node_id: String,
}

impl History {
    async fn new() -> Self {
        let (dir, database) = crate::test_support::test_database().await;
        crate::network::create_network(
            &database,
            "platon-mainnet",
            "PlatON Mainnet",
            GENESIS,
            210425,
            210425,
            "lat",
        )
        .await
        .unwrap();
        let agent_id = "0195f2a1-0011-4011-8011-000000000011";
        let node_id = blocks(&[]).inventory.nodes[0].node_id.to_string();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES ($1, 1, NULL, NULL, NULL, $2, $3)")
            .bind(agent_id).bind("2026-08-12T08:00:00Z").bind("2026-08-12T08:00:00Z")
            .execute(database.pool()).await.unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ($1, $2, 'platon-mainnet', 'ws://127.0.0.1:6790', 'active', 'public', 1, $3, $4)")
            .bind(&node_id).bind(agent_id).bind("2026-08-12T08:00:00Z").bind("2026-08-12T08:00:00Z")
            .execute(database.pool()).await.unwrap();
        Self {
            _dir: dir,
            database,
            node_id,
        }
    }

    fn pool(&self) -> &AnyPool {
        self.database.pool()
    }

    fn node(&self) -> NodeId {
        self.node_id.parse().unwrap()
    }

    /// Admit a Report whose Nodes the fence admitted, and commit.
    async fn admit(&self, report: &AgentReport) -> BlockAdmission {
        self.admit_fenced(report, &NodeAdmission::default()).await
    }

    /// Admit a Report under the given fence verdicts, and commit.
    async fn admit_fenced(&self, report: &AgentReport, nodes: &NodeAdmission) -> BlockAdmission {
        let mut tx = crate::database::begin(self.pool()).await.unwrap();
        let admission = admit(&mut tx, report, nodes, &format_rfc3339(now_utc()), |at| at)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        admission
    }

    /// Run arranging SQL (insert, update or delete) whose only parameter, $1,
    /// is the Node ID.
    async fn execute(&self, sql: &str) {
        sqlx::query(sql)
            .bind(&self.node_id)
            .execute(self.pool())
            .await
            .unwrap();
    }

    /// Open a recoverable gap over `first..=last`, as an earlier Report's
    /// declared gap would have.
    async fn open_gap(&self, first: i64, last: i64) {
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ($1, $2, $3, 'open_recoverable_gap', '2026-08-12T09:00:06Z', '2026-08-12T09:00:06Z')")
            .bind(&self.node_id)
            .bind(first)
            .bind(last)
            .execute(self.pool())
            .await
            .unwrap();
    }

    /// Every Block Coverage interval as (first_height, last_height, status),
    /// ordered by the lower bound.
    async fn coverage(&self) -> Vec<(i64, i64, String)> {
        sqlx::query_as("SELECT first_height, last_height, status FROM block_coverage_intervals WHERE node_id=$1 ORDER BY first_height")
            .bind(&self.node_id)
            .fetch_all(self.pool())
            .await
            .unwrap()
    }

    /// The heights the Node holds a Block Summary for, ascending.
    async fn summary_heights(&self) -> Vec<i64> {
        sqlx::query_scalar(
            "SELECT block_number FROM block_summaries WHERE node_id=$1 ORDER BY block_number",
        )
        .bind(&self.node_id)
        .fetch_all(self.pool())
        .await
        .unwrap()
    }

    /// (cumulative_block_count, cumulative_transaction_count).
    async fn counters(&self) -> (i64, i64) {
        sqlx::query_as("SELECT cumulative_block_count, cumulative_transaction_count FROM block_history_state WHERE node_id=$1")
            .bind(&self.node_id)
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    async fn cumulative_block_count(&self) -> i64 {
        self.counters().await.0
    }

    async fn bucket(&self, grain_seconds: i64, bucket_start: &str) -> Option<BlockBucket> {
        sqlx::query_as("SELECT block_count, transaction_total, transaction_min, transaction_max, interval_count, interval_min_ms, interval_max_ms FROM node_block_aggregates WHERE node_id = $1 AND grain_seconds = $2 AND bucket_start = $3")
            .bind(&self.node_id)
            .bind(grain_seconds)
            .bind(bucket_start)
            .fetch_optional(self.pool())
            .await
            .unwrap()
    }

    async fn bucket_rows(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM node_block_aggregates")
            .fetch_one(self.pool())
            .await
            .unwrap()
    }

    /// Every `unrecoverable_backfill` History Gap as (from_height, to_height,
    /// resolved).
    async fn backfill_gaps(&self) -> Vec<(i64, i64, bool)> {
        let rows: Vec<(i64, i64, Option<String>)> = sqlx::query_as(
            "SELECT from_height, to_height, resolved_at FROM block_history_gaps WHERE node_id=$1 AND kind='unrecoverable_backfill' ORDER BY from_height",
        )
        .bind(&self.node_id)
        .fetch_all(self.pool())
        .await
        .unwrap();
        rows.into_iter()
            .map(|(from, to, resolved)| (from, to, resolved.is_some()))
            .collect()
    }

    /// Every History Gap the Server authored itself as (height, reason).
    async fn server_gaps(&self) -> Vec<(i64, String)> {
        sqlx::query_as("SELECT from_height, reason FROM block_history_gaps WHERE node_id=$1 AND kind='server_rejected' AND authored_by='server' ORDER BY from_height")
            .bind(&self.node_id)
            .fetch_all(self.pool())
            .await
            .unwrap()
    }
}

/// A 5-minute bucket edge an hour before now, so the instants a case writes
/// sit inside the block tiers' own windows.
fn block_base() -> time::OffsetDateTime {
    let hour_ago = now_utc() - time::Duration::hours(1);
    time::OffsetDateTime::from_unix_timestamp(hour_ago.unix_timestamp().div_euclid(300) * 300)
        .unwrap()
}

fn at(base: time::OffsetDateTime, seconds: i64) -> String {
    format_rfc3339(base + time::Duration::seconds(seconds))
}

/// A Report carrying the given Block Summaries for the fixture's Node, each
/// stamped with the registered Network Identity.
fn blocks(samples: &[Sample]) -> AgentReport {
    let mut report: AgentReport = serde_json::from_slice(include_bytes!(
        "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
    ))
    .unwrap();
    let node_id = report.inventory.nodes[0].node_id;
    let mut identity = report.nodes[0]
        .chain
        .network_identity
        .latest
        .clone()
        .unwrap();
    identity.genesis_hash = GENESIS.parse().unwrap();
    identity.address_hrp = Some("lat".to_owned());
    for (height, timestamp_ms, observed_at, transactions, source) in samples.iter().cloned() {
        report
            .block_summaries
            .push(platpulse_core::block::BlockSummary {
                node_id,
                network_identity: identity.clone(),
                block_number: height,
                block_hash: format!("0x{height:064x}").parse().unwrap(),
                parent_hash: format!("0x{:064x}", height.saturating_sub(1))
                    .parse()
                    .unwrap(),
                block_timestamp_ms: timestamp_ms,
                observed_at: observed_at.parse().unwrap(),
                transaction_count: transactions,
                block_interval_ms: None,
                source,
                attribution: platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                    "0x1111111111111111111111111111111111111111"
                        .parse()
                        .unwrap(),
                    "test",
                ),
            });
    }
    report.validate().unwrap();
    report
}

/// A Report carrying the given Block Summaries plus Agent-declared
/// `unrecoverable_backfill` gaps, as one bounded recovery round sends them.
fn blocks_and_gaps(samples: &[Sample], gaps: &[(u64, u64)]) -> AgentReport {
    let mut report = blocks(samples);
    let node_id = report.inventory.nodes[0].node_id;
    for (from_height, to_height) in gaps.iter().copied() {
        report.history_gaps.push(platpulse_core::HistoryGap {
            node_id,
            kind: platpulse_core::GapKind::UnrecoverableBackfill,
            from_height,
            to_height,
            reason: "subscription reconnect exceeded configured backfill bounds".to_owned(),
            recorded_at: report.generated_at,
        });
    }
    report.validate().unwrap();
    report
}

/// The settlement pass a Report ran, without its duration.
fn settlement(admission: &BlockAdmission) -> Option<GapSettlement> {
    admission.settlement.map(|(settlement, _)| settlement)
}

/// An accepted block advances one bucket of each tier, by the minute and five
/// minutes its own observation falls in, with its transactions and its
/// interval to the held parent. A later Report repeating the same heights is
/// a resync replay and moves no bucket.
#[tokio::test]
async fn accepted_blocks_advance_both_block_tiers_and_a_repeat_never_does() {
    let history = History::new().await;
    let base = block_base();
    let report = blocks(&[
        (10, 1_000, at(base, 1), 3, Subscription),
        (11, 3_000, at(base, 3), 5, Subscription),
        (12, 4_500, at(base, 5), 1, Subscription),
        (13, 6_500, at(base, 67), 2, Subscription),
    ]);
    assert_eq!(
        history.admit(&report).await.samples,
        vec![BlockOutcome::Accepted; 4]
    );

    // Three blocks in the first minute: 3 + 5 + 1 transactions, and the two
    // intervals whose parent is held (block 10's parent is not).
    let minute = Some((3, 9, 1, 5, 2, Some(1_500), Some(2_000)));
    assert_eq!(history.bucket(60, &at(base, 0)).await, minute);
    assert_eq!(
        history.bucket(60, &at(base, 60)).await,
        Some((1, 2, 2, 2, 1, Some(2_000), Some(2_000)))
    );
    // The 5-minute bucket counted all four.
    let five = Some((4, 11, 1, 5, 3, Some(1_500), Some(2_000)));
    assert_eq!(history.bucket(300, &at(base, 0)).await, five);
    assert_eq!(
        history.bucket_rows().await,
        3,
        "an empty bucket is never written"
    );

    assert_eq!(
        history.admit(&report).await.samples,
        vec![BlockOutcome::ResyncReplay; 4]
    );
    assert_eq!(history.bucket(60, &at(base, 0)).await, minute);
    assert_eq!(history.bucket(300, &at(base, 0)).await, five);
    assert_eq!(history.bucket_rows().await, 3);
    assert_eq!(history.counters().await, (4, 11));
}

/// A plain resync replay - a subscription block at or below the historical
/// high-water mark whose raw row retention has already released - is refused
/// by Block history dedup, so it neither rewrites the summary nor
/// re-accumulates, even though no raw row is left to collide with.
#[tokio::test]
async fn a_resync_replay_below_the_high_water_mark_never_rewrites_or_accumulates() {
    let history = History::new().await;
    let base = block_base();
    let report = blocks(&[(10, 1_000, at(base, 1), 3, Subscription)]);
    assert_eq!(
        history.admit(&report).await.samples,
        vec![BlockOutcome::Accepted]
    );
    history
        .execute("DELETE FROM block_summaries WHERE node_id=$1")
        .await;
    history
        .execute("UPDATE block_history_state SET historical_high_watermark=100 WHERE node_id=$1")
        .await;

    assert_eq!(
        history.admit(&report).await.samples,
        vec![BlockOutcome::ResyncReplay]
    );
    assert_eq!(history.summary_heights().await, Vec::<i64>::new());
    assert_eq!(history.counters().await, (1, 3));
    let once = Some((1, 3, 3, 3, 0, None, None));
    assert_eq!(history.bucket(60, &at(base, 0)).await, once);
    assert_eq!(history.bucket(300, &at(base, 0)).await, once);
}

/// Only an explicit OpenRecoverableGap admits a block below the high-water
/// mark. Its backfill is counted once, in the bucket of its own observation,
/// and completes the interval of the child that was waiting for it in the
/// child's own bucket; a second delivery of the same backfill and a backfill
/// outside any open gap accumulate nothing.
#[tokio::test]
async fn an_open_gap_backfill_accumulates_once_and_completes_its_child_interval() {
    let history = History::new().await;
    let base = block_base();
    history
        .admit(&blocks(&[
            (10, 1_000, at(base, 1), 3, Subscription),
            (12, 5_000, at(base, 5), 4, Subscription),
        ]))
        .await;
    assert_eq!(
        history.bucket(60, &at(base, 0)).await,
        Some((2, 7, 3, 4, 0, None, None)),
        "neither block has a held parent yet"
    );
    history.open_gap(11, 11).await;

    let backfill = blocks(&[(11, 2_500, at(base, 210), 6, GapBackfill)]);
    assert_eq!(
        history.admit(&backfill).await.samples,
        vec![BlockOutcome::Accepted]
    );
    // Block 11 lands in its own minute with its interval to block 10...
    let own = Some((1, 6, 6, 6, 1, Some(1_500), Some(1_500)));
    assert_eq!(history.bucket(60, &at(base, 180)).await, own);
    // ...and completes block 12's interval in block 12's minute.
    let completed = Some((2, 7, 3, 4, 1, Some(2_500), Some(2_500)));
    assert_eq!(history.bucket(60, &at(base, 0)).await, completed);
    let five = Some((3, 13, 3, 6, 2, Some(1_500), Some(2_500)));
    assert_eq!(history.bucket(300, &at(base, 0)).await, five);

    // The gap is closed now: the same backfill again, and a backfill below
    // the high-water mark outside any open gap, are both refused.
    assert_eq!(
        history.admit(&backfill).await.samples,
        vec![BlockOutcome::OutsideOpenGap]
    );
    let outside = blocks(&[(9, 500, at(base, 220), 9, GapBackfill)]);
    assert_eq!(
        history.admit(&outside).await.samples,
        vec![BlockOutcome::OutsideOpenGap]
    );
    assert_eq!(history.bucket(60, &at(base, 0)).await, completed);
    assert_eq!(history.bucket(60, &at(base, 180)).await, own);
    assert_eq!(history.bucket(300, &at(base, 0)).await, five);
    assert_eq!(history.counters().await, (3, 13));
}

/// A backfill that lands strictly inside an open gap splits the interval, so
/// the still-missing range above it keeps its own bounds instead of being
/// reported covered. The upper half is a copy of the same gap, so it keeps the
/// gap's own `created_at` rather than the receipt time.
#[tokio::test]
async fn a_backfill_inside_an_open_gap_splits_the_interval() {
    let history = History::new().await;
    let base = block_base();
    history
        .admit(&blocks(&[(10, 1_000, at(base, 1), 3, Subscription)]))
        .await;
    history.open_gap(11, 20).await;

    let backfill = blocks(&[(15, 5_000, at(base, 5), 4, GapBackfill)]);
    assert_eq!(
        history.admit(&backfill).await.samples,
        vec![BlockOutcome::Accepted]
    );
    assert_eq!(
        history.coverage().await,
        vec![
            (10, 10, "covered".to_owned()),
            (11, 14, "open_recoverable_gap".to_owned()),
            (15, 15, "covered".to_owned()),
            (16, 20, "open_recoverable_gap".to_owned()),
        ]
    );
    let halves: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT first_height, last_height, created_at FROM block_coverage_intervals WHERE node_id=$1 AND status='open_recoverable_gap' ORDER BY first_height",
    )
    .bind(&history.node_id)
    .fetch_all(history.pool())
    .await
    .unwrap();
    assert_eq!(
        halves,
        vec![
            (11, 14, "2026-08-12T09:00:06Z".to_owned()),
            (16, 20, "2026-08-12T09:00:06Z".to_owned()),
        ],
        "both halves are still the same gap, so they keep its created_at"
    );
}

/// A backfill that sits on an open gap's own edge narrows the interval instead
/// of splitting, duplicating, or dropping it: the backfill at the lower edge
/// and the one at the upper edge each keep the rest of the gap open.
#[tokio::test]
async fn a_backfill_on_an_open_gap_edge_narrows_the_interval() {
    let history = History::new().await;
    let base = block_base();
    history
        .admit(&blocks(&[(10, 1_000, at(base, 1), 3, Subscription)]))
        .await;
    history.open_gap(11, 20).await;

    let lower = blocks(&[(11, 2_500, at(base, 5), 4, GapBackfill)]);
    assert_eq!(
        history.admit(&lower).await.samples,
        vec![BlockOutcome::Accepted]
    );
    assert_eq!(
        history.coverage().await,
        vec![
            (10, 11, "covered".to_owned()),
            (12, 20, "open_recoverable_gap".to_owned()),
        ],
        "closing the gap's first height moves its lower bound up by one and covers it"
    );

    let upper = blocks(&[(20, 12_000, at(base, 9), 5, GapBackfill)]);
    assert_eq!(
        history.admit(&upper).await.samples,
        vec![BlockOutcome::Accepted]
    );
    assert_eq!(
        history.coverage().await,
        vec![
            (10, 11, "covered".to_owned()),
            (12, 19, "open_recoverable_gap".to_owned()),
            (20, 20, "covered".to_owned()),
        ],
        "closing the gap's last height moves its upper bound down by one"
    );
}

/// History Gap settlement is bounded by what the Report touched, not by the
/// size of the tables. A Node carries 10,000 open recoverable intervals, each
/// under an unresolved `unrecoverable_backfill` gap. A live tip Report closes
/// nothing and runs no settlement at all; a backfill of one height examines
/// and resolves only the gap around it; a gap declared over covered heights
/// examines only itself.
#[tokio::test]
async fn gap_settlement_examines_only_the_gaps_a_report_touched() {
    const OPEN_GAPS: i64 = 10_000;
    let history = History::new().await;
    let base = block_base();
    history
        .admit(&blocks(&[
            (10, 10_000, at(base, 1), 1, Subscription),
            (11, 11_000, at(base, 2), 1, Subscription),
            (50_000, 50_000_000, at(base, 3), 1, Subscription),
        ]))
        .await;

    // One single-height open interval and one unresolved gap at every even
    // height from 20,000, as a long-unfilled recovery leaves them.
    let stamp = format_rfc3339(now_utc());
    let pool = history.pool();
    sqlx::query("WITH RECURSIVE seq(k) AS (SELECT 0 UNION ALL SELECT k + 1 FROM seq WHERE k + 1 < $1) INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) SELECT $2, 20000 + 2 * k, 20000 + 2 * k, 'open_recoverable_gap', $3, $4 FROM seq")
        .bind(OPEN_GAPS).bind(&history.node_id).bind(&stamp).bind(&stamp)
        .execute(pool).await.unwrap();
    sqlx::query("WITH RECURSIVE seq(k) AS (SELECT 0 UNION ALL SELECT k + 1 FROM seq WHERE k + 1 < $1) INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) SELECT $2, 20000 + 2 * k, 20000 + 2 * k, 'unrecoverable_backfill', 'seeded', $3, 'agent' FROM seq")
        .bind(OPEN_GAPS).bind(&history.node_id).bind(&stamp)
        .execute(pool).await.unwrap();
    let unresolved = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM block_history_gaps WHERE node_id=$1 AND kind='unrecoverable_backfill' AND resolved_at IS NULL",
        )
        .bind(&history.node_id)
        .fetch_one(pool)
        .await
        .unwrap()
    };
    assert_eq!(unresolved().await, OPEN_GAPS);

    // A live tip Report touches no open height: no settlement runs.
    let tip = history
        .admit(&blocks(&[(
            50_001,
            50_001_000,
            at(base, 4),
            1,
            Subscription,
        )]))
        .await;
    assert_eq!(tip.samples, vec![BlockOutcome::Accepted]);
    assert_eq!(settlement(&tip), None);
    assert_eq!(unresolved().await, OPEN_GAPS);

    // Backfilling one height settles exactly the gap around it.
    let backfill = history
        .admit(&blocks(&[(
            30_000,
            30_000_000,
            at(base, 5),
            1,
            GapBackfill,
        )]))
        .await;
    assert_eq!(backfill.samples, vec![BlockOutcome::Accepted]);
    assert_eq!(
        settlement(&backfill),
        Some(GapSettlement {
            examined: 1,
            resolved: 1
        })
    );
    assert_eq!(unresolved().await, OPEN_GAPS - 1);
    let settled: Option<String> = sqlx::query_scalar(
        "SELECT resolved_at FROM block_history_gaps WHERE node_id=$1 AND from_height=30000",
    )
    .bind(&history.node_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(settled.is_some(), "the backfilled gap is resolved");

    // A gap declared over heights already covered settles at once, and
    // examines nothing but itself.
    let declared = history
        .admit(&blocks_and_gaps(
            &[(50_002, 50_002_000, at(base, 6), 1, Subscription)],
            &[(10, 11)],
        ))
        .await;
    assert_eq!(declared.samples, vec![BlockOutcome::Accepted]);
    assert_eq!(declared.gaps, vec![GapOutcome::Accepted]);
    assert_eq!(
        settlement(&declared),
        Some(GapSettlement {
            examined: 1,
            resolved: 1
        })
    );
    assert_eq!(unresolved().await, OPEN_GAPS - 1);
}

/// A bounded recovery round backfills the head of a hole and declares the
/// rest a gap, while a live summary has already raised the High-Water Mark
/// past the whole hole. The head of the hole lies in no open gap (the gap is
/// declared in the same Report, after the samples), yet the Server must still
/// admit it as a hole it observes itself; the declared rest is backfilled by
/// a later round; and the hole ends as one covered interval with its History
/// Gap resolved.
#[tokio::test]
async fn a_hole_whose_head_is_outside_any_open_gap_still_backfills() {
    let history = History::new().await;
    let base = block_base();
    history
        .admit(&blocks(&[
            (10, 1_000, at(base, 1), 1, Subscription),
            (11, 2_000, at(base, 2), 1, Subscription),
            (20, 11_000, at(base, 11), 1, Subscription),
        ]))
        .await;

    // The bounded round: heights 12..=14 backfilled, 15..=19 declared.
    let round = history
        .admit(&blocks_and_gaps(
            &[
                (12, 3_000, at(base, 12), 1, GapBackfill),
                (13, 4_000, at(base, 12), 1, GapBackfill),
                (14, 5_000, at(base, 12), 1, GapBackfill),
            ],
            &[(15, 19)],
        ))
        .await;
    assert_eq!(
        round.samples,
        vec![BlockOutcome::Accepted; 3],
        "the head of a Server-observed hole is admitted without an open gap"
    );
    assert_eq!(round.gaps, vec![GapOutcome::Accepted]);
    assert_eq!(
        history.coverage().await,
        vec![
            (10, 14, "covered".to_owned()),
            (15, 19, "open_recoverable_gap".to_owned()),
            (20, 20, "covered".to_owned()),
        ]
    );
    assert_eq!(history.backfill_gaps().await, vec![(15, 19, false)]);

    // A later round backfills the declared rest of the hole.
    let rest: Vec<_> = (15..=19)
        .map(|height| (height, height * 1_000 - 9_000, at(base, 13), 1, GapBackfill))
        .collect();
    assert_eq!(
        history.admit(&blocks(&rest)).await.samples,
        vec![BlockOutcome::Accepted; 5]
    );
    assert_eq!(
        history.coverage().await,
        vec![(10, 20, "covered".to_owned())],
        "a fully backfilled hole is one covered interval"
    );
    assert_eq!(
        history.backfill_gaps().await,
        vec![(15, 19, true)],
        "the History Gap is resolved once nothing of it is open"
    );
    assert_eq!(history.cumulative_block_count().await, 11);

    // A settled height is a replay again, even after its raw row expired:
    // it is never counted twice.
    history
        .execute("DELETE FROM block_summaries WHERE node_id=$1")
        .await;
    let replay = blocks(&[(13, 4_000, at(base, 14), 1, GapBackfill)]);
    assert_eq!(
        history.admit(&replay).await.samples,
        vec![BlockOutcome::OutsideOpenGap]
    );
    assert_eq!(history.cumulative_block_count().await, 11);
}

/// The Server-observed hole is bounded by what the Server itself settled. A
/// height above the High-Water Mark is new history; a height below the first
/// covered height, or one a `server_rejected` gap holds (ADR 0011 decision 3),
/// is never admitted.
#[tokio::test]
async fn a_backfill_is_admitted_only_into_a_hole_the_server_observes() {
    let history = History::new().await;
    let base = block_base();
    history
        .admit(&blocks(&[
            (10, 1_000, at(base, 1), 1, Subscription),
            (20, 11_000, at(base, 11), 1, Subscription),
        ]))
        .await;
    history.execute("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, 15, 15, 'server_rejected', 'identity mismatch', '2026-08-12T09:00:12Z', 'server')").await;
    // The Agent's own copy of an earlier GapBackfillOutsideOpenGap refusal is
    // not the Server's verdict and never makes a hole unrecoverable.
    history.execute("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, 17, 17, 'server_rejected', 'GapBackfill sample is outside an explicit open recoverable gap', '2026-08-12T09:00:12Z', 'agent')").await;

    let probe = blocks(&[
        (9, 500, at(base, 12), 1, GapBackfill),
        (14, 5_000, at(base, 12), 1, GapBackfill),
        (15, 6_000, at(base, 12), 1, GapBackfill),
        (17, 8_000, at(base, 12), 1, GapBackfill),
        (21, 12_000, at(base, 12), 1, GapBackfill),
    ]);
    use BlockOutcome::{Accepted, OutsideOpenGap};
    assert_eq!(
        history.admit(&probe).await.samples,
        vec![OutsideOpenGap, Accepted, OutsideOpenGap, Accepted, Accepted]
    );
    assert_eq!(
        history.coverage().await,
        vec![
            (10, 10, "covered".to_owned()),
            (14, 14, "covered".to_owned()),
            (17, 17, "covered".to_owned()),
            (20, 21, "covered".to_owned()),
        ]
    );
    let head: i64 =
        sqlx::query_scalar("SELECT block_number FROM observed_network_heads WHERE node_id=$1")
            .bind(&history.node_id)
            .fetch_one(history.pool())
            .await
            .unwrap();
    assert_eq!(head, 20, "a backfill never moves the observed network head");

    // A declared range never opens a height the Server refused itself.
    history.admit(&blocks_and_gaps(&[], &[(15, 16)])).await;
    let coverage = history.coverage().await;
    assert!(coverage.contains(&(16, 16, "open_recoverable_gap".to_owned())));
    assert!(
        !coverage
            .iter()
            .any(|(first, last, _)| *first <= 15 && 15 <= *last)
    );
}

/// A height backfilled into a declared gap by an older Server only narrowed
/// the open interval and left no coverage record. Such a height is never a
/// Server-observed hole, and settling its gap records it covered, so it cannot
/// be admitted and counted again once its raw summary expires. A Report
/// settles only the gaps around the heights it closed, so a legacy gap no
/// Report touches is settled by the retention sweep's bounded catch-up walk.
#[tokio::test]
async fn a_height_backfilled_by_an_older_server_is_never_counted_twice() {
    let history = History::new().await;
    let base = block_base();
    history
        .admit(&blocks(&[
            (10, 1_000, at(base, 1), 1, Subscription),
            (20, 11_000, at(base, 11), 1, Subscription),
        ]))
        .await;
    // The legacy shape: gap 12..=14 declared and fully backfilled, so no
    // interval is left for it, and the raw rows have expired.
    history.execute("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ($1, 12, 14, 'unrecoverable_backfill', 'legacy', '2026-08-12T09:00:12Z', 'agent')").await;
    let count = history.cumulative_block_count().await;

    let probe = blocks(&[
        (13, 4_000, at(base, 12), 1, GapBackfill),
        (16, 7_000, at(base, 12), 1, GapBackfill),
    ]);
    assert_eq!(
        history.admit(&probe).await.samples,
        vec![BlockOutcome::OutsideOpenGap, BlockOutcome::Accepted]
    );
    assert_eq!(history.cumulative_block_count().await, count + 1);
    // The Report closed height 16 only, so it leaves the legacy gap alone.
    assert_eq!(history.backfill_gaps().await, vec![(12, 14, false)]);
    // The retention sweep's catch-up walk settles it: its heights are now
    // covered.
    compact(
        history.pool(),
        now_utc(),
        crate::retention::raw_block_summary_cutoff(now_utc()),
        &mut 0,
    )
    .await
    .unwrap();
    assert_eq!(history.backfill_gaps().await, vec![(12, 14, true)]);
    assert!(
        history
            .coverage()
            .await
            .contains(&(12, 14, "covered".to_owned()))
    );
}

/// A declared range that reaches heights the Server already covered opens
/// only its uncovered stretches, so a later backfill can never rewrite an
/// accepted height, and the gap resolves as soon as those stretches are
/// backfilled.
#[tokio::test]
async fn a_declared_gap_opens_only_its_uncovered_heights() {
    let history = History::new().await;
    let base = block_base();
    history
        .admit(&blocks(&[
            (10, 1_000, at(base, 1), 1, Subscription),
            (11, 2_000, at(base, 2), 1, Subscription),
            (14, 5_000, at(base, 5), 1, Subscription),
        ]))
        .await;
    assert_eq!(
        history.admit(&blocks_and_gaps(&[], &[(9, 16)])).await.gaps,
        vec![GapOutcome::Accepted]
    );
    assert_eq!(
        history.coverage().await,
        vec![
            (9, 9, "open_recoverable_gap".to_owned()),
            (10, 11, "covered".to_owned()),
            (12, 13, "open_recoverable_gap".to_owned()),
            (14, 14, "covered".to_owned()),
            (15, 16, "open_recoverable_gap".to_owned()),
        ]
    );
    assert_eq!(history.backfill_gaps().await, vec![(9, 16, false)]);

    let backfill: Vec<_> = [9, 12, 13, 15, 16]
        .into_iter()
        .map(|height| {
            (
                height,
                height * 1_000 - 9_000 + 1,
                at(base, 20),
                1,
                GapBackfill,
            )
        })
        .collect();
    assert_eq!(
        history.admit(&blocks(&backfill)).await.samples,
        vec![BlockOutcome::Accepted; 5]
    );
    assert_eq!(
        history.coverage().await,
        vec![(9, 16, "covered".to_owned())]
    );
    assert_eq!(history.backfill_gaps().await, vec![(9, 16, true)]);
    assert_eq!(history.cumulative_block_count().await, 8);
}

/// Admission writes in the caller's transaction and never swallows a failed
/// statement: when a bucket update fails after the first block was written,
/// rolling the transaction back leaves no bucket and no Block Summary behind.
#[tokio::test]
async fn a_failed_admission_rolls_back_with_its_transaction() {
    let history = History::new().await;
    let base = block_base();
    // The first block opens the bucket; the second one's update aborts.
    crate::test_support::install_failure_trigger(
        history.pool(),
        "abort_block_bucket",
        "BEFORE",
        "UPDATE",
        "node_block_aggregates",
        "injected bucket failure",
    )
    .await
    .unwrap();
    let report = blocks(&[
        (10, 1_000, at(base, 1), 3, Subscription),
        (11, 3_000, at(base, 3), 5, Subscription),
    ]);
    let mut tx = crate::database::begin(history.pool()).await.unwrap();
    let failed = admit(
        &mut tx,
        &report,
        &NodeAdmission::default(),
        &format_rfc3339(now_utc()),
        |at| at,
    )
    .await;
    assert!(failed.is_err(), "the injected failure reaches the caller");
    tx.rollback().await.unwrap();
    assert_eq!(history.bucket_rows().await, 0);
    assert_eq!(history.summary_heights().await, Vec::<i64>::new());

    // Without the injected failure the same Report is accepted once.
    crate::test_support::drop_failure_trigger(
        history.pool(),
        "abort_block_bucket",
        "node_block_aggregates",
    )
    .await
    .unwrap();
    assert_eq!(
        history.admit(&report).await.samples,
        vec![BlockOutcome::Accepted; 2]
    );
    assert_eq!(
        history.bucket(60, &at(base, 0)).await,
        Some((2, 8, 3, 5, 1, Some(2_000), Some(2_000)))
    );
}

#[tokio::test]
async fn repeated_history_gap_declarations_are_deduplicated() {
    let history = History::new().await;
    let declared = blocks_and_gaps(&[], &[(8, 9)]);
    for _ in 0..2 {
        assert_eq!(
            history.admit(&declared).await.gaps,
            vec![GapOutcome::Accepted]
        );
    }
    assert_eq!(history.backfill_gaps().await, vec![(8, 9, false)]);
}

/// The reason of the History Gap the Server records for a refused sample is
/// the reason the Receipt carries for it, for every refusal that leaves a
/// gap; a purged Node's history is gone, so its refusal leaves none. A refused
/// Node's samples and declared gaps are never written.
#[tokio::test]
async fn every_refusal_records_the_receipt_reason_as_its_server_gap() {
    let history = History::new().await;
    let base = block_base();

    let mut owned_elsewhere = NodeAdmission::default();
    owned_elsewhere.refuse_ownership(history.node());
    let refused = history
        .admit_fenced(
            &blocks_and_gaps(&[(10, 1_000, at(base, 1), 3, Subscription)], &[(20, 25)]),
            &owned_elsewhere,
        )
        .await;
    assert_eq!(
        refused.samples,
        vec![BlockOutcome::Refused(Refusal::OwnershipMismatch)]
    );
    assert_eq!(
        refused.gaps,
        vec![GapOutcome::Refused(Refusal::OwnershipMismatch)]
    );
    assert!(!refused.identity_contradicted);

    let mut stamped = blocks(&[(11, 2_000, at(base, 2), 3, Subscription)]);
    stamped.block_summaries[0].network_identity.chain_id = 1;
    let contradicted = history.admit(&stamped).await;
    assert!(matches!(
        contradicted.samples[..],
        [BlockOutcome::Refused(Refusal::IdentityMismatch(_))]
    ));
    assert!(contradicted.identity_contradicted);

    let receipt_reason = |admission: &BlockAdmission| {
        admission.samples[0]
            .rejection()
            .expect("a refused sample is rejected")
            .reason
    };
    assert_eq!(
        history.server_gaps().await,
        vec![
            (10, receipt_reason(&refused)),
            (11, receipt_reason(&contradicted)),
        ]
    );
    assert_eq!(
        receipt_reason(&refused),
        "Node belongs to another Agent",
        "the ownership reason is the one the Receipt has always sent"
    );

    let mut purged = NodeAdmission::default();
    purged.refuse_purged(history.node());
    let erased = history
        .admit_fenced(
            &blocks(&[(12, 3_000, at(base, 3), 3, Subscription)]),
            &purged,
        )
        .await;
    assert_eq!(erased.samples, vec![BlockOutcome::Refused(Refusal::Purged)]);
    assert_eq!(
        history.server_gaps().await.len(),
        2,
        "a purge records no gap"
    );
    assert_eq!(history.summary_heights().await, Vec::<i64>::new());
    assert!(history.backfill_gaps().await.is_empty());
}

/// A subscription sample below a height the same Report already raised the
/// high-water mark to is a resync replay, exactly as it would be in the next
/// Report, instead of an acceptance of a height that is never written.
#[tokio::test]
async fn a_lower_subscription_height_after_a_higher_one_in_one_report_is_a_replay() {
    let history = History::new().await;
    let base = block_base();
    let report = blocks(&[
        (5, 5_000, at(base, 5), 2, Subscription),
        (3, 3_000, at(base, 3), 4, Subscription),
    ]);
    assert_eq!(
        history.admit(&report).await.samples,
        vec![BlockOutcome::Accepted, BlockOutcome::ResyncReplay]
    );
    assert_eq!(history.summary_heights().await, vec![5]);
    assert_eq!(history.counters().await, (1, 2));
}

/// A GapBackfill sample whose hash contradicts the identity evidence the
/// Server retains for its height is a Chain Divergence: it is rejected, only
/// the observation is recorded, once however often it is sent again, and the
/// hole stays open for the right block.
#[tokio::test]
async fn a_divergent_backfill_is_rejected_and_never_written() {
    let history = History::new().await;
    let base = block_base();
    history
        .admit(&blocks(&[(10, 1_000, at(base, 1), 3, Subscription)]))
        .await;
    history.open_gap(11, 20).await;
    sqlx::query("INSERT INTO block_identity_window (node_id, height, block_hash, retained_until, observed_at) VALUES ($1, 15, $2, NULL, $3)")
        .bind(&history.node_id)
        .bind(format!("0x{:064x}", 0xdead_u64))
        .bind(at(base, 4))
        .execute(history.pool())
        .await
        .unwrap();

    let backfill = blocks(&[(15, 5_000, at(base, 5), 4, GapBackfill)]);
    for _ in 0..2 {
        assert_eq!(
            history.admit(&backfill).await.samples,
            vec![BlockOutcome::ChainDivergence]
        );
    }
    let observations: Vec<(i64, String, String)> = sqlx::query_as("SELECT height, retained_block_hash, observed_block_hash FROM chain_divergence_observations WHERE node_id=$1")
        .bind(&history.node_id)
        .fetch_all(history.pool())
        .await
        .unwrap();
    assert_eq!(
        observations,
        vec![(
            15,
            format!("0x{:064x}", 0xdead_u64),
            backfill.block_summaries[0].block_hash.to_string()
        )]
    );
    assert_eq!(history.summary_heights().await, vec![10]);
    assert_eq!(history.cumulative_block_count().await, 1);
    assert_eq!(
        history.coverage().await,
        vec![
            (10, 10, "covered".to_owned()),
            (11, 20, "open_recoverable_gap".to_owned()),
        ]
    );
}

/// A divergent hash at a height is recorded once as a Chain Divergence
/// Observation, however often it is seen, and never touches the summary or
/// the counters already recorded for that height.
#[tokio::test]
async fn divergent_hash_is_append_only_idempotent_and_preserves_summary_and_counters() {
    let history = History::new().await;
    let report = blocks(&[(
        42,
        1_000,
        "2026-08-12T09:00:00Z".to_owned(),
        3,
        Subscription,
    )]);
    let first = report.block_summaries[0].clone();
    let mut second = first.clone();
    second.block_hash = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        .parse()
        .unwrap();
    assert_eq!(
        history.admit(&report).await.samples,
        vec![BlockOutcome::Accepted]
    );

    let mut tx = crate::database::begin(history.pool()).await.unwrap();
    for _ in 0..2 {
        assert!(
            observe_block_identity(&mut tx, &second, second.observed_at, "2026-08-12T09:01:00Z")
                .await
                .unwrap()
        );
    }
    tx.commit().await.unwrap();
    let summary_hash: String = sqlx::query_scalar(
        "SELECT block_hash FROM block_summaries WHERE node_id=$1 AND block_number=42",
    )
    .bind(&history.node_id)
    .fetch_one(history.pool())
    .await
    .unwrap();
    let divergence_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chain_divergence_observations WHERE node_id=$1 AND height=42",
    )
    .bind(&history.node_id)
    .fetch_one(history.pool())
    .await
    .unwrap();
    assert_eq!(summary_hash, first.block_hash.to_string());
    assert_eq!(history.counters().await, (1, 3));
    assert_eq!(divergence_count, 1);
}

/// Compaction keeps coverage bounded without changing what any
/// height is. A stale open gap is confirmed missing, a fresh one stays
/// open, touching same-status intervals merge, and a History Gap nothing
/// open overlaps is resolved (so its retention family can release it).
#[tokio::test]
async fn coverage_compaction_confirms_stale_gaps_merges_and_resolves() {
    let dir = tempfile::TempDir::new().unwrap();
    let database = crate::test_support::test_database_in(dir.path()).await;
    let pool = database.pool();
    let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
    let window_start = crate::retention::raw_block_summary_cutoff(now);
    let stale = crate::auth::format_rfc3339(window_start - time::Duration::hours(1));
    let fresh = crate::auth::format_rfc3339(now - time::Duration::hours(1));
    sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('compact-agent', 1, $1, $2)")
        .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('compact-network', 'Compact', '0xgenesis', 1, 1, 'lat', $1, $2)")
        .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('compact-node', 'compact-agent', 'compact-network', 'ws://127.0.0.1:1', 'active', 'private', 1, $1, $2)")
        .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
    let interval = "INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ('compact-node', $1, $2, $3, $4, $5)";
    for (first, last, status, stamp) in [
        (1_i64, 9_i64, "covered", &fresh),
        (10, 19, "covered", &stale),
        (20, 29, "open_recoverable_gap", &stale),
        (30, 39, "covered", &fresh),
        (40, 49, "open_recoverable_gap", &fresh),
        (50, 59, "covered", &fresh),
    ] {
        sqlx::query(interval)
            .bind(first)
            .bind(last)
            .bind(status)
            .bind(stamp)
            .bind(stamp)
            .execute(pool)
            .await
            .unwrap();
    }
    let gap = "INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, created_at, authored_by) VALUES ('compact-node', $1, $2, $3, $4, $5)";
    for (from, to, kind, stamp, author) in [
        (20_i64, 29_i64, "unrecoverable_backfill", &stale, "agent"),
        (40, 49, "unrecoverable_backfill", &stale, "agent"),
        // Legacy: backfilled by a Server that recorded no coverage, so no interval
        // records 60..=64.
        (60, 64, "unrecoverable_backfill", &stale, "agent"),
        (70, 70, "server_rejected", &stale, "agent"),
        (71, 71, "server_rejected", &fresh, "agent"),
        (72, 72, "server_rejected", &stale, "server"),
    ] {
        sqlx::query(gap)
            .bind(from)
            .bind(to)
            .bind(kind)
            .bind(stamp)
            .bind(author)
            .execute(pool)
            .await
            .unwrap();
    }

    compact(pool, now, window_start, &mut 0).await.unwrap();

    let intervals: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT first_height, last_height, status FROM block_coverage_intervals WHERE node_id='compact-node' ORDER BY first_height",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(
        intervals,
        vec![
            (1, 19, "covered".to_owned()),
            (20, 29, "permanent_gap".to_owned()),
            (30, 39, "covered".to_owned()),
            (40, 49, "open_recoverable_gap".to_owned()),
            (50, 64, "covered".to_owned()),
        ],
        "a settled legacy gap records its backfilled heights covered"
    );
    let gaps: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT from_height, resolved_at FROM block_history_gaps WHERE node_id='compact-node' ORDER BY from_height",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let resolved: Vec<(i64, bool)> = gaps
        .into_iter()
        .map(|(from, resolved_at)| (from, resolved_at.is_some()))
        .collect();
    assert_eq!(
        resolved,
        vec![
            (20, true),
            (40, false),
            (60, true),
            (70, true),
            (71, false),
            (72, false),
        ],
        "confirmed and settled gaps and aged Agent refusal copies resolve; open gaps, fresh copies and the Server's own refusals stay"
    );

    // A second step is a no-op: compaction converges.
    assert_eq!(
        compact(pool, now, window_start, &mut 0)
            .await
            .unwrap()
            .changed,
        0
    );
}

/// Compaction settles History Gaps in bounded steps. The gap
/// around a range it confirms missing is settled directly, and the rest of
/// the unresolved gaps are examined one batch per step by a keyset walk
/// that resumes where the previous step stopped and starts over at the end.
#[tokio::test]
async fn coverage_compaction_settles_history_gaps_in_bounded_steps() {
    let dir = tempfile::TempDir::new().unwrap();
    let database = crate::test_support::test_database_in(dir.path()).await;
    let pool = database.pool();
    let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
    let window_start = crate::retention::raw_block_summary_cutoff(now);
    let stale = crate::auth::format_rfc3339(window_start - time::Duration::hours(1));
    let fresh = crate::auth::format_rfc3339(now - time::Duration::hours(1));
    sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('walk-agent', 1, $1, $2)")
        .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('walk-network', 'Walk', '0xgenesis', 1, 1, 'lat', $1, $2)")
        .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES ('walk-node', 'walk-agent', 'walk-network', 'ws://127.0.0.1:1', 'active', 'private', 1, $1, $2)")
        .bind(&fresh).bind(&fresh).execute(pool).await.unwrap();
    let interval = "INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ('walk-node', $1, $2, $3, $4, $5)";
    for (first, last, status, stamp) in [
        (1_i64, 1_000_i64, "covered", &fresh),
        (2_000, 2_000, "open_recoverable_gap", &stale),
    ] {
        sqlx::query(interval)
            .bind(first)
            .bind(last)
            .bind(status)
            .bind(stamp)
            .bind(stamp)
            .execute(pool)
            .await
            .unwrap();
    }
    // 300 gaps that are settled but were never resolved, then one gap whose
    // last open height is confirmed missing by the first step.
    let gap = "INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, created_at, authored_by) VALUES ('walk-node', $1, $2, 'unrecoverable_backfill', $3, 'agent')";
    for height in (1..=300_i64).chain([2_000]) {
        sqlx::query(gap)
            .bind(height)
            .bind(height)
            .bind(&stale)
            .execute(pool)
            .await
            .unwrap();
    }
    let unresolved = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM block_history_gaps WHERE node_id='walk-node' AND resolved_at IS NULL",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    };

    let mut cursor = 0;
    let first = compact(pool, now, window_start, &mut cursor).await.unwrap();
    assert_eq!(
        first.settlement,
        GapSettlement {
            examined: 129,
            resolved: 129
        },
        "the confirmed gap plus one batch of the walk"
    );
    assert!(cursor > 0, "the walk resumes after the batch it examined");
    assert_eq!(unresolved().await, 172);

    let second = compact(pool, now, window_start, &mut cursor).await.unwrap();
    assert_eq!(
        second.settlement,
        GapSettlement {
            examined: 128,
            resolved: 128
        }
    );
    assert_eq!(unresolved().await, 44);

    let third = compact(pool, now, window_start, &mut cursor).await.unwrap();
    assert_eq!(
        third.settlement,
        GapSettlement {
            examined: 44,
            resolved: 44
        }
    );
    assert_eq!(cursor, 0, "a short batch ends the walk, which starts over");
    assert_eq!(unresolved().await, 0);

    let idle = compact(pool, now, window_start, &mut cursor).await.unwrap();
    assert_eq!(idle.settlement, GapSettlement::default());
}
