-- Server-owned current Validator insight and bounded ranking history.
-- Provider payloads never cross this schema; only normalized, exact values are stored.
CREATE TABLE current_validator_insights (
    validator_id TEXT PRIMARY KEY REFERENCES validators(validator_id),
    source TEXT,
    outcome TEXT NOT NULL CHECK (outcome IN ('success', 'not_found', 'empty', 'error', 'unsupported')),
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
    updated_at TEXT NOT NULL
);

CREATE INDEX current_validator_insights_outcome_idx
    ON current_validator_insights (outcome, last_attempt_received_at);

CREATE TABLE validator_ranking_history (
    history_id TEXT PRIMARY KEY,
    validator_id TEXT NOT NULL REFERENCES validators(validator_id),
    previous_rank INTEGER,
    current_rank INTEGER NOT NULL,
    observed_at TEXT NOT NULL,
    provider_timestamp TEXT,
    observation_key TEXT NOT NULL,
    UNIQUE (validator_id, observation_key)
);

CREATE INDEX validator_ranking_history_validator_idx
    ON validator_ranking_history (validator_id, observed_at DESC);
