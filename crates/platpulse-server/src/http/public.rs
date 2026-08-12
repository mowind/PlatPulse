//! `/api/public/v1` route group — the Home Public Projection.
//!
//! Middleware and DTO namespace are independent from Admin and Agent: DTOs
//! live in this module and are never reused as Admin DTOs by runtime field
//! filtering (design §13.1). Phase 0 registers the group and its middleware;
//! routes and DTOs arrive with Phase 1 tickets.

use axum::Router;
use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;

use super::{AppState, ROUTE_GROUP_HEADER, api_not_found};

async fn group_middleware(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(ROUTE_GROUP_HEADER, HeaderValue::from_static("public"));
    response
}

pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .fallback(api_not_found)
        .layer(axum::middleware::from_fn(group_middleware))
}
