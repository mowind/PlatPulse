-- #158: dedicated Server-side aliveStakingList ranking state, independent from
-- the per-Validator detail observation. The existing `rank` column is reused
-- as the ranking last-good value. Every new column is nullable so pre-existing
-- rows stay Unknown instead of acquiring a fabricated rank; no value is
-- backfilled. Only a complete successful list fetch may clear `rank` to NULL
-- (a genuine unranked outcome); a failed, truncated, duplicate, or drifted
-- fetch retains the last-good rank and never fabricates an unranked state.
ALTER TABLE current_validator_insights ADD COLUMN rank_outcome TEXT
    CHECK (rank_outcome IN ('success', 'error', 'not_configured', 'unsupported'));
ALTER TABLE current_validator_insights ADD COLUMN rank_diagnostic TEXT;
ALTER TABLE current_validator_insights ADD COLUMN rank_last_attempt_received_at TEXT;
ALTER TABLE current_validator_insights ADD COLUMN rank_last_good_received_at TEXT;
ALTER TABLE current_validator_insights ADD COLUMN rank_cohort_size INTEGER;

-- The detail response has no authoritative ranking alias: any pre-existing
-- `rank` value came from an unreliable detail alias, not from a ranking list.
-- Clear it so upgraded rows stay Unknown instead of exposing an untrusted
-- position, and never backfill a fabricated rank. The ranking pipeline then
-- writes the first authoritative value on its next successful list fetch.
UPDATE current_validator_insights SET rank = NULL;
