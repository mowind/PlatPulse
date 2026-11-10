//! Node list, Node detail, Peer churn and Peer history, and Node investigation.
use super::*;

/// The Server's own diagnosis of the Block history it is refusing for one
/// Node right now (ADR 0011 decision 4, issue #235). The reason names the
/// identity fields that differ with both values, and distinguishes an Agent
/// stamp that went stale after a Node build upgrade from a genuine
/// cross-Network conflict, so "identity matched, incident resolved, history
/// fully rejected" stops being an unexplained state. The Server writes this
/// same text to the History Gap its rejected samples leave behind, which keeps
/// the Receipt, the gap, and this view naming one cause. The Server never
/// rewrites the Registry to match a stamped sample.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct BlockHistoryRejection {
    /// Server-authored diagnosis: the differing fields, whether the stamp is
    /// stale or the Node really moved Networks, and the remedy.
    pub reason: String,
    /// Lowest height the Server is currently refusing.
    pub from_height: i64,
    /// Highest height the Server is currently refusing.
    pub to_height: i64,
    /// How many heights the Server has refused since the Node's newest
    /// accepted Block Summary.
    pub rejected_heights: i64,
    /// When the Server recorded the most recent refusal.
    pub recorded_at: String,
}

/// Read the Block history the Server is refusing for one Node *currently*: a
/// refusal older than the Node's newest accepted Block Summary belongs to a
/// Node that has since recovered, and reporting it as a live alarm would
/// invent an incident. Every timestamp here is RFC 3339 UTC text written by
/// the Server, so the boundary comparison is a plain text comparison.
///
/// A refusal counts only when the Server authored it. An Agent declares its
/// own `server_rejected` gaps, and its reason and timestamp are Agent input:
/// presenting them here would let a reporting Agent fabricate this diagnosis
/// (AGENTS.md: the Server is the trust boundary). Recovery is proven by the
/// newer of the Node's newest retained Block Summary and the durable
/// acceptance stamp on its history state, so the diagnosis does not reappear
/// once retention ages the raw summaries out. A Node with neither keeps
/// showing its newest refusal: nothing proves that history resumed.
async fn block_history_rejection(
    state: &AppState,
    node_id: &str,
) -> Result<Option<BlockHistoryRejection>, sqlx::Error> {
    // One query keeps the newest refusal and its aggregate on the same
    // predicate, so the panel can never report a height span from evidence
    // other than the refusal it displays. The two recovery proofs are compared
    // as text and the newer one wins (an empty string stands for "none", which
    // sorts before every RFC 3339 timestamp): a Node whose raw summaries
    // retention has not aged out yet is judged by its newest accepted summary,
    // and one whose summaries are gone by the durable acceptance stamp.
    let (count, from_height, to_height, reason, recorded_at) = sqlx::query_as::<
        _,
        (i64, Option<i64>, Option<i64>, Option<String>, Option<String>),
    >(
        "WITH current_refusals AS (SELECT g.from_height, g.to_height, g.reason, g.created_at FROM block_history_gaps g WHERE g.node_id = $1 AND g.kind = 'server_rejected' AND g.authored_by = 'server' AND g.created_at > CASE WHEN COALESCE((SELECT MAX(s.accepted_at) FROM block_summaries s WHERE s.node_id = g.node_id), '') >= COALESCE((SELECT h.last_accepted_summary_at FROM block_history_state h WHERE h.node_id = g.node_id), '') THEN COALESCE((SELECT MAX(s.accepted_at) FROM block_summaries s WHERE s.node_id = g.node_id), '') ELSE COALESCE((SELECT h.last_accepted_summary_at FROM block_history_state h WHERE h.node_id = g.node_id), '') END) SELECT COUNT(*), MIN(from_height), MAX(to_height), (SELECT reason FROM current_refusals ORDER BY created_at DESC, from_height DESC LIMIT 1), (SELECT created_at FROM current_refusals ORDER BY created_at DESC, from_height DESC LIMIT 1) FROM current_refusals",
    )
    .bind(node_id)
    .fetch_one(state.db().pool())
    .await?;
    let (Some(from_height), Some(to_height), Some(reason), Some(recorded_at)) =
        (from_height, to_height, reason, recorded_at)
    else {
        return Ok(None);
    };
    Ok(Some(BlockHistoryRejection {
        reason,
        from_height,
        to_height,
        rejected_heights: count,
        recorded_at,
    }))
}

/// The holes in one Node's Block history the Server tracks itself (issue
/// #281): heights an Agent declared it could not recover within its bounds,
/// either still awaiting backfill or confirmed missing after no Agent filled
/// them for a whole raw Block History window. Both come from the Server's own
/// coverage state, never from an Agent's reason text, so the panel can show
/// what is missing even when the Server rejected nothing.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct BlockHistoryHoles {
    /// Heights still missing that a backfill may yet fill.
    pub awaiting_backfill_heights: i64,
    /// Heights confirmed missing; the Server no longer admits a backfill.
    pub confirmed_missing_heights: i64,
    /// The newest holes, highest first, at most [`BLOCK_HISTORY_HOLE_LIMIT`].
    pub intervals: Vec<BlockHistoryHole>,
}

/// One contiguous hole in a Node's Block history.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct BlockHistoryHole {
    pub from_height: i64,
    pub to_height: i64,
    pub state: BlockHistoryHoleState,
    /// When the Server first recorded the hole.
    pub since: String,
    /// When the hole last narrowed or changed state.
    pub updated_at: String,
}

/// Whether a hole can still be filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockHistoryHoleState {
    /// An open recoverable gap: a GapBackfill may still fill it.
    AwaitingBackfill,
    /// A permanent gap: no backfill arrived within the raw Block History
    /// window, and the Server no longer admits one.
    ConfirmedMissing,
}

/// How many holes the Node detail lists; the totals still count every hole.
const BLOCK_HISTORY_HOLE_LIMIT: i64 = 20;

/// Read the holes the Server tracks for one Node; None when it has none.
async fn block_history_holes(
    state: &AppState,
    node_id: &str,
) -> Result<Option<BlockHistoryHoles>, sqlx::Error> {
    let pool = state.db().pool();
    let (awaiting_backfill_heights, confirmed_missing_heights) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT CAST(COALESCE(SUM(CASE WHEN status = 'open_recoverable_gap' THEN last_height - first_height + 1 ELSE 0 END), 0) AS BIGINT), CAST(COALESCE(SUM(CASE WHEN status = 'permanent_gap' THEN last_height - first_height + 1 ELSE 0 END), 0) AS BIGINT) FROM block_coverage_intervals WHERE node_id = $1",
    )
    .bind(node_id)
    .fetch_one(pool)
    .await?;
    if awaiting_backfill_heights == 0 && confirmed_missing_heights == 0 {
        return Ok(None);
    }
    let rows = sqlx::query_as::<_, (i64, i64, String, String, String)>(
        "SELECT first_height, last_height, status, created_at, updated_at FROM block_coverage_intervals WHERE node_id = $1 AND status IN ('open_recoverable_gap', 'permanent_gap') ORDER BY last_height DESC LIMIT $2",
    )
    .bind(node_id)
    .bind(BLOCK_HISTORY_HOLE_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(Some(BlockHistoryHoles {
        awaiting_backfill_heights,
        confirmed_missing_heights,
        intervals: rows
            .into_iter()
            .map(
                |(from_height, to_height, status, since, updated_at)| BlockHistoryHole {
                    from_height,
                    to_height,
                    state: if status == "permanent_gap" {
                        BlockHistoryHoleState::ConfirmedMissing
                    } else {
                        BlockHistoryHoleState::AwaitingBackfill
                    },
                    since,
                    updated_at,
                },
            )
            .collect(),
    }))
}

/// Owner-only Node inventory row: Server-owned metadata (display name,
/// visibility, lifecycle guidance) stays distinct from Agent-observed
/// identity and endpoint configuration, and each Node is its own row so
/// block, transaction, consensus, peer, and error state never merge across
/// Nodes.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminNodeListItem {
    pub node_id: String,
    pub agent_id: String,
    pub display_name: Option<String>,
    pub network_key: String,
    pub network_display_name: String,
    pub lifecycle: String,
    pub lifecycle_guidance: String,
    pub visibility: String,
    pub inventory_revision: i64,
    pub first_seen_at: String,
    /// Last Server-side metadata change (display name / visibility).
    pub updated_at: String,
    /// Redacted Agent-declared endpoint (scheme + host only).
    pub rpc_endpoint: String,
    /// Server-owned Node Health Summary severity and primary reason.
    pub health: String,
    pub health_reason: String,
    /// The Node Currency judged from the RPC, sync and consensus observations.
    pub currency: crate::node_currency::NodeCurrencyState,
    /// The oldest of the RPC, sync and consensus Server-clock observation
    /// instants; null while any of them was never observed.
    pub currency_as_of: Option<String>,
    pub current_head: Option<i64>,
    pub resync_state: String,
    pub identity: NodeIdentityStatus,
}

#[derive(Debug, sqlx::FromRow)]
struct AdminNodeRow {
    node_id: String,
    agent_id: String,
    display_name: Option<String>,
    network_key: String,
    network_display_name: String,
    lifecycle: String,
    visibility: String,
    inventory_revision: i64,
    first_seen_at: String,
    updated_at: String,
    rpc_endpoint: String,
}

async fn admin_node_list_item(
    state: &AppState,
    row: AdminNodeRow,
) -> Result<AdminNodeListItem, sqlx::Error> {
    let assessed = node_diagnostic(
        state,
        row.node_id.clone(),
        row.network_key.clone(),
        row.display_name.clone(),
        row.lifecycle.clone(),
        row.inventory_revision,
        row.visibility.clone(),
    )
    .await?;
    let diagnostic = assessed.diagnostic;
    Ok(AdminNodeListItem {
        node_id: row.node_id.clone(),
        agent_id: row.agent_id,
        display_name: row.display_name,
        network_key: row.network_key.clone(),
        network_display_name: row.network_display_name,
        lifecycle: row.lifecycle.clone(),
        lifecycle_guidance: lifecycle_guidance(&row.lifecycle).to_owned(),
        visibility: row.visibility,
        inventory_revision: row.inventory_revision,
        first_seen_at: row.first_seen_at,
        updated_at: row.updated_at,
        rpc_endpoint: redact_endpoint(&row.rpc_endpoint),
        health: diagnostic.health,
        health_reason: diagnostic.health_reason,
        currency: diagnostic.currency,
        currency_as_of: diagnostic.currency_as_of,
        current_head: diagnostic.current_head,
        resync_state: diagnostic.resync_state,
        identity: assessed.identity,
    })
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes",
    tag = "admin",
    responses((status = 200, description = "Owner-only Node inventory with Server-owned metadata and per-Node identity disposition", body = [AdminNodeListItem]), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(super) async fn admin_nodes(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    let rows = sqlx::query_as::<_, AdminNodeRow>(
        "SELECT n.node_id, n.agent_id, n.display_name, n.network_key, r.display_name AS network_display_name, n.lifecycle, n.visibility, n.inventory_revision, n.first_seen_at, n.updated_at, n.rpc_endpoint FROM nodes n JOIN networks r ON r.network_key = n.network_key ORDER BY n.network_key, COALESCE(n.display_name, n.node_id)",
    )
    .fetch_all(state.db().pool())
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        match admin_node_list_item(&state, row).await {
            Ok(item) => items.push(item),
            Err(error) => return storage_failure(&state, &request_id.0, &error),
        }
    }
    Json(items).into_response()
}

/// Owner-only Node detail: the full per-Node view with Server-owned
/// metadata, lifecycle guidance, identity disposition, and the independent
/// process/RPC/sync/consensus observation dimensions (design §4.6, §5.1).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AdminNodeDetail {
    pub node_id: String,
    pub agent_id: String,
    pub display_name: Option<String>,
    pub network_key: String,
    pub network_display_name: String,
    pub lifecycle: String,
    pub lifecycle_guidance: String,
    pub visibility: String,
    pub inventory_revision: i64,
    pub first_seen_at: String,
    pub updated_at: String,
    pub rpc_endpoint: String,
    pub node_key_fingerprint: Option<String>,
    pub health: String,
    pub health_reason: String,
    /// The Node Currency judged from the RPC, sync and consensus observations.
    pub currency: crate::node_currency::NodeCurrencyState,
    /// The oldest of the RPC, sync and consensus Server-clock observation
    /// instants; null while any of them was never observed.
    pub currency_as_of: Option<String>,
    pub identity: NodeIdentityStatus,
    /// Block history the Server is refusing for this Node right now, with the
    /// Server's own reason; None when the Server stopped refusing, so the
    /// field never grows into a stale alarm after the Node recovers (ADR 0011
    /// decision 4, issue #235).
    pub block_history_rejection: Option<BlockHistoryRejection>,
    /// Block history holes the Server tracks for this Node, awaiting backfill
    /// or confirmed missing; None when its history has no hole (issue #281).
    pub block_history_holes: Option<BlockHistoryHoles>,
    pub process: Option<ProcessDiagnostic>,
    pub data_directory: Option<DataDirectoryDiagnostic>,
    pub rpc: Option<RpcDiagnostic>,
    pub sync: Option<SyncDiagnostic>,
    pub consensus: Option<ConsensusDiagnostic>,
    pub peers: Option<PeerDiagnostic>,
    pub current_head: Option<i64>,
    pub historical_high_watermark: Option<i64>,
    pub resync_state: String,
    pub resync_progress: Option<String>,
    pub network_reference_head: Option<i64>,
    pub network_reference_confidence: String,
    /// Most recent two-phase Transfer for this Node (any outcome), with the
    /// Server-owned effective status; `None` when no Transfer ever existed.
    pub transfer: Option<NodeTransfer>,
    /// Spool Backlog Age of the Agent that owns this Node (ADR 0014, issue
    /// #272). Spool state is observed once per Agent, so this reading belongs
    /// to the Agent and is shown here as the delivery backlog that carries this
    /// Node's Reports; `None` when the Agent never reported its Durable Spool.
    pub agent_spool_backlog_age: Option<SpoolBacklogAgeDiagnostic>,
    /// Automatic Validator identity coverage for this Node (#218): the
    /// discovery state, its sanitized reason, and the currently open automatic
    /// Link interval. `None` only when the Node row disappeared between the
    /// metadata read and this projection; `not_evaluated` means the discovery
    /// dimension has never examined the Node.
    pub validator_identity: Option<crate::http::validators_admin::AdminNodeValidatorIdentity>,
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    responses((status = 200, description = "Owner-only Node detail with Server-owned metadata and per-Node diagnostics", body = AdminNodeDetail), (status = 404, body = crate::http::ApiErrorBody), (status = 503, body = crate::http::ApiErrorBody))
)]
pub(super) async fn admin_node_detail(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    let row = sqlx::query_as::<_, AdminNodeRow>(
        "SELECT n.node_id, n.agent_id, n.display_name, n.network_key, r.display_name AS network_display_name, n.lifecycle, n.visibility, n.inventory_revision, n.first_seen_at, n.updated_at, n.rpc_endpoint FROM nodes n JOIN networks r ON r.network_key = n.network_key WHERE n.node_id = $1",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await;
    let Some(row) = (match row {
        Ok(row) => row,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    }) else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    let assessed = match node_diagnostic(
        &state,
        row.node_id.clone(),
        row.network_key.clone(),
        row.display_name.clone(),
        row.lifecycle.clone(),
        row.inventory_revision,
        row.visibility.clone(),
    )
    .await
    {
        Ok(assessed) => assessed,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let diagnostic = assessed.diagnostic;
    let fingerprint = match sqlx::query_scalar::<_, Option<String>>(
        "SELECT node_key_fingerprint FROM current_node_chain_observations WHERE node_id = $1",
    )
    .bind(&row.node_id)
    .fetch_optional(state.db().pool())
    .await
    {
        Ok(fingerprint) => fingerprint.flatten(),
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let agent_spool_backlog_age = match spool_backlog_age_diagnostic(&state, &row.agent_id).await {
        Ok(value) => value,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let identity = assessed.identity;
    let block_history_rejection = match block_history_rejection(&state, &row.node_id).await {
        Ok(value) => value,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let block_history_holes = match block_history_holes(&state, &row.node_id).await {
        Ok(value) => value,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let transfer = sqlx::query_as::<_, NodeTransferRow>(&format!(
        "SELECT {TRANSFER_COLUMNS} FROM node_transfers WHERE node_id=$1 ORDER BY created_at DESC LIMIT 1"
    ))
    .bind(&row.node_id)
    .fetch_optional(state.db().pool())
    .await;
    let transfer = match transfer {
        Ok(transfer) => transfer.map(node_transfer_dto),
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    };
    let validator_identity = match crate::validator::list_node_validator_identities(
        &state.database(),
        Some(&row.node_id),
    )
    .await
    {
        Ok(records) => records
            .into_iter()
            .next()
            .map(crate::http::validators_admin::AdminNodeValidatorIdentity::from),
        // A failed read is not an absence: report the failure instead of
        // letting the UI render a fabricated not_evaluated identity.
        Err(crate::validator::ValidatorError::Database(error)) => {
            return storage_failure(&state, &request_id.0, &error);
        }
        Err(error) => {
            return crate::http::validators_admin::error_response(&state, &request_id.0, error);
        }
    };
    Json(AdminNodeDetail {
        node_id: row.node_id,
        agent_id: row.agent_id,
        display_name: row.display_name,
        network_key: row.network_key,
        network_display_name: row.network_display_name,
        lifecycle: row.lifecycle.clone(),
        lifecycle_guidance: lifecycle_guidance(&row.lifecycle).to_owned(),
        visibility: row.visibility,
        inventory_revision: row.inventory_revision,
        first_seen_at: row.first_seen_at,
        updated_at: row.updated_at,
        rpc_endpoint: redact_endpoint(&row.rpc_endpoint),
        node_key_fingerprint: fingerprint,
        health: diagnostic.health.clone(),
        health_reason: diagnostic.health_reason.clone(),
        currency: diagnostic.currency,
        currency_as_of: diagnostic.currency_as_of.clone(),
        identity,
        block_history_rejection,
        block_history_holes,
        process: diagnostic.process,
        data_directory: diagnostic.data_directory,
        rpc: diagnostic.rpc,
        sync: diagnostic.sync,
        consensus: diagnostic.consensus,
        peers: diagnostic.peers,
        current_head: diagnostic.current_head,
        historical_high_watermark: diagnostic.historical_high_watermark,
        resync_state: diagnostic.resync_state,
        resync_progress: diagnostic.resync_progress,
        network_reference_head: diagnostic.network_reference_head,
        network_reference_confidence: diagnostic.network_reference_confidence,
        agent_spool_backlog_age,
        transfer,
        validator_identity,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/peer-churn",
    tag = "admin",
    params(("node_id" = String, Path, description = "Node ID")),
    responses((status = 200, description = "Owner-only bounded Peer arrival/departure history", body = PeerChurnDiagnostic), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(super) async fn admin_node_peer_churn(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
) -> Response {
    let exists = sqlx::query_scalar::<_, String>("SELECT node_id FROM nodes WHERE node_id=$1")
        .bind(&node_id)
        .fetch_optional(state.db().pool())
        .await;
    let Some(_) = (match exists {
        Ok(node_id) => node_id,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    }) else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    match peer_churn_diagnostic(&state, &node_id).await {
        Ok(diagnostic) => Json(diagnostic).into_response(),
        Err(error) => storage_failure(&state, &request_id.0, &error),
    }
}

/// One page of Peer receipt buckets may not exceed this, whatever the caller
/// asks for: a range read is a page of evidence, not an export.
const MAX_PEER_RANGE_LIMIT: i64 = 1000;

/// The widest Peer receipt range one answer may cover. The tiers themselves keep
/// less than this, and a wider range would only be answered as a shortfall.
const MAX_PEER_RANGE_SECONDS: i64 = 30 * 24 * 60 * 60;

/// Query for one bounded read of a Node's Peer receipt buckets (issue #220).
///
/// A range only means something at a grain: `grain` names the tier, and the tier
/// decides the grid a shortfall is measured against. With no `from`/`to` the
/// read answers the tier's own horizon ending now, which is what the fixed tail
/// covers, so a caller can page what the tail could only show the newest of.
#[derive(Debug, Default, Deserialize)]
pub(super) struct AdminPeerHistoryQuery {
    grain: Option<String>,
    from: Option<String>,
    to: Option<String>,
    before: Option<String>,
    limit: Option<i64>,
}

/// A Peer receipt range the Admin API refuses, as the error body it answers in.
struct PeerRangeRejection {
    code: &'static str,
    message: &'static str,
}

/// A range bound the route cannot answer: not canonical, reversed, or wider than
/// one answer may cover.
fn invalid_peer_range() -> PeerRangeRejection {
    PeerRangeRejection {
        code: "invalid_history_range",
        message: "history range is invalid",
    }
}

/// Read the Peer receipt range a query asks for, or refuse it.
///
/// The range is half-open — a bucket belongs to it when its start is at or after
/// `from` and before `to` — which is the same rule the stored buckets and the
/// tier's grid use, so the range answered and the range paged never disagree at
/// an edge. A half-stated range and a bound that is not the canonical
/// second-precision UTC shape are refused rather than completed: completing
/// either would answer a stretch the caller never asked for.
fn peer_range_request(
    query: &AdminPeerHistoryQuery,
    now: time::OffsetDateTime,
) -> Result<Option<crate::peer_history::PeerRangeRequest>, PeerRangeRejection> {
    let Some(grain_value) = query.grain.as_deref() else {
        if query.from.is_some()
            || query.to.is_some()
            || query.before.is_some()
            || query.limit.is_some()
        {
            return Err(PeerRangeRejection {
                code: "invalid_query",
                message: "a Peer range needs a grain: pass grain=5m or grain=1h",
            });
        }
        return Ok(None);
    };
    let Some(grain) = crate::peer_history::PeerGrain::parse(grain_value) else {
        return Err(PeerRangeRejection {
            code: "invalid_grain",
            message: "grain is not a Peer receipt grain (5m or 1h)",
        });
    };
    let (from, to) = match (query.from.as_deref(), query.to.as_deref()) {
        (None, None) => (
            now - time::Duration::seconds(grain.seconds() * grain.tail_limit()),
            now,
        ),
        (Some(from), Some(to)) => {
            let (Some(from), Some(to)) = (
                crate::metric_history::canonical_instant(from),
                crate::metric_history::canonical_instant(to),
            ) else {
                return Err(invalid_peer_range());
            };
            if from >= to || (to - from).whole_seconds() > MAX_PEER_RANGE_SECONDS {
                return Err(invalid_peer_range());
            }
            (from, to)
        }
        _ => {
            return Err(PeerRangeRejection {
                code: "invalid_history_range",
                message: "a Peer range needs both from and to",
            });
        }
    };
    let from = crate::auth::format_rfc3339(from);
    let to = crate::auth::format_rfc3339(to);
    // A paging cursor narrows the answer to the receipts older than a coordinate
    // the caller has already seen, so it must sit inside the requested range: a
    // cursor outside it would answer a stretch nobody asked about.
    let before = match query.before.as_deref() {
        None => None,
        Some(value) => {
            let Some(before) = crate::metric_history::canonical_instant(value) else {
                return Err(invalid_peer_range());
            };
            let before = crate::auth::format_rfc3339(before);
            if before <= from || before > to {
                return Err(invalid_peer_range());
            }
            Some(before)
        }
    };
    let limit = query
        .limit
        .unwrap_or_else(|| grain.tail_limit())
        .clamp(1, MAX_PEER_RANGE_LIMIT);
    Ok(Some(crate::peer_history::PeerRangeRequest {
        grain,
        from,
        to,
        before,
        limit,
    }))
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/peer-history",
    tag = "admin",
    params(
        ("node_id" = String, Path, description = "Node ID"),
        ("grain" = Option<String>, Query, description = "Peer receipt grain: 5m or 1h. Naming one answers a bounded range instead of the fixed newest tails"),
        ("from" = Option<String>, Query, description = "Canonical RFC 3339 UTC start of the range, second precision (default: the grain's horizon before to)"),
        ("to" = Option<String>, Query, description = "Canonical RFC 3339 UTC exclusive end of the range, second precision (default: now)"),
        ("before" = Option<String>, Query, description = "Canonical RFC 3339 UTC exclusive upper bound for paging: the continuation coordinate a truncated answer returned"),
        ("limit" = Option<i64>, Query, minimum = 1, maximum = 1000, description = "Maximum Peer receipt buckets in the page (default: the grain's tail limit)")
    ),
    responses((status = 200, description = "Owner-only aggregate Peer history: the fixed newest tails, or one bounded receipt range when a grain is named", body = AdminPeerHistory), (status = 400, body = crate::http::ApiErrorBody), (status = 404, body = crate::http::ApiErrorBody))
)]
pub(super) async fn admin_node_peer_history(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
    query: Result<
        axum::extract::Query<AdminPeerHistoryQuery>,
        axum::extract::rejection::QueryRejection,
    >,
) -> Response {
    let params = match query {
        Ok(axum::extract::Query(params)) => params,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_query",
                "query is not valid for peer history",
            );
        }
    };
    let range = match peer_range_request(&params, crate::auth::now_utc()) {
        Ok(range) => range,
        Err(rejection) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                rejection.code,
                rejection.message,
            );
        }
    };
    let exists = sqlx::query_scalar::<_, String>("SELECT node_id FROM nodes WHERE node_id=$1")
        .bind(&node_id)
        .fetch_optional(state.db().pool())
        .await;
    let Some(_) = (match exists {
        Ok(node_id) => node_id,
        Err(error) => return storage_failure(&state, &request_id.0, &error),
    }) else {
        return mutation_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
        );
    };
    // A ranged read answers the range it was asked for: the fixed tails stay
    // empty, so one response never means both "the newest buckets" and "the
    // range you asked about".
    if let Some(request) = range {
        return match crate::peer_history::load_range(state.db().pool(), &node_id, &request).await {
            Ok(answer) => Json(admin_peer_range_answer(answer)).into_response(),
            Err(error) => storage_failure(&state, &request_id.0, &error),
        };
    }
    match crate::peer_history::load_history(state.db().pool(), &node_id, crate::auth::now_utc())
        .await
    {
        Ok(history) => Json(admin_peer_history(history)).into_response(),
        Err(error) => storage_failure(&state, &request_id.0, &error),
    }
}

/// Query for the Owner-side investigation of one Node (issue #220).
///
/// Either `window` names one supported preset, or `from` and `to` name an
/// explicit range. A half-stated range is rejected rather than completed,
/// because which end a client meant cannot be guessed from one timestamp.
#[derive(Debug, Deserialize)]
pub(super) struct AdminInvestigationQuery {
    window: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

/// An error envelope whose message is built from what the Server found.
///
/// The shared mutation helper carries static text only; a window that cannot be
/// read has to say which part of it was wrong, and a purged Node has to say when
/// it was purged, so those two messages are built here and redacted like any
/// other.
fn dynamic_error(
    request_id: &str,
    status: StatusCode,
    code: &'static str,
    message: String,
) -> Response {
    (
        status,
        Json(crate::http::ApiErrorBody::with_fields_message(
            code,
            message,
            request_id,
            Vec::new(),
        )),
    )
        .into_response()
}

/// The one window the other evidence routes are read inside.
///
/// This answers coordinates, coverage and boundaries only: where a family's
/// evidence is for a window, how complete it is, and which boundary explains a
/// stretch it cannot answer. The evidence itself stays with that family's own
/// route, so no second copy of the history is served from here and the two can
/// never drift apart (design §11.4).
#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/investigation",
    tag = "admin",
    params(
        ("node_id" = String, Path, description = "Node ID"),
        ("window" = Option<String>, Query, description = "One supported preset: 1h, 6h, 24h, 7d or 30d (default: 24h)"),
        ("from" = Option<String>, Query, description = "Canonical RFC 3339 UTC start of an explicit range, second precision (paired with to)"),
        ("to" = Option<String>, Query, description = "Canonical RFC 3339 UTC exclusive end of an explicit range, second precision (paired with from)")
    ),
    responses(
        (status = 200, description = "Owner-only investigation window over one Node's own evidence", body = crate::investigation::InvestigationResponse),
        (status = 400, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(super) async fn admin_node_investigation(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
    query: Result<
        axum::extract::Query<AdminInvestigationQuery>,
        axum::extract::rejection::QueryRejection,
    >,
    Path(node_id): Path<String>,
) -> Response {
    // A window the typed extractor cannot read answers with the repo's error
    // envelope, so this route never answers in two error shapes.
    let Ok(axum::extract::Query(query)) = query else {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            crate::investigation::INVALID_WINDOW_CODE,
            "the investigation window could not be read; name a preset, or pass from and to together",
        );
    };
    let now = crate::auth::now_utc();
    let window = match crate::investigation::resolve_window(
        query.window.as_deref(),
        query.from.as_deref(),
        query.to.as_deref(),
        now,
    ) {
        Ok(window) => window,
        Err(reason) => {
            return dynamic_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                crate::investigation::INVALID_WINDOW_CODE,
                reason,
            );
        }
    };
    match crate::investigation::investigate_node(
        state.db().pool(),
        &node_id,
        &window,
        now,
        state.validator_timezone(),
    )
    .await
    {
        Ok(investigation) => no_store(Json(investigation).into_response()),
        Err(crate::investigation::InvestigationError::NotFound) => {
            not_found_response(&request_id.0)
        }
        // A purged Node is told apart from an unknown id: its evidence is gone
        // by design, so a window over it can never be answered again (design
        // §15.3), and saying so is not the same as saying the Server never knew
        // the Node.
        Err(crate::investigation::InvestigationError::Purged { deleted_at }) => dynamic_error(
            &request_id.0,
            StatusCode::NOT_FOUND,
            "node_purged",
            format!(
                "this Node was purged at {deleted_at}; its evidence was deleted by design and cannot be reconstructed"
            ),
        ),
        Err(crate::investigation::InvestigationError::Unavailable(error)) => {
            storage_failure(&state, &request_id.0, &error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::admin::test_helpers::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    /// Home and Admin read one Node Health Summary and Node Currency: whatever
    /// the evidence, the same Node carries the same Health, reason, currency
    /// and currency instant on both pages.
    #[tokio::test]
    async fn public_and_admin_agree_on_a_nodes_health_and_currency() {
        let (_dir, state) = node_inventory_state().await;
        sqlx::query("UPDATE nodes SET visibility = 'public'")
            .execute(state.db().pool())
            .await
            .unwrap();
        let fresh = crate::auth::format_rfc3339(crate::auth::now_utc());
        let old = crate::auth::format_rfc3339(crate::auth::now_utc() - time::Duration::hours(2));
        let health_on_both_pages = |node_id: &'static str| {
            let state = state.clone();
            async move {
                let admin = admin_node_detail(
                    State(state.clone()),
                    Path(node_id.to_owned()),
                    Extension(lifecycle_session()),
                    Extension(request_id()),
                )
                .await;
                assert_eq!(admin.status(), StatusCode::OK);
                let admin: Value =
                    serde_json::from_slice(&to_bytes(admin.into_body(), usize::MAX).await.unwrap())
                        .unwrap();
                let public = crate::http::public::public_node_detail(
                    State(state.clone()),
                    Path(node_id.to_owned()),
                    Extension(request_id()),
                )
                .await;
                assert_eq!(public.status(), StatusCode::OK);
                let public: Value = serde_json::from_slice(
                    &to_bytes(public.into_body(), usize::MAX).await.unwrap(),
                )
                .unwrap();
                let admin = (
                    admin["health"].clone(),
                    admin["health_reason"].clone(),
                    admin["currency"].clone(),
                    admin["currency_as_of"].clone(),
                );
                let public = (
                    public["health"].clone(),
                    public["healthReason"].clone(),
                    public["currency"].clone(),
                    public["currencyAsOf"].clone(),
                );
                assert_eq!(admin, public, "{node_id}: Admin and Public disagree");
                assert_eq!(
                    admin.3.is_null(),
                    admin.2 == "never_observed",
                    "{node_id}: a currency instant exists exactly when every gating observation does"
                );
                (
                    admin.0.as_str().unwrap().to_owned(),
                    admin.1.as_str().unwrap().to_owned(),
                    admin.2.as_str().unwrap().to_owned(),
                )
            }
        };
        let execute = |sql: &'static str, bind: Option<String>| {
            let state = state.clone();
            async move {
                let query = sqlx::query(sql);
                let query = match bind {
                    Some(value) => query.bind(value),
                    None => query,
                };
                query.execute(state.db().pool()).await.unwrap();
            }
        };

        assert_eq!(
            health_on_both_pages("node-healthy").await,
            (
                "healthy".to_owned(),
                "RPC, sync, and consensus are current".to_owned(),
                "current".to_owned()
            )
        );
        assert_eq!(
            health_on_both_pages("node-mismatched").await,
            (
                "unhealthy".to_owned(),
                "RPC observation failed".to_owned(),
                "current".to_owned()
            )
        );

        // A contradicted Registry identity leaves Health Unknown on both pages.
        execute(
            "UPDATE current_node_chain_observations SET network_chain_id = 999999 WHERE node_id = 'node-healthy'",
            None,
        )
        .await;
        assert_eq!(
            health_on_both_pages("node-healthy").await,
            (
                "unknown".to_owned(),
                "observed network identity mismatches the registered Network".to_owned(),
                "current".to_owned()
            )
        );
        execute(
            "UPDATE current_node_chain_observations SET network_chain_id = 210425 WHERE node_id = 'node-healthy'",
            None,
        )
        .await;

        // A failed process observation is a definite failure on both pages.
        execute(
            "INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, observation_at, state_revision, value_revision) VALUES ('agent-lifecycle-test', 'node', 'node-healthy', 'node-healthy', 'process', 'error', $1, $1, $1, $1, 1, 1)",
            Some(fresh),
        )
        .await;
        assert_eq!(
            health_on_both_pages("node-healthy").await,
            (
                "unhealthy".to_owned(),
                "process observation failed".to_owned(),
                "current".to_owned()
            )
        );
        execute(
            "DELETE FROM component_status WHERE node_id = 'node-healthy' AND component_key = 'process'",
            None,
        )
        .await;

        // A failure seen only in stale evidence is named, never a verdict.
        execute(
            "UPDATE component_status SET observation_at = $1 WHERE node_id = 'node-mismatched' AND component_key IN ('rpc', 'sync', 'consensus')",
            Some(old),
        )
        .await;
        assert_eq!(
            health_on_both_pages("node-mismatched").await,
            (
                "unknown".to_owned(),
                "observations are stale; last RPC observation failed".to_owned(),
                "stale".to_owned()
            )
        );

        // A gating component never observed leaves Health Unknown.
        execute(
            "DELETE FROM component_status WHERE node_id = 'node-healthy' AND component_key = 'consensus'",
            None,
        )
        .await;
        assert_eq!(
            health_on_both_pages("node-healthy").await,
            (
                "unknown".to_owned(),
                "Node is awaiting its first accepted consensus observation".to_owned(),
                "never_observed".to_owned()
            )
        );
    }

    /// A diagnostics read the database refuses fails the request instead of
    /// rendering the Node from the rows that happened to load.
    #[tokio::test]
    async fn a_refused_diagnostics_read_fails_the_admin_node_pages() {
        let (_dir, state) = node_inventory_state().await;
        sqlx::query("DROP TABLE current_node_process_observations")
            .execute(state.db().pool())
            .await
            .unwrap();
        let list = admin_nodes(
            State(state.clone()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(list.status(), StatusCode::SERVICE_UNAVAILABLE);
        let detail = admin_node_detail(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(detail.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn admin_node_inventory_is_per_node_with_server_owned_dimensions() {
        let (_dir, state) = node_inventory_state().await;
        let response = admin_nodes(
            State(state.clone()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let items = value.as_array().unwrap();
        assert_eq!(items.len(), 3, "every Node is its own inventory row");
        let by_id = |id: &str| {
            items
                .iter()
                .find(|item| item["node_id"] == id)
                .unwrap_or_else(|| panic!("missing Node {id}"))
                .clone()
        };
        // Server-owned metadata is present and distinct from the
        // Agent-observed identity disposition.
        let healthy = by_id("node-healthy");
        assert_eq!(healthy["display_name"], "node-healthy");
        assert_eq!(healthy["network_display_name"], "PlatON Mainnet");
        assert_eq!(healthy["lifecycle"], "active");
        assert_eq!(healthy["visibility"], "private");
        assert_eq!(healthy["health"], "healthy");
        assert_eq!(healthy["currency"], "current");
        assert_eq!(healthy["identity"]["state"], "matched");
        assert_eq!(
            healthy["identity"]["observed"]["chain_id"], 210425,
            "the observed identity tuple is reported without rewriting the Registry"
        );
        assert!(
            healthy["identity"]["mismatched_fields"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // Endpoints are redacted destination summaries.
        assert_eq!(healthy["rpc_endpoint"], "ws://[REDACTED_IP]:****");
        assert!(
            healthy["lifecycle_guidance"]
                .as_str()
                .unwrap()
                .contains("Agent-local")
        );
        // Mismatch is a typed, distinct diagnostic; last-good values remain.
        let mismatched = by_id("node-mismatched");
        assert_eq!(mismatched["identity"]["state"], "mismatched");
        assert_eq!(
            mismatched["identity"]["mismatched_fields"],
            serde_json::json!(["chain_id"])
        );
        assert_eq!(mismatched["health"], "unhealthy");
        assert_eq!(mismatched["health_reason"], "RPC observation failed");
        // Retired guidance is explicit and never implies remote control.
        let retired = by_id("node-retired");
        assert_eq!(retired["lifecycle"], "retired");
        assert!(
            retired["lifecycle_guidance"]
                .as_str()
                .unwrap()
                .contains("Reactivation requires declaring the same Node ID")
        );
        assert!(
            retired["lifecycle_guidance"]
                .as_str()
                .unwrap()
                .contains("never changes Node lifecycle")
        );
    }

    /// ADR 0011 decision 4 / issue #235: the Owner Node detail carries the
    /// Server's own diagnosis of the Block history it is refusing *right now*,
    /// which is exactly the #235 state the old surfaces could not explain
    /// (identity matched, incident resolved, every Block Summary rejected).
    /// The field disappears once the Node's history moves past the refusal, so
    /// it can never grow into a stale alarm.
    #[tokio::test]
    async fn admin_node_detail_reports_the_block_history_the_server_refuses() {
        let (_dir, state) = node_inventory_state().await;

        async fn detail(state: &AppState, node_id: &str) -> Value {
            let response = admin_node_detail(
                State(state.clone()),
                Path(node_id.to_owned()),
                Extension(lifecycle_session()),
                Extension(request_id()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap()
        }

        /// A refusal the Server recorded while ingesting a Report, which is
        /// the only kind of row the panel may present as its own diagnosis.
        async fn reject_gap(state: &AppState, reason: &str, height: i64, at: &str) {
            insert_gap(state, reason, height, at, "server").await;
        }

        /// A gap the reporting Agent declared about itself: the Server stores
        /// it with its author, and it must never become the Server's own
        /// diagnosis, however it is timestamped.
        async fn declare_gap(state: &AppState, reason: &str, height: i64, at: &str) {
            insert_gap(state, reason, height, at, "agent").await;
        }

        async fn insert_gap(state: &AppState, reason: &str, height: i64, at: &str, author: &str) {
            sqlx::query(
                "INSERT INTO block_history_gaps (node_id, from_height, to_height, kind, reason, created_at, authored_by) VALUES ('node-healthy', $1, $2, 'server_rejected', $3, $4, $5)",
            )
            .bind(height)
            .bind(height)
            .bind(reason)
            .bind(at)
            .bind(author)
            .execute(state.db().pool())
            .await
            .unwrap();
        }

        async fn accept_summary(state: &AppState, at: &str) {
            sqlx::query("INSERT INTO block_summaries (node_id, block_number, block_hash, parent_hash, network_genesis_hash, network_chain_id, network_p2p_network_id, network_address_hrp, block_timestamp_ms, observed_at, transaction_count, source, coinbase, seal_signer_match, protocol_proposer_kind, attribution_reason, accepted_at) VALUES ('node-healthy', 100, '0xhash', '0xparent', '0xgenesis', 210425, 1, 'lat', 1, $1, 2, 'subscription', '0x0000000000000000000000000000000000000000', 'unknown', 'unknown', 'test', $2)")
                .bind(at)
                .bind(at)
                .execute(state.db().pool())
                .await
                .unwrap();
        }

        let value = detail(&state, "node-healthy").await;
        assert!(
            value["block_history_rejection"].is_null(),
            "a Node whose history the Server accepts carries no refusal"
        );

        // An Agent declares its own server_rejected gaps, with its own reason
        // and its own timestamp. Presenting one here would let a reporting
        // Agent fabricate this diagnosis, so a future-dated Agent claim leaves
        // the panel empty (AGENTS.md: the Server is the trust boundary).
        declare_gap(
            &state,
            "Block network identity does not match the registered Network (Agent claim)",
            40,
            "2099-01-01T00:00:00Z",
        )
        .await;
        let value = detail(&state, "node-healthy").await;
        assert!(
            value["block_history_rejection"].is_null(),
            "an Agent-declared gap is Agent evidence, not a Server refusal"
        );

        // The Server records the refusal itself while ingesting a Report, so
        // the reason an operator reads is the Server's diagnosis rather than
        // an Agent claim about itself.
        let stale = "Block network identity does not match the registered Network (genesis_hash stamped 0xaa registered 0x01). Because the Agent's stamp is stale, restarting the Agent on that Host is the only remedy.";
        reject_gap(&state, stale, 41, "2026-08-12T10:00:00Z").await;
        let value = detail(&state, "node-healthy").await;
        assert_eq!(value["block_history_rejection"]["reason"], stale);
        assert_eq!(value["block_history_rejection"]["from_height"], 41);
        assert_eq!(value["block_history_rejection"]["to_height"], 41);
        assert_eq!(value["block_history_rejection"]["rejected_heights"], 1);
        assert_eq!(
            value["block_history_rejection"]["recorded_at"],
            "2026-08-12T10:00:00Z"
        );

        // A Block Summary accepted after the refusal proves the Node moved on.
        accept_summary(&state, "2026-08-12T11:00:00Z").await;
        let value = detail(&state, "node-healthy").await;
        assert!(
            value["block_history_rejection"].is_null(),
            "a recovered Node must not keep reporting the refusal it already recovered from"
        );

        // Retention ages accepted Block Summaries out. The recovery evidence
        // survives it on the history state row, so the refusal cannot come
        // back as a live alarm for a Node that already recovered (ADR 0011
        // decision 4).
        sqlx::query("DELETE FROM block_summaries WHERE node_id = 'node-healthy'")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO block_history_state (node_id, updated_at, last_accepted_summary_at) VALUES ('node-healthy', '2026-08-12T11:00:00Z', '2026-08-12T11:00:00Z') ON CONFLICT(node_id) DO UPDATE SET last_accepted_summary_at = excluded.last_accepted_summary_at")
            .execute(state.db().pool())
            .await
            .unwrap();
        let value = detail(&state, "node-healthy").await;
        assert!(
            value["block_history_rejection"].is_null(),
            "recovery evidence must outlive raw Block Summary retention"
        );

        // A fresh refusal is the Node's current disposition again, and it
        // stays visible even while the identity component reports matched:
        // this is the #235 contradiction the field exists to explain.
        let conflict = "Block network identity does not match the registered Network (chain_id stamped 100 registered 210425). Because the Node itself contradicts the Registry, this is a genuine cross-Network conflict that restarting the Agent cannot reconcile.";
        reject_gap(&state, conflict, 52, "2026-08-12T12:00:00Z").await;
        reject_gap(&state, conflict, 53, "2026-08-12T12:00:00Z").await;
        let value = detail(&state, "node-healthy").await;
        assert_eq!(value["identity"]["state"], "matched");
        assert_eq!(value["block_history_rejection"]["reason"], conflict);
        assert_eq!(value["block_history_rejection"]["from_height"], 52);
        assert_eq!(value["block_history_rejection"]["to_height"], 53);
        assert_eq!(
            value["block_history_rejection"]["rejected_heights"], 2,
            "only the heights refused since the last accepted Block Summary count"
        );
        assert_eq!(
            value["block_history_rejection"]["recorded_at"],
            "2026-08-12T12:00:00Z"
        );
    }

    /// Issue #281: the Node detail shows the holes the Server tracks, awaiting
    /// backfill or confirmed missing, even when nothing was Server-rejected.
    #[tokio::test]
    async fn admin_node_detail_lists_block_history_holes() {
        let (_dir, state) = node_inventory_state().await;

        async fn detail(state: &AppState, node_id: &str) -> Value {
            let response = admin_node_detail(
                State(state.clone()),
                Path(node_id.to_owned()),
                Extension(lifecycle_session()),
                Extension(request_id()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap()
        }
        let value = detail(&state, "node-healthy").await;
        assert!(
            value["block_history_holes"].is_null(),
            "a Node without holes carries no hole panel"
        );
        let interval = "INSERT INTO block_coverage_intervals (node_id, first_height, last_height, status, created_at, updated_at) VALUES ('node-healthy', $1, $2, $3, $4, $5)";
        for (first, last, status, since) in [
            (1_i64, 9_i64, "covered", "2026-08-12T09:00:00Z"),
            (10, 14, "permanent_gap", "2026-08-01T09:00:00Z"),
            (15, 29, "covered", "2026-08-12T09:00:00Z"),
            (30, 32, "open_recoverable_gap", "2026-08-12T10:00:00Z"),
        ] {
            sqlx::query(interval)
                .bind(first)
                .bind(last)
                .bind(status)
                .bind(since)
                .bind(since)
                .execute(state.db().pool())
                .await
                .unwrap();
        }
        let value = detail(&state, "node-healthy").await;
        let holes = &value["block_history_holes"];
        assert_eq!(holes["awaiting_backfill_heights"], 3);
        assert_eq!(holes["confirmed_missing_heights"], 5);
        assert_eq!(
            holes["intervals"],
            serde_json::json!([
                {"from_height": 30, "to_height": 32, "state": "awaiting_backfill", "since": "2026-08-12T10:00:00Z", "updated_at": "2026-08-12T10:00:00Z"},
                {"from_height": 10, "to_height": 14, "state": "confirmed_missing", "since": "2026-08-01T09:00:00Z", "updated_at": "2026-08-01T09:00:00Z"}
            ])
        );
    }

    #[tokio::test]
    async fn admin_node_detail_includes_identity_and_per_component_diagnostics() {
        let (_dir, state) = node_inventory_state().await;
        let response = admin_node_detail(
            State(state.clone()),
            Path("node-mismatched".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["node_id"], "node-mismatched");
        assert_eq!(value["identity"]["state"], "mismatched");
        assert_eq!(value["rpc"]["state"], "error");
        assert_eq!(value["rpc"]["error_message"], "RPC probe failed");
        assert_eq!(
            value["sync"]["state"], "ok",
            "last-good sync stays visible beside the RPC error"
        );

        assert_eq!(value["sync"]["current_block"], 99);
        assert_eq!(value["current_head"], 99);
        assert_eq!(value["consensus"]["validator"], false);
        let response = admin_node_detail(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["node_key_fingerprint"], "fp-healthy");
        assert_eq!(value["health"], "healthy");
        assert_eq!(value["peers"]["state"], "ok");
        assert_eq!(value["peers"]["peer_count"], 1);
        assert_eq!(value["peers"]["peers"][0]["peer_id"], "peer-healthy");
        assert_eq!(value["peers"]["peers"][0]["direction"], "inbound");
        assert_eq!(
            value["peers"]["peers"][0]["capabilities"],
            serde_json::json!(["cbft/1"])
        );
        assert!(
            !value.to_string().contains("203.0.113.7"),
            "Admin diagnostics must not expose the stored remote address"
        );
        let response = admin_node_peer_churn(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let churn: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(churn["state"], "ok");
        assert_eq!(churn["recent_departures"][0]["peer_id"], "peer-healthy");
        assert!(churn["recent_departures"][0]["duration_seconds"].is_number());
        assert!(!churn.to_string().contains("203.0.113.7"));
        sqlx::query("INSERT INTO peer_aggregate_5m (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)")
            .bind("node-healthy")
            .bind("2026-08-12T10:05:00Z")
            .bind(1_i64)
            .bind(1_i64)
            .bind(1_i64)
            .bind(0_i64)
            .bind(1_i64)
            .bind(0_i64)
            .bind(1_i64)
            .bind(1_i64)
            .bind(0_i64)
            .bind(1_i64)
            .bind(0_i64)
            .bind(0_i64)
            .bind(0_i64)
            .bind(Option::<i64>::None)
            .bind(Option::<i64>::None)
            .bind("2026-08-12T10:05:00Z")
            .bind("2026-08-12T10:09:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO peer_aggregate_5m_countries (node_id, bucket_start, country_code, peer_count) VALUES ($1, $2, $3, $4)")
            .bind("node-healthy")
            .bind("2026-08-12T10:05:00Z")
            .bind("US")
            .bind(1_i64)
            .execute(state.db().pool())
            .await
            .unwrap();
        let response = admin_node_peer_history(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
            Ok(axum::extract::Query(AdminPeerHistoryQuery::default())),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let peer_history: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(peer_history["state"], "ok");
        assert_eq!(
            peer_history["five_minute"][0]["countries"][0]["country_code"],
            "US"
        );
        assert!(peer_history["hourly"].is_array());
        assert!(!peer_history.to_string().contains("203.0.113.7"));
        assert!(!peer_history.to_string().contains("peer-healthy"));
        sqlx::query(
            "UPDATE component_status SET state='error', error_code='admin_peers_failed', error_message='peer probe failed' WHERE node_id='node-healthy' AND component_key='peers'",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        let response = admin_node_peer_churn(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let churn: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(churn["state"], "error");
        assert_eq!(churn["recent_departures"][0]["peer_id"], "peer-healthy");
        sqlx::query(
            "UPDATE component_status SET state='unsupported', error_code=NULL, error_message=NULL WHERE node_id='node-healthy' AND component_key='peers'",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        let response = admin_node_peer_churn(
            State(state.clone()),
            Path("node-healthy".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let churn: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(churn["state"], "unsupported");
        // Unknown Node is a non-leaking 404 for Admin too.
        let response = admin_node_detail(
            State(state.clone()),
            Path("node-missing".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// Pin Peer aggregate retention so the fixed receipt buckets below stay
    /// answerable however far the wall clock has moved past them. This is test
    /// fixture data; retention cleanup itself is covered by its own tests.
    async fn pin_peer_retention(state: &AppState) {
        sqlx::query(
            "UPDATE retention_policies SET retention_days = 0 WHERE family IN ('peer_aggregate_5m', 'peer_aggregate_1h')",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
    }

    /// Insert one 5-minute Peer receipt bucket holding a single trusted inbound
    /// Peer: the row shape one successful Report leaves behind.
    async fn insert_peer_bucket(state: &AppState, node_id: &str, bucket_start: &str) {
        sqlx::query("INSERT INTO peer_aggregate_5m (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)")
            .bind(node_id)
            .bind(bucket_start)
            .bind(1_i64)
            .bind(1_i64)
            .bind(1_i64)
            .bind(0_i64)
            .bind(1_i64)
            .bind(0_i64)
            .bind(1_i64)
            .bind(1_i64)
            .bind(0_i64)
            .bind(0_i64)
            .bind(0_i64)
            .bind(0_i64)
            .bind(0_i64)
            .bind(Option::<i64>::None)
            .bind(Option::<i64>::None)
            .bind(bucket_start)
            .bind(bucket_start)
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    /// Call the Peer history route with one query, the way the browser does.
    async fn peer_history_query(
        state: &AppState,
        node_id: &str,
        query: AdminPeerHistoryQuery,
    ) -> (StatusCode, Value) {
        let response = admin_node_peer_history(
            State(state.clone()),
            Path(node_id.to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
            Ok(axum::extract::Query(query)),
        )
        .await;
        let status = response.status();
        let body =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        (status, body)
    }

    #[tokio::test]
    async fn peer_history_answers_one_bounded_range_with_its_own_grid() {
        let (_dir, state) = node_inventory_state().await;
        pin_peer_retention(&state).await;
        let range_query = |limit: Option<i64>, before: Option<String>| AdminPeerHistoryQuery {
            grain: Some("5m".to_owned()),
            from: Some("2026-08-12T10:00:00Z".to_owned()),
            to: Some("2026-08-12T10:15:00Z".to_owned()),
            before,
            limit,
        };
        // Nothing is recorded for this Node yet: the range is unavailable rather
        // than an empty window, and a ranged read never fills the fixed tails.
        let (status, body) =
            peer_history_query(&state, "node-healthy", range_query(None, None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["range"]["coverage"], "unavailable");
        assert_eq!(body["range"]["matching"], 0);
        assert!(body["range"]["buckets"].as_array().unwrap().is_empty());
        assert!(body["range"]["first_bucket"].is_null());
        assert!(body["range"]["last_bucket"].is_null());
        assert!(body["range"]["continuation"].is_null());
        assert!(
            body["five_minute"].as_array().unwrap().is_empty(),
            "a ranged read answers the range, never the fixed newest tail"
        );
        assert!(body["hourly"].as_array().unwrap().is_empty());

        for bucket in [
            "2026-08-12T10:00:00Z",
            "2026-08-12T10:05:00Z",
            "2026-08-12T10:10:00Z",
        ] {
            insert_peer_bucket(&state, "node-healthy", bucket).await;
        }
        let (status, body) =
            peer_history_query(&state, "node-healthy", range_query(None, None)).await;
        assert_eq!(status, StatusCode::OK);
        let range = body["range"].clone();
        assert_eq!(range["grain"], "5m");
        assert_eq!(range["grain_seconds"], 300);
        assert_eq!(range["from"], "2026-08-12T10:00:00Z");
        assert_eq!(range["to"], "2026-08-12T10:15:00Z");
        assert_eq!(range["matching"], 3);
        assert_eq!(range["returned"], 3);
        assert_eq!(range["missing_buckets"], 0);
        assert_eq!(range["coverage"], "complete");
        assert_eq!(range["truncated"], false);
        assert_eq!(range["first_bucket"], "2026-08-12T10:00:00Z");
        assert_eq!(range["last_bucket"], "2026-08-12T10:10:00Z");
        assert_eq!(
            range["buckets"][0]["bucket_start"], "2026-08-12T10:10:00Z",
            "the page is newest first"
        );
        assert_eq!(range["buckets"][2]["bucket_start"], "2026-08-12T10:00:00Z");
        assert!(range["buckets"][0]["total_peers"].is_number());
        assert!(!body.to_string().contains("203.0.113.7"));
    }

    #[tokio::test]
    async fn peer_history_range_counts_missing_receipts_and_pages_at_a_bucket_boundary() {
        let (_dir, state) = node_inventory_state().await;
        pin_peer_retention(&state).await;
        for bucket in [
            "2026-08-12T10:00:00Z",
            "2026-08-12T10:05:00Z",
            "2026-08-12T10:10:00Z",
        ] {
            insert_peer_bucket(&state, "node-healthy", bucket).await;
        }
        let paged = AdminPeerHistoryQuery {
            grain: Some("5m".to_owned()),
            from: Some("2026-08-12T10:00:00Z".to_owned()),
            to: Some("2026-08-12T10:30:00Z".to_owned()),
            before: None,
            limit: Some(2),
        };
        let (status, body) = peer_history_query(&state, "node-healthy", paged).await;
        assert_eq!(status, StatusCode::OK);
        let range = body["range"].clone();
        assert_eq!(range["matching"], 3, "the range holds three receipts");
        assert_eq!(range["returned"], 2);
        assert_eq!(range["truncated"], true);
        assert_eq!(
            range["continuation"], "2026-08-12T10:05:00Z",
            "the oldest returned bucket is where the next page continues"
        );
        assert_eq!(
            range["missing_buckets"], 3,
            "the record owes 10:15, 10:20 and 10:25 inside the range"
        );
        assert_eq!(
            range["coverage"], "partial",
            "a shortfall inside the record is partial coverage, never complete"
        );
        // The continuation pages backwards without repeating or skipping a bucket.
        let (status, page) = peer_history_query(
            &state,
            "node-healthy",
            AdminPeerHistoryQuery {
                grain: Some("5m".to_owned()),
                from: Some("2026-08-12T10:00:00Z".to_owned()),
                to: Some("2026-08-12T10:30:00Z".to_owned()),
                before: Some("2026-08-12T10:05:00Z".to_owned()),
                limit: Some(2),
            },
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["range"]["buckets"].as_array().unwrap().len(), 1);
        assert_eq!(
            page["range"]["buckets"][0]["bucket_start"],
            "2026-08-12T10:00:00Z"
        );
        assert_eq!(page["range"]["truncated"], false);
        assert!(page["range"]["continuation"].is_null());
        assert_eq!(
            page["range"]["matching"], 3,
            "paging narrows the page, never the range it reports"
        );
    }

    #[tokio::test]
    async fn peer_history_refuses_a_half_stated_reversed_or_too_wide_range() {
        let (_dir, state) = node_inventory_state().await;
        let range = |from: Option<&str>, to: Option<&str>| AdminPeerHistoryQuery {
            grain: Some("5m".to_owned()),
            from: from.map(str::to_owned),
            to: to.map(str::to_owned),
            ..Default::default()
        };
        let refused = [
            (
                "invalid_query",
                AdminPeerHistoryQuery {
                    from: Some("2026-08-12T10:00:00Z".to_owned()),
                    ..Default::default()
                },
            ),
            (
                "invalid_query",
                AdminPeerHistoryQuery {
                    limit: Some(10),
                    ..Default::default()
                },
            ),
            (
                "invalid_grain",
                AdminPeerHistoryQuery {
                    grain: Some("15m".to_owned()),
                    ..Default::default()
                },
            ),
            (
                "invalid_history_range",
                range(Some("2026-08-12T10:00:00Z"), None),
            ),
            (
                "invalid_history_range",
                range(Some("2026-08-12T10:00:00Z"), Some("2026-08-12T10:00:00Z")),
            ),
            (
                "invalid_history_range",
                range(Some("2026-08-12T10:00:00Z"), Some("2026-09-13T10:00:00Z")),
            ),
            (
                "invalid_history_range",
                range(
                    Some("2026-08-12T10:00:00+08:00"),
                    Some("2026-08-12T10:15:00Z"),
                ),
            ),
            (
                "invalid_history_range",
                AdminPeerHistoryQuery {
                    grain: Some("5m".to_owned()),
                    from: Some("2026-08-12T10:00:00Z".to_owned()),
                    to: Some("2026-08-12T10:15:00Z".to_owned()),
                    before: Some("2026-08-12T11:00:00Z".to_owned()),
                    limit: None,
                },
            ),
        ];
        for (code, query) in refused {
            let (status, body) = peer_history_query(&state, "node-healthy", query).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"]["code"], code);
        }
        let (_, body) = peer_history_query(
            &state,
            "node-healthy",
            range(Some("2026-08-12T10:00:00Z"), None),
        )
        .await;
        assert_eq!(
            body["error"]["message"], "a Peer range needs both from and to",
            "a half-stated range is refused rather than completed"
        );
        // No grain at all stays the fixed-tail answer the Node page already reads.
        let (status, body) =
            peer_history_query(&state, "node-healthy", AdminPeerHistoryQuery::default()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["range"].is_null(), "the fixed tails are not a range");
        assert!(body["five_minute"].is_array());
        assert!(body["hourly"].is_array());
        // An unknown Node is a non-leaking 404 for a ranged read too.
        let (status, body) = peer_history_query(&state, "node-missing", range(None, None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn partial_identity_observation_stays_unknown_and_never_fabricates_values() {
        let (_dir, state) = node_inventory_state().await;
        // The retired Node observes only a chain id; genesis and HRP are
        // never observed. A partial observation must not claim `matched`
        // or invent Registry values for the missing fields.
        sqlx::query(
            "INSERT INTO current_node_chain_observations (node_id, network_chain_id, updated_at) VALUES ('node-retired', 210425, '2026-01-01T00:00:00Z')",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        let response = admin_node_detail(
            State(state.clone()),
            Path("node-retired".to_owned()),
            Extension(lifecycle_session()),
            Extension(request_id()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["identity"]["state"], "unknown");
        assert_eq!(value["identity"]["observed"]["chain_id"], 210425);
        assert_eq!(value["identity"]["observed"]["genesis_hash"], Value::Null);
        assert_eq!(value["identity"]["observed"]["address_hrp"], Value::Null);
        assert!(
            value["identity"]["mismatched_fields"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
