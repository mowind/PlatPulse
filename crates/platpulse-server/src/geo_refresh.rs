//! Owner-triggered global Peer geolocation refresh (issue #136).
//!
//! The Owner-only Settings action re-resolves the country of every Peer
//! address the Server currently references, deliberately bypassing the
//! 24-hour validity of the retained cache. It reuses the one background
//! execution path in `crate::geo_backfill`: the same provider resolver,
//! the same bounded concurrency, the same generation-guarded cache write. A
//! refresh never clears the cache and never reports success merely because a
//! batch was accepted - a run reaches `completed` only after every
//! address in its scope has been resolved and written.
//!
//! Rules this module owns:
//!
//! * Scope is frozen when the run starts: the distinct canonical public
//!   addresses currently referenced by `current_node_peers`, across every
//!   Network and independent of any Home filter. A Peer reference added later
//!   is handled by the normal background pass, so the denominator an Owner
//!   sees cannot move under a running run.
//! * At most one run is active. A repeated request for the same provider and
//!   configuration joins the running run instead of starting a second batch,
//!   and a run for an older configuration is superseded, never merged.
//! * Every lookup is deduplicated by canonical address: one resolution per
//!   address, however many Peer records reference it.
//! * Counters are per address lookup. The number of Peer records those
//!   addresses serve is reported beside them, because the Public country
//!   projection counts Peer records per Node; the two denominators are never
//!   conflated and neither is derived from the other.
//! * A provider change, a rate limit, or a shutdown aborts the run with a
//!   stable reason and stops scheduling further work. The abort wakes the
//!   worker directly, so its in-flight lookups are cancelled at the next await
//!   instead of running to their own timeout, and a late result from the old
//!   selection can not be written as the new selection's data.
//! * While a run is in flight the ordinary background pass yields exactly the
//!   addresses the run owns. Everything else keeps its normal cadence, so a
//!   Peer reference that appears mid-run is resolved by the next pass rather
//!   than delayed until the run ends.
//! * The terminal state lives in this process only. A restart forgets the run
//!   instead of reporting a stale "running" state forever.

use std::collections::{BTreeSet, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sqlx::SqlitePool;

use crate::geo::{GeoConfig, GeoLookup, GeoProvider};
use crate::http::AppState;

/// Minimum spacing between progress invalidations. A large scope produces at
/// most a few events per second while every run still publishes its start and
/// its terminal state, so a connected Settings or Public page follows real
/// progress without one SSE event per address.
pub const PROGRESS_EVENT_INTERVAL: Duration = Duration::from_millis(250);

/// The reason a refresh cannot start at all. The Owner-only surface reports
/// this instead of a fake success.
pub const DISABLED_REASON: &str = "Geo is Disabled, so no Peer address is resolved";

/// The lifecycle of one refresh run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshState {
    Running,
    Completed,
    Aborted,
}

impl RefreshState {
    pub fn as_str(self) -> &'static str {
        match self {
            RefreshState::Running => "running",
            RefreshState::Completed => "completed",
            RefreshState::Aborted => "aborted",
        }
    }
}

/// Why a run stopped before its whole scope was resolved. Every reason has a
/// stable, path-free sentence: the API never forwards a database error, an
/// endpoint, or a provider's own error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshAbort {
    /// The selected provider or its configuration generation changed while
    /// the run was in flight. The remaining addresses belong to the new
    /// configuration and are never resolved under the old one.
    ProviderChanged,
    /// A newer refresh replaced this run before it finished.
    Superseded,
    /// The provider refused a request because this Server is being rate
    /// limited. No authoritative result exists, so the run stops instead of
    /// recording attempts for every remaining address.
    RateLimited,
    /// The Server began shutting down.
    ServerShutdown,
    /// The Server database refused a cache write, so the run cannot claim to
    /// have refreshed anything.
    InternalError,
}

impl RefreshAbort {
    pub fn code(self) -> &'static str {
        match self {
            RefreshAbort::ProviderChanged => "provider_changed",
            RefreshAbort::Superseded => "superseded",
            RefreshAbort::RateLimited => "rate_limited",
            RefreshAbort::ServerShutdown => "server_shutdown",
            RefreshAbort::InternalError => "internal_error",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            RefreshAbort::ProviderChanged => {
                "The Geo provider or its configuration changed, so the refresh stopped early. The remaining addresses will be handled by the current configuration."
            }
            RefreshAbort::Superseded => "A newer refresh replaced this run before it finished.",
            RefreshAbort::RateLimited => {
                "The Geo provider is rate limiting this Server, so the refresh stopped early. The remaining addresses stay pending."
            }
            RefreshAbort::ServerShutdown => "The Server shut down before the refresh finished.",
            RefreshAbort::InternalError => {
                "The Server could not record a refresh result, so the refresh stopped early."
            }
        }
    }
}

/// One refresh run as the Admin surface reads it. Every count is a count of
/// address lookups except `peer_records_in_scope`, which keeps the Public
/// projection's per-Node Peer-record denominator visible next to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeoRefreshSnapshot {
    /// An opaque identifier for this run. It carries no address.
    pub run_id: String,
    pub provider: GeoProvider,
    pub provider_generation: u64,
    pub state: RefreshState,
    pub abort: Option<RefreshAbort>,
    /// Distinct eligible public Peer addresses this run may resolve. One
    /// lookup per address; this is not a count of Peer records.
    pub total_lookups: u64,
    /// Current Peer records that reference those addresses. One address may
    /// serve several records, so this is never derived from `total_lookups`.
    pub peer_records_in_scope: u64,
    /// Lookups that reached an authoritative outcome: resolved, no country,
    /// or failed. A rate-limited request is not a result and is not counted
    /// here, so an aborted run never reports work it did not do.
    pub completed_lookups: u64,
    pub resolved_lookups: u64,
    pub no_country_lookups: u64,
    pub failed_lookups: u64,
    /// Requests the provider refused while rate limiting this Server.
    pub rate_limited_lookups: u64,
    pub started_at: String,
    pub finished_at: Option<String>,
}

impl GeoRefreshSnapshot {
    fn new(
        run_id: String,
        provider: GeoProvider,
        provider_generation: u64,
        total_lookups: u64,
        peer_records_in_scope: u64,
        started_at: String,
    ) -> Self {
        Self {
            run_id,
            provider,
            provider_generation,
            state: RefreshState::Running,
            abort: None,
            total_lookups,
            peer_records_in_scope,
            completed_lookups: 0,
            resolved_lookups: 0,
            no_country_lookups: 0,
            failed_lookups: 0,
            rate_limited_lookups: 0,
            started_at,
            finished_at: None,
        }
    }
}

/// The addresses one refresh covers, and the Peer-record count that keeps the
/// lookup denominator honest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshScope {
    /// Distinct canonical public addresses, in address order.
    pub addresses: Vec<IpAddr>,
    /// Current Peer records that reference one of those addresses.
    pub peer_records: u64,
}

/// Enumerate the refresh scope: every distinct canonical public Peer address
/// currently referenced by `current_node_peers`, across all Networks.
/// Malformed, private, loopback, link-local, and reserved values are skipped
/// by the same trust-boundary rule the resolution path applies, and an address
/// referenced by several records or several Nodes is one entry.
pub async fn refresh_scope(pool: &SqlitePool) -> Result<RefreshScope, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT remote_ip, COUNT(*) FROM current_node_peers WHERE remote_ip IS NOT NULL GROUP BY remote_ip",
    )
    .fetch_all(pool)
    .await?;
    let mut addresses = BTreeSet::new();
    let mut peer_records = 0u64;
    for (remote_ip, count) in rows {
        let Some(canonical) = crate::geo::GeoLoader::canonical_public_ip(&remote_ip) else {
            continue;
        };
        let Ok(ip) = canonical.parse::<IpAddr>() else {
            continue;
        };
        addresses.insert(ip);
        peer_records = peer_records.saturating_add(count.max(0) as u64);
    }
    Ok(RefreshScope {
        addresses: addresses.into_iter().collect(),
        peer_records,
    })
}

/// Why Geo cannot start a refresh in this configuration, or `None` when it
/// can. The one explanation the Admin refusal and the Settings surface both
/// read, so a disabled option can never disagree with the error.
///
/// It is deliberately not named like `GeoConfig::unavailable_reason`: that
/// one answers "why can this provider not be selected", which is never true of
/// Disabled, while this one answers "why can a refresh not run", which is.
pub fn refresh_unavailable_reason(config: &GeoConfig) -> Option<&'static str> {
    if config.provider == GeoProvider::Disabled {
        return Some(DISABLED_REASON);
    }
    config.unavailable_reason(config.provider)
}

/// The live state of one run. The worker holds its own handle, so a run that
/// was superseded or aborted keeps its truthful terminal state even though a
/// newer run is what the registry reports.
#[derive(Debug)]
struct ActiveRun {
    snapshot: GeoRefreshSnapshot,
    /// The addresses this run owns: the frozen scope. The background pass
    /// yields exactly these and keeps resolving anything else, so a Peer
    /// reference that appears mid-run is not delayed and no address is
    /// resolved by two paths at once.
    owned: Arc<BTreeSet<IpAddr>>,
    /// Wakes the worker out of its join wait the instant the run stops being
    /// current. Without it an aborted run would keep its in-flight outbound
    /// requests alive until one of them completed or timed out.
    cancel: Arc<tokio::sync::Notify>,
}

impl ActiveRun {
    fn is_running(&self) -> bool {
        self.snapshot.state == RefreshState::Running
    }

    /// Account one authoritative lookup outcome. Returns whether the run was
    /// still running, so a worker that was aborted externally stops counting.
    fn record(&mut self, outcome: &GeoLookup) -> bool {
        if !self.is_running() {
            return false;
        }
        match outcome {
            GeoLookup::Country(_) => {
                self.snapshot.resolved_lookups += 1;
                self.snapshot.completed_lookups += 1;
            }
            GeoLookup::NoCountry => {
                self.snapshot.no_country_lookups += 1;
                self.snapshot.completed_lookups += 1;
            }
            GeoLookup::Unavailable => {
                self.snapshot.failed_lookups += 1;
                self.snapshot.completed_lookups += 1;
            }
            // A rate limit is not a result: it is counted separately and the
            // worker aborts the run, so it never advances `completed`.
            GeoLookup::RateLimited => {
                self.snapshot.rate_limited_lookups += 1;
            }
        }
        true
    }

    /// Move a running run to its terminal state. A run that is already
    /// terminal keeps its own reason: a provider change that was observed
    /// first is never rewritten as a completion.
    fn finish(&mut self, state: RefreshState, abort: Option<RefreshAbort>, now: &str) -> bool {
        if !self.is_running() || state == RefreshState::Running {
            return false;
        }
        self.snapshot.state = state;
        self.snapshot.abort = abort;
        self.snapshot.finished_at = Some(now.to_owned());
        // The worker may be waiting on an in-flight lookup. Waking it here is
        // what makes an abort prompt instead of delayed by a slow provider.
        // `notify_one` stores a permit when the worker is not waiting yet, so
        // an abort between two iterations can not be missed.
        self.cancel.notify_one();
        true
    }

    fn abort(&mut self, reason: RefreshAbort, now: &str) -> bool {
        self.finish(RefreshState::Aborted, Some(reason), now)
    }
}

#[derive(Debug, Default)]
struct RegistryInner {
    /// The run the Admin surface reports: the newest one, running or terminal.
    active: Option<Arc<Mutex<ActiveRun>>>,
}

/// The process-local registry of the one refresh run. Cloning shares the same
/// registry, which is what lets a spawned worker and an HTTP handler observe
/// each other.
#[derive(Debug, Default, Clone)]
pub struct GeoRefreshRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

impl GeoRefreshRegistry {
    /// The run the Admin surface reports, or `None` when no refresh has been
    /// requested since this process started.
    pub fn snapshot(&self) -> Option<GeoRefreshSnapshot> {
        let inner = self
            .inner
            .lock()
            .expect("Geo refresh registry lock poisoned");
        inner.active.as_ref().map(|run| {
            run.lock()
                .expect("Geo refresh run lock poisoned")
                .snapshot
                .clone()
        })
    }

    /// The addresses a running run for exactly this provider and generation
    /// owns. The background pass excludes them and resolves nothing else
    /// differently, so one address is never scheduled by two paths at once
    /// while a new Peer reference is still handled on the normal cadence.
    pub fn owned_by(&self, provider: GeoProvider, generation: u64) -> BTreeSet<IpAddr> {
        let inner = self
            .inner
            .lock()
            .expect("Geo refresh registry lock poisoned");
        let Some(run) = inner.active.as_ref() else {
            return BTreeSet::new();
        };
        let run = run.lock().expect("Geo refresh run lock poisoned");
        if !run.is_running()
            || run.snapshot.provider != provider
            || run.snapshot.provider_generation != generation
        {
            return BTreeSet::new();
        }
        run.owned.as_ref().clone()
    }

    /// Whether a run for exactly this provider and generation is still
    /// running.
    pub fn is_running_for(&self, provider: GeoProvider, generation: u64) -> bool {
        let inner = self
            .inner
            .lock()
            .expect("Geo refresh registry lock poisoned");
        inner.active.as_ref().is_some_and(|run| {
            let run = run.lock().expect("Geo refresh run lock poisoned");
            run.is_running()
                && run.snapshot.provider == provider
                && run.snapshot.provider_generation == generation
        })
    }

    fn running_for(&self, config: &GeoConfig) -> Option<GeoRefreshSnapshot> {
        let inner = self
            .inner
            .lock()
            .expect("Geo refresh registry lock poisoned");
        inner.active.as_ref().and_then(|run| {
            let run = run.lock().expect("Geo refresh run lock poisoned");
            (run.is_running()
                && run.snapshot.provider == config.provider
                && run.snapshot.provider_generation == config.generation)
                .then(|| run.snapshot.clone())
        })
    }

    /// Install a new run. A running run for the same selection is joined
    /// instead of replaced; a running run for any other selection is marked
    /// superseded before the new one takes its place.
    fn install(
        &self,
        config: &GeoConfig,
        run_id: String,
        scope: &RefreshScope,
        now: &str,
    ) -> Install {
        let mut inner = self
            .inner
            .lock()
            .expect("Geo refresh registry lock poisoned");
        if let Some(active) = inner.active.as_ref() {
            let mut active = active.lock().expect("Geo refresh run lock poisoned");
            if active.is_running()
                && active.snapshot.provider == config.provider
                && active.snapshot.provider_generation == config.generation
            {
                return Install::Joined(active.snapshot.clone());
            }
            active.abort(RefreshAbort::Superseded, now);
        }
        let run = Arc::new(Mutex::new(ActiveRun {
            snapshot: GeoRefreshSnapshot::new(
                run_id,
                config.provider,
                config.generation,
                scope.addresses.len() as u64,
                scope.peer_records,
                now.to_owned(),
            ),
            owned: Arc::new(scope.addresses.iter().copied().collect()),
            cancel: Arc::new(tokio::sync::Notify::new()),
        }));
        inner.active = Some(Arc::clone(&run));
        Install::Installed(run)
    }

    /// Abort a running run with a specific reason. It is how an Owner action
    /// that invalidates the selection makes the Settings surface truthful at
    /// once, without waiting for the worker to observe the change.
    pub fn abort_active(&self, reason: RefreshAbort, now: &str) -> bool {
        let inner = self
            .inner
            .lock()
            .expect("Geo refresh registry lock poisoned");
        inner.active.as_ref().is_some_and(|run| {
            run.lock()
                .expect("Geo refresh run lock poisoned")
                .abort(reason, now)
        })
    }
}

enum Install {
    Joined(GeoRefreshSnapshot),
    Installed(Arc<Mutex<ActiveRun>>),
}

/// A run that has been installed and is waiting to be spawned. Preparing and
/// spawning separately lets the Admin mutation record its Audit Event before
/// any address is resolved, so an unaudited refresh never runs.
pub struct PreparedRefresh {
    run: Arc<Mutex<ActiveRun>>,
    config: GeoConfig,
    addresses: Vec<IpAddr>,
}

impl PreparedRefresh {
    pub fn snapshot(&self) -> GeoRefreshSnapshot {
        self.run
            .lock()
            .expect("Geo refresh run lock poisoned")
            .snapshot
            .clone()
    }

    /// Abandon a prepared run without resolving anything. The caller uses it
    /// when the Admin mutation cannot be recorded.
    pub fn abandon(self, reason: RefreshAbort, now: &str) {
        self.run
            .lock()
            .expect("Geo refresh run lock poisoned")
            .abort(reason, now);
    }
}

/// The outcome of one refresh request: a new run to spawn, or the identical
/// run an earlier request already started.
pub enum RefreshStart {
    Started(PreparedRefresh),
    Joined(GeoRefreshSnapshot),
}

/// Why a refresh request was refused before any run was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartRefreshError {
    /// Geo cannot resolve any address in this configuration.
    Unavailable(&'static str),
    /// The Server database could not enumerate the current Peer references.
    Database,
}

/// Prepare a refresh: enumerate the current scope, then either join the
/// identical running run or install a new one. Nothing is resolved here.
pub async fn begin_refresh(
    state: &AppState,
    run_id: String,
    now: &str,
) -> Result<RefreshStart, StartRefreshError> {
    let config = state.geo_config();
    if let Some(reason) = refresh_unavailable_reason(&config) {
        return Err(StartRefreshError::Unavailable(reason));
    }
    if let Some(snapshot) = state.geo_refresh().running_for(&config) {
        return Ok(RefreshStart::Joined(snapshot));
    }
    let scope = refresh_scope(state.db().pool())
        .await
        .map_err(|_| StartRefreshError::Database)?;
    match state.geo_refresh().install(&config, run_id, &scope, now) {
        Install::Joined(snapshot) => Ok(RefreshStart::Joined(snapshot)),
        Install::Installed(run) => Ok(RefreshStart::Started(PreparedRefresh {
            run,
            config,
            addresses: scope.addresses,
        })),
    }
}

/// Start a prepared refresh. Every address in the frozen scope is resolved
/// through the selected provider's own path, with the same bounded
/// concurrency the background pass uses, and each result is written through
/// the same generation-guarded cache write.
pub fn spawn_refresh(state: &AppState, prepared: PreparedRefresh) {
    let state = state.clone();
    tokio::spawn(run_refresh(state, prepared));
}

fn clock() -> String {
    crate::auth::format_rfc3339(crate::auth::now_utc())
}

/// Publish one Geo invalidation to both the Admin and Public namespaces. The
/// hub owns the monotonic event id; the run's own state is read from REST.
fn publish_geo(state: &AppState) {
    state.admin_realtime().publish("geo", None::<String>, 0);
    state.public_realtime().publish("geo", None::<String>, 0);
}

async fn run_refresh(state: AppState, prepared: PreparedRefresh) {
    let PreparedRefresh {
        run,
        config,
        addresses,
    } = prepared;
    let concurrency = crate::geo_backfill::provider_concurrency(&state, &config).max(1);
    let cancel = {
        let guard = run.lock().expect("Geo refresh run lock poisoned");
        Arc::clone(&guard.cancel)
    };
    let mut queue: VecDeque<IpAddr> = addresses.into();
    let mut in_flight = tokio::task::JoinSet::new();
    let mut last_publish = Instant::now();
    publish_geo(&state);

    loop {
        if !run
            .lock()
            .expect("Geo refresh run lock poisoned")
            .is_running()
        {
            break;
        }
        // Keep a bounded window in flight: the queue may be far larger than
        // the concurrency bound, so work is scheduled only as results arrive.
        while in_flight.len() < concurrency {
            let Some(ip) = queue.pop_front() else {
                break;
            };
            let task_state = state.clone();
            let task_config = config.clone();
            in_flight.spawn(async move {
                let now = clock();
                let outcome =
                    crate::geo_backfill::resolve_address(&task_state, &task_config, ip, &now).await;
                (ip, outcome)
            });
        }
        // Wait for the first of: one lookup finishing, or the run being
        // aborted by a provider change, a newer run, or a shutdown. The
        // abort path wakes this wait directly, so nothing outbound survives
        // the run that scheduled it - it is cancelled at the next await
        // instead of running to its own timeout. A permit stored while the
        // worker was not yet waiting is consumed here and costs one pass
        // through this loop, which then observes the terminal state.
        let joined = if in_flight.is_empty() {
            break;
        } else {
            tokio::select! {
                biased;
                _ = cancel.notified() => None,
                joined = in_flight.join_next() => joined,
            }
        };
        if !run
            .lock()
            .expect("Geo refresh run lock poisoned")
            .is_running()
        {
            break;
        }
        let Some(joined) = joined else {
            continue;
        };
        // A lookup task that did not return produced no outcome for its
        // address, so the run cannot claim the whole scope was resolved.
        let Ok((ip, outcome)) = joined else {
            finish_run(&run, RefreshAbort::InternalError);
            break;
        };
        let current = state.geo_config();
        if current.provider != config.provider || current.generation != config.generation {
            finish_run(&run, RefreshAbort::ProviderChanged);
            break;
        }
        if state.is_shutting_down() {
            finish_run(&run, RefreshAbort::ServerShutdown);
            break;
        }
        if outcome == GeoLookup::RateLimited {
            // The provider produced no authoritative result for this address
            // and is refusing more. Counting it as a failure would claim a
            // result that does not exist, so the run stops here and leaves
            // every remaining address to the bounded provider backoff.
            let mut run_guard = run.lock().expect("Geo refresh run lock poisoned");
            run_guard.record(&outcome);
            run_guard.finish(
                RefreshState::Aborted,
                Some(RefreshAbort::RateLimited),
                &clock(),
            );
            break;
        }
        let now = clock();
        match crate::geo_backfill::record_lookup(&state, &config, ip, outcome.clone(), &now).await {
            Ok(true) => {
                run.lock()
                    .expect("Geo refresh run lock poisoned")
                    .record(&outcome);
            }
            // The durable selection changed between the check above and the
            // write, so this result belongs to a configuration that is no
            // longer selected. It is not counted and the run stops truthfully.
            Ok(false) => {
                finish_run(&run, RefreshAbort::ProviderChanged);
                break;
            }
            Err(_) => {
                finish_run(&run, RefreshAbort::InternalError);
                break;
            }
        }
        if last_publish.elapsed() >= PROGRESS_EVENT_INTERVAL {
            last_publish = Instant::now();
            publish_geo(&state);
        }
    }

    // Dropping the join set aborts every in-flight lookup: no request outlives
    // the run that scheduled it, which is what makes switching providers or
    // disabling Geo stop outbound work immediately.
    in_flight.shutdown().await;
    run.lock().expect("Geo refresh run lock poisoned").finish(
        RefreshState::Completed,
        None,
        &clock(),
    );
    // The background pass yields the whole selection while a run owns it, so
    // the run wakes it again: a Peer reference that arrived mid-run is picked
    // up immediately instead of waiting for the next cadence tick.
    state.notify_geo_backfill();
    publish_geo(&state);
}

fn finish_run(run: &Arc<Mutex<ActiveRun>>, reason: RefreshAbort) {
    run.lock()
        .expect("Geo refresh run lock poisoned")
        .abort(reason, &clock());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::GeoSelection;
    use crate::geo_external::ExternalGeoClient;
    use crate::geo_external::stub::{StubReply, StubServer};
    use std::time::Duration;
    use tempfile::tempdir;

    const NOW: &str = "2026-08-12T10:00:00Z";
    /// One address the bundled GeoIP2 Country test fixture resolves to SE.
    const RESOLVABLE: &str = "89.160.20.112";
    /// A public address the bundled fixture has no country for.
    const NO_COUNTRY: &str = "1.1.1.1";

    async fn refresh_state(
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
            crate::geo::write_test_database(&mmdb_path);
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

    /// A Server whose selected provider is an external path, wired to the
    /// deterministic loopback stub instead of that provider's fixed
    /// destination. The selection is persisted exactly as the Admin mutation
    /// leaves it.
    async fn external_state(
        dir: &tempfile::TempDir,
        stub: &StubServer,
        provider: GeoProvider,
        timeout: Duration,
    ) -> AppState {
        let profile = provider
            .external_profile()
            .expect("the provider resolves countries outside this Server");
        refresh_state(dir, false, provider)
            .await
            .with_external_geo_client(
                provider,
                Arc::new(ExternalGeoClient::for_tests(
                    profile,
                    stub.base_url(),
                    timeout,
                )),
            )
    }

    async fn seed_node(state: &AppState, node_id: &str) {
        sqlx::query("INSERT OR IGNORE INTO agents (agent_id, agent_epoch, last_received_at, created_at, updated_at) VALUES ('agent-geo-refresh', 1, ?, ?, ?)")
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
        sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, display_name, rpc_endpoint, lifecycle, visibility, inventory_revision, first_seen_at, updated_at) VALUES (?, 'agent-geo-refresh', 'geo-net', ?, 'ws://127.0.0.1:1', 'active', 'public', 1, ?, ?)")
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

    async fn cache_row(state: &AppState, provider: GeoProvider, ip: &str) -> Option<CacheRow> {
        sqlx::query_as(
            "SELECT country_code, state, created_at, expires_at, last_success_at, last_attempt_at FROM geo_location_cache WHERE provider = ? AND canonical_ip = ?",
        )
        .bind(provider.as_str())
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

    /// Start a refresh and wait for its observable terminal state instead of
    /// sleeping for a fixed duration.
    async fn run_refresh_to_terminal(state: &AppState) -> GeoRefreshSnapshot {
        let prepared = match begin_refresh(state, format!("run-{}", uuid::Uuid::new_v4()), &clock())
            .await
            .unwrap()
        {
            RefreshStart::Started(prepared) => prepared,
            RefreshStart::Joined(_) => panic!("a clean state starts a new run"),
        };
        spawn_refresh(state, prepared);
        wait_for_terminal(state).await
    }

    async fn wait_for_terminal(state: &AppState) -> GeoRefreshSnapshot {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(snapshot) = state.geo_refresh().snapshot() {
                    if snapshot.state != RefreshState::Running {
                        return snapshot;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the refresh run reaches an observable terminal state")
    }

    async fn wait_for_requests(stub: &StubServer, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while stub.request_count() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the stub observes the outbound requests");
    }

    /// The core promise of the feature: a retained country that is still
    /// inside its 24-hour validity is re-resolved anyway, and the new result
    /// is really written. Clearing the cache would leave the wrong country in
    /// place; this test seeds a deliberately wrong one.
    #[tokio::test]
    async fn a_valid_retained_country_is_recomputed_and_rewritten() {
        let dir = tempdir().unwrap();
        let state = refresh_state(&dir, true, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('local_mmdb', ?, 'US', 'current', ?, ?, ?, ?, ?)")
            .bind(RESOLVABLE)
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .bind("2099-01-01T00:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        // The automatic path schedules nothing for a valid result, which is
        // exactly what makes the forced re-query observable.
        assert_eq!(
            crate::geo_backfill::run_pass(&state, NOW).await.unwrap(),
            crate::geo_backfill::BackfillSummary::default()
        );
        let before = clock();

        let snapshot = run_refresh_to_terminal(&state).await;

        assert_eq!(snapshot.state, RefreshState::Completed);
        assert_eq!(snapshot.total_lookups, 1);
        assert_eq!(snapshot.peer_records_in_scope, 1);
        assert_eq!(snapshot.completed_lookups, 1);
        assert_eq!(snapshot.resolved_lookups, 1);
        assert_eq!(snapshot.failed_lookups, 0);
        let row = cache_row(&state, GeoProvider::LocalMmdb, RESOLVABLE)
            .await
            .unwrap();
        assert_eq!(
            row.0.as_deref(),
            Some("SE"),
            "the forced query replaced the cached country"
        );
        assert_eq!(row.1, "current");
        assert!(
            row.4
                .as_deref()
                .is_some_and(|success| success >= before.as_str()),
            "the success time is the forced lookup's"
        );
    }

    /// The two denominators stay distinct: several Peer records may reference
    /// one address, so the map's Peer-record count is never the lookup count.
    #[tokio::test]
    async fn lookup_and_peer_record_denominators_stay_distinct() {
        let dir = tempdir().unwrap();
        let state = refresh_state(&dir, true, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        seed_node(&state, "geo-node-b").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        insert_peer(&state, "geo-node-a", "p2", RESOLVABLE).await;
        insert_peer(&state, "geo-node-b", "p3", RESOLVABLE).await;
        insert_peer(&state, "geo-node-a", "p4", NO_COUNTRY).await;
        // A documentation address is refused by the trust boundary: it is in
        // neither denominator.
        insert_peer(&state, "geo-node-a", "p5", "203.0.113.9").await;

        let snapshot = run_refresh_to_terminal(&state).await;

        assert_eq!(snapshot.state, RefreshState::Completed);
        assert_eq!(snapshot.total_lookups, 2, "two distinct addresses");
        assert_eq!(
            snapshot.peer_records_in_scope, 4,
            "four Peer records reference them"
        );
        assert_eq!(snapshot.resolved_lookups, 1);
        assert_eq!(snapshot.no_country_lookups, 1);
        assert_eq!(snapshot.completed_lookups, 2);
    }

    /// Repeated Owner requests share one batch instead of scheduling a second
    /// one. The first run is installed but deliberately not spawned, so the
    /// rule is observed without racing a real lookup.
    #[tokio::test]
    async fn an_identical_request_joins_the_running_run() {
        let dir = tempdir().unwrap();
        let state = refresh_state(&dir, true, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;

        let first = match begin_refresh(&state, "run-1".to_owned(), NOW)
            .await
            .unwrap()
        {
            RefreshStart::Started(prepared) => prepared,
            RefreshStart::Joined(_) => panic!("the first request starts the run"),
        };
        assert_eq!(first.snapshot().total_lookups, 1);
        assert!(
            state
                .geo_refresh()
                .is_running_for(GeoProvider::LocalMmdb, 1)
        );

        let joined = match begin_refresh(&state, "run-2".to_owned(), NOW)
            .await
            .unwrap()
        {
            RefreshStart::Joined(snapshot) => snapshot,
            RefreshStart::Started(_) => {
                panic!("an identical request must never start a second batch")
            }
        };
        assert_eq!(joined.run_id, "run-1");
        assert_eq!(joined.total_lookups, 1);

        // A request under a different configuration is not merged into the
        // old run, and the old run is not left claiming to be current.
        state.apply_geo_provider(GeoSelection {
            provider: GeoProvider::LocalMmdb,
            generation: 2,
        });
        let second = match begin_refresh(&state, "run-3".to_owned(), NOW)
            .await
            .unwrap()
        {
            RefreshStart::Started(prepared) => prepared,
            RefreshStart::Joined(_) => panic!("a new generation never joins the old run"),
        };
        assert_eq!(second.snapshot().provider_generation, 2);
        let first_snapshot = first.snapshot();
        assert_eq!(first_snapshot.state, RefreshState::Aborted);
        assert_eq!(first_snapshot.abort, Some(RefreshAbort::ProviderChanged));
        assert_eq!(first_snapshot.completed_lookups, 0);
    }

    /// A run that is replaced by a run for another selection is marked
    /// superseded, never silently merged, and the newest run keeps its own
    /// identity.
    #[test]
    fn a_newer_run_supersedes_a_running_one() {
        let registry = GeoRefreshRegistry::default();
        let scope = RefreshScope {
            addresses: Vec::new(),
            peer_records: 0,
        };
        let first_config = GeoConfig {
            provider: GeoProvider::LocalMmdb,
            generation: 1,
            mmdb_path: None,
        };
        let Install::Installed(first) =
            registry.install(&first_config, "run-a".to_owned(), &scope, NOW)
        else {
            panic!("the first install always starts a run")
        };
        let second_config = GeoConfig {
            provider: GeoProvider::GeoJs,
            generation: 2,
            mmdb_path: None,
        };
        let Install::Installed(second) =
            registry.install(&second_config, "run-b".to_owned(), &scope, NOW)
        else {
            panic!("a different selection never joins the old run")
        };
        let first = first.lock().unwrap();
        assert_eq!(first.snapshot.state, RefreshState::Aborted);
        assert_eq!(first.snapshot.abort, Some(RefreshAbort::Superseded));
        drop(first);
        let second = second.lock().unwrap();
        assert!(second.is_running());
        assert_eq!(second.snapshot.run_id, "run-b");
        drop(second);
        assert_eq!(registry.snapshot().unwrap().run_id, "run-b");
    }

    /// Geo that cannot resolve anything is refused with a stable reason
    /// instead of a run that pretends to have refreshed something.
    #[tokio::test]
    async fn disabled_and_unconfigured_geo_are_refused() {
        let dir = tempdir().unwrap();
        let state = refresh_state(&dir, true, GeoProvider::Disabled).await;
        assert_eq!(
            begin_refresh(&state, "run-disabled".to_owned(), NOW)
                .await
                .err(),
            Some(StartRefreshError::Unavailable(DISABLED_REASON))
        );
        assert!(state.geo_refresh().snapshot().is_none());

        // A deployment without a configured local database cannot run Local
        // MMDB, so a refresh is refused for the same reason the option is
        // unavailable.
        let other = tempdir().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            other.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pepper_path = other.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth).with_geo_provider(GeoSelection {
            provider: GeoProvider::LocalMmdb,
            generation: 1,
        });
        assert_eq!(
            begin_refresh(&state, "run-local".to_owned(), NOW)
                .await
                .err(),
            Some(StartRefreshError::Unavailable(
                crate::geo::NO_LOCAL_DATABASE_REASON
            ))
        );
    }

    /// The background pass yields exactly the addresses a running refresh
    /// owns, so one address is never resolved by two paths at once - while a
    /// Peer reference that appears mid-run is still resolved normally and is
    /// not delayed until the run ends.
    #[tokio::test]
    async fn the_background_pass_yields_only_the_owned_addresses() {
        let dir = tempdir().unwrap();
        let state = refresh_state(&dir, true, GeoProvider::LocalMmdb).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;

        let prepared = match begin_refresh(&state, "run-owned".to_owned(), NOW)
            .await
            .unwrap()
        {
            RefreshStart::Started(prepared) => prepared,
            RefreshStart::Joined(_) => panic!("the first request starts the run"),
        };
        assert_eq!(
            crate::geo_backfill::run_pass(&state, NOW).await.unwrap(),
            crate::geo_backfill::BackfillSummary::default(),
            "the address the run owns is not resolved a second time"
        );
        assert_eq!(cache_count(&state).await, 0);

        // A Peer reference that only appears once the run is in flight falls
        // outside the frozen scope, so the ordinary pass resolves it with no
        // new Agent report and no wait for the run to finish.
        insert_peer(&state, "geo-node-a", "p2", NO_COUNTRY).await;
        let summary = crate::geo_backfill::run_pass(&state, NOW).await.unwrap();
        assert_eq!(summary.attempted, 1);
        assert_eq!(summary.no_country, 1);
        let row = cache_row(&state, GeoProvider::LocalMmdb, NO_COUNTRY)
            .await
            .unwrap();
        assert_eq!(row.1, "no_country");

        prepared.abandon(RefreshAbort::Superseded, NOW);
        assert_eq!(
            crate::geo_backfill::run_pass(&state, NOW)
                .await
                .unwrap()
                .resolved,
            1
        );
    }

    /// A slow refresh must not block report ingestion or the Public
    /// projection: external requests live entirely outside the database
    /// transaction, so a stalled provider delays only the run that asked for
    /// it. While the run waits on a deliberately slow reply, a new Peer
    /// reference is recorded and the Public country projection is read - both
    /// against the same real database - and the ordinary pass still resolves
    /// the address the run does not own.
    #[tokio::test]
    async fn a_slow_refresh_does_not_block_reports_or_the_home_projection() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::Slow(
            1_500,
            r#"{"country_code":"US"}"#.to_owned(),
        )])
        .await;
        let state = external_state(&dir, &stub, GeoProvider::GeoJs, Duration::from_secs(5)).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        // A successful Peer Snapshot basis so the projection has a denominator.
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, node_id, component_key, state, attempted_at, observed_at, received_at, value_received_at, state_revision, value_revision) VALUES ('agent-geo-refresh', 'node', 'geo-node-a', 'geo-node-a', 'peers', 'ok', ?, ?, ?, ?, 1, 1)")
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .execute(state.db().pool())
            .await
            .unwrap();

        let prepared = match begin_refresh(&state, "run-slow".to_owned(), &clock())
            .await
            .unwrap()
        {
            RefreshStart::Started(prepared) => prepared,
            RefreshStart::Joined(_) => panic!("a clean state starts a new run"),
        };
        spawn_refresh(&state, prepared);
        wait_for_requests(&stub, 1).await;
        assert_eq!(
            state.geo_refresh().snapshot().unwrap().state,
            RefreshState::Running
        );

        // A report that arrives mid-refresh is still recorded, and the Public
        // country projection is still served, both well inside the slow
        // provider's own reply time.
        let started = Instant::now();
        insert_peer(&state, "geo-node-a", "p2", NO_COUNTRY).await;
        let insight = crate::http::public::public_country_distribution(
            &state,
            "geo-net",
            &state.geo_status(),
        )
        .await;
        assert_eq!(insight.known_country_count, Some(0));
        assert_eq!(insight.unknown_country_count, Some(2));
        assert!(
            started.elapsed() < Duration::from_millis(1_000),
            "a slow Geo provider must not delay ingestion or the country list"
        );

        // The ordinary pass yields the address the run owns and keeps working
        // on everything else, without waiting for the run to finish.
        let summary = crate::geo_backfill::run_pass(&state, &clock())
            .await
            .unwrap();
        assert_eq!(summary.attempted, 1);
        assert_eq!(summary.resolved, 1);
        let row = cache_row(&state, GeoProvider::GeoJs, NO_COUNTRY)
            .await
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("US"));

        // The run itself completes with its own address.
        let snapshot = wait_for_terminal(&state).await;
        assert_eq!(snapshot.state, RefreshState::Completed);
        assert_eq!(snapshot.completed_lookups, 1);
        assert_eq!(snapshot.resolved_lookups, 1);
    }

    /// An aborted run cancels its in-flight lookups at the next await instead
    /// of letting them run to their own timeout. A slow provider is the worst
    /// case for that window, so it is the one measured here: the run reaches
    /// its terminal state promptly, and no further request is scheduled.
    #[tokio::test]
    async fn an_aborted_run_cancels_its_in_flight_requests_promptly() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::Slow(
            5_000,
            r#"{"country_code":"US"}"#.to_owned(),
        )])
        .await;
        let state = external_state(&dir, &stub, GeoProvider::GeoJs, Duration::from_secs(5)).await;
        seed_node(&state, "geo-node-a").await;
        for index in 0..4 {
            insert_peer(
                &state,
                "geo-node-a",
                &format!("p{index}"),
                &format!("8.8.8.{}", index + 1),
            )
            .await;
        }

        let prepared = match begin_refresh(&state, "run-abort".to_owned(), &clock())
            .await
            .unwrap()
        {
            RefreshStart::Started(prepared) => prepared,
            RefreshStart::Joined(_) => panic!("a clean state starts a new run"),
        };
        spawn_refresh(&state, prepared);
        wait_for_requests(&stub, 1).await;

        let started = Instant::now();
        state.apply_geo_provider(GeoSelection {
            provider: GeoProvider::Disabled,
            generation: 2,
        });
        let snapshot = wait_for_terminal(&state).await;
        assert_eq!(snapshot.state, RefreshState::Aborted);
        assert_eq!(snapshot.abort, Some(RefreshAbort::ProviderChanged));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "an abort must not wait for a slow provider's timeout"
        );
        let observed = stub.request_count();
        assert_eq!(snapshot.completed_lookups, 0);
        assert_eq!(cache_count(&state).await, 0);
        assert!(
            observed <= 4,
            "the run scheduled no more work than its frozen scope"
        );
    }

    /// An empty scope is a real, honest completion with zero work, not a
    /// refusal and not a fake success.
    #[tokio::test]
    async fn a_run_without_current_peers_completes_with_zero_work() {
        let dir = tempdir().unwrap();
        let state = refresh_state(&dir, true, GeoProvider::LocalMmdb).await;

        let snapshot = run_refresh_to_terminal(&state).await;
        assert_eq!(snapshot.state, RefreshState::Completed);
        assert_eq!(snapshot.total_lookups, 0);
        assert_eq!(snapshot.peer_records_in_scope, 0);
        assert_eq!(snapshot.completed_lookups, 0);
        assert!(snapshot.finished_at.is_some());
    }

    /// A provider that changes while addresses are in flight stops the run at
    /// once: the terminal state is truthful and no late result is written for
    /// the configuration that is no longer selected.
    #[tokio::test]
    async fn a_provider_change_aborts_the_run_and_leaves_old_results_alone() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::Slow(
            2_000,
            r#"{"country_code":"US"}"#.to_owned(),
        )])
        .await;
        let state = external_state(&dir, &stub, GeoProvider::GeoJs, Duration::from_secs(2)).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        insert_peer(&state, "geo-node-a", "p2", NO_COUNTRY).await;

        let prepared = match begin_refresh(&state, "run-switch".to_owned(), &clock())
            .await
            .unwrap()
        {
            RefreshStart::Started(prepared) => prepared,
            RefreshStart::Joined(_) => panic!("a clean state starts a new run"),
        };
        spawn_refresh(&state, prepared);
        wait_for_requests(&stub, 1).await;

        // The Owner disables Geo while the refresh is in flight.
        state.apply_geo_provider(GeoSelection {
            provider: GeoProvider::Disabled,
            generation: 2,
        });
        let snapshot = wait_for_terminal(&state).await;

        assert_eq!(snapshot.state, RefreshState::Aborted);
        assert_eq!(snapshot.abort, Some(RefreshAbort::ProviderChanged));
        assert_eq!(snapshot.resolved_lookups, 0);
        assert_eq!(snapshot.completed_lookups, 0);
        assert_eq!(
            cache_count(&state).await,
            0,
            "a result from the old selection is never written"
        );
    }

    /// A rate limit ends the run with a truthful terminal state: the refused
    /// requests are counted as rate-limited, never as failed results, and
    /// nothing is recorded against the addresses.
    #[tokio::test]
    async fn a_rate_limit_ends_the_run_with_a_truthful_terminal_state() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::Status(429)]).await;
        let state = external_state(&dir, &stub, GeoProvider::Ipinfo, Duration::from_secs(2)).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        insert_peer(&state, "geo-node-a", "p2", NO_COUNTRY).await;
        insert_peer(&state, "geo-node-a", "p3", "8.8.8.8").await;

        let snapshot = run_refresh_to_terminal(&state).await;

        assert_eq!(snapshot.state, RefreshState::Aborted);
        assert_eq!(snapshot.abort, Some(RefreshAbort::RateLimited));
        assert!(snapshot.rate_limited_lookups >= 1);
        assert_eq!(snapshot.resolved_lookups, 0);
        assert_eq!(
            snapshot.completed_lookups, 0,
            "a refused request is not an authoritative outcome"
        );
        assert!(snapshot.completed_lookups < snapshot.total_lookups);
        assert_eq!(
            cache_count(&state).await,
            0,
            "a rate-limited request is not recorded as a failed attempt"
        );
        assert!(
            stub.request_count() <= snapshot.total_lookups as usize,
            "the provider-wide backoff stops the remaining requests"
        );
    }

    /// An External Geo Provider gets the same forced re-query: a valid
    /// retained country does not stop the refresh, and the provider's answer
    /// replaces it.
    #[tokio::test]
    async fn an_external_force_refresh_bypasses_a_valid_cache_entry() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::country("DE")]).await;
        let state = external_state(&dir, &stub, GeoProvider::Ipinfo, Duration::from_secs(2)).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;
        sqlx::query("INSERT INTO geo_location_cache (provider, canonical_ip, country_code, state, created_at, last_attempt_at, last_success_at, last_referenced_at, expires_at) VALUES ('ipinfo', ?, 'US', 'current', ?, ?, ?, ?, ?)")
            .bind(RESOLVABLE)
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .bind(NOW)
            .bind("2099-01-01T00:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        assert!(
            crate::geo_backfill::pending_addresses(
                state.db().pool(),
                GeoProvider::Ipinfo,
                NOW,
                None
            )
            .await
            .unwrap()
            .is_empty(),
            "the automatic path treats the retained result as valid"
        );

        let snapshot = run_refresh_to_terminal(&state).await;

        assert_eq!(snapshot.state, RefreshState::Completed);
        assert_eq!(snapshot.resolved_lookups, 1);
        assert_eq!(stub.request_count(), 1, "the address was really re-queried");
        let row = cache_row(&state, GeoProvider::Ipinfo, RESOLVABLE)
            .await
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("DE"));
        assert_eq!(row.1, "current");
    }

    /// Partial failure is a completed run with real counts: the successful
    /// addresses are written and the failed ones keep no result.
    #[tokio::test]
    async fn a_partial_failure_completes_with_real_counts() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![
            StubReply::document(r#"{"country_code":"US"}"#),
            StubReply::Status(500),
            StubReply::Slow(500, r#"{"country_code":"FR"}"#.to_owned()),
        ])
        .await;
        // A short bounded timeout turns the slow reply into a transport
        // failure, exactly like a real provider timeout.
        let state =
            external_state(&dir, &stub, GeoProvider::GeoJs, Duration::from_millis(150)).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", "8.8.8.8").await;
        insert_peer(&state, "geo-node-a", "p2", NO_COUNTRY).await;
        insert_peer(&state, "geo-node-a", "p3", RESOLVABLE).await;

        let snapshot = run_refresh_to_terminal(&state).await;

        assert_eq!(snapshot.state, RefreshState::Completed);
        assert_eq!(snapshot.total_lookups, 3);
        assert_eq!(snapshot.completed_lookups, 3);
        assert_eq!(snapshot.resolved_lookups, 1);
        assert_eq!(snapshot.failed_lookups, 2);
        // A failure records only the attempt, so every address has a row but
        // only the successful one carries a country.
        assert_eq!(cache_count(&state).await, 3);
        let resolved_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM geo_location_cache WHERE country_code IS NOT NULL",
        )
        .fetch_one(state.db().pool())
        .await
        .unwrap();
        assert_eq!(resolved_rows, 1);
    }

    /// A forced re-query of a previously failed address recovers without a
    /// new Agent report: the retained attempt is replaced by the real answer.
    #[tokio::test]
    async fn a_failed_address_recovers_on_the_next_forced_refresh() {
        let dir = tempdir().unwrap();
        let stub = StubServer::start(vec![StubReply::Status(500), StubReply::country("SE")]).await;
        let state = external_state(&dir, &stub, GeoProvider::Ipinfo, Duration::from_secs(2)).await;
        seed_node(&state, "geo-node-a").await;
        insert_peer(&state, "geo-node-a", "p1", RESOLVABLE).await;

        let first = run_refresh_to_terminal(&state).await;
        assert_eq!(first.state, RefreshState::Completed);
        assert_eq!(first.failed_lookups, 1);
        assert_eq!(
            cache_row(&state, GeoProvider::Ipinfo, RESOLVABLE)
                .await
                .unwrap()
                .1,
            "failed"
        );

        let second = run_refresh_to_terminal(&state).await;
        assert_eq!(second.state, RefreshState::Completed);
        assert_eq!(second.resolved_lookups, 1);
        let row = cache_row(&state, GeoProvider::Ipinfo, RESOLVABLE)
            .await
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("SE"));
        assert_eq!(row.1, "current");
    }
}
