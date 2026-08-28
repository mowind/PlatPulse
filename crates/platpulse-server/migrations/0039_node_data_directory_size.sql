-- Current last-good size of each explicitly configured PlatON data directory.

CREATE TABLE current_node_data_directory_observations (
    node_id TEXT PRIMARY KEY REFERENCES nodes(node_id),
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    updated_at TEXT NOT NULL
);
