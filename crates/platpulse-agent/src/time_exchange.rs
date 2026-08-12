//! Agent↔Server wall-clock exchange.
//! The Server timestamp is authoritative; local monotonic time is used only
//! to bound request latency and never mixed with RFC3339 observation times.

use std::time::Instant;

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
pub async fn exchange_server_time(
    config: &AgentConfig,
) -> Result<ClockSkewEstimate, TimeExchangeError> {
    let credential = load_credential_file(&config.credential_file)?;
    let client = reqwest::Client::builder()
        .user_agent(format!("platpulse-agent/{}", crate::VERSION))
        .build()?;
    let before = OffsetDateTime::now_utc();
    let monotonic_before = Instant::now();
    let response = client
        .get(format!("{}/api/agent/v1/time", config.server_url))
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
    use super::*;

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
}
