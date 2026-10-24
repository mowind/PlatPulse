-- Spool Backlog Age derived at Report receipt (ADR 0014, issue #272).
--
-- PostgreSQL counterpart of migrations/0077_spool_backlog_age.sql; both
-- backends share the logical schema version 77. Ingestion derives the age from
-- the receipt instant and the Report's own measured clock skew, so Admin shows
-- how long the oldest queued Report has really been waiting instead of the
-- stale reading baked into an immutable Report. See the SQLite migration for
-- why the pair is cleared rather than preserved when a Report carries no Spool
-- component.
ALTER TABLE current_host_observations ADD COLUMN spool_backlog_age_ms BIGINT;
ALTER TABLE current_host_observations ADD COLUMN spool_backlog_age_basis TEXT COLLATE "C";
