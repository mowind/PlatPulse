//! The request boundary of the report routes (method, content type, encoding
//! and body size) and the report routers.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn};
use axum::response::Response;

use crate::http::{AppState, ROUTE_GROUP_HEADER, RequestId};

use super::*;

fn valid_report_content_type(value: Option<&axum::http::HeaderValue>) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|media_type| {
                media_type.trim().eq_ignore_ascii_case("application/json")
            })
        })
}

fn report_content_encoding_supported(value: Option<&axum::http::HeaderValue>) -> bool {
    value.is_none()
}

async fn report_request_boundary(request: Request, next: Next) -> Response {
    if request.method() != axum::http::Method::POST {
        return error_from_request(
            &request,
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "report endpoint requires POST",
        );
    }
    if !valid_report_content_type(request.headers().get(axum::http::header::CONTENT_TYPE)) {
        return error_from_request(
            &request,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_content_type",
            "Agent reports require application/json",
        );
    }
    if !report_content_encoding_supported(
        request.headers().get(axum::http::header::CONTENT_ENCODING),
    ) {
        return error_from_request(
            &request,
            StatusCode::BAD_REQUEST,
            "content_encoding_unsupported",
            "compressed Agent reports are not supported",
        );
    }
    next.run(request).await
}

async fn body_size_boundary(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, platpulse_core::protocol::MAX_REPORT_BODY_BYTES + 1).await {
        Ok(bytes) if validate_report_body_size(bytes.len()).is_ok() => bytes,
        Ok(_) => {
            let request = Request::from_parts(parts, Body::empty());
            return error_from_request(
                &request,
                StatusCode::PAYLOAD_TOO_LARGE,
                "report_too_large",
                "Agent report exceeds the protocol size limit",
            );
        }
        Err(_) => {
            let request = Request::from_parts(parts, Body::empty());
            return error_from_request(
                &request,
                StatusCode::PAYLOAD_TOO_LARGE,
                "report_too_large",
                "Agent report exceeds the protocol size limit",
            );
        }
    };
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

fn error_from_request(
    request: &Request,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| &*id.0)
        .unwrap_or("unknown");
    error(request_id, status, code, message)
}

fn report_layers(router: Router<AppState>) -> Router<AppState> {
    router
        .layer(from_fn(body_size_boundary))
        .layer(from_fn(report_request_boundary))
        .layer(from_fn(
            |request: axum::extract::Request, next: Next| async move {
                let mut response = next.run(request).await;
                response.headers_mut().insert(
                    ROUTE_GROUP_HEADER,
                    axum::http::HeaderValue::from_static("agent"),
                );
                response
            },
        ))
}

pub(crate) fn router() -> Router<AppState> {
    report_layers(Router::<AppState>::new().route("/reports", axum::routing::post(handler)))
}

/// v2 report ingestion router, nested at `/api/agent/v2`.
pub(crate) fn router_v2() -> Router<AppState> {
    report_layers(Router::<AppState>::new().route("/reports", axum::routing::post(handler_v2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::http::StatusCode;

    #[test]
    pub(super) fn boundary_rejects_method_content_type_and_encoding() {
        use axum::http::HeaderValue;
        assert!(valid_report_content_type(Some(&HeaderValue::from_static(
            "application/json; charset=utf-8"
        ))));
        assert!(!valid_report_content_type(Some(&HeaderValue::from_static(
            "text/plain"
        ))));
        assert!(report_content_encoding_supported(None));
        assert!(!report_content_encoding_supported(Some(
            &HeaderValue::from_static("gzip")
        )));
        assert!(validate_report_body_size(platpulse_core::protocol::MAX_REPORT_BODY_BYTES).is_ok());
        assert_eq!(
            validate_report_body_size(platpulse_core::protocol::MAX_REPORT_BODY_BYTES + 1),
            Err(StatusCode::PAYLOAD_TOO_LARGE)
        );
    }
}
