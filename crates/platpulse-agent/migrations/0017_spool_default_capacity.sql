-- Raise the shipped Durable Spool default to 64 MiB (48 MiB flush reserve).
-- The effective policy is recorded from [spool] configuration at startup, so
-- this only aligns a Store whose row still carries the previous defaults.
UPDATE spool_state
SET max_bytes = 67108864,
    preflush_bytes = 50331648
WHERE singleton = 1
  AND max_bytes = 2097152
  AND preflush_bytes = 1572864
  AND max_age_seconds = 86400;
