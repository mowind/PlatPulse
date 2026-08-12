-- Preserve the authoritative complete Inventory revision even when it is empty.
ALTER TABLE agents ADD COLUMN last_inventory_revision INTEGER NOT NULL DEFAULT 0 CHECK (last_inventory_revision >= 0);
