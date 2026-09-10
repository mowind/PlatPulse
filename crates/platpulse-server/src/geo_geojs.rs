//! The GeoJS country lookup boundary.
//!
//! GeoJS is the second External Geo Provider (issue #135). It reuses the
//! outbound mechanism the first one proved - the fixed destination, the
//! public-literal precondition, the redirect, timeout, and body bounds, the
//! bounded provider-wide rate-limit backoff, and the shared country-code
//! filter, all owned by `crate::geo_external` - and adds only what is
//! GeoJS-specific:
//!
//! * the fixed HTTPS destination `https://get.geojs.io/v1/ip/geo/{ip}.json`
//!   from a compile-time profile, so no configuration, Admin request, or
//!   Agent report can move the request;
//! * GeoJS's own field spelling: the two-letter code is `country_code`,
//!   while `country` is the full country *name*. Reading the wrong one would
//!   turn "United States" into a malformed result, so the field name is part
//!   of the profile rather than shared.
//!
//! Every provider difference is adapted at this boundary; the retained
//! cache, its provider key, the dedup rule, the last-good rule, and the Peer
//! record counts downstream are unchanged and shared with every other
//! provider.
//!
//! Two GeoJS details are deliberate and must not be "simplified":
//!
//! * the sibling `/v1/ip/country/{ip}.json` endpoint spells the alpha-2 code
//!   as `country` and answers unknown addresses with empty strings, the
//!   opposite of the `/geo/` endpoint this profile uses. Reading that one
//!   with this profile, or this one with that spelling, silently turns results
//!   into failures or into a bogus empty country;
//! * a `?fields=` parameter is not documented and is silently ignored by the
//!   service, so nothing here narrows the response: the whole document is read
//!   and every field except the country code is discarded at the boundary.
//!
//! Nothing in this module is reachable from report ingestion: the background
//! path in `crate::geo_backfill` is the only caller, and a lookup never runs
//! inside a receipt transaction.

use std::sync::OnceLock;
use std::time::Duration;

use crate::geo::GeoProvider;
use crate::geo_external::ExternalProfile;

/// The fixed GeoJS endpoint base. `{base}/v1/ip/geo/{ip}.json` is the
/// per-address JSON endpoint of the service the Owner approved; the
/// batch and self-lookup endpoints are deliberately not used.
pub const GEOJS_ENDPOINT_BASE: &str = "https://get.geojs.io";
/// Upper bound on one outbound lookup. The value is a Server decision, not a
/// provider setting, and it never blocks report ingestion.
pub const GEOJS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Upper bound on a response body that may be interpreted as a country
/// result. The per-address document is well under a kilobyte; anything
/// larger is not one.
pub const GEOJS_MAX_BODY_BYTES: usize = 8 * 1024;
/// Upper bound on concurrent outbound lookups in one background pass, kept
/// at the external-provider bound: GeoJS is asked no more aggressively than
/// any other third-party service.
pub const GEOJS_MAX_CONCURRENCY: usize = 2;
/// The first bounded backoff after a rate-limit response.
pub const GEOJS_THROTTLE_BACKOFF: Duration = Duration::from_secs(60);
/// The largest bounded backoff a repeated rate limit may reach.
pub const GEOJS_MAX_THROTTLE_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// The stable, path-free explanation the Owner-only diagnostic reports while
/// the provider asks this Server to wait. It contains no address and no
/// request detail, so it can never leak one.
pub const GEOJS_THROTTLE_REASON: &str = "the GeoJS endpoint is rate limiting this Server";
/// The stable, path-free explanation the Owner-only diagnostic reports when
/// the most recent outbound lookup produced no usable result. It names no
/// address, endpoint, or provider error text.
pub const GEOJS_FAILURE_REASON: &str = "the last GeoJS lookup produced no usable country result";
/// The attribution shown wherever GeoJS country results are displayed.
///
/// GeoJS's own terms require no attribution, but it is still a third-party
/// source, so the Public surface names it instead of presenting the data as
/// this Server's own observation. GeoJS states that its GeoIP data comes from
/// the MaxMind GeoLite database, whose licence requires the MaxMind credit
/// below from its licensees; whether that obligation reaches a downstream
/// consumer of GeoJS's API is not something this Server can establish, so the
/// conservative reading is applied and the exact required sentence is kept.
/// It is a stable Server-owned string: the browser never composes one.
pub const GEOJS_ATTRIBUTION: &str = "IP address data from GeoJS (https://get.geojs.io). This product includes GeoLite Data created by MaxMind, available from https://www.maxmind.com.";
/// The exact disclosure the Owner-only Settings surface shows before GeoJS
/// can be selected. It is Server-owned, so the endpoint named here and the
/// endpoint the request is built from are the same constant.
pub const GEOJS_DISCLOSURE: &str = "GeoJS asks the fixed HTTPS endpoint https://get.geojs.io/v1/ip/geo/{ip}.json for each observed Peer public IP and keeps only the returned two-letter country code. It carries no token and sends no other Peer data.";

/// The GeoJS outbound profile. It is a compile-time constant, so the
/// production destination cannot be changed by configuration, an Admin
/// request, or an Agent report. `crate::geo_external::PROFILES` is the list
/// the whole Server reads, so this constant is the only statement of what
/// GeoJS is.
pub(crate) static GEOJS_PROFILE: ExternalProfile = ExternalProfile {
    provider: GeoProvider::GeoJs,
    endpoint_base: GEOJS_ENDPOINT_BASE,
    // `/v1/ip/geo/{ip}.json`: a fixed prefix segment and a fixed suffix.
    path_prefix: "/v1/ip/geo/",
    path_suffix: ".json",
    // GeoJS spells the ISO 3166-1 alpha-2 code `country_code`; its
    // `country` field is the full country name and is never read.
    country_field: "country_code",
    attribution: GEOJS_ATTRIBUTION,
    disclosure: GEOJS_DISCLOSURE,
    throttle_reason: GEOJS_THROTTLE_REASON,
    failure_reason: GEOJS_FAILURE_REASON,
    request_timeout: GEOJS_REQUEST_TIMEOUT,
    max_body_bytes: GEOJS_MAX_BODY_BYTES,
    max_concurrency: GEOJS_MAX_CONCURRENCY,
    throttle_backoff: GEOJS_THROTTLE_BACKOFF,
    max_throttle_backoff: GEOJS_MAX_THROTTLE_BACKOFF,
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
        ExternalGeoClient::for_tests(&GEOJS_PROFILE, stub.base_url(), Duration::from_secs(2))
    }

    fn ip() -> IpAddr {
        FIXTURE.parse().unwrap()
    }

    /// A realistic GeoJS document. Only `country_code` may become a country:
    /// the coordinates, city, and country *name* are discarded at the trust
    /// boundary exactly like IPinfo's extra fields.
    fn geojs_document(country_code: &str) -> StubReply {
        StubReply::document(&format!(
            r#"{{"organization_name":"Example","region":"Ostergotland County","accuracy":0,"asn":12345,"organization":"Example AB","timezone":"Europe/Stockholm","longitude":"15.0000","country_code3":"SWE","area_code":"0","ip":"{FIXTURE}","country":"Sweden","continent_code":"EU","country_code":"{country_code}","latitude":"58.0000","city":"Linkoping"}}"#
        ))
    }

    /// What is GeoJS-specific: the `/v1/ip/geo/{ip}.json` path, the
    /// `country_code` field, the credit, and the shape of a real document.
    /// The shareable mechanism is proven once for every profile in
    /// `crate::geo_external`.
    #[tokio::test]
    async fn reads_the_country_code_field_from_the_fixed_path() {
        let stub = StubServer::start(vec![geojs_document("se")]).await;
        let client = stub_client(&stub);
        assert_eq!(
            client.resolve(ip(), NOW).await,
            GeoLookup::Country("SE".to_owned())
        );
        let requests = stub.requests();
        assert_eq!(requests.len(), 1);
        // The per-address path is the fixed GeoJS one, with no query and no
        // credentials.
        assert_eq!(requests[0].path, format!("/v1/ip/geo/{FIXTURE}.json"));
        assert!(requests[0].query.is_empty());
        assert!(!requests[0].authorization);
    }

    /// The provider difference the ticket calls out: GeoJS returns the full
    /// country *name* in `country`. It must never be read as a country code,
    /// and a valid `country_code` beside it must still win.
    #[tokio::test]
    async fn the_full_country_name_is_never_read_as_a_country_code() {
        let stub = StubServer::start(vec![StubReply::document(&format!(
            r#"{{"ip":"{FIXTURE}","country":"United States","country_code":"US"}}"#
        ))])
        .await;
        assert_eq!(
            stub_client(&stub).resolve(ip(), NOW).await,
            GeoLookup::Country("US".to_owned())
        );

        // A document carrying only the name has no country code at all: that
        // is the provider's own empty answer, not the name reinterpreted.
        let stub = StubServer::start(vec![StubReply::document(&format!(
            r#"{{"ip":"{FIXTURE}","country":"United States"}}"#
        ))])
        .await;
        assert_eq!(
            stub_client(&stub).resolve(ip(), NOW).await,
            GeoLookup::NoCountry
        );
    }

    /// GeoJS omits the country keys entirely for an address it cannot place
    /// (HTTP 200, no `country_code` key), which the shared boundary reads as
    /// the authoritative empty answer.
    #[tokio::test]
    async fn an_empty_or_absent_country_code_is_an_authoritative_no_country() {
        for reply in [
            geojs_document(""),
            StubReply::document(&format!(r#"{{"ip":"{FIXTURE}"}}"#)),
            StubReply::document(&format!(r#"{{"ip":"{FIXTURE}","country_code":null}}"#)),
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
            r#"{"country_code":"USA"}"#,
            r#"{"country_code":"S"}"#,
            r#"{"country_code":"1A"}"#,
            r#"{"country_code":12}"#,
            r#"{"country_code":["US"]}"#,
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
        assert_eq!(GEOJS_ENDPOINT_BASE, "https://get.geojs.io");
        let profile = GeoProvider::GeoJs
            .external_profile()
            .expect("GeoJS has a profile");
        assert_eq!(profile.path_prefix, "/v1/ip/geo/");
        assert_eq!(profile.path_suffix, ".json");
        assert_eq!(profile.country_field, "country_code");
        // The production destination is assembled exactly once, from
        // constants, and stays HTTPS.
        assert_eq!(
            format!(
                "{}{}{}{}",
                profile.endpoint_base, profile.path_prefix, "1.1.1.1", profile.path_suffix
            ),
            "https://get.geojs.io/v1/ip/geo/1.1.1.1.json"
        );
    }

    #[test]
    fn the_provider_is_external_and_owns_its_attribution() {
        assert!(!GeoProvider::GeoJs.needs_local_database());
        assert!(GeoProvider::GeoJs.sends_peer_addresses());
        assert_eq!(GeoProvider::GeoJs.attribution(), Some(GEOJS_ATTRIBUTION));
        assert!(GEOJS_ATTRIBUTION.contains("GeoJS"));
        // The MaxMind sentence GeoLite's licence requires is kept verbatim, so
        // the two credit strings can never drift apart.
        assert!(GEOJS_ATTRIBUTION.contains(crate::geo::MAXMIND_ATTRIBUTION));
    }
}
