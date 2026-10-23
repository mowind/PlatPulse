-- Storage Backend Migration baseline (ADR 0013, issue #257).
--
-- PostgreSQL counterpart of migrations/0076_storage_migration.sql; both
-- backends share the logical schema version 76. See the SQLite migration for
-- the meaning of the singleton row: it records which SQLite file supplied the
-- data, which schema version both sides agreed on, and the content digest the
-- copy verified, so 'storage migrate abort' can refuse once the target owns
-- durable state the untouched source cannot reproduce.
CREATE TABLE storage_migration (
    singleton BIGINT PRIMARY KEY CHECK (singleton = 1),
    completed_at TEXT COLLATE "C" NOT NULL CHECK (length(completed_at) > 0),
    source_path TEXT COLLATE "C" NOT NULL CHECK (length(source_path) > 0),
    source_schema_version BIGINT NOT NULL CHECK (source_schema_version > 0),
    target_schema_version BIGINT NOT NULL CHECK (target_schema_version > 0),
    target_description TEXT COLLATE "C" NOT NULL CHECK (length(target_description) > 0),
    table_count BIGINT NOT NULL CHECK (table_count >= 0),
    row_count BIGINT NOT NULL CHECK (row_count >= 0),
    content_sha256 TEXT COLLATE "C" NOT NULL CHECK (length(content_sha256) = 64),
    -- The identity sequences as the copy left them: a PostgreSQL sequence
    -- advances outside the transaction that consumed it, so an insert that was
    -- rolled back still moves it. See the SQLite migration.
    sequence_sha256 TEXT COLLATE "C" NOT NULL CHECK (length(sequence_sha256) = 64)
);
