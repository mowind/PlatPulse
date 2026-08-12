-- Phase 1 report receipts, current projections, and bounded block history.
-- Peer/Geo/Validator/Alert/Notification/Transfer/aggregate tables are
-- deliberately not part of the Phase 0/1 schema.

CREATE TABLE agent_report_receipts (
    report_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(agent_id),
    agent_epoch INTEGER NOT NULL CHECK (agent_epoch >= 0),
    boot_id TEXT NOT NULL,
    report_sequence INTEGER NOT NULL CHECK (report_sequence > 0),
    report_body_sha256 TEXT NOT NULL,
    disposition TEXT NOT NULL CHECK (
        disposition IN ('accepted', 'partially_accepted', 'rejected')
    ),
    receipt_body BLOB NOT NULL,
    received_at TEXT NOT NULL,
    UNIQUE (agent_id, agent_epoch, boot_id, report_sequence)
);

CREATE TABLE component_status (
    agent_id TEXT NOT NULL REFERENCES agents(agent_id),
    scope TEXT NOT NULL CHECK (scope IN ('host', 'node')),
    scope_key TEXT NOT NULL,
    node_id TEXT,
    component_key TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN ('starting', 'ok', 'error', 'disabled', 'unsupported')
    ),
    attempted_at TEXT,
    observed_at TEXT,
    received_at TEXT,
    state_revision INTEGER NOT NULL CHECK (state_revision >= 0),
    value_revision INTEGER NOT NULL CHECK (value_revision >= 0),
    error_code TEXT,
    error_message TEXT,
    PRIMARY KEY (agent_id, scope, scope_key, component_key),
    FOREIGN KEY (agent_id, node_id) REFERENCES nodes(agent_id, node_id),
    CHECK (
        (scope = 'host' AND scope_key = 'host' AND node_id IS NULL)
        OR (scope = 'node' AND node_id IS NOT NULL AND scope_key = node_id)
    )
);

CREATE TABLE current_host_observations (
    agent_id TEXT PRIMARY KEY REFERENCES agents(agent_id),
    cpu_percent REAL,
    memory_total_bytes INTEGER,
    memory_used_bytes INTEGER,
    load1 REAL,
    load5 REAL,
    load15 REAL,
    network_rx_bytes_per_sec INTEGER,
    network_tx_bytes_per_sec INTEGER,
    clock_skew_ms INTEGER,
    spool_queued_bytes INTEGER,
    spool_queued_reports INTEGER,
    spool_oldest_queued_age_ms INTEGER,
    spool_dropped_reports INTEGER,
    spool_dropped_samples INTEGER,
    updated_at TEXT NOT NULL
);

CREATE TABLE current_host_disk_mounts (
    agent_id TEXT NOT NULL REFERENCES agents(agent_id),
    mount_path TEXT NOT NULL,
    total_bytes INTEGER NOT NULL CHECK (total_bytes >= 0),
    used_bytes INTEGER NOT NULL CHECK (used_bytes >= 0 AND used_bytes <= total_bytes),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (agent_id, mount_path)
);

CREATE TABLE current_node_process_observations (
    node_id TEXT PRIMARY KEY REFERENCES nodes(node_id),
    pid INTEGER,
    started_at TEXT,
    cpu_percent REAL,
    memory_bytes INTEGER,
    uptime_ms INTEGER,
    updated_at TEXT NOT NULL
);

CREATE TABLE current_node_chain_observations (
    node_id TEXT PRIMARY KEY REFERENCES nodes(node_id),
    rpc_client_version TEXT,
    syncing INTEGER CHECK (syncing IS NULL OR syncing IN (0, 1)),
    current_block INTEGER,
    highest_block INTEGER,
    pulled_states INTEGER,
    known_states INTEGER,
    consensus_epoch INTEGER,
    consensus_view_number INTEGER,
    consensus_validator INTEGER CHECK (
        consensus_validator IS NULL OR consensus_validator IN (0, 1)
    ),
    consensus_highest_qc_block INTEGER,
    consensus_highest_lock_block INTEGER,
    consensus_highest_commit_block INTEGER,
    network_genesis_hash TEXT,
    network_chain_id INTEGER,
    network_p2p_network_id INTEGER,
    network_address_hrp TEXT,
    node_key_fingerprint TEXT,
    enode TEXT,
    updated_at TEXT NOT NULL
);

CREATE TABLE current_node_rpc_namespaces (
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    namespace TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (node_id, namespace)
);

CREATE TABLE current_node_rpc_methods (
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    method TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (node_id, method)
);

CREATE TABLE block_summaries (
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    block_number INTEGER NOT NULL CHECK (block_number >= 0),
    block_hash TEXT NOT NULL,
    parent_hash TEXT NOT NULL,
    network_genesis_hash TEXT NOT NULL,
    network_chain_id INTEGER NOT NULL CHECK (network_chain_id >= 0),
    network_p2p_network_id INTEGER NOT NULL CHECK (network_p2p_network_id >= 0),
    network_address_hrp TEXT NOT NULL,
    block_timestamp_ms INTEGER NOT NULL CHECK (block_timestamp_ms >= 0),
    observed_at TEXT NOT NULL,
    transaction_count INTEGER NOT NULL CHECK (transaction_count >= 0),
    block_interval_ms INTEGER CHECK (block_interval_ms IS NULL OR block_interval_ms >= 0),
    source TEXT NOT NULL CHECK (source IN ('subscription', 'gap_backfill')),
    coinbase TEXT NOT NULL,
    seal_signer_key_fingerprint TEXT,
    seal_signer_match TEXT NOT NULL CHECK (
        seal_signer_match IN ('self', 'other', 'unknown')
    ),
    protocol_proposer_kind TEXT NOT NULL CHECK (
        protocol_proposer_kind IN ('verified', 'unknown')
    ),
    protocol_proposer_identity TEXT,
    attribution_reason TEXT NOT NULL,
    accepted_at TEXT NOT NULL,
    PRIMARY KEY (node_id, block_number),
    CHECK (
        (protocol_proposer_kind = 'verified' AND protocol_proposer_identity IS NOT NULL)
        OR (protocol_proposer_kind = 'unknown' AND protocol_proposer_identity IS NULL)
    )
);

CREATE TABLE block_history_state (
    node_id TEXT PRIMARY KEY REFERENCES nodes(node_id),
    historical_high_watermark INTEGER NOT NULL DEFAULT 0 CHECK (historical_high_watermark >= 0),
    cumulative_block_count INTEGER NOT NULL DEFAULT 0 CHECK (cumulative_block_count >= 0),
    cumulative_transaction_count INTEGER NOT NULL DEFAULT 0 CHECK (cumulative_transaction_count >= 0),
    cumulative_self_seal_count INTEGER NOT NULL DEFAULT 0 CHECK (cumulative_self_seal_count >= 0),
    updated_at TEXT NOT NULL
);

CREATE TABLE block_coverage_intervals (
    coverage_id INTEGER PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    first_height INTEGER NOT NULL CHECK (first_height >= 0),
    last_height INTEGER NOT NULL CHECK (last_height >= first_height),
    status TEXT NOT NULL CHECK (
        status IN ('covered', 'open_recoverable_gap', 'permanent_gap')
    ),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE block_identity_window (
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    height INTEGER NOT NULL CHECK (height >= 0),
    block_hash TEXT NOT NULL,
    retained_until TEXT,
    PRIMARY KEY (node_id, height)
);

CREATE TABLE block_history_gaps (
    gap_id INTEGER PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    from_height INTEGER NOT NULL CHECK (from_height >= 0),
    to_height INTEGER NOT NULL CHECK (to_height >= from_height),
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved_at TEXT
);

CREATE TABLE report_sequence_gaps (
    gap_id INTEGER PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(agent_id),
    boot_id TEXT NOT NULL,
    from_sequence INTEGER NOT NULL CHECK (from_sequence > 0),
    to_sequence INTEGER NOT NULL CHECK (to_sequence >= from_sequence),
    created_at TEXT NOT NULL
);

CREATE TABLE chain_divergence_observations (
    divergence_id INTEGER PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    height INTEGER NOT NULL CHECK (height >= 0),
    retained_block_hash TEXT NOT NULL,
    observed_block_hash TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    reason TEXT NOT NULL
);

CREATE INDEX agent_report_receipts_agent_idx
    ON agent_report_receipts (agent_id, received_at, report_sequence);
CREATE INDEX component_status_node_idx
    ON component_status (node_id, component_key);
CREATE INDEX block_summaries_recent_idx
    ON block_summaries (node_id, block_number DESC);
CREATE INDEX block_coverage_node_idx
    ON block_coverage_intervals (node_id, first_height, last_height);
CREATE INDEX block_gaps_node_idx
    ON block_history_gaps (node_id, from_height, to_height);
CREATE INDEX divergence_node_idx
    ON chain_divergence_observations (node_id, height, divergence_id);
