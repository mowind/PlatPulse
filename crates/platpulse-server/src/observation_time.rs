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
        // `observed` comes from the Agent, so the Server cannot assume it sits
        // comfortably inside the calendar: a Report may legally stamp
        // 9999-12-31T23:59:59Z, and correcting that by a millisecond overflows
        // `time` (its `Sub` panics). An instant the Server cannot correct is an
        // instant it cannot judge, so it falls back like an unmeasured skew.
        // The arithmetic also drops sub-second precision, because every other
        // stored instant is second-precision and the rest of the Server compares
        // them as strings.
        (Some(observed), Some(skew)) => observed
            .as_datetime()
            .checked_sub(time::Duration::milliseconds(skew))
            .and_then(|corrected| corrected.replace_nanosecond(0).ok())
            .map(crate::auth::format_rfc3339)
            .unwrap_or_else(|| fallback.to_owned()),
        _ => fallback.to_owned(),
    }
}

/// Whether `instant` sits within `window_seconds` of `now`, counting both
/// directions.
///
/// The Public projection, the Admin projection and the Observed Network Head
/// source filter all ask the same question, and they must read the same answer:
/// a future instant is as untrustworthy as an old one, so the window is
/// symmetric (ADR 0014, issue #274).
pub fn is_fresh(instant: Option<&str>, now: time::OffsetDateTime, window_seconds: i64) -> bool {
    instant
        .and_then(crate::auth::parse_rfc3339)
        .is_some_and(|at| (now - at).whole_seconds().abs() <= window_seconds)
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
    fn an_instant_the_calendar_cannot_hold_falls_back_to_the_receipt_instant() {
        // A Report may legally stamp the last representable second; correcting
        // it by a measured skew must fall back, never panic.
        assert_eq!(
            observation_instant(
                Some(instant("9999-12-31T23:59:59Z")),
                Some(-1_000),
                "2026-08-12T10:00:00Z"
            ),
            "2026-08-12T10:00:00Z"
        );
    }

    #[test]
    fn freshness_is_a_window_on_both_sides_of_now() {
        let now = instant("2026-08-12T10:00:00Z").as_datetime();
        assert!(is_fresh(Some("2026-08-12T09:58:00Z"), now, 120));
        assert!(is_fresh(Some("2026-08-12T10:02:00Z"), now, 120));
        assert!(!is_fresh(Some("2026-08-12T09:57:59Z"), now, 120));
        assert!(!is_fresh(Some("2026-08-12T10:02:01Z"), now, 120));
        assert!(!is_fresh(None, now, 120));
        assert!(!is_fresh(Some("2026-08-12T09:58"), now, 120));
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
