-- Durable Validator analytics samples. One selected successful observation per
-- Validator and configured local calendar day; the unique observation key makes
-- exact retries harmless while later timestamped observations replace an older
-- delayed sample deterministically.
CREATE TABLE validator_daily_snapshots (
    snapshot_id TEXT PRIMARY KEY,
    validator_id TEXT NOT NULL REFERENCES validators(validator_id),
    timezone TEXT NOT NULL,
    local_date TEXT NOT NULL,
    month_key TEXT NOT NULL,
    sample_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    provider_timestamp TEXT,
    source TEXT NOT NULL,
    observation_key TEXT NOT NULL,
    rank INTEGER,
    stake_amount TEXT,
    reward_amount TEXT,
    reward_rate TEXT,
    delegator_count INTEGER,
    epoch INTEGER,
    block_count INTEGER,
    UNIQUE (validator_id, timezone, local_date)
);

CREATE INDEX validator_daily_snapshots_lookup_idx
    ON validator_daily_snapshots (validator_id, timezone, local_date DESC);
CREATE INDEX validator_daily_snapshots_month_idx
    ON validator_daily_snapshots (validator_id, timezone, month_key, sample_at DESC);

-- Monthly aggregates are rebuilt from the durable daily rows in the same
-- transaction as a daily upsert. Keeping the aggregate row separate means API
-- reads remain bounded and late observations can correct a calendar month
-- without multiplying values for linked Nodes.
CREATE TABLE validator_monthly_aggregates (
    aggregate_id TEXT PRIMARY KEY,
    validator_id TEXT NOT NULL REFERENCES validators(validator_id),
    timezone TEXT NOT NULL,
    month_key TEXT NOT NULL,
    snapshot_count INTEGER NOT NULL CHECK (snapshot_count >= 0),
    first_sample_at TEXT NOT NULL,
    last_sample_at TEXT NOT NULL,
    rank_min INTEGER,
    rank_max INTEGER,
    rank_last INTEGER,
    stake_last TEXT,
    reward_last TEXT,
    reward_rate_last TEXT,
    delegator_count_last INTEGER,
    epoch_last INTEGER,
    block_count_last INTEGER,
    updated_at TEXT NOT NULL,
    UNIQUE (validator_id, timezone, month_key)
);

CREATE INDEX validator_monthly_aggregates_lookup_idx
    ON validator_monthly_aggregates (validator_id, timezone, month_key DESC);
