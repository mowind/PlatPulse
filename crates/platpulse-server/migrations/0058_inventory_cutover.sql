-- Coordinated v1 -> v2 production cutover gate (issue #192).
--
-- ADR 0007 and design 15.10.4/15.10.5 require the offline conversion
-- (migration 0057) to remain unauthorized until an operator passes one explicit
-- switch gate that re-binds the verified preparation, the coordinated
-- checkpoint and the offline conversion, and only then resumes business writes.
--
-- The presence of a row in inventory_migration marks a *converted* deployment.
-- This singleton row is written by 'platpulse-server cutover resume' into that
-- converted Server database, after the command has re-verified the source
-- checkpoint and the converted deployment. Until it exists the Server refuses
-- ordinary collection/ingestion, Admin mutations and every external-effect
-- worker, so a converted database can never be served as if the switch had
-- happened. After it exists ordinary Reports are v2-only and the frozen v1
-- route is replay-only.
--
-- The binding hashes keep the durable marker tied to the exact checkpoint and
-- conversion manifests it was authorized against; a re-run that sees a
-- different source is refused instead of silently adopting it. This table does
-- not replace or extend the existing Report Receipt retention (issue #184 stays
-- separate) and does not archive any old-protocol payload.
CREATE TABLE inventory_cutover (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    state TEXT NOT NULL CHECK (state = 'resumed'),
    resumed_at TEXT NOT NULL,
    checkpoint_manifest_sha256 TEXT NOT NULL,
    conversion_manifest_sha256 TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    protocol_major INTEGER NOT NULL CHECK (protocol_major = 2),
    preserved_revision INTEGER NOT NULL CHECK (preserved_revision >= 0),
    previous_sha256 TEXT,
    fingerprint_sha256 TEXT
);
