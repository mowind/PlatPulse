-- Split PlatScan `candidate` (status 1) from `active` (status 2) in the
-- canonical Validator Activity. SQLite cannot widen a CHECK constraint in
-- place, so rebuild the table and copy every existing row verbatim: no stored
-- value is rewritten or fabricated, and last-good rows survive the migration
-- unchanged.
CREATE TABLE current_validator_insights_new (
    validator_id TEXT PRIMARY KEY REFERENCES validators(validator_id),
    source TEXT,
    outcome TEXT NOT NULL CHECK (outcome IN ('success', 'not_found', 'empty', 'not_configured', 'error', 'unsupported')),
    diagnostic TEXT,
    provider_timestamp TEXT,
    last_attempt_received_at TEXT NOT NULL,
    last_good_received_at TEXT,
    last_good_provider_timestamp TEXT,
    rank INTEGER,
    stake_amount TEXT,
    reward_amount TEXT,
    reward_rate TEXT,
    delegator_count INTEGER,
    epoch INTEGER,
    block_count INTEGER,
    counter_state TEXT NOT NULL DEFAULT 'normal' CHECK (counter_state IN ('normal', 'counter_reset')),
    candidate_previous_rank INTEGER,
    candidate_rank INTEGER,
    candidate_observations INTEGER NOT NULL DEFAULT 0,
    last_observation_key TEXT,
    updated_at TEXT NOT NULL,
    change_state TEXT NOT NULL DEFAULT 'normal' CHECK (change_state IN ('normal', 'ranking_changed', 'counter_reset')),
    candidate_observed_at TEXT,
    candidate_provider_timestamp TEXT,
    candidate_observation_key TEXT,
    activity TEXT CHECK (activity IN ('candidate', 'active', 'producing', 'exiting', 'exited', 'verifying', 'locked')),
    expected_block_count INTEGER,
    gen_blocks_rate TEXT,
    delegation_reward_percentage TEXT,
    rank_outcome TEXT CHECK (rank_outcome IN ('success', 'error', 'not_configured', 'unsupported')),
    rank_diagnostic TEXT,
    rank_last_attempt_received_at TEXT,
    rank_last_good_received_at TEXT,
    rank_cohort_size INTEGER
);

INSERT INTO current_validator_insights_new (
    validator_id, source, outcome, diagnostic, provider_timestamp,
    last_attempt_received_at, last_good_received_at, last_good_provider_timestamp,
    rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch,
    block_count, counter_state, candidate_previous_rank, candidate_rank,
    candidate_observations, last_observation_key, updated_at, change_state,
    candidate_observed_at, candidate_provider_timestamp, candidate_observation_key,
    activity, expected_block_count, gen_blocks_rate, delegation_reward_percentage,
    rank_outcome, rank_diagnostic, rank_last_attempt_received_at,
    rank_last_good_received_at, rank_cohort_size
) SELECT
    validator_id, source, outcome, diagnostic, provider_timestamp,
    last_attempt_received_at, last_good_received_at, last_good_provider_timestamp,
    rank, stake_amount, reward_amount, reward_rate, delegator_count, epoch,
    block_count, counter_state, candidate_previous_rank, candidate_rank,
    candidate_observations, last_observation_key, updated_at, change_state,
    candidate_observed_at, candidate_provider_timestamp, candidate_observation_key,
    activity, expected_block_count, gen_blocks_rate, delegation_reward_percentage,
    rank_outcome, rank_diagnostic, rank_last_attempt_received_at,
    rank_last_good_received_at, rank_cohort_size
FROM current_validator_insights;

DROP TABLE current_validator_insights;
ALTER TABLE current_validator_insights_new RENAME TO current_validator_insights;

CREATE INDEX current_validator_insights_outcome_idx
    ON current_validator_insights (outcome, last_attempt_received_at);
