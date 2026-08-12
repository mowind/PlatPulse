//! Owner-only Agent/Node Inventory diagnostics.
use super::{AppState, ROUTE_GROUP_HEADER, api_not_found};
use axum::extract::{Extension, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentDiagnostic {
    pub agent_id: String,
    pub agent_epoch: i64,
    pub last_report_sequence: Option<i64>,
    pub nodes: Vec<NodeDiagnostic>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NodeDiagnostic {
    pub node_id: String,
    pub network_key: String,
    pub display_name: Option<String>,
    pub lifecycle: String,
    pub inventory_revision: i64,
    pub visibility: String,
}

async fn diagnostics(
    State(state): State<AppState>,
    Extension(_session): Extension<super::AuthenticatedSession>,
) -> impl IntoResponse {
    let agents = sqlx::query_as::<_, (String, i64, Option<i64>)>(
        "SELECT agent_id, agent_epoch, last_report_sequence FROM agents ORDER BY agent_id",
    )
    .fetch_all(state.db().pool())
    .await
    .unwrap_or_default();
    let mut result = Vec::with_capacity(agents.len());
    for (agent_id, agent_epoch, last_report_sequence) in agents {
        let nodes = sqlx::query_as::<_, (String, String, Option<String>, String, i64, String)>("SELECT node_id, network_key, display_name, lifecycle, inventory_revision, visibility FROM nodes WHERE agent_id = ? ORDER BY node_id")
            .bind(&agent_id).fetch_all(state.db().pool()).await.unwrap_or_default()
            .into_iter().map(|(node_id, network_key, display_name, lifecycle, inventory_revision, visibility)| NodeDiagnostic { node_id, network_key, display_name, lifecycle, inventory_revision, visibility }).collect();
        result.push(AgentDiagnostic {
            agent_id,
            agent_epoch,
            last_report_sequence,
            nodes,
        });
    }
    Json(result)
}

pub fn router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/agents", get(diagnostics))
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
