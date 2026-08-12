ALTER TABLE block_history_state ADD COLUMN current_head INTEGER;
ALTER TABLE block_history_state ADD COLUMN resync_state TEXT NOT NULL DEFAULT 'normal' CHECK (resync_state IN ('normal', 'resyncing', 'stalled'));
ALTER TABLE block_history_state ADD COLUMN resync_started_at TEXT;
ALTER TABLE block_history_state ADD COLUMN resync_last_progress_at TEXT;
ALTER TABLE block_history_state ADD COLUMN resync_target_height INTEGER;

-- Network reference is an observed, confidence-labelled projection, never a canonical head.
CREATE TABLE network_reference_heads (
    network_key TEXT PRIMARY KEY REFERENCES networks(network_key),
    block_number INTEGER CHECK (block_number IS NULL OR block_number >= 0),
    observed_at TEXT NOT NULL,
    confidence TEXT NOT NULL CHECK (confidence IN ('high', 'medium', 'low', 'unknown')),
    eligible_source_count INTEGER NOT NULL DEFAULT 0 CHECK (eligible_source_count >= 0),
    contributing_node_id TEXT
);
CREATE INDEX network_reference_heads_confidence_idx ON network_reference_heads (confidence, observed_at);
