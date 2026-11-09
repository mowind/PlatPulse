//! Owner-only Admin realtime event stream.
use super::*;

#[utoipa::path(
    get,
    path = "/api/admin/v1/events",
    tag = "admin",
    responses((
        status = 200,
        description = "Owner-authenticated Admin invalidation stream. Each `invalidation` or `reset` event carries one Invalidation Event as JSON data.",
        content_type = "text/event-stream",
        body = realtime::Invalidation<realtime::AdminResource>
    ))
)]
pub(super) async fn admin_events(
    State(state): State<AppState>,
    Extension(_session): Extension<crate::http::AuthenticatedSession>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<realtime::CursorQuery>,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let cursor = realtime::parse_cursor(
        headers.get("last-event-id").and_then(|v| v.to_str().ok()),
        query.after.as_deref(),
    );
    Sse::new(state.admin_realtime().stream_with_session(
        cursor,
        state.database(),
        _session.0.session_id.clone(),
        _session.0.role.clone(),
        _session.0.expires_at,
    ))
    .keep_alive(
        KeepAlive::new()
            .interval(realtime::keepalive_interval())
            .text("keepalive"),
    )
}
