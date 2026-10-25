-- Observation instants on the Server clock (ADR 0014, issue #274).
--
-- A Report carries the Agent's own wall-clock instants and keeps carrying them
-- however late it is delivered, so a Server that stamps a receipt instant
-- everywhere it records "when was this observed" measures delivery instead of
-- the Node. Ingestion therefore also records the observation instant corrected
-- by the clock skew the Report itself measured (positive = Agent clock ahead),
-- which is the instant Current Projection freshness, alert evidence and the
-- Observed Network Head eligibility are judged by.
--
-- `observation_at` describes the newest attempt (the state evidence a failed
-- collection also carries), `value_observation_at` describes the value the
-- Server still holds, and both fall back to the receipt instant when the skew is
-- unmeasured or the Agent is Clock Unreliable (ADR 0014). The existing
-- `observed_at` column keeps the Agent's raw instant for display; these are
-- Server-clock instants and are never mixed with it.
ALTER TABLE component_status ADD COLUMN observation_at TEXT;
ALTER TABLE component_status ADD COLUMN value_observation_at TEXT;
-- The head a Node reported is evidence at its own observation instant, so
-- block-stalled and head comparisons can tell a replayed head from a live one.
ALTER TABLE block_history_state ADD COLUMN observation_at TEXT;
