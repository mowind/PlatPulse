//! The Public Projection: what Home reads about the Active Nodes, their
//! Networks and their linked Validators, and each Active Node's Block
//! History, metrics and Peer history. Every entry takes the instant `at`
//! its response is judged at, so one response never mixes two clocks; a
//! database failure is [`Unavailable`] and a Node outside the projection
//! is `None`, never a degraded body. The HTTP DTOs it fills stay in
//! `http::public`.

mod block_history;
mod geo;
mod metrics;
mod peer_history;
mod validators;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use time::OffsetDateTime;

use crate::database::ServerDatabase;
use crate::geo::{GeoProvider, GeoStatus};
use crate::http::public::{
    PublicBlockHistoryItem, PublicConsensusInsight, PublicMetricWindow, PublicNetwork, PublicNode,
    PublicNodeDetail, PublicNodeMetricHistory, PublicPeerHistory, PublicPeerInsight,
};
use crate::node_currency;
use crate::validator;

pub(crate) use block_history::BlockHistoryRange;
#[cfg(test)]
pub(crate) use geo::PUBLIC_GEO_ERROR;
pub(crate) use geo::{GeoScope, PublicGeoReadings};
#[cfg(test)]
pub(crate) use metrics::PUBLIC_METRIC_SERIES;
pub(crate) use validators::public_validator_summary;

use geo::{unknown_public_geo_insight, unknown_public_node_peer_countries};
use validators::{
    associate_node_validators, listed_links, load_validator_rows, validator_insights,
};

/// The Public Projection could not read the database; the response is a 503.
#[derive(Debug)]
pub struct Unavailable;

impl From<sqlx::Error> for Unavailable {
    fn from(_: sqlx::Error) -> Self {
        Self
    }
}

/// The Node lifecycle the Public Projection lists: the one definition of an
/// Active Node every Public read filters on, written against the alias `n`
/// for `nodes`.
const ACTIVE_NODE: &str = "n.lifecycle = 'active'";

/// The Agent of `node_id` when it is an Active Node. `None` keeps a retired or
/// unknown Node out of every per-Node read before anything else is read, so
/// a guessed retired Node ID is indistinguishable from a missing one.
async fn active_node_agent(
    db: &ServerDatabase,
    node_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(&format!(
        "SELECT n.agent_id FROM nodes n WHERE n.node_id = $1 AND {ACTIVE_NODE}"
    ))
    .bind(node_id)
    .fetch_optional(db.pool())
    .await
}

/// Reads the Public Projection from one database with the Server's current
/// Geo provider, Geo database status and Validator freshness limit.
pub struct PublicProjection<'a> {
    db: &'a ServerDatabase,
    geo_provider: GeoProvider,
    geo_status: GeoStatus,
    validator_freshness_seconds: i64,
}

impl<'a> PublicProjection<'a> {
    pub fn new(
        db: &'a ServerDatabase,
        geo_provider: GeoProvider,
        geo_status: GeoStatus,
        validator_freshness_seconds: i64,
    ) -> Self {
        Self {
            db,
            geo_provider,
            geo_status,
            validator_freshness_seconds,
        }
    }

    /// Every Network with at least one Active Node, each with its Active
    /// Nodes, aggregate Peer and Geo insight and the Validators linked to its
    /// listed Nodes at `at`. One Link read and one Validator read serve every
    /// Network, so the query count does not grow with the number of Networks.
    pub async fn networks(&self, at: OffsetDateTime) -> Result<Vec<PublicNetwork>, Unavailable> {
        let rows = sqlx::query_as::<_, PublicNodeRow>(&public_node_query(
            ACTIVE_NODE,
            "r.network_key, n.node_id",
        ))
        .fetch_all(self.db.pool())
        .await?;
        let node_ids = rows
            .iter()
            .map(|row| row.node_id.as_str())
            .collect::<Vec<_>>();
        let evidence = load_currency_evidence(self.db, &node_ids).await?;
        // One database-status reading and one set of country rows per response
        // keep every Network's and every Node's Geo projection mutually
        // consistent: a Network Geo Insight is the sum of its Active Node buckets,
        // so the two can never drift apart. The batch is then restricted to
        // exactly the Nodes this response lists, each with its Network: the rows
        // above and the Geo rows below are separate reads, so a Node that changed
        // lifecycle between them would otherwise be counted in a Network reading
        // without being listed, and no browser could reproduce that reading from
        // the buckets it was handed. A listed Node the Geo batch lost keeps its
        // place in its Network's denominator without a reading of its own.
        let listed_nodes: BTreeMap<String, String> = rows
            .iter()
            .map(|row| (row.node_id.clone(), row.network_key.clone()))
            .collect();
        let mut geo_readings = PublicGeoReadings::load(
            self.db,
            self.geo_provider,
            self.geo_status.clone(),
            None,
            at,
        )
        .await;
        geo_readings.restrict_to_listed_nodes(&listed_nodes);
        let mut networks: Vec<PublicNetwork> = Vec::new();
        for row in rows {
            let network_key = row.network_key.clone();
            let node_evidence = evidence.get(&row.node_id).cloned().unwrap_or_default();
            let (network_display_name, node) = public_node(row, &node_evidence, at);
            if let Some(network) = networks
                .iter_mut()
                .find(|network| network.network_key == network_key)
            {
                network.nodes.push(node);
            } else {
                networks.push(PublicNetwork {
                    network_key,
                    display_name: network_display_name,
                    peers: unknown_public_peer_insight(),
                    geo: unknown_public_geo_insight(),
                    validators: Vec::new(),
                    nodes: vec![node],
                    validator_summary: public_validator_summary(&[], &[]),
                });
            }
        }
        let links = validator::effective_automatic_links(
            self.db.pool(),
            at,
            validator::LinkScope::AllNodes,
        )
        .await?
        .into_iter()
        .filter(|link| listed_nodes.contains_key(&link.node_id))
        .collect::<Vec<_>>();
        let validator_ids = links
            .iter()
            .map(|link| link.validator_id.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let validator_rows = load_validator_rows(self.db, &validator_ids).await?;
        for network in &mut networks {
            network.peers = aggregate_peer_insight(&network.nodes);
            network.geo = geo_readings.insight(GeoScope::Network(&network.network_key));
            for node in &mut network.nodes {
                node.peer_countries = geo_readings.node_peer_countries(&node.node_id);
            }
            let links = listed_links(&links, &network.nodes);
            network.validators = validator_insights(
                &validator_rows,
                &network.network_key,
                &links,
                at,
                self.validator_freshness_seconds,
            );
            associate_node_validators(&network.validators, &links, &mut network.nodes);
            network.validator_summary =
                public_validator_summary(&network.validators, &network.nodes);
        }
        Ok(networks)
    }

    /// One Active Node with the Validator linked to it at `at` and its Node
    /// Peer Country View; `None` when the Node is not in the projection.
    pub async fn node(
        &self,
        node_id: &str,
        at: OffsetDateTime,
    ) -> Result<Option<PublicNodeDetail>, Unavailable> {
        let filter = format!("n.node_id = $1 AND {ACTIVE_NODE}");
        let Some(row) =
            sqlx::query_as::<_, PublicNodeRow>(&public_node_query(&filter, "n.node_id"))
                .bind(node_id)
                .fetch_optional(self.db.pool())
                .await?
        else {
            return Ok(None);
        };
        let mut evidence = load_currency_evidence(self.db, &[row.node_id.as_str()]).await?;
        let node_evidence = evidence.remove(&row.node_id).unwrap_or_default();
        let (_, node) = public_node(row, &node_evidence, at);
        let mut nodes = [node];
        let links = validator::effective_automatic_links(
            self.db.pool(),
            at,
            validator::LinkScope::Node(&nodes[0].node_id),
        )
        .await?;
        let validator_ids = links
            .iter()
            .map(|link| link.validator_id.as_str())
            .collect::<Vec<_>>();
        let validator_rows = load_validator_rows(self.db, &validator_ids).await?;
        let validators = validator_insights(
            &validator_rows,
            &nodes[0].network_key,
            &links,
            at,
            self.validator_freshness_seconds,
        );
        associate_node_validators(&validators, &links, &mut nodes);
        let [mut node] = nodes;
        // The Node Peer Country View and this Node's compact buckets come
        // from one read of the same retained cache rows the Network
        // projection uses, so the Node reading is exactly what Home shows
        // with only this Node selected.
        let geo_readings = PublicGeoReadings::load(
            self.db,
            self.geo_provider,
            self.geo_status.clone(),
            Some(GeoScope::Node(&node.node_id)),
            at,
        )
        .await;
        node.peer_countries = geo_readings.node_peer_countries(&node.node_id);
        let geo = geo_readings.insight(GeoScope::Node(&node.node_id));
        Ok(Some(PublicNodeDetail { node, geo }))
    }

    /// One Active Node's Block History within `range`, with raw Block
    /// Summaries kept by the retention cutoff at `at`; `None` when the Node is
    /// not in the projection.
    pub async fn block_history(
        &self,
        node_id: &str,
        range: BlockHistoryRange,
        at: OffsetDateTime,
    ) -> Result<Option<Vec<PublicBlockHistoryItem>>, Unavailable> {
        if active_node_agent(self.db, node_id).await?.is_none() {
            return Ok(None);
        }
        Ok(Some(
            block_history::block_history(self.db, node_id, range, at).await?,
        ))
    }

    /// One Active Node's metrics for `window` ending at `at`, the Host series
    /// read from the Node's Agent; `None` when the Node is not in the projection.
    pub async fn metrics(
        &self,
        node_id: &str,
        window: PublicMetricWindow,
        at: OffsetDateTime,
    ) -> Result<Option<PublicNodeMetricHistory>, Unavailable> {
        let Some(agent_id) = active_node_agent(self.db, node_id).await? else {
            return Ok(None);
        };
        let history = if window == PublicMetricWindow::Live {
            metrics::live_metrics(self.db, node_id, &agent_id, at).await?
        } else {
            metrics::window_metrics(self.db, node_id, &agent_id, window, at).await?
        };
        Ok(Some(history))
    }

    /// One Active Node's Peer history with its freshness judged at `at`;
    /// `None` when the Node is not in the projection.
    pub async fn peer_history(
        &self,
        node_id: &str,
        at: OffsetDateTime,
    ) -> Result<Option<PublicPeerHistory>, Unavailable> {
        if active_node_agent(self.db, node_id).await?.is_none() {
            return Ok(None);
        }
        let history = crate::peer_history::load_history(self.db.pool(), node_id, at).await?;
        Ok(Some(peer_history::public_peer_history(history)))
    }
}

fn unknown_public_peer_insight() -> PublicPeerInsight {
    PublicPeerInsight {
        state: "unknown".to_owned(),
        freshness: "unknown".to_owned(),
        observed_at: None,
        received_at: None,
        stale_since: None,
        peer_count: None,
        inbound_count: None,
        outbound_count: None,
        trusted_count: None,
        static_count: None,
        consensus_count: None,
    }
}

#[derive(Debug, Default, sqlx::FromRow)]
struct PublicNodeRow {
    node_id: String,
    display_name: Option<String>,
    network_key: String,
    network_display_name: String,
    lifecycle: String,
    rpc_state: Option<String>,
    identity_state: Option<String>,
    identity_contradicts_registry: i64,
    process_cpu_percent: Option<f64>,
    process_memory_percent: Option<f64>,
    process_started_at: Option<String>,
    process_uptime_ms: Option<i64>,
    last_report_at: Option<String>,
    host_cpu_percent: Option<f64>,
    host_memory_percent: Option<f64>,
    host_storage_percent: Option<f64>,
    node_data_directory_size_bytes: Option<i64>,
    node_data_directory_capacity_bytes: Option<i64>,
    host_network_rx_bytes_per_sec: Option<i64>,
    host_network_tx_bytes_per_sec: Option<i64>,
    sync_state: Option<String>,
    consensus_state: Option<String>,
    process_state: Option<String>,
    peer_state: Option<String>,
    peer_observed_at: Option<String>,
    peer_value_received_at: Option<String>,
    peer_value_observation_at: Option<String>,
    peer_value_revision: Option<i64>,
    peer_count: Option<i64>,
    peer_inbound_count: Option<i64>,
    peer_outbound_count: Option<i64>,
    peer_trusted_count: Option<i64>,
    peer_static_count: Option<i64>,
    peer_consensus_count: Option<i64>,
    auto_validator_identity_state: Option<String>,
    consensus_observed_at: Option<String>,
    consensus_value_received_at: Option<String>,
    consensus_value_observation_at: Option<String>,
    consensus_value_revision: Option<i64>,
    consensus_epoch: Option<i64>,
    consensus_view_number: Option<i64>,
    consensus_validator: Option<i64>,
    consensus_highest_qc_block: Option<i64>,
    consensus_highest_lock_block: Option<i64>,
    consensus_highest_commit_block: Option<i64>,
    current_head: Option<i64>,
    latest_block_transaction_count: Option<i64>,
    historical_high_watermark: Option<i64>,
    resync_state: Option<String>,
    resync_last_progress_at: Option<String>,
    network_reference_head: Option<i64>,
    network_reference_confidence: Option<String>,
}

fn fresh(timestamp: Option<&str>, now: OffsetDateTime) -> bool {
    crate::observation_time::is_fresh(
        timestamp,
        now,
        crate::observation_time::FRESHNESS_LIMIT_SECONDS,
    )
}

fn public_peer_insight(row: &PublicNodeRow, at: OffsetDateTime) -> PublicPeerInsight {
    let has_value = row.peer_value_revision.unwrap_or_default() > 0;
    let freshness = if has_value {
        match row.peer_value_observation_at.as_deref() {
            Some(timestamp) if fresh(Some(timestamp), at) => "current",
            Some(_) => "stale",
            None => "unknown",
        }
    } else {
        "unknown"
    };
    let stale_since = if freshness == "stale" {
        crate::observation_time::stale_since(
            row.peer_value_observation_at.as_deref(),
            crate::observation_time::FRESHNESS_LIMIT_SECONDS,
        )
    } else {
        None
    };
    PublicPeerInsight {
        state: row
            .peer_state
            .clone()
            .unwrap_or_else(|| "unknown".to_owned()),
        freshness: freshness.to_owned(),
        observed_at: row.peer_observed_at.clone(),
        received_at: row.peer_value_received_at.clone(),
        stale_since,
        peer_count: has_value.then_some(row.peer_count).flatten(),
        inbound_count: has_value.then_some(row.peer_inbound_count).flatten(),
        outbound_count: has_value.then_some(row.peer_outbound_count).flatten(),
        trusted_count: has_value.then_some(row.peer_trusted_count).flatten(),
        static_count: has_value.then_some(row.peer_static_count).flatten(),
        consensus_count: has_value.then_some(row.peer_consensus_count).flatten(),
    }
}

fn public_consensus_insight(row: &PublicNodeRow, at: OffsetDateTime) -> PublicConsensusInsight {
    let has_value = row.consensus_value_revision.unwrap_or_default() > 0;
    let freshness = if has_value {
        match row.consensus_value_observation_at.as_deref() {
            Some(timestamp) if fresh(Some(timestamp), at) => "current",
            Some(_) => "stale",
            None => "unknown",
        }
    } else {
        "unknown"
    };
    let stale_since = if freshness == "stale" {
        crate::observation_time::stale_since(
            row.consensus_value_observation_at.as_deref(),
            crate::observation_time::FRESHNESS_LIMIT_SECONDS,
        )
    } else {
        None
    };
    PublicConsensusInsight {
        state: row
            .consensus_state
            .clone()
            .unwrap_or_else(|| "unknown".to_owned()),
        freshness: freshness.to_owned(),
        observed_at: row.consensus_observed_at.clone(),
        received_at: row.consensus_value_received_at.clone(),
        stale_since,
        epoch: has_value.then_some(row.consensus_epoch).flatten(),
        view_number: has_value.then_some(row.consensus_view_number).flatten(),
        validator: has_value
            .then_some(row.consensus_validator.map(|value| value != 0))
            .flatten(),
        highest_qc_block: has_value
            .then_some(row.consensus_highest_qc_block)
            .flatten(),
        highest_lock_block: has_value
            .then_some(row.consensus_highest_lock_block)
            .flatten(),
        highest_commit_block: has_value
            .then_some(row.consensus_highest_commit_block)
            .flatten(),
    }
}

fn aggregate_peer_insight(nodes: &[PublicNode]) -> PublicPeerInsight {
    let insights = nodes.iter().map(|node| &node.peers).collect::<Vec<_>>();
    let state = if insights.iter().any(|peer| peer.state == "error") {
        "error"
    } else if insights.iter().any(|peer| peer.state == "unsupported") {
        "unsupported"
    } else if insights.iter().any(|peer| peer.state == "disabled") {
        "disabled"
    } else if insights.iter().any(|peer| peer.state == "starting") {
        "starting"
    } else if insights.iter().all(|peer| peer.state == "ok")
        && insights.iter().all(|peer| peer.peer_count.is_some())
    {
        "ok"
    } else {
        "unknown"
    };
    let all_values = insights.iter().all(|peer| peer.peer_count.is_some());
    let freshness = if !all_values || insights.is_empty() {
        "unknown"
    } else if insights.iter().all(|peer| peer.freshness == "current") {
        "current"
    } else if insights.iter().any(|peer| peer.freshness == "stale") {
        "stale"
    } else {
        "unknown"
    };
    let sum = |value: fn(&PublicPeerInsight) -> Option<i64>| {
        all_values.then(|| insights.iter().filter_map(|peer| value(peer)).sum())
    };
    PublicPeerInsight {
        state: state.to_owned(),
        freshness: freshness.to_owned(),
        // A Network has multiple observation times; exposing one would imply
        // a precision the aggregate does not have.
        observed_at: None,
        received_at: None,
        stale_since: None,
        peer_count: sum(|peer| peer.peer_count),
        inbound_count: sum(|peer| peer.inbound_count),
        outbound_count: sum(|peer| peer.outbound_count),
        trusted_count: sum(|peer| peer.trusted_count),
        static_count: sum(|peer| peer.static_count),
        consensus_count: sum(|peer| peer.consensus_count),
    }
}

/// Project one Node row, judging its Health from the gating evidence
/// [`node_currency::load`] read for it at `now`.
fn public_node(
    row: PublicNodeRow,
    evidence: &node_currency::CurrencyEvidence,
    now: OffsetDateTime,
) -> (String, PublicNode) {
    let identity = node_currency::NodeIdentity::from_observation(
        row.identity_state.as_deref(),
        row.identity_contradicts_registry != 0,
    );
    let summary = node_currency::health_summary(
        &row.lifecycle,
        row.process_state.as_deref(),
        identity,
        evidence,
        now,
    );
    let currency = evidence.currency(now).state();
    let currency_as_of = evidence.as_of();
    let peers = public_peer_insight(&row, now);
    let consensus = public_consensus_insight(&row, now);
    let node = PublicNode {
        node_id: row.node_id.clone(),
        display_name: row.display_name,
        network_key: row.network_key,
        health: summary.health.as_str().to_owned(),
        health_reason: summary.reason.text().to_owned(),
        currency,
        currency_as_of,
        rpc_state: row.rpc_state.unwrap_or_else(|| "unknown".to_owned()),
        sync_state: row.sync_state.unwrap_or_else(|| "unknown".to_owned()),
        consensus_state: row.consensus_state.unwrap_or_else(|| "unknown".to_owned()),
        process_state: row.process_state.unwrap_or_else(|| "unknown".to_owned()),
        peers,
        consensus,
        process_cpu_percent: row.process_cpu_percent,
        process_memory_percent: row.process_memory_percent,
        process_started_at: row.process_started_at,
        process_uptime_ms: row.process_uptime_ms,
        last_report_at: row.last_report_at,
        host_cpu_percent: row.host_cpu_percent,
        host_memory_percent: row.host_memory_percent,
        host_storage_percent: row.host_storage_percent,
        node_data_directory_size_bytes: row.node_data_directory_size_bytes,
        node_data_directory_capacity_bytes: row.node_data_directory_capacity_bytes,
        host_network_rx_bytes_per_sec: row.host_network_rx_bytes_per_sec,
        host_network_tx_bytes_per_sec: row.host_network_tx_bytes_per_sec,
        current_head: row.current_head,
        latest_block_transaction_count: row.latest_block_transaction_count,
        historical_high_watermark: row.historical_high_watermark,
        resync_state: row.resync_state.unwrap_or_else(|| "normal".to_owned()),
        network_reference_head: row.network_reference_head,
        network_reference_confidence: row
            .network_reference_confidence
            .unwrap_or_else(|| "unknown".to_owned()),
        resync_progress: row.historical_high_watermark.zip(row.current_head).map(
            |(high, current)| match row.resync_last_progress_at.as_deref() {
                Some(at) => format!("{current}/{high} (last progress {at})"),
                None => format!("{current}/{high}"),
            },
        ),
        resync_last_progress_at: row.resync_last_progress_at,
        validator: None,
        validator_identity_reason: validator::automatic_identity_reason(
            row.auto_validator_identity_state.as_deref(),
        ),
        validator_identity_state: row.auto_validator_identity_state,
        // Filled in by the caller from the response's single Geo read, so
        // every projection of this Node shares one set of buckets.
        peer_countries: unknown_public_node_peer_countries(),
    };
    (row.network_display_name, node)
}

const PUBLIC_NODE_QUERY_BASE: &str = r#"SELECT n.node_id, n.display_name, n.network_key, r.display_name AS network_display_name,
       n.lifecycle, s.state AS rpc_state, sy.state AS sync_state, co.state AS consensus_state,
       p.state AS process_state, i.state AS identity_state,
       CASE WHEN i.state = 'ok' AND i.value_revision > 0
                 AND c.network_genesis_hash IS NOT NULL AND c.network_chain_id IS NOT NULL
                 AND c.network_p2p_network_id IS NOT NULL AND c.network_address_hrp IS NOT NULL
                 AND (c.network_genesis_hash != r.genesis_hash OR c.network_chain_id != r.chain_id
                      OR c.network_p2p_network_id != r.p2p_network_id OR c.network_address_hrp != r.address_hrp)
            THEN 1 ELSE 0 END AS identity_contradicts_registry,
       CASE WHEN p.state IN ('ok', 'error') THEN proc.cpu_percent ELSE NULL END AS process_cpu_percent,
       CASE WHEN p.state IN ('ok', 'error') AND h.memory_total_bytes > 0 THEN (CAST(proc.memory_bytes AS REAL) * 100.0 / h.memory_total_bytes) ELSE NULL END AS process_memory_percent,
       CASE WHEN p.state IN ('ok', 'error') THEN proc.started_at ELSE NULL END AS process_started_at,
       CASE WHEN p.state IN ('ok', 'error') THEN proc.uptime_ms ELSE NULL END AS process_uptime_ms,
       a.last_received_at AS last_report_at,
       h.cpu_percent AS host_cpu_percent,
       CASE WHEN h.memory_total_bytes > 0 THEN (CAST(h.memory_used_bytes AS REAL) * 100.0 / h.memory_total_bytes) ELSE NULL END AS host_memory_percent,
       hd.storage_percent AS host_storage_percent,
       CASE WHEN ds.state IN ('ok', 'error') THEN dd.size_bytes ELSE NULL END AS node_data_directory_size_bytes,
       CASE WHEN ds.state IN ('ok', 'error') AND dc.state IN ('ok', 'error') THEN dd.capacity_bytes ELSE NULL END AS node_data_directory_capacity_bytes,
       h.network_rx_bytes_per_sec AS host_network_rx_bytes_per_sec,
       h.network_tx_bytes_per_sec AS host_network_tx_bytes_per_sec,
       ps.state AS peer_state, ps.observed_at AS peer_observed_at,
       ps.value_received_at AS peer_value_received_at,
       COALESCE(ps.value_observation_at, ps.value_received_at) AS peer_value_observation_at,
       ps.value_revision AS peer_value_revision,
       vis.state AS auto_validator_identity_state,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.peer_count, 0) ELSE NULL END AS peer_count,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.inbound_count, 0) ELSE NULL END AS peer_inbound_count,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.outbound_count, 0) ELSE NULL END AS peer_outbound_count,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.trusted_count, 0) ELSE NULL END AS peer_trusted_count,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.static_count, 0) ELSE NULL END AS peer_static_count,
       CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN COALESCE(pc.consensus_count, 0) ELSE NULL END AS peer_consensus_count,
       co.observed_at AS consensus_observed_at,
       co.value_received_at AS consensus_value_received_at,
       COALESCE(co.value_observation_at, co.value_received_at) AS consensus_value_observation_at,
       co.value_revision AS consensus_value_revision,
       c.consensus_epoch,
       c.consensus_view_number,
       c.consensus_validator,
       c.consensus_highest_qc_block,
       c.consensus_highest_lock_block,
       c.consensus_highest_commit_block,
       c.current_block AS current_head,
       bs.transaction_count AS latest_block_transaction_count,
       hs.historical_high_watermark,
       hs.resync_state, hs.resync_last_progress_at,
       nr.block_number AS network_reference_head, nr.confidence AS network_reference_confidence
  FROM nodes n
  JOIN agents a ON a.agent_id = n.agent_id
  JOIN networks r ON r.network_key = n.network_key
  LEFT JOIN component_status s ON s.node_id = n.node_id AND s.component_key = 'rpc'
  LEFT JOIN component_status sy ON sy.node_id = n.node_id AND sy.component_key = 'sync'
  LEFT JOIN component_status co ON co.node_id = n.node_id AND co.component_key = 'consensus'
  LEFT JOIN component_status p ON p.node_id = n.node_id AND p.component_key = 'process'
  LEFT JOIN component_status ds ON ds.node_id = n.node_id AND ds.component_key = 'datadirectorysizebytes'
  LEFT JOIN component_status dc ON dc.node_id = n.node_id AND dc.component_key = 'datadirectorycapacitybytes'
  LEFT JOIN component_status i ON i.node_id = n.node_id AND i.component_key = 'network_identity'
  LEFT JOIN node_validator_identity_status vis ON vis.node_id = n.node_id
  LEFT JOIN component_status ps ON ps.node_id = n.node_id AND ps.component_key = 'peers'
  LEFT JOIN (
       SELECT node_id,
              COUNT(*) AS peer_count,
              CAST(SUM(CASE WHEN direction = 'inbound' THEN 1 ELSE 0 END) AS BIGINT) AS inbound_count,
              CAST(SUM(CASE WHEN direction = 'outbound' THEN 1 ELSE 0 END) AS BIGINT) AS outbound_count,
              CAST(SUM(CASE WHEN trusted = 1 THEN 1 ELSE 0 END) AS BIGINT) AS trusted_count,
              CAST(SUM(CASE WHEN static_peer = 1 THEN 1 ELSE 0 END) AS BIGINT) AS static_count,
              CAST(SUM(CASE WHEN consensus_peer = 1 THEN 1 ELSE 0 END) AS BIGINT) AS consensus_count
         FROM current_node_peers
        GROUP BY node_id
  ) pc ON pc.node_id = n.node_id
  LEFT JOIN current_host_observations h ON h.agent_id = n.agent_id
  LEFT JOIN current_node_process_observations proc ON proc.node_id = n.node_id
  LEFT JOIN (
       SELECT agent_id,
              MAX(CASE WHEN total_bytes > 0 THEN (CAST(used_bytes AS REAL) * 100.0 / total_bytes) ELSE NULL END) AS storage_percent
         FROM current_host_disk_mounts
        GROUP BY agent_id
  ) hd ON hd.agent_id = n.agent_id
  LEFT JOIN current_node_data_directory_observations dd ON dd.node_id = n.node_id
  LEFT JOIN current_node_chain_observations c ON c.node_id = n.node_id
  LEFT JOIN block_summaries bs ON bs.node_id = n.node_id
       AND bs.block_number = (SELECT MAX(latest.block_number) FROM block_summaries latest WHERE latest.node_id = n.node_id)
  LEFT JOIN block_history_state hs ON hs.node_id = n.node_id
  LEFT JOIN network_reference_heads nr ON nr.network_key = n.network_key"#;

/// Read the Node Currency evidence of the listed Nodes on one pooled
/// connection.
async fn load_currency_evidence(
    db: &ServerDatabase,
    node_ids: &[&str],
) -> Result<HashMap<String, node_currency::CurrencyEvidence>, sqlx::Error> {
    let mut conn = db.pool().acquire().await?;
    node_currency::load(&mut conn, node_ids).await
}

fn public_node_query(filter: &str, order: &str) -> String {
    format!("{PUBLIC_NODE_QUERY_BASE} WHERE {filter} ORDER BY {order}")
}
