-- Minimal permanent deletion identity for an Owner-explicit Node Purge
-- (main design §15.3, ADR 0004). A Purge removes that Node's observation,
-- monitoring history, and Node Validator Links inside one transaction; this
-- row is the retained identity that keeps the disposition explainable and is
-- the anchor for the durable admission boundary (issue #170) without exposing
-- any Node-owned observation. Shared Agent/Host/Network/Validator data,
-- existing Alert Incident evidence, and Audit are deliberately not touched.
CREATE TABLE deleted_nodes (
    node_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    network_key TEXT NOT NULL,
    display_name TEXT,
    deleted_by_user_id TEXT REFERENCES users(user_id),
    deleted_at TEXT NOT NULL
);

CREATE INDEX deleted_nodes_deleted_at_idx ON deleted_nodes (deleted_at DESC, node_id);
