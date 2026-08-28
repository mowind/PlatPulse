-- Bounded samples for the one-minute Public Node Detail charts. Node process,
-- data-directory, and Peer values stay Node-scoped; Host network throughput is
-- stored once per Agent and referenced by each Node view.
--
-- Values come only from validated Observation Envelope last-good values. The
-- Agent observation time deduplicates retained last-good values while the first
-- Server receipt time anchors chart order without trusting Agent wall clocks.
CREATE TABLE node_metric_samples (
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    metric TEXT NOT NULL CHECK (metric IN (
        'process_cpu_percent',
        'process_memory_percent',
        'data_directory_percent',
        'peer_inbound_count',
        'peer_outbound_count'
    )),
    observed_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    value REAL NOT NULL CHECK (value >= 0),
    PRIMARY KEY (node_id, metric, observed_at)
);

CREATE INDEX node_metric_samples_recent_idx
ON node_metric_samples (node_id, received_at DESC, metric);

CREATE TABLE host_metric_samples (
    agent_id TEXT NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    metric TEXT NOT NULL CHECK (metric IN (
        'network_rx_bytes_per_sec',
        'network_tx_bytes_per_sec'
    )),
    observed_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    value REAL NOT NULL CHECK (value >= 0),
    PRIMARY KEY (agent_id, metric, observed_at)
);

CREATE INDEX host_metric_samples_recent_idx
ON host_metric_samples (agent_id, received_at DESC, metric);
