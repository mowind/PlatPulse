-- Canonical last-good Validator Activity on the existing current insight row.
-- Provider Activity is a live Server snapshot signal, not a new analytics
-- dimension: no Activity history table is created, and Provider values never
-- enter Node Health or Server readiness.
ALTER TABLE current_validator_insights
    ADD COLUMN activity TEXT
    CHECK (activity IN ('active', 'producing', 'exiting', 'exited', 'verifying', 'locked'));
