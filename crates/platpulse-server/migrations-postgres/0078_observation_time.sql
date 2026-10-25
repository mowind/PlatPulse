-- Observation instants on the Server clock (ADR 0014, issue #274).
--
-- PostgreSQL counterpart of migrations/0078_observation_time.sql; both backends
-- share the logical schema version 78. Ingestion records the observation instant
-- of a component and of the value it still holds, corrected by the clock skew
-- the Report itself measured, and the observation instant of the head behind
-- block_history_state. See the SQLite migration for the fallback rules and for
-- why the existing Agent-clock `observed_at` column keeps its meaning.
ALTER TABLE component_status ADD COLUMN observation_at TEXT COLLATE "C";
ALTER TABLE component_status ADD COLUMN value_observation_at TEXT COLLATE "C";
ALTER TABLE block_history_state ADD COLUMN observation_at TEXT COLLATE "C";
