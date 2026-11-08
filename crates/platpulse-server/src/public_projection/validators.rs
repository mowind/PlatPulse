//! The Validators the Public Projection shows: those linked to the listed
//! Active Nodes by an effective automatic Node Validator Link.

use time::OffsetDateTime;

use crate::database::ServerDatabase;
use crate::http::public::{
    PublicNode, PublicValidatorBlockTotal, PublicValidatorInsight, PublicValidatorRewardTotal,
    PublicValidatorSummary,
};
use crate::validator;

#[derive(Debug, Clone, sqlx::FromRow)]
pub(super) struct PublicValidatorRow {
    validator_id: String,
    network_key: String,
    validator_node_id: String,
    display_name: Option<String>,
    source: Option<String>,
    outcome: Option<String>,
    provider_timestamp: Option<String>,
    activity: Option<String>,
    last_good_received_at: Option<String>,
    last_good_verdict_outcome: Option<String>,
    last_good_verdict_received_at: Option<String>,
    rank: Option<i64>,
    rank_outcome: Option<String>,
    rank_last_good_received_at: Option<String>,
    rank_cohort_size: Option<i64>,
    stake_amount: Option<String>,
    reward_amount: Option<String>,
    reward_rate: Option<String>,
    delegation_reward_percentage: Option<String>,
    delegator_count: Option<i64>,
    epoch: Option<i64>,
    block_count: Option<i64>,
    expected_block_count: Option<i64>,
    gen_blocks_rate: Option<String>,
    counter_state: Option<String>,
}

/// Map the linked Validator's independent Network ranking state. Only a
/// complete successful list establishes `ranked` or `unranked`; a failed or
/// incomplete fetch retains a last-good rank and reports stale freshness,
/// while a failure without one is `unknown` and never `unranked` (#158).
fn public_validator_rank(
    rank_outcome: Option<&str>,
    rank: Option<i64>,
    last_good_received_at: Option<&str>,
    now: time::OffsetDateTime,
    stale_after_seconds: i64,
) -> (String, String) {
    // A failed fetch with a retained value is stale; without one it is unknown.
    let retained_freshness = if rank.is_some() { "stale" } else { "unknown" };
    match rank_outcome {
        Some("success") => (
            if rank.is_some() { "ranked" } else { "unranked" }.to_owned(),
            validator::freshness(last_good_received_at, now, stale_after_seconds).to_owned(),
        ),
        Some("error") => ("error".to_owned(), retained_freshness.to_owned()),
        Some("not_configured") => ("not_configured".to_owned(), retained_freshness.to_owned()),
        Some("unsupported") => ("unsupported".to_owned(), retained_freshness.to_owned()),
        _ => ("unknown".to_owned(), "unknown".to_owned()),
    }
}

/// The insights of the Validators that `links` reference in one Network, each
/// carrying the Node of its most recent Link (the lowest Link ID among equally
/// recent ones). `links` are the effective automatic Links of the Active Nodes
/// being projected, so a Validator linked only to a retired Node is not
/// eligible.
pub(super) fn validator_insights(
    rows: &[PublicValidatorRow],
    network_key: &str,
    links: &[validator::EffectiveAutomaticLink],
    at: OffsetDateTime,
    stale_after_seconds: i64,
) -> Vec<PublicValidatorInsight> {
    let mut linked_nodes: std::collections::BTreeMap<&str, &validator::EffectiveAutomaticLink> =
        std::collections::BTreeMap::new();
    for link in links {
        let newer = linked_nodes
            .get(link.validator_id.as_str())
            .is_none_or(|current| {
                (
                    link.valid_from.as_str(),
                    std::cmp::Reverse(link.link_id.as_str()),
                ) > (
                    current.valid_from.as_str(),
                    std::cmp::Reverse(current.link_id.as_str()),
                )
            });
        if newer {
            linked_nodes.insert(link.validator_id.as_str(), link);
        }
    }
    rows.iter()
        .filter(|row| row.network_key == network_key)
        .filter_map(|row| {
            let node_id = linked_nodes
                .get(row.validator_id.as_str())
                .map(|link| link.node_id.clone())?;
            Some((row.clone(), node_id))
        })
        .map(|(row, node_id)| {
            let freshness = validator::freshness(
                row.last_good_received_at.as_deref(),
                at,
                stale_after_seconds,
            );
            let outcome = row.outcome.unwrap_or_else(|| "not_configured".to_owned());
            let state = if outcome == "success" {
                freshness
            } else {
                outcome.as_str()
            };
            let verdict_freshness = validator::freshness(
                row.last_good_verdict_received_at.as_deref(),
                at,
                stale_after_seconds,
            );
            let (activity, activity_state, status) = validator::project_verdict(
                &outcome,
                row.last_good_verdict_outcome.as_deref(),
                row.activity.as_deref(),
                verdict_freshness,
            );
            let (rank_state, rank_freshness) = public_validator_rank(
                row.rank_outcome.as_deref(),
                row.rank,
                row.rank_last_good_received_at.as_deref(),
                at,
                stale_after_seconds,
            );
            let (block_rate, block_rate_state) =
                validator::cumulative_block_rate(row.block_count, row.expected_block_count);
            let current_validator_status = status.status.as_str().to_owned();
            let current_validator_status_state = status.state.to_owned();
            let current_validator_status_qualifier =
                status.qualifier.map(|value| value.as_str().to_owned());
            PublicValidatorInsight {
                validator_id: row.validator_id,
                validator_node_id: row.validator_node_id,
                display_name: row.display_name,
                node_id: Some(node_id),
                state: state.to_owned(),
                freshness: freshness.to_owned(),
                source: row.source.or_else(|| Some("disabled".to_owned())),
                provider_timestamp: row.provider_timestamp,
                received_at: row.last_good_received_at,
                rank: row.rank,
                rank_state,
                rank_freshness,
                rank_received_at: row.rank_last_good_received_at,
                rank_cohort_size: row.rank_cohort_size,
                stake_amount: row.stake_amount,
                reward_amount: row.reward_amount,
                reward_rate: row.reward_rate,
                delegation_reward_percentage: row.delegation_reward_percentage,
                delegator_count: row.delegator_count,
                epoch: row.epoch,
                block_count: row.block_count,
                expected_block_count: row.expected_block_count,
                block_rate,
                block_rate_state: block_rate_state.to_owned(),
                gen_blocks_rate: row.gen_blocks_rate,
                counter_state: row.counter_state.unwrap_or_else(|| "normal".to_owned()),
                activity,
                activity_state,
                activity_received_at: row.last_good_verdict_received_at,
                current_validator_status,
                current_validator_status_state,
                current_validator_status_qualifier,
            }
        })
        .collect()
}

/// Read the rows of the listed Validators with their current insights, ordered
/// by Network and chain identity. No ID reads nothing.
pub(super) async fn load_validator_rows(
    db: &ServerDatabase,
    validator_ids: &[&str],
) -> Result<Vec<PublicValidatorRow>, sqlx::Error> {
    if validator_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; validator_ids.len()].join(", ");
    let sql = crate::database::number_placeholders(&format!(
        "SELECT v.validator_id, v.network_key, v.validator_node_id, v.display_name, i.source, i.outcome, i.provider_timestamp, i.activity, i.last_good_received_at, i.last_good_verdict_outcome, i.last_good_verdict_received_at, i.rank, i.rank_outcome, i.rank_last_good_received_at, i.rank_cohort_size, i.stake_amount, i.reward_amount, i.reward_rate, i.delegation_reward_percentage, i.delegator_count, i.epoch, i.block_count, i.expected_block_count, i.gen_blocks_rate, i.counter_state FROM validators v LEFT JOIN current_validator_insights i ON i.validator_id = v.validator_id WHERE v.validator_id IN ({placeholders}) ORDER BY v.network_key, v.validator_node_id, v.validator_id"
    ));
    let mut query = sqlx::query_as::<_, PublicValidatorRow>(&sql);
    for validator_id in validator_ids {
        query = query.bind(*validator_id);
    }
    query.fetch_all(db.pool()).await
}

/// The effective automatic Links of the listed Active Nodes. Activity is
/// associated per Node, so a Validator linked concurrently to several Nodes is
/// visible on every one of them.
pub(super) fn listed_links(
    links: &[validator::EffectiveAutomaticLink],
    nodes: &[PublicNode],
) -> Vec<validator::EffectiveAutomaticLink> {
    links
        .iter()
        .filter(|link| nodes.iter().any(|node| node.node_id == link.node_id))
        .cloned()
        .collect()
}

/// Attach each Node's automatically identified Validator insight. Only an
/// effective automatic Link is used; a legacy manual Link is never a fallback.
/// A Node without an effective automatic Link keeps a `None` Validator and
/// renders the explicit identity reason instead of a guessed association
/// (#173).
pub(super) fn associate_node_validators(
    validators: &[PublicValidatorInsight],
    links: &[validator::EffectiveAutomaticLink],
    nodes: &mut [PublicNode],
) {
    for node in nodes.iter_mut() {
        let Some(link) = links.iter().find(|link| link.node_id == node.node_id) else {
            continue;
        };
        let Some(validator) = validators
            .iter()
            .find(|validator| validator.validator_id == link.validator_id)
        else {
            continue;
        };
        let mut associated = validator.clone();
        associated.node_id = Some(link.node_id.clone());
        node.validator = Some(associated);
    }
}

/// Coverage label for one metric: a total is never presented as complete while
/// an eligible Validator is missing a value, and a total with no values at all
/// is unknown rather than zero.
fn validation_coverage_state(valued_count: i64, expected_count: i64) -> &'static str {
    if valued_count == 0 {
        "unknown"
    } else if valued_count < expected_count {
        "partial"
    } else {
        "complete"
    }
}

/// Reduce the effective-link Validator set to one Network's deduplicated
/// cumulative totals. `validators` is already the distinct eligible set (one
/// row per Validator referenced by an effective Link on an Active Node), so a
/// primary/standby/observer pair sharing one Validator contributes once. Each
/// metric keeps its own coverage counts: a Validator can have a block count and
/// no reward, or vice versa.
pub(crate) fn public_validator_summary(
    validators: &[PublicValidatorInsight],
    nodes: &[PublicNode],
) -> PublicValidatorSummary {
    let expected_count = validators.len() as i64;
    let mut block_sum: Option<String> = None;
    let mut block_valued = 0_i64;
    let mut block_stale = 0_i64;
    let mut reward_sum: Option<String> = None;
    let mut reward_valued = 0_i64;
    let mut reward_stale = 0_i64;
    for validator in validators {
        // A retained last-good value is stale unless the latest successful
        // observation is still current; Provider detail state is unaffected by
        // the independent ranking collection.
        let stale = validator.state != "fresh";
        if let Some(value) = validator.block_count.filter(|value| *value >= 0) {
            // Every accepted i64 count is a valid bounded decimal; keep the
            // aggregate as digits instead of saturating or sending a JSON number.
            validator::accumulate_decimal(&mut block_sum, &value.to_string());
            block_valued += 1;
            if stale {
                block_stale += 1;
            }
        }
        if let Some(value) = validator.reward_amount.as_deref() {
            if validator::accumulate_decimal(&mut reward_sum, value) {
                reward_valued += 1;
                if stale {
                    reward_stale += 1;
                }
            }
        }
    }
    let linked_node_count = nodes.iter().filter(|node| node.validator.is_some()).count() as i64;
    PublicValidatorSummary {
        blocks: PublicValidatorBlockTotal {
            known_sum: block_sum,
            expected_count,
            valued_count: block_valued,
            stale_count: block_stale,
            state: validation_coverage_state(block_valued, expected_count).to_owned(),
        },
        rewards: PublicValidatorRewardTotal {
            known_sum: reward_sum,
            expected_count,
            valued_count: reward_valued,
            stale_count: reward_stale,
            state: validation_coverage_state(reward_valued, expected_count).to_owned(),
        },
        eligible_validator_count: expected_count,
        linked_node_count,
        unlinked_node_count: nodes.len() as i64 - linked_node_count,
    }
}
