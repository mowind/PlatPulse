//! Peer presence history and the current Peer projection of a Node.
use std::collections::{BTreeMap, BTreeSet, HashSet};

use sqlx::{Any, FromRow, Transaction};

use crate::peer_history::PeerPresenceDelta;
use platpulse_core::observation::{PeerDirection, PeerSnapshot};

/// Record the Peer presence intervals of one successful Peer snapshot.
///
/// `observed_at` is the snapshot's observation instant on the Server clock, not
/// its receipt instant: an interval says when a Peer was really seen, so a
/// replayed Report must produce the intervals of its own window instead of
/// squeezing hours of churn into the catch-up window (ADR 0014, issue #274).
pub(super) async fn save_peer_presence(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    snapshot: &PeerSnapshot,
    observed_at: &str,
    had_previous_successful_snapshot: bool,
    stored: &StoredPeerState,
) -> Result<PeerPresenceDelta, sqlx::Error> {
    // The first successful snapshot is the baseline. Presence intervals are
    // derived only from a difference between two successful snapshots.
    if !had_previous_successful_snapshot {
        return Ok(PeerPresenceDelta::default());
    }
    // The Peer set the Server holds is the previous successful snapshot, and
    // the Report has already read it once for its diff write (issue #271), so
    // the boundary is derived from that read instead of a second query.
    let previous_peer_ids = &stored.peers;
    let open_peer_ids: HashSet<String> = sqlx::query_scalar(
        "SELECT peer_id FROM peer_presence_intervals WHERE node_id=$1 AND closed_at IS NULL",
    )
    .bind(node_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect();
    let incoming: HashSet<String> = snapshot
        .peers
        .iter()
        .map(|peer| crate::redaction::redact_peer_identity(&peer.peer_id))
        .collect();
    let delta = PeerPresenceDelta {
        arrivals: incoming
            .iter()
            .filter(|peer_id| !previous_peer_ids.contains_key(peer_id.as_str()))
            .count() as i64,
        departures: previous_peer_ids
            .keys()
            .filter(|peer_id| !incoming.contains(peer_id.as_str()))
            .count() as i64,
    };

    // An interval is a snapshot of the arrival boundary. Once opened, its
    // identity/operational fields do not change while the Peer remains
    // present; a later changed snapshot is still the same connection.
    for peer in &snapshot.peers {
        let safe_peer_id = crate::redaction::redact_peer_identity(&peer.peer_id);
        if previous_peer_ids.contains_key(safe_peer_id.as_str())
            || open_peer_ids.contains(&safe_peer_id)
        {
            continue;
        }
        let direction = match peer.direction {
            PeerDirection::Inbound => "inbound",
            PeerDirection::Outbound => "outbound",
        };
        let safe_client_name = peer
            .client_name
            .as_deref()
            .map(crate::redaction::redact_sensitive);
        sqlx::query("INSERT INTO peer_presence_intervals (node_id, peer_id, direction, trusted, static_peer, consensus_peer, client_name, opened_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
            .bind(node_id)
            .bind(&safe_peer_id)
            .bind(direction)
            .bind(peer.trusted as i64)
            .bind(peer.static_peer as i64)
            .bind(peer.consensus_peer as i64)
            .bind(safe_client_name.as_deref())
            .bind(observed_at)
            .execute(&mut **tx)
            .await?;
    }

    for peer_id in open_peer_ids {
        if !incoming.contains(peer_id.as_str()) {
            // Server timestamps are normally monotonic, but a wall-clock
            // adjustment must not violate the interval ordering constraint.
            sqlx::query("UPDATE peer_presence_intervals SET closed_at=CASE WHEN opened_at > $1 THEN opened_at ELSE $2 END WHERE node_id=$3 AND peer_id=$4 AND closed_at IS NULL")
                .bind(observed_at)
                .bind(observed_at)
                .bind(node_id)
                .bind(peer_id)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(delta)
}

/// Rows one batched statement carries when a Peer snapshot is written.
///
/// A snapshot holds a few hundred Peers and their capabilities, and Report
/// Ingestion writes it through the Server's single write connection (ADR 0013,
/// issue #213), so the snapshot costs one round trip per chunk instead of one
/// per row.
const PEER_INSERT_ROWS: usize = 64;
const PEER_CAPABILITY_INSERT_ROWS: usize = 200;

/// Rows one batched statement carries when a Node's RPC surface is written. A
/// Node reports a few hundred methods, and they reach the Server in one
/// statement per chunk rather than one per method (issue #213).
pub(super) const RPC_INSERT_ROWS: usize = 200;

/// One Peer of a successful snapshot, already redacted and canonicalised.
///
/// The Server stores a Peer and a snapshot reports a Peer in the same shape, so
/// the difference between two snapshots is a field comparison rather than a
/// second query: [`StoredPeerState`] holds the rows the Server already has, and
/// a row the snapshot no longer reports is exactly the row the write deletes.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
struct CurrentPeerRow {
    peer_id: String,
    remote_ip: Option<String>,
    direction: String,
    trusted: i64,
    static_peer: i64,
    consensus_peer: i64,
    client_name: Option<String>,
    cbft_protocol_version: Option<i64>,
    cbft_highest_qc_block: Option<i64>,
    cbft_locked_block: Option<i64>,
    cbft_commit_block: Option<i64>,
}

/// The Peer set one Node already has, with the capabilities recorded for it.
///
/// Report Ingestion reads it once per Report: the presence interval boundary
/// and the diff write both derive from the same rows (issue #271), and a
/// snapshot that repeats them writes nothing.
#[derive(Debug, Default)]
pub(super) struct StoredPeerState {
    peers: BTreeMap<String, CurrentPeerRow>,
    capabilities: BTreeMap<String, BTreeSet<String>>,
}

pub(super) async fn load_stored_peers(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
) -> Result<StoredPeerState, sqlx::Error> {
    let peers = sqlx::query_as::<_, CurrentPeerRow>(
        "SELECT peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, client_name, cbft_protocol_version, cbft_highest_qc_block, cbft_locked_block, cbft_commit_block FROM current_node_peers WHERE node_id=$1",
    )
    .bind(node_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut stored = StoredPeerState {
        peers: peers
            .into_iter()
            .map(|peer| (peer.peer_id.clone(), peer))
            .collect(),
        capabilities: BTreeMap::new(),
    };
    let capabilities = sqlx::query_as::<_, (String, String)>(
        "SELECT peer_id, capability FROM current_node_peer_capabilities WHERE node_id=$1",
    )
    .bind(node_id)
    .fetch_all(&mut **tx)
    .await?;
    for (peer_id, capability) in capabilities {
        stored
            .capabilities
            .entry(peer_id)
            .or_default()
            .insert(capability);
    }
    Ok(stored)
}

/// Write only the difference between the Peer set a snapshot reports and the
/// one the Server holds: arrivals are inserted, departures deleted, and a Peer
/// whose attributes changed is rewritten.
///
/// A Report that repeats a Peer set writes no row here at all (issue #271).
/// Peer freshness is the `peers` component's last value receipt
/// (`component_status.value_received_at`), so the row stamp records a change
/// instead of the receipt, and an unchanged snapshot leaves both Peer tables
/// exactly as they were.
pub(super) async fn save_current_peers(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    snapshot: &PeerSnapshot,
    received_at: &str,
    stored: &StoredPeerState,
) -> Result<(), sqlx::Error> {
    // The snapshot is reduced to the rows the Server can compare, keyed by Peer
    // ID so the diff walks both sides in one pass.
    let mut reported: BTreeMap<String, (CurrentPeerRow, BTreeSet<String>)> = BTreeMap::new();
    for peer in &snapshot.peers {
        let safe_peer_id = crate::redaction::redact_peer_identity(&peer.peer_id);
        let capabilities = peer
            .caps
            .iter()
            .map(|capability| crate::redaction::redact_sensitive(capability))
            .collect();
        reported.insert(
            safe_peer_id.clone(),
            (
                CurrentPeerRow {
                    peer_id: safe_peer_id,
                    remote_ip: peer
                        .remote_ip
                        .as_deref()
                        .and_then(crate::geo::GeoLoader::canonical_public_ip),
                    direction: match peer.direction {
                        PeerDirection::Inbound => "inbound".to_owned(),
                        PeerDirection::Outbound => "outbound".to_owned(),
                    },
                    trusted: peer.trusted as i64,
                    static_peer: peer.static_peer as i64,
                    consensus_peer: peer.consensus_peer as i64,
                    client_name: peer
                        .client_name
                        .as_deref()
                        .map(crate::redaction::redact_sensitive),
                    cbft_protocol_version: peer.cbft_protocol_version.map(|value| value as i64),
                    cbft_highest_qc_block: peer.cbft_highest_qc_block.map(|value| value as i64),
                    cbft_locked_block: peer.cbft_locked_block.map(|value| value as i64),
                    cbft_commit_block: peer.cbft_commit_block.map(|value| value as i64),
                },
                capabilities,
            ),
        );
    }

    // A successful snapshot is authoritative, including an empty list: every
    // Peer it no longer reports loses its row and the capabilities recorded
    // with it. The capabilities are deleted explicitly rather than left to the
    // foreign key cascade, so both tables stay clean on every Storage Backend.
    let departed: Vec<&str> = stored
        .peers
        .keys()
        .filter(|peer_id| !reported.contains_key(*peer_id))
        .map(String::as_str)
        .collect();
    for chunk in departed.chunks(PEER_INSERT_ROWS) {
        let sql = crate::database::number_placeholders(&format!(
            "DELETE FROM current_node_peer_capabilities WHERE node_id=? AND peer_id IN ({})",
            crate::database::placeholder_list(chunk.len())
        ));
        let mut statement = sqlx::query(&sql).bind(node_id);
        for peer_id in chunk {
            statement = statement.bind(*peer_id);
        }
        statement.execute(&mut **tx).await?;
        let sql = crate::database::number_placeholders(&format!(
            "DELETE FROM current_node_peers WHERE node_id=? AND peer_id IN ({})",
            crate::database::placeholder_list(chunk.len())
        ));
        let mut statement = sqlx::query(&sql).bind(node_id);
        for peer_id in chunk {
            statement = statement.bind(*peer_id);
        }
        statement.execute(&mut **tx).await?;
    }

    // Arrivals and changed attributes share one statement shape, so the round
    // trips stay proportional to what changed instead of to the Peer count: a
    // Peer the Server does not hold is inserted, and a Peer whose content
    // differs is rewritten with the snapshot's values. An unchanged Peer is in
    // neither list, which is what makes a repeated snapshot write nothing.
    let mut rewritten: Vec<&CurrentPeerRow> = Vec::new();
    let mut arrived_capabilities: Vec<(&str, &str)> = Vec::new();
    let mut departed_capabilities: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (peer_id, (row, capabilities)) in &reported {
        if stored.peers.get(peer_id) != Some(row) {
            rewritten.push(row);
        }
        let recorded = stored.capabilities.get(peer_id);
        for capability in capabilities {
            if !recorded.is_some_and(|recorded| recorded.contains(capability)) {
                arrived_capabilities.push((peer_id.as_str(), capability.as_str()));
            }
        }
        if let Some(recorded) = recorded {
            for capability in recorded {
                if !capabilities.contains(capability) {
                    departed_capabilities
                        .entry(peer_id.as_str())
                        .or_default()
                        .push(capability.as_str());
                }
            }
        }
    }

    for chunk in rewritten.chunks(PEER_INSERT_ROWS) {
        let sql = crate::database::number_placeholders(&format!(
            "INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, client_name, cbft_protocol_version, cbft_highest_qc_block, cbft_locked_block, cbft_commit_block, updated_at) VALUES {} ON CONFLICT(node_id, peer_id) DO UPDATE SET remote_ip=excluded.remote_ip, direction=excluded.direction, trusted=excluded.trusted, static_peer=excluded.static_peer, consensus_peer=excluded.consensus_peer, client_name=excluded.client_name, cbft_protocol_version=excluded.cbft_protocol_version, cbft_highest_qc_block=excluded.cbft_highest_qc_block, cbft_locked_block=excluded.cbft_locked_block, cbft_commit_block=excluded.cbft_commit_block, updated_at=excluded.updated_at",
            crate::database::row_placeholders(13, chunk.len())
        ));
        let mut statement = sqlx::query(&sql);
        for peer in chunk {
            statement = statement
                .bind(node_id)
                .bind(&peer.peer_id)
                .bind(peer.remote_ip.as_deref())
                .bind(peer.direction.as_str())
                .bind(peer.trusted)
                .bind(peer.static_peer)
                .bind(peer.consensus_peer)
                .bind(peer.client_name.as_deref())
                .bind(peer.cbft_protocol_version)
                .bind(peer.cbft_highest_qc_block)
                .bind(peer.cbft_locked_block)
                .bind(peer.cbft_commit_block)
                .bind(received_at);
        }
        statement.execute(&mut **tx).await?;
    }

    // A Peer whose capabilities changed is diffed per capability. A capability
    // rarely disappears from a connected Peer, so a removal is one bounded
    // statement for that Peer while every addition shares the batched insert.
    for (peer_id, capabilities) in &departed_capabilities {
        for chunk in capabilities.chunks(PEER_CAPABILITY_INSERT_ROWS) {
            let sql = crate::database::number_placeholders(&format!(
                "DELETE FROM current_node_peer_capabilities WHERE node_id=? AND peer_id=? AND capability IN ({})",
                crate::database::placeholder_list(chunk.len())
            ));
            let mut statement = sqlx::query(&sql).bind(node_id).bind(*peer_id);
            for capability in chunk {
                statement = statement.bind(*capability);
            }
            statement.execute(&mut **tx).await?;
        }
    }

    for chunk in arrived_capabilities.chunks(PEER_CAPABILITY_INSERT_ROWS) {
        let sql = crate::database::number_placeholders(&format!(
            "INSERT INTO current_node_peer_capabilities (node_id, peer_id, capability, updated_at) VALUES {}",
            crate::database::row_placeholders(4, chunk.len())
        ));
        let mut statement = sqlx::query(&sql);
        for (peer_id, capability) in chunk {
            statement = statement
                .bind(node_id)
                .bind(*peer_id)
                .bind(*capability)
                .bind(received_at);
        }
        statement.execute(&mut **tx).await?;
    }

    // Geo cache hygiene is not part of this write: `crate::geo::cleanup_cache`
    // refreshes Peer references and drops unreferenced addresses on its own
    // maintenance cycle (issue #271).
    Ok(())
}

#[cfg(test)]
mod tests {

    use crate::http::report_ingestion::test_helpers::*;

    use axum::http::StatusCode;

    use crate::auth::AuthConfig;

    use crate::http::AppState;

    use crate::secrets::load_pepper_file;

    use platpulse_core::ReceiptDisposition;

    #[tokio::test]
    pub(super) async fn peer_snapshot_projects_per_node_and_preserves_last_good_on_error() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:00Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 1,
            "value_revision": 1,
            "latest": {
                "peers": [{
                    "peer_id": "peer-a",
                    "remote_ip": "203.0.113.4",
                    "direction": "inbound",
                    "trusted": true,
                    "static_peer": false,
                    "consensus_peer": true,
                    "client_name": "PlatON/v1.5.1",
                    "caps": ["cbft/1"],
                    "cbft_protocol_version": 1,
                    "cbft_highest_qc_block": 100,
                    "cbft_locked_block": 99,
                    "cbft_commit_block": 98
                }]
            }
        });
        let first_body = serde_json::to_vec(&value).unwrap();
        let first = submit(&state, &agent_id, first_body.clone()).await;
        assert_eq!(first.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            state
                .admin_realtime()
                .pending_events()
                .iter()
                .map(|event| event.resource.as_str())
                .collect::<Vec<_>>(),
            vec!["node", "peer", "geo"]
        );
        assert_eq!(
            state
                .public_realtime()
                .pending_events()
                .iter()
                .map(|event| event.resource.as_str())
                .collect::<Vec<_>>(),
            vec!["node", "network"]
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND closed_at IS NULL",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "the first successful Peer Snapshot establishes the baseline only"
        );
        let first_value_received_at: String = sqlx::query_scalar(
            "SELECT value_received_at FROM component_status WHERE node_id=$1 AND component_key='peers'",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert!(!first_value_received_at.is_empty());
        sqlx::query("UPDATE component_status SET value_received_at=$1 WHERE node_id=$2 AND component_key='peers'")
            .bind("2026-01-01T00:00:00Z")
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .execute(state.db().pool())
            .await
            .unwrap();
        let current: (String, String, i64, String, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT peer_id, direction, trusted, client_name, remote_ip, cbft_commit_block FROM current_node_peers WHERE node_id=$1",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(current.0, "peer-a");
        assert_eq!(current.1, "inbound");
        assert_eq!(current.2, 1);
        assert_eq!(current.3, "PlatON/v1.5.1");
        assert_eq!(current.4, None);
        assert_eq!(current.5, Some(98));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM current_node_peer_capabilities WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );

        let replay = submit(&state, &agent_id, first_body).await;
        assert_eq!(replay, first);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "a replayed immutable report must not create a baseline presence interval"
        );

        let mut duplicate_value = value.clone();
        duplicate_value["report_id"] = serde_json::json!("0195f2a1-0098-4098-8098-000000000098");
        duplicate_value["report_sequence"] = serde_json::json!(2);
        let peer = value["nodes"][0]["chain"]["peers"]["latest"]["peers"][0].clone();
        duplicate_value["nodes"][0]["chain"]["peers"]["latest"]["peers"] =
            serde_json::json!([peer.clone(), peer]);
        let duplicate_status = submit_status(
            &state,
            &agent_id,
            serde_json::to_vec(&duplicate_value).unwrap(),
        )
        .await;
        assert_eq!(duplicate_status, StatusCode::BAD_REQUEST);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM current_node_peers WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "a duplicate Peer ID must not alter current state"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0099-4099-8099-000000000099");
        value["report_sequence"] = serde_json::json!(2);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "error",
            "attempted_at": "2026-08-12T10:00:05Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 2,
            "value_revision": 1,
            "latest": {
                "peers": [{
                    "peer_id": "peer-a",
                    "remote_ip": "203.0.113.4",
                    "direction": "inbound",
                    "trusted": true,
                    "static_peer": false,
                    "consensus_peer": true,
                    "client_name": "PlatON/v1.5.1",
                    "caps": ["cbft/1"],
                    "cbft_protocol_version": 1,
                    "cbft_highest_qc_block": 100,
                    "cbft_locked_block": 99,
                    "cbft_commit_block": 98
                }]
            },
            "error": {"code": "admin_peers_failed", "message": "peer probe failed"}
        });
        let error_receipt = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(error_receipt.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND closed_at IS NULL",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "a failed Peer collection cannot close or invent a baseline interval"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM current_node_peers WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "a Peer collection error must not erase last-good current peers"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT state FROM component_status WHERE node_id=$1 AND component_key='peers'",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            "error"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT value_received_at FROM component_status WHERE node_id=$1 AND component_key='peers'",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            "2026-01-01T00:00:00Z",
            "an error receipt must not replace the last successful value receipt"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0104-4104-8104-000000000104");
        value["report_sequence"] = serde_json::json!(3);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:07Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 3,
            "value_revision": 2,
            "latest": {
                "peers": [{
                    "peer_id": "peer-a",
                    "remote_ip": "203.0.113.4",
                    "direction": "outbound",
                    "trusted": false,
                    "static_peer": true,
                    "consensus_peer": false,
                    "client_name": "PlatON/v1.5.2",
                    "caps": ["cbft/2"],
                    "cbft_protocol_version": 2,
                    "cbft_highest_qc_block": 101,
                    "cbft_locked_block": 100,
                    "cbft_commit_block": 99
                }]
            }
        });
        let unchanged_receipt =
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(unchanged_receipt.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "a successful snapshot that retains the Peer does not invent an arrival"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0100-4100-8100-000000000100");
        value["report_sequence"] = serde_json::json!(4);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "unsupported",
            "state_revision": 3,
            "value_revision": 1
        });
        let unsupported_receipt =
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(
            unsupported_receipt.disposition,
            ReceiptDisposition::Accepted
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM current_node_peers WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "Unsupported must preserve last-good current peers"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0101-4101-8101-000000000101");
        value["report_sequence"] = serde_json::json!(5);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:10Z",
            "latest_observed_at": "2026-08-12T10:00:10Z",
            "state_revision": 4,
            "value_revision": 2,
            "latest": {"peers": []}
        });
        let empty_receipt = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(empty_receipt.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a'",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "an empty snapshot removes the baseline Peer without inventing a departure interval"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM current_node_peers WHERE node_id=$1",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "a successful empty snapshot must clear current peers"
        );
        let empty_value_received_at: String = sqlx::query_scalar(
            "SELECT value_received_at FROM component_status WHERE node_id=$1 AND component_key='peers'",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_ne!(empty_value_received_at, "2026-01-01T00:00:00Z");
        sqlx::query("UPDATE component_status SET value_received_at=$1 WHERE node_id=$2 AND component_key='peers'")
            .bind("2026-01-02T00:00:00Z")
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .execute(state.db().pool())
            .await
            .unwrap();

        value["report_id"] = serde_json::json!("0195f2a1-0102-4102-8102-000000000102");
        value["report_sequence"] = serde_json::json!(6);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "error",
            "attempted_at": "2026-08-12T10:00:15Z",
            "latest_observed_at": "2026-08-12T10:00:10Z",
            "state_revision": 5,
            "value_revision": 2,
            "latest": {"peers": []},
            "error": {"code": "admin_peers_failed", "message": "peer probe failed after empty snapshot"}
        });
        let empty_error_receipt =
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(
            empty_error_receipt.disposition,
            ReceiptDisposition::Accepted
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT value_received_at FROM component_status WHERE node_id=$1 AND component_key='peers'",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            "2026-01-02T00:00:00Z",
            "an error after an empty snapshot must preserve its value receipt"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0103-4103-8103-000000000103");
        value["report_sequence"] = serde_json::json!(7);
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:20Z",
            "latest_observed_at": "2026-08-12T10:00:20Z",
            "state_revision": 6,
            "value_revision": 3,
            "latest": {
                "peers": [{
                    "peer_id": "peer-a",
                    "remote_ip": "203.0.113.5",
                    "direction": "outbound",
                    "trusted": false,
                    "static_peer": true,
                    "consensus_peer": false,
                    "client_name": "PlatON/v1.5.2",
                    "caps": ["cbft/2"],
                    "cbft_protocol_version": 2,
                    "cbft_highest_qc_block": 200,
                    "cbft_locked_block": 199,
                    "cbft_commit_block": 198
                }]
            }
        });
        let reappearance_receipt =
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(
            reappearance_receipt.disposition,
            ReceiptDisposition::Accepted
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a'",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "a Peer that reappears after the baseline was cleared opens a new interval"
        );
        let open_intervals: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' AND closed_at IS NULL",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(open_intervals, 1);
        let first_interval_client: String = sqlx::query_scalar(
            "SELECT client_name FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' ORDER BY interval_id LIMIT 1",
        )
        .bind("0195f2a1-0014-4014-8014-000000000014")
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            first_interval_client, "PlatON/v1.5.2",
            "the arrival interval records the reappearance metadata"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0105-4105-8105-000000000105");
        value["report_sequence"] = serde_json::json!(8);
        value["inventory"]["revision"] = serde_json::json!(2);
        value["inventory"]["nodes"] = serde_json::json!([]);
        value["nodes"] = serde_json::json!([]);
        let retired = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(retired.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT lifecycle FROM nodes WHERE node_id=$1",)
                .bind("0195f2a1-0014-4014-8014-000000000014")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            "retired"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' AND closed_at IS NULL",
            )
            .bind("0195f2a1-0014-4014-8014-000000000014")
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "retiring a Node without a snapshot does not fabricate a departure"
        );
    }

    /// Every stamp a Peer write can move: one entry per current Peer and one
    /// per recorded capability, so a Report that rewrites nothing is visible as
    /// an unchanged stamp instead of a guess about timing.
    pub(super) async fn peer_stamps(state: &AppState, node_id: &str) -> Vec<(String, String)> {
        let mut stamps: Vec<(String, String)> = sqlx::query_as(
            "SELECT peer_id, updated_at FROM current_node_peers WHERE node_id=$1 ORDER BY peer_id",
        )
        .bind(node_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        stamps.extend(
            sqlx::query_as::<_, (String, String)>(
                "SELECT peer_id || ':' || capability, updated_at FROM current_node_peer_capabilities WHERE node_id=$1 ORDER BY peer_id, capability",
            )
            .bind(node_id)
            .fetch_all(state.db().pool())
            .await
            .unwrap(),
        );
        stamps
    }

    /// Replace every Peer stamp with a sentinel, so the next Report can only
    /// leave it in place by writing no row at all.
    pub(super) async fn stamp_peer_rows(state: &AppState, node_id: &str, stamp: &str) {
        for statement in [
            "UPDATE current_node_peers SET updated_at=$1 WHERE node_id=$2",
            "UPDATE current_node_peer_capabilities SET updated_at=$1 WHERE node_id=$2",
        ] {
            sqlx::query(statement)
                .bind(stamp)
                .bind(node_id)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
    }

    pub(super) fn stamp_of(stamps: &[(String, String)], key: &str) -> String {
        stamps
            .iter()
            .find(|(recorded, _)| recorded == key)
            .unwrap_or_else(|| panic!("{key} is not recorded in {stamps:?}"))
            .1
            .clone()
    }

    /// Acceptance of issue #271: a Report whose Peer set has not changed writes
    /// no row to the Peer tables or the Geo cache, and the Peer Snapshot still
    /// proves it is fresh through its own component receipt rather than a row
    /// stamp.
    #[tokio::test]
    pub(super) async fn repeated_peer_snapshot_writes_no_peer_row() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let node_id = "0195f2a1-0014-4014-8014-000000000014";
        let sentinel = "2026-01-01T00:00:00Z";
        let peers = serde_json::json!([
            {
                "peer_id": "peer-a",
                "remote_ip": "8.8.8.8",
                "direction": "inbound",
                "trusted": true,
                "static_peer": false,
                "consensus_peer": true,
                "client_name": "PlatON/v1.5.1",
                "caps": ["cbft/1"],
                "cbft_protocol_version": 1,
                "cbft_highest_qc_block": 100,
                "cbft_locked_block": 99,
                "cbft_commit_block": 98
            },
            {
                "peer_id": "peer-b",
                "remote_ip": "192.0.2.10",
                "direction": "outbound",
                "trusted": false,
                "static_peer": true,
                "consensus_peer": false,
                "caps": ["cbft/2"]
            }
        ]);
        let mut value = peer_snapshot_body(peers);
        value["report_id"] = serde_json::json!("0195f2a1-0180-4180-8180-000000000180");
        value["report_sequence"] = serde_json::json!(1);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        // A country result as the maintenance cycle leaves it for the public
        // address this snapshot references.
        let cache_now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('local_mmdb', '8.8.8.8', 'US', 'current', $1, $2, $3, $4, $5)")
            .bind(&cache_now)
            .bind(&cache_now)
            .bind(&cache_now)
            .bind(&cache_now)
            .bind("2026-01-02T00:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        stamp_peer_rows(&state, node_id, sentinel).await;
        sqlx::query("UPDATE geo_location_cache SET last_referenced_at=$1")
            .bind(sentinel)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("UPDATE component_status SET value_received_at=$1 WHERE node_id=$2 AND component_key='peers'")
            .bind(sentinel)
            .bind(node_id)
            .execute(state.db().pool())
            .await
            .unwrap();

        // A row-level write counter, so "writes no row" is counted instead of
        // inferred from an unchanged stamp: both backends fire these triggers
        // for every row a statement touches even when the new values equal the
        // old ones. The counter is test-only DDL and never reaches a production
        // schema.
        sqlx::query("CREATE TABLE peer_write_probe (probe TEXT NOT NULL)")
            .execute(state.db().pool())
            .await
            .unwrap();
        for (table, name) in [
            ("current_node_peers", "peers"),
            ("current_node_peer_capabilities", "capabilities"),
            ("geo_location_cache", "geo"),
        ] {
            if cfg!(storage_postgres) {
                sqlx::query(
                    "CREATE OR REPLACE FUNCTION platpulse_probe_write() RETURNS trigger \
                     LANGUAGE plpgsql AS $$ BEGIN \
                     INSERT INTO peer_write_probe (probe) VALUES (TG_TABLE_NAME::text || ' ' || TG_OP); \
                     RETURN NULL; END $$",
                )
                .execute(state.db().pool())
                .await
                .unwrap();
                sqlx::query(&format!(
                    "CREATE TRIGGER probe_{name} AFTER INSERT OR UPDATE OR DELETE ON {table} \
                     FOR EACH ROW EXECUTE FUNCTION platpulse_probe_write()"
                ))
                .execute(state.db().pool())
                .await
                .unwrap();
            } else {
                for event in ["INSERT", "UPDATE", "DELETE"] {
                    sqlx::query(&format!(
                        "CREATE TRIGGER probe_{name}_{event} AFTER {event} ON {table} \
                         BEGIN INSERT INTO peer_write_probe (probe) VALUES ('{name} {event}'); END"
                    ))
                    .execute(state.db().pool())
                    .await
                    .unwrap();
                }
            }
        }
        // Self-check: a no-op UPDATE of the same rows must be counted, so the
        // zero below cannot pass because the triggers never fire.
        sqlx::query("UPDATE current_node_peers SET trusted = trusted")
            .execute(state.db().pool())
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM peer_write_probe")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            2,
            "the write probe counts a same-value Peer UPDATE"
        );
        sqlx::query("DELETE FROM peer_write_probe")
            .execute(state.db().pool())
            .await
            .unwrap();

        value["report_id"] = serde_json::json!("0195f2a1-0181-4181-8181-000000000181");
        value["report_sequence"] = serde_json::json!(2);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );

        let stamps = peer_stamps(&state, node_id).await;
        assert_eq!(
            stamps,
            vec![
                ("peer-a".to_owned(), sentinel.to_owned()),
                ("peer-b".to_owned(), sentinel.to_owned()),
                ("peer-a:cbft/1".to_owned(), sentinel.to_owned()),
                ("peer-b:cbft/2".to_owned(), sentinel.to_owned()),
            ],
            "a repeated Peer set must not rewrite a Peer or a capability row"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM peer_write_probe")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            0,
            "a repeated Peer set must not write a Peer, capability or Geo cache row"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT last_referenced_at FROM geo_location_cache WHERE canonical_ip='8.8.8.8'",
            )
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            sentinel,
            "the receipt no longer refreshes Peer references; the maintenance cycle owns them"
        );
        assert_ne!(
            sqlx::query_scalar::<_, String>(
                "SELECT value_received_at FROM component_status WHERE node_id=$1 AND component_key='peers'",
            )
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            sentinel,
            "the Peer Snapshot still stamps its own freshness on every successful snapshot"
        );
    }

    /// The diff write of issue #271: an arrival is inserted, a departure is
    /// deleted with its capabilities, and a Peer whose attributes changed is
    /// the only row rewritten. A Peer the snapshot repeats untouched keeps its
    /// stamp, so the Server never rewrites the whole set to absorb one change.
    #[tokio::test]
    pub(super) async fn peer_snapshot_writes_only_the_difference() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let node_id = "0195f2a1-0014-4014-8014-000000000014";
        let sentinel = "2026-01-01T00:00:00Z";
        let peer_a = serde_json::json!({
            "peer_id": "peer-a",
            "remote_ip": "8.8.8.8",
            "direction": "inbound",
            "trusted": true,
            "static_peer": false,
            "consensus_peer": true,
            "client_name": "PlatON/v1.5.1",
            "caps": ["cbft/1"],
            "cbft_protocol_version": 1,
            "cbft_highest_qc_block": 100,
            "cbft_locked_block": 99,
            "cbft_commit_block": 98
        });
        let peer_b = serde_json::json!({
            "peer_id": "peer-b",
            "direction": "outbound",
            "trusted": false,
            "static_peer": true,
            "consensus_peer": false,
            "caps": ["cbft/2"]
        });
        let peer_c = serde_json::json!({
            "peer_id": "peer-c",
            "direction": "inbound",
            "trusted": false,
            "static_peer": false,
            "consensus_peer": false,
            "caps": ["cbft/3"]
        });
        let mut value = peer_snapshot_body(serde_json::json!([peer_a.clone(), peer_b]));
        value["report_id"] = serde_json::json!("0195f2a1-0190-4190-8190-000000000190");
        value["report_sequence"] = serde_json::json!(1);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        stamp_peer_rows(&state, node_id, sentinel).await;

        // peer-b departs, peer-c arrives, and peer-a is repeated unchanged.
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] =
            serde_json::json!([peer_a.clone(), peer_c]);
        value["report_id"] = serde_json::json!("0195f2a1-0191-4191-8191-000000000191");
        value["report_sequence"] = serde_json::json!(2);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        let stamps = peer_stamps(&state, node_id).await;
        assert_eq!(
            stamps
                .iter()
                .map(|(recorded, _)| recorded.as_str())
                .collect::<Vec<_>>(),
            vec!["peer-a", "peer-c", "peer-a:cbft/1", "peer-c:cbft/3"],
            "the departure is deleted and the arrival inserted"
        );
        assert_eq!(
            stamp_of(&stamps, "peer-a"),
            sentinel,
            "an unchanged Peer keeps its stamp instead of being rewritten"
        );
        assert_ne!(stamp_of(&stamps, "peer-c"), sentinel);
        assert_ne!(stamp_of(&stamps, "peer-c:cbft/3"), sentinel);

        // peer-a now reports a newer highest QC block and one extra capability.
        let mut changed = peer_a.clone();
        changed["cbft_highest_qc_block"] = serde_json::json!(101);
        changed["caps"] = serde_json::json!(["cbft/1", "cbft/9"]);
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] =
            serde_json::json!([changed, peer_c.clone()]);
        value["report_id"] = serde_json::json!("0195f2a1-0192-4192-8192-000000000192");
        value["report_sequence"] = serde_json::json!(3);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        let stamps = peer_stamps(&state, node_id).await;
        assert_ne!(
            stamp_of(&stamps, "peer-a"),
            sentinel,
            "a Peer whose attributes changed is rewritten"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT cbft_highest_qc_block FROM current_node_peers WHERE node_id=$1 AND peer_id='peer-a'",
            )
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            101
        );
        assert_eq!(
            stamp_of(&stamps, "peer-a:cbft/1"),
            sentinel,
            "a capability the snapshot repeats is not rewritten"
        );
        assert_ne!(stamp_of(&stamps, "peer-a:cbft/9"), sentinel);

        // The capability that is no longer reported disappears on its own.
        let mut narrowed = changed;
        narrowed["caps"] = serde_json::json!(["cbft/9"]);
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] =
            serde_json::json!([narrowed, peer_c]);
        value["report_id"] = serde_json::json!("0195f2a1-0193-4193-8193-000000000193");
        value["report_sequence"] = serde_json::json!(4);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        let capabilities: Vec<String> = sqlx::query_scalar(
            "SELECT capability FROM current_node_peer_capabilities WHERE node_id=$1 AND peer_id='peer-a' ORDER BY capability",
        )
        .bind(node_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(capabilities, vec!["cbft/9".to_owned()]);
    }

    #[tokio::test]
    pub(super) async fn peer_presence_is_isolated_per_node_and_survives_server_restart() {
        let (dir, state, agent_id) = state_with_agent().await;
        let node_a = "0195f2a1-0014-4014-8014-000000000014";
        let node_b = "0195f2a1-0015-4015-8015-000000000015";
        let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:00Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 1,
            "value_revision": 1,
            "latest": {
                "peers": [{
                    "peer_id": "peer-a",
                    "remote_ip": "203.0.113.10",
                    "direction": "inbound",
                    "trusted": true,
                    "static_peer": false,
                    "consensus_peer": true,
                    "client_name": "PlatON/v1.5.1",
                    "caps": ["cbft/1"],
                    "cbft_protocol_version": 1,
                    "cbft_highest_qc_block": 100,
                    "cbft_locked_block": 99,
                    "cbft_commit_block": 98
                }]
            }
        });
        let mut second_inventory = value["inventory"]["nodes"][0].clone();
        second_inventory["node_id"] = serde_json::json!(node_b);
        second_inventory["rpc_endpoint"] = serde_json::json!("ws://127.0.0.1:6791");
        let mut inventory_nodes = value["inventory"]["nodes"].as_array().unwrap().clone();
        inventory_nodes.push(second_inventory);
        value["inventory"]["nodes"] = serde_json::json!(inventory_nodes);

        let mut second_observation = value["nodes"][0].clone();
        second_observation["node_id"] = serde_json::json!(node_b);
        let mut peer_b = second_observation["chain"]["peers"]["latest"]["peers"][0].clone();
        peer_b["peer_id"] = serde_json::json!("peer-b");
        peer_b["remote_ip"] = serde_json::json!("203.0.113.11");
        second_observation["chain"]["peers"]["latest"]["peers"] = serde_json::json!([peer_b]);
        let mut observations = value["nodes"].as_array().unwrap().clone();
        observations.push(second_observation);
        value["nodes"] = serde_json::json!(observations);
        value["report_id"] = serde_json::json!("0195f2a1-0110-4110-8110-000000000110");
        value["report_sequence"] = serde_json::json!(1);
        let first = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(first.disposition, ReceiptDisposition::Accepted);
        for (node_id, peer_id) in [(node_a, "peer-a"), (node_b, "peer-b")] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id=$2 AND closed_at IS NULL",
                )
                .bind(node_id)
                .bind(peer_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
                0,
                "each Node establishes its own successful baseline"
            );
        }

        value["report_id"] = serde_json::json!("0195f2a1-0111-4111-8111-000000000111");
        value["report_sequence"] = serde_json::json!(2);
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] = serde_json::json!([]);
        let second = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(second.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' AND closed_at IS NULL",
            )
            .bind(node_a)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "an empty snapshot closes only that Node's interval"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-b' AND closed_at IS NULL",
            )
            .bind(node_b)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            0,
            "a different Node's unchanged baseline does not create an interval"
        );

        state.db().close().await;
        let database = crate::test_support::test_database_in(dir.path()).await;
        let pepper_path = dir.path().join("pepper");
        let auth = AuthConfig::development(
            load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let restarted = AppState::new(database, None, auth);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-b' AND closed_at IS NULL",
            )
            .bind(node_b)
            .fetch_one(restarted.db().pool())
            .await
            .unwrap(),
            0,
            "a Server restart does not fabricate a Peer departure or arrival"
        );

        let mut peer_a = value["nodes"][1]["chain"]["peers"]["latest"]["peers"][0].clone();
        peer_a["peer_id"] = serde_json::json!("peer-a");
        peer_a["remote_ip"] = serde_json::json!("203.0.113.10");
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] = serde_json::json!([peer_a]);
        value["report_id"] = serde_json::json!("0195f2a1-0112-4112-8112-000000000112");
        value["report_sequence"] = serde_json::json!(3);
        let after_restart =
            submit(&restarted, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(after_restart.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' AND closed_at IS NULL",
            )
            .bind(node_a)
            .fetch_one(restarted.db().pool())
            .await
            .unwrap(),
            1,
            "the first post-restart difference opens a Node-scoped interval"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-b'",
            )
            .bind(node_b)
            .fetch_one(restarted.db().pool())
            .await
            .unwrap(),
            0,
            "Node B's unchanged baseline does not create Node A's interval"
        );

        value["report_id"] = serde_json::json!("0195f2a1-0113-4113-8113-000000000113");
        value["report_sequence"] = serde_json::json!(4);
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] = serde_json::json!([]);
        let departure = submit(&restarted, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(departure.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_presence_intervals WHERE node_id=$1 AND peer_id='peer-a' AND closed_at IS NOT NULL",
            )
            .bind(node_a)
            .fetch_one(restarted.db().pool())
            .await
            .unwrap(),
            1,
            "a later successful empty snapshot closes the open interval"
        );
    }

    #[tokio::test]
    pub(super) async fn successful_peer_snapshots_update_both_aggregate_families_once() {
        let (_dir, state, agent_id) = state_with_agent().await;
        // Country aggregation reads the retained results of the selected
        // provider only, exactly like the Public projection does.
        let state = state.with_geo_provider(crate::geo::GeoSelection {
            provider: crate::geo::GeoProvider::LocalMmdb,
            generation: 1,
        });
        let node_id = "0195f2a1-0014-4014-8014-000000000014";
        let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:00Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 1,
            "value_revision": 1,
            "latest": {
                "peers": [
                    {
                        "peer_id": "peer-a",
                        "remote_ip": "8.8.8.8",
                        "direction": "inbound",
                        "trusted": true,
                        "static_peer": false,
                        "consensus_peer": true,
                        "client_name": "PlatON/v1.5.1",
                        "caps": ["cbft/1"],
                        "cbft_protocol_version": 1,
                        "cbft_highest_qc_block": 100,
                        "cbft_locked_block": 99,
                        "cbft_commit_block": 98
                    },
                    {
                        "peer_id": "peer-b",
                        "remote_ip": "192.0.2.10",
                        "direction": "outbound",
                        "trusted": false,
                        "static_peer": true,
                        "consensus_peer": false,
                        "client_name": "PlatON/v1.5.2",
                        "caps": ["cbft/2"]
                    }
                ]
            }
        });
        value["report_id"] = serde_json::json!("0195f2a1-0200-4200-8200-000000000200");
        value["report_sequence"] = serde_json::json!(1);
        let body = serde_json::to_vec(&value).unwrap();
        // A cache row as the Server itself writes it: created now with its
        // absolute expiry capped by the hard retention boundary.
        let cache_now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('local_mmdb', '8.8.8.8', 'US', 'current', $1, $2, $3, $4, $5)")
            .bind(&cache_now)
            .bind(&cache_now)
            .bind(&cache_now)
            .bind(&cache_now)
            .bind(crate::geo::cache_expiry(&cache_now))
            .execute(state.db().pool())
            .await
            .unwrap();
        assert_eq!(
            submit(&state, &agent_id, body.clone()).await.disposition,
            ReceiptDisposition::Accepted
        );
        assert_eq!(
            submit_status(&state, &agent_id, body).await,
            StatusCode::OK,
            "a report replay must return the durable receipt without reapplying it"
        );

        for table in ["peer_aggregate_5m", "peer_aggregate_1h"] {
            let row: (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(&format!(
                "SELECT sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count FROM {table} WHERE node_id=$1"
            ))
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap();
            assert_eq!(row, (1, 2, 1, 1, 1, 1, 1));
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM peer_aggregate_5m_countries WHERE node_id=$1",
            )
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1,
            "known public addresses are reduced to country counts only"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT peer_count FROM peer_aggregate_5m_countries WHERE node_id=$1 AND country_code='US'",
            )
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );

        value["report_id"] = serde_json::json!("0195f2a1-0201-4201-8201-000000000201");
        value["report_sequence"] = serde_json::json!(2);
        let second = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(second.disposition, ReceiptDisposition::Accepted);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT sample_count FROM peer_aggregate_5m WHERE node_id=$1",
            )
            .bind(node_id)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            2
        );

        value["report_id"] = serde_json::json!("0195f2a1-0202-4202-8202-000000000202");
        value["report_sequence"] = serde_json::json!(3);
        value["nodes"][0]["chain"]["peers"]["latest"]["peers"] = serde_json::json!([]);
        let third = submit(&state, &agent_id, serde_json::to_vec(&value).unwrap()).await;
        assert_eq!(third.disposition, ReceiptDisposition::Accepted);
        let churn: (i64, i64, i64) = sqlx::query_as(
            "SELECT total_peers, arrivals, departures FROM peer_aggregate_5m WHERE node_id=$1",
        )
        .bind(node_id)
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(churn, (4, 0, 2));
        // Report Ingestion no longer writes the cache, so the row outlives the
        // last current Peer reference until the maintenance cycle collects it
        // (issue #271).
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM geo_location_cache")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            1,
            "a receipt must not write the Geo cache"
        );
        let now_text = crate::auth::format_rfc3339(crate::auth::now_utc());
        assert_eq!(
            crate::geo::cleanup_cache(
                state.db().pool(),
                &now_text,
                crate::geo::GeoProvider::LocalMmdb,
            )
            .await
            .unwrap(),
            1,
            "the maintenance cycle drops a row when its last current Peer reference disappears"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM geo_location_cache")
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
            0
        );
        let history = crate::peer_history::load_history(state.db().pool(), node_id)
            .await
            .unwrap();
        assert_eq!(history.state, "empty");
        assert_eq!(history.five_minute[0].total_peers, 4);
        sqlx::query("UPDATE component_status SET state='starting' WHERE node_id=$1 AND component_key='peers'")
            .bind(node_id)
            .execute(state.db().pool())
            .await
            .unwrap();
        assert_eq!(
            crate::peer_history::load_history(state.db().pool(), node_id)
                .await
                .unwrap()
                .state,
            "starting"
        );
    }

    #[tokio::test]
    pub(super) async fn aggregate_history_is_isolated_per_node() {
        let (_dir, state, agent_id) = state_with_agent().await;
        let node_a = "0195f2a1-0014-4014-8014-000000000014";
        let node_b = "0195f2a1-0015-4015-8015-000000000015";
        let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        let mut second_inventory = value["inventory"]["nodes"][0].clone();
        second_inventory["node_id"] = serde_json::json!(node_b);
        second_inventory["rpc_endpoint"] = serde_json::json!("ws://127.0.0.1:6791");
        value["inventory"]["nodes"] =
            serde_json::json!([value["inventory"]["nodes"][0].clone(), second_inventory]);
        let peer = serde_json::json!({
            "peer_id": "peer-a",
            "direction": "inbound",
            "trusted": true,
            "static_peer": false,
            "consensus_peer": true,
            "caps": []
        });
        value["nodes"][0]["chain"]["peers"] = serde_json::json!({
            "status": "ok",
            "attempted_at": "2026-08-12T10:00:00Z",
            "latest_observed_at": "2026-08-12T10:00:00Z",
            "state_revision": 1,
            "value_revision": 1,
            "latest": {"peers": [peer.clone()]}
        });
        let mut second_node = value["nodes"][0].clone();
        second_node["node_id"] = serde_json::json!(node_b);
        second_node["chain"]["peers"]["latest"]["peers"] = serde_json::json!([{
            "peer_id": "peer-b",
            "direction": "outbound",
            "trusted": false,
            "static_peer": true,
            "consensus_peer": false,
            "caps": []
        }]);
        value["nodes"] = serde_json::json!([value["nodes"][0].clone(), second_node]);
        value["report_id"] = serde_json::json!("0195f2a1-0210-4210-8210-000000000210");
        value["report_sequence"] = serde_json::json!(1);
        assert_eq!(
            submit(&state, &agent_id, serde_json::to_vec(&value).unwrap())
                .await
                .disposition,
            ReceiptDisposition::Accepted
        );
        for node_id in [node_a, node_b] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM peer_aggregate_5m WHERE node_id=$1",
                )
                .bind(node_id)
                .fetch_one(state.db().pool())
                .await
                .unwrap(),
                1
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT total_peers FROM peer_aggregate_5m WHERE node_id=$1",
            )
            .bind(node_a)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT total_peers FROM peer_aggregate_5m WHERE node_id=$1",
            )
            .bind(node_b)
            .fetch_one(state.db().pool())
            .await
            .unwrap(),
            1
        );
    }
}
