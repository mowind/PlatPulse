-- Durable per-Node recovery cursor and unresolved ranges.
CREATE TABLE node_recovery_state (
    node_id TEXT PRIMARY KEY,
    boot_id TEXT,
    last_head INTEGER CHECK (last_head IS NULL OR last_head >= 0),
    pending_from INTEGER CHECK (pending_from IS NULL OR pending_from >= 0),
    pending_to INTEGER CHECK (pending_to IS NULL OR pending_to >= pending_from),
    pending_trigger TEXT,
    pending_reason TEXT,
    updated_at TEXT NOT NULL
);
