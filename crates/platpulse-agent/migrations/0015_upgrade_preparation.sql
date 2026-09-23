-- Bounded v1 upgrade-preparation evidence (issue #189).
--
-- The v1 -> v2 migration (ADR 0007) may only claim a verified baseline when the
-- Agent can prove that the declaration used by its final successfully applied
-- Closing is exactly what the Server most recently accepted. This table keeps
-- that proof as one bounded row: the Closing report identity, the complete
-- declaration it carried in the frozen v1 representation, the Server's accepted
-- revision/hash/protocol, and the post-Closing Boot linkage that the later
-- coordinated checkpoint must preserve.
--
-- It is migration evidence, not a new permanent Report History and not an
-- Inventory snapshot archive: one singleton row, overwritten by a later verified
-- preparation, never an append-only history.

CREATE TABLE upgrade_preparation (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    agent_id TEXT NOT NULL,
    agent_epoch INTEGER NOT NULL CHECK (agent_epoch >= 0),
    closing_report_id TEXT NOT NULL,
    closing_report_sequence INTEGER NOT NULL CHECK (closing_report_sequence > 0),
    closing_receipt_disposition TEXT NOT NULL CHECK (
        closing_receipt_disposition IN ('accepted', 'partially_accepted')
    ),
    -- Frozen v1 declaration: revision-inclusive and Node-order preserving.
    inventory_revision INTEGER NOT NULL CHECK (inventory_revision >= 1),
    inventory_sha256 TEXT NOT NULL,
    -- The verified frozen-v1 declaration exactly as hashed: the full
    -- NodeInventory JSON (revision + declared Node order), so a checkpoint can
    -- recompute inventory_sha256 independently.
    declaration_json TEXT NOT NULL,
    -- Boot linkage after the accepted Closing: the new Boot is pending its
    -- DrainedPrevious transition, and the closed Boot is its previous.
    closed_boot_id TEXT NOT NULL,
    next_boot_id TEXT NOT NULL,
    pending_transition TEXT NOT NULL CHECK (pending_transition = 'drained_previous'),
    -- The Server's own accepted baseline read back over the Agent API.
    accepted_inventory_revision INTEGER NOT NULL CHECK (accepted_inventory_revision >= 1),
    accepted_inventory_sha256 TEXT NOT NULL,
    accepted_inventory_protocol_major INTEGER NOT NULL,
    server_active_boot_id TEXT,
    server_active_boot_status TEXT NOT NULL,
    server_close_report_id TEXT,
    verified_at TEXT NOT NULL
);
