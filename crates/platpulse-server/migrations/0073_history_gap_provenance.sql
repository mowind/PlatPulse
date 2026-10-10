-- History Gap rows have two authors: the Server records the gaps for the
-- samples it refuses itself, and an Agent declares the gaps it discovered while
-- collecting. Only the Server's own rows may be presented as the Server's live
-- refusal diagnosis (ADR 0011 decision 4), so the row must carry its author:
-- the Server is the trust boundary and never presents an Agent claim as its
-- own observation (AGENTS.md). Rows written before this migration are
-- attributed to 'agent' so no unverified historical claim becomes a Server
-- diagnosis, and a Server report rewrites the row it authored with its own
-- text whenever it refuses the same (node, from_height, to_height, kind).
ALTER TABLE block_history_gaps ADD COLUMN authored_by TEXT NOT NULL DEFAULT 'agent'
    CHECK (authored_by IN ('server', 'agent'));

-- Recovery evidence for the same diagnosis. The Admin surface clears the
-- refusal diagnosis once the Server accepts a Block Summary again, but raw
-- Block Summaries are aged out by retention, so the evidence cannot be the
-- newest retained summary alone: record the moment the Server last accepted a
-- summary on the durable per-Node state row that retention keeps.
ALTER TABLE block_history_state ADD COLUMN last_accepted_summary_at TEXT;
