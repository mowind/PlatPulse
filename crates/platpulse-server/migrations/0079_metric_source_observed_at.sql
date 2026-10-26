-- Metric sample observation identity separated from corrected timeline (ADR 0014, issue #279).
--
-- A Report carries the Agent's raw observation instant along with a measured clock
-- skew. The skew correction jitters by milliseconds across reports and can cross
-- rounding boundaries (e.g. 501 ms vs 499 ms), moving the corrected instant into
-- a neighbouring second. If the observation instant serves as both identity and
-- timeline, a carried last-good sample crossing that boundary appears as a new
-- sample past the high-water mark, double-counting observation_count and rollup
-- buckets (#213).
--
-- Splitting the two keeps the Agent's raw instant in source_observed_at as the
-- stable identity (raw primary key, stored_value lookup, ledger high-water mark)
-- while observed_at records the skew-corrected Server-clock instant for range
-- queries, aggregate buckets and retention expiry.

ALTER TABLE node_metric_samples RENAME TO node_metric_samples_old;

CREATE TABLE node_metric_samples (
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    metric TEXT NOT NULL CHECK (metric IN (
        'process_cpu_percent',
        'process_memory_percent',
        'data_directory_percent',
        'peer_inbound_count',
        'peer_outbound_count',
        'sync_current_block',
        'sync_highest_block',
        'consensus_highest_qc_block',
        'consensus_highest_lock_block',
        'consensus_highest_commit_block'
    )),
    observed_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    value REAL NOT NULL CHECK (value >= 0),
    source_observed_at TEXT NOT NULL,
    PRIMARY KEY (node_id, metric, source_observed_at)
);

INSERT INTO node_metric_samples (node_id, metric, observed_at, received_at, value, source_observed_at)
SELECT node_id, metric, observed_at, received_at, value, observed_at
FROM node_metric_samples_old;

DROP TABLE node_metric_samples_old;

CREATE INDEX node_metric_samples_recent_idx
    ON node_metric_samples (node_id, received_at DESC, metric);

CREATE INDEX node_metric_samples_observed_at_idx
    ON node_metric_samples (observed_at, node_id, metric);

ALTER TABLE host_metric_samples RENAME TO host_metric_samples_old;

CREATE TABLE host_metric_samples (
    agent_id TEXT NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    metric TEXT NOT NULL CHECK (metric IN (
        'cpu_percent',
        'memory_used_bytes',
        'memory_total_bytes',
        'load1',
        'load5',
        'load15',
        'network_rx_bytes_per_sec',
        'network_tx_bytes_per_sec',
        'disk_used_bytes',
        'disk_total_bytes'
    )),
    dimension TEXT NOT NULL DEFAULT '' CHECK (length(dimension) <= 4096),
    observed_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    value REAL NOT NULL CHECK (value >= 0),
    source_observed_at TEXT NOT NULL,
    PRIMARY KEY (agent_id, metric, dimension, source_observed_at)
);

INSERT INTO host_metric_samples (agent_id, metric, dimension, observed_at, received_at, value, source_observed_at)
SELECT agent_id, metric, dimension, observed_at, received_at, value, observed_at
FROM host_metric_samples_old;

DROP TABLE host_metric_samples_old;

CREATE INDEX host_metric_samples_recent_idx
    ON host_metric_samples (agent_id, received_at DESC, metric);

CREATE INDEX host_metric_samples_observed_at_idx
    ON host_metric_samples (observed_at, agent_id, metric);
