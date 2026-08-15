//! Transactional AgentReport ingestion for the first Inventory vertical slice.
use std::str::FromStr;

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Extension, Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Sqlite, Transaction};

use crate::enrollment::AgentAuthInfo;
use crate::http::{AppState, ROUTE_GROUP_HEADER, RequestId};
use platpulse_core::component::{ComponentKey, ComponentObservation, ComponentStatus};
use platpulse_core::protocol::SUPPORTED_PROTOCOL_MAJORS;
use platpulse_core::{
    AgentReport, ComponentRevision, InventoryDisposition, NodeCurrentDisposition, NodeReceipt,
    ReceiptDisposition, ReportId, ReportReceipt, Rfc3339, SampleDisposition, SampleDispositionKind,
    SampleRef, Sha256Hex,
};

#[derive(Debug, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReportResponse {
    pub receipt: ReportReceipt,
}

#[derive(Debug, FromRow)]
struct ReceiptRow {
    report_body_sha256: String,
    receipt_body: Vec<u8>,
}

#[derive(Debug, FromRow)]
struct AgentRow {
    agent_epoch: i64,
    active_boot_id: Option<String>,
    active_boot_status: String,
    previous_boot_id: Option<String>,
    close_report_id: Option<String>,
    last_report_sequence: Option<i64>,
    last_inventory_revision: i64,
}

fn now() -> Rfc3339 {
    crate::auth::format_rfc3339(crate::auth::now_utc())
        .parse()
        .expect("formatted timestamp is valid")
}

fn rejection(code: platpulse_core::RejectionCode, reason: &str) -> platpulse_core::Rejection {
    platpulse_core::Rejection {
        code,
        retryable: code.is_retryable(),
        reason: reason.to_owned(),
    }
}

fn disposition_name(disposition: ReceiptDisposition) -> &'static str {
    match disposition {
        ReceiptDisposition::Accepted => "accepted",
        ReceiptDisposition::PartiallyAccepted => "partially_accepted",
        ReceiptDisposition::Rejected => "rejected",
    }
}
fn rejected(
    report_id: ReportId,
    hash: Sha256Hex,
    code: platpulse_core::RejectionCode,
    reason: &str,
) -> ReportReceipt {
    ReportReceipt {
        report_id,
        disposition: ReceiptDisposition::Rejected,
        report_body_sha256: hash,
        server_version: crate::VERSION.to_owned(),
        supported_protocol_majors: SUPPORTED_PROTOCOL_MAJORS.to_vec(),
        server_time: now(),
        rotation_hint: None,
        inventory: Some(InventoryDisposition::Rejected),
        rejections: vec![rejection(code, reason)],
        nodes: vec![],
        samples: vec![],
    }
}

fn error(
    request_id: &str,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    (
        status,
        Json(crate::http::ApiErrorBody::new(code, message, request_id)),
    )
        .into_response()
}

async fn store_rejected(
    mut tx: Transaction<'_, Sqlite>,
    report: &AgentReport,
    hash: Sha256Hex,
    receipt: ReportReceipt,
    request_id: &str,
) -> Response {
    let stored = serde_json::to_vec(&receipt).expect("receipt serializes");
    let result = sqlx::query("INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(report.report_id.to_string()).bind(report.agent_id.to_string())
        .bind(report.agent_epoch as i64).bind(report.boot_id.to_string())
        .bind(report.report_sequence as i64).bind(hash.to_string())
        .bind(disposition_name(receipt.disposition)).bind(&stored)
        .bind(now().to_string()).execute(&mut *tx).await;
    if result.is_err() || tx.commit().await.is_err() {
        return error(
            request_id,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    (StatusCode::OK, Json(ReportResponse { receipt })).into_response()
}

async fn record_security_event(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agents SET security_event_count=security_event_count+1 WHERE agent_id=?")
        .bind(agent_id)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}

fn status_name(status: ComponentStatus) -> &'static str {
    match status {
        ComponentStatus::Starting => "starting",
        ComponentStatus::Ok => "ok",
        ComponentStatus::Error => "error",
        ComponentStatus::Disabled => "disabled",
        ComponentStatus::Unsupported => "unsupported",
    }
}

#[allow(clippy::too_many_arguments)]
async fn save_component<T: serde::Serialize>(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
    scope: &str,
    scope_key: &str,
    node_id: Option<&str>,
    key: ComponentKey,
    component: &ComponentObservation<T>,
    received_at: &str,
) -> Result<(), sqlx::Error> {
    let error_code = component.error.as_ref().map(|e| e.code.as_str());
    let error_message = component.error.as_ref().map(|e| e.message.as_str());
    sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision, error_code, error_message) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(agent_id, scope, scope_key, component_key) DO UPDATE SET state=excluded.state, attempted_at=excluded.attempted_at, observed_at=COALESCE(excluded.observed_at, component_status.observed_at), received_at=excluded.received_at, state_revision=excluded.state_revision, value_revision=CASE WHEN excluded.value_revision > 0 THEN excluded.value_revision ELSE component_status.value_revision END, error_code=excluded.error_code, error_message=excluded.error_message")
        .bind(agent_id).bind(scope).bind(scope_key).bind(node_id).bind(format!("{key:?}").to_lowercase())
        .bind(status_name(component.status)).bind(component.attempted_at.map(|v| v.to_string()))
        .bind(component.latest_observed_at.map(|v| v.to_string())).bind(received_at)
        .bind(component.state_revision as i64).bind(component.value_revision as i64)
        .bind(error_code).bind(error_message).execute(&mut **tx).await?;
    Ok(())
}

async fn open_coverage_gap(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    from_height: u64,
    to_height: u64,
    created_at: &str,
) -> Result<(), sqlx::Error> {
    let overlap = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT coverage_id, first_height, last_height FROM block_coverage_intervals WHERE node_id=? AND status='open_recoverable_gap' AND first_height <= ? AND last_height >= ? ORDER BY first_height LIMIT 1",
    )
    .bind(node_id).bind(to_height as i64).bind(from_height as i64)
    .fetch_optional(&mut **tx).await?;
    if let Some((coverage_id, _first, _last)) = overlap {
        sqlx::query("UPDATE block_coverage_intervals SET first_height=MIN(first_height,?), last_height=MAX(last_height,?), updated_at=? WHERE coverage_id=?")
            .bind(from_height as i64).bind(to_height as i64).bind(created_at).bind(coverage_id)
            .execute(&mut **tx).await?;
    } else {
        sqlx::query("INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES (?, ?, ?, 'open_recoverable_gap', ?, ?)")
            .bind(node_id).bind(from_height as i64).bind(to_height as i64).bind(created_at).bind(created_at)
            .execute(&mut **tx).await?;
    }
    Ok(())
}

async fn append_coverage_height(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    height: u64,
    created_at: &str,
) -> Result<(), sqlx::Error> {
    let previous = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT coverage_id, first_height, last_height FROM block_coverage_intervals WHERE node_id=? AND status='covered' AND last_height < ? ORDER BY last_height DESC LIMIT 1",
    )
    .bind(node_id).bind(height as i64).fetch_optional(&mut **tx).await?;
    if let Some((coverage_id, first, last)) = previous {
        if last + 1 == height as i64 {
            sqlx::query("UPDATE block_coverage_intervals SET last_height=?,updated_at=? WHERE coverage_id=?")
                .bind(height as i64).bind(created_at).bind(coverage_id).execute(&mut **tx).await?;
            return Ok(());
        }
        let _ = (first, last);
    }
    sqlx::query("INSERT OR IGNORE INTO block_coverage_intervals (node_id,first_height,last_height,status,created_at,updated_at) VALUES (?, ?, ?, 'covered', ?, ?)")
        .bind(node_id).bind(height as i64).bind(height as i64).bind(created_at).bind(created_at).execute(&mut **tx).await?;
    Ok(())
}

async fn coverage_allows_backfill(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    height: u64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT coverage_id FROM block_coverage_intervals WHERE node_id=? AND status='open_recoverable_gap' AND first_height <= ? AND last_height >= ? LIMIT 1")
        .bind(node_id).bind(height as i64).bind(height as i64).fetch_optional(&mut **tx).await
}

async fn close_coverage_height(
    tx: &mut Transaction<'_, Sqlite>,
    coverage_id: i64,
    height: u64,
    now_text: &str,
) -> Result<(), sqlx::Error> {
    let row = sqlx::query_as::<_, (i64, i64)>(
        "SELECT first_height,last_height FROM block_coverage_intervals WHERE coverage_id=?",
    )
    .bind(coverage_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((first, last)) = row else {
        return Ok(());
    };
    let first = first as u64;
    let last = last as u64;
    if first == last {
        sqlx::query("DELETE FROM block_coverage_intervals WHERE coverage_id=?")
            .bind(coverage_id)
            .execute(&mut **tx)
            .await?;
    } else if height == first {
        sqlx::query("UPDATE block_coverage_intervals SET first_height=first_height+1,updated_at=? WHERE coverage_id=?").bind(now_text).bind(coverage_id).execute(&mut **tx).await?;
    } else if height == last {
        sqlx::query("UPDATE block_coverage_intervals SET last_height=last_height-1,updated_at=? WHERE coverage_id=?").bind(now_text).bind(coverage_id).execute(&mut **tx).await?;
    } else if height > first && height < last {
        sqlx::query(
            "UPDATE block_coverage_intervals SET last_height=? ,updated_at=? WHERE coverage_id=?",
        )
        .bind((height - 1) as i64)
        .bind(now_text)
        .bind(coverage_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query("INSERT INTO block_coverage_intervals (node_id,first_height,last_height,status,created_at,updated_at) SELECT node_id,?,?,status,created_at,? FROM block_coverage_intervals WHERE coverage_id=?").bind(last as i64).bind(now_text).execute(&mut **tx).await?;
    }
    Ok(())
}

async fn update_network_references(
    tx: &mut Transaction<'_, Sqlite>,
    network_keys: &[String],
    observed_at: &str,
) -> Result<(), sqlx::Error> {
    for network_key in network_keys {
        let rows = sqlx::query_as::<_, (String, i64, Option<String>)>(
            "SELECT n.node_id, c.current_block, h.resync_state FROM nodes n JOIN agents a ON a.agent_id=n.agent_id JOIN current_node_chain_observations c ON c.node_id=n.node_id LEFT JOIN block_history_state h ON h.node_id=n.node_id WHERE n.network_key=? AND n.lifecycle='active' AND julianday(a.last_received_at) >= julianday(?) - (120.0/86400.0) ORDER BY c.current_block DESC",
        )
        .bind(network_key)
        .bind(observed_at)
        .fetch_all(&mut **tx)
        .await?;
        let eligible = rows
            .iter()
            .filter(|(_, _, state)| state.as_deref() != Some("resyncing"))
            .collect::<Vec<_>>();
        let candidates = if eligible.is_empty() {
            rows.iter().collect::<Vec<_>>()
        } else {
            eligible
        };
        let Some((node_id, head, _)) = candidates.first() else {
            sqlx::query("INSERT INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES (?, NULL, ?, 'unknown', 0, NULL) ON CONFLICT(network_key) DO UPDATE SET block_number=NULL, observed_at=excluded.observed_at, confidence='unknown', eligible_source_count=0, contributing_node_id=NULL")
                .bind(network_key).bind(observed_at).execute(&mut **tx).await?;
            continue;
        };
        let confidence = if candidates.len() >= 2 { "high" } else { "low" };
        sqlx::query("INSERT INTO network_reference_heads (network_key, block_number, observed_at, confidence, eligible_source_count, contributing_node_id) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(network_key) DO UPDATE SET block_number=excluded.block_number, observed_at=excluded.observed_at, confidence=excluded.confidence, eligible_source_count=excluded.eligible_source_count, contributing_node_id=excluded.contributing_node_id")
            .bind(network_key).bind(*head).bind(observed_at).bind(confidence).bind(candidates.len() as i64).bind(node_id)
            .execute(&mut **tx).await?;
    }
    Ok(())
}

async fn block_network_identity_mismatches(
    tx: &mut Transaction<'_, Sqlite>,
    report: &AgentReport,
) -> Result<std::collections::HashSet<platpulse_core::identity::NodeId>, sqlx::Error> {
    let mut mismatches = std::collections::HashSet::new();
    for sample in &report.block_summaries {
        let Some(node) = report
            .inventory
            .nodes
            .iter()
            .find(|node| node.node_id == sample.node_id)
        else {
            continue;
        };
        let registered = sqlx::query_as::<_, (String, i64, i64, String)>(
            "SELECT genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks WHERE network_key = ?",
        ).bind(node.network_key.as_str()).fetch_optional(&mut **tx).await?;
        let Some((genesis, chain_id, p2p_network_id, address_hrp)) = registered else {
            continue;
        };
        if sample.network_identity.genesis_hash.to_string() != genesis
            || sample.network_identity.chain_id != chain_id as u64
            || sample.network_identity.p2p_network_id != p2p_network_id as u64
            || sample.network_identity.address_hrp.as_deref().unwrap_or("") != address_hrp
        {
            mismatches.insert(sample.node_id);
        }
    }
    Ok(mismatches)
}

async fn observe_block_identity(
    tx: &mut Transaction<'_, Sqlite>,
    sample: &platpulse_core::block::BlockSummary,
    received_at: &str,
) -> Result<bool, sqlx::Error> {
    const MAX_WINDOW_HEIGHTS: i64 = 2_048;
    let node_id = sample.node_id.to_string();
    // Identity evidence has its own retention and bounded height count. It is
    // deliberately independent from raw Block Summary retention.
    sqlx::query("DELETE FROM block_identity_window WHERE node_id=? AND retained_until IS NOT NULL AND retained_until < ?")
        .bind(&node_id)
        .bind(received_at)
        .execute(&mut **tx)
        .await?;
    let existing = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT block_hash, retained_until FROM block_identity_window WHERE node_id=? AND height=?",
    )
    .bind(&node_id)
    .bind(sample.block_number as i64)
    .fetch_optional(&mut **tx)
    .await?;
    let retention_until = crate::auth::parse_rfc3339(received_at)
        .and_then(|value| value.checked_add(time::Duration::days(180)))
        .map(crate::auth::format_rfc3339);
    let observed_hash = sample.block_hash.to_string();
    if let Some((retained_hash, _)) = existing {
        if retained_hash != observed_hash {
            sqlx::query("INSERT OR IGNORE INTO chain_divergence_observations (node_id, height, retained_block_hash, observed_block_hash, observed_at, reason, retained_observed_at) SELECT ?, ?, block_hash, ?, ?, 'chain_divergence', observed_at FROM block_identity_window WHERE node_id=? AND height=?")
                .bind(&node_id)
                .bind(sample.block_number as i64)
                .bind(&observed_hash)
                .bind(sample.observed_at.to_string())
                .bind(&node_id)
                .bind(sample.block_number as i64)
                .execute(&mut **tx)
                .await?;
            return Ok(true);
        }
        sqlx::query("UPDATE block_identity_window SET observed_at=?, retained_until=? WHERE node_id=? AND height=?")
            .bind(sample.observed_at.to_string())
            .bind(retention_until)
            .bind(&node_id)
            .bind(sample.block_number as i64)
            .execute(&mut **tx)
            .await?;
        return Ok(false);
    }
    sqlx::query("INSERT INTO block_identity_window (node_id, height, block_hash, retained_until, observed_at) VALUES (?, ?, ?, ?, ?)")
        .bind(&node_id)
        .bind(sample.block_number as i64)
        .bind(&observed_hash)
        .bind(retention_until)
        .bind(sample.observed_at.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM block_identity_window WHERE node_id=? AND height NOT IN (SELECT height FROM block_identity_window WHERE node_id=? ORDER BY height DESC LIMIT ?)")
        .bind(&node_id)
        .bind(&node_id)
        .bind(MAX_WINDOW_HEIGHTS)
        .execute(&mut **tx)
        .await?;
    Ok(false)
}

async fn save_current(
    tx: &mut Transaction<'_, Sqlite>,
    report: &AgentReport,
    received_at: &str,
) -> Result<(), sqlx::Error> {
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
        received_at,
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
        received_at,
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
        received_at,
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
        received_at,
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
        received_at,
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
        received_at,
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
        received_at,
    )
    .await?;

    sqlx::query("INSERT INTO current_host_observations (agent_id, cpu_percent, memory_total_bytes, memory_used_bytes, load1, load5, load15, network_rx_bytes_per_sec, network_tx_bytes_per_sec, clock_skew_ms, spool_queued_bytes, spool_queued_reports, spool_oldest_queued_age_ms, spool_dropped_reports, spool_dropped_samples, spool_in_flight, spool_last_delivery_error, spool_last_delivery_at, spool_capacity_bytes, spool_max_age_seconds, spool_dropped_sequence_from, spool_dropped_sequence_to, spool_dropped_time_from, spool_dropped_time_to, spool_dropped_height_from, spool_dropped_height_to, spool_pending_history_gaps, spool_report_too_large, spool_store_fatal, spool_store_error, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(agent_id) DO UPDATE SET cpu_percent=COALESCE(excluded.cpu_percent, current_host_observations.cpu_percent), memory_total_bytes=COALESCE(excluded.memory_total_bytes, current_host_observations.memory_total_bytes), memory_used_bytes=COALESCE(excluded.memory_used_bytes, current_host_observations.memory_used_bytes), load1=COALESCE(excluded.load1, current_host_observations.load1), load5=COALESCE(excluded.load5, current_host_observations.load5), load15=COALESCE(excluded.load15, current_host_observations.load15), network_rx_bytes_per_sec=COALESCE(excluded.network_rx_bytes_per_sec, current_host_observations.network_rx_bytes_per_sec), network_tx_bytes_per_sec=COALESCE(excluded.network_tx_bytes_per_sec, current_host_observations.network_tx_bytes_per_sec), clock_skew_ms=COALESCE(excluded.clock_skew_ms, current_host_observations.clock_skew_ms), spool_queued_bytes=COALESCE(excluded.spool_queued_bytes, current_host_observations.spool_queued_bytes), spool_queued_reports=COALESCE(excluded.spool_queued_reports, current_host_observations.spool_queued_reports), spool_oldest_queued_age_ms=COALESCE(excluded.spool_oldest_queued_age_ms, current_host_observations.spool_oldest_queued_age_ms), spool_dropped_reports=COALESCE(excluded.spool_dropped_reports, current_host_observations.spool_dropped_reports), spool_dropped_samples=COALESCE(excluded.spool_dropped_samples, current_host_observations.spool_dropped_samples), spool_in_flight=COALESCE(excluded.spool_in_flight, current_host_observations.spool_in_flight), spool_last_delivery_error=COALESCE(excluded.spool_last_delivery_error, current_host_observations.spool_last_delivery_error), spool_last_delivery_at=COALESCE(excluded.spool_last_delivery_at, current_host_observations.spool_last_delivery_at), spool_capacity_bytes=COALESCE(excluded.spool_capacity_bytes, current_host_observations.spool_capacity_bytes), spool_max_age_seconds=COALESCE(excluded.spool_max_age_seconds, current_host_observations.spool_max_age_seconds), spool_dropped_sequence_from=COALESCE(excluded.spool_dropped_sequence_from, current_host_observations.spool_dropped_sequence_from), spool_dropped_sequence_to=COALESCE(excluded.spool_dropped_sequence_to, current_host_observations.spool_dropped_sequence_to), spool_dropped_time_from=COALESCE(excluded.spool_dropped_time_from, current_host_observations.spool_dropped_time_from), spool_dropped_time_to=COALESCE(excluded.spool_dropped_time_to, current_host_observations.spool_dropped_time_to), spool_dropped_height_from=COALESCE(excluded.spool_dropped_height_from, current_host_observations.spool_dropped_height_from), spool_dropped_height_to=COALESCE(excluded.spool_dropped_height_to, current_host_observations.spool_dropped_height_to), spool_pending_history_gaps=COALESCE(excluded.spool_pending_history_gaps, current_host_observations.spool_pending_history_gaps), spool_report_too_large=COALESCE(excluded.spool_report_too_large, current_host_observations.spool_report_too_large), spool_store_fatal=COALESCE(excluded.spool_store_fatal, current_host_observations.spool_store_fatal), spool_store_error=COALESCE(excluded.spool_store_error, current_host_observations.spool_store_error), updated_at=excluded.updated_at")
        .bind(&agent_id)
        .bind(host.cpu_percent.latest)
        .bind(host.memory.latest.map(|v| v.total_bytes as i64))
        .bind(host.memory.latest.map(|v| v.used_bytes as i64))
        .bind(host.load.latest.map(|v| v.load1))
        .bind(host.load.latest.map(|v| v.load5))
        .bind(host.load.latest.map(|v| v.load15))
        .bind(host.network_throughput.latest.map(|v| v.rx_bytes_per_sec as i64))
        .bind(host.network_throughput.latest.map(|v| v.tx_bytes_per_sec as i64))
        .bind(host.clock_skew.latest)
        .bind(host.spool.latest.as_ref().map(|v| v.queued_bytes as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.queued_reports as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.oldest_queued_age_ms as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.dropped_reports as i64))
        .bind(host.spool.latest.as_ref().map(|v| v.dropped_samples as i64))
        .bind(host.spool.latest.as_ref().and_then(|v| v.in_flight.map(|value| value as i64)))
        .bind(host.spool.latest.as_ref().and_then(|v| v.last_delivery_error.as_deref()))
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
        .bind(host.spool.latest.as_ref().and_then(|v| v.store_error.as_deref()))
        .bind(received_at)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO agent_spool_diagnostics (agent_id, max_bytes, max_age_seconds, dropped_sequence_from, dropped_sequence_to, dropped_time_from, dropped_time_to, dropped_height_from, dropped_height_to, pending_history_gaps, report_too_large, store_fatal, store_error, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(agent_id) DO UPDATE SET max_bytes=excluded.max_bytes, max_age_seconds=excluded.max_age_seconds, dropped_sequence_from=excluded.dropped_sequence_from, dropped_sequence_to=excluded.dropped_sequence_to, dropped_time_from=excluded.dropped_time_from, dropped_time_to=excluded.dropped_time_to, dropped_height_from=excluded.dropped_height_from, dropped_height_to=excluded.dropped_height_to, pending_history_gaps=excluded.pending_history_gaps, report_too_large=excluded.report_too_large, store_fatal=excluded.store_fatal, store_error=excluded.store_error, updated_at=excluded.updated_at")
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
        .bind(host.spool.latest.as_ref().and_then(|v| v.store_error.as_deref()))
        .bind(received_at)
        .execute(&mut **tx)
        .await?;
    if let Some(disk) = host.disk.latest.as_ref() {
        sqlx::query("DELETE FROM current_host_disk_mounts WHERE agent_id = ?")
            .bind(&agent_id)
            .execute(&mut **tx)
            .await?;
        for mount in &disk.mounts {
            sqlx::query("INSERT INTO current_host_disk_mounts (agent_id, mount_path, total_bytes, used_bytes, updated_at) VALUES (?, ?, ?, ?, ?)")
                .bind(&agent_id).bind(&mount.mount_path).bind(mount.total_bytes as i64).bind(mount.used_bytes as i64).bind(received_at).execute(&mut **tx).await?;
        }
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
            received_at,
        )
        .await?;
        save_component(
            tx,
            &agent_id,
            "node",
            &node_id,
            Some(&node_id),
            ComponentKey::Rpc,
            &node.chain.rpc,
            received_at,
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
            received_at,
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
            received_at,
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
            received_at,
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
            received_at,
        )
        .await?;
        if let Some(process) = node.process.latest {
            sqlx::query("INSERT INTO current_node_process_observations (node_id, pid, started_at, cpu_percent, memory_bytes, uptime_ms, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET pid=excluded.pid, started_at=excluded.started_at, cpu_percent=excluded.cpu_percent, memory_bytes=excluded.memory_bytes, uptime_ms=excluded.uptime_ms, updated_at=excluded.updated_at")
                .bind(&node_id).bind(process.pid as i64).bind(process.started_at.to_string()).bind(process.cpu_percent).bind(process.memory_bytes as i64).bind(process.uptime_ms as i64).bind(received_at).execute(&mut **tx).await?;
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
            sqlx::query("INSERT INTO block_history_state (node_id, current_head, resync_state, resync_last_progress_at, updated_at) VALUES (?, ?, 'normal', ?, ?) ON CONFLICT(node_id) DO UPDATE SET current_head=excluded.current_head, resync_last_progress_at=excluded.resync_last_progress_at, updated_at=excluded.updated_at")
                .bind(&node_id)
                .bind(current)
                .bind(&current_timestamp)
                .bind(received_at)
                .execute(&mut **tx)
                .await?;
            sqlx::query("UPDATE block_history_state SET resync_state=CASE WHEN current_head IS NOT NULL AND current_head < historical_high_watermark THEN 'resyncing' ELSE 'normal' END, resync_started_at=CASE WHEN current_head IS NOT NULL AND current_head < historical_high_watermark AND resync_state != 'resyncing' THEN updated_at ELSE resync_started_at END WHERE node_id=?")
                .bind(&node_id)
                .execute(&mut **tx)
                .await?;

            let rpc = node.chain.rpc.latest.as_ref();
            let sync = node.chain.sync.latest;
            let consensus = node.chain.consensus.latest;
            let identity = node.chain.network_identity.latest.as_ref();
            let metadata = node.chain.static_metadata.latest.as_ref();
            sqlx::query("INSERT INTO current_node_chain_observations (node_id, rpc_client_version, syncing, current_block, highest_block, pulled_states, known_states, consensus_epoch, consensus_view_number, consensus_validator, consensus_highest_qc_block, consensus_highest_lock_block, consensus_highest_commit_block, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, node_key_fingerprint, enode, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET rpc_client_version=COALESCE(excluded.rpc_client_version, current_node_chain_observations.rpc_client_version), syncing=COALESCE(excluded.syncing, current_node_chain_observations.syncing), current_block=COALESCE(excluded.current_block, current_node_chain_observations.current_block), highest_block=COALESCE(excluded.highest_block, current_node_chain_observations.highest_block), pulled_states=COALESCE(excluded.pulled_states, current_node_chain_observations.pulled_states), known_states=COALESCE(excluded.known_states, current_node_chain_observations.known_states), consensus_epoch=COALESCE(excluded.consensus_epoch, current_node_chain_observations.consensus_epoch), consensus_view_number=COALESCE(excluded.consensus_view_number, current_node_chain_observations.consensus_view_number), consensus_validator=COALESCE(excluded.consensus_validator, current_node_chain_observations.consensus_validator), consensus_highest_qc_block=COALESCE(excluded.consensus_highest_qc_block, current_node_chain_observations.consensus_highest_qc_block), consensus_highest_lock_block=COALESCE(excluded.consensus_highest_lock_block, current_node_chain_observations.consensus_highest_lock_block), consensus_highest_commit_block=COALESCE(excluded.consensus_highest_commit_block, current_node_chain_observations.consensus_highest_commit_block), network_genesis_hash=COALESCE(excluded.network_genesis_hash, current_node_chain_observations.network_genesis_hash), network_chain_id=COALESCE(excluded.network_chain_id, current_node_chain_observations.network_chain_id), network_p2p_network_id=COALESCE(excluded.network_p2p_network_id, current_node_chain_observations.network_p2p_network_id), network_address_hrp=COALESCE(excluded.network_address_hrp, current_node_chain_observations.network_address_hrp), node_key_fingerprint=COALESCE(excluded.node_key_fingerprint, current_node_chain_observations.node_key_fingerprint), enode=COALESCE(excluded.enode, current_node_chain_observations.enode), updated_at=excluded.updated_at")
                .bind(&node_id).bind(rpc.map(|v| &v.client_version)).bind(sync.map(|v| v.syncing as i64)).bind(sync.map(|v| v.current_block as i64)).bind(sync.map(|v| v.highest_block as i64)).bind(sync.map(|v| v.pulled_states as i64)).bind(sync.map(|v| v.known_states as i64)).bind(consensus.map(|v| v.epoch as i64)).bind(consensus.map(|v| v.view_number as i64)).bind(consensus.map(|v| v.validator as i64)).bind(consensus.map(|v| v.highest_qc_block as i64)).bind(consensus.map(|v| v.highest_lock_block as i64)).bind(consensus.map(|v| v.highest_commit_block as i64)).bind(identity.map(|v| v.genesis_hash.to_string())).bind(identity.map(|v| v.chain_id as i64)).bind(identity.map(|v| v.p2p_network_id as i64)).bind(identity.and_then(|v| v.address_hrp.as_deref())).bind(metadata.map(|v| v.node_key_fingerprint.to_string())).bind(metadata.and_then(|v| v.enode.as_deref())).bind(received_at).execute(&mut **tx).await?;
            if let Some(rpc) = rpc {
                sqlx::query("DELETE FROM current_node_rpc_namespaces WHERE node_id = ?")
                    .bind(&node_id)
                    .execute(&mut **tx)
                    .await?;
                for namespace in &rpc.namespaces {
                    sqlx::query("INSERT INTO current_node_rpc_namespaces (node_id, namespace, updated_at) VALUES (?, ?, ?)").bind(&node_id).bind(namespace).bind(received_at).execute(&mut **tx).await?;
                }
                sqlx::query("DELETE FROM current_node_rpc_methods WHERE node_id = ?")
                    .bind(&node_id)
                    .execute(&mut **tx)
                    .await?;
                for method in &rpc.methods {
                    sqlx::query("INSERT INTO current_node_rpc_methods (node_id, method, updated_at) VALUES (?, ?, ?)").bind(&node_id).bind(method).bind(received_at).execute(&mut **tx).await?;
                }
            }
        }
    }
    for sample in &report.block_summaries {
        let node_id = sample.node_id.to_string();
        let registered_identity = sqlx::query_as::<_, (String, i64, i64, String)>("SELECT genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks n JOIN nodes nd ON nd.network_key = n.network_key WHERE nd.node_id = ?")
            .bind(&node_id)
            .fetch_optional(&mut **tx)
            .await?;
        let identity_matches = registered_identity.is_some_and(|(genesis, chain_id, p2p, hrp)| {
            sample.network_identity.genesis_hash.to_string() == genesis
                && sample.network_identity.chain_id == chain_id as u64
                && sample.network_identity.p2p_network_id == p2p as u64
                && sample.network_identity.address_hrp.as_deref().unwrap_or("") == hrp
        });
        if !identity_matches {
            continue;
        }

        let proposer = match &sample.attribution.protocol_proposer {
            platpulse_core::block::ProtocolProposer::Verified { identity } => {
                ("verified", Some(identity.as_str()))
            }
            platpulse_core::block::ProtocolProposer::Unknown {} => ("unknown", None),
        };
        let signer = match sample.attribution.seal_signer_match {
            platpulse_core::block::SealSignerMatch::SignerSelf => "self",
            platpulse_core::block::SealSignerMatch::Other => "other",
            platpulse_core::block::SealSignerMatch::Unknown => "unknown",
        };
        if sample.source == platpulse_core::block::BlockSource::GapBackfill
            && coverage_allows_backfill(&mut *tx, &node_id, sample.block_number)
                .await?
                .is_none()
        {
            continue;
        }
        if sample.source == platpulse_core::block::BlockSource::Subscription {
            let historical_high_watermark: Option<i64> = sqlx::query_scalar(
                "SELECT historical_high_watermark FROM block_history_state WHERE node_id=?",
            )
            .bind(&node_id)
            .fetch_optional(&mut **tx)
            .await?;
            if historical_high_watermark.is_some_and(|height| sample.block_number as i64 <= height)
            {
                continue;
            }
        }
        let coverage_id = if sample.source == platpulse_core::block::BlockSource::GapBackfill {
            coverage_allows_backfill(&mut *tx, &node_id, sample.block_number).await?
        } else {
            None
        };
        let inserted = sqlx::query("INSERT OR IGNORE INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, block_interval_ms, source, coinbase, seal_signer_key_fingerprint, seal_signer_match, node_key_fingerprint, node_key_valid_from, node_key_valid_until, node_key_history_complete, seal_recovery_rule, seal_evidence, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, accepted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&node_id).bind(sample.block_number as i64).bind(sample.block_hash.to_string()).bind(sample.parent_hash.to_string())
            .bind(sample.network_identity.genesis_hash.to_string()).bind(sample.network_identity.chain_id as i64).bind(sample.network_identity.p2p_network_id as i64).bind(sample.network_identity.address_hrp.as_deref().unwrap_or(""))
            .bind(sample.block_timestamp_ms as i64).bind(sample.observed_at.to_string()).bind(sample.transaction_count as i64).bind(sample.block_interval_ms.map(|v| v as i64)).bind(match sample.source { platpulse_core::block::BlockSource::Subscription => "subscription", platpulse_core::block::BlockSource::GapBackfill => "gap_backfill" })
            .bind(sample.attribution.coinbase.to_string()).bind(sample.attribution.seal_signer_key_fingerprint.as_ref().map(ToString::to_string)).bind(signer).bind(sample.attribution.node_key.as_ref().map(|key| key.fingerprint.to_string())).bind(sample.attribution.node_key.as_ref().and_then(|key| key.valid_from.map(|value| value.to_string()))).bind(sample.attribution.node_key.as_ref().and_then(|key| key.valid_until.map(|value| value.to_string()))).bind(sample.attribution.node_key.as_ref().is_some_and(|key| key.history_complete) as i64).bind(sample.attribution.seal_recovery_rule.as_deref()).bind(sample.attribution.seal_evidence.as_deref()).bind(proposer.0).bind(proposer.1).bind(&sample.attribution.attribution_reason).bind(received_at)
            .execute(&mut **tx).await?;
        if inserted.rows_affected() == 0 {
            continue;
        }
        if let Some(coverage_id) = coverage_id {
            close_coverage_height(&mut *tx, coverage_id, sample.block_number, received_at).await?;
        } else {
            append_coverage_height(&mut *tx, &node_id, sample.block_number, received_at).await?;
        }

        sqlx::query("INSERT INTO observed_network_heads (node_id, block_number, block_hash, observed_at, confidence, eligible_sources) VALUES (?, ?, ?, ?, 'unknown', '[\\\"subscription\\\"]') ON CONFLICT(node_id) DO UPDATE SET block_number=excluded.block_number, block_hash=excluded.block_hash, observed_at=excluded.observed_at, confidence=excluded.confidence, eligible_sources=excluded.eligible_sources")
            .bind(&node_id).bind(sample.block_number as i64).bind(sample.block_hash.to_string()).bind(sample.observed_at.to_string()).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, updated_at) VALUES (?, ?, 1, ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET historical_high_watermark=MAX(block_history_state.historical_high_watermark, excluded.historical_high_watermark), cumulative_block_count=block_history_state.cumulative_block_count + 1, cumulative_transaction_count=block_history_state.cumulative_transaction_count + excluded.cumulative_transaction_count, cumulative_self_seal_count=block_history_state.cumulative_self_seal_count + excluded.cumulative_self_seal_count, updated_at=excluded.updated_at")
            .bind(&node_id).bind(sample.block_number as i64).bind(sample.transaction_count as i64)
            .bind((sample.attribution.seal_signer_match == platpulse_core::block::SealSignerMatch::SignerSelf) as i64).bind(received_at).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Outcome of evaluating one Agent's Inventory declaration of a Node it does
/// not yet own (design §4.4). A declaration only wins ownership when a
/// pending transfer targets the reporting Agent, the declared Network key
/// matches the registered Node, and the observed Network Identity matches
/// the Registry tuple; everything else keeps the source Agent authoritative.
#[derive(Debug)]
enum TransferResolution {
    /// No pending transfer covers this declaration; ownership stays put.
    NoTransfer,
    /// The declaration completed the pending transfer; the reporting Agent
    /// owns the Node from this transaction on.
    Completed,
    /// A pending transfer exists but the report carries no usable identity
    /// observation yet; the transfer stays pending and ownership stays put.
    Unverified,
    /// The declared Network key differs from the registered Network; the
    /// transfer is terminally rejected and ownership stays put.
    NetworkKeyMismatch,
    /// The observed identity contradicts the Registry tuple; the transfer
    /// ends in a typed `identity_mismatch` outcome and ownership stays put.
    IdentityMismatch,
}

/// Evaluate a non-owner Inventory declaration against the pending transfer
/// state, inside the ingestion transaction. A successful resolution switches
/// `nodes.agent_id` in the same transaction that accepts the report, so the
/// source stays authoritative until the switch is atomic (issue #46).
async fn resolve_node_transfer(
    tx: &mut Transaction<'_, Sqlite>,
    report: &AgentReport,
    node: &platpulse_core::inventory::InventoryNode,
    reporting_agent: &str,
    now_text: &str,
) -> Result<TransferResolution, sqlx::Error> {
    // Materialize transfers that expired before this declaration so the
    // history never shows a stale pending row for an expired handover.
    let expired = sqlx::query(
        "UPDATE node_transfers SET status='expired', updated_at=? WHERE node_id=? AND target_agent_id=? AND status='pending' AND expires_at <= ?",
    )
    .bind(now_text)
    .bind(node.node_id.to_string())
    .bind(reporting_agent)
    .bind(now_text)
    .execute(&mut **tx)
    .await?;
    if expired.rows_affected() > 0 {
        let _ = crate::auth::insert_audit_event(
            &mut **tx,
            None,
            "node_transfer_expired",
            "node",
            &node.node_id.to_string(),
            Some(&serde_json::json!({ "expired_at": now_text })),
        )
        .await;
    }
    let pending = sqlx::query_as::<_, (String, String)>(
        "SELECT transfer_id, source_agent_id FROM node_transfers WHERE node_id=? AND target_agent_id=? AND status='pending' AND expires_at > ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(node.node_id.to_string())
    .bind(reporting_agent)
    .bind(now_text)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((transfer_id, source_agent_id)) = pending else {
        return Ok(TransferResolution::NoTransfer);
    };
    let registered = sqlx::query_as::<_, (String, String, i64, i64, String)>(
        "SELECT n.network_key, r.genesis_hash, r.chain_id, r.p2p_network_id, r.address_hrp FROM nodes n JOIN networks r ON r.network_key=n.network_key WHERE n.node_id=?",
    )
    .bind(node.node_id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    let Some((registered_key, genesis, chain_id, p2p, hrp)) = registered else {
        return Ok(TransferResolution::NoTransfer);
    };
    if node.network_key.as_str() != registered_key {
        sqlx::query(
            "UPDATE node_transfers SET status='rejected', rejection_code='network_key_mismatch', rejection_reason=?, updated_at=? WHERE transfer_id=?",
        )
        .bind("target declared the Node under a different Network key than the registered Network")
        .bind(now_text)
        .bind(&transfer_id)
        .execute(&mut **tx)
        .await?;
        let _ = crate::auth::insert_audit_event(
            &mut **tx,
            None,
            "node_transfer_rejected",
            "node",
            &node.node_id.to_string(),
            Some(&serde_json::json!({
                "transfer_id": transfer_id,
                "rejection_code": "network_key_mismatch",
            })),
        )
        .await;
        return Ok(TransferResolution::NetworkKeyMismatch);
    }
    // The identity authority is the report's own Node observation: a
    // declaration without a successful identity probe cannot switch
    // ownership (source remains authoritative until validation passes).
    let identity = report
        .nodes
        .iter()
        .find(|observation| observation.node_id == node.node_id)
        .filter(|observation| observation.chain.network_identity.status == ComponentStatus::Ok)
        .and_then(|observation| observation.chain.network_identity.latest.as_ref());
    let Some(identity) = identity else {
        return Ok(TransferResolution::Unverified);
    };
    let mut mismatched_fields = Vec::new();
    if identity.genesis_hash.to_string() != genesis {
        mismatched_fields.push("genesis_hash");
    }
    if identity.chain_id != chain_id as u64 {
        mismatched_fields.push("chain_id");
    }
    if identity.p2p_network_id != p2p as u64 {
        mismatched_fields.push("p2p_network_id");
    }
    if identity.address_hrp.as_deref().unwrap_or("") != hrp {
        mismatched_fields.push("address_hrp");
    }
    if !mismatched_fields.is_empty() {
        sqlx::query(
            "UPDATE node_transfers SET status='identity_mismatch', rejection_code='identity_mismatch', rejection_reason=?, mismatched_fields=?, updated_at=? WHERE transfer_id=?",
        )
        .bind("the target-declared Network identity contradicts the registered Network; ownership stays with the source Agent")
        .bind(serde_json::to_string(&mismatched_fields).expect("fields serialize"))
        .bind(now_text)
        .bind(&transfer_id)
        .execute(&mut **tx)
        .await?;
        let _ = crate::auth::insert_audit_event(
            &mut **tx,
            None,
            "node_transfer_identity_mismatch",
            "node",
            &node.node_id.to_string(),
            Some(&serde_json::json!({
                "transfer_id": transfer_id,
                "mismatched_fields": mismatched_fields,
            })),
        )
        .await;
        return Ok(TransferResolution::IdentityMismatch);
    }
    sqlx::query(
        "UPDATE node_transfers SET status='completed', completed_at=?, updated_at=? WHERE transfer_id=?",
    )
    .bind(now_text)
    .bind(now_text)
    .bind(&transfer_id)
    .execute(&mut **tx)
    .await?;
    // Ownership lives in `nodes.agent_id` AND in the composite FK
    // `component_status(agent_id, node_id)` rows of the same Node. Both
    // sides must move in this transaction; FK enforcement is deferred to
    // the commit so the parent and child rows switch atomically (and the
    // pragma resets automatically when the transaction ends).
    sqlx::query("PRAGMA defer_foreign_keys=ON")
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE nodes SET agent_id=?, updated_at=? WHERE node_id=?")
        .bind(reporting_agent)
        .bind(now_text)
        .bind(node.node_id.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE component_status SET agent_id=? WHERE node_id=?")
        .bind(reporting_agent)
        .bind(node.node_id.to_string())
        .execute(&mut **tx)
        .await?;
    let _ = crate::auth::insert_audit_event(
        &mut **tx,
        None,
        "node_transfer_completed",
        "node",
        &node.node_id.to_string(),
        Some(&serde_json::json!({
            "transfer_id": transfer_id,
            "source_agent_id": source_agent_id,
            "target_agent_id": reporting_agent,
        })),
    )
    .await;
    Ok(TransferResolution::Completed)
}

pub(crate) fn validate_report_body_size(len: usize) -> Result<(), StatusCode> {
    if len > platpulse_core::protocol::MAX_REPORT_BODY_BYTES {
        Err(StatusCode::PAYLOAD_TOO_LARGE)
    } else {
        Ok(())
    }
}

async fn handler(
    State(state): State<AppState>,
    Extension(auth): Extension<AgentAuthInfo>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    if state.is_shutting_down() {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "shutting_down",
            "Server is shutting down",
        );
    }
    let _ingestion = match state.ingestion_guard() {
        Some(guard) => guard,
        None => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "shutting_down",
                "Server is shutting down",
            );
        }
    };
    if validate_report_body_size(body.len()).is_err() {
        return error(
            &request_id.0,
            StatusCode::PAYLOAD_TOO_LARGE,
            "report_too_large",
            "Agent report exceeds the protocol size limit",
        );
    }
    let digest = Sha256::digest(&body);
    let hash = Sha256Hex::from_str(&format!("0x{digest:x}")).expect("SHA-256 output is valid");
    let parsed: AgentReport = match serde_json::from_slice(&body) {
        Ok(report) => report,
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_report",
                "Agent report is invalid",
            );
        }
    };
    if parsed.validate().is_err() {
        return error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_report",
            "Agent report is invalid",
        );
    }
    if parsed.agent_id.to_string() != auth.agent_id {
        return error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "agent_identity_mismatch",
            "Agent identity is not authorized",
        );
    }
    let mut tx = match state.db().pool().begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };

    let existing = match sqlx::query_as::<_, ReceiptRow>(
        "SELECT report_body_sha256, receipt_body FROM agent_report_receipts WHERE report_id = ?",
    )
    .bind(parsed.report_id.to_string())
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(row) => row,
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if let Some(existing) = existing {
        if existing.report_body_sha256 != hash.to_string() {
            return error(
                &request_id.0,
                StatusCode::CONFLICT,
                "report_identity_conflict",
                "Report identity conflicts with a stored report",
            );
        }
        let receipt: ReportReceipt = match serde_json::from_slice(&existing.receipt_body) {
            Ok(v) => v,
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Stored receipt is unavailable",
                );
            }
        };
        return (StatusCode::OK, Json(ReportResponse { receipt })).into_response();
    }
    let agent = match sqlx::query_as::<_, AgentRow>(
        "SELECT agent_epoch, active_boot_id, active_boot_status, previous_boot_id, close_report_id, last_report_sequence, last_inventory_revision FROM agents WHERE agent_id = ?",
    )
    .bind(&auth.agent_id)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error(
                &request_id.0,
                StatusCode::UNAUTHORIZED,
                "agent_auth_required",
                "Agent credential is invalid",
            );
        }
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if parsed.agent_epoch != agent.agent_epoch as u64 {
        return store_rejected(
            tx,
            &parsed,
            hash.clone(),
            rejected(
                parsed.report_id,
                hash,
                platpulse_core::RejectionCode::StaleBoot,
                "Agent epoch is stale",
            ),
            &request_id.0,
        )
        .await;
    }

    let old_boot_status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM agent_boots WHERE agent_id=? AND agent_epoch=? AND boot_id=?",
    )
    .bind(&auth.agent_id)
    .bind(parsed.agent_epoch as i64)
    .bind(parsed.boot_id.to_string())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| ())
    .ok()
    .flatten();
    let _boot_markers = (&agent.previous_boot_id, &agent.close_report_id);
    if parsed.inventory.revision < agent.last_inventory_revision as u64 {
        return store_rejected(
            tx,
            &parsed,
            hash.clone(),
            rejected(
                parsed.report_id,
                hash,
                platpulse_core::RejectionCode::InventoryRevisionConflict,
                "Inventory revision is older than the accepted revision",
            ),
            &request_id.0,
        )
        .await;
    }

    // A sequence is unique within one boot. Never silently accept a competing body.
    let sequence_conflict = match sqlx::query_scalar::<_, String>("SELECT report_id FROM agent_report_receipts WHERE agent_id = ? AND agent_epoch = ? AND boot_id = ? AND report_sequence = ?")
        .bind(&auth.agent_id).bind(parsed.agent_epoch as i64).bind(parsed.boot_id.to_string()).bind(parsed.report_sequence as i64).fetch_optional(&mut *tx).await {
            Ok(v) => v, Err(_) => return error(&request_id.0, StatusCode::SERVICE_UNAVAILABLE, "unavailable", "Server database is unavailable")
        };
    if sequence_conflict.is_some() {
        let _ = record_security_event(&mut tx, &auth.agent_id).await;
        return error(
            &request_id.0,
            StatusCode::CONFLICT,
            "conflicting_boot",
            "Boot sequence conflicts with a stored report",
        );
    }
    if let Some(active) = &agent.active_boot_id {
        if active != &parsed.boot_id.to_string() {
            if old_boot_status.as_deref() == Some("closed")
                && parsed.boot_transition != platpulse_core::BootTransition::DrainedPrevious
            {
                return store_rejected(
                    tx,
                    &parsed,
                    hash.clone(),
                    rejected(
                        parsed.report_id,
                        hash,
                        platpulse_core::RejectionCode::StaleBoot,
                        "Report belongs to a closed boot; only exact replay is accepted",
                    ),
                    &request_id.0,
                )
                .await;
            }
            if parsed.boot_transition != platpulse_core::BootTransition::DrainedPrevious
                || parsed
                    .previous_boot_id
                    .as_ref()
                    .map(ToString::to_string)
                    .as_deref()
                    != Some(active.as_str())
            {
                let _ = record_security_event(&mut tx, &auth.agent_id).await;
                return store_rejected(
                    tx,
                    &parsed,
                    hash.clone(),
                    rejected(
                        parsed.report_id,
                        hash,
                        platpulse_core::RejectionCode::ConflictingBoot,
                        "Report belongs to a competing boot",
                    ),
                    &request_id.0,
                )
                .await;
            }
            if agent.active_boot_status != "closed" {
                let _ = record_security_event(&mut tx, &auth.agent_id).await;
                return store_rejected(
                    tx,
                    &parsed,
                    hash.clone(),
                    rejected(
                        parsed.report_id,
                        hash,
                        platpulse_core::RejectionCode::ConflictingBoot,
                        "Previous boot has not completed its closing receipt",
                    ),
                    &request_id.0,
                )
                .await;
            }
        } else if agent.active_boot_status == "closing"
            || agent.active_boot_status == "closed"
            || agent
                .last_report_sequence
                .is_some_and(|last| parsed.report_sequence <= last as u64)
        {
            return store_rejected(
                tx,
                &parsed,
                hash.clone(),
                rejected(
                    parsed.report_id,
                    hash,
                    platpulse_core::RejectionCode::StaleReport,
                    "Report sequence is not newer than the accepted report",
                ),
                &request_id.0,
            )
            .await;
        }
    }
    if let Some(last) = agent.last_report_sequence
        && parsed.boot_id.to_string() == agent.active_boot_id.as_deref().unwrap_or_default()
        && parsed.report_sequence > (last as u64).saturating_add(1)
    {
        let _ = sqlx::query("INSERT OR IGNORE INTO report_sequence_gaps (agent_id, boot_id, from_sequence, to_sequence, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(&auth.agent_id)
            .bind(parsed.boot_id.to_string())
            .bind(last.saturating_add(1))
            .bind(parsed.report_sequence.saturating_sub(1) as i64)
            .bind(now().to_string())
            .execute(&mut *tx)
            .await;
    }
    let now_text = now().to_string();
    let capabilities =
        serde_json::to_string(&parsed.agent_capabilities).expect("capabilities serialize");
    let mut ownership_mismatches = std::collections::HashSet::new();
    let mut ownership_contradiction = false;
    for node in &parsed.inventory.nodes {
        let known = match sqlx::query_scalar::<_, String>(
            "SELECT network_key FROM networks WHERE network_key = ?",
        )
        .bind(node.network_key.as_str())
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(v) => v,
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        if known.is_none() {
            return store_rejected(
                tx,
                &parsed,
                hash.clone(),
                rejected(
                    parsed.report_id,
                    hash,
                    platpulse_core::RejectionCode::NetworkKeyUnknown,
                    "Network key is not registered",
                ),
                &request_id.0,
            )
            .await;
        }
        let owner =
            match sqlx::query_scalar::<_, String>("SELECT agent_id FROM nodes WHERE node_id = ?")
                .bind(node.node_id.to_string())
                .fetch_optional(&mut *tx)
                .await
            {
                Ok(v) => v,
                Err(_) => {
                    return error(
                        &request_id.0,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "Server database is unavailable",
                    );
                }
            };
        if owner.is_some_and(|owner| owner != auth.agent_id) {
            // The reporting Agent declared a Node owned by another Agent.
            // Only a pending transfer targeting this Agent with a validated
            // Network Identity may switch ownership (design §4.4); every
            // other outcome keeps the source authoritative and is a
            // contradiction worth a security event.
            match resolve_node_transfer(&mut tx, &parsed, node, &auth.agent_id, &now_text).await {
                Ok(TransferResolution::Completed) => {
                    // Ownership switched atomically; this Node is no longer a
                    // mismatch and its current/samples join this report.
                }
                Ok(TransferResolution::Unverified) => {
                    // Legitimate in-flight declaration without an identity
                    // probe yet: the transfer stays pending and the Node
                    // entry stays rejected for this report.
                    ownership_mismatches.insert(node.node_id);
                }
                Ok(TransferResolution::NoTransfer)
                | Ok(TransferResolution::NetworkKeyMismatch)
                | Ok(TransferResolution::IdentityMismatch) => {
                    ownership_mismatches.insert(node.node_id);
                    ownership_contradiction = true;
                }
                Err(_) => {
                    return error(
                        &request_id.0,
                        StatusCode::SERVICE_UNAVAILABLE,
                        "unavailable",
                        "Server database is unavailable",
                    );
                }
            }
        }
    }
    // A non-owner declaration without a valid pending transfer (or with a
    // rejected/contradicting transfer) is a security conflict: the source
    // Agent's later submissions after a completed Transfer also land here
    // and must be visible and auditable (design §4.4). The counter is
    // recorded once per report together with block identity mismatches.
    // Inventory is accepted as a complete set only after structural/network
    // validation. Ownership-invalid Nodes reject only their own current and
    // samples; they are not allowed to retire valid siblings.
    let accepted_inventory_ids = parsed
        .inventory
        .nodes
        .iter()
        .filter(|node| !ownership_mismatches.contains(&node.node_id))
        .map(|node| node.node_id.to_string())
        .collect::<Vec<_>>();

    let inventory_bytes = serde_json::to_vec(&parsed.inventory).expect("inventory serializes");
    let inventory_hash = format!("0x{:x}", Sha256::digest(&inventory_bytes));
    let prior_inventory_hash: Option<String> =
        sqlx::query_scalar("SELECT inventory_sha256 FROM agents WHERE agent_id=?")
            .bind(&auth.agent_id)
            .fetch_optional(&mut *tx)
            .await
            .unwrap_or(None);
    let inventory_revision_unchanged = parsed.inventory.revision
        == agent.last_inventory_revision as u64
        && prior_inventory_hash.as_deref() == Some(inventory_hash.as_str());

    // Equal-revision content is unchanged; do not retire siblings or rewrite
    // inventory ownership on a replay. A new revision remains authoritative.
    if !inventory_revision_unchanged && parsed.inventory.nodes.is_empty() {
        if sqlx::query("UPDATE nodes SET lifecycle='retired', updated_at=? WHERE agent_id=?")
            .bind(&now_text)
            .bind(&auth.agent_id)
            .execute(&mut *tx)
            .await
            .is_err()
        {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    } else if !inventory_revision_unchanged && !accepted_inventory_ids.is_empty() {
        let placeholders = std::iter::repeat_n("?", accepted_inventory_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE nodes SET lifecycle='retired', updated_at=? WHERE agent_id=? AND node_id NOT IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql).bind(&now_text).bind(&auth.agent_id);
        for node_id in &accepted_inventory_ids {
            query = query.bind(node_id);
        }
        if query.execute(&mut *tx).await.is_err() {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    }
    for node in &parsed.inventory.nodes {
        if ownership_mismatches.contains(&node.node_id) {
            continue;
        }
        let result = sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, ?, ?, ?, ?, 'active', 'private', ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET network_key=excluded.network_key, display_name=COALESCE(nodes.display_name, excluded.display_name), rpc_endpoint=excluded.rpc_endpoint, lifecycle='active', inventory_revision=excluded.inventory_revision, updated_at=excluded.updated_at")
            .bind(node.node_id.to_string()).bind(&auth.agent_id).bind(node.network_key.as_str()).bind(&node.display_name).bind(node.rpc_endpoint.as_str()).bind(parsed.inventory.revision as i64).bind(&now_text).bind(&now_text).execute(&mut *tx).await;
        if result.is_err() {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    }
    let mismatches = match block_network_identity_mismatches(&mut tx, &parsed).await {
        Ok(value) => value,
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if !mismatches.is_empty() {
        // Identity mismatch is a typed, audited outcome (design §7.1): the
        // samples are never merged and the Agent's security counter records
        // the contradiction so the Admin surface can surface it.
        ownership_contradiction = true;
    }
    if ownership_contradiction {
        let _ = record_security_event(&mut tx, &auth.agent_id).await;
    }
    let mut outside_open_gap: std::collections::HashSet<(platpulse_core::identity::NodeId, u64)> =
        std::collections::HashSet::new();
    for sample in &parsed.block_summaries {
        if sample.source == platpulse_core::block::BlockSource::GapBackfill
            && coverage_allows_backfill(&mut tx, &sample.node_id.to_string(), sample.block_number)
                .await
                .map_err(|_| ())
                .ok()
                .flatten()
                .is_none()
        {
            outside_open_gap.insert((sample.node_id, sample.block_number));
        }
    }
    let mut replay_samples: std::collections::HashSet<(platpulse_core::identity::NodeId, u64)> =
        std::collections::HashSet::new();
    let mut divergence_samples: std::collections::HashSet<(platpulse_core::identity::NodeId, u64)> =
        std::collections::HashSet::new();
    for sample in &parsed.block_summaries {
        // History mutation is ownership-scoped: a sample from a Node the
        // reporting Agent does not own must never touch the identity
        // window, even when its declared identity happens to match the
        // Registry (issue #46: mismatch can never merge new history).
        if ownership_mismatches.contains(&sample.node_id) {
            continue;
        }
        let node_id = sample.node_id.to_string();
        let registered_identity = match sqlx::query_as::<_, (String, i64, i64, String)>("SELECT genesis_hash, chain_id, p2p_network_id, address_hrp FROM networks n JOIN nodes nd ON nd.network_key = n.network_key WHERE nd.node_id = ?")
            .bind(&node_id)
            .fetch_optional(&mut *tx)
            .await {
                Ok(value) => value,
                Err(_) => return error(&request_id.0, StatusCode::SERVICE_UNAVAILABLE, "unavailable", "Server database is unavailable"),
            };
        let identity_matches = registered_identity.is_some_and(|(genesis, chain_id, p2p, hrp)| {
            sample.network_identity.genesis_hash.to_string() == genesis
                && sample.network_identity.chain_id == chain_id as u64
                && sample.network_identity.p2p_network_id == p2p as u64
                && sample.network_identity.address_hrp.as_deref().unwrap_or("") == hrp
        });
        if !identity_matches {
            continue;
        }
        if sample.source == platpulse_core::block::BlockSource::GapBackfill
            && outside_open_gap.contains(&(sample.node_id, sample.block_number))
        {
            continue;
        }
        let high: Option<i64> = match sqlx::query_scalar(
            "SELECT historical_high_watermark FROM block_history_state WHERE node_id=?",
        )
        .bind(&node_id)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(v) => v,
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        if sample.source == platpulse_core::block::BlockSource::Subscription
            && high.is_some_and(|value| sample.block_number as i64 <= value)
        {
            replay_samples.insert((sample.node_id, sample.block_number));
            continue;
        }
        let retained: Option<String> = match sqlx::query_scalar(
            "SELECT block_hash FROM block_identity_window WHERE node_id=? AND height=?",
        )
        .bind(&node_id)
        .bind(sample.block_number as i64)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(v) => v,
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        };
        let candidate = sample.source == platpulse_core::block::BlockSource::GapBackfill
            || high.is_none_or(|value| sample.block_number as i64 > value)
            || retained.is_some();
        if !candidate {
            continue;
        }
        match observe_block_identity(&mut tx, sample, &now_text).await {
            Ok(true) => {
                divergence_samples.insert((sample.node_id, sample.block_number));
            }
            Ok(false) => {}
            Err(_) => {
                return error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Server database is unavailable",
                );
            }
        }
    }

    let mut projection_report = parsed.clone();
    projection_report
        .nodes
        .retain(|node| !ownership_mismatches.contains(&node.node_id));
    projection_report
        .block_summaries
        .retain(|sample| !ownership_mismatches.contains(&sample.node_id));
    if let Err(save_error) = save_current(&mut tx, &projection_report, &now_text).await {
        eprintln!(
            "save_current error: {}",
            crate::redaction::redact_sensitive(&save_error.to_string())
        );
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    let network_keys = projection_report
        .inventory
        .nodes
        .iter()
        .map(|node| node.network_key.to_string())
        .collect::<Vec<_>>();
    if update_network_references(&mut tx, &network_keys, &now_text)
        .await
        .is_err()
    {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    for sample in &parsed.block_summaries {
        if ownership_mismatches.contains(&sample.node_id) {
            sqlx::query("INSERT OR IGNORE INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at) VALUES (?, ?, ?, 'server_rejected', 'Node ownership mismatch', ?)")
                .bind(sample.node_id.to_string())
                .bind(sample.block_number as i64)
                .bind(sample.block_number as i64)
                .bind(&now_text)
                .execute(&mut *tx)
                .await
                .map_err(|_| ())
                .ok();
        }
    }
    for gap in &parsed.history_gaps {
        if ownership_mismatches.contains(&gap.node_id) {
            continue;
        }
        let kind = match gap.kind {
            platpulse_core::gap::GapKind::UnrecoverableBackfill => "unrecoverable_backfill",
            platpulse_core::gap::GapKind::SpoolOverflow => "spool_overflow",
            platpulse_core::gap::GapKind::ServerRejected => "server_rejected",
        };
        sqlx::query("INSERT OR IGNORE INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(gap.node_id.to_string()).bind(gap.from_height as i64).bind(gap.to_height as i64)
            .bind(kind)
            .bind(&gap.reason)
            .bind(gap.recorded_at.to_string())
            .execute(&mut *tx).await.map_err(|_| ()).ok();
        if matches!(
            gap.kind,
            platpulse_core::gap::GapKind::UnrecoverableBackfill
        ) {
            open_coverage_gap(
                &mut tx,
                &gap.node_id.to_string(),
                gap.from_height,
                gap.to_height,
                &now_text,
            )
            .await
            .map_err(|_| ())
            .ok();
        }
    }
    let nodes: Vec<NodeReceipt> = parsed
        .inventory
        .nodes
        .iter()
        .map(|node| {
            let rejected = ownership_mismatches.contains(&node.node_id);
            NodeReceipt {
                node_id: node.node_id,
                current: if rejected {
                    NodeCurrentDisposition::Rejected
                } else {
                    NodeCurrentDisposition::Accepted
                },
                accepted_component_revisions: if rejected {
                    vec![]
                } else if let Some(observed) = parsed
                    .nodes
                    .iter()
                    .find(|candidate| candidate.node_id == node.node_id)
                {
                    vec![
                        ComponentRevision {
                            component: ComponentKey::Process,
                            state_revision: observed.process.state_revision,
                            value_revision: observed.process.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::Rpc,
                            state_revision: observed.chain.rpc.state_revision,
                            value_revision: observed.chain.rpc.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::Sync,
                            state_revision: observed.chain.sync.state_revision,
                            value_revision: observed.chain.sync.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::Consensus,
                            state_revision: observed.chain.consensus.state_revision,
                            value_revision: observed.chain.consensus.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::NetworkIdentity,
                            state_revision: observed.chain.network_identity.state_revision,
                            value_revision: observed.chain.network_identity.value_revision,
                        },
                        ComponentRevision {
                            component: ComponentKey::StaticMetadata,
                            state_revision: observed.chain.static_metadata.state_revision,
                            value_revision: observed.chain.static_metadata.value_revision,
                        },
                    ]
                } else {
                    vec![]
                },
                rejections: if rejected {
                    vec![rejection(
                        platpulse_core::RejectionCode::NodeOwnershipMismatch,
                        "Node belongs to another Agent",
                    )]
                } else {
                    vec![]
                },
            }
        })
        .collect();
    let samples = parsed
        .block_summaries
        .iter()
        .map(|sample| {
            let rejected = ownership_mismatches.contains(&sample.node_id)
                || mismatches.contains(&sample.node_id)
                || outside_open_gap.contains(&(sample.node_id, sample.block_number))
                || divergence_samples.contains(&(sample.node_id, sample.block_number))
                || replay_samples.contains(&(sample.node_id, sample.block_number));
            SampleDisposition {
                node_id: sample.node_id,
                sample: SampleRef::Block {
                    height: sample.block_number,
                },
                disposition: if rejected {
                    SampleDispositionKind::TerminalRejected
                } else {
                    SampleDispositionKind::Accepted
                },
                rejection: rejected.then(|| {
                    let code = if ownership_mismatches.contains(&sample.node_id) {
                        platpulse_core::RejectionCode::NodeOwnershipMismatch
                    } else if outside_open_gap.contains(&(sample.node_id, sample.block_number)) {
                        platpulse_core::RejectionCode::GapBackfillOutsideOpenGap
                    } else if divergence_samples.contains(&(sample.node_id, sample.block_number)) {
                        platpulse_core::RejectionCode::ChainDivergence
                    } else if replay_samples.contains(&(sample.node_id, sample.block_number)) {
                        platpulse_core::RejectionCode::ResyncReplay
                    } else {
                        platpulse_core::RejectionCode::NetworkIdentityMismatch
                    };
                    let reason = if outside_open_gap
                        .contains(&(sample.node_id, sample.block_number))
                    {
                        "GapBackfill sample is outside an explicit open recoverable gap"
                    } else if divergence_samples.contains(&(sample.node_id, sample.block_number)) {
                        "Block hash diverges from retained Node identity evidence"
                    } else if replay_samples.contains(&(sample.node_id, sample.block_number)) {
                        "Normal resync replay at or below the historical high-water mark"
                    } else {
                        "Block network identity does not match the registered Network"
                    };
                    rejection(code, reason)
                }),
            }
        })
        .chain(parsed.history_gaps.iter().map(|gap| {
            let rejected = ownership_mismatches.contains(&gap.node_id);
            SampleDisposition {
                node_id: gap.node_id,
                sample: SampleRef::Gap {
                    from_height: gap.from_height,
                    to_height: gap.to_height,
                },
                disposition: if rejected {
                    SampleDispositionKind::TerminalRejected
                } else {
                    SampleDispositionKind::Accepted
                },
                rejection: rejected.then(|| {
                    rejection(
                        platpulse_core::RejectionCode::NodeOwnershipMismatch,
                        "Node belongs to another Agent",
                    )
                }),
            }
        }))
        .collect::<Vec<_>>();
    let inventory_bytes = serde_json::to_vec(&parsed.inventory).expect("inventory serializes");
    let inventory_hash = format!("0x{:x}", Sha256::digest(&inventory_bytes));
    let prior_inventory_hash: Option<String> =
        sqlx::query_scalar("SELECT inventory_sha256 FROM agents WHERE agent_id=?")
            .bind(&auth.agent_id)
            .fetch_optional(&mut *tx)
            .await
            .unwrap_or(None);
    let inventory_unchanged = parsed.inventory.revision == agent.last_inventory_revision as u64
        && prior_inventory_hash.as_deref() == Some(inventory_hash.as_str());
    if parsed.inventory.revision == agent.last_inventory_revision as u64
        && prior_inventory_hash.is_some()
        && !inventory_unchanged
    {
        return store_rejected(
            tx,
            &parsed,
            hash.clone(),
            rejected(
                parsed.report_id,
                hash,
                platpulse_core::RejectionCode::InventoryRevisionConflict,
                "Inventory content conflicts at the accepted revision",
            ),
            &request_id.0,
        )
        .await;
    }
    let inventory_disposition = if inventory_unchanged {
        InventoryDisposition::Unchanged
    } else {
        InventoryDisposition::Accepted
    };
    let disposition = if nodes
        .iter()
        .any(|node| node.current == NodeCurrentDisposition::Rejected)
        || samples
            .iter()
            .any(|sample| sample.disposition != SampleDispositionKind::Accepted)
    {
        ReceiptDisposition::PartiallyAccepted
    } else {
        ReceiptDisposition::Accepted
    };
    let receipt = ReportReceipt {
        report_id: parsed.report_id,
        disposition,
        report_body_sha256: hash.clone(),
        server_version: crate::VERSION.to_owned(),
        supported_protocol_majors: SUPPORTED_PROTOCOL_MAJORS.to_vec(),
        server_time: now(),
        rotation_hint: None,
        inventory: Some(inventory_disposition),
        rejections: vec![],
        nodes,
        samples,
    };
    let stored = serde_json::to_vec(&receipt).expect("receipt serializes");
    let inserted = sqlx::query("INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)").bind(parsed.report_id.to_string()).bind(&auth.agent_id).bind(parsed.agent_epoch as i64).bind(parsed.boot_id.to_string()).bind(parsed.report_sequence as i64).bind(hash.to_string()).bind(disposition_name(receipt.disposition)).bind(&stored).bind(&now_text).execute(&mut *tx).await;
    if inserted.is_err() {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    let lifecycle_status = match parsed.boot_transition {
        platpulse_core::BootTransition::Closing => "closed",
        _ => "active",
    };
    let boot_upsert = sqlx::query("INSERT INTO agent_boots (agent_id, agent_epoch, boot_id, status, previous_boot_id, last_sequence, close_report_id, closed_at, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, CASE WHEN ?='closed' THEN ? ELSE NULL END, CASE WHEN ?='closed' THEN ? ELSE NULL END, ?, ?) ON CONFLICT(agent_id, agent_epoch, boot_id) DO UPDATE SET status=excluded.status, previous_boot_id=COALESCE(excluded.previous_boot_id, agent_boots.previous_boot_id), last_sequence=MAX(agent_boots.last_sequence, excluded.last_sequence), close_report_id=COALESCE(excluded.close_report_id, agent_boots.close_report_id), closed_at=COALESCE(excluded.closed_at, agent_boots.closed_at), updated_at=excluded.updated_at")
        .bind(&auth.agent_id)
        .bind(parsed.agent_epoch as i64)
        .bind(parsed.boot_id.to_string())
        .bind(lifecycle_status)
        .bind(parsed.previous_boot_id.map(|v| v.to_string()))
        .bind(parsed.report_sequence as i64)
        .bind(lifecycle_status)
        .bind(parsed.report_id.to_string())
        .bind(lifecycle_status)
        .bind(&now_text)
        .bind(&now_text)
        .bind(&now_text)
        .execute(&mut *tx)
        .await;
    if boot_upsert.is_err() {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    let clock_skew_ms = parsed.host.clock_skew.latest;
    let clock_status = match clock_skew_ms {
        Some(value) if value.abs() > crate::http::agent::CLOCK_UNRELIABLE_THRESHOLD_MS => {
            "clock_unreliable"
        }
        Some(_) => "known",
        None => "unknown",
    };
    let should_activate_new_boot = parsed.boot_transition
        == platpulse_core::BootTransition::DrainedPrevious
        && parsed
            .previous_boot_id
            .as_ref()
            .map(ToString::to_string)
            .as_deref()
            == agent.active_boot_id.as_deref();
    let shutdown_diag = parsed.host.spool.latest.as_ref();
    let shutdown_state = shutdown_diag
        .and_then(|v| v.shutdown_state.as_deref())
        .unwrap_or(match parsed.boot_transition {
            platpulse_core::BootTransition::Closing => "final_stored",
            _ => "running",
        });
    let updated = sqlx::query("UPDATE agents SET active_boot_id=?, active_boot_status=?, previous_boot_id=?, close_report_id=CASE WHEN ?='closed' THEN ? ELSE close_report_id END, last_report_sequence=?, last_inventory_revision=?, inventory_sha256=?, last_received_at=?, clock_skew_ms=?, clock_status=?, agent_capabilities_json=?, shutdown_state=?, shutdown_started_at=?, shutdown_deadline_at=?, shutdown_finished_at=?, shutdown_unresolved_from=?, shutdown_unresolved_to=?, shutdown_last_error=?, shutdown_forced=?, shutdown_report_id=CASE WHEN ?='closed' THEN ? ELSE shutdown_report_id END, shutdown_report_sequence=?, shutdown_updated_at=?, updated_at=? WHERE agent_id=?")
        .bind(parsed.boot_id.to_string()).bind(lifecycle_status)
        .bind(parsed.previous_boot_id.map(|v| v.to_string()))
        .bind(lifecycle_status).bind(parsed.report_id.to_string())
        .bind(parsed.report_sequence as i64).bind(parsed.inventory.revision as i64)
        .bind(&inventory_hash).bind(&now_text).bind(clock_skew_ms).bind(clock_status)
        .bind(capabilities)
        .bind(shutdown_state)
        .bind(shutdown_diag.and_then(|v| v.shutdown_started_at.as_ref()).map(ToString::to_string))
        .bind(shutdown_diag.and_then(|v| v.shutdown_deadline_at.as_ref()).map(ToString::to_string))
        .bind(shutdown_diag.and_then(|v| v.shutdown_finished_at.as_ref()).map(ToString::to_string))
        .bind(shutdown_diag.and_then(|v| v.shutdown_unresolved_range.map(|range| range.0 as i64)))
        .bind(shutdown_diag.and_then(|v| v.shutdown_unresolved_range.map(|range| range.1 as i64)))
        .bind(shutdown_diag.and_then(|v| v.shutdown_last_error.as_deref()))
        .bind(shutdown_diag.and_then(|v| v.shutdown_forced).unwrap_or(false) as i64)
        .bind(lifecycle_status)
        .bind(parsed.report_id.to_string())
        .bind(parsed.report_sequence as i64)
        .bind(&now_text)
        .bind(&now_text)
        .bind(&auth.agent_id)
        .execute(&mut *tx)
        .await;
    if should_activate_new_boot {
        sqlx::query("UPDATE agents SET active_boot_status='active', previous_boot_id=?, close_applied_at=? WHERE agent_id=?")
            .bind(parsed.previous_boot_id.map(|v| v.to_string()))
            .bind(&now_text)
            .bind(&auth.agent_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ())
            .ok();
        sqlx::query("UPDATE agent_boots SET status='closed', closed_at=?, updated_at=? WHERE agent_id=? AND agent_epoch=? AND boot_id=?")
            .bind(&now_text).bind(&now_text).bind(&auth.agent_id)
            .bind(parsed.agent_epoch as i64)
            .bind(parsed.previous_boot_id.map(|v| v.to_string()))
            .execute(&mut *tx)
            .await
            .map_err(|_| ())
            .ok();
    }
    if updated.is_err() {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    // Alert evaluation runs in the same transaction as the accepted
    // projection (design §1058: Alert input and invalidation belong to the
    // report transaction), so a transition is never observed without its
    // accepted facts. The sweep re-evaluates the remaining subjects. Only
    // Nodes owned by this Agent may be evaluated: a partially accepted or
    // hostile report must never advance another Agent's Node alert state.
    // Fail safe: without ownership proof no reported Node is evaluated here;
    // the sweep re-evaluates owned subjects on its own schedule.
    let owned: Vec<String> = sqlx::query_scalar("SELECT node_id FROM nodes WHERE agent_id = ?")
        .bind(&auth.agent_id)
        .fetch_all(&mut *tx)
        .await
        .unwrap_or_default();
    let node_ids: Vec<String> = parsed
        .nodes
        .iter()
        .map(|node| node.node_id.to_string())
        .filter(|node_id| owned.contains(node_id))
        .collect();
    let alert_changes = match crate::alerts::evaluate_report(
        &mut tx,
        &auth.agent_id,
        &node_ids,
        crate::auth::now_utc(),
    )
    .await
    {
        Ok(changes) => changes,
        Err(_) => {
            return error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Server database is unavailable",
            );
        }
    };
    if tx.commit().await.is_err() {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    if let Err(error) =
        crate::retention::cleanup_raw_block_summaries(state.db().pool(), crate::auth::now_utc())
            .await
    {
        eprintln!(
            "raw block retention cleanup deferred after ingestion: {}",
            crate::redaction::redact_sensitive(&error.to_string())
        );
    }
    state
        .admin_realtime()
        .publish("node", None::<String>, parsed.report_sequence);
    state
        .public_realtime()
        .publish("node", None::<String>, parsed.report_sequence);
    if alert_changes > 0 {
        state
            .admin_realtime()
            .publish("alerts", None::<String>, parsed.report_sequence);
    }
    (StatusCode::OK, Json(ReportResponse { receipt })).into_response()
}

fn valid_report_content_type(value: Option<&axum::http::HeaderValue>) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|media_type| {
                media_type.trim().eq_ignore_ascii_case("application/json")
            })
        })
}

fn report_content_encoding_supported(value: Option<&axum::http::HeaderValue>) -> bool {
    value.is_none()
}

async fn report_request_boundary(request: Request, next: Next) -> Response {
    if request.method() != axum::http::Method::POST {
        return error_from_request(
            &request,
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "report endpoint requires POST",
        );
    }
    if !valid_report_content_type(request.headers().get(axum::http::header::CONTENT_TYPE)) {
        return error_from_request(
            &request,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_content_type",
            "Agent reports require application/json",
        );
    }
    if !report_content_encoding_supported(
        request.headers().get(axum::http::header::CONTENT_ENCODING),
    ) {
        return error_from_request(
            &request,
            StatusCode::BAD_REQUEST,
            "content_encoding_unsupported",
            "compressed Agent reports are not supported",
        );
    }
    next.run(request).await
}

async fn body_size_boundary(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, platpulse_core::protocol::MAX_REPORT_BODY_BYTES + 1).await {
        Ok(bytes) if validate_report_body_size(bytes.len()).is_ok() => bytes,
        Ok(_) => {
            let request = Request::from_parts(parts, Body::empty());
            return error_from_request(
                &request,
                StatusCode::PAYLOAD_TOO_LARGE,
                "report_too_large",
                "Agent report exceeds the protocol size limit",
            );
        }
        Err(_) => {
            let request = Request::from_parts(parts, Body::empty());
            return error_from_request(
                &request,
                StatusCode::PAYLOAD_TOO_LARGE,
                "report_too_large",
                "Agent report exceeds the protocol size limit",
            );
        }
    };
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}
fn error_from_request(
    request: &Request,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| &*id.0)
        .unwrap_or("unknown");
    error(request_id, status, code, message)
}

pub(crate) fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/reports", axum::routing::post(handler))
        .layer(from_fn(body_size_boundary))
        .layer(from_fn(report_request_boundary))
        .layer(from_fn(
            |request: axum::extract::Request, next: Next| async move {
                let mut response = next.run(request).await;
                response.headers_mut().insert(
                    ROUTE_GROUP_HEADER,
                    axum::http::HeaderValue::from_static("agent"),
                );
                response
            },
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthConfig, format_rfc3339, now_utc};
    use crate::database::{ServerDatabaseConfig, initialize};
    use crate::enrollment::AgentAuthInfo;
    use crate::network::create_network;
    use crate::secrets::{create_pepper_file, load_pepper_file};
    use axum::body::{Bytes, to_bytes};
    use axum::extract::{Extension, State};
    use axum::http::StatusCode;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn state_with_agent() -> (TempDir, AppState, String) {
        let dir = TempDir::new().unwrap();
        let database = initialize(ServerDatabaseConfig::new(dir.path().join("server.db")))
            .await
            .unwrap();
        let pepper_path = dir.path().join("pepper");
        create_pepper_file(&pepper_path).unwrap();
        let auth = AuthConfig::development(
            load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        create_network(
            &database,
            "platon-mainnet",
            "PlatON Mainnet",
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            210425,
            210425,
            "lat",
        )
        .await
        .unwrap();
        let agent_id = "0195f2a1-0011-4011-8011-000000000011".to_owned();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES (?, 1, NULL, NULL, NULL, ?, ?)").bind(&agent_id).bind("2026-08-12T08:00:00Z").bind("2026-08-12T08:00:00Z").execute(database.pool()).await.unwrap();
        (dir, AppState::new(database, None, auth), agent_id)
    }

    async fn submit(state: &AppState, agent_id: &str, body: Vec<u8>) -> ReportReceipt {
        let response = handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.to_owned(),
                credential_id: "test-credential".to_owned(),
            }),
            Extension(RequestId(Arc::from("test-request"))),
            Bytes::from(body),
        )
        .await;
        if response.status() != StatusCode::OK {
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            panic!(
                "unexpected report response for agent {agent_id}: {}",
                String::from_utf8_lossy(&body)
            );
        }
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice::<ReportResponse>(&body)
            .unwrap()
            .receipt
    }

    #[test]
    fn boundary_rejects_method_content_type_and_encoding() {
        use axum::http::HeaderValue;
        assert!(valid_report_content_type(Some(&HeaderValue::from_static(
            "application/json; charset=utf-8"
        ))));
        assert!(!valid_report_content_type(Some(&HeaderValue::from_static(
            "text/plain"
        ))));
        assert!(report_content_encoding_supported(None));
        assert!(!report_content_encoding_supported(Some(
            &HeaderValue::from_static("gzip")
        )));
        assert!(validate_report_body_size(platpulse_core::protocol::MAX_REPORT_BODY_BYTES).is_ok());
        assert_eq!(
            validate_report_body_size(platpulse_core::protocol::MAX_REPORT_BODY_BYTES + 1),
            Err(StatusCode::PAYLOAD_TOO_LARGE)
        );
    }
    #[tokio::test]
    async fn boot_sequence_gaps_and_competing_boots_are_recorded_and_rejected() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        report.report_sequence = 3;
        report.report_id = "0195f2a1-0026-4026-8026-000000000026".parse().unwrap();
        submit(&state, &agent_id, serde_json::to_vec(&report).unwrap()).await;
        let gaps: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM report_sequence_gaps WHERE agent_id=?")
                .bind(&agent_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(gaps, 1);

        let old_boot = report.boot_id;
        report.boot_id = "0195f2a1-0027-4027-8027-000000000027".parse().unwrap();
        report.report_sequence = 1;
        report.report_id = "0195f2a1-0028-4028-8028-000000000028".parse().unwrap();
        let response = handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.clone(),
                credential_id: "test".into(),
            }),
            Extension(RequestId(Arc::from("test-request"))),
            Bytes::from(serde_json::to_vec(&report).unwrap()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let receipt: ReportResponse =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(receipt.receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            receipt.receipt.rejections[0].code,
            platpulse_core::RejectionCode::ConflictingBoot
        );
        assert_ne!(old_boot, report.boot_id);
    }
    #[tokio::test]
    async fn closing_then_drained_previous_atomically_rotates_boot_and_rejects_old_reports() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut closing: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let old_boot = closing.boot_id;
        closing.report_sequence = 2;
        closing.report_id = "0195f2a1-0030-4030-8030-000000000030".parse().unwrap();
        closing.boot_transition = platpulse_core::BootTransition::Closing;
        submit(&state, &agent_id, serde_json::to_vec(&closing).unwrap()).await;

        let mut stale = closing.clone();
        stale.report_id = "0195f2a1-0031-4031-8031-000000000031".parse().unwrap();
        stale.report_sequence = 3;
        stale.boot_transition = platpulse_core::BootTransition::Continuing;
        let stale_receipt = submit(&state, &agent_id, serde_json::to_vec(&stale).unwrap()).await;
        assert_eq!(
            stale_receipt.rejections[0].code,
            platpulse_core::RejectionCode::StaleReport
        );

        let mut next = closing;
        next.boot_id = "0195f2a1-0032-4032-8032-000000000032".parse().unwrap();
        next.previous_boot_id = Some(old_boot);
        next.boot_transition = platpulse_core::BootTransition::DrainedPrevious;
        next.report_sequence = 1;
        next.report_id = "0195f2a1-0033-4033-8033-000000000033".parse().unwrap();
        let next_receipt = submit(&state, &agent_id, serde_json::to_vec(&next).unwrap()).await;
        assert_eq!(next_receipt.disposition, ReceiptDisposition::Accepted);

        let statuses = sqlx::query_as::<_, (String, String)>(
            "SELECT boot_id, status FROM agent_boots WHERE agent_id=? ORDER BY boot_id",
        )
        .bind(&agent_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(statuses.len(), 2);
        assert!(
            statuses
                .iter()
                .any(|(boot, status)| boot == &old_boot.to_string() && status == "closed")
        );
        assert!(
            statuses
                .iter()
                .any(|(boot, status)| boot == &next.boot_id.to_string() && status == "active")
        );
        let security_events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=?")
                .bind(&agent_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(security_events, 0);
    }
    #[tokio::test]
    async fn accepted_inventory_persists_observations_and_replay_is_exact() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let body = include_bytes!("../../../platpulse-core/tests/fixtures/report_v1_minimal.json");
        let first = submit(&state, &agent_id, body.to_vec()).await;
        assert_eq!(first.disposition, ReceiptDisposition::Accepted);
        assert_eq!(first.inventory, Some(InventoryDisposition::Accepted));
        assert_eq!(first.nodes.len(), 1);
        let statuses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM component_status")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert!(statuses >= 13);
        let memory: i64 = sqlx::query_scalar(
            "SELECT memory_used_bytes FROM current_host_observations WHERE agent_id = ?",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(memory, 4_294_967_296);
        let replay = submit(&state, &agent_id, body.to_vec()).await;
        assert_eq!(replay, first);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_report_receipts")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn replay_sample_is_terminal_and_does_not_rewrite_history() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report.report_id = "0195f2a1-0013-4013-8013-000000000201".parse().unwrap();
        let node_id = report.inventory.nodes[0].node_id;
        let mut identity = report.nodes[0]
            .chain
            .network_identity
            .latest
            .clone()
            .unwrap();
        identity.genesis_hash =
            "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap();
        identity.address_hrp = Some("lat".to_owned());
        report
            .block_summaries
            .push(platpulse_core::block::BlockSummary {
                node_id,
                network_identity: identity,
                block_number: 10,
                block_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .parse()
                    .unwrap(),
                parent_hash: "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .parse()
                    .unwrap(),
                block_timestamp_ms: 1_000,
                observed_at: report.generated_at,
                transaction_count: 3,
                block_interval_ms: None,
                source: platpulse_core::block::BlockSource::Subscription,
                attribution: platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                    "0x1111111111111111111111111111111111111111"
                        .parse()
                        .unwrap(),
                    "test",
                ),
            });
        report.validate().unwrap();
        let first = report.clone();
        submit(&state, &agent_id, serde_json::to_vec(&first).unwrap()).await;
        sqlx::query("UPDATE block_summaries SET accepted_at=? WHERE node_id=?")
            .bind("2026-01-01T00:00:00Z")
            .bind(node_id.to_string())
            .execute(state.db().pool())
            .await
            .unwrap();
        crate::retention::cleanup_raw_block_summaries(
            state.db().pool(),
            crate::auth::parse_rfc3339("2026-08-12T08:00:00Z").unwrap(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE block_history_state SET historical_high_watermark=100, cumulative_block_count=1, cumulative_transaction_count=3 WHERE node_id=?").bind(node_id.to_string()).execute(state.db().pool()).await.unwrap();

        let mut replay = report;
        replay.report_sequence = 2;
        replay.report_id = "0195f2a1-0013-4013-8013-000000000202".parse().unwrap();
        let receipt = submit(&state, &agent_id, serde_json::to_vec(&replay).unwrap()).await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(
            receipt.samples[0].disposition,
            SampleDispositionKind::TerminalRejected
        );
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::ResyncReplay
        );
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=?")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        let counters: (i64, i64) = sqlx::query_as("SELECT cumulative_block_count, cumulative_transaction_count FROM block_history_state WHERE node_id=?").bind(node_id.to_string()).fetch_one(state.db().pool()).await.unwrap();
        assert_eq!(summaries, 0);
        assert_eq!(counters, (1, 3));
    }

    #[tokio::test]
    async fn two_node_inventory_keeps_host_scoped_once_and_nodes_separate() {
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
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
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
        let node_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE agent_id = ?")
            .bind(&agent_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(node_count, 2);
        let host_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM current_host_observations WHERE agent_id = ?")
                .bind(&agent_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(host_count, 1);
        let node_rpc_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM component_status WHERE agent_id = ? AND scope = 'node' AND component_key = 'rpc'",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(node_rpc_count, 2);
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
    async fn accepted_empty_inventory_retires_omitted_node() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
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
        let lifecycle: String = sqlx::query_scalar("SELECT lifecycle FROM nodes WHERE node_id = ?")
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(lifecycle, "retired");

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
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        restored.report_sequence = 4;
        restored.report_id = "0195f2a1-0013-4013-8013-000000000100".parse().unwrap();
        restored.inventory.revision = 3;
        submit(&state, &agent_id, serde_json::to_vec(&restored).unwrap()).await;
        let lifecycle: String = sqlx::query_scalar("SELECT lifecycle FROM nodes WHERE node_id = ?")
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(lifecycle, "active");
    }

    #[tokio::test]
    async fn stale_inventory_revision_is_rejected_without_lifecycle_mutation() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut original: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        original.inventory.revision = 2;
        submit(&state, &agent_id, serde_json::to_vec(&original).unwrap()).await;
        let mut stale = original.clone();
        stale.report_sequence = 2;
        stale.report_id = "0195f2a1-0013-4013-8013-000000000101".parse().unwrap();
        stale.inventory.revision = 1;
        let response = handler(
            State(state.clone()),
            Extension(AgentAuthInfo {
                agent_id: agent_id.clone(),
                credential_id: "test".into(),
            }),
            Extension(RequestId(Arc::from("stale"))),
            Bytes::from(serde_json::to_vec(&stale).unwrap()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let receipt: ReportResponse =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(receipt.receipt.disposition, ReceiptDisposition::Rejected);
        assert_eq!(
            receipt.receipt.rejections[0].code,
            platpulse_core::RejectionCode::InventoryRevisionConflict
        );
        let lifecycle: String = sqlx::query_scalar("SELECT lifecycle FROM nodes WHERE node_id = ?")
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(lifecycle, "active");
    }

    #[tokio::test]
    async fn divergent_hash_is_append_only_idempotent_and_preserves_summary_and_counters() {
        let (_dir, state, _agent_id) = state_with_agent().await;
        let node_id: platpulse_core::identity::NodeId =
            "0195f2a1-0014-4014-8014-000000000014".parse().unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, ?, 'platon-mainnet', 'ws://127.0.0.1:6790', 'active', 'public', 1, ?, ?)")
            .bind(node_id.to_string()).bind("0195f2a1-0011-4011-8011-000000000011").bind("2026-08-12T08:00:00Z").bind("2026-08-12T08:00:00Z").execute(state.db().pool()).await.unwrap();
        let identity = platpulse_core::network::NetworkIdentity {
            genesis_hash: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            chain_id: 210425,
            p2p_network_id: 210425,
            address_hrp: Some("lat".into()),
        };
        let make_sample =
            |block_hash: platpulse_core::hex::Hash32| platpulse_core::block::BlockSummary {
                node_id,
                network_identity: identity.clone(),
                block_number: 42,
                block_hash,
                parent_hash: "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                    .parse()
                    .unwrap(),
                block_timestamp_ms: 1_000,
                observed_at: "2026-08-12T09:00:00Z".parse().unwrap(),
                transaction_count: 3,
                block_interval_ms: None,
                source: platpulse_core::block::BlockSource::Subscription,
                attribution: platpulse_core::block::BlockProductionAttribution::unknown_attribution(
                    "0x1111111111111111111111111111111111111111"
                        .parse()
                        .unwrap(),
                    "test",
                ),
            };
        let hash_a: platpulse_core::hex::Hash32 =
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .parse()
                .unwrap();
        let hash_b: platpulse_core::hex::Hash32 =
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .parse()
                .unwrap();
        let first = make_sample(hash_a.clone());
        let second = make_sample(hash_b);
        let mut tx = state.db().pool().begin().await.unwrap();
        assert!(
            !observe_block_identity(&mut tx, &first, "2026-08-12T09:00:00Z")
                .await
                .unwrap()
        );
        sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'subscription', ?, 'unknown', 'unknown', 'test', ?)")
            .bind(node_id.to_string()).bind(42_i64).bind(hash_a.to_string()).bind(first.parent_hash.to_string()).bind(identity.genesis_hash.to_string()).bind(210425_i64).bind(210425_i64).bind("lat").bind(1000_i64).bind(first.observed_at.to_string()).bind(3_i64).bind(first.attribution.coinbase.to_string()).bind("2026-08-12T09:00:00Z").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, updated_at) VALUES (?, 42, 1, 3, 0, ?)").bind(node_id.to_string()).bind("2026-08-12T09:00:00Z").execute(&mut *tx).await.unwrap();
        assert!(
            observe_block_identity(&mut tx, &second, "2026-08-12T09:01:00Z")
                .await
                .unwrap()
        );
        assert!(
            observe_block_identity(&mut tx, &second, "2026-08-12T09:01:00Z")
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        let summary_hash: String = sqlx::query_scalar(
            "SELECT block_hash FROM block_summaries WHERE node_id=? AND block_number=42",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let counters: (i64, i64) = sqlx::query_as("SELECT cumulative_block_count, cumulative_transaction_count FROM block_history_state WHERE node_id=?").bind(node_id.to_string()).fetch_one(state.db().pool()).await.unwrap();
        let divergence_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chain_divergence_observations WHERE node_id=? AND height=42",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(summary_hash, hash_a.to_string());
        assert_eq!(counters, (1, 3));
        assert_eq!(divergence_count, 1);
    }

    #[tokio::test]
    async fn current_head_regresses_without_rewriting_history_and_restart_preserves_state() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut first: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
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
        sqlx::query("UPDATE block_history_state SET historical_high_watermark=100, cumulative_block_count=4 WHERE node_id=?")
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
            sqlx::query_scalar("SELECT current_head FROM block_history_state WHERE node_id=?")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        let high: i64 = sqlx::query_scalar(
            "SELECT historical_high_watermark FROM block_history_state WHERE node_id=?",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let cumulative: i64 = sqlx::query_scalar(
            "SELECT cumulative_block_count FROM block_history_state WHERE node_id=?",
        )
        .bind(node_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        let state_name: String =
            sqlx::query_scalar("SELECT resync_state FROM block_history_state WHERE node_id=?")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(current, 4);
        assert_eq!(high, 100);
        assert_eq!(cumulative, 4);
        assert_eq!(state_name, "resyncing");
        let db_path = _dir.path().join("server.db");
        // The temporary database remains valid across a fresh connection; closing
        // the AppState-owned pool is intentionally covered by the next process.
        let reopened = initialize(ServerDatabaseConfig::new(db_path))
            .await
            .unwrap();
        let persisted: (i64, i64, String) = sqlx::query_as("SELECT current_head, historical_high_watermark, resync_state FROM block_history_state WHERE node_id=?").bind(node_id.to_string()).fetch_one(reopened.pool()).await.unwrap();
        assert_eq!(persisted, (4, 100, "resyncing".to_owned()));
    }
    #[tokio::test]
    async fn failed_node_keeps_host_and_other_node_projection() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
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
            "SELECT rpc_client_version FROM current_node_chain_observations WHERE node_id = ?",
        )
        .bind(second_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(client, "other");
        let memory: i64 = sqlx::query_scalar(
            "SELECT memory_used_bytes FROM current_host_observations WHERE agent_id = ?",
        )
        .bind(&agent_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(memory, 4_294_967_296);
        let state: String = sqlx::query_scalar(
            "SELECT state FROM component_status WHERE node_id = ? AND component_key = 'rpc'",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(state, "error");
    }

    /// Source Agent owns the fixture Node; a second Agent exists as a
    /// transfer target. The source report deliberately keeps the fixture
    /// identity (which mismatches the Registry) so identity assertions in
    /// transfer tests are deterministic.
    async fn state_with_source_and_target() -> (TempDir, AppState, String, String, String) {
        let (dir, state, source_agent) = state_with_agent().await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        submit(
            &state,
            &source_agent,
            serde_json::to_vec(&source_report).unwrap(),
        )
        .await;
        let target_agent = "0195f2a1-0019-4019-8019-000000000019".to_owned();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, active_boot_id, last_report_sequence, last_received_at, created_at, updated_at) VALUES (?, 1, NULL, NULL, NULL, ?, ?)")
            .bind(&target_agent)
            .bind("2026-08-12T08:00:00Z")
            .bind("2026-08-12T08:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        (
            dir,
            state,
            source_agent,
            target_agent,
            source_report.inventory.nodes[0].node_id.to_string(),
        )
    }

    /// Build the target Agent's declaration of the source-owned Node. The
    /// report identity matches the registered Registry tuple when
    /// `matching_identity` is true (genesis `0x…01`, chain/p2p 210425,
    /// hrp `lat`); otherwise the fixture's contradictory identity is kept.
    fn target_declaration(
        source_report: &AgentReport,
        target_agent: &str,
        matching_identity: bool,
        with_sample: bool,
    ) -> AgentReport {
        let mut report = source_report.clone();
        report.agent_id = target_agent.parse().unwrap();
        report.boot_id = "0195f2a1-0034-4034-8034-000000000034".parse().unwrap();
        report.previous_boot_id = None;
        report.report_sequence = 1;
        report.report_id = "0195f2a1-0035-4035-8035-000000000035".parse().unwrap();
        if matching_identity {
            let identity = report.nodes[0]
                .chain
                .network_identity
                .latest
                .as_mut()
                .unwrap();
            identity.genesis_hash =
                "0x0000000000000000000000000000000000000000000000000000000000000001"
                    .parse()
                    .unwrap();
            identity.address_hrp = Some("lat".to_owned());
        }
        if with_sample {
            let identity = report.nodes[0]
                .chain
                .network_identity
                .latest
                .clone()
                .unwrap();
            report
                .block_summaries
                .push(platpulse_core::block::BlockSummary {
                    node_id: report.inventory.nodes[0].node_id,
                    network_identity: identity,
                    block_number: 10,
                    block_hash:
                        "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .parse()
                            .unwrap(),
                    parent_hash:
                        "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                            .parse()
                            .unwrap(),
                    block_timestamp_ms: 1_000,
                    observed_at: report.generated_at,
                    transaction_count: 3,
                    block_interval_ms: None,
                    source: platpulse_core::block::BlockSource::Subscription,
                    attribution:
                        platpulse_core::block::BlockProductionAttribution::unknown_attribution(
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

    async fn pending_transfer(state: &AppState, node_id: &str, source: &str, target: &str) {
        // Windows are relative to the real clock so the fixture never
        // silently expires mid-suite.
        let now = now_utc();
        let created = format_rfc3339(now - time::Duration::hours(2));
        let expires = format_rfc3339(now + time::Duration::hours(2));
        sqlx::query(
            "INSERT INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, operator_reason, created_at, expires_at, updated_at) VALUES ('transfer-pending-1', ?, ?, ?, 'pending', 'move the validator', ?, ?, ?)",
        )
        .bind(node_id.to_string())
        .bind(source)
        .bind(target)
        .bind(&created)
        .bind(&expires)
        .bind(&created)
        .execute(state.db().pool())
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn matching_declaration_completes_transfer_atomically_and_source_loses_ownership() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let declaration = target_declaration(&source_report, &target_agent, true, true);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Accepted);
        assert_eq!(
            receipt.samples[0].disposition,
            SampleDispositionKind::Accepted
        );

        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=?")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, target_agent);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "completed");
        let completed: Option<String> = sqlx::query_scalar(
            "SELECT completed_at FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(completed.is_some());
        // The same transaction merged the matching sample into history.
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=?")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 1);
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_completed'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
        // The target's security counter stays clean: this was a valid
        // declaration, not a contradiction.
        let target_events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=?")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(target_events, 0);

        // The source Agent's later submission of the same Node only rejects
        // that Node entry and records a security event; ownership stays with
        // the target (design §4.4 step 6).
        let mut stale_source = source_report.clone();
        stale_source.report_sequence = 2;
        stale_source.report_id = "0195f2a1-0013-4013-8013-000000000301".parse().unwrap();
        let source_receipt = submit(
            &state,
            &source_agent,
            serde_json::to_vec(&stale_source).unwrap(),
        )
        .await;
        assert_eq!(
            source_receipt.disposition,
            ReceiptDisposition::PartiallyAccepted
        );
        assert_eq!(
            source_receipt.nodes[0].current,
            NodeCurrentDisposition::Rejected
        );
        assert_eq!(
            source_receipt.nodes[0].rejections[0].code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        let owner_after: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=?")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner_after, target_agent);
        let source_events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=?")
                .bind(&source_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(source_events, 1);
    }

    #[tokio::test]
    async fn identity_mismatch_blocks_transfer_and_never_merges_history() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        // The fixture identity (0xaaa…, no hrp) contradicts the Registry.
        let declaration = target_declaration(&source_report, &target_agent, false, true);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        assert_eq!(
            receipt.nodes[0].rejections[0].code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "identity_mismatch");
        let fields: String = sqlx::query_scalar(
            "SELECT mismatched_fields FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&fields).unwrap(),
            vec!["genesis_hash", "address_hrp"]
        );
        // Ownership never switched and no history merged.
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=?")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=?")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 0);
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_identity_mismatch'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
        let events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=?")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(events, 1);
    }

    #[tokio::test]
    async fn declaration_without_identity_probe_keeps_transfer_pending() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let mut declaration = target_declaration(&source_report, &target_agent, true, false);
        declaration.nodes[0].chain.network_identity.status = ComponentStatus::Error;
        declaration.nodes[0].chain.network_identity.latest = None;
        declaration.nodes[0]
            .chain
            .network_identity
            .latest_observed_at = None;
        declaration.nodes[0].chain.network_identity.error =
            Some(platpulse_core::component::BoundedError {
                code: "rpc_unreachable".into(),
                message: "identity probe failed".into(),
            });
        declaration.validate().unwrap();
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "pending");
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=?")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        // An in-flight declaration without a contradiction is not a
        // security event.
        let events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=?")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(events, 0);
    }

    #[tokio::test]
    async fn expired_transfer_is_materialized_and_declaration_stays_rejected() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        sqlx::query(
            "INSERT INTO node_transfers (transfer_id, node_id, source_agent_id, target_agent_id, status, created_at, expires_at, updated_at) VALUES ('transfer-expired-1', ?, ?, ?, 'pending', '2026-08-10T08:00:00Z', '2026-08-11T08:00:00Z', '2026-08-10T08:00:00Z')",
        )
        .bind(node_id.to_string())
        .bind(&source_agent)
        .bind(&target_agent)
        .execute(state.db().pool())
        .await
        .unwrap();
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let declaration = target_declaration(&source_report, &target_agent, true, false);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-expired-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "expired");
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=?")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_kind='node_transfer_expired'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(audits, 1);
    }

    #[tokio::test]
    async fn declaration_under_different_network_key_rejects_transfer() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
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
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let mut declaration = target_declaration(&source_report, &target_agent, true, false);
        declaration.inventory.nodes[0].network_key = "platon-testnet".parse().unwrap();
        declaration.validate().unwrap();
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "rejected");
        let code: String = sqlx::query_scalar(
            "SELECT rejection_code FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(code, "network_key_mismatch");
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=?")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
    }

    #[tokio::test]
    async fn matching_sample_never_merges_history_for_ownership_rejected_node() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        pending_transfer(&state, &node_id, &source_agent, &target_agent).await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        // The observation contradicts the Registry (fixture identity), but
        // the block sample claims the registered tuple: the declaration must
        // still be rejected on the observation, and the matching sample must
        // never merge into the registered Network history.
        let mut declaration = target_declaration(&source_report, &target_agent, false, true);
        let mut sample = declaration.block_summaries[0].clone();
        sample.network_identity.genesis_hash =
            "0x0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap();
        sample.network_identity.address_hrp = Some("lat".to_owned());
        declaration.block_summaries[0] = sample;
        declaration.validate().unwrap();
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        assert_eq!(
            receipt.samples[0].rejection.as_ref().unwrap().code,
            platpulse_core::RejectionCode::NodeOwnershipMismatch
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM node_transfers WHERE transfer_id='transfer-pending-1'",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(status, "identity_mismatch");
        // Neither the identity window nor the summaries nor the counters
        // were touched by the matching sample.
        let window: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_identity_window WHERE node_id=?")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(window, 0);
        let summaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM block_summaries WHERE node_id=?")
                .bind(node_id.to_string())
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(summaries, 0);
        let counts: Option<(i64, i64)> = sqlx::query_as(
            "SELECT cumulative_block_count, cumulative_transaction_count FROM block_history_state WHERE node_id=?",
        )
        .bind(node_id.to_string())
        .fetch_optional(state.db().pool())
        .await
        .unwrap();
        // The row exists only from the source's earlier report; the
        // rejected declaration never incremented the counters.
        assert_eq!(counts, Some((0, 0)));
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=?")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
    }

    #[tokio::test]
    async fn declaration_without_pending_transfer_is_an_ownership_conflict() {
        let (_dir, state, source_agent, target_agent, node_id) =
            state_with_source_and_target().await;
        let source_report: AgentReport = serde_json::from_slice(include_bytes!(
            "../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let declaration = target_declaration(&source_report, &target_agent, true, false);
        let receipt = submit(
            &state,
            &target_agent,
            serde_json::to_vec(&declaration).unwrap(),
        )
        .await;
        assert_eq!(receipt.disposition, ReceiptDisposition::PartiallyAccepted);
        assert_eq!(receipt.nodes[0].current, NodeCurrentDisposition::Rejected);
        let owner: String = sqlx::query_scalar("SELECT agent_id FROM nodes WHERE node_id=?")
            .bind(node_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .unwrap();
        assert_eq!(owner, source_agent);
        let events: i64 =
            sqlx::query_scalar("SELECT security_event_count FROM agents WHERE agent_id=?")
                .bind(&target_agent)
                .fetch_one(state.db().pool())
                .await
                .unwrap();
        assert_eq!(events, 1);
    }
}
