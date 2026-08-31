-- Applied receipts are terminal identity/outcome markers, not an Agent-side
-- archive of Server receipt bodies. Keep only a bounded recent window so
-- startup validation remains bounded even for long-lived Agents.
CREATE TABLE report_receipts_bounded (
    report_id TEXT PRIMARY KEY NOT NULL,
    report_body_sha256 TEXT NOT NULL,
    disposition TEXT NOT NULL CHECK (
        disposition IN ('accepted', 'partially_accepted', 'rejected')
    ),
    applied_at TEXT NOT NULL
);

INSERT INTO report_receipts_bounded (report_id, report_body_sha256, disposition, applied_at)
SELECT report_id, report_body_sha256, disposition, applied_at
FROM report_receipts
ORDER BY applied_at DESC, report_id DESC
LIMIT 256;

DROP TABLE report_receipts;
ALTER TABLE report_receipts_bounded RENAME TO report_receipts;

CREATE INDEX report_receipts_applied_at_idx
    ON report_receipts (applied_at, report_id);
