//! Current projections written from an accepted Report: component state, the
//! Node and Host current rows, and metric samples.

use sqlx::{Any, Transaction};

use crate::metric_history::Delivery;
use crate::peer_history::PeerPresenceDelta;
use platpulse_core::component::{ComponentKey, ComponentObservation, ComponentStatus};
use platpulse_core::observation::PeerDirection;
use platpulse_core::{AgentReport, ReportInventory, Rfc3339};

use super::*;

/// A polled Chain Head can trail the subscribed Block History by the blocks
/// that arrived between polls, so one below-high-water sample is observational
/// skew rather than a resync. A below-high-water episode must persist for this
/// long before the resync projection treats it as real; a genuine replay lasts
/// far longer, while skew clears on the next report.
const RESYNC_TRANSITION_GRACE_SECONDS: i64 = 30;

fn status_name(status: ComponentStatus) -> &'static str {
    match status {
        ComponentStatus::Starting => "starting",
        ComponentStatus::Ok => "ok",
        ComponentStatus::Error => "error",
        ComponentStatus::Disabled => "disabled",
        ComponentStatus::Unsupported => "unsupported",
    }
}

/// Return the persisted component key used by the Server projections.
///
/// Most legacy keys predate the wire enum's snake_case names. Network identity
/// is read by the Admin projection under its stable network_identity key, so
/// ingestion must use that same key rather than the Rust debug spelling.
pub(crate) fn component_storage_key(key: ComponentKey) -> String {
    match key {
        ComponentKey::NetworkIdentity => "network_identity".to_owned(),
        key => format!("{key:?}").to_lowercase(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn save_component<T: serde::Serialize>(
    tx: &mut Transaction<'_, Any>,
    agent_id: &str,
    scope: &str,
    scope_key: &str,
    node_id: Option<&str>,
    key: ComponentKey,
    component: &ComponentObservation<T>,
    stamp: ReportStamp<'_>,
) -> Result<(), sqlx::Error> {
    let received_at = stamp.received_at();
    let error_code = component.error.as_ref().map(|e| e.code.as_str());
    let redacted_error_message = component
        .error
        .as_ref()
        .map(|error| crate::redaction::redact_sensitive(&error.message));
    let error_message = redacted_error_message.as_deref();
    let has_value = component.status == ComponentStatus::Ok && component.latest.is_some();
    let value_received_at = has_value.then_some(received_at);
    // The state evidence is the newest attempt, which a failed collection also
    // carries; the value evidence is the instant the value the Server still
    // holds was observed. Both are Server-clock instants, so the freshness and
    // the alert evidence of a replayed Report stay where the Report observed
    // them (ADR 0014, issue #274). The Agent's raw instant stays in `observed_at`.
    let observation_at = stamp.observed_at(component.attempted_at.or(component.latest_observed_at));
    let value_observation_at = has_value
        .then(|| stamp.observed_at(component.latest_observed_at.or(component.attempted_at)));
    sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, value_received_at, observation_at, value_observation_at, state_revision, value_revision, error_code, error_message) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) ON CONFLICT(agent_id, scope, scope_key, component_key) DO UPDATE SET state=excluded.state, attempted_at=excluded.attempted_at, observed_at=COALESCE(excluded.observed_at, component_status.observed_at), received_at=excluded.received_at, value_received_at=COALESCE(excluded.value_received_at, component_status.value_received_at), observation_at=excluded.observation_at, value_observation_at=COALESCE(excluded.value_observation_at, component_status.value_observation_at), state_revision=excluded.state_revision, value_revision=CASE WHEN excluded.value_revision > 0 THEN excluded.value_revision ELSE component_status.value_revision END, error_code=excluded.error_code, error_message=excluded.error_message")
        .bind(agent_id).bind(scope).bind(scope_key).bind(node_id).bind(component_storage_key(key))
        .bind(status_name(component.status)).bind(component.attempted_at.map(|v| v.to_string()))
        .bind(component.latest_observed_at.map(|v| v.to_string())).bind(received_at)
        .bind(value_received_at).bind(&observation_at).bind(value_observation_at)
        .bind(component.state_revision as i64).bind(component.value_revision as i64)
        .bind(error_code).bind(error_message).execute(&mut **tx).await?;
    Ok(())
}

fn metric_observed_at<T>(observation: &ComponentObservation<T>) -> Option<Rfc3339> {
    observation.latest_observed_at.or_else(|| {
        (observation.status == ComponentStatus::Ok)
            .then_some(observation.attempted_at)
            .flatten()
    })
}

/// The Host series one Report carries, as (metric, dimension, observed_at,
/// value).
///
/// Host evidence belongs to the Agent, never to a Node of it: one Report states
/// each quantity once and the history stores it once, so six Nodes on two Hosts
/// bill two series sets and no Node owns a copy of its Host's totals (design
/// §11.6, issue #215). The storage series are dimensioned by mount path, so a
/// path that moves to another filesystem starts a new series instead of
/// inheriting the old one's history.
///
/// Only quantities the Report actually carried are listed: a component with no
/// latest value states nothing, and a missing reading is not zero.
fn host_series_samples(
    host: &platpulse_core::observation::HostObservation,
) -> Vec<(&'static str, &str, Rfc3339, f64)> {
    let mut samples: Vec<(&'static str, &str, Rfc3339, f64)> = Vec::new();
    if let (Some(cpu_percent), Some(observed_at)) = (
        host.cpu_percent.latest,
        metric_observed_at(&host.cpu_percent),
    ) {
        samples.push(("cpu_percent", "", observed_at, cpu_percent));
    }
    if let (Some(memory), Some(observed_at)) =
        (host.memory.latest, metric_observed_at(&host.memory))
    {
        samples.push((
            "memory_used_bytes",
            "",
            observed_at,
            memory.used_bytes as f64,
        ));
        samples.push((
            "memory_total_bytes",
            "",
            observed_at,
            memory.total_bytes as f64,
        ));
    }
    if let (Some(load), Some(observed_at)) = (host.load.latest, metric_observed_at(&host.load)) {
        samples.push(("load1", "", observed_at, load.load1));
        samples.push(("load5", "", observed_at, load.load5));
        samples.push(("load15", "", observed_at, load.load15));
    }
    if let (Some(network), Some(observed_at)) = (
        host.network_throughput.latest,
        metric_observed_at(&host.network_throughput),
    ) {
        samples.push((
            "network_rx_bytes_per_sec",
            "",
            observed_at,
            network.rx_bytes_per_sec as f64,
        ));
        samples.push((
            "network_tx_bytes_per_sec",
            "",
            observed_at,
            network.tx_bytes_per_sec as f64,
        ));
    }
    if let (Some(disk), Some(observed_at)) =
        (host.disk.latest.as_ref(), metric_observed_at(&host.disk))
    {
        for mount in &disk.mounts {
            samples.push((
                "disk_used_bytes",
                mount.mount_path.as_str(),
                observed_at,
                mount.used_bytes as f64,
            ));
            samples.push((
                "disk_total_bytes",
                mount.mount_path.as_str(),
                observed_at,
                mount.total_bytes as f64,
            ));
        }
    }
    samples
}

/// The raw metric window a delivery in this Report is judged against.
///
/// The cutoff comes from the persisted `raw_metric_sample` policy, the same
/// family the post-ingestion cleanup reads, and it is read through the open
/// ingestion transaction: ingestion holds the Server's single write connection,
/// so a pool read here would wait on the caller's own transaction (issue #213).
async fn metric_window_cutoff(tx: &mut Transaction<'_, Any>) -> Result<String, sqlx::Error> {
    let days = crate::retention::metric_sample_retention_days_tx(tx).await?;
    Ok(crate::auth::format_rfc3339(
        crate::retention::family_cutoff(crate::auth::now_utc(), days),
    ))
}

/// The recorded-state window a delivery in this Report is judged against.
///
/// The state log is its own retention family (issue #217), so its floor comes
/// from its own policy and is read through the open ingestion transaction for
/// the same reason the raw window is (issue #213).
async fn state_window_cutoff(tx: &mut Transaction<'_, Any>) -> Result<String, sqlx::Error> {
    let days = crate::retention::state_retention_days_tx(tx).await?;
    Ok(crate::auth::format_rfc3339(
        crate::retention::family_cutoff(crate::auth::now_utc(), days),
    ))
}

/// Record one metric sample for a scope's series, unless optional history is
/// paused.
///
/// One writer serves both scopes (issue #215): a Host series is the same
/// evidence as a Node series. The scope selects the tables, the column that
/// names the owner, and whether the series identity carries a dimension (the
/// mount path of a storage series), so every rule below - the delivery
/// classification, the raw window's cutoff, the aggregate tiers, the ledger and
/// the skipped-series record - is one rule for both.
///
/// Nothing here expires history: raw samples are bounded by the retention
/// policy family `raw_metric_sample` (issue #213), because a fixed per-series
/// row cap tied the retained window to the report cadence and deleted readings
/// the Server had already accepted. The Server keeps at least the last 24 hours
/// of raw samples and never connects across a stretch it did not observe.
///
/// Under low-space protection the sample is not written: the Server records the
/// gap instead of deleting history it already holds (design §11.5, issue #212).
/// The gap record shares the ingestion transaction, so "sample skipped" and
/// "gap recorded" commit together and a failure rolls the whole Report back.
/// The series ledger moves in this same transaction.
async fn save_metric_sample(
    tx: &mut Transaction<'_, Any>,
    history: &crate::capacity::HistoryGate,
    scope: &crate::metric_history::SeriesScope<'_>,
    stamp: ReportStamp<'_>,
    source_observed_at: Rfc3339,
    raw_cutoff: &str,
    value: f64,
) -> Result<(), sqlx::Error> {
    // A metric sample's stable observation identity is kept separate from its
    // skew-corrected timeline (ADR 0014, issue #279).
    //
    // The Agent's raw observation instant serves as identity: the raw table's
    // unique key, stored_value lookup, and the series ledger's delivery
    // classification (last_observed_at high-water mark) all use it. A
    // per-Report skew reading jitters by milliseconds, and a correction that
    // crosses a second boundary (e.g. 501 ms vs 499 ms) moves the corrected
    // instant into a neighbouring second. If the instant were both identity
    // and timeline, a carried last-good sample crossing that boundary would appear
    // past the ledger's high-water mark and be double-counted (issue #213).
    //
    // Skew-corrected `observed_at` places the sample on the Server timeline for
    // range queries, aggregate buckets and retention expiry.
    let source_observed_str = source_observed_at.to_string();
    let observed_at = stamp.history_at(source_observed_at).to_string();
    let received_at = stamp.received_at();
    // What the Server already holds for this observation time, and how the
    // series ledger classifies this delivery: new history, a replay of an
    // observation it already has, or a correction to one (issue #213). The
    // ledger and the raw window's own cutoff answer that, not the sample row
    // alone: retention releases the row once it leaves the window while the
    // ledger keeps counting.
    let stored = crate::metric_history::stored_sample(tx, scope, &source_observed_str).await?;
    let stored_value = stored.as_ref().map(|s| s.value);
    let delivery = crate::metric_history::classify_delivery(
        tx,
        scope,
        &source_observed_str,
        stored_value,
        value,
        raw_cutoff,
    )
    .await?;
    if let crate::capacity::HistoryGate::Paused { interval_id } = history {
        // An observation the Server has already counted is not new history: a
        // repeated last-good sample is not a second lost sample (design §11.5,
        // issue #212). Only a first-time observation counts as a gap.
        if delivery != Delivery::Observed {
            return Ok(());
        }
        return crate::capacity::record_skipped_series(
            tx,
            interval_id,
            scope.schema.skipped_scope,
            scope.scope_key,
            scope.series.metric,
            scope.series.dimension,
            &observed_at,
        )
        .await;
    }
    // A delivery the raw window can no longer answer with, and that the Server
    // holds nothing for, is not written back: the ledger still counts what it
    // knows about the series, but the cleanup that runs on this same Report
    // would release the row again. A replay writes no row either, even when a
    // widened window covers its instant again: the row was released on purpose,
    // and recreating it would invent coverage and a fresh receipt for a stretch
    // the Server deliberately stopped holding (issue #213).
    let released =
        crate::metric_history::outside_retained_window(stored_value, &observed_at, raw_cutoff);
    if !released && delivery != Delivery::Replay {
        crate::metric_history::store_sample(
            tx,
            scope,
            &source_observed_str,
            &observed_at,
            received_at,
            value,
        )
        .await?;
    }
    // The tiers beyond the raw window accumulate in this same transaction: an
    // observation the Server counted advances its minute and five-minute
    // buckets even while its raw row is released, and a correction recomputes
    // the buckets it can still restate from retained rows. A replay moves
    // nothing — it is the same observation, not another one. Deriving the tiers
    // at read time instead would state a count the Server never counted, and
    // would count one stretch twice the moment it is read at two grains (issue
    // #214, design §11.6).
    match delivery {
        Delivery::Observed => {
            crate::metric_history::record_aggregates(tx, scope, &observed_at, received_at, value)
                .await?;
        }
        Delivery::Correction => {
            let target_observed_at = stored
                .as_ref()
                .map(|s| s.observed_at.as_str())
                .unwrap_or(observed_at.as_str());
            crate::metric_history::recompute_aggregates(
                tx,
                scope,
                target_observed_at,
                received_at,
                value,
            )
            .await?;
        }
        Delivery::Replay => {}
    }
    // The ledger moves with the sample it describes, so a committed Report can
    // never leave the count describing a sample the Server does not hold.
    crate::metric_history::record_delivery(tx, scope, &source_observed_str, received_at, delivery)
        .await?;
    // A first-time observation the window cannot hold is counted while it is
    // stored nowhere, so the instant it was counted at is recorded as evidence
    // the Server no longer holds: a later carry of the same instant — after the
    // Operator widens the window back over it — is a replay, never a second
    // observation.
    if released && delivery == Delivery::Observed {
        if let Some(floor) = crate::metric_history::counted_evidence_floor(&source_observed_str) {
            crate::metric_history::stamp_evidence_floor(tx, scope, &floor).await?;
        }
    }
    Ok(())
}

pub(super) async fn save_current<I: ReportInventory>(
    tx: &mut Transaction<'_, Any>,
    history: &crate::capacity::HistoryGate,
    report: &AgentReport<I>,
    stamp: ReportStamp<'_>,
    geo_provider: crate::geo::GeoProvider,
) -> Result<(), sqlx::Error> {
    let received_at = stamp.received_at();
    // Both retention windows are read once for the whole Report: they depend
    // on the policy in this transaction's snapshot and on the clock, never on
    // the series being written, and Report Ingestion holds the Server's single
    // write connection (issue #213), so reading them per sample spent a round
    // trip per sample to re-learn the same two instants.
    let raw_cutoff = metric_window_cutoff(tx).await?;
    let state_cutoff = state_window_cutoff(tx).await?;
    let agent_id = report.agent_id.to_string();
    let host = &report.host;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::CpuPercent,
        &host.cpu_percent,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::Memory,
        &host.memory,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::Load,
        &host.load,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::Disk,
        &host.disk,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::NetworkThroughput,
        &host.network_throughput,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::ClockSkew,
        &host.clock_skew,
        stamp,
    )
    .await?;
    save_component(
        tx,
        &agent_id,
        "host",
        "host",
        None,
        ComponentKey::Spool,
        &host.spool,
        stamp,
    )
    .await?;

    let spool_last_delivery_error = host
        .spool
        .latest
        .as_ref()
        .and_then(|value| value.last_delivery_error.as_deref())
        .map(crate::redaction::redact_sensitive);
    let spool_store_error = host
        .spool
        .latest
        .as_ref()
        .and_then(|value| value.store_error.as_deref())
        .map(crate::redaction::redact_sensitive);
    // Spool Backlog Age is derived here, at receipt, because the Report itself
    // cannot express it: an immutable Report can only say how old its oldest
    // queued Report was when it was generated, which after a replay is hours
    // stale. The reading and its clock basis are recorded together so the Admin
    // projections never mix the two bases (ADR 0014, issue #272).
    let spool_backlog = crate::spool_backlog::spool_backlog_age(
        host.spool.latest.as_ref(),
        report.generated_at,
        received_instant(received_at, report.generated_at),
        host.clock_skew.latest,
    );
    sqlx::query("INSERT INTO current_host_observations (agent_id, cpu_percent, memory_total_bytes, memory_used_bytes, load1, load5, load15, network_rx_bytes_per_sec, network_tx_bytes_per_sec, clock_skew_ms, spool_queued_bytes, spool_queued_reports, spool_oldest_queued_age_ms, spool_dropped_reports, spool_dropped_samples, spool_in_flight, spool_last_delivery_error, spool_last_delivery_at, spool_capacity_bytes, spool_max_age_seconds, spool_dropped_sequence_from, spool_dropped_sequence_to, spool_dropped_time_from, spool_dropped_time_to, spool_dropped_height_from, spool_dropped_height_to, spool_pending_history_gaps, spool_report_too_large, spool_store_fatal, spool_store_error, spool_backlog_age_ms, spool_backlog_age_basis, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33) ON CONFLICT(agent_id) DO UPDATE SET cpu_percent=COALESCE(excluded.cpu_percent, current_host_observations.cpu_percent), memory_total_bytes=COALESCE(excluded.memory_total_bytes, current_host_observations.memory_total_bytes), memory_used_bytes=COALESCE(excluded.memory_used_bytes, current_host_observations.memory_used_bytes), load1=COALESCE(excluded.load1, current_host_observations.load1), load5=COALESCE(excluded.load5, current_host_observations.load5), load15=COALESCE(excluded.load15, current_host_observations.load15), network_rx_bytes_per_sec=COALESCE(excluded.network_rx_bytes_per_sec, current_host_observations.network_rx_bytes_per_sec), network_tx_bytes_per_sec=COALESCE(excluded.network_tx_bytes_per_sec, current_host_observations.network_tx_bytes_per_sec), clock_skew_ms=COALESCE(excluded.clock_skew_ms, current_host_observations.clock_skew_ms), spool_queued_bytes=COALESCE(excluded.spool_queued_bytes, current_host_observations.spool_queued_bytes), spool_queued_reports=COALESCE(excluded.spool_queued_reports, current_host_observations.spool_queued_reports), spool_oldest_queued_age_ms=COALESCE(excluded.spool_oldest_queued_age_ms, current_host_observations.spool_oldest_queued_age_ms), spool_dropped_reports=COALESCE(excluded.spool_dropped_reports, current_host_observations.spool_dropped_reports), spool_dropped_samples=COALESCE(excluded.spool_dropped_samples, current_host_observations.spool_dropped_samples), spool_in_flight=COALESCE(excluded.spool_in_flight, current_host_observations.spool_in_flight), spool_last_delivery_error=COALESCE(excluded.spool_last_delivery_error, current_host_observations.spool_last_delivery_error), spool_last_delivery_at=COALESCE(excluded.spool_last_delivery_at, current_host_observations.spool_last_delivery_at), spool_capacity_bytes=COALESCE(excluded.spool_capacity_bytes, current_host_observations.spool_capacity_bytes), spool_max_age_seconds=COALESCE(excluded.spool_max_age_seconds, current_host_observations.spool_max_age_seconds), spool_dropped_sequence_from=COALESCE(excluded.spool_dropped_sequence_from, current_host_observations.spool_dropped_sequence_from), spool_dropped_sequence_to=COALESCE(excluded.spool_dropped_sequence_to, current_host_observations.spool_dropped_sequence_to), spool_dropped_time_from=COALESCE(excluded.spool_dropped_time_from, current_host_observations.spool_dropped_time_from), spool_dropped_time_to=COALESCE(excluded.spool_dropped_time_to, current_host_observations.spool_dropped_time_to), spool_dropped_height_from=COALESCE(excluded.spool_dropped_height_from, current_host_observations.spool_dropped_height_from), spool_dropped_height_to=COALESCE(excluded.spool_dropped_height_to, current_host_observations.spool_dropped_height_to), spool_pending_history_gaps=COALESCE(excluded.spool_pending_history_gaps, current_host_observations.spool_pending_history_gaps), spool_report_too_large=COALESCE(excluded.spool_report_too_large, current_host_observations.spool_report_too_large), spool_store_fatal=COALESCE(excluded.spool_store_fatal, current_host_observations.spool_store_fatal), spool_store_error=COALESCE(excluded.spool_store_error, current_host_observations.spool_store_error), spool_backlog_age_ms=excluded.spool_backlog_age_ms, spool_backlog_age_basis=excluded.spool_backlog_age_basis, updated_at=excluded.updated_at")
        .bind(&agent_id)
        .bind(crate::database::NullableReal(host.cpu_percent.latest))
        .bind(host.memory.latest.map(|v| v.total_bytes as i64))
        .bind(host.memory.latest.map(|v| v.used_bytes as i64))
        .bind(crate::database::NullableReal(host.load.latest.map(|v| v.load1)))
        .bind(crate::database::NullableReal(host.load.latest.map(|v| v.load5)))
        .bind(crate::database::NullableReal(host.load.latest.map(|v| v.load15)))
        .bind(host.network_throughput.latest.map(|v| v.rx_bytes_per_sec as i64))
        .bind(host.network_throughput.latest.map(|v| v.tx_bytes_per_sec as i64))
        .bind(host.clock_skew.latest)
        .bind(host.spool.latest.as_ref().map(|v| v.queued_bytes as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.queued_reports as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.oldest_queued_age_ms as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.dropped_reports as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.dropped_samples as i64))
        .bind(host.spool.latest.as_ref().and_then(|v| v.in_flight.map(|value| value as i64)))
        .bind(spool_last_delivery_error.as_deref())
        .bind(host.spool.latest.as_ref().and_then(|v| v.last_delivery_at.as_ref()).map(ToString::to_string))
        .bind(host.spool.latest.as_ref().and_then(|v| v.capacity_bytes.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.max_age_seconds.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_sequence_range.map(|x| x.0 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_sequence_range.map(|x| x.1 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_time_range.as_ref().map(|x| x.0.to_string())))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_time_range.as_ref().map(|x| x.1.to_string())))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_height_range.map(|x| x.0 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_height_range.map(|x| x.1 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.pending_history_gaps.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.report_too_large.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.store_fatal.map(|x| x as i64)))
        .bind(spool_store_error.as_deref())
        .bind(spool_backlog.as_ref().map(|age| age.age_ms))
        .bind(spool_backlog.as_ref().map(|age| age.basis.as_str()))
        .bind(received_at)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO agent_spool_diagnostics (agent_id, max_bytes, max_age_seconds, dropped_sequence_from, dropped_sequence_to, dropped_time_from, dropped_time_to, dropped_height_from, dropped_height_to, pending_history_gaps, report_too_large, store_fatal, store_error, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) ON CONFLICT(agent_id) DO UPDATE SET max_bytes=excluded.max_bytes, max_age_seconds=excluded.max_age_seconds, dropped_sequence_from=excluded.dropped_sequence_from, dropped_sequence_to=excluded.dropped_sequence_to, dropped_time_from=excluded.dropped_time_from, dropped_time_to=excluded.dropped_time_to, dropped_height_from=excluded.dropped_height_from, dropped_height_to=excluded.dropped_height_to, pending_history_gaps=excluded.pending_history_gaps, report_too_large=excluded.report_too_large, store_fatal=excluded.store_fatal, store_error=excluded.store_error, updated_at=excluded.updated_at")
        .bind(&agent_id)
        .bind(host.spool.latest.as_ref().and_then(|v| v.capacity_bytes.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.max_age_seconds.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_sequence_range.map(|x| x.0 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_sequence_range.map(|x| x.1 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_time_range.as_ref().map(|x| x.0.to_string())))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_time_range.as_ref().map(|x| x.1.to_string())))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_height_range.map(|x| x.0 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.dropped_height_range.map(|x| x.1 as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.pending_history_gaps.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.report_too_large.map(|x| x as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.store_fatal.map(|x| x as i64)))
        .bind(spool_store_error.as_deref())
        .bind(received_at)
        .execute(&mut **tx)
        .await?;
    if let Some(disk) = host.disk.latest.as_ref() {
        sqlx::query("DELETE FROM current_host_disk_mounts WHERE agent_id = $1")
            .bind(&agent_id)
            .execute(&mut **tx)
            .await?;
        for mount in &disk.mounts {
            sqlx::query("INSERT INTO current_host_disk_mounts (agent_id, mount_path, total_bytes, used_bytes, updated_at) VALUES ($1, $2, $3, $4, $5)")
                .bind(&agent_id).bind(&mount.mount_path).bind(mount.total_bytes as i64).bind(mount.used_bytes as i64).bind(received_at).execute(&mut **tx).await?;
        }
    }
    // Every Host series this Report carries is stored once, under the Agent
    // that owns it: a Node view reads these series through its Agent and keeps
    // no copy of its own (design §11.6, issue #215).
    for (metric, dimension, observed_at, value) in host_series_samples(host) {
        save_metric_sample(
            tx,
            history,
            &crate::metric_history::SeriesScope::host(&agent_id, metric, dimension),
            stamp,
            observed_at,
            &raw_cutoff,
            value,
        )
        .await?;
    }
    for node in &report.nodes {
        let node_id = node.node_id.to_string();
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::Process,
            &node.process,
            stamp,
        )
        .await?;
        if let Some(data_directory_size) = &node.data_directory_size_bytes {
            save_component(
                tx,
                &agent_id,
                "node",
                &node_id,
                Some(&node_id),
                ComponentKey::DataDirectorySizeBytes,
                data_directory_size,
                stamp,
            )
            .await?;
        }
        if let Some(data_directory_capacity) = &node.data_directory_capacity_bytes {
            save_component(
                tx,
                &agent_id,
                "node",
                &node_id,
                Some(&node_id),
                ComponentKey::DataDirectoryCapacityBytes,
                data_directory_capacity,
                stamp,
            )
            .await?;
        }
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::Rpc,
            &node.chain.rpc,
            stamp,
        )
        .await?;
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::Sync,
            &node.chain.sync,
            stamp,
        )
        .await?;
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::Consensus,
            &node.chain.consensus,
            stamp,
        )
        .await?;
        // Issue #217: the sync and consensus states are recorded as evidence of
        // their own. A numeric aggregate cannot hold an Error or Unknown stretch
        // without erasing it, and it cannot say a state stayed the same without
        // inventing a transition, so the state log is an append-only family
        // written beside the projection it describes.
        //
        // It runs after the component saves because the projection is what the
        // Owner actually sees: the instant of a value the Server still holds for
        // a component is read back from its own row, so a failure that carries
        // no value reports the age of the last-good value instead of a fresh
        // instant.
        let sync_state = crate::state_history::StateObservation::of_component(
            &node.chain.sync,
            node.chain.sync.latest.map(|value| value.syncing),
            crate::state_history::retained_value_at(
                tx,
                &agent_id,
                &node_id,
                crate::state_history::COMPONENT_SYNC,
            )
            .await?,
            stamp.history_at(crate::state_history::component_instant(
                &node.chain.sync,
                report.generated_at,
            )),
            received_instant(received_at, report.generated_at),
        );
        crate::state_history::record_state(
            tx,
            history,
            &node_id,
            crate::state_history::COMPONENT_SYNC,
            &sync_state,
            &state_cutoff,
        )
        .await?;
        // Consensus carries no syncing flag of its own: the flag belongs to the
        // sync value, so the consensus state vector never claims one.
        let consensus_state = crate::state_history::StateObservation::of_component(
            &node.chain.consensus,
            None,
            crate::state_history::retained_value_at(
                tx,
                &agent_id,
                &node_id,
                crate::state_history::COMPONENT_CONSENSUS,
            )
            .await?,
            stamp.history_at(crate::state_history::component_instant(
                &node.chain.consensus,
                report.generated_at,
            )),
            received_instant(received_at, report.generated_at),
        );
        crate::state_history::record_state(
            tx,
            history,
            &node_id,
            crate::state_history::COMPONENT_CONSENSUS,
            &consensus_state,
            &state_cutoff,
        )
        .await?;
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::NetworkIdentity,
            &node.chain.network_identity,
            stamp,
        )
        .await?;
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::StaticMetadata,
            &node.chain.static_metadata,
            stamp,
        )
        .await?;
        if let Some(peers) = &node.chain.peers {
            // A failed or unsupported collection keeps the last successful set
            // and its age; only a successful snapshot is authoritative,
            // including an empty list. Repetition of that rule lives in the
            // writers below.
            let snapshot = if peers.status == ComponentStatus::Ok {
                peers.latest.as_ref()
            } else {
                None
            };
            let had_previous_successful_snapshot = match snapshot {
                None => false,
                Some(_) => sqlx::query_scalar::<_, Option<String>>(
                    "SELECT value_received_at FROM component_status WHERE agent_id=$1 AND scope='node' AND scope_key=$2 AND component_key='peers'",
                )
                .bind(&agent_id)
                .bind(&node_id)
                .fetch_optional(&mut **tx)
                .await?
                .flatten()
                .is_some(),
            };
            // The current Peer set is read once per Report: the presence
            // interval boundary and the diff write share that read (#271).
            let stored_peers = match snapshot {
                None => StoredPeerState::default(),
                Some(_) => load_stored_peers(tx, &node_id).await?,
            };
            save_component(
                tx,
                &agent_id,
                "node",
                &node_id,
                Some(&node_id),
                ComponentKey::Peers,
                peers,
                stamp,
            )
            .await?;
            // A Peer Snapshot says what was seen at one instant on the Server
            // clock; the presence interval and the history bucket both belong to
            // that instant, so a replayed snapshot produces its own window instead
            // of one stretched across the catch-up (ADR 0014, #274).
            //
            // The snapshot carries its own observation instant, so the history
            // instant is taken without the receipt-time fallback that freshness
            // allows: borrowing the receipt instant here is exactly the
            // compression of history this rule forbids. Only a snapshot that
            // names no instant at all falls back, and then the receipt instant is
            // the only instant the Server can defend.
            let peers_observed_at = match peers.latest_observed_at.or(peers.attempted_at) {
                Some(observed) => stamp.history_at(observed).to_string(),
                None => stamp.observed_at(None),
            };
            let presence_delta = match snapshot {
                None => PeerPresenceDelta::default(),
                Some(snapshot) => {
                    save_peer_presence(
                        tx,
                        &node_id,
                        snapshot,
                        &peers_observed_at,
                        had_previous_successful_snapshot,
                        &stored_peers,
                    )
                    .await?
                }
            };
            if let Some(snapshot) = snapshot {
                // The current Peer set is a projection write under an idempotent
                // report: it keeps the receipt instant, the freshness of its value
                // comes from the peers component's own observation stamp.
                save_current_peers(tx, &node_id, snapshot, received_at, &stored_peers).await?;
                let local_head = node.chain.sync.latest.map(|sync| sync.current_block);
                crate::peer_history::record_successful_snapshot(
                    tx,
                    &node_id,
                    snapshot,
                    crate::peer_history::SnapshotInstants {
                        observed_at: &peers_observed_at,
                        received_at,
                    },
                    local_head,
                    presence_delta,
                    geo_provider,
                )
                .await?;
            }
        }
        if let Some(process) = node.process.latest {
            sqlx::query("INSERT INTO current_node_process_observations (node_id, pid, started_at, cpu_percent, memory_bytes, uptime_ms, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT(node_id) DO UPDATE SET pid=excluded.pid, started_at=excluded.started_at, cpu_percent=excluded.cpu_percent, memory_bytes=excluded.memory_bytes, uptime_ms=excluded.uptime_ms, updated_at=excluded.updated_at")
                .bind(&node_id).bind(process.pid as i64).bind(process.started_at.to_string()).bind(process.cpu_percent).bind(process.memory_bytes as i64).bind(process.uptime_ms as i64).bind(received_at).execute(&mut **tx).await?;
        }
        if let Some(size_bytes) = node
            .data_directory_size_bytes
            .as_ref()
            .and_then(|observation| observation.latest)
        {
            sqlx::query("INSERT INTO current_node_data_directory_observations (node_id, size_bytes, updated_at) VALUES ($1, $2, $3) ON CONFLICT(node_id) DO UPDATE SET size_bytes=excluded.size_bytes, updated_at=excluded.updated_at")
                .bind(&node_id)
                .bind(size_bytes as i64)
                .bind(received_at)
                .execute(&mut **tx)
                .await?;
        }
        if let Some(capacity_bytes) = node
            .data_directory_capacity_bytes
            .as_ref()
            .and_then(|observation| observation.latest)
        {
            sqlx::query("UPDATE current_node_data_directory_observations SET capacity_bytes=$1, updated_at=$2 WHERE node_id=$3")
                .bind(capacity_bytes as i64)
                .bind(received_at)
                .bind(&node_id)
                .execute(&mut **tx)
                .await?;
        }
        if let (Some(process), Some(observed_at)) =
            (node.process.latest, metric_observed_at(&node.process))
        {
            save_metric_sample(
                tx,
                history,
                &crate::metric_history::SeriesScope::node(&node_id, "process_cpu_percent"),
                stamp,
                observed_at,
                &raw_cutoff,
                process.cpu_percent,
            )
            .await?;
            if let Some(memory) = host.memory.latest.filter(|memory| memory.total_bytes > 0) {
                save_metric_sample(
                    tx,
                    history,
                    &crate::metric_history::SeriesScope::node(&node_id, "process_memory_percent"),
                    stamp,
                    observed_at,
                    &raw_cutoff,
                    process.memory_bytes as f64 * 100.0 / memory.total_bytes as f64,
                )
                .await?;
            }
        }
        if let (Some(size), Some(capacity), Some(size_observed_at), Some(capacity_observed_at)) = (
            node.data_directory_size_bytes
                .as_ref()
                .and_then(|observation| observation.latest),
            node.data_directory_capacity_bytes
                .as_ref()
                .and_then(|observation| observation.latest)
                .filter(|capacity| *capacity > 0),
            node.data_directory_size_bytes
                .as_ref()
                .and_then(metric_observed_at),
            node.data_directory_capacity_bytes
                .as_ref()
                .and_then(metric_observed_at),
        ) {
            // The ratio is only as fresh as the older of the two readings it
            // divides: stamping it at the newer one would present a carried
            // size over a new capacity (or the reverse) as a fresh observation
            // of the percentage.
            save_metric_sample(
                tx,
                history,
                &crate::metric_history::SeriesScope::node(&node_id, "data_directory_percent"),
                stamp,
                size_observed_at.min(capacity_observed_at),
                &raw_cutoff,
                size as f64 * 100.0 / capacity as f64,
            )
            .await?;
        }
        if let Some(peers) = node.chain.peers.as_ref()
            && let (Some(snapshot), Some(observed_at)) =
                (peers.latest.as_ref(), metric_observed_at(peers))
        {
            let inbound = snapshot
                .peers
                .iter()
                .filter(|peer| peer.direction == PeerDirection::Inbound)
                .count();
            let outbound = snapshot.peers.len().saturating_sub(inbound);
            save_metric_sample(
                tx,
                history,
                &crate::metric_history::SeriesScope::node(&node_id, "peer_inbound_count"),
                stamp,
                observed_at,
                &raw_cutoff,
                inbound as f64,
            )
            .await?;
            save_metric_sample(
                tx,
                history,
                &crate::metric_history::SeriesScope::node(&node_id, "peer_outbound_count"),
                stamp,
                observed_at,
                &raw_cutoff,
                outbound as f64,
            )
            .await?;
        }
        // Issue #217: the key sync and consensus heights are numeric history, so
        // they ride the metric engine with the other Node series: the same raw
        // window, the same one-minute and five-minute tiers, the same gaps and
        // the same replay/correction ledger. A probe that failed writes no row
        // here at all, because an absent observation is Unknown rather than a
        // restatement of the last reading; the failure itself is recorded by the
        // state log above.
        //
        // A block height is an integer and a metric sample is a real. Every
        // height a chain can reach is far below 2^53, so the stored value is the
        // height exactly and the history keeps the precision it was measured at.
        // A height beyond that bound would be rounded by the sample: the canonical
        // fixture carries 2^53 + 1 to prove the JSON layer itself loses nothing.
        if let (Some(sync), Some(observed_at)) =
            (node.chain.sync.latest, metric_observed_at(&node.chain.sync))
        {
            for (metric, height) in [
                ("sync_current_block", sync.current_block),
                ("sync_highest_block", sync.highest_block),
            ] {
                save_metric_sample(
                    tx,
                    history,
                    &crate::metric_history::SeriesScope::node(&node_id, metric),
                    stamp,
                    observed_at,
                    &raw_cutoff,
                    height as f64,
                )
                .await?;
            }
        }
        if let (Some(consensus), Some(observed_at)) = (
            node.chain.consensus.latest,
            metric_observed_at(&node.chain.consensus),
        ) {
            for (metric, height) in [
                ("consensus_highest_qc_block", consensus.highest_qc_block),
                ("consensus_highest_lock_block", consensus.highest_lock_block),
                (
                    "consensus_highest_commit_block",
                    consensus.highest_commit_block,
                ),
            ] {
                save_metric_sample(
                    tx,
                    history,
                    &crate::metric_history::SeriesScope::node(&node_id, metric),
                    stamp,
                    observed_at,
                    &raw_cutoff,
                    height as f64,
                )
                .await?;
            }
        }
        if node.chain.rpc.latest.is_some()
            || node.chain.sync.latest.is_some()
            || node.chain.consensus.latest.is_some()
            || node.chain.network_identity.latest.is_some()
            || node.chain.static_metadata.latest.is_some()
        {
            let sync = node.chain.sync.latest;
            let current = sync.map(|v| v.current_block as i64);
            let current_timestamp = report.generated_at.to_string();
            // `observation_at` answers "when was this head seen" and is the instant
            // head freshness and head-lag evidence are judged on; `updated_at` stays
            // the receipt instant because the resync grace window measures how long
            // the Server has been watching the episode (ADR 0014, issue #274).
            let head_observed_at = stamp
                .observed_at(metric_observed_at(&node.chain.sync).or(Some(report.generated_at)));
            // Only a head that actually advances is progress. Every report
            // re-sends the latest sample, so bumping the timestamp
            // unconditionally would turn the "(last progress ...)" diagnostic
            // into a clock instead of an answer.
            sqlx::query("INSERT INTO block_history_state (node_id, current_head, resync_state, resync_last_progress_at, updated_at, observation_at) VALUES ($1, $2, 'normal', $3, $4, $5) ON CONFLICT(node_id) DO UPDATE SET current_head=excluded.current_head, resync_last_progress_at=CASE WHEN excluded.current_head IS NOT NULL AND (block_history_state.current_head IS NULL OR excluded.current_head > block_history_state.current_head) THEN excluded.resync_last_progress_at ELSE block_history_state.resync_last_progress_at END, updated_at=excluded.updated_at, observation_at=excluded.observation_at")
                .bind(&node_id)
                .bind(current)
                .bind(&current_timestamp)
                .bind(received_at)
                .bind(&head_observed_at)
                .execute(&mut **tx)
                .await?;
            // The polled Chain Head can trail the subscribed Block History by
            // the blocks that arrived between polls, so a single
            // below-high-water sample is observation skew, not a resync. An
            // episode has to persist for the grace window before it flips the
            // projection; resync_started_at marks the episode start and is
            // cleared once the head re-reaches the high-water mark.
            // Whether the open episode has outlasted the grace window is judged
            // on parsed instants; an episode or update instant that cannot be
            // parsed has not outlasted it.
            let episode: Option<(Option<String>, String)> = sqlx::query_as(
                "SELECT resync_started_at, updated_at FROM block_history_state WHERE node_id=$1",
            )
            .bind(&node_id)
            .fetch_optional(&mut **tx)
            .await?;
            let grace_elapsed = episode.is_some_and(|(started_at, updated_at)| {
                let started_at = started_at.as_deref().and_then(crate::auth::parse_rfc3339);
                let updated_at = crate::auth::parse_rfc3339(&updated_at);
                matches!((started_at, updated_at), (Some(started_at), Some(updated_at))
                    if updated_at - started_at
                        >= time::Duration::seconds(RESYNC_TRANSITION_GRACE_SECONDS))
            });
            sqlx::query("UPDATE block_history_state SET resync_state=CASE WHEN current_head IS NULL OR historical_high_watermark IS NULL THEN resync_state WHEN current_head >= historical_high_watermark THEN 'normal' WHEN resync_started_at IS NOT NULL AND $1 = 1 THEN 'resyncing' ELSE resync_state END, resync_started_at=CASE WHEN current_head IS NULL OR historical_high_watermark IS NULL THEN resync_started_at WHEN current_head >= historical_high_watermark THEN NULL ELSE COALESCE(resync_started_at, updated_at) END WHERE node_id=$2")
                .bind(i64::from(grace_elapsed))
                .bind(&node_id)
                .execute(&mut **tx)
                .await?;

            let rpc = node.chain.rpc.latest.as_ref();
            let sync = node.chain.sync.latest;
            let consensus = node.chain.consensus.latest;
            let identity = node.chain.network_identity.latest.as_ref();
            let metadata = node.chain.static_metadata.latest.as_ref();
            sqlx::query("INSERT INTO current_node_chain_observations (node_id, rpc_client_version, syncing, current_block, highest_block, pulled_states, known_states, consensus_epoch, consensus_view_number, consensus_validator, consensus_highest_qc_block, consensus_highest_lock_block, consensus_highest_commit_block, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, node_key_fingerprint, enode, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20) ON CONFLICT(node_id) DO UPDATE SET rpc_client_version=COALESCE(excluded.rpc_client_version, current_node_chain_observations.rpc_client_version), syncing=COALESCE(excluded.syncing, current_node_chain_observations.syncing), current_block=COALESCE(excluded.current_block, current_node_chain_observations.current_block), highest_block=COALESCE(excluded.highest_block, current_node_chain_observations.highest_block), pulled_states=COALESCE(excluded.pulled_states, current_node_chain_observations.pulled_states), known_states=COALESCE(excluded.known_states, current_node_chain_observations.known_states), consensus_epoch=COALESCE(excluded.consensus_epoch, current_node_chain_observations.consensus_epoch), consensus_view_number=COALESCE(excluded.consensus_view_number, current_node_chain_observations.consensus_view_number), consensus_validator=COALESCE(excluded.consensus_validator, current_node_chain_observations.consensus_validator), consensus_highest_qc_block=COALESCE(excluded.consensus_highest_qc_block, current_node_chain_observations.consensus_highest_qc_block), consensus_highest_lock_block=COALESCE(excluded.consensus_highest_lock_block, current_node_chain_observations.consensus_highest_lock_block), consensus_highest_commit_block=COALESCE(excluded.consensus_highest_commit_block, current_node_chain_observations.consensus_highest_commit_block), network_genesis_hash=COALESCE(excluded.network_genesis_hash, current_node_chain_observations.network_genesis_hash), network_chain_id=COALESCE(excluded.network_chain_id, current_node_chain_observations.network_chain_id), network_p2p_network_id=COALESCE(excluded.network_p2p_network_id, current_node_chain_observations.network_p2p_network_id), network_address_hrp=COALESCE(excluded.network_address_hrp, current_node_chain_observations.network_address_hrp), node_key_fingerprint=COALESCE(excluded.node_key_fingerprint, current_node_chain_observations.node_key_fingerprint), enode=COALESCE(excluded.enode, current_node_chain_observations.enode), updated_at=excluded.updated_at")
                .bind(&node_id).bind(rpc.map(|v| &v.client_version)).bind(sync.map(|v| v.syncing as i64)).bind(sync.map(|v| v.current_block as i64)).bind(sync.map(|v| v.highest_block as i64)).bind(sync.map(|v| v.pulled_states as i64)).bind(sync.map(|v| v.known_states as i64)).bind(consensus.map(|v| v.epoch as i64)).bind(consensus.map(|v| v.view_number as i64)).bind(consensus.map(|v| v.validator as i64)).bind(consensus.map(|v| v.highest_qc_block as i64)).bind(consensus.map(|v| v.highest_lock_block as i64)).bind(consensus.map(|v| v.highest_commit_block as i64)).bind(identity.map(|v| v.genesis_hash.to_string())).bind(identity.map(|v| v.chain_id as i64)).bind(identity.map(|v| v.p2p_network_id as i64)).bind(identity.and_then(|v| v.address_hrp.as_deref())).bind(metadata.map(|v| v.node_key_fingerprint.to_string())).bind(metadata.and_then(|v| v.enode.as_deref())).bind(received_at).execute(&mut **tx).await?;
            if let Some(rpc) = rpc {
                sqlx::query("DELETE FROM current_node_rpc_namespaces WHERE node_id = $1")
                    .bind(&node_id)
                    .execute(&mut **tx)
                    .await?;
                for chunk in rpc.namespaces.chunks(RPC_INSERT_ROWS) {
                    let sql = crate::database::number_placeholders(&format!(
                        "INSERT INTO current_node_rpc_namespaces (node_id, namespace, updated_at) VALUES {}",
                        crate::database::row_placeholders(3, chunk.len())
                    ));
                    let mut statement = sqlx::query(&sql);
                    for namespace in chunk {
                        statement = statement.bind(&node_id).bind(namespace).bind(received_at);
                    }
                    statement.execute(&mut **tx).await?;
                }
                sqlx::query("DELETE FROM current_node_rpc_methods WHERE node_id = $1")
                    .bind(&node_id)
                    .execute(&mut **tx)
                    .await?;
                for chunk in rpc.methods.chunks(RPC_INSERT_ROWS) {
                    let sql = crate::database::number_placeholders(&format!(
                        "INSERT INTO current_node_rpc_methods (node_id, method, updated_at) VALUES {}",
                        crate::database::row_placeholders(3, chunk.len())
                    ));
                    let mut statement = sqlx::query(&sql);
                    for method in chunk {
                        statement = statement.bind(&node_id).bind(method).bind(received_at);
                    }
                    statement.execute(&mut **tx).await?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    use crate::http::report_ingestion::test_helpers::*;

    use std::sync::Arc;

    use axum::extract::{Extension, State};

    use crate::auth::{format_rfc3339, now_utc};

    use crate::http::{AppState, RequestId};

    use crate::network::create_network;

    use platpulse_core::component::{ComponentKey, ComponentObservation, ComponentStatus};

    use platpulse_core::{AgentReport, ReceiptDisposition};

    /// The report fixture with every instant moved onto the Server clock.
    pub(super) fn report_near_now() -> AgentReport {
        let mut raw: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        restamp_fixture(&mut raw, now_utc());
        serde_json::from_value(raw).unwrap()
    }

    #[tokio::test]
    pub(super) async fn persists_node_data_directory_size_and_preserves_last_good_value() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_near_now();
        let observed_at = report.generated_at;
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(12_884_901_888),
            error: None,
        });
        report.nodes[0].data_directory_capacity_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(51_539_607_552),
            error: None,
        });

        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert!(
            receipt.nodes[0]
                .accepted_component_revisions
                .iter()
                .any(|revision| revision.component == ComponentKey::DataDirectorySizeBytes)
        );
        assert!(
            receipt.nodes[0]
                .accepted_component_revisions
                .iter()
                .any(|revision| revision.component == ComponentKey::DataDirectoryCapacityBytes)
        );
        let stored: (i64, Option<i64>) = sqlx::query_as(
            "SELECT size_bytes, capacity_bytes FROM current_node_data_directory_observations WHERE node_id=$1",
        )
        .bind(report.nodes[0].node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, (12_884_901_888, Some(51_539_607_552)));
        let disk_sample: (f64, String) = sqlx::query_as(
            "SELECT value, received_at FROM node_metric_samples WHERE node_id=$1 AND metric='data_directory_percent'",
        )
        .bind(report.nodes[0].node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(disk_sample.0, 25.0);
        assert!(!disk_sample.1.is_empty());
        let host_network_samples: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM host_metric_samples WHERE agent_id=$1 AND metric IN ('network_rx_bytes_per_sec', 'network_tx_bytes_per_sec')",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(host_network_samples, 2);

        report.report_sequence += 1;
        report.report_id = "0195f2a1-0090-4090-8090-000000000090".parse().unwrap();
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Error,
            attempted_at: Some(observed_at),
            latest_observed_at: None,
            received_at: None,
            state_revision: 2,
            value_revision: 1,
            latest: None,
            error: Some(platpulse_core::BoundedError {
                code: "data_directory_scan_failed".to_owned(),
                message: "PlatON data directory could not be measured".to_owned(),
            }),
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let stored: i64 = sqlx::query_scalar(
            "SELECT size_bytes FROM current_node_data_directory_observations WHERE node_id=$1",
        )
        .bind(report.nodes[0].node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, 12_884_901_888);

        report.report_sequence += 1;
        report.report_id = "0195f2a1-0091-4091-8091-000000000091".parse().unwrap();
        report.nodes[0].data_directory_size_bytes = None;
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let stored: i64 = sqlx::query_scalar(
            "SELECT size_bytes FROM current_node_data_directory_observations WHERE node_id=$1",
        )
        .bind(report.nodes[0].node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, 12_884_901_888);
    }

    /// Issue #213: the series ledger counts what the series observed. A
    /// restated delivery — the same observation arriving again, including a
    /// carried last-good — is not a new observation, and a restated value that
    /// really did change is counted as a correction instead.
    #[tokio::test]
    pub(super) async fn replay_and_correction_never_inflate_the_series_observation_count() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report = report_near_now();
        let node_id = report.nodes[0].node_id.to_string();
        let observed_at = report.generated_at;
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(12_884_901_888),
            error: None,
        });
        report.nodes[0].data_directory_capacity_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(51_539_607_552),
            error: None,
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 0, 0)
        );

        // The same observation delivered again in a new Report: the value is
        // already stored, so this is a replay and not a second observation.
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0092-4092-8092-000000000092".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 1, 0)
        );
        let stored_series: (String, String) = sqlx::query_as(
            "SELECT first_observed_at, last_observed_at FROM node_metric_series_state WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored_series.0, observed_at.to_string());
        assert_eq!(stored_series.1, observed_at.to_string());

        // The same instant observed with a different value is a correction: it
        // rewrites the sample without inventing an observation or advancing the
        // series clock.
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0093-4093-8093-000000000093".parse().unwrap();
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 2,
            value_revision: 2,
            latest: Some(25_769_803_776),
            error: None,
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 1, 1)
        );

        // A genuinely newer observation is the only thing that advances the
        // count and the series clock.
        let later: platpulse_core::Rfc3339 =
            format_rfc3339(observed_at.as_datetime() + time::Duration::minutes(5))
                .parse()
                .unwrap();
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0094-4094-8094-000000000094".parse().unwrap();
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(later),
            latest_observed_at: Some(later),
            received_at: None,
            state_revision: 3,
            value_revision: 3,
            latest: Some(12_884_901_888),
            error: None,
        });
        report.nodes[0].data_directory_capacity_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(later),
            latest_observed_at: Some(later),
            received_at: None,
            state_revision: 3,
            value_revision: 3,
            latest: Some(51_539_607_552),
            error: None,
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (2, 1, 1)
        );
        let samples: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_metric_samples WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(samples, 2, "a replay must not add a second sample row");
        let last_observed: String = sqlx::query_scalar(
            "SELECT last_observed_at FROM node_metric_series_state WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(last_observed, later.to_string());
    }

    /// The scenario the review raised: an observation is retained, released by a
    /// cleanup, and then the Operator widens the retention window past it. Its
    /// row is gone from the sample table, so the only evidence that can keep a
    /// carried copy from being counted a second time is the series'
    /// released-before floor, and that floor has to survive a restart.
    #[tokio::test]
    pub(super) async fn a_retention_widening_cannot_recount_an_observation_it_released() {
        let (dir, state, agent_id) = state_with_agent().await;
        let mut report = report_near_now();
        let node_id = report.nodes[0].node_id.to_string();
        let observed_at = report.generated_at;
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(12_884_901_888),
            error: None,
        });
        report.nodes[0].data_directory_capacity_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(51_539_607_552),
            error: None,
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 0, 0)
        );

        // Time passes. The cleanup runs with a clock far enough ahead that the
        // real one-day cutoff reaches the stored observation: it releases the
        // row and stamps the series with the cutoff it applied.
        let released_at = now_utc() + time::Duration::days(2);
        let removed =
            crate::retention::cleanup_expired_metric_samples(state.db().pool(), released_at)
                .await
                .unwrap();
        assert!(removed >= 1, "the cleanup released the stored observation");
        let released_before: String = sqlx::query_scalar(
            "SELECT released_before FROM node_metric_series_state WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(
            released_before.as_str() > observed_at.to_string().as_str(),
            "the series is stamped with a cutoff the released observation is below: {released_before}"
        );

        // The Operator then widens the window to two days. The observation is
        // inside the widened window again while its row is gone, so the current
        // cutoff on its own would call the carried copy a brand new observation.
        sqlx::query(
            "UPDATE retention_policies SET retention_days = 2 WHERE family = 'raw_metric_sample'",
        )
        .execute(state.db().pool())
        .await
        .unwrap();

        // Restart: the floor is durable evidence, not a memory of this process.
        // The old Server has to let go of the file first, exactly as a restart
        // would.
        state.db().pool().close().await;
        drop(state);
        let state = restart(&dir).await;
        let after_restart: String = sqlx::query_scalar(
            "SELECT released_before FROM node_metric_series_state WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(after_restart, released_before);

        // The Agent carries its last good reading in a new Report. It is the
        // observation that was already counted, so it is a replay: the lifetime
        // count does not move, and no row comes back for it. Recreating the row
        // would restamp receipt evidence for an instant the Server released on
        // purpose, and a carry of adjacent expired instants would rebuild an
        // interval the retention window deliberately stopped holding.
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0095-4095-8095-000000000095".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 1, 0)
        );
        let samples: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_metric_samples WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            samples, 0,
            "a replay never writes a row back, however wide the window is: the count of observations is what a replay must never move"
        );
    }

    /// The reviewer's second scenario: the very first observation of a series is
    /// older than the raw window, so it is counted while no row can answer for
    /// it. Counting it is what leaves an evidence floor behind; without that
    /// floor a later widening of the window over the same instant would turn the
    /// Agent's carried last good reading into a brand new observation.
    #[tokio::test]
    pub(super) async fn an_observation_counted_below_the_window_is_not_recounted_by_a_widening() {
        let (dir, state, agent_id) = state_with_agent().await;
        let mut report = report_near_now();
        let node_id = report.nodes[0].node_id.to_string();
        // Forty hours back: outside the one-day window, inside a two-day one.
        let observed_at = format_rfc3339(now_utc() - time::Duration::hours(40));
        report.generated_at = observed_at.parse().unwrap();
        report.nodes[0].data_directory_size_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(report.generated_at),
            latest_observed_at: Some(report.generated_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(12_884_901_888),
            error: None,
        });
        report.nodes[0].data_directory_capacity_bytes = Some(ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(report.generated_at),
            latest_observed_at: Some(report.generated_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(51_539_607_552),
            error: None,
        });
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 0, 0),
            "the first observation of a series is counted even though the window cannot hold it"
        );
        let samples: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM node_metric_samples WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(samples, 0, "and no row can answer for it");
        let floor: String = sqlx::query_scalar(
            "SELECT released_before FROM node_metric_series_state WHERE node_id = $1 AND metric = 'data_directory_percent'",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(
            floor.as_str() > observed_at.as_str(),
            "counting an observation nothing holds leaves an evidence floor above it: {floor}"
        );

        // The Operator widens the window past the instant. Its row never comes
        // back, so the floor is the only thing that can refuse a second count.
        sqlx::query(
            "UPDATE retention_policies SET retention_days = 2 WHERE family = 'raw_metric_sample'",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        // Restart: the floor is durable evidence, not a memory of this process.
        state.db().pool().close().await;
        drop(state);
        let state = restart(&dir).await;

        report.report_sequence += 1;
        report.report_id = "0195f2a1-0096-4096-8096-000000000096".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(
            metric_ledger(state.db().pool(), &node_id, "data_directory_percent").await,
            (1, 1, 0),
            "the widened window reads the carried instant again, it does not count it again"
        );
    }

    pub(super) async fn metric_ledger(
        pool: &sqlx::AnyPool,
        node_id: &str,
        metric: &str,
    ) -> (i64, i64, i64) {
        sqlx::query_as(
            "SELECT observation_count, replayed_count, corrected_count FROM node_metric_series_state WHERE node_id = $1 AND metric = $2",
        )
        .bind(node_id)
        .bind(metric)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    pub(super) async fn admin_agent_json(state: &AppState, agent_id: &str) -> serde_json::Value {
        let response = crate::http::admin::admin_agent_detail(
            State(state.clone()),
            axum::extract::Path(agent_id.to_owned()),
            Extension(crate::http::AuthenticatedSession(
                crate::auth::SessionInfo {
                    session_id: "session".to_owned(),
                    user_id: "owner".to_owned(),
                    username: "owner".to_owned(),
                    role: "owner".to_owned(),
                    created_at: crate::auth::now_utc(),
                    last_seen_at: crate::auth::now_utc(),
                    expires_at: crate::auth::now_utc(),
                    csrf_token: "csrf".to_owned(),
                },
            )),
            Extension(RequestId(Arc::from("test-request"))),
        )
        .await;
        body_json(response).await
    }

    pub(super) fn disabled_process()
    -> ComponentObservation<platpulse_core::observation::ProcessCurrent> {
        ComponentObservation {
            status: ComponentStatus::Disabled,
            attempted_at: None,
            latest_observed_at: None,
            received_at: None,
            state_revision: 1,
            value_revision: 0,
            latest: None,
            error: None,
        }
    }

    #[tokio::test]
    pub(super) async fn supervisor_process_selector_projects_real_values_and_keeps_last_good() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let node_id = report.inventory.nodes[0].node_id.to_string();
        report.inventory.nodes[0].process = Some(platpulse_core::ProcessSelector::Supervisor {
            program: "validators:platon-validator-a".into(),
        });
        report.agent_capabilities = vec![platpulse_core::AgentCapability::ProcessSupervisor];
        let observed_at = report.generated_at;
        report.nodes[0].process = ComponentObservation {
            status: ComponentStatus::Ok,
            attempted_at: Some(observed_at),
            latest_observed_at: Some(observed_at),
            received_at: None,
            state_revision: 1,
            value_revision: 1,
            latest: Some(platpulse_core::observation::ProcessCurrent {
                pid: 4242,
                started_at: observed_at,
                cpu_percent: 12.5,
                memory_bytes: 268_435_456,
                uptime_ms: 86_400_000,
            }),
            error: None,
        };
        report.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        // Ingestion persisted the supervisor observation before projection.
        let stored: (i64, f64, i64, i64) = sqlx::query_as(
            "SELECT pid, cpu_percent, memory_bytes, uptime_ms FROM current_node_process_observations WHERE node_id=$1",
        )
        .bind(&node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(stored, (4242, 12.5, 268_435_456, 86_400_000));

        // Public and Admin both render real values instead of Unknown.
        let public = public_home_node(&state, &node_id).await;
        assert_eq!(public["processState"], "ok");
        assert_eq!(public["processCpuPercent"], 12.5);
        assert_eq!(public["processUptimeMs"], 86_400_000);
        assert_eq!(public["processStartedAt"], observed_at.to_string());
        assert!(public["processMemoryPercent"].as_f64().unwrap() > 0.0);
        let admin = admin_agent_json(&state, &agent_id).await;
        let process = &admin["nodes"][0]["process"];
        assert_eq!(process["state"], "ok");
        assert_eq!(process["pid"], 4242);
        assert_eq!(process["cpu_percent"], 12.5);
        assert_eq!(process["uptime_ms"], 86_400_000);

        // A failed collection keeps the last-good value with an explicit error.
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0091-4091-8091-000000000091".parse().unwrap();
        report.nodes[0].process = ComponentObservation {
            status: ComponentStatus::Error,
            attempted_at: Some(observed_at),
            latest_observed_at: None,
            received_at: None,
            state_revision: 2,
            value_revision: 1,
            latest: None,
            error: Some(platpulse_core::BoundedError {
                code: "process_selector_error".into(),
                message: "supervisor control command failed".into(),
            }),
        };
        report.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        let public = public_home_node(&state, &node_id).await;
        assert_eq!(public["processState"], "error");
        assert_eq!(public["processCpuPercent"], 12.5);
        assert_eq!(public["processUptimeMs"], 86_400_000);
        let admin = admin_agent_json(&state, &agent_id).await;
        let process = &admin["nodes"][0]["process"];
        assert_eq!(process["state"], "error");
        assert_eq!(process["error_code"], "process_selector_error");
        assert_eq!(process["cpu_percent"], 12.5);

        // A Node with no selector stays Disabled: unknown is never rendered as 0.
        let second_id = "0195f2a1-0015-4015-8015-000000000015".parse().unwrap();
        let mut second = report.inventory.nodes[0].clone();
        second.node_id = second_id;
        second.rpc_endpoint = "ws://127.0.0.1:6791".parse().unwrap();
        second.process = None;
        report.inventory.nodes.push(second);
        report.inventory.revision = 2;
        report.report_sequence += 1;
        report.report_id = "0195f2a1-0092-4092-8092-000000000092".parse().unwrap();
        let mut second_obs = report.nodes[0].clone();
        second_obs.node_id = second_id;
        second_obs.process = disabled_process();
        report.nodes.push(second_obs);
        report.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;

        let public = public_home_node(&state, &second_id.to_string()).await;
        assert_eq!(public["processState"], "disabled");
        assert!(public["processCpuPercent"].is_null());
        assert!(public["processUptimeMs"].is_null());
        let admin = admin_agent_json(&state, &agent_id).await;
        let second_diagnostic = admin["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["node_id"] == second_id.to_string())
            .unwrap();
        assert_eq!(second_diagnostic["process"]["state"], "disabled");
        assert!(second_diagnostic["process"]["pid"].is_null());
        assert!(second_diagnostic["process"]["cpu_percent"].is_null());
    }

    #[tokio::test]
    pub(super) async fn two_node_inventory_keeps_host_scoped_once_and_nodes_separate() {
        let (_dir, state, agent_id) = state_with_agent().await;
        create_network(
            state.db(),
            "platon-testnet",
            "PlatON Testnet",
            "0x0000000000000000000000000000000000000000000000000000000000000002",
            210426,
            210426,
            "lat",
        )
        .await
        .unwrap();
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let first_node = report.inventory.nodes[0].clone();
        let first_observation = report.nodes[0].clone();
        let second_id = "0195f2a1-0015-4015-8015-000000000015".parse().unwrap();
        let mut second_node = first_node;
        second_node.node_id = second_id;
        second_node.network_key = "platon-testnet".parse().unwrap();
        second_node.rpc_endpoint = "ws://127.0.0.1:6791".parse().unwrap();
        let mut second_observation = first_observation;
        second_observation.node_id = second_id;
        second_observation
            .chain
            .rpc
            .latest
            .as_mut()
            .unwrap()
            .client_version = "fake-platon/testnet".to_owned();
        report.inventory.nodes.push(second_node);
        report.nodes.push(second_observation);
        report.report_id = "0195f2a1-0013-4013-8013-000000000015".parse().unwrap();
        report.validate().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
        assert_eq!(receipt.nodes.len(), 2);
        let node_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE agent_id = $1")
            .bind(&agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(node_count, 2);
        let host_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM current_host_observations WHERE agent_id = $1",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(host_count, 1);
        let node_rpc_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM component_status WHERE agent_id = $1 AND scope = 'node' AND component_key = 'rpc'",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(node_rpc_count, 2);
        let node_identity_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM component_status WHERE agent_id = $1 AND scope = 'node' AND component_key = 'network_identity'",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            node_identity_count, 2,
            "accepted Network Identity observations use the stable snake_case component key"
        );
        let clients: Vec<String> = sqlx::query_scalar(
            "SELECT rpc_client_version FROM current_node_chain_observations ORDER BY node_id",
        )
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            clients,
            vec![
                "PlatON/v1.4.0-unstable/linux-amd64/go1.21.1",
                "fake-platon/testnet"
            ]
        );
    }

    #[tokio::test]
    pub(super) async fn accepted_empty_inventory_retires_omitted_node() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&original).unwrap()).await;
        let mut empty = original;
        empty.report_sequence = 2;
        empty.report_id = "0195f2a1-0013-4013-8013-000000000099".parse().unwrap();
        empty.inventory.revision = 2;
        empty.inventory.nodes.clear();
        empty.nodes.clear();
        submit(&state, &agent_id, serde_json::to_vec(&empty).unwrap()).await;
        let lifecycle: String =
            sqlx::query_scalar("SELECT lifecycle FROM nodes WHERE node_id = $1")
                .bind("0195f2a1-0014-4014-8014-000000000014")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(lifecycle, "retired");
        assert!(
            state
                .public_realtime()
                .pending_events()
                .iter()
                .find(|event| event.resource == "node" && event.resource_id.is_none())
                .is_some_and(|event| event.revision == event.event_id && event.revision > 0)
        );

        let mut stale = empty.clone();
        stale.report_sequence = 3;
        stale.report_id = "0195f2a1-0013-4013-8013-000000000098".parse().unwrap();
        stale.inventory.revision = 1;
        let stale_receipt = submit(&state, &agent_id, serde_json::to_vec(&stale).unwrap()).await;
        assert_eq!(stale_receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            stale_receipt.rejections[0].code,
            platpulse_core::RejectionCode::InventoryRevisionConflict
        );

        let mut restored: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        restored.report_sequence = 4;
        restored.report_id = "0195f2a1-0013-4013-8013-000000000100".parse().unwrap();
        restored.inventory.revision = 3;
        submit(&state, &agent_id, serde_json::to_vec(&restored).unwrap()).await;
        let lifecycle: String =
            sqlx::query_scalar("SELECT lifecycle FROM nodes WHERE node_id = $1")
                .bind("0195f2a1-0014-4014-8014-000000000014")
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(lifecycle, "active");
    }

    #[tokio::test]
    pub(super) async fn current_head_regresses_without_rewriting_history_and_restart_preserves_state()
     {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut first: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let node_id = first.inventory.nodes[0].node_id;
        first.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .current_block = 100;
        first.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .highest_block = 100;
        first.report_id = "0195f2a1-0013-4013-8013-000000000104".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;
        // A below-high-water episode is already underway: a real resync lasts
        // far longer than the grace window, so the regression is classified as
        // resyncing rather than a transient poll/subscription skew.
        sqlx::query("UPDATE block_history_state SET historical_high_watermark=100, cumulative_block_count=4, resync_started_at='2026-08-12T08:00:00Z' WHERE node_id=$1")
            .bind(node_id.to_string()).execute(state.db().pool()).await.unwrap();
        let mut replay = first.clone();
        replay.report_sequence = 2;
        replay.report_id = "0195f2a1-0013-4013-8013-000000000105".parse().unwrap();
        replay.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .current_block = 4;
        replay.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .highest_block = 4;
        submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;
        let current: i64 =
            sqlx::query_scalar("SELECT current_head FROM block_history_state WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        let high: i64 = sqlx::query_scalar(
            "SELECT historical_high_watermark FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let cumulative: i64 = sqlx::query_scalar(
            "SELECT cumulative_block_count FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let state_name: String =
            sqlx::query_scalar("SELECT resync_state FROM block_history_state WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(current, 4);
        assert_eq!(high, 100);
        assert_eq!(cumulative, 4);
        assert_eq!(state_name, "resyncing");
        // The restart asks for the same directory, so the shared helper hands
        // back the same database on either Storage Backend (ADR 0013).
        // A restart is a new process: the previous connection is gone before
        // the next one opens. The Server holds exclusive locking, so the
        // AppState-owned pool must be closed here to model that.
        state.db().close().await;
        let reopened = crate::test_support::test_database_in(_dir.path()).await;
        let persisted: (i64, i64, String) = sqlx::query_as("SELECT current_head, historical_high_watermark, resync_state FROM block_history_state WHERE node_id=$1").bind(node_id.to_string()).fetch_one(reopened.pool()).await.unwrap();
        assert_eq!(persisted, (4, 100, "resyncing".to_owned()));
    }

    #[tokio::test]
    pub(super) async fn transient_below_high_water_sample_does_not_flap_resync() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut first: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let node_id = first.inventory.nodes[0].node_id;
        first.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .current_block = 100;
        first.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .highest_block = 100;
        first.report_id = "0195f2a1-0013-4013-8013-000000000204".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;
        sqlx::query("UPDATE block_history_state SET historical_high_watermark=100, cumulative_block_count=4 WHERE node_id=$1")
            .bind(node_id.to_string()).execute(state.db().pool()).await.unwrap();

        // The subscription observed a block that the polled Chain Head had not
        // returned yet. That is observation skew, not a resync, and a healthy
        // Node must not flap on it.
        let mut replay = first.clone();
        replay.report_sequence = 2;
        replay.report_id = "0195f2a1-0013-4013-8013-000000000205".parse().unwrap();
        replay.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .current_block = 4;
        replay.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .highest_block = 4;
        submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;

        let (state_name, episode_started_at, progress_at): (
            String,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT resync_state, resync_started_at, resync_last_progress_at FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(state_name, "normal");
        assert!(
            episode_started_at.is_some(),
            "a below-high-water sample still opens an episode clock"
        );
        assert_eq!(
            progress_at.as_deref(),
            Some("2026-08-12T09:00:00Z"),
            "a regressed head must not advance the last-progress time"
        );
    }

    #[tokio::test]
    pub(super) async fn sustained_below_high_water_marks_resyncing_and_catch_up_clears_it() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut base: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let node_id = base.inventory.nodes[0].node_id;
        base.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .current_block = 100;
        base.nodes[0]
            .chain
            .sync
            .latest
            .as_mut()
            .unwrap()
            .highest_block = 100;
        base.report_id = "0195f2a1-0013-4013-8013-000000000304".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&base).unwrap()).await;
        sqlx::query("UPDATE block_history_state SET historical_high_watermark=100, cumulative_block_count=4 WHERE node_id=$1")
            .bind(node_id.to_string()).execute(state.db().pool()).await.unwrap();

        let body = |sequence: u64, report_id: &str, head: u64| {
            let mut report = base.clone();
            report.report_sequence = sequence;
            report.report_id = report_id.parse().unwrap();
            report.nodes[0]
                .chain
                .sync
                .latest
                .as_mut()
                .unwrap()
                .current_block = head;
            report.nodes[0]
                .chain
                .sync
                .latest
                .as_mut()
                .unwrap()
                .highest_block = head;
            serde_json::to_vec(&report).unwrap()
        };

        // First below-high-water report: the episode opens but stays normal.
        submit(
            &state,
            &agent_id,
            body(2, "0195f2a1-0013-4013-8013-000000000305", 4),
        )
        .await;
        // Age the episode past the grace window, then report the same head.
        sqlx::query("UPDATE block_history_state SET resync_started_at='2026-08-12T08:00:00Z' WHERE node_id=$1")
            .bind(node_id.to_string()).execute(state.db().pool()).await.unwrap();
        submit(
            &state,
            &agent_id,
            body(3, "0195f2a1-0013-4013-8013-000000000306", 4),
        )
        .await;
        let state_name: String =
            sqlx::query_scalar("SELECT resync_state FROM block_history_state WHERE node_id=$1")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(state_name, "resyncing");

        // Re-reaching the high-water mark clears the episode.
        submit(
            &state,
            &agent_id,
            body(4, "0195f2a1-0013-4013-8013-000000000307", 101),
        )
        .await;
        let cleared: (String, Option<String>) = sqlx::query_as(
            "SELECT resync_state, resync_started_at FROM block_history_state WHERE node_id=$1",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(cleared, ("normal".to_owned(), None));
    }

    #[tokio::test]
    pub(super) async fn failed_node_keeps_host_and_other_node_projection() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let second_id = "0195f2a1-0015-4015-8015-000000000015".parse().unwrap();
        let mut second = report.inventory.nodes[0].clone();
        second.node_id = second_id;
        second.rpc_endpoint = "ws://127.0.0.1:6791".parse().unwrap();
        report.inventory.nodes.push(second);
        let mut second_obs = report.nodes[0].clone();
        second_obs.node_id = second_id;
        second_obs.chain.rpc.latest.as_mut().unwrap().client_version = "other".into();
        report.nodes.push(second_obs);
        report.report_id = "0195f2a1-0013-4013-8013-000000000102".parse().unwrap();
        report.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let mut failed = report.clone();
        failed.report_sequence = 2;
        failed.report_id = "0195f2a1-0013-4013-8013-000000000103".parse().unwrap();
        failed.nodes[0].chain.rpc.status = ComponentStatus::Error;
        failed.nodes[0].chain.rpc.latest = None;
        failed.nodes[0].chain.rpc.latest_observed_at = None;
        failed.nodes[0].chain.rpc.value_revision = 0;
        failed.nodes[0].chain.rpc.error = Some(platpulse_core::component::BoundedError {
            code: "rpc_unreachable".into(),
            message: "RPC probe failed".into(),
        });
        failed.validate().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&failed).unwrap()).await;
        let client: String = sqlx::query_scalar(
            "SELECT rpc_client_version FROM current_node_chain_observations WHERE node_id = $1",
        )
        .bind(second_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(client, "other");
        let memory: i64 = sqlx::query_scalar(
            "SELECT memory_used_bytes FROM current_host_observations WHERE agent_id = $1",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(memory, 4_294_967_296);
        let state: String = sqlx::query_scalar(
            "SELECT state FROM component_status WHERE node_id = $1 AND component_key = 'rpc'",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(state, "error");
    }
}
