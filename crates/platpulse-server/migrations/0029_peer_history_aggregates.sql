-- Phase 3 bounded Peer history aggregates (issue #57).
-- Parent rows contain only approved operational summaries. Country distribution
-- is normalized into country/count child rows; no raw Peer address or provider
-- response can enter either aggregate family.
CREATE TABLE peer_aggregate_5m (
    aggregate_id INTEGER PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    bucket_start TEXT NOT NULL,
    sample_count INTEGER NOT NULL CHECK (sample_count > 0),
    total_peers INTEGER NOT NULL CHECK (total_peers >= 0),
    inbound_count INTEGER NOT NULL CHECK (inbound_count >= 0),
    outbound_count INTEGER NOT NULL CHECK (outbound_count >= 0),
    trusted_count INTEGER NOT NULL CHECK (trusted_count >= 0),
    static_count INTEGER NOT NULL CHECK (static_count >= 0),
    consensus_count INTEGER NOT NULL CHECK (consensus_count >= 0),
    known_country_count INTEGER NOT NULL CHECK (known_country_count >= 0),
    unknown_country_count INTEGER NOT NULL CHECK (unknown_country_count >= 0),
    arrivals INTEGER NOT NULL CHECK (arrivals >= 0),
    departures INTEGER NOT NULL CHECK (departures >= 0),
    cbft_lag_count INTEGER NOT NULL CHECK (cbft_lag_count >= 0),
    cbft_lag_sum INTEGER NOT NULL CHECK (cbft_lag_sum >= 0),
    cbft_lag_min INTEGER CHECK (cbft_lag_min IS NULL OR cbft_lag_min >= 0),
    cbft_lag_max INTEGER CHECK (cbft_lag_max IS NULL OR cbft_lag_max >= 0),
    first_observed_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    UNIQUE (node_id, bucket_start),
    CHECK (outbound_count + inbound_count <= total_peers),
    CHECK (known_country_count + unknown_country_count = total_peers),
    CHECK (cbft_lag_count = 0 OR (cbft_lag_min IS NOT NULL AND cbft_lag_max IS NOT NULL))
);

CREATE INDEX peer_aggregate_5m_node_idx
ON peer_aggregate_5m (node_id, bucket_start DESC);
CREATE INDEX peer_aggregate_5m_retention_idx
ON peer_aggregate_5m (bucket_start);

CREATE TABLE peer_aggregate_5m_countries (
    node_id TEXT NOT NULL,
    bucket_start TEXT NOT NULL,
    country_code TEXT NOT NULL CHECK (length(country_code) = 2 AND country_code GLOB '[A-Z][A-Z]'),
    peer_count INTEGER NOT NULL CHECK (peer_count > 0),
    PRIMARY KEY (node_id, bucket_start, country_code),
    FOREIGN KEY (node_id, bucket_start)
        REFERENCES peer_aggregate_5m (node_id, bucket_start)
        ON DELETE CASCADE
);

CREATE TABLE peer_aggregate_1h (
    aggregate_id INTEGER PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    bucket_start TEXT NOT NULL,
    sample_count INTEGER NOT NULL CHECK (sample_count > 0),
    total_peers INTEGER NOT NULL CHECK (total_peers >= 0),
    inbound_count INTEGER NOT NULL CHECK (inbound_count >= 0),
    outbound_count INTEGER NOT NULL CHECK (outbound_count >= 0),
    trusted_count INTEGER NOT NULL CHECK (trusted_count >= 0),
    static_count INTEGER NOT NULL CHECK (static_count >= 0),
    consensus_count INTEGER NOT NULL CHECK (consensus_count >= 0),
    known_country_count INTEGER NOT NULL CHECK (known_country_count >= 0),
    unknown_country_count INTEGER NOT NULL CHECK (unknown_country_count >= 0),
    arrivals INTEGER NOT NULL CHECK (arrivals >= 0),
    departures INTEGER NOT NULL CHECK (departures >= 0),
    cbft_lag_count INTEGER NOT NULL CHECK (cbft_lag_count >= 0),
    cbft_lag_sum INTEGER NOT NULL CHECK (cbft_lag_sum >= 0),
    cbft_lag_min INTEGER CHECK (cbft_lag_min IS NULL OR cbft_lag_min >= 0),
    cbft_lag_max INTEGER CHECK (cbft_lag_max IS NULL OR cbft_lag_max >= 0),
    first_observed_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    UNIQUE (node_id, bucket_start),
    CHECK (outbound_count + inbound_count <= total_peers),
    CHECK (known_country_count + unknown_country_count = total_peers),
    CHECK (cbft_lag_count = 0 OR (cbft_lag_min IS NOT NULL AND cbft_lag_max IS NOT NULL))
);

CREATE INDEX peer_aggregate_1h_node_idx
ON peer_aggregate_1h (node_id, bucket_start DESC);
CREATE INDEX peer_aggregate_1h_retention_idx
ON peer_aggregate_1h (bucket_start);

CREATE TABLE peer_aggregate_1h_countries (
    node_id TEXT NOT NULL,
    bucket_start TEXT NOT NULL,
    country_code TEXT NOT NULL CHECK (length(country_code) = 2 AND country_code GLOB '[A-Z][A-Z]'),
    peer_count INTEGER NOT NULL CHECK (peer_count > 0),
    PRIMARY KEY (node_id, bucket_start, country_code),
    FOREIGN KEY (node_id, bucket_start)
        REFERENCES peer_aggregate_1h (node_id, bucket_start)
        ON DELETE CASCADE
);

-- Extend the existing policy table without changing operator-selected values.
ALTER TABLE retention_policies RENAME TO retention_policies_old;
CREATE TABLE retention_policies (
    family TEXT PRIMARY KEY CHECK (family IN (
        'raw_block_summary',
        'one_minute_aggregate',
        'one_hour_aggregate',
        'history_gap',
        'divergence_observation',
        'audit_event',
        'alert_notification',
        'peer_presence_interval',
        'peer_aggregate_5m',
        'peer_aggregate_1h'
    )),
    retention_days INTEGER NOT NULL CHECK (retention_days >= 0),
    min_days INTEGER NOT NULL CHECK (min_days >= 0),
    max_days INTEGER NOT NULL CHECK (max_days = 0 OR max_days >= min_days),
    supported INTEGER NOT NULL CHECK (supported IN (0, 1)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    updated_at TEXT NOT NULL,
    updated_by TEXT
);
INSERT INTO retention_policies (family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by)
SELECT family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by
FROM retention_policies_old;
DROP TABLE retention_policies_old;

INSERT OR IGNORE INTO retention_policies (family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by)
VALUES
    ('peer_aggregate_5m', 90, 7, 365, 1, 1, '1970-01-01T00:00:00Z', 'defaults'),
    ('peer_aggregate_1h', 0, 0, 0, 1, 1, '1970-01-01T00:00:00Z', 'defaults');
