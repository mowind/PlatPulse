-- Durable Agent boot lifecycle and recovery-drain state.
ALTER TABLE agent_state ADD COLUMN previous_boot_id TEXT;
ALTER TABLE agent_state ADD COLUMN boot_state TEXT NOT NULL DEFAULT 'active' CHECK (boot_state IN ('active', 'draining', 'drained_pending'));
ALTER TABLE agent_state ADD COLUMN pending_transition TEXT CHECK (pending_transition IS NULL OR pending_transition IN ('closing', 'drained_previous'));
ALTER TABLE agent_state ADD COLUMN pending_previous_boot_id TEXT;
ALTER TABLE agent_state ADD COLUMN close_report_id TEXT;
ALTER TABLE agent_state ADD COLUMN close_applied_at TEXT;

CREATE TABLE agent_boots (
    agent_id TEXT NOT NULL,
    agent_epoch INTEGER NOT NULL CHECK (agent_epoch >= 0),
    boot_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'closing', 'closed')),
    previous_boot_id TEXT,
    last_sequence INTEGER NOT NULL DEFAULT 0 CHECK (last_sequence >= 0),
    close_report_id TEXT,
    closed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (agent_id, agent_epoch, boot_id)
);
CREATE UNIQUE INDEX agent_boots_one_active
    ON agent_boots(agent_id, agent_epoch) WHERE status IN ('active', 'closing');
CREATE INDEX agent_boots_status_idx ON agent_boots(agent_id, status, updated_at);
