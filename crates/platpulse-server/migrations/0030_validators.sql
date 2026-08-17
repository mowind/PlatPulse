-- Server-managed Network-scoped Validator identities and explicit Node links.
-- A Validator is independent from a PlatPulse Node, Agent, and consensus
-- observation. Link history is append-preserving; ending a link sets its
-- validity boundary rather than deleting or rewriting the row.
CREATE TABLE validators (
    validator_id TEXT PRIMARY KEY,
    network_key TEXT NOT NULL REFERENCES networks(network_key),
    validator_node_id TEXT NOT NULL,
    display_name TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (network_key, validator_node_id)
);

CREATE INDEX validators_network_idx ON validators (network_key, validator_node_id);

CREATE TABLE node_validator_links (
    link_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    validator_id TEXT NOT NULL REFERENCES validators(validator_id),
    role TEXT NOT NULL CHECK (role IN ('primary', 'standby', 'observer')),
    valid_from TEXT NOT NULL,
    valid_until TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (valid_until IS NULL OR valid_until > valid_from)
);

CREATE INDEX node_validator_links_node_idx
    ON node_validator_links (node_id, valid_from, valid_until);
CREATE INDEX node_validator_links_validator_idx
    ON node_validator_links (validator_id, valid_from, valid_until);

-- The application performs a friendly overlap check, while these triggers
-- enforce the same invariant at the SQLite boundary for concurrent writers.
CREATE TRIGGER node_validator_links_no_overlap_insert
BEFORE INSERT ON node_validator_links
WHEN EXISTS (
    SELECT 1 FROM node_validator_links existing
    WHERE existing.node_id = NEW.node_id
      AND NEW.valid_from < COALESCE(existing.valid_until, '9999-12-31T23:59:59Z')
      AND (existing.valid_until IS NULL OR existing.valid_from < COALESCE(NEW.valid_until, '9999-12-31T23:59:59Z'))
)
BEGIN
    SELECT RAISE(ABORT, 'node_validator_link_overlap');
END;

CREATE TRIGGER node_validator_links_no_overlap_update
BEFORE UPDATE OF node_id, valid_from, valid_until ON node_validator_links
WHEN EXISTS (
    SELECT 1 FROM node_validator_links existing
    WHERE existing.link_id != NEW.link_id
      AND existing.node_id = NEW.node_id
      AND NEW.valid_from < COALESCE(existing.valid_until, '9999-12-31T23:59:59Z')
      AND (existing.valid_until IS NULL OR existing.valid_from < COALESCE(NEW.valid_until, '9999-12-31T23:59:59Z'))
)
BEGIN
    SELECT RAISE(ABORT, 'node_validator_link_overlap');
END;
