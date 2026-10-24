-- Spool Backlog Age derived at Report receipt (ADR 0014, issue #272).
--
-- A Report is immutable, so the `spool_oldest_queued_age_ms` it carries always
-- describes the backlog as of its generation instant. A replayed Report keeps
-- saying the same thing hours later, while Admin needs the Server's answer: how
-- long the oldest queued Report has really been waiting. Ingestion therefore
-- derives a Spool Backlog Age from the receipt instant and the clock skew the
-- Report itself measured, and records it here next to (not instead of) the
-- Agent-reported value.
--
-- The pair is written on every Report that carries a Spool component. A Report
-- that carries none writes NULL in both columns: an unknown reading must never
-- keep an older number alive, so the Admin projection can show "unknown"
-- instead of a backlog that has since cleared.
ALTER TABLE current_host_observations ADD COLUMN spool_backlog_age_ms INTEGER;
ALTER TABLE current_host_observations ADD COLUMN spool_backlog_age_basis TEXT;
