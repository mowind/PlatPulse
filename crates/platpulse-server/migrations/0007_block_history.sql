-- Observed head is deliberately separate from canonical/public chain head.
CREATE TABLE observed_network_heads (
    node_id TEXT PRIMARY KEY REFERENCES nodes(node_id),
    block_number INTEGER CHECK (block_number IS NULL OR block_number >= 0),
    block_hash TEXT,
    observed_at TEXT NOT NULL,
    confidence TEXT NOT NULL CHECK (confidence IN ('high', 'medium', 'low', 'unknown')),
    eligible_sources TEXT NOT NULL
);
