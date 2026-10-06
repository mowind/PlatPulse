-- Activity/identity evidence has its own confirmation time. An authoritative
-- absence supersedes old activity without renewing old detail metrics.
ALTER TABLE current_validator_insights ADD COLUMN last_good_verdict_outcome TEXT
    CHECK (last_good_verdict_outcome IN ('success', 'empty'));
ALTER TABLE current_validator_insights ADD COLUMN last_good_verdict_received_at TEXT;

-- The shipped PlatScan normalizer always supplies explicit Activity on success,
-- so its current successful answer has recoverable confirmation provenance.
-- Other Provider implementations may return metric-only success while retaining
-- old raw Activity (including success -> empty -> metric-only success), so their
-- cached value is not proof of the latest verdict. Legacy failures are likewise
-- ambiguous. Leave these cases Unknown until the next authoritative observation;
-- never backfill a verdict from their metric receipt or a failed attempt.
UPDATE current_validator_insights
SET last_good_verdict_outcome = 'success',
    last_good_verdict_received_at = last_good_received_at
WHERE source = 'platscan' AND outcome = 'success' AND activity IS NOT NULL AND last_good_received_at IS NOT NULL;
UPDATE current_validator_insights
SET last_good_verdict_outcome = 'empty',
    last_good_verdict_received_at = last_attempt_received_at
WHERE outcome = 'empty';
