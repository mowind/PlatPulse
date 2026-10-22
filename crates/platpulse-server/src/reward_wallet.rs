//! Reward Wallet Balance (issue #246, CONTEXT.md): the freely available
//! native balance of the one fixed Reward Wallet, as PlatScan reports it.
//!
//! The address and the PlatScan deployment are Server source constants. They
//! are not part of `server.toml`, not editable from Admin and never reported by
//! an Agent, and they are unrelated to `[validator_provider]`. The base URL is
//! injected through [`PlatScanBalanceSource::new`] only so tests can point the
//! refresher at a mock.
//!
//! A background refresher asks PlatScan once per [`REFRESH_INTERVAL`], bounded
//! by [`REQUEST_TIMEOUT`], independent of how often Home is read. A failure is
//! recorded as an attempt with an [`ErrorCategory`] and waits for the next
//! cycle; it never overwrites the last-good balance. The balance stays an exact
//! decimal string end to end and never passes through floating point. Raw
//! upstream errors go to the Server log only; the Public reading carries the
//! category alone.

use std::time::Duration;

use serde_json::Value;
use sqlx::SqlitePool;
use time::OffsetDateTime;

use crate::http::AppState;

/// The Reward Wallet: the PlatON mainnet address that receives this
/// deployment's Validator rewards. Fixed for the deployment (CONTEXT.md).
pub const REWARD_WALLET_ADDRESS: &str = "lat1zyctwgvw3ssagn722cneh6sz92y8j3dp8gqj5h";

/// The PlatScan mainnet deployment the balance is read from.
pub const PLATSCAN_BASE_URL: &str = "https://scan.platon.network";

/// PlatScan reports `data.balance` in LAT already.
pub const REWARD_WALLET_UNIT: &str = "LAT";

/// One PlatScan request per cycle; a failure waits for the next cycle.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Upper bound of one PlatScan request, connection to last body byte.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A last-good balance older than three refresh cycles is stale.
pub const STALE_AFTER: time::Duration = time::Duration::seconds(180);

/// Public SSE invalidation resource published when the stored row changes.
pub const REALTIME_RESOURCE: &str = "reward_wallet";

/// `address/details` answers are a few kilobytes; anything far larger is not
/// the response this refresher understands.
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// PlatScan serializes LAT amounts with bounded precision; a longer string is
/// not a balance.
const MAX_BALANCE_LEN: usize = 64;

/// Why the latest refresh did not produce a balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// HTTP error status, transport failure or timeout.
    UpstreamUnavailable,
    /// PlatScan answered, but not with a usable non-negative decimal balance.
    InvalidResponse,
}

impl ErrorCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpstreamUnavailable => "upstream_unavailable",
            Self::InvalidResponse => "invalid_response",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "upstream_unavailable" => Some(Self::UpstreamUnavailable),
            "invalid_response" => Some(Self::InvalidResponse),
            _ => None,
        }
    }
}

/// How current the reading is. Unavailable means no refresh ever succeeded;
/// it is never presented as a zero balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalanceState {
    Current,
    Stale,
    Unavailable,
}

/// A failed refresh: the category the Public reading carries, and the raw
/// detail that only reaches the Server log.
#[derive(Debug)]
pub struct FetchFailure {
    pub category: ErrorCategory,
    detail: String,
}

impl FetchFailure {
    fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::UpstreamUnavailable,
            detail: detail.into(),
        }
    }

    fn invalid(detail: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::InvalidResponse,
            detail: detail.into(),
        }
    }
}

/// The PlatScan `address/details` reader for the fixed Reward Wallet.
pub struct PlatScanBalanceSource {
    client: reqwest::Client,
    endpoint: String,
}

impl PlatScanBalanceSource {
    /// The production source: the PlatScan mainnet deployment and the fixed
    /// request timeout.
    pub fn platscan() -> Result<Self, String> {
        Self::new(PLATSCAN_BASE_URL, REQUEST_TIMEOUT)
    }

    /// A source reading from `base_url`. Production uses [`Self::platscan`];
    /// tests inject a mock deployment.
    pub fn new(base_url: &str, timeout: Duration) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|_| "unable to construct PlatScan client".to_owned())?;
        Ok(Self {
            client,
            endpoint: format!(
                "{}/browser-server/address/details",
                base_url.trim_end_matches('/')
            ),
        })
    }

    /// Ask PlatScan for the Reward Wallet's available balance.
    pub async fn fetch(&self) -> Result<String, FetchFailure> {
        let body = serde_json::json!({ "address": REWARD_WALLET_ADDRESS });
        let response = self
            .client
            .post(&self.endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|error| FetchFailure::unavailable(format!("request failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(FetchFailure::unavailable(format!("HTTP status {status}")));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| FetchFailure::unavailable(format!("body read failed: {error}")))?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(FetchFailure::invalid("response exceeded the size limit"));
        }
        balance_from_response(&bytes).map_err(FetchFailure::invalid)
    }
}

/// Extract `data.balance` from an `address/details` body. Only `code == 0`
/// with a non-negative decimal string is a balance; every other field of the
/// answer (restricted, delegated, unclaimed reward amounts) is ignored.
fn balance_from_response(body: &[u8]) -> Result<String, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|error| format!("malformed JSON: {error}"))?;
    match value.get("code").and_then(Value::as_i64) {
        Some(0) => {}
        _ => {
            return Err(format!(
                "unsuccessful code {} ({})",
                value.get("code").unwrap_or(&Value::Null),
                value.get("errMsg").unwrap_or(&Value::Null)
            ));
        }
    }
    let balance = value
        .get("data")
        .and_then(|data| data.get("balance"))
        .ok_or_else(|| "data.balance is missing".to_owned())?;
    let Some(balance) = balance.as_str() else {
        return Err(format!("data.balance is not a decimal string: {balance}"));
    };
    if !is_nonnegative_decimal(balance) {
        return Err(format!(
            "data.balance is not a non-negative decimal: {balance:?}"
        ));
    }
    Ok(balance.to_owned())
}

/// `digits` or `digits.digits`, bounded. No sign, exponent or whitespace.
fn is_nonnegative_decimal(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_BALANCE_LEN {
        return false;
    }
    let (whole, fractional) = match value.split_once('.') {
        Some((whole, fractional)) => (whole, Some(fractional)),
        None => (value, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    digits(whole) && fractional.is_none_or(digits)
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct StoredReading {
    balance: Option<String>,
    last_good_at: Option<String>,
    last_attempt_at: String,
    error_category: Option<String>,
}

/// The Reward Wallet as the Server currently knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewardWalletReading {
    pub address: &'static str,
    /// Exact decimal LAT string of the last successful refresh.
    pub balance: Option<String>,
    pub state: BalanceState,
    pub last_good_at: Option<String>,
    pub last_attempt_at: Option<String>,
    /// Category of the latest attempt's failure; none after a success.
    pub error_category: Option<ErrorCategory>,
}

async fn load(
    executor: impl sqlx::SqliteExecutor<'_>,
) -> Result<Option<StoredReading>, sqlx::Error> {
    sqlx::query_as::<_, StoredReading>(
        "SELECT balance, last_good_at, last_attempt_at, error_category FROM reward_wallet_balance WHERE address = $1",
    )
    .bind(REWARD_WALLET_ADDRESS)
    .fetch_optional(executor)
    .await
}

/// Read the stored reading and judge it against `now`.
pub async fn read(
    pool: &SqlitePool,
    now: OffsetDateTime,
) -> Result<RewardWalletReading, sqlx::Error> {
    Ok(reading(load(pool).await?, now))
}

fn reading(stored: Option<StoredReading>, now: OffsetDateTime) -> RewardWalletReading {
    let Some(stored) = stored else {
        return RewardWalletReading {
            address: REWARD_WALLET_ADDRESS,
            balance: None,
            state: BalanceState::Unavailable,
            last_good_at: None,
            last_attempt_at: None,
            error_category: None,
        };
    };
    let state = match (&stored.balance, stored.last_good_at.as_deref()) {
        (Some(_), Some(last_good_at)) => match crate::auth::parse_rfc3339(last_good_at) {
            Some(last_good) if now - last_good <= STALE_AFTER => BalanceState::Current,
            // An unreadable success time cannot prove the value is current.
            _ => BalanceState::Stale,
        },
        _ => BalanceState::Unavailable,
    };
    RewardWalletReading {
        address: REWARD_WALLET_ADDRESS,
        balance: stored.balance,
        state,
        last_good_at: stored.last_good_at,
        last_attempt_at: Some(stored.last_attempt_at),
        error_category: stored
            .error_category
            .as_deref()
            .and_then(ErrorCategory::parse),
    }
}

/// Record one refresh attempt at `at` in one transaction. A success replaces
/// the balance and clears the error; a failure moves only the attempt time and
/// the error category, so the last-good balance survives it. Returns whether
/// the stored row changed; a rolled-back write returns the error instead.
pub async fn record(
    pool: &SqlitePool,
    outcome: Result<&str, ErrorCategory>,
    at: OffsetDateTime,
) -> Result<bool, sqlx::Error> {
    let at = crate::auth::format_rfc3339(at);
    let mut tx = pool.begin().await?;
    let before = load(&mut *tx).await?;
    match outcome {
        Ok(balance) => {
            sqlx::query(
                "INSERT INTO reward_wallet_balance (address, balance, last_good_at, last_attempt_at, error_category)
                 VALUES ($1, $2, $3, $4, NULL)
                 ON CONFLICT(address) DO UPDATE SET
                   balance = excluded.balance,
                   last_good_at = excluded.last_good_at,
                   last_attempt_at = excluded.last_attempt_at,
                   error_category = NULL",
            )
            .bind(REWARD_WALLET_ADDRESS)
            .bind(balance)
            .bind(&at)
            .bind(&at)
            .execute(&mut *tx)
            .await?;
        }
        Err(category) => {
            sqlx::query(
                "INSERT INTO reward_wallet_balance (address, balance, last_good_at, last_attempt_at, error_category)
                 VALUES ($1, NULL, NULL, $2, $3)
                 ON CONFLICT(address) DO UPDATE SET
                   last_attempt_at = excluded.last_attempt_at,
                   error_category = excluded.error_category",
            )
            .bind(REWARD_WALLET_ADDRESS)
            .bind(&at)
            .bind(category.as_str())
            .execute(&mut *tx)
            .await?;
        }
    }
    let after = load(&mut *tx).await?;
    tx.commit().await?;
    Ok(before != after)
}

/// One refresh cycle: ask PlatScan, record the outcome, and publish the Public
/// `reward_wallet` invalidation only after a committed change.
pub async fn refresh_once(
    state: &AppState,
    source: &PlatScanBalanceSource,
) -> Result<bool, sqlx::Error> {
    let outcome = source.fetch().await;
    if let Err(failure) = &outcome {
        tracing::warn!(
            outcome = failure.category.as_str(),
            error = %failure.detail,
            "Reward Wallet refresh failed"
        );
    }
    let outcome = outcome.as_deref().map_err(|failure| failure.category);
    let changed = record(state.db().pool(), outcome, crate::auth::now_utc()).await?;
    if changed {
        state
            .public_realtime()
            .publish(REALTIME_RESOURCE, None::<String>, 1);
    }
    Ok(changed)
}

/// Start the refresher. Development mode starts nothing, so local runs and the
/// e2e suite never contact PlatScan; their Reward Wallet row is seeded directly.
pub fn start(
    state: &AppState,
    development: bool,
    source: PlatScanBalanceSource,
) -> Option<tokio::task::JoinHandle<()>> {
    if development {
        return None;
    }
    Some(tokio::spawn(run_worker(state.clone(), source)))
}

async fn run_worker(state: AppState, source: PlatScanBalanceSource) {
    let mut tick = tokio::time::interval(REFRESH_INTERVAL);
    // A slow or failed cycle never makes the next one come sooner.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if state.is_shutting_down() {
            break;
        }
        tokio::select! {
            _ = state.shutdown_signal() => break,
            _ = tick.tick() => {}
        }
        if state.is_shutting_down() {
            break;
        }
        tokio::select! {
            _ = state.shutdown_signal() => break,
            result = refresh_once(&state, &source) => {
                if let Err(error) = result {
                    tracing::warn!(error = %error, "Reward Wallet refresh not recorded");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use tempfile::TempDir;

    use super::*;
    use crate::database::{ServerDatabase, ServerDatabaseConfig, initialize};

    const MAINNET_BODY: &str = r#"{"errMsg":"Success","code":0,"data":{"balance":"138179.543005858536","restrictingBalance":"0","delegateValue":"4073589","haveReward":"54375.757000581313"}}"#;

    /// Path, content type and body of one request the mock received.
    type RecordedRequest = (String, Option<String>, String);

    #[derive(Clone, Default)]
    struct MockPlatScan {
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        responses: Arc<Mutex<VecDeque<(u16, String)>>>,
        delay: Duration,
    }

    impl MockPlatScan {
        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    async fn mock_handler(
        axum::extract::State(mock): axum::extract::State<MockPlatScan>,
        request: axum::http::Request<axum::body::Body>,
    ) -> axum::response::Response<String> {
        let path = request.uri().path().to_owned();
        let content_type = request
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
            .await
            .unwrap();
        mock.requests.lock().unwrap().push((
            path,
            content_type,
            String::from_utf8_lossy(&body).into_owned(),
        ));
        if !mock.delay.is_zero() {
            tokio::time::sleep(mock.delay).await;
        }
        let (status, body) = mock
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or((500, String::new()));
        axum::response::Response::builder()
            .status(status)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(body)
            .unwrap()
    }

    async fn start_mock(
        responses: Vec<(u16, &str)>,
        delay: Duration,
    ) -> (String, MockPlatScan, tokio::task::JoinHandle<()>) {
        let mock = MockPlatScan {
            requests: Arc::default(),
            responses: Arc::new(Mutex::new(
                responses
                    .into_iter()
                    .map(|(status, body)| (status, body.to_owned()))
                    .collect(),
            )),
            delay,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new()
            .route(
                "/browser-server/address/details",
                axum::routing::post(mock_handler),
            )
            .with_state(mock.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), mock, handle)
    }

    async fn test_db() -> (TempDir, ServerDatabase) {
        let dir = TempDir::new().unwrap();
        let db = initialize(ServerDatabaseConfig::new(dir.path().join("server.db")))
            .await
            .unwrap();
        (dir, db)
    }

    fn test_state(dir: &TempDir, db: ServerDatabase) -> AppState {
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        AppState::new(db, None, auth)
    }

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_792_000_000 + seconds).unwrap()
    }

    fn reward_wallet_events(state: &AppState) -> usize {
        state
            .public_realtime()
            .pending_events()
            .iter()
            .filter(|event| event.resource == REALTIME_RESOURCE)
            .count()
    }

    #[tokio::test]
    async fn a_successful_refresh_records_the_exact_platscan_balance() {
        let (base_url, mock, handle) = start_mock(vec![(200, MAINNET_BODY)], Duration::ZERO).await;
        let source = PlatScanBalanceSource::new(&base_url, Duration::from_secs(5)).unwrap();
        assert_eq!(source.fetch().await.unwrap(), "138179.543005858536");

        let requests = mock.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        let (path, content_type, body) = &requests[0];
        assert_eq!(path, "/browser-server/address/details");
        assert_eq!(content_type.as_deref(), Some("application/json"));
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            body,
            serde_json::json!({ "address": REWARD_WALLET_ADDRESS })
        );
        handle.abort();

        let (_dir, db) = test_db().await;
        assert!(
            record(db.pool(), Ok("138179.543005858536"), at(0))
                .await
                .unwrap()
        );
        let reading = read(db.pool(), at(30)).await.unwrap();
        assert_eq!(reading.address, REWARD_WALLET_ADDRESS);
        assert_eq!(reading.balance.as_deref(), Some("138179.543005858536"));
        assert_eq!(reading.state, BalanceState::Current);
        assert_eq!(reading.error_category, None);
        let stamp = crate::auth::format_rfc3339(at(0));
        assert_eq!(reading.last_good_at.as_deref(), Some(stamp.as_str()));
        assert_eq!(reading.last_attempt_at.as_deref(), Some(stamp.as_str()));
    }

    #[tokio::test]
    async fn unusable_answers_are_invalid_responses() {
        let cases: Vec<(&str, &str)> = vec![
            (
                "code != 0",
                r#"{"errMsg":"Failure","code":-1,"data":{"balance":"1"}}"#,
            ),
            ("code missing", r#"{"data":{"balance":"1"}}"#),
            (
                "balance missing",
                r#"{"errMsg":"Success","code":0,"data":{"restrictingBalance":"0"}}"#,
            ),
            ("data missing", r#"{"errMsg":"Success","code":0}"#),
            ("negative", r#"{"code":0,"data":{"balance":"-1.5"}}"#),
            ("exponent", r#"{"code":0,"data":{"balance":"1e5"}}"#),
            ("text", r#"{"code":0,"data":{"balance":"abc"}}"#),
            ("empty", r#"{"code":0,"data":{"balance":""}}"#),
            ("two dots", r#"{"code":0,"data":{"balance":"1.2.3"}}"#),
            ("dangling dot", r#"{"code":0,"data":{"balance":"1."}}"#),
            ("whitespace", r#"{"code":0,"data":{"balance":" 1"}}"#),
            ("number", r#"{"code":0,"data":{"balance":138179.54}}"#),
            ("null", r#"{"code":0,"data":{"balance":null}}"#),
            ("not JSON", "<html>maintenance</html>"),
        ];
        let responses = cases.iter().map(|(_, body)| (200, *body)).collect();
        let (base_url, _mock, handle) = start_mock(responses, Duration::ZERO).await;
        let source = PlatScanBalanceSource::new(&base_url, Duration::from_secs(5)).unwrap();
        for (name, _) in &cases {
            let failure = source.fetch().await.expect_err(name);
            assert_eq!(failure.category, ErrorCategory::InvalidResponse, "{name}");
        }
        handle.abort();
    }

    #[tokio::test]
    async fn http_errors_and_timeouts_are_upstream_unavailable() {
        let (base_url, _mock, handle) = start_mock(
            vec![(500, ""), (503, MAINNET_BODY), (404, "")],
            Duration::ZERO,
        )
        .await;
        let source = PlatScanBalanceSource::new(&base_url, Duration::from_secs(5)).unwrap();
        for status in [500, 503, 404] {
            let failure = source.fetch().await.expect_err("HTTP error");
            assert_eq!(
                failure.category,
                ErrorCategory::UpstreamUnavailable,
                "{status}"
            );
        }
        handle.abort();

        let (base_url, _mock, handle) =
            start_mock(vec![(200, MAINNET_BODY)], Duration::from_millis(500)).await;
        let source = PlatScanBalanceSource::new(&base_url, Duration::from_millis(100)).unwrap();
        let failure = source.fetch().await.expect_err("timeout");
        assert_eq!(failure.category, ErrorCategory::UpstreamUnavailable);
        handle.abort();

        // Nothing listening at all is the same category.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let closed = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let source = PlatScanBalanceSource::new(&closed, Duration::from_secs(1)).unwrap();
        let failure = source.fetch().await.expect_err("connection refused");
        assert_eq!(failure.category, ErrorCategory::UpstreamUnavailable);
    }

    #[tokio::test]
    async fn a_failure_never_overwrites_the_last_good_balance() {
        let (_dir, db) = test_db().await;
        record(db.pool(), Ok("138179.543005858536"), at(0))
            .await
            .unwrap();
        for (offset, category) in [
            (60, ErrorCategory::UpstreamUnavailable),
            (120, ErrorCategory::InvalidResponse),
        ] {
            assert!(record(db.pool(), Err(category), at(offset)).await.unwrap());
            let reading = read(db.pool(), at(offset)).await.unwrap();
            assert_eq!(reading.balance.as_deref(), Some("138179.543005858536"));
            assert_eq!(
                reading.last_good_at.as_deref(),
                Some(crate::auth::format_rfc3339(at(0)).as_str())
            );
            let attempt = crate::auth::format_rfc3339(at(offset));
            assert_eq!(reading.last_attempt_at.as_deref(), Some(attempt.as_str()));
            assert_eq!(reading.error_category, Some(category));
        }
        // The next success clears the category.
        record(db.pool(), Ok("138180"), at(180)).await.unwrap();
        let reading = read(db.pool(), at(180)).await.unwrap();
        assert_eq!(reading.balance.as_deref(), Some("138180"));
        assert_eq!(reading.error_category, None);
        assert_eq!(reading.state, BalanceState::Current);
    }

    #[tokio::test]
    async fn a_wallet_never_read_successfully_is_unavailable_not_zero() {
        let (_dir, db) = test_db().await;
        let reading = read(db.pool(), at(0)).await.unwrap();
        assert_eq!(reading.state, BalanceState::Unavailable);
        assert_eq!(reading.balance, None);
        assert_eq!(reading.last_attempt_at, None);
        assert_eq!(reading.error_category, None);

        record(db.pool(), Err(ErrorCategory::UpstreamUnavailable), at(0))
            .await
            .unwrap();
        let reading = read(db.pool(), at(0)).await.unwrap();
        assert_eq!(reading.state, BalanceState::Unavailable);
        assert_eq!(reading.balance, None);
        assert_eq!(reading.last_good_at, None);
        assert_eq!(
            reading.error_category,
            Some(ErrorCategory::UpstreamUnavailable)
        );
        assert!(reading.last_attempt_at.is_some());
    }

    #[tokio::test]
    async fn a_last_good_balance_older_than_three_cycles_is_stale() {
        let (_dir, db) = test_db().await;
        record(db.pool(), Ok("1"), at(0)).await.unwrap();
        assert_eq!(
            read(db.pool(), at(180)).await.unwrap().state,
            BalanceState::Current
        );
        let stale = read(db.pool(), at(181)).await.unwrap();
        assert_eq!(stale.state, BalanceState::Stale);
        assert_eq!(stale.balance.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn the_last_good_balance_survives_a_restart() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("server.db");
        {
            let db = initialize(ServerDatabaseConfig::new(&path)).await.unwrap();
            record(db.pool(), Ok("138179.543005858536"), at(0))
                .await
                .unwrap();
            record(db.pool(), Err(ErrorCategory::InvalidResponse), at(60))
                .await
                .unwrap();
            db.pool().close().await;
        }
        let db = ServerDatabase::open_existing(ServerDatabaseConfig::new(&path))
            .await
            .unwrap();
        let reading = read(db.pool(), at(60)).await.unwrap();
        assert_eq!(reading.balance.as_deref(), Some("138179.543005858536"));
        assert_eq!(reading.state, BalanceState::Current);
        assert_eq!(reading.error_category, Some(ErrorCategory::InvalidResponse));
    }

    #[tokio::test]
    async fn a_committed_change_publishes_the_invalidation_and_a_rollback_does_not() {
        let (base_url, _mock, handle) = start_mock(
            vec![(200, MAINNET_BODY), (200, MAINNET_BODY)],
            Duration::ZERO,
        )
        .await;
        let source = PlatScanBalanceSource::new(&base_url, Duration::from_secs(5)).unwrap();
        let (dir, db) = test_db().await;
        let state = test_state(&dir, db);

        assert!(refresh_once(&state, &source).await.unwrap());
        assert_eq!(reward_wallet_events(&state), 1);
        assert_eq!(
            read(state.db().pool(), crate::auth::now_utc())
                .await
                .unwrap()
                .balance
                .as_deref(),
            Some("138179.543005858536")
        );

        // A write that rolls back leaves the row and the stream untouched.
        sqlx::query(
            "CREATE TRIGGER reject_reward_wallet BEFORE UPDATE ON reward_wallet_balance
             BEGIN SELECT RAISE(ABORT, 'rejected'); END",
        )
        .execute(state.db().pool())
        .await
        .unwrap();
        let before = state.public_realtime().current_id();
        assert!(refresh_once(&state, &source).await.is_err());
        assert_eq!(state.public_realtime().current_id(), before);
        assert_eq!(reward_wallet_events(&state), 1);
        handle.abort();
    }

    #[tokio::test]
    async fn an_unchanged_row_publishes_nothing() {
        let (_dir, db) = test_db().await;
        assert!(record(db.pool(), Ok("1"), at(0)).await.unwrap());
        assert!(!record(db.pool(), Ok("1"), at(0)).await.unwrap());
        assert!(
            record(db.pool(), Err(ErrorCategory::InvalidResponse), at(0))
                .await
                .unwrap()
        );
        assert!(
            !record(db.pool(), Err(ErrorCategory::InvalidResponse), at(0))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn development_mode_never_contacts_platscan() {
        let (base_url, mock, handle) = start_mock(
            vec![(200, MAINNET_BODY), (200, MAINNET_BODY)],
            Duration::ZERO,
        )
        .await;
        let (dir, db) = test_db().await;
        let state = test_state(&dir, db);

        let source = PlatScanBalanceSource::new(&base_url, Duration::from_secs(5)).unwrap();
        assert!(start(&state, true, source).is_none());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(mock.request_count(), 0);

        // The same wiring outside development mode refreshes immediately.
        let source = PlatScanBalanceSource::new(&base_url, Duration::from_secs(5)).unwrap();
        let worker = start(&state, false, source).expect("production refresher");
        for _ in 0..50 {
            if mock.request_count() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(mock.request_count(), 1);
        worker.abort();
        handle.abort();
    }

    #[test]
    fn decimal_recognition_is_strict() {
        for valid in ["0", "1", "138179.543005858536", "007.10"] {
            assert!(is_nonnegative_decimal(valid), "{valid}");
        }
        for invalid in ["", ".", ".5", "5.", "-1", "+1", "1e3", "1,000", " 1", "NaN"] {
            assert!(!is_nonnegative_decimal(invalid), "{invalid}");
        }
        assert!(!is_nonnegative_decimal(&"9".repeat(MAX_BALANCE_LEN + 1)));
    }
}
