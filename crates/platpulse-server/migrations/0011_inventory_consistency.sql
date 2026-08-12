-- Persist the canonical inventory content hash so equal revisions cannot
-- silently change Node ownership or retire siblings.
ALTER TABLE agents ADD COLUMN inventory_sha256 TEXT;
