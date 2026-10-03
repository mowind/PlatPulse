-- Server-side persistence for Retention impact previews (issue #210,
-- design §11.4, parent #202 User Stories 37/38/41/42).
--
-- A preview freezes the policy versions it was composed against, the family
-- scope the Owner selected, and ONE cutoff per family. A retention run may only
-- execute a preview the Server still accepts (same versions, same scope, still
-- unexpired): a stale confirmation is rejected instead of deleting against a
-- plan the operator never reviewed.
--
-- This table holds configuration intent only, never retained history and never
-- protected state:
--   * estimated_rows is the Server's own upper-bound count at composition time,
--     not a frozen row set — the bounded run stops when no expired row remains.
--   * entries_json carries each family's frozen cutoff and its per-table
--     estimate, so nothing is re-estimated while the run is in flight.
--   * scope_json carries the requested scope plus the requested families the
--     preview will NOT act on, so a run can warn about exactly those.
--   * Expired rows are pruned (bounded) while the next preview is composed, so
--     the table stays small without a scheduler.
--
-- No secret is stored: preview_id and policy_version are Server-computed
-- fingerprints and created_by is the Owner user id.

CREATE TABLE retention_previews (
    preview_id TEXT PRIMARY KEY CHECK (length(preview_id) BETWEEN 1 AND 64),
    created_at TEXT NOT NULL,
    created_by TEXT NOT NULL,
    policy_version TEXT NOT NULL CHECK (length(policy_version) BETWEEN 1 AND 64),
    scope_json TEXT NOT NULL,
    entries_json TEXT NOT NULL,
    estimated_rows INTEGER NOT NULL CHECK (estimated_rows >= 0),
    expires_at TEXT NOT NULL
);

-- Both pruning and the "latest live preview" lookup scan by time.
CREATE INDEX retention_previews_expires_at_idx
    ON retention_previews (expires_at);
