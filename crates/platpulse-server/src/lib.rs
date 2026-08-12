//! `platpulse-server` — the central collection point and trust boundary for
//! PlatPulse: report ingestion, SQLite projections, auth, alerts, and Web
//! asset hosting.
//!
//! Phase 0 provides the thin-binary/library split, the Server-local SQLite
//! startup/migration harness, the HTTP route group skeleton with health
//! routes and Web asset hosting, and the OpenAPI 3 document. Auth, Report
//! Ingestion, and real Public/Admin/Agent routes arrive with Phase 1 tickets.
//! Keep startup logic in this library so it can be exercised from tests;
//! `main.rs` stays a thin entry point.

pub mod database;
pub mod http;
pub mod openapi;

pub use database::{
    DEFAULT_BUSY_TIMEOUT, JournalMode, SERVER_MIGRATOR, SERVER_SCHEMA_VERSION,
    SERVER_WRITE_CONNECTIONS, ServerDatabase, ServerDatabaseConfig, ServerDatabaseError,
    SqlitePragmas, initialize,
};
pub use http::{ApiError, ApiErrorBody, AppState};

/// Name of the server binary, as declared in `Cargo.toml`.
pub const BINARY_NAME: &str = env!("CARGO_PKG_NAME");

/// Version of the server crate, as declared in `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One-line identity printed at process start.
pub fn startup_version_line() -> String {
    format!("{BINARY_NAME} {VERSION}")
}

/// Refusal to bind a listener outside the loopback interface.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ListenAddressError {
    #[error(
        "refusing to bind {0}: non-loopback binds require TLS or a trusted reverse proxy (design §19.4)"
    )]
    NonLoopback(std::net::SocketAddr),
}

/// Startup guard from design §19.4: the Server must not expose plaintext
/// service on a non-loopback interface until TLS/trusted-proxy
/// configuration exists (Phase 1).
pub fn validate_listen_address(addr: std::net::SocketAddr) -> Result<(), ListenAddressError> {
    if addr.ip().is_loopback() {
        Ok(())
    } else {
        Err(ListenAddressError::NonLoopback(addr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_line_names_the_server() {
        assert!(startup_version_line().starts_with("platpulse-server "));
    }

    #[test]
    fn loopback_listen_addresses_are_accepted() {
        for addr in [
            "127.0.0.1:8080".parse().unwrap(),
            "[::1]:8080".parse().unwrap(),
        ] {
            assert_eq!(validate_listen_address(addr), Ok(()));
        }
    }

    #[test]
    fn non_loopback_listen_addresses_are_refused() {
        for addr in [
            "0.0.0.0:8080".parse().unwrap(),
            "192.168.1.10:8080".parse().unwrap(),
        ] {
            assert_eq!(
                validate_listen_address(addr),
                Err(ListenAddressError::NonLoopback(addr))
            );
        }
    }
}
