-- Node Transfer (design §4.4, issue #46): Owner-preauthorized two-phase
-- ownership handover. The source Agent stays authoritative until the target
-- Agent declares the same Node ID in a valid Inventory and the Server
-- validates its Network Identity; only then does the Server switch
-- ownership atomically in the ingestion transaction. Every terminal and
-- conflict outcome is retained so the Admin surface can render the typed
-- timeline and the Audit trail.
CREATE TABLE node_transfers (
    transfer_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    source_agent_id TEXT NOT NULL REFERENCES agents(agent_id),
    target_agent_id TEXT NOT NULL REFERENCES agents(agent_id),
    status TEXT NOT NULL CHECK (status IN (
        'pending', 'completed', 'cancelled', 'expired', 'rejected',
        'conflict', 'identity_mismatch'
    )),
    operator_reason TEXT,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    cancelled_at TEXT,
    completed_at TEXT,
    rejection_code TEXT,
    rejection_reason TEXT,
    mismatched_fields TEXT,
    updated_at TEXT NOT NULL
);

CREATE INDEX node_transfers_node_idx ON node_transfers (node_id, created_at);
CREATE INDEX node_transfers_target_pending_idx
    ON node_transfers (target_agent_id, status);
