-- Shared Host resource history (issue #215, design §11.4, §11.6 and §6,
-- parent #202 stories 46, 50, 51, 52 and 59).
--
-- The raw Host tier of migration 0041 held two network-rate series and nothing
-- else, because those two are what the one-minute Public Node Detail charts
-- needed. An investigation of why a Host is short of memory must not be
-- restricted to Node Process metrics, so the Host tier is extended to the
-- values the Agent already collects: CPU percent, physical memory, Load 1/5/15,
-- network rate, and per-mount storage usage and capacity.
--
-- Host evidence is shared: it is stored once per Agent and referenced by every
-- Node that Agent runs. Six Nodes on two Hosts therefore bill two Host series
-- sets, never six, and one Node's Purge cannot delete the Host history its
-- siblings still reference (design §11.4). A Node view says Host, not this
-- Node's Host: the distinction that keeps resource totals from multiplying is
-- carried by the storage itself, not by a convention in the read path.
--
-- Storage history is identified by Agent *and mount path*, never by a device
-- identity (story 52): a path that moves to another filesystem starts a new
-- series instead of claiming the old one, and the Server never guesses that two
-- paths are the same device. The mount path is therefore a real part of the
-- series key (dimension), not a metric-name suffix, so disk_used_bytes on
-- /data and on /var/lib are two series with their own ledger, their own
-- buckets and their own skip counts.
--
-- Values are the collected quantities (bytes, percent, load average, bytes per
-- second), never a percentage this Server baked out of two of them: a later
-- reading of memory_total_bytes or disk_total_bytes stays able to re-derive a
-- ratio against the total that was true at that instant.

-- Raw samples gain both the wider metric set and the dimension column. The
-- primary key grows with the dimension, because the observation instants the
-- Agent reports for two mounts in one Report must both survive: keyed on
-- (agent_id, metric, observed_at) alone the second mount would silently
-- overwrite the first.
--
-- The table is rebuilt rather than re-created because the rows it already holds
-- are real evidence: existing deployments keep every network-rate sample they
-- have, mapped onto the empty dimension (the value they were always keyed by),
-- and the ledger below is derived from those same rows so a series that already
-- had history does not restart its count at zero (issue #213's rule).
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
    PRIMARY KEY (agent_id, metric, dimension, observed_at)
);

INSERT INTO host_metric_samples (agent_id, metric, dimension, observed_at, received_at, value)
SELECT agent_id, metric, '', observed_at, received_at, value
FROM host_metric_samples_old;

DROP TABLE host_metric_samples_old;

CREATE INDEX host_metric_samples_recent_idx
ON host_metric_samples (agent_id, received_at DESC, metric);

CREATE INDEX host_metric_samples_observed_at_idx
ON host_metric_samples (observed_at, agent_id, metric);

-- The per-series ledger of the Host scope, the twin of
-- node_metric_series_state: one row per (Agent, metric, dimension) holding no
-- metric value, so it is a series ledger and not a second copy of the history.
-- It is what makes a Host series' first observation, its replay and correction
-- counts, and its released_before floor survive the expiration of the raw
-- samples themselves.
CREATE TABLE host_metric_series_state (
    agent_id TEXT NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    metric TEXT NOT NULL CHECK (length(metric) BETWEEN 1 AND 64),
    dimension TEXT NOT NULL DEFAULT '' CHECK (length(dimension) <= 4096),
    first_observed_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    last_received_at TEXT NOT NULL,
    observation_count INTEGER NOT NULL CHECK (observation_count >= 1),
    replayed_count INTEGER NOT NULL DEFAULT 0 CHECK (replayed_count >= 0),
    corrected_count INTEGER NOT NULL DEFAULT 0 CHECK (corrected_count >= 0),
    released_before TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z'
        CHECK (length(released_before) = 20),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (agent_id, metric, dimension)
);

CREATE UNIQUE INDEX host_metric_series_state_series_idx
    ON host_metric_series_state (agent_id, metric, dimension);

INSERT INTO host_metric_series_state (
    agent_id, metric, dimension, first_observed_at, last_observed_at, last_received_at,
    observation_count, replayed_count, corrected_count, updated_at
)
SELECT
    stored.agent_id,
    stored.metric,
    stored.dimension,
    MIN(stored.observed_at),
    MAX(stored.observed_at),
    (
        SELECT newest.received_at
        FROM host_metric_samples AS newest
        WHERE newest.agent_id = stored.agent_id
          AND newest.metric = stored.metric
          AND newest.dimension = stored.dimension
        ORDER BY newest.observed_at DESC, newest.received_at DESC
        LIMIT 1
    ),
    COUNT(*),
    0,
    0,
    MAX(stored.received_at)
FROM host_metric_samples AS stored
GROUP BY stored.agent_id, stored.metric, stored.dimension;

-- Host aggregate buckets are the twin of node_metric_aggregates and obey the
-- same contract in full (migration 0067): the bucket is written when the
-- observation arrives and never re-derived at read time, an empty bucket is
-- absent rather than zero, sample_count counts received observations only, and
-- max_gap_seconds records the widest interval the bucket actually proved.
-- grain_seconds stays the bucket's own width (60 for >24h..7d, 300 for >7d..30d)
-- and is stored rather than implied, so the boundaries are rows the Server
-- wrote. Both tiers are guarded by the existing retention families; Host
-- history adds no new tier and no new family.
CREATE TABLE host_metric_aggregates (
    agent_id TEXT NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    metric TEXT NOT NULL CHECK (length(metric) BETWEEN 1 AND 64),
    dimension TEXT NOT NULL DEFAULT '' CHECK (length(dimension) <= 4096),
    grain_seconds INTEGER NOT NULL CHECK (grain_seconds IN (60, 300)),
    bucket_start TEXT NOT NULL CHECK (length(bucket_start) = 20),
    sample_count INTEGER NOT NULL CHECK (sample_count >= 1),
    min_value REAL NOT NULL,
    max_value REAL NOT NULL CHECK (max_value >= min_value),
    last_value REAL NOT NULL,
    first_observed_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    last_received_at TEXT NOT NULL,
    max_gap_seconds INTEGER NOT NULL DEFAULT 0
        CHECK (max_gap_seconds >= 0 AND max_gap_seconds <= grain_seconds),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (agent_id, metric, dimension, grain_seconds, bucket_start)
);

CREATE INDEX host_metric_aggregates_expiry_idx
    ON host_metric_aggregates (grain_seconds, bucket_start);

-- The capacity ledger must not merge the skips of two mounts into one count:
-- a paused Report that refused a /data sample and a /var/lib sample has to keep
-- saying which path lost which observation, or the protection pause would
-- report a loss it cannot attribute and the investigation would see one
-- unexplained number. The dimension joins the ledger's key for exactly the same
-- reason it joined the raw key.
ALTER TABLE capacity_skipped_series RENAME TO capacity_skipped_series_old;

CREATE TABLE capacity_skipped_series (
    interval_id TEXT NOT NULL REFERENCES capacity_protection_intervals(interval_id) ON DELETE CASCADE,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('node', 'host')),
    scope_key TEXT NOT NULL CHECK (length(scope_key) BETWEEN 1 AND 128),
    metric TEXT NOT NULL CHECK (length(metric) BETWEEN 1 AND 128),
    dimension TEXT NOT NULL DEFAULT '' CHECK (length(dimension) <= 4096),
    skipped_count INTEGER NOT NULL CHECK (skipped_count > 0),
    first_skipped_at TEXT NOT NULL,
    last_skipped_at TEXT NOT NULL,
    PRIMARY KEY (interval_id, scope_kind, scope_key, metric, dimension)
);

INSERT INTO capacity_skipped_series
    (interval_id, scope_kind, scope_key, metric, dimension, skipped_count, first_skipped_at, last_skipped_at)
SELECT interval_id, scope_kind, scope_key, metric, '', skipped_count, first_skipped_at, last_skipped_at
FROM capacity_skipped_series_old;

DROP TABLE capacity_skipped_series_old;

CREATE INDEX capacity_skipped_series_interval_idx
    ON capacity_skipped_series (interval_id, skipped_count DESC);

-- The pause lookup beside the range read pins one series out of the whole
-- ledger, so the series identity leads and the ordered bound the range read
-- needs comes last; the dimension joins that identity (issue #213).
CREATE INDEX capacity_skipped_series_lookup_idx
    ON capacity_skipped_series (scope_kind, scope_key, metric, dimension, last_skipped_at);
