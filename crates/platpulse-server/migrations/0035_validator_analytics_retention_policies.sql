-- Add explicit long-term retention families for Validator daily snapshots and
-- monthly aggregates. These are derived reporting state and are kept forever;
-- deleting them could let a delayed retry re-open a partially retained month
-- or lose the durable source needed to rebuild calendar-month aggregates.
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
        'peer_presence_interval',
        'peer_aggregate_5m',
        'peer_aggregate_1h',
        'validator_daily_snapshot',
        'validator_monthly_aggregate'
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

INSERT OR IGNORE INTO retention_policies
    (family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by)
VALUES
    ('validator_daily_snapshot', 0, 0, 0, 1, 1, '1970-01-01T00:00:00Z', 'defaults'),
    ('validator_monthly_aggregate', 0, 0, 0, 1, 1, '1970-01-01T00:00:00Z', 'defaults');