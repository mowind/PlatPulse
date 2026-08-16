CREATE TABLE geo_location_cache (
    canonical_ip TEXT PRIMARY KEY NOT NULL,
    country_code TEXT NOT NULL CHECK(country_code GLOB '[A-Z][A-Z]'),
    created_at TEXT NOT NULL,
    last_lookup_at TEXT NOT NULL,
    last_referenced_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX geo_location_cache_expires_idx
    ON geo_location_cache (expires_at);
