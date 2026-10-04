-- Recorded Node relationship intervals (issue #221, stories 60 and 61).
--
-- A Node's reporting Agent and its Network are mutable: a completed Node
-- Transfer moves nodes.agent_id, and a later Report may declare another
-- network_key. Reading those current columns for a past window would apply
-- today's relation to a stretch the Server never observed it in. This table is
-- the Server's own record of the relations it accepted, from the instant it
-- accepted them, and every historical answer is built from these intervals.
--
-- The table is deliberately created empty. A relation the Server did not record
-- cannot be reconstructed from the current columns without backfilling the
-- past, so a window before the first recorded interval stays unknown rather
-- than being read as "no relation" or as the relation the Node has now. The
-- Server starts recording here and only moves forward.
CREATE TABLE node_relationship_intervals (
    interval_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    -- 'agent' is the Node's reporting Agent. A Host relation is not stored
    -- separately: Host observations are collected once per Agent and Host
    -- alert subjects are keyed by the Agent id, so the recorded Agent interval
    -- is the Host relation's evidence too.
    relation_kind TEXT NOT NULL CHECK (relation_kind IN ('agent', 'network')),
    related_key TEXT NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('enrollment', 'transfer', 'network_change')),
    valid_from TEXT NOT NULL,
    valid_until TEXT,
    recorded_at TEXT NOT NULL,
    CHECK (valid_until IS NULL OR valid_until > valid_from)
);

CREATE INDEX node_relationship_intervals_node_idx
    ON node_relationship_intervals (node_id, relation_kind, valid_from, valid_until);
CREATE INDEX node_relationship_intervals_related_idx
    ON node_relationship_intervals (relation_kind, related_key, valid_from, valid_until);

-- One open interval per Node and kind, and no overlap inside a kind: a change
-- closes the previous interval before the next one opens, so a window can never
-- be attributed to two relations of the same kind at once.
CREATE TRIGGER node_relationship_intervals_no_overlap_insert
BEFORE INSERT ON node_relationship_intervals
WHEN EXISTS (
    SELECT 1 FROM node_relationship_intervals existing
    WHERE existing.node_id = NEW.node_id
      AND existing.relation_kind = NEW.relation_kind
      AND NEW.valid_from < COALESCE(existing.valid_until, '9999-12-31T23:59:59Z')
      AND COALESCE(NEW.valid_until, '9999-12-31T23:59:59Z') > existing.valid_from
)
BEGIN
    SELECT RAISE(ABORT, 'node_relationship_interval_overlap');
END;

CREATE TRIGGER node_relationship_intervals_no_overlap_update
BEFORE UPDATE OF node_id, relation_kind, valid_from, valid_until ON node_relationship_intervals
WHEN EXISTS (
    SELECT 1 FROM node_relationship_intervals existing
    WHERE existing.interval_id != NEW.interval_id
      AND existing.node_id = NEW.node_id
      AND existing.relation_kind = NEW.relation_kind
      AND NEW.valid_from < COALESCE(existing.valid_until, '9999-12-31T23:59:59Z')
      AND COALESCE(NEW.valid_until, '9999-12-31T23:59:59Z') > existing.valid_from
)
BEGIN
    SELECT RAISE(ABORT, 'node_relationship_interval_overlap');
END;
