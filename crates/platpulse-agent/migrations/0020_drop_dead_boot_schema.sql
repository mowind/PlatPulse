-- Remove Boot lifecycle schema no Agent code writes or reads: the 'closing'
-- pending transition, the 'stopping' shutdown state and the agent_boots table.
-- SQLite cannot alter a CHECK constraint in place, so each narrowed column is
-- rebuilt under its own name. A value the narrowed constraint refuses was never
-- written by any release and maps to the column's neutral value.

ALTER TABLE agent_state ADD COLUMN pending_transition_v20 TEXT
    CHECK (pending_transition_v20 IS NULL OR pending_transition_v20 = 'drained_previous');
UPDATE agent_state SET pending_transition_v20 =
    CASE WHEN pending_transition = 'drained_previous' THEN 'drained_previous' END;
ALTER TABLE agent_state DROP COLUMN pending_transition;
ALTER TABLE agent_state RENAME COLUMN pending_transition_v20 TO pending_transition;

DROP INDEX agent_state_shutdown_idx;
ALTER TABLE agent_state ADD COLUMN shutdown_state_v20 TEXT NOT NULL DEFAULT 'running'
    CHECK (shutdown_state_v20 IN ('running', 'draining', 'final_stored', 'send_failed', 'forced_kill_recovery'));
UPDATE agent_state SET shutdown_state_v20 =
    CASE WHEN shutdown_state = 'stopping' THEN 'running' ELSE shutdown_state END;
ALTER TABLE agent_state DROP COLUMN shutdown_state;
ALTER TABLE agent_state RENAME COLUMN shutdown_state_v20 TO shutdown_state;
CREATE INDEX agent_state_shutdown_idx ON agent_state(shutdown_state, shutdown_updated_at);

DROP TABLE agent_boots;
