-- Bounded History Gap settlement (issue #282).
--
-- PostgreSQL counterpart of migrations/0080_bounded_gap_settlement.sql; both
-- backends share the logical schema version 80. Partial indexes over the
-- unresolved unrecoverable_backfill gaps and the open recoverable intervals
-- keep settlement proportional to the rows still in play.

CREATE INDEX block_gaps_unsettled_idx
    ON block_history_gaps (node_id, to_height, from_height)
    WHERE resolved_at IS NULL AND kind = 'unrecoverable_backfill';

CREATE INDEX block_gaps_unsettled_id_idx
    ON block_history_gaps (gap_id)
    WHERE resolved_at IS NULL AND kind = 'unrecoverable_backfill';

CREATE INDEX block_coverage_open_idx
    ON block_coverage_intervals (node_id, last_height, first_height)
    WHERE status = 'open_recoverable_gap';
