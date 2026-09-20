-- #174: one-time cutover from the legacy manually-linked Validator model to
-- the automatic-identity model (#173, main design §15.5, ADR 0005).
--
-- The legacy generation is every Validator identity that no automatic Node
-- Validator Link (the #173 model) references. The cutover removes that
-- generation's manual Links, current snapshot, ranking/counter history and
-- daily/monthly aggregates, and the legacy identity itself, so a later
-- Provider refresh pass cannot re-open a generation that was already removed.
-- Automatic identities are shared and keep their own identity and history.
--
-- This is an explicit single destructive conversion, not ordinary retention.
-- Node block/Peer/metric monitoring history, existing Alert Incident evidence
-- and Audit are deliberately untouched; ordinary Node Purge is narrower still
-- and never runs this conversion.
--
-- Every statement runs inside the single migration transaction SQLx opens, so
-- a failure rolls the whole cutover back and Server startup fails rather than
-- serving a half-migrated database. The runtime writers (identity discovery and
-- the Provider refresh worker) are only started after this migration commits,
-- so no old writer can interleave with the cutover.

-- The durable one-time cutover boundary. Written last, after every deletion
-- has succeeded, so a reader can never observe the marker without the cleanup.
CREATE TABLE IF NOT EXISTS validator_model_migration (
    migration_key TEXT PRIMARY KEY CHECK (migration_key = 'validator_model'),
    migrated_at TEXT NOT NULL
);

-- Capture the legacy identities before the manual Links are removed. An
-- identity that already has an automatic Link belongs to the #173 model.
CREATE TEMP TABLE validator_model_legacy AS
SELECT validator_id FROM validators
WHERE NOT EXISTS (
    SELECT 1 FROM node_validator_links link
    WHERE link.validator_id = validators.validator_id
      AND link.origin = 'automatic'
);

-- Legacy manual Links immediately stop being a current association or a
-- fallback.
DELETE FROM node_validator_links WHERE origin = 'manual';

-- The legacy generation's retained state. Removing the current snapshot also
-- removes the counter/ranking candidate baselines that belonged to the old
-- model, so it cannot later be reported as a counter reset or ranking change.
DELETE FROM current_validator_insights
 WHERE validator_id IN (SELECT validator_id FROM validator_model_legacy);
DELETE FROM validator_ranking_history
 WHERE validator_id IN (SELECT validator_id FROM validator_model_legacy);
DELETE FROM validator_counter_history
 WHERE validator_id IN (SELECT validator_id FROM validator_model_legacy);
DELETE FROM validator_daily_snapshots
 WHERE validator_id IN (SELECT validator_id FROM validator_model_legacy);
DELETE FROM validator_monthly_aggregates
 WHERE validator_id IN (SELECT validator_id FROM validator_model_legacy);

-- Remove the legacy identity last, after every row that references it, so the
-- Provider refresh worker cannot recreate the generation it belonged to.
DELETE FROM validators
 WHERE validator_id IN (SELECT validator_id FROM validator_model_legacy);

DROP TABLE validator_model_legacy;

INSERT INTO validator_model_migration (migration_key, migrated_at)
VALUES ('validator_model', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
