-- Retain the most recent complete observation view after its immutable
-- delivery row is acknowledged and removed from the Durable Spool.
ALTER TABLE agent_state ADD COLUMN last_report_body BLOB;

-- Preserve the existing most-recent immutable report when upgrading an Agent
-- Store that predates the snapshot column.
UPDATE agent_state
SET last_report_body = (
    SELECT body
    FROM reports AS report
    WHERE report.agent_epoch = agent_state.agent_epoch
      AND (agent_state.boot_id IS NULL OR report.boot_id = agent_state.boot_id)
      AND report.report_sequence <= agent_state.report_sequence
    ORDER BY report.report_sequence DESC, report.created_at DESC, report.report_id DESC
    LIMIT 1
)
WHERE singleton = 1;
