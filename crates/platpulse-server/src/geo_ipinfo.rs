//! The IPinfo country lookup boundary.
//!
//! IPinfo was the first Geo Provider that resolves countries outside this
//! Server, and it keeps exactly one fixed HTTPS destination (issue #134):
//! the legacy `{base}/{ip}/json` path the Owner approved - never the Lite
//! API, never a token, and never a destination an Admin request or an Agent
//! report can influence.
//!
//! The outbound mechanism itself (eligibility, redirects, timeout, body
//! bound, rate-limit backoff, country-code filter) is shared with every
//! other External Geo Provider and lives in `crate::geo_external`. This
//! module owns only what is IPinfo-specific: the destination shape, the
//! `country` field, the bounds, the stable diagnostic reasons, and the
//! attribution.
//!
//! Nothing in this module is reachable from report ingestion: the background
//! path in `crate::geo_backfill` is the only caller, and a lookup never runs
//! inside a receipt transaction.

use std::sync::OnceLock;
use std::time::Duration;

use crate::geo::GeoProvider;
use crate::geo_external::ExternalProfile;

/// The fixed legacy endpoint base. `{base}/{ip}/json` is the Komari-style
/// legacy path this phase was approved for; the Lite endpoint
/// (`api.ipinfo.io/lite/...`) is deliberately not used.
pub const IPINFO_ENDPOINT_BASE: &str = "https://ipinfo.io";
/// Upper bound on one outbound lookup. The value is a Server decision, not a
/// provider setting, and it never blocks report ingestion.
pub const IPINFO_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Upper bound on a response body that may be interpreted as a country
/// result. Anything larger is not a legacy country document.
pub const IPINFO_MAX_BODY_BYTES: usize = 8 * 1024;
/// Upper bound on concurrent outbound lookups in one background pass. It is
/// deliberately lower than the local-database bound: an external service is
/// asked less aggressively than a file is read.
pub const IPINFO_MAX_CONCURRENCY: usize = 2;
/// The first bounded backoff after a rate-limit response.
pub const IPINFO_THROTTLE_BACKOFF: Duration = Duration::from_secs(60);
/// The largest bounded backoff a repeated rate limit may reach.
pub const IPINFO_MAX_THROTTLE_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// The stable, path-free explanation the Owner-only diagnostic reports while
/// the provider asks this Server to wait. It contains no address and no
/// request detail, so it can never leak one.
pub const IPINFO_THROTTLE_REASON: &str = "the IPinfo endpoint is rate limiting this Server";
/// The stable, path-free explanation the Owner-only diagnostic reports when
/// the most recent outbound lookup produced no usable result. It names no
/// address, endpoint, or provider error text.
pub const IPINFO_FAILURE_REASON: &str = "the last IPinfo lookup produced no usable country result";
/// The attribution IPinfo's terms require. It is the Public attribution
/// whenever IPinfo is the selected provider.
pub const IPINFO_ATTRIBUTION: &str = "IP address data powered by IPinfo (https://ipinfo.io)";
/// The exact disclosure the Owner-only Settings surface shows before IPinfo
/// can be selected. It is Server-owned, so the endpoint named here and the
/// endpoint the request is built from are the same constant.
pub const IPINFO_DISCLOSURE: &str = "IPinfo asks the fixed HTTPS endpoint https://ipinfo.io/{ip}/json for each observed Peer public IP and keeps only the returned two-letter country code. It carries no token and sends no other Peer data.";

/// The IPinfo outbound profile. It is a compile-time constant, so the
/// production destination cannot be changed by configuration, an Admin
/// request, or an Agent report. `crate::geo_external::PROFILES` is the list
/// the whole Server reads, so this constant is the only statement of what
/// IPinfo is.
pub(crate) static IPINFO_PROFILE: ExternalProfile = ExternalProfile {
    provider: GeoProvider::Ipinfo,
    endpoint_base: IPINFO_ENDPOINT_BASE,
    // The legacy path has no segment between the origin and the address, so
    // the prefix is just the separator itself.
    path_prefix: "/",
    path_suffix: "/json",
    // The legacy document spells the two-letter code "country". Every other
    // field it returns (hostname, city, org, loc, ...) is discarded.
    country_field: "country",
    attribution: IPINFO_ATTRIBUTION,
    disclosure: IPINFO_DISCLOSURE,
    throttle_reason: IPINFO_THROTTLE_REASON,
    failure_reason: IPINFO_FAILURE_REASON,
    request_timeout: IPINFO_REQUEST_TIMEOUT,
    max_body_bytes: IPINFO_MAX_BODY_BYTES,
    max_concurrency: IPINFO_MAX_CONCURRENCY,
    throttle_backoff: IPINFO_THROTTLE_BACKOFF,
    max_throttle_backoff: IPINFO_MAX_THROTTLE_BACKOFF,
    http: OnceLock::new(),
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::GeoLookup;
    use crate::geo_external::ExternalGeoClient;
    use crate::geo_external::stub::{StubReply, StubServer};
    use std::net::IpAddr;

    const NOW: &str = "2026-08-12T10:00:00Z";
    /// A globally routable fixture address. It is never sent to the real
    /// service: every test points the client at the loopback stub.
    const FIXTURE: &str = "89.160.20.112";

    fn stub_client(stub: &StubServer) -> ExternalGeoClient {
        ExternalGeoClient::for_tests(&IPINFO_PROFILE, stub.base_url(), Duration::from_secs(2))
    }

    fn ip() -> IpAddr {
        FIXTURE.parse().unwrap()
    }

    /// What is IPinfo-specific: the legacy `{ip}/json` path, the `country`
    /// field, no credentials, and the credit its terms require. The shareable
    /// mechanism (redirects, timeouts, size bound, 429 backoff, ineligible
    /// input) is proven once for every profile in `crate::geo_external`.
    #[tokio::test]
    async fn reads_the_legacy_country_field_without_credentials() {
        let stub = StubServer::start(vec![StubReply::country("se")]).await;
        let client = stub_client(&stub);
        assert_eq!(
            client.resolve(ip(), NOW).await,
            GeoLookup::Country("SE".to_owned())
        );
        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, format!("/{FIXTURE}/json"));
        // The legacy path carries no token, no query, and no credentials.
        assert!(requests[0].query.is_empty());
        assert!(!requests[0].authorization);
    }

    /// IPinfo's own empty answers, as opposed to GeoJS's missing keys: an
    /// empty string and an explicit null are both authoritative NoCountry.
    #[tokio::test]
    async fn an_empty_or_absent_country_is_an_authoritative_no_country() {
        for reply in [
            StubReply::Json(r#"{"ip":"89.160.20.112","country":""}"#.to_owned()),
            StubReply::Json(r#"{"ip":"89.160.20.112"}"#.to_owned()),
            StubReply::Json(r#"{"ip":"89.160.20.112","country":null}"#.to_owned()),
        ] {
            let stub = StubServer::start(vec![reply]).await;
            assert_eq!(
                stub_client(&stub).resolve(ip(), NOW).await,
                GeoLookup::NoCountry
            );
        }
    }

    #[tokio::test]
    async fn an_invalid_country_code_is_a_failure_not_a_no_country_result() {
        for body in [
            r#"{"country":"XYZ"}"#,
            r#"{"country":"S"}"#,
            r#"{"country":"1A"}"#,
            r#"{"country":12}"#,
        ] {
            let stub = StubServer::start(vec![StubReply::document(body)]).await;
            assert_eq!(
                stub_client(&stub).resolve(ip(), NOW).await,
                GeoLookup::Unavailable,
                "{body} must not become a country or a retained no-country"
            );
        }
    }

    #[test]
    fn the_production_destination_is_fixed_and_https() {
        assert_eq!(IPINFO_ENDPOINT_BASE, "https://ipinfo.io");
        assert!(!GeoProvider::Ipinfo.needs_local_database());
        assert!(GeoProvider::Ipinfo.sends_peer_addresses());
        assert_eq!(GeoProvider::Ipinfo.attribution(), Some(IPINFO_ATTRIBUTION));
        assert!(IPINFO_ATTRIBUTION.contains("IPinfo"));
        assert_eq!(
            GeoProvider::Ipinfo
                .external_profile()
                .map(|profile| profile.country_field),
            Some("country")
        );
        // GeoJS is a separate destination whose profile spells the code
        // differently; the two must never share a field name.
        assert_ne!(
            GeoProvider::GeoJs
                .external_profile()
                .map(|profile| profile.country_field),
            Some("country")
        );
    }
}
