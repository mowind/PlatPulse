-- Durable evidence for confirmed Validator ranking changes and cumulative corrections.
-- History is one row per Validator observation, never one row per linked Node.
ALTER TABLE current_validator_insights ADD COLUMN candidate_observed_at TEXT;
ALTER TABLE current_validator_insights ADD COLUMN candidate_provider_timestamp TEXT;
ALTER TABLE current_validator_insights ADD COLUMN candidate_observation_key TEXT;
ALTER TABLE validator_ranking_history ADD COLUMN candidate_observed_at TEXT;
ALTER TABLE validator_ranking_history ADD COLUMN candidate_provider_timestamp TEXT;
ALTER TABLE validator_ranking_history ADD COLUMN candidate_observation_key TEXT;

CREATE TABLE validator_counter_history (
    history_id TEXT PRIMARY KEY,
    validator_id TEXT NOT NULL REFERENCES validators(validator_id),
    counter_name TEXT NOT NULL CHECK (counter_name IN ('stake_amount', 'reward_amount', 'block_count')),
    previous_value TEXT NOT NULL,
    current_value TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    provider_timestamp TEXT,
    observation_key TEXT NOT NULL,
    UNIQUE (validator_id, counter_name, observation_key)
);

CREATE INDEX validator_counter_history_validator_idx
    ON validator_counter_history (validator_id, observed_at DESC);
