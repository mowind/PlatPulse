-- Bounded History Gap settlement (issue #282).
--
-- Settling an unrecoverable_backfill History Gap asks two questions: which
-- unresolved gaps of a Node overlap the heights a Report or a retention sweep
-- just closed, and whether an open recoverable interval still overlaps a gap.
-- Both are answered by partial indexes over only the rows still in play, so
-- the cost follows the unresolved gaps and open intervals near the touched
-- heights instead of the size of either table. The third index serves the
-- retention sweep's keyset walk over the unresolved gaps.

CREATE INDEX block_gaps_unsettled_idx
    ON block_history_gaps (node_id, to_height, from_height)
    WHERE resolved_at IS NULL AND kind = 'unrecoverable_backfill';

CREATE INDEX block_gaps_unsettled_id_idx
    ON block_history_gaps (gap_id)
    WHERE resolved_at IS NULL AND kind = 'unrecoverable_backfill';

CREATE INDEX block_coverage_open_idx
    ON block_coverage_intervals (node_id, last_height, first_height)
    WHERE status = 'open_recoverable_gap';
