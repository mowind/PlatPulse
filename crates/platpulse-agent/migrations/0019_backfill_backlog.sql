-- Issue #281: declared gap ranges a later bounded step may still backfill.
-- A recovery that exceeds its bounds declares the rest of its range as an
-- unrecoverable_backfill History Gap and queues it here, so the hole is
-- backfilled step by step instead of staying a hole forever.
CREATE TABLE backfill_backlog (
    node_id TEXT NOT NULL,
    from_height INTEGER NOT NULL CHECK (from_height >= 0),
    to_height INTEGER NOT NULL CHECK (to_height >= from_height),
    stalls INTEGER NOT NULL DEFAULT 0 CHECK (stalls >= 0),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (node_id, from_height)
);
