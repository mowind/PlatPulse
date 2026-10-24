//! Node-scoped Peer history aggregation and bounded history reads.
//!
//! Successful snapshots are reduced inside the Report Ingestion transaction.
//! The only historical Peer data retained here is operational summary data:
//! counts, country codes/counts, churn counts, and bounded CBFT lag statistics.
//! Raw Peer IDs, addresses, capabilities, and provider responses never enter
//! these tables.

use std::collections::{BTreeMap, HashMap, HashSet};

use sqlx::{Any, AnyPool, FromRow, Transaction};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

use platpulse_core::observation::PeerSnapshot;

const FIVE_MINUTES: i64 = 5 * 60;
const ONE_HOUR: i64 = 60 * 60;
pub(crate) const HISTORY_FIVE_MINUTE_LIMIT: i64 = 288;
pub(crate) const HISTORY_HOURLY_LIMIT: i64 = 168;

/// Distinct addresses one country lookup asks about, and country rows one
/// upsert statement carries.
///
/// A snapshot can name a few hundred Peers, and the whole Peer aggregate is
/// written inside Report Ingestion's receipt transaction, which holds the
/// Server's single write connection (ADR 0013, issue #213): the round trips
/// have to follow the Report, not the Peer count.
const COUNTRY_LOOKUP_IPS: usize = 500;
const COUNTRY_UPSERT_ROWS: usize = 200;

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
                "SELECT bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at FROM peer_aggregate_5m WHERE node_id=$1 AND bucket_start=$2"
            }
            Self::Hourly => {
                "SELECT bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at FROM peer_aggregate_1h WHERE node_id=$1 AND bucket_start=$2"
            }
        }
    }

    fn update_sql(self) -> &'static str {
        match self {
            Self::FiveMinute => {
                "UPDATE peer_aggregate_5m SET sample_count=$1, total_peers=$2, inbound_count=$3, outbound_count=$4, trusted_count=$5, static_count=$6, consensus_count=$7, known_country_count=$8, unknown_country_count=$9, arrivals=$10, departures=$11, cbft_lag_count=$12, cbft_lag_sum=$13, cbft_lag_min=$14, cbft_lag_max=$15, first_observed_at=$16, last_observed_at=$17 WHERE node_id=$18 AND bucket_start=$19"
            }
            Self::Hourly => {
                "UPDATE peer_aggregate_1h SET sample_count=$1, total_peers=$2, inbound_count=$3, outbound_count=$4, trusted_count=$5, static_count=$6, consensus_count=$7, known_country_count=$8, unknown_country_count=$9, arrivals=$10, departures=$11, cbft_lag_count=$12, cbft_lag_sum=$13, cbft_lag_min=$14, cbft_lag_max=$15, first_observed_at=$16, last_observed_at=$17 WHERE node_id=$18 AND bucket_start=$19"
            }
        }
    }

    fn insert_sql(self) -> &'static str {
        match self {
            Self::FiveMinute => {
                "INSERT INTO peer_aggregate_5m (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)"
            }
            Self::Hourly => {
                "INSERT INTO peer_aggregate_1h (node_id, bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)"
            }
        }
    }

    /// The country counts table one grain is stored in.
    fn country_table(self) -> &'static str {
        match self {
            Self::FiveMinute => "peer_aggregate_5m_countries",
            Self::Hourly => "peer_aggregate_1h_countries",
        }
    }

    /// The aggregate table one grain is stored in.
    fn table(self) -> &'static str {
        match self {
            Self::FiveMinute => "peer_aggregate_5m",
            Self::Hourly => "peer_aggregate_1h",
        }
    }

    /// The columns every history read returns, so the fixed tail and a bounded
    /// range answer the same shape.
    fn history_columns(self) -> &'static str {
        "bucket_start, sample_count, total_peers, inbound_count, outbound_count, trusted_count, static_count, consensus_count, known_country_count, unknown_country_count, arrivals, departures, cbft_lag_count, cbft_lag_sum, cbft_lag_min, cbft_lag_max, first_observed_at, last_observed_at"
    }

    fn history_select_sql(self) -> String {
        format!(
            "SELECT {} FROM {} WHERE node_id=$1 ORDER BY bucket_start DESC LIMIT $2",
            self.history_columns(),
            self.table()
        )
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

/// Resolve the country of every distinct address a snapshot names, for a
/// sample taken at `received_at`.
///
/// A country result stays usable for the whole hard cache-retention boundary,
/// not only for its 24-hour TTL: that is the same retained last-good rule the
/// Public Geo Insight applies, so both Public projections of one cache agree.
/// Beyond the boundary the result is no longer retained and counts as unknown.
async fn snapshot_countries(
    tx: &mut Transaction<'_, Any>,
    snapshot: &PeerSnapshot,
    received_at: &str,
    provider: crate::geo::GeoProvider,
) -> Result<HashMap<String, String>, sqlx::Error> {
    let remote_ips: Vec<Option<&str>> = snapshot
        .peers
        .iter()
        .map(|peer| peer.remote_ip.as_deref())
        .collect();
    countries_for_ips(tx, &remote_ips, received_at, provider).await
}

/// The lookup behind [`snapshot_countries`], over the addresses a snapshot
/// names. A non-public address never resolves, and duplicate addresses ask the
/// cache once.
async fn countries_for_ips(
    tx: &mut Transaction<'_, Any>,
    remote_ips: &[Option<&str>],
    received_at: &str,
    provider: crate::geo::GeoProvider,
) -> Result<HashMap<String, String>, sqlx::Error> {
    let mut canonical_ips: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for remote_ip in remote_ips {
        let Some(canonical_ip) = remote_ip.and_then(crate::geo::GeoLoader::canonical_public_ip)
        else {
            continue;
        };
        if seen.insert(canonical_ip.clone()) {
            canonical_ips.push(canonical_ip);
        }
    }
    let mut countries = HashMap::new();
    if canonical_ips.is_empty() {
        return Ok(countries);
    }
    let cutoff = crate::geo::cache_rebuild_cutoff(received_at);
    for chunk in canonical_ips.chunks(COUNTRY_LOOKUP_IPS) {
        // Only the selected provider's retained results are usable; a result
        // left over from another provider never resolves a country.
        let sql = crate::database::number_placeholders(&format!(
            "SELECT canonical_ip, country_code FROM geo_location_cache WHERE provider = ? AND canonical_ip IN ({}) AND created_at IS NOT NULL AND created_at > ?",
            crate::database::placeholder_list(chunk.len())
        ));
        let mut query = sqlx::query_as::<_, (String, Option<String>)>(&sql).bind(provider.as_str());
        for canonical_ip in chunk {
            query = query.bind(canonical_ip.as_str());
        }
        for (canonical_ip, country_code) in query.bind(&cutoff).fetch_all(&mut **tx).await? {
            if let Some(country_code) = country_code {
                countries.insert(canonical_ip, country_code);
            }
        }
    }
    Ok(countries)
}

async fn aggregate_values(
    tx: &mut Transaction<'_, Any>,
    snapshot: &PeerSnapshot,
    received_at: &str,
    local_head: Option<u64>,
    delta: PeerPresenceDelta,
    provider: crate::geo::GeoProvider,
) -> Result<AggregateValues, sqlx::Error> {
    let countries_by_ip = snapshot_countries(tx, snapshot, received_at, provider).await?;
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

        if let Some(country_code) = peer
            .remote_ip
            .as_deref()
            .and_then(crate::geo::GeoLoader::canonical_public_ip)
            .and_then(|canonical_ip| countries_by_ip.get(&canonical_ip).cloned())
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
    tx: &mut Transaction<'_, Any>,
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

    let country_rows: Vec<(&str, i64)> = incoming
        .countries
        .iter()
        .map(|(country_code, count)| (country_code.as_str(), *count))
        .collect();
    for chunk in country_rows.chunks(COUNTRY_UPSERT_ROWS) {
        let table = family.country_table();
        let sql = crate::database::number_placeholders(&format!(
            "INSERT INTO {table} (node_id, bucket_start, country_code, peer_count) VALUES {} ON CONFLICT(node_id, bucket_start, country_code) DO UPDATE SET peer_count={table}.peer_count + excluded.peer_count",
            crate::database::row_placeholders(4, chunk.len())
        ));
        let mut statement = sqlx::query(&sql);
        for (country_code, count) in chunk {
            statement = statement
                .bind(node_id)
                .bind(bucket)
                .bind(*country_code)
                .bind(*count);
        }
        statement.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Record one successful Node-scoped Peer observation in both aggregate
/// families. The caller must invoke this inside the report receipt
/// transaction, after current Peer/Geo projections have been updated.
pub(crate) async fn record_successful_snapshot(
    tx: &mut Transaction<'_, Any>,
    node_id: &str,
    snapshot: &PeerSnapshot,
    received_at: &str,
    local_head: Option<u64>,
    delta: PeerPresenceDelta,
    provider: crate::geo::GeoProvider,
) -> Result<(), sqlx::Error> {
    let values = aggregate_values(tx, snapshot, received_at, local_head, delta, provider).await?;
    for family in [AggregateFamily::FiveMinute, AggregateFamily::Hourly] {
        let bucket = bucket_start(received_at, family.seconds())?;
        upsert_family(tx, family, node_id, &bucket, &values).await?;
    }
    Ok(())
}

async fn load_family(
    pool: &AnyPool,
    node_id: &str,
    family: AggregateFamily,
    limit: i64,
) -> Result<Vec<PeerAggregateRow>, sqlx::Error> {
    let db_rows = sqlx::query_as::<_, DbAggregateRow>(&family.history_select_sql())
        .bind(node_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    attach_countries(pool, node_id, family, db_rows).await
}

/// Attach the retained country counts of every bucket a read returned.
async fn attach_countries(
    pool: &AnyPool,
    node_id: &str,
    family: AggregateFamily,
    db_rows: Vec<DbAggregateRow>,
) -> Result<Vec<PeerAggregateRow>, sqlx::Error> {
    if db_rows.is_empty() {
        return Ok(Vec::new());
    }

    let bucket_placeholders = vec!["?"; db_rows.len()].join(",");
    let country_sql = crate::database::number_placeholders(&format!(
        "{}? AND bucket_start IN ({bucket_placeholders}) ORDER BY bucket_start DESC, country_code",
        family.country_select_prefix()
    ));
    let mut country_query = sqlx::query_as::<_, (String, String, i64)>(&country_sql).bind(node_id);
    for row in &db_rows {
        country_query = country_query.bind(&row.bucket_start);
    }
    let country_rows = country_query.fetch_all(pool).await?;
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

/// The grain one bounded Peer receipt read answers in.
///
/// Both grains are written from the same successful snapshots, so a range read
/// picks exactly one of them: the two tiers keep different horizons, and an
/// answer that mixed them would hide where one of them ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerGrain {
    FiveMinute,
    Hourly,
}

impl PeerGrain {
    /// Parse the \`grain\` query value: \`5m\` or \`1h\`.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "5m" => Some(Self::FiveMinute),
            "1h" => Some(Self::Hourly),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::FiveMinute => "5m",
            Self::Hourly => "1h",
        }
    }

    pub(crate) fn seconds(self) -> i64 {
        self.family().seconds()
    }

    /// How many buckets this grain keeps, so a read that names only a grain
    /// answers the same stretch the fixed history tail does.
    pub(crate) fn tail_limit(self) -> i64 {
        match self {
            Self::FiveMinute => HISTORY_FIVE_MINUTE_LIMIT,
            Self::Hourly => HISTORY_HOURLY_LIMIT,
        }
    }

    fn family(self) -> AggregateFamily {
        match self {
            Self::FiveMinute => AggregateFamily::FiveMinute,
            Self::Hourly => AggregateFamily::Hourly,
        }
    }
}

/// One bounded read of a Node's Peer receipt buckets.
#[derive(Debug, Clone)]
pub(crate) struct PeerRangeRequest {
    pub grain: PeerGrain,
    /// The answered range, half-open: a bucket belongs to it when its start is
    /// at or after \`from\` and before \`to\`.
    pub from: String,
    pub to: String,
    /// Read only buckets older than this cursor, for the next page.
    pub before: Option<String>,
    pub limit: i64,
}

/// What one bounded read of a Node's Peer receipt buckets covers.
#[derive(Debug)]
pub(crate) struct PeerRangeAnswer {
    pub state: String,
    pub freshness: String,
    pub grain: &'static str,
    pub grain_seconds: i64,
    pub from: String,
    pub to: String,
    /// The page, newest bucket first.
    pub buckets: Vec<PeerAggregateRow>,
    /// How many buckets the range holds, whatever the page limit is.
    pub matching: i64,
    /// The oldest and newest bucket the range holds, when it holds any.
    pub first_bucket: Option<String>,
    pub last_bucket: Option<String>,
    /// Aligned buckets the Node's own stretch expects inside the range and the
    /// Server holds none for.
    pub missing_buckets: i64,
    /// \`complete\`, \`partial\`, \`empty\` or \`unavailable\`.
    pub coverage: &'static str,
    pub truncated: bool,
    pub continuation: Option<String>,
}

/// How many buckets the aligned grid of one grain expects inside a half-open
/// range.
///
/// A receipt bucket opens on a UTC boundary of its grain, so the grid covers the
/// range exactly: \`[from, to)\` holds every boundary at or after \`from\` and
/// before \`to\`.
fn expected_buckets(from: &str, to: &str, grain_seconds: i64) -> Option<i64> {
    let from = crate::metric_history::canonical_instant(from)?.unix_timestamp();
    let to = crate::metric_history::canonical_instant(to)?.unix_timestamp();
    if to <= from {
        return Some(0);
    }
    let first = from.div_euclid(grain_seconds) + i64::from(from.rem_euclid(grain_seconds) != 0);
    let last = (to - 1).div_euclid(grain_seconds);
    Some(if last < first { 0 } else { last - first + 1 })
}

/// The instant before which this family's cleanup no longer answers.
///
/// Each tier of Peer aggregates is declared with its own retention, and a
/// zero-day policy keeps every row forever. A range that ends at or before this
/// horizon was released by cleanup, so the Server cannot tell a quiet stretch
/// from a stretch it no longer holds.
async fn retention_horizon(pool: &AnyPool, family: &str) -> Result<Option<String>, sqlx::Error> {
    let days = crate::retention::list_policies(pool)
        .await?
        .into_iter()
        .find(|row| row.family == family)
        .map(|row| row.retention_days)
        .filter(|days| *days > 0);
    Ok(days.map(|days| crate::auth::format_rfc3339(crate::auth::now_utc() - Duration::days(days))))
}

/// How current the newest receipt this grain holds is.
fn freshness_of(latest: Option<&str>) -> String {
    match latest.and_then(crate::auth::parse_rfc3339) {
        None => "unknown".to_owned(),
        Some(value) if (crate::auth::now_utc() - value).whole_seconds().abs() <= 120 => {
            "current".to_owned()
        }
        Some(_) => "stale".to_owned(),
    }
}

/// The component state the Server reports, falling back to what a read's own
/// rows show: a retained all-empty snapshot is \`empty\`, rows without any
/// reported status are \`ok\`, and a current collection error stays \`error\`
/// while the last-good rows remain visible to the caller.
fn collection_state(
    component_state: Option<&str>,
    current_peer_count: Option<i64>,
    rows: &[&PeerAggregateRow],
) -> String {
    let has_rows = !rows.is_empty();
    let all_empty = rows.iter().all(|row| row_is_empty(row));
    match component_state {
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
    .to_owned()
}

/// The Peer component's reported state and, when it reports a successful
/// collection, how many peers the current projection holds.
async fn component_status(
    pool: &AnyPool,
    node_id: &str,
) -> Result<(Option<String>, Option<i64>), sqlx::Error> {
    let status: Option<(String, i64)> = sqlx::query_as(
        "SELECT state, value_revision FROM component_status WHERE node_id=$1 AND component_key='peers'",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?;
    let state = status.as_ref().map(|(state, _)| state.clone());
    let current_peer_count = match status.as_ref() {
        Some((state, value_revision)) if state == "ok" && *value_revision > 0 => Some(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM current_node_peers WHERE node_id=$1",
            )
            .bind(node_id)
            .fetch_one(pool)
            .await?,
        ),
        _ => None,
    };
    Ok((state, current_peer_count))
}

/// Read one bounded stretch of a Node's Peer receipt buckets.
///
/// The two tiers keep a bounded horizon, so a range wider than a tier holds is
/// answered from the buckets that exist and the shortfall is disclosed instead
/// of hidden: the aligned grid of the grain says how many receipts the range
/// expects, and the peer engine's own rule — a bucket exists only when a Report
/// carried Peer evidence — makes each one of them a receipt the Server never
/// received. The range's start is raised to the oldest bucket the grain still
/// holds, because the stretch before a Node's first successful snapshot is not a
/// hole; its end is not, because silence after the newest receipt is. A range
/// the tier's own retention has already released is answered as unavailable
/// rather than as an empty era, because the Server no longer holds those
/// receipts and cannot prove the quiet.
pub(crate) async fn load_range(
    pool: &AnyPool,
    node_id: &str,
    request: &PeerRangeRequest,
) -> Result<PeerRangeAnswer, sqlx::Error> {
    let family = request.grain.family();
    let grain_seconds = family.seconds();
    let (component_state, current_peer_count) = component_status(pool, node_id).await?;

    let (matching, first_bucket, last_bucket): (i64, Option<String>, Option<String>) =
        sqlx::query_as(&format!(
            "SELECT COUNT(*), MIN(bucket_start), MAX(bucket_start) FROM {} \
              WHERE node_id = $1 AND bucket_start >= $2 AND bucket_start < $3",
            family.table()
        ))
        .bind(node_id)
        .bind(&request.from)
        .bind(&request.to)
        .fetch_one(pool)
        .await?;

    let (ever_min, ever_max, latest): (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as(&format!(
            "SELECT MIN(bucket_start), MAX(bucket_start), MAX(last_observed_at) FROM {} \
              WHERE node_id = $1",
            family.table()
        ))
        .bind(node_id)
        .fetch_one(pool)
        .await?;

    let mut page_sql = format!(
        "SELECT {} FROM {} WHERE node_id = ? AND bucket_start >= ? AND bucket_start < ?",
        family.history_columns(),
        family.table()
    );
    if request.before.is_some() {
        page_sql.push_str(" AND bucket_start < ?");
    }
    page_sql.push_str(" ORDER BY bucket_start DESC LIMIT ?");
    let page_sql = crate::database::number_placeholders(&page_sql);
    let mut page = sqlx::query_as::<_, DbAggregateRow>(&page_sql)
        .bind(node_id)
        .bind(&request.from)
        .bind(&request.to);
    if let Some(before) = request.before.as_deref() {
        page = page.bind(before);
    }
    let limit = request.limit.max(1);
    let mut db_rows = page.bind(limit + 1).fetch_all(pool).await?;
    let truncated = db_rows.len() as i64 > limit;
    if truncated {
        db_rows.truncate(limit as usize);
    }
    let buckets = attach_countries(pool, node_id, family, db_rows).await?;
    let continuation = truncated
        .then(|| buckets.last().map(|row| row.bucket_start.clone()))
        .flatten();

    // The grid starts where the Node's own history does, so the era before its
    // first receipt is a boundary the answer names rather than a hole it counts.
    let grid_from = match ever_min.as_deref() {
        Some(ever_min) if ever_min > request.from.as_str() => Some(ever_min),
        _ => Some(request.from.as_str()),
    };
    let missing_buckets = match (
        ever_min.is_some(),
        grid_from.and_then(|grid_from| expected_buckets(grid_from, &request.to, grain_seconds)),
    ) {
        (false, _) => 0,
        (true, Some(expected)) => (expected - matching).max(0),
        (true, None) => 0,
    };
    // Whether the Node's own record reaches both edges of the range: the range
    // starts at or after the first bucket ever recorded, and its newest bucket is
    // the last aligned bucket the range expects. A range that runs past either
    // edge asks about a stretch the record does not cover, which is partial
    // coverage of that range even when every bucket inside it is present.
    let held_whole_range = match (ever_min.as_deref(), ever_max.as_deref()) {
        (Some(ever_min), Some(ever_max)) => {
            request.from.as_str() >= ever_min
                && expected_buckets(ever_max, &request.to, grain_seconds)
                    .is_some_and(|expected| expected <= 1)
        }
        _ => false,
    };
    // Coverage is about the aligned buckets the Node's own record owes for the
    // range, in the vocabulary the investigation window uses. A shortfall is
    // `partial` even when it leaves the page with no row at all, because a gap
    // inside the record is receipts that never arrived, not an empty window; a
    // range that ends before the record begins stays `empty` for the same
    // reason that era is not a hole the Node reported. A range cleanup has
    // already released is neither: the receipts are gone, so the honest answer
    // is that the Server cannot answer that stretch of the grain.
    let released_before = retention_horizon(pool, family.table()).await?;
    let coverage = match (ever_min.as_deref(), ever_max.as_deref()) {
        (None, _) => "unavailable",
        _ if released_before
            .as_deref()
            .is_some_and(|horizon| request.to.as_str() <= horizon) =>
        {
            "unavailable"
        }
        (Some(ever_min), _) if request.to.as_str() <= ever_min => "empty",
        _ if missing_buckets > 0 || !held_whole_range => "partial",
        _ if matching == 0 => "empty",
        _ => "complete",
    };

    Ok(PeerRangeAnswer {
        state: collection_state(component_state.as_deref(), current_peer_count, &[]),
        freshness: freshness_of(latest.as_deref()),
        grain: request.grain.as_str(),
        grain_seconds,
        from: request.from.clone(),
        to: request.to.clone(),
        buckets,
        matching,
        first_bucket,
        last_bucket,
        missing_buckets,
        coverage,
        truncated,
        continuation,
    })
}

fn row_is_empty(row: &PeerAggregateRow) -> bool {
    row.total_peers == 0
}

/// Read bounded aggregate history and preserve the component's explicit
/// state/freshness dimensions. A retained empty snapshot is `empty`; missing
/// history is `unknown`; a current collection error remains `error` while the
/// last-good aggregate rows stay available to the caller.
pub(crate) async fn load_history(
    pool: &AnyPool,
    node_id: &str,
) -> Result<PeerHistory, sqlx::Error> {
    let (component_state, current_peer_count) = component_status(pool, node_id).await?;
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
        .max();
    let freshness = freshness_of(latest);

    let rows: Vec<&PeerAggregateRow> = five_minute.iter().chain(hourly.iter()).collect();
    let state = collection_state(component_state.as_deref(), current_peer_count, &rows);

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

    #[cfg_attr(storage_postgres, ignore = "it stages a historical SQLite schema")]
    #[tokio::test]
    async fn retained_last_good_countries_resolve_beyond_the_ttl_but_within_retention() {
        if !crate::test_support::skip_unless_sqlite("it stages a historical SQLite schema") {
            return;
        }
        let pool = crate::test_support::unmigrated_sqlite_pool(None).await;
        sqlx::query(crate::geo::CACHE_TABLE_DDL)
            .execute(&pool)
            .await
            .unwrap();
        let insert = |ip: &str, created_at: &str, expires_at: &str| {
            let ip = ip.to_owned();
            let created_at = created_at.to_owned();
            let expires_at = expires_at.to_owned();
            let pool = pool.clone();
            async move {
                sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('local_mmdb', $1, 'US', 'current', $2, $3, $4, $5, $6)")
                    .bind(ip)
                    .bind(&created_at)
                    .bind(&created_at)
                    .bind(&created_at)
                    .bind(&created_at)
                    .bind(expires_at)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        };
        // Expired last-good inside the 30-day retention boundary.
        insert("8.8.4.4", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z").await;
        // Past the hard retention boundary: no longer retainable.
        insert("8.8.8.8", "2025-11-01T00:00:00Z", "2025-11-02T00:00:00Z").await;

        let mut tx = pool.begin().await.unwrap();
        // One lookup resolves every distinct address the snapshot names: the
        // retained last-good result of the selected provider, and nothing else.
        let countries = countries_for_ips(
            &mut tx,
            &[
                Some("8.8.4.4"),
                Some("8.8.8.8"),
                Some("10.0.0.1"),
                Some("8.8.4.4"),
                None,
            ],
            "2026-01-05T00:00:00Z",
            crate::geo::GeoProvider::LocalMmdb,
        )
        .await
        .unwrap();
        assert_eq!(
            countries.get("8.8.4.4").map(String::as_str),
            Some("US"),
            "a retained last-good result resolves beyond its TTL"
        );
        assert_eq!(
            countries.len(),
            1,
            "an expired-by-retention address, a non-public address and a missing address never resolve a country"
        );
        let other_provider = countries_for_ips(
            &mut tx,
            &[Some("8.8.4.4")],
            "2026-01-05T00:00:00Z",
            crate::geo::GeoProvider::Disabled,
        )
        .await
        .unwrap();
        assert!(
            other_provider.is_empty(),
            "another provider's retained result never resolves a country"
        );
        tx.commit().await.unwrap();
    }

    #[test]
    fn lag_stats_merge_without_inventing_values() {
        assert_eq!(add_min(None, None), None);
        assert_eq!(add_min(None, Some(4)), Some(4));
        assert_eq!(add_min(Some(8), Some(4)), Some(4));
        assert_eq!(add_max(Some(8), Some(4)), Some(8));
    }
}
