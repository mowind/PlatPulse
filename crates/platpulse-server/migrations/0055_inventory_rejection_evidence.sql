-- Durable per-rejection Inventory evidence (issue #181).
--
-- A whole-report rejection already had to be stored as an immutable receipt,
-- but the Owner had no way to see that an Agent's Inventory was being refused:
-- the reason lived only inside the `receipt_body` BLOB, and the rejected
-- report's Inventory revision/hash were never persisted at all. These columns
-- let the Admin query answer "is this Agent's latest ingestion attempt an
-- Inventory rejection, and what did it declare?" without decoding receipts.
--
-- Historical rows keep NULL: they predate the evidence, and the columns are
-- diagnostics, not part of the idempotency contract.
ALTER TABLE agent_report_receipts ADD COLUMN rejection_code TEXT;
ALTER TABLE agent_report_receipts ADD COLUMN inventory_revision INTEGER;
ALTER TABLE agent_report_receipts ADD COLUMN inventory_sha256 TEXT;

CREATE INDEX agent_report_receipts_latest_idx
    ON agent_report_receipts (agent_id, received_at DESC, report_sequence DESC);
