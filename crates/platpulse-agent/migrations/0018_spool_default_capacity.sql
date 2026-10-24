-- Raise the shipped Durable Spool default to 512 MiB (384 MiB flush reserve),
-- which covers the reference four-hour Server outage at a one-second collection
-- interval with about 60 peers per Host (issue #273). The effective policy is
-- recorded from [spool] configuration at startup, so this only aligns a Store
-- whose row still carries the previous defaults.
UPDATE spool_state
SET max_bytes = 536870912,
    preflush_bytes = 402653184
WHERE singleton = 1
  AND max_bytes = 67108864
  AND preflush_bytes = 50331648
  AND max_age_seconds = 86400;
