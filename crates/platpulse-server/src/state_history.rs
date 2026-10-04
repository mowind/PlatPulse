//! Node synchronization-state history: the recorded sync and consensus states
//! of a Node, as served to the Owner-side Node Admin surface (issue #217,
//! design §11.4, §11.5, §11.6 and §11.7, story 53).
//!
//! Story 53 asks for two kinds of evidence, and this module owns the second:
//!
//! * The **numeric** key heights of a Node — sync current and highest block,
//!   and the epoch's highest QC, lock and commit block — are quantities, so
//!   they ride the metric-history engine: migration 0070 adds the five series
//!   to node_metric_samples, and with them the raw window, both aggregate
//!   tiers, the gap and coverage rules and the replay ledger.
//! * A **state** is not a quantity. (collection_state, value_source,
//!   error_code, syncing) cannot be averaged, and it changes by becoming
//!   something else. States are therefore recorded as a change log: one
//!   immutable row per recognized state plus a periodic anchor while the state
//!   does not change, in node_state_observations, with
//!   node_state_series_state as its ledger.
//!
//! Four rules keep that log trustworthy, and each of them is visible in the
//! data rather than assumed:
//!
//! * **A missing Report writes nothing.** The log is appended to by
//!   deliveries only, so a Node that stops reporting leaves a hole rather than
//!   a state change, and the read side reports "this stretch is not proven"
//!   instead of inventing a transition. Silence is a gap; a state change is
//!   evidence.
//! * **Unknown is not false.** The sync flag is stored only when the collection
//!   succeeded; an attempt that failed records the failure and leaves the flag
//!   empty, so nothing downstream can turn an unobserved flag into "not
//!   syncing". A Node whose value is gone reports whether the Server still
//!   holds one from before (value_source), never a fresh zero.
//! * **The last known value survives the failure.** value_source = last_good
//!   says the Server is still showing a value it observed earlier, and
//!   value_observed_at carries that earlier instant, so the age of the
//!   last-good value is read from evidence instead of recomputed.
//! * **A pause is a disclosed boundary, not a normal period.** While low-space
//!   protection is open the log is not appended to, and the skipped deliveries
//!   are counted on the shared skipped-series ledger (issue #212) the same way
//!   metric samples are, so a pause is reported with its counted losses
//!   separately from an unexplained silence (story 59).
//!
//! Nothing here is derived from Block history or from an Incident: the log
//! records what the Agent reported, and only that.

use platpulse_core::Rfc3339;
use platpulse_core::component::{ComponentObservation, ComponentStatus};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::auth;
use crate::capacity::{HistoryGate, SkippedScope};
use crate::metric_history::{self, Delivery, MetricGap, ProtectionPause, SeriesScope};

/// The components whose state is recorded, in the storage vocabulary.
pub const STATE_COMPONENTS: [&str; 2] = [COMPONENT_SYNC, COMPONENT_CONSENSUS];

/// The sync component key.
pub const COMPONENT_SYNC: &str = "sync";

/// The consensus component key.
pub const COMPONENT_CONSENSUS: &str = "consensus";

/// A recorded state that differs from the state recorded before it.
pub const ENTRY_KIND_CHANGE: &str = "change";

/// A periodic re-recording of a state that has not changed.
pub const ENTRY_KIND_ANCHOR: &str = "anchor";

/// The collection states the log stores, in the ingestion's own vocabulary.
pub const COLLECTION_STATE_STARTING: &str = "starting";
/// The collection succeeded.
pub const COLLECTION_STATE_OK: &str = "ok";
/// The collection failed; the failure is in error_code.
pub const COLLECTION_STATE_ERROR: &str = "error";
/// The Agent has this collection switched off.
pub const COLLECTION_STATE_DISABLED: &str = "disabled";
/// The Node's chain does not support this collection.
pub const COLLECTION_STATE_UNSUPPORTED: &str = "unsupported";

/// The Server observed the value in this delivery.
pub const VALUE_SOURCE_CURRENT: &str = "current";
/// The Server is still showing a value observed earlier (value_observed_at).
pub const VALUE_SOURCE_LAST_GOOD: &str = "last_good";
/// No value was ever observed, or the one held was released.
pub const VALUE_SOURCE_NONE: &str = "none";

/// How long an unchanged state stays proven before it is recorded again.
///
/// One state row per (node, component, hour) while nothing changes, plus one
/// row per change. The read side discloses this cadence, and it is what bounds
/// the stretch two adjacent rows prove.
pub const STATE_ANCHOR_SECONDS: i64 = 3600;

/// How many state rows one answer carries by default.
pub const DEFAULT_STATE_LIMIT: i64 = 2_000;

/// The largest number of state rows one answer may carry.
pub const MAX_STATE_LIMIT: i64 = 20_000;

/// Whether one component name is a recorded state component.
pub fn is_state_component(component: &str) -> bool {
    STATE_COMPONENTS.contains(&component)
}

/// The stored name of one collection state.
///
/// This is the vocabulary component_status.state already uses, so the state
/// log and the live projection cannot drift apart.
pub fn collection_state_name(status: ComponentStatus) -> &'static str {
    match status {
        ComponentStatus::Starting => COLLECTION_STATE_STARTING,
        ComponentStatus::Ok => COLLECTION_STATE_OK,
        ComponentStatus::Error => COLLECTION_STATE_ERROR,
        ComponentStatus::Disabled => COLLECTION_STATE_DISABLED,
        ComponentStatus::Unsupported => COLLECTION_STATE_UNSUPPORTED,
    }
}

/// The instant a delivered component was heard from.
///
/// The Agent's own attempt instant is the evidence whenever it reports one. A
/// component that carries no attempt (disabled, unsupported) still proves the
/// Node was alive at the instant its Report was generated, because the Report
/// itself is the evidence that the Node is up.
pub fn component_instant<T>(component: &ComponentObservation<T>, generated_at: Rfc3339) -> Rfc3339 {
    component.attempted_at.unwrap_or(generated_at)
}

/// What the Server can say about one component at one instant.
///
/// The four fields are the whole state: a change of any of them is a state
/// change, which is why a failure and a success never collapse into one row.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct StateVector {
    /// One of the COLLECTION_STATE_* names.
    pub collection_state: String,
    /// One of the VALUE_SOURCE_* names.
    pub value_source: String,
    /// The failure the Agent reported, when the collection failed.
    pub error_code: Option<String>,
    /// The sync flag the value carries, kept only for a successful collection.
    pub syncing: Option<bool>,
}

/// One delivered component state, ready to be recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateObservation {
    /// The instant the Node was heard from for this component.
    pub observed_at: Rfc3339,
    /// The instant the Server received the Report carrying it.
    pub received_at: Rfc3339,
    /// The state the delivery reports.
    pub vector: StateVector,
    /// The instant of the value the state refers to, when there is one.
    pub value_observed_at: Option<String>,
}

impl StateObservation {
    /// Read one delivered component as the Server's own projection holds it.
    ///
    /// retained_value_at is the component_status.observed_at of the component
    /// row *after* the delivery being recorded was saved, or None when the
    /// Server holds no value for it. It is what makes the difference between
    /// "the Agent failed and still remembers its last reading" and "the Agent
    /// failed and the Server is showing an older reading it kept" — in both
    /// cases the value on display is a last-good value, and its instant is the
    /// evidence for its age.
    ///
    /// syncing is the flag the delivered value carries. It is kept only for a
    /// successful collection, because an attempt that failed did not observe
    /// the flag at all.
    pub fn of_component<T>(
        component: &ComponentObservation<T>,
        syncing: Option<bool>,
        retained_value_at: Option<String>,
        observed_at: Rfc3339,
        received_at: Rfc3339,
    ) -> StateObservation {
        let carries_value = component.latest.is_some();
        let (value_source, value_observed_at) = if carries_value {
            let source = if component.status == ComponentStatus::Ok {
                VALUE_SOURCE_CURRENT
            } else {
                VALUE_SOURCE_LAST_GOOD
            };
            (
                source,
                component
                    .latest_observed_at
                    .map(|instant| instant.to_string()),
            )
        } else if let Some(instant) = retained_value_at {
            (VALUE_SOURCE_LAST_GOOD, Some(instant))
        } else {
            (VALUE_SOURCE_NONE, None)
        };

        StateObservation {
            observed_at,
            received_at,
            vector: StateVector {
                collection_state: collection_state_name(component.status).to_owned(),
                value_source: value_source.to_owned(),
                error_code: component.error.as_ref().map(|error| error.code.to_string()),
                syncing: if component.status == ComponentStatus::Ok {
                    syncing
                } else {
                    None
                },
            },
            value_observed_at,
        }
    }
}

/// The value the Server still holds for one Node component, as the instant it
/// was observed.
///
/// Read after the delivery being recorded has been written, because the
/// projection is what the Owner actually sees: an attempt that failed without
/// carrying a value leaves the projection untouched, and the log says so.
pub async fn retained_value_at(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
    node_id: &str,
    component: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT observed_at FROM component_status \
         WHERE agent_id = ? AND scope = 'node' AND scope_key = ? AND component_key = ? \
         AND value_received_at IS NOT NULL",
    )
    .bind(agent_id)
    .bind(node_id)
    .bind(component)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(observed_at,)| observed_at))
}

/// Record one delivered component state.
///
/// The delivery is classified against the series' own ledger exactly as a
/// metric sample is, and gated by low-space protection exactly as a metric
/// sample is. It is judged by the state the log already holds for its instant:
/// the row it would replace, or the row immediately before it when the log holds
/// none. A repeated delivery is a replay, a disagreement is a correction, and an
/// out-of-order delivery that states a state the log does not hold for that
/// stretch records the transition it is the first evidence of.
///
/// The ledger last_delivery_at is what makes a repeat recognisable at all: an
/// unchanged state writes no row between anchors, so an instant the series has
/// already counted looks exactly like an instant it has never seen. The log can
/// therefore promise an exact count only for the instants it holds a row for and
/// for the newest counted instant; a repeat of an older instant that wrote no row
/// is recorded as new evidence when it disagrees with the row before it (see
/// classify_state).
///
/// cutoff is the retention floor of this family (state_window_cutoff): evidence
/// older than it has been released, and a delivery that arrives for such an
/// instant is counted without being stored again — writing the row back would
/// claim coverage the Server deliberately stopped holding.
pub async fn record_state(
    tx: &mut Transaction<'_, Sqlite>,
    history: &HistoryGate,
    node_id: &str,
    component: &str,
    observation: &StateObservation,
    cutoff: &str,
) -> Result<(), sqlx::Error> {
    let observed_at = observation.observed_at.to_string();

    let ledger = fetch_ledger(&mut **tx, node_id, component).await?;
    let stored = fetch_entry(&mut *tx, node_id, component, &observed_at).await?;
    // The retained row before this instant, read whenever the log holds no row for
    // the instant itself: that is where a retried delivery and an out-of-order one
    // are told apart, and what the state was before a transition the delivery may
    // be the first evidence of. A delivery newer than the newest one the series
    // counted reads it too, because a late Report can insert a row behind that
    // evidence: the ledger's newest state then belongs to an instant no row
    // records, and comparing against it would judge the delivery by evidence that
    // came after it and leave the transition back to the delivered state
    // unrecorded.
    let predecessor = if stored.is_none() {
        fetch_predecessor(&mut *tx, node_id, component, &observed_at).await?
    } else {
        None
    };
    let latest = ledger.as_ref().map(StateLedger::latest);
    let previous = predecessor
        .as_ref()
        .map(|previous| &previous.vector)
        .or(latest.as_ref());
    let previous_at = predecessor
        .as_ref()
        .map(|previous| previous.observed_at.as_str())
        .or_else(|| {
            ledger
                .as_ref()
                .and_then(|ledger| ledger.last_entry_at.as_deref())
        });
    let delivery = classify_state(
        ledger.as_ref(),
        stored.as_ref(),
        predecessor.as_ref(),
        &observation.vector,
        &observed_at,
        cutoff,
    );

    if let HistoryGate::Paused { interval_id } = history {
        if delivery != Delivery::Observed {
            return Ok(());
        }
        // A pause is a disclosed boundary (story 59): the delivery is not
        // recorded, and the loss is counted on the shared skipped-series
        // ledger, keyed by the component the way a metric is keyed by its name.
        return crate::capacity::record_skipped_series(
            tx,
            interval_id,
            SkippedScope::Node,
            node_id,
            component,
            "",
            &observed_at,
        )
        .await;
    }

    let released = stored.is_none() && observed_at.as_str() < cutoff;
    let counted = delivery == Delivery::Observed;
    let kind = entry_kind(
        delivery,
        previous,
        &observation.vector,
        previous_at,
        &observed_at,
    );

    // Whether the newest state the series counted needed a row of its own,
    // because the row just written sits behind it and contradicts it (see
    // repair_late_insert).
    let mut repaired_at: Option<String> = None;
    if !released && delivery != Delivery::Replay {
        if let Some(kind) = kind {
            store_entry(tx, node_id, component, kind, observation).await?;
            repaired_at = repair_late_insert(
                tx,
                node_id,
                component,
                ledger.as_ref(),
                cutoff,
                &observed_at,
                observation,
            )
            .await?;
        }
    }

    store_ledger(
        tx,
        node_id,
        component,
        observation,
        &LedgerDelta {
            counted,
            change: counted && kind == Some(ENTRY_KIND_CHANGE),
            anchor: counted && kind == Some(ENTRY_KIND_ANCHOR),
            replay: delivery == Delivery::Replay,
            correction: delivery == Delivery::Correction,
            entry_at: if !released && kind.is_some() {
                Some(repaired_at.as_deref().unwrap_or(observed_at.as_str()))
            } else {
                None
            },
        },
    )
    .await?;

    if released && counted {
        if let Some(floor) = metric_history::counted_evidence_floor(&observed_at) {
            sqlx::query(
                "UPDATE node_state_series_state SET released_before = MAX(released_before, ?) \
                 WHERE node_id = ? AND component = ? AND released_before < ?",
            )
            .bind(&floor)
            .bind(node_id)
            .bind(component)
            .bind(&floor)
            .execute(&mut **tx)
            .await?;
        }
    }

    Ok(())
}

/// The ledger counters one delivery contributes to.
struct LedgerDelta<'a> {
    counted: bool,
    change: bool,
    anchor: bool,
    replay: bool,
    correction: bool,
    entry_at: Option<&'a str>,
}

/// Whether this delivery restates the recorded state or changes it.
///
/// previous is the state the log held immediately before the delivered instant
/// and previous_at is the row that held it. A delivery that disagrees with it
/// records a change; one that agrees needs a row of its own only once the state
/// has gone long enough without one.
fn entry_kind(
    delivery: Delivery,
    previous: Option<&StateVector>,
    vector: &StateVector,
    previous_at: Option<&str>,
    observed_at: &str,
) -> Option<&'static str> {
    match delivery {
        // The log already held a different state at this instant: the row is
        // rewritten, and the record of that instant does change.
        Delivery::Correction => Some(ENTRY_KIND_CHANGE),
        Delivery::Replay => None,
        Delivery::Observed => {
            if previous != Some(vector) {
                Some(ENTRY_KIND_CHANGE)
            } else if anchor_due(previous_at, observed_at) {
                Some(ENTRY_KIND_ANCHOR)
            } else {
                None
            }
        }
    }
}

/// Whether an unchanged state has gone long enough without a row to need one.
///
/// The anchor keeps a constant state provable: without it the read side could
/// only say "the state was X at some instant long ago" and would have to guess
/// whether everything after it is unknown or unchanged.
fn anchor_due(previous_at: Option<&str>, observed_at: &str) -> bool {
    let Some(last) = previous_at.and_then(auth::parse_rfc3339) else {
        return true;
    };
    let Some(now) = auth::parse_rfc3339(observed_at) else {
        return true;
    };
    now - last >= time::Duration::seconds(STATE_ANCHOR_SECONDS)
}

/// Classify one state delivery against the series' ledger and the retained
/// window, by the same four rules a metric sample is classified by.
///
/// The ledger judges an instant newer than its last_delivery_at as a new
/// delivery. At or before that instant the log's own rows decide everything the
/// delivery can be: the row held for the instant, or - when an unchanged delivery
/// wrote no row - the row immediately before it, which is the predecessor the
/// caller read. A delivery that equals the newest counted instant is the newest
/// word about evidence counted once (a correction if it disagrees with that
/// stretch, a replay if it restates it); one that states a different state at an
/// older instant the log never held is the first evidence of a transition.
///
/// One case stays open by construction: a repeat of an instant that is older than
/// the newest counted one and that the log holds no row for, because its unchanged
/// delivery wrote none. Nothing stored tells it apart from a delivery never seen
/// before, so a repeat that states a state the row before it does not hold is
/// recorded as new evidence. Only the instants the log holds a row for, and the
/// newest counted instant, are counted exactly once.
fn classify_state(
    ledger: Option<&StateLedger>,
    stored: Option<&StateVector>,
    predecessor: Option<&RetainedState>,
    vector: &StateVector,
    observed_at: &str,
    cutoff: &str,
) -> Delivery {
    if ledger.is_none_or(|ledger| observed_at > ledger.last_delivery_at.as_str()) {
        return Delivery::Observed;
    }
    if stored.is_some_and(|stored| stored != vector) {
        return Delivery::Correction;
    }
    let floor = ledger
        .map(|ledger| ledger.released_before.as_str())
        .filter(|released| *released > cutoff)
        .unwrap_or(cutoff);
    if stored.is_none() && observed_at >= floor {
        // The log holds no row here, and the series has already counted
        // deliveries at or after this instant, so the delivery is one of two
        // things. Restating the state the log already holds for the stretch — an
        // unchanged delivery that wrote no row, or a retried Report — is a replay
        // of evidence that was counted once. A state the log does not hold there
        // is new evidence: a skewed or out-of-order Report is the only record of
        // the transition that happened before the newest row, and discarding it
        // would leave the log stating the state it replaced for longer than that
        // state held.
        if predecessor.is_some_and(|previous| previous.vector == *vector) {
            return Delivery::Replay;
        }
        if ledger.is_some_and(|ledger| observed_at == ledger.last_delivery_at.as_str()) {
            // This instant was counted once already, and its unchanged state
            // wrote no row. The delivery disagrees with the state the log holds
            // for that stretch, so it corrects the record of that instant: it is
            // the newest word about evidence that was counted once, not a second
            // delivery.
            return Delivery::Correction;
        }
        return Delivery::Observed;
    }
    Delivery::Replay
}

async fn fetch_ledger<'e, E>(
    executor: E,
    node_id: &str,
    component: &str,
) -> Result<Option<StateLedger>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_as::<_, StateLedger>(
        "SELECT first_observed_at, last_observed_at, last_received_at, last_collection_state, \
         last_value_source, last_value_observed_at, last_error_code, last_syncing, last_entry_at, \
         last_delivery_at, entry_count, change_count, anchor_count, replayed_count, corrected_count, \
         released_before \
         FROM node_state_series_state WHERE node_id = ? AND component = ?",
    )
    .bind(node_id)
    .bind(component)
    .fetch_optional(executor)
    .await
}

async fn fetch_entry(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    component: &str,
    observed_at: &str,
) -> Result<Option<StateVector>, sqlx::Error> {
    sqlx::query_as::<_, StateVector>(
        "SELECT collection_state, value_source, error_code, syncing \
         FROM node_state_observations \
         WHERE node_id = ? AND component = ? AND observed_at = ?",
    )
    .bind(node_id)
    .bind(component)
    .bind(observed_at)
    .fetch_optional(&mut **tx)
    .await
}

/// One retained row read as the state immediately before an instant.
struct RetainedState {
    /// The instant that row recorded.
    observed_at: String,
    /// The state that row recorded.
    vector: StateVector,
}

/// The retained row immediately before one instant.
///
/// A delivery for an instant the log holds no row for is judged by this row.
/// Unchanged deliveries deliberately write no row, a Report can be retried after
/// a newer one landed, and an Agent whose clock disagrees can report an older
/// instant after a newer one: in all three cases the ledger's newest state
/// belongs to a later instant, so comparing against it would judge the delivery
/// by evidence that came after it and would drop a transition the delivery is the
/// only evidence of.
async fn fetch_predecessor(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    component: &str,
    observed_at: &str,
) -> Result<Option<RetainedState>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, String, String, Option<String>, Option<bool>)>(
        "SELECT observed_at, collection_state, value_source, error_code, syncing \
         FROM node_state_observations \
         WHERE node_id = ? AND component = ? AND observed_at < ? \
         ORDER BY observed_at DESC LIMIT 1",
    )
    .bind(node_id)
    .bind(component)
    .bind(observed_at)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(
        |(observed_at, collection_state, value_source, error_code, syncing)| RetainedState {
            observed_at,
            vector: StateVector {
                collection_state,
                value_source,
                error_code,
                syncing,
            },
        },
    ))
}

/// Write one state row, replacing the record of that instant when the Server
/// has already written one.
///
/// The row states exactly the vector the delivery's own projection holds: an
/// attempt that failed carries no flag and stores NULL for it — never the flag
/// of an era that has ended — and refers to the value it really still has, which
/// for a last-good value is component_status's retained instant. A correction is
/// the newest word about its instant rather than a merge with the row it replaces:
/// merging would leave the stored vector different from the one the classifier
/// reads next, so an identically retried delivery would be classified as another
/// correction instead of the replay it is, and a failed collection would keep
/// reporting a syncing flag it did not observe.
async fn store_entry(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    component: &str,
    kind: &str,
    observation: &StateObservation,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO node_state_observations (
            node_id, component, observed_at, received_at, entry_kind, collection_state,
            value_source, value_observed_at, error_code, syncing
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(node_id, component, observed_at) DO UPDATE SET
            received_at = excluded.received_at,
            entry_kind = excluded.entry_kind,
            collection_state = excluded.collection_state,
            value_source = excluded.value_source,
            value_observed_at = excluded.value_observed_at,
            error_code = excluded.error_code,
            syncing = excluded.syncing",
    )
    .bind(node_id)
    .bind(component)
    .bind(observation.observed_at.to_string())
    .bind(observation.received_at.to_string())
    .bind(kind)
    .bind(&observation.vector.collection_state)
    .bind(&observation.vector.value_source)
    .bind(observation.value_observed_at.as_deref())
    .bind(observation.vector.error_code.as_deref())
    .bind(observation.vector.syncing)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Fold one delivery into the series ledger: the newest state, the newest
/// instant, and what the delivery turned out to be.
///
/// The counters and the high-water marks move by the same rule the metric
/// ledger uses, so an out-of-order report cannot move the series backwards and
/// an instant the log holds a row for is counted once.
///
/// That promise is bounded by what the log keeps. An unchanged delivery writes no
/// row between anchors, so the instants it was counted for left nothing behind,
/// and a later repeat of one of them cannot be told from a delivery never seen
/// before. Such a repeat is counted as new evidence when it states a state the row
/// before it does not hold: the log cannot do better, and dropping it would lose
/// the only record of a transition it never held.
async fn store_ledger(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    component: &str,
    observation: &StateObservation,
    delta: &LedgerDelta<'_>,
) -> Result<(), sqlx::Error> {
    let observed_at = observation.observed_at.to_string();
    let received_at = observation.received_at.to_string();
    // The counter columns state what a brand new ledger row holds, not what this
    // delivery contributes: a series is created by the one change that wrote it,
    // because a component whose ledger does not exist yet can only be delivering
    // its first, newly observed state. The conflict branch below folds this
    // delivery in on top of that. Binding the delta here instead would make the
    // CHECK constraints reject the row a replay or an anchor is folded into: a
    // constraint is judged before the conflict is resolved, exactly as the metric
    // ledger writes its own counters (metric_history.rs SERIES_UPSERT_SQL).
    sqlx::query(
        "INSERT INTO node_state_series_state (
            node_id, component, first_observed_at, last_observed_at, last_received_at,
            last_collection_state, last_value_source, last_value_observed_at, last_error_code,
            last_syncing, last_entry_at, last_delivery_at, entry_count, change_count, anchor_count,
            replayed_count, corrected_count, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, 1, 0, 0, 0, ?)
        ON CONFLICT(node_id, component) DO UPDATE SET
            first_observed_at = MIN(node_state_series_state.first_observed_at, excluded.first_observed_at),
            last_received_at = CASE
                WHEN excluded.last_observed_at >= node_state_series_state.last_observed_at
                    THEN excluded.last_received_at
                ELSE node_state_series_state.last_received_at END,
            last_collection_state = CASE
                WHEN excluded.last_observed_at >= node_state_series_state.last_observed_at
                    THEN excluded.last_collection_state
                ELSE node_state_series_state.last_collection_state END,
            last_value_source = CASE
                WHEN excluded.last_observed_at >= node_state_series_state.last_observed_at
                    THEN excluded.last_value_source
                ELSE node_state_series_state.last_value_source END,
            last_value_observed_at = CASE
                WHEN excluded.last_observed_at >= node_state_series_state.last_observed_at
                    THEN excluded.last_value_observed_at
                ELSE node_state_series_state.last_value_observed_at END,
            last_error_code = CASE
                WHEN excluded.last_observed_at >= node_state_series_state.last_observed_at
                    THEN excluded.last_error_code
                ELSE node_state_series_state.last_error_code END,
            last_syncing = CASE
                WHEN excluded.last_observed_at >= node_state_series_state.last_observed_at
                    THEN excluded.last_syncing
                ELSE node_state_series_state.last_syncing END,
            last_entry_at = CASE
                WHEN excluded.last_entry_at IS NULL THEN node_state_series_state.last_entry_at
                WHEN node_state_series_state.last_entry_at IS NULL THEN excluded.last_entry_at
                WHEN excluded.last_entry_at > node_state_series_state.last_entry_at
                    THEN excluded.last_entry_at
                ELSE node_state_series_state.last_entry_at END,
            last_observed_at = MAX(node_state_series_state.last_observed_at, excluded.last_observed_at),
            last_delivery_at = MAX(node_state_series_state.last_delivery_at, excluded.last_delivery_at),
            entry_count = node_state_series_state.entry_count + ?,
            change_count = node_state_series_state.change_count + ?,
            anchor_count = node_state_series_state.anchor_count + ?,
            replayed_count = node_state_series_state.replayed_count + ?,
            corrected_count = node_state_series_state.corrected_count + ?,
            updated_at = excluded.updated_at",
    )
    .bind(node_id)
    .bind(component)
    .bind(&observed_at)
    .bind(&observed_at)
    .bind(&received_at)
    .bind(&observation.vector.collection_state)
    .bind(&observation.vector.value_source)
    .bind(observation.value_observed_at.as_deref())
    .bind(observation.vector.error_code.as_deref())
    .bind(observation.vector.syncing)
    .bind(delta.entry_at)
    .bind(&observed_at)
    .bind(&received_at)
    .bind(i64::from(delta.counted))
    .bind(i64::from(delta.change))
    .bind(i64::from(delta.anchor))
    .bind(i64::from(delta.replay))
    .bind(i64::from(delta.correction))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// One recorded state, as an Admin history answer carries it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct StateEntry {
    /// The instant the Node was heard from.
    pub observed_at: String,
    /// The instant the Server received the Report.
    pub received_at: String,
    /// change or anchor.
    pub entry_kind: String,
    /// One of the COLLECTION_STATE_* names.
    pub collection_state: String,
    /// One of the VALUE_SOURCE_* names, as recorded at that instant.
    pub value_source: String,
    /// The instant of the value the state refers to.
    pub value_observed_at: Option<String>,
    /// The failure the Agent reported.
    pub error_code: Option<String>,
    /// The sync flag, present only where a collection succeeded.
    pub syncing: Option<bool>,
}

impl StateEntry {
    /// The state this entry records.
    pub fn vector(&self) -> StateVector {
        StateVector {
            collection_state: self.collection_state.clone(),
            value_source: self.value_source.clone(),
            error_code: self.error_code.clone(),
            syncing: self.syncing,
        }
    }
}

/// The ledger of one state series: everything the Server ever counted for it,
/// independent of the window an answer asked for.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct StateLedger {
    /// The oldest counted instant.
    pub first_observed_at: String,
    /// The newest counted instant, which is the Server's latest word on the
    /// state even when the row that recorded it has been released.
    pub last_observed_at: String,
    /// The instant the newest counted Report arrived.
    pub last_received_at: String,
    /// The collection state of the newest counted delivery.
    pub last_collection_state: String,
    /// The value source of the newest counted delivery.
    pub last_value_source: String,
    /// The instant of the value the newest counted delivery refers to.
    pub last_value_observed_at: Option<String>,
    /// The failure the newest counted delivery reported.
    pub last_error_code: Option<String>,
    /// The sync flag of the newest counted delivery.
    pub last_syncing: Option<bool>,
    /// The newest instant that has a state row; None when none is retained.
    pub last_entry_at: Option<String>,
    /// The newest instant the series counted a delivery for, whether or not that
    /// delivery needed a row. This is what tells a repeat apart from an older
    /// instant the log is being told about for the first time: an unchanged state
    /// writes no row between anchors, so the rows alone cannot.
    pub last_delivery_at: String,
    /// Counted deliveries, including the ones that restated the state.
    pub entry_count: i64,
    /// Counted deliveries that recorded a state change.
    pub change_count: i64,
    /// Counted deliveries that re-recorded an unchanged state.
    pub anchor_count: i64,
    /// Deliveries of an instant the log had already counted.
    pub replayed_count: i64,
    /// Deliveries that disagreed with the row already held for their instant.
    pub corrected_count: i64,
    /// The floor last stamped for counted evidence the window had released.
    pub released_before: String,
}

impl StateLedger {
    /// The latest state the Server knows, independent of the requested window.
    pub fn latest(&self) -> StateVector {
        StateVector {
            collection_state: self.last_collection_state.clone(),
            value_source: self.last_value_source.clone(),
            error_code: self.last_error_code.clone(),
            syncing: self.last_syncing,
        }
    }

    /// The average spacing between counted deliveries.
    ///
    /// The ledger counts every delivery the log accepted, including the ones
    /// that wrote no row, so this is the Node's reporting cadence and not the
    /// anchor cadence of the rows. It is what the read side needs to tell a
    /// state that simply did not change from a Node that stopped reporting;
    /// zero means unknown, never "no interval".
    pub fn delivery_cadence_seconds(&self) -> i64 {
        if self.entry_count < 2 {
            return 0;
        }
        let (Some(first), Some(last)) = (
            auth::parse_rfc3339(&self.first_observed_at),
            auth::parse_rfc3339(&self.last_observed_at),
        ) else {
            return 0;
        };
        let span = (last - first).whole_seconds();
        if span <= 0 {
            return 0;
        }
        (span / (self.entry_count - 1)).max(1)
    }
}

/// The window of state history one answer asked for.
pub struct StateRangeQuery<'a> {
    /// The Node the series belongs to.
    pub node_id: &'a str,
    /// The recorded component.
    pub component: &'a str,
    /// The oldest instant the answer may carry.
    pub from: &'a str,
    /// The newest instant the answer may carry.
    pub to: &'a str,
    /// Answer only instants older than this cursor.
    pub before: Option<&'a str>,
    /// The largest number of rows the answer may carry.
    pub limit: i64,
    /// The retention floor: evidence older than it has been released.
    pub cutoff: &'a str,
}

/// What one window of the state log answers.
pub struct StateRange {
    /// The entries in the window, oldest first.
    pub entries: Vec<StateEntry>,
    /// The series ledger, when the series has ever been written to.
    pub ledger: Option<StateLedger>,
    /// The silences inside the window.
    pub gaps: Vec<MetricGap>,
    /// The seconds the entries prove, which is never the seconds they span.
    pub coverage_seconds: i64,
    /// Whether the window held more entries than the limit allowed.
    pub truncated: bool,
    /// The cursor that pages older without a hole.
    pub continuation: Option<String>,
    /// The reporting cadence the coverage was judged against.
    pub cadence_seconds: i64,
}

/// Read one window of a state series.
///
/// The answer separates what the log recorded from what it proves: the entries
/// are the recorded states, the gaps are the stretches where a Node should have
/// been heard from and was not (or where protection paused collection), and the
/// coverage is the part of the window two adjacent entries account for. Nothing
/// after the newest entry is claimed, because a state is not a constant that
/// extends until the next report.
pub async fn load_range(
    pool: &SqlitePool,
    query: &StateRangeQuery<'_>,
) -> Result<StateRange, sqlx::Error> {
    let fetch = query.limit.saturating_add(1);
    let mut entries = match query.before {
        Some(before) => {
            sqlx::query_as::<_, StateEntry>(
                "SELECT observed_at, received_at, entry_kind, collection_state, value_source, \
                 value_observed_at, error_code, syncing FROM node_state_observations \
                 WHERE node_id = ? AND component = ? AND observed_at > ? AND observed_at <= ? \
                 AND observed_at < ? ORDER BY observed_at DESC LIMIT ?",
            )
            .bind(query.node_id)
            .bind(query.component)
            .bind(query.from)
            .bind(query.to)
            .bind(before)
            .bind(fetch)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, StateEntry>(
                "SELECT observed_at, received_at, entry_kind, collection_state, value_source, \
                 value_observed_at, error_code, syncing FROM node_state_observations \
                 WHERE node_id = ? AND component = ? AND observed_at > ? AND observed_at <= ? \
                 ORDER BY observed_at DESC LIMIT ?",
            )
            .bind(query.node_id)
            .bind(query.component)
            .bind(query.from)
            .bind(query.to)
            .bind(fetch)
            .fetch_all(pool)
            .await?
        }
    };

    let truncated = entries.len() as i64 > query.limit;
    if truncated {
        entries.truncate(query.limit.max(0) as usize);
    }
    // The cursor is the oldest row of this page: a page that continues there
    // excludes it, so paging older neither repeats a row nor skips one.
    let continuation = if truncated {
        entries.last().map(|entry| entry.observed_at.clone())
    } else {
        None
    };
    // The answer is chronological, exactly like every other history surface:
    // the continuity rule below judges a pair oldest first, and a reader pages
    // older with the cursor rather than by reading a list backwards.
    entries.reverse();

    let ledger = fetch_ledger(pool, query.node_id, query.component).await?;
    let cadence_seconds = ledger
        .as_ref()
        .map_or(0, StateLedger::delivery_cadence_seconds);
    let pauses: Vec<ProtectionPause> = metric_history::load_pauses(
        pool,
        &SeriesScope::node(query.node_id, query.component),
        query.from,
        query.to,
    )
    .await?;

    // The stretch a recorded state answers for: the anchor interval, plus the
    // series' own delivery cadence. An unchanged state is re-stated only once the
    // anchor is due, so the delivery that states it can arrive one cadence after
    // that instant, and a window of the anchor interval alone would report the
    // ordinary distance between two anchors as silence.
    let anchor_window_seconds = STATE_ANCHOR_SECONDS + cadence_seconds.max(0);
    let points: Vec<metric_history::ObservablePoint<'_>> = entries
        .iter()
        .map(|entry| metric_history::ObservablePoint {
            from: entry.observed_at.as_str(),
            until: entry.observed_at.as_str(),
            window_seconds: anchor_window_seconds,
        })
        .collect();
    let continuity = metric_history::continuity_with_windows(
        &points,
        &pauses,
        cadence_seconds,
        query.from,
        query.to,
    );

    Ok(StateRange {
        entries,
        ledger,
        gaps: continuity.gaps,
        coverage_seconds: continuity.coverage_seconds,
        truncated,
        continuation,
        cadence_seconds,
    })
}

/// Keep the newest state the series counted on the record when a late Report is
/// inserted behind it.
///
/// A late Report can write a row for an instant older than the newest one the
/// series counted. The ledger's newest state then sits behind a row that
/// contradicts it: the read side would state the late state as the newest one
/// the log holds, while the transition back is nowhere in it, although the
/// Server did count it. Waiting for a later delivery to state it would leave the
/// history wrong until that delivery arrives, may wait forever, and would
/// attribute the state to an instant it was not proved at.
///
/// The row written here is not a delivery: it states evidence the series already
/// counted, so it moves no counter, exactly as the row a correction rewrites
/// moves none. It is written at the ledger's newest instant, which is above the
/// retention floor whenever the row that inserted behind it was, because
/// cutoff <= the delivered instant < the ledger's newest instant.
///
/// Returns the instant of the row it wrote, if it wrote one.
async fn repair_late_insert(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    component: &str,
    ledger: Option<&StateLedger>,
    cutoff: &str,
    observed_at: &str,
    observation: &StateObservation,
) -> Result<Option<String>, sqlx::Error> {
    let Some(ledger) = ledger else {
        return Ok(None);
    };
    let newest = ledger.latest();
    if ledger.last_observed_at.as_str() <= observed_at
        || ledger.last_observed_at.as_str() < cutoff
        || newest == observation.vector
    {
        return Ok(None);
    }
    let before = fetch_predecessor(tx, node_id, component, &ledger.last_observed_at).await?;
    // The row's kind is judged against the row that sits before it now, exactly as
    // a delivery's is: the row just written may have become its predecessor.
    let kind = entry_kind(
        Delivery::Observed,
        before.as_ref().map(|before| &before.vector),
        &newest,
        before.as_ref().map(|before| before.observed_at.as_str()),
        &ledger.last_observed_at,
    );
    let Some(kind) = kind else {
        // The row before it already holds the same state, so the newest state is
        // on the record and no row of its own is due yet.
        return Ok(None);
    };
    let (Ok(repair_at), Ok(repair_received_at)) = (
        ledger.last_observed_at.parse::<Rfc3339>(),
        ledger.last_received_at.parse::<Rfc3339>(),
    ) else {
        return Ok(None);
    };
    store_entry(
        tx,
        node_id,
        component,
        kind,
        &StateObservation {
            observed_at: repair_at,
            received_at: repair_received_at,
            vector: newest,
            value_observed_at: ledger.last_value_observed_at.clone(),
        },
    )
    .await?;
    Ok(Some(ledger.last_observed_at.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger(last: &str, first: &str, entries: i64) -> StateLedger {
        StateLedger {
            first_observed_at: first.to_owned(),
            last_observed_at: last.to_owned(),
            last_received_at: last.to_owned(),
            last_collection_state: COLLECTION_STATE_OK.to_owned(),
            last_value_source: VALUE_SOURCE_CURRENT.to_owned(),
            last_value_observed_at: Some(last.to_owned()),
            last_error_code: None,
            last_syncing: Some(true),
            last_entry_at: Some(last.to_owned()),
            last_delivery_at: last.to_owned(),
            entry_count: entries,
            change_count: 1,
            anchor_count: 0,
            replayed_count: 0,
            corrected_count: 0,
            released_before: "1970-01-01T00:00:00Z".to_owned(),
        }
    }

    fn vector(collection_state: &str, syncing: Option<bool>) -> StateVector {
        StateVector {
            collection_state: collection_state.to_owned(),
            value_source: if syncing.is_some() {
                VALUE_SOURCE_CURRENT.to_owned()
            } else {
                VALUE_SOURCE_NONE.to_owned()
            },
            error_code: None,
            syncing,
        }
    }

    #[test]
    fn every_collection_state_has_one_stored_name() {
        assert_eq!(
            collection_state_name(ComponentStatus::Starting),
            COLLECTION_STATE_STARTING
        );
        assert_eq!(
            collection_state_name(ComponentStatus::Ok),
            COLLECTION_STATE_OK
        );
        assert_eq!(
            collection_state_name(ComponentStatus::Error),
            COLLECTION_STATE_ERROR
        );
        assert_eq!(
            collection_state_name(ComponentStatus::Disabled),
            COLLECTION_STATE_DISABLED
        );
        assert_eq!(
            collection_state_name(ComponentStatus::Unsupported),
            COLLECTION_STATE_UNSUPPORTED
        );
    }

    #[test]
    fn a_first_delivery_is_new_and_an_older_one_is_a_replay() {
        let series = ledger("2026-01-01T01:00:00Z", "2026-01-01T00:00:00Z", 3);
        assert_eq!(
            classify_state(
                None,
                None,
                None,
                &vector(COLLECTION_STATE_OK, Some(true)),
                "2026-01-01T00:00:00Z",
                "1970-01-01T00:00:00Z"
            ),
            Delivery::Observed
        );
        assert_eq!(
            classify_state(
                Some(&series),
                Some(&vector(COLLECTION_STATE_OK, Some(true))),
                None,
                &vector(COLLECTION_STATE_OK, Some(true)),
                "2026-01-01T02:00:00Z",
                "1970-01-01T00:00:00Z"
            ),
            Delivery::Observed
        );
        assert_eq!(
            classify_state(
                Some(&series),
                Some(&vector(COLLECTION_STATE_OK, Some(true))),
                None,
                &vector(COLLECTION_STATE_OK, Some(true)),
                "2026-01-01T00:30:00Z",
                "1970-01-01T00:00:00Z"
            ),
            Delivery::Replay
        );
    }

    #[test]
    fn disagreeing_with_a_held_row_is_a_correction() {
        let series = ledger("2026-01-01T01:00:00Z", "2026-01-01T00:00:00Z", 3);
        assert_eq!(
            classify_state(
                Some(&series),
                Some(&vector(COLLECTION_STATE_ERROR, None)),
                None,
                &vector(COLLECTION_STATE_OK, Some(true)),
                "2026-01-01T00:30:00Z",
                "1970-01-01T00:00:00Z"
            ),
            Delivery::Correction
        );
    }

    #[test]
    fn a_released_instant_is_counted_without_being_stored_again() {
        let series = ledger("2026-01-01T01:00:00Z", "2026-01-01T00:00:00Z", 3);
        assert_eq!(
            classify_state(
                Some(&series),
                None,
                None,
                &vector(COLLECTION_STATE_OK, Some(true)),
                "2026-01-01T00:30:00Z",
                "2025-06-01T00:00:00Z"
            ),
            Delivery::Observed
        );
        assert_eq!(
            classify_state(
                Some(&series),
                None,
                None,
                &vector(COLLECTION_STATE_OK, Some(true)),
                "2026-01-01T00:30:00Z",
                "2026-01-01T00:45:00Z"
            ),
            Delivery::Replay
        );
    }

    #[test]
    fn a_changed_state_is_a_change_and_a_repeated_one_is_an_anchor() {
        let series = ledger("2026-01-01T01:00:00Z", "2026-01-01T00:00:00Z", 3);
        let unchanged = vector(COLLECTION_STATE_OK, Some(true));
        assert_eq!(
            entry_kind(
                Delivery::Observed,
                Some(&series.latest()),
                &vector(COLLECTION_STATE_ERROR, None),
                Some("2026-01-01T00:00:00Z"),
                "2026-01-01T01:00:00Z"
            ),
            Some(ENTRY_KIND_CHANGE)
        );
        assert_eq!(
            entry_kind(
                Delivery::Observed,
                Some(&series.latest()),
                &unchanged,
                Some("2026-01-01T00:00:00Z"),
                "2026-01-01T01:00:00Z"
            ),
            Some(ENTRY_KIND_ANCHOR)
        );
        assert_eq!(
            entry_kind(
                Delivery::Observed,
                Some(&series.latest()),
                &unchanged,
                Some("2026-01-01T01:00:00Z"),
                "2026-01-01T01:00:30Z"
            ),
            None
        );
    }

    #[test]
    fn a_state_without_a_row_is_anchored_at_once() {
        assert!(anchor_due(None, "2026-01-01T00:00:00Z"));
        assert!(anchor_due(
            Some("2026-01-01T00:00:00Z"),
            "2026-01-01T01:00:00Z"
        ));
        assert!(!anchor_due(
            Some("2026-01-01T00:00:00Z"),
            "2026-01-01T00:59:59Z"
        ));
    }

    #[test]
    fn the_cadence_counts_every_delivery_not_every_row() {
        let series = ledger("2026-01-01T01:00:00Z", "2026-01-01T00:00:00Z", 121);
        assert_eq!(series.delivery_cadence_seconds(), 30);
        assert_eq!(
            ledger("2026-01-01T01:00:00Z", "2026-01-01T01:00:00Z", 1).delivery_cadence_seconds(),
            0
        );
    }

    #[test]
    fn a_repeated_delivery_for_a_suppressed_instant_is_a_replay() {
        // The log holds Ok at 00:00 and no row at 00:30, because an unchanged
        // delivery writes no row there. Restating that state is evidence the
        // series already counted once.
        let series = ledger("2026-01-01T01:00:00Z", "2026-01-01T00:00:00Z", 120);
        let predecessor = RetainedState {
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            vector: vector(COLLECTION_STATE_OK, Some(true)),
        };
        assert_eq!(
            classify_state(
                Some(&series),
                None,
                Some(&predecessor),
                &vector(COLLECTION_STATE_OK, Some(true)),
                "2026-01-01T00:30:00Z",
                "1970-01-01T00:00:00Z"
            ),
            Delivery::Replay
        );
        // A state the log does not hold for that stretch is new evidence, and the
        // delivery is the row that records the transition it is evidence of.
        assert_eq!(
            classify_state(
                Some(&series),
                None,
                Some(&predecessor),
                &vector(COLLECTION_STATE_ERROR, None),
                "2026-01-01T00:30:00Z",
                "1970-01-01T00:00:00Z"
            ),
            Delivery::Observed
        );
        assert_eq!(
            entry_kind(
                Delivery::Observed,
                Some(&predecessor.vector),
                &vector(COLLECTION_STATE_ERROR, None),
                Some(predecessor.observed_at.as_str()),
                "2026-01-01T00:30:00Z"
            ),
            Some(ENTRY_KIND_CHANGE)
        );
    }

    #[test]
    fn an_out_of_order_delivery_states_the_transition_it_evidences() {
        // Error at 00:00, Ok at 01:00, then Ok at 00:30: the log holds no row at
        // 00:30 and the state there differs from the row immediately before it,
        // so the delivery is the only evidence the transition happened at 00:30.
        let series = ledger("2026-01-01T01:00:00Z", "2026-01-01T00:00:00Z", 90);
        let predecessor = RetainedState {
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            vector: vector(COLLECTION_STATE_ERROR, None),
        };
        assert_eq!(
            classify_state(
                Some(&series),
                None,
                Some(&predecessor),
                &vector(COLLECTION_STATE_OK, Some(true)),
                "2026-01-01T00:30:00Z",
                "1970-01-01T00:00:00Z"
            ),
            Delivery::Observed
        );
        assert_eq!(
            entry_kind(
                Delivery::Observed,
                Some(&predecessor.vector),
                &vector(COLLECTION_STATE_OK, Some(true)),
                Some(predecessor.observed_at.as_str()),
                "2026-01-01T00:30:00Z"
            ),
            Some(ENTRY_KIND_CHANGE)
        );
    }
    #[test]
    fn a_repeat_at_a_counted_instant_is_not_counted_twice() {
        // The delivery at 00:30 was counted as cadence evidence and wrote no row,
        // because its state was unchanged and no anchor was due. Delivering that
        // instant again is the same evidence, not a second delivery.
        let counted = ledger("2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z", 1);
        let series = StateLedger {
            last_delivery_at: "2026-01-01T00:30:00Z".to_owned(),
            ..counted
        };
        let predecessor = RetainedState {
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            vector: vector(COLLECTION_STATE_OK, Some(true)),
        };
        assert_eq!(
            classify_state(
                Some(&series),
                None,
                Some(&predecessor),
                &vector(COLLECTION_STATE_OK, Some(true)),
                "2026-01-01T00:30:00Z",
                "1970-01-01T00:00:00Z"
            ),
            Delivery::Replay
        );
        // Delivered again with a different state, that instant corrects the record
        // of itself and must not be counted a second time.
        assert_eq!(
            classify_state(
                Some(&series),
                None,
                Some(&predecessor),
                &vector(COLLECTION_STATE_ERROR, None),
                "2026-01-01T00:30:00Z",
                "1970-01-01T00:00:00Z"
            ),
            Delivery::Correction
        );
        // A later instant with an unchanged state is a new delivery, counted as
        // cadence evidence even though it writes no row.
        assert_eq!(
            classify_state(
                Some(&series),
                None,
                None,
                &vector(COLLECTION_STATE_OK, Some(true)),
                "2026-01-01T00:31:00Z",
                "1970-01-01T00:00:00Z"
            ),
            Delivery::Observed
        );
    }
}
