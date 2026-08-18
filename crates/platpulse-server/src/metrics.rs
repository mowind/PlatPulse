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
use sqlx::SqlitePool;

use crate::http::AppState;

const SURFACES: [&str; 5] = ["public", "admin", "agent", "health", "other"];
const STATUSES: [&str; 5] = ["2xx", "3xx", "4xx", "5xx", "other"];
const OUTCOMES: [&str; 4] = ["accepted", "partially_accepted", "rejected", "unknown"];
const READINESS_COMPONENTS: [&str; 6] = [
    "sqlite",
    "owner",
    "web_assets",
    "shutdown",
    "critical_workers",
    "corruption",
];
const REALTIME_SURFACES: [&str; 2] = ["public", "admin"];
const OPERATION_STATUSES: [&str; 6] = [
    "queued",
    "running",
    "succeeded",
    "succeeded_with_warnings",
    "failed",
    "cancelled",
];
const DELIVERY_STATES: [&str; 7] = [
    "pending",
    "in_flight",
    "retry_scheduled",
    "succeeded",
    "failed",
    "dead_letter",
    "suppressed",
];

struct MetricsInner {
    http_requests: [[AtomicU64; STATUSES.len()]; SURFACES.len()],
    reports: [AtomicU64; OUTCOMES.len()],
    receipts: [AtomicU64; OUTCOMES.len()],
    readiness: [AtomicU64; READINESS_COMPONENTS.len()],
    realtime_connections: [AtomicU64; REALTIME_SURFACES.len()],
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

    pub fn set_realtime_connections(&self, surface: &str, count: u64) {
        if let Some(index) = REALTIME_SURFACES.iter().position(|value| *value == surface) {
            self.inner.realtime_connections[index].store(count, Ordering::Relaxed);
        }
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
            metric_gauge(
                &mut output,
                "platpulse_readiness",
                "Readiness state by fixed Server component.",
                &["component", component],
                self.inner.readiness[index].load(Ordering::Relaxed),
            );
        }
        let overall_ready = self
            .inner
            .readiness
            .iter()
            .all(|value| value.load(Ordering::Relaxed) == 1);
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
    pub sqlite_pool_size: u64,
    pub sqlite_pool_idle: u64,
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
    snapshot.sqlite_page_count = pragma_u64(pool, "PRAGMA page_count").await;
    snapshot.sqlite_freelist_pages = pragma_u64(pool, "PRAGMA freelist_count").await;
    let wal_path = db.path().with_file_name(format!(
        "{}-wal",
        db.path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("database")
    ));
    snapshot.sqlite_wal_bytes = std::fs::metadata(wal_path).ok().map(|value| value.len());
    snapshot.sqlite_pool_size = pool.size() as u64;
    snapshot.sqlite_pool_idle = pool.num_idle() as u64;

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

async fn pragma_u64(pool: &SqlitePool, query: &str) -> Option<u64> {
    let value = sqlx::query_scalar::<_, i64>(query)
        .fetch_one(pool)
        .await
        .ok()?;
    u64::try_from(value).ok()
}

async fn grouped_counts(pool: &SqlitePool, query: &str) -> Option<Vec<(String, u64)>> {
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
    let sqlite_ready =
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(state.db().pool())
            .await
            .is_ok_and(|version| version >= crate::database::SERVER_SCHEMA_VERSION)
            && !state.is_corrupt();
    registry.set_readiness("sqlite", sqlite_ready);
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
}
