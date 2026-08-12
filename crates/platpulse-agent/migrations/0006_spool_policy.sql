-- Durable spool capacity policy and loss accounting.
CREATE TABLE spool_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    max_bytes INTEGER NOT NULL,
    max_age_seconds INTEGER NOT NULL,
    preflush_bytes INTEGER NOT NULL,
    dropped_reports INTEGER NOT NULL DEFAULT 0,
    dropped_samples INTEGER NOT NULL DEFAULT 0,
    dropped_sequence_from INTEGER,
    dropped_sequence_to INTEGER,
    dropped_time_from TEXT,
    dropped_time_to TEXT,
    dropped_height_from INTEGER,
    dropped_height_to INTEGER,
    pending_history_gaps INTEGER NOT NULL DEFAULT 0,
    report_too_large INTEGER NOT NULL DEFAULT 0 CHECK (report_too_large IN (0, 1)),
    store_fatal INTEGER NOT NULL DEFAULT 0 CHECK (store_fatal IN (0, 1)),
    store_error TEXT,
    updated_at TEXT NOT NULL
);
INSERT INTO spool_state (singleton, max_bytes, max_age_seconds, preflush_bytes, updated_at)
VALUES (1, 2097152, 86400, 1572864, '1970-01-01T00:00:00Z');
CREATE INDEX reports_in_flight_idx ON reports (in_flight, created_at, report_id);
