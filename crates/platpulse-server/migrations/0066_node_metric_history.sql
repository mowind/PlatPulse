-- Trustworthy 24-hour raw Node metric history (issue #213, design §11.4, parent
-- #202 stories 46, 47, 49, 57 and 59).
--
-- Two things stood between the bounded sample tables of migration 0041 and a
-- raw history an Operator can trust:
--
--   * a fixed per-series row cap (64 samples) that silently deleted the oldest
--     reading of a series. What the Server kept was therefore a function of the
--     report cadence rather than of a policy, and it could not be 24 hours
--     wide. Expiration is a retention-policy decision (§11.4), so the cap is
--     gone and raw metric samples gain a real retention family that carries the
--     24-hour raw floor.
--
--   * no record of what a series actually observed. node_metric_series_state
--     is that record: one row per (node, metric), and it stores no metric
--     value, so it is a series ledger and not a second copy of the history.
--
-- observation_count advances only when the Server stores an observation time it
-- did not already hold. A repeated delivery of the same
-- (node, metric, observed_at) is the same observation: it is counted as a
-- replay, never as coverage, so an accepted Report replay cannot inflate the
-- count, and a carried last-good value (which keeps its original observation
-- time) is not a new observation either. corrected_count counts one
-- observation time re-delivered with a different value: a correction to one
-- observation, still not a new one.
--
-- released_before is the oldest cutoff the cleanup has ever pruned this series
-- at. It exists because the policy cutoff alone cannot say whether the Server
-- once held an observation it has since released: retention accepts, stores and
-- then expires a reading, and widening the policy moves the window back over an
-- instant whose row is already gone. Judging novelty by the *current* cutoff
-- would then count that observation a second time, so the floor is the later of
-- the two and only ever moves forward (issue #213). It stores an instant, never
-- a metric value: releasing evidence cannot be undone, and the stamp is what
-- keeps a replayed last-good a replay after a widening and after a restart.
--
-- first_observed_at is the series' enablement boundary, the oldest observation
-- ever recorded for it. It deliberately outlives the samples themselves,
-- because "this series began here" stays true after the oldest raw sample
-- expires; it is never evidence that raw samples survive that far back, which
-- is why the range reader reports the retained window separately.
CREATE TABLE node_metric_series_state (
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    metric TEXT NOT NULL CHECK (length(metric) BETWEEN 1 AND 64),
    first_observed_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    last_received_at TEXT NOT NULL,
    observation_count INTEGER NOT NULL CHECK (observation_count >= 1),
    replayed_count INTEGER NOT NULL DEFAULT 0 CHECK (replayed_count >= 0),
    corrected_count INTEGER NOT NULL DEFAULT 0 CHECK (corrected_count >= 0),
    released_before TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z'
        CHECK (length(released_before) = 20),
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX node_metric_series_state_series_idx
    ON node_metric_series_state (node_id, metric);

-- Existing deployments keep the raw samples they already hold: the ledger is
-- derived from the stored observations, so a series that already had history
-- does not restart its count at zero. This preserves real evidence instead of
-- rebuilding lost history (#213); nothing here is inferred for a series the
-- Server never stored a sample for. released_before keeps its epoch default: no
-- cleanup under this policy regime has released evidence for those series yet.
INSERT INTO node_metric_series_state (
    node_id, metric, first_observed_at, last_observed_at, last_received_at,
    observation_count, replayed_count, corrected_count, updated_at
)
SELECT
    stored.node_id,
    stored.metric,
    MIN(stored.observed_at),
    MAX(stored.observed_at),
    (
        SELECT newest.received_at
        FROM node_metric_samples AS newest
        WHERE newest.node_id = stored.node_id AND newest.metric = stored.metric
        ORDER BY newest.observed_at DESC, newest.received_at DESC
        LIMIT 1
    ),
    COUNT(*),
    0,
    0,
    MAX(stored.received_at)
FROM node_metric_samples AS stored
GROUP BY stored.node_id, stored.metric;

-- Expiration is now a cutoff on observed_at, so both raw tables need the
-- ordered range index the bounded cleanup and the range read rely on. A
-- cleanup that runs after every accepted Report must never full-scan (issue
-- #137).
CREATE INDEX node_metric_samples_observed_at_idx
    ON node_metric_samples (observed_at, node_id, metric);

CREATE INDEX host_metric_samples_observed_at_idx
    ON host_metric_samples (observed_at, agent_id, metric);

-- The pause lookup beside the range read filters one series out of the whole
-- capacity ledger (scope_kind, scope_key, metric) and bounds it by
-- last_skipped_at, but the ledger's own primary key leads with interval_id, so
-- nothing served that predicate: every metric-history request would scan a
-- table that grows with every Node that has ever lost optional history. The
-- index leads with the series identity the lookup pins and then the ordered
-- bound the range read needs (issue #213).
CREATE INDEX capacity_skipped_series_lookup_idx
    ON capacity_skipped_series (scope_kind, scope_key, metric, last_skipped_at);

-- The new raw_metric_sample family must be a legal value of the policy table's
-- family constraint. With the old constraint in place the default seeding of
-- the family would be skipped silently (the seeding inserts with INSERT OR
-- IGNORE), and a family with no policy row is invisible to the Owner and is
-- skipped by its own cleanup path — raw metric samples would then grow without
-- bound, which is the very outcome that removing the fixed 64-sample cap was
-- meant to prevent. The established rebuild pattern extends the constraint
-- without changing any operator-selected value.
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
        'peer_aggregate_1h',
        'validator_daily_snapshot',
        'validator_monthly_aggregate',
        'report_receipt_body',
        'raw_metric_sample'
    )),
    retention_days INTEGER NOT NULL CHECK (retention_days >= 0),
    min_days INTEGER NOT NULL CHECK (min_days >= 0),
    max_days INTEGER NOT NULL CHECK (max_days = 0 OR max_days >= min_days),
    supported INTEGER NOT NULL CHECK (supported IN (0, 1)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    updated_at TEXT NOT NULL,
    updated_by TEXT
);

INSERT INTO retention_policies
    (family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by)
SELECT family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by
FROM retention_policies_old;

DROP TABLE retention_policies_old;

-- The 24-hour raw floor is the family's default and its minimum, matching the
-- catalog in crates/platpulse-server/src/retention.rs.
INSERT OR IGNORE INTO retention_policies
    (family, retention_days, min_days, max_days, supported, enabled, updated_at, updated_by)
VALUES
    ('raw_metric_sample', 1, 1, 30, 1, 1, '1970-01-01T00:00:00Z', 'defaults');
