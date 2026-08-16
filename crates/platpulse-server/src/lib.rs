//! `platpulse-server` — the central collection point and trust boundary for
//! PlatPulse: report ingestion, SQLite projections, auth, alerts, and Web
//! asset hosting.
//!
//! Phase 0 provided the thin-binary/library split, the Server-local SQLite
//! startup/migration harness, the HTTP route group skeleton with health
//! routes and Web asset hosting, and the OpenAPI 3 document. P1-01 adds
//! local initialization (`init`), first-Owner creation, human sessions and
//! the private Home/Admin gates. Keep startup logic in this library so it
//! can be exercised from tests; `main.rs` stays a thin entry point.

pub mod alerts;
pub mod auth;
pub mod backup;
pub mod cli;
pub mod config;
pub mod database;
pub mod doctor;
pub mod enrollment;
pub mod file_security;
pub mod geo;
pub mod http;
pub mod init;
pub mod network;
pub mod notifications;
pub mod openapi;
pub mod operations;
pub mod peer_history;
pub mod redaction;
pub mod restore;
pub mod retention;
pub mod secrets;

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

pub fn validate_listen_address_with_proxy(
    addr: std::net::SocketAddr,
    trusted_proxy_cidrs: &[ipnet::IpNet],
    trusted_proxy_scheme: Option<&str>,
) -> Result<(), ListenAddressError> {
    if addr.ip().is_loopback()
        || (!trusted_proxy_cidrs.is_empty() && trusted_proxy_scheme.is_some())
    {
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
    fn configured_proxy_does_not_change_legacy_guard() {
        let cidr: ipnet::IpNet = "10.0.0.0/8".parse().unwrap();
        assert!(
            validate_listen_address_with_proxy(
                "0.0.0.0:8080".parse().unwrap(),
                &[cidr],
                Some("https")
            )
            .is_ok()
        );
        assert!(
            validate_listen_address_with_proxy("0.0.0.0:8080".parse().unwrap(), &[], Some("https"))
                .is_err()
        );
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
