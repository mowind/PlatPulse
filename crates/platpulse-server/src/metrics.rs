//! Internal Prometheus-compatible operational metrics.
//!
//! This module intentionally uses fixed, typed label dimensions instead of a
//! general-purpose registry. The management surface is assembled separately
//! from the public, Admin, Agent, and SPA routes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sqlx::AnyPool;

use crate::http::AppState;

const SURFACES: [&str; 5] = ["public", "admin", "agent", "health", "other"];
const STATUSES: [&str; 5] = ["2xx", "3xx", "4xx", "5xx", "other"];
const OUTCOMES: [&str; 4] = ["accepted", "partially_accepted", "rejected", "unknown"];
/// Every readiness component the exposition may report. The storage component
/// is named for the Storage Backend in use ([ADR 0013]): a SQLite deployment
/// reports `sqlite`, a PostgreSQL deployment reports `postgres`, and the other
/// name is not applicable and omitted from the exposition.
///
/// [ADR 0013]: ../../docs/adr/0013-postgresql-server-storage-backend.md
const READINESS_COMPONENTS: [&str; 8] = [
    "storage_ownership",
    "sqlite",
    "postgres",
    "owner",
    "web_assets",
    "shutdown",
    "critical_workers",
    "corruption",
];
const REALTIME_SURFACES: [&str; 2] = ["public", "admin"];
/// The two paths that settle History Gaps (issue #282): Report Ingestion and
/// the retention sweep.
const GAP_SETTLEMENT_PATHS: [&str; 2] = ["report", "retention"];
/// The History Gap settlement counters, each kept per settlement path.
const GAP_SETTLEMENT_METRICS: [(&str, &str); 5] = [
    (
        "platpulse_history_gap_settlements_total",
        "History Gap settlement passes by settlement path.",
    ),
    (
        "platpulse_history_gap_settlement_slow_total",
        "History Gap settlement passes slower than 500 ms by settlement path.",
    ),
    (
        "platpulse_history_gap_settlement_milliseconds_total",
        "Milliseconds spent settling History Gaps by settlement path.",
    ),
    (
        "platpulse_history_gap_settlement_examined_total",
        "Unresolved History Gaps examined for settlement by settlement path.",
    ),
    (
        "platpulse_history_gap_settlement_resolved_total",
        "History Gaps resolved by settlement path.",
    ),
];
const OPERATION_STATUSES: [&str; 6] = [
    "queued",
    "running",
    "succeeded",
    "succeeded_with_warnings",
    "failed",
    "cancelled",
];
const DELIVERY_STATES: [&str; 8] = [
    "pending",
    "in_flight",
    "retry_scheduled",
    "succeeded",
    "failed",
    "dead_letter",
    "suppressed",
    "cancelled",
];

/// A readiness component this deployment does not have. It is neither rendered
/// as a sample nor counted by `platpulse_ready`.
const READINESS_NOT_APPLICABLE: u64 = 2;

struct MetricsInner {
    http_requests: [[AtomicU64; STATUSES.len()]; SURFACES.len()],
    reports: [AtomicU64; OUTCOMES.len()],
    receipts: [AtomicU64; OUTCOMES.len()],
    readiness: [AtomicU64; READINESS_COMPONENTS.len()],
    realtime_connections: [AtomicU64; REALTIME_SURFACES.len()],
    /// Indexed by [`GAP_SETTLEMENT_PATHS`], then [`GAP_SETTLEMENT_METRICS`].
    gap_settlements: [[AtomicU64; GAP_SETTLEMENT_METRICS.len()]; GAP_SETTLEMENT_PATHS.len()],
    scrapes: AtomicU64,
    listener_failures: AtomicU64,
    listener_enabled: AtomicU64,
    listener_ready: AtomicU64,
}

/// Process-local counters and gauges with a fixed label vocabulary.
#[derive(Clone)]
pub struct MetricsRegistry {
    inner: Arc<MetricsInner>,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MetricsInner {
                http_requests: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
                reports: std::array::from_fn(|_| AtomicU64::new(0)),
                receipts: std::array::from_fn(|_| AtomicU64::new(0)),
                readiness: std::array::from_fn(|_| AtomicU64::new(0)),
                realtime_connections: std::array::from_fn(|_| AtomicU64::new(0)),
                gap_settlements: std::array::from_fn(|_| {
                    std::array::from_fn(|_| AtomicU64::new(0))
                }),
                scrapes: AtomicU64::new(0),
                listener_failures: AtomicU64::new(0),
                listener_enabled: AtomicU64::new(0),
                listener_ready: AtomicU64::new(0),
            }),
        }
    }

    pub fn observe_http_response(&self, path: &str, status: u16) {
        let surface = surface_index(path);
        let status = status_index(status);
        self.inner.http_requests[surface][status].fetch_add(1, Ordering::Relaxed);
    }

    pub fn observe_report(&self, outcome: &str) {
        self.inner.reports[outcome_index(outcome)].fetch_add(1, Ordering::Relaxed);
    }

    pub fn observe_receipt(&self, outcome: &str) {
        self.inner.receipts[outcome_index(outcome)].fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_readiness(&self, component: &str, ready: bool) {
        if let Some(index) = READINESS_COMPONENTS
            .iter()
            .position(|value| *value == component)
        {
            self.inner.readiness[index].store(ready as u64, Ordering::Relaxed);
        }
    }

    /// Mark a component as absent for this deployment (see
    /// [`READINESS_NOT_APPLICABLE`]).
    pub fn set_readiness_not_applicable(&self, component: &str) {
        if let Some(index) = READINESS_COMPONENTS
            .iter()
            .position(|value| *value == component)
        {
            self.inner.readiness[index].store(READINESS_NOT_APPLICABLE, Ordering::Relaxed);
        }
    }

    pub fn set_realtime_connections(&self, surface: &str, count: u64) {
        if let Some(index) = REALTIME_SURFACES.iter().position(|value| *value == surface) {
            self.inner.realtime_connections[index].store(count, Ordering::Relaxed);
        }
    }

    /// Count one History Gap settlement pass on `path` (`report` or
    /// `retention`); an unknown path is ignored.
    pub(crate) fn observe_gap_settlement(
        &self,
        path: &str,
        settlement: crate::http::report_ingestion::GapSettlement,
        elapsed: std::time::Duration,
        slow: bool,
    ) {
        let Some(index) = GAP_SETTLEMENT_PATHS.iter().position(|value| *value == path) else {
            return;
        };
        let values = [
            1,
            slow as u64,
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            settlement.examined,
            settlement.resolved,
        ];
        for (counter, value) in self.inner.gap_settlements[index].iter().zip(values) {
            counter.fetch_add(value, Ordering::Relaxed);
        }
    }

    /// The History Gap settlement counters of `path`, in
    /// [`GAP_SETTLEMENT_METRICS`] order: passes, slow passes, milliseconds,
    /// gaps examined, gaps resolved.
    #[cfg(test)]
    pub(crate) fn gap_settlement_counters(
        &self,
        path: &str,
    ) -> [u64; GAP_SETTLEMENT_METRICS.len()] {
        let index = GAP_SETTLEMENT_PATHS
            .iter()
            .position(|value| *value == path)
            .expect("a known settlement path");
        std::array::from_fn(|metric| {
            self.inner.gap_settlements[index][metric].load(Ordering::Relaxed)
        })
    }

    pub(crate) fn set_listener_enabled(&self, enabled: bool) {
        self.inner
            .listener_enabled
            .store(enabled as u64, Ordering::Relaxed);
    }

    pub(crate) fn set_listener_ready(&self, ready: bool) {
        self.inner
            .listener_ready
            .store(ready as u64, Ordering::Relaxed);
    }

    pub(crate) fn observe_listener_failure(&self) {
        self.inner.listener_failures.fetch_add(1, Ordering::Relaxed);
    }

    fn observe_scrape(&self) {
        self.inner.scrapes.fetch_add(1, Ordering::Relaxed);
    }

    /// Render the complete exposition with only fixed metric names and labels.
    pub fn render(&self, snapshot: &MetricsSnapshot) -> String {
        let mut output = String::new();
        metric_header(
            &mut output,
            "platpulse_http_requests_total",
            "HTTP responses by bounded surface and status class.",
            "counter",
        );
        for (surface_index, surface) in SURFACES.iter().enumerate() {
            for (status_index, status) in STATUSES.iter().enumerate() {
                metric_counter(
                    &mut output,
                    "platpulse_http_requests_total",
                    "HTTP responses by bounded surface and status class.",
                    &["surface", surface, "status", status],
                    self.inner.http_requests[surface_index][status_index].load(Ordering::Relaxed),
                );
            }
        }
        metric_header(
            &mut output,
            "platpulse_agent_reports_total",
            "AgentReport requests observed by bounded outcome.",
            "counter",
        );
        metric_header(
            &mut output,
            "platpulse_report_receipts_total",
            "Report Receipts returned by bounded outcome.",
            "counter",
        );
        for (index, outcome) in OUTCOMES.iter().enumerate() {
            metric_counter(
                &mut output,
                "platpulse_agent_reports_total",
                "AgentReport requests observed by bounded outcome.",
                &["outcome", outcome],
                self.inner.reports[index].load(Ordering::Relaxed),
            );
            metric_counter(
                &mut output,
                "platpulse_report_receipts_total",
                "Report Receipts returned by bounded outcome.",
                &["outcome", outcome],
                self.inner.receipts[index].load(Ordering::Relaxed),
            );
        }
        metric_header(
            &mut output,
            "platpulse_readiness",
            "Readiness state by fixed Server component.",
            "gauge",
        );
        for (index, component) in READINESS_COMPONENTS.iter().enumerate() {
            let readiness = self.inner.readiness[index].load(Ordering::Relaxed);
            if readiness == READINESS_NOT_APPLICABLE {
                continue;
            }
            metric_gauge(
                &mut output,
                "platpulse_readiness",
                "Readiness state by fixed Server component.",
                &["component", component],
                readiness,
            );
        }
        let overall_ready = self.inner.readiness.iter().all(|value| {
            let readiness = value.load(Ordering::Relaxed);
            readiness == 1 || readiness == READINESS_NOT_APPLICABLE
        });
        metric_header(
            &mut output,
            "platpulse_ready",
            "Whether all required Server readiness components are ready.",
            "gauge",
        );
        metric_gauge(
            &mut output,
            "platpulse_ready",
            "Whether all required Server readiness components are ready.",
            &[],
            overall_ready as u64,
        );
        metric_header(
            &mut output,
            "platpulse_liveness",
            "Whether this process is serving the metrics surface.",
            "gauge",
        );
        metric_gauge(
            &mut output,
            "platpulse_liveness",
            "Whether this process is serving the metrics surface.",
            &[],
            1,
        );
        metric_header(
            &mut output,
            "platpulse_realtime_connections",
            "Active realtime connections by bounded surface.",
            "gauge",
        );
        for (index, surface) in REALTIME_SURFACES.iter().enumerate() {
            metric_gauge(
                &mut output,
                "platpulse_realtime_connections",
                "Active realtime connections by bounded surface.",
                &["surface", surface],
                self.inner.realtime_connections[index].load(Ordering::Relaxed),
            );
        }
        for (metric_index, (name, help)) in GAP_SETTLEMENT_METRICS.iter().enumerate() {
            metric_header(&mut output, name, help, "counter");
            for (path_index, path) in GAP_SETTLEMENT_PATHS.iter().enumerate() {
                metric_counter(
                    &mut output,
                    name,
                    help,
                    &["path", path],
                    self.inner.gap_settlements[path_index][metric_index].load(Ordering::Relaxed),
                );
            }
        }
        metric_header(
            &mut output,
            "platpulse_metrics_scrapes_total",
            "Metrics scrapes served by this process.",
            "counter",
        );
        metric_counter(
            &mut output,
            "platpulse_metrics_scrapes_total",
            "Metrics scrapes served by this process.",
            &[],
            self.inner.scrapes.load(Ordering::Relaxed),
        );
        metric_header(
            &mut output,
            "platpulse_metrics_listener_failures_total",
            "Metrics listener startup or runtime failures.",
            "counter",
        );
        metric_counter(
            &mut output,
            "platpulse_metrics_listener_failures_total",
            "Metrics listener startup or runtime failures.",
            &[],
            self.inner.listener_failures.load(Ordering::Relaxed),
        );
        metric_header(
            &mut output,
            "platpulse_metrics_listener_enabled",
            "Whether the dedicated metrics listener is configured.",
            "gauge",
        );
        metric_gauge(
            &mut output,
            "platpulse_metrics_listener_enabled",
            "Whether the dedicated metrics listener is configured.",
            &[],
            self.inner.listener_enabled.load(Ordering::Relaxed),
        );
        metric_header(
            &mut output,
            "platpulse_metrics_listener_ready",
            "Whether the dedicated metrics listener is accepting connections.",
            "gauge",
        );
        metric_gauge(
            &mut output,
            "platpulse_metrics_listener_ready",
            "Whether the dedicated metrics listener is accepting connections.",
            &[],
            self.inner.listener_ready.load(Ordering::Relaxed),
        );
        metric_header(
            &mut output,
            "platpulse_critical_worker_heartbeat_age_seconds",
            "Age of the critical worker heartbeat in seconds.",
            "gauge",
        );
        metric_optional_gauge(
            &mut output,
            "platpulse_critical_worker_heartbeat_age_seconds",
            "Age of the critical worker heartbeat in seconds.",
            &[],
            snapshot.critical_worker_heartbeat_age_seconds,
        );
        metric_header(
            &mut output,
            "platpulse_operations",
            "Current operation rows by fixed status.",
            "gauge",
        );
        for (index, status) in OPERATION_STATUSES.iter().enumerate() {
            if let Some(value) = snapshot.operations[index] {
                metric_gauge(
                    &mut output,
                    "platpulse_operations",
                    "Current operation rows by fixed status.",
                    &["status", status],
                    value,
                );
            }
        }
        metric_header(
            &mut output,
            "platpulse_notification_deliveries",
            "Current notification deliveries by fixed state.",
            "gauge",
        );
        for (index, state) in DELIVERY_STATES.iter().enumerate() {
            if let Some(value) = snapshot.notification_deliveries[index] {
                metric_gauge(
                    &mut output,
                    "platpulse_notification_deliveries",
                    "Current notification deliveries by fixed state.",
                    &["state", state],
                    value,
                );
            }
        }
        metric_header(
            &mut output,
            "platpulse_sqlite_page_count",
            "SQLite database pages currently allocated.",
            "gauge",
        );
        metric_optional_gauge(
            &mut output,
            "platpulse_sqlite_page_count",
            "SQLite database pages currently allocated.",
            &[],
            snapshot.sqlite_page_count,
        );
        metric_header(
            &mut output,
            "platpulse_sqlite_freelist_pages",
            "SQLite pages currently available on the freelist.",
            "gauge",
        );
        metric_optional_gauge(
            &mut output,
            "platpulse_sqlite_freelist_pages",
            "SQLite pages currently available on the freelist.",
            &[],
            snapshot.sqlite_freelist_pages,
        );
        metric_header(
            &mut output,
            "platpulse_sqlite_wal_bytes",
            "SQLite WAL sidecar size in bytes.",
            "gauge",
        );
        metric_optional_gauge(
            &mut output,
            "platpulse_sqlite_wal_bytes",
            "SQLite WAL sidecar size in bytes.",
            &[],
            snapshot.sqlite_wal_bytes,
        );
        // The PostgreSQL Storage Backend has no page accounting the Server may
        // read, but `pg_database_size` reports the database size. The series is
        // omitted when the server cannot answer, never published as zero
        // (ADR 0013).
        metric_header(
            &mut output,
            "platpulse_storage_database_bytes",
            "Bytes the Storage Backend database occupies.",
            "gauge",
        );
        metric_optional_gauge(
            &mut output,
            "platpulse_storage_database_bytes",
            "Bytes the Storage Backend database occupies.",
            &[],
            snapshot.storage_database_bytes,
        );
        metric_header(
            &mut output,
            "platpulse_sqlite_pool_size",
            "SQLite connection pool capacity in connections.",
            "gauge",
        );
        metric_gauge(
            &mut output,
            "platpulse_sqlite_pool_size",
            "SQLite connection pool capacity in connections.",
            &[],
            snapshot.sqlite_pool_size,
        );
        metric_header(
            &mut output,
            "platpulse_sqlite_pool_idle",
            "SQLite idle connections in the pool.",
            "gauge",
        );
        metric_gauge(
            &mut output,
            "platpulse_sqlite_pool_idle",
            "SQLite idle connections in the pool.",
            &[],
            snapshot.sqlite_pool_idle,
        );
        metric_header(
            &mut output,
            "platpulse_capacity_total_bytes",
            "Size of the filesystem that holds Server state, in bytes.",
            "gauge",
        );
        metric_optional_gauge(
            &mut output,
            "platpulse_capacity_total_bytes",
            "Size of the filesystem that holds Server state, in bytes.",
            &[],
            snapshot.capacity_total_bytes,
        );
        metric_header(
            &mut output,
            "platpulse_capacity_available_bytes",
            "Bytes available on the filesystem that holds Server state.",
            "gauge",
        );
        metric_optional_gauge(
            &mut output,
            "platpulse_capacity_available_bytes",
            "Bytes available on the filesystem that holds Server state.",
            &[],
            snapshot.capacity_available_bytes,
        );
        metric_header(
            &mut output,
            "platpulse_capacity_paused",
            "1 while optional history is paused for low space, 0 otherwise.",
            "gauge",
        );
        metric_gauge(
            &mut output,
            "platpulse_capacity_paused",
            "1 while optional history is paused for low space, 0 otherwise.",
            &[],
            snapshot.capacity_paused,
        );
        metric_header(
            &mut output,
            "platpulse_ingestion_in_flight",
            "AgentReport ingestions currently in flight.",
            "gauge",
        );
        metric_gauge(
            &mut output,
            "platpulse_ingestion_in_flight",
            "AgentReport ingestions currently in flight.",
            &[],
            snapshot.ingestion_in_flight,
        );
        metric_header(
            &mut output,
            "platpulse_realtime_buffered_events",
            "Bounded realtime events currently buffered.",
            "gauge",
        );
        metric_gauge(
            &mut output,
            "platpulse_realtime_buffered_events",
            "Bounded realtime events currently buffered.",
            &["surface", "public"],
            snapshot.public_buffered_events,
        );
        metric_gauge(
            &mut output,
            "platpulse_realtime_buffered_events",
            "Bounded realtime events currently buffered.",
            &["surface", "admin"],
            snapshot.admin_buffered_events,
        );
        output
    }
}

#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    pub critical_worker_heartbeat_age_seconds: Option<u64>,
    pub operations: [Option<u64>; OPERATION_STATUSES.len()],
    pub notification_deliveries: [Option<u64>; DELIVERY_STATES.len()],
    pub sqlite_page_count: Option<u64>,
    pub sqlite_freelist_pages: Option<u64>,
    pub sqlite_wal_bytes: Option<u64>,
    /// Size of the PostgreSQL database, from `pg_database_size`. Unknown on
    /// SQLite, which reports page accounting instead, and unknown when the
    /// server cannot answer (ADR 0013).
    pub storage_database_bytes: Option<u64>,
    pub sqlite_pool_size: u64,
    pub sqlite_pool_idle: u64,
    /// Latest measurement of the filesystem that holds Server state, taken by
    /// the sampling worker rather than at scrape time: a scrape must not do
    /// filesystem I/O, and an unknown measurement stays unknown.
    pub capacity_total_bytes: Option<u64>,
    pub capacity_available_bytes: Option<u64>,
    /// 1 while optional history is paused for low space.
    pub capacity_paused: u64,
    pub ingestion_in_flight: u64,
    pub public_buffered_events: u64,
    pub admin_buffered_events: u64,
}

fn metric_header(output: &mut String, name: &str, help: &str, kind: &str) {
    output.push_str("# HELP ");
    output.push_str(name);
    output.push(' ');
    output.push_str(help);
    output.push('\n');
    output.push_str("# TYPE ");
    output.push_str(name);
    output.push(' ');
    output.push_str(kind);
    output.push('\n');
}

fn metric_labels(labels: &[&str]) -> String {
    let mut rendered = String::new();
    if labels.is_empty() {
        return rendered;
    }
    rendered.push('{');
    for (index, value) in labels.chunks_exact(2).enumerate() {
        if index > 0 {
            rendered.push(',');
        }
        rendered.push_str(value[0]);
        rendered.push_str("=\"");
        rendered.push_str(value[1]);
        rendered.push('\"');
    }
    rendered.push('}');
    rendered
}

fn metric_counter(output: &mut String, name: &str, _help: &str, labels: &[&str], value: u64) {
    output.push_str(name);
    output.push_str(&metric_labels(labels));
    output.push(' ');
    output.push_str(&value.to_string());
    output.push('\n');
}

fn metric_gauge(output: &mut String, name: &str, _help: &str, labels: &[&str], value: u64) {
    output.push_str(name);
    output.push_str(&metric_labels(labels));
    output.push(' ');
    output.push_str(&value.to_string());
    output.push('\n');
}

fn metric_optional_gauge(
    output: &mut String,
    name: &str,
    help: &str,
    labels: &[&str],
    value: Option<u64>,
) {
    if let Some(value) = value {
        metric_gauge(output, name, help, labels, value);
    }
}

fn surface_index(path: &str) -> usize {
    let path = path.split('?').next().unwrap_or(path);
    if path.starts_with("/api/public/v1/") {
        0
    } else if path.starts_with("/api/admin/v1/") {
        1
    } else if path.starts_with("/api/agent/v1/") {
        2
    } else if path.starts_with("/health/") {
        3
    } else {
        4
    }
}

fn status_index(status: u16) -> usize {
    match status {
        200..=299 => 0,
        300..=399 => 1,
        400..=499 => 2,
        500..=599 => 3,
        _ => 4,
    }
}

fn outcome_index(outcome: &str) -> usize {
    OUTCOMES
        .iter()
        .position(|value| *value == outcome)
        .unwrap_or(3)
}

#[derive(Clone)]
struct MetricsState {
    state: AppState,
    require_trusted_proxy: bool,
}

/// Build the isolated management application. It has one route and no SPA or
/// API fallback, so accidental exposure through a browser route is impossible.
pub(crate) fn build_app(state: &AppState, require_trusted_proxy: bool) -> Router {
    let metrics_state = MetricsState {
        state: state.clone(),
        require_trusted_proxy,
    };
    Router::new()
        .route("/metrics", get(metrics_handler))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(from_fn_with_state(
            metrics_state.clone(),
            metrics_proxy_guard,
        ))
        .with_state(metrics_state)
}

async fn metrics_proxy_guard(
    State(state): State<MetricsState>,
    request: Request,
    next: Next,
) -> Response {
    let request_is_allowed = match crate::http::evaluate_proxy_request(
        &state.state.proxy_policy,
        crate::http::request_peer_ip(&request),
        request.headers(),
    ) {
        Ok(true) => true,
        Ok(false) => !state.require_trusted_proxy,
        Err(_) => false,
    };
    if request_is_allowed {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

async fn metrics_handler(State(state): State<MetricsState>) -> Response {
    let registry = state.state.metrics();
    registry.observe_scrape();
    let snapshot = collect_snapshot(&state.state).await;
    let body = registry.render(&snapshot);
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

async fn collect_snapshot(state: &AppState) -> MetricsSnapshot {
    let mut snapshot = MetricsSnapshot::default();
    let registry = state.metrics();
    let db = state.db();
    let pool = db.pool();
    let pages = db.storage_pages().await;
    snapshot.sqlite_page_count = pages.page_count;
    snapshot.sqlite_freelist_pages = pages.freelist_pages;
    // Write-ahead accounting is a SQLite concept; the PostgreSQL backend has
    // no Server-side file to measure, so the sizes stay unknown there.
    snapshot.sqlite_wal_bytes = db.path().and_then(|path| {
        let name = path.file_name()?.to_str()?;
        std::fs::metadata(path.with_file_name(format!("{name}-wal")))
            .ok()
            .map(|value| value.len())
    });
    snapshot.storage_database_bytes = db
        .storage_database_bytes()
        .await
        .and_then(|bytes| u64::try_from(bytes).ok());
    snapshot.sqlite_pool_size = pool.size() as u64;
    snapshot.sqlite_pool_idle = pool.num_idle() as u64;
    let capacity = state.capacity().status();
    snapshot.capacity_total_bytes = capacity.sample.as_ref().map(|sample| sample.total_bytes);
    snapshot.capacity_available_bytes = capacity
        .sample
        .as_ref()
        .map(|sample| sample.available_bytes);
    snapshot.capacity_paused = u64::from(capacity.protected);

    if let Some(counts) = grouped_counts(
        pool,
        "SELECT status, COUNT(*) FROM operations GROUP BY status",
    )
    .await
    {
        snapshot.operations.fill(Some(0));
        for (status, count) in counts {
            if let Some(index) = OPERATION_STATUSES.iter().position(|value| *value == status) {
                snapshot.operations[index] = Some(count);
            }
        }
    }
    if let Some(counts) = grouped_counts(
        pool,
        "SELECT state, COUNT(*) FROM notification_deliveries GROUP BY state",
    )
    .await
    {
        snapshot.notification_deliveries.fill(Some(0));
        for (state, count) in counts {
            if let Some(index) = DELIVERY_STATES.iter().position(|value| *value == state) {
                snapshot.notification_deliveries[index] = Some(count);
            }
        }
    }
    snapshot.ingestion_in_flight = state.in_flight_ingestion();
    snapshot.critical_worker_heartbeat_age_seconds = state.critical_worker_heartbeat_age_seconds();
    snapshot.public_buffered_events = state.public_realtime().buffered_event_count();
    snapshot.admin_buffered_events = state.admin_realtime().buffered_event_count();
    registry.set_realtime_connections("public", state.public_realtime().active_connections());
    registry.set_realtime_connections("admin", state.admin_realtime().active_connections());
    update_readiness(state).await;
    snapshot
}

async fn grouped_counts(pool: &AnyPool, query: &str) -> Option<Vec<(String, u64)>> {
    let rows = sqlx::query_as::<_, (String, i64)>(query)
        .fetch_all(pool)
        .await
        .ok()?;
    rows.into_iter()
        .map(|(key, count)| Some((key, u64::try_from(count).ok()?)))
        .collect()
}

async fn update_readiness(state: &AppState) {
    let registry = state.metrics();
    let schema =
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(state.db().pool())
            .await;
    if let Err(error) = &schema {
        state.note_database_error(error);
    }
    let sqlite_ready = schema
        .is_ok_and(|version| version >= crate::database::SERVER_SCHEMA_VERSION)
        && !state.is_corrupt();
    registry.set_readiness("storage_ownership", state.db().ownership_held());
    let storage = state.db().backend();
    let unrealised_storage = match storage {
        crate::database::StorageBackend::Postgres => "sqlite",
        crate::database::StorageBackend::Sqlite => "postgres",
    };
    registry.set_readiness(storage.as_str(), sqlite_ready);
    registry.set_readiness_not_applicable(unrealised_storage);
    registry.set_readiness(
        "owner",
        crate::auth::has_owner(state.db()).await.unwrap_or(false),
    );
    registry.set_readiness("web_assets", state.web_assets_ready());
    registry.set_readiness("shutdown", !state.is_shutting_down());
    registry.set_readiness("critical_workers", state.critical_workers_healthy());
    registry.set_readiness("corruption", !state.is_corrupt());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposition_has_bounded_labels_and_no_sensitive_values() {
        let registry = MetricsRegistry::new();
        registry.observe_http_response("/api/admin/v1/nodes/secret-node", 200);
        registry.observe_http_response("/api/agent/v1/reports", 503);
        registry.observe_report("accepted");
        registry.observe_receipt("accepted");
        registry.set_readiness("critical_workers", true);
        registry.set_realtime_connections("admin", 3);

        let text = registry.render(&MetricsSnapshot::default());

        assert!(text.contains("platpulse_http_requests_total{surface=\"admin\",status=\"2xx\"} 1"));
        assert!(text.contains("platpulse_http_requests_total{surface=\"agent\",status=\"5xx\"} 1"));
        assert_eq!(
            text.matches("# HELP platpulse_http_requests_total ")
                .count(),
            1
        );
        assert_eq!(
            text.matches("# TYPE platpulse_http_requests_total counter")
                .count(),
            1
        );
        assert!(text.contains("platpulse_agent_reports_total{outcome=\"accepted\"} 1"));
        assert!(text.contains("platpulse_report_receipts_total{outcome=\"accepted\"} 1"));
        assert!(text.contains("platpulse_readiness{component=\"critical_workers\"} 1"));
        assert!(text.contains("platpulse_liveness 1"));
        assert!(text.contains("platpulse_realtime_connections{surface=\"admin\"} 3"));
        assert!(!text.contains("secret-node"));
        assert!(!text.contains("report_id"));
        assert!(!text.contains("raw"));
    }

    #[test]
    fn a_component_the_deployment_lacks_is_omitted_and_never_blocks_readiness() {
        let registry = MetricsRegistry::new();
        for component in READINESS_COMPONENTS {
            registry.set_readiness(component, true);
        }
        registry.set_readiness_not_applicable("postgres");
        registry.set_readiness("sqlite", false);

        let text = registry.render(&MetricsSnapshot::default());

        assert!(!text.contains("component=\"postgres\""), "{text}");
        assert!(text.contains("platpulse_readiness{component=\"sqlite\"} 0"));
        assert!(text.contains("platpulse_ready 0"), "{text}");

        registry.set_readiness("sqlite", true);
        let text = registry.render(&MetricsSnapshot::default());
        assert!(text.contains("platpulse_ready 1"), "{text}");
    }

    /// `pg_database_size` is the only storage figure the PostgreSQL backend has
    /// (issue #256): page and freelist samples do not apply there, and a server
    /// that cannot answer the size publishes no sample at all instead of a zero
    /// the Operator would read as an empty database (ADR 0013).
    #[test]
    fn the_postgres_database_size_is_published_only_when_it_is_known() {
        let registry = MetricsRegistry::new();
        let known = MetricsSnapshot {
            storage_database_bytes: Some(70_254_592),
            ..MetricsSnapshot::default()
        };
        let text = registry.render(&known);
        // Exactly one sample line, which is what a scraper reads as the size.
        let samples = text
            .lines()
            .filter(|line| line.starts_with("platpulse_storage_database_bytes"))
            .collect::<Vec<_>>();
        assert_eq!(samples, vec!["platpulse_storage_database_bytes 70254592"]);
        assert_eq!(
            text.matches("# HELP platpulse_storage_database_bytes ")
                .count(),
            1
        );

        // The HELP and TYPE lines stay in the exposition so a scraper keeps its
        // descriptor, but no sample line may appear: an unreadable size is a
        // missing reading, not a zero-byte database.
        let unknown = registry.render(&MetricsSnapshot::default());
        assert!(
            !unknown
                .lines()
                .any(|line| line.starts_with("platpulse_storage_database_bytes")),
            "an unreadable database size must not be published: {unknown}"
        );
    }

    #[test]
    fn unknown_paths_and_statuses_collapse_to_fixed_dimensions() {
        let registry = MetricsRegistry::new();
        registry.observe_http_response("/arbitrary?user_id=secret", 418);
        registry.observe_report("unexpected-error-text");

        let text = registry.render(&MetricsSnapshot::default());

        assert!(text.contains("platpulse_http_requests_total{surface=\"other\",status=\"4xx\"} 1"));
        assert!(text.contains("platpulse_agent_reports_total{outcome=\"unknown\"} 1"));
        assert!(!text.contains("user_id"));
        assert!(!text.contains("unexpected-error-text"));
    }

    /// The metrics label vocabulary must stay identical to the Delivery state
    /// vocabulary the Server actually writes; otherwise a state is silently
    /// dropped from the gauge (issue #175 widened both to include
    /// `cancelled`).
    #[test]
    fn delivery_state_labels_mirror_the_notification_vocabulary() {
        assert_eq!(
            DELIVERY_STATES.as_slice(),
            crate::notifications::DELIVERY_STATES,
        );
    }
}
