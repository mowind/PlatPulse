//! The Agent's process-wide HTTP client (issue #244).
//!
//! Every Agent→Server request that is not part of the Report delivery spool
//! goes through one `reqwest::Client`, so that Client owns a single connection
//! pool and a keep-alive connection survives across Collection ticks. Before
//! #244 the Server time exchange built a Client per tick: the pool was dropped
//! with the Client, so nginx saw one upstream connection per request.
//!
//! Per-request budgets belong on the `RequestBuilder`, never here: a
//! client-wide default timeout would leak one caller's budget into every other
//! caller's request.
//!
//! # Why a process-wide client rather than an injected one
//!
//! AGENTS.md asks for dependencies to be injected through explicit
//! constructors, and `exchange_server_time` does take `&reqwest::Client` so the
//! seam exists. Its production caller, however, sits at the bottom of the
//! Collection call chain (`cli::run` → `collect_and_persist_*` → the exchange),
//! and issue #244 sanctions the process-wide `OnceLock` as the alternative to
//! threading one client through every layer. Injecting at a real composition
//! root is the better shape if that chain ever grows one.

use std::sync::OnceLock;

use thiserror::Error;

/// The shared client could not be built (TLS backend initialization).
#[derive(Debug, Error)]
#[error("failed to build the shared Agent HTTP client: {0}")]
pub struct SharedClientError(String);

/// Built lazily on first use and kept for the life of the process.
static SHARED_CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

fn build_shared_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(format!("platpulse-agent/{}", crate::VERSION))
        .build()
        .map_err(|error| error.to_string())
}

/// The one `reqwest::Client` behind the Agent's non-report Server requests.
///
/// The build error is cached alongside the client: a TLS backend that fails to
/// initialize will fail the same way on every retry, and re-attempting it keeps
/// no state worth keeping.
pub fn shared_client() -> Result<&'static reqwest::Client, SharedClientError> {
    SHARED_CLIENT
        .get_or_init(build_shared_client)
        .as_ref()
        .map_err(|message| SharedClientError(message.clone()))
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::*;

    #[test]
    fn shared_client_is_one_process_wide_instance() {
        let first = shared_client().expect("the shared client builds");
        let second = shared_client().expect("the shared client builds");
        assert!(ptr::eq(first, second));
    }

    #[tokio::test]
    async fn shared_client_keeps_its_user_agent_and_reuses_one_connection() {
        let server =
            crate::http_test_support::KeepAliveServer::start("{\"ok\":true}".to_owned()).await;
        for _ in 0..3 {
            let response = shared_client()
                .expect("the shared client builds")
                .get(server.url("/probe"))
                .send()
                .await
                .expect("the probe request succeeds");
            assert_eq!(response.status(), reqwest::StatusCode::OK);
        }
        assert_eq!(server.request_count(), 3);
        assert_eq!(server.connection_count(), 1);
        assert_eq!(
            server.user_agents(),
            vec![format!("platpulse-agent/{}", crate::VERSION); 3]
        );
    }
}
