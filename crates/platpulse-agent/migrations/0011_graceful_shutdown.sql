-- Durable graceful-shutdown and recovery diagnostics.
ALTER TABLE agent_state ADD COLUMN shutdown_state TEXT NOT NULL DEFAULT 'running' CHECK (shutdown_state IN ('running', 'stopping', 'draining', 'final_stored', 'send_failed', 'forced_kill_recovery'));
ALTER TABLE agent_state ADD COLUMN shutdown_started_at TEXT;
ALTER TABLE agent_state ADD COLUMN shutdown_deadline_at TEXT;
ALTER TABLE agent_state ADD COLUMN shutdown_finished_at TEXT;
ALTER TABLE agent_state ADD COLUMN shutdown_unresolved_from INTEGER;
ALTER TABLE agent_state ADD COLUMN shutdown_unresolved_to INTEGER;
ALTER TABLE agent_state ADD COLUMN shutdown_last_error TEXT;
ALTER TABLE agent_state ADD COLUMN shutdown_forced INTEGER NOT NULL DEFAULT 0 CHECK (shutdown_forced IN (0, 1));
ALTER TABLE agent_state ADD COLUMN shutdown_report_id TEXT;
ALTER TABLE agent_state ADD COLUMN shutdown_report_sequence INTEGER;
ALTER TABLE agent_state ADD COLUMN shutdown_updated_at TEXT;
CREATE INDEX agent_state_shutdown_idx ON agent_state(shutdown_state, shutdown_updated_at);
