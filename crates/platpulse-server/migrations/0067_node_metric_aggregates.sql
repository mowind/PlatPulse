-- Multi-precision Node metric history (issue #214, design §11.6 and §6, parent
-- #202 stories 47, 48, 54 and 58).
--
-- The raw tier is bounded by its own retention family (24 hours by default).
-- Beyond it the Server keeps *summaries* that age monotonically: a bucket holds
-- the extremes, the count and the newest value of the observations it actually
-- received, so zooming an old stretch can never recover a raw sample. It
-- returns the bucket that was written when the observation arrived, and the
-- bucket is never re-derived from a finer or coarser tier at read time.
--
-- Because nothing is derived at read time, nothing can be accumulated twice
-- either. A bucket advances only for an observation the Server accepted as new
-- (node_metric_series_state's delivery classification): a replayed Report, a
-- repeated delivery of an instant already counted, and a sample the low-space
-- protection refused to append all leave the tier exactly as it was.
--
-- grain_seconds is the bucket's own width in seconds - 60 for the stretch older
-- than the raw window and up to day 7, 300 for day 7 up to day 30 - and
-- bucket_start is the UTC-aligned start of the bucket. Both are stored rather
-- than implied, so the boundary is a row the Server wrote and not a rule the
-- reader has to re-invent, and a later tier needs no migration.
--
-- A bucket row exists only when at least one observation arrived in it. An
-- empty bucket is simply absent: it is never a zero, never a carried-forward
-- value, and never a straight line across a stretch the Server did not
-- observe. sample_count is therefore the number of received observations, and
-- min_value/max_value keep the spike that the raw samples would have shown.
--
-- first_observed_at/last_observed_at are real observation instants inside the
-- bucket; last_value/last_received_at belong to the newest of them, so a closed
-- bucket answers "what did this series last say" without a raw row, and the
-- gap/coverage rule is judged between real instants rather than between bucket
-- edges.
--
-- Two stored instants are not continuity evidence on their own: the same
-- count 4 with first 00:00, last 04:59 and the same extremes describes a bucket
-- observed every second and one with a 296-second hole in the middle. The
-- Server therefore also keeps max_gap_seconds, the largest gap between two
-- observations the bucket counted, so a reader can tell a bucket it proved
-- wholly observed from one that merely starts and ends somewhere. It is
-- maintained incrementally as observations arrive and can only grow, not
-- exceed the bucket's own width; a bucket holding a single observation has 0.
CREATE TABLE node_metric_aggregates (
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    metric TEXT NOT NULL CHECK (length(metric) BETWEEN 1 AND 64),
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
    PRIMARY KEY (node_id, metric, grain_seconds, bucket_start)
);

-- The read path answers one series and one tier over a range, newest first, so
-- the primary key is exactly its access path and needs no companion index.
--
-- Cleanup walks (grain_seconds, bucket_start) instead: one bounded batch
-- removes the oldest expired buckets of one tier across every Node, which is
-- what the declared cleanup targets of the one_minute_aggregate and
-- five_minute_aggregate families do.
CREATE INDEX node_metric_aggregates_expiry_idx
    ON node_metric_aggregates (grain_seconds, bucket_start);

-- The aggregate tiers are served for the two stretches the raw window cannot
-- reach, and the retention catalog declares those same windows as their safety
-- floor: one_minute_aggregate keeps the >24h..7d tier for 7 days and
-- five_minute_aggregate keeps the >7d..30d tier for 30 days. Both are registered
-- by POLICY_CATALOG rather than inserted here, because a policy row is the
-- Owner's setting and a migration must not overwrite one. The single exception
-- is the row #214 turned from a never-produced placeholder into a real tier:
-- see the guarded alignment at the end of this migration.
--
-- five_minute_aggregate is a family name the policy table's family constraint
-- does not know yet. With the old constraint in place the default seeding of
-- the family would be skipped silently (the seeding inserts with INSERT OR
-- IGNORE), and a family with no policy row is invisible to the Owner and is
-- skipped by its own cleanup path - the 5-minute tier would then grow without
-- bound, which is exactly what registering the 30-day floor is meant to
-- prevent. The established rebuild pattern extends the constraint without
-- changing any operator-selected value.
ALTER TABLE retention_policies RENAME TO retention_policies_old;

CREATE TABLE retention_policies (
    family TEXT PRIMARY KEY CHECK (family IN (
        'raw_block_summary',
        'one_minute_aggregate',
        'five_minute_aggregate',
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

-- Before #214 the one_minute_aggregate family was declared but never produced:
-- the catalog said supported = 0 with a 90-day default and a 7..365 bound, and
-- ensure_seeded wrote exactly that row on every database. Because the seeding
-- only inserts and the rebuild above only copies, an upgrade would keep a row
-- that contradicts the tier's own contract: the automatic cleanup would hold
-- 1-minute buckets for 90 days instead of 7, and the policy surface would call
-- a family the Server now really produces "not produced in the current phase".
--
-- Every one_minute_aggregate row is normalised, not only the untouched
-- placeholder. The 1-minute tier is served for exactly [now - 7d, now - 24h];
-- a stored 30-day window is a promise the Server cannot keep, because the read
-- path never serves 1m beyond day 7 and every bucket older than that is
-- unreachable data charged to the disk budget. An Owner's edit of the old
-- window is a choice between values that were all invented for a family that
-- did not exist yet, so it is superseded rather than honoured, and it stays
-- visible in audit_events. A database that never saw #213 has no row here at
-- all and is seeded with the new contract on first start.
UPDATE retention_policies
SET retention_days = 7, min_days = 7, max_days = 7, supported = 1
WHERE family = 'one_minute_aggregate'
  AND (retention_days <> 7 OR min_days <> 7 OR max_days <> 7 OR supported <> 1);
