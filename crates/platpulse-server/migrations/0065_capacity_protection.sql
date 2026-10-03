-- Capacity visibility and low-space protection (issue #212, design §11.4,
-- parent #202 story 43-46).
--
-- History has two halves with different obligations:
--   * The core current projection, the Report receipt and the allocation that
--     backs it are the ingestion transaction. If they cannot commit, the
--     Server must not report success (see report_ingestion.rs).
--   * The generic optional metric history written by save_node_metric /
--     save_host_metric is an extra: it is bounded per series and nothing else
--     in the Server depends on one more sample.
--
-- When the Server database filesystem runs out of room, the second half is
-- expendable and the first half is not. This table pair records WHICH optional
-- writes were skipped and WHY, so a skipped stretch of history is a visible,
-- accountable gap instead of a silent one. Nothing here deletes or re-encodes
-- retained history: protection only stops new optional writes.
--
-- One row per protection interval (a low-space stretch). The partial unique
-- index enforces at most one open interval, so "is protection active" is a
-- fact about the database and survives a Server restart. ended_at/ended_reason
-- are the resume evidence; the opened_/resumed_ byte figures are the measured
-- values that justified entering and leaving protection.
CREATE TABLE capacity_protection_intervals (
    interval_id TEXT PRIMARY KEY CHECK (length(interval_id) BETWEEN 1 AND 64),
    source_mount TEXT NOT NULL CHECK (length(source_mount) BETWEEN 1 AND 4096),
    started_at TEXT NOT NULL,
    started_reason TEXT NOT NULL CHECK (started_reason IN ('low_space')),
    opened_total_bytes INTEGER NOT NULL CHECK (opened_total_bytes >= 0),
    opened_available_bytes INTEGER NOT NULL CHECK (opened_available_bytes >= 0),
    pause_below_bytes INTEGER NOT NULL CHECK (pause_below_bytes > 0),
    resume_above_bytes INTEGER NOT NULL CHECK (resume_above_bytes >= pause_below_bytes),
    ended_at TEXT,
    ended_reason TEXT CHECK (ended_reason IN ('resumed', 'protection_disabled')),
    resumed_total_bytes INTEGER CHECK (resumed_total_bytes >= 0),
    resumed_available_bytes INTEGER CHECK (resumed_available_bytes >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK ((ended_at IS NULL) = (ended_reason IS NULL)),
    CHECK ((ended_at IS NULL) = (resumed_total_bytes IS NULL)),
    CHECK ((ended_at IS NULL) = (resumed_available_bytes IS NULL)),
    CHECK (ended_at IS NULL OR ended_at >= started_at)
);

-- At most one open interval: the guard holds for a concurrent writer that
-- slipped past the sampling loop's own state check.
CREATE UNIQUE INDEX capacity_protection_open_interval_idx
    ON capacity_protection_intervals ((1))
    WHERE ended_at IS NULL;

CREATE INDEX capacity_protection_intervals_started_idx
    ON capacity_protection_intervals (started_at DESC);

-- One row per (interval, scope, series): the coarse series identity the Server
-- already uses, never the raw observation values. skipped_count is how many
-- optional samples of that series were NOT persisted while protection was on;
-- first_/last_skipped_at bound the observation window whose coverage was
-- given up. The Server can therefore answer "what history is missing, for
-- which node, over which window" without inventing a zero.
CREATE TABLE capacity_skipped_series (
    interval_id TEXT NOT NULL REFERENCES capacity_protection_intervals(interval_id) ON DELETE CASCADE,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('node', 'host')),
    scope_key TEXT NOT NULL CHECK (length(scope_key) BETWEEN 1 AND 128),
    metric TEXT NOT NULL CHECK (length(metric) BETWEEN 1 AND 128),
    skipped_count INTEGER NOT NULL CHECK (skipped_count > 0),
    first_skipped_at TEXT NOT NULL,
    last_skipped_at TEXT NOT NULL,
    PRIMARY KEY (interval_id, scope_kind, scope_key, metric)
);

CREATE INDEX capacity_skipped_series_interval_idx
    ON capacity_skipped_series (interval_id, skipped_count DESC);
