//! Activity, verdict, freshness and Current Validator Status projection.

use time::OffsetDateTime;

/// Project the canonical last-good Activity and its currency for a projection
/// surface. Provider outcomes never fabricate a value: authoritative absence is
/// Observing, a successful snapshot shows the canonical label (Stale when
/// Server freshness expired), and Error with a last-good Activity is always
/// Stale. Unsupported coverage projects Unknown even when a last-good Activity
/// was previously observed (#100, #101, #168).
pub fn project_activity(
    outcome: &str,
    activity: Option<&str>,
    freshness: &str,
) -> (String, String) {
    match outcome {
        "empty" => (
            "observing".to_owned(),
            match freshness {
                "fresh" => "current",
                "stale" => "stale",
                _ => "unknown",
            }
            .to_owned(),
        ),
        // The deployment answers an absent staking identity with a 200 empty
        // object; a 404 can only come from routing or a deployment anomaly, so
        // it is never presented as an observing Validator (#168).
        "not_found" => ("unknown".to_owned(), "unknown".to_owned()),
        "success" => match activity {
            Some(value) => (
                value.to_owned(),
                match freshness {
                    "fresh" => "current",
                    "stale" => "stale",
                    _ => "unknown",
                }
                .to_owned(),
            ),
            None => ("unknown".to_owned(), "unknown".to_owned()),
        },
        "error" => match activity {
            Some(value) => (value.to_owned(), "stale".to_owned()),
            None => ("unknown".to_owned(), "unknown".to_owned()),
        },
        "unsupported" | "not_configured" => ("unknown".to_owned(), "unknown".to_owned()),
        _ => ("unknown".to_owned(), "unknown".to_owned()),
    }
}

/// Current Validator Status of an automatically identified Node identity: the
/// currently valid staking identity, not current consensus selection or Node
/// Health (#173, main design §15.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentValidatorStatus {
    Validator,
    NotValidator,
    Unknown,
}

impl CurrentValidatorStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CurrentValidatorStatus::Validator => "validator",
            CurrentValidatorStatus::NotValidator => "not_validator",
            CurrentValidatorStatus::Unknown => "unknown",
        }
    }
}

/// The special state shown while a staking identity is confirmed valid but is
/// not normally participating: a low-production lock, or a withdrawal that has
/// not completed. Neither is a negative verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentValidatorQualifier {
    Locked,
    Exiting,
}

impl CurrentValidatorQualifier {
    pub fn as_str(&self) -> &'static str {
        match self {
            CurrentValidatorQualifier::Locked => "locked",
            CurrentValidatorQualifier::Exiting => "exiting",
        }
    }
}

/// Server-owned Current Validator Status plus its currency. `state` is
/// `current`, `stale`, or `unknown`; a retained last-good value after a
/// failed refresh is never presented as fresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentValidatorStatusView {
    pub status: CurrentValidatorStatus,
    pub state: &'static str,
    pub qualifier: Option<CurrentValidatorQualifier>,
}

fn classify_activity(
    activity: Option<&str>,
) -> (CurrentValidatorStatus, Option<CurrentValidatorQualifier>) {
    match activity {
        Some("candidate") | Some("active") | Some("producing") => {
            (CurrentValidatorStatus::Validator, None)
        }
        Some("exiting") => (
            CurrentValidatorStatus::Validator,
            Some(CurrentValidatorQualifier::Exiting),
        ),
        Some("locked") => (
            CurrentValidatorStatus::Validator,
            Some(CurrentValidatorQualifier::Locked),
        ),
        Some("exited") | Some("observing") => (CurrentValidatorStatus::NotValidator, None),
        // The investigated source maps 6 to "candidate in a consensus round"
        // (getCodeByStatus(CANDIDATE, isConsensus = 1, *)): the stake is in
        // force and only the consensus-round dimension differs. Reading it as
        // Unknown made a Node taking part in the current round look less
        // confirmed than an idle candidate and made the Home Validator bucket
        // flap, so it classifies as Validator (Owner decision 2026-10-08,
        // amending CONTEXT.md and ADR 0005; see section 4.2 of the #168 evidence note).
        Some("verifying") => (CurrentValidatorStatus::Validator, None),
        _ => (CurrentValidatorStatus::Unknown, None),
    }
}

/// Map the canonical last-good outcome and Activity to Current Validator
/// Status using only the evidence predicates established in #168. HTTP 404 is
/// not a negative: the deployment answers absence with a 200 empty form, so a
/// 404 can only be a routing or deployment anomaly and stays Unknown.
pub fn current_validator_status(
    outcome: Option<&str>,
    activity: Option<&str>,
    freshness: &str,
) -> CurrentValidatorStatusView {
    let unknown = || CurrentValidatorStatusView {
        status: CurrentValidatorStatus::Unknown,
        state: "unknown",
        qualifier: None,
    };
    match outcome.unwrap_or("") {
        "success" => {
            let (status, qualifier) = classify_activity(activity);
            if status == CurrentValidatorStatus::Unknown {
                return unknown();
            }
            let state = match freshness {
                "fresh" => "current",
                "stale" => "stale",
                _ => "unknown",
            };
            CurrentValidatorStatusView {
                status,
                state,
                qualifier,
            }
        }
        // Strictly validated 200/code 0/empty nodeId/status 0: authoritative
        // absence of current staking identity.
        "empty" => CurrentValidatorStatusView {
            status: CurrentValidatorStatus::NotValidator,
            state: match freshness {
                "fresh" => "current",
                "stale" => "stale",
                _ => "unknown",
            },
            qualifier: None,
        },
        // A failed refresh with a retained last-good Activity is stale; the
        // verdict is retained but never presented as fresh.
        "error" => {
            let (status, qualifier) = classify_activity(activity);
            if status == CurrentValidatorStatus::Unknown {
                return unknown();
            }
            CurrentValidatorStatusView {
                status,
                state: "stale",
                qualifier,
            }
        }
        _ => unknown(),
    }
}

/// Project only a confirmed verdict, independently of detail metrics and the
/// latest attempt. A failed/partial refresh cannot revive activity superseded by
/// authoritative absence, and ambiguous pre-migration failures remain Unknown.
pub fn project_verdict(
    outcome: &str,
    last_good_outcome: Option<&str>,
    activity: Option<&str>,
    verdict_freshness: &str,
) -> (String, String, CurrentValidatorStatusView) {
    // These responses cannot classify an identity. Retain persisted evidence
    // for a later recoverable refresh, but preserve their Unknown presentation.
    let last_good_outcome = if matches!(outcome, "not_found" | "unsupported" | "not_configured") {
        None
    } else {
        last_good_outcome
    };
    let (evidence_outcome, evidence_activity) = match last_good_outcome {
        Some("empty") => ("empty", Some("observing")),
        Some("success") => ("success", activity),
        _ => ("unknown", None),
    };
    let freshness = if evidence_outcome == "unknown" || verdict_freshness == "unknown" {
        "unknown"
    } else if outcome == "error"
        || outcome == "not_found"
        || outcome == "unsupported"
        || outcome == "not_configured"
    {
        "stale"
    } else {
        verdict_freshness
    };
    let projected_outcome = if freshness == "unknown" {
        "unknown"
    } else {
        evidence_outcome
    };
    let (activity, activity_state) =
        project_activity(projected_outcome, evidence_activity, freshness);
    let status = current_validator_status(Some(projected_outcome), evidence_activity, freshness);
    (activity, activity_state, status)
}

/// Age of the last successful Provider observation in whole seconds, or `None`
/// when no last-good value was ever recorded. The Server owns this arithmetic
/// so every projection reports last-good age the same way, and it is never
/// rendered as 0 for a Validator that has never had a successful refresh
/// (#218).
pub fn last_good_age_seconds(
    last_good_received_at: Option<&str>,
    now: OffsetDateTime,
) -> Option<i64> {
    let received_at = last_good_received_at.and_then(crate::auth::parse_rfc3339)?;
    Some((now - received_at).whole_seconds().max(0))
}

/// Freshness follows the configured Provider refresh interval rather than a
/// fixed constant: a last-good observation is fresh while it is no older
/// than `stale_after_seconds`, which defaults to two refresh intervals
/// (120s at the 60s default refresh). This keeps a legitimate slower refresh
/// setting from immediately looking stale (#154).
pub fn freshness(
    last_good_received_at: Option<&str>,
    now: OffsetDateTime,
    stale_after_seconds: i64,
) -> &'static str {
    let Some(received_at) = last_good_received_at.and_then(crate::auth::parse_rfc3339) else {
        return "unknown";
    };
    if (now - received_at).whole_seconds().abs() <= stale_after_seconds.max(1) {
        "fresh"
    } else {
        "stale"
    }
}

#[cfg(test)]
mod tests {

    use time::OffsetDateTime;

    use super::*;

    #[test]
    fn freshness_window_defaults_to_two_refresh_intervals() {
        let now = crate::auth::now_utc();
        let fresh_at = crate::auth::format_rfc3339(now - time::Duration::seconds(119));
        let stale_at = crate::auth::format_rfc3339(now - time::Duration::seconds(121));
        assert_eq!(freshness(Some(&fresh_at), now, 120), "fresh");
        assert_eq!(freshness(Some(&stale_at), now, 120), "stale");
        // A slower configured refresh interval keeps the same age fresh.
        assert_eq!(freshness(Some(&stale_at), now, 600), "fresh");
        assert_eq!(freshness(None, now, 600), "unknown");
    }

    #[test]
    fn project_activity_follows_the_evidence_predicates() {
        // Authoritative absence is Observing, never Unknown and never a
        // fabricated Activity.
        assert_eq!(
            project_activity("empty", None, "fresh"),
            ("observing".to_owned(), "current".to_owned())
        );
        // A 404 is a routing or deployment anomaly, never an observing
        // Validator (#168).
        assert_eq!(
            project_activity("not_found", Some("active"), "fresh"),
            ("unknown".to_owned(), "unknown".to_owned())
        );
        assert_eq!(
            project_activity("success", Some("producing"), "fresh"),
            ("producing".to_owned(), "current".to_owned())
        );
        assert_eq!(
            project_activity("success", Some("producing"), "stale"),
            ("producing".to_owned(), "stale".to_owned())
        );
        // A success without a canonical Activity stays Unknown rather than
        // inheriting an older label.
        assert_eq!(
            project_activity("success", None, "fresh"),
            ("unknown".to_owned(), "unknown".to_owned())
        );
        // A failed refresh retains the last-good Activity, marked Stale.
        assert_eq!(
            project_activity("error", Some("active"), "fresh"),
            ("active".to_owned(), "stale".to_owned())
        );
        assert_eq!(
            project_activity("error", None, "fresh"),
            ("unknown".to_owned(), "unknown".to_owned())
        );
        // Unsupported or unconfigured coverage never projects a stale label.
        assert_eq!(
            project_activity("unsupported", Some("active"), "stale"),
            ("unknown".to_owned(), "unknown".to_owned())
        );
        assert_eq!(
            project_activity("not_configured", None, "unknown"),
            ("unknown".to_owned(), "unknown".to_owned())
        );
    }

    #[test]
    fn last_good_age_is_absent_without_a_last_good_value() {
        let now = OffsetDateTime::parse(
            "2026-01-01T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        assert_eq!(last_good_age_seconds(None, now), None);
        assert_eq!(last_good_age_seconds(Some("not a timestamp"), now), None);
        assert_eq!(
            last_good_age_seconds(Some("2025-12-31T23:58:30Z"), now),
            Some(90)
        );
        // A clock skew never renders a negative age.
        assert_eq!(
            last_good_age_seconds(Some("2026-01-01T00:00:05Z"), now),
            Some(0)
        );
    }

    #[test]
    fn current_validator_status_follows_the_evidence_predicates() {
        let view = |outcome: Option<&str>, activity: Option<&str>| {
            current_validator_status(outcome, activity, "fresh")
        };
        assert_eq!(
            view(Some("success"), Some("active")).status,
            CurrentValidatorStatus::Validator
        );
        // A candidate is a currently valid staking identity: it stays Validator,
        // it is only a distinct Activity from Active.
        assert_eq!(
            view(Some("success"), Some("candidate")).status,
            CurrentValidatorStatus::Validator
        );
        assert_eq!(view(Some("success"), Some("producing")).state, "current");
        assert_eq!(
            view(Some("success"), Some("exiting")).qualifier,
            Some(CurrentValidatorQualifier::Exiting)
        );
        assert_eq!(
            view(Some("success"), Some("locked")).qualifier,
            Some(CurrentValidatorQualifier::Locked)
        );
        assert_eq!(
            view(Some("success"), Some("exited")).status,
            CurrentValidatorStatus::NotValidator
        );
        // Verifying is a candidate in a consensus round: a currently valid
        // staking identity, so Validator (Owner decision 2026-10-08).
        assert_eq!(
            view(Some("success"), Some("verifying")).status,
            CurrentValidatorStatus::Validator
        );
        // A success without an Activity, an unrecognized status, or a status 0
        // with a non-empty identifier cannot be fabricated into a negative.
        assert_eq!(
            view(Some("success"), None).status,
            CurrentValidatorStatus::Unknown
        );
        // Authoritative absence is negative; HTTP 404 is not.
        assert_eq!(
            view(Some("empty"), None).status,
            CurrentValidatorStatus::NotValidator
        );
        assert_eq!(
            view(Some("not_found"), None).status,
            CurrentValidatorStatus::Unknown
        );
        // A failed refresh retains the last-good verdict but marks it stale.
        let stale = current_validator_status(Some("error"), Some("active"), "stale");
        assert_eq!(stale.status, CurrentValidatorStatus::Validator);
        assert_eq!(stale.state, "stale");
        assert_eq!(
            current_validator_status(Some("error"), None, "unknown").status,
            CurrentValidatorStatus::Unknown
        );
        // Unconfigured or unsupported coverage never establishes a verdict,
        // even with a retained last-good Activity.
        assert_eq!(
            current_validator_status(Some("not_configured"), None, "unknown").status,
            CurrentValidatorStatus::Unknown
        );
        assert_eq!(
            current_validator_status(Some("unsupported"), Some("active"), "unknown").status,
            CurrentValidatorStatus::Unknown
        );
    }
}
