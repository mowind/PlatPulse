//! Minimal operational health routes (design §20.3).
//!
//! `/health/live` only proves the event loop responds; `/health/ready`
//! checks the components the Server owns (SQLite migrations, the first
//! Owner, Web assets). Neither response leaks versions, DB paths, or
//! internal counts.
//!
//! Runtime corruption is latched from an `SQLITE_CORRUPT` observed on a real
//! query (`AppState::note_sqlite_error`). The authoritative whole-database
//! verdict is an offline operation (ADR 0008, issue #194).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;
use utoipa::ToSchema;

use crate::http::AppState;

/// Whether an error carries SQLite's primary or extended `SQLITE_CORRUPT` /
/// `SQLITE_NOTADB` result code.
pub(crate) fn is_corruption_error(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|error| error.code())
        .and_then(|code| code.parse::<i32>().ok())
        // SQLite primary and extended SQLITE_CORRUPT / SQLITE_NOTADB codes.
        .is_some_and(|code| matches!(code & 0xff, 11 | 26))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LiveResponse {
    status: &'static str,
}

#[utoipa::path(
    get,
    path = "/health/live",
    tag = "system",
    responses((status = 200, description = "Event loop is responding", body = LiveResponse))
)]
pub async fn live() -> impl IntoResponse {
    Json(LiveResponse { status: "ok" })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadyState {
    Ready,
    NotReady,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadyComponent {
    name: String,
    status: ReadyState,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

impl ReadyComponent {
    fn ready(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            status: ReadyState::Ready,
            reason: None,
        }
    }

    fn not_ready(name: &str, reason: &'static str) -> Self {
        Self {
            name: name.to_owned(),
            status: ReadyState::NotReady,
            reason: Some(reason),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadyResponse {
    status: ReadyState,
    components: Vec<ReadyComponent>,
}

#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "system",
    responses(
        (status = 200, description = "Server is ready to serve", body = ReadyResponse),
        (status = 503, description = "Server is not ready", body = ReadyResponse)
    )
)]
pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let sqlite = if state.is_corrupt() {
        ReadyComponent::not_ready("sqlite", "integrity_check_failed")
    } else {
        match state.db().schema_version().await {
            Ok(version) if version >= crate::database::SERVER_SCHEMA_VERSION => {
                ReadyComponent::ready("sqlite")
            }
            Ok(_) => ReadyComponent::not_ready("sqlite", "migration_pending"),
            Err(error) => {
                if let Some(error) = error.as_sqlx() {
                    state.note_sqlite_error(error);
                }
                ReadyComponent::not_ready("sqlite", "unavailable")
            }
        }
    };
    let owner = match crate::auth::has_owner(state.db()).await {
        Ok(true) => ReadyComponent::ready("owner"),
        Ok(false) => ReadyComponent::not_ready("owner", "setup_required"),
        Err(error) => {
            state.note_sqlite_error(&error);
            ReadyComponent::not_ready("owner", "unavailable")
        }
    };
    let web_assets = if state.web_assets_ready() {
        ReadyComponent::ready("web_assets")
    } else {
        ReadyComponent::not_ready("web_assets", "web_assets_missing")
    };

    let shutdown = if state.is_shutting_down() {
        ReadyComponent::not_ready("shutdown", "shutting_down")
    } else {
        ReadyComponent::ready("shutdown")
    };
    let workers = if state.critical_workers_healthy() {
        ReadyComponent::ready("critical_workers")
    } else {
        ReadyComponent::not_ready("critical_workers", "worker_unhealthy")
    };
    // Corruption is latched from an observed `SQLITE_CORRUPT` error and never
    // cleared by a later successful query; with no periodic scan there is no
    // separate "unavailable" verdict to report (ADR 0008, issue #194).
    let corruption = if state.is_corrupt() {
        ReadyComponent::not_ready("corruption", "integrity_check_failed")
    } else {
        ReadyComponent::ready("corruption")
    };
    // A converted deployment that has not passed the coordinated cutover gate
    // is deliberately not ready: it serves read-only diagnostics only while the
    // operator completes the switch (issue #192).
    let cutover = if state.db().inventory_cutover().business_writes_blocked() {
        ReadyComponent::not_ready("inventory_cutover", "cutover_not_resumed")
    } else {
        ReadyComponent::ready("inventory_cutover")
    };

    let components = vec![
        sqlite, owner, web_assets, shutdown, workers, corruption, cutover,
    ];
    let ready = components
        .iter()
        .all(|component| component.status == ReadyState::Ready);
    let status_code = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status_code,
        Json(ReadyResponse {
            status: if ready {
                ReadyState::Ready
            } else {
                ReadyState::NotReady
            },
            components,
        }),
    )
}
