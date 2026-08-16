-- Phase 3 current per-Node Peer projection. Raw remote_ip is retained only
-- for the current set and is never selected by Public/Admin DTO queries.
CREATE TABLE current_node_peers (
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    peer_id TEXT NOT NULL CHECK (length(peer_id) > 0 AND length(peer_id) <= 128),
    remote_ip TEXT CHECK (remote_ip IS NULL OR length(remote_ip) <= 45),
    direction TEXT NOT NULL CHECK (direction IN ('inbound', 'outbound')),
    trusted INTEGER NOT NULL CHECK (trusted IN (0, 1)),
    static_peer INTEGER NOT NULL CHECK (static_peer IN (0, 1)),
    consensus_peer INTEGER NOT NULL CHECK (consensus_peer IN (0, 1)),
    client_name TEXT CHECK (client_name IS NULL OR length(client_name) <= 256),
    cbft_protocol_version INTEGER CHECK (cbft_protocol_version IS NULL OR (cbft_protocol_version >= 0 AND cbft_protocol_version <= 1024)),
    cbft_highest_qc_block INTEGER CHECK (cbft_highest_qc_block IS NULL OR cbft_highest_qc_block >= 0),
    cbft_locked_block INTEGER CHECK (cbft_locked_block IS NULL OR cbft_locked_block >= 0),
    cbft_commit_block INTEGER CHECK (cbft_commit_block IS NULL OR cbft_commit_block >= 0),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (node_id, peer_id)
);

CREATE TABLE current_node_peer_capabilities (
    node_id TEXT NOT NULL,
    peer_id TEXT NOT NULL,
    capability TEXT NOT NULL CHECK (length(capability) > 0 AND length(capability) <= 128),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (node_id, peer_id, capability),
    FOREIGN KEY (node_id, peer_id)
        REFERENCES current_node_peers(node_id, peer_id)
        ON DELETE CASCADE
);

CREATE INDEX current_node_peers_node_id_idx ON current_node_peers(node_id);
