-- #175: deleting a subject (Node Purge / Agent Removal) is not recovery.
--
-- A purged Node or a removed Agent leaves current Attention and every later
-- alert evaluation, but its Incident evidence is retained and annotated as
-- subject-deleted instead of being rewritten to a false recovery, and its
-- not-yet-sent notifications are cancelled -- including resolution Events
-- that carry no incident_id. Already-sent or already-handed-off facts stay
-- exactly as they are, and other subjects (including a shared Validator) are
-- untouched. See main design §15.7 and ADR 0004.

-- 1. Incidents keep their immutable facts; the annotation is additive so an
--    open Incident is never turned into a claimed known recovery.
ALTER TABLE alert_incidents ADD COLUMN subject_deleted_at TEXT;

CREATE INDEX alert_incidents_subject_live_idx
    ON alert_incidents (state, subject_deleted_at);

-- 2. A distinct terminal Delivery state for cancellation. SQLite cannot
--    widen a CHECK constraint in place, so the Delivery table and its
--    attempt child are rebuilt; every stored row and attempt fact is copied
--    verbatim. The child is recreated against the rebuilt parent first so the
--    implicit DELETE of the DROP cannot cascade away attempt history.
CREATE TABLE notification_deliveries_0054 (
    delivery_id TEXT PRIMARY KEY,
    event_id TEXT NOT NULL REFERENCES notification_events(event_id) ON DELETE CASCADE,
    channel_kind TEXT NOT NULL CHECK (channel_kind IN ('telegram')),
    destination TEXT NOT NULL CHECK (length(destination) BETWEEN 1 AND 200),
    state TEXT NOT NULL CHECK (
        state IN ('pending', 'in_flight', 'retry_scheduled', 'succeeded', 'failed', 'dead_letter', 'suppressed', 'cancelled')
    ),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at TEXT,
    last_attempt_at TEXT,
    last_result TEXT CHECK (last_result IS NULL OR length(last_result) BETWEEN 1 AND 300),
    last_error_kind TEXT CHECK (
        last_error_kind IS NULL
        OR last_error_kind IN ('telegram_api', 'network', 'timeout', 'config', 'internal')
    ),
    retry_after_seconds INTEGER CHECK (retry_after_seconds IS NULL OR retry_after_seconds >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (event_id, channel_kind)
);

INSERT INTO notification_deliveries_0054 (
    delivery_id, event_id, channel_kind, destination, state, attempt_count,
    next_attempt_at, last_attempt_at, last_result, last_error_kind,
    retry_after_seconds, created_at, updated_at
) SELECT
    delivery_id, event_id, channel_kind, destination, state, attempt_count,
    next_attempt_at, last_attempt_at, last_result, last_error_kind,
    retry_after_seconds, created_at, updated_at
FROM notification_deliveries;

CREATE TABLE delivery_attempts_0054 (
    attempt_id TEXT PRIMARY KEY,
    delivery_id TEXT NOT NULL REFERENCES notification_deliveries_0054(delivery_id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL CHECK (attempt_number >= 1),
    attempted_at TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('succeeded', 'failed')),
    provider_result TEXT NOT NULL CHECK (length(provider_result) BETWEEN 1 AND 300),
    error_kind TEXT CHECK (
        error_kind IS NULL
        OR error_kind IN ('telegram_api', 'network', 'timeout', 'config', 'internal')
    ),
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    retry_after_seconds INTEGER CHECK (retry_after_seconds IS NULL OR retry_after_seconds >= 0),
    UNIQUE (delivery_id, attempt_number)
);

INSERT INTO delivery_attempts_0054 (
    attempt_id, delivery_id, attempt_number, attempted_at, outcome,
    provider_result, error_kind, duration_ms, retry_after_seconds
) SELECT
    attempt_id, delivery_id, attempt_number, attempted_at, outcome,
    provider_result, error_kind, duration_ms, retry_after_seconds
FROM delivery_attempts;

DROP TABLE delivery_attempts;
DROP TABLE notification_deliveries;
ALTER TABLE notification_deliveries_0054 RENAME TO notification_deliveries;
ALTER TABLE delivery_attempts_0054 RENAME TO delivery_attempts;

CREATE INDEX notification_deliveries_state_idx
    ON notification_deliveries (state, next_attempt_at);
CREATE INDEX notification_deliveries_created_idx
    ON notification_deliveries (created_at DESC);
CREATE INDEX delivery_attempts_delivery_idx
    ON delivery_attempts (delivery_id, attempt_number);
