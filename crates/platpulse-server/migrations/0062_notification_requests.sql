-- Server-side request ledger for Owner Notification commands (issue #206,
-- design §17.4/§17.5, parent #202 User Stories 31-33).
--
-- A Notification test or manual retry is one Server command with one
-- durable result. The browser supplies an opaque requestId for each Owner
-- intent; this table makes that identity durable so a repeated HTTP
-- request, a lost response, or a Server restart reconciles to the SAME
-- Server command result instead of triggering another external send.
--
-- This is the Server's request-level dedup and Audit association. It is
-- NOT a claim that the external provider delivered exactly once:
--   * The Delivery row and its Attempt history remain the authoritative
--     provider outcome (at-least-once, never exactly-once).
--   * intent_fingerprint pins the command to the resource it acted on, so
--     reusing a requestId for a different channel or Delivery is a
--     conflict, never a silent second action.
--   * command_kind='test' rows are also the durable cooldown clock
--     (notifications.test_cooldown_seconds); created_at/expires_at bound
--     how long a result stays queryable and are pruned lazily on the next
--     accepted command (notifications.dedup_retention_seconds).
--
-- No provider token, chat id, or other secret is stored: delivery_id and
-- event_id are opaque Server identifiers and audit_event_id is the Audit
-- row reference.

CREATE TABLE notification_requests (
    request_id TEXT PRIMARY KEY CHECK (length(request_id) BETWEEN 1 AND 128),
    command_kind TEXT NOT NULL CHECK (command_kind IN ('test', 'retry')),
    intent_fingerprint TEXT NOT NULL CHECK (length(intent_fingerprint) BETWEEN 1 AND 200),
    event_id TEXT REFERENCES notification_events(event_id) ON DELETE CASCADE,
    delivery_id TEXT NOT NULL REFERENCES notification_deliveries(delivery_id) ON DELETE CASCADE,
    audit_event_id INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

-- Cooldown clock and retention pruning both scan by kind/time.
CREATE INDEX notification_requests_command_created_idx
    ON notification_requests (command_kind, created_at);

CREATE INDEX notification_requests_expires_idx
    ON notification_requests (expires_at);
