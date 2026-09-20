-- Owner-explicit Agent Removal (main design §15.2, ADR 0004, issue #171).
-- Removing an Agent is not credential revocation: it revokes every credential,
-- permanently purges every Node the Agent authoritatively owns through the
-- Node Purge path, and records a durable removal marker on the Agent
-- registration identity. The Agent row and its foreign-keyed evidence
-- (immutable Report receipts, Host/diagnostic projections, Transfer history,
-- Audit) are deliberately retained and marked rather than cascaded away, so
-- the existing Incident evidence and audit accountability survive without a
-- lossy rewrite. deleted_at is the authoritative removal boundary: list and
-- detail reads hide a removed Agent, and every credential recovery path
-- (Recovery/Rotation/Revocation, report admission) must refuse it.

ALTER TABLE agents ADD COLUMN deleted_at TEXT;
ALTER TABLE agents ADD COLUMN deleted_by_user_id TEXT REFERENCES users(user_id);

CREATE INDEX agents_live_idx ON agents (deleted_at, agent_id);
