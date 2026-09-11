-- Raw Block Summary retention deletes expired rows in accepted_at order and
-- runs after every accepted AgentReport. Without a leading accepted_at index
-- SQLite satisfies the ordered LIMIT with a full table scan plus a temporary
-- B-tree, which cost ~124 ms per report on a 260 MB block_summaries table even
-- when nothing was expired (issue #137). The covering index turns the batch
-- into a bounded range read of at most RAW_BLOCK_SUMMARY_CLEANUP_BATCH rows,
-- so catch-up throughput is no longer bounded by the retention scan.
--
-- node_id and block_number follow accepted_at so the ordered LIMIT is served
-- by the index alone and no temporary B-tree is built.
CREATE INDEX block_summaries_accepted_at_idx
ON block_summaries (accepted_at, node_id, block_number);
