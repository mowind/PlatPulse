//! Background Peer country resolution.
//!
//! Report ingestion records current Peer references and never performs a
//! country lookup inside the receipt transaction. This module owns the
//! asynchronous execution boundary: each pass schedules a bounded,
//! IP-deduplicated set of missing or expired addresses, resolves them with
//! limited concurrency, and writes a result back only while the provider and
//! the configuration generation that scheduled it are still selected.
//!
//! Every result carries the same retention rule as the Public projection: a
//! retained country result stays usable until its hard cache boundary, a
//! successful lookup may refresh it within that boundary, and a failure only
//! records the attempt so a last-good value is never rewritten as current.
//!
//! The provider decides only *how* one address is resolved: the local MMDB
//! reader runs on a blocking thread, and every External Geo Provider (IPinfo
//! and GeoJS) runs on the shared bounded outbound boundary in
//! `crate::geo_external` through its own profile. Scheduling, dedup,
//! retention, generation checks, and cache writes are shared by all of them,
//! so switching providers changes the destination and the field spelling but
//! never the statistics, the retention rules, or the Peer record counts.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;

use sqlx::SqlitePool;

use crate::geo::{self, GeoConfig, GeoLookup, GeoProvider, GeoSelection};
use crate::http::AppState;

/// Accounting for one background pass. Counts are per canonical IP, because
/// that is the unit the cache and any future provider request uses; the Peer
/// record counts a reader sees are never derived from these numbers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackfillSummary {
    /// Lookups this pass processed, whatever their outcome. It is the sum of
    /// `resolved`, `no_country`, `failed`, and `rate_limited`.
    pub attempted: u64,
    pub resolved: u64,
    pub no_country: u64,
    /// Lookups that produced no usable result at all: an unloaded or
    /// unreadable database, a transport failure, a bounded timeout, a
    /// non-success status, or a malformed document.
    pub failed: u64,
    /// Lookups the external provider refused because this Server is being
    /// rate limited. They are counted as attempted but never recorded against
    /// the address: no authoritative result exists, the bounded provider-wide
    /// backoff governs the retry, and the address stays pending.
    pub rate_limited: u64,
    /// Results discarded because the selection changed while they were in
    /// flight. They must never be written as the current configuration's data.
    pub discarded: u64,
}

impl BackfillSummary {
    /// Whether the pass wrote anything to the retained cache.
    pub fn changed(self) -> bool {
        self.resolved + self.no_country + self.failed > 0
    }
}

/// Addresses referenced by a current Peer that need a country lookup now:
/// never attempted, previously attempted without a country result after the
/// bounded retry age, or holding a country result past its expiry.
///
/// Addresses are canonical public literals; ineligible or malformed values
/// recorded outside the trust boundary are skipped rather than resolved.
pub async fn pending_addresses(
    pool: &SqlitePool,
    provider: GeoProvider,
    now: &str,
    limit: Option<usize>,
) -> Result<Vec<IpAddr>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>)>(
        "SELECT DISTINCT peers.remote_ip, cache.country_code, cache.expires_at, cache.last_attempt_at FROM current_node_peers peers LEFT JOIN geo_location_cache cache ON cache.provider = ? AND cache.canonical_ip = peers.remote_ip WHERE peers.remote_ip IS NOT NULL ORDER BY peers.remote_ip ASC",
    )
    .bind(provider.as_str())
    .fetch_all(pool)
    .await?;
    let retry_before = geo::cache_attempt_cutoff(now);
    let bound = limit.unwrap_or(usize::MAX);
    let mut pending = BTreeSet::new();
    // Rows arrive ordered by address, so the bound is reached after the
    // lowest addresses and a later pass continues from there.
    for (remote_ip, country_code, expires_at, last_attempt_at) in rows {
        if pending.len() >= bound {
            break;
        }
        let Some(canonical) = geo::GeoLoader::canonical_public_ip(&remote_ip) else {
            continue;
        };
        // A retained country result is refreshed once it expired; without a
        // retained country the address needs a lookup. Both cases are
        // retried only after the bounded attempt backoff, so a large Peer set
        // or an unreadable database can never turn into unbounded work.
        let needs_lookup = match (country_code.as_deref(), expires_at.as_deref()) {
            (Some(_), Some(expires_at)) => expires_at <= now,
            (Some(_), None) => true,
            (None, _) => true,
        };
        let after_backoff = last_attempt_at
            .as_deref()
            .is_none_or(|last_attempt_at| last_attempt_at <= retry_before.as_str());
        if needs_lookup && after_backoff {
            if let Ok(ip) = canonical.parse::<IpAddr>() {
                pending.insert(ip);
            }
        }
    }
    Ok(pending.into_iter().collect())
}

/// Run one bounded background pass for the selected provider. Disabled
/// schedules no work at all.
pub async fn run_pass(state: &AppState, now: &str) -> Result<BackfillSummary, sqlx::Error> {
    let config = state.geo_config();
    match config.provider {
        GeoProvider::Disabled => Ok(BackfillSummary::default()),
        GeoProvider::LocalMmdb => run_local_pass(state, &config, now).await,
        // Both external providers reuse one execution path: the provider only
        // changes which fixed profile the shared outbound client carries.
        GeoProvider::Ipinfo | GeoProvider::GeoJs => run_external_pass(state, &config, now).await,
    }
}

/// Apply one lookup outcome to the pass accounting and, unless the provider
/// refused the request, to the retained cache.
async fn record_outcome(
    state: &AppState,
    config: &GeoConfig,
    summary: &mut BackfillSummary,
    ip: IpAddr,
    outcome: GeoLookup,
    now: &str,
) -> Result<(), sqlx::Error> {
    summary.attempted += 1;
    match outcome {
        GeoLookup::Country(_) => summary.resolved += 1,
        GeoLookup::NoCountry => summary.no_country += 1,
        GeoLookup::Unavailable => summary.failed += 1,
        GeoLookup::RateLimited => {
            summary.rate_limited += 1;
            // Deliberately not recorded: the provider produced no result, so
            // the address is still pending and the bounded provider-wide
            // backoff, not a per-address attempt, decides when it is retried.
            return Ok(());
        }
    }
    if !record_lookup(state, config, ip, outcome, now).await? {
        summary.discarded += 1;
    }
    Ok(())
}

/// The addresses one pass may schedule: the whole queue is bounded by the
/// batch limit, and every provider reads the same IP-deduplicated queue.
async fn scheduled_addresses(
    state: &AppState,
    config: &GeoConfig,
    now: &str,
) -> Result<Vec<IpAddr>, sqlx::Error> {
    pending_addresses(
        state.db().pool(),
        config.provider,
        now,
        Some(geo::MAX_BACKFILL_BATCH),
    )
    .await
}

/// Drive one bounded, IP-deduplicated lookup set. Scheduling, concurrency,
/// accounting, generation checks, and cache writes are identical for both
/// providers; only the per-address resolver and its concurrency bound differ.
async fn run_bounded_pass<F, Fut>(
    state: &AppState,
    config: &GeoConfig,
    now: &str,
    concurrency: usize,
    addresses: Vec<IpAddr>,
    lookup: F,
) -> Result<BackfillSummary, sqlx::Error>
where
    F: Fn(IpAddr) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = GeoLookup> + Send,
{
    let mut summary = BackfillSummary::default();
    if addresses.is_empty() {
        return Ok(summary);
    }
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut lookups = tokio::task::JoinSet::new();
    for ip in addresses {
        let lookup = lookup.clone();
        let permit = Arc::clone(&semaphore);
        lookups.spawn(async move {
            let _permit = permit
                .acquire_owned()
                .await
                .expect("Geo backfill semaphore is never closed");
            (ip, lookup(ip).await)
        });
    }
    while let Some(joined) = lookups.join_next().await {
        let Ok((ip, outcome)) = joined else {
            continue;
        };
        record_outcome(state, config, &mut summary, ip, outcome, now).await?;
    }
    Ok(summary)
}

/// The local-database pass: a synchronous MMDB read per address, bounded by
/// the local concurrency limit.
async fn run_local_pass(
    state: &AppState,
    config: &GeoConfig,
    now: &str,
) -> Result<BackfillSummary, sqlx::Error> {
    let addresses = scheduled_addresses(state, config, now).await?;
    let loader = Arc::clone(state.geo());
    run_bounded_pass(
        state,
        config,
        now,
        geo::MAX_BACKFILL_CONCURRENCY,
        addresses,
        move |ip| {
            let loader = Arc::clone(&loader);
            async move {
                // The MMDB read is synchronous and must not occupy the async
                // runtime: a slow or replaced database cannot delay report
                // ingestion, which writes on other connections.
                tokio::task::spawn_blocking(move || loader.resolve(&ip))
                    .await
                    .unwrap_or(GeoLookup::Unavailable)
            }
        },
    )
    .await
}

/// The external pass, shared by every External Geo Provider. It adds exactly
/// two things to the shared scheduling: a bounded provider-wide rate-limit
/// backoff, and the outbound concurrency bound. Neither can delay report
/// ingestion, because both live entirely inside this background task.
///
/// The selected provider's own client is used, so its destination, its
/// country field, its bounded backoff, and its latest-outcome flag are the
/// only ones observed. A provider that is not selected performs no work at
/// all, which is also what makes Disabled safe: nothing falls back to the
/// other external provider.
async fn run_external_pass(
    state: &AppState,
    config: &GeoConfig,
    now: &str,
) -> Result<BackfillSummary, sqlx::Error> {
    let Some(client) = state.external_geo(config.provider) else {
        // Unreachable by construction: only external providers are routed
        // here. Refusing to send is the safe direction if that ever changes.
        return Ok(BackfillSummary::default());
    };
    let client = Arc::clone(client);
    // A recently rate-limited provider is left alone until its bounded window
    // passes instead of being asked again on every tick.
    if client.throttled_until(now).is_some() {
        return Ok(BackfillSummary::default());
    }
    let addresses = scheduled_addresses(state, config, now).await?;
    let concurrency = client.max_concurrency();
    let clock: Arc<str> = Arc::from(now);
    let lookup_clock = Arc::clone(&clock);
    run_bounded_pass(state, config, &clock, concurrency, addresses, move |ip| {
        let client = Arc::clone(&client);
        let clock = Arc::clone(&lookup_clock);
        async move { client.resolve(ip, &clock).await }
    })
    .await
}

/// Write one lookup result for the selection that scheduled it. The durable
/// selection and the in-process generation are both revalidated inside the
/// write transaction, so a task that started under an older configuration can
/// never contribute to the current one.
pub(crate) async fn record_lookup(
    state: &AppState,
    config: &GeoConfig,
    ip: IpAddr,
    outcome: GeoLookup,
    now: &str,
) -> Result<bool, sqlx::Error> {
    // A rate-limited lookup is not a result: no authoritative answer exists,
    // so nothing may be recorded against the address. The caller leaves it
    // pending and the bounded provider-wide backoff owns the retry. Guarding
    // here keeps that rule true even if a future caller routes it in.
    if outcome == GeoLookup::RateLimited {
        return Ok(false);
    }
    let current = state.geo_config();
    if current.provider != config.provider || current.generation != config.generation {
        return Ok(false);
    }
    let expected = GeoSelection {
        provider: config.provider,
        generation: config.generation,
    };
    let canonical_ip = ip.to_string();
    let mut tx = state.db().pool().begin().await?;
    if geo::read_provider_selection(&mut *tx).await? != Some(expected) {
        let _ = tx.rollback().await;
        return Ok(false);
    }
    match outcome {
        GeoLookup::Country(country_code) => {
            let existing_created_at: Option<String> = sqlx::query_scalar(
                "SELECT created_at FROM geo_location_cache WHERE provider = ? AND canonical_ip = ?",
            )
            .bind(config.provider.as_str())
            .bind(&canonical_ip)
            .fetch_optional(&mut *tx)
            .await?
            .flatten();
            let (created_at, expires_at) =
                geo::cache_refresh_window(existing_created_at.as_deref(), now);
            sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES (?, ?, ?, 'current', ?, ?, ?, ?, ?) ON CONFLICT(provider, canonical_ip) DO UPDATE SET country_code=excluded.country_code, state='current', created_at=excluded.created_at, last_attempt_at=excluded.last_attempt_at, last_success_at=excluded.last_success_at, last_referenced_at=excluded.last_referenced_at, expires_at=excluded.expires_at")
                .bind(config.provider.as_str())
                .bind(&canonical_ip)
                .bind(country_code)
                .bind(created_at)
                .bind(now)
                .bind(now)
                .bind(now)
                .bind(expires_at)
                .execute(&mut *tx)
                .await?;
        }
        GeoLookup::NoCountry | GeoLookup::Unavailable => {
            // An attempt without a country result records itself only. Any
            // retained country keeps its country, birth time, success time,
            // and expiry, so a failure never refreshes last-good to Current.
            let state_value = match outcome {
                GeoLookup::NoCountry => "no_country",
                _ => "failed",
            };
            sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES (?, ?, NULL, ?, NULL, ?, NULL, ?, NULL) ON CONFLICT(provider, canonical_ip) DO UPDATE SET state=excluded.state, last_attempt_at=excluded.last_attempt_at, last_referenced_at=excluded.last_referenced_at")
                .bind(config.provider.as_str())
                .bind(&canonical_ip)
                .bind(state_value)
                .bind(now)
                .bind(now)
                .execute(&mut *tx)
                .await?;
        }
        // Already refused above, before this transaction was opened. The arm
        // is explicit so a rate-limited result can never fall into a write,
        // and so a new outcome variant cannot silently become one.
        GeoLookup::RateLimited => return Ok(false),
    }
    tx.commit().await?;
    Ok(true)
}

/// Run the background resolution loop until the Server shuts down. The loop
/// is woken by a new Peer reference and by a provider change, and otherwise
/// runs on a fixed bounded cadence.
pub async fn run_worker(state: AppState) {
    let wake = state.geo_backfill_wake();
    let mut tick = tokio::time::interval(geo::BACKFILL_INTERVAL);
    loop {
        if state.is_shutting_down() {
            break;
        }
        tokio::select! {
            _ = state.shutdown_signal() => break,
            _ = tick.tick() => {}
            _ = wake.notified() => {}
        }
        if state.is_shutting_down() {
            break;
        }
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        match run_pass(&state, &now).await {
            Ok(summary) if summary.changed() => {
                // A completed background batch invalidates the country list
                // through the existing realtime mechanism; the hub supplies
                // the monotonic event revision.
                let revision = state.geo_config().generation;
                state
                    .admin_realtime()
                    .publish("geo", None::<String>, revision);
                state
                    .public_realtime()
                    .publish("geo", None::<String>, revision);
            }
            Ok(_) => {}
            Err(error) => eprintln!(
                "geo backfill deferred: {}",
                crate::redaction::redact_sensitive(&error.to_string())
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo_external::stub::{StubReply, StubServer};
    use tempfile::tempdir;

    const NOW: &str = "2026-08-12T10:00:00Z";
    /// One address the bundled GeoIP2 Country test fixture resolves to SE.
    const RESOLVABLE: &str = "89.160.20.112";

    fn write_mmdb(path: &std::path::Path) {
        crate::geo::write_test_database(path);
    }

    /// A real Server database with a real GeoIP2 Country fixture, the given
    /// provider already selected and persisted, and its MMDB loaded unless
    /// the test needs a database that cannot be read.
    async fn backfill_state(
        dir: &tempfile::TempDir,
        with_database: bool,
        provider: GeoProvider,
    ) -> AppState {
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let mmdb_path = dir.path().join("GeoIP2-Country-Test.mmdb");
        if with_database {
            write_mmdb(&mmdb_path);
        }
        let loader = Arc::new(crate::geo::GeoLoader::new(Some(mmdb_path)));
        if with_database {
            assert!(loader.reload());
        }
        let state = AppState::new(database, None, auth)
            .with_geo_loader(loader)
            .with_geo_provider(GeoSelection {
                provider,
                generation: 1,
            });
        crate::geo::write_provider_selection(
            state.db().pool(),
            GeoSelection {
                provider,
                generation: 1,
            },
            NOW,
        )
        .await
        .unwrap();
        state
    }

    async fn seed_node(state: &AppState, node_id: &str) {
        sqlx::query("INSERT OR IGNORE INTO agents (agent_id, agent_epoch, last_received_at, created_at, updated_at) VALUES ('agent-geo-backfill', 1, ?, ?, ?)")
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT OR IGNORE INTO networks (network_key, display_name, genesis_hash, chain_id, p2p_network_id, address_hrp, created_at, updated_at) VALUES ('geo-net', 'Geo Network', '0xgenesis', 1, 1, 'lat', ?, ?)")
            .bind(NOW)
            .bind(NOW)
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, 'agent-geo-backfill', 'geo-net', ?, 'ws://127.0.0.1:1', 'active', 'public', 1, ?, ?)")
            .bind(node_id)
            .bind(node_id)
            .bind(NOW)
            .bind(NOW)
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    async fn insert_peer(state: &AppState, node_id: &str, peer_id: &str, remote_ip: &str) {
        sqlx::query("INSERT INTO current_node_peers (node_id, peer_id, remote_ip, direction, trusted, static_peer, consensus_peer, updated_at) VALUES (?, ?, ?, 'inbound', 0, 0, 0, ?)")
            .bind(node_id)
            .bind(peer_id)
            .bind(remote_ip)
            .bind(NOW)
            .execute(state.db().pool())
            .await
            .unwrap();
    }

    type CacheRow = (
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    );

    async fn cache_row(state: &AppState, ip: &str) -> Option<CacheRow> {
        sqlx::query_as(
            "SELECT country_code, state, created_at, expires_at, last_success_at, last_attempt_at FROM geo_location_cache WHERE provider = 'local_mmdb' AND canonical_ip = ?",
        )
        .bind(ip)
        .fetch_optional(state.db().pool())
        .await
        .unwrap()
    }

    async fn cache_count(state: &AppState) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM geo_location_cache")
            .fetch_one(state.db().pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn resolves_missing_addresses_once_per_canonical_ip() {
        let dir = tempdir().unwrap();
        let state = backfill_state(&dir, true, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        seed_node(&state, "geo-node-b").await;
        // The same address observed by two Nodes and twice on one of them is
        // exactly one lookup.
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        insert_peer(&state, "geo-node-a", "p2", RESOLVABLE).await;
        insert_peer(&state, "geo-node-b", "p3", RESOLVABLE).await;
        // A documentation address is refused by the trust boundary and no
        // lookup is scheduled for it.
        insert_peer(&state, "geo-node-a", "p4", "203.0.113.9").await;

        let summary = run_pass(&state, NOW).await.unwrap();
        assert_eq!(
            summary,
            BackfillSummary {
                attempted: 1,
                resolved: 1,
                no_country: 0,
                failed: 0,
                rate_limited: 0,
                discarded: 0,
            }
        );
        let row = cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.0.as_deref(), Some("SE"));
        assert_eq!(row.1, "current");
        assert_eq!(row.2.as_deref(), Some(NOW));
        assert_eq!(row.3.as_deref(), Some("2026-08-13T10:00:00Z"));
        assert_eq!(row.4.as_deref(), Some(NOW));
        assert_eq!(row.5, NOW);
        // One retained row per address, regardless of how many Peer records
        // or Nodes referenced it.
        assert_eq!(cache_count(&state).await, 1);

        // A fresh result is a cache hit and schedules nothing.
        assert_eq!(
            run_pass(&state, NOW).await.unwrap(),
            BackfillSummary::default()
        );
        let pending = pending_addresses(state.db().pool(), GeoProvider::LocalMmdb, NOW, None)
            .await
            .unwrap();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn expired_results_are_refreshed_inside_the_hard_boundary() {
        let dir = tempdir().unwrap();
        let state = backfill_state(&dir, true, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        assert_eq!(run_pass(&state, NOW).await.unwrap().resolved, 1);

        // 25 hours later the 24-hour result is expired: it stays usable as
        // last-good, and the pass refreshes it while keeping its birth time
        // inside the 30-day retention boundary.
        let later = "2026-08-13T11:00:00Z";
        assert_eq!(run_pass(&state, later).await.unwrap().resolved, 1);
        let row = cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.2.as_deref(), Some(NOW));
        assert_eq!(row.3.as_deref(), Some("2026-08-14T11:00:00Z"));
        assert_eq!(row.4.as_deref(), Some(later));

        // Beyond the hard boundary the birth time is rebuilt instead of
        // extending an unretainable row.
        let rebuild = "2026-09-11T11:00:00Z";
        assert_eq!(run_pass(&state, rebuild).await.unwrap().resolved, 1);
        let row = cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.2.as_deref(), Some(rebuild));
        assert_eq!(row.3.as_deref(), Some("2026-09-12T11:00:00Z"));
    }

    #[tokio::test]
    async fn a_failed_lookup_records_only_the_attempt_and_keeps_last_good() {
        let dir = tempdir().unwrap();
        let state = backfill_state(&dir, false, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        // A retained country result that expired one hour before the pass.
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('local_mmdb', ?, 'SE', 'current', '2026-08-11T09:00:00Z', '2026-08-11T09:00:00Z', '2026-08-11T09:00:00Z', '2026-08-11T09:00:00Z', '2026-08-12T09:00:00Z')")
            .bind(RESOLVABLE)
            .execute(state.db().pool())
            .await
            .unwrap();

        let summary = run_pass(&state, NOW).await.unwrap();
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.resolved, 0);
        let row = cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.0.as_deref(), Some("SE"), "last-good is never erased");
        assert_eq!(row.1, "failed");
        assert_eq!(
            row.2.as_deref(),
            Some("2026-08-11T09:00:00Z"),
            "a failure never refreshes a retained result to Current"
        );
        assert_eq!(row.3.as_deref(), Some("2026-08-12T09:00:00Z"));
        assert_eq!(row.4.as_deref(), Some("2026-08-11T09:00:00Z"));
        assert_eq!(row.5, NOW);

        // The failed attempt is not retried on every pass.
        assert_eq!(
            run_pass(&state, "2026-08-12T10:30:00Z").await.unwrap(),
            BackfillSummary::default()
        );

        // Once the database can be read again, the retry resolves it.
        write_mmdb(&dir.path().join("GeoIP2-Country-Test.mmdb"));
        assert!(state.geo().reload());
        let retry = run_pass(&state, "2026-08-12T11:30:00Z").await.unwrap();
        assert_eq!(retry.resolved, 1);
        let row = cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.1, "current");
        assert_eq!(row.3.as_deref(), Some("2026-08-13T11:30:00Z"));
    }

    #[tokio::test]
    async fn a_no_country_result_is_retained_as_an_attempt() {
        let dir = tempdir().unwrap();
        let state = backfill_state(&dir, true, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        // A public address the fixture has no country for.
        insert_peer(&state, "geo-node-a", "p1", "1.1.1.1").await;

        let summary = run_pass(&state, NOW).await.unwrap();
        assert_eq!(summary.no_country, 1);
        let row = cache_row(&state, "1.1.1.1").await.unwrap();
        assert!(row.0.is_none());
        assert_eq!(row.1, "no_country");
        assert!(row.2.is_none());
        assert!(row.3.is_none());
        assert_eq!(row.5, NOW);
        // The recorded attempt is retried only after the bounded backoff.
        assert_eq!(
            run_pass(&state, NOW).await.unwrap(),
            BackfillSummary::default()
        );
        assert_eq!(
            run_pass(&state, "2026-08-12T11:30:00Z")
                .await
                .unwrap()
                .no_country,
            1
        );
    }

    #[tokio::test]
    async fn disabled_schedules_no_lookups_at_all() {
        let dir = tempdir().unwrap();
        let state = backfill_state(&dir, true, GeoProvider::Disabled).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;

        assert_eq!(
            run_pass(&state, NOW).await.unwrap(),
            BackfillSummary::default()
        );
        assert_eq!(cache_count(&state).await, 0);

        // Selecting Local MMDB is what starts work; the generation advances
        // with the durable selection.
        state.apply_geo_provider(GeoSelection {
            provider: GeoProvider::LocalMmdb,
            generation: 2,
        });
        crate::geo::write_provider_selection(
            state.db().pool(),
            GeoSelection {
                provider: GeoProvider::LocalMmdb,
                generation: 2,
            },
            NOW,
        )
        .await
        .unwrap();
        assert_eq!(run_pass(&state, NOW).await.unwrap().resolved, 1);
    }

    #[tokio::test]
    async fn results_from_an_older_selection_are_never_written() {
        let dir = tempdir().unwrap();
        let state = backfill_state(&dir, true, GeoProvider::LocalMmdb).await;
        let scheduled = state.geo_config();
        let ip: IpAddr = RESOLVABLE.parse().unwrap();

        // A durable change that landed while a lookup was in flight is
        // already enough to discard the result, even before the process-local
        // view catches up.
        crate::geo::write_provider_selection(
            state.db().pool(),
            GeoSelection {
                provider: GeoProvider::LocalMmdb,
                generation: 2,
            },
            NOW,
        )
        .await
        .unwrap();
        assert!(
            !record_lookup(
                &state,
                &scheduled,
                ip,
                GeoLookup::Country("SE".to_owned()),
                NOW
            )
            .await
            .unwrap()
        );
        assert_eq!(cache_count(&state).await, 0);

        // The process-local selection alone is not enough either: once the
        // provider is disabled, no older result may be written.
        let scheduled = GeoConfig {
            provider: GeoProvider::LocalMmdb,
            generation: 2,
            mmdb_path: state.geo_config().mmdb_path,
        };
        state.apply_geo_provider(GeoSelection {
            provider: GeoProvider::Disabled,
            generation: 3,
        });
        assert!(
            !record_lookup(
                &state,
                &scheduled,
                ip,
                GeoLookup::Country("SE".to_owned()),
                NOW
            )
            .await
            .unwrap()
        );
        assert_eq!(cache_count(&state).await, 0);

        // The current selection writes normally.
        state.apply_geo_provider(GeoSelection {
            provider: GeoProvider::LocalMmdb,
            generation: 4,
        });
        crate::geo::write_provider_selection(
            state.db().pool(),
            GeoSelection {
                provider: GeoProvider::LocalMmdb,
                generation: 4,
            },
            NOW,
        )
        .await
        .unwrap();
        assert!(
            record_lookup(
                &state,
                &state.geo_config(),
                ip,
                GeoLookup::Country("SE".to_owned()),
                NOW
            )
            .await
            .unwrap()
        );
        assert_eq!(cache_count(&state).await, 1);
    }

    #[tokio::test]
    async fn cleanup_drops_retired_providers_and_unreferenced_addresses() {
        let dir = tempdir().unwrap();
        let state = backfill_state(&dir, true, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        for (provider, ip) in [
            ("local_mmdb", RESOLVABLE),
            ("disabled", RESOLVABLE),
            ("local_mmdb", "1.1.1.1"),
            ("disabled", "1.1.1.1"),
        ] {
            sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES (?, ?, 'US', 'current', ?, ?, ?, ?, ?)")
                .bind(provider)
                .bind(ip)
                .bind(NOW)
                .bind(NOW)
                .bind(NOW)
                .bind(NOW)
                .bind(crate::geo::cache_expiry(NOW))
                .execute(state.db().pool())
                .await
                .unwrap();
        }

        crate::geo::cleanup_cache(state.db().pool(), NOW, GeoProvider::LocalMmdb)
            .await
            .unwrap();
        let remaining: Vec<(String, String)> = sqlx::query_as(
            "SELECT provider, canonical_ip FROM geo_location_cache ORDER BY canonical_ip",
        )
        .fetch_all(state.db().pool())
        .await
        .unwrap();
        assert_eq!(
            remaining,
            vec![("local_mmdb".to_owned(), RESOLVABLE.to_owned())],
            "results of a retired provider and addresses without a current Peer reference are removed"
        );

        // With no current Peer reference left, the retained address goes too.
        sqlx::query("DELETE FROM current_node_peers")
            .execute(state.db().pool())
            .await
            .unwrap();
        crate::geo::cleanup_cache(state.db().pool(), NOW, GeoProvider::LocalMmdb)
            .await
            .unwrap();
        assert_eq!(cache_count(&state).await, 0);
    }

    /// Regression (issue #132 review): cleanup must never delete a retained
    /// country result that is still last-good. Expiry marks Stale; only the
    /// hard retention boundary, the current-reference rule, and the size
    /// bound remove anything. Before this rule, the 60-second maintenance
    /// tick deleted the very row the backfill had just recorded as failed.
    #[tokio::test]
    async fn cleanup_never_deletes_a_retained_last_good_country() {
        let dir = tempdir().unwrap();
        let state = backfill_state(&dir, true, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;

        // A country result whose 24-hour TTL is long past but whose birth
        // time is still inside the 30-day hard boundary: exactly the
        // retained last-good state the projections serve as Stale.
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('local_mmdb', ?, 'SE', 'failed', '2026-08-01T09:00:00Z', '2026-08-12T09:00:00Z', '2026-08-01T09:00:00Z', '2026-08-12T09:00:00Z', '2026-08-02T09:00:00Z')")
            .bind(RESOLVABLE)
            .execute(state.db().pool())
            .await
            .unwrap();
        assert_eq!(
            crate::geo::cleanup_cache(state.db().pool(), NOW, GeoProvider::LocalMmdb)
                .await
                .unwrap(),
            0
        );
        let row = cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.0.as_deref(), Some("SE"));
        assert_eq!(row.1, "failed");

        // Past the hard boundary the row is genuinely unretainable.
        sqlx::query("UPDATE geo_location_cache SET created_at = '2026-06-01T09:00:00Z' WHERE canonical_ip = ?")
            .bind(RESOLVABLE)
            .execute(state.db().pool())
            .await
            .unwrap();
        assert_eq!(
            crate::geo::cleanup_cache(state.db().pool(), NOW, GeoProvider::LocalMmdb)
                .await
                .unwrap(),
            1
        );
        assert_eq!(cache_count(&state).await, 0);
    }

    /// Regression (issue #132 review): the reported backlog is the whole
    /// queue, not the bounded batch one pass may schedule.
    #[tokio::test]
    async fn the_reported_backlog_is_not_capped_by_the_batch_bound() {
        let dir = tempdir().unwrap();
        let state = backfill_state(&dir, true, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        for index in 0..(geo::MAX_BACKFILL_BATCH + 7) {
            let ip = format!("8.8.{}.{}", index / 256 + 1, index % 256);
            insert_peer(&state, "geo-node-a", &format!("p{index}"), &ip).await;
        }

        let full = pending_addresses(state.db().pool(), GeoProvider::LocalMmdb, NOW, None)
            .await
            .unwrap();
        assert_eq!(full.len(), geo::MAX_BACKFILL_BATCH + 7);
        let bounded = pending_addresses(
            state.db().pool(),
            GeoProvider::LocalMmdb,
            NOW,
            Some(geo::MAX_BACKFILL_BATCH),
        )
        .await
        .unwrap();
        assert_eq!(bounded.len(), geo::MAX_BACKFILL_BATCH);
        // One pass stays bounded and the next one continues the queue.
        assert_eq!(
            run_pass(&state, NOW).await.unwrap().attempted as usize,
            geo::MAX_BACKFILL_BATCH
        );
        let remaining = pending_addresses(state.db().pool(), GeoProvider::LocalMmdb, NOW, None)
            .await
            .unwrap();
        assert_eq!(remaining.len(), 7);
    }

    /// A restart reuses both the durable provider selection and the retained
    /// country result: the new process resolves from the existing cache and
    /// schedules no new lookup for a result that is still valid.
    #[tokio::test]
    async fn the_selection_and_retained_results_survive_a_server_restart() {
        let dir = tempdir().unwrap();
        // The pass time comes from the real clock so the retained 24-hour
        // result is still valid after the restart below.
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        {
            let state = backfill_state(&dir, true, GeoProvider::LocalMmdb).await;
            seed_node(&state, "geo-node-a").await;
            insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
            assert_eq!(run_pass(&state, &now).await.unwrap().resolved, 1);
            state.db().close().await;
        }

        // A new Server process over the same state directory.
        let database = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(dir.path().join("server.db")),
        )
        .await
        .unwrap();
        let pepper_path = dir.path().join("pepper");
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let loader = Arc::new(crate::geo::GeoLoader::new(Some(
            dir.path().join("GeoIP2-Country-Test.mmdb"),
        )));
        assert!(loader.reload());
        // Startup resolves the durable selection again instead of guessing.
        let selection = crate::geo::ensure_provider_selection(database.pool(), true)
            .await
            .unwrap();
        assert_eq!(selection.provider, GeoProvider::LocalMmdb);
        assert_eq!(selection.generation, 1);
        let state = AppState::new(database, None, auth)
            .with_geo_loader(loader)
            .with_geo_provider(selection);
        // The MMDB was reloaded while the retained row stayed usable.
        assert!(
            pending_addresses(state.db().pool(), GeoProvider::LocalMmdb, &now, None)
                .await
                .unwrap()
                .is_empty(),
            "a retained result inside its lifetime is not looked up again"
        );
        let row = cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.0.as_deref(), Some("SE"));
        assert_eq!(row.1, "current");
        state.db().close().await;
    }

    /// The background path and the Public projection agree end to end: before
    /// the pass the address has no country, and after it the country list is
    /// served from the real retained cache.
    #[tokio::test]
    async fn a_completed_pass_updates_the_public_country_projection() {
        let dir = tempdir().unwrap();
        let state = backfill_state(&dir, true, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        // A successful Peer Snapshot basis so the projection has a denominator.
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, value_received_at, state_revision, value_revision) VALUES ('agent-geo-backfill', 'node', 'geo-node-a', 'geo-node-a', 'peers', 'ok', ?, ?, ?, ?, 1, 1)")
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .execute(state.db().pool())
            .await
            .unwrap();

        // The projection compares retained expiries against the real clock,
        // so this test injects the pass time from the same clock.
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        let before = crate::http::public::public_country_distribution(
            &state,
            "geo-net",
            &state.geo_status(),
        )
        .await;
        assert_eq!(before.known_country_count, Some(0));
        assert_eq!(before.unknown_country_count, Some(1));

        assert_eq!(run_pass(&state, &now).await.unwrap().resolved, 1);
        let insight = crate::http::public::public_country_distribution(
            &state,
            "geo-net",
            &state.geo_status(),
        )
        .await;
        assert_eq!(insight.known_country_count, Some(1));
        assert_eq!(insight.unknown_country_count, Some(0));
        let countries = insight.countries.unwrap();
        assert_eq!(countries.len(), 1);
        assert_eq!(countries[0].country_code, "SE");
        assert_eq!(countries[0].count, 1);
        assert_eq!(countries[0].stale_count, 0);
    }

    /// One provider's retained row, read the same way the local provider's
    /// row is read so the shared retention rules stay comparable.
    async fn provider_cache_row(
        state: &AppState,
        provider: GeoProvider,
        ip: &str,
    ) -> Option<CacheRow> {
        sqlx::query_as(
            "SELECT country_code, state, created_at, expires_at, last_success_at, last_attempt_at FROM geo_location_cache WHERE provider = ? AND canonical_ip = ?",
        )
        .bind(provider.as_str())
        .bind(ip)
        .fetch_optional(state.db().pool())
        .await
        .unwrap()
    }

    /// An external provider's retained row.
    async fn ipinfo_cache_row(state: &AppState, ip: &str) -> Option<CacheRow> {
        provider_cache_row(state, GeoProvider::Ipinfo, ip).await
    }

    /// The compile-time profile of an implemented external provider. The
    /// tests never invent a destination: they reuse the production profile and
    /// only redirect the client, which is unreachable from configuration.
    fn external_profile(provider: GeoProvider) -> &'static crate::geo_external::ExternalProfile {
        provider
            .external_profile()
            .expect("the provider resolves countries outside this Server")
    }

    /// A Server whose selected provider is an external path, wired to the
    /// deterministic loopback stub instead of that provider's fixed
    /// destination. The provider selection is persisted exactly as the Admin
    /// mutation leaves it, so the background pass reads a real selection.
    async fn external_state(
        dir: &tempfile::TempDir,
        stub: &StubServer,
        provider: GeoProvider,
    ) -> AppState {
        backfill_state(dir, false, provider)
            .await
            .with_external_geo_client(
                provider,
                Arc::new(crate::geo_external::ExternalGeoClient::for_tests(
                    external_profile(provider),
                    stub.base_url(),
                    std::time::Duration::from_secs(2),
                )),
            )
    }

    async fn ipinfo_state(dir: &tempfile::TempDir, stub: &StubServer) -> AppState {
        external_state(dir, stub, GeoProvider::Ipinfo).await
    }

    async fn geojs_state(dir: &tempfile::TempDir, stub: &StubServer) -> AppState {
        external_state(dir, stub, GeoProvider::GeoJs).await
    }

    /// One address shared by three Peer records across two Nodes is one
    /// outbound query, and a refused address is never sent at all.
    #[tokio::test]
    async fn ipinfo_queries_each_address_once_and_never_sends_ineligible_input() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::country("SE")]).await;
        let state = ipinfo_state(&dir, &stub).await;
        seed_node(&state, "geo-node-a").await;
        seed_node(&state, "geo-node-b").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        insert_peer(&state, "geo-node-a", "p2", RESOLVABLE).await;
        insert_peer(&state, "geo-node-b", "p3", RESOLVABLE).await;
        // A documentation address the trust boundary refuses.
        insert_peer(&state, "geo-node-a", "p4", "203.0.113.9").await;

        let summary = run_pass(&state, NOW).await.unwrap();
        assert_eq!(
            summary,
            BackfillSummary {
                attempted: 1,
                resolved: 1,
                no_country: 0,
                failed: 0,
                rate_limited: 0,
                discarded: 0,
            }
        );
        let requests = stub.requests();
        assert_eq!(requests.len(), 1, "one query per canonical address");
        assert_eq!(requests[0].path, format!("/{RESOLVABLE}/json"));
        assert!(requests[0].query.is_empty());
        assert!(!requests[0].authorization);
        let row = ipinfo_cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.0.as_deref(), Some("SE"));
        assert_eq!(row.1, "current");
        assert_eq!(row.4.as_deref(), Some(NOW));
        assert_eq!(cache_count(&state).await, 1);

        // A retained result inside its lifetime schedules nothing, so a
        // repeated Peer snapshot does not repeat the request.
        assert_eq!(
            run_pass(&state, NOW).await.unwrap(),
            BackfillSummary::default()
        );
        assert_eq!(stub.request_count(), 1);
    }

    /// The external path feeds the same Public projection as the local one,
    /// and the projection counts Peer records rather than deduplicated
    /// addresses.
    #[tokio::test]
    async fn ipinfo_completion_updates_the_public_projection_per_peer_record() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::country("SE")]).await;
        let state = ipinfo_state(&dir, &stub).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        insert_peer(&state, "geo-node-a", "p2", RESOLVABLE).await;
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, value_received_at, state_revision, value_revision) VALUES ('agent-geo-backfill', 'node', 'geo-node-a', 'geo-node-a', 'peers', 'ok', ?, ?, ?, ?, 1, 1)")
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .execute(state.db().pool())
            .await
            .unwrap();

        // The projection compares retained expiries against the real clock.
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        let before = crate::http::public::public_country_distribution(
            &state,
            "geo-net",
            &state.geo_status(),
        )
        .await;
        assert_eq!(before.known_country_count, Some(0));
        assert_eq!(before.unknown_country_count, Some(2));
        assert_eq!(
            before.attribution.as_deref(),
            Some(crate::geo_ipinfo::IPINFO_ATTRIBUTION),
            "the external provider owns the Public attribution"
        );

        assert_eq!(run_pass(&state, &now).await.unwrap().resolved, 1);
        let insight = crate::http::public::public_country_distribution(
            &state,
            "geo-net",
            &state.geo_status(),
        )
        .await;
        assert_eq!(insight.state, "current");
        assert_eq!(insight.known_country_count, Some(2));
        assert_eq!(insight.unknown_country_count, Some(0));
        assert_eq!(insight.last_good_at, None, "no database was loaded");
        assert_eq!(insight.database_age_seconds, None);
        let countries = insight.countries.unwrap();
        assert_eq!(countries.len(), 1);
        assert_eq!(countries[0].country_code, "SE");
        assert_eq!(
            countries[0].count, 2,
            "two records sharing one address are two Peer records"
        );
    }

    /// A failed external lookup records only the attempt: the retained
    /// country stays last-good, and a later success is a real refresh.
    #[tokio::test]
    async fn ipinfo_failure_keeps_last_good_and_a_later_success_is_real() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::Status(500), StubReply::country("US")]).await;
        let state = ipinfo_state(&dir, &stub).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        // A retained country result that expired one hour before the pass.
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('ipinfo', ?, 'SE', 'current', '2026-08-11T09:00:00Z', '2026-08-11T09:00:00Z', '2026-08-11T09:00:00Z', '2026-08-11T09:00:00Z', '2026-08-12T09:00:00Z')")
            .bind(RESOLVABLE)
            .execute(state.db().pool())
            .await
            .unwrap();

        let summary = run_pass(&state, NOW).await.unwrap();
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.resolved, 0);
        assert_eq!(summary.rate_limited, 0);
        let row = ipinfo_cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.0.as_deref(), Some("SE"), "last-good is never erased");
        assert_eq!(row.1, "failed");
        assert_eq!(
            row.2.as_deref(),
            Some("2026-08-11T09:00:00Z"),
            "a failure never refreshes a retained result to Current"
        );
        assert_eq!(row.3.as_deref(), Some("2026-08-12T09:00:00Z"));
        assert_eq!(row.5, NOW);

        // The Public and Admin surfaces report the provider's real failure
        // state instead of claiming Current over a failing path, and the
        // retained country stays visible as last-good.
        assert_eq!(state.geo_status().state, "error");
        assert!(state.geo_status().last_error.is_some());
        let insight = crate::http::public::public_country_distribution(
            &state,
            "geo-net",
            &state.geo_status(),
        )
        .await;
        assert_eq!(insight.state, "error");
        assert!(
            insight.error_reason.is_some(),
            "the Public reason stays stable and non-sensitive"
        );

        // The failed attempt is not retried on every pass.
        assert_eq!(
            run_pass(&state, "2026-08-12T10:30:00Z").await.unwrap(),
            BackfillSummary::default()
        );
        assert_eq!(stub.request_count(), 1);

        // Past the bounded attempt backoff the retry is a real provider call.
        assert_eq!(
            run_pass(&state, "2026-08-12T11:30:00Z")
                .await
                .unwrap()
                .resolved,
            1
        );
        let row = ipinfo_cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.0.as_deref(), Some("US"));
        assert_eq!(row.1, "current");
        assert_eq!(row.4.as_deref(), Some("2026-08-12T11:30:00Z"));
        assert_eq!(stub.request_count(), 2);
        // The provider answered again, so the failure state clears.
        assert_eq!(state.geo_status().state, "current");
        assert!(state.geo_status().last_error.is_none());
    }

    /// A rate-limited lookup is never written as an address attempt, even if
    /// a caller hands it to the write seam directly: the address must stay
    /// pending under the bounded provider-wide backoff.
    #[tokio::test]
    async fn ipinfo_rate_limited_is_never_recorded_even_by_a_direct_write() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::Status(429)]).await;
        let state = ipinfo_state(&dir, &stub).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        // The pass observes the real 429 and arms the bounded backoff.
        assert_eq!(run_pass(&state, NOW).await.unwrap().rate_limited, 1);
        // Handing the same outcome to the write seam directly must still not
        // record an attempt against the address.
        assert!(
            !record_lookup(
                &state,
                &state.geo_config(),
                RESOLVABLE.parse().unwrap(),
                GeoLookup::RateLimited,
                NOW
            )
            .await
            .unwrap()
        );
        assert_eq!(cache_count(&state).await, 0);
        assert_eq!(
            pending_addresses(state.db().pool(), GeoProvider::Ipinfo, NOW, None)
                .await
                .unwrap()
                .len(),
            1
        );
        // The reported state describes the outbound path (its clock is the
        // real one, not this test's injected pass time), and the reason is
        // always one of the two stable, non-sensitive strings.
        assert_eq!(state.geo_status().state, "error");
        let reason = state.geo_status().last_error.unwrap();
        assert!(
            reason == crate::geo_ipinfo::IPINFO_THROTTLE_REASON
                || reason == crate::geo_ipinfo::IPINFO_FAILURE_REASON,
            "unexpected reason: {reason}"
        );
    }

    /// A rate limit is not a result: nothing is recorded against the address,
    /// the address stays pending, and one bounded provider-wide window keeps
    /// every later address from being asked.
    #[tokio::test]
    async fn ipinfo_rate_limit_backs_off_without_recording_an_attempt() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::Status(429)]).await;
        let state = ipinfo_state(&dir, &stub).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;

        let summary = run_pass(&state, NOW).await.unwrap();
        assert_eq!(
            summary,
            BackfillSummary {
                attempted: 1,
                resolved: 0,
                no_country: 0,
                failed: 0,
                rate_limited: 1,
                discarded: 0,
            }
        );
        assert_eq!(cache_count(&state).await, 0);
        assert_eq!(stub.request_count(), 1);
        assert_eq!(
            state.geo_status().state,
            "error",
            "a rate-limited provider is not reported as Current"
        );

        // Inside the bounded window no outbound work is scheduled at all.
        assert_eq!(
            run_pass(&state, "2026-08-12T10:00:30Z").await.unwrap(),
            BackfillSummary::default()
        );
        assert_eq!(stub.request_count(), 1);

        // Past the window the address is asked again, and it is still a
        // pending address rather than a failed one.
        assert_eq!(
            run_pass(&state, "2026-08-12T10:01:01Z")
                .await
                .unwrap()
                .rate_limited,
            1
        );
        assert_eq!(stub.request_count(), 2);
        let pending = pending_addresses(
            state.db().pool(),
            GeoProvider::Ipinfo,
            "2026-08-12T10:01:01Z",
            None,
        )
        .await
        .unwrap();
        assert_eq!(pending.len(), 1);
    }

    /// Disabling Geo, or switching provider, takes effect on the outbound
    /// path immediately: no request is sent for a selection that is gone, and
    /// a late result can never be written as the new selection's data.
    #[tokio::test]
    async fn ipinfo_stops_sending_the_moment_the_selection_changes() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::country("SE")]).await;
        let state = ipinfo_state(&dir, &stub).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        let scheduled = state.geo_config();

        state.apply_geo_provider(GeoSelection {
            provider: GeoProvider::Disabled,
            generation: 2,
        });
        crate::geo::write_provider_selection(
            state.db().pool(),
            GeoSelection {
                provider: GeoProvider::Disabled,
                generation: 2,
            },
            NOW,
        )
        .await
        .unwrap();
        assert_eq!(
            run_pass(&state, NOW).await.unwrap(),
            BackfillSummary::default()
        );
        assert_eq!(stub.request_count(), 0, "no outbound work while disabled");
        assert!(
            !record_lookup(
                &state,
                &scheduled,
                RESOLVABLE.parse().unwrap(),
                GeoLookup::Country("SE".to_owned()),
                NOW
            )
            .await
            .unwrap()
        );
        assert_eq!(cache_count(&state).await, 0);

        // Selecting the external provider again starts real work, with the
        // new generation owning the result.
        state.apply_geo_provider(GeoSelection {
            provider: GeoProvider::Ipinfo,
            generation: 3,
        });
        crate::geo::write_provider_selection(
            state.db().pool(),
            GeoSelection {
                provider: GeoProvider::Ipinfo,
                generation: 3,
            },
            NOW,
        )
        .await
        .unwrap();
        assert_eq!(run_pass(&state, NOW).await.unwrap().resolved, 1);
        assert_eq!(stub.request_count(), 1);
        assert_eq!(
            ipinfo_cache_row(&state, RESOLVABLE)
                .await
                .unwrap()
                .0
                .as_deref(),
            Some("SE")
        );
    }

    /// A restart reuses the durable provider selection and the retained
    /// external result, so a Server that already resolved its Peers does not
    /// ask the provider again for a valid country.
    #[tokio::test]
    async fn ipinfo_selection_and_retained_results_survive_a_server_restart() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::country("SE")]).await;
        // The pass time comes from the real clock so the retained 24-hour
        // result is still valid after the restart below.
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        {
            let state = ipinfo_state(&dir, &stub).await;
            seed_node(&state, "geo-node-a").await;
            insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
            assert_eq!(run_pass(&state, &now).await.unwrap().resolved, 1);
            state.db().close().await;
        }

        let database = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(dir.path().join("server.db")),
        )
        .await
        .unwrap();
        let pepper_path = dir.path().join("pepper");
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        // Startup resolves the durable selection again instead of guessing or
        // resetting to Disabled.
        let selection = crate::geo::ensure_provider_selection(database.pool(), false)
            .await
            .unwrap();
        assert_eq!(selection.provider, GeoProvider::Ipinfo);
        assert_eq!(selection.generation, 1);
        let state = AppState::new(database, None, auth)
            .with_external_geo_client(
                GeoProvider::Ipinfo,
                Arc::new(crate::geo_external::ExternalGeoClient::for_tests(
                    external_profile(GeoProvider::Ipinfo),
                    stub.base_url(),
                    std::time::Duration::from_secs(2),
                )),
            )
            .with_geo_provider(selection);
        assert!(
            pending_addresses(state.db().pool(), GeoProvider::Ipinfo, &now, None)
                .await
                .unwrap()
                .is_empty(),
            "a retained result inside its lifetime is not looked up again"
        );
        let row = ipinfo_cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.0.as_deref(), Some("SE"));
        assert_eq!(row.1, "current");
        state.db().close().await;
    }

    /// A Server with both external providers wired to their own loopback
    /// stub, so a provider switch changes which destination is really asked.
    /// The selected provider is GeoJS and persisted as such.
    async fn external_switch_state(
        dir: &tempfile::TempDir,
        geojs_stub: &StubServer,
        ipinfo_stub: &StubServer,
    ) -> AppState {
        backfill_state(dir, false, GeoProvider::GeoJs)
            .await
            .with_external_geo_client(
                GeoProvider::GeoJs,
                Arc::new(crate::geo_external::ExternalGeoClient::for_tests(
                    external_profile(GeoProvider::GeoJs),
                    geojs_stub.base_url(),
                    std::time::Duration::from_secs(2),
                )),
            )
            .with_external_geo_client(
                GeoProvider::Ipinfo,
                Arc::new(crate::geo_external::ExternalGeoClient::for_tests(
                    external_profile(GeoProvider::Ipinfo),
                    ipinfo_stub.base_url(),
                    std::time::Duration::from_secs(2),
                )),
            )
    }

    /// Select `provider` durably and in-process, the way the Owner mutation
    /// leaves the running Server.
    async fn select_provider(state: &AppState, provider: GeoProvider, generation: u64) {
        let selection = GeoSelection {
            provider,
            generation,
        };
        crate::geo::write_provider_selection(state.db().pool(), selection, NOW)
            .await
            .unwrap();
        state.apply_geo_provider(selection);
        crate::geo::cleanup_cache(state.db().pool(), NOW, provider)
            .await
            .unwrap();
    }

    /// GeoJS runs the whole chain the IPinfo slice proved, with the GeoJS
    /// field and path: one outbound query per canonical address, a retained
    /// provider-keyed result, and Public counts that stay per Peer record.
    #[tokio::test]
    async fn geojs_queries_each_address_once_and_feeds_the_public_projection() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::document(
            r#"{"ip":"89.160.20.112","country":"Sweden","country_code":"SE"}"#,
        )])
        .await;
        let state = geojs_state(&dir, &stub).await;
        seed_node(&state, "geo-node-a").await;
        seed_node(&state, "geo-node-b").await;
        // Two Nodes and three Peer records share one address: one query.
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        insert_peer(&state, "geo-node-a", "p2", RESOLVABLE).await;
        insert_peer(&state, "geo-node-b", "p3", RESOLVABLE).await;
        // A documentation address is refused by the trust boundary and never
        // reaches the third party.
        insert_peer(&state, "geo-node-a", "p4", "203.0.113.9").await;
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, value_received_at, state_revision, value_revision) VALUES ('agent-geo-backfill', 'node', 'geo-node-a', 'geo-node-a', 'peers', 'ok', ?, ?, ?, ?, 1, 1)")
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .execute(state.db().pool())
            .await
            .unwrap();

        let summary = run_pass(&state, NOW).await.unwrap();
        assert_eq!(
            summary,
            BackfillSummary {
                attempted: 1,
                resolved: 1,
                no_country: 0,
                failed: 0,
                rate_limited: 0,
                discarded: 0,
            }
        );
        let requests = stub.requests();
        assert_eq!(requests.len(), 1, "one query per canonical address");
        assert_eq!(requests[0].path, format!("/v1/ip/geo/{RESOLVABLE}.json"));
        assert!(requests[0].query.is_empty());
        assert!(!requests[0].authorization);
        let row = provider_cache_row(&state, GeoProvider::GeoJs, RESOLVABLE)
            .await
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("SE"));
        assert_eq!(row.1, "current");
        assert_eq!(row.4.as_deref(), Some(NOW));
        // The retained row is keyed by the GeoJS provider, never shared with
        // the other external provider.
        assert!(ipinfo_cache_row(&state, RESOLVABLE).await.is_none());
        assert_eq!(cache_count(&state).await, 1);

        // A retained result inside its lifetime schedules nothing.
        assert_eq!(
            run_pass(&state, NOW).await.unwrap(),
            BackfillSummary::default()
        );
        assert_eq!(stub.request_count(), 1);

        // The Public projection reads the same retained result, counts Peer
        // records rather than deduplicated addresses, and carries the GeoJS
        // attribution the selected provider owns.
        let insight = crate::http::public::public_country_distribution(
            &state,
            "geo-net",
            &state.geo_status(),
        )
        .await;
        assert_eq!(insight.state, "current");
        assert_eq!(insight.known_country_count, Some(3));
        assert_eq!(insight.unknown_country_count, Some(1));
        let countries = insight.countries.unwrap();
        assert_eq!(countries.len(), 1);
        assert_eq!(countries[0].country_code, "SE");
        assert_eq!(countries[0].count, 3, "Peer records, not unique addresses");
        assert_eq!(
            insight.attribution.as_deref(),
            Some(crate::geo_geojs::GEOJS_ATTRIBUTION)
        );
    }

    /// A GeoJS HTTP failure keeps last-good exactly like the other external
    /// provider, and a later real success refreshes it.
    #[tokio::test]
    async fn geojs_failure_keeps_last_good_and_a_later_success_is_real() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![
            StubReply::Status(500),
            StubReply::document(r#"{"country_code":"US"}"#),
        ])
        .await;
        let state = geojs_state(&dir, &stub).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('geojs', ?, 'SE', 'current', '2026-08-11T09:00:00Z', '2026-08-11T09:00:00Z', '2026-08-11T09:00:00Z', '2026-08-11T09:00:00Z', '2026-08-12T09:00:00Z')")
            .bind(RESOLVABLE)
            .execute(state.db().pool())
            .await
            .unwrap();

        assert_eq!(run_pass(&state, NOW).await.unwrap().failed, 1);
        let row = provider_cache_row(&state, GeoProvider::GeoJs, RESOLVABLE)
            .await
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("SE"), "last-good is never erased");
        assert_eq!(row.1, "failed");
        assert_eq!(
            row.2.as_deref(),
            Some("2026-08-11T09:00:00Z"),
            "a failure never refreshes a retained result to Current"
        );
        assert_eq!(
            state.geo_status().last_error.as_deref(),
            Some(crate::geo_geojs::GEOJS_FAILURE_REASON),
            "the reported reason belongs to the selected provider"
        );

        // The failed attempt is not retried on every pass.
        assert_eq!(
            run_pass(&state, "2026-08-12T10:30:00Z").await.unwrap(),
            BackfillSummary::default()
        );
        assert_eq!(stub.request_count(), 1);

        // Past the bounded attempt backoff the retry is a real provider call.
        let later = "2026-08-12T11:30:00Z";
        assert_eq!(run_pass(&state, later).await.unwrap().resolved, 1);
        let row = provider_cache_row(&state, GeoProvider::GeoJs, RESOLVABLE)
            .await
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("US"));
        assert_eq!(row.1, "current");
        assert_eq!(row.4.as_deref(), Some(later));
        assert_eq!(state.geo_status().state, "current");
        assert!(state.geo_status().last_error.is_none());
        assert_eq!(stub.request_count(), 2);
    }

    /// Switching between the two External Geo Providers, and disabling Geo,
    /// never mixes their data and never asks the provider that is not
    /// selected. A result produced under the previous provider is discarded
    /// even when it arrives after the switch.
    #[tokio::test]
    async fn switching_external_providers_never_mixes_their_results() {
        let dir = tempdir().unwrap();
        let geojs_stub = StubServer::start(vec![StubReply::document(
            r#"{"country":"Sweden","country_code":"SE"}"#,
        )])
        .await;
        let ipinfo_stub = StubServer::start(vec![StubReply::country("US")]).await;
        let state = external_switch_state(&dir, &geojs_stub, &ipinfo_stub).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;

        // GeoJS is the initial selection: only the GeoJS destination is asked.
        let geojs_config = state.geo_config();
        assert_eq!(geojs_config.provider, GeoProvider::GeoJs);
        assert_eq!(run_pass(&state, NOW).await.unwrap().resolved, 1);
        assert_eq!(geojs_stub.request_count(), 1);
        assert_eq!(ipinfo_stub.request_count(), 0);
        assert_eq!(
            provider_cache_row(&state, GeoProvider::GeoJs, RESOLVABLE)
                .await
                .unwrap()
                .0
                .as_deref(),
            Some("SE")
        );

        // Owner switches to IPinfo. The retired provider's rows are dropped
        // immediately, so GeoJS's SE can never be served as IPinfo's result,
        // and the GeoJS destination is not asked again.
        select_provider(&state, GeoProvider::Ipinfo, 2).await;
        assert_eq!(cache_count(&state).await, 0);
        assert_eq!(run_pass(&state, NOW).await.unwrap().resolved, 1);
        assert_eq!(ipinfo_stub.request_count(), 1);
        assert_eq!(
            geojs_stub.request_count(),
            1,
            "no work for a provider that is not selected"
        );
        let row = ipinfo_cache_row(&state, RESOLVABLE).await.unwrap();
        assert_eq!(row.0.as_deref(), Some("US"));
        assert!(
            provider_cache_row(&state, GeoProvider::GeoJs, RESOLVABLE)
                .await
                .is_none()
        );

        // A GeoJS lookup that was already in flight when the Owner switched
        // arrives late: it must not become the new selection's result.
        assert!(
            !record_lookup(
                &state,
                &geojs_config,
                RESOLVABLE.parse().unwrap(),
                GeoLookup::Country("SE".to_owned()),
                NOW
            )
            .await
            .unwrap()
        );
        assert_eq!(cache_count(&state).await, 1);
        assert_eq!(
            ipinfo_cache_row(&state, RESOLVABLE)
                .await
                .unwrap()
                .0
                .as_deref(),
            Some("US"),
            "the late GeoJS result did not overwrite the IPinfo result"
        );

        // Switching back to GeoJS is a real re-selection: the IPinfo row is
        // retired and the GeoJS destination is asked again.
        select_provider(&state, GeoProvider::GeoJs, 3).await;
        assert_eq!(cache_count(&state).await, 0);
        assert_eq!(
            run_pass(&state, "2026-08-12T10:30:00Z")
                .await
                .unwrap()
                .resolved,
            1
        );
        assert_eq!(geojs_stub.request_count(), 2);
        assert_eq!(ipinfo_stub.request_count(), 1);
        assert_eq!(
            provider_cache_row(&state, GeoProvider::GeoJs, RESOLVABLE)
                .await
                .unwrap()
                .0
                .as_deref(),
            Some("SE")
        );

        // Disabled schedules no outbound work at all, and a failing selected
        // provider never triggers an implicit fallback to the other one.
        select_provider(&state, GeoProvider::Disabled, 4).await;
        assert_eq!(
            run_pass(&state, NOW).await.unwrap(),
            BackfillSummary::default()
        );
        assert_eq!(geojs_stub.request_count(), 2);
        assert_eq!(ipinfo_stub.request_count(), 1);

        let failing_geojs = StubServer::start(vec![StubReply::Status(500)]).await;
        let failing_dir = tempdir().unwrap();
        let state = external_switch_state(&failing_dir, &failing_geojs, &ipinfo_stub).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        assert_eq!(state.geo_config().provider, GeoProvider::GeoJs);
        assert_eq!(run_pass(&state, NOW).await.unwrap().failed, 1);
        assert_eq!(failing_geojs.request_count(), 1);
        assert_eq!(
            ipinfo_stub.request_count(),
            1,
            "a failing provider is never replaced by another one"
        );
    }

    /// A restart reuses the durable provider selection and the retained
    /// GeoJS result, so a Server that already resolved its Peers does not ask
    /// the GeoJS endpoint again for a valid country.
    #[tokio::test]
    async fn geojs_selection_and_retained_results_survive_a_server_restart() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::document(
            r#"{"country":"Sweden","country_code":"SE"}"#,
        )])
        .await;
        // The pass time comes from the real clock so the retained 24-hour
        // result is still valid after the restart below.
        let now = crate::auth::format_rfc3339(crate::auth::now_utc());
        {
            let state = geojs_state(&dir, &stub).await;
            seed_node(&state, "geo-node-a").await;
            insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
            assert_eq!(run_pass(&state, &now).await.unwrap().resolved, 1);
            state.db().close().await;
        }

        let database = crate::database::ServerDatabase::open_existing(
            crate::database::ServerDatabaseConfig::new(dir.path().join("server.db")),
        )
        .await
        .unwrap();
        let pepper_path = dir.path().join("pepper");
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        // Startup resolves the durable selection again instead of guessing,
        // resetting to Disabled, or falling back to the other external
        // provider.
        let selection = crate::geo::ensure_provider_selection(database.pool(), true)
            .await
            .unwrap();
        assert_eq!(selection.provider, GeoProvider::GeoJs);
        assert_eq!(selection.generation, 1);
        let state = AppState::new(database, None, auth)
            .with_external_geo_client(
                GeoProvider::GeoJs,
                Arc::new(crate::geo_external::ExternalGeoClient::for_tests(
                    external_profile(GeoProvider::GeoJs),
                    stub.base_url(),
                    std::time::Duration::from_secs(2),
                )),
            )
            .with_geo_provider(selection);
        assert!(
            pending_addresses(state.db().pool(), GeoProvider::GeoJs, &now, None)
                .await
                .unwrap()
                .is_empty(),
            "a retained result inside its lifetime is not looked up again"
        );
        let row = provider_cache_row(&state, GeoProvider::GeoJs, RESOLVABLE)
            .await
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("SE"));
        assert_eq!(row.1, "current");
        assert_eq!(
            stub.request_count(),
            1,
            "restarting does not repeat the outbound query"
        );
        state.db().close().await;
    }
}
