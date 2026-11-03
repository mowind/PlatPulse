//! Provider refresh: applies detail and ranking results to the insight tables.

use crate::database::ServerDatabase;
use chrono_tz::Tz;
use sha2::{Digest, Sha256};
use sqlx::{Any, Transaction};
use std::collections::BTreeMap;

use super::analytics::{
    StoredDay, rebuild_monthly_aggregate, record_daily_snapshot, record_snapshot_rank,
};
use super::decimal::{decimal_decreased, decimal_exceeds_max, validate_nonnegative_decimal};
use super::error::ValidatorError;
use super::insight::{INSIGHT_SELECT, ValidatorInsightRecord};
use super::platscan::normalize_bounded_text;
use super::provider::{
    RankingProviderResult, ValidatorObservation, ValidatorProvider, ValidatorProviderResult,
    provider_diagnostic,
};
use super::registry::canonical_timestamp;

fn observation_key(observation: &ValidatorObservation) -> String {
    let bytes = format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        observation.provider_timestamp,
        observation.activity,
        observation.stake_amount,
        observation.reward_amount,
        observation.reward_rate,
        observation.delegation_reward_percentage,
        observation.delegator_count,
        observation.epoch,
        observation.block_count,
        observation.expected_block_count,
        observation.gen_blocks_rate
    );
    let mut hash = Sha256::new();
    hash.update(bytes.as_bytes());
    format!("{:x}", hash.finalize())
}

fn counter_decreases(
    existing: Option<&ValidatorInsightRecord>,
    observation: &ValidatorObservation,
) -> Vec<(&'static str, String, String)> {
    let Some(existing) = existing else {
        return Vec::new();
    };
    let mut decreases = Vec::new();
    if decimal_decreased(
        existing.stake_amount.as_deref(),
        observation.stake_amount.as_deref(),
    ) {
        decreases.push((
            "stake_amount",
            existing.stake_amount.clone().unwrap_or_default(),
            observation.stake_amount.clone().unwrap_or_default(),
        ));
    }
    if decimal_decreased(
        existing.reward_amount.as_deref(),
        observation.reward_amount.as_deref(),
    ) {
        decreases.push((
            "reward_amount",
            existing.reward_amount.clone().unwrap_or_default(),
            observation.reward_amount.clone().unwrap_or_default(),
        ));
    }
    if matches!(
        (existing.block_count, observation.block_count),
        (Some(previous), Some(current)) if current < previous
    ) {
        decreases.push((
            "block_count",
            existing.block_count.unwrap_or_default().to_string(),
            observation.block_count.unwrap_or_default().to_string(),
        ));
    }
    decreases
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshSummary {
    pub attempted: usize,
    pub successful: usize,
    pub changed: usize,
    pub invalidations: usize,
    pub alert_invalidations: usize,
    pub invalidated_network_keys: Vec<String>,
    pub invalidated_validator_ids: Vec<String>,
}

pub async fn refresh_all(
    db: &ServerDatabase,
    provider: &dyn ValidatorProvider,
) -> Result<RefreshSummary, ValidatorError> {
    refresh_all_with_channels_in_timezone(
        db,
        provider,
        &crate::config::NotificationChannels::default(),
        "UTC",
    )
    .await
}

pub async fn refresh_all_with_channels(
    db: &ServerDatabase,
    provider: &dyn ValidatorProvider,
    channels: &crate::config::NotificationChannels,
) -> Result<RefreshSummary, ValidatorError> {
    refresh_all_with_channels_in_timezone(db, provider, channels, "UTC").await
}

pub async fn refresh_all_with_channels_in_timezone(
    db: &ServerDatabase,
    provider: &dyn ValidatorProvider,
    channels: &crate::config::NotificationChannels,
    timezone: &str,
) -> Result<RefreshSummary, ValidatorError> {
    timezone
        .parse::<Tz>()
        .map_err(|_| ValidatorError::InvalidTimezone(timezone.to_owned()))?;
    let validators = sqlx::query_as::<_, (String, String, String)>(
        "SELECT validator_id, network_key, validator_node_id FROM validators ORDER BY validator_id",
    )
    .fetch_all(db.pool())
    .await?;
    let mut network_keys: Vec<String> = Vec::new();
    let mut provider_results = Vec::with_capacity(validators.len());
    for (validator_id, network_key, validator_node_id) in validators {
        let result = provider.fetch(&network_key, &validator_node_id).await;
        if !network_keys.contains(&network_key) {
            network_keys.push(network_key.clone());
        }
        provider_results.push((validator_id, network_key, validator_node_id, result));
    }

    // The dedicated ranking list is shared by every Validator on a Network:
    // it is fetched once per distinct Network, never once per monitored Node,
    // and it is not a fallback triggered by a detail failure (#158).
    let mut rankings: BTreeMap<String, RankingProviderResult> = BTreeMap::new();
    for network_key in &network_keys {
        let result = provider.fetch_ranking(network_key).await;
        rankings.insert(network_key.clone(), result);
    }

    let mut summary = RefreshSummary {
        attempted: provider_results.len(),
        ..RefreshSummary::default()
    };
    let mut tx = crate::database::begin(db.pool()).await?;
    for (validator_id, network_key, validator_node_id, result) in provider_results {
        let detail =
            apply_provider_result(&mut tx, provider.source(), &validator_id, result, timezone)
                .await?;
        // Every Network in this loop was collected above; the defensive arm
        // keeps a missing result a degraded outcome rather than a panic.
        let lookup = match rankings.get(&network_key) {
            Some(result) => ranking_lookup(result, &validator_node_id),
            None => NetworkRankingLookup::Error("ranking was not collected".to_owned()),
        };
        let (ranking_changed, ranking_invalidated) = apply_ranking_result(
            &mut tx,
            &validator_id,
            lookup,
            timezone,
            detail.stored_day.as_ref(),
        )
        .await?;
        let alert_changes = crate::alerts::evaluate_validator_in_transaction(
            &mut tx,
            &validator_id,
            channels,
            crate::auth::now_utc(),
        )
        .await
        .map_err(|error| ValidatorError::Alert(error.to_string()))?;
        if alert_changes > 0 {
            summary.alert_invalidations += alert_changes;
        }
        if detail.stored {
            summary.successful += 1;
        }
        if detail.activity_changed || ranking_changed {
            summary.changed += 1;
        }
        if detail.invalidated || ranking_invalidated {
            summary.invalidations += 1;
            summary.invalidated_network_keys.push(network_key);
            summary.invalidated_validator_ids.push(validator_id);
        }
    }
    tx.commit().await?;
    Ok(summary)
}

/// One Validator's view of the shared Network ranking result. The Network
/// list is fetched once and looked up per Validator; an absent identifier is
/// only `Unranked` when the whole list was retrieved completely (#158).
#[derive(Debug, Clone, PartialEq, Eq)]
enum NetworkRankingLookup {
    Ranked { rank: i64, cohort_size: i64 },
    Unranked { cohort_size: i64 },
    NotConfigured(String),
    Unsupported(String),
    Error(String),
}

/// The pending ranking-change candidate used only to debounce the
/// `validator.ranking_changed` alert. The displayed rank is adopted
/// immediately, so this state never delays what the API or the UI shows (#158).
#[derive(Debug, Clone, Default)]
struct RankingCandidate {
    previous_rank: Option<i64>,
    rank: Option<i64>,
    observations: i64,
    observed_at: Option<String>,
    provider_timestamp: Option<String>,
    observation_key: Option<String>,
}

fn ranking_lookup(result: &RankingProviderResult, validator_node_id: &str) -> NetworkRankingLookup {
    match result {
        RankingProviderResult::Success(ranking) => match ranking.entries.get(validator_node_id) {
            Some(rank) => NetworkRankingLookup::Ranked {
                rank: *rank,
                cohort_size: ranking.cohort_size,
            },
            None => NetworkRankingLookup::Unranked {
                cohort_size: ranking.cohort_size,
            },
        },
        RankingProviderResult::NotConfigured(value) => {
            NetworkRankingLookup::NotConfigured(value.clone())
        }
        RankingProviderResult::Unsupported(value) => {
            NetworkRankingLookup::Unsupported(value.clone())
        }
        RankingProviderResult::Error(value) => NetworkRankingLookup::Error(value.clone()),
    }
}

fn ranking_observation_key(rank: i64, cohort_size: i64) -> String {
    format!("ranking:{rank}/{cohort_size}")
}

/// What one detail read changed, and the configured local day it stored a
/// snapshot for (#219). `stored_day` is `None` when this cycle stored no
/// snapshot at all, which is what keeps same-cycle ranking evidence off days
/// nobody observed.
struct AppliedDetail {
    stored: bool,
    activity_changed: bool,
    invalidated: bool,
    stored_day: Option<StoredDay>,
}

async fn apply_provider_result(
    tx: &mut Transaction<'_, Any>,
    source: &str,
    validator_id: &str,
    result: ValidatorProviderResult,
    timezone: &str,
) -> Result<AppliedDetail, ValidatorError> {
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    let existing = sqlx::query_as::<_, ValidatorInsightRecord>(INSIGHT_SELECT)
        .bind(validator_id)
        .fetch_optional(&mut **tx)
        .await?;
    match result {
        ValidatorProviderResult::Success(observation) => {
            validate_observation(&observation)?;
            let key = observation_key(&observation);
            let stored_activity = match observation.activity {
                Some(activity) => Some(activity.as_str()),
                None => existing.as_ref().and_then(|row| row.activity.as_deref()),
            };
            // A partial metrics success is not new Activity/identity evidence.
            // Retain an absence until an explicit Activity supersedes it.
            let verdict_outcome = if observation.activity.is_some() {
                Some("success")
            } else {
                existing
                    .as_ref()
                    .and_then(|row| row.last_good_verdict_outcome.as_deref())
            };
            let verdict_received_at = if observation.activity.is_some() {
                Some(now.as_str())
            } else {
                existing
                    .as_ref()
                    .and_then(|row| row.last_good_verdict_received_at.as_deref())
            };
            let activity_changed = existing.as_ref().and_then(|row| row.activity.as_deref())
                != stored_activity
                || existing
                    .as_ref()
                    .and_then(|row| row.last_good_verdict_outcome.as_deref())
                    != verdict_outcome;
            // Receipt/attempt recovery changes the displayed evidence even
            // when the Activity value and historical sample are unchanged.
            let verdict_refreshed = existing.as_ref().is_none_or(|row| {
                row.outcome != "success"
                    || row.diagnostic.is_some()
                    || row.last_good_verdict_received_at.as_deref() != verdict_received_at
            });
            if existing
                .as_ref()
                .and_then(|row| row.last_observation_key.as_deref())
                == Some(key.as_str())
            {
                sqlx::query(
                    "UPDATE current_validator_insights SET outcome = 'success', diagnostic = NULL, activity = $1, last_good_verdict_outcome = $2, last_good_verdict_received_at = $3, last_attempt_received_at = $4, last_good_received_at = $5, counter_state = 'normal', updated_at = $6 WHERE validator_id = $7",
                )
                .bind(stored_activity)
                .bind(verdict_outcome)
                .bind(verdict_received_at)
                .bind(&now)
                .bind(&now)
                .bind(&now)
                .bind(validator_id)
                .execute(&mut **tx)
                .await?;
                let (analytics_changed, stored_day) = record_daily_snapshot(
                    tx,
                    validator_id,
                    source,
                    &observation,
                    &key,
                    &now,
                    timezone,
                )
                .await?;
                rebuild_monthly_aggregate(tx, validator_id, timezone, &stored_day.month_key, &now)
                    .await?;
                return Ok(AppliedDetail {
                    stored: true,
                    activity_changed,
                    invalidated: analytics_changed || activity_changed || verdict_refreshed,
                    stored_day: Some(stored_day),
                });
            }

            let decreases = counter_decreases(existing.as_ref(), &observation);
            let counter_changed = !decreases.is_empty();
            let counter_state = if counter_changed {
                "counter_reset"
            } else {
                "normal"
            };

            for (counter_name, previous_value, current_value) in decreases {
                sqlx::query("INSERT INTO validator_counter_history (history_id, validator_id, counter_name, previous_value, current_value, observed_at, provider_timestamp, observation_key) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT DO NOTHING")
                    .bind(uuid::Uuid::new_v4().to_string())
                    .bind(validator_id)
                    .bind(counter_name)
                    .bind(previous_value)
                    .bind(current_value)
                    .bind(&now)
                    .bind(observation.provider_timestamp.as_deref())
                    .bind(&key)
                    .execute(&mut **tx)
                    .await?;
            }

            let source = bounded_source(source);
            sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, stake_amount, reward_amount, reward_rate, delegation_reward_percentage, delegator_count, epoch, block_count, expected_block_count, gen_blocks_rate, counter_state, last_observation_key, updated_at) VALUES ($1, $2, 'success', NULL, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19) ON CONFLICT(validator_id) DO UPDATE SET source=excluded.source, outcome=excluded.outcome, diagnostic=NULL, provider_timestamp=excluded.provider_timestamp, activity=excluded.activity, last_attempt_received_at=excluded.last_attempt_received_at, last_good_received_at=excluded.last_good_received_at, last_good_provider_timestamp=excluded.last_good_provider_timestamp, stake_amount=excluded.stake_amount, reward_amount=excluded.reward_amount, reward_rate=excluded.reward_rate, delegation_reward_percentage=excluded.delegation_reward_percentage, delegator_count=excluded.delegator_count, epoch=excluded.epoch, block_count=excluded.block_count, expected_block_count=excluded.expected_block_count, gen_blocks_rate=excluded.gen_blocks_rate, counter_state=excluded.counter_state, last_observation_key=excluded.last_observation_key, updated_at=excluded.updated_at")
                .bind(validator_id)
                .bind(&source)
                .bind(observation.provider_timestamp.as_deref())
                .bind(stored_activity)
                .bind(&now)
                .bind(&now)
                .bind(observation.provider_timestamp.as_deref())
                .bind(observation.stake_amount.as_deref())
                .bind(observation.reward_amount.as_deref())
                .bind(observation.reward_rate.as_deref())
                .bind(observation.delegation_reward_percentage.as_deref())
                .bind(observation.delegator_count)
                .bind(observation.epoch)
                .bind(observation.block_count)
                .bind(observation.expected_block_count)
                .bind(observation.gen_blocks_rate.as_deref())
                .bind(counter_state)
                .bind(&key)
                .bind(&now)
                .execute(&mut **tx)
                .await?;
            sqlx::query("UPDATE current_validator_insights SET last_good_verdict_outcome = $1, last_good_verdict_received_at = $2 WHERE validator_id = $3")
                .bind(verdict_outcome).bind(verdict_received_at).bind(validator_id)
                .execute(&mut **tx).await?;
            let (analytics_changed, stored_day) = record_daily_snapshot(
                tx,
                validator_id,
                &source,
                &observation,
                &key,
                &now,
                timezone,
            )
            .await?;
            rebuild_monthly_aggregate(tx, validator_id, timezone, &stored_day.month_key, &now)
                .await?;
            Ok(AppliedDetail {
                stored: true,
                activity_changed,
                invalidated: analytics_changed || activity_changed || verdict_refreshed,
                stored_day: Some(stored_day),
            })
        }
        outcome => {
            let (name, diagnostic) = match outcome {
                ValidatorProviderResult::NotFound => ("not_found", None),
                ValidatorProviderResult::AuthoritativeEmpty => ("empty", None),
                ValidatorProviderResult::NotConfigured(value) => {
                    ("not_configured", Some(provider_diagnostic(value)))
                }
                ValidatorProviderResult::Error(value) => {
                    ("error", Some(provider_diagnostic(value)))
                }
                ValidatorProviderResult::Unsupported(value) => {
                    ("unsupported", Some(provider_diagnostic(value)))
                }
                ValidatorProviderResult::Success(_) => unreachable!(),
            };
            let invalidated = existing.as_ref().is_none_or(|row| {
                row.outcome != name
                    || row.diagnostic != diagnostic
                    || (name == "empty"
                        && (row.last_good_verdict_outcome.as_deref() != Some("empty")
                            || row.last_good_verdict_received_at.as_deref() != Some(now.as_str())))
            });
            let source = bounded_source(source);
            sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, last_attempt_received_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT(validator_id) DO UPDATE SET source=excluded.source, outcome=excluded.outcome, diagnostic=excluded.diagnostic, last_attempt_received_at=excluded.last_attempt_received_at, updated_at=excluded.updated_at")
                .bind(validator_id)
                .bind(source)
                .bind(name)
                .bind(diagnostic)
                .bind(&now)
                .bind(&now)
                .execute(&mut **tx)
                .await?;
            if name == "empty" {
                sqlx::query("UPDATE current_validator_insights SET last_good_verdict_outcome = 'empty', last_good_verdict_received_at = $1 WHERE validator_id = $2")
                    .bind(&now).bind(validator_id).execute(&mut **tx).await?;
            }
            Ok(AppliedDetail {
                stored: false,
                activity_changed: false,
                invalidated,
                stored_day: None,
            })
        }
    }
}

/// Apply one Validator's shared-Network ranking lookup. Ranking state is
/// stored independently from detail metrics: a ranking failure updates only
/// the rank attempt/diagnostic and never clears the detail columns, and a
/// detail failure never clears a last-good rank (#158).
///
/// The answer also reaches that Validator's snapshot for the configured local
/// day the same refresh cycle observed, because a daily rank trend can only
/// describe ranks the ranking endpoint actually reported for a day that was
/// observed (#219). `stored_day` is that day, taken from the cycle's own
/// detail read: a cycle that stored no snapshot writes no rank, so a fresh
/// ranking answer is never attached to an older day's evidence or to a day
/// nobody observed. The day carries the newest rank reading for it: an
/// authoritative "not in the cohort" answer stores NULL, and a failed ranking
/// attempt leaves the stored reading untouched, exactly like the current-state
/// columns above it.
async fn apply_ranking_result(
    tx: &mut Transaction<'_, Any>,
    validator_id: &str,
    lookup: NetworkRankingLookup,
    timezone: &str,
    stored_day: Option<&StoredDay>,
) -> Result<(bool, bool), ValidatorError> {
    let now_instant = crate::auth::now_utc();
    let now = crate::auth::format_rfc3339(now_instant);
    let existing = sqlx::query_as::<_, ValidatorInsightRecord>(INSIGHT_SELECT)
        .bind(validator_id)
        .fetch_optional(&mut **tx)
        .await?;
    let Some(existing) = existing else {
        return Ok((false, false));
    };
    match lookup {
        NetworkRankingLookup::Ranked { rank, cohort_size } => {
            let baseline_exists =
                existing.rank_last_good_received_at.is_some() && existing.rank.is_some();
            // The last confirmed rank a change is compared against. When a
            // change is already pending, the baseline is the value before it;
            // the pending rank is already the adopted upstream value.
            let baseline = match (existing.candidate_rank, existing.candidate_previous_rank) {
                (Some(_), Some(previous)) => Some(previous),
                _ => existing.rank,
            };
            let mut confirmed_ranking_change = false;
            let candidate = if !baseline_exists || baseline == Some(rank) {
                // The first rank, an unchanged rank, or a return to the known
                // baseline: nothing to record.
                RankingCandidate::default()
            } else if existing.candidate_rank == Some(rank)
                && existing.candidate_previous_rank == baseline
                && existing.candidate_observations == 1
            {
                // A consecutive observation of the same new rank confirms the
                // change for the ranking alert. The displayed rank already
                // adopted it on the first observation.
                let previous = baseline.unwrap_or(rank);
                sqlx::query("INSERT INTO validator_ranking_history (history_id, validator_id, previous_rank, current_rank, observed_at, provider_timestamp, observation_key, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT DO NOTHING")
                    .bind(uuid::Uuid::new_v4().to_string())
                    .bind(validator_id)
                    .bind(previous)
                    .bind(rank)
                    .bind(&now)
                    .bind(None::<&str>)
                    .bind(ranking_observation_key(rank, cohort_size))
                    .bind(existing.candidate_observed_at.as_deref())
                    .bind(existing.candidate_provider_timestamp.as_deref())
                    .bind(existing.candidate_observation_key.as_deref())
                    .execute(&mut **tx)
                    .await?;
                confirmed_ranking_change = true;
                RankingCandidate::default()
            } else {
                // First observation of a change: record it as a candidate for
                // the alert while still displaying the upstream rank.
                RankingCandidate {
                    previous_rank: baseline,
                    rank: Some(rank),
                    observations: 1,
                    observed_at: Some(now.clone()),
                    provider_timestamp: None,
                    observation_key: Some(ranking_observation_key(rank, cohort_size)),
                }
            };

            // The upstream rank is adopted verbatim and immediately; pending
            // alert confirmation never delays the displayed position (#158).
            let stored_rank = Some(rank);
            let change_state = if confirmed_ranking_change {
                "ranking_changed"
            } else {
                "normal"
            };
            let invalidated = existing.rank != stored_rank
                || existing.rank_outcome.as_deref() != Some("success")
                || existing.rank_cohort_size != Some(cohort_size)
                || confirmed_ranking_change;
            sqlx::query("UPDATE current_validator_insights SET rank = $1, rank_outcome = 'success', rank_diagnostic = NULL, rank_last_attempt_received_at = $2, rank_last_good_received_at = $3, rank_cohort_size = $4, change_state = $5, candidate_previous_rank = $6, candidate_rank = $7, candidate_observations = $8, candidate_observed_at = $9, candidate_provider_timestamp = $10, candidate_observation_key = $11, updated_at = $12 WHERE validator_id = $13")
                .bind(stored_rank)
                .bind(&now)
                .bind(&now)
                .bind(cohort_size)
                .bind(change_state)
                .bind(candidate.previous_rank)
                .bind(candidate.rank)
                .bind(candidate.observations)
                .bind(candidate.observed_at)
                .bind(candidate.provider_timestamp)
                .bind(candidate.observation_key)
                .bind(&now)
                .bind(validator_id)
                .execute(&mut **tx)
                .await?;
            if let Some(day) = stored_day {
                record_snapshot_rank(tx, validator_id, timezone, stored_rank, day, &now).await?;
            }
            Ok((confirmed_ranking_change, invalidated))
        }
        NetworkRankingLookup::Unranked { cohort_size } => {
            let invalidated = existing.rank.is_some()
                || existing.rank_outcome.as_deref() != Some("success")
                || existing.rank_cohort_size != Some(cohort_size)
                || existing.change_state != "normal";
            sqlx::query("UPDATE current_validator_insights SET rank = NULL, rank_outcome = 'success', rank_diagnostic = NULL, rank_last_attempt_received_at = $1, rank_last_good_received_at = $2, rank_cohort_size = $3, change_state = 'normal', candidate_previous_rank = NULL, candidate_rank = NULL, candidate_observations = 0, candidate_observed_at = NULL, candidate_provider_timestamp = NULL, candidate_observation_key = NULL, updated_at = $4 WHERE validator_id = $5")
                .bind(&now)
                .bind(&now)
                .bind(cohort_size)
                .bind(&now)
                .bind(validator_id)
                .execute(&mut **tx)
                .await?;
            if let Some(day) = stored_day {
                record_snapshot_rank(tx, validator_id, timezone, None, day, &now).await?;
            }
            Ok((false, invalidated))
        }
        failure => {
            let (name, diagnostic) = match failure {
                NetworkRankingLookup::NotConfigured(value) => {
                    ("not_configured", Some(provider_diagnostic(value)))
                }
                NetworkRankingLookup::Unsupported(value) => {
                    ("unsupported", Some(provider_diagnostic(value)))
                }
                NetworkRankingLookup::Error(value) => ("error", Some(provider_diagnostic(value))),
                NetworkRankingLookup::Ranked { .. } | NetworkRankingLookup::Unranked { .. } => {
                    unreachable!()
                }
            };
            let invalidated = existing.rank_outcome.as_deref() != Some(name)
                || existing.rank_diagnostic != diagnostic
                || existing.change_state != "normal";
            sqlx::query("UPDATE current_validator_insights SET rank_outcome = $1, rank_diagnostic = $2, rank_last_attempt_received_at = $3, change_state = 'normal', candidate_previous_rank = NULL, candidate_rank = NULL, candidate_observations = 0, candidate_observed_at = NULL, candidate_provider_timestamp = NULL, candidate_observation_key = NULL, updated_at = $4 WHERE validator_id = $5")
                .bind(name)
                .bind(diagnostic)
                .bind(&now)
                .bind(&now)
                .bind(validator_id)
                .execute(&mut **tx)
                .await?;
            Ok((false, invalidated))
        }
    }
}
pub(super) fn bounded_source(source: &str) -> String {
    source.chars().take(64).collect()
}

pub(super) fn validate_observation(
    observation: &ValidatorObservation,
) -> Result<(), ValidatorError> {
    let has_supported_value = observation.activity.is_some()
        || observation.stake_amount.is_some()
        || observation.reward_amount.is_some()
        || observation.reward_rate.is_some()
        || observation.delegation_reward_percentage.is_some()
        || observation.delegator_count.is_some()
        || observation.epoch.is_some()
        || observation.block_count.is_some()
        || observation.expected_block_count.is_some()
        || observation.gen_blocks_rate.is_some();
    if !has_supported_value {
        return Err(ValidatorError::InvalidProviderObservation(
            "empty observation".to_owned(),
        ));
    }
    if observation.delegator_count.is_some_and(|value| value < 0)
        || observation.epoch.is_some_and(|value| value < 0)
        || observation.block_count.is_some_and(|value| value < 0)
        || observation
            .expected_block_count
            .is_some_and(|value| value < 0)
    {
        return Err(ValidatorError::InvalidProviderObservation(
            "negative integer".to_owned(),
        ));
    }
    for value in [
        observation.stake_amount.as_deref(),
        observation.reward_amount.as_deref(),
        observation.reward_rate.as_deref(),
        observation.delegation_reward_percentage.as_deref(),
        observation.gen_blocks_rate.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        normalize_bounded_text(value).map_err(ValidatorError::InvalidProviderObservation)?;
        validate_nonnegative_decimal(value).map_err(ValidatorError::InvalidProviderObservation)?;
    }
    if observation
        .delegation_reward_percentage
        .as_deref()
        .is_some_and(|value| decimal_exceeds_max(value, 100))
    {
        return Err(ValidatorError::InvalidProviderObservation(
            "out-of-range delegation reward percentage".to_owned(),
        ));
    }
    if let Some(timestamp) = observation.provider_timestamp.as_deref() {
        canonical_timestamp(timestamp)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    use crate::network::create_network;

    use serde_json::Value;

    use crate::database::ServerDatabase;

    use crate::validator::decimal::cumulative_block_rate;
    use crate::validator::insight::{ValidatorCounterHistoryRecord, load_insight};
    use crate::validator::platscan::PlatScanValidatorProvider;
    use crate::validator::provider::{
        RankingProviderResult, ValidatorActivity, ValidatorObservation, ValidatorProviderResult,
    };
    use crate::validator::registry::{create_link, create_validator};
    use crate::validator::status::{CurrentValidatorStatus, project_verdict};
    use crate::validator::test_helpers::{
        FakeProvider, deployments, platscan_success, provider_node_id, ranking_with,
        start_mock_platscan, test_db,
    };

    use super::*;

    async fn negative_verdict_test_state(
        node_id: &str,
    ) -> (tempfile::TempDir, crate::http::AppState) {
        let (dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", node_id, None, &owner_id)
            .await
            .unwrap();
        let (link, _) = create_link(
            &db,
            "node-1",
            &validator.validator_id,
            "observer",
            "2025-01-01T00:00:00Z",
            None,
            &owner_id,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE node_validator_links SET origin = 'automatic' WHERE link_id = $1")
            .bind(&link.link_id)
            .execute(db.pool())
            .await
            .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        (dir, crate::http::AppState::new(db, None, auth))
    }

    async fn negative_verdict_public_insight(state: &crate::http::AppState) -> Value {
        let response =
            crate::http::public::public_networks(axum::extract::State(state.clone())).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice::<Value>(&body).unwrap()[0]["nodes"][0]["validator"].clone()
    }

    #[tokio::test]
    async fn platscan_negative_verdict_survives_failure_without_reviving_old_activity() {
        // Both positive->absence->failure and first absence->failure use the
        // actual HTTP adapter, transactional refresh and public projection.
        for previously_active in [true, false] {
            let node_id = provider_node_id();
            let (_dir, state) = negative_verdict_test_state(&node_id).await;
            let mut responses = Vec::new();
            if previously_active {
                responses.extend([
                    (
                        200,
                        serde_json::to_vec(&platscan_success(&node_id, 2)).unwrap(),
                    ),
                    (503, Vec::new()),
                ]);
            }
            responses.extend([
                (
                    200,
                    serde_json::to_vec(&serde_json::json!({"code": 0,
                    "data": {"nodeId": "", "status": 0}}))
                    .unwrap(),
                ),
                (503, Vec::new()),
                (503, Vec::new()),
                (503, Vec::new()),
            ]);
            let (base_url, mock, handle) = start_mock_platscan(responses, 0).await;
            let provider = PlatScanValidatorProvider::new(
                deployments(&base_url, &["platon-mainnet"]),
                std::time::Duration::from_secs(5),
            )
            .unwrap();
            if previously_active {
                refresh_all(state.db(), &provider).await.unwrap();
                assert_eq!(
                    negative_verdict_public_insight(&state).await["currentValidatorStatus"],
                    "validator"
                );
                // Age only the old detail metrics. A subsequent absence must
                // not turn these retained amounts/counters into fresh values.
                sqlx::query("UPDATE current_validator_insights SET last_good_received_at = '2025-01-01T00:00:00Z'")
                    .execute(state.db().pool()).await.unwrap();
            }
            refresh_all(state.db(), &provider).await.unwrap();
            let absent = negative_verdict_public_insight(&state).await;
            assert_eq!(absent["currentValidatorStatus"], "not_validator");
            assert_eq!(absent["currentValidatorStatusState"], "current");
            assert_eq!(absent["activity"], "observing");
            assert_eq!(absent["activityState"], "current");
            let metrics_received_at = absent["receivedAt"].clone();
            assert_eq!(
                absent["freshness"],
                if previously_active {
                    "stale"
                } else {
                    "unknown"
                }
            );
            refresh_all(state.db(), &provider).await.unwrap();
            let failed = negative_verdict_public_insight(&state).await;
            assert_eq!(failed["state"], "error");
            assert_eq!(
                failed["currentValidatorStatus"], "not_validator",
                "failed refresh must retain the latest authoritative absence, not resurrect older activity"
            );
            assert_eq!(failed["currentValidatorStatusState"], "stale");
            assert_eq!(failed["activity"], "observing");
            assert_eq!(failed["activityState"], "stale");
            assert_eq!(failed["receivedAt"], metrics_received_at);
            assert_eq!(failed["activityReceivedAt"], absent["activityReceivedAt"]);
            let snapshot_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM validator_daily_snapshots")
                    .fetch_one(state.db().pool())
                    .await
                    .unwrap();
            assert_eq!(
                snapshot_count,
                i64::from(previously_active),
                "absence and failure must not fabricate or rewrite metrics history"
            );
            assert_eq!(
                mock.requests.lock().unwrap().len(),
                if previously_active { 6 } else { 4 }
            );
            handle.abort();
        }
    }

    #[tokio::test]
    async fn platscan_negative_verdict_ages_without_a_new_refresh() {
        let node_id = provider_node_id();
        let (_dir, state) = negative_verdict_test_state(&node_id).await;
        let (base_url, _mock, handle) = start_mock_platscan(
            vec![
                (
                    200,
                    serde_json::to_vec(&serde_json::json!({"code": 0,
                "data": {"nodeId": "", "status": 0}}))
                    .unwrap(),
                ),
                (503, Vec::new()),
            ],
            0,
        )
        .await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        refresh_all(state.db(), &provider).await.unwrap();
        assert_eq!(
            negative_verdict_public_insight(&state).await["currentValidatorStatusState"],
            "current"
        );
        sqlx::query("UPDATE current_validator_insights SET last_attempt_received_at = '2025-01-01T00:00:00Z', last_good_verdict_received_at = '2025-01-01T00:00:00Z'")
            .execute(state.db().pool()).await.unwrap();
        let aged = negative_verdict_public_insight(&state).await;
        assert_eq!(aged["currentValidatorStatus"], "not_validator");
        assert_eq!(
            aged["currentValidatorStatusState"], "stale",
            "confirmed absence must expire just like positive evidence"
        );
        assert_eq!(aged["activity"], "observing");
        assert_eq!(aged["activityState"], "stale");
        handle.abort();
    }

    #[tokio::test]
    async fn platscan_negative_verdict_renewal_invalidates_even_with_unchanged_ranking() {
        let node_id = provider_node_id();
        let (_dir, state) = negative_verdict_test_state(&node_id).await;
        let absent = serde_json::to_vec(&serde_json::json!({"code": 0,
            "data": {"nodeId": "", "status": 0}}))
        .unwrap();
        let (base_url, _mock, handle) = start_mock_platscan(
            vec![
                (200, absent.clone()),
                (503, Vec::new()),
                (200, absent),
                (503, Vec::new()),
            ],
            0,
        )
        .await;
        let provider = PlatScanValidatorProvider::new(
            deployments(&base_url, &["platon-mainnet"]),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        refresh_all(state.db(), &provider).await.unwrap();
        sqlx::query("UPDATE current_validator_insights SET last_good_verdict_received_at = '2025-01-01T00:00:00Z'")
            .execute(state.db().pool()).await.unwrap();
        assert_eq!(
            negative_verdict_public_insight(&state).await["currentValidatorStatusState"],
            "stale"
        );
        let renewed = refresh_all(state.db(), &provider).await.unwrap();
        assert_eq!(
            negative_verdict_public_insight(&state).await["currentValidatorStatusState"],
            "current"
        );
        assert!(
            !renewed.invalidated_validator_ids.is_empty(),
            "a renewed negative verdict must refetch an already-open stale projection"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn confirmed_verdict_replay_recovers_and_invalidates_without_rewriting_history() {
        for interruption in [
            Some(ValidatorProviderResult::Error("failed".to_owned())),
            Some(ValidatorProviderResult::AuthoritativeEmpty),
            None,
        ] {
            let node_id = provider_node_id();
            let (_dir, state) = negative_verdict_test_state(&node_id).await;
            let observation = ValidatorObservation {
                provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                activity: Some(ValidatorActivity::Active),
                stake_amount: Some("123456789000000000000".to_owned()),
                ..Default::default()
            };
            let mut results = vec![ValidatorProviderResult::Success(Box::new(
                observation.clone(),
            ))];
            if let Some(result) = interruption {
                results.push(result);
            }
            results.push(ValidatorProviderResult::Success(Box::new(observation)));
            let provider = FakeProvider {
                results: std::sync::Mutex::new(results),
                ..Default::default()
            };
            refresh_all(state.db(), &provider).await.unwrap();
            if provider.results.lock().unwrap().len() > 1 {
                refresh_all(state.db(), &provider).await.unwrap();
            }
            sqlx::query("UPDATE current_validator_insights SET last_good_verdict_received_at = '2025-01-01T00:00:00Z'")
                .execute(state.db().pool()).await.unwrap();
            assert_eq!(
                negative_verdict_public_insight(&state).await["activityState"],
                "stale"
            );
            let recovered = refresh_all(state.db(), &provider).await.unwrap();
            let insight = negative_verdict_public_insight(&state).await;
            assert_eq!(insight["currentValidatorStatus"], "validator");
            assert_eq!(insight["activity"], "active");
            assert_eq!(insight["activityState"], "current");
            assert!(
                !recovered.invalidated_validator_ids.is_empty(),
                "an identical confirmed recovery must refetch stale/error projections"
            );
            let snapshots: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM validator_daily_snapshots")
                    .fetch_one(state.db().pool())
                    .await
                    .unwrap();
            assert_eq!(
                snapshots, 1,
                "replay must not duplicate a historical sample"
            );
        }
    }

    #[tokio::test]
    async fn metric_only_replay_does_not_replace_or_renew_confirmed_absence() {
        let (_dir, state) = negative_verdict_test_state(&provider_node_id()).await;
        let observation = ValidatorObservation {
            provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
            stake_amount: Some("123456789000000000000".to_owned()),
            ..Default::default()
        };
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(observation.clone())),
                ValidatorProviderResult::AuthoritativeEmpty,
                ValidatorProviderResult::Success(Box::new(observation)),
            ]),
            ..Default::default()
        };
        refresh_all(state.db(), &provider).await.unwrap();
        refresh_all(state.db(), &provider).await.unwrap();
        sqlx::query("UPDATE current_validator_insights SET last_good_verdict_received_at = '2025-01-01T00:00:00Z'")
            .execute(state.db().pool()).await.unwrap();
        refresh_all(state.db(), &provider).await.unwrap();
        let insight = negative_verdict_public_insight(&state).await;
        assert_eq!(insight["activityReceivedAt"], "2025-01-01T00:00:00Z");
        assert_eq!(insight["currentValidatorStatus"], "not_validator");
        assert_eq!(insight["activity"], "observing");
        assert_eq!(insight["activityState"], "stale");
        assert_eq!(insight["freshness"], "fresh");
        assert_eq!(insight["stakeAmount"], "123456789000000000000");
    }

    #[tokio::test]
    async fn provider_refresh_preserves_last_good_and_confirms_rank_after_two_observations() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xprovider", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    stake_amount: Some("123456789012345678901234567890".to_owned()),
                    reward_rate: Some("0.125000000000000001".to_owned()),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:01:00Z".to_owned()),
                    stake_amount: Some("123456789012345678901234567889".to_owned()),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:02:00Z".to_owned()),
                    stake_amount: Some("123456789012345678901234567888".to_owned()),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::Error(
                    "provider timeout at https://secret.example".to_owned(),
                ),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(3, &[("0xprovider", 1)]),
                ranking_with(3, &[("0xprovider", 2)]),
                ranking_with(3, &[("0xprovider", 2)]),
                RankingProviderResult::Error("ranking timeout".to_owned()),
            ]),
            ..FakeProvider::default()
        };
        let first = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(first.attempted, 1);
        assert_eq!(first.invalidations, 1);
        let second = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(second.changed, 0);
        assert_eq!(second.invalidations, 1);
        let third = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(third.changed, 1);
        assert_eq!(third.invalidations, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM validator_ranking_history")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "success");
        assert_eq!(insight.rank, Some(2));
        assert_eq!(
            insight.stake_amount.as_deref(),
            Some("123456789012345678901234567888")
        );
        assert_eq!(insight.counter_state, "counter_reset");
        let incidents = sqlx::query_as::<_, (String, String, String)>(
            "SELECT rule_key, state, opened_evidence_json FROM alert_incidents WHERE subject_key = $1 ORDER BY rule_key",
        )
        .bind(&validator.validator_id)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            incidents.len(),
            2,
            "ranking and counter signals are independent"
        );
        assert!(incidents.iter().all(|(rule_key, state, _)| state == "open"
            && (rule_key == "validator.ranking_changed" || rule_key == "validator.counter_reset")));
        let counter_evidence = incidents
            .iter()
            .find(|(rule_key, _, _)| rule_key == "validator.counter_reset")
            .map(|(_, _, evidence)| evidence)
            .unwrap();
        assert!(counter_evidence.contains("123456789012345678901234567890"));
        assert!(counter_evidence.contains("123456789012345678901234567889"));
        let ranking_state: (i64, String) = sqlx::query_as(
            "SELECT evaluation_unavailable, input_kind FROM alert_rule_state WHERE rule_key = 'validator.ranking_changed' AND subject_key = $1",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(ranking_state, (0, "known".to_owned()));
        let counter_state: (i64, String) = sqlx::query_as(
            "SELECT evaluation_unavailable, input_kind FROM alert_rule_state WHERE rule_key = 'validator.counter_reset' AND subject_key = $1",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(counter_state, (0, "known".to_owned()));
        refresh_all(&db, &provider).await.unwrap();
        let failed = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.outcome, "error");
        assert_eq!(failed.rank, Some(2));
        assert_eq!(
            failed.stake_amount.as_deref(),
            Some("123456789012345678901234567888")
        );
        let failed_diagnostic = failed.diagnostic.unwrap_or_default();
        assert!(failed_diagnostic.contains("provider timeout"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM alert_incidents WHERE subject_key = $1 AND state = 'open'",
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            2
        );
        let unavailable: Vec<(i64, String)> = sqlx::query_as(
            "SELECT evaluation_unavailable, input_kind FROM alert_rule_state WHERE subject_key = $1 AND rule_key LIKE 'validator.%' ORDER BY rule_key",
        )
        .bind(&validator.validator_id)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            unavailable,
            vec![(1, "unsupported".to_owned()), (1, "unsupported".to_owned())]
        );
    }

    #[tokio::test]
    async fn candidate_activity_persists_as_its_own_canonical_value() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) =
            create_validator(&db, "platon-mainnet", "0xcandidate", None, &owner_id)
                .await
                .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Candidate),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:01:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Active),
                    ..Default::default()
                })),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(1, &[("0xcandidate", 1)]),
                ranking_with(1, &[("0xcandidate", 1)]),
            ]),
            ..FakeProvider::default()
        };

        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        // Candidate is stored under its own canonical value, not folded into
        // Active: the persisted turn must pass the activity CHECK constraint.
        assert_eq!(insight.activity.as_deref(), Some("candidate"));

        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.activity.as_deref(), Some("active"));
    }

    #[tokio::test]
    async fn activity_persists_applies_immediately_and_survives_provider_failure() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xactivity", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Active),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:01:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Producing),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:02:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Producing),
                    ..Default::default()
                })),
                ValidatorProviderResult::Error("provider timeout".to_owned()),
                ValidatorProviderResult::AuthoritativeEmpty,
                // A validated snapshot that omits Activity preserves the
                // last-good canonical value instead of erasing it.
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:03:00Z".to_owned()),
                    stake_amount: Some("1".to_owned()),
                    ..Default::default()
                })),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(3, &[("0xactivity", 1)]),
                ranking_with(3, &[("0xactivity", 2)]),
                ranking_with(3, &[("0xactivity", 2)]),
                ranking_with(3, &[("0xactivity", 2)]),
                ranking_with(3, &[("0xactivity", 2)]),
                ranking_with(3, &[("0xactivity", 2)]),
            ]),
            ..FakeProvider::default()
        };

        let first = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(first.invalidations, 1);
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "success");
        assert_eq!(insight.activity.as_deref(), Some("active"));

        // Activity applies on the very next successful snapshot. The upstream
        // rank is adopted immediately, while the ranking-change alert waits
        // for the second consecutive observation to confirm.
        let second = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(second.changed, 1);
        assert_eq!(second.invalidations, 1);
        assert_eq!(
            second.invalidated_validator_ids,
            vec![validator.validator_id.clone()]
        );
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.activity.as_deref(), Some("producing"));
        assert_eq!(insight.rank, Some(2));
        assert_eq!(insight.candidate_rank, Some(2));

        let third = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(third.changed, 1);
        assert_eq!(third.invalidations, 1);
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.activity.as_deref(), Some("producing"));
        assert_eq!(insight.rank, Some(2));
        assert_eq!(insight.candidate_rank, None);

        // Provider failure keeps the last-good Activity: Error outcomes
        // retain Producing so the Public projection can mark it Stale.
        let failed = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(failed.invalidations, 1);
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.activity.as_deref(), Some("producing"));

        // An authoritative empty result is still Activity-less for the
        // Public projection (Observing) and invalidates the projection.
        let empty = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(empty.invalidations, 1);
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "empty");

        assert_eq!(insight.last_good_verdict_outcome.as_deref(), Some("empty"));
        let absence_received_at = insight.last_good_verdict_received_at.clone();

        // Metric-only success retains the raw cache, but cannot supersede or
        // renew the last confirmed absence.
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "success");
        assert_eq!(insight.rank, Some(2));
        assert_eq!(insight.activity.as_deref(), Some("producing"));
        assert_eq!(insight.last_good_verdict_outcome.as_deref(), Some("empty"));
        assert_eq!(insight.last_good_verdict_received_at, absence_received_at);
        let (activity, currency, status) = project_verdict(
            &insight.outcome,
            insight.last_good_verdict_outcome.as_deref(),
            insight.activity.as_deref(),
            "fresh",
        );
        assert_eq!(
            (activity.as_str(), currency.as_str()),
            ("observing", "current")
        );
        assert_eq!(status.status, CurrentValidatorStatus::NotValidator);

        // A Validator that has never seen a successful Activity stays Unknown
        // even after an Error; Provider state never fabricates a value.
        let (unknown, _) = create_validator(&db, "platon-mainnet", "0xunknown", None, &owner_id)
            .await
            .unwrap();
        let failed_first = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Error("never seen".to_owned()),
                ValidatorProviderResult::Error("never seen".to_owned()),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &failed_first).await.unwrap();
        let insight = load_insight(&db, &unknown.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.activity, None);
    }

    #[tokio::test]
    async fn ranking_changes_need_consecutive_successes_and_replay_is_idempotent() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xrank", None, &owner_id)
            .await
            .unwrap();
        fn detail(provider_timestamp: &str) -> ValidatorProviderResult {
            ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                provider_timestamp: Some(provider_timestamp.to_owned()),
                stake_amount: Some("1000".to_owned()),
                ..ValidatorObservation::default()
            }))
        }
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                detail("2025-01-01T00:00:00Z"),
                detail("2025-01-01T00:01:00Z"),
                detail("2025-01-01T00:02:00Z"),
                ValidatorProviderResult::Error("temporary detail failure".to_owned()),
                detail("2025-01-01T00:03:00Z"),
                detail("2025-01-01T00:04:00Z"),
                detail("2025-01-01T00:05:00Z"),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(3, &[("0xrank", 1)]),
                ranking_with(3, &[("0xrank", 2)]),
                ranking_with(3, &[("0xrank", 2)]),
                RankingProviderResult::Error("temporary ranking failure".to_owned()),
                ranking_with(3, &[("0xrank", 3)]),
                ranking_with(3, &[("0xrank", 3)]),
                ranking_with(3, &[("0xrank", 3)]),
            ]),
            ..FakeProvider::default()
        };

        async fn history_count(db: &ServerDatabase, validator_id: &str) -> i64 {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_ranking_history WHERE validator_id = $1",
            )
            .bind(validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap()
        }

        // One success establishes the baseline and is never a change.
        refresh_all(&db, &provider).await.unwrap();
        let baseline = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(baseline.rank, Some(1));
        assert_eq!(baseline.rank_outcome.as_deref(), Some("success"));
        assert_eq!(history_count(&db, &validator.validator_id).await, 0);

        // A second distinct rank is a candidate, not yet a confirmed change.
        refresh_all(&db, &provider).await.unwrap();
        let candidate = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(candidate.rank, Some(2));
        assert_eq!(candidate.candidate_rank, Some(2));
        assert_eq!(history_count(&db, &validator.validator_id).await, 0);
        db.close().await;
        let db = crate::test_support::test_database_in(_dir.path()).await;

        // A consecutive successful list confirms the change across restart.
        refresh_all(&db, &provider).await.unwrap();
        let confirmed = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(confirmed.rank, Some(2));
        assert_eq!(confirmed.candidate_rank, None);
        assert_eq!(history_count(&db, &validator.validator_id).await, 1);

        // A failed list fetch retains the rank and discards the candidate.
        refresh_all(&db, &provider).await.unwrap();
        let after_failure = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after_failure.rank, Some(2));
        assert_eq!(after_failure.rank_outcome.as_deref(), Some("error"));
        assert_eq!(after_failure.candidate_rank, None);
        assert_eq!(history_count(&db, &validator.validator_id).await, 1);

        // After a failure the next success is a fresh candidate, never a
        // confirmation stitched across the gap.
        refresh_all(&db, &provider).await.unwrap();
        let rearmed = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rearmed.rank, Some(3));
        assert_eq!(rearmed.candidate_rank, Some(3));
        assert_eq!(history_count(&db, &validator.validator_id).await, 1);

        refresh_all(&db, &provider).await.unwrap();
        let confirmed_again = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(confirmed_again.rank, Some(3));
        assert_eq!(history_count(&db, &validator.validator_id).await, 2);

        // Replaying the same confirmed list is idempotent.
        refresh_all(&db, &provider).await.unwrap();
        assert_eq!(history_count(&db, &validator.validator_id).await, 2);
    }

    #[tokio::test]
    async fn counter_correction_history_keeps_exact_decimal_evidence() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xcounter", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    stake_amount: Some("100.000000000000000001".to_owned()),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    stake_amount: Some("99.999999999999999999".to_owned()),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    stake_amount: Some("99.999999999999999999".to_owned()),
                    ..Default::default()
                })),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        refresh_all(&db, &provider).await.unwrap();
        refresh_all(&db, &provider).await.unwrap();
        let row = sqlx::query_as::<_, ValidatorCounterHistoryRecord>(
            "SELECT history_id, validator_id, counter_name, previous_value, current_value, observed_at, provider_timestamp, observation_key FROM validator_counter_history WHERE validator_id = $1",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(row.counter_name, "stake_amount");
        assert_eq!(row.previous_value, "100.000000000000000001");
        assert_eq!(row.current_value, "99.999999999999999999");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM validator_counter_history WHERE validator_id = $1"
            )
            .bind(&validator.validator_id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn reward_amount_decrease_is_recorded_as_a_counter_correction() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xreward", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    reward_amount: Some("500.000000000012".to_owned()),
                    ..Default::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    reward_amount: Some("499.999999999999".to_owned()),
                    ..Default::default()
                })),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        refresh_all(&db, &provider).await.unwrap();

        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        // The exact 12-decimal source value is retained and the falling
        // cumulative reward is flagged as a reset/correction rather than
        // silently rewritten as normal growth.
        assert_eq!(insight.reward_amount.as_deref(), Some("499.999999999999"));
        assert_eq!(insight.counter_state, "counter_reset");
        let row = sqlx::query_as::<_, ValidatorCounterHistoryRecord>(
            "SELECT history_id, validator_id, counter_name, previous_value, current_value, observed_at, provider_timestamp, observation_key FROM validator_counter_history WHERE validator_id = $1",
        )
        .bind(&validator.validator_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(row.counter_name, "reward_amount");
        assert_eq!(row.previous_value, "500.000000000012");
        assert_eq!(row.current_value, "499.999999999999");
    }

    #[tokio::test]
    async fn not_configured_network_is_explicit_and_retains_last_good_block_count() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Producing),
                    block_count: Some(100),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::NotConfigured("no bound deployment".to_owned()),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let first = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.outcome, "success");
        assert_eq!(first.block_count, Some(100));
        assert_eq!(first.activity.as_deref(), Some("producing"));

        let summary = refresh_all(&db, &provider).await.unwrap();
        assert_eq!(summary.attempted, 1);
        assert_eq!(summary.successful, 0);
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "not_configured");
        assert_eq!(
            insight.block_count,
            Some(100),
            "last-good count is retained"
        );
        assert_eq!(insight.activity.as_deref(), Some("producing"));
        assert_eq!(insight.last_good_received_at, first.last_good_received_at);
        assert_eq!(insight.provider_timestamp, first.provider_timestamp);
        assert_eq!(insight.counter_state, "normal");
    }

    #[tokio::test]
    async fn migrated_schema_preserves_existing_last_good_insight_row() {
        // Migration 0044 widens the outcome CHECK to allow not_configured. A
        // pre-existing unsupported row keeps its retained last-good values.
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        sqlx::query("INSERT INTO current_validator_insights (validator_id, source, outcome, diagnostic, provider_timestamp, last_attempt_received_at, last_good_received_at, block_count, counter_state, change_state, candidate_observations, updated_at) VALUES ($1, 'platscan', 'unsupported', 'legacy', '2025-01-01T00:00:00Z', $2, $3, 77, 'normal', 'normal', 0, $4)")
            .bind(&validator.validator_id)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .execute(db.pool())
            .await
            .unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "unsupported");
        assert_eq!(insight.block_count, Some(77));
        assert_eq!(insight.last_good_received_at.as_deref(), Some(now.as_str()));
        assert_eq!(
            insight.provider_timestamp.as_deref(),
            Some("2025-01-01T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn block_count_survives_database_reopen_without_fabricating_values() {
        let (dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    activity: Some(ValidatorActivity::Active),
                    block_count: Some(55),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    activity: Some(ValidatorActivity::Active),
                    ..ValidatorObservation::default()
                })),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let validator_id = validator.validator_id.clone();
        drop(db);

        let reopened = crate::test_support::test_database_in(dir.path()).await;
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "success");
        assert_eq!(insight.block_count, Some(55));

        // A successful observation that omits blockQty is Unknown, never a
        // fabricated zero.
        refresh_all(&reopened, &provider).await.unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.block_count, None);
    }

    #[tokio::test]
    async fn block_rates_persist_retain_last_good_and_never_splice_observations() {
        let (dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let validator_id = validator.validator_id.clone();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    provider_timestamp: Some("2025-01-01T00:00:00Z".to_owned()),
                    activity: Some(ValidatorActivity::Producing),
                    block_count: Some(100),
                    expected_block_count: Some(110),
                    gen_blocks_rate: Some("90.909091".to_owned()),
                    ..ValidatorObservation::default()
                },
            ))]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator_id).await.unwrap().unwrap();
        assert_eq!(insight.block_count, Some(100));
        assert_eq!(insight.expected_block_count, Some(110));
        assert_eq!(insight.gen_blocks_rate.as_deref(), Some("90.909091"));

        // Restart persistence: the rate rows survive a database reopen.
        drop(db);
        let reopened = crate::test_support::test_database_in(dir.path()).await;
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.expected_block_count, Some(110));
        assert_eq!(insight.gen_blocks_rate.as_deref(), Some("90.909091"));

        // A non-success outcome retains every last-good rate value.
        let failure = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Error(
                "platscan failed".to_owned(),
            )]),
            ..FakeProvider::default()
        };
        refresh_all(&reopened, &failure).await.unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.expected_block_count, Some(110));
        assert_eq!(insight.gen_blocks_rate.as_deref(), Some("90.909091"));
        assert_eq!(
            cumulative_block_rate(insight.block_count, insight.expected_block_count),
            (Some("90.909091".to_owned()), "ok")
        );

        // A later success with a fresh numerator but no denominator clears the
        // pair: the new numerator is never divided by the older denominator.
        let partial = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    activity: Some(ValidatorActivity::Producing),
                    block_count: Some(200),
                    ..ValidatorObservation::default()
                },
            ))]),
            ..FakeProvider::default()
        };
        refresh_all(&reopened, &partial).await.unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.block_count, Some(200));
        assert_eq!(insight.expected_block_count, None);
        assert_eq!(insight.gen_blocks_rate, None);
        assert_eq!(
            cumulative_block_rate(insight.block_count, insight.expected_block_count),
            (None, "unknown")
        );
    }

    #[tokio::test]
    async fn delegation_reward_percentage_persists_retains_last_good_and_survives_restart() {
        let (dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabc", None, &owner_id)
            .await
            .unwrap();
        let validator_id = validator.validator_id.clone();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    activity: Some(ValidatorActivity::Producing),
                    delegation_reward_percentage: Some("20".to_owned()),
                    ..ValidatorObservation::default()
                },
            ))]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator_id).await.unwrap().unwrap();
        assert_eq!(insight.delegation_reward_percentage.as_deref(), Some("20"));

        // The percentage survives a Server restart (database reopen).
        drop(db);
        let reopened = crate::test_support::test_database_in(dir.path()).await;
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.delegation_reward_percentage.as_deref(), Some("20"));

        // A non-success outcome retains the last-good percentage.
        let failure = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Error(
                "platscan failed".to_owned(),
            )]),
            ..FakeProvider::default()
        };
        refresh_all(&reopened, &failure).await.unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.delegation_reward_percentage.as_deref(), Some("20"));

        // A later success that omits rewardPer is Unknown, never carried from a
        // different observation and never fabricated as zero.
        let partial = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    activity: Some(ValidatorActivity::Producing),
                    ..ValidatorObservation::default()
                },
            ))]),
            ..FakeProvider::default()
        };
        refresh_all(&reopened, &partial).await.unwrap();
        let insight = load_insight(&reopened, &validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.delegation_reward_percentage, None);
    }

    #[tokio::test]
    async fn ranking_is_fetched_once_per_network_not_per_validator() {
        let (_dir, db) = test_db().await;
        create_network(
            &db,
            "net-a",
            "Network A",
            "0x00000000000000000000000000000000000000000000000000000000000000a1",
            11,
            11,
            "lat",
        )
        .await
        .unwrap();
        create_network(
            &db,
            "net-b",
            "Network B",
            "0x00000000000000000000000000000000000000000000000000000000000000b1",
            12,
            12,
            "lat",
        )
        .await
        .unwrap();
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        for node_id in ["0xa", "0xb"] {
            create_validator(&db, "net-a", node_id, None, &owner_id)
                .await
                .unwrap();
        }
        create_validator(&db, "net-b", "0xc", None, &owner_id)
            .await
            .unwrap();
        let detail = || {
            ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                stake_amount: Some("1".to_owned()),
                ..ValidatorObservation::default()
            }))
        };
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![detail(), detail(), detail()]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(3, &[("0xa", 1), ("0xb", 1), ("0xc", 1)]),
                ranking_with(3, &[("0xa", 1), ("0xb", 1), ("0xc", 1)]),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let calls = provider.ranking_calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "one ranking fetch per distinct Network");
        assert!(calls.iter().any(|key| key == "net-a"));
        assert!(calls.iter().any(|key| key == "net-b"));
        assert_eq!(provider.calls.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn only_a_complete_list_establishes_unranked_and_failures_retain_last_good() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xabsent", None, &owner_id)
            .await
            .unwrap();
        let detail = || {
            ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                stake_amount: Some("11".to_owned()),
                ..ValidatorObservation::default()
            }))
        };
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![detail(), detail(), detail()]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(2, &[("0xother", 1)]),
                RankingProviderResult::Error("incomplete page".to_owned()),
                ranking_with(2, &[("0xabsent", 1)]),
            ]),
            ..FakeProvider::default()
        };
        // A complete list that omits the Validator is an authoritative unranked.
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.rank_outcome.as_deref(), Some("success"));
        assert_eq!(insight.rank, None);
        // An incomplete or failed list must never be mistaken for unranked.
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.rank_outcome.as_deref(), Some("error"));
        assert_eq!(insight.rank, None);
        assert_eq!(insight.stake_amount.as_deref(), Some("11"));
        // A later complete list that contains the Validator sets the rank.
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.rank_outcome.as_deref(), Some("success"));
        assert_eq!(insight.rank, Some(1));
        assert_eq!(insight.rank_cohort_size, Some(2));
    }

    #[tokio::test]
    async fn ranking_failure_retains_a_last_good_rank_across_detail_failure() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xkeep", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![
                ValidatorProviderResult::Success(Box::new(ValidatorObservation {
                    stake_amount: Some("7".to_owned()),
                    ..ValidatorObservation::default()
                })),
                ValidatorProviderResult::Error("detail down".to_owned()),
            ]),
            rankings: std::sync::Mutex::new(vec![
                ranking_with(5, &[("0xkeep", 4)]),
                RankingProviderResult::Error("ranking down".to_owned()),
            ]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        // The detail failure retains the ranked value even though the ranking
        // list also failed: the two sources stay independent.
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "error");
        assert_eq!(insight.rank_outcome.as_deref(), Some("error"));
        assert_eq!(insight.rank, Some(4));
        assert_eq!(insight.stake_amount.as_deref(), Some("7"));
        // A detail success that omits a rank update keeps the ranking last-good.
        let partial = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    stake_amount: Some("8".to_owned()),
                    ..ValidatorObservation::default()
                },
            ))]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &partial).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.outcome, "success");
        assert_eq!(insight.stake_amount.as_deref(), Some("8"));
        assert_eq!(insight.rank, Some(4));
    }
    #[tokio::test]
    async fn rank_is_adopted_from_the_network_cohort_not_recomputed_over_monitored_nodes() {
        let (_dir, db) = test_db().await;
        let owner_id: String =
            sqlx::query_scalar("SELECT user_id FROM users WHERE username = 'owner'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let (validator, _) = create_validator(&db, "platon-mainnet", "0xtarget", None, &owner_id)
            .await
            .unwrap();
        let provider = FakeProvider {
            results: std::sync::Mutex::new(vec![ValidatorProviderResult::Success(Box::new(
                ValidatorObservation {
                    stake_amount: Some("1".to_owned()),
                    ..ValidatorObservation::default()
                },
            ))]),
            // The cohort contains unmonitored Validators, so the position of
            // the one monitored Validator is the upstream ALL-cohort rank.
            rankings: std::sync::Mutex::new(vec![ranking_with(
                10,
                &[
                    ("0xother-1", 1),
                    ("0xother-2", 2),
                    ("0xother-3", 3),
                    ("0xtarget", 4),
                ],
            )]),
            ..FakeProvider::default()
        };
        refresh_all(&db, &provider).await.unwrap();
        let insight = load_insight(&db, &validator.validator_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(insight.rank_outcome.as_deref(), Some("success"));
        assert_eq!(insight.rank, Some(4));
        assert_eq!(insight.rank_cohort_size, Some(10));
    }
}
