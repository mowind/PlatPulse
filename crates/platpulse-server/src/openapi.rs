//! OpenAPI 3 document for the Server's HTTP surface.
//!
//! The full document (including the `agent` tag) is exported through the
//! `--print-openapi` CLI for `docs/openapi/openapi.json`; the browser client
//! is generated from a filtered copy that drops `agent` operations and their
//! schemas (design §13.4). Public and Admin routes use independent tags.

use utoipa::Modify;
use utoipa::OpenApi;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Components, OpenApi as OpenApiDoc};

use crate::http::health;

/// Document the Agent API's Bearer authentication (design §12.5/§12.6):
/// `POST /api/agent/v1/enroll` authenticates a one-time Enrollment Token
/// (`pp_enroll_…`), every other Agent route an Agent Credential
/// (`pp_agent_…`). Human routes use cookie sessions and must stay
/// unmarked, so the scheme is attached per operation instead of globally.
struct AgentBearerScheme;

impl Modify for AgentBearerScheme {
    fn modify(&self, openapi: &mut OpenApiDoc) {
        openapi.components.get_or_insert_with(Components::new).add_security_scheme(
            "agent_bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("Agent Credential (pp_agent_…) or one-time Enrollment Token (pp_enroll_…)")
                    .description(Some(
                        "Agent API Bearer token. Enrollment exchanges a one-time pp_enroll_… token for an Agent identity; all other Agent routes require the issued pp_agent_… credential.",
                    ))
                    .build(),
            ),
        );

        if let Some(operation) = openapi
            .paths
            .paths
            .get_mut("/api/agent/v1/enroll")
            .and_then(|path| path.post.as_mut())
        {
            operation.security = Some(vec![utoipa::openapi::security::SecurityRequirement::new(
                "agent_bearer",
                Vec::<String>::new(),
            )]);
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(
        health::live,
        health::ready,
        crate::http::public::login_handler,
        crate::http::public::logout_handler,
        crate::http::public::session_handler,
        crate::http::public::public_networks,
        crate::http::public::public_network,
        crate::http::public::public_node_detail,
        crate::http::admin::set_visibility,
        crate::http::admin::diagnostics,
        crate::http::agent::enroll_handler,
    ),
    components(schemas(
        crate::http::ApiErrorBody,
        crate::http::ApiError,
        crate::http::public::LoginRequest,
        crate::http::public::SessionResponse,
        crate::http::public::SessionProjection,
        crate::http::public::PublicNetwork,
        crate::http::public::PublicNode,
        crate::http::admin::VisibilityRequest,
        crate::http::admin::VisibilityResponse,
        crate::http::admin::AgentDiagnostic,
        crate::http::admin::HostDiagnostic,
        crate::http::admin::HostComponentDiagnostic,
        crate::http::admin::RpcDiagnostic,
        crate::http::admin::SyncDiagnostic,
        crate::http::admin::ConsensusDiagnostic,
        crate::http::admin::ComponentDiagnostic,
        crate::http::agent::EnrollResponse,
    )),
    modifiers(&AgentBearerScheme),
    tags(
        (name = "system", description = "Operational health endpoints"),
        (name = "public", description = "Home Public Projection routes and DTOs"),
        (name = "admin", description = "Owner administration routes and DTOs"),
        (name = "agent", description = "Agent enrollment, recovery, and report ingestion; never generated into the browser client"),
    )
)]
pub struct ApiDoc;

/// Serialize the OpenAPI document as pretty JSON. Deterministic for the same
/// code, which lets CI regenerate `docs/openapi/openapi.json` and verify no
/// diff.
pub fn spec_json() -> String {
    ApiDoc::openapi()
        .to_pretty_json()
        .expect("OpenAPI document must serialize")
}
