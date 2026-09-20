-- Agent display metadata (design §15.2, webui.md §15.1): the
-- Owner-editable Server-owned display name and notes. Agent ID, actual Host
-- identity, receipt-derived liveness, the Agent Epoch, and local collection
-- configuration remain non-editable; an Agent without a name is still
-- identified by its stable Agent ID.

ALTER TABLE agents ADD COLUMN display_name TEXT;
ALTER TABLE agents ADD COLUMN notes TEXT;
