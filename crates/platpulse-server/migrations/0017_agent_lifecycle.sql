-- Agent lifecycle operations (design §4.5, §12.5, §12.6): Recovery Tokens
-- and credential rotation overlap. Recovery Tokens are short-lived, single
-- use, and bound to one existing Agent; exchanging one advances the Agent
-- Epoch and issues a fresh credential without creating a duplicate Agent.
-- The Server stores only the pepper-keyed HMAC digest.

CREATE TABLE recovery_tokens (
    token_id TEXT PRIMARY KEY,
    token_digest BLOB NOT NULL UNIQUE,
    agent_id TEXT NOT NULL REFERENCES agents(agent_id),
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    revoked_at TEXT
);

CREATE INDEX recovery_tokens_expiry_idx
    ON recovery_tokens (expires_at, consumed_at, revoked_at);
CREATE INDEX recovery_tokens_agent_idx
    ON recovery_tokens (agent_id, consumed_at, revoked_at);

-- Credential rotation overlap: a rotated-out credential stays valid until
-- `revoke_after` even without an explicit revoke. The Server enforces this
-- lazily at authentication time, so no background worker is required.
ALTER TABLE agent_credentials ADD COLUMN revoke_after TEXT;
