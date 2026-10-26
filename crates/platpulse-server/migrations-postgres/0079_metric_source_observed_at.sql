-- Metric sample observation identity separated from corrected timeline (ADR 0014, issue #279).
--
-- PostgreSQL counterpart of migrations/0079_metric_source_observed_at.sql; both
-- backends share the logical schema version 79. Adds source_observed_at to raw
-- metric sample tables as the stable identity and raw primary key, leaving
-- observed_at for the skew-corrected Server-clock timeline.

ALTER TABLE node_metric_samples ADD COLUMN source_observed_at TEXT COLLATE "C";
UPDATE node_metric_samples SET source_observed_at = observed_at;
ALTER TABLE node_metric_samples ALTER COLUMN source_observed_at SET NOT NULL;
ALTER TABLE node_metric_samples DROP CONSTRAINT node_metric_samples_pkey;
ALTER TABLE node_metric_samples ADD PRIMARY KEY (node_id, metric, source_observed_at);

ALTER TABLE host_metric_samples ADD COLUMN source_observed_at TEXT COLLATE "C";
UPDATE host_metric_samples SET source_observed_at = observed_at;
ALTER TABLE host_metric_samples ALTER COLUMN source_observed_at SET NOT NULL;
ALTER TABLE host_metric_samples DROP CONSTRAINT host_metric_samples_pkey;
ALTER TABLE host_metric_samples ADD PRIMARY KEY (agent_id, metric, dimension, source_observed_at);
