-- Offline Server-managed Inventory baseline conversion (issue #191).
--
-- ADR 0007 and design 15.10.4 require converting an already-accepted frozen-v1
-- Inventory baseline to the v2 canonical declaration fingerprint without
-- changing the accepted revision. The converted baseline itself lives in the
-- existing agents.inventory_sha256 (now the v2 fingerprint) and
-- agents.inventory_protocol_major (now 2) columns. This table records the
-- conversion as one bounded row so an offline verifier, and later diagnostics,
-- can explain the old and new fingerprint algorithms instead of inferring them
-- from the current Node projection, which cannot reconstruct the declaration.
--
-- NULL previous_sha256/fingerprint_sha256 mean the Agent had never had an
-- Inventory accepted (a legitimate uninitialized state). That is never turned
-- into an invented accepted empty declaration; the first legal v2 acceptance
-- still assigns revision 1.
CREATE TABLE inventory_migration (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    protocol_major INTEGER NOT NULL CHECK (protocol_major = 2),
    preserved_revision INTEGER NOT NULL CHECK (preserved_revision >= 0),
    previous_sha256 TEXT,
    fingerprint_sha256 TEXT,
    converted_at TEXT NOT NULL
);
