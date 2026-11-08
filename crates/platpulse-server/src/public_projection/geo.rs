//! The country projection of the Peer records of the listed Active Nodes:
//! each Node's compact buckets, the Network Geo Insight and the Node Peer
//! Country View, all from one read per response.

use time::OffsetDateTime;

use crate::database::ServerDatabase;
use crate::geo::GeoProvider;
use crate::http::public::{PublicCountryCount, PublicGeoInsight, PublicNodePeerCountries};

use super::ACTIVE_NODE;

pub(crate) const PUBLIC_GEO_ERROR: &str = "Country data is currently unavailable";

pub(super) fn unknown_public_geo_insight() -> PublicGeoInsight {
    PublicGeoInsight {
        state: "disabled".to_owned(),
        last_good_at: None,
        database_age_seconds: None,
        stale_since: None,
        error_reason: None,
        scope: "unavailable".to_owned(),
        known_country_count: None,
        unknown_country_count: None,
        available_peer_count: None,
        unknown_without_remote_ip_count: None,
        unknown_with_public_ip_count: None,
        countries: None,
        attribution: None,
    }
}

pub(super) fn unknown_public_node_peer_countries() -> PublicNodePeerCountries {
    PublicNodePeerCountries {
        scope: "unavailable".to_owned(),
        known_country_count: None,
        unknown_country_count: None,
        available_peer_count: None,
        unknown_without_remote_ip_count: None,
        unknown_with_public_ip_count: None,
        countries: Vec::new(),
    }
}

fn geo_timing(
    status: &crate::geo::GeoStatus,
    now: OffsetDateTime,
) -> (Option<u64>, Option<String>) {
    let Some(build_epoch) = status.build_epoch else {
        return (None, None);
    };
    let Some(build_epoch_seconds) = i64::try_from(build_epoch).ok() else {
        return (None, None);
    };
    let age_seconds = now
        .unix_timestamp()
        .saturating_sub(build_epoch_seconds)
        .max(0) as u64;
    let stale_since = build_epoch
        .checked_add(crate::geo::DATABASE_MAX_AGE.as_secs())
        .and_then(|epoch| i64::try_from(epoch).ok())
        .filter(|epoch| now.unix_timestamp() >= *epoch)
        .and_then(|epoch| time::OffsetDateTime::from_unix_timestamp(epoch).ok())
        .map(crate::auth::format_rfc3339);
    (Some(age_seconds), stale_since)
}

/// One Network's country projection over the current Peer records of its
/// Active Nodes. Every bucket is derived from the same record set, so
/// Known + Unknown always equals `available_peer_count`.
#[derive(Debug, Default, Clone, Copy)]
struct CountryBucket {
    count: i64,
    stale_count: i64,
}

#[derive(Debug, Default)]
struct GeoDistribution {
    countries: std::collections::BTreeMap<String, CountryBucket>,
    known_country_count: i64,
    unknown_country_count: i64,
    unknown_without_remote_ip_count: i64,
    active_node_count: i64,
    observed_node_count: i64,
}

impl GeoDistribution {
    fn available_peer_count(&self) -> i64 {
        self.known_country_count + self.unknown_country_count
    }

    fn scope(&self) -> &'static str {
        if self.observed_node_count == 0 {
            "unobserved"
        } else if self.observed_node_count < self.active_node_count {
            "partial"
        } else {
            "complete"
        }
    }
}

/// The scope whose current Peer records a Public Geo Insight projects. The
/// Network scope covers every Active Node of one Network; the Node scope is
/// the Node Peer Country View over exactly one Node.
#[derive(Debug, Clone, Copy)]
pub(crate) enum GeoScope<'a> {
    Network(&'a str),
    Node(&'a str),
}

impl<'a> GeoScope<'a> {
    /// The `nodes` column this scope filters on and its bound key. Both are
    /// fixed fragments, never request input, so the two scopes share one SQL
    /// shape and differ only in the column they filter.
    fn filter(self) -> (&'static str, &'a str) {
        match self {
            GeoScope::Network(network_key) => ("n.network_key", network_key),
            GeoScope::Node(node_id) => ("n.node_id", node_id),
        }
    }
}

/// One Active Node's country buckets inside a [`PublicGeoReadings`] map.
#[derive(Debug)]
struct PublicNodeGeo {
    network_key: String,
    distribution: GeoDistribution,
}

/// Every in-scope country bucket plus the shared Geo database status, read
/// once per response. One set of rows feeds all three readings: each Node's
/// compact `PublicNodePeerCountries`, the Network-level Geo Insight and the
/// Node Peer Country View, so the Network reading is by construction the sum
/// of its Active Node buckets and a Node reading is that Node's own bucket.
pub(crate) struct PublicGeoReadings {
    /// The selected provider, which owns the attribution its terms require.
    provider: GeoProvider,
    status: crate::geo::GeoStatus,
    database_age_seconds: Option<u64>,
    stale_since: Option<String>,
    /// `None` when Geo is Disabled or the read failed: no scope has a basis.
    nodes: Option<std::collections::BTreeMap<String, PublicNodeGeo>>,
}

impl PublicGeoReadings {
    /// `scope` is `None` for every Active Node, which the Network list needs
    /// so each listed Node can carry its own buckets; a scoped caller (the Node
    /// Detail handler, or a test projecting one scope) passes `Some`.
    pub(crate) async fn load(
        db: &ServerDatabase,
        provider: GeoProvider,
        status: crate::geo::GeoStatus,
        scope: Option<GeoScope<'_>>,
        at: OffsetDateTime,
    ) -> Self {
        let (database_age_seconds, stale_since) = geo_timing(&status, at);
        let readings = Self {
            provider,
            status,
            database_age_seconds,
            stale_since,
            nodes: None,
        };
        if readings.status.state == "disabled" {
            return readings;
        }
        let now = crate::auth::format_rfc3339(at);
        match load_node_geo(db, provider, scope, &now).await {
            Some(nodes) => Self {
                nodes: Some(nodes),
                ..readings
            },
            // A failed read keeps the database status and timing, and leaves
            // every scope without a basis.
            None => readings,
        }
    }

    /// Restrict the reading to exactly the Nodes a response lists, keyed by the
    /// Network each one was listed under. The listed rows and this Geo batch
    /// are two separate reads, so a Node that changed lifecycle in between must
    /// neither enter a Network Geo Insight whose response carries no bucket for
    /// it (an extra batch Node) nor vanish from one whose response does list it
    /// (a Node this batch lost). A listed Node the batch no longer knows keeps
    /// its place in the Active Node count with no observed reading of its own,
    /// so that Network can only read `partial` and never `complete`, and its
    /// own compact bucket carries no basis — which is what the browser computes
    /// from the buckets it was handed, so summing them still reproduces the
    /// reading exactly.
    pub(crate) fn restrict_to_listed_nodes(
        &mut self,
        listed: &std::collections::BTreeMap<String, String>,
    ) {
        let Some(nodes) = self.nodes.as_mut() else {
            return;
        };
        nodes.retain(|node_id, _| listed.contains_key(node_id));
        for (node_id, network_key) in listed {
            nodes
                .entry(node_id.clone())
                .or_insert_with(|| PublicNodeGeo {
                    network_key: network_key.clone(),
                    distribution: GeoDistribution {
                        active_node_count: 1,
                        ..GeoDistribution::default()
                    },
                });
        }
    }

    /// Sum the in-scope Node buckets. A scope with no Active Node at all sums
    /// to an empty distribution, which is `unobserved` rather than a zero.
    ///
    /// A Node whose own scope is `unobserved` has no reliable denominator, so
    /// its rows are not a country count at that Node either; it keeps its
    /// place in the Active Node count (a Network with one such Node is still
    /// `partial`, never `complete`) but contributes no Known, Unknown or
    /// country bucket. The Network reading is therefore exactly the sum of the
    /// compact Node buckets the same response hands to the browser, which is
    /// what makes the unfiltered reading and a selection that matches every
    /// Node one reading. Ingestion writes `current_node_peers` rows only
    /// inside the transaction that records a successful Peer Snapshot, so a
    /// Node with rows and no accepted value is not a state the write path
    /// produces; the gate still keeps both readings equal there instead of
    /// reporting a country the Node's own reading denies.
    fn distribution(&self, scope: GeoScope<'_>) -> GeoDistribution {
        let mut total = GeoDistribution::default();
        let Some(nodes) = self.nodes.as_ref() else {
            return total;
        };
        for (node_id, node_geo) in nodes {
            let in_scope = match scope {
                GeoScope::Network(network_key) => node_geo.network_key == network_key,
                GeoScope::Node(id) => node_id == id,
            };
            if !in_scope {
                continue;
            }
            let distribution = &node_geo.distribution;
            total.active_node_count += distribution.active_node_count;
            if distribution.scope() == "unobserved" {
                continue;
            }
            total.observed_node_count += distribution.observed_node_count;
            total.known_country_count += distribution.known_country_count;
            total.unknown_country_count += distribution.unknown_country_count;
            total.unknown_without_remote_ip_count += distribution.unknown_without_remote_ip_count;
            for (country_code, bucket) in &distribution.countries {
                let entry = total.countries.entry(country_code.clone()).or_default();
                entry.count += bucket.count;
                entry.stale_count += bucket.stale_count;
            }
        }
        total
    }

    pub(crate) fn insight(&self, scope: GeoScope<'_>) -> PublicGeoInsight {
        if self.status.state == "disabled" {
            return unknown_public_geo_insight();
        }
        if self.nodes.is_none() {
            return public_geo_error_insight(
                &self.status,
                self.database_age_seconds,
                self.stale_since.clone(),
            );
        }
        public_geo_insight(
            self.provider,
            &self.status,
            self.database_age_seconds,
            self.stale_since.clone(),
            &self.distribution(scope),
        )
    }

    /// One Node's compact buckets. A Node outside the scope, or one whose
    /// scope has no reliable denominator, projects nothing at all.
    pub(crate) fn node_peer_countries(&self, node_id: &str) -> PublicNodePeerCountries {
        let Some(node_geo) = self.nodes.as_ref().and_then(|nodes| nodes.get(node_id)) else {
            return unknown_public_node_peer_countries();
        };
        node_peer_countries(&node_geo.distribution)
    }
}

fn public_geo_error_insight(
    status: &crate::geo::GeoStatus,
    database_age_seconds: Option<u64>,
    stale_since: Option<String>,
) -> PublicGeoInsight {
    PublicGeoInsight {
        state: "error".to_owned(),
        last_good_at: status.loaded_at.clone(),
        database_age_seconds,
        stale_since,
        // Loader errors can contain filesystem/parser details. Public
        // responses expose only a stable, non-sensitive explanation.
        error_reason: Some(PUBLIC_GEO_ERROR.to_owned()),
        scope: "unavailable".to_owned(),
        known_country_count: None,
        unknown_country_count: None,
        available_peer_count: None,
        unknown_without_remote_ip_count: None,
        unknown_with_public_ip_count: None,
        countries: None,
        attribution: None,
    }
}

fn public_geo_insight(
    provider: GeoProvider,
    status: &crate::geo::GeoStatus,
    database_age_seconds: Option<u64>,
    stale_since: Option<String>,
    distribution: &GeoDistribution,
) -> PublicGeoInsight {
    let scope = distribution.scope();
    // Without a single successful Peer Snapshot there is no reliable
    // denominator: never-observed is not a country count of zero.
    let has_basis = scope != "unobserved";
    let geo_error = status.state == "error";
    PublicGeoInsight {
        state: status.state.clone(),
        last_good_at: status.loaded_at.clone(),
        database_age_seconds,
        stale_since,
        error_reason: geo_error.then(|| PUBLIC_GEO_ERROR.to_owned()),
        scope: scope.to_owned(),
        known_country_count: has_basis.then_some(distribution.known_country_count),
        unknown_country_count: has_basis.then_some(distribution.unknown_country_count),
        available_peer_count: has_basis.then(|| distribution.available_peer_count()),
        unknown_without_remote_ip_count: has_basis
            .then_some(distribution.unknown_without_remote_ip_count),
        unknown_with_public_ip_count: has_basis.then(|| {
            distribution.unknown_country_count - distribution.unknown_without_remote_ip_count
        }),
        countries: has_basis.then(|| public_countries(distribution)),
        // The selected provider owns the attribution its terms require; a
        // provider that resolves locally must never be credited to an
        // external service and vice versa.
        attribution: provider.attribution().map(str::to_owned),
    }
}

fn public_countries(distribution: &GeoDistribution) -> Vec<PublicCountryCount> {
    distribution
        .countries
        .iter()
        .map(|(country_code, bucket)| {
            let (centroid_lat, centroid_lon) = crate::geo::country_centroid(country_code);
            PublicCountryCount {
                country_code: country_code.clone(),
                count: bucket.count,
                stale_count: bucket.stale_count,
                centroid_lat,
                centroid_lon,
            }
        })
        .collect()
}

fn node_peer_countries(distribution: &GeoDistribution) -> PublicNodePeerCountries {
    let scope = distribution.scope();
    let has_basis = scope != "unobserved";
    PublicNodePeerCountries {
        scope: scope.to_owned(),
        known_country_count: has_basis.then_some(distribution.known_country_count),
        unknown_country_count: has_basis.then_some(distribution.unknown_country_count),
        available_peer_count: has_basis.then(|| distribution.available_peer_count()),
        unknown_without_remote_ip_count: has_basis
            .then_some(distribution.unknown_without_remote_ip_count),
        unknown_with_public_ip_count: has_basis.then(|| {
            distribution.unknown_country_count - distribution.unknown_without_remote_ip_count
        }),
        // A Node without a reliable denominator carries no countries at all.
        // The browser gates on the counts, so an empty list is never a zero.
        countries: if has_basis {
            public_countries(distribution)
        } else {
            Vec::new()
        },
    }
}

/// Load the current Peer records in scope grouped by Active Node and country
/// onto the retained country cache. A Peer record is one row of
/// current_node_peers: the same IP seen on two Peers or two Nodes is two
/// records. A row is only retainable inside the hard cache-retention
/// boundary; inside that boundary an expired country stays as explicit
/// last-good Stale data instead of becoming Unknown.
///
/// Only the fixed scope column varies between the Network and Node scopes;
/// everything after the filter — the selected provider, the Peer-record
/// denominator, retention and the Unknown split — is shared. The per-Node
/// denominator is loaded separately, so a Node that never reported a Peer
/// Snapshot stays distinguishable from one whose snapshot was authoritatively
/// empty.
async fn load_node_geo(
    db: &ServerDatabase,
    provider: GeoProvider,
    scope: Option<GeoScope<'_>>,
    now: &str,
) -> Option<std::collections::BTreeMap<String, PublicNodeGeo>> {
    let rebuild_before = crate::geo::cache_rebuild_cutoff(now);
    let filter = match scope {
        Some(scope) => format!(" AND {} = ?", scope.filter().0),
        None => String::new(),
    };
    // Timestamps are canonical RFC 3339 UTC, so the ordering comparisons the
    // cache itself relies on are valid here too. A row outside the hard
    // retention boundary is not retainable at all, and only the selected
    // provider's retained rows are usable: a result left over from another
    // provider is not a country for the current projection.
    let sql = format!(
        "SELECT p.node_id, n.network_key, g.country_code, COUNT(*) AS record_count, CAST(COALESCE(SUM(CASE WHEN g.country_code IS NOT NULL AND g.created_at > ? THEN 1 ELSE 0 END), 0) AS BIGINT) AS known_count, CAST(COALESCE(SUM(CASE WHEN g.country_code IS NOT NULL AND g.created_at > ? AND (g.expires_at IS NULL OR g.expires_at <= ?) THEN 1 ELSE 0 END), 0) AS BIGINT) AS stale_count, CAST(COALESCE(SUM(CASE WHEN g.country_code IS NULL AND p.remote_ip IS NULL THEN 1 ELSE 0 END), 0) AS BIGINT) AS unknown_without_remote_ip_count FROM current_node_peers p JOIN nodes n ON n.node_id = p.node_id LEFT JOIN geo_location_cache g ON g.provider = ? AND g.canonical_ip = p.remote_ip WHERE {ACTIVE_NODE}{filter} GROUP BY p.node_id, n.network_key, g.country_code"
    );
    let sql = crate::database::number_placeholders(&sql);
    let mut rows = sqlx::query_as::<_, (String, String, Option<String>, i64, i64, i64, i64)>(&sql)
        .bind(rebuild_before.as_str())
        .bind(rebuild_before.as_str())
        .bind(now)
        .bind(provider.as_str());
    if let Some(scope) = scope {
        rows = rows.bind(scope.filter().1);
    }
    let rows = rows.fetch_all(db.pool()).await.ok()?;
    // The denominator is one row per Active Node in scope, whether or not it
    // currently has any Peer record.
    let sql = format!(
        "SELECT n.node_id, n.network_key, CAST(COALESCE(SUM(CASE WHEN COALESCE(ps.value_revision, 0) > 0 THEN 1 ELSE 0 END), 0) AS BIGINT) FROM nodes n LEFT JOIN component_status ps ON ps.node_id = n.node_id AND ps.component_key = 'peers' WHERE {ACTIVE_NODE}{filter} GROUP BY n.node_id, n.network_key"
    );
    let sql = crate::database::number_placeholders(&sql);
    let mut denominator = sqlx::query_as::<_, (String, String, i64)>(&sql);
    if let Some(scope) = scope {
        denominator = denominator.bind(scope.filter().1);
    }
    let denominator = denominator.fetch_all(db.pool()).await.ok()?;

    let mut nodes = std::collections::BTreeMap::new();
    for (node_id, network_key, observed) in denominator {
        nodes.insert(
            node_id,
            PublicNodeGeo {
                network_key,
                distribution: GeoDistribution {
                    active_node_count: 1,
                    observed_node_count: i64::from(observed > 0),
                    ..GeoDistribution::default()
                },
            },
        );
    }
    for (
        node_id,
        network_key,
        country_code,
        record_count,
        known_count,
        stale_count,
        unknown_without_remote_ip_count,
    ) in rows
    {
        // A Peer record only exists for an Active Node, so its denominator row
        // is already present; this entry is defensive only.
        let node_geo = nodes.entry(node_id).or_insert_with(|| PublicNodeGeo {
            network_key,
            distribution: GeoDistribution {
                active_node_count: 1,
                ..GeoDistribution::default()
            },
        });
        let distribution = &mut node_geo.distribution;
        distribution.known_country_count += known_count;
        // Every row of a country group that is not retained is one more
        // Unknown record; a group without a country is Unknown entirely.
        distribution.unknown_country_count += record_count - known_count;
        distribution.unknown_without_remote_ip_count += unknown_without_remote_ip_count;
        if known_count > 0 {
            if let Some(country_code) = country_code {
                distribution.countries.insert(
                    country_code,
                    CountryBucket {
                        count: known_count,
                        stale_count,
                    },
                );
            }
        }
    }
    Some(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geo_timing_reports_age_and_stale_boundary() {
        let build_epoch = 1_700_000_000;
        let now = OffsetDateTime::from_unix_timestamp(build_epoch + 31 * 24 * 60 * 60).unwrap();
        let status = crate::geo::GeoStatus {
            state: "stale".to_owned(),
            configured: true,
            build_epoch: Some(build_epoch as u64),
            digest: Some("digest".to_owned()),
            loaded_at: Some("2026-01-01T00:00:00Z".to_owned()),
            last_error: None,
        };
        let (age_seconds, stale_since) = geo_timing(&status, now);
        assert_eq!(age_seconds, Some(31 * 24 * 60 * 60));
        assert_eq!(
            stale_since,
            Some(crate::auth::format_rfc3339(
                OffsetDateTime::from_unix_timestamp(build_epoch + 30 * 24 * 60 * 60,).unwrap(),
            )),
        );
    }
}
