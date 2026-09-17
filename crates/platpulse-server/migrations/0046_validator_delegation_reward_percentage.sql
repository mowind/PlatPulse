-- #157: persist the currently effective delegation reward distribution
-- percentage (PlatScan detail `rewardPer`) alongside the existing Validator
-- insight. The column is nullable so pre-existing rows stay Unknown instead of
-- acquiring a fabricated value; the migration copies nothing and rewrites
-- nothing. The upstream detail response already scales its internal basis
-- points, so a source `rewardPer` of 20 is stored as the percentage 20 and
-- rendered as 20%.
ALTER TABLE current_validator_insights ADD COLUMN delegation_reward_percentage TEXT;
