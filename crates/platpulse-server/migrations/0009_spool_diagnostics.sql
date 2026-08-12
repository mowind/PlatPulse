-- Durable Agent spool delivery diagnostics projected from immutable reports.
ALTER TABLE current_host_observations ADD COLUMN spool_in_flight INTEGER;
ALTER TABLE current_host_observations ADD COLUMN spool_last_delivery_error TEXT;
ALTER TABLE current_host_observations ADD COLUMN spool_last_delivery_at TEXT;
