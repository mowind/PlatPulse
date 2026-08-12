//! Owner-only Agent/Node current observation diagnostics.
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
    pub host: Option<HostDiagnostic>,
    pub nodes: Vec<NodeDiagnostic>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct HostDiagnostic {
    pub cpu_percent: Option<f64>,
    pub memory_total_bytes: Option<i64>,
    pub memory_used_bytes: Option<i64>,
    pub load1: Option<f64>,
    pub load5: Option<f64>,
    pub load15: Option<f64>,
    pub network_rx_bytes_per_sec: Option<i64>,
    pub network_tx_bytes_per_sec: Option<i64>,
    pub updated_at: String,
    pub components: Vec<HostComponentDiagnostic>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct HostComponentDiagnostic {
    pub component: String,
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: i64,
    pub value_revision: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RpcDiagnostic {
    pub client_version: Option<String>,
    pub namespaces: Vec<String>,
    pub methods: Vec<String>,
    pub state: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempted_at: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub state_revision: Option<i64>,
    pub value_revision: Option<i64>,
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
    pub rpc: Option<RpcDiagnostic>,
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
        let host_components = sqlx::query_as::<_, (String, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64)>(
            "SELECT component_key, state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision FROM component_status WHERE agent_id = ? AND scope = 'host' ORDER BY component_key",
        )
        .bind(&agent_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(component, state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision)| HostComponentDiagnostic {
            component, state, error_code, error_message, attempted_at, observed_at, received_at, state_revision, value_revision,
        })
        .collect::<Vec<_>>();
        let host = sqlx::query_as::<_, (Option<f64>, Option<i64>, Option<i64>, Option<f64>, Option<f64>, Option<f64>, Option<i64>, Option<i64>, String)>(
            "SELECT cpu_percent, memory_total_bytes, memory_used_bytes, load1, load5, load15, network_rx_bytes_per_sec, network_tx_bytes_per_sec, updated_at FROM current_host_observations WHERE agent_id = ?",
        )
        .bind(&agent_id)
        .fetch_optional(state.db().pool())
        .await
        .ok()
        .flatten()
        .map(|(cpu_percent, memory_total_bytes, memory_used_bytes, load1, load5, load15, network_rx_bytes_per_sec, network_tx_bytes_per_sec, updated_at)| HostDiagnostic {
            cpu_percent, memory_total_bytes, memory_used_bytes, load1, load5, load15, network_rx_bytes_per_sec, network_tx_bytes_per_sec, updated_at, components: host_components,
        });
        let rows = sqlx::query_as::<_, (String, String, Option<String>, String, i64, String)>(
            "SELECT node_id, network_key, display_name, lifecycle, inventory_revision, visibility FROM nodes WHERE agent_id = ? ORDER BY node_id",
        )
        .bind(&agent_id)
        .fetch_all(state.db().pool())
        .await
        .unwrap_or_default();
        let mut nodes = Vec::with_capacity(rows.len());
        for (node_id, network_key, display_name, lifecycle, inventory_revision, visibility) in rows
        {
            let rpc = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>, Option<i64>)>(
                "SELECT c.rpc_client_version, s.state, s.error_code, s.error_message, s.attempted_at, s.observed_at, s.received_at, s.state_revision, s.value_revision FROM component_status s LEFT JOIN current_node_chain_observations c ON c.node_id = s.node_id WHERE s.node_id = ? AND s.component_key = 'rpc'",
            )
            .bind(&node_id)
            .fetch_optional(state.db().pool())
            .await
            .ok()
            .flatten();
            let rpc = if let Some((
                client_version,
                component_state,
                error_code,
                error_message,
                attempted_at,
                observed_at,
                received_at,
                state_revision,
                value_revision,
            )) = rpc
            {
                let namespaces = sqlx::query_scalar::<_, String>("SELECT namespace FROM current_node_rpc_namespaces WHERE node_id = ? ORDER BY namespace")
                    .bind(&node_id).fetch_all(state.db().pool()).await.unwrap_or_default();
                let methods = sqlx::query_scalar::<_, String>(
                    "SELECT method FROM current_node_rpc_methods WHERE node_id = ? ORDER BY method",
                )
                .bind(&node_id)
                .fetch_all(state.db().pool())
                .await
                .unwrap_or_default();
                Some(RpcDiagnostic {
                    client_version,
                    namespaces,
                    methods,
                    state: component_state,
                    error_code,
                    error_message,
                    attempted_at,
                    observed_at,
                    received_at,
                    state_revision,
                    value_revision,
                })
            } else {
                None
            };
            nodes.push(NodeDiagnostic {
                node_id,
                network_key,
                display_name,
                lifecycle,
                inventory_revision,
                visibility,
                rpc,
            });
        }
        result.push(AgentDiagnostic {
            agent_id,
            agent_epoch,
            last_report_sequence,
            host,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::AuthenticatedSession;
    use axum::body::to_bytes;
    use serde_json::Value;
    use tempfile::tempdir;
    use time::OffsetDateTime;

    #[tokio::test]
    async fn host_diagnostics_include_failure_and_freshness_fields() {
        let dir = tempdir().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pepper_path = dir.path().join("pepper");
        crate::secrets::create_pepper_file(&pepper_path).unwrap();
        let auth = crate::auth::AuthConfig::development(
            crate::secrets::load_pepper_file(&pepper_path).unwrap(),
            "http://127.0.0.1:8080".to_owned(),
        );
        let state = AppState::new(database, None, auth);
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES (?, 1, ?, ?)")
            .bind("agent-host-test")
            .bind("2026-01-01T00:00:00Z")
            .bind("2026-01-01T00:00:00Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO current_host_observations (agent_id, updated_at) VALUES (?, ?)")
            .bind("agent-host-test")
            .bind("2026-01-01T00:00:02Z")
            .execute(state.db().pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO component_status (agent_id, scope, scope_key, component_key, state, attempted_at, observed_at, received_at, state_revision, value_revision, error_code, error_message) VALUES (?, 'host', 'host', 'cpu_percent', 'error', ?, ?, ?, 2, 1, ?, ?)")
            .bind("agent-host-test")
            .bind("2026-01-01T00:00:01Z")
            .bind("2025-12-31T23:59:00Z")
            .bind("2026-01-01T00:00:02Z")
            .bind("rpc_unreachable")
            .bind("RPC probe failed")
            .execute(state.db().pool())
            .await
            .unwrap();
        let session = crate::auth::SessionInfo {
            session_id: "session".to_owned(),
            user_id: "owner".to_owned(),
            username: "owner".to_owned(),
            role: "owner".to_owned(),
            created_at: OffsetDateTime::now_utc(),
            last_seen_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".to_owned(),
        };
        let response = diagnostics(State(state), Extension(AuthenticatedSession(session)))
            .await
            .into_response();
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let component = &value[0]["host"]["components"][0];
        assert_eq!(component["state"], "error");
        assert_eq!(component["error_code"], "rpc_unreachable");
        assert_eq!(component["attempted_at"], "2026-01-01T00:00:01Z");
        assert_eq!(component["observed_at"], "2025-12-31T23:59:00Z");
        assert_eq!(component["received_at"], "2026-01-01T00:00:02Z");
        assert_eq!(component["state_revision"], 2);
        assert_eq!(component["value_revision"], 1);
    }
}
