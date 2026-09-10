-- Geo results are keyed by provider so switching providers can never reuse
-- another provider's country result, and each row keeps its own attempt and
-- success state for the bounded background resolution path (issue #132).
-- Existing rows were produced by the operator-provided local MMDB; their
-- country result, birth time, expiry, and last reference are preserved, and
-- the previous lookup time becomes the retained attempt/success time.
CREATE TABLE geo_location_cache_provider (
    provider TEXT NOT NULL,
    canonical_ip TEXT NOT NULL,
    country_code TEXT CHECK(country_code IS NULL OR country_code GLOB '[A-Z][A-Z]'),
    state TEXT NOT NULL CHECK(state IN ('current', 'no_country', 'failed')),
    created_at TEXT,
    last_attempt_at TEXT NOT NULL,
    last_success_at TEXT,
    last_referenced_at TEXT NOT NULL,
    expires_at TEXT,
    PRIMARY KEY (provider, canonical_ip)
);

INSERT INTO geo_location_cache_provider (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at)
SELECT 'local_mmdb', canonical_ip, country_code, 'current', created_at, last_lookup_at, last_lookup_at, last_referenced_at, expires_at
FROM geo_location_cache;

DROP TABLE geo_location_cache;

ALTER TABLE geo_location_cache_provider RENAME TO geo_location_cache;

CREATE INDEX geo_location_cache_expires_idx
    ON geo_location_cache (provider, expires_at);

CREATE INDEX geo_location_cache_reference_idx
    ON geo_location_cache (canonical_ip);
