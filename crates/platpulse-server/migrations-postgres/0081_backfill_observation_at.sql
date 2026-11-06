-- Backfill the observation instant of component rows written before ingestion
-- recorded one (ADR 0014).
--
-- PostgreSQL counterpart of migrations/0081_backfill_observation_at.sql; both
-- backends share the logical schema version 81. Rows still without an
-- observation instant take their receipt instant, the instant every reader
-- already fell back to; a row that was never received keeps none, and the
-- column stays nullable.
UPDATE component_status
SET observation_at = received_at
WHERE observation_at IS NULL AND received_at IS NOT NULL;
