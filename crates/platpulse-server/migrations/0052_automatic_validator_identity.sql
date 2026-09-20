-- #173: automatic Node Validator Links and Current Validator Status.
--
-- Automatic correspondence is derived by the Server from a validated Network
-- and the full P2P public key observed on the Node (its enode). It never uses a
-- PlatPulse Node UUID, a shortened fingerprint, a display name, an IP address,
-- or a legacy Owner-maintained link.
--
-- The legacy role column becomes nullable so an automatic Link never fabricates
-- a primary/standby/observer role: origin separates the two models and the
-- one-time migration (#174) removes the manual generation. The table is rebuilt
-- because SQLite cannot relax NOT NULL in place.
CREATE TABLE node_validator_links_new (
    link_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    validator_id TEXT NOT NULL REFERENCES validators(validator_id),
    role TEXT CHECK (role IS NULL OR role IN ('primary', 'standby', 'observer')),
    origin TEXT NOT NULL DEFAULT 'manual' CHECK (origin IN ('manual', 'automatic')),
    valid_from TEXT NOT NULL,
    valid_until TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (valid_until IS NULL OR valid_until > valid_from)
);

INSERT INTO node_validator_links_new (link_id, node_id, validator_id, role, origin, valid_from, valid_until, created_at, updated_at)
    SELECT link_id, node_id, validator_id, role, 'manual', valid_from, valid_until, created_at, updated_at
      FROM node_validator_links;

DROP TABLE node_validator_links;

ALTER TABLE node_validator_links_new RENAME TO node_validator_links;

CREATE INDEX node_validator_links_node_idx
    ON node_validator_links (node_id, valid_from, valid_until);
CREATE INDEX node_validator_links_validator_idx
    ON node_validator_links (validator_id, valid_from, valid_until);
CREATE INDEX node_validator_links_origin_idx
    ON node_validator_links (origin, node_id, valid_from, valid_until);

-- Overlap remains forbidden per Node and per model: an automatic key change
-- closes the previous interval before the next one opens, while a legacy
-- manual row cannot block automatic discovery before #174 deletes it.
CREATE TRIGGER node_validator_links_no_overlap_insert
BEFORE INSERT ON node_validator_links
WHEN EXISTS (
    SELECT 1 FROM node_validator_links existing
    WHERE existing.node_id = NEW.node_id
      AND existing.origin = NEW.origin
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
      AND existing.origin = NEW.origin
      AND NEW.valid_from < COALESCE(existing.valid_until, '9999-12-31T23:59:59Z')
      AND (existing.valid_until IS NULL OR existing.valid_from < COALESCE(NEW.valid_until, '9999-12-31T23:59:59Z'))
)
BEGIN
    SELECT RAISE(ABORT, 'node_validator_link_overlap');
END;

-- Per-Node identity discovery outcome. A Node without an automatic Link
-- exposes a specific, Server-owned reason instead of a guessed or zero value.
-- Provider payloads never enter this table.
CREATE TABLE node_validator_identity_status (
    node_id TEXT PRIMARY KEY REFERENCES nodes(node_id),
    state TEXT NOT NULL CHECK (state IN (
        'identified',
        'missing_public_key',
        'invalid_public_key',
        'network_identity_missing',
        'network_identity_mismatch'
    )),
    observed_node_key TEXT,
    updated_at TEXT NOT NULL
);

CREATE INDEX node_validator_identity_status_state_idx
    ON node_validator_identity_status (state, updated_at);
