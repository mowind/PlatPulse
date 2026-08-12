-- Exactly-once ownership of durable samples while a report is in flight.
CREATE TABLE report_sample_assignments (
    report_id TEXT NOT NULL REFERENCES reports(report_id) ON DELETE CASCADE,
    node_id TEXT NOT NULL,
    sample_kind TEXT NOT NULL CHECK (sample_kind IN ('block', 'gap')),
    from_height INTEGER NOT NULL CHECK (from_height >= 0),
    to_height INTEGER NOT NULL CHECK (to_height >= from_height),
    PRIMARY KEY (report_id, node_id, sample_kind, from_height, to_height)
);
CREATE UNIQUE INDEX report_sample_assignments_sample_idx
    ON report_sample_assignments (node_id, sample_kind, from_height, to_height);
