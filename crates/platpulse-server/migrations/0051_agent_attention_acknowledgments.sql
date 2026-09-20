-- Agent Attention Acknowledgment (main design §15.6, webui.md §15.3,
-- issue #172).
--
-- An Owner's durable, shared confirmation is stored against the Server-owned
-- occurrence/evidence boundary of one Agent Attention Item, not against the
-- stable kind + subject alone. The evidence boundary is the Server-derived
-- key that distinguishes \"the same occurrence\" from new evidence or a
-- later recurrence: an acknowledgment suppresses only the boundary the Owner
-- actually saw, so ordinary refreshes, unchanged reports, a second Owner,
-- another session, or a Server restart never resurrect a confirmed
-- occurrence, while new evidence re-prompts.
--
-- The table carries no raw diagnostic payload; acknowledged_by_user_id and
-- acknowledged_at are the accountable facts also recorded as an Audit Event.

CREATE TABLE agent_attention_acknowledgments (
    agent_id TEXT NOT NULL REFERENCES agents(agent_id),
    kind TEXT NOT NULL,
    evidence_key TEXT NOT NULL,
    acknowledged_at TEXT NOT NULL,
    acknowledged_by_user_id TEXT REFERENCES users(user_id),
    PRIMARY KEY (agent_id, kind, evidence_key)
);

CREATE INDEX agent_attention_acknowledgments_agent_idx
    ON agent_attention_acknowledgments (agent_id, kind);
