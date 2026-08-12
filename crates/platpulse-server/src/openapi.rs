//! OpenAPI 3 document for the Server's HTTP surface.
//!
//! The full document (including the `agent` tag) is exported through the
//! `--print-openapi` CLI for `docs/openapi/openapi.json`; the browser client
//! is generated from a filtered copy that drops `agent` operations and their
//! schemas (design §13.4). Public and Admin routes use independent tags.

use utoipa::OpenApi;

use crate::http::health;

#[derive(OpenApi)]
#[openapi(
    paths(health::live, health::ready),
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
