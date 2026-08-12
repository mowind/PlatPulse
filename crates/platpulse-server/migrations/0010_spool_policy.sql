-- Extended durable Agent spool capacity, loss, and fatal-state diagnostics.
ALTER TABLE current_host_observations ADD COLUMN spool_capacity_bytes INTEGER;
ALTER TABLE current_host_observations ADD COLUMN spool_max_age_seconds INTEGER;
ALTER TABLE current_host_observations ADD COLUMN spool_dropped_sequence_from INTEGER;
ALTER TABLE current_host_observations ADD COLUMN spool_dropped_sequence_to INTEGER;
ALTER TABLE current_host_observations ADD COLUMN spool_dropped_time_from TEXT;
ALTER TABLE current_host_observations ADD COLUMN spool_dropped_time_to TEXT;
ALTER TABLE current_host_observations ADD COLUMN spool_dropped_height_from INTEGER;
ALTER TABLE current_host_observations ADD COLUMN spool_dropped_height_to INTEGER;
ALTER TABLE current_host_observations ADD COLUMN spool_pending_history_gaps INTEGER;
ALTER TABLE current_host_observations ADD COLUMN spool_report_too_large INTEGER;
ALTER TABLE current_host_observations ADD COLUMN spool_store_fatal INTEGER;
ALTER TABLE current_host_observations ADD COLUMN spool_store_error TEXT;

CREATE TABLE agent_spool_diagnostics (
    agent_id TEXT PRIMARY KEY REFERENCES agents(agent_id),
    max_bytes INTEGER,
    max_age_seconds INTEGER,
    dropped_sequence_from INTEGER,
    dropped_sequence_to INTEGER,
    dropped_time_from TEXT,
    dropped_time_to TEXT,
    dropped_height_from INTEGER,
    dropped_height_to INTEGER,
    pending_history_gaps INTEGER,
    report_too_large INTEGER,
    store_fatal INTEGER,
    store_error TEXT,
    updated_at TEXT NOT NULL
);
CREATE INDEX agent_spool_diagnostics_updated_idx ON agent_spool_diagnostics (updated_at);
