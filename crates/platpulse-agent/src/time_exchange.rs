//! Agent↔Server wall-clock exchange.
//! The Server timestamp is authoritative; local monotonic time is used only
//! to bound request latency and never mixed with RFC3339 observation times.

use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde::Deserialize;
use thiserror::Error;
use time::OffsetDateTime;

use crate::config::AgentConfig;
use crate::credential::{CredentialError, load_credential_file};

/// Clock is unreliable when the estimated wall-clock offset exceeds five
/// minutes. Liveness is intentionally independent and remains receipt-based.
pub const CLOCK_UNRELIABLE_THRESHOLD_MS: i64 = 5 * 60 * 1000;
pub const MAX_ROUND_TRIP_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSkewEstimate {
    /// Agent wall clock minus Server wall clock, in milliseconds. Negative
    /// means the Agent clock is behind the Server clock.
    pub offset_ms: i64,
    pub unreliable: bool,
    pub round_trip_ms: u64,
}
#[derive(Debug, Error)]
pub enum TimeExchangeError {
    #[error("failed to load Agent Credential: {0}")]
    Credential(#[from] CredentialError),
    #[error("time exchange transport failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("time exchange returned HTTP {0}")]
    Http(StatusCode),
    #[error("Server returned an invalid time exchange response")]
    InvalidResponse,
    #[error("Server time is outside the supported timestamp range")]
    InvalidTimestamp,
    #[error("time exchange round trip exceeded the bounded latency")]
    RoundTripTooLong,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerTimeResponse {
    server_time: String,
}

fn unix_millis(time: OffsetDateTime) -> Result<i64, TimeExchangeError> {
    time.unix_timestamp_nanos()
        .checked_div(1_000_000)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or(TimeExchangeError::InvalidTimestamp)
}

/// Estimate skew using the midpoint of the request's local wall-clock
/// interval. The exchange is bounded by the observed round-trip duration.
pub fn estimate_clock_skew(
    server_time: OffsetDateTime,
    local_before: OffsetDateTime,
    local_after: OffsetDateTime,
    round_trip_ms: u64,
) -> Result<ClockSkewEstimate, TimeExchangeError> {
    if round_trip_ms > MAX_ROUND_TRIP_MS {
        return Err(TimeExchangeError::RoundTripTooLong);
    }
    let before = unix_millis(local_before)?;
    let after = unix_millis(local_after)?;
    let server = unix_millis(server_time)?;
    let midpoint = before.saturating_add(after.saturating_sub(before) / 2);
    let offset_ms = midpoint.saturating_sub(server);
    Ok(ClockSkewEstimate {
        offset_ms,
        unreliable: offset_ms.abs() > CLOCK_UNRELIABLE_THRESHOLD_MS,
        round_trip_ms,
    })
}

/// Perform one authenticated Server time exchange.
///
/// The client is injected (issue #244): the Agent keeps one process-wide
/// client, so the exchange reuses the keep-alive connection instead of
/// building a throwaway pool per Collection tick. The round-trip budget
/// therefore belongs to this request, not to the client.
pub async fn exchange_server_time(
    config: &AgentConfig,
    client: &reqwest::Client,
) -> Result<ClockSkewEstimate, TimeExchangeError> {
    let credential = load_credential_file(&config.credential_file)?;
    let before = OffsetDateTime::now_utc();
    let monotonic_before = Instant::now();
    let response = client
        .get(format!("{}/api/agent/v1/time", config.server_url))
        .timeout(Duration::from_millis(MAX_ROUND_TRIP_MS))
        .bearer_auth(credential)
        .send()
        .await?;
    let after = OffsetDateTime::now_utc();
    let round_trip_ms = monotonic_before.elapsed().as_millis() as u64;
    if response.status() != StatusCode::OK {
        return Err(TimeExchangeError::Http(response.status()));
    }
    let payload: ServerTimeResponse = response
        .json()
        .await
        .map_err(|_| TimeExchangeError::InvalidResponse)?;
    let server_time = time::OffsetDateTime::parse(
        &payload.server_time,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| TimeExchangeError::InvalidResponse)?;
    estimate_clock_skew(server_time, before, after, round_trip_ms)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::BackfillConfig;
    use crate::http_test_support::KeepAliveServer;

    const SERVER_TIME_BODY: &str = r#"{"server_time":"2026-10-15T09:00:00Z"}"#;

    fn at(ms: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).unwrap()
    }

    #[test]
    fn midpoint_exchange_distinguishes_normal_and_unreliable_skew() {
        let normal = estimate_clock_skew(at(10_000), at(10_001), at(10_003), 2).unwrap();
        assert_eq!(normal.offset_ms, 2);
        assert!(!normal.unreliable);
        let bad = estimate_clock_skew(
            at(0),
            at(CLOCK_UNRELIABLE_THRESHOLD_MS + 1),
            at(CLOCK_UNRELIABLE_THRESHOLD_MS + 3),
            2,
        )
        .unwrap();
        assert!(bad.unreliable);
    }

    fn token() -> String {
        format!(
            "pp_agent_{}_{}",
            "0195f2a1-0011-4011-8011-000000000011",
            "a".repeat(64)
        )
    }

    fn config_for(dir: &Path, server_url: &str) -> AgentConfig {
        let credential_file = dir.join("credential");
        crate::credential::write_credential_file(&credential_file, &token()).unwrap();
        AgentConfig {
            config_path: dir.join("agent.toml"),
            server_url: server_url.to_owned(),
            credential_file,
            state_db: dir.join("agent.db"),
            collection_interval_seconds: 5,
            backfill: BackfillConfig::default(),
        }
    }

    #[tokio::test]
    async fn exchange_reuses_one_connection_across_collection_ticks() {
        let dir = tempfile::tempdir().unwrap();
        let server = KeepAliveServer::start(SERVER_TIME_BODY.to_owned()).await;
        let config = config_for(dir.path(), &server.url(""));
        let client = crate::http_client::shared_client().unwrap();

        for _ in 0..3 {
            exchange_server_time(&config, client)
                .await
                .expect("the Server time exchange succeeds");
        }

        assert_eq!(server.request_count(), 3);
        assert_eq!(server.connection_count(), 1);
        assert_eq!(
            server.user_agents(),
            vec![format!("platpulse-agent/{}", crate::VERSION); 3]
        );
    }

    #[tokio::test]
    async fn exchange_bounds_the_round_trip_on_the_request_not_the_client() {
        let dir = tempfile::tempdir().unwrap();
        let delay = Duration::from_millis(MAX_ROUND_TRIP_MS * 3);
        let server = KeepAliveServer::start_with_delay(SERVER_TIME_BODY.to_owned(), delay).await;
        let config = config_for(dir.path(), &server.url(""));
        // A client with no process-wide budget: the cap has to come from the request.
        let client = reqwest::Client::new();

        let started = Instant::now();
        let error = exchange_server_time(&config, &client)
            .await
            .expect_err("a Server past the round-trip budget fails the exchange");
        let elapsed = started.elapsed();

        assert!(
            matches!(&error, TimeExchangeError::Transport(source) if source.is_timeout()),
            "unexpected error: {error}"
        );
        assert!(elapsed < delay, "the exchange waited {elapsed:?}");
    }
}
