-- Report Receipt body slimming (ADR 0009).
--
-- The identity row of agent_report_receipts is permanent: conflict detection,
-- close-report disposition, the latest-rejection Attention evidence and the
-- Purge admission barrier all read its columns. Only the large receipt_body is
-- slimmed after a fixed window, so whole-database backups stop scaling with
-- total receipt history.
--
-- receipt_slimmed_at marks a row whose body already carries no per-Node or
-- per-sample/range detail. NULL means the full body is still stored. The
-- partial index keeps one bounded slimming batch a range read instead of a
-- full-table scan (the issue #137 family of concerns).
ALTER TABLE agent_report_receipts ADD COLUMN receipt_slimmed_at TEXT;

CREATE INDEX agent_report_receipts_slimming_idx
    ON agent_report_receipts (received_at)
    WHERE receipt_slimmed_at IS NULL;

-- Extend the policy table's family constraint without changing any
-- operator-selected value (the established rebuild pattern).
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
        'validator_monthly_aggregate',
        'report_receipt_body'
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
    ('report_receipt_body', 30, 30, 30, 1, 1, '1970-01-01T00:00:00Z', 'defaults');
