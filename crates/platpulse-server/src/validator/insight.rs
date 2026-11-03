//! Current Validator insight and its ranking, counter and link history reads.

use crate::database::ServerDatabase;

use super::error::ValidatorError;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorInsightRecord {
    pub validator_id: String,
    pub source: Option<String>,
    pub outcome: String,
    pub diagnostic: Option<String>,
    pub provider_timestamp: Option<String>,
    pub activity: Option<String>,
    pub last_attempt_received_at: String,
    pub last_good_received_at: Option<String>,
    pub last_good_provider_timestamp: Option<String>,
    pub last_good_verdict_outcome: Option<String>,
    pub last_good_verdict_received_at: Option<String>,
    pub rank: Option<i64>,
    pub rank_outcome: Option<String>,
    pub rank_diagnostic: Option<String>,
    pub rank_last_attempt_received_at: Option<String>,
    pub rank_last_good_received_at: Option<String>,
    pub rank_cohort_size: Option<i64>,
    pub stake_amount: Option<String>,
    pub reward_amount: Option<String>,
    pub reward_rate: Option<String>,
    pub delegation_reward_percentage: Option<String>,
    pub delegator_count: Option<i64>,
    pub epoch: Option<i64>,
    pub block_count: Option<i64>,
    pub expected_block_count: Option<i64>,
    pub gen_blocks_rate: Option<String>,
    pub counter_state: String,
    pub change_state: String,
    pub candidate_previous_rank: Option<i64>,
    pub candidate_rank: Option<i64>,
    pub candidate_observations: i64,
    pub candidate_observed_at: Option<String>,
    pub candidate_provider_timestamp: Option<String>,
    pub candidate_observation_key: Option<String>,
    pub last_observation_key: Option<String>,
    pub updated_at: String,
}

/// The canonical column list for loading one current Validator insight. Kept in
/// one place so the detail apply, the ranking apply, and direct loads cannot
/// drift apart.
pub(super) const INSIGHT_SELECT: &str = "SELECT validator_id, source, outcome, diagnostic, provider_timestamp, activity, last_attempt_received_at, last_good_received_at, last_good_provider_timestamp, last_good_verdict_outcome, last_good_verdict_received_at, rank, rank_outcome, rank_diagnostic, rank_last_attempt_received_at, rank_last_good_received_at, rank_cohort_size, stake_amount, reward_amount, reward_rate, delegation_reward_percentage, delegator_count, epoch, block_count, expected_block_count, gen_blocks_rate, counter_state, change_state, candidate_previous_rank, candidate_rank, candidate_observations, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key, last_observation_key, updated_at FROM current_validator_insights WHERE validator_id = $1";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorRankingHistoryRecord {
    pub history_id: String,
    pub validator_id: String,
    pub previous_rank: Option<i64>,
    pub current_rank: i64,
    pub observed_at: String,
    pub provider_timestamp: Option<String>,
    pub observation_key: String,
    pub candidate_observed_at: Option<String>,
    pub candidate_provider_timestamp: Option<String>,
    pub candidate_observation_key: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorCounterHistoryRecord {
    pub history_id: String,
    pub validator_id: String,
    pub counter_name: String,
    pub previous_value: String,
    pub current_value: String,
    pub observed_at: String,
    pub provider_timestamp: Option<String>,
    pub observation_key: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ValidatorLinkContextRecord {
    pub link_id: String,
    pub node_id: String,
    pub role: Option<String>,
    pub valid_from: String,
    pub valid_until: Option<String>,
}

pub async fn list_ranking_history(
    db: &ServerDatabase,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<ValidatorRankingHistoryRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorRankingHistoryRecord>(
        "SELECT history_id, validator_id, previous_rank, current_rank, observed_at, provider_timestamp, observation_key, candidate_observed_at, candidate_provider_timestamp, candidate_observation_key FROM validator_ranking_history WHERE validator_id = $1 ORDER BY observed_at DESC, history_id DESC LIMIT $2",
    )
    .bind(validator_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?)
}

pub async fn list_counter_history(
    db: &ServerDatabase,
    validator_id: &str,
    limit: i64,
) -> Result<Vec<ValidatorCounterHistoryRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorCounterHistoryRecord>(
        "SELECT history_id, validator_id, counter_name, previous_value, current_value, observed_at, provider_timestamp, observation_key FROM validator_counter_history WHERE validator_id = $1 ORDER BY observed_at DESC, history_id DESC LIMIT $2",
    )
    .bind(validator_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?)
}

pub async fn list_link_context_at(
    db: &ServerDatabase,
    validator_id: &str,
    observed_at: &str,
    active_only: bool,
) -> Result<Vec<ValidatorLinkContextRecord>, ValidatorError> {
    let mut sql = String::from(
        "SELECT l.link_id, l.node_id, l.role, l.valid_from, l.valid_until FROM node_validator_links l JOIN nodes n ON n.node_id = l.node_id WHERE l.validator_id = $1 AND l.valid_from <= $2 AND (l.valid_until IS NULL OR l.valid_until > $3)",
    );
    if active_only {
        sql.push_str(" AND n.lifecycle = 'active'");
    }
    sql.push_str(" ORDER BY l.node_id, l.link_id");
    Ok(sqlx::query_as::<_, ValidatorLinkContextRecord>(&sql)
        .bind(validator_id)
        .bind(observed_at)
        .bind(observed_at)
        .fetch_all(db.pool())
        .await?)
}

pub async fn load_insight(
    db: &ServerDatabase,
    validator_id: &str,
) -> Result<Option<ValidatorInsightRecord>, ValidatorError> {
    Ok(sqlx::query_as::<_, ValidatorInsightRecord>(INSIGHT_SELECT)
        .bind(validator_id)
        .fetch_optional(db.pool())
        .await?)
}

pub async fn list_insights(
    db: &ServerDatabase,
    network_key: Option<&str>,
) -> Result<Vec<ValidatorInsightRecord>, ValidatorError> {
    let mut sql = String::from(
        "SELECT i.validator_id, i.source, i.outcome, i.diagnostic, i.provider_timestamp, i.activity, i.last_attempt_received_at, i.last_good_received_at, i.last_good_provider_timestamp, i.last_good_verdict_outcome, i.last_good_verdict_received_at, i.rank, i.rank_outcome, i.rank_diagnostic, i.rank_last_attempt_received_at, i.rank_last_good_received_at, i.rank_cohort_size, i.stake_amount, i.reward_amount, i.reward_rate, i.delegation_reward_percentage, i.delegator_count, i.epoch, i.block_count, i.expected_block_count, i.gen_blocks_rate, i.counter_state, i.change_state, i.candidate_previous_rank, i.candidate_rank, i.candidate_observations, i.candidate_observed_at, i.candidate_provider_timestamp, i.candidate_observation_key, i.last_observation_key, i.updated_at FROM current_validator_insights i JOIN validators v ON v.validator_id = i.validator_id",
    );
    if network_key.is_some() {
        sql.push_str(" WHERE v.network_key = $1");
    }
    sql.push_str(" ORDER BY v.network_key, v.validator_node_id, i.validator_id");
    let query = sqlx::query_as::<_, ValidatorInsightRecord>(&sql);
    let rows = if let Some(network_key) = network_key {
        query.bind(network_key).fetch_all(db.pool()).await?
    } else {
        query.fetch_all(db.pool()).await?
    };
    Ok(rows)
}
