-- Incident Acknowledgment (parent #202, issue #203).
--
-- An Owner's durable, shared, non-retractable confirmation of ONE Incident
-- occurrence. The row names the acknowledged Incident instance, who confirmed
-- it, and when. It is never retracted or overwritten: the primary key makes the
-- first successful confirmation authoritative and every later request for the
-- same occurrence a no-op.
--
-- The acknowledgment does not resolve the Incident and does not change health,
-- recovery, evaluation, or notification behavior. A genuinely recovered
-- subject that faults again opens a NEW Incident row, whose acknowledgment is
-- independent of the old one. The acknowledged username is snapshotted so the
-- accountable fact survives a later rename or removal, and the same facts are
-- recorded as an Audit Event.

CREATE TABLE incident_acknowledgments (
    incident_id TEXT PRIMARY KEY REFERENCES alert_incidents(incident_id),
    acknowledged_by_user_id TEXT REFERENCES users(user_id),
    acknowledged_by_username TEXT NOT NULL,
    acknowledged_at TEXT NOT NULL
);
