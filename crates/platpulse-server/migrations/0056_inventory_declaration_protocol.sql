-- Server-owned Inventory declaration protocol evidence (issue #188).
--
-- v2 (Server-managed) declarations carry no Agent-assigned revision, so the
-- Admin Inventory diagnosis and the Agent Inventory rejection Attention must
-- explain the accepted/rejected declaration from the Server-assigned revision
-- and canonical fingerprint plus the actual rejection evidence, instead of
-- inventing an Agent-reported revision that no longer exists.
--
-- These nullable columns record the protocol major that produced the accepted
-- declaration (agents) and the refused report (agent_report_receipts). NULL
-- means unknown: rows that predate the evidence, and Agents that have never had
-- a declaration accepted. They are diagnostics, never part of idempotency or
-- the admission boundary, and an existing v1 receipt/rejection keeps its
-- original revision and hash.
ALTER TABLE agents ADD COLUMN inventory_protocol_major INTEGER;
ALTER TABLE agent_report_receipts ADD COLUMN inventory_protocol_major INTEGER;

-- Backfill: every declaration accepted or refused before this migration came
-- from the frozen v1 protocol, so an existing accepted hash is an
-- Agent-declared declaration. Rows without one stay NULL (unknown): an Agent
-- that has never had a declaration accepted has no protocol either.
UPDATE agents SET inventory_protocol_major = 1 WHERE inventory_sha256 IS NOT NULL;
UPDATE agent_report_receipts SET inventory_protocol_major = 1 WHERE rejection_code IS NOT NULL;
