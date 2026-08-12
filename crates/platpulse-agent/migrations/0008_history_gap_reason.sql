-- Preserve the exact bounded recovery reason alongside the Agent-local gap.
ALTER TABLE history_gaps ADD COLUMN reason TEXT NOT NULL DEFAULT 'history gap';
