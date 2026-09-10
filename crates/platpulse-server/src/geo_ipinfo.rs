//! The IPinfo country lookup boundary.
//!
//! IPinfo is the first Geo Provider that resolves countries outside this
//! Server, so this module owns the whole outbound contract (issue #134):
//!
//! * exactly one fixed HTTPS destination, the legacy `{base}/{ip}/json` path
//!   the Owner approved - never the Lite API, never a token, and never a
//!   destination an Admin request or an Agent report can influence;
//! * only Server-validated canonical public literals are sent; a private,
//!   loopback, link-local, or reserved address is refused here as well as in
//!   the scheduling path, and a hostname is never resolved;
//! * redirects are not followed at all, so a response can never move the
//!   request to another destination;
//! * one bounded timeout, one bounded response size, and one bounded
//!   provider-wide backoff after a rate-limit response.
//!
//! Nothing in this module is reachable from report ingestion: the background
//! path in `crate::geo_backfill` is the only caller, and a lookup never runs
//! inside a receipt transaction.

use std::net::IpAddr;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use crate::geo::{self, GeoLookup};

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

/// The subset of a legacy IPinfo document this Server interprets. Every other
/// field (`hostname`, `city`, `org`, `loc`, ...) is discarded at the trust
/// boundary: only the two-letter country code is ever retained.
#[derive(Debug, serde::Deserialize)]
struct IpinfoDocument {
    country: Option<String>,
}

/// What this process knows about the outbound path without reading the
/// retained cache. It carries no address and no request detail, only the
/// bounded backoff window and whether the latest attempt worked.
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

/// The outbound IPinfo path. It owns the HTTP client, the fixed destination,
/// the bounded provider-wide backoff, and the latest path outcome; it holds
/// no per-address state, so the retained cache stays the single source of
/// country results.
pub struct IpinfoClient {
    http: reqwest::Client,
    base_url: String,
    path: RwLock<PathState>,
}

impl std::fmt::Debug for IpinfoClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IpinfoClient")
            .field("endpoint", &IPINFO_ENDPOINT_BASE)
            .finish()
    }
}

impl IpinfoClient {
    /// The production client: the fixed destination, no credentials. This is
    /// the only constructor the Server binary uses.
    pub fn production() -> Self {
        Self {
            http: production_http(),
            base_url: IPINFO_ENDPOINT_BASE.to_owned(),
            path: RwLock::new(PathState::default()),
        }
    }

    /// A deterministic test double. The loopback endpoint and shortened
    /// timeout exist only in test builds: neither is reachable from Server
    /// configuration, an Admin request, or an Agent report, so the production
    /// destination can never be overridden.
    #[cfg(test)]
    pub(crate) fn for_tests(base_url: &str, timeout: Duration) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("the IPinfo test client builds"),
            base_url: base_url.trim_end_matches('/').to_owned(),
            path: RwLock::new(PathState::default()),
        }
    }

    /// The instant before which a bounded backoff still forbids outbound
    /// work, or `None` when the provider may be asked again at `now`.
    pub fn throttled_until(&self, now: &str) -> Option<String> {
        let path = self.path.read().expect("IPinfo path lock poisoned");
        let until = path.until.as_deref()?;
        (until > now).then(|| until.to_owned())
    }

    /// Whether the most recent outbound attempt produced no usable result.
    /// It clears on the next successful provider response, so the reported
    /// failure state always describes the latest real exchange.
    pub fn last_failure(&self) -> bool {
        self.path
            .read()
            .expect("IPinfo path lock poisoned")
            .last_failure
    }

    /// Resolve one address through the fixed legacy endpoint.
    ///
    /// The outcomes are deliberately distinct: a document without a country
    /// is an authoritative `NoCountry`, a rate limit is `RateLimited` so the
    /// scheduler backs off instead of recording an attempt, and every
    /// transport, status, size, and parse failure is `Unavailable` so the
    /// caller records only the attempt and never rewrites retained evidence.
    pub async fn resolve(&self, ip: IpAddr, now: &str) -> GeoLookup {
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
        // The address is a canonical literal, so the request path is fixed
        // and no DNS lookup ever happens for provider input.
        let endpoint = format!("{}/{ip}/json", self.base_url);
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
            Ok(body) if body.len() <= IPINFO_MAX_BODY_BYTES => body,
            Ok(_) => return GeoLookup::Unavailable,
            Err(_) => return GeoLookup::Unavailable,
        };
        let document: IpinfoDocument = match serde_json::from_slice(&body) {
            Ok(document) => document,
            Err(_) => return GeoLookup::Unavailable,
        };
        match document.country.as_deref().map(str::trim) {
            // The provider answered authoritatively: this address has no
            // country, which is a result and not a failure.
            None | Some("") => GeoLookup::NoCountry,
            Some(country) => {
                let code = country.to_ascii_uppercase();
                if geo::is_country_code(&code) {
                    GeoLookup::Country(code)
                } else {
                    // A present but unusable code is a malformed result, not
                    // an authoritative "no country".
                    GeoLookup::Unavailable
                }
            }
        }
    }

    /// Arm the bounded provider-wide backoff. Every address in the current
    /// pass observes it, so a rate limit costs at most the in-flight requests
    /// plus one window of waiting instead of one retry per Peer address.
    fn arm_throttle(&self, now: &str) {
        let mut path = self.path.write().expect("IPinfo path lock poisoned");
        path.consecutive = path.consecutive.saturating_add(1);
        path.last_failure = true;
        let factor = 1u32 << path.consecutive.saturating_sub(1).min(4);
        let backoff = IPINFO_THROTTLE_BACKOFF
            .saturating_mul(factor)
            .min(IPINFO_MAX_THROTTLE_BACKOFF);
        path.until = Some(crate::auth::format_rfc3339(
            crate::auth::parse_rfc3339(now).unwrap_or_else(crate::auth::now_utc)
                + time::Duration::seconds(backoff.as_secs() as i64),
        ));
    }

    /// Record that the latest attempt produced no usable result.
    fn note_failure(&self) {
        self.path
            .write()
            .expect("IPinfo path lock poisoned")
            .last_failure = true;
    }

    /// A provider that answered normally is neither failing nor rate limited:
    /// forget both, so a later isolated failure starts from a clean state.
    fn note_success(&self) {
        let mut path = self.path.write().expect("IPinfo path lock poisoned");
        path.consecutive = 0;
        path.until = None;
        path.last_failure = false;
    }
}

/// The shared production client. One TLS configuration and one connection
/// pool serve every lookup; the client carries no per-request state.
fn production_http() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(IPINFO_REQUEST_TIMEOUT)
                // A redirect could move the request to a destination this
                // Server never approved, so none is followed.
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("PlatPulse/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("the IPinfo HTTP client builds")
        })
        .clone()
}

#[cfg(test)]
pub(crate) mod stub {
    //! A deterministic stand-in for the IPinfo service. It is a real HTTP
    //! server on the loopback interface, so the tests exercise the production
    //! client, redirect policy, timeout, and body handling without a single
    //! packet leaving the machine and without depending on the public
    //! internet.
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::Router;
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;

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

    /// A running loopback IPinfo stand-in.
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
                .expect("the IPinfo stub binds a loopback port");
            let address = listener.local_addr().expect("the stub has an address");
            let app = Router::new()
                .route("/{ip}/json", get(handle))
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

    async fn handle(
        State(state): State<Arc<StubState>>,
        Path(_ip): Path<String>,
        uri: Uri,
        headers: HeaderMap,
    ) -> Response {
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
                "x".repeat(super::IPINFO_MAX_BODY_BYTES + 1),
            )
                .into_response(),
            None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stub::{StubReply, StubServer};
    use super::*;

    const NOW: &str = "2026-08-12T10:00:00Z";
    /// A globally routable fixture address. It is never sent to the real
    /// service: every test points the client at the loopback stub.
    const FIXTURE: &str = "89.160.20.112";

    fn stub_client(stub: &StubServer) -> IpinfoClient {
        IpinfoClient::for_tests(stub.base_url(), Duration::from_secs(2))
    }

    fn ip() -> IpAddr {
        FIXTURE.parse().unwrap()
    }

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

    #[tokio::test]
    async fn malformed_bodies_are_failures() {
        for reply in [
            StubReply::Raw("<html>not json</html>"),
            StubReply::document("{"),
            StubReply::document("[1,2,3]"),
            StubReply::Oversized,
        ] {
            let stub = StubServer::start(vec![reply]).await;
            assert_eq!(
                stub_client(&stub).resolve(ip(), NOW).await,
                GeoLookup::Unavailable
            );
        }
    }

    #[tokio::test]
    async fn unsuccessful_statuses_are_failures_and_redirects_are_not_followed() {
        for reply in [
            StubReply::Status(500),
            StubReply::Status(404),
            StubReply::Status(302),
            StubReply::Redirect("https://example.invalid/{ip}/json"),
        ] {
            let stub = StubServer::start(vec![reply.clone()]).await;
            assert_eq!(
                stub_client(&stub).resolve(ip(), NOW).await,
                GeoLookup::Unavailable,
                "{reply:?} must not produce a country"
            );
            assert_eq!(
                stub.request_count(),
                1,
                "{reply:?} must not be followed to another destination"
            );
        }
    }

    #[tokio::test]
    async fn a_rate_limit_arms_one_bounded_provider_wide_backoff() {
        let stub = StubServer::start(vec![StubReply::Status(429)]).await;
        let client = stub_client(&stub);
        assert_eq!(client.resolve(ip(), NOW).await, GeoLookup::RateLimited);
        assert_eq!(stub.request_count(), 1);
        // The window is bounded and reported without any request detail.
        assert_eq!(
            client.throttled_until(NOW).as_deref(),
            Some("2026-08-12T10:01:00Z")
        );
        // A second address inside the window is refused without a request.
        let other: IpAddr = "1.1.1.1".parse().unwrap();
        assert_eq!(
            client.resolve(other, "2026-08-12T10:00:30Z").await,
            GeoLookup::RateLimited
        );
        assert_eq!(stub.request_count(), 1);
        // Once the bounded window passes, the provider is asked again and the
        // recovery is real: the scripted success is served from then on.
        let stub = StubServer::start(vec![StubReply::Status(429), StubReply::country("US")]).await;
        let client = stub_client(&stub);
        assert_eq!(client.resolve(ip(), NOW).await, GeoLookup::RateLimited);
        assert_eq!(
            client.resolve(ip(), "2026-08-12T10:01:01Z").await,
            GeoLookup::Country("US".to_owned())
        );
        assert_eq!(client.throttled_until("2026-08-12T10:01:01Z"), None);
        assert_eq!(stub.request_count(), 2);
    }

    #[tokio::test]
    async fn a_repeated_rate_limit_grows_the_backoff_up_to_the_bound() {
        let stub = StubServer::start(vec![StubReply::Status(429)]).await;
        let client = stub_client(&stub);
        let windows = [
            ("2026-08-12T10:00:00Z", "2026-08-12T10:01:00Z"),
            ("2026-08-12T10:01:00Z", "2026-08-12T10:03:00Z"),
            ("2026-08-12T10:03:00Z", "2026-08-12T10:07:00Z"),
            ("2026-08-12T10:07:00Z", "2026-08-12T10:15:00Z"),
            ("2026-08-12T10:15:00Z", "2026-08-12T10:30:00Z"),
            ("2026-08-12T10:30:00Z", "2026-08-12T10:45:00Z"),
        ];
        for (now, expected) in windows {
            assert_eq!(client.resolve(ip(), now).await, GeoLookup::RateLimited);
            assert_eq!(client.throttled_until(now).as_deref(), Some(expected));
        }
    }

    #[tokio::test]
    async fn a_slow_provider_hits_the_bounded_timeout() {
        let stub =
            StubServer::start(vec![StubReply::Slow(400, r#"{"country":"US"}"#.to_owned())]).await;
        let client = IpinfoClient::for_tests(stub.base_url(), Duration::from_millis(50));
        assert_eq!(client.resolve(ip(), NOW).await, GeoLookup::Unavailable);
    }

    #[tokio::test]
    async fn non_public_and_non_literal_inputs_never_reach_the_endpoint() {
        let stub = StubServer::start(vec![StubReply::country("US")]).await;
        let client = stub_client(&stub);
        for value in ["10.0.0.1", "127.0.0.1", "169.254.1.1", "203.0.113.9", "::1"] {
            assert_eq!(
                client.resolve(value.parse().unwrap(), NOW).await,
                GeoLookup::NoCountry,
                "{value} must be refused by the trust boundary"
            );
        }
        assert_eq!(stub.request_count(), 0);
    }

    #[test]
    fn the_production_destination_is_fixed_and_https() {
        assert_eq!(IPINFO_ENDPOINT_BASE, "https://ipinfo.io");
        assert!(!geo::GeoProvider::Ipinfo.needs_local_database());
        assert!(geo::GeoProvider::Ipinfo.sends_peer_addresses());
        assert_eq!(
            geo::GeoProvider::Ipinfo.attribution(),
            Some(IPINFO_ATTRIBUTION)
        );
        assert!(IPINFO_ATTRIBUTION.contains("IPinfo"));
    }
}
