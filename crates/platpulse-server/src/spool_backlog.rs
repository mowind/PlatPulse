//! Spool Backlog Age derived at Report receipt (ADR 0014, issue #272).
//!
//! A Report is immutable, so the `SpoolDiagnostics.oldest_queued_age_ms` it
//! carries describes the Agent's backlog at generation time. By the time the
//! Server acknowledges a replayed Report the real backlog may be hours
//! younger, and the Server is the only party that knows when the bytes
//! actually arrived. This module derives the single reading the Admin views
//! and Server Doctor report, from:
//!
//! * the Report's own generation instant,
//! * the Server receipt instant, and
//! * the Agent→Server clock skew the same Report measured.
//!
//! When the skew is measured and reliable the reading is anchored on the
//! Server clock, so a replay of a two-hour-old Report reports roughly two
//! hours. When it is missing or the clock is unreliable the reading falls
//! back to the Report's own age, which is still exact: both of its ends are
//! Agent-clock instants, so the offset cancels. The two bases are never
//! silently mixed — the caller stores which one produced the number so the
//! WebUI can label it (ADR 0014).

use platpulse_core::observation::SpoolDiagnostics;
use platpulse_core::time::Rfc3339;

/// The Spool Backlog Age is above this many milliseconds a warning: an Agent
/// that has been waiting longer than that is not merely catching up after a
/// short Server restart. Server Doctor warns (never fails) on it, because a
/// near-overflow backlog is already an `agent_spool_overflow` Attention item.
pub const SPOOL_BACKLOG_WARNING_MS: i64 = 5 * 60 * 1000;

/// Which clock basis produced a Spool Backlog Age.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BacklogBasis {
    /// Server receipt time minus the skew-corrected generation instant.
    Receipt,
    /// The Report's own measurement, which is as of its generation instant.
    Generation,
}

impl BacklogBasis {
    /// The stable wire and storage spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Receipt => "receipt",
            Self::Generation => "generation",
        }
    }
}

/// A Spool Backlog Age reading and the clock basis behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpoolBacklogAge {
    /// Milliseconds since the oldest queued Report was generated, never
    /// negative.
    pub age_ms: i64,
    /// The clock basis the reading is anchored on.
    pub basis: BacklogBasis,
}

/// Whether an age is past the Doctor warning threshold. Server Doctor is the
/// only production caller, and it holds the stored age rather than a
/// `SpoolBacklogAge`, so the predicate is shared from here instead of being
/// restated against the raw column.
pub fn is_backlogged(age_ms: i64) -> bool {
    age_ms > SPOOL_BACKLOG_WARNING_MS
}

/// Derive the Spool Backlog Age of one Report.
///
/// Returns `None` when the Report carries no Spool component at all: the
/// reading describes the Agent's Durable Spool, so an Agent that did not attest
/// one has no backlog for the Server to claim. The timestamps alone are
/// deliberately not enough there — a delayed Report that attests no Spool can
/// describe a paused Agent or a transport stall rather than a queue — and an
/// unknown reading is never reported as zero.
///
/// `clock_skew_ms` is the Agent→Server skew the same Report measured
/// (positive = Agent clock ahead, as `agents.clock_skew_ms` defines it).
pub fn spool_backlog_age(
    spool: Option<&SpoolDiagnostics>,
    generated_at: Rfc3339,
    received_at: Rfc3339,
    clock_skew_ms: Option<i64>,
) -> Option<SpoolBacklogAge> {
    let spool = spool?;
    // The trust rule is shared with the observation instants of issue #274, so
    // Clock Unreliable means the same thing everywhere: `measured_skew` also
    // keeps a pathological i64::MIN skew from overflowing the comparison.
    match crate::observation_time::measured_skew(clock_skew_ms) {
        Some(skew) => {
            // The Report was generated at Agent-clock `generated_at`, which on
            // the Server clock is `generated_at - skew`.
            let transit_ms = (received_at.as_datetime() - generated_at.as_datetime())
                .whole_milliseconds()
                .clamp(i64::MIN as i128, i64::MAX as i128) as i64;
            // A Report that arrives "before" it was generated cannot have a
            // real backlog; the sub-second truncation of both stamps is the
            // only way to get there, so clamp instead of reporting a negative
            // age.
            Some(SpoolBacklogAge {
                age_ms: transit_ms.saturating_add(skew).max(0),
                basis: BacklogBasis::Receipt,
            })
        }
        None => Some(SpoolBacklogAge {
            age_ms: i64::try_from(spool.oldest_queued_age_ms).unwrap_or(i64::MAX),
            basis: BacklogBasis::Generation,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::agent::CLOCK_UNRELIABLE_THRESHOLD_MS;

    fn stamp(value: &str) -> Rfc3339 {
        value.parse().expect("test timestamp is valid")
    }

    fn spool_with_age(age_ms: u64) -> SpoolDiagnostics {
        SpoolDiagnostics {
            queued_bytes: 0,
            queued_reports: 1,
            oldest_queued_age_ms: age_ms,
            dropped_reports: 0,
            dropped_samples: 0,
            in_flight: None,
            last_delivery_error: None,
            last_delivery_at: None,
            capacity_bytes: None,
            max_age_seconds: None,
            dropped_sequence_range: None,
            dropped_time_range: None,
            dropped_height_range: None,
            pending_history_gaps: None,
            report_too_large: None,
            store_fatal: None,
            store_error: None,
            shutdown_state: None,
            shutdown_started_at: None,
            shutdown_deadline_at: None,
            shutdown_finished_at: None,
            shutdown_unresolved_range: None,
            shutdown_last_error: None,
            shutdown_forced: None,
            shutdown_report_id: None,
        }
    }

    #[test]
    fn replay_of_a_two_hour_old_report_reports_two_hours_not_the_stale_reading() {
        // The Agent queued the Report two hours ago and its diagnostics then
        // said "the oldest queued Report is 5 s old"; the replay arrives now.
        let age = spool_backlog_age(
            Some(&spool_with_age(5_000)),
            stamp("2026-08-12T10:00:00Z"),
            stamp("2026-08-12T12:00:00Z"),
            Some(0),
        )
        .expect("a Report with a Spool component has a reading");
        assert_eq!(age.basis, BacklogBasis::Receipt);
        assert_eq!(age.age_ms, 7_200_000);
        assert!(is_backlogged(age.age_ms));
    }

    #[test]
    fn receipt_reading_corrects_a_measured_agent_clock_skew() {
        // The Agent clock runs one minute ahead, so the instant named by
        // `generated_at` was one minute earlier on the Server clock.
        let generated = stamp("2026-08-12T10:00:00Z");
        let received = stamp("2026-08-12T12:00:00Z");
        let ahead = spool_backlog_age(Some(&spool_with_age(0)), generated, received, Some(60_000));
        let behind =
            spool_backlog_age(Some(&spool_with_age(0)), generated, received, Some(-60_000));
        assert_eq!(ahead.expect("reading").age_ms, 7_260_000);
        assert_eq!(behind.expect("reading").age_ms, 7_140_000);
    }

    #[test]
    fn unreliable_clock_falls_back_to_the_reports_own_age() {
        let unreliable = CLOCK_UNRELIABLE_THRESHOLD_MS + 1;
        for skew in [unreliable, -unreliable] {
            let age = spool_backlog_age(
                Some(&spool_with_age(900_000)),
                stamp("2026-08-12T10:00:00Z"),
                stamp("2026-08-12T12:00:00Z"),
                Some(skew),
            )
            .expect("reading");
            assert_eq!(age.basis, BacklogBasis::Generation);
            assert_eq!(age.age_ms, 900_000);
        }
    }

    #[test]
    fn unmeasured_clock_falls_back_to_the_reports_own_age() {
        let age = spool_backlog_age(
            Some(&spool_with_age(900_000)),
            stamp("2026-08-12T10:00:00Z"),
            stamp("2026-08-12T12:00:00Z"),
            None,
        )
        .expect("reading");
        assert_eq!(age.basis, BacklogBasis::Generation);
        assert_eq!(age.age_ms, 900_000);
    }

    #[test]
    fn a_skew_exactly_at_the_reliability_threshold_is_still_trusted() {
        let age = spool_backlog_age(
            Some(&spool_with_age(0)),
            stamp("2026-08-12T10:00:00Z"),
            stamp("2026-08-12T12:00:00Z"),
            Some(CLOCK_UNRELIABLE_THRESHOLD_MS),
        )
        .expect("reading");
        assert_eq!(age.basis, BacklogBasis::Receipt);
    }

    #[test]
    fn a_report_without_a_spool_reading_has_no_backlog_age() {
        assert_eq!(
            spool_backlog_age(
                None,
                stamp("2026-08-12T10:00:00Z"),
                stamp("2026-08-12T12:00:00Z"),
                Some(0)
            ),
            None
        );
    }

    #[test]
    fn a_backlog_age_is_never_negative() {
        let age = spool_backlog_age(
            Some(&spool_with_age(0)),
            stamp("2026-08-12T12:00:00Z"),
            stamp("2026-08-12T12:00:00Z"),
            Some(-5_000),
        )
        .expect("reading");
        assert_eq!(age.age_ms, 0);
        assert!(!is_backlogged(age.age_ms));
    }

    #[test]
    fn the_warning_threshold_is_exclusive() {
        assert!(!is_backlogged(SPOOL_BACKLOG_WARNING_MS));
        assert!(is_backlogged(SPOOL_BACKLOG_WARNING_MS + 1));
    }

    #[test]
    fn a_report_huge_queued_age_does_not_overflow() {
        let age = spool_backlog_age(
            Some(&spool_with_age(u64::MAX)),
            stamp("2026-08-12T10:00:00Z"),
            stamp("2026-08-12T12:00:00Z"),
            None,
        )
        .expect("reading");
        assert_eq!(age.age_ms, i64::MAX);
    }
}
