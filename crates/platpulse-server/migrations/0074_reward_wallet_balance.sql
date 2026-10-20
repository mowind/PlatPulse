-- Reward Wallet Balance (issue #246): the last PlatScan reading of the one
-- fixed Reward Wallet address. The address and the PlatScan deployment are
-- Server source constants, never configuration, Admin input or Agent input,
-- so the table holds one row in practice, keyed by that constant address.
--
-- Last-good semantics: a failed refresh only moves last_attempt_at and
-- error_category; balance and last_good_at are written only by a successful
-- refresh, so a restart keeps showing the last good balance. The balance is
-- the exact decimal LAT string PlatScan reported and never passes through a
-- floating-point column.
CREATE TABLE reward_wallet_balance (
    address TEXT PRIMARY KEY NOT NULL,
    balance TEXT,
    last_good_at TEXT,
    last_attempt_at TEXT NOT NULL,
    error_category TEXT
        CHECK (error_category IS NULL OR error_category IN ('upstream_unavailable', 'invalid_response')),
    CHECK ((balance IS NULL) = (last_good_at IS NULL))
);
