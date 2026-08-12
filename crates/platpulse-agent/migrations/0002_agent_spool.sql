-- Immutable AgentReport spool and terminal receipt/rejection records.
-- Reports are kept until their receipt has been applied locally.

CREATE TABLE reports (
    report_id TEXT PRIMARY KEY,
    agent_epoch INTEGER NOT NULL CHECK (agent_epoch >= 0),
    boot_id TEXT NOT NULL,
    report_sequence INTEGER NOT NULL CHECK (report_sequence > 0),
    generated_at TEXT NOT NULL,
    body BLOB NOT NULL,
    body_sha256 TEXT NOT NULL,
    body_bytes INTEGER NOT NULL CHECK (body_bytes >= 0),
    in_flight INTEGER NOT NULL DEFAULT 0 CHECK (in_flight IN (0, 1)),
    created_at TEXT NOT NULL,
    UNIQUE (boot_id, report_sequence)
);

CREATE TABLE report_receipts (
    report_id TEXT PRIMARY KEY,
    report_body_sha256 TEXT NOT NULL,
    disposition TEXT NOT NULL CHECK (
        disposition IN ('accepted', 'partially_accepted', 'rejected')
    ),
    receipt_body BLOB NOT NULL,
    applied_at TEXT NOT NULL
);

CREATE TABLE rejection_ledger (
    rejection_id INTEGER PRIMARY KEY,
    report_id TEXT,
    node_id TEXT,
    sample_kind TEXT,
    from_height INTEGER CHECK (from_height IS NULL OR from_height >= 0),
    to_height INTEGER CHECK (to_height IS NULL OR to_height >= from_height),
    rejection_code TEXT NOT NULL,
    reason TEXT NOT NULL,
    rejected_at TEXT NOT NULL
);

CREATE INDEX reports_oldest_idx
    ON reports (created_at, report_id);

CREATE INDEX rejection_ledger_oldest_idx
    ON rejection_ledger (rejected_at, rejection_id);
