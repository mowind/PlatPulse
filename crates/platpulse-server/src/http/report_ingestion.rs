//! Transactional AgentReport ingestion for the first Inventory vertical slice.
use std::str::FromStr;

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
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
    AgentReport, InventoryDisposition, NodeCurrentDisposition, NodeReceipt, ReceiptDisposition,
    ReportId, ReportReceipt, Rfc3339, SampleDisposition, SampleDispositionKind, SampleRef,
    Sha256Hex,
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
        let inserted = sqlx::query("INSERT OR IGNORE INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, block_interval_ms, source, coinbase, seal_signer_key_fingerprint, seal_signer_match, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, accepted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&node_id).bind(sample.block_number as i64).bind(sample.block_hash.to_string()).bind(sample.parent_hash.to_string())
            .bind(sample.network_identity.genesis_hash.to_string()).bind(sample.network_identity.chain_id as i64).bind(sample.network_identity.p2p_network_id as i64).bind(sample.network_identity.address_hrp.as_deref().unwrap_or(""))
            .bind(sample.block_timestamp_ms as i64).bind(sample.observed_at.to_string()).bind(sample.transaction_count as i64).bind(sample.block_interval_ms.map(|v| v as i64)).bind(match sample.source { platpulse_core::block::BlockSource::Subscription => "subscription", platpulse_core::block::BlockSource::GapBackfill => "gap_backfill" })
            .bind(sample.attribution.coinbase.to_string()).bind(sample.attribution.seal_signer_key_fingerprint.as_ref().map(ToString::to_string)).bind(signer).bind(sample.attribution.node_key.as_ref().map(|key| key.fingerprint.to_string())).bind(sample.attribution.node_key.as_ref().and_then(|key| key.valid_from.map(|value| value.to_string()))).bind(sample.attribution.node_key.as_ref().and_then(|key| key.valid_until.map(|value| value.to_string()))).bind(sample.attribution.node_key.as_ref().is_some_and(|key| key.history_complete) as i64).bind(sample.attribution.seal_recovery_rule.as_deref()).bind(sample.attribution.seal_evidence.as_deref()).bind(proposer.0).bind(proposer.1).bind(&sample.attribution.attribution_reason).bind(received_at)
            .execute(&mut **tx).await?;
        if inserted.rows_affected() == 0 {
            continue;
        }
        sqlx::query("INSERT INTO observed_network_heads (node_id, block_number, block_hash, observed_at, confidence, eligible_sources) VALUES (?, ?, ?, ?, 'unknown', '[\\\"subscription\\\"]') ON CONFLICT(node_id) DO UPDATE SET block_number=excluded.block_number, block_hash=excluded.block_hash, observed_at=excluded.observed_at, confidence=excluded.confidence, eligible_sources=excluded.eligible_sources")
            .bind(&node_id).bind(sample.block_number as i64).bind(sample.block_hash.to_string()).bind(sample.observed_at.to_string()).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO block_history_state (node_id, historical_high_watermark, cumulative_block_count, cumulative_transaction_count, cumulative_self_seal_count, updated_at) VALUES (?, ?, 1, ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET historical_high_watermark=MAX(block_history_state.historical_high_watermark, excluded.historical_high_watermark), cumulative_block_count=block_history_state.cumulative_block_count + 1, cumulative_transaction_count=block_history_state.cumulative_transaction_count + excluded.cumulative_transaction_count, cumulative_self_seal_count=block_history_state.cumulative_self_seal_count + excluded.cumulative_self_seal_count, updated_at=excluded.updated_at)")
            .bind(&node_id).bind(sample.block_number as i64).bind(sample.transaction_count as i64)
            .bind((sample.attribution.seal_signer_match == platpulse_core::block::SealSignerMatch::SignerSelf) as i64).bind(received_at).execute(&mut **tx).await?;
    }
    Ok(())
}

async fn handler(
    State(state): State<AppState>,
    Extension(auth): Extension<AgentAuthInfo>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    if body.len() > platpulse_core::protocol::MAX_REPORT_BODY_BYTES {
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
        "SELECT agent_epoch, active_boot_id, last_report_sequence, last_inventory_revision FROM agents WHERE agent_id = ?",
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
        return error(
            &request_id.0,
            StatusCode::CONFLICT,
            "conflicting_boot",
            "Boot sequence conflicts with a stored report",
        );
    }
    if let Some(active) = &agent.active_boot_id {
        if active != &parsed.boot_id.to_string() {
            if parsed.boot_transition != platpulse_core::BootTransition::DrainedPrevious
                || parsed
                    .previous_boot_id
                    .as_ref()
                    .map(ToString::to_string)
                    .as_deref()
                    != Some(active.as_str())
            {
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
        } else if agent
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
    let now_text = now().to_string();
    let capabilities =
        serde_json::to_string(&parsed.agent_capabilities).expect("capabilities serialize");
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
            return store_rejected(
                tx,
                &parsed,
                hash.clone(),
                rejected(
                    parsed.report_id,
                    hash,
                    platpulse_core::RejectionCode::NodeOwnershipMismatch,
                    "Node belongs to another Agent",
                ),
                &request_id.0,
            )
            .await;
        }
    }
    // A complete, accepted Inventory is authoritative: omitted Nodes are retired.
    if parsed.inventory.nodes.is_empty() {
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
    } else {
        let placeholders = std::iter::repeat_n("?", parsed.inventory.nodes.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE nodes SET lifecycle='retired', updated_at=? WHERE agent_id=? AND node_id NOT IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql).bind(&now_text).bind(&auth.agent_id);
        for node in &parsed.inventory.nodes {
            query = query.bind(node.node_id.to_string());
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
    if save_current(&mut tx, &parsed, &now_text).await.is_err() {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
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
    // Keep explicit gap declarations even when their block samples are absent.
    for gap in &parsed.history_gaps {
        sqlx::query("INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(gap.node_id.to_string()).bind(gap.from_height as i64).bind(gap.to_height as i64)
            .bind(format!("{:?}", gap.kind).to_lowercase()).bind(gap.recorded_at.to_string())
            .execute(&mut *tx).await.map_err(|_| ()).ok();
    }
    let nodes = parsed
        .inventory
        .nodes
        .iter()
        .map(|node| NodeReceipt {
            node_id: node.node_id,
            current: NodeCurrentDisposition::Accepted,
            accepted_component_revisions: vec![],
            rejections: vec![],
        })
        .collect();
    let samples = parsed
        .block_summaries
        .iter()
        .map(|sample| {
            let rejected = mismatches.contains(&sample.node_id);
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
                    rejection(
                        platpulse_core::RejectionCode::NetworkIdentityMismatch,
                        "Block network identity does not match the registered Network",
                    )
                }),
            }
        })
        .chain(parsed.history_gaps.iter().map(|gap| SampleDisposition {
            node_id: gap.node_id,
            sample: SampleRef::Gap {
                from_height: gap.from_height,
                to_height: gap.to_height,
            },
            disposition: SampleDispositionKind::Accepted,
            rejection: None,
        }))
        .collect::<Vec<_>>();
    let disposition = if samples
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
        inventory: Some(InventoryDisposition::Accepted),
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
    let clock_skew_ms = parsed.host.clock_skew.latest;
    let clock_status = match clock_skew_ms {
        Some(value) if value.abs() > crate::http::agent::CLOCK_UNRELIABLE_THRESHOLD_MS => {
            "clock_unreliable"
        }
        Some(_) => "known",
        None => "unknown",
    };
    let updated = sqlx::query("UPDATE agents SET active_boot_id=?, last_report_sequence=?, last_inventory_revision=?, last_received_at=?, clock_skew_ms=?, clock_status=?, agent_capabilities_json=?, updated_at=? WHERE agent_id=?").bind(parsed.boot_id.to_string()).bind(parsed.report_sequence as i64).bind(parsed.inventory.revision as i64).bind(&now_text).bind(clock_skew_ms).bind(clock_status).bind(capabilities).bind(&now_text).bind(&auth.agent_id).execute(&mut *tx).await;
    if updated.is_err() || tx.commit().await.is_err() {
        return error(
            &request_id.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Server database is unavailable",
        );
    }
    state
        .admin_realtime()
        .publish("node", None::<String>, parsed.report_sequence);
    state
        .public_realtime()
        .publish("node", None::<String>, parsed.report_sequence);
    (StatusCode::OK, Json(ReportResponse { receipt })).into_response()
}

pub(crate) fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/reports", axum::routing::post(handler))
        .layer(axum::extract::DefaultBodyLimit::max(
            platpulse_core::protocol::MAX_REPORT_BODY_BYTES,
        ))
        .layer(axum::middleware::from_fn(
            |request: axum::extract::Request, next: axum::middleware::Next| async move {
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
    use crate::auth::AuthConfig;
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
                "unexpected report response: {}",
                String::from_utf8_lossy(&body)
            );
        }
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice::<ReportResponse>(&body)
            .unwrap()
            .receipt
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
}
