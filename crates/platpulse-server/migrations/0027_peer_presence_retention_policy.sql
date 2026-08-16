-- Add the Phase 3 Peer presence retention family to the policy CHECK
-- constraint without rewriting existing Owner-configured values.
ALTER TABLE retention_policies RENAME TO retention_policies_old;

CREATE TABLE retention_policies (
    family TEXT PRIMARY KEY CHECK (family IN (
        'raw_block_summary',
        'one_minute_aggregate',
        'one_hour_aggregate',
        'history_gap',
        'divergence_observation',
        'audit_event',
        'alert_notification',
        'peer_presence_interval'
    )),
    retention_days INTEGER NOT NULL CHECK (retention_days >= 0),
    min_days INTEGER NOT NULL CHECK (min_days >= 0),
    max_days INTEGER NOT NULL CHECK (max_days = 0 OR max_days >= min_days),
    supported INTEGER NOT NULL CHECK (supported IN (0, 1)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    updated_at TEXT NOT NULL,
    updated_by TEXT
);

INSERT INTO retention_policies
    (family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by)
SELECT family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by
FROM retention_policies_old;

DROP TABLE retention_policies_old;
