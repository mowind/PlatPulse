-- Phase 3 per-Node Peer presence intervals (issue #55).
-- Historical rows retain only bounded Peer identity/diagnostic fields;
-- remote addresses and advertised capability lists are never stored here.
CREATE TABLE peer_presence_intervals (
    interval_id INTEGER PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    peer_id TEXT NOT NULL CHECK (length(peer_id) > 0 AND length(peer_id) <= 128),
    direction TEXT NOT NULL CHECK (direction IN ('inbound', 'outbound')),
    trusted INTEGER NOT NULL CHECK (trusted IN (0, 1)),
    static_peer INTEGER NOT NULL CHECK (static_peer IN (0, 1)),
    consensus_peer INTEGER NOT NULL CHECK (consensus_peer IN (0, 1)),
    client_name TEXT CHECK (client_name IS NULL OR length(client_name) <= 256),
    opened_at TEXT NOT NULL,
    closed_at TEXT,
    CHECK (closed_at IS NULL OR closed_at >= opened_at)
);

-- Re-arrivals create a new closed/open pair; only one open interval is valid.
CREATE UNIQUE INDEX peer_presence_open_unique
    ON peer_presence_intervals (node_id, peer_id)
    WHERE closed_at IS NULL;

CREATE INDEX peer_presence_node_idx
    ON peer_presence_intervals (node_id, closed_at);
CREATE INDEX peer_presence_retention_idx
    ON peer_presence_intervals (closed_at);
