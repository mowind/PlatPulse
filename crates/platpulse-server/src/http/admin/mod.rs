//! Owner-only Agent/Node current observation diagnostics.
use super::{AppState, ROUTE_GROUP_HEADER, api_not_found};
use crate::http::AuthenticatedSession;
use crate::http::realtime;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio_stream::Stream;
use utoipa::ToSchema;

// Server-owned Agent Attention DTOs and the occurrence/evidence boundary live
// in the dedicated module (issue #172); they are re-exported here because the
// Admin OpenAPI document registers them under this path.
pub use crate::attention::{AttentionItem, AttentionKind, AttentionSeverity, AttentionSubjectKind};

mod agent_removal;
mod agents;
mod credentials;
mod diagnostics;
mod events;
mod geo;
mod history;
mod networks;
mod node_purge;
mod nodes;
mod overview;
#[cfg(test)]
mod test_helpers;
mod transfers;
mod visibility;

pub(crate) use agent_removal::*;
pub(crate) use agents::*;
pub(crate) use credentials::*;
pub(crate) use diagnostics::*;
pub(crate) use events::*;
pub(crate) use geo::*;
pub(crate) use history::*;
pub(crate) use networks::*;
pub(crate) use node_purge::*;
pub(crate) use nodes::*;
pub(crate) use overview::*;
pub(crate) use transfers::*;
pub(crate) use visibility::*;

pub(crate) fn mutation_error(
    request_id: &str,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    (
        status,
        Json(crate::http::ApiErrorBody::new(code, message, request_id)),
    )
        .into_response()
}

/// The answer to a failed Admin storage statement. The error is logged at
/// `error` inside the request span (which carries `request_id` and
/// `route`) with the call site in the message, runtime corruption is
/// latched (ADR 0008), and a unique or check violation answers
/// `storage_conflict` so a schema or invariant bug is not mistaken for an
/// outage. A failed read is never answered as absent: callers that check
/// existence come here, not to their not-found arm.
#[track_caller]
pub(crate) fn storage_failure(
    state: &AppState,
    request_id: &str,
    failure: &sqlx::Error,
) -> Response {
    let location = std::panic::Location::caller();
    state.note_database_error(failure);
    tracing::error!(
        error = %failure,
        "admin storage error at {}:{}",
        location.file(),
        location.line()
    );
    if crate::database::is_constraint_violation(failure) {
        return mutation_error(
            request_id,
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_conflict",
            "Server storage rejected the change",
        );
    }
    mutation_error(
        request_id,
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Server database is unavailable",
    )
}

/// Shared browser mutation trust boundary (design §10.2, webui.md §6.4): a
/// JSON content type, a same-origin request, and the session CSRF header are
/// all required before any Admin mutation parses its body. A malformed body
/// must never bypass these checks or produce a framework-generated error
/// with a different envelope.
fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(crate) fn mutation_guard_ok(
    headers: &HeaderMap,
    state: &AppState,
    session: &AuthenticatedSession,
) -> bool {
    let content_type_valid = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"));
    let origin_valid = state.auth().origin_matches(headers.get(header::ORIGIN));
    let csrf_valid = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| bool::from(token.as_bytes().ct_eq(session.0.csrf_token.as_bytes())));
    content_type_valid && origin_valid && csrf_valid
}

fn redact_optional_message(value: Option<String>) -> Option<String> {
    value.map(|message| crate::redaction::redact_sensitive(&message))
}

/// Redacted destination summary (webui.md §7, §9, `PATTERN-REDACTED-DETAIL`):
/// scheme and host with the port masked; path, query, and credentials are
/// never exposed.
fn redact_endpoint(endpoint: &str) -> String {
    let Some(scheme_end) = endpoint.find("://") else {
        return "redacted".to_owned();
    };
    let mut result = endpoint[..scheme_end + 3].to_owned();
    let authority = endpoint[scheme_end + 3..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let authority = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let (host, has_port) = if let Some(host) = authority.strip_prefix('[') {
        match host.find(']') {
            Some(end) => (&host[..end], host[end + 1..].starts_with(':')),
            None => (host, false),
        }
    } else if authority.matches(':').count() == 1 {
        let (host, _) = authority.split_once(':').unwrap_or((authority, ""));
        (host, true)
    } else {
        (authority, false)
    };
    let host = if host.parse::<std::net::IpAddr>().is_ok() {
        "[REDACTED_IP]"
    } else {
        host
    };
    result.push_str(host);
    if has_port {
        result.push_str(":****");
    }
    result
}

/// Server-owned lifecycle guidance (design §4.6): the Server never changes
/// Node lifecycle remotely; the latest valid Agent Inventory is the only
/// authority for Active/Retired.
fn lifecycle_guidance(lifecycle: &str) -> &'static str {
    if lifecycle == "retired" {
        "Retired: the latest Agent Inventory no longer declares this Node. Reactivation requires declaring the same Node ID again in the Agent-local TOML configuration and submitting a new Inventory; the Server never changes Node lifecycle remotely."
    } else {
        "Active: present in the latest valid Agent Inventory. The Agent-local configuration stays authoritative for this Node; the Server never pushes Endpoint or lifecycle changes."
    }
}

/// Browser mutation trust boundary shared by every Admin security
/// mutation (design §10.2, §13.3): same-origin, JSON content type (when the
/// mutation carries a body), and a CSRF token matching the session —
/// checked before any body parsing. Bodyless mutations (e.g. revoke) do
/// not require the JSON content type because the generated browser client
/// omits it when there is no body.
pub(crate) fn mutation_guard(
    headers: &HeaderMap,
    principal: &super::AuthenticatedSession,
    auth: &crate::auth::AuthConfig,
    request_id: &super::RequestId,
    require_json_body: bool,
) -> Option<Response> {
    let content_type_valid = !require_json_body
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"));
    let origin_valid = auth.origin_matches(headers.get(header::ORIGIN));
    let csrf_valid = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| bool::from(token.as_bytes().ct_eq(principal.0.csrf_token.as_bytes())));
    if !content_type_valid || !origin_valid || !csrf_valid {
        return Some(mutation_error(
            &request_id.0,
            StatusCode::FORBIDDEN,
            "csrf_validation_failed",
            "mutation validation failed",
        ));
    }
    None
}

pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/events", get(admin_events))
        .route("/overview", get(overview))
        .route("/geo", get(admin_geo_status))
        .route("/geo/provider", put(update_geo_provider))
        .route("/geo/refresh", post(trigger_geo_refresh))
        .route("/nodes", get(admin_nodes))
        .route("/nodes/{node_id}", get(admin_node_detail))
        .route("/nodes/{node_id}/metadata", put(set_node_metadata))
        .route(
            "/nodes/{node_id}/purge",
            get(admin_node_purge_preview).post(purge_node),
        )
        .route("/nodes/{node_id}/peer-churn", get(admin_node_peer_churn))
        .route(
            "/nodes/{node_id}/peer-history",
            get(admin_node_peer_history),
        )
        .route("/nodes/{node_id}/history", get(admin_node_history))
        .route(
            "/nodes/{node_id}/metric-history",
            get(admin_node_metric_history),
        )
        // The recorded sync and consensus state changes of that Node (#217):
        // state evidence, not a number, so it is a family of its own and it is
        // never inferred from the Block history next to it (design §11.4).
        .route(
            "/nodes/{node_id}/state-history",
            get(admin_node_state_history),
        )
        // The one UTC window every evidence route above is read inside (#220):
        // it answers where each family's evidence is and how complete it is,
        // never a second copy of the evidence itself (design §11.4).
        .route(
            "/nodes/{node_id}/investigation",
            get(admin_node_investigation),
        )
        // The Agent's shared Host series: the same evidence every Node on that
        // Host reads, answered from the Agent page and from any Node page
        // (design §11.6).
        .route(
            "/nodes/{node_id}/host-metric-history",
            get(admin_node_host_metric_history),
        )
        .route(
            "/agents/{agent_id}/metric-history",
            get(admin_agent_metric_history),
        )
        // The mount paths that Agent's Host evidence is stored under (#216),
        // because the path is the identity of a storage series (design §11.6).
        .route(
            "/agents/{agent_id}/storage-mounts",
            get(admin_agent_storage_mounts),
        )
        .route("/networks", get(admin_networks))
        .route("/networks/{network_key}", get(admin_network_detail))
        .route("/networks", post(create_network))
        .route("/networks/{network_key}", put(update_network))
        .route("/agents", get(diagnostics))
        .route("/agents/{agent_id}", get(admin_agent_detail))
        .route("/agents/{agent_id}/metadata", put(set_agent_metadata))
        .route(
            "/agents/{agent_id}/attention/acknowledgments",
            post(acknowledge_agent_attention),
        )
        .route("/agents/{agent_id}/audit", get(admin_agent_audit))
        .route("/agents/enroll-token", post(admin_enrollment_token))
        .route("/agents/{agent_id}/recover", post(admin_recovery_token))
        .route(
            "/agents/{agent_id}/credentials/rotate",
            post(admin_rotate_credential),
        )
        .route(
            "/agents/{agent_id}/credentials/{credential_id}/revoke",
            post(admin_revoke_credential),
        )
        .route(
            "/agents/{agent_id}/removal",
            get(admin_agent_removal_preview).post(remove_agent),
        )
        .route("/nodes/{node_id}/visibility", put(set_visibility))
        .route("/nodes/{node_id}/transfers", get(admin_node_transfers))
        .route("/nodes/{node_id}/transfers", post(create_node_transfer))
        .route("/transfers/{transfer_id}", get(admin_transfer_detail))
        .route(
            "/transfers/{transfer_id}/cancel",
            post(cancel_node_transfer),
        )
        .fallback(api_not_found)
        .layer(axum::middleware::from_fn(group_middleware))
}

async fn group_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        ROUTE_GROUP_HEADER,
        axum::http::HeaderValue::from_static("admin"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_endpoint_strips_userinfo_and_masks_the_port() {
        assert_eq!(
            redact_endpoint("ws://127.0.0.1:6790"),
            "ws://[REDACTED_IP]:****"
        );
        assert_eq!(
            redact_endpoint("wss://user:sekret@example.org:6790/path?token=x"),
            "wss://example.org:****"
        );
        assert_eq!(redact_endpoint("no-scheme"), "redacted");
    }
}
