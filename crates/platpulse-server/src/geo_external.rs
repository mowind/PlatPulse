//! The shared outbound boundary for every External Geo Provider.
//!
//! A Geo Provider that resolves countries outside this Server owns an
//! outbound contract that must hold identically for each of them (issues
//! #134 and #135):
//!
//! * exactly one fixed HTTPS destination per provider, built from a profile
//!   this Server owns - never a destination an Admin request, a Server
//!   setting, or an Agent report can influence;
//! * only Server-validated canonical public literals are sent; a private,
//!   loopback, link-local, or reserved address is refused here as well as in
//!   the scheduling path, and a hostname is never resolved;
//! * redirects are not followed at all, so a response can never move the
//!   request to another destination;
//! * one bounded timeout, one bounded response size, and one bounded
//!   provider-wide backoff after a rate-limit response;
//! * the provider's own country field is read and the shared two-letter
//!   shape filter decides whether the value may become a retained country.
//!
//! Each provider gets its own client instance, so the path state (the
//! bounded backoff and the latest-outcome flag) is isolated per provider:
//! switching providers never lets provider A's in-flight exchange decide
//! provider B's state.
//!
//! Nothing in this module is reachable from report ingestion: the background
//! path in `crate::geo_backfill` is the only caller, and a lookup never runs
//! inside a receipt transaction.

use std::net::IpAddr;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use crate::geo::{self, GeoLookup, GeoProvider};

impl std::fmt::Debug for ExternalGeoPaths {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalGeoPaths")
            .field(
                "providers",
                &self
                    .paths
                    .iter()
                    .map(|(profile, _)| profile.provider.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// The provider-specific half of the outbound contract. Everything that
/// differs between External Geo Providers lives here; the client below owns
/// the mechanism (HTTP, timeout, redirects, size bound, backoff) and reads
/// only these values.
pub struct ExternalProfile {
    /// The Geo Provider this profile resolves for. It is the durable
    /// `geo_provider` value results are keyed by and the value every
    /// generation check revalidates.
    pub(crate) provider: GeoProvider,
    /// The fixed HTTPS origin. It is a compile-time constant: neither
    /// configuration nor an API can change it.
    pub(crate) endpoint_base: &'static str,
    /// The fixed path prefix between the origin and the address.
    pub(crate) path_prefix: &'static str,
    /// The fixed path suffix after the address (`.json` or `/json`).
    pub(crate) path_suffix: &'static str,
    /// The JSON field that carries the two-letter country code. The
    /// providers spell it differently, so the differences are adapted here
    /// and every other interpretation is shared.
    pub(crate) country_field: &'static str,
    /// The attribution the provider's terms require wherever its country
    /// results are shown.
    pub(crate) attribution: &'static str,
    /// The exact outbound disclosure the Owner-only surface shows *before*
    /// this provider can be selected. It names this provider's own fixed
    /// endpoint, so the destination is stated once, by the Server, beside
    /// the constant the request is really built from; the browser renders
    /// this string and never composes one of its own.
    pub(crate) disclosure: &'static str,
    /// The stable, path-free explanation the Owner-only diagnostic reports
    /// while the provider asks this Server to wait.
    pub(crate) throttle_reason: &'static str,
    /// The stable, path-free explanation the Owner-only diagnostic reports
    /// when the most recent outbound lookup produced no usable result.
    pub(crate) failure_reason: &'static str,
    /// Upper bound on one outbound lookup.
    pub(crate) request_timeout: Duration,
    /// Upper bound on a response body that may be interpreted as a country
    /// result.
    pub(crate) max_body_bytes: usize,
    /// Upper bound on concurrent outbound lookups in one background pass.
    pub(crate) max_concurrency: usize,
    /// The first bounded backoff after a rate-limit response.
    pub(crate) throttle_backoff: Duration,
    /// The largest bounded backoff a repeated rate limit may reach.
    pub(crate) max_throttle_backoff: Duration,
    /// The one connection pool every lookup of this provider shares. It is
    /// per profile, so the production client is built once per provider. It
    /// is left empty at every construction site and filled on first use.
    pub(crate) http: OnceLock<reqwest::Client>,
}

impl ExternalProfile {
    /// The shared production client for this provider.
    fn http(&self) -> reqwest::Client {
        self.http
            .get_or_init(|| {
                reqwest::Client::builder()
                    .timeout(self.request_timeout)
                    // A redirect could move the request to a destination this
                    // Server never approved, so none is followed.
                    .redirect(reqwest::redirect::Policy::none())
                    .user_agent(concat!("PlatPulse/", env!("CARGO_PKG_VERSION")))
                    .build()
                    .expect("the external Geo HTTP client builds")
            })
            .clone()
    }

    /// The exact request destination for one canonical literal. The address
    /// is a literal, so this path is fixed and no DNS lookup ever happens
    /// for provider input.
    /// The prefix carries the separator and any fixed segments, so the
    /// assembled path is exactly the one the provider documents: IPinfo is
    /// `{origin}/{ip}/json` and GeoJS is `{origin}/v1/ip/geo/{ip}.json`.
    fn endpoint(&self, base_url: &str, ip: &IpAddr) -> String {
        format!("{base_url}{}{ip}{}", self.path_prefix, self.path_suffix)
    }
}

/// What this process knows about one provider's outbound path without
/// reading the retained cache. It carries no address and no request detail,
/// only the bounded backoff window and whether the latest attempt worked.
#[derive(Debug, Default)]
struct PathState {
    /// RFC 3339 instant before which no request may be sent. Compared as a
    /// string, like every other retained time in this Server.
    until: Option<String>,
    /// Consecutive rate-limit responses, used for the bounded exponential
    /// backoff. Any successful provider response resets it.
    consecutive: u32,
    /// Whether the most recent outbound attempt produced no usable result.
    /// The Public and Admin surfaces report this as the provider's real
    /// failure state instead of claiming Current over a failing path.
    last_failure: bool,
}

/// One provider's outbound path. It owns the HTTP client, the fixed
/// destination, the bounded provider-wide backoff, and the latest path
/// outcome; it holds no per-address state, so the retained cache stays the
/// single source of country results.
pub struct ExternalGeoClient {
    profile: &'static ExternalProfile,
    http: reqwest::Client,
    base_url: String,
    path: RwLock<PathState>,
}

impl std::fmt::Debug for ExternalGeoClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalGeoClient")
            .field("provider", &self.profile.provider.as_str())
            .field("endpoint", &self.profile.endpoint_base)
            .finish()
    }
}

impl ExternalGeoClient {
    /// The production client: the profile's fixed destination, no
    /// credentials. This is the only constructor the Server binary uses.
    pub fn production(profile: &'static ExternalProfile) -> Self {
        Self {
            http: profile.http(),
            base_url: profile.endpoint_base.to_owned(),
            profile,
            path: RwLock::new(PathState::default()),
        }
    }

    /// A deterministic test double. The loopback endpoint and shortened
    /// timeout exist only in test builds: neither is reachable from Server
    /// configuration, an Admin request, or an Agent report, so the
    /// production destination can never be overridden.
    #[cfg(test)]
    pub(crate) fn for_tests(
        profile: &'static ExternalProfile,
        base_url: &str,
        timeout: Duration,
    ) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("the external Geo test client builds"),
            base_url: base_url.trim_end_matches('/').to_owned(),
            profile,
            path: RwLock::new(PathState::default()),
        }
    }

    pub(crate) fn profile(&self) -> &'static ExternalProfile {
        self.profile
    }

    /// Upper bound on concurrent lookups in one background pass.
    pub(crate) fn max_concurrency(&self) -> usize {
        self.profile.max_concurrency
    }

    /// The instant before which a bounded backoff still forbids outbound
    /// work, or `None` when the provider may be asked again at `now`.
    pub(crate) fn throttled_until(&self, now: &str) -> Option<String> {
        let path = self.path.read().expect("external Geo path lock poisoned");
        let until = path.until.as_deref()?;
        (until > now).then(|| until.to_owned())
    }

    /// Whether the most recent outbound attempt produced no usable result.
    /// It clears on the next successful provider response, so the reported
    /// failure state always describes the latest real exchange.
    pub(crate) fn last_failure(&self) -> bool {
        self.path
            .read()
            .expect("external Geo path lock poisoned")
            .last_failure
    }

    /// Resolve one address through the provider's fixed endpoint.
    ///
    /// The outcomes are deliberately distinct: a document without a country
    /// is an authoritative `NoCountry`, a rate limit is `RateLimited` so the
    /// scheduler backs off instead of recording an attempt, and every
    /// transport, status, size, and parse failure is `Unavailable` so the
    /// caller records only the attempt and never rewrites retained evidence.
    pub(crate) async fn resolve(&self, ip: IpAddr, now: &str) -> GeoLookup {
        if !geo::eligible_public_ip(&ip) {
            return GeoLookup::NoCountry;
        }
        if self.throttled_until(now).is_some() {
            return GeoLookup::RateLimited;
        }
        let outcome = self.request(&ip).await;
        match outcome {
            GeoLookup::Country(_) | GeoLookup::NoCountry => self.note_success(),
            GeoLookup::RateLimited => self.arm_throttle(now),
            GeoLookup::Unavailable => self.note_failure(),
        }
        outcome
    }

    /// One bounded request against the fixed endpoint, with no path state of
    /// its own. Every failure mode collapses to `Unavailable` because the
    /// caller records only the attempt for all of them.
    async fn request(&self, ip: &IpAddr) -> GeoLookup {
        let endpoint = self.profile.endpoint(&self.base_url, ip);
        let response = match self
            .http
            .get(&endpoint)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
        {
            Ok(response) => response,
            // Transport failures and the bounded timeout are one outcome:
            // the provider produced no result and the address stays pending.
            Err(_) => return GeoLookup::Unavailable,
        };
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return GeoLookup::RateLimited;
        }
        if !response.status().is_success() {
            return GeoLookup::Unavailable;
        }
        let body = match response.bytes().await {
            Ok(body) if body.len() <= self.profile.max_body_bytes => body,
            Ok(_) => return GeoLookup::Unavailable,
            Err(_) => return GeoLookup::Unavailable,
        };
        // The document must be a JSON object. A successful response that is
        // not (an HTML error page, a bare array) is malformed and must never
        // become an authoritative "no country".
        let document: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(document) => document,
            Err(_) => return GeoLookup::Unavailable,
        };
        let Some(field) = document.get(self.profile.country_field) else {
            // The provider answered authoritatively for an object without the
            // field: this address has no country, which is a result and not a
            // failure. A non-object document was already refused above.
            return if document.is_object() {
                GeoLookup::NoCountry
            } else {
                GeoLookup::Unavailable
            };
        };
        match field {
            // A present but null field is the provider's own empty answer.
            serde_json::Value::Null => GeoLookup::NoCountry,
            serde_json::Value::String(country) => {
                let country = country.trim();
                if country.is_empty() {
                    // The provider answered authoritatively: this address has
                    // no country, which is a result and not a failure.
                    return GeoLookup::NoCountry;
                }
                let code = country.to_ascii_uppercase();
                if geo::is_country_code(&code) {
                    GeoLookup::Country(code)
                } else {
                    // A present but unusable code is a malformed result, not
                    // an authoritative "no country".
                    GeoLookup::Unavailable
                }
            }
            // Any other JSON type is a malformed country field.
            _ => GeoLookup::Unavailable,
        }
    }

    /// Arm the bounded provider-wide backoff. Every address in the current
    /// pass observes it, so a rate limit costs at most the in-flight requests
    /// plus one window of waiting instead of one retry per Peer address.
    fn arm_throttle(&self, now: &str) {
        let mut path = self.path.write().expect("external Geo path lock poisoned");
        path.consecutive = path.consecutive.saturating_add(1);
        path.last_failure = true;
        let factor = 1u32 << path.consecutive.saturating_sub(1).min(4);
        let backoff = self
            .profile
            .throttle_backoff
            .saturating_mul(factor)
            .min(self.profile.max_throttle_backoff);
        path.until = Some(crate::auth::format_rfc3339(
            crate::auth::parse_rfc3339(now).unwrap_or_else(crate::auth::now_utc)
                + time::Duration::seconds(backoff.as_secs() as i64),
        ));
    }

    /// Record that the latest attempt produced no usable result.
    fn note_failure(&self) {
        self.path
            .write()
            .expect("external Geo path lock poisoned")
            .last_failure = true;
    }

    /// A provider that answered normally is neither failing nor rate limited:
    /// forget both, so a later isolated failure starts from a clean state.
    fn note_success(&self) {
        let mut path = self.path.write().expect("external Geo path lock poisoned");
        path.consecutive = 0;
        path.until = None;
        path.last_failure = false;
    }
}
/// Every implemented External Geo Provider's compile-time profile.
///
/// This is the single list of providers that resolve countries outside this
/// Server: the privacy flag, the attribution, the background pass, and the
/// production outbound paths all derive from it, so adding a provider is one
/// entry here plus its own module. A provider absent from this list cannot
/// be described as external by any surface.
pub(crate) static PROFILES: [&ExternalProfile; 2] = [
    &crate::geo_ipinfo::IPINFO_PROFILE,
    &crate::geo_geojs::GEOJS_PROFILE,
];

/// The compile-time profile of one provider, or `None` when it resolves
/// countries on this Server. The one lookup behind
/// `GeoProvider::external_profile`.
pub(crate) fn profile_of(provider: GeoProvider) -> Option<&'static ExternalProfile> {
    PROFILES
        .iter()
        .copied()
        .find(|profile| profile.provider == provider)
}

/// Every External Geo Provider's outbound path. It is built from
/// [`PROFILES`], so it always holds exactly one entry per external provider,
/// and switching providers switches the whole outbound state (client,
/// connection pool, bounded backoff, latest-outcome flag) at once. An
/// in-flight exchange under provider A therefore can never decide provider
/// B's reported state or backoff window.
#[derive(Clone)]
pub(crate) struct ExternalGeoPaths {
    /// One entry per implemented External Geo Provider. Membership is the
    /// whole answer to "does this provider send addresses off this Server",
    /// so it is kept alongside the one other place that answers that
    /// question (the compile-time profile) and
    /// `every_external_provider_has_exactly_one_path` pins them together.
    paths: [(&'static ExternalProfile, Arc<ExternalGeoClient>); PROFILES.len()],
}

impl ExternalGeoPaths {
    /// The production paths: one client per profile, each bound to that
    /// profile's fixed destination with its own credentials-free connection
    /// pool.
    pub(crate) fn production() -> Self {
        Self {
            paths: PROFILES
                .map(|profile| (profile, Arc::new(ExternalGeoClient::production(profile)))),
        }
    }

    /// The outbound path of one provider, or `None` when the provider does
    /// not resolve outside this Server.
    pub(crate) fn get(&self, provider: GeoProvider) -> Option<&Arc<ExternalGeoClient>> {
        self.paths
            .iter()
            .find(|(profile, _)| profile.provider == provider)
            .map(|(_, client)| client)
    }

    /// Replace one provider's outbound path. Deterministic tests point it at
    /// a loopback stub; a provider that resolves no country outside this
    /// Server has no path to replace, and asking for one is a wiring mistake
    /// rather than a runtime condition.
    pub(crate) fn set(&mut self, provider: GeoProvider, client: Arc<ExternalGeoClient>) {
        let entry = self
            .paths
            .iter_mut()
            .find(|(profile, _)| profile.provider == provider)
            .unwrap_or_else(|| panic!("{} has no outbound path", provider.as_str()));
        entry.1 = client;
    }
}

#[cfg(test)]
pub(crate) mod stub {
    //! A deterministic stand-in for any External Geo Provider's service. It
    //! is a real HTTP server on the loopback interface, so the tests exercise
    //! the production client, path construction, redirect policy, timeout,
    //! and body handling without a single packet leaving the machine and
    //! without depending on the public internet.
    //!
    //! One server answers every provider path shape, because the scripted
    //! reply never depends on the address: a test asserts on the observed
    //! request path instead.
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};

    /// The body an `Oversized` reply serves. Every implemented profile must
    /// bound its accepted body below this; `the_stub_body_exceeds_every_profile_bound`
    /// pins that invariant so the reply cannot silently stop being oversized.
    pub(crate) const OVERSIZED_BODY_BYTES: usize = 64 * 1024;

    /// One scripted answer. The last scripted answer repeats, so a test that
    /// expects a single request asserts on the observed request log instead
    /// of on an exhausted script.
    #[derive(Debug, Clone)]
    pub(crate) enum StubReply {
        /// A successful JSON document.
        Json(String),
        /// A successful body that is not a JSON object.
        Raw(&'static str),
        /// A non-success status with an empty body.
        Status(u16),
        /// A redirect to another destination.
        Redirect(&'static str),
        /// A response that arrives after the caller's bounded timeout.
        Slow(u64, String),
        /// A body larger than the client-side size bound.
        Oversized,
    }

    impl StubReply {
        /// A reply shaped like the legacy IPinfo document.
        pub(crate) fn country(code: &str) -> Self {
            StubReply::Json(format!(r#"{{"ip":"89.160.20.112","country":"{code}"}}"#))
        }

        pub(crate) fn document(body: &str) -> Self {
            StubReply::Json(body.to_owned())
        }
    }

    /// One observed request. Only the parts a test asserts on are kept: the
    /// stub never sees a real Peer address in CI.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct StubRequest {
        pub path: String,
        pub query: String,
        pub authorization: bool,
    }

    struct StubState {
        replies: Mutex<VecDeque<StubReply>>,
        requests: Mutex<Vec<StubRequest>>,
    }

    /// A running loopback stand-in for one or more External Geo Providers.
    pub(crate) struct StubServer {
        base_url: String,
        state: Arc<StubState>,
        _shutdown: tokio::task::JoinHandle<()>,
    }

    impl StubServer {
        pub(crate) async fn start(replies: Vec<StubReply>) -> Self {
            let state = Arc::new(StubState {
                replies: Mutex::new(replies.into()),
                requests: Mutex::new(Vec::new()),
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("the Geo stub binds a loopback port");
            let address = listener.local_addr().expect("the stub has an address");
            // A fallback route answers every provider path shape, so the stub
            // stays provider-agnostic and the observed path stays assertable.
            let app = Router::new()
                .fallback(handle)
                .with_state(Arc::clone(&state));
            let shutdown = tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            Self {
                base_url: format!("http://{address}"),
                state,
                _shutdown: shutdown,
            }
        }

        pub(crate) fn base_url(&self) -> &str {
            &self.base_url
        }

        /// Every request the stub observed, in arrival order.
        pub(crate) fn requests(&self) -> Vec<StubRequest> {
            self.state
                .requests
                .lock()
                .expect("stub request log lock poisoned")
                .clone()
        }

        pub(crate) fn request_count(&self) -> usize {
            self.state
                .requests
                .lock()
                .expect("stub request log lock poisoned")
                .len()
        }
    }

    async fn handle(State(state): State<Arc<StubState>>, uri: Uri, headers: HeaderMap) -> Response {
        state
            .requests
            .lock()
            .expect("stub request log lock poisoned")
            .push(StubRequest {
                path: uri.path().to_owned(),
                query: uri.query().unwrap_or_default().to_owned(),
                authorization: headers.contains_key(axum::http::header::AUTHORIZATION),
            });
        let reply = {
            let mut replies = state.replies.lock().expect("stub reply lock poisoned");
            if replies.len() > 1 {
                replies.pop_front()
            } else {
                replies.front().cloned()
            }
        };
        match reply {
            Some(StubReply::Json(body)) => (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response(),
            Some(StubReply::Raw(body)) => (StatusCode::OK, body).into_response(),
            Some(StubReply::Status(status)) => StatusCode::from_u16(status)
                .expect("the scripted status is valid")
                .into_response(),
            Some(StubReply::Redirect(location)) => (
                StatusCode::FOUND,
                [(axum::http::header::LOCATION, location)],
            )
                .into_response(),
            Some(StubReply::Slow(millis, body)) => {
                tokio::time::sleep(Duration::from_millis(millis)).await;
                (
                    StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    body,
                )
                    .into_response()
            }
            Some(StubReply::Oversized) => (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                "x".repeat(OVERSIZED_BODY_BYTES),
            )
                .into_response(),
            None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `PROFILES` is the one list of External Geo Providers: it must name
    /// every provider whose profile is reachable, list each exactly once, and
    /// back every production outbound path. A duplicated or missing entry
    /// would otherwise let two answers to "is this provider external" drift
    /// apart, which is what keeps `sends_peer_addresses`, the attribution,
    /// and the background pass consistent for a newly added provider.
    #[test]
    fn profiles_list_every_external_provider_exactly_once() {
        for profile in PROFILES {
            assert_eq!(
                profile_of(profile.provider).map(|found| found as *const ExternalProfile),
                Some(profile as *const ExternalProfile),
                "{} must resolve to its own profile",
                profile.provider.as_str()
            );
        }
        for provider in GeoProvider::ALL {
            let expected = PROFILES
                .iter()
                .filter(|profile| profile.provider == provider)
                .count();
            let provider_is_external = provider.external_profile().is_some();
            assert_eq!(
                expected,
                usize::from(provider_is_external),
                "{} must appear in PROFILES exactly when it is external",
                provider.as_str()
            );
            assert_eq!(
                provider.attribution().is_some(),
                provider != GeoProvider::Disabled,
                "{} must credit its source, or nothing when Disabled",
                provider.as_str()
            );
            // The production table is built from PROFILES, so this proves the
            // runtime lookup and the compile-time list agree for every
            // provider, including the ones that are not external.
            let paths = ExternalGeoPaths::production();
            assert_eq!(
                paths.get(provider).is_some(),
                provider_is_external,
                "{} outbound path/profile disagreement",
                provider.as_str()
            );
        }
    }

    const NOW: &str = "2026-08-12T10:00:00Z";
    /// A globally routable fixture address. It is never sent to the real
    /// service: every test points the client at the loopback stub.
    const FIXTURE: &str = "89.160.20.112";

    fn ip() -> IpAddr {
        FIXTURE.parse().unwrap()
    }

    fn stub_client(
        profile: &'static ExternalProfile,
        stub: &stub::StubServer,
    ) -> ExternalGeoClient {
        ExternalGeoClient::for_tests(profile, stub.base_url(), Duration::from_secs(2))
    }

    /// Each profile assembles exactly the destination its provider documents,
    /// with no doubled or missing separator. The expected strings are written
    /// out rather than derived, so a change to either constant fails here.
    #[test]
    fn production_paths_are_assembled_exactly_once() {
        let address = "1.1.1.1".parse().unwrap();
        let expected = [
            (GeoProvider::Ipinfo, "https://ipinfo.io/1.1.1.1/json"),
            (
                GeoProvider::GeoJs,
                "https://get.geojs.io/v1/ip/geo/1.1.1.1.json",
            ),
        ];
        for (provider, expected) in expected {
            let profile = provider.external_profile().expect("external provider");
            assert_eq!(
                profile.endpoint(profile.endpoint_base, &address),
                expected,
                "{} must assemble its documented destination",
                provider.as_str()
            );
            assert!(
                profile.endpoint_base.starts_with("https://"),
                "{} must use HTTPS",
                provider.as_str()
            );
        }
    }

    /// Bounds every profile must declare for the shared mechanism to stay
    /// bounded, and the stub invariant they depend on. A profile that
    /// accepted a body the stub can serve would make the size-bound test pass
    /// for the wrong reason.
    #[test]
    fn every_profile_declares_bounded_limits() {
        for profile in PROFILES {
            let provider = profile.provider;
            assert!(
                profile.max_body_bytes < stub::OVERSIZED_BODY_BYTES,
                "{} accepts bodies of {} bytes",
                provider.as_str(),
                profile.max_body_bytes
            );
            assert!(profile.request_timeout > Duration::ZERO);
            assert!(profile.max_concurrency > 0);
            assert!(
                profile.endpoint_base.starts_with("https://"),
                "{} must use HTTPS",
                provider.as_str()
            );
            assert!(
                profile.throttle_backoff > Duration::ZERO
                    && profile.max_throttle_backoff >= profile.throttle_backoff
            );
            assert!(!profile.country_field.is_empty());
            assert!(!profile.attribution.is_empty());
            // A flagged provider always states its exact outbound
            // consequence, and that sentence names its own destination
            // origin, so the disclosure cannot describe another provider.
            assert!(
                profile.disclosure.contains(profile.endpoint_base),
                "{} disclosure must name its own endpoint",
                provider.as_str()
            );
            for reason in [profile.throttle_reason, profile.failure_reason] {
                assert!(!reason.contains(profile.endpoint_base));
                assert!(reason.contains(provider.label()));
            }
        }
    }

    /// Every mechanism rule the two External Geo Providers share is proven
    /// once, against both profiles, over the real production client. Each
    /// provider module then keeps only what is its own: the destination, the
    /// country field, and the credit.
    #[tokio::test]
    async fn the_shared_mechanism_behaves_identically_for_every_profile() {
        for profile in PROFILES {
            let provider = profile.provider;
            // A redirect is never followed, and no other non-success status
            // or malformed body ever becomes a country.
            for reply in [
                stub::StubReply::Status(500),
                stub::StubReply::Status(404),
                stub::StubReply::Status(302),
                stub::StubReply::Redirect("https://example.invalid/{ip}/json"),
                stub::StubReply::Raw("<html>not json</html>"),
                stub::StubReply::document("{"),
                stub::StubReply::document("[1,2,3]"),
                stub::StubReply::Oversized,
            ] {
                let stub = stub::StubServer::start(vec![reply.clone()]).await;
                assert_eq!(
                    stub_client(profile, &stub).resolve(ip(), NOW).await,
                    GeoLookup::Unavailable,
                    "{} {reply:?} must not produce a country",
                    provider.as_str()
                );
                assert_eq!(
                    stub.request_count(),
                    1,
                    "{} {reply:?} must not be followed elsewhere",
                    provider.as_str()
                );
            }

            // A bounded timeout is a failure, not a rate limit.
            let stub = stub::StubServer::start(vec![stub::StubReply::Slow(
                400,
                format!(r#"{{"{}":"US"}}"#, profile.country_field),
            )])
            .await;
            let client =
                ExternalGeoClient::for_tests(profile, stub.base_url(), Duration::from_millis(50));
            assert_eq!(client.resolve(ip(), NOW).await, GeoLookup::Unavailable);
            assert_eq!(client.throttled_until(NOW), None);

            // A rate limit arms one bounded provider-wide window, refuses
            // every address inside it without a request, and clears on the
            // next real answer.
            let stub = stub::StubServer::start(vec![
                stub::StubReply::Status(429),
                stub::StubReply::document(&format!(r#"{{"{}":"US"}}"#, profile.country_field)),
            ])
            .await;
            let client = stub_client(profile, &stub);
            assert_eq!(
                client.resolve(ip(), NOW).await,
                GeoLookup::RateLimited,
                "{}",
                provider.as_str()
            );
            assert_eq!(
                client.throttled_until(NOW).as_deref(),
                Some("2026-08-12T10:01:00Z")
            );
            assert_eq!(
                client
                    .resolve("1.1.1.1".parse().unwrap(), "2026-08-12T10:00:30Z")
                    .await,
                GeoLookup::RateLimited
            );
            assert_eq!(stub.request_count(), 1);
            assert_eq!(
                client.resolve(ip(), "2026-08-12T10:01:01Z").await,
                GeoLookup::Country("US".to_owned())
            );
            assert_eq!(client.throttled_until("2026-08-12T10:01:01Z"), None);
            assert_eq!(stub.request_count(), 2);

            // The public-literal precondition is enforced before any request,
            // for every destination.
            let stub = stub::StubServer::start(vec![stub::StubReply::document(&format!(
                r#"{{"{}":"US"}}"#,
                profile.country_field
            ))])
            .await;
            let client = stub_client(profile, &stub);
            for value in ["10.0.0.1", "127.0.0.1", "169.254.1.1", "203.0.113.9", "::1"] {
                assert_eq!(
                    client.resolve(value.parse().unwrap(), NOW).await,
                    GeoLookup::NoCountry,
                    "{} {value} must be refused",
                    provider.as_str()
                );
            }
            assert_eq!(stub.request_count(), 0);
        }
    }
}
