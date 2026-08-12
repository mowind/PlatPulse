//! `platpulse-server` — the central collection point and trust boundary for
//! PlatPulse: report ingestion, SQLite projections, auth, alerts, and Web
//! asset hosting.
//!
//! Phase 0 provides the thin-binary/library split only: HTTP/SSE routes,
//! auth, Report Ingestion, SQLite migrations, and Web asset serving arrive in
//! later tickets. Keep startup logic in this library so it can be exercised
//! from tests; `main.rs` stays a thin entry point.

/// Name of the server binary, as declared in `Cargo.toml`.
pub const BINARY_NAME: &str = env!("CARGO_PKG_NAME");

/// Version of the server crate, as declared in `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One-line identity printed at process start.
pub fn startup_version_line() -> String {
    format!("{BINARY_NAME} {VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_line_names_the_server() {
        assert!(startup_version_line().starts_with("platpulse-server "));
    }
}
