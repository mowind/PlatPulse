-- Block-series aggregate tiers (issue #248, ADR 0012, docs/design/webui.md
-- §15.38).
--
-- Block interval and Transactions / block are derived from block_summaries,
-- which holds roughly 600,000 rows per Node over seven days and has no tier of
-- its own. Bucketing those rows per anonymous request would derive a grain at
-- read time and let a Public request choose its own cost, so the Server keeps a
-- write-time tier instead, exactly as node_metric_aggregates does for the
-- sample series (migration 0067): one row per Node, tier and UTC-aligned
-- bucket, advanced inside the receipt transaction that accepted the block.
--
-- A bucket advances only for a block the Server accepted as new: the same
-- statement that inserts the Block Summary decides it (INSERT OR IGNORE plus the
-- Block history dedup checks in front of it), so a plain resync replay at or
-- below the historical high-water mark, a repeated delivery of a height already
-- held and a Replay of a whole Report leave every bucket exactly as it was. Only
-- an explicit OpenRecoverableGap backfill may add a block below the high-water
-- mark, and it is counted once, like any other accepted block.
--
-- A block lands in the bucket of its own observed_at - the instant the Agent
-- observed it, which is the instant the Live chart plots it at - so the
-- tiers and the Live answer place a block on the same axis.
--
-- block_count counts accepted blocks; transaction_total / transaction_min /
-- transaction_max / last_transaction_count summarise their transaction counts.
-- The interval of a block is its block timestamp minus its parent's, accepted
-- only when the parent is held and strictly older (the Live rule). A block
-- whose parent arrives later is given its interval when the parent arrives, in
-- the bucket the block itself was counted in, and only while that bucket still
-- exists: the interval is counted once, by whichever of the two blocks arrived
-- second. interval_count is therefore at most block_count, and a bucket whose
-- blocks have no known parent holds no interval at all (all four interval
-- columns NULL), never a zero.
--
-- first_observed_at / last_observed_at / last_received_at / max_gap_seconds are
-- the same continuity evidence node_metric_aggregates keeps: real observation
-- instants inside the bucket and the widest silence between two blocks it
-- counted, so the Public window judges a block bucket by the same long-history
-- rule (MIN_GAP_SECONDS plus max_gap_seconds) as every other series.
--
-- An empty bucket is absent: no row is written for a stretch in which no block
-- was accepted. Nothing is backfilled from block_summaries rows accepted before
-- this migration, so a stretch older than the tier is answered as unobserved.
CREATE TABLE node_block_aggregates (
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    grain_seconds INTEGER NOT NULL CHECK (grain_seconds IN (60, 300)),
    bucket_start TEXT NOT NULL CHECK (length(bucket_start) = 20),
    block_count INTEGER NOT NULL CHECK (block_count >= 1),
    transaction_total INTEGER NOT NULL CHECK (transaction_total >= 0),
    transaction_min INTEGER NOT NULL CHECK (transaction_min >= 0),
    transaction_max INTEGER NOT NULL CHECK (transaction_max >= transaction_min),
    last_transaction_count INTEGER NOT NULL CHECK (last_transaction_count >= 0),
    interval_count INTEGER NOT NULL DEFAULT 0 CHECK (interval_count >= 0 AND interval_count <= block_count),
    interval_min_ms INTEGER CHECK (interval_min_ms IS NULL OR interval_min_ms > 0),
    interval_max_ms INTEGER,
    last_interval_ms INTEGER,
    last_interval_observed_at TEXT,
    first_observed_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    last_received_at TEXT NOT NULL,
    max_gap_seconds INTEGER NOT NULL DEFAULT 0
        CHECK (max_gap_seconds >= 0 AND max_gap_seconds <= grain_seconds),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (node_id, grain_seconds, bucket_start),
    CHECK (
        (interval_count = 0 AND interval_min_ms IS NULL AND interval_max_ms IS NULL
            AND last_interval_ms IS NULL AND last_interval_observed_at IS NULL)
        OR (interval_count > 0 AND interval_min_ms IS NOT NULL
            AND interval_max_ms >= interval_min_ms AND last_interval_ms IS NOT NULL
            AND last_interval_observed_at IS NOT NULL)
    )
);

-- The Public window reads one Node and one tier over a range, newest first,
-- which is exactly the primary key. Cleanup walks (grain_seconds, bucket_start)
-- across every Node, as the one_minute_aggregate and five_minute_aggregate
-- families do for the sample tiers; this table is a target of both families
-- (POLICY_CATALOG), so its windows are the same 7 and 30 days and cannot drift.
CREATE INDEX node_block_aggregates_expiry_idx
    ON node_block_aggregates (grain_seconds, bucket_start);

-- The 1-hour preset is answered from the held Block Summaries themselves, by
-- the instant each block was observed - the axis every series is drawn on. A
-- walk by height would miss a block backfilled at a low height inside the hour
-- and could not tell the window's oldest blocks apart from blocks a limit cut
-- off, so the read is a bounded range of this index instead.
CREATE INDEX block_summaries_observed_idx
    ON block_summaries (node_id, observed_at);
