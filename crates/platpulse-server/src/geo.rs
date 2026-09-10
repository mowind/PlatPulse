//! Server-side Peer country resolution and its retained cache.
//!
//! The loader owns only an operator-provided MMDB reader. It never downloads
//! data or exposes a database path or raw IP through an HTTP DTO. A failed
//! reload updates the diagnostic state but deliberately keeps the previous
//! reader available for last-good lookups.
//!
//! Which provider is active is a persisted Owner decision (`server_settings`),
//! resolved once per process and never silently changed by an upgrade. Two
//! providers are implemented: the local MMDB reader owned by this module and
//! the external IPinfo lookup owned by `crate::geo_ipinfo`. Retained results
//! are keyed by provider and every background write is checked against the
//! configuration generation that scheduled it, so a result produced for one
//! provider can never be read or written as another provider's result.
//!
//! Lookups never run inside the report receipt transaction: the background
//! path in `crate::geo_backfill` owns scheduling, bounded concurrency, and
//! cache writes.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use maxminddb::Reader;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

/// Identifiers of the Geo providers this Server can select. `ipinfo` is the
/// external provider implemented by `crate::geo_ipinfo`; `geojs` is still
/// deliberately absent, because an unselectable identifier must never appear
/// in the Admin surface.
pub const PROVIDER_DISABLED: &str = "disabled";
pub const PROVIDER_LOCAL_MMDB: &str = "local_mmdb";
pub const PROVIDER_IPINFO: &str = "ipinfo";

/// `server_settings` keys holding the durable provider selection.
pub const SETTING_GEO_PROVIDER: &str = "geo_provider";
pub const SETTING_GEO_PROVIDER_GENERATION: &str = "geo_provider_generation";

pub const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub const CACHE_REBUILD_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const DATABASE_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const MAX_PEER_IP_CACHE_ROWS: i64 = 1024;
/// How long a Peer IP whose lookup produced no country result is left alone
/// before the background path retries it.
pub const ATTEMPT_RETRY_AGE: Duration = Duration::from_secs(60 * 60);
/// Upper bound on the work one background pass may schedule.
pub const MAX_BACKFILL_BATCH: usize = 128;
/// Upper bound on concurrent lookups inside one background pass.
pub const MAX_BACKFILL_CONCURRENCY: usize = 4;
/// Cadence of the background resolution loop. Report ingestion also wakes it
/// directly when a peer snapshot arrives, so this is an upper bound.
pub const BACKFILL_INTERVAL: Duration = Duration::from_secs(15);
pub const MAXMIND_ATTRIBUTION: &str = "This product includes GeoLite Data created by MaxMind, available from https://www.maxmind.com.";

/// A country lookup outcome that keeps "no country in the database" apart
/// from "no usable database", so a failure never erases a retained country.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeoLookup {
    /// The database returned a usable two-letter ISO country code.
    Country(String),
    /// The database was read successfully and has no country for this address.
    /// Ineligible addresses (private, special-purpose, documentation ranges)
    /// are refused by the trust boundary and have no country result either.
    NoCountry,
    /// No usable database is loaded and no provider answered, so no result
    /// can be produced at all.
    Unavailable,
    /// The external provider refused the request because this Server is being
    /// rate limited. It is deliberately separate from `Unavailable`: no
    /// authoritative result was produced, so the background path counts the
    /// address as still pending instead of recording a failed attempt.
    RateLimited,
}

/// The reason the Admin surface reports for a provider that needs an
/// operator-provided local database this deployment does not have.
pub const NO_LOCAL_DATABASE_REASON: &str =
    "No local GeoLite2 Country database is configured on this Server";

/// The Geo provider selected by the Owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoProvider {
    Disabled,
    LocalMmdb,
    /// The external IPinfo provider. It reads no local database and resolves
    /// countries by sending a canonical public Peer address to a fixed
    /// third-party HTTPS endpoint (`crate::geo_ipinfo`).
    Ipinfo,
}

impl GeoProvider {
    pub const ALL: [GeoProvider; 3] = [
        GeoProvider::Disabled,
        GeoProvider::LocalMmdb,
        GeoProvider::Ipinfo,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            GeoProvider::Disabled => PROVIDER_DISABLED,
            GeoProvider::LocalMmdb => PROVIDER_LOCAL_MMDB,
            GeoProvider::Ipinfo => PROVIDER_IPINFO,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GeoProvider::Disabled => "Disabled",
            GeoProvider::LocalMmdb => "Local MMDB",
            GeoProvider::Ipinfo => "IPinfo",
        }
    }

    /// Unknown persisted values fail safe to Disabled: an unrecognized
    /// provider string must never start resolution work.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            PROVIDER_DISABLED => Some(GeoProvider::Disabled),
            PROVIDER_LOCAL_MMDB => Some(GeoProvider::LocalMmdb),
            PROVIDER_IPINFO => Some(GeoProvider::Ipinfo),
            _ => None,
        }
    }

    /// Whether this provider resolves from an operator-provided local
    /// database. A provider that needs one cannot be selected without it.
    pub fn needs_local_database(self) -> bool {
        matches!(self, GeoProvider::LocalMmdb)
    }

    /// Whether selecting this provider sends observed Peer public addresses
    /// outside this Server. The Owner-only Admin surface states this before a
    /// selection is made, and the WebUI never has to hardcode which providers
    /// do it.
    pub fn sends_peer_addresses(self) -> bool {
        matches!(self, GeoProvider::Ipinfo)
    }

    /// The attribution the provider's terms require wherever its country
    /// results are shown. It is a stable Server-owned string: the browser
    /// never composes one, and a Disabled provider attributes nothing.
    pub fn attribution(self) -> Option<&'static str> {
        match self {
            GeoProvider::Disabled => None,
            GeoProvider::LocalMmdb => Some(MAXMIND_ATTRIBUTION),
            GeoProvider::Ipinfo => Some(crate::geo_ipinfo::IPINFO_ATTRIBUTION),
        }
    }
}

/// The process-local view of the persisted provider selection. The MMDB path
/// stays deployment configuration: it is read from the Server config at
/// startup and is never writable through the Admin API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeoConfig {
    pub provider: GeoProvider,
    pub generation: u64,
    pub mmdb_path: Option<PathBuf>,
}

impl GeoConfig {
    pub fn disabled() -> Self {
        Self {
            provider: GeoProvider::Disabled,
            generation: 0,
            mmdb_path: None,
        }
    }

    /// Why this deployment cannot run the given provider, or `None` when it
    /// can. The one explanation the Admin option list and the mutation guard
    /// both read, so an unavailable option can never disagree with the
    /// rejection.
    pub fn unavailable_reason(&self, provider: GeoProvider) -> Option<&'static str> {
        match provider {
            GeoProvider::LocalMmdb if self.mmdb_path.is_none() => Some(NO_LOCAL_DATABASE_REASON),
            _ => None,
        }
    }

    /// Whether this deployment can actually run the given provider. The one
    /// predicate the Admin surface reads for both the option list and the
    /// mutation guard, so the two can never disagree.
    pub fn can_run(&self, provider: GeoProvider) -> bool {
        self.unavailable_reason(provider).is_none()
    }
}

/// The durable provider selection read from `server_settings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeoSelection {
    pub provider: GeoProvider,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeoStatus {
    pub state: String,
    pub configured: bool,
    pub build_epoch: Option<u64>,
    pub digest: Option<String>,
    pub loaded_at: Option<String>,
    pub last_error: Option<String>,
}

struct LoadedDatabase {
    reader: Reader<Vec<u8>>,
    build_epoch: u64,
    digest: String,
    modified: Option<SystemTime>,
    size: u64,
}

pub struct GeoLoader {
    path: Option<PathBuf>,
    database: RwLock<Option<LoadedDatabase>>,
    status: RwLock<GeoStatus>,
}

impl std::fmt::Debug for GeoLoader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GeoLoader")
            .field("path_configured", &self.path.is_some())
            .field("status", &self.status())
            .finish()
    }
}

impl GeoLoader {
    /// Construct the loader without reading the configured file. Call `reload`
    /// from a blocking context to perform the initial load.
    pub fn new(path: Option<PathBuf>) -> Self {
        let configured = path.is_some();
        Self::with_status(
            path,
            GeoStatus {
                state: if configured { "error" } else { "disabled" }.to_owned(),
                configured,
                build_epoch: None,
                digest: None,
                loaded_at: None,
                last_error: configured.then(|| "Geo database has not loaded".to_owned()),
            },
        )
    }

    fn with_status(path: Option<PathBuf>, status: GeoStatus) -> Self {
        Self {
            path,
            database: RwLock::new(None),
            status: RwLock::new(status),
        }
    }

    pub fn disabled() -> Self {
        Self::new(None)
    }

    /// A test double for an Enabled, Current provider. Projection tests seed
    /// `geo_location_cache` directly, which is exactly the retained state a
    /// real provider produces; `disabled` is the only state the Public
    /// projection short-circuits on.
    #[cfg(test)]
    pub(crate) fn enabled_for_tests() -> Self {
        Self::with_status(
            None,
            GeoStatus {
                state: "current".to_owned(),
                configured: true,
                build_epoch: None,
                digest: None,
                loaded_at: Some("2026-01-01T00:00:00Z".to_owned()),
                last_error: None,
            },
        )
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn status(&self) -> GeoStatus {
        let build_epoch = self
            .database
            .read()
            .expect("GeoLoader database lock poisoned")
            .as_ref()
            .map(|database| database.build_epoch);
        let mut status = self.status.write().expect("GeoLoader status lock poisoned");
        if status.last_error.is_none() {
            if let Some(build_epoch) = build_epoch {
                status.state = state_for_build(build_epoch);
            }
        }
        status.clone()
    }

    /// Load a fresh database. On failure, the previous reader remains in
    /// place and status becomes Error; callers can continue last-good reads.
    pub fn reload(&self) -> bool {
        let Some(path) = self.path.as_deref() else {
            return false;
        };
        let result = load_database(path);
        match result {
            Ok(database) => {
                let loaded_at = crate::auth::format_rfc3339(crate::auth::now_utc());
                let mut guard = self
                    .database
                    .write()
                    .expect("GeoLoader database lock poisoned");
                let build_epoch = database.build_epoch;
                let digest = database.digest.clone();
                *guard = Some(database);
                drop(guard);
                let mut status = self.status.write().expect("GeoLoader status lock poisoned");
                status.state = state_for_build(build_epoch);
                status.build_epoch = Some(build_epoch);
                status.digest = Some(digest);
                status.loaded_at = Some(loaded_at);
                status.last_error = None;
                true
            }
            Err(error) => {
                let mut status = self.status.write().expect("GeoLoader status lock poisoned");
                status.state = "error".to_owned();
                status.last_error = Some(error);
                false
            }
        }
    }

    /// Check the configured file's cheap filesystem fingerprint and reload
    /// when it changes. The initial load is performed by `new`.
    pub fn reload_if_changed(&self) -> bool {
        let Some(path) = self.path.as_deref() else {
            return false;
        };
        let metadata = match file_fingerprint(path) {
            Some(metadata) => metadata,
            None => return self.reload(),
        };
        let changed = self
            .database
            .read()
            .expect("GeoLoader database lock poisoned")
            .as_ref()
            .is_none_or(|database| {
                database.modified != metadata.modified().ok() || database.size != metadata.len()
            });
        changed && self.reload()
    }

    /// Resolve one eligible public literal into a country outcome. The
    /// returned code is the two-letter ISO code from the Country database;
    /// a database that is not loaded reports `Unavailable` instead of an
    /// empty country so callers never overwrite retained evidence.
    pub fn resolve(&self, ip: &IpAddr) -> GeoLookup {
        if !eligible_public_ip(ip) {
            return GeoLookup::NoCountry;
        }
        let lookup_ip = match ip {
            IpAddr::V6(ipv6) => ipv6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(*ip),
            IpAddr::V4(_) => *ip,
        };
        let guard = self
            .database
            .read()
            .expect("GeoLoader database lock poisoned");
        let Some(database) = guard.as_ref() else {
            return GeoLookup::Unavailable;
        };
        let Ok(result) = database.reader.lookup(lookup_ip) else {
            return GeoLookup::Unavailable;
        };
        match result
            .decode_path::<String>(&maxminddb::path!["country", "iso_code"])
            .ok()
            .flatten()
            .map(|code| code.to_ascii_uppercase())
            .filter(|code| is_country_code(code))
        {
            Some(code) => GeoLookup::Country(code),
            None => GeoLookup::NoCountry,
        }
    }

    pub fn canonical_public_ip(value: &str) -> Option<String> {
        let ip = value.parse::<IpAddr>().ok()?;
        let canonical = match ip {
            IpAddr::V6(ipv6) => ipv6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            IpAddr::V4(_) => ip,
        };
        eligible_public_ip(&canonical).then(|| canonical.to_string())
    }
}

fn load_database(path: &Path) -> Result<LoadedDatabase, String> {
    use std::io::Read;
    crate::file_security::validate_private_file(path)?;
    let mut file = crate::file_security::open_readonly(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| "Geo database could not be read".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "Geo database metadata unavailable".to_owned())?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = format!("{:x}", hasher.finalize());
    let reader = Reader::from_source(bytes).map_err(|_| "Geo database is invalid".to_owned())?;
    Ok(LoadedDatabase {
        build_epoch: reader.metadata().build_epoch,
        digest,
        reader,
        modified: metadata.modified().ok(),
        size: metadata.len(),
    })
}

fn file_fingerprint(path: &Path) -> Option<std::fs::Metadata> {
    let file = crate::file_security::open_readonly(path).ok()?;
    file.metadata().ok()
}

fn state_for_build(build_epoch: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now.saturating_sub(build_epoch) > DATABASE_MAX_AGE.as_secs() {
        "stale".to_owned()
    } else {
        "current".to_owned()
    }
}

/// The two-letter ISO 3166-1 alpha-2 shape a value must have before it may
/// be retained as a country. Both providers apply the same filter, so a
/// malformed value can never become a country result on either path. The
/// shape is checked rather than a fixed ISO list because the providers
/// document the field as that code and the set of codes is not this Server's
/// to freeze.
pub(crate) fn is_country_code(value: &str) -> bool {
    value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_uppercase())
}

/// Server-side trust-boundary eligibility. Documentation and carrier-grade
/// NAT ranges are deliberately treated as non-public too.
pub fn eligible_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => eligible_ipv4(*ip),
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map_or_else(|| eligible_ipv6(*ip), eligible_ipv4),
    }
}

fn eligible_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.is_broadcast()
        // IANA special-purpose, documentation, benchmark, and reserved
        // ranges. Geo resolution is only for globally routable literals.
        || octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
        || (octets[0] == 198 && (18..=19).contains(&octets[1]))
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
        || octets[0] >= 224)
}

fn eligible_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    !(segments[0] & 0xe000 != 0x2000
        || ip.is_loopback()
        || ip.is_unspecified()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xff00) == 0xff00
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] == 0x2001 && segments[1] == 0x0002)
        || (segments[0] == 0x2001 && segments[1] == 0x0010)
        || (segments[0] & 0xfff0) == 0x3ff0)
}

pub fn country_centroid(country_code: &str) -> (Option<f64>, Option<f64>) {
    let centroid = match country_code {
        "AU" => (-25.2744, 133.7751),
        "BR" => (-14.2350, -51.9253),
        "CA" => (56.1304, -106.3468),
        "CN" => (35.8617, 104.1954),
        "DE" => (51.1657, 10.4515),
        "ES" => (40.4637, -3.7492),
        "FR" => (46.2276, 2.2137),
        "GB" => (55.3781, -3.4360),
        "HK" => (22.3193, 114.1694),
        "IN" => (20.5937, 78.9629),
        "IT" => (41.8719, 12.5674),
        "JP" => (36.2048, 138.2529),
        "KR" => (35.9078, 127.7669),
        "NL" => (52.1326, 5.2913),
        "RU" => (61.5240, 105.3188),
        "SE" => (60.1282, 18.6435),
        "SG" => (1.3521, 103.8198),
        "TW" => (23.6978, 120.9605),
        "US" => (37.0902, -95.7129),
        "VN" => (14.0583, 108.2772),
        _ => return (None, None),
    };
    (Some(centroid.0), Some(centroid.1))
}

/// Write the licensed GeoIP2 Country test database as a private file, the
/// way an operator provides one. Shared by the Geo test fixtures so they all
/// exercise the Server's real file validation.
#[cfg(test)]
pub(crate) fn write_test_database(path: &Path) {
    std::fs::write(
        path,
        include_bytes!("../test-data/GeoIP2-Country-Test.mmdb"),
    )
    .expect("the bundled GeoIP2 Country test fixture is readable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("the Geo test database permission can be set");
    }
}

/// The canonical shape of the provider-keyed country cache, shared by the
/// migration and the test fixtures so the two can never drift apart.
#[cfg(test)]
pub(crate) const CACHE_TABLE_DDL: &str = "CREATE TABLE geo_location_cache (provider TEXT NOT NULL, canonical_ip TEXT NOT NULL, country_code TEXT CHECK(country_code IS NULL OR country_code GLOB '[A-Z][A-Z]'), state TEXT NOT NULL CHECK(state IN ('current', 'no_country', 'failed')), created_at TEXT, last_attempt_at TEXT NOT NULL, last_success_at TEXT, last_referenced_at TEXT NOT NULL, expires_at TEXT, PRIMARY KEY (provider, canonical_ip))";

/// Remove rows that are no longer usable and enforce the bounded country
/// cache. Results retained for a provider that is not selected anymore are
/// deleted outright: they can never be read again, and a raw Peer IP must not
/// outlive the selection that produced it.
///
/// Expiry never deletes a row: an expired country result inside the hard
/// retention boundary is exactly the last-good Stale data the projections
/// serve, so cleanup only applies the hard boundary, the current-reference
/// rule, and the size bound.
pub async fn cleanup_cache(
    pool: &SqlitePool,
    now: &str,
    active_provider: GeoProvider,
) -> Result<u64, sqlx::Error> {
    let retired = sqlx::query("DELETE FROM geo_location_cache WHERE provider <> ?")
        .bind(active_provider.as_str())
        .execute(pool)
        .await?
        .rows_affected();
    let rebuild_before = cache_rebuild_cutoff(now);
    let rebuilt = sqlx::query("DELETE FROM geo_location_cache WHERE rowid IN (SELECT rowid FROM geo_location_cache WHERE created_at IS NOT NULL AND created_at <= ? ORDER BY created_at ASC, provider ASC, canonical_ip ASC LIMIT 1024)")
        .bind(&rebuild_before)
        .execute(pool)
        .await?
        .rows_affected();
    // A raw Peer address may not outlive the current Peer reference that
    // justified retaining it.
    let unreferenced = sqlx::query("DELETE FROM geo_location_cache WHERE NOT EXISTS (SELECT 1 FROM current_node_peers current WHERE current.remote_ip = geo_location_cache.canonical_ip)")
        .execute(pool)
        .await?
        .rows_affected();
    let trimmed = trim_cache(pool).await?;
    Ok(retired + rebuilt + unreferenced + trimmed)
}

/// Keep unreferenced raw-IP cache rows bounded even when many Nodes report
/// distinct Peers. Current-peer references are never evicted by this bound;
/// hard-age cleanup remains the authoritative retention limit for those rows.
pub async fn trim_cache<'e, E>(executor: E) -> Result<u64, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    Ok(sqlx::query(
        "DELETE FROM geo_location_cache WHERE rowid IN (SELECT cache.rowid FROM geo_location_cache cache WHERE NOT EXISTS (SELECT 1 FROM current_node_peers current WHERE current.remote_ip = cache.canonical_ip) ORDER BY cache.last_referenced_at ASC, cache.last_attempt_at ASC, cache.provider ASC, cache.canonical_ip ASC LIMIT MAX(0, (SELECT COUNT(*) FROM geo_location_cache) - ?))",
    )
    .bind(MAX_PEER_IP_CACHE_ROWS)
    .execute(executor)
    .await?
    .rows_affected())
}

/// The retry boundary for an address whose last attempt produced no country
/// result. Recent attempts are left alone so a large Peer set cannot turn
/// into unbounded work, and a database failure is not retried on every pass.
pub fn cache_attempt_cutoff(now: &str) -> String {
    let parsed = crate::auth::parse_rfc3339(now).unwrap_or_else(crate::auth::now_utc);
    crate::auth::format_rfc3339(
        parsed - time::Duration::seconds(ATTEMPT_RETRY_AGE.as_secs() as i64),
    )
}

/// Read the durable provider selection. Both keys are written together by
/// `write_provider_selection`; a missing or unrecognized provider value
/// fails safe to Disabled so an unknown string never starts resolution work.
pub async fn read_provider_selection<'e, E>(
    executor: E,
) -> Result<Option<GeoSelection>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT setting_key, setting_value FROM server_settings WHERE setting_key IN (?, ?)",
    )
    .bind(SETTING_GEO_PROVIDER)
    .bind(SETTING_GEO_PROVIDER_GENERATION)
    .fetch_all(executor)
    .await?;
    let mut provider = None;
    let mut generation = None;
    for (key, value) in rows {
        match key.as_str() {
            SETTING_GEO_PROVIDER => provider = Some(value),
            SETTING_GEO_PROVIDER_GENERATION => generation = value.parse::<u64>().ok(),
            _ => {}
        }
    }
    Ok(provider.map(|value| GeoSelection {
        provider: GeoProvider::parse(&value).unwrap_or(GeoProvider::Disabled),
        generation: generation.unwrap_or(0),
    }))
}

/// Advance and return the configuration generation. The increment and the
/// read are one SQL statement inside the caller's write transaction, so two
/// concurrent Owner changes can never persist the same generation.
pub async fn bump_provider_generation<'e, E>(executor: E, now: &str) -> Result<u64, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let value: i64 = sqlx::query_scalar(
        "INSERT INTO server_settings (setting_key, setting_value, updated_at) VALUES (?, '1', ?) ON CONFLICT(setting_key) DO UPDATE SET setting_value = CAST(CAST(setting_value AS INTEGER) + 1 AS TEXT), updated_at = excluded.updated_at RETURNING CAST(setting_value AS INTEGER)",
    )
    .bind(SETTING_GEO_PROVIDER_GENERATION)
    .bind(now)
    .fetch_one(executor)
    .await?;
    Ok(value.max(0) as u64)
}

/// Persist a full provider selection. Startup seeding and tests use this;
/// the Admin mutation advances the generation with
/// [`bump_provider_generation`] inside its own transaction instead.
pub async fn write_provider_selection<'e, E>(
    executor: E,
    selection: GeoSelection,
    now: &str,
) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "INSERT INTO server_settings (setting_key, setting_value, updated_at) VALUES (?, ?, ?), (?, ?, ?) ON CONFLICT(setting_key) DO UPDATE SET setting_value=excluded.setting_value, updated_at=excluded.updated_at",
    )
    .bind(SETTING_GEO_PROVIDER)
    .bind(selection.provider.as_str())
    .bind(now)
    .bind(SETTING_GEO_PROVIDER_GENERATION)
    .bind(selection.generation.to_string())
    .bind(now)
    .execute(executor)
    .await?;
    Ok(())
}

/// Resolve the durable provider selection once per installation. Before the
/// key exists, the deployment's MMDB configuration decides: an installation
/// that already resolved with a local database keeps doing so, and an
/// installation without one stays Disabled. Neither branch adds outbound
/// traffic, so an upgrade never changes the privacy boundary by itself.
pub async fn ensure_provider_selection(
    pool: &SqlitePool,
    mmdb_configured: bool,
) -> Result<GeoSelection, sqlx::Error> {
    if let Some(selection) = read_provider_selection(pool).await? {
        return Ok(selection);
    }
    let selection = GeoSelection {
        provider: if mmdb_configured {
            GeoProvider::LocalMmdb
        } else {
            GeoProvider::Disabled
        },
        generation: 1,
    };
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());
    write_provider_selection(pool, selection, &now).await?;
    Ok(selection)
}

pub fn cache_rebuild_cutoff(now: &str) -> String {
    let parsed = crate::auth::parse_rfc3339(now).unwrap_or_else(crate::auth::now_utc);
    crate::auth::format_rfc3339(
        parsed - time::Duration::seconds(CACHE_REBUILD_AGE.as_secs() as i64),
    )
}

pub fn cache_expiry(now: &str) -> String {
    let parsed = crate::auth::parse_rfc3339(now).unwrap_or_else(crate::auth::now_utc);
    crate::auth::format_rfc3339(parsed + time::Duration::seconds(CACHE_MAX_AGE.as_secs() as i64))
}

/// Choose the row birth time and expiry for a successful lookup. A row that
/// reaches the hard retention boundary is rebuilt; otherwise its absolute
/// expiry is capped at that boundary even when successful lookups refresh the
/// normal 24-hour TTL.
pub fn cache_refresh_window(existing_created_at: Option<&str>, now: &str) -> (String, String) {
    let now = crate::auth::parse_rfc3339(now).unwrap_or_else(crate::auth::now_utc);
    let cutoff = now - time::Duration::seconds(CACHE_REBUILD_AGE.as_secs() as i64);
    let requested_expiry = now + time::Duration::seconds(CACHE_MAX_AGE.as_secs() as i64);
    let Some(existing_created_at) = existing_created_at
        .and_then(crate::auth::parse_rfc3339)
        .filter(|created| *created > cutoff)
    else {
        return (
            crate::auth::format_rfc3339(now),
            crate::auth::format_rfc3339(requested_expiry),
        );
    };
    let hard_expiry =
        existing_created_at + time::Duration::seconds(CACHE_REBUILD_AGE.as_secs() as i64);
    (
        crate::auth::format_rfc3339(existing_created_at),
        crate::auth::format_rfc3339(requested_expiry.min(hard_expiry)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[test]
    fn rejects_private_and_reserved_ranges() {
        for value in [
            "10.0.0.1",
            "127.0.0.1",
            "0.1.2.3",
            "192.0.0.1",
            "169.254.1.1",
            "192.0.2.1",
            "198.18.1.1",
            "198.19.1.1",
            "198.51.100.1",
            "203.0.113.1",
            "100.64.0.1",
            "224.0.0.1",
            "2001:db8::1",
            "2001:2::1",
            "2001:10::1",
            "4000::1",
            "fec0::1",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "::ffff:192.168.1.1",
        ] {
            let ip = value.parse().unwrap();
            assert!(!eligible_public_ip(&ip), "{value} should be rejected");
        }
        assert!(eligible_public_ip(&"8.8.8.8".parse().unwrap()));
        assert!(eligible_public_ip(&"2001:4860:4860::8888".parse().unwrap()));
    }

    #[tokio::test]
    async fn trims_cache_to_the_configured_bound() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(CACHE_TABLE_DDL).execute(&pool).await.unwrap();
        for index in 0..=MAX_PEER_IP_CACHE_ROWS {
            let ip = format!("198.18.{}.{}", index / 256, index % 256);
            let timestamp = format!("2026-01-01T00:00:{index:04}Z");
            sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('local_mmdb', ?, 'US', 'current', ?, ?, ?, ?, ?)")
                .bind(ip)
                .bind(&timestamp)
                .bind(&timestamp)
                .bind(&timestamp)
                .bind(&timestamp)
                .bind("2026-01-02T00:00:00Z")
                .execute(&pool)
                .await
                .unwrap();
        }

        sqlx::query("CREATE TABLE current_node_peers (remote_ip TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_node_peers (remote_ip) VALUES ('198.18.0.0')")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(trim_cache(&pool).await.unwrap(), 1);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM geo_location_cache")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, MAX_PEER_IP_CACHE_ROWS);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM geo_location_cache WHERE canonical_ip='198.18.0.0'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );
    }

    #[test]
    fn loads_country_deterministically_and_reports_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("GeoIP2-Country-Test.mmdb");
        write_test_database(&path);
        let loader = GeoLoader::new(Some(path));
        assert!(loader.reload());
        let status = loader.status();
        assert_eq!(status.state, "stale");
        assert!(status.build_epoch.is_some());
        assert!(status.digest.is_some());
        assert_eq!(
            loader.resolve(&"89.160.20.112".parse().unwrap()),
            GeoLookup::Country("SE".to_owned())
        );
        // A private address is refused by the trust boundary rather than
        // reported as a database failure.
        assert_eq!(
            loader.resolve(&"10.0.0.1".parse().unwrap()),
            GeoLookup::NoCountry
        );
    }

    #[test]
    fn failed_reload_keeps_the_last_good_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("GeoIP2-Country-Test.mmdb");
        write_test_database(&path);
        let loader = GeoLoader::new(Some(path.clone()));
        assert!(loader.reload());
        assert_eq!(
            loader.resolve(&"89.160.20.112".parse().unwrap()),
            GeoLookup::Country("SE".to_owned())
        );
        std::fs::write(&path, b"not an MMDB").unwrap();
        assert!(!loader.reload());
        assert_eq!(loader.status().state, "error");
        assert_eq!(
            loader.resolve(&"89.160.20.112".parse().unwrap()),
            GeoLookup::Country("SE".to_owned())
        );
        std::fs::write(
            &path,
            include_bytes!("../test-data/GeoIP2-Country-Test.mmdb"),
        )
        .unwrap();
        assert!(loader.reload_if_changed());
        assert!(loader.status().last_error.is_none());
    }

    #[test]
    fn cache_refresh_rebuilds_at_the_hard_retention_boundary() {
        assert_eq!(
            cache_refresh_window(Some("2026-01-01T00:00:00Z"), "2026-01-30T12:00:00Z"),
            (
                "2026-01-01T00:00:00Z".to_owned(),
                "2026-01-31T00:00:00Z".to_owned()
            )
        );
        assert_eq!(
            cache_refresh_window(Some("2025-12-01T00:00:00Z"), "2026-01-01T00:00:00Z"),
            (
                "2026-01-01T00:00:00Z".to_owned(),
                "2026-01-02T00:00:00Z".to_owned()
            )
        );
    }

    #[test]
    fn cache_expiry_is_one_day_after_observation() {
        assert_eq!(cache_expiry("2026-01-01T00:00:00Z"), "2026-01-02T00:00:00Z");
        assert_eq!(
            cache_rebuild_cutoff("2026-01-31T00:00:00Z"),
            "2026-01-01T00:00:00Z"
        );
    }
    #[test]
    fn disabled_loader_is_explicit_and_has_no_database_metadata() {
        let status = GeoLoader::disabled().status();
        assert_eq!(status.state, "disabled");
        assert!(!status.configured);
        assert!(status.build_epoch.is_none());
        assert!(status.digest.is_none());
        assert!(status.last_error.is_none());
    }
    #[test]
    fn canonicalizes_only_literals() {
        assert_eq!(
            GeoLoader::canonical_public_ip("8.8.8.8"),
            Some("8.8.8.8".to_owned())
        );
        assert_eq!(
            GeoLoader::canonical_public_ip("::ffff:8.8.8.8"),
            Some("8.8.8.8".to_owned())
        );
        assert_eq!(GeoLoader::canonical_public_ip("example.com"), None);
        assert_eq!(GeoLoader::canonical_public_ip("203.0.113.1"), None);
    }
}
