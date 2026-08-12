-- Agent-local durable state and pending history. This migration intentionally
-- contains no Server projections or future Phase 2/3 data.

CREATE TABLE agent_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    agent_id TEXT,
    agent_epoch INTEGER NOT NULL DEFAULT 0 CHECK (agent_epoch >= 0),
    boot_id TEXT,
    report_sequence INTEGER NOT NULL DEFAULT 0 CHECK (report_sequence >= 0),
    inventory_revision INTEGER NOT NULL DEFAULT 0 CHECK (inventory_revision >= 0),
    updated_at TEXT
);

CREATE TABLE pending_block_summaries (
    sample_id INTEGER PRIMARY KEY,
    node_id TEXT NOT NULL,
    block_number INTEGER NOT NULL CHECK (block_number >= 0),
    block_hash TEXT NOT NULL,
    parent_hash TEXT NOT NULL,
    block_timestamp_ms INTEGER NOT NULL CHECK (block_timestamp_ms >= 0),
    observed_at TEXT NOT NULL,
    transaction_count INTEGER NOT NULL CHECK (transaction_count >= 0),
    block_interval_ms INTEGER CHECK (block_interval_ms IS NULL OR block_interval_ms >= 0),
    source TEXT NOT NULL CHECK (source IN ('subscription', 'gap_backfill')),
    coinbase TEXT NOT NULL,
    seal_signer_key_fingerprint TEXT,
    seal_signer_match TEXT NOT NULL CHECK (
        seal_signer_match IN ('self', 'other', 'unknown')
    ),
    protocol_proposer_kind TEXT NOT NULL CHECK (
        protocol_proposer_kind IN ('verified', 'unknown')
    ),
    protocol_proposer_identity TEXT,
    attribution_reason TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CHECK (
        (protocol_proposer_kind = 'verified' AND protocol_proposer_identity IS NOT NULL)
        OR (protocol_proposer_kind = 'unknown' AND protocol_proposer_identity IS NULL)
    ),
    UNIQUE (node_id, block_number, block_hash)
);

CREATE TABLE history_gaps (
    gap_id INTEGER PRIMARY KEY,
    node_id TEXT NOT NULL,
    from_height INTEGER NOT NULL CHECK (from_height >= 0),
    to_height INTEGER NOT NULL CHECK (to_height >= from_height),
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (node_id, from_height, to_height, kind)
);

CREATE INDEX pending_block_summaries_oldest_idx
    ON pending_block_summaries (created_at, sample_id);

CREATE INDEX history_gaps_oldest_idx
    ON history_gaps (created_at, gap_id);
