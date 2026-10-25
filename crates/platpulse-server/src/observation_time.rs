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

/// How current a Current Projection observation must be, in seconds.
///
/// One number, read by every judgement of whether a Node is current: the Public
/// projection, the Admin projection, the Observed Network Head source filter and
/// the head-lag alerts. A rule that judged currency with a different number
/// would answer a different question than the view the Owner is looking at
/// (ADR 0014, issue #274).
pub const FRESHNESS_LIMIT_SECONDS: i64 = 120;

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
        // The arithmetic also rounds to the second precision every stored
        // instant uses, because the rest of the Server compares them as strings.
        (Some(observed), Some(skew)) => corrected_instant(observed, skew)
            .map(crate::auth::format_rfc3339)
            .unwrap_or_else(|| fallback.to_owned()),
        _ => fallback.to_owned(),
    }
}

/// The Server-clock instant to stamp a history fact with.
///
/// Unlike [`observation_instant`], a history fact is never moved onto the receipt
/// instant: the receipt says when the Server learned the fact, so a history row
/// stamped with it would claim the Node did something at the moment the Report
/// happened to arrive. An untrustworthy skew therefore leaves the Agent's own
/// instant in place, exactly as observed, and a trustworthy one corrects the
/// fact onto the Server clock — the same correction every Current Projection and
/// every Peer presence interval already carries (ADR 0014).
pub fn history_instant(observed: Rfc3339, skew_ms: Option<i64>) -> Rfc3339 {
    match measured_skew(skew_ms) {
        Some(skew) => corrected_instant(observed, skew)
            .and_then(|corrected| crate::auth::format_rfc3339(corrected).parse().ok())
            .unwrap_or(observed),
        None => observed,
    }
}

/// `observed` on the Server clock, at the second precision every stored instant
/// uses. `None` when the calendar cannot hold the corrected instant.
///
/// The correction is **rounded** to the nearest second, not truncated. A stored
/// instant has second precision, so a correction smaller than a second cannot be
/// represented: truncating a 3 ms skew would turn `…T12:00:00Z` into
/// `…T11:59:59Z`, a whole second of error for a correction a thousand times
/// smaller, and would move in either direction depending on where the Agent's
/// own instant happens to fall inside its second.
///
/// Rounding also keeps a metric sample's identity stable. A sample's
/// observation instant is its identity — the raw row's key, the series ledger's
/// high-water mark and the aggregate buckets all key on it — while the skew is
/// re-measured in every Report, and the Agent carries a last-good observation
/// into later Reports. A sub-second difference between two readings must
/// therefore leave the identity alone; with truncation, a carry could land on
/// the neighbouring second and be counted as a second observation (#213).
fn corrected_instant(observed: Rfc3339, skew_ms: i64) -> Option<time::OffsetDateTime> {
    let corrected = observed
        .as_datetime()
        .checked_sub(time::Duration::milliseconds(skew_ms))?;
    let rounded = corrected
        .checked_add(time::Duration::milliseconds(500))
        .unwrap_or(corrected);
    rounded.replace_nanosecond(0).ok()
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
        .is_some_and(|at| is_fresh_at(at, now, window_seconds))
}

/// [`is_fresh`] for an instant that is already parsed.
pub fn is_fresh_at(
    at: time::OffsetDateTime,
    now: time::OffsetDateTime,
    window_seconds: i64,
) -> bool {
    (now - at).whole_seconds().abs() <= window_seconds
}

/// `instant` moved by `seconds` on the Server calendar.
///
/// Returns `None` when the instant is unusable or the calendar cannot hold the
/// result. Both the freshness boundary and a metric bucket's end are derived
/// from an instant an Agent supplied, so neither may use `time`'s unchecked
/// `Add`: a Report that legally stamps `9999-12-31T23:59:59Z` would panic the
/// request that renders it (AGENTS.md: the Server is the trust boundary).
pub fn shift_instant(instant: &str, seconds: i64) -> Option<time::OffsetDateTime> {
    crate::auth::parse_rfc3339(instant)
        .and_then(|at| at.checked_add(time::Duration::seconds(seconds)))
        .and_then(|shifted| shifted.replace_nanosecond(0).ok())
}

/// The instant at which an observation left its freshness window, which is what
/// a Stale reading reports for "since when".
///
/// `None` when there is no instant to name or the boundary is not representable;
/// the caller then states Staleness without a boundary rather than failing the
/// request.
pub fn stale_since(instant: Option<&str>, window_seconds: i64) -> Option<String> {
    instant
        .and_then(|instant| shift_instant(instant, window_seconds))
        .map(crate::auth::format_rfc3339)
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
    fn a_sub_second_skew_rounds_to_the_nearest_second_instead_of_shifting_one() {
        // A stored instant has second precision, so a correction below half a
        // second cannot be represented and must leave the instant where it is.
        // Truncating it instead would move a sample a whole second back, and a
        // last-good observation carried into a Report that re-measured its skew
        // would then land on a different instant — a different identity, which
        // the series ledger counts as a second observation (#213).
        assert_eq!(
            observation_instant(
                Some(instant("2026-08-12T09:00:00Z")),
                Some(320),
                "2026-08-12T10:00:00Z",
            ),
            "2026-08-12T09:00:00Z"
        );
        assert_eq!(
            history_instant(instant("2026-08-12T09:00:00Z"), Some(-320)).to_string(),
            "2026-08-12T09:00:00Z"
        );
        // Past the half second the correction is real, and it rounds away from
        // the observation rather than toward it.
        assert_eq!(
            observation_instant(
                Some(instant("2026-08-12T09:00:00Z")),
                Some(1_500),
                "2026-08-12T10:00:00Z",
            ),
            "2026-08-12T08:59:59Z"
        );
        assert_eq!(
            history_instant(instant("2026-08-12T09:00:00Z"), Some(-1_200)).to_string(),
            "2026-08-12T09:00:01Z"
        );
        // Whatever the skew, the stored instant never keeps a sub-second part:
        // every other stored instant is second-precision and compared as text.
        for skew in [-1_500, -320, 0, 320, 1_500] {
            assert_eq!(
                history_instant(instant("2026-08-12T09:00:00.750Z"), Some(skew))
                    .as_datetime()
                    .nanosecond(),
                0
            );
        }
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
    fn a_boundary_the_calendar_cannot_hold_is_not_a_boundary() {
        // The same Agent-supplied edge reaches the freshness boundary and the
        // metric bucket end; both must answer "no boundary", never panic.
        assert_eq!(shift_instant("9999-12-31T23:59:59Z", 120), None);
        assert_eq!(stale_since(Some("9999-12-31T23:59:59Z"), 120), None);
        assert_eq!(
            stale_since(Some("2026-08-12T08:00:00Z"), 120).as_deref(),
            Some("2026-08-12T08:02:00Z")
        );
        assert_eq!(stale_since(None, 120), None);
        assert_eq!(stale_since(Some("not an instant"), 120), None);
    }

    #[test]
    fn a_history_fact_is_corrected_or_left_as_observed_but_never_receipt_stamped() {
        // A measured skew moves the history fact onto the Server clock, the same
        // way a Current Projection moves.
        assert_eq!(
            history_instant(instant("2026-08-12T09:03:00Z"), Some(180_000)).to_string(),
            "2026-08-12T09:00:00Z"
        );
        assert_eq!(
            history_instant(instant("2026-08-12T08:57:00Z"), Some(-180_000)).to_string(),
            "2026-08-12T09:00:00Z"
        );
        // An untrustworthy skew, a missing one, and an instant the calendar
        // cannot correct all keep the Agent's own instant: a history fact is
        // never rewritten onto the receipt instant.
        let observed = instant("2026-08-12T09:00:00Z");
        let unreliable = CLOCK_UNRELIABLE_THRESHOLD_MS + 1;
        assert_eq!(history_instant(observed, Some(unreliable)), observed);
        assert_eq!(history_instant(observed, None), observed);
        let edge = instant("9999-12-31T23:59:59Z");
        assert_eq!(history_instant(edge, Some(-1_000)), edge);
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
