-- #156: persist the cumulative-completion denominator and PlatScan's own
-- 24-hour production rate alongside the existing Validator insight. Both
-- columns are nullable so pre-existing rows stay Unknown instead of acquiring
-- a fabricated value; the migration copies nothing and rewrites nothing.
ALTER TABLE current_validator_insights ADD COLUMN expected_block_count INTEGER;
ALTER TABLE current_validator_insights ADD COLUMN gen_blocks_rate TEXT;
