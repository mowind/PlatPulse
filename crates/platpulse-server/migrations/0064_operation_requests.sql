-- Owner command identity for long-running Operations (issue #211, story 39).
--
-- The browser supplies an opaque requestId for each Owner intent. Recording it
-- durably is what makes a doubled click, a second tab, a repeated HTTP request,
-- or a retry after a lost response reconcile to the SAME Operation instead of
-- queueing the same cleanup twice. This is request-level dedup plus Audit
-- association, not exactly-once execution: intent_fingerprint pins the command
-- to the intent it was confirmed for (the retention preview it bound), so
-- reusing a requestId for a different intent is a conflict rather than a silent
-- second action. created_at/expires_at bound how long a recorded command stays
-- replayable, and expired rows are pruned lazily on the next accepted command.
CREATE TABLE operation_requests (
    request_id TEXT PRIMARY KEY CHECK (length(request_id) BETWEEN 1 AND 128),
    kind TEXT NOT NULL,
    intent_fingerprint TEXT NOT NULL CHECK (length(intent_fingerprint) BETWEEN 1 AND 200),
    operation_id TEXT NOT NULL REFERENCES operations(operation_id) ON DELETE CASCADE,
    audit_event_id INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

-- One recorded command per intent: a second tab that composes its own requestId
-- for the same confirmed preview still replays this row instead of queueing
-- another cleanup.
CREATE UNIQUE INDEX operation_requests_intent_idx
    ON operation_requests (kind, intent_fingerprint);

CREATE INDEX operation_requests_expires_idx ON operation_requests (expires_at);

-- Retention cleanup is the one Operation kind whose duplicate would release
-- production history twice from a single confirmation, so the ledger itself
-- refuses a second open run of that kind. A busy row makes the queue answer
-- "already open" instead of doubling the work, and the guard holds for a
-- concurrent writer that slipped past the caller's check.
CREATE UNIQUE INDEX operations_open_retention_run_idx
    ON operations (kind)
    WHERE kind = 'retention_run' AND status IN ('queued', 'running');
