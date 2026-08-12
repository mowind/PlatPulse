-- Durable Agent-local Block Summary queue. Samples are persisted before report creation.
CREATE TABLE block_summaries (
    sample_id INTEGER PRIMARY KEY,
    node_id TEXT NOT NULL,
    block_number INTEGER NOT NULL CHECK (block_number >= 0),
    block_hash TEXT NOT NULL,
    parent_hash TEXT NOT NULL,
    network_genesis_hash TEXT NOT NULL,
    network_chain_id INTEGER NOT NULL,
    network_p2p_network_id INTEGER NOT NULL,
    network_address_hrp TEXT,
    block_timestamp_ms INTEGER NOT NULL,
    observed_at TEXT NOT NULL,
    transaction_count INTEGER NOT NULL CHECK (transaction_count >= 0),
    block_interval_ms INTEGER,
    source TEXT NOT NULL CHECK (source IN ('subscription', 'gap_backfill')),
    coinbase TEXT NOT NULL,
    seal_signer_key_fingerprint TEXT,
    seal_signer_match TEXT NOT NULL,
    protocol_proposer_kind TEXT NOT NULL,
    protocol_proposer_identity TEXT,
    attribution_reason TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (node_id, block_number, block_hash)
);
CREATE INDEX block_summaries_oldest_idx ON block_summaries (created_at, sample_id);
