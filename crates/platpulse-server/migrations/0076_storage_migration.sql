-- Storage Backend Migration baseline (ADR 0013, issue #257).
--
-- 'platpulse-server storage migrate' moves a stopped SQLite deployment to the
-- PostgreSQL Storage Backend in one target transaction. The transaction ends by
-- writing this single row into the target, so the target records the exact
-- baseline it holds: when the migration completed, which source file supplied
-- the data, which schema version both sides agreed on, and the content digest
-- the copy verified.
--
-- 'platpulse-server storage migrate abort' re-reads this row: it withdraws a
-- migration only while the target still matches the recorded digest, because
-- after the first Report, Admin mutation or background write the PostgreSQL
-- database owns new durable state that the untouched SQLite file cannot
-- reproduce. A target that no longer matches is refused and repaired forward.
--
-- The table is bookkeeping only: it is never copied from a source, and its
-- digest is never part of the migrated content digest.
CREATE TABLE storage_migration (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    completed_at TEXT NOT NULL CHECK (length(completed_at) > 0),
    source_path TEXT NOT NULL CHECK (length(source_path) > 0),
    source_schema_version INTEGER NOT NULL CHECK (source_schema_version > 0),
    target_schema_version INTEGER NOT NULL CHECK (target_schema_version > 0),
    target_description TEXT NOT NULL CHECK (length(target_description) > 0),
    table_count INTEGER NOT NULL CHECK (table_count >= 0),
    row_count INTEGER NOT NULL CHECK (row_count >= 0),
    content_sha256 TEXT NOT NULL CHECK (length(content_sha256) = 64),
    -- The identity sequences as the copy left them. A sequence advances
    -- outside the transaction that consumed it, so a write that rolled back or
    -- was reverted still moves it: this digest is the evidence the content
    -- digest cannot carry.
    sequence_sha256 TEXT NOT NULL CHECK (length(sequence_sha256) = 64)
);
