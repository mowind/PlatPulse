//! Node-scoped Peer history aggregation and bounded history reads.
//!
//! Successful snapshots are reduced inside the Report Ingestion transaction.
//! The only historical Peer data retained here is operational summary data:
//! counts, country codes/counts, churn counts, and bounded CBFT lag statistics.
//! Raw Peer IDs, addresses, capabilities, and provider responses never enter
//! these tables.

use std::collections::{BTreeMap, HashMap};

use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool, Transaction};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use platpulse_core::observation::PeerSnapshot;

const FIVE_MINUTES: i64 = 5 * 60;
const ONE_HOUR: i64 = 60 * 60;
pub(crate) const HISTORY_FIVE_MINUTE_LIMIT: i64 = 288;
pub(crate) const HISTORY_HOURLY_LIMIT: i64 = 168;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PeerPresenceDelta {
    pub arrivals: i64,
    pub departures: i64,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct DbAggregateRow {
    pub bucket_start: String,
    pub sample_count: i64,
    pub total_peers: i64,
    pub inbound_count: i64,
    pub outbound_count: i64,
    pub trusted_count: i64,
    pub static_count: i64,
    pub consensus_count: i64,
    pub known_country_count: i64,
    pub unknown_country_count: i64,
    pub arrivals: i64,
    pub departures: i64,
    pub cbft_lag_count: i64,
    pub cbft_lag_sum: i64,
    pub cbft_lag_min: Option<i64>,
    pub cbft_lag_max: Option<i64>,
    pub first_observed_at: String,
    pub last_observed_at: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CountryCount {
    pub country_code: String,
    pub count: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct PeerAggregateRow {
    pub bucket_start: String,
    pub sample_count: i64,
    pub total_peers: i64,
    pub inbound_count: i64,
    pub outbound_count: i64,
    pub trusted_count: i64,
    pub static_count: i64,
    pub consensus_count: i64,
    pub known_country_count: i64,
    pub unknown_country_count: i64,
    pub arrivals: i64,
    pub departures: i64,
    pub cbft_lag_count: i64,
    pub cbft_lag_sum: i64,
    pub cbft_lag_min: Option<i64>,
    pub cbft_lag_max: Option<i64>,
    pub last_observed_at: String,
    pub countries: Vec<CountryCount>,
}

#[derive(Debug, Clone)]
pub(crate) struct PeerHistory {
    pub state: String,
    pub freshness: String,
    pub five_minute: Vec<PeerAggregateRow>,
    pub hourly: Vec<PeerAggregateRow>,
}

#[derive(Debug, Clone)]
struct AggregateValues {
    sample_count: i64,
    total_peers: i64,
    inbound_count: i64,
    outbound_count: i64,
    trusted_count: i64,
    static_count: i64,
    consensus_count: i64,
    known_country_count: i64,
    unknown_country_count: i64,
    arrivals: i64,
    departures: i64,
    cbft_lag_count: i64,
    cbft_lag_sum: i64,
    cbft_lag_min: Option<i64>,
    cbft_lag_max: Option<i64>,
    first_observed_at: String,
    last_observed_at: String,
    countries: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Copy)]
enum AggregateFamily {
    FiveMinute,
    Hourly,
}

impl AggregateFamily {
    fn seconds(self) -> i64 {
        match self {
            Self::FiveMinute => FIVE_MINUTES,
            Self::Hourly => ONE_HOUR,
        }
    }

    fn select_sql(self) -> &'static str {
        match self {
            Self::FiveMinute => {
                "SELECT bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at FROM peer_aggregate_5m WHERE node_id=? AND bucket_start=?"
            }
            Self::Hourly => {
                "SELECT bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at FROM peer_aggregate_1h WHERE node_id=? AND bucket_start=?"
            }
        }
    }

    fn update_sql(self) -> &'static str {
        match self {
            Self::FiveMinute => {
                "UPDATE peer_aggregate_5m SET sample_count=?, total_peers=?, inbound_count=?, outbound_count=?, trusted_count=?, static_count=?, consensus_count=?, known_country_count=?, unknown_country_count=?, arrivals=?, departures=?, cbft_lag_count=?, cbft_lag_sum=?, cbft_lag_min=?, cbft_lag_max=?, first_observed_at=?, last_observed_at=? WHERE node_id=? AND bucket_start=?"
            }
            Self::Hourly => {
                "UPDATE peer_aggregate_1h SET sample_count=?, total_peers=?, inbound_count=?, outbound_count=?, trusted_count=?, static_count=?, consensus_count=?, known_country_count=?, unknown_country_count=?, arrivals=?, departures=?, cbft_lag_count=?, cbft_lag_sum=?, cbft_lag_min=?, cbft_lag_max=?, first_observed_at=?, last_observed_at=? WHERE node_id=? AND bucket_start=?"
            }
        }
    }

    fn insert_sql(self) -> &'static str {
        match self {
            Self::FiveMinute => {
                "INSERT INTO peer_aggregate_5m (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            }
            Self::Hourly => {
                "INSERT INTO peer_aggregate_1h (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            }
        }
    }

    fn country_insert_sql(self) -> &'static str {
        match self {
            Self::FiveMinute => {
                "INSERT INTO peer_aggregate_5m_countries (node_id, bucket_start, country_code, peer_count) VALUES (?, ?, ?, ?) ON CONFLICT(node_id, bucket_start, country_code) DO UPDATE SET peer_count=peer_count + excluded.peer_count"
            }
            Self::Hourly => {
                "INSERT INTO peer_aggregate_1h_countries (node_id, bucket_start, country_code, peer_count) VALUES (?, ?, ?, ?) ON CONFLICT(node_id, bucket_start, country_code) DO UPDATE SET peer_count=peer_count + excluded.peer_count"
            }
        }
    }

    fn history_select_sql(self) -> &'static str {
        match self {
            Self::FiveMinute => {
                "SELECT bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at FROM peer_aggregate_5m WHERE node_id=? ORDER BY bucket_start DESC LIMIT ?"
            }
            Self::Hourly => {
                "SELECT bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at FROM peer_aggregate_1h WHERE node_id=? ORDER BY bucket_start DESC LIMIT ?"
            }
        }
    }

    fn country_select_prefix(self) -> &'static str {
        match self {
            Self::FiveMinute => {
                "SELECT bucket_start, country_code, peer_count FROM peer_aggregate_5m_countries WHERE node_id="
            }
            Self::Hourly => {
                "SELECT bucket_start, country_code, peer_count FROM peer_aggregate_1h_countries WHERE node_id="
            }
        }
    }
}

fn database_protocol_error(message: impl Into<String>) -> sqlx::Error {
    sqlx::Error::Protocol(message.into())
}

fn bucket_start(received_at: &str, interval: i64) -> Result<String, sqlx::Error> {
    let timestamp = OffsetDateTime::parse(received_at, &Rfc3339)
        .map_err(|error| database_protocol_error(format!("invalid receipt timestamp: {error}")))?;
    let unix_seconds = timestamp.unix_timestamp().div_euclid(interval) * interval;
    OffsetDateTime::from_unix_timestamp(unix_seconds)
        .map_err(|error| database_protocol_error(format!("invalid aggregate bucket: {error}")))?
        .format(&Rfc3339)
        .map_err(|error| database_protocol_error(format!("invalid aggregate timestamp: {error}")))
}

fn add_min(current: Option<i64>, incoming: Option<i64>) -> Option<i64> {
    match (current, incoming) {
        (Some(current), Some(incoming)) => Some(current.min(incoming)),
        (Some(current), None) => Some(current),
        (None, Some(incoming)) => Some(incoming),
        (None, None) => None,
    }
}

fn add_max(current: Option<i64>, incoming: Option<i64>) -> Option<i64> {
    match (current, incoming) {
        (Some(current), Some(incoming)) => Some(current.max(incoming)),
        (Some(current), None) => Some(current),
        (None, Some(incoming)) => Some(incoming),
        (None, None) => None,
    }
}

fn merge_rows(existing: &DbAggregateRow, incoming: &AggregateValues) -> AggregateValues {
    AggregateValues {
        sample_count: existing.sample_count.saturating_add(incoming.sample_count),
        total_peers: existing.total_peers.saturating_add(incoming.total_peers),
        inbound_count: existing
            .inbound_count
            .saturating_add(incoming.inbound_count),
        outbound_count: existing
            .outbound_count
            .saturating_add(incoming.outbound_count),
        trusted_count: existing
            .trusted_count
            .saturating_add(incoming.trusted_count),
        static_count: existing.static_count.saturating_add(incoming.static_count),
        consensus_count: existing
            .consensus_count
            .saturating_add(incoming.consensus_count),
        known_country_count: existing
            .known_country_count
            .saturating_add(incoming.known_country_count),
        unknown_country_count: existing
            .unknown_country_count
            .saturating_add(incoming.unknown_country_count),
        arrivals: existing.arrivals.saturating_add(incoming.arrivals),
        departures: existing.departures.saturating_add(incoming.departures),
        cbft_lag_count: existing
            .cbft_lag_count
            .saturating_add(incoming.cbft_lag_count),
        cbft_lag_sum: existing.cbft_lag_sum.saturating_add(incoming.cbft_lag_sum),
        cbft_lag_min: add_min(existing.cbft_lag_min, incoming.cbft_lag_min),
        cbft_lag_max: add_max(existing.cbft_lag_max, incoming.cbft_lag_max),
        first_observed_at: existing
            .first_observed_at
            .clone()
            .min(incoming.first_observed_at.clone()),
        last_observed_at: existing
            .last_observed_at
            .clone()
            .max(incoming.last_observed_at.clone()),
        countries: incoming.countries.clone(),
    }
}

async fn country_for_peer(
    tx: &mut Transaction<'_, Sqlite>,
    remote_ip: Option<&str>,
    received_at: &str,
) -> Result<Option<String>, sqlx::Error> {
    let Some(canonical_ip) = remote_ip.and_then(crate::geo::GeoLoader::canonical_public_ip) else {
        return Ok(None);
    };
    sqlx::query_scalar(
        "SELECT country_code FROM geo_location_cache WHERE canonical_ip=? AND expires_at > ?",
    )
    .bind(canonical_ip)
    .bind(received_at)
    .fetch_optional(&mut **tx)
    .await
}

async fn aggregate_values(
    tx: &mut Transaction<'_, Sqlite>,
    snapshot: &PeerSnapshot,
    received_at: &str,
    local_head: Option<u64>,
    delta: PeerPresenceDelta,
) -> Result<AggregateValues, sqlx::Error> {
    let mut countries = BTreeMap::new();
    let mut inbound_count = 0_i64;
    let mut outbound_count = 0_i64;
    let mut trusted_count = 0_i64;
    let mut static_count = 0_i64;
    let mut consensus_count = 0_i64;
    let mut cbft_lag_count = 0_i64;
    let mut cbft_lag_sum = 0_i64;
    let mut cbft_lag_min = None;
    let mut cbft_lag_max = None;

    for peer in &snapshot.peers {
        match peer.direction {
            platpulse_core::observation::PeerDirection::Inbound => inbound_count += 1,
            platpulse_core::observation::PeerDirection::Outbound => outbound_count += 1,
        }
        trusted_count += i64::from(peer.trusted);
        static_count += i64::from(peer.static_peer);
        consensus_count += i64::from(peer.consensus_peer);

        if let Some(country_code) =
            country_for_peer(tx, peer.remote_ip.as_deref(), received_at).await?
        {
            *countries.entry(country_code).or_insert(0) += 1;
        }

        // A peer's committed block is the most useful liveness marker. Fall
        // back to highest QC and then lock when a node does not report commit.
        if let (Some(head), Some(peer_block)) = (
            local_head,
            peer.cbft_commit_block
                .or(peer.cbft_highest_qc_block)
                .or(peer.cbft_locked_block),
        ) {
            let lag = head.saturating_sub(peer_block).min(i64::MAX as u64) as i64;
            cbft_lag_count += 1;
            cbft_lag_sum = cbft_lag_sum.saturating_add(lag);
            cbft_lag_min = add_min(cbft_lag_min, Some(lag));
            cbft_lag_max = add_max(cbft_lag_max, Some(lag));
        }
    }

    let known_country_count = countries.values().copied().sum();
    let total_peers = snapshot.peers.len() as i64;
    Ok(AggregateValues {
        sample_count: 1,
        total_peers,
        inbound_count,
        outbound_count,
        trusted_count,
        static_count,
        consensus_count,
        known_country_count,
        unknown_country_count: total_peers.saturating_sub(known_country_count),
        arrivals: delta.arrivals,
        departures: delta.departures,
        cbft_lag_count,
        cbft_lag_sum,
        cbft_lag_min,
        cbft_lag_max,
        first_observed_at: received_at.to_owned(),
        last_observed_at: received_at.to_owned(),
        countries,
    })
}

async fn upsert_family(
    tx: &mut Transaction<'_, Sqlite>,
    family: AggregateFamily,
    node_id: &str,
    bucket: &str,
    incoming: &AggregateValues,
) -> Result<(), sqlx::Error> {
    let existing = sqlx::query_as::<_, DbAggregateRow>(family.select_sql())
        .bind(node_id)
        .bind(bucket)
        .fetch_optional(&mut **tx)
        .await?;
    let merged = existing
        .as_ref()
        .map(|row| merge_rows(row, incoming))
        .unwrap_or_else(|| incoming.clone());

    if existing.is_some() {
        sqlx::query(family.update_sql())
            .bind(merged.sample_count)
            .bind(merged.total_peers)
            .bind(merged.inbound_count)
            .bind(merged.outbound_count)
            .bind(merged.trusted_count)
            .bind(merged.static_count)
            .bind(merged.consensus_count)
            .bind(merged.known_country_count)
            .bind(merged.unknown_country_count)
            .bind(merged.arrivals)
            .bind(merged.departures)
            .bind(merged.cbft_lag_count)
            .bind(merged.cbft_lag_sum)
            .bind(merged.cbft_lag_min)
            .bind(merged.cbft_lag_max)
            .bind(&merged.first_observed_at)
            .bind(&merged.last_observed_at)
            .bind(node_id)
            .bind(bucket)
            .execute(&mut **tx)
            .await?;
    } else {
        sqlx::query(family.insert_sql())
            .bind(node_id)
            .bind(bucket)
            .bind(merged.sample_count)
            .bind(merged.total_peers)
            .bind(merged.inbound_count)
            .bind(merged.outbound_count)
            .bind(merged.trusted_count)
            .bind(merged.static_count)
            .bind(merged.consensus_count)
            .bind(merged.known_country_count)
            .bind(merged.unknown_country_count)
            .bind(merged.arrivals)
            .bind(merged.departures)
            .bind(merged.cbft_lag_count)
            .bind(merged.cbft_lag_sum)
            .bind(merged.cbft_lag_min)
            .bind(merged.cbft_lag_max)
            .bind(&merged.first_observed_at)
            .bind(&merged.last_observed_at)
            .execute(&mut **tx)
            .await?;
    }

    for (country_code, count) in &incoming.countries {
        sqlx::query(family.country_insert_sql())
            .bind(node_id)
            .bind(bucket)
            .bind(country_code)
            .bind(*count)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Record one successful Node-scoped Peer observation in both aggregate
/// families. The caller must invoke this inside the report receipt
/// transaction, after current Peer/Geo projections have been updated.
pub(crate) async fn record_successful_snapshot(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
    snapshot: &PeerSnapshot,
    received_at: &str,
    local_head: Option<u64>,
    delta: PeerPresenceDelta,
) -> Result<(), sqlx::Error> {
    let values = aggregate_values(tx, snapshot, received_at, local_head, delta).await?;
    for family in [AggregateFamily::FiveMinute, AggregateFamily::Hourly] {
        let bucket = bucket_start(received_at, family.seconds())?;
        upsert_family(tx, family, node_id, &bucket, &values).await?;
    }
    Ok(())
}

async fn load_family(
    pool: &SqlitePool,
    node_id: &str,
    family: AggregateFamily,
    limit: i64,
) -> Result<Vec<PeerAggregateRow>, sqlx::Error> {
    let db_rows = sqlx::query_as::<_, DbAggregateRow>(family.history_select_sql())
        .bind(node_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    if db_rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut country_query = QueryBuilder::<Sqlite>::new(family.country_select_prefix());
    country_query
        .push_bind(node_id)
        .push(" AND bucket_start IN (");
    country_query.push_bind(&db_rows[0].bucket_start);
    for row in &db_rows[1..] {
        country_query.push(",").push_bind(&row.bucket_start);
    }
    country_query.push(") ORDER BY bucket_start DESC, country_code");
    let country_rows = country_query
        .build_query_as::<(String, String, i64)>()
        .fetch_all(pool)
        .await?;
    let mut countries_by_bucket: HashMap<String, Vec<CountryCount>> = HashMap::new();
    for (bucket_start, country_code, count) in country_rows {
        countries_by_bucket
            .entry(bucket_start)
            .or_default()
            .push(CountryCount {
                country_code,
                count,
            });
    }

    Ok(db_rows
        .into_iter()
        .map(|row| PeerAggregateRow {
            countries: countries_by_bucket
                .remove(&row.bucket_start)
                .unwrap_or_default(),
            bucket_start: row.bucket_start,
            sample_count: row.sample_count,
            total_peers: row.total_peers,
            inbound_count: row.inbound_count,
            outbound_count: row.outbound_count,
            trusted_count: row.trusted_count,
            static_count: row.static_count,
            consensus_count: row.consensus_count,
            known_country_count: row.known_country_count,
            unknown_country_count: row.unknown_country_count,
            arrivals: row.arrivals,
            departures: row.departures,
            cbft_lag_count: row.cbft_lag_count,
            cbft_lag_sum: row.cbft_lag_sum,
            cbft_lag_min: row.cbft_lag_min,
            cbft_lag_max: row.cbft_lag_max,
            last_observed_at: row.last_observed_at,
        })
        .collect())
}

fn row_is_empty(row: &PeerAggregateRow) -> bool {
    row.total_peers == 0
}

/// Read bounded aggregate history and preserve the component's explicit
/// state/freshness dimensions. A retained empty snapshot is `empty`; missing
/// history is `unknown`; a current collection error remains `error` while the
/// last-good aggregate rows stay available to the caller.
pub(crate) async fn load_history(
    pool: &SqlitePool,
    node_id: &str,
) -> Result<PeerHistory, sqlx::Error> {
    let component_status: Option<(String, i64)> = sqlx::query_as(
        "SELECT state, value_revision FROM component_status WHERE node_id=? AND component_key='peers'",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?;
    let component_state = component_status.as_ref().map(|(state, _)| state.as_str());
    let current_peer_count = match component_status.as_ref() {
        Some((state, value_revision)) if state == "ok" && *value_revision > 0 => Some(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM current_node_peers WHERE node_id=?")
                .bind(node_id)
                .fetch_one(pool)
                .await?,
        ),
        _ => None,
    };
    let five_minute = load_family(
        pool,
        node_id,
        AggregateFamily::FiveMinute,
        HISTORY_FIVE_MINUTE_LIMIT,
    )
    .await?;
    let hourly = load_family(pool, node_id, AggregateFamily::Hourly, HISTORY_HOURLY_LIMIT).await?;

    let latest = five_minute
        .iter()
        .chain(hourly.iter())
        .map(|row| row.last_observed_at.as_str())
        .filter_map(crate::auth::parse_rfc3339)
        .max();
    let freshness = match latest {
        None => "unknown",
        Some(value) if (crate::auth::now_utc() - value).whole_seconds().abs() <= 120 => "current",
        Some(_) => "stale",
    }
    .to_owned();

    let has_rows = !five_minute.is_empty() || !hourly.is_empty();
    let all_empty = five_minute.iter().chain(hourly.iter()).all(row_is_empty);
    let state = match component_state {
        Some("error") => "error",
        Some("unsupported") => "unsupported",
        Some("disabled") => "disabled",
        Some("starting") => "starting",
        Some("ok") if current_peer_count == Some(0) => "empty",
        Some(_) if has_rows && all_empty => "empty",
        Some(_) if has_rows => "ok",
        Some(_) => "unknown",
        None if has_rows && all_empty => "empty",
        None if has_rows => "ok",
        None => "unknown",
    }
    .to_owned();

    Ok(PeerHistory {
        state,
        freshness,
        five_minute,
        hourly,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_are_aligned_to_utc_boundaries() {
        assert_eq!(
            bucket_start("2026-08-12T10:07:31Z", FIVE_MINUTES).unwrap(),
            "2026-08-12T10:05:00Z"
        );
        assert_eq!(
            bucket_start("2026-08-12T10:07:31Z", ONE_HOUR).unwrap(),
            "2026-08-12T10:00:00Z"
        );
    }

    #[test]
    fn lag_stats_merge_without_inventing_values() {
        assert_eq!(add_min(None, None), None);
        assert_eq!(add_min(None, Some(4)), Some(4));
        assert_eq!(add_min(Some(8), Some(4)), Some(4));
        assert_eq!(add_max(Some(8), Some(4)), Some(8));
    }
}
