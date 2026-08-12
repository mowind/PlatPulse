-- Durable per-Agent report sequence gap evidence.
ALTER TABLE agents ADD COLUMN active_boot_status TEXT NOT NULL DEFAULT 'active' CHECK (active_boot_status IN ('active', 'closing', 'closed'));
ALTER TABLE agents ADD COLUMN previous_boot_id TEXT;
ALTER TABLE agents ADD COLUMN close_report_id TEXT;
ALTER TABLE agents ADD COLUMN close_applied_at TEXT;
ALTER TABLE agents ADD COLUMN security_event_count INTEGER NOT NULL DEFAULT 0 CHECK (security_event_count >= 0);

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
