-- Preserve the exact bounded recovery reason alongside the gap interval.
ALTER TABLE block_history_gaps ADD COLUMN reason TEXT NOT NULL DEFAULT 'history gap';
