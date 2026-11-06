-- Backfill the observation instant of component rows written before ingestion
-- recorded one (ADR 0014).
--
-- Migration 0078 added `component_status.observation_at` without filling the
-- rows that already existed, so every reader of Node currency fell back to the
-- receipt instant with COALESCE(observation_at, received_at). Ingestion falls
-- back to the same receipt instant when it cannot correct for clock skew, so
-- copying `received_at` into the rows still without an observation instant is
-- exactly the instant those readers already used. A row that was never received
-- keeps no observation instant. The column stays nullable.
UPDATE component_status
SET observation_at = received_at
WHERE observation_at IS NULL AND received_at IS NOT NULL;
