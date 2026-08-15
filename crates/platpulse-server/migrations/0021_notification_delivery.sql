-- Phase 2 Notification delivery (issue #49, design §17.4, webui.md
-- PAGE-ADMIN-DELIVERIES / PAGE-ADMIN-CHANNELS).
--
-- Notification Events are durable business records of an Incident
-- transition (or an Owner test action). Notification Deliveries are the
-- per-channel/destination attempts of one Event: at-least-once (never
-- exactly-once), with bounded automatic retry, Retry-After awareness,
-- DeadLetter terminal state, and manual Owner retry that re-arms the
-- same Delivery -- it never creates a duplicate Event or business
-- transition. Provider tokens never enter the database; deliveries store
-- only a redacted destination summary and redacted provider results.

CREATE TABLE notification_events (
    event_id TEXT PRIMARY KEY,
    event_kind TEXT NOT NULL CHECK (event_kind IN ('incident', 'test')),
    incident_id TEXT REFERENCES alert_incidents(incident_id) ON DELETE SET NULL,
    rule_key TEXT,
    subject_kind TEXT CHECK (
        subject_kind IN ('agent', 'host', 'node', 'network', 'validator', 'server')
    ),
    subject_key TEXT,
    severity TEXT NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
    summary TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 500),
    created_at TEXT NOT NULL
);

CREATE INDEX notification_events_created_idx ON notification_events (created_at DESC);

CREATE TABLE notification_deliveries (
    delivery_id TEXT PRIMARY KEY,
    event_id TEXT NOT NULL REFERENCES notification_events(event_id) ON DELETE CASCADE,
    channel_kind TEXT NOT NULL CHECK (channel_kind IN ('telegram')),
    destination TEXT NOT NULL CHECK (length(destination) BETWEEN 1 AND 200),
    state TEXT NOT NULL CHECK (
        state IN ('pending', 'in_flight', 'retry_scheduled', 'succeeded', 'failed', 'dead_letter', 'suppressed')
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
    -- One Delivery per Event+channel: retries re-arm this row and never
    -- duplicate it (stable idempotency key, design §17.4).
    UNIQUE (event_id, channel_kind)
);

CREATE INDEX notification_deliveries_state_idx ON notification_deliveries (state, next_attempt_at);
CREATE INDEX notification_deliveries_created_idx ON notification_deliveries (created_at DESC);

CREATE TABLE delivery_attempts (
    attempt_id TEXT PRIMARY KEY,
    delivery_id TEXT NOT NULL REFERENCES notification_deliveries(delivery_id) ON DELETE CASCADE,
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

CREATE INDEX delivery_attempts_delivery_idx ON delivery_attempts (delivery_id, attempt_number);
