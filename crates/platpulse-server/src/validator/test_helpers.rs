//! Test-only fixtures shared by the Validator module tests.

use tempfile::tempdir;

use crate::network::create_network;

use async_trait::async_trait;
use std::collections::BTreeMap;

use crate::database::ServerDatabase;

use crate::validator::provider::{
    NetworkRanking, RankingProviderResult, ValidatorProvider, ValidatorProviderResult,
};

pub(super) async fn test_db() -> (tempfile::TempDir, ServerDatabase) {
    let dir = tempdir().unwrap();
    let db = crate::test_support::test_database_in(dir.path()).await;
    create_network(
        &db,
        "platon-mainnet",
        "Mainnet",
        "0x0000000000000000000000000000000000000000000000000000000000000001",
        1,
        1,
        "lat",
    )
    .await
    .unwrap();
    crate::auth::create_owner(
        &db,
        "owner",
        &crate::auth::hash_password(b"validator-test-password").unwrap(),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at) VALUES ('agent-1', 1, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')")
            .execute(db.pool())
            .await
            .unwrap();
    sqlx::query("INSERT INTO nodes (node_id, agent_id, network_key, lifecycle, visibility, inventory_revision, first_seen_at, updated_at, rpc_endpoint) VALUES ('node-1', 'agent-1', 'platon-mainnet', 'active', 'private', 1, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 'http://127.0.0.1:1')")
            .execute(db.pool())
            .await
            .unwrap();
    (dir, db)
}

#[derive(Default)]
pub(super) struct FakeProvider {
    pub(super) results: std::sync::Mutex<Vec<ValidatorProviderResult>>,
    pub(super) calls: std::sync::Mutex<Vec<(String, String)>>,
    pub(super) rankings: std::sync::Mutex<Vec<RankingProviderResult>>,
    pub(super) ranking_calls: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl ValidatorProvider for FakeProvider {
    fn source(&self) -> &str {
        "fake"
    }

    async fn fetch(&self, network_key: &str, validator_node_id: &str) -> ValidatorProviderResult {
        self.calls
            .lock()
            .unwrap()
            .push((network_key.to_owned(), validator_node_id.to_owned()));
        self.results.lock().unwrap().remove(0)
    }

    async fn fetch_ranking(&self, network_key: &str) -> RankingProviderResult {
        self.ranking_calls
            .lock()
            .unwrap()
            .push(network_key.to_owned());
        let mut rankings = self.rankings.lock().unwrap();
        if rankings.is_empty() {
            RankingProviderResult::Unsupported("fake ranking unsupported".to_owned())
        } else {
            rankings.remove(0)
        }
    }
}

pub(super) fn provider_node_id() -> String {
    format!("0x{}", "ab".repeat(64))
}

pub(super) fn deployments(base_url: &str, networks: &[&str]) -> BTreeMap<String, String> {
    networks
        .iter()
        .map(|network| ((*network).to_owned(), base_url.to_owned()))
        .collect()
}

pub(super) fn ranking_with(cohort_size: i64, entries: &[(&str, i64)]) -> RankingProviderResult {
    let entries = entries
        .iter()
        .map(|(node_id, rank)| ((*node_id).to_owned(), *rank))
        .collect();
    RankingProviderResult::Success(Box::new(NetworkRanking {
        entries,
        cohort_size,
    }))
}

pub(super) fn platscan_success(node_id: &str, status: i64) -> serde_json::Value {
    serde_json::json!({ "code": 0, "errMsg": "success", "data": { "nodeId": node_id, "status": status } })
}

#[derive(Clone)]
pub(super) struct RecordedMockRequest {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) content_type: Option<String>,
    pub(super) body: String,
}

pub(super) type MockPlatScanResponseQueue =
    std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<(u16, Vec<u8>)>>>;

#[derive(Clone)]
pub(super) struct MockPlatScanState {
    pub(super) requests: std::sync::Arc<std::sync::Mutex<Vec<RecordedMockRequest>>>,
    pub(super) responses: MockPlatScanResponseQueue,
    pub(super) delay_ms: u64,
}

pub(super) async fn mock_platscan_handler(
    axum::extract::State(state): axum::extract::State<MockPlatScanState>,
    request: axum::http::Request<axum::body::Body>,
) -> axum::response::Response<String> {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let content_type = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body_bytes = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&body_bytes).to_string();
    state.requests.lock().unwrap().push(RecordedMockRequest {
        method,
        path,
        content_type,
        body,
    });
    if state.delay_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(state.delay_ms)).await;
    }
    let (status, body) = state
        .responses
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or((500, Vec::new()));
    axum::response::Response::builder()
        .status(axum::http::StatusCode::from_u16(status).unwrap())
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(String::from_utf8_lossy(&body).to_string())
        .unwrap()
}

pub(super) async fn start_mock_platscan(
    responses: Vec<(u16, Vec<u8>)>,
    delay_ms: u64,
) -> (String, MockPlatScanState, tokio::task::JoinHandle<()>) {
    let state = MockPlatScanState {
        requests: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        responses: std::sync::Arc::new(std::sync::Mutex::new(responses.into())),
        delay_ms,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route(
            "/browser-server/staking/stakingDetails",
            axum::routing::post(mock_platscan_handler),
        )
        .route(
            "/browser-server/staking/aliveStakingList",
            axum::routing::post(mock_platscan_handler),
        )
        .with_state(state.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state, handle)
}
