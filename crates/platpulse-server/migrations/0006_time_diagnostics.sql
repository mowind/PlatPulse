-- Agent time exchange and server-derived liveness diagnostics.
ALTER TABLE agents ADD COLUMN clock_skew_ms INTEGER;
ALTER TABLE agents ADD COLUMN clock_status TEXT NOT NULL DEFAULT 'unknown' CHECK (clock_status IN ('known', 'clock_unreliable', 'unknown'));

CREATE INDEX agents_liveness_idx ON agents (last_received_at);
