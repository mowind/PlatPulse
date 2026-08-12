-- Bounded attribution evidence for block history; no complete key material.
ALTER TABLE block_summaries ADD COLUMN node_key_fingerprint TEXT;
ALTER TABLE block_summaries ADD COLUMN node_key_valid_from TEXT;
ALTER TABLE block_summaries ADD COLUMN node_key_valid_until TEXT;
ALTER TABLE block_summaries ADD COLUMN node_key_history_complete INTEGER NOT NULL DEFAULT 0 CHECK (node_key_history_complete IN (0, 1));
ALTER TABLE block_summaries ADD COLUMN seal_recovery_rule TEXT;
ALTER TABLE block_summaries ADD COLUMN seal_evidence TEXT;
