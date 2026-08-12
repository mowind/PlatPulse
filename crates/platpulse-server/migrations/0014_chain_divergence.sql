-- Bounded, independently retained block identity evidence and idempotent divergence keys.
ALTER TABLE block_identity_window ADD COLUMN observed_at TEXT;
ALTER TABLE chain_divergence_observations ADD COLUMN retained_observed_at TEXT;
CREATE UNIQUE INDEX chain_divergence_identity_idx
    ON chain_divergence_observations (node_id, height, retained_block_hash, observed_block_hash);
