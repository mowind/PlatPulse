//! Server-clock observation instants for Agent Reports (ADR 0014, issue #274).
//!
//! A Report carries the Agent's own wall-clock observation instants, and an
//! immutable Report keeps carrying them however late it is delivered. Judging
//! freshness, history or alerts by the instant the Server happened to receive a
//! Report therefore says something about the delivery, not about the Node: a
//! Node whose Reports were generated two hours ago looks as fresh as one that is
//! reporting right now.
//!
//! The Server's answer is the observation instant expressed on the Server clock.
//! The Agent measures its own skew against Server time in every Report
//! (`host.clock_skew`), so the corrected instant is the Agent instant minus that
//! skew. A skew the Server cannot trust is not corrected by: an Agent that is
//! Clock Unreliable only tells the Server that its wall clock is unusable, and
//! the receipt instant remains the fallback, exactly as before ADR 0014.

use platpulse_core::time::Rfc3339;

use crate::http::agent::CLOCK_UNRELIABLE_THRESHOLD_MS;

/// The measured skew the Server is willing to correct by, in milliseconds.
///
/// `host.clock_skew` is positive when the Agent clock runs ahead of Server time
/// (see `estimate_clock_skew` in the Agent). A magnitude at or beyond
/// [`CLOCK_UNRELIABLE_THRESHOLD_MS`] means the Agent is Clock Unreliable, so the
/// reading says nothing about the offset and correction is refused.
pub fn measured_skew(clock_skew_ms: Option<i64>) -> Option<i64> {
    clock_skew_ms.filter(|skew| skew.unsigned_abs() <= CLOCK_UNRELIABLE_THRESHOLD_MS as u64)
}

/// The Server-clock instant at which `observed` happened.
///
/// `skew_ms` is the Report's own `host.clock_skew` reading, filtered through
/// [`measured_skew`] here so the one rule for "this skew cannot be corrected
/// by" lives in this function and cannot be forgotten by a caller. Returns
/// `fallback` when the Report carries no observation instant or its measured
/// skew is untrustworthy: the receipt instant is the only instant the Server
/// can defend in that case.
pub fn observation_instant(
    observed: Option<Rfc3339>,
    skew_ms: Option<i64>,
    fallback: &str,
) -> String {
    match (observed, measured_skew(skew_ms)) {
        (Some(observed), Some(skew)) => {
            let corrected = observed.as_datetime() - time::Duration::milliseconds(skew);
            // Every stored instant is second-precision, so a sub-second skew
            // must not produce a value the rest of the Server cannot compare.
            let canonical = corrected
                .replace_nanosecond(0)
                .expect("zero nanoseconds is a valid component value");
            crate::auth::format_rfc3339(canonical)
        }
        _ => fallback.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn instant(value: &str) -> Rfc3339 {
        Rfc3339::from_str(value).expect("the fixture instant parses")
    }

    #[test]
    fn a_measured_skew_moves_the_instant_onto_the_server_clock() {
        // An Agent three minutes ahead stamps 09:03; on the Server clock the
        // observation happened at 09:00.
        let corrected = observation_instant(
            Some(instant("2026-08-12T09:03:00Z")),
            Some(180_000),
            "2026-08-12T10:00:00Z",
        );
        assert_eq!(corrected, "2026-08-12T09:00:00Z");
        // ... and an Agent three minutes behind stamps 08:57 for the same moment.
        let corrected = observation_instant(
            Some(instant("2026-08-12T08:57:00Z")),
            Some(-180_000),
            "2026-08-12T10:00:00Z",
        );
        assert_eq!(corrected, "2026-08-12T09:00:00Z");
    }

    #[test]
    fn a_sub_second_skew_never_produces_a_sub_second_instant() {
        let corrected = observation_instant(
            Some(instant("2026-08-12T09:00:00Z")),
            Some(1_500),
            "2026-08-12T10:00:00Z",
        );
        assert_eq!(corrected, "2026-08-12T08:59:58Z");
    }

    #[test]
    fn an_unreliable_clock_falls_back_to_the_receipt_instant() {
        let skew = CLOCK_UNRELIABLE_THRESHOLD_MS + 1;
        assert_eq!(measured_skew(Some(skew)), None);
        assert_eq!(measured_skew(Some(-skew)), None);
        assert_eq!(
            observation_instant(
                Some(instant("2026-08-12T09:00:00Z")),
                Some(skew),
                "2026-08-12T10:00:00Z"
            ),
            "2026-08-12T10:00:00Z"
        );
    }

    #[test]
    fn an_unmeasured_skew_or_missing_observation_falls_back_to_the_receipt_instant() {
        let observed = Some(instant("2026-08-12T09:00:00Z"));
        assert_eq!(
            observation_instant(observed, None, "2026-08-12T10:00:00Z"),
            "2026-08-12T10:00:00Z"
        );
        assert_eq!(
            observation_instant(None, Some(0), "2026-08-12T10:00:00Z"),
            "2026-08-12T10:00:00Z"
        );
        assert_eq!(
            observation_instant(None, None, "2026-08-12T10:00:00Z"),
            "2026-08-12T10:00:00Z"
        );
    }

    #[test]
    fn the_threshold_itself_is_still_measured() {
        assert_eq!(
            measured_skew(Some(CLOCK_UNRELIABLE_THRESHOLD_MS)),
            Some(CLOCK_UNRELIABLE_THRESHOLD_MS)
        );
        assert_eq!(measured_skew(Some(0)), Some(0));
    }
}
