-- Restore Operations (issue #51, design §20.2, webui.md §8.4): the
-- highest-risk recovery workflow runs through the durable Operations
-- machinery, so the `operations.kind` vocabulary gains `restore`.
--
-- SQLite cannot alter a CHECK constraint, so the table is rebuilt with the
-- extended kind vocabulary; every column and index is preserved byte for
-- byte. Restore never restores secret files: it replaces only the database
-- file, so no secret-bearing table or column exists anywhere in this
-- migration.

CREATE TABLE operations_0023 (
    operation_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN (
        'retention_run', 'backup_create', 'backup_verify', 'doctor_run',
        'restore'
    )),
    status TEXT NOT NULL CHECK (status IN (
        'queued', 'running', 'succeeded',
        'succeeded_with_warnings', 'failed', 'cancelled'
    )),
    progress_percent INTEGER NOT NULL DEFAULT 0 CHECK (progress_percent BETWEEN 0 AND 100),
    progress_label TEXT,
    request_id TEXT,
    -- Sanitized JSON: parameters never carry secrets or paths.
    params_json TEXT NOT NULL DEFAULT '{}',
    warnings_json TEXT NOT NULL DEFAULT '[]',
    errors_json TEXT NOT NULL DEFAULT '[]',
    result_json TEXT,
    created_by_user_id TEXT REFERENCES users(user_id),
    created_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    audit_event_id INTEGER,
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0, 1))
);

INSERT INTO operations_0023 (
    operation_id, kind, status, progress_percent, progress_label, request_id,
    params_json, warnings_json, errors_json, result_json, created_by_user_id,
    created_at, started_at, finished_at, audit_event_id, cancel_requested
)
SELECT
    operation_id, kind, status, progress_percent, progress_label, request_id,
    params_json, warnings_json, errors_json, result_json, created_by_user_id,
    created_at, started_at, finished_at, audit_event_id, cancel_requested
FROM operations;

DROP TABLE operations;
ALTER TABLE operations_0023 RENAME TO operations;

CREATE INDEX operations_created_idx ON operations (created_at DESC);
CREATE INDEX operations_status_idx ON operations (status, created_at);
CREATE INDEX operations_audit_idx ON operations (audit_event_id);
