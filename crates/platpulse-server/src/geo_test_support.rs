//! Test-only Geo helpers the crate's suites share.
//!
//! Both the Backfill and the Refresh suite read a Network's country projection
//! through the same public read model the endpoints use, so the wrapper lives
//! here once instead of in each test module: a change to how a Network read is
//! projected must reach every suite that asserts on it.

use crate::http::AppState;
use crate::http::public::PublicGeoInsight;
use crate::public_projection::{GeoScope, PublicGeoReadings};

/// The Network-level Public country projection, read through the same Geo read
/// model the public endpoints use.
pub(crate) async fn network_geo_insight(state: &AppState, network_key: &str) -> PublicGeoInsight {
    let scope = GeoScope::Network(network_key);
    geo_readings(state, Some(scope)).await.insight(scope)
}

/// One Geo read over the state's database with its current provider and
/// database status, judged now, as a public handler reads it.
pub(crate) async fn geo_readings(
    state: &AppState,
    scope: Option<GeoScope<'_>>,
) -> PublicGeoReadings {
    PublicGeoReadings::load(
        state.db(),
        state.geo_config().provider,
        state.geo_status(),
        scope,
        crate::auth::now_utc(),
    )
    .await
}
