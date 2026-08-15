-- Phase 2 recoverable Operations (issue #50, design §11.3/§20.1/§20.3,
-- webui.md §5.5/§8.4): retention policies, long-running Operations, and
-- backup artifact metadata.
--
-- Retention policies are per-family and Owner-configurable within fixed
-- safety bounds (design §11.3). Execution is batched and never lowers the
-- historical high-water mark, deletes coverage/gap/divergence state or
-- cumulative counters, and never touches immutable Incident history.
--
-- Operations are durable queue rows: every mutation returns immediately
-- with an Operation reference, the worker advances it in bounded steps,
-- and state/history stay recoverable through REST after navigation,
-- browser close, or SSE loss. Operation history is separate from Audit
-- history but each row links to its creating Audit Event.
--
-- Backup artifacts store sanitized metadata only: no database contents,
-- no paths beyond the file base name, and never secrets.

CREATE TABLE retention_policies (
    family TEXT PRIMARY KEY CHECK (family IN (
        'raw_block_summary',
        'one_minute_aggregate',
        'one_hour_aggregate',
        'history_gap',
        'divergence_observation',
        'audit_event',
        'alert_notification'
    )),
    -- 0 means "keep forever" (long-term families).
    retention_days INTEGER NOT NULL CHECK (retention_days >= 0),
    min_days INTEGER NOT NULL CHECK (min_days >= 0),
    -- 0 means "no upper bound" (safety upper limit is unbounded).
    max_days INTEGER NOT NULL CHECK (max_days = 0 OR max_days >= min_days),
    -- Aggregates do not exist in this phase; their policies render
    -- Unsupported and are never executed.
    supported INTEGER NOT NULL CHECK (supported IN (0, 1)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    updated_at TEXT NOT NULL,
    updated_by TEXT
);

CREATE TABLE operations (
    operation_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN (
        'retention_run', 'backup_create', 'backup_verify', 'doctor_run'
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

CREATE INDEX operations_created_idx ON operations (created_at DESC);
CREATE INDEX operations_status_idx ON operations (status, created_at);
CREATE INDEX operations_audit_idx ON operations (audit_event_id);

CREATE TABLE backup_artifacts (
    artifact_id TEXT PRIMARY KEY,
    -- Sanitized base name only; never an absolute path.
    filename TEXT NOT NULL CHECK (length(filename) BETWEEN 1 AND 120),
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    sha256 TEXT NOT NULL CHECK (length(sha256) = 64),
    schema_version INTEGER NOT NULL,
    server_version TEXT NOT NULL,
    created_at TEXT NOT NULL,
    data_range_min TEXT,
    data_range_max TEXT,
    verification TEXT NOT NULL DEFAULT 'pending'
        CHECK (verification IN ('pending', 'ok', 'failed')),
    verified_at TEXT,
    verification_error TEXT
        CHECK (verification_error IS NULL OR length(verification_error) BETWEEN 1 AND 300),
    create_operation_id TEXT,
    verify_operation_id TEXT
);

CREATE INDEX backup_artifacts_created_idx ON backup_artifacts (created_at DESC);
