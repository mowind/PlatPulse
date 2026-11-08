//! The Public answer for one Node's Peer history: the retained five-minute
//! and hourly aggregates, with Geo centroids for the countries they name.

use crate::http::public::{
    PublicCountryCount, PublicPeerAggregate, PublicPeerHistory, PublicPeerLagSummary,
};

fn public_peer_aggregate(row: crate::peer_history::PeerAggregateRow) -> PublicPeerAggregate {
    let average_peers =
        (row.sample_count > 0).then(|| row.total_peers as f64 / row.sample_count as f64);
    let average_lag =
        (row.cbft_lag_count > 0).then(|| row.cbft_lag_sum as f64 / row.cbft_lag_count as f64);
    PublicPeerAggregate {
        bucket_start: row.bucket_start,
        last_observed_at: row.last_observed_at,
        sample_count: row.sample_count,
        total_peers: row.total_peers,
        average_peers,
        inbound_count: row.inbound_count,
        outbound_count: row.outbound_count,
        trusted_count: row.trusted_count,
        static_count: row.static_count,
        consensus_count: row.consensus_count,
        known_country_count: row.known_country_count,
        unknown_country_count: row.unknown_country_count,
        countries: row
            .countries
            .into_iter()
            .map(|country| {
                let (centroid_lat, centroid_lon) =
                    crate::geo::country_centroid(&country.country_code);
                PublicCountryCount {
                    country_code: country.country_code,
                    count: country.count,
                    // Aggregate buckets record which countries were
                    // resolvable at sample time; they do not track staleness.
                    stale_count: 0,
                    centroid_lat,
                    centroid_lon,
                }
            })
            .collect(),
        arrivals: row.arrivals,
        departures: row.departures,
        cbft_lag: PublicPeerLagSummary {
            sample_count: row.cbft_lag_count,
            minimum: row.cbft_lag_min,
            average: average_lag,
            maximum: row.cbft_lag_max,
        },
    }
}

pub(super) fn public_peer_history(history: crate::peer_history::PeerHistory) -> PublicPeerHistory {
    PublicPeerHistory {
        state: history.state,
        freshness: history.freshness,
        five_minute: history
            .five_minute
            .into_iter()
            .map(public_peer_aggregate)
            .collect(),
        hourly: history
            .hourly
            .into_iter()
            .map(public_peer_aggregate)
            .collect(),
    }
}
