-- Bounded graceful Agent shutdown/recovery diagnostics retained at Server.
ALTER TABLE agents ADD COLUMN shutdown_state TEXT NOT NULL DEFAULT 'unknown' CHECK (shutdown_state IN ('unknown', 'running', 'stopping', 'draining', 'final_stored', 'send_failed', 'forced_kill_recovery'));
ALTER TABLE agents ADD COLUMN shutdown_started_at TEXT;
ALTER TABLE agents ADD COLUMN shutdown_deadline_at TEXT;
ALTER TABLE agents ADD COLUMN shutdown_finished_at TEXT;
ALTER TABLE agents ADD COLUMN shutdown_unresolved_from INTEGER;
ALTER TABLE agents ADD COLUMN shutdown_unresolved_to INTEGER;
ALTER TABLE agents ADD COLUMN shutdown_last_error TEXT;
ALTER TABLE agents ADD COLUMN shutdown_forced INTEGER NOT NULL DEFAULT 0 CHECK (shutdown_forced IN (0, 1));
ALTER TABLE agents ADD COLUMN shutdown_report_id TEXT;
ALTER TABLE agents ADD COLUMN shutdown_report_sequence INTEGER;
ALTER TABLE agents ADD COLUMN shutdown_updated_at TEXT;
CREATE INDEX agents_shutdown_idx ON agents(shutdown_state, shutdown_updated_at);
