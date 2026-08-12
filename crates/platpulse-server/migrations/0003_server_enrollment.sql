-- Agent Enrollment state (design §4.5, §12.5, §12.6): short-lived
-- single-use Enrollment Tokens. The Server stores only the pepper-keyed
-- HMAC digest of each token; the plaintext lives with the operator until
-- the enrolling Agent consumes it.

CREATE TABLE enrollment_tokens (
    token_id TEXT PRIMARY KEY,
    token_digest BLOB NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    consumed_agent_id TEXT REFERENCES agents(agent_id),
    revoked_at TEXT
);

CREATE INDEX enrollment_tokens_expiry_idx
    ON enrollment_tokens (expires_at, consumed_at, revoked_at);
