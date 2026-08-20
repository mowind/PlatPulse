-- A History Gap is identified by its Node, inclusive range, and kind. A
-- repeated declaration in a later immutable report must not multiply history.
DELETE FROM block_history_gaps
 WHERE gap_id NOT IN (
     SELECT MIN(gap_id)
       FROM block_history_gaps
      GROUP BY node_id, from_height, to_height, kind
 );

CREATE UNIQUE INDEX block_history_gaps_identity_idx
    ON block_history_gaps (node_id, from_height, to_height, kind);
