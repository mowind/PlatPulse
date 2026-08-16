//! Optional Server-side GeoLite2 Country resolution.
//!
//! The loader owns only an operator-provided MMDB reader. It never downloads
//! data or exposes a database path or raw IP through an HTTP DTO. A failed
//! reload updates the diagnostic state but deliberately keeps the previous
//! reader available for last-good lookups.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use maxminddb::Reader;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

pub const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub const CACHE_REBUILD_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const DATABASE_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const MAX_PEER_IP_CACHE_ROWS: i64 = 1024;
pub const MAXMIND_ATTRIBUTION: &str = "This product includes GeoLite Data created by MaxMind, available from https://www.maxmind.com.";

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
        Self {
            path,
            database: RwLock::new(None),
            status: RwLock::new(GeoStatus {
                state: if configured { "error" } else { "disabled" }.to_owned(),
                configured,
                build_epoch: None,
                digest: None,
                loaded_at: None,
                last_error: configured.then(|| "Geo database has not loaded".to_owned()),
            }),
        }
    }

    pub fn disabled() -> Self {
        Self::new(None)
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
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => return self.reload(),
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

    /// Resolve only an eligible public literal. The returned value contains
    /// only the two-letter country code extracted from the Country database.
    pub fn lookup_country(&self, ip: &IpAddr) -> Option<String> {
        if !eligible_public_ip(ip) {
            return None;
        }
        let lookup_ip = match ip {
            IpAddr::V6(ipv6) => ipv6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(*ip),
            IpAddr::V4(_) => *ip,
        };
        let guard = self
            .database
            .read()
            .expect("GeoLoader database lock poisoned");
        let database = guard.as_ref()?;
        let result = database.reader.lookup(lookup_ip).ok()?;
        result
            .decode_path::<String>(&maxminddb::path!["country", "iso_code"])
            .ok()
            .flatten()
            .map(|code| code.to_ascii_uppercase())
            .filter(|code| code.len() == 2 && code.bytes().all(|byte| byte.is_ascii_uppercase()))
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
    let bytes = std::fs::read(path).map_err(|_| "Geo database could not be read".to_owned())?;
    let metadata =
        std::fs::metadata(path).map_err(|_| "Geo database metadata unavailable".to_owned())?;
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

/// Remove expired raw-IP rows and enforce the bounded country cache. Current-peer
/// references are intentionally not enough to extend the 24-hour cache lifetime.
pub async fn cleanup_cache(pool: &SqlitePool, now: &str) -> Result<u64, sqlx::Error> {
    let rebuild_before = cache_rebuild_cutoff(now);
    let rebuilt = sqlx::query("DELETE FROM geo_location_cache WHERE canonical_ip IN (SELECT canonical_ip FROM geo_location_cache WHERE created_at <= ? ORDER BY created_at ASC, canonical_ip ASC LIMIT 1024)")
        .bind(&rebuild_before)
        .execute(pool)
        .await?
        .rows_affected();
    let expired = sqlx::query("DELETE FROM geo_location_cache WHERE canonical_ip IN (SELECT canonical_ip FROM geo_location_cache WHERE expires_at <= ? ORDER BY expires_at ASC, canonical_ip ASC LIMIT 1024)")
        .bind(now)
        .execute(pool)
        .await?
        .rows_affected();
    let trimmed = trim_cache(pool).await?;
    Ok(rebuilt + expired + trimmed)
}

/// Keep unreferenced raw-IP cache rows bounded even when many Nodes report
/// distinct Peers. Current-peer references are never evicted by this bound;
/// hard-age cleanup remains the authoritative retention limit for those rows.
pub async fn trim_cache<'e, E>(executor: E) -> Result<u64, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    Ok(sqlx::query(
        "DELETE FROM geo_location_cache WHERE canonical_ip IN (SELECT cache.canonical_ip FROM geo_location_cache cache WHERE NOT EXISTS (SELECT 1 FROM current_node_peers current WHERE current.remote_ip = cache.canonical_ip) ORDER BY cache.last_referenced_at ASC, cache.last_lookup_at ASC, cache.canonical_ip ASC LIMIT MAX(0, (SELECT COUNT(*) FROM geo_location_cache) - ?))",
    )
    .bind(MAX_PEER_IP_CACHE_ROWS)
    .execute(executor)
    .await?
    .rows_affected())
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
        sqlx::query(
            "CREATE TABLE geo_location_cache (canonical_ip TEXT PRIMARY KEY, country_code TEXT NOT NULL, created_at TEXT NOT NULL, last_lookup_at TEXT NOT NULL, last_referenced_at TEXT NOT NULL, expires_at TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for index in 0..=MAX_PEER_IP_CACHE_ROWS {
            let ip = format!("198.18.{}.{}", index / 256, index % 256);
            let timestamp = format!("2026-01-01T00:00:{index:04}Z");
            sqlx::query("INSERT INTO geo_location_cache (canonical_ip, country_code, created_at, last_lookup_at, last_referenced_at, expires_at) VALUES (?, 'US', ?, ?, ?, ?)")
                .bind(ip)
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
        std::fs::write(
            &path,
            include_bytes!("../test-data/GeoIP2-Country-Test.mmdb"),
        )
        .unwrap();
        let loader = GeoLoader::new(Some(path));
        assert!(loader.reload());
        let status = loader.status();
        assert_eq!(status.state, "stale");
        assert!(status.build_epoch.is_some());
        assert!(status.digest.is_some());
        assert_eq!(
            loader.lookup_country(&"89.160.20.112".parse().unwrap()),
            Some("SE".to_owned())
        );
        assert_eq!(loader.lookup_country(&"10.0.0.1".parse().unwrap()), None);
    }

    #[test]
    fn failed_reload_keeps_the_last_good_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("GeoIP2-Country-Test.mmdb");
        std::fs::write(
            &path,
            include_bytes!("../test-data/GeoIP2-Country-Test.mmdb"),
        )
        .unwrap();
        let loader = GeoLoader::new(Some(path.clone()));
        assert!(loader.reload());
        assert_eq!(
            loader.lookup_country(&"89.160.20.112".parse().unwrap()),
            Some("SE".to_owned())
        );
        std::fs::write(&path, b"not an MMDB").unwrap();
        assert!(!loader.reload());
        assert_eq!(loader.status().state, "error");
        assert_eq!(
            loader.lookup_country(&"89.160.20.112".parse().unwrap()),
            Some("SE".to_owned())
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
