//! Block, metric, Host metric, storage mount, and state history reads.
use super::*;

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminBlockHistoryItem {
    pub node_id: String,
    pub height: Option<i64>,
    pub block_time_ms: Option<i64>,
    pub transaction_count: Option<i64>,
    pub coinbase: Option<String>,
    pub seal_signer_match: Option<String>,
    pub seal_signer_key_fingerprint: Option<String>,
    pub node_key_fingerprint: Option<String>,
    pub node_key_valid_from: Option<String>,
    pub node_key_valid_until: Option<String>,
    pub seal_recovery_rule: Option<String>,
    pub seal_evidence: Option<String>,
    pub protocol_proposer: Option<String>,
    pub attribution_reason: Option<String>,
    pub observed_at: Option<String>,
    pub freshness: Option<String>,
    pub gap_from_height: Option<i64>,
    pub gap_to_height: Option<i64>,
    pub gap_kind: Option<String>,
    pub gap_reason: Option<String>,
    pub divergence_kind: Option<String>,
    pub divergence_reason: Option<String>,
    pub divergence_retained_hash: Option<String>,
    pub divergence_observed_hash: Option<String>,
    pub divergence_observed_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminBlockHistoryResponse {
    pub items: Vec<AdminBlockHistoryItem>,
    /// `unavailable` when the requested range extends beyond raw retention.
    pub availability: Option<String>,
    pub aggregate_supported: bool,
    pub raw_retention_days: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdminHistoryQuery {
    limit: Option<i64>,
    from: Option<i64>,
    to: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct AdminHistoryRow {
    block_number: Option<i64>,
    block_timestamp_ms: Option<i64>,
    transaction_count: Option<i64>,
    coinbase: Option<String>,
    seal_signer_match: Option<String>,
    seal_signer_key_fingerprint: Option<String>,
    node_key_fingerprint: Option<String>,
    node_key_valid_from: Option<String>,
    node_key_valid_until: Option<String>,
    seal_recovery_rule: Option<String>,
    seal_evidence: Option<String>,
    protocol_proposer: Option<String>,
    attribution_reason: Option<String>,
    observed_at: Option<String>,
    from_height: Option<i64>,
    to_height: Option<i64>,
    gap_kind: Option<String>,
    gap_reason: Option<String>,
    divergence_kind: Option<String>,
    divergence_reason: Option<String>,
    divergence_retained_hash: Option<String>,
    divergence_observed_hash: Option<String>,
    divergence_observed_at: Option<String>,
}

fn admin_history_bounds(
    params: &AdminHistoryQuery,
    request_id: &str,
) -> Result<(Option<i64>, Option<i64>), Box<Response>> {
    if params.from.is_some_and(|value| value < 0)
        || params.to.is_some_and(|value| value < 0)
        || params
            .from
            .zip(params.to)
            .is_some_and(|(from, to)| from > to)
    {
        return Err(Box::new(mutation_error(
            request_id,
            StatusCode::BAD_REQUEST,
            "invalid_history_range",
            "history range is invalid",
        )));
    }
    Ok((params.from, params.to))
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/history",
    tag = "admin",
    params(
        ("node_id" = String, Path, description = "Node ID"),
        ("from" = Option<i64>, Query, minimum = 0, description = "First block height"),
        ("to" = Option<i64>, Query, minimum = 0, description = "Last block height"),
        ("limit" = Option<i64>, Query, minimum = 1, maximum = 200, description = "Maximum rows")
    ),
    responses(
        (status = 200, body = AdminBlockHistoryResponse),
        (status = 400, body = crate::http::ApiErrorBody),
        (status = 401, body = crate::http::ApiErrorBody),
        (status = 403, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(super) async fn admin_node_history(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
    axum::extract::Query(params): axum::extract::Query<AdminHistoryQuery>,
    Path(node_id): Path<String>,
) -> Response {
    let (from, to) = match admin_history_bounds(&params, &request_id.0) {
        Ok(bounds) => bounds,
        Err(response) => return *response,
    };
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    match sqlx::query_scalar::<_, i64>("SELECT 1 FROM nodes WHERE node_id=$1")
        .bind(&node_id)
        .fetch_optional(state.db().pool())
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return mutation_error(
                &request_id.0,
                StatusCode::NOT_FOUND,
                "not_found",
                "resource not found",
            );
        }
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    }
    let raw_retention_days =
        match crate::retention::raw_block_summary_retention_days(state.db().pool()).await {
            Ok(days) => days,
            Err(_) => {
                return mutation_error(
                    &request_id.0,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "server database is unavailable",
                );
            }
        };
    let cutoff = crate::auth::format_rfc3339(crate::retention::family_cutoff(
        crate::auth::now_utc(),
        raw_retention_days,
    ));
    let oldest_raw = sqlx::query_scalar::<_, Option<String>>(
        "SELECT MIN(accepted_at) FROM block_summaries WHERE node_id=$1",
    )
    .bind(&node_id)
    .fetch_one(state.db().pool())
    .await;
    let oldest_raw = match oldest_raw {
        Ok(value) => value,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let has_history = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT historical_high_watermark FROM block_history_state WHERE node_id=$1",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await;
    let has_history = match has_history {
        Ok(value) => value.flatten().is_some_and(|height| height > 0),
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let has_expired_raw = oldest_raw.as_ref().is_some_and(|value| value < &cutoff);
    let availability = (has_expired_raw || (oldest_raw.is_none() && has_history))
        .then(|| "unavailable".to_owned());
    let rows = sqlx::query_as::<_, AdminHistoryRow>("SELECT block_number, block_timestamp_ms, transaction_count, source, coinbase, seal_signer_match, seal_signer_key_fingerprint, node_key_fingerprint, node_key_valid_from, node_key_valid_until, seal_recovery_rule, seal_evidence, CASE WHEN protocol_proposer_kind = 'verified' THEN protocol_proposer_identity ELSE NULL END, attribution_reason, observed_at, from_height, to_height, gap_kind, gap_reason, divergence_kind, divergence_reason, divergence_retained_hash, divergence_observed_hash, divergence_observed_at FROM (SELECT block_number, block_timestamp_ms, transaction_count, 'summary', coinbase, seal_signer_match, seal_signer_key_fingerprint, node_key_fingerprint, node_key_valid_from, node_key_valid_until, seal_recovery_rule, seal_evidence, protocol_proposer_kind, protocol_proposer_identity, attribution_reason, observed_at, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL FROM block_summaries WHERE node_id = $1 AND accepted_at >= $2 UNION ALL SELECT NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, created_at, from_height, to_height, kind, reason, NULL, NULL, NULL, NULL, NULL FROM block_history_gaps WHERE node_id = $3 UNION ALL SELECT NULL, NULL, NULL, 'divergence', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, retained_observed_at, height, height, NULL, NULL, 'chain_divergence', reason, retained_block_hash, observed_block_hash, observed_at FROM chain_divergence_observations WHERE node_id = $4) AS history WHERE ($5 IS NULL OR COALESCE(block_number, to_height) >= $6) AND ($7 IS NULL OR COALESCE(block_number, from_height) <= $8) ORDER BY COALESCE(block_number, from_height) DESC LIMIT $9")
        .bind(&node_id)
        .bind(&cutoff)
        .bind(&node_id)
        .bind(&node_id)
        .bind(from)
        .bind(from)
        .bind(to)
        .bind(to)
        .bind(200_i64)
        .fetch_all(state.db().pool())
        .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "server database is unavailable",
            );
        }
    };
    let mut rows = rows;
    rows.retain(|row| {
        let first_height = row.block_number.or(row.from_height);
        let last_height = row.block_number.or(row.to_height);
        from.is_none_or(|minimum| last_height.is_some_and(|height| height >= minimum))
            && to.is_none_or(|maximum| first_height.is_some_and(|height| height <= maximum))
    });
    rows.truncate(limit as usize);
    Json(AdminBlockHistoryResponse {
        items: rows
            .into_iter()
            .map(|row| AdminBlockHistoryItem {
                node_id: node_id.clone(),
                height: row.block_number,
                block_time_ms: row.block_timestamp_ms,
                transaction_count: row.transaction_count,
                coinbase: row.coinbase,
                seal_signer_match: row.seal_signer_match,
                seal_signer_key_fingerprint: row.seal_signer_key_fingerprint,
                node_key_fingerprint: row.node_key_fingerprint,
                node_key_valid_from: row.node_key_valid_from,
                node_key_valid_until: row.node_key_valid_until,
                seal_recovery_rule: row.seal_recovery_rule,
                seal_evidence: row
                    .seal_evidence
                    .map(|value| crate::redaction::redact_sensitive(&value)),
                protocol_proposer: row.protocol_proposer,
                attribution_reason: row
                    .attribution_reason
                    .map(|value| crate::redaction::redact_sensitive(&value)),
                freshness: row.observed_at.clone(),
                observed_at: row.observed_at,
                gap_from_height: row.from_height,
                gap_to_height: row.to_height,
                gap_kind: row.gap_kind,
                gap_reason: row
                    .gap_reason
                    .map(|value| crate::redaction::redact_sensitive(&value)),
                divergence_kind: row.divergence_kind,
                divergence_reason: row
                    .divergence_reason
                    .map(|value| crate::redaction::redact_sensitive(&value)),
                divergence_retained_hash: row.divergence_retained_hash,
                divergence_observed_hash: row.divergence_observed_hash,
                divergence_observed_at: row.divergence_observed_at,
            })
            .collect::<Vec<_>>(),
        availability,
        aggregate_supported: crate::retention::RAW_BLOCK_HISTORY_AGGREGATES_SUPPORTED,
        raw_retention_days,
    })
    .into_response()
}
/// Query for the Owner-side raw Node metric history range (issue #213, design
/// §11.4). `metric` is optional at the type level so an unknown name answers
/// with the same error body as every other rejected query.
#[derive(Debug, Deserialize)]
pub(super) struct AdminMetricHistoryQuery {
    metric: Option<String>,
    /// The mount path of a Host storage series. Absent or empty reads the
    /// series of an owner that has one of its own, and an unknown mount path
    /// answers as a series that was never observed.
    dimension: Option<String>,
    from: Option<String>,
    to: Option<String>,
    /// Exclusive upper bound of a paging request: the continuation coordinate a
    /// previous truncated answer returned.
    before: Option<String>,
    limit: Option<i64>,
}

/// One stored raw observation with the timing evidence that belongs to it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminMetricSample {
    /// The point's coordinate: the observation instant of a raw sample, or the
    /// aligned start of an aggregate bucket.
    pub observed_at: String,
    /// For a raw sample, the receipt that carried it. For a bucket, the receipt
    /// of the bucket's newest observation.
    pub received_at: String,
    /// A raw sample's value, or a bucket's newest value.
    pub value: f64,
    /// The grain this point was served at: `raw`, `1m` or `5m`.
    pub grain: String,
    /// `raw` for a stored observation, `aggregate` for a bucket that
    /// summarizes observations the raw window no longer holds.
    pub source: String,
    /// The extremes of the observations behind this point. A raw sample is its
    /// own extreme; a bucket keeps the spike the raw samples would have shown.
    pub min_value: f64,
    pub max_value: f64,
    /// Observations behind this point: 1 for a raw sample, the bucket's counted
    /// observations for a bucket. A thin bucket says so here instead of looking
    /// like a closed stretch.
    pub sample_count: i64,
    /// The newest observation this point holds. Equal to `observedAt` for a raw
    /// sample; for a bucket it is the instant inside the bucket that delay and
    /// clock suspicion are measured from.
    pub last_observed_at: String,
    /// The oldest observation this point holds: the other end of the stretch it
    /// really testifies to, and equal to `observedAt` for a raw sample. A bucket
    /// whose two ends are far apart states a span, not continuity - a surface
    /// that says how long the point covers must measure it between these two
    /// instants, never from the bucket's own aligned start (review F1).
    pub first_observed_at: String,
    /// The widest interval the bucket recorded between two of its consecutive
    /// observations (0 for a raw sample, and for a bucket that counted one).
    /// This is measured evidence about the inside of a bucket: a surface can
    /// compare it with the cadence it sees and draw a line break where the
    /// Server proved a hole instead of a continuous stretch. Zero means the
    /// observations it counted were contiguous, never that nobody measured.
    pub max_gap_seconds: i64,
    /// `received_at - last_observed_at` for this very point, so spool and
    /// transport delay are visible per observation. Unknown when either
    /// timestamp is unusable — never reported as zero.
    pub delay_seconds: Option<i64>,
    /// The observation is stamped after the receipt: the Agent clock is ahead.
    pub clock_suspect: bool,
    pub clock_note: Option<String>,
}

/// A stretch of the window in which the Server stores no observation.
///
/// Reported as a gap with its kind, so no surface has to bridge silence, hold a
/// constant value across it, or draw it as a zero (design §11.4).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminMetricGap {
    pub from: String,
    pub to: String,
    pub seconds: i64,
    /// `collection_gap` (nobody observed) or `protection_pause` (the operator
    /// chose to pause collection).
    pub kind: String,
    pub reason: String,
    /// Counted losses behind a `protection_pause` (issue #212 evidence).
    pub skipped_count: Option<i64>,
}

/// What the Server knows about the series itself, independent of the window.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminMetricSeries {
    /// False when this Node never reported the series: shown as absent, not as
    /// zero.
    pub observed: bool,
    /// The oldest observation ever recorded. It outlives the samples and is not
    /// evidence that raw history survives that far back, and it is deliberately
    /// not called the moment the series was enabled: the Node may have been
    /// collecting before the first Report this Server accepted, so the Server
    /// reports what it has evidence for.
    pub first_observed_at: Option<String>,
    /// The newest observation instant the series ever recorded. A series-level
    /// stamp: it outlives the samples, and it is not evidence that a stored
    /// sample exists at that instant any more.
    pub last_observed_at: Option<String>,
    /// The receipt time of the delivery that recorded the newest observation:
    /// a series-level delivery stamp, so it can belong to a later restatement of
    /// that instant rather than to the sample the answer carries. The per-sample
    /// receipt is `items[].receivedAt`.
    pub last_received_at: Option<String>,
    pub observation_count: i64,
    /// Deliveries that restated an observation already stored: counted
    /// separately, never as new observations.
    pub replayed_count: i64,
    /// Observations whose value changed: counted separately too.
    pub corrected_count: i64,
    /// Samples this answer actually carries.
    pub sampled_count: i64,
    /// Seconds the samples prove they were observed.
    pub coverage_seconds: i64,
    pub window_seconds: i64,
    /// `receivedAt - observedAt` of the newest stored sample in this answer,
    /// measured from that one row. Unknown when the answer holds no stored
    /// sample — never reported as zero.
    pub latest_delay_seconds: Option<i64>,
    /// That same newest stored sample is stamped after its receipt: the Agent
    /// clock is ahead of the Server's.
    pub latest_clock_suspect: bool,
}

/// One tier's stretch of a metric-history answer.
///
/// It tells the caller which grain answered which stretch, so no surface has to
/// infer the resolution from the points themselves or present a bucket as if it
/// were a sample (design §11.6).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminMetricSegment {
    /// The stretch this segment answers: the older end inclusive, the newer end
    /// exclusive, matching what the reader asked of the tier.
    pub from: String,
    pub to: String,
    /// `raw`, `1m` or `5m`.
    pub grain: String,
    /// `raw` or `aggregate`.
    pub source: String,
    /// Points this answer carries for the segment.
    pub point_count: i64,
    /// True when the segment hit the answer's limit, so the stretch holds more
    /// evidence than this answer carries for it.
    pub truncated: bool,
}

/// Owner-only metric history for one Node series (issues #213 and #214, design
/// §11.4 and §11.6): the stored observations of the raw window, the aggregate
/// buckets that answer the stretches beyond it, the silences between them, and
/// the state of the series behind them.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminMetricHistoryResponse {
    /// `node` when the series belongs to one Node's own process, `host` when it
    /// is the Host series the Agent collected once for every Node on it.
    pub scope_kind: String,
    /// The identity the series is stored under: the Node ID, or the Agent ID.
    pub scope_key: String,
    /// The Node whose page asked for the answer, when a Node page asked. Null on
    /// the Agent route; on a Host answer it names the Node that asked, so no
    /// surface presents shared evidence as one Node's own.
    pub node_id: Option<String>,
    pub metric: String,
    /// The series' dimension: the mount path of a Host storage series, empty for
    /// every series that exists once per owner.
    pub dimension: String,
    /// The answered range, after clamping to the investigation horizon.
    pub from: String,
    /// The answered range's end: the requested `to`, or the exclusive paging
    /// cursor when the caller paged with `before`.
    pub to: String,
    /// The range the caller asked for, so a clamped or paged answer says what it
    /// clamped or narrowed.
    pub requested_from: String,
    /// `None` while the whole requested range is answerable, `partial` when it
    /// was clamped to the investigation horizon, `unavailable` when it is older
    /// than any retained history.
    pub availability: Option<String>,
    /// The configured raw window, which decides where the raw tier ends.
    pub raw_retention_days: i64,
    /// The oldest age any tier answers, in days: the investigation horizon the
    /// 5-minute tier and its retention family both declare.
    pub history_horizon_days: i64,
    /// The finest grain this answer carries: `raw`, `1m`, `5m`, or `none`
    /// when no tier holds anything for the requested range.
    pub grain: String,
    /// True while the Server serves the 1-minute and 5-minute tiers for the
    /// stretches older than the raw window (issue #214).
    pub aggregate_supported: bool,
    /// Which tier answered which stretch, oldest stretch first.
    pub segments: Vec<AdminMetricSegment>,
    pub items: Vec<AdminMetricSample>,
    pub gaps: Vec<AdminMetricGap>,
    pub series: AdminMetricSeries,
    pub window_seconds: i64,
    /// True when any tier had more evidence than the caller's limit: the newest
    /// points are returned and the rest is reported, never dropped silently.
    pub truncated: bool,
    /// When the answer is truncated, the coordinate to pass back as `before` for
    /// the next, older page. `None` when the answer reaches the oldest end of
    /// the requested range.
    pub continuation: Option<String>,
}

/// One storage series of one mount path, in the shape the Agent page reads
/// (issue #216, design §11.6).
///
/// The counts and the boundary come from the series' ledger, which outlives the
/// samples: the evidence an operator investigates with is the range reader's, and
/// this answer only says which series hold evidence and how much of it. A series
/// that was never observed is stated as such rather than as an empty series, and a
/// series whose readings have been released says so through `releasedBefore`
/// instead of presenting its boundary as an empty stretch.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminStorageSeries {
    pub metric: String,
    /// True when this mount path was observed for this metric at all.
    pub observed: bool,
    /// The oldest instant ever observed for this series.
    pub first_observed_at: Option<String>,
    /// The newest instant the Server stores for this series. It stays set after
    /// the readings themselves are released, because the ledger keeps the fact.
    pub last_observed_at: Option<String>,
    pub last_received_at: Option<String>,
    pub observation_count: i64,
    pub replayed_count: i64,
    pub corrected_count: i64,
    /// The instant before which this series' readings were released by retention,
    /// null while none has been. It is how a missing newest reading is explained:
    /// released on purpose, never "no data was ever collected".
    pub released_before: Option<String>,
    /// The newest stored reading, null when the raw window no longer holds one.
    /// Unknown is never reported as zero.
    pub latest_value: Option<f64>,
    pub latest_observed_at: Option<String>,
    pub latest_received_at: Option<String>,
    /// `latestReceivedAt - latestObservedAt` for the newest reading, null when
    /// either timestamp is unusable.
    pub latest_delay_seconds: Option<i64>,
    /// The newest reading is stamped after its receipt: the Agent clock is ahead.
    pub latest_clock_suspect: bool,
}

/// One mount path the Agent reported, with both storage series behind it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminStorageMount {
    /// The path exactly as the Agent reported it. It is the whole identity of
    /// the series: the Server compares it literally and never normalizes it, so
    /// two spellings of one filesystem are two series, and two filesystems that
    /// swap places behind one path are not claimed to be distinguishable.
    pub mount_path: String,
    /// `reported` while the Agent is still observing this path at its measured
    /// cadence, `silent` when it has said nothing for longer than that cadence
    /// allows, `unknown` when the Agent's cadence cannot be measured from the
    /// stored evidence. A slow cadence is never mistaken for silence.
    pub observation_state: String,
    /// Seconds since the newest observation of this path, null when the Server
    /// has no cadence to judge a silence against or is holding the readings back
    /// itself. An age with no measured cadence is an age, not a verdict.
    pub silent_seconds: Option<i64>,
    /// Bytes in use on this path (`disk_used_bytes`).
    pub used: AdminStorageSeries,
    /// Bytes this path can hold (`disk_total_bytes`).
    pub capacity: AdminStorageSeries,
}

/// The mount paths one Agent reported, and the coverage of the storage series
/// behind each of them (issue #216, design §11.6).
///
/// The mount path is the identity of a Host storage series: it is stored as the
/// series' dimension and compared literally. Without this list an operator has to
/// guess the exact string, and a path the Agent stopped reporting becomes
/// unreachable exactly when it matters. The list comes from the stored evidence,
/// never from a device identifier: same path under a new device is
/// indistinguishable by contract, and no such identity is invented here.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminAgentStorageMountsResponse {
    pub agent_id: String,
    /// The instant this answer was assembled from.
    pub answered_at: String,
    /// The rhythm the Agent is observing its Host at now, measured from the
    /// newest stored observations of the Host series that stands for the whole
    /// Host observation, and 0 when that evidence does not show one yet: an Agent
    /// that has just changed its collection interval states no cadence until its
    /// newest interval agrees with the one before it.
    pub cadence_seconds: i64,
    /// The silence that means "this path is no longer being reported": three
    /// cadences of the Agent's measured cadence, with the cadence a silence is
    /// judged against capped at five minutes, and never less than two minutes;
    /// 0 when the cadence is unknown. No silence is ever excused by a cadence
    /// nobody could configure, and no silence is declared while the Server itself
    /// is pausing optional history.
    pub silence_threshold_seconds: i64,
    /// The series the `used` side of every mount answers with.
    pub used_metric: String,
    /// The series the `capacity` side of every mount answers with.
    pub capacity_metric: String,
    /// The number of mount paths this answer can carry, and the bound it keeps:
    /// each storage side is read with this bound, the two are merged by the path
    /// from one snapshot of the ledger, and the merged list is cut at the same
    /// bound in the same order.
    pub mount_limit: i64,
    /// True when the Agent holds more mount paths than this answer carries: the
    /// newest paths are answered and the oldest ones are left out, stated here
    /// rather than dropped silently.
    pub truncated: bool,
    /// True while low-space protection has optional history paused (design
    /// §11.5). The Agent is still reporting and the Server is choosing not to
    /// store the readings, so no path is answered as silent: what was already
    /// stored stays, and the age of it is not the Agent going quiet.
    pub collection_paused: bool,
    /// The mount paths, most recently observed first, ties settled by the path.
    pub mounts: Vec<AdminStorageMount>,
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/metric-history",
    tag = "admin",
    params(
        ("node_id" = String, Path, description = "Node ID"),
        ("metric" = String, Query, description = "Stored Node Process metric series"),
        ("from" = Option<String>, Query, description = "Canonical RFC 3339 UTC start of the range, second precision (default: 24 hours before to)"),
        ("to" = Option<String>, Query, description = "Canonical RFC 3339 UTC end of the range, second precision (default: now)"),
        ("before" = Option<String>, Query, description = "Canonical RFC 3339 UTC exclusive upper bound for paging: the continuation coordinate a truncated answer returned"),
        ("limit" = Option<i64>, Query, minimum = 1, maximum = 20000, description = "Maximum points across every tier")
    ),
    responses(
        (status = 200, body = AdminMetricHistoryResponse),
        (status = 400, body = crate::http::ApiErrorBody),
        (status = 401, body = crate::http::ApiErrorBody),
        (status = 403, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(super) async fn admin_node_metric_history(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
    query: Result<
        axum::extract::Query<AdminMetricHistoryQuery>,
        axum::extract::rejection::QueryRejection,
    >,
    Path(node_id): Path<String>,
) -> Response {
    let params = match metric_history_params(query, &request_id.0) {
        Ok(params) => params,
        Err(response) => return *response,
    };
    // A query the typed extractor cannot read — a duplicate parameter, a limit
    // that is not a number — is answered with the repo's stable JSON error
    // envelope instead of a framework-generated plain-text rejection, so no
    // client has to parse two shapes out of the same route.
    let Some(metric) = params
        .metric
        .as_deref()
        .filter(|metric| crate::metric_history::is_node_metric(metric))
    else {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_metric",
            "metric is not a stored node metric series",
        );
    };
    let metric = metric.to_owned();
    match sqlx::query_scalar::<_, i64>("SELECT 1 FROM nodes WHERE node_id=$1")
        .bind(&node_id)
        .fetch_optional(state.db().pool())
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return not_found_response(&request_id.0),
        Err(_) => return unavailable_response(&request_id.0),
    }
    // These series belong to the Node's own process and are stored under it, so
    // the answer says so: they are never the Host series of the Agent that owns
    // this Node, which the sibling route answers (design §11.6).
    let node_scope_key = node_id.clone();
    metric_history_response(
        &state,
        &request_id.0,
        params,
        crate::metric_history::SeriesScope::node(&node_scope_key, &metric),
        MetricHistoryIdentity {
            scope_kind: "node",
            scope_key: node_scope_key.clone(),
            node_id: Some(node_id),
            dimension: String::new(),
        },
    )
    .await
}

/// Query for the Owner-side recorded state history of one Node component
/// (issue #217, design §11.7). `component` is optional at the type
/// level so a missing or unknown name answers with the same error body as every
/// other rejected query.
#[derive(Debug, Deserialize)]
pub(super) struct AdminStateHistoryQuery {
    component: Option<String>,
    from: Option<String>,
    to: Option<String>,
    /// Exclusive upper bound of a paging request: the continuation coordinate a
    /// previous truncated answer returned.
    before: Option<String>,
    limit: Option<i64>,
}

/// One recorded state of a Node component, with the timing evidence that
/// belongs to it.
///
/// The evidence of a state is a state vector, never a number: the collection
/// state, where the value came from, which failure was reported and the sync
/// flag as it was observed. A failure is therefore recorded as a failure, and a
/// value the Agent could not refresh keeps the instant it was really observed
/// at, so no surface has to invent a transition or read Unknown as zero
/// (design §11.7, §5.1; issue #217).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminStateEntry {
    /// The instant the Node was heard from: the entry coordinate.
    pub observed_at: String,
    /// The instant the Server received the Report that carried it.
    pub received_at: String,
    /// `change` for a delivery whose state differs from the state before it,
    /// `anchor` for one that re-recorded an unchanged state.
    pub entry_kind: String,
    /// `starting`, `ok`, `error`, `disabled` or `unsupported`.
    pub collection_state: String,
    /// `current`, `last_good` or `none`.
    pub value_source: String,
    /// The instant of the value this state refers to. It is older than
    /// observed_at for a last-good state, so the age of the value can be read
    /// instead of a fresh-looking timestamp.
    pub value_observed_at: Option<String>,
    /// The failure the Agent reported, when the collection failed.
    pub error_code: Option<String>,
    /// The sync flag, present only where the collection succeeded: a failed
    /// probe is unknown, never false.
    pub syncing: Option<bool>,
    /// The Agent-to-Server delay of this delivery.
    pub delay_seconds: Option<i64>,
    /// The observation is stamped after the receipt: the Agent clock is ahead.
    pub clock_suspect: bool,
    pub clock_note: Option<String>,
}

/// A stretch of the window in which the record shows nothing, because either
/// nobody reported or protection paused recording.
///
/// Reported as a gap with its kind, so no surface bridges silence with a
/// constant state, calls a missing report a normal period, or reads a pause as
/// a change (design §11.7, §11.5; issue #217).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminStateGap {
    pub from: String,
    pub to: String,
    pub seconds: i64,
    /// `collection_gap` (nobody observed) or `protection_pause` (the operator
    /// chose to pause collection).
    pub kind: String,
    pub reason: String,
    /// Counted losses behind a `protection_pause`.
    pub skipped_count: Option<i64>,
}

/// What the Server knows about the recorded-state series itself, independent of
/// the requested window.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminStateSeries {
    /// False when this Node never had a state recorded: shown as absent, not as
    /// a state.
    pub observed: bool,
    /// The oldest instant the series ever counted, even when its row was
    /// released.
    pub first_observed_at: Option<String>,
    /// The newest instant the series ever counted.
    pub last_observed_at: Option<String>,
    /// The instant the newest counted Report arrived.
    pub last_received_at: Option<String>,
    /// Counted deliveries, including the ones that wrote no row because the
    /// state had not changed.
    pub entry_count: i64,
    /// Counted deliveries that recorded a state change.
    pub change_count: i64,
    /// Counted deliveries that re-recorded an unchanged state.
    pub anchor_count: i64,
    /// Deliveries of an instant the log had already counted.
    pub replayed_count: i64,
    /// Deliveries that disagreed with the row already held for their instant.
    pub corrected_count: i64,
    /// The state of the newest counted delivery, which is the Server latest
    /// word on this component even when its row was released.
    pub latest_collection_state: Option<String>,
    pub latest_value_source: Option<String>,
    pub latest_value_observed_at: Option<String>,
    pub latest_error_code: Option<String>,
    pub latest_syncing: Option<bool>,
    /// How many entries this answer carries.
    pub entries_returned: i64,
    /// The seconds the returned entries account for: only the stretch between
    /// two recorded states counts, never the stretch after the newest one.
    pub coverage_seconds: i64,
    pub window_seconds: i64,
    /// The floor this series last stamped for evidence its window released, so
    /// a missing early entry is a boundary and not a quiet start.
    pub released_before: Option<String>,
}

/// Owner-only recorded state history for one Node component (issue #217,
/// design §11.7): the recorded state changes and hourly anchors, the
/// silences between them, and the state of the series behind them.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminStateHistoryResponse {
    pub node_id: String,
    /// `sync` or `consensus`.
    pub component: String,
    /// The answered range, after clamping to the retention horizon.
    pub from: String,
    /// The answered range end: the requested `to`, or the exclusive paging
    /// cursor when the caller paged with `before`.
    pub to: String,
    /// The range the caller asked for, so a clamped or paged answer says what it
    /// clamped or narrowed.
    pub requested_from: String,
    /// `None` while the whole requested range is answerable, `partial` when it
    /// was clamped to the retention horizon, `unavailable` when it is older than
    /// any retained history.
    pub availability: Option<String>,
    /// The configured window of this family, which is also its whole horizon:
    /// recorded states have no aggregate tiers.
    pub retention_days: i64,
    /// How long an unchanged state may go unrecorded before an anchor is
    /// written, so a reader can tell an unchanged state from a missing report.
    pub anchor_seconds: i64,
    /// The reporting cadence of the Node, or zero when it is unknown: the
    /// cadence the silences were judged against.
    pub cadence_seconds: i64,
    pub coverage_seconds: i64,
    pub window_seconds: i64,
    pub entries: Vec<AdminStateEntry>,
    pub gaps: Vec<AdminStateGap>,
    pub series: AdminStateSeries,
    /// True when the window held more entries than the caller limit: the newest
    /// are returned and the rest is reported, never dropped silently.
    pub truncated: bool,
    /// When the answer is truncated, the coordinate to pass back as `before` for
    /// the next, older page.
    pub continuation: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/state-history",
    tag = "admin",
    params(
        ("node_id" = String, Path, description = "Node ID"),
        ("component" = String, Query, description = "Recorded state component: sync or consensus"),
        ("from" = Option<String>, Query, description = "Canonical RFC 3339 UTC start of the range, second precision (default: 24 hours before to)"),
        ("to" = Option<String>, Query, description = "Canonical RFC 3339 UTC end of the range, second precision (default: now)"),
        ("before" = Option<String>, Query, description = "Canonical RFC 3339 UTC exclusive upper bound for paging: the continuation coordinate a truncated answer returned"),
        ("limit" = Option<i64>, Query, minimum = 1, maximum = 20000, description = "Maximum recorded states across the answer")
    ),
    responses(
        (status = 200, body = AdminStateHistoryResponse),
        (status = 400, body = crate::http::ApiErrorBody),
        (status = 401, body = crate::http::ApiErrorBody),
        (status = 403, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(super) async fn admin_node_state_history(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
    query: Result<
        axum::extract::Query<AdminStateHistoryQuery>,
        axum::extract::rejection::QueryRejection,
    >,
    Path(node_id): Path<String>,
) -> Response {
    let params = match query {
        Ok(axum::extract::Query(params)) => params,
        Err(_) => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_query",
                "query is not valid for state history",
            );
        }
    };
    // A component outside the recorded pair answers like an unknown metric: the
    // caller gets the same error body and no empty series that would look like a
    // component nobody ever reported.
    let component = match params.component.as_deref() {
        Some(value) if crate::state_history::is_state_component(value) => value.to_owned(),
        _ => {
            return mutation_error(
                &request_id.0,
                StatusCode::BAD_REQUEST,
                "invalid_component",
                "component is not a recorded state series",
            );
        }
    };
    match sqlx::query_scalar::<_, i64>("SELECT 1 FROM nodes WHERE node_id=$1")
        .bind(&node_id)
        .fetch_optional(state.db().pool())
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return not_found_response(&request_id.0),
        Err(_) => return unavailable_response(&request_id.0),
    }
    // Recorded states describe the Node itself, so the answer says so: they are
    // never the Agent or Host history of the Agent that reported them
    // (design §11.6).
    state_history_response(&state, &request_id.0, params, &node_id, &component).await
}

/// One Owner recorded-state answer (design §11.7; issue #217).
async fn state_history_response(
    state: &AppState,
    request_id: &str,
    params: AdminStateHistoryQuery,
    node_id: &str,
    component: &str,
) -> Response {
    let unavailable = || unavailable_response(request_id);
    let invalid_range = || {
        mutation_error(
            request_id,
            StatusCode::BAD_REQUEST,
            "invalid_history_range",
            "history range is invalid",
        )
    };
    let retention_days =
        match crate::retention::observation_state_retention_days(state.db().pool()).await {
            Ok(days) => days,
            Err(_) => return unavailable(),
        };
    let now = crate::auth::now_utc();
    let to = match params.to.as_deref() {
        Some(value) => match crate::metric_history::canonical_instant(value) {
            Some(value) => value,
            None => return invalid_range(),
        },
        None => now,
    };
    let from = match params.from.as_deref() {
        Some(value) => match crate::metric_history::canonical_instant(value) {
            Some(value) => value,
            None => return invalid_range(),
        },
        None => to - time::Duration::hours(crate::metric_history::DEFAULT_WINDOW_HOURS),
    };
    if from > to {
        return invalid_range();
    }
    // A paging cursor narrows the answer to the evidence older than a
    // coordinate the caller has already seen, so it must sit strictly inside the
    // requested range: a cursor outside it would answer a stretch nobody asked
    // about.
    let before = match params.before.as_deref() {
        Some(value) => match crate::metric_history::canonical_instant(value) {
            Some(value) => Some(value),
            None => return invalid_range(),
        },
        None => None,
    };
    if let Some(before) = before
        && (before <= from || before > to)
    {
        return invalid_range();
    }
    // A recorded state has no aggregate tier: the retention policy is its whole
    // horizon, so the range is clamped to it and the clamping is reported rather
    // than served as a silence the Node never had.
    let horizon_cutoff = now - time::Duration::days(retention_days);
    let availability = if from >= horizon_cutoff {
        None
    } else if to <= horizon_cutoff {
        Some("unavailable".to_owned())
    } else {
        Some("partial".to_owned())
    };
    let effective_from = from.max(horizon_cutoff);
    let effective_to = before.unwrap_or(to);
    let limit = params
        .limit
        .unwrap_or(crate::state_history::DEFAULT_STATE_LIMIT)
        .clamp(1, crate::state_history::MAX_STATE_LIMIT);
    let cutoff = crate::auth::format_rfc3339(crate::retention::family_cutoff(now, retention_days));
    let from_text = crate::auth::format_rfc3339(effective_from);
    // The advertised end of the page, not the end of the whole range: an older
    // page must not report a gap that runs past where it says it stops. The row
    // query already excludes the cursor instant, so loading against the effective
    // end narrows only what the answer claims about the stretch it covers.
    let to_text = crate::auth::format_rfc3339(effective_to);
    let before_text = before.map(crate::auth::format_rfc3339);
    let range = match crate::state_history::load_range(
        state.db().pool(),
        &crate::state_history::StateRangeQuery {
            node_id,
            component,
            from: &from_text,
            to: &to_text,
            before: before_text.as_deref(),
            limit,
            cutoff: &cutoff,
        },
    )
    .await
    {
        Ok(range) => range,
        Err(_) => return unavailable(),
    };
    let window_seconds = (effective_to - effective_from).whole_seconds().max(0);
    let entries = range
        .entries
        .iter()
        .map(|entry| {
            let (delay_seconds, clock_note) = match crate::metric_history::sample_timing(
                &entry.observed_at,
                &entry.received_at,
            ) {
                Some(timing) => (Some(timing.delay_seconds), timing.clock_note),
                None => (None, None),
            };
            AdminStateEntry {
                observed_at: entry.observed_at.clone(),
                received_at: entry.received_at.clone(),
                entry_kind: entry.entry_kind.clone(),
                collection_state: entry.collection_state.clone(),
                value_source: entry.value_source.clone(),
                value_observed_at: entry.value_observed_at.clone(),
                error_code: entry.error_code.clone(),
                syncing: entry.syncing,
                delay_seconds,
                clock_suspect: clock_note.is_some(),
                clock_note,
            }
        })
        .collect::<Vec<_>>();
    let gaps = range
        .gaps
        .iter()
        .map(|gap| AdminStateGap {
            from: gap.from.clone(),
            to: gap.to.clone(),
            seconds: gap.seconds,
            kind: gap.kind.as_str().to_owned(),
            reason: match gap.kind {
                crate::metric_history::GapKind::ProtectionPause => {
                    "low-space protection paused state recording".to_owned()
                }
                crate::metric_history::GapKind::Collection => {
                    "no state was recorded in this stretch".to_owned()
                }
            },
            skipped_count: gap.skipped_count,
        })
        .collect::<Vec<_>>();
    let series = match &range.ledger {
        Some(ledger) => {
            // The ledger states the state of the newest counted delivery, which
            // is the Server latest word on this component even when the row that
            // recorded it has been released.
            let latest = ledger.latest();
            AdminStateSeries {
                observed: true,
                first_observed_at: Some(ledger.first_observed_at.clone()),
                last_observed_at: Some(ledger.last_observed_at.clone()),
                last_received_at: Some(ledger.last_received_at.clone()),
                entry_count: ledger.entry_count,
                change_count: ledger.change_count,
                anchor_count: ledger.anchor_count,
                replayed_count: ledger.replayed_count,
                corrected_count: ledger.corrected_count,
                latest_collection_state: Some(latest.collection_state.clone()),
                latest_value_source: Some(latest.value_source.clone()),
                latest_value_observed_at: ledger.last_value_observed_at.clone(),
                latest_error_code: latest.error_code.clone(),
                latest_syncing: latest.syncing,
                entries_returned: range.entries.len() as i64,
                coverage_seconds: range.coverage_seconds,
                window_seconds,
                released_before: Some(ledger.released_before.clone()),
            }
        }
        None => AdminStateSeries {
            observed: false,
            first_observed_at: None,
            last_observed_at: None,
            last_received_at: None,
            entry_count: 0,
            change_count: 0,
            anchor_count: 0,
            replayed_count: 0,
            corrected_count: 0,
            latest_collection_state: None,
            latest_value_source: None,
            latest_value_observed_at: None,
            latest_error_code: None,
            latest_syncing: None,
            entries_returned: 0,
            coverage_seconds: 0,
            window_seconds,
            released_before: None,
        },
    };
    // Owner-only Node history is per-session data: it must never be stored by
    // an intermediary or a browser cache and replayed to another session
    // (design §10.2, webui.md §6.4).
    no_store(
        Json(AdminStateHistoryResponse {
            node_id: node_id.to_owned(),
            component: component.to_owned(),
            from: crate::auth::format_rfc3339(effective_from),
            to: crate::auth::format_rfc3339(effective_to),
            requested_from: crate::auth::format_rfc3339(from),
            availability,
            retention_days,
            anchor_seconds: crate::state_history::STATE_ANCHOR_SECONDS,
            cadence_seconds: range.cadence_seconds,
            coverage_seconds: range.coverage_seconds,
            window_seconds,
            entries,
            gaps,
            series,
            truncated: range.truncated,
            continuation: range.continuation.clone(),
        })
        .into_response(),
    )
}
#[utoipa::path(
    get,
    path = "/api/admin/v1/agents/{agent_id}/metric-history",
    tag = "admin",
    params(
        ("agent_id" = String, Path, description = "Agent ID"),
        ("metric" = String, Query, description = "Stored Host metric series the Agent collected"),
        ("dimension" = Option<String>, Query, description = "Mount path of a Host storage series (disk_used_bytes, disk_total_bytes); empty for every other Host series"),
        ("from" = Option<String>, Query, description = "Canonical RFC 3339 UTC start of the range, second precision (default: 24 hours before to)"),
        ("to" = Option<String>, Query, description = "Canonical RFC 3339 UTC end of the range, second precision (default: now)"),
        ("before" = Option<String>, Query, description = "Canonical RFC 3339 UTC exclusive upper bound for paging: the continuation coordinate a truncated answer returned"),
        ("limit" = Option<i64>, Query, minimum = 1, maximum = 20000, description = "Maximum points across every tier")
    ),
    responses(
        (status = 200, body = AdminMetricHistoryResponse),
        (status = 400, body = crate::http::ApiErrorBody),
        (status = 401, body = crate::http::ApiErrorBody),
        (status = 403, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(super) async fn admin_agent_metric_history(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
    query: Result<
        axum::extract::Query<AdminMetricHistoryQuery>,
        axum::extract::rejection::QueryRejection,
    >,
    Path(agent_id): Path<String>,
) -> Response {
    let params = match metric_history_params(query, &request_id.0) {
        Ok(params) => params,
        Err(response) => return *response,
    };
    let Some(metric) = params
        .metric
        .as_deref()
        .filter(|metric| crate::metric_history::is_host_metric(metric))
    else {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_metric",
            "metric is not a stored host metric series",
        );
    };
    let metric = metric.to_owned();
    if let Err(response) = host_owner_exists(&state, &request_id.0, &agent_id).await {
        return *response;
    }
    // The series this Agent collected once for every Node on it.
    let dimension = params.dimension.clone().unwrap_or_default();
    let agent_scope_key = agent_id.clone();
    metric_history_response(
        &state,
        &request_id.0,
        params,
        crate::metric_history::SeriesScope::host(&agent_scope_key, &metric, &dimension),
        MetricHistoryIdentity {
            scope_kind: "host",
            scope_key: agent_scope_key.clone(),
            node_id: None,
            dimension: dimension.clone(),
        },
    )
    .await
}

/// The mount paths one Agent reported for its Host storage, with the coverage of
/// the series behind each path (issue #216, design §11.6).
///
/// The persisted evidence of one Host is one series per metric and mount path, so
/// the list of paths is what makes that evidence reachable at all. This route
/// answers that list from the ledger, which outlives the readings: a mount the
/// Agent stopped reporting, or one whose readings retention released, is still
/// answered with its boundary and its counts instead of disappearing from the
/// surface that is supposed to investigate it.
///
/// It reads one Agent's own evidence and nothing shared from it. The route states
/// which series holds how much, and leaves the readings themselves, their gaps and
/// their low-space pauses to the metric-history routes and the capacity route,
/// which already answer them per mount path: a second copy of the sampling
/// evidence is not what a coverage answer is for.
///
/// One answer is one snapshot of the ledger. The two storage series and the
/// cadence a silence is judged against are read inside one transaction, the paths
/// they hold are merged by path and cut at the stated limit after the merge, and
/// no path is answered as silent while low-space protection has optional history
/// paused: then the Server, not the Agent, is the one holding the readings back,
/// and the answer says so instead of calling a reporting Agent quiet.
#[utoipa::path(
    get,
    path = "/api/admin/v1/agents/{agent_id}/storage-mounts",
    tag = "admin",
    params(
        ("agent_id" = String, Path, description = "Agent ID")
    ),
    responses(
        (status = 200, body = AdminAgentStorageMountsResponse),
        (status = 401, body = crate::http::ApiErrorBody),
        (status = 403, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(super) async fn admin_agent_storage_mounts(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
    Path(agent_id): Path<String>,
) -> Response {
    if let Err(response) = host_owner_exists(&state, &request_id.0, &agent_id).await {
        return *response;
    }
    // Both sides of a mount path, and the cadence a silence is judged against,
    // come from one snapshot of the ledger: the answer opens one read
    // transaction and asks all three questions inside it, because an Agent that
    // reports a whole new mount contract between two independent reads would
    // otherwise be answered from two different ledgers, with paths present on
    // one side only and a merged list longer than the limit the answer states.
    // They are merged by the path the Agent reported, never by an identity the
    // Server would have to invent.
    let mut snapshot = match crate::database::begin(state.db().pool()).await {
        Ok(snapshot) => snapshot,
        Err(_) => return unavailable_response(&request_id.0),
    };
    let coverage = match crate::metric_history::load_mount_coverage(&mut snapshot, &agent_id).await
    {
        Ok(coverage) => coverage,
        Err(_) => return unavailable_response(&request_id.0),
    };
    if snapshot.commit().await.is_err() {
        return unavailable_response(&request_id.0);
    }
    // Every Host series is written from the one observation the Agent collected
    // for its Host, so the Agent has one cadence and it is measured once instead
    // of once per mount.
    let cadence_seconds = coverage.cadence_seconds;
    // The merge, the order and the bound are the ledger's own rules, so the
    // engine answers the paths of one snapshot merged by path and already cut at
    // the bound this answer states.
    let (paths, truncated) = crate::metric_history::merge_mount_coverage(coverage);
    let now = crate::auth::now_utc();
    // Zero when the cadence is unknown: an unmeasurable cadence cannot excuse a
    // silence, so the mounts answer `unknown` rather than `reported`.
    let silence_threshold_seconds = if cadence_seconds > 0 {
        crate::metric_history::gap_threshold_seconds(cadence_seconds)
    } else {
        0
    };
    // While low-space protection has optional history paused, the Agent is still
    // reporting and the Server is the one holding the readings back: nothing
    // already stored is deleted, and a stretch of paused collection is not the
    // Agent going quiet. The pause is stated on the answer instead of turning
    // every path into a silence the Agent never caused.
    let collection_paused = state.capacity().status().protected;
    let mut answer = Vec::with_capacity(paths.len());
    for path in paths {
        let newest = path.newest_observation().map(str::to_owned);
        // The age of the newest reading is a silence only where the Server has a
        // measured cadence to compare it with and is not holding the readings
        // back itself. With no cadence the mount answers unknown, and the age of
        // what was stored stays on the series itself (latestObservedAt) instead
        // of arriving as a silence the Server cannot support.
        let silent_seconds = if cadence_seconds > 0 && !collection_paused {
            newest
                .as_deref()
                .and_then(crate::auth::parse_rfc3339)
                .map(|observed| (now - observed).whole_seconds())
        } else {
            None
        };
        let observation_state = crate::metric_history::mount_observation_state(
            cadence_seconds,
            silent_seconds,
            silence_threshold_seconds,
        );
        answer.push(AdminStorageMount {
            mount_path: path.mount_path,
            observation_state: observation_state.to_owned(),
            silent_seconds,
            used: storage_series(
                crate::metric_history::HOST_MOUNT_USED_METRIC,
                path.used.as_ref(),
            ),
            capacity: storage_series(
                crate::metric_history::HOST_MOUNT_CAPACITY_METRIC,
                path.capacity.as_ref(),
            ),
        });
    }
    no_store(
        Json(AdminAgentStorageMountsResponse {
            agent_id,
            answered_at: crate::auth::format_rfc3339(now),
            cadence_seconds,
            silence_threshold_seconds,
            used_metric: crate::metric_history::HOST_MOUNT_USED_METRIC.to_owned(),
            capacity_metric: crate::metric_history::HOST_MOUNT_CAPACITY_METRIC.to_owned(),
            mount_limit: crate::metric_history::MOUNT_COVERAGE_LIMIT,
            truncated,
            collection_paused,
            mounts: answer,
        })
        .into_response(),
    )
}

/// One side of a mount path, in the shape the Agent page reads.
fn storage_series(
    metric: &str,
    coverage: Option<&crate::metric_history::SeriesCoverage>,
) -> AdminStorageSeries {
    let Some(coverage) = coverage else {
        return AdminStorageSeries {
            metric: metric.to_owned(),
            observed: false,
            first_observed_at: None,
            last_observed_at: None,
            last_received_at: None,
            observation_count: 0,
            replayed_count: 0,
            corrected_count: 0,
            released_before: None,
            latest_value: None,
            latest_observed_at: None,
            latest_received_at: None,
            latest_delay_seconds: None,
            latest_clock_suspect: false,
        };
    };
    let timing = coverage.latest.as_ref().and_then(|latest| {
        crate::metric_history::sample_timing(&latest.observed_at, &latest.received_at)
    });
    AdminStorageSeries {
        metric: metric.to_owned(),
        observed: true,
        first_observed_at: Some(coverage.first_observed_at.clone()),
        last_observed_at: Some(coverage.last_observed_at.clone()),
        last_received_at: Some(coverage.last_received_at.clone()),
        observation_count: coverage.observation_count,
        replayed_count: coverage.replayed_count,
        corrected_count: coverage.corrected_count,
        released_before: coverage.released_before.clone(),
        latest_value: coverage.latest.as_ref().map(|latest| latest.value),
        latest_observed_at: coverage
            .latest
            .as_ref()
            .map(|latest| latest.observed_at.clone()),
        latest_received_at: coverage
            .latest
            .as_ref()
            .map(|latest| latest.received_at.clone()),
        latest_delay_seconds: timing.as_ref().map(|timing| timing.delay_seconds),
        latest_clock_suspect: timing
            .as_ref()
            .is_some_and(|timing| timing.clock_note.is_some()),
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/v1/nodes/{node_id}/host-metric-history",
    tag = "admin",
    params(
        ("node_id" = String, Path, description = "Node ID"),
        ("metric" = String, Query, description = "Stored Host metric series of the Agent that owns this Node"),
        ("dimension" = Option<String>, Query, description = "Mount path of a Host storage series (disk_used_bytes, disk_total_bytes); empty for every other Host series"),
        ("from" = Option<String>, Query, description = "Canonical RFC 3339 UTC start of the range, second precision (default: 24 hours before to)"),
        ("to" = Option<String>, Query, description = "Canonical RFC 3339 UTC end of the range, second precision (default: now)"),
        ("before" = Option<String>, Query, description = "Canonical RFC 3339 UTC exclusive upper bound for paging: the continuation coordinate a truncated answer returned"),
        ("limit" = Option<i64>, Query, minimum = 1, maximum = 20000, description = "Maximum points across every tier")
    ),
    responses(
        (status = 200, body = AdminMetricHistoryResponse),
        (status = 400, body = crate::http::ApiErrorBody),
        (status = 401, body = crate::http::ApiErrorBody),
        (status = 403, body = crate::http::ApiErrorBody),
        (status = 404, body = crate::http::ApiErrorBody),
        (status = 503, body = crate::http::ApiErrorBody)
    )
)]
pub(super) async fn admin_node_host_metric_history(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    Extension(request_id): Extension<crate::http::RequestId>,
    query: Result<
        axum::extract::Query<AdminMetricHistoryQuery>,
        axum::extract::rejection::QueryRejection,
    >,
    Path(node_id): Path<String>,
) -> Response {
    let params = match metric_history_params(query, &request_id.0) {
        Ok(params) => params,
        Err(response) => return *response,
    };
    let Some(metric) = params
        .metric
        .as_deref()
        .filter(|metric| crate::metric_history::is_host_metric(metric))
    else {
        return mutation_error(
            &request_id.0,
            StatusCode::BAD_REQUEST,
            "invalid_metric",
            "metric is not a stored host metric series",
        );
    };
    let metric = metric.to_owned();
    // Which Agent's Host series this Node reads is a fact of its registration:
    // a Node owns none of them, and the answer names the Agent that does, so one
    // Node's page never presents the shared totals as that Node's own (design
    // §11.6).
    let agent_id =
        match sqlx::query_scalar::<_, String>("SELECT agent_id FROM nodes WHERE node_id=$1")
            .bind(&node_id)
            .fetch_optional(state.db().pool())
            .await
        {
            Ok(Some(agent_id)) => agent_id,
            Ok(None) => return not_found_response(&request_id.0),
            Err(_) => return unavailable_response(&request_id.0),
        };
    let dimension = params.dimension.clone().unwrap_or_default();
    let agent_scope_key = agent_id.clone();
    metric_history_response(
        &state,
        &request_id.0,
        params,
        crate::metric_history::SeriesScope::host(&agent_scope_key, &metric, &dimension),
        MetricHistoryIdentity {
            scope_kind: "host",
            scope_key: agent_scope_key.clone(),
            node_id: Some(node_id),
            dimension: dimension.clone(),
        },
    )
    .await
}

/// The parsed query, or the answer a query the typed extractor cannot read gets.
fn metric_history_params(
    query: Result<
        axum::extract::Query<AdminMetricHistoryQuery>,
        axum::extract::rejection::QueryRejection,
    >,
    request_id: &str,
) -> Result<AdminMetricHistoryQuery, Box<Response>> {
    match query {
        Ok(axum::extract::Query(params)) => Ok(params),
        // The rejection is the route's usual error envelope, boxed so this call
        // chain's Result stays small: the envelope is built once on a malformed
        // query, and all three handlers would otherwise carry its size in every
        // successful answer they return.
        Err(_) => Err(Box::new(mutation_error(
            request_id,
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "query is not valid for metric history",
        ))),
    }
}

/// Whether the Agent a shared Host series is stored under is registered.
///
/// The error is boxed for the same reason the metric-history query rejection is:
/// the envelope is built once on a missing or unreadable Agent, and every caller
/// would otherwise carry its size in the Result they return on success too.
async fn host_owner_exists(
    state: &AppState,
    request_id: &str,
    agent_id: &str,
) -> Result<(), Box<Response>> {
    match sqlx::query_scalar::<_, i64>("SELECT 1 FROM agents WHERE agent_id=$1")
        .bind(agent_id)
        .fetch_optional(state.db().pool())
        .await
    {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(Box::new(not_found_response(request_id))),
        Err(_) => Err(Box::new(unavailable_response(request_id))),
    }
}

/// The answer a Node or Agent the Server does not know gets, in the shape every
/// other missing resource answers with.
pub(super) fn not_found_response(request_id: &str) -> Response {
    mutation_error(
        request_id,
        StatusCode::NOT_FOUND,
        "not_found",
        "resource not found",
    )
}

/// The answer every metric-history route gives when the database cannot be read.
pub(super) fn unavailable_response(request_id: &str) -> Response {
    mutation_error(
        request_id,
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "server database is unavailable",
    )
}

/// The identity an Owner metric-history answer states: which scope the series is
/// stored under, which Node page asked for it, and which dimension separates two
/// series of one owner and metric.
struct MetricHistoryIdentity {
    /// `node` for a Node's own process series, `host` for the Host series one
    /// Agent collected once for every Node on it.
    scope_kind: &'static str,
    /// The identity the series is stored under: the Node ID, or the Agent ID.
    scope_key: String,
    /// The Node whose page asked for the answer, when a Node page asked.
    node_id: Option<String>,
    /// The series' dimension: the mount path of a Host storage series, empty for
    /// every series that exists once per owner.
    dimension: String,
}

/// One Owner metric-history answer, shared by the Node, the Agent and the
/// Node-Host routes: all three read the same engine, clamp the range the same
/// way and state the same evidence (design §11.4, §11.6).
async fn metric_history_response(
    state: &AppState,
    request_id: &str,
    params: AdminMetricHistoryQuery,
    scope: crate::metric_history::SeriesScope<'_>,
    identity: MetricHistoryIdentity,
) -> Response {
    let unavailable = || unavailable_response(request_id);
    let invalid_range = || {
        mutation_error(
            request_id,
            StatusCode::BAD_REQUEST,
            "invalid_history_range",
            "history range is invalid",
        )
    };
    let raw_retention_days =
        match crate::retention::metric_sample_retention_days(state.db().pool()).await {
            Ok(days) => days,
            Err(_) => return unavailable(),
        };
    let now = crate::auth::now_utc();
    let to = match params.to.as_deref() {
        Some(value) => match crate::metric_history::canonical_instant(value) {
            Some(value) => value,
            None => return invalid_range(),
        },
        None => now,
    };
    let from = match params.from.as_deref() {
        Some(value) => match crate::metric_history::canonical_instant(value) {
            Some(value) => value,
            None => return invalid_range(),
        },
        None => to - time::Duration::hours(crate::metric_history::DEFAULT_WINDOW_HOURS),
    };
    if from > to {
        return invalid_range();
    }
    // A paging cursor narrows the answer to the evidence older than a
    // coordinate the caller has already seen, so it must sit strictly inside the
    // requested range: a cursor outside it would answer a stretch nobody asked
    // about.
    let before = match params.before.as_deref() {
        Some(value) => match crate::metric_history::canonical_instant(value) {
            Some(value) => Some(value),
            None => return invalid_range(),
        },
        None => None,
    };
    if let Some(before) = before
        && (before <= from || before > to)
    {
        return invalid_range();
    }
    // How far back history can be answered is a fact of the retention policy,
    // not of the answer: the range is clamped to the investigation horizon and
    // the clamping is reported.
    //
    // Which stretch is answered as raw samples is not a policy at all (review
    // F3). The tier contract is fixed: the last 24 hours are answered by the
    // samples the Server received, older than that by 1-minute buckets, older
    // than 7 days by 5-minute buckets. The raw retention policy decides how long
    // the Server keeps those rows on disk; if it decided the reader's grain too,
    // widening it to 7 days would answer a three-day range out of buckets the
    // reader was promised raw, and shortening it would make buckets answer a
    // stretch whose samples are still stored.
    let raw_cutoff = crate::metric_history::raw_window_cutoff(now);
    let horizon_cutoff =
        now - time::Duration::days(crate::metric_history::FIVE_MINUTE_MAX_AGE_DAYS);
    let availability =
        crate::metric_history::horizon_availability(from, to, horizon_cutoff).map(str::to_owned);
    let effective_from = from.max(horizon_cutoff);
    let effective_to = before.unwrap_or(to);
    let limit = params
        .limit
        .unwrap_or(crate::metric_history::DEFAULT_SAMPLE_LIMIT)
        .clamp(1, crate::metric_history::MAX_SAMPLE_LIMIT);
    let range = match crate::metric_history::load_range(
        state.db().pool(),
        crate::metric_history::RangeQuery {
            scope,
            from: effective_from,
            to,
            before,
            limit,
            raw_cutoff,
            now,
        },
    )
    .await
    {
        Ok(range) => range,
        Err(_) => return unavailable(),
    };
    let window_seconds = (effective_to - effective_from).whole_seconds().max(0);
    let items = range
        .points
        .iter()
        .map(|point| {
            let (delay_seconds, clock_note) = match crate::metric_history::sample_timing(
                &point.last_observed_at,
                &point.received_at,
            ) {
                Some(timing) => (Some(timing.delay_seconds), timing.clock_note),
                None => (None, None),
            };
            AdminMetricSample {
                observed_at: point.instant.clone(),
                received_at: point.received_at.clone(),
                value: point.value,
                grain: point.grain.to_owned(),
                source: point.source.to_owned(),
                min_value: point.min_value,
                max_value: point.max_value,
                sample_count: point.sample_count,
                last_observed_at: point.last_observed_at.clone(),
                first_observed_at: point.first_observed_at.clone(),
                max_gap_seconds: point.max_gap_seconds,
                delay_seconds,
                clock_suspect: clock_note.is_some(),
                clock_note,
            }
        })
        .collect::<Vec<_>>();
    // The answer's own grain is the finest resolution it carries: a reader that
    // wants one word for the answer gets the best one, and the segments say
    // where the coarser stretches are.
    let grain = if range
        .points
        .iter()
        .any(|point| point.grain == crate::metric_history::GRAIN_RAW)
    {
        crate::metric_history::GRAIN_RAW
    } else if range
        .points
        .iter()
        .any(|point| point.grain == crate::metric_history::GRAIN_ONE_MINUTE)
    {
        crate::metric_history::GRAIN_ONE_MINUTE
    } else if range
        .points
        .iter()
        .any(|point| point.grain == crate::metric_history::GRAIN_FIVE_MINUTE)
    {
        crate::metric_history::GRAIN_FIVE_MINUTE
    } else {
        "none"
    };
    let segments = range
        .segments
        .iter()
        .map(|segment| AdminMetricSegment {
            from: segment.from.clone(),
            to: segment.to.clone(),
            grain: segment.grain.to_owned(),
            source: segment.source.to_owned(),
            point_count: segment.point_count,
            truncated: segment.truncated,
        })
        .collect::<Vec<_>>();
    let gaps = range
        .gaps
        .iter()
        .map(|gap| AdminMetricGap {
            from: gap.from.clone(),
            to: gap.to.clone(),
            seconds: gap.seconds,
            kind: gap.kind.as_str().to_owned(),
            reason: match gap.kind {
                crate::metric_history::GapKind::ProtectionPause => {
                    "low-space protection paused sample collection".to_owned()
                }
                crate::metric_history::GapKind::Collection => {
                    "no observation was received in this stretch".to_owned()
                }
            },
            skipped_count: gap.skipped_count,
        })
        .collect::<Vec<_>>();
    let series = match &range.ledger {
        Some(ledger) => {
            // The newest point of this answer carries the timestamps of the
            // observation behind it, so the reported delay and clock suspicion
            // always come from one delivery — also for a bucket, whose evidence
            // is its newest observation. The ledger pair stays a series-level
            // delivery stamp: its last_received_at can belong to a later
            // restatement of the newest instant, which is not the delay of a
            // point.
            let latest = range.points.last().and_then(|point| {
                crate::metric_history::sample_timing(&point.last_observed_at, &point.received_at)
            });
            AdminMetricSeries {
                observed: true,
                first_observed_at: Some(ledger.first_observed_at.clone()),
                last_observed_at: Some(ledger.last_observed_at.clone()),
                last_received_at: Some(ledger.last_received_at.clone()),
                observation_count: ledger.observation_count,
                replayed_count: ledger.replayed_count,
                corrected_count: ledger.corrected_count,
                sampled_count: range.points.len() as i64,
                coverage_seconds: range.coverage_seconds,
                window_seconds,
                latest_delay_seconds: latest.as_ref().map(|timing| timing.delay_seconds),
                latest_clock_suspect: latest
                    .as_ref()
                    .is_some_and(|timing| timing.clock_note.is_some()),
            }
        }
        None => AdminMetricSeries {
            observed: false,
            first_observed_at: None,
            last_observed_at: None,
            last_received_at: None,
            observation_count: 0,
            replayed_count: 0,
            corrected_count: 0,
            sampled_count: 0,
            coverage_seconds: 0,
            window_seconds,
            latest_delay_seconds: None,
            latest_clock_suspect: false,
        },
    };
    // Owner-only Node history is per-session data: it must never be stored by
    // an intermediary or a browser cache and replayed to another session
    // (design §10.2, webui.md §6.4).
    no_store(
        Json(AdminMetricHistoryResponse {
            scope_kind: identity.scope_kind.to_owned(),
            scope_key: identity.scope_key,
            node_id: identity.node_id,
            metric: scope.series.metric.to_owned(),
            dimension: identity.dimension,
            from: crate::auth::format_rfc3339(effective_from),
            to: crate::auth::format_rfc3339(effective_to),
            requested_from: crate::auth::format_rfc3339(from),
            availability,
            raw_retention_days,
            history_horizon_days: crate::metric_history::FIVE_MINUTE_MAX_AGE_DAYS,
            grain: grain.to_owned(),
            aggregate_supported: true,
            segments,
            truncated: range.truncated,
            continuation: range.continuation.clone(),
            window_seconds,
            items,
            gaps,
            series,
        })
        .into_response(),
    )
}
